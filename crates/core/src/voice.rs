//! Voice channel and DM call management on top of `ferrocord-voice`.
//!
//! Joining is a two-step handshake on the main gateway: we send op 4
//! (VOICE_STATE_UPDATE) and Discord answers with our `VOICE_STATE_UPDATE`
//! (session id) and a `VOICE_SERVER_UPDATE` (token + endpoint). Once both
//! arrive we start the voice engine.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use ferrocord_model::gateway::UpdateVoiceState;
use ferrocord_model::{ChannelType, Event, Snowflake};
use ferrocord_voice::{AudioIo, ConnectInfo, DeviceConfig, Shared, VoiceConnection, VoiceSettings, VoiceState};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;

use crate::{Client, ClientEvent};

/// Snapshot of our voice connection for the UI, plugins and the API.
#[derive(Debug, Clone, Serialize)]
pub struct VoiceStatus {
    pub guild_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    /// `connecting`, `connected`, `reconnecting` or `disconnected`.
    pub state: &'static str,
    pub ping_ms: Option<u64>,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub self_video: bool,
    /// End-to-end encrypted with DAVE.
    pub e2ee: bool,
    pub privacy_code: Option<String>,
    pub error: Option<String>,
}

struct Active {
    guild: Option<Snowflake>,
    channel: Snowflake,
    session_id: Option<String>,
    token: Option<String>,
    endpoint: Option<String>,
    conn: Option<VoiceConnection>,
    watcher: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Default)]
pub struct VoiceManager {
    active: Mutex<Option<Active>>,
    self_mute: Mutex<bool>,
    self_deaf: Mutex<bool>,
    self_video: Mutex<bool>,
    /// DM/group calls we know about: channel → users being rung.
    calls: Mutex<HashMap<Snowflake, HashSet<Snowflake>>>,
}

fn state_str(s: VoiceState) -> &'static str {
    match s {
        VoiceState::Connecting | VoiceState::Handshaking => "connecting",
        VoiceState::Connected => "connected",
        VoiceState::Reconnecting => "reconnecting",
        VoiceState::Disconnected => "disconnected",
    }
}

impl Client {
    /// Joins a voice channel, or starts/joins the call in a DM or group DM.
    pub fn join_voice(&self, channel: Snowflake) {
        let (guild, private) = {
            let s = self.store();
            let c = s.channels.get(&channel);
            (c.and_then(|c| c.guild_id), c.is_some_and(|c| matches!(c.kind, ChannelType::Dm | ChannelType::GroupDm)))
        };
        let vm = self.voice_manager();
        {
            let mut a = vm.active.lock();
            if let Some(old) = a.take() {
                if let Some(c) = old.conn {
                    c.disconnect();
                }
                if let Some(w) = old.watcher {
                    w.abort();
                }
            }
            *a = Some(Active { guild, channel, session_id: None, token: None, endpoint: None, conn: None, watcher: None });
        }
        *vm.self_video.lock() = false;
        self.send_voice_state(guild, Some(channel));
        // Start ringing if nobody is in the call yet.
        if private && !vm.calls.lock().contains_key(&channel) {
            self.spawn_action("ring", move |c| async move { c.http().ring(channel, None).await });
        }
        self.emit_voice_status();
    }

    /// Leaves the current voice channel / call.
    pub fn leave_voice(&self) {
        let guild = self.voice_manager().active.lock().as_ref().map(|a| a.guild);
        if let Some(guild) = guild {
            self.send_voice_state(guild, None);
        }
        self.voice_disconnect_local();
        self.emit_voice_status();
    }

    /// Turns our camera on or off in the current call.
    pub fn set_video(&self, on: bool) -> Result<(), String> {
        let camera = self.config().voice.camera.unwrap_or(0);
        let active = self.voice_manager().active.lock().as_ref().map(|a| (a.guild, a.channel, a.conn.clone()));
        let Some((guild, channel, Some(conn))) = active else { return Err("Join a call first".into()) };
        if on && ferrocord_voice::list_cameras().is_empty() {
            return Err("No camera found".into());
        }
        conn.set_video(on.then_some(ferrocord_voice::VideoSource::Camera(camera)));
        *self.voice_manager().self_video.lock() = on;
        self.send_voice_state(guild, Some(channel));
        self.emit_voice_status();
        Ok(())
    }

    /// Declines an incoming DM call.
    pub fn decline_call(&self, channel: Snowflake) {
        let me = self.store().my_id();
        self.spawn_action("decline_call", move |c| async move { c.http().stop_ringing(channel, Some(&[me])).await });
    }

    pub fn set_self_mute(&self, mute: bool) {
        *self.voice_manager().self_mute.lock() = mute;
        if !mute {
            *self.voice_manager().self_deaf.lock() = false;
        }
        self.apply_self_state();
    }

    pub fn set_self_deaf(&self, deaf: bool) {
        *self.voice_manager().self_deaf.lock() = deaf;
        self.apply_self_state();
    }

    pub fn self_mute(&self) -> bool {
        *self.voice_manager().self_mute.lock()
    }

    pub fn self_deaf(&self) -> bool {
        *self.voice_manager().self_deaf.lock()
    }

    /// The channel we're connected (or connecting) to.
    pub fn voice_channel(&self) -> Option<Snowflake> {
        self.voice_manager().active.lock().as_ref().map(|a| a.channel)
    }

    /// Live engine state (speaking indicators, levels) for the UI.
    pub fn voice_shared(&self) -> Option<Arc<Shared>> {
        self.voice_manager().active.lock().as_ref().and_then(|a| a.conn.as_ref().map(|c| c.shared.clone()))
    }

    /// Applies new device/level settings to the running connection.
    pub fn refresh_voice_settings(&self) {
        let cfg = self.config().voice;
        if let Some(conn) = self.voice_manager().active.lock().as_ref().and_then(|a| a.conn.clone()) {
            conn.update_settings(|s| {
                s.input_gain = cfg.input_gain;
                s.output_volume = cfg.output_volume;
                s.vad_threshold_db = cfg.sensitivity_db;
            });
        }
    }

    pub fn set_user_volume(&self, user: Snowflake, volume: f32) {
        if let Some(conn) = self.voice_manager().active.lock().as_ref().and_then(|a| a.conn.clone()) {
            conn.set_user_volume(user.0, volume);
        }
    }

    /// Whether a DM/group call is currently active in `channel`.
    pub fn call_active(&self, channel: Snowflake) -> bool {
        self.voice_manager().calls.lock().contains_key(&channel)
    }

    pub fn voice_status(&self) -> Option<VoiceStatus> {
        let vm = self.voice_manager();
        let a = vm.active.lock();
        let a = a.as_ref()?;
        let shared = a.conn.as_ref().map(|c| c.shared.clone());
        Some(VoiceStatus {
            guild_id: a.guild,
            channel_id: Some(a.channel),
            state: a.conn.as_ref().map(|c| state_str(c.state())).unwrap_or("connecting"),
            ping_ms: shared.as_ref().and_then(|s| *s.ping_ms.read()),
            self_mute: *vm.self_mute.lock(),
            self_deaf: *vm.self_deaf.lock(),
            self_video: shared.as_ref().map(|s| *s.video_enabled.read()).unwrap_or(*vm.self_video.lock()),
            e2ee: shared.as_ref().is_some_and(|s| *s.dave_version.read() > 0),
            privacy_code: shared.as_ref().and_then(|s| s.privacy_code.read().clone()),
            error: shared.as_ref().and_then(|s| s.last_error.read().clone()),
        })
    }

    fn emit_voice_status(&self) {
        let status = self.voice_status().unwrap_or(VoiceStatus {
            guild_id: None,
            channel_id: None,
            state: "disconnected",
            ping_ms: None,
            self_mute: self.self_mute(),
            self_deaf: self.self_deaf(),
            self_video: false,
            e2ee: false,
            privacy_code: None,
            error: None,
        });
        self.emit(ClientEvent::Voice(status));
    }

    fn apply_self_state(&self) {
        let (mute, deaf) = (self.self_mute(), self.self_deaf());
        let active = self.voice_manager().active.lock().as_ref().map(|a| (a.guild, a.channel, a.conn.clone()));
        if let Some((guild, channel, conn)) = active {
            if let Some(c) = conn {
                c.set_mute(mute);
                c.set_deaf(deaf);
            }
            self.send_voice_state(guild, Some(channel));
        }
        self.emit_voice_status();
    }

    fn send_voice_state(&self, guild: Option<Snowflake>, channel: Option<Snowflake>) {
        let vm = self.voice_manager();
        if let Some(gw) = self.gateway() {
            gw.update_voice_state(&UpdateVoiceState {
                guild_id: guild,
                channel_id: channel,
                self_mute: *vm.self_mute.lock(),
                self_deaf: *vm.self_deaf.lock(),
                self_video: *vm.self_video.lock(),
                flags: Some(2),
            });
        }
    }

    /// Stops the engine without telling the gateway (used on logout/disconnects).
    pub(crate) fn voice_disconnect_local(&self) {
        if let Some(old) = self.voice_manager().active.lock().take() {
            if let Some(c) = old.conn {
                c.disconnect();
            }
            if let Some(w) = old.watcher {
                w.abort();
            }
        }
    }

    /// Feeds main-gateway events that concern voice.
    pub(crate) fn voice_on_event(&self, event: &Event, raw: &Value) {
        let me = self.store().my_id();
        match event {
            Event::VoiceStateUpdate(vs) if vs.user_id == me => {
                let mut stop = false;
                {
                    let mut a = self.voice_manager().active.lock();
                    if let Some(act) = a.as_mut() {
                        match vs.channel_id {
                            None => stop = true,
                            Some(ch) => {
                                act.channel = ch;
                                act.guild = vs.guild_id;
                                act.session_id = Some(vs.session_id.clone());
                            }
                        }
                    }
                }
                if stop {
                    self.voice_disconnect_local();
                    self.emit_voice_status();
                } else {
                    self.maybe_connect_voice();
                }
            }
            Event::VoiceServerUpdate(v) => {
                {
                    let mut a = self.voice_manager().active.lock();
                    let Some(act) = a.as_mut() else { return };
                    let endpoint = v.get("endpoint").and_then(Value::as_str).map(str::to_string);
                    // A null endpoint means the server is being reallocated; wait for the next update.
                    if endpoint.is_none() {
                        return;
                    }
                    if act.endpoint != endpoint {
                        if let Some(c) = act.conn.take() {
                            c.disconnect();
                        }
                        if let Some(w) = act.watcher.take() {
                            w.abort();
                        }
                    }
                    act.endpoint = endpoint;
                    act.token = v.get("token").and_then(Value::as_str).map(str::to_string);
                }
                self.maybe_connect_voice();
            }
            Event::Other { name, .. } if name.starts_with("CALL_") => self.on_call_event(name, raw, me),
            _ => {}
        }
    }

    fn on_call_event(&self, name: &str, raw: &Value, me: Snowflake) {
        let Some(channel) = raw.get("channel_id").and_then(|v| serde_json::from_value::<Snowflake>(v.clone()).ok()) else { return };
        let ringing: HashSet<Snowflake> =
            raw.get("ringing").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()).unwrap_or_default();
        let vm = self.voice_manager();
        match name {
            "CALL_CREATE" | "CALL_UPDATE" => {
                let was_ringing_me = vm.calls.lock().get(&channel).is_some_and(|r| r.contains(&me));
                let rings_me = ringing.contains(&me) && self.voice_channel() != Some(channel);
                vm.calls.lock().insert(channel, ringing);
                if rings_me && !was_ringing_me {
                    self.emit(ClientEvent::IncomingCall { channel_id: channel });
                } else if was_ringing_me && !rings_me {
                    self.emit(ClientEvent::CallEnded { channel_id: channel });
                }
            }
            "CALL_DELETE" => {
                vm.calls.lock().remove(&channel);
                self.emit(ClientEvent::CallEnded { channel_id: channel });
            }
            _ => {}
        }
    }

    fn maybe_connect_voice(&self) {
        let me = self.store().my_id();
        let cfg = self.config().voice;
        let mut a = self.voice_manager().active.lock();
        let Some(act) = a.as_mut() else { return };
        if act.conn.is_some() {
            return;
        }
        let (Some(session_id), Some(token), Some(endpoint)) = (act.session_id.clone(), act.token.clone(), act.endpoint.clone()) else { return };
        let info = ConnectInfo { endpoint, server_id: act.guild.unwrap_or(act.channel).0, channel_id: act.channel.0, user_id: me.0, session_id, token };
        let settings = VoiceSettings {
            input_gain: cfg.input_gain,
            output_volume: cfg.output_volume,
            vad_threshold_db: cfg.sensitivity_db,
            self_mute: self.self_mute(),
            self_deaf: self.self_deaf(),
            ..Default::default()
        };
        let devices = DeviceConfig { input: cfg.input_device, output: cfg.output_device };
        let _g = self.runtime().enter();
        let conn = VoiceConnection::connect(info, settings, AudioIo::Devices(devices));
        let mut events = conn.subscribe();
        let client = self.clone();
        act.watcher = Some(self.runtime().spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(120));
            loop {
                tokio::select! {
                    ev = events.recv() => match ev {
                        Ok(_) => client.emit_voice_status(),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
                    },
                    // Speaking rings and levels change continuously while in a call.
                    _ = tick.tick() => client.request_repaint(),
                }
            }
        }));
        act.conn = Some(conn);
        drop(a);
        self.emit_voice_status();
    }
}
