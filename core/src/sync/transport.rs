//! The HTTP layer: one tiny trait ([`Transport`]) plus the real implementation using `ureq`.
//!
//! Everything else in `sync/` talks to CouchDB only through [`Transport::send`]. That is what
//! lets the tests swap in a fake in-memory CouchDB (`fake.rs`) and run without any network or
//! server: the sync logic can't tell the difference.
//!
//! The real transport, [`UreqTransport`], adds the server URL in front of every path, adds the
//! login (HTTP Basic auth), and sorts failures into "offline", "TLS problem", or "other".

use std::fmt;
use std::time::Duration;

use super::SyncSettings;

/// The two HTTP methods sync needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One request to CouchDB, relative to the configured server URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    /// Path plus query string, starting with `/`, e.g. `/mydb/_changes?since=0`.
    pub path: String,
    /// JSON body for POST requests.
    pub body: Option<String>,
}

/// CouchDB's answer: status code, body text, and the server's `Date` header (used to warn
/// about clock skew between this device and the server).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    pub date: Option<String>,
}

/// Why a request got no HTTP answer at all. (An answer with an error status, like 401, is
/// still an [`HttpResponse`]; the caller decides what that status means.)
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Couldn't reach the server: no network, DNS failure, refused, timed out.
    #[error("{0}")]
    Offline(String),
    /// Reached something, but the secure connection failed (bad or untrusted certificate).
    #[error("{0}")]
    Tls(String),
    /// Anything else (malformed URL, protocol error, response too large...).
    #[error("{0}")]
    Other(String),
}

/// Something that can send an [`HttpRequest`] and return the answer.
///
/// A *trait* is Rust's word for an interface: a list of methods a type promises to have.
/// Sync code takes `&dyn Transport` ("any type that implements Transport"), so the real HTTP
/// client and the test fake are interchangeable.
pub trait Transport {
    /// Sends one request and waits for the full answer.
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// How long to wait for the server to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The most any single request may take in total, so a dead server can't hang a sync forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The real transport: HTTP(S) via `ureq`, with Basic auth.
///
/// Holds the `Authorization` header value, which contains the password (only base64-encoded,
/// not encrypted), so `Debug` is written by hand below to never print it.
pub struct UreqTransport {
    agent: ureq::Agent,
    /// Server URL without a trailing slash, e.g. `https://couchdb.example.ts.net`.
    base_url: String,
    /// `Basic <base64(user:password)>`.
    auth_header: String,
}

impl UreqTransport {
    /// Builds a transport for these settings and password. Doesn't connect yet.
    pub fn new(settings: &SyncSettings, password: &str) -> UreqTransport {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            // Hand 4xx/5xx answers back to us as normal responses; we turn them into clear
            // messages ourselves ("wrong username or password", "database not found").
            .http_status_as_error(false)
            .build();
        let credentials = format!("{}:{password}", settings.username.trim());
        UreqTransport {
            agent: ureq::Agent::new_with_config(config),
            base_url: settings.normalized_url(),
            auth_header: format!("Basic {}", base64_encode(credentials.as_bytes())),
        }
    }
}

impl fmt::Debug for UreqTransport {
    /// Prints the URL but never the credentials.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UreqTransport")
            .field("base_url", &self.base_url)
            .field("auth_header", &"<hidden>")
            .finish()
    }
}

impl Transport for UreqTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let url = format!("{}{}", self.base_url, request.path);
        // Both arms produce the same type (`Result<http::Response<Body>, ureq::Error>`), but
        // GET and POST are built with different ureq types, hence the two branches.
        let result = match request.method {
            Method::Get => self
                .agent
                .get(&url)
                .header("Authorization", &self.auth_header)
                .header("Accept", "application/json")
                .call(),
            Method::Post => self
                .agent
                .post(&url)
                .header("Authorization", &self.auth_header)
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .send(request.body.as_deref().unwrap_or("")),
        };
        let mut response = result.map_err(classify_error)?;
        let status = response.status().as_u16();
        let date = response
            .headers()
            .get("date")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(classify_error)?;
        Ok(HttpResponse { status, body, date })
    }
}

/// Sorts a ureq error into offline / TLS / other, with a short human message.
fn classify_error(error: ureq::Error) -> TransportError {
    match error {
        ureq::Error::HostNotFound => TransportError::Offline("server name not found".into()),
        ureq::Error::ConnectionFailed => TransportError::Offline("connection failed".into()),
        ureq::Error::Timeout(_) => TransportError::Offline("timed out".into()),
        // Io covers "connection refused", "network unreachable", a dropped connection...
        ureq::Error::Io(e) => TransportError::Offline(e.to_string()),
        ureq::Error::Tls(message) => TransportError::Tls(message.to_string()),
        ureq::Error::Rustls(e) => TransportError::Tls(e.to_string()),
        ureq::Error::Pem(e) => TransportError::Tls(e.to_string()),
        other => TransportError::Other(other.to_string()),
    }
}

/// Standard base64 (with `=` padding), as HTTP Basic auth requires.
///
/// Small enough to write here instead of adding a dependency. Base64 turns every 3 bytes into
/// 4 characters from a 64-letter alphabet; a final group of 1 or 2 bytes is padded with `=`.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        // Pack up to 3 bytes into one 24-bit number, then read it back 6 bits at a time.
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            // A 1-byte chunk yields 2 characters, a 2-byte chunk 3; the rest is padding.
            if i <= chunk.len() {
                let index = (n >> (18 - 6 * i)) & 0x3f;
                out.push(ALPHABET[index as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        // Test vectors from RFC 4648, section 10.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"user:pa ss"), "dXNlcjpwYSBzcw==");
    }

    #[test]
    fn debug_output_hides_the_credentials() {
        let settings = SyncSettings {
            enabled: true,
            url: "https://couchdb.example.ts.net/".into(),
            database: "zerohour".into(),
            username: "someone".into(),
        };
        let transport = UreqTransport::new(&settings, "placeholder-password");
        let printed = format!("{transport:?}");
        assert!(printed.contains("https://couchdb.example.ts.net"));
        assert!(!printed.contains("placeholder-password"));
        assert!(!printed.contains(&base64_encode(b"someone:placeholder-password")));
    }
}
