//! Block / unblock / ignore against the mock Discord: optimistic store updates,
//! REST calls, rollback on failure and notification suppression.

use std::time::Duration;

use ferrocord_core::model::{RelationshipType, Snowflake};
use ferrocord_core::{Client, ClientEvent, ClientOptions, Paths};
use ferrocord_testkit::fixtures::{MessageBuilder, UserBuilder, ids};
use ferrocord_testkit::{MockDiscord, MockResponse};
use serde_json::json;
use tokio::sync::broadcast;

const FRIEND: Snowflake = Snowflake(ids::FRIEND);

struct Env {
    mock: MockDiscord,
    client: Client,
    ev: broadcast::Receiver<ClientEvent>,
    _dir: tempfile::TempDir,
}

impl Env {
    async fn connected() -> Self {
        let mock = MockDiscord::start().await;
        let dir = tempfile::tempdir().unwrap();
        let opts = ClientOptions::custom_endpoints(mock.api_base(), mock.gateway_url());
        let client = Client::with_options(tokio::runtime::Handle::current(), Paths::under(dir.path()), opts);
        let mut env = Self { ev: client.subscribe(), mock, client, _dir: dir };
        env.client.start_session(env.mock.token());
        env.event("READY_SUPPLEMENTAL", |e| matches!(e, ClientEvent::Dispatch { name, .. } if name == "READY_SUPPLEMENTAL")).await;
        env
    }

    async fn event(&mut self, what: &str, pred: impl Fn(&ClientEvent) -> bool) -> ClientEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, self.ev.recv()).await {
                Ok(Ok(ev)) if pred(&ev) => return ev,
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }

    /// Sends a DM from the friend and reports whether it raised a notification.
    async fn dm_notifies(&mut self, id: u64) -> bool {
        let friend = UserBuilder::friend();
        self.mock.gateway().dispatch("MESSAGE_CREATE", MessageBuilder::new(id, ids::DM, &friend).content("hey").build_event());
        // A marker dispatch: the notification (if any) is emitted before it.
        self.mock.gateway().dispatch("TYPING_START", json!({ "channel_id": ids::DM.to_string(), "user_id": ids::FRIEND.to_string(), "timestamp": 1 }));
        let mut notified = false;
        loop {
            let ev = self.event("TYPING_START", |_| true).await;
            notified |= matches!(ev, ClientEvent::Notification(_));
            if matches!(&ev, ClientEvent::Dispatch { name, .. } if name == "TYPING_START") {
                return notified;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_unblock_and_rollback() {
    let mut env = Env::connected().await;
    assert!(env.client.store().is_friend(FRIEND));

    // A failed block restores the friendship and reports the error.
    env.mock.on("PUT", "/users/@me/relationships/*").respond(MockResponse::error(403, 50007, "Cannot block"));
    env.client.block_user(FRIEND);
    assert!(env.client.store().is_blocked(FRIEND), "optimistic update");
    let ev = env.event("ActionFailed", |e| matches!(e, ClientEvent::ActionFailed { .. })).await;
    assert!(matches!(ev, ClientEvent::ActionFailed { ref action, .. } if action == "block_user"), "{ev:?}");
    assert_eq!(env.client.store().relationships[&FRIEND].kind, RelationshipType::Friend);
    env.mock.clear_routes();

    // Blocking replaces the friendship.
    env.client.block_user(FRIEND);
    assert!(env.client.store().is_blocked(FRIEND));
    assert!(!env.client.store().is_friend(FRIEND));
    let req = env.mock.wait_for_request("PUT", "/users/@me/relationships/*").await;
    assert_eq!(req.json(), json!({ "type": 2 }));

    // Blocked users don't notify and don't show as typing.
    assert!(!env.dm_notifies(1_400_000_000_000_000_010).await);
    assert!(env.client.store_mut().typing_users(Snowflake(ids::DM)).is_empty());

    env.client.unblock_user(FRIEND);
    assert!(!env.client.store().is_blocked(FRIEND));
    env.mock.wait_for_request("DELETE", "/users/@me/relationships/*").await;
    assert!(env.dm_notifies(1_400_000_000_000_000_011).await);

    // Unblocking someone who isn't blocked is a no-op.
    env.mock.clear_requests();
    env.client.unblock_user(Snowflake(ids::ME));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(env.mock.requests_to("DELETE", "/users/@me/relationships/*").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ignored_users_are_local_and_silent() {
    let mut env = Env::connected().await;
    assert!(env.dm_notifies(1_400_000_000_000_000_020).await);

    env.client.set_ignored(FRIEND, true);
    assert!(env.client.is_ignored(FRIEND));
    assert!(env.client.is_blocked_or_ignored(FRIEND));
    assert!(!env.dm_notifies(1_400_000_000_000_000_021).await);
    assert_eq!(env.client.store_mut().typing_users(Snowflake(ids::DM)), vec![FRIEND], "the store doesn't know about ignores");
    assert!(env.client.typing_users_filtered(Snowflake(ids::DM)).is_empty());
    let saved = std::fs::read_to_string(env.client.paths().config_file()).unwrap();
    assert!(saved.contains(&format!("\"{FRIEND}\"")), "{saved}");
    assert!(env.mock.requests_to("PUT", "/users/@me/relationships/*").is_empty(), "ignoring is device-only");

    env.client.set_ignored(FRIEND, false);
    assert!(!env.client.is_ignored(FRIEND));
    assert!(env.dm_notifies(1_400_000_000_000_000_022).await);
}
