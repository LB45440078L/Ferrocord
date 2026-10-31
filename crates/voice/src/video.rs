//! Camera video for calls.
//!
//! Send: camera (or injected frames) → H.264 → DAVE (codec-aware) → RFC 6184
//! packets → RTP (PT 101, 90 kHz) → transport AEAD → UDP. Incoming RTCP PLIs
//! force a keyframe.
//! Receive: RTP → depacketize → DAVE decrypt → H.264 decode → latest frame per
//! user in [`crate::Shared`]. Losses trigger a PLI so the sender refreshes.
//!
//! Both directions run on dedicated threads: encoding/decoding takes several
//! milliseconds per frame and must never stall audio.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrocord_media::VideoFrame;
use ferrocord_media::h264::{EncoderConfig, H264Decoder, H264Encoder};
use ferrocord_media::rtp_h264::{DepacketizerEvent, H264Depacketizer, H264Packetizer};
use parking_lot::Mutex;
use tokio::net::UdpSocket;

use crate::connection::{Media, Shared};
use crate::protocol;
use crate::rtp::{RtpHeader, pli};

/// Where outgoing video comes from.
pub enum VideoSource {
    /// A webcam by index (see [`ferrocord_media::camera::list_cameras`]).
    Camera(u32),
    /// Externally supplied frames (tests, screen-share-like sources).
    Custom(tokio::sync::mpsc::Receiver<VideoFrame>),
}

pub const SEND_WIDTH: u32 = 1280;
pub const SEND_HEIGHT: u32 = 720;
pub const SEND_FPS: u32 = 30;
pub const SEND_BITRATE: u32 = 2_500_000;
/// RTP payload budget per packet (leaves room for RTP + AEAD overhead under a 1200-byte MTU).
const MAX_PAYLOAD: usize = 1100;

/// Stream descriptor we announce in op 12 when the camera is on/off.
pub fn stream_descriptor(video_ssrc: u32, rtx_ssrc: u32, active: bool) -> protocol::StreamDescriptor {
    protocol::StreamDescriptor {
        kind: "video".into(),
        rid: "100".into(),
        ssrc: Some(video_ssrc),
        rtx_ssrc: Some(rtx_ssrc),
        active: Some(active),
        quality: Some(100),
        max_bitrate: Some(SEND_BITRATE),
        max_framerate: Some(SEND_FPS),
        max_resolution: Some(protocol::MaxResolution { kind: "fixed".into(), width: SEND_WIDTH, height: SEND_HEIGHT }),
    }
}

/// Handle to the running send pipeline. Dropping it stops the camera.
pub struct VideoSender {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for VideoSender {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Sends a datagram from a non-async thread, retrying briefly if the socket buffer is full.
fn send_blocking(sock: &UdpSocket, pkt: &[u8]) {
    for _ in 0..50 {
        match sock.try_send(pkt) {
            Ok(_) => return,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(1)),
            Err(_) => return,
        }
    }
}

impl VideoSender {
    pub(crate) fn start(source: VideoSource, sock: Arc<UdpSocket>, media: Arc<Mutex<Media>>, shared: Arc<Shared>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        // Open the source on the calling thread so errors (no camera) are reported immediately.
        enum Src {
            Camera(ferrocord_media::camera::CameraHandle),
            Custom(tokio::sync::mpsc::Receiver<VideoFrame>),
        }
        let src = match source {
            VideoSource::Camera(i) => Src::Camera(
                ferrocord_media::camera::Camera::start(i, ferrocord_media::camera::CaptureConfig { width: SEND_WIDTH, height: SEND_HEIGHT, fps: SEND_FPS })
                    .map_err(|e| e.to_string())?,
            ),
            VideoSource::Custom(rx) => Src::Custom(rx),
        };
        let mut encoder =
            H264Encoder::new(EncoderConfig { width: SEND_WIDTH, height: SEND_HEIGHT, fps: SEND_FPS as f32, bitrate_bps: SEND_BITRATE, ..Default::default() })
                .map_err(|e| e.to_string())?;
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ferrocord-video-tx".into())
            .spawn(move || {
                let mut src = src;
                let packetizer = H264Packetizer::new(MAX_PAYLOAD);
                let start = Instant::now();
                let ts_base: u32 = rand::random();
                let mut seq: u16 = rand::random();
                let mut last_frame: Option<Instant> = None;
                let frame_interval = Duration::from_millis(1000 / SEND_FPS as u64);
                while !stop2.load(Ordering::SeqCst) {
                    let frame = match &mut src {
                        Src::Camera(cam) => {
                            std::thread::sleep(frame_interval / 2);
                            match cam.latest() {
                                Some(f) if last_frame.is_none_or(|t| f.timestamp > t) => f,
                                _ => continue,
                            }
                        }
                        Src::Custom(rx) => match rx.try_recv() {
                            Ok(f) => f,
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                                std::thread::sleep(Duration::from_millis(5));
                                continue;
                            }
                            Err(_) => break,
                        },
                    };
                    last_frame = Some(frame.timestamp);
                    *shared.local_video.write() = Some(frame.clone());
                    if shared.keyframe_requested.swap(false, Ordering::SeqCst) {
                        encoder.force_keyframe();
                    }
                    let encoded = match encoder.encode(&frame) {
                        Ok(Some(e)) => e,
                        Ok(None) => continue,
                        Err(e) => {
                            tracing::warn!(error = %e, "video encode failed");
                            continue;
                        }
                    };
                    let ts = ts_base.wrapping_add((frame.timestamp.saturating_duration_since(start).as_micros() * 9 / 100) as u32);
                    let (e2ee, ssrc) = {
                        let mut m = media.lock();
                        (m.dave.encrypt(davey::MediaType::VIDEO, davey::Codec::H264, &encoded.data), m.video_ssrc)
                    };
                    let (Some(e2ee), true) = (e2ee, ssrc != 0) else { continue };
                    let payloads = packetizer.packetize(&e2ee);
                    let n = payloads.len();
                    for (i, p) in payloads.into_iter().enumerate() {
                        let mut pkt = Vec::with_capacity(p.len() + 40);
                        RtpHeader::write(i + 1 == n, protocol::H264_PAYLOAD_TYPE, seq, ts, ssrc, &mut pkt);
                        pkt.extend_from_slice(&p);
                        seq = seq.wrapping_add(1);
                        let ok = media.lock().cipher.as_mut().is_some_and(|c| c.encrypt_rtp(&mut pkt).is_ok());
                        if ok {
                            send_blocking(&sock, &pkt);
                        }
                    }
                }
                *shared.local_video.write() = None;
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

/// One received video RTP packet, already transport-decrypted.
pub(crate) struct VideoPacket {
    pub ssrc: u32,
    pub user: u64,
    pub seq: u16,
    pub timestamp: u32,
    pub marker: bool,
    pub payload: Vec<u8>,
}

/// Starts the receive/decode thread; feed it with the returned sender.
pub(crate) fn start_receiver(sock: Arc<UdpSocket>, media: Arc<Mutex<Media>>, shared: Arc<Shared>) -> mpsc::Sender<VideoPacket> {
    let (tx, rx) = mpsc::channel::<VideoPacket>();
    let _ = std::thread::Builder::new().name("ferrocord-video-rx".into()).spawn(move || {
        struct Stream {
            depack: H264Depacketizer,
            decoder: Option<H264Decoder>,
            last_pli: Option<Instant>,
        }
        let mut streams: HashMap<u32, Stream> = HashMap::new();
        loop {
            let pkt = match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(p) => p,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Bound latency if a marker packet was lost.
                    for s in streams.values_mut() {
                        let _ = s.depack.flush();
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let st = streams.entry(pkt.ssrc).or_insert_with(|| Stream { depack: H264Depacketizer::new(), decoder: H264Decoder::new().ok(), last_pli: None });
            let mut need_keyframe = false;
            for ev in st.depack.push(pkt.seq, pkt.timestamp, pkt.marker, &pkt.payload) {
                need_keyframe |= ev.needs_keyframe();
                let DepacketizerEvent::Frame(au) = ev else { continue };
                let (plain, recovery) = media.lock().dave.decrypt(pkt.user, davey::MediaType::VIDEO, &au.data);
                if !recovery.is_empty() {
                    tracing::debug!("DAVE recovery requested from video path");
                }
                let Some(plain) = plain else { continue };
                match st.decoder.as_mut().map(|d| d.decode(&plain)) {
                    Some(Ok(Some(frame))) => {
                        // Late frames after the sender turned the camera off must not resurrect its tile.
                        if shared.video_ssrc_user.read().get(&pkt.ssrc) == Some(&pkt.user) {
                            shared.remote_video.write().insert(pkt.user, frame);
                        }
                    }
                    Some(Err(_)) => need_keyframe = true,
                    _ => {}
                }
            }
            if need_keyframe && st.last_pli.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
                st.last_pli = Some(Instant::now());
                let mut m = media.lock();
                let mut p = pli(m.ssrc, pkt.ssrc);
                if m.cipher.as_mut().is_some_and(|c| c.encrypt_rtcp(&mut p).is_ok()) {
                    drop(m);
                    send_blocking(&sock, &p);
                }
            }
        }
    });
    tx
}
