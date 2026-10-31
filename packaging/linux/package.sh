#!/usr/bin/env bash
# Package a built ferrocord binary for Linux: .tar.gz, .deb, .rpm and .AppImage.
#
#   packaging/linux/package.sh <target-triple> <version> <out-dir> [formats]
#
# Expects `cargo build --locked --profile dist --target <triple> -p ferrocord`
# to have run already (honours CARGO_TARGET_DIR). `formats` is a comma-separated
# subset of tar,deb,rpm,appimage (default: all). Needs cargo-deb and
# cargo-generate-rpm on PATH for deb/rpm; see build-appimage.sh for AppImage.
#
# Outputs (arch = x86_64 | aarch64):
#   ferrocord-<version>-linux-<arch>.tar.gz
#   ferrocord_<debversion>_<amd64|arm64>.deb
#   ferrocord-<rpmversion>-1.<arch>.rpm
#   ferrocord-<version>-linux-<arch>.AppImage
set -euo pipefail

if [[ $# -lt 3 ]]; then
  echo "usage: $0 <target-triple> <version> <out-dir> [tar,deb,rpm,appimage]" >&2
  exit 2
fi
target=$1 version=$2 out=$3 formats=${4:-tar,deb,rpm,appimage}
profile=${PROFILE:-dist}
arch=${target%%-*}
app_id=dev.ferrocord.Ferrocord

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
target_dir=${CARGO_TARGET_DIR:-$root/target}
bin=$target_dir/$target/$profile/ferrocord
[[ -x $bin ]] || { echo "missing $bin; build it first" >&2; exit 1; }
mkdir -p "$out"
out=$(cd "$out" && pwd)

reported=$("$bin" --version)
[[ $reported == "ferrocord $version" ]] || { echo "binary reports '$reported', expected 'ferrocord $version'" >&2; exit 1; }

want() { [[ ",$formats," == *",$1,"* ]]; }

if want tar; then
  name=ferrocord-$version-linux-$arch
  stage=$(mktemp -d)
  # share/ mirrors /usr/share so it can be copied to ~/.local/share for desktop integration.
  mkdir -p "$stage/$name/share/applications" "$stage/$name/share/metainfo" "$stage/$name/share/icons"
  install -m755 "$bin" "$stage/$name/ferrocord"
  install -m644 LICENSE README.md "$stage/$name/"
  install -m644 packaging/linux/$app_id.desktop "$stage/$name/share/applications/"
  install -m644 packaging/linux/$app_id.metainfo.xml "$stage/$name/share/metainfo/"
  cp -r packaging/icons/hicolor "$stage/$name/share/icons/"
  tar -C "$stage" --owner=0 --group=0 --numeric-owner -czf "$out/$name.tar.gz" "$name"
  rm -rf "$stage"
  echo "created $out/$name.tar.gz"
fi

if want deb; then
  cargo deb -p ferrocord --no-build --target "$target" --profile "$profile" --output "$out"
fi

if want rpm; then
  # Requirements come from the binary's direct dependencies (see rpm-find-requires.sh)
  # plus the dlopen()ed libraries listed in crates/app/Cargo.toml.
  # RPM versions cannot contain "-": 0.2.0-rc.1 becomes 0.2.0~rc.1, which sorts before 0.2.0.
  rpm_version=${version//-/\~}
  chmod +x "$root/packaging/linux/rpm-find-requires.sh" # executed directly by cargo-generate-rpm
  cargo generate-rpm -p crates/app --target "$target" --profile "$profile" \
    --set-metadata "version = \"$rpm_version\"" \
    --auto-req "$root/packaging/linux/rpm-find-requires.sh" --output "$out/"
fi

if want appimage; then
  bash "$root/packaging/linux/build-appimage.sh" "$bin" "$version" "$arch" "$out"
fi
