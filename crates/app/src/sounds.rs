//! UI sound effects, synthesized in code (no audio assets, no licensing questions).
//!
//! Every sound is a short phrase of soft bell/marimba-like notes: a few sine
//! partials with exponential decay, a short attack ramp and a fade at the end of
//! each note so nothing clicks. Playback runs on a lazily started thread that owns
//! the cpal output stream (streams are not `Send` on every platform), mixes
//! overlapping sounds and closes the device after a few seconds of silence.
//!
//! [`install`] registers a client [`Extension`] that turns client events into
//! sounds. Because it watches the events rather than the buttons, mute/deafen/camera
//! toggles from the UI, keybinds, plugins and the local API all sound the same, and
//! sounds still play while the window is minimized (no frame needed).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use ferrocord_core::config::Notifications;
use ferrocord_core::model::{Event, Snowflake};
use ferrocord_core::voice::VoiceStatus;
use ferrocord_core::{Client, ClientEvent, Extension};
use parking_lot::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// New message notification.
    Message,
    /// Mention or DM notification.
    Mention,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    /// We connected to a voice channel or call.
    VoiceJoin,
    /// We left / lost the voice connection.
    VoiceLeave,
    /// Someone joined the voice channel we're in.
    UserJoin,
    /// Someone left the voice channel we're in.
    UserLeave,
    CameraOn,
    CameraOff,
    /// Incoming DM/group call (repeated while ringing).
    Ring,
}

impl Sound {
    #[cfg(test)]
    const ALL: [Sound; 13] = [
        Sound::Message,
        Sound::Mention,
        Sound::Mute,
        Sound::Unmute,
        Sound::Deafen,
        Sound::Undeafen,
        Sound::VoiceJoin,
        Sound::VoiceLeave,
        Sound::UserJoin,
        Sound::UserLeave,
        Sound::CameraOn,
        Sound::CameraOff,
        Sound::Ring,
    ];

    /// Whether the user's settings allow this sound.
    pub fn enabled(self, cfg: &Notifications) -> bool {
        match self {
            Sound::Message | Sound::Mention => cfg.sound,
            Sound::Ring => cfg.call_ring,
            _ => cfg.voice_sounds,
        }
    }
}

// ---------------------------------------------------------------- public API

/// Plays `sound` if the user's settings allow it, at the configured volume and
/// on the configured output device. Never blocks.
pub fn play(client: &Client, sound: Sound) {
    let cfg = client.config();
    if sound.enabled(&cfg.notifications) {
        play_with(sound, cfg.notifications.sound_volume, cfg.voice.output_device);
    }
}

/// Plays `sound` regardless of the on/off settings (settings "Test" button),
/// still honouring the volume and output device.
pub fn preview(client: &Client, sound: Sound) {
    let cfg = client.config();
    play_with(sound, cfg.notifications.sound_volume, cfg.voice.output_device);
}

/// Plays `sound` at `volume` (0.0–1.0) on the named output device (`None` or an
/// unknown name = system default). Never blocks; does nothing without a device.
pub fn play_with(sound: Sound, volume: f32, device: Option<String>) {
    let gain = volume.clamp(0.0, 1.0);
    if gain > 0.0
        && let Some(tx) = sender()
    {
        let _ = tx.send(Cmd::Play { sound, gain, device });
    }
}

/// Starts (or stops) the repeating incoming-call ring.
fn set_ring(client: &Client, on: bool) {
    let Some(tx) = sender() else { return };
    if on {
        let cfg = client.config();
        if !Sound::Ring.enabled(&cfg.notifications) || cfg.notifications.sound_volume <= 0.0 {
            return;
        }
        let _ = tx.send(Cmd::Ring(Some((cfg.notifications.sound_volume.clamp(0.0, 1.0), cfg.voice.output_device))));
    } else {
        let _ = tx.send(Cmd::Ring(None));
    }
}

/// Registers the extension that plays sounds for client events.
pub fn install(client: &Client) {
    client.add_extension(Arc::new(SoundEvents::default()));
}

// ---------------------------------------------------------------- event hooks

#[derive(Default)]
struct SoundEvents {
    state: Mutex<Tracker>,
}

/// What we last saw, so sounds play on transitions only.
#[derive(Default)]
struct Tracker {
    mute: bool,
    deaf: bool,
    /// Channel of the last voice status (connecting or connected).
    channel: Option<Snowflake>,
    video: bool,
    /// Channel we are fully connected to (survives `reconnecting`).
    joined: Option<Snowflake>,
    /// Other users in `joined`.
    others: HashSet<Snowflake>,
    /// Incoming calls currently ringing us.
    ringing: HashSet<Snowflake>,
    /// The latest MESSAGE_CREATE; the notification for it follows right after.
    last_message: Option<Arc<Event>>,
}

/// Other users whose voice state puts them in `channel`.
fn others_in(client: &Client, channel: Snowflake) -> HashSet<Snowflake> {
    let s = client.store();
    let me = s.my_id();
    s.voice_states.values().filter(|v| v.channel_id == Some(channel) && v.user_id != me).map(|v| v.user_id).collect()
}

impl SoundEvents {
    fn on_voice_status(&self, client: &Client, s: &VoiceStatus) {
        let mut sounds = vec![];
        let mut stop_ring = false;
        {
            let mut t = self.state.lock();
            if s.self_deaf != t.deaf {
                sounds.push(if s.self_deaf { Sound::Deafen } else { Sound::Undeafen });
            } else if s.self_mute != t.mute {
                sounds.push(if s.self_mute { Sound::Mute } else { Sound::Unmute });
            }
            // Only a toggle within the same call counts; joining/leaving resets the camera.
            if s.channel_id.is_some() && s.channel_id == t.channel && s.self_video != t.video {
                sounds.push(if s.self_video { Sound::CameraOn } else { Sound::CameraOff });
            }
            if s.state == "connected" && s.channel_id.is_some() && s.channel_id != t.joined {
                t.joined = s.channel_id;
                t.others = s.channel_id.map(|c| others_in(client, c)).unwrap_or_default();
                sounds.push(Sound::VoiceJoin);
            } else if (s.channel_id.is_none() || s.state == "disconnected") && t.joined.is_some() {
                t.joined = None;
                t.others.clear();
                sounds.push(Sound::VoiceLeave);
            } else if s.channel_id.is_some() && s.channel_id != t.joined && t.joined.is_some() {
                // Moving to another channel: stay quiet until the new one connects (then "join").
                t.joined = None;
                t.others.clear();
            }
            // Answering (joining the ringing call) stops the ring at once.
            if let Some(c) = s.channel_id
                && t.ringing.remove(&c)
            {
                stop_ring = t.ringing.is_empty();
            }
            (t.mute, t.deaf, t.channel, t.video) = (s.self_mute, s.self_deaf, s.channel_id, s.self_video);
        }
        if stop_ring {
            set_ring(client, false);
        }
        for sound in sounds {
            play(client, sound);
        }
    }

    fn on_voice_state(&self, client: &Client, user: Snowflake) {
        let sound = {
            let mut t = self.state.lock();
            let Some(channel) = t.joined else { return };
            if user == client.store().my_id() {
                return;
            }
            let now = others_in(client, channel);
            let joined = now.difference(&t.others).next().is_some();
            let left = t.others.difference(&now).next().is_some();
            t.others = now;
            match (joined, left) {
                (true, _) => Sound::UserJoin,
                (false, true) => Sound::UserLeave,
                _ => return,
            }
        };
        play(client, sound);
    }
}

impl Extension for SoundEvents {
    fn id(&self) -> &str {
        "builtin.sounds"
    }

    fn on_event(&self, client: &Client, ev: &ClientEvent) {
        match ev {
            ClientEvent::Dispatch { event, .. } => match &**event {
                Event::MessageCreate(_) => self.state.lock().last_message = Some(event.clone()),
                Event::VoiceStateUpdate(vs) => self.on_voice_state(client, vs.user_id),
                _ => {}
            },
            // The client only raises these when the message isn't in the focused channel
            // of a focused window, which is exactly when the official client plays a sound.
            ClientEvent::Notification(n) => {
                let last = self.state.lock().last_message.take();
                let mention = n.guild_id.is_none()
                    || last.as_deref().is_some_and(|e| matches!(e, Event::MessageCreate(m) if m.id == n.message_id && client.store().is_mentioned(m)));
                play(client, if mention { Sound::Mention } else { Sound::Message });
            }
            ClientEvent::Voice(status) => self.on_voice_status(client, status),
            ClientEvent::IncomingCall { channel_id } => {
                let first = {
                    let mut t = self.state.lock();
                    let first = t.ringing.is_empty();
                    t.ringing.insert(*channel_id);
                    first
                };
                if first {
                    set_ring(client, true);
                }
            }
            ClientEvent::CallEnded { channel_id } => {
                let stop = {
                    let mut t = self.state.lock();
                    t.ringing.remove(channel_id) && t.ringing.is_empty()
                };
                if stop {
                    set_ring(client, false);
                }
            }
            ClientEvent::LoggedOut { .. } => {
                *self.state.lock() = Tracker::default();
                set_ring(client, false);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- synthesis

/// Peak level of a rendered sound before the user volume.
const PEAK: f32 = 0.25;
/// Attack ramp; long enough to avoid a click, short enough to stay crisp.
const ATTACK: f32 = 0.005;
/// Fade at the end of every note so buffers end exactly at zero.
const RELEASE: f32 = 0.008;

/// One note of a phrase: start time and length in seconds, pitch in Hz, relative loudness.
#[derive(Clone, Copy)]
struct Note {
    at: f32,
    freq: f32,
    len: f32,
    amp: f32,
}

/// `note!(at, freq, len, amp)`; a struct literal, so `&[note!(..)]` is promoted to `'static`.
macro_rules! note {
    ($at:expr, $freq:expr, $len:expr, $amp:expr) => {
        Note { at: $at, freq: $freq, len: $len, amp: $amp }
    };
}

/// Partials as (frequency ratio, amplitude, decay speed relative to the fundamental).
/// A touch of inharmonic shimmer up top gives the soft mallet/bell colour.
const BELL: &[(f32, f32, f32)] = &[(1.0, 1.0, 1.0), (2.0, 0.28, 1.7), (3.0, 0.08, 2.6), (4.2, 0.035, 4.0)];
/// Nearly pure tone for tiny blips.
const BLIP: &[(f32, f32, f32)] = &[(1.0, 1.0, 1.0), (2.0, 0.12, 2.0)];

struct Recipe {
    notes: &'static [Note],
    partials: &'static [(f32, f32, f32)],
    /// Relative loudness of the whole sound (1.0 = `PEAK`).
    level: f32,
    /// Pitch starts this much sharp and settles within ~15 ms (a gentle "pop").
    glide: f32,
}

// Pitches (Hz).
const A4: f32 = 440.0;
const C5: f32 = 523.25;
const D5: f32 = 587.33;
const E5: f32 = 659.25;
const F5: f32 = 698.46;
const G5: f32 = 783.99;
const A5: f32 = 880.0;
const B5: f32 = 987.77;
const C6: f32 = 1046.5;
const E6: f32 = 1318.51;
const A6: f32 = 1760.0;

const RING: &[Note] = &[
    note!(0.0, B5, 0.28, 1.0),
    note!(0.12, G5, 0.28, 0.85),
    note!(0.24, B5, 0.28, 1.0),
    note!(0.36, G5, 0.28, 0.85),
    note!(0.55, B5, 0.28, 1.0),
    note!(0.67, G5, 0.28, 0.85),
    note!(0.79, B5, 0.28, 1.0),
    note!(0.91, G5, 0.28, 0.85),
];

fn recipe(sound: Sound) -> Recipe {
    let r = |notes, level, glide| Recipe { notes, partials: BELL, level, glide };
    match sound {
        Sound::Message => r(&[note!(0.0, G5, 0.12, 1.0)], 0.85, 0.2),
        Sound::Mention => r(&[note!(0.0, E5, 0.16, 0.85), note!(0.075, B5, 0.22, 1.0)], 1.0, 0.0),
        Sound::Mute => r(&[note!(0.0, A5, 0.12, 1.0), note!(0.07, E5, 0.16, 0.9)], 0.9, 0.0),
        Sound::Unmute => r(&[note!(0.0, E5, 0.12, 0.9), note!(0.07, A5, 0.16, 1.0)], 0.9, 0.0),
        Sound::Deafen => r(&[note!(0.0, D5, 0.13, 1.0), note!(0.08, A4, 0.2, 0.95)], 0.9, 0.0),
        Sound::Undeafen => r(&[note!(0.0, A4, 0.13, 0.95), note!(0.08, D5, 0.2, 1.0)], 0.9, 0.0),
        Sound::VoiceJoin => r(&[note!(0.0, C5, 0.22, 0.85), note!(0.075, E5, 0.22, 0.9), note!(0.15, G5, 0.32, 1.0)], 1.0, 0.0),
        Sound::VoiceLeave => r(&[note!(0.0, G5, 0.22, 1.0), note!(0.075, E5, 0.22, 0.9), note!(0.15, C5, 0.32, 0.85)], 1.0, 0.0),
        Sound::UserJoin => r(&[note!(0.0, F5, 0.14, 0.9), note!(0.08, C6, 0.2, 1.0)], 0.6, 0.0),
        Sound::UserLeave => r(&[note!(0.0, C6, 0.14, 1.0), note!(0.08, F5, 0.2, 0.9)], 0.6, 0.0),
        Sound::CameraOn => Recipe { notes: &[note!(0.0, E6, 0.045, 0.85), note!(0.05, A6, 0.06, 1.0)], partials: BLIP, level: 0.65, glide: 0.06 },
        Sound::CameraOff => Recipe { notes: &[note!(0.0, A6, 0.045, 1.0), note!(0.05, E6, 0.06, 0.85)], partials: BLIP, level: 0.65, glide: 0.06 },
        Sound::Ring => r(RING, 0.9, 0.0),
    }
}

/// Length of `sound` in seconds.
pub fn duration(sound: Sound) -> f32 {
    recipe(sound).notes.iter().map(|n| n.at + n.len).fold(0.0, f32::max)
}

/// Renders `sound` as mono samples at `rate` Hz.
pub fn render(sound: Sound, rate: u32) -> Vec<f32> {
    let r = recipe(sound);
    let rate_f = rate as f32;
    let mut out = vec![0.0f32; (duration(sound) * rate_f).round() as usize];
    for n in r.notes {
        let start = (n.at * rate_f).round() as usize;
        let len = ((n.len * rate_f).round() as usize).min(out.len().saturating_sub(start));
        // Fundamental falls ~48 dB over the note; higher partials die sooner.
        let tau = n.len / 5.5;
        let mut phase = 0.0f64;
        for i in 0..len {
            let t = i as f32 / rate_f;
            let attack = if t < ATTACK { 0.5 - 0.5 * (std::f32::consts::PI * t / ATTACK).cos() } else { 1.0 };
            let left = n.len - t;
            let release = if left < RELEASE { 0.5 - 0.5 * (std::f32::consts::PI * left.max(0.0) / RELEASE).cos() } else { 1.0 };
            let mut v = 0.0f32;
            for &(ratio, amp, decay) in r.partials {
                v += amp * (-t * decay / tau).exp() * ((phase * ratio as f64).sin() as f32);
            }
            out[start + i] += n.amp * attack * release * v;
            let freq = n.freq * (1.0 + r.glide * (-t / 0.005).exp());
            phase += std::f64::consts::TAU * freq as f64 / rate as f64;
        }
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let k = PEAK * r.level / peak;
        out.iter_mut().for_each(|s| *s *= k);
    }
    out
}

// ---------------------------------------------------------------- playback

enum Cmd {
    Play {
        sound: Sound,
        gain: f32,
        device: Option<String>,
    },
    /// `Some((gain, device))` starts ringing, `None` stops.
    Ring(Option<(f32, Option<String>)>),
}

/// Close the device after this much silence.
const IDLE_CLOSE: Duration = Duration::from_secs(4);
const RING_EVERY: Duration = Duration::from_secs(3);
/// Safety net in case a "call ended" never arrives.
const RING_MAX: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(250);
/// At most this many sounds overlap; the oldest is dropped beyond it.
const MAX_VOICES: usize = 8;

fn sender() -> Option<&'static mpsc::Sender<Cmd>> {
    static TX: OnceLock<Option<mpsc::Sender<Cmd>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new().name("ferrocord-sounds".into()).spawn(move || Player::default().run(rx)) {
            Ok(_) => Some(tx),
            Err(e) => {
                tracing::debug!(error = %e, "could not start the sound thread");
                None
            }
        }
    })
    .as_ref()
}

/// A sound being mixed: shared rendered buffer plus play position.
struct Voice {
    sound: Sound,
    buf: Arc<[f32]>,
    pos: usize,
    gain: f32,
}

type Mix = Arc<Mutex<Vec<Voice>>>;

struct Output {
    _stream: cpal::Stream,
    rate: u32,
    /// Device name we were asked for (not necessarily the one we got).
    device: Option<String>,
    mix: Mix,
}

struct Ringing {
    next: Instant,
    until: Instant,
    gain: f32,
    device: Option<String>,
}

#[derive(Default)]
struct Player {
    out: Option<Output>,
    cache: HashMap<(Sound, u32), Arc<[f32]>>,
    ring: Option<Ringing>,
    idle_since: Option<Instant>,
}

impl Player {
    fn run(mut self, rx: mpsc::Receiver<Cmd>) {
        loop {
            let wake = match &self.ring {
                Some(r) => Some(r.next.saturating_duration_since(Instant::now()).min(POLL)),
                None => self.out.as_ref().map(|_| POLL),
            };
            let cmd = match wake {
                None => match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                },
                Some(t) => match rx.recv_timeout(t) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                },
            };
            match cmd {
                Some(Cmd::Play { sound, gain, device }) => self.start(sound, gain, device),
                Some(Cmd::Ring(Some((gain, device)))) => {
                    if self.ring.is_none() {
                        let now = Instant::now();
                        self.ring = Some(Ringing { next: now, until: now + RING_MAX, gain, device });
                    }
                }
                Some(Cmd::Ring(None)) => self.ring = None,
                None => {}
            }
            self.tick();
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let mut due = None;
        if let Some(r) = &mut self.ring {
            if now >= r.until {
                self.ring = None;
            } else if now >= r.next {
                r.next = now + RING_EVERY;
                due = Some((r.gain, r.device.clone()));
            }
        }
        if let Some((gain, device)) = due {
            self.start(Sound::Ring, gain, device);
        }
        let Some(out) = &self.out else { return };
        if self.ring.is_some() || !out.mix.lock().is_empty() {
            self.idle_since = None;
        } else if now.duration_since(*self.idle_since.get_or_insert(now)) >= IDLE_CLOSE {
            self.out = None;
            self.idle_since = None;
            tracing::debug!("sound output closed after silence");
        }
    }

    fn start(&mut self, sound: Sound, gain: f32, device: Option<String>) {
        // Follow a changed output device, but don't cut off what is playing.
        if self.out.as_ref().is_some_and(|o| o.device != device && o.mix.lock().is_empty()) {
            self.out = None;
        }
        if self.out.is_none() {
            match open_output(&device) {
                Ok(o) => self.out = Some(o),
                Err(e) => {
                    tracing::debug!(error = %e, "no sound output");
                    return;
                }
            }
        }
        let Some(out) = &self.out else { return };
        let buf = self.cache.entry((sound, out.rate)).or_insert_with(|| render(sound, out.rate).into()).clone();
        let mut mix = out.mix.lock();
        // The same sound fired twice within 60 ms (a burst of messages) would just sound louder.
        if mix.iter().any(|v| v.sound == sound && v.pos < out.rate as usize * 60 / 1000) {
            return;
        }
        if mix.len() >= MAX_VOICES {
            mix.remove(0);
        }
        mix.push(Voice { sound, buf, pos: 0, gain });
        self.idle_since = None;
    }
}

/// Mixes the active voices into an interleaved buffer of `len` samples.
fn fill(mix: &Mutex<Vec<Voice>>, channels: usize, len: usize, write: &mut dyn FnMut(usize, f32)) {
    let mut voices = mix.lock();
    let frames = len / channels;
    for f in 0..frames {
        let mut v = 0.0;
        for voice in voices.iter_mut() {
            if let Some(s) = voice.buf.get(voice.pos) {
                v += s * voice.gain;
                voice.pos += 1;
            }
        }
        let v = v.clamp(-1.0, 1.0);
        for c in 0..channels {
            write(f * channels + c, v);
        }
    }
    for i in frames * channels..len {
        write(i, 0.0);
    }
    voices.retain(|v| v.pos < v.buf.len());
}

fn open_output(device: &Option<String>) -> Result<Output, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    // Same matching as the voice engine: by device name, else the system default.
    let named = device.as_ref().and_then(|wanted| host.output_devices().ok()?.find(|d| d.description().is_ok_and(|x| x.name() == wanted)));
    let dev = named.or_else(|| host.default_output_device()).ok_or("no output device")?;
    let sup = dev.default_output_config().map_err(|e| e.to_string())?;
    let fmt = sup.sample_format();
    let conf = sup.config();
    let (rate, ch) = (conf.sample_rate, (conf.channels as usize).max(1));
    let mix: Mix = Arc::default();
    let m = mix.clone();
    let err = |e| tracing::debug!(error = %e, "sound output stream error");
    let stream = match fmt {
        cpal::SampleFormat::F32 => dev.build_output_stream(conf, move |d: &mut [f32], _: &_| fill(&m, ch, d.len(), &mut |i, v| d[i] = v), err, None),
        cpal::SampleFormat::I16 => {
            dev.build_output_stream(conf, move |d: &mut [i16], _: &_| fill(&m, ch, d.len(), &mut |i, v| d[i] = (v * 32767.0) as i16), err, None)
        }
        cpal::SampleFormat::U16 => {
            dev.build_output_stream(conf, move |d: &mut [u16], _: &_| fill(&m, ch, d.len(), &mut |i, v| d[i] = (v * 32767.0 + 32768.0) as u16), err, None)
        }
        other => return Err(format!("unsupported output sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(Output { _stream: stream, rate, device: device.clone(), mix })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_renders_cleanly() {
        for rate in [44_100, 48_000, 96_000] {
            for sound in Sound::ALL {
                let buf = render(sound, rate);
                assert!(!buf.is_empty(), "{sound:?} @ {rate}");
                assert_eq!(buf.len(), (duration(sound) * rate as f32).round() as usize, "{sound:?} @ {rate}");
                let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!(peak <= PEAK + 1e-4, "{sound:?} peak {peak}");
                assert!(peak > 0.05, "{sound:?} too quiet: {peak}");
                // No clicks: starts from silence and fades out completely.
                assert!(buf[0].abs() < 1e-3, "{sound:?} starts at {}", buf[0]);
                assert!(buf[buf.len() - 1].abs() < 1e-3, "{sound:?} ends at {}", buf[buf.len() - 1]);
                assert!(buf.iter().all(|s| s.is_finite()));
            }
        }
    }

    #[test]
    fn lengths_match_the_design() {
        assert_eq!(render(Sound::Message, 48_000).len(), 5_760);
        assert_eq!(render(Sound::Message, 44_100).len(), 5_292);
        let ring = duration(Sound::Ring);
        assert!((1.0..=1.4).contains(&ring), "ring lasts {ring}s");
        assert!(duration(Sound::Message) <= 0.15);
    }

    #[test]
    fn no_sample_to_sample_jumps() {
        // A click shows up as a large jump between neighbouring samples.
        for sound in Sound::ALL {
            let buf = render(sound, 48_000);
            let max_step = buf.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
            assert!(max_step < 0.1, "{sound:?} jumps by {max_step}");
        }
    }

    #[test]
    fn mixer_sums_and_retires_voices() {
        let mix = Mutex::new(vec![
            Voice { sound: Sound::Message, buf: Arc::from(vec![0.5, 0.5]), pos: 0, gain: 0.5 },
            Voice { sound: Sound::Mute, buf: Arc::from(vec![0.25]), pos: 0, gain: 1.0 },
        ]);
        let mut out = vec![9.0f32; 5];
        fill(&mix, 2, out.len(), &mut |i, v| out[i] = v);
        assert_eq!(out, vec![0.5, 0.5, 0.25, 0.25, 0.0]);
        assert!(mix.lock().is_empty());
    }

    #[test]
    fn settings_gate_categories() {
        let mut cfg = Notifications { sound: false, ..Default::default() };
        assert!(!Sound::Mention.enabled(&cfg) && Sound::Mute.enabled(&cfg) && Sound::Ring.enabled(&cfg));
        cfg.voice_sounds = false;
        assert!(!Sound::UserJoin.enabled(&cfg) && !Sound::CameraOn.enabled(&cfg));
        cfg.call_ring = false;
        assert!(!Sound::Ring.enabled(&cfg));
    }
}
