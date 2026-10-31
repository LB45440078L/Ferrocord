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

/// Reads and clears a probe, to tell whether it was recorded in the last frame.
#[cfg(test)]
pub fn take(ctx: &Context, name: &'static str) -> Option<Rect> {
    let id = Id::unique(("layout_probe", name));
    ctx.data_mut(|d| {
        let r = d.get_temp::<Rect>(id);
        d.remove::<Rect>(id);
        r
    })
}
