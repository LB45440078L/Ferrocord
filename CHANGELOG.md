# Changelog

All notable changes to Ferrocord are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Entries are generated from [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
by [git-cliff](https://git-cliff.org) when a release PR is opened; edit them in that PR.

## [Unreleased]

## [0.1.0](https://github.com/LB45440078L/Ferrocord/releases/tag/v0.1.0) - 2026-11-01

First public version.

### Added

- Login: password, TOTP/SMS/backup-code 2FA, QR-code remote auth, token; captcha via helper WebView.
- Gateway: zlib-stream, heartbeating, resume/reconnect with backoff, lazy guild subscriptions.
- Text chat: history, sending with local echo, edit/delete, replies, reactions, pins, attachments, markdown, embeds, stickers.
- Syntax-highlighted code blocks for about 30 languages, selectable message text, clickable links and a zoomable image viewer.
- Voice and video calls in servers and DMs (Opus audio, H.264 camera video, DAVE end-to-end encryption), with UI sounds and a ringtone.
- Servers, folders, channels, threads, forum posts, DMs and group DMs, friends, profiles, presence and custom status.
- Safety tools: block and unblock, local ignore list, blurred media from strangers, confirmation before opening outside links.
- Search, quick switcher, unread/mention tracking, notification settings, desktop notifications.
- Sandboxed Luau plugins with permissions and limits; local HTTP/WebSocket API.
- Themes (Dark, Midnight, Light), accent colors, zoom, compact mode; offline demo mode.
- Installers: Windows MSI (per-user or all users) and portable zip for x64 and ARM64, macOS disk images for
  Apple Silicon and Intel, Linux `.deb`, `.rpm`, AppImage and `.tar.gz`; one-command build scripts for each OS.
- Platform-native data, cache and log folders, plus a portable mode.
