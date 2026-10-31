//! Application shell: owns UI state, pumps client events and lays out the views.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Panel, RichText, vec2};
use ferrocord_core::model::Snowflake;
use ferrocord_core::{Client, ClientEvent, ConnectionState, Theme};
use tokio::sync::broadcast;

use crate::images::ImageCache;
use crate::login::LoginScreen;
use crate::markdown::Block;
use crate::theme::{self, Palette};
use crate::views;

pub enum Screen {
    Connecting,
    Login(Box<LoginScreen>),
    Main,
}

#[derive(Default)]
pub struct Nav {
    /// `None` = home (DMs / friends).
    pub guild: Option<Snowflake>,
    pub channel: Option<Snowflake>,
    pub last_channel: HashMap<Option<Snowflake>, Snowflake>,
}

/// Per-channel composer state.
#[derive(Default)]
pub struct Composer {
    pub text: String,
    pub reply_to: Option<Snowflake>,
    pub reply_mention: bool,
    pub editing: Option<Snowflake>,
    pub files: Vec<std::path::PathBuf>,
    /// Display text → raw token replacements applied on send (`@name` → `<@id>`).
    pub replacements: Vec<(String, String)>,
    pub autocomplete_index: usize,
}

/// Per-channel scroll bookkeeping for the virtualized message list.
#[derive(Default)]
pub struct ChatScroll {
    pub heights: HashMap<Snowflake, f32>,
    pub anchor: Option<(Snowflake, f32)>,
    pub pending_offset: Option<f32>,
    pub at_bottom: bool,
    /// Last-read marker captured when the channel was opened ("NEW" divider).
    pub unread_marker: Option<Snowflake>,
    /// Jump to the newest message on the next frame (e.g. after sending).
    pub scroll_to_bottom: bool,
    /// Rows actually laid out last frame (diagnostics and tests).
    pub rendered_rows: usize,
}

pub enum Popup {
    Profile { user: Snowflake, guild: Option<Snowflake>, at: egui::Pos2 },
    EmojiPicker { target: EmojiTarget, at: egui::Pos2 },
}

#[derive(Clone, Copy)]
pub enum EmojiTarget {
    Composer(Snowflake),
    Reaction(Snowflake, Snowflake),
}

pub enum Modal {
    Settings,
    QuickSwitcher { query: String, index: usize },
    ConfirmDelete { channel: Snowflake, message: Snowflake },
    ImageViewer { url: String, open_url: String },
    Pins { channel: Snowflake, messages: Option<Vec<ferrocord_core::model::Message>> },
    JoinServer { code: String, status: Option<String> },
    CustomStatus { text: String },
    IncomingCall { channel: Snowflake },
    Search { query: String, results: Option<(u64, Vec<ferrocord_core::model::Message>)>, searching: bool },
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

pub struct App {
    pub client: Client,
    pub images: ImageCache,
    pub pal: Palette,
    applied_theme: Option<(Theme, String, f32)>,
    pub screen: Screen,
    events: broadcast::Receiver<ClientEvent>,
    pub nav: Nav,
    pub composers: HashMap<Snowflake, Composer>,
    pub scroll: HashMap<Snowflake, ChatScroll>,
    pub md_cache: HashMap<Snowflake, (u64, Arc<Vec<Block>>)>,
    pub spoilers: HashMap<Snowflake, HashSet<usize>>,
    pub popup: Option<Popup>,
    pub modal: Option<Modal>,
    pub toasts: Vec<Toast>,
    pub collapsed: HashSet<Snowflake>,
    pub open_folders: HashSet<i64>,
    pub friends_tab: views::friends::FriendsTab,
    pub add_friend: String,
    pub settings_tab: views::settings::Tab,
    pub emoji_query: String,
    pub ext: crate::integrations::Integrations,
    pub demo: bool,
    /// Cached audio device names (inputs, outputs) for settings.
    pub audio_devices: Option<(Vec<String>, Vec<String>)>,
    /// Cached cameras (index, name) for settings.
    pub cameras: Option<Vec<(u32, String)>>,
    window_focused: bool,
    last_title: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, client: Client, demo: bool, demo_view: Option<String>) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install_fonts(&ctx);
        egui_extras::install_image_loaders(&ctx);
        let repaint_ctx = ctx.clone();
        client.set_repaint(move || repaint_ctx.request_repaint());
        let images = ImageCache::new(ctx.clone(), client.http().raw_client().clone(), client.runtime().clone(), client.paths().media_cache());
        let events = client.subscribe();
        crate::sounds::install(&client);
        let cfg = client.config();
        let pal = Palette::for_theme(cfg.appearance.theme, theme::parse_hex(&cfg.appearance.accent).unwrap_or(theme::hex(0x5865f2)));

        let screen = if demo {
            crate::demo::populate(&client);
            Screen::Main
        } else if client.try_resume_saved_session() {
            Screen::Connecting
        } else {
            Screen::Login(Box::default())
        };
        let ext = crate::integrations::Integrations::start(&client, demo);
        let mut app = Self {
            client,
            images,
            pal,
            applied_theme: None,
            screen,
            events,
            nav: Nav::default(),
            composers: HashMap::new(),
            scroll: HashMap::new(),
            md_cache: HashMap::new(),
            spoilers: HashMap::new(),
            popup: None,
            modal: None,
            toasts: vec![],
            collapsed: HashSet::new(),
            open_folders: HashSet::new(),
            friends_tab: Default::default(),
            add_friend: String::new(),
            settings_tab: Default::default(),
            emoji_query: String::new(),
            ext,
            demo,
            audio_devices: None,
            cameras: None,
            window_focused: true,
            last_title: String::new(),
        };
        if demo {
            let first = app.client.store().guild_order.first().copied();
            if let Some(g) = first {
                app.select_guild(Some(g));
            }
            match demo_view.as_deref() {
                Some("settings") => {
                    app.settings_tab = views::settings::Tab::Appearance;
                    app.modal = Some(Modal::Settings);
                }
                Some("emoji") => {
                    if let Some(ch) = app.nav.channel {
                        app.popup = Some(Popup::EmojiPicker { target: EmojiTarget::Composer(ch), at: egui::pos2(1000.0, 700.0) });
                    }
                }
                Some("friends") => app.select_guild(None),
                Some("dm") => {
                    let dm = app.client.store().private_channels.iter().next().copied();
                    if let Some(c) = dm {
                        app.select_channel(c);
                    }
                }
                Some("quick") => app.modal = Some(Modal::QuickSwitcher { query: String::new(), index: 0 }),
                Some("call") => {
                    let voice = app.client.store().channels.values().find(|c| c.kind.is_voice()).map(|c| c.id);
                    if let Some(v) = voice {
                        app.select_channel(v);
                    }
                }
                Some("voice-settings") => {
                    app.settings_tab = views::settings::Tab::Voice;
                    app.modal = Some(Modal::Settings);
                }
                _ => {}
            }
        }
        app
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, at: Instant::now() });
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let cfg = self.client.config();
        let key = (cfg.appearance.theme, cfg.appearance.accent.clone(), cfg.appearance.zoom);
        if self.applied_theme.as_ref() == Some(&key) {
            return;
        }
        self.pal = Palette::for_theme(key.0, theme::parse_hex(&key.1).unwrap_or(theme::hex(0x5865f2)));
        theme::apply(ctx, &self.pal, key.0);
        ctx.set_zoom_factor(key.2.clamp(0.6, 2.0));
        self.images.set_animate(cfg.appearance.animate_emoji);
        self.applied_theme = Some(key);
    }

    fn pump_events(&mut self, ctx: &egui::Context) {
        loop {
            match self.events.try_recv() {
                Ok(ev) => self.on_event(ctx, ev),
                Err(broadcast::error::TryRecvError::Lagged(n)) => tracing::warn!(n, "UI lagged behind client events"),
                Err(_) => break,
            }
        }
        if let Screen::Login(l) = &mut self.screen
            && let Some(token) = l.token_ready.take()
        {
            self.client.start_session(token);
            self.screen = Screen::Connecting;
        }
    }

    fn on_event(&mut self, _ctx: &egui::Context, ev: ClientEvent) {
        match ev {
            ClientEvent::Dispatch { name, .. } if name == "READY" => {
                self.screen = Screen::Main;
                if self.nav.channel.is_none() {
                    self.select_guild(None);
                }
            }
            ClientEvent::LoggedOut { reason } => {
                self.nav = Nav::default();
                self.composers.clear();
                self.scroll.clear();
                self.md_cache.clear();
                self.screen = Screen::Login(Box::new(LoginScreen::with_error(reason)));
            }
            ClientEvent::ActionFailed { action, error } => {
                self.toast(format!("{}: {error}", action.replace('_', " ")), true);
            }
            ClientEvent::Notification(n) => {
                let cfg = self.client.config().notifications;
                // Silent OS notification: the message/mention sound comes from `sounds`.
                if cfg.desktop {
                    crate::notify::show(&n, cfg.show_content, false);
                }
            }
            ClientEvent::IncomingCall { channel_id } => {
                self.modal = Some(Modal::IncomingCall { channel: channel_id });
                crate::notify::show(
                    &ferrocord_core::Notification {
                        title: "Incoming call".into(),
                        body: self.client.store().channels.get(&channel_id).map(|c| c.display_name()).unwrap_or_default(),
                        channel_id,
                        guild_id: None,
                        message_id: Snowflake(0),
                        icon_url: None,
                    },
                    true,
                    false, // `sounds` rings instead
                );
            }
            ClientEvent::CallEnded { channel_id } => {
                if matches!(self.modal, Some(Modal::IncomingCall { channel }) if channel == channel_id) {
                    self.modal = None;
                }
            }
            ClientEvent::Connection(ConnectionState::Connected) if matches!(self.screen, Screen::Connecting) => {}
            _ => {}
        }
    }

    // ------------------------------------------------------------ navigation

    pub fn select_guild(&mut self, guild: Option<Snowflake>) {
        self.nav.guild = guild;
        let remembered = self.nav.last_channel.get(&guild).copied();
        let target = remembered.or_else(|| {
            let s = self.client.store();
            match guild {
                Some(g) => s.guild_channel_tree(g).into_iter().flat_map(|(_, ch)| ch).find(|c| c.kind.is_text_based() && !c.kind.is_voice()).map(|c| c.id),
                None => None,
            }
        });
        match target {
            Some(c) => self.select_channel(c),
            None => {
                self.nav.channel = None;
                self.client.set_focus(None, self.window_focused);
            }
        }
    }

    pub fn select_channel(&mut self, channel: Snowflake) {
        let (guild, kind) = {
            let s = self.client.store();
            let c = s.channels.get(&channel);
            (c.and_then(|c| c.guild_id), c.map(|c| c.kind).unwrap_or_default())
        };
        if kind.is_voice() {
            if self.client.voice_channel() != Some(channel) && !self.demo {
                self.client.join_voice(channel);
            }
            self.nav.guild = guild;
            self.nav.channel = Some(channel);
            self.nav.last_channel.insert(guild, channel);
            return;
        }
        if matches!(kind, ferrocord_core::model::ChannelType::GuildForum | ferrocord_core::model::ChannelType::GuildMedia) {
            self.nav.guild = guild;
            self.nav.channel = Some(channel);
            self.nav.last_channel.insert(guild, channel);
            if !self.demo {
                self.client.spawn_action("load_forum", move |c| async move {
                    let threads = c.http().forum_threads(channel, 0).await?;
                    {
                        let mut s = c.store_mut();
                        for mut t in threads {
                            t.guild_id = t.guild_id.or(guild);
                            s.upsert_channel(t);
                        }
                    }
                    c.request_repaint();
                    Ok(())
                });
            }
            return;
        }
        self.nav.guild = guild;
        self.nav.channel = Some(channel);
        self.nav.last_channel.insert(guild, channel);
        let marker = {
            let s = self.client.store();
            let read = s.read_states.get(&channel).and_then(|r| r.last_message_id);
            let last = s.channels.get(&channel).and_then(|c| c.last_message_id);
            match (read, last) {
                (Some(r), Some(l)) if l > r => Some(r),
                _ => None,
            }
        };
        let sc = self.scroll.entry(channel).or_default();
        sc.unread_marker = marker;
        sc.at_bottom = true;
        sc.anchor = None;
        self.popup = None;
        if !self.demo {
            self.client.open_channel(channel);
        }
        self.client.set_focus(Some(channel), self.window_focused);
    }

    /// Turns the camera on or off in the current call.
    pub fn toggle_camera(&mut self) {
        if let Err(e) = self.ext.toggle_camera(&self.client) {
            self.toast(e, true);
        }
    }

    pub fn open_dm_with(&mut self, user: Snowflake) {
        let client = self.client.clone();
        let tx = self.ext.nav_tx.clone();
        let ctx_repaint = self.client.clone();
        self.client.runtime().spawn(async move {
            match client.open_dm(user).await {
                Ok(ch) => {
                    let _ = tx.send(ch);
                    ctx_repaint.request_repaint();
                }
                Err(e) => client.emit(ClientEvent::ActionFailed { action: "open_dm".into(), error: e.to_string() }),
            }
        });
    }

    /// Parsed markdown for a message, cached by content hash.
    pub fn parsed(&mut self, id: Snowflake, content: &str) -> Arc<Vec<Block>> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut h);
        let hash = h.finish();
        if let Some((hh, b)) = self.md_cache.get(&id)
            && *hh == hash
        {
            return b.clone();
        }
        let blocks = Arc::new(crate::markdown::parse(&crate::markdown::strip_suppressed_links(content)));
        if self.md_cache.len() > 5000 {
            self.md_cache.clear();
        }
        self.md_cache.insert(id, (hash, blocks.clone()));
        blocks
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let n = self.client.store().badge_count();
        let title = if n > 0 { format!("({n}) Ferrocord") } else { "Ferrocord".to_string() };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn global_shortcuts(&mut self, ctx: &egui::Context) {
        let (k, esc, alt_up, alt_down) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::K),
                i.key_pressed(egui::Key::Escape),
                i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown),
            )
        });
        if k {
            self.modal = Some(Modal::QuickSwitcher { query: String::new(), index: 0 });
        }
        if esc {
            if self.popup.is_some() {
                self.popup = None;
            } else if self.modal.is_some() {
                self.modal = None;
            } else if let Some(ch) = self.nav.channel {
                // Esc in a channel marks it read, like the official client.
                if !self.demo {
                    self.client.ack_channel(ch);
                }
                if let Some(c) = self.composers.get_mut(&ch) {
                    c.reply_to = None;
                    c.editing = None;
                }
            }
        }
        if alt_up || alt_down {
            self.cycle_channel(if alt_up { -1 } else { 1 });
        }
    }

    fn cycle_channel(&mut self, dir: i32) {
        let ids: Vec<Snowflake> = {
            let s = self.client.store();
            match self.nav.guild {
                Some(g) => s.guild_channel_tree(g).into_iter().flat_map(|(_, c)| c).filter(|c| !c.kind.is_voice()).map(|c| c.id).collect(),
                None => s.sorted_private_channels().iter().map(|c| c.id).collect(),
            }
        };
        if ids.is_empty() {
            return;
        }
        let cur = self.nav.channel.and_then(|c| ids.iter().position(|x| *x == c)).unwrap_or(0) as i32;
        let next = (cur + dir).rem_euclid(ids.len() as i32) as usize;
        self.select_channel(ids[next]);
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(6));
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("toasts")).anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -90.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
            for t in &self.toasts {
                egui::Frame::new()
                    .fill(if t.error { self.pal.danger } else { self.pal.bg_elevated })
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(&t.text).color(Color32::WHITE));
                    });
                ui.add_space(4.0);
            }
        });
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        while let Ok(ch) = self.ext.nav_rx.try_recv() {
            self.select_channel(ch);
        }
        let show_members = self.client.config().appearance.show_member_list && self.nav.guild.is_some() && self.nav.channel.is_some();

        Panel::left("rail").exact_size(72.0).resizable(false).show_separator_line(false).frame(egui::Frame::new().fill(self.pal.bg_rail)).show(ui, |ui| {
            views::guilds::rail(self, ui);
        });
        Panel::left("sidebar").exact_size(248.0).resizable(false).show_separator_line(false).frame(egui::Frame::new().fill(self.pal.bg_side)).show(ui, |ui| {
            views::channels::sidebar(self, ui);
        });
        if show_members {
            Panel::right("members").exact_size(244.0).resizable(false).show_separator_line(false).frame(egui::Frame::new().fill(self.pal.bg_side)).show(
                ui,
                |ui| {
                    views::members::panel(self, ui);
                },
            );
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            ui.painter().rect_filled(ui.max_rect(), CornerRadius::ZERO, self.pal.bg_main);
            let is_voice = self.nav.channel.and_then(|c| self.client.store().channels.get(&c).map(|c| c.kind.is_voice())).unwrap_or(false);
            let is_forum = self
                .nav
                .channel
                .and_then(|c| {
                    self.client
                        .store()
                        .channels
                        .get(&c)
                        .map(|c| matches!(c.kind, ferrocord_core::model::ChannelType::GuildForum | ferrocord_core::model::ChannelType::GuildMedia))
                })
                .unwrap_or(false);
            match self.nav.channel {
                Some(ch) if is_forum => views::forum::forum(self, ui, ch),
                Some(ch) if is_voice => views::voice::call_view(self, ui, ch),
                Some(ch) => views::chat::chat(self, ui, ch),
                None if self.nav.guild.is_none() => views::friends::page(self, ui),
                None => {
                    ui.centered_and_justified(|ui| ui.label(RichText::new("No text channels").color(self.pal.text_muted)));
                }
            }
        });
        views::popups::show(self, ui.ctx());
        views::modals::show(self, ui.ctx());
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused != self.window_focused {
            self.window_focused = focused;
            self.client.set_focus(self.nav.channel, focused);
        }
        self.pump_events(&ctx);
        self.images.maintain();
        let pal = self.pal;
        match &mut self.screen {
            Screen::Login(login) => {
                login.ui(ui, &self.client, &self.images, &pal);
            }
            Screen::Connecting => {
                ui.painter().rect_filled(ui.max_rect(), CornerRadius::ZERO, pal.bg_rail);
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 2.0 - 40.0);
                        ui.spinner();
                        let state = self.client.connection_state();
                        ui.label(RichText::new(format!("Connecting… ({state:?})")).color(pal.text_muted));
                        if ui.link("Cancel and log out").clicked() {
                            self.client.logout();
                        }
                    });
                });
            }
            Screen::Main => {
                self.global_shortcuts(&ctx);
                self.main_ui(ui);
                self.update_title(&ctx);
            }
        }
        self.toasts_ui(&ctx);
        crate::widgets::pointer_feedback(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.ext.stop();
        self.client.shutdown();
    }
}
