//! Email/phone + password login and the MFA step that may follow.
//!
//! Mirrors the official client (`/auth/login` → optional `/auth/mfa/{type}`).
//! Captchas are handled transparently by the [`Http`] client's captcha solver.

use ferrocord_http::{Http, HttpError, Request};
use ferrocord_model::Snowflake;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AuthError;

/// MFA methods offered for a login ticket.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MfaStep {
    pub ticket: String,
    pub totp: bool,
    pub sms: bool,
    pub backup: bool,
    /// WebAuthn challenge JSON (security keys / passkeys), if enabled on the account.
    pub webauthn: Option<String>,
    pub login_instance_id: Option<String>,
    pub user_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MfaMethod {
    Totp,
    Sms,
    Backup,
    Webauthn,
}

impl MfaMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            MfaMethod::Totp => "totp",
            MfaMethod::Sms => "sms",
            MfaMethod::Backup => "backup",
            MfaMethod::Webauthn => "webauthn",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MfaMethod::Totp => "Authenticator app",
            MfaMethod::Sms => "SMS code",
            MfaMethod::Backup => "Backup code",
            MfaMethod::Webauthn => "Security key",
        }
    }
}

impl MfaStep {
    pub fn methods(&self) -> Vec<MfaMethod> {
        let mut m = vec![];
        if self.totp {
            m.push(MfaMethod::Totp);
        }
        if self.sms {
            m.push(MfaMethod::Sms);
        }
        if self.backup {
            m.push(MfaMethod::Backup);
        }
        if self.webauthn.is_some() {
            m.push(MfaMethod::Webauthn);
        }
        m
    }
}

#[derive(Debug, Clone)]
pub enum LoginOutcome {
    Success { token: String, user_id: Option<Snowflake> },
    Mfa(MfaStep),
    /// Login from a new location must be approved via an email link first.
    VerificationRequired { message: String },
    /// The account is suspended; the token only grants access to the appeal flow.
    Suspended { token: String },
}

/// `POST /auth/login`.
pub async fn login(http: &Http, login: &str, password: &str) -> Result<LoginOutcome, AuthError> {
    http.ensure_fingerprint().await;
    let req = Request::post("/auth/login").unauthenticated().json(json!({
        "login": login,
        "password": password,
        "undelete": false,
        "login_source": null,
        "gift_code_sku_id": null,
    }));
    let body = match http.send(req).await {
        Ok(v) => v,
        Err(e) => return Err(map_login_error(e)),
    };
    Ok(parse_login_response(&body))
}

fn parse_login_response(body: &Value) -> LoginOutcome {
    let user_id = body.get("user_id").and_then(|v| serde_json::from_value(v.clone()).ok());
    if body.get("mfa").and_then(Value::as_bool).unwrap_or(false) {
        let b = |k: &str| body.get(k).and_then(Value::as_bool).unwrap_or(false);
        return LoginOutcome::Mfa(MfaStep {
            ticket: body.get("ticket").and_then(Value::as_str).unwrap_or_default().into(),
            totp: b("totp"),
            sms: b("sms"),
            backup: b("backup"),
            webauthn: body.get("webauthn").and_then(Value::as_str).map(str::to_string),
            login_instance_id: body.get("login_instance_id").and_then(Value::as_str).map(str::to_string),
            user_id,
        });
    }
    match body.get("token").and_then(Value::as_str) {
        Some(t) => LoginOutcome::Success { token: t.to_string(), user_id },
        None => LoginOutcome::VerificationRequired {
            message: "Discord did not return a token. Check your email to verify this login location, then try again.".into(),
        },
    }
}

fn map_login_error(e: HttpError) -> AuthError {
    if let Some(raw) = e.raw_body() {
        if let Some(t) = raw.get("suspended_user_token").and_then(Value::as_str) {
            return AuthError::Suspended(t.to_string());
        }
    }
    match &e {
        HttpError::Api { body, .. } => {
            // 50035 with field errors on login/password = invalid credentials.
            let fields = body.field_errors();
            if let Some((field, msg)) = fields.first() {
                if field.starts_with("login") || field.starts_with("password") {
                    return AuthError::InvalidCredentials(msg.clone());
                }
                return AuthError::Rejected(format!("{field}: {msg}"));
            }
            if body.code == ferrocord_http::error::codes::ACCOUNT_LOGIN_VERIFICATION_EMAIL {
                return AuthError::NewLoginLocation(body.message.clone());
            }
            AuthError::Rejected(body.display_message())
        }
        HttpError::CaptchaRequired(_) => AuthError::CaptchaCancelled,
        _ => AuthError::Http(e),
    }
}

/// Completes login with a TOTP, SMS or backup code (`POST /auth/mfa/{type}`).
pub async fn verify_mfa(http: &Http, step: &MfaStep, method: MfaMethod, code: &str) -> Result<String, AuthError> {
    let code = code.trim().replace([' ', '-'], "");
    let req = Request::post(format!("/auth/mfa/{}", method.as_str())).unauthenticated().json(json!({
        "code": code,
        "ticket": step.ticket,
        "login_source": null,
        "gift_code_sku_id": null,
        "login_instance_id": step.login_instance_id,
    }));
    match http.send(req).await {
        Ok(v) => v.get("token").and_then(Value::as_str).map(str::to_string).ok_or_else(|| AuthError::Rejected("no token in response".into())),
        Err(HttpError::Api { body, .. }) if body.code == ferrocord_http::error::codes::MFA_INVALID_CODE => {
            Err(AuthError::InvalidMfaCode(body.message))
        }
        Err(e) => Err(map_login_error(e)),
    }
}

/// Requests an SMS code for the ticket. Returns the redacted phone number.
pub async fn send_sms(http: &Http, step: &MfaStep) -> Result<String, AuthError> {
    let v = http
        .send(Request::post("/auth/mfa/sms/send").unauthenticated().json(json!({ "ticket": step.ticket })))
        .await
        .map_err(map_login_error)?;
    Ok(v.get("phone").and_then(Value::as_str).unwrap_or("your phone").to_string())
}

/// Validates a token and returns the account it belongs to.
pub async fn validate_token(http: &Http, token: &str) -> Result<ferrocord_model::CurrentUser, AuthError> {
    let token = token.trim().trim_matches('"').to_string();
    let req = Request::get("/users/@me").unauthenticated().header("Authorization", token);
    match http.send_as(req).await {
        Ok(u) => Ok(u),
        Err(HttpError::Unauthorized) => Err(AuthError::InvalidToken),
        Err(HttpError::Api { status: 401, .. }) => Err(AuthError::InvalidToken),
        Err(e) => Err(AuthError::Http(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mfa() {
        let v = json!({"user_id":"1","mfa":true,"sms":false,"webauthn":null,"ticket":"T","backup":true,"totp":true,"login_instance_id":"x"});
        match parse_login_response(&v) {
            LoginOutcome::Mfa(m) => {
                assert_eq!(m.ticket, "T");
                assert_eq!(m.methods(), vec![MfaMethod::Totp, MfaMethod::Backup]);
            }
            other => panic!("{other:?}"),
        }
        let v = json!({"user_id":"1","token":"abc"});
        assert!(matches!(parse_login_response(&v), LoginOutcome::Success { .. }));
    }
}
