//! Chat view: header, virtualized message list and composer.

use std::collections::HashSet;

use egui::{Color32, CornerRadius, FontId, Panel, Rect, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};
use ferrocord_core::model::{Attachment, ChannelType, Embed, Message, PartialEmoji, Permissions, Snowflake, message_type};
use ferrocord_core::store::PendingState;

use crate::app::{App, EmojiTarget, Modal, Popup};
use crate::icons::{self, Icon};
use crate::markdown::{self, MdAction, MdCtx};
use crate::theme;
use crate::widgets;

const GUTTER: f32 = 72.0;
const GROUP_GAP_MS: u64 = 7 * 60 * 1000;

/// Deferred UI actions collected while the store is borrowed.
enum Act {
    Reply(Snowflake),
    Edit(Snowflake, String),
    Delete(Snowflake),
    React(Snowflake, PartialEmoji),
    Picker(Snowflake, egui::Pos2),
    Profile(Snowflake, egui::Pos2),
    Url(String),
    Channel(Snowflake),
    Jump(Snowflake),
    Image(String, String),
    Pin(Snowflake, bool),
    Spoiler(Snowflake, usize),
    Retry(String),
    Discard(String),
    LoadOlder,
}

pub fn chat(app: &mut App, ui: &mut Ui, channel: Snowflake) {
    Panel::top("chat_header").exact_size(48.0).show_separator_line(false).frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0))).show(
        ui,
        |ui| {
            header(app, ui, channel);
        },
    );
    let private = app.client.store().channels.get(&channel).is_some_and(|c| matches!(c.kind, ChannelType::Dm | ChannelType::GroupDm));
    if private && (app.client.voice_channel() == Some(channel) || app.client.call_active(channel)) {
        Panel::top("dm_call").exact_size(300.0).show_separator_line(false).frame(egui::Frame::new()).show(ui, |ui| {
            crate::views::voice::call_view(app, ui, channel);
        });
    }
    Panel::bottom("composer")
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 4 }))
        .show(ui, |ui| {
            composer(app, ui, channel);
        });
    egui::CentralPanel::no_frame().show(ui, |ui| {
        messages(app, ui, channel);
    });
}

fn header(app: &mut App, ui: &mut Ui, channel: Snowflake) {
    let pal = app.pal;
    let rect = ui.max_rect();
    ui.painter().hline(rect.x_range(), rect.max.y - 0.5, Stroke::new(1.0, pal.border));
    let (name, topic, kind, dm_user) = {
        let s = app.client.store();
        let c = s.channels.get(&channel);
        let kind = c.map(|c| c.kind).unwrap_or_default();
        let dm_user = c.filter(|c| c.kind == ChannelType::Dm).and_then(|c| c.recipient_ids().first().copied());
        let name = match (c, dm_user) {
            (_, Some(u)) => s.display_name(None, u),
            (Some(c), None) if c.kind == ChannelType::GroupDm => c.display_name(),
            (Some(c), None) => c.name.clone().unwrap_or_default(),
            _ => String::new(),
        };
        (name, c.and_then(|c| c.topic.clone()).unwrap_or_default(), kind, dm_user)
    };
    ui.horizontal_centered(|ui| {
        let nsfw = app.client.store().channels.get(&channel).is_some_and(|c| c.nsfw);
        icons::icon(ui, crate::views::channels::channel_icon(kind, nsfw), 22.0, pal.text_faint);
        let r = ui.add(egui::Label::new(RichText::new(&name).font(FontId::new(16.0, theme::bold()))).sense(Sense::click()));
        if let Some(u) = dm_user {
            let st = app.client.store().status(u);
            let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            widgets::paint_status(ui, dot.center(), 5.0, st, &pal, pal.bg_main);
            if r.clicked() {
                app.popup = Some(Popup::Profile { user: u, guild: None, at: r.rect.left_bottom() });
            }
        }
        if !topic.is_empty() {
            ui.add_space(6.0);
            let (sep, _) = ui.allocate_exact_size(vec2(1.0, 22.0), Sense::hover());
            ui.painter().rect_filled(sep, CornerRadius::ZERO, pal.border);
            ui.add_space(6.0);
            let first_line: String = topic.lines().next().unwrap_or_default().chars().filter(|c| !matches!(c, '*' | '_' | '~' | '|' | '`')).collect();
            let max = (ui.available_width() - 120.0).max(40.0);
            let g = ui.ctx().fonts_mut(|f| {
                let mut job = egui::text::LayoutJob::simple_singleline(first_line, FontId::proportional(13.5), pal.text_muted);
                job.wrap = egui::text::TextWrapping::truncate_at_width(max);
                f.layout_job(job)
            });
            ui.add(egui::Label::new(g)).on_hover_text(topic.chars().take(1000).collect::<String>());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if app.nav.guild.is_some() {
                let show = app.client.config().appearance.show_member_list;
                if widgets::icon_button(ui, Icon::Users, "Member list", &pal, show).clicked() {
                    app.client.update_config(|c| c.appearance.show_member_list = !show);
                }
            }
            let private = matches!(kind, ChannelType::Dm | ChannelType::GroupDm);
            if private && app.client.voice_channel() != Some(channel) {
                let label = if app.client.call_active(channel) { "Join call" } else { "Start voice call" };
                if widgets::icon_button(ui, Icon::Phone, label, &pal, false).clicked() && !app.demo {
                    app.client.join_voice(channel);
                }
            }
            if widgets::icon_button(ui, Icon::Search, "Search messages", &pal, false).clicked() {
                app.modal = Some(Modal::Search { query: String::new(), results: None, searching: false });
            }
            if widgets::icon_button(ui, Icon::Pin, "Pinned messages", &pal, false).clicked() {
                app.modal = Some(Modal::Pins { channel, messages: None });
                let tx = app.ext.pins_tx.clone();
                let client = app.client.clone();
                app.client.runtime().spawn(async move {
                    let r = client.http().pins(channel).await;
                    let _ = tx.send((channel, r.map_err(|e| e.to_string())));
                    client.request_repaint();
                });
            }
        });
    });
}

#[derive(Clone, Copy)]
struct Row {
    id: Snowflake,
    group_start: bool,
    day_break: bool,
    new_marker: bool,
}

fn build_rows(msgs: &[Message], marker: Option<Snowflake>) -> Vec<Row> {
    let mut rows = Vec::with_capacity(msgs.len());
    let mut marker_done = false;
    for (i, m) in msgs.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| &msgs[j]);
        let local_day = |x: &Message| x.created_at().with_timezone(&chrono::Local).date_naive();
        let day_break = prev.is_none_or(|p| local_day(p) != local_day(m));
        let new_marker = !marker_done && marker.is_some_and(|mk| m.id > mk && prev.is_some_and(|p| p.id <= mk));
        if new_marker {
            marker_done = true;
        }
        let group_start = match prev {
            None => true,
            Some(p) => {
                day_break
                    || new_marker
                    || p.author.id != m.author.id
                    || !p.is_regular()
                    || !m.is_regular()
                    || m.kind == message_type::REPLY
                    || m.id.timestamp_ms().saturating_sub(p.id.timestamp_ms()) > GROUP_GAP_MS
                    || p.webhook_id.is_some()
            }
        };
        rows.push(Row { id: m.id, group_start, day_break, new_marker });
    }
    rows
}

fn messages(app: &mut App, ui: &mut Ui, channel: Snowflake) {
    let pal = app.pal;
    let (rows, reached_start, loading, n) = {
        let s = app.client.store();
        match s.messages.get(&channel) {
            Some(cm) => {
                let marker = app.scroll.get(&channel).and_then(|sc| sc.unread_marker);
                (build_rows(&cm.messages, marker), cm.reached_start, cm.loading, cm.messages.len())
            }
            None => (vec![], false, true, 0),
        }
    };
    let mut acts: Vec<Act> = vec![];
    let (sc_offset, jump_to_bottom) = match app.scroll.get_mut(&channel) {
        Some(s) => (s.pending_offset.take(), std::mem::take(&mut s.scroll_to_bottom)),
        None => (None, false),
    };
    repair_scroll_state(ui, channel);
    let mut area = egui::ScrollArea::vertical().id_salt(("chat", channel)).auto_shrink([false, false]).stick_to_bottom(true);
    if let Some(o) = sc_offset.filter(|o| o.is_finite()) {
        area = area.vertical_scroll_offset(o.max(0.0));
    }
    let first_before = rows.first().map(|r| r.id);
    let out = area.show_viewport(ui, |ui, viewport| {
        ui.set_width(ui.available_width());
        ui.add_space(12.0);
        if reached_start {
            channel_intro(app, ui, channel);
        } else if loading || n > 0 {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.spinner();
                ui.add_space(8.0);
            });
        }
        if !reached_start && !loading && viewport.min.y < 400.0 && n > 0 {
            acts.push(Act::LoadOlder);
        }
        let mut first_visible: Option<(Snowflake, f32)> = None;
        let mut rendered = 0usize;
        for row in &rows {
            let top = ui.cursor().min.y;
            let cached = app.scroll.get(&channel).and_then(|sc| sc.heights.get(&row.id).copied());
            let content_top = top - ui.min_rect().min.y;
            let visible = cached.is_none_or(|h| content_top + h >= viewport.min.y - 200.0 && content_top <= viewport.max.y + 200.0);
            if !visible {
                ui.allocate_exact_size(vec2(ui.available_width(), cached.unwrap_or(0.0)), Sense::hover());
                continue;
            }
            if first_visible.is_none() && content_top + cached.unwrap_or(0.0) >= viewport.min.y {
                first_visible = Some((row.id, content_top));
            }
            let msg = app.client.store().message(channel, row.id).cloned();
            let Some(msg) = msg else { continue };
            rendered += 1;
            if row.day_break {
                ui.add_space(6.0);
                widgets::divider_label(ui, &msg.created_at().with_timezone(&chrono::Local).format("%B %-d, %Y").to_string(), pal.text_faint, pal.border);
            }
            if row.new_marker {
                widgets::divider_label(ui, "NEW", pal.danger, pal.danger.gamma_multiply(0.6));
            }
            message(app, ui, &msg, row.group_start, &mut acts);
            let h = ui.cursor().min.y - top;
            app.scroll.entry(channel).or_default().heights.insert(row.id, h);
        }
        typing_line(app, ui, channel);
        ui.add_space(8.0);
        if jump_to_bottom {
            // Bounded by egui to the real content height (never overshoots).
            ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
        }
        app.scroll.entry(channel).or_default().rendered_rows = rendered;
        first_visible
    });

    // Keep the view anchored when older messages are prepended above.
    let sc = app.scroll.entry(channel).or_default();
    let max_off = (out.content_size.y - out.inner_rect.height()).max(0.0);
    sc.at_bottom = out.state.clamped_offset().y >= max_off - 4.0;
    if first_before.is_some()
        && let (Some((anchor_id, old_y)), Some((now_id, now_y))) = (sc.anchor, out.inner)
        && now_id == anchor_id
        && (now_y - old_y).abs() > 1.0
        && !sc.at_bottom
    {
        let target = (out.state.clamped_offset().y + (now_y - old_y)).clamp(0.0, max_off);
        sc.pending_offset = Some(target);
        ui.ctx().request_repaint();
    }
    sc.anchor = out.inner;

    if sc.at_bottom && ui.ctx().input(|i| i.viewport().focused.unwrap_or(true)) && !app.demo && app.client.config().privacy.auto_ack {
        let unread = app.client.store().is_unread(channel) || app.client.store().mention_count(channel) > 0;
        if unread {
            app.client.ack_channel(channel);
        }
    }
    apply_acts(app, channel, acts);
}

/// Resets a persisted scroll state that holds a non-finite or absurd offset.
/// Such a state renders nothing and would otherwise survive channel switches.
pub(crate) fn repair_scroll_state(ui: &Ui, channel: Snowflake) {
    let id = ui.make_persistent_id(("chat", channel));
    if let Some(mut st) = egui::scroll_area::State::load(ui.ctx(), id)
        && !scroll_state_is_sane(&st)
    {
        tracing::warn!(%channel, "repairing corrupted chat scroll state");
        st = egui::scroll_area::State::default();
        st.store(ui.ctx(), id);
    }
}

fn scroll_state_is_sane(st: &egui::scroll_area::State) -> bool {
    let (off, vel) = (st.unclamped_offset(), st.velocity());
    // Legitimate rubber-banding overscroll is a few hundred points at most.
    off.is_finite() && vel.is_finite() && (off - st.clamped_offset()).length() < 10_000.0
}

fn channel_intro(app: &App, ui: &mut Ui, channel: Snowflake) {
    let pal = app.pal;
    let name = app.client.store().channels.get(&channel).map(|c| c.display_name()).unwrap_or_default();
    egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 24, bottom: 12 }).show(ui, |ui| {
        ui.label(RichText::new(format!("Welcome to #{name}!")).font(FontId::new(26.0, theme::bold())));
        ui.label(RichText::new(format!("This is the start of #{name}.")).color(pal.text_muted));
    });
}

fn typing_line(app: &mut App, ui: &mut Ui, channel: Snowflake) {
    let users = app.client.store_mut().typing_users(channel);
    let pal = app.pal;
    let text = {
        let s = app.client.store();
        let names: Vec<String> = users.iter().take(3).map(|u| s.display_name(app.nav.guild, *u)).collect();
        match names.len() {
            0 => String::new(),
            1 => format!("{} is typing…", names[0]),
            2 => format!("{} and {} are typing…", names[0], names[1]),
            3 if users.len() == 3 => format!("{}, {} and {} are typing…", names[0], names[1], names[2]),
            _ => "Several people are typing…".into(),
        }
    };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    if !text.is_empty() {
        let t = ui.ctx().input(|i| i.time);
        for k in 0..3 {
            let a = ((t * 4.0 - k as f64).sin() * 0.5 + 0.5) as f32;
            ui.painter().circle_filled(pos2(rect.min.x + 20.0 + k as f32 * 7.0, rect.center().y), 2.5, pal.text_muted.gamma_multiply(0.4 + a * 0.6));
        }
        ui.painter().text(pos2(rect.min.x + 44.0, rect.center().y), egui::Align2::LEFT_CENTER, text, FontId::new(12.5, theme::bold()), pal.text_muted);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn author_color(app: &App, guild: Option<Snowflake>, user: Snowflake) -> Color32 {
    guild.and_then(|g| app.client.store().member_color(g, user)).and_then(theme::role_color).unwrap_or(app.pal.text)
}

fn message(app: &mut App, ui: &mut Ui, m: &Message, group_start: bool, acts: &mut Vec<Act>) {
    let pal = app.pal;
    let guild = m.guild_id.or(app.nav.guild);
    let me = app.client.store().my_id();
    let pending = app.client.store().pending_state(m).cloned();
    let mentioned = app.client.store().is_mentioned(m) && m.author.id != me;
    let compact = app.client.config().appearance.compact_messages;
    let use_24h = app.client.config().appearance.use_24h_time;

    let bg_slot = ui.painter().add(egui::Shape::Noop);
    let top = ui.cursor().min.y;
    if group_start {
        ui.add_space(if compact { 4.0 } else { 14.0 });
    }
    let row_top = ui.cursor().min.y;

    if !m.is_regular() {
        system_message(app, ui, m, acts);
        let rect = Rect::from_min_max(pos2(ui.min_rect().min.x, row_top), pos2(ui.max_rect().max.x, ui.cursor().min.y));
        let hover = ui.interact(rect, ui.scope_id().with(("msg", m.id)), Sense::hover()).hovered();
        if hover {
            ui.painter().set(bg_slot, egui::Shape::rect_filled(rect, CornerRadius::ZERO, pal.bg_hover));
        }
        return;
    }

    let full_width = ui.available_width();
    let content_w = (full_width - GUTTER - 16.0).max(80.0);
    let left = ui.cursor().min.x;

    // Reply reference.
    if let Some(r) = &m.message_reference
        && m.kind == message_type::REPLY
    {
        let (ref_text, ref_author, ref_avatar) = match &m.referenced_message {
            Some(rm) => {
                let name = app.client.store().display_name(guild, rm.author.id);
                let mut c = rm.content.replace('\n', " ");
                if c.is_empty() {
                    c = if rm.attachments.is_empty() { "Click to see message".into() } else { "Click to see attachment".into() };
                }
                (c, Some((name, author_color(app, guild, rm.author.id))), Some(rm.author.avatar_url(32)))
            }
            None => ("Original message was deleted".into(), None, None),
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(full_width, 20.0), Sense::click());
        let x0 = left + 36.0;
        ui.painter().line_segment([pos2(x0, rect.center().y + 6.0), pos2(x0, rect.center().y)], Stroke::new(2.0, pal.text_faint));
        ui.painter().line_segment([pos2(x0, rect.center().y), pos2(left + GUTTER - 6.0, rect.center().y)], Stroke::new(2.0, pal.text_faint));
        let mut x = left + GUTTER;
        if let Some(url) = ref_avatar {
            widgets::paint_avatar(ui, &app.images, &url, Rect::from_min_size(pos2(x, rect.center().y - 8.0), Vec2::splat(16.0)), None, &pal);
            x += 20.0;
        }
        if let Some((name, color)) = ref_author {
            let g = ui.ctx().fonts_mut(|f| f.layout_no_wrap(format!("@{name}"), FontId::new(13.0, theme::bold()), color));
            let w = g.size().x;
            ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
            x += w + 6.0;
        }
        let g = ui.ctx().fonts_mut(|f| {
            let mut job = egui::text::LayoutJob::simple_singleline(ref_text, FontId::proportional(13.0), pal.text_muted);
            job.wrap = egui::text::TextWrapping::truncate_at_width((rect.max.x - x - 16.0).max(20.0));
            f.layout_job(job)
        });
        ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, pal.text_muted);
        if resp.clicked()
            && let Some(id) = r.message_id
        {
            acts.push(Act::Jump(id));
        }
    }

    let show_header = group_start || m.kind == message_type::REPLY;
    ui.horizontal_top(|ui| {
        // Gutter: avatar or hover timestamp.
        let (gutter, _) = ui.allocate_exact_size(vec2(GUTTER, if show_header { 40.0 } else { 20.0 }), Sense::hover());
        if show_header && !compact {
            let av_rect = Rect::from_min_size(pos2(gutter.min.x + 16.0, gutter.min.y + 2.0), Vec2::splat(40.0));
            let url = m.member.as_ref().and_then(|mm| guild.and_then(|g| mm.avatar_url(g, 80))).unwrap_or_else(|| m.author.avatar_url(80));
            if app.client.config().appearance.show_avatars {
                widgets::paint_avatar(ui, &app.images, &url, av_rect, None, &pal);
            }
            let r = ui.interact(av_rect, ui.scope_id().with(("av", m.id)), Sense::click());
            if r.clicked() {
                acts.push(Act::Profile(m.author.id, av_rect.right_top()));
            }
            if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
        ui.vertical(|ui| {
            ui.set_width(content_w);
            // Author line and content sit tight, like the official client.
            ui.spacing_mut().item_spacing.y = 2.0;
            if show_header || compact {
                // The row height comes from the parent's interact size; without this the
                // author line is padded to 30 px and floats away from the content.
                let interact_h = ui.spacing().interact_size.y;
                ui.spacing_mut().interact_size.y = 0.0;
                let header = ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if compact {
                        ui.label(
                            RichText::new(m.created_at().with_timezone(&chrono::Local).format(if use_24h { "%H:%M" } else { "%-I:%M %p" }).to_string())
                                .size(11.5)
                                .color(pal.text_faint),
                        );
                    }
                    let name = app.client.store().display_name(guild, m.author.id);
                    let r = ui.add(
                        egui::Label::new(RichText::new(name).font(FontId::new(15.0, theme::bold())).color(author_color(app, guild, m.author.id)))
                            .sense(Sense::click()),
                    );
                    if r.clicked() {
                        acts.push(Act::Profile(m.author.id, r.rect.right_top()));
                    }
                    if m.author.bot || m.webhook_id.is_some() {
                        markdown::render::badge(ui, if m.webhook_id.is_some() { "WEBHOOK" } else { "APP" }, pal.accent, Color32::WHITE);
                    }
                    if !compact {
                        let ts = m.created_at().with_timezone(&chrono::Local);
                        let today = chrono::Local::now().date_naive();
                        let fmt = if use_24h { "%H:%M" } else { "%-I:%M %p" };
                        let label = if ts.date_naive() == today {
                            format!("Today at {}", ts.format(fmt))
                        } else if Some(ts.date_naive()) == today.pred_opt() {
                            format!("Yesterday at {}", ts.format(fmt))
                        } else {
                            ts.format(&format!("%d/%m/%Y {fmt}")).to_string()
                        };
                        ui.label(RichText::new(label).size(12.0).color(pal.text_faint)).on_hover_text(ts.format("%A, %B %-d, %Y %H:%M:%S").to_string());
                    }
                });
                ui.spacing_mut().interact_size.y = interact_h;
                crate::probe::record(ui.ctx(), "message.header", header.response.rect);
            }
            let body_top = ui.cursor().min.y;
            body(app, ui, m, guild, acts, pending.as_ref());
            if !m.content.is_empty() {
                crate::probe::record(ui.ctx(), "message.body_top", egui::Rect::from_min_size(egui::pos2(0.0, body_top), egui::Vec2::ZERO));
            }
        });
    });

    let rect = Rect::from_min_max(pos2(left, top), pos2(left + full_width, ui.cursor().min.y));
    let hover_rect = Rect::from_min_max(pos2(left, row_top - 2.0), rect.max);
    let hovered = ui.rect_contains_pointer(hover_rect) && !matches!(app.popup, Some(Popup::EmojiPicker { .. }));
    if mentioned {
        ui.painter().set(bg_slot, egui::Shape::rect_filled(hover_rect, CornerRadius::ZERO, pal.bg_mention));
        ui.painter().rect_filled(Rect::from_min_size(hover_rect.min, vec2(2.0, hover_rect.height())), CornerRadius::ZERO, pal.warning);
    } else if hovered {
        ui.painter().set(bg_slot, egui::Shape::rect_filled(hover_rect, CornerRadius::ZERO, pal.bg_hover));
    }
    if hovered && !show_header && !compact {
        let ts = m.created_at().with_timezone(&chrono::Local).format(if use_24h { "%H:%M" } else { "%-I:%M" }).to_string();
        ui.painter().text(pos2(left + GUTTER / 2.0 + 4.0, row_top + 10.0), egui::Align2::CENTER_CENTER, ts, FontId::proportional(10.5), pal.text_faint);
    }

    // Context menu on the whole message.
    let resp = ui.interact(hover_rect, ui.scope_id().with(("msgctx", m.id)), Sense::click());
    if pending.is_none() {
        message_menu(app, &resp, m, acts);
        if hovered {
            toolbar(app, ui, hover_rect, m, acts);
        }
    }
}

fn body(app: &mut App, ui: &mut Ui, m: &Message, guild: Option<Snowflake>, acts: &mut Vec<Act>, pending: Option<&PendingState>) {
    let pal = app.pal;
    let editing = app.composers.get(&m.channel_id).and_then(|c| c.editing) == Some(m.id);
    if editing {
        ui.label(RichText::new("Editing… (edit in the message box below, Esc to cancel)").italics().color(pal.text_faint));
    }
    if !m.content.is_empty() {
        let blocks = app.parsed(m.id, &m.content);
        let revealed = app.spoilers.get(&m.id).cloned().unwrap_or_default();
        let action = {
            let store = app.client.store();
            let cfg = app.client.config();
            let ctx = MdCtx {
                store: &store,
                guild,
                pal: &pal,
                images: &app.images,
                revealed: &revealed,
                size: 15.0,
                use_24h: cfg.appearance.use_24h_time,
                animate: cfg.appearance.animate_emoji,
            };
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap()));
            if pending.is_some() {
                inner.set_opacity(0.55);
            }
            let a = markdown::render(&mut inner, &blocks, &ctx);
            let used = inner.min_rect();
            ui.allocate_rect(used, Sense::hover());
            a
        };
        if m.is_edited() {
            ui.label(RichText::new("(edited)").size(10.5).color(pal.text_faint)).on_hover_text(m.edited_timestamp.clone().unwrap_or_default());
        }
        match action {
            Some(MdAction::OpenUrl(u)) => acts.push(Act::Url(u)),
            Some(MdAction::User(u, p)) => acts.push(Act::Profile(u, p)),
            Some(MdAction::Channel(c)) => acts.push(Act::Channel(c)),
            Some(MdAction::RevealSpoiler(i)) => acts.push(Act::Spoiler(m.id, i)),
            None => {}
        }
    }
    if let Some(PendingState::Failed(err)) = pending {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Failed to send: {err}")).size(12.5).color(pal.danger));
            let nonce = m.id.to_string();
            if ui.small_button("Retry").clicked() {
                acts.push(Act::Retry(nonce.clone()));
            }
            if ui.small_button("Discard").clicked() {
                acts.push(Act::Discard(nonce));
            }
        });
    }
    for a in &m.attachments {
        attachment(app, ui, m.id, a, acts);
    }
    for s in &m.sticker_items {
        let url = ferrocord_core::model::cdn::sticker(s.id, s.format_type);
        match app.images.get(&url) {
            Some(img) => {
                ui.add(egui::Image::from_texture(img.sized(Vec2::splat(160.0)))).on_hover_text(&s.name);
            }
            None => {
                ui.label(RichText::new(format!("[sticker: {}]", s.name)).color(pal.text_faint));
            }
        }
    }
    if m.flags & ferrocord_core::model::message_flags::SUPPRESS_EMBEDS == 0 {
        for e in &m.embeds {
            embed(app, ui, m, e, guild, acts);
        }
    }
    for snap in &m.message_snapshots {
        if let Some(content) = snap.message.get("content").and_then(|c| c.as_str()).filter(|c| !c.is_empty()) {
            egui::Frame::new().stroke(Stroke::new(1.0, pal.border)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::icon(ui, Icon::Reply, 14.0, pal.text_faint);
                    ui.label(RichText::new("Forwarded").size(12.0).italics().color(pal.text_faint));
                });
                ui.label(content);
            });
        }
    }
    if !m.reactions.is_empty() {
        reactions(app, ui, m, acts);
    }
}

fn attachment(app: &App, ui: &mut Ui, msg: Snowflake, a: &Attachment, acts: &mut Vec<Act>) {
    let pal = app.pal;
    let inline_media = app.client.config().appearance.inline_media;
    if a.is_image() && inline_media {
        let (w, h) = (a.width.unwrap_or(400) as f32, a.height.unwrap_or(300) as f32);
        let scale = (400.0 / w).min(320.0 / h).min(1.0);
        let size = vec2((w * scale).max(16.0), (h * scale).max(16.0));
        let url = ferrocord_core::model::cdn::proxied_image(&a.proxy_url, (size.x * 2.0) as u32, (size.y * 2.0) as u32);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let spoiler = a.is_spoiler() && !app.spoilers.get(&msg).is_some_and(|s| s.contains(&(1000 + a.id.0 as usize % 1000)));
        match app.images.get(&url) {
            Some(img) => {
                egui::Image::from_texture(egui::load::SizedTexture::new(img.texture, size)).corner_radius(CornerRadius::same(8)).paint_at(ui, rect);
            }
            None => {
                ui.painter().rect_filled(rect, CornerRadius::same(8), pal.bg_elevated);
                if !app.images.is_failed(&url) {
                    let t = ui.ctx().input(|i| i.time) as f32;
                    ui.painter().circle_stroke(rect.center(), 9.0, Stroke::new(2.0, pal.text_faint.gamma_multiply(0.4)));
                    let a0 = t * 4.0;
                    let pts: Vec<egui::Pos2> = (0..=12)
                        .map(|k| {
                            let a = a0 + k as f32 * 0.13;
                            rect.center() + vec2(a.cos(), a.sin()) * 9.0
                        })
                        .collect();
                    ui.painter().add(egui::Shape::line(pts, Stroke::new(2.0, pal.text_muted)));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
                }
            }
        }
        if spoiler {
            ui.painter().rect_filled(rect, CornerRadius::same(8), pal.bg_elevated.gamma_multiply(0.97));
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "SPOILER", FontId::new(13.0, theme::bold()), pal.text);
        }
        if resp.clicked() {
            if spoiler {
                acts.push(Act::Spoiler(msg, 1000 + a.id.0 as usize % 1000));
            } else {
                acts.push(Act::Image(ferrocord_core::model::cdn::proxied_image(&a.proxy_url, w.min(2048.0) as u32, h.min(2048.0) as u32), a.url.clone()));
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.add_space(2.0);
        return;
    }
    let icon = if a.is_video() {
        Icon::Film
    } else if a.content_type.as_deref().is_some_and(|c| c.starts_with("audio/")) {
        Icon::Music
    } else {
        Icon::File
    };
    egui::Frame::new()
        .fill(pal.bg_elevated)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_max_width(420.0);
            ui.horizontal(|ui| {
                icons::icon(ui, icon, 30.0, pal.text_muted);
                ui.vertical(|ui| {
                    if ui.link(RichText::new(&a.filename).color(pal.link)).clicked() {
                        acts.push(Act::Url(a.url.clone()));
                    }
                    ui.label(RichText::new(human_size(a.size)).size(12.0).color(pal.text_faint));
                });
            });
        });
}

fn human_size(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.2} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.1} KB", b / 1e3)
    } else {
        format!("{b} bytes")
    }
}

fn embed(app: &mut App, ui: &mut Ui, m: &Message, e: &Embed, guild: Option<Snowflake>, acts: &mut Vec<Act>) {
    let pal = app.pal;
    let inline_media = app.client.config().appearance.inline_media;
    // Bare image/gifv embeds render as media only.
    if matches!(e.kind.as_deref(), Some("image") | Some("gifv")) && e.title.is_none() && e.description.is_none() {
        let media = e.thumbnail.as_ref().or(e.image.as_ref());
        if let (Some(t), true) = (media, inline_media) {
            let src = t.proxy_url.clone().unwrap_or(t.url.clone());
            let (w, h) = (t.width.unwrap_or(300) as f32, t.height.unwrap_or(200) as f32);
            let scale = (400.0 / w).min(300.0 / h).min(1.0);
            let size = vec2(w * scale, h * scale);
            let url = if e.kind.as_deref() == Some("gifv") {
                src.clone()
            } else {
                ferrocord_core::model::cdn::proxied_image(&src, (size.x * 2.0) as u32, (size.y * 2.0) as u32)
            };
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            match app.images.get(&url) {
                Some(img) => {
                    egui::Image::from_texture(egui::load::SizedTexture::new(img.texture, size)).corner_radius(CornerRadius::same(8)).paint_at(ui, rect)
                }
                None => {
                    ui.painter().rect_filled(rect, CornerRadius::same(8), pal.bg_elevated);
                }
            }
            if resp.clicked() {
                acts.push(Act::Url(e.url.clone().unwrap_or(t.url.clone())));
            }
        }
        return;
    }
    let bar = e.color.and_then(theme::role_color).unwrap_or(pal.border);
    let resp = egui::Frame::new()
        .fill(pal.bg_elevated)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin { left: 14, right: 12, top: 10, bottom: 10 })
        .show(ui, |ui| {
            ui.set_max_width(480.0);
            if let Some(p) = &e.provider
                && let Some(n) = &p.name
            {
                ui.label(RichText::new(n).size(12.0).color(pal.text_faint));
            }
            if let Some(a) = &e.author {
                ui.horizontal(|ui| {
                    if let Some(icon) = a.proxy_icon_url.as_ref().or(a.icon_url.as_ref()) {
                        widgets::avatar(ui, &app.images, icon, 20.0, None, &pal);
                    }
                    let r = ui.add(egui::Label::new(RichText::new(&a.name).font(FontId::new(13.5, theme::bold()))).sense(Sense::click()));
                    if r.clicked()
                        && let Some(u) = &a.url
                    {
                        acts.push(Act::Url(u.clone()));
                    }
                });
            }
            if let Some(t) = &e.title {
                let r = ui.add(
                    egui::Label::new(RichText::new(t).font(FontId::new(15.0, theme::bold())).color(if e.url.is_some() { pal.link } else { pal.text }))
                        .sense(Sense::click()),
                );
                if r.clicked()
                    && let Some(u) = &e.url
                {
                    acts.push(Act::Url(u.clone()));
                }
            }
            if let Some(d) = &e.description {
                let blocks = app.parsed(Snowflake(m.id.0 ^ hash_str(d)), d);
                let store = app.client.store();
                let empty = HashSet::new();
                let ctx = MdCtx { store: &store, guild, pal: &pal, images: &app.images, revealed: &empty, size: 14.0, use_24h: true, animate: true };
                if let Some(MdAction::OpenUrl(u)) = markdown::render(ui, &blocks, &ctx) {
                    acts.push(Act::Url(u));
                }
            }
            if !e.fields.is_empty() {
                // Inline fields flow three per row; non-inline fields take a full row.
                let full = ui.available_width();
                let col = ((full - 24.0) / 3.0).max(80.0);
                let mut i = 0;
                while i < e.fields.len() {
                    let row: Vec<&ferrocord_core::model::EmbedField> =
                        if e.fields[i].inline { e.fields[i..].iter().take_while(|f| f.inline).take(3).collect() } else { vec![&e.fields[i]] };
                    i += row.len();
                    ui.horizontal_top(|ui| {
                        for f in &row {
                            let w = if row.len() == 1 && !f.inline { full } else { col };
                            ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.set_width(w);
                                ui.label(RichText::new(&f.name).font(FontId::new(13.5, theme::bold())));
                                let blocks = app.parsed(Snowflake(m.id.0 ^ hash_str(&f.value) ^ hash_str(&f.name)), &f.value);
                                let store = app.client.store();
                                let empty = HashSet::new();
                                let ctx =
                                    MdCtx { store: &store, guild, pal: &pal, images: &app.images, revealed: &empty, size: 13.5, use_24h: true, animate: true };
                                if let Some(MdAction::OpenUrl(u)) = markdown::render(ui, &blocks, &ctx) {
                                    acts.push(Act::Url(u));
                                }
                            });
                        }
                    });
                    ui.add_space(4.0);
                }
            }
            if inline_media && let Some(img) = e.image.as_ref().or(e.thumbnail.as_ref()) {
                let src = img.proxy_url.clone().unwrap_or(img.url.clone());
                let (w, h) = (img.width.unwrap_or(400) as f32, img.height.unwrap_or(250) as f32);
                let max = if e.image.is_some() { vec2(440.0, 300.0) } else { vec2(80.0, 80.0) };
                let scale = (max.x / w).min(max.y / h).min(1.0);
                let size = vec2(w * scale, h * scale);
                let url = ferrocord_core::model::cdn::proxied_image(&src, (size.x * 2.0) as u32, (size.y * 2.0) as u32);
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                match app.images.get(&url) {
                    Some(t) => {
                        egui::Image::from_texture(egui::load::SizedTexture::new(t.texture, size)).corner_radius(CornerRadius::same(4)).paint_at(ui, rect)
                    }
                    None => {
                        ui.painter().rect_filled(rect, CornerRadius::same(4), pal.bg_selected);
                    }
                }
                if resp.clicked() {
                    acts.push(Act::Image(url, img.url.clone()));
                }
            }
            if let Some(f) = &e.footer {
                ui.label(RichText::new(&f.text).size(12.0).color(pal.text_faint));
            }
        });
    let r = resp.response.rect;
    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(4.0, r.height())), CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, bar);
    ui.add_space(4.0);
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn reactions(app: &App, ui: &mut Ui, m: &Message, acts: &mut Vec<Act>) {
    let pal = app.pal;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for r in &m.reactions {
            let (fill, stroke) =
                if r.me { (pal.accent.gamma_multiply(0.22), Stroke::new(1.0, pal.accent)) } else { (pal.bg_elevated, Stroke::new(1.0, Color32::TRANSPARENT)) };
            let resp =
                egui::Frame::new().fill(fill).stroke(stroke).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let url = r.emoji.url(48).or_else(|| r.emoji.name.as_deref().and_then(markdown::twemoji_url));
                        match url.and_then(|u| app.images.get(&u)) {
                            Some(img) => {
                                ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(img.texture, Vec2::splat(18.0))));
                            }
                            None => {
                                ui.label(r.emoji.name.clone().unwrap_or_default());
                            }
                        }
                        ui.label(RichText::new(r.count.to_string()).size(13.0).color(if r.me { pal.text } else { pal.text_muted }));
                    });
                });
            let resp = resp.response.interact(Sense::click()).on_hover_text(format!(":{}:", r.emoji.name.clone().unwrap_or_default()));
            if resp.clicked() {
                acts.push(Act::React(m.id, r.emoji.clone()));
            }
        }
        let (add_rect, add) = ui.allocate_exact_size(vec2(34.0, 24.0), Sense::click());
        ui.painter().rect_filled(add_rect, CornerRadius::same(8), if add.hovered() { pal.bg_selected } else { pal.bg_elevated });
        icons::paint_centered(ui, add_rect.center(), 16.0, Icon::SmilePlus, if add.hovered() { pal.text } else { pal.text_muted });
        let add = add.on_hover_text("Add reaction");
        if add.clicked() {
            acts.push(Act::Picker(m.id, add.rect.left_top()));
        }
    });
}

fn system_message(app: &App, ui: &mut Ui, m: &Message, acts: &mut Vec<Act>) {
    let pal = app.pal;
    let name = app.client.store().display_name(m.guild_id.or(app.nav.guild), m.author.id);
    let (icon, color, text) = match m.kind {
        message_type::USER_JOIN => (Icon::UserPlus, pal.success, format!("{name} joined the server.")),
        message_type::CHANNEL_PINNED_MESSAGE => (Icon::Pin, pal.text_faint, format!("{name} pinned a message to this channel.")),
        message_type::GUILD_BOOST | message_type::GUILD_BOOST_TIER_1 | message_type::GUILD_BOOST_TIER_2 | message_type::GUILD_BOOST_TIER_3 => {
            (Icon::Shield, theme::hex(0xff73fa), format!("{name} just boosted the server!"))
        }
        message_type::RECIPIENT_ADD => (
            Icon::UserPlus,
            pal.success,
            format!("{name} added {} to the group.", m.mentions.first().map(|u| u.display_name().to_string()).unwrap_or_default()),
        ),
        message_type::RECIPIENT_REMOVE => (Icon::LogOut, pal.danger, format!("{name} left the group.")),
        message_type::CALL => (Icon::Phone, pal.success, format!("{name} started a call.")),
        message_type::CHANNEL_NAME_CHANGE => (Icon::Pencil, pal.text_faint, format!("{name} changed the channel name: {}", m.content)),
        message_type::THREAD_CREATED => (Icon::Thread, pal.text_faint, format!("{name} started a thread: {}", m.content)),
        message_type::CHANNEL_FOLLOW_ADD => (Icon::Megaphone, pal.text_faint, format!("{name} added {} to this channel.", m.content)),
        message_type::AUTO_MODERATION_ACTION => (Icon::Shield, pal.warning, "AutoMod blocked a message.".to_string()),
        _ => (Icon::Hash, pal.text_faint, if m.content.is_empty() { format!("{name} did something.") } else { m.content.clone() }),
    };
    ui.horizontal(|ui| {
        // Icon centered in the avatar column so it lines up with avatars above and below.
        let (gutter, _) = ui.allocate_exact_size(vec2(GUTTER, 20.0), Sense::hover());
        icons::paint_centered(ui, pos2(gutter.min.x + 36.0, gutter.center().y), 16.0, icon, color);
        let r = ui.add(egui::Label::new(RichText::new(text).color(pal.text_muted)).sense(Sense::click()));
        if r.clicked()
            && m.kind == message_type::CHANNEL_PINNED_MESSAGE
            && let Some(id) = m.message_reference.as_ref().and_then(|r| r.message_id)
        {
            acts.push(Act::Jump(id));
        }
        ui.label(RichText::new(m.created_at().with_timezone(&chrono::Local).format("%d/%m/%Y %H:%M").to_string()).size(11.5).color(pal.text_faint));
    });
}

fn can(app: &App, channel: Snowflake, p: Permissions) -> bool {
    app.client.store().permissions(channel).contains(p)
}

fn toolbar(app: &App, ui: &mut Ui, rect: Rect, m: &Message, acts: &mut Vec<Act>) {
    let pal = app.pal;
    let me = app.client.store().my_id();
    let mine = m.author.id == me;
    let mut buttons: Vec<(Icon, &str, u8)> = vec![(Icon::SmilePlus, "Add reaction", 0), (Icon::Reply, "Reply", 1)];
    if mine {
        buttons.push((Icon::Pencil, "Edit", 2));
    }
    if mine || can(app, m.channel_id, Permissions::MANAGE_MESSAGES) {
        buttons.push((Icon::Trash, "Delete", 3));
    }
    let w = buttons.len() as f32 * 32.0 + 6.0;
    let bar = Rect::from_min_size(pos2(rect.max.x - w - 16.0, rect.min.y - 14.0), vec2(w, 32.0));
    egui::Area::new(ui.scope_id().with(("toolbar", m.id))).fixed_pos(bar.min).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::new()
            .fill(pal.bg_side)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::same(2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (icon, tip, k) in buttons {
                        let r = widgets::icon_button(ui, icon, tip, &pal, false);
                        if r.clicked() {
                            match k {
                                0 => acts.push(Act::Picker(m.id, r.rect.left_bottom())),
                                1 => acts.push(Act::Reply(m.id)),
                                2 => acts.push(Act::Edit(m.id, m.content.clone())),
                                _ => acts.push(Act::Delete(m.id)),
                            }
                        }
                    }
                });
            });
    });
}

fn message_menu(app: &App, resp: &egui::Response, m: &Message, acts: &mut Vec<Act>) {
    let me = app.client.store().my_id();
    let mine = m.author.id == me;
    let can_manage = can(app, m.channel_id, Permissions::MANAGE_MESSAGES);
    let can_pin = can_manage || can(app, m.channel_id, Permissions::PIN_MESSAGES) || app.nav.guild.is_none();
    let link = format!("https://discord.com/channels/{}/{}/{}", m.guild_id.map(|g| g.to_string()).unwrap_or("@me".into()), m.channel_id, m.id);
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        let fg = app.pal.text;
        if widgets::menu_item(ui, Icon::SmilePlus, "Add reaction", fg) {
            acts.push(Act::Picker(m.id, ui.min_rect().left_top()));
            ui.close();
        }
        if widgets::menu_item(ui, Icon::Reply, "Reply", fg) {
            acts.push(Act::Reply(m.id));
            ui.close();
        }
        if mine && widgets::menu_item(ui, Icon::Pencil, "Edit message", fg) {
            acts.push(Act::Edit(m.id, m.content.clone()));
            ui.close();
        }
        if can_pin && widgets::menu_item(ui, Icon::Pin, if m.pinned { "Unpin message" } else { "Pin message" }, fg) {
            acts.push(Act::Pin(m.id, !m.pinned));
            ui.close();
        }
        ui.separator();
        if widgets::menu_item(ui, Icon::Copy, "Copy text", fg) {
            ui.ctx().copy_text(m.content.clone());
            ui.close();
        }
        if widgets::menu_item(ui, Icon::Copy, "Copy message link", fg) {
            ui.ctx().copy_text(link.clone());
            ui.close();
        }
        if widgets::menu_item(ui, Icon::Copy, "Copy message ID", fg) {
            ui.ctx().copy_text(m.id.to_string());
            ui.close();
        }
        if mine || can_manage {
            ui.separator();
            if widgets::menu_item(ui, Icon::Trash, "Delete message", app.pal.danger) {
                acts.push(Act::Delete(m.id));
                ui.close();
            }
        }
    });
}

fn apply_acts(app: &mut App, channel: Snowflake, acts: Vec<Act>) {
    for a in acts {
        match a {
            Act::Reply(id) => {
                let c = app.composers.entry(channel).or_default();
                c.reply_to = Some(id);
                c.reply_mention = true;
                c.editing = None;
            }
            Act::Edit(id, content) => {
                let c = app.composers.entry(channel).or_default();
                c.editing = Some(id);
                c.reply_to = None;
                c.text = content;
            }
            Act::Delete(id) => app.modal = Some(Modal::ConfirmDelete { channel, message: id }),
            Act::React(id, e) => {
                if app.demo {
                    continue;
                }
                app.client.toggle_reaction(channel, id, e);
            }
            Act::Picker(id, at) => app.popup = Some(Popup::EmojiPicker { target: EmojiTarget::Reaction(channel, id), at }),
            Act::Profile(user, at) => app.popup = Some(Popup::Profile { user, guild: app.nav.guild, at }),
            Act::Url(u) => crate::views::open_link(app, &u),
            Act::Channel(c) => {
                if app.client.store().channels.contains_key(&c) {
                    app.select_channel(c);
                }
            }
            Act::Jump(id) => {
                if app.client.store().message(channel, id).is_none() && !app.demo {
                    app.client.load_around(channel, id);
                }
            }
            Act::Image(url, open) => app.modal = Some(Modal::ImageViewer { url, open_url: open }),
            Act::Pin(id, pin) => app.client.set_pinned(channel, id, pin),
            Act::Spoiler(id, idx) => {
                app.spoilers.entry(id).or_default().insert(idx);
            }
            Act::Retry(nonce) => app.client.retry_pending(channel, &nonce),
            Act::Discard(nonce) => app.client.discard_pending(&nonce),
            Act::LoadOlder => {
                if app.demo {
                    continue;
                }
                let oldest = app.client.store().messages.get(&channel).and_then(|cm| cm.messages.first().map(|m| m.id));
                if let Some(o) = oldest {
                    app.client.load_messages(channel, Some(o));
                }
            }
        }
    }
}

// ---------------------------------------------------------------- composer

fn composer(app: &mut App, ui: &mut Ui, channel: Snowflake) {
    let pal = app.pal;
    let (can_send, can_attach, placeholder, guild) = {
        let s = app.client.store();
        let perms = s.permissions(channel);
        let c = s.channels.get(&channel);
        let name = match c {
            Some(c) if c.kind == ChannelType::Dm => format!("@{}", c.recipient_ids().first().map(|u| s.display_name(None, *u)).unwrap_or_default()),
            Some(c) if c.kind == ChannelType::GroupDm => c.display_name(),
            Some(c) => format!("#{}", c.name.clone().unwrap_or_default()),
            None => String::new(),
        };
        let send_perm = if c.is_some_and(|c| c.kind.is_thread()) { Permissions::SEND_MESSAGES_IN_THREADS } else { Permissions::SEND_MESSAGES };
        (perms.contains(send_perm), perms.contains(Permissions::ATTACH_FILES), format!("Message {name}"), c.and_then(|c| c.guild_id))
    };
    if !can_send {
        egui::Frame::new().fill(pal.bg_elevated).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("You do not have permission to send messages in this channel.").color(pal.text_faint));
        });
        ui.add_space(18.0);
        return;
    }

    // Reply / edit bar.
    let (reply_to, editing) = app.composers.get(&channel).map(|c| (c.reply_to, c.editing)).unwrap_or((None, None));
    if let Some(r) = reply_to {
        let name = app.client.store().message(channel, r).map(|m| app.client.store().display_name(guild, m.author.id)).unwrap_or_default();
        egui::Frame::new().fill(pal.bg_side).corner_radius(CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }).inner_margin(egui::Margin::symmetric(12, 6)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Replying to").size(13.0).color(pal.text_muted));
                    ui.label(RichText::new(name).size(13.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::Close, "Cancel reply", &pal, false).clicked() {
                            app.composers.entry(channel).or_default().reply_to = None;
                        }
                        let c = app.composers.entry(channel).or_default();
                        let label = if c.reply_mention { "@ ON" } else { "@ OFF" };
                        if ui
                            .small_button(RichText::new(label).color(if c.reply_mention { pal.link } else { pal.text_faint }))
                            .on_hover_text("Mention the author")
                            .clicked()
                        {
                            c.reply_mention = !c.reply_mention;
                        }
                    });
                });
            },
        );
    }
    if editing.is_some() {
        egui::Frame::new().fill(pal.bg_side).corner_radius(CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }).inner_margin(egui::Margin::symmetric(12, 6)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Editing message — Enter to save, Esc to cancel").size(13.0).color(pal.text_muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::Close, "Cancel edit", &pal, false).clicked() {
                            let c = app.composers.entry(channel).or_default();
                            c.editing = None;
                            c.text.clear();
                        }
                    });
                });
            },
        );
    }

    // Pending attachments.
    let files = app.composers.get(&channel).map(|c| c.files.clone()).unwrap_or_default();
    if !files.is_empty() {
        ui.horizontal_wrapped(|ui| {
            let mut remove = None;
            for (i, f) in files.iter().enumerate() {
                egui::Frame::new().fill(pal.bg_elevated).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        icons::icon(ui, Icon::File, 16.0, pal.text_muted);
                        ui.label(f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                        if widgets::icon_button(ui, Icon::Close, "Remove", &pal, false).clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove {
                app.composers.entry(channel).or_default().files.remove(i);
            }
        });
    }

    autocomplete(app, ui, channel, guild);

    let mut send = false;
    let autocomplete_open = !suggestions(app, channel, guild).is_empty();
    // Enter sends; Shift+Enter inserts a newline. Swallow Enter while autocomplete is open.
    let enter = !autocomplete_open && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    let up_edit = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));

    let bar = egui::Frame::new()
        .fill(pal.bg_elevated)
        .corner_radius(if reply_to.is_some() || editing.is_some() { CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 } } else { CornerRadius::same(8) })
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            // Fixed first-row height so buttons center on the first line even when the box grows.
            let row_h = ui.fonts_mut(|f| f.row_height(&FontId::proportional(15.0))) + 18.0;
            ui.allocate_ui_with_layout(vec2(ui.available_width(), row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if can_attach {
                    let r = widgets::icon_button(ui, Icon::CirclePlus, "Upload a file", &pal, false);
                    crate::probe::record(ui.ctx(), "composer.attach", r.rect);
                    if r.clicked()
                        && let Some(paths) = rfd::FileDialog::new().pick_files()
                    {
                        app.composers.entry(channel).or_default().files.extend(paths);
                    }
                }
                let avail = ui.available_width() - 34.0;
                let c = app.composers.entry(channel).or_default();
                let te = egui::TextEdit::multiline(&mut c.text)
                    .id(egui::Id::unique(("composer", channel)))
                    .hint_text(RichText::new(&placeholder).color(pal.text_faint))
                    .desired_rows(1)
                    .desired_width(avail)
                    // Padding lives on the frame: egui ignores `.margin()` once `.frame()` is set.
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 9)))
                    .return_key(Some(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter)))
                    .font(FontId::proportional(15.0));
                let r = ui.add(te);
                crate::probe::record(ui.ctx(), "composer.input", r.rect);
                if r.changed() && !c.text.is_empty() && !c.text.starts_with('/') && !app.demo {
                    app.client.typing(channel);
                }
                if r.has_focus() && enter {
                    send = true;
                }
                // Up arrow in an empty box edits your last message.
                let c = app.composers.entry(channel).or_default();
                if r.has_focus() && up_edit && c.text.is_empty() && c.editing.is_none() {
                    let me = app.client.store().my_id();
                    let last = app
                        .client
                        .store()
                        .messages
                        .get(&channel)
                        .and_then(|cm| cm.messages.iter().rev().find(|m| m.author.id == me).map(|m| (m.id, m.content.clone())));
                    if let Some((id, content)) = last {
                        let c = app.composers.entry(channel).or_default();
                        c.editing = Some(id);
                        c.text = content;
                    }
                }
                if app.modal.is_none() && app.popup.is_none() && !ui.ctx().memory(|m| m.focused().is_some()) {
                    r.request_focus();
                }
                let r = widgets::icon_button(ui, Icon::Smile, "Emoji", &pal, false);
                crate::probe::record(ui.ctx(), "composer.emoji", r.rect);
                if r.clicked() {
                    app.popup = Some(Popup::EmojiPicker { target: EmojiTarget::Composer(channel), at: ui.min_rect().right_top() });
                }
            });
        });
    crate::probe::record(ui.ctx(), "composer.bar", bar.response.rect);

    // Files dropped onto the window are attached.
    let dropped: Vec<std::path::PathBuf> =
        ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
    if !dropped.is_empty() && can_attach {
        app.composers.entry(channel).or_default().files.extend(dropped);
    }

    if send {
        submit(app, channel);
    }
    ui.add_space(14.0);
}

fn submit(app: &mut App, channel: Snowflake) {
    let c = app.composers.entry(channel).or_default();
    let mut text = c.text.trim_end().to_string();
    for (display, raw) in &c.replacements {
        text = text.replace(display, raw);
    }
    let files = std::mem::take(&mut c.files);
    let reply = c.reply_to.take();
    let mention = c.reply_mention;
    let editing = c.editing.take();
    c.text.clear();
    c.replacements.clear();
    if let Some(id) = editing {
        if !text.is_empty() {
            app.client.edit_message(channel, id, text);
        }
        return;
    }
    if text.is_empty() && files.is_empty() {
        return;
    }
    app.scroll.entry(channel).or_default().scroll_to_bottom = true;
    if app.demo {
        crate::demo::local_send(&app.client, channel, text);
        return;
    }
    let parts: Vec<ferrocord_core::http::FilePart> = files
        .iter()
        .filter_map(|p| {
            let data = std::fs::read(p).ok()?;
            Some(ferrocord_core::http::FilePart { filename: p.file_name()?.to_string_lossy().to_string(), content_type: None, data: std::sync::Arc::new(data) })
        })
        .collect();
    app.client.send_message(channel, text, reply, mention, parts);
}

#[derive(Clone)]
struct Suggestion {
    label: String,
    detail: String,
    insert: String,
    raw: Option<String>,
    icon: Option<String>,
}

fn current_token(text: &str) -> Option<&str> {
    let start = text.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
    let tok = &text[start..];
    (!tok.is_empty()).then_some(tok)
}

fn suggestions(app: &App, channel: Snowflake, guild: Option<Snowflake>) -> Vec<Suggestion> {
    let Some(c) = app.composers.get(&channel) else { return vec![] };
    let Some(tok) = current_token(&c.text) else { return vec![] };
    let s = app.client.store();
    let mut out = vec![];
    if let Some(q) = tok.strip_prefix('@') {
        let q = q.to_lowercase();
        let mut seen = HashSet::new();
        let candidates: Vec<Snowflake> = match guild {
            Some(g) => s.members.keys().filter(|(gg, _)| *gg == g).map(|(_, u)| *u).collect(),
            None => s.channels.get(&channel).map(|c| c.recipient_ids()).unwrap_or_default(),
        };
        for u in candidates {
            if out.len() >= 8 {
                break;
            }
            let Some(user) = s.user(u) else { continue };
            let dn = s.display_name(guild, u);
            if (user.username.to_lowercase().starts_with(&q) || dn.to_lowercase().starts_with(&q)) && seen.insert(u) {
                out.push(Suggestion {
                    label: dn.clone(),
                    detail: user.username.clone(),
                    insert: format!("@{} ", user.username),
                    raw: Some(format!("<@{u}>")),
                    icon: Some(user.avatar_url(32)),
                });
            }
        }
        for w in ["everyone", "here"] {
            if guild.is_some() && w.starts_with(&q) && out.len() < 8 {
                out.push(Suggestion { label: format!("@{w}"), detail: "Notify members".into(), insert: format!("@{w} "), raw: None, icon: None });
            }
        }
    } else if let Some(q) = tok.strip_prefix(':').filter(|q| q.len() >= 2 && !q.ends_with(':')) {
        let q = q.to_lowercase();
        if let Some(g) = guild.and_then(|g| s.guilds.get(&g)) {
            for e in g.emojis.iter().filter(|e| e.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&q))).take(5) {
                let name = e.name.clone().unwrap_or_default();
                let raw = format!("<{}:{}:{}>", if e.animated { "a" } else { "" }, name, e.id.unwrap_or_default());
                out.push(Suggestion { label: format!(":{name}:"), detail: g.name().into(), insert: format!(":{name}: "), raw: Some(raw), icon: e.url(32) });
            }
        }
        for e in emojis::iter().filter(|e| e.shortcodes().any(|sc| sc.contains(&q))).take(8 - out.len().min(8)) {
            let sc = e.shortcode().unwrap_or(e.name());
            out.push(Suggestion { label: format!("{} :{sc}:", e.as_str()), detail: String::new(), insert: format!("{} ", e.as_str()), raw: None, icon: None });
        }
    } else if let Some(q) = tok.strip_prefix('#').filter(|_| guild.is_some()) {
        let q = q.to_lowercase();
        for ch in s.channels.values().filter(|c| c.guild_id == guild && !c.kind.is_voice() && c.kind != ChannelType::GuildCategory) {
            let name = ch.name.clone().unwrap_or_default();
            if name.to_lowercase().starts_with(&q) && out.len() < 8 {
                out.push(Suggestion {
                    label: format!("#{name}"),
                    detail: String::new(),
                    insert: format!("#{name} "),
                    raw: Some(format!("<#{}>", ch.id)),
                    icon: None,
                });
            }
        }
    } else if c.text.starts_with('/') && !c.text.contains(' ') {
        let q = &c.text[1..];
        for cmd in app.client.local_commands().into_iter().filter(|c| c.name.starts_with(q)).take(8) {
            out.push(Suggestion { label: format!("/{}", cmd.name), detail: cmd.description, insert: format!("/{} ", cmd.name), raw: None, icon: None });
        }
    }
    out
}

fn autocomplete(app: &mut App, ui: &mut Ui, channel: Snowflake, guild: Option<Snowflake>) {
    let items = suggestions(app, channel, guild);
    if items.is_empty() {
        return;
    }
    let pal = app.pal;
    let (up, down, pick) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Tab) || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        )
    });
    let c = app.composers.entry(channel).or_default();
    let n = items.len();
    if up {
        c.autocomplete_index = (c.autocomplete_index + n - 1) % n;
    }
    if down {
        c.autocomplete_index = (c.autocomplete_index + 1) % n;
    }
    c.autocomplete_index = c.autocomplete_index.min(n - 1);
    let mut chosen = pick.then_some(c.autocomplete_index);
    let selected = c.autocomplete_index;
    egui::Frame::new().fill(pal.bg_side).stroke(Stroke::new(1.0, pal.border)).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(6)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            for (i, s) in items.iter().enumerate() {
                let (rect, resp) = widgets::list_row(ui, 30.0, i == selected, &pal);
                let mut x = rect.min.x + 8.0;
                if let Some(url) = &s.icon {
                    widgets::paint_avatar(ui, &app.images, url, Rect::from_min_size(pos2(x, rect.center().y - 10.0), Vec2::splat(20.0)), None, &pal);
                    x += 28.0;
                }
                ui.painter().text(pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, &s.label, FontId::proportional(14.0), pal.text);
                ui.painter().text(pos2(rect.max.x - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, &s.detail, FontId::proportional(12.0), pal.text_faint);
                if resp.clicked() {
                    chosen = Some(i);
                }
            }
        },
    );
    if let Some(i) = chosen {
        let s = &items[i];
        let c = app.composers.entry(channel).or_default();
        if let Some(tok) = current_token(&c.text) {
            let cut = c.text.len() - tok.len();
            c.text.truncate(cut);
            c.text.push_str(&s.insert);
            if let Some(raw) = &s.raw {
                c.replacements.push((s.insert.trim_end().to_string(), raw.clone()));
            }
        }
        c.autocomplete_index = 0;
        ui.ctx().memory_mut(|m| m.request_focus(egui::Id::unique(("composer", channel))));
    }
}
