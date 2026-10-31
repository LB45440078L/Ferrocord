//! Forum / media channels: a list of posts (threads); opening one shows its chat.

use egui::{FontId, Panel, RichText, Stroke, Ui, pos2};
use ferrocord_core::model::{Channel, Snowflake};

use crate::app::App;
use crate::theme;
use crate::widgets;

pub fn forum(app: &mut App, ui: &mut Ui, channel: Snowflake) {
    let pal = app.pal;
    let (name, topic, posts) = {
        let s = app.client.store();
        let c = s.channels.get(&channel);
        let posts: Vec<Channel> = s.threads_of(channel).into_iter().cloned().collect();
        (c.and_then(|c| c.name.clone()).unwrap_or_default(), c.and_then(|c| c.topic.clone()).unwrap_or_default(), posts)
    };
    Panel::top("forum_header").exact_size(48.0).show_separator_line(false).frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0))).show(
        ui,
        |ui| {
            let rect = ui.max_rect();
            ui.painter().hline(rect.x_range(), rect.max.y - 0.5, Stroke::new(1.0, pal.border));
            ui.horizontal_centered(|ui| {
                crate::icons::icon(ui, crate::icons::Icon::Forum, 22.0, pal.text_faint);
                ui.label(RichText::new(&name).font(FontId::new(16.0, theme::bold())));
            });
        },
    );
    egui::CentralPanel::no_frame().show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 16)).show(ui, |ui| {
                if !topic.is_empty() {
                    ui.label(RichText::new(&topic).color(pal.text_muted));
                    ui.add_space(10.0);
                }
                widgets::section_label(ui, &format!("Active posts — {}", posts.len()), &pal);
                ui.add_space(6.0);
                if posts.is_empty() {
                    ui.label(RichText::new("No active posts loaded.").color(pal.text_faint));
                }
                for p in posts {
                    let (rect, resp) = widgets::list_row(ui, 64.0, false, &pal);
                    ui.painter().rect_stroke(rect, egui::CornerRadius::same(8), Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
                    let title = p.name.clone().unwrap_or_default();
                    ui.painter().text(pos2(rect.min.x + 14.0, rect.min.y + 20.0), egui::Align2::LEFT_CENTER, title, FontId::new(15.5, theme::bold()), pal.text);
                    let owner = p.owner_id.map(|o| app.client.store().display_name(p.guild_id, o)).unwrap_or_default();
                    let sub = format!("{owner} · {} messages", p.message_count.unwrap_or(0));
                    ui.painter().text(pos2(rect.min.x + 14.0, rect.min.y + 44.0), egui::Align2::LEFT_CENTER, sub, FontId::proportional(12.5), pal.text_faint);
                    if app.client.store().is_unread(p.id) {
                        ui.painter().circle_filled(pos2(rect.max.x - 16.0, rect.center().y), 4.0, pal.text);
                    }
                    if resp.clicked() {
                        app.select_channel(p.id);
                    }
                    ui.add_space(6.0);
                }
            });
        });
    });
}
