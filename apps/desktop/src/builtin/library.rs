//! Library: sidebar game list, library home with shelves, and game pages.

use ui::widgets::EditMenu;
use i18n::t;
use super::{Entry, entries};

#[path = "library_news.rs"]
mod news;
#[path = "library_props.rs"]
mod props;
use client::Command;
use egui::containers::menu::SubMenu;
use egui::{Align, Align2, Color32, CornerRadius, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::{Ctx, Extension, Manifest, PageSpec};
use local::manage::{Progress, VerifyReport};
use model::{AppId, InstalledGame, art};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use ui::{Theme, icons, widgets as w};

/// Per-game options the user can change.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct GameSettings {
    /// Extra launch arguments.
    args: String,
    /// DLC the user turned off.
    disabled_dlc: BTreeSet<AppId>,
    /// DLC the user turned on after installing (added on the next update).
    enabled_dlc: BTreeSet<AppId>,
    /// Version branch ("" = installed/public).
    branch: String,
    beta_code: String,
    /// 0 always, 1 on launch, 2 high priority.
    auto_update: u8,
    /// 0 global, 1 allow, 2 never.
    background: u8,
    /// 0 default, 1 force on, 2 force off.
    controller: u8,
    #[serde(default = "yes")]
    overlay_on: bool,
    private: bool,
    /// Download language ("" = interface language).
    language: String,
    notes: String,
    /// Steam Cloud turned off for this game (on by default, like Steam).
    cloud_off: bool,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            args: String::new(),
            disabled_dlc: BTreeSet::new(),
            enabled_dlc: BTreeSet::new(),
            branch: String::new(),
            beta_code: String::new(),
            auto_update: 0,
            background: 0,
            controller: 0,
            overlay_on: true,
            private: false,
            language: String::new(),
            notes: String::new(),
            cloud_off: false,
        }
    }
}

fn yes() -> bool {
    true
}

enum Dialog {
    Uninstall,
    /// One-time notice before the first Steam-DRM game launches: it starts
    /// through a background Steam instance. Play confirms and launches.
    DrmNotice { id: AppId, install_dir: PathBuf, args: client::LaunchArgs },
}

/// A Steam-DRM (SteamStub) game: it starts through a background Steam instance
/// (the headless host, or the official client when that can't run), which is what
/// the one-time notice is about.
fn is_steam_drm(mode: &client::LaunchMode) -> bool {
    match mode {
        client::LaunchMode::HeadlessHost { steam_drm } => *steam_drm,
        client::LaunchMode::OfficialSteam { anti_cheat } => anti_cheat == client::STEAM_DRM,
        _ => false,
    }
}

/// Library filters like Steam's: any choice within a group, every group must match.
#[derive(Clone, Default)]
struct AdvFilters {
    /// 0 installed, 1 not installed, 2 played, 3 never played.
    play: BTreeSet<u8>,
    kinds: BTreeSet<String>,
    genres: BTreeSet<u32>,
    features: BTreeSet<u32>,
    deck: BTreeSet<u8>,
}

impl AdvFilters {
    fn count(&self) -> usize {
        self.play.len() + self.kinds.len() + self.genres.len() + self.features.len() + self.deck.len()
    }
}

const GENRES: &[(u32, &str)] = &[
    (1, "Action"), (25, "Adventure"), (4, "Casual"), (23, "Indie"), (29, "Massively Multiplayer"), (9, "Racing"),
    (3, "RPG"), (28, "Simulation"), (18, "Sports"), (2, "Strategy"), (37, "Free to Play"), (70, "Early Access"),
    (56, "Utilities"), (59, "Game Development"), (51, "Animation & Modeling"), (57, "Video Production"),
];

const FEATURES: &[(u32, &str)] = &[
    (2, "Single-player"), (1, "Multi-player"), (9, "Co-op"), (38, "Online co-op"), (24, "Shared/split screen"),
    (28, "Full controller support"), (22, "Steam Achievements"), (23, "Steam Cloud"), (29, "Steam Trading Cards"),
    (30, "Steam Workshop"), (44, "Remote Play Together"), (41, "Remote Play on TV"),
];

/// A named group of games the player made, like Steam's collections.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Collection {
    name: String,
    games: BTreeSet<AppId>,
    /// Set when it came from Steam (its id there), so it stays in step.
    #[serde(default)]
    steam_id: Option<String>,
}

/// A launch held back because saves changed both here and in Steam Cloud.
struct CloudHold {
    app: AppId,
    args: client::LaunchArgs,
    report: model::CloudReport,
}

/// A game added by hand (not from Steam).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct CustomGame {
    name: String,
    exe: String,
    args: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    tile: f32,
    games: HashMap<AppId, GameSettings>,
    favorites: BTreeSet<AppId>,
    hidden: BTreeSet<AppId>,
    custom: Vec<CustomGame>,
    collections: Vec<Collection>,
    /// Hides the collection groups, "+ Collection" and "Add to collection"
    /// (shown by default; Settings → Library).
    hide_collections: bool,
    /// Collapsed sidebar group keys (collections and built-in sections).
    collapsed_groups: BTreeSet<String>,
    /// Collections as Steam last had them, for the account they belong to:
    /// the base of the three-way merge at sign-in.
    collections_base: Option<(u64, client::collections::Collections)>,
    /// Linux: start every game through the official Steam client.
    through_steam: bool,
    /// Kept across restarts so a shortcut still appears if the app was
    /// closed during the download.
    pending_shortcuts: BTreeMap<AppId, std::path::PathBuf>,
}

pub(crate) use news::Reader;

#[derive(Clone, Copy)]
enum MenuAct {
    Play,
    Stop,
    Install,
    PauseDownload,
    ResumeDownload,
    CancelDownload,
    Favorite,
    Hide,
    Shortcut,
    Browse,
    Verify,
    Move,
    Backup,
    Restore,
    Uninstall,
    Store,
    Help,
    Properties,
    ToggleCollection(usize),
    NewCollection,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Filter {
    #[default]
    All,
    Installed,
    Playing,
    Favorites,
    Collection(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CollectionsView {
    /// "Your Collections" overview grid with "+ Create a new collection" + collection cards.
    Overview,
    /// Managing one collection (index into `Library::collections`), with drag-and-drop.
    Detail(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Sort {
    #[default]
    Name,
    Recent,
    Playtime,
    Size,
}

/// Where a game's install stands: one answer used by the list, the game
/// page, the right-click menu and Properties, so they always agree.
#[derive(Clone)]
pub(crate) enum Setup {
    NotInstalled,
    Queued,
    Downloading(model::DownloadStatus),
    Installed,
}

/// A game's main button, see [`Library::main_action`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum MainAction {
    Play,
    Running,
    Update,
    Install,
    Syncing,
    Preparing,
    Moving(f32),
    BackingUp(f32),
    Queued,
    Uninstalling,
    /// Downloading or verifying.
    Busy { fraction: f32, verifying: bool, paused: bool },
    /// Not owned: only the store page.
    StorePage,
}

pub(crate) fn setup_state(ctx: &Ctx, id: AppId, installed: bool) -> Setup {
    if let Some(d) = ctx.state.downloads.get(&id).filter(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)) {
        return Setup::Downloading(d.clone());
    }
    if ctx.state.download_queue.contains(&id) {
        return Setup::Queued;
    }
    if installed { Setup::Installed } else { Setup::NotInstalled }
}

pub type SharedLibraryLayout = Arc<Mutex<Vec<themes::Node>>>;

#[derive(Default)]
pub struct Library {
    /// Declarative library layout tree from the active theme (empty = default Aurora library).
    layout: Option<SharedLibraryLayout>,
    /// This frame's downloads (paused?) and queue, for the right-click menu.
    downloading: HashMap<AppId, bool>,
    queued: BTreeSet<AppId>,
    rt: Option<tokio::runtime::Handle>,
    egui: Option<egui::Context>,
    favorites: BTreeSet<AppId>,
    hidden: BTreeSet<AppId>,
    show_hidden: bool,
    hidden_only: bool,
    soundtracks_only: bool,
    news_view: bool,
    custom: Vec<CustomGame>,
    /// Index into `custom` when a non-Steam game is open.
    custom_selected: Option<usize>,
    adding: Option<CustomGame>,
    /// Steam's non-Steam games being picked for import (shortcut, ticked).
    importing: Option<Vec<(local::shortcuts::Shortcut, bool)>>,
    selected: Option<AppId>,
    search: String,
    filter: Filter,
    sort: Sort,
    tile: f32,
    games: HashMap<AppId, GameSettings>,
    dialog: Option<Dialog>,
    /// Games being uninstalled in the background: (name, progress, result).
    uninstalling: HashMap<AppId, Uninstalling>,
    /// Installed build and branch per game, read from disk now and then.
    builds: HashMap<AppId, (std::time::Instant, u64, String)>,
    reports: HashMap<AppId, VerifyReport>,
    pending: Option<String>,
    events: HashMap<AppId, news::EventsSlot>,
    /// Games whose news and player counts are kept (above the 10 "What's new" reads).
    recent: super::RecentApps,
    /// People playing each game right now: (last asked, count).
    players: HashMap<AppId, (std::time::Instant, news::PlayersSlot)>,
    reader: Option<Reader>,
    props: Option<props::Props>,
    /// A game's reviews, opened from the link row.
    reviews: Option<super::reviews::ReviewWindow>,
    install_dialog: Option<InstallDialog>,
    /// Games to put on the desktop once their install finishes.
    collections: Vec<Collection>,
    hide_collections: bool,
    collapsed_groups: BTreeSet<String>,
    collections_view: Option<CollectionsView>,
    /// Game currently being dragged from the sidebar to drop into a collection.
    dragging_game: Option<(AppId, String)>,
    /// See `Saved::collections_base`.
    collections_base: Option<(u64, client::collections::Collections)>,
    /// Signed-in account (collections belong to it).
    account: Option<u64>,
    /// Steam's collections were read this session, so changes can go up.
    collections_loaded: bool,
    collections_saving: bool,
    /// After a failed save, when to try again.
    collections_retry: Option<std::time::Instant>,
    through_steam: bool,
    adv: AdvFilters,
    /// Naming a new collection (with a game to put in it), or renaming one.
    naming: Option<(Option<usize>, String, Option<AppId>)>,
    /// Launch waiting on a Steam Cloud conflict choice.
    cloud_hold: Option<CloudHold>,
    flags: Option<super::settings::SharedFlags>,
    /// Updates already queued automatically this session.
    auto_queued: BTreeSet<AppId>,
    /// Games whose cloud usage was looked up this session.
    cloud_checked: BTreeSet<AppId>,
    /// Steam Cloud file list shown in Properties for this game.
    cloud_files_open: BTreeSet<AppId>,
    /// Cloud files waiting for "Delete" to be confirmed: (game, raw names, what to call them).
    cloud_delete: Option<(AppId, Vec<String>, String)>,
    /// A right-click menu choice, applied after the frame's drawing.
    menu_request: Option<(AppId, MenuAct)>,
    /// Games to put on the desktop once installed, with their library folder.
    shortcut_after_install: BTreeMap<AppId, std::path::PathBuf>,
    seen_finished: BTreeSet<AppId>,
    storage_dialog: Option<super::storage_ui::StorageDialog>,
}

impl Extension for Library {
    fn manifest(&self) -> Manifest {
        Manifest { id: "library", name: "Library", author: "Aurora", description: "Your games, owned and installed", core: true, ..Manifest::DEFAULT }
    }

    fn pages(&self) -> Vec<PageSpec> {
        vec![PageSpec::new("library", "Library", icons::BOOKS).main().order(20)]
    }

    fn on_navigate(&mut self, _page: &str, arg: Option<&str>) {
        self.custom_selected = None;
        self.collections_view = None;
        self.dialog = None;
        self.reader = None;
        match arg {
            Some("hidden") => {
                self.selected = None;
                self.hidden_only = true;
                self.soundtracks_only = false;
                self.news_view = false;
                return;
            }
            Some("soundtracks") => {
                self.selected = None;
                self.soundtracks_only = true;
                self.hidden_only = false;
                self.news_view = false;
                return;
            }
            Some("news") => {
                self.selected = None;
                self.news_view = true;
                self.hidden_only = false;
                self.soundtracks_only = false;
                return;
            }
            _ => {
                self.hidden_only = false;
                self.soundtracks_only = false;
                self.news_view = false;
            }
        }
        if arg == Some("restore") {
            self.selected = None;
            self.pending = Some("restore".into());
            return;
        }
        // Links can ask for an action: "add", "install:ID", "uninstall:ID", "verify:ID".
        if arg == Some("add") {
            self.adding = Some(CustomGame::default());
            return;
        }
        match arg.and_then(|a| a.split_once(':')) {
            Some((action, id)) => {
                self.selected = id.parse().ok();
                self.pending = Some(action.to_string());
            }
            None => self.selected = arg.and_then(|a| a.parse().ok()),
        }
    }

    fn on_event(&mut self, event: &client::Event, ctx: &mut Ctx) {
        // An uninstall can end while another page is open.
        self.finish_uninstalls(ctx);
        if let Some(r) = &mut self.reviews {
            r.on_event(event, ctx);
        }
        match event {
            client::Event::VerifyFinished { app_id, result } => self.verify_finished(ctx, *app_id, result),
            client::Event::MoveFinished { app_id, result } => {
                let name = ctx
                    .state
                    .app_name(*app_id)
                    .map(String::from)
                    .or_else(|| ctx.local.games.iter().find(|g| g.app_id == *app_id).map(|g| g.name.clone()))
                    .unwrap_or_else(|| app_id.to_string());
                match result {
                    Ok(rep) => {
                        ctx.toast(t!("{name} moved to {path}", name = name, path = rep.to_library.display()));
                        ctx.act(extensions::Action::ReloadLocal);
                    }
                    Err(e) => ctx.toast(i18n::tr_dyn(e)),
                }
            }
            client::Event::BackupFinished { app_id, result } => {
                let name = ctx
                    .state
                    .app_name(*app_id)
                    .map(String::from)
                    .or_else(|| ctx.local.games.iter().find(|g| g.app_id == *app_id).map(|g| g.name.clone()))
                    .unwrap_or_else(|| app_id.to_string());
                match result {
                    Ok(rep) => ctx.toast(t!("{name} backed up to {path}", name = name, path = rep.backup_dir.display())),
                    Err(e) => ctx.toast(i18n::tr_dyn(e)),
                }
            }
            client::Event::RestoreFinished { result } => match result {
                Ok(rep) => {
                    ctx.toast(t!("{name} restored to {path}", name = rep.name, path = rep.library.display()));
                    ctx.act(extensions::Action::ReloadLocal);
                }
                Err(e) => ctx.toast(i18n::tr_dyn(e)),
            },
            client::Event::LoggedIn { steam_id, .. } => {
                self.account = Some(*steam_id);
                self.collections_loaded = false;
                self.send_cloud_settings(ctx);
                ctx.send(Command::LoadSteamCollections);
                // Account-wide play history, for "Recent games".
                ctx.send(Command::LoadPlayerGames { steam_id: *steam_id, language: i18n::language().steam_name().into() });
            }
            client::Event::Apps(_) => {
                self.send_cloud_settings(ctx);
                self.auto_update(ctx);
            }
            client::Event::GameStopped { .. } => self.auto_update(ctx),
            client::Event::RelaunchForSteamDrm { app_id } => {
                let Some(g) = ctx.local.games.iter().find(|g| g.app_id == *app_id).cloned() else { return };
                ctx.toast(t!("{name} uses Steam DRM: starting it again through Steam", name = g.name.clone()));
                let args = self.launch_args_for(*app_id);
                self.launch_or_notice(ctx, *app_id, g.install_dir, args);
            }
            client::Event::SteamCollections(Ok(list)) => self.merge_steam_collections(list),
            client::Event::SteamCollectionsSaved { save, delete, result } => {
                self.collections_saving = false;
                match result {
                    Ok(()) => {
                        self.collections_retry = None;
                        if let Some((_, base)) = &mut self.collections_base {
                            for (id, e) in save {
                                base.insert(id.clone(), e.clone());
                            }
                            for id in delete {
                                base.remove(id);
                            }
                        }
                    }
                    Err(e) => {
                        if self.collections_retry.is_none() {
                            ctx.toast(t!("Couldn't save collections to Steam: {error}", error = e));
                        }
                        self.collections_retry = Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
                    }
                }
            }
            client::Event::CloudReport { report, held_launch } => {
                let name = ctx.state.app_name(report.app_id).unwrap_or_default().to_string();
                if let Some(args) = held_launch {
                    self.cloud_hold = Some(CloudHold { app: report.app_id, args: args.clone(), report: report.clone() });
                } else if report.uploaded > 0 {
                    ctx.toast(t!("{name}: saves uploaded to Steam Cloud", name = name));
                } else if report.downloaded > 0 {
                    ctx.toast(t!("{name}: newer saves downloaded from Steam Cloud", name = name));
                }
                if let Some((_, e)) = report.errors.first() {
                    ctx.toast(t!("Steam Cloud: {error}", error = i18n::tr_dyn(e)));
                }
            }
            _ => {}
        }
        if let client::Event::DownloadFinished { app_id, result } = event {
            {
                self.seen_finished.insert(*app_id);
                match result {
                    Ok(_) => {
                        ctx.toast(t!("{name} is ready to play", name = ctx.state.app_name(*app_id).unwrap_or_default()));
                        ctx.act(extensions::Action::ReloadLocal);
                        self.shortcut_for_finished(ctx, *app_id);
                    }
                    Err(e) => ctx.toast(t!("Download failed: {error}", error = e)),
                }
            }
        }
    }

    fn page(&mut self, _page: &str, ui: &mut Ui, ctx: &mut Ctx) {
        let t = ctx.theme.clone();
        self.finish_uninstalls(ctx);
        if !self.uninstalling.is_empty()
            || !ctx.state.moving.is_empty()
            || !ctx.state.backing_up.is_empty()
            || ctx.state.restoring.is_some()
        {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.downloading = ctx.state.downloads.values().filter(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)).map(|d| (d.app_id, d.paused)).collect();
        self.queued = ctx.state.download_queue.iter().copied().collect();
        if self.tile == 0.0 {
            self.tile = 160.0;
        }
        let custom_layout = self
            .layout
            .as_ref()
            .and_then(|l| l.lock().ok())
            .map(|g| g.clone())
            .unwrap_or_default();
        if self.hide_collections {
            self.collections_view = None;
        }
        if !custom_layout.is_empty() {
            crate::theme_renderer::render_library(self, &custom_layout, ui, &t, ctx);
        } else {
            egui::Panel::left("library-list")
                .exact_size(280.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.chrome).inner_margin(Margin { left: 12, right: 6, top: 14, bottom: 8 }))
                .show(ui, |ui| self.sidebar(ui, &t, ctx));
            egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| match (self.collections_view, self.custom_selected, self.selected) {
                (Some(CollectionsView::Overview), _, _) => self.collections_overview(ui, &t, ctx),
                (Some(CollectionsView::Detail(i)), _, _) => self.collection_detail(i, ui, &t, ctx),
                (None, Some(i), _) => self.custom_page(i, ui, &t, ctx),
                (None, None, Some(id)) => self.game_page(id, ui, &t, ctx),
                (None, None, None) if self.news_view => self.news_feed_page(ui, &t, ctx),
                (None, None, None) if self.hidden_only => self.hidden_games_page(ui, &t, ctx),
                (None, None, None) if self.soundtracks_only => self.soundtracks_page(ui, &t, ctx),
                (None, None, None) => self.home(ui, &t, ctx),
            });
        }
        self.paint_drag_preview(ui, &t, ctx);
        self.add_dialog(ui, &t, ctx);
        self.import_dialog(ui, &t, ctx);
        self.reader(ui, &t, ctx);
        self.install_window(ui, &t, ctx);
        self.properties_window(ui, &t, ctx);
        self.run_pending(ctx);
        self.naming_dialog(ui, &t);
        if let Some(mut dlg) = self.storage_dialog.take()
            && dlg.show(ui, &t, ctx, "library")
        {
            self.storage_dialog = Some(dlg);
        }
        if let Some(r) = &mut self.reviews {
            if !r.show(ui, &t, ctx) {
                self.reviews = None;
            }
        }
        if let Some((id, act)) = self.menu_request.take() {
            self.apply_menu(ctx, id, act);
        }
        self.sync_collections(ctx);
    }

    /// `update:<id>` from the Downloads page: queue the update right away,
    /// with this game's own settings (branch, language, DLC), without
    /// opening its page.
    fn request(&mut self, what: &str, ctx: &mut Ctx) -> bool {
        let Some(id) = what.strip_prefix("update:").and_then(|i| i.parse::<AppId>().ok()) else { return false };
        let library = ctx.local.games.iter().find(|g| g.app_id == id).map(|g| g.library.clone());
        self.queue_install(ctx, id, library);
        true
    }

    fn settings(&mut self, ui: &mut Ui, ctx: &mut Ctx) -> bool {
        let t = ctx.theme.clone();
        let mut show = !self.hide_collections;
        super::settings::switch(ui, &t, t!("Show collections"), t!("Collection groups and the new collection button in the library sidebar, and the Add to collection menu on games."), &mut show);
        self.hide_collections = !show;
        if let Some(flags) = &self.flags {
            let mut f = *flags.lock().unwrap();
            super::settings::switch(ui, &t, t!("Show What's new"), t!("Show recent news from the games you play on the library home."), &mut f.show_whats_new);
            *flags.lock().unwrap() = f;
        }
        // Non-Steam games added in the official client.
        egui::Frame::new().fill(t.surface).corner_radius(t.corner_small()).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(w::text_medium(t!("Non-Steam games from Steam"), 14.0, t.text));
                    ui.add(egui::Label::new(w::text(t!("Adds the non-Steam games you added in the official Steam client (shortcuts.vdf) to Aurora's library."), 12.5, t.text_dim)).wrap());
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::button(ui, &t, Some(icons::DOWNLOAD), t!("Import"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        let picks: Vec<_> = self.steam_shortcuts(ctx).into_iter().filter(|(s, new)| *new && !s.hidden).map(|(s, _)| s).collect();
                        let n = picks.len();
                        self.import_shortcuts(picks);
                        ctx.toast(if n == 0 { t!("No new non-Steam games in Steam").to_string() } else { t!("Imported {n} non-Steam games from Steam", n = n).to_string() });
                    }
                });
            });
        });
        if !cfg!(windows) {
            let before = self.through_steam;
            super::settings::switch(ui, &t, t!("Start games through the official Steam client"), t!("Aurora uses the windowless Steam host by default. Turn this on for games that need the official Steam client."), &mut self.through_steam);
            if self.through_steam != before {
                client::launch_through_steam(self.through_steam);
            }
        }
        true
    }

    fn save(&self) -> Option<String> {
        serde_json::to_string(&Saved {
            tile: self.tile,
            games: self.games.clone(),
            favorites: self.favorites.clone(),
            hidden: self.hidden.clone(),
            custom: self.custom.clone(),
            collections: self.collections.clone(),
            hide_collections: self.hide_collections,
            collapsed_groups: self.collapsed_groups.clone(),
            collections_base: self.collections_base.clone(),
            through_steam: self.through_steam,
            pending_shortcuts: self.shortcut_after_install.clone(),
        })
        .ok()
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        self.finish_uninstalls(ctx);
    }

    fn as_any(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn load(&mut self, data: &str) {
        let saved: Saved = serde_json::from_str(data).unwrap_or_default();
        self.tile = if saved.tile > 0.0 { saved.tile } else { 160.0 };
        self.games = saved.games;
        self.favorites = saved.favorites;
        self.hidden = saved.hidden;
        self.custom = saved.custom;
        self.collections = saved.collections;
        self.hide_collections = saved.hide_collections;
        self.collapsed_groups = saved.collapsed_groups;
        self.collections_base = saved.collections_base;
        self.through_steam = saved.through_steam;
        client::launch_through_steam(self.through_steam);
        self.sync_controller_overrides();
        self.sync_overlay_overrides();
        self.shortcut_after_install = saved.pending_shortcuts;
    }
}

impl Library {
    /// Act on a link request once the library knows the game.
    fn run_pending(&mut self, ctx: &mut Ctx) {
        if self.pending.as_deref() == Some("restore") {
            self.pending = None;
            self.storage_dialog = Some(super::storage_ui::StorageDialog::Restore(super::storage_ui::RestoreDialog::new(ctx)));
            return;
        }
        let (Some(action), Some(id)) = (self.pending.clone(), self.selected) else { return };
        if ctx.state.apps.is_none() && ctx.local.games.iter().all(|g| g.app_id != id) {
            return; // wait for the library to load
        }
        self.pending = None;
        let installed = ctx.local.games.iter().find(|g| g.app_id == id).cloned();
        match (action.as_str(), installed) {
            ("install", None) => self.install_dialog = Some(InstallDialog::new(id, ctx)),
            ("uninstall", Some(_)) => self.dialog = Some(Dialog::Uninstall),
            ("verify", Some(g)) => self.start_verify(ctx, &g),
            ("update", Some(g)) => self.queue_install(ctx, id, Some(g.library.clone())),
            ("props", _) => self.props = Some(props::Props { app: id, tab: props::PropTab::General, ..Default::default() }),
            ("news", _) => self.reader = Some(Reader { app: id, index: 0 }),
            ("reviews", _) => {
                let name = ctx.state.app_name(id).unwrap_or_default().to_string();
                self.reviews = Some(super::reviews::ReviewWindow::new(id, &name, true, false));
            }
            ("resetargs", _) => {
                if let Some(s) = self.games.get_mut(&id) {
                    s.args.clear();
                }
                ctx.toast(t!("Launch options cleared"));
            }
            _ => {}
        }
    }

    /// Tell the client where Steam's `userdata` is and which games have
    /// Steam Cloud turned off.
    pub(super) fn send_cloud_settings(&self, ctx: &Ctx) {
        // Settings → Cloud can turn it off for every game.
        let all_off = self.flags.as_ref().is_some_and(|f| !f.lock().unwrap().cloud);
        let disabled = if all_off {
            ctx.state.apps.as_ref().map(|a| a.keys().copied().collect()).unwrap_or_default()
        } else {
            self.games.iter().filter(|(_, s)| s.cloud_off).map(|(id, _)| *id).collect()
        };
        let root = Some(ctx.local.steam_root.clone()).filter(|p| !p.as_os_str().is_empty());
        ctx.send(Command::CloudSettings { steam_root: root, disabled });
    }

    /// Favorites, Hidden and the collections as Steam ids → contents. Gives
    /// collections made here a Steam id first.
    fn collections_local(&mut self) -> client::collections::Collections {
        use client::collections::{Entry, new_id};
        let mut m = client::collections::Collections::new();
        m.insert("favorite".into(), Entry { name: String::new(), games: self.favorites.clone() });
        m.insert("hidden".into(), Entry { name: String::new(), games: self.hidden.clone() });
        for c in &mut self.collections {
            let id = c.steam_id.get_or_insert_with(new_id).clone();
            m.insert(id, Entry { name: c.name.clone(), games: c.games.clone() });
        }
        m
    }

    /// Bring the collections in step with Steam's at sign-in: a three-way
    /// merge with what both sides last agreed on, so changes made here
    /// (even offline) and in the official client both stay.
    fn merge_steam_collections(&mut self, list: &[model::SteamCollection]) {
        use client::collections::{Collections, Entry, merge};
        let Some(account) = self.account else { return };
        // Dynamic collections are built from filters in Steam: not ours to change.
        let remote: Collections = list
            .iter()
            .filter(|c| !c.dynamic)
            .map(|c| {
                let name = if matches!(c.id.as_str(), "favorite" | "hidden") { String::new() } else { c.name.clone() };
                (c.id.clone(), Entry { name, games: c.games.iter().copied().collect() })
            })
            .collect();
        let local = self.collections_local();
        let base = match &self.collections_base {
            Some((who, base)) if *who == account => base.clone(),
            // First sync for this account: collections already here that
            // came from Steam start from Steam's copy; Favorites and Hidden
            // are joined with Steam's.
            _ => remote.iter().filter(|(id, _)| local.contains_key(*id) && !matches!(id.as_str(), "favorite" | "hidden")).map(|(k, v)| (k.clone(), v.clone())).collect(),
        };
        let merged = merge(&base, &local, &remote);
        self.favorites = merged.get("favorite").map(|e| e.games.clone()).unwrap_or_default();
        self.hidden = merged.get("hidden").map(|e| e.games.clone()).unwrap_or_default();
        // Keep the order on this PC; new ones from Steam go after, by name.
        let mut kept: Vec<Collection> = Vec::new();
        for c in std::mem::take(&mut self.collections) {
            let Some(id) = c.steam_id.clone() else { continue };
            if let Some(e) = merged.get(&id) {
                kept.push(Collection { name: e.name.clone(), games: e.games.clone(), steam_id: Some(id) });
            }
        }
        let mut new: Vec<Collection> = merged
            .iter()
            .filter(|(id, _)| !matches!(id.as_str(), "favorite" | "hidden") && !kept.iter().any(|c| c.steam_id.as_deref() == Some(id.as_str())))
            .map(|(id, e)| Collection { name: e.name.clone(), games: e.games.clone(), steam_id: Some(id.clone()) })
            .collect();
        new.sort_by_key(|c| c.name.to_lowercase());
        kept.extend(new);
        self.collections = kept;
        if let Filter::Collection(i) = self.filter {
            if i >= self.collections.len() {
                self.filter = Filter::All;
            }
        }
        if let Some(CollectionsView::Detail(i)) = self.collections_view {
            if i >= self.collections.len() {
                self.collections_view = Some(CollectionsView::Overview);
            }
        }
        self.collections_base = Some((account, remote));
        self.collections_loaded = true;
        self.collections_retry = None;
    }

    /// Send what changed here since Steam's copy (after the sign-in merge,
    /// while online, one save at a time).
    fn sync_collections(&mut self, ctx: &mut Ctx) {
        if !self.collections_loaded || self.collections_saving || !ctx.state.online() {
            return;
        }
        if self.collections_retry.is_some_and(|at| std::time::Instant::now() < at) {
            return;
        }
        let local = self.collections_local();
        let Some((_, base)) = &self.collections_base else { return };
        let (save, delete) = client::collections::diff(base, &local);
        if save.is_empty() && delete.is_empty() {
            return;
        }
        self.collections_saving = true;
        ctx.send(Command::SaveSteamCollections { save, delete });
    }

    /// Name a new collection, or rename one.
    fn naming_dialog(&mut self, ui: &mut Ui, t: &Theme) {
        let Some((index, name, _)) = &mut self.naming else { return };
        let mut done = None;
        ui::widgets::modal(ui.ctx(), egui::Id::unique("collection-name")).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.label(w::text_bold(if index.is_some() { t!("Rename collection") } else { t!("New collection") }, 18.0, t.text));
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::singleline(name).hint_text(t!("Name, e.g. Co-op with friends")).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0))).with_edit_menu();
            w::autofocus(&r, name.is_empty());
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ok = !name.trim().is_empty();
                let save = ui.add_enabled_ui(ok, |ui| w::button(ui, t, Some(icons::CHECK), t!("Save"), w::Kind::Primary, w::Size::Md)).inner.clicked();
                if ok && (save || enter) {
                    done = Some(true);
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Md).clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                let (index, name, game) = self.naming.take().expect("open");
                let name = name.trim().to_string();
                match index {
                    Some(i) => {
                        if let Some(c) = self.collections.get_mut(i) {
                            c.name = name;
                        }
                    }
                    None => {
                        let from_game = game.is_some();
                        let col = Collection { name, games: game.into_iter().collect(), steam_id: Some(client::collections::new_id()) };
                        self.collapsed_groups.remove(&collection_key(&col));
                        self.collections.push(col);
                        let new_idx = self.collections.len() - 1;
                        if matches!(self.filter, Filter::Collection(_)) {
                            self.filter = Filter::All;
                        }
                        if !from_game {
                            self.selected = None;
                            self.custom_selected = None;
                            self.collections_view = Some(CollectionsView::Detail(new_idx));
                        }
                    }
                }
            }
            Some(false) => self.naming = None,
            None if ui.input(|i| i.key_pressed(egui::Key::Escape)) => self.naming = None,
            None => {}
        }
    }

    /// Saves changed both here and in the cloud: ask which to keep, like Steam.
    fn cloud_conflict_dialog(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(hold) = &self.cloud_hold else { return };
        let app = hold.app;
        let name = ctx.state.app_name(app).unwrap_or_default().to_string();
        let dir = ctx.local.games.iter().find(|g| g.app_id == app).map(|g| g.install_dir.clone());
        let names: Vec<String> = hold.report.conflicts.iter().map(|c| c.name.clone()).collect();
        let mut choice = ui.input(|i| i.key_pressed(egui::Key::Escape)).then_some(0);
        ui::widgets::modal(ui.ctx(), egui::Id::unique(("cloud-conflict", app))).show(ui.ctx(), |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                ui.label(w::text(icons::CLOUD_CHECK, 28.0, t.away));
                ui.vertical(|ui| {
                    ui.label(w::text_bold(t!("Steam Cloud conflict"), 19.0, t.text));
                    ui.label(w::text(&name, 13.5, t.text_dim));
                });
            });
            ui.add_space(6.0);
            ui.add(egui::Label::new(w::text(t!("Your saves on this PC and in Steam Cloud both changed since the last sync. Pick which to keep. The other copy is backed up, not lost."), 13.5, t.text_dim)).wrap());
            ui.add_space(10.0);
            for c in hold.report.conflicts.iter().take(6) {
                w::panel(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let file = c.name.rsplit(['/', '%']).next().unwrap_or(&c.name);
                    ui.label(w::text_semibold(file, 13.5, t.text)).on_hover_text(&c.name);
                    ui.columns(2, |cols| {
                        cols[0].label(w::caption_text(&t, t!("ON THIS PC")));
                        cols[0].label(w::text(format!("{} · {}", ui::date(c.local_time), ui::bytes(c.local_size)), 12.5, t.text));
                        cols[1].label(w::caption_text(&t, t!("IN STEAM CLOUD")));
                        cols[1].label(w::text(format!("{} · {}", ui::date(c.cloud_time), ui::bytes(c.cloud_size)), 12.5, t.text));
                    });
                });
                ui.add_space(4.0);
            }
            if hold.report.conflicts.len() > 6 {
                ui.label(w::text(t!("and {n} more files", n = hold.report.conflicts.len() - 6), 12.5, t.text_faint));
            }
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                if w::button(ui, t, Some(icons::DOWNLOAD), t!("Use the cloud's saves"), w::Kind::Primary, w::Size::Md).clicked() {
                    choice = Some(1);
                }
                if w::button(ui, t, Some(icons::DESKTOP), t!("Keep this PC's saves"), w::Kind::Secondary, w::Size::Md).clicked() {
                    choice = Some(2);
                }
                if w::button(ui, t, Some(icons::PLAY), t!("Play without syncing"), w::Kind::Ghost, w::Size::Md).clicked() {
                    choice = Some(3);
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Md).clicked() {
                    choice = Some(0);
                }
            });
        });
        let Some(choice) = choice else { return };
        let hold = self.cloud_hold.take().expect("checked above");
        let Some(dir) = dir else { return };
        match choice {
            1 => ctx.send(Command::CloudSync { app_id: app, install_dir: dir, mode: client::cloud::Mode::Sync, keep_local: vec![], keep_cloud: names, then_launch: Some(hold.args) }),
            2 => ctx.send(Command::CloudSync { app_id: app, install_dir: dir, mode: client::cloud::Mode::Sync, keep_local: names, keep_cloud: vec![], then_launch: Some(hold.args) }),
            3 => ctx.send(Command::LaunchWithoutCloud { app_id: app, install_dir: dir, args: hold.args }),
            _ => {}
        }
    }

    /// "What's new": the latest update posts from games you play, newest first.
    fn whats_new(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, recent: &[(AppId, &str, u64)]) {
        if !self.whats_new_enabled() {
            return;
        }
        let ids: Vec<_> = recent.iter().map(|g| g.0).collect();
        let (posts, loading) = self.whats_new_posts(&ids);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if posts.is_empty() && !loading {
            return;
        }
        if w::section(ui, t, t!("What's new"), Some(t!("Hide"))) {
            self.set_whats_new_enabled(false);
            ctx.toast(t!("What's new hidden. You can turn it back on in Settings → Library."));
            ui.ctx().request_repaint();
            return;
        }
        // Steam's event images are 16:9 (800x450): a 16:9 slot shows them whole.
        let card = vec2(300.0, 300.0 * 9.0 / 16.0 + 80.0);
        egui::ScrollArea::horizontal().id_salt("whats-new").show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                if posts.is_empty() {
                    for _ in 0..4 {
                        let (r, _) = ui.allocate_exact_size(card, Sense::hover());
                        w::skeleton(ui, t, r, t.corner());
                    }
                }
                for (id, index, e) in posts.iter().take(12) {
                    let (r, resp) = ui.allocate_exact_size(card, Sense::click());
                    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                    let age = now.saturating_sub(e.time) / 86_400;
                    let when = match age {
                        0 => t!("Today").to_string(),
                        1 => t!("Yesterday").to_string(),
                        2..=6 => t!("This week").to_string(),
                        _ => ui::date(e.time),
                    };
                    ui.painter().text(r.left_top(), Align2::LEFT_TOP, when, w::font(12.0), t.text_faint);
                    let img = Rect::from_min_size(r.min + vec2(0.0, 18.0), vec2(card.x, card.x * 9.0 / 16.0));
                    match e.capsule.as_deref().or(e.header.as_deref()) {
                        Some(u) => w::image_or_title(ui, t, img, u, &e.title, t.corner_small()),
                        None => w::image_or_title(ui, t, img, &art::header(*id), &e.title, t.corner_small()),
                    }
                    if hover > 0.0 {
                        ui.painter().rect_stroke(img, t.corner_small(), Stroke::new(2.0, t.accent.gamma_multiply(hover)), egui::StrokeKind::Inside);
                    }
                    let text_rect = Rect::from_min_max(pos2(r.left(), img.bottom() + 6.0), r.max);
                    let mut job = egui::text::LayoutJob::simple(e.title.clone(), w::font_semibold(13.5), t.text, card.x);
                    job.wrap.max_rows = 2;
                    job.wrap.overflow_character = Some('…');
                    let galley = ui.painter().layout_job(job);
                    ui.painter().galley(text_rect.min, galley, t.text);
                    let game = ctx.state.app_name(*id).unwrap_or_default().to_string();
                    let icon = Rect::from_min_size(pos2(r.left(), r.bottom() - 18.0), vec2(16.0, 16.0));
                    let ok = ctx.state.apps.as_ref().and_then(|a| a.get(id)).and_then(|o| o.icon.as_deref()).is_some_and(|h| w::paint_cover(ui, icon, &art::icon(*id, h), CornerRadius::same(3)));
                    let x = if ok { icon.right() + 6.0 } else { r.left() };
                    ui.painter().text(pos2(x, icon.center().y), Align2::LEFT_CENTER, game, w::font(12.0), t.text_dim);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.reader = Some(Reader { app: *id, index: *index });
                    }
                }
            });
        });
        ui.add_space(26.0);
    }

    /// Right-click menu for a game anywhere in the library, like Steam's.
    fn game_menu(&mut self, t: &Theme, ctx: &Ctx, resp: &egui::Response, id: AppId, installed: bool, running: bool) {
        let fav = self.favorites.contains(&id);
        let hid = self.hidden.contains(&id);
        let pick = std::cell::Cell::new(None);
        resp.context_menu(|ui| {
            ui.set_min_width(200.0);
            w::menu_spacing(ui);
            let item = |ui: &mut Ui, icon: &str, label: &str, act: MenuAct| {
                if w::menu_item(ui, t, icon, label).clicked() {
                    pick.set(Some(act));
                    ui.close();
                }
            };
            if running {
                item(ui, icons::X, t!("Stop"), MenuAct::Stop);
            } else if let Some(paused) = self.downloading.get(&id) {
                if *paused {
                    item(ui, icons::PLAY, t!("Resume download"), MenuAct::ResumeDownload);
                } else {
                    item(ui, "\u{E39E}", t!("Pause download"), MenuAct::PauseDownload);
                }
                item(ui, icons::X, t!("Cancel download"), MenuAct::CancelDownload);
            } else if self.queued.contains(&id) {
                item(ui, icons::X, t!("Remove from download queue"), MenuAct::CancelDownload);
            } else if installed {
                item(ui, icons::PLAY, t!("Play"), MenuAct::Play);
            } else {
                item(ui, icons::DOWNLOAD, t!("Install"), MenuAct::Install);
            }
            w::menu_separator(ui, t);
            item(ui, icons::STAR, if fav { t!("Remove from favorites") } else { t!("Add to favorites") }, MenuAct::Favorite);
            item(ui, icons::EYE, if hid { t!("Unhide") } else { t!("Hide this game") }, MenuAct::Hide);
            if installed {
                let manage = w::submenu_item(ui, t, icons::GEAR, t!("Manage"));
                SubMenu::new().show(ui, &manage, |ui| {
                    ui.set_min_width(200.0);
                    w::menu_spacing(ui);
                    item(ui, icons::DESKTOP, t!("Add desktop shortcut"), MenuAct::Shortcut);
                    item(ui, icons::FOLDER, t!("Browse local files"), MenuAct::Browse);
                    item(ui, icons::SHIELD, t!("Verify game files"), MenuAct::Verify);
                    item(ui, icons::DRIVES, t!("Move install folder…"), MenuAct::Move);
                    item(ui, icons::PACKAGE, t!("Back up game files…"), MenuAct::Backup);
                    item(ui, icons::RESTORE, t!("Restore game backup…"), MenuAct::Restore);
                    #[cfg(not(windows))]
                    proton_submenu(ui, t, ctx, id);
                    w::menu_separator(ui, t);
                    if w::menu_item_tinted(ui, t, icons::TRASH, t!("Uninstall…"), Some(t.danger)).clicked() {
                        pick.set(Some(MenuAct::Uninstall));
                        ui.close();
                    }
                });
            } else {
                item(ui, icons::RESTORE, t!("Restore game backup…"), MenuAct::Restore);
            }
            if !self.hide_collections {
                let add = w::submenu_item(ui, t, icons::STACK, t!("Add to collection"));
                SubMenu::new().show(ui, &add, |ui| {
                    ui.set_min_width(200.0);
                    w::menu_spacing(ui);
                    for (i, c) in self.collections.iter().enumerate() {
                        let on = c.games.contains(&id);
                        if w::menu_item(ui, t, if on { icons::CHECK } else { "" }, &c.name).clicked() {
                            pick.set(Some(MenuAct::ToggleCollection(i)));
                            ui.close();
                        }
                    }
                    if !self.collections.is_empty() {
                        w::menu_separator(ui, t);
                    }
                    item(ui, icons::PLUS, t!("New collection…"), MenuAct::NewCollection);
                });
            }
            w::menu_separator(ui, t);
            item(ui, icons::STORE, t!("Store page"), MenuAct::Store);
            item(ui, icons::INFO, t!("Get help with this game"), MenuAct::Help);
            item(ui, icons::SLIDERS, t!("Properties…"), MenuAct::Properties);
        });
        if let Some(act) = pick.get() {
            self.menu_request = Some((id, act));
        }
    }

    fn apply_menu(&mut self, ctx: &mut Ctx, id: AppId, act: MenuAct) {
        let local = ctx.local.games.iter().find(|g| g.app_id == id).cloned();
        match act {
            MenuAct::Play => {
                if let Some(g) = local {
                    let args = self.launch_args_for(id);
                    self.launch_or_notice(ctx, id, g.install_dir.clone(), args);
                }
            }
            MenuAct::Stop => ctx.send(Command::StopGame { app_id: id }),
            MenuAct::PauseDownload => ctx.send(Command::PauseDownload { app_id: id }),
            MenuAct::ResumeDownload => ctx.send(Command::ResumeDownload { app_id: id }),
            MenuAct::CancelDownload => ctx.send(Command::CancelDownload { app_id: id }),
            MenuAct::Install => self.install_dialog = Some(InstallDialog::new(id, ctx)),
            MenuAct::Favorite => {
                if !self.favorites.remove(&id) {
                    self.favorites.insert(id);
                }
            }
            MenuAct::Hide => {
                if !self.hidden.remove(&id) {
                    self.hidden.insert(id);
                }
            }
            MenuAct::Shortcut => {
                if let Some(g) = local {
                    self.add_shortcut(ctx, id, &g);
                }
            }
            MenuAct::Browse => {
                if let Some(g) = local {
                    local::open_folder(&g.install_dir);
                }
            }
            MenuAct::Verify => {
                if let Some(g) = local {
                    self.start_verify(ctx, &g);
                }
            }
            MenuAct::Move => {
                if let Some(g) = local {
                    let name = ctx.state.app_name(id).map(String::from).unwrap_or_else(|| g.name.clone());
                    self.storage_dialog = Some(super::storage_ui::StorageDialog::Move(super::storage_ui::MoveDialog::new(g, name, ctx)));
                }
            }
            MenuAct::Backup => {
                if let Some(g) = local {
                    let name = ctx.state.app_name(id).map(String::from).unwrap_or_else(|| g.name.clone());
                    self.storage_dialog = Some(super::storage_ui::StorageDialog::Backup(super::storage_ui::BackupDialog::new(g, name)));
                }
            }
            MenuAct::Restore => {
                self.storage_dialog = Some(super::storage_ui::StorageDialog::Restore(super::storage_ui::RestoreDialog::new(ctx)));
            }
            MenuAct::Uninstall => {
                self.selected = Some(id);
                self.custom_selected = None;
                self.dialog = Some(Dialog::Uninstall);
            }
            MenuAct::Store => ctx.navigate("store", Some(format!("app:{id}"))),
            MenuAct::Help => ctx.navigate("support", Some(format!("game:{id}"))),
            MenuAct::Properties => self.props = Some(props::Props { app: id, tab: props::PropTab::General, ..Default::default() }),
            MenuAct::ToggleCollection(i) => {
                if let Some(c) = self.collections.get_mut(i) {
                    if !c.games.remove(&id) {
                        c.games.insert(id);
                        ctx.toast(t!("Added to {name}", name = c.name));
                    }
                }
            }
            MenuAct::NewCollection => self.naming = Some((None, String::new(), Some(id))),
        }
    }

    /// Put a just-installed game on the desktop, if the install asked for it.
    /// Runs from the download event, so it works whatever page is open.
    fn shortcut_for_finished(&mut self, ctx: &mut Ctx, id: AppId) {
        let Some(library) = self.shortcut_after_install.remove(&id) else { return };
        let owned = ctx.state.apps.as_ref().and_then(|a| a.get(&id));
        let name = owned.map(|o| o.name.clone()).or_else(|| ctx.local.games.iter().find(|g| g.app_id == id).map(|g| g.name.clone())).unwrap_or_else(|| id.to_string());
        let folder = owned.map(|o| o.install_dir.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| name.clone());
        let dir = library.join("steamapps").join("common").join(folder);
        match crate::links::desktop_shortcut(id, &name, crate::links::game_icon(owned, &dir)) {
            Ok(_) => ctx.toast(t!("Shortcut added to your desktop")),
            Err(e) => ctx.toast(t!("Couldn't add a desktop shortcut: {error}", error = e)),
        }
    }

    pub fn new(rt: tokio::runtime::Handle, egui: egui::Context, flags: super::settings::SharedFlags, layout: SharedLibraryLayout) -> Self {
        Self { rt: Some(rt), egui: Some(egui), tile: 160.0, flags: Some(flags), layout: Some(layout), ..Default::default() }
    }

    /// Settings > Library > "Show What's new" (on unless turned off).
    fn whats_new_enabled(&self) -> bool {
        self.flags.as_ref().is_none_or(|f| f.lock().unwrap().show_whats_new)
    }

    fn set_whats_new_enabled(&self, enabled: bool) {
        if let Some(flags) = &self.flags {
            flags.lock().unwrap().show_whats_new = enabled;
        }
    }

    /// Like Steam: queue games that need an update, unless auto-updates are
    /// off (globally, or "only when I launch it" for the game).
    fn auto_update(&mut self, ctx: &mut Ctx) {
        let on = self.flags.as_ref().is_some_and(|f| f.lock().unwrap().auto_updates);
        if !on || ctx.state.apps.is_none() {
            return;
        }
        let due: Vec<(AppId, std::path::PathBuf)> = ctx
            .local
            .games
            .iter()
            .filter(|g| g.state == model::InstallState::UpdateRequired)
            .filter(|g| self.games.get(&g.app_id).is_none_or(|s| s.auto_update != 1))
            .filter(|g| !ctx.state.downloads.contains_key(&g.app_id) && !ctx.state.download_queue.contains(&g.app_id))
            .filter(|g| self.auto_queued.insert(g.app_id))
            .map(|g| (g.app_id, g.library.clone()))
            .collect();
        for (id, lib) in due {
            self.queue_install(ctx, id, Some(lib));
        }
    }

    fn filtered<'a>(&self, ctx: &'a Ctx) -> Vec<Entry<'a>> {
        let mut items = entries(ctx.state, ctx.local);
        let q = self.search.to_lowercase();
        let mut playtime = HashMap::new();
        if self.adv.play.iter().any(|p| *p >= 2)
            && let Some(Ok(games)) = ctx.state.player_games.get(&ctx.state.steam_id)
        {
            playtime.reserve(games.len());
            for game in games {
                playtime.entry(game.app_id).or_insert(game.playtime_minutes);
            }
        }
        items.retain(|e| {
            (model::contains_ignore_case(e.name, &q))
                && match self.filter {
                    Filter::All => true,
                    Filter::Installed => e.installed.is_some(),
                    Filter::Playing => e.installed.and_then(|g| g.last_played).is_some(),
                    Filter::Favorites => self.favorites.contains(&e.app_id),
                    Filter::Collection(i) => self.collections.get(i).is_some_and(|c| c.games.contains(&e.app_id)),
                }
                && if self.hidden_only {
                    self.hidden.contains(&e.app_id)
                } else {
                    self.show_hidden || !self.hidden.contains(&e.app_id)
                }
                && self.adv_match(e, &playtime)
        });
        match self.sort {
            Sort::Name => items.sort_by(|a, b| model::cmp_ignore_case(a.name, b.name)),
            Sort::Recent => items.sort_by_key(|e| std::cmp::Reverse(e.installed.and_then(|g| g.last_played))),
            Sort::Playtime => items.sort_by_key(|e| std::cmp::Reverse(e.installed.and_then(|g| g.playtime_minutes))),
            Sort::Size => items.sort_by_key(|e| std::cmp::Reverse(e.installed.map(|g| g.size_on_disk))),
        }
        items
    }

    /// Whether a game passes the advanced filters.
    fn adv_match(&self, e: &Entry, playtime: &HashMap<AppId, u64>) -> bool {
        let f = &self.adv;
        let o = e.owned;
        // Like Steam, the library lists games unless other types are picked
        // in the filters: no dedicated servers, SDKs, tools or soundtracks.
        // (Games without app info, e.g. non-Steam ones, always show.)
        if self.soundtracks_only {
            if !o.is_some_and(|o| o.kind.eq_ignore_ascii_case("music")) {
                return false;
            }
        } else if f.kinds.is_empty() && o.is_some_and(|o| !is_game_kind(&o.kind)) {
            return false;
        }
        if f.count() == 0 {
            return true;
        }
        let minutes = e
            .installed
            .and_then(|g| g.playtime_minutes)
            .or_else(|| playtime.get(&e.app_id).copied())
            .unwrap_or(0);
        let play_ok = f.play.is_empty()
            || f.play.iter().any(|p| match p {
                0 => e.installed.is_some(),
                1 => e.installed.is_none(),
                2 => minutes > 0,
                _ => minutes == 0,
            });
        let kind_ok = self.soundtracks_only || f.kinds.is_empty() || o.is_some_and(|o| f.kinds.contains(&o.kind));
        let genre_ok = f.genres.is_empty() || o.is_some_and(|o| o.genres.iter().any(|g| f.genres.contains(g)));
        let feature_ok = f.features.is_empty() || o.is_some_and(|o| f.features.iter().all(|c| o.categories.contains(c)));
        let deck_ok = f.deck.is_empty() || f.deck.contains(&o.map(|o| o.deck).unwrap_or(0));
        play_ok && kind_ok && genre_ok && feature_ok && deck_ok
    }

    /// The filter popup, like Steam's library filters.
    fn adv_panel(&mut self, ui: &mut Ui, t: &Theme, ctx: &Ctx) {
        // Only offer what this library has.
        let apps: Vec<&model::OwnedApp> = ctx.state.apps.as_ref().map(|a| a.values().filter(|a| !a.is_dlc()).collect()).unwrap_or_default();
        let has_genre = |g: u32| apps.iter().any(|a| a.genres.contains(&g));
        let has_feature = |c: u32| apps.iter().any(|a| a.categories.contains(&c));
        let mut kinds: Vec<String> = apps.iter().map(|a| a.kind.clone()).filter(|k| !k.is_empty()).collect();
        kinds.sort();
        kinds.dedup();
        let group = |ui: &mut Ui, title: &str| {
            ui.add_space(6.0);
            ui.label(w::caption_text(t, title));
        };
        let f = &mut self.adv;
        ui.horizontal(|ui| {
            ui.label(w::text_bold(t!("Filters"), 15.0, t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if f.count() > 0 && ui.link(w::text(t!("Clear all"), 12.5, t.accent)).clicked() {
                    *f = AdvFilters::default();
                }
            });
        });
        // Checkboxes in columns, compact enough to fit the window.
        {
            group(ui, t!("Play state"));
            let play = [(0u8, t!("Ready to play")), (1, t!("Not installed")), (2, t!("Played")), (3, t!("Never played"))];
            if let Some(i) = check_grid(ui, t, "f-play", 4, play.iter().map(|(v, l)| (f.play.contains(v), l.to_string()))) {
                let v = play[i].0;
                if !f.play.remove(&v) {
                    f.play.insert(v);
                }
            }
            group(ui, t!("Type"));
            let kind_label = |k: &str| match k {
                "game" => t!("Games").to_string(),
                "application" => t!("Software").to_string(),
                "tool" => t!("Tools").to_string(),
                "demo" => t!("Demos").to_string(),
                "music" => t!("Soundtracks").to_string(),
                other => other.to_string(),
            };
            if let Some(i) = check_grid(ui, t, "f-kind", 5, kinds.iter().map(|k| (f.kinds.contains(k), kind_label(k)))) {
                let k = &kinds[i];
                if !f.kinds.remove(k) {
                    f.kinds.insert(k.clone());
                }
            }
            group(ui, t!("Genre"));
            let genres: Vec<&(u32, &str)> = GENRES.iter().filter(|(g, _)| has_genre(*g)).collect();
            if let Some(i) = check_grid(ui, t, "f-genre", 3, genres.iter().map(|(g, l)| (f.genres.contains(g), i18n::tr_dyn(l).to_string()))) {
                let g = genres[i].0;
                if !f.genres.remove(&g) {
                    f.genres.insert(g);
                }
            }
            group(ui, t!("Features"));
            let features: Vec<&(u32, &str)> = FEATURES.iter().filter(|(c, _)| has_feature(*c)).collect();
            if let Some(i) = check_grid(ui, t, "f-feature", 3, features.iter().map(|(c, l)| (f.features.contains(c), i18n::tr_dyn(l).to_string()))) {
                let c = features[i].0;
                if !f.features.remove(&c) {
                    f.features.insert(c);
                }
            }
            group(ui, t!("Steam Deck"));
            let deck = [(3u8, t!("Verified")), (2, t!("Playable")), (1, t!("Unsupported")), (0, t!("Unknown"))];
            if let Some(i) = check_grid(ui, t, "f-deck", 4, deck.iter().map(|(v, l)| (f.deck.contains(v), l.to_string()))) {
                let v = deck[i].0;
                if !f.deck.remove(&v) {
                    f.deck.insert(v);
                }
            }
        }
    }

    fn sidebar_groups<'a>(
        &self,
        items: &'a [Entry<'a>],
        ctx: &Ctx,
        group_by: themes::LibraryGrouping,
        rest_label: &str,
        use_collections: bool,
    ) -> Vec<SidebarGroup<'a>> {
        let history = if group_by == themes::LibraryGrouping::LastPlayed {
            Some(themed_recent_games(ctx).into_iter().map(|g| (g.0, g.2)).collect())
        } else {
            None
        };
        self.sidebar_groups_with_history(items, history.as_ref(), group_by, rest_label, use_collections)
    }

    fn sidebar_groups_with_history<'a>(
        &self,
        items: &'a [Entry<'a>],
        history: Option<&BTreeMap<AppId, u64>>,
        group_by: themes::LibraryGrouping,
        rest_label: &str,
        use_collections: bool,
    ) -> Vec<SidebarGroup<'a>> {
        let mut groups: Vec<SidebarGroup<'a>> = Vec::new();
        if let Filter::Collection(i) = self.filter {
            let (key, label) = self
                .collections
                .get(i)
                .map(|c| (collection_key(c), c.name.clone()))
                .unwrap_or_else(|| (format!("col-idx:{i}"), String::new()));
            groups.push(SidebarGroup { key, label, collection: Some(i), games: items.iter().collect() });
            return groups;
        }
        if self.filter == Filter::Favorites {
            if !items.is_empty() {
                groups.push(SidebarGroup {
                    key: "sys:favorites".into(),
                    label: t!("Favorites").to_string(),
                    collection: None,
                    games: items.iter().collect(),
                });
            }
            return groups;
        }
        let favs: Vec<&Entry> = items.iter().filter(|e| self.favorites.contains(&e.app_id)).collect();
        if !favs.is_empty() {
            groups.push(SidebarGroup {
                key: "sys:favorites".into(),
                label: t!("Favorites").to_string(),
                collection: None,
                games: favs,
            });
        }
        if group_by == themes::LibraryGrouping::LastPlayed {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let year = ui::calendar_date(now).0;
            let mut dated: BTreeMap<PlayedGroup, Vec<&Entry>> = BTreeMap::new();
            for e in items.iter().filter(|e| !self.favorites.contains(&e.app_id)) {
                let played = history.and_then(|h| h.get(&e.app_id).copied());
                dated.entry(played_group(played, now)).or_default().push(e);
            }
            for (group, mut games) in dated {
                games.sort_by_key(|e| (std::cmp::Reverse(history.and_then(|h| h.get(&e.app_id).copied()).unwrap_or(0)), e.name));
                let label = group.label(year);
                groups.push(SidebarGroup { key: format!("played:{label}"), label, collection: None, games });
            }
            return groups;
        }
        if use_collections && !self.collections.is_empty() {
            let filtering = !self.search.trim().is_empty() || self.filter != Filter::All || self.adv.count() > 0;
            let mut order: Vec<usize> = (0..self.collections.len()).collect();
            order.sort_by(|&a, &b| self.collections[a].name.to_lowercase().cmp(&self.collections[b].name.to_lowercase()).then(a.cmp(&b)));
            let mut categorized: HashSet<AppId> = HashSet::new();
            for i in order {
                let c = &self.collections[i];
                let games: Vec<&Entry> = items.iter().filter(|e| c.games.contains(&e.app_id)).collect();
                for e in &games {
                    categorized.insert(e.app_id);
                }
                if !games.is_empty() || !filtering {
                    groups.push(SidebarGroup {
                        key: collection_key(c),
                        label: c.name.clone(),
                        collection: Some(i),
                        games,
                    });
                }
            }
            let uncategorized: Vec<&Entry> = items.iter().filter(|e| !self.favorites.contains(&e.app_id) && !categorized.contains(&e.app_id)).collect();
            if !uncategorized.is_empty() {
                groups.push(SidebarGroup {
                    key: "sys:uncategorized".into(),
                    label: t!("Uncategorized").to_string(),
                    collection: None,
                    games: uncategorized,
                });
            }
            return groups;
        }
        let (installed, rest): (Vec<_>, Vec<_>) = items.iter().filter(|e| !self.favorites.contains(&e.app_id)).partition(|e| e.installed.is_some());
        if !installed.is_empty() {
            groups.push(SidebarGroup {
                key: "sys:installed".into(),
                label: t!("Installed").to_string(),
                collection: None,
                games: installed,
            });
        }
        if !rest.is_empty() {
            groups.push(SidebarGroup {
                key: "sys:rest".into(),
                label: rest_label.to_string(),
                collection: None,
                games: rest,
            });
        }
        groups
    }

    fn search_group_id(&self, ui: &Ui, key: &str) -> egui::Id {
        ui.make_persistent_id(("library-search-group", self.search.trim(), key))
    }

    fn is_group_open(&self, ui: &Ui, key: &str, searching: bool) -> bool {
        if searching {
            // Search results start expanded without losing the normal list's collapse choices.
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), self.search_group_id(ui, key), true).is_open()
        } else {
            !self.collapsed_groups.contains(key)
        }
    }

    fn toggle_group(&mut self, ui: &Ui, key: &str, searching: bool) {
        if searching {
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), self.search_group_id(ui, key), true);
            state.toggle(ui);
            state.store(ui.ctx());
        } else if !self.collapsed_groups.remove(key) {
            self.collapsed_groups.insert(key.to_string());
        }
    }

    fn set_all_groups_open(&mut self, ui: &Ui, keys: &[String], open: bool, searching: bool) {
        if searching {
            for key in keys {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), self.search_group_id(ui, key), true);
                state.set_open(open);
                state.store(ui.ctx());
            }
        } else if open {
            for key in keys {
                self.collapsed_groups.remove(key);
            }
        } else {
            for key in keys {
                self.collapsed_groups.insert(key.clone());
            }
        }
    }

    fn delete_collection(&mut self, index: usize) {
        if index >= self.collections.len() {
            return;
        }
        let removed = self.collections.remove(index);
        self.collapsed_groups.remove(&collection_key(&removed));
        match self.filter {
            Filter::Collection(j) if j == index => self.filter = Filter::All,
            Filter::Collection(j) if j > index => self.filter = Filter::Collection(j - 1),
            _ => {}
        }
        match self.collections_view {
            Some(CollectionsView::Detail(j)) if j == index => self.collections_view = Some(CollectionsView::Overview),
            Some(CollectionsView::Detail(j)) if j > index => self.collections_view = Some(CollectionsView::Detail(j - 1)),
            _ => {}
        }
    }

    fn return_home(&mut self) {
        self.selected = None;
        self.custom_selected = None;
        self.collections_view = None;
        self.hidden_only = false;
        self.soundtracks_only = false;
        self.news_view = false;
        if matches!(self.filter, Filter::Collection(_)) {
            self.filter = Filter::All;
        }
    }

    fn toggle_collections_overview(&mut self) {
        if self.hide_collections {
            return;
        }
        self.selected = None;
        self.custom_selected = None;
        self.collections_view = if self.collections_view == Some(CollectionsView::Overview) {
            None
        } else {
            Some(CollectionsView::Overview)
        };
    }

    fn add_game_to_collection(&mut self, index: usize, app_id: AppId, ctx: &mut Ctx) {
        let Some(c) = self.collections.get_mut(index) else { return };
        if c.games.insert(app_id) {
            ctx.toast(t!("Added to {name}", name = c.name));
        }
    }

    /// Check whether a game dragged from the sidebar is hovering or dropped on `rect`.
    fn check_drop_on_rect(&mut self, ui: &Ui, rect: Rect) -> (bool, Option<AppId>) {
        if self.dragging_game.is_none() {
            return (false, None);
        }
        let hovered = ui.ctx().pointer_latest_pos().is_some_and(|p| rect.contains(p));
        let released = !ui.input(|i| i.pointer.any_down());
        if hovered && released {
            (true, self.dragging_game.take().map(|(id, _)| id))
        } else {
            (hovered, None)
        }
    }

    /// Floating pill following the pointer while dragging a game from the sidebar.
    fn paint_drag_preview(&mut self, ui: &Ui, t: &Theme, ctx: &Ctx) {
        let Some((app_id, name)) = self.dragging_game.clone() else { return };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) || !ui.input(|i| i.pointer.any_down()) {
            self.dragging_game = None;
            return;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let Some(pos) = ui.ctx().pointer_latest_pos() else { return };
        let icon_url = ctx.state.apps.as_ref().and_then(|a| a.get(&app_id)).and_then(|o| o.icon.as_deref()).map(|h| art::icon(app_id, h));
        let header = art::header(app_id);
        egui::Area::new(egui::Id::unique("library-drag-game-preview"))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .fixed_pos(pos + vec2(14.0, 12.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(t.raised)
                    .stroke(Stroke::new(1.5, t.accent))
                    .corner_radius(t.corner_small())
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let (icon_rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                            let urls: Vec<&str> = icon_url.as_deref().into_iter().chain([header.as_str()]).collect();
                            if !w::paint_cover_first(ui, icon_rect, &urls, CornerRadius::same(3)) {
                                ui.painter().rect_filled(icon_rect, CornerRadius::same(3), t.surface_hover);
                            }
                            ui.label(w::text_medium(short(&name, 32), 13.0, t.text));
                        });
                    });
            });
    }

    fn apply_group_menu(&mut self, ui: &Ui, act: Option<GroupMenuAct>, all_keys: &[String], searching: bool) {
        match act {
            Some(GroupMenuAct::Rename(i)) => {
                if let Some(c) = self.collections.get(i) {
                    self.naming = Some((Some(i), c.name.clone(), None));
                }
            }
            Some(GroupMenuAct::Delete(i)) => self.delete_collection(i),
            Some(GroupMenuAct::CollapseAll) => self.set_all_groups_open(ui, all_keys, false, searching),
            Some(GroupMenuAct::ExpandAll) => self.set_all_groups_open(ui, all_keys, true, searching),
            None => {}
        }
    }

    fn sidebar(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        ui.horizontal(|ui| {
            let home = home_button(
                ui,
                t,
                self.selected.is_none() && self.custom_selected.is_none() && self.collections_view.is_none(),
                ui.available_width() - 46.0,
            );
            if home.clicked() {
                self.return_home();
            }
            if w::icon_button(ui, t, icons::PLUS, t!("Add a non-Steam game"), false, 0).clicked() {
                self.adding = Some(CustomGame::default());
            }
        });
        ui.add_space(8.0);
        if self.hidden_only || self.soundtracks_only {
            let label = if self.hidden_only { t!("Showing: Hidden Games") } else { t!("Showing: Soundtracks") };
            ui.horizontal(|ui| {
                ui.label(w::text_medium(label, 12.0, t.accent));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_button(ui, t, icons::X, t!("Back to Library"), false, 0).clicked() {
                        self.return_home();
                    }
                });
            });
            ui.add_space(4.0);
        }
        let width = ui.available_width() - 6.0;
        w::search(ui, t, &mut self.search, t!("Search games"), width);
        ui.add_space(6.0);
        // The chips fill the search box's width (the Filters + Collections row
        // below aligns its split with the Installed / ★ split above).
        let chips = [(Filter::All, t!("All")), (Filter::Installed, t!("Installed")), (Filter::Favorites, icons::STAR), (Filter::Playing, t!("Played"))];
        let gap = 6.0;
        // Collections (only when turned on in Settings → Library).
        if self.hide_collections && matches!(self.filter, Filter::Collection(_)) {
            self.filter = Filter::All;
        }
        let n = self.adv.count();
        let filters_label = if n > 0 { format!("{}  {} ({n})", icons::FUNNEL, t!("Filters")) } else { format!("{}  {}", icons::FUNNEL, t!("Filters")) };
        let collections_label = format!("{}  {}", icons::GRID, t!("Collections"));
        let top_nat = [
            w::chip_width(ui, chips[0].1),
            w::chip_width(ui, chips[1].1),
            w::chip_width(ui, chips[2].1),
            w::chip_width(ui, chips[3].1),
        ];
        let bottom_nat = (!self.hide_collections).then(|| [w::chip_width(ui, &filters_label), w::chip_width(ui, &collections_label)]);
        let (top_widths, bottom_widths) = sidebar_filter_widths(top_nat, bottom_nat, width, gap);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            for (i, (f, label)) in chips.into_iter().enumerate() {
                let r = if let Some(tw) = top_widths {
                    w::wide_chip(ui, t, label, self.filter == f, tw[i])
                } else {
                    w::chip(ui, t, label, self.filter == f)
                };
                if r.clicked() {
                    self.filter = f;
                }
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
        // Steam-style filters in a popup, paired with clickable "Collections" button when collections are on.
        let filters_resp = if self.hide_collections {
            w::wide_chip(ui, t, &filters_label, n > 0, width)
        } else if let Some([fw, cw]) = bottom_widths {
            let mut f_resp = None;
            let mut c_resp = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                f_resp = Some(w::wide_chip(ui, t, &filters_label, n > 0, fw));
                c_resp = Some(w::wide_chip(ui, t, &collections_label, self.collections_view.is_some(), cw));
            });
            if c_resp.unwrap().on_hover_text(t!("Manage your collections")).clicked() {
                self.toggle_collections_overview();
            }
            f_resp.unwrap()
        } else {
            let mut row = Vec::new();
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                let labels = [
                    (filters_label.clone(), n > 0),
                    (collections_label.clone(), self.collections_view.is_some()),
                ];
                row = chip_rows(ui, t, &labels, width, gap);
            });
            if row[1].clone().on_hover_text(t!("Manage your collections")).clicked() {
                self.toggle_collections_overview();
            }
            row.swap_remove(0)
        };
        egui::Popup::from_toggle_button_response(&filters_resp)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(720.0)
            .show(|ui| self.adv_panel(ui, t, ctx));
        ui.add_space(8.0);
        let items = self.filtered(ctx);
        let searching = !self.search.trim().is_empty();
        let groups = self.sidebar_groups(&items, ctx, themes::LibraryGrouping::Installed, t!("Not installed"), !self.hide_collections);
        let mut all_keys: Vec<String> = groups.iter().map(|g| g.key.clone()).collect();
        if !self.custom.is_empty() {
            all_keys.push("sys:non_steam".into());
        }
        let total_groups = all_keys.len();
        let mut group_act: Option<GroupMenuAct> = None;
        let mut drop_add: Option<(usize, AppId)> = None;
        if ctx.state.apps.is_none() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0).color(t.accent));
                ui.label(w::text(t!("Loading owned games…"), 12.0, t.text_faint));
            });
        }
        egui::ScrollArea::vertical().id_salt("lib-side").auto_shrink([false, false]).show(ui, |ui| {
            if items.is_empty() && ctx.state.apps.is_none() {
                for i in 0..10 {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width() - 6.0, 30.0), Sense::hover());
                    w::skeleton(ui, t, Rect::from_min_size(r.min + vec2(6.0, 3.0), vec2(24.0, 24.0)), CornerRadius::same(4));
                    w::skeleton(ui, t, Rect::from_min_size(r.min + vec2(40.0, 9.0), vec2(80.0 + (i * 37 % 90) as f32, 12.0)), CornerRadius::same(4));
                }
            }
            for group in groups {
                let mut open = self.is_group_open(ui, &group.key, searching);
                let hresp = sidebar_group_header(ui, t, &group.label, group.games.len(), open, ui.available_width() - 6.0);
                if let Some(col_idx) = group.collection {
                    let (drop_hover, dropped) = self.check_drop_on_rect(ui, hresp.rect);
                    if drop_hover {
                        ui.painter().rect_filled(hresp.rect, t.corner_small(), t.accent.gamma_multiply(0.2));
                        ui.painter().rect_stroke(hresp.rect, t.corner_small(), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                    }
                    if let Some(app_id) = dropped {
                        drop_add = Some((col_idx, app_id));
                    }
                }
                if hresp.clicked() {
                    self.toggle_group(ui, &group.key, searching);
                    open = !open;
                }
                group_context_menu(&hresp, group.collection, total_groups, &mut group_act);
                if !open {
                    ui.add_space(2.0);
                    continue;
                }
                ui.add_space(2.0);
                if group.games.is_empty() && group.collection.is_some() {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.label(w::text(t!("Empty collection"), 12.0, t.text_faint));
                    });
                    ui.add_space(6.0);
                    continue;
                }
                ui.push_id(&group.key, |ui| {
                    for e in group.games {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 6.0, 34.0), Sense::click_and_drag());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let sel = self.selected == Some(e.app_id);
                        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
                        if sel {
                            ui.painter().rect_filled(rect, t.corner_small(), t.accent.gamma_multiply(0.16));
                        } else if hover > 0.0 {
                            ui.painter().rect_filled(rect, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface, hover));
                        }
                        let icon = Rect::from_min_size(rect.min + vec2(6.0, 5.0), vec2(24.0, 24.0));
                        let url = e.owned.and_then(|o| o.icon.as_deref()).map(|h| art::icon(e.app_id, h));
                        // The big header only when the game has no icon at all.
                        let header = art::header(e.app_id);
                        let urls: Vec<&str> = url.as_deref().into_iter().chain([header.as_str()]).collect();
                        if !w::paint_cover_first(ui, icon, &urls, CornerRadius::same(4)) {
                            ui.painter().rect_filled(icon, CornerRadius::same(4), t.surface_hover);
                        }
                        let setup = setup_state(ctx, e.app_id, e.installed.is_some());
                        let busy = matches!(setup, Setup::Downloading(_) | Setup::Queued);
                        let color = if busy { t.accent } else if sel { t.text } else if e.installed.is_some() { w::mix(t.text_dim, t.text, hover) } else { t.text_faint };
                        let mut job = egui::text::LayoutJob::simple_singleline(e.name.to_owned(), w::font_medium(13.5), color);
                        job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 50.0);
                        let galley = ui.painter().layout_job(job);
                        ui.painter().galley(pos2(icon.right() + 10.0, rect.center().y - galley.size().y / 2.0), galley, color);
                        if ctx.state.playing == Some(e.app_id) {
                            ui.painter().circle_filled(rect.right_center() - vec2(10.0, 0.0), 4.0, t.in_game);
                        }
                        // Installing: a thin bar along the bottom of the row, like Steam.
                        if let Setup::Downloading(d) = &setup {
                            let frac = d.fraction();
                            let track = Rect::from_min_max(pos2(icon.right() + 10.0, rect.bottom() - 4.0), pos2(rect.right() - 8.0, rect.bottom() - 2.0));
                            ui.painter().rect_filled(track, CornerRadius::same(1), t.raised);
                            let fill = Rect::from_min_size(track.min, vec2(track.width() * frac, track.height()));
                            ui.painter().rect_filled(fill, CornerRadius::same(1), if d.paused { t.text_faint } else { t.accent });
                        }
                        let id = e.app_id;
                        self.game_menu(t, ctx, &resp, id, e.installed.is_some(), ctx.state.playing == Some(id));
                        if resp.drag_started_by(egui::PointerButton::Primary) {
                            self.dragging_game = Some((id, e.name.to_owned()));
                        }
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.selected = Some(id);
                            self.custom_selected = None;
                            self.collections_view = None;
                        }
                    }
                });
                ui.add_space(8.0);
            }
            if !self.custom.is_empty() {
                let key = "sys:non_steam";
                let mut open = self.is_group_open(ui, key, searching);
                let hresp = sidebar_group_header(ui, t, t!("Non-Steam"), self.custom.len(), open, ui.available_width() - 6.0);
                if hresp.clicked() {
                    self.toggle_group(ui, key, searching);
                    open = !open;
                }
                group_context_menu(&hresp, None, total_groups, &mut group_act);
                if open {
                    ui.add_space(2.0);
                    for (i, g) in self.custom.iter().enumerate() {
                        let sel = self.custom_selected == Some(i);
                        let r = ui.add(egui::Button::selectable(sel, w::text_medium(format!("{}   {}", icons::CONTROLLER, g.name), 13.5, t.text_dim)).min_size(vec2(ui.available_width() - 6.0, 32.0)));
                        if r.clicked() {
                            self.custom_selected = Some(i);
                            self.selected = None;
                            self.collections_view = None;
                        }
                    }
                    ui.add_space(8.0);
                } else {
                    ui.add_space(2.0);
                }
            }
            if !self.hidden.is_empty() {
                w::check(ui, t, &mut self.show_hidden, w::text(t!("Show hidden games"), 12.5, t.text_faint));
            }
        });
        drop(items);
        if let Some((col_idx, app_id)) = drop_add {
            self.add_game_to_collection(col_idx, app_id, ctx);
        }
        self.apply_group_menu(ui, group_act, &all_keys, searching);
    }

    fn add_dialog(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(g) = &mut self.adding else { return };
        let mut close = false;
        let mut add = false;
        let mut import = false;
        ui::widgets::modal(ui.ctx(), egui::Id::unique("add-game")).show(ui.ctx(), |ui| {
            ui.set_width(500.0);
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("Add a non-Steam game"), 20.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::button(ui, t, Some(icons::DOWNLOAD), t!("Import from Steam…"), w::Kind::Ghost, w::Size::Sm).on_hover_text(t!("The non-Steam games you added in the official Steam client")).clicked() {
                        import = true;
                    }
                });
            });
            ui.add_space(8.0);
            ui.label(w::text_medium(t!("Program"), 12.5, t.text_dim));
            ui.horizontal(|ui| {
                let hint = if cfg!(windows) { t!("Path to the game's .exe") } else { t!("Path to the game's program (Linux or Windows .exe)") };
                ui.add(egui::TextEdit::singleline(&mut g.exe).hint_text(hint).desired_width(360.0).margin(vec2(10.0, 8.0))).with_edit_menu();
                if w::button(ui, t, Some(icons::FOLDER), t!("Choose…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    // Linux programs often have no extension (or are AppImages): show everything there.
                    let dialog = if cfg!(windows) { rfd::FileDialog::new().add_filter("Programs", &["exe", "bat", "cmd", "lnk"]) } else { rfd::FileDialog::new() };
                    if let Some(path) = dialog.pick_file() {
                        g.exe = path.display().to_string();
                        if g.name.is_empty() {
                            g.name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        }
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(w::text_medium(t!("Display name"), 12.5, t.text_dim));
            ui.add(egui::TextEdit::singleline(&mut g.name).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0))).with_edit_menu();
            ui.add_space(6.0);
            ui.label(w::text_medium(t!("Launch options"), 12.5, t.text_dim));
            ui.add(egui::TextEdit::singleline(&mut g.args).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0))).with_edit_menu();
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let ok = !g.exe.trim().is_empty() && !g.name.trim().is_empty();
                if ui.add_enabled_ui(ok, |ui| w::button(ui, t, Some(icons::PLUS), t!("Add to library"), w::Kind::Primary, w::Size::Md)).inner.clicked() {
                    add = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });
        if add {
            let g = self.adding.take().unwrap();
            ctx.toast(t!("{name} was added to your library", name = g.name));
            self.custom.push(g);
            self.custom_selected = Some(self.custom.len() - 1);
            self.selected = None;
        } else if close {
            self.adding = None;
        } else if import {
            self.adding = None;
            self.importing = Some(self.steam_shortcuts(ctx).into_iter().map(|(s, new)| {
                let tick = new && !s.hidden;
                ((s, new), tick)
            }).map(|((s, _), tick)| (s, tick)).collect());
        }
    }

    /// Steam's non-Steam games for this account, each with whether it's
    /// new here (not in Aurora's list yet).
    fn steam_shortcuts(&self, ctx: &Ctx) -> Vec<(local::shortcuts::Shortcut, bool)> {
        let root = &ctx.local.steam_root;
        if root.as_os_str().is_empty() {
            return Vec::new();
        }
        let account = Some((ctx.state.steam_id & 0xFFFF_FFFF) as u32).filter(|a| *a != 0);
        local::shortcuts::load(root, account).into_iter().map(|s| {
            let new = !self.custom.iter().any(|c| c.exe.trim().trim_matches('"') == s.exe && c.args.trim() == s.launch_options);
            (s, new)
        }).collect()
    }

    fn import_shortcuts(&mut self, picks: Vec<local::shortcuts::Shortcut>) {
        for s in picks {
            self.custom.push(CustomGame { name: s.display_name(), exe: s.exe, args: s.launch_options });
        }
    }

    /// Pick which of Steam's non-Steam games to bring in.
    fn import_dialog(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(list) = &mut self.importing else { return };
        let mut close = false;
        let mut go = false;
        ui::widgets::modal(ui.ctx(), egui::Id::unique("import-shortcuts")).show(ui.ctx(), |ui| {
            ui.set_width(520.0);
            ui.label(w::text_bold(t!("Import from Steam"), 20.0, t.text));
            ui.add(egui::Label::new(w::text(t!("Non-Steam games you added in the official Steam client. Ones already in Aurora are left out."), 12.5, t.text_dim)).wrap());
            ui.add_space(8.0);
            if list.is_empty() {
                ui.label(w::text(t!("Steam has no non-Steam games for this account."), 13.0, t.text_faint));
            }
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (s, tick) in list.iter_mut() {
                    let new = !self.custom.iter().any(|c| c.exe.trim().trim_matches('"') == s.exe && c.args.trim() == s.launch_options);
                    ui.add_enabled_ui(new, |ui| {
                        ui.horizontal(|ui| {
                            let mut on = *tick && new;
                            let label = if new { s.display_name() } else { t!("{name} (already added)", name = s.display_name()).to_string() };
                            if w::check(ui, t, &mut on, label).changed() {
                                *tick = on;
                            }
                            if s.hidden {
                                ui.label(w::text(t!("hidden in Steam"), 11.5, t.text_faint));
                            }
                        });
                        ui.add(egui::Label::new(w::text(&s.exe, 11.5, t.text_faint).monospace()).truncate());
                    });
                    ui.add_space(4.0);
                }
            });
            ui.add_space(10.0);
            let n = list.iter().filter(|(_, tick)| *tick).count();
            ui.horizontal(|ui| {
                if ui.add_enabled_ui(n > 0, |ui| w::button(ui, t, Some(icons::DOWNLOAD), &t!("Import {n}", n = n), w::Kind::Primary, w::Size::Md)).inner.clicked() {
                    go = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });
        if go {
            let picks: Vec<_> = self.importing.take().unwrap_or_default().into_iter().filter(|(_, tick)| *tick).map(|(s, _)| s).collect();
            let n = picks.len();
            self.import_shortcuts(picks);
            ctx.toast(t!("Imported {n} non-Steam games from Steam", n = n));
        } else if close || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.importing = None;
        }
    }

    /// Steam-style "Your Collections" overview: "+ Create a new collection" plus a card for each collection.
    fn collections_overview(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let mut open_detail: Option<usize> = None;
        let mut rename_col: Option<(usize, String)> = None;
        let mut delete_col: Option<usize> = None;
        let mut create_new = false;
        let mut drop_add: Option<(usize, AppId)> = None;

        let mut order: Vec<usize> = (0..self.collections.len()).collect();
        order.sort_by(|&a, &b| self.collections[a].name.to_lowercase().cmp(&self.collections[b].name.to_lowercase()).then(a.cmp(&b)));
        let cards: Vec<Option<(usize, String, Vec<AppId>, usize)>> = std::iter::once(None)
            .chain(order.into_iter().map(|i| {
                let c = &self.collections[i];
                let preview: Vec<AppId> = c.games.iter().copied().take(4).collect();
                Some((i, c.name.clone(), preview, c.games.len()))
            }))
            .collect();

        egui::ScrollArea::vertical().id_salt("lib-collections-overview").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(24.0);
            w::centered(ui, 1400.0, 28.0, |ui| {
                ui.label(w::text_semibold(t!("Your collections").to_uppercase(), 14.0, t.text_dim));
                ui.add_space(16.0);
                let (card_w, card_h, gap) = (196.0_f32, 236.0_f32, 18.0_f32);
                let avail = ui.available_width();
                let cols = (((avail + gap) / (card_w + gap)).floor() as usize).max(1);
                let radius = t.corner();
                for row in cards.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for card in row {
                            let (rect, resp) = ui.allocate_exact_size(vec2(card_w, card_h), Sense::click());
                            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                            match card {
                                None => {
                                    ui.painter().rect_filled(rect, radius, w::mix(t.surface, t.surface_hover, hover));
                                    ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, w::mix(t.border, t.accent, hover * 0.65)), egui::StrokeKind::Inside);
                                    ui.painter().text(
                                        pos2(rect.center().x, rect.center().y - 24.0),
                                        Align2::CENTER_CENTER,
                                        icons::PLUS,
                                        w::font(32.0),
                                        w::mix(t.text_dim, t.text, hover),
                                    );
                                    let mut create_job = egui::text::LayoutJob::simple(
                                        t!("Create a new collection").to_uppercase(),
                                        w::font_bold(12.5),
                                        w::mix(t.text_dim, t.text, hover),
                                        card_w - 28.0,
                                    );
                                    create_job.halign = Align::Center;
                                    let galley = ui.painter().layout_job(create_job);
                                    ui.painter().galley(
                                        pos2(rect.center().x, rect.center().y + 12.0),
                                        galley,
                                        w::mix(t.text_dim, t.text, hover),
                                    );
                                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        create_new = true;
                                    }
                                }
                                Some((i, name, preview, count)) => {
                                    let (drop_hover, dropped) = self.check_drop_on_rect(ui, rect);
                                    if let Some(app_id) = dropped {
                                        drop_add = Some((*i, app_id));
                                    }
                                    paint_collection_collage(ui, t, rect, preview, radius);
                                    let scrim = if drop_hover {
                                        Color32::from_black_alpha(120)
                                    } else {
                                        Color32::from_black_alpha((165.0 - hover * 35.0) as u8)
                                    };
                                    ui.painter().rect_filled(rect, radius, scrim);
                                    let stroke = if drop_hover {
                                        Stroke::new(2.0, t.accent)
                                    } else {
                                        Stroke::new(1.0, w::mix(t.border, t.text_faint, hover))
                                    };
                                    ui.painter().rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
                                    let mut job = egui::text::LayoutJob::simple(
                                        name.to_uppercase(),
                                        w::font_bold(15.0),
                                        Color32::WHITE,
                                        card_w - 24.0,
                                    );
                                    job.halign = Align::Center;
                                    job.wrap.max_rows = 3;
                                    let title_g = ui.painter().layout_job(job);
                                    let count_g = ui.painter().layout_no_wrap(format!("( {count} )"), w::font_medium(13.0), Color32::from_white_alpha(205));
                                    let total_h = title_g.size().y + 6.0 + count_g.size().y;
                                    let top_y = rect.center().y - total_h * 0.5;
                                    ui.painter().galley(pos2(rect.center().x, top_y), title_g, Color32::WHITE);
                                    ui.painter().galley(pos2(rect.center().x - count_g.size().x * 0.5, top_y + total_h - count_g.size().y), count_g, Color32::from_white_alpha(205));
                                    resp.context_menu(|ui| {
                                        if ui.button(format!("{}   {}", icons::NOTE, t!("Rename"))).clicked() {
                                            rename_col = Some((*i, name.clone()));
                                            ui.close();
                                        }
                                        if ui.button(format!("{}   {}", icons::TRASH, t!("Delete collection"))).clicked() {
                                            delete_col = Some(*i);
                                            ui.close();
                                        }
                                    });
                                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        open_detail = Some(*i);
                                    }
                                }
                            }
                        }
                    });
                    ui.add_space(gap);
                }
            });
            ui.add_space(28.0);
        });

        if let Some((col_idx, app_id)) = drop_add {
            self.add_game_to_collection(col_idx, app_id, ctx);
        }
        if create_new {
            self.naming = Some((None, String::new(), None));
        } else if let Some((i, name)) = rename_col {
            self.naming = Some((Some(i), name, None));
        } else if let Some(i) = delete_col {
            self.delete_collection(i);
        } else if let Some(i) = open_detail {
            self.collections_view = Some(CollectionsView::Detail(i));
        }
    }

    /// Steam-style single collection view with rename/delete and drag-and-drop from the sidebar.
    fn collection_detail(&mut self, i: usize, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some((name, game_ids)) = self.collections.get(i).map(|c| (c.name.clone(), c.games.clone())) else {
            self.collections_view = Some(CollectionsView::Overview);
            return;
        };
        let mut back = false;
        let mut rename = false;
        let mut delete_this = false;
        let mut open_game: Option<AppId> = None;
        let mut remove_game: Option<AppId> = None;
        let mut dropped_game: Option<AppId> = None;

        let mut items: Vec<Entry> = entries(ctx.state, ctx.local)
            .into_iter()
            .filter(|e| game_ids.contains(&e.app_id))
            .collect();
        match self.sort {
            Sort::Name => items.sort_by_key(|e| e.name.to_lowercase()),
            Sort::Recent => items.sort_by_key(|e| std::cmp::Reverse(e.installed.and_then(|g| g.last_played))),
            Sort::Playtime => items.sort_by_key(|e| std::cmp::Reverse(e.installed.and_then(|g| g.playtime_minutes))),
            Sort::Size => items.sort_by_key(|e| std::cmp::Reverse(e.installed.map(|g| g.size_on_disk))),
        }

        let viewport_bottom = ui.max_rect().bottom();
        egui::ScrollArea::vertical().id_salt(("lib-collection-detail", i)).auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(24.0);
            w::centered(ui, 1400.0, 28.0, |ui| {
                ui.horizontal(|ui| {
                    if w::button(ui, t, Some(icons::BACK), t!("Collections"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        back = true;
                    }
                    ui.add_space(6.0);
                    ui.label(w::text_bold(&name, 22.0, t.text));
                    if w::icon_button(ui, t, icons::NOTE, t!("Rename collection"), false, 0).clicked() {
                        rename = true;
                    }
                    ui.label(w::text(format!("({})", items.len()), 15.0, t.text_faint));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, t, Some(icons::TRASH), t!("Delete collection"), w::Kind::Ghost, w::Size::Sm).clicked() {
                            delete_this = true;
                        }
                    });
                });
                ui.add_space(6.0);
                let (line_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().hline(line_rect.x_range(), line_rect.center().y, Stroke::new(1.0, t.border));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(w::text(t!("Sort"), 13.0, t.text_faint));
                    w::segmented(ui, t, &mut self.sort, &[(Sort::Name, t!("Name")), (Sort::Recent, t!("Recent")), (Sort::Playtime, t!("Hours")), (Sort::Size, t!("Size"))]);
                    ui.add_space(10.0);
                    ui.label(w::text(icons::GRID, 15.0, t.text_faint));
                    ui.add(egui::Slider::new(&mut self.tile, 110.0..=220.0).show_value(false)).on_hover_text(t!("Tile size"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.dragging_game.is_some() {
                            ui.label(w::text_medium(t!("Drop here to add to {name}", name = name.clone()), 13.0, t.accent));
                        } else if !items.is_empty() {
                            ui.label(w::text(t!("Drag games from the sidebar to add"), 12.5, t.text_faint));
                        }
                    });
                });
                ui.add_space(14.0);

                let drop_top = ui.cursor().top();
                let avail_w = ui.available_width();
                let min_drop_h = (viewport_bottom - drop_top - 28.0).max(320.0);
                let drop_rect = Rect::from_min_size(pos2(ui.cursor().left(), drop_top), vec2(avail_w, min_drop_h));

                if items.is_empty() {
                    let (rect, _) = ui.allocate_exact_size(vec2(avail_w, min_drop_h), Sense::hover());
                    let (drop_hover, dropped) = self.check_drop_on_rect(ui, rect);
                    if let Some(app_id) = dropped {
                        dropped_game = Some(app_id);
                    }
                    if self.dragging_game.is_some() {
                        let fill = if drop_hover { t.accent.gamma_multiply(0.1) } else { t.accent.gamma_multiply(0.03) };
                        let stroke = if drop_hover { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.accent.gamma_multiply(0.45)) };
                        ui.painter().rect_filled(rect, t.corner(), fill);
                        ui.painter().rect_stroke(rect, t.corner(), stroke, egui::StrokeKind::Inside);
                    }
                    let msg_color = if drop_hover { t.accent } else { t.text_dim };
                    ui.painter().text(
                        pos2(rect.center().x, rect.top() + (rect.height() * 0.28).max(72.0)),
                        Align2::CENTER_CENTER,
                        t!("Drag games here to build this collection."),
                        w::font(15.0),
                        msg_color,
                    );
                } else {
                    let gap = 14.0;
                    let cols = (((avail_w + gap) / (self.tile + gap)).floor() as usize).max(1);
                    let w_tile = ((avail_w - gap * (cols - 1) as f32) / cols as f32).floor();
                    let size = vec2(w_tile, (w_tile * 1.5).round());
                    for row in items.chunks(cols) {
                        if w::skip_offscreen_row(ui, size.y) {
                            ui.add_space(gap);
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for e in row {
                                let r = w::game_tile(ui, t, &art::capsule(e.app_id), &art::header(e.app_id), e.name, e.installed.is_some(), size);
                                let x_rect = Rect::from_min_size(pos2(r.rect.right() - 28.0, r.rect.top() + 6.0), vec2(22.0, 22.0));
                                let pointer_in_x = ui.ctx().pointer_latest_pos().is_some_and(|p| x_rect.contains(p));
                                let show_x = self.dragging_game.is_none() && (r.hovered() || pointer_in_x);
                                let mut removed_here = false;
                                if show_x {
                                    let x_resp = ui.interact(x_rect, ui.scope_id().with(("col-rm", i, e.app_id)), Sense::click());
                                    let bg = if x_resp.hovered() { t.danger } else { Color32::from_black_alpha(180) };
                                    ui.painter().rect_filled(x_rect, CornerRadius::same(4), bg);
                                    ui.painter().text(x_rect.center(), Align2::CENTER_CENTER, icons::X, w::font(12.0), Color32::WHITE);
                                    if x_resp.on_hover_text(t!("Remove from collection")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        remove_game = Some(e.app_id);
                                        removed_here = true;
                                    }
                                }
                                self.game_menu(t, ctx, &r, e.app_id, e.installed.is_some(), ctx.state.playing == Some(e.app_id));
                                if !removed_here && !pointer_in_x && r.clicked() {
                                    open_game = Some(e.app_id);
                                }
                            }
                        });
                        ui.add_space(gap);
                    }
                    let used_bottom = ui.cursor().top();
                    if used_bottom < drop_rect.bottom() {
                        ui.allocate_exact_size(vec2(avail_w, drop_rect.bottom() - used_bottom), Sense::hover());
                    }
                    let full_drop_rect = Rect::from_min_max(drop_rect.min, pos2(drop_rect.right(), used_bottom.max(drop_rect.bottom())));
                    let (drop_hover, dropped) = self.check_drop_on_rect(ui, full_drop_rect);
                    if let Some(app_id) = dropped {
                        dropped_game = Some(app_id);
                    }
                    if self.dragging_game.is_some() {
                        let pad_rect = full_drop_rect.expand(6.0);
                        let fill = if drop_hover { t.accent.gamma_multiply(0.08) } else { Color32::TRANSPARENT };
                        let stroke = if drop_hover { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.accent.gamma_multiply(0.4)) };
                        ui.painter().rect_filled(pad_rect, t.corner(), fill);
                        ui.painter().rect_stroke(pad_rect, t.corner(), stroke, egui::StrokeKind::Inside);
                    }
                }
            });
            ui.add_space(28.0);
        });
        drop(items);

        if let Some(app_id) = dropped_game {
            self.add_game_to_collection(i, app_id, ctx);
        }
        if let Some(app_id) = remove_game {
            if let Some(c) = self.collections.get_mut(i) {
                c.games.remove(&app_id);
            }
        }
        if back {
            self.collections_view = Some(CollectionsView::Overview);
        } else if rename {
            self.naming = Some((Some(i), name, None));
        } else if delete_this {
            self.delete_collection(i);
        } else if let Some(id) = open_game {
            self.selected = Some(id);
            self.custom_selected = None;
            self.collections_view = None;
        }
    }

    fn custom_page(&mut self, i: usize, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(g) = self.custom.get(i).cloned() else {
            self.custom_selected = None;
            return;
        };
        let mut remove = false;
        ui.add_space(28.0);
        w::centered(ui, 900.0, 32.0, |ui| {
            w::pill(ui, t!("Non-Steam game"), t.text_dim, t.raised);
            ui.label(w::text_bold(&g.name, 32.0, t.text));
            ui.label(w::text(&g.exe, 13.0, t.text_faint).monospace());
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::PLAY), t!("Play"), w::Kind::Play, w::Size::Lg).clicked() {
                    // Windows programs run with Proton on Linux; shortcuts and
                    // admin-only programs go through the shell on Windows.
                    match client::spawn_non_steam(&g.exe, &g.args) {
                        Ok(Some(mut child)) => {
                            std::thread::spawn(move || child.wait());
                        }
                        Ok(None) => {}
                        Err(e) => ctx.toast(e),
                    }
                }
                if let Some(dir) = std::path::Path::new(g.exe.trim().trim_matches('"')).parent() {
                    if w::button(ui, t, Some(icons::FOLDER), t!("Browse local files"), w::Kind::Secondary, w::Size::Lg).clicked() {
                        local::open_folder(dir);
                    }
                }
                if w::button(ui, t, Some(icons::TRASH), t!("Remove from library"), w::Kind::Danger, w::Size::Lg).clicked() {
                    remove = true;
                }
            });
        });
        if remove {
            self.custom.remove(i);
            self.custom_selected = None;
        }
    }

    fn home(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let mut recent: Vec<_> = ctx.local.games.iter().filter(|g| g.last_played.is_some()).collect();
        recent.sort_by_key(|g| std::cmp::Reverse(g.last_played));
        // Recent games across the whole account (installed or not), like Steam.
        let mut account_recent: Vec<(AppId, &str, u64)> = match ctx.state.player_games.get(&ctx.state.steam_id) {
            Some(Ok(list)) if !list.is_empty() => list.iter().filter_map(|g| Some((g.app_id, g.name.as_str(), g.last_played?))).collect(),
            _ => recent.iter().filter_map(|g| Some((g.app_id, g.name.as_str(), g.last_played?))).collect(),
        };
        account_recent.sort_by_key(|g| std::cmp::Reverse(g.2));

        egui::ScrollArea::vertical().id_salt("lib-home").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(24.0);
            w::centered(ui, 1400.0, 28.0, |ui| {
                self.whats_new(ui, t, ctx, &account_recent);
                // Continue playing.
                if let Some(g) = recent.first() {
                    let width = ui.available_width();
                    let (rect, resp) = ui.allocate_exact_size(vec2(width, 300.0), Sense::click());
                    let r = t.corner();
                    if !w::paint_cover_first(ui, rect, &[&art::hero(g.app_id), &art::header(g.app_id)], r) {
                        w::skeleton(ui, t, rect, r);
                    }
                    w::fade_right_rounded(ui, rect, Color32::from_black_alpha(200), 0.65, r);
                    w::fade_down_rounded(ui, rect, Color32::from_black_alpha(160), 0.4, r);
                    let inner = rect.shrink(32.0);
                    ui.painter().text(inner.left_top(), Align2::LEFT_TOP, t!("CONTINUE PLAYING"), w::font_bold(12.0), t.accent);
                    let logo = Rect::from_min_max(inner.left_top() + vec2(0.0, 28.0), inner.left_bottom() + vec2(380.0, -66.0));
                    if !w::paint_contain(ui, logo, &art::logo(g.app_id), Align2::LEFT_BOTTOM) {
                        ui.painter().text(logo.left_bottom(), Align2::LEFT_BOTTOM, &g.name, w::font_bold(34.0), Color32::WHITE);
                    }
                    let btn = Rect::from_min_size(inner.left_bottom() - vec2(0.0, 48.0), vec2(150.0, 48.0));
                    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(btn.min, vec2(600.0, 48.0))).layout(Layout::left_to_right(Align::Center)));
                    let playing = ctx.state.playing == Some(g.app_id);
                    let g: InstalledGame = (*g).clone();
                    self.play_button(&mut c, t, ctx, &g);
                    c.add_space(12.0);
                    if let Some(m) = g.playtime_minutes {
                        c.label(w::text_medium(t!("{time} played", time = ui::playtime(m)), 14.0, Color32::from_white_alpha(220)));
                    }
                    self.game_menu(t, ctx, &resp, g.app_id, true, playing);
                    if resp.clicked() {
                        self.selected = Some(g.app_id);
                    }
                    ui.add_space(30.0);
                }

                // Recent games shelf: everything played lately, installed or not.
                let first = recent.first().map(|g| g.app_id);
                let shelf: Vec<&(AppId, &str, u64)> = account_recent.iter().filter(|g| Some(g.0) != first).collect();
                if !shelf.is_empty() {
                    w::section(ui, t, t!("Recent games"), None);
                    // As many 2:3 covers as fit; a full row is widened a little so it
                    // ends at the right edge, like the cards above it.
                    let (gap, base) = (16.0, 170.0);
                    let avail = ui.available_width();
                    let per = (((avail + gap) / (base + gap)).floor() as usize).max(1);
                    let width = if shelf.len() >= per { ((avail - gap * (per - 1) as f32) / per as f32).floor() } else { base };
                    let size = vec2(width, width * 1.5);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 16.0;
                        for (id, name, when) in shelf.into_iter().take(per) {
                            let installed = ctx.local.games.iter().any(|g| g.app_id == *id);
                            ui.vertical(|ui| {
                                ui.set_width(size.x);
                                let r = w::game_tile(ui, t, &art::capsule(*id), &art::header(*id), name, installed, size);
                                self.game_menu(t, ctx, &r, *id, installed, ctx.state.playing == Some(*id));
                                if r.clicked() {
                                    self.selected = Some(*id);
                                }
                                ui.label(w::text(ui::ago(*when), 12.0, t.text_faint));
                            });
                        }
                    });
                    ui.add_space(26.0);
                }

                // All games grid.
                let items = self.filtered(ctx);
                ui.horizontal(|ui| {
                    ui.label(w::text_bold(t!("All games ({n})", n = items.len()), 18.0, t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(egui::Slider::new(&mut self.tile, 110.0..=220.0).show_value(false)).on_hover_text(t!("Tile size"));
                        ui.label(w::text(icons::GRID, 15.0, t.text_faint));
                        ui.add_space(10.0);
                        w::segmented(ui, t, &mut self.sort, &[(Sort::Name, t!("Name")), (Sort::Recent, t!("Recent")), (Sort::Playtime, t!("Hours")), (Sort::Size, t!("Size"))]);
                        ui.label(w::text(t!("Sort"), 13.0, t.text_faint));
                    });
                });
                ui.add_space(12.0);
                if items.is_empty() {
                    if ctx.state.apps.is_none() {
                        w::loading(ui, t, t!("Loading your library…"));
                    } else {
                        w::empty(ui, t, icons::SEARCH, t!("No games match"), t!("Try a different search or filter."));
                    }
                    return;
                }
                let size = vec2(self.tile, self.tile * 1.5);
                let gap = 16.0;
                let cols = (((ui.available_width() + gap) / (size.x + gap)).floor() as usize).max(1);
                let label_height = ui.fonts_mut(|fonts| fonts.row_height(&w::font_medium(13.0)));
                let row_height = size.y + 6.0 + label_height;
                for row in items.chunks(cols) {
                    if w::skip_offscreen_row(ui, row_height) {
                        ui.add_space(10.0);
                        continue;
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for e in row {
                            ui.vertical(|ui| {
                                ui.set_width(size.x);
                                ui.spacing_mut().item_spacing.y = 6.0;
                                let r = w::game_tile(ui, t, &art::capsule(e.app_id), &art::header(e.app_id), e.name, e.installed.is_some(), size);
                                self.game_menu(t, ctx, &r, e.app_id, e.installed.is_some(), ctx.state.playing == Some(e.app_id));
                                if r.clicked() {
                                    self.selected = Some(e.app_id);
                                }
                                ui.add(egui::Label::new(w::text_medium(e.name, 13.0, if e.installed.is_some() { t.text } else { t.text_dim })).truncate());
                            });
                        }
                    });
                    ui.add_space(10.0);
                }
                ui.add_space(30.0);
            });
        });
    }

    fn game_page(&mut self, id: AppId, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let owned = ctx.state.apps.as_ref().and_then(|apps| apps.get(&id)).filter(|app| !app.is_dlc());
        let mut installs = ctx.local.games.iter().filter(|game| game.app_id == id);
        // Match entries(): use the last install for owned apps and the first
        // for apps that are only present on disk.
        let installed = if owned.is_some() { installs.next_back() } else { installs.next() };
        let Some(name) = owned.map(|app| &app.name).or_else(|| installed.map(|game| &game.name)) else {
            self.selected = None;
            return;
        };
        let (name, installed, owned) = (name.clone(), installed.cloned(), owned.cloned());
        let download = ctx.state.downloads.get(&id).cloned().filter(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed));
        let queued = ctx.state.download_queue.contains(&id);

        egui::ScrollArea::vertical().id_salt(("game", id)).auto_shrink([false, false]).show(ui, |ui| {
            // Hero with logo.
            let full = ui.available_width();
            let (hero, _) = ui.allocate_exact_size(vec2(full, 340.0), Sense::hover());
            if !w::paint_cover_first(ui, hero, &[&props::hero(id), &art::header(id)], CornerRadius::ZERO) {
                w::placeholder(ui, t, hero, "", CornerRadius::ZERO);
            }
            w::fade_down(ui, hero, t.bg, 0.5);
            let logo = Rect::from_min_max(hero.left_top() + vec2(40.0, 60.0), hero.left_bottom() + vec2(460.0, -30.0));
            if !w::paint_contain(ui, logo, &props::logo(id), Align2::LEFT_BOTTOM) {
                ui.painter().text(logo.left_bottom(), Align2::LEFT_BOTTOM, &name, w::font_bold(40.0), Color32::WHITE);
            }

            // Play bar.
            egui::Frame::new().fill(t.chrome).corner_radius(t.corner()).inner_margin(Margin::symmetric(20, 14)).outer_margin(Margin::symmetric(24, 0)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let playing = ctx.state.playing == Some(id);
                    if let Some(d) = &download {
                        self.download_bar(ui, t, ctx, id, d);
                    } else if queued {
                        ui.add_enabled_ui(false, |ui| w::button(ui, t, Some(icons::CLOCK), t!("Queued"), w::Kind::Secondary, w::Size::Lg));
                    } else {
                        match &installed {
                            Some(_) if self.uninstalling.contains_key(&id) => {
                                let p = self.uninstalling[&id].progress.fraction();
                                uninstall_bar(ui, t, p);
                            }
                            Some(_) if ctx.state.moving.contains_key(&id) => {
                                let p = ctx.state.moving[&id].fraction();
                                busy_bar(ui, t, t!("Moving… {pct}%", pct = (p * 100.0).round() as u32), p);
                            }
                            Some(_) if ctx.state.backing_up.contains_key(&id) => {
                                let p = ctx.state.backing_up[&id].fraction();
                                busy_bar(ui, t, t!("Backing up… {pct}%", pct = (p * 100.0).round() as u32), p);
                            }
                            Some(_) if playing => {
                                if w::button(ui, t, Some(icons::X), t!("Stop"), w::Kind::Danger, w::Size::Lg).on_hover_text(t!("Close the game")).clicked() {
                                    ctx.send(Command::StopGame { app_id: id });
                                }
                            }
                            Some(g) if self.needs_update(ctx, g) => {
                                if w::button(ui, t, Some(icons::DOWNLOAD), t!("Update"), w::Kind::Primary, w::Size::Lg).clicked() {
                                    self.queue_install(ctx, id, Some(g.library.clone()));
                                }
                            }
                            Some(_) if ctx.state.install_script.contains_key(&id) => {
                                // Like Steam's "Running install script" on a first launch.
                                let step = &ctx.state.install_script[&id];
                                let label = if step.is_empty() { t!("Running install script…").to_string() } else { t!("Running install script: {step}", step = step) };
                                busy_bar(ui, t, label, 0.0);
                            }
                            Some(_) if ctx.state.shader_compiling.contains_key(&id) => {
                                // Like Steam's "Processing Vulkan shaders": the game starts after.
                                let (done, total) = ctx.state.shader_compiling[&id];
                                compile_bar(ui, t, done, total);
                                ui.add_space(8.0);
                                if w::button(ui, t, None, t!("Skip"), w::Kind::Secondary, w::Size::Lg).on_hover_text(t!("Start the game now; it compiles what's missing while you play")).clicked() {
                                    ctx.send(Command::SkipShaderCompile { app_id: id });
                                }
                            }
                            Some(_) if ctx.state.cloud_syncing.contains(&id) => {
                                // Like Steam: the game starts once its saves are in sync.
                                ui.add_enabled_ui(false, |ui| w::button(ui, t, Some(icons::CLOUD_CHECK), t!("Syncing Steam Cloud…"), w::Kind::Play, w::Size::Lg));
                                ui.add(egui::Spinner::new().color(t.accent));
                            }
                            Some(_) if ctx.state.launching == Some(id) => {
                                if w::button(ui, t, Some(icons::X), t!("Launching…"), w::Kind::Danger, w::Size::Lg).on_hover_text(t!("Cancel")).clicked() {
                                    ctx.send(Command::StopGame { app_id: id });
                                }
                                ui.add(egui::Spinner::new().color(t.accent));
                            }
                            Some(g) => {
                                let r = w::button(ui, t, Some(icons::PLAY), t!("Play"), w::Kind::Play, w::Size::Lg);
                                if r.clicked() {
                                    let args = self.launch_args_for(id);
                                    self.launch_or_notice(ctx, id, g.install_dir.clone(), args);
                                }
                            }
                            None if owned.is_some() => {
                                if w::button(ui, t, Some(icons::DOWNLOAD), t!("Install"), w::Kind::Primary, w::Size::Lg).clicked() {
                                    self.install_dialog = Some(InstallDialog::new(id, ctx));
                                }
                            }
                            None => {
                                if w::button(ui, t, Some(icons::STORE), t!("Store page"), w::Kind::Primary, w::Size::Lg).clicked() {
                                    ctx.navigate("store", Some(format!("app:{id}")));
                                }
                            }
                        }
                        ui.add_space(22.0);
                        if let Some(g) = &installed {
                            w::stat(ui, t, t!("Last played"), &g.last_played.map(ui::ago).unwrap_or_else(|| t!("Never").to_string()));
                            ui.add_space(20.0);
                            crate::offline_ui::playtime_stat(ui, t, ctx.state, id, g.playtime_minutes);
                            ui.add_space(20.0);
                            w::stat(ui, t, t!("Size"), &ui::bytes(g.size_on_disk));
                            // Where the game's Steam features come from (see docs/steam-compatibility.md).
                            // Worked out in the background: reading the install folder
                            // would hold up the frame (the stat appears a moment later).
                            let mode = owned.as_ref().and_then(|o| client::launch_mode_peek(o, &g.install_dir));
                            if owned.is_some() && mode.is_none() {
                                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                            }
                            if let Some(mode) = mode {
                                let (mode, why) = match mode {
                                    client::LaunchMode::Aurora => (t!("Aurora"), t!("Achievements, cloud saves and friends work through Aurora.").to_string()),
                                    client::LaunchMode::OfficialSteam { anti_cheat } => (if anti_cheat == client::STEAM_DRM { t!("Runs through Steam (Steam DRM)") } else if cfg!(windows) { t!("Runs through Steam (anti-cheat)") } else { t!("Runs through Steam") }, t!("Uses {name}: starts through the official Steam client, which opens in the background.", name = anti_cheat)),
                                    client::LaunchMode::HeadlessHost { .. } => (t!("Your Steam (no window)"), t!("Starts a limited, windowless instance of your own installed Steam in the background, with no store or friends. It signs in and closes when the game does.").to_string()),
                                    client::LaunchMode::NoSteam { reason } => (t!("Starts without Steam"), i18n::tr_dyn(&reason)),
                                };
                                ui.add_space(20.0);
                                ui.scope(|ui| w::stat(ui, t, t!("Steam features"), mode)).response.on_hover_text(why);
                            }
                        } else if let Some(o) = &owned {
                            let size: u64 = o.depots.iter().filter(|d| d.dlc_app.is_none() && (d.os.is_empty() || d.os.contains("windows"))).filter_map(|d| d.manifest("public").map(|m| m.1)).sum();
                            if size > 0 {
                                w::stat(ui, t, t!("Download size"), &format!("≈ {}", ui::bytes(size)));
                            }
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let fav = self.favorites.contains(&id);
                        let heart = w::icon_button(ui, t, icons::HEART, if fav { t!("Remove from favorites") } else { t!("Add to favorites") }, fav, 0);
                        if heart.clicked() && !self.favorites.remove(&id) {
                            self.favorites.insert(id);
                        }
                        if w::icon_button(ui, t, icons::GEAR, t!("Properties…"), false, 0).clicked() {
                            self.props = Some(props::Props { app: id, tab: props::PropTab::General, ..Default::default() });
                        }
                        let more = w::icon_button(ui, t, icons::DOTS, t!("Manage"), false, 0);
                        egui::Popup::menu(&more).width(240.0).show(|ui| self.manage_menu(ui, t, ctx, id, installed.as_ref()));
                        ctx.game_actions(id, ui);
                        crate::offline_ui::cloud_pending(ui, t, ctx.state, id);
                    });
                });
            });
            ui.add_space(10.0);

            // Link row (like Steam's under the play bar).
            egui::Frame::new().outer_margin(Margin::symmetric(24, 0)).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    let link = |ui: &mut Ui, icon: &str, label: &str| w::button(ui, t, Some(icon), label, w::Kind::Secondary, w::Size::Sm).clicked();
                    if link(ui, icons::STORE, t!("Store page")) {
                        ctx.navigate("store", Some(format!("app:{id}")));
                    }
                    if link(ui, icons::THUMBS_UP, t!("Reviews")) {
                        let name = ctx.state.app_name(id).unwrap_or_default().to_string();
                        self.reviews = Some(super::reviews::ReviewWindow::new(id, &name, true, false));
                    }
                    if link(ui, icons::PUZZLE, t!("Workshop")) {
                        ctx.navigate("workshop", Some(id.to_string()));
                    }
                    for (icon, label, page) in [
                        (icons::USERS_THREE, t!("Community hub"), "hub"),
                        (icons::CHATS, t!("Discussions"), "discussions"),
                        (icons::SCROLL, t!("Guides"), "guides"),
                        (icons::TROPHY, t!("Achievements"), "achievements"),
                    ] {
                        if link(ui, icon, label) {
                            ctx.navigate(page, Some(id.to_string()));
                        }
                    }
                    if link(ui, icons::QUESTION, t!("Support")) {
                        ctx.navigate("support", Some(format!("game:{id}")));
                    }
                    // The Market has no native page yet.
                    if w::button(ui, t, Some(icons::EXTERNAL), t!("Market"), w::Kind::Ghost, w::Size::Sm).on_hover_text(t!("Opens in your browser")).clicked() {
                        crate::links::open_http(&format!("https://steamcommunity.com/market/search?appid={id}"));
                    }
                });
            });
            ui.add_space(18.0);

            w::centered(ui, 1300.0, 24.0, |ui| {
                // Two columns when there's room, stacked in a narrow window
                // (the window can also be tiny for a moment, e.g. while restoring).
                let total = ui.available_width();
                let side = (total * 0.32).clamp(280.0, 400.0);
                if total >= 720.0 {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 24.0;
                        ui.vertical(|ui| {
                            ui.set_width(total - side - 24.0);
                            self.dlc_strip(ui, t, ctx, id);
                            self.updates_feed(ui, t, id);
                        });
                        ui.vertical(|ui| {
                            ui.set_width(side);
                            self.side_panels(ui, t, ctx, id, installed.as_ref(), owned.as_ref());
                        });
                    });
                } else {
                    self.side_panels(ui, t, ctx, id, installed.as_ref(), owned.as_ref());
                    ui.add_space(16.0);
                    self.dlc_strip(ui, t, ctx, id);
                    self.updates_feed(ui, t, id);
                }
                ui.add_space(40.0);
            });
        });
        self.dialogs(ui, t, ctx, id, &name, installed.as_ref());
    }

    fn download_bar(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId, d: &model::DownloadStatus) {
        let label = i18n::tr_dyn(d.activity());
        ui.vertical(|ui| {
            ui.set_width(380.0);
            ui.horizontal(|ui| {
                ui.label(w::text_semibold(label, 14.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Like Steam: an update shows only its own size.
                    let (done, total) = d.shown();
                    ui.label(w::text(format!("{} / {}", ui::bytes(done), ui::bytes(total)), 12.5, t.text_dim));
                });
            });
            w::progress(ui, t, d.fraction(), 380.0);
            // Network and disk separately (they differ a lot: reused data
            // and buffered writes hit the disk without coming over the network).
            if !d.verifying() {
                ui.label(w::text(t!("Network {net}/s · Disk {disk}/s", net = ui::bytes(d.speed), disk = ui::bytes(d.disk_speed)), 12.0, t.text_faint));
            }
        });
        if d.verifying() {
            if w::icon_button(ui, t, icons::X, t!("Cancel"), false, 0).clicked() {
                ctx.send(Command::CancelVerify { app_id: id });
            }
            return;
        }
        if d.paused {
            if w::icon_button(ui, t, icons::PLAY, t!("Resume"), false, 0).clicked() {
                ctx.send(Command::ResumeDownload { app_id: id });
            }
        } else if w::icon_button(ui, t, "\u{E39E}", t!("Pause"), false, 0).clicked() {
            ctx.send(Command::PauseDownload { app_id: id });
        }
        if w::icon_button(ui, t, icons::X, t!("Cancel"), false, 0).clicked() {
            ctx.send(Command::CancelDownload { app_id: id });
        }
    }

    fn manage_menu(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId, installed: Option<&InstalledGame>) {
        ui.set_min_width(240.0);
        w::menu_spacing(ui);
        let item = |ui: &mut Ui, icon: &str, label: &str, color: Color32| w::menu_item_tinted(ui, t, icon, label, (color != t.text).then_some(color)).clicked();
        if let Some(g) = installed {
            if item(ui, icons::DESKTOP, t!("Add desktop shortcut"), t.text) {
                self.add_shortcut(ctx, id, g);
            }
            if item(ui, icons::FOLDER, t!("Browse local files"), t.text) {
                local::open_folder(&g.install_dir);
            }
            if item(ui, icons::SHIELD, t!("Verify game files"), t.text) {
                self.start_verify(ctx, g);
            }
            if item(ui, icons::DRIVES, t!("Move install folder…"), t.text) {
                let name = ctx.state.app_name(id).map(String::from).unwrap_or_else(|| g.name.clone());
                self.storage_dialog = Some(super::storage_ui::StorageDialog::Move(super::storage_ui::MoveDialog::new(g.clone(), name, ctx)));
            }
            if item(ui, icons::PACKAGE, t!("Back up game files…"), t.text) {
                let name = ctx.state.app_name(id).map(String::from).unwrap_or_else(|| g.name.clone());
                self.storage_dialog = Some(super::storage_ui::StorageDialog::Backup(super::storage_ui::BackupDialog::new(g.clone(), name)));
            }
        }
        if item(ui, icons::RESTORE, t!("Restore game backup…"), t.text) {
            self.storage_dialog = Some(super::storage_ui::StorageDialog::Restore(super::storage_ui::RestoreDialog::new(ctx)));
        }
        #[cfg(not(windows))]
        proton_submenu(ui, t, ctx, id);
        if item(ui, icons::INFO, t!("Get help with this game"), t.text) {
            ctx.navigate("support", Some(format!("game:{id}")));
        }
        if item(ui, icons::SLIDERS, t!("Properties…"), t.text) {
            self.props = Some(props::Props { app: id, tab: props::PropTab::General, ..Default::default() });
        }
        if installed.is_some() {
            w::menu_separator(ui, t);
            if item(ui, icons::TRASH, t!("Uninstall…"), t.danger) {
                self.dialog = Some(Dialog::Uninstall);
            }
        }
    }

    /// DLC owned for this game, as a horizontal strip.
    fn dlc_strip(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId) {
        let dlc: Vec<(AppId, String)> = ctx
            .state
            .apps
            .as_ref()
            // Like Steam: only DLC with a store page. Free "DLC" such as Dota 2's
            // Vulkan support stays in Properties, not on the game's page.
            .map(|a| a.values().filter(|d| d.is_dlc() && d.parent == Some(id) && d.store_assets).map(|d| (d.app_id, d.name.clone())).collect())
            .unwrap_or_default();
        if dlc.is_empty() {
            return;
        }
        egui::Frame::new().fill(t.surface).stroke(Stroke::new(1.0, t.accent_2.gamma_multiply(0.5))).corner_radius(t.corner()).inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::text_bold(t!("YOUR DLC ({n})", n = dlc.len()), 12.0, t.text_dim));
            ui.add_space(6.0);
            egui::ScrollArea::horizontal().id_salt(("dlc", id)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (d, name) in &dlc {
                        let (r, resp) = ui.allocate_exact_size(vec2(220.0, 103.0), Sense::click());
                        // DLC without its own art shows the game's, with a DLC badge and the name, like Steam.
                        let url = art::header(*d);
                        if !w::paint_cover(ui, r, &url, t.corner_small()) {
                            if w::image_state(ui, &url) == w::ImageState::Missing {
                                w::image_or_skeleton(ui, t, r, &art::header(id), t.corner_small());
                                let shade = egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 30.0), r.max);
                                ui.painter().rect_filled(shade, egui::CornerRadius { sw: t.corner_small().sw, se: t.corner_small().se, nw: 0, ne: 0 }, Color32::from_black_alpha(190));
                                let galley = ui.painter().layout(name.clone(), w::font_semibold(12.5), Color32::WHITE, r.width() - 16.0);
                                ui.painter().galley(egui::pos2(r.left() + 8.0, shade.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
                                let badge = egui::Rect::from_min_size(r.min, vec2(34.0, 17.0));
                                ui.painter().rect_filled(badge, egui::CornerRadius { nw: t.corner_small().nw, se: 4, ne: 0, sw: 0 }, t.accent_2);
                                ui.painter().text(badge.center(), egui::Align2::CENTER_CENTER, "DLC", w::font_bold(10.5), Color32::WHITE);
                            } else {
                                w::skeleton(ui, t, r, t.corner_small());
                            }
                        }
                        let off = self.games.get(&id).is_some_and(|g| g.disabled_dlc.contains(d));
                        if off {
                            ui.painter().rect_filled(r, t.corner_small(), Color32::from_black_alpha(150));
                        }
                        if resp.on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            ctx.navigate("store", Some(format!("app:{d}")));
                        }
                    }
                });
            });
        });
        ui.add_space(16.0);
    }

    fn side_panels(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId, installed: Option<&InstalledGame>, owned: Option<&model::OwnedApp>) {
        // Notes.
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::caption_text(&t, t!("NOTES")));
            ui.add_space(4.0);
            let s = self.games.entry(id).or_default();
            ui.add(egui::TextEdit::multiline(&mut s.notes).hint_text(t!("Write a note for yourself…")).desired_rows(3).desired_width(f32::INFINITY)).with_edit_menu();
        });
        ui.add_space(12.0);

        // Friends who are playing.
        let friends = ctx.state.friends_playing(id);
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::caption_text(&t, t!("FRIENDS PLAYING NOW")));
            ui.add_space(4.0);
            if friends.is_empty() {
                ui.label(w::text(t!("None of your friends are playing this right now."), 13.0, t.text_faint));
            }
            ui.horizontal_wrapped(|ui| {
                for p in friends {
                    w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 36.0, Some(t.in_game)).on_hover_text(&p.name);
                }
            });
        });
        ui.add_space(12.0);

        // Workshop.
        if let Some(g) = installed {
            let items = local::manage::workshop_items(g);
            if !items.is_empty() {
                w::card(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(w::caption_text(&t, t!("STEAM WORKSHOP")));
                    ui.add_space(4.0);
                    let total: u64 = items.iter().map(|i| i.size).sum();
                    ui.label(w::text(t!("{n} items · {size}", n = items.len(), size = ui::bytes(total)), 13.5, t.text));
                    ui.horizontal(|ui| {
                        if w::button(ui, t, None, t!("Manage"), w::Kind::Secondary, w::Size::Sm).clicked() {
                            self.props = Some(props::Props { app: id, tab: props::PropTab::Workshop, ..Default::default() });
                        }
                        if w::button(ui, t, None, t!("Browse the Workshop"), w::Kind::Ghost, w::Size::Sm).clicked() {
                            ctx.navigate("workshop", Some(id.to_string()));
                        }
                    });
                });
                ui.add_space(12.0);
            }
        }

        // Details.
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::caption_text(&t, t!("DETAILS")));
            ui.add_space(4.0);
            egui::Grid::new("game-details").num_columns(2).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                ui.label(w::caption_text(&t, t!("APP ID")));
                ui.label(w::text(id.to_string(), 13.0, t.text));
                ui.end_row();
                if let Some(n) = self.current_players(id) {
                    ui.label(w::caption_text(&t, t!("PLAYING NOW")));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.0, t.play);
                        ui.label(w::text(super::store::thousands(n as u64), 13.0, t.text));
                    })
                    .response
                    .on_hover_text(t!("People playing on Steam right now. Updates every minute."));
                    ui.end_row();
                }
                if let Some(g) = installed {
                    let (build, branch) = local::manage::installed_build(g);
                    ui.label(w::caption_text(&t, t!("BUILD")));
                    ui.label(w::text(format!("{build}{}", if branch != "public" { format!(" ({branch})") } else { String::new() }), 13.0, t.text));
                    ui.end_row();
                    ui.label(w::caption_text(&t, t!("LAST UPDATED")));
                    ui.label(w::text(g.last_updated.map(news::date).unwrap_or_else(|| "—".into()), 13.0, t.text));
                    ui.end_row();
                }
                if let Some(o) = owned {
                    if !o.branches.is_empty() {
                        ui.label(w::caption_text(&t, t!("VERSIONS")));
                        ui.label(w::text(o.branches.len().to_string(), 13.0, t.text));
                        ui.end_row();
                    }
                }
            });
        });
    }

    fn queue_install(&mut self, ctx: &mut Ctx, id: AppId, library: Option<std::path::PathBuf>) {
        self.queue_install_as(ctx, id, library, String::new());
    }

    /// `platform`: the build to download ("windows", "linux"; empty keeps
    /// the installed one, else Windows).
    fn queue_install_as(&mut self, ctx: &mut Ctx, id: AppId, library: Option<std::path::PathBuf>, platform: String) {
        let s = self.games.get(&id).cloned().unwrap_or_default();
        let library = library.or_else(|| ctx.local.libraries.first().cloned()).unwrap_or_else(|| ctx.local.steam_root.clone());
        let steam_root = if ctx.local.steam_root.as_os_str().is_empty() { library.clone() } else { ctx.local.steam_root.clone() };
        let language = if s.language.is_empty() { i18n::language().steam_name().to_string() } else { s.language.clone() };
        ctx.send(Command::Install(client::download::InstallRequest {
            app_id: id,
            library,
            steam_root,
            branch: if s.branch.is_empty() { "public".into() } else { s.branch.clone() },
            language,
            skip_dlc: s.disabled_dlc.iter().copied().collect(),
            add_dlc: s.enabled_dlc.iter().copied().collect(),
            platform,
        }));
        ctx.toast(t!("Added to downloads"));
    }

    fn add_shortcut(&mut self, ctx: &mut Ctx, id: AppId, g: &InstalledGame) {
        let owned = ctx.state.apps.as_ref().and_then(|a| a.get(&id));
        match crate::links::desktop_shortcut(id, &g.name, crate::links::game_icon(owned, &g.install_dir)) {
            Ok(_) => ctx.toast(t!("Shortcut added to your desktop")),
            Err(e) => ctx.toast(e),
        }
    }

    fn install_window(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(dlg) = &mut self.install_dialog else { return };
        let id = dlg.app;
        let name = ctx.state.app_name(id).unwrap_or_default().to_string();
        let mut go = false;
        let mut close = false;
        ui::widgets::modal(ui.ctx(), egui::Id::unique(("install", id))).show(ui.ctx(), |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(150.0, 70.0), Sense::hover());
                w::image_or_skeleton(ui, t, r, &art::header(id), t.corner_small());
                ui.vertical(|ui| {
                    ui.label(w::text(t!("Install"), 13.0, t.text_dim));
                    ui.label(w::text_bold(&name, 20.0, t.text));
                });
            });
            ui.add_space(12.0);
            ui.label(w::text_medium(t!("Install to"), 12.5, t.text_dim));
            for lib in &dlg.libraries {
                let free = fs4_free(lib);
                let label = format!("{}   ·   {}", lib.display(), t!("{size} free", size = free.map(ui::bytes).unwrap_or_else(|| "?".into())));
                w::radio_value(ui, t, &mut dlg.library, lib.clone(), w::text(label, 13.5, t.text));
            }
            if w::button(ui, t, Some(icons::FOLDER), t!("Choose another folder…"), w::Kind::Ghost, w::Size::Sm).clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    // Remembered, so games installed there show up (with or
                    // without a Steam install).
                    if let Err(e) = local::add_library(&p) {
                        ctx.toast(format!("{}: {e}", p.display()));
                    }
                    dlg.libraries.push(p.clone());
                    dlg.library = p;
                }
            }
            if dlg.choose_platform {
                ui.add_space(8.0);
                ui.label(w::text_medium(t!("Version"), 12.5, t.text_dim));
                w::radio_value(ui, t, &mut dlg.platform, "windows".to_string(), w::text(t!("Windows (runs with Proton)"), 13.5, t.text));
                w::radio_value(ui, t, &mut dlg.platform, "linux".to_string(), w::text(t!("Linux (native)"), 13.5, t.text));
                if dlg.platform == "linux" {
                    ui.label(w::text(t!("Aurora's own Steam features work in the Windows version. The Linux version gets them from your Steam install."), 12.5, t.text_dim));
                }
            }
            ui.add_space(8.0);
            let size = dlg.size(ctx);
            if size > 0 {
                ui.label(w::text(t!("Space required: {size}", size = ui::bytes(size)), 13.5, t.text));
                if fs4_free(&dlg.library).is_some_and(|f| f < size) {
                    ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Not enough free space on this drive.")), 13.0, t.danger));
                }
            }
            ui.add_space(6.0);
            w::check(ui, t, &mut dlg.desktop, t!("Create a desktop shortcut"));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::DOWNLOAD), t!("Install"), w::Kind::Primary, w::Size::Md).clicked() {
                    go = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });
        if go {
            let d = self.install_dialog.take().unwrap();
            if d.desktop {
                self.shortcut_after_install.insert(id, d.library.clone());
            }
            self.queue_install_as(ctx, id, Some(d.library), d.platform);
        } else if close {
            self.install_dialog = None;
        }
    }

    fn start_verify(&mut self, ctx: &mut Ctx, g: &InstalledGame) {
        ctx.send(Command::VerifyGame { game: g.clone(), steam_root: ctx.local.steam_root.clone() });
        ctx.navigate("downloads", None);
    }

    /// Like Steam: report the check, and download damaged or missing files again.
    fn verify_finished(&mut self, ctx: &mut Ctx, id: AppId, result: &Result<local::manage::VerifyReport, String>) {
        let name = ctx.state.app_name(id).map(String::from).or_else(|| ctx.local.games.iter().find(|g| g.app_id == id).map(|g| g.name.clone())).unwrap_or_default();
        let report = match result {
            Ok(r) => r.clone(),
            Err(e) => {
                ctx.toast(i18n::tr_dyn(e).to_string());
                return;
            }
        };
        if report.cancelled {
            ctx.toast(t!("{name}: verification cancelled.", name = name));
        } else if !report.unchecked_depots.is_empty() && report.files == 0 {
            ctx.toast(t!("{name}: Steam's file list for this game isn't on this PC, so it can't be checked yet.", name = name));
        } else if report.healthy() {
            ctx.toast(t!("{name}: all {n} files are OK.", name = name, n = report.ok));
        } else {
            let bad = report.missing.len() + report.corrupt.len();
            ctx.toast(t!("{name}: {n} files failed to validate and will be downloaded again.", name = name, n = bad));
            if let Some(g) = ctx.local.games.iter().find(|g| g.app_id == id).cloned() {
                self.queue_install(ctx, id, Some(g.library.clone()));
            }
        }
        self.reports.insert(id, report);
    }

    fn news_feed_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let mut local_recent: Vec<(u64, AppId)> = ctx
            .local
            .games
            .iter()
            .filter_map(|g| g.last_played.map(|lp| (lp, g.app_id)))
            .collect();
        local_recent.sort_by_key(|(lp, _)| std::cmp::Reverse(*lp));
        let mut ids: Vec<AppId> = local_recent.into_iter().map(|(_, id)| id).collect();
        if let Some(Ok(pg)) = ctx.state.player_games.get(&ctx.state.steam_id) {
            let mut owned_recent: Vec<_> = pg.iter().collect();
            owned_recent.sort_by_key(|g| std::cmp::Reverse(g.last_played));
            for g in owned_recent {
                if !ids.contains(&g.app_id) {
                    ids.push(g.app_id);
                }
            }
        }
        for g in &ctx.local.games {
            if !ids.contains(&g.app_id) {
                ids.push(g.app_id);
            }
        }
        ids.truncate(15);
        let (posts, loading) = self.whats_new_posts(&ids);

        egui::ScrollArea::vertical().id_salt("lib-news-feed").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(22.0);
            w::centered(ui, 980.0, 28.0, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(w::caption_text(t, t!("LIBRARY")));
                        ui.label(w::text_bold(t!("Update News"), 24.0, t.text));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, t, Some(icons::BACK), t!("Back to Library"), w::Kind::Secondary, w::Size::Md).clicked() {
                            self.return_home();
                        }
                    });
                });
                ui.add_space(16.0);
                if posts.is_empty() && loading {
                    for _ in 0..4 {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 130.0), Sense::hover());
                        w::skeleton(ui, t, r, t.corner());
                        ui.add_space(10.0);
                    }
                    return;
                }
                if posts.is_empty() {
                    w::empty(ui, t, icons::NOTE, t!("No recent updates"), t!("Updates and announcements from your games will appear here."));
                    return;
                }
                for (app_id, idx, ev) in posts {
                    let game_name = ctx
                        .state
                        .app_name(app_id)
                        .map(String::from)
                        .or_else(|| ctx.local.games.iter().find(|g| g.app_id == app_id).map(|g| g.name.clone()))
                        .unwrap_or_else(|| app_id.to_string());
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 144.0), Sense::click());
                    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                    ui.painter().rect_filled(rect, t.corner(), w::mix(t.surface, t.surface_hover, hover));
                    ui.painter().rect_stroke(rect, t.corner(), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                    let img = Rect::from_min_size(rect.min + vec2(12.0, 12.0), vec2(214.0, 120.0));
                    let art_url = ev.capsule.as_deref().or(ev.header.as_deref());
                    match art_url {
                        Some(u) => w::image_or_skeleton(ui, t, img, u, t.corner_small()),
                        None => {
                            let hdr = art::header(app_id);
                            w::image_or_skeleton(ui, t, img, &hdr, t.corner_small());
                        }
                    }
                    let text_rect = Rect::from_min_max(pos2(img.right() + 16.0, rect.top() + 12.0), rect.max - vec2(14.0, 12.0));
                    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
                    c.horizontal(|ui| {
                        ui.label(w::text_bold(i18n::tr_dyn(ev.kind.label()).to_uppercase(), 11.0, news::kind_color(t, ev.kind)));
                        ui.label(w::text("·", 11.0, t.text_faint));
                        ui.label(w::text_medium(&game_name, 12.0, t.accent));
                        ui.label(w::text("·", 11.0, t.text_faint));
                        ui.label(w::text(news::date(ev.time), 11.5, t.text_faint));
                    });
                    c.add(egui::Label::new(w::text_bold(&ev.title, 16.5, t.text)).truncate());
                    c.add_space(4.0);
                    let summary: String = ev.summary.chars().take(220).collect();
                    c.add(egui::Label::new(w::text(summary, 13.0, t.text_dim)).wrap());
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.reader = Some(Reader { app: app_id, index: idx });
                    }
                    ui.add_space(12.0);
                }
            });
        });
    }

    fn hidden_games_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let rows: Vec<(AppId, String, Option<String>, Option<InstalledGame>)> = entries(ctx.state, ctx.local)
            .into_iter()
            .filter(|e| self.hidden.contains(&e.app_id))
            .map(|e| {
                let icon = e.owned.and_then(|o| o.icon.as_deref()).map(|h| art::icon(e.app_id, h));
                (e.app_id, e.name.to_owned(), icon, e.installed.cloned())
            })
            .collect();
        egui::ScrollArea::vertical().id_salt("lib-hidden").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(22.0);
            w::centered(ui, 980.0, 28.0, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(w::caption_text(t, t!("LIBRARY")));
                        ui.label(w::text_bold(t!("Hidden Games"), 24.0, t.text));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, t, Some(icons::BACK), t!("Show all games"), w::Kind::Secondary, w::Size::Md).clicked() {
                            self.return_home();
                        }
                    });
                });
                ui.add_space(16.0);
                if rows.is_empty() {
                    w::empty(ui, t, icons::EYE_SLASH, t!("No hidden games"), t!("Right-click any game in your library and choose “Hide this game” to hide it here."));
                    return;
                }
                for (id, name, icon_url, installed) in rows {
                    let playing = ctx.state.playing == Some(id);
                    egui::Frame::new()
                        .fill(t.surface)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (ir, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                                let hdr = art::header(id);
                                let urls: Vec<&str> = icon_url.as_deref().into_iter().chain([hdr.as_str()]).collect();
                                if !w::paint_cover_first(ui, ir, &urls, CornerRadius::same(4)) {
                                    ui.painter().rect_filled(ir, CornerRadius::same(4), t.raised);
                                }
                                ui.add_space(8.0);
                                ui.vertical(|ui| {
                                    ui.label(w::text_semibold(&name, 14.5, t.text));
                                    let sub = if playing {
                                        t!("Running").to_string()
                                    } else if installed.is_some() {
                                        t!("Installed").to_string()
                                    } else {
                                        t!("Not installed").to_string()
                                    };
                                    ui.label(w::text(sub, 12.0, t.text_dim));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if w::button(ui, t, Some(icons::EYE), t!("Unhide"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                        self.hidden.remove(&id);
                                    }
                                    if w::button(ui, t, None, t!("Open"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                        self.selected = Some(id);
                                    }
                                    if let Some(g) = &installed {
                                        if !playing && w::button(ui, t, Some(icons::PLAY), t!("Play"), w::Kind::Play, w::Size::Sm).clicked() {
                                            let args = self.launch_args_for(id);
                                            self.launch_or_notice(ctx, id, g.install_dir.clone(), args);
                                        }
                                    } else if w::button(ui, t, Some(icons::DOWNLOAD), t!("Install"), w::Kind::Primary, w::Size::Sm).clicked() {
                                        self.install_dialog = Some(InstallDialog::new(id, ctx));
                                    }
                                });
                            });
                        });
                    ui.add_space(8.0);
                }
            });
        });
    }

    fn soundtracks_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let mut rows: Vec<(AppId, String, Option<String>, Option<InstalledGame>)> = entries(ctx.state, ctx.local)
            .into_iter()
            .filter(|e| e.owned.is_some_and(|o| o.kind.eq_ignore_ascii_case("music")))
            .map(|e| {
                let icon = e.owned.and_then(|o| o.icon.as_deref()).map(|h| art::icon(e.app_id, h));
                (e.app_id, e.name.to_owned(), icon, e.installed.cloned())
            })
            .collect();
        rows.sort_by(|a, b| model::cmp_ignore_case(&a.1, &b.1));
        egui::ScrollArea::vertical().id_salt("lib-soundtracks").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(22.0);
            w::centered(ui, 980.0, 28.0, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(w::caption_text(t, t!("LIBRARY")));
                        ui.label(w::text_bold(t!("Soundtracks"), 24.0, t.text));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, t, Some(icons::BACK), t!("Show all games"), w::Kind::Secondary, w::Size::Md).clicked() {
                            self.return_home();
                        }
                    });
                });
                ui.add_space(16.0);
                if rows.is_empty() {
                    w::empty(ui, t, icons::SPEAKER_HIGH, t!("No soundtracks"), t!("Soundtracks you own on Steam appear here."));
                    return;
                }
                for (id, name, icon_url, installed) in rows {
                    egui::Frame::new()
                        .fill(t.surface)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (ir, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                                let hdr = art::header(id);
                                let urls: Vec<&str> = icon_url.as_deref().into_iter().chain([hdr.as_str()]).collect();
                                if !w::paint_cover_first(ui, ir, &urls, CornerRadius::same(4)) {
                                    ui.painter().rect_filled(ir, CornerRadius::same(4), t.raised);
                                }
                                ui.add_space(8.0);
                                ui.vertical(|ui| {
                                    ui.label(w::text_semibold(&name, 14.5, t.text));
                                    ui.label(w::text(if installed.is_some() { t!("Downloaded") } else { t!("Not downloaded") }, 12.0, t.text_dim));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if w::button(ui, t, None, t!("Open"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                        self.selected = Some(id);
                                    }
                                    if let Some(g) = &installed {
                                        if w::button(ui, t, Some(icons::FOLDER), t!("Browse files"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                            local::open_folder(&g.install_dir);
                                        }
                                    } else if w::button(ui, t, Some(icons::DOWNLOAD), t!("Download"), w::Kind::Primary, w::Size::Sm).clicked() {
                                        self.install_dialog = Some(InstallDialog::new(id, ctx));
                                    }
                                });
                            });
                        });
                    ui.add_space(8.0);
                }
            });
        });
    }

    /// Compact single-column game list for `View -> Small Mode`.
    pub(crate) fn show_small_mode(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        self.finish_uninstalls(ctx);
        self.downloading = ctx
            .state
            .downloads
            .values()
            .filter(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed))
            .map(|d| (d.app_id, d.paused))
            .collect();
        self.queued = ctx.state.download_queue.iter().copied().collect();

        ui.add_space(4.0);
        let width = ui.available_width();
        w::search(ui, t, &mut self.search, t!("Search games"), width);
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            for (f, label) in [(Filter::All, t!("All")), (Filter::Installed, t!("Installed")), (Filter::Favorites, icons::STAR)] {
                if w::chip(ui, t, label, self.filter == f).clicked() {
                    self.filter = f;
                }
            }
            if !self.hidden.is_empty() && w::chip(ui, t, icons::EYE, self.show_hidden).on_hover_text(t!("Show hidden games")).clicked() {
                self.show_hidden = !self.show_hidden;
            }
        });
        ui.add_space(6.0);

        let rows: Vec<(AppId, String, Option<String>, Option<InstalledGame>, bool)> = self
            .filtered(ctx)
            .into_iter()
            .map(|e| {
                let icon = e.owned.and_then(|o| o.icon.as_deref()).map(|h| art::icon(e.app_id, h));
                (e.app_id, e.name.to_owned(), icon, e.installed.cloned(), e.owned.is_some())
            })
            .collect();

        egui::ScrollArea::vertical().id_salt("lib-small-mode").auto_shrink([false, false]).show(ui, |ui| {
            if rows.is_empty() {
                ui.add_space(16.0);
                ui.label(w::text(t!("No matching games."), 13.0, t.text_faint));
            }
            for (id, name, icon_url, installed, owned) in rows {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 4.0, 32.0), Sense::click());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let sel = self.selected == Some(id);
                let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
                if sel {
                    ui.painter().rect_filled(rect, t.corner_small(), t.accent.gamma_multiply(0.18));
                } else if hover > 0.0 {
                    ui.painter().rect_filled(rect, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface, hover));
                }
                let icon = Rect::from_min_size(rect.min + vec2(6.0, 5.0), vec2(22.0, 22.0));
                let header = art::header(id);
                let urls: Vec<&str> = icon_url.as_deref().into_iter().chain([header.as_str()]).collect();
                if !w::paint_cover_first(ui, icon, &urls, CornerRadius::same(4)) {
                    ui.painter().rect_filled(icon, CornerRadius::same(4), t.surface_hover);
                }
                let setup = setup_state(ctx, id, installed.is_some());
                let busy = matches!(setup, Setup::Downloading(_) | Setup::Queued);
                let color = if busy {
                    t.accent
                } else if sel {
                    t.text
                } else if installed.is_some() {
                    w::mix(t.text_dim, t.text, hover)
                } else {
                    t.text_faint
                };
                let playing = ctx.state.playing == Some(id);
                let right_pad = if playing || busy { 56.0 } else { 20.0 };
                let mut job = egui::text::LayoutJob::simple_singleline(name.clone(), w::font_medium(13.0), color);
                job.wrap = egui::text::TextWrapping::truncate_at_width((rect.width() - 40.0 - right_pad).max(40.0));
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(pos2(icon.right() + 8.0, rect.center().y - galley.size().y / 2.0), galley, color);
                if playing {
                    ui.painter().text(rect.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, t!("Running"), w::font(11.0), t.in_game);
                } else if let Setup::Downloading(d) = &setup {
                    ui.painter().text(rect.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, format!("{}%", (d.fraction() * 100.0) as u32), w::font(11.0), t.accent);
                }
                self.game_menu(t, ctx, &resp, id, installed.is_some(), playing);
                if resp.double_clicked() {
                    if playing {
                        ctx.send(Command::StopGame { app_id: id });
                    } else if let Some(g) = &installed {
                        let args = self.launch_args_for(id);
                        self.launch_or_notice(ctx, id, g.install_dir.clone(), args);
                    } else if owned {
                        self.install_dialog = Some(InstallDialog::new(id, ctx));
                    }
                } else if resp.clicked() {
                    self.selected = Some(id);
                }
            }
        });

        self.add_dialog(ui, t, ctx);
        self.import_dialog(ui, t, ctx);
        self.reader(ui, t, ctx);
        self.install_window(ui, t, ctx);
        self.properties_window(ui, t, ctx);
        self.run_pending(ctx);
        self.naming_dialog(ui, t);
        if let Some(mut dlg) = self.storage_dialog.take()
            && dlg.show(ui, t, ctx, "library")
        {
            self.storage_dialog = Some(dlg);
        }
        if let Some((id, act)) = self.menu_request.take() {
            self.apply_menu(ctx, id, act);
        }
    }

    /// Recent news for Big Picture and the desktop library.
    pub(crate) fn whats_new_posts(&mut self, ids: &[AppId]) -> (Vec<(AppId, usize, store::events::GameEvent)>, bool) {
        let mut posts: Vec<(AppId, usize, store::events::GameEvent)> = Vec::new();
        let mut loading = false;
        for id in ids.iter().take(10) {
            match self.events(*id) {
                Some(list) => posts.extend(list.iter().take(3).cloned().enumerate().map(|(i, e)| (*id, i, e))),
                None => loading = true,
            }
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        posts.retain(|p| now.saturating_sub(p.2.time) < 45 * 86_400);
        posts.sort_by_key(|p| std::cmp::Reverse(p.2.time));
        (posts, loading)
    }

    /// What the game's main button does right now (the library page and Big
    /// Picture show the same thing).
    pub(crate) fn main_action(&mut self, ctx: &Ctx, id: AppId) -> MainAction {
        if let Some(progress) = ctx.state.moving.get(&id) {
            return MainAction::Moving(progress.fraction());
        }
        if let Some(progress) = ctx.state.backing_up.get(&id) {
            return MainAction::BackingUp(progress.fraction());
        }
        if ctx.state.install_script.contains_key(&id) || ctx.state.shader_compiling.contains_key(&id) {
            return MainAction::Preparing;
        }
        if let Some(d) = ctx.state.downloads.get(&id).filter(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)) {
            return MainAction::Busy { fraction: d.fraction(), verifying: d.verifying(), paused: d.paused };
        }
        if ctx.state.download_queue.contains(&id) {
            return MainAction::Queued;
        }
        let installed = ctx.local.games.iter().find(|g| g.app_id == id).cloned();
        match installed {
            Some(_) if self.uninstalling.contains_key(&id) => MainAction::Uninstalling,
            Some(_) if ctx.state.playing == Some(id) => MainAction::Running,
            Some(_) if ctx.state.launching == Some(id) => MainAction::Preparing,
            Some(g) if self.needs_update(ctx, &g) => MainAction::Update,
            Some(_) if ctx.state.cloud_syncing.contains(&id) => MainAction::Syncing,
            Some(_) => MainAction::Play,
            None if ctx.state.apps.as_ref().is_some_and(|a| a.contains_key(&id)) => MainAction::Install,
            None => MainAction::StorePage,
        }
    }

    /// Play, stop, update or change download state from Big Picture.
    pub(crate) fn run_main_action(&mut self, ctx: &mut Ctx, id: AppId) {
        let installed = ctx.local.games.iter().find(|g| g.app_id == id).cloned();
        match (self.main_action(ctx, id), installed) {
            (MainAction::Play, Some(g)) => self.launch_or_notice(ctx, id, g.install_dir, self.launch_args_for(id)),
            (MainAction::Running, _) => ctx.send(Command::StopGame { app_id: id }),
            (MainAction::Update, Some(g)) => self.queue_install(ctx, id, Some(g.library.clone())),
            (MainAction::Busy { paused: true, .. }, _) => ctx.send(Command::ResumeDownload { app_id: id }),
            (MainAction::Busy { verifying: false, .. }, _) => ctx.send(Command::PauseDownload { app_id: id }),
            _ => {}
        }
    }

    /// Hidden from the library by the user.
    pub(crate) fn launch_dialog_pending(&self) -> bool {
        self.cloud_hold.is_some() || matches!(self.dialog, Some(Dialog::DrmNotice { .. }))
    }

    /// Questions the library may need answered whatever page is open (a
    /// Steam Cloud conflict before a game starts): the app draws these
    /// every frame, so a launch from a shortcut, a plugin or another page
    /// isn't left waiting on a dialog that isn't shown.
    pub(crate) fn background_dialogs(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        let t = ctx.theme.clone();
        self.cloud_conflict_dialog(ui, &t, ctx);
        self.drm_notice(ui, &t, ctx);
    }

    /// Is a newer build of the installed version out? (Cached: it reads the
    /// game's manifest from disk.)
    fn needs_update(&mut self, ctx: &Ctx, g: &InstalledGame) -> bool {
        if g.state == model::InstallState::UpdateRequired {
            return true;
        }
        let now = std::time::Instant::now();
        let stale = self.builds.get(&g.app_id).is_none_or(|(t, _, _)| now.duration_since(*t).as_secs() > 10);
        if stale {
            let (build, branch) = local::manage::installed_build(g);
            self.builds.insert(g.app_id, (now, build, branch));
        }
        let (_, build, branch) = &self.builds[&g.app_id];
        let latest = ctx.state.apps.as_ref().and_then(|a| a.get(&g.app_id)).and_then(|o| o.branches.iter().find(|b| &b.name == branch)).map(|b| b.build_id).unwrap_or(0);
        *build != 0 && latest != 0 && latest != *build
    }

    fn launch_args_for(&self, id: AppId) -> client::LaunchArgs {
        let s = self.games.get(&id);
        client::LaunchArgs::user(s.map(|x| x.args.clone()).unwrap_or_default())
            .with_controller(s.map(|x| x.controller).unwrap_or(0))
    }

    fn sync_controller_overrides(&self) {
        let map = self
            .games
            .iter()
            .filter(|(_, s)| s.controller != 0)
            .map(|(id, s)| (*id, s.controller))
            .collect();
        client::set_app_controllers(map);
    }

    /// The game's main button, like Steam's: Play, Update, or what's running.
    /// Start a game, or — the first time a Steam-DRM game is launched — show
    /// the one-time notice about the background Steam instance first.
    fn launch_or_notice(&mut self, ctx: &mut Ctx, id: AppId, install_dir: PathBuf, args: client::LaunchArgs) {
        let is_drm = ctx.state.apps.as_ref().and_then(|a| a.get(&id)).is_some_and(|o| is_steam_drm(&client::launch_mode_cached(o, &install_dir)));
        let acked = self.flags.as_ref().is_some_and(|f| f.lock().unwrap().drm_host_ack);
        if is_drm && !acked {
            self.dialog = Some(Dialog::DrmNotice { id, install_dir, args });
        } else {
            ctx.send(Command::Launch { app_id: id, install_dir, args });
        }
    }

    fn play_button(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, g: &InstalledGame) {
        let id = g.app_id;
        let busy = ctx.state.downloads.get(&id).filter(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)).cloned();
        if let Some(d) = busy {
            let what = if d.verifying() { t!("Verifying…") } else { t!("Updating…") };
            let label = format!("{what} {}%", (d.fraction() * 100.0) as u32);
            if ui.add_enabled_ui(true, |ui| w::button(ui, t, Some(icons::DOWNLOAD), &label, w::Kind::Secondary, w::Size::Lg)).inner.on_hover_text(t!("Open Downloads")).clicked() {
                ctx.navigate("downloads", None);
            }
        } else if ctx.state.download_queue.contains(&id) {
            ui.add_enabled_ui(false, |ui| w::button(ui, t, Some(icons::CLOCK), t!("Queued"), w::Kind::Secondary, w::Size::Lg));
        } else if ctx.state.playing == Some(id) {
            ui.add_enabled_ui(false, |ui| w::button(ui, t, Some(icons::PLAY), t!("Running"), w::Kind::Play, w::Size::Lg));
        } else if ctx.state.launching == Some(id) {
            ui.add_enabled_ui(false, |ui| w::button(ui, t, Some(icons::CLOCK), t!("Launching…"), w::Kind::Play, w::Size::Lg));
        } else if self.needs_update(ctx, g) {
            if w::button(ui, t, Some(icons::DOWNLOAD), t!("Update"), w::Kind::Primary, w::Size::Lg).clicked() {
                self.queue_install(ctx, id, Some(g.library.clone()));
            }
        } else if w::button(ui, t, Some(icons::PLAY), t!("Play"), w::Kind::Play, w::Size::Lg).clicked() {
            let args = self.launch_args_for(id);
            self.launch_or_notice(ctx, id, g.install_dir.clone(), args);
        }
    }

    /// One-time notice before a Steam-DRM game first launches.
    fn drm_notice(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(Dialog::DrmNotice { id, install_dir, args }) = &self.dialog else { return };
        let (id, install_dir, args) = (*id, install_dir.clone(), args.clone());
        let name = ctx.state.app_name(id).unwrap_or_default().to_string();
        let steam_root = local::find_steam_root();
        let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let mut play = false;
        ui::widgets::modal(ui.ctx(), egui::Id::new(("drm-notice", id))).show(ui.ctx(), |ui| {
            ui.set_width(480.0);
            ui.label(w::text(icons::SHIELD, 28.0, t.accent));
            ui.label(w::text_bold(t!("Before {name} starts", name = name), 20.0, t.text));
            ui.add_space(6.0);
            let headless = ctx.state.online()
                && ctx.state.apps.as_ref().and_then(|a| a.get(&id)).is_some_and(|o| matches!(client::launch_mode_cached(o, &install_dir), client::LaunchMode::HeadlessHost { .. }));
            let body = if headless {
                t!("{name} gets its Steam features (and Steam DRM check) from your own Steam. Aurora starts a limited, windowless copy of it — no store, friends or web browser — signs in, and closes it again when you quit the game.", name = name)
            } else {
                t!("{name} is protected by Steam DRM, so Aurora starts the official Steam client minimized in the background (no window) and closes it again when you quit the game.", name = name)
            };
            ui.add(egui::Label::new(w::text(body, 14.0, t.text_dim)).wrap());
            ui.add_space(10.0);
            match &steam_root {
                Some(r) => {
                    ui.label(w::text(t!("Using your Steam install:"), 12.5, t.text_faint));
                    ui.label(w::text(r.display().to_string(), 12.5, t.text).monospace());
                }
                None => {
                    ui.add(egui::Label::new(w::text(format!("{}  {}", icons::WARNING, t!("No Steam install found. Install Steam and sign in to it once, then try again.")), 13.0, t.away)).wrap());
                }
            }
            ui.add_space(8.0);
            if headless {
                ui.add(egui::Label::new(w::text(t!("Close the official Steam first if it's open — only one Steam can run at a time."), 12.5, t.text_faint)).wrap());
            }
            ui.add(egui::Label::new(w::text(t!("This signs in with your own Steam account through an unofficial client; use it at your own risk."), 12.5, t.text_faint)).wrap());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let can = steam_root.is_some();
                if ui.add_enabled_ui(can, |ui| w::button(ui, t, Some(icons::PLAY), t!("Play"), w::Kind::Play, w::Size::Md)).inner.clicked() {
                    play = true;
                    close = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });
        if play {
            if let Some(f) = &self.flags {
                f.lock().unwrap().drm_host_ack = true;
            }
            ctx.send(Command::Launch { app_id: id, install_dir, args });
        }
        if close {
            self.dialog = None;
        }
    }

    fn dialogs(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId, name: &str, installed: Option<&InstalledGame>) {
        if !matches!(self.dialog, Some(Dialog::Uninstall)) {
            return;
        }
        let mut close = false;
        let Some(g) = installed else {
            self.dialog = None;
            return;
        };
        if self.uninstalling.contains_key(&id) {
            self.dialog = None;
            return;
        }
        let running = ctx.state.playing == Some(id);
        ui::widgets::modal(ui.ctx(), egui::Id::unique(("uninstall", id))).show(ui.ctx(), |ui| {
            ui.set_width(460.0);
            ui.label(w::text(icons::TRASH, 28.0, t.danger));
            ui.label(w::text_bold(t!("Uninstall {name}?", name = name), 20.0, t.text));
            ui.add_space(4.0);
            ui.add(egui::Label::new(w::text(t!("This deletes {size} from:", size = ui::bytes(g.size_on_disk)), 14.0, t.text_dim)).wrap());
            ui.label(w::text(g.install_dir.display().to_string(), 13.0, t.text).monospace());
            ui.add_space(4.0);
            ui.add(egui::Label::new(w::text(t!("Saves kept inside the game folder are deleted too. Steam Cloud saves aren't affected."), 12.5, t.text_faint)).wrap());
            if running {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Close the game first.")), 13.0, t.away));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let go = ui.add_enabled_ui(!running, |ui| w::button(ui, t, Some(icons::TRASH), t!("Uninstall"), w::Kind::Danger, w::Size::Md)).inner.clicked();
                if go {
                    // Deleting a big game takes a while: do it in the
                    // background, with progress on the game's button.
                    let job = Uninstalling { name: name.to_string(), progress: Arc::new(Progress::default()), result: Arc::new(Mutex::new(None)) };
                    let (game, progress, result, egui_ctx) = (g.clone(), job.progress.clone(), job.result.clone(), ui.ctx().clone());
                    std::thread::spawn(move || {
                        let r = local::manage::uninstall_with(&game, &progress);
                        *result.lock().unwrap() = Some(r);
                        egui_ctx.request_repaint();
                    });
                    self.uninstalling.insert(id, job);
                    close = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.dialog = None;
        }
    }
}

/// A game being uninstalled in the background.
struct Uninstalling {
    name: String,
    progress: Arc<Progress>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

impl Library {
    /// Report uninstalls that ended, and refresh the installed games.
    fn finish_uninstalls(&mut self, ctx: &mut Ctx) {
        let done: Vec<AppId> = self.uninstalling.iter().filter(|(_, j)| j.result.lock().unwrap().is_some()).map(|(id, _)| *id).collect();
        for id in done {
            let Some(job) = self.uninstalling.remove(&id) else { continue };
            let name = job.name;
            match job.result.lock().unwrap().take() {
                Some(Ok(())) => {
                    // Its desktop shortcut would only lead nowhere now.
                    if crate::links::remove_desktop_shortcuts(id) > 0 {
                        ctx.toast(t!("{name} was uninstalled and its desktop shortcut removed", name = name));
                    } else {
                        ctx.toast(t!("{name} was uninstalled", name = name));
                    }
                }
                Some(Err(e)) => ctx.toast(e),
                None => {}
            }
            ctx.act(extensions::Action::ReloadLocal);
        }
    }
}

/// The play button while a game is being deleted: red, filling up, like Steam.
/// The Play button while shaders compile: Play's green, a spinner and the
/// percentage inside, and a lighter fill that grows with the progress.
fn compile_bar(ui: &mut Ui, t: &Theme, done: u32, total: u32) {
    let fraction = if total > 0 { done.min(total) as f32 / total as f32 } else { 0.0 };
    let label = if total > 0 {
        t!("Compiling shaders… {pct}%", pct = (fraction * 100.0).floor() as u32)
    } else {
        t!("Compiling shaders…").to_string()
    };
    busy_bar(ui, t, label, fraction);
}

/// The Play button's place while something runs before the game starts.
fn busy_bar(ui: &mut Ui, t: &Theme, label: String, fraction: f32) {
    let galley = ui.painter().layout_no_wrap(label, w::font_semibold(16.0), Color32::WHITE);
    let spinner = 18.0;
    let (rect, _) = ui.allocate_exact_size(vec2((galley.size().x + spinner + 58.0).max(240.0), 48.0), Sense::hover());
    let r = t.corner_small();
    let p = ui.painter();
    p.rect_filled(rect, r, t.play);
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
    p.rect_filled(fill, r, Color32::WHITE.gamma_multiply(0.18));
    let x = rect.center().x - (spinner + 10.0 + galley.size().x) / 2.0;
    let spin = Rect::from_center_size(pos2(x + spinner / 2.0, rect.center().y), vec2(spinner, spinner));
    egui::Spinner::new().size(spinner).color(Color32::WHITE).paint_at(ui, spin);
    ui.painter().galley(pos2(x + spinner + 10.0, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
}

fn uninstall_bar(ui: &mut Ui, t: &Theme, fraction: f32) {
    let label = t!("Uninstalling… {pct}%", pct = (fraction.clamp(0.0, 1.0) * 100.0).round() as u32);
    let galley = ui.painter().layout_no_wrap(format!("{}  {label}", icons::TRASH), w::font_semibold(16.0), t.danger);
    let (rect, _) = ui.allocate_exact_size(vec2((galley.size().x + 48.0).max(220.0), 48.0), Sense::hover());
    let r = t.corner_small();
    let p = ui.painter();
    p.rect_filled(rect, r, t.danger.gamma_multiply(0.14));
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
    p.rect_filled(fill, r, t.danger.gamma_multiply(0.35));
    p.rect_stroke(rect, r, Stroke::new(1.0, t.danger.gamma_multiply(0.5)), egui::StrokeKind::Inside);
    p.galley(pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0), galley, t.danger);
}

/// Checkboxes in `cols` columns; returns the index of the one clicked.
fn check_grid(ui: &mut Ui, t: &Theme, id: &str, cols: usize, items: impl Iterator<Item = (bool, String)>) -> Option<usize> {
    let mut hit = None;
    let col_w = (ui.available_width() - 16.0 * (cols as f32 - 1.0)) / cols as f32;
    egui::Grid::new(id).num_columns(cols).spacing(vec2(16.0, 2.0)).min_col_width(col_w).max_col_width(col_w).show(ui, |ui| {
        for (i, (on, label)) in items.enumerate() {
            let mut v = on;
            if w::check(ui, t, &mut v, w::text(label, 13.0, t.text)).changed() {
                hit = Some(i);
            }
            if i % cols == cols - 1 {
                ui.end_row();
            }
        }
    });
    hit
}

fn fs4_free(path: &std::path::Path) -> Option<u64> {
    fs4::available_space(path).ok()
}

pub(crate) struct InstallDialog {
    app: AppId,
    libraries: Vec<std::path::PathBuf>,
    library: std::path::PathBuf,
    desktop: bool,
    /// The build to download, "windows" or "linux".
    platform: String,
    /// Offer the choice: a Linux system and a game with both builds.
    choose_platform: bool,
}

impl InstallDialog {
    pub(crate) fn new(app: AppId, ctx: &Ctx) -> Self {
        let mut libraries = ctx.local.libraries.clone();
        if libraries.is_empty() {
            let default = client::store::config_dir().join("Library");
            let _ = std::fs::create_dir_all(default.join("steamapps"));
            libraries.push(default);
        }
        let owned = ctx.state.apps.as_ref().and_then(|a| a.get(&app));
        let choose_platform = cfg!(not(windows)) && owned.is_some_and(|o| client::download::has_build(o, "linux") && client::download::has_build(o, "windows"));
        // Pick the library with the most free space.
        let library = libraries.iter().max_by_key(|l| fs4_free(l).unwrap_or(0)).cloned().unwrap_or_default();
        Self { app, libraries, library, desktop: true, platform: "windows".into(), choose_platform }
    }

    /// Download size of the chosen build (base game, no DLC).
    fn size(&self, ctx: &Ctx) -> u64 {
        let os = self.platform.as_str();
        ctx.state
            .apps
            .as_ref()
            .and_then(|a| a.get(&self.app))
            .map(|o| o.depots.iter().filter(|d| d.dlc_app.is_none() && !d.shared_install && (d.os.is_empty() || d.os.split(',').any(|x| x.trim() == os))).filter_map(|d| d.manifest("public").map(|m| m.1)).sum())
            .unwrap_or(0)
    }
}


/// "Library home": a solid grey button, a lighter grey while hovered. It is
/// never transparent, so it reads as a button whether or not it is open.
fn home_button(ui: &mut Ui, t: &Theme, selected: bool, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, 0.15);
    let base = w::mix(t.raised, t.surface_hover, sel);
    let fill = w::mix(base, w::mix(t.surface_hover, t.text, 0.12), hover);
    let color = w::mix(t.text_dim, t.text, sel.max(hover));
    let p = ui.painter();
    p.rect_filled(rect, t.corner_small(), fill);
    let galley = p.layout_no_wrap(format!("{}   {}", icons::HOUSE, t!("Library home")), w::font_semibold(14.0), color);
    p.galley(rect.center() - galley.size() / 2.0, galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// App types the library shows by default. Unknown types count as games.
fn is_game_kind(kind: &str) -> bool {
    matches!(kind, "game" | "demo" | "")
}

/// Linux: "Compatibility" in a game's Manage menu, like Steam's Properties >
/// Compatibility: run it with Steam's/the global choice, or force one of the
/// installed Proton builds for this game only. Games with a Linux build also
/// get "Native", the default for them.
#[cfg(not(windows))]
fn proton_submenu(ui: &mut Ui, t: &Theme, ctx: &Ctx, id: AppId) {
    let builds = client::proton_builds();
    // (runs natively without a choice, the Windows build is installed too)
    let native = ctx
        .state
        .apps
        .as_ref()
        .and_then(|a| a.get(&id))
        .zip(ctx.local.games.iter().find(|g| g.app_id == id))
        .and_then(|(o, g)| client::native_build(o, &g.install_dir));
    if builds.is_empty() && native.is_none() {
        return;
    }
    let row = w::submenu_item(ui, t, icons::WINDOWS, t!("Compatibility"));
    SubMenu::new().show(ui, &row, |ui| {
        ui.set_min_width(220.0);
        w::menu_spacing(ui);
        ui.add_space(2.0);
        let heading = if native.is_some() { t!("Run with:") } else { t!("Run with this Proton:") };
        ui.label(w::text(heading, 12.0, t.text_faint));
        ui.add_space(2.0);
        let current = client::app_proton(id);
        let check = |on: bool| if on { icons::CHECK } else { "" };
        let default = match client::default_proton(id) {
            Some(d) => t!("Default ({tool})", tool = d),
            None => t!("Default").to_string(),
        };
        let pick = |ui: &mut Ui, choice: Option<&str>| {
            client::set_app_proton(id, choice.map(String::from));
            ui.close();
        };
        match native {
            Some((by_default, windows)) => {
                // No choice here = whatever runs by default; picking that is
                // the same as clearing the choice.
                let native_on = current.as_deref() == Some(client::NATIVE) || (current.is_none() && by_default);
                if w::menu_item(ui, t, check(native_on), t!("Native (Linux)")).clicked() {
                    pick(ui, (!by_default).then_some(client::NATIVE));
                }
                if !windows {
                    // Only the Linux files are on disk: Proton has nothing to run.
                    return;
                }
                let proton_on = current.as_deref() == Some(client::DEFAULT_PROTON) || (current.is_none() && !by_default);
                if w::menu_item(ui, t, check(proton_on), &default).clicked() {
                    pick(ui, by_default.then_some(client::DEFAULT_PROTON));
                }
            }
            None => {
                if w::menu_item(ui, t, check(current.is_none()), &default).clicked() {
                    pick(ui, None);
                }
            }
        }
        if builds.is_empty() {
            return;
        }
        w::menu_separator(ui, t);
        for (name, display) in &builds {
            let on = current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name));
            if w::menu_item(ui, t, check(on), display).clicked() {
                pick(ui, Some(name));
            }
        }
    });
}

/// One collapsible section in the library sidebar (a collection, Favorites,
/// Uncategorized, Installed, etc.).
struct SidebarGroup<'a> {
    key: String,
    label: String,
    collection: Option<usize>,
    games: Vec<&'a Entry<'a>>,
}

#[derive(Clone, Copy)]
enum GroupMenuAct {
    Rename(usize),
    Delete(usize),
    CollapseAll,
    ExpandAll,
}

fn collection_key(c: &Collection) -> String {
    match &c.steam_id {
        Some(id) if !id.is_empty() => format!("col:{id}"),
        _ => format!("col-name:{}", c.name),
    }
}

fn sidebar_group_header(ui: &mut Ui, t: &Theme, title: &str, count: usize, open: bool, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    if hover > 0.0 {
        ui.painter().rect_filled(rect, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface, hover));
    }
    let caret = if open { icons::CARET_DOWN } else { icons::CARET_RIGHT };
    let caret_color = w::mix(t.text_faint, t.text_dim, hover);
    ui.painter().text(pos2(rect.left() + 10.0, rect.center().y), Align2::CENTER_CENTER, caret, w::font(11.0), caret_color);
    let label_color = w::mix(t.text_dim, t.text, hover);
    let count_text = format!(" · {count}");
    let count_galley = ui.painter().layout_no_wrap(count_text, w::font_semibold(12.0), t.text_faint);
    let text_left = rect.left() + 20.0;
    let max_name_w = (rect.right() - text_left - count_galley.size().x - 6.0).max(24.0);
    let mut name_job = egui::text::LayoutJob::simple_singleline(title.to_owned(), w::font_semibold(12.5), label_color);
    name_job.wrap = egui::text::TextWrapping::truncate_at_width(max_name_w);
    let name_galley = ui.painter().layout_job(name_job);
    let count_x = text_left + name_galley.size().x;
    ui.painter().galley(pos2(text_left, rect.center().y - name_galley.size().y / 2.0), name_galley, label_color);
    ui.painter().galley(pos2(count_x, rect.center().y - count_galley.size().y / 2.0), count_galley, t.text_faint);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn group_context_menu(resp: &egui::Response, collection: Option<usize>, total_groups: usize, out: &mut Option<GroupMenuAct>) {
    if collection.is_none() && total_groups <= 1 {
        return;
    }
    resp.context_menu(|ui| {
        if let Some(i) = collection {
            if ui.button(format!("{}   {}", icons::NOTE, t!("Rename"))).clicked() {
                *out = Some(GroupMenuAct::Rename(i));
                ui.close();
            }
            if ui.button(format!("{}   {}", icons::TRASH, t!("Delete collection"))).clicked() {
                *out = Some(GroupMenuAct::Delete(i));
                ui.close();
            }
            if total_groups > 1 {
                ui.separator();
            }
        }
        if total_groups > 1 {
            if ui.button(t!("Collapse all")).clicked() {
                *out = Some(GroupMenuAct::CollapseAll);
                ui.close();
            }
            if ui.button(t!("Expand all")).clicked() {
                *out = Some(GroupMenuAct::ExpandAll);
                ui.close();
            }
        }
    });
}

/// Paint a 1–4 cover collage for a collection card, like Steam's Your Collections view.
fn paint_collection_collage(ui: &Ui, t: &Theme, rect: Rect, games: &[AppId], radius: CornerRadius) {
    ui.painter().rect_filled(rect, radius, t.raised);
    if games.is_empty() {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, icons::STACK, w::font(44.0), t.surface_hover);
        return;
    }
    let cells: Vec<Rect> = match games.len() {
        1 => vec![rect],
        2 => {
            let mid = rect.center().x;
            vec![
                Rect::from_min_max(rect.min, pos2(mid, rect.bottom())),
                Rect::from_min_max(pos2(mid, rect.top()), rect.max),
            ]
        }
        _ => {
            let mid_x = rect.center().x;
            let mid_y = rect.center().y;
            vec![
                Rect::from_min_max(rect.min, pos2(mid_x, mid_y)),
                Rect::from_min_max(pos2(mid_x, rect.top()), pos2(rect.right(), mid_y)),
                Rect::from_min_max(pos2(rect.left(), mid_y), pos2(mid_x, rect.bottom())),
                Rect::from_min_max(pos2(mid_x, mid_y), rect.max),
            ]
        }
    };
    for (idx, cell) in cells.into_iter().enumerate() {
        let Some(&id) = games.get(idx).or_else(|| games.first()) else { continue };
        let cell_radius = if games.len() == 1 { radius } else { CornerRadius::ZERO };
        w::paint_cover_first(ui, cell, &[&art::capsule(id), &art::header(id)], cell_radius);
    }
}

/// Compute pixel-aligned widths for the top 4 filter chips (`All`, `Installed`,
/// `★`, `Played`) and the 2 buttons below them (`Filters`, `Collections`) so the
/// seam between `Filters` and `Collections` lines up with the seam between
/// `Installed` and `★`.
fn sidebar_filter_widths(
    top: [f32; 4],
    bottom: Option<[f32; 2]>,
    width: f32,
    gap: f32,
) -> (Option<[f32; 4]>, Option<[f32; 2]>) {
    let top_sum: f32 = top.iter().sum::<f32>() + gap * 3.0;
    if top_sum > width {
        return (None, None);
    }
    let extra = (width - top_sum) / 4.0;
    let mut tw = [top[0] + extra, top[1] + extra, top[2] + extra, top[3] + extra];
    let Some([b0, b1]) = bottom else {
        return (Some(tw), None);
    };
    if b0 + gap + b1 > width {
        return (Some(tw), None);
    }
    let left_span = tw[0] + gap + tw[1];
    let right_span = tw[2] + gap + tw[3];
    let left_min = (top[0] + gap + top[1]).max(b0);
    let right_min = (top[2] + gap + top[3]).max(b1);
    if right_span < right_min && left_span > left_min {
        let shift = (right_min - right_span).min(left_span - left_min);
        tw[0] -= shift * 0.5;
        tw[1] -= shift * 0.5;
        tw[2] += shift * 0.5;
        tw[3] += shift * 0.5;
    } else if left_span < left_min && right_span > right_min {
        let shift = (left_min - left_span).min(right_span - right_min);
        tw[0] += shift * 0.5;
        tw[1] += shift * 0.5;
        tw[2] -= shift * 0.5;
        tw[3] -= shift * 0.5;
    }
    let final_left = tw[0] + gap + tw[1];
    let final_right = tw[2] + gap + tw[3];
    if final_left + 0.5 >= b0 && final_right + 0.5 >= b1 {
        (Some(tw), Some([final_left, final_right]))
    } else {
        (Some(tw), None)
    }
}

/// Chips in rows that fill `width`, like the library's filter chips: each
/// row shares its spare room between its chips.
fn chip_rows(ui: &mut Ui, t: &Theme, labels: &[(String, bool)], width: f32, gap: f32) -> Vec<egui::Response> {
    let widths: Vec<f32> = labels.iter().map(|(l, _)| w::chip_width(ui, l)).collect();
    let mut rows: Vec<std::ops::Range<usize>> = Vec::new();
    let (mut start, mut used) = (0, 0.0);
    for (i, cw) in widths.iter().enumerate() {
        if i > start && used + gap + cw > width {
            rows.push(start..i);
            start = i;
            used = *cw;
        } else {
            used += if i > start { gap + cw } else { *cw };
        }
    }
    if start < labels.len() {
        rows.push(start..labels.len());
    }
    let mut out = Vec::with_capacity(labels.len());
    for row in rows {
        let natural: f32 = widths[row.clone()].iter().sum::<f32>() + gap * (row.len() - 1) as f32;
        let extra = ((width - natural) / row.len() as f32).max(0.0);
        ui.horizontal(|ui| {
            for i in row {
                out.push(w::wide_chip(ui, t, &labels[i].0, labels[i].1, widths[i] + extra));
            }
        });
    }
    out
}

/// `s` cut to `max` characters, with an ellipsis.
fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max - 1).collect::<String>()) }
}


fn paint_featured_pane(ui: &Ui, t: &Theme, rect: Rect, radius: u8) {
    ui.painter_at(rect).rect_filled(rect, CornerRadius { nw: 0, ne: 0, sw: radius, se: radius }, t.raised.gamma_multiply(225.0 / 255.0));
    ui.painter_at(rect).hline(rect.x_range(), rect.top(), Stroke::new(1.0, t.text.gamma_multiply(24.0 / 255.0)));
}

fn paint_featured_playtime(ui: &mut Ui, t: &Theme, bar_rect: Rect, btn_rect: Rect, played_str: &str, recent: Option<&str>) {
    let text_x = btn_rect.right() + 14.0;
    let text_w = (bar_rect.right() - text_x - 10.0).max(0.0);
    if text_w < 24.0 { return; }
    let info_rect = Rect::from_min_max(pos2(text_x, bar_rect.top() + 14.0), pos2(text_x + text_w, bar_rect.bottom() - 10.0));
    let mut info_ui = ui.new_child(egui::UiBuilder::new().max_rect(info_rect).layout(Layout::top_down(Align::Min)));
    info_ui.set_clip_rect(ui.clip_rect().intersect(info_rect));
    info_ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    info_ui.spacing_mut().item_spacing.y = 2.0;
    info_ui.label(w::text_semibold(t!("TIME PLAYED").to_uppercase(), 10.5, t.text_dim));
    if let Some(recent) = recent { info_ui.label(w::text(recent, 12.0, t.text_dim)); }
    info_ui.label(w::text(format!("Total: {played_str}"), 12.0, t.text));
}

/// Draw a game capsule or banner card with an explicit `card_radius` (e.g. `0` for sharp Steam corners).
fn themed_game_tile(
    ui: &mut Ui,
    t: &Theme,
    url: &str,
    fallback_url: &str,
    title: &str,
    installed: bool,
    size: egui::Vec2,
    card_radius: u8,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let r = CornerRadius::same(card_radius);
    let img = rect.expand(hover * 2.5);
    // Subtle resting drop shadow + deeper hover shadow, like Steam's capsules.
    ui.painter().rect_filled(img.translate(vec2(0.0, 3.0 + 4.0 * hover)), r, Color32::from_black_alpha((55.0 + 60.0 * hover) as u8));
    if !w::paint_cover_first(ui, img, &[url, fallback_url], r) {
        w::placeholder(ui, t, img, title, r);
    }
    if !installed {
        ui.painter().rect_filled(img, r, Color32::from_black_alpha(55));
    }
    if hover > 0.01 {
        ui.painter().rect_stroke(img, r, Stroke::new(1.5, t.accent.gamma_multiply(hover)), egui::StrokeKind::Outside);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// Account history may arrive after the local library, or contain only part of it.
// Keep both sources, one card per app, ordered by the freshest known play time.
fn merge_recent_games(
    account: impl IntoIterator<Item = (AppId, String, u64)>,
    local: impl IntoIterator<Item = (AppId, String, u64)>,
) -> Vec<(AppId, String, u64)> {
    let mut games: BTreeMap<AppId, (String, u64)> = BTreeMap::new();
    for (id, name, time) in account.into_iter().chain(local) {
        games.entry(id).and_modify(|g| g.1 = g.1.max(time)).or_insert((name, time));
    }
    let mut games: Vec<_> = games.into_iter().map(|(id, (name, time))| (id, name, time)).collect();
    games.sort_by_key(|g| (std::cmp::Reverse(g.2), g.0));
    games
}

fn themed_recent_games(ctx: &Ctx) -> Vec<(AppId, String, u64)> {
    let account = ctx.state.player_games.get(&ctx.state.steam_id)
        .and_then(|r| r.as_ref().ok()).into_iter().flatten()
        .filter_map(|g| Some((g.app_id, g.name.clone(), g.last_played?)));
    let local = ctx.local.games.iter()
        .filter_map(|g| Some((g.app_id, g.name.clone(), g.last_played?)));
    merge_recent_games(account, local)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PlayedGroup {
    Recent,
    Month(std::cmp::Reverse<(i64, u32)>),
    Never,
}

fn played_group(time: Option<u64>, now: u64) -> PlayedGroup {
    match time.filter(|t| *t > 0) {
        None => PlayedGroup::Never,
        Some(time) if now.saturating_sub(time) < 14 * 86_400 => PlayedGroup::Recent,
        Some(time) => {
            let (year, month, _) = ui::calendar_date(time);
            PlayedGroup::Month(std::cmp::Reverse((year, month)))
        }
    }
}

impl PlayedGroup {
    fn label(self, current_year: i64) -> String {
        match self {
            Self::Recent => t!("Recent").to_string(),
            Self::Never => t!("Never played").to_string(),
            Self::Month(std::cmp::Reverse((year, month))) => {
                let months = [t!("January"), t!("February"), t!("March"), t!("April"), t!("May"), t!("June"), t!("July"), t!("August"), t!("September"), t!("October"), t!("November"), t!("December")];
                let name = months[(month - 1) as usize];
                if year == current_year { name.to_string() } else { format!("{name} {year}") }
            }
        }
    }
}

// The header arrows page the actual scroll area rather than dropping excess cards.
fn themed_shelf_header(ui: &mut Ui, t: &Theme, title: &str, line: bool, on_hide: Option<&mut bool>) -> f32 {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let g = ui.painter().layout_no_wrap(i18n::tr_dyn(title), w::font(16.0), t.text);
    let end = r.left() + g.size().x;
    ui.painter().galley(r.left_center() - vec2(0.0, g.size().y * 0.5), g, t.text);
    let mut right_edge = r.right() - 80.0;
    if let Some(hide) = on_hide {
        let hide_label = t!("Hide");
        let hg = ui.painter().layout_no_wrap(hide_label.to_string(), w::font(12.5), t.text_faint);
        let hw = hg.size().x;
        let hide_rect = Rect::from_min_size(pos2(r.right() - 78.0 - hw, r.top()), vec2(hw, 24.0));
        let resp = ui.interact(hide_rect, ui.scope_id().with((title, "hide")), Sense::click());
        let fg = if resp.hovered() { t.text } else { t.text_faint };
        let hg = ui.painter().layout_no_wrap(hide_label.to_string(), w::font(12.5), fg);
        let text_pos = pos2(hide_rect.left(), hide_rect.center().y - hg.size().y * 0.5);
        ui.painter().galley(text_pos, hg, fg);
        if resp.hovered() {
            ui.painter().hline(hide_rect.x_range(), hide_rect.bottom() - 3.0, Stroke::new(1.0, fg));
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            *hide = true;
        }
        right_edge = hide_rect.left() - 16.0;
    }
    if line {
        ui.painter().hline((end + 20.0)..=right_edge.max(end + 20.0), r.center().y,
            Stroke::new(6.0, t.surface_hover));
    }
    let mut delta = 0.0;
    for (i, sign) in [-1.0, 1.0].into_iter().enumerate() {
        let rect = Rect::from_min_size(pos2(r.right() - 64.0 + i as f32 * 36.0, r.top()), vec2(28.0, 24.0));
        let resp = ui.interact(rect, ui.scope_id().with((title, "page", i)), Sense::click());
        let fg = if resp.hovered() { t.text } else { t.text_faint };
        let c = rect.center();
        let dx = -sign * 4.0;
        ui.painter().add(egui::Shape::line(vec![c + vec2(dx, -8.0), c - vec2(dx, 0.0), c + vec2(dx, 8.0)], Stroke::new(1.5, fg)));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            delta = sign * (r.width() * 0.8);
        }
    }
    delta
}

fn themed_shelf_scroll(ui: &Ui, salt: &str, delta: f32) -> egui::ScrollArea {
    let mut scroll = egui::ScrollArea::horizontal().id_salt(salt).auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
    if delta != 0.0 {
        let offset = egui::scroll_area::State::load(ui.ctx(), ui.make_persistent_id(salt))
            .map(|s| s.clamped_offset().x).unwrap_or(0.0);
        scroll = scroll.horizontal_scroll_offset((offset + delta).max(0.0));
    }
    scroll
}

impl crate::theme_renderer::LibraryHost for Library {
    fn is_detail_open(&self) -> bool {
        self.custom_selected.is_some()
            || self.selected.is_some()
            || self.collections_view.is_some()
            || self.news_view
            || self.hidden_only
            || self.soundtracks_only
    }

    fn is_collections_open(&self) -> bool {
        self.collections_view.is_some()
    }

    fn whats_new_enabled(&self) -> bool {
        Library::whats_new_enabled(self)
    }

    fn render_detail(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        match (self.collections_view, self.custom_selected, self.selected) {
            (Some(CollectionsView::Overview), _, _) => self.collections_overview(ui, t, ctx),
            (Some(CollectionsView::Detail(i)), _, _) => self.collection_detail(i, ui, t, ctx),
            (None, Some(i), _) => self.custom_page(i, ui, t, ctx),
            (None, None, Some(id)) => self.game_page(id, ui, t, ctx),
            (None, None, None) if self.news_view => self.news_feed_page(ui, t, ctx),
            (None, None, None) if self.hidden_only => self.hidden_games_page(ui, t, ctx),
            (None, None, None) if self.soundtracks_only => self.soundtracks_page(ui, t, ctx),
            (None, None, None) => {}
        }
    }

    fn handle_library_action(&mut self, action: &str, ctx: &mut Ctx) {
        if action == "navigate:library" {
            self.return_home();
        } else if action == "library:collections" {
            self.toggle_collections_overview();
        } else if action == "add_game" {
            self.adding = Some(CustomGame::default());
        } else if let Some(rest) = action.strip_prefix("navigate:") {
            if let Some((page, arg)) = rest.split_once(':') {
                ctx.navigate(page, Some(arg.into()));
            } else {
                ctx.navigate(rest, None);
            }
        } else if let Some(url) = action.strip_prefix("open_url:") {
            crate::links::open_http(url);
        }
    }

    fn render_action_popup(&mut self, _ui: &mut Ui, _t: &Theme, _response: &egui::Response, _action: &str) {}

    fn render_search(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        ctx: &mut Ctx,
        placeholder: Option<&str>,
        show_filter_button: bool,
        style: &themes::NodeStyle,
    ) {
        let h = match style.height {
            Some(themes::SizeVal::Px(px)) => px,
            _ => 32.0,
        };
        let bg = style.bg.map(|c| c.resolve(t)).unwrap_or(t.chrome);
        let radius = CornerRadius::same(style.radius.unwrap_or(2));
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        ui.painter().rect_filled(rect, radius, bg);
        ui.painter().text(pos2(rect.left() + 13.0, rect.center().y), Align2::CENTER_CENTER,
            icons::SEARCH, w::font(16.0), t.text_faint);
        let button_w = if show_filter_button { 30.0 } else { 0.0 };
        let input = Rect::from_min_max(rect.min + vec2(26.0, 5.0), rect.max - vec2(button_w + 4.0, 3.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input));
        child.add(egui::TextEdit::singleline(&mut self.search)
            .hint_text(w::text(i18n::tr_dyn(placeholder.unwrap_or("Search")), 13.0, t.text_faint))
            .font(w::font(13.0)).frame(egui::Frame::NONE).desired_width(input.width()))
            .with_edit_menu();
        if show_filter_button {
            let frect = Rect::from_min_size(pos2(rect.right() - button_w, rect.top()), vec2(button_w, h));
            let fresp = ui.interact(frect, ui.scope_id().with("library-filters"), Sense::click());
            let fg = if self.adv.count() > 0 { t.accent } else if fresp.hovered() { t.text } else { t.text_dim };
            // Steam's filter is three narrowing horizontal rules, inset in the search field.
            for (dy, width) in [(-5.0, 18.0), (0.0, 12.0), (5.0, 6.0)] {
                let c = frect.center() + vec2(0.0, dy);
                ui.painter().line_segment([c - vec2(width * 0.5, 0.0), c + vec2(width * 0.5, 0.0)], Stroke::new(2.0, fg));
            }
            let fresp = fresp.on_hover_text(t!("Filters")).on_hover_cursor(egui::CursorIcon::PointingHand);
            egui::Popup::from_toggle_button_response(&fresp)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .width(720.0).show(|ui| self.adv_panel(ui, t, ctx));
        }
    }

    fn render_filters(&mut self, ui: &mut Ui, t: &Theme, _ctx: &mut Ctx, variant: &str, _style: &themes::NodeStyle) {
        let avail_w = ui.available_width();
        match variant {
            "dropdown_icons" => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let current_label = match self.filter {
                        Filter::All => t!("Games and Software").to_string(),
                        Filter::Installed => t!("Installed").to_string(),
                        Filter::Playing => t!("Played").to_string(),
                        Filter::Favorites => t!("Favorites").to_string(),
                        Filter::Collection(i) => self.collections.get(i).map(|c| c.name.clone()).unwrap_or_else(|| t!("Collection").to_string()),
                    };
                    let drop_w = (avail_w - 56.0).max(80.0);
                    let (drect, dresp) = ui.allocate_exact_size(vec2(drop_w, 32.0), Sense::click());
                    let dhover = ui.ctx().animate_bool_with_time(dresp.id, dresp.hovered(), 0.08);
                    ui.painter().rect_filled(drect, CornerRadius::same(2), w::mix(t.chrome, t.surface_hover, dhover * 0.5));
                    let txt = short(&current_label, 24);
                    ui.painter().text(drect.right_center() - vec2(10.0, 0.0), Align2::CENTER_CENTER,
                        icons::CARET_DOWN, w::font(11.0), t.text_dim);
                    ui.painter().text(
                        pos2(drect.left() + 8.0, drect.center().y),
                        Align2::LEFT_CENTER,
                        txt,
                        w::font(12.5),
                        w::mix(t.text_dim, t.text, dhover),
                    );
                    let dresp = dresp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    egui::Popup::menu(&dresp).width(200.0).show(|ui| {
                        if w::menu_item(ui, t, icons::BOOKS, t!("Games and Software")).clicked() {
                            self.filter = Filter::All;
                        }
                        if w::menu_item(ui, t, icons::STAR, t!("Favorites")).clicked() {
                            self.filter = Filter::Favorites;
                        }
                        for (i, c) in self.collections.iter().enumerate() {
                            if w::menu_item(ui, t, icons::STACK, &c.name).clicked() {
                                self.filter = Filter::Collection(i);
                            }
                        }
                    });

                    let installed_on = self.filter == Filter::Installed;
                    let recent_on = self.filter == Filter::Playing;
                    let circle_toggle = |ui: &mut Ui, icon: &str, tip: &str, active: bool| -> bool {
                        let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                        let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.08);
                        let fg = if active { t.accent } else { w::mix(t.text_dim, t.text, h) };
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, w::font(11.5), fg);
                        resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                    };
                    if circle_toggle(ui, icons::PLAY, t!("Ready to play"), installed_on) {
                        self.filter = if installed_on { Filter::All } else { Filter::Installed };
                    }
                    if circle_toggle(ui, icons::CLOCK, t!("Played"), recent_on) {
                        self.filter = if recent_on { Filter::All } else { Filter::Playing };
                    }
                });
            }
            "minimal" => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (f, label) in [(Filter::All, t!("All")), (Filter::Installed, t!("Installed")), (Filter::Favorites, icons::STAR)] {
                        let sel = self.filter == f;
                        let r = ui.add(egui::Label::new(w::text_medium(label, 12.0, if sel { t.accent } else { t.text_dim })).sense(Sense::click()));
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.filter = f;
                        }
                    }
                });
            }
            _ => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (f, label) in [(Filter::All, t!("All")), (Filter::Installed, t!("Installed")), (Filter::Favorites, icons::STAR), (Filter::Playing, t!("Played"))] {
                        if w::chip(ui, t, label, self.filter == f).clicked() {
                            self.filter = f;
                        }
                    }
                });
            }
        }
    }

    fn render_list(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        ctx: &mut Ctx,
        row_height: f32,
        group_by: themes::LibraryGrouping,
        icon_size: f32,
        group_headers: bool,
        show_counts: bool,
        style: &themes::NodeStyle,
    ) {
        if self.hide_collections && matches!(self.filter, Filter::Collection(_)) {
            self.filter = Filter::All;
        }
        let items = self.filtered(ctx);
        let searching = !self.search.trim().is_empty();
        let groups = self.sidebar_groups(&items, ctx, group_by, t!("All games"), !self.hide_collections && group_headers);
        let mut all_keys: Vec<String> = groups.iter().map(|g| g.key.clone()).collect();
        if !self.custom.is_empty() {
            all_keys.push("sys:non_steam".into());
        }
        let total_groups = all_keys.len();
        let mut group_act: Option<GroupMenuAct> = None;
        let mut drop_add: Option<(usize, AppId)> = None;

        if ctx.state.apps.is_none() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(11.0).color(t.accent));
                ui.label(w::text(t!("Loading owned games…"), 11.5, t.text_faint));
            });
        }

        let fs = style.font_size.unwrap_or(12.5);
        let row_h = row_height.clamp(20.0, 48.0);
        let ic_sz = icon_size.clamp(14.0, (row_h - 4.0).max(14.0));

        egui::ScrollArea::vertical().id_salt("lib-side-themed").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for group in groups {
                if group_headers {
                    let mut open = self.is_group_open(ui, &group.key, searching);
                    let (hrect, hresp) = ui.allocate_exact_size(vec2(ui.available_width() - 4.0, 24.0), Sense::click());
                    let hhover = ui.ctx().animate_bool_with_time(hresp.id, hresp.hovered(), 0.08);
                    let hresp = hresp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    let drop_hover = if let Some(col_idx) = group.collection {
                        let (dh, dropped) = self.check_drop_on_rect(ui, hrect);
                        if let Some(app_id) = dropped {
                            drop_add = Some((col_idx, app_id));
                        }
                        dh
                    } else {
                        false
                    };
                    if hresp.clicked() {
                        self.toggle_group(ui, &group.key, searching);
                        open = !open;
                    }
                    group_context_menu(&hresp, group.collection, total_groups, &mut group_act);
                    let bg = if drop_hover {
                        t.accent.gamma_multiply(0.25)
                    } else {
                        w::mix(t.raised.gamma_multiply(0.45), t.surface_hover.gamma_multiply(0.55), hhover)
                    };
                    ui.painter().rect_filled(hrect, CornerRadius::ZERO, bg);
                    if drop_hover {
                        ui.painter().rect_stroke(hrect, CornerRadius::ZERO, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                    }
                    let mark = if open { "−" } else { "+" };
                    let count_suffix = if show_counts { format!(" ({})", group.games.len()) } else { String::new() };
                    let count_galley = ui.painter().layout_no_wrap(count_suffix, w::font(11.5), t.text_faint);
                    let label_x = hrect.left() + 4.0;
                    let max_title_w = (hrect.right() - label_x - count_galley.size().x - 6.0).max(24.0);
                    let title_color = w::mix(t.text_dim, t.text, hhover);
                    let mut title_job = egui::text::LayoutJob::simple_singleline(format!("{mark} {}", group.label.to_uppercase()), w::font(11.5), title_color);
                    title_job.wrap = egui::text::TextWrapping::truncate_at_width(max_title_w);
                    let title_galley = ui.painter().layout_job(title_job);
                    let count_x = label_x + title_galley.size().x;
                    ui.painter().galley(pos2(label_x, hrect.center().y - title_galley.size().y * 0.5), title_galley, title_color);
                    ui.painter().galley(pos2(count_x, hrect.center().y - count_galley.size().y * 0.5), count_galley, t.text_faint);
                    if !open {
                        ui.add_space(2.0);
                        continue;
                    }
                }
                if group.games.is_empty() && group.collection.is_some() {
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        ui.label(w::text(t!("Empty collection"), 11.5, t.text_faint));
                    });
                    ui.add_space(6.0);
                    continue;
                }
                ui.push_id(&group.key, |ui| {
                    for e in group.games {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 4.0, row_h), Sense::click_and_drag());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let sel = self.selected == Some(e.app_id);
                        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.08);
                        if sel {
                            ui.painter().rect_filled(rect, CornerRadius::same(2), t.accent.gamma_multiply(0.22));
                        } else if hover > 0.0 {
                            ui.painter().rect_filled(rect, CornerRadius::same(2), t.surface_hover.gamma_multiply(hover * 0.75));
                        }
                        let icon = Rect::from_min_size(pos2(rect.left() + 18.0, rect.center().y - ic_sz * 0.5), vec2(ic_sz, ic_sz));
                        let url = e.owned.and_then(|o| o.icon.as_deref()).map(|h| art::icon(e.app_id, h));
                        let header = art::header(e.app_id);
                        let urls: Vec<&str> = url.as_deref().into_iter().chain([header.as_str()]).collect();
                        if !w::paint_cover_first(ui, icon, &urls, CornerRadius::same(2)) {
                            ui.painter().rect_filled(icon, CornerRadius::same(2), t.surface_hover);
                        }
                        let setup = setup_state(ctx, e.app_id, e.installed.is_some());
                        let busy = matches!(setup, Setup::Downloading(_) | Setup::Queued);
                        let color = if busy {
                            t.accent
                        } else if sel || e.installed.is_some() {
                            t.text
                        } else {
                            t.text_dim
                        };
                        let mut job = egui::text::LayoutJob::simple_singleline(e.name.to_string(), w::font(fs), color);
                        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - icon.right() - 11.0).max(40.0));
                        let galley = ui.painter().layout_job(job);
                        ui.painter().galley(pos2(icon.right() + 7.0, rect.center().y - galley.size().y * 0.5), galley, color);
                        if ctx.state.playing == Some(e.app_id) {
                            ui.painter().circle_filled(rect.right_center() - vec2(8.0, 0.0), 3.5, t.in_game);
                        }
                        if let Setup::Downloading(d) = &setup {
                            let frac = d.fraction();
                            let track = Rect::from_min_max(pos2(icon.right() + 7.0, rect.bottom() - 2.5), pos2(rect.right() - 6.0, rect.bottom() - 1.0));
                            ui.painter().rect_filled(track, CornerRadius::ZERO, t.raised);
                            let fill = Rect::from_min_size(track.min, vec2(track.width() * frac, track.height()));
                            ui.painter().rect_filled(fill, CornerRadius::ZERO, if d.paused { t.text_faint } else { t.accent });
                        }
                        let id = e.app_id;
                        self.game_menu(t, ctx, &resp, id, e.installed.is_some(), ctx.state.playing == Some(id));
                        if resp.drag_started_by(egui::PointerButton::Primary) {
                            self.dragging_game = Some((id, e.name.to_owned()));
                        }
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.selected = Some(id);
                            self.custom_selected = None;
                            self.collections_view = None;
                        }
                    }
                });
                ui.add_space(6.0);
            }
            if !self.custom.is_empty() {
                let key = "sys:non_steam";
                let mut open = true;
                if group_headers {
                    open = self.is_group_open(ui, key, searching);
                    let (hrect, hresp) = ui.allocate_exact_size(vec2(ui.available_width() - 4.0, 24.0), Sense::click());
                    let hhover = ui.ctx().animate_bool_with_time(hresp.id, hresp.hovered(), 0.08);
                    let hresp = hresp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if hresp.clicked() {
                        self.toggle_group(ui, key, searching);
                        open = !open;
                    }
                    group_context_menu(&hresp, None, total_groups, &mut group_act);
                    ui.painter().rect_filled(hrect, CornerRadius::ZERO, w::mix(t.raised.gamma_multiply(0.45), t.surface_hover.gamma_multiply(0.55), hhover));
                    let mark = if open { "−" } else { "+" };
                    let suffix = if show_counts { format!(" ({})", self.custom.len()) } else { String::new() };
                    let label = format!("{mark} {}{suffix}", t!("Non-Steam").to_uppercase());
                    ui.painter().text(pos2(hrect.left() + 4.0, hrect.center().y), Align2::LEFT_CENTER, label, w::font(11.5), w::mix(t.text_dim, t.text, hhover));
                }
                if open {
                    for (i, g) in self.custom.iter().enumerate() {
                        let sel = self.custom_selected == Some(i);
                        let r = ui.add(egui::Button::selectable(sel, w::text(format!("{}  {}", icons::CONTROLLER, g.name), fs, t.text)).min_size(vec2(ui.available_width() - 4.0, row_h)));
                        if r.clicked() {
                            self.custom_selected = Some(i);
                            self.selected = None;
                            self.collections_view = None;
                        }
                    }
                    ui.add_space(6.0);
                } else {
                    ui.add_space(2.0);
                }
            }
            if !self.hidden.is_empty() {
                w::check(ui, t, &mut self.show_hidden, w::text(t!("Show hidden games"), 12.0, t.text_faint));
            }
        });
        drop(items);
        if let Some((col_idx, app_id)) = drop_add {
            self.add_game_to_collection(col_idx, app_id, ctx);
        }
        self.apply_group_menu(ui, group_act, &all_keys, searching);
    }

    fn render_whats_new(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        ctx: &mut Ctx,
        title: Option<&str>,
        card_width: f32,
        card_height: f32,
        card_radius: u8,
        date_above: bool,
        max_items: usize,
        gap: f32,
        _style: &themes::NodeStyle,
    ) {
        if !self.whats_new_enabled() {
            return;
        }
        let account_recent = themed_recent_games(ctx);

        let mut posts: Vec<(AppId, usize, store::events::GameEvent)> = Vec::new();
        let mut loading = false;
        for (id, _, _) in account_recent.iter().take(10) {
            match self.events(*id) {
                Some(list) => posts.extend(list.iter().take(1).cloned().enumerate().map(|(i, e)| (*id, i, e))),
                None => loading = true,
            }
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        posts.retain(|p| now.saturating_sub(p.2.time) < 45 * 86_400);
        posts.sort_by_key(|p| std::cmp::Reverse(p.2.time));
        if posts.is_empty() && !loading {
            return;
        }
        let mut hide = false;
        let scroll_delta = title.map(|hdr| themed_shelf_header(ui, t, hdr, false, Some(&mut hide))).unwrap_or(0.0);
        if hide {
            self.set_whats_new_enabled(false);
            ctx.toast(t!("What's new hidden. You can turn it back on in Settings → Library."));
            ui.ctx().request_repaint();
            return;
        }
        ui.add_space(4.0);
        let radius = CornerRadius::same(card_radius);
        let top_off = if date_above { 18.0 } else { 0.0 };
        let limit = max_items.max(1);
        let actual_w = card_width.max(1.0);
        let actual_h = card_height.max(1.0);
        let card = vec2(actual_w, top_off + actual_h + 58.0);
        themed_shelf_scroll(ui, "whats-new-themed", scroll_delta).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                if posts.is_empty() {
                    for _ in 0..limit.min(5) {
                        let (r, _) = ui.allocate_exact_size(card, Sense::hover());
                        w::skeleton(ui, t, r, radius);
                    }
                }
                for (id, index, e) in posts.iter().take(limit) {
                    let (r, resp) = ui.allocate_exact_size(card, Sense::click());
                    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                    if date_above {
                        let age = now.saturating_sub(e.time) / 86_400;
                        let when = match age {
                            0 => t!("Today").to_string(),
                            1 => t!("Yesterday").to_string(),
                            2..=6 => t!("This week").to_string(),
                            _ => ui::date(e.time),
                        };
                        ui.painter().text(r.left_top(), Align2::LEFT_TOP, when, w::font(11.5), t.text_faint);
                    }
                    let img = Rect::from_min_size(r.min + vec2(0.0, top_off), vec2(actual_w, actual_h));
                    match e.capsule.as_deref().or(e.header.as_deref()) {
                        Some(u) => w::image_or_title(ui, t, img, u, &e.title, radius),
                        None => w::image_or_title(ui, t, img, &art::header(*id), &e.title, radius),
                    }
                    if hover > 0.0 {
                        ui.painter().rect_stroke(img, radius, Stroke::new(1.5, t.accent.gamma_multiply(hover)), egui::StrokeKind::Inside);
                    }
                    let text_rect = Rect::from_min_max(pos2(r.left(), img.bottom() + 6.0), r.max);
                    let mut job = egui::text::LayoutJob::simple(e.title.clone(), w::font(14.0), t.text_dim, actual_w);
                    job.wrap.max_rows = 2;
                    job.wrap.overflow_character = Some('…');
                    let galley = ui.painter().layout_job(job);
                    ui.painter().galley(text_rect.min, galley, t.text_dim);
                    let game = ctx.state.app_name(*id).unwrap_or_default().to_string();
                    let game_galley = ui.painter().layout_no_wrap(game, w::font(12.0), t.text_faint);
                    let badge = Rect::from_min_size(pos2(r.left(), r.bottom() - 20.0), vec2((game_galley.size().x + 28.0).min(actual_w), 20.0));
                    ui.painter().rect_filled(badge, CornerRadius::ZERO, t.raised);
                    let icon = Rect::from_min_size(badge.min + vec2(0.0, 1.0), vec2(18.0, 18.0));
                    let icon_url = ctx.state.apps.as_ref().and_then(|a| a.get(id)).and_then(|o| o.icon.as_deref()).map(|h| art::icon(*id, h));
                    let fallback = art::header(*id);
                    let urls: Vec<_> = icon_url.as_deref().into_iter().chain([fallback.as_str()]).collect();
                    w::paint_cover_first(ui, icon, &urls, CornerRadius::ZERO);
                    ui.painter_at(badge).galley(pos2(icon.right() + 5.0, badge.center().y - game_galley.size().y * 0.5), game_galley, t.text_faint);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.reader = Some(Reader { app: *id, index: *index });
                    }
                }
            });
        });
    }

    fn render_recent_games(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        ctx: &mut Ctx,
        title: Option<&str>,
        featured_first: bool,
        featured_width: f32,
        capsule_width: f32,
        capsule_height: f32,
        card_radius: u8,
        gap: f32,
        _style: &themes::NodeStyle,
    ) {
        let account_recent = themed_recent_games(ctx);
        if account_recent.is_empty() {
            return;
        }
        let scroll_delta = title.map(|hdr| themed_shelf_header(ui, t, hdr, true, None)).unwrap_or(0.0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let age = now.saturating_sub(account_recent[0].2) / 86_400;
        let when = match age {
            0 => t!("Today").to_string(),
            1 => t!("Yesterday").to_string(),
            2..=6 => t!("This week").to_string(),
            _ => ui::date(account_recent[0].2),
        };
        ui.label(w::text(when, 11.5, t.text_faint));
        ui.add_space(4.0);
        let avail = ui.available_width();
        let radius = CornerRadius::same(card_radius);
        let mut launch: Option<(AppId, PathBuf)> = None;
        let mut stop: Option<AppId> = None;
        let mut install: Option<AppId> = None;

        themed_shelf_scroll(ui, "recent-games-themed", scroll_delta).show(ui, |ui| {
          ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let skip = if featured_first {
                let (fid, fname, _) = &account_recent[0];
                let finstalled = ctx.local.games.iter().find(|g| g.app_id == *fid).cloned();
                let fplaying = ctx.state.playing == Some(*fid);
                let flaunching = !fplaying && ctx.state.launching == Some(*fid);
                let feat_w = featured_width.min(avail).max(1.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(feat_w, capsule_height), Sense::click());
                let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                let artwork = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 92.0));
                if !w::paint_cover_first(ui, artwork, &[&art::header(*fid), &art::hero(*fid)], radius) {
                    w::placeholder(ui, t, artwork, fname, radius);
                }
                if hover > 0.01 {
                    ui.painter().rect_stroke(rect, radius, Stroke::new(1.5, t.accent.gamma_multiply(hover)), egui::StrokeKind::Inside);
                }
                let bar_h = 92.0;
                let bar_rect = Rect::from_min_max(pos2(rect.left(), rect.bottom() - bar_h), rect.max);
                // Dimmed artwork beneath the cool translucent playtime pane.
                w::paint_cover_first(ui, bar_rect, &[&art::header(*fid)], radius);
                paint_featured_pane(ui, t, bar_rect, card_radius);

                let btn_size = (bar_rect.width() - 16.0).clamp(0.0, 62.0);
                let btn_rect = Rect::from_center_size(pos2(bar_rect.left() + 8.0 + btn_size * 0.5, bar_rect.center().y), vec2(btn_size, btn_size));
                let btn_resp = ui.interact(btn_rect, ui.scope_id().with(("feat-play", *fid)), Sense::click());
                let bhover = ui.ctx().animate_bool_with_time(btn_resp.id, btn_resp.hovered(), 0.08);
                let (btn_bg, btn_icon, btn_tip) = if fplaying {
                    (t.danger, icons::STOP, t!("Stop"))
                } else if flaunching {
                    (t.danger, icons::CLOCK, t!("Launching…"))
                } else if finstalled.is_some() {
                    (t.play, icons::PLAY, t!("Play"))
                } else {
                    (t.accent, icons::DOWNLOAD, t!("Install"))
                };
                ui.painter().rect_filled(btn_rect, CornerRadius::same(2), w::mix(btn_bg, Color32::WHITE, bhover * 0.14));
                if !fplaying && !flaunching && finstalled.is_some() {
                    // Crisp filled white play triangle matching Steam's Play button.
                    let c = btn_rect.center();
                    let tri = vec![
                        pos2(c.x - 7.0, c.y - 9.0),
                        pos2(c.x + 9.0, c.y),
                        pos2(c.x - 7.0, c.y + 9.0),
                    ];
                    ui.painter().add(egui::Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
                } else {
                    ui.painter().text(btn_rect.center(), Align2::CENTER_CENTER, btn_icon, w::font(20.0), Color32::WHITE);
                }
                if btn_resp.on_hover_text(btn_tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    if fplaying || flaunching {
                        stop = Some(*fid);
                    } else if let Some(g) = &finstalled {
                        launch = Some((*fid, g.install_dir.clone()));
                    } else {
                        install = Some(*fid);
                    }
                }

                let minutes = finstalled
                    .as_ref()
                    .and_then(|g| g.playtime_minutes)
                    .or_else(|| match ctx.state.player_games.get(&ctx.state.steam_id) {
                        Some(Ok(list)) => list.iter().find(|g| g.app_id == *fid).map(|g| g.playtime_minutes),
                        _ => None,
                    })
                    .unwrap_or(0);
                let played_str = if minutes > 0 { ui::playtime(minutes) } else { t!("Never").to_string() };
                let recent = ctx.state.player_games.get(&ctx.state.steam_id).and_then(|r| r.as_ref().ok())
                    .and_then(|games| games.iter().find(|g| g.app_id == *fid)).filter(|g| g.playtime_2weeks > 0)
                    .map(|g| format!("Last two weeks: {}", ui::playtime(g.playtime_2weeks)));
                paint_featured_playtime(ui, t, bar_rect, btn_rect, &played_str, recent.as_deref());

                self.game_menu(t, ctx, &resp, *fid, finstalled.is_some(), fplaying);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.selected = Some(*fid);
                }
                1
            } else {
                0
            };

            let fitted_size = vec2(capsule_width, capsule_height);
            for (id, name, _) in account_recent.iter().skip(skip) {
                let installed = ctx.local.games.iter().any(|g| g.app_id == *id);
                let r = themed_game_tile(ui, t, &art::capsule(*id), &art::header(*id), name, installed, fitted_size, card_radius);
                self.game_menu(t, ctx, &r, *id, installed, ctx.state.playing == Some(*id));
                if r.clicked() {
                    self.selected = Some(*id);
                }
            }
          });
        });

        if let Some((id, dir)) = launch {
            let args = client::LaunchArgs::user(self.games.get(&id).map(|s| s.args.clone()).unwrap_or_default());
            self.launch_or_notice(ctx, id, dir, args);
        } else if let Some(id) = stop {
            ctx.send(Command::StopGame { app_id: id });
        } else if let Some(id) = install {
            self.install_dialog = Some(InstallDialog::new(id, ctx));
        }
    }

    fn render_all_games(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        ctx: &mut Ctx,
        title: Option<&str>,
        show_count: bool,
        show_sort: bool,
        card_aspect: &str,
        card_width: f32,
        card_height: f32,
        card_radius: u8,
        gap: f32,
        _style: &themes::NodeStyle,
    ) {
        let items = self.filtered(ctx);
        if title.is_some() || show_sort {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if let Some(hdr) = title {
                    let heading = if show_count {
                        format!("{} ({})", i18n::tr_dyn(hdr), items.len())
                    } else {
                        i18n::tr_dyn(hdr)
                    };
                    ui.label(w::text(heading, 16.0, t.text));
                }
                if show_sort {
                    ui.add_space(4.0);
                    ui.label(w::text(t!("SORT BY"), 11.0, t.text_faint));
                    let current_label = match self.sort {
                        Sort::Name => t!("Alphabetical"),
                        Sort::Recent => t!("Last Played"),
                        Sort::Playtime => t!("Hours Played"),
                        Sort::Size => t!("Size on Disk"),
                    };
                    let btn = ui.add(
                        egui::Button::new(w::text(format!("{current_label}  {}", icons::CARET_DOWN), 12.0, t.text))
                            .fill(t.raised)
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(2))
                            .min_size(vec2(110.0, 24.0)),
                    );
                    egui::Popup::menu(&btn).show(|ui| {
                        ui.set_min_width(140.0);
                        for (s, lbl) in [
                            (Sort::Playtime, t!("Hours Played")),
                            (Sort::Recent, t!("Last Played")),
                            (Sort::Name, t!("Alphabetical")),
                            (Sort::Size, t!("Size on Disk")),
                        ] {
                            let mark = if self.sort == s { icons::CHECK } else { "" };
                            if w::menu_item(ui, t, mark, lbl).clicked() {
                                self.sort = s;
                                ui.close();
                            }
                        }
                    });
                }
            });
            ui.add_space(10.0);
        }
        if items.is_empty() {
            if ctx.state.apps.is_none() {
                w::loading(ui, t, t!("Loading your library…"));
            } else {
                w::empty(ui, t, icons::SEARCH, t!("No games match"), t!("Try a different search or filter."));
            }
            return;
        }
        let is_banner = card_aspect.eq_ignore_ascii_case("banner") || card_aspect == "16:9";
        let avail = ui.available_width();
        let cols = (((avail + gap) / (card_width + gap)).floor() as usize).max(1);
        let actual_w = card_width.min(avail).max(1.0);
        let actual_gap = if cols > 1 { ((avail - actual_w * cols as f32) / (cols - 1) as f32).max(gap) } else { gap };
        let ratio = if card_width > 0.0 { card_height / card_width } else if is_banner { 9.0 / 16.0 } else { 1.5 };
        let size = vec2(actual_w, (actual_w * ratio).round());
        for row in items.chunks(cols) {
            if w::skip_offscreen_row(ui, size.y) {
                ui.add_space(gap);
                continue;
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = actual_gap;
                for e in row {
                    let (primary, fallback) = if is_banner {
                        (art::header(e.app_id), art::capsule(e.app_id))
                    } else {
                        (art::capsule(e.app_id), art::header(e.app_id))
                    };
                    let r = themed_game_tile(ui, t, &primary, &fallback, e.name, e.installed.is_some(), size, card_radius);
                    self.game_menu(t, ctx, &r, e.app_id, e.installed.is_some(), ctx.state.playing == Some(e.app_id));
                    if r.clicked() {
                        self.selected = Some(e.app_id);
                    }
                }
            });
            ui.add_space(gap);
        }
    }
}

#[cfg(test)]
mod themed_library_tests {
    use super::*;

    #[test]
    fn steam_collections_button_opens_overview_and_drops_dragged_games() {
        use crate::theme_renderer::LibraryHost as _;
        let ctx = egui::Context::default();
        let mut library = Library::default();
        library.selected = Some(42);
        library.collections.push(Collection { name: "My collection".into(), games: BTreeSet::new(), steam_id: None });

        // Clicking the Collections button opens the Your Collections view and clears game selection.
        library.toggle_collections_overview();
        assert_eq!(library.collections_view, Some(CollectionsView::Overview));
        assert!(library.is_collections_open());
        assert!(library.is_detail_open());
        assert_eq!(library.selected, None);

        // Dragging a game over a drop zone and releasing drops that game.
        library.dragging_game = Some((620, "Portal 2".into()));
        let zone = Rect::from_min_size(pos2(20.0, 20.0), vec2(200.0, 200.0));
        let mut dropped = None;
        let mut out = ctx.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 400.0))),
            events: vec![
                egui::Event::PointerMoved(pos2(80.0, 80.0)),
                egui::Event::PointerButton { pos: pos2(80.0, 80.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
            ],
            ..Default::default()
        }, |ui| {
            let (hovered, id) = library.check_drop_on_rect(ui, zone);
            assert!(hovered);
            dropped = id;
        });
        out.textures_delta.clear();
        assert_eq!(dropped, Some(620));
        assert!(library.dragging_game.is_none());

        // Clicking Home closes the collections view.
        library.return_home();
        assert_eq!(library.collections_view, None);
        assert!(!library.is_collections_open());
    }

    #[test]
    fn sidebar_filters_and_collections_align_with_installed_and_star() {
        let ctx = egui::Context::default();
        ui::install_fonts(&ctx);
        let theme = Theme::default();
        let width = 256.0;
        let gap = 6.0;
        let mut top_rects = [Rect::NOTHING; 4];
        let mut filters_rect = Rect::NOTHING;
        let mut collections_rect = Rect::NOTHING;
        let mut out = ctx.run_ui(
            egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(300.0, 200.0))), ..Default::default() },
            |ui| {
                let top_labels = ["All", "Installed", icons::STAR, "Played"];
                let filters_label = format!("{}  Filters", icons::FUNNEL);
                let collections_label = format!("{}  Collections", icons::GRID);
                let top_nat = [
                    w::chip_width(ui, top_labels[0]),
                    w::chip_width(ui, top_labels[1]),
                    w::chip_width(ui, top_labels[2]),
                    w::chip_width(ui, top_labels[3]),
                ];
                let bottom_nat = [w::chip_width(ui, &filters_label), w::chip_width(ui, &collections_label)];
                let (tw, bw) = sidebar_filter_widths(top_nat, Some(bottom_nat), width, gap);
                let tw = tw.expect("top row fits");
                let [fw, cw] = bw.expect("bottom row aligns");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(gap, gap);
                    for (i, label) in top_labels.into_iter().enumerate() {
                        top_rects[i] = w::wide_chip(ui, &theme, label, false, tw[i]).rect;
                    }
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(gap, gap);
                    filters_rect = w::wide_chip(ui, &theme, &filters_label, false, fw).rect;
                    collections_rect = w::wide_chip(ui, &theme, &collections_label, false, cw).rect;
                });
            },
        );
        out.textures_delta.clear();
        assert!((filters_rect.left() - top_rects[0].left()).abs() < 0.5);
        assert!((filters_rect.right() - top_rects[1].right()).abs() < 0.5);
        assert!((collections_rect.left() - top_rects[2].left()).abs() < 0.5);
        assert!((collections_rect.right() - top_rects[3].right()).abs() < 0.5);
    }

    #[test]
    fn steam_featured_pane_and_text_follow_theme_roles_and_stay_inside_card() {
        let ctx = egui::Context::default();
        let mut theme = Theme::default();
        theme.raised = Color32::from_rgb(50, 54, 48);
        theme.text = Color32::LIGHT_GREEN;
        theme.text_dim = Color32::YELLOW;
        let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(150.0, 92.0));
        let button = Rect::from_min_size(pos2(8.0, 15.0), vec2(62.0, 62.0));
        let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0))), ..Default::default() }, |ui| {
            paint_featured_pane(ui, &theme, bar, 2);
            paint_featured_playtime(ui, &theme, bar, button, "123456 hours", Some("Last two weeks: 123 hours"));
        });
        output.textures_delta.clear();
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == theme.raised.gamma_multiply(225.0 / 255.0))), "pane ignores theme surface");
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape {
                assert!(shape.clip_rect.right() <= bar.right() && shape.clip_rect.bottom() <= bar.bottom(), "playtime text escapes card: {:?}", shape.clip_rect);
                if text.galley.job.text.starts_with("Total:") {
                    assert!(text.galley.job.sections.iter().all(|section| section.format.color == theme.text), "total ignores theme text");
                }
            }
        }
    }

    #[test]
    fn sidebar_date_groups_cover_recent_months_years_and_missing_history() {
        // 20 November 2026, UTC.
        let now = 1_795_132_800;
        assert_eq!(played_group(Some(now - 1), now), PlayedGroup::Recent);
        assert_eq!(played_group(Some(now - 14 * 86_400), now), PlayedGroup::Month(std::cmp::Reverse((2026, 11))));
        assert_eq!(played_group(Some(1_791_504_000), now), PlayedGroup::Month(std::cmp::Reverse((2026, 10))));
        assert_eq!(played_group(Some(1_767_139_200), now), PlayedGroup::Month(std::cmp::Reverse((2025, 12))));
        assert_eq!(played_group(None, now), PlayedGroup::Never);
        assert_eq!(played_group(Some(0), now), PlayedGroup::Never);
        let mut groups = vec![PlayedGroup::Never, played_group(Some(1_767_139_200), now), PlayedGroup::Recent, played_group(Some(1_791_504_000), now)];
        groups.sort();
        assert_eq!(groups, vec![PlayedGroup::Recent, PlayedGroup::Month(std::cmp::Reverse((2026, 10))), PlayedGroup::Month(std::cmp::Reverse((2025, 12))), PlayedGroup::Never]);
    }

    #[test]
    fn recent_shelf_merges_partial_account_history_with_local_games() {
        let merged = merge_recent_games(
            [(1, "Account name".into(), 100), (2, "Account only".into(), 300)],
            [(1, "Local name".into(), 200), (3, "Local only".into(), 250)],
        );
        assert_eq!(merged, vec![
            (2, "Account only".into(), 300),
            (3, "Local only".into(), 250),
            (1, "Account name".into(), 200),
        ]);
    }

    #[test]
    fn launch_args_for_carries_controller_override() {
        let mut lib = Library::default();
        lib.games.insert(
            620,
            GameSettings {
                args: "-novid".into(),
                controller: 2,
                ..Default::default()
            },
        );
        let args = lib.launch_args_for(620);
        assert_eq!(args.user, "-novid");
        assert_eq!(args.controller, 2);
        assert_eq!(lib.launch_args_for(440).controller, 0);
    }

    #[test]
    fn whats_new_enabled_respects_shared_flags() {
        let mut lib = Library::default();
        assert!(lib.whats_new_enabled());

        let flags = std::sync::Arc::new(std::sync::Mutex::new(super::super::settings::AppFlags {
            show_whats_new: true,
            ..Default::default()
        }));
        lib.flags = Some(flags.clone());
        assert!(lib.whats_new_enabled());

        lib.set_whats_new_enabled(false);
        assert!(!lib.whats_new_enabled());
        assert!(!flags.lock().unwrap().show_whats_new);
    }

    #[test]
    fn sidebar_groups_sort_by_collections_with_uncategorized_and_favorites() {
        let mut lib = Library::default();
        lib.favorites.insert(10);
        lib.collections = vec![
            Collection { name: "Valve".into(), games: [10, 20].into_iter().collect(), steam_id: Some("uc-valve".into()) },
            Collection { name: "Capcom".into(), games: [30].into_iter().collect(), steam_id: Some("uc-capcom".into()) },
            Collection { name: "Empty".into(), games: BTreeSet::new(), steam_id: Some("uc-empty".into()) },
        ];
        let items = vec![
            Entry { app_id: 10, name: "Portal 2", installed: None, owned: None },
            Entry { app_id: 20, name: "Half-Life 2", installed: None, owned: None },
            Entry { app_id: 30, name: "Resident Evil", installed: None, owned: None },
            Entry { app_id: 40, name: "Stray", installed: None, owned: None },
        ];
        let groups = lib.sidebar_groups_with_history(&items, None, themes::LibraryGrouping::Installed, "Not installed", true);
        let summary: Vec<(&str, Vec<AppId>)> = groups.iter().map(|g| (g.label.as_str(), g.games.iter().map(|e| e.app_id).collect())).collect();
        assert_eq!(summary, vec![
            ("Favorites", vec![10]),
            ("Capcom", vec![30]),
            ("Empty", vec![]),
            ("Valve", vec![10, 20]),
            ("Uncategorized", vec![40]),
        ]);
        // Search/filtering hides empty collections.
        lib.search = "portal".into();
        let filtered_items = vec![Entry { app_id: 10, name: "Portal 2", installed: None, owned: None }];
        let filtered_groups = lib.sidebar_groups_with_history(&filtered_items, None, themes::LibraryGrouping::Installed, "Not installed", true);
        let labels: Vec<&str> = filtered_groups.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, vec!["Favorites", "Valve"]);

        // Hiding collections falls back to Installed / Not installed.
        lib.search.clear();
        let fallback = lib.sidebar_groups_with_history(&items, None, themes::LibraryGrouping::Installed, "Not installed", false);
        let fallback_labels: Vec<&str> = fallback.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(fallback_labels, vec!["Favorites", "Not installed"]);
    }

    #[test]
    fn sidebar_collection_dropdown_toggles_persists_and_cleans_up_on_delete() {
        let ctx = egui::Context::default();
        let theme = Theme::default();
        let mut lib = Library::default();
        lib.collections = vec![
            Collection { name: "Capcom".into(), games: [1].into_iter().collect(), steam_id: Some("uc-1".into()) },
            Collection { name: "Valve".into(), games: [2].into_iter().collect(), steam_id: Some("uc-2".into()) },
        ];
        let key = collection_key(&lib.collections[0]);
        let header_center = std::cell::Cell::new(egui::Pos2::ZERO);
        let render = |events: Vec<egui::Event>, lib: &mut Library| {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(300.0, 200.0))), events, ..Default::default() }, |ui| {
                let open = lib.is_group_open(ui, &key, false);
                let resp = sidebar_group_header(ui, &theme, "Capcom", 1, open, 240.0);
                header_center.set(resp.rect.center());
                if resp.clicked() {
                    lib.toggle_group(ui, &key, false);
                }
            });
            out.textures_delta.clear();
        };
        let click = |pos| vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
        ];
        render(vec![], &mut lib);
        assert!(!lib.collapsed_groups.contains(&key));
        render(click(header_center.get()), &mut lib);
        assert!(lib.collapsed_groups.contains(&key));

        // Collapse state survives save/load.
        let saved = lib.save().expect("save");
        let mut restored = Library::default();
        restored.load(&saved);
        assert!(restored.collapsed_groups.contains(&key));

        // Deleting a collection removes its collapse entry and keeps Filter::Collection aligned.
        restored.filter = Filter::Collection(1);
        restored.delete_collection(0);
        assert!(!restored.collapsed_groups.contains(&key));
        assert!(restored.filter == Filter::Collection(0));
    }
}

/// What installing a game needs (for Big Picture's install screen).
#[derive(Clone)]
pub(crate) struct InstallPlan {
    /// Library folders with their free space.
    pub libraries: Vec<(std::path::PathBuf, u64)>,
    /// The one with the most free space.
    pub default: std::path::PathBuf,
    /// Size on disk.
    pub size: u64,
    pub choose_platform: bool,
}

impl Library {
    pub(crate) fn install_plan(&self, ctx: &Ctx, id: AppId, platform: &str) -> InstallPlan {
        let mut d = InstallDialog::new(id, ctx);
        d.platform = platform.into();
        let libraries = d.libraries.iter().map(|l| (l.clone(), fs4_free(l).unwrap_or(0))).collect();
        let size = d.size(ctx);
        InstallPlan { libraries, default: d.library, size, choose_platform: d.choose_platform }
    }

    /// Install to a chosen folder, with a desktop shortcut afterwards if asked.
    pub(crate) fn install_to(&mut self, ctx: &mut Ctx, id: AppId, library: std::path::PathBuf, shortcut: bool, platform: &str) {
        if shortcut {
            self.shortcut_after_install.insert(id, library.clone());
        }
        self.queue_install_as(ctx, id, Some(library), platform.into());
    }

    /// Per-game settings & state snapshot for Big Picture's Manage page.
    pub(crate) fn game_manage_snapshot(&self, id: AppId) -> GameManageSnapshot {
        let s = self.games.get(&id);
        let verify_report = self.reports.get(&id).map(|r| (r.ok, r.files, r.healthy()));
        GameManageSnapshot {
            favorite: self.favorites.contains(&id),
            hidden: self.hidden.contains(&id),
            cloud_off: s.is_some_and(|g| g.cloud_off),
            overlay_on: s.map_or(true, |g| g.overlay_on),
            auto_update: s.map_or(0, |g| g.auto_update),
            background: s.map_or(0, |g| g.background),
            branch: s.map(|g| g.branch.clone()).unwrap_or_default(),
            disabled_dlc: s.map(|g| g.disabled_dlc.clone()).unwrap_or_default(),
            enabled_dlc: s.map(|g| g.enabled_dlc.clone()).unwrap_or_default(),
            uninstalling: self.uninstalling.contains_key(&id),
            verify_report,
        }
    }

    pub(crate) fn is_favorite(&self, id: AppId) -> bool {
        self.favorites.contains(&id)
    }

    pub(crate) fn toggle_favorite(&mut self, ctx: &mut Ctx, id: AppId) -> bool {
        let added = if self.favorites.remove(&id) {
            false
        } else {
            self.favorites.insert(id);
            true
        };
        self.sync_collections(ctx);
        added
    }

    pub(crate) fn toggle_hidden(&mut self, ctx: &mut Ctx, id: AppId) -> bool {
        let added = if self.hidden.remove(&id) {
            false
        } else {
            self.hidden.insert(id);
            true
        };
        self.sync_collections(ctx);
        added
    }

    pub(crate) fn favorites_set(&self) -> BTreeSet<AppId> {
        self.favorites.clone()
    }

    pub(crate) fn hidden_set(&self) -> BTreeSet<AppId> {
        self.hidden.clone()
    }

    pub(crate) fn collections_summary(&self) -> Vec<(String, BTreeSet<AppId>)> {
        if self.hide_collections {
            return Vec::new();
        }
        self.collections
            .iter()
            .filter(|c| !c.games.is_empty())
            .map(|c| (c.name.clone(), c.games.clone()))
            .collect()
    }

    pub(crate) fn set_game_cloud(&mut self, ctx: &Ctx, id: AppId, enabled: bool) {
        self.games.entry(id).or_default().cloud_off = !enabled;
        self.send_cloud_settings(ctx);
    }

    fn sync_overlay_overrides(&self) {
        let disabled: BTreeSet<AppId> = self
            .games
            .iter()
            .filter(|(_, s)| !s.overlay_on)
            .map(|(id, _)| *id)
            .collect();
        client::set_overlay_disabled_apps(disabled);
    }

    pub(crate) fn set_game_overlay(&mut self, id: AppId, enabled: bool) {
        self.games.entry(id).or_default().overlay_on = enabled;
        client::set_app_overlay(id, enabled);
    }

    pub(crate) fn set_game_auto_update(&mut self, id: AppId, mode: u8) {
        self.games.entry(id).or_default().auto_update = mode.min(2);
    }

    pub(crate) fn set_game_background_downloads(&mut self, id: AppId, mode: u8) {
        self.games.entry(id).or_default().background = mode.min(2);
    }

    pub(crate) fn set_game_branch(&mut self, id: AppId, branch: String) {
        self.games.entry(id).or_default().branch = branch;
    }

    pub(crate) fn toggle_game_dlc(&mut self, id: AppId, dlc_id: AppId, enable: bool) {
        let s = self.games.entry(id).or_default();
        if enable {
            s.disabled_dlc.remove(&dlc_id);
            s.enabled_dlc.insert(dlc_id);
        } else {
            s.disabled_dlc.insert(dlc_id);
            s.enabled_dlc.remove(&dlc_id);
        }
    }

    pub(crate) fn apply_game_changes(&mut self, ctx: &mut Ctx, id: AppId, library: std::path::PathBuf) {
        let running = ctx.state.downloads.get(&id).is_some_and(|d| {
            !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed) && !d.verifying()
        });
        if running {
            ctx.send(Command::CancelDownload { app_id: id });
        }
        self.queue_install(ctx, id, Some(library));
    }

    pub(crate) fn verify_game_quiet(&mut self, ctx: &mut Ctx, g: &InstalledGame) {
        ctx.send(Command::VerifyGame {
            game: g.clone(),
            steam_root: ctx.local.steam_root.clone(),
        });
        ctx.toast(t!("Verifying files…"));
    }

    pub(crate) fn is_app_busy(&self, ctx: &Ctx, id: AppId) -> bool {
        ctx.state.playing == Some(id)
            || self.uninstalling.contains_key(&id)
            || ctx.state.moving.contains_key(&id)
            || ctx.state.backing_up.contains_key(&id)
            || ctx.state.download_queue.contains(&id)
            || ctx.state.cloud_syncing.contains(&id)
            || ctx.state.downloads.get(&id).is_some_and(|d| {
                !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)
            })
    }

    pub(crate) fn start_uninstall_bg(&mut self, ctx: &mut Ctx, egui_ctx: egui::Context, g: InstalledGame, name: String) -> bool {
        let id = g.app_id;
        if self.is_app_busy(ctx, id) {
            return false;
        }
        let job = Uninstalling {
            name: name.clone(),
            progress: Arc::new(Progress::default()),
            result: Arc::new(Mutex::new(None)),
        };
        let (progress, result) = (job.progress.clone(), job.result.clone());
        std::thread::spawn(move || {
            let r = local::manage::uninstall_with(&g, &progress);
            *result.lock().unwrap() = Some(r);
            egui_ctx.request_repaint_of(egui::ViewportId::ROOT);
        });
        self.uninstalling.insert(id, job);
        ctx.toast(t!("Uninstalling…"));
        true
    }

    pub(crate) fn queue_update_game(&mut self, ctx: &mut Ctx, id: AppId, library: std::path::PathBuf) {
        self.queue_install(ctx, id, Some(library));
    }

    pub(crate) fn games_needing_update(&mut self, ctx: &Ctx) -> Vec<(AppId, String, u64, std::path::PathBuf)> {
        let busy = |id: AppId| {
            ctx.state
                .downloads
                .get(&id)
                .is_some_and(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed))
                || ctx.state.download_queue.contains(&id)
        };
        let games: Vec<InstalledGame> = ctx.local.games.clone();
        let mut out = Vec::new();
        for g in games {
            if !busy(g.app_id) && self.needs_update(ctx, &g) {
                let name = ctx.state.app_name(g.app_id).map(String::from).unwrap_or(g.name);
                out.push((g.app_id, name, g.size_on_disk, g.library));
            }
        }
        out.sort_by(|a, b| model::cmp_ignore_case(&a.1, &b.1));
        out
    }
}

#[derive(Clone, Default)]
pub(crate) struct GameManageSnapshot {
    pub favorite: bool,
    pub hidden: bool,
    pub cloud_off: bool,
    pub overlay_on: bool,
    pub auto_update: u8,
    pub background: u8,
    pub branch: String,
    pub disabled_dlc: BTreeSet<AppId>,
    pub enabled_dlc: BTreeSet<AppId>,
    pub uninstalling: bool,
    pub verify_report: Option<(usize, usize, bool)>,
}
