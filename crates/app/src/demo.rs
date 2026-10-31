//! Offline demo mode (`ferrocord --demo`): fills the store with sample data so
//! the UI can be explored, screenshotted and developed without an account.

use ferrocord_core::Client;
use ferrocord_core::model::{Event, Snowflake};
use serde_json::{Value, json};

const ME: u64 = 1_000;
const GUILD: u64 = 900_000_000_000_000_000;
const GENERAL: u64 = GUILD + 10;
const RUST: u64 = GUILD + 11;
const ANNOUNCE: u64 = GUILD + 12;
const VOICE: u64 = GUILD + 13;
const DM: u64 = GUILD + 50;

fn sid(v: u64) -> String {
    v.to_string()
}

/// Message id minted `mins_ago` minutes in the past.
fn mid(mins_ago: u64, n: u64) -> u64 {
    let now = chrono::Utc::now().timestamp_millis() as u64;
    Snowflake::from_timestamp_ms(now - mins_ago * 60_000).0 + n
}

fn user(id: u64, name: &str, global: &str, bot: bool) -> Value {
    json!({ "id": sid(id), "username": name, "global_name": global, "discriminator": "0", "avatar": null, "bot": bot })
}

pub fn populate(client: &Client) {
    let users = vec![
        user(1001, "alice", "Alice", false),
        user(1002, "bob", "Bob", false),
        user(1003, "ferris", "Ferris", false),
        user(1004, "helper", "Helper Bot", true),
    ];
    let ready = json!({
        "v": 9,
        "session_id": "demo",
        "user": { "id": sid(ME), "username": "you", "global_name": "You", "discriminator": "0", "avatar": null, "mfa_enabled": true, "email": "you@example.com" },
        "users": users,
        "guilds": [{
            "id": sid(GUILD),
            "properties": { "name": "Ferrocord Lounge", "owner_id": sid(1001), "default_message_notifications": 1 },
            "joined_at": "2024-01-01T00:00:00Z",
            "member_count": 4,
            "roles": [
                { "id": sid(GUILD), "name": "@everyone", "permissions": "2248473465835073", "position": 0, "color": 0 },
                { "id": sid(GUILD + 1), "name": "Maintainers", "permissions": "8", "position": 2, "color": 0xe67e22, "hoist": true },
                { "id": sid(GUILD + 2), "name": "Rustaceans", "permissions": "0", "position": 1, "color": 0x3ba55d, "hoist": true }
            ],
            "channels": [
                { "id": sid(GUILD + 100), "type": 4, "name": "Text Channels", "position": 0 },
                { "id": sid(GENERAL), "type": 0, "name": "general", "position": 0, "parent_id": sid(GUILD + 100), "topic": "Say hi! Be nice. **Markdown** works here.", "last_message_id": sid(mid(1, 20)) },
                { "id": sid(RUST), "type": 0, "name": "rust-help", "position": 1, "parent_id": sid(GUILD + 100), "last_message_id": sid(mid(30, 0)) },
                { "id": sid(ANNOUNCE), "type": 5, "name": "announcements", "position": 2, "parent_id": sid(GUILD + 100), "last_message_id": sid(mid(600, 0)) },
                { "id": sid(GUILD + 101), "type": 4, "name": "Voice", "position": 1 },
                { "id": sid(VOICE), "type": 2, "name": "Hangout", "position": 0, "parent_id": sid(GUILD + 101) }
            ],
            "emojis": []
        }],
        "merged_members": [[{ "user_id": sid(ME), "roles": [sid(GUILD + 2)] }]],
        "private_channels": [{ "id": sid(DM), "type": 1, "recipient_ids": [sid(1003)], "last_message_id": sid(mid(5, 0)) }],
        "relationships": [
            { "id": sid(1001), "type": 1, "user_id": sid(1001) },
            { "id": sid(1002), "type": 1, "user_id": sid(1002) },
            { "id": sid(1003), "type": 3, "user_id": sid(1003) }
        ],
        "read_state": { "entries": [ { "id": sid(GENERAL), "last_message_id": sid(mid(3, 0)), "mention_count": 1 }, { "id": sid(RUST), "last_message_id": sid(mid(30, 0)) }, { "id": sid(ANNOUNCE), "last_message_id": sid(mid(600, 0)) } ], "partial": false, "version": 1 },
        "user_guild_settings": { "entries": [], "partial": false, "version": 1 }
    });
    apply(client, "READY", ready);

    let mut members = vec![];
    for (uid, roles, status, activity) in [
        (1001u64, vec![GUILD + 1], "online", Some("Writing a Discord client in Rust")),
        (1003, vec![GUILD + 2], "idle", None),
        (1004, vec![], "online", None),
        (ME, vec![GUILD + 2], "online", None),
        (1002, vec![], "offline", None),
    ] {
        let acts = match activity {
            Some(a) => json!([{ "name": "Custom Status", "type": 4, "state": a }]),
            None => json!([]),
        };
        members.push((uid, roles, status, acts));
    }
    let item = |uid: u64, roles: &Vec<u64>, status: &str, acts: &Value| json!({ "member": { "user": { "id": sid(uid) }, "roles": roles.iter().map(|r| sid(*r)).collect::<Vec<_>>(), "presence": { "status": status, "activities": acts, "user": { "id": sid(uid) } } } });
    let mut items = vec![json!({ "group": { "id": sid(GUILD + 1), "count": 1 } })];
    items.push(item(members[0].0, &members[0].1, members[0].2, &members[0].3));
    items.push(json!({ "group": { "id": "online", "count": 3 } }));
    for m in &members[1..4] {
        items.push(item(m.0, &m.1, m.2, &m.3));
    }
    items.push(json!({ "group": { "id": "offline", "count": 1 } }));
    items.push(item(members[4].0, &members[4].1, members[4].2, &members[4].3));
    apply(
        client,
        "GUILD_MEMBER_LIST_UPDATE",
        json!({ "guild_id": sid(GUILD), "id": "everyone", "member_count": 5, "online_count": 4,
                "groups": [{ "id": sid(GUILD + 1), "count": 1 }, { "id": "online", "count": 3 }, { "id": "offline", "count": 1 }],
                "ops": [{ "op": "SYNC", "range": [0, 99], "items": items }] }),
    );
    apply(client, "PRESENCE_UPDATE", json!({ "user": { "id": sid(1003) }, "status": "idle", "activities": [] }));
    for (uid, mute) in [(1001u64, false), (1003, true), (1002, false)] {
        apply(
            client,
            "VOICE_STATE_UPDATE",
            json!({ "guild_id": sid(GUILD), "channel_id": sid(VOICE), "user_id": sid(uid), "session_id": "demo", "self_mute": mute, "self_deaf": false, "deaf": false, "mute": false, "self_video": false, "suppress": false }),
        );
    }

    let msg = |id: u64, ch: u64, author: Value, content: &str, extra: Value| {
        let mut m = json!({ "id": sid(id), "channel_id": sid(ch), "author": author, "content": content, "timestamp": "", "type": 0, "mentions": [], "attachments": [], "embeds": [], "reactions": [] });
        if ch != DM {
            m["guild_id"] = json!(sid(GUILD));
        }
        if let (Some(o), Some(e)) = (m.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                o.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value::<ferrocord_core::model::Message>(m).expect("demo message")
    };
    let alice = user(1001, "alice", "Alice", false);
    let bob = user(1002, "bob", "Bob", false);
    let ferris = user(1003, "ferris", "Ferris", false);
    let bot = user(1004, "helper", "Helper Bot", true);
    let me = user(ME, "you", "You", false);
    let welcome_id = mid(1500, 0);
    let general = vec![
        msg(mid(1500, 9), GENERAL, ferris.clone(), "", json!({ "type": 7 })),
        msg(
            welcome_id,
            GENERAL,
            alice.clone(),
            "# Welcome to the lounge 👋\nThis server is a **demo** of _Ferrocord_ — a fast, open-source Discord client written in Rust.\n> Everything here is local sample data.\n> Try hovering messages, right-clicking, and the emoji picker.",
            json!({ "reactions": [{ "count": 3, "me": true, "emoji": { "id": null, "name": "🎉" } }, { "count": 1, "me": false, "emoji": { "id": null, "name": "🦀" } }] }),
        ),
        msg(
            mid(1499, 0),
            GENERAL,
            alice.clone(),
            "Markdown showcase: **bold**, *italic*, __underline__, ~~strike~~, `inline code`, ||spoiler (click me)||, and a link: https://www.rust-lang.org",
            json!({}),
        ),
        msg(mid(1498, 0), GENERAL, alice.clone(), "```rust\nfn main() {\n    println!(\"Hello from Ferrocord!\");\n}\n```", json!({})),
        msg(
            mid(61, 0),
            GENERAL,
            ferris.clone(),
            "Code blocks are highlighted for many languages:\n```java\n@Override\npublic String greet(String name) {\n    return \"Hello, \" + name + \"!\"; // 👋\n}\n```\n```py\ndef greet(name: str) -> str:\n    return f\"Hello, {name}!\"  # 👋\n```",
            json!({}),
        ),
        msg(mid(60, 0), GENERAL, bob.clone(), "Is it fast? 🤔", json!({})),
        msg(
            mid(59, 0),
            GENERAL,
            ferris.clone(),
            "Very. Immediate-mode UI, virtualized lists, zero-copy event parsing 🚀\n- window up in ~0.2 s\n- ~90 MB RAM in this demo\n- plugins in sandboxed Luau",
            json!({}),
        ),
        msg(
            mid(58, 0),
            GENERAL,
            bob.clone(),
            "nice!! 😄😄",
            json!({ "type": 19, "message_reference": { "message_id": sid(mid(59, 0)), "channel_id": sid(GENERAL) }, "referenced_message": serde_json::to_value(msg(mid(59, 0), GENERAL, ferris.clone(), "Very. Immediate-mode UI, virtualized lists, zero-copy event parsing 🚀", json!({}))).unwrap() }),
        ),
        msg(
            mid(10, 0),
            GENERAL,
            bot.clone(),
            "",
            json!({ "embeds": [{ "title": "Build passed ✅", "description": "All **42** tests passed on `main`.", "color": 0x3ba55d, "fields": [{ "name": "Duration", "value": "1m 12s", "inline": true }, { "name": "Commit", "value": "`a1b2c3d`", "inline": true }], "footer": { "text": "CI · just now" } }] }),
        ),
        msg(
            mid(1, 20),
            GENERAL,
            alice.clone(),
            &format!("<@{ME}> want to try the quick switcher? Press **Ctrl+K** 😉 — server time is <t:{}:R>", chrono::Utc::now().timestamp() - 3600),
            json!({ "mentions": [me.clone()] }),
        ),
    ];
    let rust = vec![
        msg(mid(31, 0), RUST, bob.clone(), "How do I share state between threads?", json!({})),
        msg(
            mid(30, 0),
            RUST,
            ferris.clone(),
            "Wrap it in `Arc<Mutex<T>>`, or use channels:\n```rust\nlet (tx, rx) = std::sync::mpsc::channel();\n```",
            json!({}),
        ),
        msg(
            mid(29, 0),
            RUST,
            bob.clone(),
            "Unrelated, but look at this sunset 🌅 (click to zoom)",
            json!({ "attachments": [{ "id": sid(9001), "filename": "sunset.png", "size": 482_113, "url": "demo://sunset.png", "proxy_url": "demo://sunset.png", "width": 1600, "height": 900, "content_type": "image/png" }] }),
        ),
    ];
    let announce = vec![msg(mid(600, 0), ANNOUNCE, alice.clone(), "📢 Ferrocord **0.1** is out! Grab it from the releases page.", json!({}))];
    let dm = vec![
        msg(mid(6, 0), DM, ferris.clone(), "hey! sent you a friend request 🦀", json!({})),
        msg(mid(5, 0), DM, me.clone(), "accepting in a sec!", json!({})),
    ];

    let mut s = client.store_mut();
    for (ch, list) in [(GENERAL, general), (RUST, rust), (ANNOUNCE, announce), (DM, dm)] {
        let n = list.len();
        s.insert_history(Snowflake(ch), list, n + 1, true);
    }
}

fn apply(client: &Client, name: &str, data: Value) {
    let (ev, err) = Event::parse(name, &data);
    if let Some(e) = err {
        tracing::error!(name, error = %e, "demo fixture failed to parse");
    }
    client.store_mut().apply(&ev);
}

/// Echoes a message locally instead of sending it.
pub fn local_send(client: &Client, channel: Snowflake, content: String) {
    let me = client.store().me.clone().unwrap_or_default();
    let id = Snowflake::nonce_now();
    let guild = client.store().channels.get(&channel).and_then(|c| c.guild_id);
    let data = json!({ "id": id, "channel_id": channel, "guild_id": guild, "author": me.user, "content": content, "timestamp": chrono::Utc::now().to_rfc3339(), "type": 0, "mentions": [] });
    apply(client, "MESSAGE_CREATE", data);
}
