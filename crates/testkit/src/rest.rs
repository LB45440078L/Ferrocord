//! REST side of the mock: request recording, scripted routes and the axum router.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use base64::Engine;
use serde_json::{Value, json};

use crate::Shared;

/// A REST request as received by the mock.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// Arrival time (monotonic), for rate-limit timing assertions.
    pub at: Instant,
    pub method: String,
    /// Path below `/api/v9`, exactly as sent (still percent-encoded), e.g. `/channels/1/messages`.
    pub path: String,
    /// Decoded query pairs in order.
    pub query: Vec<(String, String)>,
    /// Header names are lowercase.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.header(name).is_some()
    }

    /// First value of a query parameter.
    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Body parsed as JSON (`Null` when empty or not JSON).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// Path segments, percent-decoded.
    pub fn segments(&self) -> Vec<String> {
        self.path.split('/').filter(|s| !s.is_empty()).map(percent_decode).collect()
    }

    /// `X-Super-Properties` decoded from base64 JSON.
    pub fn super_properties(&self) -> Option<Value> {
        self.base64_json_header("x-super-properties")
    }

    /// A header holding base64-encoded JSON (`X-Super-Properties`, `X-Context-Properties`).
    pub fn base64_json_header(&self, name: &str) -> Option<Value> {
        let raw = base64::engine::general_purpose::STANDARD.decode(self.header(name)?).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// Parts of a `multipart/form-data` body.
    pub fn multipart(&self) -> Option<Vec<MultipartPart>> {
        let ct = self.header("content-type")?;
        let boundary = ct.split(';').map(str::trim).find_map(|p| p.strip_prefix("boundary="))?.trim_matches('"');
        Some(parse_multipart(&self.body, boundary))
    }
}

/// One part of a multipart body.
#[derive(Debug, Clone)]
pub struct MultipartPart {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

impl MultipartPart {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn parse_multipart(body: &[u8], boundary: &str) -> Vec<MultipartPart> {
    let delim = format!("--{boundary}");
    let mut parts = vec![];
    let Some(mut pos) = find(body, delim.as_bytes(), 0) else { return parts };
    loop {
        pos += delim.len();
        if body[pos..].starts_with(b"--") {
            break;
        }
        // Skip CRLF after the delimiter.
        pos += 2;
        let Some(header_end) = find(body, b"\r\n\r\n", pos) else { break };
        let headers = String::from_utf8_lossy(&body[pos..header_end]).into_owned();
        let data_start = header_end + 4;
        let Some(next) = find(body, format!("\r\n{delim}").as_bytes(), data_start) else { break };
        let mut part = MultipartPart { name: String::new(), filename: None, content_type: None, data: body[data_start..next].to_vec() };
        for line in headers.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("content-disposition:") {
                for attr in line.split(';').map(str::trim) {
                    if let Some(v) = attr.strip_prefix("name=") {
                        part.name = v.trim_matches('"').to_string();
                    } else if let Some(v) = attr.strip_prefix("filename=") {
                        part.filename = Some(v.trim_matches('"').to_string());
                    }
                }
            } else if lower.starts_with("content-type:") {
                part.content_type = Some(line["content-type:".len()..].trim().to_string());
            }
        }
        parts.push(part);
        pos = next + 2;
    }
    parts
}

pub(crate) fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(&k.replace('+', " ")), percent_decode(&v.replace('+', " ")))
        })
        .collect()
}

/// A canned REST response.
#[derive(Debug, Clone)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
    /// Wait this long before answering.
    pub delay: Option<Duration>,
}

impl MockResponse {
    pub fn json(status: u16, body: Value) -> Self {
        Self { status, headers: vec![], body: Some(body), delay: None }
    }

    pub fn ok(body: Value) -> Self {
        Self::json(200, body)
    }

    /// `204 No Content`.
    pub fn no_content() -> Self {
        Self { status: 204, headers: vec![], body: None, delay: None }
    }

    /// A Discord JSON error: `{"code":…,"message":…}`.
    pub fn error(status: u16, code: u32, message: &str) -> Self {
        Self::json(status, json!({ "code": code, "message": message }))
    }

    /// `401 {"message":"401: Unauthorized","code":0}`.
    pub fn unauthorized() -> Self {
        Self::error(401, 0, "401: Unauthorized")
    }

    /// `404 {"message":"404: Not Found","code":0}`.
    pub fn not_found() -> Self {
        Self::error(404, 0, "404: Not Found")
    }

    /// `400 Invalid Form Body` with one field error (`errors.<field>._errors[0]`).
    pub fn form_error(field: &str, code: &str, message: &str) -> Self {
        let mut errors = serde_json::Map::new();
        let mut node = json!({ "_errors": [{ "code": code, "message": message }] });
        for seg in field.rsplit('.') {
            node = json!({ seg: node });
        }
        if let Value::Object(m) = node {
            errors = m;
        }
        Self::json(400, json!({ "code": 50035, "message": "Invalid Form Body", "errors": errors }))
    }

    /// `429` with Discord's body and rate-limit headers.
    pub fn rate_limited(retry_after: f64, global: bool) -> Self {
        let mut r = Self::json(429, json!({ "message": "You are being rate limited.", "retry_after": retry_after, "global": global, "code": 0 }));
        r.headers.push(("retry-after".into(), format!("{}", retry_after.ceil() as u64)));
        if global {
            r.headers.push(("x-ratelimit-global".into(), "true".into()));
        }
        r.headers.push(("x-ratelimit-scope".into(), if global { "global".into() } else { "user".into() }));
        r
    }

    pub fn with_header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_ascii_lowercase(), v.into()));
        self
    }

    /// Adds `X-RateLimit-*` bucket headers.
    pub fn with_bucket(self, bucket: &str, limit: u32, remaining: u32, reset_after: f64) -> Self {
        self.with_header("x-ratelimit-bucket", bucket)
            .with_header("x-ratelimit-limit", limit.to_string())
            .with_header("x-ratelimit-remaining", remaining.to_string())
            .with_header("x-ratelimit-reset-after", format!("{reset_after:.3}"))
    }

    pub fn with_delay(mut self, d: Duration) -> Self {
        self.delay = Some(d);
        self
    }

    fn into_response(self) -> Response {
        let mut b = Response::builder().status(StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
        let has_ct = self.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
        if let Some(h) = b.headers_mut() {
            for (k, v) in &self.headers {
                if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
                    h.append(k, v);
                }
            }
            if self.body.is_some() && !has_ct {
                h.insert("content-type", HeaderValue::from_static("application/json"));
            }
        }
        let body = match self.body {
            Some(v) => Body::from(v.to_string()),
            None => Body::empty(),
        };
        b.body(body).unwrap_or_else(|_| Response::new(Body::empty()))
    }
}

// ---------------------------------------------------------------- path patterns

#[derive(Debug, Clone)]
enum Seg {
    Lit(String),
    Any,
    Rest,
}

/// Path pattern: `/channels/*/messages/{id}/**`. `*` and `{name}` match one
/// segment, `**` matches any remainder. Literals match either the raw or the
/// percent-decoded segment.
#[derive(Debug, Clone)]
pub(crate) struct Pattern(Vec<Seg>);

impl Pattern {
    pub fn parse(p: &str) -> Self {
        Self(
            p.split('/')
                .filter(|s| !s.is_empty())
                .map(|s| match s {
                    "*" => Seg::Any,
                    "**" => Seg::Rest,
                    s if s.starts_with('{') && s.ends_with('}') => Seg::Any,
                    s => Seg::Lit(s.to_string()),
                })
                .collect(),
        )
    }

    pub fn matches(&self, path: &str) -> bool {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut i = 0;
        for p in &self.0 {
            match p {
                Seg::Rest => return true,
                Seg::Any => {
                    if i >= segs.len() {
                        return false;
                    }
                }
                Seg::Lit(l) => {
                    let Some(s) = segs.get(i) else { return false };
                    if l != s && *l != percent_decode(s) {
                        return false;
                    }
                }
            }
            i += 1;
        }
        i == segs.len()
    }
}

// ---------------------------------------------------------------- scripted routes

pub(crate) type Handler = Arc<dyn Fn(&RecordedRequest) -> Option<MockResponse> + Send + Sync>;

pub(crate) struct Route {
    pub id: usize,
    pub method: String,
    pub pattern: Pattern,
    /// `None` = unlimited.
    pub remaining: Option<usize>,
    pub handler: Handler,
}

/// Builder returned by [`crate::MockDiscord::on`].
#[must_use = "a route does nothing until respond/handle is called"]
pub struct RouteBuilder<'a> {
    shared: &'a Shared,
    method: String,
    pattern: String,
    times: Option<usize>,
}

impl<'a> RouteBuilder<'a> {
    pub(crate) fn new(shared: &'a Shared, method: &str, pattern: &str) -> Self {
        Self { shared, method: method.to_ascii_uppercase(), pattern: pattern.to_string(), times: None }
    }

    /// Only answer the next `n` matching requests, then fall through.
    pub fn times(mut self, n: usize) -> Self {
        self.times = Some(n);
        self
    }

    /// Shorthand for `times(1)`.
    pub fn once(self) -> Self {
        self.times(1)
    }

    /// Always answer with `resp`.
    pub fn respond(self, resp: MockResponse) {
        self.handle(move |_| Some(resp.clone()));
    }

    /// Answer with `responses` in order, then fall through.
    pub fn respond_seq(self, responses: Vec<MockResponse>) {
        let n = responses.len();
        let queue = parking_lot::Mutex::new(std::collections::VecDeque::from(responses));
        self.times(n).handle(move |_| queue.lock().pop_front());
    }

    /// Custom handler. Returning `None` falls through to older routes and the world.
    pub fn handle(self, f: impl Fn(&RecordedRequest) -> Option<MockResponse> + Send + Sync + 'static) {
        let id = self.shared.next_route.fetch_add(1, Ordering::Relaxed);
        self.shared.routes.lock().push(Route { id, method: self.method, pattern: Pattern::parse(&self.pattern), remaining: self.times, handler: Arc::new(f) });
    }
}

/// Runs scripted routes (newest first) and returns the first answer.
fn scripted(shared: &Shared, req: &RecordedRequest) -> Option<MockResponse> {
    let mut routes = shared.routes.lock();
    let mut hit = None;
    for r in routes.iter_mut().rev() {
        if !(r.method == "*" || r.method == req.method) || !r.pattern.matches(&req.path) {
            continue;
        }
        if let Some(resp) = (r.handler)(req) {
            if let Some(n) = r.remaining.as_mut() {
                *n = n.saturating_sub(1);
            }
            hit = Some((r.id, resp));
            break;
        }
    }
    routes.retain(|r| r.remaining != Some(0));
    hit.map(|(_, r)| r)
}

// ---------------------------------------------------------------- router

pub(crate) fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/gateway", get(crate::gateway::ws_handler))
        .route("/gateway/", get(crate::gateway::ws_handler))
        .route("/resume", get(crate::gateway::ws_handler))
        .route("/resume/", get(crate::gateway::ws_handler))
        .route("/remote-auth", get(crate::remote_auth::ws_handler))
        .route("/remote-auth/", get(crate::remote_auth::ws_handler))
        .fallback(handle)
        .with_state(shared)
}

async fn handle(State(shared): State<Arc<Shared>>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, 64 * 1024 * 1024).await.map(|b| b.to_vec()).unwrap_or_default();
    let Some(path) = parts.uri.path().strip_prefix("/api/v9") else {
        return MockResponse::not_found().into_response();
    };
    let recorded = RecordedRequest {
        at: Instant::now(),
        method: parts.method.as_str().to_string(),
        path: if path.is_empty() { "/".into() } else { path.to_string() },
        query: parts.uri.query().map(parse_query).unwrap_or_default(),
        headers: parts.headers.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect(),
        body,
    };
    shared.requests.lock().push(recorded.clone());
    shared.bump();

    let delay = shared.delays.lock().iter().filter(|(m, p, _)| *m == recorded.method && p.matches(&recorded.path)).map(|(_, _, d)| *d).max();
    if let Some(d) = delay {
        tokio::time::sleep(d).await;
    }
    let resp = match scripted(&shared, &recorded) {
        Some(r) => r,
        None => crate::world::handle(&shared, &recorded),
    };
    if let Some(d) = resp.delay {
        tokio::time::sleep(d).await;
    }
    resp.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(Pattern::parse("/channels/*/messages").matches("/channels/1/messages"));
        assert!(!Pattern::parse("/channels/*/messages").matches("/channels/1/messages/2"));
        assert!(Pattern::parse("/channels/**").matches("/channels/1/messages/2"));
        assert!(Pattern::parse("/x/👍").matches("/x/%F0%9F%91%8D"));
        assert!(Pattern::parse("/users/@me").matches("/users/@me"));
    }

    #[test]
    fn multipart() {
        let body = b"--XX\r\nContent-Disposition: form-data; name=\"payload_json\"\r\n\r\n{\"a\":1}\r\n--XX\r\nContent-Disposition: form-data; name=\"files[0]\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--XX--\r\n";
        let parts = parse_multipart(body, "XX");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "payload_json");
        assert_eq!(parts[1].filename.as_deref(), Some("a.txt"));
        assert_eq!(parts[1].data, b"hi");
    }
}
