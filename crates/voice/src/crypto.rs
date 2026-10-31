//! Transport encryption for voice UDP packets (the "rtpsize" AEAD modes).
//!
//! Layout of an encrypted RTP packet:
//! `[header (clear, used as AAD)] [ciphertext of payload incl. extension body] [16-byte tag] [4-byte nonce counter BE]`.
//! The AEAD nonce is the 4-byte counter followed by zeros (12 bytes for AES-GCM,
//! 24 for XChaCha20). RTCP uses the same scheme with its 8-byte header as AAD.

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce as GcmNonce};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use crate::rtp::{RTCP_HEADER_LEN, RtpHeader};

const TAG_LEN: usize = 16;
const NONCE_SUFFIX_LEN: usize = 4;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("unsupported encryption mode {0}")]
    UnsupportedMode(String),
    #[error("invalid key length")]
    BadKey,
    #[error("packet too short")]
    TooShort,
    #[error("authentication failed")]
    AuthFailed,
}

enum Cipher {
    Aes(Box<Aes256Gcm>),
    XChaCha(Box<XChaCha20Poly1305>),
}

pub struct TransportCipher {
    cipher: Cipher,
    counter: u32,
    mode: String,
}

impl TransportCipher {
    pub fn new(mode: &str, key: &[u8]) -> Result<Self, CryptoError> {
        let cipher = match mode {
            "aead_aes256_gcm_rtpsize" => Cipher::Aes(Box::new(Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::BadKey)?)),
            "aead_xchacha20_poly1305_rtpsize" => Cipher::XChaCha(Box::new(XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::BadKey)?)),
            other => return Err(CryptoError::UnsupportedMode(other.to_string())),
        };
        Ok(Self { cipher, counter: rand::random(), mode: mode.to_string() })
    }

    pub fn mode(&self) -> &str {
        &self.mode
    }

    fn seal(&mut self, buf: &mut Vec<u8>, aad_len: usize) -> Result<(), CryptoError> {
        let counter = self.counter;
        self.counter = self.counter.wrapping_add(1);
        let (aad, body) = buf.split_at_mut(aad_len);
        let mut payload = body.to_vec();
        match &self.cipher {
            Cipher::Aes(c) => {
                let mut n = [0u8; 12];
                n[..4].copy_from_slice(&counter.to_be_bytes());
                c.encrypt_in_place(GcmNonce::from_slice(&n), aad, &mut payload).map_err(|_| CryptoError::AuthFailed)?;
            }
            Cipher::XChaCha(c) => {
                let mut n = [0u8; 24];
                n[..4].copy_from_slice(&counter.to_be_bytes());
                c.encrypt_in_place(XNonce::from_slice(&n), aad, &mut payload).map_err(|_| CryptoError::AuthFailed)?;
            }
        }
        buf.truncate(aad_len);
        buf.extend_from_slice(&payload);
        buf.extend_from_slice(&counter.to_be_bytes());
        Ok(())
    }

    fn open(&self, pkt: &[u8], aad_len: usize) -> Result<Vec<u8>, CryptoError> {
        if pkt.len() < aad_len + TAG_LEN + NONCE_SUFFIX_LEN {
            return Err(CryptoError::TooShort);
        }
        let nonce_at = pkt.len() - NONCE_SUFFIX_LEN;
        let counter = &pkt[nonce_at..];
        let aad = &pkt[..aad_len];
        let mut body = pkt[aad_len..nonce_at].to_vec();
        match &self.cipher {
            Cipher::Aes(c) => {
                let mut n = [0u8; 12];
                n[..4].copy_from_slice(counter);
                c.decrypt_in_place(GcmNonce::from_slice(&n), aad, &mut body).map_err(|_| CryptoError::AuthFailed)?;
            }
            Cipher::XChaCha(c) => {
                let mut n = [0u8; 24];
                n[..4].copy_from_slice(counter);
                c.decrypt_in_place(XNonce::from_slice(&n), aad, &mut body).map_err(|_| CryptoError::AuthFailed)?;
            }
        }
        Ok(body)
    }

    /// Encrypts a plain RTP packet (header + payload) in place.
    pub fn encrypt_rtp(&mut self, packet: &mut Vec<u8>) -> Result<(), CryptoError> {
        let header = RtpHeader::parse(packet).ok_or(CryptoError::TooShort)?;
        self.seal(packet, header.unencrypted_len)
    }

    /// Decrypts an RTP packet, returning the header and the media payload
    /// (with any header-extension body stripped).
    pub fn decrypt_rtp(&self, packet: &[u8]) -> Result<(RtpHeader, Vec<u8>), CryptoError> {
        let header = RtpHeader::parse(packet).ok_or(CryptoError::TooShort)?;
        let mut plain = self.open(packet, header.unencrypted_len)?;
        if header.extension_body_len > 0 {
            if plain.len() < header.extension_body_len {
                return Err(CryptoError::TooShort);
            }
            plain.drain(..header.extension_body_len);
        }
        Ok((header, plain))
    }

    pub fn encrypt_rtcp(&mut self, packet: &mut Vec<u8>) -> Result<(), CryptoError> {
        if packet.len() < RTCP_HEADER_LEN {
            return Err(CryptoError::TooShort);
        }
        self.seal(packet, RTCP_HEADER_LEN)
    }

    /// Decrypts RTCP, returning the full plaintext packet (header + body).
    pub fn decrypt_rtcp(&self, packet: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let body = self.open(packet, RTCP_HEADER_LEN)?;
        let mut out = packet[..RTCP_HEADER_LEN].to_vec();
        out.extend_from_slice(&body);
        Ok(out)
    }
}

/// Picks the best mode both sides support.
pub fn choose_mode(offered: &[String]) -> Option<&'static str> {
    crate::protocol::SUPPORTED_MODES.iter().copied().find(|m| offered.iter().any(|o| o == m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet() -> Vec<u8> {
        let mut p = vec![];
        RtpHeader::write(false, 120, 7, 960, 42, &mut p);
        p.extend_from_slice(b"opus frame bytes");
        p
    }

    #[test]
    fn roundtrip_both_modes() {
        for mode in crate::protocol::SUPPORTED_MODES {
            let key = [7u8; 32];
            let mut tx = TransportCipher::new(mode, &key).unwrap();
            let rx = TransportCipher::new(mode, &key).unwrap();
            let mut p = packet();
            let header = p[..12].to_vec();
            tx.encrypt_rtp(&mut p).unwrap();
            assert_eq!(&p[..12], &header[..], "header must stay in the clear");
            assert_eq!(p.len(), 12 + 16 + TAG_LEN + NONCE_SUFFIX_LEN);
            let (h, payload) = rx.decrypt_rtp(&p).unwrap();
            assert_eq!(h.sequence, 7);
            assert_eq!(payload, b"opus frame bytes");
        }
    }

    #[test]
    fn tampering_is_detected() {
        let mut tx = TransportCipher::new("aead_aes256_gcm_rtpsize", &[1; 32]).unwrap();
        let rx = TransportCipher::new("aead_aes256_gcm_rtpsize", &[1; 32]).unwrap();
        let mut p = packet();
        tx.encrypt_rtp(&mut p).unwrap();
        p[3] ^= 1; // header is authenticated as AAD
        assert_eq!(rx.decrypt_rtp(&p).unwrap_err(), CryptoError::AuthFailed);
        let wrong = TransportCipher::new("aead_aes256_gcm_rtpsize", &[2; 32]).unwrap();
        let mut p = packet();
        tx.encrypt_rtp(&mut p).unwrap();
        assert!(wrong.decrypt_rtp(&p).is_err());
    }

    #[test]
    fn extension_body_is_stripped() {
        let key = [3u8; 32];
        let mut tx = TransportCipher::new("aead_xchacha20_poly1305_rtpsize", &key).unwrap();
        let rx = TransportCipher::new("aead_xchacha20_poly1305_rtpsize", &key).unwrap();
        let mut p = vec![0x90, 120, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0xBE, 0xDE, 0, 1, 0xAA, 0xBB, 0xCC, 0xDD];
        p.extend_from_slice(b"media");
        tx.encrypt_rtp(&mut p).unwrap();
        assert_eq!(&p[12..16], &[0xBE, 0xDE, 0, 1], "extension header stays clear");
        let (_, payload) = rx.decrypt_rtp(&p).unwrap();
        assert_eq!(payload, b"media");
    }

    #[test]
    fn rtcp_roundtrip() {
        let mut tx = TransportCipher::new("aead_aes256_gcm_rtpsize", &[9; 32]).unwrap();
        let rx = TransportCipher::new("aead_aes256_gcm_rtpsize", &[9; 32]).unwrap();
        let plain = crate::rtp::pli(1, 2);
        let mut p = plain.clone();
        tx.encrypt_rtcp(&mut p).unwrap();
        assert_eq!(rx.decrypt_rtcp(&p).unwrap(), plain);
    }

    #[test]
    fn mode_negotiation_prefers_aes() {
        let offered = vec!["aead_xchacha20_poly1305_rtpsize".to_string(), "aead_aes256_gcm_rtpsize".to_string(), "xsalsa20_poly1305".to_string()];
        assert_eq!(choose_mode(&offered), Some("aead_aes256_gcm_rtpsize"));
        assert_eq!(choose_mode(&["xsalsa20_poly1305".to_string()]), None);
    }
}
