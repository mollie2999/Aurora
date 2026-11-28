//! Our own window frame (the Windows title bar is turned off): a drag area,
//! minimize / maximize / close buttons, and resize handles on the edges.

use egui::{Align2, Color32, CursorIcon, Id, Rect, ResizeDirection, Sense, Ui, Vec2, ViewportCommand, vec2};
use ui::{Theme, icons, widgets as w};

/// Make `rect` move the window when dragged (double-click maximizes).
/// Call it before adding widgets on top, so they still get their clicks.
pub fn drag_area(ui: &mut Ui, rect: Rect, can_maximize: bool) {
    // One id per drag area (a window can have several, e.g. Friends & Chat).
    let resp = ui.interact(rect, Id::unique(("window-drag", rect.min.x as i32, rect.min.y as i32)), Sense::click_and_drag());
    if resp.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if can_maximize && resp.double_clicked() {
        toggle_maximized(ui.ctx());
    }
}

pub fn toggle_maximized(ctx: &egui::Context) {
    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
}

fn trim_after_frame_id() -> Id {
    Id::unique("trim-after-frame")
}

pub(crate) fn take_memory_trim_request(ctx: &egui::Context) -> bool {
    ctx.data_mut(|data| data.remove_temp::<bool>(trim_after_frame_id())).unwrap_or(false)
}

/// Minimize the window and release cached images before the frame completes.
pub fn minimize(ctx: &egui::Context) {
    ctx.forget_all_images();
    ctx.data_mut(|data| data.insert_temp(trim_after_frame_id(), true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
}

/// Outer corner radius for floating (non-maximized, non-fullscreen) windows.
pub fn window_rounding(ctx: &egui::Context) -> egui::CornerRadius {
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        egui::CornerRadius::ZERO
    } else {
        egui::CornerRadius::same(12)
    }
}

/// Minimize, maximize/restore and close, right to left (call inside a
/// right-to-left layout). `height` is the bar height.
pub fn window_buttons(ui: &mut Ui, t: &Theme, height: f32, can_maximize: bool) {
    window_buttons_variant(ui, t, height, can_maximize, themes::WinCtrlVariant::Windows);
}

/// Minimize, maximize/restore and close styled by [`themes::WinCtrlVariant`].
///
/// Works cleanly in both left-to-right and right-to-left layouts (detects the
/// layout direction from `ui.layout().main_dir()`).
pub fn window_buttons_variant(
    ui: &mut Ui,
    t: &Theme,
    height: f32,
    can_maximize: bool,
    variant: themes::WinCtrlVariant,
) {
    use themes::WinCtrlVariant as V;
    let ctx = ui.ctx().clone();
    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
    match variant {
        V::Windows => {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if rtl {
                    if button(ui, t, icons::X, height, true).on_hover_text(i18n::tr_dyn("Close")).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    if can_maximize {
                        let (icon, tip) = if max { (icons::RESTORE, "Restore") } else { (icons::SQUARE, "Maximize") };
                        if button(ui, t, icon, height, false).on_hover_text(i18n::tr_dyn(tip)).clicked() {
                            toggle_maximized(&ctx);
                        }
                    }
                    if button(ui, t, icons::MINUS, height, false).on_hover_text(i18n::tr_dyn("Minimize")).clicked() {
                        minimize(&ctx);
                    }
                } else {
                    if button(ui, t, icons::MINUS, height, false).on_hover_text(i18n::tr_dyn("Minimize")).clicked() {
                        minimize(&ctx);
                    }
                    if can_maximize {
                        let (icon, tip) = if max { (icons::RESTORE, "Restore") } else { (icons::SQUARE, "Maximize") };
                        if button(ui, t, icon, height, false).on_hover_text(i18n::tr_dyn(tip)).clicked() {
                            toggle_maximized(&ctx);
                        }
                    }
                    if button(ui, t, icons::X, height, true).on_hover_text(i18n::tr_dyn("Close")).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
            });
        }
        V::Macos => {
            let group_id = ui.scope_id().with("macos-lights");
            let show_glyphs: bool = ui.data(|d| d.get_temp(group_id)).unwrap_or(false);
            let mut any_hovered = false;
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if rtl {
                    ui.add_space(6.0);
                    let r = traffic_light(ui, height, Color32::from_rgb(0xFF, 0x5F, 0x56), "×", show_glyphs).on_hover_text(i18n::tr_dyn("Close"));
                    any_hovered |= r.hovered();
                    if r.clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    if can_maximize {
                        let tip = if max { "Restore" } else { "Maximize" };
                        let r = traffic_light(ui, height, Color32::from_rgb(0x27, 0xC9, 0x3F), "+", show_glyphs).on_hover_text(i18n::tr_dyn(tip));
                        any_hovered |= r.hovered();
                        if r.clicked() {
                            toggle_maximized(&ctx);
                        }
                    }
                    let r = traffic_light(ui, height, Color32::from_rgb(0xFF, 0xBD, 0x2E), "−", show_glyphs).on_hover_text(i18n::tr_dyn("Minimize"));
                    any_hovered |= r.hovered();
                    if r.clicked() {
                        minimize(&ctx);
                    }
                } else {
                    let r = traffic_light(ui, height, Color32::from_rgb(0xFF, 0x5F, 0x56), "×", show_glyphs).on_hover_text(i18n::tr_dyn("Close"));
                    any_hovered |= r.hovered();
                    if r.clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    let r = traffic_light(ui, height, Color32::from_rgb(0xFF, 0xBD, 0x2E), "−", show_glyphs).on_hover_text(i18n::tr_dyn("Minimize"));
                    any_hovered |= r.hovered();
                    if r.clicked() {
                        minimize(&ctx);
                    }
                    if can_maximize {
                        let tip = if max { "Restore" } else { "Maximize" };
                        let r = traffic_light(ui, height, Color32::from_rgb(0x27, 0xC9, 0x3F), "+", show_glyphs).on_hover_text(i18n::tr_dyn(tip));
                        any_hovered |= r.hovered();
                        if r.clicked() {
                            toggle_maximized(&ctx);
                        }
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(group_id, any_hovered));
        }
        V::Minimal => {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if rtl {
                    ui.add_space(6.0);
                    if minimal_button(ui, t, icons::X, height, true).on_hover_text(i18n::tr_dyn("Close")).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    if can_maximize {
                        let (icon, tip) = if max { (icons::RESTORE, "Restore") } else { (icons::SQUARE, "Maximize") };
                        if minimal_button(ui, t, icon, height, false).on_hover_text(i18n::tr_dyn(tip)).clicked() {
                            toggle_maximized(&ctx);
                        }
                    }
                    if minimal_button(ui, t, icons::MINUS, height, false).on_hover_text(i18n::tr_dyn("Minimize")).clicked() {
                        minimize(&ctx);
                    }
                } else {
                    if minimal_button(ui, t, icons::MINUS, height, false).on_hover_text(i18n::tr_dyn("Minimize")).clicked() {
                        minimize(&ctx);
                    }
                    if can_maximize {
                        let (icon, tip) = if max { (icons::RESTORE, "Restore") } else { (icons::SQUARE, "Maximize") };
                        if minimal_button(ui, t, icon, height, false).on_hover_text(i18n::tr_dyn(tip)).clicked() {
                            toggle_maximized(&ctx);
                        }
                    }
                    if minimal_button(ui, t, icons::X, height, true).on_hover_text(i18n::tr_dyn("Close")).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
            });
        }
    }
}

fn traffic_light(ui: &mut Ui, height: f32, color: Color32, glyph: &str, show_glyph: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, height), Sense::click());
    let center = rect.center();
    let radius = 6.0;
    let p = ui.painter();
    p.circle_filled(center, radius, color);
    p.circle_stroke(center, radius, egui::Stroke::new(0.5, Color32::from_black_alpha(45)));
    let hover = ui.ctx().animate_bool_with_time(resp.id, show_glyph || resp.hovered(), 0.08);
    if hover > 0.01 {
        p.text(
            center,
            Align2::CENTER_CENTER,
            glyph,
            w::font_bold(10.0),
            Color32::from_black_alpha((170.0 * hover) as u8),
        );
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn minimal_button(ui: &mut Ui, t: &Theme, icon: &str, height: f32, close: bool) -> egui::Response {
    let (outer, resp) = ui.allocate_exact_size(vec2(28.0, height), Sense::click());
    let btn = Rect::from_center_size(outer.center(), vec2(28.0, 28.0));
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.08);
    let bg = if close { Color32::from_rgb(0xC4, 0x2B, 0x1C) } else { t.surface_hover };
    if hover > 0.0 {
        ui.painter().rect_filled(btn, t.corner_small(), bg.gamma_multiply(hover));
    }
    let fg = if close && hover > 0.5 { Color32::WHITE } else { w::mix(t.text_dim, t.text, hover) };
    ui.painter().text(btn.center(), Align2::CENTER_CENTER, icon, w::font(13.0), fg);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn button(ui: &mut Ui, t: &Theme, icon: &str, height: f32, close: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(46.0, height), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.08);
    let bg = if close { Color32::from_rgb(0xC4, 0x2B, 0x1C) } else { t.surface_hover };
    if hover > 0.0 {
        let r = if close {
            let wr = window_rounding(ui.ctx());
            egui::CornerRadius { ne: wr.ne, ..egui::CornerRadius::ZERO }
        } else {
            egui::CornerRadius::ZERO
        };
        ui.painter().rect_filled(rect, r, bg.gamma_multiply(hover));
    }
    let fg = if close && hover > 0.5 { Color32::WHITE } else { w::mix(t.text_dim, t.text, hover) };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, w::font(14.0), fg);
    resp
}

/// Invisible handles on the window edges for resizing (not when maximized).
pub fn resize_handles(ctx: &egui::Context) {
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let screen = ctx.content_rect();
    let (e, c) = (8.0, 16.0);
    let (l, r, tp, b) = (screen.left(), screen.right(), screen.top(), screen.bottom());
    // Keep clear of the minimize / maximize / close buttons in the top-right.
    let buttons = 3.0 * 46.0 + 8.0;
    let handles = [
        (Rect::from_min_max(egui::pos2(l + c, tp), egui::pos2(r - buttons, tp + e)), ResizeDirection::North, CursorIcon::ResizeVertical),
        (Rect::from_min_max(egui::pos2(l + c, b - e), egui::pos2(r - c, b)), ResizeDirection::South, CursorIcon::ResizeVertical),
        (Rect::from_min_max(egui::pos2(l, tp + c), egui::pos2(l + e, b - c)), ResizeDirection::West, CursorIcon::ResizeHorizontal),
        (Rect::from_min_max(egui::pos2(r - e, tp + 64.0), egui::pos2(r, b - c)), ResizeDirection::East, CursorIcon::ResizeHorizontal),
        (Rect::from_min_size(egui::pos2(l, tp), Vec2::splat(c)), ResizeDirection::NorthWest, CursorIcon::ResizeNwSe),
        (Rect::from_min_size(egui::pos2(l, b - c), Vec2::splat(c)), ResizeDirection::SouthWest, CursorIcon::ResizeNeSw),
        (Rect::from_min_size(egui::pos2(r - c, b - c), Vec2::splat(c)), ResizeDirection::SouthEast, CursorIcon::ResizeNwSe),
    ];
    for (i, (rect, dir, cursor)) in handles.into_iter().enumerate() {
        if !rect.is_positive() {
            continue;
        }
        egui::Area::new(Id::unique(("resize-handle", i)))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .default_size(rect.size())
            .constrain(false)
            .show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(rect.size(), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(cursor);
                }
                if resp.drag_started() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
                }
            });
    }
}

/// Subtle bottom-right resize corner grip for floating resizable windows.
pub fn resize_grip(ctx: &egui::Context, t: &Theme) {
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let rect = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique(("window-resize-grip", ctx.viewport_id()))));
    let stroke = egui::Stroke::new(1.5, t.text_faint.gamma_multiply(0.65));
    let br = rect.right_bottom();
    for offset in [4.5f32, 8.5, 12.5] {
        p.line_segment([egui::pos2(br.x - offset, br.y - 2.0), egui::pos2(br.x - 2.0, br.y - offset)], stroke);
    }
}

/// A thin border, since a frameless window has none of its own.
pub fn border(ctx: &egui::Context, t: &Theme) {
    let rounding = window_rounding(ctx);
    if rounding == egui::CornerRadius::ZERO {
        return;
    }
    let rect = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique(("window-border", ctx.viewport_id()))))
        .rect_stroke(rect, rounding, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
}

/// Resize handles drawn into `ui` itself (for child windows such as Friends &
/// Chat, where separate areas end up swallowing clicks). Call it last, so the
/// thin edge strips sit on top of the window's content.
pub fn resize_edges(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let screen = ctx.content_rect();
    let (e, c) = (5.0, 12.0);
    let (l, r, tp, b) = (screen.left(), screen.right(), screen.top(), screen.bottom());
    let buttons = 3.0 * 46.0 + 8.0;
    let handles = [
        (Rect::from_min_max(egui::pos2(l + c, tp), egui::pos2(r - buttons, tp + e)), ResizeDirection::North, CursorIcon::ResizeVertical),
        (Rect::from_min_max(egui::pos2(l + c, b - e), egui::pos2(r - c, b)), ResizeDirection::South, CursorIcon::ResizeVertical),
        (Rect::from_min_max(egui::pos2(l, tp + c), egui::pos2(l + e, b - c)), ResizeDirection::West, CursorIcon::ResizeHorizontal),
        (Rect::from_min_max(egui::pos2(r - e, tp + 40.0), egui::pos2(r, b - c)), ResizeDirection::East, CursorIcon::ResizeHorizontal),
        (Rect::from_min_size(egui::pos2(l, b - c), Vec2::splat(c)), ResizeDirection::SouthWest, CursorIcon::ResizeNeSw),
        (Rect::from_min_size(egui::pos2(r - c, b - c), Vec2::splat(c)), ResizeDirection::SouthEast, CursorIcon::ResizeNwSe),
    ];
    for (i, (rect, dir, cursor)) in handles.into_iter().enumerate() {
        let resp = ui.interact(rect, Id::unique(("resize-edge", ctx.viewport_id(), i)), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(cursor);
        }
        if resp.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, PointerButton, Pos2, RawInput, ViewportInfo, pos2};

    fn run_frame(
        ctx: &Context,
        size: Vec2,
        maximized: bool,
        events: Vec<Event>,
        btn_pos: Pos2,
    ) -> (bool, bool, CursorIcon) {
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        raw.viewports.insert(
            egui::ViewportId::ROOT,
            ViewportInfo {
                maximized: Some(maximized),
                ..Default::default()
            },
        );
        let mut hovered = false;
        let mut clicked = false;
        let mut out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let resp = ui.interact(
                    Rect::from_center_size(btn_pos, vec2(120.0, 34.0)),
                    Id::unique("test-btn"),
                    Sense::click(),
                );
                hovered = resp.hovered();
                clicked = resp.clicked();
            });
            resize_handles(ui.ctx());
        });
        out.textures_delta.clear();
        (hovered, clicked, out.platform_output.cursor_icon)
    }

    #[test]
    fn resize_handles_stay_thin_and_do_not_block_clicks() {
        let ctx = Context::default();
        // Simulate Splash -> Main -> shrink -> maximize -> unmaximize.
        let stages = [
            (vec2(380.0, 250.0), false),
            (vec2(380.0, 250.0), false),
            (vec2(1266.0, 758.0), false),
            (vec2(1266.0, 758.0), false),
            (vec2(1060.0, 680.0), false),
            (vec2(1060.0, 680.0), false),
            (vec2(1920.0, 1080.0), true),
            (vec2(1266.0, 758.0), false),
            (vec2(1266.0, 758.0), false),
        ];
        for (size, max) in stages {
            let _ = run_frame(&ctx, size, max, vec![], pos2(100.0, 450.0));
        }

        // South and East handle areas must stay within their thin edge strips,
        // not bloat to default_area_size (600x400).
        let south = ctx.memory(|m| m.area_rect(Id::unique(("resize-handle", 1)))).expect("south handle");
        let east = ctx.memory(|m| m.area_rect(Id::unique(("resize-handle", 3)))).expect("east handle");
        assert!(south.height() <= 8.0, "south resize handle bloated: {south:?}");
        assert!(east.width() <= 8.0, "east resize handle bloated: {east:?}");

        // Buttons in the bottom-left (Settings nav), bottom-center (Appearance),
        // and right half must all receive hover and clicks.
        for pos in [pos2(100.0, 450.0), pos2(600.0, 650.0), pos2(900.0, 250.0)] {
            let _ = run_frame(&ctx, vec2(1266.0, 758.0), false, vec![Event::PointerMoved(pos)], pos);
            let (hov, _, _) = run_frame(&ctx, vec2(1266.0, 758.0), false, vec![Event::PointerMoved(pos)], pos);
            assert!(hov, "button at {pos:?} was not hovered");

            let _ = run_frame(
                &ctx,
                vec2(1266.0, 758.0),
                false,
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
                ],
                pos,
            );
            let (_, clicked, _) = run_frame(
                &ctx,
                vec2(1266.0, 758.0),
                false,
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
                ],
                pos,
            );
            assert!(clicked, "button at {pos:?} was not clicked");
        }

        // Hovering the actual South edge strip still shows the vertical resize cursor.
        let edge = pos2(500.0, 756.0);
        let _ = run_frame(&ctx, vec2(1266.0, 758.0), false, vec![Event::PointerMoved(edge)], pos2(100.0, 100.0));
        let (_, _, cursor) = run_frame(&ctx, vec2(1266.0, 758.0), false, vec![Event::PointerMoved(edge)], pos2(100.0, 100.0));
        assert_eq!(cursor, CursorIcon::ResizeVertical);
    }
}
