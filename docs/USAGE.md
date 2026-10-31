# Using Ferrocord

## Installing

### From source

```sh
git clone https://github.com/ferrocord/ferrocord
cd ferrocord
cargo build --release
# the binary is target/release/ferrocord(.exe)
```

Build requirements:

| Platform | Needs |
|---|---|
| Windows 10/11 | Rust (MSVC toolchain) and Visual Studio Build Tools. The WebView2 runtime (preinstalled on Windows 11) is used for captchas. |
| macOS 12+ | Rust and Xcode command-line tools. |
| Linux | Rust, a C/C++ compiler, and the `pkg-config`, `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libdbus-1-dev`, `libxkbcommon-dev` and `libssl-dev` packages. |

The release binary is self-contained: fonts are embedded, and emoji and avatars
are downloaded on demand and cached.

## Command line

```text
ferrocord                     start normally (auto-login if a session is saved)
ferrocord --demo              explore the UI offline with sample data
ferrocord --demo-view VIEW    demo mode opened on: settings, emoji, friends, quick, dm
ferrocord --help
```

Set `RUST_LOG` to change log verbosity, for example
`RUST_LOG=ferrocord=debug,ferrocord_gateway=debug`.

## Logging in

Pick one of the tabs on the login screen:

- **Password**: your email or phone number and password.
  - If the account has two-factor authentication, you'll be asked for a code
    from your authenticator app, a code sent by SMS (press *Send SMS code*),
    or a backup code.
  - If Discord asks for a captcha, a small **Verification** window opens. Solve
    it and the login continues automatically. Closing the window cancels it.
  - Logging in from a new location may require approving it through a link
    Discord emails you. Approve it, then press *Log In* again.
- **QR Code**: open the Discord mobile app → *Settings* → *Scan QR Code* and
  scan it. Confirm on your phone. The code refreshes automatically when it
  expires. This is the recommended method for accounts protected by security
  keys.
- **Token**: paste an existing user token. Never share your token; anyone with
  it controls your account.

After a successful login, the token is stored in your operating system's
credential vault, and Ferrocord reconnects automatically on next start. *Log
Out* (in Settings) revokes the token server-side and removes it locally.

## Finding your way around

| Area | What it does |
|---|---|
| Server rail (far left) | Home/DMs button, your servers and folders. A white pill marks unread servers; red badges count mentions. Right-click a server for *Mark as read*, *Mute* and *Leave*. |
| Sidebar | Channel tree (categories collapse when clicked) or, on Home, Friends and your DMs. Right-click a channel for *Mark as read*, *Mute* and *Copy link*. |
| Account panel | Click your name to change status or set a custom status. ⚙ opens Settings. |
| Chat | Hover a message for quick actions (react, reply, edit, delete). Right-click for more (pin, copy text/link/ID). Click avatars or names for profiles. |
| Header | 🔍 search, 📌 pinned messages, 👥 toggle the member list. |

### Composer

- **Enter** sends; **Shift+Enter** inserts a new line.
- **↑** in an empty composer edits your last message.
- Type `@`, `#`, `:` (two or more letters) or `/` for autocomplete. Use
  **↑/↓** to choose and **Tab** or **Enter** to insert.
- Attach files with the **➕** button or by dragging them onto the window.
- Built-in commands: `/shrug`, `/tableflip`, `/unflip`, `/lenny`, `/me`,
  `/spoiler` and `/nick`. Plugins can add more.

### Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+K` | Quick switcher (jump to any channel or DM) |
| `Alt+↑` / `Alt+↓` | Previous / next channel |
| `Esc` | Close popup/modal; otherwise mark the channel read and cancel reply/edit |
| `Ctrl` + `+` / `-` / `0` | Zoom in / out / reset |

## Settings

| Tab | Options |
|---|---|
| My Account | Account overview, user ID |
| Appearance | Theme (Dark / Midnight / Light), accent color, zoom, compact mode, avatars, inline media, emoji animation, 24-hour clock, member list |
| Notifications | Desktop notifications, message content in notifications, sound, unread count in the title bar |
| Privacy | Typing indicators, automatic read marking |
| Plugins | Enable or disable plugins, grant permissions, reload, open the plugins folder |
| Developer API | Enable the local API, choose its port, allow actions, copy or regenerate the token |
| Advanced | Client build, connection state and latency, config folder, clear media cache, reconnect |

Per-server and per-channel notification levels follow your Discord settings,
which sync across devices.

## Files and folders

| Platform | Config (`config.toml`, `api-token.txt`) | Data (`plugins/`) | Cache (`media/`, `webview/`) |
|---|---|---|---|
| Windows | `%APPDATA%\ferrocord\Ferrocord\config` | `%APPDATA%\ferrocord\Ferrocord\data` | `%LOCALAPPDATA%\ferrocord\Ferrocord\cache` |
| macOS | `~/Library/Application Support/dev.ferrocord.Ferrocord` | same | `~/Library/Caches/dev.ferrocord.Ferrocord` |
| Linux | `~/.config/ferrocord` | `~/.local/share/ferrocord` | `~/.cache/ferrocord` |

`config.toml` is plain TOML and is safe to edit while Ferrocord is closed:

```toml
[appearance]
theme = "midnight"
accent = "#eb459e"
zoom = 1.1
compact_messages = true

[notifications]
desktop = true
show_content = false

[api]
enabled = true
port = 6473
allow_actions = false

[plugins]
enabled = ["hello"]
grants = { hello = ["commands", "messages.send"] }
```

Tokens are **never** written to this file.

## Plugins and automation

- Plugins: copy a plugin folder (containing `plugin.toml`) into the plugins
  folder, press *Rescan* in Settings → Plugins, enable it and grant the
  permissions it asks for. The repository ships examples in
  `plugins/examples/`. See [PLUGINS.md](PLUGINS.md).
- Local API: enable it in Settings → Developer API, copy the token, and talk to
  `http://127.0.0.1:6473/v1/...` or `ws://127.0.0.1:6473/v1/ws`. See
  [API.md](API.md).

## Troubleshooting

| Problem | Fix |
|---|---|
| Captcha window is blank (Windows) | Install or repair the Microsoft Edge WebView2 Runtime. |
| "Your session expired…" | The token was revoked (password change or logout elsewhere). Log in again. |
| Stuck on *Connecting…* | Check your connection. *Cancel and log out* returns to the login screen. Run with `RUST_LOG=ferrocord_gateway=debug` for details. |
| Images don't load | Settings → Advanced → *Clear media cache*. |
| Fonts look blurry | Adjust *Zoom* in Appearance. Ferrocord follows the OS DPI scale automatically. |
