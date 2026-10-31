//! Not a pass/fail test: logs 1280×720 conversion/encode/decode timings so we know the pipeline is real-time.
//!
//! Meaningful numbers need an optimised build:
//! `cargo test -p ferrocord-media --release --test bench_720p -- --nocapture`
#![cfg(feature = "h264")]

use std::time::{Duration, Instant};

use ferrocord_media::h264::{EncoderConfig, H264Decoder, H264Encoder};
use ferrocord_media::rtp_h264::{DepacketizerEvent, H264Depacketizer, H264Packetizer};
use ferrocord_media::{I420Frame, VideoFrame, convert};

#[derive(Clone, Copy, Debug)]
enum Content {
    /// Smooth moving gradients, roughly what a webcam produces.
    CameraLike,
    /// High-frequency XOR texture in motion: worst case for the encoder.
    WorstCase,
}

fn frame(content: Content, t: u32, base: Instant) -> VideoFrame {
    let (w, h) = (1280u32, 720u32);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            match content {
                Content::CameraLike => {
                    let fx = (x as f32 + 3.0 * t as f32) / 53.0;
                    let fy = (y as f32 + 2.0 * t as f32) / 37.0;
                    data.extend_from_slice(&[(128.0 + 90.0 * fx.sin()) as u8, (128.0 + 90.0 * fy.cos()) as u8, ((x + y) * 255 / (w + h)) as u8, 255]);
                }
                Content::WorstCase => data.extend_from_slice(&[((x + 4 * t) & 0xff) as u8, ((y + 2 * t) & 0xff) as u8, ((x ^ y) & 0xff) as u8, 255]),
            }
        }
    }
    VideoFrame::with_timestamp(w, h, data, base + Duration::from_millis(33 * t as u64)).unwrap()
}

fn run(content: Content) {
    let frames = if cfg!(debug_assertions) { 6 } else { 60 };
    let base = Instant::now();
    let inputs: Vec<VideoFrame> = (0..frames).map(|t| frame(content, t, base)).collect();
    let mut encoder = H264Encoder::new(EncoderConfig::default()).unwrap();
    let mut decoder = H264Decoder::new().unwrap();
    let packetizer = H264Packetizer::default();
    let mut depacketizer = H264Depacketizer::new();
    let (mut t_conv, mut t_enc, mut t_rtp, mut t_dec) = (Duration::ZERO, Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let (mut bytes, mut seq, mut decoded) = (0usize, 0u16, 0usize);
    let mut yuv = I420Frame::black(1, 1).unwrap();
    let mut decode = |events: Vec<DepacketizerEvent>, t_dec: &mut Duration| {
        let start = Instant::now();
        for event in events {
            if let DepacketizerEvent::Frame(au) = event
                && decoder.decode(&au.data).unwrap().is_some()
            {
                decoded += 1;
            }
        }
        *t_dec += start.elapsed();
    };
    for (i, input) in inputs.iter().enumerate() {
        let start = Instant::now();
        convert::rgba_to_i420(&input.data, input.width, input.height, &mut yuv).unwrap();
        t_conv += start.elapsed();

        let start = Instant::now();
        let Some(out) = encoder.encode(input).unwrap() else { continue };
        t_enc += start.elapsed();
        bytes += out.data.len();

        let start = Instant::now();
        let payloads = packetizer.packetize(&out.data);
        let last = payloads.len() - 1;
        let mut events = Vec::new();
        for (k, p) in payloads.iter().enumerate() {
            events.extend(depacketizer.push(seq, i as u32 * 3000, k == last, p));
            seq = seq.wrapping_add(1);
        }
        t_rtp += start.elapsed();
        decode(events, &mut t_dec);
    }
    decode(depacketizer.flush(), &mut t_dec);
    let per = |d: Duration| d.as_secs_f64() * 1000.0 / frames as f64;
    eprintln!(
        "1280x720 {content:?} x{frames} ({}): rgba->i420 {:.2} ms, encode (incl. conversion) {:.2} ms, packetize+depacketize {:.3} ms, decode (incl. ->rgba) {:.2} ms per frame; {:.0} kbit/s at 30 fps",
        if cfg!(debug_assertions) { "debug build, not representative" } else { "release" },
        per(t_conv),
        per(t_enc),
        per(t_rtp),
        per(t_dec),
        bytes as f64 * 8.0 * 30.0 / frames as f64 / 1000.0,
    );
    let start = Instant::now();
    for input in &inputs {
        let _ = input.thumbnail(320, 180).unwrap();
    }
    eprintln!("thumbnail 1280x720 -> 320x180: {:.2} ms per frame", per(start.elapsed()));
    assert_eq!(decoded, frames as usize);
}

#[test]
fn bench_720p_pipeline() {
    run(Content::CameraLike);
    run(Content::WorstCase);
}
