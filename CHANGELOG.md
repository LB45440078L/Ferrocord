# Changelog

All notable changes to Ferrocord are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Entries are generated from [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
by [git-cliff](https://git-cliff.org) when a release PR is opened; edit them in that PR.

## [Unreleased]

First public version.

### Added

- Login: password, TOTP/SMS/backup-code 2FA, QR-code remote auth, token; captcha via helper WebView.
- Gateway: zlib-stream, heartbeating, resume/reconnect with backoff, lazy guild subscriptions.
- Text chat: history, sending with local echo, edit/delete, replies, reactions, pins, attachments, markdown, embeds, stickers.
- Servers, folders, channels, threads, forum posts, DMs and group DMs, friends, profiles, presence and custom status.
- Search, quick switcher, unread/mention tracking, notification settings, desktop notifications.
- Sandboxed Luau plugins with permissions and limits; local HTTP/WebSocket API.
- Themes (Dark, Midnight, Light), accent colors, zoom, compact mode; offline demo mode.
