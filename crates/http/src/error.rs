use serde::Deserialize;
use serde_json::Value;

/// JSON error body returned by the API (`{"code":50001,"message":"Missing Access","errors":{...}}`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ApiErrorBody {
    pub code: u32,
    pub message: String,
    pub errors: Option<Value>,
    pub retry_after: Option<f64>,
    pub global: Option<bool>,
}

impl ApiErrorBody {
    /// Flattens nested field errors (`errors.login._errors[0].message`) into readable lines.
    pub fn field_errors(&self) -> Vec<(String, String)> {
        fn walk(path: &str, v: &Value, out: &mut Vec<(String, String)>) {
            match v {
                Value::Object(m) => {
                    if let Some(Value::Array(errs)) = m.get("_errors") {
                        for e in errs {
                            let msg = e.get("message").and_then(Value::as_str).unwrap_or("invalid").to_string();
                            out.push((path.to_string(), msg));
                        }
                    }
                    for (k, v) in m {
                        if k != "_errors" {
                            let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                            walk(&p, v, out);
                        }
                    }
                }
                Value::Array(a) => {
                    for (i, v) in a.iter().enumerate() {
                        walk(&format!("{path}[{i}]"), v, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = vec![];
        if let Some(e) = &self.errors {
            walk("", e, &mut out);
        }
        out
    }

    /// The most user-presentable message: first field error, else the top-level message.
    pub fn display_message(&self) -> String {
        self.field_errors().into_iter().next().map(|(_, m)| m).unwrap_or_else(|| self.message.clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("discord API error {status} (code {}): {}", body.code, body.display_message())]
    Api { status: u16, body: ApiErrorBody, raw: Value },
    #[error("captcha required but no solver is available or it was cancelled")]
    CaptchaRequired(Value),
    #[error("MFA verification required but no handler is available or it was cancelled")]
    MfaRequired(Value),
    #[error("not authenticated")]
    Unauthorized,
    #[error("invalid response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("rate limited for too long ({0:.1}s)")]
    RateLimited(f64),
}

impl HttpError {
    pub fn code(&self) -> Option<u32> {
        match self {
            HttpError::Api { body, .. } => Some(body.code),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            HttpError::Api { status, .. } => Some(*status),
            HttpError::Unauthorized => Some(401),
            _ => None,
        }
    }

    pub fn raw_body(&self) -> Option<&Value> {
        match self {
            HttpError::Api { raw, .. } => Some(raw),
            HttpError::CaptchaRequired(v) | HttpError::MfaRequired(v) => Some(v),
            _ => None,
        }
    }
}

/// Well-known API JSON error codes.
pub mod codes {
    pub const UNKNOWN_MESSAGE: u32 = 10008;
    pub const MISSING_ACCESS: u32 = 50001;
    pub const INVALID_FORM_BODY: u32 = 50035;
    pub const MFA_REQUIRED: u32 = 60003;
    pub const MFA_INVALID_CODE: u32 = 60008;
    pub const ACCOUNT_LOGIN_VERIFICATION_EMAIL: u32 = 20013;
    pub const INVALID_LOGIN: u32 = 50035;
}
