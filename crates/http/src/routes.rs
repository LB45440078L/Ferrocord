//! Typed wrappers over the REST endpoints used by the client.

use ferrocord_model::*;
use serde_json::{Value, json};

use crate::client::{Body, FilePart, Http, Request};
use crate::error::HttpError;

type R<T> = Result<T, HttpError>;

/// Pagination anchor for message history.
#[derive(Debug, Clone, Copy)]
pub enum MessageAnchor {
    Latest,
    Before(Snowflake),
    After(Snowflake),
    Around(Snowflake),
}

/// Parameters for `GET /guilds/{id}/messages/search` and the channel/DM equivalent.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SearchQuery {
    pub content: Option<String>,
    pub author_id: Vec<Snowflake>,
    pub mentions: Vec<Snowflake>,
    pub channel_id: Vec<Snowflake>,
    pub has: Vec<String>,
    pub min_id: Option<Snowflake>,
    pub max_id: Option<Snowflake>,
    pub pinned: Option<bool>,
    pub offset: u32,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub include_nsfw: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SearchResults {
    pub total_results: u64,
    /// Each hit is wrapped in an array (legacy context format); index 0 is the hit.
    pub messages: Vec<Vec<Message>>,
    pub doing_deep_historical_index: Option<bool>,
}

impl Http {
    // ----- users -------------------------------------------------------------

    pub async fn current_user(&self) -> R<CurrentUser> {
        self.send_as(Request::get("/users/@me")).await
    }

    pub async fn user_profile(&self, user: Snowflake, guild: Option<Snowflake>) -> R<UserProfile> {
        self.send_as(
            Request::get(format!("/users/{user}/profile"))
                .query("with_mutual_guilds", "true")
                .query("with_mutual_friends", "true")
                .query_opt("guild_id", guild),
        )
        .await
    }

    pub async fn set_note(&self, user: Snowflake, note: &str) -> R<()> {
        self.send_unit(Request::put(format!("/users/@me/notes/{user}")).json(json!({ "note": note }))).await
    }

    pub async fn update_settings_proto(&self, settings_b64: &str) -> R<Value> {
        self.send(Request::patch("/users/@me/settings-proto/1").json(json!({ "settings": settings_b64 }))).await
    }

    pub async fn update_guild_settings(&self, guild: Option<Snowflake>, body: Value) -> R<Value> {
        let id = guild.map(|g| g.to_string()).unwrap_or_else(|| "%40me".into());
        self.send(Request::patch(format!("/users/@me/guilds/{id}/settings")).json(body)).await
    }

    // ----- relationships ------------------------------------------------------

    pub async fn relationships(&self) -> R<Vec<Relationship>> {
        self.send_as(Request::get("/users/@me/relationships")).await
    }

    /// Sends a friend request by username (new unique usernames have no discriminator).
    pub async fn send_friend_request(&self, username: &str) -> R<()> {
        let (name, disc) = match username.rsplit_once('#') {
            Some((n, d)) if d.len() == 4 && d.bytes().all(|b| b.is_ascii_digit()) => (n, Some(d)),
            _ => (username, None),
        };
        self.send_unit(
            Request::post("/users/@me/relationships")
                .header("X-Context-Properties", context_properties("Add Friend"))
                .json(json!({ "username": name, "discriminator": disc })),
        )
        .await
    }

    /// Accepts a request or (re)sends one to a known user id.
    pub async fn add_relationship(&self, user: Snowflake) -> R<()> {
        self.send_unit(
            Request::put(format!("/users/@me/relationships/{user}")).header("X-Context-Properties", context_properties("ContextMenu")).json(json!({})),
        )
        .await
    }

    pub async fn block_user(&self, user: Snowflake) -> R<()> {
        self.send_unit(Request::put(format!("/users/@me/relationships/{user}")).json(json!({ "type": 2 }))).await
    }

    pub async fn remove_relationship(&self, user: Snowflake) -> R<()> {
        self.send_unit(Request::delete(format!("/users/@me/relationships/{user}"))).await
    }

    // ----- channels -----------------------------------------------------------

    pub async fn channel(&self, id: Snowflake) -> R<Channel> {
        self.send_as(Request::get(format!("/channels/{id}"))).await
    }

    pub async fn create_dm(&self, recipients: &[Snowflake]) -> R<Channel> {
        self.send_as(Request::post("/users/@me/channels").json(json!({ "recipients": recipients }))).await
    }

    pub async fn close_channel(&self, id: Snowflake) -> R<()> {
        self.send_unit(Request::delete(format!("/channels/{id}")).query("silent", "false")).await
    }

    pub async fn messages(&self, channel: Snowflake, anchor: MessageAnchor, limit: u8) -> R<Vec<Message>> {
        let mut req = Request::get(format!("/channels/{channel}/messages")).query("limit", limit.clamp(1, 100));
        req = match anchor {
            MessageAnchor::Latest => req,
            MessageAnchor::Before(id) => req.query("before", id),
            MessageAnchor::After(id) => req.query("after", id),
            MessageAnchor::Around(id) => req.query("around", id),
        };
        self.send_as(req).await
    }

    pub async fn send_message(&self, channel: Snowflake, msg: &CreateMessage, files: Vec<FilePart>) -> R<Message> {
        let mut req = Request::post(format!("/channels/{channel}/messages"));
        if files.is_empty() {
            req = req.json(msg);
        } else {
            let mut payload = serde_json::to_value(msg)?;
            let atts: Vec<Value> = files.iter().enumerate().map(|(i, f)| json!({ "id": i.to_string(), "filename": f.filename })).collect();
            payload["attachments"] = Value::Array(atts);
            req.body = Some(Body::Multipart { payload, files });
        }
        self.send_as(req).await
    }

    pub async fn edit_message(&self, channel: Snowflake, message: Snowflake, content: &str) -> R<Message> {
        self.send_as(Request::patch(format!("/channels/{channel}/messages/{message}")).json(json!({ "content": content }))).await
    }

    pub async fn delete_message(&self, channel: Snowflake, message: Snowflake) -> R<()> {
        self.send_unit(Request::delete(format!("/channels/{channel}/messages/{message}"))).await
    }

    pub async fn suppress_embeds(&self, channel: Snowflake, message: Snowflake, flags: u64) -> R<()> {
        self.send_unit(Request::patch(format!("/channels/{channel}/messages/{message}")).json(json!({ "flags": flags | message_flags::SUPPRESS_EMBEDS }))).await
    }

    pub async fn trigger_typing(&self, channel: Snowflake) -> R<()> {
        self.send_unit(Request::post(format!("/channels/{channel}/typing"))).await
    }

    /// Marks a message (and everything before it) as read.
    pub async fn ack_message(&self, channel: Snowflake, message: Snowflake, manual: bool) -> R<()> {
        let mut body = json!({ "token": null });
        if manual {
            body["manual"] = json!(true);
            body["mention_count"] = json!(0);
        } else {
            body["last_viewed"] = json!(days_since_discord_epoch());
        }
        self.send_unit(Request::post(format!("/channels/{channel}/messages/{message}/ack")).json(body)).await
    }

    pub async fn ack_bulk(&self, entries: &[(Snowflake, Snowflake)]) -> R<()> {
        let read_states: Vec<Value> = entries.iter().map(|(c, m)| json!({ "channel_id": c, "message_id": m, "read_state_type": 0 })).collect();
        self.send_unit(Request::post("/read-states/ack-bulk").json(json!({ "read_states": read_states }))).await
    }

    pub async fn ack_guild(&self, guild: Snowflake) -> R<()> {
        self.send_unit(Request::post(format!("/guilds/{guild}/ack")).json(json!({}))).await
    }

    pub async fn pins(&self, channel: Snowflake) -> R<Vec<Message>> {
        let v = self.send(Request::get(format!("/channels/{channel}/messages/pins")).query("limit", 50)).await?;
        // New format: {items:[{message, pinned_at}], has_more}.
        if let Some(items) = v.get("items").and_then(Value::as_array) {
            return Ok(items.iter().filter_map(|i| serde_json::from_value(i.get("message")?.clone()).ok()).collect());
        }
        Ok(serde_json::from_value(v)?)
    }

    pub async fn pin(&self, channel: Snowflake, message: Snowflake) -> R<()> {
        self.send_unit(Request::put(format!("/channels/{channel}/messages/pins/{message}"))).await
    }

    pub async fn unpin(&self, channel: Snowflake, message: Snowflake) -> R<()> {
        self.send_unit(Request::delete(format!("/channels/{channel}/messages/pins/{message}"))).await
    }

    pub async fn add_reaction(&self, channel: Snowflake, message: Snowflake, emoji: &PartialEmoji) -> R<()> {
        let e = encode_path(&emoji.route_key());
        self.send_unit(Request::put(format!("/channels/{channel}/messages/{message}/reactions/{e}/@me")).query("location", "Message").query("type", 0)).await
    }

    pub async fn remove_reaction(&self, channel: Snowflake, message: Snowflake, emoji: &PartialEmoji, user: Option<Snowflake>) -> R<()> {
        let e = encode_path(&emoji.route_key());
        let who = user.map(|u| u.to_string()).unwrap_or_else(|| "@me".into());
        self.send_unit(
            Request::delete(format!("/channels/{channel}/messages/{message}/reactions/{e}/0/{who}")).query("location", "Message").query("burst", "false"),
        )
        .await
    }

    pub async fn reaction_users(&self, channel: Snowflake, message: Snowflake, emoji: &PartialEmoji) -> R<Vec<User>> {
        let e = encode_path(&emoji.route_key());
        self.send_as(Request::get(format!("/channels/{channel}/messages/{message}/reactions/{e}")).query("limit", 100).query("type", 0)).await
    }

    pub async fn search(&self, scope: SearchScope, q: &SearchQuery) -> R<SearchResults> {
        let path = match scope {
            SearchScope::Guild(g) => format!("/guilds/{g}/messages/search"),
            SearchScope::Channel(c) => format!("/channels/{c}/messages/search"),
        };
        let mut req = Request::get(path).query_opt("content", q.content.as_ref()).query_opt("min_id", q.min_id).query_opt("max_id", q.max_id);
        for a in &q.author_id {
            req = req.query("author_id", a);
        }
        for m in &q.mentions {
            req = req.query("mentions", m);
        }
        for c in &q.channel_id {
            req = req.query("channel_id", c);
        }
        for h in &q.has {
            req = req.query("has", h);
        }
        if let Some(p) = q.pinned {
            req = req.query("pinned", p);
        }
        if q.offset > 0 {
            req = req.query("offset", q.offset);
        }
        req = req.query_opt("sort_by", q.sort_by.as_ref()).query_opt("sort_order", q.sort_order.as_ref());
        if q.include_nsfw {
            req = req.query("include_nsfw", true);
        }
        self.send_as(req).await
    }

    pub async fn create_thread_from_message(&self, channel: Snowflake, message: Snowflake, name: &str) -> R<Channel> {
        self.send_as(Request::post(format!("/channels/{channel}/messages/{message}/threads")).json(json!({ "name": name, "auto_archive_duration": 4320 })))
            .await
    }

    /// Active posts of a forum/media channel, most recently active first.
    pub async fn forum_threads(&self, channel: Snowflake, offset: u32) -> R<Vec<Channel>> {
        let v = self
            .send(
                Request::get(format!("/channels/{channel}/threads/search"))
                    .query("archived", "false")
                    .query("sort_by", "last_message_time")
                    .query("sort_order", "desc")
                    .query("limit", 25)
                    .query("offset", offset),
            )
            .await?;
        Ok(v.get("threads").and_then(|t| serde_json::from_value(t.clone()).ok()).unwrap_or_default())
    }

    /// Rings the other members of a DM/group DM (`recipients: None` rings everyone).
    pub async fn ring(&self, channel: Snowflake, recipients: Option<&[Snowflake]>) -> R<()> {
        self.send_unit(Request::post(format!("/channels/{channel}/call/ring")).json(json!({ "recipients": recipients }))).await
    }

    /// Stops ringing (declines an incoming call) for the given users, or ourselves.
    pub async fn stop_ringing(&self, channel: Snowflake, recipients: Option<&[Snowflake]>) -> R<()> {
        self.send_unit(Request::post(format!("/channels/{channel}/call/stop-ringing")).json(json!({ "recipients": recipients }))).await
    }

    pub async fn join_thread(&self, thread: Snowflake) -> R<()> {
        self.send_unit(Request::put(format!("/channels/{thread}/thread-members/@me")).query("location", "Sidebar Overflow")).await
    }

    pub async fn leave_thread(&self, thread: Snowflake) -> R<()> {
        self.send_unit(Request::delete(format!("/channels/{thread}/thread-members/@me")).query("location", "Sidebar Overflow")).await
    }

    // ----- guilds -------------------------------------------------------------

    pub async fn guild(&self, id: Snowflake) -> R<Guild> {
        self.send_as(Request::get(format!("/guilds/{id}")).query("with_counts", true)).await
    }

    pub async fn guild_member(&self, guild: Snowflake, user: Snowflake) -> R<Member> {
        self.send_as(Request::get(format!("/guilds/{guild}/members/{user}"))).await
    }

    pub async fn leave_guild(&self, guild: Snowflake) -> R<()> {
        self.send_unit(Request::delete(format!("/users/@me/guilds/{guild}")).json(json!({ "lurking": false }))).await
    }

    pub async fn set_nickname(&self, guild: Snowflake, nick: Option<&str>) -> R<()> {
        self.send_unit(Request::patch(format!("/guilds/{guild}/members/@me")).json(json!({ "nick": nick }))).await
    }

    pub async fn kick(&self, guild: Snowflake, user: Snowflake, reason: Option<&str>) -> R<()> {
        let mut r = Request::delete(format!("/guilds/{guild}/members/{user}"));
        if let Some(reason) = reason {
            r = r.reason(reason);
        }
        self.send_unit(r).await
    }

    pub async fn ban(&self, guild: Snowflake, user: Snowflake, delete_message_seconds: u32, reason: Option<&str>) -> R<()> {
        let mut r = Request::put(format!("/guilds/{guild}/bans/{user}")).json(json!({ "delete_message_seconds": delete_message_seconds }));
        if let Some(reason) = reason {
            r = r.reason(reason);
        }
        self.send_unit(r).await
    }

    pub async fn timeout_member(&self, guild: Snowflake, user: Snowflake, until_iso: Option<&str>) -> R<()> {
        self.send_unit(Request::patch(format!("/guilds/{guild}/members/{user}")).json(json!({ "communication_disabled_until": until_iso }))).await
    }

    pub async fn create_invite(&self, channel: Snowflake, max_age: u32, max_uses: u32) -> R<Invite> {
        self.send_as(
            Request::post(format!("/channels/{channel}/invites"))
                .json(json!({ "max_age": max_age, "max_uses": max_uses, "temporary": false, "flags": 0, "validate": null, "target_type": null })),
        )
        .await
    }

    // ----- invites --------------------------------------------------------------

    pub async fn invite(&self, code: &str) -> R<Invite> {
        self.send_as(Request::get(format!("/invites/{}", encode_path(code))).query("with_counts", true).query("with_expiration", true)).await
    }

    pub async fn accept_invite(&self, code: &str) -> R<Value> {
        self.send(
            Request::post(format!("/invites/{}", encode_path(code)))
                .header("X-Context-Properties", context_properties("Markdown Link"))
                .json(json!({ "session_id": null })),
        )
        .await
    }

    // ----- misc ---------------------------------------------------------------

    pub async fn logout(&self) -> R<()> {
        self.send_unit(Request::post("/auth/logout").json(json!({ "provider": null, "voip_provider": null }))).await
    }

    pub async fn gateway_url(&self) -> R<String> {
        let v = self.send(Request::get("/gateway").unauthenticated()).await?;
        Ok(v.get("url").and_then(Value::as_str).unwrap_or("wss://gateway.discord.gg").to_string())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SearchScope {
    Guild(Snowflake),
    Channel(Snowflake),
}

/// Percent-encodes a single path segment.
pub fn encode_path(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

/// `X-Context-Properties`: base64 JSON describing the UI location of an action.
fn context_properties(location: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(json!({ "location": location }).to_string())
}

fn days_since_discord_epoch() -> u64 {
    let now = chrono_now_ms();
    now.saturating_sub(ferrocord_model::snowflake::DISCORD_EPOCH) / 86_400_000
}

fn chrono_now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
