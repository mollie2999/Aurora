//! X11 / XWayland input capture for the Linux Vulkan overlay layer.
//!
//! Dynamically loads `libX11.so.6` and optionally `libXi.so.6` via `dlopen`
//! (zero link-time dependencies). When Aurora opens the overlay
//! (`ToGame::SetOpen(true)`), creates a child `InputOnly` shield window on the
//! game's X11 window, syncs with the X server, and attempts pointer/keyboard
//! grabs via a dedicated `XOpenDisplay(NULL)` connection.
//!
//! If Wine/Proton holds an active pointer or keyboard grab (`AlreadyGrabbed`),
//! negotiates XInput 2.1+ and captures `XI_RawMotion`, `XI_RawButtonPress`/
//! `Release`, and `XI_RawKeyPress`/`Release` (`XIAllMasterDevices`) so a
//! virtual overlay cursor and full keyboard input continue working even under
//! XWayland pointer lock. Native Wayland Vulkan surfaces without X11 cannot
//! safely verify game-surface focus from a Vulkan layer, so they wait cleanly
//! without reading global `/dev/input/event*` devices.

use crate::net::shared;
use crate::wire::{Input, MOD_ALT, MOD_CTRL, MOD_SHIFT};
use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

static ACTIVE_WINDOW: AtomicU64 = AtomicU64::new(0);
static SURFACE_IS_WAYLAND: AtomicBool = AtomicBool::new(false);
static OUR_DISPLAY: AtomicUsize = AtomicUsize::new(0);
static PREV_ERR_HANDLER: AtomicUsize = AtomicUsize::new(0);
static OUR_LAST_X11_ERR: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SurfaceKind {
    X11(u64),
    Wayland,
}

/// Record the game's active window/surface target.
pub fn set_surface(kind: SurfaceKind) {
    match kind {
        SurfaceKind::X11(win) => {
            SURFACE_IS_WAYLAND.store(false, Ordering::Relaxed);
            if win != 0 {
                ACTIVE_WINDOW.store(win, Ordering::Relaxed);
            }
        }
        SurfaceKind::Wayland => {
            SURFACE_IS_WAYLAND.store(true, Ordering::Relaxed);
            ACTIVE_WINDOW.store(0, Ordering::Relaxed);
        }
    }
}

/// Start the background input capture thread once.
pub fn ensure_started() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("aurora-vk-overlay-input".into()).spawn(input_loop);
    });
}

// ---- Minimal libdl + X11 + XInput2 ABI -------------------------------------

const RTLD_LAZY: c_int = 1;
const RTLD_LOCAL: c_int = 0;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type Display = c_void;
type Window = c_ulong;
type Cursor = c_ulong;
type Time = c_ulong;
type KeySym = c_ulong;
type Bool = c_int;

const GRAB_SUCCESS: c_int = 0;
const GRAB_MODE_ASYNC: c_int = 1;
const CURRENT_TIME: Time = 0;

const KEY_PRESS_MASK: c_uint = 1 << 0;
const KEY_RELEASE_MASK: c_uint = 1 << 1;
const BUTTON_PRESS_MASK: c_uint = 1 << 2;
const BUTTON_RELEASE_MASK: c_uint = 1 << 3;
const POINTER_MOTION_MASK: c_uint = 1 << 6;

const INPUT_ONLY: c_uint = 2;
const CW_EVENT_MASK: c_ulong = 1 << 11;
const CW_CURSOR: c_ulong = 1 << 14;
const XC_LEFT_PTR: c_uint = 68;

const KEY_PRESS: c_int = 2;
const KEY_RELEASE: c_int = 3;
const BUTTON_PRESS: c_int = 4;
const BUTTON_RELEASE: c_int = 5;
const MOTION_NOTIFY: c_int = 6;
const GENERIC_EVENT: c_int = 35;

const SHIFT_MASK: c_uint = 1 << 0;
const CONTROL_MASK: c_uint = 1 << 2;
const MOD1_MASK: c_uint = 1 << 3;
const BUTTON1_MASK: c_uint = 1 << 8;
const BUTTON2_MASK: c_uint = 1 << 9;
const BUTTON3_MASK: c_uint = 1 << 10;

// XInput2 constants (verified against <X11/extensions/XI2.h>).
// XI 2.1+ is required so Xorg/XWayland still deliver raw events while another
// client (e.g. Wine/Proton) holds an active device grab.
const XI_ALL_MASTER_DEVICES: c_int = 1;
const XI_RAW_KEY_PRESS: c_int = 13;
const XI_RAW_KEY_RELEASE: c_int = 14;
const XI_RAW_BUTTON_PRESS: c_int = 15;
const XI_RAW_BUTTON_RELEASE: c_int = 16;
const XI_RAW_MOTION: c_int = 17;

pub fn grab_status_name(code: c_int) -> &'static str {
    match code {
        0 => "Success",
        1 => "AlreadyGrabbed",
        2 => "InvalidTime",
        3 => "GrabNotViewable",
        4 => "GrabFrozen",
        _ => "Unknown",
    }
}

/// Xorg/XWayland only deliver `XI_Raw*` events to clients that negotiated XI 2.1+
/// while another client holds a pointer or keyboard grab.
pub fn xi2_version_supports_grabbed_raw(major: c_int, minor: c_int) -> bool {
    major > 2 || (major == 2 && minor >= 1)
}

#[repr(C)]
#[derive(Default)]
struct XSetWindowAttributes {
    background_pixmap: c_ulong,
    background_pixel: c_ulong,
    border_pixmap: c_ulong,
    border_pixel: c_ulong,
    bit_gravity: c_int,
    win_gravity: c_int,
    backing_store: c_int,
    backing_planes: c_ulong,
    backing_pixel: c_ulong,
    save_under: Bool,
    event_mask: c_long,
    do_not_propagate_mask: c_long,
    override_redirect: Bool,
    colormap: c_ulong,
    cursor: Cursor,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct XAnyEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: Window,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct XKeyEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: Time,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    keycode: c_uint,
    same_screen: Bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct XButtonEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: Time,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    button: c_uint,
    same_screen: Bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct XMotionEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: Time,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    is_hint: c_char,
    same_screen: Bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct XGenericEventCookie {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    extension: c_int,
    evtype: c_int,
    cookie: c_uint,
    data: *mut c_void,
}

#[repr(C)]
union XEvent {
    ty: c_int,
    any: XAnyEvent,
    key: XKeyEvent,
    button: XButtonEvent,
    motion: XMotionEvent,
    xcookie: XGenericEventCookie,
    pad: [c_long; 24],
}

#[repr(C)]
struct XIValuatorState {
    mask_len: c_int,
    mask: *mut u8,
    values: *mut f64,
}

#[repr(C)]
struct XIRawEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    extension: c_int,
    evtype: c_int,
    time: Time,
    deviceid: c_int,
    sourceid: c_int,
    detail: c_int,
    flags: c_int,
    valuators: XIValuatorState,
    raw_values: *mut f64,
}

#[repr(C)]
struct XIEventMask {
    deviceid: c_int,
    mask_len: c_int,
    mask: *mut u8,
}

#[repr(C)]
struct XErrorEvent {
    ty: c_int,
    display: *mut Display,
    resourceid: c_ulong,
    serial: c_ulong,
    error_code: u8,
    request_code: u8,
    minor_code: u8,
}

#[allow(non_snake_case, dead_code)]
struct X11 {
    XOpenDisplay: unsafe extern "C" fn(*const c_char) -> *mut Display,
    XCloseDisplay: unsafe extern "C" fn(*mut Display) -> c_int,
    XFlush: unsafe extern "C" fn(*mut Display) -> c_int,
    XSync: Option<unsafe extern "C" fn(*mut Display, Bool) -> c_int>,
    XPending: unsafe extern "C" fn(*mut Display) -> c_int,
    XNextEvent: unsafe extern "C" fn(*mut Display, *mut XEvent) -> c_int,
    XGrabPointer: unsafe extern "C" fn(*mut Display, Window, Bool, c_uint, c_int, c_int, Window, Cursor, Time) -> c_int,
    XUngrabPointer: unsafe extern "C" fn(*mut Display, Time) -> c_int,
    XGrabKeyboard: unsafe extern "C" fn(*mut Display, Window, Bool, c_int, c_int, Time) -> c_int,
    XUngrabKeyboard: unsafe extern "C" fn(*mut Display, Time) -> c_int,
    XQueryPointer: unsafe extern "C" fn(
        *mut Display,
        Window,
        *mut Window,
        *mut Window,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut c_uint,
    ) -> Bool,
    XGetGeometry: unsafe extern "C" fn(
        *mut Display,
        Window,
        *mut Window,
        *mut c_int,
        *mut c_int,
        *mut c_uint,
        *mut c_uint,
        *mut c_uint,
        *mut c_uint,
    ) -> c_int,
    XLookupString: unsafe extern "C" fn(*mut XKeyEvent, *mut c_char, c_int, *mut KeySym, *mut c_void) -> c_int,
    XLookupKeysym: unsafe extern "C" fn(*mut XKeyEvent, c_int) -> KeySym,
    XKeysymToKeycode: unsafe extern "C" fn(*mut Display, KeySym) -> u8,
    XQueryKeymap: unsafe extern "C" fn(*mut Display, *mut c_char) -> c_int,
    XQueryExtension: Option<unsafe extern "C" fn(*mut Display, *const c_char, *mut c_int, *mut c_int, *mut c_int) -> Bool>,
    XGetEventData: Option<unsafe extern "C" fn(*mut Display, *mut XGenericEventCookie) -> Bool>,
    XFreeEventData: Option<unsafe extern "C" fn(*mut Display, *mut XGenericEventCookie)>,
    XSetErrorHandler: Option<unsafe extern "C" fn(Option<unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int>) -> *mut c_void>,
    XDefaultRootWindow: Option<unsafe extern "C" fn(*mut Display) -> Window>,
    XCreateWindow: Option<
        unsafe extern "C" fn(
            *mut Display,
            Window,
            c_int,
            c_int,
            c_uint,
            c_uint,
            c_uint,
            c_int,
            c_uint,
            *mut c_void,
            c_ulong,
            *mut XSetWindowAttributes,
        ) -> Window,
    >,
    XMapRaised: Option<unsafe extern "C" fn(*mut Display, Window) -> c_int>,
    XDestroyWindow: Option<unsafe extern "C" fn(*mut Display, Window) -> c_int>,
    XResizeWindow: Option<unsafe extern "C" fn(*mut Display, Window, c_uint, c_uint) -> c_int>,
    XGetInputFocus: Option<unsafe extern "C" fn(*mut Display, *mut Window, *mut c_int) -> c_int>,
    XQueryTree: Option<unsafe extern "C" fn(*mut Display, Window, *mut Window, *mut Window, *mut *mut Window, *mut c_uint) -> c_int>,
    XFree: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    XTranslateCoordinates: Option<
        unsafe extern "C" fn(*mut Display, Window, Window, c_int, c_int, *mut c_int, *mut c_int, *mut Window) -> Bool,
    >,
    XCreateFontCursor: Option<unsafe extern "C" fn(*mut Display, c_uint) -> Cursor>,
    XFreeCursor: Option<unsafe extern "C" fn(*mut Display, Cursor) -> c_int>,
}

#[allow(non_snake_case)]
struct Xi2 {
    opcode: c_int,
    major: c_int,
    minor: c_int,
    XISelectEvents: unsafe extern "C" fn(*mut Display, Window, *mut XIEventMask, c_int) -> c_int,
}

unsafe extern "C" fn ignore_x11_error(dpy: *mut Display, err: *mut c_void) -> c_int {
    let ours = OUR_DISPLAY.load(Ordering::Acquire) as *mut Display;
    let (err_dpy, err_code) = if !err.is_null() {
        let ev = unsafe { &*(err as *const XErrorEvent) };
        (ev.display, ev.error_code as u32)
    } else {
        (dpy, 1)
    };
    if !ours.is_null() && (dpy == ours || err_dpy == ours) {
        OUR_LAST_X11_ERR.store(err_code.max(1), Ordering::Release);
        return 0;
    }
    let prev = PREV_ERR_HANDLER.load(Ordering::Acquire);
    if prev != 0 {
        let f: unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int = unsafe { std::mem::transmute(prev) };
        return unsafe { f(dpy, err) };
    }
    0
}

unsafe fn dl_sym<T>(lib: *mut c_void, name: &CStr) -> Option<T> {
    let p = unsafe { dlsym(lib, name.as_ptr()) };
    if p.is_null() { None } else { Some(unsafe { std::mem::transmute_copy(&p) }) }
}

impl X11 {
    unsafe fn load() -> Option<Self> {
        let lib = unsafe { dlopen(c"libX11.so.6".as_ptr(), RTLD_LAZY | RTLD_LOCAL) };
        if lib.is_null() {
            return None;
        }
        unsafe {
            Some(Self {
                XOpenDisplay: dl_sym(lib, c"XOpenDisplay")?,
                XCloseDisplay: dl_sym(lib, c"XCloseDisplay")?,
                XFlush: dl_sym(lib, c"XFlush")?,
                XSync: dl_sym(lib, c"XSync"),
                XPending: dl_sym(lib, c"XPending")?,
                XNextEvent: dl_sym(lib, c"XNextEvent")?,
                XGrabPointer: dl_sym(lib, c"XGrabPointer")?,
                XUngrabPointer: dl_sym(lib, c"XUngrabPointer")?,
                XGrabKeyboard: dl_sym(lib, c"XGrabKeyboard")?,
                XUngrabKeyboard: dl_sym(lib, c"XUngrabKeyboard")?,
                XQueryPointer: dl_sym(lib, c"XQueryPointer")?,
                XGetGeometry: dl_sym(lib, c"XGetGeometry")?,
                XLookupString: dl_sym(lib, c"XLookupString")?,
                XLookupKeysym: dl_sym(lib, c"XLookupKeysym")?,
                XKeysymToKeycode: dl_sym(lib, c"XKeysymToKeycode")?,
                XQueryKeymap: dl_sym(lib, c"XQueryKeymap")?,
                XQueryExtension: dl_sym(lib, c"XQueryExtension"),
                XGetEventData: dl_sym(lib, c"XGetEventData"),
                XFreeEventData: dl_sym(lib, c"XFreeEventData"),
                XSetErrorHandler: dl_sym(lib, c"XSetErrorHandler"),
                XDefaultRootWindow: dl_sym(lib, c"XDefaultRootWindow"),
                XCreateWindow: dl_sym(lib, c"XCreateWindow"),
                XMapRaised: dl_sym(lib, c"XMapRaised"),
                XDestroyWindow: dl_sym(lib, c"XDestroyWindow"),
                XResizeWindow: dl_sym(lib, c"XResizeWindow"),
                XGetInputFocus: dl_sym(lib, c"XGetInputFocus"),
                XQueryTree: dl_sym(lib, c"XQueryTree"),
                XFree: dl_sym(lib, c"XFree"),
                XTranslateCoordinates: dl_sym(lib, c"XTranslateCoordinates"),
                XCreateFontCursor: dl_sym(lib, c"XCreateFontCursor"),
                XFreeCursor: dl_sym(lib, c"XFreeCursor"),
            })
        }
    }

    unsafe fn sync_or_flush(&self, dpy: *mut Display) {
        if let Some(sync_fn) = self.XSync {
            unsafe { sync_fn(dpy, 0) };
        } else {
            unsafe { (self.XFlush)(dpy) };
        }
    }

    unsafe fn drain_pending(&self, dpy: *mut Display) {
        while unsafe { (self.XPending)(dpy) } > 0 {
            let mut ev: XEvent = unsafe { std::mem::zeroed() };
            unsafe { (self.XNextEvent)(dpy, &mut ev) };
        }
    }
}

impl Xi2 {
    unsafe fn load(x11: &X11, dpy: *mut Display) -> Option<Self> {
        let query_ext = x11.XQueryExtension?;
        x11.XGetEventData?;
        x11.XFreeEventData?;
        x11.XDefaultRootWindow?;

        let mut opcode: c_int = 0;
        let mut first_ev: c_int = 0;
        let mut first_err: c_int = 0;
        if unsafe { query_ext(dpy, c"XInputExtension".as_ptr(), &mut opcode, &mut first_ev, &mut first_err) } == 0 {
            return None;
        }
        let lib = unsafe { dlopen(c"libXi.so.6".as_ptr(), RTLD_LAZY | RTLD_LOCAL) };
        if lib.is_null() {
            return None;
        }
        let query_ver: unsafe extern "C" fn(*mut Display, *mut c_int, *mut c_int) -> c_int =
            unsafe { dl_sym(lib, c"XIQueryVersion")? };
        let select_events: unsafe extern "C" fn(*mut Display, Window, *mut XIEventMask, c_int) -> c_int =
            unsafe { dl_sym(lib, c"XISelectEvents")? };

        // Request XI 2.2 (or >= 2.1): Xorg/XWayland suppress XI_Raw* events
        // during another client's grab for XI 2.0 clients.
        let mut major = 2;
        let mut minor = 2;
        if unsafe { query_ver(dpy, &mut major, &mut minor) } != 0 || !xi2_version_supports_grabbed_raw(major, minor) {
            return None;
        }
        Some(Self { opcode, major, minor, XISelectEvents: select_events })
    }

    unsafe fn select_raw(&self, x11: &X11, dpy: *mut Display, enabled: bool) -> bool {
        let Some(root_fn) = x11.XDefaultRootWindow else { return false };
        let root = unsafe { root_fn(dpy) };
        if root == 0 {
            return false;
        }
        let mut mask_bits = [0u8; 4];
        if enabled {
            for ev in [XI_RAW_KEY_PRESS, XI_RAW_KEY_RELEASE, XI_RAW_BUTTON_PRESS, XI_RAW_BUTTON_RELEASE, XI_RAW_MOTION] {
                mask_bits[(ev >> 3) as usize] |= 1u8 << (ev & 7);
            }
        }
        let mut em = XIEventMask {
            deviceid: XI_ALL_MASTER_DEVICES,
            mask_len: mask_bits.len() as c_int,
            mask: mask_bits.as_mut_ptr(),
        };
        // Flush/sync prior requests first so only protocol errors caused by
        // XISelectEvents itself can trip OUR_LAST_X11_ERR.
        unsafe { x11.sync_or_flush(dpy) };
        OUR_LAST_X11_ERR.store(0, Ordering::Release);
        let rc = unsafe { (self.XISelectEvents)(dpy, root, &mut em, 1) };
        unsafe { x11.sync_or_flush(dpy) };
        rc == 0 && OUR_LAST_X11_ERR.load(Ordering::Acquire) == 0
    }
}

struct CookieGuard<'a> {
    free_fn: unsafe extern "C" fn(*mut Display, *mut XGenericEventCookie),
    dpy: *mut Display,
    cookie: &'a mut XGenericEventCookie,
}

impl Drop for CookieGuard<'_> {
    fn drop(&mut self) {
        unsafe { (self.free_fn)(self.dpy, self.cookie) };
    }
}

// ---- Pure helpers (unit-tested) --------------------------------------------

#[inline]
fn key_down(keymap: &[u8; 32], kc: u8) -> bool {
    kc != 0 && (keymap[(kc / 8) as usize] & (1u8 << (kc % 8))) != 0
}

fn x11_mods(state: c_uint) -> u8 {
    let mut m = 0u8;
    if state & SHIFT_MASK != 0 {
        m |= MOD_SHIFT;
    }
    if state & CONTROL_MASK != 0 {
        m |= MOD_CTRL;
    }
    if state & MOD1_MASK != 0 {
        m |= MOD_ALT;
    }
    m
}

/// Extract relative X (valuator 0) and Y (valuator 1) deltas from an `XIRawEvent`
/// valuator bitmask using a caller-supplied value reader (which reads via
/// `ptr::read_unaligned` for libXi's potentially 4-byte-aligned buffers).
pub fn extract_raw_motion_deltas_with(mask: &[u8], read_val: impl Fn(usize) -> Option<f64>) -> (f64, f64) {
    let mut dx = 0.0f64;
    let mut dy = 0.0f64;
    let mut idx = 0usize;
    let max_bits = (mask.len().saturating_mul(8)).min(2);
    for bit in 0..max_bits {
        let is_set = (mask[bit >> 3] & (1u8 << (bit & 7))) != 0;
        if is_set {
            if let Some(v) = read_val(idx) {
                if v.is_finite() {
                    let clamped = v.clamp(-4096.0, 4096.0);
                    if bit == 0 {
                        dx = clamped;
                    } else if bit == 1 {
                        dy = clamped;
                    }
                }
            }
            idx += 1;
        }
    }
    (dx, dy)
}

#[derive(Clone, Copy, Debug)]
struct VirtualCursor {
    x: f32,
    y: f32,
}

impl VirtualCursor {
    fn centered(w: u32, h: u32) -> Self {
        Self { x: (w.max(1) as f32) * 0.5, y: (h.max(1) as f32) * 0.5 }
    }

    fn set_clamped(&mut self, x: f32, y: f32, w: u32, h: u32) {
        let max_x = (w.max(1).saturating_sub(1)) as f32;
        let max_y = (h.max(1).saturating_sub(1)) as f32;
        self.x = if x.is_finite() { x.clamp(0.0, max_x) } else { self.x.clamp(0.0, max_x) };
        self.y = if y.is_finite() { y.clamp(0.0, max_y) } else { self.y.clamp(0.0, max_y) };
    }

    fn apply_delta(&mut self, dx: f64, dy: f64, w: u32, h: u32) -> bool {
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        let prev = (self.x, self.y);
        self.set_clamped(self.x + dx as f32, self.y + dy as f32, w, h);
        (self.x - prev.0).abs() > f32::EPSILON || (self.y - prev.1).abs() > f32::EPSILON
    }
}

/// Shared press/release latch across event and polling paths so a held
/// `Shift+Tab` that opens the overlay never immediately closes it (and `Esc`
/// is tracked independently from the overlay hotkey).
#[derive(Default, Debug)]
struct HotkeyLatch {
    ov_held: bool,
    esc_held: bool,
    shot_held: bool,
}

impl HotkeyLatch {
    /// Update from a full polled state (`ov_down`, `esc_down`, `shot_down`).
    /// Returns `(ov_rising, esc_rising, shot_rising)`.
    fn poll(&mut self, ov_down: bool, esc_down: bool, shot_down: bool) -> (bool, bool, bool) {
        let ov_rise = ov_down && !self.ov_held;
        let esc_rise = esc_down && !self.esc_held;
        let shot_rise = shot_down && !self.shot_held;
        self.ov_held = ov_down;
        self.esc_held = esc_down;
        self.shot_held = shot_down;
        (ov_rise, esc_rise, shot_rise)
    }

    fn trigger_ov_event(&mut self) -> bool {
        if self.ov_held {
            return false;
        }
        self.ov_held = true;
        true
    }

    fn trigger_esc_event(&mut self) -> bool {
        if self.esc_held {
            return false;
        }
        self.esc_held = true;
        true
    }

    fn trigger_shot_event(&mut self) -> bool {
        if self.shot_held {
            return false;
        }
        self.shot_held = true;
        true
    }
}

/// Tracks mouse buttons and virtual keys currently held in Aurora's overlay UI
/// so grab retries defer while input is held and close/window-change always
/// releases every held button/key cleanly.
#[derive(Default, Debug)]
struct HeldInputs {
    buttons: u8,
    keys: Vec<u32>,
}

impl HeldInputs {
    fn record(&mut self, inp: &Input) {
        match *inp {
            Input::Button { button, down, .. } if button < 8 => {
                if down {
                    self.buttons |= 1u8 << button;
                } else {
                    self.buttons &= !(1u8 << button);
                }
            }
            Input::Key { vk, down, .. } => {
                if down {
                    if !self.keys.contains(&vk) {
                        self.keys.push(vk);
                    }
                } else {
                    self.keys.retain(|&k| k != vk);
                }
            }
            _ => {}
        }
    }

    fn any_buttons_held(&self) -> bool {
        self.buttons != 0
    }

    fn any_keys_held(&self) -> bool {
        !self.keys.is_empty()
    }

    fn release_all(&mut self, x: f32, y: f32) -> Vec<Input> {
        let mut out = Vec::new();
        for btn in 0u8..8 {
            if self.buttons & (1u8 << btn) != 0 {
                out.push(Input::Button { button: btn, down: false, x, y });
            }
        }
        self.buttons = 0;
        for vk in self.keys.drain(..) {
            out.push(Input::Key { vk, down: false, repeat: false });
        }
        out.push(Input::Leave);
        out
    }
}

fn map_x11_button(button: c_uint, down: bool, x: f32, y: f32) -> Option<Input> {
    match button {
        1 => Some(Input::Button { button: 0, down, x, y }),
        2 => Some(Input::Button { button: 2, down, x, y }),
        3 => Some(Input::Button { button: 1, down, x, y }),
        4 if down => Some(Input::Wheel { x: 0.0, y: 1.0 }),
        5 if down => Some(Input::Wheel { x: 0.0, y: -1.0 }),
        6 if down => Some(Input::Wheel { x: -1.0, y: 0.0 }),
        7 if down => Some(Input::Wheel { x: 1.0, y: 0.0 }),
        8 => Some(Input::Button { button: 3, down, x, y }),
        9 => Some(Input::Button { button: 4, down, x, y }),
        _ => None,
    }
}

/// Map X11 KeySym to Windows Virtual-Key code (what `apps/desktop/src/ingame.rs` expects).
pub fn keysym_to_vk(sym: u64) -> Option<u32> {
    match sym {
        0xff08 => Some(0x08), // BackSpace -> VK_BACK
        0xff09 | 0xfe20 => Some(0x09), // Tab / ISO_Left_Tab -> VK_TAB
        0xff0d | 0xff8d => Some(0x0D), // Return / KP_Enter -> VK_RETURN
        0xff1b => Some(0x1B), // Escape -> VK_ESCAPE
        0x0020 => Some(0x20), // space -> VK_SPACE
        0xff55 => Some(0x21), // Prior / Page_Up -> VK_PRIOR
        0xff56 => Some(0x22), // Next / Page_Down -> VK_NEXT
        0xff57 => Some(0x23), // End -> VK_END
        0xff50 => Some(0x24), // Home -> VK_HOME
        0xff51 => Some(0x25), // Left -> VK_LEFT
        0xff52 => Some(0x26), // Up -> VK_UP
        0xff53 => Some(0x27), // Right -> VK_RIGHT
        0xff54 => Some(0x28), // Down -> VK_DOWN
        0xff63 => Some(0x2D), // Insert -> VK_INSERT
        0xffff => Some(0x2E), // Delete -> VK_DELETE
        0x0030..=0x0039 => Some(sym as u32), // '0'..'9'
        0x0041..=0x005a => Some(sym as u32), // 'A'..'Z'
        0x0061..=0x007a => Some((sym - 0x20) as u32), // 'a'..'z' -> 'A'..'Z'
        0x0060 => Some(0xC0), // grave -> VK_OEM_3
        0xffbe..=0xffc9 => Some(0x70 + (sym - 0xffbe) as u32), // F1..F12
        0xffe1 | 0xffe2 => Some(0x10), // Shift
        0xffe3 | 0xffe4 => Some(0x11), // Control
        0xffe9 | 0xffea => Some(0x12), // Alt
        _ => None,
    }
}

/// Map a Windows Virtual-Key code back to an X11 KeySym for hotkey matching.
pub fn vk_to_keysym(vk: u32) -> Option<u64> {
    match vk {
        0x08 => Some(0xff08),
        0x09 => Some(0xff09),
        0x0D => Some(0xff0d),
        0x1B => Some(0xff1b),
        0x20 => Some(0x0020),
        0x21 => Some(0xff55),
        0x22 => Some(0xff56),
        0x23 => Some(0xff57),
        0x24 => Some(0xff50),
        0x25 => Some(0xff51),
        0x26 => Some(0xff52),
        0x27 => Some(0xff53),
        0x28 => Some(0xff54),
        0x2D => Some(0xff63),
        0x2E => Some(0xffff),
        0x30..=0x39 => Some(vk as u64),
        0x41..=0x5A => Some(vk as u64),
        0xC0 => Some(0x0060),
        0x70..=0x7B => Some(0xffbe + (vk - 0x70) as u64),
        _ => None,
    }
}

fn push_event(events: &mut Vec<Input>, held: &mut HeldInputs, e: Input) {
    held.record(&e);
    if let (Input::Move { .. }, Some(Input::Move { .. })) = (&e, events.last()) {
        events.pop();
    }
    events.push(e);
}

fn scale_point_f32(x: f32, y: f32, win_w: u32, win_h: u32) -> (f32, f32) {
    let (fb_w, fb_h) = *shared().size.lock().unwrap_or_else(|e| e.into_inner());
    if win_w > 0 && win_h > 0 && fb_w > 0 && fb_h > 0 {
        (x * (fb_w as f32 / win_w as f32), y * (fb_h as f32 / win_h as f32))
    } else {
        (x, y)
    }
}

fn scale_point(x: i32, y: i32, win_w: u32, win_h: u32) -> (f32, f32) {
    scale_point_f32(x as f32, y as f32, win_w, win_h)
}

fn combo_matches(keymap: &[u8; 32], req_mods: u8, kc_key: u8, mods_kc: (u8, u8, u8, u8, u8, u8)) -> bool {
    if kc_key == 0 || !key_down(keymap, kc_key) {
        return false;
    }
    let shift = key_down(keymap, mods_kc.0) || key_down(keymap, mods_kc.1);
    let ctrl = key_down(keymap, mods_kc.2) || key_down(keymap, mods_kc.3);
    let alt = key_down(keymap, mods_kc.4) || key_down(keymap, mods_kc.5);
    let held = (if shift { MOD_SHIFT } else { 0 })
        | (if ctrl { MOD_CTRL } else { 0 })
        | (if alt { MOD_ALT } else { 0 });
    held == (req_mods & 0x07)
}

fn is_ancestor_of(x11: &X11, dpy: *mut Display, maybe_anc: Window, mut child: Window) -> bool {
    let Some(query_tree) = x11.XQueryTree else { return false };
    for _ in 0..8 {
        let mut root: Window = 0;
        let mut parent: Window = 0;
        let mut children: *mut Window = std::ptr::null_mut();
        let mut nchildren: c_uint = 0;
        let ok = unsafe { query_tree(dpy, child, &mut root, &mut parent, &mut children, &mut nchildren) };
        if !children.is_null() {
            if let Some(free_fn) = x11.XFree {
                unsafe { free_fn(children.cast()) };
            }
        }
        if ok == 0 || parent == 0 || parent == root {
            return false;
        }
        if parent == maybe_anc {
            return true;
        }
        child = parent;
    }
    false
}

fn window_has_focus(x11: &X11, dpy: *mut Display, win: Window, shield: Window) -> bool {
    let Some(get_focus) = x11.XGetInputFocus else { return false };
    let mut focus: Window = 0;
    let mut revert: c_int = 0;
    unsafe { get_focus(dpy, &mut focus, &mut revert) };
    // 0 = None, 1 = PointerRoot
    if focus <= 1 {
        return false;
    }
    if focus == win || (shield != 0 && focus == shield) {
        return true;
    }
    is_ancestor_of(x11, dpy, win, focus)
}

unsafe fn create_shield(x11: &X11, dpy: *mut Display, win: Window, win_w: u32, win_h: u32, arrow_cursor: Cursor) -> Window {
    let (Some(create_fn), Some(map_fn)) = (x11.XCreateWindow, x11.XMapRaised) else {
        return 0;
    };
    let sw = win_w.max(128);
    let sh = win_h.max(128);
    let mut attrs = XSetWindowAttributes {
        event_mask: (POINTER_MOTION_MASK | BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK | KEY_PRESS_MASK | KEY_RELEASE_MASK) as c_long,
        cursor: arrow_cursor,
        ..Default::default()
    };
    let mask = CW_EVENT_MASK | if arrow_cursor != 0 { CW_CURSOR } else { 0 };
    let w = unsafe { create_fn(dpy, win, 0, 0, sw, sh, 0, 0, INPUT_ONLY, std::ptr::null_mut(), mask, &mut attrs) };
    if w != 0 {
        unsafe {
            map_fn(dpy, w);
            // Sync with the X server so the shield is viewable before XGrab*
            // (prevents spurious GrabNotViewable = 3 failures).
            x11.sync_or_flush(dpy);
        }
    }
    w
}

unsafe fn try_grab_ptr(x11: &X11, dpy: *mut Display, shield: Window, win: Window, ptr_mask: c_uint, arrow_cursor: Cursor) -> (bool, c_int) {
    if !window_has_focus(x11, dpy, win, shield) { return (false, 3); }
    let primary = if shield != 0 { shield } else { win };
    let mut code = unsafe {
        (x11.XGrabPointer)(dpy, primary, 0, ptr_mask, GRAB_MODE_ASYNC, GRAB_MODE_ASYNC, 0, arrow_cursor, CURRENT_TIME)
    };
    if code != GRAB_SUCCESS && shield != 0 && shield != win && window_has_focus(x11, dpy, win, shield) {
        let fb = unsafe {
            (x11.XGrabPointer)(dpy, win, 0, ptr_mask, GRAB_MODE_ASYNC, GRAB_MODE_ASYNC, 0, arrow_cursor, CURRENT_TIME)
        };
        if fb == GRAB_SUCCESS {
            code = fb;
        }
    }
    (code == GRAB_SUCCESS, code)
}

unsafe fn try_grab_kbd(x11: &X11, dpy: *mut Display, shield: Window, win: Window) -> (bool, c_int) {
    if !window_has_focus(x11, dpy, win, shield) { return (false, 3); }
    let primary = if shield != 0 { shield } else { win };
    let mut code = unsafe {
        (x11.XGrabKeyboard)(dpy, primary, 0, GRAB_MODE_ASYNC, GRAB_MODE_ASYNC, CURRENT_TIME)
    };
    if code != GRAB_SUCCESS && shield != 0 && shield != win && window_has_focus(x11, dpy, win, shield) {
        let fb = unsafe {
            (x11.XGrabKeyboard)(dpy, win, 0, GRAB_MODE_ASYNC, GRAB_MODE_ASYNC, CURRENT_TIME)
        };
        if fb == GRAB_SUCCESS {
            code = fb;
        }
    }
    (code == GRAB_SUCCESS, code)
}

#[allow(clippy::too_many_arguments)]
unsafe fn handle_xkey_event(
    x11: &X11,
    mut k: XKeyEvent,
    down: bool,
    cur_ov: (u8, u32, u8),
    cur_sh: (u8, u32, u8),
    mods: &mut u8,
    latch: &mut HotkeyLatch,
    held: &mut HeldInputs,
    close_requested: &mut bool,
    events: &mut Vec<Input>,
) {
    *mods = x11_mods(k.state);
    let mut buf = [0u8; 32];
    let mut sym: KeySym = 0;
    let n = unsafe {
        (x11.XLookupString)(
            &mut k,
            buf.as_mut_ptr().cast(),
            buf.len() as c_int,
            &mut sym,
            std::ptr::null_mut(),
        )
    };
    if sym == 0 {
        sym = unsafe { (x11.XLookupKeysym)(&mut k, 0) };
    }
    let vk_opt = keysym_to_vk(sym as u64);
    // Note: Do NOT clear `latch` on KEY_RELEASE events here because X11
    // autorepeat emits synthetic Release+Press pairs while a hotkey is held.
    // The hardware keymap poll (`latch.poll`) clears each latch once the key
    // is actually released.
    if down && vk_opt == Some(cur_sh.1) && (*mods & 0x07) == cur_sh.0 {
        if latch.trigger_shot_event() {
            shared().request_screenshot();
        }
        return;
    }
    if down && sym == 0xff1b {
        if latch.trigger_esc_event() {
            *close_requested = true;
        }
        return;
    }
    if down && vk_opt == Some(cur_ov.1) && (*mods & 0x07) == cur_ov.0 {
        if latch.trigger_ov_event() {
            *close_requested = true;
        }
        return;
    }
    if let Some(vk) = vk_opt {
        push_event(events, held, Input::Key { vk, down, repeat: false });
    }
    if down && n > 0 && (*mods & (MOD_CTRL | MOD_ALT) == 0) {
        if let Ok(s) = std::str::from_utf8(&buf[..n as usize]) {
            let printable: String = s.chars().filter(|c| !c.is_control()).collect();
            if !printable.is_empty() {
                push_event(events, held, Input::Text(printable));
            }
        }
    }
}

/// Wait cleanly when the game presents to a native Wayland Vulkan surface (or
/// when no X11/XWayland display exists). A Vulkan layer cannot verify Wayland
/// surface focus without owning the compositor seat, so we intentionally avoid
/// reading global `/dev/input/event*` devices (which would leak input from
/// other apps or lock screens) and wait until Aurora closes the overlay or an
/// X11 surface becomes active.
fn wait_native_wayland(logged_once: &mut bool) {
    if !*logged_once {
        eprintln!(
            "[aurora-vk-overlay] surface=wayland (X11 input capture unavailable; native Wayland Vulkan surfaces skip global evdev capture for focus safety — unset PROTON_ENABLE_WAYLAND for full overlay input)"
        );
        *logged_once = true;
    }
    while !shared().is_shutdown() {
        if !SURFACE_IS_WAYLAND.load(Ordering::Relaxed) && ACTIVE_WINDOW.load(Ordering::Relaxed) != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn input_loop() {
    let mut wayland_logged = false;
    let x11_opt = unsafe { X11::load() };
    let dpy = x11_opt
        .as_ref()
        .map(|x| unsafe { (x.XOpenDisplay)(std::ptr::null()) })
        .unwrap_or(std::ptr::null_mut());
    if dpy.is_null() {
        wait_native_wayland(&mut wayland_logged);
        return;
    }
    let x11 = x11_opt.unwrap();
    OUR_DISPLAY.store(dpy as usize, Ordering::Release);
    if let Some(set_err) = x11.XSetErrorHandler {
        let prev = unsafe { set_err(Some(ignore_x11_error)) } as usize;
        if prev != ignore_x11_error as *const () as usize {
            PREV_ERR_HANDLER.store(prev, Ordering::Release);
        }
    }

    let xi2 = unsafe { Xi2::load(&x11, dpy) };

    let kc_shift_l = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe1) };
    let kc_shift_r = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe2) };
    let kc_ctrl_l = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe3) };
    let kc_ctrl_r = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe4) };
    let kc_alt_l = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe9) };
    let kc_alt_r = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffea) };
    let mods_kc = (kc_shift_l, kc_shift_r, kc_ctrl_l, kc_ctrl_r, kc_alt_l, kc_alt_r);
    let kc_esc = unsafe { (x11.XKeysymToKeycode)(dpy, 0xff1b) };

    let resolve_kc = |vk: u32| -> u8 {
        vk_to_keysym(vk).map(|s| unsafe { (x11.XKeysymToKeycode)(dpy, s) }).unwrap_or(0)
    };
    let mut cur_ov = (0u8, 0u32, 0u8);
    let mut cur_sh = (0u8, 0u32, 0u8);
    let refresh_keys = |ov: &mut (u8, u32, u8), sh: &mut (u8, u32, u8)| {
        let (om, ovk) = shared().overlay_hotkey();
        if (om, ovk) != (ov.0, ov.1) {
            *ov = (om, ovk, resolve_kc(ovk));
        }
        let (sm, svk) = shared().screenshot_hotkey();
        if (sm, svk) != (sh.0, sh.1) {
            *sh = (sm, svk, resolve_kc(svk));
        }
    };

    let mut latch = HotkeyLatch::default();

    while !shared().is_shutdown() {
        // Select backend from the live surface even while closed so native
        // Wayland games never stay stuck in X11 hotkey polling.
        if SURFACE_IS_WAYLAND.load(Ordering::Relaxed) && ACTIVE_WINDOW.load(Ordering::Relaxed) == 0 {
            wait_native_wayland(&mut wayland_logged);
            continue;
        }
        wayland_logged = false;

        // While closed, poll XQueryKeymap at ~60Hz for the overlay & screenshot hotkeys.
        while !shared().is_open() && !shared().is_shutdown() {
            if SURFACE_IS_WAYLAND.load(Ordering::Relaxed) && ACTIVE_WINDOW.load(Ordering::Relaxed) == 0 {
                break;
            }
            refresh_keys(&mut cur_ov, &mut cur_sh);
            let mut keymap = [0u8; 32];
            unsafe { (x11.XQueryKeymap)(dpy, keymap.as_mut_ptr().cast()) };

            let ov_down = combo_matches(&keymap, cur_ov.0, cur_ov.2, mods_kc);
            let esc_down = key_down(&keymap, kc_esc);
            let shot_down = combo_matches(&keymap, cur_sh.0, cur_sh.2, mods_kc);
            let (ov_rise, _, shot_rise) = latch.poll(ov_down, esc_down, shot_down);
            let focused = (ov_rise || shot_rise) && window_has_focus(&x11, dpy, ACTIVE_WINDOW.load(Ordering::Relaxed) as Window, 0);
            if shot_rise && focused {
                shared().request_screenshot();
            }
            if ov_rise && focused {
                shared().notify_opened(true);
                break;
            }
            std::thread::sleep(Duration::from_millis(16));
        }

        if shared().is_shutdown() {
            break;
        }

        let is_wayland = SURFACE_IS_WAYLAND.load(Ordering::Relaxed);
        let mut win = ACTIVE_WINDOW.load(Ordering::Relaxed) as Window;
        if is_wayland && win == 0 {
            wait_native_wayland(&mut wayland_logged);
            continue;
        }

        if win == 0 {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }

        let query_geom = |w: Window| -> (u32, u32) {
            let (mut ww, mut wh) = (0u32, 0u32);
            let mut root = 0;
            let (mut gx, mut gy) = (0, 0);
            let (mut bw, mut depth) = (0, 0);
            if unsafe { (x11.XGetGeometry)(dpy, w, &mut root, &mut gx, &mut gy, &mut ww, &mut wh, &mut bw, &mut depth) } != 0 {
                (ww, wh)
            } else {
                (0, 0)
            }
        };

        if !window_has_focus(&x11, dpy, win, 0) {
            shared().notify_opened(false);
            continue;
        }
        let (mut win_w, mut win_h) = query_geom(win);
        let arrow_cursor = x11.XCreateFontCursor.map(|f| unsafe { f(dpy, XC_LEFT_PTR) }).unwrap_or(0);
        // Create a child InputOnly shield window inside `win` (NOT a top-level
        // override-redirect window on root) so we intercept pointer events and
        // show a visible cursor without stealing top-level WM focus or blocking
        // GNOME's `_NET_WM_PING` responsiveness check.
        let mut shield = unsafe { create_shield(&x11, dpy, win, win_w, win_h, arrow_cursor) };
        let ptr_mask = POINTER_MOTION_MASK | BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK;
        let (mut ptr_grabbed, mut ptr_code) = unsafe { try_grab_ptr(&x11, dpy, shield, win, ptr_mask, arrow_cursor) };
        let (mut kbd_grabbed, mut kbd_code) = unsafe { try_grab_kbd(&x11, dpy, shield, win) };
        let xi2_active = xi2.as_ref().is_some_and(|xi| unsafe { xi.select_raw(&x11, dpy, true) });
        let xi2_ver = xi2.as_ref().map(|x| (x.major, x.minor)).unwrap_or((0, 0));
        unsafe { x11.sync_or_flush(dpy) };
        eprintln!(
            "[aurora-vk-overlay] open: surface=x11 win=0x{win:x} shield=0x{shield:x} geom={win_w}x{win_h} ptr_grab={ptr_grabbed}({ptr_code}:{}) kbd_grab={kbd_grabbed}({kbd_code}:{}) xi2={xi2_active}({}.{})",
            grab_status_name(ptr_code),
            grab_status_name(kbd_code),
            xi2_ver.0,
            xi2_ver.1,
        );

        // Seed virtual cursor from current pointer location inside `win` (or center).
        let mut virt_cursor = VirtualCursor::centered(win_w, win_h);
        {
            let (mut root, mut child) = (0, 0);
            let (mut rx, mut ry, mut wx, mut wy) = (0, 0, 0, 0);
            let mut mask = 0u32;
            if unsafe { (x11.XQueryPointer)(dpy, win, &mut root, &mut child, &mut rx, &mut ry, &mut wx, &mut wy, &mut mask) } != 0
                && wx >= 0
                && wy >= 0
                && (wx as u32) < win_w.max(1)
                && (wy as u32) < win_h.max(1)
            {
                virt_cursor.set_clamped(wx as f32, wy as f32, win_w, win_h);
            }
            let (sx, sy) = scale_point_f32(virt_cursor.x, virt_cursor.y, win_w, win_h);
            shared().send_input(0, vec![Input::Move { x: sx, y: sy }]);
        }

        let mut held = HeldInputs::default();
        let mut last_poll_pos: Option<(i32, i32)> = None;
        let mut last_poll_mask: c_uint = 0;
        let mut xi2_saw_raw_ptr = false;
        let mut ticks = 0u32;
        let mut unfocused_checks = 0u32;

        while shared().is_open() && !shared().is_shutdown() {
            refresh_keys(&mut cur_ov, &mut cur_sh);
            ticks = ticks.wrapping_add(1);

            // Transition cleanly if the game switched to a native Wayland surface.
            if SURFACE_IS_WAYLAND.load(Ordering::Relaxed) && ACTIVE_WINDOW.load(Ordering::Relaxed) == 0 {
                break;
            }

            let cur_win = ACTIVE_WINDOW.load(Ordering::Relaxed) as Window;
            if cur_win != 0 && cur_win != win {
                // Flush held UI inputs before switching target windows.
                let (sx, sy) = scale_point_f32(virt_cursor.x, virt_cursor.y, win_w, win_h);
                shared().send_input(0, held.release_all(sx, sy));
                if ptr_grabbed {
                    unsafe { (x11.XUngrabPointer)(dpy, CURRENT_TIME) };
                }
                if kbd_grabbed {
                    unsafe { (x11.XUngrabKeyboard)(dpy, CURRENT_TIME) };
                }
                if shield != 0 {
                    if let Some(destroy_fn) = x11.XDestroyWindow {
                        unsafe { destroy_fn(dpy, shield) };
                    }
                }
                unsafe {
                    x11.sync_or_flush(dpy);
                    x11.drain_pending(dpy);
                }
                win = cur_win;
                (win_w, win_h) = query_geom(win);
                shield = unsafe { create_shield(&x11, dpy, win, win_w, win_h, arrow_cursor) };
                (ptr_grabbed, ptr_code) = unsafe { try_grab_ptr(&x11, dpy, shield, win, ptr_mask, arrow_cursor) };
                (kbd_grabbed, kbd_code) = unsafe { try_grab_kbd(&x11, dpy, shield, win) };
                virt_cursor.set_clamped(virt_cursor.x, virt_cursor.y, win_w, win_h);
                eprintln!(
                    "[aurora-vk-overlay] window changed: win=0x{win:x} shield=0x{shield:x} geom={win_w}x{win_h} ptr_grab={ptr_grabbed}({ptr_code}:{}) kbd_grab={kbd_grabbed}({kbd_code}:{})",
                    grab_status_name(ptr_code),
                    grab_status_name(kbd_code),
                );
            } else if ticks % 30 == 0 {
                let geom = query_geom(win);
                if geom.0 > 0 && geom.1 > 0 && geom != (win_w, win_h) {
                    (win_w, win_h) = geom;
                    virt_cursor.set_clamped(virt_cursor.x, virt_cursor.y, win_w, win_h);
                    if shield != 0 {
                        if let Some(resize_fn) = x11.XResizeWindow {
                            unsafe { resize_fn(dpy, shield, win_w.max(128), win_h.max(128)) };
                        }
                    }
                }
            }
            // If focus leaves the game (Alt+Tab, Super, compositor dialog),
            // release grabs and close the overlay so we never freeze or trigger
            // Mutter's "not responding" dialog.
            if ticks > 25 && ticks % 8 == 0 {
                if window_has_focus(&x11, dpy, win, shield) {
                    unfocused_checks = 0;
                } else {
                    unfocused_checks += 1;
                    if unfocused_checks >= 2 {
                        shared().notify_opened(false);
                        break;
                    }
                }
            }

            let mut keymap = [0u8; 32];
            unsafe { (x11.XQueryKeymap)(dpy, keymap.as_mut_ptr().cast()) };
            let mut mod_sides = [
                key_down(&keymap, mods_kc.0),
                key_down(&keymap, mods_kc.1),
                key_down(&keymap, mods_kc.2),
                key_down(&keymap, mods_kc.3),
                key_down(&keymap, mods_kc.4),
                key_down(&keymap, mods_kc.5),
            ];
            let derive_raw_state = |ms: &[bool; 6]| -> c_uint {
                (if ms[0] || ms[1] { SHIFT_MASK } else { 0 })
                    | (if ms[2] || ms[3] { CONTROL_MASK } else { 0 })
                    | (if ms[4] || ms[5] { MOD1_MASK } else { 0 })
            };
            let mut raw_x11_state = derive_raw_state(&mod_sides);

            let mut events = Vec::new();
            let mut mods = x11_mods(raw_x11_state);
            let mut close_requested = false;

            // Drain events FIRST using the current single-source-per-device
            // routing before attempting any deferred grab retries.
            unsafe {
                while (x11.XPending)(dpy) > 0 {
                    let mut ev: XEvent = std::mem::zeroed();
                    (x11.XNextEvent)(dpy, &mut ev);
                    match ev.ty {
                        MOTION_NOTIFY if ptr_grabbed => {
                            let m = ev.motion;
                            mods = x11_mods(m.state);
                            virt_cursor.set_clamped(m.x as f32, m.y as f32, win_w, win_h);
                            let (x, y) = scale_point(m.x, m.y, win_w, win_h);
                            push_event(&mut events, &mut held, Input::Move { x, y });
                        }
                        BUTTON_PRESS | BUTTON_RELEASE if ptr_grabbed => {
                            let b = ev.button;
                            let down = ev.ty == BUTTON_PRESS;
                            mods = x11_mods(b.state);
                            virt_cursor.set_clamped(b.x as f32, b.y as f32, win_w, win_h);
                            let (x, y) = scale_point(b.x, b.y, win_w, win_h);
                            if let Some(inp) = map_x11_button(b.button, down, x, y) {
                                push_event(&mut events, &mut held, inp);
                            }
                        }
                        KEY_PRESS | KEY_RELEASE if kbd_grabbed => {
                            let k = ev.key;
                            let down = ev.ty == KEY_PRESS;
                            handle_xkey_event(
                                &x11,
                                k,
                                down,
                                cur_ov,
                                cur_sh,
                                &mut mods,
                                &mut latch,
                                &mut held,
                                &mut close_requested,
                                &mut events,
                            );
                            if close_requested {
                                break;
                            }
                        }
                        GENERIC_EVENT if xi2_active => {
                            let (Some(xi), Some(get_data), Some(free_data)) =
                                (xi2.as_ref(), x11.XGetEventData, x11.XFreeEventData)
                            else {
                                continue;
                            };
                            let cookie = &mut ev.xcookie;
                            if cookie.extension != xi.opcode {
                                continue;
                            }
                            if get_data(dpy, cookie) == 0 {
                                continue;
                            }
                            let guard = CookieGuard { free_fn: free_data, dpy, cookie };
                            if guard.cookie.data.is_null() {
                                continue;
                            }
                            match guard.cookie.evtype {
                                XI_RAW_MOTION if !ptr_grabbed => {
                                    let raw = &*(guard.cookie.data as *const XIRawEvent);
                                    let mask_len = raw.valuators.mask_len;
                                    let vals_ptr = if !raw.raw_values.is_null() {
                                        raw.raw_values
                                    } else {
                                        raw.valuators.values
                                    };
                                    if mask_len > 0 && (mask_len as usize) <= 64 && !raw.valuators.mask.is_null() && !vals_ptr.is_null() {
                                        let mask_slice = std::slice::from_raw_parts(raw.valuators.mask, mask_len as usize);
                                        let present = (0..mask_slice.len().saturating_mul(8).min(2))
                                            .filter(|&b| (mask_slice[b >> 3] & (1u8 << (b & 7))) != 0)
                                            .count();
                                        let (dx, dy) = extract_raw_motion_deltas_with(mask_slice, |i| {
                                            if i < present {
                                                // libXi packs f64 values immediately after the byte mask,
                                                // which can be 4-byte aligned; always read unaligned.
                                                Some(std::ptr::read_unaligned(vals_ptr.add(i)))
                                            } else {
                                                None
                                            }
                                        });
                                        if virt_cursor.apply_delta(dx, dy, win_w, win_h) {
                                            xi2_saw_raw_ptr = true;
                                            let (x, y) = scale_point_f32(virt_cursor.x, virt_cursor.y, win_w, win_h);
                                            push_event(&mut events, &mut held, Input::Move { x, y });
                                        }
                                    }
                                }
                                XI_RAW_BUTTON_PRESS | XI_RAW_BUTTON_RELEASE if !ptr_grabbed => {
                                    let raw = &*(guard.cookie.data as *const XIRawEvent);
                                    xi2_saw_raw_ptr = true;
                                    let down = guard.cookie.evtype == XI_RAW_BUTTON_PRESS;
                                    let (x, y) = scale_point_f32(virt_cursor.x, virt_cursor.y, win_w, win_h);
                                    if let Some(inp) = map_x11_button(raw.detail as c_uint, down, x, y) {
                                        push_event(&mut events, &mut held, inp);
                                    }
                                }
                                XI_RAW_KEY_PRESS | XI_RAW_KEY_RELEASE if !kbd_grabbed => {
                                    let raw = &*(guard.cookie.data as *const XIRawEvent);
                                    let down = guard.cookie.evtype == XI_RAW_KEY_PRESS;
                                    let kc = raw.detail as u8;
                                    let side_kcs = [mods_kc.0, mods_kc.1, mods_kc.2, mods_kc.3, mods_kc.4, mods_kc.5];
                                    for (slot, &mkc) in mod_sides.iter_mut().zip(side_kcs.iter()) {
                                        if mkc != 0 && kc == mkc {
                                            *slot = down;
                                        }
                                    }
                                    raw_x11_state = derive_raw_state(&mod_sides);
                                    let mut synth: XKeyEvent = std::mem::zeroed();
                                    synth.ty = if down { KEY_PRESS } else { KEY_RELEASE };
                                    synth.display = dpy;
                                    synth.window = win;
                                    synth.state = raw_x11_state;
                                    synth.keycode = raw.detail as c_uint;
                                    handle_xkey_event(
                                        &x11,
                                        synth,
                                        down,
                                        cur_ov,
                                        cur_sh,
                                        &mut mods,
                                        &mut latch,
                                        &mut held,
                                        &mut close_requested,
                                        &mut events,
                                    );
                                    if close_requested {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }

                // Always poll hotkeys + Esc via XQueryKeymap (even when kbd_grabbed)
                // so Shift+Tab / Esc can NEVER get stuck if a compositor dialog appears.
                if !close_requested {
                    (x11.XQueryKeymap)(dpy, keymap.as_mut_ptr().cast());
                    let ov_down = combo_matches(&keymap, cur_ov.0, cur_ov.2, mods_kc);
                    let esc_down = key_down(&keymap, kc_esc);
                    let shot_down = combo_matches(&keymap, cur_sh.0, cur_sh.2, mods_kc);
                    let (ov_rise, esc_rise, shot_rise) = latch.poll(ov_down, esc_down, shot_down);
                    if shot_rise {
                        shared().request_screenshot();
                    }
                    if ov_rise || esc_rise {
                        close_requested = true;
                    }
                }

                // Secondary pointer fallback if XInput2 is unavailable or hasn't
                // delivered any raw pointer events yet while Wine holds the grab.
                if !ptr_grabbed && !close_requested && (!xi2_active || !xi2_saw_raw_ptr) {
                    let (mut root, mut child) = (0, 0);
                    let (mut rx, mut ry, mut wx, mut wy) = (0, 0, 0, 0);
                    let mut mask = 0u32;
                    if (x11.XQueryPointer)(dpy, win, &mut root, &mut child, &mut rx, &mut ry, &mut wx, &mut wy, &mut mask) != 0 {
                        mods = x11_mods(mask);
                        let (x, y) = scale_point(wx, wy, win_w, win_h);
                        if last_poll_pos != Some((wx, wy)) {
                            last_poll_pos = Some((wx, wy));
                            virt_cursor.set_clamped(wx as f32, wy as f32, win_w, win_h);
                            push_event(&mut events, &mut held, Input::Move { x, y });
                        }
                        for (bit, btn) in [(BUTTON1_MASK, 0u8), (BUTTON3_MASK, 1), (BUTTON2_MASK, 2)] {
                            let was = last_poll_mask & bit != 0;
                            let now = mask & bit != 0;
                            if was != now {
                                push_event(&mut events, &mut held, Input::Button { button: btn, down: now, x, y });
                            }
                        }
                        last_poll_mask = mask;
                    }
                }
            }

            if close_requested {
                shared().notify_opened(false);
                break;
            }
            if !events.is_empty() {
                shared().send_input(mods, events);
            }

            // Defer grab retries until after event draining and only when no
            // buttons/keys are currently held on that device so source handoff
            // never drops a release event.
            if ticks % 12 == 0 {
                let prev = (ptr_grabbed, ptr_code, kbd_grabbed, kbd_code);
                let mut changed_grab = false;
                if !ptr_grabbed && !held.any_buttons_held() {
                    (ptr_grabbed, ptr_code) = unsafe { try_grab_ptr(&x11, dpy, shield, win, ptr_mask, arrow_cursor) };
                    if ptr_grabbed {
                        changed_grab = true;
                    }
                }
                if !kbd_grabbed && !held.any_keys_held() {
                    (kbd_grabbed, kbd_code) = unsafe { try_grab_kbd(&x11, dpy, shield, win) };
                    if kbd_grabbed {
                        changed_grab = true;
                    }
                }
                if changed_grab {
                    unsafe {
                        x11.sync_or_flush(dpy);
                        x11.drain_pending(dpy);
                    }
                }
                if (ptr_grabbed, ptr_code, kbd_grabbed, kbd_code) != prev {
                    eprintln!(
                        "[aurora-vk-overlay] grab update: ptr_grab={ptr_grabbed}({ptr_code}:{}) kbd_grab={kbd_grabbed}({kbd_code}:{})",
                        grab_status_name(ptr_code),
                        grab_status_name(kbd_code),
                    );
                }
            }

            std::thread::sleep(Duration::from_millis(8));
        }

        // Release any still-held UI buttons/keys and send Leave before teardown.
        let (sx, sy) = scale_point_f32(virt_cursor.x, virt_cursor.y, win_w, win_h);
        shared().send_input(0, held.release_all(sx, sy));

        unsafe {
            if xi2_active {
                if let Some(xi) = xi2.as_ref() {
                    let _ = xi.select_raw(&x11, dpy, false);
                }
            }
            if ptr_grabbed {
                (x11.XUngrabPointer)(dpy, CURRENT_TIME);
            }
            if kbd_grabbed {
                (x11.XUngrabKeyboard)(dpy, CURRENT_TIME);
            }
            if shield != 0 {
                if let Some(destroy_fn) = x11.XDestroyWindow {
                    destroy_fn(dpy, shield);
                }
                // Do NOT call XSetInputFocus(win) here: opening the child
                // InputOnly shield did not change top-level WM focus, and
                // writing focus on close can undo Alt+Tab after focus loss.
            }
            if arrow_cursor != 0 {
                if let Some(free_fn) = x11.XFreeCursor {
                    free_fn(dpy, arrow_cursor);
                }
            }
            x11.sync_or_flush(dpy);
            x11.drain_pending(dpy);
        }
    }

    unsafe { (x11.XCloseDisplay)(dpy) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grab_status_names_and_xi2_version_check() {
        assert_eq!(grab_status_name(0), "Success");
        assert_eq!(grab_status_name(1), "AlreadyGrabbed");
        assert_eq!(grab_status_name(2), "InvalidTime");
        assert_eq!(grab_status_name(3), "GrabNotViewable");
        assert_eq!(grab_status_name(4), "GrabFrozen");
        assert_eq!(grab_status_name(99), "Unknown");

        assert!(!xi2_version_supports_grabbed_raw(2, 0));
        assert!(xi2_version_supports_grabbed_raw(2, 1));
        assert!(xi2_version_supports_grabbed_raw(2, 4));
        assert!(xi2_version_supports_grabbed_raw(3, 0));
    }

    #[test]
    fn extract_raw_motion_deltas_handles_masks_unaligned_and_bounds() {
        let from_slice = |m: &[u8], v: &[f64]| extract_raw_motion_deltas_with(m, |i| v.get(i).copied());
        assert_eq!(from_slice(&[0b11], &[12.5, -4.25]), (12.5, -4.25));
        assert_eq!(from_slice(&[0b10], &[-7.0]), (0.0, -7.0));
        assert_eq!(from_slice(&[0b01], &[3.5]), (3.5, 0.0));
        assert_eq!(from_slice(&[0b100], &[1.0]), (0.0, 0.0));
        assert_eq!(from_slice(&[0b11], &[f64::NAN, 99999.0]), (0.0, 4096.0));
        assert_eq!(from_slice(&[0b11], &[5.0]), (5.0, 0.0));

        // Verify unaligned raw byte buffer (simulating 4-byte mask followed by f64).
        let mut buf = [0u8; 4 + 16];
        buf[0] = 0b11;
        buf[4..12].copy_from_slice(&(-3.5f64).to_ne_bytes());
        buf[12..20].copy_from_slice(&(8.25f64).to_ne_bytes());
        let ptr = unsafe { buf.as_ptr().add(4) as *const f64 };
        let res = extract_raw_motion_deltas_with(&buf[..4], |i| {
            (i < 2).then(|| unsafe { std::ptr::read_unaligned(ptr.add(i)) })
        });
        assert_eq!(res, (-3.5, 8.25));
    }

    #[test]
    fn hotkey_latch_prevents_open_to_immediate_close_bounce() {
        let mut l = HotkeyLatch::default();
        // Polling detects Shift+Tab -> opens overlay and latches ov_held.
        assert_eq!(l.poll(true, false, false), (true, false, false));
        // Event arriving while Shift+Tab is still held must NOT immediately close.
        assert!(!l.trigger_ov_event());
        // Escape works independently even while Shift+Tab is held.
        assert!(l.trigger_esc_event());
        assert!(!l.trigger_esc_event());
        // Polling with Shift+Tab released clears the overlay latch (immune to X11 autorepeat).
        assert_eq!(l.poll(false, false, false), (false, false, false));
        assert!(l.trigger_ov_event());
    }

    #[test]
    fn held_inputs_tracks_and_releases_cleanly() {
        let mut h = HeldInputs::default();
        assert!(!h.any_buttons_held());
        assert!(!h.any_keys_held());
        h.record(&Input::Button { button: 0, down: true, x: 10.0, y: 20.0 });
        h.record(&Input::Key { vk: 0x41, down: true, repeat: false });
        assert!(h.any_buttons_held());
        assert!(h.any_keys_held());
        let released = h.release_all(10.0, 20.0);
        assert!(!h.any_buttons_held());
        assert!(!h.any_keys_held());
        assert_eq!(
            released,
            vec![
                Input::Button { button: 0, down: false, x: 10.0, y: 20.0 },
                Input::Key { vk: 0x41, down: false, repeat: false },
                Input::Leave,
            ]
        );
    }

    #[test]
    fn virtual_cursor_clamps_and_maps_buttons() {
        let mut cur = VirtualCursor::centered(1920, 1080);
        assert_eq!((cur.x, cur.y), (960.0, 540.0));
        assert!(cur.apply_delta(2000.0, -1000.0, 1920, 1080));
        assert_eq!((cur.x, cur.y), (1919.0, 0.0));
        assert!(!cur.apply_delta(10.0, -10.0, 1920, 1080));

        assert_eq!(map_x11_button(1, true, 10.0, 20.0), Some(Input::Button { button: 0, down: true, x: 10.0, y: 20.0 }));
        assert_eq!(map_x11_button(3, false, 10.0, 20.0), Some(Input::Button { button: 1, down: false, x: 10.0, y: 20.0 }));
        assert_eq!(map_x11_button(4, true, 10.0, 20.0), Some(Input::Wheel { x: 0.0, y: 1.0 }));
        assert_eq!(map_x11_button(4, false, 10.0, 20.0), None);
        assert_eq!(map_x11_button(5, true, 10.0, 20.0), Some(Input::Wheel { x: 0.0, y: -1.0 }));
    }
}
