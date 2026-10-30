# Ferrocord plugins

Ferrocord can be extended with small scripts written in [Luau](https://luau.org), the
fast, sandboxable dialect of Lua 5.1 (with type annotations, `+=`, `continue`,
`string.split`, `table.find`, `math.clamp` and friends).

Plugins are deliberately limited: each one runs in its own sandbox, only sees
the parts of the client it asked for, and only after you agreed. A slow, broken or
malicious plugin cannot freeze or crash Ferrocord.

- [Quick start](#quick-start)
- [How plugins run](#how-plugins-run)
- [The manifest (`plugin.toml`)](#the-manifest-plugintoml)
- [Permissions](#permissions)
- [Lua API reference](#lua-api-reference)
- [Limits and timeouts](#limits-and-timeouts)
- [Sandbox guarantees](#sandbox-guarantees)
- [Installing, enabling and granting](#installing-enabling-and-granting)
- [Example plugins](#example-plugins)
- [Host API (for Ferrocord developers)](#host-api-for-ferrocord-developers)

## Quick start

1. Create a folder named after your plugin id inside the plugins directory:

   | OS      | Plugins directory                                                  |
   |---------|--------------------------------------------------------------------|
   | Windows | `%APPDATA%\ferrocord\Ferrocord\data\plugins\`                      |
   | Linux   | `~/.local/share/ferrocord/plugins/` (or `$XDG_DATA_HOME/ferrocord/plugins/`) |
   | macOS   | `~/Library/Application Support/dev.ferrocord.Ferrocord/plugins/`  |

2. Add `plugin.toml`:

   ```toml
   id = "shout"
   name = "Shout"
   version = "0.1.0"
   author = "you"
   description = "Adds /shout, which sends your text in capitals."
   permissions = ["commands", "messages.send"]
   ```

3. Add `main.lua`:

   ```lua
   ferrocord.command("shout", "Send text in capitals", function(channel_id, args)
       if args == "" then
           ferrocord.log("usage: /shout <text>")
           return
       end
       ferrocord.send_message(channel_id, string.upper(args))
   end)
   ```

4. In Ferrocord's plugin settings, enable **Shout** and grant the two permissions it
   asks for (see [Installing, enabling and granting](#installing-enabling-and-granting)).
   Type `/shout hello` in any channel.

## How plugins run

- **One thread per plugin.** Every enabled plugin gets a dedicated OS thread that
  owns its Luau VM. The client never runs Lua itself; it posts messages (events,
  commands, hook requests) into the plugin's bounded queue, and the plugin thread
  processes them one at a time. Plugin code is therefore single-threaded: there are
  no data races to worry about and no locks to take.
- **Loading.** When a plugin is enabled (or Ferrocord starts with it enabled), its
  entry script runs once from top to bottom. That is where you register commands,
  event handlers, hooks and timers. The script must finish within the load budget
  (2 s) — a load-time error or timeout marks the plugin as failed and shows the
  error with the plugin.
- **After loading** the thread waits for work: queued messages and due timers. All
  your callbacks run on that same thread.
- **Unloading.** Disabling, reloading or changing the permissions of a plugin stops
  its thread and throws away the VM (all Lua state is lost; use
  [`ferrocord.storage`](#ferrocordstorage) for anything that must survive). Granting
  or revoking a permission automatically restarts a running plugin, because the API
  is assembled when the VM is created.
- **Errors never escape.** An error in any callback is caught, written to the
  plugin's log (kept for display in the UI and written to Ferrocord's log output) and
  the plugin keeps running.

## The manifest (`plugin.toml`)

```toml
id = "keyword-alerts"            # required
name = "Keyword Alerts"          # defaults to the id
version = "1.0.0"                # defaults to "0.0.0"
author = "Ferrocord contributors" # optional
description = "Notifies you when a message contains one of your keywords." # optional
entry = "main.lua"               # defaults to "main.lua"
permissions = ["events.messages", "notify", "storage", "commands", "state.read"]
```

| Field         | Type            | Notes |
|---------------|-----------------|-------|
| `id`          | string          | 1–64 characters of `a-z`, `0-9`, `-`, `_`, `.` (not starting with `.`). **Must equal the plugin's directory name.** |
| `name`        | string          | Display name. |
| `version`     | string          | Free-form, shown in the UI. |
| `author`      | string          | Shown in the UI. |
| `description` | string          | Shown in the UI. |
| `entry`       | string          | Path of the entry script relative to the plugin directory. Must be a `.lua` or `.luau` file inside the directory (no `..`, no absolute paths). |
| `permissions` | array of string | Permissions the plugin wants. Unknown names are ignored (and flagged in the UI). |

Only the entry script is executed. There is no `require`; put everything in one file
(scripts up to 4 MiB are accepted).

## Permissions

A plugin only receives the Lua functions belonging to a permission that it **declares
in `plugin.toml` *and* that the user granted**. Anything else is simply absent (the
field is `nil`), so a plugin can feature-detect:

```lua
if ferrocord.notify then
    ferrocord.notify("Hi", "Notifications are allowed")
else
    ferrocord.log("notify permission not granted")
end
-- or: ferrocord.has_permission("notify")
```

Grants are stored in Ferrocord's `config.toml` under `[plugins.grants]`, e.g.
`keyword-alerts = ["events.messages", "notify"]`.

| Permission        | Unlocks | Sensitive |
|-------------------|---------|-----------|
| `events.messages` | `ferrocord.on` for `MESSAGE_*` events (create, update, delete, delete bulk, reactions, acks) | |
| `events.all`      | `ferrocord.on` for every client event (presence, typing, guild/channel changes, `READY`, connection state…) | yes |
| `messages.send`   | `ferrocord.send_message`, `ferrocord.reply` | yes |
| `messages.modify` | `ferrocord.on_send` (rewrite or cancel messages you send) | yes |
| `commands`        | `ferrocord.command` (local slash commands) | |
| `state.read`      | `ferrocord.state.*` (cached guilds, channels, users, messages) | |
| `storage`         | `ferrocord.storage.*` (persistent key-value store) | |
| `notify`          | `ferrocord.notify` (desktop notifications) | |
| `http`            | `ferrocord.http.*` (HTTPS requests to any host) | yes |

"Sensitive" permissions can leak your data or act on your behalf (`PermissionStatus::sensitive`
lets the UI highlight them).

## Lua API reference

Everything lives in the global table `ferrocord`. Conventions:

- **Snowflake ids are strings** (`"175928847299117063"`), because Discord ids do not
  fit in a Lua number. Functions taking an id also accept a small integer, but always
  prefer strings.
- JSON `null` becomes `nil`; JSON arrays become sequences (1-based); objects become tables.
- Functions that talk to Discord or the network are **asynchronous**: they return
  immediately and call an optional `callback(result, err)` later, on the plugin thread.
  Exactly one of `result`/`err` is non-`nil`.
- Invalid arguments raise a Lua error (catch with `pcall` if needed).

### Always available

#### `ferrocord.plugin`
Read-only table describing the plugin: `id`, `name`, `version`, `author`,
`description` and `permissions` (the *effective* permissions, i.e. declared and granted).

```lua
ferrocord.log(ferrocord.plugin.name .. " v" .. ferrocord.plugin.version)
```

#### `ferrocord.log(...)`
Writes its arguments (converted with `tostring`, separated by tabs) to the plugin log.
The global `print` does the same.

```lua
ferrocord.log("loaded with", #ferrocord.plugin.permissions, "permissions")
```

#### `ferrocord.has_permission(name) -> boolean`
Whether the permission is declared **and** granted.

#### `ferrocord.after(ms, fn) -> timer_id`
Runs `fn()` once after `ms` milliseconds.

#### `ferrocord.every(ms, fn) -> timer_id`
Runs `fn()` every `ms` milliseconds (minimum 50 ms). The next run is scheduled after
the previous one finished, so slow callbacks do not pile up.

#### `ferrocord.cancel(timer_id) -> boolean`
Cancels a timer; returns `false` if it already fired or never existed.

```lua
local ticks = 0
local id
id = ferrocord.every(1000, function()
    ticks += 1
    if ticks == 10 then
        ferrocord.cancel(id)
    end
end)
ferrocord.after(5000, function() ferrocord.log("five seconds passed") end)
```

At most 256 timers can exist at once.

#### `ferrocord.json.encode(value) -> string` / `ferrocord.json.decode(string) -> value`
JSON helpers (useful with `ferrocord.http`).

### Events — `events.messages` / `events.all`

#### `ferrocord.on(event_name, fn)`
Calls `fn(data, event_name)` whenever the client emits the event. Several handlers
can be registered for the same event; they run in registration order. `event_name`
is case-insensitive. `"*"` subscribes to every event the plugin may receive.

With only `events.messages`, `event_name` must start with `MESSAGE_` (otherwise
`ferrocord.on` raises an error). With `events.all`, any event is allowed.

`data` is the event payload exactly as Discord sent it (gateway dispatches), or:

| Event              | `data` |
|--------------------|--------|
| `CONNECTION_STATE` | state name, e.g. `"Connected"` |
| `LATENCY`          | heartbeat round-trip in ms |
| `LOGGED_OUT`       | `{ reason = "..." }` |
| `NOTIFICATION`     | `{ title, body, channel_id, guild_id, message_id, icon_url }` |
| `MESSAGES_LOADED`  | `{ channel_id = "..." }` |
| `ACTION_FAILED`    | `{ action = "...", error = "..." }` |

Common message events: `MESSAGE_CREATE` (a full message object: `id`, `channel_id`,
`guild_id`, `author = {id, username, global_name, ...}`, `content`, `attachments`,
`embeds`, `mentions`, …), `MESSAGE_UPDATE` (partial message), `MESSAGE_DELETE`
(`{id, channel_id, guild_id}`), `MESSAGE_DELETE_BULK` (`{ids, channel_id, guild_id}`),
`MESSAGE_REACTION_ADD` / `MESSAGE_REACTION_REMOVE`.

```lua
ferrocord.on("MESSAGE_CREATE", function(msg)
    if msg.content:find("ferris", 1, true) then
        ferrocord.log(msg.author.username .. " mentioned Ferris in " .. msg.channel_id)
    end
end)
```

Events are delivered through a queue of 1024 entries; if the plugin falls that far
behind, newer events are dropped (and a warning is logged).

### Outgoing messages — `messages.modify`

#### `ferrocord.on_send(fn)`
Calls `fn(content, info)` before each message you send from Ferrocord, where `info`
is `{ channel_id, guild_id, reply_to }` (`guild_id`/`reply_to` may be `nil`). The
handler returns:

- a **string** — the message is sent with that content instead;
- **`false`** — the message is not sent at all;
- **`nil`** (or anything else) — the message continues unchanged.

Multiple hooks are chained; each receives the content produced by the previous one.

The client waits **at most 150 ms** for all of a plugin's hooks combined. If the
plugin is busy or the hooks are too slow, the message is sent unchanged. Keep hooks
fast and do not call asynchronous APIs from them.

```lua
-- Replace "teh" with "the" and refuse to send messages that are only "lol".
ferrocord.on_send(function(content, info)
    if content == "lol" then
        return false
    end
    return (content:gsub("%f[%w]teh%f[%W]", "the"))
end)
```

Messages sent by plugins themselves (`ferrocord.send_message`) and by the local API
do not pass through `on_send` hooks.

### Commands — `commands`

#### `ferrocord.command(name, description, fn)`
Registers a local slash command. When you send `/name some text`, Ferrocord does not
send anything to Discord; instead it calls `fn(channel_id, args)` with the current
channel id and the text after the command (`"some text"`, trimmed; `""` if none).
Commands appear in the composer's autocomplete with `description`.

`name` must be 1–32 characters of `a-z`, `0-9`, `-`, `_` (a leading `/` is ignored).
Registering the same name again replaces the handler. Parse arguments yourself, e.g.
with `string.split(args, " ")` or patterns:

```lua
ferrocord.command("remind", "Remind me: /remind <seconds> <text>", function(channel_id, args)
    local secs, text = string.match(args, "^(%d+)%s+(.+)$")
    if not secs then
        ferrocord.log("usage: /remind <seconds> <text>")
        return
    end
    ferrocord.after(tonumber(secs) * 1000, function()
        if ferrocord.notify then ferrocord.notify("Reminder", text) end
    end)
end)
```

### Sending — `messages.send`

#### `ferrocord.send_message(channel_id, content [, callback])`
Sends a message to a channel. `callback(message, err)` receives the created message
object (same shape as `MESSAGE_CREATE`) or an error string.

#### `ferrocord.reply(channel_id, message_id, content [, callback])`
Sends `content` as a reply to `message_id` (pinging the author, like the official
client's default). Same callback as `send_message`.

```lua
ferrocord.on("MESSAGE_CREATE", function(msg)
    if msg.content == "!ping" then
        ferrocord.reply(msg.channel_id, msg.id, "pong", function(sent, err)
            if err then ferrocord.log("reply failed: " .. err) end
        end)
    end
end)
```

To protect your account, a plugin can send **at most 5 messages per 5 seconds**;
further calls raise a `rate limited` error. Empty messages raise an error.
Automating a user account can violate Discord's terms — use these functions
sparingly and only for things you would type yourself.

### State — `state.read`

All functions read Ferrocord's in-memory cache; they are synchronous and never hit
the network. They return `nil` (or an empty list) when the item is not cached.

| Function | Returns |
|----------|---------|
| `ferrocord.state.me()` | the logged-in user (see *user* below) or `nil` before login |
| `ferrocord.state.guilds()` | list of *guild*s in sidebar order |
| `ferrocord.state.guild(guild_id)` | a *guild* or `nil` |
| `ferrocord.state.channels(guild_id)` | list of *channel*s you can view, in sidebar order (categories first, followed by their children) |
| `ferrocord.state.channel(channel_id)` | a *channel* or `nil` |
| `ferrocord.state.dms()` | list of DM and group DM *channel*s, most recent first |
| `ferrocord.state.user(user_id [, guild_id])` | a *user* or `nil`; `display_name` honours the guild nickname when `guild_id` is given |
| `ferrocord.state.messages(channel_id [, limit])` | up to `limit` (default 50, max 100) cached messages, **oldest first**; full message objects |

- *user*: `{ id, username, global_name, display_name, tag, bot, avatar_url }`
- *guild*: `{ id, name, icon_url, owner_id, description, member_count, unavailable, unread, mentions }`
- *channel*: `{ id, type, name, display_name, guild_id, parent_id, topic, position, nsfw, last_message_id, recipient_ids, unread, mentions }`
  (`type` is Discord's channel type number: 0 text, 1 DM, 2 voice, 3 group DM, 4 category, 5 announcement, 11/12 threads, 15 forum…)

```lua
for _, g in ipairs(ferrocord.state.guilds()) do
    if g.mentions > 0 then
        ferrocord.log(g.name .. ": " .. g.mentions .. " mentions")
    end
end
```

### Storage — `storage`

A small persistent key-value store, saved as `storage.json` in the plugin's
directory. Values can be any JSON-compatible Lua value (strings, numbers, booleans,
tables without functions/cycles). Writes are saved to disk immediately.

| Function | Description |
|----------|-------------|
| `ferrocord.storage.get(key) -> value` | the stored value or `nil` |
| `ferrocord.storage.set(key, value)` | stores a value; `value = nil` deletes the key |
| `ferrocord.storage.keys() -> {string}` | all keys |

Keys are 1–256 bytes. The whole store is limited to 1 MiB of JSON; a `set` that
would exceed it raises an error and changes nothing. Note that an empty table may
come back as an empty table of either kind, and integer numbers as floats — both
behave the same in Lua.

```lua
local count = (ferrocord.storage.get("launches") or 0) + 1
ferrocord.storage.set("launches", count)
```

### Notifications — `notify`

#### `ferrocord.notify(title [, body])`
Shows a desktop notification (title up to 256 characters, body up to 1024). If the UI
has not installed a notification handler, the text is written to the plugin log.

### HTTP — `http`

Outgoing HTTPS requests to any host. **Only `https://` URLs are allowed.** Requests
time out after 30 s by default (configurable 0.1–60 s via `request`), follow
redirects, and the response body is limited to 4 MiB. Requests do **not** carry your
Discord token or cookies.

The callback receives `(response, err)` where `response` is:

```lua
{
    status = 200,            -- HTTP status code
    ok = true,               -- status is 2xx
    headers = { ["content-type"] = "application/json", ... }, -- lower-case names
    body = "...",            -- body as a string (invalid UTF-8 is replaced)
}
```

Non-2xx responses are *not* errors (`err` is only set for network failures, timeouts
and oversized bodies).

| Function | Description |
|----------|-------------|
| `ferrocord.http.get(url, callback)` | GET request |
| `ferrocord.http.post(url, body, callback)` | POST; `body` is a string, or a table that is sent as JSON with `content-type: application/json` |
| `ferrocord.http.request(options, callback)` | `options = { method = "PUT", url = "...", headers = { ["x-key"] = "..." }, body = "..." or table, timeout_ms = 10000 }` |

At most 256 asynchronous calls (HTTP requests and sends with callbacks) may be pending.

```lua
ferrocord.command("cat", "Log a cat fact", function()
    ferrocord.http.get("https://catfact.ninja/fact", function(res, err)
        if err then return ferrocord.log("request failed: " .. err) end
        if not res.ok then return ferrocord.log("HTTP " .. res.status) end
        ferrocord.log(ferrocord.json.decode(res.body).fact)
    end)
end)
```

## Limits and timeouts

| Limit | Value | What happens when exceeded |
|-------|-------|----------------------------|
| Entry script run time | 2 s | Loading fails with "script exceeded its time limit" |
| Any callback (event, command, timer, async callback) | 250 ms | The callback is aborted with a time-limit error (logged); the plugin keeps running |
| `on_send` hooks (all of a plugin's hooks together) | 150 ms | The message is sent unchanged |
| Memory per plugin | 64 MiB | The allocation fails with a memory error |
| Queued events/commands | 1024 | New events are dropped |
| Timers | 256 | `after`/`every` raise an error |
| Pending async calls | 256 | The call raises an error |
| Messages sent | 5 per 5 s | `send_message`/`reply` raise an error |
| Storage | 1 MiB JSON | `storage.set` raises an error |
| HTTP response body | 4 MiB | The callback gets an error |

Time limits are enforced by Luau's interrupt mechanism, which fires at every loop
iteration and function call, so even `while true do end` is stopped. Catching the
time-limit error with `pcall` does not help: once the budget is spent, the next
check outside the `pcall` raises it again.

## Sandbox guarantees

Every plugin VM is created with Luau's sandbox mode enabled and only these libraries:
the base library, `coroutine`, `table`, `string`, `utf8`, `bit32`, `math`, `buffer`
and Luau's `os` (which only has `clock`, `date`, `difftime` and `time`).

- **No file system or process access**: there is no `io` library, no `os.execute`,
  `os.exit`, `os.remove`, `os.getenv`, etc.
- **No code loading**: `require`, `load`, `loadstring`, `dofile` and `loadfile` are
  removed, and the entry script is only accepted as source text — precompiled
  bytecode is rejected.
- **No introspection escape hatches**: `debug`, `getfenv` and `setfenv` are removed;
  `collectgarbage` only supports `"count"`.
- **Read-only built-ins**: the standard libraries, their metatables and the
  `ferrocord` table are frozen. Writing a global stores it in the plugin's own
  environment, never in the shared one.
- **Isolation**: each plugin has its own VM on its own thread; plugins cannot see
  each other's globals, storage or callbacks.
- **Capability-based access**: the only way to reach Discord, the network, the disk
  (own `storage.json` only) or the desktop is through the `ferrocord` functions
  unlocked by granted permissions.
- **Resource limits**: CPU time, memory, queue sizes, timers and send rate are bounded
  (see [Limits](#limits-and-timeouts)).

Plugins are still code from someone else. Only grant sensitive permissions
(`events.all`, `messages.send`, `messages.modify`, `http`) to plugins you trust: a
plugin with `events.messages` + `http` can, for example, upload every message you
receive to a server.

## Installing, enabling and granting

The plugin settings in Ferrocord map onto these operations (each is a
[`PluginHost`](#host-api-for-ferrocord-developers) method):

- **Install**: copy the plugin folder into the plugins directory, or install it from
  any folder (`install_from_dir`), which copies it there for you. Reinstalling a plugin
  over an existing one replaces its files but keeps its `storage.json`. Installing does
  not enable a plugin.
- **Enable/disable**: enabling stores the plugin id in `config.plugins.enabled`;
  enabled plugins are started when Ferrocord starts. A plugin stays enabled even if it
  fails to load, so a fixed version is picked up on the next start or reload. The load
  error is reported with the plugin.
- **Grant/revoke**: each declared permission is granted individually (or all at
  once). A running plugin restarts automatically after a change. Nothing is granted
  by default.
- **Reload**: re-reads `plugin.toml` and the script — handy while developing.
- **Logs**: the last 200 lines of each plugin's output (logs and errors) are kept in
  memory for display. They are also written to Ferrocord's log via `tracing` (target
  `ferrocord_plugin`), e.g. run with `RUST_LOG=ferrocord_plugin=info`.
- **Uninstall**: disables the plugin, forgets its grants and deletes its folder
  (including its storage).

You can also edit `config.toml` by hand while Ferrocord is closed:

```toml
[plugins]
enabled = ["hello", "keyword-alerts"]

[plugins.grants]
hello = ["commands", "messages.send"]
keyword-alerts = ["events.messages", "notify", "storage", "commands", "state.read"]
```

### Development tips

- Keep callbacks short; do expensive work incrementally with timers.
- Use `ferrocord.log` liberally and watch the plugin log (or Ferrocord's console output).
- Luau type annotations (`local n: number = 0`) are accepted; they are not checked
  at run time.

## Example plugins

Three complete plugins live in [`plugins/examples/`](../plugins/examples):

### `hello`
*Permissions: `commands`, `messages.send`.*

Registers `/hello [name]`, which sends `👋 Hello, <name>!` (default "everyone") to the
current channel, and logs the id of the sent message or the error. Shows the basic
pattern: register a command, call an async API with a callback.

### `keyword-alerts`
*Permissions: `events.messages`, `notify`, `storage`, `commands`, `state.read`.*

Keeps a list of keywords in storage and manages it with `/alerts add <word>`,
`/alerts remove <word>` and `/alerts list`. A `MESSAGE_CREATE` handler lower-cases
each incoming message (ignoring your own, found via `state.me()`), and when a keyword
occurs it shows a notification titled `"word" in #channel` with the author and
message. Demonstrates storage, command argument parsing, event handling, state
lookups and notifications.

### `message-logger`
*Permissions: `events.messages`, `storage`, `commands`.*

Caches the content of the last 2000 messages it saw (`MESSAGE_CREATE`, updated by
`MESSAGE_UPDATE`). When `MESSAGE_DELETE` or `MESSAGE_DELETE_BULK` arrives for a cached
message, it appends `{author, channel_id, content, deleted_at}` to a list of the last 50
deleted messages persisted in storage. `/deleted [n]` writes the last *n* (default 10)
to the plugin log; `/deleted clear` empties the list. Note that it can only recover
messages it saw arrive while Ferrocord was running.

## Host API (for Ferrocord developers)

The `ferrocord-plugin` crate exposes `PluginHost` (cheap to clone):

| Method | Description |
|--------|-------------|
| `PluginHost::new(client)` | Creates the host. |
| `plugins_dir()` | `<data_dir>/plugins`. |
| `set_notifier(Fn(String, String))` | Handler for `ferrocord.notify(title, body)`; called on the plugin thread. |
| `discover() -> Vec<PluginInfo>` | All plugin folders: manifest, `enabled`, `loaded`, `error`, permission list. |
| `info(id) -> Option<PluginInfo>` | One plugin. |
| `permissions(id) -> Vec<PermissionStatus>` | Declared permissions with `granted`, `description`, `sensitive`. |
| `load_enabled() -> Vec<(String, PluginError)>` | Starts all enabled plugins (call once after creating the client). |
| `enable(id)` / `disable(id)` | Persisted in `config.plugins.enabled`. |
| `reload(id)` | Restart with fresh manifest/script. |
| `grant(id, perm)` / `revoke(id, perm)` / `grant_all(id)` | Persisted in `config.plugins.grants`; restarts a running plugin. |
| `install_from_dir(path) -> PluginManifest` | Copies a plugin folder into the plugins directory. |
| `uninstall(id)` | Disable, forget grants, delete folder. |
| `is_loaded(id)`, `logs(id) -> Vec<PluginLog>` | Status and recent output. |
| `shutdown()` | Stops all plugins (config unchanged). |

`enable`, `reload`, `grant`, `revoke` and `install_from_dir` block until the plugin's
entry script has finished (at most ~2 s) and stopping a plugin waits up to 1 s for its
current callback, so call them off the UI thread if that matters.

Each running plugin is registered with the client as an `Extension` with id
`plugin:<id>`; its local commands report that id as `LocalCommand::owner`.
