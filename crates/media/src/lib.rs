//! # ferrocord-media
//!
//! The video media layer of Ferrocord. It sits between the platform (webcam, screen) and the
//! voice/RTC crate, and is deliberately transport-agnostic: it never touches sockets, RTP headers,
//! SRTP or Discord's DAVE end-to-end encryption.
//!
//! ```text
//!  camera ──RGBA VideoFrame──▶ h264::H264Encoder ──Annex-B──▶ (DAVE encrypt) ──▶ rtp_h264::H264Packetizer ──▶ RTP
//!  RTP ──▶ rtp_h264::H264Depacketizer ──Annex-B──▶ (DAVE decrypt) ──▶ h264::H264Decoder ──RGBA VideoFrame──▶ UI
//! ```
//!
//! ## Modules
//!
//! * [`VideoFrame`] / [`I420Frame`]: the frame types every module exchanges. `VideoFrame` is RGBA8 behind an
//!   `Arc`, so handing the same frame to a preview widget and to the encoder is free.
//! * [`convert`]: RGBA ↔ I420 (BT.601 limited range, what H.264 decoders in browsers assume when the
//!   bitstream carries no colour description), camera pixel formats (NV12, YUYV, RGB24, …) → RGBA and scaling
//!   for thumbnails. Pure Rust, no dependencies.
//! * [`rtp_h264`]: RFC 6184 payloader/depayloader (single NAL, STAP-A, FU-A; packetization-mode=1). Pure Rust,
//!   byte-exact for DAVE-transformed frames.
//! * `h264` (feature `h264`, default on): OpenH264 encoder/decoder configured for WebRTC interop
//!   (Constrained Baseline, `profile-level-id=42e01f` for ≤ 720p).
//! * `camera` (feature `camera`, default on): webcam enumeration and capture on a dedicated thread via
//!   nokhwa (MediaFoundation on Windows, V4L2 on Linux, AVFoundation on macOS).
//!
//! ## Cargo features
//!
//! | feature  | default | pulls in                         |
//! |----------|---------|----------------------------------|
//! | `camera` | yes     | `nokhwa` (platform backend), `zune-jpeg` |
//! | `h264`   | yes     | `openh264` (built from bundled C++ source, no NASM required) |
//!
//! ## Licensing note
//!
//! OpenH264 is BSD-2-Clause, but the H.264 format itself is covered by patents. Cisco only pays the
//! MPEG-LA royalties for its *own prebuilt* OpenH264 binaries; a build compiled from source (which is what the
//! `h264` feature does) is not covered. Distributors who are concerned about this can build with
//! `--no-default-features --features camera` and rely on a system/runtime-loaded codec instead. The
//! RTP packetizer is codec-syntax only and remains available without the `h264` feature.
//! nokhwa is Apache-2.0, zune-jpeg is MIT/Apache-2.0/Zlib.

#![warn(missing_docs)]

pub mod convert;
mod error;
mod frame;
pub mod rtp_h264;

#[cfg(feature = "h264")]
pub mod h264;

#[cfg(feature = "camera")]
pub mod camera;

pub use error::{MediaError, Result};
pub use frame::{I420Frame, MAX_DIMENSION, VideoFrame};
