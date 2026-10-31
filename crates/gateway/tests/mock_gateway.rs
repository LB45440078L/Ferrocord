//! `ferrocord-gateway` against the mock gateway: identify, zlib-stream framing,
//! heartbeats, zombie detection, resume/replay, invalid sessions and close codes.

use std::sync::Arc;
use std::time::Duration;

use ferrocord_gateway::{ConnectionState, Gateway, GatewayConfig, GatewayError, GatewayHandle, GatewayMessage};
use ferrocord_http::ClientProfile;
use ferrocord_model::gateway::{GuildSubscription, UpdatePresence};
use ferrocord_model::{Event, Snowflake, Status};
use ferrocord_testkit::MockDiscord;
use ferrocord_testkit::fixtures::{MessageBuilder, UserBuilder, ids};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

struct Client {
    handle: GatewayHandle,
    rx: UnboundedReceiver<GatewayMessage>,
    states: Vec<ConnectionState>,
}

impl Client {
    fn start(mock: &MockDiscord) -> Self {
        Self::start_with_token(mock, &mock.token())
    }

    fn start_with_token(mock: &MockDiscord, token: &str) -> Self {
        let mut cfg = GatewayConfig::new(token.to_string(), ClientProfile::desktop());
        cfg.url = mock.gateway_url();
        let (handle, rx) = Gateway::spawn(cfg);
        Self { handle, rx, states: vec![] }
    }

    async fn next_within(&mut self, d: Duration) -> Option<GatewayMessage> {
        let m = tokio::time::timeout(d, self.rx.recv()).await.expect("gateway message in time");
        if let Some(GatewayMessage::State(s)) = &m {
            self.states.push(*s);
        }
        m
    }

    async fn next(&mut self) -> GatewayMessage {
        self.next_within(Duration::from_secs(5)).await.expect("gateway task alive")
    }

    /// Next dispatch with this name (other messages are skipped).
    async fn dispatch_within(&mut self, name: &str, d: Duration) -> (u64, Event, Arc<Value>) {
        loop {
            if let Some(GatewayMessage::Dispatch { seq, event, raw }) = self.next_within(d).await
                && event.name() == name
            {
                return (seq, *event, raw);
            }
        }
    }

    async fn dispatch(&mut self, name: &str) -> (u64, Event, Arc<Value>) {
        self.dispatch_within(name, Duration::from_secs(5)).await
    }

    async fn ready(&mut self) -> Box<ferrocord_model::gateway::Ready> {
        match self.dispatch("READY").await.1 {
            Event::Ready(r) => r,
            other => panic!("{other:?}"),
        }
    }

    async fn fatal(&mut self) -> GatewayError {
        loop {
            if let GatewayMessage::Fatal(e) = self.next().await {
                return e;
            }
        }
    }
}

// ---------------------------------------------------------------- identify / READY

#[tokio::test]
async fn identify_payload_and_ready() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    let ready = c.ready().await;

    let identify = mock.gateway().wait_for_op(2).await.d;
    assert_eq!(identify["token"], mock.token());
    assert_eq!(identify["capabilities"], 1_734_653);
    assert_eq!(identify["compress"], false);
    assert_eq!(identify["client_state"], json!({ "guild_versions": {} }));
    assert_eq!(identify["presence"]["status"], "online");
    let props = &identify["properties"];
    assert_eq!(props["client_build_number"], 657_850);
    assert_eq!(props["browser"], "Discord Client");
    assert_eq!(props["release_channel"], "stable");
    assert!(props["browser_user_agent"].as_str().unwrap().contains("Electron/"));

    let conn = &mock.gateway().connections()[0];
    assert_eq!(conn.path, "/gateway/");
    assert_eq!(conn.query, "encoding=json&v=9&compress=zlib-stream");
    assert!(conn.compressed);
    assert_eq!(conn.origin.as_deref(), Some("https://discord.com"));
    assert!(conn.user_agent.as_deref().unwrap().contains("discord/"));

    assert_eq!(c.states, vec![ConnectionState::Connecting, ConnectionState::Identifying, ConnectionState::Connected]);
    assert_eq!(ready.session_id, "mock-session-1");
    assert_eq!(ready.resume_gateway_url.as_deref(), Some(mock.resume_gateway_url().as_str()));
    assert_eq!(ready.user.user.id, Snowflake(ids::ME));
    assert_eq!(ready.guilds.len(), 1);
    assert_eq!(ready.guilds[0].name(), "Mock Guild", "user-account guild properties are unwrapped");
    assert_eq!(ready.guilds[0].channels.len(), 4);
    assert_eq!(ready.merged_members[0][0].user_id, Some(Snowflake(ids::ME)));
    assert_eq!(ready.private_channels[0].recipient_ids, vec![Snowflake(ids::FRIEND)]);
    assert_eq!(ready.users[0].id, Snowflake(ids::FRIEND));
    assert_eq!(ready.read_state.clone().into_entries().len(), 2);
    assert!(ready.user_settings_proto.is_some());

    let (seq, ev, _) = c.dispatch("READY_SUPPLEMENTAL").await;
    assert_eq!(seq, 2);
    let Event::ReadySupplemental(s) = ev else { panic!() };
    assert_eq!(s.merged_presences.friends.len(), 1);
}

#[tokio::test]
async fn invalid_token_is_fatal() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start_with_token(&mock, "revoked");
    assert!(matches!(c.fatal().await, GatewayError::AuthenticationFailed));
    assert!(matches!(c.next().await, GatewayMessage::State(ConnectionState::Disconnected)));
    assert!(c.next_within(Duration::from_secs(2)).await.is_none(), "task exited");
    assert_eq!(mock.gateway().connections().len(), 1, "no reconnect after 4004");
}

// ---------------------------------------------------------------- framing

#[tokio::test]
async fn zlib_stream_frames_split_across_messages() {
    let mock = MockDiscord::start().await;
    mock.gateway().split_frames(Some(7));
    let mut c = Client::start(&mock);
    c.ready().await;

    // Low-redundancy content so the compressed message spans many frames.
    let content: String = (0..6000u32).map(|i| char::from_u32(0x4e00 + (i * 7919) % 20000).unwrap()).collect();
    let friend = UserBuilder::friend();
    for i in 0..3u64 {
        let m = MessageBuilder::new(1_300_000_000_000_000_000 + i, ids::GENERAL, &friend).guild(ids::GUILD).content(format!("{i}:{content}"));
        mock.gateway().dispatch("MESSAGE_CREATE", m.build_event());
    }
    for i in 0..3u64 {
        let (seq, ev, raw) = c.dispatch("MESSAGE_CREATE").await;
        assert_eq!(seq, 3 + i);
        let Event::MessageCreate(m) = ev else { panic!() };
        assert_eq!(m.content, format!("{i}:{content}"));
        assert_eq!(m.guild_id, Some(Snowflake(ids::GUILD)));
        assert_eq!(raw["author"]["username"], "friend", "raw payload is kept verbatim");
    }
}

#[tokio::test]
async fn malformed_and_unknown_dispatches_degrade_to_other() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    mock.gateway().dispatch("MESSAGE_CREATE", json!({ "id": { "not": "a snowflake" } }));
    mock.gateway().dispatch("BRAND_NEW_EVENT", json!({ "x": 1 }));
    mock.gateway().dispatch("TYPING_START", json!({ "channel_id": ids::GENERAL.to_string(), "user_id": ids::FRIEND.to_string(), "timestamp": 1 }));
    let (_, ev, _) = c.dispatch("MESSAGE_CREATE").await;
    assert!(matches!(ev, Event::Other { ref name, .. } if name == "MESSAGE_CREATE"), "{ev:?}");
    let (_, ev, _) = c.dispatch("BRAND_NEW_EVENT").await;
    assert!(matches!(ev, Event::Other { ref data, .. } if data["x"] == 1));
    let (_, ev, _) = c.dispatch("TYPING_START").await;
    assert!(matches!(ev, Event::TypingStart(t) if t.user_id == Snowflake(ids::FRIEND)), "session survives bad payloads");
}

// ---------------------------------------------------------------- heartbeats

#[tokio::test]
async fn heartbeats_report_latency_and_sequence() {
    let mock = MockDiscord::start().await;
    // Short enough to see three beats quickly, long enough that a slow CI runner
    // still delivers each ACK before the next beat (a missed ACK means "zombie,
    // resume", which would add a RESUMED dispatch and a second connection).
    mock.gateway().set_heartbeat_interval(250);
    let mut c = Client::start(&mock);
    c.ready().await;
    c.dispatch("READY_SUPPLEMENTAL").await;
    loop {
        if let GatewayMessage::Latency(d) = c.next().await {
            assert!(d < Duration::from_secs(1));
            break;
        }
    }
    let hb = mock.gateway().wait_for_nth_op(1, 3).await;
    assert_eq!(hb.d, json!(2), "heartbeat carries the last sequence number");
    assert_eq!(mock.gateway().connections().len(), 1, "acked heartbeats keep the connection");
}

#[tokio::test]
async fn server_heartbeat_request_is_answered_immediately() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    let before = mock.gateway().frames_with_op(1).len();
    mock.gateway().send_op(1, Value::Null);
    let hb = mock.gateway().wait_for_nth_op(1, before + 1).await;
    assert!(hb.d.is_u64());
}

#[tokio::test]
async fn zombie_connection_reconnects_and_resumes() {
    let mock = MockDiscord::start().await;
    mock.gateway().set_heartbeat_interval(40);
    mock.gateway().drop_acks_on_next_connections(1);
    let mut c = Client::start(&mock);
    let ready = c.ready().await;
    c.dispatch("READY_SUPPLEMENTAL").await;
    c.dispatch("RESUMED").await;

    let resume = mock.gateway().wait_for_op(6).await;
    assert_eq!(resume.d, json!({ "token": mock.token(), "session_id": ready.session_id, "seq": 2 }));
    let conns = mock.gateway().connections();
    assert_eq!(conns[0].client_close_code, Some(4000), "client closes the zombie with 4000");
    assert_eq!(conns[1].path, "/resume/", "resume goes to resume_gateway_url");
    assert!(conns[1].resumed);
    assert!(c.states.contains(&ConnectionState::Resuming));
    assert_eq!(c.states.last(), Some(&ConnectionState::Connected));
    assert_eq!(mock.gateway().frames_with_op(2).len(), 1, "no second IDENTIFY");
}

// ---------------------------------------------------------------- server-initiated reconnects

#[tokio::test]
async fn reconnect_opcode_resumes_and_replays_missed_events() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    c.dispatch("READY_SUPPLEMENTAL").await;

    // Lost in flight: recorded in the session but never delivered.
    let missed = MessageBuilder::new(1_300_000_000_000_000_001, ids::GENERAL, &UserBuilder::friend()).content("missed").build_event();
    mock.gateway().dispatch_buffered("MESSAGE_CREATE", missed);
    mock.gateway().send_op(7, Value::Null);

    let (seq, ev, _) = c.dispatch("MESSAGE_CREATE").await;
    assert_eq!(seq, 3);
    assert!(matches!(ev, Event::MessageCreate(m) if m.content == "missed"));
    let (seq, _, _) = c.dispatch("RESUMED").await;
    assert_eq!(seq, 4);
    assert_eq!(mock.gateway().wait_for_op(6).await.d["seq"], 2);

    // The resumed session keeps receiving live events.
    mock.gateway().dispatch("TYPING_START", json!({ "channel_id": ids::GENERAL.to_string(), "user_id": ids::FRIEND.to_string(), "timestamp": 1 }));
    assert_eq!(c.dispatch("TYPING_START").await.0, 5);
}

#[tokio::test]
async fn ready_auth_token_is_used_on_resume() {
    let mock = MockDiscord::start().await;
    mock.gateway().rotate_token(Some("rotated-token"));
    let mut c = Client::start(&mock);
    let ready = c.ready().await;
    assert_eq!(ready.auth_token.as_deref(), Some("rotated-token"));
    mock.gateway().send_op(7, Value::Null);
    c.dispatch("RESUMED").await;
    assert_eq!(mock.gateway().wait_for_op(6).await.d["token"], "rotated-token");
}

#[tokio::test]
async fn invalid_session_not_resumable_reidentifies() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    mock.gateway().send_op(9, json!(false));
    // The client waits 1–5 s before identifying again, as Discord asks.
    let (_, ev, _) = c.dispatch_within("READY", Duration::from_secs(8)).await;
    let Event::Ready(r) = ev else { panic!() };
    assert_eq!(r.session_id, "mock-session-2");
    assert_eq!(mock.gateway().frames_with_op(2).len(), 2);
    assert!(mock.gateway().frames_with_op(6).is_empty(), "no RESUME for a dead session");
    assert_eq!(mock.gateway().connections()[1].path, "/gateway/");
}

#[tokio::test]
async fn close_4009_starts_a_fresh_session() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    mock.gateway().close(4009, "Session timed out.");
    let r = c.ready().await;
    assert_eq!(r.session_id, "mock-session-2");
    assert!(mock.gateway().frames_with_op(6).is_empty());
    let conns = mock.gateway().connections();
    assert_eq!(conns[0].server_close_code, Some(4009));
    assert_eq!(conns[1].path, "/gateway/", "fresh sessions use the main gateway URL");
}

#[tokio::test]
async fn close_4004_is_fatal() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    mock.gateway().close(4004, "Authentication failed.");
    assert!(matches!(c.fatal().await, GatewayError::AuthenticationFailed));
    assert!(matches!(c.next().await, GatewayMessage::State(ConnectionState::Disconnected)));
    assert!(c.next_within(Duration::from_secs(2)).await.is_none());
}

#[tokio::test]
async fn other_fatal_close_codes() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    mock.gateway().close(4014, "Disallowed intent(s).");
    assert!(matches!(c.fatal().await, GatewayError::Fatal(4014, ref r) if r.contains("intent")));
}

// ---------------------------------------------------------------- commands

#[tokio::test]
async fn commands_are_sent_only_after_ready() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    let dnd = UpdatePresence { status: Status::Dnd, ..Default::default() };
    c.handle.update_presence(&dnd); // before READY: dropped
    c.ready().await;
    let idle = UpdatePresence { status: Status::Idle, ..Default::default() };
    c.handle.update_presence(&idle);
    let p = mock.gateway().wait_for_op(3).await;
    assert_eq!(p.d, json!({ "status": "idle", "since": 0, "activities": [], "afk": false }));

    // Lazy guild subscriptions are batched at ~15 KiB per frame.
    let subs = (0..400u64).map(|i| {
        let mut s = GuildSubscription { typing: Some(true), threads: Some(true), activities: Some(true), ..Default::default() };
        s.channels.insert(Snowflake(ids::GENERAL + i), vec![[0, 99]]);
        (Snowflake(ids::GUILD + i), s)
    });
    c.handle.subscribe_guilds(subs);
    let gw = mock.gateway();
    let frames = gw.wait_for_frame("all op 37 batches", |f| {
        let ops: Vec<_> = f.iter().filter(|f| f.op == 37).cloned().collect();
        let total: usize = ops.iter().map(|f| f.d["subscriptions"].as_object().unwrap().len()).sum();
        (total == 400).then(|| ops.last().cloned()).flatten()
    });
    frames.await;
    let batches = mock.gateway().frames_with_op(37);
    assert!(batches.len() > 1, "split into several frames");
    for b in &batches {
        assert!(b.d.to_string().len() < 16 * 1024 + 512, "batch too large: {}", b.d.to_string().len());
    }
    let ops: Vec<u8> = mock.gateway().frames().iter().map(|f| f.op).filter(|o| *o == 3).collect();
    assert_eq!(ops.len(), 1, "the pre-READY presence update was dropped");
}

#[tokio::test]
async fn shutdown_closes_normally() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    c.handle.shutdown();
    loop {
        match c.next_within(Duration::from_secs(5)).await {
            Some(GatewayMessage::State(ConnectionState::Disconnected)) => break,
            Some(_) => {}
            None => panic!("channel closed before Disconnected"),
        }
    }
    let conn = mock.gateway().wait_for_connection("socket closed", |c| c.first().filter(|c| !c.open).cloned()).await;
    assert_eq!(conn.client_close_code, Some(1000));
}

#[tokio::test]
async fn repeated_reconnect_requests_are_not_backed_off() {
    let mock = MockDiscord::start().await;
    let mut c = Client::start(&mock);
    c.ready().await;
    mock.gateway().send_op(7, Value::Null);
    c.dispatch("RESUMED").await;
    let started = std::time::Instant::now();
    mock.gateway().send_op(7, Value::Null);
    c.dispatch_within("RESUMED", Duration::from_secs(8)).await;
    assert!(started.elapsed() < Duration::from_secs(1), "second resume took {:?}", started.elapsed());
    assert!(!c.states.iter().any(|s| matches!(s, ConnectionState::Reconnecting(_))));
}
