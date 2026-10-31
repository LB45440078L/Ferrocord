#!/usr/bin/env bash
# Build Ferrocord.app for one architecture and package it as a styled .dmg.
#
#   packaging/macos/package.sh <version> <arm64|x86_64> <ferrocord-binary> <out-dir>
#
# Runs on macOS. Needs Xcode command line tools (codesign, hdiutil)
# and dmgbuild (`pip install dmgbuild`).
#
# Signing (MACOS_SIGN_IDENTITY):
#   unset or "-"                      ad-hoc signature. Runs after the user approves it once in
#                                     System Settings > Privacy & Security > Open Anyway.
#   "Developer ID Application: ..."   hardened runtime + entitlements, timestamped; the .dmg is
#                                     signed too.
# Notarization (Developer ID only), when these are set:
#   APPLE_NOTARY_KEY        path to an App Store Connect API key (.p8)
#   APPLE_NOTARY_KEY_ID     its key id
#   APPLE_NOTARY_ISSUER     its issuer id
# The .dmg is submitted with notarytool and the ticket is stapled to it.
#
# Output: <out-dir>/ferrocord-<version>-macos-<arch>.dmg
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 <version> <arm64|x86_64> <ferrocord-binary> <out-dir>" >&2
  exit 2
fi
version=$1 arch=$2 bin=$3 out=$4
case $arch in arm64 | x86_64) ;; *) echo "unsupported arch: $arch" >&2; exit 2 ;; esac
# Apple Silicon needs macOS 11; 12 is the oldest release Ferrocord is tested on.
min_macos=${MACOSX_DEPLOYMENT_TARGET:-12.0}
identity=${MACOS_SIGN_IDENTITY:--}

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

got_arch=$(lipo -archs "$bin")
[[ $got_arch == "$arch" ]] || { echo "$bin is '$got_arch', expected $arch" >&2; exit 1; }

# --- app bundle ---------------------------------------------------------------
short=${version%%[-+]*} # CFBundleShortVersionString must be numeric
app=$stage/Ferrocord.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/ferrocord"
sed -e "s/@VERSION@/$version/g" -e "s/@SHORT_VERSION@/$short/g" -e "s/@MIN_MACOS@/$min_macos/g" \
  "$here/Info.plist.in" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
cp "$root/packaging/icons/ferrocord.icns" "$app/Contents/Resources/Ferrocord.icns"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE.txt"

if [[ $identity == - ]]; then
  codesign --force --sign - "$app"
else
  codesign --force --timestamp --options runtime --entitlements "$here/Ferrocord.entitlements" \
    --sign "$identity" "$app"
fi
codesign --verify --strict --verbose=2 "$app"

# --- disk image ---------------------------------------------------------------
name=ferrocord-$version-macos-$arch
dmg=$out/$name.dmg
rm -f "$dmg"
dmgbuild -s "$here/dmg-settings.py" \
  -D app="$app" -D background="$here/dmg/background.png" -D icon="$root/packaging/icons/ferrocord.icns" \
  "Ferrocord" "$dmg"

if [[ $identity != - ]]; then
  codesign --force --timestamp --sign "$identity" "$dmg"
  if [[ -n ${APPLE_NOTARY_KEY:-} ]]; then
    xcrun notarytool submit "$dmg" --wait --timeout 30m \
      --key "$APPLE_NOTARY_KEY" --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER"
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
  fi
fi
hdiutil verify "$dmg"
echo "created $dmg"
