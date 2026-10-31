//! API operations, shared by the REST routes and WebSocket RPC methods.
//!
//! Every function returns the JSON payload of a successful call; REST handlers
//! wrap it in a response and the WebSocket layer in `{"id", "result"}`.

use ferrocord_core::Client;
use ferrocord_core::http::MessageAnchor;
use ferrocord_core::model::{AllowedMentions, CreateMessage, Message, MessageReference, PartialEmoji, Snowflake, Status};
use ferrocord_core::store::{GuildListEntry, Store};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ApiError;

pub(crate) type Res = Result<Value, ApiError>;

/// API protocol version reported by `/v1/info`.
pub const API_VERSION: u32 = 1;
const MAX_CONTENT_CHARS: usize = 4000;

pub(crate) fn parse_id(s: &str, what: &str) -> Result<Snowflake, ApiError> {
    s.trim().parse::<u64>().ok().filter(|v| *v != 0).map(Snowflake).ok_or_else(|| ApiError::bad_request(format!("invalid {what}")))
}

fn ensure_actions(client: &Client) -> Result<(), ApiError> {
    if client.config().api.allow_actions { Ok(()) } else { Err(ApiError::actions_disabled()) }
}

fn ensure_ready(client: &Client) -> Result<(), ApiError> {
    if client.store().me.is_some() { Ok(()) } else { Err(ApiError::not_ready()) }
}

// ---------------------------------------------------------------- JSON views

fn user_json(s: &Store, id: Snowflake, guild: Option<Snowflake>) -> Option<Value> {
    let u = s.user(id)?;
    Some(json!({
        "id": u.id,
        "username": u.username,
        "global_name": u.global_name,
        "display_name": s.display_name(guild, u.id),
        "discriminator": u.discriminator,
        "avatar_url": u.avatar_url(128),
        "bot": u.bot,
        "status": s.status(u.id).as_str(),
    }))
}

fn channel_json(s: &Store, id: Snowflake) -> Option<Value> {
    let c = s.channels.get(&id)?;
    let recipients: Vec<Value> = c.recipient_ids().into_iter().filter_map(|u| user_json(s, u, None)).collect();
    Some(json!({
        "id": c.id,
        "type": u8::from(c.kind),
        "name": c.name,
        "display_name": c.display_name(),
        "guild_id": c.guild_id,
        "parent_id": c.parent_id,
        "position": c.position,
        "topic": c.topic,
        "nsfw": c.nsfw,
        "last_message_id": c.last_message_id,
        "icon_url": c.icon_url(128),
        "recipients": recipients,
        "unread": s.is_unread(c.id),
        "mention_count": s.mention_count(c.id),
        "muted": s.is_muted(c.id),
    }))
}

fn guild_summary(s: &Store, id: Snowflake) -> Option<Value> {
    let g = s.guilds.get(&id)?;
    let (unread, mentions) = s.guild_unread(id);
    Some(json!({
        "id": g.id,
        "name": g.name(),
        "icon_url": g.icon_url(128),
        "unavailable": g.unavailable,
        "member_count": g.member_count,
        "unread": unread,
        "mention_count": mentions,
        "muted": s.is_guild_muted(id),
    }))
}

/// A channel visible to the user (guild channels must grant VIEW_CHANNEL).
fn visible_channel(s: &Store, id: Snowflake) -> Result<Value, ApiError> {
    let c = s.channels.get(&id).ok_or_else(|| ApiError::not_found("unknown channel"))?;
    if c.guild_id.is_some() && !s.can_view(id) {
        return Err(ApiError::not_found("unknown channel"));
    }
    channel_json(s, id).ok_or_else(|| ApiError::not_found("unknown channel"))
}

// ---------------------------------------------------------------- reads

pub(crate) fn info(client: &Client) -> Value {
    let s = client.store();
    json!({
        "name": "ferrocord",
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": API_VERSION,
        "connection": format!("{:?}", client.connection_state()),
        "latency_ms": client.latency().map(|d| d.as_millis() as u64),
        "logged_in": s.me.is_some(),
        "user_id": s.me.as_ref().map(|m| m.user.id),
        "actions_allowed": client.config().api.allow_actions,
    })
}

pub(crate) fn me(client: &Client) -> Res {
    ensure_ready(client)?;
    let s = client.store();
    let me = s.me.as_ref().ok_or_else(ApiError::not_ready)?;
    let mut v = user_json(&s, me.user.id, None).unwrap_or_else(|| json!({ "id": me.user.id, "username": me.user.username }));
    let custom = s.settings.custom_status().filter(|c| !c.text.is_empty() || !c.emoji_name.is_empty());
    v["custom_status"] = match custom {
        Some(c) => json!({ "text": c.text, "emoji_name": (!c.emoji_name.is_empty()).then_some(&c.emoji_name) }),
        None => Value::Null,
    };
    v["mfa_enabled"] = json!(me.mfa_enabled);
    v["verified"] = json!(me.verified);
    Ok(v)
}

pub(crate) fn guilds(client: &Client) -> Res {
    ensure_ready(client)?;
    let s = client.store();
    let list: Vec<Value> = s
        .guild_list()
        .into_iter()
        .flat_map(|e| match e {
            GuildListEntry::Guild(g) => vec![g],
            GuildListEntry::Folder { guilds, .. } => guilds,
        })
        .filter_map(|g| guild_summary(&s, g))
        .collect();
    Ok(Value::Array(list))
}

pub(crate) fn guild(client: &Client, id: Snowflake) -> Res {
    ensure_ready(client)?;
    let s = client.store();
    let mut v = guild_summary(&s, id).ok_or_else(|| ApiError::not_found("unknown guild"))?;
    let g = &s.guilds[&id];
    let mut roles: Vec<&ferrocord_core::model::Role> = g.roles.iter().collect();
    roles.sort_by_key(|r| std::cmp::Reverse(r.position));
    v["description"] = json!(g.props.description);
    v["owner_id"] = json!(g.props.owner_id);
    v["features"] = json!(g.props.features);
    v["premium_tier"] = json!(g.props.premium_tier);
    v["joined_at"] = json!(g.joined_at);
    v["roles"] = roles
        .into_iter()
        .map(|r| json!({ "id": r.id, "name": r.name, "color": r.color, "position": r.position, "hoist": r.hoist, "mentionable": r.mentionable }))
        .collect();
    Ok(v)
}

pub(crate) fn guild_channels(client: &Client, id: Snowflake) -> Res {
    ensure_ready(client)?;
    let s = client.store();
    if !s.guilds.contains_key(&id) {
        return Err(ApiError::not_found("unknown guild"));
    }
    let list: Vec<Value> =
        s.guild_channel_tree(id).into_iter().flat_map(|(cat, kids)| cat.into_iter().chain(kids)).filter_map(|c| channel_json(&s, c.id)).collect();
    Ok(Value::Array(list))
}

pub(crate) fn channel(client: &Client, id: Snowflake) -> Res {
    ensure_ready(client)?;
    visible_channel(&client.store(), id)
}

pub(crate) async fn messages(client: &Client, channel: Snowflake, limit: Option<u32>, before: Option<Snowflake>) -> Res {
    ensure_ready(client)?;
    visible_channel(&client.store(), channel)?;
    let limit = limit.unwrap_or(50).clamp(1, 100) as usize;
    let cached: Option<Vec<Message>> = {
        let s = client.store();
        s.messages.get(&channel).and_then(|cm| {
            let sent = cm.messages.iter().filter(|m| s.pending_state(m).is_none());
            let older: Vec<&Message> = match before {
                Some(b) => sent.filter(|m| m.id < b).collect(),
                None if cm.has_latest => sent.collect(),
                None => return None,
            };
            // Only serve from cache when it holds a full page (or the start of the channel).
            let contiguous = before.is_none_or(|b| cm.has_latest || cm.messages.iter().any(|m| m.id >= b));
            (contiguous && (older.len() >= limit || cm.reached_start)).then(|| older.iter().rev().take(limit).map(|m| (*m).clone()).collect())
        })
    };
    let page = match cached {
        Some(p) => p,
        None => {
            let anchor = before.map(MessageAnchor::Before).unwrap_or(MessageAnchor::Latest);
            let mut p = client.http().messages(channel, anchor, limit as u8).await?;
            p.sort_by_key(|m| std::cmp::Reverse(m.id));
            p
        }
    };
    Ok(serde_json::to_value(page).unwrap_or_default())
}

pub(crate) async fn user(client: &Client, id: Snowflake) -> Res {
    ensure_ready(client)?;
    if let Some(v) = user_json(&client.store(), id, None) {
        return Ok(v);
    }
    let profile = client.http().user_profile(id, None).await?;
    let u = profile.user;
    Ok(json!({
        "id": u.id,
        "username": u.username,
        "global_name": u.global_name,
        "display_name": u.display_name(),
        "discriminator": u.discriminator,
        "avatar_url": u.avatar_url(128),
        "bot": u.bot,
        "status": Status::Offline.as_str(),
    }))
}

pub(crate) fn dms(client: &Client) -> Res {
    ensure_ready(client)?;
    let s = client.store();
    Ok(Value::Array(s.sorted_private_channels().into_iter().filter_map(|c| channel_json(&s, c.id)).collect()))
}

pub(crate) fn relationships(client: &Client) -> Res {
    ensure_ready(client)?;
    let s = client.store();
    let mut list: Vec<&ferrocord_core::model::Relationship> = s.relationships.values().collect();
    list.sort_by_key(|r| (u8::from(r.kind), r.id));
    Ok(list
        .into_iter()
        .map(|r| {
            let kind = match u8::from(r.kind) {
                1 => "friend",
                2 => "blocked",
                3 => "incoming_request",
                4 => "outgoing_request",
                5 => "implicit",
                _ => "none",
            };
            json!({
                "id": r.id,
                "type": u8::from(r.kind),
                "type_name": kind,
                "nickname": r.nickname,
                "since": r.since,
                "user": user_json(&s, r.id, None),
            })
        })
        .collect())
}

// ---------------------------------------------------------------- writes

#[derive(Debug, Deserialize)]
pub(crate) struct SendMessage {
    pub content: String,
    #[serde(default)]
    pub reply_to: Option<Snowflake>,
}

pub(crate) async fn send_message(client: &Client, channel: Snowflake, body: SendMessage) -> Res {
    ensure_actions(client)?;
    ensure_ready(client)?;
    if body.content.trim().is_empty() {
        return Err(ApiError::bad_request("content must not be empty"));
    }
    if body.content.chars().count() > MAX_CONTENT_CHARS {
        return Err(ApiError::bad_request(format!("content is longer than {MAX_CONTENT_CHARS} characters")));
    }
    let guild_id = visible_channel(&client.store(), channel)?.get("guild_id").and_then(|g| serde_json::from_value(g.clone()).ok());
    let req = CreateMessage {
        content: body.content,
        nonce: Some(Snowflake::nonce_now().to_string()),
        message_reference: body.reply_to.map(|id| MessageReference {
            kind: Some(0),
            message_id: Some(id),
            channel_id: Some(channel),
            guild_id,
            fail_if_not_exists: Some(false),
        }),
        allowed_mentions: body.reply_to.map(|_| AllowedMentions::all(true)),
        ..Default::default()
    };
    let msg = client.http().send_message(channel, &req, vec![]).await?;
    Ok(serde_json::to_value(msg).unwrap_or_default())
}

pub(crate) async fn typing(client: &Client, channel: Snowflake) -> Res {
    ensure_actions(client)?;
    ensure_ready(client)?;
    client.http().trigger_typing(channel).await?;
    Ok(Value::Null)
}

pub(crate) async fn ack(client: &Client, channel: Snowflake, message: Option<Snowflake>) -> Res {
    ensure_actions(client)?;
    ensure_ready(client)?;
    visible_channel(&client.store(), channel)?;
    match message {
        Some(m) => client.http().ack_message(channel, m, true).await?,
        None => client.ack_channel(channel),
    }
    Ok(Value::Null)
}

/// Parses a reaction emoji: a unicode emoji, or `name:id` for custom emoji.
pub(crate) fn parse_emoji(s: &str) -> Result<PartialEmoji, ApiError> {
    let s = s.trim().trim_start_matches('<').trim_end_matches('>').trim_start_matches("a:");
    if s.is_empty() {
        return Err(ApiError::bad_request("emoji must not be empty"));
    }
    if let Some((name, id)) = s.rsplit_once(':')
        && let Ok(id) = id.parse::<u64>()
    {
        return Ok(PartialEmoji { id: Some(Snowflake(id)), name: Some(name.trim_start_matches(':').to_string()), animated: false });
    }
    Ok(PartialEmoji::unicode(s))
}

pub(crate) async fn reaction(client: &Client, channel: Snowflake, message: Snowflake, emoji: &str, add: bool) -> Res {
    ensure_actions(client)?;
    ensure_ready(client)?;
    let emoji = parse_emoji(emoji)?;
    if add {
        client.http().add_reaction(channel, message, &emoji).await?;
    } else {
        client.http().remove_reaction(channel, message, &emoji, None).await?;
    }
    Ok(Value::Null)
}

/// `PATCH /v1/presence` body: `status` and/or `custom_status` (string, or `null` to clear).
pub(crate) async fn presence(client: &Client, body: Value) -> Res {
    ensure_actions(client)?;
    ensure_ready(client)?;
    let obj = body.as_object().ok_or_else(|| ApiError::bad_request("expected a JSON object"))?;
    let status = match obj.get("status") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(match s.as_str() {
            "online" => Status::Online,
            "idle" => Status::Idle,
            "dnd" => Status::Dnd,
            "invisible" => Status::Invisible,
            _ => return Err(ApiError::bad_request("status must be one of online, idle, dnd, invisible")),
        }),
        Some(_) => return Err(ApiError::bad_request("status must be a string")),
    };
    let custom = match obj.get("custom_status") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) if s.chars().count() <= 128 => Some(Some(s.clone()).filter(|s| !s.is_empty())),
        Some(Value::String(_)) => return Err(ApiError::bad_request("custom_status is longer than 128 characters")),
        Some(_) => return Err(ApiError::bad_request("custom_status must be a string or null")),
    };
    if status.is_none() && custom.is_none() {
        return Err(ApiError::bad_request("nothing to update: pass status and/or custom_status"));
    }
    if let Some(st) = status {
        client.set_status(st);
    }
    if let Some(text) = custom {
        client.set_custom_status(text, None);
    }
    let s = client.store();
    let custom = s.settings.custom_status().map(|c| c.text.clone()).filter(|t| !t.is_empty());
    Ok(json!({ "status": s.status(s.my_id()).as_str(), "custom_status": custom }))
}

// ---------------------------------------------------------------- RPC

#[derive(Deserialize)]
struct ChannelArg {
    channel_id: Snowflake,
}

#[derive(Deserialize)]
struct GuildArg {
    guild_id: Snowflake,
}

#[derive(Deserialize)]
struct UserArg {
    user_id: Snowflake,
}

#[derive(Deserialize)]
struct MessagesArgs {
    channel_id: Snowflake,
    limit: Option<u32>,
    before: Option<Snowflake>,
}

#[derive(Deserialize)]
struct SendArgs {
    channel_id: Snowflake,
    content: String,
    reply_to: Option<Snowflake>,
}

#[derive(Deserialize)]
struct AckArgs {
    channel_id: Snowflake,
    message_id: Option<Snowflake>,
}

#[derive(Deserialize)]
struct ReactionArgs {
    channel_id: Snowflake,
    message_id: Snowflake,
    emoji: String,
}

fn args<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ApiError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|e| ApiError::bad_request(format!("invalid params: {e}")))
}

/// Names of the WebSocket RPC methods.
pub const METHODS: &[&str] = &[
    "info",
    "me",
    "guilds",
    "guild",
    "guild_channels",
    "channel",
    "messages",
    "user",
    "dms",
    "relationships",
    "send_message",
    "typing",
    "ack",
    "add_reaction",
    "remove_reaction",
    "set_presence",
];

/// Runs a WebSocket RPC method.
pub(crate) async fn dispatch(client: &Client, method: &str, params: Value) -> Res {
    match method {
        "info" => Ok(info(client)),
        "me" => me(client),
        "guilds" => guilds(client),
        "guild" => guild(client, args::<GuildArg>(params)?.guild_id),
        "guild_channels" => guild_channels(client, args::<GuildArg>(params)?.guild_id),
        "channel" => channel(client, args::<ChannelArg>(params)?.channel_id),
        "messages" => {
            let a: MessagesArgs = args(params)?;
            messages(client, a.channel_id, a.limit, a.before).await
        }
        "user" => user(client, args::<UserArg>(params)?.user_id).await,
        "dms" => dms(client),
        "relationships" => relationships(client),
        "send_message" => {
            let a: SendArgs = args(params)?;
            send_message(client, a.channel_id, SendMessage { content: a.content, reply_to: a.reply_to }).await
        }
        "typing" => typing(client, args::<ChannelArg>(params)?.channel_id).await,
        "ack" => {
            let a: AckArgs = args(params)?;
            ack(client, a.channel_id, a.message_id).await
        }
        "add_reaction" | "remove_reaction" => {
            let a: ReactionArgs = args(params)?;
            reaction(client, a.channel_id, a.message_id, &a.emoji, method == "add_reaction").await
        }
        "set_presence" => presence(client, params).await,
        _ => Err(ApiError::new(axum::http::StatusCode::NOT_FOUND, "unknown_method", format!("unknown method `{method}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_parsing() {
        assert_eq!(parse_emoji("👍").unwrap(), PartialEmoji::unicode("👍"));
        let e = parse_emoji("blob:123").unwrap();
        assert_eq!((e.id, e.name.as_deref()), (Some(Snowflake(123)), Some("blob")));
        let e = parse_emoji("<a:party:456>").unwrap();
        assert_eq!((e.id, e.name.as_deref()), (Some(Snowflake(456)), Some("party")));
        assert!(parse_emoji(" ").is_err());
    }
}
