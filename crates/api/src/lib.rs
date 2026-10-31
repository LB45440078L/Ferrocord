//! Ferrocord's local control API: an HTTP + WebSocket server on `127.0.0.1`
//! that lets external tools (scripts, stream decks, status bars…) read client
//! state, receive live events and — when the user allows it — act.
//!
//! Security model:
//! * binds to the loopback interface only;
//! * every request needs `Authorization: Bearer <token>` (WebSocket also
//!   accepts `?token=`); the token lives in `<config_dir>/api-token.txt`;
//! * requests whose `Host` is not `127.0.0.1:<port>`/`localhost:<port>` are
//!   rejected (DNS rebinding), as are requests with a foreign `Origin` header
//!   (web pages), so browsers cannot drive the API;
//! * write actions return 403 unless `config.api.allow_actions` is set; the
//!   flag is re-read on every request.
//!
//! See `docs/API.md` for the endpoint reference.

mod auth;
mod error;
mod ops;
mod rest;
mod ws;

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post, put};
use ferrocord_core::Client;
use parking_lot::{Mutex, RwLock};
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use auth::{TOKEN_FILE, load_or_create_token, regenerate_token, token_path};
pub use error::ApiError;
pub use ops::{API_VERSION, METHODS};

/// Shared state of the route handlers.
#[derive(Clone)]
pub(crate) struct AppState {
    pub client: Client,
    pub shutdown: watch::Receiver<bool>,
}

/// Entry point for starting the API server.
pub struct ApiServer;

impl ApiServer {
    /// Binds `127.0.0.1:<config.api.port>` (port `0` picks a free port) and
    /// starts serving on the client's runtime. Creates the token file on first
    /// start. The caller decides whether the API is enabled
    /// (`config.api.enabled`); dropping the handle stops the server.
    pub fn start(client: Client) -> io::Result<ApiServerHandle> {
        let port = client.config().api.port;
        let token = Arc::new(RwLock::new(load_or_create_token(client.paths())?));
        let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        std_listener.set_nonblocking(true)?;
        let addr = std_listener.local_addr()?;
        let listener = {
            let _rt = client.runtime().enter();
            tokio::net::TcpListener::from_std(std_listener)?
        };
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let app = router(AppState { client: client.clone(), shutdown: shutdown_rx.clone() }, auth::Guard { token: token.clone(), port: addr.port() });
        let mut stop = shutdown_rx;
        let task = client.runtime().spawn(async move {
            let shutdown = async move {
                let _ = stop.wait_for(|s| *s).await;
            };
            if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
                tracing::error!(error = %e, "local API server failed");
            }
        });
        tracing::info!(%addr, "local API listening");
        Ok(ApiServerHandle { addr, token, client, shutdown: shutdown_tx, task: Mutex::new(Some(task)) })
    }
}

fn router(state: AppState, guard: auth::Guard) -> Router {
    Router::new()
        .route("/v1/info", get(rest::info))
        .route("/v1/me", get(rest::me))
        .route("/v1/guilds", get(rest::guilds))
        .route("/v1/guilds/{id}", get(rest::guild))
        .route("/v1/guilds/{id}/channels", get(rest::guild_channels))
        .route("/v1/channels/{id}", get(rest::channel))
        .route("/v1/channels/{id}/messages", get(rest::messages).post(rest::send_message))
        .route("/v1/channels/{id}/typing", post(rest::typing))
        .route("/v1/channels/{id}/ack", post(rest::ack))
        .route("/v1/channels/{cid}/messages/{mid}/reactions/{emoji}", put(rest::add_reaction).delete(rest::remove_reaction))
        .route("/v1/users/{id}", get(rest::user))
        .route("/v1/dms", get(rest::dms))
        .route("/v1/relationships", get(rest::relationships))
        .route("/v1/presence", axum::routing::patch(rest::presence))
        .route("/v1/ws", get(ws::handler))
        .fallback(rest::not_found)
        .method_not_allowed_fallback(rest::method_not_allowed)
        .layer(axum::middleware::from_fn_with_state(guard, auth::guard))
        .with_state(state)
}

/// A running API server. Dropping it stops the server.
pub struct ApiServerHandle {
    addr: SocketAddr,
    token: Arc<RwLock<String>>,
    client: Client,
    shutdown: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl ApiServerHandle {
    /// The bound address (always on 127.0.0.1).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The bearer token clients must present.
    pub fn token(&self) -> String {
        self.token.read().clone()
    }

    /// Replaces the token (file and running server); existing WebSocket
    /// sessions stay open, new requests need the new token.
    pub fn regenerate_token(&self) -> io::Result<String> {
        let t = regenerate_token(self.client.paths())?;
        *self.token.write() = t.clone();
        Ok(t)
    }

    pub fn is_running(&self) -> bool {
        self.task.lock().as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Stops accepting connections and closes open WebSocket sessions.
    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.lock().take() {
            // In-flight HTTP requests get a moment to finish, then the server is torn down.
            self.client.runtime().spawn(async move {
                let abort = task.abort_handle();
                if tokio::time::timeout(std::time::Duration::from_secs(5), task).await.is_err() {
                    abort.abort();
                }
            });
        }
    }
}

impl Drop for ApiServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}
