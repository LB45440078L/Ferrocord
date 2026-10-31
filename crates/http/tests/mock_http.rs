//! `ferrocord-http` against the mock Discord: headers, captcha/MFA interception,
//! rate limits, retries, error bodies and route helpers.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrocord_http::{
    BoxFuture, CaptchaChallenge, CaptchaSolution, CaptchaSolver, ClientProfile, FilePart, Http, HttpError, MessageAnchor, MfaChallenge, MfaHandler,
    MfaResponse, Request,
};
use ferrocord_model::{CreateMessage, PartialEmoji, Snowflake};
use ferrocord_testkit::fixtures::{MessageBuilder, UserBuilder, ids};
use ferrocord_testkit::{CaptchaGate, MfaGate, MockDiscord, MockResponse};
use parking_lot::Mutex;
use serde_json::{Value, json};

async fn setup() -> (MockDiscord, Http) {
    let mock = MockDiscord::start().await;
    let http = Http::with_base_url(ClientProfile::desktop(), &mock.api_base());
    http.set_token(Some(mock.token()));
    (mock, http)
}

fn ms(a: Instant, b: Instant) -> u128 {
    b.saturating_duration_since(a).as_millis()
}

// ---------------------------------------------------------------- headers

#[tokio::test]
async fn authenticated_requests_carry_client_identity() {
    let (mock, http) = setup().await;
    let me = http.current_user().await.expect("current user");
    assert_eq!(me.user.id, Snowflake(ids::ME));
    assert_eq!(me.email.as_deref(), Some(ferrocord_testkit::fixtures::DEFAULT_EMAIL));

    let req = mock.wait_for_request("GET", "/users/@me").await;
    assert_eq!(req.header("authorization"), Some(mock.token().as_str()));
    assert!(!req.has_header("x-fingerprint"), "fingerprint is only sent before login");
    let sp = req.super_properties().expect("X-Super-Properties is base64 JSON");
    assert_eq!(sp["client_build_number"], json!(657_850));
    assert_eq!(sp["browser"], "Discord Client");
    assert_eq!(sp["release_channel"], "stable");
    assert_eq!(sp["client_version"], "1.0.9266");
    assert_eq!(sp["has_client_mods"], false);
    assert!(sp["client_launch_id"].as_str().is_some_and(|s| s.len() == 36));
    assert!(sp["browser_user_agent"].as_str().unwrap().contains("discord/1.0.9266"));
    assert_eq!(req.header("x-debug-options"), Some("bugReporterEnabled"));
    assert!(req.has_header("x-discord-locale"));
    assert!(req.has_header("x-discord-timezone"));
    assert_eq!(req.header("origin"), Some("https://discord.com"));
    assert!(req.header("user-agent").unwrap().contains("Electron/44.9.3"));
}

#[tokio::test]
async fn fingerprint_is_fetched_once_and_sent_before_login() {
    let (mock, http) = setup().await;
    http.set_token(None);
    let fp = http.ensure_fingerprint().await.expect("fingerprint");
    assert_eq!(fp, mock.with_world(|w| w.fingerprint.clone()));
    assert_eq!(http.ensure_fingerprint().await.as_deref(), Some(fp.as_str()));
    let exp = mock.requests_to("GET", "/experiments");
    assert_eq!(exp.len(), 1, "fingerprint is cached");
    assert_eq!(exp[0].query_param("with_guild_experiments"), Some("true"));

    let _ = http.send(Request::post("/auth/login").unauthenticated().json(json!({}))).await;
    let login = mock.wait_for_request("POST", "/auth/login").await;
    assert_eq!(login.header("x-fingerprint"), Some(fp.as_str()));
    assert!(!login.has_header("authorization"));
}

#[tokio::test]
async fn unauthenticated_requests_omit_the_token() {
    let (mock, http) = setup().await;
    let url = http.gateway_url().await.unwrap();
    assert_eq!(url, mock.gateway_url());
    let req = mock.wait_for_request("GET", "/gateway").await;
    assert!(!req.has_header("authorization"));
}

// ---------------------------------------------------------------- captcha

#[derive(Default)]
struct FakeSolver {
    seen: Mutex<Vec<CaptchaChallenge>>,
    /// `None` = user cancels.
    answer: Option<String>,
}

impl CaptchaSolver for FakeSolver {
    fn solve(&self, challenge: CaptchaChallenge) -> BoxFuture<'static, Option<CaptchaSolution>> {
        self.seen.lock().push(challenge.clone());
        let answer = self.answer.clone();
        Box::pin(async move { answer.map(|key| CaptchaSolution { key, rqtoken: challenge.rqtoken, session_id: challenge.session_id }) })
    }
}

#[tokio::test]
async fn captcha_is_solved_and_request_replayed() {
    let (mock, http) = setup().await;
    let gate = CaptchaGate::default();
    mock.require_captcha("POST", "/users/@me/relationships", gate.clone());
    let solver = Arc::new(FakeSolver { answer: Some(gate.solution.clone()), ..Default::default() });
    http.set_captcha_solver(Some(solver.clone()));

    http.send_friend_request("someone").await.expect("replayed after captcha");

    let seen = solver.seen.lock().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].sitekey, gate.sitekey);
    assert_eq!(seen[0].service, "hcaptcha");
    assert_eq!(seen[0].rqdata, gate.rqdata);
    assert!(!seen[0].invisible);

    let reqs = mock.requests_to("POST", "/users/@me/relationships");
    assert_eq!(reqs.len(), 2);
    assert!(!reqs[0].has_header("x-captcha-key"));
    assert_eq!(reqs[1].header("x-captcha-key"), Some(gate.solution.as_str()));
    assert_eq!(reqs[1].header("x-captcha-rqtoken"), gate.rqtoken.as_deref());
    assert_eq!(reqs[1].header("x-captcha-session-id"), gate.session_id.as_deref());
    assert_eq!(reqs[0].json(), reqs[1].json(), "the same body is replayed");
}

#[tokio::test]
async fn captcha_cancelled_or_unsolvable_surfaces_the_challenge() {
    let (mock, http) = setup().await;
    mock.require_captcha("POST", "/users/@me/relationships", CaptchaGate::default());

    // No solver installed.
    let err = http.send_friend_request("a").await.unwrap_err();
    assert!(matches!(&err, HttpError::CaptchaRequired(b) if b["captcha_sitekey"].is_string()), "{err:?}");

    // User cancels.
    http.set_captcha_solver(Some(Arc::new(FakeSolver::default())));
    assert!(matches!(http.send_friend_request("a").await, Err(HttpError::CaptchaRequired(_))));
    assert_eq!(mock.requests_to("POST", "/users/@me/relationships").len(), 2);

    // Wrong answers: the client gives up after three attempts.
    mock.clear_requests();
    http.set_captcha_solver(Some(Arc::new(FakeSolver { answer: Some("wrong".into()), ..Default::default() })));
    assert!(matches!(http.send_friend_request("a").await, Err(HttpError::CaptchaRequired(_))));
    assert_eq!(mock.requests_to("POST", "/users/@me/relationships").len(), 4);
}

// ---------------------------------------------------------------- MFA

struct FakeMfa {
    seen: Mutex<Vec<MfaChallenge>>,
    answer: Option<MfaResponse>,
}

impl MfaHandler for FakeMfa {
    fn verify(&self, challenge: MfaChallenge) -> BoxFuture<'static, Option<MfaResponse>> {
        self.seen.lock().push(challenge);
        let a = self.answer.clone();
        Box::pin(async move { a })
    }
}

#[tokio::test]
async fn mfa_reauth_replays_with_authorization_header() {
    let (mock, http) = setup().await;
    mock.on("DELETE", "/guilds/*/members/*").respond(MockResponse::no_content());
    let gate = MfaGate::default();
    mock.require_mfa("DELETE", "/guilds/*/members/*", gate.clone());
    let handler = Arc::new(FakeMfa { seen: Mutex::new(vec![]), answer: Some(MfaResponse { mfa_type: "totp".into(), data: gate.code.clone() }) });
    http.set_mfa_handler(Some(handler.clone()));

    http.kick(Snowflake(ids::GUILD), Snowflake(ids::FRIEND), Some("spam & abuse")).await.expect("kick after MFA");

    let seen = handler.seen.lock().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].ticket, gate.ticket);
    assert_eq!(seen[0].methods, vec!["totp".to_string(), "password".to_string()]);

    let finish = mock.wait_for_request("POST", "/mfa/finish").await;
    assert_eq!(finish.json(), json!({ "ticket": gate.ticket, "mfa_type": "totp", "data": gate.code }));

    let kicks = mock.requests_to("DELETE", "/guilds/*/members/*");
    assert_eq!(kicks.len(), 2);
    assert_eq!(kicks[1].header("x-discord-mfa-authorization"), Some(gate.token.as_str()));
    // Audit log reasons are percent-encoded like encodeURIComponent (spaces as %20, not '+').
    assert_eq!(kicks[1].header("x-audit-log-reason"), Some("spam%20%26%20abuse"));
}

#[tokio::test]
async fn mfa_cancelled_returns_mfa_required() {
    let (mock, http) = setup().await;
    mock.require_mfa("DELETE", "/guilds/*/members/*", MfaGate::default());
    let err = http.kick(Snowflake(ids::GUILD), Snowflake(ids::FRIEND), None).await.unwrap_err();
    assert!(matches!(&err, HttpError::MfaRequired(b) if b["code"] == 60003), "{err:?}");

    http.set_mfa_handler(Some(Arc::new(FakeMfa { seen: Mutex::new(vec![]), answer: None })));
    assert!(matches!(http.kick(Snowflake(ids::GUILD), Snowflake(ids::FRIEND), None).await, Err(HttpError::MfaRequired(_))));
    assert!(mock.requests_to("POST", "/mfa/finish").is_empty());
}

#[tokio::test]
async fn mfa_wrong_code_fails_with_invalid_code() {
    let (mock, http) = setup().await;
    mock.require_mfa("DELETE", "/guilds/*/members/*", MfaGate::default());
    http.set_mfa_handler(Some(Arc::new(FakeMfa { seen: Mutex::new(vec![]), answer: Some(MfaResponse { mfa_type: "totp".into(), data: "000000".into() }) })));
    let err = http.kick(Snowflake(ids::GUILD), Snowflake(ids::FRIEND), None).await.unwrap_err();
    assert_eq!(err.code(), Some(60008), "{err:?}");
}

// ---------------------------------------------------------------- rate limits & retries

#[tokio::test]
async fn retries_after_429() {
    let (mock, http) = setup().await;
    mock.rate_limit("GET", "/users/@me", 0.1, false, 1);
    http.current_user().await.expect("succeeds after waiting");
    let reqs = mock.requests_to("GET", "/users/@me");
    assert_eq!(reqs.len(), 2);
    assert!(ms(reqs[0].at, reqs[1].at) >= 90, "waited retry_after");
}

#[tokio::test]
async fn global_429_delays_other_routes() {
    let (mock, http) = setup().await;
    mock.rate_limit("GET", "/users/@me", 0.4, true, 1);
    let h2 = http.clone();
    let first = tokio::spawn(async move { h2.current_user().await });
    let hit = mock.wait_for_request("GET", "/users/@me").await;
    // Give the client a moment to process the 429 and set the global limit.
    tokio::time::sleep(Duration::from_millis(50)).await;
    http.gateway_url().await.unwrap();
    let other = mock.wait_for_request("GET", "/gateway").await;
    assert!(ms(hit.at, other.at) >= 350, "global limit applied to another route ({}ms)", ms(hit.at, other.at));
    first.await.unwrap().unwrap();
}

#[tokio::test]
async fn very_long_retry_after_is_an_error() {
    let (mock, http) = setup().await;
    mock.rate_limit("GET", "/users/@me", 120.0, false, 1);
    let err = http.current_user().await.unwrap_err();
    assert!(matches!(err, HttpError::RateLimited(w) if w == 120.0), "{err:?}");
    assert_eq!(mock.requests_to("GET", "/users/@me").len(), 1);
}

#[tokio::test]
async fn exhausted_bucket_waits_for_reset_per_major_parameter() {
    let (mock, http) = setup().await;
    mock.on("GET", "/channels/*/messages/pins").respond(MockResponse::ok(json!({ "items": [], "has_more": false })).with_bucket("pins-bucket", 1, 0, 0.4));
    let (a, b) = (Snowflake(ids::GENERAL), Snowflake(ids::RANDOM));
    http.pins(a).await.unwrap();
    http.pins(b).await.unwrap();
    http.pins(a).await.unwrap();
    let reqs = mock.requests_to("GET", "/channels/*/messages/pins");
    assert_eq!(reqs.len(), 3);
    assert!(ms(reqs[0].at, reqs[1].at) < 300, "other channel is a different bucket");
    assert!(ms(reqs[0].at, reqs[2].at) >= 350, "same channel waited for reset ({}ms)", ms(reqs[0].at, reqs[2].at));
}

#[tokio::test]
async fn server_errors_are_retried() {
    let (mock, http) = setup().await;
    mock.on("GET", "/users/@me").respond_seq(vec![MockResponse::error(502, 0, "Bad Gateway")]);
    http.current_user().await.expect("retried");
    assert_eq!(mock.requests_to("GET", "/users/@me").len(), 2);

    mock.clear_requests();
    mock.on("GET", "/users/@me").respond(MockResponse::error(503, 0, "Service Unavailable"));
    let err = http.current_user().await.unwrap_err();
    assert_eq!(err.status(), Some(503));
    assert_eq!(mock.requests_to("GET", "/users/@me").len(), 3, "gives up after 3 attempts");
}

// ---------------------------------------------------------------- error bodies

#[tokio::test]
async fn error_bodies_are_parsed() {
    let (mock, http) = setup().await;
    mock.on("GET", "/channels/*").respond(MockResponse::error(403, 50013, "Missing Permissions"));
    let err = http.channel(Snowflake(1)).await.unwrap_err();
    assert_eq!(err.status(), Some(403));
    assert_eq!(err.code(), Some(50013));
    assert!(err.to_string().contains("Missing Permissions"), "{err}");

    mock.on("PATCH", "/channels/*/messages/*").respond(MockResponse::form_error("content", "BASE_TYPE_MAX_LENGTH", "Must be 2000 or fewer in length."));
    let err = http.edit_message(Snowflake(1), Snowflake(2), "x").await.unwrap_err();
    let HttpError::Api { body, raw, .. } = &err else { panic!("{err:?}") };
    assert_eq!(body.code, 50035);
    assert_eq!(body.display_message(), "Must be 2000 or fewer in length.");
    assert_eq!(body.field_errors(), vec![("content".to_string(), "Must be 2000 or fewer in length.".to_string())]);
    assert_eq!(raw["errors"]["content"]["_errors"][0]["code"], "BASE_TYPE_MAX_LENGTH");

    // Nested and array field errors flatten to dotted paths.
    let nested = MockResponse::json(
        400,
        json!({ "code": 50035, "message": "Invalid Form Body", "errors": { "embeds": { "0": { "title": { "_errors": [{ "code": "X", "message": "Too long" }] } } } } }),
    );
    mock.on("POST", "/channels/*/messages").respond(nested);
    let err = http.send_message(Snowflake(ids::GENERAL), &CreateMessage { content: "x".into(), ..Default::default() }, vec![]).await.unwrap_err();
    let HttpError::Api { body, .. } = &err else { panic!("{err:?}") };
    assert_eq!(body.field_errors(), vec![("embeds.0.title".to_string(), "Too long".to_string())]);

    // Bad token → Unauthorized; unknown message → 10008.
    http.set_token(Some("bogus".into()));
    assert!(matches!(http.current_user().await, Err(HttpError::Unauthorized)));
    http.set_token(Some(mock.token()));
    let err = http.delete_message(Snowflake(ids::GENERAL), Snowflake(42)).await.unwrap_err();
    assert_eq!(err.code(), Some(10008));
}

#[tokio::test]
async fn unexpected_success_body_is_a_decode_error() {
    let (mock, http) = setup().await;
    mock.on("GET", "/users/@me").respond(MockResponse::ok(json!([1, 2, 3])));
    assert!(matches!(http.current_user().await, Err(HttpError::Decode(_))));
}

// ---------------------------------------------------------------- routes

#[tokio::test]
async fn multipart_upload_shape() {
    let (mock, http) = setup().await;
    let files = vec![
        FilePart { filename: "hello.txt".into(), content_type: Some("text/plain".into()), data: Arc::new(b"hello world".to_vec()) },
        FilePart { filename: "pic.png".into(), content_type: None, data: Arc::new(vec![0x89, b'P', b'N', b'G']) },
    ];
    let msg = CreateMessage { content: "with files".into(), nonce: Some("123".into()), ..Default::default() };
    let sent = http.send_message(Snowflake(ids::GENERAL), &msg, files).await.expect("upload");
    assert_eq!(sent.attachments.len(), 2);
    assert_eq!(sent.attachments[0].filename, "hello.txt");

    let req = mock.wait_for_request("POST", "/channels/*/messages").await;
    assert!(req.header("content-type").unwrap().starts_with("multipart/form-data; boundary="));
    let parts = req.multipart().expect("multipart body");
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0].name, "payload_json");
    let payload: Value = serde_json::from_slice(&parts[0].data).unwrap();
    assert_eq!(payload["content"], "with files");
    assert_eq!(payload["attachments"], json!([{ "id": "0", "filename": "hello.txt" }, { "id": "1", "filename": "pic.png" }]));
    assert_eq!(parts[1].name, "files[0]");
    assert_eq!(parts[1].filename.as_deref(), Some("hello.txt"));
    assert_eq!(parts[1].content_type.as_deref(), Some("text/plain"));
    assert_eq!(parts[1].data, b"hello world");
    assert_eq!(parts[2].name, "files[1]");
    assert_eq!(parts[2].data, vec![0x89, b'P', b'N', b'G']);
}

#[tokio::test]
async fn message_pagination_parameters() {
    let (mock, http) = setup().await;
    let c = ids::GENERAL;
    let seeded = mock.with_world(|w| w.seed_messages(c, 120, &UserBuilder::friend()));
    let ch = Snowflake(c);

    let latest = http.messages(ch, MessageAnchor::Latest, 50).await.unwrap();
    assert_eq!(latest.len(), 50);
    assert_eq!(latest[0].id.0, seeded[119], "newest first");
    assert_eq!(latest[49].id.0, seeded[70]);

    let before = http.messages(ch, MessageAnchor::Before(Snowflake(seeded[70])), 100).await.unwrap();
    assert_eq!(before.len(), 70);
    assert_eq!(before.last().unwrap().id.0, seeded[0]);

    let after = http.messages(ch, MessageAnchor::After(Snowflake(seeded[10])), 5).await.unwrap();
    let ids_after: Vec<u64> = after.iter().map(|m| m.id.0).collect();
    assert_eq!(ids_after, vec![seeded[15], seeded[14], seeded[13], seeded[12], seeded[11]]);

    let around = http.messages(ch, MessageAnchor::Around(Snowflake(seeded[60])), 10).await.unwrap();
    assert_eq!(around.len(), 10);
    assert!(around.iter().any(|m| m.id.0 == seeded[60]));

    http.messages(ch, MessageAnchor::Latest, 0).await.unwrap();
    http.messages(ch, MessageAnchor::Latest, 200).await.unwrap();

    let reqs = mock.requests_to("GET", "/channels/*/messages");
    let q = |i: usize, k: &str| reqs[i].query_param(k).map(str::to_string);
    assert_eq!(q(0, "limit").as_deref(), Some("50"));
    assert_eq!(q(0, "before"), None);
    assert_eq!(q(1, "before"), Some(seeded[70].to_string()));
    assert_eq!(q(2, "after"), Some(seeded[10].to_string()));
    assert_eq!(q(3, "around"), Some(seeded[60].to_string()));
    assert_eq!(q(4, "limit").as_deref(), Some("1"), "limit is clamped to 1..=100");
    assert_eq!(q(5, "limit").as_deref(), Some("100"));
}

#[tokio::test]
async fn reaction_routes_encode_emoji() {
    let (mock, http) = setup().await;
    let (c, m) = (ids::GENERAL, 1_300_000_000_000_000_000u64);
    mock.with_world(|w| w.insert_message(MessageBuilder::new(m, c, &UserBuilder::friend()).content("react to me").build()));
    let thumbs = PartialEmoji::unicode("👍");
    let custom = PartialEmoji { id: Some(Snowflake(123)), name: Some("blob".into()), animated: false };

    http.add_reaction(Snowflake(c), Snowflake(m), &thumbs).await.unwrap();
    http.add_reaction(Snowflake(c), Snowflake(m), &custom).await.unwrap();
    http.remove_reaction(Snowflake(c), Snowflake(m), &custom, None).await.unwrap();

    let puts = mock.requests_to("PUT", "/channels/*/messages/*/reactions/*/@me");
    assert_eq!(puts[0].path, format!("/channels/{c}/messages/{m}/reactions/%F0%9F%91%8D/@me"));
    assert_eq!(puts[0].query_param("location"), Some("Message"));
    assert_eq!(puts[0].query_param("type"), Some("0"));
    assert_eq!(puts[1].path, format!("/channels/{c}/messages/{m}/reactions/blob%3A123/@me"));
    let del = mock.wait_for_request("DELETE", "/channels/*/messages/*/reactions/*/0/@me").await;
    assert_eq!(del.path, format!("/channels/{c}/messages/{m}/reactions/blob%3A123/0/@me"));
    assert_eq!(del.query_param("burst"), Some("false"));

    let msg = mock.with_world(|w| w.message(c, m)).unwrap();
    assert_eq!(msg["reactions"].as_array().unwrap().len(), 1, "custom reaction removed again");
    assert_eq!(msg["reactions"][0]["emoji"]["name"], "👍");
}

#[tokio::test]
async fn friend_requests_split_legacy_discriminators() {
    let (mock, http) = setup().await;
    for name in ["legacy#1234", "newname", "odd#12", "a#b#0001"] {
        http.send_friend_request(name).await.unwrap();
    }
    let reqs = mock.requests_to("POST", "/users/@me/relationships");
    let bodies: Vec<Value> = reqs.iter().map(|r| r.json()).collect();
    assert_eq!(bodies[0], json!({ "username": "legacy", "discriminator": "1234" }));
    assert_eq!(bodies[1], json!({ "username": "newname", "discriminator": null }));
    assert_eq!(bodies[2], json!({ "username": "odd#12", "discriminator": null }));
    assert_eq!(bodies[3], json!({ "username": "a#b", "discriminator": "0001" }));
    assert_eq!(reqs[0].base64_json_header("x-context-properties"), Some(json!({ "location": "Add Friend" })));
}

#[tokio::test]
async fn ack_bodies() {
    let (mock, http) = setup().await;
    let (c, m) = (Snowflake(ids::GENERAL), Snowflake(1_300_000_000_000_000_000));
    http.ack_message(c, m, false).await.unwrap();
    http.ack_message(c, m, true).await.unwrap();
    let reqs = mock.requests_to("POST", "/channels/*/messages/*/ack");
    let auto = reqs[0].json();
    assert_eq!(auto["token"], Value::Null);
    let days = auto["last_viewed"].as_u64().expect("last_viewed is a day count");
    assert!((3_000..10_000).contains(&days), "days since 2015: {days}");
    assert_eq!(reqs[1].json(), json!({ "token": null, "manual": true, "mention_count": 0 }));
    assert_eq!(mock.with_world(|w| w.read_states.get(&ids::GENERAL).copied()), Some(m.0));
}

#[tokio::test]
async fn message_crud_and_pins_against_the_world() {
    let (mock, http) = setup().await;
    let c = Snowflake(ids::GENERAL);
    let sent = http
        .send_message(c, &CreateMessage { content: "hi".into(), nonce: Some("1".into()), enforce_nonce: Some(true), ..Default::default() }, vec![])
        .await
        .unwrap();
    assert_eq!(sent.content, "hi");
    assert_eq!(sent.author.id, Snowflake(ids::ME));
    assert_eq!(sent.nonce, Some(json!("1")));
    // Same nonce with enforce_nonce: the server returns the original message.
    let dup = http
        .send_message(c, &CreateMessage { content: "hi".into(), nonce: Some("1".into()), enforce_nonce: Some(true), ..Default::default() }, vec![])
        .await
        .unwrap();
    assert_eq!(dup.id, sent.id);

    let edited = http.edit_message(c, sent.id, "edited").await.unwrap();
    assert_eq!(edited.content, "edited");
    assert!(edited.is_edited());

    let pinned_id = mock.with_world(|w| {
        let id = w.next_snowflake();
        w.insert_message(MessageBuilder::new(id, ids::GENERAL, &UserBuilder::friend()).content("pinned!").pinned().build());
        id
    });
    let pins = http.pins(c).await.unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].id.0, pinned_id);

    http.trigger_typing(c).await.unwrap();
    http.delete_message(c, sent.id).await.unwrap();
    assert!(mock.with_world(|w| w.message(ids::GENERAL, sent.id.0)).is_none());
    assert!(matches!(http.send_message(c, &CreateMessage::default(), vec![]).await, Err(e) if e.code() == Some(50006)));
}
