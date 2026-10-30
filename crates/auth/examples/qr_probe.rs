//! Live check of the QR remote-auth handshake: prints the QR URL and exits.
//! No credentials are used. Run: `cargo run -p ferrocord-auth --example qr_probe`
use ferrocord_auth::remote::{self, RemoteAuthEvent};
use ferrocord_http::{ClientProfile, Http};

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let http = Http::new(ClientProfile::desktop());
    let mut rx = remote::start(http);
    while let Some(ev) = rx.recv().await {
        match ev {
            RemoteAuthEvent::QrCode(url) => {
                println!("QR OK: {url}");
                return;
            }
            other => println!("{other:?}"),
        }
    }
}
