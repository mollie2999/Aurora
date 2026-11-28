# Changelog

## [1.0.0-rc6](https://github.com/mollie2999/Aurora/compare/v1.0.0-rc5...v1.0.0-rc6) (2026-11-28)

### New

* **View menu, Game Servers browser, Screenshots manager & Small Mode:** added a Steam-style **View** menu with **Small Mode**, a native **Game Servers** browser with filter controls, and a **Screenshots** manager, with translations. ([#111](https://github.com/mollie2999/Aurora/pull/111))
* **Chat copy, clickable links & media previews:** chat messages are now selectable and copyable, URLs are clickable, and shared media renders inline previews with a lightbox viewer. ([#110](https://github.com/mollie2999/Aurora/pull/110))
* **Metro theme & Steam Family preferred copy:** added the **Metro** theme (with Steam-style supernav spacing, readable links, and download control spacing) and a **Change Preferred Copy** option showing each Steam Family copy's DLC. ([#109](https://github.com/mollie2999/Aurora/pull/109), [#133](https://github.com/mollie2999/Aurora/pull/133))
* **Opt-in Steam-style profile layout & file filters:** added an opt-in Steam-style profile page with showcases and common connections, plus game, type, and sort filters on profile screenshots and shared files. ([#117](https://github.com/mollie2999/Aurora/pull/117), [#123](https://github.com/mollie2999/Aurora/pull/123))
* **Browse Proton prefix:** added a **Browse Proton prefix** option to the per-game Manage menu with translations. ([#137](https://github.com/mollie2999/Aurora/pull/137))
* **Desktop navigation & QoL:** added **Copy page URL** on Store, Wishlist, and Profile pages, browser-style **middle-click autoscroll** (automatically stopped when entering Big Picture), a **Smooth scrolling** setting, and an optional **60 fps cap** for the main window. ([#118](https://github.com/mollie2999/Aurora/pull/118), [#119](https://github.com/mollie2999/Aurora/pull/119), [#120](https://github.com/mollie2999/Aurora/pull/120), [#121](https://github.com/mollie2999/Aurora/pull/121))

### Fixes

* **Steam Deck, Big Picture OSK & unified SDL3 controllers:** unified host controller input on bundled SDL3 (removing `gilrs`, adding virtual Xbox pads, and fixing Windows XInput touchpad init), added the Big Picture on-screen keyboard with touchpad haptics, long-press, Tab cycling, and IME suppression, improved Steam Deck detection, DPI scaling, Quick Access navigation, and controller hints, and eliminated KWallet prompt spam. ([#87](https://github.com/mollie2999/Aurora/pull/87))
* **Hyprland fallback overlay:** added Hyprland workspace/Lua IPC support so the fallback overlay raises above fullscreen and floating games while preserving fullscreen state, and prevented a Wayland `BadSurface` crash on close. ([#108](https://github.com/mollie2999/Aurora/pull/108))
* **In-game presence & Invisible mode:** preserved your own in-game status while playing, announced in-game status over CM for HeadlessHost launches, and hid your current game while Invisible. ([#113](https://github.com/mollie2999/Aurora/pull/113), [#122](https://github.com/mollie2999/Aurora/pull/122), [#138](https://github.com/mollie2999/Aurora/pull/138))
* **Trusted Steam CDN over TUN / fake-IP proxies:** allowed trusted HTTPS Steam CDN hosts to load over TUN/fake-IP proxies while hardening SSRF checks. ([#136](https://github.com/mollie2999/Aurora/pull/136))
* **Library recent activity sort:** fixed the library clock button so it sorts by recent activity. ([#116](https://github.com/mollie2999/Aurora/pull/116))

### Credits

* Thanks to **[@liathy](https://github.com/liathy)** for contributing the Metro theme, Steam Family preferred copy selector, Steam-style profile layout, screenshots/shared files filters, middle-click autoscroll, smooth scrolling setting, 60 fps cap toggle, Copy page URL, recent-activity sort fix, and in-game/invisible status fixes ([#109](https://github.com/mollie2999/Aurora/pull/109), [#113](https://github.com/mollie2999/Aurora/pull/113), [#116](https://github.com/mollie2999/Aurora/pull/116), [#117](https://github.com/mollie2999/Aurora/pull/117), [#118](https://github.com/mollie2999/Aurora/pull/118), [#119](https://github.com/mollie2999/Aurora/pull/119), [#120](https://github.com/mollie2999/Aurora/pull/120), [#121](https://github.com/mollie2999/Aurora/pull/121), [#122](https://github.com/mollie2999/Aurora/pull/122), [#123](https://github.com/mollie2999/Aurora/pull/123)).

## [1.0.0-rc5](https://github.com/mollie2999/Aurora/compare/v1.0.0-rc4...v1.0.0-rc5) (2026-11-25)

### New

* **Official Linux AppImage:** added a portable x86-64 `.AppImage` release asset with embedded Gear Lever / AppImageUpdate `.zsync` update metadata for SteamOS, Bazzite, and immutable distros. ([#92](https://github.com/mollie2999/Aurora/pull/92))
* **Library collections:** added collapsible sidebar collections and a Steam-style collections manager for organizing your library. ([#98](https://github.com/mollie2999/Aurora/pull/98))
* **Bundled default themes:** bundled the **Adwaita** and **Aurora Desktop** themes directly with Aurora. ([#99](https://github.com/mollie2999/Aurora/pull/99))
* **Hide "What's new" shelf:** added a **Show What's new** toggle in Library settings (desktop and Big Picture) plus a shelf **Hide** action that hides the news shelf across default, custom-themed, and Big Picture views and stops fetching it. ([#97](https://github.com/mollie2999/Aurora/pull/97))

### Fixes

* **Headless `ISteamHTMLSurface` compatibility:** added `libaurora_htmlsurface.so` (`html_shim`) and `steamclient` support so `ISteamHTMLSurface::CreateBrowser` completes cleanly under the headless Steam host, fixing startup hangs in games like Microsoft Flight Simulator (2020). ([#104](https://github.com/mollie2999/Aurora/pull/104))
* **Play/Stop button state lag:** eliminated lag updating the Play/Stop button state when launching and exiting games. ([#94](https://github.com/mollie2999/Aurora/pull/94))
* **Flatpak packaging:** bundled `steamhost`, `libVkLayer_aurora_overlay.so`, and `libaurora_htmlsurface.so` with Flatpak releases and switched X11 access to `--socket=fallback-x11`. ([#91](https://github.com/mollie2999/Aurora/pull/91), [#103](https://github.com/mollie2999/Aurora/pull/103))

### Security, reliability & performance

* Hardened installscript handling, IPC (including Windows pipe spawning), launch paths, CDN/PICS, plugins, and auth; improved backup safety, depot key handling, Steam Cloud sync, installer replacement, and theme loading. ([#100](https://github.com/mollie2999/Aurora/pull/100), [#101](https://github.com/mollie2999/Aurora/pull/101))
* Hardened GNS/P2P, overlay queues/focus/GL cache, `steamclient` IPC, `steamhost`, video ranges/HLS/ffdec, image and store caches, updater, Flatpak sandbox permissions, and CI workflow permissions. ([#102](https://github.com/mollie2999/Aurora/pull/102), [#103](https://github.com/mollie2999/Aurora/pull/103))

## [1.0.0-rc4](https://github.com/mollie2999/Aurora/compare/v1.0.0-rc3...v1.0.0-rc4) (2026-11-24)

### New

* **Steam Deck Game Mode & Big Picture overhaul:**
  * Added an opt-in Steam Deck preview (`--steam-deck` / `--steam-deck=window`, `F2` for Quick Access, `F11` for fullscreen/windowed) alongside Gamescope and session diagnostics. ([#84](https://github.com/mollie2999/Aurora/pull/84))
  * Added a four-tab slide-over **Quick Access** drawer (**System**, **Performance**, **Notifications**, **Friends**) accessible via `Guide + A`, `[` / `]`, or `F2`, plus an upgraded **Main Menu** with account/clock/battery header, running-game controls, and Power submenu. ([#85](https://github.com/mollie2999/Aurora/pull/85))
  * Added native controller-first **Profile** and **Edit Profile** pages (backgrounds, animated avatars and frames, badges, card collections, games, reviews, groups, comments, and privacy/theme settings). ([#85](https://github.com/mollie2999/Aurora/pull/85))
  * Added a shared **On-Screen Keyboard** (English/Cyrillic), a comprehensive per-game **Manage** screen (Steam Cloud sync and conflict resolution, Betas, DLC, integrity verification, move, and uninstall), **Storage** and **System** settings with Linux battery/backlight/audio controls, Downloads queue reordering and history, and updated French, Spanish, and Russian translations. ([#85](https://github.com/mollie2999/Aurora/pull/85))
* **Controller Guide button:** pressing the Xbox, PlayStation, or other controller Guide button opens or restores fullscreen Big Picture while Aurora is signed in (including from the tray), while staying inactive when an Aurora-launched game is running and Aurora is unfocused. ([#83](https://github.com/mollie2999/Aurora/pull/83))
* **Headless Steam host default:** installed Steam games now use the windowless Steam host by default when available, with a per-game warning before the first headless launch of an anti-cheat game replacing the experimental Library toggle. ([#82](https://github.com/mollie2999/Aurora/pull/82))

### Performance

* Replaced Big Picture's unconditional 33 ms idle repaint loop with event-driven repaint scheduling, relaxed idle `gilrs` polling to 250 ms, cached host telemetry and Library/Chat layouts, and added an auto-hiding top bar. ([#85](https://github.com/mollie2999/Aurora/pull/85))

## [1.0.0-rc3](https://github.com/mollie2999/Aurora/compare/v1.0.0-rc2...v1.0.0-rc3) (2026-11-23)

### New

* **Big Picture mode:** browse your library and the Store, watch trailers, install and launch games, manage downloads, and use Friends, chat, and settings in a fullscreen interface designed for controllers. Open it from the toolbar, the app menu, the Guide button while Aurora is focused, or `aurora --bigpicture`. ([Aurora port](https://github.com/mollie2999/Aurora/commit/8dd8a67), [focus and navigation](https://github.com/mollie2999/Aurora/commit/7a069b2), [artwork and trailers](https://github.com/mollie2999/Aurora/commit/78d433e))
* **Steam Input layouts:** choose a per-game template, browse Steam Community Layouts, and edit bindings in a visual editor. The in-game overlay adds a controller panel, and supported games can read active bindings through Steam Input. ([Layout editor and game integration](https://github.com/mollie2999/Aurora/commit/8364e9e), [community layout search](https://github.com/mollie2999/Aurora/commit/5b40a17))
* **Wayland tray support:** Hide to Tray now removes the window from the compositor. Restoring it uses a Wayland activation token when available, and hiding releases UI resources. ([Tray changes](https://github.com/mollie2999/Aurora/commit/ad93260))
* **Experimental Linux Flatpak:** Aurora now ships a standalone x86-64 bundle. For host Steam, Proton, and in-game overlay integration, use one of the native packages. ([Flatpak packaging](https://github.com/mollie2999/Aurora/commit/e381b26))

### Fixes

* **Controllers on Windows:** improved XInput and Steam Input initialization, including controller handoff for games launched through the headless Steam host. ([Controller initialization](https://github.com/mollie2999/Aurora/commit/aaefa8c), [headless Steam Input routing](https://github.com/mollie2999/Aurora/commit/60a0ed9))
* **Linux Vulkan overlay:** improved mouse and keyboard capture under X11 and made input release safer when the overlay closes or loses focus. The renderer now keeps cleared textures alive until pending GPU work has finished. ([Input capture](https://github.com/mollie2999/Aurora/commit/917cdcf), [texture lifetime](https://github.com/mollie2999/Aurora/commit/11048c0))

### Performance

* Reduced work in the Vulkan and D3D12 overlays with bounded queues, less Vulkan lock contention, and a faster D3D12 command-queue path. Moved screenshot processing off the UI thread and reduced work in theme rendering and large file copies. ([Overlay changes](https://github.com/mollie2999/Aurora/commit/5616b66), [UI and storage changes](https://github.com/mollie2999/Aurora/commit/76b362f))

### Security and hardening

* Hardened overlay memory handling, installer and backup paths, Steam Input data, Workshop content, theme packages, social images and custom artwork, and game-server queries. ([Overlay](https://github.com/mollie2999/Aurora/pull/69), [client and installer](https://github.com/mollie2999/Aurora/pull/70), [themes, social, and game servers](https://github.com/mollie2999/Aurora/pull/72))

### Credits

* Thanks to **[@madonchik123](https://github.com/madonchik123)** for the SteamInRust work ported into this release. That includes [Big Picture mode](https://github.com/madonchik123/SteamInRust/commit/b1077e4), [friend pages](https://github.com/madonchik123/SteamInRust/commit/a6e9336), [Store trailers and reviews, the install screen, and library filters](https://github.com/madonchik123/SteamInRust/commit/64f2913), [chat and screenshot navigation](https://github.com/madonchik123/SteamInRust/commit/2bf61d4), the [Backspace fix](https://github.com/madonchik123/SteamInRust/commit/49d9ec6), and Big Picture changes adapted from the [chat and settings update](https://github.com/madonchik123/SteamInRust/commit/98c0c70) and [screenshot manager update](https://github.com/madonchik123/SteamInRust/commit/d7bcba4).

## [1.0.0-rc2](https://github.com/mollie2999/Aurora/compare/v1.0.0-rc1...v1.0.0-rc2) (2026-11-22)

### Faster

* Ranked download servers in parallel and moved depot decompression, verification, and file writes off the async workers, with SIMD `zlib-rs`, `lzma-rust2`, `ruzstd 0.9`, and `fast_image_resize` ([#67](https://github.com/mollie2999/Aurora/pull/67)).
* Reduced per-frame CPU/GPU work across the Library, Store, Wishlist, Points Shop, Hub, Friends, Themes, and In-Game Overlay; added bounded LRU caches, a 96 MiB offscreen texture budget, cached Linux font discovery, and image/heap purging when minimizing or hiding to tray ([#67](https://github.com/mollie2999/Aurora/pull/67)).

### Security & Hardening

* Hardened live broadcast FFmpeg URL validation & protocol whitelisting, theme install size/symlink limits, `0600` log file permissions, Discovery Queue follow handling, and Linux `evdev` debounce ([#66](https://github.com/mollie2999/Aurora/pull/66)).

### Fixes

* **Windows installer & auto-updater:** replaced fragile PowerShell shortcut creation with native Win32 COM (`IShellLinkW` / `IPersistFile`), `SHGetKnownFolderPath`, `SHChangeNotify`, and `App Paths` registration so the Start Menu shortcut is always created and repaired on update; fixed the auto-updater launching two instances of Aurora after an update ([#67](https://github.com/mollie2999/Aurora/pull/67)).
* **Live Steam Broadcasts:** switched to a single FFmpeg process with decoupled audio playback, 30 fps frame pacing, and a loopback HLS auth manifest rewriter for Akamai `cdn_auth_url_parameters` ([#67](https://github.com/mollie2999/Aurora/pull/67)).
* **Window chrome & Linux overlay:** added 12 px rounded corners for floating windows, removed the cramped top-bar back/forward arrows, added X11 focus-loss auto-ungrab to the Vulkan overlay, and fixed fail-closed Proton anti-cheat launch-option detection ([#67](https://github.com/mollie2999/Aurora/pull/67)).
* **Packaging & icons:** restored the Arch package (`.pkg.tar.zst`), added a Fedora `.rpm`, added a release-notes download table, dropped the duplicate `SteamInRust-Setup` installer asset, and updated Linux and Windows icons to the Aurora logo ([#67](https://github.com/mollie2999/Aurora/pull/67)).

## [1.0.0-rc1](https://github.com/mollie2999/Aurora/compare/v0.9.2...v1.0.0-rc1) (2026-11-22)

> **Release Candidate / Test Version:** Aurora `1.0.0-rc1` is a pre-1.0 test release to validate all new features across Linux and Windows before tagging `1.0.0`. The updater now supports SemVer prerelease ordering, so anyone on `1.0.0-rc1` will automatically be offered `1.0.0` when it ships. Please report any bugs with your `aurora.log` file from your settings folder!

### Highlights & New Features

* **Declarative Theme Engine, Native Adwaita & Steam Desktop Replica:** build and install custom `.aurora-theme` packages with box-model layouts, custom fonts/weight presets, and Rhai hooks — includes built-in **Native Adwaita** and **Steam Desktop Replica** themes, plus full authoring docs in `docs/THEMING.md` ([#56](https://github.com/mollie2999/Aurora/pull/56)).
* **Community Tab, Friends Activity Feed & Live Steam Broadcasts:** added the **Community** tab next to Library with **Friends activity** (status posting & filters), **Explore**, **Developers** news, and **Discussions** — plus native Linux **Steam Broadcasts** playback via `ffmpeg` + PulseAudio/PipeWire with audio, play/pause, mute/volume, `-10s`/`+10s` rewind, and a 30-second in-memory A/V DVR scrubber ([#62](https://github.com/mollie2999/Aurora/pull/62)).
* **Complete Store, Search Bar, Discovery Queue & Points Shop:** restored the live Store search bar with autocomplete suggestions, merged the Store sub-bar into game pages, added the interactive **Discovery Queue** walkthrough, **Points Shop** with live Steam Points balance, Browse & Review filters, and fixed Wishlist drag-to-reorder ([#63](https://github.com/mollie2999/Aurora/pull/63)).
* **In-Game Overlay Overhaul (Vulkan / Fullscreen + Features):** fixed the Linux Vulkan implicit layer for fullscreen DXVK/VKD3D games (including Helldivers 2) with swapchain texture replay, non-freezing X11 input shield, software cursor, and always-on Shift+Tab/Esc polling — plus **FPS counter HUD**, **F12 screenshots**, **persistent per-game notes**, and **in-game toasts** ([#57](https://github.com/mollie2999/Aurora/pull/57)).
* **Complete Steam Input & Controller Settings:** added the full Controller settings menu with live USB/Bluetooth hotplug & Refresh, Xbox/`evdev` rumble testing, deadzone/trigger calibration, live button/stick tester, and per-game Steam Input overrides ([#58](https://github.com/mollie2999/Aurora/pull/58)).
* **Move, Backup & Restore Games:** move installed games between Steam library folders and back up or restore game installations directly in Library Properties and Settings → Storage ([#60](https://github.com/mollie2999/Aurora/pull/60)).
* **Native Profile Editing:** edit your Steam display name, real name, summary, custom URL, location, avatar, frame, background, favorite badge, theme, and privacy settings natively in-app ([#61](https://github.com/mollie2999/Aurora/pull/61)).
* **Linux Polish (Dock Progress, CJK/Emoji Fonts & Native Notifications):** added Linux dock download progress (Unity/GNOME `LauncherEntry` D-Bus), system CJK & emoji fallback font discovery so non-Latin reviews/titles render cleanly, Steam Family desktop notification routing, and fixed the Library Stop button icon ([#59](https://github.com/mollie2999/Aurora/pull/59)).
* **Persistent App Logs (`aurora.log`):** mirrors app logs to `aurora.log` (and `aurora.old.log`) in the settings folder on both Windows and Linux for easier bug reports ([#65](https://github.com/mollie2999/Aurora/pull/65)).

### Fixes

* **Updater SemVer prereleases:** prerelease versions like `1.0.0-rc1` now sort strictly below `1.0.0`, enabling seamless `0.9.2 → 1.0.0-rc1 → 1.0.0` updates ([#64](https://github.com/mollie2999/Aurora/pull/64)).

### Credits

* Huge thanks to **[@madonchik123](https://github.com/madonchik123)** (`madonchik123/SteamInRust`) for the upstream **Community / Friends Activity Feed**, **Discovery Queue**, and **Points Shop** work ported and adapted in this release.

## [0.9.2](https://github.com/mollie2999/Aurora/compare/v0.9.1...v0.9.2) (2026-11-19)

### New

* **In-game overlay renderers:** added in-process overlay support for D3D12, D3D9, OpenGL, and Vulkan on Windows, plus a Linux Vulkan implicit layer for anti-cheat Proton games ([#55](https://github.com/mollie2999/Aurora/pull/55)) ([41b96e9](https://github.com/mollie2999/Aurora/commit/41b96e9)).
* **Steam group chats:** added Steam group chat support across Community, the Friends window, and the in-game overlay ([#51](https://github.com/mollie2999/Aurora/pull/51)) ([7c66fcc](https://github.com/mollie2999/Aurora/commit/7c66fcc)).
* **Steam Family & taskbar progress:** added native Steam Family management and Windows taskbar download progress ([#53](https://github.com/mollie2999/Aurora/pull/53)) ([c426636](https://github.com/mollie2999/Aurora/commit/c426636)).
* **In-game Steam APIs & SDL3 motion:** added `ISteamInventory` definitions, `ISteamUGC` publishing, `ISteamGameServer`, and SDL3 gyro/accelerometer motion support ([#54](https://github.com/mollie2999/Aurora/pull/54)) ([b776af5](https://github.com/mollie2999/Aurora/commit/b776af5)).
* **Bundled SDL3 (`3.4.18`):** `cargo xtask package` now bundles the official `SDL3.dll` (with `SDL3-LICENSE.txt` under the zlib license) by default so DualSense, Switch, and other non-Xbox controllers work out of the box ([3c050f3](https://github.com/mollie2999/Aurora/commit/3c050f3)).

### Security & hardening

* **Keyring & offline playtime:** added `0600` file-permission fallback/migration for keyring storage and hardened offline playtime tracking ([#52](https://github.com/mollie2999/Aurora/pull/52)) ([adcbb76](https://github.com/mollie2999/Aurora/commit/adcbb76)).

### Fixes

* **Aurora logo:** restored the `egui_extras` SVG loader for the Aurora logo ([#50](https://github.com/mollie2999/Aurora/pull/50)) ([2bd9422](https://github.com/mollie2999/Aurora/commit/2bd9422)).
* **Overlay & packaging:** fixed Linux Vulkan `BadWindow` handling on game exit, fullscreen fallback overlay on Linux, and enabled `Win32_Security` in `steamclient` for 32-bit Windows builds ([1eafa22](https://github.com/mollie2999/Aurora/commit/1eafa22)).

## [0.9.1](https://github.com/mollie2999/Aurora/compare/v0.9.0...v0.9.1) (2026-11-18)

**Aurora 0.9.1 is a critical security update.** It hardens game launches, downloads, networking, Steam Cloud, plugins, web links, local storage and IPC across Aurora and `steamhost`. Everyone should update as soon as possible.

### Security fixes

* **Remote game invites:** friend invites and join-game requests (`+connect` / Rich Presence) are kept separate from local Launch Options and validated strictly, so a remote invite cannot inject `VAR=val` environment variables or `%command%` wrappers ([#34](https://github.com/mollie2999/Aurora/pull/34)) ([d74b3b4](https://github.com/mollie2999/Aurora/commit/d74b3b47f9949e38c585e1de75f7d4933963fc28)).
* **Depot downloads and installs:** relative depot paths are checked to stay inside the target install or Workshop directory (blocking traversal and symlinks), CDN selection requires HTTPS, and downloaded depot chunks require valid SHA-1 checksums ([#35](https://github.com/mollie2999/Aurora/pull/35)) ([c9a8cd0](https://github.com/mollie2999/Aurora/commit/c9a8cd085a69758bb3648ae26203b1b046b5eb29)).
* **Legacy networking (`PseudoTcp`):** fixed wrap-around sequence comparisons using modular 32-bit arithmetic and added segment and buffer bounds checks against malformed peer packets ([#36](https://github.com/mollie2999/Aurora/pull/36)) ([2c93441](https://github.com/mollie2999/Aurora/commit/2c93441a01d36be1a97b81ca5146712ac3ccef51)).
* **Game Networking Sockets (`GNS`):** hardened unreliable fragment reassembly in SNP against out-of-bounds reads/writes, duplicate or overlapping fragments and integer overflow ([#37](https://github.com/mollie2999/Aurora/pull/37)) ([d18928b](https://github.com/mollie2999/Aurora/commit/d18928b2c3d936c3ce2d2ee675d3eeebd17177b8)).
* **Steam Cloud saves:** downloaded cloud files must match an OS-appropriate `SaveRule` (or stay confined inside `remote`), OS path roots (`%Win*`, `%Mac*`, `%Linux*`) only resolve on matching platforms, and symlinks along save paths are rejected ([#38](https://github.com/mollie2999/Aurora/pull/38)) ([5c3bfb4](https://github.com/mollie2999/Aurora/commit/5c3bfb4ec287c0e27ebbc4b660fefb86bf1a6bd8)).
* **Uninstall and download cleanup:** validates game folder names, rejects `..` traversal, confirms target directories resolve canonically inside the library's `common` folder, and unlinks symlinks or junctions directly without recursing into their targets ([#39](https://github.com/mollie2999/Aurora/pull/39)) ([7b51e60](https://github.com/mollie2999/Aurora/commit/7b51e60369b877bb050ccde1d084524afb2c5588)).
* **Plugin sandbox and permissions:** Rhai plugin scripts now run with module loading disabled (`no_module`) and widget nesting caps (`MAX_ROW_DEPTH`), and plugins requesting permissions (or adding new permissions on reload) require explicit approval before running ([#40](https://github.com/mollie2999/Aurora/pull/40)) ([9dd6320](https://github.com/mollie2999/Aurora/commit/9dd63207e9eeea257c4b5596ad0294d17e7a214a)).
* **Web links and file loader removal:** removed `egui_extras`'s local `FileLoader` so UI images cannot read arbitrary local files, typed clickable rich-text actions (`rich::Click`), and restricted external links and thumbnails to validated `http`/`https` URLs and YouTube IDs across hub, store, profile, support, overlay and updater screens ([#41](https://github.com/mollie2999/Aurora/pull/41)) ([ca2becf](https://github.com/mollie2999/Aurora/commit/ca2becfa0485360e8be86b0aef9dff536580a29e)).
* **Parsers, video playback and P2P memory bounds:** capped text and binary VDF recursion (`MAX_DEPTH = 64`), CM multi-unzip payloads (`64 MiB`), depot chunks (`32 MiB`) and manifests (`256 MiB`), MP4 dimensions (`4096`), sample tables, HLS playlists and video downloads (`256 MiB`), and per-peer P2P channels (`64`) and early packet queues (`512`) ([#42](https://github.com/mollie2999/Aurora/pull/42)) ([09d0e78](https://github.com/mollie2999/Aurora/commit/09d0e7895ec7134f2172078afcc23b3b08949d5d)).
* **DRM host (`steamhost`) and server list FFI:** `steamhost` validates `steamclient` object pointers and vtable memory (`VirtualQuery` on Windows, `/proc/self/maps` on Linux), bounds RTTI class name scanning, and zeroizes login tokens immediately on drop; `steamclient` server lists bound C rule/player string reads and avoid callbacks after release or cancellation ([#43](https://github.com/mollie2999/Aurora/pull/43)) ([c3a3a92](https://github.com/mollie2999/Aurora/commit/c3a3a921f14749164a2d4a0116102b11218afd4f)).
* **Local IPC and self-updater:** localhost link forwarding (`steam://` only) requires a CSPRNG token stored in private `0700`/`0600` storage with constant-time checks, timeouts and bounded reads; overlay/game IPC handshakes (`512` B / `64 KiB`) and HTTP/IPC bodies (`64 MiB`) are capped; and staged updates require a valid SHA-256 digest and safe setup filename ([#44](https://github.com/mollie2999/Aurora/pull/44)) ([b6cb922](https://github.com/mollie2999/Aurora/commit/b6cb922f3b4c47fcc486efac71c82e7265187f35)).
* **Web cookies and image cache SSRF protection:** the desktop image cache validates URLs and resolved DNS addresses to block loopback, private and link-local networks (including HTTP redirects) and caps image responses at `16 MiB`; Steam web cookies stay scoped to Steam web hosts with a CSPRNG `sessionid` and hardened URL/path validation for account, hub and support links ([#45](https://github.com/mollie2999/Aurora/pull/45)) ([bd02abd](https://github.com/mollie2999/Aurora/commit/bd02abd87701e9e96f5f9b17e8eaca98957a060c)).
* **HTTPS and integrity enforcement:** Steam Cloud uploads and downloads (with a `256 MiB` uncompressed limit), PICS info fetches (`32 MiB` / `64 MiB` caps) and legacy Workshop downloads (`512 MiB` cap) enforce HTTPS (including redirects) and mandatory SHA-1 verification ([#46](https://github.com/mollie2999/Aurora/pull/46)) ([12bc36b](https://github.com/mollie2999/Aurora/commit/12bc36bc88fef926995d269196570876df9f0dc5)).
* **Secret redaction and local file permissions:** auth tokens, passwords, Steam Guard blobs, trade tokens and activation keys are redacted (`[REDACTED]`) in debug output, and local account, offline, Cloud and `steam_settings` writes use private `0700`/`0600` permissions on Unix and exclusive unique temp files ([#47](https://github.com/mollie2999/Aurora/pull/47)) ([fc58a70](https://github.com/mollie2999/Aurora/commit/fc58a7023c78b7e87c6af251ce59eec5833e7993)).
* **Windows W^X trampolines and controller shared memory:** x86 vtable trampolines switch memory protection from writable to non-writable executable (`VirtualProtect` + `FlushInstructionCache`) instead of staying RWX, and controller shared memory uses random `Local\sir-pads-<pid>-<128-bit>` CSPRNG names validated on open ([#48](https://github.com/mollie2999/Aurora/pull/48)) ([529f402](https://github.com/mollie2999/Aurora/commit/529f402caeb5f76bef47384552fcbf755c4ed942)).
* **CPU loop and inline image decode limits:** capped GNS ACK blocks (`64`) and receive tracking (`4096`), fixed hanging on truncated HTML topic tags in the community hub, replaced backtracking Cloud glob matching with linear matching, and capped decoded inline images at `8192×8192` and `64 MiB` ([#49](https://github.com/mollie2999/Aurora/pull/49)) ([6b1ab15](https://github.com/mollie2999/Aurora/commit/6b1ab157362176bc4028bd67c8314de674c68f03)).

## [0.9.0](https://github.com/mollie2999/Aurora/compare/v0.8.1...v0.9.0)

Aurora 0.9.0 is a major release with a Linux in-game overlay, two new languages, reviews, trading and more control over your library.

### New

* **French and Spanish:** choose your language on first launch, or change it later in Settings. The language also applies to supported Steam store content and reviews ([9259a3b](https://github.com/mollie2999/Aurora/commit/9259a3bed7f624169a10fe11911252fbcbd0840b)).
* **Linux overlay:** press Shift+Tab to open the overlay inside Proton D3D11 games. Vulkan, OpenGL and native games use a separate overlay window ([db38113](https://github.com/mollie2999/Aurora/commit/db38113a78530f649c912d54d26127e57016f7be)) ([5e33767](https://github.com/mollie2999/Aurora/commit/5e33767d74cc53eeee698c293424a1b70dbbc320)).
* **Game reviews:** read player reviews from the library or store, filter by language, and write, edit or delete your own review ([6b566ac](https://github.com/mollie2999/Aurora/commit/6b566ace1c51b38b36db1dfe961cc175da9b9082)).
* **Trade offers:** offer items to a friend or someone with a Steam trade URL, or ask for theirs. Steam may ask you to confirm the offer in its mobile app. For now, an offer contains items from one game, one of each item ([ad9a8af](https://github.com/mollie2999/Aurora/commit/ad9a8afa6ea8e26068e5702d0697263e9b7ca3eb)).
* **Steam Cloud:** see a game’s cloud files, sizes and dates in Properties, and delete individual files or all of them after confirmation. Copies on this PC stay ([259e1fd](https://github.com/mollie2999/Aurora/commit/259e1fd413465f960284dfb3e2e14cdd1d6ac024)).
* **Collections:** collection buttons are back under the library filters. Collections, Favorites and Hidden sync with Steam, including changes made offline ([f266c1e](https://github.com/mollie2999/Aurora/commit/f266c1e381a4250789f8aa7849611414216f1381)).
* **Store recommendations:** see picks for you, games popular with friends, recommended specials, free and upcoming games, and picks in your favourite tags ([8cfc3b6](https://github.com/mollie2999/Aurora/commit/8cfc3b60d74486cd3d2e844a5b864849da62e6ae)).
* **Import non-Steam games:** bring shortcuts from the official Steam client into Aurora’s library. Games already added are left out ([80d4b14](https://github.com/mollie2999/Aurora/commit/80d4b147a60913cc9b8fedcd507a9f900eb5a988)).
* **Downloads:** reorder queued games by dragging, using the arrows or choosing Download next. Update now queues an update without leaving the downloads page ([4ef8132](https://github.com/mollie2999/Aurora/commit/4ef81323e92e51b2fa9cd00f9563997f06c784f4)) ([daeefa2](https://github.com/mollie2999/Aurora/commit/daeefa2808158437f3ebf3c79299c81bcfb9314b)).

### Fixes

* **Windows:** PlayStation and other non-Xbox controllers now work through Aurora’s XInput support in XNA/FNA games such as Celeste ([d415edb](https://github.com/mollie2999/Aurora/commit/d415edbc3a6eb1c366c47aef3c060c586aabf54d)).
* **Linux:** fixed an overlay helper window that could cause Aurora to close on Wayland, and improved Shift+Tab handling in games using raw keyboard input ([2eb06ae](https://github.com/mollie2999/Aurora/commit/2eb06ae6aee5545b5d0ec971bf63120ba518e609)) ([ec03d96](https://github.com/mollie2999/Aurora/commit/ec03d961080dca7ea30ec915f0669fc14eaf39ce)).

## [0.8.1](https://github.com/mollie2999/Aurora/compare/v0.8.0...v0.8.1) (2026-11-17)

### New

* Launch options work like Steam's `%command%`: `MANGOHUD=1 %command%` sets variables for the game, `gamemoderun mangohud %command% -dx11` runs it through a wrapper with arguments after it. Without `%command%`, everything stays a game argument as before ([#15](https://github.com/mollie2999/Aurora/pull/15)) ([e792e1b](https://github.com/mollie2999/Aurora/commit/e792e1b6f5a59e50af6150bd1940c4fda4c9b6bc))
* Linux: games with both a Windows and a Linux build ask which one to install (Windows with Proton stays the default); updates keep the one you have ([#17](https://github.com/mollie2999/Aurora/pull/17)) ([9a465f9](https://github.com/mollie2999/Aurora/commit/9a465f92bb2eaf5a6928e5faa963991c0d1a3ef8))
* Badges, reviews and groups open in Aurora instead of the browser: all badges with your level and XP, reviews with playtime and helpful votes, groups with their member counts ([#20](https://github.com/mollie2999/Aurora/pull/20)) ([920794b](https://github.com/mollie2999/Aurora/commit/920794ba191c70bcc5aab6cc4d2046f7f2d29049))
* Trading-card collections: a game badge opens its card set (which cards you have, how many, the badge so far, normal and foil), and a card in the inventory has a See set button ([#20](https://github.com/mollie2999/Aurora/pull/20)) ([5c2623c](https://github.com/mollie2999/Aurora/commit/5c2623c1f081984e14d10941c815d02fab3edb94)) ([f1c5bba](https://github.com/mollie2999/Aurora/commit/f1c5bbab4eec6230b354065a5cfce3d2117c3adf))

### Fixes

* Linux: a Proton game's first launch has Steam features (online, achievements). It used to get them only from the second launch; a short "Proton setup" now runs first ([#18](https://github.com/mollie2999/Aurora/pull/18)) ([6ba3137](https://github.com/mollie2999/Aurora/commit/6ba3137caaa26ff79cd834995ada1bc58796aae4))
* Linux: games with a launcher (Cities: Skylines' Paradox Launcher, War Thunder) no longer lose Steam when the launcher hands over: Aurora waits for everything the game started ([#23](https://github.com/mollie2999/Aurora/pull/23)) ([3b2ab7e](https://github.com/mollie2999/Aurora/commit/3b2ab7e9fb30afb2a185d010bd50193049c64d75))
* Linux: native games with a newer Steam SDK find Steam ("Steam is not running" before) ([#19](https://github.com/mollie2999/Aurora/pull/19)) ([f62bda6](https://github.com/mollie2999/Aurora/commit/f62bda6920cee652551245c3131dea1de3a5522c))
* Linux: games on a drive whose path has a colon (like `/mnt/usb-…-0:0-part1`) start; Unity games like Cities: Skylines quit with exit code 1 there ([#22](https://github.com/mollie2999/Aurora/pull/22)) ([53ea3cf](https://github.com/mollie2999/Aurora/commit/53ea3cff814572aa620aa98ae23d1e20faaf5f52))
* Linux: Linux builds installed by Aurora start ("Permission denied" before) ([#21](https://github.com/mollie2999/Aurora/pull/21)) ([890bfce](https://github.com/mollie2999/Aurora/commit/890bfce0587c9d89f3d0d6a05225a1cabba6dd96))
* Linux: a game with only its Windows files installed runs them with Proton by default instead of looking for the missing Linux build ("No such file or directory") ([#16](https://github.com/mollie2999/Aurora/pull/16)) ([958e3b6](https://github.com/mollie2999/Aurora/commit/958e3b6e72f70ef651c38d10b8eb0860f5d5602a))

## [0.8.0](https://github.com/mollie2999/Aurora/compare/v0.7.2...v0.8.0) (2026-11-17)

**SteamInRust is now Aurora everywhere:** the program is `aurora`, the Windows setup `Aurora-Setup`, the Linux archive `Aurora-Linux-x86_64`, the Arch package `aurora-bin` (it replaces `steamonrust-bin`). Your settings, sign-ins and caches stay where they are, and installed copies update and move over by themselves ([f87388a](https://github.com/mollie2999/Aurora/commit/f87388a5f5f79ad8a00c12bdd11a433650439e1e))

Thanks to [madonchik123](https://github.com/madonchik123) for the clipboard and font fixes below, ported from upstream SteamInRust.

### New

* Windows: PlayStation, Switch, Steam Controller and other non-Xbox controllers work in games, like with Steam. Aurora reads them (SDL3) and games see them as Xbox pads, through Steam Input and XInput; rumble works. Button prompts stay Xbox's, and there's no gyro yet ([cb79247](https://github.com/mollie2999/Aurora/commit/cb79247b22178dd21a9bc5de686d1b422fdf950a))
* Linux: a tray icon (KDE, GNOME with the AppIndicator extension, Cinnamon, XFCE…) with the desktop's own menu. Closing the window keeps Aurora running in the tray; the icon brings it back, also on Wayland ([920412e](https://github.com/mollie2999/Aurora/commit/920412ef2f0eceb86d7ae8d67479754e0e5d8226))
* Steam install scripts run on a game's first launch, like Steam's "Running install script": the registry keys and setup steps some games need to start (e.g. EasyAntiCheat). On Linux inside the game's Proton prefix ([dcb5676](https://github.com/mollie2999/Aurora/commit/dcb56760c73c282b9b4b49593d2d00ef19bacacf))
* Store pages and game news are saved on disk: a game opened before shows at once, also after a restart or offline ([57e1d4a](https://github.com/mollie2999/Aurora/commit/57e1d4a65ce560f6f25c2b1d6e448f83241e269d))
* A right-click Cut / Copy / Paste menu in text fields. By [madonchik123](https://github.com/madonchik123), from [upstream #40](https://github.com/madonchik123/SteamInRust/pull/40) ([67800bd](https://github.com/mollie2999/Aurora/commit/67800bd28541453e52576e3a0f1681ff9c2859a9))
* A new Aurora logo ([b5c3e31](https://github.com/mollie2999/Aurora/commit/b5c3e3161dc25cfcdd0639f53e341f9f71dad559))
* Debian/Ubuntu package (`aurora_<version>_amd64.deb`) next to the Arch one ([467aa30](https://github.com/mollie2999/Aurora/commit/467aa30309c6fa998948902bbf645bbffe2a9a89))

### Faster and lighter

* Clicking a game in the library no longer freezes the window for a moment: how the game launches is worked out in the background, and game and workshop manifests are no longer read every frame ([59bda3d](https://github.com/mollie2999/Aurora/commit/59bda3d2281341d734ddf9a43035841ea71cad1b))
* Pictures are decoded at the size they're shown, a few at a time: a store page's pictures take a quarter of the memory (44.6 → 11.7 MB on Hades), the library's less than half ([7ea2cd3](https://github.com/mollie2999/Aurora/commit/7ea2cd37a683e3243a673c3f0bb12dfcd2cdeccc))
* Signing in only fetches the apps that changed since last time, instead of the whole library ([1ab86fe](https://github.com/mollie2999/Aurora/commit/1ab86fe7e65658a3bf78b178a3449acaff5c1575))
* Pictures and Steam's web API load over HTTP/2, on one connection ([c094472](https://github.com/mollie2999/Aurora/commit/c094472aa41e1ce57a5dd299422100732514f007))
* Windows: the fallback fonts for Chinese, Japanese, Korean and other scripts are memory-mapped instead of read whole: about 70 MB less memory. By [madonchik123](https://github.com/madonchik123), from [upstream #57](https://github.com/madonchik123/SteamInRust/pull/57) ([c7e7c37](https://github.com/mollie2999/Aurora/commit/c7e7c37aeafd184f38e456d99595ea3d9586eefa))
* Memory no longer grows as you open game pages: about a third less after browsing the library ([57e1d4a](https://github.com/mollie2999/Aurora/commit/57e1d4a65ce560f6f25c2b1d6e448f83241e269d))

### Fixes

* Paste, copy and cut (Ctrl+V / Ctrl+C / Ctrl+X) did nothing in any text field. By [madonchik123](https://github.com/madonchik123), from [upstream #40](https://github.com/madonchik123/SteamInRust/pull/40) ([67800bd](https://github.com/mollie2999/Aurora/commit/67800bd28541453e52576e3a0f1681ff9c2859a9))
* Non-Steam games launch: on Linux, Windows programs run with Proton and others are made executable; on Windows, shortcuts, quoted paths and games that need admin rights work ([dcb5676](https://github.com/mollie2999/Aurora/commit/dcb56760c73c282b9b4b49593d2d00ef19bacacf))
* Notifications start with a capital letter: "A game from your wishlist is on sale" ([28f5aee](https://github.com/mollie2999/Aurora/commit/28f5aeec2c722c8ecb7d0d6298a7698df4c91e6a))
* Linux: the app no longer needs GTK ([920412e](https://github.com/mollie2999/Aurora/commit/920412ef2f0eceb86d7ae8d67479754e0e5d8226))

## [0.7.2](https://github.com/mollie2999/Aurora/compare/v0.7.1-1...v0.7.2) (2026-11-16)

### New

* Linux: store trailers play, with sound. Needs `ffmpeg` installed ([f3f4a0a](https://github.com/mollie2999/Aurora/commit/f3f4a0a7bf2f3641d740f67fa5f9185ee3a2eb0a))
* Linux: Manage > Compatibility picks the Proton version for one game ([19918af](https://github.com/mollie2999/Aurora/commit/19918afd0dcb9935de477cdf2eac0acac9fc3218))
* Linux: games with a Linux build get "Native" in Manage > Compatibility, selected by default. Before, the menu said these games used the default Proton, though they already ran natively ([6561add](https://github.com/mollie2999/Aurora/commit/6561add8115aa964c829654f3ed99e57bc5f8362))

### Fixes

* The game right-click menu was about 600 px wide; it now fits its items ([3a857d8](https://github.com/mollie2999/Aurora/commit/3a857d84edbb7e5a666640c9c9f264d888cfcb3f))
* Library sidebar: the filter buttons and Filters now line up with the search box ([eda24e1](https://github.com/mollie2999/Aurora/commit/eda24e15f120c750623d0cabaaf637207d4141b9))

## [0.7.1-1](https://github.com/mollie2999/Aurora/compare/v0.7.1...v0.7.1-1) (2026-11-16)

**The GitHub page is now [mollie2999/Aurora](https://github.com/mollie2999/Aurora).**

### Fixes

* Updates: the update check uses the renamed repository, and a revision release like 0.7.1-1 now counts as newer than 0.7.1. Before, installed copies never offered 0.6.3-1 to 0.6.3
* Arch package: releases with a revision (0.7.1-1) now get a package too (pkgver 0.7.1, pkgrel 2)

## [0.7.1](https://github.com/mollie2999/Aurora/compare/v0.7.0...v0.7.1) (2026-11-16)

**Linux: shaders like Steam, and much less memory for the windowless Steam host.**

### New

* Linux: games now use Steam's shader cache (`steamapps/shadercache/<game>`), like when Steam starts them. Before, shaders Steam had pre-cached for your GPU were never used ([0e06ee7](https://github.com/mollie2999/Aurora/commit/0e06ee745c093991f85c48e7345747cd29506cd2))
* Linux: shaders are compiled before a game starts, like Steam's "Processing Vulkan shaders": the Play button shows "Compiling shaders… N%" with a Skip button. It runs once per game, and again only after a driver or cache update ([0e06ee7](https://github.com/mollie2999/Aurora/commit/0e06ee745c093991f85c48e7345747cd29506cd2))
* Settings > Downloads: "Allow downloads during gameplay" (off by default), shader pre-caching, and compiling shaders before a game starts. SteamInRust puts these into Steam's own settings before every game while Steam is closed; while it compiles shaders itself, Steam's background Vulkan shader processing is turned off so the work isn't done twice ([0e06ee7](https://github.com/mollie2999/Aurora/commit/0e06ee745c093991f85c48e7345747cd29506cd2))

### Fixes

* Linux: the windowless Steam host used 350–475 MB instead of about 150 MB while a game ran: with Steam's "Allow downloads during gameplay" on, it kept starting game updates and shader downloads in the background ([0e06ee7](https://github.com/mollie2999/Aurora/commit/0e06ee745c093991f85c48e7345747cd29506cd2))

## [0.7.0](https://github.com/mollie2999/Aurora/compare/v0.6.3...v0.7.0) (2026-11-16)

**Mostly Linux fixes.** Nothing changes for Windows in this release, so updating there is optional.

### New

* Arch Linux package on the release page: `sudo pacman -U steamonrust-bin-0.7.0-1-x86_64.pkg.tar.zst` (installs to `/opt/steamonrust`, adds an Aurora menu entry and the `steamonrust` command)
* Linux, experimental: Settings > Library can start anti-cheat games (Easy Anti-Cheat, BattlEye…) on the windowless Steam host instead of the official client. Off by default, and turning it on first warns about the risks (kicks, game or VAC bans). Tried with Helldivers 2 ([cd285ef](https://github.com/mollie2999/Aurora/commit/cd285ef1fa27bd446f443718d5f8fd887e936f19))

### Fixes

* Linux (X11): the window froze ("not responding") right after it opened, on the sign-in screen, e.g. on Linux Mint Cinnamon. The Steam Guard code prompt, the nickname dialog and the collection name dialog could freeze the same way. First shipped in 0.6.3-1 ([f9cd0e2](https://github.com/mollie2999/Aurora/commit/f9cd0e23c5bfe5029939c271648a1ad056b04e21))
* Linux: a game with Proton forced in Steam but only its Linux build installed (e.g. Dead Island Riptide Definitive Edition) started through the official Steam client; it now runs its Linux build on the windowless Steam host ([e27fa2b](https://github.com/mollie2999/Aurora/commit/e27fa2b264af2fc726addb3b757c15b998054b81))

## [0.5.0](https://github.com/madonchik123/SteamInRust/compare/v0.4.1...v0.5.0) (2026-11-10)


### New

* Steam-style cart: one Add to Cart, an Added to your cart popup, gifts per copy ([#35](https://github.com/madonchik123/SteamInRust/issues/35)) ([0b010ce](https://github.com/madonchik123/SteamInRust/commit/0b010ce1d757acbed42491d17b139fb56417d4df))
* Steam-style discount price tags ([#33](https://github.com/madonchik123/SteamInRust/issues/33)) ([f748477](https://github.com/madonchik123/SteamInRust/commit/f748477237465e2a68bb2e2fd441b94b9f31c302))


### Fixes

* cancelling a download frees its disk space ([#36](https://github.com/madonchik123/SteamInRust/issues/36)) ([b147abe](https://github.com/madonchik123/SteamInRust/commit/b147abe9cc01ee0c5ff4725632b5ad334dfb89b9))
* downloads, updates and verify work like Steam ([#37](https://github.com/madonchik123/SteamInRust/issues/37)) ([7755bb9](https://github.com/madonchik123/SteamInRust/commit/7755bb9df9e67842201a59536b0cfdb717ea69dd))
* library filters fit, cart names, Back in the store, free games, tag search ([#32](https://github.com/madonchik123/SteamInRust/issues/32)) ([42e188b](https://github.com/madonchik123/SteamInRust/commit/42e188b410c131fcf1a08a0c95f455016ce5fe2c))

## [0.4.1](https://github.com/madonchik123/SteamInRust/compare/v0.4.0...v0.4.1) (2026-11-09)


### Fixes

* readable update dialog, hover states in menus, no update prompts for builds from source ([#30](https://github.com/madonchik123/SteamInRust/issues/30)) ([8a60364](https://github.com/madonchik123/SteamInRust/commit/8a60364f8b570e21c5512071e0fa0a7df77066dd))

## [0.4.0](https://github.com/madonchik123/SteamInRust/compare/v0.3.0...v0.4.0) (2026-11-08)


### New

* compact Workshop filters; items download when subscribed ([#17](https://github.com/madonchik123/SteamInRust/issues/17)) ([7562a6d](https://github.com/madonchik123/SteamInRust/commit/7562a6d30e28e0814f6cf5b2d9c45cfe01bd1056))
* ISteamNetworkingSockets / Messages on Valve's protocol (certs, rendezvous, SNP, ICE) ([#22](https://github.com/madonchik123/SteamInRust/issues/22)) ([417a283](https://github.com/madonchik123/SteamInRust/commit/417a283eae3740a51475a6c5d73cb1a9b087424b))
* live player count on game pages; easy 32-bit DLL for dev builds ([#14](https://github.com/madonchik123/SteamInRust/issues/14)) ([19cebea](https://github.com/madonchik123/SteamInRust/commit/19cebea4a5cdf19bc230e2b9650cc37bf679ed96))
* native account details, purchase history, licenses and Steam Family ([#18](https://github.com/madonchik123/SteamInRust/issues/18)) ([1a119b5](https://github.com/madonchik123/SteamInRust/commit/1a119b59390b5773733f87f997377fe090820e86))
* P2P with official-client players (ISteamNetworking over Valve's libjingle protocol) ([#21](https://github.com/madonchik123/SteamInRust/issues/21)) ([57a2f56](https://github.com/madonchik123/SteamInRust/commit/57a2f56fd64b18594f00e8ea1c283c7f655c8ba5))
* real Steam services in our steamclient (sockets, Workshop, inventory, server browser, screenshots, Steam Input) ([#20](https://github.com/madonchik123/SteamInRust/issues/20)) ([4ab1c87](https://github.com/madonchik123/SteamInRust/commit/4ab1c87a2312b247dee2475df014f085ce600514))
* Shift+Tab overlay drawn inside the game; Steam-style invites in chat ([#19](https://github.com/madonchik123/SteamInRust/issues/19)) ([62b93ea](https://github.com/madonchik123/SteamInRust/commit/62b93eaaa61de5624e2eb3e6dcfe860d5b873bd2))


### Fixes

* 32-bit games on older Steamworks SDKs crashed at start (Human Fall Flat) ([#15](https://github.com/madonchik123/SteamInRust/issues/15)) ([4b9dd49](https://github.com/madonchik123/SteamInRust/commit/4b9dd49aa9e2ce9098c9420326f6a7b80e7f2655))
* uninstalling no longer freezes or crashes the app ([#23](https://github.com/madonchik123/SteamInRust/issues/23)) ([cf676e8](https://github.com/madonchik123/SteamInRust/commit/cf676e8103fbf43371bb491114263aa7e3195121))


### Faster

* library images no longer fetch their fallback while loading ([#26](https://github.com/madonchik123/SteamInRust/issues/26)) ([eacdedd](https://github.com/madonchik123/SteamInRust/commit/eacdedd9c1c18e6f71bbab59437f4043f91772ad))
* the library shows at once instead of "Loading your games" ([#27](https://github.com/madonchik123/SteamInRust/issues/27)) ([b2d7170](https://github.com/madonchik123/SteamInRust/commit/b2d71701990a1cfa97a0499ab89df9da4223b307))

## [0.3.0](https://github.com/madonchik123/SteamInRust/compare/v0.2.0...v0.3.0) (2026-11-08)


### New

* install states, download activity, friends window and tray polish ([#10](https://github.com/madonchik123/SteamInRust/issues/10)) ([29d9fa1](https://github.com/madonchik123/SteamInRust/commit/29d9fa114ec9c2a4d1bea441aa8f0fa19f1a705f))
* online play in our steamclient (lobbies, P2P, 32-bit games, leaderboards, HTTP) ([#11](https://github.com/madonchik123/SteamInRust/issues/11)) ([e289050](https://github.com/madonchik123/SteamInRust/commit/e289050fcdf4778fa8c5f07651ba212e0d4043db))
* Steam-style Friends & Chat menu, own title bar; fix stuck downloads ([#9](https://github.com/madonchik123/SteamInRust/issues/9)) ([70f4eba](https://github.com/madonchik123/SteamInRust/commit/70f4eba0348a9dd8445e81ed88f35ee3a3ac04b2))


### Fixes

* crash from a negative progress bar width during downloads ([#7](https://github.com/madonchik123/SteamInRust/issues/7)) ([d2e3a3e](https://github.com/madonchik123/SteamInRust/commit/d2e3a3e55f9b6b4bafbb6b0ad218bdcfe7688cca))

## [0.2.0](https://github.com/madonchik123/SteamInRust/compare/v0.1.1...v0.2.0) (2026-11-08)


### New

* gifts for games you own, picture cache, store polish ([#5](https://github.com/madonchik123/SteamInRust/issues/5)) ([04f4c4c](https://github.com/madonchik123/SteamInRust/commit/04f4c4cc9c1ddddca75ca47b6d2203dfa254e3f9))

## [0.1.1](https://github.com/madonchik123/SteamInRust/compare/v0.1.0...v0.1.1) (2026-11-07)


### Fixes

* single slashes in Steam Cloud file names for Auto-Cloud folders ([af8a0ac](https://github.com/madonchik123/SteamInRust/commit/af8a0ac4656953171c046c48938dbebb24c40d03))
