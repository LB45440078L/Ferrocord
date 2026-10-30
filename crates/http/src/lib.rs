//! Discord REST client.
//!
//! * Presents the same client fingerprint as the desktop app ([`ClientProfile`]).
//! * Honours per-bucket and global rate limits.
//! * Transparently handles captcha challenges and MFA re-authentication by
//!   delegating to pluggable [`CaptchaSolver`] / [`MfaHandler`] implementations
//!   and replaying the request — exactly like the official client's response interceptor.

mod client;
pub mod error;
pub mod profile;
pub mod ratelimit;
pub mod routes;

pub use client::*;
pub use error::{ApiErrorBody, HttpError};
pub use profile::ClientProfile;
pub use routes::{MessageAnchor, SearchQuery, SearchResults, SearchScope};
