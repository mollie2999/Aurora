//! App root: owns the engine, the state snapshot, extensions, prefs and the overlay.

use client::{Client, Command, Event, SavedAccount, State, state::Phase};
use extensions::{Action, Env, Registry};
use local::LocalState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use ui::Theme;

/// Shared theme manager state between `SteamApp` and `Settings → Appearance`.
pub struct ThemeState {
    pub manager: themes::ThemeManager,
    pub hot_reload: bool,
    pub last_error: Option<String>,
    pub dirty: bool,
    pub revision: u64,
}

pub type SharedThemeManager = Arc<Mutex<ThemeState>>;
pub type SharedLibraryLayout = Arc<Mutex<Vec<themes::Node>>>;

fn lcd_subpixel_enabled() -> bool {
    cfg!(target_os = "linux") && std::env::var("STEAMRS_LCD").as_deref() != Ok("0")
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: Theme,
    pub zoom: f32,
    pub disabled: Vec<String>,
    pub ext_data: HashMap<String, String>,
    pub page: String,
    pub friends_open: bool,
    pub overlay_enabled: bool,
    #[serde(default)]
    pub fps_counter: crate::builtin::settings::FpsCorner,
    #[serde(default)]
    pub fps_high_contrast: bool,
    /// Show the "What's new" news shelf on the library home.
    #[serde(default = "yes")]
    pub show_whats_new: bool,
    /// Account to sign in automatically at startup (cleared on log out).
    pub auto_login: Option<String>,
    /// Interface language code; `None` follows the system.
    pub language: Option<String>,
    /// The first-launch language question was answered.
    pub language_asked: bool,
    pub close_to_tray: bool,
    /// Show the account picker at startup instead of signing in automatically.
    pub ask_account: bool,
    #[serde(default = "yes")]
    pub auto_updates: bool,
    #[serde(default = "yes")]
    pub cloud: bool,
    #[serde(default = "yes")]
    pub toast_messages: bool,
    #[serde(default = "yes")]
    pub toast_notifications: bool,
    /// Offline mode was on at exit: start offline again, like Steam.
    pub wants_offline: bool,
    /// Start in offline mode by itself when Steam can't be reached.
    pub auto_offline: bool,
    /// Friends pinned to the top of the list ("Favorites"), like Steam.
    #[serde(default)]
    pub favorite_friends: Vec<u64>,
    /// Mouse wheel and touchpad scroll distance, as a multiple of egui's.
    pub scroll_speed: f32,
    /// The player has seen the one-time notice for Steam-DRM games (how they
    /// start through a background Steam instance).
    #[serde(default)]
    pub drm_host_ack: bool,
    /// Linux: a Proton/compat tool (internal name) chosen to run Windows games
    /// with, instead of Steam's pick. `None` follows Steam's choice.
    #[serde(default)]
    pub proton_override: Option<String>,
    /// Linux: a Proton forced for single games (their Manage menu).
    #[serde(default)]
    pub proton_per_app: std::collections::BTreeMap<model::AppId, String>,
    /// Games whose first-launch anti-cheat warning was accepted.
    pub anti_cheat_acknowledged: BTreeSet<model::AppId>,
    /// Linux: compile a game's shaders before it starts (Settings > Downloads).
    #[serde(default = "yes")]
    pub precompile_shaders: bool,
    /// Steam's "Allow downloads during gameplay", set before every game.
    #[serde(default)]
    pub gameplay_downloads: bool,
    /// Global controller & Steam Input settings (Settings → Controller).
    #[serde(default)]
    pub controller: client::ControllerSettings,
    /// Per-game selected Steam Input configuration (Auto, LocalFile, Workshop).
    #[serde(default)]
    pub controller_bindings: std::collections::BTreeMap<model::AppId, client::BindingChoice>,
    /// The main window's last size, position and maximized state, restored on
    /// the next run (the sign-in and splash windows always center small).
    #[serde(default)]
    pub win_size: Option<[f32; 2]>,
    #[serde(default)]
    pub win_pos: Option<[f32; 2]>,
    #[serde(default)]
    pub win_maximized: bool,
    /// Active installed theme id (`None` = Default Aurora UI).
    #[serde(default)]
    pub active_theme_id: Option<String>,
    /// Watch active theme files on disk and hot-reload on change.
    #[serde(default)]
    pub hot_reload_themes: bool,
    /// Start in Big Picture after signing in.
    pub bigpicture_start: bool,
    /// Big Picture interface scale relative to its screen size.
    pub bigpicture_scale: f32,
    pub bigpicture_library: u8,
    pub bigpicture_swap_ab: bool,
    pub bigpicture_repeat: u8,
    pub bigpicture_stick: u8,
}

fn yes() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            zoom: 1.0,
            disabled: Vec::new(),
            ext_data: HashMap::new(),
            page: "library".into(),
            friends_open: true,
            overlay_enabled: true,
            fps_counter: crate::builtin::settings::FpsCorner::Off,
            fps_high_contrast: false,
            show_whats_new: true,
            auto_login: None,
            language: None,
            language_asked: false,
            close_to_tray: true,
            ask_account: false,
            auto_updates: true,
            cloud: true,
            toast_messages: true,
            toast_notifications: true,
            wants_offline: false,
            auto_offline: false,
            favorite_friends: Vec::new(),
            scroll_speed: 2.0,
            drm_host_ack: false,
            proton_override: None,
            proton_per_app: Default::default(),
            anti_cheat_acknowledged: BTreeSet::new(),
            precompile_shaders: true,
            gameplay_downloads: false,
            controller: client::ControllerSettings::default(),
            controller_bindings: Default::default(),
            win_size: None,
            win_pos: None,
            win_maximized: false,
            active_theme_id: None,
            hot_reload_themes: false,
            bigpicture_start: false,
            bigpicture_scale: 1.0,
            bigpicture_library: 0,
            bigpicture_swap_ab: false,
            bigpicture_repeat: 1,
            bigpicture_stick: 1,
        }
    }
}

#[derive(Default)]
pub struct Activate {
    pub key: String,
    pub busy: bool,
    pub result: Option<Result<Vec<String>, String>>,
}

pub struct Toast {
    pub title: String,
    pub body: String,
    pub until: f64,
}

struct AntiCheatNotice {
    app_id: model::AppId,
    install_dir: PathBuf,
    args: client::LaunchArgs,
    skip_cloud: bool,
}

pub struct SteamApp {
    pub(crate) _rt: tokio::runtime::Runtime,
    pub client: Client,
    events: mpsc::Receiver<Event>,
    pub state: State,
    pub local: LocalState,
    local_tx: mpsc::Sender<LocalState>,
    local_rx: mpsc::Receiver<LocalState>,
    pub registry: Registry,
    pub prefs: Prefs,
    pub saved: Vec<SavedAccount>,
    /// First launch: the language picker is showing, with this one picked.
    pub ask_language: Option<i18n::Lang>,
    pub login: crate::login::LoginForm,
    pub toasts: Vec<Toast>,
    pub friend_filter: String,
    /// An open friend dialog (nickname, server info, remove, block) and the
    /// window it belongs to.
    pub friend_dialog: Option<(egui::ViewportId, crate::shell::FriendDialog)>,
    /// Back/forward navigation stacks of (page, arg).
    pub back: Vec<(String, Option<String>)>,
    pub forward: Vec<(String, Option<String>)>,
    pub current_arg: Option<String>,
    pub overlay: crate::overlay::Overlay,
    pub demo: bool,
    pub activate: Option<Activate>,
    /// Set by "Exit" so closing really quits instead of hiding to the tray.
    pub exiting: bool,
    /// Items in the Steam cart last time we heard (for "added" toasts).
    cart_count: Option<usize>,
    /// "A download is in progress, exit anyway?" is showing.
    confirm_exit: bool,
    window_mode: Option<WindowMode>,
    pub(crate) bigpicture: crate::bigpicture::BigPicture,
    /// A window size waiting to be centered once the monitor size is known.
    center_window: Option<[f32; 2]>,
    /// A window size asked for but not reached yet: (size, resizable, frames
    /// waited).
    pending_size: Option<([f32; 2], bool, u32)>,
    pub tray: crate::tray::Tray,
    taskbar: crate::taskbar::Taskbar,
    flags: crate::builtin::settings::SharedFlags,
    pub friends_window: crate::friends_window::FriendsWindow,
    links: std::sync::mpsc::Receiver<crate::links::LinkRequest>,
    pub launch_popup: Option<crate::launch_popup::LaunchPopup>,
    anti_cheat_notices: VecDeque<AntiCheatNotice>,
    /// Links waiting for sign-in to finish.
    pending_links: Vec<String>,
    /// Main window is hidden in the tray.
    hidden: bool,
    tray_restore_retry_at: Option<std::time::Instant>,
    pending_memory_trim: u8,
    applied_theme: Option<Theme>,
    applied_zoom: f32,
    pub themes: SharedThemeManager,
    pub library_layout: SharedLibraryLayout,
    resolved_snapshot: Option<themes::ResolvedTheme>,
    synced_revision: u64,
    preferences_revision: Option<u64>,
    applied_fonts_key: Option<String>,
    bg_tex: Option<(PathBuf, egui::TextureWrapMode, egui::TextureHandle)>,
    logo_tex: Option<(PathBuf, egui::TextureWrapMode, egui::TextureHandle)>,
    last_hot_reload_check: f64,
    native_appearance: Option<themes::native::NativeAppearanceWatcher>,
    applied_native_artwork: Option<Arc<ui::native_style::NativeControlArtwork>>,
}

impl SteamApp {
    pub fn new(cc: &eframe::CreationContext<'_>, link: Option<String>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        // Subpixel text on Linux unless turned off (e.g. for a BGR or rotated
        // screen). Other systems keep grayscale text: only Linux gets the
        // per-channel blending (see vendor/egui_glow).
        ui::install_fonts_with(&cc.egui_ctx, lcd_subpixel_enabled());
        let mut prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, "prefs")).unwrap_or_default();
        let cfg_dir = client::store::config_dir();
        let mut theme_manager = themes::ThemeManager::new(cfg_dir.join("themes"), cfg_dir.join("theme-data"));
        match theme_manager.sync_bundled_themes() {
            Ok(report) => {
                if !report.installed.is_empty() || !report.updated.is_empty() {
                    tracing::info!(
                        installed = ?report.installed,
                        updated = ?report.updated,
                        "synced bundled themes"
                    );
                }
            }
            Err(e) => tracing::warn!("sync bundled themes: {e}"),
        }
        theme_manager.scan();
        let mut last_error = None;
        if let Some(id) = prefs.active_theme_id.as_deref() {
            if let Err(e) = theme_manager.activate(Some(id)) {
                tracing::warn!("activate theme '{id}': {e}");
                last_error = Some(e.to_string());
            }
        }
        let initial_lib_nodes = theme_manager
            .resolved
            .as_ref()
            .map(|r| r.layout.library.clone())
            .unwrap_or_default();
        let initial_snapshot = theme_manager.resolved.as_ref().map(snapshot_resolved);
        prefs.theme = theme_manager
            .resolved
            .as_ref()
            .map(|r| r.theme.clone())
            .unwrap_or_else(Theme::default);
        let themes = Arc::new(Mutex::new(ThemeState {
            manager: theme_manager,
            hot_reload: prefs.hot_reload_themes,
            last_error,
            dirty: true,
            revision: 1,
        }));
        let library_layout = Arc::new(Mutex::new(initial_lib_nodes));
        // `--lang=en` / `--lang=ru` overrides the saved choice (screenshots, testing).
        let lang_flag = std::env::args().find_map(|a| a.strip_prefix("--lang=").map(String::from));
        // The language picked in setup counts until one is chosen in the app.
        let from_setup = || std::fs::read_to_string(client::store::config_dir().join("language")).ok().map(|s| s.trim().to_string());
        // First launch (nothing chosen anywhere yet): ask before sign-in.
        let first_launch = lang_flag.is_none() && prefs.language.is_none() && !prefs.language_asked && from_setup().is_none();
        i18n::set_language(lang_flag.or_else(|| prefs.language.clone()).or_else(from_setup).as_deref().and_then(i18n::Lang::from_code).unwrap_or_else(i18n::Lang::detect));
        let ask_language = first_launch.then(i18n::language);

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .enable_all()
            .build()
            .expect("tokio runtime");
        // Pictures come from the disk cache when possible (added after the
        // standard loaders, so it's asked first).
        crate::image_cache::ImageCache::install(&cc.egui_ctx, rt.handle().clone());
        // Store pages and game news, saved next to the pictures.
        store::disk::init(crate::image_cache::dir().with_file_name("pages"));
        // `--no-local`: don't read this PC's Steam library (clean demo screenshots).
        let local = if std::env::args().any(|a| a == "--no-local") {
            LocalState::default()
        } else {
            local::load(None).unwrap_or_else(|e| {
                tracing::warn!("local Steam library: {e}");
                LocalState::default()
            })
        };

        let demo = std::env::args().any(|a| a == "--demo");
        let (tx, events) = mpsc::channel();
        let repaint = cc.egui_ctx.clone();
        let sink: client::Sink = Arc::new(move |e| {
            let _ = tx.send(e);
            repaint.request_repaint_of(egui::ViewportId::ROOT);
            repaint.request_repaint_of(egui::ViewportId::from_hash_of("friends-window"));
        });
        let client = if demo {
            crate::demo::spawn(rt.handle(), &local, sink)
        } else {
            let c = Client::spawn(rt.handle(), sink);
            // Tell the session the setting before it signs in and would otherwise
            // resume saved downloads on its own.
            c.send(Command::SetAutoDownloads(prefs.auto_updates));
            c
        };

        let flags = std::sync::Arc::new(std::sync::Mutex::new(crate::builtin::settings::AppFlags {
            close_to_tray: prefs.close_to_tray,
            ask_account: prefs.ask_account,
            auto_updates: prefs.auto_updates,
            cloud: prefs.cloud,
            toast_messages: prefs.toast_messages,
            toast_notifications: prefs.toast_notifications,
            scroll_speed: prefs.scroll_speed,
            drm_host_ack: prefs.drm_host_ack,
            overlay_enabled: prefs.overlay_enabled,
            fps_counter: prefs.fps_counter,
            fps_high_contrast: prefs.fps_high_contrast,
            show_whats_new: prefs.show_whats_new,
        }));
        client::set_proton_override(prefs.proton_override.clone());
        client::set_app_protons(prefs.proton_per_app.clone());
        client::set_anti_cheat_acknowledged(prefs.anti_cheat_acknowledged.clone());
        client::set_gameplay_downloads(prefs.gameplay_downloads);
        client::set_controller_settings(prefs.controller.clone());
        client::set_app_bindings(prefs.controller_bindings.clone());
        #[cfg(not(windows))]
        client::shaders::set_precompile(prefs.precompile_shaders);
        let mut registry = crate::builtin::registry(rt.handle().clone(), cc.egui_ctx.clone(), flags.clone(), themes.clone(), library_layout.clone());
        for id in &prefs.disabled {
            registry.set_enabled(id, false);
        }
        for (id, data) in &prefs.ext_data {
            registry.load(id, data);
        }

        let saved = client::store::load();
        // Someone who already signed in here has used the app before.
        let ask_language = ask_language.filter(|_| saved.is_empty());
        let mut auto_account = None;
        if !demo {
            if let Some(acc) = prefs.auto_login.as_ref().filter(|_| !prefs.ask_account && !std::env::args().any(|a| a == "--no-auto-login")) {
                if saved.iter().any(|s| &s.account_name == acc) {
                    auto_account = Some(acc.clone());
                    // Offline mode sticks across restarts until "Go online".
                    client.send(if prefs.wants_offline { Command::StartOffline { account: acc.clone() } } else { Command::LoginSaved { account: acc.clone() } });
                }
            }
        }

        let overlay = crate::overlay::Overlay::new(&cc.egui_ctx);
        let mut bigpicture = crate::bigpicture::BigPicture::new(&cc.egui_ctx);
        if prefs.bigpicture_start && !bigpicture.active {
            bigpicture.enter(&cc.egui_ctx);
        }
        let (local_tx, local_rx) = mpsc::channel();
        let mut app = Self {
            _rt: rt,
            client,
            events,
            state: State::default(),
            local,
            local_tx,
            local_rx,
            registry,
            prefs,
            saved,
            ask_language,
            login: Default::default(),
            toasts: Vec::new(),
            friend_filter: String::new(),
            friend_dialog: None,
            back: Vec::new(),
            forward: Vec::new(),
            current_arg: None,
            overlay,
            demo,
            activate: None,
            exiting: false,
            cart_count: None,
            confirm_exit: false,
            window_mode: None,
            bigpicture,
            center_window: None,
            pending_size: None,
            tray: crate::tray::Tray::new(cc),
            taskbar: crate::taskbar::Taskbar::new(cc),
            flags,
            friends_window: Default::default(),
            links: crate::links::listen(cc.egui_ctx.clone()),
            launch_popup: None,
            anti_cheat_notices: VecDeque::new(),
            pending_links: link.into_iter().collect(),
            hidden: false,
            tray_restore_retry_at: None,
            pending_memory_trim: 0,
            applied_theme: None,
            applied_zoom: 0.0,
            themes,
            library_layout,
            resolved_snapshot: initial_snapshot,
            synced_revision: 0,
            preferences_revision: None,
            applied_fonts_key: Some(format!("{:?}", ui::theme::WeightPreset::Heavy)),
            bg_tex: None,
            logo_tex: None,
            last_hot_reload_check: 0.0,
            native_appearance: None,
            applied_native_artwork: None,
        };
        // `--page=inventory --arg=listings`: open a page at startup (handy with --demo).
        let flag = |name: &str| std::env::args().find_map(|a| a.strip_prefix(&format!("--{name}=")).map(String::from));
        if let Some(page) = flag("page") {
            app.go(page, flag("arg"));
        }
        app.login.signing_in = auto_account;
        // `--overlay` opens the Shift+Tab overlay right away (for previews).
        // `--friends` opens the Friends & Chat window right away (for previews).
        if std::env::args().any(|a| a == "--friends") {
            app.friends_window.open = true;
        }
        // `--tray-menu` opens the tray menu near the screen corner (previews).
        if std::env::args().any(|a| a == "--tray-menu") {
            app.tray.open_menu(egui::pos2(900.0, 700.0));
        }
        if std::env::args().any(|a| a == "--overlay") {
            if demo {
                // A pretend running game, so the preview has an overview.
                app.overlay.game_started(1145360, false);
            }
            app.overlay.open = true;
        }
        app
    }

    pub fn toast(&mut self, now: f64, title: impl Into<String>, body: impl Into<String>) {
        self.toasts.push(Toast { title: title.into(), body: body.into(), until: now + 5.0 });
    }

    fn reload_local_async(&self, ctx: &egui::Context) {
        let root = self.local.steam_root.clone();
        let tx = self.local_tx.clone();
        let ctx = ctx.clone();
        self._rt.spawn_blocking(move || {
            if let Ok(l) = local::load(Some(root)) {
                let _ = tx.send(l);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    }

    pub(crate) fn drain_events(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        while let Ok(l) = self.local_rx.try_recv() {
            self.local = l;
        }
        let now = ctx.input(|i| i.time);
        while let Ok(e) = self.events.try_recv() {
            // Toast-worthy things are decided before state changes.
            if let Event::Message(m) = &e {
                let open = (self.prefs.page == "community" && self.current_arg.as_deref() == Some(&m.friend.to_string()))
                    || (self.friends_window.open && self.friends_window.chat == Some(crate::builtin::group_chat::ChatTarget::Friend(m.friend)));
                if m.from != self.state.steam_id && !open && self.prefs.toast_messages {
                    let name = self.state.persona(m.from).name;
                    self.toast(now, name, m.text.clone());
                }
            }
            if let Event::GroupMessage(m) = &e {
                let open_main = self.prefs.page == "community"
                    && self
                        .current_arg
                        .as_deref()
                        .and_then(|s| s.strip_prefix("group:"))
                        .is_some_and(|rest| rest.split(':').next().and_then(|g| g.parse::<u64>().ok()) == Some(m.group_id));
                let open_fw = self.friends_window.open
                    && self.friends_window.chat == Some(crate::builtin::group_chat::ChatTarget::Group(m.group_id));
                if m.from != self.state.steam_id && m.server_msg.is_none() && !open_main && !open_fw && self.prefs.toast_messages {
                    if let Some(g) = self.state.chat_groups.get(&m.group_id) {
                        let ch_muted = g.channel(m.chat_id).is_some_and(|c| c.unread_muted);
                        let should = !g.unread_muted && !ch_muted && (m.mentioned_me || !g.is_saved() || g.desktop_notification_level == 4);
                        if should {
                            let group_name = self.state.group_display_name(g);
                            let sender = self.state.persona(m.from).display_name();
                            self.toast(now, group_name, format!("{sender}: {}", m.text));
                        }
                    }
                }
            }
            if let (Event::NotificationsArrived(list), true) = (&e, self.prefs.toast_notifications) {
                for n in list.iter().filter(|n| !n.read).take(3) {
                    let (_, text) = crate::shell::notification_text(&self.state, n);
                    self.toast(now, i18n::tr_dyn("Notification"), text);
                }
            }
            crate::offline_ui::on_event(self, &e, now);
            self.state.apply(&e);
            match &e {
                Event::LoggedIn { account, .. } => {
                    if !self.demo {
                        self.prefs.auto_login = Some(account.clone());
                    }
                    self.login = Default::default();
                }
                Event::LoggedOut { reason } => {
                    self.anti_cheat_notices.clear();
                    self.bigpicture.leave(ctx);
                    self.window_mode = None;
                    if reason == "Logged out" {
                        self.prefs.auto_login = None;
                    }
                    self.saved = client::store::load();
                }
                // Store actions that failed (they used to be silent).
                Event::StoreError(e) => self.toast(now, i18n::tr_dyn(e), ""),
                // Additions show in the store's "Added to your cart!" popup.
                Event::Cart(lines) => {
                    self.cart_count = Some(lines.len());
                }
                Event::LoginFailed(_) => {
                    self.prefs.auto_login = None;
                    self.saved = client::store::load();
                    self.login.on_failed();
                }
                Event::AccountInfo { .. } => self.saved = client::store::load(),
                Event::GroupJoined(gid) => {
                    if self.friends_window.open {
                        self.friends_window.chat = Some(crate::builtin::group_chat::ChatTarget::Group(*gid));
                        self.friends_window.group_ui.chat_id = None;
                    }
                }
                Event::ChatGroupRemoved(gid) => {
                    if self.friends_window.chat == Some(crate::builtin::group_chat::ChatTarget::Group(*gid)) {
                        self.friends_window.chat = None;
                        self.friends_window.group_ui.chat_id = None;
                    }
                }
                Event::AntiCheatWarning { app_id, install_dir, args, skip_cloud } => {
                    if !self.prefs.anti_cheat_acknowledged.contains(app_id)
                        && !self.anti_cheat_notices.iter().any(|notice| notice.app_id == *app_id)
                    {
                        self.anti_cheat_notices.push_back(AntiCheatNotice {
                            app_id: *app_id,
                            install_dir: install_dir.clone(),
                            args: args.clone(),
                            skip_cloud: *skip_cloud,
                        });
                    }
                }
                Event::GameStarted { app_id } => {
                    self.overlay.game_started(*app_id, self.prefs.overlay_enabled);
                    if let Some(p) = self.launch_popup.as_mut().filter(|p| p.app_id == *app_id) {
                        p.stage = crate::launch_popup::Stage::Running(now);
                    }
                }
                Event::GameStopped { .. } => {
                    self.overlay.game_stopped();
                    self.reload_local_async(ctx);
                }
                Event::ControllerLayoutRoute { app_id, enabled } => {
                    self.overlay.set_controller_route(*app_id, *enabled);
                }
                Event::ControllerLayoutApplied { app_id, source } => {
                    self.overlay.set_controller_applied(*app_id, source.clone());
                }
                Event::MoveFinished { result: Ok(_), .. } | Event::RestoreFinished { result: Ok(_), .. } => {
                    if let Ok(l) = local::load(Some(self.local.steam_root.clone())) {
                        self.local = l;
                    }
                }
                Event::Error(msg) => {
                    match self.launch_popup.as_mut().filter(|p| matches!(p.stage, crate::launch_popup::Stage::Launching)) {
                        Some(p) => p.stage = crate::launch_popup::Stage::Failed(msg.clone()),
                        None => self.toast(now, i18n::t!("Something went wrong"), i18n::tr_dyn(msg)),
                    }
                }
                Event::KeyActivated(r) => {
                    if let Some(a) = &mut self.activate {
                        a.busy = false;
                        a.result = Some(r.clone());
                    }
                    if r.is_ok() {
                        self.client.send(Command::SetPersonaState(self.state.me().map(|m| m.state).unwrap_or(model::PersonaState::Online)));
                    }
                }
                _ => {}
            }
            let env = Env {
                state: &self.state,
                local: &self.local,
                theme: &self.prefs.theme,
                zoom: self.prefs.zoom,
                client: &self.client,
                actions: &mut *actions,
            };
            self.registry.broadcast(&e, env);
            self.bigpicture.on_event(&e, &self.client, self.state.steam_id, actions);
        }
    }

    pub fn navigate(&mut self, page: String, arg: Option<String>) {
        if page != self.prefs.page || arg != self.current_arg {
            self.back.push((self.prefs.page.clone(), self.current_arg.clone()));
            self.forward.clear();
        }
        self.go(page, arg);
    }

    fn go(&mut self, page: String, arg: Option<String>) {
        self.registry.navigate(&page, arg.as_deref());
        self.prefs.page = page;
        self.current_arg = arg;
    }

    pub(crate) fn restore_desktop_page(&mut self) {
        self.registry.navigate(&self.prefs.page, self.current_arg.as_deref());
    }

    pub fn go_back(&mut self) {
        if let Some((p, a)) = self.back.pop() {
            self.forward.push((self.prefs.page.clone(), self.current_arg.clone()));
            self.go(p, a);
        }
    }

    pub fn go_forward(&mut self) {
        if let Some((p, a)) = self.forward.pop() {
            self.back.push((self.prefs.page.clone(), self.current_arg.clone()));
            self.go(p, a);
        }
    }

    pub fn apply_actions(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        let now = ctx.input(|i| i.time);
        for a in actions {
            match a {
                Action::Navigate { page, arg } => {
                    if !self.bigpicture.navigate(ctx, &page, arg.as_deref()) {
                        self.navigate(page, arg);
                    }
                }
                Action::Toast(text) => self.toast(now, text, ""),
                Action::SetTheme(t) => self.prefs.theme = t,
                Action::SetZoom(z) => self.prefs.zoom = z.clamp(0.7, 2.0),
                Action::SetEnabled { id, enabled } => {
                    self.registry.set_enabled(&id, enabled);
                    self.prefs.disabled.retain(|d| d != &id);
                    if !enabled {
                        self.prefs.disabled.push(id);
                    }
                }
                Action::OpenChat(id) => self.navigate("community".into(), Some(id.to_string())),
                Action::OpenGroupChat { group_id, chat_id } => {
                    let arg = match chat_id {
                        Some(cid) => format!("group:{group_id}:{cid}"),
                        None => format!("group:{group_id}"),
                    };
                    if self.friends_window.open {
                        self.friends_window.chat = Some(crate::builtin::group_chat::ChatTarget::Group(group_id));
                        self.friends_window.group_ui.chat_id = chat_id;
                    }
                    self.navigate("community".into(), Some(arg));
                }
                Action::ReloadLocal => {
                    if let Ok(l) = local::load(Some(self.local.steam_root.clone())) {
                        self.local = l;
                    }
                }
                Action::MarkRead(id) => {
                    self.state.unread.remove(&id);
                }
                Action::MarkGroupRead { group_id, chat_id } => {
                    if let Some(ts) = self.state.mark_group_read(group_id, chat_id) {
                        self.client.send(Command::AckGroupMessage { group_id, chat_id, timestamp: ts });
                    }
                }
                Action::JoinGroupInvite { group_id, chat_id, code } => {
                    self.client.send(Command::JoinChatGroup { group_id, chat_id, invite_code: code });
                }
                Action::ToggleOverlay => self.overlay.toggle(),
                Action::JoinGame { app_id, connect } => {
                    let now = ctx.input(|i| i.time);
                    if let Err(msg) = client::safe_connect_args(&connect) {
                        self.toast(now, i18n::t!("Something went wrong"), msg);
                    } else if self.overlay.game.is_some_and(|(id, _)| id == app_id) {
                        // A second copy would just fight the first one.
                        self.toast(now, i18n::tr("The game is already running"), i18n::tr("Join from the game's own menu."));
                    } else if let Some(g) = self.local.games.iter().find(|g| g.app_id == app_id) {
                        self.launch_popup = Some(crate::launch_popup::LaunchPopup::new(app_id, g.name.clone(), now));
                        self.client.send(Command::Launch { app_id, install_dir: g.install_dir.clone(), args: client::LaunchArgs::connect(connect) });
                    } else {
                        self.navigate("library".into(), Some(format!("install:{app_id}")));
                    }
                }
                Action::SetLanguage(code) => {
                    if let Some(lang) = i18n::Lang::from_code(&code) {
                        i18n::set_language(lang);
                        self.prefs.language = Some(code);
                    }
                }
                Action::ActivateTheme(id) => {
                    let err_msg = if let Ok(mut st) = self.themes.lock() {
                        match st.manager.activate(id.as_deref()) {
                            Ok(()) => {
                                self.prefs.active_theme_id = id.clone();
                                st.last_error = None;
                                st.revision = st.revision.wrapping_add(1);
                                st.dirty = true;
                                None
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                st.last_error = Some(msg.clone());
                                st.dirty = true;
                                Some(msg)
                            }
                        }
                    } else {
                        None
                    };
                    if let Some(msg) = err_msg {
                        self.toast(now, "Theme error", msg);
                    }
                }
                Action::ReloadThemes => {
                    if let Ok(mut st) = self.themes.lock() {
                        st.manager.scan();
                        st.last_error = None;
                        st.revision = st.revision.wrapping_add(1);
                        st.dirty = true;
                    }
                }
                Action::SetThemeOption { theme_id, key, value } => {
                    let err_msg = if let Ok(mut st) = self.themes.lock() {
                        match st.manager.set_option_str(&theme_id, &key, &value) {
                            Ok(()) => {
                                st.last_error = None;
                                st.revision = st.revision.wrapping_add(1);
                                st.dirty = true;
                                None
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                st.last_error = Some(msg.clone());
                                Some(msg)
                            }
                        }
                    } else {
                        None
                    };
                    if let Some(msg) = err_msg {
                        self.toast(now, "Theme option error", msg);
                    }
                }
            }
        }
        let pages = self.registry.pages();
        if !pages.iter().any(|p| p.id == self.prefs.page) {
            self.prefs.page = "library".into();
        }
    }
}

impl SteamApp {
    fn sync_client_preferences(&mut self, force: bool) {
        let revision = client::preferences_revision();
        if !force && self.preferences_revision == Some(revision) {
            return;
        }
        self.prefs.proton_override = client::proton_override();
        self.prefs.proton_per_app = client::app_protons();
        self.prefs.gameplay_downloads = client::gameplay_downloads();
        self.prefs.controller = client::controller_settings();
        self.prefs.controller_bindings = client::app_bindings();
        self.preferences_revision = Some(revision);
    }

    fn sync_theme_state(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let (active_theme, fonts_key, custom_fonts, bg_path, bg_wrap, logo_path, new_snapshot, new_lib_nodes, rev) = {
            let mut st = self.themes.lock().unwrap();
            let native = st.manager.uses_native_appearance();
            if !st.dirty && st.revision == self.synced_revision && !native && !st.hot_reload
                && !self.prefs.hot_reload_themes && self.applied_theme.as_ref() == Some(&self.prefs.theme)
                && st.manager.resolved.as_ref().is_none_or(|theme| {
                    let want_wrap = if theme.background_fit == themes::BackgroundFit::Tile {
                        egui::TextureWrapMode::Repeat
                    } else {
                        egui::TextureWrapMode::ClampToEdge
                    };
                    (theme.background_path.is_none()
                        || self.bg_tex.as_ref().is_some_and(|(_, w, _)| *w == want_wrap))
                        && (theme.logo_path.is_none() || self.logo_tex.is_some())
                })
            {
                return;
            }
            self.prefs.hot_reload_themes = st.hot_reload;
            self.prefs.active_theme_id = st.manager.active_id.clone();

            // The watcher reads desktop settings off the UI thread. Keep it
            // alive only for opted-in themes, independently of file hot reload.
            if native {
                let watcher = self.native_appearance.get_or_insert_with(themes::native::NativeAppearanceWatcher::new);
                if let Some(appearance) = watcher.latest() {
                    if st.manager.set_native_appearance(appearance) {
                        st.revision = st.revision.wrapping_add(1);
                        st.dirty = true;
                    }
                }
                if let Some(artwork) = watcher.latest_artwork() {
                    if !self.applied_native_artwork.as_ref().is_some_and(|old| Arc::ptr_eq(old, &artwork))
                        && ui::native_style::install(ctx, &artwork)
                    {
                        self.applied_native_artwork = Some(artwork);
                    }
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            } else {
                self.native_appearance = None;
                self.applied_native_artwork = None;
                ui::native_style::clear(ctx);
            }

            if st.hot_reload && st.manager.active_id.is_some() && (now - self.last_hot_reload_check).abs() >= 0.5 {
                self.last_hot_reload_check = now;
                if st.manager.check_hot_reload() {
                    st.revision = st.revision.wrapping_add(1);
                    st.dirty = true;
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }

            let changed = st.dirty || st.revision != self.synced_revision;
            st.dirty = false;
            let rev = st.revision;

            if let Some(res) = st.manager.resolved.as_ref() {
                let theme = res.theme.clone();
                let wp = theme.weight_preset;
                let fonts_key = Some(match res.custom_fonts.as_ref() {
                    Some(_) => format!("{wp:?}:{}:{rev}", res.meta.id),
                    None => format!("{wp:?}"),
                });
                let custom_fonts = if self.applied_fonts_key != fonts_key { res.custom_fonts.clone() } else { None };
                let snap = changed.then(|| Some(snapshot_resolved(res)));
                let lib_nodes = changed.then(|| res.layout.library.clone());
                let bg_wrap = if res.background_fit == themes::BackgroundFit::Tile {
                    egui::TextureWrapMode::Repeat
                } else {
                    egui::TextureWrapMode::ClampToEdge
                };
                (theme, fonts_key, custom_fonts, res.background_path.clone(), bg_wrap, res.logo_path.clone(), snap, lib_nodes, rev)
            } else {
                let theme = Theme::default();
                let fonts_key = Some(format!("{:?}", theme.weight_preset));
                let snap = changed.then_some(None);
                let lib_nodes = changed.then(Vec::new);
                (theme, fonts_key, None, None, egui::TextureWrapMode::ClampToEdge, None, snap, lib_nodes, rev)
            }
        };

        if let Some(snap) = new_snapshot {
            self.resolved_snapshot = snap;
        }
        if let Some(nodes) = new_lib_nodes {
            *self.library_layout.lock().unwrap() = nodes;
        }
        self.synced_revision = rev;
        self.prefs.theme = active_theme;

        if self.applied_theme.as_ref() != Some(&self.prefs.theme) {
            self.prefs.theme.apply(ctx);
            self.applied_theme = Some(self.prefs.theme.clone());
        }
        if self.applied_fonts_key != fonts_key {
            ui::install_fonts_for_theme(ctx, lcd_subpixel_enabled(), self.prefs.theme.weight_preset, custom_fonts.as_ref());
            self.applied_fonts_key = fonts_key;
        }
        sync_texture(ctx, &mut self.bg_tex, bg_path.as_deref(), bg_wrap, "theme-bg");
        sync_texture(ctx, &mut self.logo_tex, logo_path.as_deref(), egui::TextureWrapMode::ClampToEdge, "theme-logo");
    }

    fn anti_cheat_notice(&mut self, ctx: &egui::Context) {
        use i18n::t;
        use ui::{icons, widgets as w};

        let Some(notice) = self.anti_cheat_notices.front() else { return };
        let app_id = notice.app_id;
        let name = self.state.app_name(app_id).unwrap_or_default();
        let theme = &self.prefs.theme;
        let mut play = false;
        let mut cancel = false;
        let modal = w::modal(ctx, egui::Id::new(("anti-cheat-launch", app_id))).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(w::text(icons::WARNING, 28.0, theme.danger));
            ui.label(w::text_bold(t!("Play anti-cheat games at your own risk"), 20.0, theme.text));
            ui.add_space(6.0);
            for text in [
                t!("{name} uses anti-cheat. Aurora will start it through the windowless Steam host.", name = name).to_string(),
                t!("The anti-cheat may see this as an unsupported setup. That can mean a kick, a game ban or a Steam (VAC) ban on your account, and bans are usually permanent.").to_string(),
                t!("Some games won't start, or can't go online, without the official client.").to_string(),
            ] {
                ui.add(egui::Label::new(w::text(text, 14.0, theme.text_dim)).wrap());
                ui.add_space(6.0);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let button = w::button(ui, theme, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md);
                w::autofocus(&button, true);
                cancel = button.clicked();
                play = w::button(ui, theme, Some(icons::PLAY), t!("Continue"), w::Kind::Danger, w::Size::Md).clicked();
            });
        });
        if !play && !cancel && !modal.should_close() {
            return;
        }
        let notice = self.anti_cheat_notices.pop_front().unwrap();
        if let Some(popup) = self.launch_popup.as_mut().filter(|popup| popup.app_id == app_id) {
            if play {
                popup.since = ctx.input(|i| i.time);
            } else {
                self.launch_popup = None;
            }
        }
        if play {
            self.prefs.anti_cheat_acknowledged.insert(app_id);
            client::acknowledge_anti_cheat(app_id);
            let AntiCheatNotice { app_id, install_dir, args, skip_cloud } = notice;
            self.client.send(if skip_cloud {
                Command::LaunchWithoutCloud { app_id, install_dir, args }
            } else {
                Command::Launch { app_id, install_dir, args }
            });
        }
    }

    fn show_window(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame, token: Option<String>) {
        self.tray.activate_window(frame, token);
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Hide the main window while a tray host can bring it back.
    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        ctx.forget_all_images();
        self.pending_memory_trim = 1;
        if self.tray.available() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden = true;
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    fn handle_links(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame, actions: &mut Vec<Action>) {
        use crate::links::Link;
        while let Ok(request) = self.links.try_recv() {
            self.show_window(ctx, frame, request.activation_token);
            if request.arg != "show" {
                self.pending_links.push(request.arg);
            }
        }
        if self.state.phase != Phase::LoggedIn {
            return;
        }
        for raw in std::mem::take(&mut self.pending_links) {
            let nav = |page: &str, arg: Option<String>| Action::Navigate { page: page.into(), arg };
            if let Some((deck_preview, windowed)) = crate::links::big_picture_mode(&raw) {
                self.bigpicture.enter_requested(ctx, deck_preview, windowed);
                continue;
            }
            match crate::links::parse(&raw) {
                Some(Link::Run(id)) => match self.local.games.iter().find(|g| g.app_id == id) {
                    Some(g) => {
                        let now = ctx.input(|i| i.time);
                        self.launch_popup = Some(crate::launch_popup::LaunchPopup::new(id, g.name.clone(), now));
                        self.client.send(Command::Launch { app_id: id, install_dir: g.install_dir.clone(), args: client::LaunchArgs::default() });
                    }
                    None => actions.push(nav("library", Some(format!("install:{id}")))),
                },
                Some(Link::Install(id)) => actions.push(nav("library", Some(format!("install:{id}")))),
                Some(Link::Uninstall(id)) => actions.push(nav("library", Some(format!("uninstall:{id}")))),
                Some(Link::Validate(id)) => actions.push(nav("library", Some(format!("verify:{id}")))),
                Some(Link::Store(id)) => actions.push(nav("store", Some(format!("app:{id}")))),
                Some(Link::Page(p)) => actions.push(nav(p, None)),
                Some(Link::Show) | None => {}
            }
        }
    }

    fn handle_tray(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame, actions: &mut Vec<Action>) {
        use crate::tray::TrayCmd;
        for event in self.tray.poll(ctx) {
            match event.command {
                TrayCmd::Show => self.show_window(ctx, frame, event.activation_token),
                TrayCmd::Menu(..) => {}
                TrayCmd::Page(page) => {
                    self.show_window(ctx, frame, event.activation_token);
                    actions.push(Action::Navigate { page: page.into(), arg: None });
                }
                TrayCmd::Friends => self.friends_window.open = true,
                TrayCmd::Status(s) => self.client.send(Command::SetPersonaState(s)),
                TrayCmd::Exit => {
                    if frame.is_window_attached() || self.downloading() {
                        self.show_window(ctx, frame, event.activation_token);
                    }
                    self.request_exit(ctx);
                }
            }
        }
    }
}

impl SteamApp {
    /// A download is running, paused or queued.
    pub(crate) fn shared_flags(&self) -> crate::builtin::settings::SharedFlags {
        self.flags.clone()
    }

    pub fn downloading(&self) -> bool {
        !self.state.download_queue.is_empty() || self.state.downloads.values().any(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed))
    }

    /// Exit the app, asking first if a download is in progress (like Steam).
    pub fn request_exit(&mut self, ctx: &egui::Context) {
        if self.downloading() {
            self.confirm_exit = true;
        } else {
            self.exiting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn exit_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_exit {
            return;
        }
        let t = self.prefs.theme.clone();
        let name = self
            .state
            .downloads
            .values()
            .find(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed))
            .map(|d| d.app_id)
            .or_else(|| self.state.download_queue.first().copied())
            .and_then(|id| self.state.app_name(id).map(String::from))
            .unwrap_or_default();
        let (mut exit, mut tray, mut cancel) = (false, false, false);
        ui::widgets::modal(ctx, egui::Id::unique("confirm-exit")).show(ctx, |ui| {
            ui.set_width(500.0);
            ui.label(ui::widgets::text_bold(i18n::tr("A download is in progress"), 19.0, t.text));
            ui.add_space(10.0);
            let body = i18n::trf("{game} is still downloading. If you exit, it continues the next time you open Aurora.", &[("game", name.clone())]);
            ui.add(egui::Label::new(ui::widgets::text(body, 14.0, t.text_dim)).wrap());
            ui.add_space(26.0);
            // No tray icon: the window minimizes instead.
            let keep = if self.tray.available() { i18n::tr("Keep downloading in the tray") } else { i18n::tr("Minimize and keep downloading") };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                exit = ui::widgets::button(ui, &t, Some(ui::icons::POWER), i18n::tr("Exit anyway"), ui::widgets::Kind::Danger, ui::widgets::Size::Md).clicked();
                tray = ui::widgets::button(ui, &t, None, keep, ui::widgets::Kind::Secondary, ui::widgets::Size::Md).clicked();
                cancel = ui::widgets::button(ui, &t, None, i18n::tr("Cancel"), ui::widgets::Kind::Ghost, ui::widgets::Size::Md).clicked();
            });
        });
        if exit {
            self.confirm_exit = false;
            self.exiting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if tray {
            self.confirm_exit = false;
            self.hide_to_tray(&ctx);
        } else if cancel {
            self.confirm_exit = false;
        }
    }
}

fn finish_memory_trim(ctx: &egui::Context, pending: &mut u8) -> bool {
    if crate::chrome::take_memory_trim_request(ctx) {
        *pending = 1;
    }
    if *pending == 0 {
        return false;
    }
    *pending -= 1;
    *pending == 0
}

impl eframe::App for SteamApp {
    /// Runs before every frame, and also while the window gets no frames
    /// (minimized or behind a fullscreen game on Wayland): the overlay drawn
    /// inside the game runs here so it stays quick then.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let mut actions = Vec::new();
        self.drain_events(ctx, &mut actions);
        self.handle_tray(ctx, frame, &mut actions);
        self.handle_links(ctx, frame, &mut actions);
        if !self.anti_cheat_notices.is_empty() && (self.hidden || ctx.input(|i| i.viewport().minimized == Some(true))) {
            self.show_window(ctx, frame, None);
        }
        if frame.is_window_attached() || self.tray.available() {
            self.tray_restore_retry_at = None;
        } else {
            let now = std::time::Instant::now();
            if self.tray_restore_retry_at.is_none_or(|retry_at| now >= retry_at) {
                self.show_window(ctx, frame, None);
                self.tray_restore_retry_at = Some(now + std::time::Duration::from_secs(1));
            }
            // If EGL failed to remap, retry without depending on the lost tray host.
            if let Some(retry_at) = self.tray_restore_retry_at {
                ctx.request_repaint_after_for(retry_at.saturating_duration_since(now), egui::ViewportId::ROOT);
            }
        }
        self.apply_actions(ctx, actions);
        let visible = !self.hidden && frame.is_window_attached();
        if crate::bigpicture::poll(self, ctx, visible) {
            self.show_window(ctx, frame, None);
            // Winit's Focus command has no implementation on Wayland.
            if self.tray.wayland() && !ctx.input(|i| i.raw.focused) {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            }
        }
        crate::overlay::logic(self, ctx);
    }

    fn post_frame(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.tray.wayland() && frame.is_window_attached() {
            // The EGL backend may have fallen back to minimizing.
            self.hidden = false;
        }
        if finish_memory_trim(ctx, &mut self.pending_memory_trim) {
            crate::release_memory();
        }
    }

    /// Scroll faster than egui's default (Settings → Accessibility → Scroll
    /// speed). Ctrl/Cmd + wheel zooms, so those events stay as they are.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.bigpicture.feed_input(raw_input);
        let speed = self.prefs.scroll_speed;
        if (speed - 1.0).abs() < f32::EPSILON {
            return;
        }
        for event in &mut raw_input.events {
            if let egui::Event::MouseWheel { delta, modifiers, .. } = event
                && !modifiers.command
            {
                *delta *= speed;
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Transparent so the overlay window can see through; the main window
        // paints its own background.
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.sync_theme_state(&ctx);
        if self.applied_zoom != self.prefs.zoom {
            ctx.set_zoom_factor(self.prefs.zoom);
            self.applied_zoom = self.prefs.zoom;
        }
        // Update checks (not in demo/preview runs).
        if !self.demo {
            crate::updater::tick(self, &ctx);
        }

        let mut actions = Vec::new();
        {
            let f = *self.flags.lock().unwrap();
            self.prefs.close_to_tray = f.close_to_tray;
            self.prefs.ask_account = f.ask_account;
            if self.prefs.auto_updates != f.auto_updates {
                self.client.send(Command::SetAutoDownloads(f.auto_updates));
            }
            self.prefs.auto_updates = f.auto_updates;
            self.prefs.toast_messages = f.toast_messages;
            self.prefs.toast_notifications = f.toast_notifications;
            self.prefs.scroll_speed = f.scroll_speed;
            self.prefs.drm_host_ack = f.drm_host_ack;
            self.prefs.overlay_enabled = f.overlay_enabled;
            self.prefs.fps_counter = f.fps_counter;
            self.prefs.fps_high_contrast = f.fps_high_contrast;
            self.prefs.show_whats_new = f.show_whats_new;
            #[cfg(not(windows))]
            {
                self.prefs.precompile_shaders = client::shaders::precompile();
            }
            if self.prefs.cloud != f.cloud {
                self.prefs.cloud = f.cloud;
                self.client.send(Command::CloudSettings { steam_root: Some(self.local.steam_root.clone()), disabled: if f.cloud { vec![] } else { self.state.apps.as_ref().map(|a| a.keys().copied().collect()).unwrap_or_default() } });
            }
        }
        self.sync_client_preferences(false);
        self.drain_events(&ctx, &mut actions);
        self.taskbar.update(&self.state.downloads);
        self.handle_tray(&ctx, frame, &mut actions);
        self.handle_links(&ctx, frame, &mut actions);

        // Closing the window hides it in the tray (like Steam) unless "Exit" was chosen.
        // With no tray icon (e.g. GNOME without the AppIndicator extension), closing quits.
        let signed_in_now = self.state.phase == Phase::LoggedIn;
        if self.tray.available() && ctx.input(|i| i.viewport().close_requested()) && self.prefs.close_to_tray && signed_in_now && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_to_tray(&ctx);
        } else if ctx.input(|i| i.viewport().close_requested()) && !self.exiting && self.downloading() {
            // Closing for real while something downloads: ask first.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_exit = true;
        }
        self.exit_dialog(&ctx);
        let visible = !self.hidden && frame.is_window_attached();
        if !visible {
            // Keep the friends window, overlay and notifications alive while hidden.
            if !self.tray.wayland() {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            }
            crate::friends_window::show(self, &ctx, &mut actions);
            crate::overlay::show(self, &ctx, &mut actions);
            crate::launch_popup::show(self, &ctx);
            crate::tray::show(self, &ctx);
            let env = Env {
                state: &self.state,
                local: &self.local,
                theme: &self.prefs.theme,
                zoom: self.prefs.zoom,
                client: &self.client,
                actions: &mut actions,
            };
            self.registry.tick(env);
            self.apply_actions(&ctx, actions);
            return;
        }

        let env = Env {
            state: &self.state, local: &self.local, theme: &self.prefs.theme,
            zoom: self.prefs.zoom, client: &self.client, actions: &mut actions,
        };
        let launch_dialog = signed_in_now && self.registry.with_ext::<crate::builtin::library::Library, _>(
            "library", env, |lib, _| lib.launch_dialog_pending(),
        ).unwrap_or(false);
        let launch_dialog = launch_dialog || !self.anti_cheat_notices.is_empty();
        self.bigpicture.launch_dialog = launch_dialog;
        let big_picture = signed_in_now && self.bigpicture.active;
        let fullscreen = big_picture && !self.bigpicture.is_windowed();

        // Mouse back/forward buttons.
        let (b, f) = ctx.input(|i| (i.pointer.button_pressed(egui::PointerButton::Extra1), i.pointer.button_pressed(egui::PointerButton::Extra2)));
        if !big_picture {
            if b {
                self.go_back();
            }
            if f {
                self.go_forward();
            }
        }

        let signed_in = self.state.phase == Phase::LoggedIn;
        let mode = if signed_in {
            WindowMode::Main
        } else if crate::login::wants_splash(self) {
            WindowMode::Splash
        } else {
            WindowMode::SignIn
        };
        if fullscreen {
            if self.window_mode.take().is_some() {
                // A fixed-size login window leaves size constraints behind on Wayland.
                ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(egui::vec2(16384.0, 16384.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(2.0, 1.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(true));
            }
            self.pending_size = None;
            self.center_window = None;
        } else if self.window_mode != Some(mode) {
            self.window_mode = Some(mode);
            let (size, min, resizable) = match mode {
                WindowMode::Main => ([1440.0, 900.0], [1060.0, 680.0], true),
                // Small, compact sign-in and splash windows, like Steam's.
                WindowMode::SignIn => ([560.0, 400.0], [560.0, 400.0], false),
                WindowMode::Splash => ([380.0, 250.0], [380.0, 250.0], false),
            };
            // Locking the size (Resizable(false)) pins it to whatever it is
            // at that moment, so a fixed-size window is locked only once it
            // has its size (see `pending_size` below).
            if resizable {
                // winit (Wayland) keeps the size it pinned as the max size when
                // the window becomes resizable again: a max below the main
                // window's min size is a protocol error that closes the app.
                ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(egui::vec2(16384.0, 16384.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(true));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min.into()));
            // Main: restore the last size/position/maximized. Sign-in and
            // splash: always small and centered on the main display.
            if mode == WindowMode::Main && self.prefs.win_maximized {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.prefs.win_size.unwrap_or(size).into()));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                self.center_window = None;
                self.pending_size = None;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
                let size = if mode == WindowMode::Main { self.prefs.win_size.unwrap_or(size) } else { size };
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size.into()));
                self.pending_size = Some((size, resizable, 0));
                // Restore a saved main-window position, else center once the
                // monitor size is known (it can be unknown on this frame).
                match (mode, self.prefs.win_pos) {
                    (WindowMode::Main, Some(pos)) => ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos.into())),
                    _ => self.center_window = Some(size),
                }
            }
        }
        // The compositor ignores a new size while the window is still
        // maximized (it starts that way when the last session ended
        // maximized): ask again until the window has it, then lock it.
        if let Some((size, resizable, tries)) = self.pending_size {
            let (now, maximized) = ctx.input(|i| (i.viewport().inner_rect.map(|r| r.size()), i.viewport().maximized.unwrap_or(false)));
            let done = now.is_some_and(|s| (s - egui::Vec2::from(size)).abs().max_elem() < 2.0);
            if done || tries >= 120 {
                if !resizable {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(false));
                }
                self.pending_size = None;
            } else {
                if maximized {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
                } else if tries % 10 == 9 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size.into()));
                }
                self.pending_size = Some((size, resizable, tries + 1));
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        // Center when a size is pending and the monitor size has arrived.
        if let (Some(size), Some(monitor)) = (self.center_window, ctx.input(|i| i.viewport().monitor_size)) {
            let pos = ((monitor - egui::Vec2::from(size)) / 2.0).max(egui::Vec2::ZERO);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos.to_pos2()));
            self.center_window = None;
        }
        // Remember the main window's geometry for next time.
        if mode == WindowMode::Main && !fullscreen && self.pending_size.is_none() {
            ctx.input(|i| {
                let vp = i.viewport();
                // Right after sign-in the window is still sign-in sized for a
                // few frames: don't take that for the main window's size.
                if vp.fullscreen == Some(true) || vp.inner_rect.is_none_or(|r| r.width() < 1000.0 || r.height() < 600.0) {
                    return;
                }
                self.prefs.win_maximized = vp.maximized.unwrap_or(false);
                if !self.prefs.win_maximized && !vp.minimized.unwrap_or(false) {
                    if let Some(r) = vp.outer_rect {
                        self.prefs.win_pos = Some([r.min.x, r.min.y]);
                    }
                    if let Some(r) = vp.inner_rect {
                        self.prefs.win_size = Some([r.width(), r.height()]);
                    }
                }
            });
        }
        if big_picture {
            crate::bigpicture::show(self, ui, &mut actions);
            crate::friends_window::show(self, &ctx, &mut actions);
            crate::overlay::show(self, &ctx, &mut actions);
            if !launch_dialog {
                crate::launch_popup::show(self, &ctx);
            }
            crate::tray::show(self, &ctx);
        } else if signed_in {
            let snapshot = self.resolved_snapshot.take();
            let bg_tex = self.bg_tex.as_ref().map(|(_, _, h)| h.clone());
            let logo_tex = self.logo_tex.as_ref().map(|(_, _, h)| h.clone());
            let now = ctx.input(|i| i.time);
            let (has_bg_script, has_chrome_script) = {
                let st = self.themes.lock().unwrap();
                match st.manager.resolved.as_ref().and_then(|r| r.script.as_ref()) {
                    Some(s) => (s.has_background(), s.has_chrome()),
                    None => (false, false),
                }
            };
            let bg_script = has_bg_script.then(|| (self.themes.clone(), self.prefs.theme.clone()));
            let mut bg_hook = move |painter: &egui::Painter, rect: egui::Rect| {
                let cmds = {
                    let Some((themes, theme)) = &bg_script else { return };
                    let mut st = themes.lock().unwrap();
                    if let Some(script) = st.manager.resolved.as_mut().and_then(|r| r.script.as_mut()) {
                        script.paint_background(rect.width(), rect.height(), now, theme)
                    } else {
                        Vec::new()
                    }
                };
                themes::paint_draw_cmds(painter, rect, &cmds);
            };
            let chrome_script = has_chrome_script.then(|| (self.themes.clone(), self.prefs.theme.clone()));
            let mut chrome_hook = move |painter: &egui::Painter, rect: egui::Rect| {
                let cmds = {
                    let Some((themes, theme)) = &chrome_script else { return };
                    let mut st = themes.lock().unwrap();
                    if let Some(script) = st.manager.resolved.as_mut().and_then(|r| r.script.as_mut()) {
                        script.paint_chrome(rect.width(), rect.height(), now, theme)
                    } else {
                        Vec::new()
                    }
                };
                themes::paint_draw_cmds(painter, rect, &cmds);
            };
            crate::shell::show_with_theme(
                self,
                ui,
                &mut actions,
                snapshot.as_ref(),
                bg_tex.as_ref(),
                logo_tex.as_ref(),
                has_bg_script.then_some(&mut bg_hook as &mut dyn FnMut(&egui::Painter, egui::Rect)),
                has_chrome_script.then_some(&mut chrome_hook as &mut dyn FnMut(&egui::Painter, egui::Rect)),
            );
            self.resolved_snapshot = snapshot;
            crate::shell::activate_dialog(self, &ctx);
            crate::shell::friend_dialogs(self, &ctx);
            crate::friends_window::show(self, &ctx, &mut actions);
            crate::overlay::show(self, &ctx, &mut actions);
            if !launch_dialog {
                crate::launch_popup::show(self, &ctx);
            }
            crate::tray::show(self, &ctx);
        } else {
            crate::login::show(self, ui);
            crate::tray::show(self, &ctx);
        }

        let env = Env {
            state: &self.state,
            local: &self.local,
            theme: &self.prefs.theme,
            zoom: self.prefs.zoom,
            client: &self.client,
            actions: &mut actions,
        };
        if signed_in {
            self.registry.with_ext::<crate::builtin::library::Library, _>("library", env, |lib, cx| lib.background_dialogs(ui, cx));
            self.anti_cheat_notice(&ctx);
        }
        let env = Env {
            state: &self.state, local: &self.local, theme: &self.prefs.theme,
            zoom: self.prefs.zoom, client: &self.client, actions: &mut actions,
        };
        self.registry.tick(env);
        self.apply_actions(&ctx, actions);
        crate::shell::toasts(self, &ctx);
        if !big_picture || self.bigpicture.is_windowed() {
            if signed_in {
                crate::chrome::resize_handles(&ctx);
            }
            crate::chrome::border(&ctx, &self.prefs.theme);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.sync_client_preferences(true);
        self.prefs.ext_data = self.registry.save_all().into_iter().collect();
        eframe::set_value(storage, "prefs", &self.prefs);
        // eframe restores its own "window" entry even with persist_window
        // off; an old maximized one reopened sign-in full screen. Blank it.
        storage.set_string("window", String::new());
    }

    /// Dropping a tokio runtime waits for every blocking task to return: a
    /// running game's wait, a verify, the headless Steam host. Closing then
    /// left an invisible process behind, and the next launch handed itself to
    /// that stuck copy. Give the tasks a moment and stop waiting for them.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(mut watcher) = self.native_appearance.take() {
            watcher.shutdown();
        }
        let idle = tokio::runtime::Builder::new_current_thread().build().expect("tokio runtime");
        std::mem::replace(&mut self._rt, idle).shutdown_timeout(std::time::Duration::from_secs(2));
    }
}

/// Window size and style for each stage, like Steam's separate windows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WindowMode {
    /// Small "signing in" splash.
    Splash,
    SignIn,
    Main,
}

fn snapshot_resolved(res: &themes::ResolvedTheme) -> themes::ResolvedTheme {
    themes::ResolvedTheme {
        meta: res.meta.clone(),
        dir: res.dir.clone(),
        theme: res.theme.clone(),
        layout: res.layout.clone(),
        custom_fonts: None,
        background_path: res.background_path.clone(),
        background_fit: res.background_fit,
        background_opacity: res.background_opacity,
        background_tint: res.background_tint,
        logo_path: res.logo_path.clone(),
        script: None,
        script_error: res.script_error.clone(),
    }
}

fn sync_texture(
    ctx: &egui::Context,
    slot: &mut Option<(PathBuf, egui::TextureWrapMode, egui::TextureHandle)>,
    path: Option<&std::path::Path>,
    wrap_mode: egui::TextureWrapMode,
    name: &str,
) {
    const MAX_THEME_IMG_DIM: u32 = 8192;
    const MAX_THEME_IMG_ALLOC: u64 = 64 << 20;
    match path {
        None => {
            *slot = None;
        }
        Some(p) => {
            if slot.as_ref().is_some_and(|(cached, w, _)| cached.as_path() == p && *w == wrap_mode) {
                return;
            }
            if std::fs::symlink_metadata(p).is_err_and(|_| true)
                || std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink() || !m.is_file())
            {
                tracing::warn!("theme image {}: refused non-regular file or symlink", p.display());
                *slot = None;
                return;
            }
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_THEME_IMG_DIM);
            limits.max_image_height = Some(MAX_THEME_IMG_DIM);
            limits.max_alloc = Some(MAX_THEME_IMG_ALLOC);
            let decoded = image::ImageReader::open(p)
                .map_err(|e| e.to_string())
                .and_then(|r| r.with_guessed_format().map_err(|e| e.to_string()))
                .and_then(|mut r| {
                    r.limits(limits);
                    r.decode().map_err(|e| e.to_string())
                });
            match decoded {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let color_img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let opts = egui::TextureOptions {
                        wrap_mode,
                        ..egui::TextureOptions::LINEAR
                    };
                    let handle = ctx.load_texture(name, color_img, opts);
                    *slot = Some((p.to_path_buf(), wrap_mode, handle));
                }
                Err(e) => {
                    tracing::warn!("theme image {}: {e}", p.display());
                    *slot = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Prefs;

    #[test]
    fn prefs_controller_defaults_and_roundtrip() {
        let legacy: Prefs = serde_json::from_str("{}").unwrap();
        assert!(legacy.controller.families.xbox);
        assert!(legacy.controller.families.playstation);
        assert!(legacy.controller.families.switch_pro);
        assert!(legacy.controller.families.generic);
        assert_eq!(legacy.controller.calibration, client::PadCalibration::default());
        assert!(legacy.controller_bindings.is_empty());

        let mut prefs = Prefs::default();
        prefs.controller.families.xbox = false;
        prefs.controller.families.switch_pro = false;
        prefs.controller.calibration.left_deadzone = 0.24;
        prefs.controller.calibration.nintendo_layout = true;
        prefs.controller.calibration.rumble_enabled = false;
        prefs.controller_bindings.insert(620, client::BindingChoice::Workshop(987654));

        let json = serde_json::to_string(&prefs).unwrap();
        let restored: Prefs = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.controller, prefs.controller);
        assert_eq!(restored.controller_bindings, prefs.controller_bindings);
    }

    #[test]
    fn prefs_show_whats_new_defaults_true_and_roundtrips() {
        let legacy: Prefs = serde_json::from_str("{}").unwrap();
        assert!(legacy.show_whats_new);
        assert!(Prefs::default().show_whats_new);

        let mut prefs = Prefs::default();
        prefs.show_whats_new = false;
        let json = serde_json::to_string(&prefs).unwrap();
        let restored: Prefs = serde_json::from_str(&json).unwrap();
        assert!(!restored.show_whats_new);
    }
}

#[cfg(test)]
mod memory_trim_tests {
    use super::finish_memory_trim;

    #[test]
    fn trim_is_due_only_after_a_completed_frame() {
        let ctx = egui::Context::default();
        let mut pending_memory_trim = 0;
        assert!(!finish_memory_trim(&ctx, &mut pending_memory_trim));

        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::chrome::minimize(ui.ctx());
            assert_eq!(pending_memory_trim, 0);
        });
        output.drop_without_applying_deltas();
        assert!(finish_memory_trim(&ctx, &mut pending_memory_trim));
        assert!(!finish_memory_trim(&ctx, &mut pending_memory_trim));

        // Hide-to-tray queues the countdown directly on the app.
        pending_memory_trim = 1;
        assert!(finish_memory_trim(&ctx, &mut pending_memory_trim));
        assert!(!finish_memory_trim(&ctx, &mut pending_memory_trim));
    }
}
