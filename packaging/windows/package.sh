#!/usr/bin/env bash
# Package a built ferrocord.exe as a portable .zip and an NSIS installer.
#
#   packaging/windows/package.sh <path/to/ferrocord.exe> <version> <x86_64|arm64> <out-dir>
#
# Needs `makensis` (NSIS 3.x) and `zip`. Runs on Linux (release CI uses
# ubuntu + the distro `nsis` package) or on Windows under Git Bash.
# Outputs:
#   <out-dir>/ferrocord-<version>-windows-<arch>.zip
#   <out-dir>/ferrocord-<version>-windows-<arch>-setup.exe
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 <ferrocord.exe> <version> <x86_64|arm64> <out-dir>" >&2
  exit 2
fi
exe=$1 version=$2 arch=$3 out=$4
case $arch in x86_64 | arm64) ;; *) echo "unsupported arch: $arch" >&2; exit 2 ;; esac

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

# VIProductVersion needs four numeric fields: 0.2.0-rc.1 -> 0.2.0.0
core=${version%%[-+]*}
if [[ ! $core =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version '$version' is not SemVer" >&2
  exit 1
fi

name="ferrocord-${version}-windows-${arch}"
mkdir -p "$stage/$name"
cp "$exe" "$stage/$name/ferrocord.exe"
# CRLF text files so Notepad and the installer's license page render them properly.
sed 's/\r$//; s/$/\r/' "$root/LICENSE" > "$stage/$name/LICENSE.txt"
sed 's/\r$//; s/$/\r/' "$root/README.md" > "$stage/$name/README.txt"

(cd "$stage" && zip -9 -q -r "$out/$name.zip" "$name")
echo "created $out/$name.zip"

makensis -V2 -NOCD \
  "-DVERSION=$version" \
  "-DVERSION_NUMERIC=$core.0" \
  "-DARCH=$arch" \
  "-DSRCDIR=$stage/$name" \
  "-DICON=$root/packaging/icons/ferrocord.ico" \
  "-DOUTFILE=$out/$name-setup.exe" \
  "$root/packaging/windows/installer.nsi"
echo "created $out/$name-setup.exe"
