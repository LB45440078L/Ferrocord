//! The testkit fixtures parse into the model types without errors, i.e. the
//! fixtures look like real user-account payloads and the model accepts them.

use std::collections::HashSet;

use ferrocord_model::gateway::Ready;
use ferrocord_model::settings::PreloadedUserSettings;
use ferrocord_model::{ChannelType, Event, Guild, Message, Permissions, ReadStates, Snowflake};
use ferrocord_testkit::fixtures::{self, ChannelBuilder, GuildBuilder, MessageBuilder, ReadyBuilder, UserBuilder, ids};
use serde_json::json;

fn parse(name: &str, v: &serde_json::Value) -> Event {
    let (ev, err) = Event::parse(name, v);
    assert!(err.is_none(), "{name} failed to parse: {err:?}");
    ev
}

#[test]
fn standard_ready_parses() {
    let b = ReadyBuilder::standard();
    let Event::Ready(r) = parse("READY", &b.build()) else { panic!() };
    let r: Ready = *r;
    assert_eq!(r.v, 9);
    assert_eq!(r.user.user.id, Snowflake(ids::ME));
    assert_eq!(r.user.email.as_deref(), Some(fixtures::DEFAULT_EMAIL));
    assert!(r.user.verified);

    let g = &r.guilds[0];
    assert_eq!(g.name(), "Mock Guild");
    assert_eq!(g.props.owner_id, Snowflake(ids::OTHER));
    assert_eq!(g.props.default_message_notifications, 1);
    assert!(g.channels.iter().all(|c| c.guild_id == Some(g.id)), "guild ids are filled in");
    let general = g.channels.iter().find(|c| c.id == Snowflake(ids::GENERAL)).unwrap();
    assert_eq!(general.kind, ChannelType::GuildText);
    assert_eq!(general.parent_id, Some(Snowflake(ids::CATEGORY)));
    assert_eq!(general.topic.as_deref(), Some("General chat"));
    assert!(g.everyone_role().unwrap().permissions.contains(Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::ADD_REACTIONS));
    assert_eq!(g.roles.len(), 2);

    assert_eq!(r.merged_members.len(), 1);
    assert_eq!(r.merged_members[0][0].user_id(), Some(Snowflake(ids::ME)));
    assert_eq!(r.users.len(), 1);
    assert_eq!(r.relationships[0].user_id, Some(Snowflake(ids::FRIEND)));
    let dm = &r.private_channels[0];
    assert_eq!(dm.kind, ChannelType::Dm);
    assert_eq!(dm.recipient_ids(), vec![Snowflake(ids::FRIEND)]);
    assert!(matches!(r.read_state, ReadStates::Versioned { version: 1, .. }));
    assert_eq!(r.read_state.into_entries().len(), 2);

    let s = PreloadedUserSettings::from_base64(r.user_settings_proto.as_deref().unwrap()).unwrap();
    assert_eq!(s.status(), Some("online"));
    let folder = &s.guild_folders.unwrap().folders[0];
    assert_eq!(folder.guild_ids, vec![ids::GUILD]);
    assert_eq!(folder.name.as_ref().unwrap().value, "Mock Folder");

    let Event::ReadySupplemental(sup) = parse("READY_SUPPLEMENTAL", &b.build_supplemental()) else { panic!() };
    assert_eq!(sup.guilds[0].id, Snowflake(ids::GUILD));
    assert_eq!(sup.merged_presences.friends[0].user_id(), Some(Snowflake(ids::FRIEND)));
}

#[test]
fn guild_flat_and_nested_forms_agree() {
    let b = GuildBuilder::new(5, "Flat Guild").icon("abc").emoji(9, "blob", true).channel(ChannelBuilder::voice(6, "vc"));
    let nested: Guild = serde_json::from_value(b.build()).unwrap();
    let flat: Guild = serde_json::from_value(b.build_flat()).unwrap();
    assert_eq!(nested.name(), flat.name());
    assert_eq!(nested.props.icon, flat.props.icon);
    assert!(nested.icon_url(64).unwrap().contains("/icons/5/abc"));
    assert!(nested.emojis[0].animated);
    assert_eq!(nested.channels[0].kind, ChannelType::GuildVoice);
    assert_eq!(nested.channels[0].bitrate, Some(64000));
}

#[test]
fn messages_and_events_parse() {
    let friend = UserBuilder::friend();
    let me = UserBuilder::me();
    let id = Snowflake::from_timestamp_ms(1_760_000_000_000).0;
    let m = MessageBuilder::new(id, ids::GENERAL, &friend)
        .guild(ids::GUILD)
        .content("hi <@me>")
        .mention(&me)
        .reaction(json!({ "id": null, "name": "🔥" }), 2, true)
        .reply_to(id - 1)
        .nonce("123")
        .edited();
    let Event::MessageCreate(msg) = parse("MESSAGE_CREATE", &m.build_event()) else { panic!() };
    assert_eq!(msg.guild_id, Some(Snowflake(ids::GUILD)));
    assert!(msg.member.is_some());
    assert!(msg.mentions_user(Snowflake(ids::ME)));
    assert_eq!(msg.reactions[0].count, 2);
    assert_eq!(msg.message_reference.as_ref().unwrap().message_id, Some(Snowflake(id - 1)));
    assert_eq!(msg.kind, 19);
    assert!(msg.is_regular() && msg.is_edited());
    assert_eq!(msg.created_at().timestamp_millis(), 1_760_000_000_000);
    let parsed_ts = chrono::DateTime::parse_from_rfc3339(&msg.timestamp).unwrap();
    assert_eq!(parsed_ts.timestamp_millis(), 1_760_000_000_000, "timestamp matches the id");

    let rest: Message = serde_json::from_value(m.build()).unwrap();
    assert_eq!(rest.guild_id, None, "REST messages carry no guild_id");

    let custom = parse(
        "MESSAGE_REACTION_ADD",
        &json!({ "user_id": "1", "channel_id": "2", "message_id": "3", "emoji": { "id": "77", "name": "blob" }, "burst": false, "type": 0 }),
    );
    let Event::ReactionAdd(r) = custom else { panic!() };
    assert_eq!(r.emoji.route_key(), "blob:77");

    for (name, v) in [
        ("MESSAGE_DELETE", json!({ "id": "1", "channel_id": "2" })),
        ("MESSAGE_ACK", json!({ "channel_id": "2", "message_id": "1", "version": 3, "last_viewed": 4000, "flags": null })),
        ("TYPING_START", json!({ "channel_id": "2", "user_id": "1", "timestamp": 1 })),
        ("USER_SETTINGS_PROTO_UPDATE", json!({ "settings": { "type": 1, "proto": fixtures::settings("idle", &[]).to_base64() }, "partial": true })),
        ("CHANNEL_CREATE", ChannelBuilder::group_dm(9, "group", &[1, 2]).build()),
    ] {
        let ev = parse(name, &v);
        assert_eq!(ev.name(), name);
    }
}

#[test]
fn bot_and_legacy_users() {
    let u: ferrocord_model::User = serde_json::from_value(UserBuilder::new(1, "old").discriminator("0042").bot().build()).unwrap();
    assert!(u.bot);
    assert_eq!(u.tag(), "old#0042");
    assert_eq!(u.display_name(), "old");
}

#[test]
fn message_nonces_are_unique_even_within_one_millisecond() {
    let nonces: Vec<Snowflake> = (0..2000).map(|_| Snowflake::nonce_now()).collect();
    let unique: HashSet<_> = nonces.iter().copied().collect();
    assert_eq!(unique.len(), nonces.len(), "nonces collide, so enforce_nonce makes Discord drop messages");
    assert!(nonces.windows(2).all(|w| w[0] < w[1]), "nonces are monotonic");
    let now = chrono::Utc::now().timestamp_millis() as u64;
    let ts = nonces.last().unwrap().timestamp_ms();
    assert!(ts <= now + 1000 && ts + 60_000 >= now, "nonce still encodes the current time");
}
