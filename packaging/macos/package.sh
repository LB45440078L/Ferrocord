#!/usr/bin/env bash
# Build a universal (arm64 + x86_64) Ferrocord.app and package it as .dmg and .tar.gz.
#
#   packaging/macos/package.sh <version> <out-dir> <aarch64-binary> <x86_64-binary>
#
# Runs on macOS (needs lipo, codesign, hdiutil). Signing identity:
#   MACOS_SIGN_IDENTITY  unset or "-" -> ad-hoc signature (default, not notarizable)
#                        "Developer ID Application: ..." -> hardened-runtime signature
#                        (notarize afterwards; see docs/RELEASING.md)
# Outputs:
#   ferrocord-<version>-macos-universal.dmg
#   ferrocord-<version>-macos-universal.tar.gz  (contains Ferrocord.app)
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 <version> <out-dir> <aarch64-binary> <x86_64-binary>" >&2
  exit 2
fi
version=$1 out=$2 bin_arm=$3 bin_x86=$4
min_macos=${MACOSX_DEPLOYMENT_TARGET:-12.0}
identity=${MACOS_SIGN_IDENTITY:--}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

short=${version%%[-+]*} # CFBundleShortVersionString must be numeric
app=$stage/Ferrocord.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/ferrocord" "$bin_arm" "$bin_x86"
lipo -info "$app/Contents/MacOS/ferrocord"
sed -e "s/@VERSION@/$version/g" -e "s/@SHORT_VERSION@/$short/g" -e "s/@MIN_MACOS@/$min_macos/g" \
  "$root/packaging/macos/Info.plist.in" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
cp "$root/packaging/icons/ferrocord.icns" "$app/Contents/Resources/Ferrocord.icns"

if [[ $identity == - ]]; then
  codesign --force --sign - "$app"
else
  codesign --force --timestamp --options runtime --sign "$identity" "$app"
fi
codesign --verify --strict --verbose=2 "$app"

name=ferrocord-$version-macos-universal
tar -C "$stage" -czf "$out/$name.tar.gz" Ferrocord.app
echo "created $out/$name.tar.gz"

dmgroot=$stage/dmg
mkdir -p "$dmgroot"
cp -R "$app" "$dmgroot/"
ln -s /Applications "$dmgroot/Applications"
cp "$root/LICENSE" "$dmgroot/LICENSE.txt"
hdiutil create -volname "Ferrocord $version" -srcfolder "$dmgroot" -fs HFS+ -format UDZO -ov "$out/$name.dmg"
echo "created $out/$name.dmg"
