//! Channel sidebar: guild channel tree or DM list, plus the account panel.

use egui::{Color32, CornerRadius, FontId, Panel, RichText, Sense, Ui, pos2, vec2};
use ferrocord_core::model::{Channel, ChannelType, Snowflake, Status};

use crate::app::{App, Modal};
use crate::theme;
use crate::widgets;

pub fn sidebar(app: &mut App, ui: &mut Ui) {
    Panel::bottom("account").exact_size(56.0).show_separator_line(false).frame(egui::Frame::new().fill(app.pal.bg_rail.lerp_to_gamma(app.pal.bg_side, 0.5))).show(ui, |ui| {
        account_panel(app, ui);
    });
    Panel::top("sidebar_header").exact_size(48.0).show_separator_line(false).frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 0))).show(ui, |ui| {
        header(app, ui);
    });
    egui::CentralPanel::no_frame().show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("channels").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(6.0);
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 0)).show(ui, |ui| match app.nav.guild {
                Some(g) => guild_channels(app, ui, g),
                None => dm_list(app, ui),
            });
            ui.add_space(8.0);
        });
    });
}

fn header(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let rect = ui.max_rect();
    ui.painter().hline(rect.x_range(), rect.max.y - 0.5, egui::Stroke::new(1.0, pal.border));
    ui.horizontal_centered(|ui| match app.nav.guild {
        Some(g) => {
            let name = app.client.store().guilds.get(&g).map(|g| g.name().to_string()).unwrap_or_default();
            ui.label(RichText::new(name).font(FontId::new(15.5, theme::bold())));
        }
        None => {
            let b = egui::Button::new(RichText::new("Find or start a conversation").small().color(pal.text_faint)).fill(pal.bg_rail).min_size(vec2(ui.available_width(), 28.0));
            if ui.add(b).clicked() {
                app.modal = Some(Modal::QuickSwitcher { query: String::new(), index: 0 });
            }
        }
    });
}

fn guild_channels(app: &mut App, ui: &mut Ui, guild: Snowflake) {
    let tree = app.client.store().guild_channel_tree(guild);
    for (cat, channels) in tree {
        if let Some(cat) = &cat {
            let collapsed = app.collapsed.contains(&cat.id);
            ui.add_space(10.0);
            let name = cat.name.clone().unwrap_or_default().to_uppercase();
            let resp = ui.add(egui::Label::new(RichText::new(format!("{} {name}", if collapsed { "›" } else { "⌄" })).font(FontId::new(11.5, theme::bold())).color(app.pal.text_faint)).sense(Sense::click()));
            if resp.clicked() {
                if collapsed {
                    app.collapsed.remove(&cat.id);
                } else {
                    app.collapsed.insert(cat.id);
                }
            }
            if collapsed {
                // Collapsed categories still show the selected or unread channels.
                let visible: Vec<&Channel> = channels.iter().filter(|c| app.nav.channel == Some(c.id) || app.client.store().is_unread(c.id)).collect();
                for c in visible {
                    channel_row(app, ui, c, 0.0);
                }
                continue;
            }
        }
        for c in &channels {
            channel_row(app, ui, c, 0.0);
            let threads: Vec<Channel> = app.client.store().threads_of(c.id).into_iter().cloned().collect();
            for t in threads.iter().take(8) {
                channel_row(app, ui, t, 18.0);
            }
            if c.kind.is_voice() {
                voice_members(app, ui, c.id);
            }
        }
    }
}

fn channel_icon(kind: ChannelType, nsfw: bool) -> &'static str {
    match kind {
        ChannelType::GuildVoice => "🔊",
        ChannelType::GuildStageVoice => "📡",
        ChannelType::GuildAnnouncement => "📢",
        ChannelType::GuildForum | ChannelType::GuildMedia => "💬",
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::AnnouncementThread => "↳",
        _ if nsfw => "🔞",
        _ => "#",
    }
}

fn channel_row(app: &mut App, ui: &mut Ui, c: &Channel, indent: f32) {
    let pal = app.pal;
    let (unread, mentions, muted) = {
        let s = app.client.store();
        (s.is_unread(c.id), s.mention_count(c.id), s.is_muted(c.id))
    };
    let selected = app.nav.channel == Some(c.id);
    let (rect, resp) = widgets::list_row(ui, 32.0, selected, &pal);
    if unread && !selected && !muted {
        ui.painter().rect_filled(egui::Rect::from_center_size(pos2(rect.min.x - 6.0, rect.center().y), vec2(4.0, 8.0)), CornerRadius::same(2), pal.text);
    }
    let color = if selected || (unread && !muted) {
        pal.text
    } else if muted {
        pal.text_faint.gamma_multiply(0.7)
    } else {
        pal.text_muted
    };
    let x = rect.min.x + 8.0 + indent;
    ui.painter().text(pos2(x + 8.0, rect.center().y), egui::Align2::CENTER_CENTER, channel_icon(c.kind, c.nsfw), FontId::proportional(16.0), pal.text_faint);
    let font = if unread && !muted { FontId::new(14.5, theme::bold()) } else { FontId::proportional(14.5) };
    let name = c.name.clone().unwrap_or_default();
    let max_w = rect.max.x - (x + 22.0) - if mentions > 0 { 28.0 } else { 6.0 };
    let galley = ui.ctx().fonts_mut(|f| {
        let mut job = egui::text::LayoutJob::simple_singleline(name, font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
        f.layout_job(job)
    });
    ui.painter().galley(pos2(x + 22.0, rect.center().y - galley.size().y / 2.0), galley, color);
    if mentions > 0 {
        widgets::count_badge(ui, pos2(rect.max.x - 6.0, rect.center().y), mentions, &pal);
    }

    if resp.clicked() && !c.kind.is_voice() {
        app.select_channel(c.id);
    }
    let resp = if let Some(topic) = c.topic.as_ref().filter(|t| !t.is_empty()) { resp.on_hover_text(topic.chars().take(300).collect::<String>()) } else { resp };
    let client = app.client.clone();
    let (id, guild) = (c.id, c.guild_id);
    resp.context_menu(|ui| {
        if ui.button("Mark as read").clicked() {
            client.ack_channel(id);
            ui.close();
        }
        if ui.button(if muted { "Unmute channel" } else { "Mute channel" }).clicked() {
            client.set_channel_muted(guild, id, !muted);
            ui.close();
        }
        ui.separator();
        if ui.button("Copy link").clicked() {
            ui.ctx().copy_text(format!("https://discord.com/channels/{}/{id}", guild.map(|g| g.to_string()).unwrap_or("@me".into())));
            ui.close();
        }
        if ui.button("Copy channel ID").clicked() {
            ui.ctx().copy_text(id.to_string());
            ui.close();
        }
    });
}

fn voice_members(app: &mut App, ui: &mut Ui, channel: Snowflake) {
    let users: Vec<(Snowflake, bool, bool)> = {
        let s = app.client.store();
        s.voice_states.values().filter(|v| v.channel_id == Some(channel)).map(|v| (v.user_id, v.self_mute || v.mute, v.self_deaf || v.deaf)).collect()
    };
    for (u, mute, deaf) in users {
        let (url, name) = {
            let s = app.client.store();
            (s.user(u).map(|x| x.avatar_url(32)).unwrap_or_default(), s.display_name(app.nav.guild, u))
        };
        ui.horizontal(|ui| {
            ui.add_space(30.0);
            widgets::avatar(ui, &app.images, &url, 20.0, None, &app.pal);
            ui.label(RichText::new(name).size(13.0).color(app.pal.text_muted));
            if deaf {
                ui.label(RichText::new("🔇").size(11.0));
            } else if mute {
                ui.label(RichText::new("🎙").size(11.0).color(app.pal.danger));
            }
        });
    }
}

fn dm_list(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let selected_home = app.nav.channel.is_none();
    let (rect, resp) = widgets::list_row(ui, 40.0, selected_home, &pal);
    ui.painter().text(pos2(rect.min.x + 22.0, rect.center().y), egui::Align2::CENTER_CENTER, "👥", FontId::proportional(18.0), pal.text_muted);
    ui.painter().text(pos2(rect.min.x + 44.0, rect.center().y), egui::Align2::LEFT_CENTER, "Friends", FontId::new(15.0, theme::bold()), if selected_home { pal.text } else { pal.text_muted });
    let pending = app.client.store().relationships.values().filter(|r| r.kind == ferrocord_core::model::RelationshipType::IncomingRequest).count() as u32;
    if pending > 0 {
        widgets::count_badge(ui, pos2(rect.max.x - 8.0, rect.center().y), pending, &pal);
    }
    if resp.clicked() {
        app.nav.channel = None;
        app.nav.last_channel.remove(&None);
        app.client.set_focus(None, true);
    }
    ui.add_space(12.0);
    widgets::section_label(ui, "Direct Messages", &pal);
    ui.add_space(4.0);

    let rows: Vec<(Snowflake, String, String, Option<Status>, bool, u32, String)> = {
        let s = app.client.store();
        s.sorted_private_channels()
            .into_iter()
            .take(200)
            .map(|c| {
                let rids = c.recipient_ids();
                let (name, avatar, status, sub) = if c.kind == ChannelType::GroupDm {
                    let name = c.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| rids.iter().map(|u| s.display_name(None, *u)).collect::<Vec<_>>().join(", "));
                    let avatar = c.icon_url(64).unwrap_or_else(|| ferrocord_core::model::cdn::default_avatar(c.id, "0"));
                    (name, avatar, None, format!("{} Members", rids.len() + 1))
                } else {
                    let u = rids.first().copied().unwrap_or_default();
                    let avatar = s.user(u).map(|x| x.avatar_url(64)).unwrap_or_else(|| ferrocord_core::model::cdn::default_avatar(u, "0"));
                    let sub = s.presences.get(&u).and_then(|p| p.custom_status().and_then(|a| a.state.clone())).unwrap_or_default();
                    (s.display_name(None, u), avatar, Some(s.status(u)), sub)
                };
                (c.id, name, avatar, status, s.is_unread(c.id), s.mention_count(c.id), sub)
            })
            .collect()
    };
    for (id, name, avatar, status, unread, mentions, sub) in rows {
        let selected = app.nav.channel == Some(id);
        let (rect, resp) = widgets::list_row(ui, 44.0, selected, &pal);
        let av = egui::Rect::from_min_size(pos2(rect.min.x + 8.0, rect.center().y - 16.0), vec2(32.0, 32.0));
        let ring = if selected { pal.bg_selected } else if resp.hovered() { pal.bg_hover } else { pal.bg_side };
        widgets::paint_avatar(ui, &app.images, &avatar, av, status.map(|s| (s, ring)), &pal);
        let color = if selected || unread { pal.text } else { pal.text_muted };
        let font = if unread { FontId::new(14.5, theme::bold()) } else { FontId::proportional(14.5) };
        let text_x = av.max.x + 10.0;
        let max_w = rect.max.x - text_x - if mentions > 0 { 28.0 } else { 6.0 };
        let line = |text: String, font: FontId, color: Color32| {
            ui.ctx().fonts_mut(|f| {
                let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
                job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
                f.layout_job(job)
            })
        };
        if sub.is_empty() {
            let g = line(name, font, color);
            ui.painter().galley(pos2(text_x, rect.center().y - g.size().y / 2.0), g, color);
        } else {
            let g = line(name, font, color);
            ui.painter().galley(pos2(text_x, rect.center().y - g.size().y + 1.0), g, color);
            let g2 = line(sub, FontId::proportional(12.0), pal.text_faint);
            ui.painter().galley(pos2(text_x, rect.center().y + 1.0), g2, pal.text_faint);
        }
        if mentions > 0 {
            widgets::count_badge(ui, pos2(rect.max.x - 6.0, rect.center().y), mentions, &pal);
        }
        if resp.clicked() {
            app.select_channel(id);
        }
        let client = app.client.clone();
        resp.context_menu(|ui| {
            if ui.button("Mark as read").clicked() {
                client.ack_channel(id);
                ui.close();
            }
            if ui.button("Close DM").clicked() {
                client.spawn_action("close_dm", move |c| async move { c.http().close_channel(id).await });
                ui.close();
            }
            if ui.button("Copy channel ID").clicked() {
                ui.ctx().copy_text(id.to_string());
                ui.close();
            }
        });
    }
}

fn account_panel(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let (me, avatar, status, custom) = {
        let s = app.client.store();
        let me = s.me.clone();
        let id = s.my_id();
        let custom = s.settings.custom_status().map(|c| c.text.clone()).filter(|t| !t.is_empty());
        (me.clone(), me.map(|m| m.user.avatar_url(64)).unwrap_or_default(), s.status(id), custom)
    };
    let Some(me) = me else { return };
    ui.horizontal_centered(|ui| {
        ui.add_space(8.0);
        let ring = pal.bg_rail.lerp_to_gamma(pal.bg_side, 0.5);
        let resp = egui::Frame::new().show(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::avatar(ui, &app.images, &avatar, 34.0, Some((status, ring)), &pal);
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.label(RichText::new(me.user.display_name()).font(FontId::new(14.0, theme::bold())));
                    let sub = custom.clone().unwrap_or_else(|| status.label().to_string());
                    ui.label(RichText::new(sub).size(12.0).color(pal.text_faint));
                });
            });
        });
        let r = resp.response.interact(Sense::click());
        egui::Popup::menu(&r).show(|ui| {
            ui.set_min_width(200.0);
            for s in [Status::Online, Status::Idle, Status::Dnd, Status::Invisible] {
                let label = format!("{}  {}", match s { Status::Online => "🟢", Status::Idle => "🌙", Status::Dnd => "⛔", _ => "⚪" }, s.label());
                if ui.selectable_label(status == s, label).clicked() {
                    app.client.set_status(s);
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Set custom status…").clicked() {
                app.modal = Some(Modal::CustomStatus { text: custom.clone().unwrap_or_default() });
                ui.close();
            }
            if ui.button("Copy user ID").clicked() {
                ui.ctx().copy_text(me.user.id.to_string());
                ui.close();
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            if widgets::icon_button(ui, "⚙", "User settings", &pal, false).clicked() {
                app.modal = Some(Modal::Settings);
            }
        });
    });
}
