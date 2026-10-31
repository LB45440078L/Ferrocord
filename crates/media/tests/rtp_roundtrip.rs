//! Property tests: the packetizer/depacketizer pair must reproduce arbitrary (DAVE-transformed) Annex-B
//! streams byte for byte, under reordering and duplication, and must never panic on hostile input.

use ferrocord_media::rtp_h264::{DepacketizerConfig, DepacketizerEvent, H264Depacketizer, H264Packetizer, START_CODE, nal_units};
use proptest::prelude::*;

/// Inserts emulation-prevention bytes so that `00 00 01` never occurs, like an encoder (or DAVE's
/// ciphertext validation) guarantees.
fn escape(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    let mut zeros = 0;
    for &b in body {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Raw NAL body bytes, biased towards zeros and ones to provoke start-code edge cases.
fn body(max: usize) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop_oneof![3 => Just(0u8), 1 => Just(1u8), 6 => any::<u8>()], 0..max)
}

/// A NAL unit: non-zero header byte (any type, any F/NRI bits) + escaped body (may end in zero bytes).
fn nal() -> impl Strategy<Value = Vec<u8>> {
    (1u8..=255, body(2500)).prop_map(|(header, body)| std::iter::once(header).chain(escape(&body)).collect())
}

/// An access unit: several NAL units with 4-byte start codes, the last one followed by an arbitrary trailer
/// (DAVE appends its unencrypted supplemental data there; like DAVE, the trailer contains no start code).
fn access_unit() -> impl Strategy<Value = Vec<u8>> {
    (prop::collection::vec((1u8..=255, body(2500)), 1..8), body(64)).prop_map(|(nals, trailer)| {
        let mut au = Vec::new();
        let count = nals.len();
        for (i, (header, mut body)) in nals.into_iter().enumerate() {
            if i + 1 == count {
                body.extend_from_slice(&trailer);
            }
            au.extend_from_slice(&START_CODE);
            au.push(header);
            au.extend(escape(&body));
        }
        au
    })
}

/// Deterministically permutes `items` within windows of `window` elements, using `seed`.
fn shuffle_windows<T>(items: &mut [T], window: usize, mut seed: u64) {
    for chunk in items.chunks_mut(window.max(1)) {
        for i in (1..chunk.len()).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = (seed >> 33) as usize % (i + 1);
            chunk.swap(i, j);
        }
    }
}

fn no_wait() -> DepacketizerConfig {
    DepacketizerConfig { wait_for_keyframe: false, ..DepacketizerConfig::default() }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn single_frame_round_trip_is_byte_exact(au in access_unit(), mtu in 8usize..1500, aggregate in any::<bool>(), seq0 in any::<u16>(), ts in any::<u32>(), window in 1usize..16, seed in any::<u64>()) {
        let payloads = H264Packetizer::new(mtu).with_aggregation(aggregate).packetize(&au);
        prop_assert!(!payloads.is_empty());
        for p in &payloads {
            prop_assert!(p.len() <= mtu.max(8));
            prop_assert!(!p.is_empty());
        }
        let last = payloads.len() - 1;
        let mut packets: Vec<(u16, bool, &Vec<u8>)> =
            payloads.iter().enumerate().map(|(i, p)| (seq0.wrapping_add(i as u16), i == last, p)).collect();
        shuffle_windows(&mut packets, window, seed);

        let mut depacketizer = H264Depacketizer::with_config(no_wait());
        let mut events = Vec::new();
        for (i, (seq, marker, payload)) in packets.iter().enumerate() {
            events.extend(depacketizer.push(*seq, ts, *marker, payload));
            if i % 5 == 0 {
                events.extend(depacketizer.push(*seq, ts, *marker, payload)); // duplicate
            }
        }
        events.extend(depacketizer.flush());
        prop_assert_eq!(events.len(), 1, "events: {:?}", events);
        match &events[0] {
            DepacketizerEvent::Frame(frame) => {
                prop_assert_eq!(frame.timestamp, ts);
                prop_assert!(frame.data == au, "stream differs (len {} vs {})", frame.data.len(), au.len());
            }
            other => prop_assert!(false, "unexpected {:?}", other),
        }
    }

    #[test]
    fn multi_frame_stream_round_trip(aus in prop::collection::vec(access_unit(), 1..6), mtu in 64usize..1400, seed in any::<u64>()) {
        let packetizer = H264Packetizer::new(mtu);
        let mut packets = Vec::new();
        let mut seq = 65530u16;
        for (i, au) in aus.iter().enumerate() {
            let payloads = packetizer.packetize(au);
            let last = payloads.len() - 1;
            for (k, p) in payloads.into_iter().enumerate() {
                packets.push((seq, i as u32 * 3000, k == last, p));
                seq = seq.wrapping_add(1);
            }
        }
        shuffle_windows(&mut packets, 2, seed);
        let mut depacketizer = H264Depacketizer::with_config(no_wait());
        let mut frames = Vec::new();
        for (seq, ts, marker, payload) in &packets {
            frames.extend(depacketizer.push(*seq, *ts, *marker, payload));
        }
        frames.extend(depacketizer.flush());
        let frames: Vec<_> = frames.into_iter().map(|e| match e {
            DepacketizerEvent::Frame(f) => Ok(f),
            other => Err(other),
        }).collect::<Result<_, _>>().map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
        prop_assert_eq!(frames.len(), aus.len());
        for (i, (frame, au)) in frames.iter().zip(&aus).enumerate() {
            prop_assert_eq!(frame.timestamp, i as u32 * 3000);
            prop_assert!(&frame.data == au);
        }
    }

    #[test]
    fn three_byte_start_codes_are_normalised(nals in prop::collection::vec(nal(), 1..6)) {
        // Real NAL units end with the RBSP stop bit, i.e. a non-zero byte. A zero byte right before a 3-byte
        // start code is indistinguishable from a 4-byte start code, so give every NAL a non-zero tail here.
        let nals: Vec<Vec<u8>> = nals.into_iter().map(|mut n| { if n.last() == Some(&0) { n.push(0x80); } n }).collect();
        let short: Vec<u8> = nals.iter().flat_map(|n| [0, 0, 1].into_iter().chain(n.iter().copied())).collect();
        let long: Vec<u8> = nals.iter().flat_map(|n| START_CODE.into_iter().chain(n.iter().copied())).collect();
        let split: Vec<&[u8]> = nal_units(&short).collect();
        prop_assert_eq!(split.len(), nals.len());
        let payloads = H264Packetizer::new(500).packetize(&short);
        let mut depacketizer = H264Depacketizer::with_config(no_wait());
        let mut events = Vec::new();
        for (i, p) in payloads.iter().enumerate() {
            events.extend(depacketizer.push(i as u16, 0, i + 1 == payloads.len(), p));
        }
        events.extend(depacketizer.flush());
        prop_assert_eq!(events, vec![DepacketizerEvent::Frame(ferrocord_media::rtp_h264::AccessUnit { timestamp: 0, data: long, keyframe: nals.iter().any(|n| n[0] & 0x1f == 5) })]);
    }

    #[test]
    fn depacketizer_never_panics(packets in prop::collection::vec((any::<u16>(), 0u32..8, any::<bool>(), prop::collection::vec(any::<u8>(), 0..64)), 0..200), small_seq in any::<bool>()) {
        let mut depacketizer = H264Depacketizer::new();
        for (seq, ts, marker, payload) in &packets {
            let seq = if small_seq { seq % 40 } else { *seq };
            for event in depacketizer.push(seq, *ts, *marker, payload) {
                if let DepacketizerEvent::Frame(frame) = event {
                    prop_assert!(frame.data.starts_with(&START_CODE));
                }
            }
        }
        depacketizer.flush();
        depacketizer.reset();
        depacketizer.push(1, 1, true, &[0x65, 0x88]);
    }

    #[test]
    fn packetizer_never_panics(data in prop::collection::vec(prop_oneof![2 => Just(0u8), 1 => Just(1u8), 4 => any::<u8>()], 0..3000), mtu in 0usize..300) {
        for p in H264Packetizer::new(mtu).packetize(&data) {
            prop_assert!(p.len() <= mtu.max(8) && !p.is_empty());
        }
    }
}
