//! Chat view behaviour.

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;

use super::demo_app;

#[test]
fn sending_a_message_keeps_history_visible() {
    let mut f = demo_app(None);
    let channel = f.harness.state().nav.channel.expect("demo opens a channel");
    let before = f.harness.state().scroll.get(&channel).map(|s| s.rendered_rows).unwrap_or(0);
    assert!(before > 0, "history should render before sending");
    let count_before = f.harness.state().client.store().messages.get(&channel).map(|c| c.messages.len()).unwrap_or(0);

    f.harness.get_by_role(Role::MultilineTextInput).focus();
    f.harness.run_steps(2);
    f.harness.get_by_role(Role::MultilineTextInput).type_text("hello from the test harness");
    f.harness.run_steps(2);
    f.harness.key_press(egui::Key::Enter);
    f.harness.run_steps(30);

    let state = f.harness.state();
    let count_after = state.client.store().messages.get(&channel).map(|c| c.messages.len()).unwrap_or(0);
    assert_eq!(count_after, count_before + 1, "message should be appended");
    let rendered = state.scroll.get(&channel).map(|s| s.rendered_rows).unwrap_or(0);
    assert!(rendered > 0, "history went blank after sending (rendered {rendered} rows)");
}

#[test]
fn switching_channels_after_sending_still_renders() {
    let mut f = demo_app(None);
    let channel = f.harness.state().nav.channel.expect("channel");
    f.harness.get_by_role(Role::MultilineTextInput).focus();
    f.harness.get_by_role(Role::MultilineTextInput).type_text("one");
    f.harness.key_press(egui::Key::Enter);
    f.harness.run_steps(10);
    let other = {
        let s = f.harness.state().client.store();
        s.channels.values().find(|c| c.guild_id.is_some() && c.id != channel && c.name.as_deref() == Some("rust-help")).map(|c| c.id).expect("rust-help")
    };
    f.harness.state_mut().select_channel(other);
    f.harness.run_steps(6);
    f.harness.state_mut().select_channel(channel);
    f.harness.run_steps(10);
    let rendered = f.harness.state().scroll.get(&channel).map(|s| s.rendered_rows).unwrap_or(0);
    assert!(rendered > 0, "history should render after returning to the channel");
}

#[test]
fn corrupted_scroll_state_is_repaired() {
    use ferrocord_core::model::Snowflake;
    let channel = Snowflake(42);
    let mut checked = false;
    let mut harness = egui_kittest::Harness::new_ui(|ui| {
        let id = ui.make_persistent_id(("chat", channel));
        // Poison the state the way the old `f32::MAX` jump did (fields are private: go through serde).
        let mut v = serde_json::to_value(egui::scroll_area::State::default()).expect("serialize");
        v["offset"] = serde_json::json!([0.0, 3.0e38]);
        let bad: egui::scroll_area::State = serde_json::from_value(v).expect("deserialize");
        bad.store(ui.ctx(), id);
        crate::views::chat::repair_scroll_state(ui, channel);
        let fixed = egui::scroll_area::State::load(ui.ctx(), id).expect("state");
        assert!(fixed.unclamped_offset().y < 1.0, "offset should be reset, got {:?}", fixed.unclamped_offset());
        checked = true;
    });
    harness.run_steps(1);
    drop(harness);
    assert!(checked);
}
