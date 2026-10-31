//! Normalized, in-memory cache of everything the client knows.
//!
//! The store is a plain data structure: [`Store::apply`] folds a gateway
//! [`Event`] into it. Synchronization (locking) is the owner's job, which keeps
//! this crate trivially testable and free of async.

mod member_list;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use ferrocord_model::gateway::{MemberListItem, Ready, ReadySupplemental};
use ferrocord_model::settings::PreloadedUserSettings;
use ferrocord_model::*;
use serde_json::Value;

pub use member_list::MemberList;

/// Max messages kept per channel; older ones are dropped and re-fetched on scroll.
pub const MESSAGE_CACHE_LIMIT: usize = 600;
const TYPING_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub struct ChannelMessages {
    /// Ascending by id.
    pub messages: Vec<Message>,
    /// The oldest message in the channel has been loaded.
    pub reached_start: bool,
    /// The cache is contiguous up to the newest message (live events can be appended).
    pub has_latest: bool,
    pub loading: bool,
}

impl ChannelMessages {
    fn upsert(&mut self, msg: Message) {
        match self.messages.binary_search_by_key(&msg.id, |m| m.id) {
            Ok(i) => self.messages[i] = msg,
            Err(i) => self.messages.insert(i, msg),
        }
    }

    fn get_mut(&mut self, id: Snowflake) -> Option<&mut Message> {
        self.messages.binary_search_by_key(&id, |m| m.id).ok().map(|i| &mut self.messages[i])
    }

    fn trim(&mut self) {
        if self.messages.len() > MESSAGE_CACHE_LIMIT {
            let drop = self.messages.len() - MESSAGE_CACHE_LIMIT;
            self.messages.drain(..drop);
            self.reached_start = false;
        }
    }
}

/// Local state of a message the user is sending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingState {
    Sending,
    Failed(String),
}

/// An entry of the guild sidebar after applying the user's folder settings.
#[derive(Debug, Clone)]
pub enum GuildListEntry {
    Guild(Snowflake),
    Folder { id: i64, name: Option<String>, color: Option<u32>, guilds: Vec<Snowflake> },
}

#[derive(Debug, Default)]
pub struct Store {
    pub me: Option<CurrentUser>,
    pub session_id: Option<String>,
    pub users: HashMap<Snowflake, User>,
    pub guilds: HashMap<Snowflake, Guild>,
    /// Guild ids in the order received (fallback ordering).
    pub guild_order: Vec<Snowflake>,
    /// Every known channel: guild channels, threads, DMs and group DMs.
    pub channels: HashMap<Snowflake, Channel>,
    pub private_channels: HashSet<Snowflake>,
    pub messages: HashMap<Snowflake, ChannelMessages>,
    pub pending: HashMap<String, (Snowflake, PendingState)>,
    pub read_states: HashMap<Snowflake, ReadState>,
    pub guild_settings: HashMap<Option<Snowflake>, UserGuildSettings>,
    pub relationships: HashMap<Snowflake, Relationship>,
    /// Global presence per user (last received, any guild).
    pub presences: HashMap<Snowflake, Presence>,
    /// Members keyed by (guild, user).
    pub members: HashMap<(Snowflake, Snowflake), Member>,
    pub member_lists: HashMap<(Snowflake, String), MemberList>,
    /// Member list id most recently updated per guild (the subscribed channel's list).
    pub active_member_list: HashMap<Snowflake, String>,
    pub typing: HashMap<Snowflake, HashMap<Snowflake, Instant>>,
    pub voice_states: HashMap<Snowflake, VoiceState>,
    pub notes: HashMap<Snowflake, String>,
    pub settings: PreloadedUserSettings,
    pub sessions: Vec<Value>,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn my_id(&self) -> Snowflake {
        self.me.as_ref().map(|m| m.user.id).unwrap_or_default()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    // ---------------------------------------------------------------- apply

    /// Folds a gateway event into the cache.
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Ready(r) => self.apply_ready(r),
            Event::ReadySupplemental(s) => self.apply_supplemental(s),
            Event::Resumed => {}
            Event::MessageCreate(m) => self.on_message_create(m),
            Event::MessageUpdate(v) => self.on_message_update(v),
            Event::MessageDelete(d) => {
                if let Some(cm) = self.messages.get_mut(&d.channel_id) {
                    cm.messages.retain(|m| m.id != d.id);
                }
            }
            Event::MessageDeleteBulk(d) => {
                if let Some(cm) = self.messages.get_mut(&d.channel_id) {
                    let ids: HashSet<_> = d.ids.iter().copied().collect();
                    cm.messages.retain(|m| !ids.contains(&m.id));
                }
            }
            Event::ReactionAdd(r) => {
                let me = r.user_id == self.my_id();
                self.with_message(r.channel_id, r.message_id, |m| add_reaction(m, &r.emoji, me, 1));
            }
            Event::ReactionAddMany(r) => {
                let my = self.my_id();
                self.with_message(r.channel_id, r.message_id, |m| {
                    for dr in &r.reactions {
                        let me = dr.users.contains(&my);
                        add_reaction(m, &dr.emoji, me, dr.users.len() as u32);
                    }
                });
            }
            Event::ReactionRemove(r) => {
                let me = r.user_id == self.my_id();
                self.with_message(r.channel_id, r.message_id, |m| {
                    if let Some(i) = m.reactions.iter().position(|x| x.emoji == r.emoji) {
                        let x = &mut m.reactions[i];
                        x.count = x.count.saturating_sub(1);
                        if me {
                            x.me = false;
                        }
                        if x.count == 0 {
                            m.reactions.remove(i);
                        }
                    }
                });
            }
            Event::ReactionRemoveAll(r) => self.with_message(r.channel_id, r.message_id, |m| m.reactions.clear()),
            Event::ReactionRemoveEmoji(r) => {
                if let Some(e) = &r.emoji {
                    self.with_message(r.channel_id, r.message_id, |m| m.reactions.retain(|x| &x.emoji != e));
                }
            }
            Event::MessageAck(a) => {
                let rs = self.read_states.entry(a.channel_id).or_insert_with(|| ReadState { id: a.channel_id, ..Default::default() });
                rs.last_message_id = Some(a.message_id);
                rs.mention_count = a.mention_count.unwrap_or(0);
            }
            Event::TypingStart(t) => {
                if let Some(m) = &t.member
                    && let Some(g) = t.guild_id
                {
                    self.upsert_member(g, m.clone());
                }
                self.typing.entry(t.channel_id).or_default().insert(t.user_id, Instant::now());
            }
            Event::ChannelCreate(c) | Event::ChannelUpdate(c) | Event::ThreadCreate(c) | Event::ThreadUpdate(c) => {
                self.upsert_channel((**c).clone());
            }
            Event::ChannelDelete(c) | Event::ThreadDelete(c) => {
                self.channels.remove(&c.id);
                self.private_channels.remove(&c.id);
                self.messages.remove(&c.id);
            }
            Event::ThreadListSync(t) => {
                for c in &t.threads {
                    let mut c = c.clone();
                    c.guild_id.get_or_insert(t.guild_id);
                    self.upsert_channel(c);
                }
            }
            Event::ChannelPinsUpdate(p) => {
                if let Some(c) = self.channels.get_mut(&p.channel_id) {
                    c.last_pin_timestamp = p.last_pin_timestamp.clone();
                }
            }
            Event::ChannelRecipientAdd(r) => {
                self.users.insert(r.user.id, r.user.clone());
                if let Some(c) = self.channels.get_mut(&r.channel_id)
                    && !c.recipient_ids.contains(&r.user.id)
                {
                    c.recipient_ids.push(r.user.id);
                    c.recipients.push(r.user.clone());
                }
            }
            Event::ChannelRecipientRemove(r) => {
                if let Some(c) = self.channels.get_mut(&r.channel_id) {
                    c.recipient_ids.retain(|u| *u != r.user.id);
                    c.recipients.retain(|u| u.id != r.user.id);
                }
            }
            Event::GuildCreate(g) => self.insert_guild((**g).clone()),
            Event::GuildUpdate(v) => {
                if let Some(id) = v.get("id").and_then(|i| serde_json::from_value::<Snowflake>(i.clone()).ok())
                    && let Some(g) = self.guilds.get_mut(&id)
                    && let Ok(props) = serde_json::from_value::<GuildProperties>((**v).clone())
                {
                    g.props = props;
                    if let Some(roles) = v.get("roles").and_then(|r| serde_json::from_value(r.clone()).ok()) {
                        g.roles = roles;
                    }
                }
            }
            Event::GuildDelete(u) => {
                if u.unavailable {
                    if let Some(g) = self.guilds.get_mut(&u.id) {
                        g.unavailable = true;
                    }
                } else {
                    self.remove_guild(u.id);
                }
            }
            Event::GuildRoleCreate(r) | Event::GuildRoleUpdate(r) => {
                if let Some(g) = self.guilds.get_mut(&r.guild_id) {
                    match g.roles.iter_mut().find(|x| x.id == r.role.id) {
                        Some(x) => *x = r.role.clone(),
                        None => g.roles.push(r.role.clone()),
                    }
                }
            }
            Event::GuildRoleDelete(r) => {
                if let Some(g) = self.guilds.get_mut(&r.guild_id) {
                    g.roles.retain(|x| x.id != r.role_id);
                }
            }
            Event::GuildEmojisUpdate(e) => {
                if let Some(g) = self.guilds.get_mut(&e.guild_id) {
                    g.emojis = e.emojis.clone();
                }
            }
            Event::GuildMemberAdd(m) | Event::GuildMemberUpdate(m) => {
                if let Some(g) = m.guild_id {
                    self.upsert_member(g, (**m).clone());
                }
            }
            Event::GuildMemberRemove(r) => {
                self.members.remove(&(r.guild_id, r.user.id));
            }
            Event::GuildMembersChunk(c) => {
                for m in &c.members {
                    self.upsert_member(c.guild_id, m.clone());
                }
                for p in &c.presences {
                    self.upsert_presence(p.clone());
                }
            }
            Event::GuildMemberListUpdate(u) => {
                for op in &u.ops {
                    let items: Vec<&MemberListItem> = match op {
                        gateway::MemberListOp::Sync { items, .. } => items.iter().collect(),
                        gateway::MemberListOp::Insert { item, .. } | gateway::MemberListOp::Update { item, .. } => vec![item],
                        _ => vec![],
                    };
                    for it in items {
                        if let MemberListItem::Member(m) = it {
                            self.upsert_member(u.guild_id, m.member.clone());
                            if let Some(p) = &m.presence {
                                let mut p = p.clone();
                                if p.user_id().is_none() {
                                    p.user_id = m.member.user_id();
                                }
                                self.upsert_presence(p);
                            }
                        }
                    }
                }
                self.member_lists.entry((u.guild_id, u.id.clone())).or_default().apply(u);
                self.active_member_list.insert(u.guild_id, u.id.clone());
            }
            Event::PresenceUpdate(p) => self.upsert_presence((**p).clone()),
            Event::RelationshipAdd(r) | Event::RelationshipUpdate(r) => {
                if let Some(u) = &r.user {
                    self.users.insert(u.id, u.clone());
                }
                self.relationships.insert(r.id, (**r).clone());
            }
            Event::RelationshipRemove(r) => {
                self.relationships.remove(&r.id);
            }
            Event::UserUpdate(u) => {
                self.users.insert(u.user.id, u.user.clone());
                if self.me.as_ref().is_some_and(|m| m.user.id == u.user.id) {
                    self.me = Some((**u).clone());
                }
            }
            Event::UserSettingsProtoUpdate(s) => {
                if s.settings.kind == 1
                    && let Some(p) = PreloadedUserSettings::from_base64(&s.settings.proto)
                {
                    if s.partial {
                        self.settings.merge_partial(p);
                    } else {
                        self.settings = p;
                    }
                }
            }
            Event::UserGuildSettingsUpdate(s) => {
                self.guild_settings.insert(s.guild_id, (**s).clone());
            }
            Event::UserNoteUpdate { id, note } => {
                self.notes.insert(*id, note.clone());
            }
            Event::VoiceStateUpdate(v) => {
                if v.channel_id.is_some() {
                    self.voice_states.insert(v.user_id, (**v).clone());
                } else {
                    self.voice_states.remove(&v.user_id);
                }
            }
            Event::SessionsReplace(v) => {
                if let Some(a) = v.as_array() {
                    self.sessions = a.clone();
                }
            }
            Event::VoiceServerUpdate(_) | Event::Other { .. } => {}
        }
    }

    fn apply_ready(&mut self, r: &Ready) {
        self.clear();
        self.session_id = Some(r.session_id.clone());
        self.users.insert(r.user.user.id, r.user.user.clone());
        self.me = Some(r.user.clone());
        for u in &r.users {
            self.users.insert(u.id, u.clone());
        }
        for (i, g) in r.guilds.iter().enumerate() {
            self.insert_guild(g.clone());
            if let Some(members) = r.merged_members.get(i) {
                for m in members {
                    self.upsert_member(g.id, m.clone());
                }
            }
        }
        for c in &r.private_channels {
            self.upsert_channel(c.clone());
        }
        for rel in &r.relationships {
            if let Some(u) = &rel.user {
                self.users.insert(u.id, u.clone());
            }
            self.relationships.insert(rel.id, rel.clone());
        }
        for rs in r.read_state.clone().into_entries() {
            if rs.read_state_type.unwrap_or(0) == 0 {
                self.read_states.insert(rs.id, rs);
            }
        }
        for s in r.user_guild_settings.clone().into_entries() {
            self.guild_settings.insert(s.guild_id, s);
        }
        if let Some(p) = r.user_settings_proto.as_deref().and_then(PreloadedUserSettings::from_base64) {
            self.settings = p;
        }
        if let Some(n) = &r.notes {
            self.notes = n.clone();
        }
        self.sessions = r.sessions.clone();
        tracing::info!(guilds = self.guilds.len(), channels = self.channels.len(), users = self.users.len(), "READY applied");
    }

    fn apply_supplemental(&mut self, s: &ReadySupplemental) {
        for p in &s.merged_presences.friends {
            self.upsert_presence(p.clone());
        }
        for (i, list) in s.merged_presences.guilds.iter().enumerate() {
            let gid = s.guilds.get(i).map(|g| g.id);
            for p in list {
                let mut p = p.clone();
                if p.guild_id.is_none() {
                    p.guild_id = gid;
                }
                self.upsert_presence(p);
            }
        }
        for (i, members) in s.merged_members.iter().enumerate() {
            if let Some(g) = s.guilds.get(i) {
                for m in members {
                    self.upsert_member(g.id, m.clone());
                }
            }
        }
        for g in &s.guilds {
            for v in &g.voice_states {
                let mut v = v.clone();
                v.guild_id.get_or_insert(g.id);
                self.voice_states.insert(v.user_id, v);
            }
        }
        for c in &s.lazy_private_channels {
            self.upsert_channel(c.clone());
        }
    }

    fn insert_guild(&mut self, mut g: Guild) {
        for c in g.channels.drain(..).chain(g.threads.drain(..)) {
            let mut c = c;
            c.guild_id.get_or_insert(g.id);
            self.channels.insert(c.id, c);
        }
        for m in g.members.drain(..) {
            self.upsert_member(g.id, m);
        }
        for p in g.presences.drain(..) {
            self.upsert_presence(p);
        }
        for v in g.voice_states.drain(..) {
            let mut v = v;
            v.guild_id.get_or_insert(g.id);
            self.voice_states.insert(v.user_id, v);
        }
        if !self.guild_order.contains(&g.id) {
            self.guild_order.push(g.id);
        }
        self.guilds.insert(g.id, g);
    }

    fn remove_guild(&mut self, id: Snowflake) {
        self.guilds.remove(&id);
        self.guild_order.retain(|g| *g != id);
        let chans: Vec<_> = self.channels.values().filter(|c| c.guild_id == Some(id)).map(|c| c.id).collect();
        for c in chans {
            self.channels.remove(&c);
            self.messages.remove(&c);
        }
        self.members.retain(|(g, _), _| *g != id);
    }

    pub fn upsert_channel(&mut self, c: Channel) {
        for u in &c.recipients {
            self.users.insert(u.id, u.clone());
        }
        if c.kind.is_private() {
            self.private_channels.insert(c.id);
        }
        match self.channels.get_mut(&c.id) {
            // Updates may omit fields like last_message_id; keep what we know.
            Some(old) => {
                let last = old.last_message_id;
                *old = c;
                if old.last_message_id.is_none() {
                    old.last_message_id = last;
                }
            }
            None => {
                self.channels.insert(c.id, c);
            }
        }
    }

    pub fn upsert_member(&mut self, guild: Snowflake, mut m: Member) {
        if let Some(u) = m.user.take() {
            m.user_id = Some(u.id);
            // Member payloads carry full users; partial ones lack a username.
            if !u.username.is_empty() {
                self.users.insert(u.id, u);
            }
        }
        if let Some(uid) = m.user_id {
            m.guild_id = Some(guild);
            self.members.insert((guild, uid), m);
        }
    }

    pub fn upsert_presence(&mut self, p: Presence) {
        if let Some(u) = p.user.as_ref().and_then(|u| u.as_user()) {
            self.users.insert(u.id, u);
        }
        if let Some(uid) = p.user_id() {
            self.presences.insert(uid, p);
        }
    }

    fn with_message(&mut self, channel: Snowflake, id: Snowflake, f: impl FnOnce(&mut Message)) {
        if let Some(m) = self.messages.get_mut(&channel).and_then(|cm| cm.get_mut(id)) {
            f(m);
        }
    }

    fn on_message_create(&mut self, m: &Message) {
        let my_id = self.my_id();
        self.users.insert(m.author.id, m.author.clone());
        for u in &m.mentions {
            self.users.entry(u.id).or_insert_with(|| u.clone());
        }
        if let (Some(g), Some(mem)) = (m.guild_id, &m.member) {
            let mut mem = mem.clone();
            mem.user_id = Some(m.author.id);
            self.upsert_member(g, mem);
        }
        if let Some(c) = self.channels.get_mut(&m.channel_id) {
            c.last_message_id = Some(m.id);
            if let Some(n) = c.message_count.as_mut() {
                *n += 1;
            }
        }
        if let Some(t) = self.typing.get_mut(&m.channel_id) {
            t.remove(&m.author.id);
        }
        // Resolve our optimistic local echo.
        if let Some(nonce) = m.nonce.as_ref().and_then(nonce_str)
            && let Some((_, _)) = self.pending.remove(&nonce)
            && let Some(cm) = self.messages.get_mut(&m.channel_id)
            && let Ok(local) = nonce.parse::<u64>()
        {
            cm.messages.retain(|x| x.id.0 != local);
        }
        if m.author.id == my_id {
            let rs = self.read_states.entry(m.channel_id).or_insert_with(|| ReadState { id: m.channel_id, ..Default::default() });
            rs.last_message_id = Some(m.id);
            rs.mention_count = 0;
        } else if self.is_mentioned(m) {
            let rs = self.read_states.entry(m.channel_id).or_insert_with(|| ReadState { id: m.channel_id, ..Default::default() });
            rs.mention_count += 1;
        }
        if let Some(cm) = self.messages.get_mut(&m.channel_id)
            && cm.has_latest
        {
            cm.upsert(m.clone());
            cm.trim();
        }
    }

    fn on_message_update(&mut self, v: &Value) {
        let (Some(id), Some(ch)) = (
            v.get("id").and_then(|x| serde_json::from_value::<Snowflake>(x.clone()).ok()),
            v.get("channel_id").and_then(|x| serde_json::from_value::<Snowflake>(x.clone()).ok()),
        ) else {
            return;
        };
        self.with_message(ch, id, |m| {
            // Merge the partial update over the cached message.
            if let Ok(mut cur) = serde_json::to_value(&*m)
                && let (Some(obj), Some(upd)) = (cur.as_object_mut(), v.as_object())
            {
                for (k, val) in upd {
                    obj.insert(k.clone(), val.clone());
                }
                if let Ok(merged) = serde_json::from_value::<Message>(cur) {
                    *m = merged;
                }
            }
        });
    }

    // ---------------------------------------------------------------- messages

    /// Inserts a page of history fetched over REST.
    pub fn insert_history(&mut self, channel: Snowflake, mut page: Vec<Message>, requested: usize, latest: bool) {
        for m in &page {
            self.users.insert(m.author.id, m.author.clone());
            if let (Some(g), Some(mem)) = (m.guild_id, &m.member) {
                let mut mem = mem.clone();
                mem.user_id = Some(m.author.id);
                self.upsert_member(g, mem);
            }
        }
        let cm = self.messages.entry(channel).or_default();
        if page.len() < requested {
            cm.reached_start = true;
        }
        if latest {
            cm.has_latest = true;
        }
        page.sort_by_key(|m| m.id);
        for m in page {
            cm.upsert(m);
        }
        cm.loading = false;
    }

    /// Adds an optimistic local echo for a message being sent.
    pub fn add_pending(&mut self, channel: Snowflake, nonce: &str, content: &str, reply_to: Option<Snowflake>) {
        let Some(me) = self.me.clone() else { return };
        let id = Snowflake(nonce.parse().unwrap_or(0));
        let msg = Message {
            id,
            channel_id: channel,
            author: me.user,
            content: content.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            nonce: Some(Value::String(nonce.to_string())),
            message_reference: reply_to.map(|r| MessageReference { message_id: Some(r), channel_id: Some(channel), ..Default::default() }),
            referenced_message: reply_to.and_then(|r| self.message(channel, r).cloned().map(Box::new)),
            ..Default::default()
        };
        self.pending.insert(nonce.to_string(), (channel, PendingState::Sending));
        let cm = self.messages.entry(channel).or_default();
        cm.upsert(msg);
    }

    pub fn fail_pending(&mut self, nonce: &str, error: String) {
        if let Some(p) = self.pending.get_mut(nonce) {
            p.1 = PendingState::Failed(error);
        }
    }

    pub fn remove_pending(&mut self, nonce: &str) {
        if let Some((ch, _)) = self.pending.remove(nonce)
            && let (Some(cm), Ok(id)) = (self.messages.get_mut(&ch), nonce.parse::<u64>())
        {
            cm.messages.retain(|m| m.id.0 != id);
        }
    }

    pub fn pending_state(&self, m: &Message) -> Option<&PendingState> {
        let n = m.nonce.as_ref().and_then(nonce_str)?;
        if n.parse::<u64>().ok()? != m.id.0 {
            return None;
        }
        self.pending.get(&n).map(|(_, s)| s)
    }

    pub fn message(&self, channel: Snowflake, id: Snowflake) -> Option<&Message> {
        let cm = self.messages.get(&channel)?;
        cm.messages.binary_search_by_key(&id, |m| m.id).ok().map(|i| &cm.messages[i])
    }

    /// Upserts a single message (e.g. after an edit over REST).
    pub fn put_message(&mut self, m: Message) {
        if let Some(cm) = self.messages.get_mut(&m.channel_id) {
            cm.upsert(m);
        }
    }

    // ---------------------------------------------------------------- queries

    pub fn user(&self, id: Snowflake) -> Option<&User> {
        self.users.get(&id)
    }

    pub fn member(&self, guild: Snowflake, user: Snowflake) -> Option<&Member> {
        self.members.get(&(guild, user))
    }

    pub fn status(&self, user: Snowflake) -> Status {
        if user == self.my_id() {
            return self.settings.status().and_then(|s| serde_json::from_value(Value::String(s.into())).ok()).unwrap_or(Status::Online);
        }
        self.presences.get(&user).map(|p| p.status).unwrap_or(Status::Offline)
    }

    /// Name shown for a user in a guild context: nick > global name > username.
    pub fn display_name(&self, guild: Option<Snowflake>, user: Snowflake) -> String {
        if let Some(g) = guild
            && let Some(nick) = self.member(g, user).and_then(|m| m.nick.clone())
        {
            return nick;
        }
        if let Some(r) = self.relationships.get(&user).and_then(|r| r.nickname.clone()) {
            return r;
        }
        self.user(user).map(|u| u.display_name().to_string()).unwrap_or_else(|| format!("<@{user}>"))
    }

    /// Color of the member's highest colored role.
    pub fn member_color(&self, guild: Snowflake, user: Snowflake) -> Option<u32> {
        let g = self.guilds.get(&guild)?;
        let m = self.member(guild, user)?;
        g.roles.iter().filter(|r| m.roles.contains(&r.id) && r.color != 0).max_by_key(|r| r.position).map(|r| r.color)
    }

    pub fn my_roles(&self, guild: Snowflake) -> Vec<Snowflake> {
        self.member(guild, self.my_id()).map(|m| m.roles.clone()).unwrap_or_default()
    }

    /// Effective permissions of the current user in a channel (threads inherit their parent).
    pub fn permissions(&self, channel: Snowflake) -> Permissions {
        let Some(c) = self.channels.get(&channel) else { return Permissions::empty() };
        let Some(gid) = c.guild_id else { return Permissions::all() };
        let Some(g) = self.guilds.get(&gid) else { return Permissions::empty() };
        let target = if c.kind.is_thread() { c.parent_id.and_then(|p| self.channels.get(&p)).unwrap_or(c) } else { c };
        channel_permissions(g, target, self.my_id(), &self.my_roles(gid))
    }

    pub fn can_view(&self, channel: Snowflake) -> bool {
        self.permissions(channel).contains(Permissions::VIEW_CHANNEL)
    }

    /// Guild channels sorted like the official sidebar: uncategorized first,
    /// then categories with their children; text before voice within a group.
    pub fn guild_channel_tree(&self, guild: Snowflake) -> Vec<(Option<Channel>, Vec<Channel>)> {
        let mut cats: Vec<&Channel> = vec![];
        let mut by_parent: HashMap<Option<Snowflake>, Vec<&Channel>> = HashMap::new();
        for c in self.channels.values().filter(|c| c.guild_id == Some(guild) && !c.kind.is_thread()) {
            if c.kind == ChannelType::GuildCategory {
                cats.push(c);
            } else if self.can_view(c.id) {
                by_parent.entry(c.parent_id).or_default().push(c);
            }
        }
        let sort = |v: &mut Vec<&Channel>| {
            v.sort_by_key(|c| (c.kind.is_voice(), c.position, c.id));
        };
        cats.sort_by_key(|c| (c.position, c.id));
        let mut out = vec![];
        let mut top = by_parent.remove(&None).unwrap_or_default();
        sort(&mut top);
        if !top.is_empty() {
            out.push((None, top.into_iter().cloned().collect()));
        }
        for cat in cats {
            let mut kids = by_parent.remove(&Some(cat.id)).unwrap_or_default();
            if kids.is_empty() && !self.can_view(cat.id) {
                continue;
            }
            sort(&mut kids);
            out.push((Some(cat.clone()), kids.into_iter().cloned().collect()));
        }
        out
    }

    /// Active (non-archived) threads under a channel.
    pub fn threads_of(&self, parent: Snowflake) -> Vec<&Channel> {
        let mut v: Vec<_> = self
            .channels
            .values()
            .filter(|c| c.parent_id == Some(parent) && c.kind.is_thread() && !c.thread_metadata.as_ref().is_some_and(|t| t.archived))
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.unwrap_or(c.id)));
        v
    }

    /// DMs and group DMs, most recent activity first.
    pub fn sorted_private_channels(&self) -> Vec<&Channel> {
        let mut v: Vec<&Channel> = self.private_channels.iter().filter_map(|id| self.channels.get(id)).collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.unwrap_or(c.id)));
        v
    }

    /// Guild sidebar order from the user's folder settings, with unlisted guilds first.
    pub fn guild_list(&self) -> Vec<GuildListEntry> {
        let mut seen = HashSet::new();
        let mut entries = vec![];
        if let Some(f) = &self.settings.guild_folders {
            for folder in &f.folders {
                let ids: Vec<Snowflake> = folder.guild_ids.iter().map(|g| Snowflake(*g)).filter(|g| self.guilds.contains_key(g)).collect();
                if ids.is_empty() {
                    continue;
                }
                seen.extend(ids.iter().copied());
                match &folder.id {
                    Some(id) if id.value != 0 || ids.len() > 1 => entries.push(GuildListEntry::Folder {
                        id: id.value,
                        name: folder.name.as_ref().map(|n| n.value.clone()).filter(|n| !n.is_empty()),
                        color: folder.color.as_ref().map(|c| c.value as u32),
                        guilds: ids,
                    }),
                    _ => entries.extend(ids.into_iter().map(GuildListEntry::Guild)),
                }
            }
        }
        let mut rest: Vec<Snowflake> = self.guild_order.iter().copied().filter(|g| !seen.contains(g)).collect();
        // New guilds (not yet placed in settings) appear at the top, newest first.
        rest.sort_by_key(|g| std::cmp::Reverse(self.guilds.get(g).and_then(|g| g.joined_at.clone())));
        let mut out: Vec<GuildListEntry> = rest.into_iter().map(GuildListEntry::Guild).collect();
        out.extend(entries);
        out
    }

    // ---------------------------------------------------------------- unread

    pub fn is_muted(&self, channel: Snowflake) -> bool {
        let Some(c) = self.channels.get(&channel) else { return false };
        let s = self.guild_settings.get(&c.guild_id);
        let now = chrono::Utc::now();
        let active = |muted: bool, cfg: &Option<MuteConfig>| {
            muted && cfg.as_ref().and_then(|c| c.end_time.as_ref()).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_none_or(|end| end > now)
        };
        let Some(s) = s else { return false };
        let check = |id: Snowflake| s.channel_overrides.iter().any(|o| o.channel_id == id && active(o.muted, &o.mute_config));
        check(channel) || c.parent_id.is_some_and(check)
    }

    pub fn is_guild_muted(&self, guild: Snowflake) -> bool {
        self.guild_settings.get(&Some(guild)).is_some_and(|s| s.muted)
    }

    pub fn is_unread(&self, channel: Snowflake) -> bool {
        let Some(c) = self.channels.get(&channel) else { return false };
        if !c.kind.is_text_based() || c.kind.is_voice() {
            return false;
        }
        let Some(last) = c.last_message_id else { return false };
        match self.read_states.get(&channel).and_then(|r| r.last_message_id) {
            Some(read) => last > read,
            // Never-opened guild channels are not considered unread by the client.
            None => c.kind.is_private(),
        }
    }

    pub fn mention_count(&self, channel: Snowflake) -> u32 {
        self.read_states.get(&channel).map(|r| r.mention_count).unwrap_or(0)
    }

    pub fn guild_unread(&self, guild: Snowflake) -> (bool, u32) {
        let mut unread = false;
        let mut mentions = 0;
        let muted_guild = self.is_guild_muted(guild);
        for c in self.channels.values().filter(|c| c.guild_id == Some(guild)) {
            if !c.kind.is_text_based() {
                continue;
            }
            mentions += self.mention_count(c.id);
            if !unread && !muted_guild && !self.is_muted(c.id) && self.is_unread(c.id) && self.can_view(c.id) {
                unread = true;
            }
        }
        (unread, mentions)
    }

    /// Total unread DMs + mentions, for the taskbar badge.
    pub fn badge_count(&self) -> u32 {
        let dms: u32 = self.private_channels.iter().map(|c| self.mention_count(*c)).sum();
        let guilds: u32 = self.guilds.keys().map(|g| self.guild_unread(*g).1).sum();
        dms + guilds
    }

    pub fn is_mentioned(&self, m: &Message) -> bool {
        let me = self.my_id();
        // Blocked users never ping us (same as the official client).
        if m.author.id == me || self.is_blocked(m.author.id) {
            return false;
        }
        if m.mentions_user(me) {
            return true;
        }
        let channel = self.channels.get(&m.channel_id);
        if channel.is_some_and(|c| c.kind.is_private()) {
            return true;
        }
        let Some(gid) = m.guild_id.or_else(|| channel.and_then(|c| c.guild_id)) else { return false };
        let settings = self.guild_settings.get(&Some(gid));
        if m.mention_everyone && !settings.is_some_and(|s| s.suppress_everyone) {
            return true;
        }
        if !m.mention_roles.is_empty() && !settings.is_some_and(|s| s.suppress_roles) {
            let roles = self.my_roles(gid);
            return m.mention_roles.iter().any(|r| roles.contains(r));
        }
        false
    }

    /// Users currently typing in a channel (expired entries are pruned).
    /// Blocked users are left out.
    pub fn typing_users(&mut self, channel: Snowflake) -> Vec<Snowflake> {
        let me = self.my_id();
        let relationships = &self.relationships;
        let blocked = |u: &Snowflake| relationships.get(u).is_some_and(|r| r.kind == RelationshipType::Blocked);
        match self.typing.get_mut(&channel) {
            Some(t) => {
                t.retain(|_, at| at.elapsed() < TYPING_TTL);
                t.keys().copied().filter(|u| *u != me && !blocked(u)).collect()
            }
            None => vec![],
        }
    }

    pub fn member_list(&self, guild: Snowflake) -> Option<&MemberList> {
        let id = self.active_member_list.get(&guild)?;
        self.member_lists.get(&(guild, id.clone()))
    }

    pub fn friends(&self) -> Vec<&Relationship> {
        self.relationships.values().filter(|r| r.kind == RelationshipType::Friend).collect()
    }

    /// We have blocked this user.
    pub fn is_blocked(&self, user: Snowflake) -> bool {
        self.relationships.get(&user).is_some_and(|r| r.kind == RelationshipType::Blocked)
    }

    /// This user is on our friends list.
    pub fn is_friend(&self, user: Snowflake) -> bool {
        self.relationships.get(&user).is_some_and(|r| r.kind == RelationshipType::Friend)
    }
}

fn nonce_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn add_reaction(m: &mut Message, emoji: &PartialEmoji, me: bool, n: u32) {
    match m.reactions.iter_mut().find(|r| r.emoji.id == emoji.id && (emoji.id.is_some() || r.emoji.name == emoji.name)) {
        Some(r) => {
            r.count += n;
            r.me |= me;
        }
        None => m.reactions.push(Reaction { count: n, me, emoji: emoji.clone(), ..Default::default() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ready() -> Event {
        let v = json!({
            "v": 9, "session_id": "s",
            "user": {"id": "1", "username": "me", "discriminator": "0"},
            "users": [{"id": "2", "username": "friend", "discriminator": "0"}],
            "guilds": [{
                "id": "100",
                "properties": {"name": "Test Guild", "owner_id": "9"},
                "roles": [{"id": "100", "name": "@everyone", "permissions": "1024", "position": 0}],
                "channels": [
                    {"id": "200", "type": 0, "name": "general", "position": 0, "last_message_id": "500"},
                    {"id": "201", "type": 4, "name": "Cat", "position": 0},
                    {"id": "202", "type": 0, "name": "in-cat", "position": 0, "parent_id": "201"}
                ]
            }],
            "merged_members": [[{"user_id": "1", "roles": []}]],
            "private_channels": [{"id": "300", "type": 1, "recipient_ids": ["2"], "last_message_id": "600"}],
            "read_state": {"entries": [{"id": "200", "last_message_id": "400", "mention_count": 0}], "partial": false, "version": 1},
            "user_guild_settings": {"entries": [], "partial": false, "version": 1},
            "relationships": [{"id": "2", "type": 1, "user_id": "2"}]
        });
        let (ev, err) = Event::parse("READY", &v);
        assert!(err.is_none(), "{err:?}");
        ev
    }

    #[test]
    fn ready_and_unreads() {
        let mut s = Store::new();
        s.apply(&ready());
        assert_eq!(s.my_id(), Snowflake(1));
        assert_eq!(s.guilds[&Snowflake(100)].name(), "Test Guild");
        assert!(s.is_unread(Snowflake(200)));
        assert!(s.is_unread(Snowflake(300)));
        assert!(s.can_view(Snowflake(200)));
        let tree = s.guild_channel_tree(Snowflake(100));
        assert_eq!(tree.len(), 2);
        assert_eq!(s.friends().len(), 1);

        let (ack, _) = Event::parse("MESSAGE_ACK", &json!({"channel_id": "200", "message_id": "500", "version": 2}));
        s.apply(&ack);
        assert!(!s.is_unread(Snowflake(200)));
    }

    #[test]
    fn messages_and_pending() {
        let mut s = Store::new();
        s.apply(&ready());
        s.insert_history(Snowflake(200), vec![], 50, true);
        s.add_pending(Snowflake(200), "123456", "hello", None);
        assert_eq!(s.messages[&Snowflake(200)].messages.len(), 1);
        let (mc, err) = Event::parse(
            "MESSAGE_CREATE",
            &json!({"id": "999", "channel_id": "200", "guild_id": "100", "nonce": "123456", "content": "hello",
                    "author": {"id": "1", "username": "me"}, "timestamp": "2026-01-01T00:00:00Z", "mentions": []}),
        );
        assert!(err.is_none());
        s.apply(&mc);
        let msgs = &s.messages[&Snowflake(200)].messages;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, Snowflake(999));

        let (upd, _) =
            Event::parse("MESSAGE_UPDATE", &json!({"id": "999", "channel_id": "200", "content": "edited", "edited_timestamp": "2026-01-01T00:01:00Z"}));
        s.apply(&upd);
        assert_eq!(s.message(Snowflake(200), Snowflake(999)).unwrap().content, "edited");

        let (r, _) =
            Event::parse("MESSAGE_REACTION_ADD", &json!({"user_id": "2", "channel_id": "200", "message_id": "999", "emoji": {"id": null, "name": "👍"}}));
        s.apply(&r);
        assert_eq!(s.message(Snowflake(200), Snowflake(999)).unwrap().reactions[0].count, 1);
    }

    #[test]
    fn blocked_users() {
        let mut s = Store::new();
        let Event::Ready(mut r) = ready() else { unreachable!() };
        r.relationships.push(serde_json::from_value(json!({"id": "3", "type": 2, "user_id": "3"})).unwrap());
        s.apply(&Event::Ready(r));
        assert!(s.is_blocked(Snowflake(3)));
        assert!(!s.is_blocked(Snowflake(2)));
        assert!(s.is_friend(Snowflake(2)));
        assert!(!s.is_blocked(Snowflake(4)));

        // Blocked users don't ping us, even in DMs.
        let dm = |author: &str, id: &str| {
            let (ev, err) = Event::parse(
                "MESSAGE_CREATE",
                &json!({"id": id, "channel_id": "300", "content": "hi", "author": {"id": author, "username": "x"},
                        "timestamp": "2026-01-01T00:00:00Z", "mentions": []}),
            );
            assert!(err.is_none(), "{err:?}");
            ev
        };
        s.apply(&dm("3", "700"));
        assert_eq!(s.mention_count(Snowflake(300)), 0);
        s.apply(&dm("2", "701"));
        assert_eq!(s.mention_count(Snowflake(300)), 1);

        // Typing from blocked users is filtered.
        for user in ["2", "3"] {
            let (t, err) = Event::parse("TYPING_START", &json!({"channel_id": "300", "user_id": user, "timestamp": 0}));
            assert!(err.is_none(), "{err:?}");
            s.apply(&t);
        }
        assert_eq!(s.typing_users(Snowflake(300)), vec![Snowflake(2)]);

        // Relationship events: blocking a friend replaces the friendship, removal unblocks.
        let (add, err) = Event::parse("RELATIONSHIP_ADD", &json!({"id": "2", "type": 2, "user": {"id": "2", "username": "friend"}}));
        assert!(err.is_none(), "{err:?}");
        s.apply(&add);
        assert!(s.is_blocked(Snowflake(2)));
        assert!(!s.is_friend(Snowflake(2)));
        assert!(s.typing_users(Snowflake(300)).is_empty());
        let (rm, err) = Event::parse("RELATIONSHIP_REMOVE", &json!({"id": "3", "type": 2}));
        assert!(err.is_none(), "{err:?}");
        s.apply(&rm);
        assert!(!s.is_blocked(Snowflake(3)));
    }
}
