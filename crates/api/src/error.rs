//! The JSON error type shared by REST responses and WebSocket RPC replies.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ferrocord_core::http::HttpError;
use serde_json::{Value, json};

/// An API failure: rendered as `{"error": message, "code": code}` with `status`.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    /// Stable machine-readable code (`unauthorized`, `not_found`, …).
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid API token")
    }

    pub fn actions_disabled() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "actions_disabled",
            "write actions are disabled; enable \"Allow actions\" in Ferrocord's API settings",
        )
    }

    pub fn not_ready() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "not_ready", "Ferrocord is not logged in yet")
    }

    /// Body used inside WebSocket RPC error replies.
    pub fn to_json(&self) -> Value {
        json!({ "error": self.message, "code": self.code, "status": self.status.as_u16() })
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut res = (self.status, Json(json!({ "error": self.message, "code": self.code }))).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        res
    }
}

impl From<HttpError> for ApiError {
    fn from(e: HttpError) -> Self {
        let message = ferrocord_core::error_text(&e);
        match &e {
            HttpError::Unauthorized => Self::new(StatusCode::SERVICE_UNAVAILABLE, "not_ready", "Ferrocord is not logged in"),
            HttpError::RateLimited(_) => Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message),
            HttpError::Api { status, .. } => match *status {
                400 => Self::new(StatusCode::BAD_REQUEST, "discord_bad_request", message),
                403 => Self::new(StatusCode::FORBIDDEN, "discord_forbidden", message),
                404 => Self::new(StatusCode::NOT_FOUND, "not_found", message),
                429 => Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message),
                _ => Self::new(StatusCode::BAD_GATEWAY, "discord_error", message),
            },
            _ => Self::new(StatusCode::BAD_GATEWAY, "discord_error", message),
        }
    }
}
