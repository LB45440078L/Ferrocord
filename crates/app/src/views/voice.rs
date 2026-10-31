//! Voice UI: connection panel, call view (participant grid) and call controls.

use egui::{Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};
use ferrocord_core::model::{ChannelType, Snowflake};

use crate::app::App;
use crate::icons::{self, Icon};
use crate::theme;
use crate::widgets;

/// Participants of a call: (user id, display name, avatar url, muted, deafened, video).
pub fn participants(app: &App, channel: Snowflake) -> Vec<(Snowflake, String, String, bool, bool, bool)> {
    let s = app.client.store();
    let guild = s.channels.get(&channel).and_then(|c| c.guild_id);
    let mut v: Vec<_> = s
        .voice_states
        .values()
        .filter(|vs| vs.channel_id == Some(channel))
        .map(|vs| {
            let avatar = vs
                .member
                .as_ref()
                .and_then(|m| guild.and_then(|g| m.avatar_url(g, 128)))
                .or_else(|| s.user(vs.user_id).map(|u| u.avatar_url(128)))
                .unwrap_or_else(|| ferrocord_core::model::cdn::default_avatar(vs.user_id, "0"));
            (vs.user_id, s.display_name(guild, vs.user_id), avatar, vs.self_mute || vs.mute, vs.self_deaf || vs.deaf, vs.self_video)
        })
        .collect();
    // Show ourselves while the voice state round-trips.
    if app.client.voice_channel() == Some(channel) && !v.iter().any(|p| p.0 == s.my_id()) {
        let me = s.my_id();
        let avatar = s.me.as_ref().map(|m| m.user.avatar_url(128)).unwrap_or_default();
        v.push((me, s.display_name(guild, me), avatar, app.client.self_mute(), app.client.self_deaf(), false));
    }
    v.sort_by_key(|p| p.1.to_lowercase());
    v
}

/// Whether `user` is talking right now (only known for the call we're in).
pub fn is_speaking(app: &App, channel: Snowflake, user: Snowflake) -> bool {
    if app.client.voice_channel() != Some(channel) {
        return false;
    }
    let Some(shared) = app.client.voice_shared() else { return false };
    if user == app.client.store().my_id() {
        return *shared.self_speaking.read();
    }
    shared.is_speaking(user.0)
}

/// Green ring drawn around the avatar of someone who is speaking.
pub fn speaking_ring(ui: &Ui, rect: Rect, speaking: bool, pal: &theme::Palette) {
    if speaking {
        ui.painter().circle_stroke(rect.center(), rect.width() / 2.0 + 2.0, Stroke::new(2.5, pal.online));
    }
}

/// Sidebar panel shown while connected to voice.
pub fn connection_panel(app: &mut App, ui: &mut Ui) {
    let Some(status) = app.client.voice_status() else { return };
    let pal = app.pal;
    let (label, color) = match status.state {
        "connected" => ("Voice Connected", pal.online),
        "reconnecting" => ("Reconnecting…", pal.warning),
        "disconnected" => ("Disconnected", pal.danger),
        _ => ("Connecting…", pal.warning),
    };
    let where_ = status
        .channel_id
        .map(|c| {
            let s = app.client.store();
            let ch = s.channels.get(&c);
            let name = ch.map(|c| c.display_name()).unwrap_or_default();
            match ch.and_then(|c| c.guild_id).and_then(|g| s.guilds.get(&g)) {
                Some(g) => format!("{name} / {}", g.name()),
                None => name,
            }
        })
        .unwrap_or_default();
    egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 8, top: 8, bottom: 6 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            icons::icon(ui, Icon::Signal, 16.0, color);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(label).font(FontId::new(13.5, theme::bold())).color(color));
                let r = ui.add(egui::Label::new(RichText::new(&where_).size(12.0).color(pal.text_faint)).truncate().sense(Sense::click()));
                if r.clicked()
                    && let Some(c) = status.channel_id
                {
                    app.select_channel(c);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::PhoneOff, "Disconnect", &pal, false).clicked() {
                    app.client.leave_voice();
                }
                let shield_color = if status.e2ee { pal.online } else { pal.text_faint };
                let tip = match (&status.privacy_code, status.e2ee) {
                    (Some(code), true) => format!("End-to-end encrypted (DAVE)\nVoice privacy code: {code}"),
                    (_, true) => "End-to-end encrypted (DAVE)".to_string(),
                    _ => "Transport-encrypted".to_string(),
                };
                icons::icon(ui, Icon::Shield, 16.0, shield_color).on_hover_text(tip);
                if let Some(p) = status.ping_ms {
                    ui.label(RichText::new(format!("{p} ms")).size(11.0).color(pal.text_faint));
                }
            });
        });
        if let Some(e) = status.error.as_ref().filter(|_| status.state != "connected") {
            ui.label(RichText::new(e).size(11.5).color(pal.danger));
        }
    });
    let r = ui.min_rect();
    ui.painter().hline(r.x_range(), r.max.y, Stroke::new(1.0, pal.border));
}

/// Mute / deafen toggles for the account panel.
pub fn self_controls(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let (mute, deaf) = (app.client.self_mute(), app.client.self_deaf());
    let r = toggle_button(ui, if deaf { Icon::HeadphonesOff } else { Icon::Headphones }, if deaf { "Undeafen" } else { "Deafen" }, deaf, &pal);
    if r.clicked() {
        app.client.set_self_deaf(!deaf);
    }
    let r = toggle_button(ui, if mute || deaf { Icon::MicOff } else { Icon::Mic }, if mute { "Unmute" } else { "Mute" }, mute || deaf, &pal);
    if r.clicked() {
        app.client.set_self_mute(!(mute || deaf));
    }
}

fn toggle_button(ui: &mut Ui, icon: Icon, tip: &str, on: bool, pal: &theme::Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), pal.bg_selected);
    }
    let color = if on {
        pal.danger
    } else if resp.hovered() {
        pal.text
    } else {
        pal.text_muted
    };
    icons::paint_centered(ui, rect.center(), 18.0, icon, color);
    resp.on_hover_text(tip)
}

/// Central view for a voice channel or DM call: participant tiles + controls.
pub fn call_view(app: &mut App, ui: &mut Ui, channel: Snowflake) {
    let pal = app.pal;
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, CornerRadius::ZERO, pal.bg_rail);
    let joined = app.client.voice_channel() == Some(channel);
    let people = participants(app, channel);
    let (name, private) = {
        let s = app.client.store();
        let c = s.channels.get(&channel);
        (c.map(|c| c.display_name()).unwrap_or_default(), c.is_some_and(|c| matches!(c.kind, ChannelType::Dm | ChannelType::GroupDm)))
    };

    egui::Panel::bottom("call_controls").exact_size(76.0).show_separator_line(false).frame(egui::Frame::new()).show(ui, |ui| {
        let bar_w = if joined { 4.0 * 56.0 } else { 160.0 };
        let left = ui.max_rect().center().x - bar_w / 2.0;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(left, ui.max_rect().min.y + 12.0), vec2(bar_w, 52.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;
        if joined {
            let (mute, deaf) = (app.client.self_mute(), app.client.self_deaf());
            if round_button(ui, if mute || deaf { Icon::MicOff } else { Icon::Mic }, mute || deaf, false, &pal).on_hover_text("Mute").clicked() {
                app.client.set_self_mute(!(mute || deaf));
            }
            if round_button(ui, if deaf { Icon::HeadphonesOff } else { Icon::Headphones }, deaf, false, &pal).on_hover_text("Deafen").clicked() {
                app.client.set_self_deaf(!deaf);
            }
            let video_on = app.client.voice_status().is_some_and(|s| s.self_video);
            if round_button(ui, if video_on { Icon::Video } else { Icon::VideoOff }, !video_on, false, &pal).on_hover_text("Camera").clicked() {
                app.toggle_camera();
            }
            if round_button(ui, Icon::PhoneOff, false, true, &pal).on_hover_text("Leave call").clicked() {
                app.client.leave_voice();
            }
        } else {
            let b = egui::Button::new(RichText::new(if private { "Join Call" } else { "Join Voice" }).color(Color32::WHITE).strong())
                .fill(pal.success)
                .corner_radius(CornerRadius::same(24))
                .min_size(vec2(160.0, 44.0));
            if ui.add(b).clicked() {
                app.client.join_voice(channel);
            }
        }
    });

    egui::CentralPanel::no_frame().show(ui, |ui| {
        let area = ui.max_rect().shrink(24.0);
        if people.is_empty() {
            ui.painter().text(area.center() - vec2(0.0, 12.0), egui::Align2::CENTER_CENTER, &name, FontId::new(22.0, theme::bold()), pal.text);
            ui.painter().text(area.center() + vec2(0.0, 16.0), egui::Align2::CENTER_CENTER, "No one is here yet.", FontId::proportional(14.0), pal.text_muted);
            return;
        }
        // Lay out tiles in a grid that fills the area at 16:9.
        let n = people.len();
        let mut best = (1usize, 0.0f32);
        for cols in 1..=n {
            let rows = n.div_ceil(cols);
            let w = ((area.width() - 12.0 * (cols - 1) as f32) / cols as f32).min((area.height() - 12.0 * (rows - 1) as f32) / rows as f32 * 16.0 / 9.0);
            if w > best.1 {
                best = (cols, w);
            }
        }
        let (cols, tile_w) = best;
        let tile = vec2(tile_w, tile_w * 9.0 / 16.0);
        let rows = n.div_ceil(cols);
        let total = vec2(cols as f32 * tile.x + (cols - 1) as f32 * 12.0, rows as f32 * tile.y + (rows - 1) as f32 * 12.0);
        let origin = area.center() - total / 2.0;
        for (i, (user, uname, avatar, muted, deafened, _video)) in people.iter().enumerate() {
            let (r, c) = (i / cols, i % cols);
            let in_row = if r == rows - 1 { n - r * cols } else { cols };
            let row_offset = (cols - in_row) as f32 * (tile.x + 12.0) / 2.0;
            let tr = Rect::from_min_size(origin + vec2(row_offset + c as f32 * (tile.x + 12.0), r as f32 * (tile.y + 12.0)), tile);
            let speaking = is_speaking(app, channel, *user);
            ui.painter().rect_filled(tr, CornerRadius::same(10), pal.bg_elevated);
            let frame = app.ext.video_frame(*user);
            if let Some(tex) = frame {
                let fitted = Rect::from_center_size(tr.center(), fit(tex.size, tr.size()));
                egui::Image::from_texture(egui::load::SizedTexture::new(tex.texture, fitted.size())).corner_radius(CornerRadius::same(10)).paint_at(ui, fitted);
            } else {
                let size = (tr.height() * 0.38).clamp(40.0, 128.0);
                let av = Rect::from_center_size(tr.center(), Vec2::splat(size));
                widgets::paint_avatar(ui, &app.images, avatar, av, None, &pal);
                speaking_ring(ui, av, speaking, &pal);
            }
            if speaking {
                ui.painter().rect_stroke(tr, CornerRadius::same(10), Stroke::new(3.0, pal.online), egui::StrokeKind::Inside);
            }
            // Name tag.
            let galley = ui.ctx().fonts_mut(|f| f.layout_no_wrap(uname.clone(), FontId::new(13.0, theme::bold()), Color32::WHITE));
            let tag =
                Rect::from_min_size(pos2(tr.min.x + 10.0, tr.max.y - 32.0), vec2(galley.size().x + 16.0 + if *muted || *deafened { 20.0 } else { 0.0 }, 24.0));
            ui.painter().rect_filled(tag, CornerRadius::same(6), Color32::from_black_alpha(150));
            let mut x = tag.min.x + 8.0;
            if *deafened || *muted {
                icons::paint_centered(ui, pos2(x + 7.0, tag.center().y), 14.0, if *deafened { Icon::HeadphonesOff } else { Icon::MicOff }, pal.danger);
                x += 20.0;
            }
            ui.painter().galley(pos2(x, tag.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
            let resp = ui.interact(tr, ui.scope_id().with(("tile", user)), Sense::click());
            if *user != app.client.store().my_id() && joined {
                resp.context_menu(|ui| {
                    ui.label(RichText::new(uname).strong());
                    let mut vol = app.ext.user_volume(*user);
                    if ui.add(egui::Slider::new(&mut vol, 0.0..=2.0).text("Volume").custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed() {
                        app.ext.set_user_volume(*user, vol);
                        app.client.set_user_volume(*user, vol);
                    }
                });
            }
        }
    });
}

fn fit(src: Vec2, max: Vec2) -> Vec2 {
    let s = (max.x / src.x).min(max.y / src.y);
    src * s
}

fn round_button(ui: &mut Ui, icon: Icon, active: bool, danger: bool, pal: &theme::Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::click());
    let r = Rect::from_center_size(rect.center(), Vec2::splat(46.0));
    let fill = if danger {
        pal.danger
    } else if active {
        Color32::WHITE
    } else if resp.hovered() {
        pal.bg_selected
    } else {
        pal.bg_elevated
    };
    ui.painter().circle_filled(r.center(), 23.0, fill);
    let fg = if danger {
        Color32::WHITE
    } else if active {
        pal.bg_rail
    } else {
        pal.text
    };
    icons::paint_centered(ui, r.center(), 22.0, icon, fg);
    resp
}
