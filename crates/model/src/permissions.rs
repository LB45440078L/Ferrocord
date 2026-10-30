//! Permission bitfield and the standard overwrite resolution algorithm.

use std::fmt;

use bitflags::bitflags;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Channel, Guild, OverwriteType, Snowflake};

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Permissions: u64 {
        const CREATE_INSTANT_INVITE = 1 << 0;
        const KICK_MEMBERS = 1 << 1;
        const BAN_MEMBERS = 1 << 2;
        const ADMINISTRATOR = 1 << 3;
        const MANAGE_CHANNELS = 1 << 4;
        const MANAGE_GUILD = 1 << 5;
        const ADD_REACTIONS = 1 << 6;
        const VIEW_AUDIT_LOG = 1 << 7;
        const PRIORITY_SPEAKER = 1 << 8;
        const STREAM = 1 << 9;
        const VIEW_CHANNEL = 1 << 10;
        const SEND_MESSAGES = 1 << 11;
        const SEND_TTS_MESSAGES = 1 << 12;
        const MANAGE_MESSAGES = 1 << 13;
        const EMBED_LINKS = 1 << 14;
        const ATTACH_FILES = 1 << 15;
        const READ_MESSAGE_HISTORY = 1 << 16;
        const MENTION_EVERYONE = 1 << 17;
        const USE_EXTERNAL_EMOJIS = 1 << 18;
        const VIEW_GUILD_INSIGHTS = 1 << 19;
        const CONNECT = 1 << 20;
        const SPEAK = 1 << 21;
        const MUTE_MEMBERS = 1 << 22;
        const DEAFEN_MEMBERS = 1 << 23;
        const MOVE_MEMBERS = 1 << 24;
        const USE_VAD = 1 << 25;
        const CHANGE_NICKNAME = 1 << 26;
        const MANAGE_NICKNAMES = 1 << 27;
        const MANAGE_ROLES = 1 << 28;
        const MANAGE_WEBHOOKS = 1 << 29;
        const MANAGE_GUILD_EXPRESSIONS = 1 << 30;
        const USE_APPLICATION_COMMANDS = 1 << 31;
        const REQUEST_TO_SPEAK = 1 << 32;
        const MANAGE_EVENTS = 1 << 33;
        const MANAGE_THREADS = 1 << 34;
        const CREATE_PUBLIC_THREADS = 1 << 35;
        const CREATE_PRIVATE_THREADS = 1 << 36;
        const USE_EXTERNAL_STICKERS = 1 << 37;
        const SEND_MESSAGES_IN_THREADS = 1 << 38;
        const USE_EMBEDDED_ACTIVITIES = 1 << 39;
        const MODERATE_MEMBERS = 1 << 40;
        const VIEW_CREATOR_MONETIZATION_ANALYTICS = 1 << 41;
        const USE_SOUNDBOARD = 1 << 42;
        const CREATE_GUILD_EXPRESSIONS = 1 << 43;
        const CREATE_EVENTS = 1 << 44;
        const USE_EXTERNAL_SOUNDS = 1 << 45;
        const SEND_VOICE_MESSAGES = 1 << 46;
        const SEND_POLLS = 1 << 49;
        const USE_EXTERNAL_APPS = 1 << 50;
        const PIN_MESSAGES = 1 << 51;
        const BYPASS_SLOWMODE = 1 << 52;
    }
}

impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.bits())
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Permissions;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("permission bits as string or integer")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Permissions, E> {
                Ok(Permissions::from_bits_retain(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Permissions, E> {
                Ok(Permissions::from_bits_retain(v as u64))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Permissions, E> {
                v.parse::<u64>().map(Permissions::from_bits_retain).map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

/// Computes guild-level permissions for a member with the given role IDs.
pub fn base_permissions(guild: &Guild, user_id: Snowflake, member_roles: &[Snowflake]) -> Permissions {
    if guild.props.owner_id == user_id {
        return Permissions::all();
    }
    let mut perms = guild.everyone_role().map(|r| r.permissions).unwrap_or_default();
    for role in &guild.roles {
        if member_roles.contains(&role.id) {
            perms |= role.permissions;
        }
    }
    if perms.contains(Permissions::ADMINISTRATOR) {
        return Permissions::all();
    }
    perms
}

/// Applies channel overwrites on top of base permissions (everyone → roles → member).
pub fn channel_permissions(
    guild: &Guild,
    channel: &Channel,
    user_id: Snowflake,
    member_roles: &[Snowflake],
) -> Permissions {
    let base = base_permissions(guild, user_id, member_roles);
    if base.contains(Permissions::ADMINISTRATOR) || base == Permissions::all() {
        return Permissions::all();
    }
    let mut perms = base;
    let ow = &channel.permission_overwrites;
    if let Some(e) = ow.iter().find(|o| o.id == guild.id) {
        perms &= !e.deny;
        perms |= e.allow;
    }
    let mut allow = Permissions::empty();
    let mut deny = Permissions::empty();
    for o in ow.iter().filter(|o| o.kind == OverwriteType::Role && member_roles.contains(&o.id)) {
        allow |= o.allow;
        deny |= o.deny;
    }
    perms &= !deny;
    perms |= allow;
    if let Some(m) = ow.iter().find(|o| o.kind == OverwriteType::Member && o.id == user_id) {
        perms &= !m.deny;
        perms |= m.allow;
    }
    // Without VIEW_CHANNEL nothing else matters; without SEND_MESSAGES, sub-permissions are moot.
    if !perms.contains(Permissions::VIEW_CHANNEL) {
        return Permissions::empty();
    }
    perms
}
