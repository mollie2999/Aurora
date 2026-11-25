//! Controller and keyboard input for Big Picture, turned into the same few
//! presses: a direction, A (select), B (back), X, Y, LB/RB (tabs), Start
//! (menu), Guide and Quick Access (Guide + A).
//!
//! Controllers come from Aurora's single unified SDL3 host reader in
//! `client::pads` on both Windows and Linux (no `gilrs` split-brain).

use client::LauncherPadFrame;
use egui::{Key, Modifiers};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::focus::Dir;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Press {
    Move(Dir),
    A,
    B,
    X,
    Y,
    Lb,
    Rb,
    Lt,
    Rt,
    L3,
    R3,
    /// Right-stick vertical scroll tick: positive scrolls down, negative scrolls up.
    Scroll(i8),
    Start,
    Guide,
    QuickAccess,
    /// Guide + X (or hardware keyboard toggle): show/hide the overlay OSK.
    ToggleOsk,
}

/// What the last input came from: hints show buttons or keys to match.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Device {
    #[default]
    Keyboard,
    Pad,
}

pub enum PadEvent {
    Press(Press),
    /// Live touchpad contacts (`[left, right]` on dual-pad devices such as Steam Deck).
    Touchpads([client::TouchpadFinger; 2]),
    /// How many controllers are connected now.
    Connected(usize),
}

// XInput / client::pads button bits.
const BTN_DPAD_UP: u16 = 0x0001;
const BTN_DPAD_DOWN: u16 = 0x0002;
const BTN_DPAD_LEFT: u16 = 0x0004;
const BTN_DPAD_RIGHT: u16 = 0x0008;
const BTN_START: u16 = 0x0010;
const BTN_LSTICK: u16 = 0x0040;
const BTN_RSTICK: u16 = 0x0080;
const BTN_LB: u16 = 0x0100;
const BTN_RB: u16 = 0x0200;
const BTN_GUIDE: u16 = 0x0400;
const BTN_A: u16 = 0x1000;
const BTN_B: u16 = 0x2000;
const BTN_X: u16 = 0x4000;
const BTN_Y: u16 = 0x8000;

/// Start reading controllers from Aurora's unified SDL3 host service.
/// Each press wakes the UI.
pub fn spawn(ctx: egui::Context) -> mpsc::Receiver<PadEvent> {
    let (tx, rx) = mpsc::channel();
    let frames = client::subscribe_launcher_pads();
    let spawned = std::thread::Builder::new().name("gamepads".into()).spawn(move || read_pads(frames, tx, ctx));
    if let Err(e) = spawned {
        tracing::warn!("controllers: {e}");
    }
    rx
}

/// The controller settings, read by the thread that translates pad frames.
static SWAP_AB: AtomicBool = AtomicBool::new(false);
static REPEAT_LEVEL: AtomicU8 = AtomicU8::new(1);
static STICK_LEVEL: AtomicU8 = AtomicU8::new(1);

/// Apply the controller settings.
pub fn tune(swap_ab: bool, repeat: u8, stick: u8) {
    SWAP_AB.store(swap_ab, Ordering::Relaxed);
    REPEAT_LEVEL.store(repeat, Ordering::Relaxed);
    STICK_LEVEL.store(stick, Ordering::Relaxed);
}

/// A held direction repeats like a held key: once, then after a pause,
/// steadily. (First repeat, then every…) for slow, normal and fast.
fn repeat_times(level: u8) -> (Duration, Duration) {
    let (first, every) = match level {
        0 => (500, 200),
        2 => (280, 70),
        _ => (380, 120),
    };
    (Duration::from_millis(first), Duration::from_millis(every))
}

/// How far the stick goes to start a move, and to end it (with a gap so a
/// stick near the edge doesn't flicker), for low, normal and high sensitivity.
fn stick_edges(level: u8) -> (f32, f32) {
    match level {
        0 => (0.72, 0.5),
        2 => (0.4, 0.25),
        _ => (0.55, 0.35),
    }
}

/// What a face or menu button bit means. `swap`: for pads with A and B the other
/// way round, the right button confirms and the bottom one goes back.
fn button_press(bit: u16, swap: bool) -> Option<Press> {
    Some(match bit {
        BTN_A if swap => Press::B,
        BTN_B if swap => Press::A,
        BTN_A => Press::A,
        BTN_B => Press::B,
        BTN_X => Press::X,
        BTN_Y => Press::Y,
        BTN_LB => Press::Lb,
        BTN_RB => Press::Rb,
        BTN_LSTICK => Press::L3,
        BTN_RSTICK => Press::R3,
        BTN_START => Press::Start,
        BTN_GUIDE => Press::Guide,
        _ => return None,
    })
}

const TRACKED_BUTTONS: [u16; 10] = [
    BTN_GUIDE,
    BTN_A,
    BTN_B,
    BTN_X,
    BTN_Y,
    BTN_LB,
    BTN_RB,
    BTN_LSTICK,
    BTN_RSTICK,
    BTN_START,
];

#[derive(Default)]
struct HeldDirections {
    prev_buttons: u16,
    prev_misc: bool,
    dpad: [bool; 4],
    stick: [bool; 4],
    next: [Option<Instant>; 4],
    /// Right stick Y scroll direction (-1 up, 0 neutral, 1 down) and repeat timer.
    scroll_dir: i8,
    next_scroll: Option<Instant>,
    /// Analog trigger latching with hysteresis.
    lt_down: bool,
    rt_down: bool,
    /// Guide button held: Guide + A emits QuickAccess, Guide + X emits ToggleOsk.
    guide_down: bool,
    guide_chorded: bool,
}

const SCROLL_REPEAT: Duration = Duration::from_millis(65);
const TRIGGER_PRESS_THRESHOLD: f32 = 0.45;
const TRIGGER_RELEASE_THRESHOLD: f32 = 0.20;

impl HeldDirections {
    fn update_sample(&mut self, sample: &client::LauncherPadSample, out: &mut Vec<Press>) {
        // Button edges (Guide processed first so Guide+A / Guide+X chording works in one frame).
        let changed = self.prev_buttons ^ sample.buttons;
        if changed != 0 {
            for bit in TRACKED_BUTTONS {
                if changed & bit != 0 {
                    let pressed = sample.buttons & bit != 0;
                    if let Some(p) = self.update_button(bit, pressed) {
                        out.push(p);
                    }
                }
            }
            self.prev_buttons = sample.buttons;
        }
        if sample.misc && !self.prev_misc {
            out.push(Press::QuickAccess);
        }
        self.prev_misc = sample.misc;

        // D-pad.
        self.dpad[0] = sample.buttons & BTN_DPAD_UP != 0;
        self.dpad[1] = sample.buttons & BTN_DPAD_DOWN != 0;
        self.dpad[2] = sample.buttons & BTN_DPAD_LEFT != 0;
        self.dpad[3] = sample.buttons & BTN_DPAD_RIGHT != 0;

        // Left stick (+Y is up).
        let (on, off) = stick_edges(STICK_LEVEL.load(Ordering::Relaxed));
        self.stick[0] = sample.ly > if self.stick[0] { off } else { on };
        self.stick[1] = -sample.ly > if self.stick[1] { off } else { on };
        self.stick[3] = sample.lx > if self.stick[3] { off } else { on };
        self.stick[2] = -sample.lx > if self.stick[2] { off } else { on };

        // Right stick vertical scroll (+Y is up, negative scrolls down).
        let prev = self.scroll_dir;
        let (son, soff) = (0.45, 0.25);
        self.scroll_dir = if sample.ry < -if prev == 1 { soff } else { son } {
            1
        } else if sample.ry > if prev == -1 { soff } else { son } {
            -1
        } else {
            0
        };

        // Analog triggers.
        let was_lt = self.lt_down;
        self.lt_down = sample.lt > if was_lt { TRIGGER_RELEASE_THRESHOLD } else { TRIGGER_PRESS_THRESHOLD };
        if self.lt_down && !was_lt {
            out.push(Press::Lt);
        }
        let was_rt = self.rt_down;
        self.rt_down = sample.rt > if was_rt { TRIGGER_RELEASE_THRESHOLD } else { TRIGGER_PRESS_THRESHOLD };
        if self.rt_down && !was_rt {
            out.push(Press::Rt);
        }
    }

    fn update_button(&mut self, bit: u16, pressed: bool) -> Option<Press> {
        let swap = SWAP_AB.load(Ordering::Relaxed);
        if bit == BTN_GUIDE {
            if pressed {
                self.guide_down = true;
                self.guide_chorded = false;
                return None;
            }
            let emit_guide = self.guide_down && !self.guide_chorded;
            self.guide_down = false;
            self.guide_chorded = false;
            return emit_guide.then_some(Press::Guide);
        }
        if self.guide_down {
            let mapped = button_press(bit, swap);
            if mapped == Some(Press::A) || mapped == Some(Press::X) {
                if pressed {
                    self.guide_chorded = true;
                    return Some(if mapped == Some(Press::A) {
                        Press::QuickAccess
                    } else {
                        Press::ToggleOsk
                    });
                }
                return None;
            }
        }
        pressed.then(|| button_press(bit, swap)).flatten()
    }

    fn due(&mut self, now: Instant, first: Duration, repeat: Duration) -> impl Iterator<Item = Dir> + '_ {
        [Dir::Up, Dir::Down, Dir::Left, Dir::Right].into_iter().enumerate().filter_map(move |(i, dir)| {
            if !self.dpad[i] && !self.stick[i] {
                self.next[i] = None;
                return None;
            }
            match self.next[i] {
                None => self.next[i] = Some(now + first),
                Some(at) if now >= at => self.next[i] = Some(now + repeat),
                Some(_) => return None,
            }
            Some(dir)
        })
    }

    fn due_scroll(&mut self, now: Instant) -> Option<i8> {
        if self.scroll_dir == 0 {
            self.next_scroll = None;
            return None;
        }
        match self.next_scroll {
            None => {
                self.next_scroll = Some(now + SCROLL_REPEAT);
                Some(self.scroll_dir)
            }
            Some(at) if now >= at => {
                self.next_scroll = Some(now + SCROLL_REPEAT);
                Some(self.scroll_dir)
            }
            Some(_) => None,
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.next.into_iter().flatten().chain(self.next_scroll).min()
    }

    fn has_active_hold(&self) -> bool {
        self.dpad.iter().any(|b| *b) || self.stick.iter().any(|b| *b) || self.scroll_dir != 0
    }
}

fn next_poll_timeout(held: &HashMap<u32, HeldDirections>, now: Instant) -> Duration {
    let earliest = held.values().filter_map(HeldDirections::next_deadline).min();
    if let Some(deadline) = earliest {
        return deadline.saturating_duration_since(now).clamp(Duration::from_millis(5), Duration::from_millis(50));
    }
    if held.values().any(HeldDirections::has_active_hold) {
        return Duration::from_millis(16);
    }
    // Idle: wake immediately on any incoming pad frame from client::pads.
    Duration::from_millis(250)
}

fn read_pads(frames: mpsc::Receiver<LauncherPadFrame>, tx: mpsc::Sender<PadEvent>, ctx: egui::Context) {
    let send = |e: PadEvent| {
        let ok = tx.send(e).is_ok();
        if ok {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        ok
    };
    let mut held = HashMap::<u32, HeldDirections>::new();
    let mut connected: Option<usize> = None;
    let mut last_touchpads = [client::TouchpadFinger::default(); 2];
    let mut presses = Vec::new();
    loop {
        let wait = next_poll_timeout(&held, Instant::now());
        match frames.recv_timeout(wait) {
            Ok(mut frame) => {
                // Drain to the latest frame, processing button edges in order.
                loop {
                    if connected != Some(frame.connected) {
                        connected = Some(frame.connected);
                        if !send(PadEvent::Connected(frame.connected)) {
                            return;
                        }
                    }
                    // Drop disconnected pads.
                    held.retain(|id, _| frame.pads.iter().any(|p| p.id == *id));
                    presses.clear();
                    for sample in &frame.pads {
                        let state = held.entry(sample.id).or_default();
                        state.update_sample(sample, &mut presses);
                    }
                    for p in presses.drain(..) {
                        if !send(PadEvent::Press(p)) {
                            return;
                        }
                    }
                    let touchpads = frame
                        .pads
                        .iter()
                        .find(|p| p.touchpads.iter().any(|t| t.down || t.click))
                        .map(|p| p.touchpads)
                        .unwrap_or_default();
                    if touchpads != last_touchpads {
                        last_touchpads = touchpads;
                        if !send(PadEvent::Touchpads(touchpads)) {
                            return;
                        }
                    }
                    match frames.try_recv() {
                        Ok(next) => frame = next,
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let (first, repeat) = repeat_times(REPEAT_LEVEL.load(Ordering::Relaxed));
        let now = Instant::now();
        for state in held.values_mut() {
            for dir in state.due(now, first, repeat) {
                if !send(PadEvent::Press(Press::Move(dir))) {
                    return;
                }
            }
            if let Some(scroll) = state.due_scroll(now) {
                if !send(PadEvent::Press(Press::Scroll(scroll))) {
                    return;
                }
            }
        }
    }
}

/// Which keys stand for which presses. `typing`: letters go to a text box,
/// so only keys that aren't text count. Back is Esc only, like Steam's Big
/// Picture: Backspace deletes in a text box (see [`backspace`]) and does
/// nothing anywhere else, so deleting can never leave a page.
pub fn key_map(typing: bool) -> Vec<(Key, Press)> {
    let mut map = vec![
        (Key::ArrowUp, Press::Move(Dir::Up)),
        (Key::ArrowDown, Press::Move(Dir::Down)),
        (Key::ArrowLeft, Press::Move(Dir::Left)),
        (Key::ArrowRight, Press::Move(Dir::Right)),
        (Key::Enter, Press::A),
        (Key::Escape, Press::B),
        (Key::PageUp, Press::Lb),
        (Key::PageDown, Press::Rb),
        (Key::F1, Press::Start),
        (Key::F2, Press::QuickAccess),
        (Key::Home, Press::Guide),
    ];
    if !typing {
        map.extend([
            (Key::Space, Press::A),
            (Key::Q, Press::Lb),
            (Key::E, Press::Rb),
            (Key::X, Press::X),
            (Key::Y, Press::Y),
            (Key::M, Press::Start),
            (Key::OpenBracket, Press::Lt),
            (Key::CloseBracket, Press::Rt),
            // WASD moves too.
            (Key::W, Press::Move(Dir::Up)),
            (Key::S, Press::Move(Dir::Down)),
            (Key::A, Press::Move(Dir::Left)),
            (Key::D, Press::Move(Dir::Right)),
        ]);
    }
    map
}

/// Backspace in a text box: delete the last character (with Ctrl, the last
/// word). An empty box stays empty; it never means "back".
pub fn backspace(field: &mut String, word: bool) {
    if word {
        let kept = field.trim_end().len();
        field.truncate(kept);
        let cut = field.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
        field.truncate(cut);
    } else {
        field.pop();
    }
}

/// This frame's keyboard presses (taken, so egui doesn't also act on them).
pub fn keyboard(ctx: &egui::Context, typing: bool) -> Vec<Press> {
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        for (key, press) in key_map(typing) {
            if i.consume_key(Modifiers::NONE, key) {
                out.push(press);
            }
        }
        if i.consume_key(Modifiers::SHIFT, Key::Tab) {
            out.push(Press::Lb);
        }
        if i.consume_key(Modifiers::NONE, Key::Tab) {
            out.push(Press::Rb);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swapping_a_and_b() {
        assert_eq!(button_press(BTN_A, false), Some(Press::A));
        assert_eq!(button_press(BTN_B, false), Some(Press::B));
        assert_eq!(button_press(BTN_A, true), Some(Press::B));
        assert_eq!(button_press(BTN_B, true), Some(Press::A));
        // Only A and B swap.
        assert_eq!(button_press(BTN_X, true), Some(Press::X));
        // Guide works uniformly across both Windows and Linux.
        assert_eq!(button_press(BTN_GUIDE, true), Some(Press::Guide));
        assert_eq!(button_press(BTN_DPAD_UP, false), None);
    }

    #[test]
    fn controller_levels_keep_their_order() {
        let (slow, normal, fast) = (repeat_times(0), repeat_times(1), repeat_times(2));
        assert!(slow.1 > normal.1 && normal.1 > fast.1);
        assert!(slow.0 > normal.0 && normal.0 > fast.0);
        for level in 0..3 {
            let (on, off) = stick_edges(level);
            assert!(on > off && off > 0.0 && on < 1.0, "the stick needs a gap between on and off");
        }
        assert!(stick_edges(0).0 > stick_edges(1).0 && stick_edges(1).0 > stick_edges(2).0);
    }

    #[test]
    fn short_tap_fires_and_release_cancels_repeat() {
        let mut held = HeldDirections::default();
        let now = Instant::now();
        let (first, repeat) = repeat_times(1);
        held.dpad[2] = true;
        assert_eq!(held.due(now, first, repeat).collect::<Vec<_>>(), [Dir::Left]);
        held.dpad[2] = false;
        assert!(held.due(now + first, first, repeat).next().is_none());
        held.dpad[2] = true;
        assert_eq!(held.due(now + first, first, repeat).collect::<Vec<_>>(), [Dir::Left]);
        assert_eq!(held.due(now + first + first, first, repeat).collect::<Vec<_>>(), [Dir::Left]);
    }

    #[test]
    fn backspace_never_goes_back() {
        for typing in [false, true] {
            let map = key_map(typing);
            assert!(!map.iter().any(|(k, _)| *k == Key::Backspace), "Backspace mapped (typing: {typing})");
            assert!(map.iter().any(|(k, p)| *k == Key::Escape && *p == Press::B), "Esc must be back (typing: {typing})");
        }
    }

    #[test]
    fn typing_keeps_letters_for_the_text_box() {
        let map = key_map(true);
        for key in [Key::Space, Key::Q, Key::E, Key::X, Key::Y, Key::M, Key::W, Key::A, Key::S, Key::D] {
            assert!(!map.iter().any(|(k, _)| *k == key), "{key:?} must type, not act");
        }
    }

    #[test]
    fn backspace_deletes_and_stops_at_empty() {
        let mut s = String::from("hi");
        backspace(&mut s, false);
        assert_eq!(s, "h");
        backspace(&mut s, false);
        backspace(&mut s, false);
        backspace(&mut s, false);
        assert_eq!(s, "");
        let mut s = String::from("see you tonight  ");
        backspace(&mut s, true);
        assert_eq!(s, "see you ");
        backspace(&mut s, true);
        assert_eq!(s, "see ");
        backspace(&mut s, true);
        assert_eq!(s, "");
        backspace(&mut s, true);
        assert_eq!(s, "");
    }

    #[test]
    fn guide_chord_emits_quick_access_without_guide_or_a() {
        SWAP_AB.store(false, Ordering::Relaxed);
        let mut held = HeldDirections::default();
        let mut out = Vec::new();
        // Standalone Guide emits on release.
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: BTN_GUIDE, ..Default::default() }, &mut out);
        assert!(out.is_empty());
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: 0, ..Default::default() }, &mut out);
        assert_eq!(out, [Press::Guide]);
        out.clear();

        // Guide + A emits QuickAccess immediately and suppresses Guide on release.
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: BTN_GUIDE, ..Default::default() }, &mut out);
        assert!(out.is_empty());
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: BTN_GUIDE | BTN_A, ..Default::default() }, &mut out);
        assert_eq!(out, [Press::QuickAccess]);
        out.clear();
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: BTN_GUIDE, ..Default::default() }, &mut out);
        assert!(out.is_empty());
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: 0, ..Default::default() }, &mut out);
        assert!(out.is_empty());

        // Guide + X emits ToggleOsk immediately and suppresses Guide on release.
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: BTN_GUIDE, ..Default::default() }, &mut out);
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: BTN_GUIDE | BTN_X, ..Default::default() }, &mut out);
        assert_eq!(out, [Press::ToggleOsk]);
        out.clear();
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: 0, ..Default::default() }, &mut out);
        assert!(out.is_empty());

        // Dedicated Quick Access button (`misc`) emits QuickAccess on rising edge only.
        held.update_sample(&client::LauncherPadSample { id: 1, misc: true, ..Default::default() }, &mut out);
        held.update_sample(&client::LauncherPadSample { id: 1, misc: true, ..Default::default() }, &mut out);
        assert_eq!(out, [Press::QuickAccess]);
        out.clear();

        // L3 / R3 emit cleanly.
        held.update_sample(&client::LauncherPadSample { id: 1, buttons: BTN_LSTICK | BTN_RSTICK, ..Default::default() }, &mut out);
        assert_eq!(out, [Press::L3, Press::R3]);
    }

    #[test]
    fn right_stick_scrolls_and_triggers_fire_once() {
        let mut held = HeldDirections::default();
        let mut out = Vec::new();
        let now = Instant::now();
        held.update_sample(&client::LauncherPadSample { id: 1, ry: -0.8, ..Default::default() }, &mut out);
        assert_eq!(held.due_scroll(now), Some(1));
        assert_eq!(held.due_scroll(now + Duration::from_millis(20)), None);
        assert_eq!(held.due_scroll(now + SCROLL_REPEAT), Some(1));
        held.update_sample(&client::LauncherPadSample { id: 1, ry: 0.0, ..Default::default() }, &mut out);
        assert_eq!(held.due_scroll(now + SCROLL_REPEAT * 2), None);

        out.clear();
        held.update_sample(&client::LauncherPadSample { id: 1, lt: 0.50, ..Default::default() }, &mut out);
        assert_eq!(out, [Press::Lt]);
        out.clear();
        held.update_sample(&client::LauncherPadSample { id: 1, lt: 0.9, ..Default::default() }, &mut out);
        assert!(out.is_empty());
        held.update_sample(&client::LauncherPadSample { id: 1, lt: 0.1, ..Default::default() }, &mut out);
        assert!(out.is_empty());
        held.update_sample(&client::LauncherPadSample { id: 1, lt: 0.50, ..Default::default() }, &mut out);
        assert_eq!(out, [Press::Lt]);
    }

    #[test]
    fn idle_poll_timeout_relaxes_when_nothing_held() {
        let held = HashMap::new();
        let now = Instant::now();
        assert_eq!(next_poll_timeout(&held, now), Duration::from_millis(250));
    }
}
