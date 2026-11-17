<p align="center">
  <img src="docs/media/aurora-banner.png" alt="Aurora: your Steam library, none of the bloat" width="100%">
</p>

<p align="center">
  <a href="https://mollie2999.github.io/Aurora/"><b>Website</b></a> ·
  <a href="#download"><b>Download</b></a> ·
  <a href="#every-game-launches"><b>Games</b></a> ·
  <a href="#status"><b>Status</b></a> ·
  <a href="#benchmarks"><b>Benchmarks</b></a> ·
  <a href="CHANGELOG.md"><b>Changelog</b></a>
</p>

<p align="center">
  <a href="https://github.com/mollie2999/Aurora/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/mollie2999/Aurora?style=flat-square&color=F07A4A"></a>
  <img alt="Linux and Windows" src="https://img.shields.io/badge/Linux%20%C2%B7%20Windows-F07A4A?style=flat-square">
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-2FB36B?style=flat-square">
</p>

# Aurora

A Steam client written in Rust, with no browser inside. Store, library,
downloads, friends, chat, Workshop, inventory, Market and Steam Cloud, drawn
natively with [egui](https://github.com/emilk/egui) in one process. It signs in
to your own Steam account and talks to Steam's servers directly.

Aurora is a fork of [SteamInRust](https://github.com/madonchik123/SteamInRust)
that adds Linux: it builds and runs there (Wayland and X11) and starts every
kind of game in your library.

## Every game launches

On Linux, Aurora picks how to start each game:

| Game | How it starts | Steam features in the game |
| --- | --- | --- |
| Native Linux games | Directly | From your own Steam's `steamclient.so`, on a windowless Steam host |
| Windows games | Proton (Steam's default, or one you pick in Manage › Compatibility) | Aurora's own `steamclient`: achievements, stats, cloud, lobbies, Workshop |
| Steam DRM (SteamStub) | Proton, on the windowless Steam host | Valve's engine answers the DRM check itself |
| Anti-cheat (VAC, Easy Anti-Cheat, BattlEye) | Through the official Steam client, or on the windowless Steam host (experimental, Settings › Library) | Steam's own, from Valve's engine either way |

The windowless Steam host is your installed Steam's engine without its
interface: no `steamwebhelper`, no Chromium. It checks ownership itself, so it
can't start a game you don't own.

Games with both a Linux and a Windows build run the Linux build by default.
Manage › Compatibility switches a game to Proton or to a specific Proton
version.

Needs Steam installed (for Proton and the Steam host). Anti-cheat games also
need it signed in, unless they run on the windowless Steam host: that option
(Settings › Library, experimental) starts them without the official client.
The anti-cheat may count that as an unsupported setup and ban you.

On Windows, games run as in Aurora: Aurora's `steamclient` for most games,
the official client for anti-cheat.

## Status

Works: sign-in (password, Steam Guard, QR), store with trailers, library with
collections and non-Steam games, downloads, friends and chat, community pages,
Workshop, inventory and Market, Steam Cloud, offline mode, plugins. English and
Russian.

Missing:

- **Linux:** tray icon, `steam://` links, Shift+Tab overlay inside games.
- **Linux, known bug:** the first launch of a Windows game with a new Proton
  prefix can fail when the game needs Steam (Bloons TD 6 crashed with
  "NoSteamClient"). The second launch works.
- **Everywhere:** Big Picture, Remote Play, group chats and voice chat, writing
  reviews. Checkout and trade offers open Steam's website.

Area-by-area detail (from Aurora): [docs/parity.md](docs/parity.md).

## Benchmarks

Aurora 0.7.2 against the official Steam client (build 1792618285, no
mods), on the same PC and account: Ryzen 7 5700X, 16 GB RAM, GeForce RTX
5070, CachyOS with GNOME on Wayland. Measured on 16 November 2026. Each figure
is the median of 3 runs after a warm-up run.

| | Steam | Aurora |
| --- | ---: | ---: |
| Start → first window | 2.77 s | 0.14 s |
| Start → main window, signed in | 4.90 s | 2.16 s |
| Memory when idle (PSS, all its processes) | 1,408 MB | 166 MB |
| CPU when idle | 18 % of a core (9–33 %) | 0.2 % |
| Processes | 16 | 1 |
| Program size | 2,129 MB | 46 MB |

Steam signs in automatically; its first window is its sign-in window.
Aurora's first window is its account picker, which the script clicked as soon
as it showed. Idle figures are
taken 45 s after the main window opened, CPU over the next 30 s. Steam's
program size leaves out games, user data, caches and compatibility tools.
Aurora still needs a Steam install on Linux for Proton and the Steam host.

Method, script and every run: [docs/benchmarks-linux.md](docs/benchmarks-linux.md).

## Download

From [the latest release](https://github.com/mollie2999/Aurora/releases/latest):

| File | Platform |
| --- | --- |
| `Aurora-Linux-x86_64.tar.gz` | Linux x86_64. Unpack and run `aurora` |
| `aurora-bin-<version>-x86_64.pkg.tar.zst` | Arch Linux package (`sudo pacman -U …`) |
| `aurora_<version>_amd64.deb` | Debian, Ubuntu 22.04+, Mint (`sudo apt install ./aurora_…deb`) |
| `Aurora-Setup-<version>.exe` | Windows 10/11 installer, updates itself |
| `Aurora-<version>-portable.zip` | Windows, no installer |

Store trailers on Linux need `ffmpeg`, and `libpulse` for sound.

## Build from source

Needs [Rust](https://rustup.rs).

```sh
git clone https://github.com/mollie2999/Aurora.git
cd Aurora
cargo build --release -p aurora -p steamhost
target/release/aurora            # sign in with your account
target/release/aurora --demo     # sample data, no sign-in
```

The Windows `steamclient` DLLs that Proton games load are built by CI
(`.github/workflows/build.yml`); copy them next to `aurora` from a release.

## Is it safe to use?

- **Your account.** Aurora signs in the way the official client and the Steam
  Mobile app do. Your sign-in stays on your PC and is only sent to Steam.
- **Anti-cheat.** By default, anti-cheat games always run through Valve's
  client.
- **Other connections.** Steam, GitHub (updates), an exchange-rate service
  (gift regions) and, if you add a key, IsThereAnyDeal. No telemetry.

## Credits

Built on [SteamInRust](https://github.com/madonchik123/SteamInRust) by
madonchik123. Fonts: [Geist](https://github.com/vercel/geist-font) and
[Inter](https://rsms.me/inter/) (OFL). Icons: [Phosphor](https://phosphoricons.com) (MIT).
Protocol definitions: [SteamDatabase/Protobufs](https://github.com/SteamDatabase/Protobufs).

## License

[MIT](LICENSE). Unofficial. Not affiliated with Valve. Steam is a trademark of
Valve Corporation.
