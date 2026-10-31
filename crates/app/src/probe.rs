//! Layout probes: named rects recorded during a frame so tests can assert
//! alignment (centering, baselines) on the real UI. Cost: one small map insert.

use egui::{Context, Id, Rect};

pub fn record(ctx: &Context, name: &'static str, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(Id::unique(("layout_probe", name)), rect));
}

#[cfg(test)]
pub fn get(ctx: &Context, name: &'static str) -> Option<Rect> {
    ctx.data(|d| d.get_temp(Id::unique(("layout_probe", name))))
}
