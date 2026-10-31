//! Discord gateway client.
//!
//! [`Gateway::spawn`] starts a background task that owns the websocket. It
//! identifies (or resumes), heartbeats, transparently reconnects with backoff and
//! reports everything through a channel of [`GatewayMessage`]s. Commands such as
//! presence updates or lazy-guild subscriptions are sent via the cloneable
//! [`GatewayHandle`].

mod inflate;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrocord_http::ClientProfile;
use ferrocord_model::Event;
use ferrocord_model::Snowflake;
use ferrocord_model::gateway::{
    ClientState, GatewayPayload, GuildSubscription, Hello, Identify, RequestGuildMembers, Resume, UpdatePresence, UpdateVoiceState, capabilities, opcode,
};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

pub use inflate::ZlibStream;

pub const DEFAULT_GATEWAY: &str = "wss://gateway.discord.gg";

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub token: String,
    pub profile: ClientProfile,
    pub capabilities: u64,
    pub presence: UpdatePresence,
    pub url: String,
}

impl GatewayConfig {
    pub fn new(token: String, profile: ClientProfile) -> Self {
        Self { token, profile, capabilities: capabilities::OFFICIAL_DESKTOP, presence: UpdatePresence::default(), url: DEFAULT_GATEWAY.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Identifying,
    Resuming,
    Connected,
    /// Waiting before reconnect attempt `n`.
    Reconnecting(u32),
    Disconnected,
}

#[derive(Debug, thiserror::Error, Clone)]
pub enum GatewayError {
    #[error("authentication failed (invalid or revoked token)")]
    AuthenticationFailed,
    #[error("gateway closed with fatal code {0}: {1}")]
    Fatal(u16, String),
}

#[derive(Debug, Clone)]
pub enum GatewayMessage {
    Dispatch {
        seq: u64,
        event: Box<Event>,
        raw: Arc<Value>,
    },
    State(ConnectionState),
    /// Round-trip time of the last heartbeat.
    Latency(Duration),
    /// The session died for good (bad token, etc.). The task has exited.
    Fatal(GatewayError),
}

#[derive(Debug)]
enum Command {
    Send(GatewayPayload),
    Reconnect,
    Shutdown,
}

#[derive(Clone)]
pub struct GatewayHandle {
    tx: mpsc::UnboundedSender<Command>,
}

impl GatewayHandle {
    pub fn send_raw(&self, op: u8, d: Value) {
        let _ = self.tx.send(Command::Send(GatewayPayload { op, d, s: None, t: None }));
    }

    pub fn update_presence(&self, p: &UpdatePresence) {
        self.send_raw(opcode::PRESENCE_UPDATE, serde_json::to_value(p).unwrap_or_default());
    }

    pub fn update_voice_state(&self, v: &UpdateVoiceState) {
        self.send_raw(opcode::VOICE_STATE_UPDATE, serde_json::to_value(v).unwrap_or_default());
    }

    pub fn request_guild_members(&self, r: &RequestGuildMembers) {
        self.send_raw(opcode::REQUEST_GUILD_MEMBERS, serde_json::to_value(r).unwrap_or_default());
    }

    /// Lazy guild subscriptions (op 37). The official client batches these at ~15 KiB per frame.
    pub fn subscribe_guilds(&self, subs: impl IntoIterator<Item = (Snowflake, GuildSubscription)>) {
        let mut batch = serde_json::Map::new();
        let mut size = 0usize;
        for (gid, sub) in subs {
            let v = serde_json::to_value(sub).unwrap_or_default();
            let len = gid.to_string().len() + v.to_string().len() + 8;
            if size + len > 15_360 && !batch.is_empty() {
                let subs = std::mem::take(&mut batch);
                self.send_raw(opcode::GUILD_SUBSCRIPTIONS_BULK, serde_json::json!({ "subscriptions": subs }));
                size = 0;
            }
            batch.insert(gid.to_string(), v);
            size += len;
        }
        if !batch.is_empty() {
            self.send_raw(opcode::GUILD_SUBSCRIPTIONS_BULK, serde_json::json!({ "subscriptions": batch }));
        }
    }

    pub fn request_channel_info(&self, guild: Snowflake, fields: &[&str]) {
        self.send_raw(opcode::GUILD_CHANNELS_RESYNC, serde_json::json!({ "guild_id": guild, "fields": fields }));
    }

    pub fn search_recent_members(&self, guild: Snowflake, query: &str) {
        self.send_raw(opcode::SEARCH_RECENT_MEMBERS, serde_json::json!({ "guild_id": guild, "query": query, "continuation_token": null }));
    }

    pub fn reconnect(&self) {
        let _ = self.tx.send(Command::Reconnect);
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

pub struct Gateway;

impl Gateway {
    /// Starts the gateway task. Must be called inside a Tokio runtime.
    pub fn spawn(config: GatewayConfig) -> (GatewayHandle, mpsc::UnboundedReceiver<GatewayMessage>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        tokio::spawn(run(config, cmd_rx, out_tx));
        (GatewayHandle { tx: cmd_tx }, out_rx)
    }
}

#[derive(Default)]
struct Session {
    id: Option<String>,
    seq: Option<u64>,
    resume_url: Option<String>,
}

enum Outcome {
    /// Reconnect; `resume` says whether the session can be resumed.
    Reconnect {
        resume: bool,
        delay: bool,
    },
    Fatal(GatewayError),
    Shutdown,
}

async fn run(mut config: GatewayConfig, mut cmd_rx: mpsc::UnboundedReceiver<Command>, out: mpsc::UnboundedSender<GatewayMessage>) {
    let mut session = Session::default();
    let mut failures = 0u32;
    loop {
        let resuming = session.id.is_some() && session.seq.is_some();
        let _ = out.send(GatewayMessage::State(ConnectionState::Connecting));
        let base = if resuming { session.resume_url.clone().unwrap_or(config.url.clone()) } else { config.url.clone() };
        let started = Instant::now();
        let outcome = connect_once(&base, &mut config, &mut session, &mut cmd_rx, &out).await;
        if started.elapsed() > Duration::from_secs(60) {
            failures = 0;
        }
        match outcome {
            Outcome::Shutdown => {
                let _ = out.send(GatewayMessage::State(ConnectionState::Disconnected));
                return;
            }
            Outcome::Fatal(e) => {
                let _ = out.send(GatewayMessage::Fatal(e));
                let _ = out.send(GatewayMessage::State(ConnectionState::Disconnected));
                return;
            }
            Outcome::Reconnect { resume, delay } => {
                if !resume {
                    session = Session::default();
                }
                failures += 1;
                if delay || failures > 1 {
                    // Exponential backoff with jitter, capped at ~60s.
                    let base_ms = 1000u64.saturating_mul(1 << failures.min(6)).min(60_000);
                    let jitter = rand::thread_rng().gen_range(0..=base_ms / 2);
                    let _ = out.send(GatewayMessage::State(ConnectionState::Reconnecting(failures)));
                    let wait = Duration::from_millis(base_ms / 2 + jitter);
                    // Allow shutdown during backoff.
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        cmd = cmd_rx.recv() => {
                            if matches!(cmd, None | Some(Command::Shutdown)) {
                                let _ = out.send(GatewayMessage::State(ConnectionState::Disconnected));
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn connect_once(
    base: &str,
    config: &mut GatewayConfig,
    session: &mut Session,
    cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
    out: &mpsc::UnboundedSender<GatewayMessage>,
) -> Outcome {
    let url = format!("{}/?encoding=json&v={}&compress=zlib-stream", base.trim_end_matches('/'), ferrocord_model::API_VERSION);
    tracing::info!(%url, "connecting to gateway");
    let req = {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut r = match url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => return Outcome::Fatal(GatewayError::Fatal(0, e.to_string())),
        };
        if let Ok(v) = config.profile.user_agent().parse() {
            r.headers_mut().insert("user-agent", v);
        }
        if let Ok(v) = "https://discord.com".parse() {
            r.headers_mut().insert("origin", v);
        }
        r
    };
    let ws = match tokio::time::timeout(Duration::from_secs(20), tokio_tungstenite::connect_async(req)).await {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "gateway connect failed");
            return Outcome::Reconnect { resume: true, delay: true };
        }
        Err(_) => {
            tracing::warn!("gateway connect timed out");
            return Outcome::Reconnect { resume: true, delay: true };
        }
    };
    let (mut sink, mut stream) = ws.split();
    let mut inflate = ZlibStream::default();

    let mut heartbeat: Option<tokio::time::Interval> = None;
    let mut awaiting_ack = false;
    let mut last_beat = Instant::now();
    let mut ready = false;

    macro_rules! send {
        ($payload:expr) => {{
            let text = serde_json::to_string(&$payload).unwrap_or_default();
            if let Err(e) = sink.send(WsMessage::Text(text.into())).await {
                tracing::warn!(error = %e, "gateway send failed");
                return Outcome::Reconnect { resume: true, delay: true };
            }
        }};
    }

    loop {
        let hb_tick = async {
            match heartbeat.as_mut() {
                Some(i) => {
                    i.tick().await;
                }
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            _ = hb_tick => {
                if awaiting_ack {
                    tracing::warn!("missed heartbeat ACK; reconnecting (zombied connection)");
                    let _ = sink.send(WsMessage::Close(Some(CloseFrame { code: CloseCode::Library(4000), reason: "zombie".into() }))).await;
                    return Outcome::Reconnect { resume: true, delay: false };
                }
                awaiting_ack = true;
                last_beat = Instant::now();
                send!(GatewayPayload { op: opcode::HEARTBEAT, d: session.seq.map(Value::from).unwrap_or(Value::Null), s: None, t: None });
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    None | Some(Command::Shutdown) => {
                        let _ = sink.send(WsMessage::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "".into() }))).await;
                        return Outcome::Shutdown;
                    }
                    Some(Command::Reconnect) => {
                        let _ = sink.send(WsMessage::Close(Some(CloseFrame { code: CloseCode::Library(4000), reason: "reconnect".into() }))).await;
                        return Outcome::Reconnect { resume: true, delay: false };
                    }
                    Some(Command::Send(p)) => {
                        if ready {
                            if p.op == opcode::PRESENCE_UPDATE
                                && let Ok(pres) = serde_json::from_value::<UpdatePresence>(p.d.clone())
                            {
                                config.presence = pres;
                            }
                            send!(p);
                        } else {
                            tracing::debug!(op = p.op, "dropping command sent before READY");
                        }
                    }
                }
            }
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return Outcome::Reconnect { resume: true, delay: true };
                };
                let frame = match frame {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(error = %e, "gateway read error");
                        return Outcome::Reconnect { resume: true, delay: true };
                    }
                };
                let payload: GatewayPayload = match frame {
                    WsMessage::Binary(b) => match inflate.push(&b) {
                        Ok(Some(bytes)) => match serde_json::from_slice(bytes) {
                            Ok(p) => p,
                            Err(e) => { tracing::error!(error = %e, "bad gateway JSON"); continue; }
                        },
                        Ok(None) => continue,
                        Err(e) => {
                            tracing::error!(error = %e, "zlib error; reconnecting");
                            return Outcome::Reconnect { resume: true, delay: false };
                        }
                    },
                    WsMessage::Text(t) => match serde_json::from_str(&t) {
                        Ok(p) => p,
                        Err(_) => continue,
                    },
                    WsMessage::Close(frame) => {
                        let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(1000);
                        let reason = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                        tracing::warn!(code, %reason, "gateway closed");
                        return close_outcome(code, reason);
                    }
                    _ => continue,
                };

                match payload.op {
                    opcode::HELLO => {
                        let hello: Hello = serde_json::from_value(payload.d).unwrap_or(Hello { heartbeat_interval: 41_250 });
                        let interval = Duration::from_millis(hello.heartbeat_interval);
                        let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..hello.heartbeat_interval.max(1)));
                        heartbeat = Some(tokio::time::interval_at(tokio::time::Instant::now() + jitter, interval));
                        if let (Some(id), Some(seq)) = (session.id.clone(), session.seq) {
                            let _ = out.send(GatewayMessage::State(ConnectionState::Resuming));
                            send!(GatewayPayload {
                                op: opcode::RESUME,
                                d: serde_json::to_value(Resume { token: config.token.clone(), session_id: id, seq }).unwrap_or_default(),
                                s: None, t: None,
                            });
                        } else {
                            let _ = out.send(GatewayMessage::State(ConnectionState::Identifying));
                            send!(GatewayPayload { op: opcode::IDENTIFY, d: identify_payload(config), s: None, t: None });
                        }
                    }
                    opcode::HEARTBEAT => {
                        send!(GatewayPayload { op: opcode::HEARTBEAT, d: session.seq.map(Value::from).unwrap_or(Value::Null), s: None, t: None });
                    }
                    opcode::HEARTBEAT_ACK => {
                        awaiting_ack = false;
                        let _ = out.send(GatewayMessage::Latency(last_beat.elapsed()));
                    }
                    opcode::RECONNECT => {
                        tracing::info!("server requested reconnect");
                        return Outcome::Reconnect { resume: true, delay: false };
                    }
                    opcode::INVALID_SESSION => {
                        let resumable = payload.d.as_bool().unwrap_or(false);
                        tracing::warn!(resumable, "invalid session");
                        // Discord asks clients to wait 1-5s before re-identifying.
                        let wait = Duration::from_millis(rand::thread_rng().gen_range(1000..5000));
                        tokio::time::sleep(wait).await;
                        return Outcome::Reconnect { resume: resumable, delay: false };
                    }
                    opcode::DISPATCH => {
                        if let Some(s) = payload.s {
                            session.seq = Some(s);
                        }
                        let name = payload.t.unwrap_or_default();
                        let (event, err) = Event::parse(&name, &payload.d);
                        if let Some(e) = err {
                            tracing::warn!(event = %name, error = %e, "failed to parse dispatch; forwarding raw");
                        }
                        match &event {
                            Event::Ready(r) => {
                                session.id = Some(r.session_id.clone());
                                session.resume_url = r.resume_gateway_url.clone();
                                if let Some(t) = &r.auth_token {
                                    config.token = t.clone();
                                }
                                ready = true;
                                let _ = out.send(GatewayMessage::State(ConnectionState::Connected));
                            }
                            Event::Resumed => {
                                ready = true;
                                let _ = out.send(GatewayMessage::State(ConnectionState::Connected));
                            }
                            _ => {}
                        }
                        let raw = Arc::new(payload.d);
                        if out.send(GatewayMessage::Dispatch { seq: payload.s.unwrap_or(0), event: Box::new(event), raw }).is_err() {
                            return Outcome::Shutdown;
                        }
                    }
                    other => tracing::debug!(op = other, "unhandled gateway opcode"),
                }
            }
        }
    }
}

fn identify_payload(config: &GatewayConfig) -> Value {
    let props = serde_json::to_value(config.profile.super_properties()).unwrap_or_default();
    serde_json::to_value(Identify {
        token: config.token.clone(),
        capabilities: config.capabilities,
        properties: props,
        presence: config.presence.clone(),
        compress: false,
        client_state: ClientState::default(),
    })
    .unwrap_or_default()
}

/// Maps a close code to the reconnect policy documented by Discord.
fn close_outcome(code: u16, reason: String) -> Outcome {
    match code {
        4004 => Outcome::Fatal(GatewayError::AuthenticationFailed),
        4010..=4014 => Outcome::Fatal(GatewayError::Fatal(code, reason)),
        // Invalid seq / session timed out: start a fresh session.
        4007 | 4009 => Outcome::Reconnect { resume: false, delay: false },
        // Clean close from the server ends the session as well.
        1000 | 1001 => Outcome::Reconnect { resume: false, delay: true },
        _ => Outcome::Reconnect { resume: true, delay: true },
    }
}
