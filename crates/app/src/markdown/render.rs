//! Renders parsed markdown with egui.
//!
//! Each paragraph is laid out as a single galley (fast, wraps naturally). Inline
//! objects — links, mentions, spoilers, emoji — are tracked as character ranges;
//! after layout we hit-test the galley for clicks and paint emoji images and
//! spoiler covers on top of placeholder glyphs.

use std::collections::HashSet;
use std::ops::Range;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use ferrocord_core::model::Snowflake;
use ferrocord_core::store::Store;
use unicode_segmentation::UnicodeSegmentation;

use super::parse::{Block, Inline, Style};
use crate::images::ImageCache;
use crate::theme::{self, Palette};

/// Something the user clicked inside rendered markdown.
#[derive(Debug, Clone)]
pub enum MdAction {
    OpenUrl(String),
    User(Snowflake, egui::Pos2),
    Channel(Snowflake),
    RevealSpoiler(usize),
}

pub struct MdCtx<'a> {
    pub store: &'a Store,
    pub guild: Option<Snowflake>,
    pub pal: &'a Palette,
    pub images: &'a ImageCache,
    /// Spoiler indices (within this message) the user has revealed.
    pub revealed: &'a HashSet<usize>,
    pub size: f32,
    pub use_24h: bool,
    pub animate: bool,
}

#[derive(Clone)]
enum Span {
    Link(String),
    User(Snowflake),
    Channel(Snowflake),
    Tooltip(String),
}

struct EmojiSlot {
    char_index: usize,
    url: String,
    size: f32,
}

struct Builder<'a, 'b> {
    ctx: &'a MdCtx<'b>,
    job: LayoutJob,
    chars: usize,
    spans: Vec<(Range<usize>, Span)>,
    emoji: Vec<EmojiSlot>,
    spoilers: Vec<(Range<usize>, usize)>,
    size: f32,
    color: Color32,
    family: FontFamily,
    emoji_size: f32,
}

impl<'a, 'b> Builder<'a, 'b> {
    fn new(ctx: &'a MdCtx<'b>, size: f32, color: Color32, family: FontFamily, emoji_size: f32) -> Self {
        Self { ctx, job: LayoutJob::default(), chars: 0, spans: vec![], emoji: vec![], spoilers: vec![], size, color, family, emoji_size }
    }

    fn format(&self, style: Style) -> TextFormat {
        let p = self.ctx.pal;
        let family = if style.code {
            FontFamily::Monospace
        } else if style.bold {
            theme::bold()
        } else if style.italic {
            theme::italic()
        } else {
            self.family.clone()
        };
        let size = if style.code { self.size * 0.9 } else { self.size };
        let mut f = TextFormat { font_id: FontId::new(size, family), color: self.color, ..Default::default() };
        if style.code {
            f.background = p.bg_elevated;
        }
        if style.underline {
            f.underline = Stroke::new(1.0, self.color);
        }
        if style.strike {
            f.strikethrough = Stroke::new(1.0, self.color);
        }
        if let Some(i) = style.spoiler
            && self.ctx.revealed.contains(&i)
        {
            f.background = p.bg_selected;
        }
        f
    }

    fn push(&mut self, text: &str, format: TextFormat, span: Option<Span>, spoiler: Option<usize>) {
        if text.is_empty() {
            return;
        }
        let n = text.chars().count();
        let range = self.chars..self.chars + n;
        self.job.append(text, 0.0, format);
        if let Some(s) = span {
            self.spans.push((range.clone(), s));
        }
        if let Some(sp) = spoiler {
            self.spoilers.push((range.clone(), sp));
        }
        self.chars += n;
    }

    /// Appends text, swapping unicode emoji for Twemoji image placeholders.
    fn push_text(&mut self, text: &str, style: Style) {
        let fmt = self.format(style);
        if style.code || !text.chars().any(|c| c as u32 > 0x2000) {
            self.push(text, fmt, None, style.spoiler);
            return;
        }
        let mut buf = String::new();
        for g in text.graphemes(true) {
            match twemoji_url(g) {
                Some(url) => {
                    let f = fmt.clone();
                    self.push(&std::mem::take(&mut buf), f, None, style.spoiler);
                    self.push_emoji(url, g, style);
                }
                None => buf.push_str(g),
            }
        }
        self.push(&buf, fmt, None, style.spoiler);
    }

    fn push_emoji(&mut self, url: String, alt: &str, style: Style) {
        let size = self.emoji_size;
        // An em space is exactly one font-size wide: a perfect image placeholder.
        let fmt = TextFormat { font_id: FontId::new(size, FontFamily::Proportional), color: Color32::TRANSPARENT, ..Default::default() };
        self.emoji.push(EmojiSlot { char_index: self.chars, url, size });
        self.push("\u{2003}", fmt, Some(Span::Tooltip(alt.to_string())), style.spoiler);
    }

    fn pill(&mut self, text: &str, style: Style, color: Color32, span: Span) {
        let p = self.ctx.pal;
        let mut f = self.format(style);
        f.color = if color == p.text { p.accent.lerp_to_gamma(Color32::WHITE, 0.55) } else { color };
        f.background = color.gamma_multiply(0.18);
        f.font_id.family = theme::bold();
        self.push(text, f, Some(span), style.spoiler);
    }

    fn inline(&mut self, item: &Inline) {
        let p = self.ctx.pal;
        let store = self.ctx.store;
        match item {
            Inline::Text(t, s) => self.push_text(t, *s),
            Inline::Link { text, url, style } => {
                let mut f = self.format(*style);
                f.color = p.link;
                self.push(text, f, Some(Span::Link(url.clone())), style.spoiler);
            }
            Inline::User(id, s) => {
                let name = format!("@{}", store.display_name(self.ctx.guild, *id));
                self.pill(&name, *s, p.text, Span::User(*id));
            }
            Inline::Role(id, s) => {
                let role = self.ctx.guild.and_then(|g| store.guilds.get(&g)).and_then(|g| g.roles.iter().find(|r| r.id == *id));
                let (name, color) = match role {
                    Some(r) => (format!("@{}", r.name), theme::role_color(r.color).unwrap_or(p.text)),
                    None => ("@unknown-role".into(), p.text),
                };
                self.pill(&name, *s, color, Span::Tooltip(name.clone()));
            }
            Inline::Channel(id, s) => {
                let name = store.channels.get(id).and_then(|c| c.name.clone()).map(|n| format!("#{n}")).unwrap_or_else(|| "#unknown".into());
                self.pill(&name, *s, p.text, Span::Channel(*id));
            }
            Inline::Everyone(w, s) => self.pill(w, *s, p.text, Span::Tooltip(w.clone())),
            Inline::Emoji { id, name, animated, style } => {
                let url = ferrocord_core::model::cdn::emoji(*id, *animated && self.ctx.animate, 96);
                self.push_emoji(url, &format!(":{name}:"), *style);
            }
            Inline::Timestamp { unix, format, style } => {
                let (short, full) = format_timestamp(*unix, *format, self.ctx.use_24h);
                let mut f = self.format(*style);
                f.background = p.bg_elevated;
                self.push(&short, f, Some(Span::Tooltip(full)), style.spoiler);
            }
        }
    }

    /// Lays out and paints the paragraph; returns a click action if any.
    fn finish(self, ui: &mut Ui, indent: f32) -> Option<MdAction> {
        let Builder { ctx, mut job, spans, emoji, spoilers, .. } = self;
        if job.text.is_empty() {
            return None;
        }
        let width = (ui.available_width() - indent).max(40.0);
        job.wrap.max_width = width;
        let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
        let (rect, resp) = ui.allocate_exact_size(vec2(width, galley.size().y) + vec2(indent, 0.0), Sense::click());
        let origin = rect.min + vec2(indent, 0.0);
        if !ui.is_rect_visible(rect) {
            return None;
        }
        let painter = ui.painter();
        painter.galley(origin, galley.clone(), ctx.pal.text);

        let glyph_rect = |i: usize| galley.pos_from_cursor(CCursor::new(i)).translate(origin.to_vec2());

        for e in &emoji {
            let at = glyph_rect(e.char_index);
            let r = Rect::from_center_size(pos2(at.min.x + e.size / 2.0, at.center().y), Vec2::splat(e.size));
            if let Some(img) = ctx.images.get(&e.url) {
                painter.image(img.texture, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                painter.rect_filled(r.shrink(2.0), CornerRadius::same(4), ctx.pal.bg_elevated);
            }
        }

        // Covers for unrevealed spoilers, one rect per wrapped row.
        let mut hit_spoiler = None;
        let pointer = ui.ctx().pointer_latest_pos();
        for (range, idx) in &spoilers {
            if ctx.revealed.contains(idx) {
                continue;
            }
            let mut row: Option<Rect> = None;
            let mut rows = vec![];
            for i in range.clone() {
                let a = glyph_rect(i);
                let b = glyph_rect(i + 1);
                let r = if (b.min.y - a.min.y).abs() < 1.0 { Rect::from_min_max(a.min, pos2(b.min.x, a.max.y)) } else { a.expand2(vec2(4.0, 0.0)) };
                match row {
                    Some(cur) if (cur.min.y - r.min.y).abs() < 1.0 => row = Some(cur.union(r)),
                    Some(cur) => {
                        rows.push(cur);
                        row = Some(r);
                    }
                    None => row = Some(r),
                }
            }
            rows.extend(row);
            for r in rows {
                let r = r.expand2(vec2(1.0, 0.0));
                painter.rect_filled(r, CornerRadius::same(3), ctx.pal.bg_selected.lerp_to_gamma(Color32::BLACK, 0.25));
                if pointer.is_some_and(|p| r.contains(p)) {
                    hit_spoiler = Some(*idx);
                }
            }
        }

        let hovered_span = resp.hover_pos().and_then(|pos| {
            let c = galley.cursor_from_pos(pos - origin);
            spans.iter().find(|(r, _)| r.contains(&c.index.0)).map(|(_, s)| s.clone())
        });
        if let Some(idx) = hit_spoiler {
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                return Some(MdAction::RevealSpoiler(idx));
            }
            return None;
        }
        match hovered_span {
            Some(Span::Link(url)) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    return Some(MdAction::OpenUrl(url));
                }
                resp.on_hover_text(url);
            }
            Some(Span::User(id)) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    return Some(MdAction::User(id, resp.interact_pointer_pos().unwrap_or(rect.left_bottom())));
                }
            }
            Some(Span::Channel(id)) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    return Some(MdAction::Channel(id));
                }
            }
            Some(Span::Tooltip(t)) => {
                resp.on_hover_text(t);
            }
            None => {}
        }
        None
    }
}

/// Twemoji asset URL for a grapheme if it is an emoji.
pub fn twemoji_url(g: &str) -> Option<String> {
    if g.is_ascii() || g == "\u{2003}" {
        return None;
    }
    let e = emojis::get(g)?;
    let s = e.as_str();
    let has_zwj = s.contains('\u{200d}');
    let cps: Vec<String> = s.chars().filter(|c| has_zwj || *c != '\u{fe0f}').map(|c| format!("{:x}", c as u32)).collect();
    Some(format!("https://cdn.jsdelivr.net/gh/jdecked/twemoji@15.1.0/assets/72x72/{}.png", cps.join("-")))
}

fn is_emoji_only(blocks: &[Block]) -> bool {
    let [Block::Paragraph(items)] = blocks else { return false };
    let mut count = 0;
    for i in items {
        match i {
            Inline::Emoji { .. } => count += 1,
            Inline::Text(t, s) if !s.code => {
                for g in t.graphemes(true) {
                    if g.trim().is_empty() {
                        continue;
                    }
                    if twemoji_url(g).is_none() {
                        return false;
                    }
                    count += 1;
                }
            }
            _ => return false,
        }
    }
    count > 0 && count <= 30
}

pub fn render(ui: &mut Ui, blocks: &[Block], ctx: &MdCtx<'_>) -> Option<MdAction> {
    let jumbo = is_emoji_only(blocks);
    render_blocks(ui, blocks, ctx, 0.0, jumbo)
}

fn render_blocks(ui: &mut Ui, blocks: &[Block], ctx: &MdCtx<'_>, indent: f32, jumbo: bool) -> Option<MdAction> {
    let mut action = None;
    let p = ctx.pal;
    let base = ctx.size;
    let prev_spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.0;
    for block in blocks {
        let a = match block {
            Block::Paragraph(items) => {
                if items.is_empty() {
                    ui.add_space(base * 0.6);
                    None
                } else {
                    let es = if jumbo { 48.0 } else { base * 1.3 };
                    let mut b = Builder::new(ctx, base, p.text, FontFamily::Proportional, es);
                    items.iter().for_each(|i| b.inline(i));
                    b.finish(ui, indent)
                }
            }
            Block::Heading(level, items) => {
                let size = match level {
                    1 => base * 1.6,
                    2 => base * 1.35,
                    _ => base * 1.15,
                };
                ui.add_space(4.0);
                let mut b = Builder::new(ctx, size, p.text, theme::bold(), size * 1.1);
                items.iter().for_each(|i| b.inline(i));
                b.finish(ui, indent)
            }
            Block::Subtext(items) => {
                let size = base * 0.8;
                let mut b = Builder::new(ctx, size, p.text_faint, FontFamily::Proportional, size * 1.2);
                items.iter().for_each(|i| b.inline(i));
                b.finish(ui, indent)
            }
            Block::ListItem { depth, marker, content } => {
                let ind = indent + 8.0 + *depth as f32 * 18.0;
                let mut b = Builder::new(ctx, base, p.text, FontFamily::Proportional, base * 1.3);
                b.push(&format!("{marker} "), TextFormat { font_id: FontId::proportional(base), color: p.text_muted, ..Default::default() }, None, None);
                content.iter().for_each(|i| b.inline(i));
                b.finish(ui, ind)
            }
            Block::Code { lang, code } => {
                let mut copy = false;
                egui::Frame::new()
                    .fill(p.bg_elevated)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .outer_margin(egui::Margin { left: indent as i8, top: 2, bottom: 2, right: 0 })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if !lang.is_empty() {
                                ui.label(egui::RichText::new(lang).small().color(p.text_faint));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Copy").clicked() {
                                    copy = true;
                                }
                            });
                        });
                        ui.add(egui::Label::new(egui::RichText::new(code).monospace().size(base * 0.88).color(p.text)).wrap());
                    });
                if copy {
                    ui.ctx().copy_text(code.clone());
                }
                None
            }
            Block::Quote(inner) => {
                let start = ui.cursor().min;
                let r = render_blocks(ui, inner, ctx, indent + 14.0, false);
                let end = ui.cursor().min.y - 2.0;
                ui.painter().rect_filled(Rect::from_min_max(pos2(start.x + indent, start.y), pos2(start.x + indent + 4.0, end)), CornerRadius::same(2), p.text_faint);
                r
            }
        };
        if a.is_some() {
            action = a;
        }
    }
    ui.spacing_mut().item_spacing.y = prev_spacing;
    action
}

/// Formats `<t:unix:style>` per Discord's styles. Returns (display, tooltip).
pub fn format_timestamp(unix: i64, fmt: char, h24: bool) -> (String, String) {
    use chrono::{Local, TimeZone};
    let Some(dt) = Local.timestamp_opt(unix, 0).single() else { return ("Invalid Date".into(), String::new()) };
    let time = if h24 { "%H:%M" } else { "%-I:%M %p" };
    let time_long = if h24 { "%H:%M:%S" } else { "%-I:%M:%S %p" };
    let full = dt.format(&format!("%A, %B %-d, %Y {time}")).to_string();
    let short = match fmt {
        't' => dt.format(time).to_string(),
        'T' => dt.format(time_long).to_string(),
        'd' => dt.format("%d/%m/%Y").to_string(),
        'D' => dt.format("%B %-d, %Y").to_string(),
        'F' => full.clone(),
        'R' => relative(unix),
        _ => dt.format(&format!("%B %-d, %Y {time}")).to_string(),
    };
    (short, full)
}

pub fn relative(unix: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let d = unix - now;
    let a = d.abs();
    let (n, unit) = match a {
        0..=59 => (a, "second"),
        60..=3599 => (a / 60, "minute"),
        3600..=86_399 => (a / 3600, "hour"),
        86_400..=2_591_999 => (a / 86_400, "day"),
        2_592_000..=31_535_999 => (a / 2_592_000, "month"),
        _ => (a / 31_536_000, "year"),
    };
    let plural = if n == 1 { "" } else { "s" };
    if d >= 0 { format!("in {n} {unit}{plural}") } else { format!("{n} {unit}{plural} ago") }
}

/// Paints a small text badge (used for "BOT", "(edited)" etc.).
pub fn badge(ui: &mut Ui, text: &str, fill: Color32, fg: Color32) {
    let font = FontId::new(10.0, theme::bold());
    let galley = ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, fg));
    let size = galley.size() + vec2(8.0, 2.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    let _ = Align2::CENTER_CENTER;
}
