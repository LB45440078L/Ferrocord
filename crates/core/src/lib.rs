//! The Ferrocord client core.
//!
//! [`Client`] owns the REST client, gateway connection and state store, exposes
//! high-level actions (send, edit, react, ack, presence…) and broadcasts
//! [`ClientEvent`]s. UIs, plugins and the local API are all just consumers of
//! this crate; they register as [`Extension`]s or subscribe to the event stream.

mod client;
pub mod config;
pub mod extension;

pub use client::{Client, ClientEvent, ClientOptions, Notification, error_text};
pub use config::{Config, Paths, Theme};
pub use extension::{Extension, Flow, LocalCommand, OutgoingMessage};

pub use ferrocord_auth as auth;
pub use ferrocord_gateway::{ConnectionState, GatewayHandle};
pub use ferrocord_http as http;
pub use ferrocord_model as model;
pub use ferrocord_store as store;
