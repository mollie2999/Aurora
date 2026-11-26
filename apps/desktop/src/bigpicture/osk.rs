//! Shared on-screen keyboard for Aurora:
//! - Compact inline desktop OSK (`draw_desktop`) for touchscreen sign-in.
//! - Translucent SteamOS-style bottom glass overlay (`draw_overlay`) for Big Picture
//!   & Steam Deck Game Mode, with physical EN/RU dual-legend layouts, toggleable
//!   Master-Keyboard utility & F1–F12 rows, controller shortcuts, isolated `Focus`,
//!   caret/clipboard editing, and dual-trackpad reticle projection.

use super::focus::{Dir, Focus, Place};
use super::paint;
use egui::{Align2, Color32, FontId, Id, Painter, Pos2, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use i18n::t;
use ui::{Theme, widgets as w};

const KEYS_EN: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm-:."];
const KEYS_EN_SHIFT: [&str; 4] = ["!@#$%^&*()", "QWERTYUIOP", "ASDFGHJKL\"", "ZXCVBNM_?!"];
const KEYS_RU: [&str; 4] = ["1234567890-", "йцукенгшщзх", "фывапролджэ", "ячсмитьбюъё"];
const KEYS_RU_SHIFT: [&str; 4] = ["!\"№;%:?*()_", "ЙЦУКЕНГШЩЗХ", "ФЫВАПРОЛДЖЭ", "ЯЧСМИТЬБЮЪЁ"];
const KEYS_SYM: [&str; 4] = ["!@#$%^&*()", "-_=+[]{};:", "/?,.<>'\"~`", "|\\€£¥°•·+*"];

const LONG_PRESS_SECS: f64 = 0.38;

#[derive(Clone, Copy)]
struct HoldState {
    id: Id,
    primary: KeyOp,
    secondary: KeyOp,
    started_at: f64,
    long_fired: bool,
}

/// Keyboard mode, caret, focus, and touchpad state shared across Big Picture screens.
pub struct OskState {
    pub cyrillic: bool,
    pub shift: bool,
    pub caps: bool,
    pub symbols: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub master_bar: bool,
    pub fn_keys_page: bool,
    /// Enable light touchpad haptic ticks when crossing OSK keys (touch click feedback is always on).
    pub hover_haptics: bool,
    pub caret: usize,
    pub selected_all: bool,
    active_target: Option<Id>,
    pub focus: Focus,
    touchpads: [client::TouchpadFinger; 2],
    pad_clicks: Vec<(usize, client::TouchpadFinger)>,
    last_hovered_key: [Option<Id>; 2],
    pad_holds: [Option<HoldState>; 2],
    pointer_hold: Option<HoldState>,
    confirm_hold: Option<HoldState>,
    confirm_down: bool,
    opened_at: Option<f64>,
    reveal_pending: bool,
    pub field_rect: Option<Rect>,
    function_key: Option<egui::Key>,
    key_pop: Option<(Id, f64)>,
}

impl Default for OskState {
    fn default() -> Self {
        Self {
            cyrillic: false,
            shift: false,
            caps: false,
            symbols: false,
            ctrl: false,
            alt: false,
            master_bar: false,
            fn_keys_page: false,
            hover_haptics: true,
            caret: 0,
            selected_all: false,
            active_target: None,
            focus: Focus::default(),
            touchpads: [client::TouchpadFinger::default(); 2],
            pad_clicks: Vec::new(),
            last_hovered_key: [None; 2],
            pad_holds: [None; 2],
            pointer_hold: None,
            confirm_hold: None,
            confirm_down: false,
            opened_at: None,
            reveal_pending: false,
            field_rect: None,
            function_key: None,
            key_pop: None,
        }
    }
}

impl OskState {
    pub fn rows(&self) -> [&'static str; 4] {
        if self.symbols {
            KEYS_SYM
        } else if self.cyrillic {
            if self.shift { KEYS_RU_SHIFT } else { KEYS_RU }
        } else if self.shift {
            KEYS_EN_SHIFT
        } else {
            KEYS_EN
        }
    }

    pub fn max_cols(&self) -> usize {
        self.rows().iter().map(|r| r.chars().count()).max().unwrap_or(10)
    }

    /// Bind the OSK cursor cleanly whenever the edited text field changes.
    pub fn sync_target(&mut self, target_id: Id, text: &str) {
        let len = text.chars().count();
        if self.active_target != Some(target_id) {
            self.active_target = Some(target_id);
            self.caret = len;
            self.selected_all = false;
            self.ctrl = false;
            self.alt = false;
            self.reveal_pending = true;
        } else {
            self.caret = self.caret.min(len);
        }
    }

    /// Route the utility row through the app's normal keyboard shortcut handler.
    pub fn queue_function_key(&mut self, action: OverlayAction) {
        if let OverlayAction::FunctionKey(n) = action {
            self.function_key = [
                egui::Key::F1, egui::Key::F2, egui::Key::F3, egui::Key::F4,
                egui::Key::F5, egui::Key::F6, egui::Key::F7, egui::Key::F8,
                egui::Key::F9, egui::Key::F10, egui::Key::F11, egui::Key::F12,
            ].get(usize::from(n.saturating_sub(1))).copied();
        }
    }

    pub fn restore_function_key(&mut self, ctx: &egui::Context) {
        if let Some(key) = self.function_key.take() {
            ctx.input_mut(|input| input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }));
        }
    }

    pub fn update_touchpads(&mut self, pads: [client::TouchpadFinger; 2], accept_clicks: bool) {
        if accept_clicks {
            for (index, &finger) in pads.iter().enumerate() {
                if finger.click && !self.touchpads[index].click {
                    self.pad_clicks.push((index, finger));
                    let side = if index == 0 {
                        client::HapticSide::Left
                    } else {
                        client::HapticSide::Right
                    };
                    client::trigger_touchpad_haptic(side, client::TouchpadHaptic::PressClick);
                }
            }
        } else {
            self.pad_clicks.clear();
            self.pad_holds = [None; 2];
            self.last_hovered_key = [None; 2];
        }
        self.touchpads = pads;
    }

    pub fn set_confirm_down(&mut self, down: bool) {
        self.confirm_down = down;
    }

    pub fn take_field_reveal(&mut self) -> Option<Rect> {
        let rect = self.field_rect?;
        std::mem::take(&mut self.reveal_pending).then_some(rect)
    }

    /// Clear transient focus so opening the overlay starts at the center home key.
    pub fn reset_focus(&mut self) {
        self.focus.current = None;
        self.pad_clicks.clear();
        self.last_hovered_key = [None; 2];
        self.pad_holds = [None; 2];
        self.pointer_hold = None;
        self.confirm_hold = None;
        self.opened_at = None;
        self.reveal_pending = true;
    }
}

/// Legacy desktop key event used by [`draw_desktop`] and [`apply_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Del,
    Clear,
    Go,
}

/// Apply a [`Key`] event directly to `target`. Returns `true` on [`Key::Go`].
pub fn apply_key(target: &mut String, key: Key, max_chars: Option<usize>, uppercase: bool) -> bool {
    match key {
        Key::Char(ch) => {
            if uppercase {
                for up in ch.to_uppercase() {
                    if max_chars.is_none_or(|m| target.chars().count() < m) {
                        target.push(up);
                    }
                }
            } else if max_chars.is_none_or(|m| target.chars().count() < m) {
                target.push(ch);
            }
            false
        }
        Key::Del => {
            target.pop();
            false
        }
        Key::Clear => {
            target.clear();
            false
        }
        Key::Go => true,
    }
}

/// Action emitted by the SteamOS bottom glass overlay keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAction {
    Char(char),
    Tab,
    BackTab,
    Backspace,
    ForwardDelete,
    CaretLeft,
    CaretRight,
    Home,
    End,
    PageUp,
    PageDown,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Submit,
    Hide,
    Escape,
    FunctionKey(u8),
}

/// Controller/keyboard inputs routed exclusively to the overlay when open.
#[derive(Default, Clone, Copy, Debug)]
pub struct OskInput {
    pub dir: Option<Dir>,
    pub confirm: bool,
    pub back: bool,
    pub x: bool,
    pub y: bool,
    pub lt: bool,
    pub rt: bool,
    pub lb: bool,
    pub rb: bool,
    pub l3: bool,
}

impl OskInput {
    pub fn handle_press(&mut self, press: super::input::Press, from_keys: bool) {
        use super::input::Press;
        match press {
            Press::Move(dir) => self.dir = Some(dir),
            Press::A if from_keys => self.rt = true,
            Press::A => self.confirm = true,
            Press::B | Press::ToggleOsk => self.back = true,
            Press::X => self.x = true,
            Press::Y => self.y = true,
            Press::Lt => self.lt = true,
            Press::Rt => self.rt = true,
            Press::Lb => self.lb = true,
            Press::Rb => self.rb = true,
            Press::L3 => self.l3 = true,
            _ => {}
        }
    }
}

/// Side effects returned by [`apply_overlay_action`] so callers can handle focus/clipboard/scroll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditOutcome {
    pub changed: bool,
    pub submitted: bool,
    pub hide: bool,
    /// `1` for Tab (next field), `-1` for Shift+Tab (previous field), `0` for none.
    pub tab_dir: i8,
    pub copy: bool,
    pub cut: bool,
    pub request_paste: bool,
    pub scroll_page: i8,
}

/// Consume a physical `Tab` (`+1`) or `Shift+Tab` (`-1`) keypress if present.
pub fn take_physical_tab(ctx: &egui::Context) -> i8 {
    let mut dir = 0i8;
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            if let egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } = event
                && !modifiers.ctrl
                && !modifiers.alt
                && !modifiers.mac_cmd
            {
                dir = if modifiers.shift { -1 } else { 1 };
                false
            } else {
                true
            }
        });
    });
    dir
}

enum PhysicalEdit {
    Insert(String),
    Backspace(bool),
    Action(OverlayAction),
}

/// Consume text-editing events in their original order and leave navigation to Big Picture.
pub fn edit_physical(
    state: &mut OskState,
    text: &mut String,
    ctx: &egui::Context,
    arrows_navigate: bool,
    guard_code: bool,
    copy_allowed: bool,
) -> bool {
    use egui::{Event, Key};
    let mut edits = Vec::new();
    ctx.input_mut(|input| input.events.retain(|event| {
        let edit = match event {
            Event::Text(text) | Event::Paste(text) => PhysicalEdit::Insert(text.clone()),
            Event::Copy => PhysicalEdit::Action(OverlayAction::Copy),
            Event::Cut => PhysicalEdit::Action(OverlayAction::Cut),
            Event::Key { key, pressed: true, modifiers, .. } => {
                let action = match key {
                    Key::Backspace => {
                        edits.push(PhysicalEdit::Backspace(modifiers.ctrl || modifiers.mac_cmd || modifiers.alt));
                        return false;
                    }
                    Key::Delete => OverlayAction::ForwardDelete,
                    Key::Home => OverlayAction::Home,
                    Key::End => OverlayAction::End,
                    Key::ArrowLeft if !arrows_navigate && modifiers.is_none() => OverlayAction::CaretLeft,
                    Key::ArrowRight if !arrows_navigate && modifiers.is_none() => OverlayAction::CaretRight,
                    Key::A if modifiers.command => OverlayAction::SelectAll,
                    _ => return true,
                };
                PhysicalEdit::Action(action)
            }
            _ => return true,
        };
        edits.push(edit);
        false
    }));

    let max_chars = guard_code.then_some(5);
    let handled = !edits.is_empty();
    for edit in edits {
        match edit {
            PhysicalEdit::Insert(insert) => {
                insert_text_at_caret(state, text, &insert, max_chars, guard_code);
            }
            PhysicalEdit::Backspace(word) => {
                delete_before_caret(state, text, word);
            }
            PhysicalEdit::Action(OverlayAction::Cut) if !copy_allowed => {}
            PhysicalEdit::Action(action) => {
                let outcome = apply_overlay_action(state, text, action, max_chars, guard_code);
                if outcome.copy && copy_allowed {
                    ctx.copy_text(text.clone());
                }
                if outcome.cut {
                    delete_before_caret(state, text, false);
                }
            }
        }
    }
    handled
}

fn char_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map_or(text.len(), |(i, _)| i)
}

/// Insert `insert` at `state.caret`, replacing any active selection and enforcing `max_chars`/`uppercase`.
pub fn insert_text_at_caret(
    state: &mut OskState,
    text: &mut String,
    insert: &str,
    max_chars: Option<usize>,
    uppercase: bool,
) -> bool {
    let mut cleaned = String::new();
    for ch in insert.chars() {
        if ch.is_control() {
            continue;
        }
        if uppercase {
            for up in ch.to_uppercase() {
                cleaned.push(up);
            }
        } else {
            cleaned.push(ch);
        }
    }
    if cleaned.is_empty() {
        return false;
    }
    if state.selected_all {
        text.clear();
        state.caret = 0;
        state.selected_all = false;
    }
    let cur_chars = text.chars().count();
    state.caret = state.caret.min(cur_chars);
    let room = max_chars.map_or(usize::MAX, |m| m.saturating_sub(cur_chars));
    let mut to_insert = String::new();
    let mut added = 0usize;
    for ch in cleaned.chars().take(room) {
        to_insert.push(ch);
        added += 1;
    }
    if added == 0 {
        return false;
    }
    let byte = char_to_byte(text, state.caret);
    text.insert_str(byte, &to_insert);
    state.caret += added;
    true
}

/// Delete one character (or one whitespace-delimited word if `word` is true) before `state.caret`.
pub fn delete_before_caret(state: &mut OskState, text: &mut String, word: bool) -> bool {
    if state.selected_all {
        state.selected_all = false;
        if text.is_empty() {
            state.caret = 0;
            return false;
        }
        text.clear();
        state.caret = 0;
        return true;
    }
    let cur_chars = text.chars().count();
    state.caret = state.caret.min(cur_chars);
    if state.caret == 0 {
        return false;
    }
    let new_caret = if word {
        let chars: Vec<char> = text.chars().collect();
        let mut i = state.caret;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    } else {
        state.caret - 1
    };
    let start_b = char_to_byte(text, new_caret);
    let end_b = char_to_byte(text, state.caret);
    text.drain(start_b..end_b);
    state.caret = new_caret;
    true
}

/// Delete the character immediately at `state.caret` (forward Delete).
pub fn delete_after_caret(state: &mut OskState, text: &mut String) -> bool {
    if state.selected_all {
        state.selected_all = false;
        if text.is_empty() {
            state.caret = 0;
            return false;
        }
        text.clear();
        state.caret = 0;
        return true;
    }
    let cur_chars = text.chars().count();
    state.caret = state.caret.min(cur_chars);
    if state.caret >= cur_chars {
        return false;
    }
    let b = char_to_byte(text, state.caret);
    text.remove(b);
    true
}

/// Apply an [`OverlayAction`] to `text` and `state`, returning high-level side effects.
pub fn apply_overlay_action(
    state: &mut OskState,
    text: &mut String,
    action: OverlayAction,
    max_chars: Option<usize>,
    uppercase: bool,
) -> EditOutcome {
    let mut out = EditOutcome::default();
    if state.ctrl && let OverlayAction::Char(ch) = action {
        state.ctrl = false;
        state.alt = false;
        state.shift = false;
        match ch.to_ascii_lowercase() {
            'a' | 'ф' | 'Ф' => {
                state.selected_all = !text.is_empty();
                state.caret = text.chars().count();
            }
            'c' | 'с' | 'С' => {
                out.copy = !text.is_empty();
            }
            'v' | 'м' | 'М' => {
                out.request_paste = true;
            }
            'x' | 'ч' | 'Ч' => {
                out.copy = state.selected_all;
                out.cut = state.selected_all;
            }
            _ => {}
        }
        return out;
    }

    match action {
        OverlayAction::Char(ch) => {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            out.changed = insert_text_at_caret(state, text, s, max_chars, uppercase);
            state.shift = false;
            state.alt = false;
        }
        OverlayAction::Tab => {
            out.tab_dir = if state.shift { -1 } else { 1 };
            state.shift = false;
        }
        OverlayAction::BackTab => {
            out.tab_dir = -1;
            state.shift = false;
        }
        OverlayAction::Backspace => {
            let word = state.ctrl || state.alt;
            state.ctrl = false;
            state.alt = false;
            out.changed = delete_before_caret(state, text, word);
        }
        OverlayAction::ForwardDelete => {
            state.alt = false;
            out.changed = delete_after_caret(state, text);
        }
        OverlayAction::CaretLeft => {
            state.selected_all = false;
            state.caret = state.caret.min(text.chars().count()).saturating_sub(1);
        }
        OverlayAction::CaretRight => {
            let len = text.chars().count();
            state.selected_all = false;
            state.caret = (state.caret.min(len) + 1).min(len);
        }
        OverlayAction::Home => {
            state.selected_all = false;
            state.caret = 0;
        }
        OverlayAction::End => {
            state.selected_all = false;
            state.caret = text.chars().count();
        }
        OverlayAction::PageUp => out.scroll_page = -1,
        OverlayAction::PageDown => out.scroll_page = 1,
        OverlayAction::SelectAll => {
            state.selected_all = !text.is_empty();
            state.caret = text.chars().count();
        }
        OverlayAction::Copy => out.copy = !text.is_empty(),
        OverlayAction::Cut => {
            out.copy = state.selected_all;
            out.cut = state.selected_all;
        }
        OverlayAction::Paste => out.request_paste = true,
        OverlayAction::Submit => out.submitted = true,
        OverlayAction::Hide | OverlayAction::Escape => out.hide = true,
        OverlayAction::FunctionKey(_) => {}
    }
    out
}

/// Draw a compact, touch-friendly desktop on-screen keyboard inside `ui`.
/// Uses the same base layers as the desktop sign-in screen.
pub fn draw_desktop(ui: &mut Ui, state: &mut OskState, t: &Theme, go_label: &str) -> Option<Key> {
    let mut out = None;
    let row_w = ui.available_width();
    let gap = 6.0;
    let top_pad = 8.0;
    let key_h = ((ui.available_height() - top_pad - 4.0 * gap) / 5.0).clamp(36.0, 42.0);
    let total_h = top_pad + 5.0 * key_h + 4.0 * gap;
    let (panel_rect, _) = ui.allocate_exact_size(vec2(row_w, total_h), egui::Sense::hover());
    ui.painter().line_segment(
        [panel_rect.left_top(), panel_rect.right_top()],
        Stroke::new(1.0, t.border.gamma_multiply(0.45)),
    );
    let start_x = panel_rect.left();
    let mut y = panel_rect.top() + top_pad;
    for (ri, row) in state.rows().iter().enumerate() {
        let cols = row.chars().count();
        let key_w = (panel_rect.width() - (cols - 1) as f32 * gap) / cols as f32;
        for (ci, ch) in row.chars().enumerate() {
            let x = start_x + ci as f32 * (key_w + gap);
            let right = if ci + 1 == cols { panel_rect.right() } else { x + key_w };
            let r = Rect::from_min_max(pos2(x, y), pos2(right, y + key_h));
            let label = ch.to_string();
            if desktop_key_cap(ui, t, Id::unique(("desk-osk-key", ri, ci)), r, &label, false, false) {
                out = Some(Key::Char(ch));
            }
        }
        y += key_h + gap;
    }

    let shift_label = if state.shift { "SHIFT" } else { "Shift" };
    let sym_label = if state.symbols { "ABC" } else { "!@#" };
    let lang_label = if state.cyrillic { "ABC" } else { "АБВ" };
    let specials: [(f32, &str, &str, bool, bool); 7] = [
        (1.4, "shift", shift_label, false, state.shift),
        (1.3, "sym", sym_label, false, state.symbols),
        (1.3, "lang", lang_label, false, state.cyrillic && !state.symbols),
        (2.4, " ", t!("Space"), false, false),
        (1.4, "del", t!("Delete"), false, false),
        (1.2, "clear", t!("Clear"), false, false),
        (2.0, "go", go_label, true, false),
    ];
    let weight_sum: f32 = specials.iter().map(|x| x.0).sum();
    let avail = row_w - gap * (specials.len() - 1) as f32;
    let mut x = start_x;
    for (i, (weight, what, label, accent, active)) in specials.iter().enumerate() {
        let right = if i + 1 == specials.len() { panel_rect.right() } else { x + avail * weight / weight_sum };
        let r = Rect::from_min_max(pos2(x, y), pos2(right, y + key_h));
        if desktop_key_cap(ui, t, Id::unique(("desk-osk-special", i)), r, label, *accent, *active) {
            match *what {
                " " => out = Some(Key::Char(' ')),
                "del" => out = Some(Key::Del),
                "clear" => out = Some(Key::Clear),
                "shift" => {
                    state.symbols = false;
                    state.shift = !state.shift;
                }
                "sym" => state.symbols = !state.symbols,
                "lang" => {
                    state.symbols = false;
                    state.cyrillic = !state.cyrillic;
                }
                _ => out = Some(Key::Go),
            }
        }
        x = right + gap;
    }
    out
}

fn desktop_key_cap(ui: &Ui, t: &Theme, id: Id, r: Rect, label: &str, accent: bool, active: bool) -> bool {
    let resp = ui.interact(r, id, egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(r) {
        let hover = ui.ctx().animate_bool_with_time(id.with("h"), resp.hovered(), 0.1);
        let pressed = resp.is_pointer_button_down_on();
        let base = if accent {
            t.accent
        } else if active {
            w::mix(t.raised, t.accent, 0.28)
        } else {
            w::mix(t.raised, t.surface, 0.25)
        };
        let fill = if accent {
            w::mix(base, Color32::WHITE, 0.12 * hover)
        } else {
            w::mix(base, t.surface_hover, hover)
        };
        let fill = if pressed { w::mix(fill, Color32::BLACK, 0.14) } else { fill };
        let border = if active && !accent {
            t.accent
        } else if accent {
            Color32::TRANSPARENT
        } else {
            w::mix(t.border.gamma_multiply(0.55), t.accent, hover * 0.55)
        };
        ui.painter().rect_filled(r, t.corner_small(), fill);
        if border != Color32::TRANSPARENT {
            ui.painter().rect_stroke(r, t.corner_small(), Stroke::new(1.0, border), StrokeKind::Inside);
        }
        let is_single = label.chars().count() == 1;
        let mut font_size = if is_single { 15.5 } else { 12.0 };
        let text_color = if accent { t.on_accent } else { t.text };
        if !is_single {
            let max_w = (r.width() - 6.0).max(8.0);
            let measured = ui.painter().layout_no_wrap(label.to_string(), w::font_semibold(font_size), text_color).size().x;
            if measured > max_w {
                font_size = (font_size * (max_w / measured)).clamp(9.5, font_size);
            }
        }
        let font = if is_single { w::font_semibold(font_size) } else { w::font_medium(font_size) };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, font, text_color);
    }
    resp.clicked()
}

/// Paint single-line text inside `inner`, scrolling horizontally to keep `caret` visible
/// and rendering a selection tint + blinking caret bar at `caret`.
#[allow(clippy::too_many_arguments)]
pub fn paint_single_line_field(
    p: &Painter,
    inner: Rect,
    text: &str,
    placeholder: &str,
    font: FontId,
    t: &Theme,
    now: f64,
    active: bool,
    caret: usize,
    selected_all: bool,
) {
    if inner.width() < 4.0 || inner.height() < 4.0 {
        return;
    }
    let clip = p.with_clip_rect(inner.expand2(vec2(4.0, 2.0)));
    let caret_x = if text.is_empty() {
        clip.text(pos2(inner.left(), inner.center().y), Align2::LEFT_CENTER, placeholder, font.clone(), t.text_faint);
        inner.left()
    } else {
        let char_len = text.chars().count();
        let caret = caret.min(char_len);
        let galley = p.layout_no_wrap(text.to_string(), font.clone(), t.text);
        let local_x = galley.pos_from_cursor(egui::text::CCursor::new(caret)).min.x.max(0.0);
        let overflow = (galley.size().x - inner.width() + 6.0).max(0.0);
        let scroll_x = if overflow <= 0.0 {
            0.0
        } else {
            (local_x - inner.width() * 0.7).clamp(0.0, overflow)
        };
        let pos = pos2(inner.left() - scroll_x, inner.center().y - galley.size().y * 0.5);
        if selected_all {
            let sel = Rect::from_min_size(pos2(pos.x, pos.y - 2.0), vec2(galley.size().x, galley.size().y + 4.0));
            clip.rect_filled(sel, paint::radius(4.0), t.accent.gamma_multiply(0.35));
        }
        clip.galley(pos, galley, t.text);
        (pos.x + local_x).clamp(inner.left(), inner.right())
    };
    if active {
        let blink = if ((now * 2.0) as u64).is_multiple_of(2) { 1.0 } else { 0.25 };
        let half = (inner.height() * 0.36).max(8.0);
        p.vline(
            caret_x + 1.0,
            (inner.center().y - half)..=(inner.center().y + half),
            Stroke::new(2.5, t.accent.gamma_multiply(blink)),
        );
    }
}

/// Project normalized `(x, y)` coordinates from left (`0`) or right (`1`) controller touchpad
/// onto the keyboard grid with a slight center overlap (`0.0..0.55` and `0.45..1.0`).
pub fn project_trackpad_pos(grid_rect: Rect, pad_idx: usize, x: f32, y: f32) -> Pos2 {
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let (u0, u1) = if pad_idx == 0 { (0.0, 0.55) } else { (0.45, 1.0) };
    pos2(
        grid_rect.left() + grid_rect.width() * (u0 + (u1 - u0) * x),
        grid_rect.top() + grid_rect.height() * y,
    )
}

#[derive(Clone, Copy)]
enum KeyOp {
    Action(OverlayAction),
    ToggleShift,
    ToggleCaps,
    ToggleLang,
    ToggleMasterBar,
    ToggleCtrl,
    ToggleAlt,
    ToggleFnPage,
}

#[derive(Clone, Copy)]
enum CapStyle {
    Char,
    Modifier,
    ActiveModifier,
    Accent,
    Utility,
}

struct RenderedCap {
    id: Id,
    rect: Rect,
    op: KeyOp,
    secondary_op: Option<KeyOp>,
    secondary_label: Option<String>,
}


const EN_ROW0: [(&str, &str); 13] = [
    ("`", "~"), ("1", "!"), ("2", "@"), ("3", "#"), ("4", "$"), ("5", "%"),
    ("6", "^"), ("7", "&"), ("8", "*"), ("9", "("), ("0", ")"), ("-", "_"), ("=", "+"),
];
const RU_ROW0: [(&str, &str); 13] = [
    ("ё", "Ё"), ("1", "!"), ("2", "\""), ("3", "№"), ("4", ";"), ("5", "%"),
    ("6", ":"), ("7", "?"), ("8", "*"), ("9", "("), ("0", ")"), ("-", "_"), ("=", "+"),
];
const EN_ROW1: [(&str, &str, f32); 13] = [
    ("q", "Q", 1.0), ("w", "W", 1.0), ("e", "E", 1.0), ("r", "R", 1.0), ("t", "T", 1.0),
    ("y", "Y", 1.0), ("u", "U", 1.0), ("i", "I", 1.0), ("o", "O", 1.0), ("p", "P", 1.0),
    ("[", "{", 1.0), ("]", "}", 1.0), ("\\", "|", 1.7),
];
const RU_ROW1: [(&str, &str, f32); 13] = [
    ("й", "Й", 1.0), ("ц", "Ц", 1.0), ("у", "У", 1.0), ("к", "К", 1.0), ("е", "Е", 1.0),
    ("н", "Н", 1.0), ("г", "Г", 1.0), ("ш", "Ш", 1.0), ("щ", "Щ", 1.0), ("з", "З", 1.0),
    ("х", "Х", 1.0), ("ъ", "Ъ", 1.0), ("\\", "|", 1.7),
];
const EN_ROW2: [(&str, &str); 11] = [
    ("a", "A"), ("s", "S"), ("d", "D"), ("f", "F"), ("g", "G"),
    ("h", "H"), ("j", "J"), ("k", "K"), ("l", "L"), (";", ":"), ("'", "\""),
];
const RU_ROW2: [(&str, &str); 11] = [
    ("ф", "Ф"), ("ы", "Ы"), ("в", "В"), ("а", "А"), ("п", "П"),
    ("р", "Р"), ("о", "О"), ("л", "Л"), ("д", "Д"), ("ж", "Ж"), ("э", "Э"),
];
const EN_ROW3: [(&str, &str); 10] = [
    ("z", "Z"), ("x", "X"), ("c", "C"), ("v", "V"), ("b", "B"),
    ("n", "N"), ("m", "M"), (",", "<"), (".", ">"), ("/", "?"),
];
const RU_ROW3: [(&str, &str); 10] = [
    ("я", "Я"), ("ч", "Ч"), ("с", "С"), ("м", "М"), ("и", "И"),
    ("т", "Т"), ("ь", "Ь"), ("б", "Б"), ("ю", "Ю"), (".", ","),
];

fn overlay_metrics(screen: Rect, s: f32, state: &OskState) -> (f32, f32, f32, f32, f32) {
    let height = screen.height().max(0.0);
    let gap = (6.0 * s).clamp(4.0, 9.0);
    let pad_y = (12.0 * s).clamp(8.0, 16.0);
    let key_h = (height * 0.08).clamp((38.0 * s).max(32.0), (78.0 * s).max(46.0));
    let master_h = if state.master_bar { (key_h * 0.72).clamp(28.0, 50.0) } else { 0.0 };
    let total_h = pad_y * 2.0
        + 5.0 * key_h
        + 4.0 * gap
        + if state.master_bar { master_h + gap * 1.2 } else { 0.0 };
    let fit = (height * 0.55 / total_h).min(1.0);
    (total_h * fit, key_h * fit, master_h * fit, gap * fit, pad_y * fit)
}

/// Height occupied by [`draw_overlay`] at the bottom of `screen`.
pub fn overlay_height(screen: Rect, s: f32, state: &OskState) -> f32 {
    overlay_metrics(screen, s, state).0
}

fn first_char(s: &str) -> char {
    s.chars().next().unwrap_or(' ')
}

fn key_legends<'a>(base: &'a str, shifted: &'a str, shift: bool, caps: bool) -> (&'a str, &'a str) {
    if shift ^ (caps && first_char(base).is_alphabetic()) {
        (shifted, base)
    } else {
        (base, shifted)
    }
}

/// Draw the SteamOS-style translucent bottom overlay keyboard and return any emitted [`OverlayAction`].
#[allow(clippy::too_many_arguments)]
pub fn draw_overlay(
    state: &mut OskState,
    ui: &Ui,
    t: &Theme,
    s: f32,
    screen: Rect,
    now: f64,
    go_label: &str,
    input: OskInput,
) -> Vec<OverlayAction> {
    if input.lt {
        state.shift = !state.shift;
    }
    if input.l3 {
        state.caps = !state.caps;
    }
    if input.back {
        state.reset_focus();
        return vec![OverlayAction::Hide];
    }
    let shortcut = if input.rt {
        Some(OverlayAction::Submit)
    } else if input.x {
        Some(OverlayAction::Backspace)
    } else if input.y {
        Some(OverlayAction::Char(' '))
    } else if input.lb {
        Some(OverlayAction::CaretLeft)
    } else if input.rb {
        Some(OverlayAction::CaretRight)
    } else {
        None
    };

    let (panel_h, key_h, master_h, gap, pad_y) = overlay_metrics(screen, s, state);
    let opened_at = *state.opened_at.get_or_insert(now);
    let progress = super::focus::ease(((now - opened_at) / 0.16).clamp(0.0, 1.0) as f32);
    if progress < 1.0 {
        ui.ctx().request_repaint();
    }
    let offset = panel_h * (1.0 - progress);
    let panel = Rect::from_min_max(pos2(screen.left(), screen.bottom() - panel_h), screen.max)
        .translate(vec2(0.0, offset));
    // A separate interaction layer keeps background widgets from taking keyboard clicks.
    egui::Area::new(Id::unique("bp-osk-overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(panel.min)
        .default_size(panel.size())
        .constrain(false)
        .fade_in(false)
        .show(ui.ctx(), |panel_ui| {
            panel_ui.set_min_size(panel.size());
            panel_ui.set_clip_rect(screen);
            draw_panel(
                state, panel_ui, t, s, screen, panel, key_h, master_h, gap, pad_y,
                now, go_label, input, shortcut,
            )
        })
        .inner
}

#[allow(clippy::too_many_arguments)]
fn draw_panel(
    state: &mut OskState,
    ui: &mut Ui,
    t: &Theme,
    s: f32,
    screen: Rect,
    panel: Rect,
    key_h: f32,
    master_h: f32,
    gap: f32,
    pad_y: f32,
    now: f64,
    go_label: &str,
    input: OskInput,
    shortcut: Option<OverlayAction>,
) -> Vec<OverlayAction> {
    let grid_w = (panel.width() - (28.0 * s).clamp(16.0, 48.0))
        .min(1680.0 * s)
        .max(0.0);
    let grid_left = panel.center().x - grid_w * 0.5;
    let keys_top = panel.top() + pad_y;
    let keys_rect = Rect::from_min_max(pos2(grid_left, keys_top), pos2(grid_left + grid_w, panel.bottom() - pad_y));

    let p = ui.painter().with_clip_rect(screen);
    let fade_top = Rect::from_min_max(pos2(screen.left(), (panel.top() - 28.0 * s).max(screen.top())), pos2(screen.right(), panel.top()));
    w::gradient(&p, fade_top, t.bg.gamma_multiply(0.0), Color32::from_rgba_unmultiplied(10, 14, 20, 140), false);
    p.rect_filled(panel, 0.0, w::mix(t.bg, Color32::from_rgb(12, 16, 22), 0.75).gamma_multiply(0.92));
    let edge = Rect::from_min_size(panel.min, vec2(panel.width(), (2.0 * s).max(1.5)));
    w::gradient(&p, edge, t.accent.gamma_multiply(0.25), t.accent.gamma_multiply(0.75), true);

    let mut panel_ui = ui.new_child(egui::UiBuilder::new().max_rect(panel));
    panel_ui.set_clip_rect(panel.intersect(screen));
    state.focus.begin(&panel_ui, input.dir, input.confirm);

    let mut caps: Vec<RenderedCap> = Vec::with_capacity(80);
    let mut op_triggered: Option<(Id, KeyOp)> = None;
    let mut pointer_down_cap: Option<Id> = None;
    let mut y = keys_top;

    // Optional Master-Keyboard utility / F1..F12 bar.
    if state.master_bar {
        paint_master_row(
            state,
            &panel_ui,
            t,
            s,
            panel,
            grid_left,
            y,
            grid_w,
            master_h,
            gap,
            now,
            &mut caps,
            &mut op_triggered,
        );
        y += master_h + gap * 1.2;
    }

    let OskState { cyrillic, caps: caps_lock, shift, master_bar, .. } = *state;

    // Helper closure-style state via method for crisp 5-row layout.
    let mut push_cap = |id: Id, rect: Rect, primary: &str, secondary: Option<&str>, badge: Option<&str>, style: CapStyle, op: KeyOp, secondary_op: Option<KeyOp>, secondary_label: Option<&str>| {
        let (activated, ptr_down) = paint_key_cap(state, &panel_ui, t, s, panel, id, rect, primary, secondary, badge, style, now);
        if activated {
            op_triggered = Some((id, op));
        }
        if ptr_down {
            pointer_down_cap = Some(id);
        }
        caps.push(RenderedCap {
            id,
            rect,
            op,
            secondary_op,
            secondary_label: secondary_label.map(str::to_string),
        });
    };

    // Row 0: 13 dual-legend number/symbol keys + X Backspace (weight 2.2)
    let r0 = if cyrillic { &RU_ROW0 } else { &EN_ROW0 };
    let w0: Vec<f32> = std::iter::repeat_n(1.0, 13).chain(std::iter::once(2.2)).collect();
    let x0 = row_rects(grid_left, y, grid_w, key_h, gap, &w0);
    for (ci, (lo, hi)) in r0.iter().enumerate() {
        let (prim, sec) = key_legends(lo, hi, shift, caps_lock);
        let op = KeyOp::Action(OverlayAction::Char(first_char(prim)));
        let sec_op = (first_char(sec) != first_char(prim)).then(|| KeyOp::Action(OverlayAction::Char(first_char(sec))));
        push_cap(Id::unique(("bp-osk-k", 0usize, ci)), x0[ci], prim, Some(sec), None, CapStyle::Char, op, sec_op, Some(sec));
    }
    push_cap(
        Id::unique(("bp-osk-k", 0usize, 13usize)),
        x0[13],
        t!("Delete"),
        None,
        Some("X"),
        CapStyle::Modifier,
        KeyOp::Action(OverlayAction::Backspace),
        None,
        None,
    );
    y += key_h + gap;

    // Row 1: Tab (1.5) + 12 keys (1.0) + backslash (1.7)
    let r1 = if cyrillic { &RU_ROW1 } else { &EN_ROW1 };
    let mut w1 = Vec::with_capacity(14);
    w1.push(1.5);
    w1.extend(r1.iter().map(|k| k.2));
    let x1 = row_rects(grid_left, y, grid_w, key_h, gap, &w1);
    push_cap(
        Id::unique(("bp-osk-k", 1usize, 0usize)),
        x1[0],
        if shift { "⇧ Tab" } else { "Tab" },
        None,
        None,
        if shift { CapStyle::ActiveModifier } else { CapStyle::Modifier },
        KeyOp::Action(OverlayAction::Tab),
        Some(KeyOp::Action(OverlayAction::BackTab)),
        Some("⇧ Tab"),
    );
    for (ci, (lo, hi, _)) in r1.iter().enumerate() {
        let (prim, sec) = key_legends(lo, hi, shift, caps_lock);
        let show_sec = lo.chars().next().is_some_and(|c| !c.is_alphabetic());
        let op = KeyOp::Action(OverlayAction::Char(first_char(prim)));
        let sec_op = (first_char(sec) != first_char(prim)).then(|| KeyOp::Action(OverlayAction::Char(first_char(sec))));
        push_cap(Id::unique(("bp-osk-k", 1usize, ci + 1)), x1[ci + 1], prim, show_sec.then_some(sec), None, CapStyle::Char, op, sec_op, Some(sec));
    }
    y += key_h + gap;

    // Row 2: Caps (1.8, L3) + 11 keys (a..l ; ') + Go/Submit (2.4, R2) -> home key ('g'/'п') is col 5
    let r2 = if cyrillic { &RU_ROW2 } else { &EN_ROW2 };
    let mut w2 = Vec::with_capacity(13);
    w2.push(1.8);
    w2.extend(std::iter::repeat_n(1.0, 11));
    w2.push(2.4);
    let x2 = row_rects(grid_left, y, grid_w, key_h, gap, &w2);
    let home_id = Id::unique(("bp-osk-k", 2usize, 5usize));
    push_cap(
        Id::unique(("bp-osk-k", 2usize, 0usize)),
        x2[0],
        "Caps",
        None,
        Some("L3"),
        if caps_lock { CapStyle::ActiveModifier } else { CapStyle::Modifier },
        KeyOp::ToggleCaps,
        None,
        None,
    );
    for (ci, (lo, hi)) in r2.iter().enumerate() {
        let (prim, sec) = key_legends(lo, hi, shift, caps_lock);
        let show_sec = lo.chars().next().is_some_and(|c| !c.is_alphabetic());
        let op = KeyOp::Action(OverlayAction::Char(first_char(prim)));
        let sec_op = (first_char(sec) != first_char(prim)).then(|| KeyOp::Action(OverlayAction::Char(first_char(sec))));
        push_cap(Id::unique(("bp-osk-k", 2usize, ci + 1)), x2[ci + 1], prim, show_sec.then_some(sec), None, CapStyle::Char, op, sec_op, Some(sec));
    }
    push_cap(
        Id::unique(("bp-osk-k", 2usize, 12usize)),
        x2[12],
        go_label,
        None,
        Some("R2"),
        CapStyle::Accent,
        KeyOp::Action(OverlayAction::Submit),
        None,
        None,
    );
    y += key_h + gap;

    // Row 3: Shift (2.6, L2) + 10 keys + Shift (2.6, L2)
    let r3 = if cyrillic { &RU_ROW3 } else { &EN_ROW3 };
    let mut w3 = Vec::with_capacity(12);
    w3.push(2.6);
    w3.extend(std::iter::repeat_n(1.0, 10));
    w3.push(2.6);
    let x3 = row_rects(grid_left, y, grid_w, key_h, gap, &w3);
    let shift_style = if shift { CapStyle::ActiveModifier } else { CapStyle::Modifier };
    push_cap(Id::unique(("bp-osk-k", 3usize, 0usize)), x3[0], "Shift", None, Some("L2"), shift_style, KeyOp::ToggleShift, None, None);
    for (ci, (lo, hi)) in r3.iter().enumerate() {
        let (prim, sec) = key_legends(lo, hi, shift, caps_lock);
        let show_sec = lo.chars().next().is_some_and(|c| !c.is_alphabetic());
        let op = KeyOp::Action(OverlayAction::Char(first_char(prim)));
        let sec_op = (first_char(sec) != first_char(prim)).then(|| KeyOp::Action(OverlayAction::Char(first_char(sec))));
        push_cap(Id::unique(("bp-osk-k", 3usize, ci + 1)), x3[ci + 1], prim, show_sec.then_some(sec), None, CapStyle::Char, op, sec_op, Some(sec));
    }
    push_cap(Id::unique(("bp-osk-k", 3usize, 11usize)), x3[11], "Shift", None, Some("L2"), shift_style, KeyOp::ToggleShift, None, None);
    y += key_h + gap;

    // Row 4: Fn (1.2) + EN/RU (1.2) + Space (6.0, Y) + Left (1.1, LB) + Right (1.1, RB) + Paste (2.2) + Hide (2.4, B)
    let w4 = [1.2, 1.2, 6.0, 1.1, 1.1, 2.2, 2.4];
    let x4 = row_rects(grid_left, y, grid_w, key_h, gap, &w4);
    push_cap(
        Id::unique(("bp-osk-k", 4usize, 0usize)),
        x4[0],
        "Fn",
        None,
        None,
        if master_bar { CapStyle::ActiveModifier } else { CapStyle::Utility },
        KeyOp::ToggleMasterBar,
        None,
        None,
    );
    push_cap(
        Id::unique(("bp-osk-k", 4usize, 1usize)),
        x4[1],
        if cyrillic { "RU" } else { "EN" },
        Some(if cyrillic { "EN" } else { "RU" }),
        None,
        if cyrillic { CapStyle::ActiveModifier } else { CapStyle::Modifier },
        KeyOp::ToggleLang,
        None,
        None,
    );
    push_cap(
        Id::unique(("bp-osk-k", 4usize, 2usize)),
        x4[2],
        t!("Space"),
        None,
        Some("Y"),
        CapStyle::Char,
        KeyOp::Action(OverlayAction::Char(' ')),
        None,
        None,
    );
    push_cap(
        Id::unique(("bp-osk-k", 4usize, 3usize)),
        x4[3],
        "◀",
        None,
        Some("LB"),
        CapStyle::Modifier,
        KeyOp::Action(OverlayAction::CaretLeft),
        None,
        None,
    );
    push_cap(
        Id::unique(("bp-osk-k", 4usize, 4usize)),
        x4[4],
        "▶",
        None,
        Some("RB"),
        CapStyle::Modifier,
        KeyOp::Action(OverlayAction::CaretRight),
        None,
        None,
    );
    push_cap(
        Id::unique(("bp-osk-k", 4usize, 5usize)),
        x4[5],
        t!("Paste"),
        None,
        None,
        CapStyle::Modifier,
        KeyOp::Action(OverlayAction::Paste),
        None,
        None,
    );
    push_cap(
        Id::unique(("bp-osk-k", 4usize, 6usize)),
        x4[6],
        t!("Hide"),
        None,
        Some("B"),
        CapStyle::Modifier,
        KeyOp::Action(OverlayAction::Hide),
        None,
        None,
    );

    // Dual-trackpad projection: project touching fingers onto keys, trigger hover haptics on key transitions, paint hover & L/R reticles.
    let tp_paint = panel_ui.painter();
    for idx in 0..2usize {
        let finger = state.touchpads[idx];
        if finger.down || finger.click {
            let pos = project_trackpad_pos(keys_rect, idx, finger.x, finger.y);
            let hovered = find_key_at(&caps, pos);
            let hovered_id = hovered.map(|c| c.id);
            if hovered_id != state.last_hovered_key[idx] {
                if hovered_id.is_some() && state.hover_haptics {
                    let side = if idx == 0 {
                        client::HapticSide::Left
                    } else {
                        client::HapticSide::Right
                    };
                    client::trigger_touchpad_haptic(side, client::TouchpadHaptic::KeyCross);
                }
                state.last_hovered_key[idx] = hovered_id;
            }
            if let Some(cap) = hovered {
                tp_paint.rect_stroke(
                    cap.rect.expand(1.5),
                    paint::radius(8.0 * s),
                    Stroke::new((2.0 * s).max(1.2), t.accent.gamma_multiply(0.85)),
                    StrokeKind::Outside,
                );
            }
            let radius = (15.0 * s).clamp(11.0, 22.0) * if finger.click { 0.85 } else { 1.0 };
            let reticle_fill = if finger.click { t.accent.gamma_multiply(0.45) } else { t.accent.gamma_multiply(0.20) };
            tp_paint.circle_filled(pos, radius, reticle_fill);
            tp_paint.circle_stroke(pos, radius, Stroke::new((2.0 * s).max(1.4), Color32::from_white_alpha(220)));
            tp_paint.text(
                pos,
                Align2::CENTER_CENTER,
                if idx == 0 { "L" } else { "R" },
                w::font_bold((11.0 * s).clamp(9.0, 14.0)),
                Color32::WHITE,
            );
        } else {
            state.last_hovered_key[idx] = None;
        }
    }

    let mut operations: Vec<(Id, KeyOp)> = Vec::new();

    // Pointer / touch long-press state machine.
    let mut suppress_pointer_click = false;
    if let Some(down_id) = pointer_down_cap {
        let same = state.pointer_hold.as_ref().is_some_and(|h| h.id == down_id);
        if !same {
            state.pointer_hold = caps
                .iter()
                .find(|c| c.id == down_id)
                .and_then(|c| c.secondary_op.map(|sec| HoldState {
                    id: c.id,
                    primary: c.op,
                    secondary: sec,
                    started_at: now,
                    long_fired: false,
                }));
        }
        if let Some(hold) = state.pointer_hold.as_mut() {
            let elapsed = now - hold.started_at;
            if !hold.long_fired && elapsed >= LONG_PRESS_SECS {
                hold.long_fired = true;
                operations.push((hold.id, hold.secondary));
                client::trigger_touchpad_haptic(client::HapticSide::Both, client::TouchpadHaptic::LongPress);
            } else if !hold.long_fired {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((LONG_PRESS_SECS - elapsed).max(0.016)));
            }
        }
    } else if let Some(hold) = state.pointer_hold.take() {
        if hold.long_fired {
            suppress_pointer_click = true;
        }
    }

    // Controller A / pointer click activation.
    if let Some((id, op)) = op_triggered {
        if input.confirm && state.confirm_down {
            if let Some(sec) = caps.iter().find(|c| c.id == id).and_then(|c| c.secondary_op) {
                state.confirm_hold = Some(HoldState {
                    id,
                    primary: op,
                    secondary: sec,
                    started_at: now,
                    long_fired: false,
                });
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(LONG_PRESS_SECS));
            } else {
                operations.push((id, op));
            }
        } else if !suppress_pointer_click {
            operations.push((id, op));
        }
    }

    // Controller A long-press progression / release.
    if let Some(mut hold) = state.confirm_hold {
        if state.confirm_down {
            let elapsed = now - hold.started_at;
            if !hold.long_fired && elapsed >= LONG_PRESS_SECS {
                hold.long_fired = true;
                operations.push((hold.id, hold.secondary));
                client::trigger_touchpad_haptic(client::HapticSide::Both, client::TouchpadHaptic::LongPress);
            } else if !hold.long_fired {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((LONG_PRESS_SECS - elapsed).max(0.016)));
            }
            state.confirm_hold = Some(hold);
        } else {
            state.confirm_hold = None;
            if !hold.long_fired {
                operations.push((hold.id, hold.primary));
            }
        }
    }

    // Dual-trackpad rising clicks & long-press holds.
    for (index, finger) in state.pad_clicks.drain(..) {
        let pos = project_trackpad_pos(keys_rect, index, finger.x, finger.y);
        if screen.contains(pos) && let Some(cap) = find_key_at(&caps, pos) {
            state.focus.current = Some(cap.id);
            if let Some(sec) = cap.secondary_op && state.touchpads[index].click {
                // If an earlier hold was still pending, commit its primary before starting the new hold.
                if let Some(prev) = state.pad_holds[index].take() && !prev.long_fired {
                    operations.push((prev.id, prev.primary));
                }
                state.pad_holds[index] = Some(HoldState {
                    id: cap.id,
                    primary: cap.op,
                    secondary: sec,
                    started_at: now,
                    long_fired: false,
                });
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(LONG_PRESS_SECS));
            } else {
                operations.push((cap.id, cap.op));
            }
        }
    }
    for idx in 0..2usize {
        if let Some(mut hold) = state.pad_holds[idx] {
            if state.touchpads[idx].click {
                let elapsed = now - hold.started_at;
                if !hold.long_fired && elapsed >= LONG_PRESS_SECS {
                    hold.long_fired = true;
                    operations.push((hold.id, hold.secondary));
                    let side = if idx == 0 {
                        client::HapticSide::Left
                    } else {
                        client::HapticSide::Right
                    };
                    client::trigger_touchpad_haptic(side, client::TouchpadHaptic::LongPress);
                } else if !hold.long_fired {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((LONG_PRESS_SECS - elapsed).max(0.016)));
                }
                state.pad_holds[idx] = Some(hold);
            } else {
                state.pad_holds[idx] = None;
                if !hold.long_fired {
                    operations.push((hold.id, hold.primary));
                }
            }
        }
    }

    // Paint floating secondary-symbol preview pill above any key currently being long-pressed.
    let active_holds = [state.pad_holds[0], state.pad_holds[1], state.pointer_hold, state.confirm_hold];
    for hold in active_holds.into_iter().flatten() {
        if let Some(cap) = caps.iter().find(|c| c.id == hold.id)
            && let Some(sec_label) = cap.secondary_label.as_deref()
        {
            let progress = ((now - hold.started_at) / LONG_PRESS_SECS).clamp(0.0, 1.0) as f32;
            let pill_w = (cap.rect.width().max(38.0 * s)).min(74.0 * s);
            let pill_h = (34.0 * s).clamp(24.0, 44.0);
            let pill = Rect::from_center_size(
                pos2(cap.rect.center().x, (cap.rect.top() - pill_h * 0.65).max(screen.top() + pill_h * 0.55)),
                vec2(pill_w, pill_h),
            );
            let pill_fill = if hold.long_fired {
                t.accent
            } else {
                w::mix(t.raised, t.accent, 0.22 + 0.35 * progress)
            };
            let pill_text = if hold.long_fired { t.on_accent } else { Color32::WHITE };
            p.rect_filled(pill, paint::radius(8.0 * s), pill_fill);
            p.rect_stroke(pill, paint::radius(8.0 * s), Stroke::new(1.5, t.accent), StrokeKind::Inside);
            if !hold.long_fired {
                let bar = Rect::from_min_size(
                    pos2(pill.left() + 4.0 * s, pill.bottom() - 4.0 * s),
                    vec2((pill.width() - 8.0 * s).max(0.0) * progress, (2.5 * s).max(2.0)),
                );
                p.rect_filled(bar, paint::radius(2.0), t.accent);
            }
            p.text(
                pill.center(),
                Align2::CENTER_CENTER,
                sec_label,
                w::font_bold((18.0 * s).clamp(13.0, 24.0)),
                pill_text,
            );
        }
    }

    state.focus.end(&[home_id], home_id);
    state.focus.paint_ring(&panel_ui, t, s, (9.0 * s).clamp(6.0, 12.0), panel);

    let mut actions: Vec<_> = shortcut.into_iter().collect();
    for (id, op) in operations {
        ui.ctx().request_repaint();
        state.key_pop = Some((id, now));
        match op {
            KeyOp::Action(action) => actions.push(action),
            KeyOp::ToggleShift => state.shift = !state.shift,
            KeyOp::ToggleCaps => state.caps = !state.caps,
            KeyOp::ToggleLang => state.cyrillic = !state.cyrillic,
            KeyOp::ToggleMasterBar => {
                state.master_bar = !state.master_bar;
                state.reveal_pending = true;
            }
            KeyOp::ToggleCtrl => state.ctrl = !state.ctrl,
            KeyOp::ToggleAlt => state.alt = !state.alt,
            KeyOp::ToggleFnPage => state.fn_keys_page = !state.fn_keys_page,
        }
    }
    actions
}

#[allow(clippy::too_many_arguments)]
fn paint_master_row(
    state: &mut OskState,
    ui: &Ui,
    t: &Theme,
    s: f32,
    panel: Rect,
    left: f32,
    y: f32,
    width: f32,
    h: f32,
    gap: f32,
    now: f64,
    caps: &mut Vec<RenderedCap>,
    op_triggered: &mut Option<(Id, KeyOp)>,
) {
    if !state.fn_keys_page {
        let items: [(&str, CapStyle, KeyOp); 11] = [
            ("Esc", CapStyle::Utility, KeyOp::Action(OverlayAction::Escape)),
            ("Ctrl", if state.ctrl { CapStyle::ActiveModifier } else { CapStyle::Utility }, KeyOp::ToggleCtrl),
            ("Alt", if state.alt { CapStyle::ActiveModifier } else { CapStyle::Utility }, KeyOp::ToggleAlt),
            ("Home", CapStyle::Utility, KeyOp::Action(OverlayAction::Home)),
            ("End", CapStyle::Utility, KeyOp::Action(OverlayAction::End)),
            ("PgUp", CapStyle::Utility, KeyOp::Action(OverlayAction::PageUp)),
            ("PgDn", CapStyle::Utility, KeyOp::Action(OverlayAction::PageDown)),
            ("Del", CapStyle::Utility, KeyOp::Action(OverlayAction::ForwardDelete)),
            ("All", CapStyle::Utility, KeyOp::Action(OverlayAction::SelectAll)),
            (t!("Copy"), CapStyle::Utility, KeyOp::Action(OverlayAction::Copy)),
            ("F1-F12", CapStyle::Modifier, KeyOp::ToggleFnPage),
        ];
        let weights = [1.0; 11];
        let rects = row_rects(left, y, width, h, gap, &weights);
        for (i, (label, style, op)) in items.into_iter().enumerate() {
            let id = Id::unique(("bp-osk-m", 0usize, i));
            if paint_key_cap(state, ui, t, s, panel, id, rects[i], label, None, None, style, now).0 {
                *op_triggered = Some((id, op));
            }
            caps.push(RenderedCap { id, rect: rects[i], op, secondary_op: None, secondary_label: None });
        }
    } else {
        let weights = [1.0; 14];
        let rects = row_rects(left, y, width, h, gap, &weights);
        let items: Vec<(String, CapStyle, KeyOp)> = std::iter::once(("Esc".to_string(), CapStyle::Utility, KeyOp::Action(OverlayAction::Escape)))
            .chain((1u8..=12).map(|n| (format!("F{n}"), CapStyle::Utility, KeyOp::Action(OverlayAction::FunctionKey(n)))))
            .chain(std::iter::once(("Fn".to_string(), CapStyle::ActiveModifier, KeyOp::ToggleFnPage)))
            .collect();
        for (i, (label, style, op)) in items.into_iter().enumerate() {
            let id = Id::unique(("bp-osk-m", 1usize, i));
            if paint_key_cap(state, ui, t, s, panel, id, rects[i], &label, None, None, style, now).0 {
                *op_triggered = Some((id, op));
            }
            caps.push(RenderedCap { id, rect: rects[i], op, secondary_op: None, secondary_label: None });
        }
    }
}

fn row_rects(left: f32, y: f32, total_w: f32, h: f32, gap: f32, weights: &[f32]) -> Vec<Rect> {
    let n = weights.len().max(1);
    let avail = (total_w - gap * (n - 1) as f32).max(n as f32);
    let sum: f32 = weights.iter().sum::<f32>().max(0.01);
    let mut x = left;
    let mut out = Vec::with_capacity(n);
    for (i, w) in weights.iter().enumerate() {
        let right = if i + 1 == n { left + total_w } else { x + avail * (*w / sum) };
        out.push(Rect::from_min_max(pos2(x, y), pos2(right, y + h)));
        x = right + gap;
    }
    out
}

fn find_key_at(caps: &[RenderedCap], pos: Pos2) -> Option<&RenderedCap> {
    caps.iter().find(|c| c.rect.contains(pos)).or_else(|| {
        caps.iter()
            .filter(|c| c.rect.distance_to_pos(pos) <= 10.0)
            .min_by(|a, b| a.rect.center().distance_sq(pos).total_cmp(&b.rect.center().distance_sq(pos)))
    })
}

#[allow(clippy::too_many_arguments)]
fn paint_key_cap(
    state: &mut OskState,
    ui: &Ui,
    t: &Theme,
    s: f32,
    clip: Rect,
    id: Id,
    rect: Rect,
    primary: &str,
    secondary: Option<&str>,
    badge: Option<&str>,
    style: CapStyle,
    now: f64,
) -> (bool, bool) {
    let hit = state.focus.item(ui, id, rect, clip.intersect(ui.clip_rect()), Place::Page);
    let ptr_down = ui.ctx().read_response(id).is_some_and(|r| r.is_pointer_button_down_on());
    if hit.activated {
        state.key_pop = Some((id, now));
    }
    let pop = state
        .key_pop
        .filter(|(k, _)| *k == id)
        .map(|(_, at)| ((now - at) / 0.14) as f32)
        .filter(|e| *e < 1.0);
    let bump = pop.map(|e| (e * std::f32::consts::PI).sin() * 0.08).unwrap_or(0.0);
    if pop.is_some() {
        ui.ctx().request_repaint();
    }
    let shown = paint::grow(rect, 1.0 + 0.03 * hit.glow - bump);
    let radius = (8.0 * s).clamp(5.0, 12.0);
    let base = match style {
        CapStyle::Char => w::mix(t.raised, Color32::from_rgb(36, 44, 58), 0.55),
        CapStyle::Modifier => w::mix(t.surface, Color32::from_rgb(24, 30, 42), 0.65),
        CapStyle::Utility => w::mix(t.surface, Color32::from_rgb(20, 26, 36), 0.72),
        CapStyle::ActiveModifier => w::mix(t.raised, t.accent, 0.36),
        CapStyle::Accent => t.accent,
    };
    let fill = if matches!(style, CapStyle::Accent) {
        w::mix(base, Color32::WHITE, hit.glow * 0.22)
    } else if hit.focused {
        w::mix(base, Color32::WHITE, 0.16)
    } else {
        w::mix(base, t.accent, hit.glow * 0.25)
    };
    let p = ui.painter();
    p.rect_filled(shown, paint::radius(radius), fill);
    let border_col = match style {
        CapStyle::Accent => Color32::TRANSPARENT,
        CapStyle::ActiveModifier => t.accent,
        _ => Color32::from_white_alpha(if hit.focused { 90 } else { 24 }),
    };
    if border_col != Color32::TRANSPARENT {
        p.rect_stroke(shown, paint::radius(radius), Stroke::new(1.0, border_col), StrokeKind::Inside);
    }

    let text_col = if matches!(style, CapStyle::Accent) {
        t.on_accent
    } else if hit.focused && matches!(style, CapStyle::Char) {
        Color32::WHITE
    } else {
        t.text
    };
    if let Some(sec) = secondary {
        let sec_size = (11.5 * s).clamp(9.0, 15.0);
        p.text(
            pos2(shown.left() + 7.0 * s, shown.top() + 5.0 * s),
            Align2::LEFT_TOP,
            sec,
            w::font_medium(sec_size),
            t.text_faint,
        );
    }
    let single = primary.chars().count() == 1;
    let mut size = if single { (22.0 * s).clamp(15.0, 28.0) } else { (15.5 * s).clamp(11.0, 20.0) };
    let badge_reserve = if badge.is_some() { 26.0 * s } else { 0.0 };
    let max_text_w = (shown.width() - 10.0 * s - badge_reserve).max(10.0);
    if !single {
        let measured = p.layout_no_wrap(primary.to_string(), w::font_semibold(size), text_col).size().x;
        if measured > max_text_w {
            size = (size * (max_text_w / measured)).clamp(9.5, size);
        }
    }
    let label_pos = if badge.is_some() && !single {
        pos2(shown.center().x + badge_reserve * 0.35, shown.center().y)
    } else if secondary.is_some() {
        pos2(shown.center().x, shown.center().y + 3.0 * s)
    } else {
        shown.center()
    };
    p.text(label_pos, Align2::CENTER_CENTER, primary, w::font_semibold(size), text_col);

    if let Some(b) = badge {
        let round = b.len() == 1;
        let bh = (20.0 * s).clamp(14.0, 24.0);
        let bw = if round { bh } else { (p.layout_no_wrap(b.to_string(), w::font_bold(11.0 * s), text_col).size().x + 10.0 * s).max(bh * 1.15) };
        let br = Rect::from_center_size(pos2(shown.left() + 7.0 * s + bw * 0.5, shown.center().y), vec2(bw, bh));
        p.rect_filled(br, paint::radius(if round { bh * 0.5 } else { 5.0 * s }), Color32::from_white_alpha(28));
        p.text(br.center(), Align2::CENTER_CENTER, b, w::font_bold((11.0 * s).clamp(9.0, 14.0)), Color32::from_white_alpha(230));
    }

    if hit.focused {
        state.focus.ring_around_clipped(shown, radius, clip);
    }
    (hit.activated, ptr_down)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osk_layers_have_four_non_empty_rows() {
        for state in [
            OskState { cyrillic: false, shift: false, symbols: false, ..Default::default() },
            OskState { cyrillic: false, shift: true, symbols: false, ..Default::default() },
            OskState { cyrillic: true, shift: false, symbols: false, ..Default::default() },
            OskState { cyrillic: true, shift: true, symbols: false, ..Default::default() },
            OskState { cyrillic: false, shift: false, symbols: true, ..Default::default() },
        ] {
            let rows = state.rows();
            assert_eq!(rows.len(), 4);
            assert!(rows.iter().all(|r| r.chars().count() >= 10));
            assert!(state.max_cols() >= 10);
        }
        assert!(OskState { cyrillic: false, shift: true, symbols: false, ..Default::default() }.rows()[1].starts_with('Q'));
        assert!(OskState { cyrillic: false, shift: false, symbols: true, ..Default::default() }.rows()[0].starts_with('!'));
    }

    #[test]
    fn apply_key_handles_chars_symbols_del_clear_and_go() {
        let mut s = String::new();
        assert!(!apply_key(&mut s, Key::Char('a'), None, false));
        assert!(!apply_key(&mut s, Key::Char('@'), None, false));
        assert!(!apply_key(&mut s, Key::Char('!'), None, false));
        assert!(!apply_key(&mut s, Key::Char(' '), None, false));
        assert!(!apply_key(&mut s, Key::Char('Z'), None, false));
        assert_eq!(s, "a@! Z");
        assert!(!apply_key(&mut s, Key::Del, None, false));
        assert_eq!(s, "a@! ");
        assert!(apply_key(&mut s, Key::Go, None, false));
        assert_eq!(s, "a@! ");
        assert!(!apply_key(&mut s, Key::Clear, None, false));
        assert!(s.is_empty());
        assert!(!apply_key(&mut s, Key::Del, None, false));
        assert!(s.is_empty());
    }

    #[test]
    fn apply_key_enforces_guard_code_uppercase_and_limit() {
        let mut code = String::new();
        for ch in ['a', 'b', '3', 'c', 'd', 'e'] {
            assert!(!apply_key(&mut code, Key::Char(ch), Some(5), true));
        }
        assert_eq!(code, "AB3CD");
        assert!(!apply_key(&mut code, Key::Del, Some(5), true));
        assert_eq!(code, "AB3C");
        assert!(!apply_key(&mut code, Key::Char('z'), Some(5), true));
        assert_eq!(code, "AB3CZ");
        assert!(apply_key(&mut code, Key::Go, Some(5), true));
    }

    #[test]
    fn desktop_key_rows_align_with_sign_in_content() {
        let ctx = egui::Context::default();
        let theme = Theme::default();
        for window_width in [600.0, 720.0] {
            let screen = Rect::from_min_max(pos2(44.0, 0.0), pos2(window_width - 44.0, 260.0));
            for mut state in [
                OskState::default(),
                OskState { cyrillic: true, ..Default::default() },
                OskState { symbols: true, ..Default::default() },
            ] {
                let mut frame = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                    let bounds = ui.available_rect_before_wrap().x_range();
                    assert!(draw_desktop(ui, &mut state, &theme, "Continue").is_none());
                    let rect = |id| ctx.read_response(id).unwrap().rect;
                    for (ri, row) in state.rows().iter().enumerate() {
                        assert_eq!(rect(Id::unique(("desk-osk-key", ri, 0usize))).left(), bounds.min);
                        assert_eq!(rect(Id::unique(("desk-osk-key", ri, row.chars().count() - 1))).right(), bounds.max);
                    }
                    assert_eq!(rect(Id::unique(("desk-osk-special", 0usize))).left(), bounds.min);
                    assert_eq!(rect(Id::unique(("desk-osk-special", 6usize))).right(), bounds.max);
                });
                frame.textures_delta.clear();
            }
        }
    }

    #[test]
    fn caret_edits_support_mid_insert_word_delete_forward_del_and_unicode() {
        let mut state = OskState::default();
        let mut text = String::from("привет мир");
        state.sync_target(Id::unique("field"), &text);
        assert_eq!(state.caret, 10);

        // Home, move right 3 chars, insert exclamation.
        apply_overlay_action(&mut state, &mut text, OverlayAction::Home, None, false);
        assert_eq!(state.caret, 0);
        for _ in 0..3 {
            apply_overlay_action(&mut state, &mut text, OverlayAction::CaretRight, None, false);
        }
        assert!(insert_text_at_caret(&mut state, &mut text, "!", None, false));
        assert_eq!(text, "при!вет мир");
        assert_eq!(state.caret, 4);

        // Forward delete removes 'в'.
        assert!(delete_after_caret(&mut state, &mut text));
        assert_eq!(text, "при!ет мир");

        // End and word-backspace deletes "мир".
        apply_overlay_action(&mut state, &mut text, OverlayAction::End, None, false);
        assert!(delete_before_caret(&mut state, &mut text, true));
        assert_eq!(text, "при!ет ");
        assert!(delete_before_caret(&mut state, &mut text, true));
        assert_eq!(text, "");
    }

    #[test]
    fn ctrl_shortcuts_and_shift_vs_caps_behave_cleanly() {
        let mut state = OskState { shift: true, ..Default::default() };
        let mut text = String::new();
        // One-shot Shift clears after typing one character when Caps is off.
        apply_overlay_action(&mut state, &mut text, OverlayAction::Char('H'), None, false);
        assert!(!state.shift);
        // Caps stays active across multiple characters.
        state.caps = true;
        apply_overlay_action(&mut state, &mut text, OverlayAction::Char('i'), None, true);
        assert!(state.caps);
        assert_eq!(text, "HI");
        assert_eq!(key_legends("1", "!", false, true).0, "1");
        assert_eq!(key_legends("1", "!", true, true).0, "!");
        assert_eq!(key_legends("a", "A", false, true).0, "A");
        assert_eq!(key_legends("а", "А", true, true).0, "а");

        // Ctrl+A selects all, Ctrl+C requests copy, next insert replaces selection.
        state.ctrl = true;
        apply_overlay_action(&mut state, &mut text, OverlayAction::Char('a'), None, false);
        assert!(!state.ctrl);
        assert!(state.selected_all);
        state.ctrl = true;
        let cpy = apply_overlay_action(&mut state, &mut text, OverlayAction::Char('c'), None, false);
        assert!(cpy.copy);
        let ins = apply_overlay_action(&mut state, &mut text, OverlayAction::Char('z'), None, false);
        assert!(ins.changed);
        assert_eq!(text, "z");
        assert!(!state.selected_all);

        // Ctrl+V requests paste; Ctrl+X copies and clears.
        state.ctrl = true;
        let pst = apply_overlay_action(&mut state, &mut text, OverlayAction::Char('v'), None, false);
        assert!(pst.request_paste);
        state.selected_all = true;
        state.ctrl = true;
        let cut = apply_overlay_action(&mut state, &mut text, OverlayAction::Char('x'), None, false);
        assert!(cut.copy && cut.cut);
        assert_eq!(text, "z");
    }

    #[test]
    fn physical_edits_keep_event_order() {
        use egui::{Event, Key, Modifiers};
        let ctx = egui::Context::default();
        let mut state = OskState::default();
        let mut text = String::from("ab");
        state.sync_target(Id::unique("field"), &text);
        ctx.input_mut(|input| {
            input.events = vec![
                Event::Key {
                    key: Key::Home,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                },
                Event::Paste("z".into()),
            ];
        });
        assert!(edit_physical(&mut state, &mut text, &ctx, false, false, true));
        assert_eq!(text, "zab");
        assert_eq!(state.caret, 1);
        assert!(ctx.input(|input| input.events.is_empty()));
    }

    #[test]
    fn guard_paste_is_uppercase_and_limited_to_five_characters() {
        let ctx = egui::Context::default();
        let mut state = OskState::default();
        let mut text = String::new();
        ctx.input_mut(|input| input.events.push(egui::Event::Paste("ab\n12cd".into())));
        assert!(edit_physical(&mut state, &mut text, &ctx, true, true, true));
        assert_eq!(text, "AB12C");
    }

    #[test]
    fn trackpad_projection_maps_overlapping_halves() {
        let grid = Rect::from_min_max(pos2(100.0, 400.0), pos2(1100.0, 750.0));
        let l0 = project_trackpad_pos(grid, 0, 0.0, 0.0);
        let l1 = project_trackpad_pos(grid, 0, 1.0, 1.0);
        let r0 = project_trackpad_pos(grid, 1, 0.0, 0.0);
        let r1 = project_trackpad_pos(grid, 1, 1.0, 1.0);
        assert!((l0.x - 100.0).abs() < 0.01 && (l0.y - 400.0).abs() < 0.01);
        assert!((l1.x - 650.0).abs() < 0.01 && (l1.y - 750.0).abs() < 0.01);
        assert!((r0.x - 550.0).abs() < 0.01 && (r1.x - 1100.0).abs() < 0.01);
    }

    #[test]
    fn draw_overlay_preserves_trackpad_clicks_between_frames() {
        let ctx = egui::Context::default();
        let theme = Theme::default();
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(1280.0, 800.0));
        let mut state = OskState::default();
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            let actions = draw_overlay(
                &mut state, ui, &theme, 0.72, screen, 1.0, "Search",
                OskInput { x: true, ..Default::default() },
            );
            assert_eq!(actions, [OverlayAction::Backspace]);
        }).textures_delta.clear();

        let contact = client::TouchpadFinger { down: true, x: 0.5, y: 0.5, click: false };
        let clicked = client::TouchpadFinger { click: true, ..contact };
        state.update_touchpads([contact; 2], true);
        state.update_touchpads([clicked; 2], true);
        state.update_touchpads([clicked; 2], true);
        state.update_touchpads([contact; 2], true);
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            let actions = draw_overlay(
                &mut state, ui, &theme, 0.72, screen, 1.2, "Search", OskInput::default(),
            );
            assert_eq!(actions.len(), 2);
            assert!(actions.iter().all(|action| matches!(action, OverlayAction::Char(_))));
        }).textures_delta.clear();
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            let actions = draw_overlay(
                &mut state, ui, &theme, 0.72, screen, 1.3, "Search", OskInput::default(),
            );
            assert!(actions.is_empty());
        }).textures_delta.clear();
    }

    #[test]
    fn trackpad_long_press_emits_secondary_symbol_once() {
        let ctx = egui::Context::default();
        let theme = Theme::default();
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(1280.0, 800.0));
        let mut state = OskState::default();

        // Open the overlay first so the slide-in animation settles before pad clicks arrive.
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            assert!(draw_overlay(&mut state, ui, &theme, 0.72, screen, 1.0, "Search", OskInput::default()).is_empty());
        }).textures_delta.clear();

        // Touch and hold left pad near top-left ('1' / '!' key in Row 0).
        let contact = client::TouchpadFinger { down: true, x: 0.16, y: 0.08, click: false };
        let clicked = client::TouchpadFinger { click: true, ..contact };
        state.update_touchpads([contact, Default::default()], true);
        state.update_touchpads([clicked, Default::default()], true);

        // Initial press frame starts the hold without firing immediately.
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            let actions = draw_overlay(&mut state, ui, &theme, 0.72, screen, 1.2, "Search", OskInput::default());
            assert!(actions.is_empty());
        }).textures_delta.clear();

        // Holding past LONG_PRESS_SECS fires the secondary symbol ('!').
        let mut long_actions = Vec::new();
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            long_actions = draw_overlay(&mut state, ui, &theme, 0.72, screen, 1.2 + LONG_PRESS_SECS + 0.05, "Search", OskInput::default());
        }).textures_delta.clear();
        assert_eq!(long_actions, [OverlayAction::Char('!')]);

        // Releasing after long press does NOT fire the primary character.
        state.update_touchpads([contact, Default::default()], true);
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            let actions = draw_overlay(&mut state, ui, &theme, 0.72, screen, 1.8, "Search", OskInput::default());
            assert!(actions.is_empty());
        }).textures_delta.clear();
    }

    #[test]
    fn tab_and_shift_tab_emit_signed_direction_and_clear_shift() {
        let mut state = OskState::default();
        let mut text = String::from("abc");
        let fwd = apply_overlay_action(&mut state, &mut text, OverlayAction::Tab, None, false);
        assert_eq!(fwd.tab_dir, 1);

        state.shift = true;
        let back = apply_overlay_action(&mut state, &mut text, OverlayAction::Tab, None, false);
        assert_eq!(back.tab_dir, -1);
        assert!(!state.shift);

        let explicit_back = apply_overlay_action(&mut state, &mut text, OverlayAction::BackTab, None, false);
        assert_eq!(explicit_back.tab_dir, -1);

        let ctx = egui::Context::default();
        ctx.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            });
        });
        assert_eq!(take_physical_tab(&ctx), -1);
        assert!(ctx.input(|i| i.events.is_empty()));
    }
}
