//! End-to-end: encoder → packetizer → (reordered / lossy) delivery → depacketizer → decoder.
#![cfg(feature = "h264")]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ferrocord_media::VideoFrame;
use ferrocord_media::h264::{EncodedFrame, EncoderConfig, H264Decoder, H264Encoder};
use ferrocord_media::rtp_h264::{DepacketizerEvent, DropReason, H264Depacketizer, H264Packetizer, nal_units};

const W: u32 = 320;
const H: u32 = 240;

/// Smooth moving pattern so the encoder reaches a decent PSNR at a modest bitrate.
fn synthetic(w: u32, h: u32, t: u32, base: Instant) -> VideoFrame {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let fx = (x as f32 + 3.0 * t as f32) / 23.0;
            let fy = (y as f32 + 2.0 * t as f32) / 17.0;
            data.extend_from_slice(&[(128.0 + 90.0 * fx.sin()) as u8, (128.0 + 90.0 * fy.cos()) as u8, ((x + y) * 255 / (w + h)) as u8, 255]);
        }
    }
    VideoFrame::with_timestamp(w, h, data, base + Duration::from_millis(33 * t as u64)).unwrap()
}

fn psnr(a: &VideoFrame, b: &VideoFrame) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut sum = 0f64;
    let mut n = 0f64;
    for (pa, pb) in a.data.as_chunks::<4>().0.iter().zip(b.data.as_chunks::<4>().0) {
        for c in 0..3 {
            let d = pa[c] as f64 - pb[c] as f64;
            sum += d * d;
            n += 1.0;
        }
    }
    let mse = sum / n;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

struct Packet {
    seq: u16,
    ts: u32,
    marker: bool,
    payload: Vec<u8>,
}

/// Encodes `count` frames, forcing a keyframe before every index in `keyframes_at`.
fn encode_sequence(count: u32, keyframes_at: &[u32]) -> (Vec<VideoFrame>, Vec<EncodedFrame>) {
    let base = Instant::now();
    let mut encoder =
        H264Encoder::new(EncoderConfig { width: W, height: H, fps: 30.0, bitrate_bps: 1_500_000, keyframe_interval: 0, max_slice_bytes: 0 }).unwrap();
    let mut sources = Vec::new();
    let mut encoded = Vec::new();
    for t in 0..count {
        if keyframes_at.contains(&t) {
            encoder.force_keyframe();
        }
        let frame = synthetic(W, H, t, base);
        let out = encoder.encode(&frame).unwrap().expect("encoder skipped a frame");
        sources.push(frame);
        encoded.push(out);
    }
    (sources, encoded)
}

fn packetize(encoded: &[EncodedFrame], mtu: usize, seq0: u16) -> Vec<Packet> {
    let packetizer = H264Packetizer::new(mtu);
    let mut seq = seq0;
    let mut packets = Vec::new();
    for (i, frame) in encoded.iter().enumerate() {
        let payloads = packetizer.packetize(&frame.data);
        let last = payloads.len() - 1;
        for (k, payload) in payloads.into_iter().enumerate() {
            assert!(payload.len() <= mtu);
            packets.push(Packet { seq, ts: 1000 + i as u32 * 3000, marker: k == last, payload });
            seq = seq.wrapping_add(1);
        }
    }
    packets
}

#[test]
fn sps_is_constrained_baseline_level_3_1() {
    let (_, encoded) = encode_sequence(1, &[]);
    assert!(encoded[0].keyframe);
    let sps = nal_units(&encoded[0].data).find(|n| n[0] & 0x1f == 7).expect("keyframe without SPS");
    assert!(nal_units(&encoded[0].data).any(|n| n[0] & 0x1f == 8), "keyframe without PPS");
    assert_eq!(sps[1], 0x42, "profile_idc must be baseline");
    assert_ne!(sps[2] & 0x40, 0, "constraint_set1_flag (constrained baseline) must be set, got {:#04x}", sps[2]);
    assert_eq!(sps[3], 0x1f, "level 3.1 expected for 320x240");
    eprintln!("profile-level-id = {:02x}{:02x}{:02x}", sps[1], sps[2], sps[3]);
}

#[test]
fn round_trip_with_reordering_and_duplicates() {
    let (sources, encoded) = encode_sequence(30, &[]);
    assert!(encoded[0].keyframe);
    let mut packets = packetize(&encoded, 300, 65500); // sequence numbers wrap mid-stream
    assert!(packets.iter().any(|p| p.payload[0] & 0x1f == 28), "expected FU-A fragments");
    assert!(packets.iter().any(|p| p.payload[0] & 0x1f == 24), "expected a STAP-A for SPS/PPS");

    // Deterministic shuffle: reverse every group of three packets, and duplicate every 7th one.
    for chunk in packets.chunks_mut(3) {
        chunk.reverse();
    }
    let mut delivery: Vec<&Packet> = Vec::new();
    for (i, p) in packets.iter().enumerate() {
        delivery.push(p);
        if i % 7 == 0 {
            delivery.push(p);
        }
    }

    let by_ts: HashMap<u32, usize> = (0..encoded.len()).map(|i| (1000 + i as u32 * 3000, i)).collect();
    let mut depacketizer = H264Depacketizer::new();
    let mut decoder = H264Decoder::new().unwrap();
    let mut events = Vec::new();
    for p in delivery {
        events.extend(depacketizer.push(p.seq, p.ts, p.marker, &p.payload));
    }
    events.extend(depacketizer.flush());

    let mut decoded = 0;
    for event in events {
        let DepacketizerEvent::Frame(au) = event else { panic!("unexpected event {event:?}") };
        let index = by_ts[&au.timestamp];
        assert_eq!(au.data, encoded[index].data, "frame {index} not byte-exact");
        assert_eq!(au.keyframe, encoded[index].keyframe);
        let picture = decoder.decode(&au.data).unwrap().expect("no picture");
        assert_eq!((picture.width, picture.height), (W, H));
        let quality = psnr(&picture, &sources[index]);
        assert!(quality > 30.0, "frame {index}: PSNR {quality:.1} dB");
        decoded += 1;
    }
    assert_eq!(decoded, encoded.len());
    let stats = depacketizer.stats();
    assert_eq!(stats.packets_lost, 0);
    assert!(stats.packets_duplicate + stats.packets_late > 0);
}

#[test]
fn loss_inside_fu_a_drops_frame_until_next_keyframe() {
    let (sources, encoded) = encode_sequence(16, &[10]);
    assert!(encoded[10].keyframe);
    let packets = packetize(&encoded, 120, 7);
    let lost_ts = 1000 + 3 * 3000;
    let middle =
        packets.iter().position(|p| p.ts == lost_ts && p.payload[0] & 0x1f == 28 && p.payload[1] & 0xc0 == 0).expect("frame 3 has no middle FU-A fragment");

    let mut depacketizer = H264Depacketizer::new();
    let mut decoder = H264Decoder::new().unwrap();
    let mut events = Vec::new();
    for (i, p) in packets.iter().enumerate() {
        if i != middle {
            events.extend(depacketizer.push(p.seq, p.ts, p.marker, &p.payload));
        }
    }
    events.extend(depacketizer.flush());

    let lost = events.iter().find_map(|e| match e {
        DepacketizerEvent::PacketsLost { first_seq, count } => Some((*first_seq, *count)),
        _ => None,
    });
    assert_eq!(lost, Some((packets[middle].seq, 1)));
    assert!(events.contains(&DepacketizerEvent::FrameDropped { timestamp: lost_ts, reason: DropReason::PacketLoss }));
    for t in 4..10 {
        assert!(events.contains(&DepacketizerEvent::FrameDropped { timestamp: 1000 + t * 3000, reason: DropReason::WaitingForKeyframe }), "frame {t}");
    }

    let mut decoded = Vec::new();
    for event in &events {
        if let DepacketizerEvent::Frame(au) = event {
            let index = ((au.timestamp - 1000) / 3000) as usize;
            let picture = decoder.decode(&au.data).unwrap().expect("no picture");
            let quality = psnr(&picture, &sources[index]);
            assert!(quality > 30.0, "frame {index}: PSNR {quality:.1} dB");
            decoded.push(index);
        }
    }
    assert_eq!(decoded, vec![0, 1, 2, 10, 11, 12, 13, 14, 15]);
    assert!(!depacketizer.is_waiting_for_keyframe());
}

#[test]
fn encoder_scales_mismatched_frames_and_handles_runtime_changes() {
    let base = Instant::now();
    let mut encoder = H264Encoder::new(EncoderConfig { width: 161, height: 121, ..EncoderConfig::default() }).unwrap();
    assert_eq!((encoder.config().width, encoder.config().height), (160, 120));
    encoder.set_bitrate(300_000).unwrap(); // before the first frame
    let mut decoder = H264Decoder::new().unwrap();
    for t in 0..6 {
        let frame = synthetic(if t % 2 == 0 { 320 } else { 100 }, if t % 2 == 0 { 180 } else { 77 }, t, base);
        if t == 3 {
            encoder.set_bitrate(500_000).unwrap(); // mid-stream increase
        }
        if t == 4 {
            encoder.set_bitrate(150_000).unwrap(); // mid-stream decrease
        }
        if let Some(out) = encoder.encode(&frame).unwrap() {
            assert_eq!((out.width, out.height), (160, 120));
            let picture = decoder.decode(&out.data).unwrap().expect("no picture");
            assert_eq!((picture.width, picture.height), (160, 120));
        }
    }
    encoder.set_resolution(64, 48).unwrap();
    let out = encoder.encode(&synthetic(64, 48, 0, base)).unwrap().unwrap();
    assert!(out.keyframe);
    assert_eq!(decoder.decode(&out.data).unwrap().map(|p| (p.width, p.height)), Some((64, 48)));

    // Bad input is an error, not a panic.
    let bogus = VideoFrame { width: 10, height: 10, data: std::sync::Arc::new(vec![0; 3]), timestamp: base };
    assert!(encoder.encode(&bogus).is_err());
    assert!(H264Encoder::new(EncoderConfig { width: 8000, height: 8000, ..EncoderConfig::default() }).is_err());
}

#[test]
fn decoder_survives_garbage() {
    let (_, encoded) = encode_sequence(3, &[]);
    let key = &encoded[0].data;
    let mut decoder = H264Decoder::new().unwrap();
    let _ = decoder.decode(&[]);
    let _ = decoder.decode(&[0, 0, 0, 1]);
    let _ = decoder.decode(&[0, 0, 0, 1, 0x65]);
    for cut in [5, key.len() / 3, key.len() / 2, key.len() - 1] {
        let _ = decoder.decode(&key[..cut]);
    }
    // Deterministic pseudo-random bit flips in the slice data.
    let mut state = 0x2545_f491_u32;
    for _ in 0..40 {
        let mut damaged = key.clone();
        for _ in 0..8 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let pos = 40 + state as usize % (damaged.len() - 40);
            damaged[pos] ^= 1 << (state % 8);
        }
        let _ = decoder.decode(&damaged);
    }
    let mut noise = vec![0u8; 4096];
    for b in noise.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *b = state as u8;
    }
    let _ = decoder.decode(&noise);
    // A fresh decoder still decodes the intact stream after all that.
    let mut decoder = H264Decoder::new().unwrap();
    assert!(decoder.decode(key).unwrap().is_some());
}

#[test]
fn sliced_encoding_round_trips_as_single_nal_packets() {
    let base = Instant::now();
    let mut encoder = H264Encoder::new(EncoderConfig { width: W, height: H, max_slice_bytes: 400, ..EncoderConfig::default() }).unwrap();
    let packetizer = H264Packetizer::new(500);
    let mut depacketizer = H264Depacketizer::new();
    let mut decoder = H264Decoder::new().unwrap();
    let mut events = Vec::new();
    let mut seq = 0u16;
    let mut sources = Vec::new();
    for t in 0..10u32 {
        let frame = synthetic(W, H, t, base);
        let out = encoder.encode(&frame).unwrap().expect("skipped");
        let slices = nal_units(&out.data).filter(|n| matches!(n[0] & 0x1f, 1 | 5)).count();
        assert!(slices > 1, "frame {t}: expected several slices, got {slices}");
        let payloads = packetizer.packetize(&out.data);
        assert!(payloads.iter().all(|p| p[0] & 0x1f != 28), "slices should not need FU-A");
        let last = payloads.len() - 1;
        for (k, p) in payloads.iter().enumerate() {
            events.extend(depacketizer.push(seq, t * 3000, k == last, p));
            seq = seq.wrapping_add(1);
        }
        sources.push((frame, out));
    }
    events.extend(depacketizer.flush());
    assert_eq!(events.len(), 10);
    for event in events {
        let DepacketizerEvent::Frame(au) = event else { panic!("unexpected {event:?}") };
        let (source, encoded) = &sources[(au.timestamp / 3000) as usize];
        assert_eq!(&au.data, &encoded.data);
        let picture = decoder.decode(&au.data).unwrap().expect("no picture");
        assert!(psnr(&picture, source) > 30.0);
    }
}

#[test]
fn high_bitrate_1080p_keyframes_with_many_slices() {
    let base = Instant::now();
    let config = EncoderConfig { width: 1920, height: 1080, bitrate_bps: 12_000_000, keyframe_interval: 2, ..EncoderConfig::default() };
    let mut encoder = H264Encoder::new(config).unwrap();
    let mut decoder = H264Decoder::new().unwrap();
    for t in 0..4u32 {
        // High-frequency noise-like texture: worst case for slice count.
        let data: Vec<u8> = (0..1920 * 1080u32)
            .flat_map(|i| [(i.wrapping_mul(2_654_435_761).wrapping_add(t * 97) >> 13) as u8, (i ^ (i >> 7)) as u8, (i >> 3) as u8, 255])
            .collect();
        let frame = VideoFrame::with_timestamp(1920, 1080, data, base + Duration::from_millis(33 * t as u64)).unwrap();
        let Some(out) = encoder.encode(&frame).unwrap() else { continue };
        let slices = nal_units(&out.data).filter(|n| matches!(n[0] & 0x1f, 1 | 5)).count();
        assert!(nal_units(&out.data).all(|n| n.len() <= 1100), "slice over the limit");
        eprintln!("frame {t}: keyframe={} bytes={} slices={slices}", out.keyframe, out.data.len());
        assert_eq!(decoder.decode(&out.data).unwrap().map(|p| (p.width, p.height)), Some((1920, 1080)));
    }
}
