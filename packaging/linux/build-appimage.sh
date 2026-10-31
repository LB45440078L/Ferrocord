#!/usr/bin/env bash
# Build ferrocord-<version>-linux-<arch>.AppImage from a compiled binary.
#
#   packaging/linux/build-appimage.sh <ferrocord-binary> <version> <x86_64|aarch64> <out-dir>
#
# Uses linuxdeploy (+ the GTK plugin) to bundle shared libraries into an AppDir
# and appimagetool to produce a type-2 AppImage (static runtime, no libfuse2
# needed). Tool versions are pinned below; downloads are cached in
# $APPIMAGE_TOOLS_DIR (default: ~/.cache/ferrocord-appimage-tools).
#
# WebKitGTK (used for the captcha window) starts helper processes from a path
# compiled into libwebkit2gtk. As in Tauri's AppImages, the helpers are copied
# into the AppDir, "/usr" in the bundled library is patched to "././", and the
# AppRun hook changes into $APPDIR/usr so those relative paths resolve. The
# caller's working directory is exported as $OWD.
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 <ferrocord-binary> <version> <x86_64|aarch64> <out-dir>" >&2
  exit 2
fi
bin=$1 version=$2 arch=$3 out=$4
case $arch in x86_64 | aarch64) ;; *) echo "unsupported arch: $arch" >&2; exit 2 ;; esac

LINUXDEPLOY_VERSION=1-alpha-20251107-1
LINUXDEPLOY_GTK_COMMIT=7a3fbc31a9e5075073ff8790f26effbac5f84453
APPIMAGETOOL_VERSION=1.9.1

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
app_id=dev.ferrocord.Ferrocord
tools=${APPIMAGE_TOOLS_DIR:-$HOME/.cache/ferrocord-appimage-tools}
mkdir -p "$tools" "$out"
out=$(cd "$out" && pwd)
export APPIMAGE_EXTRACT_AND_RUN=1 # CI runners and containers have no FUSE

fetch() { # url dest
  [[ -s $2 ]] || { curl -fsSL --retry 3 -o "$2.part" "$1" && mv "$2.part" "$2"; }
  chmod +x "$2"
}
linuxdeploy=$tools/linuxdeploy-$LINUXDEPLOY_VERSION-$arch.AppImage
appimagetool=$tools/appimagetool-$APPIMAGETOOL_VERSION-$arch.AppImage
fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/$LINUXDEPLOY_VERSION/linuxdeploy-$arch.AppImage" "$linuxdeploy"
fetch "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-$arch.AppImage" "$appimagetool"
plugindir=$tools/plugins-$LINUXDEPLOY_GTK_COMMIT
mkdir -p "$plugindir"
fetch "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/$LINUXDEPLOY_GTK_COMMIT/linuxdeploy-plugin-gtk.sh" "$plugindir/linuxdeploy-plugin-gtk.sh"
export PATH="$plugindir:$PATH"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir=$work/AppDir

install -Dm755 "$bin" "$appdir/usr/bin/ferrocord"
install -Dm644 "$root/packaging/linux/$app_id.desktop" "$appdir/usr/share/applications/$app_id.desktop"
install -Dm644 "$root/packaging/linux/$app_id.metainfo.xml" "$appdir/usr/share/metainfo/$app_id.appdata.xml"
mkdir -p "$appdir/usr/share/icons"
cp -r "$root/packaging/icons/hicolor" "$appdir/usr/share/icons/"
install -Dm644 "$root/LICENSE" "$appdir/usr/share/doc/ferrocord/LICENSE"

# WebKitGTK helper processes and injected bundle, at their compiled-in location.
libdir=$(pkg-config --variable=libdir webkit2gtk-4.1)
webkit_helpers=$libdir/webkit2gtk-4.1
mkdir -p "$appdir$webkit_helpers"
cp -a "$webkit_helpers/." "$appdir$webkit_helpers/"

# Libraries winit/glutin load with dlopen() (invisible to ldd). linuxdeploy
# skips any that are on the AppImage excludelist (libGL, libEGL, libX11, ...).
extra_libs=()
for lib in libxkbcommon.so.0 libxkbcommon-x11.so.0 libwayland-cursor.so.0 libwayland-egl.so.1 libXcursor.so.1 libXi.so.6 libXrandr.so.2; do
  [[ -e $libdir/$lib ]] && extra_libs+=(--library "$libdir/$lib")
done

deps_only=()
while IFS= read -r -d '' f; do deps_only+=("--deploy-deps-only=$f"); done \
  < <(find "$appdir$webkit_helpers" -type f \( -name 'WebKit*Process' -o -name '*.so' \) -print0)

# Run from the AppDir's usr/ so the "/usr" -> "././" patched paths resolve.
mkdir -p "$appdir/apprun-hooks"
cat > "$appdir/apprun-hooks/zz-ferrocord-webkit-cwd.sh" <<'HOOK'
export OWD="${OWD:-$PWD}"
cd "$APPDIR/usr" || exit 1
HOOK

DEPLOY_GTK_VERSION=3 NO_STRIP=1 ARCH=$arch "$linuxdeploy" \
  --appdir "$appdir" \
  --executable "$appdir/usr/bin/ferrocord" \
  --desktop-file "$appdir/usr/share/applications/$app_id.desktop" \
  --icon-file "$root/packaging/icons/hicolor/256x256/apps/$app_id.png" \
  --plugin gtk \
  "${extra_libs[@]}" \
  "${deps_only[@]}"

find "$appdir/usr/lib" -name 'libwebkit2gtk-4.1.so*' -type f -exec sed -i -e 's|/usr|././|g' {} +

name=ferrocord-$version-linux-$arch.AppImage
ARCH=$arch VERSION=$version "$appimagetool" --no-appstream "$appdir" "$out/$name"
echo "created $out/$name"
