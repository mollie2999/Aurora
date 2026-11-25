//! Controller-first per-game Manage & Properties page for Big Picture / Steam Deck Game Mode:
//! Favorite, Hide, Verify files, Steam Cloud status & sync/conflicts, In-game overlay,
//! Updates policy, Betas/Versions, DLC toggles, Move install folder, and confirmed Uninstall.

use super::focus::Place;
use super::paint::{self, Kind};
use super::screens::{button, compact_row, heading, with_library};
use super::{BigPicture, Frame, Page};
use client::Command;
use egui::{Align2, Id, Rect, Ui, pos2, vec2};
use i18n::t;
use model::AppId;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use ui::{icons, widgets as w};

/// Cached filesystem probes for the currently open Manage page (avoids per-frame I/O).
#[derive(Default)]
pub struct ManageState {
    pub app_id: Option<AppId>,
    cache_key: Option<(AppId, Option<PathBuf>, u64, Option<u64>, usize, bool)>,
    probed_at: Option<Instant>,
    pub installed_build: (u64, String),
    pub installed_depots: Vec<local::manage::InstalledDepot>,
    pub move_targets: Vec<(PathBuf, Option<u64>)>,
    pub confirm_uninstall: bool,
    pub cloud_checked: bool,
}

impl ManageState {
    pub fn refresh_if_needed(
        &mut self,
        app_id: AppId,
        installed: Option<&model::InstalledGame>,
        libraries: &[PathBuf],
        busy: bool,
    ) {
        let key = (
            app_id,
            installed.map(|g| g.install_dir.clone()),
            installed.map_or(0, |g| g.size_on_disk),
            installed.and_then(|g| g.last_updated),
            libraries.len(),
            busy,
        );
        let app_changed = self.app_id != Some(app_id);
        let stale = self.probed_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(10));
        if !app_changed && self.cache_key.as_ref() == Some(&key) && !stale {
            return;
        }
        if app_changed {
            self.app_id = Some(app_id);
            self.confirm_uninstall = false;
            self.cloud_checked = false;
        }
        self.cache_key = Some(key);
        self.probed_at = Some(Instant::now());
        if let Some(g) = installed {
            self.installed_build = local::manage::installed_build(g);
            self.installed_depots = local::manage::installed_depots(g);
            self.move_targets = libraries
                .iter()
                .filter(|lib| **lib != g.library)
                .map(|lib| (lib.clone(), local::manage::available_space(lib)))
                .collect();
        } else {
            self.installed_build = (0, "public".into());
            self.installed_depots.clear();
            self.move_targets.clear();
        }
    }
}

fn auto_update_label(mode: u8) -> &'static str {
    match mode {
        1 => t!("Only update when I launch it"),
        2 => t!("High priority: update before other games"),
        _ => t!("Always keep this game updated"),
    }
}

fn background_dl_label(mode: u8) -> &'static str {
    match mode {
        1 => t!("Always allow"),
        2 => t!("Never allow"),
        _ => t!("Use the global setting"),
    }
}

pub fn page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: AppId) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let width = (f.view.width() - 2.0 * f.pad).min(1040.0 * s);
    let mut y = top + 20.0 * s;

    bp.bg_want = Some(id);
    let installed = f.app.local.games.iter().find(|g| g.app_id == id).cloned();
    let owned = f.app.state.apps.as_ref().and_then(|a| a.get(&id)).cloned();
    let name = f
        .app
        .state
        .app_name(id)
        .map(String::from)
        .or_else(|| installed.as_ref().map(|g| g.name.clone()))
        .unwrap_or_else(|| format!("App {id}"));

    let (snap, op_busy) = with_library(f.app, f.actions, |lib, cx| {
        (lib.game_manage_snapshot(id), lib.is_app_busy(cx, id))
    })
    .unwrap_or_default();
    let running = f.app.state.playing == Some(id);
    bp.manage.refresh_if_needed(id, installed.as_ref(), &f.app.local.libraries, op_busy);

    heading(ui, f, &mut y, &t!("Manage {game}", game = name));
    if let Some(g) = &installed {
        let subtitle = format!(
            "{}  ·  {}",
            ui::bytes(g.size_on_disk),
            g.install_dir.display()
        );
        paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, &subtitle, w::font(18.0 * s), f.t.text_dim, width);
        y += 38.0 * s;
    }

    let row_h = 58.0 * s;
    let row_step = 66.0 * s;

    // ---- General & Quick toggles ----
    heading(ui, f, &mut y, t!("General"));
    let fav_id = Id::unique(("bp-manage-fav", id));
    bp.preferred = Some(fav_id);
    if compact_row(
        bp,
        ui,
        f,
        fav_id,
        Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
        icons::STAR,
        t!("Favorite"),
        if snap.favorite { t!("On") } else { t!("Off") },
    )
    .activated
    {
        with_library(f.app, f.actions, |lib, cx| lib.toggle_favorite(cx, id));
        bp.library_cache_rev = bp.library_cache_rev.wrapping_add(1);
    }
    y += row_step;

    if compact_row(
        bp,
        ui,
        f,
        Id::unique(("bp-manage-hidden", id)),
        Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
        icons::EYE_SLASH,
        t!("Hide this game"),
        if snap.hidden { t!("Hidden") } else { t!("Visible") },
    )
    .activated
    {
        with_library(f.app, f.actions, |lib, cx| lib.toggle_hidden(cx, id));
        bp.library_cache_rev = bp.library_cache_rev.wrapping_add(1);
    }
    y += row_step;

    if compact_row(
        bp,
        ui,
        f,
        Id::unique(("bp-manage-overlay", id)),
        Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
        icons::STACK,
        t!("In-game overlay"),
        if snap.overlay_on { t!("On") } else { t!("Off") },
    )
    .activated
    {
        with_library(f.app, f.actions, |lib, _| lib.set_game_overlay(id, !snap.overlay_on));
    }
    y += row_step;

    if installed.is_some() && !f.app.demo {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let current_tool = client::app_proton(id).unwrap_or_else(|| t!("Automatic").into());
            if compact_row(
                bp,
                ui,
                f,
                Id::unique(("bp-manage-compat", id)),
                Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
                icons::WINDOWS,
                t!("Compatibility"),
                &current_tool,
            )
            .activated
            {
                bp.open(Page::Compatibility(id), f.now);
            }
            y += row_step;
        }
        let ctrl_label = match client::app_controller(id) {
            1 => t!("Enable Steam Input"),
            2 => t!("Disable Steam Input"),
            _ => t!("Use the default settings"),
        };
        if compact_row(
            bp,
            ui,
            f,
            Id::unique(("bp-manage-controller", id)),
            Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
            icons::CONTROLLER,
            t!("Steam Input"),
            ctrl_label,
        )
        .activated
        {
            bp.open(Page::Controller(id), f.now);
        }
        y += row_step;
    }

    // ---- Steam Cloud ----
    if let Some(cloud_cfg) = owned.as_ref().and_then(|o| o.cloud.as_ref()) {
        y += 14.0 * s;
        heading(ui, f, &mut y, t!("Steam Cloud"));
        if let Some(g) = &installed
            && !f.app.demo
            && f.app.state.online()
            && !bp.manage.cloud_checked
            && !f.app.state.cloud.contains_key(&id)
            && !f.app.state.cloud_syncing.contains(&id)
        {
            bp.manage.cloud_checked = true;
            f.app.client.send(Command::CloudSync {
                app_id: id,
                install_dir: g.install_dir.clone(),
                mode: client::cloud::Mode::Check,
                keep_local: vec![],
                keep_cloud: vec![],
                then_launch: None,
            });
        }
        let report = f.app.state.cloud.get(&id).cloned();
        let syncing = f.app.state.cloud_syncing.contains(&id);
        let cloud_on = !snap.cloud_off;
        if compact_row(
            bp,
            ui,
            f,
            Id::unique(("bp-manage-cloud-toggle", id)),
            Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
            icons::CLOUD_CHECK,
            t!("Keep saves in sync with Steam Cloud"),
            if cloud_on { t!("On") } else { t!("Off") },
        )
        .activated
        {
            with_library(f.app, f.actions, |lib, cx| lib.set_game_cloud(cx, id, !cloud_on));
        }
        y += row_step;

        let quota = report.as_ref().map(|r| r.quota_bytes).filter(|q| *q > 0).unwrap_or(cloud_cfg.quota);
        let used = report.as_ref().map(|r| ui::bytes(r.cloud_bytes)).unwrap_or_else(|| "—".into());
        let summary = if let Some(r) = &report {
            if !r.conflicts.is_empty() {
                t!("{n} files changed both here and in the cloud", n = r.conflicts.len())
            } else {
                format!("{used} / {} · {}", ui::bytes(quota), t!("{n} files in the cloud", n = r.cloud_files))
            }
        } else {
            format!("{used} / {}", ui::bytes(quota))
        };
        paint::line(
            ui.painter(),
            pos2(left + 4.0 * s, y),
            Align2::LEFT_TOP,
            &summary,
            w::font(18.0 * s),
            if report.as_ref().is_some_and(|r| !r.conflicts.is_empty()) { f.t.away } else { f.t.text_dim },
            width,
        );
        y += 34.0 * s;

        if let Some(g) = &installed {
            let cloud_locked = syncing || op_busy;
            let sync_label = if syncing {
                t!("Syncing…")
            } else if running {
                t!("Close the game first.")
            } else {
                t!("Sync now")
            };
            let (r, sync_hit) = button(
                bp,
                ui,
                f,
                Id::unique(("bp-manage-cloud-sync", id)),
                pos2(left, y),
                0.0,
                Some(icons::REFRESH),
                sync_label,
                if cloud_locked { Kind::Muted } else { Kind::Secondary },
                None,
            );
            if sync_hit.activated && !cloud_locked {
                f.app.client.send(Command::CloudSync {
                    app_id: id,
                    install_dir: g.install_dir.clone(),
                    mode: client::cloud::Mode::Sync,
                    keep_local: vec![],
                    keep_cloud: vec![],
                    then_launch: None,
                });
            }
            let mut bx = r.right() + 16.0 * s;
            if let Some(rep) = report.as_ref().filter(|r| !r.conflicts.is_empty()) {
                let names: Vec<String> = rep.conflicts.iter().map(|c| c.name.clone()).collect();
                let (r2, use_cloud) = button(
                    bp,
                    ui,
                    f,
                    Id::unique(("bp-manage-cloud-use-cloud", id)),
                    pos2(bx, y),
                    0.0,
                    Some(icons::DOWNLOAD),
                    t!("Use the cloud's saves"),
                    if cloud_locked { Kind::Muted } else { Kind::Primary },
                    None,
                );
                if use_cloud.activated && !cloud_locked {
                    f.app.client.send(Command::CloudSync {
                        app_id: id,
                        install_dir: g.install_dir.clone(),
                        mode: client::cloud::Mode::Sync,
                        keep_local: vec![],
                        keep_cloud: names.clone(),
                        then_launch: None,
                    });
                }
                bx = r2.right() + 16.0 * s;
                let (_, keep_local) = button(
                    bp,
                    ui,
                    f,
                    Id::unique(("bp-manage-cloud-keep-local", id)),
                    pos2(bx, y),
                    0.0,
                    Some(icons::DESKTOP),
                    t!("Keep this PC's saves"),
                    if cloud_locked { Kind::Muted } else { Kind::Secondary },
                    None,
                );
                if keep_local.activated && !cloud_locked {
                    f.app.client.send(Command::CloudSync {
                        app_id: id,
                        install_dir: g.install_dir.clone(),
                        mode: client::cloud::Mode::Sync,
                        keep_local: names,
                        keep_cloud: vec![],
                        then_launch: None,
                    });
                }
            }
            y += 88.0 * s;
        }
    }

    // ---- Updates & Betas ----
    y += 14.0 * s;
    heading(ui, f, &mut y, t!("Updates"));
    if compact_row(
        bp,
        ui,
        f,
        Id::unique(("bp-manage-auto-upd", id)),
        Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
        icons::DOWNLOAD,
        t!("Automatic updates"),
        auto_update_label(snap.auto_update),
    )
    .activated
    {
        let next = (snap.auto_update + 1) % 3;
        with_library(f.app, f.actions, |lib, _| lib.set_game_auto_update(id, next));
    }
    y += row_step;

    if compact_row(
        bp,
        ui,
        f,
        Id::unique(("bp-manage-bg-dl", id)),
        Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
        icons::CLOCK,
        t!("Background downloads"),
        background_dl_label(snap.background),
    )
    .activated
    {
        let next = (snap.background + 1) % 3;
        with_library(f.app, f.actions, |lib, _| lib.set_game_background_downloads(id, next));
    }
    y += row_step;

    // Public / passwordless betas.
    if let Some(o) = &owned {
        let public_branches: Vec<&model::Branch> = o.branches.iter().filter(|b| !b.password).collect();
        if public_branches.len() > 1 {
            y += 14.0 * s;
            heading(ui, f, &mut y, t!("Versions & betas"));
            let installed_branch = if bp.manage.installed_build.1.is_empty() {
                "public".to_string()
            } else {
                bp.manage.installed_build.1.clone()
            };
            let chosen = if snap.branch.is_empty() {
                installed_branch.clone()
            } else {
                snap.branch.clone()
            };
            for branch in &public_branches {
                let label = if branch.name == "public" {
                    t!("None (default version)").to_string()
                } else if !branch.description.is_empty() {
                    format!("{} · {}", branch.name, branch.description)
                } else {
                    branch.name.clone()
                };
                let selected = chosen == branch.name;
                let status = if selected && branch.name == installed_branch {
                    t!("Installed")
                } else if selected {
                    t!("Selected")
                } else {
                    ""
                };
                if compact_row(
                    bp,
                    ui,
                    f,
                    Id::unique(("bp-manage-beta", id, &branch.name)),
                    Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
                    if selected { icons::CHECK } else { icons::BRANCH },
                    &label,
                    status,
                )
                .activated
                {
                    let target = branch.name.clone();
                    with_library(f.app, f.actions, |lib, _| lib.set_game_branch(id, target));
                }
                y += row_step;
            }
            if let Some(g) = &installed
                && chosen != installed_branch
            {
                let (_, hit) = button(
                    bp,
                    ui,
                    f,
                    Id::unique(("bp-manage-beta-apply", id)),
                    pos2(left, y + 6.0 * s),
                    0.0,
                    Some(icons::DOWNLOAD),
                    t!("Switch now"),
                    Kind::Primary,
                    None,
                );
                if hit.activated {
                    let lib_path = g.library.clone();
                    with_library(f.app, f.actions, |lib, cx| lib.apply_game_changes(cx, id, lib_path));
                }
                y += 92.0 * s;
            }
        }
    }

    // ---- DLC ----
    let mut dlcs: Vec<model::OwnedApp> = f
        .app
        .state
        .apps
        .as_ref()
        .map(|a| a.values().filter(|d| d.is_dlc() && d.parent == Some(id)).cloned().collect())
        .unwrap_or_default();
    dlcs.sort_by(|a, b| model::cmp_ignore_case(&a.name, &b.name));
    if !dlcs.is_empty() {
        y += 14.0 * s;
        heading(ui, f, &mut y, &t!("DLC ({n})", n = dlcs.len()));
        for dlc in dlcs.iter().take(24) {
            let has_content = owned.as_ref().is_some_and(|o| o.depots.iter().any(|x| x.dlc_app == Some(dlc.app_id)));
            let is_installed = bp.manage.installed_depots.iter().any(|x| x.dlc_app == Some(dlc.app_id));
            let on = if installed.is_some() && has_content {
                (is_installed && !snap.disabled_dlc.contains(&dlc.app_id)) || snap.enabled_dlc.contains(&dlc.app_id)
            } else {
                !snap.disabled_dlc.contains(&dlc.app_id)
            };
            let size: u64 = owned
                .as_ref()
                .map(|o| {
                    o.depots
                        .iter()
                        .filter(|x| x.dlc_app == Some(dlc.app_id))
                        .filter_map(|x| x.manifest("public").map(|m| m.1))
                        .sum()
                })
                .unwrap_or(0);
            let status = if size > 0 {
                format!("{} · {}", ui::bytes(size), if on { t!("On") } else { t!("Off") })
            } else if on {
                t!("On").into()
            } else {
                t!("Off").into()
            };
            if compact_row(
                bp,
                ui,
                f,
                Id::unique(("bp-manage-dlc", id, dlc.app_id)),
                Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
                if on { icons::CHECK } else { icons::PACKAGE },
                &dlc.name,
                &status,
            )
            .activated
            {
                with_library(f.app, f.actions, |lib, _| lib.toggle_game_dlc(id, dlc.app_id, !on));
            }
            y += row_step;
        }
        if let Some(g) = &installed {
            let (_, hit) = button(
                bp,
                ui,
                f,
                Id::unique(("bp-manage-dlc-apply", id)),
                pos2(left, y + 6.0 * s),
                0.0,
                Some(icons::DOWNLOAD),
                t!("Apply DLC changes"),
                Kind::Primary,
                None,
            );
            if hit.activated {
                let lib_path = g.library.clone();
                with_library(f.app, f.actions, |lib, cx| lib.apply_game_changes(cx, id, lib_path));
            }
            y += 92.0 * s;
        }
    }

    // ---- Installed files: Verify, Move, Uninstall ----
    if let Some(g) = installed {
        y += 14.0 * s;
        heading(ui, f, &mut y, t!("Installed files"));

        if let Some((ok, total, healthy)) = snap.verify_report {
            let msg = t!("Last check: {ok} of {total} files OK", ok = ok, total = total);
            paint::line(
                ui.painter(),
                pos2(left + 4.0 * s, y),
                Align2::LEFT_TOP,
                &msg,
                w::font_semibold(18.0 * s),
                if healthy { f.t.play } else { f.t.away },
                width,
            );
            y += 32.0 * s;
        }

        if compact_row(
            bp,
            ui,
            f,
            Id::unique(("bp-manage-verify", id)),
            Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
            icons::SHIELD,
            t!("Verify integrity of game files"),
            if op_busy { t!("Busy") } else { "" },
        )
        .activated
            && !op_busy
        {
            with_library(f.app, f.actions, |lib, cx| lib.verify_game_quiet(cx, &g));
        }
        y += row_step;

        // Move between library folders when more than one library exists.
        // Same-filesystem moves are instant renames even with low free space, so
        // show free space as advisory and let `MoveGame` make the authoritative check.
        if !bp.manage.move_targets.is_empty() {
            let targets = bp.manage.move_targets.clone();
            for (dest, free) in targets {
                let free_str = free.map(|b| t!("{free} free", free = ui::bytes(b))).unwrap_or_else(|| "—".into());
                let label = t!("Move to {path}", path = dest.display());
                let status = if op_busy { t!("Busy").to_string() } else { free_str };
                if compact_row(
                    bp,
                    ui,
                    f,
                    Id::unique(("bp-manage-move", id, &dest)),
                    Rect::from_min_size(pos2(left, y), vec2(width, row_h)),
                    icons::DRIVES,
                    &label,
                    &status,
                )
                .activated
                    && !op_busy
                {
                    f.app.client.send(Command::MoveGame {
                        game: g.clone(),
                        dest_library: dest,
                    });
                    f.actions.push(extensions::Action::Toast(t!("Moving game…").into()));
                }
                y += row_step;
            }
        }

        // Uninstall with two-step confirmation (Cancel focused by default).
        y += 14.0 * s;
        if bp.manage.confirm_uninstall {
            let panel = Rect::from_min_size(pos2(left, y), vec2(width, 192.0 * s));
            ui.painter().rect_filled(panel, paint::radius(14.0 * s), f.t.danger.gamma_multiply(0.14));
            ui.painter().rect_stroke(
                panel,
                paint::radius(14.0 * s),
                egui::Stroke::new(1.5 * s, f.t.danger.gamma_multiply(0.6)),
                egui::StrokeKind::Inside,
            );
            let inner = panel.shrink(22.0 * s);
            ui.painter().text(
                inner.left_top(),
                Align2::LEFT_TOP,
                t!("Uninstall {name}?", name = name),
                w::font_bold(24.0 * s),
                f.t.text,
            );
            paint::line(
                ui.painter(),
                inner.left_top() + vec2(0.0, 34.0 * s),
                Align2::LEFT_TOP,
                &t!("This deletes {size} from:", size = ui::bytes(g.size_on_disk)),
                w::font(18.0 * s),
                f.t.text_dim,
                inner.width(),
            );
            paint::line(
                ui.painter(),
                inner.left_top() + vec2(0.0, 60.0 * s),
                Align2::LEFT_TOP,
                t!("Saves kept inside the game folder are deleted too. Steam Cloud saves aren't affected."),
                w::font(17.0 * s),
                f.t.text_faint,
                inner.width(),
            );
            let btn_y = inner.bottom() - 64.0 * s;
            let cancel_id = Id::unique(("bp-manage-uninst-cancel", id));
            let (r, cancel) = button(
                bp,
                ui,
                f,
                cancel_id,
                pos2(inner.left(), btn_y),
                200.0 * s,
                Some(icons::X),
                t!("Cancel"),
                Kind::Secondary,
                None,
            );
            let (_, confirm) = button(
                bp,
                ui,
                f,
                Id::unique(("bp-manage-uninst-go", id)),
                pos2(r.right() + 18.0 * s, btn_y),
                220.0 * s,
                Some(icons::TRASH),
                t!("Uninstall"),
                if op_busy { Kind::Muted } else { Kind::Danger },
                None,
            );
            if cancel.activated {
                bp.manage.confirm_uninstall = false;
            } else if confirm.activated && !op_busy {
                let ctx = ui.ctx().clone();
                let started = with_library(f.app, f.actions, |lib, cx| {
                    lib.start_uninstall_bg(cx, ctx, g.clone(), name.clone())
                })
                .unwrap_or(false);
                bp.manage.confirm_uninstall = false;
                if started {
                    bp.back(f.now);
                }
            }
            y = panel.bottom() + 20.0 * s;
        } else {
            let label = if running {
                t!("Close the game first.")
            } else if snap.uninstalling {
                t!("Uninstalling…")
            } else {
                t!("Uninstall")
            };
            let (_, hit) = button(
                bp,
                ui,
                f,
                Id::unique(("bp-manage-uninst", id)),
                pos2(left, y),
                260.0 * s,
                Some(icons::TRASH),
                label,
                if op_busy { Kind::Muted } else { Kind::Danger },
                None,
            );
            if hit.activated && !op_busy {
                bp.manage.confirm_uninstall = true;
                bp.focus.current = Some(Id::unique(("bp-manage-uninst-cancel", id)));
            }
            y += 90.0 * s;
        }
    } else {
        // Not installed: keep a valid focusable back/install hint if needed.
        let _ = bp.focus.item(ui, Id::unique(("bp-manage-spacer", id)), Rect::NOTHING, Rect::NOTHING, Place::Page);
    }

    y - top + 40.0 * s
}
