//! Client identity: the user agent and "super properties" the official desktop
//! client sends. Discord uses these for analytics and anti-abuse heuristics, so we
//! present ourselves consistently across REST and the gateway.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ferrocord_model::gateway::SuperProperties;
use serde::{Deserialize, Serialize};

/// Fallbacks observed in the official stable desktop client (Oct 2026).
pub const DEFAULT_BUILD_NUMBER: u64 = 657_850;
pub const DEFAULT_CLIENT_VERSION: &str = "1.0.9266";
pub const DEFAULT_CHROME_VERSION: &str = "152.0.7977.167";
pub const DEFAULT_ELECTRON_VERSION: &str = "44.9.3";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientProfile {
    pub client_build_number: u64,
    pub client_version: String,
    pub chrome_version: String,
    pub electron_version: String,
    pub release_channel: String,
    pub locale: String,
    pub timezone: String,
    pub client_launch_id: String,
    pub client_heartbeat_session_id: String,
}

impl Default for ClientProfile {
    fn default() -> Self {
        Self::desktop()
    }
}

impl ClientProfile {
    pub fn desktop() -> Self {
        let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".into());
        Self {
            client_build_number: DEFAULT_BUILD_NUMBER,
            client_version: DEFAULT_CLIENT_VERSION.into(),
            chrome_version: DEFAULT_CHROME_VERSION.into(),
            electron_version: DEFAULT_ELECTRON_VERSION.into(),
            release_channel: "stable".into(),
            locale,
            timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into()),
            client_launch_id: uuid::Uuid::new_v4().to_string(),
            client_heartbeat_session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn os_name() -> &'static str {
        match std::env::consts::OS {
            "windows" => "Windows",
            "macos" => "Mac OS X",
            "linux" => "Linux",
            other => other,
        }
    }

    fn os_version() -> String {
        os_version_string()
    }

    pub fn user_agent(&self) -> String {
        let platform = match std::env::consts::OS {
            "windows" => "Windows NT 10.0; Win64; x64",
            "macos" => "Macintosh; Intel Mac OS X 10_15_7",
            _ => "X11; Linux x86_64",
        };
        format!(
            "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) discord/{} Chrome/{} Electron/{} Safari/537.36",
            self.client_version, self.chrome_version, self.electron_version
        )
    }

    /// Discord's language tag form ("en-US", "pt-BR"); the API rejects arbitrary BCP-47.
    pub fn discord_locale(&self) -> String {
        let l = self.locale.replace('_', "-");
        let mut parts = l.split('-');
        let lang = parts.next().unwrap_or("en").to_lowercase();
        match parts.next() {
            Some(region) if matches!(lang.as_str(), "en" | "es" | "pt" | "zh" | "sv") => {
                format!("{lang}-{}", region.to_uppercase())
            }
            _ => lang,
        }
    }

    pub fn super_properties(&self) -> SuperProperties {
        let os_version = Self::os_version();
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            a => a,
        };
        SuperProperties {
            os: Self::os_name().into(),
            browser: "Discord Client".into(),
            release_channel: self.release_channel.clone(),
            client_version: self.client_version.clone(),
            os_sdk_version: if cfg!(windows) { os_version.split('.').nth(2).map(str::to_string) } else { None },
            os_version,
            os_arch: arch.into(),
            app_arch: arch.into(),
            system_locale: self.locale.clone(),
            has_client_mods: false,
            client_launch_id: self.client_launch_id.clone(),
            browser_user_agent: self.user_agent(),
            browser_version: self.electron_version.clone(),
            client_build_number: self.client_build_number,
            native_build_number: None,
            client_event_source: None,
            launch_signature: None,
            client_heartbeat_session_id: Some(self.client_heartbeat_session_id.clone()),
            client_app_state: Some("focused".into()),
        }
    }

    pub fn super_properties_b64(&self) -> String {
        STANDARD.encode(serde_json::to_vec(&self.super_properties()).unwrap_or_default())
    }
}

#[cfg(windows)]
fn os_version_string() -> String {
    // RtlGetVersion is not lied to by compatibility shims (unlike GetVersionEx).
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    let mut info = OsVersionInfo { size: std::mem::size_of::<OsVersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
    // SAFETY: `info` is a properly sized, initialized OSVERSIONINFOW.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status == 0 { format!("{}.{}.{}", info.major, info.minor, info.build) } else { "10.0.19045".into() }
}

#[cfg(not(windows))]
fn os_version_string() -> String {
    std::process::Command::new("uname").arg("-r").output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Scrapes the current `BUILD_NUMBER` from the web app HTML so our fingerprint
/// tracks the live client instead of aging.
pub async fn fetch_build_number(client: &reqwest::Client) -> Option<u64> {
    let html = client.get("https://discord.com/app").send().await.ok()?.text().await.ok()?;
    let idx = html.find("\"BUILD_NUMBER\":\"")? + "\"BUILD_NUMBER\":\"".len();
    let end = html[idx..].find('"')? + idx;
    html[idx..end].parse().ok()
}
