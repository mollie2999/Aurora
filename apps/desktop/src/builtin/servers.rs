//! Game Servers Browser (`View -> Game Servers`, `steam://connect/...`).
//!
//! Supports Internet master-server browsing, Steam-compatible Favorites and
//! History persistence (`serverbrowser_hist.vdf`), LAN A2S discovery, Friends'
//! active servers, and live A2S Info / Players / Rules inspection.

use client::{Command, Event, a2s};
use egui::{Align, CornerRadius, Layout, Margin, Sense, Stroke, Ui, vec2};
use extensions::{Action, Ctx, Extension, Manifest, PageSpec};
use i18n::t;
use local::servers::{SavedServer, ServerBrowserLists};
use model::{AppId, GameServerEntry};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use ui::{icons, widgets as w};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ServerTab {
    #[default]
    Internet,
    Favorites,
    History,
    Lan,
    Friends,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SortCol {
    Name,
    Game,
    Map,
    #[default]
    Players,
    Ping,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum DetailTab {
    #[default]
    Players,
    Rules,
}

pub struct Servers {
    tab: ServerTab,
    request_seq: u64,
    internet_req: Option<u64>,
    lan_req: Option<u64>,
    internet: Vec<GameServerEntry>,
    lan: Vec<GameServerEntry>,
    internet_loading: bool,
    internet_err: Option<String>,
    lan_loading: bool,
    filter_app: Option<AppId>,
    search: String,
    map_filter: String,
    has_players: bool,
    not_full: bool,
    no_password: bool,
    vac_secure: bool,
    sort: SortCol,
    sort_asc: bool,
    selected: Option<String>,
    detail_tab: DetailTab,
    connect_banner: Option<(String, Option<String>)>,
    pending_connect_query: Option<String>,
    add_dialog: Option<(String, String)>,
    add_error: Option<String>,
    local_lists: Option<(PathBuf, u32, ServerBrowserLists)>,
    live_info: HashMap<String, a2s::ServerInfo>,
    queried_saved: HashSet<String>,
    initialized: bool,
}

impl Default for Servers {
    fn default() -> Self {
        Self {
            tab: ServerTab::Internet,
            request_seq: 0,
            internet_req: None,
            lan_req: None,
            internet: Vec::new(),
            lan: Vec::new(),
            internet_loading: false,
            internet_err: None,
            lan_loading: false,
            filter_app: None,
            search: String::new(),
            map_filter: String::new(),
            has_players: false,
            not_full: false,
            no_password: false,
            vac_secure: false,
            sort: SortCol::Players,
            sort_asc: false,
            selected: None,
            detail_tab: DetailTab::Players,
            connect_banner: None,
            pending_connect_query: None,
            add_dialog: None,
            add_error: None,
            local_lists: None,
            live_info: HashMap::new(),
            queried_saved: HashSet::new(),
            initialized: false,
        }
    }
}

impl Servers {
    fn steam_root(ctx: &Ctx) -> PathBuf {
        if !ctx.local.steam_root.as_os_str().is_empty() {
            ctx.local.steam_root.clone()
        } else {
            client::store::config_dir().join("steam")
        }
    }

    fn account_id(ctx: &Ctx) -> u32 {
        if ctx.state.steam_id != 0 {
            model::account_id(ctx.state.steam_id)
        } else {
            ctx.local
                .accounts
                .iter()
                .find(|a| a.most_recent)
                .or(ctx.local.accounts.first())
                .map(|a| model::account_id(a.steam_id))
                .unwrap_or(0)
        }
    }

    fn ensure_lists(&mut self, ctx: &Ctx) -> &ServerBrowserLists {
        let root = Self::steam_root(ctx);
        let acct = Self::account_id(ctx);
        let needs_reload = self
            .local_lists
            .as_ref()
            .is_none_or(|(r, a, _)| *r != root || *a != acct);
        if needs_reload {
            let lists = local::servers::load_lists(&root, acct);
            self.local_lists = Some((root, acct, lists));
        }
        &self.local_lists.as_ref().unwrap().2
    }

    fn reload_lists(&mut self, ctx: &Ctx) {
        let root = Self::steam_root(ctx);
        let acct = Self::account_id(ctx);
        let lists = local::servers::load_lists(&root, acct);
        self.local_lists = Some((root, acct, lists));
    }

    fn build_master_filter(&self) -> String {
        let mut f = String::new();
        if let Some(app_id) = self.filter_app {
            f.push_str(&format!("\\appid\\{app_id}"));
        }
        let map = self.map_filter.trim();
        if !map.is_empty() && !map.contains('\\') {
            f.push_str(&format!("\\map\\{map}"));
        }
        if self.has_players {
            f.push_str("\\empty\\1");
        }
        if self.not_full {
            f.push_str("\\full\\1");
        }
        if self.vac_secure {
            f.push_str("\\secure\\1");
        }
        f
    }

    fn refresh_tab(&mut self, ctx: &Ctx) {
        self.initialized = true;
        match self.tab {
            ServerTab::Internet => {
                self.request_seq += 1;
                let req = self.request_seq;
                self.internet_req = Some(req);
                self.internet_loading = true;
                self.internet_err = None;
                let filter = self.build_master_filter();
                ctx.send(Command::BrowseGameServers {
                    request_id: req,
                    filter,
                    max: 500,
                });
            }
            ServerTab::Lan => {
                self.request_seq += 1;
                let req = self.request_seq;
                self.lan_req = Some(req);
                self.lan_loading = true;
                ctx.send(Command::DiscoverLanServers { request_id: req });
            }
            ServerTab::Favorites | ServerTab::History | ServerTab::Friends => {
                self.queried_saved.clear();
                self.reload_lists(ctx);
            }
        }
    }

    fn merge_saved(&self, s: &SavedServer) -> GameServerEntry {
        let (ip, port) = local::servers::parse_ipv4_port(&s.addr).unwrap_or((0, 0));
        let mut entry = GameServerEntry {
            addr: s.addr.clone(),
            ip,
            game_port: port,
            query_port: port,
            steam_id: 0,
            app_id: s.app_id,
            name: if s.name.is_empty() { s.addr.clone() } else { s.name.clone() },
            map: String::new(),
            game_dir: s.game_dir.clone(),
            description: String::new(),
            players: 0,
            max_players: 0,
            bots: 0,
            password: false,
            secure: false,
            ping_ms: 0,
            tags: String::new(),
        };
        if let Some(live) = self.live_info.get(&s.addr) {
            if !live.name.is_empty() {
                entry.name = live.name.clone();
            }
            entry.map = live.map.clone();
            entry.description = live.game.clone();
            entry.players = u32::from(live.players);
            entry.max_players = u32::from(live.max_players);
            entry.bots = u32::from(live.bots);
            entry.password = live.password;
            entry.secure = live.vac;
            entry.ping_ms = live.ping_ms;
        }
        entry
    }

    fn current_rows(&mut self, ctx: &Ctx) -> Vec<GameServerEntry> {
        self.ensure_lists(ctx);
        let mut rows: Vec<GameServerEntry> = match self.tab {
            ServerTab::Internet => self.internet.clone(),
            ServerTab::Lan => self.lan.clone(),
            ServerTab::Favorites => self
                .local_lists
                .as_ref()
                .map(|(_, _, l)| l.favorites.iter().map(|s| self.merge_saved(s)).collect())
                .unwrap_or_default(),
            ServerTab::History => self
                .local_lists
                .as_ref()
                .map(|(_, _, l)| l.history.iter().map(|s| self.merge_saved(s)).collect())
                .unwrap_or_default(),
            ServerTab::Friends => {
                let mut list = Vec::new();
                for p in ctx.state.friend_list() {
                    let Some(addr) = p.server.as_deref() else { continue };
                    let app_id = p.game_app_id.unwrap_or(0);
                    let mut entry = self.merge_saved(&SavedServer {
                        addr: addr.to_string(),
                        name: p.game_name.clone().unwrap_or_else(|| p.name.clone()),
                        app_id,
                        game_dir: String::new(),
                        last_played: 0,
                    });
                    if entry.description.is_empty() {
                        entry.description = p.name.clone();
                    }
                    list.push(entry);
                }
                list
            }
        };

        if !matches!(self.tab, ServerTab::Internet | ServerTab::Lan) {
            for r in &rows {
                if self.queried_saved.len() < 48 && self.queried_saved.insert(r.addr.clone()) {
                    ctx.send(Command::QueryServer { addr: r.addr.clone() });
                }
            }
        }

        let q = self.search.trim().to_ascii_lowercase();
        let m_q = self.map_filter.trim().to_ascii_lowercase();
        rows.retain(|s| {
            if let Some(app) = self.filter_app {
                if s.app_id != 0 && s.app_id != app {
                    return false;
                }
            }
            if !q.is_empty()
                && !s.name.to_ascii_lowercase().contains(&q)
                && !s.addr.to_ascii_lowercase().contains(&q)
                && !s.description.to_ascii_lowercase().contains(&q)
                && !s.map.to_ascii_lowercase().contains(&q)
            {
                return false;
            }
            if !m_q.is_empty() && !s.map.to_ascii_lowercase().contains(&m_q) {
                return false;
            }
            if self.has_players && s.players == 0 {
                return false;
            }
            if self.not_full && s.max_players > 0 && s.players >= s.max_players {
                return false;
            }
            if self.no_password && s.password {
                return false;
            }
            if self.vac_secure && !s.secure {
                return false;
            }
            true
        });

        rows.sort_by(|a, b| {
            let ord = match self.sort {
                SortCol::Name => a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()),
                SortCol::Game => a.description.to_ascii_lowercase().cmp(&b.description.to_ascii_lowercase()),
                SortCol::Map => a.map.to_ascii_lowercase().cmp(&b.map.to_ascii_lowercase()),
                SortCol::Players => a.players.cmp(&b.players),
                SortCol::Ping => a.ping_ms.cmp(&b.ping_ms),
            };
            if self.sort_asc { ord } else { ord.reverse() }
        });
        rows
    }

    fn is_favorite(&self, addr: &str) -> bool {
        self.local_lists
            .as_ref()
            .is_some_and(|(_, _, l)| l.favorites.iter().any(|s| s.addr == addr))
    }

    fn toggle_favorite(&mut self, ctx: &Ctx, entry: &GameServerEntry) {
        let root = Self::steam_root(ctx);
        let acct = Self::account_id(ctx);
        if self.is_favorite(&entry.addr) {
            if let Ok(lists) = local::servers::remove_favorite(&root, acct, &entry.addr) {
                self.local_lists = Some((root, acct, lists));
            }
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let saved = SavedServer {
                addr: entry.addr.clone(),
                name: entry.name.clone(),
                app_id: entry.app_id,
                game_dir: entry.game_dir.clone(),
                last_played: now,
            };
            if let Ok(lists) = local::servers::add_favorite(&root, acct, saved) {
                self.local_lists = Some((root, acct, lists));
            }
        }
    }

    fn resolve_app_id(&self, ctx: &Ctx, entry: &GameServerEntry) -> Option<AppId> {
        if entry.app_id != 0 {
            return Some(entry.app_id);
        }
        if let Some(app) = self.filter_app {
            return Some(app);
        }
        if !entry.game_dir.is_empty() {
            if let Some(g) = ctx.local.games.iter().find(|g| g.install_dir.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(&entry.game_dir))) {
                return Some(g.app_id);
            }
        }
        None
    }

    fn connect_server(&mut self, ctx: &mut Ctx, entry: &GameServerEntry, password: Option<&str>) {
        let Some(app_id) = self.resolve_app_id(ctx, entry) else {
            ctx.toast(t!("Select the game in the filter above to launch and connect."));
            return;
        };
        let connect = match password.map(str::trim).filter(|p| !p.is_empty()) {
            Some(pw) => format!("+connect {} +password {pw}", entry.addr),
            None => format!("+connect {}", entry.addr),
        };
        let root = Self::steam_root(ctx);
        let acct = Self::account_id(ctx);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(lists) = local::servers::record_history(
            &root,
            acct,
            SavedServer {
                addr: entry.addr.clone(),
                name: entry.name.clone(),
                app_id,
                game_dir: entry.game_dir.clone(),
                last_played: now,
            },
        ) {
            self.local_lists = Some((root, acct, lists));
        }
        ctx.act(Action::JoinGame { app_id, connect });
    }
}

impl Extension for Servers {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "servers",
            name: "Game Servers",
            author: "Aurora",
            description: "Browse Internet, Favorites, History, LAN, and Friends servers",
            core: true,
            ..Manifest::DEFAULT
        }
    }

    fn pages(&self) -> Vec<PageSpec> {
        vec![PageSpec::new("servers", "Game Servers", icons::GLOBE).hidden().order(65)]
    }

    fn on_navigate(&mut self, _page: &str, arg: Option<&str>) {
        if let Some(arg) = arg {
            if let Some(target) = arg.strip_prefix("connect:") {
                let (addr, password) = match target.split_once('|') {
                    Some((a, p)) => (a.to_string(), Some(p.to_string())),
                    None => (target.to_string(), None),
                };
                self.selected = Some(addr.clone());
                self.connect_banner = Some((addr.clone(), password));
                self.pending_connect_query = Some(addr);
                self.tab = ServerTab::Favorites;
                return;
            }
            if let Ok(id) = arg.parse::<AppId>() {
                self.filter_app = Some(id);
                self.initialized = false;
            }
        }
    }

    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) {
        match event {
            Event::GameServerList { request_id, result } if Some(*request_id) == self.internet_req => {
                self.internet_loading = false;
                match result {
                    Ok(list) => {
                        self.internet = list.clone();
                        self.internet_err = None;
                    }
                    Err(e) => {
                        self.internet_err = Some(e.clone());
                    }
                }
            }
            Event::LanServerList { request_id, servers } if Some(*request_id) == self.lan_req => {
                self.lan_loading = false;
                self.lan = servers.clone();
            }
            Event::ServerInfo { addr, result: Ok(info) } => {
                self.live_info.insert(addr.clone(), info.clone());
            }
            _ => {}
        }
    }

    fn page(&mut self, _page: &str, ui: &mut Ui, ctx: &mut Ctx) {
        let t = ctx.theme.clone();
        if let Some(addr) = self.pending_connect_query.take() {
            ctx.send(Command::QueryServerDetails { addr });
        }
        if !self.initialized {
            self.refresh_tab(ctx);
        }

        let mut connect_to_entry: Option<(GameServerEntry, Option<String>)> = None;
        let mut fav_toggle_entry: Option<GameServerEntry> = None;
        let mut query_details_addr: Option<String> = None;
        let mut refresh_now = false;

        egui::Frame::new()
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(w::text_bold(format!("{}  {}", icons::GLOBE, t!("Game Servers")), 20.0, t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &t, Some(icons::PLUS), t!("Add Server…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                            let default_app = self.filter_app.map(|a| a.to_string()).unwrap_or_default();
                            self.add_dialog = Some((String::new(), default_app));
                            self.add_error = None;
                        }
                        if w::button(ui, &t, Some(icons::REFRESH), t!("Refresh"), w::Kind::Primary, w::Size::Sm).clicked() {
                            refresh_now = true;
                        }
                    });
                });
                ui.add_space(10.0);

                if let Some((addr, pw)) = self.connect_banner.clone() {
                    let banner_entry = self.merge_saved(&SavedServer {
                        addr: addr.clone(),
                        name: addr.clone(),
                        app_id: self.filter_app.unwrap_or(0),
                        game_dir: String::new(),
                        last_played: 0,
                    });
                    egui::Frame::new()
                        .fill(t.accent.gamma_multiply(0.14))
                        .stroke(Stroke::new(1.0, t.accent))
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(w::text_semibold(format!("steam://connect/{addr}"), 13.5, t.text));
                                if !banner_entry.map.is_empty() {
                                    ui.label(w::text(format!("{} · {}/{}", banner_entry.name, banner_entry.players, banner_entry.max_players), 13.0, t.text_dim));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if w::button(ui, &t, Some(icons::X), t!("Dismiss"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                        self.connect_banner = None;
                                    }
                                    if w::button(ui, &t, Some(icons::PLAY), t!("Connect"), w::Kind::Primary, w::Size::Sm).clicked() {
                                        connect_to_entry = Some((banner_entry.clone(), pw.clone()));
                                    }
                                });
                            });
                        });
                    ui.add_space(8.0);
                }

                // Tabs
                ui.horizontal(|ui| {
                    let tabs = [
                        (ServerTab::Internet, t!("Internet")),
                        (ServerTab::Favorites, t!("Favorites")),
                        (ServerTab::History, t!("History")),
                        (ServerTab::Lan, t!("LAN")),
                        (ServerTab::Friends, t!("Friends")),
                    ];
                    for (tab, label) in tabs {
                        if w::chip(ui, &t, label, self.tab == tab).clicked() {
                            self.tab = tab;
                            self.refresh_tab(ctx);
                        }
                    }
                });
                ui.add_space(10.0);

                // Filters row
                ui.horizontal_wrapped(|ui| {
                    let selected_label = self
                        .filter_app
                        .and_then(|id| ctx.state.app_name(id).map(String::from).or_else(|| Some(format!("App {id}"))))
                        .unwrap_or_else(|| t!("All games").to_string());
                    egui::ComboBox::from_id_salt("srv-app-filter")
                        .selected_text(w::text(selected_label, 13.0, t.text))
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(self.filter_app.is_none(), t!("All games")).clicked() {
                                self.filter_app = None;
                                if self.tab == ServerTab::Internet {
                                    refresh_now = true;
                                }
                            }
                            let mut installed = ctx.local.games.clone();
                            installed.sort_by(|a, b| model::cmp_ignore_case(&a.name, &b.name));
                            for g in installed {
                                if ui.selectable_label(self.filter_app == Some(g.app_id), &g.name).clicked() {
                                    self.filter_app = Some(g.app_id);
                                    if self.tab == ServerTab::Internet {
                                        refresh_now = true;
                                    }
                                }
                            }
                        });
                    // Keep the inner text edits from moving the toolbar cursor inside the fields.
                    ui.scope(|ui| w::search(ui, &t, &mut self.search, t!("Filter server name or IP"), 210.0));
                    ui.scope(|ui| w::search(ui, &t, &mut self.map_filter, t!("Map"), 130.0));
                    ui.checkbox(&mut self.has_players, t!("Has players"));
                    ui.checkbox(&mut self.not_full, t!("Not full"));
                    ui.checkbox(&mut self.no_password, t!("No password"));
                    ui.checkbox(&mut self.vac_secure, t!("VAC secure"));
                });
                ui.add_space(8.0);

                if (self.tab == ServerTab::Internet && self.internet_loading)
                    || (self.tab == ServerTab::Lan && self.lan_loading)
                {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(t.accent));
                        ui.label(w::text(t!("Querying servers…"), 13.0, t.text_dim));
                    });
                    ui.add_space(4.0);
                }
                if self.tab == ServerTab::Internet {
                    if let Some(err) = &self.internet_err {
                        ui.label(w::text(format!("{}  {err}", icons::WARNING), 13.0, t.danger));
                        ui.add_space(4.0);
                    }
                }

                let rows = self.current_rows(ctx);
                let avail_h = ui.available_height();
                let inspector_h = if self.selected.is_some() { 200.0 } else { 0.0 };
                let table_h = (avail_h - inspector_h - 12.0).max(160.0);

                // Header
                egui::Frame::new()
                    .fill(t.chrome)
                    .corner_radius(CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 })
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let mut hdr = |ui: &mut Ui, col: SortCol, label: &str, w_px: f32| {
                                let arrow = if self.sort == col {
                                    if self.sort_asc { " ▲" } else { " ▼" }
                                } else {
                                    ""
                                };
                                let (r, resp) = ui.allocate_exact_size(vec2(w_px, 18.0), Sense::click());
                                ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, format!("{label}{arrow}"), w::font_semibold(12.0), t.text_dim);
                                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    if self.sort == col {
                                        self.sort_asc = !self.sort_asc;
                                    } else {
                                        self.sort = col;
                                        self.sort_asc = matches!(col, SortCol::Name | SortCol::Game | SortCol::Map | SortCol::Ping);
                                    }
                                }
                            };
                            let total_w = ui.available_width().max(640.0);
                            let name_w = (total_w - 420.0).max(180.0);
                            hdr(ui, SortCol::Name, t!("Server"), name_w);
                            hdr(ui, SortCol::Game, t!("Game"), 130.0);
                            hdr(ui, SortCol::Map, t!("Map"), 120.0);
                            hdr(ui, SortCol::Players, t!("Players"), 76.0);
                            hdr(ui, SortCol::Ping, t!("Ping"), 60.0);
                        });
                    });

                egui::ScrollArea::vertical()
                    .id_salt(("srv-rows", self.tab as u8))
                    .max_height(table_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if rows.is_empty() {
                            w::empty(ui, &t, icons::GLOBE, t!("No servers found"), t!("Try refreshing or clearing filters."));
                            return;
                        }
                        for entry in &rows {
                            let sel = self.selected.as_deref() == Some(entry.addr.as_str());
                            let total_w = ui.available_width().max(640.0);
                            let name_w = (total_w - 420.0).max(180.0);
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                            let hover = resp.hovered();
                            if sel {
                                ui.painter().rect_filled(rect, t.corner_small(), t.accent.gamma_multiply(0.18));
                            } else if hover {
                                ui.painter().rect_filled(rect, t.corner_small(), t.surface);
                            }
                            let mut x = rect.left() + 10.0;
                            let flags_prefix = if entry.password { "🔒 " } else { "" };
                            let label = format!("{flags_prefix}{}", if entry.name.is_empty() { &entry.addr } else { &entry.name });
                            ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, label, w::font(13.0), t.text);
                            x += name_w + 4.0;
                            ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, &entry.description, w::font(12.5), t.text_dim);
                            x += 134.0;
                            ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, &entry.map, w::font(12.5), t.text_dim);
                            x += 124.0;
                            ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, format!("{}/{}", entry.players, entry.max_players), w::font(12.5), t.text);
                            x += 80.0;
                            let ping_txt = if entry.ping_ms == 0 { "—".into() } else { format!("{} ms", entry.ping_ms) };
                            ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, ping_txt, w::font(12.5), t.text_dim);

                            if resp.clicked() {
                                self.selected = Some(entry.addr.clone());
                                query_details_addr = Some(entry.addr.clone());
                            }
                            if resp.double_clicked() {
                                connect_to_entry = Some((entry.clone(), None));
                            }
                            let is_fav = self.is_favorite(&entry.addr);
                            resp.context_menu(|ui| {
                                if ui.button(format!("{}  {}", icons::PLAY, t!("Connect"))).clicked() {
                                    connect_to_entry = Some((entry.clone(), None));
                                    ui.close();
                                }
                                let fav_lbl = if is_fav { t!("Remove from Favorites") } else { t!("Add to Favorites") };
                                if ui.button(format!("{}  {fav_lbl}", icons::STAR)).clicked() {
                                    fav_toggle_entry = Some(entry.clone());
                                    ui.close();
                                }
                                if ui.button(format!("{}  {}", icons::COPY, t!("Copy IP:Port"))).clicked() {
                                    ui.ctx().copy_text(entry.addr.clone());
                                    ui.close();
                                }
                                if ui.button(format!("{}  {}", icons::REFRESH, t!("Query server details"))).clicked() {
                                    query_details_addr = Some(entry.addr.clone());
                                    ui.close();
                                }
                            });
                        }
                    });

                // Inspector
                if let Some(addr) = self.selected.clone() {
                    ui.add_space(8.0);
                    let entry = rows
                        .into_iter()
                        .find(|r| r.addr == addr)
                        .unwrap_or_else(|| self.merge_saved(&SavedServer { addr: addr.clone(), ..Default::default() }));
                    let is_fav = self.is_favorite(&addr);
                    egui::Frame::new()
                        .fill(t.chrome)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(w::text_semibold(if entry.name.is_empty() { &addr } else { &entry.name }, 14.5, t.text));
                                    ui.label(w::text(format!("{addr} · {} · {} · {}/{}", entry.description, entry.map, entry.players, entry.max_players), 12.5, t.text_dim));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if w::button(ui, &t, Some(icons::PLAY), t!("Connect"), w::Kind::Primary, w::Size::Sm).clicked() {
                                        connect_to_entry = Some((entry.clone(), None));
                                    }
                                    let fav_lbl = if is_fav { t!("Unfavorite") } else { t!("Favorite") };
                                    if w::button(ui, &t, Some(icons::STAR), fav_lbl, w::Kind::Secondary, w::Size::Sm).clicked() {
                                        fav_toggle_entry = Some(entry.clone());
                                    }
                                    if w::button(ui, &t, Some(icons::COPY), t!("Copy IP"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                        ui.ctx().copy_text(addr.clone());
                                    }
                                    if w::button(ui, &t, Some(icons::REFRESH), t!("Query"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                        query_details_addr = Some(addr.clone());
                                    }
                                });
                            });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if w::chip(ui, &t, t!("Players"), self.detail_tab == DetailTab::Players).clicked() {
                                    self.detail_tab = DetailTab::Players;
                                }
                                if w::chip(ui, &t, t!("Rules"), self.detail_tab == DetailTab::Rules).clicked() {
                                    self.detail_tab = DetailTab::Rules;
                                }
                            });
                            ui.add_space(4.0);
                            egui::ScrollArea::vertical().id_salt("srv-inspector").max_height(105.0).show(ui, |ui| match self.detail_tab {
                                DetailTab::Players => match ctx.state.server_players.get(&addr) {
                                    Some(Ok(players)) if players.is_empty() => {
                                        ui.label(w::text(t!("No player details reported."), 12.5, t.text_faint));
                                    }
                                    Some(Ok(players)) => {
                                        for p in players {
                                            ui.horizontal(|ui| {
                                                ui.label(w::text(&p.name, 12.5, t.text));
                                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                                    ui.label(w::text(format!("Score: {} · {:.0}m", p.score, p.duration_secs / 60.0), 12.0, t.text_dim));
                                                });
                                            });
                                        }
                                    }
                                    Some(Err(e)) => {
                                        ui.label(w::text(e, 12.5, t.text_faint));
                                    }
                                    None => {
                                        ui.label(w::text(t!("Select Query to fetch player details."), 12.5, t.text_faint));
                                    }
                                },
                                DetailTab::Rules => match ctx.state.server_rules.get(&addr) {
                                    Some(Ok(rules)) if rules.is_empty() => {
                                        ui.label(w::text(t!("No rules reported."), 12.5, t.text_faint));
                                    }
                                    Some(Ok(rules)) => {
                                        for (k, v) in rules {
                                            ui.horizontal(|ui| {
                                                ui.label(w::text_semibold(k, 12.0, t.text_dim));
                                                ui.label(w::text(v, 12.0, t.text));
                                            });
                                        }
                                    }
                                    Some(Err(e)) => {
                                        ui.label(w::text(e, 12.5, t.text_faint));
                                    }
                                    None => {
                                        ui.label(w::text(t!("Select Query to fetch server rules."), 12.5, t.text_faint));
                                    }
                                },
                            });
                        });
                }
            });

        // Add Server modal
        if let Some((mut addr_in, mut app_in)) = self.add_dialog.take() {
            let mut keep_open = true;
            ui::widgets::modal(ui.ctx(), egui::Id::unique("add-server-modal")).show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.label(w::text_bold(t!("Add Server to Favorites"), 17.0, t.text));
                ui.add_space(8.0);
                ui.label(w::text(t!("IP:Port (e.g. 198.51.100.10:27015)"), 12.5, t.text_dim));
                ui.add(egui::TextEdit::singleline(&mut addr_in).desired_width(f32::INFINITY));
                ui.add_space(6.0);
                ui.label(w::text(t!("App ID (optional)"), 12.5, t.text_dim));
                ui.add(egui::TextEdit::singleline(&mut app_in).desired_width(f32::INFINITY));
                if let Some(err) = &self.add_error {
                    ui.add_space(4.0);
                    ui.label(w::text(err, 12.5, t.danger));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if w::button(ui, &t, Some(icons::PLUS), t!("Add"), w::Kind::Primary, w::Size::Sm).clicked() {
                        if local::servers::parse_ipv4_port(&addr_in).is_some() {
                            let app_id = app_in.trim().parse::<AppId>().unwrap_or(0);
                            let root = Self::steam_root(ctx);
                            let acct = Self::account_id(ctx);
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let saved = SavedServer {
                                addr: addr_in.trim().to_string(),
                                name: addr_in.trim().to_string(),
                                app_id,
                                game_dir: String::new(),
                                last_played: now,
                            };
                            if let Ok(lists) = local::servers::add_favorite(&root, acct, saved) {
                                self.local_lists = Some((root, acct, lists));
                            }
                            self.tab = ServerTab::Favorites;
                            ctx.send(Command::QueryServer { addr: addr_in.trim().to_string() });
                            keep_open = false;
                        } else {
                            self.add_error = Some(t!("Enter a valid IPv4 address and port (a.b.c.d:port).").to_string());
                        }
                    }
                    if w::button(ui, &t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        keep_open = false;
                    }
                });
            });
            if keep_open {
                self.add_dialog = Some((addr_in, app_in));
            }
        }

        if refresh_now {
            self.refresh_tab(ctx);
        }
        if let Some(addr) = query_details_addr {
            ctx.send(Command::QueryServerDetails { addr });
        }
        if let Some(entry) = fav_toggle_entry {
            self.toggle_favorite(ctx, &entry);
        }
        if let Some((entry, pw)) = connect_to_entry {
            self.connect_server(ctx, &entry, pw.as_deref());
        }
    }
}
