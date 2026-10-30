//! Login screen: email/phone + password, QR code, token, MFA and captcha.

use std::sync::mpsc;

use egui::{Align, Color32, CornerRadius, Layout, RichText, Stroke, TextEdit, vec2};
use ferrocord_core::Client;
use ferrocord_core::auth::login::{self, LoginOutcome, MfaMethod, MfaStep};
use ferrocord_core::auth::remote::{self, RemoteAuthEvent, ScannedUser};

use crate::images::ImageCache;
use crate::theme::{self, Palette};
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Password,
    Qr,
    Token,
}

enum Msg {
    Login(Result<LoginOutcome, String>),
    Mfa(Result<String, String>),
    Sms(Result<String, String>),
    Remote(RemoteAuthEvent),
    Token(Result<String, String>),
}

enum Step {
    Credentials,
    Mfa { step: MfaStep, method: MfaMethod, sms_to: Option<String> },
}

enum Qr {
    Idle,
    Waiting,
    Code(String),
    Scanned(ScannedUser),
}

pub struct LoginScreen {
    tab: Tab,
    login: String,
    password: String,
    token: String,
    code: String,
    error: Option<String>,
    info: Option<String>,
    busy: bool,
    step: Step,
    qr: Qr,
    qr_task: Option<tokio::task::JoinHandle<()>>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    /// Set once a token is obtained; the app takes it and starts the session.
    pub token_ready: Option<String>,
}

impl Default for LoginScreen {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tab: Tab::Password,
            login: String::new(),
            password: String::new(),
            token: String::new(),
            code: String::new(),
            error: None,
            info: None,
            busy: false,
            step: Step::Credentials,
            qr: Qr::Idle,
            qr_task: None,
            tx,
            rx,
            token_ready: None,
        }
    }
}

impl Drop for LoginScreen {
    fn drop(&mut self) {
        if let Some(t) = self.qr_task.take() {
            t.abort();
        }
    }
}

impl LoginScreen {
    pub fn with_error(reason: String) -> Self {
        let mut s = Self::default();
        s.error = Some(reason);
        s
    }

    fn spawn<F>(&self, client: &Client, ctx: &egui::Context, fut: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        client.runtime().spawn(async move {
            let m = fut.await;
            let _ = tx.send(m);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, client: &Client) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Login(r) => {
                    self.busy = false;
                    match r {
                        Ok(LoginOutcome::Success { token, .. }) => self.token_ready = Some(token),
                        Ok(LoginOutcome::Mfa(step)) => {
                            let method = step.methods().into_iter().find(|m| *m != MfaMethod::Webauthn).unwrap_or(MfaMethod::Totp);
                            self.code.clear();
                            self.error = None;
                            self.step = Step::Mfa { step, method, sms_to: None };
                        }
                        Ok(LoginOutcome::VerificationRequired { message }) => self.info = Some(message),
                        Ok(LoginOutcome::Suspended { .. }) => self.error = Some("This account has been suspended.".into()),
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Mfa(r) => {
                    self.busy = false;
                    match r {
                        Ok(token) => self.token_ready = Some(token),
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Sms(r) => {
                    self.busy = false;
                    match r {
                        Ok(phone) => {
                            if let Step::Mfa { sms_to, .. } = &mut self.step {
                                *sms_to = Some(phone);
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Token(r) => {
                    self.busy = false;
                    match r {
                        Ok(t) => self.token_ready = Some(t),
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Remote(ev) => match ev {
                    RemoteAuthEvent::QrCode(url) => self.qr = Qr::Code(url),
                    RemoteAuthEvent::Scanned(u) => self.qr = Qr::Scanned(u),
                    RemoteAuthEvent::Token(t) => self.token_ready = Some(t),
                    RemoteAuthEvent::Cancelled => {
                        self.qr = Qr::Waiting;
                        self.info = Some("Login was cancelled on your phone.".into());
                    }
                    RemoteAuthEvent::Error(e) => {
                        self.qr = Qr::Waiting;
                        tracing::warn!(error = %e, "QR login error");
                    }
                },
            }
        }
        let _ = client;
    }

    fn ensure_qr(&mut self, client: &Client, ctx: &egui::Context) {
        if self.qr_task.is_some() {
            return;
        }
        self.qr = Qr::Waiting;
        let http = client.http().clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let _g = client.runtime().enter();
        self.qr_task = Some(client.runtime().spawn(async move {
            let mut rx = remote::start(http);
            while let Some(ev) = rx.recv().await {
                let done = matches!(ev, RemoteAuthEvent::Token(_));
                let _ = tx.send(Msg::Remote(ev));
                ctx.request_repaint();
                if done {
                    break;
                }
            }
        }));
    }

    fn stop_qr(&mut self) {
        if let Some(t) = self.qr_task.take() {
            t.abort();
        }
        self.qr = Qr::Idle;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, client: &Client, images: &ImageCache, pal: &Palette) {
        self.poll(client);
        let ctx = ui.ctx().clone();
        if self.tab == Tab::Qr {
            self.ensure_qr(client, &ctx);
        } else if self.qr_task.is_some() {
            self.stop_qr();
        }

        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, CornerRadius::ZERO, pal.bg_rail);
        // Soft accent glow behind the card.
        ui.painter().circle_filled(rect.center() - vec2(220.0, 160.0), 260.0, pal.accent.gamma_multiply(0.06));
        ui.painter().circle_filled(rect.center() + vec2(260.0, 180.0), 220.0, pal.accent.gamma_multiply(0.05));

        let card_w = 440.0_f32.min(rect.width() - 32.0);
        let card = egui::Rect::from_center_size(rect.center(), vec2(card_w, 10.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(egui::pos2(card.min.x, rect.min.y), vec2(card_w, rect.height()))).layout(Layout::top_down(Align::Center)));
        let ui = &mut child;
        ui.add_space((rect.height() * 0.12).max(24.0));
        egui::Frame::new()
            .fill(pal.bg_side)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(egui::Margin::same(28))
            .show(ui, |ui| {
                ui.set_width(card_w - 56.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Ferrocord").font(egui::FontId::new(28.0, theme::bold())).color(pal.text));
                    ui.label(RichText::new("Sign in with your Discord account").color(pal.text_muted));
                });
                ui.add_space(16.0);

                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                if let Step::Mfa { .. } = self.step {
                    self.mfa_ui(ui, client, &ctx, pal);
                } else {
                    ui.horizontal(|ui| {
                        for (tab, label) in [(Tab::Password, "Password"), (Tab::Qr, "QR Code"), (Tab::Token, "Token")] {
                            let sel = self.tab == tab;
                            let b = egui::Button::new(RichText::new(label).color(if sel { Color32::WHITE } else { pal.text_muted }))
                                .fill(if sel { pal.accent } else { pal.bg_elevated })
                                .corner_radius(CornerRadius::same(6))
                                .min_size(vec2((ui.available_width() - 16.0) / 3.0, 32.0));
                            if ui.add(b).clicked() {
                                self.tab = tab;
                                self.error = None;
                                self.info = None;
                            }
                        }
                    });
                    ui.add_space(14.0);
                    match self.tab {
                        Tab::Password => self.password_ui(ui, client, &ctx, pal),
                        Tab::Qr => self.qr_ui(ui, images, pal),
                        Tab::Token => self.token_ui(ui, client, &ctx, pal),
                    }
                }

                if let Some(e) = &self.error {
                    ui.add_space(10.0);
                    ui.label(RichText::new(e).color(pal.danger));
                }
                if let Some(i) = &self.info {
                    ui.add_space(10.0);
                    ui.label(RichText::new(i).color(pal.warning));
                }
                });
            });
        ui.add_space(12.0);
        ui.label(
            RichText::new("Ferrocord is an unofficial client and is not affiliated with Discord Inc.\nUsing third-party clients may violate Discord's Terms of Service.")
                .small()
                .color(pal.text_faint),
        );
    }

    fn field(ui: &mut egui::Ui, label: &str, value: &mut String, password: bool, pal: &Palette) -> egui::Response {
        widgets::section_label(ui, label, pal);
        ui.add(TextEdit::singleline(value).password(password).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)))
    }

    fn password_ui(&mut self, ui: &mut egui::Ui, client: &Client, ctx: &egui::Context, pal: &Palette) {
        let r1 = Self::field(ui, "Email or phone number", &mut self.login, false, pal);
        ui.add_space(6.0);
        let r2 = Self::field(ui, "Password", &mut self.password, true, pal);
        ui.add_space(14.0);
        let enter = (r1.lost_focus() || r2.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let can = !self.busy && !self.login.trim().is_empty() && !self.password.is_empty();
        let clicked = ui.add_enabled_ui(can, |ui| {
            ui.add_sized(vec2(ui.available_width(), 38.0), egui::Button::new(RichText::new(if self.busy { "Logging in…" } else { "Log In" }).color(Color32::WHITE).strong()).fill(pal.accent))
        });
        if can && (clicked.inner.clicked() || enter) {
            self.busy = true;
            self.error = None;
            self.info = None;
            let http = client.http().clone();
            let (l, p) = (self.login.trim().to_string(), self.password.clone());
            self.spawn(client, ctx, async move { Msg::Login(login::login(&http, &l, &p).await.map_err(|e| e.to_string())) });
        }
        ui.add_space(6.0);
        ui.label(RichText::new("If Discord asks for a captcha, a verification window will open.").small().color(pal.text_faint));
    }

    fn mfa_ui(&mut self, ui: &mut egui::Ui, client: &Client, ctx: &egui::Context, pal: &Palette) {
        let Step::Mfa { step, method, sms_to } = &mut self.step else { return };
        ui.label(RichText::new("Two-factor authentication").font(egui::FontId::new(18.0, theme::bold())));
        ui.add_space(6.0);
        let methods: Vec<MfaMethod> = step.methods().into_iter().filter(|m| *m != MfaMethod::Webauthn).collect();
        if methods.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for m in &methods {
                    if ui.selectable_label(*method == *m, m.label()).clicked() {
                        *method = *m;
                    }
                }
            });
        }
        if step.webauthn.is_some() && methods.is_empty() {
            ui.label(RichText::new("This account only has a security key configured. Security keys aren't supported yet; use QR login instead.").color(pal.warning));
            return;
        }
        let hint = match method {
            MfaMethod::Totp => "6-digit code from your authenticator app",
            MfaMethod::Sms => "Code sent by SMS",
            MfaMethod::Backup => "8-character backup code",
            MfaMethod::Webauthn => "",
        };
        ui.label(RichText::new(hint).color(pal.text_muted));
        if *method == MfaMethod::Sms {
            match sms_to {
                Some(p) => {
                    ui.label(RichText::new(format!("Code sent to {p}")).small().color(pal.success));
                }
                None => {
                    if ui.button("Send SMS code").clicked() && !self.busy {
                        self.busy = true;
                        let http = client.http().clone();
                        let s = step.clone();
                        let tx = self.tx.clone();
                        let c = ctx.clone();
                        client.runtime().spawn(async move {
                            let _ = tx.send(Msg::Sms(login::send_sms(&http, &s).await.map_err(|e| e.to_string())));
                            c.request_repaint();
                        });
                    }
                }
            }
        }
        let r = ui.add(TextEdit::singleline(&mut self.code).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).hint_text("Code"));
        r.request_focus();
        ui.add_space(10.0);
        let submit = ui.add_sized(vec2(ui.available_width(), 38.0), egui::Button::new(RichText::new("Verify").color(Color32::WHITE).strong()).fill(pal.accent)).clicked()
            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if submit && !self.busy && !self.code.trim().is_empty() {
            self.busy = true;
            self.error = None;
            let http = client.http().clone();
            let (s, m, code) = (step.clone(), *method, self.code.clone());
            let tx = self.tx.clone();
            let c = ctx.clone();
            client.runtime().spawn(async move {
                let _ = tx.send(Msg::Mfa(login::verify_mfa(&http, &s, m, &code).await.map_err(|e| e.to_string())));
                c.request_repaint();
            });
        }
        if ui.link("← Back").clicked() {
            self.step = Step::Credentials;
            self.error = None;
        }
    }

    fn qr_ui(&mut self, ui: &mut egui::Ui, images: &ImageCache, pal: &Palette) {
        ui.vertical_centered(|ui| match &self.qr {
            Qr::Idle | Qr::Waiting => {
                ui.add_space(60.0);
                ui.spinner();
                ui.label(RichText::new("Generating QR code…").color(pal.text_muted));
                ui.add_space(60.0);
            }
            Qr::Code(url) => {
                let key = format!("qr:{url}");
                if images.get(&key).is_none() {
                    images.insert_local(&key, qr_image(url));
                }
                if let Some(img) = images.get(&key) {
                    egui::Frame::new().fill(Color32::WHITE).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                        ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(img.texture, vec2(200.0, 200.0))));
                    });
                }
                ui.add_space(10.0);
                ui.label(RichText::new("Log in with QR code").font(egui::FontId::new(17.0, theme::bold())));
                ui.label(RichText::new("Scan this with the Discord mobile app to log in instantly.").color(pal.text_muted));
            }
            Qr::Scanned(u) => {
                ui.add_space(20.0);
                if let Some(url) = u.avatar_url() {
                    widgets::avatar(ui, images, &url, 80.0, None, pal);
                }
                ui.label(RichText::new("Check your phone!").font(egui::FontId::new(18.0, theme::bold())));
                ui.label(RichText::new(format!("Logging in as {}", u.username)).color(pal.text_muted));
                ui.add_space(30.0);
            }
        });
    }

    fn token_ui(&mut self, ui: &mut egui::Ui, client: &Client, ctx: &egui::Context, pal: &Palette) {
        let r = Self::field(ui, "Token", &mut self.token, true, pal);
        ui.label(RichText::new("Never share your token. Anyone with it has full access to your account.").small().color(pal.warning));
        ui.add_space(10.0);
        let go = ui.add_sized(vec2(ui.available_width(), 38.0), egui::Button::new(RichText::new("Log In").color(Color32::WHITE).strong()).fill(pal.accent)).clicked()
            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if go && !self.busy && !self.token.trim().is_empty() {
            self.busy = true;
            self.error = None;
            let http = client.http().clone();
            let t = self.token.trim().trim_matches('"').to_string();
            self.spawn(client, ctx, async move {
                Msg::Token(login::validate_token(&http, &t).await.map(|_| t).map_err(|e| e.to_string()))
            });
        }
    }
}

/// Renders a QR code into a crisp ColorImage with a quiet zone.
fn qr_image(data: &str) -> egui::ColorImage {
    let code = match qrcode::QrCode::new(data.as_bytes()) {
        Ok(c) => c,
        Err(_) => return egui::ColorImage::new([1, 1], vec![Color32::WHITE]),
    };
    let w = code.width();
    let colors = code.to_colors();
    let scale = 8;
    let quiet = 0;
    let n = (w + quiet * 2) * scale;
    let mut px = vec![Color32::WHITE; n * n];
    for y in 0..w {
        for x in 0..w {
            if colors[y * w + x] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        px[((y + quiet) * scale + dy) * n + (x + quiet) * scale + dx] = Color32::BLACK;
                    }
                }
            }
        }
    }
    egui::ColorImage::new([n, n], px)
}
