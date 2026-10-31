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

/// Directories used by Ferrocord.
///
/// | | Windows | macOS | Linux (XDG) |
/// |---|---|---|---|
/// | config | `%APPDATA%\Ferrocord` | `~/Library/Application Support/dev.ferrocord.Ferrocord` | `~/.config/ferrocord` |
/// | data | `%APPDATA%\Ferrocord` | `~/Library/Application Support/dev.ferrocord.Ferrocord` | `~/.local/share/ferrocord` |
/// | cache | `%LOCALAPPDATA%\Ferrocord\Cache` | `~/Library/Caches/dev.ferrocord.Ferrocord` | `~/.cache/ferrocord` |
/// | logs | `%LOCALAPPDATA%\Ferrocord\Logs` | `~/Library/Logs/Ferrocord` | `~/.local/state/ferrocord/logs` |
///
/// Config and data roam with a Windows profile; the cache and logs stay on the
/// machine. Account tokens never touch these folders: they live in the OS
/// credential vault (see `ferrocord-auth`).
///
/// Two overrides exist for portable installs: the `FERROCORD_HOME` environment
/// variable, or a file named `portable` next to the executable (which puts
/// everything in a `data` folder beside it).
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Self {
        if let Some(home) = std::env::var_os("FERROCORD_HOME").filter(|v| !v.is_empty()) {
            return Self::under(PathBuf::from(home));
        }
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from))
            && dir.join("portable").is_file()
        {
            return Self::under(dir.join("data"));
        }
        Self::platform().unwrap_or_else(|| Self::under(std::env::current_dir().unwrap_or_default().join("ferrocord-data")))
    }

    /// Everything under one root folder (portable mode, tests).
    pub fn under(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self { config_dir: root.join("config"), data_dir: root.join("data"), cache_dir: root.join("cache"), logs_dir: root.join("logs") }
    }

    #[cfg(windows)]
    fn platform() -> Option<Self> {
        let b = directories::BaseDirs::new()?;
        let roaming = b.config_dir().join("Ferrocord");
        let local = b.data_local_dir().join("Ferrocord");
        Some(Self { config_dir: roaming.clone(), data_dir: roaming, cache_dir: local.join("Cache"), logs_dir: local.join("Logs") })
    }

    #[cfg(target_os = "macos")]
    fn platform() -> Option<Self> {
        let b = directories::BaseDirs::new()?;
        let support = b.data_dir().join(BUNDLE_ID);
        Some(Self {
            config_dir: support.clone(),
            data_dir: support,
            cache_dir: b.cache_dir().join(BUNDLE_ID),
            logs_dir: b.home_dir().join("Library/Logs/Ferrocord"),
        })
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn platform() -> Option<Self> {
        let b = directories::BaseDirs::new()?;
        let state = b.state_dir().map(PathBuf::from).unwrap_or_else(|| b.home_dir().join(".local/state"));
        Some(Self {
            config_dir: b.config_dir().join("ferrocord"),
            data_dir: b.data_dir().join("ferrocord"),
            cache_dir: b.cache_dir().join("ferrocord"),
            logs_dir: state.join("ferrocord/logs"),
        })
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

    /// Window layout and UI state saved by eframe.
    pub fn ui_state_file(&self) -> PathBuf {
        self.data_dir.join("ui-state.ron")
    }

    /// Profile of the embedded WebView used for captchas.
    pub fn webview_dir(&self) -> PathBuf {
        self.cache_dir.join("webview")
    }

    /// Every folder Ferrocord writes to, deduplicated, for display and cleanup.
    pub fn all_dirs(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = Vec::new();
        for d in [&self.config_dir, &self.data_dir, &self.cache_dir, &self.logs_dir] {
            if !v.iter().any(|x| d.starts_with(x)) {
                v.retain(|x| !x.starts_with(d));
                v.push(d.clone());
            }
        }
        v
    }

    pub fn ensure(&self) {
        for d in [&self.config_dir, &self.data_dir, &self.cache_dir, &self.logs_dir, &self.plugins_dir(), &self.media_cache()] {
            let _ = std::fs::create_dir_all(d);
        }
    }
}

#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "dev.ferrocord.Ferrocord";

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_puts_everything_below_the_root() {
        let p = Paths::under("/portable/data");
        for dir in [&p.config_dir, &p.data_dir, &p.cache_dir, &p.logs_dir] {
            assert!(dir.starts_with("/portable/data"), "{}", dir.display());
        }
        assert_eq!(p.ui_state_file(), PathBuf::from("/portable/data/data/ui-state.ron"));
        assert_eq!(p.webview_dir(), PathBuf::from("/portable/data/cache/webview"));
    }

    #[test]
    fn all_dirs_drops_duplicates_and_nested_folders() {
        // Windows shares one roaming folder for config and data, with the cache and logs elsewhere.
        let roaming = PathBuf::from("/roaming/Ferrocord");
        let local = PathBuf::from("/local/Ferrocord");
        let p = Paths { config_dir: roaming.clone(), data_dir: roaming.clone(), cache_dir: local.join("Cache"), logs_dir: local.join("Logs") };
        assert_eq!(p.all_dirs(), vec![roaming.clone(), local.join("Cache"), local.join("Logs")]);

        // A folder inside another one is removed together with its parent.
        let p = Paths { config_dir: roaming.join("config"), data_dir: roaming.clone(), cache_dir: roaming.join("cache"), logs_dir: local };
        assert_eq!(p.all_dirs(), vec![roaming, PathBuf::from("/local/Ferrocord")]);
    }

    #[test]
    fn platform_dirs_are_named_after_ferrocord() {
        let Some(p) = Paths::platform() else { return };
        for dir in [&p.config_dir, &p.data_dir, &p.cache_dir, &p.logs_dir] {
            let s = dir.to_string_lossy().to_lowercase();
            assert!(s.contains("ferrocord"), "{s}");
        }
    }
}
