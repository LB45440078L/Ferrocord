//! Builders for gateway/REST payloads in the shape Discord sends to *user*
//! accounts (capabilities `1734653`): READY guilds nest metadata in
//! `properties`, users are deduplicated into `users[]`, members arrive in
//! `merged_members`, read states and guild settings are versioned and settings
//! come as a base64 protobuf.
//!
//! Every builder produces a `serde_json::Value`, so tests can tweak the output
//! freely before sending it.

use ferrocord_model::settings::{GuildFolder, GuildFolders, Int64Value, PreloadedUserSettings, StatusSettings, StringValue, UInt64Value};
use serde_json::{Map, Value, json};

use crate::world::timestamp_of;

pub const DEFAULT_EMAIL: &str = "user@example.com";
pub const DEFAULT_PASSWORD: &str = "correct horse battery staple";
pub const DEFAULT_TOKEN: &str = "mock-token.MTEwMDAwMDAwMDAwMDAwMDAx.secret";

/// Ids used by [`ReadyBuilder::standard`].
pub mod ids {
    /// The logged-in account.
    pub const ME: u64 = 1_100_000_000_000_000_001;
    pub const FRIEND: u64 = 1_100_000_000_000_000_002;
    pub const OTHER: u64 = 1_100_000_000_000_000_003;
    pub const GUILD: u64 = 1_200_000_000_000_000_001;
    pub const CATEGORY: u64 = 1_200_000_000_000_000_010;
    pub const GENERAL: u64 = 1_200_000_000_000_000_011;
    pub const RANDOM: u64 = 1_200_000_000_000_000_012;
    pub const VOICE: u64 = 1_200_000_000_000_000_013;
    pub const DM: u64 = 1_200_000_000_000_000_020;
    pub const ROLE_MOD: u64 = 1_200_000_000_000_000_030;
    pub const FOLDER: i64 = 4242;
}

/// Default `@everyone` permissions (view, send, read history, react, embed, attach…).
pub const EVERYONE_PERMISSIONS: &str = "104324673";

// ---------------------------------------------------------------- users

#[derive(Debug, Clone)]
pub struct UserBuilder {
    pub id: u64,
    pub username: String,
    pub global_name: Option<String>,
    /// `"0"` for migrated (pomelo) usernames.
    pub discriminator: String,
    pub avatar: Option<String>,
    pub bot: bool,
}

impl UserBuilder {
    pub fn new(id: u64, username: &str) -> Self {
        Self { id, username: username.into(), global_name: None, discriminator: "0".into(), avatar: None, bot: false }
    }

    /// The mock's default account.
    pub fn me() -> Self {
        Self::new(ids::ME, "mockuser").global_name("Mock User").avatar("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6")
    }

    pub fn friend() -> Self {
        Self::new(ids::FRIEND, "friend").global_name("Friendly")
    }

    pub fn global_name(mut self, n: &str) -> Self {
        self.global_name = Some(n.into());
        self
    }

    pub fn discriminator(mut self, d: &str) -> Self {
        self.discriminator = d.into();
        self
    }

    pub fn avatar(mut self, hash: &str) -> Self {
        self.avatar = Some(hash.into());
        self
    }

    pub fn bot(mut self) -> Self {
        self.bot = true;
        self
    }

    /// Public user object.
    pub fn build(&self) -> Value {
        let mut v = json!({
            "id": self.id.to_string(), "username": self.username, "global_name": self.global_name,
            "discriminator": self.discriminator, "avatar": self.avatar, "avatar_decoration_data": null,
            "banner": null, "accent_color": null, "public_flags": 0, "flags": 0, "primary_guild": null,
            "collectibles": null, "display_name_styles": null, "clan": null,
        });
        if self.bot {
            v["bot"] = json!(true);
        }
        v
    }

    /// `/users/@me` / READY `user` object.
    pub fn build_current(&self) -> Value {
        let mut v = self.build();
        let extra = json!({
            "email": DEFAULT_EMAIL, "verified": true, "mfa_enabled": false, "phone": null, "locale": "en-US",
            "nsfw_allowed": true, "premium_type": 0, "purchased_flags": 0, "bio": "", "pronouns": "",
            "premium": false, "mobile": false, "desktop": true,
        });
        if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
            o.extend(e);
        }
        v
    }
}

// ---------------------------------------------------------------- channels

#[derive(Debug, Clone)]
pub struct ChannelBuilder {
    pub id: u64,
    /// Discord channel type (0 text, 1 DM, 2 voice, 3 group DM, 4 category, 11 public thread…).
    pub kind: u8,
    pub name: Option<String>,
    pub position: i32,
    pub parent_id: Option<u64>,
    pub topic: Option<String>,
    pub last_message_id: Option<u64>,
    pub recipient_ids: Vec<u64>,
    pub nsfw: bool,
    pub overwrites: Vec<Value>,
    pub extra: Map<String, Value>,
}

impl ChannelBuilder {
    pub fn new(id: u64, kind: u8) -> Self {
        Self {
            id,
            kind,
            name: None,
            position: 0,
            parent_id: None,
            topic: None,
            last_message_id: None,
            recipient_ids: vec![],
            nsfw: false,
            overwrites: vec![],
            extra: Map::new(),
        }
    }

    pub fn text(id: u64, name: &str) -> Self {
        Self::new(id, 0).name(name)
    }

    pub fn voice(id: u64, name: &str) -> Self {
        Self::new(id, 2).name(name)
    }

    pub fn category(id: u64, name: &str) -> Self {
        Self::new(id, 4).name(name)
    }

    pub fn dm(id: u64, recipient: u64) -> Self {
        let mut c = Self::new(id, 1);
        c.recipient_ids = vec![recipient];
        c
    }

    pub fn group_dm(id: u64, name: &str, recipients: &[u64]) -> Self {
        let mut c = Self::new(id, 3).name(name);
        c.recipient_ids = recipients.to_vec();
        c
    }

    pub fn name(mut self, n: &str) -> Self {
        self.name = Some(n.into());
        self
    }

    pub fn position(mut self, p: i32) -> Self {
        self.position = p;
        self
    }

    pub fn parent(mut self, id: u64) -> Self {
        self.parent_id = Some(id);
        self
    }

    pub fn topic(mut self, t: &str) -> Self {
        self.topic = Some(t.into());
        self
    }

    pub fn last_message(mut self, id: u64) -> Self {
        self.last_message_id = Some(id);
        self
    }

    pub fn nsfw(mut self) -> Self {
        self.nsfw = true;
        self
    }

    /// Permission overwrite (`kind` 0 = role, 1 = member).
    pub fn overwrite(mut self, id: u64, kind: u8, allow: u64, deny: u64) -> Self {
        self.overwrites.push(json!({ "id": id.to_string(), "type": kind, "allow": allow.to_string(), "deny": deny.to_string() }));
        self
    }

    pub fn field(mut self, k: &str, v: Value) -> Self {
        self.extra.insert(k.into(), v);
        self
    }

    pub fn is_private(&self) -> bool {
        matches!(self.kind, 1 | 3)
    }

    /// The channel as it appears inside a READY guild or `private_channels`.
    pub fn build(&self) -> Value {
        let mut v = json!({
            "id": self.id.to_string(), "type": self.kind, "flags": 0,
            "last_message_id": self.last_message_id.map(|i| i.to_string()),
        });
        if self.is_private() {
            v["recipient_ids"] = json!(self.recipient_ids.iter().map(u64::to_string).collect::<Vec<_>>());
            v["is_spam"] = json!(false);
            v["is_message_request"] = json!(false);
            v["is_message_request_timestamp"] = Value::Null;
            v["safety_warnings"] = json!([]);
            if self.kind == 3 {
                v["name"] = json!(self.name);
                v["owner_id"] = json!(self.recipient_ids.first().map(u64::to_string));
                v["icon"] = Value::Null;
            }
        } else {
            v["name"] = json!(self.name);
            v["position"] = json!(self.position);
            v["parent_id"] = json!(self.parent_id.map(|i| i.to_string()));
            v["permission_overwrites"] = json!(self.overwrites);
            v["nsfw"] = json!(self.nsfw);
            match self.kind {
                2 | 13 => {
                    v["bitrate"] = json!(64000);
                    v["user_limit"] = json!(0);
                    v["rtc_region"] = Value::Null;
                }
                4 => {}
                _ => {
                    v["topic"] = json!(self.topic);
                    v["rate_limit_per_user"] = json!(0);
                    v["last_pin_timestamp"] = Value::Null;
                }
            }
        }
        if let Some(o) = v.as_object_mut() {
            o.extend(self.extra.clone());
        }
        v
    }

    /// The channel with `guild_id`, as in `CHANNEL_CREATE` or REST.
    pub fn build_in_guild(&self, guild: u64) -> Value {
        let mut v = self.build();
        v["guild_id"] = json!(guild.to_string());
        v
    }
}

// ---------------------------------------------------------------- guilds

#[derive(Debug, Clone)]
pub struct GuildBuilder {
    pub id: u64,
    pub name: String,
    pub owner_id: u64,
    pub icon: Option<String>,
    pub channels: Vec<ChannelBuilder>,
    pub roles: Vec<Value>,
    pub emojis: Vec<Value>,
    /// The current user's member roles (READY `merged_members`).
    pub my_roles: Vec<u64>,
    pub my_nick: Option<String>,
    pub member_count: u64,
    pub joined_at: String,
    /// 0 = all messages, 1 = only mentions.
    pub default_message_notifications: u8,
}

impl GuildBuilder {
    pub fn new(id: u64, name: &str) -> Self {
        Self {
            id,
            name: name.into(),
            owner_id: ids::OTHER,
            icon: None,
            channels: vec![],
            roles: vec![json!({
                "id": id.to_string(), "name": "@everyone", "permissions": EVERYONE_PERMISSIONS, "position": 0,
                "color": 0, "colors": { "primary_color": 0, "secondary_color": null, "tertiary_color": null },
                "hoist": false, "managed": false, "mentionable": false, "icon": null, "unicode_emoji": null, "flags": 0,
            })],
            emojis: vec![],
            my_roles: vec![],
            my_nick: None,
            member_count: 3,
            joined_at: "2024-01-01T00:00:00.000000+00:00".into(),
            default_message_notifications: 1,
        }
    }

    pub fn owner(mut self, id: u64) -> Self {
        self.owner_id = id;
        self
    }

    pub fn icon(mut self, hash: &str) -> Self {
        self.icon = Some(hash.into());
        self
    }

    pub fn channel(mut self, c: ChannelBuilder) -> Self {
        self.channels.push(c);
        self
    }

    pub fn role(mut self, id: u64, name: &str, permissions: u64, position: i32, color: u32) -> Self {
        self.roles.push(json!({
            "id": id.to_string(), "name": name, "permissions": permissions.to_string(), "position": position,
            "color": color, "colors": { "primary_color": color, "secondary_color": null, "tertiary_color": null },
            "hoist": true, "managed": false, "mentionable": true, "icon": null, "unicode_emoji": null, "flags": 0,
        }));
        self
    }

    pub fn emoji(mut self, id: u64, name: &str, animated: bool) -> Self {
        self.emojis.push(
            json!({ "id": id.to_string(), "name": name, "animated": animated, "roles": [], "require_colons": true, "managed": false, "available": true }),
        );
        self
    }

    pub fn my_roles(mut self, roles: &[u64]) -> Self {
        self.my_roles = roles.to_vec();
        self
    }

    pub fn my_nick(mut self, nick: &str) -> Self {
        self.my_nick = Some(nick.into());
        self
    }

    pub fn joined_at(mut self, ts: &str) -> Self {
        self.joined_at = ts.into();
        self
    }

    pub fn notifications(mut self, level: u8) -> Self {
        self.default_message_notifications = level;
        self
    }

    fn properties(&self) -> Value {
        json!({
            "id": self.id.to_string(), "name": self.name, "icon": self.icon, "splash": null, "banner": null,
            "description": null, "owner_id": self.owner_id.to_string(), "afk_channel_id": null, "afk_timeout": 300,
            "system_channel_id": null, "rules_channel_id": null, "public_updates_channel_id": null,
            "verification_level": 1, "default_message_notifications": self.default_message_notifications,
            "explicit_content_filter": 0, "mfa_level": 0, "nsfw_level": 0, "premium_tier": 0,
            "premium_progress_bar_enabled": false, "features": [], "vanity_url_code": null,
            "preferred_locale": "en-US", "max_members": 500000, "application_id": null, "hub_type": null,
        })
    }

    /// The current user's member object for `merged_members`.
    pub fn my_member(&self, user: u64) -> Value {
        json!({
            "user_id": user.to_string(), "roles": self.my_roles.iter().map(u64::to_string).collect::<Vec<_>>(),
            "nick": self.my_nick, "avatar": null, "banner": null, "joined_at": self.joined_at, "premium_since": null,
            "deaf": false, "mute": false, "pending": false, "flags": 0, "communication_disabled_until": null,
        })
    }

    /// READY / `GUILD_CREATE` form for user accounts (metadata under `properties`).
    pub fn build(&self) -> Value {
        json!({
            "id": self.id.to_string(), "properties": self.properties(),
            "channels": self.channels.iter().map(ChannelBuilder::build).collect::<Vec<_>>(),
            "threads": [], "roles": self.roles, "emojis": self.emojis, "stickers": [],
            "member_count": self.member_count, "premium_subscription_count": 0, "joined_at": self.joined_at,
            "lazy": true, "large": false, "data_mode": "full", "version": 1_700_000_000_000u64,
            "stage_instances": [], "guild_scheduled_events": [], "activity_instances": [], "soundboard_sounds": [],
        })
    }

    /// Flat REST form (`GET /guilds/{id}`).
    pub fn build_flat(&self) -> Value {
        let mut v = self.properties();
        v["roles"] = json!(self.roles);
        v["emojis"] = json!(self.emojis);
        v["stickers"] = json!([]);
        v["approximate_member_count"] = json!(self.member_count);
        v
    }
}

// ---------------------------------------------------------------- messages

#[derive(Debug, Clone)]
pub struct MessageBuilder {
    pub id: u64,
    pub channel_id: u64,
    pub guild_id: Option<u64>,
    pub author: Value,
    pub content: String,
    pub nonce: Option<String>,
    pub kind: u8,
    pub mentions: Vec<Value>,
    pub mention_everyone: bool,
    pub reactions: Vec<Value>,
    pub pinned: bool,
    pub reply_to: Option<u64>,
    pub edited: bool,
    pub flags: u64,
}

impl MessageBuilder {
    pub fn new(id: u64, channel: u64, author: &UserBuilder) -> Self {
        Self {
            id,
            channel_id: channel,
            guild_id: None,
            author: author.build(),
            content: String::new(),
            nonce: None,
            kind: 0,
            mentions: vec![],
            mention_everyone: false,
            reactions: vec![],
            pinned: false,
            reply_to: None,
            edited: false,
            flags: 0,
        }
    }

    pub fn content(mut self, c: impl Into<String>) -> Self {
        self.content = c.into();
        self
    }

    pub fn guild(mut self, g: u64) -> Self {
        self.guild_id = Some(g);
        self
    }

    pub fn nonce(mut self, n: &str) -> Self {
        self.nonce = Some(n.into());
        self
    }

    pub fn mention(mut self, u: &UserBuilder) -> Self {
        self.mentions.push(u.build());
        self
    }

    pub fn mention_everyone(mut self) -> Self {
        self.mention_everyone = true;
        self
    }

    /// A reaction with `emoji` = `{"id":…,"name":…}`.
    pub fn reaction(mut self, emoji: Value, count: u32, me: bool) -> Self {
        self.reactions.push(json!({
            "emoji": emoji, "count": count, "count_details": { "burst": 0, "normal": count },
            "burst_colors": [], "me_burst": false, "burst_me": false, "me": me, "burst_count": 0,
        }));
        self
    }

    pub fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    pub fn reply_to(mut self, id: u64) -> Self {
        self.reply_to = Some(id);
        self.kind = 19;
        self
    }

    pub fn edited(mut self) -> Self {
        self.edited = true;
        self
    }

    pub fn flags(mut self, f: u64) -> Self {
        self.flags = f;
        self
    }

    /// REST form (no `guild_id`/`member`).
    pub fn build(&self) -> Value {
        let mut v = json!({
            "id": self.id.to_string(), "channel_id": self.channel_id.to_string(), "author": self.author,
            "content": self.content, "timestamp": timestamp_of(self.id),
            "edited_timestamp": if self.edited { json!(timestamp_of(self.id + (1 << 22) * 1000)) } else { Value::Null },
            "tts": false, "mention_everyone": self.mention_everyone, "mentions": self.mentions, "mention_roles": [],
            "attachments": [], "embeds": [], "pinned": self.pinned, "type": self.kind, "flags": self.flags, "components": [],
        });
        if !self.reactions.is_empty() {
            v["reactions"] = json!(self.reactions);
        }
        if let Some(n) = &self.nonce {
            v["nonce"] = json!(n);
        }
        if let Some(r) = self.reply_to {
            v["message_reference"] =
                json!({ "type": 0, "channel_id": self.channel_id.to_string(), "message_id": r.to_string(), "guild_id": self.guild_id.map(|g| g.to_string()) });
            v["referenced_message"] = Value::Null;
        }
        v
    }

    /// Gateway `MESSAGE_CREATE` form (adds `guild_id` and `member` in guilds).
    pub fn build_event(&self) -> Value {
        let mut v = self.build();
        if let Some(g) = self.guild_id {
            v["guild_id"] = json!(g.to_string());
            v["member"] = json!({ "roles": [], "joined_at": "2024-01-01T00:00:00.000000+00:00", "deaf": false, "mute": false, "flags": 0, "nick": null, "avatar": null, "pending": false });
        }
        v
    }
}

// ---------------------------------------------------------------- settings

/// A settings proto with a status and guild folders `(id, name, guilds)`.
pub fn settings(status: &str, folders: &[(i64, Option<&str>, &[u64])]) -> PreloadedUserSettings {
    PreloadedUserSettings {
        status: Some(StatusSettings { status: Some(StringValue { value: status.into() }), custom_status: None, show_current_game: None }),
        guild_folders: Some(GuildFolders {
            folders: folders
                .iter()
                .map(|(id, name, guilds)| GuildFolder {
                    guild_ids: guilds.to_vec(),
                    id: Some(Int64Value { value: *id }),
                    name: name.map(|n| StringValue { value: n.into() }),
                    color: Some(UInt64Value { value: 0x5865f2 }),
                })
                .collect(),
            guild_positions: vec![],
        }),
        ..Default::default()
    }
}

// ---------------------------------------------------------------- READY

#[derive(Debug, Clone)]
pub struct ReadyBuilder {
    pub user: UserBuilder,
    pub guilds: Vec<GuildBuilder>,
    pub private_channels: Vec<ChannelBuilder>,
    pub users: Vec<UserBuilder>,
    pub relationships: Vec<Value>,
    pub read_states: Vec<Value>,
    pub guild_settings: Vec<Value>,
    pub settings: PreloadedUserSettings,
    pub friend_presences: Vec<Value>,
    pub notes: Map<String, Value>,
}

impl ReadyBuilder {
    /// An empty READY for `user`.
    pub fn new(user: UserBuilder) -> Self {
        Self {
            user,
            guilds: vec![],
            private_channels: vec![],
            users: vec![],
            relationships: vec![],
            read_states: vec![],
            guild_settings: vec![],
            settings: PreloadedUserSettings::default(),
            friend_presences: vec![],
            notes: Map::new(),
        }
    }

    /// One guild ("Mock Guild": a category with `#general` and `#random`, plus a
    /// voice channel, inside a folder), a friend with a DM, and read states that
    /// leave `#general` and the DM unread.
    pub fn standard() -> Self {
        use ids::*;
        let guild = GuildBuilder::new(GUILD, "Mock Guild")
            .role(ROLE_MOD, "Moderator", 0, 1, 0x3498db)
            .my_roles(&[ROLE_MOD])
            .channel(ChannelBuilder::category(CATEGORY, "Text Channels"))
            .channel(ChannelBuilder::text(GENERAL, "general").parent(CATEGORY).position(0).topic("General chat").last_message(GENERAL + 1_000))
            .channel(ChannelBuilder::text(RANDOM, "random").parent(CATEGORY).position(1))
            .channel(ChannelBuilder::voice(VOICE, "Voice").position(0));
        Self::new(UserBuilder::me())
            .guild(guild)
            .friend(UserBuilder::friend())
            .private_channel(ChannelBuilder::dm(DM, FRIEND).last_message(DM + 1_000))
            .read_state(GENERAL, GENERAL, 0)
            .read_state(DM, DM, 1)
            .settings(settings("online", &[(FOLDER, Some("Mock Folder"), &[GUILD])]))
    }

    pub fn guild(mut self, g: GuildBuilder) -> Self {
        self.guilds.push(g);
        self
    }

    pub fn private_channel(mut self, c: ChannelBuilder) -> Self {
        self.private_channels.push(c);
        self
    }

    /// Adds a user to the deduplicated `users[]`.
    pub fn user(mut self, u: UserBuilder) -> Self {
        if !self.users.iter().any(|x| x.id == u.id) {
            self.users.push(u);
        }
        self
    }

    /// Adds a friend (`users[]` + relationship type 1 + an online presence in READY_SUPPLEMENTAL).
    pub fn friend(self, u: UserBuilder) -> Self {
        let id = u.id;
        let mut s = self.user(u).relationship(id, 1);
        s.friend_presences.push(json!({ "user_id": id.to_string(), "status": "online", "activities": [], "client_status": { "desktop": "online" } }));
        s
    }

    /// Relationship by user id (1 friend, 2 blocked, 3 incoming, 4 outgoing).
    pub fn relationship(mut self, user: u64, kind: u8) -> Self {
        self.relationships.push(json!({ "id": user.to_string(), "type": kind, "user_id": user.to_string(), "nickname": null, "since": "2024-01-01T00:00:00.000000+00:00", "is_spam_request": false }));
        self
    }

    pub fn read_state(mut self, channel: u64, last_message_id: u64, mention_count: u32) -> Self {
        self.read_states.push(json!({
            "id": channel.to_string(), "last_message_id": last_message_id.to_string(), "mention_count": mention_count,
            "last_pin_timestamp": "1970-01-01T00:00:00+00:00", "flags": 0, "last_viewed": null,
        }));
        self
    }

    /// A `user_guild_settings` entry.
    pub fn guild_settings(mut self, entry: Value) -> Self {
        self.guild_settings.push(entry);
        self
    }

    pub fn settings(mut self, s: PreloadedUserSettings) -> Self {
        self.settings = s;
        self
    }

    pub fn note(mut self, user: u64, note: &str) -> Self {
        self.notes.insert(user.to_string(), json!(note));
        self
    }

    pub fn settings_proto(&self) -> &PreloadedUserSettings {
        &self.settings
    }

    pub fn user_value(&self) -> Value {
        self.user.build()
    }

    pub fn user_values(&self) -> Vec<Value> {
        self.users.iter().map(UserBuilder::build).collect()
    }

    /// Every channel with its guild, for registering in the world.
    pub fn channels(&self) -> Vec<(u64, Option<u64>)> {
        let mut v: Vec<(u64, Option<u64>)> = self.guilds.iter().flat_map(|g| g.channels.iter().map(move |c| (c.id, Some(g.id)))).collect();
        v.extend(self.private_channels.iter().map(|c| (c.id, None)));
        v
    }

    pub fn build(&self) -> Value {
        json!({
            "v": 9,
            "user": self.user.build_current(),
            "user_settings_proto": self.settings.to_base64(),
            "session_id": "mock-session",
            "session_type": "normal",
            "resume_gateway_url": "ws://127.0.0.1:9/resume",
            "auth_session_id_hash": "bW9jay1hdXRoLXNlc3Npb24",
            "static_client_session_id": "mock-static-session",
            "analytics_token": "mock-analytics",
            "country_code": "FI",
            "guilds": self.guilds.iter().map(GuildBuilder::build).collect::<Vec<_>>(),
            "merged_members": self.guilds.iter().map(|g| vec![g.my_member(self.user.id)]).collect::<Vec<_>>(),
            "users": self.user_values(),
            "private_channels": self.private_channels.iter().map(ChannelBuilder::build).collect::<Vec<_>>(),
            "relationships": self.relationships,
            "read_state": { "entries": self.read_states, "partial": false, "version": 1 },
            "user_guild_settings": { "entries": self.guild_settings, "partial": false, "version": 1 },
            "sessions": [{ "session_id": "mock-session", "status": "online", "activities": [], "client_info": { "client": "desktop", "os": "windows", "version": 0 }, "active": true }],
            "notes": self.notes,
            "connected_accounts": [], "guild_join_requests": [], "experiments": [], "guild_experiments": [],
            "geo_ordered_rtc_regions": ["finland", "rotterdam"], "friend_suggestion_count": 0,
            "api_code_version": 1, "explicit_content_scan_version": 2, "auth": { "authenticator_types": [] },
            "tutorial": null, "consents": { "personalization": { "consented": false } },
            "_trace": ["[\"mock-gateway\",{\"micros\":0}]"],
        })
    }

    pub fn build_supplemental(&self) -> Value {
        json!({
            "merged_presences": { "guilds": self.guilds.iter().map(|_| json!([])).collect::<Vec<_>>(), "friends": self.friend_presences },
            "merged_members": self.guilds.iter().map(|_| json!([])).collect::<Vec<_>>(),
            "guilds": self.guilds.iter().map(|g| json!({ "id": g.id.to_string(), "voice_states": [], "embedded_activities": [], "activity_instances": [] })).collect::<Vec<_>>(),
            "lazy_private_channels": [],
            "disclose": [],
        })
    }
}
