//! REST route handlers: thin adapters from HTTP to [`ops`](crate::ops).

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::AppState;
use crate::error::ApiError;
use crate::ops::{self, Res, parse_id};

fn json(r: Res) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

fn no_content(r: Res) -> Response {
    match r {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// Parses an optional JSON body; an empty body yields `default`.
fn body<T: DeserializeOwned>(bytes: &Bytes, default: Option<T>) -> Result<T, ApiError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return default.ok_or_else(|| ApiError::bad_request("request body is required"));
    }
    serde_json::from_slice(bytes).map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))
}

pub(crate) async fn not_found() -> Response {
    ApiError::not_found("no such endpoint").into_response()
}

pub(crate) async fn method_not_allowed() -> Response {
    ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "method not allowed for this endpoint").into_response()
}

pub(crate) async fn info(State(st): State<AppState>) -> Response {
    Json(ops::info(&st.client)).into_response()
}

pub(crate) async fn me(State(st): State<AppState>) -> Response {
    json(ops::me(&st.client))
}

pub(crate) async fn guilds(State(st): State<AppState>) -> Response {
    json(ops::guilds(&st.client))
}

pub(crate) async fn guild(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    json(parse_id(&id, "guild id").and_then(|id| ops::guild(&st.client, id)))
}

pub(crate) async fn guild_channels(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    json(parse_id(&id, "guild id").and_then(|id| ops::guild_channels(&st.client, id)))
}

pub(crate) async fn channel(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    json(parse_id(&id, "channel id").and_then(|id| ops::channel(&st.client, id)))
}

#[derive(Deserialize)]
pub(crate) struct MessagesQuery {
    limit: Option<String>,
    before: Option<String>,
}

pub(crate) async fn messages(State(st): State<AppState>, Path(id): Path<String>, Query(q): Query<MessagesQuery>) -> Response {
    let r = async {
        let id = parse_id(&id, "channel id")?;
        let limit = match q.limit.as_deref() {
            Some(l) => Some(l.parse::<u32>().map_err(|_| ApiError::bad_request("invalid limit"))?),
            None => None,
        };
        let before = q.before.as_deref().map(|b| parse_id(b, "before id")).transpose()?;
        ops::messages(&st.client, id, limit, before).await
    };
    json(r.await)
}

pub(crate) async fn user(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let r = async { ops::user(&st.client, parse_id(&id, "user id")?).await };
    json(r.await)
}

pub(crate) async fn dms(State(st): State<AppState>) -> Response {
    json(ops::dms(&st.client))
}

pub(crate) async fn relationships(State(st): State<AppState>) -> Response {
    json(ops::relationships(&st.client))
}

pub(crate) async fn send_message(State(st): State<AppState>, Path(id): Path<String>, bytes: Bytes) -> Response {
    let r = async {
        let id = parse_id(&id, "channel id")?;
        let b: ops::SendMessage = body(&bytes, None)?;
        ops::send_message(&st.client, id, b).await
    };
    json(r.await)
}

pub(crate) async fn typing(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let r = async { ops::typing(&st.client, parse_id(&id, "channel id")?).await };
    no_content(r.await)
}

#[derive(Deserialize, Default)]
pub(crate) struct AckBody {
    message_id: Option<String>,
}

pub(crate) async fn ack(State(st): State<AppState>, Path(id): Path<String>, bytes: Bytes) -> Response {
    let r = async {
        let id = parse_id(&id, "channel id")?;
        let b: AckBody = body(&bytes, Some(AckBody::default()))?;
        let message = b.message_id.as_deref().map(|m| parse_id(m, "message id")).transpose()?;
        ops::ack(&st.client, id, message).await
    };
    no_content(r.await)
}

async fn reaction(st: AppState, cid: String, mid: String, emoji: String, add: bool) -> Response {
    let r = async {
        let cid = parse_id(&cid, "channel id")?;
        let mid = parse_id(&mid, "message id")?;
        ops::reaction(&st.client, cid, mid, &emoji, add).await
    };
    no_content(r.await)
}

pub(crate) async fn add_reaction(State(st): State<AppState>, Path((cid, mid, emoji)): Path<(String, String, String)>) -> Response {
    reaction(st, cid, mid, emoji, true).await
}

pub(crate) async fn remove_reaction(State(st): State<AppState>, Path((cid, mid, emoji)): Path<(String, String, String)>) -> Response {
    reaction(st, cid, mid, emoji, false).await
}

pub(crate) async fn presence(State(st): State<AppState>, bytes: Bytes) -> Response {
    let r = async {
        let b: Value = body(&bytes, None)?;
        ops::presence(&st.client, b).await
    };
    json(r.await)
}
