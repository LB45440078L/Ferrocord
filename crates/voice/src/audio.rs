//! Audio engine: capture → VAD → Opus encode, and receive → jitter buffer →
//! Opus decode → mix → playback.
//!
//! The device layer (cpal) only moves PCM through two endpoints: a channel of
//! 20 ms mono 48 kHz capture frames, and a shared ring of interleaved stereo
//! 48 kHz playback samples. Tests drive those endpoints directly.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Bitrate, Channels, MutSignals, SampleRate};
use parking_lot::Mutex;

use crate::rtp::SeqUnwrapper;

pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per channel in one 20 ms frame.
pub const FRAME: usize = 960;
/// Opus' "silence" frame, sent a few times when we stop talking.
pub const SILENCE_FRAME: [u8; 3] = [0xF8, 0xFF, 0xFE];

pub type PlaybackRing = Arc<Mutex<VecDeque<f32>>>;
/// Upper bound on buffered playback (stereo samples): ~200 ms.
const MAX_PLAYBACK_BUFFER: usize = FRAME * 2 * 10;

// ---------------------------------------------------------------- resampling

/// Streaming linear-interpolation resampler for interleaved audio.
/// Adequate for speech; keeps state across calls so frames join seamlessly.
#[derive(Debug, Clone)]
pub struct Resampler {
    step: f64,
    pos: f64,
    channels: usize,
    last: Vec<f32>,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32, channels: usize) -> Self {
        Self { step: from_rate as f64 / to_rate as f64, pos: 0.0, channels, last: vec![0.0; channels] }
    }

    pub fn is_passthrough(&self) -> bool {
        (self.step - 1.0).abs() < f64::EPSILON
    }

    /// Resamples `input` (interleaved), appending to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        if self.is_passthrough() {
            out.extend_from_slice(input);
            return;
        }
        let frames = input.len() / ch;
        let sample = |i: isize, c: usize, last: &[f32]| if i < 0 { last[c] } else { input[i as usize * ch + c] };
        while self.pos < frames as f64 {
            let i = self.pos.floor() as isize;
            let t = (self.pos - i as f64) as f32;
            for c in 0..ch {
                let a = sample(i - 1, c, &self.last);
                let b = sample(i, c, &self.last);
                out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        if frames > 0 {
            self.last.copy_from_slice(&input[(frames - 1) * ch..frames * ch]);
        }
    }
}

// ---------------------------------------------------------------- VAD

/// Energy-based voice activity detection with hangover, like the client's
/// "input sensitivity" slider.
#[derive(Debug, Clone)]
pub struct Vad {
    pub threshold_db: f32,
    hangover_frames: u32,
    remaining: u32,
}

impl Vad {
    pub fn new(threshold_db: f32) -> Self {
        Self { threshold_db, hangover_frames: 15, remaining: 0 }
    }

    pub fn is_speech(&mut self, frame: &[f32]) -> bool {
        if level_db(frame) >= self.threshold_db {
            self.remaining = self.hangover_frames;
            true
        } else if self.remaining > 0 {
            self.remaining -= 1;
            true
        } else {
            false
        }
    }
}

/// RMS level in dBFS (−100 for silence).
pub fn level_db(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return -100.0;
    }
    let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
    if rms <= 1e-5 { -100.0 } else { 20.0 * rms.log10() }
}

// ---------------------------------------------------------------- Opus

pub struct OpusEncoder {
    inner: Encoder,
    stereo: Vec<f32>,
    out: Vec<u8>,
}

impl OpusEncoder {
    pub fn new(bitrate: i32) -> Result<Self, audiopus::Error> {
        let mut inner = Encoder::new(SampleRate::Hz48000, Channels::Stereo, Application::Voip)?;
        inner.set_bitrate(Bitrate::BitsPerSecond(bitrate))?;
        inner.set_inband_fec(true)?;
        inner.set_packet_loss_perc(10)?;
        Ok(Self { inner, stereo: vec![0.0; FRAME * 2], out: vec![0u8; 1500] })
    }

    /// Encodes one 20 ms mono frame (sent as stereo, as Discord expects).
    pub fn encode(&mut self, mono: &[f32]) -> Result<Vec<u8>, audiopus::Error> {
        for (i, s) in mono.iter().take(FRAME).enumerate() {
            self.stereo[i * 2] = *s;
            self.stereo[i * 2 + 1] = *s;
        }
        let n = self.inner.encode_float(&self.stereo, &mut self.out)?;
        Ok(self.out[..n].to_vec())
    }
}

/// Per-speaker receive state: reorders packets and conceals losses.
pub struct SpeakerStream {
    decoder: Decoder,
    seq: SeqUnwrapper,
    buffer: BTreeMap<u64, Vec<u8>>,
    next: Option<u64>,
    started: bool,
    losses: u32,
    pub level_db: f32,
    pub last_audio: std::time::Instant,
}

/// Frames buffered before playout starts (60 ms).
const JITTER_TARGET: usize = 3;
const JITTER_MAX: usize = 25;

impl SpeakerStream {
    pub fn new() -> Result<Self, audiopus::Error> {
        Ok(Self {
            decoder: Decoder::new(SampleRate::Hz48000, Channels::Stereo)?,
            seq: SeqUnwrapper::default(),
            buffer: BTreeMap::new(),
            next: None,
            started: false,
            losses: 0,
            level_db: -100.0,
            last_audio: std::time::Instant::now(),
        })
    }

    pub fn push(&mut self, seq: u16, opus: Vec<u8>) {
        let s = self.seq.unwrap(seq);
        if self.next.is_some_and(|n| s < n) {
            return; // too late
        }
        self.buffer.insert(s, opus);
        self.last_audio = std::time::Instant::now();
        while self.buffer.len() > JITTER_MAX {
            self.buffer.pop_first();
            self.next = self.buffer.keys().next().copied();
        }
    }

    /// Produces the next 20 ms of interleaved stereo audio, if this speaker is active.
    pub fn pull(&mut self, out: &mut [f32]) -> bool {
        if !self.started {
            if self.buffer.len() < JITTER_TARGET {
                return false;
            }
            self.started = true;
            self.next = self.buffer.keys().next().copied();
        }
        let Some(next) = self.next else {
            self.started = false;
            return false;
        };
        let packet = self.buffer.remove(&next);
        self.next = Some(next + 1);
        let n = match &packet {
            Some(p) if p.as_slice() == SILENCE_FRAME => 0,
            Some(p) => {
                self.losses = 0;
                let pkt = audiopus::packet::Packet::try_from(p.as_slice()).ok();
                MutSignals::try_from(&mut out[..]).ok().and_then(|sig| self.decoder.decode_float(pkt, sig, false).ok()).unwrap_or(0)
            }
            None => {
                self.losses += 1;
                if self.losses > 5 || self.buffer.is_empty() {
                    // Stream ended (speaker stopped): reset and wait for new audio.
                    self.started = false;
                    self.next = None;
                    self.losses = 0;
                    return false;
                }
                // Packet-loss concealment.
                MutSignals::try_from(&mut out[..]).ok().and_then(|sig| self.decoder.decode_float(None, sig, false).ok()).unwrap_or(0)
            }
        };
        if n == 0 {
            out.fill(0.0);
        }
        self.level_db = level_db(out);
        true
    }
}

/// Mixes every active speaker into one stereo frame.
#[derive(Default)]
pub struct Mixer {
    pub speakers: HashMap<u32, SpeakerStream>,
    scratch: Vec<f32>,
}

impl Mixer {
    pub fn push(&mut self, ssrc: u32, seq: u16, opus: Vec<u8>) {
        use std::collections::hash_map::Entry;
        let stream = match self.speakers.entry(ssrc) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(v) => match SpeakerStream::new() {
                Ok(s) => v.insert(s),
                Err(e) => {
                    tracing::error!(error = %e, "failed to create Opus decoder");
                    return;
                }
            },
        };
        stream.push(seq, opus);
    }

    /// Mixes one frame. `volume(ssrc)` gives per-speaker gain (0 = muted).
    /// Returns the SSRCs that produced audio this frame.
    pub fn mix(&mut self, out: &mut [f32], volume: impl Fn(u32) -> f32) -> Vec<u32> {
        out.fill(0.0);
        self.scratch.resize(FRAME * 2, 0.0);
        let mut active = vec![];
        for (ssrc, s) in self.speakers.iter_mut() {
            if s.pull(&mut self.scratch) {
                let g = volume(*ssrc);
                if g > 0.0 {
                    for (o, x) in out.iter_mut().zip(&self.scratch) {
                        *o += x * g;
                    }
                }
                if s.level_db > -60.0 {
                    active.push(*ssrc);
                }
            }
        }
        // Soft clip instead of hard wrapping when several people talk.
        for s in out.iter_mut() {
            *s = s.tanh();
        }
        // Drop decoders for speakers silent for a long time.
        self.speakers.retain(|_, s| s.last_audio.elapsed() < std::time::Duration::from_secs(120));
        active
    }

    pub fn remove(&mut self, ssrc: u32) {
        self.speakers.remove(&ssrc);
    }
}

// ---------------------------------------------------------------- devices

/// Names of available input and output devices.
pub fn list_devices() -> (Vec<String>, Vec<String>) {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let names = |devs: Option<Vec<cpal::Device>>| -> Vec<String> {
        devs.unwrap_or_default().iter().filter_map(|d| d.description().ok().map(|x| x.name().to_string())).collect()
    };
    (names(host.input_devices().ok().map(|d| d.collect())), names(host.output_devices().ok().map(|d| d.collect())))
}

#[derive(Debug, Clone, Default)]
pub struct DeviceConfig {
    pub input: Option<String>,
    pub output: Option<String>,
}

/// Running audio devices. Dropping it stops capture and playback.
pub struct DeviceAudio {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for DeviceAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl DeviceAudio {
    /// Opens devices on a dedicated thread (cpal streams are not `Send` everywhere).
    /// Capture frames (20 ms mono, 48 kHz) go to `capture_tx`; playback reads from `playback`.
    pub fn start(cfg: DeviceConfig, capture_tx: mpsc::SyncSender<Vec<f32>>, playback: PlaybackRing) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ferrocord-audio".into())
            .spawn(move || {
                let streams = open_streams(&cfg, capture_tx, playback);
                match streams {
                    Ok(streams) => {
                        let _ = ready_tx.send(Ok(()));
                        while !stop2.load(Ordering::SeqCst) {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        drop(streams);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { stop, thread: Some(thread) }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("audio device did not start in time".into()),
        }
    }
}

fn find_device(devices: Result<impl Iterator<Item = cpal::Device>, cpal::Error>, name: &Option<String>) -> Option<cpal::Device> {
    use cpal::traits::DeviceTrait;
    let wanted = name.as_ref()?;
    devices.ok()?.find(|d| d.description().is_ok_and(|x| x.name() == wanted))
}

fn open_streams(cfg: &DeviceConfig, capture_tx: mpsc::SyncSender<Vec<f32>>, playback: PlaybackRing) -> Result<Vec<cpal::Stream>, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let mut streams = vec![];

    // Input.
    let input = find_device(host.input_devices().map(|d| d.collect::<Vec<_>>().into_iter()), &cfg.input).or_else(|| host.default_input_device());
    if let Some(dev) = input {
        let sup = dev.default_input_config().map_err(|e| e.to_string())?;
        let fmt = sup.sample_format();
        let conf = sup.config();
        let ch = conf.channels as usize;
        let mut rs = Resampler::new(conf.sample_rate, SAMPLE_RATE, 1);
        let mut mono = Vec::with_capacity(4096);
        let mut pending: Vec<f32> = Vec::with_capacity(FRAME * 4);
        let mut on_samples = move |frames: &mut dyn Iterator<Item = f32>| {
            mono.clear();
            let mut acc = 0.0;
            let mut n = 0;
            for s in frames {
                acc += s;
                n += 1;
                if n == ch {
                    mono.push(acc / ch as f32);
                    acc = 0.0;
                    n = 0;
                }
            }
            rs.process(&mono, &mut pending);
            while pending.len() >= FRAME {
                let frame: Vec<f32> = pending.drain(..FRAME).collect();
                let _ = capture_tx.try_send(frame);
            }
        };
        let err = |e| tracing::warn!(error = %e, "input stream error");
        let stream = match fmt {
            cpal::SampleFormat::F32 => dev.build_input_stream(conf, move |d: &[f32], _: &_| on_samples(&mut d.iter().copied()), err, None),
            cpal::SampleFormat::I16 => dev.build_input_stream(conf, move |d: &[i16], _: &_| on_samples(&mut d.iter().map(|s| *s as f32 / 32768.0)), err, None),
            cpal::SampleFormat::U16 => {
                dev.build_input_stream(conf, move |d: &[u16], _: &_| on_samples(&mut d.iter().map(|s| (*s as f32 - 32768.0) / 32768.0)), err, None)
            }
            other => return Err(format!("unsupported input sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        streams.push(stream);
    } else {
        tracing::warn!("no input device available");
    }

    // Output.
    let output = find_device(host.output_devices().map(|d| d.collect::<Vec<_>>().into_iter()), &cfg.output).or_else(|| host.default_output_device());
    if let Some(dev) = output {
        let sup = dev.default_output_config().map_err(|e| e.to_string())?;
        let fmt = sup.sample_format();
        let conf = sup.config();
        let ch = conf.channels as usize;
        let mut rs = Resampler::new(SAMPLE_RATE, conf.sample_rate, 2);
        let mut staged: VecDeque<f32> = VecDeque::new();
        let mut tmp = Vec::with_capacity(8192);
        let mut fill = move |out_len: usize, write: &mut dyn FnMut(usize, f32)| {
            let frames_needed = out_len / ch;
            while staged.len() < frames_needed * 2 {
                let mut ring = playback.lock();
                if ring.is_empty() {
                    break;
                }
                let n = ring.len().min(FRAME * 2);
                let take: Vec<f32> = ring.drain(..n).collect();
                drop(ring);
                tmp.clear();
                rs.process(&take, &mut tmp);
                staged.extend(tmp.iter().copied());
            }
            for f in 0..frames_needed {
                let (l, r) = if staged.len() >= 2 { (staged.pop_front().unwrap_or(0.0), staged.pop_front().unwrap_or(0.0)) } else { (0.0, 0.0) };
                for c in 0..ch {
                    let v = match (ch, c) {
                        (1, _) => (l + r) * 0.5,
                        (_, 0) => l,
                        (_, 1) => r,
                        _ => 0.0,
                    };
                    write(f * ch + c, v);
                }
            }
        };
        let err = |e| tracing::warn!(error = %e, "output stream error");
        let stream = match fmt {
            cpal::SampleFormat::F32 => dev.build_output_stream(conf, move |d: &mut [f32], _: &_| fill(d.len(), &mut |i, v| d[i] = v), err, None),
            cpal::SampleFormat::I16 => {
                dev.build_output_stream(conf, move |d: &mut [i16], _: &_| fill(d.len(), &mut |i, v| d[i] = (v.clamp(-1.0, 1.0) * 32767.0) as i16), err, None)
            }
            cpal::SampleFormat::U16 => dev.build_output_stream(
                conf,
                move |d: &mut [u16], _: &_| fill(d.len(), &mut |i, v| d[i] = ((v.clamp(-1.0, 1.0) * 32767.0) + 32768.0) as u16),
                err,
                None,
            ),
            other => return Err(format!("unsupported output sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        streams.push(stream);
    } else {
        tracing::warn!("no output device available");
    }
    Ok(streams)
}

/// Appends mixed audio to the playback ring, dropping the oldest audio if the
/// device stalled (keeps latency bounded).
pub fn queue_playback(ring: &PlaybackRing, frame: &[f32]) {
    let mut r = ring.lock();
    r.extend(frame.iter().copied());
    while r.len() > MAX_PLAYBACK_BUFFER {
        r.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * freq * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * amp).collect()
    }

    #[test]
    fn resampler_ratio_and_continuity() {
        let mut r = Resampler::new(44_100, 48_000, 1);
        let mut out = vec![];
        for _ in 0..10 {
            r.process(&vec![0.5; 441], &mut out);
        }
        let expected = 4410.0 * 48_000.0 / 44_100.0;
        assert!((out.len() as f64 - expected).abs() <= 2.0, "{} vs {}", out.len(), expected);
        assert!(out.iter().skip(2).all(|s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn vad_hangover() {
        let mut v = Vad::new(-40.0);
        assert!(v.is_speech(&sine(440.0, FRAME, 0.5)));
        let quiet = vec![0.0; FRAME];
        for _ in 0..15 {
            assert!(v.is_speech(&quiet));
        }
        assert!(!v.is_speech(&quiet));
    }

    #[test]
    fn opus_roundtrip_through_mixer() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let mut mixer = Mixer::default();
        let tone = sine(440.0, FRAME * 10, 0.4);
        for (i, chunk) in tone.chunks(FRAME).enumerate() {
            mixer.push(99, i as u16, enc.encode(chunk).unwrap());
        }
        let mut out = vec![0.0; FRAME * 2];
        let mut energy = 0.0;
        let mut active_frames = 0;
        for _ in 0..8 {
            if !mixer.mix(&mut out, |_| 1.0).is_empty() {
                active_frames += 1;
            }
            energy += out.iter().map(|s| s * s).sum::<f32>();
        }
        assert!(active_frames >= 5, "speaker should be reported active");
        assert!(energy > 10.0, "decoded audio should carry the tone");
    }

    #[test]
    fn jitter_buffer_reorders_and_conceals() {
        let mut enc = OpusEncoder::new(48_000).unwrap();
        let mut s = SpeakerStream::new().unwrap();
        let frames: Vec<Vec<u8>> = sine(300.0, FRAME * 6, 0.3).chunks(FRAME).map(|c| enc.encode(c).unwrap()).collect();
        // Deliver out of order with packet 2 missing.
        for i in [1usize, 0, 3, 4, 5] {
            s.push((i as u16).wrapping_add(65533), frames[i].clone()); // also crosses the u16 wrap
        }
        let mut out = vec![0.0; FRAME * 2];
        let mut produced = 0;
        for _ in 0..6 {
            if s.pull(&mut out) {
                produced += 1;
            }
        }
        assert_eq!(produced, 6, "5 real frames + 1 concealed");
    }

    #[test]
    fn muted_speaker_is_silent_but_active() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let mut mixer = Mixer::default();
        for (i, c) in sine(440.0, FRAME * 5, 0.5).chunks(FRAME).enumerate() {
            mixer.push(1, i as u16, enc.encode(c).unwrap());
        }
        let mut out = vec![0.0; FRAME * 2];
        mixer.mix(&mut out, |_| 0.0);
        assert!(out.iter().all(|s| *s == 0.0));
    }
}
