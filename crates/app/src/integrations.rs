//! Glue between the UI and the plugin host / local API server, plus small
//! channels used to hand background results back to the UI thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use ferrocord_api::{ApiServer, ApiServerHandle};
use ferrocord_core::model::{Message, Snowflake, UserProfile};
use ferrocord_core::{Client, Notification};
use ferrocord_plugin::{PluginHost, PluginInfo};
use parking_lot::Mutex;

#[derive(Clone)]
pub struct PluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub enabled: bool,
    pub error: Option<String>,
    pub declared: Vec<String>,
    pub granted: Vec<String>,
}

impl From<PluginInfo> for PluginRow {
    fn from(p: PluginInfo) -> Self {
        Self {
            id: p.manifest.id.clone(),
            name: if p.manifest.name.is_empty() { p.manifest.id.clone() } else { p.manifest.name.clone() },
            version: p.manifest.version.clone(),
            author: p.manifest.author.clone(),
            description: p.manifest.description.clone(),
            enabled: p.enabled,
            error: p.error.clone(),
            declared: p.permissions.iter().map(|s| s.name.clone()).collect(),
            granted: p.permissions.iter().filter(|s| s.granted).map(|s| s.name.clone()).collect(),
        }
    }
}

pub struct Integrations {
    client: Client,
    plugins: PluginHost,
    plugin_rows: Arc<Mutex<Vec<PluginRow>>>,
    api: Arc<Mutex<Option<ApiServerHandle>>>,
    api_error: Arc<Mutex<Option<String>>>,
    pub nav_tx: mpsc::Sender<Snowflake>,
    pub nav_rx: mpsc::Receiver<Snowflake>,
    pub pins_tx: mpsc::Sender<(Snowflake, Result<Vec<Message>, String>)>,
    pub pins_rx: mpsc::Receiver<(Snowflake, Result<Vec<Message>, String>)>,
    pub join_tx: mpsc::Sender<String>,
    pub join_rx: mpsc::Receiver<String>,
    pub search_tx: mpsc::Sender<Result<(u64, Vec<Message>), String>>,
    pub search_rx: mpsc::Receiver<Result<(u64, Vec<Message>), String>>,
    /// Full profiles fetched on demand (`None` = request in flight or failed).
    pub profiles: Arc<Mutex<HashMap<Snowflake, Option<UserProfile>>>>,
    /// Per-user voice volume chosen in this session.
    volumes: HashMap<Snowflake, f32>,
    /// Uploaded video textures per user, with the frame timestamp they show.
    video_textures: HashMap<Snowflake, (std::time::Instant, egui::TextureHandle)>,
}

impl Integrations {
    /// Loads enabled plugins and starts the local API if configured.
    /// `demo` skips both so the offline demo never runs user code or opens ports.
    pub fn start(client: &Client, demo: bool) -> Self {
        let (nav_tx, nav_rx) = mpsc::channel();
        let (pins_tx, pins_rx) = mpsc::channel();
        let (join_tx, join_rx) = mpsc::channel();
        let (search_tx, search_rx) = mpsc::channel();
        let plugins = PluginHost::new(client.clone());
        plugins.set_notifier(|title, body| {
            let n = Notification { title, body, channel_id: Snowflake(0), guild_id: None, message_id: Snowflake(0), icon_url: None };
            crate::notify::show(&n, true, true);
        });
        let this = Self {
            client: client.clone(),
            plugins,
            plugin_rows: Arc::default(),
            api: Arc::default(),
            api_error: Arc::default(),
            nav_tx,
            nav_rx,
            pins_tx,
            pins_rx,
            join_tx,
            join_rx,
            search_tx,
            search_rx,
            profiles: Arc::default(),
            volumes: HashMap::new(),
            video_textures: HashMap::new(),
        };
        if demo {
            this.rescan();
        } else {
            let (host, rows) = (this.plugins.clone(), this.plugin_rows.clone());
            std::thread::spawn(move || {
                for (id, e) in host.load_enabled() {
                    tracing::warn!(plugin = %id, error = %e, "plugin failed to load");
                }
                *rows.lock() = host.discover().into_iter().map(PluginRow::from).collect();
            });
            this.apply_api_config();
        }
        this
    }

    pub fn stop(&mut self) {
        if let Some(h) = self.api.lock().take() {
            h.stop();
        }
        self.plugins.shutdown();
    }

    /// Returns the cached full profile, fetching it in the background if needed.
    pub fn profile(&self, user: Snowflake, guild: Option<Snowflake>) -> Option<UserProfile> {
        let mut cache = self.profiles.lock();
        if let Some(p) = cache.get(&user) {
            return p.clone();
        }
        cache.insert(user, None);
        let (client, profiles) = (self.client.clone(), self.profiles.clone());
        self.client.runtime().spawn(async move {
            if let Ok(p) = client.http().user_profile(user, guild).await {
                profiles.lock().insert(user, Some(p));
                client.request_repaint();
            }
        });
        None
    }

    // ------------------------------------------------------------ voice & video

    pub fn user_volume(&self, user: Snowflake) -> f32 {
        self.volumes.get(&user).copied().unwrap_or(1.0)
    }

    pub fn set_user_volume(&mut self, user: Snowflake, volume: f32) {
        self.volumes.insert(user, volume);
    }

    /// Latest camera frame of a call participant (our own preview for `me`), as a texture.
    pub fn video_frame(&mut self, ctx: &egui::Context, client: &Client, user: Snowflake) -> Option<crate::images::Img> {
        let shared = client.voice_shared()?;
        let frame = if user == client.store().my_id() { shared.local_video.read().clone() } else { shared.remote_video.read().get(&user.0).cloned() };
        let Some(frame) = frame else {
            self.video_textures.remove(&user);
            return None;
        };
        let size = egui::vec2(frame.width as f32, frame.height as f32);
        let stale = self.video_textures.get(&user).is_none_or(|(ts, _)| *ts != frame.timestamp);
        if stale {
            let img = egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.data);
            match self.video_textures.get_mut(&user) {
                Some((ts, tex)) => {
                    tex.set(img, egui::TextureOptions::LINEAR);
                    *ts = frame.timestamp;
                }
                None => {
                    let tex = ctx.load_texture(format!("video-{user}"), img, egui::TextureOptions::LINEAR);
                    self.video_textures.insert(user, (frame.timestamp, tex));
                }
            }
        }
        // Video plays continuously; keep frames flowing while tiles are visible.
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
        self.video_textures.get(&user).map(|(_, t)| crate::images::Img { texture: t.id(), size })
    }

    pub fn toggle_camera(&mut self, client: &Client) -> Result<(), String> {
        let on = client.voice_status().is_some_and(|s| s.self_video);
        client.set_video(!on)
    }

    // ------------------------------------------------------------ plugins

    pub fn plugins_dir(&self) -> PathBuf {
        self.plugins.plugins_dir()
    }

    pub fn rescan(&self) {
        *self.plugin_rows.lock() = self.plugins.discover().into_iter().map(PluginRow::from).collect();
    }

    pub fn plugin_list(&self) -> Vec<PluginRow> {
        self.plugin_rows.lock().clone()
    }

    /// Runs a (possibly blocking) plugin operation off the UI thread, then refreshes the list.
    fn plugin_op(&self, op: impl FnOnce(&PluginHost) -> Result<(), String> + Send + 'static) {
        let (host, rows, client) = (self.plugins.clone(), self.plugin_rows.clone(), self.client.clone());
        std::thread::spawn(move || {
            if let Err(e) = op(&host) {
                client.emit(ferrocord_core::ClientEvent::ActionFailed { action: "plugin".into(), error: e });
            }
            *rows.lock() = host.discover().into_iter().map(PluginRow::from).collect();
            client.request_repaint();
        });
    }

    pub fn set_plugin_enabled(&self, id: &str, on: bool) {
        let id = id.to_string();
        self.plugin_op(move |h| {
            if on {
                h.enable(&id).map_err(|e| e.to_string())
            } else {
                h.disable(&id);
                Ok(())
            }
        });
    }

    pub fn reload_plugin(&self, id: &str) {
        let id = id.to_string();
        self.plugin_op(move |h| h.reload(&id).map_err(|e| e.to_string()));
    }

    pub fn grant(&self, id: &str, perm: &str, on: bool) {
        let (id, perm) = (id.to_string(), perm.to_string());
        self.plugin_op(move |h| if on { h.grant(&id, &perm) } else { h.revoke(&id, &perm) }.map_err(|e| e.to_string()));
    }

    // ------------------------------------------------------------ local API

    /// Starts, stops or restarts the API server to match the config.
    pub fn apply_api_config(&self) {
        let cfg = self.client.config().api;
        let mut slot = self.api.lock();
        let running_port = slot.as_ref().filter(|h| h.is_running()).map(|h| h.addr().port());
        if !cfg.enabled {
            if let Some(h) = slot.take() {
                h.stop();
            }
            *self.api_error.lock() = None;
            return;
        }
        if running_port == Some(cfg.port) {
            return;
        }
        if let Some(h) = slot.take() {
            h.stop();
        }
        match ApiServer::start(self.client.clone()) {
            Ok(h) => {
                tracing::info!(addr = %h.addr(), "local API listening");
                *slot = Some(h);
                *self.api_error.lock() = None;
            }
            Err(e) => *self.api_error.lock() = Some(format!("Could not start: {e}")),
        }
    }

    pub fn api_status(&self) -> Option<(String, String)> {
        self.api.lock().as_ref().filter(|h| h.is_running()).map(|h| (h.addr().to_string(), h.token()))
    }

    pub fn api_error(&self) -> Option<String> {
        self.api_error.lock().clone()
    }

    pub fn regenerate_api_token(&self) {
        if let Some(h) = self.api.lock().as_ref()
            && let Err(e) = h.regenerate_token()
        {
            *self.api_error.lock() = Some(e.to_string());
        }
    }
}
