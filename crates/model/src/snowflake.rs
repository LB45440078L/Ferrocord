//! Discord snowflake IDs.
//!
//! Discord encodes IDs as decimal strings on the wire (they exceed 2^53), but some
//! payloads carry them as numbers. [`Snowflake`] accepts both and always serializes
//! as a string.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Milliseconds since the Unix epoch at which Discord's snowflake epoch starts (2015-01-01).
pub const DISCORD_EPOCH: u64 = 1_420_070_400_000;

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Snowflake(pub u64);

impl Snowflake {
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// Unix timestamp in milliseconds at which this ID was minted.
    pub const fn timestamp_ms(self) -> u64 {
        (self.0 >> 22) + DISCORD_EPOCH
    }

    pub fn created_at(self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(self.timestamp_ms() as i64).unwrap_or_default()
    }

    /// Generates a nonce in snowflake format for "now", as the official client does
    /// for message nonces.
    pub fn nonce_now() -> Self {
        let now = chrono::Utc::now().timestamp_millis() as u64;
        Self((now.saturating_sub(DISCORD_EPOCH)) << 22)
    }

    /// Smallest snowflake minted at the given unix-ms time (useful for search ranges).
    pub fn from_timestamp_ms(ms: u64) -> Self {
        Self(ms.saturating_sub(DISCORD_EPOCH) << 22)
    }
}

impl fmt::Debug for Snowflake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for Snowflake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Snowflake {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl From<u64> for Snowflake {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl Serialize for Snowflake {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Snowflake {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Snowflake;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a snowflake as string or integer")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Snowflake, E> {
                Ok(Snowflake(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Snowflake, E> {
                Ok(Snowflake(v as u64))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Snowflake, E> {
                v.parse().map(Snowflake).map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

/// Deserializes an optional snowflake that may also be encoded as `""`.
pub fn opt_snowflake_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Snowflake>, D::Error> {
    let v: Option<serde_json::Value> = Option::deserialize(d)?;
    Ok(match v {
        Some(serde_json::Value::String(s)) if !s.is_empty() => s.parse().ok().map(Snowflake),
        Some(serde_json::Value::Number(n)) => n.as_u64().map(Snowflake),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s: Snowflake = serde_json::from_str("\"175928847299117063\"").unwrap();
        assert_eq!(s.0, 175928847299117063);
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"175928847299117063\"");
        let n: Snowflake = serde_json::from_str("42").unwrap();
        assert_eq!(n.0, 42);
        assert_eq!(s.timestamp_ms(), 1462015105796);
    }
}
