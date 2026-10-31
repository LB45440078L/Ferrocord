# Protocol notes

These notes document how the official Discord desktop client talks to
Discord's servers, as observed from:

- the desktop shell (`Discord/app-1.0.9266`: `resources/app.asar`,
  `modules/discord_desktop_core/core.asar`, `build_info.json`, `Discord.exe`
  version strings);
- the web client bundles that the desktop shell loads from `discord.com`
  (stable build **657850**, October 2026): `web.*.js`, `fast-connect.*.js` and
  lazily loaded chunks;
- live, credential-free checks against Discord (the remote-auth handshake,
  response headers).

No proprietary code is included in this repository. Only facts needed for
interoperability are recorded here. When Discord changes something, update this
file together with the code.

## Client identity

The desktop app is an Electron shell. The UI is the web app loaded from
`https://discord.com/app`, with `window.DiscordNative` injected by
`mainScreenPreload.js`.

| Item | Value (stable, Oct 2026) | Source |
|---|---|---|
| Host version | `1.0.9266` | `build_info.json` |
| Electron / Chrome | `44.9.3` / `152.0.7977.167` | `Discord.exe` |
| Web build number | `657850`, fetched at runtime from `GLOBAL_ENV.BUILD_NUMBER` in `/app` | `ClientProfile::refresh_build_number` |
| API version | `9` | `GLOBAL_ENV.API_VERSION` |
| Gateway | `wss://gateway.discord.gg` | `GLOBAL_ENV.GATEWAY_ENDPOINT` |
| Remote auth | `wss://remote-auth-gateway.discord.gg/?v=2` | `GLOBAL_ENV.REMOTE_AUTH_ENDPOINT` |

User agent:

```text
Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) discord/1.0.9266 Chrome/152.0.7977.167 Electron/44.9.3 Safari/537.36
```

### Super properties

These are sent base64-encoded as `X-Super-Properties` on every REST request and
as `properties` in IDENTIFY. For the desktop client they are built from
`DiscordNative`:

```json
{
  "os": "Windows", "browser": "Discord Client", "release_channel": "stable",
  "client_version": "1.0.9266", "os_version": "10.0.26220", "os_arch": "x64",
  "app_arch": "x64", "system_locale": "en-US", "has_client_mods": false,
  "client_launch_id": "<uuid per launch>",
  "browser_user_agent": "<UA above>", "browser_version": "44.9.3",
  "os_sdk_version": "26220", "client_build_number": 657850,
  "native_build_number": null, "client_event_source": null,
  "client_heartbeat_session_id": "<uuid>", "client_app_state": "focused"
}
```

The web client also computes `has_client_mods` by probing for globals such as
`BetterDiscord`, `Vencord` and `jQuery`.

Other headers the client sends: `X-Discord-Locale`, `X-Discord-Timezone` (IANA
name) and `X-Debug-Options: bugReporterEnabled`. Before login it sends
`X-Fingerprint`, taken from the `fingerprint` field of an unauthenticated
`GET /api/v9/experiments`.

## Gateway

- URL: `wss://gateway.discord.gg/?encoding=json&v=9&compress=zlib-stream`. The
  desktop app may use `zstd-stream` through a native module instead.
  Ferrocord uses zlib-stream: one inflate context per connection, with each
  message terminated by `00 00 FF FF`.
- Opcodes: 0 DISPATCH, 1 HEARTBEAT, 2 IDENTIFY, 3 PRESENCE_UPDATE,
  4 VOICE_STATE_UPDATE, 5 VOICE_SERVER_PING, 6 RESUME, 7 RECONNECT,
  8 REQUEST_GUILD_MEMBERS, 9 INVALID_SESSION, 10 HELLO, 11 HEARTBEAT_ACK,
  13 CALL_CONNECT, 14 GUILD_SUBSCRIPTIONS, 18–22 STREAM_*,
  24 REQUEST_GUILD_APPLICATION_COMMANDS, 28 REQUEST_FORUM_UNREADS,
  29 REMOTE_COMMAND, 30 GET_DELETED_ENTITY_IDS_NOT_MATCHING_HASH,
  31 REQUEST_SOUNDBOARD_SOUNDS, 34 REQUEST_LAST_MESSAGES,
  35 SEARCH_RECENT_MEMBERS, 37 GUILD_SUBSCRIPTIONS_BULK,
  38 GUILD_CHANNELS_RESYNC, 39 REQUEST_CHANNEL_MEMBER_COUNT.

### IDENTIFY (from `fast-connect.js`)

```json
{ "op": 2, "d": {
  "token": "…",
  "capabilities": 1734653,
  "properties": { …super properties…, "is_fast_connect": true },
  "client_state": { "guild_versions": {} }
}}
```

`capabilities` is `1734653`, or `1767421` with private-channel obfuscation. The
bits Ferrocord handles:

| Bit | Name | Effect |
|---|---|---|
| 2 | VERSIONED_READ_STATES | `read_state` becomes `{entries, partial, version}` |
| 3 | VERSIONED_USER_GUILD_SETTINGS | Same wrapper for `user_guild_settings` |
| 4 | DEDUPE_USER_OBJECTS | Users move to READY `users[]`; references become `user_id` / `recipient_ids`; members arrive in `merged_members` |
| 5 | PRIORITIZED_READY_PAYLOAD | Some data moves to `READY_SUPPLEMENTAL` |
| 8 | AUTH_TOKEN_REFRESH | READY may carry `auth_token`, which replaces the stored token |
| 9 | USER_SETTINGS_PROTO | Settings arrive as base64 protobuf (`user_settings_proto`) |
| 13 | DEBOUNCE_MESSAGE_REACTIONS | `MESSAGE_REACTION_ADD_MANY` events |

For user accounts, guild objects in READY nest their metadata under
`properties` (name, icon, owner_id, …) instead of the top level.

### Lazy guilds (op 37)

The client does not receive member lists automatically. Opening a channel
subscribes:

```json
{ "op": 37, "d": { "subscriptions": { "<guild_id>": {
  "typing": true, "threads": true, "activities": true,
  "channels": { "<channel_id>": [[0, 99]] }
}}}}
```

Batches are split at about 15 KiB. The server answers with
`GUILD_MEMBER_LIST_UPDATE` (`SYNC` / `INSERT` / `UPDATE` / `DELETE` /
`INVALIDATE` ops over a flat list of group headers and members). Scrolling
requests further 100-row windows.

### Close codes

4004 (authentication failed) and 4010–4014 are fatal. 4007 and 4009 require a
fresh IDENTIFY. Others resume, at `resume_gateway_url` when READY provided one.

## REST: authentication

| Action | Request |
|---|---|
| Login | `POST /auth/login {login, password, undelete:false, login_source:null, gift_code_sku_id:null}` |
| Login response (no MFA) | `{user_id, token}` |
| Login response (MFA) | `{user_id, mfa:true, ticket, totp, sms, backup, webauthn, login_instance_id}` |
| MFA code | `POST /auth/mfa/{totp|sms|backup} {code, ticket, login_source, gift_code_sku_id, login_instance_id}` → `{token}` |
| Send SMS | `POST /auth/mfa/sms/send {ticket}` → `{phone}` |
| Logout | `POST /auth/logout {provider:null, voip_provider:null}` |

Invalid credentials come back as `400 {code: 50035, errors: {login|password: {_errors:[…]}}}`.
Suspended accounts include `suspended_user_token`. An invalid MFA code is code
`60008`.

### Captcha interception

Any request can fail with `400` and a body containing:

```json
{ "captcha_key": ["captcha-required"], "captcha_sitekey": "…", "captcha_service": "hcaptcha",
  "captcha_rqdata": "…", "captcha_rqtoken": "…", "captcha_session_id": "…", "should_serve_invisible": false }
```

The client renders the widget (`hcaptcha.render`, then `setData({rqdata})`; in
invisible mode it calls `execute()`), and then **replays the same request** with
these headers:

```text
X-Captcha-Key: <solution>
X-Captcha-Rqtoken: <captcha_rqtoken>
X-Captcha-Session-Id: <captcha_session_id>
```

Ferrocord renders the widget in a helper WebView on
`https://discord.com/robots.txt`. That page is same-origin with the official
client and serves no `Content-Security-Policy`, so the site key validates
against the hostname `discord.com`.

### MFA for sensitive actions

`401 {code: 60003, mfa: {ticket, methods:[{type}]}}` → `POST /mfa/finish {ticket, mfa_type, data}` → `{token}`.
The original request is then replayed with `X-Discord-MFA-Authorization: <token>`.

## Remote auth (QR login), v2

Taken from the `useAuthWebsocket` hook in the web client. It was verified live
with `cargo run -p ferrocord-auth --example qr_probe`.

1. Connect to `wss://remote-auth-gateway.discord.gg/?v=2` with
   `Origin: https://discord.com`.
2. Generate an RSA-2048 key pair (RSA-OAEP, SHA-256). On open, send
   `{"op":"init","encoded_public_key": base64(SPKI DER)}`.
3. `hello {heartbeat_interval, timeout_ms}`: send `{"op":"heartbeat"}` at that
   interval and expect `heartbeat_ack`.
4. `nonce_proof {encrypted_nonce}`: decrypt it, then send
   `{"op":"nonce_proof","nonce": base64url_nopad(plaintext)}`. The plaintext is
   sent back, **not** a hash of it.
5. `pending_remote_init {fingerprint}`: check that it equals
   `base64url_nopad(SHA-256(SPKI))` and show the QR code for
   `https://discord.com/ra/{fingerprint}`.
6. `pending_ticket {encrypted_user_payload}`: the decrypted payload is
   `"{id}:{discriminator}:{avatar}:{username}"`. Show "check your phone".
7. `pending_login {ticket}`: call `POST /users/@me/remote-auth/login {ticket}`,
   which returns `{encrypted_token}`. The decrypted value is the user token.
8. `cancel` means the user declined on the phone, so restart. The socket closes
   when the code expires, and the client reconnects with a new key.

## User settings protobuf

`PreloadedUserSettings` (settings type 1) is delivered base64-encoded in READY
and in `USER_SETTINGS_PROTO_UPDATE`. Fields Ferrocord uses:

| Field | Message |
|---|---|
| 11 `status` | `{1 status: StringValue, 2 custom_status: {1 text, 2 emoji_id fixed64, 3 emoji_name, 4 expires_at_ms fixed64, 5 created_at_ms fixed64}, 3 show_current_game: BoolValue}` |
| 12 `localization` | `{1 locale: StringValue, 2 timezone_offset: Int32Value}` |
| 13 `appearance` | `{1 theme, 2 developer_mode}` |
| 14 `guild_folders` | `{1 folders: [{1 guild_ids: repeated fixed64, 2 id: Int64Value, 3 name: StringValue, 4 color: UInt64Value}], 2 guild_positions: repeated fixed64}` |

Updates are sent as **partial** protos with
`PATCH /users/@me/settings-proto/1 {settings: base64}`. The server merges them.

## Other REST details used

- Message send: `POST /channels/{id}/messages` with `nonce` (a snowflake for
  "now"), `enforce_nonce: true`, `mobile_network_type: "unknown"`. Attachments
  use multipart with `payload_json` + `files[n]`.
- Read state: `POST /channels/{c}/messages/{m}/ack {token:null, last_viewed: <days since 2015>}`.
  Bulk: `POST /read-states/ack-bulk {read_states:[{channel_id, message_id, read_state_type:0}]}`.
- Reactions:
  `PUT /channels/{c}/messages/{m}/reactions/{emoji}/@me?location=Message&type=0`.
  Removal: `DELETE …/{emoji}/0/@me?location=Message&burst=false`.
- Pins: `GET /channels/{c}/messages/pins` returns
  `{items:[{message, pinned_at}], has_more}`.
- Friend requests by username: `POST /users/@me/relationships {username, discriminator:null}`
  with `X-Context-Properties: base64({"location":"Add Friend"})`.
- Rate limits follow `X-RateLimit-Bucket` / `-Remaining` / `-Reset-After`. A
  429 body carries `retry_after` (seconds, float) and `global`.

## Not yet covered

The native modules (`discord_voice.node`, `discord_media`, `discord_krisp`) and
the RTC protocol (voice gateway v8, UDP/RTP, DAVE MLS end-to-end encryption)
have not been documented or implemented yet. That is the main gap to full
parity. Contributions welcome.
