//! RFC 6184 H.264 RTP payload format (packetization-mode=1: single NAL unit, STAP-A, FU-A).
//!
//! This module only deals with RTP *payloads*. The caller owns the RTP header (SSRC, sequence numbers,
//! timestamps, extensions, SRTP): it numbers the payloads returned by [`H264Packetizer::packetize`]
//! consecutively, gives them all the frame's 90 kHz timestamp and sets the marker bit on the last one.
//! On receive it hands `(seq, timestamp, marker, payload)` to [`H264Depacketizer::push`].
//!
//! ## DAVE compatibility (byte exactness)
//!
//! Discord's DAVE end-to-end encryption transforms a whole H.264 access unit *before* packetization: NAL
//! headers stay in the clear, NAL bodies are encrypted, and an unencrypted trailer (auth tag, nonce, ranges,
//! magic marker) is appended after the last NAL. The receiver must hand DAVE exactly the bytes the sender's
//! DAVE produced. Therefore this module:
//!
//! * treats the input purely as an Annex-B byte stream split on start codes (`00 00 01` /
//!   `00 00 00 01`): it never parses, rewrites or strips anything inside a NAL unit (no emulation-prevention
//!   handling, no trailing-zero trimming), so the DAVE trailer simply travels as the tail of the last NAL;
//! * reassembles NAL units with 4-byte start codes (`00 00 00 01`), which is what OpenH264 and libdave emit
//!   and what browsers' depacketizers produce. A stream written with 4-byte start codes therefore
//!   round-trips **byte for byte** (see the property tests). Streams with 3-byte start codes come back
//!   with 4-byte ones (a `00` byte directly before a 3-byte start code is read as part of a 4-byte one),
//!   empty NAL units (two adjacent start codes) and leading zero bytes are dropped;
//! * sends NAL units whose type is 0 or 24..=31 (which would be mistaken for aggregation/fragmentation
//!   packets in single-NAL mode) inside STAP-A or FU-A, so even arbitrary header bytes survive.
//!
//! DAVE guarantees its ciphertext and trailer contain no start-code sequence, which is the only assumption
//! the splitter makes.
//!
//! ## Depacketizer design
//!
//! Packets go through a small reorder buffer keyed by the extended (wrap-around-corrected) sequence number.
//! In-order packets are fed to a frame assembler immediately. A hole is declared lost when either the
//! buffered span exceeds [`DepacketizerConfig::reorder_window`] packets or packets of more than
//! [`DepacketizerConfig::reorder_frames`] distinct timestamps are waiting behind it. This bounds both memory and
//! the latency a single lost packet can add, without needing a clock (deterministic, easy to test);
//! callers that want a time bound can call [`H264Depacketizer::flush`] from a timer.
//!
//! Frames are delimited by the marker bit, or by a timestamp change if a marker was lost. A frame touched by
//! loss (missing packets, unterminated FU-A) or carrying malformed payloads is dropped and reported via
//! [`DepacketizerEvent::FrameDropped`]; the depacketizer then (by default) discards delta frames until the
//! next IDR, so the decoder never sees a broken reference chain. Every non-`Frame` event means "send a PLI".

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

/// Default maximum RTP payload size: leaves room for IP/UDP/RTP headers, header extensions, SRTP auth tag
/// and the DAVE trailer within a 1280–1500 byte path MTU.
pub const DEFAULT_MAX_PAYLOAD_SIZE: usize = 1200;

/// Smallest payload size the packetizer accepts (smaller values are clamped).
pub const MIN_PAYLOAD_SIZE: usize = 8;

/// The start code inserted before every reassembled NAL unit.
pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

const NAL_TYPE_IDR: u8 = 5;
const NAL_TYPE_STAP_A: u8 = 24;
const NAL_TYPE_FU_A: u8 = 28;

/// Finds the next start code at or after `from`. Returns `(start_code_begin, payload_begin)`.
///
/// A zero byte directly before `00 00 01` is treated as part of a 4-byte start code (even if it was really a
/// trailing zero of the previous NAL: re-emitting a 4-byte start code reproduces the same bytes either way).
fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 3 <= data.len() {
        // Look at the third byte first: if it is > 1 no start code can begin at i, i+1 or i+2.
        let c = data[i + 2];
        if c > 1 {
            i += 3;
        } else if c == 1 && data[i] == 0 && data[i + 1] == 0 {
            let begin = if i > from && data[i - 1] == 0 { i - 1 } else { i };
            return Some((begin, i + 3));
        } else {
            i += 1;
        }
    }
    None
}

/// Iterator over the NAL units of an Annex-B byte stream (without their start codes).
///
/// Created by [`nal_units`]. Bytes before the first start code are yielded as a NAL unit unless they are all
/// zero (leading zero bytes); a buffer without any start code is yielded whole. Empty NAL units are skipped.
#[derive(Debug, Clone)]
pub struct NalUnits<'a> {
    data: &'a [u8],
    leading: Option<&'a [u8]>,
    pos: Option<usize>,
}

/// Splits an Annex-B byte stream into NAL units. See [`NalUnits`].
pub fn nal_units(data: &[u8]) -> NalUnits<'_> {
    let (leading, pos) = match find_start_code(data, 0) {
        Some((begin, payload)) => (Some(&data[..begin]).filter(|l| l.iter().any(|&b| b != 0)), Some(payload)),
        None => (None, Some(0)),
    };
    NalUnits { data, leading, pos }
}

impl<'a> Iterator for NalUnits<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if let Some(leading) = self.leading.take() {
            return Some(leading);
        }
        loop {
            let start = self.pos?;
            match find_start_code(self.data, start) {
                Some((begin, payload)) => {
                    self.pos = Some(payload);
                    if begin > start {
                        return Some(&self.data[start..begin]);
                    }
                }
                None => {
                    self.pos = None;
                    let rest = &self.data[start..];
                    return (!rest.is_empty() && (start > 0 || rest.iter().any(|&b| b != 0))).then_some(rest);
                }
            }
        }
    }
}

/// Splits Annex-B access units into RFC 6184 RTP payloads (packetization-mode=1).
///
/// * NAL units that fit are sent as single-NAL-unit packets;
/// * runs of consecutive small NAL units (typically SPS + PPS + small slices) are aggregated into STAP-A
///   packets when aggregation is enabled (default);
/// * larger NAL units are split into FU-A fragments of balanced size.
///
/// The packetizer is stateless apart from its configuration, so one instance can serve many streams.
#[derive(Debug, Clone)]
pub struct H264Packetizer {
    max_payload: usize,
    aggregate: bool,
}

impl Default for H264Packetizer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_PAYLOAD_SIZE)
    }
}

impl H264Packetizer {
    /// Creates a packetizer producing payloads of at most `mtu` bytes.
    ///
    /// `mtu` is the RTP *payload* budget, i.e. path MTU minus IP/UDP/RTP/extension/SRTP overhead. Values
    /// below [`MIN_PAYLOAD_SIZE`] are clamped.
    pub fn new(mtu: usize) -> Self {
        Self { max_payload: mtu.max(MIN_PAYLOAD_SIZE), aggregate: true }
    }

    /// Enables or disables STAP-A aggregation (enabled by default).
    pub fn with_aggregation(mut self, enabled: bool) -> Self {
        self.aggregate = enabled;
        self
    }

    /// The effective maximum payload size.
    pub fn max_payload_size(&self) -> usize {
        self.max_payload
    }

    /// Packetizes one access unit (Annex-B, typically one encoded frame after DAVE encryption).
    ///
    /// Returns RTP payloads in transmission order; the caller sets the marker bit on the last one. Returns an
    /// empty vector for empty input.
    pub fn packetize(&self, annexb_frame: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        self.packetize_into(annexb_frame, &mut out);
        out
    }

    /// Like [`packetize`](Self::packetize) but appends to `out`.
    pub fn packetize_into(&self, annexb_frame: &[u8], out: &mut Vec<Vec<u8>>) {
        let nals: Vec<&[u8]> = nal_units(annexb_frame).collect();
        let mtu = self.max_payload;
        let mut i = 0;
        while i < nals.len() {
            if self.aggregate {
                let mut size = 1;
                let mut j = i;
                while j < nals.len() && size + 2 + nals[j].len() <= mtu {
                    size += 2 + nals[j].len();
                    j += 1;
                }
                if j - i >= 2 {
                    out.push(stap_a(&nals[i..j], size));
                    i = j;
                    continue;
                }
            }
            let nal = nals[i];
            let nal_type = nal[0] & 0x1f;
            let single_ok = (1..=23).contains(&nal_type);
            if single_ok && nal.len() <= mtu {
                out.push(nal.to_vec());
            } else if !single_ok && nal.len() + 3 <= mtu {
                out.push(stap_a(&nals[i..=i], nal.len() + 3));
            } else {
                fu_a(nal, mtu, out);
            }
            i += 1;
        }
    }
}

fn stap_a(nals: &[&[u8]], size: usize) -> Vec<u8> {
    let mut p = Vec::with_capacity(size);
    let forbidden = nals.iter().fold(0, |acc, n| acc | (n[0] & 0x80));
    let nri = nals.iter().map(|n| n[0] & 0x60).max().unwrap_or(0);
    p.push(forbidden | nri | NAL_TYPE_STAP_A);
    for nal in nals {
        p.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        p.extend_from_slice(nal);
    }
    p
}

fn fu_a(nal: &[u8], mtu: usize, out: &mut Vec<Vec<u8>>) {
    let header = nal[0];
    let body = &nal[1..];
    let chunk_max = mtu - 2;
    // At least two fragments: RFC 6184 forbids Start and End in the same FU header.
    let count = body.len().div_ceil(chunk_max).max(2).min(body.len().max(1));
    let (base, extra) = (body.len() / count, body.len() % count);
    let mut offset = 0;
    for k in 0..count {
        let len = base + usize::from(k < extra);
        let mut p = Vec::with_capacity(len + 2);
        p.push((header & 0xe0) | NAL_TYPE_FU_A);
        let mut fu = header & 0x1f;
        if k == 0 {
            fu |= 0x80;
        }
        if k + 1 == count {
            fu |= 0x40;
        }
        p.push(fu);
        p.extend_from_slice(&body[offset..offset + len]);
        offset += len;
        out.push(p);
    }
}

/// Configuration of an [`H264Depacketizer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepacketizerConfig {
    /// A missing packet is declared lost once the buffered span reaches this many packets.
    pub reorder_window: u16,
    /// ... or once packets of more than this many distinct RTP timestamps are waiting behind it.
    pub reorder_frames: u8,
    /// Frames larger than this are dropped ([`DropReason::TooLarge`]).
    pub max_frame_size: usize,
    /// Drop delta frames at stream start and after a loss until an IDR frame arrives.
    pub wait_for_keyframe: bool,
}

impl Default for DepacketizerConfig {
    fn default() -> Self {
        Self { reorder_window: 32, reorder_frames: 2, max_frame_size: 8 << 20, wait_for_keyframe: true }
    }
}

/// A reassembled H.264 access unit in Annex-B format (each NAL unit prefixed with `00 00 00 01`).
#[derive(Clone, PartialEq, Eq)]
pub struct AccessUnit {
    /// RTP timestamp (90 kHz) shared by the frame's packets.
    pub timestamp: u32,
    /// Annex-B bytes, ready for DAVE decryption and then the decoder.
    pub data: Vec<u8>,
    /// Whether the access unit contains an IDR slice (NAL type 5).
    pub keyframe: bool,
}

impl std::fmt::Debug for AccessUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessUnit").field("timestamp", &self.timestamp).field("bytes", &self.data.len()).field("keyframe", &self.keyframe).finish()
    }
}

/// Why a frame was discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DropReason {
    /// Some of the frame's packets never arrived (sequence gap, unterminated FU-A).
    PacketLoss,
    /// A payload was not valid packetization-mode=1 H.264 (truncated STAP-A, FU-A without start, ...).
    Malformed,
    /// The frame exceeded [`DepacketizerConfig::max_frame_size`].
    TooLarge,
    /// A delta frame arrived while the depacketizer was waiting for an IDR after a loss (or at stream start).
    WaitingForKeyframe,
}

/// Output of the [`H264Depacketizer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepacketizerEvent {
    /// A complete access unit.
    Frame(AccessUnit),
    /// `count` packets starting at sequence number `first_seq` were given up on.
    PacketsLost {
        /// First missing sequence number.
        first_seq: u16,
        /// Number of consecutive missing packets (saturating).
        count: u32,
    },
    /// A frame was discarded.
    FrameDropped {
        /// RTP timestamp of the discarded frame.
        timestamp: u32,
        /// Why it was discarded.
        reason: DropReason,
    },
}

impl DepacketizerEvent {
    /// `true` for every event except [`DepacketizerEvent::Frame`]: the receiver should request a keyframe
    /// (RTCP PLI), rate-limited by the caller.
    pub fn needs_keyframe(&self) -> bool {
        !matches!(self, Self::Frame(_))
    }
}

/// Counters exposed by [`H264Depacketizer::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DepacketizerStats {
    /// Packets passed to [`H264Depacketizer::push`].
    pub packets_received: u64,
    /// Packets declared lost.
    pub packets_lost: u64,
    /// Packets that arrived after their slot had already been given up on.
    pub packets_late: u64,
    /// Duplicate packets.
    pub packets_duplicate: u64,
    /// Access units emitted.
    pub frames_emitted: u64,
    /// Frames dropped (any [`DropReason`]).
    pub frames_dropped: u64,
}

struct Packet {
    timestamp: u32,
    marker: bool,
    payload: Vec<u8>,
}

struct Assembly {
    timestamp: u32,
    data: Vec<u8>,
    broken: Option<DropReason>,
    in_fu: bool,
    keyframe: bool,
}

impl Assembly {
    fn reserve(&mut self, extra: usize, max: usize) -> Result<(), DropReason> {
        if self.data.len() + extra > max { Err(DropReason::TooLarge) } else { Ok(()) }
    }

    fn push_nal(&mut self, nal: &[u8], max: usize) -> Result<(), DropReason> {
        self.reserve(START_CODE.len() + nal.len(), max)?;
        self.keyframe |= nal[0] & 0x1f == NAL_TYPE_IDR;
        self.data.extend_from_slice(&START_CODE);
        self.data.extend_from_slice(nal);
        Ok(())
    }

    fn append(&mut self, payload: &[u8], max: usize) -> Result<(), DropReason> {
        let Some(&indicator) = payload.first() else { return Err(DropReason::Malformed) };
        match indicator & 0x1f {
            1..=23 => {
                if self.in_fu {
                    return Err(DropReason::Malformed);
                }
                self.push_nal(payload, max)
            }
            NAL_TYPE_STAP_A => {
                if self.in_fu || payload.len() < 4 {
                    return Err(DropReason::Malformed);
                }
                let mut rest = &payload[1..];
                while !rest.is_empty() {
                    if rest.len() < 2 {
                        return Err(DropReason::Malformed);
                    }
                    let len = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                    rest = &rest[2..];
                    if len == 0 || len > rest.len() {
                        return Err(DropReason::Malformed);
                    }
                    self.push_nal(&rest[..len], max)?;
                    rest = &rest[len..];
                }
                Ok(())
            }
            NAL_TYPE_FU_A => {
                if payload.len() < 2 {
                    return Err(DropReason::Malformed);
                }
                let fu = payload[1];
                let (start, end) = (fu & 0x80 != 0, fu & 0x40 != 0);
                let body = &payload[2..];
                if start {
                    if self.in_fu {
                        return Err(DropReason::Malformed);
                    }
                    let header = (indicator & 0xe0) | (fu & 0x1f);
                    self.reserve(START_CODE.len() + 1 + body.len(), max)?;
                    self.keyframe |= header & 0x1f == NAL_TYPE_IDR;
                    self.data.extend_from_slice(&START_CODE);
                    self.data.push(header);
                } else {
                    if !self.in_fu {
                        return Err(DropReason::Malformed);
                    }
                    self.reserve(body.len(), max)?;
                }
                self.data.extend_from_slice(body);
                self.in_fu = !end;
                Ok(())
            }
            // STAP-B, MTAP16/24, FU-B are interleaved-mode only; 0, 30, 31 are undefined.
            _ => Err(DropReason::Malformed),
        }
    }
}

/// Reassembles RTP payloads into Annex-B access units, tolerating reordering, duplication and loss.
///
/// See the [module documentation](self) for the buffering and loss policy.
pub struct H264Depacketizer {
    config: DepacketizerConfig,
    buffer: BTreeMap<u64, Packet>,
    highest: Option<u64>,
    next: Option<u64>,
    frame: Option<Assembly>,
    waiting_for_keyframe: bool,
    after_gap: bool,
    consecutive_late: u32,
    stats: DepacketizerStats,
    events: Vec<DepacketizerEvent>,
}

impl Default for H264Depacketizer {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for H264Depacketizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H264Depacketizer")
            .field("config", &self.config)
            .field("buffered", &self.buffer.len())
            .field("waiting_for_keyframe", &self.waiting_for_keyframe)
            .field("stats", &self.stats)
            .finish()
    }
}

impl H264Depacketizer {
    /// Creates a depacketizer with [`DepacketizerConfig::default`].
    pub fn new() -> Self {
        Self::with_config(DepacketizerConfig::default())
    }

    /// Creates a depacketizer with a custom configuration.
    pub fn with_config(config: DepacketizerConfig) -> Self {
        let config = DepacketizerConfig { reorder_window: config.reorder_window.max(1), ..config };
        Self {
            config,
            buffer: BTreeMap::new(),
            highest: None,
            next: None,
            frame: None,
            waiting_for_keyframe: config.wait_for_keyframe,
            after_gap: false,
            consecutive_late: 0,
            stats: DepacketizerStats::default(),
            events: Vec::new(),
        }
    }

    /// The active configuration.
    pub fn config(&self) -> &DepacketizerConfig {
        &self.config
    }

    /// Counters since creation.
    pub fn stats(&self) -> DepacketizerStats {
        self.stats
    }

    /// Whether delta frames are currently being discarded until the next IDR.
    pub fn is_waiting_for_keyframe(&self) -> bool {
        self.waiting_for_keyframe
    }

    /// Forgets all buffered state (e.g. on SSRC change). Counters are kept.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.highest = None;
        self.next = None;
        self.frame = None;
        self.after_gap = false;
        self.consecutive_late = 0;
        self.waiting_for_keyframe = self.config.wait_for_keyframe;
    }

    /// Feeds one RTP packet (header fields + payload) and returns whatever became ready.
    ///
    /// Packets may arrive out of order, duplicated or not at all. Never panics, whatever the payload.
    pub fn push(&mut self, seq: u16, timestamp: u32, marker: bool, payload: &[u8]) -> Vec<DepacketizerEvent> {
        self.stats.packets_received += 1;
        let mut ext = self.extend(seq);
        if self.next.is_some_and(|next| ext < next) {
            self.stats.packets_late += 1;
            self.consecutive_late += 1;
            if self.consecutive_late <= u32::from(self.config.reorder_window) {
                return std::mem::take(&mut self.events);
            }
            // A long run of "late" packets means the sender restarted its sequence space: resynchronise.
            self.finish_frame();
            self.reset();
            ext = self.extend(seq);
        }
        self.consecutive_late = 0;
        match self.buffer.entry(ext) {
            Entry::Occupied(_) => self.stats.packets_duplicate += 1,
            Entry::Vacant(slot) => {
                slot.insert(Packet { timestamp, marker, payload: payload.to_vec() });
                self.drain(false);
            }
        }
        std::mem::take(&mut self.events)
    }

    /// Processes everything still buffered, treating holes as lost and completing the frame in progress.
    ///
    /// Call at end of stream, or from a timer when no packets have arrived for a while (e.g. 200 ms) to bound
    /// the latency of a lost marker packet.
    pub fn flush(&mut self) -> Vec<DepacketizerEvent> {
        self.drain(true);
        self.finish_frame();
        std::mem::take(&mut self.events)
    }

    fn extend(&mut self, seq: u16) -> u64 {
        match self.highest {
            None => {
                let ext = (1u64 << 32) | u64::from(seq);
                self.highest = Some(ext);
                ext
            }
            Some(highest) => {
                let delta = i64::from(seq.wrapping_sub(highest as u16) as i16);
                let ext = (highest as i64 + delta).max(0) as u64;
                if ext > highest {
                    self.highest = Some(ext);
                }
                ext
            }
        }
    }

    fn should_skip_hole(&self, first: u64) -> bool {
        let base = self.next.unwrap_or(first);
        let highest = self.highest.unwrap_or(first);
        if highest.saturating_sub(base) >= u64::from(self.config.reorder_window) {
            return true;
        }
        let mut seen: Vec<u32> = Vec::with_capacity(4);
        for packet in self.buffer.values() {
            if !seen.contains(&packet.timestamp) {
                seen.push(packet.timestamp);
                if seen.len() > usize::from(self.config.reorder_frames) {
                    return true;
                }
            }
        }
        false
    }

    fn drain(&mut self, force: bool) {
        loop {
            if let Some(mut next) = self.next {
                while let Some(packet) = self.buffer.remove(&next) {
                    self.process(packet);
                    next += 1;
                }
                self.next = Some(next);
            }
            let Some(&first) = self.buffer.keys().next() else { break };
            if !force && !self.should_skip_hole(first) {
                break;
            }
            if let Some(next) = self.next {
                let lost = first - next;
                self.stats.packets_lost += lost;
                self.events.push(DepacketizerEvent::PacketsLost { first_seq: next as u16, count: u32::try_from(lost).unwrap_or(u32::MAX) });
                if let Some(frame) = &mut self.frame {
                    frame.broken.get_or_insert(DropReason::PacketLoss);
                }
                self.after_gap = true;
                if self.config.wait_for_keyframe {
                    self.waiting_for_keyframe = true;
                }
            }
            self.next = Some(first);
        }
    }

    fn process(&mut self, packet: Packet) {
        let after_gap = std::mem::take(&mut self.after_gap);
        if self.frame.as_ref().is_some_and(|f| f.timestamp != packet.timestamp) {
            // Timestamp changed without a marker: the previous frame ended (its marker packet may be lost).
            self.finish_frame();
        }
        let max = self.config.max_frame_size;
        let frame = self.frame.get_or_insert_with(|| Assembly {
            timestamp: packet.timestamp,
            data: Vec::new(),
            // After a gap we cannot know whether this frame's first packets were among the lost ones.
            broken: after_gap.then_some(DropReason::PacketLoss),
            in_fu: false,
            keyframe: false,
        });
        if frame.broken.is_none()
            && let Err(reason) = frame.append(&packet.payload, max)
        {
            frame.broken = Some(reason);
            frame.data = Vec::new();
        }
        if packet.marker {
            self.finish_frame();
        }
    }

    fn finish_frame(&mut self) {
        let Some(frame) = self.frame.take() else { return };
        let reason = frame.broken.or_else(|| frame.in_fu.then_some(DropReason::PacketLoss)).or_else(|| frame.data.is_empty().then_some(DropReason::Malformed));
        let reason = match reason {
            Some(reason) => {
                if self.config.wait_for_keyframe {
                    self.waiting_for_keyframe = true;
                }
                Some(reason)
            }
            None if self.waiting_for_keyframe && !frame.keyframe => Some(DropReason::WaitingForKeyframe),
            None => None,
        };
        match reason {
            Some(reason) => {
                self.stats.frames_dropped += 1;
                self.events.push(DepacketizerEvent::FrameDropped { timestamp: frame.timestamp, reason });
            }
            None => {
                if frame.keyframe {
                    self.waiting_for_keyframe = false;
                }
                self.stats.frames_emitted += 1;
                self.events.push(DepacketizerEvent::Frame(AccessUnit { timestamp: frame.timestamp, data: frame.data, keyframe: frame.keyframe }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter().flat_map(|n| START_CODE.iter().chain(n.iter()).copied()).collect()
    }

    fn deliver(depack: &mut H264Depacketizer, payloads: &[Vec<u8>], seq0: u16, ts: u32) -> Vec<DepacketizerEvent> {
        let mut events = Vec::new();
        for (i, p) in payloads.iter().enumerate() {
            events.extend(depack.push(seq0.wrapping_add(i as u16), ts, i + 1 == payloads.len(), p));
        }
        events
    }

    #[test]
    fn splits_start_codes() {
        let data = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 0, 1, 0x65, 0, 0];
        let nals: Vec<&[u8]> = nal_units(&data).collect();
        assert_eq!(nals, vec![&[0x67, 1, 2][..], &[0x68, 3, 0][..], &[0x65, 0, 0][..]]);
        assert_eq!(nal_units(&[0, 0, 0]).count(), 0);
        assert_eq!(nal_units(&[]).count(), 0);
        assert_eq!(nal_units(&[0x65, 1]).collect::<Vec<_>>(), vec![&[0x65, 1][..]]);
        assert_eq!(nal_units(&[0, 0, 1, 0, 0, 1]).count(), 0);
    }

    #[test]
    fn packet_kinds() {
        let p = H264Packetizer::new(100);
        let sps = [0x67u8, 0x42, 0xe0, 0x1f, 1, 2];
        let pps = [0x68u8, 0xce, 0x3c, 0x80];
        let idr: Vec<u8> = std::iter::once(0x65).chain((0..1000).map(|i| (i % 250 + 1) as u8)).collect();
        let frame = annexb(&[&sps, &pps, &idr]);
        let payloads = p.packetize(&frame);
        assert_eq!(payloads[0][0] & 0x1f, NAL_TYPE_STAP_A);
        assert!(payloads[1..].iter().all(|p| p[0] & 0x1f == NAL_TYPE_FU_A && p.len() <= 100));
        assert_eq!(payloads[1][1] & 0xc0, 0x80);
        assert_eq!(payloads.last().unwrap()[1] & 0xc0, 0x40);

        let mut d = H264Depacketizer::new();
        let mut events = deliver(&mut d, &payloads, 65530, 9000);
        events.extend(d.flush());
        assert_eq!(events, vec![DepacketizerEvent::Frame(AccessUnit { timestamp: 9000, data: frame, keyframe: true })]);

        // Without aggregation every small NAL goes out on its own; reserved-type NALs still survive.
        let p = H264Packetizer::new(100).with_aggregation(false);
        let odd = annexb(&[&[0x7d, 1, 2], &[0x1c, 9], &[0x00, 7], &[0x41, 5]]);
        let payloads = p.packetize(&odd);
        assert_eq!(payloads.len(), 4);
        let mut d = H264Depacketizer::with_config(DepacketizerConfig { wait_for_keyframe: false, ..Default::default() });
        let mut events = deliver(&mut d, &payloads, 1, 1);
        events.extend(d.flush());
        assert_eq!(events, vec![DepacketizerEvent::Frame(AccessUnit { timestamp: 1, data: odd, keyframe: false })]);
    }

    #[test]
    fn small_mtu_reserved_type_fragments_in_two() {
        let p = H264Packetizer::new(MIN_PAYLOAD_SIZE);
        let nal = [0x1e, 1, 2, 3, 4, 5, 6];
        let payloads = p.packetize(&annexb(&[&nal]));
        assert!(payloads.len() >= 2);
        assert!(payloads.iter().all(|p| p.len() <= MIN_PAYLOAD_SIZE && p[1] & 0xc0 != 0xc0));
    }

    #[test]
    fn garbage_never_panics() {
        let mut d = H264Depacketizer::new();
        let samples: [&[u8]; 9] = [&[], &[24], &[24, 0], &[24, 0, 5, 1], &[28], &[28, 0x40, 1], &[28, 0x85, 1], &[25, 1, 2], &[0x1f]];
        for (i, s) in samples.iter().enumerate() {
            d.push(i as u16 * 7, i as u32, i % 2 == 0, s);
        }
        d.push(40000, 1, true, &[0x65, 1]);
        d.push(3, 1, true, &[0x65, 1]);
        d.flush();
    }
}
