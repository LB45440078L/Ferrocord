# Architecture

Ferrocord is a Cargo workspace of small crates with a strict dependency
direction. Lower layers know nothing about the ones above them, so every layer
can be reused on its own. A bot-like tool can use just `ferrocord-http`; a
different UI can use `ferrocord-core`.

```text
                 ┌──────────────────────────── crates/app (ferrocord) ───────────────────────────┐
                 │  egui views · login · captcha helper · image cache · markdown · notifications │
                 └───────────────┬─────────────────────────┬───────────────────────────┬─────────┘
                                 │                         │                           │
                     crates/plugin (Luau)        crates/api (axum)                     │
                                 │  implements Extension   │  subscribes to events     │
                                 └────────────┬────────────┘                           │
                                              ▼                                        ▼
                         ┌─────────────────────── crates/core ───────────────────────────┐
                         │ Client: actions · event broadcast · config · Extension trait   │
                         └───────┬───────────────┬───────────────┬───────────────┬────────┘
                                 ▼               ▼               ▼               ▼
                          crates/store    crates/gateway    crates/auth     crates/http
                         (state cache)   (websocket)      (login, QR,      (REST, rate limits,
                                 │               │          token vault)    captcha/MFA hooks)
                                 └───────────────┴───────┬───────┴───────────────┘
                                                         ▼
                                                  crates/model (wire types)
```

## Data flow

1. **Login** (`auth`) produces a token: by password + MFA, by QR remote auth, or
   by pasting one. The token goes to `Client::start_session`.
2. **Gateway** (`gateway`) runs on its own Tokio task. It owns the websocket,
   inflates zlib-stream frames, heartbeats, and identifies or resumes. Every
   dispatch is parsed into a typed `Event` by borrowing the JSON (no copy), and
   the raw `serde_json::Value` is kept in an `Arc` so plugins and API clients
   can receive it verbatim.
3. **Core pump** (`core::Client::pump`) takes each gateway message, works out
   whether it warrants a notification, applies it to the **store** under a
   write lock, and then broadcasts a `ClientEvent`:
   - registered `Extension`s (built-in commands, plugins) get it synchronously;
   - `tokio::broadcast` subscribers (the UI, API WebSocket clients) receive it;
   - the UI is woken via the repaint callback.
4. **UI** (`app`) is immediate-mode. Each frame reads the store under a short
   read lock and renders. User actions call `Client` methods that update the
   store optimistically (local echo for sends, read-state for acks) and perform
   REST calls on the runtime. Failures come back as `ClientEvent::ActionFailed`
   and are shown as toasts.

## Threading model

| Thread | Work |
|---|---|
| Main (UI) | eframe/egui event loop and rendering. It never blocks on I/O. |
| Tokio runtime (4 workers) | REST calls, the gateway task, the image downloader, remote auth, the API server |
| Tokio blocking pool | Image decoding, opening the browser |
| One thread per plugin | Each owns its Luau VM; it talks to the host through channels with timeouts |
| Captcha helper | A separate **process** (`ferrocord --captcha`) hosting the system WebView |

The store is a plain struct behind a `parking_lot::RwLock`. The UI only holds
the read lock for the duration of one view's data extraction. Views copy what
they need (usually a handful of visible messages) before drawing.

## Performance techniques

- **Virtualized lists.** The message list caches measured row heights and only
  lays out rows near the viewport. The member list uses fixed-height rows
  (`show_rows`). The scroll position is anchored when older history is
  prepended.
- **Single-galley markdown.** A paragraph is laid out once as one text galley.
  Emoji are em-space placeholders with images painted on top, and links,
  mentions and spoilers are hit-tested by character index. Parsed markdown is
  cached by message id + content hash.
- **Bounded caches.** Messages are capped per channel (600; older pages are
  fetched again on scroll), and textures are evicted when unused for 90 s.
  Images are requested pre-resized from Discord's media proxy and decoded off
  the UI thread, with at most 8 concurrent downloads.
- **Zero-copy events.** `Event::parse(&Value)` deserializes from a borrowed
  value. Raw payloads are shared through `Arc`.
- **Lazy guilds.** Member lists are requested in 100-row windows (gateway op
  37), exactly like the official client, instead of downloading whole guilds.

## Extension points

| Mechanism | For | Isolation |
|---|---|---|
| `ferrocord_core::Extension` (Rust trait) | Built-in features and embedding | In-process, trusted |
| Luau plugins (`ferrocord-plugin`) | Third-party scripts | Sandboxed VM, permission-gated API, CPU/memory/time limits, own thread |
| Local API (`ferrocord-api`) | External programs in any language | Separate process, token auth, loopback only, write actions opt-in |

The `Extension` trait:

```rust
pub trait Extension: Send + Sync {
    fn id(&self) -> &str;
    fn on_event(&self, client: &Client, event: &ClientEvent) {}
    fn on_outgoing_message(&self, client: &Client, msg: &mut OutgoingMessage) -> Flow { Flow::Continue }
    fn commands(&self) -> Vec<LocalCommand> { vec![] }
    fn run_command(&self, client: &Client, channel: Snowflake, name: &str, args: &str) -> bool { false }
    fn shutdown(&self) {}
}
```

## Error handling philosophy

- Network and protocol errors never panic the client. Unknown or malformed
  gateway events degrade to `Event::Other`, so one schema change can't kill a
  session.
- REST errors carry Discord's JSON error code and field errors
  (`ApiErrorBody::display_message`) so the UI can show useful text.
- Captcha (HTTP 400 with `captcha_key`) and MFA re-auth (HTTP 401, code 60003)
  are handled inside the HTTP client: the request is suspended, the user is
  asked, and the request is replayed with `X-Captcha-*` or
  `X-Discord-MFA-Authorization` headers.

## Testing

- Unit tests live next to the code: snowflakes, the settings protobuf, rate-limit
  route keys, zlib streaming, READY/event application in the store, member-list
  ops, markdown parsing and login response parsing.
- `crates/plugin/src/tests.rs` exercises the sandbox, permissions, timeouts and
  the example plugins.
- `crates/api/tests/server.rs` boots a real server and checks auth, the
  Host/Origin defenses, the endpoints and the WebSocket protocol.
- `cargo run -p ferrocord-auth --example qr_probe` performs a live remote-auth
  handshake with Discord, without credentials, to detect protocol drift.
- `ferrocord --demo` renders the full UI from fixture data for visual checks.
