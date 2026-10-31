//! Member sidebar backed by the lazy `GUILD_MEMBER_LIST_UPDATE` list.

use egui::{FontId, RichText, Ui, pos2, vec2};
use ferrocord_core::model::Snowflake;
use ferrocord_core::model::gateway::MemberListItem;

use crate::app::{App, Popup};
use crate::theme;
use crate::widgets;

const ROW: f32 = 44.0;

enum Row {
    Header(String),
    Member { id: Snowflake, name: String, color: Option<egui::Color32>, avatar: String, status: ferrocord_core::model::Status, sub: String, bot: bool },
    Placeholder,
}

pub fn panel(app: &mut App, ui: &mut Ui) {
    let Some(guild) = app.nav.guild else { return };
    let Some(channel) = app.nav.channel else { return };
    let pal = app.pal;

    let rows: Vec<Row> = {
        let s = app.client.store();
        let g = s.guilds.get(&guild);
        let role_name = |id: &str| -> String {
            match id {
                "online" => "Online".into(),
                "offline" => "Offline".into(),
                _ => g
                    .and_then(|g| id.parse::<u64>().ok().and_then(|rid| g.roles.iter().find(|r| r.id.0 == rid)))
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| id.to_string()),
            }
        };
        match s.member_list(guild) {
            Some(list) => list
                .rows
                .iter()
                .map(|r| match r {
                    Some(MemberListItem::Group(gr)) => Row::Header(format!("{} — {}", role_name(&gr.id), gr.count)),
                    Some(MemberListItem::Member(m)) => {
                        let id = m.member.user_id().unwrap_or_default();
                        let user = m.member.user.clone().filter(|u| !u.username.is_empty()).or_else(|| s.user(id).cloned()).unwrap_or_default();
                        let status = m.presence.as_ref().map(|p| p.status).unwrap_or_else(|| s.status(id));
                        let sub = m
                            .presence
                            .as_ref()
                            .and_then(|p| {
                                p.custom_status().and_then(|a| a.state.clone()).or_else(|| {
                                    p.activities.iter().find(|a| a.kind != 4).map(|a| match a.kind {
                                        0 => format!("Playing {}", a.name),
                                        1 => format!("Streaming {}", a.details.clone().unwrap_or(a.name.clone())),
                                        2 => format!("Listening to {}", a.name),
                                        3 => format!("Watching {}", a.name),
                                        5 => format!("Competing in {}", a.name),
                                        _ => a.name.clone(),
                                    })
                                })
                            })
                            .unwrap_or_default();
                        let avatar = m.member.avatar_url(guild, 64).unwrap_or_else(|| user.avatar_url(64));
                        Row::Member {
                            id,
                            name: m.member.nick.clone().unwrap_or_else(|| user.display_name().to_string()),
                            color: s.member_color(guild, id).and_then(theme::role_color),
                            avatar,
                            status,
                            sub,
                            bot: user.bot,
                        }
                    }
                    None => Row::Placeholder,
                })
                .collect(),
            None => vec![],
        }
    };

    if rows.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.spinner();
            ui.label(RichText::new("Loading members…").color(pal.text_faint));
        });
        return;
    }

    let total = rows.len();
    egui::ScrollArea::vertical().id_salt(("members", guild)).auto_shrink([false, false]).show_rows(ui, ROW, total, |ui, range| {
        if !app.demo {
            app.client.subscribe_member_list(guild, channel, range.end as u32);
        }
        for i in range {
            match &rows[i] {
                Row::Header(text) => {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), egui::Sense::hover());
                    ui.painter().text(
                        pos2(rect.min.x + 16.0, rect.max.y - 10.0),
                        egui::Align2::LEFT_BOTTOM,
                        text.to_uppercase(),
                        FontId::new(11.5, theme::bold()),
                        pal.text_faint,
                    );
                }
                Row::Placeholder => {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), egui::Sense::hover());
                    ui.painter().circle_filled(pos2(rect.min.x + 30.0, rect.center().y), 16.0, pal.bg_hover);
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(pos2(rect.min.x + 56.0, rect.center().y - 5.0), vec2(100.0, 10.0)),
                        egui::CornerRadius::same(5),
                        pal.bg_hover,
                    );
                }
                Row::Member { id, name, color, avatar, status, sub, bot } => {
                    let offline = matches!(status, ferrocord_core::model::Status::Offline | ferrocord_core::model::Status::Invisible);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let (rect, resp) = widgets::list_row(ui, ROW - 2.0, false, &pal);
                        let ring = if resp.hovered() { pal.bg_hover } else { pal.bg_side };
                        let av = egui::Rect::from_min_size(pos2(rect.min.x + 8.0, rect.center().y - 16.0), vec2(32.0, 32.0));
                        widgets::paint_avatar(ui, &app.images, avatar, av, Some((*status, ring)), &pal);
                        let c = color.unwrap_or(pal.text_muted);
                        let c = if offline { c.gamma_multiply(0.5) } else { c };
                        let x = av.max.x + 10.0;
                        let max_w = rect.max.x - x - 8.0;
                        let name_g = ui.ctx().fonts_mut(|f| {
                            let mut job = egui::text::LayoutJob::simple_singleline(name.clone(), FontId::new(14.5, theme::bold()), c);
                            job.wrap = egui::text::TextWrapping::truncate_at_width(max_w - if *bot { 36.0 } else { 0.0 });
                            f.layout_job(job)
                        });
                        let name_w = name_g.size().x;
                        let y = if sub.is_empty() { rect.center().y - name_g.size().y / 2.0 } else { rect.center().y - name_g.size().y + 1.0 };
                        ui.painter().galley(pos2(x, y), name_g, c);
                        if *bot {
                            let br = egui::Rect::from_min_size(pos2(x + name_w + 4.0, y + 3.0), vec2(28.0, 14.0));
                            ui.painter().rect_filled(br, egui::CornerRadius::same(3), pal.accent);
                            ui.painter().text(br.center(), egui::Align2::CENTER_CENTER, "APP", FontId::new(9.5, theme::bold()), egui::Color32::WHITE);
                        }
                        if !sub.is_empty() {
                            let g = ui.ctx().fonts_mut(|f| {
                                let mut job = egui::text::LayoutJob::simple_singleline(sub.clone(), FontId::proportional(12.0), pal.text_faint);
                                job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
                                f.layout_job(job)
                            });
                            ui.painter().galley(pos2(x, rect.center().y + 1.0), g, pal.text_faint);
                        }
                        if resp.clicked() {
                            app.popup = Some(Popup::Profile { user: *id, guild: Some(guild), at: rect.left_top() - vec2(310.0, 0.0) });
                        }
                        if !app.demo {
                            resp.context_menu(|ui| crate::views::safety::user_menu(ui, &app.client, *id, &pal));
                        }
                    });
                }
            }
        }
    });
}
