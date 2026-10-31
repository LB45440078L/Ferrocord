use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{ApiErrorBody, HttpError, codes};
use crate::profile::ClientProfile;
use crate::ratelimit::RateLimiter;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub const API_BASE: &str = "https://discord.com/api/v9";

/// A captcha the server wants solved before it accepts the request
/// (fields from a 400 response containing `captcha_key`).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CaptchaChallenge {
    /// `"hcaptcha"` or `"recaptcha"`.
    pub service: String,
    pub sitekey: String,
    pub rqdata: Option<String>,
    pub rqtoken: Option<String>,
    pub session_id: Option<String>,
    pub invisible: bool,
    pub user_flow: Option<String>,
}

impl CaptchaChallenge {
    pub fn from_body(v: &Value) -> Self {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Self {
            service: s("captcha_service").unwrap_or_else(|| "hcaptcha".into()),
            sitekey: s("captcha_sitekey").unwrap_or_default(),
            rqdata: s("captcha_rqdata"),
            rqtoken: s("captcha_rqtoken"),
            session_id: s("captcha_session_id"),
            invisible: v.get("should_serve_invisible").and_then(Value::as_bool).unwrap_or(false),
            user_flow: s("user_flow"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptchaSolution {
    pub key: String,
    pub rqtoken: Option<String>,
    pub session_id: Option<String>,
}

/// Implemented by the UI to present a captcha to the user.
pub trait CaptchaSolver: Send + Sync {
    /// Returns `None` if the user cancelled.
    fn solve(&self, challenge: CaptchaChallenge) -> BoxFuture<'static, Option<CaptchaSolution>>;
}

/// MFA challenge for sensitive actions (HTTP 401, code 60003).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MfaChallenge {
    pub ticket: String,
    /// Available method types: `totp`, `sms`, `backup`, `password`, `webauthn`.
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MfaResponse {
    pub mfa_type: String,
    pub data: String,
}

pub trait MfaHandler: Send + Sync {
    fn verify(&self, challenge: MfaChallenge) -> BoxFuture<'static, Option<MfaResponse>>;
}

#[derive(Clone)]
pub enum Body {
    Json(Value),
    /// `payload_json` plus `files[n]` parts (re-buildable for retries).
    Multipart {
        payload: Value,
        files: Vec<FilePart>,
    },
}

#[derive(Clone)]
pub struct FilePart {
    pub filename: String,
    pub content_type: Option<String>,
    pub data: bytes_shim::Bytes,
}

/// Small alias module so callers don't need a direct `bytes` dependency.
pub mod bytes_shim {
    pub type Bytes = std::sync::Arc<Vec<u8>>;
}

#[derive(Clone)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Body>,
    pub headers: Vec<(String, String)>,
    pub authenticated: bool,
}

impl Request {
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self { method, path: path.into(), query: vec![], body: None, headers: vec![], authenticated: true }
    }
    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }
    pub fn post(path: impl Into<String>) -> Self {
        Self::new(Method::POST, path)
    }
    pub fn patch(path: impl Into<String>) -> Self {
        Self::new(Method::PATCH, path)
    }
    pub fn put(path: impl Into<String>) -> Self {
        Self::new(Method::PUT, path)
    }
    pub fn delete(path: impl Into<String>) -> Self {
        Self::new(Method::DELETE, path)
    }
    pub fn json(mut self, body: impl Serialize) -> Self {
        self.body = Some(Body::Json(serde_json::to_value(body).unwrap_or(Value::Null)));
        self
    }
    pub fn query(mut self, k: &str, v: impl ToString) -> Self {
        self.query.push((k.into(), v.to_string()));
        self
    }
    pub fn query_opt(self, k: &str, v: Option<impl ToString>) -> Self {
        match v {
            Some(v) => self.query(k, v),
            None => self,
        }
    }
    pub fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn unauthenticated(mut self) -> Self {
        self.authenticated = false;
        self
    }
    /// Audit log reason for moderation actions.
    pub fn reason(self, reason: &str) -> Self {
        // Percent-encode like encodeURIComponent: form encoding would turn spaces into '+',
        // which Discord shows literally in the audit log.
        let enc = url::form_urlencoded::byte_serialize(reason.as_bytes()).collect::<String>().replace('+', "%20");
        self.header("X-Audit-Log-Reason", enc)
    }
}

struct Inner {
    client: reqwest::Client,
    base: String,
    token: RwLock<Option<String>>,
    fingerprint: RwLock<Option<String>>,
    profile: RwLock<ClientProfile>,
    limiter: RateLimiter,
    captcha: RwLock<Option<Arc<dyn CaptchaSolver>>>,
    mfa: RwLock<Option<Arc<dyn MfaHandler>>>,
}

/// Cheaply cloneable REST client.
#[derive(Clone)]
pub struct Http {
    inner: Arc<Inner>,
}

impl Http {
    pub fn new(profile: ClientProfile) -> Self {
        Self::with_base_url(profile, API_BASE)
    }

    /// Creates a client against a different API root (mock servers, staging).
    /// `base` must include the version path, e.g. `http://127.0.0.1:1234/api/v9`.
    pub fn with_base_url(profile: ClientProfile, base: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(profile.user_agent())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("failed to build HTTP client");
        Self {
            inner: Arc::new(Inner {
                client,
                base: base.trim_end_matches('/').to_string(),
                token: RwLock::new(None),
                fingerprint: RwLock::new(None),
                profile: RwLock::new(profile),
                limiter: RateLimiter::default(),
                captcha: RwLock::new(None),
                mfa: RwLock::new(None),
            }),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.inner.base
    }

    pub fn raw_client(&self) -> &reqwest::Client {
        &self.inner.client
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.inner.token.write() = token;
    }

    pub fn token(&self) -> Option<String> {
        self.inner.token.read().clone()
    }

    /// Fetches the anonymous tracking fingerprint the client sends as
    /// `X-Fingerprint` on pre-login requests (login, register, remote auth).
    pub async fn ensure_fingerprint(&self) -> Option<String> {
        if let Some(f) = self.inner.fingerprint.read().clone() {
            return Some(f);
        }
        let v = self.send(Request::get("/experiments").unauthenticated().query("with_guild_experiments", "true")).await.ok()?;
        let fp = v.get("fingerprint").and_then(Value::as_str)?.to_string();
        *self.inner.fingerprint.write() = Some(fp.clone());
        Some(fp)
    }

    pub fn profile(&self) -> ClientProfile {
        self.inner.profile.read().clone()
    }

    pub fn set_profile(&self, p: ClientProfile) {
        *self.inner.profile.write() = p;
    }

    pub fn set_captcha_solver(&self, s: Option<Arc<dyn CaptchaSolver>>) {
        *self.inner.captcha.write() = s;
    }

    pub fn set_mfa_handler(&self, h: Option<Arc<dyn MfaHandler>>) {
        *self.inner.mfa.write() = h;
    }

    /// Refreshes `client_build_number` from the live web app. Best effort.
    pub async fn refresh_build_number(&self) -> Option<u64> {
        let n = crate::profile::fetch_build_number(&self.inner.client).await?;
        self.inner.profile.write().client_build_number = n;
        tracing::info!(build = n, "using live client build number");
        Some(n)
    }

    fn default_headers(&self, req: &Request) -> HeaderMap {
        let p = self.inner.profile.read();
        let mut h = HeaderMap::new();
        let mut put = |k: &'static str, v: String| {
            if let Ok(v) = HeaderValue::from_str(&v) {
                h.insert(k, v);
            }
        };
        put("x-super-properties", p.super_properties_b64());
        put("x-discord-locale", p.discord_locale());
        put("x-discord-timezone", p.timezone.clone());
        put("x-debug-options", "bugReporterEnabled".into());
        put("accept-language", p.discord_locale());
        put("origin", "https://discord.com".into());
        put("referer", "https://discord.com/channels/@me".into());
        let token = if req.authenticated { self.inner.token.read().clone() } else { None };
        match token {
            Some(t) => put("authorization", t),
            None => {
                if let Some(f) = self.inner.fingerprint.read().clone() {
                    put("x-fingerprint", f);
                }
            }
        }
        for (k, v) in &req.headers {
            if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
                h.insert(k, v);
            }
        }
        h
    }

    fn build(&self, req: &Request) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.inner.base, req.path);
        let mut rb = self.inner.client.request(req.method.clone(), url).headers(self.default_headers(req));
        if !req.query.is_empty() {
            rb = rb.query(&req.query);
        }
        match &req.body {
            Some(Body::Json(v)) => rb = rb.json(v),
            Some(Body::Multipart { payload, files }) => {
                let mut form = reqwest::multipart::Form::new().text("payload_json", payload.to_string());
                for (i, f) in files.iter().enumerate() {
                    let make = || reqwest::multipart::Part::bytes(f.data.as_ref().clone()).file_name(f.filename.clone());
                    let part = match &f.content_type {
                        Some(ct) => make().mime_str(ct).unwrap_or_else(|_| make()),
                        None => make(),
                    };
                    form = form.part(format!("files[{i}]"), part);
                }
                rb = rb.multipart(form);
            }
            None => {
                if matches!(req.method, Method::POST | Method::PUT | Method::PATCH) {
                    rb = rb.header("content-length", "0");
                }
            }
        }
        rb
    }

    /// Sends a request and returns the raw JSON body (`Value::Null` for 204).
    pub async fn send(&self, mut req: Request) -> Result<Value, HttpError> {
        let mut attempts = 0u32;
        let mut captcha_attempts = 0u32;
        loop {
            attempts += 1;
            let method = req.method.as_str().to_string();
            self.inner.limiter.acquire(&method, &req.path).await;
            let resp = self.build(&req).send().await?;
            let status = resp.status();
            self.inner.limiter.update(&method, &req.path, resp.headers());
            let bytes = resp.bytes().await?;
            let body: Value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };

            if status.is_success() {
                return Ok(body);
            }

            if status == StatusCode::TOO_MANY_REQUESTS {
                let info: ApiErrorBody = serde_json::from_value(body.clone()).unwrap_or_default();
                let wait = info.retry_after.unwrap_or(1.0);
                if info.global.unwrap_or(false) {
                    self.inner.limiter.set_global(wait);
                }
                if wait > 60.0 || attempts > 5 {
                    return Err(HttpError::RateLimited(wait));
                }
                tracing::warn!(path = %req.path, wait, "429 rate limited");
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                continue;
            }

            if status == StatusCode::BAD_REQUEST && body.get("captcha_key").is_some() {
                let solver = self.inner.captcha.read().clone();
                let Some(solver) = solver else { return Err(HttpError::CaptchaRequired(body)) };
                if captcha_attempts >= 3 {
                    return Err(HttpError::CaptchaRequired(body));
                }
                captcha_attempts += 1;
                let challenge = CaptchaChallenge::from_body(&body);
                match solver.solve(challenge).await {
                    Some(sol) => {
                        req.headers.retain(|(k, _)| !k.to_ascii_lowercase().starts_with("x-captcha-"));
                        req.headers.push(("X-Captcha-Key".into(), sol.key));
                        if let Some(t) = sol.rqtoken {
                            req.headers.push(("X-Captcha-Rqtoken".into(), t));
                        }
                        if let Some(s) = sol.session_id {
                            req.headers.push(("X-Captcha-Session-Id".into(), s));
                        }
                        continue;
                    }
                    None => return Err(HttpError::CaptchaRequired(body)),
                }
            }

            let err: ApiErrorBody = serde_json::from_value(body.clone()).unwrap_or_default();

            if status == StatusCode::UNAUTHORIZED && err.code == codes::MFA_REQUIRED && body.get("mfa").is_some() {
                let handler = self.inner.mfa.read().clone();
                let Some(handler) = handler else { return Err(HttpError::MfaRequired(body)) };
                let mfa = &body["mfa"];
                let challenge = MfaChallenge {
                    ticket: mfa.get("ticket").and_then(Value::as_str).unwrap_or_default().into(),
                    methods: mfa
                        .get("methods")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|m| m.get("type").and_then(Value::as_str).map(str::to_string)).collect())
                        .unwrap_or_default(),
                };
                let ticket = challenge.ticket.clone();
                let Some(answer) = handler.verify(challenge).await else { return Err(HttpError::MfaRequired(body)) };
                let token = self.mfa_finish(&ticket, &answer).await?;
                req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("x-discord-mfa-authorization"));
                req.headers.push(("X-Discord-MFA-Authorization".into(), token));
                if attempts > 4 {
                    return Err(HttpError::MfaRequired(body));
                }
                continue;
            }

            if status == StatusCode::UNAUTHORIZED && err.code == 0 {
                return Err(HttpError::Unauthorized);
            }

            if status.is_server_error() && attempts < 3 {
                tokio::time::sleep(Duration::from_millis(500 * attempts as u64)).await;
                continue;
            }

            return Err(HttpError::Api { status: status.as_u16(), body: err, raw: body });
        }
    }

    async fn mfa_finish(&self, ticket: &str, answer: &MfaResponse) -> Result<String, HttpError> {
        let v = Box::pin(self.send(Request::post("/mfa/finish").json(serde_json::json!({
            "ticket": ticket,
            "mfa_type": answer.mfa_type,
            "data": answer.data,
        }))))
        .await?;
        Ok(v.get("token").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    /// Sends a request and deserializes the response.
    pub async fn send_as<T: DeserializeOwned>(&self, req: Request) -> Result<T, HttpError> {
        let v = self.send(req).await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Sends a request, discarding the body.
    pub async fn send_unit(&self, req: Request) -> Result<(), HttpError> {
        self.send(req).await.map(|_| ())
    }
}
