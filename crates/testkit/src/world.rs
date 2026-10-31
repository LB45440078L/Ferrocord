//! The mock's in-memory "world": one account, its channels and messages, and the
//! REST endpoints that operate on them. Mutating endpoints emit the gateway
//! dispatches the real server would send.

use std::collections::{BTreeMap, HashMap, HashSet};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ferrocord_model::settings::PreloadedUserSettings;
use ferrocord_model::snowflake::DISCORD_EPOCH;
use rsa::RsaPublicKey;
use serde_json::{Value, json};

use crate::Shared;
use crate::fixtures::{self, UserBuilder};
use crate::rest::{MockResponse, RecordedRequest, percent_decode};

/// MFA configuration of the mock account.
#[derive(Debug, Clone)]
pub struct MfaSetup {
    pub totp_code: String,
    pub backup_code: String,
    pub sms_code: String,
    /// Offer SMS as a method in the login response.
    pub sms: bool,
    /// WebAuthn challenge JSON offered in the login response, if any.
    pub webauthn: Option<String>,
}

impl Default for MfaSetup {
    fn default() -> Self {
        Self { totp_code: "123456".into(), backup_code: "abcd1234".into(), sms_code: "654321".into(), sms: true, webauthn: None }
    }
}

/// The single account the mock knows.
#[derive(Debug, Clone)]
pub struct Account {
    pub user: UserBuilder,
    pub email: String,
    pub password: String,
    pub token: String,
    pub phone: String,
    pub mfa: Option<MfaSetup>,
    /// When set, login fails with this `suspended_user_token`.
    pub suspended_token: Option<String>,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            user: UserBuilder::me(),
            email: fixtures::DEFAULT_EMAIL.into(),
            password: fixtures::DEFAULT_PASSWORD.into(),
            token: fixtures::DEFAULT_TOKEN.into(),
            phone: "+15555550123".into(),
            mfa: None,
            suspended_token: None,
        }
    }
}

/// In-memory server state. Accessed through [`crate::MockDiscord::with_world`].
pub struct World {
    pub account: Account,
    /// Tokens accepted in `Authorization` and IDENTIFY/RESUME.
    pub valid_tokens: HashSet<String>,
    /// Value returned by `GET /experiments` (`X-Fingerprint`).
    pub fingerprint: String,
    /// Sensitive-action MFA (`POST /mfa/finish`): `(ticket, accepted code, issued token)`.
    pub mfa_finish: Option<(String, String, String)>,
    pub settings: PreloadedUserSettings,
    /// Last acked message per channel.
    pub read_states: HashMap<u64, u64>,
    ws_base: String,
    mfa_tickets: HashSet<String>,
    next_ticket: u64,
    channels: HashMap<u64, Option<u64>>,
    messages: HashMap<u64, BTreeMap<u64, Value>>,
    users: HashMap<u64, Value>,
    nonces: HashMap<(u64, String), u64>,
    last_id: u64,
    ack_version: u64,
    pub(crate) remote_tickets: HashMap<String, (RsaPublicKey, String)>,
}

impl World {
    pub(crate) fn new(ws_base: String) -> Self {
        let account = Account::default();
        let mut valid_tokens = HashSet::new();
        valid_tokens.insert(account.token.clone());
        Self {
            account,
            valid_tokens,
            fingerprint: "1300000000000000000.mock-fingerprint".into(),
            mfa_finish: None,
            settings: PreloadedUserSettings::default(),
            read_states: HashMap::new(),
            ws_base,
            mfa_tickets: HashSet::new(),
            next_ticket: 0,
            channels: HashMap::new(),
            messages: HashMap::new(),
            users: HashMap::new(),
            nonces: HashMap::new(),
            last_id: 0,
            ack_version: 0,
            remote_tickets: HashMap::new(),
        }
    }

    pub fn my_id(&self) -> u64 {
        self.account.user.id
    }

    /// Replaces the account and makes its token valid.
    pub fn set_account(&mut self, account: Account) {
        self.valid_tokens.insert(account.token.clone());
        self.account = account;
    }

    /// Makes another token acceptable.
    pub fn add_valid_token(&mut self, token: &str) {
        self.valid_tokens.insert(token.to_string());
    }

    pub fn revoke_token(&mut self, token: &str) {
        self.valid_tokens.remove(token);
    }

    /// Registers a channel (and its guild) so message endpoints accept it.
    pub fn add_channel(&mut self, channel: u64, guild: Option<u64>) {
        self.channels.insert(channel, guild);
        self.messages.entry(channel).or_default();
    }

    pub fn register_user(&mut self, user: Value) {
        if let Some(id) = user.get("id").and_then(Value::as_str).and_then(|s| s.parse().ok()) {
            self.users.insert(id, user);
        }
    }

    /// A fresh snowflake for "now", strictly greater than every id handed out before.
    pub fn next_snowflake(&mut self) -> u64 {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let candidate = now.saturating_sub(DISCORD_EPOCH) << 22;
        self.last_id = candidate.max(self.last_id + 1);
        self.last_id
    }

    /// Adds `count` messages (`"message 0"` …) by `author`, oldest first. Returns their ids.
    pub fn seed_messages(&mut self, channel: u64, count: usize, author: &UserBuilder) -> Vec<u64> {
        let guild = *self.channels.entry(channel).or_insert(None);
        self.register_user(author.build());
        (0..count)
            .map(|i| {
                let id = self.next_snowflake();
                let mut b = fixtures::MessageBuilder::new(id, channel, author).content(format!("message {i}"));
                if let Some(g) = guild {
                    b = b.guild(g);
                }
                self.insert_message(b.build());
                id
            })
            .collect()
    }

    /// Stores a message payload (must carry `id` and `channel_id`).
    pub fn insert_message(&mut self, msg: Value) {
        let (Some(id), Some(ch)) = (id_of(&msg, "id"), id_of(&msg, "channel_id")) else { return };
        self.last_id = self.last_id.max(id);
        self.channels.entry(ch).or_insert(None);
        self.messages.entry(ch).or_default().insert(id, msg);
    }

    /// Messages of a channel, oldest first.
    pub fn messages(&self, channel: u64) -> Vec<Value> {
        self.messages.get(&channel).map(|m| m.values().cloned().collect()).unwrap_or_default()
    }

    pub fn message(&self, channel: u64, id: u64) -> Option<Value> {
        self.messages.get(&channel)?.get(&id).cloned()
    }

    fn author_value(&self) -> Value {
        self.users.get(&self.my_id()).cloned().unwrap_or_else(|| self.account.user.build())
    }

    fn current_user(&self) -> Value {
        let mut u = self.account.user.build_current();
        u["email"] = json!(self.account.email);
        u["phone"] = json!(self.account.phone);
        u["mfa_enabled"] = json!(self.account.mfa.is_some());
        u
    }

    fn authed(&self, req: &RecordedRequest) -> bool {
        req.header("authorization").is_some_and(|t| self.valid_tokens.contains(t))
    }

    fn new_ticket(&mut self, prefix: &str) -> String {
        self.next_ticket += 1;
        format!("{prefix}.{}.{}", self.next_ticket, rand::random::<u32>())
    }
}

fn id_of(v: &Value, k: &str) -> Option<u64> {
    match v.get(k)? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

fn parse_id(s: &str) -> Option<u64> {
    s.parse().ok()
}

pub(crate) fn timestamp_of(id: u64) -> String {
    let ms = (id >> 22) + DISCORD_EPOCH;
    chrono::DateTime::from_timestamp_millis(ms as i64).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}

/// Dispatches produced while handling a request.
type Dispatches = Vec<(&'static str, Value)>;

/// Default REST behaviour. Called after scripted routes declined the request.
pub(crate) fn handle(shared: &Shared, req: &RecordedRequest) -> MockResponse {
    let (resp, dispatches) = {
        let mut w = shared.world.lock();
        route(&mut w, req)
    };
    for (t, d) in dispatches {
        crate::gateway::dispatch(shared, t, d, true);
    }
    resp
}

fn route(w: &mut World, req: &RecordedRequest) -> (MockResponse, Dispatches) {
    let segs = req.segments();
    let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
    let body = req.json();
    let none = Dispatches::new();

    // Unauthenticated endpoints.
    match (req.method.as_str(), segs.as_slice()) {
        ("GET", ["experiments"]) => {
            return (MockResponse::ok(json!({ "fingerprint": w.fingerprint, "assignments": [], "guild_experiments": [] })), none);
        }
        ("GET", ["gateway"]) => return (MockResponse::ok(json!({ "url": format!("{}/gateway", w.ws_base) })), none),
        ("POST", ["auth", "login"]) => return (login(w, &body), none),
        ("POST", ["auth", "mfa", "sms", "send"]) => {
            let ok = body.get("ticket").and_then(Value::as_str).is_some_and(|t| w.mfa_tickets.contains(t));
            let resp = if ok {
                let last4 = &w.account.phone[w.account.phone.len().saturating_sub(4)..];
                MockResponse::ok(json!({ "phone": format!("+*******{last4}") }))
            } else {
                MockResponse::form_error("ticket", "TICKET_INVALID", "Invalid ticket.")
            };
            return (resp, none);
        }
        ("POST", ["auth", "mfa", kind]) => return (mfa_login(w, kind, &body), none),
        ("POST", ["users", "@me", "remote-auth", "login"]) => {
            let ticket = body.get("ticket").and_then(Value::as_str).unwrap_or_default();
            let resp = match w.remote_tickets.remove(ticket) {
                Some((key, token)) => {
                    let enc = key.encrypt(&mut rand::thread_rng(), rsa::Oaep::new::<sha2::Sha256>(), token.as_bytes()).expect("rsa encrypt");
                    MockResponse::ok(json!({ "encrypted_token": STANDARD.encode(enc) }))
                }
                None => MockResponse::form_error("ticket", "TICKET_INVALID", "Invalid ticket."),
            };
            return (resp, none);
        }
        _ => {}
    }

    if !w.authed(req) {
        return (MockResponse::unauthorized(), none);
    }
    let me = w.my_id();

    match (req.method.as_str(), segs.as_slice()) {
        ("POST", ["auth", "logout"]) => {
            if let Some(t) = req.header("authorization") {
                w.valid_tokens.remove(t);
            }
            (MockResponse::no_content(), none)
        }
        ("POST", ["mfa", "finish"]) => {
            let ticket = body.get("ticket").and_then(Value::as_str).unwrap_or_default();
            let data = body.get("data").and_then(Value::as_str).unwrap_or_default();
            match &w.mfa_finish {
                Some((t, code, token)) if t == ticket && code == data => (MockResponse::ok(json!({ "token": token })), none),
                _ => (MockResponse::error(400, 60008, "Invalid two-factor code"), none),
            }
        }
        ("GET", ["users", "@me"]) => (MockResponse::ok(w.current_user()), none),
        ("PATCH", ["users", "@me", "settings-proto", "1"]) => {
            let b64 = body.get("settings").and_then(Value::as_str).unwrap_or_default();
            match PreloadedUserSettings::from_base64(b64) {
                Some(partial) => {
                    w.settings.merge_partial(partial);
                    let d = json!({ "settings": { "type": 1, "proto": b64 }, "partial": true });
                    (MockResponse::ok(json!({ "settings": w.settings.to_base64() })), vec![("USER_SETTINGS_PROTO_UPDATE", d)])
                }
                None => (MockResponse::form_error("settings", "BASE_TYPE_BAD_PROTO", "Invalid settings proto."), none),
            }
        }
        ("POST", ["users", "@me", "relationships"]) => (MockResponse::no_content(), none),
        ("PUT" | "DELETE", ["users", "@me", "relationships", _]) => (MockResponse::no_content(), none),
        ("POST", ["read-states", "ack-bulk"]) => (MockResponse::no_content(), none),

        ("GET", ["channels", c, "messages"]) => with_channel(w, c, |w, c| (list_messages(w, c, req), none)),
        ("POST", ["channels", c, "messages"]) => with_channel(w, c, |w, c| create_message(w, c, req)),
        ("GET", ["channels", c, "messages", "pins"]) => with_channel(w, c, |w, c| {
            let items: Vec<Value> =
                w.messages(c).into_iter().rev().filter(|m| m["pinned"] == json!(true)).map(|m| json!({ "message": m, "pinned_at": m["timestamp"] })).collect();
            (MockResponse::ok(json!({ "items": items, "has_more": false })), none)
        }),
        ("PATCH", ["channels", c, "messages", m]) => with_message(w, c, m, |w, c, m| edit_message(w, c, m, &body)),
        ("DELETE", ["channels", c, "messages", m]) => with_message(w, c, m, |w, c, m| {
            w.messages.get_mut(&c).map(|ms| ms.remove(&m));
            let d = json!({ "id": m.to_string(), "channel_id": c.to_string(), "guild_id": w.channels[&c].map(|g| g.to_string()) });
            (MockResponse::no_content(), vec![("MESSAGE_DELETE", d)])
        }),
        ("PUT", ["channels", c, "messages", m, "reactions", e, "@me"]) => with_message(w, c, m, |w, c, m| react(w, c, m, e, me, true)),
        ("DELETE", ["channels", c, "messages", m, "reactions", e, "0" | "1", "@me"] | ["channels", c, "messages", m, "reactions", e, "@me"]) => {
            with_message(w, c, m, |w, c, m| react(w, c, m, e, me, false))
        }
        ("POST", ["channels", c, "messages", m, "ack"]) => with_channel(w, c, |w, c| {
            let Some(m) = parse_id(m) else { return (MockResponse::not_found(), vec![]) };
            w.ack_version += 1;
            w.read_states.insert(c, m);
            let d = json!({ "version": w.ack_version, "message_id": m.to_string(), "last_viewed": body.get("last_viewed"), "flags": null, "channel_id": c.to_string() });
            (MockResponse::ok(json!({ "token": null })), vec![("MESSAGE_ACK", d)])
        }),
        ("POST", ["channels", c, "typing"]) => with_channel(w, c, |w, c| {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let mut d = json!({ "channel_id": c.to_string(), "user_id": me.to_string(), "timestamp": secs });
            if let Some(g) = w.channels[&c] {
                d["guild_id"] = json!(g.to_string());
            }
            (MockResponse::no_content(), vec![("TYPING_START", d)])
        }),
        _ => (MockResponse::not_found(), none),
    }
}

fn with_channel(w: &mut World, c: &str, f: impl FnOnce(&mut World, u64) -> (MockResponse, Dispatches)) -> (MockResponse, Dispatches) {
    match parse_id(c).filter(|c| w.channels.contains_key(c)) {
        Some(c) => f(w, c),
        None => (MockResponse::error(404, 10003, "Unknown Channel"), vec![]),
    }
}

fn with_message(w: &mut World, c: &str, m: &str, f: impl FnOnce(&mut World, u64, u64) -> (MockResponse, Dispatches)) -> (MockResponse, Dispatches) {
    with_channel(w, c, |w, c| match parse_id(m).filter(|m| w.messages.get(&c).is_some_and(|ms| ms.contains_key(m))) {
        Some(m) => f(w, c, m),
        None => (MockResponse::error(404, 10008, "Unknown Message"), vec![]),
    })
}

fn login(w: &mut World, body: &Value) -> MockResponse {
    let login = body.get("login").and_then(Value::as_str).unwrap_or_default();
    let password = body.get("password").and_then(Value::as_str).unwrap_or_default();
    if login != w.account.email || password != w.account.password {
        let err = json!({ "_errors": [{ "code": "INVALID_LOGIN", "message": "Login or password is invalid." }] });
        return MockResponse::json(400, json!({ "message": "Invalid Form Body", "code": 50035, "errors": { "login": err, "password": err } }));
    }
    if let Some(t) = &w.account.suspended_token {
        return MockResponse::json(400, json!({ "message": "This account has been suspended.", "code": 0, "suspended_user_token": t }));
    }
    let user_id = w.my_id().to_string();
    match w.account.mfa.clone() {
        Some(mfa) => {
            let ticket = w.new_ticket("mfa");
            w.mfa_tickets.insert(ticket.clone());
            MockResponse::ok(json!({
                "user_id": user_id, "mfa": true, "sms": mfa.sms, "totp": true, "backup": true,
                "webauthn": mfa.webauthn, "ticket": ticket, "login_instance_id": "mock-login-instance",
            }))
        }
        None => MockResponse::ok(json!({ "user_id": user_id, "token": w.account.token, "user_settings": { "locale": "en-US", "theme": "dark" } })),
    }
}

fn mfa_login(w: &mut World, kind: &str, body: &Value) -> MockResponse {
    let ticket = body.get("ticket").and_then(Value::as_str).unwrap_or_default();
    let code = body.get("code").and_then(Value::as_str).unwrap_or_default();
    if !w.mfa_tickets.contains(ticket) {
        return MockResponse::form_error("ticket", "TICKET_INVALID", "Invalid ticket.");
    }
    let Some(mfa) = &w.account.mfa else { return MockResponse::not_found() };
    let expected = match kind {
        "totp" => &mfa.totp_code,
        "backup" => &mfa.backup_code,
        "sms" => &mfa.sms_code,
        _ => return MockResponse::not_found(),
    };
    if code != expected {
        return MockResponse::error(400, 60008, "Invalid two-factor code");
    }
    w.mfa_tickets.remove(ticket);
    MockResponse::ok(json!({ "token": w.account.token, "user_settings": { "locale": "en-US", "theme": "dark" } }))
}

fn list_messages(w: &World, c: u64, req: &RecordedRequest) -> MockResponse {
    let limit = req.query_param("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(50).clamp(1, 100);
    let all = &w.messages[&c];
    let q = |k: &str| req.query_param(k).and_then(parse_id);
    let mut page: Vec<Value> = if let Some(before) = q("before") {
        all.range(..before).rev().take(limit).map(|(_, m)| m.clone()).collect()
    } else if let Some(after) = q("after") {
        // The oldest `limit` messages after the anchor, returned newest first.
        let mut v: Vec<Value> = all.range(after + 1..).take(limit).map(|(_, m)| m.clone()).collect();
        v.reverse();
        v
    } else if let Some(around) = q("around") {
        let half = limit / 2;
        let mut newer: Vec<Value> = all.range(around + 1..).take(limit - half - usize::from(all.contains_key(&around))).map(|(_, m)| m.clone()).collect();
        newer.reverse();
        let older = all.range(..=around).rev().take(half + usize::from(all.contains_key(&around))).map(|(_, m)| m.clone());
        newer.extend(older);
        newer
    } else {
        all.values().rev().take(limit).cloned().collect()
    };
    page.truncate(limit);
    MockResponse::ok(Value::Array(page))
}

fn create_message(w: &mut World, c: u64, req: &RecordedRequest) -> (MockResponse, Dispatches) {
    let (payload, files) = match req.multipart() {
        Some(parts) => {
            let payload = parts.iter().find(|p| p.name == "payload_json").and_then(|p| serde_json::from_slice(&p.data).ok()).unwrap_or(Value::Null);
            let files: Vec<_> = parts.into_iter().filter(|p| p.name.starts_with("files[")).collect();
            (payload, files)
        }
        None => (req.json(), vec![]),
    };
    let content = payload.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
    if content.is_empty() && files.is_empty() && payload.get("sticker_ids").is_none() {
        return (MockResponse::error(400, 50006, "Cannot send an empty message"), vec![]);
    }
    let nonce = payload.get("nonce").and_then(|n| n.as_str().map(str::to_string).or_else(|| n.as_u64().map(|n| n.to_string())));
    if payload.get("enforce_nonce") == Some(&json!(true))
        && let Some(n) = &nonce
        && let Some(existing) = w.nonces.get(&(c, n.clone())).and_then(|id| w.message(c, *id))
    {
        // Duplicate send: Discord returns the original message and emits nothing.
        return (MockResponse::ok(existing), vec![]);
    }
    let id = w.next_snowflake();
    let attachments: Vec<Value> = files
        .iter()
        .map(|f| {
            let aid = w.next_snowflake();
            let name = f.filename.clone().unwrap_or_else(|| "file".into());
            json!({
                "id": aid.to_string(), "filename": name, "size": f.data.len(), "content_type": f.content_type,
                "url": format!("https://cdn.discordapp.com/attachments/{c}/{aid}/{name}"),
                "proxy_url": format!("https://media.discordapp.net/attachments/{c}/{aid}/{name}"),
            })
        })
        .collect();
    let reference = payload.get("message_reference").filter(|r| !r.is_null()).cloned();
    let mut msg = json!({
        "id": id.to_string(), "channel_id": c.to_string(), "author": w.author_value(), "content": content,
        "timestamp": timestamp_of(id), "edited_timestamp": null, "tts": payload.get("tts").and_then(Value::as_bool).unwrap_or(false),
        "mention_everyone": false, "mentions": [], "mention_roles": [], "attachments": attachments, "embeds": [],
        "pinned": false, "type": if reference.is_some() { 19 } else { 0 }, "flags": payload.get("flags").cloned().unwrap_or(json!(0)),
        "components": [],
    });
    if let Some(n) = &nonce {
        msg["nonce"] = json!(n);
        w.nonces.insert((c, n.clone()), id);
    }
    if let Some(r) = reference {
        let target = id_of(&r, "message_id").and_then(|m| w.message(c, m));
        msg["message_reference"] = json!({ "type": 0, "channel_id": c.to_string(), "message_id": r.get("message_id"), "guild_id": r.get("guild_id") });
        msg["referenced_message"] = target.unwrap_or(Value::Null);
    }
    w.insert_message(msg.clone());
    let mut event = msg.clone();
    if let Some(g) = w.channels[&c] {
        event["guild_id"] = json!(g.to_string());
        event["member"] = json!({ "roles": [], "joined_at": "2024-01-01T00:00:00.000000+00:00", "deaf": false, "mute": false, "flags": 0, "nick": null, "avatar": null, "banner": null, "premium_since": null, "pending": false, "communication_disabled_until": null });
    }
    (MockResponse::ok(msg), vec![("MESSAGE_CREATE", event)])
}

fn edit_message(w: &mut World, c: u64, m: u64, body: &Value) -> (MockResponse, Dispatches) {
    let me = w.my_id().to_string();
    let guild = w.channels[&c];
    let Some(msg) = w.messages.get_mut(&c).and_then(|ms| ms.get_mut(&m)) else { return (MockResponse::not_found(), vec![]) };
    if msg["author"]["id"] != json!(me) && body.get("content").is_some() {
        return (MockResponse::error(403, 50005, "Cannot edit a message authored by another user"), vec![]);
    }
    if let Some(content) = body.get("content") {
        msg["content"] = content.clone();
    }
    if let Some(flags) = body.get("flags") {
        msg["flags"] = flags.clone();
    }
    msg["edited_timestamp"] = json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false));
    let resp = msg.clone();
    let mut event = msg.clone();
    if let Some(g) = guild {
        event["guild_id"] = json!(g.to_string());
    }
    (MockResponse::ok(resp), vec![("MESSAGE_UPDATE", event)])
}

/// Parses a reaction route segment: `name:id` (custom) or a unicode emoji.
fn parse_emoji(seg: &str) -> Value {
    let s = percent_decode(seg);
    match s.rsplit_once(':') {
        Some((name, id)) if id.bytes().all(|b| b.is_ascii_digit()) && !id.is_empty() => json!({ "id": id, "name": name, "animated": false }),
        _ => json!({ "id": null, "name": s }),
    }
}

fn same_emoji(a: &Value, b: &Value) -> bool {
    if !a["id"].is_null() || !b["id"].is_null() { a["id"] == b["id"] } else { a["name"] == b["name"] }
}

fn react(w: &mut World, c: u64, m: u64, seg: &str, user: u64, add: bool) -> (MockResponse, Dispatches) {
    let emoji = parse_emoji(seg);
    let guild = w.channels[&c];
    let msg = w.messages.get_mut(&c).and_then(|ms| ms.get_mut(&m)).expect("checked by with_message");
    if !msg["reactions"].is_array() {
        msg["reactions"] = json!([]);
    }
    let author = msg["author"]["id"].clone();
    let reactions = msg["reactions"].as_array_mut().expect("array");
    let pos = reactions.iter().position(|r| same_emoji(&r["emoji"], &emoji));
    let changed = match (add, pos) {
        (true, Some(i)) if reactions[i]["me"] == json!(true) => false,
        (true, Some(i)) => {
            let r = &mut reactions[i];
            r["count"] = json!(r["count"].as_u64().unwrap_or(0) + 1);
            r["count_details"]["normal"] = r["count"].clone();
            r["me"] = json!(true);
            true
        }
        (true, None) => {
            reactions.push(json!({ "emoji": emoji, "count": 1, "count_details": { "burst": 0, "normal": 1 }, "burst_colors": [], "me_burst": false, "burst_me": false, "me": true, "burst_count": 0 }));
            true
        }
        (false, Some(i)) if reactions[i]["me"] == json!(true) => {
            let n = reactions[i]["count"].as_u64().unwrap_or(1).saturating_sub(1);
            if n == 0 {
                reactions.remove(i);
            } else {
                reactions[i]["count"] = json!(n);
                reactions[i]["count_details"]["normal"] = json!(n);
                reactions[i]["me"] = json!(false);
            }
            true
        }
        (false, _) => false,
    };
    if !changed {
        return (MockResponse::no_content(), vec![]);
    }
    let mut d = json!({ "user_id": user.to_string(), "type": 0, "message_id": m.to_string(), "channel_id": c.to_string(), "emoji": emoji, "burst": false });
    if let Some(g) = guild {
        d["guild_id"] = json!(g.to_string());
    }
    if add {
        d["message_author_id"] = author;
    }
    (MockResponse::no_content(), vec![(if add { "MESSAGE_REACTION_ADD" } else { "MESSAGE_REACTION_REMOVE" }, d)])
}
