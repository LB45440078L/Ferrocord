# Contributing to Ferrocord

Thanks for helping! This document covers how to build, test and submit changes.

## Getting started

```sh
git clone https://github.com/LB45440078L/Ferrocord
cd ferrocord
cargo run -- --demo      # UI with offline sample data, no account needed
cargo test --workspace
```

See [docs/USAGE.md](docs/USAGE.md#installing) for platform build
prerequisites and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for a tour of
the crates.

## Before you open a pull request

All of these must pass (CI runs them on Windows, macOS and Linux):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Keep changes focused; one feature or fix per PR.
- Add tests for parsing, state and protocol logic. Store and markdown changes
  are easy to unit test. UI changes should include a screenshot (use
  `--demo` / `--demo-view`).
- Match the surrounding style: doc comments on modules and public items,
  descriptive names, few inline comments (explain *why*, not *what*).
- Don't add dependencies lightly. Prefer what the workspace already uses, and
  pin versions in `[workspace.dependencies]`.
- Never panic on network data. Unknown or malformed payloads must degrade
  gracefully (see how `Event::parse` falls back to `Event::Other`).

## Protocol changes

Discord changes its client regularly. When you implement or fix
protocol behavior:

1. Record what you observed in [docs/PROTOCOL.md](docs/PROTOCOL.md): the
   endpoint or opcode, the payload shape, and the client build you looked at.
2. Do **not** commit Discord's proprietary code or assets (JavaScript bundles,
   extracted `.asar` files, binaries, decompiler output). Describe facts
   needed for interoperability in your own words.
3. Use throwaway or test accounts for live testing. Never commit tokens, and
   never paste them into issues or logs.

The live QR handshake check is a quick way to detect drift:

```sh
cargo run -p ferrocord-auth --example qr_probe
```

## Commit messages and PR titles

We use [Conventional Commits](https://www.conventionalcommits.org/): release
versions and the changelog are generated from them (see
[docs/RELEASING.md](docs/RELEASING.md)), and CI checks PR titles.

```text
feat(voice): add per-user volume
fix(chat): keep history visible after sending
docs: explain the DAVE handshake
```

Types: `feat` (minor bump), `fix`/`perf` (patch), `docs`, `refactor`, `test`,
`ci`, `build`, `chore`. Breaking changes use `feat!:` or a `BREAKING CHANGE:`
footer. Use the imperative mood and reference issues with `#123`.

## Plugins

Plugin authors don't need to touch this repository. See
[docs/PLUGINS.md](docs/PLUGINS.md). Example plugins in `plugins/examples/`
are welcome if they demonstrate an API feature.

## Code of conduct

Be respectful and constructive. Harassment or discrimination of any kind is
not tolerated. Maintainers may remove comments or ban participants who don't
follow this.

## License

By contributing, you agree that your contributions are licensed under the
[BSD 3-Clause License](LICENSE).
