//! Ferrocord desktop client.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod captcha;
mod demo;
mod images;
mod integrations;
mod login;
mod markdown;
mod notify;
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
            "Ferrocord {}\n\nUSAGE:\n  ferrocord [--demo]\n\nOPTIONS:\n  --demo               Explore the UI with offline sample data (no login)
  --demo-view <VIEW>   Demo mode opened on a view: settings, emoji, friends, quick, dm
  -V, --version        Print version and exit\n\nENVIRONMENT:\n  RUST_LOG  Log filter, e.g. RUST_LOG=ferrocord=debug",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    let demo = args.iter().any(|a| a == "--demo" || a == "--demo-view");
    let demo_view = args.iter().position(|a| a == "--demo-view").and_then(|i| args.get(i + 1)).cloned();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,ferrocord=info,ferrocord_core=info,ferrocord_gateway=info")))
        .with_target(false)
        .init();

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).thread_name("ferrocord-rt").enable_all().build()?;
    let paths = Paths::discover();
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
        ..Default::default()
    };
    let result = eframe::run_native("Ferrocord", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, client, demo, demo_view)))));
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result.map_err(|e| anyhow::anyhow!("{e}"))
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
