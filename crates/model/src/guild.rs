use serde::{Deserialize, Serialize};

use crate::{Channel, Permissions, Snowflake, User, cdn};

/// Mutable guild metadata. In user-account READY payloads this lives in a nested
/// `properties` object; in `GUILD_UPDATE` and REST it is top-level.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuildProperties {
    pub name: String,
    pub icon: Option<String>,
    pub splash: Option<String>,
    pub banner: Option<String>,
    pub description: Option<String>,
    pub owner_id: Snowflake,
    pub afk_channel_id: Option<Snowflake>,
    pub afk_timeout: u32,
    pub system_channel_id: Option<Snowflake>,
    pub rules_channel_id: Option<Snowflake>,
    pub public_updates_channel_id: Option<Snowflake>,
    pub verification_level: u8,
    pub default_message_notifications: u8,
    pub explicit_content_filter: u8,
    pub mfa_level: u8,
    pub nsfw_level: u8,
    pub premium_tier: u8,
    pub premium_progress_bar_enabled: bool,
    pub features: Vec<String>,
    pub vanity_url_code: Option<String>,
    pub preferred_locale: Option<String>,
    pub max_members: Option<u64>,
    pub application_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Guild {
    pub id: Snowflake,
    #[serde(flatten)]
    pub props: GuildProperties,
    pub unavailable: bool,
    pub member_count: Option<u64>,
    pub large: bool,
    pub lazy: bool,
    pub joined_at: Option<String>,
    pub premium_subscription_count: Option<u64>,
    pub channels: Vec<Channel>,
    pub threads: Vec<Channel>,
    pub roles: Vec<Role>,
    pub emojis: Vec<Emoji>,
    pub stickers: Vec<Sticker>,
    pub members: Vec<Member>,
    pub voice_states: Vec<crate::VoiceState>,
    pub presences: Vec<crate::Presence>,
    pub data_mode: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GuildWire {
    id: Snowflake,
    properties: Option<GuildProperties>,
    #[serde(flatten)]
    top: GuildProperties,
    unavailable: bool,
    member_count: Option<u64>,
    large: bool,
    lazy: bool,
    joined_at: Option<String>,
    premium_subscription_count: Option<u64>,
    channels: Vec<Channel>,
    threads: Vec<Channel>,
    roles: Vec<Role>,
    emojis: Vec<Emoji>,
    stickers: Vec<Sticker>,
    members: Vec<Member>,
    voice_states: Vec<crate::VoiceState>,
    presences: Vec<crate::Presence>,
    data_mode: Option<String>,
}

impl<'de> Deserialize<'de> for Guild {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = GuildWire::deserialize(d)?;
        let mut channels = w.channels;
        for c in &mut channels {
            c.guild_id.get_or_insert(w.id);
        }
        let mut threads = w.threads;
        for c in &mut threads {
            c.guild_id.get_or_insert(w.id);
        }
        Ok(Guild {
            id: w.id,
            props: w.properties.unwrap_or(w.top),
            unavailable: w.unavailable,
            member_count: w.member_count,
            large: w.large,
            lazy: w.lazy,
            joined_at: w.joined_at,
            premium_subscription_count: w.premium_subscription_count,
            channels,
            threads,
            roles: w.roles,
            emojis: w.emojis,
            stickers: w.stickers,
            members: w.members,
            voice_states: w.voice_states,
            presences: w.presences,
            data_mode: w.data_mode,
        })
    }
}

impl Guild {
    pub fn name(&self) -> &str {
        &self.props.name
    }

    pub fn icon_url(&self, size: u32) -> Option<String> {
        self.props.icon.as_ref().map(|h| cdn::guild_icon(self.id, h, size))
    }

    /// Short initials used as an icon fallback ("Rust Programming" -> "RP").
    pub fn acronym(&self) -> String {
        self.props.name.split_whitespace().filter_map(|w| w.chars().next()).take(4).collect()
    }

    pub fn everyone_role(&self) -> Option<&Role> {
        self.roles.iter().find(|r| r.id == self.id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    pub id: Snowflake,
    pub name: String,
    pub color: u32,
    pub colors: Option<RoleColors>,
    pub hoist: bool,
    pub icon: Option<String>,
    pub unicode_emoji: Option<String>,
    pub position: i32,
    pub permissions: Permissions,
    pub managed: bool,
    pub mentionable: bool,
    pub flags: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleColors {
    pub primary_color: u32,
    pub secondary_color: Option<u32>,
    pub tertiary_color: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Emoji {
    pub id: Option<Snowflake>,
    pub name: Option<String>,
    pub roles: Vec<Snowflake>,
    pub require_colons: bool,
    pub managed: bool,
    pub animated: bool,
    pub available: Option<bool>,
}

impl Emoji {
    pub fn url(&self, size: u32) -> Option<String> {
        self.id.map(|id| cdn::emoji(id, self.animated, size))
    }
}

/// Emoji reference as used in reactions and custom statuses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialEmoji {
    #[serde(deserialize_with = "crate::snowflake::opt_snowflake_lenient")]
    pub id: Option<Snowflake>,
    pub name: Option<String>,
    pub animated: bool,
}

impl PartialEmoji {
    pub fn unicode(s: impl Into<String>) -> Self {
        Self { id: None, name: Some(s.into()), animated: false }
    }

    /// Encoding used in reaction routes: `name:id` for custom, URL-escaped unicode otherwise.
    pub fn route_key(&self) -> String {
        match self.id {
            Some(id) => format!("{}:{}", self.name.as_deref().unwrap_or("_"), id),
            None => self.name.clone().unwrap_or_default(),
        }
    }

    pub fn url(&self, size: u32) -> Option<String> {
        self.id.map(|id| cdn::emoji(id, self.animated, size))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Sticker {
    pub id: Snowflake,
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<String>,
    pub format_type: u8,
    pub guild_id: Option<Snowflake>,
    pub available: Option<bool>,
}

impl Sticker {
    pub fn url(&self) -> String {
        cdn::sticker(self.id, self.format_type)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Member {
    pub user: Option<User>,
    /// Present with user-object dedupe (`merged_members`).
    pub user_id: Option<Snowflake>,
    pub guild_id: Option<Snowflake>,
    pub nick: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub roles: Vec<Snowflake>,
    pub joined_at: Option<String>,
    pub premium_since: Option<String>,
    pub deaf: bool,
    pub mute: bool,
    pub pending: bool,
    pub flags: u64,
    pub communication_disabled_until: Option<String>,
}

impl Member {
    pub fn user_id(&self) -> Option<Snowflake> {
        self.user_id.or_else(|| self.user.as_ref().map(|u| u.id))
    }

    pub fn avatar_url(&self, guild_id: Snowflake, size: u32) -> Option<String> {
        let uid = self.user_id()?;
        self.avatar.as_ref().map(|h| cdn::member_avatar(guild_id, uid, h, size))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UnavailableGuild {
    pub id: Snowflake,
    pub unavailable: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ban {
    pub reason: Option<String>,
    pub user: User,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Invite {
    pub code: String,
    pub guild: Option<serde_json::Value>,
    pub channel: Option<serde_json::Value>,
    pub inviter: Option<User>,
    pub approximate_member_count: Option<u64>,
    pub approximate_presence_count: Option<u64>,
    pub expires_at: Option<String>,
}
