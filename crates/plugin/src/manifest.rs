//! `plugin.toml` parsing and the permission model.

use std::fmt;
use std::path::{Component, Path};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::PluginError;

/// File name of the manifest inside a plugin directory.
pub const MANIFEST_FILE: &str = "plugin.toml";

/// A capability a plugin can ask for. A plugin only receives the Lua API for a
/// permission that is both declared in its manifest and granted by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    /// Receive `MESSAGE_*` gateway events.
    EventsMessages,
    /// Receive every client event (all gateway dispatches, connection state…).
    EventsAll,
    /// Send messages and replies.
    MessagesSend,
    /// Inspect, rewrite or cancel messages the user sends.
    MessagesModify,
    /// Register local slash commands.
    Commands,
    /// Read cached guilds, channels, users and messages.
    StateRead,
    /// Persistent per-plugin key-value storage.
    Storage,
    /// Show desktop notifications.
    Notify,
    /// Make outgoing HTTPS requests to arbitrary hosts.
    Http,
}

impl Permission {
    pub const ALL: [Permission; 9] = [
        Permission::EventsMessages,
        Permission::EventsAll,
        Permission::MessagesSend,
        Permission::MessagesModify,
        Permission::Commands,
        Permission::StateRead,
        Permission::Storage,
        Permission::Notify,
        Permission::Http,
    ];

    /// Name used in `plugin.toml` and `config.plugins.grants`.
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::EventsMessages => "events.messages",
            Permission::EventsAll => "events.all",
            Permission::MessagesSend => "messages.send",
            Permission::MessagesModify => "messages.modify",
            Permission::Commands => "commands",
            Permission::StateRead => "state.read",
            Permission::Storage => "storage",
            Permission::Notify => "notify",
            Permission::Http => "http",
        }
    }

    /// Human-readable explanation for permission prompts.
    pub fn description(self) -> &'static str {
        match self {
            Permission::EventsMessages => "Read messages as they are sent, edited and deleted",
            Permission::EventsAll => "Receive every event from Discord (presence, typing, guild changes…)",
            Permission::MessagesSend => "Send messages as you",
            Permission::MessagesModify => "Change or block messages before you send them",
            Permission::Commands => "Add slash commands to the message box",
            Permission::StateRead => "Read your servers, channels, users and cached messages",
            Permission::Storage => "Save its own data on disk",
            Permission::Notify => "Show desktop notifications",
            Permission::Http => "Connect to any website over HTTPS",
        }
    }

    /// Permissions that can leak data or act on the user's behalf.
    pub fn is_sensitive(self) -> bool {
        matches!(self, Permission::EventsAll | Permission::MessagesSend | Permission::MessagesModify | Permission::Http)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Permission {
    type Err = PluginError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Permission::ALL
            .into_iter()
            .find(|p| p.as_str() == s.trim())
            .ok_or_else(|| PluginError::UnknownPermission(s.to_string()))
    }
}

/// Contents of `plugin.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique id: lowercase ASCII letters, digits, `-`, `_` and `.`; must match the directory name.
    pub id: String,
    /// Display name (defaults to the id).
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Entry script, relative to the plugin directory.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// Requested permissions (see [`Permission`]).
    #[serde(default)]
    pub permissions: Vec<String>,
}

fn default_version() -> String {
    "0.0.0".into()
}

fn default_entry() -> String {
    "main.lua".into()
}

impl PluginManifest {
    /// Parses and validates a manifest.
    pub fn parse(src: &str) -> Result<Self, PluginError> {
        let mut m: PluginManifest = toml::from_str(src).map_err(|e| PluginError::Manifest(e.message().to_string()))?;
        if m.name.trim().is_empty() {
            m.name = m.id.clone();
        }
        m.validate()?;
        Ok(m)
    }

    /// Reads `<dir>/plugin.toml`.
    pub fn load(dir: &Path) -> Result<Self, PluginError> {
        let src = std::fs::read_to_string(dir.join(MANIFEST_FILE))?;
        Self::parse(&src)
    }

    fn validate(&self) -> Result<(), PluginError> {
        if !is_valid_id(&self.id) {
            return Err(PluginError::Manifest(format!(
                "invalid id `{}` (use 1-64 lowercase letters, digits, `-`, `_` or `.`)",
                self.id
            )));
        }
        let entry = Path::new(&self.entry);
        let safe = !self.entry.is_empty()
            && !self.entry.contains(':')
            && entry.components().all(|c| matches!(c, Component::Normal(_)));
        let ext_ok = entry.extension().is_some_and(|e| e == "lua" || e == "luau");
        if !safe || !ext_ok {
            return Err(PluginError::Manifest(format!(
                "invalid entry `{}` (must be a relative .lua/.luau path inside the plugin directory)",
                self.entry
            )));
        }
        Ok(())
    }

    /// Declared permissions that Ferrocord understands, deduplicated.
    pub fn declared_permissions(&self) -> Vec<Permission> {
        let mut v: Vec<Permission> = self.permissions.iter().filter_map(|p| p.parse().ok()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Declared permission strings that are not recognised (typos, newer versions).
    pub fn unknown_permissions(&self) -> Vec<String> {
        self.permissions.iter().filter(|p| p.parse::<Permission>().is_err()).cloned().collect()
    }

    pub fn declares(&self, p: Permission) -> bool {
        self.permissions.iter().any(|s| s.trim() == p.as_str())
    }
}

/// Whether `id` is an acceptable plugin id (also used as a directory name).
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_manifest() {
        let m = PluginManifest::parse(
            r#"
            id = "keyword-alerts"
            name = "Keyword Alerts"
            version = "1.2.0"
            author = "someone"
            description = "Notifies on keywords"
            permissions = ["events.messages", "notify", "storage", "storage", "bogus.perm"]
            "#,
        )
        .unwrap();
        assert_eq!(m.id, "keyword-alerts");
        assert_eq!(m.entry, "main.lua");
        assert_eq!(m.declared_permissions(), vec![Permission::EventsMessages, Permission::Storage, Permission::Notify]);
        assert_eq!(m.unknown_permissions(), vec!["bogus.perm".to_string()]);
        assert!(m.declares(Permission::Notify));
        assert!(!m.declares(Permission::Http));
    }

    #[test]
    fn defaults_and_validation() {
        let m = PluginManifest::parse("id = \"x\"").unwrap();
        assert_eq!(m.name, "x");
        assert_eq!(m.version, "0.0.0");
        assert!(m.permissions.is_empty());

        assert!(PluginManifest::parse("id = \"Bad Id\"").is_err());
        assert!(PluginManifest::parse("id = \"..\"").is_err());
        assert!(PluginManifest::parse("name = \"no id\"").is_err());
        assert!(PluginManifest::parse("id = \"a\"\nentry = \"../evil.lua\"").is_err());
        assert!(PluginManifest::parse("id = \"a\"\nentry = \"C:/evil.lua\"").is_err());
        assert!(PluginManifest::parse("id = \"a\"\nentry = \"main.txt\"").is_err());
        assert!(PluginManifest::parse("id = \"a\"\nentry = \"src/init.luau\"").is_ok());
    }

    #[test]
    fn permission_round_trip() {
        for p in Permission::ALL {
            assert_eq!(p.as_str().parse::<Permission>().unwrap(), p);
        }
        assert!("nope".parse::<Permission>().is_err());
    }
}
