#!/usr/bin/env bash
# Smoke-test one Linux artifact inside a clean distribution container (run as root).
#
#   packaging/linux/smoke-test.sh <deb|rpm|appimage|tar> <artifact> <version>
#
# Typical use from the host:
#   docker run --rm -v "$PWD:/w" -w /w ubuntu:24.04 \
#     packaging/linux/smoke-test.sh deb dist/ferrocord_0.1.0-1_amd64.deb 0.1.0
#
# deb/rpm: installs with the distribution's package manager, so declared
#   dependencies are resolved exactly as for a user, then runs `ferrocord --version`
#   and checks the desktop integration files.
# appimage: installs only the base libraries an AppImage may expect from the
#   host (graphics/X11/fonts, see linuxdeploy's excludelist), then runs it with
#   --appimage-extract-and-run (no FUSE in containers).
# tar: installs the runtime libraries, extracts and runs the binary.
#
# `--version` exits before any window or WebView is created, so no display is needed.
set -euo pipefail

kind=$1 artifact=$2 version=$3
artifact=$(readlink -f "$artifact")
app_id=dev.ferrocord.Ferrocord

if command -v apt-get >/dev/null; then
  pm=apt
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -q
elif command -v dnf >/dev/null; then
  pm=dnf
elif command -v zypper >/dev/null; then
  pm=zypper
else
  echo "unsupported distribution" >&2
  exit 1
fi

install_pkgs() {
  case $pm in
    apt) apt-get install -y -q --no-install-recommends "$@" ;;
    dnf) dnf install -y -q --setopt=install_weak_deps=False "$@" ;;
    zypper) zypper --non-interactive install --no-recommends "$@" ;;
  esac
}

# ALSA runtime (cpal): renamed to libasound2t64 in Ubuntu 24.04 / Debian 13.
apt_alsa() { if apt-cache show libasound2t64 >/dev/null 2>&1; then echo libasound2t64; else echo libasound2; fi; }

check_version() {
  local got
  got=$("$@" --version)
  if [[ $got != "ferrocord $version" ]]; then
    echo "FAIL: '$*' --version printed '$got', expected 'ferrocord $version'" >&2
    exit 1
  fi
  echo "ok: $* --version -> $got"
}

case $kind in
  deb)
    install_pkgs "$artifact"
    check_version ferrocord
    for f in /usr/share/applications/$app_id.desktop /usr/share/metainfo/$app_id.metainfo.xml \
      /usr/share/icons/hicolor/256x256/apps/$app_id.png /usr/share/icons/hicolor/scalable/apps/$app_id.svg; do
      [[ -f $f ]] || { echo "FAIL: missing $f" >&2; exit 1; }
    done
    apt-get remove -y -q ferrocord
    [[ ! -e /usr/bin/ferrocord ]] || { echo "FAIL: /usr/bin/ferrocord left after removal" >&2; exit 1; }
    echo "ok: package installs, runs and uninstalls"
    ;;
  rpm)
    case $pm in
      dnf) dnf install -y -q "$artifact" ;;
      zypper) zypper --non-interactive --no-gpg-checks install --allow-unsigned-rpm "$artifact" ;;
      *) echo "rpm smoke test needs dnf or zypper" >&2; exit 1 ;;
    esac
    check_version ferrocord
    for f in /usr/share/applications/$app_id.desktop /usr/share/metainfo/$app_id.metainfo.xml \
      /usr/share/icons/hicolor/256x256/apps/$app_id.png; do
      [[ -f $f ]] || { echo "FAIL: missing $f" >&2; exit 1; }
    done
    rpm -e ferrocord
    echo "ok: package installs, runs and uninstalls"
    ;;
  appimage)
    # Libraries on linuxdeploy's excludelist that the AppImage takes from the host
    # (graphics, X11/Wayland client, fonts, core system libraries).
    case $pm in
      apt) install_pkgs libgl1 libegl1 libgbm1 libdrm2 libx11-6 libx11-xcb1 libxcb1 libwayland-client0 \
        libfontconfig1 libfreetype6 libharfbuzz0b libfribidi0 libexpat1 libuuid1 zlib1g libgpg-error0 \
        libcom-err2 libgmp10 libstdc++6 "$(apt_alsa)" ;;
      dnf) install_pkgs mesa-libGL mesa-libEGL mesa-libgbm libdrm libX11 libX11-xcb libxcb libwayland-client \
        fontconfig freetype harfbuzz fribidi expat libuuid zlib-ng-compat libgpg-error libcom_err gmp libstdc++ \
        alsa-lib findutils ;;
      zypper) install_pkgs Mesa-libGL1 Mesa-libEGL1 libgbm1 libdrm2 libX11-6 libX11-xcb1 libxcb1 libwayland-client0 \
        fontconfig libfreetype6 libharfbuzz0 libfribidi0 libexpat1 libuuid1 libz1 libgpg-error0 libcom_err2 \
        libgmp10 libstdc++6 libasound2 ;;
    esac
    work=$(mktemp -d)
    cp "$artifact" "$work/app.AppImage" # the artifact may be on a read-only mount
    chmod +x "$work/app.AppImage"
    check_version "$work/app.AppImage" --appimage-extract-and-run
    # The captcha WebView's helper processes must be bundled.
    cd "$work"
    ./app.AppImage --appimage-extract >/dev/null
    find squashfs-root -name WebKitNetworkProcess -type f | grep -q . || { echo "FAIL: WebKitNetworkProcess not bundled" >&2; exit 1; }
    echo "ok: AppImage runs; WebKit helpers bundled"
    ;;
  tar)
    case $pm in
      apt) install_pkgs libwebkit2gtk-4.1-0 libgtk-3-0 "$(apt_alsa)" ;;
      dnf) install_pkgs webkit2gtk4.1 gtk3 alsa-lib ;;
      zypper) install_pkgs libwebkit2gtk-4_1-0 libgtk-3-0 libasound2 ;;
    esac
    dir=$(mktemp -d)
    tar -C "$dir" -xzf "$artifact"
    check_version "$dir"/ferrocord-*/ferrocord
    ;;
  *)
    echo "unknown kind: $kind" >&2
    exit 2
    ;;
esac
