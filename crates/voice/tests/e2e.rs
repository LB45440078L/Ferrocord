//! End-to-end voice test against a mock Discord voice server.
//!
//! The mock implements the server side of the voice gateway (v9), UDP IP
//! discovery, per-client transport encryption with re-encryption on relay (like
//! Discord's SFU) and the DAVE delivery service (MLS external sender, proposals,
//! commit/welcome routing, transitions). Two real `VoiceConnection`s join, one
//! sends a tone, the other must hear it — end-to-end encrypted.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ferrocord_voice::audio::{FRAME, PlaybackRing, SAMPLE_RATE};
use ferrocord_voice::crypto::TransportCipher;
use ferrocord_voice::protocol::op;
use ferrocord_voice::rtp::RtpHeader;
use ferrocord_voice::udp;
use ferrocord_voice::{AudioIo, ConnectInfo, VoiceConnection, VoiceSettings, VoiceState};
use futures_util::{SinkExt, StreamExt};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const MODE: &str = "aead_aes256_gcm_rtpsize";
const CHANNEL: u64 = 4242;

/// The voice server's MLS external sender (cf. libdave's test ExternalSender).
struct ExternalSenderRole {
    provider: OpenMlsRustCrypto,
    group_id: GroupId,
    signer: SignatureKeyPair,
    sender: openmls::extensions::ExternalSender,
}

impl ExternalSenderRole {
    fn new(channel: u64) -> Self {
        let cs = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;
        let signer = SignatureKeyPair::new(cs.signature_algorithm()).unwrap();
        let sender = openmls::extensions::ExternalSender::new(signer.public().into(), BasicCredential::new(vec![0, 1, 1, 0]).into());
        Self { provider: OpenMlsRustCrypto::default(), group_id: GroupId::from_slice(&channel.to_be_bytes()), signer, sender }
    }

    fn marshalled(&self) -> Vec<u8> {
        self.sender.tls_serialize_detached().unwrap()
    }

    fn propose_add(&self, epoch: u64, key_package: &[u8]) -> Vec<u8> {
        let kp = KeyPackageIn::tls_deserialize_exact_bytes(key_package).unwrap().validate(self.provider.crypto(), ProtocolVersion::Mls10).unwrap();
        let msg =
            ExternalProposal::new_add::<OpenMlsRustCrypto>(kp, self.group_id.clone(), GroupEpoch::from(epoch), &self.signer, SenderExtensionIndex::new(0))
                .unwrap();
        VLBytes::new(msg.tls_serialize_detached().unwrap()).tls_serialize_detached().unwrap()
    }
}

struct Client {
    ssrc: u32,
    cipher: TransportCipher,
    key: Vec<u8>,
    udp_addr: Option<SocketAddr>,
    tx: mpsc::UnboundedSender<Message>,
    key_package: Option<Vec<u8>>,
    seq: u16,
}

#[derive(Default)]
struct Server {
    clients: HashMap<u64, Client>,
    order: Vec<u64>,
    group_formed: bool,
    ready_for: HashMap<u16, Vec<u64>>,
    relayed_packets: u64,
    /// Relayed payloads that carry the DAVE frame trailer (end with magic 0xFAFA).
    dave_frames: u64,
}

impl Server {
    fn send_json(&mut self, user: u64, op: u8, d: Value) {
        if let Some(c) = self.clients.get_mut(&user) {
            c.seq = c.seq.wrapping_add(1);
            let _ = c.tx.send(Message::Text(json!({ "op": op, "d": d, "seq": c.seq }).to_string().into()));
        }
    }

    fn send_binary(&mut self, user: u64, op: u8, payload: &[u8]) {
        if let Some(c) = self.clients.get_mut(&user) {
            c.seq = c.seq.wrapping_add(1);
            let mut b = c.seq.to_be_bytes().to_vec();
            b.push(op);
            b.extend_from_slice(payload);
            let _ = c.tx.send(Message::Binary(b.into()));
        }
    }
}

struct MockVoice {
    ws_addr: SocketAddr,
    server: Arc<Mutex<Server>>,
}

impl MockVoice {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = listener.local_addr().unwrap();
        let udp_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let udp_port = udp_sock.local_addr().unwrap().port();
        let server = Arc::new(Mutex::new(Server::default()));
        let es = Arc::new(ExternalSenderRole::new(CHANNEL));

        // UDP: discovery + relay with per-recipient re-encryption.
        {
            let server = server.clone();
            let sock = udp_sock.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                loop {
                    let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
                    let pkt = &buf[..n];
                    if n == udp::DISCOVERY_LEN && pkt[1] == 1 {
                        let ssrc = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
                        if let Some(c) = server.lock().clients.values_mut().find(|c| c.ssrc == ssrc) {
                            c.udp_addr = Some(from);
                        }
                        let _ = sock.send_to(&udp::discovery_response(ssrc, &from.ip().to_string(), from.port()), from).await;
                        continue;
                    }
                    let mut out = vec![];
                    {
                        let mut s = server.lock();
                        let Some(sender) = s.clients.values().find(|c| c.udp_addr == Some(from)) else { continue };
                        let Ok((h, payload)) = sender.cipher.decrypt_rtp(pkt) else { continue };
                        let sender_ssrc = sender.ssrc;
                        let is_dave = payload.ends_with(&[0xFA, 0xFA]);
                        for c in s.clients.values_mut().filter(|c| c.ssrc != sender_ssrc) {
                            let Some(addr) = c.udp_addr else { continue };
                            let mut fwd = vec![];
                            RtpHeader::write(h.marker, h.payload_type, h.sequence, h.timestamp, h.ssrc, &mut fwd);
                            fwd.extend_from_slice(&payload);
                            if c.cipher.encrypt_rtp(&mut fwd).is_ok() {
                                out.push((fwd, addr));
                            }
                        }
                        s.relayed_packets += 1;
                        if is_dave {
                            s.dave_frames += 1;
                        }
                    }
                    for (p, a) in out {
                        let _ = sock.send_to(&p, a).await;
                    }
                }
            });
        }

        // WebSocket gateway.
        {
            let server = server.clone();
            tokio::spawn(async move {
                let mut next_ssrc = 1000u32;
                loop {
                    let Ok((tcp, _)) = listener.accept().await else { continue };
                    next_ssrc += 1;
                    tokio::spawn(serve_client(tcp, server.clone(), es.clone(), udp_port, next_ssrc));
                }
            });
        }
        Self { ws_addr, server }
    }
}

async fn serve_client(tcp: tokio::net::TcpStream, server: Arc<Mutex<Server>>, es: Arc<ExternalSenderRole>, udp_port: u16, ssrc: u32) {
    let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
    });
    let _ = tx.send(Message::Text(json!({ "op": op::HELLO, "d": { "heartbeat_interval": 1000.0, "v": 9 } }).to_string().into()));
    let mut me: Option<u64> = None;
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                let d = &v["d"];
                match v["op"].as_u64().unwrap() as u8 {
                    op::IDENTIFY => {
                        let user: u64 = d["user_id"].as_str().unwrap().parse().unwrap();
                        assert_eq!(d["max_dave_protocol_version"], 1);
                        assert_eq!(d["channel_id"].as_str(), Some(CHANNEL.to_string().as_str()));
                        me = Some(user);
                        let key: Vec<u8> = (0..32).map(|i| (i as u8) ^ (user as u8)).collect();
                        let mut s = server.lock();
                        s.clients.insert(
                            user,
                            Client { ssrc, cipher: TransportCipher::new(MODE, &key).unwrap(), key, udp_addr: None, tx: tx.clone(), key_package: None, seq: 0 },
                        );
                        s.order.push(user);
                        s.send_json(
                            user,
                            op::READY,
                            json!({ "ssrc": ssrc, "ip": "127.0.0.1", "port": udp_port, "modes": ["xsalsa20_poly1305", MODE], "streams": [] }),
                        );
                        // Tell everyone who is in the call.
                        let all: Vec<u64> = s.order.clone();
                        for u in all.clone() {
                            let others: Vec<String> = all.iter().filter(|x| **x != u).map(|x| x.to_string()).collect();
                            s.send_json(u, op::CLIENTS_CONNECT, json!({ "user_ids": others }));
                        }
                    }
                    op::SELECT_PROTOCOL => {
                        assert_eq!(d["protocol"], "udp");
                        assert_eq!(d["data"]["mode"], MODE);
                        assert_eq!(d["codecs"][0]["name"], "opus");
                        let user = me.unwrap();
                        let mut s = server.lock();
                        let key = s.clients[&user].key.clone();
                        s.send_json(
                            user,
                            op::SESSION_DESCRIPTION,
                            json!({ "mode": MODE, "secret_key": key, "audio_codec": "opus", "dave_protocol_version": 1 }),
                        );
                        s.send_binary(user, op::DAVE_MLS_EXTERNAL_SENDER, &es.marshalled());
                    }
                    op::HEARTBEAT => {
                        let _ = tx.send(Message::Text(json!({ "op": op::HEARTBEAT_ACK, "d": { "t": d["t"] } }).to_string().into()));
                    }
                    op::SPEAKING => {
                        let user = me.unwrap();
                        let mut s = server.lock();
                        let others: Vec<u64> = s.order.iter().copied().filter(|u| *u != user).collect();
                        for o in others {
                            s.send_json(o, op::SPEAKING, json!({ "user_id": user.to_string(), "ssrc": d["ssrc"], "speaking": d["speaking"] }));
                        }
                    }
                    op::DAVE_TRANSITION_READY => {
                        let tid = d["transition_id"].as_u64().unwrap() as u16;
                        let mut s = server.lock();
                        let n = s.order.len();
                        let ready = s.ready_for.entry(tid).or_default();
                        ready.push(me.unwrap());
                        if ready.len() == n {
                            for u in s.order.clone() {
                                s.send_json(u, op::DAVE_EXECUTE_TRANSITION, json!({ "transition_id": tid }));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Message::Binary(b) => {
                let (opcode, payload) = (b[0], &b[1..]);
                let user = me.unwrap();
                let mut s = server.lock();
                match opcode {
                    op::DAVE_MLS_KEY_PACKAGE => {
                        s.clients.get_mut(&user).unwrap().key_package = Some(payload.to_vec());
                        // Once two members have key packages, ask the first to add the second.
                        let ready: Vec<u64> = s.order.iter().copied().filter(|u| s.clients[u].key_package.is_some()).collect();
                        if ready.len() == 2 && !s.group_formed {
                            s.group_formed = true;
                            let (a, b) = (ready[0], ready[1]);
                            let kp_b = s.clients[&b].key_package.clone().unwrap();
                            let mut proposals = vec![0u8]; // APPEND
                            proposals.extend_from_slice(&es.propose_add(0, &kp_b));
                            s.send_binary(a, op::DAVE_MLS_PROPOSALS, &proposals);
                        }
                    }
                    op::DAVE_MLS_COMMIT_WELCOME => {
                        // Split `commit || welcome` by parsing the first MLS message.
                        let mut reader = payload;
                        let _commit = MlsMessageIn::tls_deserialize(&mut reader).expect("valid commit");
                        let commit_len = payload.len() - reader.len();
                        let (commit, welcome) = payload.split_at(commit_len);
                        let tid: u16 = 1;
                        let mut c = tid.to_be_bytes().to_vec();
                        c.extend_from_slice(commit);
                        s.send_binary(user, op::DAVE_MLS_ANNOUNCE_COMMIT_TRANSITION, &c);
                        let joiners: Vec<u64> = s.order.iter().copied().filter(|u| *u != user).collect();
                        for j in joiners {
                            let mut w = tid.to_be_bytes().to_vec();
                            w.extend_from_slice(welcome);
                            s.send_binary(j, op::DAVE_MLS_WELCOME, &w);
                        }
                    }
                    _ => {}
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

fn tone(i: usize) -> Vec<f32> {
    (0..FRAME).map(|n| ((i * FRAME + n) as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.5).collect()
}

async fn wait_for(mut f: impl FnMut() -> bool, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_clients_talk_with_dave_e2ee() {
    let mock = MockVoice::start().await;
    let endpoint = format!("ws://{}", mock.ws_addr);
    let info = |user: u64| ConnectInfo {
        endpoint: endpoint.clone(),
        server_id: 1,
        channel_id: CHANNEL,
        user_id: user,
        session_id: format!("s{user}"),
        token: "tok".into(),
    };

    let (a_cap_tx, a_cap_rx) = mpsc::channel(64);
    let a_ring: PlaybackRing = Arc::default();
    let a = VoiceConnection::connect(info(111), VoiceSettings::default(), AudioIo::Custom { capture: a_cap_rx, playback: a_ring.clone() });
    // Let A finish its handshake first, like a real call where someone is already present.
    assert!(wait_for(|| a.state() == VoiceState::Connected, 10).await, "A should connect");

    let (_b_cap_tx, b_cap_rx) = mpsc::channel(64);
    let b_ring: PlaybackRing = Arc::default();
    let b = VoiceConnection::connect(info(222), VoiceSettings::default(), AudioIo::Custom { capture: b_cap_rx, playback: b_ring.clone() });
    assert!(wait_for(|| b.state() == VoiceState::Connected, 10).await, "B should connect");

    // DAVE: both sides converge on the same epoch and privacy code.
    assert!(
        wait_for(|| a.shared.privacy_code.read().is_some() && *a.shared.privacy_code.read() == *b.shared.privacy_code.read(), 10).await,
        "DAVE group should form: A={:?} B={:?}",
        a.shared.privacy_code.read(),
        b.shared.privacy_code.read()
    );
    assert_eq!(*a.shared.encryption_mode.read(), Some(MODE.to_string()));

    // A talks for one second.
    let feeder = tokio::spawn(async move {
        for i in 0..50 {
            if a_cap_tx.send(tone(i)).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let heard = wait_for(|| b_ring.lock().iter().any(|s| s.abs() > 0.05), 10).await;
    assert!(heard, "B should hear A's tone (relayed {} packets)", mock.server.lock().relayed_packets);
    assert!(wait_for(|| b.shared.is_speaking(111), 5).await, "B should show A as speaking");
    {
        let s = mock.server.lock();
        assert!(s.relayed_packets > 0);
        // Every voice frame (silence frames excepted) must be E2EE'd: the SFU only sees DAVE ciphertext.
        assert!(s.dave_frames as f64 >= s.relayed_packets as f64 * 0.8, "only {}/{} frames were DAVE-encrypted", s.dave_frames, s.relayed_packets);
    }
    assert!(a.shared.ping_ms.read().is_some() || wait_for(|| a.shared.ping_ms.read().is_some(), 3).await, "heartbeat ACKs give a ping");
    feeder.await.unwrap();

    a.disconnect();
    b.disconnect();
    assert!(wait_for(|| a.state() == VoiceState::Disconnected && b.state() == VoiceState::Disconnected, 5).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn muted_client_sends_nothing() {
    let mock = MockVoice::start().await;
    let endpoint = format!("ws://{}", mock.ws_addr);
    let (cap_tx, cap_rx) = mpsc::channel(64);
    let settings = VoiceSettings { self_mute: true, ..Default::default() };
    let a = VoiceConnection::connect(
        ConnectInfo { endpoint, server_id: 1, channel_id: CHANNEL, user_id: 333, session_id: "s".into(), token: "t".into() },
        settings,
        AudioIo::Custom { capture: cap_rx, playback: Arc::default() },
    );
    assert!(wait_for(|| a.state() == VoiceState::Connected, 10).await);
    for i in 0..20 {
        cap_tx.send(tone(i)).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mock.server.lock().relayed_packets, 0, "muted client must not transmit audio");
    assert!(!*a.shared.self_speaking.read());
    a.disconnect();
}
