#!/usr/bin/env bash
# Smoke-test the macOS release artifacts (.dmg and .tar.gz).
#
#   packaging/macos/smoke-test.sh <dmg> <tar.gz> <version>
set -euo pipefail
dmg=$1 tarball=$2 version=$3

check() { # binary [arch-prefix...]
  local bin=$1
  shift
  local got
  got=$("$@" "$bin" --version)
  [[ $got == "ferrocord $version" ]] || { echo "$* $bin --version printed '$got'" >&2; exit 1; }
  echo "ok: ${*:+$* }$bin --version -> $got"
}

work=$(mktemp -d)
mnt=$work/mnt
cleanup() { hdiutil detach "$mnt" -quiet 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT

hdiutil verify "$dmg"
mkdir -p "$mnt"
hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mnt" -quiet
app=$mnt/Ferrocord.app
[[ -L $mnt/Applications ]] || { echo "dmg lacks the /Applications link" >&2; exit 1; }
codesign --verify --strict --verbose=2 "$app"
[[ $(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist") == dev.ferrocord.Ferrocord ]]
archs=$(lipo -archs "$app/Contents/MacOS/ferrocord")
[[ $archs == *arm64* && $archs == *x86_64* ]] || { echo "not universal: $archs" >&2; exit 1; }
check "$app/Contents/MacOS/ferrocord"
if arch -x86_64 /usr/bin/true 2>/dev/null; then
  check "$app/Contents/MacOS/ferrocord" arch -x86_64
else
  echo "note: Rosetta 2 not available, x86_64 slice not executed"
fi

mkdir -p "$work/tar"
tar -C "$work/tar" -xzf "$tarball"
codesign --verify --strict "$work/tar/Ferrocord.app"
check "$work/tar/Ferrocord.app/Contents/MacOS/ferrocord"
