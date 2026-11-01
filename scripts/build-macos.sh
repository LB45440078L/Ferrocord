#!/usr/bin/env bash
# Build the Ferrocord macOS disk image(s) on this Mac.
#
#   bash scripts/build-macos.sh [options]
#
# Checks what is missing, asks before installing anything, builds, and puts the
# .dmg in ./dist. Nothing is installed system-wide except Apple's command line
# tools and Rust (in your home folder); dmgbuild and CMake go into a private
# Python environment under ~/.cache/ferrocord-build.
#
# Options:
#   -y, --yes        install missing tools without asking
#   --intel          also build the Intel (x86_64) disk image
#   --quick          use the faster `release` profile instead of `dist` (fat LTO)
#   --out DIR        where to put the disk images (default: <repo>/dist)
#   -h, --help       show this help
#
# Signing: set MACOS_SIGN_IDENTITY="Developer ID Application: …" to sign with your
# certificate (see docs/RELEASING.md); otherwise the app is ad-hoc signed.
set -uo pipefail

if [[ -t 1 && -z ${NO_COLOR:-} ]]; then
  BOLD=$'\e[1m' DIM=$'\e[2m' RED=$'\e[31m' GREEN=$'\e[32m' YELLOW=$'\e[33m' BLUE=$'\e[34m' CYAN=$'\e[36m' RESET=$'\e[0m'
else
  BOLD='' DIM='' RED='' GREEN='' YELLOW='' BLUE='' CYAN='' RESET=''
fi
title() { printf '\n%s%s━━ %s ━━%s\n' "$BOLD" "$CYAN" "$1" "$RESET"; }
step() { printf '%s👉 %s%s\n' "$BLUE" "$1" "$RESET"; }
ok() { printf '%s✅ %s%s\n' "$GREEN" "$1" "$RESET"; }
warn() { printf '%s⚠️  %s%s\n' "$YELLOW" "$1" "$RESET"; }
fail() { printf '%s❌ %s%s\n' "$RED" "$1" "$RESET"; }
die() { fail "$1"; exit 1; }
hint() { printf '%s   %s%s\n' "$DIM" "$1" "$RESET"; }
usage() { sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

YES=0 PROFILE=dist OUT='' ARCHS=(arm64)
while [[ $# -gt 0 ]]; do
  case $1 in
    -y | --yes) YES=1 ;;
    --intel) ARCHS=(arm64 x86_64) ;;
    --quick) PROFILE=release ;;
    --out) OUT=${2:?--out needs a folder}; shift ;;
    -h | --help) usage; exit 0 ;;
    *) usage; die "Unknown option: $1" ;;
  esac
  shift
done

ask() {
  if [[ $YES == 1 ]]; then return 0; fi
  if [[ ! -t 0 ]]; then return 1; fi
  local reply
  read -r -p "$(printf '%s❓ %s [Y/n] %s' "$BOLD" "$1" "$RESET")" reply
  [[ -z $reply || $reply == [yY]* ]]
}
have() { command -v "$1" > /dev/null 2>&1; }

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
OUT=${OUT:-$repo/dist}
mkdir -p "$OUT" || die "Cannot create the output folder $OUT"
OUT=$(cd "$OUT" && pwd)
tools=$HOME/.cache/ferrocord-build
venv=$tools/macos-venv
started=$(date +%s)

printf '\n%s🍎 Ferrocord macOS disk image builder%s\n' "$BOLD" "$RESET"
hint "Disk images will be written to: $OUT"

# --- 1. system -----------------------------------------------------------------
title "1/4 🔎 Checking your Mac"
[[ $(uname -s) == Darwin ]] || die "This script runs on macOS. Use build-linux.sh or build-windows.ps1 elsewhere."
if [[ $(uname -m) != arm64 ]]; then
  warn "This Mac has an Intel processor; the Apple Silicon image is cross-compiled and cannot be test-run here."
fi
ok "macOS $(sw_vers -productVersion) on $(uname -m)"

# --- 2. dependencies -------------------------------------------------------------
title "2/4 🧰 Checking the tools"
if ! xcode-select -p > /dev/null 2>&1 || ! have clang; then
  warn "Apple's command line developer tools are missing"
  if ask "Install them now? (a macOS window will open; click Install)"; then
    xcode-select --install 2> /dev/null
    step "Waiting for the installation to finish…"
    until xcode-select -p > /dev/null 2>&1 && have clang; do sleep 10; done
    ok "Command line tools installed"
  else
    die "Cannot build without them (run: xcode-select --install)."
  fi
fi
ok "Command line tools ($(xcode-select -p))"

# shellcheck source=/dev/null
[[ -f $HOME/.cargo/env ]] && . "$HOME/.cargo/env"
if ! have cargo || ! have rustup; then
  warn "Rust is not installed"
  if ask "Install Rust with rustup (https://rustup.rs) now?"; then
    step "Installing Rust…"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal || die "Installing Rust failed."
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
  else
    die "Cannot build without Rust. See https://rustup.rs"
  fi
fi
msrv=$(sed -n 's/^rust-version *= *"\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)
version_ge() { [[ $(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1) == "$2" ]]; }
rustc_version=$(rustc --version 2> /dev/null | awk '{print $2}')
if [[ -z $rustc_version ]] || ! version_ge "$rustc_version" "$msrv"; then
  warn "Rust ${rustc_version:-?} is older than the required $msrv"
  if ask "Update Rust now?"; then
    { rustup toolchain install stable --profile minimal && rustup default stable; } || die "Updating Rust failed."
    rustc_version=$(rustc --version | awk '{print $2}')
  else
    die "Ferrocord needs Rust $msrv or newer (run: rustup update)."
  fi
fi
ok "Rust $rustc_version"

targets=()
for a in "${ARCHS[@]}"; do
  case $a in arm64) targets+=(aarch64-apple-darwin) ;; x86_64) targets+=(x86_64-apple-darwin) ;; esac
done
for t in "${targets[@]}"; do
  if ! rustup target list --installed | grep -qx "$t"; then
    step "Adding the Rust target ${t}…"
    rustup target add "$t" > /dev/null || die "Could not add the Rust target $t."
  fi
done
ok "Rust targets: ${targets[*]}"

if [[ ! -x $venv/bin/dmgbuild || ! -x $venv/bin/cmake ]]; then
  warn "dmgbuild and CMake are missing (needed for the disk image and the audio codec)"
  if ask "Install them into a private folder ($venv)?"; then
    step "Installing dmgbuild and CMake…"
    mkdir -p "$tools"
    { python3 -m venv "$venv" && "$venv/bin/pip" install -q --upgrade pip &&
      "$venv/bin/pip" install -q "dmgbuild==1.6.7" cmake; } || die "Installing dmgbuild/CMake failed."
  else
    die "Cannot build the disk image without dmgbuild."
  fi
fi
export PATH="$venv/bin:$PATH"
ok "dmgbuild $(dmgbuild --version 2> /dev/null || echo installed), $(cmake --version | head -1)"

# --- 3. build ----------------------------------------------------------------------
title "3/4 🔨 Building Ferrocord"
cd "$repo" || exit 1
version=$(sed -n '/^\[workspace.package\]/,/^\[/ s/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
[[ -n $version ]] || die "Could not read the version from Cargo.toml"
export MACOSX_DEPLOYMENT_TARGET=12.0
ok "Version $version, profile '$PROFILE', macOS $MACOSX_DEPLOYMENT_TARGET+"
hint "The first build downloads and compiles everything: expect 5-20 minutes."

declare -a done_archs=() failed_archs=()
for i in "${!ARCHS[@]}"; do
  a=${ARCHS[$i]} t=${targets[$i]}
  step "Compiling for ${a}…"
  build_start=$(date +%s)
  if cargo build --locked --profile "$PROFILE" --target "$t" -p ferrocord; then
    ok "Built for $a in $(($(date +%s) - build_start)) s"
  else
    fail "The $a build failed. Scroll up for the first 'error:' line."
    failed_archs+=("$a")
    continue
  fi

  # --- 4. package ------------------------------------------------------------------
  title "4/4 📦 Creating the $a disk image"
  if bash packaging/macos/package.sh "$version" "$a" "target/$t/$PROFILE/ferrocord" "$OUT"; then
    done_archs+=("$a")
  else
    fail "Packaging the $a disk image failed (see the messages above)."
    failed_archs+=("$a")
  fi
done

title "🏁 Summary"
for a in ${done_archs[@]+"${done_archs[@]}"}; do ok "ferrocord-$version-macos-$a.dmg"; done
for a in ${failed_archs[@]+"${failed_archs[@]}"}; do fail "$a"; done
echo
printf '%s📂 %s%s\n' "$BOLD" "$OUT" "$RESET"
shopt -s nullglob
for f in "$OUT"/ferrocord-"$version"-macos-*.dmg; do
  printf '   %s  %s(%s)%s\n' "$(basename "$f")" "$DIM" "$(du -h "$f" | cut -f1)" "$RESET"
done
if [[ ${#done_archs[@]} -gt 0 && ${MACOS_SIGN_IDENTITY:--} == - ]]; then
  echo
  hint "These images are ad-hoc signed. On another Mac, approve the first launch in"
  hint "System Settings > Privacy & Security > Open Anyway."
fi
echo
total=$(($(date +%s) - started))
if [[ ${#failed_archs[@]} -eq 0 ]]; then
  printf '%s%s🎉 All done in %dm %02ds!%s\n' "$BOLD" "$GREEN" $((total / 60)) $((total % 60)) "$RESET"
  open "$OUT" 2> /dev/null || true
else
  printf '%s%s😕 Finished with errors after %dm %02ds.%s\n' "$BOLD" "$RED" $((total / 60)) $((total % 60)) "$RESET"
  exit 1
fi
