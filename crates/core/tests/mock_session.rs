//! Full `ferrocord-core` sessions against the mock Discord: READY → store,
//! history paging, local echo, acks, presence/settings, reactions, extensions,
//! notifications and logout. `persist_credentials` is off, so the OS credential
//! vault is never touched; config files go to a temp dir.

use std::sync::Arc;
use std::time::Duration;

use ferrocord_core::model::gateway::GuildSubscription;
use ferrocord_core::model::settings::PreloadedUserSettings;
use ferrocord_core::model::{Event, PartialEmoji, Snowflake, Status};
use ferrocord_core::store::{GuildListEntry, PendingState};
use ferrocord_core::{Client, ClientEvent, ClientOptions, ConnectionState, Extension, Flow, LocalCommand, OutgoingMessage, Paths};
use ferrocord_testkit::MockDiscord;
use ferrocord_testkit::fixtures::{MessageBuilder, UserBuilder, ids};
use ferrocord_testkit::{MockResponse, RecordedRequest};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::broadcast;

const G: Snowflake = Snowflake(ids::GUILD);
const GENERAL: Snowflake = Snowflake(ids::GENERAL);
const DM: Snowflake = Snowflake(ids::DM);
const ME: Snowflake = Snowflake(ids::ME);

struct Env {
    mock: MockDiscord,
    client: Client,
    ev: Events,
    _dir: tempfile::TempDir,
}

/// The client's event broadcast, kept separate so tests can borrow `mock`/`client` alongside it.
struct Events(broadcast::Receiver<ClientEvent>);

impl Events {
    async fn event(&mut self, what: &str, pred: impl Fn(&ClientEvent) -> bool) -> ClientEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, self.0.recv()).await {
                Ok(Ok(ev)) if pred(&ev) => return ev,
                Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(e)) => panic!("event channel closed: {e}"),
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }

    async fn dispatch(&mut self, name: &str) -> (Arc<Event>, Arc<Value>) {
        let n = name.to_string();
        match self.event(name, |e| matches!(e, ClientEvent::Dispatch { name, .. } if *name == n)).await {
            ClientEvent::Dispatch { event, raw, .. } => (event, raw),
            _ => unreachable!(),
        }
    }

    async fn messages_loaded(&mut self, channel: Snowflake) {
        self.event("MessagesLoaded", |e| matches!(e, ClientEvent::MessagesLoaded { channel_id } if *channel_id == channel)).await;
    }
}

impl Env {
    async fn new() -> Self {
        let mock = MockDiscord::start().await;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data"), cache_dir: dir.path().join("cache") };
        let opts = ClientOptions::custom_endpoints(mock.api_base(), mock.gateway_url());
        let client = Client::with_options(tokio::runtime::Handle::current(), paths, opts);
        let ev = Events(client.subscribe());
        Self { mock, client, ev, _dir: dir }
    }

    /// Starts a session and waits until READY and READY_SUPPLEMENTAL are applied.
    async fn connected(self) -> Self {
        let mut env = self;
        env.client.start_session(env.mock.token());
        env.ev.dispatch("READY").await;
        env.ev.dispatch("READY_SUPPLEMENTAL").await;
        env
    }

    /// Polls the client until `f` holds.
    async fn until(&self, what: &str, f: impl Fn(&Client) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !f(&self.client) {
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

fn message_ids(c: &Client, channel: Snowflake) -> Vec<u64> {
    c.store().messages.get(&channel).map(|cm| cm.messages.iter().map(|m| m.id.0).collect()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_populates_the_store() {
    let env = Env::new().await.connected().await;
    let c = &env.client;
    assert_eq!(c.connection_state(), ConnectionState::Connected);
    {
        let s = c.store();
        assert_eq!(s.my_id(), ME);
        assert_eq!(s.session_id.as_deref(), Some("mock-session-1"));
        assert_eq!(s.guilds[&G].name(), "Mock Guild");
        assert_eq!(s.channels.len(), 5, "4 guild channels + 1 DM");
        assert!(s.private_channels.contains(&DM));
        assert_eq!(s.read_states.len(), 2);
        assert!(s.is_unread(GENERAL));
        assert!(s.is_unread(DM));
        assert_eq!(s.mention_count(DM), 1);
        assert_eq!(s.friends().len(), 1);
        assert_eq!(s.member(G, ME).unwrap().roles, vec![Snowflake(ids::ROLE_MOD)]);
        assert_eq!(s.user(Snowflake(ids::FRIEND)).unwrap().display_name(), "Friendly");
        assert_eq!(s.status(Snowflake(ids::FRIEND)), Status::Online, "presence from READY_SUPPLEMENTAL");
        assert_eq!(s.status(ME), Status::Online, "own status from the settings proto");
        match s.guild_list().as_slice() {
            [GuildListEntry::Folder { id, name, guilds, .. }] => {
                assert_eq!(*id, ids::FOLDER);
                assert_eq!(name.as_deref(), Some("Mock Folder"));
                assert_eq!(guilds, &vec![G]);
            }
            other => panic!("unexpected guild list {other:?}"),
        }
        let tree = s.guild_channel_tree(G);
        assert_eq!(tree.len(), 2, "uncategorized voice + the category");
    }
    assert_eq!(c.config().last_account, Some(ME.to_string()));
    assert_eq!(env.mock.gateway().wait_for_op(2).await.d["token"], env.mock.token());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_session() {
    let env = Env::new().await;
    let seeded = env.mock.with_world(|w| w.seed_messages(ids::GENERAL, 120, &UserBuilder::friend()));
    let mut env = env.connected().await;
    let (mock, client) = (&env.mock, env.client.clone());

    // --- open the channel: lazy subscription + latest page
    client.open_channel(GENERAL);
    env.ev.messages_loaded(GENERAL).await;
    let sub = mock.gateway().wait_for_op(37).await;
    let mut expected = GuildSubscription { typing: Some(true), threads: Some(true), activities: Some(true), ..Default::default() };
    expected.channels.insert(GENERAL, vec![[0, 99]]);
    assert_eq!(sub.d["subscriptions"][ids::GUILD.to_string()], serde_json::to_value(expected).unwrap());
    {
        let s = client.store();
        let cm = &s.messages[&GENERAL];
        assert_eq!(cm.messages.len(), 50);
        assert!(cm.has_latest && !cm.reached_start);
        assert_eq!(cm.messages.last().unwrap().id.0, seeded[119]);
    }

    // --- page back to the start
    for _ in 0..2 {
        let oldest = client.store().messages[&GENERAL].messages[0].id;
        client.load_messages(GENERAL, Some(oldest));
        env.ev.messages_loaded(GENERAL).await;
    }
    assert_eq!(message_ids(&client, GENERAL), seeded, "all history, ascending, no duplicates");
    assert!(client.store().messages[&GENERAL].reached_start);
    client.load_messages(GENERAL, Some(Snowflake(seeded[0])));
    assert_eq!(mock.requests_to("GET", "/channels/*/messages").len(), 3, "no request once the start is reached");
    let pages = mock.requests_to("GET", "/channels/*/messages");
    assert_eq!(pages[1].query_param("before"), Some(seeded[70].to_string().as_str()));

    // --- send: local echo, then resolved by MESSAGE_CREATE with our nonce
    mock.delay_route("POST", "/channels/*/messages", Duration::from_millis(150));
    client.send_message(GENERAL, "hello world".into(), None, false, vec![]);
    let nonce = {
        let s = client.store();
        let echo = s.messages[&GENERAL].messages.last().unwrap();
        assert_eq!(echo.content, "hello world");
        assert_eq!(echo.author.id, ME);
        assert_eq!(s.pending_state(echo), Some(&PendingState::Sending));
        echo.nonce.as_ref().and_then(Value::as_str).unwrap().to_string()
    };
    let (event, raw) = env.ev.dispatch("MESSAGE_CREATE").await;
    assert_eq!(raw["nonce"], nonce.as_str());
    let Event::MessageCreate(created) = &*event else { panic!() };
    let sent_id = created.id;
    {
        let s = client.store();
        assert!(s.pending.is_empty());
        let msgs = &s.messages[&GENERAL].messages;
        assert_eq!(msgs.iter().filter(|m| m.content == "hello world").count(), 1, "echo replaced, not duplicated");
        assert_eq!(msgs.last().unwrap().id, sent_id);
        assert_eq!(s.channels[&GENERAL].last_message_id, Some(sent_id));
    }
    let post = mock.wait_for_request("POST", "/channels/*/messages").await.json();
    assert_eq!(post["content"], "hello world");
    assert_eq!(post["nonce"], nonce.as_str());
    assert_eq!(post["enforce_nonce"], true);
    assert_eq!(post["mobile_network_type"], "unknown");

    // --- ack: our own message already counts as read, so a friend's reply is needed
    client.ack_channel(GENERAL);
    let friend_msg = mock.with_world(|w| {
        let id = w.next_snowflake();
        let m = MessageBuilder::new(id, ids::GENERAL, &UserBuilder::friend()).guild(ids::GUILD).content("welcome back");
        w.insert_message(m.build());
        (id, m.build_event())
    });
    mock.gateway().dispatch("MESSAGE_CREATE", friend_msg.1);
    env.ev.dispatch("MESSAGE_CREATE").await;
    assert!(client.store().is_unread(GENERAL));
    client.ack_channel(GENERAL);
    let ack = mock.wait_for_request("POST", &format!("/channels/{GENERAL}/messages/{}/ack", friend_msg.0)).await;
    assert_eq!(mock.requests_to("POST", "/channels/*/messages/*/ack").len(), 1, "acking an already-read channel sends nothing");
    assert_eq!(ack.json()["token"], Value::Null);
    assert!(ack.json()["last_viewed"].is_u64());
    env.ev.dispatch("MESSAGE_ACK").await;
    assert!(!client.store().is_unread(GENERAL));

    // --- status: op 3 + settings proto PATCH
    client.set_status(Status::Dnd);
    let presence = mock.gateway().wait_for_op(3).await;
    assert_eq!(presence.d["status"], "dnd");
    let patch = mock.wait_for_request("PATCH", "/users/@me/settings-proto/1").await;
    let sent = PreloadedUserSettings::from_base64(patch.json()["settings"].as_str().unwrap()).unwrap();
    assert_eq!(sent.status(), Some("dnd"));
    assert!(sent.guild_folders.is_none(), "partial proto: only the changed field");
    env.ev.dispatch("USER_SETTINGS_PROTO_UPDATE").await;
    assert_eq!(client.store().status(ME), Status::Dnd);
    assert!(client.store().settings.guild_folders.is_some(), "partial update merged, folders kept");
    assert_eq!(mock.with_world(|w| w.settings.status().map(str::to_string)), Some("dnd".into()));

    // --- reaction toggle
    let thumbs = PartialEmoji::unicode("👍");
    client.toggle_reaction(GENERAL, sent_id, thumbs.clone());
    env.ev.dispatch("MESSAGE_REACTION_ADD").await;
    {
        let s = client.store();
        let r = &s.message(GENERAL, sent_id).unwrap().reactions;
        assert_eq!((r.len(), r[0].count, r[0].me), (1, 1, true));
    }
    client.toggle_reaction(GENERAL, sent_id, thumbs);
    env.ev.dispatch("MESSAGE_REACTION_REMOVE").await;
    assert!(client.store().message(GENERAL, sent_id).unwrap().reactions.is_empty());
    let del = mock.wait_for_request("DELETE", "/channels/*/messages/*/reactions/**").await;
    assert!(del.path.ends_with("/reactions/%F0%9F%91%8D/0/@me"), "{}", del.path);

    // --- edit, typing (throttled), delete
    client.edit_message(GENERAL, sent_id, "edited".into());
    env.ev.dispatch("MESSAGE_UPDATE").await;
    assert_eq!(client.store().message(GENERAL, sent_id).unwrap().content, "edited");
    client.typing(GENERAL);
    client.typing(GENERAL);
    env.ev.dispatch("TYPING_START").await;
    assert_eq!(mock.requests_to("POST", "/channels/*/typing").len(), 1, "typing is throttled");
    client.delete_message(GENERAL, sent_id);
    env.ev.dispatch("MESSAGE_DELETE").await;
    assert!(client.store().message(GENERAL, sent_id).is_none());

    // --- logout
    client.logout();
    let ev = env.ev.event("LoggedOut", |e| matches!(e, ClientEvent::LoggedOut { .. })).await;
    assert!(matches!(ev, ClientEvent::LoggedOut { reason } if reason == "Logged out"));
    let logout = mock.wait_for_request("POST", "/auth/logout").await;
    assert_eq!(logout.json(), json!({ "provider": null, "voip_provider": null }));
    assert!(client.store().me.is_none());
    assert!(client.http().token().is_none());
    assert_eq!(client.config().last_account, None);
    let token = mock.token();
    assert!(!mock.with_world(|w| w.valid_tokens.contains(&token)), "token revoked server-side");
    let conn = mock.gateway().wait_for_connection("gateway closed", |c| c.first().filter(|c| !c.open).cloned()).await;
    assert_eq!(conn.client_close_code, Some(1000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_send_marks_the_echo_failed() {
    let mut env = Env::new().await.connected().await;
    env.mock.on("POST", "/channels/*/messages").respond(MockResponse::error(403, 50013, "Missing Permissions"));
    env.client.send_message(GENERAL, "nope".into(), None, false, vec![]);
    let ev = env.ev.event("ActionFailed", |e| matches!(e, ClientEvent::ActionFailed { .. })).await;
    assert!(matches!(ev, ClientEvent::ActionFailed { ref action, ref error } if action == "send_message" && error.contains("Missing Permissions")), "{ev:?}");
    let nonce = {
        let s = env.client.store();
        let echo = s.messages[&GENERAL].messages.last().unwrap();
        assert_eq!(s.pending_state(echo), Some(&PendingState::Failed("Missing Permissions".into())));
        echo.id.to_string()
    };
    env.client.discard_pending(&nonce);
    assert!(env.client.store().messages[&GENERAL].messages.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_token_logs_out() {
    let mut env = Env::new().await;
    env.client.start_session("revoked-token".into());
    let ev = env.ev.event("LoggedOut", |e| matches!(e, ClientEvent::LoggedOut { .. })).await;
    assert!(matches!(ev, ClientEvent::LoggedOut { ref reason } if reason.contains("log in again")), "{ev:?}");
    assert!(env.client.http().token().is_none());
    assert!(env.client.store().me.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_gateway_error_reports_disconnected() {
    let mut env = Env::new().await;
    env.client.start_session("revoked-token".into());
    env.ev.event("LoggedOut", |e| matches!(e, ClientEvent::LoggedOut { .. })).await;
    env.until("disconnected", |c| c.connection_state() == ConnectionState::Disconnected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotated_token_and_resume_keep_the_session() {
    let env = Env::new().await;
    env.mock.gateway().rotate_token(Some("rotated-token"));
    let mut env = env.connected().await;
    assert_eq!(env.client.http().token().as_deref(), Some("rotated-token"));
    env.client.http().current_user().await.unwrap();
    assert_eq!(env.mock.wait_for_request("GET", "/users/@me").await.header("authorization"), Some("rotated-token"));

    env.mock.gateway().send_op(7, Value::Null);
    env.ev.dispatch("RESUMED").await;
    assert_eq!(env.mock.gateway().wait_for_op(6).await.d["token"], "rotated-token");
    assert_eq!(env.client.store().guilds.len(), 1, "a resume keeps the cache");
    env.until("connected", |c| c.connection_state() == ConnectionState::Connected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notifications_for_dms_unless_focused() {
    let mut env = Env::new().await.connected().await;
    let friend = UserBuilder::friend();
    let dm = |id: u64, text: &str| MessageBuilder::new(id, ids::DM, &friend).content(text).build_event();

    env.mock.gateway().dispatch("MESSAGE_CREATE", dm(1_400_000_000_000_000_001, "ping"));
    let ev = env.ev.event("Notification", |e| matches!(e, ClientEvent::Notification(_))).await;
    let ClientEvent::Notification(n) = ev else { unreachable!() };
    assert_eq!((n.title.as_str(), n.body.as_str(), n.channel_id), ("Friendly", "ping", DM));
    assert_eq!(env.client.store().mention_count(DM), 2, "DMs count as mentions");

    env.client.set_focus(Some(DM), true);
    env.mock.gateway().dispatch("MESSAGE_CREATE", dm(1_400_000_000_000_000_002, "seen"));
    env.mock.gateway().dispatch(
        "MESSAGE_CREATE",
        MessageBuilder::new(1_400_000_000_000_000_003, ids::GENERAL, &friend).guild(ids::GUILD).content("guild chatter").build_event(),
    );
    env.mock.gateway().dispatch("TYPING_START", json!({ "channel_id": ids::DM.to_string(), "user_id": ids::FRIEND.to_string(), "timestamp": 1 }));
    // Wait for the last dispatch: no notification may have been emitted before it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ev = tokio::time::timeout_at(deadline, env.ev.0.recv()).await.expect("TYPING_START").unwrap();
        assert!(!matches!(ev, ClientEvent::Notification(_)), "focused DM / mention-only guild must not notify: {ev:?}");
        if matches!(&ev, ClientEvent::Dispatch { name, .. } if name == "TYPING_START") {
            break;
        }
    }
    assert_eq!(env.client.store().typing.get(&DM).map(|t| t.len()), Some(1));
}

// ---------------------------------------------------------------- extensions

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<String>>,
    commands: Mutex<Vec<(Snowflake, String, String)>>,
}

impl Extension for Recorder {
    fn id(&self) -> &str {
        "test.recorder"
    }

    fn on_event(&self, _client: &Client, event: &ClientEvent) {
        self.events.lock().push(event.name().to_string());
    }

    fn on_outgoing_message(&self, _client: &Client, msg: &mut OutgoingMessage) -> Flow {
        if msg.content.contains("secret") {
            return Flow::Cancel;
        }
        msg.content = msg.content.replace("foo", "bar");
        Flow::Continue
    }

    fn commands(&self) -> Vec<LocalCommand> {
        vec![LocalCommand { name: "ping".into(), description: "test".into(), owner: self.id().into() }]
    }

    fn run_command(&self, _client: &Client, channel: Snowflake, name: &str, args: &str) -> bool {
        self.commands.lock().push((channel, name.into(), args.into()));
        true
    }
}

fn posted(reqs: &[RecordedRequest]) -> Vec<String> {
    reqs.iter().map(|r| r.json()["content"].as_str().unwrap_or_default().to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extensions_rewrite_cancel_and_run_commands() {
    let env = Env::new().await;
    let rec = Arc::new(Recorder::default());
    env.client.add_extension(rec.clone());
    let mut env = env.connected().await;
    assert!(rec.events.lock().iter().any(|e| e == "READY"), "extensions see dispatches");
    assert!(env.client.local_commands().iter().any(|c| c.name == "shrug" && c.owner == "builtin.commands"));
    assert!(env.client.local_commands().iter().any(|c| c.name == "ping" && c.owner == "test.recorder"));

    let c = &env.client;
    c.send_message(GENERAL, "foo fighters".into(), None, false, vec![]);
    c.send_message(GENERAL, "my secret".into(), None, false, vec![]);
    c.send_message(GENERAL, "/ping a b".into(), None, false, vec![]);
    c.send_message(GENERAL, "/shrug".into(), None, false, vec![]);
    c.send_message(GENERAL, "/shrug oh well".into(), None, false, vec![]);
    c.send_message(GENERAL, "/me waves".into(), None, false, vec![]);
    c.send_message(GENERAL, "/spoiler the end".into(), None, false, vec![]);
    c.send_message(GENERAL, "/nick Captain".into(), None, false, vec![]);
    c.send_message(GENERAL, "   ".into(), None, false, vec![]);

    let posts = env.mock.wait_for_requests("POST", "/channels/*/messages", 5).await;
    let mut contents = posted(&posts);
    contents.sort();
    let mut expected = vec!["bar fighters", "¯\\\\_(ツ)\\_/¯", "oh well ¯\\\\_(ツ)\\_/¯", "_waves_", "||the end||"];
    expected.sort();
    assert_eq!(contents, expected);
    assert_eq!(rec.commands.lock().clone(), vec![(GENERAL, "ping".to_string(), "a b".to_string())]);
    let nick = env.mock.wait_for_request("PATCH", "/guilds/*/members/@me").await;
    assert_eq!(nick.path, format!("/guilds/{G}/members/@me"));
    assert_eq!(nick.json(), json!({ "nick": "Captain" }));

    for _ in 0..5 {
        env.ev.dispatch("MESSAGE_CREATE").await;
    }
    let s = env.client.store();
    assert!(s.pending.is_empty());
    assert!(!s.messages[&GENERAL].messages.iter().any(|m| m.content.contains("secret")), "cancelled messages leave no echo");
    assert_eq!(env.mock.requests_to("POST", "/channels/*/messages").len(), 5);
}
