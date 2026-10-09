//! A minimal CouchDB client: the three calls sync needs, on top of any [`Transport`].
//!
//! - `GET /{db}`: does the database exist, and may we read it? (Also "test connection".)
//! - `GET /{db}/_changes?since=…&include_docs=true`: everything that changed since our last
//!   checkpoint, one page at a time.
//! - `POST /{db}/_bulk_docs`: write many documents in one request.
//!
//! This client never creates or deletes databases; there's simply no code for it.
//!
//! HTTP status codes become clear [`SyncError`]s here (401 → wrong login, 404 → no such
//! database...), so the rest of the sync code only deals with meaningful errors.

use serde_json::Value;

use super::SyncError;
use super::transport::{HttpRequest, HttpResponse, Method, Transport, TransportError};

/// How many changes to ask for per `_changes` page, and how many documents per `_bulk_docs`.
pub const PAGE_SIZE: usize = 200;

/// Basic facts about the database, from `GET /{db}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbInfo {
    /// Number of (non-deleted) documents on the server.
    pub doc_count: u64,
    /// The server's `Date` header, for the clock-skew check.
    pub server_date: Option<String>,
}

/// One entry of a `_changes` page.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The document `_id`.
    pub id: String,
    /// The document's current `_rev` (the winning one).
    pub rev: String,
    /// True if the document was deleted with CouchDB's own delete (a "tombstone").
    pub deleted: bool,
    /// The document itself (we ask for `include_docs=true`).
    pub doc: Option<Value>,
}

/// One page of `_changes`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangesPage {
    pub changes: Vec<Change>,
    /// The checkpoint to resume from next time. Opaque text: we store it and send it back.
    pub last_seq: String,
    /// How many more changes are waiting after this page, if the server says.
    pub pending: Option<u64>,
}

/// The server's answer for one document in a `_bulk_docs` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BulkResult {
    /// Saved; here is its new `_rev`.
    Ok { id: String, rev: String },
    /// Someone else changed the document first (our `_rev` was out of date).
    Conflict { id: String },
    /// Rejected for another reason (e.g. a validation function on the server).
    Rejected { id: String, reason: String },
}

/// Talks to one CouchDB database through a [`Transport`].
///
/// `'a` is a lifetime: the client borrows the transport and the database name, and can't
/// outlive them.
pub struct CouchClient<'a> {
    transport: &'a dyn Transport,
    /// The database name, already percent-encoded for use in a URL path.
    db_path: String,
}

impl<'a> CouchClient<'a> {
    /// A client for the database named `database`.
    pub fn new(transport: &'a dyn Transport, database: &str) -> CouchClient<'a> {
        CouchClient {
            transport,
            db_path: format!("/{}", percent_encode(database)),
        }
    }

    /// `GET /{db}`: checks that the database exists and we may read it.
    pub fn db_info(&self) -> Result<DbInfo, SyncError> {
        let response = self.send(Method::Get, self.db_path.clone(), None)?;
        let body = expect_json(&response, &[200])?;
        Ok(DbInfo {
            doc_count: body.get("doc_count").and_then(Value::as_u64).unwrap_or(0),
            server_date: response.date,
        })
    }

    /// `GET /{db}/_changes`: one page of changes after `since` (`None` = from the beginning).
    pub fn changes(&self, since: Option<&str>) -> Result<ChangesPage, SyncError> {
        let mut path = format!(
            "{}/_changes?include_docs=true&limit={PAGE_SIZE}",
            self.db_path
        );
        if let Some(since) = since {
            path.push_str("&since=");
            path.push_str(&percent_encode(since));
        }
        let response = self.send(Method::Get, path, None)?;
        parse_changes(&expect_json(&response, &[200])?)
    }

    /// `POST /{db}/_bulk_docs`: writes these documents, returning one result per document.
    pub fn bulk_docs(&self, docs: &[Value]) -> Result<Vec<BulkResult>, SyncError> {
        let body = serde_json::json!({ "docs": docs }).to_string();
        let path = format!("{}/_bulk_docs", self.db_path);
        let response = self.send(Method::Post, path, Some(body))?;
        // 201 = all written; 202 = accepted but not yet on enough cluster nodes (still fine).
        parse_bulk_results(&expect_json(&response, &[201, 202])?)
    }

    /// Sends a request, turning "no answer at all" into the matching [`SyncError`].
    fn send(
        &self,
        method: Method,
        path: String,
        body: Option<String>,
    ) -> Result<HttpResponse, SyncError> {
        let request = HttpRequest { method, path, body };
        self.transport.send(&request).map_err(|e| match e {
            TransportError::Offline(m) => SyncError::Offline(m),
            TransportError::Tls(m) => SyncError::Tls(m),
            TransportError::Other(m) => SyncError::BadResponse(m),
        })
    }
}

/// Checks the status code and parses the body as JSON. Error statuses become clear errors.
fn expect_json(response: &HttpResponse, ok_statuses: &[u16]) -> Result<Value, SyncError> {
    if !ok_statuses.contains(&response.status) {
        // CouchDB error bodies look like {"error":"not_found","reason":"Database does not exist."}.
        let reason = serde_json::from_str::<Value>(&response.body)
            .ok()
            .and_then(|v| v.get("reason").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        return Err(match response.status {
            401 => SyncError::Unauthorized,
            403 => SyncError::Forbidden,
            404 => SyncError::DbNotFound,
            status => SyncError::Http { status, reason },
        });
    }
    serde_json::from_str(&response.body)
        .map_err(|e| SyncError::BadResponse(format!("server sent invalid JSON: {e}")))
}

/// Reads a `_changes` response body.
fn parse_changes(body: &Value) -> Result<ChangesPage, SyncError> {
    let bad = |what: &str| SyncError::BadResponse(format!("_changes response: {what}"));
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("missing 'results'"))?;
    let mut changes = Vec::with_capacity(results.len());
    for entry in results {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("entry without 'id'"))?;
        // `changes` is a list of revs; with the default style it holds just the winning one.
        let rev = entry
            .get("changes")
            .and_then(Value::as_array)
            .and_then(|revs| revs.first())
            .and_then(|r| r.get("rev"))
            .and_then(Value::as_str)
            .ok_or_else(|| bad("entry without a rev"))?;
        changes.push(Change {
            id: id.to_string(),
            rev: rev.to_string(),
            deleted: entry
                .get("deleted")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            doc: entry.get("doc").filter(|d| !d.is_null()).cloned(),
        });
    }
    let last_seq = body
        .get("last_seq")
        .and_then(seq_text)
        .ok_or_else(|| bad("missing 'last_seq'"))?;
    Ok(ChangesPage {
        changes,
        last_seq,
        pending: body.get("pending").and_then(Value::as_u64),
    })
}

/// A sequence value as text. CouchDB 2+ uses opaque strings ("12-g1AAAA..."); 1.x used plain
/// numbers. Either way we only store it and send it back.
fn seq_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Reads a `_bulk_docs` response body: a list with one object per document sent.
fn parse_bulk_results(body: &Value) -> Result<Vec<BulkResult>, SyncError> {
    let bad = |what: &str| SyncError::BadResponse(format!("_bulk_docs response: {what}"));
    let entries = body.as_array().ok_or_else(|| bad("not a list"))?;
    entries
        .iter()
        .map(|entry| {
            let text = |field: &str| entry.get(field).and_then(Value::as_str);
            let id = text("id")
                .ok_or_else(|| bad("entry without 'id'"))?
                .to_string();
            Ok(match (text("rev"), text("error")) {
                (_, Some("conflict")) => BulkResult::Conflict { id },
                (_, Some(error)) => BulkResult::Rejected {
                    id,
                    reason: text("reason").unwrap_or(error).to_string(),
                },
                (Some(rev), None) => BulkResult::Ok {
                    id,
                    rev: rev.to_string(),
                },
                (None, None) => return Err(bad("entry with neither 'rev' nor 'error'")),
            })
        })
        .collect()
}

/// Percent-encodes text for use inside a URL path segment or query value: every byte except
/// letters, digits and `-._~` becomes `%XX`. (A `/` in a database name must become `%2F`,
/// which is how CouchDB expects it.)
pub(crate) fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded response shapes, as CouchDB 3.x sends them (ids and values are placeholders).
    const CHANGES_FIXTURE: &str = include_str!("fixtures/changes.json");
    const BULK_DOCS_FIXTURE: &str = include_str!("fixtures/bulk_docs.json");
    const NOT_FOUND_FIXTURE: &str = include_str!("fixtures/db_not_found.json");

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            body: body.to_string(),
            date: None,
        }
    }

    #[test]
    fn parses_a_recorded_changes_page() {
        let body = expect_json(&response(200, CHANGES_FIXTURE), &[200]).unwrap();
        let page = parse_changes(&body).unwrap();
        assert_eq!(page.changes.len(), 3);
        assert_eq!(page.changes[0].id, "_design/app");
        assert_eq!(
            page.changes[1].id,
            "item:0f8e2c1a-5b7d-4c3e-9a10-2b4c6d8e0f12"
        );
        assert_eq!(page.changes[1].rev, "2-7051cbe5c8faecd085a3fa619e6e6337");
        assert!(page.changes[1].doc.is_some());
        assert!(page.changes[2].deleted);
        assert_eq!(
            page.last_seq,
            "3-g1AAAABteJzLYWBgYMpgTmHgz8tPSTV0MDQy1zMwMDTXMwRKMeUAAAF0gYc"
        );
        assert_eq!(page.pending, Some(0));
    }

    #[test]
    fn parses_a_recorded_bulk_docs_answer() {
        let body = expect_json(&response(201, BULK_DOCS_FIXTURE), &[201]).unwrap();
        let results = parse_bulk_results(&body).unwrap();
        assert_eq!(
            results,
            vec![
                BulkResult::Ok {
                    id: "item:0f8e2c1a-5b7d-4c3e-9a10-2b4c6d8e0f12".into(),
                    rev: "3-c9dd1ad3a76b0c4da6bd8a91e4b8f0aa".into(),
                },
                BulkResult::Conflict {
                    id: "item:7a1c9e3b-2d4f-4a6b-8c0d-1e2f3a4b5c6d".into(),
                },
                BulkResult::Rejected {
                    id: "completion:5e6f7a8b-9c0d-4e1f-a2b3-c4d5e6f7a8b9".into(),
                    reason: "Only admins may write here.".into(),
                },
            ]
        );
    }

    #[test]
    fn error_statuses_become_clear_errors() {
        assert!(matches!(
            expect_json(&response(401, "{}"), &[200]),
            Err(SyncError::Unauthorized)
        ));
        assert!(matches!(
            expect_json(&response(403, "{}"), &[200]),
            Err(SyncError::Forbidden)
        ));
        assert!(matches!(
            expect_json(&response(404, NOT_FOUND_FIXTURE), &[200]),
            Err(SyncError::DbNotFound)
        ));
        match expect_json(
            &response(500, r#"{"error":"x","reason":"disk full"}"#),
            &[200],
        ) {
            Err(SyncError::Http { status, reason }) => {
                assert_eq!(status, 500);
                assert_eq!(reason, "disk full");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            expect_json(&response(200, "<html>proxy page</html>"), &[200]),
            Err(SyncError::BadResponse(_))
        ));
    }

    #[test]
    fn numeric_sequences_from_old_servers_work_too() {
        let body = serde_json::json!({ "results": [], "last_seq": 42 });
        assert_eq!(parse_changes(&body).unwrap().last_seq, "42");
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(percent_encode("zerohour"), "zerohour");
        assert_eq!(percent_encode("team/zero hour"), "team%2Fzero%20hour");
        assert_eq!(percent_encode("12-g1A=="), "12-g1A%3D%3D");
    }
}
