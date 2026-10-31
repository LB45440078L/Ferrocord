//! Modal dialogs.

use egui::{CornerRadius, FontId, RichText, Vec2, vec2};
use ferrocord_core::model::{ChannelType, Snowflake};

use crate::app::{App, Modal};
use crate::theme;
use crate::widgets;

fn frame(app: &App) -> egui::Frame {
    egui::Frame::new()
        .fill(app.pal.bg_side)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(20))
        .stroke(egui::Stroke::new(1.0, app.pal.border))
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    // Results delivered by background tasks.
    while let Ok((ch, res)) = app.ext.pins_rx.try_recv() {
        if let Some(Modal::Pins { channel, messages }) = &mut app.modal
            && *channel == ch
        {
            match res {
                Ok(m) => *messages = Some(m),
                Err(e) => {
                    *messages = Some(vec![]);
                    app.toasts.push(crate::app::Toast { text: e, error: true, at: std::time::Instant::now() });
                }
            }
        }
    }
    while let Ok(status) = app.ext.join_rx.try_recv() {
        if let Some(Modal::JoinServer { status: s, .. }) = &mut app.modal {
            *s = Some(status);
        }
    }

    while let Ok(res) = app.ext.search_rx.try_recv() {
        if let Some(Modal::Search { results, searching, .. }) = &mut app.modal {
            *searching = false;
            match res {
                Ok(r) => *results = Some(r),
                Err(e) => app.toasts.push(crate::app::Toast { text: e, error: true, at: std::time::Instant::now() }),
            }
        }
    }

    let Some(modal) = app.modal.take() else { return };
    let mut keep = true;
    let modal = match modal {
        Modal::Settings => {
            keep = crate::views::settings::show(app, ctx);
            Modal::Settings
        }
        Modal::QuickSwitcher { mut query, mut index } => {
            keep = quick_switcher(app, ctx, &mut query, &mut index);
            Modal::QuickSwitcher { query, index }
        }
        Modal::ConfirmDelete { channel, message } => {
            let r = egui::Modal::new(egui::Id::unique("confirm_delete")).frame(frame(app)).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("Delete Message").font(FontId::new(18.0, theme::bold())));
                ui.label(RichText::new("Are you sure you want to delete this message?").color(app.pal.text_muted));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, "Cancel", false, &app.pal).clicked() {
                        keep = false;
                    }
                    let del = ui.add(egui::Button::new(RichText::new("Delete").color(egui::Color32::WHITE).strong()).fill(app.pal.danger));
                    if del.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if app.demo {
                            if let Some(cm) = app.client.store_mut().messages.get_mut(&channel) {
                                cm.messages.retain(|m| m.id != message);
                            }
                        } else {
                            app.client.delete_message(channel, message);
                        }
                        keep = false;
                    }
                });
            });
            if r.should_close() {
                keep = false;
            }
            Modal::ConfirmDelete { channel, message }
        }
        Modal::ImageViewer { url, open_url } => {
            let r = egui::Modal::new(egui::Id::unique("image_viewer")).frame(egui::Frame::NONE).backdrop_color(egui::Color32::from_black_alpha(200)).show(
                ctx,
                |ui| {
                    let screen = ctx.content_rect().size() * 0.85;
                    match app.images.get(&url) {
                        Some(img) => {
                            let resp = ui.add(egui::Image::from_texture(img.sized(screen)).sense(egui::Sense::click()));
                            if resp.clicked() {
                                keep = false;
                            }
                        }
                        None => {
                            ui.add_sized(Vec2::splat(120.0), egui::Spinner::new());
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.link(RichText::new("Open original").color(egui::Color32::WHITE)).clicked() {
                            crate::views::open_link(app, &open_url);
                        }
                        if ui.link(RichText::new("Copy link").color(egui::Color32::WHITE)).clicked() {
                            ctx.copy_text(open_url.clone());
                        }
                    });
                },
            );
            if r.should_close() {
                keep = false;
            }
            Modal::ImageViewer { url, open_url }
        }
        Modal::Pins { channel, messages } => {
            let mut jump = None;
            let r = egui::Modal::new(egui::Id::unique("pins")).frame(frame(app)).show(ctx, |ui| {
                ui.set_width(520.0);
                ui.horizontal(|ui| {
                    crate::icons::icon(ui, crate::icons::Icon::Pin, 20.0, app.pal.text_muted);
                    ui.label(RichText::new("Pinned Messages").font(FontId::new(18.0, theme::bold())));
                });
                ui.add_space(8.0);
                match &messages {
                    None => {
                        ui.spinner();
                    }
                    Some(list) if list.is_empty() => {
                        ui.label(RichText::new("This channel doesn't have any pinned messages… yet.").color(app.pal.text_muted));
                    }
                    Some(list) => {
                        egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                            for m in list {
                                egui::Frame::new().fill(app.pal.bg_elevated).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(
                                    ui,
                                    |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            widgets::avatar(ui, &app.images, &m.author.avatar_url(48), 28.0, None, &app.pal);
                                            ui.label(RichText::new(m.author.display_name()).strong());
                                            ui.label(
                                                RichText::new(m.created_at().with_timezone(&chrono::Local).format("%d/%m/%Y").to_string())
                                                    .small()
                                                    .color(app.pal.text_faint),
                                            );
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if ui.small_button("Jump").clicked() {
                                                    jump = Some(m.id);
                                                }
                                            });
                                        });
                                        ui.label(&m.content);
                                    },
                                );
                                ui.add_space(6.0);
                            }
                        });
                    }
                }
            });
            if r.should_close() {
                keep = false;
            }
            if let Some(id) = jump {
                keep = false;
                if app.client.store().message(channel, id).is_none() {
                    app.client.load_around(channel, id);
                }
            }
            Modal::Pins { channel, messages }
        }
        Modal::JoinServer { mut code, status } => {
            let r = egui::Modal::new(egui::Id::unique("join")).frame(frame(app)).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new("Join a Server").font(FontId::new(18.0, theme::bold())));
                ui.label(RichText::new("Enter an invite link or code.").color(app.pal.text_muted));
                ui.add_space(8.0);
                ui.add(egui::TextEdit::singleline(&mut code).hint_text("https://discord.gg/hTKzmak").desired_width(f32::INFINITY));
                if let Some(s) = &status {
                    ui.label(RichText::new(s).color(app.pal.text_muted));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, "Cancel", false, &app.pal).clicked() {
                        keep = false;
                    }
                    if widgets::button(ui, "Join Server", true, &app.pal).clicked() && !code.trim().is_empty() {
                        let c = code.trim().rsplit('/').next().unwrap_or_default().to_string();
                        let client = app.client.clone();
                        let tx = app.ext.join_tx.clone();
                        app.client.runtime().spawn(async move {
                            let msg = match client.http().accept_invite(&c).await {
                                Ok(v) => format!("Joined {}!", v.get("guild").and_then(|g| g.get("name")).and_then(|n| n.as_str()).unwrap_or("the server")),
                                Err(e) => format!("Couldn't join: {}", ferrocord_core::error_text(&e)),
                            };
                            let _ = tx.send(msg);
                            client.request_repaint();
                        });
                    }
                });
            });
            if r.should_close() {
                keep = false;
            }
            Modal::JoinServer { code, status }
        }
        Modal::Search { mut query, results, mut searching } => {
            let mut jump: Option<(Snowflake, Snowflake)> = None;
            let r = egui::Modal::new(egui::Id::unique("search")).frame(frame(app)).show(ctx, |ui| {
                ui.set_width(600.0);
                let scope_label = if app.nav.guild.is_some() { "Search this server" } else { "Search this conversation" };
                ui.label(RichText::new(scope_label).font(FontId::new(17.0, theme::bold())));
                ui.add_space(6.0);
                let te = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search messages").desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)));
                te.request_focus();
                if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !query.trim().is_empty() && !app.demo {
                    searching = true;
                    let scope = match (app.nav.guild, app.nav.channel) {
                        (Some(g), _) => ferrocord_core::http::SearchScope::Guild(g),
                        (None, Some(c)) => ferrocord_core::http::SearchScope::Channel(c),
                        _ => ferrocord_core::http::SearchScope::Channel(Snowflake(0)),
                    };
                    let q = ferrocord_core::http::SearchQuery { content: Some(query.trim().to_string()), ..Default::default() };
                    let (client, tx) = (app.client.clone(), app.ext.search_tx.clone());
                    app.client.runtime().spawn(async move {
                        let r = client.http().search(scope, &q).await;
                        let _ = tx.send(
                            r.map(|r| {
                                (r.total_results, r.messages.into_iter().filter_map(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) }).collect())
                            })
                            .map_err(|e| ferrocord_core::error_text(&e)),
                        );
                        client.request_repaint();
                    });
                }
                if searching {
                    ui.spinner();
                }
                if let Some((total, msgs)) = &results {
                    ui.label(RichText::new(format!("{total} results")).small().color(app.pal.text_faint));
                    egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                        for m in msgs {
                            let resp = egui::Frame::new()
                                .fill(app.pal.bg_elevated)
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(egui::Margin::same(10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        widgets::avatar(ui, &app.images, &m.author.avatar_url(48), 24.0, None, &app.pal);
                                        ui.label(RichText::new(m.author.display_name()).strong());
                                        let ch = app
                                            .client
                                            .store()
                                            .channels
                                            .get(&m.channel_id)
                                            .and_then(|c| c.name.clone())
                                            .map(|n| format!("#{n}"))
                                            .unwrap_or_default();
                                        ui.label(RichText::new(ch).small().color(app.pal.text_faint));
                                        ui.label(
                                            RichText::new(m.created_at().with_timezone(&chrono::Local).format("%d/%m/%Y %H:%M").to_string())
                                                .small()
                                                .color(app.pal.text_faint),
                                        );
                                    });
                                    ui.label(m.content.chars().take(400).collect::<String>());
                                });
                            if resp.response.interact(egui::Sense::click()).clicked() {
                                jump = Some((m.channel_id, m.id));
                            }
                            ui.add_space(4.0);
                        }
                    });
                }
            });
            if r.should_close() {
                keep = false;
            }
            if let Some((ch, id)) = jump {
                keep = false;
                if app.client.store().channels.contains_key(&ch) {
                    app.select_channel(ch);
                    if app.client.store().message(ch, id).is_none() {
                        app.client.load_around(ch, id);
                    }
                }
            }
            Modal::Search { query, results, searching }
        }
        Modal::IncomingCall { channel } => {
            let (name, avatar) = {
                let s = app.client.store();
                let c = s.channels.get(&channel);
                let avatar = c.and_then(|c| {
                    if c.kind == ChannelType::Dm { c.recipient_ids().first().and_then(|u| s.user(*u).map(|u| u.avatar_url(160))) } else { c.icon_url(160) }
                });
                (
                    c.map(|c| {
                        if c.kind == ChannelType::Dm {
                            c.recipient_ids().first().map(|u| s.display_name(None, *u)).unwrap_or_default()
                        } else {
                            c.display_name()
                        }
                    })
                    .unwrap_or_default(),
                    avatar,
                )
            };
            let r = egui::Modal::new(egui::Id::unique("incoming_call")).frame(frame(app)).show(ctx, |ui| {
                ui.set_width(300.0);
                ui.vertical_centered(|ui| {
                    if let Some(a) = &avatar {
                        widgets::avatar(ui, &app.images, a, 96.0, None, &app.pal);
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new(&name).font(FontId::new(18.0, theme::bold())));
                    ui.label(RichText::new("Incoming call…").color(app.pal.text_muted));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 2.0 * 120.0 - 8.0).max(0.0) / 2.0);
                        let decline =
                            egui::Button::new(RichText::new("Decline").color(egui::Color32::WHITE).strong()).fill(app.pal.danger).min_size(vec2(120.0, 36.0));
                        if ui.add(decline).clicked() {
                            app.client.decline_call(channel);
                            keep = false;
                        }
                        let accept = egui::Button::new(RichText::new("Join Call").color(egui::Color32::WHITE).strong())
                            .fill(app.pal.success)
                            .min_size(vec2(120.0, 36.0));
                        if ui.add(accept).clicked() {
                            app.client.join_voice(channel);
                            app.select_channel(channel);
                            keep = false;
                        }
                    });
                });
            });
            if r.should_close() {
                keep = false;
            }
            Modal::IncomingCall { channel }
        }
        Modal::CustomStatus { mut text } => {
            let r = egui::Modal::new(egui::Id::unique("custom_status")).frame(frame(app)).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new("Set a custom status").font(FontId::new(18.0, theme::bold())));
                ui.add_space(8.0);
                ui.add(egui::TextEdit::singleline(&mut text).hint_text("What's cookin'?").desired_width(f32::INFINITY).char_limit(128));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, "Clear", false, &app.pal).clicked() {
                        app.client.set_custom_status(None, None);
                        keep = false;
                    }
                    if widgets::button(ui, "Save", true, &app.pal).clicked() {
                        app.client.set_custom_status(Some(text.clone()).filter(|t| !t.trim().is_empty()), None);
                        keep = false;
                    }
                });
            });
            if r.should_close() {
                keep = false;
            }
            Modal::CustomStatus { text }
        }
    };
    if keep && app.modal.is_none() {
        app.modal = Some(modal);
    }
}

fn quick_switcher(app: &mut App, ctx: &egui::Context, query: &mut String, index: &mut usize) -> bool {
    let mut keep = true;
    let results: Vec<(Snowflake, String, String)> = {
        let s = app.client.store();
        let q = query.trim().trim_start_matches(['#', '@']).to_lowercase();
        let mut v: Vec<(i64, Snowflake, String, String)> = vec![];
        for c in s.channels.values() {
            if matches!(c.kind, ChannelType::GuildCategory) || c.kind.is_voice() {
                continue;
            }
            let (label, sub) = if c.kind.is_private() {
                let name =
                    if c.kind == ChannelType::Dm { c.recipient_ids().first().map(|u| s.display_name(None, *u)).unwrap_or_default() } else { c.display_name() };
                (format!("@{name}"), "Direct Message".to_string())
            } else {
                if !s.can_view(c.id) {
                    continue;
                }
                let g = c.guild_id.and_then(|g| s.guilds.get(&g)).map(|g| g.name().to_string()).unwrap_or_default();
                (format!("#{}", c.name.clone().unwrap_or_default()), g)
            };
            let l = label.to_lowercase();
            let score = if q.is_empty() {
                // Recent activity first.
                (c.last_message_id.map(|m| m.0 >> 22).unwrap_or(0) / 1000) as i64
            } else if l[1..].starts_with(&q) {
                1_000_000
            } else if l.contains(&q) {
                500_000
            } else {
                continue;
            };
            let unread_boost = if s.is_unread(c.id) { 10 } else { 0 };
            v.push((score + unread_boost, c.id, label, sub));
        }
        v.sort_by_key(|x| std::cmp::Reverse(x.0));
        v.into_iter().take(12).map(|(_, a, b, c)| (a, b, c)).collect()
    };
    let (up, down, enter) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        )
    });
    if !results.is_empty() {
        if up {
            *index = (*index + results.len() - 1) % results.len();
        }
        if down {
            *index = (*index + 1) % results.len();
        }
        *index = (*index).min(results.len() - 1);
    }
    let mut go = enter.then(|| results.get(*index).map(|r| r.0)).flatten();
    let r = egui::Modal::new(egui::Id::unique("quick_switcher")).frame(frame(app)).show(ctx, |ui| {
        ui.set_width(540.0);
        ui.label(RichText::new("Where would you like to go?").font(FontId::new(17.0, theme::bold())));
        ui.add_space(6.0);
        let te = ui.add(
            egui::TextEdit::singleline(query)
                .hint_text("Search channels and DMs")
                .desired_width(f32::INFINITY)
                .margin(vec2(10.0, 10.0))
                .font(FontId::proportional(16.0)),
        );
        te.request_focus();
        if te.changed() {
            *index = 0;
        }
        ui.add_space(6.0);
        for (i, (id, label, sub)) in results.iter().enumerate() {
            let (rect, resp) = widgets::list_row(ui, 34.0, i == *index, &app.pal);
            ui.painter().text(rect.left_center() + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, label, FontId::proportional(15.0), app.pal.text);
            ui.painter().text(rect.right_center() - vec2(10.0, 0.0), egui::Align2::RIGHT_CENTER, sub, FontId::proportional(12.0), app.pal.text_faint);
            if resp.clicked() {
                go = Some(*id);
            }
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Tip: Ctrl+K anywhere · ↑↓ to navigate · Enter to open").small().color(app.pal.text_faint));
    });
    if r.should_close() {
        keep = false;
    }
    if let Some(id) = go {
        app.select_channel(id);
        keep = false;
    }
    keep
}
