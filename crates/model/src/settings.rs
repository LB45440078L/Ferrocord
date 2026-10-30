//! Subset of the `PreloadedUserSettings` protobuf (settings type 1) that the
//! client needs: status, custom status, locale, appearance and guild folders.
//!
//! Unknown fields are ignored on decode. Updates are sent as *partial* protos via
//! `PATCH /users/@me/settings-proto/1`, which the server merges, so we never need
//! to round-trip fields we do not model.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct StringValue {
    #[prost(string, tag = "1")]
    pub value: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct BoolValue {
    #[prost(bool, tag = "1")]
    pub value: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct Int64Value {
    #[prost(int64, tag = "1")]
    pub value: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct UInt64Value {
    #[prost(uint64, tag = "1")]
    pub value: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Int32Value {
    #[prost(int32, tag = "1")]
    pub value: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct CustomStatus {
    #[prost(string, tag = "1")]
    pub text: String,
    #[prost(fixed64, tag = "2")]
    pub emoji_id: u64,
    #[prost(string, tag = "3")]
    pub emoji_name: String,
    #[prost(fixed64, tag = "4")]
    pub expires_at_ms: u64,
    #[prost(fixed64, tag = "5")]
    pub created_at_ms: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct StatusSettings {
    #[prost(message, optional, tag = "1")]
    pub status: Option<StringValue>,
    #[prost(message, optional, tag = "2")]
    pub custom_status: Option<CustomStatus>,
    #[prost(message, optional, tag = "3")]
    pub show_current_game: Option<BoolValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LocalizationSettings {
    #[prost(message, optional, tag = "1")]
    pub locale: Option<StringValue>,
    #[prost(message, optional, tag = "2")]
    pub timezone_offset: Option<Int32Value>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppearanceSettings {
    #[prost(int32, tag = "1")]
    pub theme: i32,
    #[prost(bool, tag = "2")]
    pub developer_mode: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct GuildFolder {
    #[prost(fixed64, repeated, tag = "1")]
    pub guild_ids: Vec<u64>,
    #[prost(message, optional, tag = "2")]
    pub id: Option<Int64Value>,
    #[prost(message, optional, tag = "3")]
    pub name: Option<StringValue>,
    #[prost(message, optional, tag = "4")]
    pub color: Option<UInt64Value>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GuildFolders {
    #[prost(message, repeated, tag = "1")]
    pub folders: Vec<GuildFolder>,
    #[prost(fixed64, repeated, tag = "2")]
    pub guild_positions: Vec<u64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PreloadedUserSettings {
    #[prost(message, optional, tag = "11")]
    pub status: Option<StatusSettings>,
    #[prost(message, optional, tag = "12")]
    pub localization: Option<LocalizationSettings>,
    #[prost(message, optional, tag = "13")]
    pub appearance: Option<AppearanceSettings>,
    #[prost(message, optional, tag = "14")]
    pub guild_folders: Option<GuildFolders>,
}

impl PreloadedUserSettings {
    pub fn from_base64(s: &str) -> Option<Self> {
        let bytes = STANDARD.decode(s).ok()?;
        Self::decode(bytes.as_slice()).ok()
    }

    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.encode_to_vec())
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_ref()?.status.as_ref().map(|s| s.value.as_str())
    }

    pub fn custom_status(&self) -> Option<&CustomStatus> {
        self.status.as_ref()?.custom_status.as_ref()
    }

    /// Merges a partial update (as received in `USER_SETTINGS_PROTO_UPDATE`).
    pub fn merge_partial(&mut self, other: PreloadedUserSettings) {
        if let Some(s) = other.status {
            let cur = self.status.get_or_insert_with(Default::default);
            if s.status.is_some() {
                cur.status = s.status;
            }
            if s.custom_status.is_some() {
                cur.custom_status = s.custom_status;
            }
            if s.show_current_game.is_some() {
                cur.show_current_game = s.show_current_game;
            }
        }
        if other.localization.is_some() {
            self.localization = other.localization;
        }
        if other.appearance.is_some() {
            self.appearance = other.appearance;
        }
        if other.guild_folders.is_some() {
            self.guild_folders = other.guild_folders;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_status() {
        let s = PreloadedUserSettings {
            status: Some(StatusSettings {
                status: Some(StringValue { value: "dnd".into() }),
                custom_status: None,
                show_current_game: None,
            }),
            ..Default::default()
        };
        let b = s.to_base64();
        let d = PreloadedUserSettings::from_base64(&b).unwrap();
        assert_eq!(d.status(), Some("dnd"));
    }
}
