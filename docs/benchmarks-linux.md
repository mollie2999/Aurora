# Linux benchmarks

Aurora 1.0.0-rc4 against the official Steam client on Linux, 25 November 2026.

## Setup

- AMD Ryzen 7 5700X, 16 GB RAM, NVIDIA GeForce RTX 5070 (driver 620.73.04)
- CachyOS, kernel `7.3.5-2-cachyos-bore`, GNOME on Wayland
- Steam client build `1792618285` (Arch `steam` `1.0.0.87-3`), updates off.
  The Millennium mod installed on this PC was unhooked for Steam's runs.
- Aurora `1.0.0-rc4` release build (`/opt/aurora` from `aurora-bin 1.0.0rc4-1`)
- Same Steam account, signed in automatically by both clients

## Summary (median of runs 1–3)

| Metric | Steam | Aurora 1.0.0-rc4 | Difference |
| --- | ---: | ---: | ---: |
| Start → first window | 3.41 s | 0.18 s | 19× faster |
| Start → main window, signed in | 5.55 s | 2.18 s | 2.5× faster |
| Memory when idle (PSS, all processes) | 1,637 MB | 150 MB | 10.9× less RAM |
| CPU when idle (% of a core) | 14.07 % | 0.33 % | 43× less CPU |
| Processes | 28 | 1 | Single process |
| Program size (excluding games/caches) | 2,217 MB | 63 MB | 35× smaller |

## Method

Script: [benchmarks-linux.sh](benchmarks-linux.sh). One client at a time, never both.
Each client ran 4 times; run 0 is a warm-up and isn't counted. Figures in the
README are the median of runs 1–3.

- **Start → first window / main window.** From launch to the window
  appearing. Aurora runs on XWayland here and is found with `xdotool`; its
  first window is the small "Signing in" dialog, which automatically transitions
  to the main window (`width > 700`). Steam's windows are native Wayland
  windows, so its own log line (`webhelper.txt`:
  `SP DesktopLoginWindow_uid0… WasHidden 0` for the sign-in window,
  `SP Desktop_uid0-'Steam': WasHidden 0` for the main window) is the signal,
  checked every 50 ms.
- **Memory.** 45 s after the main window opened: the sum of PSS
  (`/proc/<pid>/smaps_rollup`) over every process the client started,
  including Steam's container and runtime loggers.
- **CPU.** User + system time of those processes over the next 30 s, as a
  percentage of one core.
- **Processes.** The processes counted for memory.
- **Program size.** Steam: `~/.local/share/Steam` without `steamapps`,
  `userdata`, `appcache`, `depotcache`, `config`, `logs`, `package` (update
  downloads), `compatibilitytools.d`, `millennium`, and `steamui/skins`.
  Aurora: `/opt/aurora` (`aurora`, `steamhost`, `libVkLayer_aurora_overlay.so`,
  and the two `steamclient` DLLs).

## Every run

| Client | Run | First window (s) | Main window (s) | PSS (MB) | Processes | CPU (% of a core) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Steam | 0 (warm-up) | 4.03 | 6.05 | 1,854 | 31 | 44.90 |
| Steam | 1 | 3.41 | 5.55 | 1,637 | 28 | 14.07 |
| Steam | 2 | 5.33 | 7.14 | 1,596 | 28 | 16.90 |
| Steam | 3 | 2.99 | 5.18 | 1,754 | 35 | 12.17 |
| Aurora | 0 (warm-up) | 0.29 | 17.20 | 158 | 1 | 0.33 |
| Aurora | 1 | 0.17 | 1.84 | 149 | 1 | 0.40 |
| Aurora | 2 | 0.19 | 2.18 | 151 | 1 | 0.33 |
| Aurora | 3 | 0.18 | 2.70 | 150 | 1 | 0.20 |

## Not measured

- Memory while a game runs. Aurora starts the windowless Steam host for
  native and Steam-DRM games; Steam's own processes keep running beside a game.
- Page-switching speed, download speed, and Wayland-native Aurora (it ran on
  XWayland so the script could find its window).
