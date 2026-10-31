use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrocord_auth::TokenStore;
use ferrocord_gateway::{ConnectionState, Gateway, GatewayConfig, GatewayHandle, GatewayMessage};
use ferrocord_http::{FilePart, Http, HttpError, MessageAnchor};
use ferrocord_model::gateway::{GuildSubscription, UpdatePresence};
use ferrocord_model::settings::{CustomStatus, PreloadedUserSettings, StatusSettings, StringValue};
use ferrocord_model::*;
use ferrocord_store::Store;
use parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::config::{Config, Paths};
use crate::extension::{Extension, Flow, LocalCommand, OutgoingMessage};

/// Everything observable about the client, broadcast to the UI, plugins and API clients.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// A gateway dispatch, already applied to the store.
    Dispatch {
        name: String,
        event: Arc<Event>,
        raw: Arc<Value>,
    },
    Connection(ConnectionState),
    Latency(Duration),
    /// The session ended; the UI should return to the login screen.
    LoggedOut {
        reason: String,
    },
    /// A message that warrants a desktop notification.
    Notification(Notification),
    /// A history page finished loading.
    MessagesLoaded {
        channel_id: Snowflake,
    },
    /// A user-initiated action failed.
    ActionFailed {
        action: String,
        error: String,
    },
}

impl ClientEvent {
    /// Stable event name used by the plugin and API layers.
    pub fn name(&self) -> &str {
        match self {
            ClientEvent::Dispatch { name, .. } => name,
            ClientEvent::Connection(_) => "CONNECTION_STATE",
            ClientEvent::Latency(_) => "LATENCY",
            ClientEvent::LoggedOut { .. } => "LOGGED_OUT",
            ClientEvent::Notification(_) => "NOTIFICATION",
            ClientEvent::MessagesLoaded { .. } => "MESSAGES_LOADED",
            ClientEvent::ActionFailed { .. } => "ACTION_FAILED",
        }
    }

    /// JSON form used by plugins and the API.
    pub fn to_json(&self) -> Value {
        use serde_json::json;
        match self {
            ClientEvent::Dispatch { name, raw, .. } => json!({ "type": name, "data": &**raw }),
            ClientEvent::Connection(s) => json!({ "type": self.name(), "data": format!("{s:?}") }),
            ClientEvent::Latency(d) => json!({ "type": self.name(), "data": d.as_millis() as u64 }),
            ClientEvent::LoggedOut { reason } => json!({ "type": self.name(), "data": { "reason": reason } }),
            ClientEvent::Notification(n) => json!({ "type": self.name(), "data": n }),
            ClientEvent::MessagesLoaded { channel_id } => json!({ "type": self.name(), "data": { "channel_id": channel_id } }),
            ClientEvent::ActionFailed { action, error } => json!({ "type": self.name(), "data": { "action": action, "error": error } }),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub message_id: Snowflake,
    pub icon_url: Option<String>,
}

/// Construction options for [`Client`].
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Fetch the live web client build number on startup (network access).
    pub refresh_build_number: bool,
    /// REST API root including the version, e.g. `https://discord.com/api/v9`.
    pub api_base: String,
    /// Gateway websocket base URL (without query string).
    pub gateway_url: String,
    /// Save/restore tokens in the OS credential vault. Disable in tests.
    pub persist_credentials: bool,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            refresh_build_number: true,
            api_base: ferrocord_http::API_BASE.to_string(),
            gateway_url: ferrocord_gateway::DEFAULT_GATEWAY.to_string(),
            persist_credentials: true,
        }
    }
}

impl ClientOptions {
    /// No background network activity: for tests and offline demos.
    pub fn offline() -> Self {
        Self { refresh_build_number: false, persist_credentials: false, ..Self::default() }
    }

    /// Points the client at a mock or staging server (no build-number fetch).
    pub fn custom_endpoints(api_base: impl Into<String>, gateway_url: impl Into<String>) -> Self {
        Self { refresh_build_number: false, api_base: api_base.into(), gateway_url: gateway_url.into(), persist_credentials: false }
    }
}

type Repaint = Arc<dyn Fn() + Send + Sync>;
/// Lazy-guild subscription: the channel whose member list we follow, and its row ranges.
type Subscription = (Snowflake, Vec<[u32; 2]>);

struct Inner {
    http: Http,
    rt: tokio::runtime::Handle,
    store: RwLock<Store>,
    gateway: Mutex<Option<GatewayHandle>>,
    events: broadcast::Sender<ClientEvent>,
    config: RwLock<Config>,
    paths: Paths,
    extensions: RwLock<Vec<Arc<dyn Extension>>>,
    connection: RwLock<ConnectionState>,
    latency: RwLock<Option<Duration>>,
    repaint: RwLock<Option<Repaint>>,
    typing_sent: Mutex<HashMap<Snowflake, Instant>>,
    /// Guilds we've sent a lazy subscription for, with the channel + ranges.
    subscriptions: Mutex<HashMap<Snowflake, Subscription>>,
    /// Channel currently shown in the UI (suppresses notifications for it).
    focused_channel: RwLock<Option<Snowflake>>,
    window_focused: RwLock<bool>,
    gateway_url: String,
    persist_credentials: bool,
}

/// The Ferrocord client. Cheap to clone; all clones share state.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    /// Creates a client bound to a Tokio runtime handle.
    pub fn new(rt: tokio::runtime::Handle, paths: Paths) -> Self {
        Self::with_options(rt, paths, ClientOptions::default())
    }

    /// Creates a client with explicit options (used by tests and embedders).
    pub fn with_options(rt: tokio::runtime::Handle, paths: Paths, options: ClientOptions) -> Self {
        paths.ensure();
        let config = Config::load(&paths);
        let http = Http::with_base_url(ferrocord_http::ClientProfile::desktop(), &options.api_base);
        let (events, _) = broadcast::channel(4096);
        let client = Self {
            inner: Arc::new(Inner {
                http,
                rt,
                store: RwLock::new(Store::new()),
                gateway: Mutex::new(None),
                events,
                config: RwLock::new(config),
                paths,
                extensions: RwLock::new(vec![]),
                connection: RwLock::new(ConnectionState::Disconnected),
                latency: RwLock::new(None),
                repaint: RwLock::new(None),
                typing_sent: Mutex::new(HashMap::new()),
                subscriptions: Mutex::new(HashMap::new()),
                focused_channel: RwLock::new(None),
                window_focused: RwLock::new(true),
                gateway_url: options.gateway_url.clone(),
                persist_credentials: options.persist_credentials,
            }),
        };
        client.add_extension(Arc::new(crate::extension::BuiltinCommands));
        if options.refresh_build_number {
            // Track the live client build so our fingerprint never goes stale.
            let http = client.inner.http.clone();
            client.inner.rt.spawn(async move {
                http.refresh_build_number().await;
            });
        }
        client
    }

    // ------------------------------------------------------------ accessors

    pub fn http(&self) -> &Http {
        &self.inner.http
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.inner.rt
    }

    pub fn store(&self) -> RwLockReadGuard<'_, Store> {
        self.inner.store.read()
    }

    pub fn store_mut(&self) -> RwLockWriteGuard<'_, Store> {
        self.inner.store.write()
    }

    pub fn config(&self) -> Config {
        self.inner.config.read().clone()
    }

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let mut c = self.inner.config.write();
        f(&mut c);
        c.save(&self.inner.paths);
    }

    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ClientEvent> {
        self.inner.events.subscribe()
    }

    pub fn connection_state(&self) -> ConnectionState {
        *self.inner.connection.read()
    }

    pub fn latency(&self) -> Option<Duration> {
        *self.inner.latency.read()
    }

    pub fn gateway(&self) -> Option<GatewayHandle> {
        self.inner.gateway.lock().clone()
    }

    /// Registers a callback to wake the UI after state changes.
    pub fn set_repaint(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.inner.repaint.write() = Some(Arc::new(f));
    }

    /// Wakes the UI so it re-renders (e.g. after background work updated caches).
    pub fn request_repaint(&self) {
        if let Some(r) = self.inner.repaint.read().clone() {
            r();
        }
    }

    fn repaint(&self) {
        self.request_repaint();
    }

    pub fn emit(&self, ev: ClientEvent) {
        for ext in self.inner.extensions.read().clone() {
            ext.on_event(self, &ev);
        }
        let _ = self.inner.events.send(ev);
        self.repaint();
    }

    fn fail(&self, action: &str, e: impl std::fmt::Display) {
        tracing::warn!(action, error = %e, "action failed");
        self.emit(ClientEvent::ActionFailed { action: action.into(), error: e.to_string() });
    }

    pub fn set_focus(&self, channel: Option<Snowflake>, window_focused: bool) {
        *self.inner.focused_channel.write() = channel;
        *self.inner.window_focused.write() = window_focused;
    }

    // ------------------------------------------------------------ extensions

    pub fn add_extension(&self, ext: Arc<dyn Extension>) {
        let mut v = self.inner.extensions.write();
        v.retain(|e| e.id() != ext.id());
        v.push(ext);
    }

    pub fn remove_extension(&self, id: &str) {
        let mut v = self.inner.extensions.write();
        if let Some(pos) = v.iter().position(|e| e.id() == id) {
            v.remove(pos).shutdown();
        }
    }

    pub fn extensions(&self) -> Vec<Arc<dyn Extension>> {
        self.inner.extensions.read().clone()
    }

    pub fn local_commands(&self) -> Vec<LocalCommand> {
        self.extensions().iter().flat_map(|e| e.commands()).collect()
    }

    // ------------------------------------------------------------ session

    /// Restores the last session from the credential vault, if any.
    pub fn try_resume_saved_session(&self) -> bool {
        if !self.inner.persist_credentials {
            return false;
        }
        let Some(account) = self.inner.config.read().last_account.clone() else { return false };
        match TokenStore::load(&account) {
            Some(token) => {
                self.start_session(token);
                true
            }
            None => false,
        }
    }

    /// Connects to the gateway with a token. The token is persisted to the OS
    /// credential vault once READY confirms it is valid.
    pub fn start_session(&self, token: String) {
        self.stop_gateway();
        self.inner.store.write().clear();
        self.inner.subscriptions.lock().clear();
        self.inner.http.set_token(Some(token.clone()));
        let mut cfg = GatewayConfig::new(token, self.inner.http.profile());
        cfg.url = self.inner.gateway_url.clone();
        cfg.presence = UpdatePresence::default();
        let client = self.clone();
        let _guard = self.inner.rt.enter();
        let (handle, rx) = Gateway::spawn(cfg);
        *self.inner.gateway.lock() = Some(handle);
        self.inner.rt.spawn(async move { client.pump(rx).await });
    }

    fn stop_gateway(&self) {
        if let Some(g) = self.inner.gateway.lock().take() {
            g.shutdown();
        }
    }

    /// Logs out: revokes the token server-side, wipes it locally and resets state.
    pub fn logout(&self) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            let _ = client.inner.http.logout().await;
            client.end_session("Logged out");
        });
    }

    fn end_session(&self, reason: &str) {
        self.stop_gateway();
        if let Some(acc) = self.inner.config.read().last_account.clone()
            && self.inner.persist_credentials
        {
            let _ = TokenStore::delete(&acc);
        }
        self.update_config(|c| c.last_account = None);
        self.inner.http.set_token(None);
        self.inner.store.write().clear();
        self.emit(ClientEvent::LoggedOut { reason: reason.into() });
    }

    async fn pump(self, mut rx: tokio::sync::mpsc::UnboundedReceiver<GatewayMessage>) {
        while let Some(msg) = rx.recv().await {
            match msg {
                GatewayMessage::Dispatch { event, raw, .. } => {
                    let notification = match &*event {
                        Event::MessageCreate(m) => self.notification_for(m),
                        _ => None,
                    };
                    if let Event::Ready(r) = &*event {
                        self.on_ready(r);
                    }
                    self.inner.store.write().apply(&event);
                    let name = event.name().to_string();
                    self.emit(ClientEvent::Dispatch { name, event: Arc::from(event), raw });
                    if let Some(n) = notification {
                        self.emit(ClientEvent::Notification(n));
                    }
                }
                GatewayMessage::State(s) => {
                    *self.inner.connection.write() = s;
                    if s == ConnectionState::Connected {
                        self.resubscribe();
                    }
                    self.emit(ClientEvent::Connection(s));
                }
                GatewayMessage::Latency(d) => {
                    *self.inner.latency.write() = Some(d);
                    self.emit(ClientEvent::Latency(d));
                }
                GatewayMessage::Fatal(e) => {
                    tracing::error!(error = %e, "gateway fatal error");
                    let auth = matches!(e, ferrocord_gateway::GatewayError::AuthenticationFailed);
                    if auth {
                        self.end_session("Your session expired or the token was revoked. Please log in again.");
                    } else {
                        self.emit(ClientEvent::LoggedOut { reason: e.to_string() });
                    }
                    return;
                }
            }
        }
    }

    fn on_ready(&self, r: &gateway::Ready) {
        let account = r.user.user.id.to_string();
        let token = r.auth_token.clone().or_else(|| self.inner.http.token());
        if let Some(t) = &token {
            if r.auth_token.is_some() {
                self.inner.http.set_token(Some(t.clone()));
            }
            if self.inner.persist_credentials
                && let Err(e) = TokenStore::save(&account, t)
            {
                tracing::warn!(error = %e, "could not save token to credential vault");
            }
        }
        self.update_config(|c| c.last_account = Some(account));
    }

    /// Re-sends lazy subscriptions after (re)connect.
    fn resubscribe(&self) {
        let subs = self.inner.subscriptions.lock().clone();
        if subs.is_empty() {
            return;
        }
        if let Some(gw) = self.gateway() {
            gw.subscribe_guilds(subs.into_iter().map(|(g, (c, ranges))| (g, guild_subscription(c, ranges))));
        }
    }

    // ------------------------------------------------------------ notifications

    fn notification_for(&self, m: &Message) -> Option<Notification> {
        let store = self.inner.store.read();
        if m.author.id == store.my_id() || m.author.id.0 == 0 {
            return None;
        }
        let focused = *self.inner.focused_channel.read() == Some(m.channel_id) && *self.inner.window_focused.read();
        if focused {
            return None;
        }
        let channel = store.channels.get(&m.channel_id)?;
        if store.is_muted(m.channel_id) || channel.guild_id.is_some_and(|g| store.is_guild_muted(g)) {
            // Muted channels still notify on direct mentions.
            if !m.mentions_user(store.my_id()) {
                return None;
            }
        }
        if m.flags & message_flags::SUPPRESS_NOTIFICATIONS != 0 {
            return None;
        }
        let level = notification_level(&store, channel);
        let notify = match level {
            0 => true,
            1 => store.is_mentioned(m),
            _ => false,
        };
        if !notify {
            return None;
        }
        let author = store.display_name(m.guild_id, m.author.id);
        let title = match channel.guild_id.and_then(|g| store.guilds.get(&g)) {
            Some(g) => format!("{author} (#{}, {})", channel.name.as_deref().unwrap_or("?"), g.name()),
            None if channel.kind == ChannelType::GroupDm => format!("{author} ({})", channel.display_name()),
            None => author,
        };
        let mut body = m.content.clone();
        if body.is_empty() {
            body = if !m.attachments.is_empty() { "Sent an attachment".into() } else { "Sent a message".into() };
        }
        Some(Notification { title, body, channel_id: m.channel_id, guild_id: channel.guild_id, message_id: m.id, icon_url: Some(m.author.avatar_url(64)) })
    }

    // ------------------------------------------------------------ messages

    /// Loads a page of history for a channel. `before = None` loads the latest page.
    pub fn load_messages(&self, channel: Snowflake, before: Option<Snowflake>) {
        {
            let mut s = self.inner.store.write();
            let cm = s.messages.entry(channel).or_default();
            if cm.loading || (before.is_some() && cm.reached_start) {
                return;
            }
            cm.loading = true;
        }
        let client = self.clone();
        self.inner.rt.spawn(async move {
            let anchor = before.map(MessageAnchor::Before).unwrap_or(MessageAnchor::Latest);
            match client.inner.http.messages(channel, anchor, 50).await {
                Ok(page) => {
                    client.inner.store.write().insert_history(channel, page, 50, before.is_none());
                    client.emit(ClientEvent::MessagesLoaded { channel_id: channel });
                }
                Err(e) => {
                    if let Some(cm) = client.inner.store.write().messages.get_mut(&channel) {
                        cm.loading = false;
                        // Missing access: stop retrying.
                        cm.reached_start = true;
                        cm.has_latest = true;
                    }
                    client.fail("load_messages", e);
                }
            }
        });
    }

    /// Jumps to a message by loading the page around it.
    pub fn load_around(&self, channel: Snowflake, message: Snowflake) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            match client.inner.http.messages(channel, MessageAnchor::Around(message), 50).await {
                Ok(page) => {
                    {
                        let mut s = client.inner.store.write();
                        // A jump creates a gap; reset the cache to the new window.
                        s.messages.remove(&channel);
                        s.insert_history(channel, page, 0, false);
                    }
                    client.emit(ClientEvent::MessagesLoaded { channel_id: channel });
                }
                Err(e) => client.fail("load_around", e),
            }
        });
    }

    /// Sends a message, running extension hooks and showing a local echo.
    pub fn send_message(&self, channel: Snowflake, content: String, reply_to: Option<Snowflake>, mention_reply: bool, files: Vec<FilePart>) {
        let guild_id = self.inner.store.read().channels.get(&channel).and_then(|c| c.guild_id);
        let mut out = OutgoingMessage { channel_id: channel, guild_id, content, reply_to };

        // Local commands provided by extensions (e.g. `/nick`, plugin commands).
        if let Some(rest) = out.content.strip_prefix('/') {
            let (cmd, args) = rest.split_once(' ').unwrap_or((rest, ""));
            for ext in self.extensions() {
                if ext.commands().iter().any(|c| c.name == cmd) && ext.id() != "builtin.commands" && ext.run_command(self, channel, cmd, args) {
                    return;
                }
            }
        }
        for ext in self.extensions() {
            if ext.on_outgoing_message(self, &mut out) == Flow::Cancel {
                return;
            }
        }
        if out.content.trim().is_empty() && files.is_empty() {
            return;
        }

        let nonce = Snowflake::nonce_now().to_string();
        self.inner.store.write().add_pending(channel, &nonce, &out.content, reply_to);
        self.repaint();
        self.inner.typing_sent.lock().remove(&channel);

        let req = CreateMessage {
            content: out.content,
            nonce: Some(nonce.clone()),
            tts: false,
            flags: 0,
            message_reference: reply_to.map(|id| MessageReference {
                kind: Some(0),
                message_id: Some(id),
                channel_id: Some(channel),
                guild_id,
                fail_if_not_exists: None,
            }),
            allowed_mentions: reply_to.map(|_| AllowedMentions::all(mention_reply)),
            sticker_ids: vec![],
            attachments: vec![],
            enforce_nonce: Some(true),
            mobile_network_type: Some("unknown".into()),
        };
        let client = self.clone();
        self.inner.rt.spawn(async move {
            match client.inner.http.send_message(channel, &req, files).await {
                Ok(_) => {}
                Err(e) => {
                    client.inner.store.write().fail_pending(&nonce, error_text(&e));
                    client.fail("send_message", e);
                }
            }
        });
    }

    /// Async variant for API/plugin callers that need the created message.
    pub async fn send_message_async(&self, channel: Snowflake, content: String) -> Result<Message, HttpError> {
        let msg = CreateMessage { content, nonce: Some(Snowflake::nonce_now().to_string()), ..Default::default() };
        self.inner.http.send_message(channel, &msg, vec![]).await
    }

    pub fn retry_pending(&self, channel: Snowflake, nonce: &str) {
        let content =
            self.inner.store.read().messages.get(&channel).and_then(|cm| cm.messages.iter().find(|m| m.id.to_string() == nonce).map(|m| m.content.clone()));
        self.inner.store.write().remove_pending(nonce);
        if let Some(c) = content {
            self.send_message(channel, c, None, false, vec![]);
        }
    }

    pub fn discard_pending(&self, nonce: &str) {
        self.inner.store.write().remove_pending(nonce);
        self.repaint();
    }

    pub fn edit_message(&self, channel: Snowflake, message: Snowflake, content: String) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            match client.inner.http.edit_message(channel, message, &content).await {
                Ok(m) => {
                    client.inner.store.write().put_message(m);
                    client.repaint();
                }
                Err(e) => client.fail("edit_message", e),
            }
        });
    }

    pub fn delete_message(&self, channel: Snowflake, message: Snowflake) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = client.inner.http.delete_message(channel, message).await {
                client.fail("delete_message", e);
            }
        });
    }

    pub fn toggle_reaction(&self, channel: Snowflake, message: Snowflake, emoji: PartialEmoji) {
        let me = self
            .inner
            .store
            .read()
            .message(channel, message)
            .and_then(|m| m.reactions.iter().find(|r| r.emoji.id == emoji.id && (emoji.id.is_some() || r.emoji.name == emoji.name)).map(|r| r.me))
            .unwrap_or(false);
        let client = self.clone();
        self.inner.rt.spawn(async move {
            let r = if me {
                client.inner.http.remove_reaction(channel, message, &emoji, None).await
            } else {
                client.inner.http.add_reaction(channel, message, &emoji).await
            };
            if let Err(e) = r {
                client.fail("reaction", e);
            }
        });
    }

    pub fn set_pinned(&self, channel: Snowflake, message: Snowflake, pinned: bool) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            let r = if pinned { client.inner.http.pin(channel, message).await } else { client.inner.http.unpin(channel, message).await };
            if let Err(e) = r {
                client.fail("pin", e);
            }
        });
    }

    /// Sends a typing indicator, throttled like the official client (every 8s).
    pub fn typing(&self, channel: Snowflake) {
        if !self.inner.config.read().privacy.send_typing {
            return;
        }
        {
            let mut t = self.inner.typing_sent.lock();
            if t.get(&channel).is_some_and(|at| at.elapsed() < Duration::from_secs(8)) {
                return;
            }
            t.insert(channel, Instant::now());
        }
        let http = self.inner.http.clone();
        self.inner.rt.spawn(async move {
            let _ = http.trigger_typing(channel).await;
        });
    }

    /// Marks a channel read up to its latest message.
    pub fn ack_channel(&self, channel: Snowflake) {
        let last = {
            let s = self.inner.store.read();
            let Some(c) = s.channels.get(&channel) else { return };
            let Some(last) = c.last_message_id else { return };
            let read = s.read_states.get(&channel).and_then(|r| r.last_message_id);
            if read.is_some_and(|r| r >= last) && s.mention_count(channel) == 0 {
                return;
            }
            last
        };
        {
            let mut s = self.inner.store.write();
            let rs = s.read_states.entry(channel).or_insert_with(|| ReadState { id: channel, ..Default::default() });
            rs.last_message_id = Some(last);
            rs.mention_count = 0;
        }
        let http = self.inner.http.clone();
        self.inner.rt.spawn(async move {
            let _ = http.ack_message(channel, last, false).await;
        });
    }

    pub fn ack_guild(&self, guild: Snowflake) {
        let entries: Vec<(Snowflake, Snowflake)> = {
            let mut s = self.inner.store.write();
            let ids: Vec<_> = s.channels.values().filter(|c| c.guild_id == Some(guild)).filter_map(|c| c.last_message_id.map(|l| (c.id, l))).collect();
            for (c, l) in &ids {
                let rs = s.read_states.entry(*c).or_insert_with(|| ReadState { id: *c, ..Default::default() });
                rs.last_message_id = Some(*l);
                rs.mention_count = 0;
            }
            ids
        };
        let http = self.inner.http.clone();
        self.inner.rt.spawn(async move {
            for chunk in entries.chunks(100) {
                let _ = http.ack_bulk(chunk).await;
            }
        });
        self.repaint();
    }

    // ------------------------------------------------------------ navigation

    /// Called when the user opens a channel: subscribes to lazy guild events
    /// (typing, member list, threads) like the official client does.
    pub fn open_channel(&self, channel: Snowflake) {
        let guild = self.inner.store.read().channels.get(&channel).and_then(|c| c.guild_id);
        if let Some(g) = guild {
            self.subscribe_member_list(g, channel, 0);
        }
        let needs_load = self.inner.store.read().messages.get(&channel).is_none_or(|cm| !cm.has_latest);
        if needs_load {
            self.load_messages(channel, None);
        }
    }

    /// Requests member-list rows up to `max_index` for a channel's sidebar.
    pub fn subscribe_member_list(&self, guild: Snowflake, channel: Snowflake, max_index: u32) {
        let ranges: Vec<[u32; 2]> = {
            let chunk = max_index / 100;
            let mut r = vec![[0, 99]];
            if chunk > 0 {
                if chunk > 1 {
                    r.push([(chunk - 1) * 100, (chunk - 1) * 100 + 99]);
                }
                r.push([chunk * 100, chunk * 100 + 99]);
            }
            r.dedup();
            r
        };
        {
            let mut subs = self.inner.subscriptions.lock();
            if subs.get(&guild) == Some(&(channel, ranges.clone())) {
                return;
            }
            subs.insert(guild, (channel, ranges.clone()));
        }
        if let Some(gw) = self.gateway() {
            gw.subscribe_guilds([(guild, guild_subscription(channel, ranges))]);
        }
    }

    /// Requests specific members (e.g. authors in view without cached members).
    pub fn request_members(&self, guild: Snowflake, users: Vec<Snowflake>) {
        if users.is_empty() {
            return;
        }
        if let Some(gw) = self.gateway() {
            gw.request_guild_members(&gateway::RequestGuildMembers { guild_id: vec![guild], user_ids: users, presences: false, ..Default::default() });
        }
    }

    /// Opens (or creates) a DM with a user and returns its channel id.
    pub async fn open_dm(&self, user: Snowflake) -> Result<Snowflake, HttpError> {
        let existing = {
            let s = self.inner.store.read();
            s.sorted_private_channels().into_iter().find(|c| c.kind == ChannelType::Dm && c.recipient_ids() == vec![user]).map(|c| c.id)
        };
        if let Some(id) = existing {
            return Ok(id);
        }
        let ch = self.inner.http.create_dm(&[user]).await?;
        let id = ch.id;
        self.inner.store.write().upsert_channel(ch);
        Ok(id)
    }

    // ------------------------------------------------------------ account

    pub fn set_status(&self, status: Status) {
        let settings = PreloadedUserSettings {
            status: Some(StatusSettings { status: Some(StringValue { value: status.as_str().into() }), custom_status: None, show_current_game: None }),
            ..Default::default()
        };
        self.inner.store.write().settings.merge_partial(settings.clone());
        if let Some(gw) = self.gateway() {
            gw.update_presence(&UpdatePresence { status, since: 0, activities: self.presence_activities(), afk: false });
        }
        self.patch_settings(settings);
    }

    pub fn set_custom_status(&self, text: Option<String>, emoji: Option<PartialEmoji>) {
        let cs = text.clone().map(|t| CustomStatus {
            text: t,
            emoji_id: emoji.as_ref().and_then(|e| e.id).map(|i| i.0).unwrap_or(0),
            emoji_name: emoji.as_ref().and_then(|e| e.name.clone()).unwrap_or_default(),
            expires_at_ms: 0,
            created_at_ms: 0,
        });
        let settings = PreloadedUserSettings {
            status: Some(StatusSettings {
                status: None,
                custom_status: Some(cs.unwrap_or(CustomStatus {
                    text: String::new(),
                    emoji_id: 0,
                    emoji_name: String::new(),
                    expires_at_ms: 0,
                    created_at_ms: 0,
                })),
                show_current_game: None,
            }),
            ..Default::default()
        };
        self.inner.store.write().settings.merge_partial(settings.clone());
        let status = {
            let s = self.inner.store.read();
            s.status(s.my_id())
        };
        if let Some(gw) = self.gateway() {
            gw.update_presence(&UpdatePresence { status, since: 0, activities: self.presence_activities(), afk: false });
        }
        self.patch_settings(settings);
    }

    fn presence_activities(&self) -> Vec<Activity> {
        let s = self.inner.store.read();
        match s.settings.custom_status() {
            Some(cs) if !cs.text.is_empty() || !cs.emoji_name.is_empty() => vec![Activity {
                name: "Custom Status".into(),
                kind: activity_type::CUSTOM,
                state: Some(cs.text.clone()).filter(|t| !t.is_empty()),
                emoji: (!cs.emoji_name.is_empty()).then(|| PartialEmoji {
                    id: (cs.emoji_id != 0).then_some(Snowflake(cs.emoji_id)),
                    name: Some(cs.emoji_name.clone()),
                    animated: false,
                }),
                ..Default::default()
            }],
            _ => vec![],
        }
    }

    fn patch_settings(&self, settings: PreloadedUserSettings) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = client.inner.http.update_settings_proto(&settings.to_base64()).await {
                client.fail("update_settings", e);
            }
        });
    }

    pub fn set_nickname(&self, guild: Snowflake, nick: Option<String>) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = client.inner.http.set_nickname(guild, nick.as_deref()).await {
                client.fail("set_nickname", e);
            }
        });
    }

    pub fn set_guild_muted(&self, guild: Snowflake, muted: bool) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            let body = serde_json::json!({ "muted": muted, "mute_config": if muted { serde_json::json!({ "selected_time_window": -1, "end_time": null }) } else { Value::Null } });
            if let Err(e) = client.inner.http.update_guild_settings(Some(guild), body).await {
                client.fail("mute_guild", e);
            }
        });
    }

    pub fn set_channel_muted(&self, guild: Option<Snowflake>, channel: Snowflake, muted: bool) {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            let body = serde_json::json!({ "channel_overrides": { channel.to_string(): {
                "muted": muted,
                "mute_config": if muted { serde_json::json!({ "selected_time_window": -1, "end_time": null }) } else { Value::Null }
            }}});
            if let Err(e) = client.inner.http.update_guild_settings(guild, body).await {
                client.fail("mute_channel", e);
            }
        });
    }

    /// Runs any async closure on the client runtime, reporting failures as events.
    pub fn spawn_action<F>(&self, name: &'static str, f: impl FnOnce(Client) -> F + Send + 'static)
    where
        F: std::future::Future<Output = Result<(), HttpError>> + Send + 'static,
    {
        let client = self.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = f(client.clone()).await {
                client.fail(name, e);
            }
        });
    }

    pub fn shutdown(&self) {
        for e in self.extensions() {
            e.shutdown();
        }
        self.stop_gateway();
    }
}

fn guild_subscription(channel: Snowflake, ranges: Vec<[u32; 2]>) -> GuildSubscription {
    let mut channels = HashMap::new();
    channels.insert(channel, ranges);
    GuildSubscription {
        typing: Some(true),
        threads: Some(true),
        activities: Some(true),
        member_updates: None,
        members: None,
        channels,
        thread_member_lists: None,
    }
}

/// Effective notification level for a channel: 0 = all, 1 = mentions, 2 = nothing.
fn notification_level(store: &Store, channel: &Channel) -> u8 {
    let Some(gid) = channel.guild_id else { return 0 };
    let settings = store.guild_settings.get(&Some(gid));
    if let Some(s) = settings {
        let ov = s
            .channel_overrides
            .iter()
            .find(|o| o.channel_id == channel.id)
            .or_else(|| channel.parent_id.and_then(|p| s.channel_overrides.iter().find(|o| o.channel_id == p)));
        // 3 = inherit (NULL) in overrides.
        if let Some(o) = ov
            && o.message_notifications != 3
        {
            return o.message_notifications;
        }
        if s.message_notifications != 3 {
            return s.message_notifications;
        }
    }
    store.guilds.get(&gid).map(|g| g.props.default_message_notifications).unwrap_or(1)
}

pub fn error_text(e: &HttpError) -> String {
    match e {
        HttpError::Api { body, .. } => body.display_message(),
        other => other.to_string(),
    }
}
