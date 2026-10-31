//! DAVE (Discord Audio & Video End-to-End Encryption) state machine.
//!
//! Wraps a [`davey::DaveSession`] and implements the voice-gateway side of the
//! protocol (opcodes 21–31, see <https://daveprotocol.com>). Every handler
//! returns the messages to send back, so this type is pure and testable.

use std::collections::HashSet;
use std::num::NonZeroU16;

use davey::{Codec, DaveSession, MediaType, ProposalsOperationType};
use serde_json::json;

use crate::protocol::op;

/// A message for the voice gateway.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    Json(u8, serde_json::Value),
    Binary(u8, Vec<u8>),
}

/// Consecutive decryption failures before we ask for a new group.
const FAILURE_THRESHOLD: u32 = 25;
/// How long (seconds) to accept unencrypted media after a transition.
const PASSTHROUGH_GRACE: u32 = 10;

pub struct Dave {
    user_id: u64,
    channel_id: u64,
    protocol_version: u16,
    session: Option<DaveSession>,
    pending_transition: Option<(u16, u16)>,
    /// Users currently connected to the call (always includes us).
    connected: HashSet<u64>,
    reinitializing: bool,
    failures: u32,
}

impl Dave {
    pub fn new(user_id: u64, channel_id: u64) -> Self {
        let mut connected = HashSet::new();
        connected.insert(user_id);
        Self { user_id, channel_id, protocol_version: 0, session: None, pending_transition: None, connected, reinitializing: false, failures: 0 }
    }

    pub fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    /// Ready to end-to-end encrypt outgoing media.
    pub fn is_active(&self) -> bool {
        self.protocol_version != 0 && self.session.as_ref().is_some_and(|s| s.is_ready())
    }

    /// The "voice privacy code" users can compare to verify E2EE.
    pub fn privacy_code(&self) -> Option<String> {
        self.session.as_ref().and_then(|s| s.voice_privacy_code()).filter(|c| !c.is_empty()).map(str::to_string)
    }

    /// Pairwise verification code for another member.
    pub fn verification_code(&self, user: u64) -> Option<String> {
        self.session.as_ref()?.get_verification_code(user).ok()
    }

    pub fn clients_connected(&mut self, ids: impl IntoIterator<Item = u64>) {
        self.connected.extend(ids);
    }

    pub fn client_disconnected(&mut self, id: u64) {
        if id != self.user_id {
            self.connected.remove(&id);
        }
    }

    /// Called with `dave_protocol_version` from SESSION_DESCRIPTION.
    pub fn init(&mut self, protocol_version: u16) -> Vec<Outgoing> {
        self.protocol_version = protocol_version;
        self.reinit()
    }

    /// (Re)creates the MLS session and returns our key package (op 26).
    fn reinit(&mut self) -> Vec<Outgoing> {
        let Some(version) = NonZeroU16::new(self.protocol_version) else {
            if let Some(s) = &mut self.session {
                let _ = s.reset();
                s.set_passthrough_mode(true, Some(PASSTHROUGH_GRACE));
            }
            return vec![];
        };
        let result = match &mut self.session {
            Some(s) => s.reinit(version, self.user_id, self.channel_id, None).map_err(|e| e.to_string()),
            None => DaveSession::new(version, self.user_id, self.channel_id, None).map(|s| self.session = Some(s)).map_err(|e| e.to_string()),
        };
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to initialize DAVE session");
            return vec![];
        }
        self.key_package()
    }

    fn key_package(&mut self) -> Vec<Outgoing> {
        match self.session.as_mut().map(|s| s.create_key_package()) {
            Some(Ok(kp)) => vec![Outgoing::Binary(op::DAVE_MLS_KEY_PACKAGE, kp)],
            Some(Err(e)) => {
                tracing::error!(error = %e, "failed to create DAVE key package");
                vec![]
            }
            None => vec![],
        }
    }

    /// Op 25: the voice server's MLS external sender.
    pub fn on_external_sender(&mut self, payload: &[u8]) -> Vec<Outgoing> {
        if let Some(s) = &mut self.session
            && let Err(e) = s.set_external_sender(payload)
        {
            tracing::warn!(error = %e, "set_external_sender failed");
        }
        vec![]
    }

    /// Op 27: proposals to add/remove members. Returns our commit (+welcome) as op 28.
    pub fn on_proposals(&mut self, payload: &[u8]) -> Vec<Outgoing> {
        let Some((&kind, proposals)) = payload.split_first() else { return vec![] };
        let kind = if kind == 1 { ProposalsOperationType::REVOKE } else { ProposalsOperationType::APPEND };
        let expected: Vec<u64> = self.connected.iter().copied().collect();
        let Some(s) = &mut self.session else { return vec![] };
        match s.process_proposals(kind, proposals, Some(&expected)) {
            Ok(Some(cw)) => {
                let mut buf = cw.commit;
                if let Some(w) = cw.welcome {
                    buf.extend_from_slice(&w);
                }
                vec![Outgoing::Binary(op::DAVE_MLS_COMMIT_WELCOME, buf)]
            }
            Ok(None) => vec![],
            Err(e) => {
                tracing::warn!(error = %e, "failed to process DAVE proposals");
                vec![]
            }
        }
    }

    /// Op 29: the commit that won this round, `[transition_id u16][commit]`.
    pub fn on_announce_commit(&mut self, payload: &[u8]) -> Vec<Outgoing> {
        let Some((tid, commit)) = split_transition(payload) else { return vec![] };
        let ok = self.session.as_mut().map(|s| s.process_commit(commit)).map(|r| r.map_err(|e| e.to_string()));
        self.after_group_change(tid, ok)
    }

    /// Op 30: we were added to the group, `[transition_id u16][welcome]`.
    pub fn on_welcome(&mut self, payload: &[u8]) -> Vec<Outgoing> {
        let Some((tid, welcome)) = split_transition(payload) else { return vec![] };
        let expected: Vec<u64> = self.connected.iter().copied().collect();
        let ok = self.session.as_mut().map(|s| s.process_welcome(welcome, Some(&expected)).map(|_| true).map_err(|e| e.to_string()));
        self.after_group_change(tid, ok)
    }

    fn after_group_change(&mut self, transition_id: u16, result: Option<Result<bool, String>>) -> Vec<Outgoing> {
        match result {
            Some(Ok(_)) => {
                if transition_id == 0 {
                    self.reinitializing = false;
                    // Transition 0 applies immediately (initial group).
                    if let Some(s) = &mut self.session {
                        let _ = s.prepare_transition(0, self.protocol_version);
                    }
                    vec![]
                } else {
                    if let Some(s) = &mut self.session {
                        let _ = s.prepare_transition(transition_id, self.protocol_version);
                    }
                    self.pending_transition = Some((transition_id, self.protocol_version));
                    vec![Outgoing::Json(op::DAVE_TRANSITION_READY, json!({ "transition_id": transition_id }))]
                }
            }
            Some(Err(e)) => {
                tracing::warn!(error = %e, transition_id, "invalid DAVE commit/welcome; recovering");
                self.recover(transition_id)
            }
            None => vec![],
        }
    }

    /// Asks the server to remove us from the broken group and rejoin.
    fn recover(&mut self, transition_id: u16) -> Vec<Outgoing> {
        if self.reinitializing {
            return vec![];
        }
        self.reinitializing = true;
        self.failures = 0;
        let mut out = vec![Outgoing::Json(op::DAVE_MLS_INVALID_COMMIT_WELCOME, json!({ "transition_id": transition_id }))];
        out.extend(self.reinit());
        out
    }

    /// Op 21: a protocol upgrade/downgrade is coming.
    pub fn on_prepare_transition(&mut self, transition_id: u16, protocol_version: u16) -> Vec<Outgoing> {
        self.pending_transition = Some((transition_id, protocol_version));
        if let Some(s) = &mut self.session {
            if protocol_version == 0 {
                s.set_passthrough_mode(true, Some(120));
            }
            let _ = s.prepare_transition(transition_id, protocol_version);
        }
        if transition_id == 0 {
            self.protocol_version = protocol_version;
            return vec![];
        }
        vec![Outgoing::Json(op::DAVE_TRANSITION_READY, json!({ "transition_id": transition_id }))]
    }

    /// Op 22: switch keys now.
    pub fn on_execute_transition(&mut self, transition_id: u16) -> Vec<Outgoing> {
        let Some((tid, version)) = self.pending_transition else { return vec![] };
        if tid != transition_id {
            return vec![];
        }
        self.pending_transition = None;
        let was_downgraded = self.protocol_version == 0;
        self.protocol_version = version;
        if let Some(s) = &mut self.session {
            let _ = s.execute_transition(transition_id);
            if was_downgraded && version != 0 {
                // Upgrading back: keep accepting plaintext for a short grace period.
                s.set_passthrough_mode(true, Some(PASSTHROUGH_GRACE));
            }
        }
        vec![]
    }

    /// Op 24: a new MLS epoch. Epoch 1 means a brand-new group: start over.
    pub fn on_prepare_epoch(&mut self, epoch: u64, protocol_version: u16) -> Vec<Outgoing> {
        if epoch == 1 {
            self.protocol_version = protocol_version;
            return self.reinit();
        }
        vec![]
    }

    /// End-to-end encrypts an outgoing frame (passes through when DAVE is off).
    pub fn encrypt(&mut self, media: MediaType, codec: Codec, frame: &[u8]) -> Option<Vec<u8>> {
        if self.protocol_version == 0 {
            return Some(frame.to_vec());
        }
        let s = self.session.as_mut()?;
        if !s.is_ready() {
            // Never leak plaintext into an E2EE call: drop until keys exist.
            return None;
        }
        match s.encrypt(media, codec, frame) {
            Ok(c) => Some(c.into_owned()),
            Err(e) => {
                tracing::debug!(error = %e, "DAVE encrypt failed");
                None
            }
        }
    }

    /// Decrypts an incoming frame from `user`. Returns `(frame, recovery messages)`.
    pub fn decrypt(&mut self, user: u64, media: MediaType, frame: &[u8]) -> (Option<Vec<u8>>, Vec<Outgoing>) {
        if frame == davey::OPUS_SILENCE_PACKET {
            return (Some(frame.to_vec()), vec![]);
        }
        let Some(s) = self.session.as_mut() else { return (Some(frame.to_vec()), vec![]) };
        let can_decrypt = s.is_ready() && (self.protocol_version != 0 || s.can_passthrough(user));
        if !can_decrypt {
            return (if self.protocol_version == 0 { Some(frame.to_vec()) } else { None }, vec![]);
        }
        match s.decrypt(user, media, frame) {
            Ok(p) => {
                self.failures = 0;
                (Some(p), vec![])
            }
            Err(e) => {
                self.failures += 1;
                tracing::debug!(user, error = %e, failures = self.failures, "DAVE decrypt failed");
                if self.failures > FAILURE_THRESHOLD {
                    let tid = self.pending_transition.map(|t| t.0).unwrap_or(0);
                    return (None, self.recover(tid));
                }
                (None, vec![])
            }
        }
    }
}

fn split_transition(payload: &[u8]) -> Option<(u16, &[u8])> {
    if payload.len() < 2 {
        return None;
    }
    Some((u16::from_be_bytes([payload[0], payload[1]]), &payload[2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_zero_is_passthrough() {
        let mut d = Dave::new(1, 2);
        assert!(d.init(0).is_empty());
        assert!(!d.is_active());
        assert_eq!(d.encrypt(MediaType::AUDIO, Codec::OPUS, b"abc"), Some(b"abc".to_vec()));
        assert_eq!(d.decrypt(5, MediaType::AUDIO, b"abc").0, Some(b"abc".to_vec()));
    }

    #[test]
    fn version_one_sends_key_package_and_withholds_plaintext() {
        let mut d = Dave::new(1, 2);
        let out = d.init(1);
        assert!(matches!(out.as_slice(), [Outgoing::Binary(26, kp)] if !kp.is_empty()));
        assert!(!d.is_active());
        assert_eq!(d.encrypt(MediaType::AUDIO, Codec::OPUS, b"secret"), None, "no plaintext before keys exist");
    }

    #[test]
    fn prepare_transition_acks_nonzero_ids() {
        let mut d = Dave::new(1, 2);
        d.init(1);
        assert_eq!(d.on_prepare_transition(7, 1), vec![Outgoing::Json(23, json!({ "transition_id": 7 }))]);
        assert!(d.on_prepare_transition(0, 1).is_empty());
    }
}
