//! Big Picture: a fullscreen mode for the TV, driven by a controller or the
//! keyboard. It shows the same library, store, downloads and friends as the
//! normal window, and uses the Library's and Store's own logic for playing,
//! installing and loading, so the two never disagree.
//!
//! Controls: D-pad/stick or arrows move, A/Enter selects, B/Esc goes back,
//! LB/RB (Q/E, PageUp/PageDown, Tab) switch tabs, Start (M/F1) opens the
//! menu, Guide opens the menu and, pressed again there, leaves Big Picture.

mod focus;
mod input;
mod manage;
mod menu;
mod osk;
mod paint;
mod profile;
pub(crate) mod quick_access;
mod screens;
mod settings;
pub(crate) mod system;

use crate::app::SteamApp;
use egui::{Align2, Color32, Id, Rect, Ui, pos2, vec2};
use extensions::Action;
use focus::{Dir, Focus, Place, Scroller, since};
use i18n::t;
use input::{Device, PadEvent, Press};
use model::{AppId, ChatRoomId, GroupChatId, SteamId};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use ui::{Theme, icons, widgets as w};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
    #[default]
    Home,
    Library,
    Store,
    Downloads,
    Friends,
}

const TABS: [Tab; 5] = [Tab::Home, Tab::Library, Tab::Store, Tab::Downloads, Tab::Friends];

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Home => t!("Home"),
            Tab::Library => t!("Library"),
            Tab::Store => t!("Store"),
            Tab::Downloads => t!("Downloads"),
            Tab::Friends => t!("Friends"),
        }
    }
    fn index(self) -> usize {
        TABS.iter().position(|x| *x == self).unwrap_or(0)
    }
}

/// A screen: a tab, or something opened on top of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ChatTarget {
    Friend(SteamId),
    Group(GroupChatId, ChatRoomId),
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Page {
    Tab(Tab),
    Game(AppId),
    GameManage(AppId),
    Controller(AppId),
    Notifications,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    Compatibility(AppId),
    StoreApp(AppId),
    Review(AppId, String),
    News(AppId, usize),
    Search,
    /// Screenshots of a store page, at one of them.
    Shots(AppId, usize),
    Settings,
    /// A friend: message, join, invite, profile.
    Friend(SteamId),
    /// Native Big Picture profile pages (your own or another user's).
    Profile(SteamId),
    ProfileGames(SteamId),
    ProfileBadges(SteamId),
    ProfileCards(SteamId, AppId, bool),
    ProfileReviews(SteamId),
    ProfileGroups(SteamId),
    ProfileEdit,
    /// A conversation with a friend.
    Group(GroupChatId),
    Chat(ChatTarget),
    Embedded(String, Option<String>),
    /// A store trailer, playing over the page.
    Video(AppId, usize),
    /// Before installing: size, where, shortcut.
    Install(AppId),
}

impl Page {
    fn key(&self) -> String {
        format!("{self:?}")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum LibrarySort {
    #[default]
    Title,
    Recent,
    Playtime,
    Size,
}

impl LibrarySort {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Title => Self::Recent,
            Self::Recent => Self::Playtime,
            Self::Playtime => Self::Size,
            Self::Size => Self::Title,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Title => t!("A–Z"),
            Self::Recent => t!("Recently played"),
            Self::Playtime => t!("Most played"),
            Self::Size => t!("Size on disk"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum LibraryFilter {
    Installed,
    Favorites,
    RunsHere,
    All,
    Collection(String),
}

pub(crate) struct LibraryEntry {
    pub id: AppId,
    pub name: String,
    pub installed: bool,
    pub runs: bool,
    pub favorite: bool,
    pub last_played: u64,
    pub playtime_minutes: u64,
    pub size_on_disk: u64,
}

#[derive(Default)]
pub(crate) struct LibraryCache {
    pub fingerprint: u64,
    pub all_games: Vec<LibraryEntry>,
    pub collections: Vec<(String, std::collections::BTreeSet<AppId>)>,
    pub n_installed: usize,
    pub n_favorites: usize,
    pub n_runs: usize,
}

pub(crate) struct ChatLayoutCache {
    pub target: ChatTarget,
    pub fingerprint: u64,
    pub wrap_tenths: i32,
    pub font_tenths: i32,
    pub laid: Vec<(f32, egui::Vec2, bool, std::sync::Arc<egui::Galley>, u64)>,
    pub total: f32,
}

#[derive(Default)]
pub struct BigPicture {
    pub active: bool,
    deck_preview: bool,
    /// `--bigpicture=window`: stay in a window (previews, testing).
    windowed: bool,
    fullscreen_sent: bool,
    fullscreen_asked: f64,
    entered_at: f64,
    tab: Tab,
    stack: Vec<Page>,
    focus: Focus,
    /// What was focused on each page, to come back to.
    memory: HashMap<String, Id>,
    /// Where focus goes first on the page being drawn (its main button).
    preferred: Option<Id>,
    scroll: HashMap<String, Scroller>,
    /// When the page changed, and from which side it slides in (-1, 0, 1).
    page_since: f64,
    slide: f32,
    menu: Option<menu::Menu>,
    quick_access: Option<quick_access::QuickAccess>,
    pub(crate) system: system::SystemMonitor,
    pub(crate) sys_status: system::SystemStatus,
    pad: Option<mpsc::Receiver<PadEvent>>,
    last_tuned: Option<(bool, u8, u8)>,
    /// Controller presses waiting for the next frame.
    queued: Vec<Press>,
    pads: usize,
    device: Device,
    /// The last hint pressed, for a little flash.
    flash: Option<(Press, f64)>,
    /// Library filter: installed only (None = decide from the library).
    installed_only: Option<bool>,
    /// Library filter: only games that run on this computer.
    this_os_only: bool,
    pub(crate) library_filter: Option<LibraryFilter>,
    pub(crate) library_sort: LibrarySort,
    pub(crate) library_query: String,
    pub(crate) library_search_open: bool,
    pub(crate) library_cache_rev: u64,
    pub(crate) library_cache: Option<LibraryCache>,
    /// The install screen's choices: (game, library folder, desktop shortcut).
    install: Option<(AppId, std::path::PathBuf, bool, &'static str)>,
    pub(crate) install_plan_cache: Option<(AppId, &'static str, crate::builtin::library::InstallPlan)>,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub(crate) compat_cache: Option<(AppId, Option<(bool, bool)>, Option<String>, Vec<(String, String)>)>,
    pub(crate) manage: manage::ManageState,
    pub(crate) profile: profile::ProfileState,
    /// A trailer was playing (stopped when its page closes).
    video_on: bool,
    // Search (on-screen keyboard).
    query: String,
    query_changed: f64,
    submitted: String,
    pub(crate) osk: osk::OskState,
    /// Chat: the message being typed, Enter pressed on a real keyboard, and
    /// conversations whose history was asked for.
    message: String,
    send_now: bool,
    pub(crate) chat_layout_cache: Option<ChatLayoutCache>,
    /// Background art: current & outgoing game with crossfade start time.
    bg_cur: Option<AppId>,
    bg_prev: Option<AppId>,
    bg_since: f64,
    /// The game whose art the background should show this frame.
    bg_want: Option<AppId>,
    game_backdrop: Option<String>,
    /// Achievement lists asked for this session.
    asked: HashSet<AppId>,
    /// Sliding highlight under the selected tab.
    tab_pill: Option<Rect>,
    /// How hidden the top bar is when scrolled down (0.0 = shown, 1.0 = tucked away).
    pub(crate) bar_hide: f32,
    /// When the screenshot viewer last changed picture, and which way.
    shot_since: (f64, f32),
    /// The on-screen key pressed last, and when (it pops).
    key_pop: Option<(Id, f64)>,
    /// Mouse wheel turned in the screenshot viewer, not yet a flip.
    shot_wheel: f32,
    /// Chat: whose conversation is open, whether it sticks to the newest
    /// message, a pending up/down scroll, whether the keyboard is up, and
    /// whether the history is at its (top, bottom).
    chat_for: Option<ChatTarget>,
    chat_follow: bool,
    chat_nudge: f32,
    chat_keys: bool,
    chat_edges: (bool, bool),
    settings: settings::SettingsState,
    embedded: Option<(String, Option<String>)>,
    wheel: f32,
    pub launch_dialog: bool,
    modal_keys: Vec<(egui::Key, egui::Modifiers)>,
    reviews_requested: Option<(AppId, i18n::Lang)>,
    review_cursor: Option<String>,
}

impl BigPicture {
    pub fn new(ctx: &egui::Context) -> Self {
        let mode = crate::links::big_picture_request(std::env::args().skip(1)).and_then(crate::links::big_picture_mode);
        let (deck_preview, windowed) = mode.unwrap_or_default();
        let mut bp = Self {
            active: mode.is_some(),
            deck_preview,
            windowed,
            pad: Some(input::spawn(ctx.clone())),
            slide: 0.0,
            ..Default::default()
        };
        // `--bp-page=store:<app>|game:<app>|install:<app>|shots:<app>|search|
        // chat:<steam id>|library` opens a page right away (previews and checks).
        if let Some(arg) = std::env::args().find_map(|a| a.strip_prefix("--bp-page=").map(String::from)) {
            let (kind, id) = arg.split_once(':').unwrap_or((arg.as_str(), ""));
            let wide: u64 = id.parse().unwrap_or(0);
            let id = id.parse().unwrap_or(0);
            match kind {
                "library" => bp.tab = Tab::Library,
                "settings" => {
                    bp.settings.open_at(id as usize);
                    bp.stack.push(Page::Settings);
                }
                "store" => bp.stack.push(Page::StoreApp(id)),
                "shots" => bp.stack.extend([Page::StoreApp(id), Page::Shots(id, 0)]),
                "search" => {
                    bp.tab = Tab::Store;
                    bp.stack.push(Page::Search);
                }
                "chat" => {
                    bp.tab = Tab::Friends;
                    bp.stack.extend([Page::Friend(wide), Page::Chat(ChatTarget::Friend(wide))]);
                }
                "group" => {
                    if let Some((group, room)) = id_pair(arg.as_str()) {
                        bp.tab = Tab::Friends;
                        bp.stack.extend([Page::Group(group), Page::Chat(ChatTarget::Group(group, room))]);
                    }
                }
                "game" => bp.stack.push(Page::Game(id)),
                "install" => bp.stack.push(Page::Install(id)),
                "profile" => bp.stack.push(Page::Profile(wide)),
                "profile-edit" => bp.stack.push(Page::ProfileEdit),
                _ => {}
            }
        }
        bp
    }

    pub fn enter(&mut self, ctx: &egui::Context) {
        if self.active {
            return;
        }
        self.active = true;
        self.fullscreen_asked = f64::NEG_INFINITY;
        self.fullscreen_sent = false;
        self.entered_at = ctx.input(|i| i.time);
        self.tab = Tab::Home;
        self.stack.clear();
        self.menu = None;
        self.quick_access = None;
        self.focus.current = None;
        self.chat_keys = false;
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    pub fn open_quick_access(&mut self, ctx: &egui::Context, section: quick_access::Section, now: f64) {
        self.menu = None;
        self.system.refresh_now(Some(ctx));
        if let Some(qa) = &mut self.quick_access {
            qa.section = section;
        } else {
            self.quick_access = Some(quick_access::QuickAccess::new(now, section));
        }
        ctx.request_repaint();
    }

    pub fn open_menu(&mut self, now: f64) {
        self.quick_access = None;
        self.menu = Some(menu::Menu::new(now));
    }

    pub fn is_deck_preview(&self) -> bool {
        self.deck_preview
    }

    pub fn enter_requested(&mut self, ctx: &egui::Context, deck_preview: bool, windowed: bool) {
        if self.windowed != windowed || self.deck_preview != deck_preview {
            self.leave(ctx);
            self.windowed = windowed;
            self.deck_preview = deck_preview;
        }
        self.enter(ctx);
    }

    /// Returns whether the desktop window should be shown and raised.
    fn handle_pad_press(&mut self, ctx: &egui::Context, press: Press, focused: bool, playing: bool) -> bool {
        if focused && self.active {
            self.queued.push(press);
        } else if press == Press::Guide && (focused || !playing) {
            if !self.active {
                self.enter_requested(ctx, false, false);
            }
            self.device = Device::Pad;
            return true;
        }
        false
    }

    pub fn on_event(&mut self, event: &client::Event, client: &client::Client, my_id: SteamId, actions: &mut Vec<Action>) {
        self.profile.on_event(event, client, my_id, actions);
    }

    pub fn navigate(&mut self, ctx: &egui::Context, page: &str, arg: Option<&str>) -> bool {
        if !self.active {
            return false;
        }
        let now = ctx.input(|i| i.time);
        let id_after = |prefix: &str| arg?.strip_prefix(prefix)?.parse().ok();
        match (page, arg) {
            ("store", _) if let Some(id) = id_after("app:") => self.open(Page::StoreApp(id), now),
            ("library", _) if let Some(id) = id_after("install:") => self.open(Page::Install(id), now),
            ("downloads", None) => self.switch_tab(Tab::Downloads, now, true),
            ("library", None) => self.switch_tab(Tab::Library, now, true),
            ("store", None) => self.switch_tab(Tab::Store, now, true),
            ("settings", None) => self.open(Page::Settings, now),
            ("profile", arg) => {
                let (dest, edit_sec) = profile::parse_route(0, arg);
                if let Some(sec) = edit_sec {
                    self.profile.edit.section = sec;
                }
                self.open(dest, now);
            }
            ("community" | "workshop" | "hub" | "inventory" | "workshop_item" | "guides" | "discussions" | "thread" | "achievements" | "user_files", _) => self.open(Page::Embedded(page.into(), arg.map(str::to_owned)), now),
            _ => {
                self.leave(ctx);
                return false;
            }
        }
        ctx.request_repaint();
        true
    }

    pub fn feed_input(&mut self, raw: &mut egui::RawInput) {
        for (key, modifiers) in self.modal_keys.drain(..) {
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
            }
        }
        let delta = std::mem::take(&mut self.wheel);
        if delta != 0.0 {
            if let Some(rect) = raw.screen_rect {
                raw.events.push(egui::Event::PointerMoved(rect.center()));
            }
            raw.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, delta), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: egui::Modifiers::NONE });
        }
    }

    pub fn leave(&mut self, ctx: &egui::Context) {
        if let Some(uri) = self.game_backdrop.take() {
            ctx.forget_image(&uri);
        }
        let was_active = std::mem::replace(&mut self.active, false);
        self.menu = None;
        self.quick_access = None;
        self.queued.clear();
        self.modal_keys.clear();
        if was_active && !self.windowed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    /// Running in a window (`--bigpicture=window`), not full screen.
    pub fn is_windowed(&self) -> bool {
        self.windowed
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.windowed = !self.windowed;
        self.fullscreen_asked = f64::NEG_INFINITY;
        if self.windowed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        ctx.request_repaint();
    }

    fn page(&self) -> Page {
        self.stack.last().cloned().unwrap_or(Page::Tab(self.tab))
    }

    fn remember(&mut self) {
        // Only page items: a top-bar item remembered for a page would jump back to the bar on return.
        if let Some(c) = self.focus.current.filter(|c| !is_bar_id(*c)) {
            self.memory.insert(self.page().key(), c);
        }
    }

    fn arrive(&mut self, now: f64, slide: f32) {
        self.chat_keys = false;
        self.focus.current = self.memory.get(&self.page().key()).copied();
        self.page_since = now;
        self.slide = slide;
    }

    /// Open a page on top of the current one.
    pub(crate) fn open(&mut self, page: Page, now: f64) {
        self.remember();
        if matches!(page, Page::Chat(_)) {
            self.message.clear();
            self.chat_for = None;
            self.chat_layout_cache = None;
        }
        if matches!(page, Page::Install(_)) {
            self.install_plan_cache = None;
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if matches!(page, Page::Compatibility(_)) {
            self.compat_cache = None;
        }
        if matches!(page, Page::GameManage(_)) {
            self.manage.app_id = None;
        }
        self.profile.on_open_page(&page);
        // A fresh visit starts at the top.
        self.memory.remove(&page.key());
        self.scroll.retain(|k, _| !k.starts_with(&page.key()));
        self.stack.push(page);
        self.arrive(now, 1.0);
    }

    /// Replace the top stacked page in place (e.g. Normal ↔ Foil card view).
    pub(crate) fn replace_top(&mut self, page: Page, now: f64) {
        self.profile.on_open_page(&page);
        self.memory.remove(&page.key());
        self.scroll.retain(|k, _| !k.starts_with(&page.key()));
        if let Some(top) = self.stack.last_mut() {
            *top = page;
        } else {
            self.stack.push(page);
        }
        self.arrive(now, 0.0);
    }

    pub(crate) fn back(&mut self, now: f64) {
        if !self.stack.is_empty() {
            self.remember();
            self.stack.pop();
            self.arrive(now, -1.0);
        } else if self.tab != Tab::Home {
            self.switch_tab(Tab::Home, now, true);
        } else {
            self.open_menu(now);
        }
    }

    /// Show another tab. `take_focus`: move focus into it (LB/RB, B), or
    /// leave it on the tab bar (moving along the tabs).
    pub(crate) fn switch_tab(&mut self, tab: Tab, now: f64, take_focus: bool) {
        if tab == self.tab && self.stack.is_empty() {
            return;
        }
        let keep = self.focus.current;
        self.remember();
        let slide = if tab.index() >= self.tab.index() { 1.0 } else { -1.0 };
        self.stack.clear();
        self.tab = tab;
        self.arrive(now, slide);
        if !take_focus {
            self.focus.current = keep;
        }
    }

    pub(crate) fn open_settings_at(&mut self, section: usize, now: f64) {
        self.settings.open_at(section);
        self.open(Page::Settings, now);
    }

    fn scroller(&mut self, key: &str) -> &mut Scroller {
        let k = format!("{}/{key}", self.page().key());
        self.scroll.entry(k).or_default()
    }
}

fn id_pair(arg: &str) -> Option<(GroupChatId, ChatRoomId)> {
    let (group, room) = arg.strip_prefix("group:")?.split_once('/')?;
    Some((group.parse().ok()?, room.parse().ok()?))
}

/// Drain controller events even when the root window has no frame to draw.
pub fn poll(app: &mut SteamApp, ctx: &egui::Context, visible: bool) -> bool {
    let tune_key = (app.prefs.bigpicture_swap_ab, app.prefs.bigpicture_repeat, app.prefs.bigpicture_stick);
    if app.bigpicture.last_tuned != Some(tune_key) {
        input::tune(tune_key.0, tune_key.1, tune_key.2);
        app.bigpicture.last_tuned = Some(tune_key);
    }
    let signed_in = app.state.phase == client::state::Phase::LoggedIn;
    let focused = visible && ctx.input(|i| {
        let viewport = i.viewport();
        viewport.focused.unwrap_or(i.raw.focused) && viewport.minimized != Some(true)
    });
    // A game launched by Aurora owns Guide while the client is in the background.
    let playing = app.state.playing.is_some();
    let bp = &mut app.bigpicture;
    let Some(rx) = bp.pad.take() else { return false };
    let mut show_window = false;
    for event in rx.try_iter() {
        match event {
            PadEvent::Connected(n) => bp.pads = n,
            PadEvent::Press(press) if signed_in => {
                show_window |= bp.handle_pad_press(ctx, press, focused, playing);
            }
            PadEvent::Press(_) => {}
        }
    }
    bp.pad = Some(rx);
    show_window
}

/// Size everything for the screen: 1.0 on a 1080p TV, bigger on 4K, readable on 1280×800 handhelds.
fn scale(screen: Rect, user: f32, deck_preview: bool) -> f32 {
    let base = (screen.width() / 1920.0).min(screen.height() / 1080.0);
    let handheld_boost = if deck_preview || (screen.width() <= 1366.0 && screen.height() <= 860.0) {
        1.08
    } else {
        1.0
    };
    (base * handheld_boost * user).clamp(0.5, 3.0)
}

/// Horizontal page margin: narrower on handheld/Deck screens to keep cards and settings roomy.
fn side_pad(screen: Rect, s: f32, deck_preview: bool) -> f32 {
    if deck_preview || screen.width() <= 1366.0 {
        48.0 * s
    } else {
        80.0 * s
    }
}

/// Draw Big Picture over the whole window.
pub fn show(app: &mut SteamApp, ui: &mut Ui, actions: &mut Vec<Action>) {
    let mut bp = std::mem::take(&mut app.bigpicture);
    bp.frame(app, ui, actions);
    if !bp.active {
        if bp.embedded.take().is_some() {
            app.restore_desktop_page();
        }
        screens::with_store(app, actions, |store, _| store.stop_trailer());
    }
    // Keep anything that arrived meanwhile (it's empty unless Guide re-entered).
    bp.queued.append(&mut app.bigpicture.queued);
    app.bigpicture = bp;
}

/// What a page is drawn with this frame.
pub struct Frame<'a> {
    pub app: &'a mut SteamApp,
    pub actions: &'a mut Vec<Action>,
    pub t: Theme,
    pub s: f32,
    /// The page's visible area and its padding at the sides.
    pub view: Rect,
    pub pad: f32,
    pub now: f64,
    /// X/Y/LT/RT pressed this frame (each page decides what they do).
    pub x: bool,
    pub y: bool,
    pub lt: bool,
    pub rt: bool,
    /// Right-stick vertical scroll delta (positive = down) in logical pixels.
    pub stick_scroll: f32,
}

impl BigPicture {
    fn frame(&mut self, app: &mut SteamApp, ui: &mut Ui, actions: &mut Vec<Action>) {
        let ctx = ui.ctx().clone();
        let t = app.prefs.theme.clone();
        let screen = ui.max_rect();
        let s = scale(screen, app.prefs.bigpicture_scale, self.deck_preview);
        let now = ctx.input(|i| i.time);
        self.sys_status = self.system.snapshot(self.quick_access.is_some(), Some(&ctx));
        if !self.fullscreen_sent {
            self.fullscreen_sent = true;
            if self.entered_at == 0.0 {
                self.entered_at = now;
            }
        }
        let mut toggle_fullscreen = false;
        ctx.input_mut(|i| i.events.retain(|event| {
            if let egui::Event::Key { key: egui::Key::F11, pressed, repeat, modifiers, .. } = event
                && *modifiers == egui::Modifiers::NONE
            {
                toggle_fullscreen |= *pressed && !*repeat;
                false
            } else {
                true
            }
        }));
        if toggle_fullscreen {
            self.toggle_fullscreen(&ctx);
        }
        // Fullscreen, and again if something resized the window meanwhile
        // (signing in sets the normal window size).
        if !self.windowed && ctx.input(|i| i.viewport().fullscreen) != Some(true) {
            if now - self.fullscreen_asked > 0.5 {
                self.fullscreen_asked = now;
                if ctx.input(|i| i.viewport().maximized) == Some(true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
                }
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Fullscreen(true));
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        // ---- input ----
        let drawer_open = self.menu.is_some() || self.quick_access.is_some();
        let page = self.page();
        let chat = matches!(page, Page::Chat(_));
        let lib_typing = page == Page::Tab(Tab::Library) && self.library_search_open;
        let prof_typing = self.profile.active_text_mut(&page).is_some();
        let typing = (page == Page::Search || chat || lib_typing || prof_typing) && !drawer_open && !self.launch_dialog;
        let mut submit = false;
        // Backspace only ever deletes (held down too, and on an empty box);
        // outside a text box it does nothing. Back is Esc / B.
        let backspaces: Vec<bool> = if typing { ctx.input_mut(|i| {
            let mut out = Vec::new();
            i.events.retain(|e| match e {
                egui::Event::Key { key: egui::Key::Backspace, pressed, modifiers, .. } => {
                    if *pressed {
                        out.push(modifiers.ctrl || modifiers.alt || modifiers.mac_cmd);
                    }
                    false
                }
                _ => true,
            });
            out
        }) } else { Vec::new() };
        if typing {
            let text: String = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.clone()) } else { None }).collect());
            let field: Option<&mut String> = if chat {
                Some(&mut self.message)
            } else if lib_typing {
                Some(&mut self.library_query)
            } else if prof_typing {
                self.profile.active_text_mut(&page)
            } else {
                Some(&mut self.query)
            };
            if let Some(field) = field {
                if !text.is_empty() {
                    field.push_str(&text);
                    self.query_changed = now;
                    self.device = Device::Keyboard;
                }
                for word in &backspaces {
                    input::backspace(field, *word);
                }
                if !backspaces.is_empty() {
                    self.query_changed = now;
                    self.device = Device::Keyboard;
                }
            }
        }
        let embedded_typing = matches!(page, Page::Embedded(..)) && !drawer_open && ctx.egui_wants_keyboard_input();
        let keys = if embedded_typing || self.launch_dialog { Vec::new() } else { input::keyboard(&ctx, typing) };
        if !keys.is_empty() {
            self.device = Device::Keyboard;
        } else if !self.queued.is_empty() {
            self.device = Device::Pad;
        }
        let mut presses: Vec<(Press, bool)> = keys.into_iter().map(|p| (p, true)).collect();
        presses.extend(self.queued.drain(..).map(|p| (p, false)));
        if let Some((p, _)) = presses.last() {
            self.flash = Some((*p, now));
        }

        if self.launch_dialog {
            for (press, _) in presses.drain(..) {
                let key = match press {
                    Press::A => (egui::Key::Enter, egui::Modifiers::NONE),
                    Press::B => (egui::Key::Escape, egui::Modifiers::NONE),
                    Press::Move(Dir::Left | Dir::Up) => (egui::Key::Tab, egui::Modifiers::SHIFT),
                    Press::Move(Dir::Right | Dir::Down) => (egui::Key::Tab, egui::Modifiers::NONE),
                    _ => continue,
                };
                self.modal_keys.push(key);
            }
            if !self.modal_keys.is_empty() {
                ctx.request_repaint();
            }
        }
        let mut dir = None;
        let mut press = false;
        let (mut x, mut y, mut lt, mut rt) = (false, false, false, false);
        let mut stick_scroll = 0.0f32;
        if let Some(m) = &mut self.menu {
            if presses.iter().any(|(p, _)| *p == Press::QuickAccess) {
                self.open_quick_access(&ctx, quick_access::Section::System, now);
            } else if m.input(&presses, now) == menu::Outcome::LeaveBigPicture {
                self.leave(&ctx);
                ctx.request_repaint();
                return;
            }
        } else if let Some(qa) = &mut self.quick_access {
            if presses.iter().any(|(p, _)| matches!(p, Press::Start | Press::Guide)) {
                self.open_menu(now);
            } else {
                qa.input(&presses, now);
            }
        } else {
            for (p, from_keys) in presses {
                match p {
                    Press::Move(d) => dir = Some(d),
                    // Typing on a real keyboard: Enter searches what was typed
                    // (and then selects, like everywhere else).
                    Press::A if typing && from_keys && chat && !self.message.trim().is_empty() => self.send_now = true,
                    Press::A if typing && from_keys && lib_typing => self.library_search_open = false,
                    Press::A if typing && from_keys && prof_typing => {
                        self.profile.close_osk(&page);
                    }
                    Press::A if typing && from_keys && !chat && self.query.trim() != self.submitted => submit = true,
                    Press::A => press = true,
                    // Library search OSK open: B closes it first.
                    Press::B if lib_typing => self.library_search_open = false,
                    // Profile OSK open: B closes it first.
                    Press::B if self.profile.close_osk(&page) => {}
                    // Chat with the keyboard up: B puts it away first.
                    Press::B if chat && self.chat_keys => {
                        self.chat_keys = false;
                        self.focus.current = Some(screens::chat_history_id());
                    }
                    Press::B => self.back(now),
                    Press::X => x = true,
                    Press::Y => y = true,
                    Press::Lt => lt = true,
                    Press::Rt => rt = true,
                    Press::Scroll(delta) => stick_scroll += delta as f32 * 110.0 * s,
                    Press::Lb => self.switch_tab(TABS[(self.tab.index() + TABS.len() - 1) % TABS.len()], now, true),
                    Press::Rb => self.switch_tab(TABS[(self.tab.index() + 1) % TABS.len()], now, true),
                    Press::Start | Press::Guide => self.open_menu(now),
                    Press::QuickAccess => self.open_quick_access(&ctx, quick_access::Section::System, now),
                }
            }
        }
        if submit {
            self.submitted = self.query.trim().to_string();
        }
        // The page changed this frame: a move meant for the old one is dropped.
        if self.page() != page {
            dir = None;
            press = false;
            stick_scroll = 0.0;
        }
        // A trailer: A pauses, left/right move 10 seconds.
        if let Page::Video(..) = self.page() {
            let seconds = match dir {
                Some(Dir::Left) => -10.0,
                Some(Dir::Right) => 10.0,
                _ => 0.0,
            };
            if press || seconds != 0.0 {
                screens::with_store(app, actions, |st, _| st.trailer_control(press, seconds));
            }
            dir = None;
            press = false;
        }
        // Chat: up/down on the history scroll it; past its ends they move on.
        if let (Page::Chat(_), Some(d @ (Dir::Up | Dir::Down))) = (self.page(), dir) {
            let (at_top, at_bottom) = self.chat_edges;
            let leave = (d == Dir::Up && at_top) || (d == Dir::Down && at_bottom);
            if self.focus.current == Some(screens::chat_history_id()) && !leave {
                self.chat_nudge += if d == Dir::Up { -1.0 } else { 1.0 };
                dir = None;
            }
        }
        if matches!(self.page(), Page::Chat(_)) && stick_scroll != 0.0 {
            self.chat_nudge += stick_scroll / (180.0 * s).max(1.0);
            stick_scroll = 0.0;
        }
        if let Page::Embedded(..) = self.page() {
            if let Some(d @ (Dir::Up | Dir::Down)) = dir {
                self.wheel += if d == Dir::Up { 120.0 } else { -120.0 } * s;
                ctx.request_repaint();
                dir = None;
            }
            if stick_scroll != 0.0 {
                self.wheel -= stick_scroll;
                ctx.request_repaint();
                stick_scroll = 0.0;
            }
        }
        if let (Page::Review(..) | Page::News(..), Some(d @ (Dir::Up | Dir::Down))) = (self.page(), dir) {
            self.scroller("page").target += if d == Dir::Up { -120.0 } else { 120.0 } * s;
            dir = None;
        }
        // The screenshot viewer pages with left/right.
        if let (Page::Shots(app_id, i), Some(d @ (Dir::Left | Dir::Right))) = (self.page(), dir) {
            let n = screens::shot_count(app, actions, app_id);
            if n > 0 {
                let step = if d == Dir::Left { n - 1 } else { 1 };
                *self.stack.last_mut().unwrap() = Page::Shots(app_id, (i + step) % n);
                self.shot_since = (now, if d == Dir::Left { -1.0 } else { 1.0 });
            }
            dir = None;
        }
        let drawer_open = self.menu.is_some() || self.quick_access.is_some();
        let before = self.focus.current;
        if drawer_open {
            self.focus.pause_frame();
        } else {
            self.focus.begin(ui, dir, press);
            // Up from a page into the tab bar lands on this page's tab (the
            // nearest one could be another tab, which would switch pages).
            // Moving sideways while already in the top bar moves freely to Bell, Quick Access, Avatar, Logo.
            if self.focus.moved && before.is_some_and(|b| !is_bar_id(b)) && self.focus.current.is_some_and(is_bar_id) {
                self.focus.current = Some(tab_id(self.tab));
            }
        }

        // ---- draw ----
        let top_h = 98.0 * s;
        let hint_h = 74.0 * s;
        let view = Rect::from_min_max(pos2(screen.left(), screen.top() + top_h), pos2(screen.right(), screen.bottom() - hint_h));
        self.preferred = None;

        let key = format!("{}/page", self.page().key());
        let pos = self.scroll.get(&key).map(|sc| sc.pos).unwrap_or(0.0);
        let scroll_pos = match self.page() {
            Page::Settings => self.scroll.get(&format!("{}/rows", self.page().key())).map(|sc| sc.pos).unwrap_or(0.0),
            Page::ProfileEdit => self.profile.edit.body_scroll.pos,
            _ => pos,
        };
        let focus_in_bar = self.focus.current.is_some_and(is_bar_id);
        let pointer_at_top = !drawer_open
            && ui.input(|i| i.pointer.latest_pos().is_some_and(|p| p.y <= screen.top() + 32.0 * s));
        let target_hide = if focus_in_bar || pointer_at_top {
            0.0
        } else {
            ((scroll_pos - 12.0 * s) / (top_h * 0.85)).clamp(0.0, 1.0)
        };
        let dt = ui.input(|i| i.stable_dt.min(0.1));
        let k = 1.0 - (-dt * 14.0).exp();
        self.bar_hide += (target_hide - self.bar_hide) * k;
        if (target_hide - self.bar_hide).abs() < 0.005 {
            self.bar_hide = target_hide;
        } else {
            ui.ctx().request_repaint();
        }

        // Pages slide and fade in.
        let e = since(ui, self.page_since, 0.32);
        let dx = (1.0 - e) * 80.0 * s * self.slide;
        let mut content = paint::faded(ui, screen, e.max(0.0));
        if drawer_open {
            content.disable();
        }
        let clip_top = screen.top() + (top_h - 28.0 * s).max(0.0) * (1.0 - self.bar_hide);
        content.set_clip_rect(Rect::from_min_max(pos2(screen.left(), clip_top), pos2(screen.right(), view.bottom())));
        let page_view = view.translate(vec2(dx, 0.0));
        let pad = side_pad(screen, s, self.deck_preview);
        let mut f = Frame { app, actions, t: t.clone(), s, view: page_view, pad, now, x, y, lt, rt, stick_scroll };

        self.background(ui, &f);
        let height = screens::draw(self, &mut content, &mut f, pos);
        // Scroll the page: follow the focus, right-stick, or mouse wheel (only while no drawer is open).
        if !drawer_open {
            let page_has_own_scroll = matches!(self.page(), Page::Settings | Page::ProfileEdit);
            let wheel = ui.input(|i| i.smooth_scroll_delta.y);
            let moved = self.focus.moved;
            let focused = self.focus.focused();
            let sc = self.scroll.entry(key).or_default();
            if wheel != 0.0 && ui.rect_contains_pointer(view) && !page_has_own_scroll {
                sc.target -= wheel;
            }
            if f.stick_scroll != 0.0 && !page_has_own_scroll {
                sc.target += f.stick_scroll;
            }
            if !page_has_own_scroll && let (true, Some((r, Place::Page))) = (moved, focused) {
                let top = r.top() - (view.top() - sc.pos);
                sc.reveal(top, top + r.height(), view.height(), view.height() * 0.2);
                // Near the start: show the very top (the page title, the hero).
                if sc.target < view.height() * 0.35 {
                    sc.target = 0.0;
                }
            }
            sc.clamp(height, view.height());
            sc.step(ui);
        }

        if self.bar_hide < 0.99 {
            let mut bar_ui = paint::faded(ui, screen, (1.0 - self.bar_hide).clamp(0.0, 1.0));
            if drawer_open {
                bar_ui.disable();
            }
            self.top_bar(&mut bar_ui, &mut f, screen, top_h, -top_h * self.bar_hide);
        }
        if !drawer_open {
            let first: Vec<Id> = self.preferred.into_iter().collect();
            let assigned = self.focus.end(&first, tab_id(self.tab));
            if assigned
                && let Some((r, Place::Page)) = self.focus.focused()
            {
                let key = format!("{}/page", self.page().key());
                let sc = self.scroll.entry(key).or_default();
                let top = r.top() - (view.top() - sc.pos);
                sc.reveal(top, top + r.height(), view.height(), view.height() * 0.2);
                sc.clamp(height, view.height());
                ui.ctx().request_repaint();
            }
            self.focus.paint_ring(ui, &t, s, 12.0 * s, content.clip_rect());
        }
        {
            let mut hint_ui = ui.new_child(egui::UiBuilder::new().max_rect(screen));
            if drawer_open {
                hint_ui.disable();
            }
            self.hint_bar(&mut hint_ui, &mut f, screen, hint_h);
        }
        if self.menu.is_some() {
            menu::show(self, ui, &mut f, screen);
        }
        if self.quick_access.is_some() {
            quick_access::show(self, ui, &mut f, screen);
        }
        if !self.active {
            return;
        }
        self.intro(ui, &t, screen, s, now);

        // Event-driven repaint scheduling (no unconditional 33ms idle loop):
        // - Search debounce timer
        if self.page() == Page::Search {
            let q = self.query.trim();
            if q != self.submitted && (q.chars().count() >= 2 || q.is_empty()) {
                let wait = (0.6 - (now - self.query_changed)).max(0.05);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            }
        }
        // - Caret blink only on typing pages (calm 500ms cadence instead of 30fps)
        if typing {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        // - Active downloads / file ops refresh at 500ms
        let active_transfer = f.app.downloading()
            || !f.app.state.moving.is_empty()
            || !f.app.state.backing_up.is_empty();
        if active_transfer {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        // - Top-bar clock rollover at the next minute boundary
        ctx.request_repaint_after(std::time::Duration::from_secs(self.sys_status.secs_until_next_minute.clamp(1, 60)));
    }

    /// The focused game's art, cross-faded cleanly with two slots (no per-frame idle spin);
    /// soft static theme glow behind everything.
    fn background(&mut self, ui: &mut Ui, f: &Frame) {
        let (t, _s, now) = (&f.t, f.s, f.now);
        let screen = ui.max_rect();
        let p = ui.painter().clone();
        p.rect_filled(screen, 0.0, t.bg);
        // Two calm theme glows in fixed balanced positions (zero idle animation cost).
        let a = screen.center() + vec2(-screen.width() * 0.24, -screen.height() * 0.22);
        let b = screen.center() + vec2(screen.width() * 0.28, screen.height() * 0.24);
        paint::glow(&p, a, screen.width() * 0.45, t.accent.gamma_multiply(0.10));
        paint::glow(&p, b, screen.width() * 0.4, t.accent_2.gamma_multiply(0.09));

        let profile_id = match self.page() {
            Page::Profile(id)
            | Page::ProfileGames(id)
            | Page::ProfileBadges(id)
            | Page::ProfileCards(id, _, _)
            | Page::ProfileReviews(id)
            | Page::ProfileGroups(id) => Some(if id == 0 { f.app.state.steam_id } else { id }),
            Page::ProfileEdit => Some(f.app.state.steam_id),
            _ => None,
        };
        if let Some(pid) = profile_id {
            if let Some(bg_url) = f.app.state.profiles.get(&pid).and_then(|i| i.background.as_deref()) {
                if w::paint_cover(ui, screen, bg_url, egui::CornerRadius::ZERO) {
                    p.rect_filled(screen, 0.0, t.bg.gamma_multiply(0.58));
                    w::gradient(&p, screen, t.bg.gamma_multiply(0.25), t.bg.gamma_multiply(0.88), false);
                    return;
                }
            }
        }

        if let Page::Game(id) | Page::GameManage(id) = self.page() {
            let e = since(ui, self.page_since, 0.7);
            let rect = paint::grow(screen, 1.0 + (1.0 - e) * 0.04);
            let artwork = paint::faded(ui, screen, since(ui, self.page_since, 0.35));
            let uri = w::paint_blurred_cover_first(&artwork, screen, rect, &[&model::art::hero(id), &model::art::header(id)]);
            if self.game_backdrop != uri {
                if let Some(old) = self.game_backdrop.take() {
                    ui.ctx().forget_image(&old);
                }
                self.game_backdrop = uri;
            }
            w::gradient(&p, screen, t.bg.gamma_multiply(0.08), t.bg.gamma_multiply(0.94), false);
            w::fade_right(ui, screen, t.bg.gamma_multiply(0.65), 0.58);
            return;
        }
        if let Some(uri) = self.game_backdrop.take() {
            ui.ctx().forget_image(&uri);
        }

        // Pages say which game is focused while drawing: crossfade cleanly with 2 slots.
        if let Some(want) = self.bg_want.take()
            && self.bg_cur != Some(want)
        {
            self.bg_prev = self.bg_cur;
            self.bg_cur = Some(want);
            self.bg_since = now;
        }
        let fade = since(ui, self.bg_since, 0.55);
        if fade >= 1.0 {
            self.bg_prev = None;
        }
        let big = paint::grow(screen, 1.06);
        if let Some(prev) = self.bg_prev {
            let c = paint::faded(ui, screen, (1.0 - fade) * 0.38);
            w::paint_cover_first(&c, big, &[&model::art::hero(prev), &model::art::header(prev)], egui::CornerRadius::ZERO);
        }
        if let Some(cur) = self.bg_cur {
            let c = paint::faded(ui, screen, fade * 0.38);
            w::paint_cover_first(&c, big, &[&model::art::hero(cur), &model::art::header(cur)], egui::CornerRadius::ZERO);
        }
        // Darken toward the bottom and left so text stays readable.
        w::gradient(&p, screen, t.bg.gamma_multiply(0.55), t.bg.gamma_multiply(0.97), false);
        w::fade_right(ui, screen, t.bg.gamma_multiply(0.75), 0.6);
    }

    fn top_bar(&mut self, ui: &mut Ui, f: &mut Frame, screen: Rect, h: f32, y_off: f32) {
        let (t, s) = (f.t.clone(), f.s);
        let bar = Rect::from_min_size(pos2(screen.left(), screen.top() + y_off), vec2(screen.width(), h));
        let p = ui.painter().clone();
        w::gradient(&p, bar, t.bg.gamma_multiply(0.96), t.bg.gamma_multiply(0.0), false);
        let pad = side_pad(screen, s, self.deck_preview);
        // Logo on the left (controller-focusable).
        let logo_rect = Rect::from_center_size(pos2(bar.left() + pad + 20.0 * s, bar.center().y), vec2(44.0 * s, 44.0 * s));
        let logo_hit = self.focus.item(ui, top_logo_id(), logo_rect, screen, Place::Bar);
        if logo_hit.glow > 0.0 {
            p.circle_filled(logo_rect.center(), 24.0 * s, Color32::from_white_alpha((26.0 * logo_hit.glow) as u8));
        }
        let mut logo = ui.new_child(egui::UiBuilder::new().max_rect(logo_rect.shrink(2.0 * s)));
        w::logo(&mut logo, &t, 40.0 * s);
        if logo_hit.focused {
            self.focus.ring_around(logo_rect.expand(2.0 * s), 24.0 * s);
        }
        if logo_hit.activated {
            self.open_menu(f.now);
        }

        // Running-game pill beside the logo when a game is active (controller-focusable).
        let mut left_edge = logo_rect.right() + 18.0 * s;
        if let Some(playing_id) = f.app.state.playing {
            let name = f.app.state.app_name(playing_id).unwrap_or("Game");
            let pill_label = format!("{}  {}", icons::PLAY, name);
            let font = w::font_semibold(17.0 * s);
            let pw = (paint::text_width(&p, &pill_label, font.clone()) + 28.0 * s).min(240.0 * s);
            let pr = Rect::from_min_size(pos2(left_edge, bar.center().y - 20.0 * s), vec2(pw, 40.0 * s));
            let hit = self.focus.item(ui, top_playing_id(), pr, screen, Place::Bar);
            p.rect_filled(pr, paint::radius(20.0 * s), w::mix(t.play.gamma_multiply(0.28), t.play.gamma_multiply(0.48), hit.glow));
            p.rect_stroke(pr, paint::radius(20.0 * s), egui::Stroke::new(1.5 * s, t.play.gamma_multiply(0.7)), egui::StrokeKind::Inside);
            paint::line(&p, pr.center(), Align2::CENTER_CENTER, &pill_label, font, t.text, pw - 20.0 * s);
            if hit.focused {
                self.focus.ring_around(pr, 20.0 * s);
            }
            if hit.activated {
                self.open(Page::Game(playing_id), f.now);
            }
            left_edge = pr.right() + 14.0 * s;
        }
        let _ = left_edge;

        // Tabs in the middle.
        let font = w::font_bold(24.0 * s);
        let gap = 10.0 * s;
        let widths: Vec<f32> = TABS.iter().map(|tab| paint::text_width(&p, &tab_label(f, *tab), font.clone()) + 38.0 * s).collect();
        let total: f32 = widths.iter().sum::<f32>() + gap * (TABS.len() - 1) as f32;
        let mut x = bar.center().x - total / 2.0;
        let tab_h = 52.0 * s;
        let mut tabs = Vec::with_capacity(TABS.len());
        for (i, tab) in TABS.iter().enumerate() {
            let r = Rect::from_min_size(pos2(x, bar.center().y - tab_h / 2.0), vec2(widths[i], tab_h));
            x += widths[i] + gap;
            let hit = self.focus.item(ui, tab_id(*tab), r, screen, Place::Bar);
            if hit.focused {
                self.focus.ring_around(r, tab_h / 2.0);
            }
            let selected = *tab == self.tab;
            if hit.focused && !selected && !self.focus.by_pointer {
                // Moving along the tabs (D-pad, keys) shows each one; the
                // mouse only highlights a tab until it's clicked.
                self.switch_tab(*tab, f.now, false);
            }
            if hit.activated {
                if !self.stack.is_empty() || !selected {
                    // Clicking the current tab from a subpage returns to its root.
                    self.switch_tab(*tab, f.now, true);
                } else {
                    // A on the current tab goes into its page.
                    self.focus.current = self.memory.get(&Page::Tab(*tab).key()).copied().or(Some(Id::NULL));
                    self.focus.moved = true;
                }
            }
            tabs.push((r, hit.glow));
        }
        // The selected tab's fill, underline and focus outline share one animation.
        let target = tabs[self.tab.index()].0;
        let dt = ui.input(|i| i.stable_dt.min(0.1));
        let k = 1.0 - (-dt * 14.0).exp();
        let cur = self.tab_pill.map(|c| Rect::from_min_max(c.min + (target.min - c.min) * k, c.max + (target.max - c.max) * k)).unwrap_or(target);
        let cur = if (cur.min - target.min).length() > 0.5 || (cur.max - target.max).length() > 0.5 {
            ui.ctx().request_repaint();
            cur
        } else {
            target
        };
        self.tab_pill = Some(cur);
        p.rect_filled(cur, paint::radius(cur.height() / 2.0), Color32::from_white_alpha(18));
        let line = Rect::from_center_size(pos2(cur.center().x, cur.bottom() + 6.0 * s), vec2(cur.width() * 0.4, 4.0 * s));
        w::gradient(&p.with_clip_rect(line), line, t.accent, t.accent_2, true);
        if self.focus.current == Some(tab_id(self.tab)) {
            self.focus.ring_around(cur, cur.height() / 2.0);
        }
        for (tab, (r, glow)) in TABS.iter().zip(tabs) {
            let color = if *tab == self.tab { t.text } else { w::mix(t.text_dim, t.text, glow) };
            p.text(r.center(), Align2::CENTER_CENTER, tab_label(f, *tab), font.clone(), color);
        }

        // Right cluster: Avatar (Profile) · Quick Access (⋯) · Clock · Battery · Notifications (Bell) · Controller.
        // All interactive buttons are registered with `Focus` so the controller can navigate across them!
        let mut rx = bar.right() - pad;
        let me = f.app.state.me().cloned();
        let av = Rect::from_center_size(pos2(rx - 23.0 * s, bar.center().y), vec2(46.0 * s, 46.0 * s));
        let av_hit = self.focus.item(ui, top_avatar_id(), av, screen, Place::Bar);
        let url = model::art::avatar_full(me.as_ref().and_then(|m| m.avatar_hash.as_deref()));
        if !w::paint_cover(ui, av, &url, paint::radius(23.0 * s)) {
            p.circle_filled(av.center(), 23.0 * s, t.raised);
        }
        if let Some(m) = &me {
            p.circle_stroke(av.center(), 24.5 * s, egui::Stroke::new(2.5 * s, crate::shell::status_color(&t, m)));
        }
        if av_hit.focused {
            self.focus.ring_around(av.expand(2.0 * s), 25.0 * s);
        }
        if av_hit.activated && f.app.state.steam_id != 0 {
            self.open(Page::Profile(f.app.state.steam_id), f.now);
        }
        rx = av.left() - 16.0 * s;

        // Quick Access button (`⋯`, controller-focusable).
        let qa_r = Rect::from_center_size(pos2(rx - 22.0 * s, bar.center().y), vec2(44.0 * s, 44.0 * s));
        let qa_hit = self.focus.item(ui, top_qa_id(), qa_r, screen, Place::Bar);
        p.circle_filled(qa_r.center(), 22.0 * s, w::mix(Color32::from_white_alpha(18), Color32::from_white_alpha(44), qa_hit.glow));
        p.text(qa_r.center(), Align2::CENTER_CENTER, icons::DOTS, w::font(22.0 * s), t.text);
        if qa_hit.focused {
            self.focus.ring_around(qa_r, 22.0 * s);
        }
        if qa_hit.activated {
            self.open_quick_access(&ui.ctx().clone(), quick_access::Section::System, f.now);
        }
        rx = qa_r.left() - 16.0 * s;

        // Wall clock (`HH:MM`).
        if !self.sys_status.clock.is_empty() {
            let clk_r = p.text(pos2(rx, bar.center().y), Align2::RIGHT_CENTER, &self.sys_status.clock, w::font_semibold(20.0 * s), t.text);
            rx = clk_r.left() - 16.0 * s;
        }

        // Battery telemetry (when a system battery is present, e.g. Steam Deck / laptop).
        if let Some(pct) = self.sys_status.battery_percent {
            let color = if self.sys_status.battery_charging {
                t.play
            } else if pct <= 15 {
                t.danger
            } else if pct <= 30 {
                t.away
            } else {
                t.text_dim
            };
            let label = if self.sys_status.battery_charging {
                format!("{} {pct}%", icons::LIGHTNING)
            } else {
                format!("{pct}%")
            };
            let br = p.text(pos2(rx, bar.center().y), Align2::RIGHT_CENTER, label, w::font_semibold(18.0 * s), color);
            rx = br.left() - 16.0 * s;
        }

        // Notifications bell (always shown & controller-focusable; opens Quick Access on Notifications).
        let unread = f.app.state.notifications.as_ref().map_or(0usize, |d| d.unread as usize) + f.app.state.total_unread();
        let bell_r = Rect::from_center_size(pos2(rx - 22.0 * s, bar.center().y), vec2(44.0 * s, 44.0 * s));
        let bell_hit = self.focus.item(ui, top_bell_id(), bell_r, screen, Place::Bar);
        let bell_idle = if unread > 0 { t.accent.gamma_multiply(0.24) } else { Color32::from_white_alpha(14) };
        let bell_active = if unread > 0 { t.accent.gamma_multiply(0.44) } else { Color32::from_white_alpha(38) };
        p.circle_filled(bell_r.center(), 22.0 * s, w::mix(bell_idle, bell_active, bell_hit.glow));
        p.text(bell_r.center(), Align2::CENTER_CENTER, icons::BELL, w::font(21.0 * s), if unread > 0 || bell_hit.focused { t.text } else { t.text_dim });
        if unread > 0 {
            paint::badge(&p, bell_r.right_top() + vec2(3.0 * s, -2.0 * s), Align2::CENTER_CENTER, &unread.min(99).to_string(), s * 0.72, t.accent, t.on_accent);
        }
        if bell_hit.focused {
            self.focus.ring_around(bell_r, 22.0 * s);
        }
        if bell_hit.activated {
            self.open_quick_access(&ui.ctx().clone(), quick_access::Section::Notifications, f.now);
        }
        rx = bell_r.left() - 16.0 * s;

        // Offline indicator pill.
        if !f.app.state.online() && !f.app.demo {
            let off_r = paint::badge(&p, pos2(rx, bar.center().y), Align2::RIGHT_CENTER, t!("Offline"), s * 0.9, t.away.gamma_multiply(0.28), t.away);
            rx = off_r.left() - 14.0 * s;
        }

        // Connected controllers indicator.
        if self.pads > 0 {
            p.text(pos2(rx, bar.center().y), Align2::RIGHT_CENTER, icons::CONTROLLER, w::font(24.0 * s), t.text_dim);
        }
    }

    fn hint_bar(&mut self, ui: &mut Ui, f: &mut Frame, screen: Rect, h: f32) {
        let (t, s) = (f.t.clone(), f.s);
        let bar = Rect::from_min_max(pos2(screen.left(), screen.bottom() - h), screen.max);
        let p = ui.painter().clone();
        // Content fades out just above the bar.
        let fade = Rect::from_min_max(pos2(bar.left(), bar.top() - 36.0 * s), pos2(bar.right(), bar.top()));
        w::gradient(&p, fade, t.bg.gamma_multiply(0.0), t.bg.gamma_multiply(0.85), false);
        w::gradient(&p, bar, t.bg.gamma_multiply(0.85), t.bg.gamma_multiply(0.97), false);
        let pad = side_pad(screen, s, self.deck_preview);
        p.hline(bar.x_range().shrink(pad), bar.top() + 6.0 * s, egui::Stroke::new(1.0, Color32::from_white_alpha(18)));
        let pad_mode = self.device == Device::Pad;
        // Left: where you are.
        let left = match self.page() {
            Page::Tab(_) => format!("{}  {}", if pad_mode { "LB / RB" } else { "Q / E" }, t!("Switch tabs")),
            Page::Settings | Page::ProfileEdit => format!("{}  {}", if pad_mode { "◀ / ▶" } else { "← / →" }, t!("Adjust value")),
            _ => String::new(),
        };
        if !left.is_empty() {
            p.text(pos2(bar.left() + pad, bar.center().y), Align2::LEFT_CENTER, left, w::font_medium(18.0 * s), t.text_faint);
        }
        let drawer_open = self.menu.is_some() || self.quick_access.is_some();
        let osk_open = (matches!(self.page(), Page::Chat(_)) && self.chat_keys)
            || (self.page() == Page::Tab(Tab::Library) && self.library_search_open)
            || self.profile.active_text_mut(&self.page()).is_some();
        let mut hints: Vec<(Press, String)> = vec![(Press::A, t!("Select").into()), (Press::B, if drawer_open || osk_open { t!("Close") } else { t!("Back") }.into())];
        if !drawer_open {
            if let Some(x) = screens::x_hint(self) {
                hints.push((Press::X, x));
            }
            if let Some(y) = screens::y_hint(self) {
                hints.push((Press::Y, y));
            }
            hints.push((Press::QuickAccess, t!("Quick Access").into()));
            hints.push((Press::Start, t!("Menu").into()));
        }
        let mut x = bar.right() - pad;
        let flash = self.flash.filter(|(_, at)| f.now - at < 0.3);
        for (press, label) in hints.iter().rev() {
            let lw = paint::text_width(&p, label, w::font_medium(19.0 * s));
            x -= lw;
            p.text(pos2(x, bar.center().y), Align2::LEFT_CENTER, label, w::font_medium(19.0 * s), t.text_dim);
            let (glyph, round) = match (press, pad_mode, self.page()) {
                // Typing: X is a letter, Backspace deletes.
                (Press::X, false, Page::Search | Page::Chat(_)) => ("Backspace", false),
                (Press::X, false, _) if osk_open => ("Backspace", false),
                _ => glyph_for(*press, pad_mode),
            };
            let fl = flash.filter(|(fp, _)| fp == press).map(|(_, at)| 1.0 - ((f.now - at) / 0.3) as f32).unwrap_or(0.0);
            let gw = paint::text_width(&p, glyph, w::font_bold(17.0 * s)) + 22.0 * s;
            let gw = if round { 34.0 * s } else { gw.max(34.0 * s) };
            x -= gw + 10.0 * s;
            let r = paint::glyph(&p, &t, pos2(x, bar.center().y), glyph, round, s, fl);
            // Hints are buttons for the mouse too.
            let resp = ui.interact(r.union(Rect::from_min_size(r.right_top(), vec2(lw + 10.0 * s, r.height()))), Id::unique(("bp-hint", label)), egui::Sense::click());
            if resp.clicked() {
                self.queued.push(*press);
            }
            x -= 28.0 * s;
        }
        if flash.is_some() {
            ui.ctx().request_repaint();
        }
    }

    /// Entering: the logo blooms and fades as the screen appears.
    fn intro(&self, ui: &mut Ui, t: &Theme, screen: Rect, s: f32, now: f64) {
        let e = ((now - self.entered_at) / 0.9) as f32;
        if !(0.0..1.0).contains(&e) {
            return;
        }
        ui.ctx().request_repaint();
        let fade = 1.0 - focus::ease(((e - 0.35) / 0.65).max(0.0));
        let p = ui.painter().clone();
        p.rect_filled(screen, 0.0, t.bg.gamma_multiply(fade));
        paint::glow(&p, screen.center(), screen.height() * (0.25 + 0.5 * e), t.accent.gamma_multiply(0.35 * fade));
        let size = 120.0 * s * (0.8 + 0.35 * focus::ease(e));
        let mut c = paint::faded(ui, Rect::from_center_size(screen.center(), vec2(size, size)), fade);
        w::logo(&mut c, t, size);
        p.text(screen.center() + vec2(0.0, size * 0.75 + 20.0 * s), Align2::CENTER_TOP, if self.deck_preview { t!("Deck preview") } else { t!("Big Picture") }, w::font_bold(30.0 * s), t.text.gamma_multiply(fade));
    }
}

pub(crate) fn tab_id(tab: Tab) -> Id {
    Id::unique(("bp-tab", tab.index()))
}

fn top_logo_id() -> Id {
    Id::unique("bp-top-logo")
}

fn top_playing_id() -> Id {
    Id::unique("bp-top-playing")
}

fn top_bell_id() -> Id {
    Id::unique("bp-top-bell")
}

fn top_qa_id() -> Id {
    Id::unique("bp-top-qa")
}

fn top_avatar_id() -> Id {
    Id::unique("bp-top-avatar")
}

fn is_bar_id(id: Id) -> bool {
    TABS.iter().any(|t| tab_id(*t) == id)
        || id == top_logo_id()
        || id == top_playing_id()
        || id == top_bell_id()
        || id == top_qa_id()
        || id == top_avatar_id()
}

/// Tab names, with a count where it helps (downloads, friends online).
pub(crate) fn tab_label(f: &Frame, tab: Tab) -> String {
    match tab {
        Tab::Downloads => {
            let n = f.app.state.downloads.values().filter(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)).count() + f.app.state.download_queue.len();
            if n > 0 { format!("{} · {n}", tab.label()) } else { tab.label().to_string() }
        }
        _ => tab.label().to_string(),
    }
}

/// The glyph for a press: controller buttons, or keys.
pub(crate) fn glyph_for(p: Press, pad: bool) -> (&'static str, bool) {
    match (p, pad) {
        (Press::A, true) => ("A", true),
        (Press::B, true) => ("B", true),
        (Press::X, true) => ("X", true),
        (Press::Y, true) => ("Y", true),
        (Press::Lt, true) => ("LT", false),
        (Press::Rt, true) => ("RT", false),
        (Press::Start, true) => (icons::LIST, true),
        (Press::A, false) => ("Enter", false),
        (Press::B, false) => ("Esc", false),
        (Press::X, false) => ("X", false),
        (Press::Y, false) => ("Y", false),
        (Press::Lt, false) => ("[", false),
        (Press::Rt, false) => ("]", false),
        (Press::Start, false) => ("M", false),
        (Press::QuickAccess, true) => (icons::DOTS, true),
        (Press::QuickAccess, false) => ("F2", false),
        (_, true) => ("LB", false),
        (_, false) => ("Q", false),
    }
}

#[cfg(test)]
mod guide_tests {
    use super::*;

    #[test]
    fn guide_opens_fullscreen_big_picture_from_the_desktop() {
        let ctx = egui::Context::default();
        let mut bp = BigPicture { windowed: true, ..Default::default() };

        assert!(bp.handle_pad_press(&ctx, Press::Guide, false, false));
        assert!(bp.active);
        assert!(!bp.is_windowed());
        assert_eq!(bp.device, Device::Pad);
        assert!(bp.queued.is_empty());
    }

    #[test]
    fn guide_restores_unfocused_big_picture_without_opening_the_menu() {
        let ctx = egui::Context::default();
        let mut bp = BigPicture { active: true, tab: Tab::Library, ..Default::default() };

        assert!(!bp.handle_pad_press(&ctx, Press::A, false, false));
        assert!(!bp.handle_pad_press(&ctx, Press::Guide, false, true));
        assert!(bp.handle_pad_press(&ctx, Press::Guide, false, false));
        assert_eq!(bp.tab.index(), Tab::Library.index());
        assert!(bp.queued.is_empty());

        assert!(!bp.handle_pad_press(&ctx, Press::Guide, true, true));
        assert_eq!(bp.queued, [Press::Guide]);
    }
}
