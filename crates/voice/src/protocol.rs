//! Voice gateway wire protocol (v9), as spoken by the official client.
//!
//! JSON frames are `{op, d, seq?}`. DAVE MLS messages travel as binary frames:
//! server → client `[seq: u16 BE][op: u8][payload]`, client → server `[op: u8][payload]`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VOICE_GATEWAY_VERSION: u8 = 9;

pub mod op {
    pub const IDENTIFY: u8 = 0;
    pub const SELECT_PROTOCOL: u8 = 1;
    pub const READY: u8 = 2;
    pub const HEARTBEAT: u8 = 3;
    pub const SESSION_DESCRIPTION: u8 = 4;
    pub const SPEAKING: u8 = 5;
    pub const HEARTBEAT_ACK: u8 = 6;
    pub const RESUME: u8 = 7;
    pub const HELLO: u8 = 8;
    pub const RESUMED: u8 = 9;
    pub const CLIENTS_CONNECT: u8 = 11;
    pub const VIDEO: u8 = 12;
    pub const CLIENT_DISCONNECT: u8 = 13;
    pub const SESSION_UPDATE: u8 = 14;
    pub const MEDIA_SINK_WANTS: u8 = 15;
    pub const VOICE_BACKEND_VERSION: u8 = 16;
    pub const FLAGS: u8 = 18;
    pub const PLATFORM: u8 = 20;
    pub const DAVE_PREPARE_TRANSITION: u8 = 21;
    pub const DAVE_EXECUTE_TRANSITION: u8 = 22;
    pub const DAVE_TRANSITION_READY: u8 = 23;
    pub const DAVE_PREPARE_EPOCH: u8 = 24;
    pub const DAVE_MLS_EXTERNAL_SENDER: u8 = 25;
    pub const DAVE_MLS_KEY_PACKAGE: u8 = 26;
    pub const DAVE_MLS_PROPOSALS: u8 = 27;
    pub const DAVE_MLS_COMMIT_WELCOME: u8 = 28;
    pub const DAVE_MLS_ANNOUNCE_COMMIT_TRANSITION: u8 = 29;
    pub const DAVE_MLS_WELCOME: u8 = 30;
    pub const DAVE_MLS_INVALID_COMMIT_WELCOME: u8 = 31;
}

/// Close codes. 4006 (session no longer valid), 4014 (disconnected/kicked) and 4004
/// (auth failed) are terminal; most others can be resumed.
pub mod close {
    pub const UNKNOWN_OPCODE: u16 = 4001;
    pub const AUTH_FAILED: u16 = 4004;
    pub const SESSION_INVALID: u16 = 4006;
    pub const SESSION_TIMEOUT: u16 = 4009;
    pub const SERVER_NOT_FOUND: u16 = 4011;
    pub const UNKNOWN_PROTOCOL: u16 = 4012;
    pub const DISCONNECTED: u16 = 4014;
    pub const SERVER_CRASHED: u16 = 4015;
    pub const UNKNOWN_ENCRYPTION_MODE: u16 = 4016;
    pub const E2EE_REQUIRED: u16 = 4017;
    pub const DAVE_INVALID_MESSAGE: u16 = 4805;

    pub fn is_fatal(code: u16) -> bool {
        matches!(code, AUTH_FAILED | SESSION_INVALID | SERVER_NOT_FOUND | UNKNOWN_PROTOCOL | DISCONNECTED | UNKNOWN_ENCRYPTION_MODE | E2EE_REQUIRED)
    }
}

/// Speaking flags (op 5).
pub mod speaking {
    pub const NONE: u32 = 0;
    pub const MICROPHONE: u32 = 1 << 0;
    pub const SOUNDSHARE: u32 = 1 << 1;
    pub const PRIORITY: u32 = 1 << 2;
}

/// Opus payload type advertised by the official client.
pub const OPUS_PAYLOAD_TYPE: u8 = 120;
/// H.264 is listed first among video codecs, so it gets 101 (RTX 102).
pub const H264_PAYLOAD_TYPE: u8 = 101;
pub const H264_RTX_PAYLOAD_TYPE: u8 = 102;

/// Encryption modes we implement, in order of preference.
pub const SUPPORTED_MODES: &[&str] = &["aead_aes256_gcm_rtpsize", "aead_xchacha20_poly1305_rtpsize"];

/// Highest DAVE protocol version supported by the `davey` implementation.
pub const MAX_DAVE_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub op: u8,
    #[serde(default)]
    pub d: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StreamDescriptor {
    #[serde(rename = "type")]
    pub kind: String,
    pub rid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssrc: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtx_ssrc: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bitrate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_framerate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_resolution: Option<MaxResolution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct MaxResolution {
    #[serde(rename = "type")]
    pub kind: String,
    pub width: u32,
    pub height: u32,
}

impl StreamDescriptor {
    /// The single camera stream the desktop client requests in IDENTIFY.
    pub fn camera() -> Self {
        Self { kind: "video".into(), rid: "100".into(), quality: Some(100), ..Default::default() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Identify {
    pub server_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub session_id: String,
    pub token: String,
    pub max_dave_protocol_version: u16,
    pub video: bool,
    pub streams: Vec<StreamDescriptor>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Resume {
    pub token: String,
    pub session_id: String,
    pub server_id: String,
    pub channel_id: String,
    pub seq_ack: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hello {
    pub heartbeat_interval: f64,
    #[serde(default)]
    pub v: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ready {
    pub ssrc: u32,
    pub ip: String,
    pub port: u16,
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub streams: Vec<StreamDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Codec {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub priority: u32,
    pub payload_type: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtx_payload_type: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode: Option<bool>,
}

/// Codec list in the official client's format: Opus, then video codecs we can handle.
pub fn codecs(video: bool) -> Vec<Codec> {
    let mut v = vec![Codec {
        name: "opus".into(),
        kind: "audio".into(),
        priority: 1000,
        payload_type: OPUS_PAYLOAD_TYPE,
        rtx_payload_type: None,
        encode: None,
        decode: None,
    }];
    if video {
        v.push(Codec {
            name: "H264".into(),
            kind: "video".into(),
            priority: 1000,
            payload_type: H264_PAYLOAD_TYPE,
            rtx_payload_type: Some(H264_RTX_PAYLOAD_TYPE),
            encode: Some(true),
            decode: Some(true),
        });
    }
    v
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectProtocolData {
    pub address: String,
    pub port: u16,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectProtocol {
    pub protocol: String,
    pub data: SelectProtocolData,
    pub address: String,
    pub port: u16,
    pub mode: String,
    pub codecs: Vec<Codec>,
    pub rtc_connection_id: String,
    pub experiments: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionDescription {
    pub mode: String,
    pub secret_key: Vec<u8>,
    #[serde(default)]
    pub audio_codec: Option<String>,
    #[serde(default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub media_session_id: Option<String>,
    #[serde(default)]
    pub dave_protocol_version: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Speaking {
    pub speaking: u32,
    #[serde(default)]
    pub delay: u32,
    pub ssrc: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// Op 12 in both directions: announces SSRCs for a user's camera.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Video {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub audio_ssrc: u32,
    pub video_ssrc: u32,
    #[serde(default)]
    pub rtx_ssrc: u32,
    #[serde(default)]
    pub streams: Vec<StreamDescriptor>,
}

/// Parses a binary frame from the server (`[seq u16][op u8][payload]`).
pub fn parse_binary(data: &[u8]) -> Option<(u16, u8, &[u8])> {
    if data.len() < 3 {
        return None;
    }
    Some((u16::from_be_bytes([data[0], data[1]]), data[2], &data[3..]))
}

/// Builds a client → server binary frame (`[op u8][payload]`).
pub fn build_binary(op: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 1);
    v.push(op);
    v.extend_from_slice(payload);
    v
}

/// Extracts a snowflake that may be encoded as a string or a number.
pub fn snowflake(v: &Value) -> Option<u64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_framing() {
        let frame = [0x01, 0x02, op::DAVE_MLS_PROPOSALS, 9, 8, 7];
        assert_eq!(parse_binary(&frame), Some((0x0102, op::DAVE_MLS_PROPOSALS, &[9u8, 8, 7][..])));
        assert_eq!(build_binary(op::DAVE_MLS_KEY_PACKAGE, &[1, 2]), vec![26, 1, 2]);
        assert!(parse_binary(&[1, 2]).is_none());
    }

    #[test]
    fn identify_matches_official_shape() {
        let id = Identify {
            server_id: "1".into(),
            channel_id: "2".into(),
            user_id: "3".into(),
            session_id: "s".into(),
            token: "t".into(),
            max_dave_protocol_version: 1,
            video: true,
            streams: vec![StreamDescriptor::camera()],
        };
        let v = serde_json::to_value(&id).unwrap();
        assert_eq!(v["streams"][0], serde_json::json!({"type": "video", "rid": "100", "quality": 100}));
        assert_eq!(v["max_dave_protocol_version"], 1);
    }

    #[test]
    fn session_description_parses() {
        let sd: SessionDescription = serde_json::from_value(serde_json::json!({
            "mode": "aead_aes256_gcm_rtpsize", "secret_key": [1, 2, 3], "audio_codec": "opus",
            "video_codec": "H264", "dave_protocol_version": 1
        }))
        .unwrap();
        assert_eq!(sd.dave_protocol_version, 1);
        assert_eq!(sd.secret_key, vec![1, 2, 3]);
    }
}
