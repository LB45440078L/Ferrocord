//! End-to-end tests against a real (logged-out) client and a live server.

use std::net::SocketAddr;
use std::time::Duration;

use ferrocord_api::{ApiServer, ApiServerHandle};
use ferrocord_core::{Client, ClientEvent, ConnectionState, Paths};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

struct Env {
    _dir: tempfile::TempDir,
    client: Client,
    server: ApiServerHandle,
}

fn start() -> Env {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data"), cache_dir: dir.path().join("cache") };
    let client = Client::new(tokio::runtime::Handle::current(), paths);
    client.update_config(|c| {
        c.api.enabled = true;
        c.api.port = 0;
    });
    let server = ApiServer::start(client.clone()).unwrap();
    Env { _dir: dir, client, server }
}

/// Sends a raw HTTP/1.1 request so every header (Host, Origin…) is under our control.
async fn raw(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, String)], body: Option<&str>) -> (u16, Value) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    let status: u16 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.trim()).unwrap_or("");
    (status, if body.is_empty() { Value::Null } else { serde_json::from_str(body).unwrap_or(Value::String(body.into())) })
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Next text frame as JSON.
async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("frame").unwrap().unwrap();
        if let Message::Text(t) = msg {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

fn auth(e: &Env) -> Vec<(&'static str, String)> {
    vec![("Host", format!("127.0.0.1:{}", e.server.addr().port())), ("Authorization", format!("Bearer {}", e.server.token()))]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn info_requires_valid_token() {
    let e = start();
    let addr = e.server.addr();
    assert!(addr.ip().is_loopback());
    let host = ("Host", format!("127.0.0.1:{}", addr.port()));

    let (status, body) = raw(addr, "GET", "/v1/info", std::slice::from_ref(&host), None).await;
    assert_eq!(status, 401);
    assert_eq!(body["code"], "unauthorized");

    let (status, _) = raw(addr, "GET", "/v1/info", &[host.clone(), ("Authorization", "Bearer nope".into())], None).await;
    assert_eq!(status, 401);

    // The query-string token is only accepted for the WebSocket endpoint.
    let (status, _) = raw(addr, "GET", &format!("/v1/info?token={}", e.server.token()), &[host], None).await;
    assert_eq!(status, 401);

    let (status, body) = raw(addr, "GET", "/v1/info", &auth(&e), None).await;
    assert_eq!(status, 200);
    assert_eq!(body["api_version"], 1);
    assert_eq!(body["logged_in"], false);
    assert_eq!(body["actions_allowed"], false);
    assert_eq!(body["connection"], "Disconnected");

    // The token is persisted in the config dir.
    let saved = std::fs::read_to_string(ferrocord_api::token_path(e.client.paths())).unwrap();
    assert_eq!(saved, e.server.token());

    // Regenerating invalidates the old token immediately.
    let old = e.server.token();
    let new = e.server.regenerate_token().unwrap();
    assert_ne!(old, new);
    let mut stale = auth(&e);
    stale[1].1 = format!("Bearer {old}");
    assert_eq!(raw(addr, "GET", "/v1/info", &stale, None).await.0, 401);
    assert_eq!(raw(addr, "GET", "/v1/info", &auth(&e), None).await.0, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_foreign_host_and_origin() {
    let e = start();
    let addr = e.server.addr();
    let port = addr.port();

    let mut h = auth(&e);
    h[0].1 = format!("evil.example:{port}");
    let (status, body) = raw(addr, "GET", "/v1/info", &h, None).await;
    assert_eq!(status, 403);
    assert_eq!(body["code"], "forbidden_host");

    h[0].1 = "127.0.0.1".into();
    assert_eq!(raw(addr, "GET", "/v1/info", &h, None).await.0, 403);

    h[0].1 = format!("localhost:{port}");
    assert_eq!(raw(addr, "GET", "/v1/info", &h, None).await.0, 200);

    let mut h = auth(&e);
    h.push(("Origin", "https://evil.example".into()));
    let (status, body) = raw(addr, "GET", "/v1/info", &h, None).await;
    assert_eq!(status, 403);
    assert_eq!(body["code"], "forbidden_origin");

    let mut h = auth(&e);
    h.push(("Origin", "null".into()));
    assert_eq!(raw(addr, "GET", "/v1/info", &h, None).await.0, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_actions_need_permission() {
    let e = start();
    let addr = e.server.addr();
    let h = auth(&e);

    let cases: [(&str, &str, Option<&str>); 6] = [
        ("POST", "/v1/channels/1/messages", Some(r#"{"content":"hi"}"#)),
        ("POST", "/v1/channels/1/typing", None),
        ("POST", "/v1/channels/1/ack", None),
        ("PUT", "/v1/channels/1/messages/2/reactions/%F0%9F%91%8D", None),
        ("DELETE", "/v1/channels/1/messages/2/reactions/%F0%9F%91%8D", None),
        ("PATCH", "/v1/presence", Some(r#"{"status":"idle"}"#)),
    ];
    for (method, path, body) in cases {
        let (status, json) = raw(addr, method, path, &h, body).await;
        assert_eq!(status, 403, "{method} {path}");
        assert_eq!(json["code"], "actions_disabled", "{method} {path}");
    }

    // The flag is read live: once allowed, the request gets past the check
    // (and fails only because nobody is logged in).
    e.client.update_config(|c| c.api.allow_actions = true);
    let (status, json) = raw(addr, "POST", "/v1/channels/1/messages", &h, Some(r#"{"content":"hi"}"#)).await;
    assert_eq!(status, 503);
    assert_eq!(json["code"], "not_ready");
    let (status, json) = raw(addr, "POST", "/v1/channels/1/messages", &h, Some("{oops")).await;
    assert_eq!(status, 400, "{json}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_and_errors_are_json() {
    let e = start();
    let addr = e.server.addr();
    let h = auth(&e);
    let (status, body) = raw(addr, "GET", "/v1/me", &h, None).await;
    assert_eq!((status, body["code"].as_str()), (503, Some("not_ready")));
    let (status, body) = raw(addr, "GET", "/v1/nope", &h, None).await;
    assert_eq!((status, body["code"].as_str()), (404, Some("not_found")));
    let (status, body) = raw(addr, "DELETE", "/v1/info", &h, None).await;
    assert_eq!((status, body["code"].as_str()), (405, Some("method_not_allowed")));
    let (status, body) = raw(addr, "GET", "/v1/guilds/abc", &h, None).await;
    assert_eq!((status, body["code"].as_str()), (400, Some("bad_request")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_events_and_rpc() {
    let e = start();
    let port = e.server.addr().port();

    let unauthorized = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/v1/ws")).await;
    assert!(unauthorized.is_err());

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/v1/ws?token={}", e.server.token())).await.unwrap();
    let hello = recv(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["actions_allowed"], false);

    ws.send(Message::Text(json!({"op": "subscribe", "events": ["latency"]}).to_string().into())).await.unwrap();
    let sub = recv(&mut ws).await;
    assert_eq!(sub, json!({"type": "subscribed", "events": ["LATENCY"]}));

    e.client.emit(ClientEvent::Connection(ConnectionState::Connecting));
    e.client.emit(ClientEvent::Latency(Duration::from_millis(42)));
    let ev = recv(&mut ws).await;
    assert_eq!(ev, json!({"type": "event", "event": {"type": "LATENCY", "data": 42}}));

    ws.send(Message::Text(json!({"id": 1, "method": "info"}).to_string().into())).await.unwrap();
    let r = recv(&mut ws).await;
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["api_version"], 1);

    ws.send(Message::Text(json!({"id": "two", "method": "send_message", "params": {"channel_id": "1", "content": "x"}}).to_string().into())).await.unwrap();
    let r = recv(&mut ws).await;
    assert_eq!(r["id"], "two");
    assert_eq!(r["error"]["code"], "actions_disabled");

    ws.send(Message::Text(json!({"id": 3, "method": "nope"}).to_string().into())).await.unwrap();
    assert_eq!(recv(&mut ws).await["error"]["code"], "unknown_method");

    ws.send(Message::Text("not json".into())).await.unwrap();
    assert_eq!(recv(&mut ws).await["type"], "error");

    // Stopping the server closes live sessions and refuses new connections.
    e.server.stop();
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(tokio::net::TcpStream::connect(e.server.addr()).await.is_err());
}
