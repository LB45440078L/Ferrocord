//! A live voice connection: voice gateway + UDP media + audio engine + DAVE.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::audio::{self, DeviceAudio, DeviceConfig, FRAME, Mixer, OpusEncoder, PlaybackRing, SILENCE_FRAME, Vad};
use crate::crypto::{TransportCipher, choose_mode};
use crate::dave::{Dave, Outgoing};
use crate::protocol::{self, op};
use crate::rtp::{RtpHeader, is_rtcp};

/// Everything needed to join, from the main gateway's VOICE_STATE_UPDATE and VOICE_SERVER_UPDATE.
#[derive(Debug, Clone)]
pub struct ConnectInfo {
    /// `host[:port]` from VOICE_SERVER_UPDATE, or a full `ws://`/`wss://` URL (tests).
    pub endpoint: String,
    /// Guild id, or the channel id for DM/group calls.
    pub server_id: u64,
    pub channel_id: u64,
    pub user_id: u64,
    pub session_id: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct VoiceSettings {
    pub input_gain: f32,
    pub output_volume: f32,
    /// Voice activity threshold in dBFS ("input sensitivity").
    pub vad_threshold_db: f32,
    pub opus_bitrate: i32,
    pub self_mute: bool,
    pub self_deaf: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self { input_gain: 1.0, output_volume: 1.0, vad_threshold_db: -50.0, opus_bitrate: 64_000, self_mute: false, self_deaf: false }
    }
}

/// Where PCM comes from and goes to.
pub enum AudioIo {
    /// Real devices via cpal.
    Devices(DeviceConfig),
    /// Externally driven (tests, recorders): 20 ms mono 48 kHz frames in, stereo out.
    Custom { capture: mpsc::Receiver<Vec<f32>>, playback: PlaybackRing },
    /// Listen-only without devices.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    Connecting,
    Handshaking,
    Connected,
    Reconnecting,
    Disconnected,
}

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    State(VoiceState),
    Error(String),
    /// The call's E2EE state changed (DAVE version, privacy code).
    Encryption {
        dave_version: u16,
        privacy_code: Option<String>,
    },
}

/// Live state for the UI; cheap to read every frame.
#[derive(Debug, Default)]
pub struct Shared {
    pub state: RwLock<Option<VoiceState>>,
    pub ping_ms: RwLock<Option<u64>>,
    pub users: RwLock<HashSet<u64>>,
    pub speaking: RwLock<HashMap<u64, Instant>>,
    pub self_speaking: RwLock<bool>,
    pub input_level_db: RwLock<f32>,
    pub ssrc_user: RwLock<HashMap<u32, u64>>,
    pub user_volume: RwLock<HashMap<u64, f32>>,
    pub settings: RwLock<VoiceSettings>,
    pub encryption_mode: RwLock<Option<String>>,
    pub dave_version: RwLock<u16>,
    pub privacy_code: RwLock<Option<String>>,
    pub last_error: RwLock<Option<String>>,
}

impl Shared {
    /// Users heard within the last 250 ms.
    pub fn is_speaking(&self, user: u64) -> bool {
        self.speaking.read().get(&user).is_some_and(|t| t.elapsed() < Duration::from_millis(250))
    }
}

enum Cmd {
    Disconnect,
    Ws(Outgoing),
}

/// Handle to a running connection. Dropping it does not disconnect; call [`VoiceConnection::disconnect`].
#[derive(Clone)]
pub struct VoiceConnection {
    cmd: mpsc::UnboundedSender<Cmd>,
    pub shared: Arc<Shared>,
    events: broadcast::Sender<VoiceEvent>,
}

impl VoiceConnection {
    /// Starts connecting in the background. Must be called within a Tokio runtime.
    pub fn connect(info: ConnectInfo, settings: VoiceSettings, audio: AudioIo) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(64);
        let shared = Arc::new(Shared::default());
        *shared.settings.write() = settings;
        shared.users.write().insert(info.user_id);
        let conn = Self { cmd: cmd_tx.clone(), shared: shared.clone(), events: events.clone() };
        tokio::spawn(run(info, audio, shared, events, cmd_rx, cmd_tx));
        conn
    }

    pub fn subscribe(&self) -> broadcast::Receiver<VoiceEvent> {
        self.events.subscribe()
    }

    pub fn state(&self) -> VoiceState {
        self.shared.state.read().unwrap_or(VoiceState::Connecting)
    }

    pub fn set_mute(&self, mute: bool) {
        self.shared.settings.write().self_mute = mute;
    }

    pub fn set_deaf(&self, deaf: bool) {
        self.shared.settings.write().self_deaf = deaf;
    }

    pub fn set_user_volume(&self, user: u64, volume: f32) {
        self.shared.user_volume.write().insert(user, volume.clamp(0.0, 2.0));
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut VoiceSettings)) {
        f(&mut self.shared.settings.write());
    }

    pub fn disconnect(&self) {
        let _ = self.cmd.send(Cmd::Disconnect);
    }
}

fn set_state(shared: &Shared, events: &broadcast::Sender<VoiceEvent>, s: VoiceState) {
    *shared.state.write() = Some(s);
    let _ = events.send(VoiceEvent::State(s));
}

/// Media state shared by the gateway, sender and receiver tasks.
struct Media {
    cipher: Option<TransportCipher>,
    dave: Dave,
    ssrc: u32,
}

fn ws_url(endpoint: &str) -> String {
    let base = if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") { endpoint.to_string() } else { format!("wss://{endpoint}") };
    let base = base.trim_end_matches('/');
    format!("{base}/?v={}", protocol::VOICE_GATEWAY_VERSION)
}

fn frame(op: u8, d: Value) -> WsMessage {
    WsMessage::Text(json!({ "op": op, "d": d }).to_string().into())
}

fn outgoing_msg(o: Outgoing) -> WsMessage {
    match o {
        Outgoing::Json(op, d) => frame(op, d),
        Outgoing::Binary(op, payload) => WsMessage::Binary(protocol::build_binary(op, &payload).into()),
    }
}

async fn run(
    info: ConnectInfo,
    audio_io: AudioIo,
    shared: Arc<Shared>,
    events: broadcast::Sender<VoiceEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
) {
    set_state(&shared, &events, VoiceState::Connecting);
    let media = Arc::new(Mutex::new(Media { cipher: None, dave: Dave::new(info.user_id, info.channel_id), ssrc: 0 }));
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = vec![];
    let mut _devices: Option<DeviceAudio> = None;
    let mut audio_io = Some(audio_io);
    let mut seq_ack: i64 = -1;
    let mut resumable = false;
    let mut attempts = 0u32;

    'outer: loop {
        let url = ws_url(&info.endpoint);
        let ws = match tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(url.as_str())).await {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                fail(&shared, &events, format!("voice gateway connect failed: {e}"));
                if retry(&shared, &events, &mut attempts).await {
                    continue;
                }
                break;
            }
            Err(_) => {
                fail(&shared, &events, "voice gateway connect timed out".into());
                if retry(&shared, &events, &mut attempts).await {
                    continue;
                }
                break;
            }
        };
        let (mut sink, mut stream) = ws.split();
        set_state(&shared, &events, VoiceState::Handshaking);

        let ids = (info.server_id.to_string(), info.channel_id.to_string());
        if resumable {
            let r = protocol::Resume {
                token: info.token.clone(),
                session_id: info.session_id.clone(),
                server_id: ids.0.clone(),
                channel_id: ids.1.clone(),
                seq_ack,
            };
            let _ = sink.send(frame(op::RESUME, serde_json::to_value(r).unwrap_or_default())).await;
        } else {
            let id = protocol::Identify {
                server_id: ids.0.clone(),
                channel_id: ids.1.clone(),
                user_id: info.user_id.to_string(),
                session_id: info.session_id.clone(),
                token: info.token.clone(),
                max_dave_protocol_version: protocol::MAX_DAVE_PROTOCOL_VERSION,
                video: false,
                streams: vec![protocol::StreamDescriptor::camera()],
            };
            let _ = sink.send(frame(op::IDENTIFY, serde_json::to_value(id).unwrap_or_default())).await;
        }

        let mut heartbeat: Option<tokio::time::Interval> = None;
        let mut awaiting_ack = false;
        let mut udp: Option<Arc<UdpSocket>> = None;

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
                    if awaiting_ack {
                        tracing::warn!("voice heartbeat not acknowledged; reconnecting");
                        break;
                    }
                    awaiting_ack = true;
                    let _ = sink.send(frame(op::HEARTBEAT, json!({ "t": now_ms(), "seq_ack": seq_ack }))).await;
                }
                cmd = cmd_rx.recv() => match cmd {
                    None | Some(Cmd::Disconnect) => {
                        let _ = sink.send(WsMessage::Close(None)).await;
                        break 'outer;
                    }
                    Some(Cmd::Ws(o)) => {
                        let _ = sink.send(outgoing_msg(o)).await;
                    }
                },
                msg = stream.next() => {
                    let msg = match msg {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => { tracing::warn!(error = %e, "voice gateway read error"); break; }
                        None => break,
                    };
                    let (opcode, d, binary): (u8, Value, Option<Vec<u8>>) = match msg {
                        WsMessage::Text(t) => match serde_json::from_str::<protocol::Frame>(&t) {
                            Ok(f) => {
                                if let Some(s) = f.seq { seq_ack = s; }
                                (f.op, f.d, None)
                            }
                            Err(_) => continue,
                        },
                        WsMessage::Binary(b) => match protocol::parse_binary(&b) {
                            Some((seq, opcode, payload)) => {
                                seq_ack = seq as i64;
                                (opcode, Value::Null, Some(payload.to_vec()))
                            }
                            None => continue,
                        },
                        WsMessage::Close(f) => {
                            let code = f.as_ref().map(|f| u16::from(f.code)).unwrap_or(1000);
                            tracing::info!(code, "voice gateway closed");
                            if protocol::close::is_fatal(code) {
                                fail(&shared, &events, format!("voice session ended (code {code})"));
                                break 'outer;
                            }
                            break;
                        }
                        _ => continue,
                    };
                    let replies = handle(opcode, d, binary, &shared, &events, &media, &mut udp, &mut tasks, &mut audio_io, &mut _devices, &cmd_tx, &mut heartbeat, &mut awaiting_ack, &mut resumable).await;
                    for r in replies {
                        let _ = sink.send(r).await;
                    }
                    attempts = 0;
                }
            }
        }
        // Lost the gateway: try to resume (media tasks keep running on the same UDP socket).
        if !retry(&shared, &events, &mut attempts).await {
            break;
        }
    }

    for t in tasks {
        t.abort();
    }
    set_state(&shared, &events, VoiceState::Disconnected);
}

fn fail(shared: &Shared, events: &broadcast::Sender<VoiceEvent>, e: String) {
    tracing::warn!(error = %e, "voice");
    *shared.last_error.write() = Some(e.clone());
    let _ = events.send(VoiceEvent::Error(e));
}

async fn retry(shared: &Shared, events: &broadcast::Sender<VoiceEvent>, attempts: &mut u32) -> bool {
    *attempts += 1;
    if *attempts > 5 {
        return false;
    }
    set_state(shared, events, VoiceState::Reconnecting);
    tokio::time::sleep(Duration::from_millis(500 * (1 << (*attempts).min(4)) as u64)).await;
    true
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
async fn handle(
    opcode: u8,
    d: Value,
    binary: Option<Vec<u8>>,
    shared: &Arc<Shared>,
    events: &broadcast::Sender<VoiceEvent>,
    media: &Arc<Mutex<Media>>,
    udp: &mut Option<Arc<UdpSocket>>,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
    audio_io: &mut Option<AudioIo>,
    devices: &mut Option<DeviceAudio>,
    cmd_tx: &mpsc::UnboundedSender<Cmd>,
    heartbeat: &mut Option<tokio::time::Interval>,
    awaiting_ack: &mut bool,
    resumable: &mut bool,
) -> Vec<WsMessage> {
    let mut out = vec![];
    let dave_out = |o: Vec<Outgoing>| -> Vec<WsMessage> { o.into_iter().map(outgoing_msg).collect() };
    match opcode {
        op::HELLO => {
            let ms = d.get("heartbeat_interval").and_then(Value::as_f64).unwrap_or(5000.0).clamp(250.0, 5000.0);
            let mut i = tokio::time::interval(Duration::from_millis(ms as u64));
            i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            i.tick().await;
            *heartbeat = Some(i);
        }
        op::HEARTBEAT_ACK => {
            *awaiting_ack = false;
            if let Some(t) = d.get("t").and_then(Value::as_u64) {
                *shared.ping_ms.write() = Some(now_ms().saturating_sub(t));
            }
        }
        op::HEARTBEAT => out.push(frame(op::HEARTBEAT, json!({ "t": now_ms() }))),
        op::READY => {
            let Ok(ready) = serde_json::from_value::<protocol::Ready>(d) else { return out };
            media.lock().ssrc = ready.ssrc;
            let Some(mode) = choose_mode(&ready.modes) else {
                fail(shared, events, format!("no supported encryption mode in {:?}", ready.modes));
                return out;
            };
            let addr: SocketAddr = match format!("{}:{}", ready.ip, ready.port).parse() {
                Ok(a) => a,
                Err(_) => match tokio::net::lookup_host((ready.ip.as_str(), ready.port)).await.ok().and_then(|mut a| a.next()) {
                    Some(a) => a,
                    None => {
                        fail(shared, events, format!("bad media address {}:{}", ready.ip, ready.port));
                        return out;
                    }
                },
            };
            match crate::udp::connect_and_discover(addr, ready.ssrc).await {
                Ok((sock, ip, port)) => {
                    *udp = Some(Arc::new(sock));
                    let sel = protocol::SelectProtocol {
                        protocol: "udp".into(),
                        data: protocol::SelectProtocolData { address: ip.clone(), port, mode: mode.into() },
                        address: ip,
                        port,
                        mode: mode.into(),
                        codecs: protocol::codecs(false),
                        rtc_connection_id: uuid_v4(),
                        experiments: vec![],
                    };
                    out.push(frame(op::SELECT_PROTOCOL, serde_json::to_value(sel).unwrap_or_default()));
                    out.push(frame(op::VOICE_BACKEND_VERSION, json!({})));
                }
                Err(e) => fail(shared, events, format!("UDP connection failed: {e}")),
            }
        }
        op::SESSION_DESCRIPTION => {
            let Ok(sd) = serde_json::from_value::<protocol::SessionDescription>(d) else { return out };
            match TransportCipher::new(&sd.mode, &sd.secret_key) {
                Ok(c) => {
                    *shared.encryption_mode.write() = Some(sd.mode.clone());
                    let dave_msgs = {
                        let mut m = media.lock();
                        m.cipher = Some(c);
                        m.dave.init(sd.dave_protocol_version)
                    };
                    *shared.dave_version.write() = sd.dave_protocol_version;
                    let _ = events.send(VoiceEvent::Encryption { dave_version: sd.dave_protocol_version, privacy_code: None });
                    out.extend(dave_out(dave_msgs));
                    if let Some(sock) = udp.clone()
                        && tasks.is_empty()
                    {
                        start_media(sock, shared.clone(), media.clone(), audio_io, devices, cmd_tx.clone(), tasks);
                    }
                    *resumable = true;
                    set_state(shared, events, VoiceState::Connected);
                }
                Err(e) => fail(shared, events, format!("encryption setup failed: {e}")),
            }
        }
        op::RESUMED => set_state(shared, events, VoiceState::Connected),
        op::SPEAKING => {
            if let (Some(user), Some(ssrc)) = (d.get("user_id").and_then(protocol::snowflake), d.get("ssrc").and_then(Value::as_u64)) {
                shared.ssrc_user.write().insert(ssrc as u32, user);
                shared.users.write().insert(user);
            }
        }
        op::VIDEO => {
            if let Some(user) = d.get("user_id").and_then(protocol::snowflake) {
                if let Some(a) = d.get("audio_ssrc").and_then(Value::as_u64).filter(|a| *a != 0) {
                    shared.ssrc_user.write().insert(a as u32, user);
                }
                shared.users.write().insert(user);
            }
        }
        op::CLIENTS_CONNECT => {
            let ids: Vec<u64> = d.get("user_ids").and_then(Value::as_array).map(|a| a.iter().filter_map(protocol::snowflake).collect()).unwrap_or_default();
            shared.users.write().extend(ids.iter().copied());
            media.lock().dave.clients_connected(ids);
        }
        op::CLIENT_DISCONNECT => {
            if let Some(user) = d.get("user_id").and_then(protocol::snowflake) {
                shared.users.write().remove(&user);
                shared.ssrc_user.write().retain(|_, u| *u != user);
                shared.speaking.write().remove(&user);
                media.lock().dave.client_disconnected(user);
            }
        }
        op::DAVE_PREPARE_TRANSITION => {
            let tid = d.get("transition_id").and_then(Value::as_u64).unwrap_or(0) as u16;
            let ver = d.get("protocol_version").and_then(Value::as_u64).unwrap_or(0) as u16;
            out.extend(dave_out(media.lock().dave.on_prepare_transition(tid, ver)));
        }
        op::DAVE_EXECUTE_TRANSITION => {
            let tid = d.get("transition_id").and_then(Value::as_u64).unwrap_or(0) as u16;
            let (msgs, version, code) = {
                let mut m = media.lock();
                let msgs = m.dave.on_execute_transition(tid);
                (msgs, m.dave.protocol_version(), m.dave.privacy_code())
            };
            *shared.dave_version.write() = version;
            *shared.privacy_code.write() = code.clone();
            let _ = events.send(VoiceEvent::Encryption { dave_version: version, privacy_code: code });
            out.extend(dave_out(msgs));
        }
        op::DAVE_PREPARE_EPOCH => {
            let epoch = d.get("epoch").and_then(Value::as_u64).unwrap_or(0);
            let ver = d.get("protocol_version").and_then(Value::as_u64).unwrap_or(0) as u16;
            out.extend(dave_out(media.lock().dave.on_prepare_epoch(epoch, ver)));
        }
        op::DAVE_MLS_EXTERNAL_SENDER | op::DAVE_MLS_PROPOSALS | op::DAVE_MLS_ANNOUNCE_COMMIT_TRANSITION | op::DAVE_MLS_WELCOME => {
            let payload = binary.unwrap_or_default();
            let msgs = {
                let mut m = media.lock();
                let msgs = match opcode {
                    op::DAVE_MLS_EXTERNAL_SENDER => m.dave.on_external_sender(&payload),
                    op::DAVE_MLS_PROPOSALS => m.dave.on_proposals(&payload),
                    op::DAVE_MLS_ANNOUNCE_COMMIT_TRANSITION => m.dave.on_announce_commit(&payload),
                    _ => m.dave.on_welcome(&payload),
                };
                *shared.privacy_code.write() = m.dave.privacy_code();
                msgs
            };
            out.extend(dave_out(msgs));
        }
        other => tracing::trace!(op = other, "unhandled voice op"),
    }
    out
}

fn uuid_v4() -> String {
    let b: [u8; 16] = rand::random();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-{:01x}{:01x}{:02x}-{}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6] & 0x0f,
        b[7],
        8 | (b[8] & 0x3),
        b[8] >> 4,
        b[9],
        b[10..16].iter().map(|x| format!("{x:02x}")).collect::<String>()
    )
}

fn start_media(
    sock: Arc<UdpSocket>,
    shared: Arc<Shared>,
    media: Arc<Mutex<Media>>,
    audio_io: &mut Option<AudioIo>,
    devices: &mut Option<DeviceAudio>,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    let playback: PlaybackRing = Arc::default();
    let (cap_tx, cap_rx) = mpsc::channel::<Vec<f32>>(50);
    let (ring, capture_rx) = match audio_io.take() {
        Some(AudioIo::Devices(cfg)) => {
            let (std_tx, std_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(50);
            match DeviceAudio::start(cfg, std_tx, playback.clone()) {
                Ok(d) => *devices = Some(d),
                Err(e) => {
                    tracing::warn!(error = %e, "audio devices unavailable");
                    *shared.last_error.write() = Some(format!("Audio device error: {e}"));
                }
            }
            // Bridge the device thread's std channel into async land.
            std::thread::spawn(move || {
                while let Ok(f) = std_rx.recv() {
                    if cap_tx.blocking_send(f).is_err() {
                        break;
                    }
                }
            });
            (playback, cap_rx)
        }
        Some(AudioIo::Custom { capture, playback }) => (playback, capture),
        Some(AudioIo::None) | None => (playback, cap_rx),
    };
    let mixer = Arc::new(Mutex::new(Mixer::default()));
    tasks.push(tokio::spawn(send_loop(sock.clone(), shared.clone(), media.clone(), capture_rx, cmd_tx.clone())));
    tasks.push(tokio::spawn(recv_loop(sock, shared.clone(), media, mixer.clone(), cmd_tx)));
    tasks.push(tokio::spawn(mix_loop(shared, mixer, ring)));
}

async fn send_loop(
    sock: Arc<UdpSocket>,
    shared: Arc<Shared>,
    media: Arc<Mutex<Media>>,
    mut capture: mpsc::Receiver<Vec<f32>>,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
) {
    let bitrate = shared.settings.read().opus_bitrate;
    let mut encoder = match OpusEncoder::new(bitrate) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "Opus encoder failed");
            return;
        }
    };
    let mut vad = Vad::new(shared.settings.read().vad_threshold_db);
    let (mut seq, mut ts): (u16, u32) = (rand::random(), rand::random());
    let mut speaking = false;
    let mut silence_left = 0u8;
    while let Some(mut pcm) = capture.recv().await {
        let s = shared.settings.read().clone();
        vad.threshold_db = s.vad_threshold_db;
        for x in pcm.iter_mut() {
            *x *= s.input_gain;
        }
        *shared.input_level_db.write() = audio::level_db(&pcm);
        let voice = !s.self_mute && !s.self_deaf && vad.is_speech(&pcm);
        let ssrc = media.lock().ssrc;
        if voice != speaking {
            speaking = voice;
            *shared.self_speaking.write() = voice;
            let flags = if voice { protocol::speaking::MICROPHONE } else { protocol::speaking::NONE };
            let _ = cmd_tx.send(Cmd::Ws(Outgoing::Json(op::SPEAKING, json!({ "speaking": flags, "delay": 0, "ssrc": ssrc }))));
            if !voice {
                silence_left = 5;
            }
        }
        let payload = if voice {
            match encoder.encode(&pcm[..FRAME.min(pcm.len())]) {
                Ok(p) => p,
                Err(_) => continue,
            }
        } else if silence_left > 0 {
            silence_left -= 1;
            SILENCE_FRAME.to_vec()
        } else {
            ts = ts.wrapping_add(FRAME as u32);
            continue;
        };
        let packet = {
            let mut m = media.lock();
            let Some(e2ee) = m.dave.encrypt(davey::MediaType::AUDIO, davey::Codec::OPUS, &payload) else {
                ts = ts.wrapping_add(FRAME as u32);
                continue;
            };
            let mut pkt = Vec::with_capacity(e2ee.len() + 40);
            RtpHeader::write(false, protocol::OPUS_PAYLOAD_TYPE, seq, ts, ssrc, &mut pkt);
            pkt.extend_from_slice(&e2ee);
            match m.cipher.as_mut().map(|c| c.encrypt_rtp(&mut pkt)) {
                Some(Ok(())) => pkt,
                _ => continue,
            }
        };
        seq = seq.wrapping_add(1);
        ts = ts.wrapping_add(FRAME as u32);
        if let Err(e) = sock.send(&packet).await {
            tracing::debug!(error = %e, "UDP send failed");
        }
    }
}

async fn recv_loop(sock: Arc<UdpSocket>, shared: Arc<Shared>, media: Arc<Mutex<Media>>, mixer: Arc<Mutex<Mixer>>, cmd_tx: mpsc::UnboundedSender<Cmd>) {
    let mut buf = vec![0u8; 2048];
    loop {
        let n = match sock.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                tracing::debug!(error = %e, "UDP recv failed");
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };
        let pkt = &buf[..n];
        if is_rtcp(pkt) {
            continue; // Keyframe requests are handled by the video pipeline.
        }
        let mut m = media.lock();
        let Some(Ok((header, payload))) = m.cipher.as_ref().map(|c| c.decrypt_rtp(pkt)) else { continue };
        if header.payload_type != protocol::OPUS_PAYLOAD_TYPE {
            continue;
        }
        let Some(user) = shared.ssrc_user.read().get(&header.ssrc).copied() else { continue };
        let (plain, recovery) = m.dave.decrypt(user, davey::MediaType::AUDIO, &payload);
        drop(m);
        for r in recovery {
            let _ = cmd_tx.send(Cmd::Ws(r));
        }
        if let Some(opus) = plain {
            mixer.lock().push(header.ssrc, header.sequence, opus);
        }
    }
}

async fn mix_loop(shared: Arc<Shared>, mixer: Arc<Mutex<Mixer>>, ring: PlaybackRing) {
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut frame = vec![0f32; FRAME * 2];
    loop {
        tick.tick().await;
        let (deaf, out_vol) = {
            let s = shared.settings.read();
            (s.self_deaf, s.output_volume)
        };
        let ssrc_user = shared.ssrc_user.read().clone();
        let volumes = shared.user_volume.read().clone();
        let active = mixer.lock().mix(&mut frame, |ssrc| {
            let user_vol = ssrc_user.get(&ssrc).and_then(|u| volumes.get(u)).copied().unwrap_or(1.0);
            if deaf { 0.0 } else { user_vol * out_vol }
        });
        if !active.is_empty() {
            let now = Instant::now();
            let mut sp = shared.speaking.write();
            for ssrc in active {
                if let Some(u) = ssrc_user.get(&ssrc) {
                    sp.insert(*u, now);
                }
            }
        }
        if !deaf {
            audio::queue_playback(&ring, &frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_building() {
        assert_eq!(ws_url("c-ams-1.discord.media:2087"), "wss://c-ams-1.discord.media:2087/?v=9");
        assert_eq!(ws_url("ws://127.0.0.1:1234"), "ws://127.0.0.1:1234/?v=9");
    }

    #[test]
    fn uuid_shape() {
        let u = uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
