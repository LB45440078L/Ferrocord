//! QR-code login ("remote auth", protocol v2), reverse engineered from the web client.
//!
//! ```text
//! client                                   remote-auth-gateway
//!   | -- ws connect (Origin: discord.com) -->|
//!   | -- init {encoded_public_key: SPKI} --> |
//!   | <-- hello {heartbeat_interval, timeout_ms}
//!   | <-- nonce_proof {encrypted_nonce}      |   RSA-OAEP(SHA-256)
//!   | -- nonce_proof {nonce: b64url(plain)}->|
//!   | <-- pending_remote_init {fingerprint}  |   QR = https://discord.com/ra/{fingerprint}
//!   | <-- pending_ticket {encrypted_user_payload}   "id:discrim:avatar:username"
//!   | <-- pending_login {ticket}             |
//!   | POST /users/@me/remote-auth/login {ticket} -> {encrypted_token}
//! ```
//! `fingerprint` = base64url(SHA-256(SPKI DER)), unpadded.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ferrocord_http::{Http, Request};
use futures_util::{SinkExt, StreamExt};
use rsa::pkcs8::EncodePublicKey;
use rsa::{Oaep, RsaPrivateKey, RsaPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

pub const REMOTE_AUTH_GATEWAY: &str = "wss://remote-auth-gateway.discord.gg/?v=2";

/// The account that scanned the QR code (shown while awaiting confirmation on the phone).
#[derive(Debug, Clone)]
pub struct ScannedUser {
    pub id: String,
    pub discriminator: String,
    pub avatar: Option<String>,
    pub username: String,
}

impl ScannedUser {
    pub fn avatar_url(&self) -> Option<String> {
        let id = self.id.parse().ok()?;
        Some(match &self.avatar {
            Some(h) => ferrocord_model::cdn::user_avatar(id, h, 128),
            None => ferrocord_model::cdn::default_avatar(id, &self.discriminator),
        })
    }
}

#[derive(Debug, Clone)]
pub enum RemoteAuthEvent {
    /// Show this URL as a QR code.
    QrCode(String),
    /// A phone scanned the code; waiting for the user to confirm there.
    Scanned(ScannedUser),
    /// Login confirmed.
    Token(String),
    /// User cancelled on the phone; a new code will follow.
    Cancelled,
    /// Connection problem; the task retries automatically.
    Error(String),
}

/// Starts the QR flow. The task keeps regenerating codes until a token is
/// obtained or the receiver is dropped.
pub fn start(http: Http) -> mpsc::Receiver<RemoteAuthEvent> {
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(async move {
        let mut backoff = 1u64;
        loop {
            match session(&http, &tx).await {
                Ok(true) => return,
                Ok(false) => backoff = 1,
                Err(e) => {
                    tracing::warn!(error = %e, "remote auth session failed");
                    if tx.send(RemoteAuthEvent::Error(e)).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(30);
                }
            }
            if tx.is_closed() {
                return;
            }
        }
    });
    rx
}

fn decrypt(key: &RsaPrivateKey, b64: &str) -> Result<Vec<u8>, String> {
    let ct = STANDARD.decode(b64).map_err(|e| e.to_string())?;
    key.decrypt(Oaep::new::<Sha256>(), &ct).map_err(|e| e.to_string())
}

/// Runs one remote-auth websocket session. `Ok(true)` = token delivered,
/// `Ok(false)` = session ended normally (timeout/cancel) and should restart.
async fn session(http: &Http, tx: &mpsc::Sender<RemoteAuthEvent>) -> Result<bool, String> {
    let key = tokio::task::spawn_blocking(|| RsaPrivateKey::new(&mut rand::thread_rng(), 2048))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let spki = RsaPublicKey::from(&key).to_public_key_der().map_err(|e| e.to_string())?;
    let encoded_public_key = STANDARD.encode(spki.as_bytes());
    let fingerprint = URL_SAFE_NO_PAD.encode(Sha256::digest(spki.as_bytes()));

    let req = {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut r = REMOTE_AUTH_GATEWAY.into_client_request().map_err(|e| e.to_string())?;
        r.headers_mut().insert("origin", "https://discord.com".parse().expect("static header"));
        if let Ok(ua) = http.profile().user_agent().parse() {
            r.headers_mut().insert("user-agent", ua);
        }
        r
    };
    let (ws, _) = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| "timed out connecting to remote auth gateway".to_string())?
        .map_err(|e| e.to_string())?;
    let (mut sink, mut stream) = ws.split();
    sink.send(WsMessage::Text(json!({ "op": "init", "encoded_public_key": encoded_public_key }).to_string().into()))
        .await
        .map_err(|e| e.to_string())?;

    let mut heartbeat: Option<tokio::time::Interval> = None;
    let mut acked = true;

    loop {
        let tick = async {
            match heartbeat.as_mut() {
                Some(i) => {
                    i.tick().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tick => {
                if !acked {
                    return Err("remote auth heartbeat timed out".into());
                }
                acked = false;
                sink.send(WsMessage::Text(json!({ "op": "heartbeat" }).to_string().into())).await.map_err(|e| e.to_string())?;
            }
            _ = tx.closed() => {
                let _ = sink.send(WsMessage::Close(None)).await;
                return Ok(true);
            }
            msg = stream.next() => {
                let msg = match msg {
                    Some(Ok(WsMessage::Text(t))) => t,
                    Some(Ok(WsMessage::Close(f))) => {
                        // 4003 = handshake timed out (the QR expired); just restart.
                        tracing::info!(?f, "remote auth gateway closed");
                        return Ok(false);
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e.to_string()),
                    None => return Ok(false),
                };
                let v: Value = serde_json::from_str(&msg).map_err(|e| e.to_string())?;
                let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
                match v.get("op").and_then(Value::as_str).unwrap_or_default() {
                    "hello" => {
                        let ms = v.get("heartbeat_interval").and_then(Value::as_u64).unwrap_or(41_250);
                        let first = tokio::time::Instant::now() + Duration::from_millis(rand::random::<u64>() % ms.max(1));
                        heartbeat = Some(tokio::time::interval_at(first, Duration::from_millis(ms)));
                    }
                    "heartbeat_ack" => acked = true,
                    "nonce_proof" => {
                        let nonce = decrypt(&key, &s("encrypted_nonce"))?;
                        let proof = URL_SAFE_NO_PAD.encode(nonce);
                        sink.send(WsMessage::Text(json!({ "op": "nonce_proof", "nonce": proof }).to_string().into()))
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                    "pending_remote_init" => {
                        let fp = s("fingerprint");
                        if fp != fingerprint {
                            return Err(format!("fingerprint mismatch ({fp} != {fingerprint})"));
                        }
                        let _ = tx.send(RemoteAuthEvent::QrCode(format!("https://discord.com/ra/{fp}"))).await;
                    }
                    "pending_ticket" => {
                        let plain = String::from_utf8(decrypt(&key, &s("encrypted_user_payload"))?).map_err(|e| e.to_string())?;
                        let mut parts = plain.splitn(4, ':');
                        let user = ScannedUser {
                            id: parts.next().unwrap_or_default().into(),
                            discriminator: parts.next().unwrap_or_default().into(),
                            avatar: parts.next().filter(|a| *a != "0").map(str::to_string),
                            username: parts.next().unwrap_or_default().into(),
                        };
                        let _ = tx.send(RemoteAuthEvent::Scanned(user)).await;
                    }
                    "pending_login" => {
                        let ticket = s("ticket");
                        let resp = http
                            .send(Request::post("/users/@me/remote-auth/login").unauthenticated().json(json!({ "ticket": ticket })))
                            .await
                            .map_err(|e| e.to_string())?;
                        let enc = resp.get("encrypted_token").and_then(Value::as_str).ok_or("missing encrypted_token")?;
                        let token = String::from_utf8(decrypt(&key, enc)?).map_err(|e| e.to_string())?;
                        let _ = sink.send(WsMessage::Close(None)).await;
                        let _ = tx.send(RemoteAuthEvent::Token(token)).await;
                        return Ok(true);
                    }
                    "cancel" => {
                        let _ = tx.send(RemoteAuthEvent::Cancelled).await;
                        return Ok(false);
                    }
                    other => tracing::debug!(op = other, "unknown remote auth op"),
                }
            }
        }
    }
}
