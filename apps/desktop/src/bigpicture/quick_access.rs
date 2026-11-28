//! Quick Access's system controls, performance settings, and Steam activity.

use super::focus::{Dir, Focus, Place, Scroller};
use super::input::Press;
use super::menu::{
    DrawerRows, active_downloads, avatar, clipped_child, drawer_extent, drawer_panel, friend_is_online, friend_unread,
    game_name, running_game, scroll_body,
};
use super::{BigPicture, ChatTarget, Frame, Page, Tab, paint, settings};
use crate::builtin::settings::FpsCorner;
use client::Command;
use egui::{Align2, Id, Rangef, Rect, Ui, pos2, vec2};
use extensions::Action;
use i18n::t;
use model::{AppId, DownloadStatus, PersonaState};
use std::cmp::Reverse;
use std::time::Duration;
use ui::{Theme, icons, widgets as w};

const OPEN_SECS: f32 = 0.24;
const STOP_CONFIRM_SECS: f64 = 4.0;
const SECTIONS: [Section; 4] = [Section::System, Section::Performance, Section::Notifications, Section::Friends];
const STATUSES: [PersonaState; 5] =
    [PersonaState::Online, PersonaState::Away, PersonaState::Busy, PersonaState::Invisible, PersonaState::Offline];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Section {
    #[default]
    System,
    Performance,
    Notifications,
    Friends,
}

impl Section {
    fn index(self) -> usize {
        match self {
            Self::System => 0,
            Self::Performance => 1,
            Self::Notifications => 2,
            Self::Friends => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::System => t!("System"),
            Self::Performance => t!("Performance"),
            Self::Notifications => t!("Notifications"),
            Self::Friends => t!("Friends"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::System => icons::GEAR,
            Self::Performance => icons::CHART_BAR,
            Self::Notifications => icons::BELL,
            Self::Friends => icons::USERS,
        }
    }
}

#[derive(Default)]
struct SectionState {
    focus: Focus,
    scroll: Scroller,
}

#[derive(Default)]
struct PendingInput {
    dir: Option<Dir>,
    press: bool,
    stick: f32,
}

#[derive(Default)]
struct StopConfirmation(Option<(AppId, f64)>);

impl StopConfirmation {
    fn armed(&self, id: AppId, now: f64) -> bool {
        self.0.is_some_and(|(app, expires)| id == app && now < expires)
    }

    fn select(&mut self, id: AppId, now: f64) -> bool {
        if self.armed(id, now) {
            self.0 = None;
            true
        } else {
            self.0 = Some((id, now + STOP_CONFIRM_SECS));
            false
        }
    }
}

pub struct QuickAccess {
    pub section: Section,
    sections: [SectionState; SECTIONS.len()],
    opened: f64,
    closing: Option<f64>,
    pending: PendingInput,
    stop: StopConfirmation,
}

impl QuickAccess {
    pub fn new(now: f64, section: Section) -> Self {
        Self {
            section,
            sections: Default::default(),
            opened: now,
            closing: None,
            pending: PendingInput::default(),
            stop: StopConfirmation::default(),
        }
    }

    pub fn is_animating(&self, now: f64) -> bool {
        now - self.closing.unwrap_or(self.opened) < f64::from(OPEN_SECS)
    }

    pub fn close(&mut self, now: f64) {
        self.closing.get_or_insert(now);
        self.pending = PendingInput::default();
        self.stop.0 = None;
    }

    fn select(&mut self, section: Section) {
        if self.section != section {
            self.section = section;
            self.pending = PendingInput::default();
            self.stop.0 = None;
        }
    }

    fn step_section(&mut self, delta: i32, wrap: bool) {
        let previous = self.section;
        let current = self.sections[previous.index()].focus.current;
        let index = previous.index() as i32 + delta;
        let count = SECTIONS.len() as i32;
        let next = if wrap { index.rem_euclid(count) } else { index.clamp(0, count - 1) };
        self.select(SECTIONS[next as usize]);
        if self.section != previous && current.is_some_and(is_qa_tab) {
            let focus = &mut self.sections[self.section.index()].focus;
            focus.current = Some(qa_tab_id(self.section));
            focus.by_pointer = false;
        }
    }

    pub fn input(&mut self, presses: &[(Press, bool)], now: f64) {
        if self.closing.is_some() {
            return;
        }
        for (p, _) in presses {
            match p {
                Press::Move(d @ (Dir::Left | Dir::Right))
                    if self.sections[self.section.index()].focus.current.is_some_and(is_qa_tab) =>
                {
                    self.step_section(if *d == Dir::Left { -1 } else { 1 }, false);
                }
                Press::Move(d) => self.pending.dir = Some(*d),
                Press::A => self.pending.press = true,
                Press::Scroll(delta) => self.pending.stick += f32::from(*delta),
                Press::Lb => self.step_section(-1, true),
                Press::Rb => self.step_section(1, true),
                Press::B | Press::QuickAccess => {
                    self.close(now);
                    return;
                }
                _ => {}
            }
        }
    }
}

fn qa_id(key: &str) -> Id {
    Id::unique(("bp-qa", key))
}

fn qa_tab_id(tab: Section) -> Id {
    Id::unique(("bp-qa-tab", tab.index()))
}

fn is_qa_tab(id: Id) -> bool {
    SECTIONS.iter().any(|&tab| qa_tab_id(tab) == id)
}

pub fn show(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, screen: Rect) {
    let Some(mut drawer) = bp.quick_access.take() else {
        return;
    };
    let (theme, s, now) = (f.t.clone(), f.s, f.now);
    let animating = drawer.is_animating(now);
    if drawer.closing.is_some() && !animating {
        ui.ctx().request_repaint();
        return;
    }
    if animating {
        ui.ctx().request_repaint();
    }
    let e = drawer_extent(now, drawer.opened, drawer.closing, OPEN_SECS);
    let width = (660.0 * s).min(screen.width() * 0.90);
    let (panel, outside) = drawer_panel(ui, &theme, screen, width, s, e, "bp-qa");
    if outside && drawer.closing.is_none() {
        drawer.close(now);
        ui.ctx().request_repaint();
    }
    let active = drawer.closing.is_none();
    let pad = (32.0 * s).min(width * 0.1);
    let span = Rangef::new(panel.left() + pad, panel.right() - pad);
    let tabs = Rect::from_min_size(pos2(span.min, panel.top() + 68.0 * s), vec2(span.span(), 66.0 * s));
    let section = &mut drawer.sections[drawer.section.index()];
    let input = std::mem::take(&mut drawer.pending);
    let current = section.focus.current;
    let available_tab = current.filter(|id| is_qa_tab(*id)).unwrap_or_else(|| qa_tab_id(drawer.section));
    let (nav, horizontal) = match input.dir.filter(|_| active) {
        Some(Dir::Left) => (None, -1),
        Some(Dir::Right) => (None, 1),
        dir => (dir, 0),
    };
    let step = Step {
        item: current.filter(|_| section.focus.current_place() == Some(Place::Page)),
        delta: horizontal,
    };
    section.focus.begin_drawer(ui, nav, active && input.press, |id| is_qa_tab(id) && id != available_tab, false);
    let mut header_ui = clipped_child(ui, panel, panel.intersect(screen));
    let pad_mode = bp.effective_pad_mode(f.app.prefs.bigpicture_hints);
    let (header_close, tab_clicked) = draw_header(drawer.section, &mut section.focus, &mut header_ui, f, panel, tabs, active, pad_mode);
    let active = active && !header_close && tab_clicked.is_none();

    let playing = f.app.state.playing;
    let download = if playing.is_none() {
        active_downloads(&f.app.state).min_by_key(|d| (d.paused, d.app_id)).cloned()
    } else {
        None
    };
    if drawer.stop.0.is_some_and(|(id, _)| playing != Some(id) || !drawer.stop.armed(id, now)) {
        drawer.stop.0 = None;
    }

    let banner_top = (tabs.bottom() + 24.0 * s).min(panel.bottom());
    let banner_height = if playing.is_some() {
        208.0 * s
    } else {
        download.as_ref().map_or(0.0, |d| if d.verifying() { 82.0 * s } else { 146.0 * s })
    };
    let pinned = banner_height > 0.0 && panel.bottom() - banner_top - banner_height >= 140.0 * s;
    let footer = (16.0 * s).min((panel.bottom() - banner_top) * 0.1);
    let body_top = banner_top + if pinned { banner_height } else { 0.0 };
    let viewport = Rect::from_min_max(pos2(panel.left(), body_top), pos2(panel.right(), panel.bottom() - footer));
    let body_clip = viewport.intersect(screen);
    let body_ui = clipped_child(ui, viewport, body_clip);
    let mut close = header_close;
    if pinned {
        let bounds = Rect::from_min_max(pos2(panel.left(), banner_top), pos2(panel.right(), body_top));
        let clip = bounds.intersect(screen);
        let banner_ui = clipped_child(ui, bounds, clip);
        let mut rows = DrawerRows {
            ui: &banner_ui,
            theme: &theme,
            focus: &mut section.focus,
            clip,
            span,
            y: banner_top + 4.0 * s,
            s,
            active,
            place: Place::Bar,
            open_style: true,
        };
        close |= live_banner(&mut rows, bp, f, &mut drawer.stop, playing, download.as_ref());
    }
    let top = viewport.top() - section.scroll.pos;
    let mut rows = DrawerRows {
        ui: &body_ui,
        theme: &theme,
        focus: &mut section.focus,
        clip: body_clip,
        span,
        y: top + 6.0 * s,
        s,
        active: active && !close,
        place: Place::Page,
        open_style: true,
    };
    if !pinned {
        close |= live_banner(&mut rows, bp, f, &mut drawer.stop, playing, download.as_ref());
        rows.active &= !close;
    }
    close |= match drawer.section {
        Section::System => system_rows(&mut rows, bp, f, step),
        Section::Performance => performance_rows(&mut rows, bp, f, step),
        Section::Notifications => notification_rows(&mut rows, bp, f),
        Section::Friends => friend_rows(&mut rows, bp, f),
    };
    let height = rows.y - top + 16.0 * s;
    section.focus.end(&[], Id::NULL);
    if let Some((rect, Place::Page)) = section.focus.focused() {
        section.focus.ring_around_clipped(rect, 12.0 * s, body_clip);
    }
    scroll_body(
        &body_ui,
        &section.focus,
        &mut section.scroll,
        viewport,
        height,
        input.stick * 44.0 * s,
        active && !close,
    );
    // The focus painter uses the row's clip, including for the pinned banner.
    section.focus.paint_ring(ui, &theme, s, 12.0 * s, body_clip);
    if !bp.active {
        return;
    }
    if close {
        drawer.close(now);
        ui.ctx().request_repaint();
    } else if let Some(tab) = tab_clicked {
        drawer.select(tab);
        let focus = &mut drawer.sections[tab.index()].focus;
        focus.current = Some(qa_tab_id(tab));
        focus.by_pointer = false;
        ui.ctx().request_repaint();
    }
    bp.quick_access = Some(drawer);
}

fn draw_header(
    section: Section, focus: &mut Focus, ui: &mut Ui, f: &Frame<'_>, panel: Rect, tabs: Rect, active: bool, pad_mode: bool,
) -> (bool, Option<Section>) {
    let s = f.s;
    let p = ui.painter().with_clip_rect(panel);
    paint::line(
        &p,
        pos2(tabs.left(), panel.top() + 20.0 * s),
        Align2::LEFT_TOP,
        t!("Quick Access"),
        w::font_bold(30.0 * s),
        f.t.text,
        (tabs.width() - 64.0 * s).max(0.0),
    );
    let close = Rect::from_center_size(pos2(tabs.right() - 22.0 * s, panel.top() + 36.0 * s), vec2(40.0 * s, 40.0 * s));
    p.circle_filled(close.center(), 19.0 * s, egui::Color32::from_white_alpha(12));
    p.text(close.center(), Align2::CENTER_CENTER, icons::X, w::font(21.0 * s), f.t.text_dim);
    let clip = if active { ui.clip_rect() } else { Rect::NOTHING };
    let close_hit = focus.item(ui, qa_id("close"), close, clip, Place::Bar);
    if let Some(response) = close_hit.response {
        response.on_hover_text(t!("Close"));
    }
    if close_hit.focused {
        focus.ring_around_clipped(close, 20.0 * s, ui.clip_rect());
    }

    let notifications = f.app.state.notifications.as_ref().map_or(0, |n| n.unread);
    let friend_messages = f
        .app
        .state
        .unread
        .iter()
        .filter(|(id, _)| f.app.state.friends.get(id) == Some(&model::Relationship::Friend))
        .map(|(_, n)| n)
        .sum::<usize>();
    let badge_scale = (s * 0.8).min(tabs.width() / 220.0);
    let (left, right) = if pad_mode { ("LB", "RB") } else { ("Q", "E") };
    let left_badge = paint::glyph(&p, &f.t, tabs.left_center(), left, false, badge_scale, 0.0);
    let right_width = paint::glyph_width(&p, right, false, badge_scale);
    let right_badge = paint::glyph(&p, &f.t, tabs.right_center() - vec2(right_width, 0.0), right, false, badge_scale, 0.0);
    let gap = (8.0 * s).min(tabs.width() * 0.025);
    let start = left_badge.right() + gap;
    let stride = (right_badge.left() - gap - start).max(0.0) / SECTIONS.len() as f32;
    let mut clicked = None;
    for tab in SECTIONS {
        let rect = Rect::from_min_size(
            pos2(start + tab.index() as f32 * stride, tabs.top()),
            vec2((stride - gap * 0.5).max(0.0), tabs.height()),
        );
        let hit = focus.item(ui, qa_tab_id(tab), rect, clip, Place::Bar);
        if hit.focused {
            focus.ring_around_clipped(rect, 12.0 * s, ui.clip_rect());
        }
        if let Some(response) = hit.response {
            response.on_hover_text(tab.label());
        }
        if hit.activated && tab != section {
            clicked = Some(tab);
        }
        let selected = tab == section;
        let fill = if selected {
            w::mix(f.t.raised, f.t.accent.gamma_multiply(0.24), 0.35)
        } else {
            egui::Color32::from_white_alpha(7)
        };
        let color = if selected { f.t.accent } else { f.t.text_dim };
        let text_color = if selected { f.t.text } else { f.t.text_dim };
        p.rect_filled(rect, paint::radius(12.0 * s), fill);
        p.text(
            pos2(rect.center().x, rect.top() + 20.0 * s),
            Align2::CENTER_CENTER,
            tab.icon(),
            w::font(24.0 * s),
            color,
        );
        paint::line(
            &p,
            pos2(rect.center().x, rect.bottom() - 15.0 * s),
            Align2::CENTER_CENTER,
            tab.label(),
            w::font_semibold(15.5 * s),
            text_color,
            rect.width() - 10.0 * s,
        );
        if selected {
            let underline = Rect::from_center_size(
                pos2(rect.center().x, rect.bottom() - 2.5 * s),
                vec2(rect.width() * 0.48, 3.5 * s),
            );
            p.rect_filled(underline, paint::radius(2.0 * s), color);
        }
        let unread = match tab {
            Section::Notifications => notifications as usize,
            Section::Friends => friend_messages,
            _ => 0,
        };
        if unread > 0 {
            count_badge(&p, &f.t, rect.right_top() + vec2(-5.0 * s, 4.0 * s), Align2::RIGHT_TOP, unread, s * 0.78);
        }
    }
    p.hline(
        tabs.x_range(),
        tabs.bottom() + 12.0 * s,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(18)),
    );
    (close_hit.activated, clicked)
}

fn live_banner(
    rows: &mut DrawerRows<'_>,
    bp: &mut BigPicture,
    f: &mut Frame<'_>,
    stop: &mut StopConfirmation,
    playing: Option<AppId>,
    download: Option<&DownloadStatus>,
) -> bool {
    if let Some(id) = playing {
        running_game(rows, f, id);
        let view = rows.button(Id::unique(("bp-qa-view", id)), icons::PLAY, t!("View game"), "", false);
        let armed = stop.armed(id, f.now);
        let label = if armed { t!("Select again to stop") } else { t!("Stop game") };
        let pressed = rows.button(Id::unique(("bp-qa-stop", id)), icons::STOP, label, "", true);
        if view {
            bp.open(Page::Game(id), f.now);
            return true;
        }
        if pressed {
            if stop.select(id, f.now) {
                f.app.client.send(Command::StopGame { app_id: id });
            }
            rows.ui.ctx().request_repaint();
        }
        if let Some((_, expires)) = stop.0 {
            rows.ui.ctx().request_repaint_after(Duration::from_secs_f64((expires - f.now).max(0.0)));
        }
    } else if let Some(d) = download {
        let s = rows.s;
        let rect = rows.next_rect(62.0);
        let p = rows.ui.painter();
        p.rect_filled(rect, paint::radius(10.0 * s), f.t.raised);
        let name = game_name(&f.app.state, d.app_id);
        let percent = format!("{:.0}%", d.fraction() * 100.0);
        paint::line(
            p,
            rect.min + vec2(16.0 * s, 11.0 * s),
            Align2::LEFT_TOP,
            &name,
            w::font_semibold(21.0 * s),
            f.t.text,
            rect.width() - 98.0 * s,
        );
        p.text(
            rect.right_top() + vec2(-16.0 * s, 12.0 * s),
            Align2::RIGHT_TOP,
            percent,
            w::font_medium(18.0 * s),
            f.t.text_dim,
        );
        let track = Rect::from_min_size(
            rect.left_bottom() + vec2(16.0 * s, -20.0 * s),
            vec2((rect.width() - 32.0 * s).max(0.0), 7.0 * s),
        );
        paint::bar(rows.ui, &f.t, track, d.fraction(), false);
        if !d.verifying() {
            let app_id = d.app_id;
            let (icon, label, command) = if d.paused {
                (icons::PLAY, t!("Resume"), Command::ResumeDownload { app_id })
            } else {
                (icons::STOP, t!("Pause"), Command::PauseDownload { app_id })
            };
            if rows.button(Id::unique(("bp-qa-download", app_id)), icon, label, "", false) {
                f.app.client.send(command);
            }
        }
    }
    false
}

#[derive(Clone, Copy)]
struct Step {
    item: Option<Id>,
    delta: i32,
}

impl Step {
    fn for_item(self, id: Id, activated: bool) -> i32 {
        if self.item == Some(id) && self.delta != 0 {
            self.delta
        } else if activated {
            2 // A and clicks advance with wrap on enum choices.
        } else {
            0
        }
    }
}

fn stepped<T: Copy + PartialEq>(choices: &[T], value: T, delta: i32) -> T {
    let index = choices.iter().position(|x| *x == value).unwrap_or(0);
    choices[settings::step_index(choices.len(), index, delta, delta > 1)]
}

fn choice(rows: &mut DrawerRows<'_>, step: Step, item: Id, icon: &str, label: &str, value: &str) -> i32 {
    let rect = rows.next_rect(56.0);
    let hit = rows.tile(item, rect);
    let value = format!("{}  {value}  {}", icons::CARET_LEFT, icons::CARET_RIGHT);
    rows.button_text(rect, icon, label, &value, rows.theme.text);
    if rows.active && rect.intersect(rows.clip).is_positive() { step.for_item(item, hit.activated) } else { 0 }
}

fn toggle_value(on: bool) -> &'static str {
    if on { t!("On") } else { t!("Off") }
}

fn system_rows(rows: &mut DrawerRows<'_>, bp: &mut BigPicture, f: &mut Frame<'_>, step: Step) -> bool {
    let sys = &bp.sys_status;
    let has_device = sys.can_set_volume || sys.volume_percent.is_some() || sys.brightness_percent.is_some() || sys.battery_percent.is_some();
    if has_device {
        rows.caption(t!("System"));
    }
    if sys.can_set_volume || sys.volume_percent.is_some() {
        let icon = if sys.volume_muted { icons::SPEAKER_X } else { icons::SPEAKER_HIGH };
        if let Some(pct) = level(rows, step, "volume", icon, t!("Volume"), sys.volume_percent, 0, true) {
            bp.system.set_volume(pct, Some(rows.ui.ctx().clone()));
            rows.ui.ctx().request_repaint();
        }
        let (icon, label) =
            if sys.volume_muted { (icons::SPEAKER_X, t!("Unmute")) } else { (icons::SPEAKER_HIGH, t!("Mute")) };
        if rows.button(qa_id("mute"), icon, label, "", false) {
            bp.system.toggle_mute(Some(rows.ui.ctx().clone()));
            rows.ui.ctx().request_repaint();
        }
    }
    if let Some(brightness) = sys.brightness_percent
        && let Some(pct) =
            level(rows, step, "brightness", icons::SPARKLE, t!("Brightness"), Some(brightness), 5, sys.can_set_brightness)
    {
        bp.system.set_brightness(pct, Some(rows.ui.ctx().clone()));
        rows.ui.ctx().request_repaint();
    }
    if let Some(battery) = sys.battery_percent {
        let text = if sys.battery_charging {
            t!("Battery {n}% · Charging", n = battery)
        } else {
            t!("Battery {n}%", n = battery)
        };
        rows.note(&text);
    }
    if has_device {
        rows.separator();
    }
    rows.caption(t!("Account"));
    if rows.button(qa_id("fullscreen"), icons::MONITOR, t!("Fullscreen"), toggle_value(!bp.is_windowed()), false) {
        bp.toggle_fullscreen(rows.ui.ctx());
    }
    if rows.button(qa_id("leave"), icons::DESKTOP, t!("Exit Big Picture"), "", false) {
        bp.leave(rows.ui.ctx());
        return true;
    }
    if !f.app.demo {
        let online = f.app.state.online();
        let label = if online { t!("Go offline") } else { t!("Go online") };
        if rows.button(qa_id("offline"), icons::GLOBE, label, "", false) {
            f.app.prefs.wants_offline = online;
            f.app.client.send(if online { Command::GoOffline } else { Command::GoOnline });
        }
    }
    let status = f.app.state.me().map_or(PersonaState::Online, |p| p.state);
    let item = qa_id("status");
    let delta = choice(rows, step, item, icons::USER_CIRCLE, t!("Status"), &i18n::tr_dyn(status.label()));
    if delta != 0 {
        let next = stepped(&STATUSES, status, delta);
        if next != status {
            f.app.client.send(Command::SetPersonaState(next));
        }
    }
    rows.gap(6.0);
    if f.app.state.steam_id != 0
        && rows.button(qa_id("profile"), icons::USER_CIRCLE, t!("Profile"), icons::CARET_RIGHT, false)
    {
        bp.open(Page::Profile(f.app.state.steam_id), f.now);
        return true;
    }
    if rows.button(qa_id("settings"), icons::GEAR, t!("Settings"), icons::CARET_RIGHT, false) {
        bp.open_settings_at(settings::GENERAL, f.now);
        return true;
    }
    false
}

fn level(
    rows: &mut DrawerRows<'_>,
    step: Step,
    key: &str,
    icon: &str,
    label: &str,
    current: Option<u8>,
    minimum: u8,
    writable: bool,
) -> Option<u8> {
    let s = rows.s;
    let rect = rows.next_rect(80.0);
    let item = qa_id(key);
    let hit = writable.then(|| rows.tile(item, rect));
    let p = rows.ui.painter();
    if !writable {
        p.rect_filled(rect, paint::radius(12.0 * s), egui::Color32::from_white_alpha(9));
    }
    let pad = 18.0 * s;
    p.text(rect.min + vec2(pad, 24.0 * s), Align2::LEFT_CENTER, icon, w::font(22.0 * s), rows.theme.accent);
    paint::line(
        p,
        rect.min + vec2(pad + 34.0 * s, 24.0 * s),
        Align2::LEFT_CENTER,
        label,
        w::font_semibold(20.5 * s),
        rows.theme.text,
        rect.width() - 120.0 * s,
    );
    let value = current.map_or_else(|| "—".to_string(), |pct| format!("{pct}%"));
    p.text(
        rect.right_top() + vec2(-pad, 24.0 * s),
        Align2::RIGHT_CENTER,
        value,
        w::font_semibold(19.0 * s),
        rows.theme.text_dim,
    );
    let bar = Rect::from_min_size(
        rect.left_bottom() + vec2(pad, -24.0 * s),
        vec2((rect.width() - pad * 2.0).max(1.0), 9.0 * s),
    );
    paint::level_bar(p, rows.theme, bar, f32::from(current.unwrap_or(0)) / 100.0, writable);
    let hit = hit.filter(|_| rows.active && rect.intersect(rows.clip).is_positive())?;
    let delta = step.for_item(item, hit.activated);
    if delta == 0 {
        return None;
    }
    if let Some(pointer) = hit.response.as_ref().filter(|r| r.clicked()).and_then(|r| r.interact_pointer_pos())
        && bar.expand2(vec2(0.0, 12.0 * s)).contains(pointer)
    {
        let percent = ((pointer.x - bar.left()) / bar.width() * 100.0).round() as u8;
        return Some(percent.clamp(minimum, 100)).filter(|pct| Some(*pct) != current);
    }
    let pct = (i32::from(current.unwrap_or(0)) + 5 * delta.signum()).clamp(i32::from(minimum), 100) as u8;
    Some(pct).filter(|pct| Some(*pct) != current)
}

fn performance_rows(rows: &mut DrawerRows<'_>, bp: &mut BigPicture, f: &mut Frame<'_>, step: Step) -> bool {
    let shared = f.app.shared_flags();
    let flags = *shared.lock().unwrap();
    rows.caption(t!("In-game overlay"));
    if rows.button(
        qa_id("overlay-enabled"),
        icons::MONITOR,
        t!("Show the overlay in games"),
        toggle_value(flags.overlay_enabled),
        false,
    ) {
        shared.lock().unwrap().overlay_enabled = !flags.overlay_enabled;
        rows.ui.ctx().request_repaint();
    }
    rows.note(t!("Open in supported games with Shift+Tab or Guide. Applies on the next game launch."));
    let item = qa_id("fps");
    let delta = choice(rows, step, item, icons::CHART_BAR, t!("FPS counter"), flags.fps_counter.label());
    if delta != 0 {
        let next = stepped(&FpsCorner::ALL, flags.fps_counter, delta);
        if next != flags.fps_counter {
            shared.lock().unwrap().fps_counter = next;
            rows.ui.ctx().request_repaint();
        }
    }
    if flags.fps_counter != FpsCorner::Off
        && rows.button(
            qa_id("fps-contrast"),
            icons::EYE,
            t!("High contrast color"),
            toggle_value(flags.fps_high_contrast),
            false,
        )
    {
        shared.lock().unwrap().fps_high_contrast = !flags.fps_high_contrast;
        rows.ui.ctx().request_repaint();
    }
    if rows.button(qa_id("preview"), icons::EYE, t!("Preview overlay"), "", false) {
        f.actions.push(Action::ToggleOverlay);
    }
    #[cfg(target_os = "linux")]
    {
        rows.separator();
        let precompile = client::shaders::precompile();
        if rows.button(
            qa_id("shader-precompile"),
            icons::LIGHTNING,
            t!("Compile shaders before a game starts"),
            toggle_value(precompile),
            false,
        ) {
            client::shaders::set_precompile(!precompile);
            client::apply_steam_prefs();
            rows.ui.ctx().request_repaint();
        }
    }
    rows.separator();
    for (key, label, icon, section) in [
        ("controller-settings", t!("Controller settings"), icons::CONTROLLER, settings::CONTROLLER),
        ("overlay-settings", t!("In-game overlay"), icons::MONITOR, settings::OVERLAY),
    ] {
        if rows.button(qa_id(key), icon, label, icons::CARET_RIGHT, false) {
            bp.open_settings_at(section, f.now);
            return true;
        }
    }
    false
}

fn notification_rows(rows: &mut DrawerRows<'_>, bp: &mut BigPicture, f: &mut Frame<'_>) -> bool {
    let data = f.app.state.notifications.as_ref();
    let unread = data.map_or(0, |n| n.unread);
    rows.heading(&t!("{n} unread", n = unread));
    rows.gap(4.0);
    if unread > 0 && !f.app.demo && rows.button(qa_id("mark-all"), icons::CHECK, t!("Mark all as read"), "", false) {
        f.app.client.send(Command::MarkNotificationsRead { ids: vec![] });
    }
    let shared = f.app.shared_flags();
    let flags = *shared.lock().unwrap();
    if rows.button(
        qa_id("notification-toasts"),
        icons::BELL,
        t!("Do Not Disturb"),
        toggle_value(!flags.toast_notifications),
        false,
    ) {
        shared.lock().unwrap().toast_notifications = !flags.toast_notifications;
        rows.ui.ctx().request_repaint();
    }
    if rows.button(
        qa_id("chat-toasts"),
        icons::CHAT,
        t!("New chat messages"),
        toggle_value(flags.toast_messages),
        false,
    ) {
        shared.lock().unwrap().toast_messages = !flags.toast_messages;
        rows.ui.ctx().request_repaint();
    }
    if rows.button(qa_id("notifications-page"), icons::BELL, t!("Notifications"), icons::CARET_RIGHT, false) {
        bp.open(Page::Notifications, f.now);
        return true;
    }
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
        if !summary.is_empty() {
            rows.gap(6.0);
            for (key, icon, text, kind) in summary {
                if rows.button(qa_id(key), icon, &text, icons::CARET_RIGHT, false) {
                    let row = model::Notification { kind, ..Default::default() };
                    crate::shell::open_notification(&f.app.state, &row, f.actions);
                    return true;
                }
            }
        }
    }
    rows.separator();
    if let Some(data) = data.filter(|n| !n.items.is_empty()) {
        let mut navigated = false;
        let s = rows.s;
        for n in data.items.iter().take(15) {
            let (icon, text) = crate::shell::notification_text(&f.app.state, n);
            let rect = rows.next_rect(82.0);
            let hit = rows.tile(Id::unique(("bp-qa-notification", n.id)), rect);
            if rect.intersects(rows.clip) {
                let color = if n.read { f.t.text_dim } else { f.t.text };
                let pic = Rect::from_center_size(rect.left_center() + vec2(38.0 * s, 0.0), vec2(50.0 * s, 50.0 * s));
                crate::shell::notification_picture_scaled(rows.ui, &f.t, &f.app.state, n, icon, pic, s);
                let p = rows.ui.painter();
                let x = pic.right() + 16.0 * s;
                let text_w = (rect.right() - x - 26.0 * s).max(40.0);
                paint::wrapped(
                    p,
                    pos2(x, rect.top() + 12.0 * s),
                    &text,
                    w::font_medium(18.5 * s),
                    color,
                    text_w,
                    2,
                );
                paint::line(
                    p,
                    pos2(x, rect.bottom() - 10.0 * s),
                    Align2::LEFT_BOTTOM,
                    &ui::ago(n.time),
                    w::font(15.0 * s),
                    f.t.text_faint,
                    text_w,
                );
                if !n.read {
                    p.circle_filled(rect.right_center() - vec2(14.0 * s, 0.0), 4.5 * s, f.t.accent);
                }
            }
            if hit.activated && !navigated {
                if !n.read && !f.app.demo {
                    f.app.client.send(Command::MarkNotificationsRead { ids: vec![n.id] });
                }
                let queued = f.actions.len();
                crate::shell::open_notification(&f.app.state, n, f.actions);
                if let Some(&Action::OpenChat(id)) = f.actions.get(queued) {
                    f.actions.remove(queued);
                    bp.open(Page::Chat(ChatTarget::Friend(id)), f.now);
                    navigated = true;
                    continue;
                }
                // The shell opens gifts and trades in the browser without emitting an action.
                let opens_browser = f.app.state.steam_id != 0 && matches!(n.kind, 2 | 9 | 29);
                navigated = f.actions.len() > queued || opens_browser;
            }
        }
        navigated
    } else {
        rows.note(if data.is_some() { t!("You're all caught up.") } else { t!("Notifications unavailable") });
        false
    }
}

fn friend_rows(rows: &mut DrawerRows<'_>, bp: &mut BigPicture, f: &Frame<'_>) -> bool {
    if rows.button(qa_id("friends-page"), icons::USERS, t!("Friends"), icons::CARET_RIGHT, false) {
        bp.switch_tab(Tab::Friends, f.now, true);
        return true;
    }
    rows.separator();
    let state = &f.app.state;
    let mut friends = state.friend_list();
    friends.retain(friend_is_online);
    friends.sort_by_key(|p| (Reverse(p.in_game()), Reverse(friend_unread(state, p.steam_id))));
    if friends.is_empty() {
        rows.note(t!("No friends online"));
    }
    let mut selected = None;
    let s = rows.s;
    for friend in friends.iter().take(20) {
        let id = friend.steam_id;
        let rect = rows.next_rect(74.0);
        let hit = rows.tile(Id::unique(("bp-qa-friend", id)), rect);
        let color = crate::shell::status_color(&f.t, friend);
        if rect.intersects(rows.clip) {
            let av = Rect::from_center_size(rect.left_center() + vec2(38.0 * s, 0.0), vec2(46.0 * s, 46.0 * s));
            avatar(rows.ui, &f.t, av, friend.avatar_hash.as_deref(), color, s);
            let unread = friend_unread(state, id);
            let p = rows.ui.painter();
            let right = rect.right() - 16.0 * s;
            let right = if unread > 0 {
                count_badge(p, &f.t, pos2(right, rect.center().y), Align2::RIGHT_CENTER, unread, s).left() - 10.0 * s
            } else {
                right
            };
            let x = av.right() + 16.0 * s;
            paint::line(
                p,
                pos2(x, rect.top() + 13.0 * s),
                Align2::LEFT_TOP,
                &friend.display_name(),
                w::font_semibold(20.0 * s),
                f.t.text,
                right - x,
            );
            paint::line(
                p,
                pos2(x, rect.bottom() - 12.0 * s),
                Align2::LEFT_BOTTOM,
                &crate::shell::status_text(state, friend),
                w::font_medium(16.5 * s),
                color,
                right - x,
            );
        }
        if hit.activated {
            selected = Some(id);
        }
    }
    if let Some(id) = selected {
        bp.open(Page::Chat(ChatTarget::Friend(id)), f.now);
        return true;
    }
    false
}

fn count_badge(p: &egui::Painter, theme: &Theme, pos: egui::Pos2, align: Align2, count: usize, s: f32) -> Rect {
    let label = if count > 99 { "99+".to_owned() } else { count.to_string() };
    paint::badge(p, pos, align, &label, s, theme.accent, theme.on_accent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumpers_cycle_in_both_directions_and_wrap() {
        let mut drawer = QuickAccess::new(0.0, Section::default());
        drawer.input(&[(Press::Lb, false)], 1.0);
        assert_eq!(drawer.section, Section::Friends);
        drawer.input(&[(Press::Rb, false)], 2.0);
        assert_eq!(drawer.section, Section::System);
        drawer.input(&[(Press::Rb, false)], 3.0);
        assert_eq!(drawer.section, Section::Performance);
    }

    #[test]
    fn each_section_keeps_its_focus_and_scroll() {
        let mut drawer = QuickAccess::new(0.0, Section::Notifications);
        let notification = Id::unique("notification");
        let state = &mut drawer.sections[drawer.section.index()];
        state.focus.current = Some(notification);
        state.scroll.pos = 50.0;
        drawer.input(&[(Press::Rb, false)], 1.0);
        let state = &drawer.sections[drawer.section.index()];
        assert_eq!(state.focus.current, None);
        assert_eq!(state.scroll.pos, 0.0);
        drawer.input(&[(Press::Lb, false)], 2.0);
        let state = &drawer.sections[drawer.section.index()];
        assert_eq!(state.focus.current, Some(notification));
        assert_eq!(state.scroll.pos, 50.0);
    }

    #[test]
    fn stopping_requires_a_second_selection_for_the_same_game_before_timeout() {
        let mut stop = StopConfirmation::default();
        assert!(!stop.select(42, 0.0));
        assert!(!stop.select(99, 1.0));
        assert!(!stop.select(99, 1.0 + STOP_CONFIRM_SECS));
        assert!(stop.select(99, 2.0 + STOP_CONFIRM_SECS));
        assert!(!stop.select(99, 3.0 + STOP_CONFIRM_SECS));
    }

    #[test]
    fn close_shortcuts_stop_section_input() {
        for close in [Press::B, Press::QuickAccess] {
            let mut drawer = QuickAccess::new(0.0, Section::Performance);
            drawer.input(&[(close, false), (Press::Rb, false)], 1.0);
            assert_eq!(drawer.section, Section::Performance);
            assert!(drawer.is_animating(1.1));
            assert!(!drawer.is_animating(2.0));
        }
    }
}

#[cfg(test)]
#[test]
fn arrows_switch_sections_only_when_a_tab_has_focus() {
    let mut drawer = QuickAccess::new(0.0, Section::System);
    drawer.sections[0].focus.current = Some(qa_id("volume"));
    drawer.input(&[(Press::Move(Dir::Right), false)], 1.0);
    assert_eq!(drawer.section, Section::System);
    assert_eq!(drawer.pending.dir, Some(Dir::Right));
    drawer.sections[0].focus.current = Some(qa_tab_id(Section::System));
    drawer.input(&[(Press::Move(Dir::Right), false)], 2.0);
    assert_eq!(drawer.section, Section::Performance);
    assert_eq!(drawer.sections[1].focus.current, Some(qa_tab_id(Section::Performance)));
    drawer.input(&[(Press::Rb, false)], 3.0);
    assert_eq!(drawer.section, Section::Notifications);
    assert_eq!(drawer.sections[2].focus.current, Some(qa_tab_id(Section::Notifications)));
    drawer.input(&[(Press::Move(Dir::Left), false)], 4.0);
    assert_eq!(drawer.section, Section::Performance);
}
