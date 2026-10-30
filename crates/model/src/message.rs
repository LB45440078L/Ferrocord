use serde::{Deserialize, Serialize};

use crate::{Channel, Member, PartialEmoji, Snowflake, User};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub author: User,
    /// Present on gateway `MESSAGE_CREATE` in guilds.
    pub member: Option<Member>,
    pub content: String,
    pub timestamp: String,
    pub edited_timestamp: Option<String>,
    pub tts: bool,
    pub mention_everyone: bool,
    pub mentions: Vec<User>,
    pub mention_roles: Vec<Snowflake>,
    pub mention_channels: Vec<serde_json::Value>,
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub reactions: Vec<Reaction>,
    pub nonce: Option<serde_json::Value>,
    pub pinned: bool,
    pub webhook_id: Option<Snowflake>,
    #[serde(rename = "type")]
    pub kind: u8,
    pub activity: Option<serde_json::Value>,
    pub application: Option<serde_json::Value>,
    pub application_id: Option<Snowflake>,
    pub message_reference: Option<MessageReference>,
    pub message_snapshots: Vec<MessageSnapshot>,
    pub flags: u64,
    pub referenced_message: Option<Box<Message>>,
    pub interaction_metadata: Option<serde_json::Value>,
    pub thread: Option<Box<Channel>>,
    pub components: Vec<serde_json::Value>,
    pub sticker_items: Vec<StickerItem>,
    pub poll: Option<serde_json::Value>,
    pub call: Option<serde_json::Value>,
}

impl Message {
    pub fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.id.created_at()
    }

    pub fn is_edited(&self) -> bool {
        self.edited_timestamp.is_some()
    }

    pub fn mentions_user(&self, id: Snowflake) -> bool {
        self.mentions.iter().any(|u| u.id == id)
    }

    /// Returns true for user-authored chat messages (default + reply), as opposed
    /// to system messages such as joins, pins or boosts.
    pub fn is_regular(&self) -> bool {
        matches!(self.kind, message_type::DEFAULT | message_type::REPLY | message_type::CHAT_INPUT_COMMAND | message_type::CONTEXT_MENU_COMMAND)
    }
}

pub mod message_type {
    pub const DEFAULT: u8 = 0;
    pub const RECIPIENT_ADD: u8 = 1;
    pub const RECIPIENT_REMOVE: u8 = 2;
    pub const CALL: u8 = 3;
    pub const CHANNEL_NAME_CHANGE: u8 = 4;
    pub const CHANNEL_ICON_CHANGE: u8 = 5;
    pub const CHANNEL_PINNED_MESSAGE: u8 = 6;
    pub const USER_JOIN: u8 = 7;
    pub const GUILD_BOOST: u8 = 8;
    pub const GUILD_BOOST_TIER_1: u8 = 9;
    pub const GUILD_BOOST_TIER_2: u8 = 10;
    pub const GUILD_BOOST_TIER_3: u8 = 11;
    pub const CHANNEL_FOLLOW_ADD: u8 = 12;
    pub const THREAD_CREATED: u8 = 18;
    pub const REPLY: u8 = 19;
    pub const CHAT_INPUT_COMMAND: u8 = 20;
    pub const THREAD_STARTER_MESSAGE: u8 = 21;
    pub const CONTEXT_MENU_COMMAND: u8 = 23;
    pub const AUTO_MODERATION_ACTION: u8 = 24;
    pub const POLL_RESULT: u8 = 46;
}

pub mod message_flags {
    pub const CROSSPOSTED: u64 = 1 << 0;
    pub const IS_CROSSPOST: u64 = 1 << 1;
    pub const SUPPRESS_EMBEDS: u64 = 1 << 2;
    pub const URGENT: u64 = 1 << 4;
    pub const HAS_THREAD: u64 = 1 << 5;
    pub const EPHEMERAL: u64 = 1 << 6;
    pub const LOADING: u64 = 1 << 7;
    pub const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;
    pub const IS_VOICE_MESSAGE: u64 = 1 << 13;
    pub const HAS_SNAPSHOT: u64 = 1 << 14;
    pub const IS_COMPONENTS_V2: u64 = 1 << 15;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageReference {
    #[serde(rename = "type")]
    pub kind: Option<u8>,
    pub message_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub guild_id: Option<Snowflake>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_if_not_exists: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageSnapshot {
    pub message: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StickerItem {
    pub id: Snowflake,
    pub name: String,
    pub format_type: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub id: Snowflake,
    pub filename: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content_type: Option<String>,
    pub size: u64,
    pub url: String,
    pub proxy_url: String,
    pub height: Option<u32>,
    pub width: Option<u32>,
    pub ephemeral: bool,
    pub duration_secs: Option<f64>,
    pub waveform: Option<String>,
    pub flags: u64,
    pub placeholder: Option<String>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.content_type.as_deref().is_some_and(|c| c.starts_with("image/"))
            || (self.width.is_some() && !self.is_video())
    }

    pub fn is_video(&self) -> bool {
        self.content_type.as_deref().is_some_and(|c| c.starts_with("video/"))
    }

    pub fn is_spoiler(&self) -> bool {
        self.filename.starts_with("SPOILER_") || self.flags & 8 != 0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Embed {
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub timestamp: Option<String>,
    pub color: Option<u32>,
    pub footer: Option<EmbedFooter>,
    pub image: Option<EmbedMedia>,
    pub thumbnail: Option<EmbedMedia>,
    pub video: Option<EmbedMedia>,
    pub provider: Option<EmbedProvider>,
    pub author: Option<EmbedAuthor>,
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedFooter {
    pub text: String,
    pub icon_url: Option<String>,
    pub proxy_icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedMedia {
    pub url: String,
    pub proxy_url: Option<String>,
    pub height: Option<u32>,
    pub width: Option<u32>,
    pub content_type: Option<String>,
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedProvider {
    pub name: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedAuthor {
    pub name: String,
    pub url: Option<String>,
    pub icon_url: Option<String>,
    pub proxy_icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub count: u32,
    pub count_details: Option<ReactionCountDetails>,
    pub me: bool,
    pub me_burst: bool,
    pub emoji: PartialEmoji,
    pub burst_colors: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactionCountDetails {
    pub burst: u32,
    pub normal: u32,
}

/// Body for `POST /channels/{id}/messages`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateMessage {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(default)]
    pub tts: bool,
    #[serde(skip_serializing_if = "u64_is_zero", default)]
    pub flags: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_reference: Option<MessageReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_mentions: Option<AllowedMentions>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub sticker_ids: Vec<Snowflake>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub attachments: Vec<AttachmentRef>,
    /// Set by the official client; enables server-side nonce de-duplication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enforce_nonce: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mobile_network_type: Option<String>,
}

fn u64_is_zero(v: &u64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub id: String,
    pub filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AllowedMentions {
    pub parse: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub users: Vec<Snowflake>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub roles: Vec<Snowflake>,
    pub replied_user: bool,
}

impl AllowedMentions {
    /// The client default: parse everything, ping the replied user.
    pub fn all(replied_user: bool) -> Self {
        Self {
            parse: vec!["users".into(), "roles".into(), "everyone".into()],
            replied_user,
            ..Default::default()
        }
    }
}
