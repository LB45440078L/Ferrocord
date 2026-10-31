//! Gateway (websocket) wire protocol: opcodes, envelopes, identify payload and dispatch events.
//!
//! Opcode values and payload shapes were taken from the official web client
//! (build 657850): see `docs/PROTOCOL.md`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::*;

pub mod opcode {
    pub const DISPATCH: u8 = 0;
    pub const HEARTBEAT: u8 = 1;
    pub const IDENTIFY: u8 = 2;
    pub const PRESENCE_UPDATE: u8 = 3;
    pub const VOICE_STATE_UPDATE: u8 = 4;
    pub const VOICE_SERVER_PING: u8 = 5;
    pub const RESUME: u8 = 6;
    pub const RECONNECT: u8 = 7;
    pub const REQUEST_GUILD_MEMBERS: u8 = 8;
    pub const INVALID_SESSION: u8 = 9;
    pub const HELLO: u8 = 10;
    pub const HEARTBEAT_ACK: u8 = 11;
    pub const CALL_CONNECT: u8 = 13;
    pub const GUILD_SUBSCRIPTIONS: u8 = 14;
    pub const STREAM_CREATE: u8 = 18;
    pub const STREAM_DELETE: u8 = 19;
    pub const STREAM_WATCH: u8 = 20;
    pub const STREAM_PING: u8 = 21;
    pub const STREAM_SET_PAUSED: u8 = 22;
    pub const REQUEST_GUILD_APPLICATION_COMMANDS: u8 = 24;
    pub const REQUEST_FORUM_UNREADS: u8 = 28;
    pub const REMOTE_COMMAND: u8 = 29;
    pub const REQUEST_SOUNDBOARD_SOUNDS: u8 = 31;
    pub const REQUEST_LAST_MESSAGES: u8 = 34;
    pub const SEARCH_RECENT_MEMBERS: u8 = 35;
    pub const GUILD_SUBSCRIPTIONS_BULK: u8 = 37;
    pub const GUILD_CHANNELS_RESYNC: u8 = 38;
    pub const REQUEST_CHANNEL_MEMBER_COUNT: u8 = 39;
}

/// Capability flags sent in IDENTIFY. The official desktop client sends
/// `1734653` (or `1767421` with private-channel obfuscation).
pub mod capabilities {
    pub const LAZY_USER_NOTES: u64 = 1 << 0;
    pub const NO_AFFINE_USER_IDS: u64 = 1 << 1;
    pub const VERSIONED_READ_STATES: u64 = 1 << 2;
    pub const VERSIONED_USER_GUILD_SETTINGS: u64 = 1 << 3;
    pub const DEDUPE_USER_OBJECTS: u64 = 1 << 4;
    pub const PRIORITIZED_READY_PAYLOAD: u64 = 1 << 5;
    pub const MULTIPLE_GUILD_EXPERIMENT_POPULATIONS: u64 = 1 << 6;
    pub const NON_CHANNEL_READ_STATES: u64 = 1 << 7;
    pub const AUTH_TOKEN_REFRESH: u64 = 1 << 8;
    pub const USER_SETTINGS_PROTO: u64 = 1 << 9;
    pub const CLIENT_STATE_V2: u64 = 1 << 10;
    pub const PASSIVE_GUILD_UPDATE: u64 = 1 << 11;
    pub const AUTO_CALL_CONNECT: u64 = 1 << 12;
    pub const DEBOUNCE_MESSAGE_REACTIONS: u64 = 1 << 13;
    pub const PASSIVE_GUILD_UPDATE_V2: u64 = 1 << 14;

    /// Value used by the official desktop client.
    pub const OFFICIAL_DESKTOP: u64 = 1_734_653;
}

/// Raw gateway frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayPayload {
    pub op: u8,
    #[serde(default)]
    pub d: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hello {
    pub heartbeat_interval: u64,
}

/// Client fingerprint sent in IDENTIFY `properties` and, base64-encoded, as the
/// `X-Super-Properties` header on every REST call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SuperProperties {
    pub os: String,
    pub browser: String,
    pub release_channel: String,
    pub client_version: String,
    pub os_version: String,
    pub os_arch: String,
    pub app_arch: String,
    pub system_locale: String,
    pub has_client_mods: bool,
    pub client_launch_id: String,
    pub browser_user_agent: String,
    pub browser_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_sdk_version: Option<String>,
    pub client_build_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_build_number: Option<u64>,
    pub client_event_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_heartbeat_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_app_state: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Identify {
    pub token: String,
    pub capabilities: u64,
    pub properties: Value,
    pub presence: UpdatePresence,
    pub compress: bool,
    pub client_state: ClientState,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ClientState {
    pub guild_versions: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePresence {
    pub status: Status,
    pub since: u64,
    pub activities: Vec<Activity>,
    pub afk: bool,
}

impl Default for UpdatePresence {
    fn default() -> Self {
        Self { status: Status::Online, since: 0, activities: vec![], afk: false }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Resume {
    pub token: String,
    pub session_id: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateVoiceState {
    pub guild_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub self_video: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RequestGuildMembers {
    pub guild_id: Vec<Snowflake>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub user_ids: Vec<Snowflake>,
    pub presences: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

/// One guild's entry in `GUILD_SUBSCRIPTIONS_BULK` (op 37).
#[derive(Debug, Clone, Serialize, Default)]
pub struct GuildSubscription {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activities: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_updates: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<Snowflake>>,
    /// channel id -> list of `[start, end]` member-list ranges (100 rows each).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub channels: HashMap<Snowflake, Vec<[u32; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_member_lists: Option<Vec<Snowflake>>,
}

// ---------------------------------------------------------------------------
// Dispatch payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ready {
    pub v: u8,
    pub user: CurrentUser,
    pub session_id: String,
    pub resume_gateway_url: Option<String>,
    /// Sent with `AUTH_TOKEN_REFRESH`: replaces the stored token.
    pub auth_token: Option<String>,
    pub guilds: Vec<Guild>,
    pub private_channels: Vec<Channel>,
    pub relationships: Vec<Relationship>,
    /// Deduplicated user objects referenced by id elsewhere in READY.
    pub users: Vec<User>,
    /// Per-guild member lists, parallel to `guilds` (current user's member object).
    pub merged_members: Vec<Vec<Member>>,
    pub read_state: ReadStates,
    pub user_guild_settings: UserGuildSettingsList,
    /// Base64 `PreloadedUserSettings` protobuf.
    pub user_settings_proto: Option<String>,
    pub sessions: Vec<Value>,
    pub country_code: Option<String>,
    pub analytics_token: Option<String>,
    pub auth_session_id_hash: Option<String>,
    pub static_client_session_id: Option<String>,
    pub notes: Option<HashMap<Snowflake, String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MergedPresences {
    pub guilds: Vec<Vec<Presence>>,
    pub friends: Vec<Presence>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SupplementalGuild {
    pub id: Snowflake,
    pub voice_states: Vec<VoiceState>,
    pub embedded_activities: Vec<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReadySupplemental {
    pub merged_presences: MergedPresences,
    pub merged_members: Vec<Vec<Member>>,
    pub guilds: Vec<SupplementalGuild>,
    pub lazy_private_channels: Vec<Channel>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageDelete {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageDeleteBulk {
    pub ids: Vec<Snowflake>,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactionEvent {
    pub user_id: Snowflake,
    pub channel_id: Snowflake,
    pub message_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub emoji: PartialEmoji,
    pub burst: bool,
    pub member: Option<Member>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactionRemoveAll {
    pub channel_id: Snowflake,
    pub message_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub emoji: Option<PartialEmoji>,
}

/// `MESSAGE_REACTION_ADD_MANY` (sent with `DEBOUNCE_MESSAGE_REACTIONS`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactionAddMany {
    pub channel_id: Snowflake,
    pub message_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub reactions: Vec<DebouncedReaction>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DebouncedReaction {
    pub emoji: PartialEmoji,
    pub users: Vec<Snowflake>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageAck {
    pub channel_id: Snowflake,
    pub message_id: Snowflake,
    pub version: u64,
    pub mention_count: Option<u32>,
    pub manual: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TypingStart {
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub user_id: Snowflake,
    pub timestamp: u64,
    pub member: Option<Member>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelPinsUpdate {
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub last_pin_timestamp: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleEvent {
    pub guild_id: Snowflake,
    pub role: Role,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleDelete {
    pub guild_id: Snowflake,
    pub role_id: Snowflake,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmojisUpdate {
    pub guild_id: Snowflake,
    pub emojis: Vec<Emoji>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MemberRemove {
    pub guild_id: Snowflake,
    pub user: User,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MembersChunk {
    pub guild_id: Snowflake,
    pub members: Vec<Member>,
    pub presences: Vec<Presence>,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub not_found: Vec<Snowflake>,
    pub nonce: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MemberListGroup {
    pub id: String,
    pub count: u32,
}

/// Entry of the lazy member sidebar: either a group header (role/online/offline) or a member.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberListItem {
    Group(MemberListGroup),
    Member(Box<MemberListMember>),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MemberListMember {
    #[serde(flatten)]
    pub member: Member,
    pub presence: Option<Presence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "UPPERCASE")]
pub enum MemberListOp {
    Sync { range: [u32; 2], items: Vec<MemberListItem> },
    Insert { index: u32, item: MemberListItem },
    Update { index: u32, item: MemberListItem },
    Delete { index: u32 },
    Invalidate { range: [u32; 2] },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MemberListUpdate {
    pub guild_id: Snowflake,
    /// List id: `"everyone"` or a hash of the channel's permission overwrites.
    pub id: String,
    pub member_count: u32,
    pub online_count: u32,
    pub groups: Vec<MemberListGroup>,
    pub ops: Vec<MemberListOp>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RelationshipRemove {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: RelationshipType,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsProtoUpdate {
    pub settings: SettingsProtoBlob,
    pub partial: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsProtoBlob {
    #[serde(rename = "type")]
    pub kind: u8,
    pub proto: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecipientEvent {
    pub channel_id: Snowflake,
    pub user: User,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadListSync {
    pub guild_id: Snowflake,
    pub channel_ids: Vec<Snowflake>,
    pub threads: Vec<Channel>,
}

/// A parsed dispatch event. Unknown or unmodelled events are kept as [`Event::Other`]
/// so plugins and API consumers still receive them verbatim.
#[derive(Debug, Clone)]
pub enum Event {
    Ready(Box<Ready>),
    ReadySupplemental(Box<ReadySupplemental>),
    Resumed,
    MessageCreate(Box<Message>),
    /// Partial message object (only changed fields + ids).
    MessageUpdate(Value),
    MessageDelete(MessageDelete),
    MessageDeleteBulk(MessageDeleteBulk),
    ReactionAdd(ReactionEvent),
    ReactionAddMany(ReactionAddMany),
    ReactionRemove(ReactionEvent),
    ReactionRemoveAll(ReactionRemoveAll),
    ReactionRemoveEmoji(ReactionRemoveAll),
    MessageAck(MessageAck),
    TypingStart(TypingStart),
    ChannelCreate(Box<Channel>),
    ChannelUpdate(Box<Channel>),
    ChannelDelete(Box<Channel>),
    ChannelPinsUpdate(ChannelPinsUpdate),
    ChannelRecipientAdd(RecipientEvent),
    ChannelRecipientRemove(RecipientEvent),
    ThreadCreate(Box<Channel>),
    ThreadUpdate(Box<Channel>),
    ThreadDelete(Box<Channel>),
    ThreadListSync(ThreadListSync),
    GuildCreate(Box<Guild>),
    /// Top-level guild properties (no channels/roles).
    GuildUpdate(Box<Value>),
    GuildDelete(UnavailableGuild),
    GuildRoleCreate(RoleEvent),
    GuildRoleUpdate(RoleEvent),
    GuildRoleDelete(RoleDelete),
    GuildEmojisUpdate(EmojisUpdate),
    GuildMemberAdd(Box<Member>),
    GuildMemberUpdate(Box<Member>),
    GuildMemberRemove(MemberRemove),
    GuildMembersChunk(Box<MembersChunk>),
    GuildMemberListUpdate(Box<MemberListUpdate>),
    PresenceUpdate(Box<Presence>),
    RelationshipAdd(Box<Relationship>),
    RelationshipUpdate(Box<Relationship>),
    RelationshipRemove(RelationshipRemove),
    UserUpdate(Box<CurrentUser>),
    UserSettingsProtoUpdate(SettingsProtoUpdate),
    UserGuildSettingsUpdate(Box<UserGuildSettings>),
    UserNoteUpdate {
        id: Snowflake,
        note: String,
    },
    VoiceStateUpdate(Box<VoiceState>),
    VoiceServerUpdate(Value),
    SessionsReplace(Value),
    Other {
        name: String,
        data: Value,
    },
}

impl Event {
    /// Parses a dispatch by event name, borrowing the JSON so the caller can keep
    /// the raw payload. Parse failures degrade to [`Event::Other`] instead of
    /// erroring so a schema change in one event cannot kill the session.
    pub fn parse(name: &str, data: &Value) -> (Event, Option<serde_json::Error>) {
        macro_rules! p {
            ($variant:ident) => {
                match Deserialize::deserialize(data) {
                    Ok(v) => (Event::$variant(v), None),
                    Err(e) => (Event::Other { name: name.to_string(), data: data.clone() }, Some(e)),
                }
            };
        }
        match name {
            "READY" => p!(Ready),
            "READY_SUPPLEMENTAL" => p!(ReadySupplemental),
            "RESUMED" => (Event::Resumed, None),
            "MESSAGE_CREATE" => p!(MessageCreate),
            "MESSAGE_UPDATE" => (Event::MessageUpdate(data.clone()), None),
            "MESSAGE_DELETE" => p!(MessageDelete),
            "MESSAGE_DELETE_BULK" => p!(MessageDeleteBulk),
            "MESSAGE_REACTION_ADD" => p!(ReactionAdd),
            "MESSAGE_REACTION_ADD_MANY" => p!(ReactionAddMany),
            "MESSAGE_REACTION_REMOVE" => p!(ReactionRemove),
            "MESSAGE_REACTION_REMOVE_ALL" => p!(ReactionRemoveAll),
            "MESSAGE_REACTION_REMOVE_EMOJI" => p!(ReactionRemoveEmoji),
            "MESSAGE_ACK" => p!(MessageAck),
            "TYPING_START" => p!(TypingStart),
            "CHANNEL_CREATE" => p!(ChannelCreate),
            "CHANNEL_UPDATE" => p!(ChannelUpdate),
            "CHANNEL_DELETE" => p!(ChannelDelete),
            "CHANNEL_PINS_UPDATE" => p!(ChannelPinsUpdate),
            "CHANNEL_RECIPIENT_ADD" => p!(ChannelRecipientAdd),
            "CHANNEL_RECIPIENT_REMOVE" => p!(ChannelRecipientRemove),
            "THREAD_CREATE" => p!(ThreadCreate),
            "THREAD_UPDATE" => p!(ThreadUpdate),
            "THREAD_DELETE" => p!(ThreadDelete),
            "THREAD_LIST_SYNC" => p!(ThreadListSync),
            "GUILD_CREATE" => p!(GuildCreate),
            "GUILD_UPDATE" => (Event::GuildUpdate(Box::new(data.clone())), None),
            "GUILD_DELETE" => p!(GuildDelete),
            "GUILD_ROLE_CREATE" => p!(GuildRoleCreate),
            "GUILD_ROLE_UPDATE" => p!(GuildRoleUpdate),
            "GUILD_ROLE_DELETE" => p!(GuildRoleDelete),
            "GUILD_EMOJIS_UPDATE" => p!(GuildEmojisUpdate),
            "GUILD_MEMBER_ADD" => p!(GuildMemberAdd),
            "GUILD_MEMBER_UPDATE" => p!(GuildMemberUpdate),
            "GUILD_MEMBER_REMOVE" => p!(GuildMemberRemove),
            "GUILD_MEMBERS_CHUNK" => p!(GuildMembersChunk),
            "GUILD_MEMBER_LIST_UPDATE" => p!(GuildMemberListUpdate),
            "PRESENCE_UPDATE" => p!(PresenceUpdate),
            "RELATIONSHIP_ADD" => p!(RelationshipAdd),
            "RELATIONSHIP_UPDATE" => p!(RelationshipUpdate),
            "RELATIONSHIP_REMOVE" => p!(RelationshipRemove),
            "USER_UPDATE" => p!(UserUpdate),
            "USER_SETTINGS_PROTO_UPDATE" => p!(UserSettingsProtoUpdate),
            "USER_GUILD_SETTINGS_UPDATE" => p!(UserGuildSettingsUpdate),
            "USER_NOTE_UPDATE" => {
                let id = data.get("id").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
                let note = data.get("note").and_then(Value::as_str).unwrap_or_default().to_string();
                (Event::UserNoteUpdate { id, note }, None)
            }
            "VOICE_STATE_UPDATE" => p!(VoiceStateUpdate),
            "VOICE_SERVER_UPDATE" => (Event::VoiceServerUpdate(data.clone()), None),
            "SESSIONS_REPLACE" => (Event::SessionsReplace(data.clone()), None),
            _ => (Event::Other { name: name.to_string(), data: data.clone() }, None),
        }
    }

    /// The gateway event name (e.g. `MESSAGE_CREATE`).
    pub fn name(&self) -> &str {
        match self {
            Event::Ready(_) => "READY",
            Event::ReadySupplemental(_) => "READY_SUPPLEMENTAL",
            Event::Resumed => "RESUMED",
            Event::MessageCreate(_) => "MESSAGE_CREATE",
            Event::MessageUpdate(_) => "MESSAGE_UPDATE",
            Event::MessageDelete(_) => "MESSAGE_DELETE",
            Event::MessageDeleteBulk(_) => "MESSAGE_DELETE_BULK",
            Event::ReactionAdd(_) => "MESSAGE_REACTION_ADD",
            Event::ReactionAddMany(_) => "MESSAGE_REACTION_ADD_MANY",
            Event::ReactionRemove(_) => "MESSAGE_REACTION_REMOVE",
            Event::ReactionRemoveAll(_) => "MESSAGE_REACTION_REMOVE_ALL",
            Event::ReactionRemoveEmoji(_) => "MESSAGE_REACTION_REMOVE_EMOJI",
            Event::MessageAck(_) => "MESSAGE_ACK",
            Event::TypingStart(_) => "TYPING_START",
            Event::ChannelCreate(_) => "CHANNEL_CREATE",
            Event::ChannelUpdate(_) => "CHANNEL_UPDATE",
            Event::ChannelDelete(_) => "CHANNEL_DELETE",
            Event::ChannelPinsUpdate(_) => "CHANNEL_PINS_UPDATE",
            Event::ChannelRecipientAdd(_) => "CHANNEL_RECIPIENT_ADD",
            Event::ChannelRecipientRemove(_) => "CHANNEL_RECIPIENT_REMOVE",
            Event::ThreadCreate(_) => "THREAD_CREATE",
            Event::ThreadUpdate(_) => "THREAD_UPDATE",
            Event::ThreadDelete(_) => "THREAD_DELETE",
            Event::ThreadListSync(_) => "THREAD_LIST_SYNC",
            Event::GuildCreate(_) => "GUILD_CREATE",
            Event::GuildUpdate(_) => "GUILD_UPDATE",
            Event::GuildDelete(_) => "GUILD_DELETE",
            Event::GuildRoleCreate(_) => "GUILD_ROLE_CREATE",
            Event::GuildRoleUpdate(_) => "GUILD_ROLE_UPDATE",
            Event::GuildRoleDelete(_) => "GUILD_ROLE_DELETE",
            Event::GuildEmojisUpdate(_) => "GUILD_EMOJIS_UPDATE",
            Event::GuildMemberAdd(_) => "GUILD_MEMBER_ADD",
            Event::GuildMemberUpdate(_) => "GUILD_MEMBER_UPDATE",
            Event::GuildMemberRemove(_) => "GUILD_MEMBER_REMOVE",
            Event::GuildMembersChunk(_) => "GUILD_MEMBERS_CHUNK",
            Event::GuildMemberListUpdate(_) => "GUILD_MEMBER_LIST_UPDATE",
            Event::PresenceUpdate(_) => "PRESENCE_UPDATE",
            Event::RelationshipAdd(_) => "RELATIONSHIP_ADD",
            Event::RelationshipUpdate(_) => "RELATIONSHIP_UPDATE",
            Event::RelationshipRemove(_) => "RELATIONSHIP_REMOVE",
            Event::UserUpdate(_) => "USER_UPDATE",
            Event::UserSettingsProtoUpdate(_) => "USER_SETTINGS_PROTO_UPDATE",
            Event::UserGuildSettingsUpdate(_) => "USER_GUILD_SETTINGS_UPDATE",
            Event::UserNoteUpdate { .. } => "USER_NOTE_UPDATE",
            Event::VoiceStateUpdate(_) => "VOICE_STATE_UPDATE",
            Event::VoiceServerUpdate(_) => "VOICE_SERVER_UPDATE",
            Event::SessionsReplace(_) => "SESSIONS_REPLACE",
            Event::Other { name, .. } => name,
        }
    }
}
