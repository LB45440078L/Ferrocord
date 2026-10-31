//! Mock gateway: HELLO, IDENTIFY → READY, heartbeats, RESUME with replay, and
//! server-initiated RECONNECT / INVALID_SESSION / close codes. Messages are
//! zlib-stream compressed when the client asks for `compress=zlib-stream`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, State};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::zlib::{ZlibStreamEncoder, split_frames};
use crate::{DEFAULT_WAIT, Shared};

/// One websocket connection made to the gateway.
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// 0-based, in connection order.
    pub id: usize,
    /// `/gateway/` or `/resume/` (the `resume_gateway_url`).
    pub path: String,
    pub query: String,
    pub compressed: bool,
    pub open: bool,
    pub user_agent: Option<String>,
    pub origin: Option<String>,
    pub session_id: Option<String>,
    pub identified: bool,
    pub resumed: bool,
    /// Close code sent by the client, if it closed the socket with a close frame.
    pub client_close_code: Option<u16>,
    /// Close code the mock sent, if any.
    pub server_close_code: Option<u16>,
}

/// A payload received from the client.
#[derive(Debug, Clone)]
pub struct ClientFrame {
    pub conn: usize,
    pub op: u8,
    pub d: Value,
    pub at: Instant,
}

pub(crate) enum ServerCmd {
    Send(Value),
    Close(u16, String),
    Drop,
}

struct Conn {
    info: ConnectionInfo,
    tx: mpsc::UnboundedSender<ServerCmd>,
    drop_acks: bool,
}

struct Session {
    id: String,
    seq: u64,
    history: Vec<Value>,
    conn: Option<usize>,
    valid: bool,
}

pub(crate) struct HubState {
    pub heartbeat_interval: u64,
    pub ack_heartbeats: bool,
    pub drop_acks_next: usize,
    pub rotate_token: Option<String>,
    pub split: Option<usize>,
    pub reject_resume: bool,
    pub ready: Option<Value>,
    pub ready_supplemental: Option<Value>,
    conns: Vec<Conn>,
    sessions: Vec<Session>,
    frames: Vec<ClientFrame>,
    next_session: usize,
}

impl Default for HubState {
    fn default() -> Self {
        Self {
            heartbeat_interval: 41_250,
            ack_heartbeats: true,
            drop_acks_next: 0,
            rotate_token: None,
            split: None,
            reject_resume: false,
            ready: None,
            ready_supplemental: None,
            conns: vec![],
            sessions: vec![],
            frames: vec![],
            next_session: 0,
        }
    }
}

impl HubState {
    fn send(&self, conn: usize, payload: Value) {
        if let Some(c) = self.conns.get(conn).filter(|c| c.info.open) {
            let _ = c.tx.send(ServerCmd::Send(payload));
        }
    }

    fn push(&mut self, session: usize, t: &str, d: Value, deliver: bool) {
        let s = &mut self.sessions[session];
        s.seq += 1;
        let payload = json!({ "op": 0, "t": t, "s": s.seq, "d": d });
        s.history.push(payload.clone());
        if deliver && let Some(c) = s.conn {
            self.send(c, payload);
        }
    }
}

/// Emits a dispatch to every live session (`deliver = false` only records it in
/// the session history, as if it was lost in flight; RESUME replays it).
pub(crate) fn dispatch(shared: &Shared, t: &str, d: Value, deliver: bool) {
    {
        let mut g = shared.gateway.lock();
        for i in 0..g.sessions.len() {
            if g.sessions[i].valid {
                g.push(i, t, d.clone(), deliver);
            }
        }
    }
    shared.bump();
}

pub(crate) async fn ws_handler(State(shared): State<Arc<Shared>>, OriginalUri(uri): OriginalUri, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let query = uri.query().unwrap_or_default().to_string();
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let info = ConnectionInfo {
        id: 0,
        path: uri.path().to_string(),
        compressed: query.split('&').any(|p| p == "compress=zlib-stream"),
        query,
        open: true,
        user_agent: h("user-agent"),
        origin: h("origin"),
        session_id: None,
        identified: false,
        resumed: false,
        client_close_code: None,
        server_close_code: None,
    };
    ws.on_upgrade(move |socket| connection(shared, info, socket))
}

async fn connection(shared: Arc<Shared>, mut info: ConnectionInfo, socket: WebSocket) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let compressed = info.compressed;
    let id = {
        let mut g = shared.gateway.lock();
        let id = g.conns.len();
        info.id = id;
        let drop_acks = if g.drop_acks_next > 0 {
            g.drop_acks_next -= 1;
            true
        } else {
            !g.ack_heartbeats
        };
        let hello = json!({ "op": 10, "d": { "heartbeat_interval": g.heartbeat_interval, "_trace": ["[\"mock-gateway\",{\"micros\":0}]"] } });
        let _ = tx.send(ServerCmd::Send(hello));
        g.conns.push(Conn { info, tx: tx.clone(), drop_acks });
        id
    };
    shared.bump();

    let (mut sink, mut stream) = socket.split();
    let mut zlib = compressed.then(ZlibStreamEncoder::default);
    let mut client_close = None;
    let mut server_close = None;
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(ServerCmd::Send(v)) => {
                    let text = v.to_string();
                    let frames = match zlib.as_mut() {
                        Some(z) => {
                            let bytes = z.encode(text.as_bytes());
                            let split = shared.gateway.lock().split;
                            match split {
                                Some(n) => split_frames(&bytes, n),
                                None => vec![bytes],
                            }
                            .into_iter()
                            .map(|b| Message::Binary(b.into()))
                            .collect()
                        }
                        None => vec![Message::Text(text.into())],
                    };
                    let mut failed = false;
                    for f in frames {
                        if sink.send(f).await.is_err() {
                            failed = true;
                            break;
                        }
                    }
                    if failed {
                        break;
                    }
                }
                Some(ServerCmd::Close(code, reason)) => {
                    server_close = Some(code);
                    let _ = sink.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                    // Give the client a moment to read the close frame before the socket drops.
                    let _ = tokio::time::timeout(Duration::from_millis(500), async {
                        while let Some(Ok(m)) = stream.next().await {
                            if let Message::Close(f) = m {
                                client_close = f.map(|f| f.code);
                                break;
                            }
                        }
                    })
                    .await;
                    break;
                }
                Some(ServerCmd::Drop) | None => break,
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => on_client_frame(&shared, id, t.as_str()),
                Some(Ok(Message::Close(f))) => {
                    client_close = Some(f.map(|f| f.code).unwrap_or(1005));
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
        }
    }
    {
        let mut g = shared.gateway.lock();
        let c = &mut g.conns[id];
        c.info.open = false;
        c.info.client_close_code = client_close;
        c.info.server_close_code = server_close;
    }
    shared.bump();
}

fn on_client_frame(shared: &Shared, conn: usize, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    let op = v.get("op").and_then(Value::as_u64).unwrap_or(u64::MAX) as u8;
    let d = v.get("d").cloned().unwrap_or(Value::Null);
    shared.gateway.lock().frames.push(ClientFrame { conn, op, d: d.clone(), at: Instant::now() });
    match op {
        1 => {
            let g = shared.gateway.lock();
            if !g.conns[conn].drop_acks {
                g.send(conn, json!({ "op": 11 }));
            }
        }
        2 => identify(shared, conn, &d),
        6 => resume(shared, conn, &d),
        _ => {}
    }
    shared.bump();
}

fn token_valid(shared: &Shared, d: &Value) -> bool {
    let token = d.get("token").and_then(Value::as_str).unwrap_or_default();
    shared.world.lock().valid_tokens.contains(token)
}

fn close(g: &HubState, conn: usize, code: u16, reason: &str) {
    let _ = g.conns[conn].tx.send(ServerCmd::Close(code, reason.into()));
}

fn identify(shared: &Shared, conn: usize, d: &Value) {
    let valid = token_valid(shared, d);
    let rotate = shared.gateway.lock().rotate_token.clone();
    if valid && let Some(t) = &rotate {
        shared.world.lock().add_valid_token(t);
    }
    let mut g = shared.gateway.lock();
    if !valid {
        return close(&g, conn, 4004, "Authentication failed.");
    }
    if g.conns[conn].info.identified {
        return close(&g, conn, 4005, "Already authenticated.");
    }
    for s in &mut g.sessions {
        s.valid = false;
    }
    g.next_session += 1;
    let session_id = format!("mock-session-{}", g.next_session);
    let mut ready = g.ready.clone().unwrap_or_else(|| crate::fixtures::ReadyBuilder::standard().build());
    ready["session_id"] = json!(session_id);
    ready["resume_gateway_url"] = json!(format!("{}/resume", shared.ws_base));
    if let Some(t) = rotate {
        ready["auth_token"] = json!(t);
    }
    let supplemental = g.ready_supplemental.clone();
    g.sessions.push(Session { id: session_id.clone(), seq: 0, history: vec![], conn: Some(conn), valid: true });
    let idx = g.sessions.len() - 1;
    let info = &mut g.conns[conn].info;
    info.identified = true;
    info.session_id = Some(session_id);
    g.push(idx, "READY", ready, true);
    if let Some(s) = supplemental {
        g.push(idx, "READY_SUPPLEMENTAL", s, true);
    }
}

fn resume(shared: &Shared, conn: usize, d: &Value) {
    let valid = token_valid(shared, d);
    let mut g = shared.gateway.lock();
    if !valid {
        return close(&g, conn, 4004, "Authentication failed.");
    }
    let sid = d.get("session_id").and_then(Value::as_str).unwrap_or_default();
    let seq = d.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let found = g.sessions.iter().position(|s| s.id == sid && s.valid);
    let (Some(idx), false) = (found, g.reject_resume) else {
        g.send(conn, json!({ "op": 9, "d": false }));
        return;
    };
    g.sessions[idx].conn = Some(conn);
    let replay: Vec<Value> = g.sessions[idx].history.iter().filter(|p| p["s"].as_u64().unwrap_or(0) > seq).cloned().collect();
    for p in replay {
        g.send(conn, p);
    }
    let info = &mut g.conns[conn].info;
    info.resumed = true;
    info.session_id = Some(sid.to_string());
    g.push(idx, "RESUMED", json!({ "_trace": ["mock-gateway"] }), true);
}

/// Gateway controls, from [`crate::MockDiscord::gateway`].
pub struct GatewayControl<'a> {
    pub(crate) shared: &'a Shared,
}

impl GatewayControl<'_> {
    /// `heartbeat_interval` sent in HELLO for new connections (ms).
    pub fn set_heartbeat_interval(&self, ms: u64) {
        self.shared.gateway.lock().heartbeat_interval = ms;
    }

    /// Whether heartbeats are ACKed (for connections opened from now on).
    pub fn set_ack_heartbeats(&self, ack: bool) {
        self.shared.gateway.lock().ack_heartbeats = ack;
    }

    /// The next `n` connections never ACK heartbeats (zombie connections).
    pub fn drop_acks_on_next_connections(&self, n: usize) {
        self.shared.gateway.lock().drop_acks_next = n;
    }

    /// READY carries `auth_token` (AUTH_TOKEN_REFRESH); the new token becomes valid.
    pub fn rotate_token(&self, token: Option<&str>) {
        self.shared.gateway.lock().rotate_token = token.map(str::to_string);
    }

    /// Splits every compressed message into websocket frames of about `n` bytes.
    pub fn split_frames(&self, n: Option<usize>) {
        self.shared.gateway.lock().split = n;
    }

    /// Answer every RESUME with INVALID_SESSION(false).
    pub fn reject_resumes(&self, reject: bool) {
        self.shared.gateway.lock().reject_resume = reject;
    }

    /// Sends a dispatch to the live session.
    pub fn dispatch(&self, t: &str, d: Value) {
        dispatch(self.shared, t, d, true);
    }

    /// Records a dispatch in the session history without delivering it, as if it
    /// was lost when the connection died. A later RESUME replays it.
    pub fn dispatch_buffered(&self, t: &str, d: Value) {
        dispatch(self.shared, t, d, false);
    }

    /// Sends a raw opcode to every open connection (e.g. 7 RECONNECT, 9 INVALID_SESSION, 1 HEARTBEAT).
    pub fn send_op(&self, op: u8, d: Value) {
        let g = self.shared.gateway.lock();
        for c in g.conns.iter().filter(|c| c.info.open) {
            let _ = c.tx.send(ServerCmd::Send(json!({ "op": op, "d": d })));
        }
    }

    /// Closes every open connection with a close code.
    pub fn close(&self, code: u16, reason: &str) {
        let g = self.shared.gateway.lock();
        for c in g.conns.iter().filter(|c| c.info.open) {
            let _ = c.tx.send(ServerCmd::Close(code, reason.into()));
        }
    }

    /// Drops every open connection without a close frame.
    pub fn drop_connections(&self) {
        let g = self.shared.gateway.lock();
        for c in g.conns.iter().filter(|c| c.info.open) {
            let _ = c.tx.send(ServerCmd::Drop);
        }
    }

    pub fn connections(&self) -> Vec<ConnectionInfo> {
        self.shared.gateway.lock().conns.iter().map(|c| c.info.clone()).collect()
    }

    /// Every payload received from clients.
    pub fn frames(&self) -> Vec<ClientFrame> {
        self.shared.gateway.lock().frames.clone()
    }

    pub fn frames_with_op(&self, op: u8) -> Vec<ClientFrame> {
        self.shared.gateway.lock().frames.iter().filter(|f| f.op == op).cloned().collect()
    }

    /// Current sequence number of the live session.
    pub fn seq(&self) -> u64 {
        self.shared.gateway.lock().sessions.iter().rev().find(|s| s.valid).map(|s| s.seq).unwrap_or(0)
    }

    /// Waits for the first frame with this opcode.
    pub async fn wait_for_op(&self, op: u8) -> ClientFrame {
        self.wait_for_nth_op(op, 1).await
    }

    /// Waits until `n` frames with this opcode were received and returns the `n`th.
    pub async fn wait_for_nth_op(&self, op: u8, n: usize) -> ClientFrame {
        self.wait_for_frame(&format!("{n}x op {op}"), |frames| frames.iter().filter(|f| f.op == op).nth(n - 1).cloned()).await
    }

    /// Waits for a frame selected by `pick` over all frames received so far.
    pub async fn wait_for_frame(&self, what: &str, pick: impl Fn(&[ClientFrame]) -> Option<ClientFrame>) -> ClientFrame {
        match self.shared.wait_for(DEFAULT_WAIT, || pick(&self.shared.gateway.lock().frames)).await {
            Some(f) => f,
            None => {
                let ops: Vec<u8> = self.frames().iter().map(|f| f.op).collect();
                panic!("timed out waiting for gateway frame: {what}; received ops {ops:?}")
            }
        }
    }

    /// Waits until at least `n` connections were opened.
    pub async fn wait_for_connections(&self, n: usize) -> Vec<ConnectionInfo> {
        self.wait_for_connection(&format!("{n} connections"), |c| (c.len() >= n).then(|| c.to_vec())).await
    }

    /// Waits for a condition over the connection list.
    pub async fn wait_for_connection<T>(&self, what: &str, pick: impl Fn(&[ConnectionInfo]) -> Option<T>) -> T {
        match self.shared.wait_for(DEFAULT_WAIT, || pick(&self.connections())).await {
            Some(v) => v,
            None => panic!("timed out waiting for {what}; connections: {:#?}", self.connections()),
        }
    }
}
