//! Wire types for Discord's user-facing API, shared by every Ferrocord crate.
//!
//! All structs are deliberately lenient (`#[serde(default)]`, `Option` fields) because
//! Discord frequently sends partial objects and adds fields without notice.

pub mod cdn;
mod channel;
pub mod gateway;
mod guild;
mod message;
mod permissions;
pub mod settings;
pub mod snowflake;
mod user;

pub use channel::*;
pub use gateway::Event;
pub use guild::*;
pub use message::*;
pub use permissions::*;
pub use snowflake::Snowflake;
pub use user::*;

/// REST API version used by the current official client.
pub const API_VERSION: u8 = 9;
