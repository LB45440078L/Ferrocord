# Testing

Ferrocord's tests never touch the real Discord or a real account. Protocol
crates are tested end to end against **`ferrocord-testkit`**, a scriptable mock
of Discord's REST API, gateway and remote-auth gateway on `127.0.0.1`.

## Test layers

| Layer | Where | What it covers |
|---|---|---|
| Unit tests | `#[cfg(test)]` modules next to the code | Snowflakes, settings protobuf, rate-limit route keys, zlib inflate, login response parsing, store event application, member-list ops, markdown, the plugin sandbox (`crates/plugin/src/tests.rs`) |
| Mock-server integration | `crates/{model,http,gateway,auth,store,core}/tests/` | Wire behaviour against `ferrocord-testkit` (see below) |
| Property tests | `crates/store/tests/props.rs` (`proptest`) | Member-list ops and message-cache invariants under random operation sequences |
| Local API | `crates/api/tests/server.rs` | A real API server: auth, Host/Origin defences, endpoints, WebSocket protocol |
| UI tests | `crates/app/src/tests/` (`egui_kittest`) | The real `App` in demo mode, driven frame by frame (typing, clicking, layout) |
| Live protocol probe | `cargo run -p ferrocord-auth --example qr_probe` | A credential-free remote-auth handshake with the real Discord, to detect protocol drift. Not part of `cargo test` |

What the integration suites check:

- **model** (`payloads.rs`): fixtures parse through `Event::parse` without errors, user-account READY quirks (`properties`, `merged_members`, versioned read states, settings proto) and nonce uniqueness.
- **http** (`mock_http.rs`): client identity headers (`X-Super-Properties`, `X-Fingerprint` before login), captcha solve-and-replay and cancel, MFA re-auth replay, 429 (per-route and global), bucket waits, 5xx retries, error bodies and `display_message`, multipart uploads, and route helpers (pagination, reaction URL encoding, friend-request username splitting, acks).
- **auth** (`mock_auth.rs`): password login, MFA (TOTP, backup, SMS, invalid code), invalid credentials, suspended accounts, new-location verification, captcha on login, token validation, and QR login (full flow, scanned-user parsing, cancel, expiry, fingerprint mismatch, heartbeats).
- **gateway** (`mock_gateway.rs`): the IDENTIFY payload, zlib-stream frames split across messages, heartbeats and latency, zombie detection → resume, `RECONNECT` with replay of missed events, `INVALID_SESSION`, close codes 4004/4009/4014, `auth_token` rotation used on resume, and command gating and op-37 batching.
- **store** (`props.rs`): property tests; see the table above.
- **core** (`mock_session.rs`): a full session (READY → store, paging, local echo resolved by nonce, acks, status via op 3 and settings proto, reactions, edit/delete/typing, logout), failed sends, revoked tokens, token rotation plus resume, notifications, and the `Extension` hooks and built-in commands.

## Running

```sh
cargo test --workspace                       # everything
cargo test -p ferrocord-gateway              # one crate
cargo test -p ferrocord-core --test mock_session full_session -- --nocapture
cargo test --workspace -- --ignored          # known-bug tests (expected to fail)
cargo clippy --workspace --all-targets -- -D warnings
```

Mock-server tests run in parallel and each one starts its own mock on a random
port, so they do not interfere with each other. Most finish in well under a
second. `invalid_session_not_resumable_reidentifies` takes 1–5 s because the
client waits as long as Discord asks it to. Proptest case counts are set per
test; set `PROPTEST_CASES` to run more.

Tests marked `#[ignore = "bug: …"]` document known bugs in code that is
currently being worked on. Remove the `ignore` together with the fix.

## The mock (`ferrocord-testkit`)

```rust
let mock = MockDiscord::start().await;              // 127.0.0.1:0, own thread + runtime
let http = Http::with_base_url(ClientProfile::desktop(), &mock.api_base());
let mut gw = GatewayConfig::new(mock.token(), ClientProfile::desktop());
gw.url = mock.gateway_url();
let qr = ferrocord_auth::remote::start_with_url(http.clone(), mock.remote_auth_url());
let client = Client::with_options(rt, paths, ClientOptions::custom_endpoints(mock.api_base(), mock.gateway_url()));
```

`ClientOptions::custom_endpoints` turns off `persist_credentials`, so tests never
write to the OS credential vault. Point `Paths` at a `tempfile::TempDir`.

Each REST request goes through these steps in order:

1. **Scripted routes** (`mock.on(method, pattern)`, newest first). A handler
   that returns `None` falls through to the next step.
2. **The world** (`mock.with_world(|w| …)`): one account (login, MFA, logout,
   `/users/@me`), channels and messages (send with nonce dedupe, paginated
   history with `before`/`after`/`around`, edit, delete, pins, reactions, acks,
   typing) and the settings proto. Mutations emit the matching gateway
   dispatches (`MESSAGE_CREATE` with the client's nonce, `MESSAGE_ACK`,
   `MESSAGE_REACTION_ADD`, …).
3. A fallback `404 {"code": 0}`.

Every REST request, gateway frame and remote-auth exchange is recorded:

```rust
let req = mock.wait_for_request("POST", "/channels/*/messages").await;
assert_eq!(req.json()["nonce"], nonce);
assert_eq!(req.super_properties().unwrap()["client_build_number"], 657850);
let identify = mock.gateway().wait_for_op(2).await;           // ClientFrame { op, d, .. }
let conns = mock.gateway().connections();                     // path, close codes, resumed…
let qr = mock.remote_auth().sessions();                       // key, fingerprint, nonce_verified…
```

Patterns: `*` and `{name}` match one segment and `**` matches the rest. Literals
match either the raw or the percent-decoded segment.

### Writing a new scenario

Canned responses:

```rust
mock.on("GET", "/users/@me").once().respond(MockResponse::error(500, 0, "oops"));
mock.on("GET", "/channels/*/messages/pins")
    .respond(MockResponse::ok(json!({"items": [], "has_more": false})).with_bucket("pins", 1, 0, 0.5));
mock.on("POST", "/auth/login").respond_seq(vec![first, second]);   // then falls through
mock.on("PATCH", "/guilds/*").handle(|req| {
    (req.json()["name"] == "bad").then(|| MockResponse::form_error("name", "BAD", "Nope"))
});
mock.delay_route("POST", "/channels/*/messages", Duration::from_millis(150)); // observe optimistic state
```

Ready-made gates (in `scenarios.rs`):

```rust
mock.require_captcha("POST", "/auth/login", CaptchaGate::default());   // 400 captcha until X-Captcha-* match
mock.require_mfa("DELETE", "/guilds/*/members/*", MfaGate::default()); // 401 60003 + /mfa/finish
mock.rate_limit("GET", "/users/@me", 0.1, /* global */ false, /* times */ 1);
```

World state:

```rust
mock.with_world(|w| {
    w.account.mfa = Some(MfaSetup::default());          // login now returns an MFA ticket
    w.account.suspended_token = Some("x".into());
    w.seed_messages(ids::GENERAL, 120, &UserBuilder::friend());
    w.insert_message(MessageBuilder::new(id, ch, &author).pinned().build());
});
```

`with_world` holds the world lock while it runs, so do not call other
`MockDiscord` methods inside the closure.

Gateway:

```rust
mock.set_ready(ReadyBuilder::standard().guild(GuildBuilder::new(7, "Another")));
mock.gateway().set_heartbeat_interval(40);          // ms, in HELLO
mock.gateway().drop_acks_on_next_connections(1);    // zombie connection
mock.gateway().rotate_token(Some("new-token"));     // READY.auth_token
mock.gateway().split_frames(Some(7));               // zlib message split into ~7-byte frames
mock.gateway().dispatch("TYPING_START", json!({...}));
mock.gateway().dispatch_buffered("MESSAGE_CREATE", m); // lost in flight; replayed on RESUME
mock.gateway().send_op(7, Value::Null);             // RECONNECT; op 9 = INVALID_SESSION
mock.gateway().close(4009, "Session timed out.");
mock.gateway().reject_resumes(true);
```

Compression matches Discord: one deflate stream per connection, each message
sync-flushed (`00 00 FF FF`). Split points never produce a prefix ending in that
marker. READY gets `session_id = mock-session-N` and
`resume_gateway_url = <mock>/resume`. Resume connections are recorded with path
`/resume/`.

Remote auth: `mock.remote_auth().script([RemoteAuthBehaviour::CancelAfterScan])`
sets the "phone" behaviour per connection: `Login` (default),
`CancelAfterScan`, `ExpireAfterInit`, `WrongFingerprint` or `Idle`. The server
performs the real RSA-OAEP/SHA-256 handshake. It checks `nonce_proof`, announces
`base64url(sha256(SPKI))` and encrypts the user payload and the token returned
by `POST /users/@me/remote-auth/login`.

Fixtures (`ferrocord_testkit::fixtures`): `UserBuilder`, `ChannelBuilder`,
`GuildBuilder`, `MessageBuilder` and `ReadyBuilder` produce `serde_json::Value`s
in the shape Discord sends to user accounts. `ReadyBuilder::standard()` has one
guild in a folder (category, `#general`, `#random`, voice), a friend with an
unread DM, and read states. Its ids are in `fixtures::ids`. If you change a
fixture, `crates/model/tests/payloads.rs` checks that it still parses.

## Conventions

- Never contact real Discord hosts from `cargo test`. Every client is pointed at
  the mock, and no test reads real tokens or the credential vault.
- Use the `wait_for_*` helpers (5 s timeout, event-driven) instead of sleeping.
  Keep sleeps for timing assertions (rate limits) short and assert lower bounds
  only.
- Do not use `tokio::time::pause()` for tests with network I/O. Auto-advance
  fires heartbeat and connect timers while the runtime waits on sockets. The
  mock runs on its own runtime, but the client under test does not.
- Core tests use `#[tokio::test(flavor = "multi_thread")]`, because `Client`
  spawns on the runtime and uses blocking locks. Never hold `client.store()`
  across an `.await`.
- A test for a known bug that cannot be fixed yet is marked
  `#[ignore = "bug: <description>"]` and must fail when run with `--ignored`.
- Format only the files you touched (`rustfmt --edition 2024 <files>`). The
  repo's `rustfmt.toml` applies.
