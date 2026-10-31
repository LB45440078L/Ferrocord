//! Captcha solving.
//!
//! Discord's hCaptcha site key only validates on `discord.com`. We therefore run a
//! tiny helper process (`ferrocord --captcha`) hosting a system WebView that
//! navigates to `https://discord.com/robots.txt` — a same-origin page without a
//! Content-Security-Policy — and injects the hCaptcha widget there. The solved
//! token is printed to stdout and handed back to the REST client, which replays
//! the request with `X-Captcha-Key` exactly like the official client.
//!
//! Running the WebView out-of-process keeps the main UI free of browser engines
//! and avoids fighting over the windowing event loop.

use std::io::{BufRead, Write};
use std::process::Stdio;

use ferrocord_core::http::{BoxFuture, CaptchaChallenge, CaptchaSolution, CaptchaSolver};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

pub const HELPER_FLAG: &str = "--captcha";

/// [`CaptchaSolver`] that launches the helper process.
pub struct WebViewCaptcha {
    pub data_dir: std::path::PathBuf,
}

impl CaptchaSolver for WebViewCaptcha {
    fn solve(&self, challenge: CaptchaChallenge) -> BoxFuture<'static, Option<CaptchaSolution>> {
        let data_dir = self.data_dir.clone();
        Box::pin(async move {
            let exe = std::env::current_exe().ok()?;
            let mut child = tokio::process::Command::new(exe)
                .arg(HELPER_FLAG)
                .arg(&data_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| tracing::error!(error = %e, "failed to start captcha helper"))
                .ok()?;
            let mut stdin = child.stdin.take()?;
            stdin.write_all(serde_json::to_string(&challenge).ok()?.as_bytes()).await.ok()?;
            stdin.write_all(b"\n").await.ok()?;
            drop(stdin);
            let mut lines = tokio::io::BufReader::new(child.stdout.take()?).lines();
            let mut result = None;
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(json) = line.strip_prefix("CAPTCHA_RESULT ") {
                    let v: serde_json::Value = serde_json::from_str(json).ok()?;
                    if let Some(key) = v.get("key").and_then(|k| k.as_str()) {
                        result = Some(CaptchaSolution { key: key.to_string(), rqtoken: challenge.rqtoken.clone(), session_id: challenge.session_id.clone() });
                    }
                    break;
                }
            }
            let _ = child.wait().await;
            result
        })
    }
}

fn page_script(ch: &CaptchaChallenge) -> String {
    let cfg = serde_json::json!({
        "service": ch.service,
        "sitekey": ch.sitekey,
        "rqdata": ch.rqdata,
        "invisible": ch.invisible,
    });
    format!(
        r#"(function () {{
  if (location.hostname !== 'discord.com' || location.pathname !== '/robots.txt') return;
  const CFG = {cfg};
  const send = (o) => window.ipc.postMessage(JSON.stringify(o));
  function boot() {{
    document.documentElement.innerHTML = `
      <head><meta name="viewport" content="width=device-width">
      <style>
        html,body{{margin:0;height:100%;background:#1c1d22;color:#e6e7eb;font:15px system-ui,sans-serif}}
        body{{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:16px}}
        h1{{font-size:18px;font-weight:600;margin:0}} p{{margin:0;color:#a3a6b0;font-size:13px}}
      </style></head>
      <body><h1>Verify you're human</h1><p>Discord needs you to complete a captcha to continue.</p><div id="w"></div></body>`;
    const s = document.createElement('script');
    if (CFG.service === 'recaptcha') {{
      s.src = 'https://www.google.com/recaptcha/api.js?onload=__fcLoad&render=explicit';
    }} else {{
      s.src = 'https://js.hcaptcha.com/1/api.js?onload=__fcLoad&render=explicit&recaptchacompat=off';
    }}
    s.async = true;
    document.head.appendChild(s);
  }}
  window.__fcLoad = function () {{
    try {{
      if (CFG.service === 'recaptcha') {{
        grecaptcha.render('w', {{ sitekey: CFG.sitekey, theme: 'dark', callback: (t) => send({{ key: t }}) }});
        return;
      }}
      const id = hcaptcha.render('w', {{
        sitekey: CFG.sitekey,
        theme: 'dark',
        size: CFG.invisible ? 'invisible' : 'normal',
        callback: (t) => send({{ key: t }}),
        'error-callback': (e) => send({{ error: String(e) }}),
        'expired-callback': () => send({{ event: 'expired' }}),
      }});
      if (CFG.rqdata) hcaptcha.setData(id, {{ rqdata: CFG.rqdata }});
      if (CFG.invisible) hcaptcha.execute(id);
      send({{ event: 'rendered' }});
    }} catch (e) {{ send({{ error: String(e) }}); }}
  }};
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot); else boot();
}})();"#
    )
}

/// Entry point of the helper process. Never returns.
pub fn run_helper(data_dir: Option<String>) -> ! {
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;

    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    let challenge: CaptchaChallenge = match serde_json::from_str(&line) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("invalid captcha challenge: {e}");
            std::process::exit(2);
        }
    };

    let emit = |v: &str| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "CAPTCHA_RESULT {v}");
        let _ = out.flush();
    };

    #[derive(Debug)]
    enum UserEvent {
        Done(String),
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Ferrocord — Verification")
        .with_inner_size(LogicalSize::new(420.0, 640.0))
        .with_resizable(true)
        .build(&event_loop)
        .expect("failed to create captcha window");

    let mut web_ctx = wry::WebContext::new(data_dir.map(|d| std::path::PathBuf::from(d).join("webview")));
    let webview = wry::WebViewBuilder::new_with_web_context(&mut web_ctx)
        .with_initialization_script(page_script(&challenge))
        .with_ipc_handler(move |req| {
            let body = req.body().to_string();
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            if v.get("key").is_some() {
                let _ = proxy.send_event(UserEvent::Done(body));
            } else if let Some(e) = v.get("error") {
                eprintln!("captcha widget error: {e}");
            }
        })
        .with_url("https://discord.com/robots.txt")
        .with_devtools(cfg!(debug_assertions))
        .build(&window)
        .expect("failed to create webview (is WebView2 installed?)");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _keep = &webview;
        match event {
            Event::UserEvent(UserEvent::Done(json)) => {
                emit(&json);
                std::process::exit(0);
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                emit("{}");
                std::process::exit(1);
            }
            _ => {}
        }
    });
}
