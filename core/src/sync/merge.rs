//! Conflict rules: what happens when this device and the server both have a version of the
//! same item. Pure functions with no database or network, so they are easy to test.
//!
//! The rules, in order:
//! 1. **Newest `updated_at` wins** the whole item (every field comes from the winner).
//! 2. **Ties** (equal timestamps) go to the version whose document JSON sorts last. That's
//!    arbitrary, but every device computes the same answer, so they all agree without talking.
//! 3. **Deletes are sticky**: if either version is deleted, the result is deleted. Otherwise an
//!    edit on a device that hadn't heard about the delete yet would bring the item back.
//! 4. **Completion is sticky**: if either version is completed, the result is completed, at the
//!    earliest completion time. Otherwise editing a task's title on one device could reopen a
//!    task that was completed (and logged, and repeated) on another.
//!
//! If rules 3 or 4 change the winner, the result is a *new* version that neither side has.
//! It gets `updated_at` = 1 ms after the newer of the two, so it beats both everywhere.
//!
//! Completions never need merging: they're only ever created, never edited, and their ids are
//! unique (random, or derived from a task that can only be completed once). If the same
//! completion id arrives twice, both copies describe the same event, so we keep the one we have.

use chrono::TimeDelta;

use super::doc::item_to_doc;
use crate::model::Item;

/// The result of comparing the local and remote version of one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemMerge {
    /// Both are identical. Nothing to do.
    Same,
    /// The remote version wins as-is: write it locally.
    TakeRemote,
    /// The local version wins as-is: keep it, and push it to the server.
    KeepLocal,
    /// Neither wins as-is (a sticky rule applied): write this locally and push it.
    Merged(Item),
}

/// Decides between the local and remote version of the same item (same id).
pub fn merge_item(local: &Item, remote: &Item) -> ItemMerge {
    if local == remote {
        return ItemMerge::Same;
    }
    let winner = if remote_wins(local, remote) {
        remote
    } else {
        local
    };

    // Start from the winner, then apply the sticky rules.
    let mut merged = winner.clone();
    merged.deleted = local.deleted || remote.deleted;
    merged.completed_at = match (local.completed_at, remote.completed_at) {
        (Some(a), Some(b)) => Some(a.min(b)),
        // `.or(...)` = "the first one if present, otherwise the second".
        (a, b) => a.or(b),
    };

    if merged == *remote {
        ItemMerge::TakeRemote
    } else if merged == *local {
        ItemMerge::KeepLocal
    } else {
        merged.updated_at = local.updated_at.max(remote.updated_at) + TimeDelta::milliseconds(1);
        ItemMerge::Merged(merged)
    }
}

/// Rule 1 and 2: true if the remote version is the newer one (or wins the tie).
fn remote_wins(local: &Item, remote: &Item) -> bool {
    if remote.updated_at != local.updated_at {
        return remote.updated_at > local.updated_at;
    }
    // Tie: compare the documents' JSON text. Both devices see the same two versions, so both
    // make the same choice. (Same text would mean same item, which `Same` already caught.)
    let local_text = item_to_doc(local, None).to_string();
    let remote_text = item_to_doc(remote, None).to_string();
    remote_text > local_text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;
    use chrono::{DateTime, TimeZone, Utc};

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, 12, minute, 0).unwrap()
    }

    /// One item, and a copy of it edited at a later time.
    fn base() -> Item {
        Item::new(Kind::Task, "original", at(0))
    }

    fn edited(item: &Item, title: &str, minute: u32) -> Item {
        let mut copy = item.clone();
        copy.title = title.into();
        copy.updated_at = at(minute);
        copy
    }

    #[test]
    fn identical_versions_need_nothing() {
        let item = base();
        assert_eq!(merge_item(&item, &item.clone()), ItemMerge::Same);
    }

    #[test]
    fn newest_updated_at_wins() {
        let item = base();
        let local = edited(&item, "local", 5);
        let remote = edited(&item, "remote", 9);
        assert_eq!(merge_item(&local, &remote), ItemMerge::TakeRemote);
        assert_eq!(merge_item(&remote, &local), ItemMerge::KeepLocal);
    }

    #[test]
    fn ties_are_decided_the_same_way_on_both_devices() {
        let item = base();
        let a = edited(&item, "alpha", 5);
        let b = edited(&item, "bravo", 5);
        // Device 1 has `a` locally and sees `b`; device 2 the other way around.
        let device1 = merge_item(&a, &b);
        let device2 = merge_item(&b, &a);
        let winner1 = if device1 == ItemMerge::TakeRemote {
            &b
        } else {
            &a
        };
        let winner2 = if device2 == ItemMerge::TakeRemote {
            &a
        } else {
            &b
        };
        assert_eq!(winner1, winner2);
    }

    #[test]
    fn delete_sticks_even_against_a_newer_edit() {
        let item = base();
        let mut deleted = item.clone();
        deleted.deleted = true;
        deleted.updated_at = at(5);
        let newer_edit = edited(&item, "renamed later", 9);

        // Local deleted, remote has the newer edit: result = the edit, but deleted.
        match merge_item(&deleted, &newer_edit) {
            ItemMerge::Merged(m) => {
                assert!(m.deleted);
                assert_eq!(m.title, "renamed later");
                assert_eq!(m.updated_at, at(9) + TimeDelta::milliseconds(1));
            }
            other => panic!("expected a merged version, got {other:?}"),
        }
        // And the same from the other side.
        assert!(matches!(
            merge_item(&newer_edit, &deleted),
            ItemMerge::Merged(m) if m.deleted
        ));
    }

    #[test]
    fn completion_sticks_at_the_earliest_time() {
        let item = base();
        let mut done_here = item.clone();
        done_here.completed_at = Some(at(3));
        done_here.updated_at = at(3);
        let mut done_there = item.clone();
        done_there.completed_at = Some(at(1));
        done_there.updated_at = at(1);
        let renamed_later = edited(&item, "renamed", 9);

        // Completed here, renamed (still open) there later: stays completed.
        match merge_item(&done_here, &renamed_later) {
            ItemMerge::Merged(m) => {
                assert_eq!(m.completed_at, Some(at(3)));
                assert_eq!(m.title, "renamed");
            }
            other => panic!("expected a merged version, got {other:?}"),
        }
        // Completed on both: the newer version wins but keeps the earliest completion time.
        match merge_item(&done_here, &done_there) {
            ItemMerge::Merged(m) => assert_eq!(m.completed_at, Some(at(1))),
            other => panic!("expected a merged version, got {other:?}"),
        }
    }

    #[test]
    fn a_newer_remote_that_already_has_the_sticky_flags_is_taken_as_is() {
        let item = base();
        let mut local = item.clone();
        local.deleted = true;
        local.updated_at = at(2);
        let mut remote = edited(&item, "renamed", 7);
        remote.deleted = true;
        assert_eq!(merge_item(&local, &remote), ItemMerge::TakeRemote);
    }
}
