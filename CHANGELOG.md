# Changelog

All notable changes to Ferrocord are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Entries are generated from [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
by [git-cliff](https://git-cliff.org) when a release PR is opened; edit them in that PR.

## [Unreleased]

## [0.1.0](https://github.com/LB45440078L/Ferrocord/releases/tag/v0.1.0) - 2026-10-31

### Added

- **app:** Platform-native data folders, log files and portable mode ([f45761f](https://github.com/LB45440078L/Ferrocord/commit/f45761f1aee1f825f6d4d8ec35ffa0ed2ebb5737))
- Chat interaction fixes, code highlighting, sounds and safety tools ([5baff70](https://github.com/LB45440078L/Ferrocord/commit/5baff703d5c07770999b4e43cc653695f64c2a3c))

### Fixed

- **ci:** Declare the MSRV on every crate and de-flake heartbeat tests ([1fd5279](https://github.com/LB45440078L/Ferrocord/commit/1fd52796f65d8c391e5bc1b27efe5a15f4b9286d))

### Other

- Core protocol crates: model, http, gateway, auth, store ([7afe54b](https://github.com/LB45440078L/Ferrocord/commit/7afe54bbd8ce05418623eedc102f40f47362cdf9))
- Core client and desktop UI ([c524265](https://github.com/LB45440078L/Ferrocord/commit/c52426590c5a7491e9810dc05b6d8276223c5350))
- Plugins, local API, forums, search, docs and CI ([aca0a7d](https://github.com/LB45440078L/Ferrocord/commit/aca0a7d63bb9263be2db2002cdb075740e2ae194))
- Fix chat history going blank after sending a message ([0eccf67](https://github.com/LB45440078L/Ferrocord/commit/0eccf6709b511a455f0aceeab2b2e40eac9b6af6))
- Add test injection points and --version ([9615d14](https://github.com/LB45440078L/Ferrocord/commit/9615d147f1ce254492bcbaa69a434a9602c0d297))
- Replace glyph icons with a vector icon set; fix composer alignment ([bc003dc](https://github.com/LB45440078L/Ferrocord/commit/bc003dc7f02f8bbad90cee492ba574948be61b2e))
- Voice UI: call view, connection panel, DM calls, voice settings ([44a3278](https://github.com/LB45440078L/Ferrocord/commit/44a32786cc336a89bcf90dc21b13806ed67bed7b))
- Add camera video to calls (H.264 over RTP with DAVE E2EE) ([bd56d17](https://github.com/LB45440078L/Ferrocord/commit/bd56d17f77bf99eb3a3f830151cbf4b66f76a65c))
- Fix account panel and message row alignment ([24f7edb](https://github.com/LB45440078L/Ferrocord/commit/24f7edba2bf3fa63bfc2285cc8b98c3324339cf0))


First public version.

### Added

- Login: password, TOTP/SMS/backup-code 2FA, QR-code remote auth, token; captcha via helper WebView.
- Gateway: zlib-stream, heartbeating, resume/reconnect with backoff, lazy guild subscriptions.
- Text chat: history, sending with local echo, edit/delete, replies, reactions, pins, attachments, markdown, embeds, stickers.
- Servers, folders, channels, threads, forum posts, DMs and group DMs, friends, profiles, presence and custom status.
- Search, quick switcher, unread/mention tracking, notification settings, desktop notifications.
- Sandboxed Luau plugins with permissions and limits; local HTTP/WebSocket API.
- Themes (Dark, Midnight, Light), accent colors, zoom, compact mode; offline demo mode.
