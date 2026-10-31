//! A scriptable mock of Discord for Ferrocord's integration tests.
//!
//! [`MockDiscord::start`] binds `127.0.0.1:0` and serves, on one port:
//!
//! * the REST API under `/api/v9` ([`MockDiscord::api_base`]),
//! * a gateway websocket with zlib-stream compression ([`MockDiscord::gateway_url`]),
//! * a remote-auth (QR login) v2 websocket ([`MockDiscord::remote_auth_url`]).
//!
//! The server runs on its own thread and Tokio runtime, so it keeps working
//! regardless of how the test's runtime is configured, and it shuts down when
//! the [`MockDiscord`] is dropped.
//!
//! Behaviour comes from three places, checked in this order for every REST call:
//!
//! 1. scripted routes installed with [`MockDiscord::on`] (newest first; a handler
//!    may return `None` to fall through),
//! 2. a small in-memory [`World`] (account, login/MFA, channels and messages,
//!    reactions, read states, settings) that also emits the matching gateway
//!    dispatches,
//! 3. a `404 {"code":0}` fallback.
//!
//! Every request, gateway frame and remote-auth exchange is recorded so tests
//! can assert on exactly what the client sent.
//!
//! No real Discord host is ever contacted: the client under test must be pointed
//! at this server (`Http::with_base_url`, `GatewayConfig::url`,
//! `remote::start_with_url`, `ClientOptions::custom_endpoints`).

pub mod fixtures;
mod gateway;
mod remote_auth;
mod rest;
mod scenarios;
mod world;
pub mod zlib;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::{Notify, oneshot};

pub use gateway::{ClientFrame, ConnectionInfo, GatewayControl};
pub use remote_auth::{RemoteAuthBehaviour, RemoteAuthControl, RemoteAuthSessionInfo};
pub use rest::{MockResponse, MultipartPart, RecordedRequest, RouteBuilder};
pub use scenarios::{CaptchaGate, MfaGate};
pub use world::{Account, MfaSetup, World};

/// How long the `wait_for_*` helpers wait before panicking.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(5);

pub(crate) struct Shared {
    pub http_base: String,
    pub ws_base: String,
    pub requests: Mutex<Vec<RecordedRequest>>,
    pub routes: Mutex<Vec<rest::Route>>,
    pub delays: Mutex<Vec<(String, rest::Pattern, Duration)>>,
    pub next_route: AtomicUsize,
    pub world: Mutex<World>,
    pub gateway: Mutex<gateway::HubState>,
    pub remote: Mutex<remote_auth::RemoteState>,
    /// Signalled whenever anything observable changes (request recorded, frame received, …).
    pub changed: Notify,
}

impl Shared {
    pub fn bump(&self) {
        self.changed.notify_waiters();
    }

    /// Polls `check` every time the mock's state changes until it returns `Some`.
    pub async fn wait_for<T>(&self, timeout: Duration, mut check: impl FnMut() -> Option<T>) -> Option<T> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(v) = check() {
                return Some(v);
            }
            // Also re-check periodically in case a change raced the registration above.
            let tick = tokio::time::Instant::now() + Duration::from_millis(50);
            if tokio::time::timeout_at(deadline.min(tick), notified).await.is_err() && tokio::time::Instant::now() >= deadline {
                return check();
            }
        }
    }
}

/// A running mock Discord server. Dropping it stops the server.
pub struct MockDiscord {
    shared: Arc<Shared>,
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
}

impl MockDiscord {
    /// Starts a mock with the default [`Account`] and the standard READY fixture
    /// ([`fixtures::ReadyBuilder::standard`]).
    pub async fn start() -> MockDiscord {
        install_crypto_provider();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        listener.set_nonblocking(true).expect("nonblocking listener");
        let addr = listener.local_addr().expect("local addr");
        let shared = Arc::new(Shared {
            http_base: format!("http://{addr}"),
            ws_base: format!("ws://{addr}"),
            requests: Mutex::new(vec![]),
            routes: Mutex::new(vec![]),
            delays: Mutex::new(vec![]),
            next_route: AtomicUsize::new(0),
            world: Mutex::new(World::new(format!("ws://{addr}"))),
            gateway: Mutex::new(gateway::HubState::default()),
            remote: Mutex::new(remote_auth::RemoteState::default()),
            changed: Notify::new(),
        });
        let mut mock = MockDiscord { shared: shared.clone(), addr, shutdown: None };
        mock.set_ready(fixtures::ReadyBuilder::standard());

        let app = rest::router(shared);
        let (tx, rx) = oneshot::channel::<()>();
        std::thread::Builder::new()
            .name(format!("mock-discord-{}", addr.port()))
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("mock runtime");
                rt.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
                    tokio::select! {
                        r = axum::serve(listener, app) => { if let Err(e) = r { tracing::error!(error = %e, "mock server failed"); } }
                        _ = rx => {}
                    }
                });
                rt.shutdown_background();
            })
            .expect("spawn mock thread");
        mock.shutdown = Some(tx);
        mock
    }

    // ------------------------------------------------------------ endpoints

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// REST root including the version, e.g. `http://127.0.0.1:1234/api/v9`.
    pub fn api_base(&self) -> String {
        format!("{}/api/v9", self.shared.http_base)
    }

    /// Gateway base URL (the client appends `/?encoding=json&v=9&compress=zlib-stream`).
    pub fn gateway_url(&self) -> String {
        format!("{}/gateway", self.shared.ws_base)
    }

    /// The `resume_gateway_url` sent in READY. Connections to it are recorded with path `/resume/`.
    pub fn resume_gateway_url(&self) -> String {
        format!("{}/resume", self.shared.ws_base)
    }

    /// Remote-auth (QR login) websocket URL.
    pub fn remote_auth_url(&self) -> String {
        format!("{}/remote-auth/?v=2", self.shared.ws_base)
    }

    /// The default account's token (always valid unless revoked).
    pub fn token(&self) -> String {
        self.shared.world.lock().account.token.clone()
    }

    // ------------------------------------------------------------ REST recording

    /// Every REST request received so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.shared.requests.lock().clone()
    }

    /// Requests whose method and path match (`*` matches one segment, `**` the rest).
    pub fn requests_to(&self, method: &str, pattern: &str) -> Vec<RecordedRequest> {
        let pat = rest::Pattern::parse(pattern);
        self.shared.requests.lock().iter().filter(|r| r.method.eq_ignore_ascii_case(method) && pat.matches(&r.path)).cloned().collect()
    }

    pub fn clear_requests(&self) {
        self.shared.requests.lock().clear();
    }

    /// Waits for the first request matching `method` + `pattern`.
    pub async fn wait_for_request(&self, method: &str, pattern: &str) -> RecordedRequest {
        self.wait_for_requests(method, pattern, 1).await.remove(0)
    }

    /// Waits until at least `n` matching requests were received and returns all of them.
    pub async fn wait_for_requests(&self, method: &str, pattern: &str, n: usize) -> Vec<RecordedRequest> {
        let got = self
            .shared
            .wait_for(DEFAULT_WAIT, || {
                let v = self.requests_to(method, pattern);
                (v.len() >= n).then_some(v)
            })
            .await;
        match got {
            Some(v) => v,
            None => {
                let seen: Vec<String> = self.requests().iter().map(|r| format!("{} {}", r.method, r.path)).collect();
                panic!("timed out waiting for {n}x {method} {pattern}; received: {seen:#?}")
            }
        }
    }

    /// Waits for an arbitrary condition over the mock's state (re-evaluated on every change).
    pub async fn wait_until<T>(&self, what: &str, check: impl FnMut() -> Option<T>) -> T {
        match self.shared.wait_for(DEFAULT_WAIT, check).await {
            Some(v) => v,
            None => panic!("timed out waiting for {what}"),
        }
    }

    // ------------------------------------------------------------ scripting

    /// Installs a scripted REST route. Newer routes take precedence over older ones
    /// and over the built-in [`World`] behaviour.
    pub fn on(&self, method: &str, pattern: &str) -> RouteBuilder<'_> {
        RouteBuilder::new(&self.shared, method, pattern)
    }

    /// Delays every matching request by `d` before it is answered (by a scripted
    /// route or the world), e.g. to observe a client's optimistic state.
    pub fn delay_route(&self, method: &str, pattern: &str, d: Duration) {
        self.shared.delays.lock().push((method.to_ascii_uppercase(), rest::Pattern::parse(pattern), d));
    }

    /// Removes every scripted route and delay (the [`World`] behaviour remains).
    pub fn clear_routes(&self) {
        self.shared.routes.lock().clear();
        self.shared.delays.lock().clear();
    }

    /// Runs `f` with exclusive access to the in-memory world.
    ///
    /// The world lock is held while `f` runs: do not call other `MockDiscord`
    /// methods (such as [`MockDiscord::token`]) inside `f`, or it deadlocks.
    pub fn with_world<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        let r = f(&mut self.shared.world.lock());
        self.shared.bump();
        r
    }

    /// Sets the READY (and READY_SUPPLEMENTAL) sent after IDENTIFY, registers its
    /// channels in the world and adopts its settings proto.
    pub fn set_ready(&self, ready: fixtures::ReadyBuilder) {
        {
            let mut w = self.shared.world.lock();
            for (c, g) in ready.channels() {
                w.add_channel(c, g);
            }
            w.settings = ready.settings_proto().clone();
            w.register_user(ready.user_value());
            for u in ready.user_values() {
                w.register_user(u);
            }
        }
        let mut g = self.shared.gateway.lock();
        g.ready = Some(ready.build());
        g.ready_supplemental = Some(ready.build_supplemental());
    }

    /// Replaces the READY payload verbatim (no world registration).
    pub fn set_ready_value(&self, ready: Value, supplemental: Option<Value>) {
        let mut g = self.shared.gateway.lock();
        g.ready = Some(ready);
        g.ready_supplemental = supplemental;
    }

    /// Gateway controls and recordings.
    pub fn gateway(&self) -> GatewayControl<'_> {
        GatewayControl { shared: &self.shared }
    }

    /// Remote-auth controls and recordings.
    pub fn remote_auth(&self) -> RemoteAuthControl<'_> {
        RemoteAuthControl { shared: &self.shared }
    }
}

impl Drop for MockDiscord {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Installs the rustls crypto provider used by the workspace (idempotent).
pub fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}
