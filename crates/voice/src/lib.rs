//! Discord voice for Ferrocord.
//!
//! * [`protocol`] — voice gateway v9 payloads (taken from the official client).
//! * [`crypto`] — `aead_aes256_gcm_rtpsize` / `aead_xchacha20_poly1305_rtpsize` transport encryption.
//! * [`dave`] — DAVE end-to-end encryption (MLS via the `davey` crate).
//! * [`audio`] — Opus, jitter buffering, mixing, VAD and cpal device I/O.
//! * [`connection`] — the orchestrator: [`VoiceConnection::connect`].
//!
//! The crate is independent of the rest of the client: give it the endpoint,
//! session id and token from the main gateway and it does the rest.

pub mod audio;
pub mod connection;
pub mod crypto;
pub mod dave;
pub mod protocol;
pub mod rtp;
pub mod udp;

pub use audio::{DeviceConfig, list_devices};
pub use connection::{AudioIo, ConnectInfo, Shared, VoiceConnection, VoiceEvent, VoiceSettings, VoiceState};
