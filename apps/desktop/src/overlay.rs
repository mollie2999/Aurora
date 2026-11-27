//! Shift+Tab overlay, laid out like Steam's: the game is dimmed, a bar at the
//! top shows the game, session time, the window buttons and the clock, and
//! Game Overview, Achievements, Friends, Chat and Notes open as windows you
//! can move, resize and pin. A pinned window stays over the game after the
//! overlay closes, at the opacity you pick, and lets clicks through to it.
//!
//! For games we start with our steamclient DLL (D3D11, D3D12, D3D9, OpenGL,
//! and Vulkan) it is drawn inside the game, like Steam's (see `ingame.rs` and
//! `crates/steamclient/src/overlay`), so it works in exclusive fullscreen and
//! the game keeps focus. Other games (games whose API hook isn't active, or
//! anti-cheat games real Steam runs) automatically fall back to a separate
//! always-on-top window, which shows over windowed and borderless-fullscreen
//! games.

use ui::widgets::EditMenu;
use i18n::t;
use crate::app::SteamApp;
use client::Command;
use egui::{Align, Align2, Color32, CornerRadius, Key, Layout, Margin, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use extensions::Action;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use model::{AppId, Persona, PersonaState, SteamId, art};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use ui::{Theme, icons, widgets as w};

/// The overlay's windows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Win {
    Overview,
    Achievements,
    Friends,
    Chat,
    Notes,
    /// Community pages: hub, guides, discussions, Workshop items.
    Community,
    /// The game's store page.
    Store,
    /// Local screenshots taken during the game.
    Screenshots,
    /// Per-game Steam Input / controller layout switcher.
    Controller,
}

impl Win {
    const ALL: [Win; 9] = [
        Win::Overview,
        Win::Achievements,
        Win::Friends,
        Win::Chat,
        Win::Notes,
        Win::Community,
        Win::Store,
        Win::Screenshots,
        Win::Controller,
    ];
}

#[derive(Clone, Copy)]
struct WinState {
    open: bool,
    pinned: bool,
    /// How see-through the window is while pinned over the game.
    opacity: f32,
}

struct ForegroundProbe {
    in_game: bool,
    hypr_window: Option<(String, i64, String)>,
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct HyprOverlayState {
    placement_generation: u64,
    overlay_addr: Option<String>,
}

#[cfg(target_os = "linux")]
impl HyprOverlayState {
    fn cancel_placement(&mut self) {
        self.placement_generation = self.placement_generation.wrapping_add(1);
        self.overlay_addr = None;
    }
}

pub struct Overlay {
    pub open: bool,
    manager: Option<GlobalHotKeyManager>,
    hotkey: HotKey,
    registered: bool,
    /// The overlay is turned on for the running game.
    enabled: bool,
    /// When the foreground window was last checked.
    checked: Option<Instant>,
    foreground_check: Option<tokio::sync::oneshot::Receiver<ForegroundProbe>>,
    /// The game's window was in front at the last check.
    game_front: bool,
    pressed: Arc<AtomicBool>,
    pub game: Option<(AppId, Instant)>,
    chat_with: Option<crate::builtin::group_chat::ChatTarget>,
    group_ui: crate::builtin::group_chat::GroupUiState,
    input: String,
    notes: String,
    search: String,
    show_offline: bool,
    wins: [WinState; 9],
    /// Focus was given to the overlay since it opened.
    focused: bool,
    /// The window that had focus when the overlay opened (the game), so it
    /// gets focus back when the overlay closes.
    game_window: isize,
    /// Clicks currently pass through the overlay window.
    passthrough: Option<bool>,
    /// True after `show()` submits the fallback overlay viewport until a pass
    /// omits it, so an occluded root viewport knows when a UI pass is needed.
    fallback_vp_active: bool,
    /// Last known Hyprland window address of the active game.
    game_hypr_addr: Option<String>,
    /// Last known Hyprland `(workspace_id, workspace_name)` of the active game.
    game_hypr_ws: Option<(i64, String)>,
    /// True once `"Aurora Overlay"` has been moved/focused onto `game_hypr_ws`.
    hypr_placed: bool,
    /// Shared Hyprland open/restore state between the UI thread and placement task.
    #[cfg(target_os = "linux")]
    hypr_state: Arc<std::sync::Mutex<HyprOverlayState>>,
    /// Data asked for this session.
    asked: bool,
    /// What the Community window shows (with its own back history).
    community: crate::builtin::hub::HubView,
    /// The Achievements window (compare with a friend inside it).
    achievements: crate::builtin::hub::HubView,
    /// Game whose store page the Store window shows, and whether the store
    /// page was pointed at it yet.
    store_app: Option<(AppId, bool)>,
    /// `--overlay-open=guides|discussions|hub|store|achievements`: a window to
    /// open for previews.
    preview: Option<String>,
    /// The running game draws the overlay itself.
    pub ingame: Option<crate::ingame::Session>,
    /// Actions from in-game overlay frames, for the main window to apply.
    pending: Vec<Action>,
    /// Live FPS reported by the in-game hook.
    fps: Option<u32>,
    /// Cached list of local screenshots for the running game.
    screenshots: Vec<client::ingame::LocalScreenshot>,
    screenshots_dirty: bool,
    /// In-flight background screenshot encode/save tasks (off the UI thread).
    screenshot_saves: Vec<(AppId, tokio::sync::oneshot::Receiver<Result<std::path::PathBuf, String>>)>,
    /// Pending screenshot request from the overlay UI button.
    take_shot_requested: bool,
    /// App ID whose notes are currently loaded, and debounce timestamp for saving.
    notes_app: Option<AppId>,
    notes_dirty: Option<Instant>,
    /// Whether Aurora's live controller layout bridge is active for the running game.
    controller_route_enabled: Option<bool>,
    /// Last layout source confirmed applied by the game bridge.
    controller_applied_source: Option<String>,
}

impl Overlay {
    pub fn new(ctx: &egui::Context) -> Self {
        let pressed = Arc::new(AtomicBool::new(false));
        let manager = GlobalHotKeyManager::new().map_err(|e| tracing::warn!("global hotkeys unavailable: {e}")).ok();
        let (flag, repaint) = (pressed.clone(), ctx.clone());
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                flag.store(true, Ordering::Relaxed);
                repaint.request_repaint();
                repaint.request_repaint_of(overlay_viewport());
            }
        }));
        // A game's in-game overlay sent something: run the UI.
        let main = ctx.clone();
        client::ingame::set_waker(move || crate::ingame::wake(&main, std::time::Duration::ZERO));
        let closed = WinState { open: false, pinned: false, opacity: 0.85 };
        let mut wins = [closed; 9];
        // Like Steam: the game overview and friends open by default.
        wins[Win::Overview as usize].open = true;
        wins[Win::Friends as usize].open = true;
        Self {
            open: false,
            manager,
            hotkey: HotKey::new(Some(Modifiers::SHIFT), Code::Tab),
            registered: false,
            enabled: false,
            checked: None,
            foreground_check: None,
            game_front: false,
            pressed,
            game: None,
            chat_with: None,
            group_ui: Default::default(),
            input: String::new(),
            notes: String::new(),
            search: String::new(),
            show_offline: false,
            wins,
            focused: false,
            game_window: 0,
            passthrough: None,
            fallback_vp_active: false,
            game_hypr_addr: None,
            game_hypr_ws: None,
            hypr_placed: false,
            #[cfg(target_os = "linux")]
            hypr_state: Arc::new(std::sync::Mutex::new(HyprOverlayState::default())),
            asked: false,
            community: Default::default(),
            achievements: Default::default(),
            store_app: None,
            preview: std::env::args().find_map(|a| a.strip_prefix("--overlay-open=").map(String::from)),
            ingame: None,
            pending: Vec::new(),
            fps: None,
            screenshots: Vec::new(),
            screenshots_dirty: true,
            screenshot_saves: Vec::new(),
            take_shot_requested: false,
            notes_app: None,
            notes_dirty: None,
            controller_route_enabled: None,
            controller_applied_source: None,
        }
    }

    pub fn set_controller_route(&mut self, app_id: AppId, enabled: bool) {
        if self.game.is_some_and(|(id, _)| id == app_id) {
            self.controller_route_enabled = Some(enabled);
        }
    }

    pub fn set_controller_applied(&mut self, app_id: AppId, source: String) {
        if self.game.is_some_and(|(id, _)| id == app_id) {
            self.controller_applied_source = Some(source);
        }
    }

    /// Shift+Tab is only grabbed while the game's own window is in front
    /// (see `sync_hotkey`), so it keeps working normally everywhere else
    /// and the overlay never pops up over other apps.
    pub fn game_started(&mut self, app_id: AppId, enabled: bool) {
        self.flush_notes();
        self.game = Some((app_id, Instant::now()));
        self.asked = false;
        self.enabled = enabled;
        self.checked = None;
        self.foreground_check = None;
        self.game_front = false;
        self.game_hypr_addr = None;
        self.game_hypr_ws = None;
        self.hypr_placed = false;
        #[cfg(target_os = "linux")]
        {
            let mut st = self.hypr_state.lock().unwrap_or_else(|e| e.into_inner());
            st.cancel_placement();
        }
        self.fps = None;
        self.screenshots.clear();
        self.screenshots_dirty = true;
        self.take_shot_requested = false;
        self.notes_app = Some(app_id);
        self.notes_dirty = None;
        self.notes = std::fs::read_to_string(notes_path(app_id)).unwrap_or_default();
        self.controller_route_enabled = None;
        self.controller_applied_source = None;
    }

    /// Register Shift+Tab only while the game is the focused window (or the
    /// overlay is open, so Shift+Tab can close it).
    fn sync_hotkey(&mut self, game_dir: Option<&std::path::Path>, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        let fallback = self.ingame.is_none() && self.game.is_some() && (self.enabled || self.any_pinned());
        if fallback {
            if let Some(rx) = self.foreground_check.as_mut() {
                match rx.try_recv() {
                    Ok(probe) => {
                        self.game_front = probe.in_game;
                        if let Some((addr, ws_id, ws_name)) = probe.hypr_window {
                            self.game_hypr_addr = Some(addr);
                            self.game_hypr_ws = Some((ws_id, ws_name));
                        }
                        self.foreground_check = None;
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => self.foreground_check = None,
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                }
            }
            if self.foreground_check.is_none() && self.checked.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(400)) {
                self.checked = Some(Instant::now());
                if let Some(dir) = game_dir {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    self.foreground_check = Some(rx);
                    let dir = dir.to_path_buf();
                    let ctx = ctx.clone();
                    rt.spawn_blocking(move || {
                        if tx.send(foreground_probe(&dir)).is_ok() {
                            ctx.request_repaint();
                        }
                    });
                } else {
                    self.game_front = false;
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        } else {
            self.checked = None;
            self.foreground_check = None;
            self.game_front = false;
        }

        // The in-process overlay receives its own hotkey.
        let want = fallback && self.enabled && (self.open || self.game_front);
        let Some(m) = &self.manager else { return };
        if want && !self.registered {
            self.registered = m.register(self.hotkey).map_err(|e| tracing::warn!("Shift+Tab: {e}")).is_ok();
        } else if !want && self.registered {
            let _ = m.unregister(self.hotkey);
            self.registered = false;
        }
    }

    pub fn game_stopped(&mut self) {
        self.flush_notes();
        self.game = None;
        self.foreground_check = None;
        self.game_hypr_addr = None;
        self.game_hypr_ws = None;
        self.hypr_placed = false;
        #[cfg(target_os = "linux")]
        {
            let mut st = self.hypr_state.lock().unwrap_or_else(|e| e.into_inner());
            st.cancel_placement();
        }
        self.ingame = None;
        self.open = false;
        self.fps = None;
        self.screenshots.clear();
        self.screenshots_dirty = true;
        self.take_shot_requested = false;
        self.notes_app = None;
        self.controller_route_enabled = None;
        self.controller_applied_source = None;
        for w in &mut self.wins {
            w.pinned = false;
        }
        if self.registered {
            if let Some(m) = &self.manager {
                let _ = m.unregister(self.hotkey);
            }
            self.registered = false;
        }
    }

    fn flush_notes(&mut self) {
        if let Some(app_id) = self.notes_app {
            if self.notes_dirty.take().is_some() {
                let path = notes_path(app_id);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(path, &self.notes);
            }
        }
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open = true;
            self.focused = false;
            self.hypr_placed = false;
            self.game_window = foreground_window();
            #[cfg(target_os = "linux")]
            if crate::hyprland::is_available() {
                if let Some(aw) = crate::hyprland::active_window() {
                    if aw.pid != std::process::id() {
                        self.game_hypr_addr = Some(aw.address);
                        self.game_hypr_ws = Some((aw.workspace_id, aw.workspace_name));
                    }
                }
            }
        }
    }

    fn close(&mut self) {
        self.open = false;
        self.hypr_placed = false;
        // Give the keyboard back to the game and keep any pinned windows above it.
        #[cfg(target_os = "linux")]
        {
            let any_pinned = self.any_pinned();
            let mut st = self.hypr_state.lock().unwrap_or_else(|e| e.into_inner());
            let overlay_addr = if any_pinned { st.overlay_addr.take() } else { None };
            st.cancel_placement();
            drop(st);
            if crate::hyprland::is_available() {
                if let Some(addr) = self.game_hypr_addr.as_deref() {
                    match crate::hyprland::focus_game_keeping_overlay_top(addr, overlay_addr.as_deref()) {
                        Ok(()) => return,
                        Err(err) => tracing::warn!("Hyprland focus restore failed: {err}"),
                    }
                }
            }
        }
        give_focus(self.game_window);
    }

    fn win(&self, w: Win) -> WinState {
        self.wins[w as usize]
    }

    fn win_mut(&mut self, w: Win) -> &mut WinState {
        &mut self.wins[w as usize]
    }

    fn any_pinned(&self) -> bool {
        Win::ALL.iter().any(|w| {
            let s = self.win(*w);
            s.open && s.pinned && (*w != Win::Chat || self.chat_with.is_some())
        })
    }
}

#[cfg(windows)]
fn foreground_window() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}
#[cfg(target_os = "linux")]
fn foreground_window() -> isize {
    x11::active_window().map(|w| w as isize).unwrap_or(0)
}
#[cfg(not(any(windows, target_os = "linux")))]
fn foreground_window() -> isize {
    0
}

/// Check whether a Windows executable path is strictly inside `dir` (respecting
/// path-component boundaries so `C:\Games\App` never matches `C:\Games\App2\...`).
#[cfg(any(windows, test))]
fn win_exe_in_dir(exe: &str, dir: &str) -> bool {
    let exe = exe.to_lowercase().replace('/', "\\");
    let dir = dir.to_lowercase().replace('/', "\\");
    let trimmed = dir.trim_end_matches('\\');
    if trimmed.is_empty() {
        return false;
    }
    let prefix = format!("{trimmed}\\");
    exe.starts_with(&prefix)
}

/// The focused window belongs to a program inside `dir` (the game's folder).
#[cfg(windows)]
fn foreground_is_in(dir: &std::path::Path) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == std::process::id() {
            return false;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        if !ok {
            return false;
        }
        let exe = String::from_utf16_lossy(&buf[..len as usize]);
        win_exe_in_dir(&exe, &dir.display().to_string())
    }
}
#[cfg(target_os = "linux")]
fn foreground_probe(dir: &std::path::Path) -> ForegroundProbe {
    let our_pid = std::process::id();
    if crate::hyprland::is_available() {
        if let Some(aw) = crate::hyprland::active_window() {
            if aw.pid != our_pid && process_in(aw.pid, dir) {
                return ForegroundProbe {
                    in_game: true,
                    hypr_window: Some((aw.address, aw.workspace_id, aw.workspace_name)),
                };
            }
            return ForegroundProbe {
                in_game: false,
                hypr_window: None,
            };
        }
    }
    ForegroundProbe {
        in_game: foreground_is_in(dir),
        hypr_window: None,
    }
}
#[cfg(not(target_os = "linux"))]
fn foreground_probe(dir: &std::path::Path) -> ForegroundProbe {
    ForegroundProbe {
        in_game: foreground_is_in(dir),
        hypr_window: None,
    }
}

/// Linux: the focused X window (games run on X11 or XWayland) belongs to a
/// process whose program, folder or command line is inside the game's
/// folder. Proton games are Wine processes, so their command line holds the
/// game's Windows path (`Z:\home\…\Game.exe`).
#[cfg(target_os = "linux")]
fn foreground_is_in(dir: &std::path::Path) -> bool {
    let Some(pid) = x11::active_window().and_then(x11::window_pid) else { return false };
    if pid == std::process::id() {
        return false;
    }
    process_in(pid, dir)
}
#[cfg(not(any(windows, target_os = "linux")))]
fn foreground_is_in(_: &std::path::Path) -> bool {
    false
}

/// A process runs from `dir`: its executable, working directory or any
/// command-line argument (Wine paths too) is inside it.
#[cfg(target_os = "linux")]
fn process_in(pid: u32, dir: &std::path::Path) -> bool {
    let dir = dir.to_string_lossy().trim_end_matches('/').to_lowercase();
    if dir.is_empty() {
        return false;
    }
    let inside = |p: &str| {
        let p = wine_to_unix(p).to_lowercase();
        p == dir || p.starts_with(&format!("{dir}/"))
    };
    let proc_ = std::path::PathBuf::from(format!("/proc/{pid}"));
    for link in ["exe", "cwd"] {
        if std::fs::read_link(proc_.join(link)).is_ok_and(|p| inside(&p.to_string_lossy())) {
            return true;
        }
    }
    std::fs::read(proc_.join("cmdline")).is_ok_and(|c| c.split(|b| *b == 0).any(|a| !a.is_empty() && inside(&String::from_utf8_lossy(a))))
}

/// `Z:\home\me\Game.exe` (Wine's drive for `/`) → `/home/me/Game.exe`;
/// other paths unchanged.
#[cfg(target_os = "linux")]
fn wine_to_unix(p: &str) -> String {
    match p.get(..2) {
        Some(d) if d.eq_ignore_ascii_case("z:") => p[2..].replace('\\', "/"),
        _ => p.to_string(),
    }
}

#[cfg(windows)]
fn give_focus(window: isize) {
    if window != 0 {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(window as _) };
    }
}
#[cfg(target_os = "linux")]
fn give_focus(window: isize) {
    if window != 0 {
        x11::activate(window as u32);
    }
}
#[cfg(not(any(windows, target_os = "linux")))]
fn give_focus(_: isize) {}

/// The few X11 calls the overlay needs, on one shared connection.
#[cfg(target_os = "linux")]
mod x11 {
    use std::sync::{Mutex, OnceLock};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask};
    use x11rb::rust_connection::RustConnection;

    struct X {
        conn: RustConnection,
        root: u32,
        active: u32,
        pid: u32,
    }

    fn x() -> Option<&'static Mutex<X>> {
        static X11: OnceLock<Option<Mutex<X>>> = OnceLock::new();
        X11.get_or_init(|| {
            let (conn, screen) = x11rb::connect(None).map_err(|e| tracing::info!("overlay: no X11 display ({e})")).ok()?;
            let root = conn.setup().roots.get(screen)?.root;
            let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
            let (active, pid) = (atom("_NET_ACTIVE_WINDOW")?, atom("_NET_WM_PID")?);
            Some(Mutex::new(X { conn, root, active, pid }))
        })
        .as_ref()
    }

    fn property_u32(x: &X, window: u32, prop: u32, kind: AtomEnum) -> Option<u32> {
        let r = x.conn.get_property(false, window, prop, kind, 0, 1).ok()?.reply().ok()?;
        r.value32()?.next()
    }

    /// The window the window manager says is in front.
    pub fn active_window() -> Option<u32> {
        let x = x()?.lock().ok()?;
        property_u32(&x, x.root, x.active, AtomEnum::WINDOW).filter(|w| *w != 0)
    }

    /// The process that owns a window (`_NET_WM_PID`).
    pub fn window_pid(window: u32) -> Option<u32> {
        let x = x()?.lock().ok()?;
        property_u32(&x, window, x.pid, AtomEnum::CARDINAL).filter(|p| *p != 0)
    }

    /// Ask the window manager to bring a window to the front and focus it.
    pub fn activate(window: u32) {
        let Some(Ok(x)) = x().map(|m| m.lock()) else { return };
        // Source 2: a pager-like request, which window managers honour.
        let ev = ClientMessageEvent::new(32, window, x.active, [2u32, x11rb::CURRENT_TIME, 0, 0, 0]);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let _ = x.conn.send_event(false, x.root, mask, ev);
        let _ = x.conn.flush();
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    #[test]
    fn wine_paths_become_unix_paths() {
        assert_eq!(super::wine_to_unix(r"Z:\home\me\Games\Celeste\Celeste.exe"), "/home/me/Games/Celeste/Celeste.exe");
        assert_eq!(super::wine_to_unix("/usr/bin/foo"), "/usr/bin/foo");
    }

    #[test]
    fn finds_this_process_in_its_own_folder() {
        let exe = std::env::current_exe().unwrap();
        assert!(super::process_in(std::process::id(), exe.parent().unwrap()));
        assert!(!super::process_in(std::process::id(), std::path::Path::new("/nonexistent-aurora-dir")));
    }
}

/// Pick up games that draw the overlay themselves and read what they sent.
fn poll_ingame(app: &mut SteamApp, ctx: &egui::Context) {
    // Always drain completed background screenshot saves even if the game has
    // already disconnected, associating gallery invalidation with the originating game.
    let now = ctx.input(|i| i.time);
    let mut completed_saves = Vec::new();
    app.overlay.screenshot_saves.retain_mut(|(origin_app, rx)| match rx.try_recv() {
        Ok(res) => {
            completed_saves.push((*origin_app, res));
            false
        }
        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => true,
        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => false,
    });
    for (origin_app, saved) in completed_saves {
        match saved {
            Ok(_) => {
                if app.overlay.game.is_some_and(|(id, _)| id == origin_app) {
                    app.overlay.screenshots_dirty = true;
                }
                app.toast(now, t!("Screenshot saved"), "");
            }
            Err(e) => {
                tracing::warn!("saving streamed screenshot failed: {e}");
                app.toast(now, t!("Couldn't save screenshot"), e);
            }
        }
    }
    for conn in client::ingame::take_new() {
        if app.overlay.game.is_some_and(|(id, _)| id != conn.app_id) {
            tracing::info!("in-game overlay from app {} while {:?} runs; using it anyway", conn.app_id, app.overlay.game);
        }
        let session = crate::ingame::Session::new(conn, ctx, &app._rt, &app.prefs.theme);
        // Shift+Tab to toggle overlay, F12 to take a screenshot.
        session.send_config(client::ingame::wire::MOD_SHIFT, 0x09, 0, 0x7B);
        app.overlay.ingame = Some(session);
        // Let go of Shift+Tab now: the game handles it.
        app.overlay.checked = None;
        app.overlay.open = false;
    }
    let enabled = app.overlay.enabled && app.prefs.overlay_enabled;
    let Some(s) = app.overlay.ingame.as_mut() else { return };
    let res = s.poll();
    if !res.alive {
        tracing::info!("in-game overlay connection closed");
        app.overlay.ingame = None;
        app.overlay.open = false;
        app.overlay.fps = None;
        app.overlay.checked = None;
        return;
    }
    if let Some(fps) = res.fps {
        app.overlay.fps = Some(fps);
    }
    match res.opened {
        // Overlay turned off for this game: don't open.
        Some(true) if !enabled => s.set_open(false),
        Some(open) => {
            app.overlay.open = open;
            // No window to focus: the game keeps focus.
            app.overlay.focused = true;
            app.overlay.game_window = 0;
        }
        None => {}
    }
    if let Some(ok) = res.screenshot_saved {
        if ok {
            app.overlay.screenshots_dirty = true;
            app.toast(now, t!("Screenshot saved"), "");
        } else {
            app.toast(now, t!("Couldn't save screenshot"), "");
        }
    }
    if let Some((w, h, rgb)) = res.screenshot_rgb {
        let app_id = app.overlay.game.map(|(id, _)| id).or_else(|| app.overlay.ingame.as_ref().map(|s| s.conn.app_id)).unwrap_or(0);
        let account_id = (app.state.steam_id & 0xFFFF_FFFF) as u32;
        let steam_root = app.local.steam_root.clone();
        let ctx_clone = ctx.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.overlay.screenshot_saves.push((app_id, rx));
        app._rt.spawn_blocking(move || {
            let res = client::ingame::save_screenshot_rgb(&steam_root, account_id, app_id, w, h, rgb);
            let _ = tx.send(res);
            ctx_clone.request_repaint();
        });
    }
}

/// What the overview and achievements need, once per game session.
fn load_once(app: &mut SteamApp) {
    if !app.overlay.open || app.overlay.asked {
        return;
    }
    app.overlay.asked = true;
    if let Some((id, _)) = app.overlay.game {
        if !app.state.achievement_list.contains_key(&id) {
            app.client.send(Command::LoadAchievements { app_id: id, language: i18n::language().steam_name().into() });
        }
        let me = app.state.steam_id;
        if me != 0 && !app.state.player_games.contains_key(&me) {
            app.client.send(Command::LoadPlayerGames { steam_id: me, language: i18n::language().steam_name().into() });
        }
    }
}

/// One in-game overlay frame, when one is due: the game's input in, the UI
/// run, the triangles out to the game.
fn ingame_frame(app: &mut SteamApp, ctx: &egui::Context, actions: &mut Vec<Action>) {
    load_once(app);
    let Some(mut s) = app.overlay.ingame.take() else { return };
    if std::mem::take(&mut app.overlay.take_shot_requested) {
        s.take_screenshot();
    }
    let t = app.prefs.theme.clone();
    s.theme(&t);
    // Opened or closed here (tray, a game's dialog request): tell the game.
    s.set_open(app.overlay.open);
    let active = app.overlay.open;
    let fps_visible = app.prefs.fps_counter != crate::builtin::settings::FpsCorner::Off && app.overlay.fps.is_some();
    let has_toasts = !app.toasts.is_empty();
    let draw = active || app.overlay.any_pinned() || fps_visible || has_toasts;
    let mut close = false;
    s.run(ctx, draw, |ui| close |= content(app, ui, &t, actions, active));
    if close {
        app.overlay.open = false;
    }
    s.set_open(app.overlay.open);
    app.overlay.ingame = Some(s);
}

/// eframe runs the main window's frames at most every 100 ms while it's
/// minimized or in the tray, which is how it usually is during a game: the
/// in-game overlay would run at 10 fps. So a 1×1 helper window that never
/// takes focus or shows in the taskbar gets the frames instead, and runs the
/// in-game overlay at full speed.
/// The app for a deferred viewport's callback.
struct AppPtr(*mut SteamApp);
// SAFETY: only used on the UI thread: eframe calls deferred viewport
// callbacks there, in their own frames, never inside `App::ui` (callers
// check that viewports aren't embedded), and the app outlives its viewports.
unsafe impl Send for AppPtr {}
unsafe impl Sync for AppPtr {}
impl AppPtr {
    fn get(&self) -> *mut SteamApp {
        self.0
    }
}

/// The window overlay's own viewport.
fn overlay_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("aurora-overlay")
}

fn keep_ticking(app: &mut SteamApp, ctx: &egui::Context) {
    // Without real windows the callback would run inside this frame, while
    // `app` is borrowed: the main window's frames have to do then.
    if ctx.embed_viewports() {
        return;
    }
    // Linux: the main window keeps getting frames while minimized or behind
    // the game (GNOME on Wayland still sends them), so it drives the overlay.
    // A 1x1 fixed-size helper window is also a Wayland protocol error
    // ("Invalid min/max size") that closes the app.
    if cfg!(target_os = "linux") {
        return;
    }
    let app = AppPtr(app);
    let builder = egui::ViewportBuilder::default()
        .with_title(format!("{} overlay timer", i18n::BRAND))
        .with_decorations(false)
        .with_transparent(true)
        .with_taskbar(false)
        .with_active(false)
        .with_resizable(false)
        .with_mouse_passthrough(true)
        .with_position(egui::pos2(0.0, 0.0))
        .with_inner_size(egui::vec2(1.0, 1.0));
    ctx.show_viewport_deferred(crate::ingame::tick(), builder, move |ui, _| {
        // SAFETY: eframe calls a deferred viewport's callback on the UI
        // thread in its own frame, never inside `App::ui` (viewports aren't
        // embedded, checked above), and the app outlives its viewports.
        let app = unsafe { &mut *app.get() };
        let ctx = ui.ctx().clone();
        poll_ingame(app, &ctx);
        // Window commands from here would go to this helper window, so
        // actions wait for the main window's next frame.
        let mut actions = std::mem::take(&mut app.overlay.pending);
        ingame_frame(app, &ctx, &mut actions);
        app.overlay.pending = actions;
        if !app.overlay.pending.is_empty() {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    });
}

/// The overlay drawn inside a game (from `App::logic`, which also runs when
/// the app's window gets no frames): read the game's input, run the UI, send
/// the frame. Window commands it asks for wait for the next real frame.
pub fn logic(app: &mut SteamApp, ctx: &egui::Context) {
    poll_ingame(app, ctx);
    if app.overlay.ingame.is_none() {
        if let Some((id, _)) = app.overlay.game {
            let dir = app.local.games.iter().find(|g| g.app_id == id).map(|g| g.install_dir.as_path());
            app.overlay.sync_hotkey(dir, ctx, &app._rt);
            let toggled = if app.overlay.pressed.swap(false, Ordering::Relaxed) {
                app.overlay.toggle();
                true
            } else {
                false
            };
            let want_fallback_ui = app.overlay.open || (app.overlay.any_pinned() && app.overlay.game_front);
            if want_fallback_ui != app.overlay.fallback_vp_active
                || !app.overlay.pending.is_empty()
                || toggled
                || app.overlay.preview.is_some()
            {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::RequestPaintWhileHidden,
                );
                ctx.request_repaint();
            }
        } else if app.overlay.fallback_vp_active {
            // Game exited while the fallback overlay viewport was open: ask ROOT for
            // one UI pass so `show()` omits the child viewport and closes it.
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::RequestPaintWhileHidden,
            );
            ctx.request_repaint();
        }
        return;
    }
    let mut actions = std::mem::take(&mut app.overlay.pending);
    ingame_frame(app, ctx, &mut actions);
    app.overlay.pending = actions;
    if !app.overlay.pending.is_empty() {
        ctx.request_repaint();
    }
}

pub fn show(app: &mut SteamApp, ctx: &egui::Context, actions: &mut Vec<Action>) {
    actions.append(&mut app.overlay.pending);
    // Shift+Tab belongs to the game only while the game is in front.
    if let Some((id, _)) = app.overlay.game {
        let dir = app.local.games.iter().find(|g| g.app_id == id).map(|g| g.install_dir.as_path());
        app.overlay.sync_hotkey(dir, ctx, &app._rt);
    }
    if app.overlay.pressed.swap(false, Ordering::Relaxed) {
        app.overlay.toggle();
    }
    let active = app.overlay.open;
    let ingame = app.overlay.ingame.is_some();
    // Pinned windows only float over the game, not over other apps.
    if !ingame && !active && !(app.overlay.any_pinned() && app.overlay.game_front) {
        app.overlay.passthrough = None;
        app.overlay.fallback_vp_active = false;
        return;
    }
    load_once(app);
    if let (Some(what), Some((id, _))) = (app.overlay.preview.take(), app.overlay.game) {
        use crate::builtin::hub::{HubTab, View};
        match what.as_str() {
            "store" => open_store(app, id),
            "achievements" => app.overlay.win_mut(Win::Achievements).open = true,
            "guides" => open_community(app, View::Guides(id), true),
            "discussions" => open_community(app, View::Forum { app: id, forum: String::new() }, true),
            _ => open_community(app, View::Hub { app: id, tab: HubTab::All }, true),
        }
    }
    let t = app.prefs.theme.clone();
    // Drawn inside the game: `logic` runs its UI (before this frame, and
    // while the window gets no frames).
    if app.overlay.ingame.is_some() {
        keep_ticking(app, ctx);
        return;
    }
    let viewport = overlay_viewport();
    // Not true fullscreen on Windows: Windows turns a window that exactly covers
    // the screen into exclusive fullscreen, which drops transparency (the game
    // went black). One pixel short keeps it a normal see-through window.
    // On Linux (especially GNOME Wayland), compositors ignore `with_position`
    // and clamp undecorated floating windows, so request fullscreen directly so
    // the fallback overlay fills the screen without needing manual maximize.
    let monitor = ctx.input(|i| i.viewport().monitor_size).unwrap_or(egui::vec2(1920.0, 1080.0));
    let mut builder = egui::ViewportBuilder::default()
        .with_title(format!("{} Overlay", i18n::BRAND))
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_position(egui::pos2(0.0, 0.0))
        .with_inner_size(monitor - egui::vec2(0.0, 1.0))
        .with_mouse_passthrough(!active)
        .with_taskbar(false);
    #[cfg(target_os = "linux")]
    if crate::hyprland::is_available() {
        // Fixed-size Wayland toplevels map as floating in Hyprland. This also
        // avoids its tiled-window fullscreen policy during the initial map.
        builder = builder.with_resizable(false);
    } else {
        builder = builder.with_fullscreen(true);
    }
    // Linux: the overlay window draws in its own frames. The main window is
    // behind the game or minimized then, and the compositor (GNOME on
    // Wayland) hardly gives it frames, so an overlay drawn from them
    // opened late and crawled.
    app.overlay.fallback_vp_active = true;
    #[cfg(target_os = "linux")]
    if !ctx.embed_viewports() {
        let ptr = AppPtr(app);
        ctx.show_viewport_deferred(viewport, builder, move |ui, _class| {
            let app = unsafe { &mut *ptr.get() };
            // Shift+Tab while the overlay is in front closes it right here.
            if app.overlay.pressed.swap(false, Ordering::Relaxed) {
                app.overlay.toggle();
            }
            let active = app.overlay.open;
            let opened_now = active && !app.overlay.focused;
            if app.overlay.passthrough != Some(!active) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::MousePassthrough(!active));
                app.overlay.passthrough = Some(!active);
            }
            if opened_now {
                if !crate::hyprland::is_available() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                app.overlay.focused = true;
            }
            if active && !app.overlay.hypr_placed && crate::hyprland::is_available() {
                app.overlay.hypr_placed = true;
                if let Some((ws_id, ws_name)) = app.overlay.game_hypr_ws.clone() {
                    let hypr_state = app.overlay.hypr_state.clone();
                    let generation = hypr_state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .placement_generation;
                    let game_addr = app.overlay.game_hypr_addr.clone();
                    let overlay_title = format!("{} Overlay", i18n::BRAND);
                    let our_pid = std::process::id();
                    app._rt.spawn_blocking(move || {
                        for attempt in 0..16 {
                            {
                                let st = hypr_state.lock().unwrap_or_else(|e| e.into_inner());
                                if st.placement_generation != generation {
                                    return;
                                }
                            }
                            let all_clients = crate::hyprland::clients();
                            if let Some(win) = crate::hyprland::select_overlay_window(
                                &all_clients,
                                our_pid,
                                &overlay_title,
                            )
                            .filter(|win| win.mapped && !win.hidden)
                            {
                                if win.fullscreen != 0 {
                                    tracing::warn!("Hyprland overlay must be non-fullscreen to float above a game");
                                    return;
                                }
                                let monitors = crate::hyprland::monitors();
                                let target_game = game_addr.as_deref().and_then(|addr| {
                                    all_clients
                                        .iter()
                                        .find(|w| w.address == addr && w.workspace_id == ws_id)
                                });
                                let monitor_geom = crate::hyprland::select_monitor_geom(
                                    &monitors,
                                    target_game,
                                    ws_id,
                                );
                                let placement = crate::hyprland::OverlayPlacement {
                                    overlay_addr: &win.address,
                                    ws_id,
                                    ws_name: &ws_name,
                                    monitor_geom,
                                };
                                let mut st = hypr_state.lock().unwrap_or_else(|e| e.into_inner());
                                if st.placement_generation != generation {
                                    return;
                                }
                                match crate::hyprland::place_overlay_on_workspace(&placement) {
                                    Ok(()) => {
                                        st.overlay_addr = Some(win.address.clone());
                                    }
                                    Err(err) => {
                                        tracing::warn!("Hyprland overlay placement failed: {err}");
                                    }
                                }
                                break;
                            }
                            if attempt < 15 {
                                std::thread::sleep(std::time::Duration::from_millis(25));
                            }
                        }
                    });
                }
            }
            let t = app.prefs.theme.clone();
            let mut actions = std::mem::take(&mut app.overlay.pending);
            if content(app, ui, &t, &mut actions, active) {
                app.overlay.close();
            }
            app.overlay.pending = actions;
            // Window commands (and closing) are the main window's job.
            if !app.overlay.pending.is_empty() || !app.overlay.open {
                ui.ctx().send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::RequestPaintWhileHidden,
                );
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            if app.overlay.open {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
        });
        return;
    }
    let opened_now = active && !app.overlay.focused;
    ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
        // Pinned windows over the game let clicks through; the open overlay takes them.
        if app.overlay.passthrough != Some(!active) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::MousePassthrough(!active));
            app.overlay.passthrough = Some(!active);
        }
        // Take keyboard and mouse focus so input goes to the overlay, not the game.
        if opened_now {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            app.overlay.focused = true;
        }
        if content(app, ui, &t, actions, active) {
            app.overlay.close();
        }
    });
}

/// The overlay itself: the dimmed game, the top bar and the windows. Shared
/// by the window overlay and the in-game one. Returns true to close.
fn content(app: &mut SteamApp, ui: &mut Ui, t: &Theme, actions: &mut Vec<Action>, active: bool) -> bool {
    let close = active && ui.input(|i| i.key_pressed(Key::Escape) || (i.modifiers.shift && i.key_pressed(Key::Tab)) || i.viewport().close_requested());
    // Dim the game, like Steam does (nothing but pinned windows when closed).
    let fill = if active { Color32::from_black_alpha(150) } else { Color32::TRANSPARENT };
    egui::CentralPanel::default().frame(egui::Frame::new().fill(fill)).show(ui, |ui| {
        if active {
            top_strip(app, ui, t);
        }
    });
    let ctx = ui.ctx().clone();
    fps_hud(app, &ctx, t);
    toasts_hud(app, &ctx, t);
    overview_window(app, &ctx, t);
    achievements_window(app, &ctx, t);
    friends_window(app, &ctx, t, actions);
    chat_window(app, &ctx, t, actions);
    notes_window(app, &ctx, t);
    screenshots_window(app, &ctx, t);
    controller_window(app, &ctx, t, actions);
    community_window(app, &ctx, t, actions);
    store_window(app, &ctx, t, actions);
    crate::builtin::group_chat::modals(&ctx, t, &app.state, &app.client, &mut app.overlay.group_ui);
    crate::builtin::chat_render::show_lightbox(&ctx, t);
    if active && app.overlay.ingame.is_some() {
        software_cursor(&ctx);
    }
    if app.overlay.notes_dirty.is_some_and(|at| at.elapsed() >= std::time::Duration::from_secs(1)) {
        app.overlay.flush_notes();
    }
    // The clock and session time.
    ctx.request_repaint_after(std::time::Duration::from_secs(1));
    close
}

/// Local time as HH:MM.
fn clock() -> String {
    #[cfg(windows)]
    {
        let mut st = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
        unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st) };
        format!("{:02}:{:02}", st.wHour, st.wMinute)
    }
    // Local time, not UTC.
    #[cfg(target_os = "linux")]
    {
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
        unsafe { libc::localtime_r(&now, &mut tm) };
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) % 86_400;
        format!("{:02}:{:02}", s / 3600, (s / 60) % 60)
    }
}

const STRIP: f32 = 76.0;

fn top_strip(app: &mut SteamApp, ui: &mut Ui, t: &Theme) {
    let full = ui.max_rect();
    let strip = Rect::from_min_size(full.min, vec2(full.width(), STRIP));
    ui.painter().rect_filled(strip, CornerRadius::ZERO, t.chrome);
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, t.border));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(strip.shrink2(vec2(24.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
    // The game: its art opens the overview.
    match app.overlay.game {
        Some((id, since)) => {
            let (r, resp) = c.allocate_exact_size(vec2(118.0, 55.0), Sense::click());
            let name = app.state.app_name(id).unwrap_or("").to_string();
            w::image_or_title(&c, t, r, &art::header(id), &name, CornerRadius::same(4));
            if resp.on_hover_text(t!("Game overview")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                let s = app.overlay.win_mut(Win::Overview);
                s.open = !s.open;
            }
            c.add_space(10.0);
            c.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(w::text_bold(&name, 18.0, t.text));
                ui.label(w::text(format!("{}  {}", icons::CLOCK, session_text(since)), 12.5, t.text_dim));
            });
        }
        None => {
            w::logo(&mut c, t, 26.0);
            c.label(w::text_bold(t!("Overlay preview"), 18.0, t.text));
        }
    }
    // Window buttons in the middle, like Steam's toolbar.
    let game = app.overlay.game.map(|g| g.0);
    let mid = Rect::from_center_size(strip.center(), vec2(740.0, STRIP));
    let mut m = ui.new_child(egui::UiBuilder::new().max_rect(mid).layout(Layout::left_to_right(Align::Center)));
    m.spacing_mut().item_spacing.x = 6.0;
    let unread = app.state.total_unread();
    let mut toggle = |m: &mut Ui, win: Win, icon: &str, label: &str, badge: usize| {
        let on = app.overlay.win(win).open;
        if strip_button(m, t, icon, label, on, badge).clicked() {
            let s = app.overlay.win_mut(win);
            s.open = !s.open;
        }
    };
    if game.is_some() {
        toggle(&mut m, Win::Overview, icons::CONTROLLER, t!("Overview"), 0);
        toggle(&mut m, Win::Achievements, icons::TROPHY, t!("Achievements"), 0);
    }
    toggle(&mut m, Win::Friends, icons::USERS, t!("Friends"), unread);
    toggle(&mut m, Win::Notes, icons::NOTE, t!("Notes"), 0);
    if game.is_some() {
        toggle(&mut m, Win::Screenshots, icons::CAMERA, t!("Screenshots"), 0);
        toggle(&mut m, Win::Controller, icons::SLIDERS, t!("Controller"), 0);
    }
    if let Some(id) = game {
        use crate::builtin::hub::View;
        let now = if app.overlay.win(Win::Community).open { app.overlay.community.current().cloned() } else { None };
        let links = [
            (icons::BOOKS, t!("Guides"), View::Guides(id), matches!(now, Some(View::Guides(_)))),
            (icons::CHATS, t!("Discussions"), View::Forum { app: id, forum: String::new() }, matches!(now, Some(View::Forum { .. } | View::Thread(_)))),
        ];
        for (icon, label, view, on) in links {
            if strip_button(&mut m, t, icon, label, on, 0).clicked() {
                if on {
                    app.overlay.win_mut(Win::Community).open = false;
                } else {
                    open_community(app, view, true);
                }
            }
        }
    }
    // Clock and close on the right.
    let right = Rect::from_min_max(pos2(strip.right() - 420.0, strip.top()), pos2(strip.right() - 24.0, strip.bottom()));
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)));
    if w::icon_button(&mut r, t, icons::X, t!("Close (Shift+Tab)"), false, 0).clicked() {
        app.overlay.close();
    }
    r.add_space(10.0);
    r.vertical(|ui| {
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(14.0);
            ui.label(w::text_bold(clock(), 22.0, t.text));
            let hint = if app.overlay.game.is_some() { t!("Shift+Tab or Esc to return to the game") } else { t!("Esc to close") };
            ui.label(w::text(hint, 11.5, t.text_faint));
        });
    });
}

fn session_text(since: Instant) -> String {
    let mins = since.elapsed().as_secs() / 60;
    if mins < 60 { t!("{n} min this session", n = mins) } else { t!("{h} h {m} min this session", h = mins / 60, m = mins % 60) }
}

fn hours(minutes: u64) -> String {
    if minutes < 60 { t!("{n} min", n = minutes) } else { t!("{h} h", h = format!("{:.1}", minutes as f32 / 60.0)) }
}

fn strip_button(ui: &mut Ui, t: &Theme, icon: &str, label: &str, on: bool, badge: usize) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), w::font_medium(11.5), t.text);
    let width = galley.size().x.max(44.0) + 20.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 58.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let p = ui.painter();
    if on || hover > 0.0 {
        p.rect_filled(rect, CornerRadius::same(6), if on { t.accent.gamma_multiply(0.18) } else { t.surface_hover.gamma_multiply(hover) });
    }
    let color = if on { t.accent } else { w::mix(t.text_dim, t.text, hover) };
    p.text(pos2(rect.center().x, rect.top() + 21.0), Align2::CENTER_CENTER, icon, w::font(20.0), color);
    p.galley(pos2(rect.center().x - galley.size().x / 2.0, rect.top() + 36.0), galley, color);
    if badge > 0 {
        let c = pos2(rect.center().x + 13.0, rect.top() + 11.0);
        p.circle_filled(c, 8.0, t.danger);
        p.text(c, Align2::CENTER_CENTER, badge.min(99).to_string(), w::font_bold(10.0), Color32::WHITE);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Solid window look shared by the overlay's windows.
fn window_frame(t: &Theme, opacity: f32) -> egui::Frame {
    egui::Frame::new()
        .fill(t.chrome.gamma_multiply(opacity))
        .stroke(Stroke::new(1.0, t.border.gamma_multiply(opacity)))
        .corner_radius(t.corner())
        .shadow(if opacity < 1.0 { egui::Shadow::NONE } else { egui::Shadow { offset: [0, 12], blur: 40, spread: 0, color: Color32::from_black_alpha(160) } })
        .inner_margin(Margin::same(14))
}

/// One overlay window, Steam style: a title row with pin and close. While the
/// overlay is closed only pinned windows show, see-through and click-through.
fn ov_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme, win: Win, title: &str, pos: Pos2, size: Vec2, body: impl FnOnce(&mut SteamApp, &mut Ui)) {
    let s = app.overlay.win(win);
    let active = app.overlay.open;
    if !s.open || (!active && !s.pinned) {
        return;
    }
    let opacity = if active { 1.0 } else { s.opacity };
    let (mut close, mut pin, mut level) = (false, false, s.opacity);
    egui::Window::new(title)
        .id(egui::Id::unique(("ov-window", win as u8)))
        .title_bar(false)
        .frame(window_frame(t, opacity))
        .default_pos(pos)
        .default_size(size)
        .min_size(vec2(260.0, 160.0))
        .resizable(true)
        .movable(active)
        .interactable(active)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(opacity.max(0.35));
            if active {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(w::text_bold(title, 15.0, t.text)).truncate().selectable(false));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        close = w::icon_button(ui, t, icons::X, t!("Close"), false, 0).clicked();
                        let tip = if s.pinned { t!("Unpin: hide this window when the overlay closes") } else { t!("Pin: keep this window over the game") };
                        pin = w::icon_button(ui, t, if s.pinned { icons::PUSH_PIN_SLASH } else { icons::PUSH_PIN }, tip, s.pinned, 0).clicked();
                        if s.pinned {
                            ui.add(egui::Slider::new(&mut level, 0.2..=1.0).show_value(false)).on_hover_text(t!("Opacity over the game"));
                        }
                    });
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
            }
            body(app, ui);
        });
    let s = app.overlay.win_mut(win);
    s.opacity = level;
    if pin {
        s.pinned = !s.pinned;
    }
    if close {
        s.open = false;
        s.pinned = false;
        if win == Win::Chat {
            app.overlay.chat_with = None;
        }
    }
}

fn overview_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme) {
    let Some((id, since)) = app.overlay.game else { return };
    let name = app.state.app_name(id).unwrap_or("").to_string();
    let pos = pos2(40.0, STRIP + 40.0);
    ov_window(app, ctx, t, Win::Overview, &t!("Game overview"), pos, vec2(460.0, 600.0), |app, ui| {
        egui::ScrollArea::vertical().id_salt("ov-overview").auto_shrink([false, false]).show(ui, |ui| {
            let wdt = ui.available_width();
            let (r, _) = ui.allocate_exact_size(vec2(wdt, wdt * 215.0 / 460.0), Sense::hover());
            w::image_or_title(ui, t, r, &art::header(id), &name, t.corner_small());
            ui.add_space(8.0);
            ui.label(w::text_bold(&name, 18.0, t.text));
            ui.add_space(4.0);
            // Playtime.
            let me = app.state.steam_id;
            let total = match app.state.player_games.get(&me) {
                Some(Ok(list)) => list.iter().find(|g| g.app_id == id).map(|g| (g.playtime_minutes, g.playtime_2weeks)),
                _ => None,
            };
            egui::Grid::new("ov-playtime").num_columns(2).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                ui.label(w::caption_text(&t, t!("THIS SESSION")));
                ui.label(w::text(session_text(since), 13.0, t.text));
                ui.end_row();
                if let Some((all, recent)) = total {
                    ui.label(w::caption_text(&t, t!("PLAYED IN TOTAL")));
                    ui.label(w::text(hours(all), 13.0, t.text));
                    ui.end_row();
                    ui.label(w::caption_text(&t, t!("LAST TWO WEEKS")));
                    ui.label(w::text(hours(recent), 13.0, t.text));
                    ui.end_row();
                }
            });
            // Achievements.
            ui.add_space(10.0);
            match app.state.achievements.get(&id).copied() {
                Some((done, all)) if all > 0 => {
                    ui.horizontal(|ui| {
                        ui.label(w::caption_text(&t, t!("ACHIEVEMENTS")));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(w::text(t!("{done} of {all}", done = done, all = all), 12.5, t.text_dim));
                        });
                    });
                    w::progress(ui, t, done as f32 / all as f32, ui.available_width());
                    ui.add_space(6.0);
                    if w::button(ui, t, Some(icons::TROPHY), t!("View achievements"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        app.overlay.win_mut(Win::Achievements).open = true;
                    }
                }
                Some(_) => {
                    ui.label(w::text(t!("This game has no achievements."), 12.5, t.text_faint));
                }
                None => {
                    ui.label(w::text(t!("Loading achievements…"), 12.5, t.text_faint));
                }
            }
            // Links, like the overview's shortcuts in Steam.
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                use crate::builtin::hub::{HubTab, View};
                if w::button(ui, t, Some(icons::STOREFRONT), t!("Store page"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    open_store(app, id);
                }
                for (icon, label, view) in [
                    (icons::USERS, t!("Community hub"), View::Hub { app: id, tab: HubTab::All }),
                    (icons::BOOKS, t!("Guides"), View::Guides(id)),
                    (icons::CHATS, t!("Discussions"), View::Forum { app: id, forum: String::new() }),
                ] {
                    if w::button(ui, t, Some(icon), label, w::Kind::Ghost, w::Size::Sm).clicked() {
                        open_community(app, view, true);
                    }
                }
            });
        });
    });
}

fn achievements_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme) {
    use crate::builtin::hub::View;
    let Some((id, _)) = app.overlay.game else { return };
    let pos = pos2(520.0, STRIP + 40.0);
    // The full achievements page, with "compare with a friend" inside the window.
    if !matches!(app.overlay.achievements.current(), Some(View::Achievements { app, .. }) if *app == id) {
        app.overlay.achievements.open(View::Achievements { app: id, friend: None }, true);
    }
    let mut toasts = Vec::new();
    ov_window(app, ctx, t, Win::Achievements, &t!("Achievements"), pos, vec2(600.0, 620.0), |app, ui| {
        let (cmds, acts, open) = {
            let mut io = crate::builtin::hub::Io::new(&app.state, &app.local, true);
            app.overlay.achievements.show(ui, t, &mut io);
            (io.cmds, io.actions, io.open)
        };
        for c in cmds {
            app.client.send(c);
        }
        toasts.extend(acts);
        for v in open {
            match v {
                View::Achievements { .. } => app.overlay.achievements.open(v, true),
                other => open_community(app, other, true),
            }
        }
    });
    for a in toasts {
        if let Action::Toast(s) = a {
            let now = ctx.input(|i| i.time);
            app.toast(now, s, "");
        }
    }
}

/// Show a page in the overlay's Community window (or the Store window).
/// `fresh` starts a new back history, for the overlay's own buttons.
fn open_community(app: &mut SteamApp, view: crate::builtin::hub::View, fresh: bool) {
    use crate::builtin::hub::View;
    match view {
        View::Page(page, arg) if page == "store" => {
            if let Some(id) = arg.as_deref().and_then(|a| a.strip_prefix("app:")).and_then(|a| a.parse().ok()) {
                open_store(app, id);
            }
        }
        View::Page(..) => {}
        v => {
            app.overlay.community.open(v, fresh);
            app.overlay.win_mut(Win::Community).open = true;
        }
    }
}

fn open_store(app: &mut SteamApp, id: AppId) {
    app.overlay.store_app = Some((id, false));
    app.overlay.win_mut(Win::Store).open = true;
}

/// Guides, discussions, the community hub and Workshop items, in a window.
fn community_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme, actions: &mut Vec<Action>) {
    use crate::builtin::hub::View;
    let Some(title) = app.overlay.community.current().map(|v| v.title(&app.state)) else { return };
    let pos = pos2(300.0, STRIP + 30.0);
    let size = vec2(980.0, (ctx.content_rect().height() - STRIP - 80.0).clamp(400.0, 820.0));
    ov_window(app, ctx, t, Win::Community, &title, pos, size, |app, ui| {
        let (cmds, acts, open) = {
            let mut io = crate::builtin::hub::Io::new(&app.state, &app.local, true);
            app.overlay.community.show(ui, t, &mut io);
            (io.cmds, io.actions, io.open)
        };
        for c in cmds {
            app.client.send(c);
        }
        actions.extend(acts);
        for v in open {
            match v {
                // Pages the overlay doesn't have (profiles, the Workshop browser) open in the main window.
                View::Page(page, arg) if page != "store" => actions.push(Action::Navigate { page, arg }),
                v => open_community(app, v, false),
            }
        }
    });
}

/// The game's store page (the app's native store) in a window.
fn store_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme, actions: &mut Vec<Action>) {
    let Some((id, sent)) = app.overlay.store_app else { return };
    if !app.overlay.win(Win::Store).open {
        return;
    }
    if !sent {
        app.registry.navigate("store", Some(&format!("app:{id}")));
        app.overlay.store_app = Some((id, true));
    }
    let title = app.state.app_name(id).map(|n| format!("{n} · {}", t!("Store page"))).unwrap_or_else(|| t!("Store page").to_string());
    let pos = pos2(260.0, STRIP + 50.0);
    let size = vec2(1000.0, (ctx.content_rect().height() - STRIP - 100.0).clamp(400.0, 800.0));
    ov_window(app, ctx, t, Win::Store, &title, pos, size, |app, ui| {
        let env = extensions::Env { state: &app.state, local: &app.local, theme: &app.prefs.theme, zoom: app.prefs.zoom, client: &app.client, actions: &mut *actions };
        if !app.registry.show_page("store", ui, env) {
            ui.label(w::text(t!("The store isn't available."), 13.0, t.text_faint));
        }
    });
}

fn friends_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme, actions: &mut Vec<Action>) {
    let screen = ctx.content_rect();
    let pos = pos2(screen.right() - 370.0, STRIP + 40.0);
    ov_window(app, ctx, t, Win::Friends, &t!("Friends"), pos, vec2(330.0, 600.0), |app, ui| {
        w::search(ui, t, &mut app.overlay.search, &t!("Search friends & groups"), ui.available_width());
        ui.add_space(8.0);
        let sel_group = match app.overlay.chat_with {
            Some(crate::builtin::group_chat::ChatTarget::Group(gid)) => Some(gid),
            _ => None,
        };
        let search = app.overlay.search.clone();
        let mut g_ui = std::mem::take(&mut app.overlay.group_ui);
        let clicked_group = crate::builtin::group_chat::group_list_section(ui, t, &app.state, &search, sel_group, &mut g_ui, &app.client, actions);
        app.overlay.group_ui = g_ui;
        if let Some(gid) = clicked_group {
            open_group_chat(app, gid);
        }
        ui.add_space(6.0);
        let q = app.overlay.search.trim().to_lowercase();
        let mut in_game = Vec::new();
        let mut online = Vec::new();
        let mut offline = Vec::new();
        for (id, persona) in app.state.friend_refs() {
            let name = persona.map_or("", |p| p.name.as_str());
            if !model::contains_ignore_case(name, &q) {
                continue;
            }
            let list = if persona.is_some_and(Persona::in_game) {
                &mut in_game
            } else if persona.is_some_and(|p| p.state != PersonaState::Offline) {
                &mut online
            } else {
                &mut offline
            };
            list.push(id);
        }
        egui::ScrollArea::vertical().id_salt("ov-friends-list").auto_shrink([false, false]).show(ui, |ui| {
            for (title, list, collapsible) in [(t!("In game"), &in_game, false), (t!("Online"), &online, false), (t!("Offline"), &offline, true)] {
                if list.is_empty() {
                    continue;
                }
                let open_section = !collapsible || app.overlay.show_offline || !q.is_empty();
                let head = format!("{}  {} — {}", if open_section { icons::CARET_DOWN } else { icons::CARET_RIGHT }, title.to_uppercase(), list.len());
                let r = ui.add(egui::Label::new(w::text_bold(head, 11.0, t.text_faint)).sense(Sense::click()));
                if collapsible && r.clicked() {
                    app.overlay.show_offline = !app.overlay.show_offline;
                }
                ui.add_space(4.0);
                if open_section {
                    for &id in list {
                        friend_row(app, ui, t, id, actions);
                    }
                }
                ui.add_space(8.0);
            }
            if in_game.is_empty() && online.is_empty() && offline.is_empty() {
                ui.label(w::text(t!("No friends match."), 13.0, t.text_faint));
            }
        });
    });
}

fn friend_row(app: &mut SteamApp, ui: &mut Ui, t: &Theme, id: model::SteamId, actions: &mut Vec<Action>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    if !ui.is_rect_visible(rect) && !resp.context_menu_opened() {
        return;
    }
    let persona = app.state.persona(id);
    let p = &persona;
    // The pointer anywhere on the row, its quick buttons included: those take
    // the hover from the row, and checking the row alone made them flicker.
    let over = ui.rect_contains_pointer(rect);
    let hover = over || resp.hovered() || resp.context_menu_opened();
    if hover || app.overlay.chat_with == Some(crate::builtin::group_chat::ChatTarget::Friend(p.steam_id)) {
        ui.painter().rect_filled(rect, t.corner_small(), t.surface_hover);
    }
    let av = Rect::from_min_size(rect.min + vec2(4.0, 7.0), vec2(32.0, 32.0));
    let offline = p.state == PersonaState::Offline;
    if !w::paint_cover(ui, av, &art::avatar(p.avatar_hash.as_deref()), CornerRadius::same(4)) {
        w::skeleton(ui, t, av, CornerRadius::same(4));
    }
    if offline {
        ui.painter().rect_filled(av, CornerRadius::same(4), Color32::from_black_alpha(110));
    }
    let color = crate::shell::status_color(t, p);
    let name_color = if p.in_game() { t.in_game } else if offline { t.text_dim } else { t.text };
    ui.painter().text(pos2(av.right() + 10.0, rect.top() + 15.0), Align2::LEFT_CENTER, &p.name, w::font_semibold(13.5), name_color);
    ui.painter().text(pos2(av.right() + 10.0, rect.top() + 32.0), Align2::LEFT_CENTER, crate::shell::status_text(&app.state, p), w::font(11.5), if offline { t.text_faint } else { color });
    let unread = app.state.unread.get(&p.steam_id).copied().unwrap_or(0);
    if unread > 0 {
        let c = pos2(rect.right() - 14.0, rect.center().y);
        ui.painter().circle_filled(c, 9.0, t.accent);
        ui.painter().text(c, Align2::CENTER_CENTER, unread.to_string(), w::font_bold(10.5), Color32::WHITE);
    }
    // Quick actions on hover, like Steam's overlay. The invite matches what
    // the game reports: its lobby, its match (join string), or just the game.
    let options = crate::shell::invite_options(&app.state);
    let mut invite = None;
    if over && unread == 0 {
        let mut x = rect.right() - 6.0;
        let mut button = |ui: &mut Ui, icon: &str, tip: &str| -> bool {
            let r = Rect::from_center_size(pos2(x - 14.0, rect.center().y), vec2(28.0, 28.0));
            x -= 32.0;
            let b = ui.interact(r, egui::Id::unique(("ov-act", p.steam_id, icon)), Sense::click());
            ui.painter().rect_filled(r, CornerRadius::same(6), if b.hovered() { t.accent } else { t.raised });
            ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, w::font(14.0), if b.hovered() { t.on_accent } else { t.text });
            b.on_hover_text(tip).clicked()
        };
        if let Some((best, label)) = options.first() {
            if button(ui, icons::USER_PLUS, label) {
                invite = Some(best.clone());
            }
        }
        if button(ui, icons::CHAT, t!("Send a message")) {
            open_chat(app, p.steam_id, actions);
        }
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        open_chat(app, p.steam_id, actions);
    }
    resp.context_menu(|ui| {
        if ui.button(format!("{}  {}", icons::CHAT, t!("Send a message"))).clicked() {
            open_chat(app, p.steam_id, actions);
            ui.close();
        }
        for (option, label) in &options {
            if ui.button(format!("{}  {}", icons::USER_PLUS, label)).clicked() {
                invite = Some(option.clone());
                ui.close();
            }
        }
        if ui.button(format!("{}  {}", icons::USER_CIRCLE, t!("View profile"))).clicked() {
            actions.push(Action::Navigate { page: "profile".into(), arg: Some(p.steam_id.to_string()) });
            ui.close();
        }
    });
    if let Some(invite) = invite {
        crate::shell::send_invite(app, p, invite, actions);
    }
}

fn open_chat(app: &mut SteamApp, id: SteamId, actions: &mut Vec<Action>) {
    app.overlay.chat_with = Some(crate::builtin::group_chat::ChatTarget::Friend(id));
    app.overlay.win_mut(Win::Chat).open = true;
    app.client.send(Command::LoadHistory { friend: id });
    actions.push(Action::MarkRead(id));
}

fn open_group_chat(app: &mut SteamApp, group_id: model::GroupChatId) {
    app.overlay.chat_with = Some(crate::builtin::group_chat::ChatTarget::Group(group_id));
    app.overlay.group_ui.chat_id = None;
    app.overlay.win_mut(Win::Chat).open = true;
}

fn chat_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme, actions: &mut Vec<Action>) {
    let Some(target) = app.overlay.chat_with else { return };
    let screen = ctx.content_rect();
    let pos = pos2(screen.right() - 830.0, STRIP + 40.0);
    match target {
        crate::builtin::group_chat::ChatTarget::Friend(id) => {
            let p = app.state.persona(id);
            ov_window(app, ctx, t, Win::Chat, &p.name, pos, vec2(440.0, 520.0), |app, ui| {
                ui.horizontal(|ui| {
                    w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 30.0, Some(crate::shell::status_color(t, &p)));
                    ui.label(w::text(crate::shell::status_text(&app.state, &p), 12.5, t.text_dim));
                });
                ui.add_space(6.0);
                // Exactly the space the window has, so it never grows by itself.
                let h = ui.available_height();
                actions.extend(crate::builtin::community::chat_body(ui, t, &app.state, &app.client, id, &mut app.overlay.input, h));
            });
        }
        crate::builtin::group_chat::ChatTarget::Group(gid) => {
            let Some(g) = app.state.chat_groups.get(&gid).cloned() else {
                app.overlay.chat_with = None;
                app.overlay.win_mut(Win::Chat).open = false;
                return;
            };
            let title = app.state.group_display_name(&g);
            ov_window(app, ctx, t, Win::Chat, &title, pos, vec2(500.0, 540.0), |app, ui| {
                let mut g_ui = std::mem::take(&mut app.overlay.group_ui);
                ui.horizontal(|ui| {
                    crate::builtin::group_chat::group_avatar(ui, t, &g, &title, 28.0);
                    ui.label(w::text(t!("{n} members", n = g.active_member_count.max(g.members.len() as u32)), 12.5, t.text_dim));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        crate::builtin::group_chat::header_actions(ui, t, &app.state, &app.client, &g, &mut g_ui, true);
                    });
                });
                ui.add_space(6.0);
                let h = ui.available_height();
                crate::builtin::group_chat::group_view(ui, t, &app.state, &app.client, gid, &mut g_ui, actions, true, h);
                app.overlay.group_ui = g_ui;
            });
        }
    }
    if !app.overlay.win(Win::Chat).open {
        app.overlay.chat_with = None;
    }
}

/// Draw a crisp software arrow cursor inside the in-game overlay so games that
/// hide the OS/X11 cursor (like Helldivers 2) still show a visible pointer.
fn software_cursor(ctx: &egui::Context) {
    let Some(p) = ctx.pointer_hover_pos() else { return };
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("ov-soft-cursor"));
    let painter = ctx.layer_painter(layer);
    let pts = [
        p,
        p + vec2(0.0, 17.0),
        p + vec2(4.2, 13.2),
        p + vec2(7.4, 19.5),
        p + vec2(9.8, 18.3),
        p + vec2(6.8, 12.2),
        p + vec2(12.2, 12.2),
    ];
    painter.add(egui::Shape::convex_polygon(
        vec![pts[0], pts[1], pts[2], pts[6]],
        Color32::WHITE,
        Stroke::new(1.5, Color32::BLACK),
    ));
    painter.line_segment([pts[2], pts[3]], Stroke::new(3.0, Color32::BLACK));
    painter.line_segment([pts[2], pts[3]], Stroke::new(1.6, Color32::WHITE));
}

fn notes_path(app_id: AppId) -> std::path::PathBuf {
    client::store::config_dir().join("notes").join(format!("{app_id}.txt"))
}

fn notes_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme) {
    let screen = ctx.content_rect();
    let pos = pos2(520.0, (screen.bottom() - 460.0).max(STRIP + 80.0));
    ov_window(app, ctx, t, Win::Notes, &t!("Notes"), pos, vec2(360.0, 420.0), |app, ui| {
        ui.label(w::text(t!("Jot things down while you play. Saved automatically for this game."), 12.0, t.text_dim));
        ui.add_space(6.0);
        let size = ui.available_size();
        if ui.add_sized(size, egui::TextEdit::multiline(&mut app.overlay.notes).font(w::font(14.0))).with_edit_menu().changed() {
            app.overlay.notes_dirty = Some(Instant::now());
        }
    });
}

fn screenshots_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme) {
    let Some((id, _)) = app.overlay.game else { return };
    if app.overlay.win(Win::Screenshots).open && app.overlay.screenshots_dirty {
        let account = (app.state.steam_id & 0xFFFF_FFFF) as u32;
        app.overlay.screenshots = client::ingame::list_screenshots(&app.local.steam_root, account, id);
        app.overlay.screenshots_dirty = false;
    }
    let screen = ctx.content_rect();
    let pos = pos2(160.0, STRIP + 60.0);
    let mut delete_path: Option<std::path::PathBuf> = None;
    ov_window(app, ctx, t, Win::Screenshots, &t!("Screenshots"), pos, vec2(520.0, (screen.height() - STRIP - 140.0).clamp(320.0, 620.0)), |app, ui| {
        ui.horizontal_wrapped(|ui| {
            if w::button(ui, t, Some(icons::CAMERA), t!("Take screenshot (F12)"), w::Kind::Primary, w::Size::Sm).clicked() {
                app.overlay.take_shot_requested = true;
                // Close overlay so the screenshot captures the game itself.
                if app.overlay.ingame.is_none() {
                    app.overlay.close();
                } else {
                    app.overlay.open = false;
                }
            }
            if w::button(ui, t, Some(icons::FOLDER), t!("Open folder"), w::Kind::Secondary, w::Size::Sm).clicked() {
                let account = (app.state.steam_id & 0xFFFF_FFFF) as u32;
                let dir = client::ingame::screenshots_dir(&app.local.steam_root, account, id);
                let _ = std::fs::create_dir_all(&dir);
                local::open_folder(&dir);
            }
            if w::button(ui, t, Some(icons::RESTORE), t!("Refresh"), w::Kind::Ghost, w::Size::Sm).clicked() {
                app.overlay.screenshots_dirty = true;
            }
        });
        ui.add_space(8.0);
        if app.overlay.screenshots.is_empty() {
            ui.label(w::text(t!("No screenshots for this game yet. Press F12 while playing to capture one."), 13.0, t.text_faint));
            return;
        }
        egui::ScrollArea::vertical().id_salt("ov-screenshots").auto_shrink([false, false]).show(ui, |ui| {
            for shot in &app.overlay.screenshots {
                egui::Frame::new()
                    .fill(t.surface)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(t.corner_small())
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(w::text_medium(&shot.name, 13.0, t.text));
                                ui.label(w::text(shot.path.display().to_string(), 11.0, t.text_faint));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if w::icon_button(ui, t, icons::TRASH, t!("Delete"), false, 0).clicked() {
                                    delete_path = Some(shot.path.clone());
                                }
                                if w::icon_button(ui, t, icons::EXTERNAL, t!("Open"), false, 0).clicked() {
                                    let _ = open::that(&shot.path);
                                }
                            });
                        });
                    });
                ui.add_space(6.0);
            }
        });
    });
    if let Some(path) = delete_path {
        if client::ingame::delete_screenshot(&path).is_ok() {
            app.overlay.screenshots_dirty = true;
        }
    }
}

fn controller_window(app: &mut SteamApp, ctx: &egui::Context, t: &Theme, actions: &mut Vec<Action>) {
    let Some((app_id, _)) = app.overlay.game else { return };
    let account = (app.state.steam_id & 0xFFFF_FFFF) as u32;
    let screen = ctx.content_rect();
    let pos = pos2(220.0, STRIP + 60.0);
    let mut open_settings = false;

    ov_window(
        app,
        ctx,
        t,
        Win::Controller,
        &t!("Controller"),
        pos,
        vec2(460.0, (screen.height() - STRIP - 140.0).clamp(320.0, 560.0)),
        |app, ui| {
            let live_route = app.overlay.controller_route_enabled;
            let applied_source = app.overlay.controller_applied_source.clone();
            let current = client::app_binding(app_id);
            let options = client::managed_options(&app.local.steam_root, account, app_id);

            ui.horizontal_wrapped(|ui| {
                match live_route {
                    Some(true) => {
                        w::pill(ui, t!("Live switching active"), t.play, t.play.gamma_multiply(0.14));
                    }
                    Some(false) => {
                        w::pill(ui, t!("Live apply unavailable for this launch"), t.away, t.away.gamma_multiply(0.14));
                    }
                    None => {
                        w::pill(ui, t!("Saved for next launch"), t.text_dim, t.raised);
                    }
                }
                if let Some(src) = applied_source.as_deref().filter(|s| !s.is_empty()) {
                    ui.label(w::text(t!("Applied: {source}", source = src), 12.0, t.text_dim));
                }
            });
            ui.add_space(6.0);

            if live_route == Some(false) {
                ui.add(
                    egui::Label::new(w::text(
                        t!("This launch is managed outside Aurora's Steam Input bridge. Changes will be saved for future Aurora-managed launches."),
                        12.0,
                        t.text_dim,
                    ))
                    .wrap(),
                );
                ui.add_space(6.0);
            }

            let can_apply_now = live_route != Some(false);
            let mut next_choice: Option<client::BindingChoice> = None;

            egui::ScrollArea::vertical().id_salt("ov-controller-layouts").max_height(340.0).show(ui, |ui| {
                let is_auto = current == client::BindingChoice::Auto;
                let auto_kind = if is_auto { w::Kind::Primary } else { w::Kind::Secondary };
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_apply_now || !is_auto, |ui: &mut Ui| {
                            w::button(ui, t, Some(icons::CONTROLLER), t!("Automatic (default)"), auto_kind, w::Size::Sm)
                        })
                        .clicked()
                    {
                        next_choice = Some(client::BindingChoice::Auto);
                    }
                });
                ui.add_space(6.0);

                ui.label(w::caption_text(t, t!("TEMPLATES & SAVED LAYOUTS")));
                ui.add_space(4.0);
                for opt in &options {
                    let active = current == opt.choice;
                    let fill = if active { w::active_fill(t) } else { t.surface };
                    let stroke = Stroke::new(1.0, if active { t.accent } else { t.border });
                    egui::Frame::new()
                        .fill(fill)
                        .stroke(stroke)
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(w::text_semibold(&opt.label, 13.0, t.text));
                                    let kind_lbl = match opt.origin {
                                        client::BindingOrigin::Template => t!("Template"),
                                        client::BindingOrigin::Community(_) => t!("Community"),
                                        client::BindingOrigin::Custom => t!("Custom"),
                                        _ => t!("Configuration"),
                                    };
                                    ui.label(w::text(kind_lbl, 11.5, t.text_dim));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if active {
                                        w::pill(ui, t!("Active"), t.play, t.play.gamma_multiply(0.14));
                                    } else if w::button(ui, t, None, t!("Apply"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                        next_choice = Some(opt.choice.clone());
                                    }
                                });
                            });
                        });
                    ui.add_space(4.0);
                }
            });

            if let Some(choice) = next_choice {
                client::set_app_binding(app_id, choice);
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::SLIDERS), t!("Edit Layout…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    open_settings = true;
                }
            });
        },
    );

    if open_settings {
        actions.push(Action::Navigate { page: "settings".into(), arg: Some(format!("controller:{app_id}")) });
        app.overlay.close();
    }
}

fn fps_hud(app: &SteamApp, ctx: &egui::Context, t: &Theme) {
    use crate::builtin::settings::FpsCorner;
    let corner = app.prefs.fps_counter;
    if corner == FpsCorner::Off {
        return;
    }
    let Some(fps) = app.overlay.fps else { return };
    let (align, offset) = match corner {
        FpsCorner::Off => return,
        FpsCorner::TopLeft => (Align2::LEFT_TOP, vec2(12.0, if app.overlay.open { STRIP + 10.0 } else { 10.0 })),
        FpsCorner::TopRight => (Align2::RIGHT_TOP, vec2(-12.0, if app.overlay.open { STRIP + 10.0 } else { 10.0 })),
        FpsCorner::BottomLeft => (Align2::LEFT_BOTTOM, vec2(12.0, -10.0)),
        FpsCorner::BottomRight => (Align2::RIGHT_BOTTOM, vec2(-12.0, -10.0)),
    };
    let fg = if app.prefs.fps_high_contrast { Color32::from_rgb(0x4A, 0xFF, 0x4A) } else { t.text_dim };
    let bg = if app.prefs.fps_high_contrast { Color32::from_black_alpha(190) } else { Color32::from_black_alpha(130) };
    egui::Area::new(egui::Id::unique("ov-fps-hud"))
        .anchor(align, offset)
        .interactable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(bg)
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{fps} FPS"))
                                .font(w::font_bold(13.0))
                                .color(fg)
                                .monospace(),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .selectable(false),
                    );
                });
        });
}

#[cfg(test)]
mod win_path_tests {
    use super::win_exe_in_dir;

    #[test]
    fn respects_path_component_boundaries() {
        assert!(win_exe_in_dir(r"C:\Games\App\game.exe", r"C:\Games\App"));
        assert!(win_exe_in_dir(r"c:/games/app/sub/game.exe", r"C:\Games\App\"));
        assert!(!win_exe_in_dir(r"C:\Games\App2\game.exe", r"C:\Games\App"));
        assert!(!win_exe_in_dir(r"C:\Games\App", r"C:\Games\App"));
        assert!(!win_exe_in_dir(r"C:\Games\App\game.exe", ""));
    }
}

fn toasts_hud(app: &SteamApp, ctx: &egui::Context, t: &Theme) {
    if app.toasts.is_empty() || app.overlay.ingame.is_none() {
        return;
    }
    egui::Area::new(egui::Id::unique("ov-toasts-hud"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0))
        .interactable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_max_width(320.0);
            for toast in app.toasts.iter().rev().take(3) {
                egui::Frame::new()
                    .fill(t.chrome.gamma_multiply(0.94))
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(t.corner_small())
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.label(w::text_semibold(&toast.title, 13.0, t.text));
                        if !toast.body.is_empty() {
                            ui.label(w::text(&toast.body, 12.0, t.text_dim));
                        }
                    });
                ui.add_space(6.0);
            }
        });
}
