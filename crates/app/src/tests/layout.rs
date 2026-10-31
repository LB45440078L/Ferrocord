//! Alignment invariants measured on the live UI via layout probes.

use super::demo_app;
use crate::probe;

fn center_y(r: egui::Rect) -> f32 {
    r.center().y
}

#[test]
fn composer_contents_are_vertically_centered() {
    let f = demo_app(None);
    let ctx = f.harness.ctx.clone();
    let bar = probe::get(&ctx, "composer.bar").expect("composer bar probe");
    let input = probe::get(&ctx, "composer.input").expect("composer input probe");
    let emoji = probe::get(&ctx, "composer.emoji").expect("emoji probe");
    let attach = probe::get(&ctx, "composer.attach").expect("attach probe");
    println!("bar {bar:?}\ninput {input:?}\nemoji {emoji:?}\nattach {attach:?}");
    for (name, r) in [("input", input), ("emoji", emoji), ("attach", attach)] {
        assert!((center_y(r) - center_y(bar)).abs() <= 1.0, "{name} not centered in composer bar: {} vs {}", center_y(r), center_y(bar));
    }
}

#[test]
fn composer_input_has_room_for_one_line_plus_padding() {
    let f = demo_app(None);
    let ctx = f.harness.ctx.clone();
    let input = probe::get(&ctx, "composer.input").expect("composer input probe");
    let row = ctx.fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(15.0)));
    // A clipped text box puts the placeholder off-center; it must fit a row plus 2×9px padding.
    assert!(input.height() >= row + 18.0 - 0.5, "composer input too short: {} < {}", input.height(), row + 18.0);
}

#[test]
fn account_panel_items_share_one_center_line() {
    let f = demo_app(None);
    let ctx = f.harness.ctx.clone();
    let avatar = probe::get(&ctx, "account.avatar").expect("avatar probe");
    let text = probe::get(&ctx, "account.text").expect("text probe");
    let settings = probe::get(&ctx, "account.settings").expect("settings probe");
    for (name, r) in [("text", text), ("settings", settings)] {
        assert!((center_y(r) - center_y(avatar)).abs() <= 1.0, "{name} off the avatar's center line: {} vs {}", center_y(r), center_y(avatar));
    }
}

#[test]
fn message_body_follows_author_line_tightly() {
    let f = demo_app(None);
    let ctx = f.harness.ctx.clone();
    let header = probe::get(&ctx, "message.header").expect("header probe");
    let body_top = probe::get(&ctx, "message.body_top").expect("body probe").min.y;
    let gap = body_top - header.max.y;
    assert!(header.height() <= 24.0, "author line too tall: {}", header.height());
    assert!((0.0..=4.0).contains(&gap), "gap between author line and content: {gap}");
}
