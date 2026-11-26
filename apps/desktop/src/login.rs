//! Sign-in, laid out like Steam's own sign-in window: a compact frameless
//! window, account name and password on the left, QR code on the right, an
//! account picker for saved logins, and a small splash while signing in.

use ui::widgets::EditMenu;
use i18n::t;
use crate::app::SteamApp;
use client::{Command, Guard, state::Phase};
use egui::{Align, Align2, Color32, CornerRadius, Key, Layout, Rect, Sense, Stroke, Ui, pos2, vec2};
use ui::{Theme, icons, widgets as w};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoginField {
    Account,
    Password,
    GuardCode,
}

#[derive(Default)]
pub struct LoginForm {
    pub account: String,
    pub password: String,
    pub code: String,
    /// Show the sign-in form even when saved accounts exist.
    pub adding: bool,
    /// A password login is in flight.
    pub(crate) submitted: bool,
    /// The last failure came from a password attempt (show its error).
    pub(crate) password_error: bool,
    pub(crate) qr_requested: bool,
    pub(crate) qr_failed: bool,
    pub(crate) qr_cache: Option<(String, Vec<bool>, usize)>,
    /// Saved account being signed in (for the splash).
    pub signing_in: Option<String>,
    /// A saved login couldn't reach Steam ("Connection error").
    pub connection_error: Option<crate::offline_ui::ConnectionError>,
    /// Which field receives desktop OSK / Big Picture login text input.
    pub(crate) osk_field: Option<LoginField>,
    /// `None` follows `app.has_touchscreen()`; `Some(b)` is an explicit user toggle.
    pub(crate) osk_manual: Option<bool>,
    pub(crate) osk: crate::bigpicture::login::OskState,
    pub(crate) show_password: bool,
    pub(crate) bp_osk_open: bool,
    pub(crate) bp_forget_confirm: Option<(String, f64)>,
}

impl LoginForm {
    pub fn on_failed(&mut self) {
        self.password_error = self.submitted;
        if !self.submitted {
            self.qr_failed = true;
        }
        self.submitted = false;
        self.qr_requested = false;
        self.signing_in = None;
        self.show_password = false;
    }
}

/// Which window the sign-in needs: the small splash or the sign-in window.
pub fn wants_splash(app: &SteamApp) -> bool {
    app.state.phase == Phase::Connecting && !shows_form(app)
}

pub(crate) fn shows_form(app: &SteamApp) -> bool {
    app.saved.is_empty() || app.login.adding
}

pub(crate) fn desktop_osk_visible(app: &SteamApp) -> bool {
    app.login.osk_manual.unwrap_or_else(|| app.has_touchscreen())
}

fn wants_tall_window_parts(
    splash: bool,
    ask_language: bool,
    has_connection_error: bool,
    shows_form: bool,
    is_guard: bool,
    osk_visible: bool,
) -> bool {
    !splash && !ask_language && !has_connection_error && (shows_form || is_guard) && osk_visible
}

/// Whether the desktop sign-in window needs extra height for the docked OSK.
pub fn wants_tall_window(app: &SteamApp) -> bool {
    wants_tall_window_parts(
        wants_splash(app),
        app.ask_language.is_some(),
        app.login.connection_error.is_some(),
        shows_form(app),
        matches!(app.state.phase, Phase::Guard(_)),
        desktop_osk_visible(app),
    )
}

const BAR: f32 = 34.0;

pub fn show(app: &mut SteamApp, ui: &mut Ui) {
    let t = app.prefs.theme.clone();
    let show_form = shows_form(app);
    let phase = app.state.phase.clone();
    // Ask Steam for a QR code as soon as the form is visible.
    if show_form && phase == Phase::LoggedOut && !app.login.qr_requested && !app.login.qr_failed {
        app.login.qr_requested = true;
        app.client.send(Command::LoginQr);
    }
    egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
        let full = ui.max_rect();
        background(ui, &t, full);

        // Title strip: drag to move, minimize and close.
        let bar = Rect::from_min_size(full.min, vec2(full.width(), BAR));
        crate::chrome::drag_area(ui, bar, false);
        let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(Layout::left_to_right(Align::Center)));
        strip.add_space(12.0);
        w::logo(&mut strip, &t, 16.0);
        strip.add_space(2.0);
        strip.label(w::text_semibold(i18n::BRAND, 12.5, t.text_dim));
        strip.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            crate::chrome::window_buttons(ui, &t, BAR, false);
        });

        let splash = wants_splash(app) && app.ask_language.is_none();
        let pad = if splash { 24.0 } else { 44.0 };
        let body = Rect::from_min_max(pos2(full.left() + pad, bar.bottom() + 14.0), pos2(full.right() - pad, full.bottom() - 30.0));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
        match phase {
            _ if app.ask_language.is_some() => language(app, &mut c, &t),
            Phase::Guard(guards) => guard(app, &mut c, &t, &guards),
            _ if app.login.connection_error.is_some() => {
                if let Some(err) = app.login.connection_error.clone() {
                    crate::offline_ui::connection_error(app, &mut c, &t, &err);
                }
            }
            _ if splash => signing_in(app, &mut c, &t),
            _ if !show_form => saved(app, &mut c, &t),
            _ => form_and_qr(app, &mut c, &t, &phase),
        }
        if !splash {
            ui.painter().text(pos2(full.center().x, full.bottom() - 16.0), Align2::CENTER_CENTER, t!("Not affiliated with Valve."), w::font(11.0), t.text_faint);
        }
    });
}

/// First launch: pick the interface language before anything else. The
/// system's language is picked already; Settings › Interface changes it later.
fn language(app: &mut SteamApp, ui: &mut Ui, t: &Theme) {
    let Some(mut pick) = app.ask_language else { return };
    ui.add_space(6.0);
    ui.label(w::text_bold(t!("Welcome to {brand}", brand = i18n::BRAND), 22.0, t.text));
    ui.label(w::text(t!("Choose your language. You can change it later in Settings."), 13.0, t.text_dim));
    ui.add_space(14.0);
    let width = ui.available_width();
    for lang in i18n::Lang::ALL {
        let on = pick == lang;
        if w::wide_chip(ui, t, lang.native_name(), on, width).clicked() {
            pick = lang;
            // Show the screen in it right away.
            i18n::set_language(lang);
        }
        ui.add_space(6.0);
    }
    app.ask_language = Some(pick);
    ui.add_space(8.0);
    let go = w::button(ui, t, Some(icons::ARROW_RIGHT), t!("Continue"), w::Kind::Primary, w::Size::Md).clicked();
    if go || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        i18n::set_language(pick);
        app.prefs.language = Some(pick.code().to_string());
        app.prefs.language_asked = true;
        app.ask_language = None;
    }
}

/// A simple top-to-bottom gradient: a hint of the accent color fading into
/// the background.
fn background(ui: &Ui, t: &Theme, rect: Rect) {
    w::gradient(ui.painter(), rect, w::mix(t.bg, t.accent, 0.16), t.bg, false);
}

/// Small splash while a saved account signs in.
fn signing_in(app: &mut SteamApp, ui: &mut Ui, t: &Theme) {
    let who = app.login.signing_in.clone().and_then(|a| app.saved.iter().find(|s| s.account_name == a).cloned());
    ui.vertical_centered(|ui| {
        ui.add_space(8.0);
        match &who {
            Some(acc) => {
                w::avatar(ui, t, &model::art::avatar(acc.avatar_hash.as_deref()), 56.0, None);
                ui.add_space(8.0);
                let name = if acc.persona_name.is_empty() { &acc.account_name } else { &acc.persona_name };
                ui.label(w::text_semibold(t!("Signing in as {name}", name = name), 15.0, t.text));
            }
            None => {
                w::logo(ui, t, 48.0);
                ui.add_space(8.0);
                ui.label(w::text_semibold(t!("Signing in…"), 15.0, t.text));
            }
        }
        ui.add_space(14.0);
        // Indeterminate progress bar.
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().min(260.0), 4.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(2), t.raised);
        let k = (ui.input(|i| i.time) * 0.8).fract() as f32;
        let seg = r.width() * 0.35;
        let x0 = r.left() - seg + (r.width() + seg) * k;
        let bar = Rect::from_min_max(pos2(x0.max(r.left()), r.top()), pos2((x0 + seg).min(r.right()), r.bottom()));
        ui.painter().rect_filled(bar, CornerRadius::same(2), t.accent);
        ui.ctx().request_repaint();
        ui.add_space(14.0);
        if ui.link(w::text(t!("Cancel"), 12.5, t.text_dim)).clicked() {
            app.client.send(Command::CancelLogin);
            app.login.submitted = false;
            app.login.signing_in = None;
        }
    });
}

/// Saved accounts as avatar tiles, like Steam's account switcher.
fn saved(app: &mut SteamApp, ui: &mut Ui, t: &Theme) {
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        ui.label(w::text_bold(t!("Who's playing?"), 22.0, t.text));
        ui.label(w::text(t!("Choose an account to sign in."), 13.5, t.text_dim));
    });
    ui.add_space(22.0);
    if let Some(e) = &app.state.last_error {
        ui.vertical_centered(|ui| ui.label(w::text(format!("{}  {}", icons::WARNING, i18n::tr_dyn(e)), 13.0, t.danger)));
        ui.add_space(8.0);
    }
    let tile = vec2(124.0, 150.0);
    let count = app.saved.len() + 1;
    let gap = 14.0;
    let per_row = (((ui.available_width() + gap) / (tile.x + gap)).floor() as usize).max(1);
    let row_w = per_row.min(count) as f32 * (tile.x + gap) - gap;
    let mut login = None;
    let mut forget = None;
    let mut add = false;
    let accounts = app.saved.clone();
    egui::ScrollArea::vertical().max_height(ui.available_height() - 10.0).show(ui, |ui| {
        for chunk in (0..count).collect::<Vec<_>>().chunks(per_row) {
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - row_w) / 2.0).max(0.0));
                ui.spacing_mut().item_spacing.x = gap;
                for &i in chunk {
                    let (rect, resp) = ui.allocate_exact_size(tile, Sense::click());
                    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                    // Every account sits in a grey box; its border lights up on hover.
                    ui.painter().rect_filled(rect, t.corner(), t.raised);
                    ui.painter().rect_stroke(rect, t.corner(), Stroke::new(1.5, w::mix(t.border, t.accent, hover)), egui::StrokeKind::Inside);
                    let pic = Rect::from_center_size(pos2(rect.center().x, rect.top() + 56.0), vec2(92.0, 92.0));
                    if let Some(acc) = accounts.get(i) {
                        if !w::paint_cover(ui, pic, &model::art::avatar_full(acc.avatar_hash.as_deref()), CornerRadius::same(8)) {
                            w::skeleton(ui, t, pic, CornerRadius::same(8));
                        }
                        let name = if acc.persona_name.is_empty() { &acc.account_name } else { &acc.persona_name };
                        ui.painter().text(pos2(rect.center().x, pic.bottom() + 18.0), Align2::CENTER_CENTER, name, w::font_semibold(13.5), t.text);
                        ui.painter().text(pos2(rect.center().x, pic.bottom() + 35.0), Align2::CENTER_CENTER, &acc.account_name, w::font(11.5), t.text_faint);
                        // Remove from this PC, shown on hover.
                        let x = Rect::from_center_size(pic.right_top() + vec2(-2.0, 2.0), vec2(22.0, 22.0));
                        let xr = ui.interact(x, egui::Id::unique(("forget", i)), Sense::click());
                        if hover > 0.5 || xr.hovered() {
                            ui.painter().circle_filled(x.center(), 11.0, if xr.hovered() { t.danger } else { t.surface_hover });
                            ui.painter().text(x.center(), Align2::CENTER_CENTER, icons::X, w::font(11.0), t.text);
                        }
                        if xr.on_hover_text(t!("Remove from this PC")).clicked() {
                            forget = Some(acc.account_name.clone());
                        } else if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            login = Some(acc.account_name.clone());
                        }
                    } else {
                        ui.painter().rect_filled(pic, CornerRadius::same(8), t.surface_hover);
                        ui.painter().text(pic.center(), Align2::CENTER_CENTER, icons::PLUS, w::font(30.0), w::mix(t.text_dim, t.accent, hover));
                        ui.painter().text(pos2(rect.center().x, pic.bottom() + 18.0), Align2::CENTER_CENTER, t!("Add account"), w::font_semibold(13.5), t.text_dim);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            add = true;
                        }
                    }
                }
            });
            ui.add_space(gap);
        }
    });
    if let Some(account) = login {
        app.login.submitted = true;
        app.login.signing_in = Some(account.clone());
        app.client.send(Command::LoginSaved { account });
    }
    if let Some(account) = forget {
        client::store::remove(&account);
        app.saved = client::store::load();
    }
    if add {
        app.login.adding = true;
        app.state.last_error = None;
    }
}

fn label(ui: &mut Ui, t: &Theme, text: &str) {
    ui.label(w::caption_text(t, text));
    ui.add_space(-2.0);
}

fn keyboard_toggle(app: &mut SteamApp, ui: &mut Ui, t: &Theme, open: bool) {
    let kind = if open { w::Kind::Primary } else { w::Kind::Secondary };
    if w::button(ui, t, Some(icons::KEYBOARD), t!("Keyboard"), kind, w::Size::Md)
        .on_hover_text(if open { t!("Hide") } else { t!("Show") })
        .clicked()
    {
        app.login.osk_manual = Some(!open);
    }
}

fn form_and_qr(app: &mut SteamApp, ui: &mut Ui, t: &Theme, phase: &Phase) {
    let busy = app.login.submitted && *phase == Phase::Connecting;
    let osk_open = desktop_osk_visible(app);
    if !matches!(app.login.osk_field, Some(LoginField::Account | LoginField::Password)) {
        app.login.osk_field = Some(if app.login.account.is_empty() { LoginField::Account } else { LoginField::Password });
    }
    ui.add_space(if osk_open { 0.0 } else { 6.0 });
    ui.horizontal(|ui| {
        ui.label(w::text_bold(t!("Sign in"), 24.0, t.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| keyboard_toggle(app, ui, t, osk_open));
    });
    ui.add_space(if osk_open { 8.0 } else { 14.0 });
    let qr_w = 210.0;
    let item_spacing = ui.spacing().item_spacing.x;
    let column_gap = 40.0;
    let left_w = ui.available_width() - qr_w - column_gap;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = column_gap;
        // Left: account name and password.
        ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = item_spacing;
            ui.set_width(left_w);
            ui.add_enabled_ui(!busy, |ui| {
                label(ui, t, t!("Sign in with account name"));
                let name = ui.add(egui::TextEdit::singleline(&mut app.login.account).font(w::font(15.0)).desired_width(f32::INFINITY).margin(vec2(12.0, 9.0))).with_edit_menu();
                if name.gained_focus() || name.clicked() {
                    app.login.osk_field = Some(LoginField::Account);
                }
                ui.add_space(if osk_open { 8.0 } else { 12.0 });
                label(ui, t, t!("Password"));
                let mut pass_focus = false;
                let mut pass_lost_enter = false;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let pass_w = (ui.available_width() - 38.0).max(80.0);
                    let pass = ui.add(
                        egui::TextEdit::singleline(&mut app.login.password)
                            .password(!app.login.show_password)
                            .font(w::font(15.0))
                            .desired_width(pass_w)
                            .margin(vec2(12.0, 9.0)),
                    ).with_edit_menu();
                    if pass.gained_focus() || pass.clicked() {
                        app.login.osk_field = Some(LoginField::Password);
                    }
                    pass_focus = pass.has_focus();
                    pass_lost_enter = pass.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let eye_icon = if app.login.show_password { icons::EYE_SLASH } else { icons::EYE };
                    let eye_tip = if app.login.show_password { t!("Hide") } else { t!("Show") };
                    if w::icon_button_sized(ui, t, eye_icon, eye_tip, app.login.show_password, 0, 32.0).clicked() {
                        app.login.show_password = !app.login.show_password;
                    }
                });
                w::autofocus(
                    &name,
                    !osk_open
                        && app.login.account.is_empty()
                        && !pass_focus
                        && app.login.osk_field != Some(LoginField::Password),
                );
                if app.login.password_error {
                    if let Some(e) = &app.state.last_error {
                        ui.add_space(6.0);
                        ui.add(egui::Label::new(w::text(format!("{}  {}", icons::WARNING, i18n::tr_dyn(e)), 12.5, t.danger)).wrap());
                    }
                }
                ui.add_space(if osk_open { 12.0 } else { 18.0 });
                let ready = !app.login.account.trim().is_empty() && !app.login.password.is_empty();
                let label = if busy { t!("Signing in…") } else { t!("Sign in") };
                let clicked = ui
                    .add_enabled_ui(ready, |ui| {
                        ui.add_sized(vec2(ui.available_width(), if osk_open { 38.0 } else { 44.0 }), egui::Button::new(w::text_bold(label, 15.0, t.on_accent)).fill(t.accent).corner_radius(t.corner_small()))
                    })
                    .inner
                    .clicked();
                if ready && (clicked || pass_lost_enter) {
                    app.login.submitted = true;
                    app.login.show_password = false;
                    app.client.send(Command::LoginPassword { account: app.login.account.trim().to_string(), password: std::mem::take(&mut app.login.password) });
                }
            });
            ui.add_space(if osk_open { 8.0 } else { 12.0 });
            ui.horizontal(|ui| {
                if ui.link(w::text(t!("Help, I can't sign in"), 12.5, t.text_dim)).clicked() {
                    crate::links::open_http("https://help.steampowered.com/wizard/HelpWithLogin");
                }
                if !app.saved.is_empty() {
                    ui.label(w::text("·", 12.5, t.text_faint));
                    if ui.link(w::text(t!("Saved accounts"), 12.5, t.text_dim)).clicked() {
                        app.login.adding = false;
                    }
                }
            });
        });
        // Right: QR code.
        ui.allocate_ui_with_layout(vec2(qr_w, 0.0), Layout::top_down(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = item_spacing;
            label(ui, t, t!("Or sign in with QR"));
            ui.add_space(8.0);
            let qr_side = if osk_open { 164.0 } else { 190.0 };
            let (rect, _) = ui.allocate_exact_size(vec2(qr_side, qr_side), Sense::hover());
            match phase {
                Phase::Qr(url) => draw_qr(ui, &mut app.login.qr_cache, url, rect),
                _ if app.login.qr_failed => {
                    ui.painter().rect_filled(rect, t.corner(), t.raised);
                    ui.painter().text(rect.center() - vec2(0.0, 16.0), Align2::CENTER_CENTER, icons::QR, w::font(36.0), t.text_faint);
                    ui.painter().text(rect.center() + vec2(0.0, 22.0), Align2::CENTER_CENTER, t!("This code has expired"), w::font_medium(12.5), t.text_dim);
                }
                _ => w::skeleton(ui, t, rect, t.corner()),
            }
            ui.add_space(8.0);
            if app.login.qr_failed {
                if w::button(ui, t, Some(icons::REFRESH), t!("Get a new code"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    app.login.qr_failed = false;
                }
            } else {
                ui.add(egui::Label::new(w::text(t!("Use the Steam Mobile App to sign in with a QR code"), 12.0, t.text_dim)).wrap());
            }
        });
    });

    if osk_open {
        ui.add_space(10.0);
        let active = app.login.osk_field.unwrap_or(LoginField::Account);
        let go_label = match active {
            LoginField::Account => t!("Continue"),
            _ => t!("Sign in"),
        };
        if let Some(key) = crate::bigpicture::login::draw_desktop(ui, &mut app.login.osk, t, go_label) {
            if let Some(cmd) = apply_form_osk_key(&mut app.login, key, busy) {
                app.client.send(cmd);
            }
        }
    }
}

pub(crate) fn apply_form_osk_key(
    form: &mut LoginForm,
    key: crate::bigpicture::login::OskKey,
    busy: bool,
) -> Option<Command> {
    let field = match form.osk_field {
        Some(LoginField::Password) => LoginField::Password,
        _ => LoginField::Account,
    };
    match field {
        LoginField::Account => {
            if crate::bigpicture::login::apply_key(&mut form.account, key, None, false) {
                form.osk_field = Some(LoginField::Password);
            }
            None
        }
        LoginField::Password => {
            if crate::bigpicture::login::apply_key(&mut form.password, key, None, false)
                && !busy
                && !form.account.trim().is_empty()
                && !form.password.is_empty()
            {
                form.submitted = true;
                form.show_password = false;
                Some(Command::LoginPassword {
                    account: form.account.trim().to_string(),
                    password: std::mem::take(&mut form.password),
                })
            } else {
                None
            }
        }
        LoginField::GuardCode => None,
    }
}

pub(crate) fn apply_guard_osk_key(
    form: &mut LoginForm,
    key: crate::bigpicture::login::OskKey,
) -> Option<Command> {
    if crate::bigpicture::login::apply_key(&mut form.code, key, Some(5), true)
        && form.code.trim().len() == 5
    {
        Some(Command::SubmitCode(std::mem::take(&mut form.code)))
    } else {
        None
    }
}

pub(crate) fn draw_qr(ui: &Ui, cache: &mut Option<(String, Vec<bool>, usize)>, url: &str, rect: Rect) {
    if cache.as_ref().is_none_or(|(u, _, _)| u != url) {
        *cache = qrcode::QrCode::new(url.as_bytes()).ok().map(|code| {
            let width = code.width();
            let dark = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
            (url.to_string(), dark, width)
        });
    }
    let Some((_, dark, n)) = cache else { return };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(10), Color32::WHITE);
    let inner = rect.shrink(12.0);
    let cell = inner.width() / *n as f32;
    for y in 0..*n {
        for x in 0..*n {
            if dark[y * *n + x] {
                let r = Rect::from_min_size(inner.min + vec2(x as f32 * cell, y as f32 * cell), vec2(cell + 0.4, cell + 0.4));
                p.rect_filled(r, CornerRadius::ZERO, Color32::from_rgb(0x0A, 0x0D, 0x12));
            }
        }
    }
}

fn guard(app: &mut SteamApp, ui: &mut Ui, t: &Theme, guards: &[Guard]) {
    ui.vertical_centered(|ui| {
        ui.label(w::text(icons::SHIELD, 40.0, t.accent));
        ui.add_space(6.0);
        ui.label(w::text_bold(t!("Steam Guard"), 24.0, t.text));
    });
    ui.add_space(12.0);

    let code = guards.iter().find(|g| g.needs_code());
    let confirm = guards.iter().find(|g| matches!(g, Guard::DeviceConfirmation | Guard::EmailConfirmation));

    if let Some(c) = confirm {
        w::panel(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().color(t.accent));
                ui.label(w::text(
                    match c {
                        Guard::DeviceConfirmation => t!("Approve this sign-in in the Steam Mobile App."),
                        _ => t!("Open the confirmation link Steam emailed you."),
                    },
                    14.0,
                    t.text,
                ));
            });
        });
        ui.add_space(10.0);
    }

    let osk_open = desktop_osk_visible(app);
    if let Some(c) = code {
        app.login.osk_field = Some(LoginField::GuardCode);
        let hint = match c {
            Guard::EmailCode { hint } if !hint.is_empty() => t!("Enter the code Steam emailed to {email}", email = hint),
            Guard::EmailCode { .. } => t!("Enter the code Steam emailed you").to_string(),
            _ => t!("Enter the code from the Steam Mobile App").to_string(),
        };
        ui.label(w::text(if confirm.is_some() { t!("Or type a code instead:").to_string() } else { hint }, 14.0, t.text_dim));
        ui.add_space(8.0);
        let field = ui.add(
            egui::TextEdit::singleline(&mut app.login.code)
                .font(w::font_bold(30.0))
                .char_limit(5)
                .align(Align2::CENTER_CENTER)
                .desired_width(f32::INFINITY)
                .margin(vec2(12.0, if osk_open { 8.0 } else { 12.0 })),
        ).with_edit_menu();
        if field.gained_focus() || field.clicked() {
            app.login.osk_field = Some(LoginField::GuardCode);
        }
        w::autofocus(&field, app.login.code.is_empty());
        app.login.code = app.login.code.to_uppercase();
        if let Some(e) = &app.state.guard_error {
            ui.label(w::text(format!("{}  {}", icons::WARNING, i18n::tr_dyn(e)), 13.0, t.danger));
        }
        ui.add_space(10.0);
        let ready = app.login.code.trim().len() == 5;
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let mut clicked = false;
        ui.horizontal(|ui| {
            clicked = ui.add_enabled_ui(ready, |ui| w::button(ui, t, None, t!("Continue"), w::Kind::Primary, if osk_open { w::Size::Md } else { w::Size::Lg })).inner.clicked();
            ui.add_space(6.0);
            keyboard_toggle(app, ui, t, osk_open);
        });
        if ready && (clicked || enter) {
            app.client.send(Command::SubmitCode(std::mem::take(&mut app.login.code)));
        }
    }

    ui.add_space(8.0);
    if ui.link(w::text(t!("Cancel"), 13.0, t.text_dim)).clicked() {
        app.client.send(Command::CancelLogin);
        app.login.submitted = false;
    }

    if code.is_some() && osk_open {
        ui.add_space(10.0);
        if let Some(key) = crate::bigpicture::login::draw_desktop(ui, &mut app.login.osk, t, t!("Continue")) {
            if let Some(cmd) = apply_guard_osk_key(&mut app.login, key) {
                app.client.send(cmd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bigpicture::login::OskKey;

    #[test]
    fn tall_window_only_when_form_or_guard_and_osk_visible() {
        assert!(wants_tall_window_parts(false, false, false, true, false, true));
        assert!(wants_tall_window_parts(false, false, false, false, true, true));
        assert!(!wants_tall_window_parts(false, false, false, true, false, false));
        assert!(!wants_tall_window_parts(true, false, false, true, false, true));
        assert!(!wants_tall_window_parts(false, true, false, true, false, true));
        assert!(!wants_tall_window_parts(false, false, true, true, false, true));
        assert!(!wants_tall_window_parts(false, false, false, false, false, true));
    }

    #[test]
    fn desktop_osk_progresses_account_to_password_and_submits() {
        let mut form = LoginForm {
            osk_field: Some(LoginField::Account),
            show_password: true,
            ..Default::default()
        };
        assert!(apply_form_osk_key(&mut form, OskKey::Char('a'), false).is_none());
        assert!(apply_form_osk_key(&mut form, OskKey::Char('b'), false).is_none());
        assert_eq!(form.account, "ab");
        // Go on Account advances to Password without submitting.
        assert!(apply_form_osk_key(&mut form, OskKey::Go, false).is_none());
        assert_eq!(form.osk_field, Some(LoginField::Password));

        // Typing symbols into Password preserves case & symbols.
        for ch in ['P', '!', '@', '9'] {
            assert!(apply_form_osk_key(&mut form, OskKey::Char(ch), false).is_none());
        }
        assert_eq!(form.password, "P!@9");
        let cmd = apply_form_osk_key(&mut form, OskKey::Go, false);
        match cmd {
            Some(Command::LoginPassword { account, password }) => {
                assert_eq!(account, "ab");
                assert_eq!(password, "P!@9");
            }
            other => panic!("expected LoginPassword, got {other:?}"),
        }
        assert!(form.password.is_empty());
        assert!(!form.show_password);
        assert!(form.submitted);
    }

    #[test]
    fn desktop_osk_guard_code_uppercases_caps_at_five_and_submits() {
        let mut form = LoginForm {
            osk_field: Some(LoginField::GuardCode),
            ..Default::default()
        };
        for ch in ['x', 'y', '1', '2'] {
            assert!(apply_guard_osk_key(&mut form, OskKey::Char(ch)).is_none());
        }
        // Only 4 chars so far: Go does not submit yet.
        assert!(apply_guard_osk_key(&mut form, OskKey::Go).is_none());
        assert!(apply_guard_osk_key(&mut form, OskKey::Char('z')).is_none());
        assert!(apply_guard_osk_key(&mut form, OskKey::Char('q')).is_none());
        assert_eq!(form.code, "XY12Z");
        match apply_guard_osk_key(&mut form, OskKey::Go) {
            Some(Command::SubmitCode(code)) => assert_eq!(code, "XY12Z"),
            other => panic!("expected SubmitCode, got {other:?}"),
        }
        assert!(form.code.is_empty());
    }
}
