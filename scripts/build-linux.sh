#!/usr/bin/env bash
# Build every Linux package of Ferrocord on this machine: .deb, .rpm, .AppImage, .tar.gz.
#
#   bash scripts/build-linux.sh [options]
#
# Works on Debian/Ubuntu (including Ubuntu on WSL2). Checks what is missing, asks
# before installing anything, builds, and puts the packages in ./dist.
#
# Options:
#   -y, --yes        install missing tools without asking
#   --quick          use the faster `release` profile instead of `dist` (fat LTO)
#   --out DIR        where to put the packages (default: <repo>/dist)
#   --formats LIST   comma-separated subset of tar,deb,rpm,appimage (default: all)
#   -h, --help       show this help
set -uo pipefail

# --- look and feel -----------------------------------------------------------
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

usage() { sed -n '2,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

# --- options -----------------------------------------------------------------
YES=0 PROFILE=dist OUT='' FORMATS=tar,deb,rpm,appimage
while [[ $# -gt 0 ]]; do
  case $1 in
    -y | --yes) YES=1 ;;
    --quick) PROFILE=release ;;
    --out) OUT=${2:?--out needs a folder}; shift ;;
    --formats) FORMATS=${2:?--formats needs a list}; shift ;;
    -h | --help) usage; exit 0 ;;
    *) usage; die "Unknown option: $1" ;;
  esac
  shift
done

ask() { # question -> 0 for yes
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
started=$(date +%s)

printf '\n%s🦀 Ferrocord Linux package builder%s\n' "$BOLD" "$RESET"
hint "Packages will be written to: $OUT"

# --- 1. system -----------------------------------------------------------------
title "1/4 🔎 Checking your system"
arch=$(uname -m)
case $arch in
  x86_64) target=x86_64-unknown-linux-gnu ;;
  aarch64 | arm64) arch=aarch64 target=aarch64-unknown-linux-gnu ;;
  *) die "Unsupported CPU architecture: $arch (x86_64 and aarch64 are supported)" ;;
esac
ok "CPU: $arch"

if ! have apt-get; then
  fail "This script installs its tools with apt, so it needs Debian or Ubuntu."
  hint "On other distributions, run it inside an Ubuntu container:"
  hint "  docker run --rm -it -v \"$repo:/w\" -w /w ubuntu:22.04 bash scripts/build-linux.sh --yes"
  exit 1
fi
# shellcheck source=/dev/null
. /etc/os-release 2> /dev/null && ok "System: ${PRETTY_NAME:-Linux}"
if grep -qi microsoft /proc/version 2> /dev/null; then ok "Running inside WSL"; fi

SUDO=''
if [[ $(id -u) -ne 0 ]]; then
  have sudo || die "sudo is not installed; run this script as root instead."
  SUDO=sudo
fi

# --- 2. dependencies -----------------------------------------------------------
title "2/4 🧰 Checking the tools"

missing_pkgs=()
for pkg in build-essential pkg-config cmake clang libclang-dev libwebkit2gtk-4.1-dev libgtk-3-dev \
  libdbus-1-dev libxkbcommon-dev libssl-dev libasound2-dev libv4l-dev dpkg-dev zip file curl; do
  dpkg-query -W -f='${Status}' "$pkg" 2> /dev/null | grep -q 'install ok installed' || missing_pkgs+=("$pkg")
done
if [[ ${#missing_pkgs[@]} -eq 0 ]]; then
  ok "System libraries and compilers"
else
  warn "Missing system packages: ${missing_pkgs[*]}"
  if ask "Install them now with apt? (asks for your password)"; then
    step "Installing system packages…"
    $SUDO bash "$repo/packaging/linux/install-build-deps.sh" --package || die "Installing the system packages failed."
    $SUDO apt-get install -y -q --no-install-recommends curl > /dev/null || true
    ok "System packages installed"
  else
    die "Cannot build without them. Install with: sudo bash packaging/linux/install-build-deps.sh --package"
  fi
fi

# shellcheck source=/dev/null
[[ -f $HOME/.cargo/env ]] && . "$HOME/.cargo/env"
if ! have cargo || ! have rustup; then
  warn "Rust is not installed"
  if ask "Install Rust with rustup (https://rustup.rs) now?"; then
    step "Installing Rust…"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal || die "Installing Rust failed."
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
    ok "Rust installed"
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

want() { [[ ",$FORMATS," == *",$1,"* ]]; }
cargo_tools=()
want deb && ! cargo deb --version > /dev/null 2>&1 && cargo_tools+=(cargo-deb)
want rpm && ! cargo generate-rpm --version > /dev/null 2>&1 && cargo_tools+=(cargo-generate-rpm)
if [[ ${#cargo_tools[@]} -gt 0 ]]; then
  warn "Missing packaging tools: ${cargo_tools[*]}"
  if ask "Install them with cargo? (takes a few minutes, once)"; then
    step "Installing ${cargo_tools[*]}…"
    cargo install --locked "${cargo_tools[@]}" || die "Installing ${cargo_tools[*]} failed."
  else
    die "Cannot build .deb/.rpm without them (or pass --formats tar,appimage)."
  fi
fi
want deb && ok "cargo-deb $(cargo deb --version 2> /dev/null | awk '{print $NF}')"
want rpm && ok "cargo-generate-rpm $(cargo generate-rpm --version 2> /dev/null | awk '{print $NF}')"
want appimage && ok "AppImage tools are downloaded automatically (cached in ~/.cache)"

# --- 3. build --------------------------------------------------------------------
title "3/4 🔨 Building Ferrocord"
src=$repo
if [[ $repo == /mnt/* ]]; then
  # Building on a Windows drive through WSL is very slow; work on a copy in the Linux filesystem.
  src=$HOME/.cache/ferrocord-build/src
  step "Copying the sources from the Windows drive into WSL (much faster to build)…"
  rm -rf "$src" && mkdir -p "$src"
  (cd "$repo" && tar --exclude=./target --exclude=./dist --exclude=./.git -cf - .) | (cd "$src" && tar -xf -) ||
    die "Copying the sources failed."
  export CARGO_TARGET_DIR=$HOME/.cache/ferrocord-build/target
  ok "Sources copied to $src"
fi
cd "$src" || die "Cannot enter $src"
version=$(sed -n '/^\[workspace.package\]/,/^\[/ s/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
[[ -n $version ]] || die "Could not read the version from Cargo.toml"
ok "Version $version, profile '$PROFILE'"
hint "The first build downloads and compiles everything: expect 5-20 minutes."
build_start=$(date +%s)
if ! cargo build --locked --profile "$PROFILE" --target "$target" -p ferrocord; then
  die "The build failed. Scroll up for the first 'error:' line."
fi
ok "Built in $(($(date +%s) - build_start)) s"

# --- 4. package ------------------------------------------------------------------
title "4/4 📦 Creating packages"
declare -A result
for fmt in tar deb rpm appimage; do
  want "$fmt" || continue
  step "Making the $fmt package…"
  if PROFILE=$PROFILE bash packaging/linux/package.sh "$target" "$version" "$OUT" "$fmt"; then
    result[$fmt]=ok
  else
    result[$fmt]=failed
    fail "The $fmt package failed (see the messages above)."
  fi
done

title "🏁 Summary"
failed=0
for fmt in tar deb rpm appimage; do
  want "$fmt" || continue
  if [[ ${result[$fmt]} == ok ]]; then ok "$fmt"; else fail "$fmt"; failed=1; fi
done
echo
printf '%s📂 %s%s\n' "$BOLD" "$OUT" "$RESET"
shopt -s nullglob
for f in "$OUT"/*"$version"*{.tar.gz,.deb,.rpm,.AppImage}; do
  printf '   %s  %s(%s)%s\n' "$(basename "$f")" "$DIM" "$(du -h "$f" | cut -f1)" "$RESET"
done
echo
total=$(($(date +%s) - started))
if [[ $failed == 0 ]]; then
  printf '%s%s🎉 All done in %dm %02ds!%s\n' "$BOLD" "$GREEN" $((total / 60)) $((total % 60)) "$RESET"
else
  printf '%s%s😕 Finished with errors after %dm %02ds.%s\n' "$BOLD" "$RED" $((total / 60)) $((total % 60)) "$RESET"
  exit 1
fi
