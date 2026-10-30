//! Account authentication for Ferrocord.
//!
//! * [`login::login`] / [`login::verify_mfa`] — password + MFA (TOTP, SMS, backup codes).
//! * [`remote::start`] — QR-code login with the mobile app.
//! * [`TokenStore`] — tokens are kept in the OS credential vault, never in plain files.
//!
//! Captchas raised during login are solved through the `CaptchaSolver`
//! installed on the [`ferrocord_http::Http`] client.

pub mod login;
pub mod remote;

pub use login::{LoginOutcome, MfaMethod, MfaStep};

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("{0}")]
    InvalidCredentials(String),
    #[error("invalid two-factor code: {0}")]
    InvalidMfaCode(String),
    #[error("the token is invalid or has been revoked")]
    InvalidToken,
    #[error("captcha was cancelled")]
    CaptchaCancelled,
    #[error("new login location: {0}")]
    NewLoginLocation(String),
    #[error("this account is suspended")]
    Suspended(String),
    #[error("{0}")]
    Rejected(String),
    #[error(transparent)]
    Http(#[from] ferrocord_http::HttpError),
    #[error("credential store error: {0}")]
    Store(String),
}

const SERVICE: &str = "dev.ferrocord.client";

/// Stores account tokens in the platform credential vault
/// (Windows Credential Manager, macOS Keychain, Secret Service on Linux).
pub struct TokenStore;

impl TokenStore {
    fn entry(account: &str) -> Result<keyring::Entry, AuthError> {
        keyring::Entry::new(SERVICE, account).map_err(|e| AuthError::Store(e.to_string()))
    }

    pub fn save(account: &str, token: &str) -> Result<(), AuthError> {
        Self::entry(account)?.set_password(token).map_err(|e| AuthError::Store(e.to_string()))
    }

    pub fn load(account: &str) -> Option<String> {
        Self::entry(account).ok()?.get_password().ok()
    }

    pub fn delete(account: &str) -> Result<(), AuthError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::Store(e.to_string())),
        }
    }
}
