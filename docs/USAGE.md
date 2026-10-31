# Using Ferrocord

## Installing

### Prebuilt packages

Each [release](https://github.com/ferrocord/ferrocord/releases) has packages
for Windows, Linux and macOS (`X` is the version):

| File | How to install |
|---|---|
| `ferrocord-X-windows-x86_64.msi`, `…-arm64.msi` | Double-click it. **Install** sets Ferrocord up for you alone (no administrator rights) in `%LOCALAPPDATA%\Programs\Ferrocord`; **Advanced** lets you install for all users in `%ProgramFiles%\Ferrocord` and choose the shortcuts. Uninstall from *Settings > Apps*. Newer versions upgrade in place. |
| `ferrocord-X-windows-<arch>.zip` | Portable: unzip anywhere and run `ferrocord.exe`. Create an empty file named `portable` next to it to keep all data in a `data` folder beside the exe (for USB sticks). |
| `ferrocord_X-1_amd64.deb`, `…_arm64.deb` | `sudo apt install ./ferrocord_X-1_amd64.deb` (Debian 12+, Ubuntu 22.04+). |
| `ferrocord-X-1.x86_64.rpm`, `….aarch64.rpm` | `sudo dnf install ./ferrocord-X-1.x86_64.rpm` (Fedora) or `sudo zypper install ./…rpm` (openSUSE). |
| `ferrocord-X-linux-<arch>.AppImage` | `chmod +x ferrocord-*.AppImage && ./ferrocord-*.AppImage`. Bundles GTK and WebKitGTK; needs glibc 2.35+ and the usual graphics libraries. |
| `ferrocord-X-linux-<arch>.tar.gz` | Unpack and run `ferrocord`. Needs the WebKitGTK 4.1, GTK 3 and ALSA runtime libraries. Copy `share/` to `~/.local/share/` for a menu entry and icon. |
| `ferrocord-X-macos-arm64.dmg` | Apple Silicon Macs (M1 and later), macOS 12+. Open it and drag *Ferrocord* onto *Applications*. |
| `ferrocord-X-macos-x86_64.dmg` | Intel Macs, macOS 12+. Same as above. |

With a package manager:

```sh
winget install Ferrocord.Ferrocord                  # Windows (add --scope machine for all users)
brew install --cask ferrocord/tap/ferrocord         # macOS
```

Unattended Windows installs use the standard `msiexec` switches:
`msiexec /i ferrocord-X-windows-x86_64.msi /qn` (per-user), add `ALLUSERS=1` for
all users or `DESKTOPSHORTCUT=0` to skip the desktop shortcut.

Notes:

- **WebView2 (Windows).** Captchas during login use the Microsoft Edge
  WebView2 runtime. It ships with Windows 11 and current Windows 10; if it is
  missing, the installer offers to open the download page. Everything else works
  without it.
- **Unsigned builds.** Until releases are code-signed, Windows SmartScreen may
  show *Windows protected your PC*: choose *More info > Run anyway*. On macOS the
  first launch is blocked: open *System Settings > Privacy & Security*, scroll to
  the message about Ferrocord and click *Open Anyway* (or run
  `xattr -dr com.apple.quarantine /Applications/Ferrocord.app`).
- **Verifying downloads.** Check the file against `SHA256SUMS` from the same
  release (`sha256sum -c SHA256SUMS --ignore-missing`), or verify its build
  provenance with `gh attestation verify <file> --repo ferrocord/ferrocord`.
- **Uninstalling** keeps your settings, plugins and cache (see
  [Where Ferrocord keeps its files](#where-ferrocord-keeps-its-files)) and the
  saved login, so a reinstall picks up where you left off. Run
  `ferrocord --remove-user-data` first for a clean slate.

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
| Linux | Rust, a C/C++ compiler, and the `pkg-config`, `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libdbus-1-dev`, `libxkbcommon-dev` and `libssl-dev` packages (on Debian/Ubuntu, `sudo packaging/linux/install-build-deps.sh` installs everything CI uses, including the voice/camera libraries). |

The release binary is self-contained: fonts are embedded, and emoji and avatars
are downloaded on demand and cached.

## Command line

```text
ferrocord                     start normally (auto-login if a session is saved)
ferrocord --demo              explore the UI offline with sample data
ferrocord --demo-view VIEW    demo mode opened on: settings, emoji, friends, quick, dm, call, voice-settings
ferrocord --paths             print the folders Ferrocord stores files in
ferrocord --remove-user-data  sign out and delete all settings, plugins, caches and logs
ferrocord --help
```

Set `RUST_LOG` to change log verbosity, for example
`RUST_LOG=ferrocord=debug,ferrocord_gateway=debug`. Set `FERROCORD_HOME` to keep
every file under one folder.

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
| Advanced | Client build, connection state and latency, links to the settings and logs folders, clear media cache, reconnect |

Per-server and per-channel notification levels follow your Discord settings,
which sync across devices.

## Where Ferrocord keeps its files

Ferrocord follows each system's conventions. `ferrocord --paths` prints the
folders in use, and *Settings > Advanced* links to them.

**Windows**

| What | Where |
|---|---|
| Program (MSI, per-user) | `%LOCALAPPDATA%\Programs\Ferrocord` |
| Program (MSI, all users) | `%ProgramFiles%\Ferrocord` |
| Settings and data: `config.toml`, `api-token.txt`, `plugins\`, `ui-state.ron` | `%APPDATA%\Ferrocord` (roams with your profile) |
| Cache: `media\`, `webview\` | `%LOCALAPPDATA%\Ferrocord\Cache` |
| Logs | `%LOCALAPPDATA%\Ferrocord\Logs` |
| Saved login | Credential Manager, *Windows Credentials*, entry ending in `.dev.ferrocord.client` |

**macOS**

| What | Where |
|---|---|
| Program | `/Applications/Ferrocord.app` |
| Settings and data | `~/Library/Application Support/dev.ferrocord.Ferrocord` |
| Cache | `~/Library/Caches/dev.ferrocord.Ferrocord` |
| Logs | `~/Library/Logs/Ferrocord` (also visible in Console.app) |
| Saved login | Login keychain, service `dev.ferrocord.client` |

**Linux** (XDG base directories; `$XDG_*` variables are honoured)

| What | Where |
|---|---|
| Program | `/usr/bin/ferrocord` (packages), or wherever the AppImage / tarball lives |
| Settings: `config.toml`, `api-token.txt` | `~/.config/ferrocord` |
| Data: `plugins/`, `ui-state.ron` | `~/.local/share/ferrocord` |
| Cache | `~/.cache/ferrocord` |
| Logs | `~/.local/state/ferrocord/logs` |
| Saved login | Secret Service (GNOME Keyring, KWallet), service `dev.ferrocord.client` |

The log folder keeps the current run (`ferrocord.log`) and the two before it
(`ferrocord.1.log`, `ferrocord.2.log`). Attach them to bug reports (Ferrocord does not
write tokens to its logs, but skim them for message text first).

**Portable mode.** If a file named `portable` sits next to the executable, or
`FERROCORD_HOME` is set, everything except the saved login goes into
`config/`, `data/`, `cache/` and `logs/` under that folder (`data\` beside the
exe in the portable case).

**Removing everything.** Uninstalling leaves your data in place.
`ferrocord --remove-user-data` deletes the folders above and the saved login of
the last account.

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
| Something else went wrong | Look at `ferrocord.log` in the logs folder (`ferrocord --paths`). |
| Fonts look blurry | Adjust *Zoom* in Appearance. Ferrocord follows the OS DPI scale automatically. |
