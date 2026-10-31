//! Minimal RTP (RFC 3550) and RTCP helpers.

/// Parsed fixed RTP header plus the byte offsets needed by `rtpsize` encryption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    pub marker: bool,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    /// Bytes of the header sent in the clear: fixed header + CSRCs + 4-byte
    /// extension header (the extension *body* is encrypted in `rtpsize` modes).
    pub unencrypted_len: usize,
    /// Length of the extension body in bytes (0 without the X bit).
    pub extension_body_len: usize,
}

pub const RTP_HEADER_LEN: usize = 12;

impl RtpHeader {
    pub fn parse(pkt: &[u8]) -> Option<Self> {
        if pkt.len() < RTP_HEADER_LEN || pkt[0] >> 6 != 2 {
            return None;
        }
        let cc = (pkt[0] & 0x0f) as usize;
        let has_ext = pkt[0] & 0x10 != 0;
        let mut len = RTP_HEADER_LEN + cc * 4;
        let mut ext_body = 0;
        if has_ext {
            if pkt.len() < len + 4 {
                return None;
            }
            ext_body = u16::from_be_bytes([pkt[len + 2], pkt[len + 3]]) as usize * 4;
            len += 4;
        }
        if pkt.len() < len {
            return None;
        }
        Some(Self {
            marker: pkt[1] & 0x80 != 0,
            payload_type: pkt[1] & 0x7f,
            sequence: u16::from_be_bytes([pkt[2], pkt[3]]),
            timestamp: u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]),
            ssrc: u32::from_be_bytes([pkt[8], pkt[9], pkt[10], pkt[11]]),
            unencrypted_len: len,
            extension_body_len: ext_body,
        })
    }

    /// Writes a 12-byte header without CSRCs or extensions.
    pub fn write(marker: bool, payload_type: u8, sequence: u16, timestamp: u32, ssrc: u32, out: &mut Vec<u8>) {
        out.push(0x80);
        out.push(((marker as u8) << 7) | (payload_type & 0x7f));
        out.extend_from_slice(&sequence.to_be_bytes());
        out.extend_from_slice(&timestamp.to_be_bytes());
        out.extend_from_slice(&ssrc.to_be_bytes());
    }
}

/// RTCP packet types share the second byte with RTP payload types when muxed
/// (RFC 5761): values 192–223 identify RTCP.
pub fn is_rtcp(pkt: &[u8]) -> bool {
    pkt.len() >= 8 && pkt[0] >> 6 == 2 && (192..=223).contains(&pkt[1])
}

pub const RTCP_HEADER_LEN: usize = 8;

/// Picture Loss Indication (RFC 4585, PT=206 PSFB, FMT=1): asks the sender of
/// `media_ssrc` for a new keyframe.
pub fn pli(sender_ssrc: u32, media_ssrc: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(12);
    v.push(0x80 | 1);
    v.push(206);
    v.extend_from_slice(&2u16.to_be_bytes()); // length in 32-bit words minus one
    v.extend_from_slice(&sender_ssrc.to_be_bytes());
    v.extend_from_slice(&media_ssrc.to_be_bytes());
    v
}

/// Returns the media SSRCs that a (compound) RTCP packet requests keyframes for
/// via PLI or FIR.
pub fn keyframe_requests(pkt: &[u8]) -> Vec<u32> {
    let mut out = vec![];
    let mut off = 0;
    while off + 4 <= pkt.len() {
        let fmt = pkt[off] & 0x1f;
        let pt = pkt[off + 1];
        let words = u16::from_be_bytes([pkt[off + 2], pkt[off + 3]]) as usize;
        let end = off + (words + 1) * 4;
        if end > pkt.len() {
            break;
        }
        if pt == 206 && (fmt == 1 || fmt == 4) && end >= off + 12 {
            out.push(u32::from_be_bytes([pkt[off + 8], pkt[off + 9], pkt[off + 10], pkt[off + 11]]));
        }
        off = end;
    }
    out
}

/// Unwraps 16-bit sequence numbers into a monotonically increasing u64.
#[derive(Debug, Default, Clone)]
pub struct SeqUnwrapper {
    last: Option<u64>,
}

impl SeqUnwrapper {
    pub fn unwrap(&mut self, seq: u16) -> u64 {
        let v = match self.last {
            None => seq as u64 + (1 << 16),
            Some(last) => {
                let base = last & !0xffff;
                let candidates = [base.wrapping_sub(1 << 16) | seq as u64, base | seq as u64, (base + (1 << 16)) | seq as u64];
                *candidates.iter().min_by_key(|c| c.abs_diff(last)).expect("non-empty")
            }
        };
        if self.last.is_none_or(|l| v > l) {
            self.last = Some(v);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let mut buf = vec![];
        RtpHeader::write(true, 120, 0xBEEF, 0x01020304, 0xCAFEBABE, &mut buf);
        buf.extend_from_slice(b"payload");
        let h = RtpHeader::parse(&buf).unwrap();
        assert!(h.marker);
        assert_eq!((h.payload_type, h.sequence, h.timestamp, h.ssrc), (120, 0xBEEF, 0x01020304, 0xCAFEBABE));
        assert_eq!(h.unencrypted_len, 12);
    }

    #[test]
    fn extension_offsets() {
        // X bit set, one CSRC, extension of 2 words.
        let mut p = vec![0x91, 120, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3];
        p.extend_from_slice(&[0, 0, 0, 9]); // CSRC
        p.extend_from_slice(&[0xBE, 0xDE, 0, 2]); // ext header
        p.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]); // ext body
        let h = RtpHeader::parse(&p).unwrap();
        assert_eq!(h.unencrypted_len, 12 + 4 + 4);
        assert_eq!(h.extension_body_len, 8);
    }

    #[test]
    fn rtcp_detection_and_pli() {
        let p = pli(1, 2);
        assert!(is_rtcp(&p));
        assert_eq!(keyframe_requests(&p), vec![2]);
        let mut rtp = vec![];
        RtpHeader::write(false, 120, 1, 1, 1, &mut rtp);
        assert!(!is_rtcp(&rtp));
    }

    #[test]
    fn seq_unwrap_handles_wrap_and_reorder() {
        let mut u = SeqUnwrapper::default();
        let a = u.unwrap(65534);
        let b = u.unwrap(65535);
        let c = u.unwrap(0);
        let d = u.unwrap(65533); // late packet from before the wrap
        assert!(a < b && b < c && d < a);
        assert_eq!(c - b, 1);
    }
}
