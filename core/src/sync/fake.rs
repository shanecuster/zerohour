//! A fake CouchDB that lives in memory, for tests only (`#[cfg(test)]` in `mod.rs`).
//!
//! It implements [`Transport`], so the real sync code talks to it exactly as it would to a
//! server, with no network. It copies the CouchDB behavior sync relies on:
//! - `GET /{db}` → info, or 404 if the name doesn't match;
//! - `_changes` with `since`, `limit`, `include_docs`, `pending`, and opaque sequence strings;
//!   each document appears once, at its latest change;
//! - `_bulk_docs` with `_rev` checks: a stale or missing `_rev` on an existing document is a
//!   conflict; a deleted document (tombstone) may be recreated without a `_rev`.
//!
//! Plus switches to simulate trouble: offline, an HTTP error status, a `Date` header, and
//! "another device writes right before your next push".

use std::cell::RefCell;
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::transport::{HttpRequest, HttpResponse, Method, Transport, TransportError};
use crate::db::ts_to_db;

/// One stored document.
#[derive(Debug, Clone)]
struct FakeDoc {
    /// The generation number in front of the rev ("3" in "3-abc").
    generation: u64,
    rev: String,
    /// The fields, without `_id`/`_rev`.
    body: Map<String, Value>,
    deleted: bool,
    /// The sequence number of this document's latest change.
    seq: u64,
}

/// A write some "other device" makes just before the next `_bulk_docs` is processed.
#[derive(Debug, Clone)]
struct PendingWrite {
    doc_id: String,
    patch: Value,
    updated_at: DateTime<Utc>,
}

/// Everything the fake remembers. Inside a `RefCell` because `Transport::send` takes `&self`.
#[derive(Debug, Default)]
struct State {
    docs: BTreeMap<String, FakeDoc>,
    seq: u64,
    offline: bool,
    status: Option<u16>,
    post_status: Option<u16>,
    date: Option<String>,
    before_next_bulk: Option<PendingWrite>,
    requests: Vec<Method>,
}

/// The fake server, holding one database.
///
/// `RefCell` gives "interior mutability": changing data behind a shared `&` reference, with
/// the borrow rules checked at run time instead of compile time. Fine for single-threaded tests.
#[derive(Debug)]
pub struct FakeCouch {
    database: String,
    state: RefCell<State>,
}

impl FakeCouch {
    /// An empty server with one database of this name.
    pub fn new(database: &str) -> FakeCouch {
        FakeCouch {
            database: database.to_string(),
            state: RefCell::new(State::default()),
        }
    }

    /// Simulates no network.
    pub fn set_offline(&self, offline: bool) {
        self.state.borrow_mut().offline = offline;
    }

    /// Makes every request answer with this HTTP status.
    pub fn set_status(&self, status: Option<u16>) {
        self.state.borrow_mut().status = status;
    }

    /// Makes every POST (write) answer with this HTTP status; reads keep working.
    pub fn set_post_status(&self, status: Option<u16>) {
        self.state.borrow_mut().post_status = status;
    }

    /// Sets the `Date` header the server sends.
    pub fn set_date(&self, date: Option<&str>) {
        self.state.borrow_mut().date = date.map(str::to_string);
    }

    /// Writes a document as another client would (no `_rev` check).
    pub fn put(&self, doc: Value) {
        let mut state = self.state.borrow_mut();
        let id = doc["_id"].as_str().expect("doc needs _id").to_string();
        let body = strip_meta(&doc);
        let deleted = doc
            .get("_deleted")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        store(&mut state, &id, body, deleted);
    }

    /// Deletes a document the CouchDB way (a tombstone), like the Fauxton web UI does.
    pub fn tombstone(&self, doc_id: &str) {
        let mut state = self.state.borrow_mut();
        store(&mut state, doc_id, Map::new(), true);
    }

    /// Right before processing the next `_bulk_docs`, changes these fields of `doc_id` and
    /// stamps it `updated_at`, as if another device got its write in first.
    pub fn write_before_next_bulk(&self, doc_id: &str, patch: Value, updated_at: DateTime<Utc>) {
        self.state.borrow_mut().before_next_bulk = Some(PendingWrite {
            doc_id: doc_id.to_string(),
            patch,
            updated_at,
        });
    }

    /// The current version of a document (with `_id`/`_rev`), unless missing or deleted.
    pub fn doc(&self, doc_id: &str) -> Option<Value> {
        let state = self.state.borrow();
        state
            .docs
            .get(doc_id)
            .filter(|d| !d.deleted)
            .map(|d| full_doc(doc_id, d))
    }

    /// Number of non-deleted documents.
    pub fn doc_count(&self) -> usize {
        self.state
            .borrow()
            .docs
            .values()
            .filter(|d| !d.deleted)
            .count()
    }

    /// The current sequence, as `_changes` would report it.
    pub fn current_seq(&self) -> String {
        seq_string(self.state.borrow().seq)
    }

    /// How many requests of this method were made.
    pub fn request_count(&self, method: Method) -> usize {
        self.state
            .borrow()
            .requests
            .iter()
            .filter(|m| **m == method)
            .count()
    }

    /// `GET /{db}/_changes?...`
    fn changes(&self, query: &str) -> HttpResponse {
        let state = self.state.borrow();
        let param = |name: &str| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
        };
        let since = param("since").map(parse_seq).unwrap_or(0);
        let limit: usize = param("limit")
            .and_then(|l| l.parse().ok())
            .unwrap_or(usize::MAX);

        let mut newer: Vec<(&String, &FakeDoc)> =
            state.docs.iter().filter(|(_, d)| d.seq > since).collect();
        newer.sort_by_key(|(_, d)| d.seq);
        let page: Vec<_> = newer.iter().take(limit).collect();
        let results: Vec<Value> = page
            .iter()
            .map(|(id, d)| {
                let mut entry = json!({
                    "seq": seq_string(d.seq),
                    "id": id,
                    "changes": [{ "rev": d.rev }],
                    "doc": full_doc(id, d),
                });
                if d.deleted {
                    entry["deleted"] = json!(true);
                }
                entry
            })
            .collect();
        let last_seq = page.last().map(|(_, d)| d.seq).unwrap_or(state.seq);
        let body = json!({
            "results": results,
            "last_seq": seq_string(last_seq),
            "pending": newer.len() - page.len(),
        });
        respond(200, &body, &state)
    }

    /// `POST /{db}/_bulk_docs`
    fn bulk_docs(&self, body: &str) -> HttpResponse {
        let mut state = self.state.borrow_mut();
        if let Some(write) = state.before_next_bulk.take() {
            let mut fields = state
                .docs
                .get(&write.doc_id)
                .map(|d| d.body.clone())
                .unwrap_or_default();
            if let Some(patch) = write.patch.as_object() {
                fields.extend(patch.clone());
            }
            fields.insert("updated_at".into(), json!(ts_to_db(write.updated_at)));
            store(&mut state, &write.doc_id, fields, false);
        }

        let request: Value = serde_json::from_str(body).expect("bulk body is JSON");
        let docs = request["docs"].as_array().cloned().unwrap_or_default();
        let mut results = Vec::new();
        for doc in docs {
            let id = doc["_id"].as_str().unwrap_or_default().to_string();
            let sent_rev = doc.get("_rev").and_then(Value::as_str);
            let accepted = match state.docs.get(&id) {
                // A live document: the sent _rev must be the current one.
                Some(existing) if !existing.deleted => sent_rev == Some(existing.rev.as_str()),
                // A tombstone may be recreated without a rev (or with the tombstone's rev).
                Some(existing) => sent_rev.is_none() || sent_rev == Some(existing.rev.as_str()),
                // A brand-new document must not claim a rev.
                None => sent_rev.is_none(),
            };
            if accepted {
                let deleted = doc
                    .get("_deleted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let rev = store(&mut state, &id, strip_meta(&doc), deleted);
                results.push(json!({ "ok": true, "id": id, "rev": rev }));
            } else {
                results.push(json!({
                    "id": id, "error": "conflict", "reason": "Document update conflict."
                }));
            }
        }
        respond(201, &Value::Array(results), &state)
    }
}

impl Transport for FakeCouch {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.state.borrow_mut().requests.push(request.method);
        {
            let state = self.state.borrow();
            if state.offline {
                return Err(TransportError::Offline("connection refused".into()));
            }
            let failing = match request.method {
                Method::Post => state.status.or(state.post_status),
                Method::Get => state.status,
            };
            if let Some(status) = failing {
                let body = json!({ "error": "error", "reason": "simulated" });
                return Ok(respond(status, &body, &state));
            }
        }
        // Everything must be under "/<our database>"; anything else is "no such database".
        let prefix = format!("/{}", self.database);
        let Some(rest) = request.path.strip_prefix(&prefix) else {
            let state = self.state.borrow();
            let body = json!({ "error": "not_found", "reason": "Database does not exist." });
            return Ok(respond(404, &body, &state));
        };
        let (route, query) = rest.split_once('?').unwrap_or((rest, ""));
        Ok(match (request.method, route) {
            (Method::Get, "") => {
                let state = self.state.borrow();
                let count = state.docs.values().filter(|d| !d.deleted).count();
                respond(
                    200,
                    &json!({ "db_name": self.database, "doc_count": count }),
                    &state,
                )
            }
            (Method::Get, "/_changes") => self.changes(query),
            (Method::Post, "/_bulk_docs") => self.bulk_docs(request.body.as_deref().unwrap_or("")),
            _ => {
                let state = self.state.borrow();
                respond(
                    404,
                    &json!({ "error": "not_found", "reason": "missing" }),
                    &state,
                )
            }
        })
    }
}

/// Saves a new version of a document: next generation, new rev, new sequence number.
fn store(state: &mut State, id: &str, body: Map<String, Value>, deleted: bool) -> String {
    state.seq += 1;
    let generation = state.docs.get(id).map(|d| d.generation).unwrap_or(0) + 1;
    // Real revs end in an MD5 of the content; any unique text works for the fake.
    let rev = format!("{generation}-fake{:08x}", state.seq);
    state.docs.insert(
        id.to_string(),
        FakeDoc {
            generation,
            rev: rev.clone(),
            body,
            deleted,
            seq: state.seq,
        },
    );
    rev
}

/// A document's fields without the CouchDB metadata (`_id`, `_rev`, `_deleted`).
fn strip_meta(doc: &Value) -> Map<String, Value> {
    let mut body = doc.as_object().cloned().unwrap_or_default();
    for key in ["_id", "_rev", "_deleted"] {
        body.remove(key);
    }
    body
}

/// A stored document as CouchDB would return it.
fn full_doc(id: &str, doc: &FakeDoc) -> Value {
    let mut full = Map::new();
    full.insert("_id".into(), json!(id));
    full.insert("_rev".into(), json!(doc.rev));
    if doc.deleted {
        full.insert("_deleted".into(), json!(true));
    } else {
        full.extend(doc.body.clone());
    }
    Value::Object(full)
}

/// Sequence numbers are opaque strings in CouchDB 2+; we mimic that shape.
fn seq_string(seq: u64) -> String {
    format!("{seq}-fakeseq")
}

/// Reads the number back out of [`seq_string`].
fn parse_seq(text: &str) -> u64 {
    text.split('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Builds a response with the configured `Date` header.
fn respond(status: u16, body: &Value, state: &State) -> HttpResponse {
    HttpResponse {
        status,
        body: body.to_string(),
        date: state.date.clone(),
    }
}
