use serde::{Deserialize, Serialize};

use crate::{Snowflake, cdn};

/// A Discord user. Only `id` is guaranteed; partial user objects are common
/// (e.g. in `GUILD_MEMBER_LIST_UPDATE` or presence updates).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: Snowflake,
    pub username: String,
    /// Legacy 4-digit discriminator; `"0"` for migrated (pomelo) usernames.
    pub discriminator: String,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
    pub avatar_decoration_data: Option<serde_json::Value>,
    pub banner: Option<String>,
    pub accent_color: Option<u32>,
    pub bot: bool,
    pub system: bool,
    pub public_flags: u64,
    pub flags: u64,
    pub premium_type: Option<u8>,
    pub primary_guild: Option<PrimaryGuild>,
    pub collectibles: Option<serde_json::Value>,
    pub display_name_styles: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrimaryGuild {
    pub identity_guild_id: Option<Snowflake>,
    pub identity_enabled: Option<bool>,
    pub tag: Option<String>,
    pub badge: Option<String>,
}

impl User {
    /// Name to show in UI: global display name falling back to username.
    pub fn display_name(&self) -> &str {
        match &self.global_name {
            Some(n) if !n.is_empty() => n,
            _ => &self.username,
        }
    }

    /// `username` or `username#1234` for legacy accounts.
    pub fn tag(&self) -> String {
        if self.discriminator.is_empty() || self.discriminator == "0" {
            self.username.clone()
        } else {
            format!("{}#{}", self.username, self.discriminator)
        }
    }

    pub fn avatar_url(&self, size: u32) -> String {
        match &self.avatar {
            Some(hash) => cdn::user_avatar(self.id, hash, size),
            None => cdn::default_avatar(self.id, &self.discriminator),
        }
    }
}

/// The logged-in account (`/users/@me` and READY `user`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CurrentUser {
    #[serde(flatten)]
    pub user: User,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub verified: bool,
    pub mfa_enabled: bool,
    pub nsfw_allowed: Option<bool>,
    pub bio: Option<String>,
    pub pronouns: Option<String>,
    pub locale: Option<String>,
    pub purchased_flags: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(from = "u8", into = "u8")]
pub enum RelationshipType {
    #[default]
    None,
    Friend,
    Blocked,
    IncomingRequest,
    OutgoingRequest,
    Implicit,
    Unknown(u8),
}

impl From<u8> for RelationshipType {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::None,
            1 => Self::Friend,
            2 => Self::Blocked,
            3 => Self::IncomingRequest,
            4 => Self::OutgoingRequest,
            5 => Self::Implicit,
            x => Self::Unknown(x),
        }
    }
}

impl From<RelationshipType> for u8 {
    fn from(v: RelationshipType) -> u8 {
        match v {
            RelationshipType::None => 0,
            RelationshipType::Friend => 1,
            RelationshipType::Blocked => 2,
            RelationshipType::IncomingRequest => 3,
            RelationshipType::OutgoingRequest => 4,
            RelationshipType::Implicit => 5,
            RelationshipType::Unknown(x) => x,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Relationship {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: RelationshipType,
    pub nickname: Option<String>,
    /// Present without user-object dedupe; with dedupe use `user_id`.
    pub user: Option<User>,
    pub user_id: Option<Snowflake>,
    pub since: Option<String>,
    pub is_spam_request: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Online,
    Idle,
    Dnd,
    Invisible,
    #[default]
    #[serde(other)]
    Offline,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Online => "online",
            Status::Idle => "idle",
            Status::Dnd => "dnd",
            Status::Invisible => "invisible",
            Status::Offline => "offline",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Online => "Online",
            Status::Idle => "Idle",
            Status::Dnd => "Do Not Disturb",
            Status::Invisible => "Invisible",
            Status::Offline => "Offline",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientStatus {
    pub desktop: Option<Status>,
    pub mobile: Option<Status>,
    pub web: Option<Status>,
    pub embedded: Option<Status>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Activity {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: u8,
    pub url: Option<String>,
    pub state: Option<String>,
    pub details: Option<String>,
    pub application_id: Option<Snowflake>,
    pub emoji: Option<crate::PartialEmoji>,
    pub timestamps: Option<serde_json::Value>,
    pub assets: Option<serde_json::Value>,
    pub created_at: Option<u64>,
}

pub mod activity_type {
    pub const PLAYING: u8 = 0;
    pub const STREAMING: u8 = 1;
    pub const LISTENING: u8 = 2;
    pub const WATCHING: u8 = 3;
    pub const CUSTOM: u8 = 4;
    pub const COMPETING: u8 = 5;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Presence {
    pub user: Option<PartialUserRef>,
    pub user_id: Option<Snowflake>,
    pub guild_id: Option<Snowflake>,
    pub status: Status,
    pub activities: Vec<Activity>,
    pub client_status: ClientStatus,
}

impl Presence {
    pub fn user_id(&self) -> Option<Snowflake> {
        self.user_id.or_else(|| self.user.as_ref().map(|u| u.id))
    }

    pub fn custom_status(&self) -> Option<&Activity> {
        self.activities.iter().find(|a| a.kind == activity_type::CUSTOM)
    }
}

/// Presence/member payloads often carry a user object with only `id` (or a full one).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialUserRef {
    pub id: Snowflake,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

impl PartialUserRef {
    /// Returns a full user if this reference carries a username.
    pub fn as_user(&self) -> Option<User> {
        if self.rest.contains_key("username") {
            let mut m = self.rest.clone();
            m.insert("id".into(), serde_json::Value::String(self.id.to_string()));
            serde_json::from_value(serde_json::Value::Object(m)).ok()
        } else {
            None
        }
    }
}

/// Response of `GET /users/{id}/profile`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserProfile {
    pub user: User,
    pub user_profile: Option<ProfileMeta>,
    pub mutual_guilds: Option<Vec<MutualGuild>>,
    pub mutual_friends_count: Option<u32>,
    pub premium_since: Option<String>,
    pub premium_guild_since: Option<String>,
    pub connected_accounts: Vec<serde_json::Value>,
    pub badges: Vec<serde_json::Value>,
    pub guild_member: Option<crate::Member>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileMeta {
    pub bio: Option<String>,
    pub pronouns: Option<String>,
    pub accent_color: Option<u32>,
    pub banner: Option<String>,
    pub theme_colors: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MutualGuild {
    pub id: Snowflake,
    pub nick: Option<String>,
}
