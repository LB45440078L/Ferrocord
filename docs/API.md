# Ferrocord local API

Ferrocord can expose a small HTTP + WebSocket API on your own machine so that
external tools — scripts, status-bar widgets, stream-deck buttons, bots that react to
what you see — can read the client's state, receive live events and, if you allow it,
perform actions.

- [Enabling the API](#enabling-the-api)
- [Authentication](#authentication)
- [Security model](#security-model)
- [Conventions](#conventions)
- [REST endpoints](#rest-endpoints)
- [WebSocket](#websocket)
- [Examples](#examples)
- [Embedding (for Ferrocord developers)](#embedding-for-ferrocord-developers)

## Enabling the API

The API is **off by default**. It is controlled by the `[api]` section of
Ferrocord's `config.toml` (also exposed in the settings UI):

```toml
[api]
enabled = true         # start the server
port = 6473            # TCP port on 127.0.0.1
allow_actions = false  # allow sending messages, reacting, typing, acking, presence
```

The server listens on `127.0.0.1:<port>` only — it is never reachable from other
machines. `allow_actions` can be toggled at any time; it is checked on every request.

## Authentication

Every request must carry the API token:

```
Authorization: Bearer <token>
```

The token is 64 hex characters (32 random bytes) generated on the first start and
stored in `api-token.txt` in Ferrocord's config directory:

| OS      | Token file |
|---------|------------|
| Windows | `%APPDATA%\ferrocord\Ferrocord\config\api-token.txt` |
| Linux   | `~/.config/ferrocord/api-token.txt` (or `$XDG_CONFIG_HOME/ferrocord/api-token.txt`) |
| macOS   | `~/Library/Application Support/dev.ferrocord.Ferrocord/api-token.txt` |

On Unix the file permissions are restricted to `0600`. Regenerating the token (from the
settings, or by deleting the file and restarting Ferrocord) immediately invalidates
the old one for new requests.

The WebSocket endpoint additionally accepts the token as a query parameter,
`/v1/ws?token=<token>`, for clients that cannot set headers. The query parameter is
**not** accepted on REST endpoints.

Tokens are compared in constant time. A missing or wrong token yields
`401 {"error": "...", "code": "unauthorized"}` with `WWW-Authenticate: Bearer`.

## Security model

The API can read your private messages and (optionally) act as you, so it is
locked down in several layers:

1. **Loopback only.** The socket is bound to `127.0.0.1`.
2. **Token.** Other local users or programs cannot use the API without reading the
   token file from your profile.
3. **DNS-rebinding protection.** The `Host` header must be exactly
   `127.0.0.1:<port>` or `localhost:<port>`; anything else gets
   `403 forbidden_host`. A malicious web page that points its own domain at
   `127.0.0.1` therefore cannot talk to the API.
4. **No browsers.** Browsers attach an `Origin` header to cross-site requests.
   Requests whose `Origin` is anything other than absent, `null`, or Ferrocord's own
   origin (`http://127.0.0.1:<port>` / `http://localhost:<port>`) get
   `403 forbidden_origin`. Ferrocord never sends CORS headers, so web pages cannot
   read responses either. Command-line tools and normal HTTP/WebSocket libraries do
   not send `Origin` (some WebSocket libraries send the server's own origin, which is
   accepted).
5. **Read-only by default.** Endpoints that change anything (send, react, type, ack,
   presence) return `403 actions_disabled` unless `allow_actions = true`.
6. **Discord-side checks still apply.** Actions use your normal session and are
   subject to Discord's permissions and rate limits.

Checks run in this order: Host → Origin → token → endpoint (actions flag, login state,
parameters).

Treat the token like a password. Anything that has it can read all your messages.

## Conventions

- Base URL: `http://127.0.0.1:6473` (adjust the port). All paths start with `/v1`.
- Request and response bodies are JSON (`Content-Type: application/json`).
- **Snowflake ids are strings** (`"175928847299117063"`) in responses. Requests accept
  strings (preferred) or numbers.
- Discord objects (messages) are returned in Discord's own JSON format.
- Most data comes from Ferrocord's in-memory cache, so it is what the client
  currently knows (e.g. only guilds/channels/users received from the gateway).
- Until Ferrocord has logged in and received `READY`, every endpoint except
  `/v1/info` returns `503 not_ready`.

### Errors

Errors use an HTTP status code and a JSON body:

```json
{ "error": "write actions are disabled; enable \"Allow actions\" in Ferrocord's API settings", "code": "actions_disabled" }
```

| Status | `code` | Meaning |
|--------|--------|---------|
| 400 | `bad_request` | Invalid id, query parameter or JSON body |
| 400 | `discord_bad_request` | Discord rejected the request (e.g. message too long) |
| 401 | `unauthorized` | Missing or invalid token |
| 403 | `forbidden_host` | `Host` header is not `127.0.0.1:<port>`/`localhost:<port>` |
| 403 | `forbidden_origin` | Request came from a web page |
| 403 | `actions_disabled` | Write endpoint while `allow_actions = false` |
| 403 | `discord_forbidden` | Discord denied the action (missing permission, blocked…) |
| 404 | `not_found` | Unknown endpoint, or object not found / not visible |
| 405 | `method_not_allowed` | Wrong HTTP method for the endpoint |
| 429 | `rate_limited` | Discord rate limit lasted too long |
| 502 | `discord_error` | Network or unexpected Discord error |
| 503 | `not_ready` | Ferrocord is not logged in yet |

## REST endpoints

Read endpoints:

| Method & path | Description |
|---------------|-------------|
| `GET /v1/info` | Version, connection state, latency |
| `GET /v1/me` | The logged-in user |
| `GET /v1/guilds` | Guilds in sidebar order with unread state |
| `GET /v1/guilds/{guild_id}` | One guild with roles |
| `GET /v1/guilds/{guild_id}/channels` | Channels of a guild you can view |
| `GET /v1/channels/{channel_id}` | One channel (guild channel, DM or thread) |
| `GET /v1/channels/{channel_id}/messages?limit=&before=` | Message history |
| `GET /v1/users/{user_id}` | A user |
| `GET /v1/dms` | DMs and group DMs |
| `GET /v1/relationships` | Friends, blocks and friend requests |

Write endpoints (require `allow_actions = true`):

| Method & path | Description |
|---------------|-------------|
| `POST /v1/channels/{channel_id}/messages` | Send a message |
| `POST /v1/channels/{channel_id}/typing` | Show the typing indicator |
| `POST /v1/channels/{channel_id}/ack` | Mark a channel read |
| `PUT /v1/channels/{channel_id}/messages/{message_id}/reactions/{emoji}` | Add your reaction |
| `DELETE /v1/channels/{channel_id}/messages/{message_id}/reactions/{emoji}` | Remove your reaction |
| `PATCH /v1/presence` | Change status / custom status |

The examples below use these shell variables:

```sh
TOKEN=$(cat ~/.config/ferrocord/api-token.txt)
API=http://127.0.0.1:6473
```

### `GET /v1/info`

Works before login; useful as a health check.

```sh
curl -s -H "Authorization: Bearer $TOKEN" $API/v1/info
```

```json
{
  "name": "ferrocord",
  "version": "0.1.0",
  "api_version": 1,
  "connection": "Connected",
  "latency_ms": 41,
  "logged_in": true,
  "user_id": "80351110224678912",
  "actions_allowed": false
}
```

`connection` is one of `Connecting`, `Identifying`, `Resuming`, `Connected`,
`Reconnecting(n)`, `Disconnected`. `latency_ms` is the last heartbeat round-trip
(`null` before the first one).

### `GET /v1/me`

```json
{
  "id": "80351110224678912",
  "username": "nelly",
  "global_name": "Nelly",
  "display_name": "Nelly",
  "discriminator": "0",
  "avatar_url": "https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.webp?size=128",
  "bot": false,
  "status": "online",
  "custom_status": { "text": "shipping", "emoji_name": null },
  "mfa_enabled": true,
  "verified": true
}
```

Email address and phone number are never exposed. `custom_status` is `null` when unset.

**User objects** (used by `/v1/me`, `/v1/users/{id}`, channel recipients and
relationships) have the fields `id`, `username`, `global_name`, `display_name`,
`discriminator`, `avatar_url`, `bot`, `status` (`online`, `idle`, `dnd`, `invisible`,
`offline`).

### `GET /v1/guilds`

```json
[
  {
    "id": "613425648685547541",
    "name": "Rust Programming",
    "icon_url": "https://cdn.discordapp.com/icons/613425648685547541/a_1234.gif?size=128",
    "unavailable": false,
    "member_count": 41234,
    "unread": true,
    "mention_count": 2,
    "muted": false
  }
]
```

Guilds are listed in the user's sidebar order (folders flattened). `icon_url` is
`null` for guilds without an icon.

### `GET /v1/guilds/{guild_id}`

The guild summary above plus details:

```json
{
  "id": "613425648685547541",
  "name": "Rust Programming",
  "icon_url": null,
  "unavailable": false,
  "member_count": 41234,
  "unread": false,
  "mention_count": 0,
  "muted": false,
  "description": "A place to talk about Rust",
  "owner_id": "1234567890",
  "features": ["COMMUNITY", "NEWS"],
  "premium_tier": 3,
  "joined_at": "2021-03-04T10:11:12.000000+00:00",
  "roles": [
    { "id": "6134...", "name": "Moderator", "color": 15844367, "position": 10, "hoist": true, "mentionable": false }
  ]
}
```

Roles are sorted highest first. Unknown guild: `404 not_found`.

### `GET /v1/guilds/{guild_id}/channels`

Channels you are allowed to view, in sidebar order: uncategorised channels first, then
each category followed by its channels. Threads are not included.

```json
[
  {
    "id": "613425649135337473",
    "type": 0,
    "name": "general",
    "display_name": "general",
    "guild_id": "613425648685547541",
    "parent_id": "613425649135337472",
    "position": 0,
    "topic": "Talk about anything",
    "nsfw": false,
    "last_message_id": "1300000000000000000",
    "icon_url": null,
    "recipients": [],
    "unread": true,
    "mention_count": 0,
    "muted": false
  }
]
```

`type` is Discord's channel type: 0 text, 1 DM, 2 voice, 3 group DM, 4 category,
5 announcement, 10/11/12 threads, 13 stage, 15 forum, 16 media.

### `GET /v1/channels/{channel_id}`

A channel object as above. For DMs and group DMs `recipients` lists user objects and
`guild_id` is `null`. Channels you cannot view, or that Ferrocord does not know,
return `404 not_found`.

### `GET /v1/channels/{channel_id}/messages`

| Query | Default | Description |
|-------|---------|-------------|
| `limit` | 50 | 1–100 messages |
| `before` | — | Only messages older than this message id |

Returns Discord message objects, **newest first**. Served from Ferrocord's cache when
it holds a complete page; otherwise fetched from Discord (the fetched page is not added
to the cache).

```sh
curl -s -H "Authorization: Bearer $TOKEN" "$API/v1/channels/613425649135337473/messages?limit=2"
```

```json
[
  {
    "id": "1300000000000000001",
    "channel_id": "613425649135337473",
    "guild_id": "613425648685547541",
    "author": { "id": "80351110224678912", "username": "nelly", "global_name": "Nelly", "avatar": null, "...": "..." },
    "content": "hello world",
    "timestamp": "2026-10-31T12:00:00.000000+00:00",
    "edited_timestamp": null,
    "attachments": [],
    "embeds": [],
    "reactions": [],
    "mentions": [],
    "type": 0,
    "...": "..."
  }
]
```

### `GET /v1/users/{user_id}`

A user object from the cache. Users Ferrocord has not seen are fetched from Discord's
profile endpoint (their `status` is then reported as `offline`); unknown users give
`404 not_found`.

### `GET /v1/dms`

Channel objects for DMs and group DMs, most recent activity first.

### `GET /v1/relationships`

```json
[
  {
    "id": "123456789012345678",
    "type": 1,
    "type_name": "friend",
    "nickname": null,
    "since": "2024-02-01T09:00:00.000000+00:00",
    "user": { "id": "123456789012345678", "username": "ferris", "...": "..." }
  }
]
```

`type_name` is one of `friend`, `blocked`, `incoming_request`, `outgoing_request`,
`implicit`, `none`. `user` may be `null` if the user is not cached.

### `POST /v1/channels/{channel_id}/messages`

*Requires `allow_actions`.*

```json
{ "content": "Hello from a script", "reply_to": "1300000000000000001" }
```

`content` is required (1–4000 characters; Discord enforces its own limit of 2000
without Nitro). `reply_to` is optional and makes the message a reply (pinging the
author). The channel must be visible to you. Returns `200` with the created Discord
message object.

```sh
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
     -d '{"content":"Hello from curl"}' $API/v1/channels/613425649135337473/messages
```

Messages sent through the API go straight to Discord: they do not run plugin
`on_send` hooks or local `/commands`.

### `POST /v1/channels/{channel_id}/typing`

*Requires `allow_actions`.* Shows "… is typing" for ~10 seconds. Returns `204`.

### `POST /v1/channels/{channel_id}/ack`

*Requires `allow_actions`.* Marks the channel as read. With no body (or `{}`), it is
marked read up to its latest known message; with `{"message_id": "..."}` it is marked
read up to that message. Returns `204`.

### `PUT` / `DELETE /v1/channels/{channel_id}/messages/{message_id}/reactions/{emoji}`

*Requires `allow_actions`.* Adds or removes **your** reaction. `{emoji}` is either a
URL-encoded Unicode emoji (`%F0%9F%91%8D` for 👍) or a custom emoji as `name:id`
(`<:name:id>` and `<a:name:id>` are also accepted, URL-encoded). Returns `204`.

```sh
curl -s -X PUT -H "Authorization: Bearer $TOKEN" \
     $API/v1/channels/613425649135337473/messages/1300000000000000001/reactions/%F0%9F%91%8D
```

### `PATCH /v1/presence`

*Requires `allow_actions`.*

```json
{ "status": "dnd", "custom_status": "Focus time" }
```

Both fields are optional, but at least one must be present.

- `status`: `online`, `idle`, `dnd` or `invisible`.
- `custom_status`: a string up to 128 characters, or `null` / `""` to clear it.

Returns the resulting state, e.g. `{"status": "dnd", "custom_status": "Focus time"}`.
The change is applied like the status picker in the client (gateway presence + synced
settings).

## WebSocket

`GET /v1/ws` upgrades to a WebSocket. Authenticate with the `Authorization` header or
`?token=`; the Host/Origin rules above apply to the handshake. All frames are JSON
text frames (binary frames are rejected); client frames are limited to 1 MiB.

```
ws://127.0.0.1:6473/v1/ws?token=<token>
```

### Server → client

**Hello** — sent once after connecting:

```json
{
  "type": "hello",
  "version": "0.1.0",
  "api_version": 1,
  "actions_allowed": false,
  "methods": ["info", "me", "guilds", "guild", "guild_channels", "channel", "messages", "user", "dms", "relationships",
              "send_message", "typing", "ack", "add_reaction", "remove_reaction", "set_presence"]
}
```

**Event** — every client event (all by default, see *subscribe*):

```json
{ "type": "event", "event": { "type": "MESSAGE_CREATE", "data": { "id": "1300000000000000002", "content": "hi", "...": "..." } } }
```

`event.type` is the gateway dispatch name (`MESSAGE_CREATE`, `MESSAGE_UPDATE`,
`MESSAGE_DELETE`, `PRESENCE_UPDATE`, `TYPING_START`, `GUILD_CREATE`, `READY`, …) with
`data` being Discord's raw payload, or one of Ferrocord's own events:

| `type` | `data` |
|--------|--------|
| `CONNECTION_STATE` | `"Connected"`, `"Reconnecting(1)"`, … |
| `LATENCY` | heartbeat round-trip in ms |
| `LOGGED_OUT` | `{ "reason": "..." }` |
| `NOTIFICATION` | `{ "title", "body", "channel_id", "guild_id", "message_id", "icon_url" }` — a message that would show a desktop notification |
| `MESSAGES_LOADED` | `{ "channel_id": "..." }` |
| `ACTION_FAILED` | `{ "action": "...", "error": "..." }` |

**Response** — answer to an RPC request:

```json
{ "type": "response", "id": 1, "result": { "...": "..." } }
{ "type": "response", "id": 2, "error": { "error": "write actions are disabled; ...", "code": "actions_disabled", "status": 403 } }
```

**Subscribed** — acknowledgement of a subscribe op: `{"type": "subscribed", "events": ["MESSAGE_CREATE"]}`
(`["*"]` means all events).

**Lagged** — `{"type": "lagged", "missed": 128}`: the connection could not keep up and
that many events were skipped. The session continues; refetch state over REST/RPC if
you need consistency.

**Error** — `{"type": "error", "error": {...}}` for frames that are not valid JSON or
not a known op.

When the server shuts down (API disabled or Ferrocord exits) it sends a Close frame.

### Client → server

**Subscribe** — choose which events to receive (replaces the previous filter):

```json
{ "op": "subscribe", "events": ["MESSAGE_CREATE", "MESSAGE_DELETE", "NOTIFICATION"] }
```

Names are case-insensitive. An empty list or `["*"]` restores the default (all events).

**RPC request** — mirrors the REST endpoints:

```json
{ "id": 1, "method": "messages", "params": { "channel_id": "613425649135337473", "limit": 10 } }
```

`id` may be any JSON value and is echoed back; requests are processed concurrently, so
responses can arrive out of order and interleaved with events. At most 32 requests may
be in flight per connection (more get `too_many_requests`). Unknown methods get
`unknown_method`.

| Method | Params | REST equivalent |
|--------|--------|-----------------|
| `info` | — | `GET /v1/info` |
| `me` | — | `GET /v1/me` |
| `guilds` | — | `GET /v1/guilds` |
| `guild` | `guild_id` | `GET /v1/guilds/{id}` |
| `guild_channels` | `guild_id` | `GET /v1/guilds/{id}/channels` |
| `channel` | `channel_id` | `GET /v1/channels/{id}` |
| `messages` | `channel_id`, `limit?`, `before?` | `GET /v1/channels/{id}/messages` |
| `user` | `user_id` | `GET /v1/users/{id}` |
| `dms` | — | `GET /v1/dms` |
| `relationships` | — | `GET /v1/relationships` |
| `send_message` | `channel_id`, `content`, `reply_to?` | `POST /v1/channels/{id}/messages` |
| `typing` | `channel_id` | `POST /v1/channels/{id}/typing` (result `null`) |
| `ack` | `channel_id`, `message_id?` | `POST /v1/channels/{id}/ack` (result `null`) |
| `add_reaction` | `channel_id`, `message_id`, `emoji` | `PUT …/reactions/{emoji}` (result `null`) |
| `remove_reaction` | `channel_id`, `message_id`, `emoji` | `DELETE …/reactions/{emoji}` (result `null`) |
| `set_presence` | `status?`, `custom_status?` | `PATCH /v1/presence` |

`emoji` in RPC params is plain (not URL-encoded): `"👍"` or `"name:id"`. Write methods
follow the same `allow_actions` rule as REST and fail with `actions_disabled`.

## Examples

### curl

```sh
TOKEN=$(cat ~/.config/ferrocord/api-token.txt)   # Windows (PowerShell): $TOKEN = Get-Content "$env:APPDATA\ferrocord\Ferrocord\config\api-token.txt"
API=http://127.0.0.1:6473
H="Authorization: Bearer $TOKEN"

curl -s -H "$H" $API/v1/info
curl -s -H "$H" $API/v1/guilds
curl -s -H "$H" "$API/v1/channels/613425649135337473/messages?limit=5"

# Requires allow_actions = true
curl -s -X POST -H "$H" -H "Content-Type: application/json" -d '{"content":"hi"}' \
     $API/v1/channels/613425649135337473/messages
curl -s -X PATCH -H "$H" -H "Content-Type: application/json" -d '{"status":"idle","custom_status":"brb"}' \
     $API/v1/presence
```

### Python WebSocket client

Uses the [`websockets`](https://pypi.org/project/websockets/) library
(`pip install websockets`), which does not send an `Origin` header.

```python
import asyncio
import json
import os
import pathlib
import sys

import websockets

PORT = 6473


def token_path() -> pathlib.Path:
    if sys.platform == "win32":
        return pathlib.Path(os.environ["APPDATA"]) / "ferrocord" / "Ferrocord" / "config" / "api-token.txt"
    if sys.platform == "darwin":
        return pathlib.Path.home() / "Library/Application Support/dev.ferrocord.Ferrocord/api-token.txt"
    base = os.environ.get("XDG_CONFIG_HOME", str(pathlib.Path.home() / ".config"))
    return pathlib.Path(base) / "ferrocord" / "api-token.txt"


async def main() -> None:
    token = token_path().read_text().strip()
    async with websockets.connect(f"ws://127.0.0.1:{PORT}/v1/ws?token={token}") as ws:
        hello = json.loads(await ws.recv())
        print("connected to Ferrocord", hello["version"], "- actions allowed:", hello["actions_allowed"])

        await ws.send(json.dumps({"op": "subscribe", "events": ["MESSAGE_CREATE", "NOTIFICATION"]}))
        await ws.send(json.dumps({"id": 1, "method": "me"}))

        async for frame in ws:
            msg = json.loads(frame)
            if msg["type"] == "response" and msg["id"] == 1:
                if "error" in msg:
                    print("me failed:", msg["error"]["code"])
                else:
                    print("logged in as", msg["result"]["display_name"])
            elif msg["type"] == "event":
                event = msg["event"]
                if event["type"] == "MESSAGE_CREATE":
                    m = event["data"]
                    print(f'#{m["channel_id"]} {m["author"]["username"]}: {m["content"]}')
                elif event["type"] == "NOTIFICATION":
                    print("notification:", event["data"]["title"], "-", event["data"]["body"])
            elif msg["type"] == "lagged":
                print("missed", msg["missed"], "events")


asyncio.run(main())
```

To send a message over the WebSocket (with `allow_actions = true`):

```python
await ws.send(json.dumps({
    "id": 2,
    "method": "send_message",
    "params": {"channel_id": "613425649135337473", "content": "sent over the websocket"},
}))
```

## Embedding (for Ferrocord developers)

The `ferrocord-api` crate exposes:

| Item | Description |
|------|-------------|
| `ApiServer::start(client) -> io::Result<ApiServerHandle>` | Binds `127.0.0.1:<config.api.port>` (port `0` = any free port), creates the token file if needed and serves on the client's Tokio runtime. Callable from any thread. The caller decides based on `config.api.enabled`. |
| `ApiServerHandle::addr()` | Bound socket address. |
| `ApiServerHandle::token()` | Current token. |
| `ApiServerHandle::regenerate_token() -> io::Result<String>` | New token for the file and the running server. |
| `ApiServerHandle::is_running()` | Whether the server task is alive. |
| `ApiServerHandle::stop()` | Stops accepting connections and closes WebSocket sessions. Dropping the handle does the same. |
| `token_path(paths)`, `load_or_create_token(paths)`, `regenerate_token(paths)` | Token file helpers usable without a running server. |

To apply a port change, stop the server and start it again.
