//! User-safety UI shared by several views: Block / Ignore entries for user menus,
//! the profile-card buttons and the "leaving Ferrocord" link confirmation.

use std::sync::Mutex;

use egui::{CornerRadius, FontId, RichText, Stroke, Ui};
use ferrocord_core::Client;
use ferrocord_core::model::Snowflake;

use crate::app::App;
use crate::icons::Icon;
use crate::theme::{self, Palette};
use crate::widgets;

pub const IGNORE_HINT: &str = "Hides their messages and notifications on this device only";
/// How long a "Click again to confirm" button stays armed, in seconds.
const CONFIRM_WINDOW: f64 = 5.0;

/// Context-menu entries for a user: Block / Unblock, Ignore / Stop ignoring and Copy user ID.
///
/// Block asks for confirmation in a submenu (context menus close on any click, so a
/// second click on the same entry is not possible).
pub fn user_menu(ui: &mut Ui, client: &Client, user: Snowflake, pal: &Palette) {
    let (me, blocked, name) = {
        let s = client.store();
        (s.my_id(), s.is_blocked(user), s.display_name(None, user))
    };
    if user != me {
        if blocked {
            if widgets::menu_item(ui, Icon::Shield, "Unblock", pal.danger) {
                client.unblock_user(user);
                ui.close();
            }
        } else {
            ui.menu_button(RichText::new("Block").color(pal.danger), |ui| {
                ui.set_max_width(240.0);
                ui.label(RichText::new(format!("Block {name}?")).strong());
                ui.label(
                    RichText::new("They won't be able to message you, and their messages are collapsed. Blocking a friend removes them as a friend.")
                        .size(12.0)
                        .color(pal.text_muted),
                );
                if widgets::menu_item(ui, Icon::Shield, "Block", pal.danger) {
                    client.block_user(user);
                    ui.close();
                }
            });
        }
        let ignored = client.is_ignored(user);
        let r = ui.scope(|ui| widgets::menu_item(ui, Icon::VolumeOff, if ignored { "Stop ignoring" } else { "Ignore" }, pal.text_muted));
        if r.inner {
            client.set_ignored(user, !ignored);
            ui.close();
        }
        r.response.on_hover_text(IGNORE_HINT);
        ui.separator();
    }
    if widgets::menu_item(ui, Icon::Copy, "Copy user ID", pal.text_muted) {
        ui.ctx().copy_text(user.to_string());
        ui.close();
    }
}

/// Block / Ignore buttons for the profile card. Blocking needs a second click within a few seconds.
pub fn profile_buttons(ui: &mut Ui, client: &Client, user: Snowflake, pal: &Palette) {
    if client.store().my_id() == user {
        return;
    }
    let blocked = client.store().is_blocked(user);
    let armed_id = egui::Id::unique(("block_confirm", user));
    let now = ui.input(|i| i.time);
    let armed = ui.data(|d| d.get_temp::<f64>(armed_id)).is_some_and(|t| now - t < CONFIRM_WINDOW);
    let danger = |text: &str| egui::Button::new(RichText::new(text).color(pal.danger)).stroke(Stroke::new(1.0, pal.danger.gamma_multiply(0.5)));
    if blocked {
        if ui.add(danger("Unblock")).clicked() {
            client.unblock_user(user);
        }
    } else if armed {
        let r = ui.add(danger("Click again to confirm").fill(pal.danger.gamma_multiply(0.15)));
        if r.clicked() {
            ui.data_mut(|d| d.remove::<f64>(armed_id));
            client.block_user(user);
        }
        // Repaint so the button disarms on its own.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(CONFIRM_WINDOW));
    } else if ui.add(danger("Block")).on_hover_text("They won't be able to message you").clicked() {
        ui.data_mut(|d| d.insert_temp(armed_id, now));
    }
    let ignored = client.is_ignored(user);
    if ui.button(if ignored { "Stop ignoring" } else { "Ignore" }).on_hover_text(IGNORE_HINT).clicked() {
        client.set_ignored(user, !ignored);
    }
}

// ------------------------------------------------------------ external links

/// Link waiting for the user's confirmation. Only one window exists, so a global is enough
/// (and keeps `App` and the modal list untouched).
static PENDING_LINK: Mutex<Option<String>> = Mutex::new(None);

/// Host part of a URL, lowercased, without user info or port.
pub fn link_host(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = if host.starts_with('[') { host.split_inclusive(']').next().unwrap_or(host) } else { host.split(':').next().unwrap_or(host) };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Discord's own sites and CDN don't need a "leaving" confirmation.
pub fn is_trusted_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return false;
    }
    let host = link_host(url);
    ["discord.com", "discordapp.com", "discordapp.net", "discord.gg", "discord.media"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Queues `url` for the "Leaving Ferrocord" confirmation (see [`link_confirm`]).
pub fn ask_open_link(url: &str) {
    if let Ok(mut p) = PENDING_LINK.lock() {
        *p = Some(url.to_string());
    }
}

/// Opens a URL in the system browser without asking.
pub fn open_in_browser(client: &Client, url: &str) {
    client.runtime().spawn_blocking({
        let url = url.to_string();
        move || {
            if let Err(e) = open::that_detached(&url) {
                tracing::warn!(error = %e, "failed to open browser");
            }
        }
    });
}

/// Shows the external-link confirmation while a link is pending. Called every frame.
pub fn link_confirm(app: &mut App, ctx: &egui::Context) {
    let Some(url) = PENDING_LINK.lock().ok().and_then(|p| p.clone()) else { return };
    let pal = app.pal;
    let host = link_host(&url);
    let mut close = false;
    let mut open = false;
    let dont_ask_id = egui::Id::unique("confirm_external_link_dont_ask");
    let mut dont_ask = ctx.data(|d| d.get_temp::<bool>(dont_ask_id)).unwrap_or(false);
    let r = egui::Modal::new(egui::Id::unique("confirm_external_link"))
        .frame(
            egui::Frame::new()
                .fill(pal.bg_main)
                .corner_radius(CornerRadius::same(10))
                .stroke(Stroke::new(1.0, pal.border))
                .inner_margin(egui::Margin::same(18)),
        )
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Leaving Ferrocord").font(FontId::new(18.0, theme::bold())));
            ui.add_space(6.0);
            ui.label(RichText::new("This link is taking you to the following website:").color(pal.text_muted));
            ui.add_space(6.0);
            egui::Frame::new().fill(pal.bg_side).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !host.is_empty() {
                    ui.label(RichText::new(&host).strong());
                }
                ui.add(egui::Label::new(RichText::new(url.chars().take(500).collect::<String>()).size(12.5).color(pal.text_muted)).wrap());
            });
            ui.add_space(8.0);
            ui.checkbox(&mut dont_ask, "Don't ask again (Settings → Privacy)");
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::button(ui, "Visit site", true, &pal).clicked() {
                    open = true;
                }
                if widgets::button(ui, "Go back", false, &pal).clicked() {
                    close = true;
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(dont_ask_id, dont_ask));
    if open {
        if dont_ask {
            app.client.update_config(|c| c.privacy.confirm_external_links = false);
        }
        open_in_browser(&app.client, &url);
    }
    if open || close || r.should_close() {
        ctx.data_mut(|d| d.remove::<bool>(dont_ask_id));
        if let Ok(mut p) = PENDING_LINK.lock() {
            *p = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_hosts() {
        assert_eq!(link_host("https://Example.com/a?b"), "example.com");
        assert_eq!(link_host("https://user:pw@evil.com:8443/x"), "evil.com");
        assert_eq!(link_host("http://[::1]:80/"), "[::1]");
        assert!(is_trusted_link("https://cdn.discordapp.com/attachments/1/2/a.png"));
        assert!(is_trusted_link("https://discord.com/channels/@me"));
        assert!(!is_trusted_link("https://discord.com@evil.com/"));
        assert!(!is_trusted_link("https://discord.com.evil.com/"));
        assert!(!is_trusted_link("https://notdiscord.com/"));
        assert!(!is_trusted_link("javascript://discord.com/"));
    }
}
