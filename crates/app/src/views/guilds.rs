//! Server rail: home button, guild icons, folders, unread pills and badges.

use egui::{Color32, CornerRadius, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use ferrocord_core::model::Snowflake;
use ferrocord_core::store::GuildListEntry;

use crate::app::{App, Modal};
use crate::icons::{self, Icon};
use crate::theme;
use crate::widgets;

const ICON: f32 = 46.0;

pub fn rail(app: &mut App, ui: &mut Ui) {
    egui::ScrollArea::vertical().id_salt("rail").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            // Home (DMs).
            let dm_mentions: u32 = {
                let s = app.client.store();
                s.private_channels.iter().map(|c| s.mention_count(*c)).sum()
            };
            if guild_button(app, ui, None, "Direct Messages", app.nav.guild.is_none(), false, dm_mentions).clicked() {
                app.select_guild(None);
            }
            separator(ui, &app.pal);

            let entries = app.client.store().guild_list();
            for e in entries {
                match e {
                    GuildListEntry::Guild(g) => guild_entry(app, ui, g),
                    GuildListEntry::Folder { id, name, color, guilds } => folder(app, ui, id, name, color, guilds),
                }
            }
            separator(ui, &app.pal);
            let add = action_circle(ui, Icon::Plus, &app.pal).on_hover_text("Join a server");
            if add.clicked() {
                app.modal = Some(Modal::JoinServer { code: String::new(), status: None });
            }
            ui.add_space(10.0);
        });
    });
}

fn separator(ui: &mut Ui, pal: &theme::Palette) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(32.0, 2.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(1), pal.border);
    ui.add_space(4.0);
}

fn guild_entry(app: &mut App, ui: &mut Ui, g: Snowflake) {
    let (name, unread, mentions) = {
        let s = app.client.store();
        let Some(guild) = s.guilds.get(&g) else { return };
        let (u, m) = s.guild_unread(g);
        (guild.name().to_string(), u, m)
    };
    let selected = app.nav.guild == Some(g);
    let resp = guild_button(app, ui, Some(g), &name, selected, unread, mentions);
    if resp.clicked() {
        app.select_guild(Some(g));
    }
    let client = app.client.clone();
    let muted = client.store().is_guild_muted(g);
    resp.context_menu(|ui| {
        if ui.button("Mark as read").clicked() {
            client.ack_guild(g);
            ui.close();
        }
        if ui.button(if muted { "Unmute server" } else { "Mute server" }).clicked() {
            client.set_guild_muted(g, !muted);
            ui.close();
        }
        ui.separator();
        if ui.button("Copy server ID").clicked() {
            ui.ctx().copy_text(g.to_string());
            ui.close();
        }
        ui.separator();
        if ui.button(egui::RichText::new("Leave server").color(theme::hex(0xf04f5c))).clicked() {
            client.spawn_action("leave_guild", move |c| async move { c.http().leave_guild(g).await });
            ui.close();
        }
    });
}

/// Draws a 46px guild/home icon with the left "pill" indicator.
fn guild_button(app: &App, ui: &mut Ui, guild: Option<Snowflake>, name: &str, selected: bool, unread: bool, mentions: u32) -> egui::Response {
    let pal = &app.pal;
    let (slot, resp) = ui.allocate_exact_size(vec2(72.0, ICON + 8.0), Sense::click());
    let icon_rect = Rect::from_center_size(slot.center(), vec2(ICON, ICON));
    let hovered = resp.hovered();

    // Left pill: tall when selected, medium on hover, dot when unread.
    let pill_h = if selected {
        40.0
    } else if hovered {
        20.0
    } else if unread {
        8.0
    } else {
        0.0
    };
    if pill_h > 0.0 {
        let r = Rect::from_center_size(pos2(slot.min.x + 2.0, slot.center().y), vec2(8.0, pill_h));
        ui.painter().rect_filled(r, CornerRadius::same(4), pal.text);
    }

    let radius = if selected || hovered { 15 } else { (ICON / 2.0) as u8 };
    let icon_url = guild.and_then(|g| app.client.store().guilds.get(&g).and_then(|g| g.icon_url(96)));
    match (guild, icon_url.and_then(|u| app.images.get(&u))) {
        (Some(_), Some(img)) => {
            egui::Image::from_texture(egui::load::SizedTexture::new(img.texture, icon_rect.size()))
                .corner_radius(CornerRadius::same(radius))
                .paint_at(ui, icon_rect);
        }
        (Some(g), None) => {
            let fill = if selected || hovered { pal.accent } else { pal.bg_elevated };
            ui.painter().rect_filled(icon_rect, CornerRadius::same(radius), fill);
            let acr = app.client.store().guilds.get(&g).map(|g| g.acronym()).unwrap_or_default();
            let size = if acr.chars().count() > 3 { 12.0 } else { 15.0 };
            ui.painter().text(icon_rect.center(), egui::Align2::CENTER_CENTER, acr, FontId::new(size, theme::bold()), pal.text);
        }
        (None, _) => {
            let fill = if selected || hovered { pal.accent } else { pal.bg_elevated };
            ui.painter().rect_filled(icon_rect, CornerRadius::same(radius), fill);
            icons::paint_centered(ui, icon_rect.center(), 24.0, Icon::Home, if selected || hovered { Color32::WHITE } else { pal.text_muted });
        }
    }
    if mentions > 0 {
        let badge_center = pos2(icon_rect.max.x - 2.0, icon_rect.max.y - 6.0);
        let r = widgets::count_badge(ui, badge_center + vec2(8.0, 0.0), mentions, pal);
        ui.painter().rect_stroke(r.expand(1.5), CornerRadius::same(9), Stroke::new(3.0, pal.bg_rail), egui::StrokeKind::Outside);
    }
    resp.on_hover_text(name)
}

fn folder(app: &mut App, ui: &mut Ui, id: i64, name: Option<String>, color: Option<u32>, guilds: Vec<Snowflake>) {
    let open = app.open_folders.contains(&id);
    let pal = app.pal;
    let fcolor = color.and_then(theme::role_color).unwrap_or(pal.accent);
    let (unread, mentions) = {
        let s = app.client.store();
        guilds.iter().fold((false, 0u32), |(u, m), g| {
            let (gu, gm) = s.guild_unread(*g);
            (u || gu, m + gm)
        })
    };
    let bg_slot = ui.painter().add(egui::Shape::Noop);
    let (slot, resp) = ui.allocate_exact_size(vec2(72.0, ICON + 8.0), Sense::click());
    let icon = Rect::from_center_size(slot.center(), vec2(ICON, ICON));
    if !open && unread {
        let r = Rect::from_center_size(pos2(slot.min.x + 2.0, slot.center().y), vec2(8.0, 8.0));
        ui.painter().rect_filled(r, CornerRadius::same(4), pal.text);
    }
    ui.painter().rect_filled(icon, CornerRadius::same(15), fcolor.gamma_multiply(if open { 0.25 } else { 0.35 }));
    if open {
        icons::paint_centered(ui, icon.center(), 22.0, Icon::Folder, fcolor);
    } else {
        // 2x2 mini icons preview.
        let s = app.client.store();
        for (i, g) in guilds.iter().take(4).enumerate() {
            let cell = Rect::from_min_size(icon.min + vec2(5.0 + (i % 2) as f32 * 19.0, 5.0 + (i / 2) as f32 * 19.0), vec2(17.0, 17.0));
            let url = s.guilds.get(g).and_then(|g| g.icon_url(32));
            match url.and_then(|u| app.images.get(&u)) {
                Some(img) => {
                    egui::Image::from_texture(egui::load::SizedTexture::new(img.texture, cell.size())).corner_radius(CornerRadius::same(6)).paint_at(ui, cell)
                }
                None => {
                    ui.painter().rect_filled(cell, CornerRadius::same(6), pal.bg_elevated);
                }
            }
        }
    }
    if mentions > 0 && !open {
        widgets::count_badge(ui, pos2(icon.max.x + 6.0, icon.max.y - 6.0), mentions, &pal);
    }
    let resp = resp.on_hover_text(name.unwrap_or_else(|| "Server folder".into()));
    if resp.clicked() {
        if open {
            app.open_folders.remove(&id);
        } else {
            app.open_folders.insert(id);
        }
    }
    if open {
        for g in guilds {
            guild_entry(app, ui, g);
        }
        let bottom = ui.cursor().min.y;
        let bg = Rect::from_min_max(pos2(slot.center().x - 26.0, slot.min.y + 2.0), pos2(slot.center().x + 26.0, bottom));
        ui.painter().set(bg_slot, egui::Shape::rect_filled(bg, CornerRadius::same(16), fcolor.gamma_multiply(0.08)));
    }
}

fn action_circle(ui: &mut Ui, glyph: Icon, pal: &theme::Palette) -> egui::Response {
    let (slot, resp) = ui.allocate_exact_size(vec2(72.0, ICON + 8.0), Sense::click());
    let icon = Rect::from_center_size(slot.center(), vec2(ICON, ICON));
    let h = resp.hovered();
    ui.painter().rect_filled(icon, CornerRadius::same(if h { 15 } else { 23 }), if h { pal.success } else { pal.bg_elevated });
    icons::paint_centered(ui, icon.center(), 22.0, glyph, if h { Color32::WHITE } else { pal.success });
    resp
}
