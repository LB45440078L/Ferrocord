//! Settings modal.

use egui::{Color32, CornerRadius, FontId, RichText, Stroke, Ui, vec2};
use ferrocord_core::Theme;

use crate::app::App;
use crate::icons::Icon;
use crate::theme;
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Account,
    Appearance,
    Notifications,
    Privacy,
    Plugins,
    Api,
    Advanced,
    About,
}

const TABS: &[(Tab, &str)] = &[
    (Tab::Account, "My Account"),
    (Tab::Appearance, "Appearance"),
    (Tab::Notifications, "Notifications"),
    (Tab::Privacy, "Privacy"),
    (Tab::Plugins, "Plugins"),
    (Tab::Api, "Developer API"),
    (Tab::Advanced, "Advanced"),
    (Tab::About, "About"),
];

/// Returns false when the modal should close.
pub fn show(app: &mut App, ctx: &egui::Context) -> bool {
    let pal = app.pal;
    let mut keep = true;
    let size = ctx.content_rect().size();
    let r = egui::Modal::new(egui::Id::unique("settings"))
        .frame(egui::Frame::new().fill(pal.bg_main).corner_radius(CornerRadius::same(12)).stroke(Stroke::new(1.0, pal.border)))
        .show(ctx, |ui| {
            let w = (size.x * 0.8).clamp(640.0, 1000.0);
            let h = (size.y * 0.8).clamp(420.0, 760.0);
            ui.set_min_size(vec2(w, h));
            ui.set_max_size(vec2(w, h));
            let top_down = egui::Layout::top_down(egui::Align::Min);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(vec2(210.0, h), top_down, |ui| {
                    egui::Frame::new()
                        .fill(pal.bg_side)
                        .corner_radius(CornerRadius { nw: 12, sw: 12, ne: 0, se: 0 })
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_min_size(vec2(186.0, h - 24.0));
                            ui.set_max_width(186.0);
                            widgets::section_label(ui, "User settings", &pal);
                            ui.add_space(4.0);
                            for (tab, label) in TABS {
                                let (rect, resp) = widgets::list_row(ui, 32.0, app.settings_tab == *tab, &pal);
                                ui.painter().text(
                                    rect.left_center() + vec2(10.0, 0.0),
                                    egui::Align2::LEFT_CENTER,
                                    *label,
                                    FontId::proportional(14.5),
                                    if app.settings_tab == *tab { pal.text } else { pal.text_muted },
                                );
                                if resp.clicked() {
                                    app.settings_tab = *tab;
                                }
                            }
                            ui.add_space(8.0);
                            ui.separator();
                            let (rect, resp) = widgets::list_row(ui, 32.0, false, &pal);
                            ui.painter().text(
                                rect.left_center() + vec2(10.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                "Log Out",
                                FontId::proportional(14.5),
                                pal.danger,
                            );
                            if resp.clicked() {
                                app.client.logout();
                                keep = false;
                            }
                        });
                });
                ui.allocate_ui_with_layout(vec2(w - 210.0, h), top_down, |ui| {
                    egui::Frame::new().inner_margin(egui::Margin::same(24)).show(ui, |ui| {
                        ui.set_min_size(vec2(w - 258.0, h - 48.0));
                        ui.set_max_width(w - 258.0);
                        ui.horizontal(|ui| {
                            let title = TABS.iter().find(|t| t.0 == app.settings_tab).map(|t| t.1).unwrap_or_default();
                            ui.label(RichText::new(title).font(FontId::new(20.0, theme::bold())));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if widgets::icon_button(ui, Icon::Close, "Close (Esc)", &pal, false).clicked() {
                                    keep = false;
                                }
                            });
                        });
                        ui.add_space(12.0);
                        egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(h - 110.0).show(ui, |ui| match app.settings_tab {
                            Tab::Account => account(app, ui),
                            Tab::Appearance => appearance(app, ui),
                            Tab::Notifications => notifications(app, ui),
                            Tab::Privacy => privacy(app, ui),
                            Tab::Plugins => plugins(app, ui),
                            Tab::Api => api(app, ui),
                            Tab::Advanced => advanced(app, ui),
                            Tab::About => about(app, ui),
                        });
                    });
                });
            });
        });
    if r.should_close() {
        keep = false;
    }
    keep
}

fn card(ui: &mut Ui, pal: &theme::Palette, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(pal.bg_side).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical(add);
    });
    ui.add_space(10.0);
}

fn toggle(ui: &mut Ui, value: &mut bool, label: &str, hint: &str, muted: Color32) -> bool {
    let r = ui.checkbox(value, RichText::new(label).size(15.0));
    if !hint.is_empty() {
        ui.label(RichText::new(hint).size(12.5).color(muted));
    }
    ui.add_space(6.0);
    r.changed()
}

fn account(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let me = app.client.store().me.clone();
    let Some(me) = me else {
        ui.label("Not logged in.");
        return;
    };
    card(ui, &pal, |ui| {
        ui.horizontal(|ui| {
            widgets::avatar(ui, &app.images, &me.user.avatar_url(160), 80.0, None, &pal);
            ui.vertical(|ui| {
                ui.label(RichText::new(me.user.display_name()).font(FontId::new(20.0, theme::bold())));
                ui.label(RichText::new(me.user.tag()).color(pal.text_muted));
            });
        });
        ui.add_space(12.0);
        egui::Grid::new("acct").num_columns(2).spacing(vec2(24.0, 8.0)).show(ui, |ui| {
            ui.label(RichText::new("Email").color(pal.text_faint));
            ui.label(me.email.clone().map(|e| redact_email(&e)).unwrap_or_else(|| "—".into()));
            ui.end_row();
            ui.label(RichText::new("Phone").color(pal.text_faint));
            ui.label(me.phone.clone().map(|p| format!("•••• {}", &p[p.len().saturating_sub(4)..])).unwrap_or_else(|| "—".into()));
            ui.end_row();
            ui.label(RichText::new("Two-factor auth").color(pal.text_faint));
            ui.label(if me.mfa_enabled { RichText::new("Enabled").color(pal.success) } else { RichText::new("Disabled").color(pal.warning) });
            ui.end_row();
            ui.label(RichText::new("User ID").color(pal.text_faint));
            if ui.link(me.user.id.to_string()).on_hover_text("Click to copy").clicked() {
                ui.ctx().copy_text(me.user.id.to_string());
            }
            ui.end_row();
        });
    });
    ui.label(RichText::new("Profile, password and 2FA changes are made in the official app or website for now.").size(12.5).color(pal.text_faint));
}

fn redact_email(e: &str) -> String {
    match e.split_once('@') {
        Some((u, d)) => format!("{}{}@{d}", &u[..u.len().min(2)], "•".repeat(u.len().saturating_sub(2).min(8))),
        None => e.to_string(),
    }
}

fn appearance(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let mut cfg = app.client.config().appearance;
    let mut changed = false;
    card(ui, &pal, |ui| {
        widgets::section_label(ui, "Theme", &pal);
        ui.horizontal(|ui| {
            for (t, label) in [(Theme::Dark, "Dark"), (Theme::Midnight, "Midnight"), (Theme::Light, "Light")] {
                if ui.selectable_label(cfg.theme == t, label).clicked() {
                    cfg.theme = t;
                    changed = true;
                }
            }
        });
        ui.add_space(8.0);
        widgets::section_label(ui, "Accent color", &pal);
        ui.horizontal(|ui| {
            for hex in ["#5865f2", "#3ba55d", "#eb459e", "#f0b232", "#ed4245", "#00a8fc", "#9b84ec", "#e67e22"] {
                let c = theme::parse_hex(hex).unwrap_or(Color32::GRAY);
                let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::click());
                ui.painter().circle_filled(r.center(), 12.0, c);
                if cfg.accent.eq_ignore_ascii_case(hex) {
                    ui.painter().circle_stroke(r.center(), 13.5, Stroke::new(2.0, pal.text));
                }
                if resp.clicked() {
                    cfg.accent = hex.into();
                    changed = true;
                }
            }
        });
        ui.add_space(8.0);
        widgets::section_label(ui, "Zoom", &pal);
        changed |= ui.add(egui::Slider::new(&mut cfg.zoom, 0.7..=1.6).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).drag_stopped();
    });
    card(ui, &pal, |ui| {
        widgets::section_label(ui, "Messages", &pal);
        changed |= toggle(ui, &mut cfg.compact_messages, "Compact mode", "Fit more messages on screen.", pal.text_faint);
        changed |= toggle(ui, &mut cfg.show_avatars, "Show avatars", "", pal.text_faint);
        changed |= toggle(ui, &mut cfg.inline_media, "Show images and embeds inline", "", pal.text_faint);
        changed |= toggle(ui, &mut cfg.animate_emoji, "Animate emoji and GIFs", "", pal.text_faint);
        changed |= toggle(ui, &mut cfg.use_24h_time, "24-hour clock", "", pal.text_faint);
        changed |= toggle(ui, &mut cfg.show_member_list, "Show member list", "", pal.text_faint);
    });
    if changed {
        app.client.update_config(|c| c.appearance = cfg);
    }
}

fn notifications(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let mut cfg = app.client.config().notifications;
    let mut changed = false;
    card(ui, &pal, |ui| {
        changed |= toggle(
            ui,
            &mut cfg.desktop,
            "Desktop notifications",
            "Show a system notification for mentions and DMs when the window isn't focused.",
            pal.text_faint,
        );
        changed |= toggle(ui, &mut cfg.show_content, "Show message content", "Include message text in notifications.", pal.text_faint);
        changed |= toggle(ui, &mut cfg.sound, "Notification sound", "", pal.text_faint);
        changed |= toggle(ui, &mut cfg.taskbar_badge, "Unread count in window title", "", pal.text_faint);
    });
    ui.label(
        RichText::new("Per-server and per-channel notification levels follow your Discord settings (right-click a server or channel to mute).")
            .size(12.5)
            .color(pal.text_faint),
    );
    if changed {
        app.client.update_config(|c| c.notifications = cfg);
    }
}

fn privacy(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let mut cfg = app.client.config().privacy;
    let mut changed = false;
    card(ui, &pal, |ui| {
        changed |= toggle(ui, &mut cfg.send_typing, "Send typing indicators", "Let others see when you're typing.", pal.text_faint);
        changed |= toggle(ui, &mut cfg.auto_ack, "Mark channels as read automatically", "When off, use Esc or right-click → Mark as read.", pal.text_faint);
    });
    card(ui, &pal, |ui| {
        widgets::section_label(ui, "Your data", &pal);
        ui.label(RichText::new("Your token is stored in the operating system's credential vault. Media is cached locally and can be cleared under Advanced. Ferrocord sends no telemetry of its own.").size(13.0).color(pal.text_muted));
    });
    if changed {
        app.client.update_config(|c| c.privacy = cfg);
    }
}

fn plugins(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    ui.label(RichText::new("Plugins are sandboxed Luau scripts. They only get the permissions you grant them.").color(pal.text_muted));
    ui.horizontal(|ui| {
        if ui.button("Open plugins folder").clicked() {
            let dir = app.ext.plugins_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = open::that_detached(dir);
        }
        if ui.button("Rescan").clicked() {
            app.ext.rescan();
        }
    });
    ui.add_space(10.0);
    let list = app.ext.plugin_list();
    if list.is_empty() {
        card(ui, &pal, |ui| {
            ui.label(
                RichText::new("No plugins installed. Copy a plugin folder (with plugin.toml) into the plugins folder, then press Rescan.")
                    .color(pal.text_faint),
            );
        });
    }
    for p in list {
        card(ui, &pal, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(format!("{}  v{}", p.name, p.version)).font(FontId::new(15.5, theme::bold())));
                    ui.label(RichText::new(format!("by {} · {}", p.author, p.id)).size(12.0).color(pal.text_faint));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut on = p.enabled;
                    if ui.checkbox(&mut on, "Enabled").changed() {
                        app.ext.set_plugin_enabled(&p.id, on);
                    }
                    if p.enabled && ui.small_button("Reload").clicked() {
                        app.ext.reload_plugin(&p.id);
                    }
                });
            });
            if !p.description.is_empty() {
                ui.label(RichText::new(&p.description).size(13.5));
            }
            if let Some(e) = &p.error {
                ui.label(RichText::new(format!("Error: {e}")).size(12.5).color(pal.danger));
            }
            if !p.declared.is_empty() {
                ui.add_space(4.0);
                widgets::section_label(ui, "Permissions", &pal);
                ui.horizontal_wrapped(|ui| {
                    for perm in &p.declared {
                        let mut granted = p.granted.contains(perm);
                        if ui.checkbox(&mut granted, RichText::new(perm).monospace().size(12.5)).changed() {
                            app.ext.grant(&p.id, perm, granted);
                        }
                    }
                });
            }
        });
    }
}

fn api(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let mut cfg = app.client.config().api;
    let mut changed = false;
    ui.label(RichText::new("A local HTTP + WebSocket API lets your own scripts and tools read state, receive live events and (optionally) act on your behalf. It only listens on 127.0.0.1 and requires a secret token.").color(pal.text_muted));
    ui.add_space(8.0);
    card(ui, &pal, |ui| {
        changed |= toggle(ui, &mut cfg.enabled, "Enable local API", "", pal.text_faint);
        changed |= toggle(
            ui,
            &mut cfg.allow_actions,
            "Allow actions",
            "Let API clients send messages, react, change presence and mark channels read.",
            pal.text_faint,
        );
        ui.horizontal(|ui| {
            ui.label("Port");
            let mut port = cfg.port as u32;
            if ui.add(egui::DragValue::new(&mut port).range(1024..=65535)).changed() {
                cfg.port = port as u16;
                changed = true;
            }
        });
    });
    if changed {
        app.client.update_config(|c| c.api = cfg.clone());
        app.ext.apply_api_config();
    }
    card(ui, &pal, |ui| match app.ext.api_status() {
        Some((addr, token)) => {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, pal.success);
                ui.label(RichText::new(format!("Listening on http://{addr}")).color(pal.success));
            });
            ui.horizontal(|ui| {
                ui.label("Token:");
                ui.label(RichText::new(format!("{}…", &token[..token.len().min(8)])).monospace());
                if ui.button("Copy token").clicked() {
                    ui.ctx().copy_text(token.clone());
                }
                if ui.button("Regenerate").clicked() {
                    app.ext.regenerate_api_token();
                }
            });
            ui.label(RichText::new("See docs/API.md for endpoints and the WebSocket protocol.").size(12.5).color(pal.text_faint));
        }
        None => {
            ui.label(
                RichText::new(if cfg.enabled { app.ext.api_error().unwrap_or_else(|| "Starting…".into()) } else { "Disabled".into() }).color(pal.text_faint),
            );
        }
    });
}

fn advanced(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    let profile = app.client.http().profile();
    card(ui, &pal, |ui| {
        egui::Grid::new("adv").num_columns(2).spacing(vec2(24.0, 8.0)).show(ui, |ui| {
            ui.label(RichText::new("Client build").color(pal.text_faint));
            ui.label(profile.client_build_number.to_string());
            ui.end_row();
            ui.label(RichText::new("Gateway").color(pal.text_faint));
            ui.label(format!("{:?}", app.client.connection_state()));
            ui.end_row();
            ui.label(RichText::new("Latency").color(pal.text_faint));
            ui.label(app.client.latency().map(|l| format!("{} ms", l.as_millis())).unwrap_or_else(|| "—".into()));
            ui.end_row();
            ui.label(RichText::new("Config folder").color(pal.text_faint));
            if ui.link(app.client.paths().config_dir.display().to_string()).clicked() {
                let _ = open::that_detached(&app.client.paths().config_dir);
            }
            ui.end_row();
        });
    });
    card(ui, &pal, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Clear media cache").clicked() {
                let dir = app.client.paths().media_cache();
                let _ = std::fs::remove_dir_all(&dir);
                let _ = std::fs::create_dir_all(&dir);
                app.toast("Media cache cleared", false);
            }
            if ui.button("Reconnect gateway").clicked()
                && let Some(g) = app.client.gateway()
            {
                g.reconnect();
            }
        });
    });
}

fn about(app: &mut App, ui: &mut Ui) {
    let pal = app.pal;
    card(ui, &pal, |ui| {
        ui.label(RichText::new(format!("Ferrocord {}", env!("CARGO_PKG_VERSION"))).font(FontId::new(18.0, theme::bold())));
        ui.label(RichText::new("A fast, open-source Discord client written in Rust.").color(pal.text_muted));
        ui.add_space(6.0);
        ui.label("Licensed under the BSD 3-Clause License.");
        ui.label(
            RichText::new("Fonts: Inter and JetBrains Mono (SIL Open Font License). Emoji graphics: Twemoji (CC-BY 4.0).").size(12.5).color(pal.text_faint),
        );
        ui.add_space(6.0);
        ui.label(RichText::new("Ferrocord is not affiliated with, endorsed by or sponsored by Discord Inc. Using third-party clients may violate Discord's Terms of Service; use at your own risk.").size(12.5).color(pal.warning));
    });
}
