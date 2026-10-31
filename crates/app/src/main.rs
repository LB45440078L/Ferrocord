//! Ferrocord desktop client.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod captcha;
mod demo;
mod icons;
mod images;
mod integrations;
mod login;
mod markdown;
mod notify;
mod probe;
mod theme;
mod views;
mod widgets;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use ferrocord_core::{Client, ClientOptions, Paths};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(captcha::HELPER_FLAG) {
        captcha::run_helper(args.get(2).cloned());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("ferrocord {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Ferrocord {}

USAGE:
  ferrocord [OPTIONS]

OPTIONS:
  --demo               Explore the UI with offline sample data (no login)
  --demo-view <VIEW>   Demo mode opened on a view: settings, emoji, friends, quick, dm, call, voice-settings
  --paths              Print the folders Ferrocord stores files in
  --remove-user-data   Sign out and delete all settings, plugins, caches and logs
  -V, --version        Print version and exit

ENVIRONMENT:
  RUST_LOG        Log filter, e.g. RUST_LOG=ferrocord=debug
  FERROCORD_HOME  Keep all files under this folder (portable mode)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--paths") {
        let p = Paths::discover();
        for (name, dir) in [("config", &p.config_dir), ("data", &p.data_dir), ("cache", &p.cache_dir), ("logs", &p.logs_dir)] {
            println!("{name:<7}{}", dir.display());
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--remove-user-data") {
        return remove_user_data(&Paths::discover());
    }
    let demo = args.iter().any(|a| a == "--demo" || a == "--demo-view");
    let demo_view = args.iter().position(|a| a == "--demo-view").and_then(|i| args.get(i + 1)).cloned();

    let paths = Paths::discover();
    init_logging(&paths);

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).thread_name("ferrocord-rt").enable_all().build()?;
    let options = if demo { ClientOptions::offline() } else { ClientOptions::default() };
    let client = Client::with_options(runtime.handle().clone(), paths.clone(), options);
    client.http().set_captcha_solver(Some(Arc::new(captcha::WebViewCaptcha { data_dir: paths.cache_dir.clone() })));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ferrocord")
            .with_app_id("dev.ferrocord.Ferrocord")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([860.0, 520.0])
            .with_icon(app_icon())
            .with_drag_and_drop(true),
        persist_window: true,
        persistence_path: Some(paths.ui_state_file()),
        ..Default::default()
    };
    let result = eframe::run_native("Ferrocord", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, client, demo, demo_view)))));
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result.map_err(|e| anyhow::anyhow!("{e}"))
}

/// Logs go to stderr and to `ferrocord.log` in the logs folder (release builds on
/// Windows have no console). The previous runs are kept as `ferrocord.1.log`
/// and `ferrocord.2.log`.
fn init_logging(paths: &Paths) {
    use tracing_subscriber::prelude::*;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,ferrocord=info,ferrocord_core=info,ferrocord_gateway=info,ferrocord_voice=info"));
    let file = std::fs::create_dir_all(&paths.logs_dir).ok().and_then(|_| {
        let log = paths.logs_dir.join("ferrocord.log");
        for i in (1..LOG_GENERATIONS).rev() {
            let from = if i == 1 { log.clone() } else { paths.logs_dir.join(format!("ferrocord.{}.log", i - 1)) };
            let _ = std::fs::rename(from, paths.logs_dir.join(format!("ferrocord.{i}.log")));
        }
        std::fs::File::create(log).ok()
    });
    let file_layer = file.map(|f| tracing_subscriber::fmt::layer().with_writer(std::sync::Mutex::new(f)).with_ansi(false).with_target(true));
    tracing_subscriber::registry().with(filter).with(tracing_subscriber::fmt::layer().with_target(false)).with(file_layer).init();
}

const LOG_GENERATIONS: usize = 3;

/// Deletes every folder Ferrocord writes to and the saved login of the last account.
fn remove_user_data(paths: &Paths) -> anyhow::Result<()> {
    let config = ferrocord_core::Config::load(paths);
    if let Some(account) = config.last_account {
        match ferrocord_core::auth::TokenStore::delete(&account) {
            Ok(()) => println!("removed saved login for account {account}"),
            Err(e) => eprintln!("could not remove saved login: {e}"),
        }
    }
    for dir in paths.all_dirs() {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => println!("removed {}", dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("could not remove {}: {e}", dir.display()),
        }
    }
    Ok(())
}

/// A simple generated window icon: accent squircle with a white "F".
fn app_icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 - 31.5, y as f32 - 31.5);
            // Superellipse |x|^4 + |y|^4 <= r^4.
            let inside = (fx.abs() / 31.0).powi(4) + (fy.abs() / 31.0).powi(4) <= 1.0;
            let glyph = (18..=26).contains(&x) && (14..=50).contains(&y)
                || (18..=46).contains(&x) && (14..=21).contains(&y)
                || (18..=40).contains(&x) && (29..=35).contains(&y);
            let i = (y * n + x) * 4;
            if inside {
                let (r, g, b) = if glyph { (255, 255, 255) } else { (0xe0, 0x6b, 0x2b) };
                rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}
