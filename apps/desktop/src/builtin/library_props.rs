//! The game Properties window (a separate OS window, like Steam's).

use ui::widgets::EditMenu;
use super::{GameSettings, Library};
use client::Command;
use egui::{Align, Layout, Margin, Sense, Ui, vec2};
use extensions::{Action, Ctx};
use i18n::t;
use model::{AppId, InstalledGame, OwnedApp, art};
use std::sync::{Arc, Mutex};
use store::events::WorkshopItem as RemoteItem;
use ui::{Theme, icons, widgets as w};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PropTab {
    #[default]
    General,
    Updates,
    Files,
    Betas,
    Dlc,
    Controller,
    Workshop,
    Privacy,
    Customization,
}

const TABS: [(PropTab, &str); 9] = [
    (PropTab::General, "General"),
    (PropTab::Updates, "Updates"),
    (PropTab::Files, "Installed files"),
    (PropTab::Betas, "Versions & betas"),
    (PropTab::Dlc, "DLC"),
    (PropTab::Controller, "Controller"),
    (PropTab::Workshop, "Workshop"),
    (PropTab::Privacy, "Privacy"),
    (PropTab::Customization, "Customization"),
];

#[derive(Default)]
pub struct Props {
    pub app: AppId,
    pub tab: PropTab,
    pub workshop: Option<Arc<Mutex<Option<Vec<RemoteItem>>>>>,
    /// Workshop tab: search, sort, and items picked for batch unsubscribe.
    pub ws_search: String,
    pub ws_sort: WsSort,
    /// 0 any size, 1 under 100 MB, 2 100 MB to 1 GB, 3 over 1 GB.
    pub ws_size: u8,
    pub ws_selecting: bool,
    pub ws_selected: std::collections::BTreeSet<u64>,
    pub ws_confirm: bool,
    pub dlc_search: String,
    pub(super) storage_dialog: Option<super::super::storage_ui::StorageDialog>,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum WsSort {
    #[default]
    Updated,
    Name,
    Size,
}

fn custom_art_kind_id(kind: &str) -> Option<u8> {
    match kind {
        "capsule" => Some(0),
        "hero" => Some(1),
        "logo" => Some(2),
        _ => None,
    }
}

const CUSTOM_ART_REVALIDATE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

static CUSTOM_ART_CACHE: std::sync::LazyLock<
    Mutex<std::collections::HashMap<(AppId, u8), (Option<String>, std::time::Instant)>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

fn invalidate_custom_art(app: AppId, kind: &str) {
    if let Some(k) = custom_art_kind_id(kind) {
        if let Ok(mut map) = CUSTOM_ART_CACHE.lock() {
            map.remove(&(app, k));
        }
    }
}

fn probe_custom_art(app: AppId, kind: &str) -> Option<String> {
    let dir = client::store::config_dir().join("custom-art").join(app.to_string());
    ["png", "jpg", "jpeg", "webp"]
        .iter()
        .map(|ext| dir.join(format!("{kind}.{ext}")))
        .find(|p| p.exists())
        .map(|p| format!("file://{}", p.display().to_string().replace('\\', "/")))
}

/// Custom art the user picked for a game (capsule, hero, logo).
/// Cached in memory per `(AppId, kind)` with a short revalidation TTL and
/// invalidated immediately on Change/Reset to eliminate per-frame `stat()` calls
/// while still picking up edits made from another Aurora instance.
pub fn custom_art(app: AppId, kind: &str) -> Option<String> {
    let Some(k) = custom_art_kind_id(kind) else {
        return probe_custom_art(app, kind);
    };
    if let Ok(map) = CUSTOM_ART_CACHE.lock() {
        if let Some((cached, at)) = map.get(&(app, k))
            && at.elapsed() < CUSTOM_ART_REVALIDATE_TTL
        {
            return cached.clone();
        }
    }
    let found = probe_custom_art(app, kind);
    if let Ok(mut map) = CUSTOM_ART_CACHE.lock() {
        map.insert((app, k), (found.clone(), std::time::Instant::now()));
    }
    found
}

pub fn capsule(app: AppId) -> String {
    custom_art(app, "capsule").unwrap_or_else(|| art::capsule(app))
}
pub fn hero(app: AppId) -> String {
    custom_art(app, "hero").unwrap_or_else(|| art::hero(app))
}
pub fn logo(app: AppId) -> String {
    custom_art(app, "logo").unwrap_or_else(|| art::logo(app))
}

impl Library {
    pub(super) fn properties_window(&mut self, ui: &Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(app_id) = self.props.as_ref().map(|p| p.app) else { return };
        let name = ctx.state.app_name(app_id).map(String::from).or_else(|| ctx.local.games.iter().find(|g| g.app_id == app_id).map(|g| g.name.clone())).unwrap_or_default();
        let installed = ctx.local.games.iter().find(|g| g.app_id == app_id).cloned();
        let owned = ctx.state.apps.as_ref().and_then(|a| a.get(&app_id)).cloned();
        let builder = egui::ViewportBuilder::default().with_title(&name).with_inner_size([860.0, 620.0]).with_min_inner_size([700.0, 480.0]);
        let mut close = false;
        ui.ctx().show_viewport_immediate(egui::ViewportId::from_hash_of(("props", app_id)), builder, |ui, _| {
            if ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
            egui::Panel::left("props-nav")
                .exact_size(210.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.chrome).inner_margin(Margin::same(14)))
                .show(ui, |ui| {
                    let current = self.props.as_ref().map(|p| p.tab).unwrap_or_default();
                    for (tab, label) in TABS {
                        let r = ui.add(egui::Button::selectable(current == tab, w::text_medium(i18n::tr_dyn(label), 14.0, t.text)).min_size(vec2(ui.available_width(), 34.0)));
                        if r.clicked() {
                            if let Some(p) = &mut self.props {
                                p.tab = tab;
                            }
                        }
                    }
                });
            egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(Margin::same(22))).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let tab = self.props.as_ref().map(|p| p.tab).unwrap_or_default();
                    ui.label(w::text_bold(i18n::tr_dyn(TABS.iter().find(|x| x.0 == tab).map(|x| x.1).unwrap_or("")), 24.0, t.text));
                    ui.add_space(10.0);
                    match tab {
                        PropTab::General => {
                            self.tab_cloud(ui, t, ctx, app_id, &name, installed.as_ref(), owned.as_ref());
                            self.tab_general(ui, t, ctx, app_id, owned.as_ref());
                        }
                        PropTab::Updates => self.tab_updates(ui, t, ctx, app_id, installed.as_ref(), owned.as_ref()),
                        PropTab::Files => self.tab_files(ui, t, ctx, app_id, installed.as_ref()),
                        PropTab::Betas => self.tab_betas(ui, t, ctx, app_id, installed.as_ref(), owned.as_ref()),
                        PropTab::Controller => self.tab_controller(ui, t, ctx, app_id),
                        PropTab::Dlc => self.tab_dlc(ui, t, ctx, app_id, installed.as_ref(), owned.as_ref()),
                        PropTab::Workshop => self.tab_workshop(ui, t, ctx, app_id, installed.as_ref()),
                        PropTab::Privacy => self.tab_privacy(ui, t, app_id),
                        PropTab::Customization => self.tab_custom(ui, t, app_id),
                    }
                });
            });
            if ctx.state.moving.contains_key(&app_id)
                || ctx.state.backing_up.contains_key(&app_id)
                || ctx.state.restoring.is_some()
            {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
            if let Some(mut dlg) = self.props.as_mut().and_then(|p| p.storage_dialog.take()) {
                if dlg.show(ui, t, ctx, "props")
                    && let Some(p) = &mut self.props
                {
                    p.storage_dialog = Some(dlg);
                }
            }
        });
        if close {
            self.props = None;
        }
    }

    fn settings_mut(&mut self, app: AppId) -> &mut GameSettings {
        self.games.entry(app).or_default()
    }

    /// Steam Cloud, as in Steam's Properties: toggle, usage, sync now.
    #[allow(clippy::too_many_arguments)]
    fn tab_cloud(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId, name: &str, installed: Option<&InstalledGame>, owned: Option<&OwnedApp>) {
        let Some(cfg) = owned.and_then(|o| o.cloud.clone()) else { return };
        // Usage comes from a look-only check, once per window.
        if let Some(g) = installed {
            if !ctx.state.cloud.contains_key(&app) && !ctx.state.cloud_syncing.contains(&app) && self.cloud_checked.insert(app) {
                ctx.send(Command::CloudSync { app_id: app, install_dir: g.install_dir.clone(), mode: client::cloud::Mode::Check, keep_local: vec![], keep_cloud: vec![], then_launch: None });
            }
        }
        let report = ctx.state.cloud.get(&app).cloned();
        let syncing = ctx.state.cloud_syncing.contains(&app);
        let mut changed = false;
        ui.label(w::text_semibold(t!("Cloud saves"), 16.0, t.text));
        ui.label(w::text(t!("Steam Cloud keeps your game data the same on every PC."), 12.5, t.text_dim));
        ui.add_space(6.0);
        w::panel(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let s = self.games.entry(app).or_default();
                let mut on = !s.cloud_off;
                ui.add(egui::Label::new(w::text(t!("Keep saves for “{name}” in sync with Steam Cloud", name = name), 14.0, t.text)).wrap());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(crate::builtin::settings::toggle(&mut on)).changed() {
                        s.cloud_off = !on;
                        changed = true;
                    }
                });
            });
        });
        // Used / available, like Steam.
        let quota = report.as_ref().map(|r| r.quota_bytes).filter(|q| *q > 0).unwrap_or(cfg.quota);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(w::text_semibold(t!("Available: {size}", size = ui::bytes(quota)), 12.5, t.accent));
                ui.label(w::text("/", 12.5, t.text_faint));
                let used = report.as_ref().map(|r| ui::bytes(r.cloud_bytes)).unwrap_or_else(|| "…".into());
                ui.label(w::text(t!("Used: {size}", size = used), 12.5, t.text_dim));
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            if let Some(g) = installed {
                ui.add_enabled_ui(!syncing, |ui| {
                    if w::button(ui, t, Some(icons::REFRESH), if syncing { t!("Syncing…") } else { t!("Sync now") }, w::Kind::Secondary, w::Size::Sm).clicked() {
                        ctx.send(Command::CloudSync { app_id: app, install_dir: g.install_dir.clone(), mode: client::cloud::Mode::Sync, keep_local: vec![], keep_cloud: vec![], then_launch: None });
                    }
                });
            }
            let backups = client::cloud::backups_dir(app);
            if backups.exists() && w::button(ui, t, Some(icons::FOLDER), t!("Backups of replaced saves"), w::Kind::Ghost, w::Size::Sm).clicked() {
                local::open_folder(&backups);
            }
            if let Some(r) = &report {
                let text = if !r.conflicts.is_empty() {
                    t!("{n} files changed both here and in the cloud", n = r.conflicts.len())
                } else if r.uploaded + r.downloaded > 0 {
                    t!("Synced {when}: {up} up, {down} down", when = ui::ago(r.finished), up = r.uploaded, down = r.downloaded)
                } else {
                    t!("{n} files in the cloud · checked {when}", n = r.cloud_files, when = ui::ago(r.finished))
                };
                ui.label(w::text(text, 12.5, if r.conflicts.is_empty() { t.text_faint } else { t.away }));
            }
        });
        if let (Some(r), Some(g)) = (&report, installed) {
            if !r.conflicts.is_empty() {
                ui.horizontal(|ui| {
                    let names: Vec<String> = r.conflicts.iter().map(|c| c.name.clone()).collect();
                    if w::button(ui, t, Some(icons::DOWNLOAD), t!("Use the cloud's saves"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        ctx.send(Command::CloudSync { app_id: app, install_dir: g.install_dir.clone(), mode: client::cloud::Mode::Sync, keep_local: vec![], keep_cloud: names.clone(), then_launch: None });
                    }
                    if w::button(ui, t, Some(icons::DESKTOP), t!("Keep this PC's saves"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        ctx.send(Command::CloudSync { app_id: app, install_dir: g.install_dir.clone(), mode: client::cloud::Mode::Sync, keep_local: names, keep_cloud: vec![], then_launch: None });
                    }
                });
            }
        }
        if let (Some(r), Some(g)) = (&report, installed) {
            self.cloud_file_list(ui, t, ctx, app, r, &g.install_dir, syncing);
        }
        ui.add_space(14.0);
        if changed {
            self.send_cloud_settings(ctx);
        }
    }

    /// The game's files in Steam Cloud, each with a delete button, like
    /// Steam's "View Steam Cloud" page but inside Aurora.
    #[allow(clippy::too_many_arguments)]
    fn cloud_file_list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId, r: &model::CloudReport, install_dir: &std::path::Path, syncing: bool) {
        if r.files.is_empty() {
            return;
        }
        ui.add_space(8.0);
        let open = self.cloud_files_open.contains(&app);
        ui.horizontal(|ui| {
            let label = t!("Files in the cloud ({n})", n = r.files.len());
            if w::button(ui, t, Some(if open { icons::CARET_DOWN } else { icons::CARET_RIGHT }), &label, w::Kind::Ghost, w::Size::Sm).clicked() {
                if open {
                    self.cloud_files_open.remove(&app);
                } else {
                    self.cloud_files_open.insert(app);
                }
            }
            if open {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(!syncing, |ui| {
                        if w::button(ui, t, Some(icons::TRASH), t!("Delete all"), w::Kind::Ghost, w::Size::Sm).clicked() {
                            let names = r.files.iter().map(|f| f.raw.clone()).collect();
                            self.cloud_delete = Some((app, names, t!("all {n} files", n = r.files.len()).to_string()));
                        }
                    });
                });
            }
        });
        if open {
            w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().id_salt(("cloud_files", app)).max_height(260.0).show(ui, |ui| {
                    for f in &r.files {
                        ui.horizontal(|ui| {
                            ui.label(w::text(icons::NOTE, 13.0, t.text_faint));
                            let when = if f.time > 0 { ui::ago(f.time) } else { String::new() };
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_enabled_ui(!syncing, |ui| {
                                    if w::icon_button_sized(ui, t, icons::TRASH, &t!("Delete from Steam Cloud"), false, 0, 22.0).clicked() {
                                        self.cloud_delete = Some((app, vec![f.raw.clone()], f.name.clone()));
                                    }
                                });
                                ui.label(w::text(when, 12.0, t.text_faint));
                                ui.label(w::text(ui::bytes(f.size), 12.0, t.text_dim));
                                ui.add(egui::Label::new(w::text(&f.name, 12.5, t.text)).truncate()).on_hover_text(&f.name);
                            });
                        });
                    }
                });
            });
        }
        // Confirm before anything leaves the cloud: other PCs lose it too.
        let Some((who, names, what)) = self.cloud_delete.clone() else { return };
        if who != app {
            return;
        }
        let mut close = false;
        w::modal(ui.ctx(), egui::Id::unique(("cloud_delete", app))).show(ui.ctx(), |ui| {
            ui.set_width(440.0);
            ui.label(w::text(icons::TRASH, 28.0, t.danger));
            ui.label(w::text_bold(t!("Delete {what} from Steam Cloud?", what = what), 18.0, t.text));
            ui.add_space(4.0);
            ui.add(egui::Label::new(w::text(t!("Your other PCs won't get it any more. The copy on this PC stays, and Aurora won't upload it again."), 13.0, t.text_dim)).wrap());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::TRASH), t!("Delete"), w::Kind::Danger, w::Size::Md).clicked() {
                    ctx.send(Command::CloudDelete { app_id: app, install_dir: install_dir.to_path_buf(), names: names.clone() });
                    close = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.cloud_delete = None;
        }
    }

    /// DLC, like Steam's tab: choose which to install, with size.
    fn tab_dlc(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId, installed: Option<&InstalledGame>, owned: Option<&OwnedApp>) {
        let mut dlc: Vec<OwnedApp> = ctx.state.apps.as_ref().map(|a| a.values().filter(|d| d.is_dlc() && d.parent == Some(app)).cloned().collect()).unwrap_or_default();
        dlc.sort_by_key(|d| d.name.to_lowercase());
        ui.horizontal(|ui| {
            if w::button(ui, t, Some(icons::STORE), t!("More in store"), w::Kind::Secondary, w::Size::Md).clicked() {
                ctx.navigate("store", Some(format!("app:{app}")));
            }
            let search = &mut self.props.as_mut().expect("open").dlc_search;
            w::search(ui, t, search, &t!("Search…"), 260.0);
        });
        ui.add_space(10.0);
        if dlc.is_empty() {
            ui.label(w::text(t!("You don't own any DLC for this game."), 13.5, t.text_faint));
            return;
        }
        let q = self.props.as_ref().map(|p| p.dlc_search.trim().to_lowercase()).unwrap_or_default();
        // Size of each DLC's own content, from the game's depots.
        let size_of = |d: AppId| -> u64 { owned.map(|o| o.depots.iter().filter(|x| x.dlc_app == Some(d)).filter_map(|x| x.manifest("public").map(|m| m.1)).sum()).unwrap_or(0) };
        let mut changed = false;
        // What's on disk now: a DLC with content counts as installed when its
        // depots are (an update keeps those; ticking adds, unticking removes).
        let on_disk: Vec<local::manage::InstalledDepot> = installed.map(local::manage::installed_depots).unwrap_or_default();
        let has_content = |d: AppId| owned.is_some_and(|o| o.depots.iter().any(|x| x.dlc_app == Some(d)));
        let is_installed = |d: AppId| on_disk.iter().any(|x| x.dlc_app == Some(d));
        egui::Frame::new().fill(t.surface).corner_radius(t.corner_small()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.add_sized(vec2(70.0, 30.0), egui::Label::new(w::caption_text(&t, t!("INSTALL"))));
                ui.add_sized(vec2((ui.available_width() - 110.0).max(0.0), 30.0), egui::Label::new(w::caption_text(&t, t!("NAME"))));
                ui.label(w::caption_text(&t, t!("SIZE")));
            });
            for d in dlc.iter().filter(|d| q.is_empty() || d.name.to_lowercase().contains(&q)) {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    let s = self.games.entry(app).or_default();
                    let mut on = if installed.is_some() && has_content(d.app_id) {
                        (is_installed(d.app_id) && !s.disabled_dlc.contains(&d.app_id)) || s.enabled_dlc.contains(&d.app_id)
                    } else {
                        // Not installed yet: a new install gets owned DLC unless unticked.
                        !s.disabled_dlc.contains(&d.app_id)
                    };
                    if w::check(ui, t, &mut on, "").changed() {
                        if on {
                            s.disabled_dlc.remove(&d.app_id);
                            s.enabled_dlc.insert(d.app_id);
                        } else {
                            s.disabled_dlc.insert(d.app_id);
                            s.enabled_dlc.remove(&d.app_id);
                        }
                        changed = true;
                    }
                    ui.add_space(22.0);
                    // DLC without its own art shows the game's, with a DLC badge, like Steam.
                    let (r, _) = ui.allocate_exact_size(vec2(96.0, 45.0), Sense::hover());
                    let own = d.store_assets && w::paint_cover(ui, r, &art::header(d.app_id), t.corner_small());
                    if !own {
                        w::image_or_skeleton(ui, t, r, &art::header(app), t.corner_small());
                        let badge = egui::Rect::from_min_size(r.min, vec2(28.0, 14.0));
                        ui.painter().rect_filled(badge, egui::CornerRadius { nw: 4, se: 4, ne: 0, sw: 0 }, t.accent_2);
                        ui.painter().text(badge.center(), egui::Align2::CENTER_CENTER, "DLC", w::font_bold(9.0), egui::Color32::WHITE);
                    }
                    ui.add_space(8.0);
                    ui.add_sized(vec2((ui.available_width() - 100.0).max(0.0), 45.0), egui::Label::new(w::text_medium(&d.name, 13.5, t.text)).wrap());
                    let size = size_of(d.app_id);
                    ui.label(w::text(if size > 0 { ui::bytes(size) } else { "—".into() }, 12.5, t.text_dim));
                });
            }
            ui.add_space(6.0);
        });
        if changed {
            ctx.toast(t!("DLC choice saved. Apply now to download or remove it, or it applies on the next update."));
        }
        if let Some(g) = installed {
            ui.add_space(10.0);
            let running = ctx.state.downloads.get(&app).is_some_and(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed) && !d.verifying());
            let hint = if running { t!("Restarts the running download with your DLC choice") } else { t!("Downloads ticked DLC and removes unticked DLC") };
            if w::button(ui, t, Some(icons::DOWNLOAD), t!("Apply now"), w::Kind::Primary, w::Size::Sm).on_hover_text(hint).clicked() {
                // A running download has its file list fixed: start it again.
                if running {
                    ctx.send(Command::CancelDownload { app_id: app });
                }
                self.queue_install(ctx, app, Some(g.library.clone()));
            }
        }
    }

    fn tab_general(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId, owned: Option<&OwnedApp>) {
        let steam_opts = ctx.local.steam_launch_options.get(&app).cloned();
        let mut imported = false;
        let s = self.settings_mut(app);
        section(ui, t, t!("Launch options"), t!("Extra arguments added when the game starts. As in Steam, %command% stands for the game: MANGOHUD=1 gamemoderun %command% -novid"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut s.args).hint_text("MANGOHUD=1 %command% -novid").desired_width(f32::INFINITY).margin(vec2(10.0, 8.0))).with_edit_menu();
            if let Some(opts) = steam_opts.as_deref().filter(|o| o.trim() != s.args.trim()) {
                ui.add_space(6.0);
                if w::button(ui, t, Some(icons::DOWNLOAD), t!("Import from Steam"), w::Kind::Secondary, w::Size::Sm)
                    .on_hover_text(opts)
                    .clicked()
                {
                    s.args = opts.to_string();
                    imported = true;
                }
            }
        });
        if imported {
            ctx.toast(t!("Imported launch options from Steam"));
        }
        section(ui, t, t!("In-game overlay"), t!("Open friends and chat with Shift+Tab while playing."), |ui| {
            if ui.add(crate::builtin::settings::toggle(&mut s.overlay_on)).changed() {
                client::set_app_overlay(app, s.overlay_on);
            }
        });
        let langs: Vec<String> = {
            let mut v: Vec<String> = owned.map(|o| o.depots.iter().map(|d| d.language.clone()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
            v.sort();
            v.dedup();
            v
        };
        if !langs.is_empty() {
            section(ui, t, t!("Language"), t!("Which language to download for this game."), |ui| {
                let current = if s.language.is_empty() { t!("Same as Aurora").to_string() } else { s.language.clone() };
                egui::ComboBox::from_id_salt(("game-lang", app)).selected_text(current).width(240.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.language, String::new(), t!("Same as Aurora"));
                    for l in &langs {
                        ui.selectable_value(&mut s.language, l.clone(), l);
                    }
                });
            });
        }
    }

    fn tab_updates(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId, installed: Option<&InstalledGame>, owned: Option<&OwnedApp>) {
        let branch = self.games.get(&app).map(|g| g.branch.clone()).filter(|b| !b.is_empty()).unwrap_or_else(|| "public".into());
        if let Some(g) = installed {
            let (build, _) = local::manage::installed_build(g);
            let latest = owned.and_then(|o| o.branches.iter().find(|b| b.name == branch)).map(|b| (b.build_id, b.updated));
            w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new("upd-grid").num_columns(2).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
                    ui.label(w::caption_text(&t, t!("INSTALLED BUILD")));
                    ui.label(w::text(if build == 0 { "—".into() } else { build.to_string() }, 13.5, t.text));
                    ui.end_row();
                    if let Some((b, when)) = latest {
                        ui.label(w::caption_text(&t, t!("LATEST BUILD")));
                        ui.label(w::text(format!("{b}  ·  {}", super::news::date(when)), 13.5, t.text));
                        ui.end_row();
                    }
                    ui.label(w::caption_text(&t, t!("LAST UPDATED")));
                    ui.label(w::text(g.last_updated.map(super::news::date).unwrap_or_else(|| "—".into()), 13.5, t.text));
                    ui.end_row();
                });
                ui.add_space(8.0);
                match latest {
                    Some((b, _)) if b != 0 && build != 0 && b != build => {
                        ui.label(w::text(format!("{}  {}", icons::DOWNLOAD, t!("An update is available.")), 14.0, t.away));
                        if w::button(ui, t, Some(icons::DOWNLOAD), t!("Update now"), w::Kind::Primary, w::Size::Sm).clicked() {
                            self.queue_install(ctx, app, Some(g.library.clone()));
                        }
                    }
                    Some(_) => {
                        ui.label(w::text(format!("{}  {}", icons::CHECK, t!("Up to date")), 14.0, t.play));
                    }
                    None => {}
                }
            });
            ui.add_space(12.0);
        }
        let s = self.settings_mut(app);
        section(ui, t, t!("Automatic updates"), t!("When this game should be updated."), |ui| {
            let opts = [(0u8, t!("Always keep this game updated")), (1, t!("Only update when I launch it")), (2, t!("High priority: update before other games"))];
            for (v, l) in opts {
                w::radio_value(ui, t, &mut s.auto_update, v, l);
            }
        });
        section(ui, t, t!("Background downloads"), t!("Whether updates for this game may download while you play something else."), |ui| {
            let opts = [(0u8, t!("Use the global setting")), (1, t!("Always allow")), (2, t!("Never allow"))];
            for (v, l) in opts {
                w::radio_value(ui, t, &mut s.background, v, l);
            }
        });
    }

    fn tab_files(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId, installed: Option<&InstalledGame>) {
        // Being downloaded: its folder already exists, so it can be opened.
        match super::setup_state(ctx, app, installed.is_some()) {
            super::Setup::Downloading(d) => {
                w::panel(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let pct = d.disk_done as f64 * 100.0 / d.disk_total.max(1) as f64;
                    let label = if d.paused { t!("Download paused at {pct}%", pct = format!("{pct:.0}")) } else { t!("Installing… {pct}%", pct = format!("{pct:.0}")) };
                    ui.label(w::text_semibold(label, 15.0, t.accent));
                    ui.label(w::text(format!("{} / {}", ui::bytes(d.disk_done), ui::bytes(d.disk_total)), 12.5, t.text_dim));
                    w::progress(ui, t, (pct / 100.0) as f32, ui.available_width());
                    if let Some(dir) = &d.install_dir {
                        ui.add_space(6.0);
                        let path = dir.display().to_string();
                        ui.add(egui::Label::new(w::text(&path, 12.5, t.text_dim).monospace()).truncate()).on_hover_text(&path);
                    }
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                        if let Some(dir) = d.install_dir.as_ref().filter(|p| p.exists()) {
                            if w::button(ui, t, Some(icons::FOLDER), t!("Browse…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                local::open_folder(dir);
                            }
                        }
                        let (icon, label, cmd) = if d.paused {
                            (icons::PLAY, t!("Resume download"), Command::ResumeDownload { app_id: app })
                        } else {
                            ("\u{E39E}", t!("Pause download"), Command::PauseDownload { app_id: app })
                        };
                        if w::button(ui, t, Some(icon), label, w::Kind::Secondary, w::Size::Sm).clicked() {
                            ctx.send(cmd);
                        }
                        if w::button(ui, t, Some(icons::X), t!("Cancel download"), w::Kind::Ghost, w::Size::Sm).clicked() {
                            ctx.send(Command::CancelDownload { app_id: app });
                        }
                    });
                });
                return;
            }
            super::Setup::Queued if installed.is_none() => {
                ui.label(w::text(t!("Waiting in the download queue."), 14.0, t.accent));
                if w::button(ui, t, Some(icons::X), t!("Remove from download queue"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    ctx.send(Command::CancelDownload { app_id: app });
                }
                return;
            }
            _ => {}
        }
        let Some(g) = installed else {
            ui.label(w::text(t!("This game isn't installed."), 14.0, t.text_dim));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                if w::button(ui, t, Some(icons::DOWNLOAD), t!("Install"), w::Kind::Primary, w::Size::Md).clicked() {
                    self.install_dialog = Some(super::InstallDialog::new(app, ctx));
                }
                if w::button(ui, t, Some(icons::RESTORE), t!("Restore game backup…"), w::Kind::Secondary, w::Size::Md).clicked() {
                    let dlg = super::super::storage_ui::StorageDialog::Restore(super::super::storage_ui::RestoreDialog::new(ctx));
                    if let Some(p) = &mut self.props {
                        p.storage_dialog = Some(dlg);
                    }
                }
            });
            if let Some(r) = &ctx.state.restoring {
                ui.add_space(8.0);
                super::super::storage_ui::restore_progress_card(ui, t, ctx, r);
            }
            return;
        };
        let game_name = ctx
            .state
            .app_name(app)
            .map(String::from)
            .unwrap_or_else(|| g.name.clone());
        let op_busy = ctx.state.playing == Some(app)
            || ctx.state.moving.contains_key(&app)
            || ctx.state.backing_up.contains_key(&app)
            || ctx.state.downloads.get(&app).is_some_and(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed));
        w::panel(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::text_semibold(t!("Size of installation: {size}", size = ui::bytes(g.size_on_disk)), 15.0, t.text));
            let path = g.install_dir.display().to_string();
            ui.add(egui::Label::new(w::text(&path, 12.5, t.text_dim).monospace()).truncate()).on_hover_text(&path);
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                if w::button(ui, t, Some(icons::FOLDER), t!("Browse…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    local::open_folder(&g.install_dir);
                }
                #[cfg(not(windows))]
                if let Some(pfx) = local::compat::prefix_dir(&g.install_dir, app) {
                    if w::button(ui, t, Some(icons::FOLDER), t!("Browse Proton prefix"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        local::open_folder(&pfx);
                    }
                }
                if w::button(ui, t, Some(icons::SHIELD), t!("Verify integrity of game files"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    self.start_verify(ctx, g);
                }
                if ui.add_enabled_ui(!op_busy, |ui| w::button(ui, t, Some(icons::DRIVES), t!("Move install folder…"), w::Kind::Secondary, w::Size::Sm)).inner.clicked() {
                    let dlg = super::super::storage_ui::StorageDialog::Move(super::super::storage_ui::MoveDialog::new(g.clone(), game_name.clone(), ctx));
                    if let Some(p) = &mut self.props {
                        p.storage_dialog = Some(dlg);
                    }
                }
                if ui.add_enabled_ui(!op_busy, |ui| w::button(ui, t, Some(icons::PACKAGE), t!("Back up game files…"), w::Kind::Secondary, w::Size::Sm)).inner.clicked() {
                    let dlg = super::super::storage_ui::StorageDialog::Backup(super::super::storage_ui::BackupDialog::new(g.clone(), game_name.clone()));
                    if let Some(p) = &mut self.props {
                        p.storage_dialog = Some(dlg);
                    }
                }
                if w::button(ui, t, Some(icons::DESKTOP), t!("Add desktop shortcut"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    self.add_shortcut(ctx, app, g);
                }
            });
        });
        if let Some(mp) = ctx.state.moving.get(&app) {
            ui.add_space(8.0);
            super::super::storage_ui::move_progress_panel(ui, t, ctx, app, mp);
        }
        if let Some(bp) = ctx.state.backing_up.get(&app) {
            ui.add_space(8.0);
            super::super::storage_ui::backup_progress_panel(ui, t, ctx, app, bp);
        }
        if let Some(d) = ctx.state.downloads.get(&app).filter(|d| d.verifying()) {
            ui.add_space(8.0);
            ui.label(w::text(t!("Verifying files…"), 13.5, t.text_dim));
            w::progress(ui, t, d.fraction(), ui.available_width());
        }
        if let Some(r) = self.reports.get(&app) {
            ui.add_space(10.0);
            let (icon, color) = if r.healthy() { (icons::CHECK, t.play) } else { (icons::WARNING, t.away) };
            ui.label(w::text_semibold(format!("{icon}  {}", t!("Last check: {ok} of {total} files OK", ok = r.ok, total = r.files)), 14.0, color));
            if !r.healthy() && w::button(ui, t, Some(icons::DOWNLOAD), t!("Repair now"), w::Kind::Primary, w::Size::Sm).clicked() {
                self.queue_install(ctx, app, Some(g.library.clone()));
            }
        }
        ui.add_space(14.0);
        if w::button(ui, t, Some(icons::TRASH), t!("Uninstall…"), w::Kind::Danger, w::Size::Sm).clicked() {
            self.dialog = Some(super::Dialog::Uninstall);
            self.selected = Some(app);
        }
    }

    fn tab_betas(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId, installed: Option<&InstalledGame>, owned: Option<&OwnedApp>) {
        let branches = owned.map(|o| o.branches.clone()).unwrap_or_default();
        let installed_branch = installed.map(|g| local::manage::installed_build(g).1).unwrap_or_else(|| "public".into());
        ui.label(w::text(t!("Choose which version of the game to play. Older versions and betas are provided by the developer."), 13.5, t.text_dim));
        ui.add_space(8.0);
        if branches.is_empty() {
            ui.label(w::text(t!("Version information appears once your owned games are loaded from Steam."), 13.5, t.text_faint));
            return;
        }
        let mut chosen = self.games.get(&app).map(|g| g.branch.clone()).filter(|b| !b.is_empty()).unwrap_or(installed_branch.clone());
        for b in &branches {
            let label = if b.name == "public" { t!("None (default version)").to_string() } else { b.name.clone() };
            let r = w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    w::radio_value(ui, t, &mut chosen, b.name.clone(), w::text_semibold(label, 14.0, t.text));
                    if b.password {
                        w::pill(ui, t!("Needs a code"), t.away, t.away.gamma_multiply(0.15));
                    }
                    if b.name == installed_branch {
                        w::pill(ui, t!("Installed"), t.play, t.play.gamma_multiply(0.15));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(w::text(format!("{} {}  ·  {}", t!("build"), b.build_id, super::news::date(b.updated)), 12.0, t.text_faint));
                    });
                });
                if !b.description.is_empty() {
                    ui.label(w::text(&b.description, 12.5, t.text_dim));
                }
            });
            let _ = r;
            ui.add_space(6.0);
        }
        let s = self.settings_mut(app);
        s.branch = chosen.clone();
        ui.add_space(8.0);
        section(ui, t, t!("Private beta code"), t!("Some betas need a code from the developer."), |ui| {
            ui.add(egui::TextEdit::singleline(&mut s.beta_code).desired_width(260.0).margin(vec2(10.0, 8.0))).with_edit_menu();
        });
        // Like Steam: an installed game downloads the other version on
        // request; one that isn't installed just remembers the choice.
        match installed {
            Some(g) if chosen != installed_branch => {
                ui.label(w::text(t!("Switching versions downloads the files for that version."), 13.0, t.away));
                if w::button(ui, t, Some(icons::DOWNLOAD), t!("Switch now"), w::Kind::Primary, w::Size::Sm).clicked() {
                    self.queue_install(ctx, app, Some(g.library.clone()));
                }
            }
            None if chosen != "public" => {
                ui.label(w::text(t!("This version installs when you install the game."), 13.0, t.text_dim));
            }
            _ => {}
        }
    }

    pub(crate) fn set_game_controller(&mut self, app: AppId, choice: u8) {
        self.settings_mut(app).controller = choice;
        client::set_app_controller(app, choice);
    }

    fn tab_controller(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId) {
        let mut choice = self.settings_mut(app).controller;
        section(ui, t, t!("Steam Input"), t!("Override controller handling for this game."), |ui| {
            for (v, l) in [(0u8, t!("Use the default settings")), (1, t!("Enable Steam Input")), (2, t!("Disable Steam Input"))] {
                if w::radio_value(ui, t, &mut choice, v, l).changed() {
                    self.set_game_controller(app, choice);
                }
            }
            ui.add_space(6.0);
            let cfg = client::controller_settings();
            let decision = client::resolve_steam_input(choice, &cfg.families);
            let status = if !decision.enabled && choice == 2 {
                t!("Aurora Steam Input and non-Xbox controller bridging are disabled for this game.").to_string()
            } else if !decision.enabled {
                t!("All controller families are currently turned off in global Controller settings.").to_string()
            } else {
                let mut active = Vec::new();
                if cfg.families.xbox { active.push(t!("Xbox")); }
                if cfg.families.playstation { active.push(t!("PlayStation")); }
                if cfg.families.switch_pro { active.push(t!("Switch Pro")); }
                if cfg.families.generic { active.push(t!("Generic")); }
                t!("Enabled for: {list}", list = active.join(", "))
            };
            ui.add(egui::Label::new(w::text(status, 12.5, t.text_dim)).wrap());
            ui.add_space(6.0);
            if w::button(ui, t, Some(icons::CONTROLLER), t!("Controller Settings…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                ctx.navigate("settings", Some(format!("controller:{app}")));
            }
        });
    }

    fn tab_workshop(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, app: AppId, installed: Option<&InstalledGame>) {
        let Some(g) = installed else {
            ui.label(w::text(t!("This game isn't installed."), 14.0, t.text_dim));
            return;
        };
        let items = local::manage::workshop_items(g);
        let total: u64 = items.iter().map(|i| i.size).sum();
        ui.horizontal(|ui| {
            ui.label(w::text_semibold(t!("{n} items · {size}", n = items.len(), size = ui::bytes(total)), 15.0, t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::STORE), t!("Browse the Workshop"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    ctx.act(Action::Navigate { page: "workshop".into(), arg: Some(app.to_string()) });
                }
                if w::button(ui, t, Some(icons::DOWNLOAD), t!("Download subscribed items"), w::Kind::Secondary, w::Size::Sm)
                    .on_hover_text(t!("Downloads every item you subscribe to that's missing or out of date. This also happens before the game starts."))
                    .clicked()
                {
                    ctx.send(Command::SyncWorkshop { app_id: app });
                    ctx.toast(t!("Checking your Workshop subscriptions…"));
                }
            });
        });
        ui.add_space(8.0);
        if items.is_empty() {
            ui.label(w::text(t!("You're not subscribed to any Workshop items for this game."), 13.5, t.text_faint));
            return;
        }
        // Titles and previews from Steam (one request for all items).
        let need_fetch = self.props.as_ref().is_some_and(|p| p.workshop.is_none());
        if need_fetch {
            let slot = Arc::new(Mutex::new(None));
            if let (Some(rt), Some(ectx)) = (&self.rt, &self.egui) {
                let ids: Vec<u64> = items.iter().map(|i| i.id).collect();
                let (out, ectx) = (slot.clone(), ectx.clone());
                rt.spawn(async move {
                    let r = store::StoreClient::default().workshop_details(&ids).await.unwrap_or_default();
                    *out.lock().unwrap() = Some(r);
                    ectx.request_repaint();
                });
            }
            if let Some(p) = &mut self.props {
                p.workshop = Some(slot);
            }
        }
        let remote = self.props.as_ref().and_then(|p| p.workshop.as_ref()).and_then(|s| s.lock().unwrap().clone()).unwrap_or_default();
        let Some(props) = self.props.as_mut() else { return };
        let title_of = |id: u64| remote.iter().find(|r| r.id == id).map(|r| r.title.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("#{id}"));

        // Toolbar: search, sort, select mode.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            w::search(ui, t, &mut props.ws_search, &t!("Search your items"), 220.0);
            w::segmented(ui, t, &mut props.ws_sort, &[(WsSort::Updated, &t!("Recently updated")), (WsSort::Name, &t!("Name")), (WsSort::Size, &t!("Size"))]);
            w::segmented(ui, t, &mut props.ws_size, &[(0u8, &t!("Any size")), (1, &t!("Under 100 MB")), (2, &t!("100 MB – 1 GB")), (3, &t!("Over 1 GB"))]);
            let label = if props.ws_selecting { t!("Done") } else { t!("Select") };
            if w::toggle_chip(ui, t, label, &mut props.ws_selecting).changed() && !props.ws_selecting {
                props.ws_selected.clear();
            }
        });
        let q = props.ws_search.trim().to_lowercase();
        const MB: u64 = 1024 * 1024;
        let size_ok = |n: u64| match props.ws_size {
            1 => n < 100 * MB,
            2 => (100 * MB..1024 * MB).contains(&n),
            3 => n >= 1024 * MB,
            _ => true,
        };
        let mut shown: Vec<&local::manage::WorkshopItem> = items
            .iter()
            .filter(|i| size_ok(i.size))
            .filter(|i| q.is_empty() || title_of(i.id).to_lowercase().contains(&q) || i.id.to_string().contains(&q))
            .collect();
        match props.ws_sort {
            WsSort::Updated => shown.sort_by_key(|i| std::cmp::Reverse(i.updated)),
            WsSort::Name => shown.sort_by_key(|i| title_of(i.id).to_lowercase()),
            WsSort::Size => shown.sort_by_key(|i| std::cmp::Reverse(i.size)),
        }
        // Batch bar.
        if props.ws_selecting {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let all = !shown.is_empty() && shown.iter().all(|i| props.ws_selected.contains(&i.id));
                let mut pick_all = all;
                if w::check(ui, t, &mut pick_all, t!("Select all")).changed() {
                    if pick_all {
                        props.ws_selected.extend(shown.iter().map(|i| i.id));
                    } else {
                        props.ws_selected.clear();
                    }
                }
                let n = props.ws_selected.len();
                let size: u64 = items.iter().filter(|i| props.ws_selected.contains(&i.id)).map(|i| i.size).sum();
                ui.label(w::text(t!("{n} selected · {size}", n = n, size = ui::bytes(size)), 13.0, t.text_dim));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(n > 0, |ui| {
                        if w::button(ui, t, Some(icons::TRASH), &t!("Unsubscribe selected ({n})", n = n), w::Kind::Danger, w::Size::Sm).clicked() {
                            props.ws_confirm = true;
                        }
                    });
                });
            });
        }
        ui.add_space(8.0);
        let mut removed: Vec<u64> = Vec::new();
        for item in &shown {
            let info = remote.iter().find(|r| r.id == item.id);
            let picked = props.ws_selected.contains(&item.id);
            let frame = w::panel(t).stroke(egui::Stroke::new(1.0, if picked { t.accent } else { t.border }));
            let resp = frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if props.ws_selecting {
                        let mut on = picked;
                        if w::check(ui, t, &mut on, "").changed() {
                            if on {
                                props.ws_selected.insert(item.id);
                            } else {
                                props.ws_selected.remove(&item.id);
                            }
                        }
                    }
                    let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                    match info.and_then(|i| i.preview.as_deref()) {
                        Some(u) => w::image_or_skeleton(ui, t, r, u, t.corner_small()),
                        None => w::placeholder(ui, t, r, "", t.corner_small()),
                    }
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(w::text_semibold(title_of(item.id), 14.0, t.text)).truncate());
                        ui.label(w::text(format!("{}  ·  {}", ui::bytes(item.size), t!("updated {when}", when = super::news::date(item.updated))), 12.5, t.text_faint));
                    });
                    if !props.ws_selecting {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::button(ui, t, Some(icons::TRASH), t!("Unsubscribe"), w::Kind::Danger, w::Size::Sm).on_hover_text(t!("Remove it from this game and delete its files")).clicked() {
                                ctx.send(Command::UnsubscribeWorkshop { app_id: app, file_id: item.id });
                                let _ = local::manage::delete_workshop_files(item);
                                removed.push(item.id);
                            }
                            if w::icon_button(ui, t, icons::FOLDER, t!("Open folder"), false, 0).clicked() {
                                local::open_folder(&item.folder);
                            }
                            if w::icon_button(ui, t, icons::CUBE, t!("Workshop page"), false, 0).clicked() {
                                ctx.navigate("workshop_item", Some(item.id.to_string()));
                            }
                        });
                    }
                });
            });
            // In select mode the whole row toggles.
            if props.ws_selecting && resp.response.interact(Sense::click()).clicked() {
                if !props.ws_selected.remove(&item.id) {
                    props.ws_selected.insert(item.id);
                }
            }
            ui.add_space(6.0);
        }
        if shown.is_empty() {
            ui.label(w::text(t!("No items match your search."), 13.0, t.text_faint));
        }
        // Confirm batch unsubscribe.
        if props.ws_confirm {
            let n = props.ws_selected.len();
            let mut go = false;
            let mut cancel = false;
            ui::widgets::modal(ui.ctx(), egui::Id::unique(("ws-confirm", app))).show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.label(w::text_bold(t!("Unsubscribe from {n} items?", n = n), 17.0, t.text));
                ui.add(egui::Label::new(w::text(t!("They'll be removed from this game and their files deleted. You can subscribe again later from the Workshop."), 13.0, t.text_dim)).wrap());
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if w::button(ui, t, Some(icons::TRASH), t!("Unsubscribe"), w::Kind::Danger, w::Size::Md).clicked() {
                        go = true;
                    }
                    if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Md).clicked() {
                        cancel = true;
                    }
                });
            });
            if go {
                for item in items.iter().filter(|i| props.ws_selected.contains(&i.id)) {
                    ctx.send(Command::UnsubscribeWorkshop { app_id: app, file_id: item.id });
                    let _ = local::manage::delete_workshop_files(item);
                    removed.push(item.id);
                }
                props.ws_selected.clear();
                props.ws_selecting = false;
            }
            if go || cancel {
                props.ws_confirm = false;
            }
        }
        match removed.len() {
            0 => {}
            1 => ctx.toast(t!("Unsubscribed from item {id}", id = removed[0])),
            n => ctx.toast(t!("Unsubscribed from {n} items", n = n)),
        }
    }

    fn tab_privacy(&mut self, ui: &mut Ui, t: &Theme, app: AppId) {
        let mut hidden = self.hidden.contains(&app);
        section(ui, t, t!("Hide in library"), t!("Hidden games only show when you turn on “Show hidden games”."), |ui| {
            if ui.add(crate::builtin::settings::toggle(&mut hidden)).changed() {
                if hidden {
                    self.hidden.insert(app);
                } else {
                    self.hidden.remove(&app);
                }
            }
        });
        let s = self.settings_mut(app);
        section(ui, t, t!("Mark as private"), t!("Don't show this game as “now playing” in your status while you play."), |ui| {
            ui.add(crate::builtin::settings::toggle(&mut s.private));
        });
    }

    fn tab_custom(&mut self, ui: &mut Ui, t: &Theme, app: AppId) {
        for (kind, title, body, size) in [
            ("capsule", t!("Capsule"), t!("Shown in the library grid and collections. Best at 600 × 900."), vec2(100.0, 150.0)),
            ("hero", t!("Background"), t!("Shown at the top of the game's page. Best at 3840 × 1240."), vec2(220.0, 71.0)),
            ("logo", t!("Logo"), t!("Shown over the background. Transparent PNG works best."), vec2(220.0, 80.0)),
        ] {
            w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let url = match kind {
                        "capsule" => capsule(app),
                        "hero" => hero(app),
                        _ => logo(app),
                    };
                    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                    if !w::paint_cover(ui, r, &url, t.corner_small()) {
                        w::placeholder(ui, t, r, "", t.corner_small());
                    }
                    ui.vertical(|ui| {
                        ui.label(w::text_semibold(title, 15.0, t.text));
                        ui.add(egui::Label::new(w::text(body, 12.5, t.text_dim)).wrap());
                        ui.horizontal(|ui| {
                            if w::button(ui, t, Some(icons::IMAGE), t!("Change"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                if let Some(src) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp"]).pick_file() {
                                    let ctx_clone = ui.ctx().clone();
                                    let kind = kind.to_string();
                                    std::thread::spawn(move || {
                                        if install_custom_art(&src, app, &kind).is_ok() {
                                            invalidate_custom_art(app, &kind);
                                            ctx_clone.forget_all_images();
                                            ctx_clone.request_repaint();
                                        }
                                    });
                                }
                            }
                            let has_custom = custom_art(app, kind).is_some();
                            if ui.add_enabled_ui(has_custom, |ui| w::button(ui, t, None, t!("Reset"), w::Kind::Ghost, w::Size::Sm)).inner.clicked() {
                                let ctx_clone = ui.ctx().clone();
                                let kind = kind.to_string();
                                std::thread::spawn(move || {
                                    reset_custom_art(app, &kind);
                                    invalidate_custom_art(app, &kind);
                                    ctx_clone.forget_all_images();
                                    ctx_clone.request_repaint();
                                });
                            }
                        });
                    });
                });
            });
            ui.add_space(8.0);
        }
    }
}

/// Title + description + control block used throughout Properties.
fn section(ui: &mut Ui, t: &Theme, title: &str, body: &str, add: impl FnOnce(&mut Ui)) {
    w::panel(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(w::text_semibold(title, 15.0, t.text));
        ui.label(w::text(body, 12.5, t.text_dim));
        ui.add_space(6.0);
        add(ui);
    });
    ui.add_space(8.0);
}

const MAX_CUSTOM_ART_BYTES: u64 = 16 * 1024 * 1024;
static CUSTOM_ART_LOCK: std::sync::LazyLock<Mutex<()>> = std::sync::LazyLock::new(|| Mutex::new(()));

fn allowed_art_ext_from_magic(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if header.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn ensure_real_custom_art_dir(app: AppId) -> Result<std::path::PathBuf, String> {
    let base = client::store::config_dir().join("custom-art");
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let base_meta = std::fs::symlink_metadata(&base).map_err(|e| e.to_string())?;
    if base_meta.file_type().is_symlink() || !base_meta.is_dir() {
        return Err("custom-art directory is not a real directory".into());
    }
    let dir = base.join(app.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dir_meta = std::fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return Err("custom-art app directory is not a real directory".into());
    }
    Ok(dir)
}

fn reset_custom_art(app: AppId, kind: &str) {
    if !matches!(kind, "capsule" | "hero" | "logo") {
        return;
    }
    let _guard = CUSTOM_ART_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = client::store::config_dir().join("custom-art").join(app.to_string());
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir()) {
        return;
    }
    for ext in ["png", "jpg", "jpeg", "webp"] {
        let p = dir.join(format!("{kind}.{ext}"));
        if std::fs::symlink_metadata(&p).is_ok_and(|m| !m.file_type().is_symlink() && m.is_file()) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Validate and copy a user-selected custom art image into Aurora's config
/// directory: allowlists image extension & magic bytes, refuses symlinks and
/// files larger than 16 MiB, stages via exclusive unique temp files, serializes
/// against concurrent Change/Reset, and preserves old art until rename succeeds.
fn install_custom_art(src: &std::path::Path, app: AppId, kind: &str) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    if !matches!(kind, "capsule" | "hero" | "logo") {
        return Err("invalid custom art kind".into());
    }
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp"))
        .ok_or_else(|| "unsupported custom art extension".to_string())?;
    let pre = std::fs::symlink_metadata(src).map_err(|e| e.to_string())?;
    if pre.file_type().is_symlink() || !pre.is_file() {
        return Err("custom art source must be a regular file".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if (pre.file_attributes() & 0x0400) != 0 {
            return Err("custom art source must not be a reparse point".into());
        }
    }
    if pre.len() > MAX_CUSTOM_ART_BYTES {
        return Err("custom art exceeds 16 MiB limit".into());
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.custom_flags(0x0020_0000);
    }
    let file = opts.open(src).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_CUSTOM_ART_BYTES {
        return Err("custom art must be a regular file <= 16 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CUSTOM_ART_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_CUSTOM_ART_BYTES {
        return Err("custom art exceeds 16 MiB limit".into());
    }
    let magic_ext = allowed_art_ext_from_magic(&bytes).ok_or_else(|| "custom art must be a valid PNG, JPEG, or WebP image".to_string())?;
    let canonical_ext = if ext == "jpeg" && magic_ext == "jpg" { "jpg" } else { magic_ext };

    let _guard = CUSTOM_ART_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = ensure_real_custom_art_dir(app)?;
    let pid = std::process::id();
    let (tmp, mut out) = loop {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let candidate = dir.join(format!(".{kind}.{pid}.{seq}.tmp"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(f) => break (candidate, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    };
    if let Err(e) = out.write_all(&bytes) {
        drop(out);
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    drop(out);

    let final_path = dir.join(format!("{kind}.{canonical_ext}"));
    if let Ok(m) = std::fs::symlink_metadata(&final_path) {
        if m.file_type().is_symlink() {
            let _ = std::fs::remove_file(&tmp);
            return Err("refusing to overwrite symlinked custom art file".into());
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    // Only remove previous alternate extensions after the new file is safely in place.
    for old_ext in ["png", "jpg", "jpeg", "webp"] {
        if old_ext == canonical_ext {
            continue;
        }
        let p = dir.join(format!("{kind}.{old_ext}"));
        if std::fs::symlink_metadata(&p).is_ok_and(|m| !m.file_type().is_symlink() && m.is_file()) {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(())
}
