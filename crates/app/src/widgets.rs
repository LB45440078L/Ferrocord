//! Small reusable widgets.

use egui::{Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};
use ferrocord_core::model::Status;

use crate::images::ImageCache;
use crate::theme::{self, Palette};

/// Circular avatar with an optional presence dot. `ring` is the background
/// color the dot is cut out of.
pub fn avatar(ui: &mut Ui, images: &ImageCache, url: &str, size: f32, status: Option<(Status, Color32)>, pal: &Palette) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    paint_avatar(ui, images, url, rect, status, pal);
    resp
}

pub fn paint_avatar(ui: &Ui, images: &ImageCache, url: &str, rect: Rect, status: Option<(Status, Color32)>, pal: &Palette) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let r = rect.width() / 2.0;
    match images.get(url) {
        Some(img) => {
            egui::Image::from_texture(egui::load::SizedTexture::new(img.texture, rect.size()))
                .corner_radius(CornerRadius::same(r.min(255.0) as u8))
                .paint_at(ui, rect);
        }
        None => {
            ui.painter().circle_filled(rect.center(), r, pal.bg_selected);
        }
    }
    if let Some((s, ring)) = status {
        let dot = (rect.width() * 0.3).max(8.0);
        let c = pos2(rect.max.x - dot / 2.0 + 1.0, rect.max.y - dot / 2.0 + 1.0);
        ui.painter().circle_filled(c, dot / 2.0 + 2.5, ring);
        paint_status(ui, c, dot / 2.0, s, pal, ring);
    }
}

/// Status glyph like the official client: filled (online), moon (idle), bar (dnd), ring (offline).
pub fn paint_status(ui: &Ui, c: egui::Pos2, r: f32, s: Status, pal: &Palette, bg: Color32) {
    let color = pal.status(s);
    let p = ui.painter();
    match s {
        Status::Online => {
            p.circle_filled(c, r, color);
        }
        Status::Idle => {
            p.circle_filled(c, r, color);
            p.circle_filled(c - vec2(r * 0.45, r * 0.45), r * 0.6, bg);
        }
        Status::Dnd => {
            p.circle_filled(c, r, color);
            p.rect_filled(Rect::from_center_size(c, vec2(r * 1.1, r * 0.4)), CornerRadius::same(2), bg);
        }
        Status::Invisible | Status::Offline => {
            p.circle_filled(c, r, color);
            p.circle_filled(c, r * 0.45, bg);
        }
    }
}

/// Text-glyph icon button with hover highlight.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str, pal: &Palette, active: bool) -> Response {
    let size = Vec2::splat(28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.hovered() || active {
        ui.painter().rect_filled(rect, CornerRadius::same(6), pal.bg_selected);
    }
    let color = if active || resp.hovered() { pal.text } else { pal.text_muted };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(16.0), color);
    resp.on_hover_text(tooltip)
}

/// A full-width selectable row used in channel/member lists.
pub fn list_row(ui: &mut Ui, height: f32, selected: bool, pal: &Palette) -> (Rect, Response) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let fill = if selected {
        Some(pal.bg_selected)
    } else if resp.hovered() {
        Some(pal.bg_hover)
    } else {
        None
    };
    if let Some(f) = fill {
        ui.painter().rect_filled(rect, CornerRadius::same(6), f);
    }
    (rect, resp)
}

/// Rounded "pill" counter (mention badges).
pub fn count_badge(ui: &Ui, right_center: egui::Pos2, n: u32, pal: &Palette) -> Rect {
    let text = if n > 99 { "99+".to_string() } else { n.to_string() };
    let galley = ui.ctx().fonts_mut(|f| f.layout_no_wrap(text, FontId::new(11.0, theme::bold()), Color32::WHITE));
    let w = (galley.size().x + 9.0).max(16.0);
    let rect = Rect::from_center_size(right_center - vec2(w / 2.0, 0.0), vec2(w, 16.0));
    ui.painter().rect_filled(rect, CornerRadius::same(8), pal.danger);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    rect
}

/// Primary/secondary styled button.
pub fn button(ui: &mut Ui, text: &str, primary: bool, pal: &Palette) -> Response {
    let fill = if primary { pal.accent } else { pal.bg_selected };
    let fg = if primary { Color32::WHITE } else { pal.text };
    ui.add(egui::Button::new(egui::RichText::new(text).color(fg).strong()).fill(fill).stroke(Stroke::NONE).corner_radius(CornerRadius::same(6)))
}

pub fn section_label(ui: &mut Ui, text: &str, pal: &Palette) {
    ui.label(egui::RichText::new(text.to_uppercase()).font(FontId::new(11.5, theme::bold())).color(pal.text_faint));
}

/// Horizontal rule with a centered label (e.g. date separators).
pub fn divider_label(ui: &mut Ui, text: &str, color: Color32, line: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let galley = ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_string(), FontId::new(11.5, theme::bold()), color));
    let c = rect.center();
    let half = galley.size().x / 2.0 + 8.0;
    ui.painter().hline(rect.min.x + 8.0..=c.x - half, c.y, Stroke::new(1.0, line));
    ui.painter().hline(c.x + half..=rect.max.x - 8.0, c.y, Stroke::new(1.0, line));
    ui.painter().galley(c - galley.size() / 2.0, galley, color);
}
