//! Big Picture's pages. Each lays itself out in screen coordinates from the
//! top of its scrolled content and returns how tall it is.

use super::focus::{Hit, Place, since};
use super::paint::{self, Kind};
use super::{BigPicture, ChatTarget, Frame, Page, Tab};
use crate::app::SteamApp;
use crate::builtin::library::{Library, MainAction};
use crate::builtin::store::Store;
use client::Command;
use egui::{Align2, Color32, Id, Rect, Ui, Vec2, pos2, vec2};
use extensions::Action;
use i18n::t;
use model::{AppId, PersonaState, art};
use ui::{icons, widgets as w};

/// Run `f` with the Library extension (its play/install logic and data).
pub fn with_library<R>(app: &mut SteamApp, actions: &mut Vec<Action>, f: impl FnOnce(&mut Library, &mut extensions::Ctx) -> R) -> Option<R> {
    let env = extensions::Env { state: &app.state, local: &app.local, theme: &app.prefs.theme, zoom: app.prefs.zoom, client: &app.client, actions };
    app.registry.with_ext::<Library, R>("library", env, f)
}

/// Run `f` with the Store extension (its data and caches).
pub fn with_store<R>(app: &mut SteamApp, actions: &mut Vec<Action>, f: impl FnOnce(&mut Store, &mut extensions::Ctx) -> R) -> Option<R> {
    let env = extensions::Env { state: &app.state, local: &app.local, theme: &app.prefs.theme, zoom: app.prefs.zoom, client: &app.client, actions };
    app.registry.with_ext::<Store, R>("store", env, f)
}

pub fn draw(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, pos: f32) -> f32 {
    let top = f.view.top() - pos;
    // Leaving a trailer stops it.
    if bp.video_on && !matches!(bp.page(), Page::Video(..)) {
        bp.video_on = false;
        with_store(f.app, f.actions, |st, _| st.stop_trailer());
    }
    match bp.page() {
        Page::Tab(Tab::Home) => home(bp, ui, f, top),
        Page::Tab(Tab::Library) => library_tab(bp, ui, f, top),
        Page::Tab(Tab::Store) => store_tab(bp, ui, f, top),
        Page::Tab(Tab::Downloads) => downloads(bp, ui, f, top),
        Page::Tab(Tab::Friends) => friends(bp, ui, f, top),
        Page::Game(id) => game(bp, ui, f, top, id),
        Page::GameManage(id) => super::manage::page(bp, ui, f, top, id),
        Page::Controller(id) => controller_page(bp, ui, f, top, id),
        Page::Notifications => notifications(bp, ui, f, top),
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        Page::Compatibility(id) => compatibility_page(bp, ui, f, top, id),
        Page::StoreApp(id) => store_app(bp, ui, f, top, id),
        Page::Review(id, review) => review_page(ui, f, top, id, &review),
        Page::News(id, index) => news_page(bp, ui, f, top, id, index),
        Page::Search => search(bp, ui, f, top),
        Page::Shots(id, i) => shots(bp, ui, f, id, i),
        Page::Settings => super::settings::page(bp, ui, f),
        Page::Friend(id) => friend_page(bp, ui, f, top, id),
        Page::Profile(id) => super::profile::profile_page(bp, ui, f, top, id),
        Page::ProfileGames(id) => super::profile::games_page(bp, ui, f, top, id),
        Page::ProfileBadges(id) => super::profile::badges_page(bp, ui, f, top, id),
        Page::ProfileCards(id, app, foil) => super::profile::cards_page(bp, ui, f, top, id, app, foil),
        Page::ProfileReviews(id) => super::profile::reviews_page(bp, ui, f, top, id),
        Page::ProfileGroups(id) => super::profile::groups_page(bp, ui, f, top, id),
        Page::ProfileEdit => super::profile::edit_page(bp, ui, f),
        Page::Group(id) => group_page(bp, ui, f, top, id),
        Page::Chat(id) => chat_page(bp, ui, f, top, id),
        Page::Embedded(page, arg) => embedded(bp, ui, f, &page, arg.as_deref()),
        Page::Video(id, i) => video_page(bp, ui, f, id, i),
        Page::Install(id) => install_page(bp, ui, f, top, id),
    }
}

/// What X does on this page, for the hints.
pub fn x_hint(bp: &BigPicture) -> Option<String> {
    match bp.page() {
        Page::Tab(Tab::Library) if bp.library_search_open => Some(t!("Delete").into()),
        Page::Tab(Tab::Library) => Some(t!("Change filter").into()),
        Page::Game(_) => Some(t!("Favorite").into()),
        Page::Install(_) => Some(t!("Desktop shortcut").into()),
        Page::Profile(_) if bp.profile.comment_open => Some(t!("Delete").into()),
        Page::ProfileGames(_) if bp.profile.games_search_open => Some(t!("Delete").into()),
        Page::ProfileEdit if bp.profile.edit_typing() => Some(t!("Delete").into()),
        Page::Search | Page::Chat(_) => Some(t!("Delete").into()),
        _ => None,
    }
}

/// What Y does on this page, for the hints.
pub fn y_hint(bp: &BigPicture) -> Option<String> {
    match bp.page() {
        Page::Tab(Tab::Library) if bp.library_search_open => Some(t!("Space").into()),
        Page::Tab(Tab::Library) => Some(t!("Sort").into()),
        Page::Game(_) => Some(t!("Manage").into()),
        Page::Profile(_) if bp.profile.comment_open => Some(t!("Space").into()),
        Page::ProfileGames(_) if bp.profile.games_search_open => Some(t!("Space").into()),
        Page::ProfileGames(_) => Some(t!("Sort").into()),
        Page::ProfileEdit if bp.profile.edit_typing() => Some(t!("Space").into()),
        Page::Search | Page::Chat(_) => Some(t!("Space").into()),
        _ => None,
    }
}

// ---------- shared pieces ----------

/// Fade and rise in, one after another, when a page opens.
fn appear(ui: &Ui, bp: &BigPicture, order: usize) -> (f32, f32) {
    let k = since(ui, bp.page_since + 0.045 * order.min(10) as f64, 0.4);
    (k, (1.0 - k) * 26.0)
}

pub(super) fn heading(ui: &Ui, f: &Frame, y: &mut f32, title: &str) {
    ui.painter().text(pos2(f.view.left() + f.pad, *y), Align2::LEFT_TOP, title, w::font_bold(30.0 * f.s), f.t.text);
    *y += 54.0 * f.s;
}

/// A shelf of `n` tiles that scrolls sideways to follow the focus. Returns
/// the tiles near the screen: (index, rect, hit).
fn shelf(bp: &mut BigPicture, ui: &mut Ui, f: &Frame, y: &mut f32, key: &str, n: usize, tile: Vec2, id: impl Fn(usize) -> Id) -> Vec<(usize, Rect, Hit)> {
    let gap = 26.0 * f.s;
    let width = f.view.width() - 2.0 * f.pad;
    let pos = bp.scroller(key).pos;
    let clip = ui.clip_rect();
    let mut out = Vec::new();
    for i in 0..n {
        let at = i as f32 * (tile.x + gap);
        let r = Rect::from_min_size(pos2(f.view.left() + f.pad + at - pos, *y), tile);
        let hit = bp.focus.item(ui, id(i), r, clip, Place::Page);
        if hit.focused && bp.focus.moved {
            bp.scroller(key).reveal(at, at + tile.x, width, tile.x * 0.6 + gap);
        }
        if r.right() > clip.left() - tile.x && r.left() < clip.right() + tile.x {
            out.push((i, r, hit));
        }
    }
    let sc = bp.scroller(key);
    // A sideways wheel or Shift+wheel over the row scrolls it (the focus
    // only follows the D-pad and keys).
    let row = Rect::from_min_max(pos2(f.view.left(), *y), pos2(f.view.right(), *y + tile.y));
    if ui.rect_contains_pointer(row) {
        let dx = ui.input(|i| i.smooth_scroll_delta.x);
        if dx != 0.0 {
            sc.target -= dx;
        }
    }
    sc.clamp(n as f32 * (tile.x + gap) - gap, width);
    sc.step(ui);
    *y += tile.y;
    out
}

/// A game's portrait art tile (with a caption under it) and what A does.
fn capsule(bp: &mut BigPicture, ui: &mut Ui, f: &Frame, r: Rect, hit: &Hit, order: usize, id: AppId, name: &str, caption: &str, dim: bool) {
    let (k, dy) = appear(ui, bp, order);
    let r = r.translate(vec2(0.0, dy * f.s));
    let mut c = paint::faded(ui, r.expand(40.0 * f.s), k);
    let sheen = (f.now - bp.focus.changed_at) as f32 / 0.7;
    let shown = paint::art_tile(&mut c, &f.t, r, &[&art::capsule(id), &art::header(id)], name, hit, f.s, sheen);
    if dim {
        c.painter().rect_filled(shown, paint::radius(10.0 * f.s), Color32::from_black_alpha((110.0 * (1.0 - hit.glow)) as u8));
    }
    if hit.focused {
        bp.focus.ring_around(shown, 10.0 * f.s);
        bp.bg_want = Some(id);
    }
    let text_y = r.bottom() + 12.0 * f.s + 6.0 * f.s * hit.glow;
    let color = w::mix(f.t.text_dim, f.t.text, hit.glow);
    paint::line(c.painter(), pos2(r.left(), text_y), Align2::LEFT_TOP, name, w::font_semibold(19.0 * f.s), color, r.width());
    if !caption.is_empty() {
        paint::line(c.painter(), pos2(r.left(), text_y + 26.0 * f.s), Align2::LEFT_TOP, caption, w::font(16.0 * f.s), f.t.text_faint, r.width());
    }
}

/// A wide picture tile (store capsules, news) with up to two lines under it.
#[allow(clippy::too_many_arguments)]
fn wide(bp: &mut BigPicture, ui: &mut Ui, f: &Frame, r: Rect, hit: &Hit, order: usize, urls: &[&str], title: &str, sub: &str, sub_color: Color32) -> Rect {
    let (k, dy) = appear(ui, bp, order);
    let r = r.translate(vec2(0.0, dy * f.s));
    let mut c = paint::faded(ui, r.expand(40.0 * f.s), k);
    let sheen = (f.now - bp.focus.changed_at) as f32 / 0.7;
    let shown = paint::art_tile(&mut c, &f.t, r, urls, title, hit, f.s, sheen);
    if hit.focused {
        bp.focus.ring_around(shown, 10.0 * f.s);
    }
    let text_y = r.bottom() + 12.0 * f.s + 6.0 * f.s * hit.glow;
    let color = w::mix(f.t.text_dim, f.t.text, hit.glow);
    paint::line(c.painter(), pos2(r.left(), text_y), Align2::LEFT_TOP, title, w::font_semibold(19.0 * f.s), color, r.width());
    if !sub.is_empty() {
        paint::line(c.painter(), pos2(r.left(), text_y + 26.0 * f.s), Align2::LEFT_TOP, sub, w::font_medium(17.0 * f.s), sub_color, r.width());
    }
    shown
}

/// A pill button that's focusable. Returns its hit.
#[allow(clippy::too_many_arguments)]
pub(super) fn button(bp: &mut BigPicture, ui: &Ui, f: &Frame, id: Id, at: egui::Pos2, min_w: f32, icon: Option<&str>, label: &str, kind: Kind, progress: Option<f32>) -> (Rect, Hit) {
    let h = 72.0 * f.s;
    let wdt = paint::pill_width(ui, icon.is_some(), label, f.s).max(min_w);
    let r = Rect::from_min_size(at, vec2(wdt, h));
    let hit = bp.focus.item(ui, id, r, ui.clip_rect(), Place::Page);
    let shown = paint::pill(ui, &f.t, r, icon, label, kind, &hit, f.s, progress);
    if hit.focused {
        bp.focus.ring_around(shown, shown.height() / 2.0);
    }
    (r, hit)
}

/// Compact settings row with a clipped label and a separate status column.
pub(super) fn compact_row(bp: &mut BigPicture, ui: &Ui, f: &Frame, id: Id, rect: Rect, icon: &str, label: &str, value: &str) -> Hit {
    let s = f.s;
    let hit = bp.focus.item(ui, id, rect, ui.clip_rect(), Place::Page);
    let p = ui.painter();
    p.rect_filled(rect, paint::radius(10.0 * s), w::mix(f.t.surface, f.t.surface_hover, hit.glow));
    let text_x = rect.left() + 49.0 * s;
    let value_width = if value.is_empty() { 0.0 } else { rect.width() * 0.30 };
    p.text(pos2(rect.left() + 23.0 * s, rect.center().y), Align2::CENTER_CENTER, icon, w::font(21.0 * s), f.t.accent);
    paint::line(p, pos2(text_x, rect.center().y), Align2::LEFT_CENTER, label, w::font_semibold(21.0 * s),
        f.t.text, rect.right() - text_x - value_width - 16.0 * s);
    if !value.is_empty() {
        paint::line(p, pos2(rect.right() - 16.0 * s, rect.center().y), Align2::RIGHT_CENTER, value, w::font_medium(18.0 * s),
            f.t.text_dim, value_width);
    }
    if hit.focused {
        bp.focus.ring_around(rect, 10.0 * s);
    }
    hit
}

/// A big empty-state message with one way forward.
pub(super) fn empty(bp: &mut BigPicture, ui: &Ui, f: &Frame, y: f32, icon: &str, title: &str, body: &str, action: Option<(&str, &str)>) -> Option<bool> {
    let c = pos2(f.view.center().x, y + 150.0 * f.s);
    let p = ui.painter();
    p.text(c, Align2::CENTER_CENTER, icon, w::font(96.0 * f.s), f.t.text_faint);
    p.text(c + vec2(0.0, 90.0 * f.s), Align2::CENTER_CENTER, title, w::font_bold(34.0 * f.s), f.t.text);
    p.text(c + vec2(0.0, 136.0 * f.s), Align2::CENTER_CENTER, body, w::font(21.0 * f.s), f.t.text_dim);
    let (icon2, label) = action?;
    let wdt = paint::pill_width(ui, true, label, f.s);
    let (_, hit) = button(bp, ui, f, Id::unique(("bp-empty", title)), c + vec2(-wdt / 2.0, 186.0 * f.s), wdt, Some(icon2), label, Kind::Primary, None);
    Some(hit.activated)
}

fn open_game(bp: &mut BigPicture, f: &Frame, id: AppId) {
    let owned = f.app.state.apps.as_ref().is_some_and(|a| a.contains_key(&id)) || f.app.local.games.iter().any(|g| g.app_id == id);
    bp.open(if owned { Page::Game(id) } else { Page::StoreApp(id) }, f.now);
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The main button for a game, as label, icon, look and progress.
fn main_button(a: MainAction) -> (String, &'static str, Kind, Option<f32>) {
    match a {
        MainAction::Play => (t!("Play").into(), icons::PLAY, Kind::Play, None),
        MainAction::Running => (t!("Stop").into(), icons::STOP, Kind::Danger, None),
        MainAction::Update => (t!("Update").into(), icons::DOWNLOAD, Kind::Primary, None),
        MainAction::Install => (t!("Install").into(), icons::DOWNLOAD, Kind::Primary, None),
        MainAction::Moving(progress) => (t!("Moving… {pct}%", pct = (progress * 100.0).round() as u32), icons::CLOCK, Kind::Muted, Some(progress)),
        MainAction::BackingUp(progress) => (t!("Backing up… {pct}%", pct = (progress * 100.0).round() as u32), icons::CLOCK, Kind::Muted, Some(progress)),
        MainAction::Preparing => (t!("Preparing to launch…").into(), icons::CLOCK, Kind::Muted, None),
        MainAction::Syncing => (t!("Syncing Steam Cloud…").into(), icons::CLOUD_CHECK, Kind::Muted, None),
        MainAction::Queued => (t!("Queued").into(), icons::CLOCK, Kind::Muted, None),
        MainAction::Uninstalling => (t!("Uninstalling…").into(), icons::TRASH, Kind::Muted, None),
        MainAction::Busy { fraction, verifying, paused } => {
            let pct = (fraction * 100.0) as u32;
            // Says what A does: resume or pause (verifying can only be cancelled).
            let label = if paused {
                t!("Resume · {pct}%", pct = pct)
            } else if verifying {
                t!("Verifying… {pct}%", pct = pct)
            } else {
                t!("Pause · {pct}%", pct = pct)
            };
            (label, if paused { icons::PLAY } else { icons::DOWNLOAD }, Kind::Secondary, Some(fraction))
        }
        MainAction::StorePage => (t!("Store page").into(), icons::STORE, Kind::Primary, None),
    }
}

/// Recently played games merged across account history and local installs, newest first.
fn recent_games(app: &SteamApp) -> Vec<(AppId, String, u64)> {
    let mut by_id: std::collections::HashMap<AppId, (String, u64)> = std::collections::HashMap::new();
    if let Some(Ok(list)) = app.state.player_games.get(&app.state.steam_id) {
        for g in list {
            if let Some(when) = g.last_played {
                by_id.insert(g.app_id, (g.name.clone(), when));
            }
        }
    }
    for g in &app.local.games {
        if let Some(when) = g.last_played {
            by_id
                .entry(g.app_id)
                .and_modify(|e| {
                    if when > e.1 {
                        e.1 = when;
                    }
                    if e.0.is_empty() {
                        e.0 = g.name.clone();
                    }
                })
                .or_insert_with(|| (g.name.clone(), when));
        }
    }
    let mut list: Vec<(AppId, String, u64)> = by_id.into_iter().map(|(id, (n, w))| (id, n, w)).collect();
    list.sort_by_key(|g| std::cmp::Reverse(g.2));
    list
}

// ---------- Home ----------

fn home(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32) -> f32 {
    let s = f.s;
    let mut y = top + 16.0 * s;
    let recent = recent_games(f.app);
    // Prioritize a currently running game as hero, then most recently played installed game.
    let playing = f.app.state.playing;
    let hero = playing
        .and_then(|pid| {
            let inst = f.app.local.games.iter().find(|g| g.app_id == pid);
            let name = f
                .app
                .state
                .app_name(pid)
                .map(String::from)
                .or_else(|| inst.map(|g| g.name.clone()))
                .unwrap_or_default();
            Some((pid, name, inst.and_then(|g| g.playtime_minutes), inst.and_then(|g| g.last_played), true))
        })
        .or_else(|| {
            f.app
                .local
                .games
                .iter()
                .max_by_key(|g| g.last_played.unwrap_or(0))
                .map(|g| (g.app_id, g.name.clone(), g.playtime_minutes, g.last_played, false))
        });
    match hero.clone() {
        Some((id, name, minutes, last, is_running)) => {
            bp.bg_want = Some(id);
            let rect = Rect::from_min_size(pos2(f.view.left() + f.pad, y), vec2(f.view.width() - 2.0 * f.pad, 430.0 * s));
            let (k, dy) = appear(ui, bp, 0);
            let rect = rect.translate(vec2(0.0, dy * s));
            let c = paint::faded(ui, rect.expand(60.0 * s), k);
            let r = 18.0 * s;
            let radius = paint::radius(r);
            paint::shadow(c.painter(), rect, r, s, 0.8, Color32::from_black_alpha(220));
            if !w::paint_cover_first(&c, rect, &[&art::hero(id), &art::header(id)], radius) {
                paint::skeleton(&c, &f.t, rect, radius);
            }
            w::fade_right_rounded(&c, rect, Color32::from_black_alpha(220), 0.7, radius);
            w::fade_down_rounded(&c, rect, Color32::from_black_alpha(170), 0.45, radius);
            let inner = rect.shrink(40.0 * s);
            let p = c.painter().clone();
            let kicker = if is_running { t!("NOW PLAYING") } else { t!("CONTINUE PLAYING") };
            let kicker_col = if is_running { f.t.play } else { f.t.accent };
            p.text(inner.left_top(), Align2::LEFT_TOP, kicker, w::font_bold(18.0 * s), kicker_col);
            let logo = Rect::from_min_max(inner.left_top() + vec2(0.0, 40.0 * s), pos2(inner.left() + 560.0 * s, inner.bottom() - 150.0 * s));
            if !w::paint_contain(&c, logo, &art::logo(id), Align2::LEFT_BOTTOM) {
                paint::wrapped(&p, pos2(logo.left(), logo.bottom() - 120.0 * s), &name, w::font_bold(50.0 * s), Color32::WHITE, 740.0 * s, 2);
            }
            let mut facts = Vec::new();
            if let Some(m) = minutes.filter(|m| *m > 0) {
                facts.push(t!("{time} played", time = ui::playtime(m)));
            }
            if let Some(l) = last {
                facts.push(t!("Last played {when}", when = ui::ago(l)));
            }
            p.text(pos2(inner.left(), inner.bottom() - 102.0 * s), Align2::LEFT_BOTTOM, facts.join("   ·   "), w::font_medium(20.0 * s), Color32::from_white_alpha(225));
            let action = with_library(f.app, f.actions, |lib, cx| lib.main_action(cx, id)).unwrap_or(MainAction::Play);
            let (label, icon, kind, progress) = main_button(action);
            let main_id = Id::unique(("bp-home-main", id));
            let (b, main) = button(bp, &c, f, main_id, pos2(inner.left(), inner.bottom() - 72.0 * s), 250.0 * s, Some(icon), &label, kind, progress);
            let (_, details) = button(bp, &c, f, Id::unique(("bp-home-details", id)), pos2(b.right() + 18.0 * s, b.top()), 0.0, Some(icons::INFO), t!("Game details"), Kind::Secondary, None);
            bp.preferred = Some(main_id);
            if main.activated {
                if action == MainAction::StorePage {
                    bp.open(Page::StoreApp(id), f.now);
                } else {
                    with_library(f.app, f.actions, |lib, cx| lib.run_main_action(cx, id));
                }
            }
            if details.activated {
                bp.open(Page::Game(id), f.now);
            }
            y += 430.0 * s + 52.0 * s;
        }
        None => {
            if let Some(go) = empty(bp, ui, f, y, icons::BOOKS, t!("Nothing installed yet"), t!("Install a game from your library to see it here."), Some((icons::BOOKS, t!("Go to your library")))) {
                if go {
                    bp.switch_tab(Tab::Library, f.now, true);
                }
            }
            y += 480.0 * s;
        }
    }

    // Recently played (semantic AppId-based focus IDs).
    let first = hero.as_ref().map(|h| h.0);
    let shelf_games: Vec<(AppId, String, u64)> = recent.iter().filter(|g| Some(g.0) != first).take(20).cloned().collect();
    if !shelf_games.is_empty() {
        heading(ui, f, &mut y, t!("Recently played"));
        let tile = vec2(200.0 * s, 300.0 * s);
        let tiles = shelf(bp, ui, f, &mut y, "recent", shelf_games.len(), tile, |i| Id::unique(("bp-recent", shelf_games[i].0)));
        for (i, r, hit) in tiles {
            let (id, name, when) = &shelf_games[i];
            let dim = !f.app.local.games.iter().any(|g| g.app_id == *id);
            capsule(bp, ui, f, r, &hit, i + 1, *id, name, &ui::ago(*when), dim);
            if hit.activated {
                open_game(bp, f, *id);
            }
        }
        y += 90.0 * s;
    }

    if f.app.shared_flags().lock().unwrap().show_whats_new {
        whats_new(bp, ui, f, &mut y, first, &recent);
    }
    y - top + 40.0 * s
}

/// What's new in the games you play (cards open the inline article reader).
fn whats_new(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, y: &mut f32, first: Option<AppId>, recent: &[(AppId, String, u64)]) {
    let s = f.s;
    let mut ids: Vec<AppId> = first.into_iter().collect();
    ids.extend(recent.iter().map(|g| g.0).filter(|id| Some(*id) != first));
    let (posts, loading) = with_library(f.app, f.actions, |lib, _| lib.whats_new_posts(&ids)).unwrap_or_default();
    if posts.is_empty() && (!loading || ids.is_empty()) {
        return;
    }
    heading(ui, f, y, t!("What's new"));
    let tile = vec2(440.0 * s, 206.0 * s);
    if posts.is_empty() {
        for i in 0..4 {
            let r = Rect::from_min_size(pos2(f.view.left() + f.pad + i as f32 * (tile.x + 26.0 * s), *y), tile);
            paint::skeleton(ui, &f.t, r, paint::radius(10.0 * s));
        }
        *y += tile.y;
    } else {
        let posts: Vec<_> = posts.into_iter().take(12).collect();
        let tiles = shelf(bp, ui, f, y, "news", posts.len(), tile, |i| Id::unique(("bp-news", posts[i].0, posts[i].1)));
        for (i, r, hit) in tiles {
            let (id, idx, e) = &posts[i];
            let game = f.app.state.app_name(*id).unwrap_or_default().to_string();
            let header = art::header(*id);
            let urls: Vec<&str> = e.capsule.as_deref().into_iter().chain(e.header.as_deref()).chain(std::iter::once(header.as_str())).collect();
            let age = now_unix().saturating_sub(e.time) / 86_400;
            let when = match age {
                0 => t!("Today").to_string(),
                1 => t!("Yesterday").to_string(),
                _ => ui::date(e.time),
            };
            wide(bp, ui, f, r, &hit, i + 2, &urls, &e.title, &format!("{game} · {when}"), f.t.text_faint);
            if hit.focused {
                bp.bg_want = Some(*id);
            }
            if hit.activated {
                bp.open(Page::News(*id, *idx), f.now);
            }
        }
    }
    *y += 90.0 * s;
}

/// Inline What's New article reader (scrolls with up/down or right stick).
fn news_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: AppId, index: usize) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let width = (f.view.width() - 2.0 * f.pad).min(1100.0 * s);
    let mut y = top + 20.0 * s;
    bp.bg_want = Some(id);

    let (posts, _) = with_library(f.app, f.actions, |lib, _| lib.whats_new_posts(&[id])).unwrap_or_default();
    let Some((_, _, event)) = posts.into_iter().find(|(app, idx, _)| *app == id && *idx == index) else {
        empty(bp, ui, f, y, icons::NOTE, t!("Article unavailable"), "", None);
        return 420.0 * s;
    };

    let game_name = f.app.state.app_name(id).unwrap_or_default().to_string();
    let (r, open_hit) = button(bp, ui, f, Id::unique(("bp-news-open-game", id)), pos2(left, y), 0.0, Some(icons::CONTROLLER), &game_name, Kind::Secondary, None);
    bp.preferred = Some(Id::unique(("bp-news-open-game", id)));
    if open_hit.activated {
        open_game(bp, f, id);
    }
    let _ = r;
    y += 92.0 * s;

    let p = ui.painter().clone();
    let title_r = paint::wrapped(&p, pos2(left, y), &event.title, w::font_bold(38.0 * s), f.t.text, width, 3);
    y = title_r.bottom() + 10.0 * s;
    p.text(pos2(left, y), Align2::LEFT_TOP, ui::date(event.time), w::font_medium(19.0 * s), f.t.text_dim);
    y += 38.0 * s;

    if let Some(url) = event.header.as_deref().or(event.capsule.as_deref()) {
        let img_h = (width * 0.42).min(360.0 * s);
        let img_r = Rect::from_min_size(pos2(left, y), vec2(width, img_h));
        if w::paint_cover(ui, img_r, url, paint::radius(14.0 * s)) {
            y = img_r.bottom() + 26.0 * s;
        }
    }

    if event.body.is_empty() && !event.summary.is_empty() {
        let r = paint::wrapped(&p, pos2(left, y), &event.summary, w::font(22.0 * s), f.t.text, width, usize::MAX);
        y = r.bottom() + 20.0 * s;
    } else {
        for block in &event.body {
            match block {
                store::html::Block::Heading(h) => {
                    y += 10.0 * s;
                    let r = paint::wrapped(&p, pos2(left, y), h, w::font_bold(27.0 * s), f.t.text, width, 4);
                    y = r.bottom() + 14.0 * s;
                }
                store::html::Block::Paragraph(runs) => {
                    let text: String = runs.iter().map(|(t, _)| t.as_str()).collect();
                    if !text.trim().is_empty() {
                        let r = paint::wrapped(&p, pos2(left, y), text.trim(), w::font(21.0 * s), f.t.text_dim, width, usize::MAX);
                        y = r.bottom() + 16.0 * s;
                    }
                }
                store::html::Block::Bullet(runs) => {
                    let text: String = runs.iter().map(|(t, _)| t.as_str()).collect();
                    if !text.trim().is_empty() {
                        let r = paint::wrapped(&p, pos2(left + 20.0 * s, y), &format!("•  {}", text.trim()), w::font(20.0 * s), f.t.text_dim, width - 20.0 * s, usize::MAX);
                        y = r.bottom() + 10.0 * s;
                    }
                }
                store::html::Block::Image(_) | store::html::Block::Video(_) => {}
            }
        }
    }
    y - top + 40.0 * s
}

// ---------- Library ----------

fn runs_here(app: &model::OwnedApp) -> bool {
    let mut launch = app.launch.iter().filter(|option| !option.executable.is_empty()).peekable();
    if launch.peek().is_some() {
        return launch.any(|option| {
            option.runs_on_this_os()
                || cfg!(any(target_os = "linux", target_os = "freebsd")) && option.os_list.split(',').any(|os| os.trim() == "windows")
        });
    }
    let depots = app.depots.iter().filter(|depot| depot.dlc_app.is_none() && !depot.manifests.is_empty());
    let mut depots = depots.peekable();
    depots.peek().is_none() || depots.any(|depot| {
        depot.os.is_empty() || depot.os.split(',').any(|os| {
            let os = os.trim();
            os == std::env::consts::OS || cfg!(any(target_os = "linux", target_os = "freebsd")) && os == "windows" || cfg!(target_os = "freebsd") && os == "linux"
        })
    })
}

fn library_fingerprint(bp: &BigPicture, f: &mut Frame) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bp.library_cache_rev.hash(&mut h);
    (bp.library_sort as u8).hash(&mut h);
    if let Some(apps) = &f.app.state.apps {
        apps.len().hash(&mut h);
        let mut id_sum = 0u64;
        for id in apps.keys() {
            id_sum = id_sum.wrapping_add(u64::from(*id));
        }
        id_sum.hash(&mut h);
    } else {
        0usize.hash(&mut h);
    }
    f.app.local.games.len().hash(&mut h);
    for g in &f.app.local.games {
        g.app_id.hash(&mut h);
        g.size_on_disk.hash(&mut h);
        g.last_played.hash(&mut h);
        g.playtime_minutes.hash(&mut h);
    }
    if let Some(Ok(pg)) = f.app.state.player_games.get(&f.app.state.steam_id) {
        pg.len().hash(&mut h);
        let (mut play_sum, mut last_sum) = (0u64, 0u64);
        for g in pg {
            play_sum = play_sum.wrapping_add(g.playtime_minutes);
            last_sum = last_sum.wrapping_add(g.last_played.unwrap_or(0));
        }
        play_sum.hash(&mut h);
        last_sum.hash(&mut h);
    }
    let (fav_hash, hid_hash, col_hash) = with_library(f.app, f.actions, |lib, _| {
        let mut fh = std::collections::hash_map::DefaultHasher::new();
        for id in lib.favorites_set() {
            id.hash(&mut fh);
        }
        let mut hh = std::collections::hash_map::DefaultHasher::new();
        for id in lib.hidden_set() {
            id.hash(&mut hh);
        }
        let mut ch = std::collections::hash_map::DefaultHasher::new();
        for (name, set) in lib.collections_summary() {
            name.hash(&mut ch);
            set.len().hash(&mut ch);
            for id in set {
                id.hash(&mut ch);
            }
        }
        (fh.finish(), hh.finish(), ch.finish())
    })
    .unwrap_or_default();
    fav_hash.hash(&mut h);
    hid_hash.hash(&mut h);
    col_hash.hash(&mut h);
    h.finish()
}

fn ensure_library_cache(bp: &mut BigPicture, f: &mut Frame) {
    let fingerprint = library_fingerprint(bp, f);
    if bp.library_cache.as_ref().is_some_and(|c| c.fingerprint == fingerprint) {
        return;
    }
    let (favorites, hidden, collections) = with_library(f.app, f.actions, |lib, _| {
        (lib.favorites_set(), lib.hidden_set(), lib.collections_summary())
    })
    .unwrap_or_default();
    let entries = crate::builtin::entries(&f.app.state, &f.app.local);
    let playtime_map: std::collections::HashMap<AppId, (u64, u64)> = match f.app.state.player_games.get(&f.app.state.steam_id) {
        Some(Ok(list)) => list.iter().map(|g| (g.app_id, (g.playtime_minutes, g.last_played.unwrap_or(0)))).collect(),
        _ => std::collections::HashMap::new(),
    };
    let mut all_games: Vec<super::LibraryEntry> = entries
        .iter()
        .filter(|e| !hidden.contains(&e.app_id))
        .map(|e| {
            let runs = e.installed.is_some() || e.owned.is_none_or(|o| runs_here(o));
            let (acct_min, acct_last) = playtime_map.get(&e.app_id).copied().unwrap_or((0, 0));
            let inst_min = e.installed.and_then(|g| g.playtime_minutes).unwrap_or(0);
            let inst_last = e.installed.and_then(|g| g.last_played).unwrap_or(0);
            super::LibraryEntry {
                id: e.app_id,
                name: e.name.to_string(),
                installed: e.installed.is_some(),
                runs,
                favorite: favorites.contains(&e.app_id),
                last_played: inst_last.max(acct_last),
                playtime_minutes: inst_min.max(acct_min),
                size_on_disk: e.installed.map_or(0, |g| g.size_on_disk),
            }
        })
        .collect();
    match bp.library_sort {
        super::LibrarySort::Title => all_games.sort_by(|a, b| model::cmp_ignore_case(&a.name, &b.name)),
        super::LibrarySort::Recent => all_games.sort_by(|a, b| {
            b.last_played
                .cmp(&a.last_played)
                .then_with(|| model::cmp_ignore_case(&a.name, &b.name))
        }),
        super::LibrarySort::Playtime => all_games.sort_by(|a, b| {
            b.playtime_minutes
                .cmp(&a.playtime_minutes)
                .then_with(|| model::cmp_ignore_case(&a.name, &b.name))
        }),
        super::LibrarySort::Size => all_games.sort_by(|a, b| {
            b.size_on_disk
                .cmp(&a.size_on_disk)
                .then_with(|| model::cmp_ignore_case(&a.name, &b.name))
        }),
    }
    let n_installed = all_games.iter().filter(|g| g.installed).count();
    let n_favorites = all_games.iter().filter(|g| g.favorite).count();
    let n_runs = all_games.iter().filter(|g| g.runs).count();
    bp.library_cache = Some(super::LibraryCache {
        fingerprint,
        all_games,
        collections,
        n_installed,
        n_favorites,
        n_runs,
    });
}

fn library_tab(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32) -> f32 {
    use super::LibraryFilter;
    let s = f.s;
    let left = f.view.left() + f.pad;
    let right = f.view.right() - f.pad;
    let mut y = top + 16.0 * s;

    // Reset filter from preference when Settings → Library changed it.
    if bp.installed_only.is_none() || bp.library_filter.is_none() {
        let pref = f.app.prefs.bigpicture_library;
        bp.installed_only = Some(pref == 0);
        bp.this_os_only = pref == 1;
        bp.library_filter = Some(match pref {
            0 => LibraryFilter::Installed,
            1 => LibraryFilter::RunsHere,
            _ => LibraryFilter::All,
        });
    }

    ensure_library_cache(bp, f);
    let cache = bp.library_cache.as_ref().unwrap();
    let total_games = cache.all_games.len();
    let n_installed = cache.n_installed;
    let n_favorites = cache.n_favorites;
    let n_runs = cache.n_runs;
    let collection_chips: Vec<(String, usize)> = cache
        .collections
        .iter()
        .take(5)
        .map(|(name, set)| (name.clone(), set.len()))
        .collect();

    // Build available filter chips.
    let mut chips: Vec<(LibraryFilter, Option<&'static str>, String)> = vec![(
        LibraryFilter::Installed,
        None,
        t!("Installed ({n})", n = n_installed),
    )];
    if n_favorites > 0 {
        chips.push((LibraryFilter::Favorites, Some(icons::STAR), t!("Favorites ({n})", n = n_favorites)));
    }
    if n_runs < total_games {
        let os_icon = if cfg!(target_os = "macos") {
            icons::APPLE
        } else if cfg!(windows) {
            icons::WINDOWS
        } else {
            icons::LINUX
        };
        chips.push((LibraryFilter::RunsHere, Some(os_icon), t!("Runs here ({n})", n = n_runs)));
    }
    chips.push((LibraryFilter::All, None, t!("All ({n})", n = total_games)));
    for (cname, count) in &collection_chips {
        chips.push((LibraryFilter::Collection(cname.clone()), Some(icons::TAG), format!("{cname} ({count})")));
    }

    let current = bp.library_filter.clone().unwrap_or(LibraryFilter::Installed);
    let current = if chips.iter().any(|c| c.0 == current) {
        current
    } else {
        bp.library_filter = Some(LibraryFilter::Installed);
        LibraryFilter::Installed
    };

    if bp.library_search_open {
        if f.x {
            bp.library_query.pop();
        }
        if f.y {
            bp.library_query.push(' ');
        }
    } else {
        if f.x {
            let idx = chips.iter().position(|c| c.0 == current).unwrap_or(0);
            let next = chips[(idx + 1) % chips.len()].0.clone();
            bp.library_filter = Some(next);
        }
        if f.y {
            bp.library_sort = bp.library_sort.next();
        }
    }

    // Top controls row: filter chips + Search + Sort (wrapping gracefully on 1280×800).
    let current = bp.library_filter.clone().unwrap_or(LibraryFilter::Installed);
    let mut x = left;
    let chip_gap = 14.0 * s;
    let place_chip = |bp: &mut BigPicture, ui: &Ui, f: &Frame, id: Id, icon: Option<&str>, label: &str, kind: Kind, x: &mut f32, y: &mut f32| -> bool {
        let wdt = paint::pill_width(ui, icon.is_some(), label, s);
        if *x > left && *x + wdt > right {
            *x = left;
            *y += 82.0 * s;
        }
        let (r, hit) = button(bp, ui, f, id, pos2(*x, *y), 0.0, icon, label, kind, None);
        *x = r.right() + chip_gap;
        hit.activated
    };

    for (value, icon, label) in &chips {
        let on = current == *value;
        let chip_id = match value {
            LibraryFilter::Installed => Id::unique("bp-lib-filter-installed"),
            LibraryFilter::Favorites => Id::unique("bp-lib-filter-favorites"),
            LibraryFilter::RunsHere => Id::unique("bp-lib-filter-runs"),
            LibraryFilter::All => Id::unique("bp-lib-filter-all"),
            LibraryFilter::Collection(name) => Id::unique(("bp-lib-filter-col", name)),
        };
        if place_chip(bp, ui, f, chip_id, *icon, label, if on { Kind::Primary } else { Kind::Secondary }, &mut x, &mut y) {
            bp.library_filter = Some(value.clone());
        }
    }
    let sort_label = format!("{}", bp.library_sort.label());
    if place_chip(bp, ui, f, Id::unique("bp-lib-sort"), Some(icons::SORT), &sort_label, Kind::Secondary, &mut x, &mut y) {
        bp.library_sort = bp.library_sort.next();
    }
    let search_label = if bp.library_query.trim().is_empty() {
        t!("Filter").to_string()
    } else {
        format!("“{}”", bp.library_query.trim())
    };
    if place_chip(
        bp,
        ui,
        f,
        Id::unique("bp-lib-search-btn"),
        Some(icons::SEARCH),
        &search_label,
        if bp.library_search_open || !bp.library_query.is_empty() { Kind::Primary } else { Kind::Secondary },
        &mut x,
        &mut y,
    ) {
        bp.library_search_open = !bp.library_search_open;
    }
    y += 72.0 * s + 24.0 * s;

    // Inline local library search box + OSK when open.
    if bp.library_search_open {
        let field_w = (right - left).min(960.0 * s);
        let field = Rect::from_min_size(pos2(left, y), vec2(field_w, 72.0 * s));
        let p = ui.painter().clone();
        p.rect_filled(field, paint::radius(36.0 * s), Color32::from_black_alpha(125));
        p.rect_stroke(field, paint::radius(36.0 * s), egui::Stroke::new(2.0 * s, f.t.accent.gamma_multiply(0.75)), egui::StrokeKind::Inside);
        p.text(pos2(field.left() + 30.0 * s, field.center().y), Align2::LEFT_CENTER, icons::SEARCH, w::font(26.0 * s), f.t.text_dim);
        let tx = field.left() + 72.0 * s;
        if bp.library_query.is_empty() {
            p.text(pos2(tx, field.center().y), Align2::LEFT_CENTER, t!("Filter your library…"), w::font(26.0 * s), f.t.text_faint);
        } else {
            paint::line(&p, pos2(tx, field.center().y), Align2::LEFT_CENTER, &bp.library_query, w::font_semibold(26.0 * s), f.t.text, field.right() - tx - 28.0 * s);
        }
        y += 88.0 * s;
        match super::osk::draw(bp, ui, f, left, right - left, &mut y, t!("Done")) {
            Some(super::osk::Key::Char(ch)) => bp.library_query.push(ch),
            Some(super::osk::Key::Del) => {
                bp.library_query.pop();
            }
            Some(super::osk::Key::Clear) => bp.library_query.clear(),
            Some(super::osk::Key::Go) => bp.library_search_open = false,
            None => {}
        }
        y += 22.0 * s;
    }

    // LT / RT fast-jump through large libraries.
    if !bp.library_search_open {
        let jump = f.view.height() * 0.85;
        if f.lt {
            bp.scroller("page").target -= jump;
        }
        if f.rt {
            bp.scroller("page").target += jump;
        }
    }

    let q_lower = bp.library_query.trim().to_lowercase();
    let active_filter = bp.library_filter.clone().unwrap_or(LibraryFilter::Installed);
    let filtered: Vec<(AppId, String, bool, bool, bool, u64, u64)> = {
        let cache = bp.library_cache.as_ref().unwrap();
        let col_set = match &active_filter {
            LibraryFilter::Collection(name) => cache.collections.iter().find(|(n, _)| n == name).map(|(_, s)| s),
            _ => None,
        };
        cache
            .all_games
            .iter()
            .filter(|g| match &active_filter {
                LibraryFilter::Installed => g.installed,
                LibraryFilter::Favorites => g.favorite,
                LibraryFilter::RunsHere => g.runs,
                LibraryFilter::All => true,
                LibraryFilter::Collection(_) => col_set.is_some_and(|s| s.contains(&g.id)),
            })
            .filter(|g| q_lower.is_empty() || g.name.to_lowercase().contains(&q_lower))
            .map(|g| (g.id, g.name.clone(), g.installed, g.runs, g.favorite, g.playtime_minutes, g.size_on_disk))
            .collect()
    };

    if filtered.is_empty() {
        let loading = f.app.state.apps.is_none();
        let (title, body) = if loading {
            (t!("Loading your library…"), "")
        } else if !q_lower.is_empty() {
            (t!("No matching games"), t!("Try clearing the search filter."))
        } else {
            (t!("No games here"), t!("Switch to All games to see everything you own."))
        };
        empty(bp, ui, f, y - 40.0 * s, icons::BOOKS, title, body, None);
        return y - top + 380.0 * s;
    }

    let tile = vec2(190.0 * s, 285.0 * s);
    let gap = 26.0 * s;
    let row_h = tile.y + 76.0 * s;
    let width = right - left;
    let cols = (((width + gap) / (tile.x + gap)).floor() as usize).max(1);
    let clip = ui.clip_rect();
    for (k, (id, name, installed, runs, fav, mins, size)) in filtered.iter().enumerate() {
        let (row, col) = (k / cols, k % cols);
        let r = Rect::from_min_size(pos2(left + col as f32 * (tile.x + gap), y + row as f32 * row_h), tile);
        let hit = bp.focus.item(ui, Id::unique(("bp-lib", *id)), r, clip, Place::Page);
        if r.bottom() + row_h > clip.top() && r.top() - row_h < clip.bottom() {
            let caption = match bp.library_sort {
                super::LibrarySort::Playtime if *mins > 0 => ui::playtime(*mins),
                super::LibrarySort::Size if *size > 0 => ui::bytes(*size),
                _ if *installed => String::new(),
                _ if !runs => t!("Doesn't run on this computer").to_string(),
                _ => t!("Not installed").to_string(),
            };
            capsule(bp, ui, f, r, &hit, k.min(10), *id, name, &caption, !installed);
            if *fav {
                let star_c = pos2(r.right() - 18.0 * s, r.top() + 18.0 * s);
                ui.painter().circle_filled(star_c, 14.0 * s, Color32::from_black_alpha(165));
                ui.painter().text(star_c, Align2::CENTER_CENTER, icons::STAR, w::font(16.0 * s), f.t.away);
            }
        }
        if hit.activated {
            bp.open(Page::Game(*id), f.now);
        }
    }
    let rows = filtered.len().div_ceil(cols);
    y += rows as f32 * row_h;
    y - top + 30.0 * s
}

// ---------- Game page ----------

fn game(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: AppId) -> f32 {
    let s = f.s;
    bp.bg_want = Some(id);
    let installed = f.app.local.games.iter().find(|g| g.app_id == id).cloned();
    let name = f.app.state.app_name(id).map(String::from).or_else(|| installed.as_ref().map(|g| g.name.clone())).unwrap_or_default();

    let (k, dy) = appear(ui, bp, 0);
    let logo = Rect::from_min_size(pos2(f.view.left() + f.pad, top + 70.0 * s + dy * s), vec2(620.0 * s, 230.0 * s));
    let c = paint::faded(ui, logo.expand(20.0 * s), k);
    if !w::paint_contain(&c, logo, &art::logo(id), Align2::LEFT_BOTTOM) {
        paint::wrapped(c.painter(), pos2(logo.left(), logo.bottom() - 130.0 * s), &name, w::font_bold(58.0 * s), Color32::WHITE, 900.0 * s, 2);
    }

    // Buttons (wrap cleanly on 1280×800 handheld widths).
    let left = f.view.left() + f.pad;
    let right = f.view.right() - f.pad;
    let mut y = top + 350.0 * s;
    let action = with_library(f.app, f.actions, |lib, cx| lib.main_action(cx, id)).unwrap_or(MainAction::StorePage);
    let is_fav = with_library(f.app, f.actions, |lib, _| lib.is_favorite(id)).unwrap_or(false);
    if f.x {
        with_library(f.app, f.actions, |lib, cx| lib.toggle_favorite(cx, id));
        bp.library_cache_rev = bp.library_cache_rev.wrapping_add(1);
    }
    if f.y {
        bp.open(Page::GameManage(id), f.now);
    }
    let (label, icon, kind, progress) = main_button(action);
    let main_id = Id::unique(("bp-game-main", id));
    bp.preferred = Some(main_id);
    let (b, main) = button(bp, ui, f, main_id, pos2(left, y), 260.0 * s, Some(icon), &label, kind, progress);
    if main.activated {
        if action == MainAction::StorePage {
            bp.open(Page::StoreApp(id), f.now);
        } else if action == MainAction::Install {
            // Like Steam: size, where, and a shortcut, before anything starts.
            bp.install = None;
            bp.open(Page::Install(id), f.now);
        } else {
            with_library(f.app, f.actions, |lib, cx| lib.run_main_action(cx, id));
        }
    }
    let mut x = b.right() + 18.0 * s;
    let place_btn = |bp: &mut BigPicture, ui: &Ui, f: &Frame, btn_id: Id, ic: &str, lbl: &str, k: Kind, x: &mut f32, y: &mut f32| -> bool {
        let wdt = paint::pill_width(ui, true, lbl, s);
        if *x > left && *x + wdt > right {
            *x = left;
            *y += 84.0 * s;
        }
        let (r, hit) = button(bp, ui, f, btn_id, pos2(*x, *y), 0.0, Some(ic), lbl, k, None);
        *x = r.right() + 18.0 * s;
        hit.activated
    };
    if let MainAction::Busy { verifying, .. } = action
        && place_btn(bp, ui, f, Id::unique(("bp-game-cancel", id)), icons::X, t!("Cancel"), Kind::Secondary, &mut x, &mut y)
    {
        f.app.client.send(if verifying { Command::CancelVerify { app_id: id } } else { Command::CancelDownload { app_id: id } });
    }
    if place_btn(bp, ui, f, Id::unique(("bp-game-manage", id)), icons::SLIDERS, t!("Manage"), Kind::Secondary, &mut x, &mut y) {
        bp.open(Page::GameManage(id), f.now);
    }
    if place_btn(
        bp,
        ui,
        f,
        Id::unique(("bp-game-fav", id)),
        icons::STAR,
        if is_fav { t!("Favorited") } else { t!("Favorite") },
        if is_fav { Kind::Primary } else { Kind::Secondary },
        &mut x,
        &mut y,
    ) {
        with_library(f.app, f.actions, |lib, cx| lib.toggle_favorite(cx, id));
        bp.library_cache_rev = bp.library_cache_rev.wrapping_add(1);
    }
    if action != MainAction::StorePage
        && place_btn(bp, ui, f, Id::unique(("bp-game-store", id)), icons::STORE, t!("Store page"), Kind::Secondary, &mut x, &mut y)
    {
        bp.open(Page::StoreApp(id), f.now);
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if installed.is_some()
        && !f.app.demo
        && !matches!(action, MainAction::Busy { .. })
        && place_btn(bp, ui, f, Id::unique(("bp-game-compatibility", id)), icons::GEAR, t!("Compatibility"), Kind::Secondary, &mut x, &mut y)
    {
        bp.open(Page::Compatibility(id), f.now);
    }
    if installed.is_some()
        && !f.app.demo
        && place_btn(bp, ui, f, Id::unique(("bp-game-controller", id)), icons::CONTROLLER, t!("Controller"), Kind::Secondary, &mut x, &mut y)
    {
        bp.open(Page::Controller(id), f.now);
    }
    y += 72.0 * s + 44.0 * s;

    // Facts.
    let played = installed.as_ref().and_then(|g| g.playtime_minutes).or_else(|| match f.app.state.player_games.get(&f.app.state.steam_id) {
        Some(Ok(list)) => list.iter().find(|g| g.app_id == id).map(|g| g.playtime_minutes),
        _ => None,
    });
    let mut facts: Vec<(String, String)> = Vec::new();
    facts.push((t!("Play time").into(), played.filter(|m| *m > 0).map(ui::playtime).unwrap_or_else(|| t!("None yet").into())));
    let last = installed.as_ref().and_then(|g| g.last_played);
    facts.push((t!("Last played").into(), last.map(ui::ago).unwrap_or_else(|| t!("Never").into())));
    if let Some(g) = &installed {
        facts.push((t!("Size").into(), ui::bytes(g.size_on_disk)));
    }
    let achievements = f.app.state.achievements.get(&id).copied().or_else(|| match f.app.state.achievement_list.get(&id) {
        Some(Ok(list)) if !list.is_empty() => Some((list.iter().filter(|a| a.unlocked).count() as u32, list.len() as u32)),
        _ => None,
    });
    if let Some((got, total)) = achievements.filter(|a| a.1 > 0) {
        facts.push((t!("Achievements").into(), format!("{got} / {total}")));
    }
    let p = ui.painter().clone();
    let mut x = f.view.left() + f.pad;
    for (label, value) in &facts {
        p.text(pos2(x, y), Align2::LEFT_TOP, label.to_uppercase(), w::font_bold(15.0 * s), f.t.text_faint);
        let r = p.text(pos2(x, y + 26.0 * s), Align2::LEFT_TOP, value, w::font_semibold(28.0 * s), f.t.text);
        x = r.right().max(x + 180.0 * s) + 60.0 * s;
    }
    if let Some((got, total)) = achievements.filter(|a| a.1 > 0) {
        let r = Rect::from_min_size(pos2(x - 40.0 * s, y + 38.0 * s), vec2(220.0 * s, 8.0 * s));
        paint::bar(ui, &f.t, r, got as f32 / total as f32, false);
    }
    y += 100.0 * s;

    // Achievements, when Steam already sent them.
    let list = match f.app.state.achievement_list.get(&id) {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    if f.app.state.achievement_list.get(&id).is_none() && !f.app.demo && f.app.state.online() && bp.asked.insert(id) {
        f.app.client.send(Command::LoadAchievements { app_id: id, language: i18n::language().steam_name().into() });
    }
    if !list.is_empty() {
        y += 10.0 * s;
        heading(ui, f, &mut y, t!("Achievements"));
        let mut list = list;
        // Unlocked first, newest first; then the rest, most common first.
        list.sort_by(|a, b| b.unlocked.cmp(&a.unlocked).then(b.unlock_time.cmp(&a.unlock_time)).then(b.global_percent.total_cmp(&a.global_percent)));
        let tile = vec2(120.0 * s, 120.0 * s);
        let tiles = shelf(bp, ui, f, &mut y, "achievements", list.len(), tile, |i| Id::unique(("bp-ach", i)));
        let mut focused = None;
        for (i, r, hit) in tiles {
            let a = &list[i];
            let shown = paint::grow(r, 1.0 + 0.08 * hit.glow);
            paint::shadow(ui.painter(), shown, 12.0 * s, s, 0.4 + 0.6 * hit.glow, Color32::from_black_alpha(200));
            let url = if a.unlocked { &a.icon } else { &a.icon_gray };
            if !w::paint_cover(ui, shown, url, paint::radius(12.0 * s)) {
                if url.is_empty() || w::image_state(ui, url) == w::ImageState::Missing {
                    ui.painter().rect_filled(shown, paint::radius(12.0 * s), f.t.raised);
                    ui.painter().text(shown.center(), Align2::CENTER_CENTER, icons::TROPHY, w::font(48.0 * s), if a.unlocked { f.t.away } else { f.t.text_faint });
                } else {
                    paint::skeleton(ui, &f.t, shown, paint::radius(12.0 * s));
                }
            }
            if !a.unlocked {
                ui.painter().rect_filled(shown, paint::radius(12.0 * s), Color32::from_black_alpha(90));
            }
            if hit.focused {
                bp.focus.ring_around(shown, 12.0 * s);
                focused = Some(a.clone());
            }
        }
        y += 22.0 * s;
        if let Some(a) = focused {
            let hidden = a.hidden && !a.unlocked;
            let desc = if hidden { t!("Hidden until you unlock it").to_string() } else { a.description.clone() };
            let when = if a.unlocked && a.unlock_time > 0 { t!("Unlocked {date}", date = ui::date(a.unlock_time)) } else { t!("{pct}% of players have this", pct = format!("{:.1}", a.global_percent)) };
            p.text(pos2(f.view.left() + f.pad, y), Align2::LEFT_TOP, &a.name, w::font_bold(24.0 * s), f.t.text);
            paint::line(&p, pos2(f.view.left() + f.pad, y + 34.0 * s), Align2::LEFT_TOP, &format!("{desc}   ·   {when}"), w::font(19.0 * s), f.t.text_dim, f.view.width() - 2.0 * f.pad);
        } else {
            let unlocked = list.iter().filter(|a| a.unlocked).count();
            p.text(pos2(f.view.left() + f.pad, y), Align2::LEFT_TOP, t!("{n} of {total} unlocked", n = unlocked, total = list.len()), w::font(19.0 * s), f.t.text_dim);
        }
        y += 80.0 * s;
    }
    y - top + 40.0 * s
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn compatibility_choices<'a>(native: Option<(bool, bool)>, default: Option<&str>, builds: &'a [(String, String)]) -> Vec<(Option<&'a str>, String)> {
    let mut choices = vec![(None, t!("Automatic").to_string())];
    if let Some((_, windows)) = native {
        choices.push((Some(client::NATIVE), t!("Native (Linux)").to_string()));
        if !windows {
            return choices;
        }
        if let Some(tool) = default {
            choices.push((Some(client::DEFAULT_PROTON), t!("Default Proton ({tool})", tool = tool)));
        }
    }
    choices.extend(builds.iter().map(|(name, label)| (Some(name.as_str()), label.clone())));
    choices
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn compatibility_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: AppId) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let mut y = top + 24.0 * s;
    heading(ui, f, &mut y, t!("Compatibility"));
    let Some(installed) = f.app.local.games.iter().find(|game| game.app_id == id) else {
        empty(bp, ui, f, y, icons::WARNING, t!("Game is not installed"), "", None);
        return y - top + 320.0 * s;
    };
    bp.bg_want = Some(id);
    let name = f.app.state.app_name(id).unwrap_or(&installed.name);
    let width = f.view.width() - 2.0 * f.pad;
    paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, name, w::font_bold(26.0 * s), f.t.text, width);
    y += 42.0 * s;
    let note = t!("Choose how Aurora runs this game. Games handed off to Steam use Steam’s own settings.");
    let note_rect = paint::wrapped(ui.painter(), pos2(left, y), note, w::font(20.0 * s), f.t.text_dim, width, usize::MAX);
    y = note_rect.bottom() + 24.0 * s;

    // Cache native_build / default_proton / proton_builds per page visit (no per-frame disk I/O).
    let (native, default, builds) = match &bp.compat_cache {
        Some((cached_id, native, default, builds)) if *cached_id == id => {
            (*native, default.clone(), builds.clone())
        }
        _ => {
            let native = f.app.state.apps.as_ref().and_then(|apps| apps.get(&id)).and_then(|app| client::native_build(app, &installed.install_dir));
            let default = client::default_proton(id);
            let builds = client::proton_builds();
            bp.compat_cache = Some((id, native, default.clone(), builds.clone()));
            (native, default, builds)
        }
    };
    let current = client::app_proton(id);
    let choices = compatibility_choices(native, default.as_deref(), &builds);
    let is_selected = |choice: Option<&str>| match (current.as_deref(), choice) {
        (None, None) => true,
        (Some(saved), Some(name)) => saved.eq_ignore_ascii_case(name),
        _ => false,
    };
    if !choices.iter().any(|(choice, _)| is_selected(*choice)) {
        let text = t!("The saved compatibility choice is unavailable for this installation.");
        let warning = paint::wrapped(ui.painter(), pos2(left, y), text, w::font(20.0 * s), f.t.text_dim, width, usize::MAX);
        y = warning.bottom() + 16.0 * s;
    } else if builds.is_empty() && !matches!(native, Some((_, false))) {
        let text = t!("No installed Proton versions found.");
        paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, text, w::font(20.0 * s), f.t.text_dim, width);
        y += 38.0 * s;
    }

    heading(ui, f, &mut y, t!("Launch preference"));
    let mut installed_group = false;
    for (choice, label) in choices {
        if !installed_group && choice.is_some_and(|tool| !matches!(tool, client::NATIVE | client::DEFAULT_PROTON)) {
            y += 12.0 * s;
            heading(ui, f, &mut y, t!("Installed Proton versions"));
            installed_group = true;
        }
        let row = Id::unique(("bp-compatibility-choice", id, choice));
        let selected = is_selected(choice);
        if choice.is_none() || selected {
            bp.preferred = Some(row);
        }
        let rect = Rect::from_min_size(pos2(left, y), vec2(width.min(950.0 * s), 52.0 * s));
        let icon = if selected { icons::CHECK } else { icons::GEAR };
        let status = if selected { t!("Selected") } else { "" };
        let hit = compact_row(bp, ui, f, row, rect, icon, &label, status);
        if hit.activated && !selected {
            client::set_app_proton(id, choice.map(String::from));
        }
        y += 60.0 * s;
    }
    y - top + 24.0 * s
}

fn controller_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: AppId) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let mut y = top + 24.0 * s;
    heading(ui, f, &mut y, t!("Steam Input"));
    bp.bg_want = Some(id);
    let current = client::app_controller(id);
    let width = f.view.width() - 2.0 * f.pad;
    let note = t!("Override controller handling for this game.");
    let line = paint::wrapped(ui.painter(), pos2(left, y), note, w::font(20.0 * s), f.t.text_dim, width, usize::MAX);
    y = line.bottom() + 24.0 * s;
    for (choice, label) in [(0, t!("Use the default settings")), (1, t!("Enable Steam Input")), (2, t!("Disable Steam Input"))] {
        let row = Id::unique(("bp-game-steam-input", id, choice));
        if choice == current {
            bp.preferred = Some(row);
        }
        let selected = choice == current;
        let (_, hit) = button(bp, ui, f, row, pos2(left, y), 520.0 * s, selected.then_some(icons::CHECK), label,
            if selected { Kind::Primary } else { Kind::Secondary }, None);
        if hit.activated && !selected {
            with_library(f.app, f.actions, |lib, _| lib.set_game_controller(id, choice));
        }
        y += 84.0 * s;
    }
    let (_, hit) = button(bp, ui, f, Id::unique(("bp-game-controller-settings", id)), pos2(left, y), 520.0 * s, Some(icons::GEAR), t!("Controller settings"), Kind::Secondary, None);
    if hit.activated {
        bp.settings.open_at(super::settings::CONTROLLER);
        bp.open(Page::Settings, f.now);
    }
    y - top + 96.0 * s
}

fn notifications(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let mut y = top + 24.0 * s;
    heading(ui, f, &mut y, t!("Notifications"));
    let settings_id = Id::unique("bp-notification-settings");
    let (r, settings) = button(bp, ui, f, settings_id, pos2(left, y), 0.0, Some(icons::GEAR), t!("Settings"), Kind::Secondary, None);
    if settings.activated {
        bp.settings.open_at(super::settings::NOTIFICATIONS);
        bp.open(Page::Settings, f.now);
    }
    let data = f.app.state.notifications.as_ref();
    if data.is_some_and(|data| data.unread > 0) && !f.app.demo {
        let (_, read) = button(bp, ui, f, Id::unique("bp-notifications-mark-all"), pos2(r.right() + 20.0 * s, y), 0.0,
            Some(icons::CHECK), t!("Mark all as read"), Kind::Secondary, None);
        if read.activated {
            f.app.client.send(Command::MarkNotificationsRead { ids: vec![] });
        }
    }
    y += 96.0 * s;
    let width = (f.view.width() - 2.0 * f.pad).min(1100.0 * s);
    if let Some(d) = data {
        let mut summary = Vec::new();
        if d.pending_gifts > 0 {
            summary.push(("gifts", icons::GIFT, t!("{n} gifts waiting for you", n = d.pending_gifts), 2));
        }
        if d.pending_friends > 0 {
            summary.push(("friends", icons::USER_PLUS, t!("{n} friend requests", n = d.pending_friends), 5));
        }
        if d.confirmations > 0 {
            summary.push(("confirmations", icons::SHIELD, t!("{n} confirmations waiting in the Steam Mobile app", n = d.confirmations), 26));
        }
        for (key, icon, text, kind) in summary {
            let id = Id::unique(("bp-notification-summary", key));
            let rect = Rect::from_min_size(pos2(left, y), vec2(width, 64.0 * s));
            let hit = bp.focus.item(ui, id, rect, ui.clip_rect(), Place::Page);
            let painter = ui.painter();
            painter.rect_filled(rect, paint::radius(12.0 * s), w::mix(f.t.accent.gamma_multiply(0.14), f.t.accent.gamma_multiply(0.28), hit.glow));
            painter.text(pos2(left + 22.0 * s, rect.center().y), Align2::LEFT_CENTER, icon, w::font(24.0 * s), f.t.accent);
            paint::line(painter, pos2(left + 62.0 * s, rect.center().y), Align2::LEFT_CENTER, &text, w::font_semibold(20.0 * s), f.t.text, width - 108.0 * s);
            painter.text(pos2(rect.right() - 20.0 * s, rect.center().y), Align2::RIGHT_CENTER, icons::CARET_RIGHT, w::font(18.0 * s), f.t.text_dim);
            if hit.focused {
                bp.focus.ring_around(rect, 12.0 * s);
            }
            if hit.activated {
                let row = model::Notification { kind, ..Default::default() };
                crate::shell::open_notification(&f.app.state, &row, f.actions);
            }
            y += 74.0 * s;
        }
    }
    if let Some(data) = data.filter(|data| !data.items.is_empty()) {
        for n in data.items.iter().take(40) {
            let id = Id::unique(("bp-notification", n.id));
            let rect = Rect::from_min_size(pos2(left, y), vec2(width, 84.0 * s));
            let hit = bp.focus.item(ui, id, rect, ui.clip_rect(), Place::Page);
            let (icon, label) = crate::shell::notification_text(&f.app.state, n);
            let color = if n.read { f.t.text_dim } else { f.t.text };
            if rect.intersects(ui.clip_rect()) {
                ui.painter().rect_filled(rect, paint::radius(12.0 * s), w::mix(f.t.surface, f.t.raised, hit.glow));
                let pic = Rect::from_center_size(pos2(left + 44.0 * s, rect.center().y), vec2(54.0 * s, 54.0 * s));
                crate::shell::notification_picture_scaled(ui, &f.t, &f.app.state, n, icon, pic, s);
                let painter = ui.painter();
                let x = pic.right() + 18.0 * s;
                let text_w = (rect.right() - x - 34.0 * s).max(40.0);
                paint::wrapped(painter, pos2(x, rect.top() + 14.0 * s), &label, w::font_medium(20.0 * s), color, text_w, 2);
                paint::line(painter, pos2(x, rect.bottom() - 12.0 * s), Align2::LEFT_BOTTOM, &ui::ago(n.time), w::font(15.5 * s), f.t.text_faint, text_w);
                if !n.read {
                    painter.circle_filled(rect.right_center() - vec2(18.0 * s, 0.0), 5.0 * s, f.t.accent);
                }
            }
            if hit.focused {
                bp.focus.ring_around(rect, 12.0 * s);
            }
            if hit.activated {
                if !n.read && !f.app.demo {
                    f.app.client.send(Command::MarkNotificationsRead { ids: vec![n.id] });
                }
                crate::shell::open_notification(&f.app.state, n, f.actions);
            }
            y += 96.0 * s;
        }
    } else {
        let label = if data.is_some() { t!("You're all caught up.") } else { t!("Notifications unavailable") };
        paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, label, w::font(20.0 * s), f.t.text_dim, width);
        y += 34.0 * s;
    }
    y - top + 24.0 * s
}

// ---------- Store ----------

fn store_tab(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32) -> f32 {
    let s = f.s;
    let mut y = top + 16.0 * s;
    let search_id = Id::unique("bp-store-search");
    let (_, hit) = button(bp, ui, f, search_id, pos2(f.view.left() + f.pad, y), 520.0 * s, Some(icons::SEARCH), t!("Search the store"), Kind::Secondary, None);
    if hit.activated {
        bp.open(Page::Search, f.now);
    }
    if !f.app.demo && f.app.state.wishlist.is_none() && f.app.state.online() && bp.asked.insert(0) {
        f.app.client.send(Command::LoadWishlist);
    }
    y += 72.0 * s + 44.0 * s;
    let front = with_store(f.app, f.actions, |st, cx| st.front_for(cx)).flatten();
    match front {
        None => {
            heading(ui, f, &mut y, t!("Featured"));
            for i in 0..3 {
                let r = Rect::from_min_size(pos2(f.view.left() + f.pad + i as f32 * 586.0 * s, y), vec2(560.0 * s, 320.0 * s));
                paint::skeleton(ui, &f.t, r, paint::radius(10.0 * s));
            }
            y += 400.0 * s;
        }
        Some(Err(e)) => {
            empty(bp, ui, f, y, icons::WARNING, t!("The store couldn't load"), &i18n::tr_dyn(&e), None);
            y += 400.0 * s;
        }
        Some(Ok(front)) => {
            let (featured, sections) = &*front;
            let ids: Vec<AppId> = featured.iter().chain(sections.iter().flat_map(|s| s.items.iter())).map(|i| i.app_id).collect();
            let shown: std::collections::HashMap<AppId, Option<bool>> = with_store(f.app, f.actions, |st, _| ids.iter().map(|id| (*id, st.shown(*id))).collect()).unwrap_or_default();
            let ok = |id: &AppId| shown.get(id).copied().flatten() != Some(false);
            let featured: Vec<&store::Item> = featured.iter().filter(|i| ok(&i.app_id)).collect();
            let mut order = 0;
            if !featured.is_empty() {
                heading(ui, f, &mut y, t!("Featured"));
                let tiles = shelf(bp, ui, f, &mut y, "featured", featured.len(), vec2(560.0 * s, 320.0 * s), |i| Id::unique(("bp-feat", i)));
                for (i, r, hit) in tiles {
                    store_tile(bp, ui, f, r, &hit, order + i, featured[i], shown.get(&featured[i].app_id).copied().flatten(), true);
                }
                order += 2;
                y += 90.0 * s;
            }
            for sec in sections.iter().filter(|s| !s.items.is_empty()).take(6) {
                let items: Vec<&store::Item> = sec.items.iter().filter(|i| ok(&i.app_id)).collect();
                if items.is_empty() {
                    continue;
                }
                heading(ui, f, &mut y, &sec.title);
                let key = format!("sec-{}", sec.id);
                let tiles = shelf(bp, ui, f, &mut y, &key, items.len(), vec2(420.0 * s, 196.0 * s), |i| Id::unique(("bp-sec", &sec.id, i)));
                for (i, r, hit) in tiles {
                    store_tile(bp, ui, f, r, &hit, order + i, items[i], shown.get(&items[i].app_id).copied().flatten(), false);
                }
                order += 1;
                y += 90.0 * s;
            }
        }
    }
    y - top + 20.0 * s
}

#[allow(clippy::too_many_arguments)]
fn store_tile(bp: &mut BigPicture, ui: &mut Ui, f: &Frame, r: Rect, hit: &Hit, order: usize, item: &store::Item, shown: Option<bool>, large: bool) {
    let owned = f.app.state.apps.as_ref().is_some_and(|a| a.contains_key(&item.app_id));
    let price = if owned { t!("In your library").to_string() } else { item.price.label() };
    let color = if owned { f.t.online } else { f.t.text_dim };
    // Not checked against the content filter yet: a placeholder.
    let urls: Vec<&str> = if shown.is_none() { vec![] } else if large { item.large.as_deref().into_iter().chain(std::iter::once(item.header.as_str())).collect() } else { vec![item.header.as_str()] };
    let drawn = wide(bp, ui, f, r, hit, order, &urls, &item.name, &price, color);
    if item.price.discount > 0 && !owned {
        let badge = Rect::from_min_size(drawn.right_bottom() - vec2(84.0 * f.s, 40.0 * f.s), vec2(76.0 * f.s, 32.0 * f.s));
        ui.painter().rect_filled(badge, paint::radius(8.0 * f.s), f.t.discount);
        ui.painter().text(badge.center(), Align2::CENTER_CENTER, format!("-{}%", item.price.discount), w::font_bold(19.0 * f.s), Color32::from_rgb(0xBE, 0xEE, 0x11));
    }
    if hit.focused && shown == Some(true) {
        bp.bg_want = Some(item.app_id);
    }
    if hit.activated {
        bp.open(Page::StoreApp(item.app_id), f.now);
    }
}

fn store_details_error(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, y: f32, id: AppId, error: &str) -> f32 {
    if empty(bp, ui, f, y, icons::WARNING, t!("The store didn't load"), &i18n::tr_dyn(error), Some((icons::REFRESH, t!("Try again")))) == Some(true) {
        with_store(f.app, f.actions, |store, _| store.retry_details(id));
    }
    500.0 * f.s
}

fn store_app(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: AppId) -> f32 {
    let s = f.s;
    match with_store(f.app, f.actions, |store, cx| store.big_picture_access(cx, id)).flatten() {
        Some(Ok(true)) => {}
        Some(Ok(false)) => {
            if empty(bp, ui, f, top, icons::WARNING, t!("Mature content"), t!("This game contains content you chose to hide in your store preferences."), Some((icons::STORE, t!("View page")))) == Some(true) {
                with_store(f.app, f.actions, |store, _| store.allow_big_picture_page(id));
            }
            return 500.0 * s;
        }
        Some(Err(error)) => return store_details_error(bp, ui, f, top, id, &error),
        None => {
            empty(bp, ui, f, top, icons::STORE, t!("Loading…"), "", None);
            return 500.0 * s;
        }
    }
    bp.bg_want = Some(id);
    let details = match with_store(f.app, f.actions, |st, _| st.details_result(id)).flatten() {
        Some(Ok(details)) => Some(details),
        Some(Err(error)) => return store_details_error(bp, ui, f, top, id, &error),
        None => None,
    };
    let mut y = top + 20.0 * s;
    let left = f.view.left() + f.pad;
    let header = Rect::from_min_size(pos2(left, y), vec2(640.0 * s, 300.0 * s));
    let (k, dy) = appear(ui, bp, 0);
    let c = paint::faded(ui, header.expand(40.0 * s), k);
    let header_url = details.as_ref().map(|d| d.header.clone()).unwrap_or_else(|| art::header(id));
    paint::shadow(c.painter(), header, 14.0 * s, s, 0.8, Color32::from_black_alpha(220));
    if !w::paint_cover(&c, header.translate(vec2(0.0, dy * s)), &header_url, paint::radius(14.0 * s)) {
        paint::skeleton(&c, &f.t, header, paint::radius(14.0 * s));
    }
    let tx = header.right() + 48.0 * s;
    let tw = f.view.right() - f.pad - tx;
    let p = ui.painter().clone();
    let owned = f.app.state.apps.as_ref().is_some_and(|a| a.contains_key(&id)) || f.app.local.games.iter().any(|g| g.app_id == id);
    match &details {
        Some(d) => {
            let r = paint::wrapped(&p, pos2(tx, y), &d.name, w::font_bold(44.0 * s), f.t.text, tw, 2);
            let mut ty = r.bottom() + 16.0 * s;
            let meta: Vec<String> = [d.genres.iter().take(3).cloned().collect::<Vec<_>>().join(", "), d.release_date.clone(), d.developers.first().cloned().unwrap_or_default()].into_iter().filter(|x| !x.is_empty()).collect();
            paint::line(&p, pos2(tx, ty), Align2::LEFT_TOP, &meta.join("   ·   "), w::font_medium(19.0 * s), f.t.text_faint, tw);
            ty += 38.0 * s;
            paint::wrapped(&p, pos2(tx, ty), &d.short_description, w::font(22.0 * s), f.t.text_dim, tw, 5);
        }
        None => {
            let name = f.app.state.app_name(id).unwrap_or_default().to_string();
            p.text(pos2(tx, y), Align2::LEFT_TOP, name, w::font_bold(44.0 * s), f.t.text);
            paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(tx, y + 80.0 * s), vec2(tw.min(700.0 * s), 120.0 * s)), paint::radius(8.0 * s));
        }
    }
    y = header.bottom() + 44.0 * s;

    // Buttons: open it, get it, or remember it.
    let x = left;
    let main_id = Id::unique(("bp-sa-main", id));
    bp.preferred = Some(main_id);
    let free = details.as_ref().is_some_and(|d| d.is_free);
    if owned {
        let (_, hit) = button(bp, ui, f, main_id, pos2(x, y), 280.0 * s, Some(icons::BOOKS), t!("Open in library"), Kind::Primary, None);
        if hit.activated {
            bp.open(Page::Game(id), f.now);
        }
    } else if free {
        let (_, hit) = button(bp, ui, f, main_id, pos2(x, y), 280.0 * s, Some(icons::PLUS), t!("Add to library"), Kind::Play, None);
        if hit.activated {
            f.app.client.send(Command::RequestFreeLicense { app_id: id });
        }
    } else {
        let price = details.as_ref().and_then(|d| d.price.as_ref().map(|p| p.label()).or(d.price_text.clone())).unwrap_or_default();
        let label = if price.is_empty() { t!("Buy in the store window").to_string() } else { t!("Buy for {price}", price = price) };
        let (r, hit) = button(bp, ui, f, main_id, pos2(x, y), 280.0 * s, Some(icons::CART), &label, Kind::Primary, None);
        if hit.activated {
            // Paying happens in the normal window's store (Steam takes payment on its website).
            bp.leave(ui.ctx());
            f.actions.push(Action::Navigate { page: "store".into(), arg: Some(format!("app:{id}")) });
        }
        let x = r.right() + 20.0 * s;
        let wished = f.app.state.wishlist.as_ref().is_some_and(|w| w.iter().any(|(a, _)| *a == id));
        let (label, kind) = if wished { (t!("On your wishlist"), Kind::Secondary) } else { (t!("Add to wishlist"), Kind::Secondary) };
        let (_, hit) = button(bp, ui, f, Id::unique(("bp-sa-wish", id)), pos2(x, y), 0.0, Some(icons::HEART), label, kind, None);
        if hit.activated {
            f.app.client.send(if wished { Command::WishlistRemove { app_id: id } } else { Command::WishlistAdd { app_id: id } });
        }
    }
    y += 72.0 * s + 56.0 * s;

    // Trailers: play over the page (A pauses, left/right seek, B closes).
    let trailers = with_store(f.app, f.actions, |st, _| st.trailers_for(id)).unwrap_or_default();
    if !trailers.is_empty() {
        heading(ui, f, &mut y, t!("Trailers"));
        let tiles = shelf(bp, ui, f, &mut y, "trailers", trailers.len(), vec2(480.0 * s, 270.0 * s), |i| Id::unique(("bp-trailer", i)));
        for (i, r, hit) in tiles {
            let (k, dy) = appear(ui, bp, i + 1);
            let mut c = paint::faded(ui, r.expand(40.0 * s), k);
            let sheen = (f.now - bp.focus.changed_at) as f32 / 0.7;
            let shown = paint::art_tile(&mut c, &f.t, r.translate(vec2(0.0, dy * s)), &[&trailers[i].thumbnail], "", &hit, s, sheen);
            c.painter().circle_filled(shown.center(), 38.0 * s, Color32::from_black_alpha(160));
            c.painter().text(shown.center(), Align2::CENTER_CENTER, icons::PLAY, w::font(34.0 * s), Color32::WHITE);
            paint::line(c.painter(), shown.left_bottom() + vec2(16.0 * s, -16.0 * s), Align2::LEFT_BOTTOM, &trailers[i].name, w::font_semibold(19.0 * s), Color32::WHITE, shown.width() - 32.0 * s);
            if hit.focused {
                bp.focus.ring_around(shown, 10.0 * s);
            }
            if hit.activated {
                bp.open(Page::Video(id, i), f.now);
            }
        }
        y += 60.0 * s;
    }

    // Screenshots.
    if let Some(d) = &details {
        if !d.screenshots.is_empty() {
            heading(ui, f, &mut y, t!("Screenshots"));
            let shots = d.screenshots.clone();
            let tiles = shelf(bp, ui, f, &mut y, "shots", shots.len(), vec2(480.0 * s, 270.0 * s), |i| Id::unique(("bp-shot", i)));
            for (i, r, hit) in tiles {
                let (k, dy) = appear(ui, bp, i + 1);
                let mut c = paint::faded(ui, r.expand(40.0 * s), k);
                let sheen = (f.now - bp.focus.changed_at) as f32 / 0.7;
                let shown = paint::art_tile(&mut c, &f.t, r.translate(vec2(0.0, dy * s)), &[&shots[i].thumb], "", &hit, s, sheen);
                if hit.focused {
                    bp.focus.ring_around(shown, 10.0 * s);
                }
                if hit.activated {
                    bp.open(Page::Shots(id, i), f.now);
                }
            }
            y += 60.0 * s;
        }
    }

    store_reviews(bp, ui, f, id, &mut y);
    y - top + 20.0 * s
}

fn review_filter() -> model::ReviewFilter {
    model::ReviewFilter { order: model::ReviewOrder::Recent, all_languages: true, ..Default::default() }
}

fn reviews<'a>(f: &'a Frame<'_>, id: AppId) -> Option<&'a Result<model::AppReviews, String>> {
    let filter = if f.app.demo { model::ReviewFilter::default() } else { review_filter() };
    f.app.state.app_reviews.get(&(id, filter))
}

fn store_reviews(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, id: AppId, y: &mut f32) {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let language = i18n::language();
    let load = |f: &Frame, cursor| f.app.client.send(Command::LoadAppReviews {
        app_id: id, language: language.steam_name().into(), cursor, filter: review_filter(),
    });
    if bp.reviews_requested != Some((id, language)) && !f.app.demo && f.app.state.online() {
        bp.reviews_requested = Some((id, language));
        bp.review_cursor = None;
        load(f, None);
    }
    let rating = with_store(f.app, f.actions, |store, _| store.ratings_for(id)).flatten();
    let result = reviews(f, id);
    let all = rating.or_else(|| {
        let summary = result?.as_ref().ok()?;
        let (positive, total) = (u64::from(summary.positive), u64::from(summary.total));
        (total > 0).then(|| ((positive * 100 / total).min(100) as u32, total))
    });
    if all.is_none() && result.is_none() {
        return;
    }
    heading(ui, f, y, t!("Reviews"));
    show_review_rating(ui, f, y, all);
    let summary = match result {
        None => return,
        Some(Ok(summary)) => summary,
        Some(Err(error)) => {
            paint::line(ui.painter(), pos2(left, *y), Align2::LEFT_TOP, &i18n::tr_dyn(error.as_str()), w::font(20.0 * s), f.t.text_dim, f.view.width() - 2.0 * f.pad);
            *y += 36.0 * s;
            let (_, hit) = button(bp, ui, f, Id::unique("bp-reviews-retry"), pos2(left, *y), 0.0, Some(icons::REFRESH), t!("Try again"), Kind::Secondary, None);
            if hit.activated {
                bp.review_cursor = None;
                load(f, None);
            }
            *y += 90.0 * s;
            return;
        }
    };
    if !summary.items.is_empty() {
        let tiles = shelf(bp, ui, f, y, "reviews", summary.items.len(), vec2(560.0 * s, 230.0 * s), |i| Id::unique(("bp-review", &summary.items[i].id)));
        for (i, r, hit) in tiles {
            let review = &summary.items[i];
            let shown = paint::grow(r, 1.0 + 0.03 * hit.glow);
            paint::shadow(ui.painter(), shown, 12.0 * s, s, 0.4 + 0.6 * hit.glow, Color32::from_black_alpha(200));
            ui.painter().rect_filled(shown, paint::radius(12.0 * s), f.t.surface);
            let origin = shown.left_top() + vec2(20.0 * s, 20.0 * s);
            review_heading(ui, f, origin, review);
            paint::wrapped(ui.painter(), origin + vec2(0.0, 66.0 * s), &review.text.replace('\n', " "), w::font(18.0 * s), f.t.text_dim, shown.width() - 40.0 * s, 5);
            if hit.focused {
                bp.focus.ring_around(shown, 12.0 * s);
            }
            if hit.activated {
                bp.open(Page::Review(id, review.id.clone()), f.now);
            }
        }
        *y += 22.0 * s;
    }
    if let Some(cursor) = summary.cursor.as_ref().filter(|_| !f.app.demo && f.app.state.online() && summary.items.len() < summary.total as usize) {
        // Retain a stable focus target while this page is loading.
        let pending = bp.review_cursor.as_ref() == Some(cursor);
        let label = if pending { t!("Loading…") } else { t!("More reviews") };
        let (_, hit) = button(bp, ui, f, Id::unique("bp-more-reviews"), pos2(left, *y), 0.0, Some(icons::CARET_DOWN), label, Kind::Secondary, None);
        if hit.activated && !pending {
            bp.review_cursor = Some(cursor.clone());
            load(f, Some(cursor.clone()));
        }
        *y += 92.0 * s;
    }
}

fn show_review_rating(ui: &Ui, f: &Frame, y: &mut f32, rating: Option<(u32, u64)>) {
    let Some((pct, count)) = rating else { return };
    let s = f.s;
    let x = f.view.left() + f.pad;
    let p = ui.painter();
    p.text(pos2(x, *y), Align2::LEFT_TOP, t!("ALL TIME"), w::font_bold(15.0 * s), f.t.text_faint);
    let color = if pct >= 70 { Color32::from_rgb(0x66, 0xC0, 0xF4) } else if pct >= 40 { f.t.away } else { f.t.danger };
    p.text(pos2(x, *y + 24.0 * s), Align2::LEFT_TOP, t!("{pct}% positive", pct = pct), w::font_semibold(28.0 * s), color);
    let bar = Rect::from_min_size(pos2(x, *y + 66.0 * s), vec2(300.0 * s, 8.0 * s));
    paint::bar(ui, &f.t, bar, pct as f32 / 100.0, false);
    p.text(pos2(x, *y + 82.0 * s), Align2::LEFT_TOP, t!("{count} reviews", count = crate::builtin::store::thousands(count)), w::font(17.0 * s), f.t.text_dim);
    *y += 130.0 * s;
}

fn review_heading(ui: &Ui, f: &Frame, origin: egui::Pos2, review: &model::AppReview) {
    let (icon, color, verdict) = if review.recommended {
        (icons::THUMBS_UP, Color32::from_rgb(0x66, 0xC0, 0xF4), t!("Recommended|review"))
    } else {
        (icons::THUMBS_DOWN, f.t.danger, t!("Not recommended|review"))
    };
    ui.painter().text(origin, Align2::LEFT_TOP, format!("{icon}  {verdict}"), w::font_bold(20.0 * f.s), color);
    ui.painter().text(origin + vec2(0.0, 32.0 * f.s), Align2::LEFT_TOP, t!("{h} hrs played", h = format!("{:.1}", review.minutes_played as f32 / 60.0)), w::font(16.0 * f.s), f.t.text_faint);
}

fn review_page(ui: &mut Ui, f: &mut Frame, top: f32, id: AppId, review_id: &str) -> f32 {
    let Some(review) = reviews(f, id).and_then(|result| result.as_ref().ok()).and_then(|summary| summary.items.iter().find(|review| review.id == review_id)) else {
        return 0.0;
    };
    let origin = pos2(f.view.left() + f.pad, top + 26.0 * f.s);
    review_heading(ui, f, origin, review);
    let rect = paint::wrapped(ui.painter(), origin + vec2(0.0, 70.0 * f.s), &review.text, w::font(22.0 * f.s), f.t.text, f.view.width() - 2.0 * f.pad, usize::MAX);
    rect.bottom() - top + 30.0 * f.s
}

/// A trailer over the whole page.
fn video_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, id: AppId, i: usize) -> f32 {
    let s = f.s;
    bp.bg_want = Some(id);
    bp.video_on = true;
    let area = f.view.shrink2(vec2(f.pad, 10.0 * s));
    let _ = bp.focus.item(ui, Id::unique("bp-video"), area, ui.clip_rect(), Place::Page);
    bp.focus.hide_ring();
    // 16:9 inside the area.
    let w_ = area.width().min(area.height() * 16.0 / 9.0);
    let rect = Rect::from_center_size(area.center(), vec2(w_, w_ * 9.0 / 16.0));
    let t = f.t.clone();
    let ended = with_store(f.app, f.actions, |st, _| st.play_trailer(ui, &t, rect, id, i)).unwrap_or(false);
    if ended {
        bp.back(f.now);
    }
    f.view.height()
}

/// Before an install: the size, where it goes (with free space) and a
/// desktop shortcut, like Steam's install dialog.
fn install_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: AppId) -> f32 {
    let s = f.s;
    if f.app.local.games.iter().any(|game| game.app_id == id) {
        *bp.stack.last_mut().expect("install page is stacked") = Page::Game(id);
        return 0.0;
    }
    let Some(apps) = &f.app.state.apps else {
        empty(bp, ui, f, top, icons::DOWNLOAD, t!("Loading…"), "", None);
        return 500.0 * s;
    };
    if apps.get(&id).is_none_or(|app| app.is_dlc()) {
        *bp.stack.last_mut().expect("install page is stacked") = Page::StoreApp(id);
        return 0.0;
    }
    bp.bg_want = Some(id);
    let platform = bp.install.as_ref().filter(|(game, _, _, _)| *game == id).map(|choice| choice.3).unwrap_or("windows");
    let plan = match &bp.install_plan_cache {
        Some((cid, cplat, p)) if *cid == id && *cplat == platform => p.clone(),
        _ => {
            let Some(p) = with_library(f.app, f.actions, |lib, cx| lib.install_plan(cx, id, platform)) else { return 0.0 };
            bp.install_plan_cache = Some((id, platform, p.clone()));
            p
        }
    };
    if bp.install.as_ref().is_none_or(|(g, _, _, _)| *g != id) {
        bp.install = Some((id, plan.default.clone(), true, platform));
    }
    if f.x {
        if let Some(choice) = bp.install.as_mut() {
            choice.2 = !choice.2;
        }
    }
    let (_, chosen, shortcut, platform) = bp.install.clone().expect("just set");
    let name = f.app.state.app_name(id).unwrap_or_default().to_string();
    let left = f.view.left() + f.pad;
    let mut y = top + 20.0 * s;
    let header = Rect::from_min_size(pos2(left, y), vec2(560.0 * s, 262.0 * s));
    paint::shadow(ui.painter(), header, 14.0 * s, s, 0.8, Color32::from_black_alpha(220));
    if !w::paint_cover(ui, header, &art::header(id), paint::radius(14.0 * s)) {
        paint::skeleton(ui, &f.t, header, paint::radius(14.0 * s));
    }
    let p = ui.painter().clone();
    let tx = header.right() + 48.0 * s;
    let tw = f.view.right() - f.pad - tx;
    paint::wrapped(&p, pos2(tx, y), &t!("Install {game}", game = name), w::font_bold(40.0 * s), f.t.text, tw, 2);
    let free = plan.libraries.iter().find(|(l, _)| *l == chosen).map(|(_, f)| *f).unwrap_or(0);
    let enough = plan.size == 0 || free == 0 || free >= plan.size;
    let facts = [
        (t!("SIZE ON DISK"), if plan.size > 0 { ui::bytes(plan.size) } else { t!("Unknown").to_string() }),
        (t!("FREE SPACE THERE"), if free > 0 { ui::bytes(free) } else { "—".to_string() }),
    ];
    let mut fx = tx;
    for (label, value) in facts {
        p.text(pos2(fx, y + 120.0 * s), Align2::LEFT_TOP, label, w::font_bold(15.0 * s), f.t.text_faint);
        let r = p.text(pos2(fx, y + 146.0 * s), Align2::LEFT_TOP, value, w::font_semibold(28.0 * s), f.t.text);
        fx = r.right().max(fx + 200.0 * s) + 60.0 * s;
    }
    if !enough {
        p.text(pos2(tx, y + 200.0 * s), Align2::LEFT_TOP, t!("There isn't enough free space on this drive."), w::font_semibold(19.0 * s), f.t.danger);
    }
    y = header.bottom() + 50.0 * s;

    if plan.choose_platform {
        let version = if platform == "linux" { t!("Linux (native)") } else { t!("Windows (runs with Proton)") };
        let (_, hit) = button(bp, ui, f, Id::unique("bp-install-platform"), pos2(left, y), 0.0, Some(icons::CONTROLLER), version, Kind::Secondary, None);
        if hit.activated {
            if let Some(choice) = bp.install.as_mut() {
                choice.3 = if platform == "linux" { "windows" } else { "linux" };
            }
        }
        y += 86.0 * s;
    }
    heading(ui, f, &mut y, t!("Install location"));
    for (k, (lib, free)) in plan.libraries.iter().enumerate() {
        let label = format!("{}   ·   {}", lib.display(), t!("{free} free", free = ui::bytes(*free)));
        let on = *lib == chosen;
        let (_, hit) = button(bp, ui, f, Id::unique(("bp-install-lib", k)), pos2(left, y), 900.0 * s, Some(if on { icons::CHECK } else { icons::DRIVES }), &label, if on { Kind::Primary } else { Kind::Secondary }, None);
        if hit.activated {
            if let Some(choice) = bp.install.as_mut() {
                choice.1 = lib.clone();
            }
        }
        y += 72.0 * s + 14.0 * s;
    }
    y += 20.0 * s;
    let label = if shortcut { t!("Desktop shortcut: on") } else { t!("Desktop shortcut: off") };
    let (_, hit) = button(bp, ui, f, Id::unique("bp-install-shortcut"), pos2(left, y), 0.0, Some(icons::DESKTOP), label, Kind::Secondary, None);
    if hit.activated {
        if let Some(choice) = bp.install.as_mut() {
            choice.2 = !choice.2;
        }
    }
    y += 72.0 * s + 40.0 * s;
    let go_id = Id::unique(("bp-install-go", id));
    bp.preferred = Some(go_id);
    let (r, go) = button(bp, ui, f, go_id, pos2(left, y), 280.0 * s, Some(icons::DOWNLOAD), t!("Install"), if enough { Kind::Play } else { Kind::Muted }, None);
    if go.activated && enough {
        // Revalidate free space on the chosen library right before starting the install.
        let fresh_free = local::manage::available_space(&chosen).unwrap_or(free);
        if plan.size == 0 || fresh_free == 0 || fresh_free >= plan.size {
            with_library(f.app, f.actions, |lib, cx| lib.install_to(cx, id, chosen.clone(), shortcut, platform));
            bp.install = None;
            bp.install_plan_cache = None;
            bp.back(f.now);
        } else {
            bp.install_plan_cache = None;
            f.actions.push(Action::Toast(t!("There isn't enough free space on this drive.").into()));
        }
    }
    let (_, cancel) = button(bp, ui, f, Id::unique(("bp-install-cancel", id)), pos2(r.right() + 20.0 * s, y), 0.0, Some(icons::X), t!("Cancel"), Kind::Secondary, None);
    if cancel.activated {
        bp.install = None;
        bp.back(f.now);
    }
    y += 72.0 * s;
    y - top + 40.0 * s
}

pub fn shot_count(app: &mut SteamApp, actions: &mut Vec<Action>, id: AppId) -> usize {
    with_store(app, actions, |st, cx| st.details_for(cx, id).map(|d| d.screenshots.len())).flatten().unwrap_or(0)
}

/// One screenshot over the whole page; left/right flip through them.
fn shots(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, id: AppId, i: usize) -> f32 {
    let s = f.s;
    bp.bg_want = Some(id);
    let list = with_store(f.app, f.actions, |st, cx| st.details_for(cx, id).map(|d| d.screenshots.clone())).flatten().unwrap_or_default();
    let area = f.view.shrink2(vec2(f.pad, 20.0 * s));
    let hit = bp.focus.item(ui, Id::unique("bp-shot-view"), area, ui.clip_rect(), Place::Page);
    // The whole page is the viewer: a focus box around it only gets in the way.
    bp.focus.hide_ring();
    let n = list.len();
    // The mouse flips too: the wheel (either way), the arrows, or a click
    // on the picture (like A) for the next one.
    let mut step = 0i32;
    if hit.activated {
        step = 1;
    }
    if ui.rect_contains_pointer(area) {
        let d = ui.input(|i| i.smooth_scroll_delta);
        bp.shot_wheel += d.y + d.x;
    }
    if f.now - bp.shot_since.0 < 0.3 {
        bp.shot_wheel = 0.0;
    } else if bp.shot_wheel.abs() > 30.0 {
        step = if bp.shot_wheel < 0.0 { 1 } else { -1 };
        bp.shot_wheel = 0.0;
    }
    for (way, icon) in [(-1i32, icons::CARET_LEFT), (1, icons::CARET_RIGHT)] {
        let c = pos2(area.center().x + way as f32 * (area.width() / 2.0 - 30.0 * s), area.center().y);
        let r = Rect::from_center_size(c, vec2(60.0 * s, 60.0 * s));
        let resp = ui.interact(r, Id::unique(("bp-shot-arrow", way)), egui::Sense::click());
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
        ui.painter().circle_filled(c, 30.0 * s * (1.0 + 0.12 * hover), Color32::from_black_alpha((140.0 + 80.0 * hover) as u8));
        ui.painter().text(c, Align2::CENTER_CENTER, icon, w::font(30.0 * s), f.t.text);
        if resp.clicked() {
            step = way;
        }
    }
    if step != 0 && n > 0 {
        let next = (i as i32 + step).rem_euclid(n as i32) as usize;
        if let Some(Page::Shots(_, at)) = bp.stack.last_mut() {
            *at = next;
        }
        bp.shot_since = (f.now, step as f32);
        bp.shot_wheel = 0.0;
    }
    let Some(shot) = list.get(i) else {
        paint::skeleton(ui, &f.t, area, paint::radius(12.0 * s));
        return f.view.height();
    };
    let (at, way) = bp.shot_since;
    let k = since(ui, at, 0.3);
    let r = area.translate(vec2((1.0 - k) * 120.0 * s * way, 0.0));
    let c = paint::faded(ui, area.expand(200.0 * s), k);
    // The thumbnail first (it's already loaded), then the full picture over it.
    let fit = |c: &Ui, url: &str| w::paint_contain(c, r, url, Align2::CENTER_CENTER);
    if !fit(&c, &shot.full) && !fit(&c, &shot.thumb) {
        paint::skeleton(ui, &f.t, area, paint::radius(12.0 * s));
    }
    let label = format!("{} / {}", i + 1, list.len());
    let pill = Rect::from_center_size(pos2(area.center().x, area.bottom() - 24.0 * s), vec2(130.0 * s, 44.0 * s));
    ui.painter().rect_filled(pill, paint::radius(22.0 * s), Color32::from_black_alpha(170));
    ui.painter().text(pill.center(), Align2::CENTER_CENTER, label, w::font_semibold(20.0 * s), f.t.text);
    f.view.height()
}

// ---------- Search with the shared on-screen keyboard ----------

fn search(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32) -> f32 {
    let s = f.s;
    let mut y = top + 16.0 * s;
    let left = f.view.left() + f.pad;
    let width = f.view.width() - 2.0 * f.pad;
    // X deletes, Y types a space (on a controller).
    if f.x && bp.query.pop().is_some() {
        bp.query_changed = f.now;
    }
    if f.y {
        bp.query.push(' ');
        bp.query_changed = f.now;
    }
    // Search a moment after typing stops.
    let q = bp.query.trim().to_string();
    if q != bp.submitted && f.now - bp.query_changed > 0.6 && (q.chars().count() >= 2 || q.is_empty()) {
        bp.submitted = q;
    }

    // The text box.
    let field = Rect::from_min_size(pos2(left, y), vec2(width.min(1100.0 * s), 80.0 * s));
    let p = ui.painter().clone();
    p.rect_filled(field, paint::radius(40.0 * s), Color32::from_black_alpha(120));
    p.rect_stroke(field, paint::radius(40.0 * s), egui::Stroke::new(2.0 * s, f.t.accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
    p.text(pos2(field.left() + 34.0 * s, field.center().y), Align2::LEFT_CENTER, icons::SEARCH, w::font(28.0 * s), f.t.text_dim);
    let tx = field.left() + 80.0 * s;
    let typed = if bp.query.is_empty() {
        p.text(pos2(tx, field.center().y), Align2::LEFT_CENTER, t!("Type a game's name"), w::font(28.0 * s), f.t.text_faint);
        Rect::from_min_size(pos2(tx, field.center().y), Vec2::ZERO)
    } else {
        // Long text: the end stays in view (the start slides out to the left).
        let room = field.right() - tx - 40.0 * s;
        let galley = p.layout_no_wrap(bp.query.clone(), w::font_semibold(30.0 * s), f.t.text);
        let x = tx - (galley.size().x - room).max(0.0);
        let rect = Align2::LEFT_CENTER.anchor_size(pos2(x, field.center().y), galley.size());
        p.with_clip_rect(Rect::from_min_max(pos2(tx, field.top()), pos2(tx + room + 8.0 * s, field.bottom()))).galley(rect.min, galley, f.t.text);
        rect
    };
    let blink = if ((f.now * 2.0) as u64).is_multiple_of(2) { 1.0 } else { 0.25 };
    p.vline(typed.right() + 4.0 * s, (field.center().y - 18.0 * s)..=(field.center().y + 18.0 * s), egui::Stroke::new(3.0 * s, f.t.accent.gamma_multiply(blink)));
    y += 80.0 * s + 30.0 * s;

    // The shared on-screen keyboard.
    match super::osk::draw(bp, ui, f, left, width, &mut y, t!("Search")) {
        Some(super::osk::Key::Char(ch)) => {
            bp.query.push(ch);
            bp.query_changed = f.now;
        }
        Some(super::osk::Key::Del) => {
            bp.query.pop();
            bp.query_changed = f.now;
        }
        Some(super::osk::Key::Clear) => {
            bp.query.clear();
            bp.submitted.clear();
            bp.query_changed = f.now;
        }
        Some(super::osk::Key::Go) => bp.submitted = bp.query.trim().to_string(),
        None => {}
    }
    y += 32.0 * s;
    bp.preferred = Some(Id::unique(("bp-key", 1, 0)));

    // Results (semantic AppId focus IDs).
    if bp.submitted.is_empty() {
        return y - top;
    }
    let query = bp.submitted.clone();
    heading(ui, f, &mut y, &t!("Results for “{q}”", q = query));
    match with_store(f.app, f.actions, |st, cx| st.search_for(cx, &query)).flatten() {
        None => {
            for i in 0..4 {
                let r = Rect::from_min_size(pos2(left + i as f32 * 446.0 * s, y), vec2(420.0 * s, 196.0 * s));
                paint::skeleton(ui, &f.t, r, paint::radius(10.0 * s));
            }
            y += 196.0 * s;
        }
        Some(Err(e)) => {
            p.text(pos2(left, y), Align2::LEFT_TOP, i18n::tr_dyn(&e), w::font(22.0 * s), f.t.danger);
            y += 40.0 * s;
        }
        Some(Ok(items)) if items.is_empty() => {
            p.text(pos2(left, y), Align2::LEFT_TOP, t!("No games found"), w::font(22.0 * s), f.t.text_dim);
            y += 40.0 * s;
        }
        Some(Ok(items)) => {
            let ids: Vec<AppId> = items.iter().map(|i| i.app_id).collect();
            let shown: std::collections::HashMap<AppId, Option<bool>> = with_store(f.app, f.actions, |st, _| ids.iter().map(|id| (*id, st.shown(*id))).collect()).unwrap_or_default();
            let items: Vec<&store::Item> = items.iter().filter(|i| shown.get(&i.app_id).copied().flatten() != Some(false)).collect();
            let key = format!("results-{query}");
            let tiles = shelf(bp, ui, f, &mut y, &key, items.len(), vec2(420.0 * s, 196.0 * s), |i| Id::unique(("bp-result", items[i].app_id)));
            for (i, r, hit) in tiles {
                store_tile(bp, ui, f, r, &hit, i, items[i], shown.get(&items[i].app_id).copied().flatten(), false);
            }
            y += 90.0 * s;
        }
    }
    y - top + 20.0 * s
}

// ---------- Downloads ----------

fn with_downloads<R>(app: &mut SteamApp, actions: &mut Vec<Action>, f: impl FnOnce(&mut crate::builtin::downloads::Downloads, &mut extensions::Ctx) -> R) -> Option<R> {
    let env = extensions::Env { state: &app.state, local: &app.local, theme: &app.prefs.theme, zoom: app.prefs.zoom, client: &app.client, actions };
    app.registry.with_ext::<crate::builtin::downloads::Downloads, R>("downloads", env, f)
}

fn downloads(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32) -> f32 {
    let s = f.s;
    let mut y = top + 16.0 * s;
    let left = f.view.left() + f.pad;
    let width = f.view.width() - 2.0 * f.pad;
    let mut active: Vec<model::DownloadStatus> = f.app.state.downloads.values().filter(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)).cloned().collect();
    active.sort_by_key(|d| d.app_id);
    let mut completed: Vec<model::DownloadStatus> = f.app.state.downloads.values().filter(|d| matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)).cloned().collect();
    completed.sort_by_key(|d| d.app_id);
    let queue = f.app.state.download_queue.clone();
    let updates = with_library(f.app, f.actions, |lib, cx| lib.games_needing_update(cx)).unwrap_or_default();

    if active.is_empty() && queue.is_empty() && updates.is_empty() && completed.is_empty() {
        if empty(bp, ui, f, y, icons::DOWNLOAD, t!("Nothing is downloading"), t!("Games you install or update show up here."), Some((icons::BOOKS, t!("Go to your library")))) == Some(true) {
            bp.switch_tab(Tab::Library, f.now, true);
        }
        return 500.0 * s;
    }

    let p = ui.painter().clone();
    for (n, d) in active.iter().enumerate() {
        let id = d.app_id;
        let card = Rect::from_min_size(pos2(left, y), vec2(width, 210.0 * s));
        let (k, dy) = appear(ui, bp, n);
        let card = card.translate(vec2(0.0, dy * s));
        let c = paint::faded(ui, card.expand(30.0 * s), k);
        c.painter().rect_filled(card, paint::radius(18.0 * s), Color32::from_black_alpha(110));
        let art_w = (360.0 * s).min(width * 0.28);
        let art_r = Rect::from_min_size(card.min + vec2(18.0 * s, 18.0 * s), vec2(art_w, 174.0 * s));
        if !w::paint_cover(&c, art_r, &art::header(id), paint::radius(12.0 * s)) {
            paint::skeleton(&c, &f.t, art_r, paint::radius(12.0 * s));
        }
        let tx = art_r.right() + 28.0 * s;
        let right_reserve = 420.0 * s;
        let text_w = (card.right() - tx - right_reserve).max(160.0 * s);
        let name = f.app.state.app_name(id).unwrap_or_default().to_string();
        paint::line(c.painter(), pos2(tx, art_r.top() + 4.0 * s), Align2::LEFT_TOP, &name, w::font_bold(28.0 * s), f.t.text, text_w);
        let status = if d.paused { t!("Paused").to_string() } else { i18n::tr_dyn(d.activity()) };
        c.painter().text(pos2(tx, art_r.top() + 48.0 * s), Align2::LEFT_TOP, status, w::font_medium(20.0 * s), if d.paused { f.t.away } else { f.t.accent });
        let (done, total) = d.shown();
        let bar = Rect::from_min_size(pos2(tx, art_r.top() + 94.0 * s), vec2(text_w, 12.0 * s));
        paint::bar(&c, &f.t, bar, d.fraction(), !d.paused);
        let net_rate = with_downloads(f.app, f.actions, |dl, _| dl.tv_rate(d.speed))
            .unwrap_or_else(|| format!("{}/s", ui::bytes(d.speed)));
        let mut line = format!("{} / {}", ui::bytes(done), ui::bytes(total));
        if !d.paused && d.speed > 0 {
            let left_bytes = d.download_total.saturating_sub(d.downloaded);
            let eta = ui::playtime(left_bytes / d.speed / 60 + 1);
            line = format!("{line}  ·  {net_rate}  ·  {}", t!("{time} left", time = eta));
        }
        if d.disk_speed > 0 && !d.paused {
            line = format!("{line}  ·  {}", t!("disk {rate}", rate = format!("{}/s", ui::bytes(d.disk_speed))));
        }
        paint::line(c.painter(), pos2(tx, bar.bottom() + 14.0 * s), Align2::LEFT_TOP, &line, w::font(18.0 * s), f.t.text_dim, text_w);
        // Buttons on the right.
        let mut bx = card.right() - 26.0 * s;
        let (r, cancel) = {
            let label = t!("Cancel");
            let wdt = paint::pill_width(ui, true, label, s);
            bx -= wdt;
            button(bp, &c, f, Id::unique(("bp-dl-cancel", id)), pos2(bx, card.center().y - 36.0 * s), wdt, Some(icons::X), label, Kind::Secondary, None)
        };
        if !d.verifying() {
            let (label, icon) = if d.paused { (t!("Resume"), icons::PLAY) } else { (t!("Pause"), icons::STOP) };
            let wdt = paint::pill_width(ui, true, label, s).max(170.0 * s);
            let at = pos2(r.left() - 14.0 * s - wdt, card.center().y - 36.0 * s);
            let (_, hit) = button(bp, &c, f, Id::unique(("bp-dl-pause", id)), at, wdt, Some(icon), label, Kind::Primary, None);
            if n == 0 {
                bp.preferred = Some(Id::unique(("bp-dl-pause", id)));
            }
            if hit.activated {
                f.app.client.send(if d.paused { Command::ResumeDownload { app_id: id } } else { Command::PauseDownload { app_id: id } });
            }
        }
        if cancel.activated {
            f.app.client.send(if d.verifying() { Command::CancelVerify { app_id: id } } else { Command::CancelDownload { app_id: id } });
        }
        bp.bg_want = bp.bg_want.or(Some(id));
        y += 210.0 * s + 22.0 * s;
    }

    // Queued downloads with controller-friendly Move Up / Move Down / Remove!
    if !queue.is_empty() {
        y += 14.0 * s;
        heading(ui, f, &mut y, t!("Up next"));
        for (n, id) in queue.iter().enumerate() {
            let row = Rect::from_min_size(pos2(left, y), vec2(width, 116.0 * s));
            p.rect_filled(row, paint::radius(14.0 * s), Color32::from_black_alpha(80));
            let art_r = Rect::from_min_size(row.min + vec2(14.0 * s, 12.0 * s), vec2(198.0 * s, 92.0 * s));
            if !w::paint_cover(ui, art_r, &art::header(*id), paint::radius(10.0 * s)) {
                paint::skeleton(ui, &f.t, art_r, paint::radius(10.0 * s));
            }
            let name = f.app.state.app_name(*id).unwrap_or_default().to_string();
            paint::line(&p, pos2(art_r.right() + 24.0 * s, row.center().y - 4.0 * s), Align2::LEFT_BOTTOM, &name, w::font_semibold(24.0 * s), f.t.text, (width - 660.0 * s).max(160.0 * s));
            p.text(pos2(art_r.right() + 24.0 * s, row.center().y + 4.0 * s), Align2::LEFT_TOP, t!("Queued · position {n}", n = n + 1), w::font(18.0 * s), f.t.text_dim);

            let mut bx = row.right() - 20.0 * s;
            let rm_label = t!("Remove");
            let rm_w = paint::pill_width(ui, true, rm_label, s);
            bx -= rm_w;
            let (_, rm_hit) = button(bp, ui, f, Id::unique(("bp-q-remove", *id)), pos2(bx, row.center().y - 36.0 * s), rm_w, Some(icons::TRASH), rm_label, Kind::Secondary, None);
            if rm_hit.activated {
                f.app.client.send(Command::CancelDownload { app_id: *id });
            }
            if n + 1 < queue.len() {
                let dn_w = paint::pill_width(ui, true, "", s).max(84.0 * s);
                bx -= dn_w + 10.0 * s;
                let (_, dn_hit) = button(bp, ui, f, Id::unique(("bp-q-down", *id)), pos2(bx, row.center().y - 36.0 * s), dn_w, Some(icons::CARET_DOWN), "", Kind::Secondary, None);
                if dn_hit.activated {
                    let mut next = queue.clone();
                    next.swap(n, n + 1);
                    f.app.client.send(Command::ReorderDownloads(next));
                }
            }
            if n > 0 {
                let up_w = paint::pill_width(ui, true, "", s).max(84.0 * s);
                bx -= up_w + 10.0 * s;
                let (_, up_hit) = button(bp, ui, f, Id::unique(("bp-q-up", *id)), pos2(bx, row.center().y - 36.0 * s), up_w, Some(icons::CARET_UP), "", Kind::Secondary, None);
                if up_hit.activated {
                    let mut next = queue.clone();
                    next.swap(n - 1, n);
                    f.app.client.send(Command::ReorderDownloads(next));
                }
            }
            y += 116.0 * s + 14.0 * s;
        }
    }

    // Available game updates (matching desktop Downloads).
    if !updates.is_empty() {
        y += 18.0 * s;
        heading(ui, f, &mut y, &t!("Updates available ({n})", n = updates.len()));
        if updates.len() > 1 {
            let (_, all_hit) = button(bp, ui, f, Id::unique("bp-dl-update-all"), pos2(left, y), 0.0, Some(icons::DOWNLOAD), t!("Update all"), Kind::Primary, None);
            if all_hit.activated {
                for (uid, _, _, lib_path) in &updates {
                    let lib_path = lib_path.clone();
                    with_library(f.app, f.actions, |lib, cx| lib.queue_update_game(cx, *uid, lib_path));
                }
            }
            y += 88.0 * s;
        }
        for (uid, name, size, lib_path) in &updates {
            let row = Rect::from_min_size(pos2(left, y), vec2(width, 108.0 * s));
            p.rect_filled(row, paint::radius(14.0 * s), Color32::from_black_alpha(75));
            let art_r = Rect::from_min_size(row.min + vec2(14.0 * s, 10.0 * s), vec2(188.0 * s, 88.0 * s));
            if !w::paint_cover(ui, art_r, &art::header(*uid), paint::radius(10.0 * s)) {
                paint::skeleton(ui, &f.t, art_r, paint::radius(10.0 * s));
            }
            paint::line(&p, pos2(art_r.right() + 24.0 * s, row.center().y - 4.0 * s), Align2::LEFT_BOTTOM, name, w::font_semibold(23.0 * s), f.t.text, (width - 520.0 * s).max(160.0 * s));
            p.text(pos2(art_r.right() + 24.0 * s, row.center().y + 4.0 * s), Align2::LEFT_TOP, t!("Update required · {size}", size = ui::bytes(*size)), w::font(18.0 * s), f.t.away);
            let upd_label = t!("Update");
            let upd_w = paint::pill_width(ui, true, upd_label, s);
            let (_, hit) = button(bp, ui, f, Id::unique(("bp-upd-go", *uid)), pos2(row.right() - 22.0 * s - upd_w, row.center().y - 36.0 * s), upd_w, Some(icons::DOWNLOAD), upd_label, Kind::Primary, None);
            if hit.activated {
                let lib_path = lib_path.clone();
                with_library(f.app, f.actions, |lib, cx| lib.queue_update_game(cx, *uid, lib_path));
            }
            y += 108.0 * s + 12.0 * s;
        }
    }

    // Completed / failed transfers this session.
    if !completed.is_empty() {
        y += 18.0 * s;
        heading(ui, f, &mut y, t!("Completed"));
        for d in completed.iter().take(8) {
            let name = f.app.state.app_name(d.app_id).unwrap_or_default().to_string();
            let status = if matches!(d.phase, model::DownloadPhase::Failed) {
                t!("Failed")
            } else {
                t!("Completed")
            };
            let icon = if matches!(d.phase, model::DownloadPhase::Failed) {
                icons::WARNING
            } else {
                icons::CHECK
            };
            let rect = Rect::from_min_size(pos2(left, y), vec2(width, 62.0 * s));
            if compact_row(bp, ui, f, Id::unique(("bp-dl-done", d.app_id)), rect, icon, &name, status).activated {
                open_game(bp, f, d.app_id);
            }
            y += 70.0 * s;
        }
    }
    y - top + 30.0 * s
}

// ---------- Friends ----------

fn friends(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32) -> f32 {
    let s = f.s;
    let mut y = top + 16.0 * s;
    let left = f.view.left() + f.pad;
    let mut list = f.app.state.friend_list();
    let rank = |p: &model::Persona| {
        if p.in_game() {
            0
        } else {
            match p.state {
                PersonaState::Offline | PersonaState::Invisible => 3,
                PersonaState::Away | PersonaState::Snooze | PersonaState::Busy => 2,
                _ => 1,
            }
        }
    };
    list.sort_by_key(|p| (rank(p), p.display_name().to_lowercase()));
    let online = list.iter().filter(|p| rank(p) < 3).count();
    let p = ui.painter().clone();
    p.text(pos2(left, y), Align2::LEFT_TOP, t!("{n} of {total} online", n = online, total = list.len()), w::font_medium(22.0 * s), f.t.text_dim);
    y += 56.0 * s;
    if list.is_empty() {
        empty(bp, ui, f, y, icons::USERS, t!("No friends yet"), t!("Friends you add on Steam show up here."), None);
        y += 300.0 * s;
    }
    let width = f.view.width() - 2.0 * f.pad;
    let gap = 24.0 * s;
    let cols = ((width + gap) / (520.0 * s + gap)).floor().max(1.0) as usize;
    let card_w = (width - gap * (cols - 1) as f32) / cols as f32;
    let card_h = 132.0 * s;
    let clip = ui.clip_rect();
    for (k, fr) in list.iter().enumerate() {
        let (row, col) = (k / cols, k % cols);
        let r = Rect::from_min_size(pos2(left + col as f32 * (card_w + gap), y + row as f32 * (card_h + gap)), vec2(card_w, card_h));
        let hit = bp.focus.item(ui, Id::unique(("bp-friend", fr.steam_id)), r, clip, Place::Page);
        if hit.activated {
            bp.open(Page::Friend(fr.steam_id), f.now);
        }
        if r.bottom() < clip.top() || r.top() > clip.bottom() {
            continue;
        }
        let (a, dy) = appear(ui, bp, k.min(10));
        let r = r.translate(vec2(0.0, dy * s));
        let c = paint::faded(ui, r.expand(30.0 * s), a);
        let shown = paint::grow(r, 1.0 + 0.03 * hit.glow);
        let fill = w::mix(Color32::from_black_alpha(100), Color32::from_white_alpha(26), hit.glow);
        c.painter().rect_filled(shown, paint::radius(16.0 * s), fill);
        let color = crate::shell::status_color(&f.t, fr);
        let av = Rect::from_center_size(pos2(shown.left() + 70.0 * s, shown.center().y), vec2(84.0 * s, 84.0 * s));
        if !w::paint_cover(&c, av, &art::avatar_full(fr.avatar_hash.as_deref()), paint::radius(42.0 * s)) {
            c.painter().circle_filled(av.center(), 42.0 * s, f.t.raised);
        }
        c.painter().circle_stroke(av.center(), 45.0 * s, egui::Stroke::new(3.0 * s, color));
        let unread_n = f.app.state.unread.get(&fr.steam_id).copied().unwrap_or(0);
        let tx = av.right() + 26.0 * s;
        let tw = shown.right() - tx - if unread_n > 0 { 68.0 * s } else { 20.0 * s };
        paint::line(c.painter(), pos2(tx, shown.center().y - 4.0 * s), Align2::LEFT_BOTTOM, &fr.display_name(), w::font_semibold(26.0 * s), if rank(fr) == 3 { f.t.text_dim } else { f.t.text }, tw);
        paint::line(c.painter(), pos2(tx, shown.center().y + 4.0 * s), Align2::LEFT_TOP, &crate::shell::status_text(&f.app.state, fr), w::font(19.0 * s), color, tw);
        if unread_n > 0 {
            paint::badge(c.painter(), pos2(shown.right() - 18.0 * s, shown.center().y), Align2::RIGHT_CENTER, &unread_n.min(99).to_string(), s, f.t.accent, f.t.on_accent);
        }
        if hit.focused {
            bp.focus.ring_around(shown, 16.0 * s);
            if let Some(g) = fr.game_app_id {
                bp.bg_want = Some(g);
            }
        }
    }
    let rows = list.len().div_ceil(cols);
    y += rows as f32 * (card_h + gap);
    let groups: Vec<_> = f.app.state.group_list().into_iter().map(|g| (g.id, f.app.state.group_display_name(g))).collect();
    if !groups.is_empty() {
        y += 16.0 * s;
        heading(ui, f, &mut y, t!("Group chats"));
        for (id, label) in groups {
            let (_, hit) = button(bp, ui, f, Id::unique(("bp-group", id)), pos2(left, y), 560.0 * s, Some(icons::USERS), &label, Kind::Secondary, None);
            if hit.activated {
                bp.open(Page::Group(id), f.now);
            }
            y += 86.0 * s;
        }
    }
    y - top + 20.0 * s
}

fn group_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: model::GroupChatId) -> f32 {
    let Some(group) = f.app.state.chat_groups.get(&id) else { return 0.0 };
    let name = f.app.state.group_display_name(group);
    let channels: Vec<_> = group.text_channels().map(|c| (c.id, c.name.clone())).collect();
    let s = f.s;
    let left = f.view.left() + f.pad;
    let mut y = top + 16.0 * s;
    heading(ui, f, &mut y, &name);
    for (room, label) in channels {
        let (_, hit) = button(bp, ui, f, Id::unique(("bp-group-room", id, room)), pos2(left, y), 560.0 * s, Some(icons::CHAT), &label, Kind::Secondary, None);
        if hit.activated {
            bp.open(Page::Chat(ChatTarget::Group(id, room)), f.now);
        }
        y += 86.0 * s;
    }
    y - top
}

/// A friend: who they are, what they play, and what you can do together.
fn friend_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, id: model::SteamId) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let mut y = top + 24.0 * s;
    let fr = f.app.state.persona(id);
    let color = crate::shell::status_color(&f.t, &fr);
    let p = ui.painter().clone();
    let av = Rect::from_min_size(pos2(left, y), vec2(184.0 * s, 184.0 * s));
    if !w::paint_cover(ui, av, &art::avatar_full(fr.avatar_hash.as_deref()), paint::radius(92.0 * s)) {
        p.circle_filled(av.center(), 92.0 * s, f.t.raised);
    }
    p.circle_stroke(av.center(), 96.0 * s, egui::Stroke::new(4.0 * s, color));
    let tx = av.right() + 44.0 * s;
    let tw = f.view.right() - f.pad - tx;
    paint::line(&p, pos2(tx, av.center().y - 10.0 * s), Align2::LEFT_BOTTOM, &fr.display_name(), w::font_bold(52.0 * s), f.t.text, tw);
    paint::line(&p, pos2(tx, av.center().y + 6.0 * s), Align2::LEFT_TOP, &crate::shell::status_text(&f.app.state, &fr), w::font_medium(26.0 * s), color, tw);
    if let Some(g) = fr.game_app_id.filter(|_| fr.in_game()) {
        bp.bg_want = Some(g);
    }
    y = av.bottom() + 56.0 * s;

    // What you can do, best first; the first is where focus starts.
    let mut x = left;
    let mut next = |bp: &mut BigPicture, ui: &mut Ui, f: &Frame, key: &str, icon: &str, label: &str, kind: Kind| {
        let (r, hit) = button(bp, ui, f, Id::unique(("bp-friend-act", key)), pos2(x, y), 240.0 * s, Some(icon), label, kind, None);
        x = r.right() + 20.0 * s;
        hit.activated
    };
    let game = fr.game_app_id.filter(|_| fr.in_game());
    let game_name = game.map(|g| f.app.state.app_name(g).map(String::from).unwrap_or_else(|| t!("the game").to_string()));
    if let (Some(g), Some(args)) = (game, fr.join_args()) {
        if next(bp, ui, f, "join", icons::PLAY, t!("Join game"), Kind::Play) {
            let name = game_name.clone().unwrap_or_default();
            match f.app.local.games.iter().find(|x| x.app_id == g) {
                Some(installed) => {
                    f.actions.push(Action::JoinGame { app_id: installed.app_id, connect: args });
                    f.actions.push(Action::Toast(t!("Joining {name} in {game}…", name = fr.display_name(), game = name)));
                }
                None => f.actions.push(Action::Toast(t!("Install {game} to join {name}.", game = name, name = fr.display_name()))),
            }
        }
    }
    if next(bp, ui, f, "chat", icons::CHAT, t!("Send a message"), if game.is_some() { Kind::Secondary } else { Kind::Primary }) {
        bp.open(Page::Chat(ChatTarget::Friend(id)), f.now);
    }
    for (i, (invite, label)) in crate::shell::invite_options(&f.app.state).into_iter().enumerate() {
        if next(bp, ui, f, &format!("invite{i}"), icons::USER_PLUS, &label, Kind::Secondary) {
            crate::shell::send_invite(f.app, &fr, invite, f.actions);
        }
    }
    if let Some(g) = game {
        if next(bp, ui, f, "game", icons::CONTROLLER, t!("Game details"), Kind::Secondary) {
            open_game(bp, f, g);
        }
    }
    if next(bp, ui, f, "profile", icons::USER_CIRCLE, t!("View profile"), Kind::Secondary) {
        bp.open(Page::Profile(id), f.now);
    }
    y + 112.0 * s - top
}

/// The chat's message history (scrolls with up/down while focused).
pub fn chat_history_id() -> Id {
    Id::unique("bp-chat-history")
}

/// The chat's message box (selecting it brings up the keyboard).
fn chat_input_id() -> Id {
    Id::unique("bp-chat-input")
}

/// A conversation over the whole page: the friend at the top, the history
/// filling the middle (it scrolls), the message box at the bottom. The
/// on-screen keyboard slides up while the message box is selected, like
/// Steam's; a real keyboard types any time (Enter sends).
fn chat_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, _top: f32, target: ChatTarget) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let right = f.view.right() - f.pad;
    let opened = bp.chat_for != Some(target);
    if opened {
        bp.chat_for = Some(target);
        bp.chat_follow = true;
    }
    let (name, status, avatar, color) = match target {
        ChatTarget::Friend(id) => {
            if opened && !f.app.demo {
                f.app.client.send(Command::LoadHistory { friend: id });
            }
            if f.app.state.unread.get(&id).is_some_and(|n| *n > 0) {
                f.actions.push(Action::MarkRead(id));
            }
            let friend = f.app.state.persona(id);
            let status = if f.app.state.is_typing(id) { t!("typing…").into() } else { crate::shell::status_text(&f.app.state, &friend) };
            (friend.display_name(), status, art::avatar_full(friend.avatar_hash.as_deref()), crate::shell::status_color(&f.t, &friend))
        }
        ChatTarget::Group(group_id, chat_id) => {
            let Some(group) = f.app.state.chat_groups.get(&group_id) else { return 0.0 };
            let Some(channel) = group.channel(chat_id).filter(|c| !c.voice_allowed) else { return 0.0 };
            if opened && !f.app.demo {
                f.app.client.send(Command::LoadGroupHistory { group_id, chat_id, before: None });
            }
            if channel.has_unread() && f.app.state.group_messages.get(&(group_id, chat_id)).is_some() {
                f.actions.push(Action::MarkGroupRead { group_id, chat_id });
            }
            (channel.name.clone(), f.app.state.group_display_name(group), group.avatar_url.clone().unwrap_or_default(), f.t.accent)
        }
    };
    let p = ui.painter().clone();

    // Who you're talking to.
    let head_y = f.view.top() + 6.0 * s;
    let av = Rect::from_min_size(pos2(left, head_y), vec2(64.0 * s, 64.0 * s));
    if !w::paint_cover(ui, av, &avatar, paint::radius(32.0 * s)) {
        p.circle_filled(av.center(), 32.0 * s, f.t.raised);
    }
    p.circle_stroke(av.center(), 34.0 * s, egui::Stroke::new(3.0 * s, color));
    let tx = av.right() + 22.0 * s;
    paint::line(&p, pos2(tx, av.center().y - 2.0 * s), Align2::LEFT_BOTTOM, &name, w::font_bold(32.0 * s), f.t.text, right - tx);
    paint::line(&p, pos2(tx, av.center().y + 4.0 * s), Align2::LEFT_TOP, &status, w::font_medium(20.0 * s), color, right - tx);
    let head_bottom = av.bottom() + 18.0 * s;

    // Opening the keyboard is an action. Pointer focus may change as it moves.
    let input_id = chat_input_id();
    let want_keys = bp.chat_keys;
    let k = super::focus::ease(ui.ctx().animate_bool_with_time(Id::unique(("bp-chat-keys", target)), want_keys, 0.25));
    let keys_max_w = (right - left).min(960.0 * s);
    let keys_h = super::osk::panel_height(s, keys_max_w, bp.osk);
    let panel_top = f.view.bottom() - keys_h * k;
    let mut send = std::mem::take(&mut bp.send_now);
    if k > 0.01 {
        let screen = ui.max_rect();
        let panel = Rect::from_min_max(pos2(screen.left(), panel_top), pos2(screen.right(), f.view.bottom() + keys_h));
        p.rect_filled(panel, 0.0, f.t.chrome.gamma_multiply(0.96));
        let edge = Rect::from_min_size(panel.min, vec2(panel.width(), 2.0 * s));
        w::gradient(&p, edge, f.t.accent.gamma_multiply(0.0), f.t.accent.gamma_multiply(0.8), true);
        let mut ky = panel_top + 22.0 * s;
        let mut keys_ui = paint::faded(ui, panel, k);
        if !want_keys {
            keys_ui.disable();
        }
        match super::osk::draw(bp, &mut keys_ui, f, f.view.center().x - keys_max_w / 2.0, keys_max_w, &mut ky, t!("Send")) {
            Some(super::osk::Key::Char(ch)) => bp.message.push(ch),
            Some(super::osk::Key::Del) => {
                bp.message.pop();
            }
            Some(super::osk::Key::Clear) => bp.message.clear(),
            Some(super::osk::Key::Go) => send = true,
            None => {}
        }
    }

    // The message box: grows to four lines, then shows the end of the text.
    if f.x {
        bp.message.pop();
    }
    if f.y {
        bp.message.push(' ');
    }
    let send_label = t!("Send");
    let send_w = paint::pill_width(ui, true, send_label, s);
    let field_w = right - left - send_w - 18.0 * s;
    let font = w::font_semibold(26.0 * s);
    let text_w = field_w - 64.0 * s;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(egui::text::LayoutJob::simple(bp.message.clone(), font.clone(), f.t.text, text_w)));
    let line_h = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let lines = (galley.size().y / line_h).round().clamp(1.0, 4.0);
    let field_h = (lines * line_h + 34.0 * s).max(72.0 * s);
    let bottom = panel_top - 20.0 * s;
    let field = Rect::from_min_max(pos2(left, bottom - field_h), pos2(left + field_w, bottom));
    let hit = bp.focus.item(ui, input_id, field, ui.clip_rect(), Place::Page);
    let r = (36.0 * s).min(field.height() / 2.0);
    p.rect_filled(field, paint::radius(r), Color32::from_black_alpha(130));
    let stroke = w::mix(f.t.accent.gamma_multiply(0.45), f.t.accent, hit.glow);
    p.rect_stroke(field, paint::radius(r), egui::Stroke::new((2.0 + hit.glow) * s, stroke), egui::StrokeKind::Inside);
    if hit.focused {
        bp.focus.ring_around(field, r);
    }
    let inner = field.shrink2(vec2(32.0 * s, 17.0 * s));
    let caret = if bp.message.is_empty() {
        p.text(pos2(inner.left(), inner.bottom() - line_h / 2.0), Align2::LEFT_CENTER, t!("Message {name}", name = name), w::font(26.0 * s), f.t.text_faint);
        Rect::from_min_size(pos2(inner.left(), inner.bottom() - line_h), vec2(0.0, line_h))
    } else {
        // The last lines stay in view (the start scrolls up out of the box).
        let at = pos2(inner.left(), inner.bottom() - galley.size().y);
        let end = galley.pos_from_cursor(egui::text::CCursor::new(bp.message.chars().count()));
        p.with_clip_rect(inner.expand2(vec2(4.0 * s, 0.0))).galley(at, galley, f.t.text);
        end.translate(at.to_vec2())
    };
    let blink = ((f.now * 3.0).sin() as f32 * 0.5 + 0.5).powf(0.6);
    p.vline(caret.right() + 3.0 * s, (caret.top() + 3.0 * s)..=(caret.bottom() - 3.0 * s), egui::Stroke::new(3.0 * s, f.t.accent.gamma_multiply(blink)));
    // A on the box: straight to the letters.
    if hit.activated {
        bp.chat_keys = true;
        if want_keys {
            bp.focus.current = Some(Id::unique(("bp-key", 1, 0)));
        }
    }
    let empty_box = bp.message.trim().is_empty();
    let (_, send_hit) = button(bp, ui, f, Id::unique(("bp-chat-send", target)), pos2(field.right() + 18.0 * s, bottom - 72.0 * s), send_w, Some(icons::SEND), send_label, if empty_box { Kind::Secondary } else { Kind::Primary }, None);
    if send_hit.activated {
        send = true;
    }
    bp.preferred = Some(input_id);
    let text = bp.message.trim().to_string();
    if send && !text.is_empty() {
        let command = match target {
            ChatTarget::Friend(to) => Command::SendMessage { to, text },
            ChatTarget::Group(group_id, chat_id) => Command::SendGroupMessage { group_id, chat_id, text },
        };
        f.app.client.send(command);
        bp.message.clear();
        bp.chat_follow = true;
    }

    // The history fills the rest and scrolls; newest at the bottom.
    let area = Rect::from_min_max(pos2(left, head_bottom), pos2(right, field.top() - 18.0 * s));
    if area.height() < 40.0 * s {
        return f.view.height();
    }
    let me = f.app.state.steam_id;
    let messages: Vec<_> = match target {
        ChatTarget::Friend(id) => f.app.state.chats.get(&id).into_iter().flatten()
            .map(|m| (m.from, m.time, m.text.clone())).collect(),
        ChatTarget::Group(group, room) => f.app.state.group_messages.get(&(group, room)).into_iter().flatten()
            .filter(|m| !m.deleted && !m.text.is_empty())
            .map(|m| {
                let name = f.app.state.persona(m.from).display_name();
                (m.from, m.time, format!("{name}\n{}", m.text))
            }).collect(),
    };
    let hist = bp.focus.item(ui, chat_history_id(), area, ui.clip_rect(), Place::Page);
    p.rect_filled(area, paint::radius(18.0 * s), Color32::from_black_alpha(90));
    if hist.focused {
        bp.focus.ring_around(area, 18.0 * s);
    }
    let pad = 22.0 * s;
    let wrap = (area.width() * 0.68).max(200.0 * s);
    let wrap_tenths = (wrap * 10.0).round() as i32;
    let font_tenths = (22.0 * s * 10.0).round() as i32;
    let msg_fingerprint = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        messages.len().hash(&mut h);
        for (from, time, text) in &messages {
            from.hash(&mut h);
            time.hash(&mut h);
            text.hash(&mut h);
        }
        h.finish()
    };
    let cache_valid = bp.chat_layout_cache.as_ref().is_some_and(|c| {
        c.target == target
            && c.fingerprint == msg_fingerprint
            && c.wrap_tenths == wrap_tenths
            && c.font_tenths == font_tenths
    });
    if !cache_valid {
        let body = w::font(22.0 * s);
        let mut laid = Vec::with_capacity(messages.len());
        let mut total = pad;
        for (from, time, text) in &messages {
            let job = egui::text::LayoutJob::simple(text.clone(), body.clone(), f.t.text, wrap);
            let g = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let size = g.size() + vec2(36.0 * s, 24.0 * s);
            laid.push((total, size, *from == me, g, *time));
            total += size.y + 30.0 * s;
        }
        total += pad - 8.0 * s;
        bp.chat_layout_cache = Some(super::ChatLayoutCache {
            target,
            fingerprint: msg_fingerprint,
            wrap_tenths,
            font_tenths,
            laid,
            total,
        });
    }
    let total = bp.chat_layout_cache.as_ref().unwrap().total;
    let view_h = area.height();
    let max = (total - view_h).max(0.0);
    let key = format!("chat-{target:?}");
    // The mouse wheel; up/down while the history is focused (see mod.rs).
    let wheel = if ui.rect_contains_pointer(area) { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
    let nudge = std::mem::take(&mut bp.chat_nudge);
    let mut follow = bp.chat_follow;
    let sc = bp.scroller(&key);
    if wheel != 0.0 {
        sc.target -= wheel;
        follow = false;
    }
    if nudge != 0.0 {
        sc.target += nudge * 180.0 * s;
        follow = false;
    }
    if follow {
        sc.target = max;
    }
    sc.clamp(total, view_h);
    // Back at the bottom: new messages keep it there again.
    if sc.target >= max - 1.0 {
        follow = true;
    }
    sc.step(ui);
    let pos = sc.pos;
    let edges = (sc.target <= 0.5, sc.target >= max - 0.5);
    bp.chat_follow = follow;
    bp.chat_edges = edges;
    let clip = p.with_clip_rect(area.shrink(2.0));
    if messages.is_empty() {
        p.text(area.center(), Align2::CENTER_CENTER, t!("No messages yet"), w::font(22.0 * s), f.t.text_faint);
    }
    // Borrow cache temporarily without cloning the entire layout vector.
    let cache = bp.chat_layout_cache.take();
    if let Some(cache) = cache {
        for ((at, size, mine, g, time), (_, _, text)) in cache.laid.iter().zip(&messages) {
            let (at, size, mine, time) = (*at, *size, *mine, *time);
            let y = area.top() + at - pos;
            if y > area.bottom() || y + size.y + 30.0 * s < area.top() {
                continue;
            }
            let bx = if mine { area.right() - pad - size.x } else { area.left() + pad };
            let bubble = Rect::from_min_size(pos2(bx, y), size);
            let link = text.split_whitespace().find_map(|word| {
                crate::builtin::hub::route(word.trim_matches(|ch| matches!(ch, '(' | ')' | '<' | '>' | ',' | '.')))
            });
            let message_id = Id::unique(("bp-message", target, time, at.to_bits()));
            let response = if let Some(link) = link {
                let hit = bp.focus.item(ui, message_id, bubble.intersect(area), area, Place::Page);
                if hit.focused {
                    bp.focus.ring_around(bubble.intersect(area), 16.0 * s);
                }
                if hit.activated {
                    let (page, arg) = link.to_page();
                    f.actions.push(Action::Navigate { page, arg });
                }
                hit.response
            } else {
                let visible = bubble.intersect(area);
                visible.is_positive().then(|| ui.interact(visible, message_id, egui::Sense::click()))
            };
            if let Some(response) = response {
                response.context_menu(|ui| {
                    if ui.button(t!("Copy")).clicked() {
                        ui.ctx().copy_text(text.clone());
                        ui.close();
                    }
                });
            }
            clip.rect_filled(bubble, paint::radius(16.0 * s), if mine { f.t.accent.gamma_multiply(0.55) } else { f.t.raised });
            clip.galley(bubble.min + vec2(18.0 * s, 12.0 * s), g.clone(), f.t.text);
            let (anchor, align) = if mine { (bubble.right_bottom() + vec2(-4.0 * s, 4.0 * s), Align2::RIGHT_TOP) } else { (bubble.left_bottom() + vec2(4.0 * s, 4.0 * s), Align2::LEFT_TOP) };
            clip.text(anchor, align, ui::ago(time), w::font(15.0 * s), f.t.text_faint);
        }
        bp.chat_layout_cache = Some(cache);
    }
    // Where you are in a long conversation.
    if max > 0.0 {
        let track = Rect::from_min_max(pos2(area.right() - 10.0 * s, area.top() + 14.0 * s), pos2(area.right() - 5.0 * s, area.bottom() - 14.0 * s));
        let h = (track.height() * view_h / total).max(30.0 * s);
        let y = track.top() + (track.height() - h) * (pos / max).clamp(0.0, 1.0);
        p.rect_filled(Rect::from_min_size(pos2(track.left(), y), vec2(track.width(), h)), paint::radius(3.0 * s), Color32::from_white_alpha(70));
    }
    f.view.height()
}

fn embedded(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, page: &str, arg: Option<&str>) -> f32 {
    let want = (page.to_owned(), arg.map(str::to_owned));
    if bp.embedded.as_ref() != Some(&want) {
        f.app.registry.navigate(page, arg);
        bp.embedded = Some(want);
    }
    let area = f.view.shrink2(vec2(f.pad * 0.5, 6.0 * f.s));
    ui.painter().rect_filled(area, paint::radius(14.0 * f.s), f.t.bg);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(area.intersect(ui.clip_rect()));
    let app = &mut *f.app;
    let env = extensions::Env { state: &app.state, local: &app.local, theme: &f.t, zoom: app.prefs.zoom, client: &app.client, actions: f.actions };
    if !app.registry.show_page(page, &mut child, env) {
        ui.painter().text(area.center(), Align2::CENTER_CENTER, t!("This page isn't available"), w::font(24.0 * f.s), f.t.text_dim);
    }
    f.view.height()
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod compatibility_tests {
    use super::compatibility_choices;

    #[test]
    fn a_native_only_install_cannot_select_a_windows_tool() {
        let builds = vec![("proton_10".into(), "Proton 10".into())];
        let choices = compatibility_choices(Some((true, false)), Some("Proton 10"), &builds);
        assert_eq!(choices.iter().map(|row| row.0).collect::<Vec<_>>(), [None, Some(client::NATIVE)]);
    }

    #[test]
    fn a_dual_install_can_reset_or_select_either_build() {
        let builds = vec![("proton_10".into(), "Proton 10".into())];
        let choices = compatibility_choices(Some((true, true)), Some("Proton 10"), &builds);
        assert_eq!(choices.iter().map(|row| row.0).collect::<Vec<_>>(), [
            None, Some(client::NATIVE), Some(client::DEFAULT_PROTON), Some("proton_10"),
        ]);
    }
}
