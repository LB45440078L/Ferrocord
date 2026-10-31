//! API token storage and the request guard (Host/Origin checks + bearer auth).

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use ferrocord_core::Paths;
use parking_lot::RwLock;
use rand::RngCore;

use crate::error::ApiError;

/// File (in the config directory) holding the API token.
pub const TOKEN_FILE: &str = "api-token.txt";

/// Path of the token file for the given directories.
pub fn token_path(paths: &Paths) -> PathBuf {
    paths.config_dir.join(TOKEN_FILE)
}

/// Reads the API token, generating and saving one if missing or malformed.
pub fn load_or_create_token(paths: &Paths) -> io::Result<String> {
    if let Ok(s) = std::fs::read_to_string(token_path(paths)) {
        let t = s.trim();
        if t.len() >= 32 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(t.to_string());
        }
    }
    regenerate_token(paths)
}

/// Generates a new random token (32 bytes, hex-encoded), saves it and returns it.
/// Clients using the old token stop working.
pub fn regenerate_token(paths: &Paths) -> io::Result<String> {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::create_dir_all(&paths.config_dir)?;
    let path = token_path(paths);
    let tmp = path.with_extension("txt.tmp");
    std::fs::write(&tmp, &token)?;
    restrict_permissions(&tmp);
    std::fs::rename(&tmp, &path)?;
    Ok(token)
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

/// Compares two byte strings in time independent of where they differ.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// State needed by [`guard`].
#[derive(Clone)]
pub(crate) struct Guard {
    pub token: Arc<RwLock<String>>,
    pub port: u16,
}

impl Guard {
    fn host_ok(&self, host: &str) -> bool {
        let port = self.port.to_string();
        match host.rsplit_once(':') {
            Some((name, p)) => p == port && (name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost")),
            None => false,
        }
    }

    fn origin_ok(&self, origin: &str) -> bool {
        if origin == "null" {
            return true;
        }
        origin.strip_prefix("http://").is_some_and(|h| self.host_ok(h))
    }
}

/// Rejects DNS-rebinding (bad `Host`), browser-originated (`Origin`) and
/// unauthenticated requests before they reach a handler.
pub(crate) async fn guard(State(g): State<Guard>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if !g.host_ok(host) {
        return ApiError::new(axum::http::StatusCode::FORBIDDEN, "forbidden_host", "requests must be addressed to 127.0.0.1 or localhost").into_response();
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && !origin.to_str().is_ok_and(|o| g.origin_ok(o))
    {
        return ApiError::new(axum::http::StatusCode::FORBIDDEN, "forbidden_origin", "browser-originated requests are not allowed").into_response();
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer ")))
        .map(str::trim);
    let query = if req.uri().path() == "/v1/ws" { req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token="))) } else { None };
    let expected = g.token.read().clone();
    let authorized = bearer.or(query).is_some_and(|t| constant_time_eq(t.as_bytes(), expected.as_bytes()));
    if !authorized {
        return ApiError::unauthorized().into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_origin_rules() {
        let g = Guard { token: Arc::default(), port: 6473 };
        assert!(g.host_ok("127.0.0.1:6473"));
        assert!(g.host_ok("localhost:6473"));
        assert!(g.host_ok("LOCALHOST:6473"));
        assert!(!g.host_ok("127.0.0.1"));
        assert!(!g.host_ok("127.0.0.1:80"));
        assert!(!g.host_ok("evil.com:6473"));
        assert!(!g.host_ok("127.0.0.1.evil.com:6473"));
        assert!(g.origin_ok("null"));
        assert!(g.origin_ok("http://127.0.0.1:6473"));
        assert!(!g.origin_ok("https://evil.com"));
        assert!(!g.origin_ok("http://localhost:3000"));
    }

    #[test]
    fn constant_time() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn token_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let t1 = load_or_create_token(&paths).unwrap();
        assert_eq!(t1.len(), 64);
        assert_eq!(load_or_create_token(&paths).unwrap(), t1);
        let t2 = regenerate_token(&paths).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(std::fs::read_to_string(token_path(&paths)).unwrap(), t2);
    }
}
