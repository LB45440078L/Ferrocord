//! `ferrocord-auth` against the mock Discord: password login, MFA, token
//! validation and QR (remote auth v2) login.

use std::sync::Arc;
use std::time::Duration;

use ferrocord_auth::remote::{self, RemoteAuthEvent};
use ferrocord_auth::{AuthError, LoginOutcome, MfaMethod, MfaStep, login};
use ferrocord_http::{BoxFuture, CaptchaChallenge, CaptchaSolution, CaptchaSolver, ClientProfile, Http};
use ferrocord_model::Snowflake;
use ferrocord_testkit::fixtures::{DEFAULT_EMAIL, DEFAULT_PASSWORD, UserBuilder, ids};
use ferrocord_testkit::{CaptchaGate, MfaSetup, MockDiscord, MockResponse, RemoteAuthBehaviour};
use serde_json::json;
use tokio::sync::mpsc;

async fn setup() -> (MockDiscord, Http) {
    let mock = MockDiscord::start().await;
    let http = Http::with_base_url(ClientProfile::desktop(), &mock.api_base());
    (mock, http)
}

fn enable_mfa(mock: &MockDiscord) -> MfaSetup {
    let mfa = MfaSetup::default();
    mock.with_world(|w| w.account.mfa = Some(mfa.clone()));
    mfa
}

async fn mfa_step(mock: &MockDiscord, http: &Http) -> MfaStep {
    match login::login(http, DEFAULT_EMAIL, DEFAULT_PASSWORD).await.expect("login") {
        LoginOutcome::Mfa(step) => step,
        other => panic!("expected MFA, got {other:?} ({} requests)", mock.requests().len()),
    }
}

// ---------------------------------------------------------------- password login

#[tokio::test]
async fn password_login_success() {
    let (mock, http) = setup().await;
    let outcome = login::login(&http, DEFAULT_EMAIL, DEFAULT_PASSWORD).await.unwrap();
    let LoginOutcome::Success { token, user_id } = outcome else { panic!("{outcome:?}") };
    assert_eq!(token, mock.token());
    assert_eq!(user_id, Some(Snowflake(ids::ME)));

    let reqs = mock.requests();
    assert_eq!(reqs[0].path, "/experiments", "fingerprint is fetched first");
    let req = mock.wait_for_request("POST", "/auth/login").await;
    assert!(req.has_header("x-fingerprint"));
    assert!(!req.has_header("authorization"));
    assert_eq!(req.json(), json!({ "login": DEFAULT_EMAIL, "password": DEFAULT_PASSWORD, "undelete": false, "login_source": null, "gift_code_sku_id": null }));
}

#[tokio::test]
async fn invalid_credentials() {
    let (_mock, http) = setup().await;
    let err = login::login(&http, DEFAULT_EMAIL, "wrong").await.unwrap_err();
    assert!(matches!(&err, AuthError::InvalidCredentials(m) if m == "Login or password is invalid."), "{err:?}");
}

#[tokio::test]
async fn suspended_account() {
    let (mock, http) = setup().await;
    mock.with_world(|w| w.account.suspended_token = Some("suspended-token".into()));
    let err = login::login(&http, DEFAULT_EMAIL, DEFAULT_PASSWORD).await.unwrap_err();
    assert!(matches!(&err, AuthError::Suspended(t) if t == "suspended-token"), "{err:?}");
}

#[tokio::test]
async fn new_login_location_and_missing_token() {
    let (mock, http) = setup().await;
    mock.on("POST", "/auth/login").once().respond(MockResponse::error(400, 20013, "New login location detected, please check your e-mail."));
    let err = login::login(&http, DEFAULT_EMAIL, DEFAULT_PASSWORD).await.unwrap_err();
    assert!(matches!(&err, AuthError::NewLoginLocation(m) if m.contains("New login location")), "{err:?}");

    mock.on("POST", "/auth/login").once().respond(MockResponse::ok(json!({ "user_id": ids::ME.to_string() })));
    let outcome = login::login(&http, DEFAULT_EMAIL, DEFAULT_PASSWORD).await.unwrap();
    assert!(matches!(outcome, LoginOutcome::VerificationRequired { .. }), "{outcome:?}");
}

struct Solver(Option<String>);

impl CaptchaSolver for Solver {
    fn solve(&self, c: CaptchaChallenge) -> BoxFuture<'static, Option<CaptchaSolution>> {
        let key = self.0.clone();
        Box::pin(async move { key.map(|key| CaptchaSolution { key, rqtoken: c.rqtoken, session_id: c.session_id }) })
    }
}

#[tokio::test]
async fn login_captcha_solved_or_cancelled() {
    let (mock, http) = setup().await;
    let gate = CaptchaGate::default();
    mock.require_captcha("POST", "/auth/login", gate.clone());

    http.set_captcha_solver(Some(Arc::new(Solver(None))));
    let err = login::login(&http, DEFAULT_EMAIL, DEFAULT_PASSWORD).await.unwrap_err();
    assert!(matches!(err, AuthError::CaptchaCancelled), "{err:?}");

    http.set_captcha_solver(Some(Arc::new(Solver(Some(gate.solution.clone())))));
    let outcome = login::login(&http, DEFAULT_EMAIL, DEFAULT_PASSWORD).await.unwrap();
    assert!(matches!(outcome, LoginOutcome::Success { .. }), "{outcome:?}");
    let last = mock.requests_to("POST", "/auth/login").pop().unwrap();
    assert!(last.has_header("x-fingerprint"), "replay keeps the fingerprint");
}

// ---------------------------------------------------------------- MFA

#[tokio::test]
async fn mfa_totp_login() {
    let (mock, http) = setup().await;
    let mfa = enable_mfa(&mock);
    let step = mfa_step(&mock, &http).await;
    assert_eq!(step.methods(), vec![MfaMethod::Totp, MfaMethod::Sms, MfaMethod::Backup]);
    assert_eq!(step.user_id, Some(Snowflake(ids::ME)));
    assert_eq!(step.login_instance_id.as_deref(), Some("mock-login-instance"));

    let spaced = format!("{} {}", &mfa.totp_code[..3], &mfa.totp_code[3..]);
    let token = login::verify_mfa(&http, &step, MfaMethod::Totp, &spaced).await.unwrap();
    assert_eq!(token, mock.token());
    let req = mock.wait_for_request("POST", "/auth/mfa/totp").await;
    assert_eq!(
        req.json(),
        json!({ "code": mfa.totp_code, "ticket": step.ticket, "login_source": null, "gift_code_sku_id": null, "login_instance_id": "mock-login-instance" })
    );
}

#[tokio::test]
async fn mfa_backup_code_is_normalized() {
    let (mock, http) = setup().await;
    let mfa = enable_mfa(&mock);
    let step = mfa_step(&mock, &http).await;
    let dashed = format!("{}-{}", &mfa.backup_code[..4], &mfa.backup_code[4..]);
    let token = login::verify_mfa(&http, &step, MfaMethod::Backup, &dashed).await.unwrap();
    assert_eq!(token, mock.token());
    assert_eq!(mock.wait_for_request("POST", "/auth/mfa/backup").await.json()["code"], mfa.backup_code);
}

#[tokio::test]
async fn mfa_invalid_code() {
    let (mock, http) = setup().await;
    enable_mfa(&mock);
    let step = mfa_step(&mock, &http).await;
    let err = login::verify_mfa(&http, &step, MfaMethod::Totp, "000000").await.unwrap_err();
    assert!(matches!(&err, AuthError::InvalidMfaCode(m) if m == "Invalid two-factor code"), "{err:?}");
}

#[tokio::test]
async fn mfa_sms_send_and_verify() {
    let (mock, http) = setup().await;
    let mfa = enable_mfa(&mock);
    let step = mfa_step(&mock, &http).await;
    let phone = login::send_sms(&http, &step).await.unwrap();
    assert_eq!(phone, "+*******0123");
    assert_eq!(mock.wait_for_request("POST", "/auth/mfa/sms/send").await.json(), json!({ "ticket": step.ticket }));
    let token = login::verify_mfa(&http, &step, MfaMethod::Sms, &mfa.sms_code).await.unwrap();
    assert_eq!(token, mock.token());
}

// ---------------------------------------------------------------- tokens

#[tokio::test]
async fn token_validation() {
    let (mock, http) = setup().await;
    let user = login::validate_token(&http, &format!(" \"{}\" ", mock.token())).await.unwrap();
    assert_eq!(user.user.id, Snowflake(ids::ME));
    assert_eq!(user.user.username, "mockuser");
    let req = mock.wait_for_request("GET", "/users/@me").await;
    assert_eq!(req.header("authorization"), Some(mock.token().as_str()), "quotes and whitespace are trimmed");

    let err = login::validate_token(&http, "not-a-token").await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidToken), "{err:?}");
}

// ---------------------------------------------------------------- QR login

async fn next(rx: &mut mpsc::Receiver<RemoteAuthEvent>) -> RemoteAuthEvent {
    tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("remote auth event in time").expect("channel open")
}

async fn expect_qr(rx: &mut mpsc::Receiver<RemoteAuthEvent>) -> String {
    match next(rx).await {
        RemoteAuthEvent::QrCode(u) => u,
        other => panic!("expected QR code, got {other:?}"),
    }
}

#[tokio::test]
async fn qr_login_full_flow() {
    let (mock, http) = setup().await;
    let scanner = UserBuilder::new(ids::FRIEND, "phoneuser").discriminator("4321").avatar("abcdef0123456789");
    mock.remote_auth().set_scanning_user(scanner);
    mock.remote_auth().set_token("qr-issued-token");
    mock.with_world(|w| w.add_valid_token("qr-issued-token"));

    let mut rx = remote::start_with_url(http.clone(), mock.remote_auth_url());
    let url = expect_qr(&mut rx).await;
    let RemoteAuthEvent::Scanned(user) = next(&mut rx).await else { panic!("expected scan") };
    assert_eq!((user.id.as_str(), user.discriminator.as_str(), user.username.as_str()), (ids::FRIEND.to_string().as_str(), "4321", "phoneuser"));
    assert_eq!(user.avatar.as_deref(), Some("abcdef0123456789"));
    assert!(user.avatar_url().unwrap().contains("/avatars/"));
    let RemoteAuthEvent::Token(token) = next(&mut rx).await else { panic!("expected token") };
    assert_eq!(token, "qr-issued-token");

    let s = &mock.remote_auth().sessions()[0];
    assert_eq!(url, format!("https://discord.com/ra/{}", s.fingerprint.as_deref().unwrap()));
    assert!(s.nonce_verified, "client proved it can decrypt with its key");
    assert_eq!(s.origin.as_deref(), Some("https://discord.com"));
    assert_eq!(&s.client_ops[..2], &["init".to_string(), "nonce_proof".to_string()]);
    let login = mock.wait_for_request("POST", "/users/@me/remote-auth/login").await;
    assert_eq!(login.json(), json!({ "ticket": s.ticket.clone().unwrap() }));
    assert!(login.has_header("x-super-properties"));
    // The task ends after delivering the token.
    assert!(tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().is_none());
}

#[tokio::test]
async fn qr_scanned_user_without_avatar() {
    let (mock, http) = setup().await;
    mock.remote_auth().set_scanning_user(UserBuilder::new(ids::FRIEND, "with:colon"));
    let mut rx = remote::start_with_url(http, mock.remote_auth_url());
    expect_qr(&mut rx).await;
    let RemoteAuthEvent::Scanned(user) = next(&mut rx).await else { panic!("expected scan") };
    assert_eq!(user.avatar, None, "avatar \"0\" means none");
    assert_eq!(user.username, "with:colon", "username may contain colons");
    assert!(user.avatar_url().unwrap().contains("/embed/avatars/"));
}

#[tokio::test]
async fn qr_cancel_then_new_code() {
    let (mock, http) = setup().await;
    mock.remote_auth().script([RemoteAuthBehaviour::CancelAfterScan]);
    let mut rx = remote::start_with_url(http, mock.remote_auth_url());
    let first = expect_qr(&mut rx).await;
    assert!(matches!(next(&mut rx).await, RemoteAuthEvent::Scanned(_)));
    assert!(matches!(next(&mut rx).await, RemoteAuthEvent::Cancelled));
    let second = expect_qr(&mut rx).await;
    assert_ne!(first, second, "a new key pair means a new code");
    assert!(matches!(next(&mut rx).await, RemoteAuthEvent::Scanned(_)));
    assert!(matches!(next(&mut rx).await, RemoteAuthEvent::Token(t) if t == mock.token()));
    assert_eq!(mock.remote_auth().sessions().len(), 2);
}

#[tokio::test]
async fn qr_expired_code_reconnects() {
    let (mock, http) = setup().await;
    mock.remote_auth().script([RemoteAuthBehaviour::ExpireAfterInit]);
    let mut rx = remote::start_with_url(http, mock.remote_auth_url());
    let first = expect_qr(&mut rx).await;
    let second = expect_qr(&mut rx).await;
    assert_ne!(first, second);
}

#[tokio::test]
async fn qr_fingerprint_mismatch_is_rejected() {
    let (mock, http) = setup().await;
    mock.remote_auth().script([RemoteAuthBehaviour::WrongFingerprint]);
    let mut rx = remote::start_with_url(http, mock.remote_auth_url());
    let RemoteAuthEvent::Error(e) = next(&mut rx).await else { panic!("expected an error before any QR code") };
    assert!(e.contains("fingerprint mismatch"), "{e}");
    // Retries with a fresh session, which succeeds.
    expect_qr(&mut rx).await;
}

#[tokio::test]
async fn qr_heartbeats_are_acked() {
    let (mock, http) = setup().await;
    mock.remote_auth().set_heartbeat_interval(250);
    mock.remote_auth().script([RemoteAuthBehaviour::Idle]);
    let mut rx = remote::start_with_url(http, mock.remote_auth_url());
    expect_qr(&mut rx).await;
    let sessions = mock.remote_auth().wait_for_sessions(1, |s| s.client_ops.iter().filter(|o| *o == "heartbeat").count() >= 3).await;
    assert_eq!(sessions.len(), 1, "no reconnect: every heartbeat was acknowledged");
    drop(rx);
}
