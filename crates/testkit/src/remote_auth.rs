//! Mock remote-auth (QR login) gateway, protocol v2 (see `docs/PROTOCOL.md`).
//!
//! The server encrypts a random nonce with the client's SPKI key (RSA-OAEP,
//! SHA-256), checks the returned `nonce_proof`, announces the fingerprint, then
//! plays the configured [`RemoteAuthBehaviour`].

use std::collections::VecDeque;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use rsa::pkcs8::DecodePublicKey;
use rsa::{Oaep, RsaPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::fixtures::UserBuilder;
use crate::{DEFAULT_WAIT, Shared};

/// What the "phone" does on a remote-auth connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAuthBehaviour {
    /// Scan, then confirm: `pending_ticket` → `pending_login`.
    Login,
    /// Scan, then decline on the phone: `pending_ticket` → `cancel`.
    CancelAfterScan,
    /// Never scanned: the code expires and the socket closes with 4003.
    ExpireAfterInit,
    /// Announce a fingerprint that does not match the client's key.
    WrongFingerprint,
    /// Stop after `pending_remote_init` and wait.
    Idle,
}

/// What happened on one remote-auth connection.
#[derive(Debug, Clone)]
pub struct RemoteAuthSessionInfo {
    pub behaviour: RemoteAuthBehaviour,
    pub origin: Option<String>,
    /// SPKI DER from `init.encoded_public_key`.
    pub public_key_der: Option<Vec<u8>>,
    /// `base64url(sha256(spki))`, as the client should display it.
    pub fingerprint: Option<String>,
    /// The client returned the decrypted nonce correctly.
    pub nonce_verified: bool,
    /// `op` of every client message, in order.
    pub client_ops: Vec<String>,
    /// Ticket handed out in `pending_login`.
    pub ticket: Option<String>,
}

pub(crate) struct RemoteState {
    pub heartbeat_interval: u64,
    pub timeout_ms: u64,
    pub user: UserBuilder,
    /// Token delivered (encrypted) by `/users/@me/remote-auth/login`. `None` = the account token.
    pub token: Option<String>,
    pub behaviours: VecDeque<RemoteAuthBehaviour>,
    pub sessions: Vec<RemoteAuthSessionInfo>,
    next_ticket: u64,
}

impl Default for RemoteState {
    fn default() -> Self {
        Self {
            heartbeat_interval: 41_250,
            timeout_ms: 120_000,
            user: UserBuilder::me(),
            token: None,
            behaviours: VecDeque::new(),
            sessions: vec![],
            next_ticket: 0,
        }
    }
}

pub(crate) async fn ws_handler(State(shared): State<Arc<Shared>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let origin = headers.get("origin").and_then(|v| v.to_str().ok()).map(str::to_string);
    ws.on_upgrade(move |socket| connection(shared, origin, socket))
}

fn encrypt(key: &RsaPublicKey, data: &[u8]) -> String {
    STANDARD.encode(key.encrypt(&mut rand::thread_rng(), Oaep::new::<Sha256>(), data).expect("rsa encrypt"))
}

async fn connection(shared: Arc<Shared>, origin: Option<String>, socket: WebSocket) {
    let (idx, behaviour, hello) = {
        let mut r = shared.remote.lock();
        let behaviour = r.behaviours.pop_front().unwrap_or(RemoteAuthBehaviour::Login);
        r.sessions.push(RemoteAuthSessionInfo {
            behaviour,
            origin,
            public_key_der: None,
            fingerprint: None,
            nonce_verified: false,
            client_ops: vec![],
            ticket: None,
        });
        let hello = json!({ "op": "hello", "heartbeat_interval": r.heartbeat_interval, "timeout_ms": r.timeout_ms });
        (r.sessions.len() - 1, behaviour, hello)
    };
    shared.bump();
    let (mut sink, mut stream) = socket.split();
    let send = |v: Value| Message::Text(v.to_string().into());
    if sink.send(send(hello)).await.is_err() {
        return;
    }
    let mut key: Option<RsaPublicKey> = None;
    let mut nonce: Vec<u8> = vec![];

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else { continue };
        let op = v.get("op").and_then(Value::as_str).unwrap_or_default().to_string();
        shared.remote.lock().sessions[idx].client_ops.push(op.clone());
        shared.bump();
        let mut out: Vec<Message> = vec![];
        let mut close: Option<(u16, &str)> = None;
        match op.as_str() {
            "heartbeat" => out.push(send(json!({ "op": "heartbeat_ack" }))),
            "init" => {
                let der = v.get("encoded_public_key").and_then(Value::as_str).and_then(|k| STANDARD.decode(k).ok()).unwrap_or_default();
                match RsaPublicKey::from_public_key_der(&der) {
                    Ok(k) => {
                        let fp = URL_SAFE_NO_PAD.encode(Sha256::digest(&der));
                        {
                            let mut r = shared.remote.lock();
                            r.sessions[idx].public_key_der = Some(der);
                            r.sessions[idx].fingerprint = Some(fp);
                        }
                        nonce = rand::random::<[u8; 32]>().to_vec();
                        out.push(send(json!({ "op": "nonce_proof", "encrypted_nonce": encrypt(&k, &nonce) })));
                        key = Some(k);
                    }
                    Err(_) => close = Some((4001, "Invalid public key")),
                }
            }
            "nonce_proof" => {
                let ok = v.get("nonce").and_then(Value::as_str) == Some(URL_SAFE_NO_PAD.encode(&nonce).as_str()) && !nonce.is_empty();
                let Some(k) = key.as_ref().filter(|_| ok) else {
                    close = Some((4003, "Invalid nonce proof"));
                    let _ = flush(&mut sink, out, close).await;
                    break;
                };
                let (fingerprint, user, token) = {
                    let mut r = shared.remote.lock();
                    r.sessions[idx].nonce_verified = true;
                    let fp = r.sessions[idx].fingerprint.clone().unwrap_or_default();
                    (fp, r.user.clone(), r.token.clone())
                };
                let announced = if behaviour == RemoteAuthBehaviour::WrongFingerprint { URL_SAFE_NO_PAD.encode([0u8; 32]) } else { fingerprint };
                out.push(send(json!({ "op": "pending_remote_init", "fingerprint": announced })));
                let payload = format!("{}:{}:{}:{}", user.id, user.discriminator, user.avatar.clone().unwrap_or_else(|| "0".into()), user.username);
                match behaviour {
                    RemoteAuthBehaviour::Login => {
                        out.push(send(json!({ "op": "pending_ticket", "encrypted_user_payload": encrypt(k, payload.as_bytes()) })));
                        let ticket = {
                            let mut r = shared.remote.lock();
                            r.next_ticket += 1;
                            let t = format!("ra-ticket-{}", r.next_ticket);
                            r.sessions[idx].ticket = Some(t.clone());
                            t
                        };
                        {
                            let mut w = shared.world.lock();
                            let token = token.unwrap_or_else(|| w.account.token.clone());
                            w.remote_tickets.insert(ticket.clone(), (k.clone(), token));
                        }
                        out.push(send(json!({ "op": "pending_login", "ticket": ticket })));
                    }
                    RemoteAuthBehaviour::CancelAfterScan => {
                        out.push(send(json!({ "op": "pending_ticket", "encrypted_user_payload": encrypt(k, payload.as_bytes()) })));
                        out.push(send(json!({ "op": "cancel" })));
                    }
                    RemoteAuthBehaviour::ExpireAfterInit => close = Some((4003, "Handshake Timeout")),
                    RemoteAuthBehaviour::WrongFingerprint | RemoteAuthBehaviour::Idle => {}
                }
                shared.bump();
            }
            _ => {}
        }
        if flush(&mut sink, out, close).await.is_err() || close.is_some() {
            break;
        }
    }
    shared.bump();
}

async fn flush(sink: &mut futures_util::stream::SplitSink<WebSocket, Message>, out: Vec<Message>, close: Option<(u16, &str)>) -> Result<(), axum::Error> {
    for m in out {
        sink.send(m).await?;
    }
    if let Some((code, reason)) = close {
        sink.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await?;
    }
    Ok(())
}

/// Remote-auth controls, from [`crate::MockDiscord::remote_auth`].
pub struct RemoteAuthControl<'a> {
    pub(crate) shared: &'a Shared,
}

impl RemoteAuthControl<'_> {
    /// Behaviours for the next connections, in order (afterwards: [`RemoteAuthBehaviour::Login`]).
    pub fn script(&self, behaviours: impl IntoIterator<Item = RemoteAuthBehaviour>) {
        self.shared.remote.lock().behaviours.extend(behaviours);
    }

    /// The account that "scans" the code (sent in `pending_ticket`).
    pub fn set_scanning_user(&self, user: UserBuilder) {
        self.shared.remote.lock().user = user;
    }

    /// Token returned by the remote-auth login (defaults to the account token).
    pub fn set_token(&self, token: &str) {
        self.shared.remote.lock().token = Some(token.to_string());
    }

    pub fn set_heartbeat_interval(&self, ms: u64) {
        self.shared.remote.lock().heartbeat_interval = ms;
    }

    pub fn sessions(&self) -> Vec<RemoteAuthSessionInfo> {
        self.shared.remote.lock().sessions.clone()
    }

    /// Waits until at least `n` sessions satisfy `pred`.
    pub async fn wait_for_sessions(&self, n: usize, pred: impl Fn(&RemoteAuthSessionInfo) -> bool) -> Vec<RemoteAuthSessionInfo> {
        let got = self
            .shared
            .wait_for(DEFAULT_WAIT, || {
                let s = self.sessions();
                (s.iter().filter(|x| pred(x)).count() >= n).then_some(s)
            })
            .await;
        got.unwrap_or_else(|| panic!("timed out waiting for {n} remote-auth sessions; have {:#?}", self.sessions()))
    }
}
