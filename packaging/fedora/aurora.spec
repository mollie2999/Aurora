# Package the Ubuntu 22.04 build without stripping or modifying its binaries.
%global __os_install_post %{nil}
%global _build_id_links none
%global debug_package %{nil}

Name:           aurora
Version:        %{aurora_version}
Release:        1
Summary:        Unofficial Steam client written in Rust
License:        MIT
URL:            https://github.com/mollie2999/Aurora
Source0:        Aurora-Linux-x86_64.tar.gz
ExclusiveArch:  x86_64
Requires:       libglvnd-glx, libxkbcommon, libxkbcommon-x11
Requires:       libX11, libXcursor, libXi, libXrandr, libwayland-client
Recommends:     (ffmpeg-free or ffmpeg)
Recommends:     pulseaudio-libs

%description
Aurora is a native Steam client with store, library, downloads, friends
and chat, and game launching with Steam features.

%prep
%setup -q -n Aurora-Linux-x86_64

%install
install -d "%{buildroot}/opt/aurora" "%{buildroot}%{_bindir}"
install -pm755 aurora steamhost libVkLayer_aurora_overlay.so libaurora_htmlsurface.so "%{buildroot}/opt/aurora/"
if [ -f libSDL3.so.0 ]; then
    install -pm755 libSDL3.so.0 "%{buildroot}/opt/aurora/"
fi
for dll in steamclient64.dll steamclient32.dll; do
    if [ -f "$dll" ]; then
        install -pm644 "$dll" "%{buildroot}/opt/aurora/"
    fi
done
ln -s ../../opt/aurora/aurora "%{buildroot}%{_bindir}/aurora"
install -Dm644 icon-256.png "%{buildroot}%{_datadir}/icons/hicolor/256x256/apps/aurora.png"
install -Dm644 /dev/stdin "%{buildroot}%{_datadir}/applications/aurora.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Aurora
GenericName=Steam client
Comment=Unofficial Steam client written in Rust
Exec=aurora
Icon=aurora
Terminal=false
Categories=Game;
StartupWMClass=SteamInRust
DESKTOP

%files
%license LICENSE
%doc README.md
/opt/aurora/
%{_bindir}/aurora
%{_datadir}/applications/aurora.desktop
%{_datadir}/icons/hicolor/256x256/apps/aurora.png
