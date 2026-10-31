//! `config.toml` compatibility for the privacy & safety settings.

use ferrocord_core::model::Snowflake;
use ferrocord_core::{Config, Paths};

#[test]
fn old_config_without_safety_fields_loads_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    paths.ensure();
    // A config written before the safety settings existed.
    std::fs::write(paths.config_file(), "last_account = \"42\"\n\n[privacy]\nsend_typing = false\nauto_ack = true\n").unwrap();

    let c = Config::load(&paths);
    assert_eq!(c.last_account.as_deref(), Some("42"));
    assert!(!c.privacy.send_typing, "existing values are kept");
    assert!(c.privacy.auto_ack);
    assert!(!c.privacy.hide_blocked_messages);
    assert!(c.privacy.blur_media_from_strangers);
    assert!(c.privacy.confirm_external_links);
    assert!(c.privacy.ignored_users.is_empty());
}

#[test]
fn ignored_users_round_trip_as_strings() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    paths.ensure();
    let mut c = Config::default();
    c.privacy.ignored_users = vec![Snowflake(175928847299117063), Snowflake(42)];
    c.privacy.hide_blocked_messages = true;
    c.privacy.confirm_external_links = false;
    c.save(&paths);

    let text = std::fs::read_to_string(paths.config_file()).unwrap();
    assert!(text.contains("\"175928847299117063\""), "ids are stored as strings:\n{text}");

    let back = Config::load(&paths);
    assert_eq!(back.privacy.ignored_users, vec![Snowflake(175928847299117063), Snowflake(42)]);
    assert!(back.privacy.hide_blocked_messages);
    assert!(!back.privacy.confirm_external_links);
    assert!(back.privacy.blur_media_from_strangers);
}
