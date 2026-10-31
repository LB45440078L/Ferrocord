//! Headless UI tests driven by `egui_kittest`.
//!
//! The harness runs the real [`App`] in demo mode (no network, fixture data)
//! and lets tests type, click and inspect state frame by frame.

mod chat;
mod layout;

use egui_kittest::Harness;
use ferrocord_core::{Client, ClientOptions, Paths};

use crate::app::App;

/// Keeps the runtime and temp dirs alive for the harness' lifetime.
pub struct Fixture {
    pub harness: Harness<'static, App>,
    _rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
}

pub fn demo_app(view: Option<&str>) -> Fixture {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::under(dir.path());
    let client = Client::with_options(rt.handle().clone(), paths, ClientOptions::offline());
    let view = view.map(str::to_string);
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(move |cc| App::new(cc, client, true, view));
    harness.run_steps(4);
    Fixture { harness, _rt: rt, _dir: dir }
}
