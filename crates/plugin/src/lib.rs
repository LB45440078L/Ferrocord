//! Sandboxed Luau plugins for Ferrocord.
//!
//! A plugin is a directory `<plugins_dir>/<id>/` containing a `plugin.toml`
//! manifest ([`PluginManifest`]) and an entry script. Each running plugin owns a
//! dedicated OS thread with its own sandboxed Luau VM; the client only talks to
//! it through a bounded channel, so a slow or broken plugin can never block or
//! crash the client. Callbacks run under a time budget enforced by the Luau
//! interrupt handler, and the VM has a fixed memory limit.
//!
//! Plugins only get the Lua API for [`Permission`]s that they declare in their
//! manifest *and* that the user granted (`config.plugins.grants`).
//!
//! The UI drives everything through [`PluginHost`]. See `docs/PLUGINS.md` for
//! the plugin author's reference.

mod host;
pub mod manifest;
mod proxy;
mod runtime;
mod storage;

pub use host::{PermissionStatus, PluginHost, PluginInfo, effective_permissions};
pub use manifest::{MANIFEST_FILE, Permission, PluginManifest};
pub use runtime::{CALLBACK_BUDGET, LOAD_BUDGET, LogLevel, MEMORY_LIMIT, Notifier, OUTGOING_TIMEOUT, PluginLog};
pub use storage::{MAX_STORAGE_BYTES, STORAGE_FILE};

/// Errors returned by [`PluginHost`] operations.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid plugin.toml: {0}")]
    Manifest(String),
    #[error("plugin `{0}` is not installed")]
    NotFound(String),
    #[error("unknown permission `{0}`")]
    UnknownPermission(String),
    #[error("plugin `{id}` does not declare the `{permission}` permission")]
    NotDeclared { id: String, permission: String },
    #[error("script error: {0}")]
    Script(String),
    #[error("plugin did not finish loading in time")]
    LoadTimeout,
    #[error("cannot install plugin: {0}")]
    Install(String),
}

#[cfg(test)]
mod tests;
