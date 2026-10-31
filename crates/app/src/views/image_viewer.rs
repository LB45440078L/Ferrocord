//! Full-screen image viewer: fit to window, zoom (wheel, pinch, +/-, double-click),
//! drag to pan, and actions to open, copy or save the original.

use egui::{Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::app::App;
use crate::icons::{self, Icon};

const MIN_ZOOM_OF_FIT: f32 = 0.5;
const MAX_ZOOM: f32 = 8.0;

#[derive(Clone, Copy)]
struct ViewState {
    /// Screen points per image pixel. `None` until the image size is known.
    scale: Option<f32>,
    offset: Vec2,
}

fn state_id() -> Id {
    Id::unique("image_viewer_state")
}

fn result_id() -> Id {
    Id::unique("image_viewer_result")
}

/// Shows the viewer; returns false once it should close.
pub fn show(app: &mut App, ctx: &egui::Context, url: &str, open_url: &str) -> bool {
    drain_results(app, ctx);
    let screen = ctx.content_rect();
    let mut keep = true;
    let img = app.images.get(url);
    let mut st: ViewState = ctx.data(|d| d.get_temp(state_id())).unwrap_or(ViewState { scale: None, offset: Vec2::ZERO });

    egui::Area::new(Id::unique("image_viewer")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        ui.set_min_size(screen.size());
        let bg = ui.interact(screen, Id::unique("image_viewer_bg"), Sense::click_and_drag());
        ui.painter().rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(220));

        // Space for the image, leaving room for the toolbar.
        let stage = Rect::from_min_max(screen.min + vec2(24.0, 24.0), screen.max - vec2(24.0, 76.0));
        let Some(img) = img else {
            if app.images.is_failed(url) {
                ui.painter().text(stage.center(), egui::Align2::CENTER_CENTER, "Couldn't load this image", FontId::proportional(16.0), Color32::WHITE);
            } else {
                egui::Spinner::new().size(36.0).color(Color32::WHITE).paint_at(ui, Rect::from_center_size(stage.center(), Vec2::splat(36.0)));
            }
            if bg.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                keep = false;
            }
            toolbar(app, ui, screen, open_url, None, &mut st, stage, &mut keep);
            return;
        };

        let fit = (stage.width() / img.size.x).min(stage.height() / img.size.y).min(1.0);
        let mut scale = st.scale.unwrap_or(fit);
        let center = stage.center();

        // Zoom around the pointer: mouse wheel, pinch / ctrl+wheel, keyboard.
        let (wheel, pinch, keys) = ui.input(|i| {
            let k = if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                1.25
            } else if i.key_pressed(egui::Key::Minus) {
                0.8
            } else {
                1.0
            };
            (if i.modifiers.command { 0.0 } else { i.smooth_scroll_delta.y }, i.zoom_delta(), k)
        });
        let factor = (wheel * 0.0025).exp() * pinch * keys;
        if (factor - 1.0).abs() > 1e-4 {
            let anchor = ui.ctx().pointer_hover_pos().filter(|p| screen.contains(*p)).unwrap_or(center);
            let new_scale = (scale * factor).clamp(fit * MIN_ZOOM_OF_FIT, MAX_ZOOM.max(fit));
            let k = new_scale / scale;
            st.offset = (anchor - center) - (anchor - center - st.offset) * k;
            scale = new_scale;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Num0)) {
            scale = fit;
            st.offset = Vec2::ZERO;
        }

        let size = img.size * scale;
        let mut rect = Rect::from_center_size(center + st.offset, size);
        if bg.dragged() {
            st.offset += bg.drag_delta();
            rect = rect.translate(bg.drag_delta());
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if bg.hovered() && rect.contains(bg.hover_pos().unwrap_or(Pos2::ZERO)) && (size.x > stage.width() || size.y > stage.height()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        // Keep at least part of the image on screen.
        let slack = (size - stage.size()).max(Vec2::ZERO) / 2.0 + Vec2::splat(40.0);
        st.offset = st.offset.clamp(-slack, slack);

        let on_image = bg.interact_pointer_pos().is_some_and(|p| rect.contains(p));
        if bg.double_clicked() && on_image {
            // Toggle between "fit" and actual pixels (or 2x when the image already fits at 1:1).
            let target = if (scale - fit).abs() < 1e-3 { if fit < 1.0 { 1.0 } else { 2.0 } } else { fit };
            let p = bg.interact_pointer_pos().unwrap_or(center);
            st.offset = if target == fit { Vec2::ZERO } else { (p - center) - (p - center - st.offset) * (target / scale) };
            scale = target;
        } else if bg.clicked() && !on_image {
            keep = false;
        }
        let rect = Rect::from_center_size(center + st.offset, img.size * scale);
        ui.painter().with_clip_rect(screen).image(img.texture, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        st.scale = Some(scale);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            keep = false;
        }
        toolbar(app, ui, screen, open_url, Some((scale, fit, img.size)), &mut st, stage, &mut keep);
    });

    if keep {
        ctx.data_mut(|d| d.insert_temp(state_id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<ViewState>(state_id()));
    }
    keep
}

#[allow(clippy::too_many_arguments)]
fn toolbar(app: &mut App, ui: &mut egui::Ui, screen: Rect, open_url: &str, zoom: Option<(f32, f32, Vec2)>, st: &mut ViewState, stage: Rect, keep: &mut bool) {
    // Close button, top right.
    let close = Rect::from_center_size(pos2(screen.max.x - 32.0, screen.min.y + 32.0), Vec2::splat(36.0));
    let r = ui.interact(close, Id::unique("image_viewer_close"), Sense::click());
    ui.painter().circle_filled(close.center(), 18.0, Color32::from_white_alpha(if r.hovered() { 40 } else { 18 }));
    icons::paint_centered(ui, close.center(), 18.0, Icon::Close, Color32::WHITE);
    if r.on_hover_text("Close (Esc)").clicked() {
        *keep = false;
    }

    let bar_w = 520.0_f32.min(screen.width() - 32.0);
    let bar = Rect::from_center_size(pos2(screen.center().x, screen.max.y - 36.0), vec2(bar_w, 40.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut child;
    ui.painter().rect(
        bar,
        CornerRadius::same(20),
        Color32::from_rgba_unmultiplied(30, 31, 36, 235),
        Stroke::new(1.0, Color32::from_white_alpha(20)),
        egui::StrokeKind::Inside,
    );
    ui.add_space(12.0);
    ui.spacing_mut().item_spacing.x = 6.0;
    let white = Color32::WHITE;
    let muted = Color32::from_gray(190);
    let text_button = |ui: &mut egui::Ui, text: &str, tip: &str| {
        ui.add(egui::Button::new(egui::RichText::new(text).color(white).size(13.0)).frame(false).min_size(vec2(0.0, 28.0))).on_hover_text(tip).clicked()
    };
    if let Some((scale, fit, px)) = zoom {
        if text_button(ui, "−", "Zoom out (-)") {
            st.scale = Some((scale * 0.8).max(fit * MIN_ZOOM_OF_FIT));
        }
        ui.label(egui::RichText::new(format!("{:.0}%", scale * 100.0)).color(muted).size(12.5).monospace());
        if text_button(ui, "+", "Zoom in (+)") {
            st.scale = Some((scale * 1.25).min(MAX_ZOOM.max(fit)));
        }
        if text_button(ui, "Fit", "Fit to window (0)") {
            st.scale = Some(fit);
            st.offset = Vec2::ZERO;
        }
        if fit < 1.0 && text_button(ui, "1:1", "Actual size") {
            st.scale = Some(1.0);
            st.offset = Vec2::ZERO;
        }
        ui.label(egui::RichText::new(format!("{}×{}", px.x as u32, px.y as u32)).color(muted).size(11.5));
        let _ = stage;
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(12.0);
        if text_button(ui, "Save…", "Save the original image") {
            save(app, ui.ctx(), open_url);
        }
        if text_button(ui, "Copy link", "Copy the image link") {
            ui.ctx().copy_text(open_url.to_string());
            app.toasts.push(crate::app::Toast { text: "Link copied".into(), error: false, at: std::time::Instant::now() });
        }
        if text_button(ui, "Open in browser", "Open the original") {
            crate::views::open_link(app, open_url);
        }
    });
}

fn file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("image.png");
    name.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect()
}

/// Asks where to save, then downloads the original in the background.
fn save(app: &mut App, ctx: &egui::Context, url: &str) {
    let Some(path) = rfd::FileDialog::new().set_file_name(file_name(url)).save_file() else { return };
    let http = app.client.http().raw_client().clone();
    let url = url.to_string();
    let ctx = ctx.clone();
    app.client.runtime().spawn(async move {
        let result: Result<(), String> = async {
            let bytes = http.get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
            tokio::fs::write(&path, &bytes).await.map_err(|e| e.to_string())?;
            Ok(())
        }
        .await;
        let msg = match result {
            Ok(()) => (format!("Saved to {}", path.display()), false),
            Err(e) => (format!("Could not save the image: {e}"), true),
        };
        ctx.data_mut(|d| d.insert_temp(result_id(), Some(msg)));
        ctx.request_repaint();
    });
}

/// Turns finished background saves into toasts.
pub fn drain_results(app: &mut App, ctx: &egui::Context) {
    if let Some(Some((text, error))) = ctx.data_mut(|d| d.remove_temp::<Option<(String, bool)>>(result_id())) {
        app.toasts.push(crate::app::Toast { text, error, at: std::time::Instant::now() });
    }
}

#[cfg(test)]
mod tests {
    use super::file_name;

    #[test]
    fn file_names_come_from_the_url_path() {
        assert_eq!(file_name("https://cdn.discordapp.com/attachments/1/2/cat.png?ex=1&is=2"), "cat.png");
        assert_eq!(file_name("https://x/y/"), "image.png");
        assert_eq!(file_name("https://x/a:b.jpg"), "a_b.jpg");
    }
}
