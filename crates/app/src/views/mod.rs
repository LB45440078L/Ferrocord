//! UI views. Each module renders one region of the window from [`crate::app::App`] state.

pub mod channels;
pub mod chat;
pub mod forum;
pub mod friends;
pub mod guilds;
pub mod members;
pub mod modals;
pub mod popups;
pub mod settings;

use ferrocord_core::model::Snowflake;

use crate::app::{App, Modal};

/// Opens a link: Discord message/channel links and invites are handled in-app,
/// everything else goes to the system browser.
pub fn open_link(app: &mut App, url: &str) {
    let lower = url.to_ascii_lowercase();
    for prefix in ["https://discord.com/channels/", "https://canary.discord.com/channels/", "https://ptb.discord.com/channels/"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let parts: Vec<&str> = rest.split('/').collect();
            if let Some(ch) = parts.get(1).and_then(|c| c.parse::<u64>().ok()).map(Snowflake)
                && app.client.store().channels.contains_key(&ch)
            {
                app.select_channel(ch);
                if let Some(m) = parts.get(2).and_then(|m| m.parse::<u64>().ok()).map(Snowflake)
                    && app.client.store().message(ch, m).is_none()
                {
                    app.client.load_around(ch, m);
                }
                return;
            }
        }
    }
    for prefix in ["https://discord.gg/", "https://discord.com/invite/", "https://discordapp.com/invite/"] {
        if let Some(code) = url.strip_prefix(prefix) {
            app.modal = Some(Modal::JoinServer { code: code.trim_end_matches('/').to_string(), status: None });
            return;
        }
    }
    app.client.runtime().spawn_blocking({
        let url = url.to_string();
        move || {
            if let Err(e) = open::that_detached(&url) {
                tracing::warn!(error = %e, "failed to open browser");
            }
        }
    });
}
