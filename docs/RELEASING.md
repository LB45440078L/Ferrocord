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
| MSI `ProductVersion` (Apps & Features) | `packaging/windows/package.ps1` passes `X.Y.Z` to WiX (MSI versions are numeric); the full version is in the package description |
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
| Windows x86_64 | `windows-latest` | `ferrocord-X-windows-x86_64.msi`, `ferrocord-X-windows-x86_64.zip` |
| Windows ARM64 | `windows-11-arm` (native) | `ferrocord-X-windows-arm64.msi`, `ferrocord-X-windows-arm64.zip` |
| Linux x86_64 | `ubuntu-22.04` (glibc 2.35) | `ferrocord-X-linux-x86_64.tar.gz`, `ferrocord_X-1_amd64.deb`, `ferrocord-X-1.x86_64.rpm`, `ferrocord-X-linux-x86_64.AppImage` |
| Linux aarch64 | `ubuntu-22.04-arm` | `...-linux-aarch64.tar.gz`, `..._arm64.deb`, `...aarch64.rpm`, `...-linux-aarch64.AppImage` |
| macOS Apple Silicon | `macos-15` | `ferrocord-X-macos-arm64.dmg` |
| macOS Intel | `macos-15` (cross-compiled, tested under Rosetta 2) | `ferrocord-X-macos-x86_64.dmg` |
| All | | `SHA256SUMS`, plus a signed SLSA build-provenance attestation per file |

The run also keeps two non-release artifacts: `winget-manifests` and `homebrew-cask`
(see [Distribution channels](#distribution-channels)).

Users can verify a download with `gh attestation verify <file> --repo ferrocord/ferrocord`
and `sha256sum -c SHA256SUMS --ignore-missing`.

Binaries are built with the `dist` profile (`release` + fat LTO + one codegen unit).

**Windows installer (MSI).** Built with the [WiX Toolset](https://wixtoolset.org)
5 from `packaging/windows/ferrocord.wxs`, using the WixUI_Advanced dialogs with
Ferrocord artwork:

1. Welcome + license page. **Install** installs for the current user right away;
   **Advanced** opens the remaining pages.
2. Installation scope: *just for you* (`%LOCALAPPDATA%\Programs\Ferrocord`, no
   admin rights, no UAC prompt) or *all users* (`%ProgramFiles%\Ferrocord`,
   elevates). Per-machine installs can pick another folder.
3. Features: Start menu shortcut, desktop shortcut (both on by default).
4. Progress, then a finish page with *Launch Ferrocord*.

The package registers `ferrocord.exe` under *App Paths* (so `Win+R ferrocord`
works), shows the icon and links in *Apps & Features*, remembers the install
folder for upgrades, and offers to close a running copy (Restart Manager).
Installing a newer MSI replaces the old version in place (`MajorUpgrade` keyed on
the fixed `UpgradeCode` in the `.wxs` file, which must never change); installing
an older one is refused. The MSI is checked with the ICE validators
(`wix msi validate`) on every build.

Why MSI instead of an `.exe` installer: it is what Windows administrators deploy
(Intune, Group Policy, SCCM, `msiexec` with standard switches), winget handles it
natively, upgrades and repairs are transactional and roll back on failure, and the
same package covers per-user and per-machine installs.

Command line:

```bat
msiexec /i ferrocord-X-windows-x86_64.msi /qn                     :: per-user, silent
msiexec /i ferrocord-X-windows-x86_64.msi /qn ALLUSERS=1          :: all users (admin)
msiexec /i ferrocord-X-windows-x86_64.msi /qn DESKTOPSHORTCUT=0   :: no desktop shortcut
msiexec /x ferrocord-X-windows-x86_64.msi /qn                     :: uninstall
```

Uninstalling keeps settings, plugins, caches and the saved login (Windows
convention; reinstalling picks them up). `ferrocord --remove-user-data` removes
them; see [USAGE.md](USAGE.md#where-ferrocord-keeps-its-files). The WebView2
runtime is only needed for captchas and ships with Windows 11 and current
Windows 10.

The **zip** is the portable build: create an empty file named `portable` next to
`ferrocord.exe` and all data is kept in a `data` folder beside it.

**macOS disk images.** One `.dmg` per architecture, so each download contains a
single-architecture app (half the size of a universal one). The image is
LZFSE-compressed (`ULFO`, macOS 10.11+) and built with
[dmgbuild](https://dmgbuild.readthedocs.io) from `packaging/macos/dmg-settings.py`:
a 660x400 Finder window with a Retina background (drag the app onto
*Applications*), large icons, no toolbars, and the Ferrocord volume icon. The app
bundle (`dev.ferrocord.Ferrocord`, macOS 12+) declares camera and microphone usage
strings; Developer ID builds use the hardened runtime with the entitlements in
`packaging/macos/Ferrocord.entitlements`.

**Smoke tests** (all run before anything is published):

- `.deb`: installed with `apt` in clean `ubuntu:22.04`, `ubuntu:24.04`, `debian:12`
  containers (x86_64 and aarch64), `ferrocord --version`, desktop files present, removed.
- `.rpm`: installed with `dnf` in `fedora:latest` and `zypper` in `opensuse/tumbleweed`.
- AppImage: run with `--appimage-extract-and-run --version` in `ubuntu:22.04` and
  `fedora:latest` containers with only the base graphics/font libraries; checks
  that the WebKit helper processes are bundled.
- `.tar.gz`: extracted and run in `ubuntu:24.04` with the runtime libraries.
- Windows (x64 and ARM64 runners): zip extracted, version resource and icon
  checked, `--version`, portable mode checked with `--paths`; MSI properties
  checked (version, `UpgradeCode`, dual scope); silent per-user install, files,
  shortcuts, App Paths and Windows Installer registration checked, installed exe
  run, silent uninstall verified clean. When an earlier release exists, its MSI
  is installed first and the new one must upgrade it in place.
- macOS (arm64 natively, x86_64 under Rosetta 2): dmg verified, format `ULFO`,
  mounted; Finder layout, background, volume icon and Applications link present;
  `Info.plist`, resources, signature (and entitlements + Gatekeeper assessment for
  Developer ID builds) and architecture checked; app copied out like a drag
  install and run.

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

## Code signing and notarization

Signing is wired into the workflow and switches on when the secrets exist.
Without them, releases are built **unsigned** (Windows SmartScreen warns on the
first run; macOS refuses to open the app until the user approves it in *System
Settings > Privacy & Security > Open Anyway*). Integrity is always covered by
`SHA256SUMS` and the provenance attestations.

| Secret | Used by | Purpose |
|---|---|---|
| `WINDOWS_SIGN_PFX_BASE64` | `package-windows` | Authenticode certificate (`.pfx`, base64). `ferrocord.exe` is signed before it goes into the zip and MSI, then the `.msi` itself; both are timestamped. |
| `WINDOWS_SIGN_PASSWORD` | `package-windows` | Its password. |
| `MACOS_SIGN_P12_BASE64` | `package-macos` | *Developer ID Application* certificate + key (`.p12`, base64), imported into a throwaway keychain. |
| `MACOS_SIGN_P12_PASSWORD` | `package-macos` | Its password. |
| `MACOS_SIGN_IDENTITY` | `package-macos` | e.g. `Developer ID Application: Name (TEAMID)`. The app is signed with the hardened runtime and entitlements, then the `.dmg`. |
| `APPLE_NOTARY_KEY_BASE64` | `package-macos` | App Store Connect API key (`.p8`, base64) for `notarytool`. With it, the `.dmg` is notarized and the ticket stapled. |
| `APPLE_NOTARY_KEY_ID`, `APPLE_NOTARY_ISSUER` | `package-macos` | The key's id and issuer id. |

For Windows, a cloud signing service (Azure Trusted Signing, SignPath for open
source projects) avoids handling a `.pfx`: replace `Invoke-Sign` in
`packaging/windows/package.ps1` with the service's tool. Linux: optionally
GPG-sign `SHA256SUMS`, the `.rpm` (`cargo generate-rpm --signing-key`) and the
AppImage (`appimagetool --sign`).

## Distribution channels

Besides the GitHub release, two package managers are fed after a stable release
is published (pre-releases are skipped). Both jobs render their files on every
run, so dry runs show exactly what would be submitted, and only submit when their
token exists.

**winget** (`publish-winget`). `packaging/winget/render.ps1` writes the
`Ferrocord.Ferrocord` manifests: each MSI is listed for the `user` and `machine`
scopes, with its `ProductCode` and the `UpgradeCode`, so `winget upgrade` and
`winget uninstall` find existing installs. The `winget-manifests` job validates
them with `winget validate`. With the `WINGET_TOKEN` secret (a classic PAT with
`public_repo`, from an account that has forked
[winget-pkgs](https://github.com/microsoft/winget-pkgs)), `wingetcreate submit`
opens the pull request. The very first submission is reviewed by hand by the
winget maintainers; later ones are usually merged automatically.

```powershell
winget install Ferrocord.Ferrocord                    # per-user
winget install Ferrocord.Ferrocord --scope machine    # all users
```

**Homebrew** (`publish-homebrew`). `packaging/homebrew/render-cask.py` fills
`ferrocord.rb.in` with the version and both DMG checksums (`arch arm:/intel:`).
The cask installs the app, links `ferrocord` into the `PATH`, and its `zap` stanza
lists every folder Ferrocord writes to. Unnotarized builds get a `caveats` note on
approving the first launch. With the `HOMEBREW_TAP_TOKEN` secret (write access to
the tap repository), the cask is committed to `Casks/ferrocord.rb` in
`<owner>/homebrew-tap` (override with the `HOMEBREW_TAP` repository variable).

```sh
brew install --cask ferrocord/tap/ferrocord
```

Submitting to the main `homebrew/cask` repository requires notarized builds and
some popularity; the tap works from the first release.

Linux packages are attached to the release (`.deb`, `.rpm`, AppImage). Flathub
and APT/RPM repositories are possible later; `dev.ferrocord.Ferrocord.metainfo.xml`
is already AppStream-valid.

## Building packages locally

The easy way: one script per platform checks every tool, offers to install what
is missing, builds, and writes the packages to `dist/`.

| On | Run | Produces |
|---|---|---|
| Windows 10/11 | double-click `scripts\build-windows.cmd` (or `.\scripts\build-windows.ps1`) | `.msi` + `.zip` for x64 and ARM64, then the Linux packages through WSL2 |
| Linux (Debian/Ubuntu) or WSL2 | `bash scripts/build-linux.sh` | `.deb`, `.rpm`, `.AppImage`, `.tar.gz` for this machine's architecture |
| macOS (Apple Silicon) | `bash scripts/build-macos.sh` | `ferrocord-X-macos-arm64.dmg` (`--intel` adds the x86_64 image) |

Useful options: `-Yes`/`--yes` installs without asking, `-Quick`/`--quick` uses
the faster `release` profile, and `-Arch x64`, `-SkipLinux` and `-LinuxOnly`
narrow the Windows run. Under WSL the sources are copied into the Linux
filesystem (`~/.cache/ferrocord-build`) because building on `/mnt/c` is very
slow. macOS disk images can only be made on a Mac.

The manual steps below are what those scripts run.


The easy way: one script per platform checks every tool, offers to install what
is missing, builds, and writes the packages to `dist/`.

| On | Run | Produces |
|---|---|---|
| Windows 10/11 | double-click `scriptsuild-windows.cmd` (or `.\scriptsuild-windows.ps1`) | `.msi` + `.zip` for x64 and ARM64, then the Linux packages through WSL2 |
| Linux (Debian/Ubuntu) or WSL2 | `bash scripts/build-linux.sh` | `.deb`, `.rpm`, `.AppImage`, `.tar.gz` for this machine's architecture |
| macOS (Apple Silicon) | `bash scripts/build-macos.sh` | `ferrocord-X-macos-arm64.dmg` (`--intel` adds the x86_64 image) |

Useful options: `-Yes`/`--yes` installs without asking, `-Quick`/`--quick` uses
the faster `release` profile, `-Arch x64`, `-SkipLinux` and `-LinuxOnly` narrow
the Windows run. Under WSL the sources are copied into the Linux filesystem
(`~/.cache/ferrocord-build`) because building on `/mnt/c` is very slow. macOS
disk images can only be made on a Mac.

The manual steps below are what those scripts run.


```sh
cargo build --locked --profile dist --target x86_64-unknown-linux-gnu -p ferrocord
cargo install --locked cargo-deb cargo-generate-rpm
sudo packaging/linux/install-build-deps.sh --package
bash packaging/linux/package.sh x86_64-unknown-linux-gnu "$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[]|select(.name=="ferrocord").version')" dist
docker run --rm -v "$PWD:/w" -w /w fedora:latest bash packaging/linux/smoke-test.sh rpm dist/ferrocord-*.rpm 0.1.0
```

Windows zip + MSI (on Windows, with the .NET SDK):

```powershell
dotnet tool install --global wix --version 5.0.2
wix extension add --global WixToolset.UI.wixext/5.0.2 WixToolset.Util.wixext/5.0.2
cargo build --locked --profile dist -p ferrocord
./packaging/windows/package.ps1 -Exe target/dist/ferrocord.exe -Version 0.1.0 -Arch x86_64 -OutDir dist
./packaging/windows/smoke-test.ps1 -Zip dist/ferrocord-0.1.0-windows-x86_64.zip `
    -Msi dist/ferrocord-0.1.0-windows-x86_64.msi -Version 0.1.0
```

The smoke test really installs Ferrocord for the current user and uninstalls it
again; it refuses to run if Ferrocord is already installed.

macOS DMG (on a Mac, with `pipx install dmgbuild`):

```sh
cargo build --locked --profile dist --target aarch64-apple-darwin -p ferrocord
bash packaging/macos/package.sh 0.1.0 arm64 target/aarch64-apple-darwin/dist/ferrocord dist
bash packaging/macos/smoke-test.sh dist/ferrocord-0.1.0-macos-arm64.dmg 0.1.0 arm64
```

Release notes for a tag: `.github/scripts/release-notes.sh v0.2.0`.
Preview the next release PR: `release-plz update` (then `git restore .`).

### Icons

`packaging/icons/ferrocord.svg` is the source; it matches the window icon drawn
by `app_icon()` in `crates/app/src/main.rs`. After editing it, run
`python3 packaging/icons/generate.py` (standard library only) and commit the
regenerated PNGs (hicolor 16-512), `ferrocord.ico` and `ferrocord.icns`.

### Installer artwork

`python3 packaging/installer-art/generate.py` (needs Pillow) renders the WiX
banner (493x58) and dialog (493x312) bitmaps in `packaging/windows/wix/` and the
DMG background (`packaging/macos/dmg/background.png` + `@2x`) from the icon and
the bundled Inter font. Run it after changing the icon or the installer text, and
keep the icon positions in `packaging/macos/dmg-settings.py` in line with the
arrow in the background.
