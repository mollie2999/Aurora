//! Fullscreen Big Picture & Steam Deck pre-login experience:
//! first-launch language picker, saved accounts switcher, sign-in form + QR,
//! Steam Guard verification, connection error / offline recovery, and signing-in splash.

use super::focus::{Dir, Place};
use super::input::Press;
use super::osk;
pub(crate) use super::osk::{Key as OskKey, OskState, apply_key, draw_desktop};
use super::paint::{self, Kind};
use super::{BigPicture, Frame};
use crate::app::SteamApp;
use crate::login::LoginField;
use client::{Command, Guard, state::Phase};
use egui::{Align2, Color32, Id, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use i18n::t;
use std::time::Duration;
use ui::{Theme, icons, widgets as w};

const FORGET_CONFIRM_SECS: f64 = 4.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoginScreen {
    Language,
    Guard,
    ConnectionError,
    Splash,
    Saved,
    Form,
}

fn exit_bp_id() -> Id {
    Id::unique("bp-login-exit")
}
fn lang_chip_id(i: usize) -> Id {
    Id::unique(("bp-login-lang", i))
}
fn lang_continue_id() -> Id {
    Id::unique("bp-login-lang-go")
}
fn guard_code_id() -> Id {
    Id::unique("bp-login-guard-code")
}
fn guard_go_id() -> Id {
    Id::unique("bp-login-guard-go")
}
fn guard_cancel_id() -> Id {
    Id::unique("bp-login-guard-cancel")
}
fn err_retry_id() -> Id {
    Id::unique("bp-login-err-retry")
}
fn err_offline_id() -> Id {
    Id::unique("bp-login-err-offline")
}
fn err_cancel_id() -> Id {
    Id::unique("bp-login-err-cancel")
}
fn err_auto_id() -> Id {
    Id::unique("bp-login-err-auto")
}
fn splash_cancel_id() -> Id {
    Id::unique("bp-login-splash-cancel")
}
fn saved_tile_id(i: usize) -> Id {
    Id::unique(("bp-login-saved", i))
}
fn saved_forget_id(i: usize) -> Id {
    Id::unique(("bp-login-forget", i))
}
fn form_account_id() -> Id {
    Id::unique("bp-login-account")
}
fn form_password_id() -> Id {
    Id::unique("bp-login-password")
}
fn form_eye_id() -> Id {
    Id::unique("bp-login-eye")
}
fn form_signin_id() -> Id {
    Id::unique("bp-login-signin")
}
fn form_osk_btn_id() -> Id {
    Id::unique("bp-login-osk-btn")
}
fn form_saved_btn_id() -> Id {
    Id::unique("bp-login-saved-btn")
}
fn form_qr_refresh_id() -> Id {
    Id::unique("bp-login-qr-refresh")
}
pub fn show(app: &mut SteamApp, ui: &mut Ui) {
    let mut bp = std::mem::take(&mut app.bigpicture);
    show_inner(&mut bp, app, ui);
    bp.queued.append(&mut app.bigpicture.queued);
    app.bigpicture = bp;
}

fn show_inner(bp: &mut BigPicture, app: &mut SteamApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let t = app.prefs.theme.clone();
    let screen = ui.max_rect();
    let now = ctx.input(|i| i.time);

    bp.osk.restore_function_key(&ctx);
    bp.ensure_fullscreen(&ctx, now);
    bp.sys_status = bp.system.snapshot(false, Some(&ctx));
    let s = bp.compute_scale(&ctx, screen, app.prefs.bigpicture_scale);
    let deck_like = bp.system.is_steam_deck() || bp.is_deck_preview();
    let pad = super::side_pad(screen, s, deck_like);
    let pad_mode = bp.effective_pad_mode(app.prefs.bigpicture_hints);

    let mut leave_bp = false;
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::SHIFT, egui::Key::Escape)
            || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::B)
            || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::B)
        {
            leave_bp = true;
        }
    });
    if leave_bp {
        bp.leave(&ctx);
        ctx.request_repaint();
        return;
    }

    let phase = app.state.phase.clone();
    let splash = crate::login::wants_splash(app) && app.ask_language.is_none();
    let show_form = crate::login::shows_form(app);
    let screen_kind = if app.ask_language.is_some() {
        LoginScreen::Language
    } else if matches!(phase, Phase::Guard(_)) {
        LoginScreen::Guard
    } else if app.login.connection_error.is_some() {
        LoginScreen::ConnectionError
    } else if splash {
        LoginScreen::Splash
    } else if !show_form {
        LoginScreen::Saved
    } else {
        LoginScreen::Form
    };

    if screen_kind == LoginScreen::Form
        && phase == Phase::LoggedOut
        && !app.login.qr_requested
        && !app.login.qr_failed
    {
        app.login.qr_requested = true;
        app.client.send(Command::LoginQr);
    }

    // Track initial field selection when entering Form or Guard.
    if screen_kind == LoginScreen::Form
        && !matches!(app.login.osk_field, Some(LoginField::Account | LoginField::Password))
    {
        app.login.osk_field = Some(if app.login.account.is_empty() {
            LoginField::Account
        } else {
            LoginField::Password
        });
    } else if screen_kind == LoginScreen::Guard
        && app.login.osk_field != Some(LoginField::GuardCode)
    {
        app.login.osk_field = Some(LoginField::GuardCode);
        if let Phase::Guard(guards) = &phase {
            let has_code = guards.iter().any(|g| g.needs_code());
            let has_confirm = guards
                .iter()
                .any(|g| matches!(g, Guard::DeviceConfirmation | Guard::EmailConfirmation));
            if has_code && !has_confirm && (deck_like || bp.system.has_touchscreen()) {
                app.login.bp_osk_open = true;
            }
        }
    }

    if !matches!(screen_kind, LoginScreen::Form | LoginScreen::Guard) {
        app.login.bp_osk_open = false;
    }
    if screen_kind != LoginScreen::Saved {
        app.login.bp_forget_confirm = None;
    } else if let Some((_, expires)) = app.login.bp_forget_confirm {
        if now >= expires {
            app.login.bp_forget_confirm = None;
        }
    }

    // Sync active field with focus when OSK is closed on the Form screen.
    if screen_kind == LoginScreen::Form && !app.login.bp_osk_open {
        if bp.focus.current == Some(form_account_id()) {
            app.login.osk_field = Some(LoginField::Account);
        } else if bp.focus.current == Some(form_password_id()) {
            app.login.osk_field = Some(LoginField::Password);
        }
    }

    let guard_has_code = matches!(&phase, Phase::Guard(g) if g.iter().any(|x| x.needs_code()));
    if screen_kind == LoginScreen::Guard && !guard_has_code {
        app.login.bp_osk_open = false;
    }
    let focused_on_field = match screen_kind {
        LoginScreen::Form => {
            app.login.bp_osk_open
                || matches!(
                    bp.focus.current,
                    Some(id) if id == form_account_id() || id == form_password_id()
                )
                || bp.focus.current.is_none()
        }
        LoginScreen::Guard if guard_has_code => {
            app.login.bp_osk_open
                || bp.focus.current == Some(guard_code_id())
                || bp.focus.current.is_none()
        }
        _ => false,
    };
    let typing = focused_on_field
        && match screen_kind {
            LoginScreen::Form => true,
            LoginScreen::Guard => guard_has_code,
            _ => false,
        };

    let active_target = match screen_kind {
        LoginScreen::Form => Some(if app.login.osk_field == Some(LoginField::Password) {
            form_password_id()
        } else {
            form_account_id()
        }),
        LoginScreen::Guard if guard_has_code => Some(guard_code_id()),
        _ => None,
    };
    if let Some(tid) = active_target {
        let cur = match screen_kind {
            LoginScreen::Guard => &app.login.code,
            LoginScreen::Form if app.login.osk_field == Some(LoginField::Password) => &app.login.password,
            _ => &app.login.account,
        };
        bp.osk.sync_target(tid, cur);
    }

    if typing {
        let guard_code = screen_kind == LoginScreen::Guard;
        let password = app.login.osk_field == Some(LoginField::Password);
        let copy_allowed = !password || app.login.show_password;
        let field = if guard_code {
            &mut app.login.code
        } else if password {
            &mut app.login.password
        } else {
            &mut app.login.account
        };
        osk::edit_physical(
            &mut bp.osk, field, &ctx, app.login.bp_osk_open, guard_code, copy_allowed,
        );
        if screen_kind == LoginScreen::Form {
            let tab_dir = osk::take_physical_tab(&ctx);
            if tab_dir != 0 {
                if app.login.osk_field == Some(LoginField::Password) {
                    app.login.osk_field = Some(LoginField::Account);
                    bp.focus.current = Some(form_account_id());
                    bp.osk.sync_target(form_account_id(), &app.login.account);
                } else {
                    app.login.osk_field = Some(LoginField::Password);
                    bp.focus.current = Some(form_password_id());
                    bp.osk.sync_target(form_password_id(), &app.login.password);
                }
            }
        }
    }

    let presses = bp.drain_pad_and_keys(&ctx, typing);
    let mut dir: Option<Dir> = None;
    let mut press = false;
    let (mut x, mut y, mut lt, mut rt) = (false, false, false, false);
    let mut physical_enter = false;
    let mut osk_input = super::osk::OskInput::default();

    if app.login.bp_osk_open {
        for (p, from_keys) in presses {
            osk_input.handle_press(p, from_keys);
        }
    } else {
        for (p, from_keys) in presses {
            match p {
                Press::Move(d) => dir = Some(d),
                Press::A
                    if from_keys
                        && ((screen_kind == LoginScreen::Form
                            && bp.focus.current == Some(form_password_id())
                            && !app.login.account.trim().is_empty()
                            && !app.login.password.is_empty())
                            || (screen_kind == LoginScreen::Guard
                                && bp.focus.current == Some(guard_code_id())
                                && app.login.code.trim().len() == 5)) =>
                {
                    physical_enter = true;
                }
                Press::A => press = true,
                Press::ToggleOsk if matches!(screen_kind, LoginScreen::Form) || (screen_kind == LoginScreen::Guard && guard_has_code) => {
                    app.login.bp_osk_open = true;
                    bp.osk.reset_focus();
                }
                Press::B => {
                    if app.login.bp_forget_confirm.is_some() {
                        app.login.bp_forget_confirm = None;
                    } else {
                        match screen_kind {
                            LoginScreen::Guard => {
                                app.client.send(Command::CancelLogin);
                                app.login.submitted = false;
                            }
                            LoginScreen::Splash => {
                                app.client.send(Command::CancelLogin);
                                app.login.submitted = false;
                                app.login.signing_in = None;
                            }
                            LoginScreen::ConnectionError => {
                                app.login.connection_error = None;
                                app.state.last_error = None;
                            }
                            LoginScreen::Form if !app.saved.is_empty() => {
                                app.login.adding = false;
                                app.state.last_error = None;
                            }
                            _ => {}
                        }
                    }
                }
                Press::X => x = true,
                Press::Y => y = true,
                Press::Lt => lt = true,
                Press::Rt => rt = true,
                _ => {}
            }
        }
    }

    if physical_enter {
        let busy = app.login.submitted && phase == Phase::Connecting;
        match screen_kind {
            LoginScreen::Form => {
                if let Some(cmd) =
                    crate::login::apply_form_osk_key(&mut app.login, super::osk::Key::Go, busy)
                {
                    app.client.send(cmd);
                } else if !app.login.bp_osk_open
                    && app.login.osk_field == Some(LoginField::Password)
                {
                    bp.focus.current = Some(form_password_id());
                }
            }
            LoginScreen::Guard => {
                if let Some(cmd) =
                    crate::login::apply_guard_osk_key(&mut app.login, super::osk::Key::Go)
                {
                    app.client.send(cmd);
                }
            }
            _ => {}
        }
    }

    // ---- Draw ----
    bp.paint_backdrop(ui, &t, screen);

    let top_h = 84.0 * s;
    let hint_h = 68.0 * s;
    let view = Rect::from_min_max(
        pos2(screen.left(), screen.top() + top_h),
        pos2(screen.right(), screen.bottom() - hint_h),
    );

    if app.login.bp_osk_open {
        bp.focus.pause_frame();
    } else {
        bp.focus.begin(ui, dir, press);
    }

    // Top bar: logo + brand on left; controller + battery + clock + Exit Big Picture on right.
    let bar = Rect::from_min_size(screen.min, vec2(screen.width(), top_h));
    if header_bar(bp, ui, &t, s, bar, pad) {
        bp.leave(&ctx);
        ctx.request_repaint();
        return;
    }

    let mut actions = Vec::new();
    let mut f = Frame {
        app,
        actions: &mut actions,
        t: t.clone(),
        s,
        view,
        pad,
        now,
        x,
        y,
        lt,
        rt,
        stick_scroll: 0.0,
    };

    let preferred = match screen_kind {
        LoginScreen::Language => draw_language(bp, ui, &mut f),
        LoginScreen::Guard => {
            if let Phase::Guard(guards) = &phase {
                draw_guard(bp, ui, &mut f, guards)
            } else {
                None
            }
        }
        LoginScreen::ConnectionError => draw_connection_error(bp, ui, &mut f),
        LoginScreen::Splash => draw_splash(bp, ui, &mut f),
        LoginScreen::Saved => draw_saved(bp, ui, &mut f),
        LoginScreen::Form => draw_form(bp, ui, &mut f, &phase),
    };

    if !f.app.login.bp_osk_open {
        let first: Vec<Id> = preferred.into_iter().collect();
        bp.focus.end(&first, exit_bp_id());
        bp.focus.paint_ring(ui, &t, s, 12.0 * s, view);

        let hints = build_hints(screen_kind, &f);
        let left_note = t!("Not affiliated with Valve.");
        bp.draw_hint_bar_items(ui, &t, s, screen, pad, pad_mode, now, left_note, &hints);
    } else {
        let go_label = if screen_kind == LoginScreen::Form
            && f.app.login.osk_field == Some(LoginField::Password)
        {
            t!("Sign in")
        } else {
            t!("Continue")
        };
        bp.osk.hover_haptics = f.app.prefs.osk_touchpad_haptics;
        for action in osk::draw_overlay(&mut bp.osk, ui, &t, s, screen, now, go_label, osk_input) {
            ctx.request_repaint();
            bp.osk.queue_function_key(action);
            let max_chars = (screen_kind == LoginScreen::Guard).then_some(5);
            let uppercase = screen_kind == LoginScreen::Guard;
            let copy_allowed = !(screen_kind == LoginScreen::Form
                && f.app.login.osk_field == Some(LoginField::Password)
                && !f.app.login.show_password);
            let outcome = {
                let target = match screen_kind {
                    LoginScreen::Guard => &mut f.app.login.code,
                    _ if f.app.login.osk_field == Some(LoginField::Password) => &mut f.app.login.password,
                    _ => &mut f.app.login.account,
                };
                let out = super::osk::apply_overlay_action(&mut bp.osk, target, action, max_chars, uppercase);
                if out.copy && copy_allowed {
                    ctx.copy_text(target.clone());
                }
                if out.cut && copy_allowed {
                    osk::delete_before_caret(&mut bp.osk, target, false);
                }
                out
            };
            if outcome.request_paste {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            if outcome.tab_dir != 0 && screen_kind == LoginScreen::Form {
                if f.app.login.osk_field == Some(LoginField::Password) {
                    f.app.login.osk_field = Some(LoginField::Account);
                    bp.focus.current = Some(form_account_id());
                    bp.osk.sync_target(form_account_id(), &f.app.login.account);
                } else {
                    f.app.login.osk_field = Some(LoginField::Password);
                    bp.focus.current = Some(form_password_id());
                    bp.osk.sync_target(form_password_id(), &f.app.login.password);
                }
            }
            if outcome.submitted {
                let busy = f.app.login.submitted && phase == Phase::Connecting;
                match screen_kind {
                    LoginScreen::Form => {
                        if let Some(cmd) = crate::login::apply_form_osk_key(&mut f.app.login, super::osk::Key::Go, busy) {
                            f.app.client.send(cmd);
                            f.app.login.bp_osk_open = false;
                        } else if f.app.login.osk_field == Some(LoginField::Password) {
                            bp.osk.sync_target(form_password_id(), &f.app.login.password);
                        }
                    }
                    LoginScreen::Guard => {
                        if let Some(cmd) = crate::login::apply_guard_osk_key(&mut f.app.login, super::osk::Key::Go) {
                            f.app.client.send(cmd);
                            f.app.login.bp_osk_open = false;
                        }
                    }
                    _ => {}
                }
            }
            if outcome.hide {
                f.app.login.bp_osk_open = false;
                bp.focus.current = Some(match screen_kind {
                    LoginScreen::Guard => guard_code_id(),
                    _ if f.app.login.osk_field == Some(LoginField::Password) => form_password_id(),
                    _ => form_account_id(),
                });
            }
            if !f.app.login.bp_osk_open {
                break;
            }
        }
    }

    // Battery-friendly repaint scheduling (never unconditional idle loop).
    if typing {
        ctx.request_repaint_after(Duration::from_millis(500));
    }
    if let Some((_, expires)) = f.app.login.bp_forget_confirm {
        let wait = (expires - now).max(0.05);
        ctx.request_repaint_after(Duration::from_secs_f64(wait));
    }
    ctx.request_repaint_after(Duration::from_secs(
        bp.sys_status.secs_until_next_minute.clamp(1, 60),
    ));
}

fn header_bar(bp: &mut BigPicture, ui: &mut Ui, t: &Theme, s: f32, bar: Rect, pad: f32) -> bool {
    let p = ui.painter().clone();
    w::gradient(&p, bar, t.bg.gamma_multiply(0.92), t.bg.gamma_multiply(0.0), false);

    // Left: logo + brand.
    let logo_r = Rect::from_center_size(
        pos2(bar.left() + pad + 18.0 * s, bar.center().y),
        vec2(36.0 * s, 36.0 * s),
    );
    let mut logo_ui = ui.new_child(egui::UiBuilder::new().max_rect(logo_r));
    w::logo(&mut logo_ui, t, 36.0 * s);
    p.text(
        pos2(logo_r.right() + 12.0 * s, bar.center().y),
        Align2::LEFT_CENTER,
        i18n::BRAND,
        w::font_bold(24.0 * s),
        t.text,
    );

    // Right: Exit Big Picture button, clock, battery, controller icon.
    let mut rx = bar.right() - pad;
    let exit_label = format!("{}  {}", icons::DESKTOP, t!("Exit Big Picture"));
    let exit_font = w::font_semibold(17.0 * s);
    let exit_w = paint::text_width(&p, &exit_label, exit_font.clone()) + 34.0 * s;
    let exit_h = 42.0 * s;
    let exit_r = Rect::from_min_size(pos2(rx - exit_w, bar.center().y - exit_h / 2.0), vec2(exit_w, exit_h));
    let exit_hit = bp.focus.item(ui, exit_bp_id(), exit_r, bar, Place::Bar);
    let exit_shown = paint::grow(exit_r, 1.0 + 0.03 * exit_hit.glow);
    let exit_fill = w::mix(
        w::mix(t.raised, Color32::WHITE, 0.05),
        t.accent.gamma_multiply(0.32),
        exit_hit.glow,
    );
    p.rect_filled(exit_shown, paint::radius(exit_h / 2.0), exit_fill);
    p.rect_stroke(
        exit_shown,
        paint::radius(exit_h / 2.0),
        Stroke::new(1.0 * s, w::mix(t.border, t.accent, exit_hit.glow * 0.7)),
        StrokeKind::Inside,
    );
    p.text(exit_shown.center(), Align2::CENTER_CENTER, exit_label, exit_font, t.text);
    if exit_hit.focused {
        bp.focus.ring_around(exit_shown, exit_h / 2.0);
    }
    rx = exit_r.left() - 20.0 * s;

    if !bp.sys_status.clock.is_empty() {
        let cr = p.text(
            pos2(rx, bar.center().y),
            Align2::RIGHT_CENTER,
            &bp.sys_status.clock,
            w::font_semibold(19.0 * s),
            t.text,
        );
        rx = cr.left() - 16.0 * s;
    }

    if let Some(pct) = bp.sys_status.battery_percent {
        let color = if bp.sys_status.battery_charging {
            t.play
        } else if pct <= 15 {
            t.danger
        } else if pct <= 30 {
            t.away
        } else {
            t.text_dim
        };
        let label = if bp.sys_status.battery_charging {
            format!("{} {pct}%", icons::LIGHTNING)
        } else {
            format!("{pct}%")
        };
        let br = p.text(
            pos2(rx, bar.center().y),
            Align2::RIGHT_CENTER,
            label,
            w::font_semibold(17.0 * s),
            color,
        );
        rx = br.left() - 14.0 * s;
    }

    if bp.pads > 0 {
        p.text(
            pos2(rx, bar.center().y),
            Align2::RIGHT_CENTER,
            icons::CONTROLLER,
            w::font(22.0 * s),
            t.text_dim,
        );
    }

    exit_hit.activated
}

fn build_hints(screen: LoginScreen, f: &Frame) -> Vec<(Press, String)> {
    let mut hints = vec![(Press::A, t!("Select").into())];
    match screen {
        LoginScreen::Saved => {
            if f.app.login.bp_forget_confirm.is_some() {
                hints.push((Press::B, t!("Cancel").into()));
                hints.push((Press::X, t!("Remove from this PC").into()));
            } else if !f.app.saved.is_empty() {
                hints.push((Press::X, t!("Remove").into()));
            }
        }
        LoginScreen::Guard | LoginScreen::Splash | LoginScreen::ConnectionError => {
            hints.push((Press::B, t!("Cancel").into()));
        }
        LoginScreen::Form if !f.app.saved.is_empty() => {
            hints.push((Press::B, t!("Back").into()));
        }
        _ => {}
    }
    hints
}

fn pill_btn(
    bp: &mut BigPicture,
    ui: &mut Ui,
    f: &Frame,
    id: Id,
    rect: Rect,
    icon: Option<&str>,
    label: &str,
    kind: Kind,
) -> bool {
    let hit = bp.focus.item(ui, id, rect, f.view, Place::Page);
    let shown = paint::pill(ui, &f.t, rect, icon, label, kind, &hit, f.s, None);
    if hit.focused {
        bp.focus.ring_around(shown, shown.height() / 2.0);
    }
    hit.activated
}

fn compact_btn(
    bp: &mut BigPicture,
    ui: &mut Ui,
    f: &Frame,
    id: Id,
    rect: Rect,
    icon: Option<&str>,
    label: &str,
    active: bool,
) -> bool {
    let s = f.s;
    let p = ui.painter();
    let hit = bp.focus.item(ui, id, rect, f.view, Place::Page);
    let shown = paint::grow(rect, 1.0 + 0.03 * hit.glow);
    let r = shown.height() / 2.0;
    let base = if active { f.t.accent.gamma_multiply(0.28) } else { f.t.raised };
    let fill = w::mix(base, f.t.accent.gamma_multiply(0.42), hit.glow);
    p.rect_filled(shown, paint::radius(r), fill);
    p.rect_stroke(
        shown,
        paint::radius(r),
        Stroke::new(1.2 * s, if active { f.t.accent } else { w::mix(f.t.border, f.t.accent, hit.glow) }),
        StrokeKind::Inside,
    );
    let text = match icon {
        Some(i) if !label.is_empty() => format!("{i}  {label}"),
        Some(i) => i.to_string(),
        None => label.to_string(),
    };
    let font = w::font_semibold(17.0 * s);
    paint::line(p, shown.center(), Align2::CENTER_CENTER, &text, font, f.t.text, (shown.width() - 20.0 * s).max(20.0));
    if hit.focused {
        bp.focus.ring_around(shown, r);
    }
    hit.activated
}

#[allow(clippy::too_many_arguments)]
fn text_box(
    bp: &mut BigPicture,
    ui: &mut Ui,
    f: &Frame,
    id: Id,
    rect: Rect,
    value: &str,
    placeholder: &str,
    masked: bool,
    active: bool,
    caret: usize,
    selected_all: bool,
) -> (bool, bool) {
    let s = f.s;
    let p = ui.painter();
    let hit = bp.focus.item(ui, id, rect, f.view, Place::Page);
    let r = 12.0 * s;
    let fill = w::mix(f.t.surface, f.t.raised, 0.4 + 0.4 * hit.glow);
    p.rect_filled(rect, paint::radius(r), fill);
    let border_col = if active {
        f.t.accent
    } else {
        w::mix(f.t.border, f.t.accent, hit.glow * 0.8)
    };
    p.rect_stroke(rect, paint::radius(r), Stroke::new(if active { 2.0 * s } else { 1.2 * s }, border_col), StrokeKind::Inside);

    let shown_val = if masked {
        "•".repeat(value.chars().count())
    } else {
        value.to_string()
    };
    let inner = rect.shrink2(vec2(18.0 * s, 10.0 * s));
    super::osk::paint_single_line_field(p, inner, &shown_val, placeholder, w::font_semibold(21.0 * s), &f.t, f.now, active, caret, selected_all);
    if hit.focused {
        bp.focus.ring_around(rect, r);
    }
    (hit.activated, hit.focused)
}

// ---------- 1. Language picker ----------

fn draw_language(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame) -> Option<Id> {
    let Some(mut pick) = f.app.ask_language else { return None };
    let s = f.s;
    let p = ui.painter();
    let card_w = (f.view.width() - 2.0 * f.pad).min(560.0 * s);
    let left = f.view.center().x - card_w * 0.5;
    let langs = i18n::Lang::ALL;
    let chip_h = 56.0 * s;
    let gap = 12.0 * s;
    let go_h = 58.0 * s;
    let total_h = 90.0 * s + langs.len() as f32 * (chip_h + gap) + 18.0 * s + go_h;
    let mut y = (f.view.center().y - total_h * 0.5).max(f.view.top() + 12.0 * s);

    paint::line(
        p,
        pos2(f.view.center().x, y),
        Align2::CENTER_TOP,
        &t!("Welcome to {brand}", brand = i18n::BRAND),
        w::font_bold(34.0 * s),
        f.t.text,
        card_w,
    );
    y += 46.0 * s;
    paint::line(
        p,
        pos2(f.view.center().x, y),
        Align2::CENTER_TOP,
        t!("Choose your language. You can change it later in Settings."),
        w::font(19.0 * s),
        f.t.text_dim,
        card_w,
    );
    y += 44.0 * s;

    let mut preferred = None;
    for (i, lang) in langs.iter().enumerate() {
        let on = pick == *lang;
        let id = lang_chip_id(i);
        if on {
            preferred = Some(id);
        }
        let r = Rect::from_min_size(pos2(left, y), vec2(card_w, chip_h));
        let hit = bp.focus.item(ui, id, r, f.view, Place::Page);
        let shown = paint::grow(r, 1.0 + 0.025 * hit.glow);
        let fill = if on {
            w::mix(f.t.accent.gamma_multiply(0.24), f.t.accent.gamma_multiply(0.38), hit.glow)
        } else {
            w::mix(f.t.raised, f.t.surface_hover, hit.glow)
        };
        ui.painter().rect_filled(shown, paint::radius(14.0 * s), fill);
        ui.painter().rect_stroke(
            shown,
            paint::radius(14.0 * s),
            Stroke::new(if on { 2.0 * s } else { 1.0 * s }, if on { f.t.accent } else { f.t.border }),
            StrokeKind::Inside,
        );
        ui.painter().text(
            pos2(shown.left() + 24.0 * s, shown.center().y),
            Align2::LEFT_CENTER,
            lang.native_name(),
            w::font_semibold(21.0 * s),
            f.t.text,
        );
        if on {
            ui.painter().text(
                pos2(shown.right() - 22.0 * s, shown.center().y),
                Align2::RIGHT_CENTER,
                icons::CHECK,
                w::font_bold(20.0 * s),
                f.t.accent,
            );
        }
        if hit.focused {
            bp.focus.ring_around(shown, 14.0 * s);
        }
        if hit.activated {
            pick = *lang;
            i18n::set_language(*lang);
        }
        y += chip_h + gap;
    }

    f.app.ask_language = Some(pick);
    y += 14.0 * s;
    let go_w = paint::pill_width(ui, true, t!("Continue"), s).max(240.0 * s);
    let go_r = Rect::from_min_size(pos2(f.view.center().x - go_w * 0.5, y), vec2(go_w, go_h));
    if pill_btn(bp, ui, f, lang_continue_id(), go_r, Some(icons::ARROW_RIGHT), t!("Continue"), Kind::Primary) {
        i18n::set_language(pick);
        f.app.prefs.language = Some(pick.code().to_string());
        f.app.prefs.language_asked = true;
        f.app.ask_language = None;
    }

    preferred.or(Some(lang_continue_id()))
}

// ---------- 2. Steam Guard ----------

fn draw_guard(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, guards: &[Guard]) -> Option<Id> {
    let s = f.s;
    let p = ui.painter().clone();
    let code = guards.iter().find(|g| g.needs_code());
    let confirm = guards.iter().find(|g| matches!(g, Guard::DeviceConfirmation | Guard::EmailConfirmation));
    let osk_open = code.is_some() && f.app.login.bp_osk_open;

    let col_w = (f.view.width() - 2.0 * f.pad).min(if osk_open { 820.0 * s } else { 640.0 * s });
    let left = f.view.center().x - col_w * 0.5;
    let mut y = f.view.top() + if osk_open { 10.0 * s } else { 36.0 * s };

    if !osk_open {
        p.text(pos2(f.view.center().x, y), Align2::CENTER_TOP, icons::SHIELD, w::font(48.0 * s), f.t.accent);
        y += 56.0 * s;
    }
    paint::line(
        &p,
        pos2(f.view.center().x, y),
        Align2::CENTER_TOP,
        t!("Steam Guard"),
        w::font_bold(if osk_open { 28.0 * s } else { 34.0 * s }),
        f.t.text,
        col_w,
    );
    y += if osk_open { 38.0 * s } else { 48.0 * s };

    if let Some(c) = confirm {
        let card_h = if osk_open { 50.0 * s } else { 66.0 * s };
        let card = Rect::from_min_size(pos2(left, y), vec2(col_w, card_h));
        p.rect_filled(card, paint::radius(14.0 * s), f.t.raised);
        p.rect_stroke(card, paint::radius(14.0 * s), Stroke::new(1.0 * s, f.t.border), StrokeKind::Inside);
        let spin_r = Rect::from_center_size(pos2(card.left() + 28.0 * s, card.center().y), vec2(22.0 * s, 22.0 * s));
        ui.scope_builder(egui::UiBuilder::new().max_rect(spin_r), |ui| {
            ui.add(egui::Spinner::new().size(20.0 * s).color(f.t.accent));
        });
        let msg = match c {
            Guard::DeviceConfirmation => t!("Approve this sign-in in the Steam Mobile App."),
            _ => t!("Open the confirmation link Steam emailed you."),
        };
        paint::line(&p, pos2(spin_r.right() + 14.0 * s, card.center().y), Align2::LEFT_CENTER, msg, w::font(18.0 * s), f.t.text, col_w - 72.0 * s);
        y += card_h + if osk_open { 10.0 * s } else { 18.0 * s };
    }

    if let Some(c) = code {
        let hint = match c {
            Guard::EmailCode { hint } if !hint.is_empty() => t!("Enter the code Steam emailed to {email}", email = hint),
            Guard::EmailCode { .. } => t!("Enter the code Steam emailed you").to_string(),
            _ => t!("Enter the code from the Steam Mobile App").to_string(),
        };
        let prompt = if confirm.is_some() { t!("Or type a code instead:").to_string() } else { hint };
        paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, &prompt, w::font(18.0 * s), f.t.text_dim, col_w);
        y += 28.0 * s;

        let box_w = col_w.min(360.0 * s);
        let box_h = if osk_open { 58.0 * s } else { 70.0 * s };
        let code_r = Rect::from_min_size(pos2(f.view.center().x - box_w * 0.5, y), vec2(box_w, box_h));
        let hit = bp.focus.item(ui, guard_code_id(), code_r, f.view, Place::Page);
        p.rect_filled(code_r, paint::radius(14.0 * s), w::mix(f.t.surface, f.t.raised, 0.5 + 0.4 * hit.glow));
        p.rect_stroke(
            code_r,
            paint::radius(14.0 * s),
            Stroke::new(if osk_open || hit.focused { 2.0 * s } else { 1.2 * s }, if osk_open { f.t.accent } else { w::mix(f.t.border, f.t.accent, hit.glow) }),
            StrokeKind::Inside,
        );
        // 5-slot display.
        let chars: Vec<char> = f.app.login.code.chars().take(5).collect();
        let editing = osk_open || hit.focused;
        let slot_w = (box_w - 32.0 * s) / 5.0;
        let slot_start = code_r.left() + 16.0 * s;
        for idx in 0..5 {
            let sx = slot_start + (idx as f32 + 0.5) * slot_w;
            let sy = code_r.center().y;
            if let Some(ch) = chars.get(idx) {
                if editing && bp.osk.selected_all {
                    let selected = Rect::from_center_size(pos2(sx, sy), vec2(slot_w, box_h - 12.0 * s));
                    p.rect_filled(selected, 3.0, f.t.accent.gamma_multiply(0.30));
                }
                p.text(pos2(sx, sy), Align2::CENTER_CENTER, ch.to_string(), w::font_bold(30.0 * s), f.t.text);
            } else {
                let underline = Rect::from_center_size(pos2(sx, sy + 14.0 * s), vec2(slot_w * 0.55, 2.5 * s));
                p.rect_filled(underline, paint::radius(1.0), f.t.text_faint);
            }
        }
        if editing && !bp.osk.selected_all && (f.now * 2.0).fract() < 0.55 {
            let x = slot_start + bp.osk.caret.min(chars.len()) as f32 * slot_w;
            p.vline(x, (code_r.top() + 12.0 * s)..=(code_r.bottom() - 12.0 * s), Stroke::new(2.0 * s, f.t.accent));
        }
        if hit.focused {
            bp.focus.ring_around(code_r, 14.0 * s);
        }
        if hit.activated || (!osk_open && hit.focused && f.x) {
            f.app.login.bp_osk_open = !f.app.login.bp_osk_open;
            if f.app.login.bp_osk_open {
                bp.osk.reset_focus();
            }
        }
        y += box_h + 10.0 * s;

        if let Some(e) = &f.app.state.guard_error {
            paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, &format!("{}  {}", icons::WARNING, i18n::tr_dyn(e)), w::font(16.0 * s), f.t.danger, col_w);
            y += 26.0 * s;
        }

        let ready = f.app.login.code.trim().len() == 5;
        let btn_h = if osk_open { 46.0 * s } else { 54.0 * s };
        let go_w = paint::pill_width(ui, false, t!("Continue"), s).max(170.0 * s);
        let cancel_w = paint::pill_width(ui, false, t!("Cancel"), s).max(150.0 * s);
        let gap = 16.0 * s;
        let row_w = go_w + cancel_w + gap;
        let bx = f.view.center().x - row_w * 0.5;
        let go_r = Rect::from_min_size(pos2(bx, y), vec2(go_w, btn_h));
        let cancel_r = Rect::from_min_size(pos2(bx + go_w + gap, y), vec2(cancel_w, btn_h));
        if pill_btn(bp, ui, f, guard_go_id(), go_r, None, t!("Continue"), if ready { Kind::Primary } else { Kind::Muted }) && ready {
            f.app.client.send(Command::SubmitCode(std::mem::take(&mut f.app.login.code)));
        }
        if pill_btn(bp, ui, f, guard_cancel_id(), cancel_r, None, t!("Cancel"), Kind::Secondary) {
            f.app.client.send(Command::CancelLogin);
            f.app.login.submitted = false;
        }
        Some(guard_code_id())
    } else {
        y += 12.0 * s;
        let cancel_w = paint::pill_width(ui, false, t!("Cancel"), s).max(180.0 * s);
        let cancel_r = Rect::from_min_size(pos2(f.view.center().x - cancel_w * 0.5, y), vec2(cancel_w, 54.0 * s));
        if pill_btn(bp, ui, f, guard_cancel_id(), cancel_r, None, t!("Cancel"), Kind::Secondary) {
            f.app.client.send(Command::CancelLogin);
            f.app.login.submitted = false;
        }
        Some(guard_cancel_id())
    }
}

// ---------- 3. Connection error ----------

fn draw_connection_error(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame) -> Option<Id> {
    let Some(err) = f.app.login.connection_error.clone() else { return None };
    let s = f.s;
    let p = ui.painter().clone();
    let col_w = (f.view.width() - 2.0 * f.pad).min(820.0 * s);
    let who = f.app.saved.iter().find(|a| a.account_name == err.account).cloned();

    let mut y = f.view.center().y - 170.0 * s;
    y = y.max(f.view.top() + 20.0 * s);

    p.text(pos2(f.view.center().x, y), Align2::CENTER_TOP, icons::PLUGS, w::font(52.0 * s), f.t.away);
    y += 62.0 * s;
    paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, t!("Connection error"), w::font_bold(34.0 * s), f.t.text, col_w);
    y += 44.0 * s;
    paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, t!("Aurora can't reach the Steam servers right now."), w::font(20.0 * s), f.t.text_dim, col_w);
    y += 28.0 * s;
    paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, &i18n::tr_dyn(&err.message), w::font(16.0 * s), f.t.text_faint, col_w);
    y += 34.0 * s;

    if let Some(acc) = &who {
        let name = if acc.persona_name.is_empty() { &acc.account_name } else { &acc.persona_name };
        let nw = paint::text_width(&p, name, w::font_semibold(20.0 * s));
        let av_sz = 48.0 * s;
        let row_w = av_sz + 14.0 * s + nw;
        let ax = f.view.center().x - row_w * 0.5;
        let av_r = Rect::from_min_size(pos2(ax, y), vec2(av_sz, av_sz));
        if !w::paint_cover(ui, av_r, &model::art::avatar(acc.avatar_hash.as_deref()), paint::radius(av_sz * 0.5)) {
            p.circle_filled(av_r.center(), av_sz * 0.5, f.t.raised);
        }
        p.text(pos2(av_r.right() + 14.0 * s, av_r.center().y), Align2::LEFT_CENTER, name, w::font_semibold(20.0 * s), f.t.text);
        y += av_sz + 24.0 * s;
    }

    let retry_w = paint::pill_width(ui, true, t!("Retry connection"), s);
    let offline_w = paint::pill_width(ui, true, t!("Start in offline mode"), s);
    let cancel_w = paint::pill_width(ui, false, t!("Cancel"), s);
    let gap = 16.0 * s;
    let total_w = retry_w + offline_w + cancel_w + gap * 2.0;
    let btn_h = 56.0 * s;

    let mut retry = false;
    let mut offline = false;
    let mut cancel = false;

    if total_w <= col_w {
        let mut bx = f.view.center().x - total_w * 0.5;
        if pill_btn(bp, ui, f, err_retry_id(), Rect::from_min_size(pos2(bx, y), vec2(retry_w, btn_h)), Some(icons::REFRESH), t!("Retry connection"), if err.offline_ready { Kind::Secondary } else { Kind::Primary }) {
            retry = true;
        }
        bx += retry_w + gap;
        if pill_btn(bp, ui, f, err_offline_id(), Rect::from_min_size(pos2(bx, y), vec2(offline_w, btn_h)), Some(icons::PLUGS), t!("Start in offline mode"), if err.offline_ready { Kind::Primary } else { Kind::Muted }) && err.offline_ready {
            offline = true;
        }
        bx += offline_w + gap;
        if pill_btn(bp, ui, f, err_cancel_id(), Rect::from_min_size(pos2(bx, y), vec2(cancel_w, btn_h)), None, t!("Cancel"), Kind::Secondary) {
            cancel = true;
        }
        y += btn_h + 24.0 * s;
    } else {
        let bw = col_w.min(360.0 * s);
        let bx = f.view.center().x - bw * 0.5;
        if pill_btn(bp, ui, f, err_retry_id(), Rect::from_min_size(pos2(bx, y), vec2(bw, btn_h)), Some(icons::REFRESH), t!("Retry connection"), if err.offline_ready { Kind::Secondary } else { Kind::Primary }) {
            retry = true;
        }
        y += btn_h + 12.0 * s;
        if pill_btn(bp, ui, f, err_offline_id(), Rect::from_min_size(pos2(bx, y), vec2(bw, btn_h)), Some(icons::PLUGS), t!("Start in offline mode"), if err.offline_ready { Kind::Primary } else { Kind::Muted }) && err.offline_ready {
            offline = true;
        }
        y += btn_h + 12.0 * s;
        if pill_btn(bp, ui, f, err_cancel_id(), Rect::from_min_size(pos2(bx, y), vec2(bw, btn_h)), None, t!("Cancel"), Kind::Secondary) {
            cancel = true;
        }
        y += btn_h + 20.0 * s;
    }

    if err.offline_ready {
        let auto_lbl = t!("Next time, start in offline mode by itself when Steam can't be reached");
        let row_w = col_w.min(720.0 * s);
        let row_r = Rect::from_min_size(pos2(f.view.center().x - row_w * 0.5, y), vec2(row_w, 48.0 * s));
        let hit = bp.focus.item(ui, err_auto_id(), row_r, f.view, Place::Page);
        p.rect_filled(row_r, paint::radius(12.0 * s), w::mix(f.t.surface, f.t.raised, 0.4 + 0.5 * hit.glow));
        let check_r = Rect::from_center_size(pos2(row_r.left() + 26.0 * s, row_r.center().y), vec2(26.0 * s, 26.0 * s));
        p.rect_filled(check_r, paint::radius(7.0 * s), if f.app.prefs.auto_offline { f.t.accent } else { f.t.surface });
        p.rect_stroke(check_r, paint::radius(7.0 * s), Stroke::new(1.5 * s, if f.app.prefs.auto_offline { f.t.accent } else { f.t.border }), StrokeKind::Inside);
        if f.app.prefs.auto_offline {
            p.text(check_r.center(), Align2::CENTER_CENTER, icons::CHECK, w::font_bold(15.0 * s), f.t.on_accent);
        }
        paint::line(&p, pos2(check_r.right() + 14.0 * s, row_r.center().y), Align2::LEFT_CENTER, auto_lbl, w::font(17.0 * s), f.t.text_dim, row_r.width() - 68.0 * s);
        if hit.focused {
            bp.focus.ring_around(row_r, 12.0 * s);
        }
        if hit.activated {
            f.app.prefs.auto_offline = !f.app.prefs.auto_offline;
        }
    } else {
        paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, t!("Offline mode needs one online sign-in on this PC first."), w::font(17.0 * s), f.t.text_faint, col_w);
    }

    if retry || offline {
        f.app.login.connection_error = None;
        f.app.login.signing_in = Some(err.account.clone());
        f.app.client.send(if offline {
            Command::StartOffline { account: err.account }
        } else {
            Command::LoginSaved { account: err.account }
        });
    } else if cancel {
        f.app.login.connection_error = None;
        f.app.state.last_error = None;
    }

    Some(if err.offline_ready { err_offline_id() } else { err_retry_id() })
}

// ---------- 4. Signing-in splash ----------

fn draw_splash(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame) -> Option<Id> {
    let s = f.s;
    let p = ui.painter().clone();
    let who = f
        .app
        .login
        .signing_in
        .clone()
        .and_then(|a| f.app.saved.iter().find(|acc| acc.account_name == a).cloned());
    let cy = f.view.center().y;
    let mut y = cy - 110.0 * s;

    match &who {
        Some(acc) => {
            let av_r = Rect::from_center_size(pos2(f.view.center().x, y + 48.0 * s), vec2(96.0 * s, 96.0 * s));
            if !w::paint_cover(ui, av_r, &model::art::avatar_full(acc.avatar_hash.as_deref()), paint::radius(48.0 * s)) {
                p.circle_filled(av_r.center(), 48.0 * s, f.t.raised);
            }
            p.circle_stroke(av_r.center(), 50.0 * s, Stroke::new(2.5 * s, f.t.accent));
            y += 118.0 * s;
            let name = if acc.persona_name.is_empty() { &acc.account_name } else { &acc.persona_name };
            paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, &t!("Signing in as {name}", name = name), w::font_bold(28.0 * s), f.t.text, f.view.width() - 2.0 * f.pad);
        }
        None => {
            let lr = Rect::from_center_size(pos2(f.view.center().x, y + 42.0 * s), vec2(84.0 * s, 84.0 * s));
            let mut logo_ui = ui.new_child(egui::UiBuilder::new().max_rect(lr));
            w::logo(&mut logo_ui, &f.t, 84.0 * s);
            y += 108.0 * s;
            paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, t!("Signing in…"), w::font_bold(28.0 * s), f.t.text, f.view.width() - 2.0 * f.pad);
        }
    }
    y += 50.0 * s;

    let bar_w = (f.view.width() - 2.0 * f.pad).min(420.0 * s);
    let bar_r = Rect::from_min_size(pos2(f.view.center().x - bar_w * 0.5, y), vec2(bar_w, 8.0 * s));
    let k = (f.now * 0.75).fract() as f32;
    let seg_w = bar_w * 0.36;
    let x0 = bar_r.left() - seg_w + (bar_w + seg_w) * k;
    let seg = Rect::from_min_max(pos2(x0.max(bar_r.left()), bar_r.top()), pos2((x0 + seg_w).min(bar_r.right()), bar_r.bottom()));
    p.rect_filled(bar_r, paint::radius(4.0 * s), Color32::from_white_alpha(28));
    if seg.is_positive() {
        w::gradient(&p.with_clip_rect(seg), bar_r, f.t.accent, f.t.accent_2, true);
    }
    ui.ctx().request_repaint_after(Duration::from_millis(16));
    y += 38.0 * s;

    let cancel_w = paint::pill_width(ui, false, t!("Cancel"), s).max(180.0 * s);
    let cancel_r = Rect::from_min_size(pos2(f.view.center().x - cancel_w * 0.5, y), vec2(cancel_w, 52.0 * s));
    if pill_btn(bp, ui, f, splash_cancel_id(), cancel_r, None, t!("Cancel"), Kind::Secondary) {
        f.app.client.send(Command::CancelLogin);
        f.app.login.submitted = false;
        f.app.login.signing_in = None;
    }

    Some(splash_cancel_id())
}

// ---------- 5. Saved accounts picker ----------

fn draw_saved(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame) -> Option<Id> {
    let s = f.s;
    let p = ui.painter().clone();
    let avail_w = f.view.width() - 2.0 * f.pad;
    let mut y = f.view.top() + 28.0 * s;

    paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, t!("Who's playing?"), w::font_bold(38.0 * s), f.t.text, avail_w);
    y += 48.0 * s;
    paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, t!("Choose an account to sign in."), w::font(20.0 * s), f.t.text_dim, avail_w);
    y += 36.0 * s;

    if let Some(e) = &f.app.state.last_error {
        paint::line(&p, pos2(f.view.center().x, y), Align2::CENTER_TOP, &format!("{}  {}", icons::WARNING, i18n::tr_dyn(e)), w::font(18.0 * s), f.t.danger, avail_w);
        y += 30.0 * s;
    }
    y += 14.0 * s;

    let tile = vec2(204.0 * s, 248.0 * s);
    let gap = 22.0 * s;
    let accounts = f.app.saved.clone();
    let count = accounts.len() + 1;
    let per_row = (((avail_w + gap) / (tile.x + gap)).floor() as usize).max(1);
    let rows = count.div_ceil(per_row);
    let grid_h = rows as f32 * tile.y + (rows.saturating_sub(1)) as f32 * gap;
    let rem_h = (f.view.bottom() - y).max(tile.y);
    if grid_h < rem_h {
        y += (rem_h - grid_h) * 0.35;
    }

    let mut login = None;
    let mut forget_click = None;
    let mut add = false;
    let mut focused_account: Option<String> = None;

    for row_idx in 0..rows {
        let start = row_idx * per_row;
        let end = (start + per_row).min(count);
        let row_count = end - start;
        let row_w = row_count as f32 * tile.x + (row_count.saturating_sub(1)) as f32 * gap;
        let mut rx = f.view.center().x - row_w * 0.5;

        for i in start..end {
            let rect = Rect::from_min_size(pos2(rx, y), tile);
            let tid = saved_tile_id(i);
            let hit = bp.focus.item(ui, tid, rect, f.view, Place::Page);
            let shown = paint::grow(rect, 1.0 + 0.05 * hit.glow);
            let r = 16.0 * s;

            if let Some(acc) = accounts.get(i) {
                let armed = f
                    .app
                    .login
                    .bp_forget_confirm
                    .as_ref()
                    .is_some_and(|(name, expires)| name == &acc.account_name && f.now < *expires);
                if hit.focused {
                    focused_account = Some(acc.account_name.clone());
                }
                paint::shadow(&p, shown, r, s, 0.25 + 0.55 * hit.glow, Color32::from_black_alpha(180));
                p.rect_filled(shown, paint::radius(r), w::mix(f.t.raised, f.t.surface_hover, hit.glow * 0.6));
                p.rect_stroke(
                    shown,
                    paint::radius(r),
                    Stroke::new(if armed || hit.focused { 2.0 * s } else { 1.2 * s }, if armed { f.t.danger } else { w::mix(f.t.border, f.t.accent, hit.glow) }),
                    StrokeKind::Inside,
                );

                let pic = Rect::from_center_size(pos2(shown.center().x, shown.top() + 86.0 * s), vec2(124.0 * s, 124.0 * s));
                if !w::paint_cover(ui, pic, &model::art::avatar_full(acc.avatar_hash.as_deref()), paint::radius(14.0 * s)) {
                    paint::skeleton(ui, &f.t, pic, paint::radius(14.0 * s));
                }

                let name = if acc.persona_name.is_empty() { &acc.account_name } else { &acc.persona_name };
                paint::line(&p, pos2(shown.center().x, pic.bottom() + 24.0 * s), Align2::CENTER_CENTER, name, w::font_semibold(19.0 * s), f.t.text, shown.width() - 20.0 * s);
                if armed {
                    paint::line(&p, pos2(shown.center().x, pic.bottom() + 50.0 * s), Align2::CENTER_CENTER, t!("Remove from this PC"), w::font_semibold(14.0 * s), f.t.danger, shown.width() - 16.0 * s);
                } else {
                    paint::line(&p, pos2(shown.center().x, pic.bottom() + 50.0 * s), Align2::CENTER_CENTER, &acc.account_name, w::font(15.0 * s), f.t.text_faint, shown.width() - 20.0 * s);
                }

                // Direct touch/mouse remove icon on top-right corner of the tile.
                let xr = Rect::from_center_size(shown.right_top() + vec2(-18.0 * s, 18.0 * s), vec2(30.0 * s, 30.0 * s));
                let x_resp = ui.interact(xr, saved_forget_id(i), egui::Sense::click());
                if hit.focused || x_resp.hovered() || armed {
                    let xbg = if armed || x_resp.hovered() { f.t.danger } else { f.t.surface };
                    p.circle_filled(xr.center(), 15.0 * s, xbg);
                    p.text(xr.center(), Align2::CENTER_CENTER, if armed { icons::TRASH } else { icons::X }, w::font(14.0 * s), Color32::WHITE);
                }
                if x_resp.clicked() {
                    forget_click = Some(acc.account_name.clone());
                } else if hit.activated {
                    login = Some(acc.account_name.clone());
                }
            } else {
                p.rect_filled(shown, paint::radius(r), w::mix(f.t.raised, f.t.surface_hover, hit.glow * 0.6));
                p.rect_stroke(shown, paint::radius(r), Stroke::new(1.2 * s, w::mix(f.t.border, f.t.accent, hit.glow)), StrokeKind::Inside);
                let pic = Rect::from_center_size(pos2(shown.center().x, shown.top() + 86.0 * s), vec2(124.0 * s, 124.0 * s));
                p.rect_filled(pic, paint::radius(14.0 * s), f.t.surface);
                p.text(pic.center(), Align2::CENTER_CENTER, icons::PLUS, w::font(44.0 * s), w::mix(f.t.text_dim, f.t.accent, hit.glow));
                paint::line(&p, pos2(shown.center().x, pic.bottom() + 26.0 * s), Align2::CENTER_CENTER, t!("Add account"), w::font_semibold(19.0 * s), f.t.text_dim, shown.width() - 20.0 * s);
                if hit.activated {
                    add = true;
                }
            }

            if hit.focused {
                bp.focus.ring_around(shown, r);
            }
            rx += tile.x + gap;
        }
        y += tile.y + gap;
    }

    // Disarm if focus moved away from the armed account.
    if let Some((armed_acc, _)) = &f.app.login.bp_forget_confirm {
        if focused_account.as_deref() != Some(armed_acc.as_str()) && forget_click.as_deref() != Some(armed_acc.as_str()) {
            f.app.login.bp_forget_confirm = None;
        }
    }

    let mut trigger_forget = forget_click;
    if f.x && trigger_forget.is_none() {
        trigger_forget = focused_account;
    }

    if let Some(account) = trigger_forget {
        let confirmed = f
            .app
            .login
            .bp_forget_confirm
            .as_ref()
            .is_some_and(|(a, expires)| a == &account && f.now < *expires);
        if confirmed {
            client::store::remove(&account);
            f.app.saved = client::store::load();
            f.app.login.bp_forget_confirm = None;
        } else {
            f.app.login.bp_forget_confirm = Some((account, f.now + FORGET_CONFIRM_SECS));
        }
    } else if let Some(account) = login {
        f.app.login.bp_forget_confirm = None;
        f.app.login.submitted = true;
        f.app.login.signing_in = Some(account.clone());
        f.app.client.send(Command::LoginSaved { account });
    } else if add {
        f.app.login.bp_forget_confirm = None;
        f.app.login.adding = true;
        f.app.state.last_error = None;
        f.app.login.bp_osk_open = false;
        f.app.login.osk_field = Some(LoginField::Account);
    }

    Some(saved_tile_id(0))
}

// ---------- 6. Sign-in form + QR ----------

fn draw_form(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, phase: &Phase) -> Option<Id> {
    let s = f.s;
    let p = ui.painter().clone();
    let busy = f.app.login.submitted && *phase == Phase::Connecting;
    let ready = !f.app.login.account.trim().is_empty() && !f.app.login.password.is_empty();
    let osk_open = f.app.login.bp_osk_open;

    // Unified side-by-side form + QR layout (stays visible in the upper half while overlay OSK docks at bottom).
    let avail_w = f.view.width() - 2.0 * f.pad;
    let total_w = avail_w.min(1000.0 * s);
    let qr_col_w = 320.0 * s;
    let col_gap = 48.0 * s;
    let form_w = (total_w - qr_col_w - col_gap).max(360.0 * s);
    let left = f.view.center().x - total_w * 0.5;
    let right_x = left + form_w + col_gap;
    let content_h = 430.0 * s;
    let top_y = if osk_open {
        f.view.top() + 14.0 * s
    } else {
        (f.view.center().y - content_h * 0.5).max(f.view.top() + 20.0 * s)
    };

    // Left column: account, password, sign in, keyboard toggle, saved accounts.
    let mut y = top_y;
    paint::line(&p, pos2(left, y), Align2::LEFT_TOP, t!("Sign in"), w::font_bold(36.0 * s), f.t.text, form_w);
    y += 52.0 * s;

    paint::line(&p, pos2(left, y), Align2::LEFT_TOP, t!("Sign in with account name"), w::font_medium(17.0 * s), f.t.text_dim, form_w);
    y += 24.0 * s;
    let acc_r = Rect::from_min_size(pos2(left, y), vec2(form_w, 54.0 * s));
    let acc_active = f.app.login.osk_field != Some(LoginField::Password)
        && (osk_open || bp.focus.current == Some(form_account_id()));
    let acc_val = f.app.login.account.clone();
    let acc_caret = if acc_active { bp.osk.caret } else { acc_val.chars().count() };
    let (acc_act, acc_foc) = text_box(bp, ui, f, form_account_id(), acc_r, &acc_val, t!("Account"), false, acc_active, acc_caret, acc_active && bp.osk.selected_all);
    if acc_act || (!osk_open && acc_foc && f.x) {
        f.app.login.osk_field = Some(LoginField::Account);
        f.app.login.bp_osk_open = true;
        bp.osk.reset_focus();
    }
    y += 54.0 * s + 18.0 * s;

    paint::line(&p, pos2(left, y), Align2::LEFT_TOP, t!("Password"), w::font_medium(17.0 * s), f.t.text_dim, form_w);
    y += 24.0 * s;
    let eye_w = 54.0 * s;
    let pass_r = Rect::from_min_size(pos2(left, y), vec2(form_w - eye_w - 10.0 * s, 54.0 * s));
    let eye_r = Rect::from_min_size(pos2(pass_r.right() + 10.0 * s, y), vec2(eye_w, 54.0 * s));
    let pass_active = f.app.login.osk_field == Some(LoginField::Password)
        && (osk_open || bp.focus.current == Some(form_password_id()));
    let show_pw = f.app.login.show_password;
    let pass_val = f.app.login.password.clone();
    let pass_caret = if pass_active { bp.osk.caret } else { pass_val.chars().count() };
    let (pass_act, pass_foc) = text_box(bp, ui, f, form_password_id(), pass_r, &pass_val, t!("Password"), !show_pw, pass_active, pass_caret, pass_active && bp.osk.selected_all);
    if pass_act || (!osk_open && pass_foc && f.x) {
        f.app.login.osk_field = Some(LoginField::Password);
        f.app.login.bp_osk_open = true;
        bp.osk.reset_focus();
    }
    let eye_icon = if show_pw { icons::EYE_SLASH } else { icons::EYE };
    if compact_btn(bp, ui, f, form_eye_id(), eye_r, Some(eye_icon), "", show_pw) {
        f.app.login.show_password = !f.app.login.show_password;
    }
    y += 54.0 * s + 14.0 * s;

    if f.app.login.password_error
        && let Some(e) = &f.app.state.last_error
    {
        paint::line(&p, pos2(left, y), Align2::LEFT_TOP, &format!("{}  {}", icons::WARNING, i18n::tr_dyn(e)), w::font(16.0 * s), f.t.danger, form_w);
        y += 28.0 * s;
    } else {
        y += 6.0 * s;
    }

    let sign_label = if busy { t!("Signing in…") } else { t!("Sign in") };
    let sign_r = Rect::from_min_size(pos2(left, y), vec2(form_w, 56.0 * s));
    let sign_kind = if ready && !busy { Kind::Primary } else { Kind::Muted };
    if pill_btn(bp, ui, f, form_signin_id(), sign_r, None, sign_label, sign_kind) && ready && !busy {
        f.app.login.submitted = true;
        f.app.login.show_password = false;
        f.app.client.send(Command::LoginPassword {
            account: f.app.login.account.trim().to_string(),
            password: std::mem::take(&mut f.app.login.password),
        });
    }
    y += 56.0 * s + 16.0 * s;

    // Secondary action row: Keyboard toggle + Saved accounts.
    let kbd_w = (paint::text_width(&p, t!("Keyboard"), w::font_semibold(17.0 * s)) + 38.0 * s).max(140.0 * s);
    let kbd_r = Rect::from_min_size(pos2(left, y), vec2(kbd_w, 44.0 * s));
    if compact_btn(bp, ui, f, form_osk_btn_id(), kbd_r, None, t!("Keyboard"), osk_open) {
        f.app.login.bp_osk_open = !f.app.login.bp_osk_open;
        if f.app.login.bp_osk_open {
            bp.osk.reset_focus();
        }
    }
    if !f.app.saved.is_empty() {
        let saved_w = (paint::text_width(&p, t!("Saved accounts"), w::font_semibold(17.0 * s)) + 44.0 * s).max(180.0 * s);
        let saved_r = Rect::from_min_size(pos2(kbd_r.right() + 12.0 * s, y), vec2(saved_w, 44.0 * s));
        if compact_btn(bp, ui, f, form_saved_btn_id(), saved_r, Some(icons::USERS), t!("Saved accounts"), false) {
            f.app.login.adding = false;
            f.app.state.last_error = None;
        }
    }

    // Right column: QR card.
    let card_r = Rect::from_min_size(pos2(right_x, top_y + 12.0 * s), vec2(qr_col_w, 370.0 * s));
    p.rect_filled(card_r, paint::radius(18.0 * s), f.t.raised.gamma_multiply(0.85));
    p.rect_stroke(card_r, paint::radius(18.0 * s), Stroke::new(1.0 * s, f.t.border), StrokeKind::Inside);

    paint::line(&p, pos2(card_r.center().x, card_r.top() + 24.0 * s), Align2::CENTER_TOP, t!("Or sign in with QR"), w::font_semibold(19.0 * s), f.t.text, qr_col_w - 24.0 * s);
    let qr_side = 224.0 * s;
    let qr_r = Rect::from_center_size(pos2(card_r.center().x, card_r.top() + 58.0 * s + qr_side * 0.5), vec2(qr_side, qr_side));
    match phase {
        Phase::Qr(url) => crate::login::draw_qr(ui, &mut f.app.login.qr_cache, url, qr_r),
        _ if f.app.login.qr_failed => {
            p.rect_filled(qr_r, paint::radius(14.0 * s), f.t.surface);
            p.text(qr_r.center() - vec2(0.0, 18.0 * s), Align2::CENTER_CENTER, icons::QR, w::font(44.0 * s), f.t.text_faint);
            paint::line(&p, qr_r.center() + vec2(0.0, 24.0 * s), Align2::CENTER_CENTER, t!("This code has expired"), w::font_medium(15.0 * s), f.t.text_dim, qr_side - 20.0 * s);
        }
        _ => paint::skeleton(ui, &f.t, qr_r, paint::radius(14.0 * s)),
    }

    if f.app.login.qr_failed {
        let ref_w = paint::pill_width(ui, true, t!("Get a new code"), s).min(qr_col_w - 32.0 * s);
        let ref_r = Rect::from_center_size(pos2(card_r.center().x, qr_r.bottom() + 42.0 * s), vec2(ref_w, 46.0 * s));
        if pill_btn(bp, ui, f, form_qr_refresh_id(), ref_r, Some(icons::REFRESH), t!("Get a new code"), Kind::Secondary) {
            f.app.login.qr_failed = false;
        }
    } else {
        paint::wrapped(
            &p,
            pos2(card_r.left() + 24.0 * s, qr_r.bottom() + 18.0 * s),
            t!("Use the Steam Mobile App to sign in with a QR code"),
            w::font(15.0 * s),
            f.t.text_dim,
            qr_col_w - 48.0 * s,
            2,
        );
    }

    Some(if f.app.login.account.is_empty() {
        form_account_id()
    } else {
        form_password_id()
    })
}
