//! CouchDB documents: how an [`Item`] or [`Completion`] looks on the server, and back.
//!
//! One document per row, keyed by type and UUID:
//!
//! ```text
//! { "_id": "item:<uuid>", "_rev": "3-ab…", "type": "item", "v": 1,
//!   "kind": "task", "title": "…", "notes": null, "priority": "high",
//!   "date": "2026-12-25", "repeat": "every:2:weeks", "keep_after_pass": false,
//!   "completed_at": null, "created_at": "…Z", "updated_at": "…Z", "deleted": false }
//!
//! { "_id": "completion:<uuid>", "type": "completion", "v": 1, "item_id": "…",
//!   "completed_at": "…Z", "title_snapshot": "…", "priority_snapshot": "high",
//!   "was_overdue": false }
//! ```
//!
//! Every field uses exactly the same text format as the SQLite columns (see `db.rs`), so a
//! document turns into a row and back without losing anything. We write the JSON by hand
//! instead of deriving it from the structs, so that renaming a Rust field can never silently
//! change what's on the server.
//!
//! `v` is the *document format version*. A device that sees a newer `v` than it understands
//! skips that document (and says "update zerohour") instead of misreading it.
//!
//! Deletions use our own `"deleted": true`, not CouchDB's `_deleted`: a CouchDB deletion
//! ("tombstone") throws away every field, including the `updated_at` that conflict
//! resolution needs.

use serde_json::{Value, json};

use crate::db::{date_from_db, date_to_db, ts_from_db, ts_to_db};
use crate::model::{Completion, Item, Kind, Priority, RepeatRule};

/// The document format this build writes and understands.
pub const DOC_VERSION: u64 = 1;

/// `_id` prefix for item documents.
const ITEM_PREFIX: &str = "item:";
/// `_id` prefix for completion documents.
const COMPLETION_PREFIX: &str = "completion:";

/// The CouchDB `_id` of an item.
pub fn item_doc_id(item_id: &str) -> String {
    format!("{ITEM_PREFIX}{item_id}")
}

/// The CouchDB `_id` of a completion.
pub fn completion_doc_id(completion_id: &str) -> String {
    format!("{COMPLETION_PREFIX}{completion_id}")
}

/// What a CouchDB `_id` refers to locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocRef {
    /// An item, with its local id (the UUID).
    Item(String),
    /// A completion, with its local id.
    Completion(String),
}

impl DocRef {
    /// Reads an `_id`. Returns `None` for documents that aren't ours (e.g. `_design/...`).
    pub fn parse(doc_id: &str) -> Option<DocRef> {
        // `strip_prefix` returns the rest of the string if it started with the prefix.
        if let Some(id) = doc_id.strip_prefix(ITEM_PREFIX) {
            Some(DocRef::Item(id.to_string()))
        } else {
            doc_id
                .strip_prefix(COMPLETION_PREFIX)
                .map(|id| DocRef::Completion(id.to_string()))
        }
    }
}

/// An item as a CouchDB document. `rev` is the current `_rev` when updating an existing
/// document, or `None` when creating it.
pub fn item_to_doc(item: &Item, rev: Option<&str>) -> Value {
    let mut doc = json!({
        "_id": item_doc_id(&item.id),
        "type": "item",
        "v": DOC_VERSION,
        "kind": item.kind.as_str(),
        "title": item.title,
        "notes": item.notes,
        "priority": item.priority.as_str(),
        "date": item.date.map(date_to_db),
        "repeat": item.repeat.to_db_string(),
        "keep_after_pass": item.keep_after_pass,
        "completed_at": item.completed_at.map(ts_to_db),
        "created_at": ts_to_db(item.created_at),
        "updated_at": ts_to_db(item.updated_at),
        "deleted": item.deleted,
    });
    add_rev(&mut doc, rev);
    doc
}

/// A completion as a CouchDB document (see [`item_to_doc`] for `rev`).
pub fn completion_to_doc(completion: &Completion, rev: Option<&str>) -> Value {
    let mut doc = json!({
        "_id": completion_doc_id(&completion.id),
        "type": "completion",
        "v": DOC_VERSION,
        "item_id": completion.item_id,
        "completed_at": ts_to_db(completion.completed_at),
        "title_snapshot": completion.title_snapshot,
        "priority_snapshot": completion.priority_snapshot.as_str(),
        "was_overdue": completion.was_overdue,
    });
    add_rev(&mut doc, rev);
    doc
}

/// Adds `"_rev"` to a document if we know it.
fn add_rev(doc: &mut Value, rev: Option<&str>) {
    if let (Some(rev), Some(object)) = (rev, doc.as_object_mut()) {
        object.insert("_rev".into(), Value::String(rev.to_string()));
    }
}

/// Why a document from the server couldn't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocProblem {
    /// Written by a newer zerohour with a format this build doesn't know.
    NewerVersion(u64),
    /// Missing or malformed fields.
    Invalid(String),
}

/// Reads an item document. The `_id` must already be known to be an item.
pub fn item_from_doc(doc: &Value) -> Result<Item, DocProblem> {
    check_version(doc)?;
    let id = match DocRef::parse(&text(doc, "_id")?) {
        Some(DocRef::Item(id)) => id,
        _ => return Err(invalid("_id is not an item id")),
    };
    let kind = text(doc, "kind")?;
    let priority = text(doc, "priority")?;
    let repeat = text(doc, "repeat")?;
    Ok(Item {
        id,
        kind: Kind::parse(&kind).ok_or_else(|| invalid(format!("unknown kind '{kind}'")))?,
        title: text(doc, "title")?,
        notes: optional_text(doc, "notes")?,
        priority: Priority::parse(&priority)
            .ok_or_else(|| invalid(format!("unknown priority '{priority}'")))?,
        date: optional_text(doc, "date")?
            .map(|d| date_from_db(&d).map_err(|e| invalid(e.to_string())))
            .transpose()?,
        repeat: RepeatRule::parse(&repeat)
            .ok_or_else(|| invalid(format!("unknown repeat '{repeat}'")))?,
        keep_after_pass: flag(doc, "keep_after_pass")?,
        completed_at: optional_text(doc, "completed_at")?
            .map(|t| timestamp(&t))
            .transpose()?,
        created_at: timestamp(&text(doc, "created_at")?)?,
        updated_at: timestamp(&text(doc, "updated_at")?)?,
        deleted: flag(doc, "deleted")?,
    })
}

/// Reads a completion document. The `_id` must already be known to be a completion.
pub fn completion_from_doc(doc: &Value) -> Result<Completion, DocProblem> {
    check_version(doc)?;
    let id = match DocRef::parse(&text(doc, "_id")?) {
        Some(DocRef::Completion(id)) => id,
        _ => return Err(invalid("_id is not a completion id")),
    };
    let priority = text(doc, "priority_snapshot")?;
    Ok(Completion {
        id,
        item_id: text(doc, "item_id")?,
        completed_at: timestamp(&text(doc, "completed_at")?)?,
        title_snapshot: text(doc, "title_snapshot")?,
        priority_snapshot: Priority::parse(&priority)
            .ok_or_else(|| invalid(format!("unknown priority '{priority}'")))?,
        was_overdue: flag(doc, "was_overdue")?,
    })
}

/// Rejects documents written in a newer format than [`DOC_VERSION`].
fn check_version(doc: &Value) -> Result<(), DocProblem> {
    match doc.get("v").and_then(Value::as_u64) {
        Some(v) if v > DOC_VERSION => Err(DocProblem::NewerVersion(v)),
        Some(_) => Ok(()),
        None => Err(invalid("missing format version 'v'")),
    }
}

/// Shorthand for an [`DocProblem::Invalid`] with a message.
fn invalid(message: impl Into<String>) -> DocProblem {
    DocProblem::Invalid(message.into())
}

/// A required string field.
fn text(doc: &Value, field: &str) -> Result<String, DocProblem> {
    doc.get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| invalid(format!("missing text field '{field}'")))
}

/// An optional string field: missing or `null` both mean `None`.
fn optional_text(doc: &Value, field: &str) -> Result<Option<String>, DocProblem> {
    match doc.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(format!("field '{field}' is not text"))),
    }
}

/// A required true/false field.
fn flag(doc: &Value, field: &str) -> Result<bool, DocProblem> {
    doc.get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid(format!("missing true/false field '{field}'")))
}

/// An RFC 3339 timestamp field's text, parsed.
fn timestamp(text: &str) -> Result<chrono::DateTime<chrono::Utc>, DocProblem> {
    ts_from_db(text).map_err(|e| invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Unit;
    use chrono::{NaiveDate, TimeZone, Utc};

    fn sample_item() -> Item {
        let now = Utc.with_ymd_and_hms(2026, 10, 8, 14, 3, 0).unwrap();
        let mut item = Item::new(Kind::Task, "Renew registration", now);
        item.notes = Some("bring the form".into());
        item.priority = Priority::High;
        item.date = NaiveDate::from_ymd_opt(2026, 12, 25);
        item.repeat = RepeatRule::EveryN {
            n: 2,
            unit: Unit::Weeks,
        };
        item.completed_at = Some(now);
        item
    }

    #[test]
    fn item_round_trips_through_a_document() {
        let item = sample_item();
        let doc = item_to_doc(&item, Some("3-abc"));
        assert_eq!(doc["_id"], format!("item:{}", item.id));
        assert_eq!(doc["_rev"], "3-abc");
        assert_eq!(doc["repeat"], "every:2:weeks");
        assert_eq!(doc["updated_at"], "2026-10-08T14:03:00.000Z");
        assert_eq!(item_from_doc(&doc).unwrap(), item);
    }

    #[test]
    fn new_documents_have_no_rev() {
        let doc = item_to_doc(&sample_item(), None);
        assert!(doc.get("_rev").is_none());
    }

    #[test]
    fn completion_round_trips_through_a_document() {
        let completion = Completion {
            id: "c1".into(),
            item_id: "i1".into(),
            completed_at: Utc.with_ymd_and_hms(2026, 10, 8, 9, 0, 0).unwrap(),
            title_snapshot: "pay rent".into(),
            priority_snapshot: Priority::Low,
            was_overdue: true,
        };
        let doc = completion_to_doc(&completion, None);
        assert_eq!(doc["_id"], "completion:c1");
        assert_eq!(completion_from_doc(&doc).unwrap(), completion);
    }

    #[test]
    fn doc_ids_are_recognized() {
        assert_eq!(DocRef::parse("item:abc"), Some(DocRef::Item("abc".into())));
        assert_eq!(
            DocRef::parse("completion:abc"),
            Some(DocRef::Completion("abc".into()))
        );
        assert_eq!(DocRef::parse("_design/views"), None);
        assert_eq!(DocRef::parse("something-else"), None);
    }

    #[test]
    fn newer_format_and_bad_fields_are_reported() {
        let mut doc = item_to_doc(&sample_item(), None);
        doc["v"] = json!(2);
        assert_eq!(item_from_doc(&doc), Err(DocProblem::NewerVersion(2)));

        let mut doc = item_to_doc(&sample_item(), None);
        doc["priority"] = json!("urgent");
        assert!(matches!(item_from_doc(&doc), Err(DocProblem::Invalid(_))));

        let mut doc = item_to_doc(&sample_item(), None);
        doc.as_object_mut().unwrap().remove("title");
        assert!(matches!(item_from_doc(&doc), Err(DocProblem::Invalid(_))));
    }
}
