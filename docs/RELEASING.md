# Releasing Ferrocord

This document is for maintainers. It covers how versions work, how to cut a
release, a pre-release or a hotfix, what the pipeline builds, and how to
reproduce it locally.

## Versioning

- **SemVer.** One version for the whole workspace: `[workspace.package] version`
  in the root `Cargo.toml`. Every crate inherits it (`version.workspace = true`).
  That field is the only place a version is written by hand, and normally
  release-plz writes it.
- **One tag per release:** `vX.Y.Z` (or `vX.Y.Z-rc.N` for a pre-release). Crates
  are not published to crates.io.
- **Conventional Commits.** PRs are squash-merged and the PR title becomes the
  commit message, so the title must follow
  [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
  (`feat(chat): add forum post list`, `fix: ...`, `feat!: ...` for breaking
  changes). The *PR title* workflow checks this. The commit type sets the bump:

  | Commit | Before 1.0 | From 1.0 |
  |---|---|---|
  | `fix:`, `perf:`, others | patch | patch |
  | `feat:` | minor (`features_always_increment_minor`) | minor |
  | `feat!:` / `BREAKING CHANGE:` footer | minor | major |

### Where the version ends up

| Place | Source |
|---|---|
| `ferrocord --version`, `--help` | `env!("CARGO_PKG_VERSION")` at compile time |
| Windows `VERSIONINFO` (Explorer > Properties > Details) | `crates/app/build.rs` (winresource): `ProductVersion`/`FileVersion` strings = full version, numeric file version = `X.Y.Z.0` |
| NSIS installer, Add/Remove Programs `DisplayVersion` | `packaging/windows/package.sh` passes it to `makensis` |
| `.deb` | cargo-deb: `X.Y.Z-1`; pre-releases become `X.Y.Z~rc.N-1` so they sort before the final release |
| `.rpm` | cargo-generate-rpm: `X.Y.Z-1`; pre-releases become `X.Y.Z~rc.N-1` |
| AppStream `<release>` | `packaging/linux/stamp-metainfo.py`, run by the Release workflow before packaging |
| macOS `Info.plist` | `CFBundleShortVersionString`/`CFBundleVersion` = `X.Y.Z` (numeric only), full version in `CFBundleGetInfoString` |
| Asset file names | `ferrocord-<version>-<os>-<arch>.<ext>` |

The Release workflow refuses to run if the tag is not exactly `v` + the workspace version.

## Tools and workflows

| File | Role |
|---|---|
| `release-plz.toml` | [release-plz](https://release-plz.dev): computes the next version from commits since the last `v*` tag (`git_only = true`, no registry), bumps the workspace version, writes `CHANGELOG.md`, and pushes the tag when the release PR is merged. All crates are in one `version_group`; CI fails if a new crate is missing from it. |
| `cliff.toml` | [git-cliff](https://git-cliff.org) template, Keep a Changelog style. Used by release-plz for `CHANGELOG.md` and by `.github/scripts/release-notes.sh` for the GitHub release notes. |
| `.github/workflows/ci.yml` | fmt, clippy `-D warnings`, tests on Linux/Windows/macOS, MSRV, rustdoc `-D warnings`, cargo-deny, actionlint + shellcheck, desktop/AppStream validation. Also called by the Release workflow. |
| `.github/workflows/release-plz.yml` | On every push to `main`: open/update the release PR; after it is merged: tag. |
| `.github/workflows/release.yml` | On a `v*.*.*` tag: verify, CI, build, package, smoke-test, checksums, provenance, GitHub release. Manual runs and a weekly run are dry runs. |
| `.github/workflows/pr-title.yml` | Enforces Conventional Commit PR titles. |
| `deny.toml` | cargo-deny policy (licenses, advisories, bans, sources). |

## Cutting a release

1. Merge PRs into `main` as usual. After each push, release-plz updates a PR
   titled `chore(release): vX.Y.Z` that bumps the version and prepends the
   new section to `CHANGELOG.md`.
2. Review that PR. Edit the changelog text in the PR if needed (reword, drop
   noise, add highlights); push edits to the PR branch. release-plz rewrites the
   PR if more commits land on `main`, so do the final edit just before merging.
3. Merge the release PR. The *Release PR* workflow pushes the tag `vX.Y.Z`.
4. The *Release* workflow runs (about 45-60 minutes):
   verify version → CI suite → build 6 targets → package → smoke-test every
   artifact → `SHA256SUMS` + build provenance → **draft** release with all
   assets → checks the asset count → publishes the release.
5. Check the release page. Announce.

If any stage fails, nothing is published (or the release stays a draft). Fix the
problem on `main`, then either cut a new patch release, or delete the tag and
re-tag the fixed commit (only if the release was never published).

### First release (0.1.0)

There is no `v*` tag yet, so release-plz treats the current version (0.1.0) as
an initial release: its first PR does not bump the version and lists all
history under 0.1.0. The hand-written summary that is currently under
`[Unreleased]` in `CHANGELOG.md` ends up below the generated list; trim the
generated "Other" entries (history before Conventional Commits) in the PR, then
merge it.

## Pre-releases (`v0.2.0-rc.1`)

release-plz only proposes final versions, so pre-releases are tagged by hand:

```sh
git switch main && git pull
# set [workspace.package] version = "0.2.0-rc.1" in Cargo.toml
cargo update --workspace          # refresh Cargo.lock
git commit -am "chore(release): v0.2.0-rc.1"   # via a PR if main is protected
git tag -a v0.2.0-rc.1 -m "Ferrocord 0.2.0-rc.1"
git push origin main v0.2.0-rc.1
```

Any version with a `-` suffix is published as a GitHub **pre-release** and is
not marked "latest". Its notes cover the commits since the previous tag. When
the final `v0.2.0` ships (merge the release PR, or tag by hand), its notes
cover everything since `v0.1.0`, including the release candidates.

## Hotfixes

For a fix on top of an already released version while `main` has moved on:

```sh
git switch -c release/0.2.x v0.2.0
git cherry-pick <fix-commit>           # or commit the fix here and forward-port it to main
# bump [workspace.package] version to 0.2.1 in Cargo.toml, then:
cargo update --workspace
git commit -am "chore(release): v0.2.1"
git tag -a v0.2.1 -m "Ferrocord 0.2.1"
git push origin release/0.2.x v0.2.1
```

Add a `## [0.2.1]` section to `CHANGELOG.md` on `main` afterwards (copy it from
the release notes). If the hotfix is also the newest release, it is marked
"latest" automatically.

## Re-running, manual runs and dry runs

- **Dry run**: *Actions > Release > Run workflow* on any branch. Everything is
  built, packaged and smoke-tested; the artifacts are attached to the run; no
  release is created. A dry run of `main` also runs weekly (Monday 04:23 UTC)
  so packaging problems surface before release day.
- **Publish an existing tag manually**: *Run workflow*, choose the **tag** under
  "Use workflow from" and untick *dry-run*.
- **Token note**: tags pushed with the default `GITHUB_TOKEN` do not trigger
  other workflows. If the `RELEASE_PLZ_TOKEN` secret is not set, the Release PR
  workflow therefore dispatches the Release workflow for the new tag itself.

## Artifacts

| Platform | Built on | Files |
|---|---|---|
| Windows x86_64 | `windows-latest` | `ferrocord-X-windows-x86_64.zip`, `ferrocord-X-windows-x86_64-setup.exe` |
| Windows ARM64 | `windows-11-arm` (native) | `ferrocord-X-windows-arm64.zip`, `ferrocord-X-windows-arm64-setup.exe` |
| Linux x86_64 | `ubuntu-22.04` (glibc 2.35) | `ferrocord-X-linux-x86_64.tar.gz`, `ferrocord_X-1_amd64.deb`, `ferrocord-X-1.x86_64.rpm`, `ferrocord-X-linux-x86_64.AppImage` |
| Linux aarch64 | `ubuntu-22.04-arm` | `...-linux-aarch64.tar.gz`, `..._arm64.deb`, `...aarch64.rpm`, `...-linux-aarch64.AppImage` |
| macOS (universal) | `macos-15` (x86_64 cross-compiled) | `ferrocord-X-macos-universal.dmg`, `ferrocord-X-macos-universal.tar.gz` |
| All | | `SHA256SUMS`, plus a signed SLSA build-provenance attestation per file |

Users can verify a download with `gh attestation verify <file> --repo ferrocord/ferrocord`
and `sha256sum -c SHA256SUMS --ignore-missing`.

Binaries are built with the `dist` profile (`release` + fat LTO + one codegen unit).

**Windows installer.** NSIS, per-user (no admin rights, no UAC prompt), installs
to `%LOCALAPPDATA%\Programs\Ferrocord`, creates a Start-menu shortcut (desktop
shortcut optional), registers an uninstaller under *Apps > Installed apps*, and
offers to open the WebView2 download page if the runtime is missing (only needed
for captchas; preinstalled on Windows 11). Silent: `setup.exe /S [/D=dir]`,
uninstall: `uninstall.exe /S`. User data and the saved login are kept on
uninstall. The installer is compiled on Linux with the distribution's `makensis`
for both architectures (the x86 installer stub runs natively on x64 and emulated
on ARM64) and is smoke-tested on real x64 and ARM64 Windows runners.

**Smoke tests** (all run before anything is published):

- `.deb`: installed with `apt` in clean `ubuntu:22.04`, `ubuntu:24.04`, `debian:12`
  containers (x86_64 and aarch64), `ferrocord --version`, desktop files present, removed.
- `.rpm`: installed with `dnf` in `fedora:latest` and `zypper` in `opensuse/tumbleweed`.
- AppImage: run with `--appimage-extract-and-run --version` in `ubuntu:22.04` and
  `fedora:latest` containers with only the base graphics/font libraries; checks
  that the WebKit helper processes are bundled.
- `.tar.gz`: extracted and run in `ubuntu:24.04` with the runtime libraries.
- Windows: zip extracted, version resource and icon checked, `--version`;
  installer run silently, files/shortcut/registry checked, installed exe run,
  silent uninstall verified clean.
- macOS: dmg verified and mounted, signature verified, universal binary run
  natively and under Rosetta, tarball run.

`--version` exits before any window or WebView is created, so no display is needed.

## Repository setup (one-time)

1. **Secrets**: `RELEASE_PLZ_TOKEN` (recommended) - a fine-grained PAT or
   GitHub App token for this repository with *Contents* and *Pull requests*
   read/write. Without it, CI does not run on the release PR (GitHub does not
   trigger workflows for `GITHUB_TOKEN` events).
2. **Environment** `release` (Settings > Environments): optionally add required
   reviewers; the publish job waits for approval.
3. **Branch protection** on `main`: require the `CI result` check and the
   `Conventional Commit title` check; allow squash merging only.
4. **Labels**: create a `release` label (release-plz adds it to release PRs).
5. Actions settings: allow GitHub Actions to create pull requests.

## Code signing and notarization (not configured yet)

Releases are currently **unsigned** (Windows SmartScreen warns; macOS Gatekeeper
blocks the app until the user right-clicks > Open or runs
`xattr -dr com.apple.quarantine /Applications/Ferrocord.app`). Integrity is
covered by `SHA256SUMS` and the provenance attestations. To add signing:

- **Windows** (Authenticode): sign `ferrocord.exe` in the `build` job right
  after "Stage binary" (before upload), and the `*-setup.exe` + `uninstall.exe`
  after packaging. Options: Azure Trusted Signing (`azure/trusted-signing-action`,
  secrets `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, account and
  profile names), or an OV/EV certificate with `signtool` (`WINDOWS_CERT_PFX`
  base64 + `WINDOWS_CERT_PASSWORD`). Signing the NSIS uninstaller requires
  `!uninstfinalize` in `installer.nsi`; since makensis runs on Linux, either
  sign with `osslsigncode`/`jsign` there or move the installer job to Windows.
- **macOS**: import a *Developer ID Application* certificate
  (`MACOS_CERT_P12` base64, `MACOS_CERT_PASSWORD`) into a temporary keychain in
  `package-macos`, set `MACOS_SIGN_IDENTITY` (package.sh then signs with the
  hardened runtime and a timestamp), sign the `.dmg`, then
  `xcrun notarytool submit --wait` (App Store Connect API key:
  `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8`) and
  `xcrun stapler staple` the app and the dmg.
- **Linux**: optionally GPG-sign `SHA256SUMS` (`SHA256SUMS.asc`), the `.rpm`
  (`cargo generate-rpm --signing-key`) and publish an APT/RPM repository;
  AppImages can embed a signature with `appimagetool --sign`.

## Building packages locally

```sh
cargo build --locked --profile dist --target x86_64-unknown-linux-gnu -p ferrocord
cargo install --locked cargo-deb cargo-generate-rpm
sudo packaging/linux/install-build-deps.sh --package
bash packaging/linux/package.sh x86_64-unknown-linux-gnu "$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[]|select(.name=="ferrocord").version')" dist
docker run --rm -v "$PWD:/w" -w /w fedora:latest bash packaging/linux/smoke-test.sh rpm dist/ferrocord-*.rpm 0.1.0
```

Windows zip + installer (any OS with `makensis` and `zip`):
`bash packaging/windows/package.sh target/x86_64-pc-windows-msvc/dist/ferrocord.exe 0.1.0 x86_64 dist`,
then on Windows `./packaging/windows/smoke-test.ps1 -Zip ... -Installer ... -Version 0.1.0`
(it installs into a temporary directory and uninstalls again).

Release notes for a tag: `.github/scripts/release-notes.sh v0.2.0`.
Preview the next release PR: `release-plz update` (then `git restore .`).

### Icons

`packaging/icons/ferrocord.svg` is the source; it matches the window icon drawn
by `app_icon()` in `crates/app/src/main.rs`. After editing it, run
`python3 packaging/icons/generate.py` (standard library only) and commit the
regenerated PNGs (hicolor 16-512), `ferrocord.ico` and `ferrocord.icns`.
