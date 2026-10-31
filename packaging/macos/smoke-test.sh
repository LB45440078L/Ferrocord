#!/usr/bin/env bash
# Smoke-test a macOS release disk image.
#
#   packaging/macos/smoke-test.sh <dmg> <version> <arm64|x86_64>
#
# Mounts the image, checks its layout, the bundle metadata, signature and
# architecture, then "installs" the app into a temporary Applications folder
# the way a user would (drag and drop is a copy) and runs it from there.
set -euo pipefail
dmg=$1 version=$2 arch=$3

check() { # binary [prefix...]
  local bin=$1
  shift
  local got
  got=$("$@" "$bin" --version)
  [[ $got == "ferrocord $version" ]] || { echo "${*:+$* }$bin --version printed '$got'" >&2; exit 1; }
  echo "ok: ${*:+$* }$bin --version -> $got"
}
plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$app/Contents/Info.plist"; }

work=$(mktemp -d)
mnt=$work/mnt
cleanup() { hdiutil detach "$mnt" -quiet 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT

hdiutil verify "$dmg"
format=$(hdiutil imageinfo "$dmg" | awk -F': ' '/^Format:/ {print $2}')
[[ $format == ULFO ]] || { echo "dmg format is '$format', expected ULFO" >&2; exit 1; }
mkdir -p "$mnt"
hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mnt" -quiet
app=$mnt/Ferrocord.app

[[ -L $mnt/Applications && $(readlink "$mnt/Applications") == /Applications ]] || { echo "dmg lacks the /Applications link" >&2; exit 1; }
[[ -f $mnt/.DS_Store ]] || { echo "dmg has no Finder layout (.DS_Store)" >&2; exit 1; }
[[ -f $mnt/.VolumeIcon.icns ]] || { echo "dmg has no volume icon" >&2; exit 1; }
[[ -f $mnt/.background.tiff ]] || { echo "dmg has no HiDPI background image" >&2; exit 1; }
echo "ok: dmg layout (Applications link, background, volume icon)"

[[ $(plist CFBundleIdentifier) == dev.ferrocord.Ferrocord ]]
[[ $(plist CFBundleShortVersionString) == "${version%%[-+]*}" ]]
[[ $(plist CFBundleGetInfoString) == "Ferrocord $version" ]]
plist NSCameraUsageDescription > /dev/null
plist NSMicrophoneUsageDescription > /dev/null
[[ -f $app/Contents/Resources/Ferrocord.icns && -f $app/Contents/Resources/LICENSE.txt ]]
echo "ok: Info.plist and resources"

codesign --verify --strict --deep --verbose=2 "$app"
if codesign -dv "$app" 2>&1 | grep -q 'Signature=adhoc'; then
  echo "note: ad-hoc signature (unsigned release); Gatekeeper will ask the user to approve it"
else
  codesign -d --entitlements - --xml "$app" | grep -q com.apple.security.device.camera ||
    { echo "Developer ID build lacks the camera entitlement" >&2; exit 1; }
  spctl --assess --type execute --verbose=2 "$app"
fi

got_arch=$(lipo -archs "$app/Contents/MacOS/ferrocord")
[[ $got_arch == "$arch" ]] || { echo "binary is '$got_arch', expected $arch" >&2; exit 1; }

apps=$work/Applications
mkdir -p "$apps"
ditto "$app" "$apps/Ferrocord.app"
codesign --verify --strict "$apps/Ferrocord.app"
if [[ $arch == "$(uname -m)" ]]; then
  check "$apps/Ferrocord.app/Contents/MacOS/ferrocord"
elif arch -x86_64 /usr/bin/true 2>/dev/null; then
  check "$apps/Ferrocord.app/Contents/MacOS/ferrocord" arch -x86_64
else
  echo "note: cannot execute $arch on $(uname -m) here (no Rosetta 2); skipped running it"
fi
