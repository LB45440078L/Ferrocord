//! UI views. Each module renders one region of the window from [`crate::app::App`] state.

pub mod channels;
pub mod chat;
pub mod forum;
pub mod friends;
pub mod guilds;
pub mod image_viewer;
pub mod members;
pub mod modals;
pub mod popups;
pub mod safety;
pub mod settings;
pub mod voice;

use ferrocord_core::model::Snowflake;

use crate::app::{App, Modal};

/// Opens a link: Discord message/channel links and invites are handled in-app,
/// everything else goes to the system browser (after a confirmation unless
/// `privacy.confirm_external_links` is off).
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
    if app.client.config().privacy.confirm_external_links && !safety::is_trusted_link(url) {
        safety::ask_open_link(url);
    } else {
        safety::open_in_browser(&app.client, url);
    }
}
