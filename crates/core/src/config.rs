//! Persistent, user-editable configuration (`config.toml` in the platform config dir).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Id of the account to auto-login with (token lives in the OS credential vault).
    pub last_account: Option<String>,
    pub appearance: Appearance,
    pub notifications: Notifications,
    pub api: ApiConfig,
    pub plugins: PluginConfig,
    pub privacy: Privacy,
    pub voice: VoiceConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Midnight,
    Light,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    /// UI zoom factor (1.0 = 100%).
    pub zoom: f32,
    pub compact_messages: bool,
    pub show_avatars: bool,
    pub animate_emoji: bool,
    pub show_member_list: bool,
    pub inline_media: bool,
    pub use_24h_time: bool,
    /// Accent color as `#rrggbb`.
    pub accent: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            zoom: 1.0,
            compact_messages: false,
            show_avatars: true,
            animate_emoji: true,
            show_member_list: true,
            inline_media: true,
            use_24h_time: true,
            accent: "#5865f2".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub desktop: bool,
    pub sound: bool,
    pub taskbar_badge: bool,
    /// Show message text in notifications.
    pub show_content: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self { desktop: true, sound: true, taskbar_badge: true, show_content: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Local control API (HTTP + WebSocket on 127.0.0.1). Off by default.
    pub enabled: bool,
    pub port: u16,
    /// Allow API clients to send messages/perform actions (not just read).
    pub allow_actions: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { enabled: false, port: 6473, allow_actions: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PluginConfig {
    pub enabled: Vec<String>,
    /// Permissions granted per plugin id (see plugin docs).
    pub grants: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    /// Send typing indicators.
    pub send_typing: bool,
    /// Mark channels read automatically when viewed.
    pub auto_ack: bool,
}

impl Default for Privacy {
    fn default() -> Self {
        Self { send_typing: true, auto_ack: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Input device name (`None` = system default).
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// Microphone gain multiplier (1.0 = unchanged).
    pub input_gain: f32,
    /// Speaker volume multiplier.
    pub output_volume: f32,
    /// Voice activity threshold in dBFS; quieter input is not transmitted.
    pub sensitivity_db: f32,
    /// Camera index used for video.
    pub camera: Option<u32>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self { input_device: None, output_device: None, input_gain: 1.0, output_volume: 1.0, sensitivity_db: -50.0, camera: None }
    }
}

/// Platform directories used by Ferrocord.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Self {
        match directories::ProjectDirs::from("dev", "ferrocord", "Ferrocord") {
            Some(d) => Self { config_dir: d.config_dir().to_path_buf(), data_dir: d.data_dir().to_path_buf(), cache_dir: d.cache_dir().to_path_buf() },
            None => {
                let base = std::env::current_dir().unwrap_or_default().join("ferrocord-data");
                Self { config_dir: base.clone(), data_dir: base.clone(), cache_dir: base.join("cache") }
            }
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.data_dir.join("plugins")
    }

    pub fn media_cache(&self) -> PathBuf {
        self.cache_dir.join("media")
    }

    pub fn ensure(&self) {
        for d in [&self.config_dir, &self.data_dir, &self.cache_dir, &self.plugins_dir(), &self.media_cache()] {
            let _ = std::fs::create_dir_all(d);
        }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Self {
        match std::fs::read_to_string(paths.config_file()) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "config.toml is invalid; using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, paths: &Paths) {
        match toml::to_string_pretty(self) {
            Ok(s) => {
                let tmp = paths.config_file().with_extension("toml.tmp");
                if std::fs::write(&tmp, s).is_ok() {
                    let _ = std::fs::rename(&tmp, paths.config_file());
                }
            }
            Err(e) => tracing::error!(error = %e, "failed to serialize config"),
        }
    }
}
