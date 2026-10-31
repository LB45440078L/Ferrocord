//! Floating popups: user profile card and emoji picker.

use egui::{Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use ferrocord_core::model::{PartialEmoji, Snowflake};

use crate::app::{App, EmojiTarget, Popup};
use crate::theme;
use crate::widgets;

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(popup) = app.popup.take() else { return };
    let keep = match &popup {
        Popup::Profile { user, guild, at } => profile(app, ctx, *user, *guild, *at),
        Popup::EmojiPicker { target, at } => emoji_picker(app, ctx, *target, *at),
    };
    if keep && app.popup.is_none() {
        app.popup = Some(popup);
    }
}

/// Positions a popup of `size` near `at`, kept inside the screen.
fn place(ctx: &egui::Context, at: egui::Pos2, size: Vec2) -> egui::Pos2 {
    let screen = ctx.content_rect();
    let x = at.x.clamp(screen.min.x + 8.0, screen.max.x - size.x - 8.0);
    let y = at.y.clamp(screen.min.y + 8.0, screen.max.y - size.y - 8.0);
    pos2(x, y)
}

fn profile(app: &mut App, ctx: &egui::Context, user: Snowflake, guild: Option<Snowflake>, at: egui::Pos2) -> bool {
    let pal = app.pal;
    let (u, name, roles, status, custom, note, member_since) = {
        let s = app.client.store();
        let u = s.user(user).cloned().unwrap_or_default();
        let roles: Vec<(String, Color32)> = guild
            .and_then(|g| {
                let gg = s.guilds.get(&g)?;
                let m = s.member(g, user)?;
                let mut r: Vec<_> = gg.roles.iter().filter(|r| m.roles.contains(&r.id)).collect();
                r.sort_by_key(|r| std::cmp::Reverse(r.position));
                Some(r.into_iter().map(|r| (r.name.clone(), theme::role_color(r.color).unwrap_or(pal.text_faint))).collect())
            })
            .unwrap_or_default();
        let custom = s.presences.get(&user).and_then(|p| p.custom_status().and_then(|a| a.state.clone()));
        let joined = guild
            .and_then(|g| s.member(g, user))
            .and_then(|m| m.joined_at.clone())
            .and_then(|j| chrono::DateTime::parse_from_rfc3339(&j).ok())
            .map(|d| d.format("%b %-d, %Y").to_string());
        (u, s.display_name(guild, user), roles, s.status(user), custom, s.notes.get(&user).cloned(), joined)
    };
    let size = vec2(300.0, 360.0);
    let pos = place(ctx, at, size);
    let mut keep = true;
    let area = egui::Area::new(egui::Id::unique("profile_popup")).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::new().fill(pal.bg_side).stroke(Stroke::new(1.0, pal.border)).corner_radius(CornerRadius::same(10)).shadow(ui.visuals().popup_shadow).show(
            ui,
            |ui| {
                ui.set_width(size.x);
                let banner_color = u.accent_color.and_then(theme::role_color).unwrap_or(pal.accent);
                let (banner, _) = ui.allocate_exact_size(vec2(size.x, 70.0), Sense::hover());
                ui.painter().rect_filled(banner, CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, banner_color);
                let av = Rect::from_min_size(pos2(banner.min.x + 16.0, banner.max.y - 36.0), Vec2::splat(72.0));
                ui.painter().circle_filled(av.center(), 41.0, pal.bg_side);
                widgets::paint_avatar(ui, &app.images, &u.avatar_url(160), av, Some((status, pal.bg_side)), &pal);
                ui.add_space(40.0);
                egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 14 }).show(ui, |ui| {
                    ui.label(RichText::new(&name).font(FontId::new(19.0, theme::bold())));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(u.tag()).color(pal.text_muted));
                        if u.bot {
                            crate::markdown::render::badge(ui, "APP", pal.accent, Color32::WHITE);
                        }
                    });
                    if let Some(c) = &custom {
                        ui.label(RichText::new(c).size(13.0));
                    }
                    ui.add_space(6.0);
                    ui.separator();
                    let full = app.ext.profile(user, guild);
                    let bio = full.as_ref().and_then(|p| p.user_profile.as_ref().and_then(|m| m.bio.clone())).filter(|b| !b.is_empty());
                    if let Some(pr) = full.as_ref().and_then(|p| p.user_profile.as_ref().and_then(|m| m.pronouns.clone())).filter(|p| !p.is_empty()) {
                        ui.label(RichText::new(pr).size(12.5).color(pal.text_muted));
                    }
                    if let Some(bio) = bio {
                        widgets::section_label(ui, "About me", &pal);
                        ui.label(RichText::new(bio.chars().take(400).collect::<String>()).size(13.0));
                    }
                    if let Some(d) = member_since {
                        widgets::section_label(ui, "Member since", &pal);
                        ui.label(RichText::new(d).size(13.0).color(pal.text_muted));
                    }
                    if !roles.is_empty() {
                        widgets::section_label(ui, "Roles", &pal);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                            for (r, c) in roles.iter().take(20) {
                                egui::Frame::new().fill(pal.bg_elevated).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::symmetric(6, 2)).show(
                                    ui,
                                    |ui| {
                                        ui.horizontal(|ui| {
                                            let (d, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                            ui.painter().circle_filled(d.center(), 5.0, *c);
                                            ui.label(RichText::new(r).size(12.0));
                                        });
                                    },
                                );
                            }
                        });
                    }
                    if let Some(n) = note.filter(|n| !n.is_empty()) {
                        widgets::section_label(ui, "Note", &pal);
                        ui.label(RichText::new(n).size(13.0).color(pal.text_muted));
                    }
                    ui.add_space(8.0);
                    let me = app.client.store().my_id();
                    ui.horizontal(|ui| {
                        if user != me && widgets::button(ui, "Message", true, &pal).clicked() {
                            app.open_dm_with(user);
                            keep = false;
                        }
                        if ui.button("Copy ID").clicked() {
                            ui.ctx().copy_text(user.to_string());
                        }
                    });
                });
            },
        );
    });
    // Close on click outside.
    if ctx.input(|i| i.pointer.any_pressed())
        && !area.response.contains_pointer()
        && area.response.ctx.pointer_interact_pos().is_some_and(|p| !area.response.rect.contains(p))
    {
        keep = false;
    }
    keep
}

const CATEGORIES: &[(emojis::Group, &str, &str)] = &[
    (emojis::Group::SmileysAndEmotion, "😀", "Smileys & Emotion"),
    (emojis::Group::PeopleAndBody, "👋", "People & Body"),
    (emojis::Group::AnimalsAndNature, "🐱", "Animals & Nature"),
    (emojis::Group::FoodAndDrink, "🍔", "Food & Drink"),
    (emojis::Group::TravelAndPlaces, "✈", "Travel & Places"),
    (emojis::Group::Activities, "⚽", "Activities"),
    (emojis::Group::Objects, "💡", "Objects"),
    (emojis::Group::Symbols, "❤", "Symbols"),
    (emojis::Group::Flags, "🏁", "Flags"),
];

fn emoji_picker(app: &mut App, ctx: &egui::Context, target: EmojiTarget, at: egui::Pos2) -> bool {
    let pal = app.pal;
    let size = vec2(380.0, 420.0);
    let pos = place(ctx, at - vec2(size.x, size.y), size);
    let mut keep = true;
    let mut picked: Option<(PartialEmoji, String)> = None;
    let guild = app.nav.guild;
    let area = egui::Area::new(egui::Id::unique("emoji_picker")).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::new()
            .fill(pal.bg_side)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(10))
            .shadow(ui.visuals().popup_shadow)
            .show(ui, |ui| {
                ui.set_width(size.x - 20.0);
                ui.set_height(size.y - 20.0);
                let r = ui.add(egui::TextEdit::singleline(&mut app.emoji_query).hint_text("Find the perfect emoji").desired_width(f32::INFINITY));
                r.request_focus();
                let q = app.emoji_query.trim().to_lowercase();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let cell = 34.0;
                    let per_row = ((ui.available_width()) / cell).floor().max(1.0) as usize;
                    let mut grid = |ui: &mut egui::Ui, items: Vec<(String, PartialEmoji, String)>| {
                        for chunk in items.chunks(per_row) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (url, e, insert) in chunk {
                                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                                    if resp.hovered() {
                                        ui.painter().rect_filled(rect, CornerRadius::same(6), pal.bg_hover);
                                    }
                                    if ui.is_rect_visible(rect) {
                                        match app.images.get(url) {
                                            Some(img) => {
                                                ui.painter().image(
                                                    img.texture,
                                                    rect.shrink(5.0),
                                                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                                    Color32::WHITE,
                                                );
                                            }
                                            None => {
                                                ui.painter().text(
                                                    rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    e.name.clone().unwrap_or_default(),
                                                    FontId::proportional(18.0),
                                                    pal.text,
                                                );
                                            }
                                        }
                                    }
                                    if resp.on_hover_text(format!(":{}:", insert.trim_matches(':'))).clicked() {
                                        picked = Some((e.clone(), insert.clone()));
                                    }
                                }
                            });
                        }
                    };

                    // Server emoji first.
                    type Cell = (String, PartialEmoji, String);
                    let customs: Vec<(String, Vec<Cell>)> = {
                        let s = app.client.store();
                        let mut gids: Vec<Snowflake> = guild.into_iter().collect();
                        gids.extend(s.guild_order.iter().copied().filter(|g| Some(*g) != guild));
                        gids.into_iter()
                            .filter_map(|g| s.guilds.get(&g))
                            .map(|g| {
                                let items = g
                                    .emojis
                                    .iter()
                                    .filter(|e| e.available != Some(false))
                                    .filter(|e| q.is_empty() || e.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&q)))
                                    .filter_map(|e| {
                                        let name = e.name.clone()?;
                                        let id = e.id?;
                                        let raw = format!("<{}:{name}:{id}>", if e.animated { "a" } else { "" });
                                        Some((e.url(64)?, PartialEmoji { id: Some(id), name: Some(name), animated: e.animated }, raw))
                                    })
                                    .collect::<Vec<_>>();
                                (g.name().to_string(), items)
                            })
                            .filter(|(_, items)| !items.is_empty())
                            .take(if q.is_empty() { 6 } else { 50 })
                            .collect()
                    };
                    for (name, items) in customs {
                        widgets::section_label(ui, &name, &pal);
                        grid(ui, items);
                    }
                    for (group, icon, title) in CATEGORIES {
                        let items: Vec<(String, PartialEmoji, String)> = group
                            .emojis()
                            .filter(|e| q.is_empty() || e.name().contains(q.as_str()) || e.shortcodes().any(|s| s.contains(q.as_str())))
                            .filter_map(|e| Some((crate::markdown::twemoji_url(e.as_str())?, PartialEmoji::unicode(e.as_str()), e.as_str().to_string())))
                            .collect();
                        if items.is_empty() {
                            continue;
                        }
                        widgets::section_label(ui, &format!("{icon}  {title}"), &pal);
                        grid(ui, items);
                    }
                });
            });
    });
    if let Some((emoji, insert)) = picked {
        match target {
            EmojiTarget::Reaction(ch, msg) => {
                if !app.demo {
                    app.client.toggle_reaction(ch, msg, emoji);
                }
            }
            EmojiTarget::Composer(ch) => {
                let c = app.composers.entry(ch).or_default();
                if emoji.id.is_some() {
                    let display = format!(":{}:", emoji.name.clone().unwrap_or_default());
                    c.replacements.push((display.clone(), insert));
                    c.text.push_str(&display);
                } else {
                    c.text.push_str(&insert);
                }
            }
        }
        app.emoji_query.clear();
        keep = false;
    }
    if ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_interact_pos().is_some_and(|p| !area.response.rect.contains(p)) {
        keep = false;
        app.emoji_query.clear();
    }
    keep
}
