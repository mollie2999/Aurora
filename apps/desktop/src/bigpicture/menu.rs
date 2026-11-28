//! Big Picture's main and power menus.

use super::focus::{Dir, Focus, Hit, Place, Scroller, ease};
use super::input::Press;
use super::{BigPicture, Frame, Page, Tab, paint, quick_access};
use client::Command;
use egui::{Align2, Color32, Id, Rangef, Rect, Sense, Stroke, Ui, pos2, vec2};
use i18n::t;
use model::{AppId, DownloadPhase, DownloadStatus, Persona, PersonaState, SteamId};
use ui::{Theme, icons, widgets as w};

const OPEN_SECS: f32 = 0.28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Main,
    Power,
    ConfirmQuit,
    ConfirmStop(AppId),
    ConfirmSignOut,
}

impl View {
    fn parent(self) -> Option<Self> {
        match self {
            Self::Main => None,
            Self::Power | Self::ConfirmStop(_) => Some(Self::Main),
            Self::ConfirmQuit | Self::ConfirmSignOut => Some(Self::Power),
        }
    }

    fn default_item(self) -> Id {
        menu_id(match self {
            Self::Main => "home",
            Self::Power => "leave",
            _ => "cancel",
        })
    }
}

pub struct Menu {
    view: View,
    focus: Focus,
    scroll: Scroller,
    opened: f64,
    closing: Option<f64>,
    dir: Option<Dir>,
    press: bool,
    stick: f32,
}

#[derive(PartialEq, Eq)]
pub enum Outcome {
    None,
    LeaveBigPicture,
}

impl Menu {
    pub fn new(now: f64) -> Self {
        let mut focus = Focus::default();
        focus.current = Some(View::Main.default_item());
        Self {
            view: View::Main,
            focus,
            scroll: Scroller::default(),
            opened: now,
            closing: None,
            dir: None,
            press: false,
            stick: 0.0,
        }
    }

    fn set_view(&mut self, view: View) {
        self.view = view;
        self.focus = Focus::default();
        self.focus.current = Some(view.default_item());
        self.scroll = Scroller::default();
        self.dir = None;
        self.press = false;
        self.stick = 0.0;
    }

    pub(super) fn back_label(&self) -> &'static str {
        if self.view == View::Main { t!("Close") } else { t!("Back") }
    }

    pub fn input(&mut self, presses: &[(Press, bool)], now: f64) -> Outcome {
        if self.closing.is_some() {
            return Outcome::None;
        }
        for (p, _) in presses {
            match p {
                Press::Move(d) => self.dir = Some(*d),
                Press::A => self.press = true,
                Press::Scroll(delta) => self.stick += f32::from(*delta),
                Press::Guide if matches!(self.view, View::Main | View::Power) => {
                    return Outcome::LeaveBigPicture;
                }
                Press::B | Press::Guide | Press::Start => {
                    if let Some(parent) = self.view.parent() {
                        self.set_view(parent);
                    } else {
                        self.closing = Some(now);
                    }
                    return Outcome::None;
                }
                _ => {}
            }
        }
        Outcome::None
    }
}

fn menu_id(key: &str) -> Id {
    Id::unique(("bp-menu", key))
}

pub fn show(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, screen: Rect) {
    let Some(mut menu) = bp.menu.take() else {
        return;
    };
    let (t, s, now) = (f.t.clone(), f.s, f.now);
    if menu.closing.is_some_and(|at| now - at >= f64::from(OPEN_SECS)) {
        ui.ctx().request_repaint();
        return;
    }
    let e = drawer_extent(now, menu.opened, menu.closing, OPEN_SECS);
    if menu.closing.is_some() || now - menu.opened < f64::from(OPEN_SECS) {
        ui.ctx().request_repaint();
    }
    let width = (560.0 * s).min(screen.width() * 0.92);
    let (panel, outside) = drawer_panel(ui, &t, screen, width, s, e, "bp-menu");

    if outside && menu.closing.is_none() {
        if let Some(parent) = menu.view.parent().filter(|_| menu.view != View::Power) {
            menu.set_view(parent);
        } else {
            menu.closing = Some(now);
        }
        ui.ctx().request_repaint();
    }
    let pad = (28.0 * s).min(width * 0.12);
    let header_height = (106.0 * s).min(panel.height() * 0.4);
    let header = Rect::from_min_max(panel.min, pos2(panel.right(), panel.top() + header_height));
    ui.painter().hline(header.x_range(), header.bottom(), Stroke::new(1.0, t.surface_hover));

    let footer = menu_footer(panel, s, menu.view);
    let viewport = Rect::from_min_max(pos2(panel.left(), header.bottom()), footer.right_top());
    let body_clip = viewport.intersect(screen);
    let body_ui = clipped_child(ui, viewport, body_clip);
    let active = menu.closing.is_none();
    let top = viewport.top() - menu.scroll.pos;
    let dir = menu.dir.take().filter(|_| active);
    let press = std::mem::take(&mut menu.press) && active;
    menu.focus.begin_drawer(ui, dir, press, |_| false, menu.view == View::Main);
    let span = Rangef::new(panel.left() + pad, panel.right() - pad);
    let header_clicked = user_header(bp, &mut menu.focus, ui, f, header.shrink2(vec2(pad, 0.0)), header.intersect(screen), active);
    let mut rows = DrawerRows {
        ui: &body_ui,
        theme: &t,
        focus: &mut menu.focus,
        clip: body_clip,
        span,
        y: top + 16.0 * s,
        s,
        active,
        place: Place::Page,
        open_style: false,
    };
    let mut action = match menu.view {
        View::Main => main_rows(&mut rows, f),
        View::Power => power_rows(&mut rows, bp.is_windowed()),
        view => confirm_rows(&mut rows, f, view),
    };
    if header_clicked && action.is_none() && f.app.state.steam_id != 0 {
        action = Some(MenuAction::Page(Page::Profile(f.app.state.steam_id)));
    }
    let height = rows.y - top + 10.0 * s;
    if menu.view == View::Main && footer.is_positive() {
        let clip = footer.intersect(screen);
        let footer_ui = clipped_child(ui, footer, clip);
        footer_ui.painter().hline(footer.x_range(), footer.top(), Stroke::new(1.0, t.surface_hover));
        let mut footer_rows = DrawerRows {
            ui: &footer_ui, theme: &t, focus: &mut menu.focus, clip, span,
            y: footer.top() + 12.0 * s, s, active, place: Place::Bar, open_style: false,
        };
        if footer_exit(&mut footer_rows) {
            action = Some(MenuAction::Leave);
        }
    }
    let default = menu.view.default_item();
    menu.focus.end(&[default], default);
    scroll_body(
        &body_ui,
        &menu.focus,
        &mut menu.scroll,
        viewport,
        height,
        std::mem::take(&mut menu.stick) * 44.0 * s,
        active,
    );
    menu.focus.paint_ring(ui, &t, s, 10.0 * s, body_clip);

    match action {
        Some(MenuAction::View(view)) => {
            menu.set_view(view);
            ui.ctx().request_repaint();
        }
        Some(MenuAction::Tab(tab)) => {
            bp.switch_tab(tab, now, true);
            menu.closing = Some(now);
        }
        Some(MenuAction::Page(page)) => {
            bp.open(page, now);
            menu.closing = Some(now);
        }
        Some(MenuAction::QuickAccess) => {
            bp.open_quick_access(ui.ctx(), quick_access::Section::System, now);
            return;
        }
        Some(MenuAction::Fullscreen) => bp.toggle_fullscreen(ui.ctx()),
        Some(MenuAction::Leave) => {
            bp.leave(ui.ctx());
            ui.ctx().request_repaint();
            return;
        }
        Some(MenuAction::Stop(id)) => {
            if f.app.state.playing == Some(id) {
                f.app.client.send(Command::StopGame { app_id: id });
            }
            menu.set_view(View::Main);
            ui.ctx().request_repaint();
        }
        Some(MenuAction::SignOut) => {
            bp.leave(ui.ctx());
            if !f.app.demo {
                f.app.client.send(Command::LogOut);
            }
            ui.ctx().request_repaint();
            return;
        }
        Some(MenuAction::Quit) => {
            if menu.view == View::ConfirmQuit {
                f.app.exiting = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else if f.app.downloading() {
                menu.set_view(View::ConfirmQuit);
                ui.ctx().request_repaint();
            } else {
                f.app.request_exit(ui.ctx());
                menu.closing = Some(now);
            }
        }
        None => {}
    }
    if active && menu.closing.is_some() {
        ui.ctx().request_repaint();
    }
    bp.menu = Some(menu);
}

enum MenuAction {
    View(View),
    Tab(Tab),
    Page(Page),
    QuickAccess,
    Fullscreen,
    Leave,
    Stop(AppId),
    SignOut,
    Quit,
}

fn main_rows(rows: &mut DrawerRows<'_>, f: &Frame<'_>) -> Option<MenuAction> {
    let mut action = None;
    if let Some(id) = f.app.state.playing {
        running_game(rows, f, id);
        if rows.button(menu_id("game"), icons::PLAY, t!("Game details"), "", false) {
            action = Some(MenuAction::Page(Page::Game(id)));
        }
        if rows.button(menu_id("stop"), icons::STOP, t!("Stop game"), "", true) {
            action = Some(MenuAction::View(View::ConfirmStop(id)));
        }
        rows.separator();
    }
    let downloads = active_downloads(&f.app.state).count()
        + f.app
            .state
            .download_queue
            .iter()
            .filter(|id| !f.app.state.downloads.get(id).is_some_and(is_active_download))
            .count();
    let state = &f.app.state;
    let online = state
        .friends
        .iter()
        .filter(|(id, relation)| {
            **relation == model::Relationship::Friend && state.personas.get(id).is_some_and(friend_is_online)
        })
        .count();
    let downloads = (downloads > 0).then(|| downloads.to_string()).unwrap_or_default();
    let online = t!("{n} online", n = online);
    for (key, icon, tab, value) in [
        ("home", icons::HOUSE, Tab::Home, ""),
        ("library", icons::BOOKS, Tab::Library, ""),
        ("store", icons::STORE, Tab::Store, ""),
        ("downloads", icons::DOWNLOAD, Tab::Downloads, downloads.as_str()),
        ("friends", icons::USERS, Tab::Friends, online.as_str()),
    ] {
        if rows.button(menu_id(key), icon, tab.label(), value, false) {
            action = Some(MenuAction::Tab(tab));
        }
    }
    if f.app.state.steam_id != 0
        && rows.button(menu_id("profile"), icons::USER_CIRCLE, t!("Profile"), "", false)
    {
        action = Some(MenuAction::Page(Page::Profile(f.app.state.steam_id)));
    }
    if rows.button(menu_id("quick-access"), icons::SLIDERS, t!("Quick Access"), "", false) {
        action = Some(MenuAction::QuickAccess);
    }
    if rows.button(menu_id("settings"), icons::GEAR, t!("Settings"), "", false) {
        action = Some(MenuAction::Page(Page::Settings));
    }
    rows.separator();
    if rows.button(menu_id("power"), icons::POWER, t!("Power"), icons::CARET_RIGHT, false) {
        action = Some(MenuAction::View(View::Power));
    }
    action
}

fn menu_footer(panel: Rect, s: f32, view: View) -> Rect {
    let height = if view == View::Main { (74.0 * s).min(panel.height() * 0.5) } else { 0.0 };
    Rect::from_min_max(pos2(panel.left(), panel.bottom() - height), panel.max)
}

fn footer_exit(rows: &mut DrawerRows<'_>) -> bool {
    rows.button(menu_id("leave"), icons::DESKTOP, t!("Exit Big Picture"), "", false)
}

fn power_rows(rows: &mut DrawerRows<'_>, windowed: bool) -> Option<MenuAction> {
    rows.heading(t!("Power"));
    let mut action = None;
    let fullscreen = if windowed { t!("Off") } else { t!("On") };
    for (key, icon, label, danger, result) in [
        ("leave", icons::DESKTOP, t!("Exit Big Picture"), false, MenuAction::Leave),
        ("fullscreen", icons::MONITOR, t!("Fullscreen"), false, MenuAction::Fullscreen),
        ("signout", icons::SIGN_OUT, t!("Sign out of account"), true, MenuAction::View(View::ConfirmSignOut)),
        ("quit", icons::POWER, t!("Quit Aurora"), true, MenuAction::Quit),
        ("back", icons::BACK, t!("Back"), false, MenuAction::View(View::Main)),
    ] {
        let value = if key == "fullscreen" { fullscreen } else { "" };
        if rows.button(menu_id(key), icon, label, value, danger) {
            action = Some(result);
        }
    }
    action
}

fn confirm_rows(rows: &mut DrawerRows<'_>, f: &Frame<'_>, view: View) -> Option<MenuAction> {
    let (title, label, icon, action) = match view {
        View::ConfirmQuit => (t!("A download is in progress"), t!("Exit anyway"), icons::POWER, MenuAction::Quit),
        View::ConfirmStop(id) => (t!("Stop game"), t!("Stop game"), icons::STOP, MenuAction::Stop(id)),
        View::ConfirmSignOut => (t!("Sign out of account"), t!("Sign out"), icons::SIGN_OUT, MenuAction::SignOut),
        _ => unreachable!(),
    };
    rows.heading(title);
    match view {
        View::ConfirmQuit => {
            let game = active_downloads(&f.app.state)
                .min_by_key(|d| d.app_id)
                .map(|d| d.app_id)
                .or_else(|| f.app.state.download_queue.first().copied())
                .map(|id| game_name(&f.app.state, id))
                .unwrap_or_default();
            rows.note(&t!(
                "{game} is still downloading. If you exit, it continues the next time you open Aurora.",
                game = game
            ));
        }
        View::ConfirmStop(id) => rows.note(&game_name(&f.app.state, id)),
        _ => {}
    }
    let cancel = rows.button(menu_id("cancel"), icons::BACK, t!("Cancel"), "", false);
    let confirm = rows.button(menu_id("confirm"), icon, label, "", true);
    if cancel { view.parent().map(MenuAction::View) } else { confirm.then_some(action) }
}

fn user_header(bp: &BigPicture, focus: &mut Focus, ui: &Ui, f: &Frame<'_>, rect: Rect, clip: Rect, active: bool) -> bool {
    let s = f.s;
    let me = f.app.state.me();
    let color = me.map_or(f.t.text_dim, |p| crate::shell::status_color(&f.t, p));
    let bounds = rect.shrink2(vec2(0.0, (10.0 * s).min(rect.height() * 0.2)));
    let hit = (f.app.state.steam_id != 0).then(|| {
        focus.item(ui, menu_id("user-header"), bounds, if active { clip } else { Rect::NOTHING }, Place::Bar)
    });
    if let Some(hit) = &hit {
        if hit.glow > 0.0 {
            ui.painter().rect_filled(bounds, paint::radius(12.0 * s), f.t.surface_hover.gamma_multiply(hit.glow));
        }
        if hit.focused {
            focus.ring_around_clipped(bounds, 12.0 * s, clip);
        }
    }
    let av = Rect::from_center_size(pos2(rect.left() + 32.0 * s, rect.center().y), vec2(58.0 * s, 58.0 * s));
    avatar(ui, &f.t, av, me.and_then(|p| p.avatar_hash.as_deref()), color, s);
    let p = ui.painter();
    let x = av.right() + 16.0 * s;
    let width = (rect.right() - 116.0 * s - x).max(0.0);
    paint::line(
        p,
        pos2(x, av.center().y - 2.0 * s),
        Align2::LEFT_BOTTOM,
        &f.app.state.persona_name,
        w::font_bold(25.0 * s),
        f.t.text,
        width,
    );
    if let Some(me) = me {
        let status = crate::shell::status_text(&f.app.state, me);
        paint::line(
            p,
            pos2(x, av.center().y + 2.0 * s),
            Align2::LEFT_TOP,
            &status,
            w::font_medium(17.0 * s),
            color,
            width,
        );
    }
    let align = if bp.sys_status.battery_percent.is_some() { Align2::RIGHT_BOTTOM } else { Align2::RIGHT_CENTER };
    p.text(pos2(rect.right(), av.center().y), align, &bp.sys_status.clock, w::font_semibold(20.0 * s), f.t.text);
    if let Some(battery) = bp.sys_status.battery_percent {
        let battery = if bp.sys_status.battery_charging {
            format!("{} {battery}%", icons::LIGHTNING)
        } else {
            format!("{battery}%")
        };
        p.text(
            pos2(rect.right(), av.center().y + 3.0 * s),
            Align2::RIGHT_TOP,
            battery,
            w::font_medium(17.0 * s),
            f.t.text_dim,
        );
    }
    hit.is_some_and(|hit| {
        if let Some(response) = hit.response {
            response.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        hit.activated
    })
}

pub(super) fn is_active_download(d: &DownloadStatus) -> bool {
    !matches!(d.phase, DownloadPhase::Done | DownloadPhase::Failed)
}

pub(super) fn active_downloads(state: &client::State) -> impl Iterator<Item = &DownloadStatus> {
    state.downloads.values().filter(|d| is_active_download(d))
}

pub(super) fn friend_is_online(p: &Persona) -> bool {
    p.in_game() || !matches!(p.state, PersonaState::Offline | PersonaState::Invisible)
}

pub(super) fn friend_unread(state: &client::State, id: SteamId) -> usize {
    state.unread.get(&id).copied().unwrap_or_default()
}

pub(super) fn game_name(state: &client::State, id: AppId) -> String {
    state.app_name(id).map(str::to_owned).unwrap_or_else(|| id.to_string())
}

pub(super) fn avatar(ui: &Ui, t: &Theme, rect: Rect, hash: Option<&str>, color: Color32, s: f32) {
    let radius = rect.width() / 2.0;
    if !w::paint_cover(ui, rect, &model::art::avatar_full(hash), paint::radius(radius)) {
        ui.painter().circle_filled(rect.center(), radius, t.raised);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, icons::USER, w::font(0.9 * radius), t.text_faint);
    }
    ui.painter().circle_stroke(rect.center(), radius + s, Stroke::new(2.0 * s, color));
}

pub(super) fn running_game(rows: &mut DrawerRows<'_>, f: &Frame<'_>, id: AppId) {
    let s = rows.s;
    let rect = rows.next_rect(66.0);
    let p = rows.ui.painter();
    p.rect_filled(rect, paint::radius(10.0 * s), rows.theme.raised);
    let x = rect.left() + 16.0 * s;
    paint::line(
        p,
        pos2(x, rect.top() + 11.0 * s),
        Align2::LEFT_TOP,
        t!("Running game"),
        w::font_semibold(15.0 * s),
        rows.theme.in_game,
        rect.width() - 32.0 * s,
    );
    paint::line(
        p,
        pos2(x, rect.bottom() - 10.0 * s),
        Align2::LEFT_BOTTOM,
        &game_name(&f.app.state, id),
        w::font_bold(23.0 * s),
        rows.theme.text,
        rect.width() - 32.0 * s,
    );
}

pub(super) fn drawer_extent(now: f64, opened: f64, closing: Option<f64>, secs: f32) -> f32 {
    let open = ease(((closing.unwrap_or(now) - opened) as f32) / secs);
    open * closing.map_or(1.0, |at| 1.0 - ease((now - at) as f32 / secs))
}

pub(super) fn drawer_panel(ui: &Ui, t: &Theme, screen: Rect, width: f32, s: f32, e: f32, key: &str) -> (Rect, bool) {
    let panel = Rect::from_min_size(pos2(screen.right() - width * e, screen.top()), vec2(width, screen.height()));
    let outside = Rect::from_min_max(screen.min, pos2(panel.left(), screen.bottom()));
    let p = ui.painter().with_clip_rect(screen);
    p.rect_filled(screen, 0.0, Color32::from_black_alpha((150.0 * e) as u8));
    paint::shadow(&p, panel, 0.0, s, e, Color32::from_black_alpha(200));
    p.rect_filled(panel, 0.0, t.chrome);
    let edge = Rect::from_min_size(panel.min, vec2(3.0 * s, panel.height()));
    w::gradient(&p, edge, t.accent, t.accent_2, false);
    let visible = panel.intersect(screen);
    if visible.is_positive() {
        ui.interact(visible, Id::unique((key, "panel")), Sense::click());
    }
    let clicked = ui.interact(outside, Id::unique((key, "outside")), Sense::click()).clicked();
    (panel, clicked)
}

pub(super) fn clipped_child(ui: &mut Ui, bounds: Rect, clip: Rect) -> Ui {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
    child.set_clip_rect(clip);
    child
}

pub(super) fn scroll_body(
    ui: &Ui,
    focus: &Focus,
    scroll: &mut Scroller,
    viewport: Rect,
    height: f32,
    stick: f32,
    active: bool,
) {
    if active {
        if ui.rect_contains_pointer(viewport.intersect(ui.clip_rect())) {
            scroll.target -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        scroll.target += stick;
        if focus.moved
            && let Some((rect, Place::Page)) = focus.focused()
        {
            let top = rect.top() - viewport.top() + scroll.pos;
            scroll.reveal(top, top + rect.height(), viewport.height(), 10.0);
        }
    }
    scroll.clamp(height, viewport.height());
    let pos = scroll.pos;
    if !scroll.step(ui) && pos != scroll.pos {
        ui.ctx().request_repaint();
    }
    let view = viewport.height();
    if view > 0.0 && height > view {
        let thumb = (view * view / height).clamp(view.min(24.0), view);
        let offset = (pos / (height - view)).clamp(0.0, 1.0) * (view - thumb);
        let rect = Rect::from_min_size(pos2(viewport.right() - 8.0, viewport.top() + offset), vec2(3.0, thumb));
        ui.painter().rect_filled(rect, 2.0, Color32::from_white_alpha(76));
    }
}

pub(super) struct DrawerRows<'a> {
    pub ui: &'a Ui,
    pub theme: &'a Theme,
    pub focus: &'a mut Focus,
    pub clip: Rect,
    pub span: Rangef,
    pub y: f32,
    pub s: f32,
    pub active: bool,
    pub place: Place,
    /// Open/airy visual mode (used by Quick Access): calmer surface fills and wider breathing room.
    pub open_style: bool,
}

impl DrawerRows<'_> {
    pub fn next_rect(&mut self, height: f32) -> Rect {
        let gap = if self.open_style { 10.0 } else { 8.0 };
        let rect = Rect::from_min_size(pos2(self.span.min, self.y), vec2(self.span.span(), height * self.s));
        self.y = rect.bottom() + gap * self.s;
        rect
    }

    pub fn gap(&mut self, amount: f32) {
        self.y += amount * self.s;
    }

    pub fn caption(&mut self, label: &str) {
        self.y += 6.0 * self.s;
        let rect = Rect::from_min_size(pos2(self.span.min + 4.0 * self.s, self.y), vec2(self.span.span() - 8.0 * self.s, 26.0 * self.s));
        paint::line(
            self.ui.painter(),
            rect.left_center(),
            Align2::LEFT_CENTER,
            label,
            w::font_semibold(15.5 * self.s),
            self.theme.text_faint,
            rect.width(),
        );
        self.y = rect.bottom() + 6.0 * self.s;
    }

    pub fn tile(&mut self, id: Id, rect: Rect) -> Hit {
        let clip = if self.active { self.clip } else { Rect::NOTHING };
        let hit = self.focus.item(self.ui, id, rect, clip, self.place);
        let radius = if self.open_style { 12.0 * self.s } else { 10.0 * self.s };
        let idle = if self.open_style {
            Color32::from_white_alpha(9)
        } else {
            self.theme.surface
        };
        let active_bg = if self.open_style {
            w::mix(self.theme.raised, self.theme.accent.gamma_multiply(0.22), 0.25)
        } else {
            self.theme.surface_hover
        };
        self.ui.painter().rect_filled(rect, paint::radius(radius), w::mix(idle, active_bg, hit.glow));
        if hit.focused {
            self.focus.ring_around_clipped(rect, radius, self.clip);
        }
        hit
    }

    pub fn button(&mut self, id: Id, icon: &str, label: &str, value: &str, danger: bool) -> bool {
        let h = if self.open_style { 56.0 } else { 50.0 };
        let r = self.next_rect(h);
        let hit = self.tile(id, r);
        let color = if danger { self.theme.danger } else { self.theme.text };
        self.button_text(r, icon, label, value, color);
        hit.activated
    }

    pub fn button_text(&self, r: Rect, icon: &str, label: &str, value: &str, color: Color32) {
        let s = self.s;
        let p = self.ui.painter();
        let pad_x = if self.open_style { 18.0 * s } else { 16.0 * s };
        p.text(pos2(r.left() + pad_x, r.center().y), Align2::LEFT_CENTER, icon, w::font(23.0 * s), color);
        let value_width = paint::text_width(p, value, w::font_medium(18.0 * s)).min(r.width() * 0.36);
        let x = r.left() + pad_x + 36.0 * s;
        paint::line(
            p,
            pos2(x, r.center().y),
            Align2::LEFT_CENTER,
            label,
            w::font_semibold(20.5 * s),
            color,
            (r.width() - (pad_x * 2.0 + 48.0 * s) - value_width).max(0.0),
        );
        if !value.is_empty() {
            paint::line(
                p,
                pos2(r.right() - pad_x, r.center().y),
                Align2::RIGHT_CENTER,
                value,
                w::font_medium(18.0 * s),
                self.theme.text_dim,
                value_width,
            );
        }
    }

    pub fn heading(&mut self, label: &str) {
        let rect = self.next_rect(34.0);
        paint::line(
            self.ui.painter(),
            pos2(rect.left() + 2.0 * self.s, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            w::font_bold(24.0 * self.s),
            self.theme.text,
            rect.width(),
        );
    }

    pub fn note(&mut self, text: &str) {
        let r = paint::wrapped(
            self.ui.painter(),
            pos2(self.span.min + 4.0 * self.s, self.y),
            text,
            w::font(17.5 * self.s),
            self.theme.text_dim,
            (self.span.span() - 8.0 * self.s).max(40.0),
            4,
        );
        self.y = r.bottom() + 16.0 * self.s;
    }

    pub fn separator(&mut self) {
        let h = if self.open_style { 22.0 } else { 12.0 };
        let r = self.next_rect(h);
        self.ui.painter().hline(r.x_range().shrink(4.0 * self.s), r.center().y, Stroke::new(1.0, Color32::from_white_alpha(20)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_confirmation_focuses_cancel() {
        let mut menu = Menu::new(0.0);
        menu.set_view(View::Power);
        menu.focus.current = Some(menu_id("quit"));
        menu.set_view(View::ConfirmQuit);
        assert_eq!(menu.focus.current, Some(menu_id("cancel")));
    }

    #[test]
    fn guide_cancels_each_confirmation_without_exiting() {
        for (view, parent) in
            [(View::ConfirmQuit, View::Power), (View::ConfirmSignOut, View::Power), (View::ConfirmStop(42), View::Main)]
        {
            let mut menu = Menu::new(0.0);
            menu.set_view(view);
            assert!(menu.input(&[(Press::Guide, false), (Press::A, false)], 1.0) == Outcome::None);
            assert_eq!(menu.view, parent);
            assert!(!menu.press);
            assert!(menu.closing.is_none());
        }
    }

    #[test]
    fn back_returns_from_power_before_closing() {
        let mut menu = Menu::new(0.0);
        menu.set_view(View::Power);
        menu.input(&[(Press::B, false)], 1.0);
        assert_eq!(menu.view, View::Main);
        assert!(menu.closing.is_none());
        menu.input(&[(Press::B, false)], 2.0);
        assert!(menu.closing.is_some());
    }
}

#[cfg(test)]
#[test]
fn main_menu_footer_keeps_exit_visible_and_selectable() {
    let ctx = egui::Context::default();
    let panel = Rect::from_min_size(egui::Pos2::ZERO, vec2(560.0, 800.0));
    let mut focus = Focus::default();
    focus.current = Some(menu_id("user-header"));
    for (dir, press) in [(None, false), (Some(Dir::Up), true)] {
        let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(panel), ..Default::default() }, |ui| {
            focus.begin_drawer(ui, dir, press, |_| false, true);
            focus.item(ui, menu_id("user-header"), Rect::from_min_size(panel.min, vec2(550.0, 88.0)), panel, Place::Bar);
            let power = Rect::from_min_size(pos2(0.0, 1100.0), vec2(550.0, 50.0));
            focus.item(ui, menu_id("power"), power, panel, Place::Page);
            let footer = menu_footer(panel, 1.25, View::Main);
            assert!(panel.contains_rect(footer) && footer.height() > 50.0);
            let mut rows = DrawerRows {
                ui, theme: &Theme::default(), focus: &mut focus, clip: footer,
                span: footer.x_range().shrink(28.0), y: footer.top() + 12.0 * 1.25,
                s: 1.25, active: true, place: Place::Bar, open_style: false,
            };
            assert_eq!(footer_exit(&mut rows), press);
            if press {
                assert_eq!(focus.current, Some(menu_id("leave")));
                let (rect, place) = focus.focused().unwrap();
                assert_eq!(place, Place::Bar);
                assert!(footer.contains_rect(rect));
            }
        });
        output.textures_delta.clear();
    }
}
