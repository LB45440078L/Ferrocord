//! URL builders for Discord's CDN and media proxy.

use crate::Snowflake;

pub const CDN: &str = "https://cdn.discordapp.com";
pub const MEDIA_PROXY: &str = "https://media.discordapp.net";

fn ext(hash: &str) -> &'static str {
    if hash.starts_with("a_") { "gif" } else { "webp" }
}

/// Rounds to the power-of-two sizes the CDN accepts (16..=4096).
pub fn clamp_size(size: u32) -> u32 {
    size.clamp(16, 4096).next_power_of_two().min(4096)
}

pub fn user_avatar(user: Snowflake, hash: &str, size: u32) -> String {
    format!("{CDN}/avatars/{user}/{hash}.{}?size={}", ext(hash), clamp_size(size))
}

pub fn member_avatar(guild: Snowflake, user: Snowflake, hash: &str, size: u32) -> String {
    format!("{CDN}/guilds/{guild}/users/{user}/avatars/{hash}.{}?size={}", ext(hash), clamp_size(size))
}

/// Default avatars: `(id >> 22) % 6` for new usernames, `discriminator % 5` for legacy.
pub fn default_avatar(user: Snowflake, discriminator: &str) -> String {
    let idx = match discriminator.parse::<u64>() {
        Ok(d) if d != 0 => d % 5,
        _ => (user.0 >> 22) % 6,
    };
    format!("{CDN}/embed/avatars/{idx}.png")
}

pub fn guild_icon(guild: Snowflake, hash: &str, size: u32) -> String {
    format!("{CDN}/icons/{guild}/{hash}.{}?size={}", ext(hash), clamp_size(size))
}

pub fn guild_banner(guild: Snowflake, hash: &str, size: u32) -> String {
    format!("{CDN}/banners/{guild}/{hash}.{}?size={}", ext(hash), clamp_size(size))
}

pub fn user_banner(user: Snowflake, hash: &str, size: u32) -> String {
    format!("{CDN}/banners/{user}/{hash}.{}?size={}", ext(hash), clamp_size(size))
}

pub fn channel_icon(channel: Snowflake, hash: &str, size: u32) -> String {
    format!("{CDN}/channel-icons/{channel}/{hash}.webp?size={}", clamp_size(size))
}

pub fn emoji(id: Snowflake, animated: bool, size: u32) -> String {
    let e = if animated { "gif" } else { "webp" };
    format!("{CDN}/emojis/{id}.{e}?size={}&quality=lossless", clamp_size(size))
}

pub fn sticker(id: Snowflake, format_type: u8) -> String {
    match format_type {
        3 => format!("{CDN}/stickers/{id}.json"), // Lottie
        4 => format!("{MEDIA_PROXY}/stickers/{id}.gif"),
        _ => format!("{MEDIA_PROXY}/stickers/{id}.webp?size=160"),
    }
}

/// Requests a resized rendition of an attachment/embed image through the media proxy.
pub fn proxied_image(proxy_url: &str, width: u32, height: u32) -> String {
    let sep = if proxy_url.contains('?') { '&' } else { '?' };
    format!("{proxy_url}{sep}format=webp&width={width}&height={height}")
}
