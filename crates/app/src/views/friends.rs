//! Friends page: online / all / pending / blocked lists and "Add Friend".

use egui::{FontId, Panel, RichText, Stroke, Ui, pos2, vec2};
use ferrocord_core::model::{RelationshipType, Snowflake, Status};

use crate::app::{App, Popup};
use crate::theme;
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum FriendsTab {
    #[default]
    Online,
    All,
    Pending,
    Blocked,
    Add,
}

pub fn page(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    Panel::top("friends_header").exact_size(48.0).show_separator_line(false).frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0))).show(
        ui,
        |ui| {
            let rect = ui.max_rect();
            ui.painter().hline(rect.x_range(), rect.max.y - 0.5, Stroke::new(1.0, pal.border));
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("👥 Friends").font(FontId::new(16.0, theme::bold())));
                ui.add_space(12.0);
                for (tab, label) in
                    [(FriendsTab::Online, "Online"), (FriendsTab::All, "All"), (FriendsTab::Pending, "Pending"), (FriendsTab::Blocked, "Blocked")]
                {
                    if ui.selectable_label(app.friends_tab == tab, label).clicked() {
                        app.friends_tab = tab;
                    }
                }
                let add = egui::Button::new(RichText::new("Add Friend").color(egui::Color32::WHITE)).fill(if app.friends_tab == FriendsTab::Add {
                    pal.success.gamma_multiply(0.6)
                } else {
                    pal.success
                });
                if ui.add(add).clicked() {
                    app.friends_tab = FriendsTab::Add;
                }
            });
        },
    );

    egui::CentralPanel::no_frame().show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 16)).show(ui, |ui| {
            if app.friends_tab == FriendsTab::Add {
                add_friend(app, ui);
                return;
            }
            let rows: Vec<(Snowflake, String, String, String, Status, RelationshipType)> = {
                let s = app.client.store();
                let mut v: Vec<_> = s
                    .relationships
                    .values()
                    .filter(|r| match app.friends_tab {
                        FriendsTab::Online => r.kind == RelationshipType::Friend && !matches!(s.status(r.id), Status::Offline | Status::Invisible),
                        FriendsTab::All => r.kind == RelationshipType::Friend,
                        FriendsTab::Pending => matches!(r.kind, RelationshipType::IncomingRequest | RelationshipType::OutgoingRequest),
                        FriendsTab::Blocked => r.kind == RelationshipType::Blocked,
                        FriendsTab::Add => false,
                    })
                    .map(|r| {
                        let u = s.user(r.id).cloned().or(r.user.clone()).unwrap_or_default();
                        let sub = match r.kind {
                            RelationshipType::IncomingRequest => "Incoming friend request".to_string(),
                            RelationshipType::OutgoingRequest => "Outgoing friend request".to_string(),
                            RelationshipType::Blocked => "Blocked".to_string(),
                            _ => s
                                .presences
                                .get(&r.id)
                                .and_then(|p| p.custom_status().and_then(|a| a.state.clone()))
                                .unwrap_or_else(|| s.status(r.id).label().to_string()),
                        };
                        (r.id, s.display_name(None, r.id), u.avatar_url(64), sub, s.status(r.id), r.kind)
                    })
                    .collect();
                v.sort_by_key(|r| r.1.to_lowercase());
                v
            };
            widgets::section_label(
                ui,
                &format!(
                    "{} — {}",
                    match app.friends_tab {
                        FriendsTab::Online => "Online",
                        FriendsTab::All => "All friends",
                        FriendsTab::Pending => "Pending",
                        _ => "Blocked",
                    },
                    rows.len()
                ),
                &pal,
            );
            ui.add_space(8.0);
            if rows.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(RichText::new("Nobody here yet.").color(pal.text_faint)));
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (id, name, avatar, sub, status, kind) in rows {
                    let (rect, resp) = widgets::list_row(ui, 56.0, false, &pal);
                    ui.painter().hline(rect.x_range(), rect.min.y, Stroke::new(1.0, pal.border));
                    let ring = if resp.hovered() { pal.bg_hover } else { pal.bg_main };
                    let av = egui::Rect::from_min_size(pos2(rect.min.x + 8.0, rect.center().y - 16.0), vec2(32.0, 32.0));
                    widgets::paint_avatar(ui, &app.images, &avatar, av, Some((status, ring)), &pal);
                    ui.painter().text(
                        pos2(av.max.x + 12.0, rect.center().y - 8.0),
                        egui::Align2::LEFT_CENTER,
                        &name,
                        FontId::new(15.0, theme::bold()),
                        pal.text,
                    );
                    ui.painter().text(
                        pos2(av.max.x + 12.0, rect.center().y + 10.0),
                        egui::Align2::LEFT_CENTER,
                        &sub,
                        FontId::proportional(12.5),
                        pal.text_faint,
                    );

                    // Action buttons on the right.
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(egui::Rect::from_min_max(pos2(rect.max.x - 200.0, rect.min.y), rect.max))
                            .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    );
                    let client = app.client.clone();
                    match kind {
                        RelationshipType::IncomingRequest => {
                            if widgets::icon_button(&mut child, "×", "Ignore", &pal, false).clicked() {
                                client.spawn_action("ignore_request", move |c| async move { c.http().remove_relationship(id).await });
                            }
                            if widgets::icon_button(&mut child, "✔", "Accept", &pal, false).clicked() {
                                client.spawn_action("accept_request", move |c| async move { c.http().add_relationship(id).await });
                            }
                        }
                        RelationshipType::OutgoingRequest | RelationshipType::Blocked => {
                            let tip = if kind == RelationshipType::Blocked { "Unblock" } else { "Cancel" };
                            if widgets::icon_button(&mut child, "×", tip, &pal, false).clicked() {
                                client.spawn_action("remove_relationship", move |c| async move { c.http().remove_relationship(id).await });
                            }
                        }
                        _ => {
                            if widgets::icon_button(&mut child, "💬", "Message", &pal, false).clicked() {
                                app.open_dm_with(id);
                            }
                        }
                    }
                    if resp.clicked() {
                        app.popup = Some(Popup::Profile { user: id, guild: None, at: rect.left_bottom() });
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Copy user ID").clicked() {
                            ui.ctx().copy_text(id.to_string());
                            ui.close();
                        }
                        if kind == RelationshipType::Friend && ui.button(RichText::new("Remove friend").color(pal.danger)).clicked() {
                            app.client.spawn_action("remove_friend", move |c| async move { c.http().remove_relationship(id).await });
                            ui.close();
                        }
                        if kind != RelationshipType::Blocked && ui.button(RichText::new("Block").color(pal.danger)).clicked() {
                            app.client.spawn_action("block", move |c| async move { c.http().block_user(id).await });
                            ui.close();
                        }
                    });
                }
            });
        });
    });
}

fn add_friend(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    ui.label(RichText::new("ADD FRIEND").font(FontId::new(15.0, theme::bold())));
    ui.label(RichText::new("You can add friends with their Discord username.").color(pal.text_muted));
    ui.add_space(10.0);
    let mut send = false;
    egui::Frame::new().fill(pal.bg_rail).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut app.add_friend)
                    .hint_text("Enter a username")
                    .frame(egui::Frame::NONE)
                    .desired_width(ui.available_width() - 180.0)
                    .margin(vec2(6.0, 6.0)),
            );
            send |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            send |= widgets::button(ui, "Send Friend Request", true, &pal).clicked();
        });
    });
    if send && !app.add_friend.trim().is_empty() {
        let name = app.add_friend.trim().to_string();
        app.add_friend.clear();
        let client = app.client.clone();
        app.toast(format!("Sending friend request to {name}…"), false);
        client.spawn_action("friend_request", move |c| async move { c.http().send_friend_request(&name).await });
    }
}
