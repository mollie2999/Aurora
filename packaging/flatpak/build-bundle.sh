#!/usr/bin/env bash
# Package the prebuilt x86_64 release archive. Run on an x86_64 host
# with Flatpak and org.freedesktop.Platform//25.08 installed.
set -euo pipefail

if [[ $# != 2 ]]; then
    echo "Usage: $0 LINUX_RELEASE_ARCHIVE OUTPUT.flatpak" >&2
    exit 2
fi

id=io.github.mollie2999.Aurora
platform=org.freedesktop.Platform
branch=25.08
runtime_repo=https://dl.flathub.org/repo/flathub.flatpakrepo
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

tar -xzf "$1" -C "$work"
src="$work/Aurora-Linux-x86_64"
build="$work/build"
# The platform doubles as the SDK here because the payload is prebuilt.
flatpak build-init --arch=x86_64 "$build" "$id" "$platform" "$platform" "$branch"
install -Dm755 "$src/aurora" "$build/files/bin/aurora"
install -Dm755 "$src/steamhost" "$build/files/bin/steamhost"
install -Dm755 "$src/libVkLayer_aurora_overlay.so" "$build/files/bin/libVkLayer_aurora_overlay.so"
install -Dm755 "$src/libaurora_htmlsurface.so" "$build/files/bin/libaurora_htmlsurface.so"
if [[ -f "$src/libSDL3.so.0" ]]; then
    install -Dm755 "$src/libSDL3.so.0" "$build/files/bin/libSDL3.so.0"
fi
if [[ -f "$src/SDL3-LICENSE.txt" ]]; then
    install -Dm644 "$src/SDL3-LICENSE.txt" "$build/files/share/licenses/$id/SDL3-LICENSE.txt"
fi
install -Dm644 "$src/LICENSE" "$build/files/share/licenses/$id/LICENSE"
install -Dm644 "$src/icon-256.png" \
    "$build/files/share/icons/hicolor/256x256/apps/$id.png"
install -Dm644 "$root/packaging/flatpak/$id.desktop" \
    "$build/files/share/applications/$id.desktop"
install -Dm644 "$root/packaging/flatpak/$id.metainfo.xml" \
    "$build/files/share/metainfo/$id.metainfo.xml"

# Steam lives outside the sandbox. Keep Aurora's files private and use
# desktop portals for opening links and selecting files.
devices=(--device=dri)
# Older build hosts can still package a keyboard/mouse build.
flatpak_version=$(flatpak --version)
flatpak_version=${flatpak_version#Flatpak }
if [ "$(printf '%s\n' "1.15.6" "$flatpak_version" | sort -V | head -n 1)" = 1.15.6 ]; then
    devices+=(--device=input)
fi
flatpak build-finish --command=aurora \
    --share=network --share=ipc "${devices[@]}" \
    --socket=wayland --socket=fallback-x11 --socket=pulseaudio "$build"
flatpak build-export --arch=x86_64 "$work/repo" "$build"
flatpak build-bundle --arch=x86_64 --runtime-repo="$runtime_repo" "$work/repo" "$2" "$id"
