use serde::{Deserialize, Serialize};

use crate::{Permissions, Snowflake, User, cdn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub enum ChannelType {
    #[default]
    GuildText,
    Dm,
    GuildVoice,
    GroupDm,
    GuildCategory,
    GuildAnnouncement,
    AnnouncementThread,
    PublicThread,
    PrivateThread,
    GuildStageVoice,
    GuildDirectory,
    GuildForum,
    GuildMedia,
    Unknown(u8),
}

impl From<u8> for ChannelType {
    fn from(v: u8) -> Self {
        use ChannelType::*;
        match v {
            0 => GuildText,
            1 => Dm,
            2 => GuildVoice,
            3 => GroupDm,
            4 => GuildCategory,
            5 => GuildAnnouncement,
            10 => AnnouncementThread,
            11 => PublicThread,
            12 => PrivateThread,
            13 => GuildStageVoice,
            14 => GuildDirectory,
            15 => GuildForum,
            16 => GuildMedia,
            x => Unknown(x),
        }
    }
}

impl From<ChannelType> for u8 {
    fn from(v: ChannelType) -> u8 {
        use ChannelType::*;
        match v {
            GuildText => 0,
            Dm => 1,
            GuildVoice => 2,
            GroupDm => 3,
            GuildCategory => 4,
            GuildAnnouncement => 5,
            AnnouncementThread => 10,
            PublicThread => 11,
            PrivateThread => 12,
            GuildStageVoice => 13,
            GuildDirectory => 14,
            GuildForum => 15,
            GuildMedia => 16,
            Unknown(x) => x,
        }
    }
}

impl ChannelType {
    pub fn is_private(self) -> bool {
        matches!(self, Self::Dm | Self::GroupDm)
    }

    pub fn is_thread(self) -> bool {
        matches!(self, Self::AnnouncementThread | Self::PublicThread | Self::PrivateThread)
    }

    pub fn is_voice(self) -> bool {
        matches!(self, Self::GuildVoice | Self::GuildStageVoice)
    }

    /// Channels whose message history can be shown in the chat view.
    pub fn is_text_based(self) -> bool {
        matches!(
            self,
            Self::GuildText
                | Self::Dm
                | Self::GroupDm
                | Self::GuildAnnouncement
                | Self::AnnouncementThread
                | Self::PublicThread
                | Self::PrivateThread
                | Self::GuildVoice
                | Self::GuildStageVoice
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub enum OverwriteType {
    #[default]
    Role,
    Member,
}

impl From<u8> for OverwriteType {
    fn from(v: u8) -> Self {
        if v == 1 { Self::Member } else { Self::Role }
    }
}

impl From<OverwriteType> for u8 {
    fn from(v: OverwriteType) -> u8 {
        match v {
            OverwriteType::Role => 0,
            OverwriteType::Member => 1,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionOverwrite {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: OverwriteType,
    pub allow: Permissions,
    pub deny: Permissions,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadMetadata {
    pub archived: bool,
    pub auto_archive_duration: u32,
    pub archive_timestamp: Option<String>,
    pub locked: bool,
    pub invitable: Option<bool>,
    pub create_timestamp: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ForumTag {
    pub id: Snowflake,
    pub name: String,
    pub moderated: bool,
    pub emoji_id: Option<Snowflake>,
    pub emoji_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: ChannelType,
    pub guild_id: Option<Snowflake>,
    pub position: i32,
    pub permission_overwrites: Vec<PermissionOverwrite>,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub nsfw: bool,
    #[serde(deserialize_with = "crate::snowflake::opt_snowflake_lenient")]
    pub last_message_id: Option<Snowflake>,
    pub last_pin_timestamp: Option<String>,
    pub bitrate: Option<u32>,
    pub user_limit: Option<u32>,
    pub rate_limit_per_user: Option<u32>,
    /// DM recipients when user-object dedupe is off.
    pub recipients: Vec<User>,
    /// DM recipients when user-object dedupe is on (READY `private_channels`).
    pub recipient_ids: Vec<Snowflake>,
    pub icon: Option<String>,
    pub owner_id: Option<Snowflake>,
    pub application_id: Option<Snowflake>,
    pub parent_id: Option<Snowflake>,
    pub rtc_region: Option<String>,
    pub video_quality_mode: Option<u8>,
    pub message_count: Option<u32>,
    pub member_count: Option<u32>,
    pub thread_metadata: Option<ThreadMetadata>,
    pub default_auto_archive_duration: Option<u32>,
    pub flags: u64,
    pub available_tags: Vec<ForumTag>,
    pub applied_tags: Vec<Snowflake>,
    pub is_spam: Option<bool>,
    pub is_message_request: Option<bool>,
    pub is_message_request_timestamp: Option<String>,
}

impl Channel {
    pub fn display_name(&self) -> String {
        if let Some(n) = &self.name
            && !n.is_empty()
        {
            return n.clone();
        }
        let names: Vec<&str> = self.recipients.iter().map(|u| u.display_name()).collect();
        if names.is_empty() { "Unnamed".into() } else { names.join(", ") }
    }

    pub fn icon_url(&self, size: u32) -> Option<String> {
        self.icon.as_ref().map(|h| cdn::channel_icon(self.id, h, size))
    }

    pub fn recipient_ids(&self) -> Vec<Snowflake> {
        if !self.recipient_ids.is_empty() { self.recipient_ids.clone() } else { self.recipients.iter().map(|u| u.id).collect() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceState {
    pub guild_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub user_id: Snowflake,
    pub member: Option<crate::Member>,
    pub session_id: String,
    pub deaf: bool,
    pub mute: bool,
    pub self_deaf: bool,
    pub self_mute: bool,
    pub self_stream: Option<bool>,
    pub self_video: bool,
    pub suppress: bool,
    pub request_to_speak_timestamp: Option<String>,
}

/// Per-channel read marker from READY `read_state` and `MESSAGE_ACK`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadState {
    pub id: Snowflake,
    #[serde(deserialize_with = "crate::snowflake::opt_snowflake_lenient")]
    pub last_message_id: Option<Snowflake>,
    pub mention_count: u32,
    pub last_pin_timestamp: Option<String>,
    pub flags: Option<u32>,
    pub last_viewed: Option<u32>,
    /// 0 = channel; others are non-channel read states (guild events, etc.).
    pub read_state_type: Option<u8>,
    pub badge_count: Option<u32>,
}

/// Read states arrive as `{entries:[..], partial, version}` with
/// `VERSIONED_READ_STATES`, or as a bare array otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReadStates {
    Versioned {
        entries: Vec<ReadState>,
        #[serde(default)]
        partial: bool,
        #[serde(default)]
        version: u64,
    },
    Plain(Vec<ReadState>),
}

impl Default for ReadStates {
    fn default() -> Self {
        Self::Plain(vec![])
    }
}

impl ReadStates {
    pub fn into_entries(self) -> Vec<ReadState> {
        match self {
            Self::Versioned { entries, .. } => entries,
            Self::Plain(v) => v,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelOverride {
    pub channel_id: Snowflake,
    pub muted: bool,
    pub message_notifications: u8,
    pub collapsed: bool,
    pub mute_config: Option<MuteConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MuteConfig {
    pub end_time: Option<String>,
    pub selected_time_window: Option<i64>,
}

/// Per-guild notification settings (`user_guild_settings` in READY).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserGuildSettings {
    pub guild_id: Option<Snowflake>,
    pub muted: bool,
    pub mute_config: Option<MuteConfig>,
    pub suppress_everyone: bool,
    pub suppress_roles: bool,
    pub message_notifications: u8,
    pub mobile_push: bool,
    pub hide_muted_channels: bool,
    pub channel_overrides: Vec<ChannelOverride>,
    pub flags: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserGuildSettingsList {
    Versioned {
        entries: Vec<UserGuildSettings>,
        #[serde(default)]
        partial: bool,
        #[serde(default)]
        version: u64,
    },
    Plain(Vec<UserGuildSettings>),
}

impl Default for UserGuildSettingsList {
    fn default() -> Self {
        Self::Plain(vec![])
    }
}

impl UserGuildSettingsList {
    pub fn into_entries(self) -> Vec<UserGuildSettings> {
        match self {
            Self::Versioned { entries, .. } => entries,
            Self::Plain(v) => v,
        }
    }
}
