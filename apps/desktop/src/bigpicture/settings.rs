//! Big Picture & Steam Deck settings: couch- and handheld-friendly sections on
//! the left, scrollable settings rows on the right. Left/Right step choices
//! (returning to the section rail from the leftmost value or non-choice rows),
//! A toggles/advances, and slow filesystem/hardware queries are TTL-cached.

use super::focus::{Dir, Place};
use super::{BigPicture, ButtonHintMode, Frame, Page, paint};
use crate::builtin::downloads::{DownloadSettings, Downloads};
use crate::builtin::settings::{AppFlags, FpsCorner};
use client::Command;
use egui::{Align2, Color32, Id, Rect, Ui, pos2, vec2};
use extensions::Action;
use i18n::t;
use model::PersonaState;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use ui::{icons, widgets as w};

#[derive(Clone)]
struct StorageEntry {
    path: PathBuf,
    free_bytes: Option<u64>,
    games: usize,
    used_bytes: u64,
}

/// What Big Picture's settings remember while open (including cached probes).
#[derive(Default)]
pub struct SettingsState {
    section: usize,
    /// Installed themes (cached per visit).
    themes: Option<Vec<(Option<String>, String)>>,
    /// Cached storage probe (library free space + picture cache size).
    storage_cache: Option<(Instant, Vec<StorageEntry>, u64)>,
    /// In-flight background storage probe result receiver.
    storage_rx: Option<std::sync::mpsc::Receiver<(Vec<StorageEntry>, u64)>>,
    /// Cached controller detection snapshot (avoids per-frame polling).
    controller_cache: Option<(Instant, client::DetectionSnapshot)>,
    /// Cached Proton builds (Linux).
    #[cfg(target_os = "linux")]
    proton_cache: Option<(Instant, Vec<(String, String)>)>,
    /// "Sign out" was pressed once: needs a second press before this time.
    confirm_until: f64,
    last_focus: Option<Id>,
    /// Semantic IDs of Choice rows that can still step left (don't jump to nav rail).
    choice_can_step_left: std::collections::HashSet<Id>,
    /// Semantic IDs of all Choice rows (for Right stepping).
    choice_ids: std::collections::HashSet<Id>,
    /// Pending step delta (-1 or +1) for a focused Choice row this frame.
    pending_step: Option<(Id, i32)>,
}

impl SettingsState {
    /// Open on this section (previews, Quick Access shortcuts).
    pub fn open_at(&mut self, section: usize) {
        self.section = section.min(sections().len().saturating_sub(1));
        self.storage_cache = None;
        self.storage_rx = None;
        self.controller_cache = None;
        self.themes = None;
        #[cfg(target_os = "linux")]
        {
            self.proton_cache = None;
        }
    }
}

pub(super) const GENERAL: usize = 0;
pub(super) const DOWNLOADS: usize = 1;
pub(super) const STORAGE: usize = 2;
pub(super) const LIBRARY: usize = 3;
pub(super) const NOTIFICATIONS: usize = 4;
pub(super) const CONTROLLER: usize = 5;
pub(super) const OVERLAY: usize = 6;
pub(super) const APPEARANCE: usize = 7;
pub(super) const ACCOUNT: usize = 8;
#[cfg(target_os = "linux")]
pub(super) const COMPATIBILITY: usize = 9;
#[cfg(target_os = "linux")]
pub(super) const SYSTEM: usize = 10;
#[cfg(not(target_os = "linux"))]
pub(super) const SYSTEM: usize = 9;

const SIZES: [f32; 5] = [0.85, 1.0, 1.15, 1.3, 1.5];
const SCROLL_SPEEDS: [f32; 7] = [0.75, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0];
const SPEEDS_KBPS: [u32; 7] = [1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000];
const CONNECTIONS: [u32; 6] = [4, 8, 16, 24, 32, 48];
const STATUSES: [PersonaState; 5] = [
    PersonaState::Online,
    PersonaState::Away,
    PersonaState::Busy,
    PersonaState::Invisible,
    PersonaState::Offline,
];
const CORNERS: [FpsCorner; 5] = [
    FpsCorner::Off,
    FpsCorner::TopLeft,
    FpsCorner::TopRight,
    FpsCorner::BottomLeft,
    FpsCorner::BottomRight,
];

type Act = Box<dyn FnOnce(&mut BigPicture, &mut Frame, &egui::Context)>;
type StepAct = Box<dyn FnOnce(&mut BigPicture, &mut Frame, &egui::Context, i32)>;

fn act(f: impl FnOnce(&mut BigPicture, &mut Frame, &egui::Context) + 'static) -> Act {
    Box::new(f)
}

fn step_act(f: impl FnOnce(&mut BigPicture, &mut Frame, &egui::Context, i32) + 'static) -> StepAct {
    Box::new(f)
}

enum Row {
    Toggle {
        key: String,
        label: String,
        note: String,
        on: bool,
        enabled: bool,
        act: Act,
    },
    Choice {
        key: String,
        label: String,
        note: String,
        value: String,
        at_start: bool,
        at_end: bool,
        step: StepAct,
    },
    Button {
        key: String,
        icon: &'static str,
        label: String,
        danger: bool,
        checked: bool,
        act: Act,
    },
    Info {
        label: String,
        value: String,
    },
}

fn toggle(key: impl Into<String>, label: &str, note: &str, on: bool, a: Act) -> Row {
    Row::Toggle { key: key.into(), label: label.into(), note: note.into(), on, enabled: true, act: a }
}

fn toggle_enabled(key: impl Into<String>, label: &str, note: &str, on: bool, enabled: bool, a: Act) -> Row {
    Row::Toggle { key: key.into(), label: label.into(), note: note.into(), on, enabled, act: a }
}

fn choice(
    key: impl Into<String>,
    label: &str,
    note: &str,
    value: String,
    at_start: bool,
    at_end: bool,
    step: StepAct,
) -> Row {
    Row::Choice { key: key.into(), label: label.into(), note: note.into(), value, at_start, at_end, step }
}

/// Step through a list by `delta`: `-1` clamps at 0, `+1` wraps on A (or steps forward).
pub(crate) fn step_index(len: usize, current: usize, delta: i32, wrap_forward: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if delta < 0 {
        current.saturating_sub(1)
    } else if current + 1 < len {
        current + 1
    } else if wrap_forward {
        0
    } else {
        len - 1
    }
}

fn step_in<T: PartialEq + Copy>(list: &[T], now: T, delta: i32) -> T {
    let idx = list.iter().position(|x| *x == now).unwrap_or(0);
    list[step_index(list.len(), idx, delta, delta > 1)]
}

fn step_f32(list: &[f32], now: f32, delta: i32) -> f32 {
    let idx = list
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - now).abs().total_cmp(&(*b - now).abs()))
        .map(|(i, _)| i)
        .unwrap_or(0);
    list[step_index(list.len(), idx, delta, delta > 1)]
}

/// Flip one of the app's on/off settings.
fn flag(get: fn(&mut AppFlags) -> &mut bool) -> Act {
    act(move |_, f, _| {
        let flags = f.app.shared_flags();
        let mut l = flags.lock().unwrap();
        let v = get(&mut l);
        *v = !*v;
    })
}

fn controller_change(change: impl FnOnce(&mut client::ControllerSettings) + 'static) -> Act {
    act(move |_, _, _| {
        let mut settings = client::controller_settings();
        change(&mut settings);
        client::set_controller_settings(settings);
    })
}

fn controller_toggle(key: &str, label: &str, value: bool, field: fn(&mut client::ControllerSettings) -> &mut bool) -> Row {
    toggle(
        key,
        label,
        "",
        value,
        controller_change(move |settings| {
            let current = field(settings);
            *current = !*current;
        }),
    )
}

fn deadzone(key: &str, label: &str, value: f32, field: fn(&mut client::PadCalibration) -> &mut f32) -> Row {
    let cur = (value * 20.0).round().clamp(0.0, 10.0) as usize;
    choice(
        key,
        label,
        t!("Left/Right adjusts by 5% (0%–50%)."),
        format!("{}%", cur * 5),
        cur == 0,
        cur == 10,
        step_act(move |_, _, _, delta| {
            let mut settings = client::controller_settings();
            let v = field(&mut settings.calibration);
            let now = (*v * 20.0).round().clamp(0.0, 10.0) as usize;
            let next = step_index(11, now, delta, delta > 1);
            *v = next as f32 / 20.0;
            client::set_controller_settings(settings);
        }),
    )
}

fn with_downloads<R>(f: &mut Frame, g: impl FnOnce(&mut Downloads, &mut extensions::Ctx) -> R) -> Option<R> {
    let app = &mut *f.app;
    let env = extensions::Env {
        state: &app.state,
        local: &app.local,
        theme: &app.prefs.theme,
        zoom: app.prefs.zoom,
        client: &app.client,
        actions: &mut *f.actions,
    };
    app.registry.with_ext::<Downloads, R>("downloads", env, g)
}

/// Change the download settings (the Downloads page applies them).
fn dl(change: impl FnOnce(&mut DownloadSettings) + 'static) -> Act {
    act(move |_, f, _| {
        with_downloads(f, |d, cx| {
            let mut s = d.tv_settings();
            change(&mut s);
            d.tv_apply(cx, s);
        });
    })
}

fn sections() -> Vec<(&'static str, &'static str)> {
    let mut list = vec![
        (icons::MONITOR, t!("General")),
        (icons::DOWNLOAD, t!("Downloads")),
        (icons::DRIVES, t!("Storage")),
        (icons::BOOKS, t!("Library")),
        (icons::BELL, t!("Notifications")),
        (icons::CONTROLLER, t!("Controller")),
        (icons::STACK, t!("In-game overlay")),
        (icons::PALETTE, t!("Appearance")),
        (icons::USER_CIRCLE, t!("Account")),
    ];
    #[cfg(target_os = "linux")]
    list.push((icons::WINDOWS, t!("Compatibility")));
    list.push((icons::INFO, t!("System")));
    list
}

fn level_name(level: u8) -> &'static str {
    match level {
        0 => t!("Low"),
        2 => t!("High"),
        _ => t!("Normal"),
    }
}

fn speed_name(level: u8) -> &'static str {
    match level {
        0 => t!("Slow"),
        2 => t!("Fast"),
        _ => t!("Normal"),
    }
}

fn hour(h: u8) -> String {
    format!("{h:02}:00")
}

/// What the library opens on: 0 installed, 1 runs here, 2 everything.
fn library_name(v: u8) -> &'static str {
    match v {
        1 => t!("Runs on this computer"),
        2 => t!("All games"),
        _ => t!("Installed"),
    }
}

fn start_storage_probe(f: &Frame, ctx: egui::Context) -> std::sync::mpsc::Receiver<(Vec<StorageEntry>, u64)> {
    let libraries = f.app.local.libraries.clone();
    let game_stats: Vec<(PathBuf, u64)> = f
        .app
        .local
        .games
        .iter()
        .map(|g| (g.library.clone(), g.size_on_disk))
        .collect();
    let (tx, rx) = std::sync::mpsc::channel();
    f.app._rt.spawn_blocking(move || {
        let mut entries = Vec::with_capacity(libraries.len());
        for lib in libraries {
            let mut games = 0usize;
            let mut used_bytes = 0u64;
            for (g_lib, size) in &game_stats {
                if *g_lib == lib {
                    games += 1;
                    used_bytes = used_bytes.saturating_add(*size);
                }
            }
            entries.push(StorageEntry {
                free_bytes: local::manage::available_space(&lib),
                path: lib,
                games,
                used_bytes,
            });
        }
        let cache = crate::image_cache::size();
        let _ = tx.send((entries, cache));
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    });
    rx
}

fn rows(bp: &mut BigPicture, f: &mut Frame, ctx: &egui::Context) -> Vec<Row> {
    let screen = ctx.content_rect().size();
    let flags = *f.app.shared_flags().lock().unwrap();
    match bp.settings.section {
        DOWNLOADS => {
            let (s, regions) = with_downloads(f, |d, cx| (d.tv_settings(), d.tv_regions(cx))).unwrap_or_default();
            let region = if s.region == "auto" {
                t!("Automatic (recommended)").to_string()
            } else {
                regions.iter().find(|(id, _)| *id == s.region).map(|(_, n)| n.clone()).unwrap_or_else(|| s.region.clone())
            };
            let ids: Vec<String> = std::iter::once("auto".to_string()).chain(regions.into_iter().map(|(id, _)| id)).collect();
            let at = ids.iter().position(|i| *i == s.region).unwrap_or(0);
            let ids_for_step = ids.clone();
            let mut out = vec![
                toggle(
                    "dl-auto-updates",
                    t!("Automatic updates"),
                    t!("Keep installed games up to date. Turn off to update only when you choose (or when you launch a game)."),
                    flags.auto_updates,
                    flag(|x| &mut x.auto_updates),
                ),
                toggle(
                    "dl-while-playing",
                    t!("Allow downloads while playing"),
                    t!("Pause Aurora's own downloads automatically while a game is running."),
                    s.while_playing,
                    dl(|d| d.while_playing = !d.while_playing),
                ),
                toggle(
                    "dl-steam-gameplay",
                    t!("Allow downloads during gameplay"),
                    t!("Let Steam update other games and download shaders while you play. Off uses less memory."),
                    client::gameplay_downloads(),
                    act(|_, _, _| {
                        client::set_gameplay_downloads(!client::gameplay_downloads());
                        client::apply_steam_prefs();
                    }),
                ),
                choice(
                    "dl-region",
                    t!("Download region"),
                    t!("Measured from this PC: the closest servers usually download fastest."),
                    region,
                    at == 0,
                    at + 1 >= ids.len(),
                    step_act(move |_, f, _, delta| {
                        let next_idx = step_index(ids_for_step.len(), at, delta, delta > 1);
                        let chosen = ids_for_step[next_idx].clone();
                        with_downloads(f, |d, cx| {
                            let mut st = d.tv_settings();
                            st.region = chosen;
                            d.tv_apply(cx, st);
                        });
                    }),
                ),
                toggle("dl-limit-bw", t!("Limit bandwidth"), "", s.limit_bandwidth, dl(|d| d.limit_bandwidth = !d.limit_bandwidth)),
            ];
            if s.limit_bandwidth {
                let speed_idx = SPEEDS_KBPS.iter().position(|x| *x == s.limit_kbps).unwrap_or(0);
                let shown = format!("{:.1} MB/s", s.limit_kbps as f32 / 1000.0);
                out.push(choice(
                    "dl-speed",
                    t!("Speed limit"),
                    "",
                    shown,
                    speed_idx == 0,
                    speed_idx + 1 >= SPEEDS_KBPS.len(),
                    step_act(|_, f, _, delta| {
                        with_downloads(f, |d, cx| {
                            let mut st = d.tv_settings();
                            st.limit_kbps = step_in(&SPEEDS_KBPS, st.limit_kbps, delta);
                            d.tv_apply(cx, st);
                        });
                    }),
                ));
            }
            out.push(toggle("dl-schedule", t!("Only auto-update games between"), "", s.schedule, dl(|d| d.schedule = !d.schedule)));
            if s.schedule {
                out.push(choice(
                    "dl-from",
                    t!("From"),
                    "",
                    hour(s.schedule_from),
                    s.schedule_from == 0,
                    s.schedule_from == 23,
                    step_act(|_, f, _, delta| {
                        with_downloads(f, |d, cx| {
                            let mut st = d.tv_settings();
                            st.schedule_from = step_index(24, st.schedule_from as usize, delta, delta > 1) as u8;
                            d.tv_apply(cx, st);
                        });
                    }),
                ));
                out.push(choice(
                    "dl-to",
                    t!("To"),
                    "",
                    hour(s.schedule_to),
                    s.schedule_to == 0,
                    s.schedule_to == 23,
                    step_act(|_, f, _, delta| {
                        with_downloads(f, |d, cx| {
                            let mut st = d.tv_settings();
                            st.schedule_to = step_index(24, st.schedule_to as usize, delta, delta > 1) as u8;
                            d.tv_apply(cx, st);
                        });
                    }),
                ));
            }
            let conn_idx = CONNECTIONS.iter().position(|x| *x == s.connections).unwrap_or(0);
            out.push(choice(
                "dl-connections",
                t!("Parallel connections"),
                "",
                s.connections.to_string(),
                conn_idx == 0,
                conn_idx + 1 >= CONNECTIONS.len(),
                step_act(|_, f, _, delta| {
                    with_downloads(f, |d, cx| {
                        let mut st = d.tv_settings();
                        st.connections = step_in(&CONNECTIONS, st.connections, delta);
                        d.tv_apply(cx, st);
                    });
                }),
            ));
            out.push(toggle(
                "dl-bits",
                t!("Show speeds in bits per second"),
                "",
                s.show_bits,
                dl(|d| d.show_bits = !d.show_bits),
            ));

            // Shader pre-caching & Vulkan pre-compilation.
            let locked = client::steam_settings_locked();
            if let Some(precache) = client::steam_setting(client::SteamSetting::ShaderPreCaching) {
                let note = locked
                    .map(|why| i18n::tr_dyn(why).to_string())
                    .unwrap_or_else(|| t!("Download shaders already compiled for your GPU and driver, so games stutter less.").into());
                out.push(toggle_enabled(
                    "dl-shader-precache",
                    t!("Enable shader pre-caching"),
                    &note,
                    precache,
                    locked.is_none(),
                    act(move |_, _, _| {
                        let _ = client::set_steam_setting(client::SteamSetting::ShaderPreCaching, !precache);
                    }),
                ));
            }
            #[cfg(not(windows))]
            {
                let precompile = client::shaders::precompile();
                out.push(toggle(
                    "dl-shader-precompile",
                    t!("Compile shaders before a game starts"),
                    t!("Compile missing Vulkan shaders on launch instead of running Steam's background shader worker."),
                    precompile,
                    act(move |_, _, _| {
                        client::shaders::set_precompile(!precompile);
                        client::apply_steam_prefs();
                    }),
                ));
                if !precompile
                    && let Some(bg) = client::steam_setting(client::SteamSetting::ShaderBackgroundProcessing)
                {
                    let note = locked
                        .map(|why| i18n::tr_dyn(why).to_string())
                        .unwrap_or_else(|| t!("Steam compiles downloaded shaders while it's open and idle.").into());
                    out.push(toggle_enabled(
                        "dl-shader-bg",
                        t!("Allow background processing of Vulkan shaders"),
                        &note,
                        bg,
                        locked.is_none(),
                        act(move |_, _, _| {
                            let _ = client::set_steam_setting(client::SteamSetting::ShaderBackgroundProcessing, !bg);
                        }),
                    ));
                }
            }
            #[cfg(windows)]
            if let Some(bg) = client::steam_setting(client::SteamSetting::ShaderBackgroundProcessing) {
                let note = locked
                    .map(|why| i18n::tr_dyn(why).to_string())
                    .unwrap_or_else(|| t!("Steam compiles downloaded shaders while it's open and idle.").into());
                out.push(toggle_enabled(
                    "dl-shader-bg",
                    t!("Allow background processing of Vulkan shaders"),
                    &note,
                    bg,
                    locked.is_none(),
                    act(move |_, _, _| {
                        let _ = client::set_steam_setting(client::SteamSetting::ShaderBackgroundProcessing, !bg);
                    }),
                ));
            }
            out
        }
        STORAGE => {
            if let Some(rx) = &bp.settings.storage_rx {
                match rx.try_recv() {
                    Ok((entries, cache_size)) => {
                        bp.settings.storage_cache = Some((Instant::now(), entries, cache_size));
                        bp.settings.storage_rx = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        bp.settings.storage_rx = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
            let need_refresh = bp
                .settings
                .storage_cache
                .as_ref()
                .is_none_or(|(at, _, _)| at.elapsed() > Duration::from_secs(15));
            if need_refresh && bp.settings.storage_rx.is_none() {
                bp.settings.storage_rx = Some(start_storage_probe(f, ctx.clone()));
            }
            let (entries, cache_size) = bp
                .settings
                .storage_cache
                .as_ref()
                .map(|(_, e, c)| (e.clone(), *c))
                .unwrap_or_default();

            let mut out = Vec::new();
            if entries.is_empty() {
                out.push(Row::Info {
                    label: t!("Library folders").into(),
                    value: t!("None found").into(),
                });
            } else {
                for entry in entries {
                    let free_text = entry
                        .free_bytes
                        .map(|b| t!("{free} free", free = ui::bytes(b)))
                        .unwrap_or_else(|| "—".into());
                    let detail = format!(
                        "{} · {} · {}",
                        t!("{n} games", n = entry.games),
                        ui::bytes(entry.used_bytes),
                        free_text
                    );
                    out.push(Row::Info {
                        label: entry.path.display().to_string(),
                        value: detail,
                    });
                }
            }
            out.push(Row::Info {
                label: t!("Picture cache").into(),
                value: ui::bytes(cache_size),
            });
            out.push(Row::Button {
                key: "storage-clear-cache".into(),
                icon: icons::TRASH,
                label: t!("Clear picture cache").into(),
                danger: false,
                checked: false,
                act: act(|bp, f, ctx| {
                    crate::image_cache::clear(ctx);
                    bp.settings.storage_cache = None;
                    f.actions.push(Action::Toast(t!("Picture cache cleared").into()));
                }),
            });
            out.push(Row::Button {
                key: "storage-refresh".into(),
                icon: icons::REFRESH,
                label: t!("Refresh storage info").into(),
                danger: false,
                checked: false,
                act: act(|bp, f, _| {
                    bp.settings.storage_cache = None;
                    f.actions.push(Action::ReloadLocal);
                }),
            });
            out.push(Row::Button {
                key: "storage-desktop".into(),
                icon: icons::FOLDER,
                label: t!("Add library folder or restore backup (desktop)").into(),
                danger: false,
                checked: false,
                act: act(|bp, f, ctx| {
                    bp.leave(ctx);
                    f.actions.push(Action::Navigate { page: "settings".into(), arg: None });
                }),
            });
            out
        }
        LIBRARY => {
            let v = f.app.prefs.bigpicture_library.min(2) as usize;
            vec![
                choice(
                    "lib-opens-on",
                    t!("Library opens on"),
                    "",
                    library_name(v as u8).to_string(),
                    v == 0,
                    v == 2,
                    step_act(|bp, f, _, delta| {
                        let cur = f.app.prefs.bigpicture_library.min(2) as usize;
                        let next = step_index(3, cur, delta, delta > 1) as u8;
                        f.app.prefs.bigpicture_library = next;
                        bp.installed_only = None;
                    }),
                ),
                toggle(
                    "lib-cloud",
                    t!("Steam Cloud"),
                    t!("Enable Steam Cloud for games that support it. Each game can also be turned off in its Properties."),
                    flags.cloud,
                    flag(|x| &mut x.cloud),
                ),
                toggle(
                    "lib-whats-new",
                    t!("Show What's new"),
                    t!("Show recent news from the games you play on the library home."),
                    flags.show_whats_new,
                    flag(|x| &mut x.show_whats_new),
                ),
            ]
        }
        NOTIFICATIONS => vec![
            toggle(
                "notif-chat",
                t!("New chat messages"),
                t!("Show a pop-up when a friend messages you."),
                flags.toast_messages,
                flag(|x| &mut x.toast_messages),
            ),
            toggle(
                "notif-steam",
                t!("Steam notifications"),
                t!("Show a pop-up for gifts, items, friend requests, sales and more."),
                flags.toast_notifications,
                flag(|x| &mut x.toast_notifications),
            ),
            Row::Button {
                key: "notif-open-inbox".into(),
                icon: icons::BELL,
                label: t!("Open notifications").into(),
                danger: false,
                checked: false,
                act: act(|bp, f, _| bp.open(Page::Notifications, f.now)),
            },
        ],
        CONTROLLER => {
            let p = &f.app.prefs;
            let (swap, repeat, stick) = (p.bigpicture_swap_ab, p.bigpicture_repeat.min(2), p.bigpicture_stick.min(2));
            let hints = p.bigpicture_hints;
            let cfg = client::controller_settings();
            let cal = cfg.calibration;
            let need_poll = bp
                .settings
                .controller_cache
                .as_ref()
                .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(3));
            if need_poll {
                bp.settings.controller_cache = Some((Instant::now(), client::poll_controllers()));
            }
            let devices = bp.settings.controller_cache.as_ref().unwrap().1.clone();
            let mut out = vec![
                Row::Info {
                    label: t!("Controllers connected").to_string(),
                    value: bp.pads.max(devices.controllers.len()).to_string(),
                },
                Row::Button {
                    key: "ctrl-refresh".into(),
                    icon: icons::REFRESH,
                    label: t!("Refresh controllers").into(),
                    danger: false,
                    checked: false,
                    act: act(|bp, _, _| {
                        client::refresh_controllers();
                        bp.settings.controller_cache = None;
                    }),
                },
                choice(
                    "ctrl-hints",
                    t!("Button hints"),
                    "",
                    hints.label().into(),
                    hints == ButtonHintMode::Auto,
                    hints == ButtonHintMode::Keyboard,
                    step_act(|_, f, _, delta| {
                        let modes = ButtonHintMode::ALL;
                        let current = modes.iter().position(|mode| *mode == f.app.prefs.bigpicture_hints).unwrap_or(0);
                        f.app.prefs.bigpicture_hints = modes[step_index(modes.len(), current, delta, delta > 1)];
                    }),
                ),
                toggle(
                    "ctrl-swap-ab",
                    t!("Swap A and B"),
                    t!("For controllers that have A and B the other way round."),
                    swap,
                    act(|_, f, _| f.app.prefs.bigpicture_swap_ab = !f.app.prefs.bigpicture_swap_ab),
                ),
                choice(
                    "ctrl-repeat",
                    t!("Held button repeat"),
                    t!("How fast the selection moves while you hold a direction."),
                    speed_name(repeat).to_string(),
                    repeat == 0,
                    repeat == 2,
                    step_act(|_, f, _, delta| {
                        let cur = f.app.prefs.bigpicture_repeat.min(2) as usize;
                        f.app.prefs.bigpicture_repeat = step_index(3, cur, delta, delta > 1) as u8;
                    }),
                ),
                choice(
                    "ctrl-stick",
                    t!("Stick sensitivity"),
                    t!("How little you need to push the stick to move the selection."),
                    level_name(stick).to_string(),
                    stick == 0,
                    stick == 2,
                    step_act(|_, f, _, delta| {
                        let cur = f.app.prefs.bigpicture_stick.min(2) as usize;
                        f.app.prefs.bigpicture_stick = step_index(3, cur, delta, delta > 1) as u8;
                    }),
                ),
                controller_toggle("ctrl-fam-xbox", t!("Enable Steam Input for Xbox controllers"), cfg.families.xbox, |c| &mut c.families.xbox),
                controller_toggle("ctrl-fam-ps", t!("Enable Steam Input for PlayStation controllers"), cfg.families.playstation, |c| &mut c.families.playstation),
                controller_toggle("ctrl-fam-switch", t!("Enable Steam Input for Switch Pro controllers"), cfg.families.switch_pro, |c| &mut c.families.switch_pro),
                controller_toggle("ctrl-fam-generic", t!("Enable Steam Input for generic controllers"), cfg.families.generic, |c| &mut c.families.generic),
                controller_toggle("ctrl-rumble", t!("Controller vibration (rumble)"), cal.rumble_enabled, |c| &mut c.calibration.rumble_enabled),
                controller_toggle("ctrl-gyro", t!("Enable gyroscope & motion sensors"), cal.gyro_enabled, |c| &mut c.calibration.gyro_enabled),
            ];
            if cal.gyro_enabled {
                let g_step = (cal.gyro_sensitivity * 4.0).round().clamp(1.0, 16.0) as usize;
                out.push(choice(
                    "ctrl-gyro-sens",
                    t!("Gyro sensitivity"),
                    "",
                    format!("{:.2}×", cal.gyro_sensitivity),
                    g_step <= 1,
                    g_step >= 16,
                    step_act(|_, _, _, delta| {
                        let mut c = client::controller_settings();
                        let cur = (c.calibration.gyro_sensitivity * 4.0).round().clamp(1.0, 16.0) as usize - 1;
                        let next = step_index(16, cur, delta, delta > 1) + 1;
                        c.calibration.gyro_sensitivity = next as f32 / 4.0;
                        client::set_controller_settings(c);
                    }),
                ));
            }
            out.extend([
                controller_toggle("ctrl-nintendo", t!("Use Nintendo button layout"), cal.nintendo_layout, |c| &mut c.calibration.nintendo_layout),
                deadzone("ctrl-ldz", t!("Left stick deadzone"), cal.left_deadzone, |c| &mut c.left_deadzone),
                deadzone("ctrl-rdz", t!("Right stick deadzone"), cal.right_deadzone, |c| &mut c.right_deadzone),
                deadzone("ctrl-trig", t!("Trigger threshold"), cal.trigger_threshold, |c| &mut c.trigger_threshold),
                Row::Button {
                    key: "ctrl-reset".into(),
                    icon: icons::RESTORE,
                    label: t!("Reset calibration").into(),
                    danger: false,
                    checked: false,
                    act: controller_change(|c| c.calibration = client::PadCalibration::default()),
                },
                Row::Button {
                    key: "ctrl-advanced".into(),
                    icon: icons::GEAR,
                    label: t!("Advanced controller settings (desktop)").into(),
                    danger: false,
                    checked: false,
                    act: act(|bp, f, ctx| {
                        bp.leave(ctx);
                        f.actions.push(Action::Navigate { page: "settings".into(), arg: Some("controller".into()) });
                    }),
                },
            ]);
            for pad in devices.controllers {
                let value = pad
                    .battery_percent
                    .map(|n| format!("{n}% · {}", pad.backend))
                    .unwrap_or_else(|| pad.backend.to_string());
                out.push(Row::Info { label: pad.name.clone(), value });
                if pad.has_rumble && cal.rumble_enabled {
                    let pad_id = pad.id.clone();
                    out.push(Row::Button {
                        key: format!("ctrl-rumble-{}", pad.id),
                        icon: icons::CONTROLLER,
                        label: t!("Test vibration: {controller}", controller = pad.name),
                        danger: false,
                        checked: false,
                        act: act(move |_, _, _| client::trigger_test_rumble(&pad_id)),
                    });
                }
            }
            out
        }
        OVERLAY => {
            let corner_idx = CORNERS.iter().position(|c| *c == flags.fps_counter).unwrap_or(0);
            let corner = match flags.fps_counter {
                FpsCorner::Off => t!("Off"),
                FpsCorner::TopLeft => t!("Top left"),
                FpsCorner::TopRight => t!("Top right"),
                FpsCorner::BottomLeft => t!("Bottom left"),
                FpsCorner::BottomRight => t!("Bottom right"),
            };
            let mut out = vec![
                toggle(
                    "ov-enabled",
                    t!("Show the overlay in games"),
                    t!("Open in supported games with Shift+Tab or Guide. Applies on the next game launch."),
                    flags.overlay_enabled,
                    flag(|x| &mut x.overlay_enabled),
                ),
                choice(
                    "ov-fps",
                    t!("FPS counter"),
                    t!("Shows how many frames per second the game draws, in a corner of the screen."),
                    corner.to_string(),
                    corner_idx == 0,
                    corner_idx + 1 >= CORNERS.len(),
                    step_act(|_, f, _, delta| {
                        let flags = f.app.shared_flags();
                        let mut l = flags.lock().unwrap();
                        l.fps_counter = step_in(&CORNERS, l.fps_counter, delta);
                    }),
                ),
            ];
            if flags.fps_counter != FpsCorner::Off {
                out.push(toggle(
                    "ov-fps-hc",
                    t!("High contrast color"),
                    t!("Draw the FPS counter in bright high-contrast green for visibility."),
                    flags.fps_high_contrast,
                    flag(|x| &mut x.fps_high_contrast),
                ));
            }
            out.push(Row::Button {
                key: "ov-preview".into(),
                icon: icons::EYE,
                label: t!("Preview overlay").into(),
                danger: false,
                checked: false,
                act: act(|_, f, _| f.actions.push(Action::ToggleOverlay)),
            });
            out
        }
        APPEARANCE => {
            let themes_list = bp
                .settings
                .themes
                .get_or_insert_with(|| {
                    let state = f.app.themes.lock().unwrap();
                    std::iter::once((None, t!("Default").into()))
                        .chain(state.manager.installed.iter().map(|theme| {
                            let meta = &theme.manifest.meta;
                            (Some(meta.id.clone()), meta.name.clone())
                        }))
                        .collect()
                })
                .clone();

            let mut out: Vec<Row> = themes_list
                .iter()
                .map(|(id, name)| {
                    let selected = id.clone();
                    Row::Button {
                        key: format!("theme-{}", id.as_deref().unwrap_or("default")),
                        icon: icons::PAINT,
                        label: name.clone(),
                        danger: false,
                        checked: *id == f.app.prefs.active_theme_id,
                        act: act(move |_, f, _| f.actions.push(Action::ActivateTheme(selected))),
                    }
                })
                .collect();

            // Active theme options (Bool, Select, Slider) right inside Big Picture!
            let active_options = {
                let st = f.app.themes.lock().unwrap();
                st.manager.active_manifest().and_then(|m| {
                    if m.options.is_empty() {
                        None
                    } else {
                        let id = m.meta.id.clone();
                        let values = st.manager.load_option_values(&id);
                        let opts: Vec<(String, themes::ThemeOption)> =
                            m.options.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        Some((id, values, opts))
                    }
                })
            };
            if let Some((theme_id, values, options)) = active_options {
                for (opt_key, opt) in options {
                    let current = values.get(&opt_key).unwrap_or(&opt.default);
                    let row_key = format!("theme-opt-{theme_id}-{opt_key}");
                    match opt.kind {
                        themes::OptionKind::Bool => {
                            let cur = current.as_bool().unwrap_or(false);
                            let tid = theme_id.clone();
                            let k = opt_key.clone();
                            out.push(toggle(
                                row_key,
                                &opt.label,
                                &opt.description,
                                cur,
                                act(move |_, f, _| {
                                    f.actions.push(Action::SetThemeOption {
                                        theme_id: tid,
                                        key: k,
                                        value: (!cur).to_string(),
                                    });
                                }),
                            ));
                        }
                        themes::OptionKind::Select if !opt.choices.is_empty() => {
                            let cur_val = current.as_str().unwrap_or("");
                            let idx = opt.choices.iter().position(|c| c.value == cur_val).unwrap_or(0);
                            let label_shown = opt.choices[idx].label.clone();
                            let choices: Vec<String> = opt.choices.iter().map(|c| c.value.clone()).collect();
                            let tid = theme_id.clone();
                            let k = opt_key.clone();
                            out.push(choice(
                                row_key,
                                &opt.label,
                                &opt.description,
                                label_shown,
                                idx == 0,
                                idx + 1 >= choices.len(),
                                step_act(move |_, f, _, delta| {
                                    let next = step_index(choices.len(), idx, delta, delta > 1);
                                    f.actions.push(Action::SetThemeOption {
                                        theme_id: tid,
                                        key: k,
                                        value: choices[next].clone(),
                                    });
                                }),
                            ));
                        }
                        themes::OptionKind::Slider => {
                            let min = opt.min.filter(|v| v.is_finite()).unwrap_or(0.0);
                            let max = opt.max.filter(|v| v.is_finite()).unwrap_or(100.0).max(min);
                            let step_v = opt.step.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(((max - min) / 10.0).max(1.0));
                            let cur = current.as_f32().filter(|v| v.is_finite()).unwrap_or(min).clamp(min, max);
                            let tid = theme_id.clone();
                            let k = opt_key.clone();
                            out.push(choice(
                                row_key,
                                &opt.label,
                                &opt.description,
                                format!("{cur:.1}"),
                                cur <= min + step_v * 0.01,
                                cur >= max - step_v * 0.01,
                                step_act(move |_, f, _, delta| {
                                    let next = if delta < 0 {
                                        (cur - step_v).max(min)
                                    } else if cur + step_v <= max + 0.001 {
                                        (cur + step_v).min(max)
                                    } else if delta > 1 {
                                        min
                                    } else {
                                        max
                                    };
                                    f.actions.push(Action::SetThemeOption {
                                        theme_id: tid,
                                        key: k,
                                        value: themes::OptionValue::Number(f64::from(next)).to_string_value(),
                                    });
                                }),
                            ));
                        }
                        _ => {}
                    }
                }
            }

            out.push(Row::Button {
                key: "theme-reload".into(),
                icon: icons::REFRESH,
                label: t!("Reload themes").into(),
                danger: false,
                checked: false,
                act: act(|bp, f, _| {
                    bp.settings.themes = None;
                    f.actions.push(Action::ReloadThemes);
                }),
            });
            out
        }
        ACCOUNT => {
            let me = f.app.state.me().map(|m| m.state).unwrap_or(PersonaState::Online);
            let status_idx = STATUSES.iter().position(|s| *s == me).unwrap_or(0);
            let armed = f.now < bp.settings.confirm_until;
            let balance = f
                .app
                .state
                .wallet
                .map(|(b, c)| model::format_money(b, c))
                .unwrap_or_else(|| "—".into());
            let friend_code = (f.app.state.steam_id & 0xFFFF_FFFF).to_string();
            let online = f.app.state.online();
            let mut out = vec![
                Row::Info { label: t!("Profile name").to_string(), value: f.app.state.persona_name.clone() },
                Row::Info { label: t!("Wallet balance").to_string(), value: balance },
                Row::Info { label: t!("Your friend code").to_string(), value: friend_code },
                choice(
                    "acct-status",
                    t!("Status"),
                    "",
                    i18n::tr_dyn(me.label()),
                    status_idx == 0,
                    status_idx + 1 >= STATUSES.len(),
                    step_act(move |_, f, _, delta| {
                        f.app.client.send(Command::SetPersonaState(step_in(&STATUSES, me, delta)));
                    }),
                ),
            ];
            if !f.app.demo {
                out.push(toggle(
                    "acct-offline",
                    t!("Offline mode"),
                    t!("Disconnect from Steam and play installed games offline."),
                    !online,
                    act(move |_, f, _| {
                        f.app.prefs.wants_offline = online;
                        f.app.client.send(if online { Command::GoOffline } else { Command::GoOnline });
                    }),
                ));
            }
            out.extend([
                toggle(
                    "acct-ask",
                    t!("Ask which account to use"),
                    t!("Show the account picker at startup instead of signing in automatically."),
                    flags.ask_account,
                    flag(|x| &mut x.ask_account),
                ),
                toggle(
                    "acct-tray",
                    t!("Close to tray"),
                    t!("Keep Aurora running in the system tray when you close the window."),
                    flags.close_to_tray,
                    flag(|x| &mut x.close_to_tray),
                ),
                Row::Button {
                    key: "acct-signout".into(),
                    icon: icons::SIGN_OUT,
                    label: if armed {
                        t!("Select again to sign out").to_string()
                    } else {
                        t!("Sign out of account").to_string()
                    },
                    danger: true,
                    checked: false,
                    act: act(move |bp, f, ctx| {
                        if armed {
                            bp.settings.confirm_until = 0.0;
                            bp.leave(ctx);
                            if !f.app.demo {
                                f.app.client.send(Command::LogOut);
                            }
                        } else {
                            bp.settings.confirm_until = f.now + 4.0;
                        }
                    }),
                },
            ]);
            out
        }
        #[cfg(target_os = "linux")]
        COMPATIBILITY => {
            let need_refresh = bp
                .settings
                .proton_cache
                .as_ref()
                .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(10));
            if need_refresh {
                bp.settings.proton_cache = Some((Instant::now(), client::proton_builds()));
            }
            let current = client::proton_override();
            let builds = bp.settings.proton_cache.as_ref().unwrap().1.clone();
            let mut out = vec![Row::Button {
                key: "compat-auto".into(),
                icon: icons::WINDOWS,
                label: t!("Automatic").into(),
                danger: false,
                checked: current.is_none(),
                act: act(|_, _, _| client::set_proton_override(None)),
            }];
            if builds.is_empty() {
                out.push(Row::Info {
                    label: t!("No installed Proton versions found.").into(),
                    value: String::new(),
                });
            } else if let Some(saved) = current
                .as_deref()
                .filter(|saved| !builds.iter().any(|(name, _)| saved.eq_ignore_ascii_case(name)))
            {
                out.push(Row::Info {
                    label: t!("Saved Proton is unavailable").into(),
                    value: saved.to_string(),
                });
            }
            out.extend(builds.into_iter().map(|(name, label)| {
                let checked = current.as_deref().is_some_and(|saved| saved.eq_ignore_ascii_case(&name));
                Row::Button {
                    key: format!("compat-tool-{name}"),
                    icon: icons::WINDOWS,
                    label,
                    danger: false,
                    checked,
                    act: act(move |_, _, _| client::set_proton_override(Some(name))),
                }
            }));
            out.push(Row::Button {
                key: "compat-refresh".into(),
                icon: icons::REFRESH,
                label: t!("Refresh Proton list").into(),
                danger: false,
                checked: false,
                act: act(|bp, _, _| bp.settings.proton_cache = None),
            });
            out
        }
        SYSTEM => {
            let mode = if bp.system.is_steam_deck() {
                t!("Steam Deck")
            } else if bp.is_deck_preview() {
                t!("Steam Deck preview")
            } else {
                t!("Big Picture")
            };
            let mut display = format!("{:.0}×{:.0} · {:.0}%", screen.x, screen.y, f.s * 100.0);
            if let Some(ppi) = bp.sys_status.display_ppi {
                display.push_str(&format!(" · {ppi:.0} PPI"));
            }
            let mut out = vec![
                Row::Info { label: t!("Aurora version").into(), value: env!("CARGO_PKG_VERSION").into() },
                Row::Info { label: t!("Mode").into(), value: mode.into() },
                Row::Info { label: t!("Session").into(), value: bp.sys_status.session_label.clone() },
                Row::Info {
                    label: t!("Display").into(),
                    value: display,
                },
                Row::Info {
                    label: t!("Touchscreen").into(),
                    value: if bp.system.has_touchscreen() { t!("Detected") } else { t!("Not detected") }.into(),
                },
                Row::Info { label: t!("Controllers connected").into(), value: bp.pads.to_string() },
            ];
            if let Some(pct) = bp.sys_status.battery_percent {
                let label = if bp.sys_status.battery_charging {
                    format!("{pct}% ({})", t!("Charging"))
                } else {
                    format!("{pct}%")
                };
                out.push(Row::Info { label: t!("Battery").into(), value: label });
            }
            out
        }
        _ => {
            let lang = i18n::language();
            let langs = i18n::Lang::ALL;
            let lang_idx = langs.iter().position(|l| *l == lang).unwrap_or(0);
            let size_idx = SIZES
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (*a - f.app.prefs.bigpicture_scale).abs().total_cmp(&(*b - f.app.prefs.bigpicture_scale).abs()))
                .map(|(i, _)| i)
                .unwrap_or(1);
            let scroll_idx = SCROLL_SPEEDS
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (*a - flags.scroll_speed).abs().total_cmp(&(*b - flags.scroll_speed).abs()))
                .map(|(i, _)| i)
                .unwrap_or(3);
            vec![
                choice(
                    "gen-scale",
                    t!("Interface size"),
                    t!("Scales Big Picture text and tiles for your TV or handheld screen."),
                    format!("{}%", (f.app.prefs.bigpicture_scale * 100.0).round() as u32),
                    size_idx == 0,
                    size_idx + 1 >= SIZES.len(),
                    step_act(|_, f, _, delta| {
                        f.app.prefs.bigpicture_scale = step_f32(&SIZES, f.app.prefs.bigpicture_scale, delta);
                    }),
                ),
                toggle(
                    "gen-fullscreen",
                    t!("Fullscreen"),
                    t!("Press F11 to switch at any time in Big Picture."),
                    !bp.is_windowed(),
                    act(|bp, _, ctx| bp.toggle_fullscreen(ctx)),
                ),
                toggle(
                    "gen-start-bp",
                    t!("Start Aurora in Big Picture"),
                    "",
                    f.app.prefs.bigpicture_start || (bp.system.is_steam_deck() && !f.app.prefs.bigpicture_start_explicit),
                    act(|bp, f, _| {
                        let prefs = &mut f.app.prefs;
                        let current = prefs.bigpicture_start || (bp.system.is_steam_deck() && !prefs.bigpicture_start_explicit);
                        prefs.bigpicture_start = !current;
                        prefs.bigpicture_start_explicit = true;
                    }),
                ),
                choice(
                    "gen-lang",
                    t!("Language"),
                    "",
                    lang.native_name().to_string(),
                    lang_idx == 0,
                    lang_idx + 1 >= langs.len(),
                    step_act(move |_, f, _, delta| {
                        let next = langs[step_index(langs.len(), lang_idx, delta, delta > 1)];
                        f.actions.push(Action::SetLanguage(next.code().into()));
                    }),
                ),
                choice(
                    "gen-scroll",
                    t!("Scroll speed"),
                    t!("How far the mouse wheel and touchpad scroll."),
                    format!("{:.2}×", flags.scroll_speed),
                    scroll_idx == 0,
                    scroll_idx + 1 >= SCROLL_SPEEDS.len(),
                    step_act(|_, f, _, delta| {
                        let flags = f.app.shared_flags();
                        let mut l = flags.lock().unwrap();
                        l.scroll_speed = step_f32(&SCROLL_SPEEDS, l.scroll_speed, delta);
                    }),
                ),
                toggle(
                    "gen-auto-offline",
                    t!("Start offline when Steam is unreachable"),
                    t!("Automatically fall back to cached offline mode if no network is available."),
                    f.app.prefs.auto_offline,
                    act(|_, f, _| f.app.prefs.auto_offline = !f.app.prefs.auto_offline),
                ),
                Row::Button {
                    key: "gen-desktop".into(),
                    icon: icons::GEAR,
                    label: t!("All settings (leaves Big Picture)").to_string(),
                    danger: false,
                    checked: false,
                    act: act(|bp, f, ctx| {
                        bp.leave(ctx);
                        f.actions.push(Action::Navigate { page: "settings".into(), arg: None });
                    }),
                },
            ]
        }
    }
}

fn nav_id(i: usize) -> Id {
    Id::unique(("bp-set-nav", i))
}

/// The settings page: scrollable sections on the left, scrollable rows on the right.
pub fn page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let top = f.view.top() + 8.0 * s - 88.0 * s * bp.bar_hide;
    let p = ui.painter().clone();
    p.text(pos2(left, top), Align2::LEFT_TOP, t!("Settings"), w::font_bold(38.0 * s), f.t.text);
    let body_top = top + 68.0 * s;

    let list = sections();
    bp.settings.section = bp.settings.section.min(list.len().saturating_sub(1));
    let cur = bp.focus.current;
    let was = bp.settings.last_focus;
    let is_nav = |id: Option<Id>| (0..list.len()).any(|i| id == Some(nav_id(i)));
    let is_tab = |id: Option<Id>| super::TABS.iter().any(|tab| id == Some(super::tab_id(*tab)));
    let from_row = was.is_some() && !is_nav(was) && !is_tab(was);
    let dir = bp.focus.last_dir;

    // Intercept Left/Right on Choice rows so D-pad adjusts values directly instead of leaving rows.
    bp.settings.pending_step = None;
    let mut handled_choice_dir = false;
    if let (Some(was_id), Some(d @ (Dir::Left | Dir::Right))) = (was, dir)
        && from_row
        && bp.settings.choice_ids.contains(&was_id)
    {
        if d == Dir::Right {
            bp.focus.current = Some(was_id);
            bp.settings.pending_step = Some((was_id, 1));
            handled_choice_dir = true;
        } else if d == Dir::Left && bp.settings.choice_can_step_left.contains(&was_id) {
            bp.focus.current = Some(was_id);
            bp.settings.pending_step = Some((was_id, -1));
            handled_choice_dir = true;
        }
    }

    if !handled_choice_dir {
        if bp.focus.moved && is_nav(cur) && from_row {
            bp.focus.current = if dir == Some(Dir::Left) { Some(nav_id(bp.settings.section)) } else { was };
        }
        // The columns aren't the same height: nothing may be right beside you.
        if !bp.focus.moved && dir == Some(Dir::Left) && from_row && cur == was {
            bp.focus.current = Some(nav_id(bp.settings.section));
        }
    }
    let into_rows = !handled_choice_dir && dir == Some(Dir::Right) && is_nav(was) && !is_tab(cur) && (bp.focus.moved || cur == was);
    bp.settings.last_focus = bp.focus.current;

    // Responsive rail width so 1280×800 handheld screens keep plenty of room for values.
    let total_w = f.view.width() - 2.0 * f.pad;
    let nav_w = (total_w * 0.27).clamp(280.0 * s, 400.0 * s);
    let nav_h = 64.0 * s;
    let nav_gap = 6.0 * s;
    let nav_area = Rect::from_min_max(
        pos2(left - 8.0 * s, body_top - 6.0 * s),
        pos2(left + nav_w + 8.0 * s, f.view.bottom() - 6.0 * s),
    );
    let nav_pos = bp.scroller("nav").pos;
    let mut nav_child = ui.new_child(egui::UiBuilder::new().max_rect(nav_area));
    let nav_clip = nav_area.intersect(ui.clip_rect());
    nav_child.set_clip_rect(nav_clip);
    let np = nav_child.painter().clone();
    let mut enter_rows = false;
    let mut nav_reveal = None;
    for (i, (icon, label)) in list.iter().enumerate() {
        let row_y = i as f32 * (nav_h + nav_gap);
        let r = Rect::from_min_size(pos2(left, body_top + row_y - nav_pos), vec2(nav_w, nav_h));
        let hit = bp.focus.item(&nav_child, nav_id(i), r, nav_clip, Place::Bar);
        if hit.focused {
            if bp.settings.section != i {
                bp.settings.section = i;
                bp.settings.confirm_until = 0.0;
                let sc = bp.scroller("rows");
                sc.pos = 0.0;
                sc.target = 0.0;
            }
            if bp.focus.moved {
                nav_reveal = Some((row_y, row_y + nav_h));
            }
        }
        let on = bp.settings.section == i;
        let fill = if on {
            f.t.accent.gamma_multiply(0.32 + 0.2 * hit.glow)
        } else {
            w::mix(Color32::TRANSPARENT, Color32::from_white_alpha(22), hit.glow)
        };
        np.rect_filled(r, paint::radius(12.0 * s), fill);
        let c = if on { f.t.text } else { f.t.text_dim };
        np.text(pos2(r.left() + 34.0 * s, r.center().y), Align2::CENTER_CENTER, *icon, w::font(25.0 * s), c);
        paint::line(
            &np,
            pos2(r.left() + 64.0 * s, r.center().y),
            Align2::LEFT_CENTER,
            label,
            w::font_semibold(21.0 * s),
            c,
            r.width() - 78.0 * s,
        );
        if hit.focused {
            bp.focus.ring_around_clipped(r, 12.0 * s, nav_clip);
        }
        enter_rows |= hit.activated;
    }
    let nav_height = list.len() as f32 * (nav_h + nav_gap);
    let nav_view = nav_area.height() - 8.0 * s;
    let nav_wheel = if ui.rect_contains_pointer(nav_area) {
        ui.input(|i| i.smooth_scroll_delta.y)
    } else {
        0.0
    };
    let nsc = bp.scroller("nav");
    nsc.target -= nav_wheel;
    if let Some((a, b)) = nav_reveal {
        nsc.reveal(a, b, nav_view, 16.0 * s);
    }
    nsc.clamp(nav_height, nav_view);
    nsc.step(ui);
    bp.preferred = Some(nav_id(bp.settings.section));

    // The rows on the right, scrolling on their own with semantic IDs & clipped focus ring.
    let x = left + nav_w + 30.0 * s;
    let width = (f.view.right() - f.pad - x).min(1120.0 * s);
    let area = Rect::from_min_max(
        pos2(x - 12.0 * s, body_top - 8.0 * s),
        pos2(x + width + 12.0 * s, f.view.bottom() - 6.0 * s),
    );
    let list_rows = rows(bp, f, ui.ctx());
    let section = bp.settings.section;
    let pos = bp.scroller("rows").pos;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    let row_clip = area.intersect(ui.clip_rect());
    child.set_clip_rect(row_clip);
    let q = child.painter().clone();
    let mut y = body_top - pos;
    let mut picked: Option<Act> = None;
    let mut picked_step: Option<(StepAct, i32)> = None;
    let mut first_row = None;
    let mut reveal = None;
    let row_top = y;

    bp.settings.choice_ids.clear();
    bp.settings.choice_can_step_left.clear();

    for row in list_rows {
        let has_note = matches!(&row, Row::Toggle { note, .. } | Row::Choice { note, .. } if !note.is_empty());
        let h = if has_note { 98.0 } else { 76.0 } * s;
        let r = Rect::from_min_size(pos2(x, y), vec2(width, h));
        y += h + 10.0 * s;
        if let Row::Info { label, value } = &row {
            q.rect_filled(r, paint::radius(14.0 * s), f.t.surface.gamma_multiply(0.55));
            paint::line(&q, pos2(r.left() + 24.0 * s, r.center().y), Align2::LEFT_CENTER, label, w::font_semibold(21.0 * s), f.t.text_dim, r.width() * 0.56);
            paint::line(&q, pos2(r.right() - 24.0 * s, r.center().y), Align2::RIGHT_CENTER, value, w::font_medium(21.0 * s), f.t.text, r.width() * 0.40);
            continue;
        }
        let key_str = match &row {
            Row::Toggle { key, .. } | Row::Choice { key, .. } | Row::Button { key, .. } => key.clone(),
            Row::Info { .. } => unreachable!(),
        };
        let id = Id::unique(("bp-set-row", section, key_str));
        first_row.get_or_insert(id);
        let hit = bp.focus.item(&child, id, r, row_clip, Place::Bar);
        if hit.focused && bp.focus.moved {
            reveal = Some((r.top() - row_top, r.bottom() - row_top));
        }
        let shown = paint::grow(r, 1.0 + 0.01 * hit.glow);
        q.rect_filled(shown, paint::radius(14.0 * s), w::mix(f.t.surface.gamma_multiply(0.85), f.t.surface_hover, hit.glow));
        if hit.focused {
            bp.focus.ring_around_clipped(shown, 14.0 * s, row_clip);
        }
        let text_x = shown.left() + 24.0 * s;
        let right = shown.right() - 24.0 * s;
        let title_at = |note: &str| if note.is_empty() { shown.center().y } else { shown.top() + 32.0 * s };
        let draw_note = |note: &str, room: f32| {
            if !note.is_empty() {
                paint::line(&q, pos2(text_x, shown.top() + 66.0 * s), Align2::LEFT_CENTER, note, w::font(17.5 * s), f.t.text_dim, room);
            }
        };
        match row {
            Row::Toggle { label, note, on, enabled, act, .. } => {
                let k = child.ctx().animate_bool_with_time(id.with("switch"), on, 0.14);
                let track = Rect::from_center_size(pos2(right - 34.0 * s, shown.center().y), vec2(68.0 * s, 36.0 * s));
                let fill = if enabled { w::mix(f.t.raised, f.t.accent, k) } else { f.t.raised.gamma_multiply(0.5) };
                q.rect_filled(track, paint::radius(18.0 * s), fill);
                q.circle_filled(
                    pos2(track.left() + 18.0 * s + k * 32.0 * s, track.center().y),
                    13.0 * s,
                    if enabled { Color32::WHITE } else { f.t.text_faint },
                );
                let room = track.left() - text_x - 18.0 * s;
                paint::line(
                    &q,
                    pos2(text_x, title_at(&note)),
                    Align2::LEFT_CENTER,
                    &label,
                    w::font_semibold(22.0 * s),
                    if enabled { f.t.text } else { f.t.text_faint },
                    room,
                );
                draw_note(&note, room);
                if hit.activated && enabled {
                    picked = Some(act);
                }
            }
            Row::Choice { label, note, value, at_start, at_end, step, .. } => {
                bp.settings.choice_ids.insert(id);
                if !at_start {
                    bp.settings.choice_can_step_left.insert(id);
                }
                let right_arrow = pos2(right - 8.0 * s, shown.center().y);
                let right_col = if at_end { f.t.text_faint } else { f.t.accent };
                q.text(right_arrow, Align2::CENTER_CENTER, icons::CARET_RIGHT, w::font(20.0 * s), right_col);
                let vr = paint::line(
                    &q,
                    pos2(right - 26.0 * s, shown.center().y),
                    Align2::RIGHT_CENTER,
                    &value,
                    w::font_medium(22.0 * s),
                    f.t.accent,
                    width * 0.38,
                );
                let left_arrow = pos2(vr.left() - 16.0 * s, shown.center().y);
                let left_col = if at_start { f.t.text_faint } else { f.t.accent };
                q.text(left_arrow, Align2::CENTER_CENTER, icons::CARET_LEFT, w::font(20.0 * s), left_col);

                // Pointer clicks on the left arrow step backward directly.
                let left_hit = Rect::from_center_size(left_arrow, vec2(34.0 * s, shown.height()));
                let left_clicked = child
                    .interact(left_hit.intersect(row_clip), id.with("dec"), egui::Sense::click())
                    .clicked();

                let room = left_arrow.x - text_x - 24.0 * s;
                paint::line(&q, pos2(text_x, title_at(&note)), Align2::LEFT_CENTER, &label, w::font_semibold(22.0 * s), f.t.text, room);
                draw_note(&note, room);

                if left_clicked {
                    picked_step = Some((step, -1));
                } else if let Some((target_id, delta)) = bp.settings.pending_step
                    && target_id == id
                {
                    picked_step = Some((step, delta));
                } else if hit.activated {
                    // A / Enter wraps forward so single-button cycling still works.
                    picked_step = Some((step, 2));
                }
            }
            Row::Button { icon, label, danger, checked, act, .. } => {
                let c = if danger { f.t.danger } else { f.t.text };
                q.text(pos2(text_x + 14.0 * s, shown.center().y), Align2::CENTER_CENTER, icon, w::font(25.0 * s), c);
                paint::line(&q, pos2(text_x + 48.0 * s, shown.center().y), Align2::LEFT_CENTER, &label, w::font_semibold(22.0 * s), c, width - 140.0 * s);
                if checked {
                    q.text(pos2(right - 14.0 * s, shown.center().y), Align2::CENTER_CENTER, icons::CHECK, w::font(25.0 * s), f.t.accent);
                }
                if hit.activated {
                    picked = Some(act);
                }
            }
            Row::Info { .. } => {}
        }
    }
    let height = y - row_top;
    let view = area.height() - 8.0 * s;
    let wheel = if ui.rect_contains_pointer(area) {
        ui.input(|i| i.smooth_scroll_delta.y)
    } else {
        0.0
    };
    let sc = bp.scroller("rows");
    sc.target -= wheel;
    if f.stick_scroll != 0.0 {
        sc.target += f.stick_scroll;
    }
    if f.lt {
        sc.target -= view * 0.75;
    }
    if f.rt {
        sc.target += view * 0.75;
    }
    if let Some((a, b)) = reveal {
        sc.reveal(a, b, view, 20.0 * s);
    }
    sc.clamp(height, view);
    sc.step(ui);
    if enter_rows || into_rows {
        if let Some(id) = first_row {
            bp.focus.current = Some(id);
            bp.settings.last_focus = Some(id);
            let sc = bp.scroller("rows");
            sc.target = 0.0;
        }
    }
    let ctx = ui.ctx().clone();
    if let Some((s_act, delta)) = picked_step {
        s_act(bp, f, &ctx, delta);
    } else if let Some(a) = picked {
        a(bp, f, &ctx);
    }
    f.view.height()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_stepping_clamps_left_and_wraps_on_confirm() {
        assert_eq!(step_index(4, 0, -1, false), 0);
        assert_eq!(step_index(4, 2, -1, false), 1);
        assert_eq!(step_index(4, 2, 1, false), 3);
        assert_eq!(step_index(4, 3, 1, false), 3);
        assert_eq!(step_index(4, 3, 2, true), 0);
        assert_eq!(step_index(0, 0, 1, true), 0);
    }

    #[test]
    fn sections_constants_match_list_order() {
        let s = sections();
        assert_eq!(s[GENERAL].1, t!("General"));
        assert_eq!(s[DOWNLOADS].1, t!("Downloads"));
        assert_eq!(s[STORAGE].1, t!("Storage"));
        assert_eq!(s[LIBRARY].1, t!("Library"));
        assert_eq!(s[NOTIFICATIONS].1, t!("Notifications"));
        assert_eq!(s[CONTROLLER].1, t!("Controller"));
        assert_eq!(s[OVERLAY].1, t!("In-game overlay"));
        assert_eq!(s[APPEARANCE].1, t!("Appearance"));
        assert_eq!(s[ACCOUNT].1, t!("Account"));
        assert_eq!(s[SYSTEM].1, t!("System"));
    }
}
