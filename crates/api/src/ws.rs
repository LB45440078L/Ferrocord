//! `GET /v1/ws`: live event stream plus JSON-RPC-style requests.
//!
//! Server → client frames:
//! * `{"type":"hello", ...}` once after connecting,
//! * `{"type":"event","event":{"type":NAME,"data":...}}` for each client event,
//! * `{"type":"response","id":ID,"result":...}` / `{"type":"response","id":ID,"error":{...}}`,
//! * `{"type":"subscribed","events":[...]}` after a subscribe op,
//! * `{"type":"lagged","missed":N}` when the connection fell behind and events were dropped.
//!
//! Client → server frames: `{"op":"subscribe","events":[...]}` or `{"id":ID,"method":M,"params":{...}}`.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::AppState;
use crate::error::ApiError;
use crate::ops;

/// Maximum concurrent in-flight RPC calls per connection.
const MAX_IN_FLIGHT: usize = 32;
const MAX_FRAME: usize = 1 << 20;

pub(crate) async fn handler(State(st): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_FRAME).max_frame_size(MAX_FRAME).on_upgrade(move |socket| session(st, socket))
}

fn text(v: Value) -> Message {
    Message::Text(v.to_string().into())
}

async fn session(st: AppState, mut socket: WebSocket) {
    let mut events = st.client.subscribe();
    let mut shutdown = st.shutdown.clone();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let in_flight = Arc::new(AtomicUsize::new(0));
    // `None` = every event.
    let mut filter: Option<HashSet<String>> = None;

    let hello = json!({
        "type": "hello",
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": ops::API_VERSION,
        "actions_allowed": st.client.config().api.allow_actions,
        "methods": ops::METHODS,
    });
    if socket.send(text(hello)).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(ev) => {
                    if filter.as_ref().is_some_and(|f| !f.contains(ev.name())) {
                        continue;
                    }
                    if socket.send(text(json!({ "type": "event", "event": ev.to_json() }))).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!(missed = n, "API websocket lagged");
                    if socket.send(text(json!({ "type": "lagged", "missed": n }))).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => {
                    if let Some(reply) = on_text(&st, t.as_str(), &mut filter, &out_tx, &in_flight)
                        && socket.send(text(reply)).await.is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Binary(_))) => {
                    let e = ApiError::bad_request("binary frames are not supported");
                    if socket.send(text(json!({ "type": "error", "error": e.to_json() }))).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            Some(out) = out_rx.recv() => {
                if socket.send(text(out)).await.is_err() {
                    break;
                }
            }
            _ = shutdown.changed() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
        }
    }
}

/// Handles one client frame. Returns an immediate reply, if any; RPC results
/// are delivered later through `out`.
fn on_text(
    st: &AppState,
    raw: &str,
    filter: &mut Option<HashSet<String>>,
    out: &mpsc::UnboundedSender<Value>,
    in_flight: &Arc<AtomicUsize>,
) -> Option<Value> {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return Some(json!({ "type": "error", "error": ApiError::bad_request(format!("invalid JSON: {e}")).to_json() })),
    };
    if let Some(op) = v.get("op").and_then(Value::as_str) {
        return Some(match op {
            "subscribe" => {
                let names: Option<HashSet<String>> = v.get("events").and_then(Value::as_array).map(|a| {
                    a.iter().filter_map(Value::as_str).map(|s| s.trim().to_ascii_uppercase()).collect()
                });
                *filter = names.filter(|n| !n.is_empty() && !n.contains("*"));
                let mut list: Vec<&String> = filter.iter().flatten().collect();
                list.sort();
                json!({ "type": "subscribed", "events": if filter.is_some() { json!(list) } else { json!(["*"]) } })
            }
            _ => json!({ "type": "error", "error": ApiError::bad_request(format!("unknown op `{op}`")).to_json() }),
        });
    }
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = v.get("method").and_then(Value::as_str).map(str::to_string) else {
        let e = ApiError::bad_request("expected {\"op\": ...} or {\"id\": ..., \"method\": ..., \"params\": ...}");
        return Some(json!({ "type": "response", "id": id, "error": e.to_json() }));
    };
    if in_flight.load(Ordering::SeqCst) >= MAX_IN_FLIGHT {
        let e = ApiError::new(axum::http::StatusCode::TOO_MANY_REQUESTS, "too_many_requests", "too many requests in flight");
        return Some(json!({ "type": "response", "id": id, "error": e.to_json() }));
    }
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    let client = st.client.clone();
    let out = out.clone();
    let counter = in_flight.clone();
    counter.fetch_add(1, Ordering::SeqCst);
    tokio::spawn(async move {
        let reply = match ops::dispatch(&client, &method, params).await {
            Ok(result) => json!({ "type": "response", "id": id, "result": result }),
            Err(e) => json!({ "type": "response", "id": id, "error": e.to_json() }),
        };
        counter.fetch_sub(1, Ordering::SeqCst);
        let _ = out.send(reply);
    });
    None
}
