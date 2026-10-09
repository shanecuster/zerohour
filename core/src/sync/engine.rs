//! One full sync: check the database, pull the server's changes, push ours.
//!
//! **Pull** (`_changes?since=<checkpoint>`), one page at a time. For each document:
//! - not ours (e.g. `_design/...`): skip;
//! - same `_rev` as we have recorded: it's our own write coming back, skip;
//! - deleted with CouchDB's own delete (a tombstone, e.g. from the Fauxton web UI): soft-delete
//!   the local copy and forget the rev, so our soft-deleted version gets pushed back;
//! - an item: merge with the local copy (`merge.rs`) and record its rev;
//! - a completion: add it if we don't have it, and record its rev.
//!
//! Each page and its checkpoint are saved in one SQLite transaction, so a crash can never move
//! the checkpoint past changes that weren't applied.
//!
//! **Push** (`_bulk_docs`): every item whose `updated_at` differs from the version we last
//! synced, and every completion the server doesn't have, sent with the `_rev` we know. A
//! "conflict" answer means another device wrote in between; we then pull and push again (up to
//! [`MAX_ROUNDS`] times; anything left simply goes in the next sync).
//!
//! **First sync** of an existing database is the same thing: the checkpoint is empty, so we
//! pull everything, then push everything the server doesn't have.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::couch::{BulkResult, Change, CouchClient, PAGE_SIZE};
use super::doc::{
    DocProblem, DocRef, completion_doc_id, completion_from_doc, completion_to_doc, item_doc_id,
    item_from_doc, item_to_doc,
};
use super::merge::{ItemMerge, merge_item};
use super::store::SyncTx;
use super::transport::Transport;
use super::{SyncError, SyncSettings, clock_skew, keys};
use crate::db::{self, Db, ts_to_db};
use crate::model::{Item, next_updated_at};

/// How many pull+push rounds one sync may take when pushes keep hitting conflicts.
const MAX_ROUNDS: usize = 3;

/// What a sync did, for the status line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Documents from the server that changed local data.
    pub pulled: usize,
    /// Documents written to the server.
    pub pushed: usize,
    /// Documents written by a newer zerohour that this build can't read (update this device).
    pub skipped_newer: usize,
    /// Documents that looked like ours but were malformed.
    pub skipped_invalid: usize,
    /// Pushes still rejected as conflicts after [`MAX_ROUNDS`]; retried next sync.
    pub conflicts_left: usize,
    /// Seconds this device's clock differs from the server's, if more than a minute.
    pub clock_skew_secs: Option<i64>,
}

/// Runs one full sync of `db` with the server behind `transport`.
///
/// `now` is the current time, passed in (instead of read from the clock) so tests control it.
pub fn sync_with(
    db: &mut Db,
    transport: &dyn Transport,
    settings: &SyncSettings,
    now: DateTime<Utc>,
) -> Result<SyncReport, SyncError> {
    settings.validate()?;
    let client = CouchClient::new(transport, settings.database.trim());

    // Revs and the checkpoint only mean something for the server + database they came from.
    // If the settings now point somewhere else, start that one like a first sync.
    let remote = settings.remote_key();
    if db.get_setting(keys::REMOTE)?.as_deref() != Some(remote.as_str()) {
        db.clear_sync_state()?;
        db.set_setting(keys::REMOTE, &remote)?;
    }

    let info = client.db_info()?;
    let mut report = SyncReport {
        clock_skew_secs: clock_skew(info.server_date.as_deref(), now),
        ..SyncReport::default()
    };
    for _round in 0..MAX_ROUNDS {
        pull(db, &client, now, &mut report)?;
        report.conflicts_left = push(db, &client, &mut report)?;
        if report.conflicts_left == 0 {
            break;
        }
    }
    db.set_setting(keys::LAST_OK, &ts_to_db(now))?;
    Ok(report)
}

/// Pulls and applies every page of `_changes` after the saved checkpoint.
fn pull(
    db: &mut Db,
    client: &CouchClient<'_>,
    now: DateTime<Utc>,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    loop {
        let since = db.get_setting(keys::SINCE)?;
        let page = client.changes(since.as_deref())?;
        db.sync_tx(|tx| {
            for change in &page.changes {
                apply_change(tx, change, now, report)?;
            }
            // Saved in the same transaction as the changes themselves.
            tx.set_setting(keys::SINCE, &page.last_seq)
        })?;
        // Older servers don't say how many are pending; then a full page means "maybe more".
        let more = match page.pending {
            Some(pending) => pending > 0,
            None => page.changes.len() >= PAGE_SIZE,
        };
        if page.changes.is_empty() || !more {
            return Ok(());
        }
    }
}

/// Applies one `_changes` entry to the local database.
fn apply_change(
    tx: &SyncTx<'_>,
    change: &Change,
    now: DateTime<Utc>,
    report: &mut SyncReport,
) -> db::Result<()> {
    // `let ... else` = "if this pattern doesn't match, run the else block (which must leave)".
    let Some(doc_ref) = DocRef::parse(&change.id) else {
        return Ok(()); // Not one of our documents.
    };
    // `is_some_and` = "is Some, and the value inside passes this test".
    if tx.row(&change.id)?.is_some_and(|row| row.rev == change.rev) {
        return Ok(()); // Already have exactly this version (usually our own push).
    }
    if change.deleted {
        return apply_tombstone(tx, &doc_ref, &change.id, now, report);
    }
    let Some(doc) = &change.doc else {
        report.skipped_invalid += 1;
        return Ok(());
    };
    match doc_ref {
        DocRef::Item(_) => match item_from_doc(doc) {
            Ok(remote) => apply_remote_item(tx, &change.id, &change.rev, remote, report),
            Err(problem) => {
                count_problem(problem, report);
                Ok(())
            }
        },
        DocRef::Completion(_) => match completion_from_doc(doc) {
            Ok(remote) => {
                // Completions never change, so "we have it" means "we have this exact one".
                if !tx.has_completion(&remote.id)? {
                    tx.insert_completion(&remote)?;
                    report.pulled += 1;
                }
                tx.set_completion_synced(&remote.id, &change.rev)
            }
            Err(problem) => {
                count_problem(problem, report);
                Ok(())
            }
        },
    }
}

/// Merges a server version of an item into the local database and records its rev.
fn apply_remote_item(
    tx: &SyncTx<'_>,
    doc_id: &str,
    rev: &str,
    remote: Item,
    report: &mut SyncReport,
) -> db::Result<()> {
    match tx.get_item(&remote.id)? {
        None => {
            tx.write_item(&remote)?;
            report.pulled += 1;
        }
        Some(local) => match merge_item(&local, &remote) {
            // KeepLocal: nothing to write here. Because we record the *remote* version's
            // updated_at below, the local item now counts as "needs pushing".
            ItemMerge::Same | ItemMerge::KeepLocal => {}
            ItemMerge::TakeRemote => {
                tx.write_item(&remote)?;
                report.pulled += 1;
            }
            // Merged: a new version neither side had. Same trick: it differs from what the
            // server has, so it gets pushed.
            ItemMerge::Merged(merged) => {
                tx.write_item(&merged)?;
                report.pulled += 1;
            }
        },
    }
    tx.set_row(doc_id, rev, Some(remote.updated_at))
}

/// Handles a document deleted with CouchDB's own delete instead of our `deleted` flag.
///
/// Items: soft-delete the local copy (if it isn't already). Either way we forget the rev, so
/// our copy (with `deleted: true` and all its fields) is pushed back as the document's next
/// version. Completions are append-only, so ours is simply pushed back too.
fn apply_tombstone(
    tx: &SyncTx<'_>,
    doc_ref: &DocRef,
    doc_id: &str,
    now: DateTime<Utc>,
    report: &mut SyncReport,
) -> db::Result<()> {
    tx.forget_row(doc_id)?;
    // `if let ... && ...` chains: all three must hold (an item, we have it, not yet deleted).
    if let DocRef::Item(id) = doc_ref
        && let Some(mut local) = tx.get_item(id)?
        && !local.deleted
    {
        local.deleted = true;
        local.updated_at = next_updated_at(local.updated_at, now);
        tx.write_item(&local)?;
        report.pulled += 1;
    }
    Ok(())
}

/// Counts a document we couldn't use.
fn count_problem(problem: DocProblem, report: &mut SyncReport) {
    match problem {
        DocProblem::NewerVersion(_) => report.skipped_newer += 1,
        DocProblem::Invalid(_) => report.skipped_invalid += 1,
    }
}

/// Pushes everything the server doesn't have yet. Returns how many writes hit a conflict.
fn push(
    db: &mut Db,
    client: &CouchClient<'_>,
    report: &mut SyncReport,
) -> Result<usize, SyncError> {
    let mut conflicts = 0;
    // (how many were refused, the first reason) — reported after everything else is saved.
    let mut rejected: (usize, String) = (0, String::new());

    let items = db.items_to_push()?;
    // `.chunks(n)` walks a list n elements at a time (the last chunk may be shorter).
    for batch in items.chunks(PAGE_SIZE) {
        let docs: Vec<Value> = batch
            .iter()
            .map(|(item, rev)| item_to_doc(item, rev.as_deref()))
            .collect();
        let results = client.bulk_docs(&docs)?;
        db.sync_tx(|tx| {
            for result in results {
                match result {
                    BulkResult::Ok { id, rev } => {
                        if let Some((item, _)) =
                            batch.iter().find(|(i, _)| item_doc_id(&i.id) == id)
                        {
                            // Records the version we *sent*. If the item was edited again
                            // meanwhile, its updated_at differs, so it'll be pushed next time.
                            tx.set_item_pushed(item, &rev)?;
                            report.pushed += 1;
                        }
                    }
                    BulkResult::Conflict { .. } => conflicts += 1,
                    BulkResult::Rejected { reason, .. } => note_rejected(&mut rejected, reason),
                }
            }
            Ok(())
        })?;
    }

    let completions = db.completions_to_push()?;
    for batch in completions.chunks(PAGE_SIZE) {
        let docs: Vec<Value> = batch.iter().map(|c| completion_to_doc(c, None)).collect();
        let results = client.bulk_docs(&docs)?;
        db.sync_tx(|tx| {
            for result in results {
                match result {
                    BulkResult::Ok { id, rev } => {
                        if let Some(c) = batch.iter().find(|c| completion_doc_id(&c.id) == id) {
                            tx.set_completion_synced(&c.id, &rev)?;
                            report.pushed += 1;
                        }
                    }
                    // The server already has this completion (the other device completed the
                    // same task, which gives the same derived id). The next pull records it.
                    BulkResult::Conflict { .. } => conflicts += 1,
                    BulkResult::Rejected { reason, .. } => note_rejected(&mut rejected, reason),
                }
            }
            Ok(())
        })?;
    }

    if rejected.0 > 0 {
        return Err(SyncError::Rejected {
            count: rejected.0,
            reason: rejected.1,
        });
    }
    Ok(conflicts)
}

/// Counts a refused document, keeping the first reason for the error message.
fn note_rejected(rejected: &mut (usize, String), reason: String) {
    if rejected.0 == 0 {
        rejected.1 = reason;
    }
    rejected.0 += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DerivedId, Kind, RepeatRule, derived_id};
    use crate::sync::fake::FakeCouch;
    use crate::sync::transport::Method;
    use chrono::{NaiveDate, TimeZone};
    use serde_json::json;

    fn settings() -> SyncSettings {
        SyncSettings {
            enabled: true,
            url: "https://couchdb.example.ts.net".into(),
            database: "zerohour".into(),
            username: "tester".into(),
        }
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, hour, 0, 0).unwrap()
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    fn device() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn sync(db: &mut Db, server: &FakeCouch, now: DateTime<Utc>) -> SyncReport {
        sync_with(db, server, &settings(), now).unwrap()
    }

    fn task(title: &str, now: DateTime<Utc>) -> Item {
        Item::new(Kind::Task, title, now)
    }

    #[test]
    fn first_sync_pushes_existing_data_and_records_revs_and_checkpoint() {
        let server = FakeCouch::new("zerohour");
        let mut db = device();
        let a = task("a", at(9));
        let b = task("b", at(9));
        db.insert_item(&a).unwrap();
        db.insert_item(&b).unwrap();
        db.complete_task(&a.id, day(8).and_hms_opt(12, 0, 0).unwrap(), at(10))
            .unwrap();

        let report = sync(&mut db, &server, at(11));
        assert_eq!(report.pushed, 3, "two items and one completion");
        assert_eq!(report.conflicts_left, 0);
        assert_eq!(server.doc_count(), 3);
        assert_eq!(db.synced_doc_count().unwrap(), 3);
        assert!(db.get_setting(keys::SINCE).unwrap().is_some());
        assert_eq!(crate::sync::last_synced(&db).unwrap(), Some(at(11)));

        // The server holds exactly our data.
        let doc = server.doc(&item_doc_id(&a.id)).unwrap();
        assert_eq!(
            item_from_doc(&doc).unwrap(),
            db.get_item(&a.id).unwrap().unwrap()
        );
    }

    /// The `_rev` this device has recorded for a document.
    fn recorded_rev(db: &mut Db, doc_id: &str) -> Option<String> {
        db.sync_tx(|tx| tx.row(doc_id)).unwrap().map(|row| row.rev)
    }

    #[test]
    fn recorded_revs_track_the_server() {
        let server = FakeCouch::new("zerohour");
        let (mut laptop, mut phone) = (device(), device());
        let mut item = task("track me", at(9));
        laptop.insert_item(&item).unwrap();
        let doc_id = item_doc_id(&item.id);

        // After a push: the rev the server assigned.
        sync(&mut laptop, &server, at(9));
        let rev1 = server.doc(&doc_id).unwrap()["_rev"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(recorded_rev(&mut laptop, &doc_id), Some(rev1.clone()));

        // After a pull: the rev of the version we received.
        sync(&mut phone, &server, at(9));
        assert_eq!(recorded_rev(&mut phone, &doc_id), Some(rev1.clone()));

        // An update is sent with the current rev (else the fake would answer 409), and the
        // new rev is recorded on the device that pushed it and the device that pulled it.
        item.title = "tracked".into();
        phone.update_item(&mut item, at(10)).unwrap();
        let report = sync(&mut phone, &server, at(10));
        assert_eq!((report.pushed, report.conflicts_left), (1, 0));
        let rev2 = server.doc(&doc_id).unwrap()["_rev"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(rev2.starts_with("2-"));
        assert_eq!(recorded_rev(&mut phone, &doc_id), Some(rev2.clone()));
        sync(&mut laptop, &server, at(11));
        assert_eq!(recorded_rev(&mut laptop, &doc_id), Some(rev2));

        // The checkpoint only moves forward and matches the server's latest sequence.
        assert_eq!(
            laptop.get_setting(keys::SINCE).unwrap(),
            Some(server.current_seq())
        );
    }

    #[test]
    fn an_error_mid_sync_keeps_committed_pages_and_retries_the_rest() {
        let server = FakeCouch::new("zerohour");
        let mut laptop = device();
        laptop.insert_item(&task("from laptop", at(9))).unwrap();
        sync(&mut laptop, &server, at(9));
        let mut phone = device();
        let mine = task("from phone", at(9));
        phone.insert_item(&mine).unwrap();

        // The pull works, then every push fails with a server error.
        server.set_post_status(Some(500));
        let error = sync_with(&mut phone, &server, &settings(), at(10)).unwrap_err();
        assert!(matches!(error, SyncError::Http { status: 500, .. }));
        // The pulled page and its checkpoint were committed together...
        assert_eq!(phone.list_items().unwrap().len(), 2);
        assert_eq!(
            phone.get_setting(keys::SINCE).unwrap(),
            Some(server.current_seq())
        );
        // ...but the failed sync isn't recorded as a success, and our item is still pending.
        assert_eq!(crate::sync::last_synced(&phone).unwrap(), None);
        assert_eq!(recorded_rev(&mut phone, &item_doc_id(&mine.id)), None);

        // Next time it goes through.
        server.set_post_status(None);
        let report = sync(&mut phone, &server, at(11));
        assert_eq!(report.pushed, 1);
        assert_eq!(server.doc_count(), 2);
    }

    #[test]
    fn syncing_again_with_no_changes_does_nothing() {
        let server = FakeCouch::new("zerohour");
        let mut db = device();
        db.insert_item(&task("a", at(9))).unwrap();
        sync(&mut db, &server, at(10));
        let writes_before = server.request_count(Method::Post);

        let report = sync(&mut db, &server, at(11));
        assert_eq!((report.pulled, report.pushed), (0, 0));
        assert_eq!(
            server.request_count(Method::Post),
            writes_before,
            "no _bulk_docs"
        );
    }

    #[test]
    fn changes_flow_between_two_devices() {
        let server = FakeCouch::new("zerohour");
        let (mut laptop, mut phone) = (device(), device());
        let mut item = task("buy milk", at(9));
        laptop.insert_item(&item).unwrap();
        sync(&mut laptop, &server, at(9));

        let report = sync(&mut phone, &server, at(10));
        assert_eq!(report.pulled, 1);
        assert_eq!(phone.get_item(&item.id).unwrap().unwrap(), item);

        // Edit on the phone, see it on the laptop.
        item.title = "buy oat milk".into();
        phone.update_item(&mut item, at(11)).unwrap();
        sync(&mut phone, &server, at(11));
        sync(&mut laptop, &server, at(12));
        assert_eq!(
            laptop.get_item(&item.id).unwrap().unwrap().title,
            "buy oat milk"
        );

        // Delete on the laptop, gone on the phone (soft-deleted, so it can sync).
        laptop.delete_item(&item.id, at(13)).unwrap();
        sync(&mut laptop, &server, at(13));
        sync(&mut phone, &server, at(14));
        assert!(phone.list_items().unwrap().is_empty());
        assert!(phone.get_item(&item.id).unwrap().unwrap().deleted);
    }

    #[test]
    fn a_new_device_pulls_many_pages() {
        let server = FakeCouch::new("zerohour");
        let mut laptop = device();
        for i in 0..450 {
            laptop
                .insert_item(&task(&format!("task {i}"), at(9)))
                .unwrap();
        }
        let report = sync(&mut laptop, &server, at(10));
        assert_eq!(report.pushed, 450);
        assert!(
            server.request_count(Method::Post) >= 3,
            "pushed in batches of 200"
        );

        let mut phone = device();
        let report = sync(&mut phone, &server, at(11));
        assert_eq!(report.pulled, 450);
        assert_eq!(report.pushed, 0, "nothing echoed back");
        assert_eq!(phone.list_items().unwrap().len(), 450);
        assert_eq!(
            phone.get_setting(keys::SINCE).unwrap(),
            Some(server.current_seq())
        );
    }

    #[test]
    fn a_due_time_syncs_as_a_version_2_document() {
        let server = FakeCouch::new("zerohour");
        let (mut laptop, mut phone) = (device(), device());
        let mut item = task("standup", at(9));
        item.date = Some(day(9));
        item.due_time = chrono::NaiveTime::from_hms_opt(14, 0, 0);
        laptop.insert_item(&item).unwrap();
        let plain = task("no time", at(9));
        laptop.insert_item(&plain).unwrap();
        sync(&mut laptop, &server, at(9));

        assert_eq!(server.doc(&item_doc_id(&item.id)).unwrap()["v"], 2);
        assert_eq!(server.doc(&item_doc_id(&plain.id)).unwrap()["v"], 1);
        sync(&mut phone, &server, at(10));
        assert_eq!(
            phone.get_item(&item.id).unwrap().unwrap().due_time,
            item.due_time
        );
    }

    #[test]
    fn newest_edit_wins_on_both_devices() {
        let server = FakeCouch::new("zerohour");
        let (mut laptop, mut phone) = (device(), device());
        let item = task("original", at(9));
        laptop.insert_item(&item).unwrap();
        sync(&mut laptop, &server, at(9));
        sync(&mut phone, &server, at(9));

        // Both edit while offline; the phone's edit is later.
        let mut on_laptop = item.clone();
        on_laptop.title = "laptop edit".into();
        laptop.update_item(&mut on_laptop, at(10)).unwrap();
        let mut on_phone = item.clone();
        on_phone.title = "phone edit".into();
        phone.update_item(&mut on_phone, at(11)).unwrap();

        sync(&mut laptop, &server, at(12)); // laptop's edit reaches the server first...
        sync(&mut phone, &server, at(12)); // ...but the phone's newer edit wins
        sync(&mut laptop, &server, at(13));
        for db in [&laptop, &phone] {
            assert_eq!(db.get_item(&item.id).unwrap().unwrap().title, "phone edit");
        }
    }

    #[test]
    fn delete_sticks_against_a_later_edit_elsewhere() {
        let server = FakeCouch::new("zerohour");
        let (mut laptop, mut phone) = (device(), device());
        let item = task("old idea", at(9));
        laptop.insert_item(&item).unwrap();
        sync(&mut laptop, &server, at(9));
        sync(&mut phone, &server, at(9));

        laptop.delete_item(&item.id, at(10)).unwrap();
        let mut edited = item.clone();
        edited.title = "edited later".into();
        phone.update_item(&mut edited, at(11)).unwrap();

        sync(&mut laptop, &server, at(12));
        sync(&mut phone, &server, at(12));
        sync(&mut laptop, &server, at(13));
        for db in [&laptop, &phone] {
            let merged = db.get_item(&item.id).unwrap().unwrap();
            assert!(merged.deleted, "stays deleted");
            assert_eq!(merged.title, "edited later", "but keeps the newer fields");
        }
        let on_server = item_from_doc(&server.doc(&item_doc_id(&item.id)).unwrap()).unwrap();
        assert!(on_server.deleted);
    }

    #[test]
    fn completion_sticks_against_a_later_edit_elsewhere() {
        let server = FakeCouch::new("zerohour");
        let (mut laptop, mut phone) = (device(), device());
        let item = task("file taxes", at(9));
        laptop.insert_item(&item).unwrap();
        sync(&mut laptop, &server, at(9));
        sync(&mut phone, &server, at(9));

        laptop
            .complete_task(&item.id, day(8).and_hms_opt(12, 0, 0).unwrap(), at(10))
            .unwrap();
        let mut renamed = item.clone();
        renamed.title = "file taxes (state too)".into();
        phone.update_item(&mut renamed, at(11)).unwrap();

        sync(&mut laptop, &server, at(12));
        sync(&mut phone, &server, at(12));
        sync(&mut laptop, &server, at(13));
        for db in [&laptop, &phone] {
            let merged = db.get_item(&item.id).unwrap().unwrap();
            assert_eq!(merged.completed_at, Some(at(10)), "stays completed");
            assert_eq!(merged.title, "file taxes (state too)");
            assert_eq!(db.list_completions().unwrap().len(), 1);
        }
    }

    #[test]
    fn completing_the_same_repeating_task_on_two_offline_devices_merges() {
        let server = FakeCouch::new("zerohour");
        let (mut laptop, mut phone) = (device(), device());
        let mut item = task("water plants", at(9));
        item.date = Some(day(8));
        item.repeat = RepeatRule::Weekly;
        laptop.insert_item(&item).unwrap();
        sync(&mut laptop, &server, at(9));
        sync(&mut phone, &server, at(9));

        let next_laptop = laptop
            .complete_task(&item.id, day(8).and_hms_opt(12, 0, 0).unwrap(), at(10))
            .unwrap()
            .unwrap();
        let next_phone = phone
            .complete_task(&item.id, day(8).and_hms_opt(12, 0, 0).unwrap(), at(11))
            .unwrap()
            .unwrap();
        assert_eq!(next_laptop.id, next_phone.id, "same derived id");

        sync(&mut laptop, &server, at(12));
        let report = sync(&mut phone, &server, at(12));
        assert_eq!(report.conflicts_left, 0);
        sync(&mut laptop, &server, at(13));

        for db in [&laptop, &phone] {
            let items = db.list_items().unwrap();
            assert_eq!(items.len(), 2, "the original + ONE next instance");
            assert_eq!(items.iter().filter(|i| !i.is_completed()).count(), 1);
            let log = db.list_completions().unwrap();
            assert_eq!(log.len(), 1, "one completion, not two");
            assert_eq!(log[0].id, derived_id(DerivedId::Completion, &item.id));
        }
        assert_eq!(server.doc_count(), 3);
    }

    #[test]
    fn an_edit_after_seeing_a_future_timestamp_still_wins() {
        // The laptop's clock runs 2 hours fast. The phone sees its version, then edits it with
        // its own (correct, earlier-looking) clock. The phone's edit must still win.
        let server = FakeCouch::new("zerohour");
        let (mut laptop, mut phone) = (device(), device());
        let mut item = task("plan trip", at(12)); // laptop "now" = really 10:00
        laptop.insert_item(&item).unwrap();
        sync(&mut laptop, &server, at(12));
        sync(&mut phone, &server, at(10));

        item.title = "plan trip to Colorado".into();
        phone.update_item(&mut item, at(10)).unwrap(); // later in reality, earlier on the clock
        sync(&mut phone, &server, at(10));
        sync(&mut laptop, &server, at(12));
        assert_eq!(
            laptop.get_item(&item.id).unwrap().unwrap().title,
            "plan trip to Colorado"
        );
    }

    #[test]
    fn a_stale_rev_conflict_is_resolved_within_one_sync() {
        let server = FakeCouch::new("zerohour");
        let mut laptop = device();
        let mut item = task("draft", at(9));
        laptop.insert_item(&item).unwrap();
        sync(&mut laptop, &server, at(9));

        // Edit locally; then someone else writes an older version to the server right before
        // our push lands, so our _rev is stale and the first push gets a 409.
        item.title = "final".into();
        laptop.update_item(&mut item, at(11)).unwrap();
        server.write_before_next_bulk(&item_doc_id(&item.id), json!({ "title": "other" }), at(10));

        let report = sync(&mut laptop, &server, at(11));
        assert_eq!(report.conflicts_left, 0);
        let on_server = item_from_doc(&server.doc(&item_doc_id(&item.id)).unwrap()).unwrap();
        assert_eq!(
            on_server.title, "final",
            "newer local edit won after the retry"
        );
    }

    #[test]
    fn a_couchdb_delete_soft_deletes_locally_and_is_pushed_back() {
        let server = FakeCouch::new("zerohour");
        let mut db = device();
        let item = task("deleted in Fauxton", at(9));
        db.insert_item(&item).unwrap();
        sync(&mut db, &server, at(9));

        server.tombstone(&item_doc_id(&item.id));
        let report = sync(&mut db, &server, at(10));
        assert!(db.get_item(&item.id).unwrap().unwrap().deleted);
        assert_eq!(report.pushed, 1);
        let on_server = item_from_doc(&server.doc(&item_doc_id(&item.id)).unwrap()).unwrap();
        assert!(on_server.deleted, "kept as a soft delete, with all fields");
    }

    #[test]
    fn foreign_newer_and_broken_documents_are_skipped() {
        let server = FakeCouch::new("zerohour");
        server.put(json!({ "_id": "_design/app", "views": {} }));
        server.put(json!({ "_id": "notes:1", "text": "someone else's doc" }));
        let mut future = item_to_doc(&task("from the future", at(9)), None);
        future["v"] = json!(crate::sync::doc::DOC_VERSION + 1);
        server.put(future);
        server.put(json!({ "_id": "item:broken", "type": "item", "v": 1 }));

        let mut db = device();
        let report = sync(&mut db, &server, at(10));
        assert_eq!(report.skipped_newer, 1);
        assert_eq!(report.skipped_invalid, 1);
        assert_eq!(report.pulled, 0);
        assert!(db.list_items().unwrap().is_empty());
    }

    #[test]
    fn pointing_at_a_different_server_starts_over_safely() {
        let server = FakeCouch::new("zerohour");
        let mut db = device();
        db.insert_item(&task("a", at(9))).unwrap();
        sync(&mut db, &server, at(9));

        // Same server reached under a new name: revs are forgotten, then re-learned by
        // merging, without duplicating anything.
        let renamed = SyncSettings {
            url: "https://couch.example.ts.net".into(),
            ..settings()
        };
        let report = sync_with(&mut db, &server, &renamed, at(10)).unwrap();
        assert_eq!(report.conflicts_left, 0);
        assert_eq!(server.doc_count(), 1);
        assert_eq!(db.list_items().unwrap().len(), 1);
    }

    #[test]
    fn reset_sync_state_forgets_revs_and_checkpoint_only() {
        let server = FakeCouch::new("zerohour");
        let mut db = device();
        db.insert_item(&task("a", at(9))).unwrap();
        sync(&mut db, &server, at(9));

        db.clear_sync_state().unwrap();
        assert_eq!(db.synced_doc_count().unwrap(), 0);
        assert_eq!(db.get_setting(keys::SINCE).unwrap(), None);
        assert_eq!(db.list_items().unwrap().len(), 1, "data untouched");

        // The next sync rebuilds the state without duplicates.
        let report = sync(&mut db, &server, at(10));
        assert_eq!(report.conflicts_left, 0);
        assert_eq!(db.synced_doc_count().unwrap(), 1);
        assert_eq!(server.doc_count(), 1);
    }

    #[test]
    fn failures_leave_local_data_alone() {
        let mut db = device();
        let item = task("keep me", at(9));
        db.insert_item(&item).unwrap();

        let offline = FakeCouch::new("zerohour");
        offline.set_offline(true);
        let error = sync_with(&mut db, &offline, &settings(), at(10)).unwrap_err();
        assert!(error.is_offline());

        let locked = FakeCouch::new("zerohour");
        locked.set_status(Some(401));
        assert!(matches!(
            sync_with(&mut db, &locked, &settings(), at(10)),
            Err(SyncError::Unauthorized)
        ));

        // A database that doesn't exist is reported, never created.
        let other = FakeCouch::new("some-other-db");
        assert!(matches!(
            sync_with(&mut db, &other, &settings(), at(10)),
            Err(SyncError::DbNotFound)
        ));
        assert_eq!(other.request_count(Method::Post), 0);

        assert_eq!(db.list_items().unwrap(), vec![item]);
        assert_eq!(crate::sync::last_synced(&db).unwrap(), None);
        assert_eq!(db.synced_doc_count().unwrap(), 0);
    }

    #[test]
    fn clock_skew_is_reported() {
        let server = FakeCouch::new("zerohour");
        server.set_date(Some("Thu, 08 Oct 2026 09:55:00 GMT"));
        let report = sync(&mut device(), &server, at(10));
        assert_eq!(report.clock_skew_secs, Some(300));
    }
}
