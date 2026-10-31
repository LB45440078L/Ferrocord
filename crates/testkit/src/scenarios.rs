//! Ready-made REST scenarios built on scripted routes: captcha challenges,
//! MFA re-authentication and rate limits.

use serde_json::{Value, json};

use crate::MockDiscord;
use crate::rest::MockResponse;

/// A captcha challenge (`400 {captcha_key: [...]}`) and the solution that unlocks it.
#[derive(Debug, Clone)]
pub struct CaptchaGate {
    pub service: String,
    pub sitekey: String,
    pub rqdata: Option<String>,
    pub rqtoken: Option<String>,
    pub session_id: Option<String>,
    pub invisible: bool,
    /// Accepted `X-Captcha-Key`.
    pub solution: String,
}

impl Default for CaptchaGate {
    fn default() -> Self {
        Self {
            service: "hcaptcha".into(),
            sitekey: "a9b5fb07-92ff-493f-86fe-352a2803b3df".into(),
            rqdata: Some("mock-rqdata".into()),
            rqtoken: Some("mock-rqtoken".into()),
            session_id: Some("mock-captcha-session".into()),
            invisible: false,
            solution: "P1_mock-solution".into(),
        }
    }
}

impl CaptchaGate {
    /// The challenge body Discord sends.
    pub fn body(&self) -> Value {
        json!({
            "captcha_key": ["captcha-required"], "captcha_sitekey": self.sitekey, "captcha_service": self.service,
            "captcha_rqdata": self.rqdata, "captcha_rqtoken": self.rqtoken, "captcha_session_id": self.session_id,
            "should_serve_invisible": self.invisible,
        })
    }
}

/// Sensitive-action MFA: `401 {code: 60003, mfa: {ticket, methods}}` until the
/// request carries `X-Discord-MFA-Authorization: <token>`, which `POST /mfa/finish`
/// issues for `{ticket, data: code}`.
#[derive(Debug, Clone)]
pub struct MfaGate {
    pub ticket: String,
    pub methods: Vec<String>,
    pub code: String,
    pub token: String,
}

impl Default for MfaGate {
    fn default() -> Self {
        Self { ticket: "mock-mfa-ticket".into(), methods: vec!["totp".into(), "password".into()], code: "123456".into(), token: "mock-mfa-jwt".into() }
    }
}

impl MfaGate {
    pub fn body(&self) -> Value {
        let methods: Vec<Value> = self.methods.iter().map(|m| json!({ "type": m })).collect();
        json!({ "message": "Two factor is required for this operation", "code": 60003, "mfa": { "ticket": self.ticket, "methods": methods } })
    }
}

impl MockDiscord {
    /// Matching requests get a captcha challenge until they carry the gate's
    /// solution (plus its rqtoken and session id when set); then they fall
    /// through to the world / older routes.
    pub fn require_captcha(&self, method: &str, pattern: &str, gate: CaptchaGate) {
        self.on(method, pattern).handle(move |req| {
            let solved = req.header("x-captcha-key") == Some(gate.solution.as_str())
                && gate.rqtoken.as_deref().is_none_or(|t| req.header("x-captcha-rqtoken") == Some(t))
                && gate.session_id.as_deref().is_none_or(|s| req.header("x-captcha-session-id") == Some(s));
            (!solved).then(|| MockResponse::json(400, gate.body()))
        });
    }

    /// Matching requests need sensitive-action MFA (see [`MfaGate`]).
    pub fn require_mfa(&self, method: &str, pattern: &str, gate: MfaGate) {
        self.with_world(|w| w.mfa_finish = Some((gate.ticket.clone(), gate.code.clone(), gate.token.clone())));
        self.on(method, pattern).handle(move |req| {
            let ok = req.header("x-discord-mfa-authorization") == Some(gate.token.as_str());
            (!ok).then(|| MockResponse::json(401, gate.body()))
        });
    }

    /// The next `times` matching requests get `429` with `retry_after` (seconds).
    pub fn rate_limit(&self, method: &str, pattern: &str, retry_after: f64, global: bool, times: usize) {
        self.on(method, pattern).times(times).respond(MockResponse::rate_limited(retry_after, global));
    }
}
