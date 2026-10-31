//! Small reusable widgets.

use egui::{Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};
use ferrocord_core::model::Status;

use crate::icons::{self, Icon};
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

/// Square icon button with hover and pressed feedback.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str, pal: &Palette, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    let pressed = resp.is_pointer_button_down_on();
    let fill = if pressed {
        Some(pal.bg_selected.lerp_to_gamma(pal.accent, 0.25))
    } else if resp.hovered() || active {
        Some(pal.bg_selected)
    } else {
        None
    };
    if let Some(f) = fill {
        ui.painter().rect_filled(rect, CornerRadius::same(6), f);
    }
    let color = if active || resp.hovered() { pal.text } else { pal.text_muted };
    // Pressed icons shrink slightly, like a physical button.
    icons::paint_centered(ui, rect.center(), if pressed { 16.5 } else { 18.0 }, icon, color);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tooltip)
}

/// Context-menu entry with a leading icon. Returns true when clicked.
pub fn menu_item(ui: &mut Ui, icon: Icon, label: &str, color: Color32) -> bool {
    let width = ui.available_width().max(180.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    if resp.hovered() {
        let fill = ui.visuals().widgets.hovered.bg_fill;
        let fill = if resp.is_pointer_button_down_on() { ui.visuals().widgets.active.bg_fill } else { fill };
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    icons::paint_centered(ui, pos2(rect.min.x + 16.0, rect.center().y), 16.0, icon, color);
    ui.painter().text(pos2(rect.min.x + 34.0, rect.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), color);
    resp.clicked()
}

/// A full-width selectable row used in channel/member lists.
pub fn list_row(ui: &mut Ui, height: f32, selected: bool, pal: &Palette) -> (Rect, Response) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        Some(pal.bg_selected.lerp_to_gamma(pal.bg_hover, 0.5))
    } else if selected {
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

/// Marks a click-sensing widget that should *not* show the pointing-hand cursor
/// (e.g. a whole chat message, which only senses clicks for its context menu).
pub fn mark_passive(ctx: &egui::Context, id: egui::Id) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| {
        let set = d.get_temp_mut_or_default::<PassiveWidgets>(egui::Id::NULL);
        if set.frame != frame {
            set.frame = frame;
            set.ids.clear();
        }
        set.ids.insert(id);
    });
}

#[derive(Clone, Default)]
struct PassiveWidgets {
    frame: u64,
    ids: std::collections::HashSet<egui::Id>,
}

/// Shows the pointing-hand cursor over anything clickable that did not choose a
/// cursor itself, so every button, row and link gives the same feedback.
/// Call once per frame after all UI has been built.
pub fn pointer_feedback(ctx: &egui::Context) {
    if ctx.output(|o| o.cursor_icon) != egui::CursorIcon::Default || ctx.dragged_id().is_some() {
        return;
    }
    let hovered = ctx.interaction_snapshot(|s| s.hovered.clone());
    if hovered.is_empty() {
        return;
    }
    let passive =
        ctx.data(|d| d.get_temp::<PassiveWidgets>(egui::Id::NULL)).filter(|p| p.frame == ctx.cumulative_frame_nr()).map(|p| p.ids).unwrap_or_default();
    let clickable = hovered
        .iter()
        .any(|id| !passive.contains(id) && ctx.read_response(*id).is_some_and(|r| r.sense.senses_click() && !r.sense.senses_drag() && r.enabled()));
    if clickable {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}
