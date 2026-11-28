#!/usr/bin/env bash
# Package the prebuilt x86_64 Linux release archive into an AppImage with
# Gear Lever / AppImageUpdate zsync update metadata.
#
# Usage:
#   build-appimage.sh LINUX_RELEASE_ARCHIVE OUTPUT.AppImage [VERSION]
#
# Environment:
#   APPIMAGETOOL        Path to appimagetool executable (default: appimagetool on PATH)
#   APPIMAGE_UPDATE_INFO Override the embedded update information string
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
    echo "Usage: $0 LINUX_RELEASE_ARCHIVE OUTPUT.AppImage [VERSION]" >&2
    exit 2
fi

archive=$1
output=$2
version=${3:-}

appimagetool=${APPIMAGETOOL:-appimagetool}
if [[ "$appimagetool" == */* ]]; then
    appimagetool=$(cd -- "$(dirname -- "$appimagetool")" && printf '%s/%s' "$PWD" "$(basename -- "$appimagetool")")
fi
update_info=${APPIMAGE_UPDATE_INFO:-"gh-releases-zsync|mollie2999|Aurora|latest|Aurora-*-x86_64.AppImage.zsync"}
id=io.github.mollie2999.Aurora

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

tar -xzf "$archive" -C "$work"
src="$work/Aurora-Linux-x86_64"
appdir="$work/Aurora.AppDir"

# Keep steamhost, libVkLayer_aurora_overlay.so, and steamclient*.dll alongside
# the aurora binary so std::env::current_exe().parent() finds them natively.
install -d \
    "$appdir/opt/aurora" \
    "$appdir/usr/bin" \
    "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/256x256/apps" \
    "$appdir/usr/share/metainfo" \
    "$appdir/usr/share/licenses/aurora" \
    "$appdir/usr/share/doc/aurora"

install -m755 "$src/aurora" "$src/steamhost" "$src/libVkLayer_aurora_overlay.so" "$src/libaurora_htmlsurface.so" "$appdir/opt/aurora/"
if [[ -f "$src/libSDL3.so.0" ]]; then
    install -m755 "$src/libSDL3.so.0" "$appdir/opt/aurora/libSDL3.so.0"
fi
if [[ -f "$src/SDL3-LICENSE.txt" ]]; then
    install -m644 "$src/SDL3-LICENSE.txt" "$appdir/usr/share/licenses/aurora/SDL3-LICENSE.txt"
fi
for dll in steamclient64.dll steamclient32.dll; do
    if [[ -f "$src/$dll" ]]; then
        install -m644 "$src/$dll" "$appdir/opt/aurora/$dll"
    fi
done

ln -s ../../opt/aurora/aurora "$appdir/usr/bin/aurora"

install -m644 "$src/LICENSE" "$appdir/usr/share/licenses/aurora/LICENSE"
install -m644 "$src/README.md" "$appdir/usr/share/doc/aurora/README.md"
install -m644 "$src/icon-256.png" "$appdir/usr/share/icons/hicolor/256x256/apps/$id.png"
cp "$src/icon-256.png" "$appdir/$id.png"
ln -s "$id.png" "$appdir/.DirIcon"

desktop_src="$root/packaging/appimage/$id.desktop"
install -m644 "$desktop_src" "$appdir/usr/share/applications/$id.desktop"
if [[ -n "$version" ]]; then
    printf 'X-AppImage-Version=%s\n' "$version" >> "$appdir/usr/share/applications/$id.desktop"
fi
cp "$appdir/usr/share/applications/$id.desktop" "$appdir/$id.desktop"

install -m644 "$root/packaging/appimage/$id.appdata.xml" \
    "$appdir/usr/share/metainfo/$id.appdata.xml"

# Execute aurora directly without altering LD_LIBRARY_PATH so host Steam,
# Proton, Steamworks, and Vulkan overlay integration stay native.
cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
set -eu
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$HERE/opt/aurora/aurora" "$@"
EOF
chmod 755 "$appdir/AppRun"

mkdir -p -- "$(dirname -- "$output")"
out_dir=$(cd -- "$(dirname -- "$output")" && pwd)
out_name=$(basename -- "$output")
out_abs="$out_dir/$out_name"

# Run from out_dir so appimagetool writes the companion .zsync beside the AppImage.
(
    cd "$out_dir"
    ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$appimagetool" \
        -u "$update_info" \
        "$appdir" \
        "$out_abs"
)

if [[ ! -s "$out_abs" ]]; then
    echo "AppImage was not created: $out_abs" >&2
    exit 1
fi
# Under APPIMAGE_EXTRACT_AND_RUN=1, appimagetool 1.9.1 invokes zsyncmake from its
# ephemeral extract directory, so the .zsync file is discarded on exit. Generate
# it beside the AppImage whenever appimagetool did not leave one there.
if [[ ! -s "$out_abs.zsync" ]]; then
    zsyncmake -u "$out_name" -o "$out_abs.zsync" "$out_abs"
fi
if [[ ! -s "$out_abs.zsync" ]]; then
    echo "AppImage zsync metadata was not created: $out_abs.zsync" >&2
    exit 1
fi
