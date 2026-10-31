#!/usr/bin/env bash
# Install the Debian/Ubuntu packages needed to build (and package) Ferrocord.
# Used by CI on ubuntu-22.04/24.04 runners and by the local Docker recipe in
# docs/RELEASING.md. Run as root (or via sudo).
#
#   packaging/linux/install-build-deps.sh            # build + test dependencies
#   packaging/linux/install-build-deps.sh --package  # also packaging/validation tools
set -euo pipefail

packages=(
  # toolchain for vendored C/C++ code (Luau, aws-lc, OpenH264, Opus) and bindgen
  build-essential pkg-config cmake clang libclang-dev
  # wry/tao (captcha WebView) and GTK
  libwebkit2gtk-4.1-dev libgtk-3-dev
  # keyring / notifications (D-Bus), keyboard handling, TLS
  libdbus-1-dev libxkbcommon-dev libssl-dev
  # voice (cpal/ALSA) and camera (V4L2) crates
  libasound2-dev libv4l-dev
)
if [[ ${1:-} == --package ]]; then
  # dpkg-shlibdeps (cargo-deb $auto), archives, AppStream/desktop validation,
  # file for AppImage tooling
  packages+=(dpkg-dev zip file desktop-file-utils appstream)
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q --no-install-recommends "${packages[@]}"
