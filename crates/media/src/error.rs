//! Error type shared by every module of the crate.

/// Errors produced by `ferrocord-media`.
///
/// Nothing in this crate panics on bad input; malformed frames, bitstreams and packets are reported through
/// this type (or, for the depacketizer, through events).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MediaError {
    /// No camera exists at the requested index (or no camera is attached at all).
    #[error("no camera found{}", .0.as_deref().map(|s| format!(": {s}")).unwrap_or_default())]
    NoCamera(Option<String>),
    /// The OS denied access to the camera (macOS privacy prompt declined, device blocked by policy, ...).
    #[error("camera access denied")]
    PermissionDenied,
    /// The camera backend reported an error (device busy, unplugged, unsupported format, ...).
    #[error("camera error: {0}")]
    Camera(String),
    /// The codec (OpenH264) rejected the configuration or failed to encode/decode.
    #[error("codec error: {0}")]
    Codec(String),
    /// The caller passed an inconsistent frame/buffer (wrong length, zero or oversized dimensions, ...).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The operation is not available on this platform or with the enabled features.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Convenience alias for `Result<T, MediaError>`.
pub type Result<T, E = MediaError> = std::result::Result<T, E>;
