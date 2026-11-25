//! Controllers inside the game: the real XInput pads, plus the pads XInput
//! can't see (PlayStation, Switch, …) that the app reads with SDL3 and shares
//! (pads_shm.rs). Those fill the XInput slots real pads leave free. Steam
//! Input (input.rs) reads pads through here, and [`install`] hooks the game's
//! XInput calls (inline detours on every loaded XInput DLL, plus IAT/delay-load
//! and `GetProcAddress` fallback) so XInput games see them as Xbox pads, even
//! when the engine cached XInput function pointers before `SteamAPI_Init()`.
//!
//! SDL's own modules (`sdl*.dll`) bypass synthetic injection so SDL doesn't see
//! PlayStation/Switch pads twice (once via HID and once via our XInput bridge).

#![cfg_attr(not(windows), allow(dead_code))]

use crate::legacy::{self, Sample};
use crate::pads_shm::{self, GUIDE, MappedShm, Shared, State};
use std::collections::VecDeque;
use vdf::controller::LegacyMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// XINPUT_STATE.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Pad {
    pub packet: u32,
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

impl From<State> for Pad {
    fn from(s: State) -> Pad {
        Pad { packet: s.packet, buttons: s.buttons, lt: s.lt, rt: s.rt, lx: s.lx, ly: s.ly, rx: s.rx, ry: s.ry }
    }
}

/// XINPUT_KEYSTROKE (8 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Keystroke {
    pub virtual_key: u16,
    pub unicode: u16,
    pub flags: u16,
    pub user_index: u8,
    pub hid_code: u8,
}

type GetState = unsafe extern "system" fn(u32, *mut Pad) -> u32;
type SetState = unsafe extern "system" fn(u32, *mut [u16; 2]) -> u32;
type GetCaps = unsafe extern "system" fn(u32, u32, *mut [u8; 20]) -> u32;
type GetBattery = unsafe extern "system" fn(u32, u8, *mut [u8; 2]) -> u32;
type GetKeystroke = unsafe extern "system" fn(u32, u32, *mut Keystroke) -> u32;
type EnableFn = unsafe extern "system" fn(i32);
type GetProc = unsafe extern "system" fn(*mut c_void, *const c_char) -> *mut c_void;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, len: u32) -> u32;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut *mut c_void, hash: *mut u32) -> u16;
}
#[cfg(not(windows))]
mod non_windows {
    #![allow(non_snake_case)]
    use std::ffi::c_void;

    pub unsafe fn LoadLibraryW(_: *const u16) -> *mut c_void { std::ptr::null_mut() }
    pub unsafe fn GetModuleHandleW(_: *const u16) -> *mut c_void { std::ptr::null_mut() }
    pub unsafe fn GetModuleFileNameW(_: *mut c_void, _: *mut u16, _: u32) -> u32 { 0 }
    pub unsafe fn GetProcAddress(_: *mut c_void, _: *const u8) -> *mut c_void { std::ptr::null_mut() }
}
#[cfg(not(windows))]
use non_windows::*;

const ERROR_SUCCESS: u32 = 0;
const NOT_CONNECTED: u32 = 1167;
const ERROR_EMPTY: u32 = 4306;
const XUSER_INDEX_ANY: u32 = 0x0000_00FF;
const KEYSTROKE_KEYDOWN: u16 = 0x0001;
const KEYSTROKE_KEYUP: u16 = 0x0002;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// XInput's DLLs, newest first (matching `overlay/pad.rs` plus `xinputuap.dll`).
const XINPUT_DLLS: [&str; 6] = [
    "xinput1_4.dll",
    "xinput1_3.dll",
    "xinput9_1_0.dll",
    "xinput1_2.dll",
    "xinput1_1.dll",
    "xinputuap.dll",
];

pub struct XInput {
    pub dll_idx: usize,
    pub get: GetState,
    pub set: SetState,
    /// XInputGetStateEx (ordinal 100): also the guide button.
    get_ex: Option<GetState>,
    caps: Option<GetCaps>,
    battery: Option<GetBattery>,
    keystroke: Option<GetKeystroke>,
    enable: Option<EnableFn>,
}

/// Per-DLL original function trampolines populated by inline detours.
/// When non-zero, calling the trampoline invokes the real XInput code without
/// re-entering our detours.
static ORIG_GET: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
static ORIG_GET_EX: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
static ORIG_SET: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
static ORIG_CAPS: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
static ORIG_BATTERY: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
static ORIG_KEYSTROKE: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
static ORIG_ENABLE: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];

/// Tracks `XInputEnable(BOOL)` state for synthetic shared pads.
static XINPUT_ENABLED: AtomicBool = AtomicBool::new(true);

/// Cached `(base, end)` address ranges of any loaded `sdl*.dll` modules so our
/// inline XInput hooks avoid feeding shared pads back to SDL (preventing double input).
static HAS_SDL_MODULES: AtomicBool = AtomicBool::new(false);
static SDL_RANGES: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Log the first time a shared pad is actually delivered to the game per slot.
static LOGGED_SERVED: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];

/// The primary XInput DLL (loaded on demand when reading pads internally).
#[cfg(windows)]
pub static XINPUT: LazyLock<Option<XInput>> = LazyLock::new(|| {
    for (dll_idx, dll) in XINPUT_DLLS.iter().enumerate() {
        let m = unsafe { LoadLibraryW(wide(dll).as_ptr()) };
        if m.is_null() {
            continue;
        }
        let f = |name: *const c_char| unsafe { GetProcAddress(m, name.cast()) };
        let (g, s) = (f(c"XInputGetState".as_ptr()), f(c"XInputSetState".as_ptr()));
        if g.is_null() || s.is_null() {
            continue;
        }
        let opt = |p: *mut c_void| (!p.is_null()).then_some(p);
        unsafe {
            return Some(XInput {
                dll_idx,
                get: std::mem::transmute::<*mut c_void, GetState>(g),
                set: std::mem::transmute::<*mut c_void, SetState>(s),
                get_ex: opt(f(100 as *const c_char)).map(|p| std::mem::transmute::<*mut c_void, GetState>(p)),
                caps: opt(f(c"XInputGetCapabilities".as_ptr())).map(|p| std::mem::transmute::<*mut c_void, GetCaps>(p)),
                battery: opt(f(c"XInputGetBatteryInformation".as_ptr())).map(|p| std::mem::transmute::<*mut c_void, GetBattery>(p)),
                keystroke: opt(f(c"XInputGetKeystroke".as_ptr())).map(|p| std::mem::transmute::<*mut c_void, GetKeystroke>(p)),
                enable: opt(f(c"XInputEnable".as_ptr())).map(|p| std::mem::transmute::<*mut c_void, EnableFn>(p)),
            });
        }
    }
    None
});

#[cfg(not(windows))]
pub static XINPUT: LazyLock<Option<XInput>> = LazyLock::new(|| None);

/// Invoke the unhooked real `XInputGetState` (or `Ex`) for the primary XInput DLL.
fn call_primary_get(x: &XInput, i: u32, out: *mut Pad, ex: bool) -> u32 {
    let tramp = if ex {
        let t = ORIG_GET_EX[x.dll_idx].load(Ordering::Acquire);
        if t != 0 { t } else { ORIG_GET[x.dll_idx].load(Ordering::Acquire) }
    } else {
        ORIG_GET[x.dll_idx].load(Ordering::Acquire)
    };
    if tramp != 0 {
        let f: GetState = unsafe { std::mem::transmute(tramp) };
        return unsafe { f(i, out) };
    }
    let f = if ex { x.get_ex.unwrap_or(x.get) } else { x.get };
    unsafe { f(i, out) }
}

fn call_primary_set(x: &XInput, i: u32, v: *mut [u16; 2]) -> u32 {
    let tramp = ORIG_SET[x.dll_idx].load(Ordering::Acquire);
    if tramp != 0 {
        let f: SetState = unsafe { std::mem::transmute(tramp) };
        return unsafe { f(i, v) };
    }
    unsafe { (x.set)(i, v) }
}

fn call_primary_caps(x: &XInput, i: u32, flags: u32, out: *mut [u8; 20]) -> u32 {
    let tramp = ORIG_CAPS[x.dll_idx].load(Ordering::Acquire);
    if tramp != 0 {
        let f: GetCaps = unsafe { std::mem::transmute(tramp) };
        return unsafe { f(i, flags, out) };
    }
    x.caps.map_or(NOT_CONNECTED, |f| unsafe { f(i, flags, out) })
}

fn call_primary_battery(x: &XInput, i: u8, kind: u8, out: *mut [u8; 2]) -> u32 {
    let tramp = ORIG_BATTERY[x.dll_idx].load(Ordering::Acquire);
    if tramp != 0 {
        let f: GetBattery = unsafe { std::mem::transmute(tramp) };
        return unsafe { f(i as u32, kind, out) };
    }
    x.battery.map_or(NOT_CONNECTED, |f| unsafe { f(i as u32, kind, out) })
}

fn call_primary_keystroke(x: &XInput, i: u32, reserved: u32, out: *mut Keystroke) -> u32 {
    let tramp = ORIG_KEYSTROKE[x.dll_idx].load(Ordering::Acquire);
    if tramp != 0 {
        let f: GetKeystroke = unsafe { std::mem::transmute(tramp) };
        return unsafe { f(i, reserved, out) };
    }
    x.keystroke.map_or(ERROR_EMPTY, |f| unsafe { f(i, reserved, out) })
}

// ------------------------------------------------------------ shared pads

static MAPPED: std::sync::OnceLock<MappedShm> = std::sync::OnceLock::new();
static LAST_MAP_TRY: Mutex<Option<Instant>> = Mutex::new(None);
static LOGGED_MAP_ERR: AtomicBool = AtomicBool::new(false);

/// Per-slot packet bump so neutralizing/restoring state on overlay open or
/// `XInputEnable(FALSE)` always produces a new `dwPacketNumber`.
static PACKET_BUMP: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static LAST_NEUTRAL: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
/// Last rumble requested per virtual slot, restored when `XInputEnable(TRUE)`.
#[derive(Clone, Copy)]
struct SavedRumble {
    slot: usize,
    generation: u32,
    motors: [u16; 2],
}
static LAST_RUMBLE: Mutex<[Option<SavedRumble>; 4]> = Mutex::new([None; 4]);

fn open_shm(name: &str) -> Option<MappedShm> {
    match pads_shm::open_shm_with_error(name) {
        Ok(m) => {
            crate::log!("pads: connected to shared pads at {name} (writable={})", m.writable);
            Some(m)
        }
        Err(err) => {
            if !LOGGED_MAP_ERR.swap(true, Ordering::Relaxed) {
                crate::log!("pads: couldn't open shared pads at {name} (error {err})");
            }
            None
        }
    }
}

/// The app's shared memory, when it gave us one (Windows only).
/// Retryable if the first open failed transiently.
fn mapped_info() -> Option<MappedShm> {
    if let Some(m) = MAPPED.get() {
        return Some(*m);
    }
    let name = std::env::var(pads_shm::ENV_PADS).ok()?;
    let mut last = LAST_MAP_TRY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = MAPPED.get() {
        return Some(*m);
    }
    if last.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
        return None;
    }
    *last = Some(Instant::now());
    let m = open_shm(&name)?;
    Some(*MAPPED.get_or_init(|| m))
}

fn mapped() -> Option<&'static Shared> {
    mapped_info().map(|m| m.shared)
}

fn mapped_writable() -> bool {
    MAPPED.get().is_some_and(|m| m.writable)
}

/// The shared pads, while the app keeps them up to date and Steam Input is enabled.
fn shared() -> Option<&'static Shared> {
    if !steam_input_enabled() {
        return None;
    }
    mapped().filter(|s| s.live())
}

pub fn steam_input_enabled() -> bool {
    pads_shm::steam_input_env_enabled(std::env::var(pads_shm::ENV_STEAM_INPUT).ok().as_deref())
}

pub fn family_mask() -> u32 {
    pads_shm::parse_family_mask(std::env::var(pads_shm::ENV_INPUT_FAMILIES).ok().as_deref())
}

pub fn calibration() -> pads_shm::PadCalibration {
    std::env::var(pads_shm::ENV_INPUT_CALIBRATION).ok().map(|s| pads_shm::PadCalibration::decode(&s)).unwrap_or_default()
}

/// Shared slots with a pad in them whose controller family is enabled.
fn shared_pads(s: &Shared) -> Vec<usize> {
    let mask = family_mask();
    (0..pads_shm::SLOTS)
        .filter(|k| s.slots[*k].read().is_some_and(|st| pads_shm::family_allowed(mask, st.family)))
        .collect()
}

// ------------------------------------------------------------- real pads

/// Which slots hold a real pad, as last asked; empty slots are asked again
/// at most once a second (asking an empty slot is slow).
struct Real {
    connected: [bool; 4],
    asked: [Option<Instant>; 4],
}

static REAL: Mutex<Real> = Mutex::new(Real { connected: [false; 4], asked: [None; 4] });

fn note(i: usize, connected: bool) {
    if i < 4 {
        let mut r = REAL.lock().unwrap_or_else(|e| e.into_inner());
        r.connected[i] = connected;
        r.asked[i] = Some(Instant::now());
    }
}

fn real_state(x: &XInput, i: usize) -> Option<Pad> {
    {
        let r = REAL.lock().unwrap_or_else(|e| e.into_inner());
        if !r.connected[i] && r.asked[i].is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return None;
        }
    }
    let mut p = Pad::default();
    let ok = call_primary_get(x, i as u32, &mut p, false) == ERROR_SUCCESS;
    note(i, ok);
    ok.then_some(p)
}

/// Refresh any real XInput slots that have never been probed (or whose cached
/// occupancy is stale) so `GetCapabilities`/`GetBattery`/`SetState` agree with
/// `GetState` on slot seating.
fn refresh_real_snapshot(skip: Option<usize>) -> [bool; 4] {
    let stale: [bool; 4] = {
        let r = REAL.lock().unwrap_or_else(|e| e.into_inner());
        std::array::from_fn(|i| {
            if skip == Some(i) {
                false
            } else {
                r.asked[i].is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
            }
        })
    };
    if let Some(x) = XINPUT.as_ref() {
        for (i, need) in stale.into_iter().enumerate() {
            if need {
                let mut p = Pad::default();
                let ok = call_primary_get(x, i as u32, &mut p, false) == ERROR_SUCCESS;
                note(i, ok);
            }
        }
    }
    REAL.lock().unwrap_or_else(|e| e.into_inner()).connected
}

fn real_connected() -> [bool; 4] {
    REAL.lock().unwrap_or_else(|e| e.into_inner()).connected
}

/// The shared pad in XInput slot `i` (with coherent real-pad seating).
fn shared_at(i: usize) -> Option<(&'static Shared, usize)> {
    let s = shared()?;
    let real = refresh_real_snapshot(None);
    let k = (*pads_shm::seats(real, &shared_pads(s)).get(i)?)?;
    Some((s, k))
}

fn shared_at_after_probe(i: usize) -> Option<(&'static Shared, usize)> {
    let s = shared()?;
    let real = refresh_real_snapshot(Some(i));
    let k = (*pads_shm::seats(real, &shared_pads(s)).get(i)?)?;
    Some((s, k))
}

// ---------------------------------------------------- what Steam Input sees

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SteamInputPad {
    pub device_id: u32,
    pub pad: Pad,
    pub calibrated: bool,
    pub family: u8,
    pub paddles: u8,
}

impl SteamInputPad {
    pub fn input_type(self) -> i32 {
        match self.family {
            pads_shm::SLOT_FAMILY_XBOX => 3,
            pads_shm::SLOT_FAMILY_PLAYSTATION => 5,
            pads_shm::SLOT_FAMILY_SWITCH_PRO => 8,
            _ => 4, // Generic gamepad
        }
    }
}

/// The four pads as XInput / games see them.
pub fn all() -> [Option<Pad>; 4] {
    let mut pads: [Option<Pad>; 4] = std::array::from_fn(|i| XINPUT.as_ref().and_then(|x| real_state(x, i)));
    if let Some(s) = shared() {
        let seats = pads_shm::seats(pads.map(|p| p.is_some()), &shared_pads(s));
        for (i, k) in seats.iter().enumerate() {
            if let Some(k) = k {
                pads[i] = s.slots[*k].read().map(Pad::from);
            }
        }
    }
    pads
}

/// The four pads as Steam Input sees them, respecting per-game disable and
/// global controller family toggles.
pub fn steam_input_pads() -> [Option<SteamInputPad>; 4] {
    steam_input_pads_with(
        steam_input_enabled(),
        family_mask(),
        std::array::from_fn(|i| XINPUT.as_ref().and_then(|x| real_state(x, i))),
        shared(),
    )
}

fn steam_input_pads_with(
    enabled: bool,
    mask: u32,
    real: [Option<Pad>; 4],
    shm: Option<&Shared>,
) -> [Option<SteamInputPad>; 4] {
    if !enabled {
        return [None; 4];
    }
    let xbox_on = mask & pads_shm::FAMILY_XBOX != 0;
    let mut out: [Option<SteamInputPad>; 4] = real.map(|p| {
        p.filter(|_| xbox_on)
            .map(|pad| SteamInputPad { device_id: 0, pad, calibrated: false, family: pads_shm::SLOT_FAMILY_XBOX, paddles: 0 })
    });
    if let Some(s) = shm {
        let active_shared: Vec<usize> = (0..pads_shm::SLOTS)
            .filter(|k| s.slots[*k].read().is_some_and(|st| pads_shm::family_allowed(mask, st.family)))
            .collect();
        let seats = pads_shm::seats(real.map(|p| p.is_some()), &active_shared);
        for (i, k) in seats.iter().enumerate() {
            if let Some(k) = k
                && let Some(st) = s.slots[*k].read()
            {
                out[i] = Some(SteamInputPad { device_id: st.device_id, pad: Pad::from(st), calibrated: st.calibrated, family: st.family, paddles: st.paddles });
            }
        }
    }
    out
}

/// Rumble on XInput slot `i`.
pub fn rumble(i: usize, low: u16, high: u16) {
    if !steam_input_enabled() || !calibration().rumble_enabled {
        return;
    }
    if let Some((s, k)) = shared_at(i) {
        if mapped_writable() {
            s.slots[k].set_rumble(low, high);
        }
    } else if let Some(x) = XINPUT.as_ref() {
        let mut v = [low, high];
        call_primary_set(x, i as u32, &mut v);
    }
}

/// Motion sensors (accel in m/s^2, gyro in rad/s) for XInput slot `i`, when
/// backed by an SDL3 pad that exposes IMU sensors.
pub fn motion(i: usize) -> Option<([f32; 3], [f32; 3])> {
    let cal = calibration();
    if !steam_input_enabled() || !cal.gyro_enabled {
        return None;
    }
    let (s, k) = shared_at(i)?;
    let st = s.slots[k].read()?;
    if !st.has_motion {
        return None;
    }
    let mut gyro = st.gyro();
    if !st.calibrated && (cal.gyro_sensitivity - 1.0).abs() > f32::EPSILON {
        gyro = [gyro[0] * cal.gyro_sensitivity, gyro[1] * cal.gyro_sensitivity, gyro[2] * cal.gyro_sensitivity];
    }
    Some((st.accel(), gyro))
}

// --------------------------------------------------- synthetic keystrokes

/// Map XInput button bits to `VK_PAD_*` virtual key codes.
const VK_BUTTONS: [(u16, u16); 14] = [
    (0x1000, 0x5800), // VK_PAD_A
    (0x2000, 0x5801), // VK_PAD_B
    (0x4000, 0x5802), // VK_PAD_X
    (0x8000, 0x5803), // VK_PAD_Y
    (0x0200, 0x5804), // VK_PAD_RSHOULDER
    (0x0100, 0x5805), // VK_PAD_LSHOULDER
    (0x0001, 0x5810), // VK_PAD_DPAD_UP
    (0x0002, 0x5811), // VK_PAD_DPAD_DOWN
    (0x0004, 0x5812), // VK_PAD_DPAD_LEFT
    (0x0008, 0x5813), // VK_PAD_DPAD_RIGHT
    (0x0010, 0x5814), // VK_PAD_START
    (0x0020, 0x5815), // VK_PAD_BACK
    (0x0040, 0x5816), // VK_PAD_LTHUMB_PRESS
    (0x0080, 0x5817), // VK_PAD_RTHUMB_PRESS
];
const VK_PAD_LTRIGGER: u16 = 0x5806;
const VK_PAD_RTRIGGER: u16 = 0x5807;

#[derive(Default)]
struct KeystrokeTracker {
    prev_buttons: [u16; 4],
    prev_lt: [bool; 4],
    prev_rt: [bool; 4],
    queue: [VecDeque<Keystroke>; 4],
}

impl KeystrokeTracker {
    fn update_slot(&mut self, slot: usize, buttons: u16, lt: u8, rt: u8) {
        if slot >= 4 {
            return;
        }
        let prev = self.prev_buttons[slot];
        let diff = prev ^ buttons;
        if diff != 0 {
            for &(bit, vk) in &VK_BUTTONS {
                if diff & bit != 0 {
                    let flags = if buttons & bit != 0 { KEYSTROKE_KEYDOWN } else { KEYSTROKE_KEYUP };
                    if self.queue[slot].len() < 32 {
                        self.queue[slot].push_back(Keystroke {
                            virtual_key: vk,
                            unicode: 0,
                            flags,
                            user_index: slot as u8,
                            hid_code: 0,
                        });
                    }
                }
            }
            self.prev_buttons[slot] = buttons;
        }
        let lt_down = lt >= 30;
        if lt_down != self.prev_lt[slot] {
            self.prev_lt[slot] = lt_down;
            if self.queue[slot].len() < 32 {
                self.queue[slot].push_back(Keystroke {
                    virtual_key: VK_PAD_LTRIGGER,
                    unicode: 0,
                    flags: if lt_down { KEYSTROKE_KEYDOWN } else { KEYSTROKE_KEYUP },
                    user_index: slot as u8,
                    hid_code: 0,
                });
            }
        }
        let rt_down = rt >= 30;
        if rt_down != self.prev_rt[slot] {
            self.prev_rt[slot] = rt_down;
            if self.queue[slot].len() < 32 {
                self.queue[slot].push_back(Keystroke {
                    virtual_key: VK_PAD_RTRIGGER,
                    unicode: 0,
                    flags: if rt_down { KEYSTROKE_KEYDOWN } else { KEYSTROKE_KEYUP },
                    user_index: slot as u8,
                    hid_code: 0,
                });
            }
        }
    }

    fn pop(&mut self, user: u32) -> Option<Keystroke> {
        if user == XUSER_INDEX_ANY {
            for q in &mut self.queue {
                if let Some(k) = q.pop_front() {
                    return Some(k);
                }
            }
            None
        } else if (user as usize) < 4 {
            self.queue[user as usize].pop_front()
        } else {
            None
        }
    }

    fn clear_all(&mut self) {
        for q in &mut self.queue {
            q.clear();
        }
    }
}

static KEYSTROKES: LazyLock<Mutex<KeystrokeTracker>> = LazyLock::new(|| Mutex::new(KeystrokeTracker::default()));

// ------------------------------------------------------- the game's XInput

#[derive(Default)]
struct LegacyState {
    map: LegacyMap,
    packets: [Option<Pad>; 4],
    revision: u64,
}

static LEGACY: LazyLock<Mutex<LegacyState>> = LazyLock::new(|| Mutex::new(LegacyState::default()));

pub(crate) fn set_legacy_map(map: LegacyMap) {
    let mut legacy = LEGACY.lock().unwrap_or_else(|e| e.into_inner());
    legacy.map = map;
    legacy.revision = legacy.revision.wrapping_add(1);
}

#[cfg(windows)]
pub(crate) fn synthetic_layout() -> (u64, LegacyMap) {
    let state = LEGACY.lock().unwrap_or_else(|e| e.into_inner());
    (state.revision, state.map.clone())
}

#[cfg(windows)]
pub(crate) fn synthetic_revision() -> u64 {
    LEGACY.lock().unwrap_or_else(|e| e.into_inner()).revision
}

#[cfg(windows)]
pub(crate) fn synthetic_pad() -> Option<Sample> {
    steam_input_pads().iter().enumerate().find_map(|(index, entry)| {
        let entry = (*entry)?;
        Some(Sample {
            pad: entry.pad, paddles: entry.paddles,
            gyro: motion(index).map(|(_, gyro)| gyro).unwrap_or_default(),
        })
    })
}

fn translated(i: usize, sample: Sample, family: u8) -> Pad {
    if !steam_input_enabled() || !pads_shm::family_allowed(family_mask(), family) {
        return sample.pad;
    }
    let mut state = LEGACY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(previous) = state.packets.get(i).copied() else { return sample.pad };
    if state.map.0.is_empty() && previous.is_none() {
        return sample.pad;
    }
    let mut mapped = legacy::remap(sample, &state.map);
    if let Some(previous) = previous {
        mapped.packet = previous.packet;
        if mapped != previous {
            mapped.packet = mapped.packet.wrapping_add(1);
        }
    }
    state.packets[i] = Some(mapped);
    mapped
}

/// True when the immediate caller lives inside a loaded `sdl*.dll` module.
#[cfg(windows)]
fn caller_is_sdl() -> bool {
    if !HAS_SDL_MODULES.load(Ordering::Acquire) {
        return false;
    }
    let mut frames = [std::ptr::null_mut::<c_void>(); 6];
    let n = unsafe { RtlCaptureStackBackTrace(1, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    let ranges = SDL_RANGES.lock().unwrap_or_else(|e| e.into_inner());
    for &f in &frames[..n.min(frames.len())] {
        let addr = f as usize;
        if ranges.iter().any(|&(lo, hi)| addr >= lo && addr < hi) {
            return true;
        }
    }
    false
}

#[cfg(not(windows))]
fn caller_is_sdl() -> bool {
    false
}

fn neutralize_pad(p: &mut Pad) {
    p.buttons = 0;
    p.lt = 0;
    p.rt = 0;
    p.lx = 0;
    p.ly = 0;
    p.rx = 0;
    p.ry = 0;
}

fn apply_neutral_and_packet(slot: usize, p: &mut Pad, neutral: bool) {
    if slot < 4 {
        let prev = LAST_NEUTRAL[slot].swap(neutral, Ordering::AcqRel);
        if prev != neutral {
            PACKET_BUMP[slot].fetch_add(1, Ordering::Relaxed);
            KEYSTROKES.lock().unwrap_or_else(|e| e.into_inner()).clear_all();
        }
        p.packet = p.packet.wrapping_add(PACKET_BUMP[slot].load(Ordering::Relaxed));
    }
    if neutral {
        neutralize_pad(p);
    }
}

fn serve_get_state(i: u32, out: *mut Pad, ex: bool, real_fn: Option<GetState>, bypass_shared: bool) -> u32 {
    let r = match real_fn {
        Some(f) => unsafe { f(i, out) },
        None => match XINPUT.as_ref() {
            Some(x) => call_primary_get(x, i, out, ex),
            None => NOT_CONNECTED,
        },
    };
    note(i as usize, r == ERROR_SUCCESS);
    if r == ERROR_SUCCESS {
        if !out.is_null() {
            let raw = unsafe { out.read_unaligned() };
            let mut p = translated(i as usize, Sample { pad: raw, ..Default::default() }, pads_shm::SLOT_FAMILY_XBOX);
            if !ex {
                p.buttons &= !GUIDE;
            }
            apply_neutral_and_packet(i as usize, &mut p, crate::overlay::is_open());
            unsafe { out.write_unaligned(p) };
        }
        return ERROR_SUCCESS;
    }
    if out.is_null() || bypass_shared || (i as usize) >= 4 {
        return r;
    }
    match shared_at_after_probe(i as usize).and_then(|(s, k)| s.slots[k].read()) {
        Some(st) => {
            if !LOGGED_SERVED[i as usize].swap(true, Ordering::Relaxed) {
                crate::log!("pads: serving shared controller on XInput slot {i} (family {})", st.family);
            }
            let sample = Sample {
                pad: Pad::from(st),
                paddles: st.paddles,
                gyro: motion(i as usize).map(|(_, gyro)| gyro).unwrap_or_default(),
            };
            let mut p = translated(i as usize, sample, st.family);
            if !ex {
                p.buttons &= !GUIDE;
            }
            let neutral = !XINPUT_ENABLED.load(Ordering::Acquire) || crate::overlay::is_open();
            apply_neutral_and_packet(i as usize, &mut p, neutral);
            unsafe { out.write_unaligned(p) };
            ERROR_SUCCESS
        }
        None => r,
    }
}

fn serve_set_state(i: u32, v: *mut [u16; 2], real_fn: Option<SetState>, bypass_shared: bool) -> u32 {
    if !bypass_shared && !v.is_null() && (i as usize) < 4 {
        if let Some((s, k)) = shared_at(i as usize) {
            let req = unsafe { v.read_unaligned() };
            let generation = s.slots[k].rumble_generation();
            LAST_RUMBLE.lock().unwrap_or_else(|e| e.into_inner())[i as usize] = Some(SavedRumble { slot: k, generation, motors: req });
            if mapped_writable() {
                let [low, high] = if XINPUT_ENABLED.load(Ordering::Acquire) && calibration().rumble_enabled {
                    req
                } else {
                    [0, 0]
                };
                s.slots[k].set_rumble_for(generation, low, high);
            }
            return ERROR_SUCCESS;
        }
    }
    match real_fn {
        Some(f) => unsafe { f(i, v) },
        None => match XINPUT.as_ref() {
            Some(x) => call_primary_set(x, i, v),
            None => NOT_CONNECTED,
        },
    }
}

fn serve_get_caps(i: u32, flags: u32, out: *mut [u8; 20], real_fn: Option<GetCaps>, bypass_shared: bool) -> u32 {
    let r = match real_fn {
        Some(f) => unsafe { f(i, flags, out) },
        None => match XINPUT.as_ref() {
            Some(x) => call_primary_caps(x, i, flags, out),
            None => NOT_CONNECTED,
        },
    };
    // Only synthesize capabilities for genuinely disconnected slots (never on
    // invalid flags or other errors, and never overwrite REAL occupancy here).
    if r != NOT_CONNECTED || out.is_null() || bypass_shared || (i as usize) >= 4 {
        return r;
    }
    if shared_at(i as usize).is_none() {
        return r;
    }
    // XINPUT_CAPABILITIES: a wired gamepad with every button, full-range
    // axes and both motors.
    let mut c = [0u8; 20];
    c[0] = 1; // XINPUT_DEVTYPE_GAMEPAD
    c[1] = 1; // XINPUT_DEVSUBTYPE_GAMEPAD
    c[4..6].copy_from_slice(&0xF3FFu16.to_le_bytes());
    c[6] = 0xFF;
    c[7] = 0xFF;
    for a in 0..4 {
        c[8 + a * 2..10 + a * 2].copy_from_slice(&0xFFC0u16.to_le_bytes());
    }
    c[16..20].copy_from_slice(&[0xFF; 4]);
    unsafe { out.write_unaligned(c) };
    ERROR_SUCCESS
}

fn serve_get_battery(i: u32, kind: u8, out: *mut [u8; 2], real_fn: Option<GetBattery>, bypass_shared: bool) -> u32 {
    let r = match real_fn {
        Some(f) => unsafe { f(i, kind, out) },
        None => match XINPUT.as_ref() {
            Some(x) => call_primary_battery(x, i as u8, kind, out),
            None => NOT_CONNECTED,
        },
    };
    if r != NOT_CONNECTED || out.is_null() || bypass_shared || (i as usize) >= 4 {
        return r;
    }
    if shared_at(i as usize).is_none() {
        return r;
    }
    unsafe { out.write_unaligned([1, 3]) }; // wired, full
    ERROR_SUCCESS
}

fn serve_get_keystroke(user: u32, reserved: u32, out: *mut Keystroke, real_fn: Option<GetKeystroke>, bypass_shared: bool) -> u32 {
    let call_real = |dst: *mut Keystroke| match real_fn {
        Some(f) => unsafe { f(user, reserved, dst) },
        None => match XINPUT.as_ref() {
            Some(x) => call_primary_keystroke(x, user, reserved, dst),
            None => ERROR_EMPTY,
        },
    };
    if crate::overlay::is_open() {
        let mut scratch = Keystroke::default();
        for _ in 0..64 {
            if call_real(&mut scratch) != ERROR_SUCCESS {
                break;
            }
        }
        KEYSTROKES.lock().unwrap_or_else(|e| e.into_inner()).clear_all();
        return ERROR_EMPTY;
    }
    let r = call_real(out);
    if r == ERROR_SUCCESS || out.is_null() || bypass_shared {
        return r;
    }
    if !XINPUT_ENABLED.load(Ordering::Acquire) {
        KEYSTROKES.lock().unwrap_or_else(|e| e.into_inner()).clear_all();
        return ERROR_EMPTY;
    }
    // Sample virtual seats BEFORE taking the KEYSTROKES mutex so we never hold
    // KEYSTROKES across native XInput probes inside `shared_at()`.
    let mut sampled: [Option<(u16, u8, u8)>; 4] = [None; 4];
    let mut any_virtual = false;
    let range = if user == XUSER_INDEX_ANY {
        0..4
    } else if (user as usize) < 4 {
        (user as usize)..(user as usize + 1)
    } else {
        return r;
    };
    for slot in range {
        if let Some((s, k)) = shared_at(slot)
            && let Some(st) = s.slots[k].read()
        {
            any_virtual = true;
            sampled[slot] = Some((st.buttons & !GUIDE, st.lt, st.rt));
        }
    }
    let mut tracker = KEYSTROKES.lock().unwrap_or_else(|e| e.into_inner());
    for (slot, entry) in sampled.into_iter().enumerate() {
        if let Some((buttons, lt, rt)) = entry {
            tracker.update_slot(slot, buttons, lt, rt);
        } else if user == XUSER_INDEX_ANY || slot == user as usize {
            tracker.prev_buttons[slot] = 0;
            tracker.prev_lt[slot] = false;
            tracker.prev_rt[slot] = false;
            tracker.queue[slot].clear();
        }
    }
    if let Some(ev) = tracker.pop(user) {
        unsafe { out.write_unaligned(ev) };
        ERROR_SUCCESS
    } else if any_virtual {
        ERROR_EMPTY
    } else {
        r
    }
}

fn serve_enable(enable: i32, real_fn: Option<EnableFn>) {
    let on = enable != 0;
    let prev = XINPUT_ENABLED.swap(on, Ordering::AcqRel);
    if prev != on {
        for slot in 0..4 {
            PACKET_BUMP[slot].fetch_add(1, Ordering::Relaxed);
        }
        if !on {
            KEYSTROKES.lock().unwrap_or_else(|e| e.into_inner()).clear_all();
        }
        if mapped_writable() {
            let saved = *LAST_RUMBLE.lock().unwrap_or_else(|e| e.into_inner());
            let cal_rumble = calibration().rumble_enabled;
            for (slot, command) in saved.into_iter().enumerate() {
                if let Some((s, k)) = shared_at(slot) {
                    if !on || !cal_rumble {
                        s.slots[k].set_rumble(0, 0);
                    } else if let Some(cmd) = command.filter(|cmd| cmd.slot == k) {
                        s.slots[k].set_rumble_for(cmd.generation, cmd.motors[0], cmd.motors[1]);
                    }
                }
            }
        }
    }
    if let Some(f) = real_fn {
        unsafe { f(enable) };
    } else if let Some(x) = XINPUT.as_ref()
        && let Some(f) = x.enable
    {
        unsafe { f(enable) };
    }
}

// ---- IAT / GetProcAddress fallback hooks (use primary XInput trampoline)

unsafe extern "system" fn hook_get_state(i: u32, out: *mut Pad) -> u32 {
    serve_get_state(i, out, false, None, false)
}

unsafe extern "system" fn hook_get_state_ex(i: u32, out: *mut Pad) -> u32 {
    serve_get_state(i, out, true, None, false)
}

unsafe extern "system" fn hook_set_state(i: u32, v: *mut [u16; 2]) -> u32 {
    serve_set_state(i, v, None, false)
}

unsafe extern "system" fn hook_get_caps(i: u32, flags: u32, out: *mut [u8; 20]) -> u32 {
    serve_get_caps(i, flags, out, None, false)
}

unsafe extern "system" fn hook_get_battery(i: u32, kind: u8, out: *mut [u8; 2]) -> u32 {
    serve_get_battery(i, kind, out, None, false)
}

unsafe extern "system" fn hook_get_keystroke(user: u32, reserved: u32, out: *mut Keystroke) -> u32 {
    serve_get_keystroke(user, reserved, out, None, false)
}

unsafe extern "system" fn hook_enable(enable: i32) {
    serve_enable(enable, None);
}

// ---- Per-DLL inline detours (preserve each DLL's own trampoline)

#[cfg(windows)]
macro_rules! inline_detours {
    ($($n:literal => $get:ident, $get_ex:ident, $set:ident, $caps:ident, $batt:ident, $key:ident, $en:ident;)*) => {
        $(
            unsafe extern "system" fn $get(i: u32, out: *mut Pad) -> u32 {
                let real = ORIG_GET[$n].load(Ordering::Acquire);
                let f = (real != 0).then(|| unsafe { std::mem::transmute::<usize, GetState>(real) });
                serve_get_state(i, out, false, f, caller_is_sdl())
            }
            unsafe extern "system" fn $get_ex(i: u32, out: *mut Pad) -> u32 {
                let real = ORIG_GET_EX[$n].load(Ordering::Acquire);
                let f = (real != 0).then(|| unsafe { std::mem::transmute::<usize, GetState>(real) });
                serve_get_state(i, out, true, f, caller_is_sdl())
            }
            unsafe extern "system" fn $set(i: u32, v: *mut [u16; 2]) -> u32 {
                let real = ORIG_SET[$n].load(Ordering::Acquire);
                let f = (real != 0).then(|| unsafe { std::mem::transmute::<usize, SetState>(real) });
                serve_set_state(i, v, f, caller_is_sdl())
            }
            unsafe extern "system" fn $caps(i: u32, flags: u32, out: *mut [u8; 20]) -> u32 {
                let real = ORIG_CAPS[$n].load(Ordering::Acquire);
                let f = (real != 0).then(|| unsafe { std::mem::transmute::<usize, GetCaps>(real) });
                serve_get_caps(i, flags, out, f, caller_is_sdl())
            }
            unsafe extern "system" fn $batt(i: u32, kind: u8, out: *mut [u8; 2]) -> u32 {
                let real = ORIG_BATTERY[$n].load(Ordering::Acquire);
                let f = (real != 0).then(|| unsafe { std::mem::transmute::<usize, GetBattery>(real) });
                serve_get_battery(i, kind, out, f, caller_is_sdl())
            }
            unsafe extern "system" fn $key(user: u32, reserved: u32, out: *mut Keystroke) -> u32 {
                let real = ORIG_KEYSTROKE[$n].load(Ordering::Acquire);
                let f = (real != 0).then(|| unsafe { std::mem::transmute::<usize, GetKeystroke>(real) });
                serve_get_keystroke(user, reserved, out, f, caller_is_sdl())
            }
            unsafe extern "system" fn $en(enable: i32) {
                let real = ORIG_ENABLE[$n].load(Ordering::Acquire);
                let f = (real != 0).then(|| unsafe { std::mem::transmute::<usize, EnableFn>(real) });
                serve_enable(enable, f);
            }
        )*
        fn inline_detour_fns() -> [(usize, usize, usize, usize, usize, usize, usize); 6] {
            [$((
                $get as *const () as usize,
                $get_ex as *const () as usize,
                $set as *const () as usize,
                $caps as *const () as usize,
                $batt as *const () as usize,
                $key as *const () as usize,
                $en as *const () as usize,
            )),*]
        }
    };
}

#[cfg(windows)]
inline_detours! {
    0 => inl_get0, inl_get_ex0, inl_set0, inl_caps0, inl_batt0, inl_key0, inl_en0;
    1 => inl_get1, inl_get_ex1, inl_set1, inl_caps1, inl_batt1, inl_key1, inl_en1;
    2 => inl_get2, inl_get_ex2, inl_set2, inl_caps2, inl_batt2, inl_key2, inl_en2;
    3 => inl_get3, inl_get_ex3, inl_set3, inl_caps3, inl_batt3, inl_key3, inl_en3;
    4 => inl_get4, inl_get_ex4, inl_set4, inl_caps4, inl_batt4, inl_key4, inl_en4;
    5 => inl_get5, inl_get_ex5, inl_set5, inl_caps5, inl_batt5, inl_key5, inl_en5;
}

/// Our function for an XInput export (by name, or ordinal 100).
fn ours_for(name: *const c_char) -> Option<usize> {
    if (name as usize) < 0x10000 {
        return (name as usize == 100).then_some(hook_get_state_ex as *const () as usize);
    }
    Some(match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"XInputGetState" => hook_get_state as *const () as usize,
        b"XInputSetState" => hook_set_state as *const () as usize,
        b"XInputGetCapabilities" => hook_get_caps as *const () as usize,
        b"XInputGetBatteryInformation" => hook_get_battery as *const () as usize,
        b"XInputGetKeystroke" => hook_get_keystroke as *const () as usize,
        b"XInputEnable" => hook_enable as *const () as usize,
        _ => return None,
    })
}

/// A module's full path, lowercase.
#[cfg(windows)]
fn module_path(m: *mut c_void) -> String {
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(m, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())]).to_ascii_lowercase()
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or_default()
}

static REAL_GET_PROC: AtomicUsize = AtomicUsize::new(0);

/// GetProcAddress for the game's modules: if an XInput export was already
/// inline-detoured, return `p` verbatim so callers keep per-DLL trampolines
/// and the SDL double-input guard; otherwise return our fallback wrapper.
#[cfg(windows)]
unsafe extern "system" fn hook_get_proc(m: *mut c_void, name: *const c_char) -> *mut c_void {
    let real: GetProc = unsafe { std::mem::transmute(REAL_GET_PROC.load(Ordering::Acquire)) };
    let p = unsafe { real(m, name) };
    if p.is_null() || name.is_null() {
        return p;
    }
    let maybe = (name as usize) < 0x10000 || unsafe { CStr::from_ptr(name) }.to_bytes().starts_with(b"XInput");
    if maybe && file_name(&module_path(m)).starts_with("xinput") {
        let already_inline = INLINE_HOOKED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&(p as usize));
        if already_inline {
            return p;
        }
        if let Some(ours) = ours_for(name) {
            return ours as *mut c_void;
        }
    }
    p
}

#[cfg(windows)]
static INLINE_HOOKED: LazyLock<Mutex<std::collections::HashMap<usize, usize>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Pin a loaded module so its code pages stay mapped for the lifetime of our
/// inline trampolines.
#[cfg(windows)]
fn pin_module_at(addr: *const c_void) {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW};
    let mut h = HMODULE::default();
    unsafe {
        let _ = GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, windows::core::PCWSTR(addr as *const u16), &mut h);
    }
}

/// Install inline detours on every loaded XInput DLL. Deduplicates across
/// forwarding hops (like `xinputuap.dll` -> `xinput1_4.dll`) BEFORE following
/// any already-installed Aurora detour, so forwarders never double-hook.
#[cfg(windows)]
fn hook_loaded_inline() -> usize {
    let mut hooked_map = INLINE_HOOKED.lock().unwrap_or_else(|e| e.into_inner());
    let fns = inline_detour_fns();
    let mut newly = 0usize;
    for (idx, dll) in XINPUT_DLLS.iter().enumerate() {
        let m = unsafe { GetModuleHandleW(wide(dll).as_ptr()) };
        if m.is_null() {
            continue;
        }
        let (d_get, d_get_ex, d_set, d_caps, d_batt, d_key, d_en) = fns[idx];
        let entries: [(*const c_char, usize, &AtomicUsize, &str); 7] = [
            (c"XInputGetState".as_ptr(), d_get, &ORIG_GET[idx], "XInputGetState"),
            (100 as *const c_char, d_get_ex, &ORIG_GET_EX[idx], "XInputGetStateEx"),
            (c"XInputSetState".as_ptr(), d_set, &ORIG_SET[idx], "XInputSetState"),
            (c"XInputGetCapabilities".as_ptr(), d_caps, &ORIG_CAPS[idx], "XInputGetCapabilities"),
            (c"XInputGetBatteryInformation".as_ptr(), d_batt, &ORIG_BATTERY[idx], "XInputGetBatteryInformation"),
            (c"XInputGetKeystroke".as_ptr(), d_key, &ORIG_KEYSTROKE[idx], "XInputGetKeystroke"),
            (c"XInputEnable".as_ptr(), d_en, &ORIG_ENABLE[idx], "XInputEnable"),
        ];
        for (name, detour, slot, label) in entries {
            if slot.load(Ordering::Acquire) != 0 {
                continue;
            }
            let p = unsafe { GetProcAddress(m, name.cast()) };
            if p.is_null() {
                continue;
            }
            let (resolved, hops) = unsafe { crate::overlay::hook::resolve_until(p, |addr| hooked_map.contains_key(&addr)) };
            if resolved == 0 {
                continue;
            }
            if let Some(&existing_tramp) = hooked_map.get(&resolved) {
                for hop in hops {
                    hooked_map.insert(hop, existing_tramp);
                }
                slot.store(existing_tramp, Ordering::Release);
                continue;
            }
            pin_module_at(resolved as *const c_void);
            let mut tramp = 0usize;
            if unsafe {
                crate::overlay::hook::install(resolved as *const c_void, detour, |t| {
                    tramp = t;
                    slot.store(t, Ordering::Release);
                })
            } {
                for hop in hops {
                    hooked_map.insert(hop, tramp);
                }
                hooked_map.insert(resolved, tramp);
                newly += 1;
                crate::log!("pads: inline-hooked {dll}!{label}");
            }
        }
    }
    newly
}

#[cfg(windows)]
fn pe_image_size(base: *mut u8) -> usize {
    if base.is_null() {
        return 0;
    }
    unsafe {
        if (base as *const u16).read_unaligned() != 0x5A4D {
            return 0;
        }
        let nt = (base.add(0x3C) as *const u32).read_unaligned() as usize;
        if nt == 0 || nt > 0x10_0000 || (base.add(nt) as *const u32).read_unaligned() != 0x4550 {
            return 0;
        }
        let opt = nt + 24;
        (base.add(opt + 56) as *const u32).read_unaligned() as usize
    }
}

/// (real address, ours) for every loaded XInput DLL that wasn't inline-hooked,
/// plus `GetProcAddress` for late-loaded XInput DLLs.
#[cfg(windows)]
fn import_table() -> Vec<(usize, usize)> {
    let mut table = Vec::new();
    for (idx, dll) in XINPUT_DLLS.iter().enumerate() {
        let m = unsafe { GetModuleHandleW(wide(dll).as_ptr()) };
        if m.is_null() {
            continue;
        }
        let exports: [(*const c_char, &AtomicUsize); 7] = [
            (c"XInputGetState".as_ptr(), &ORIG_GET[idx]),
            (100 as *const c_char, &ORIG_GET_EX[idx]),
            (c"XInputSetState".as_ptr(), &ORIG_SET[idx]),
            (c"XInputGetCapabilities".as_ptr(), &ORIG_CAPS[idx]),
            (c"XInputGetBatteryInformation".as_ptr(), &ORIG_BATTERY[idx]),
            (c"XInputGetKeystroke".as_ptr(), &ORIG_KEYSTROKE[idx]),
            (c"XInputEnable".as_ptr(), &ORIG_ENABLE[idx]),
        ];
        for (name, inline_slot) in exports {
            // If already inline-detoured, IAT calls naturally hit the inline detour
            // (preserving per-DLL trampoline + SDL caller check). Only patch IAT for
            // exports that couldn't be inline-hooked.
            if inline_slot.load(Ordering::Acquire) != 0 {
                continue;
            }
            let p = unsafe { GetProcAddress(m, name.cast()) };
            if let (false, Some(ours)) = (p.is_null(), ours_for(name)) {
                table.push((p as usize, ours));
            }
        }
    }
    for dll in ["kernel32.dll", "kernelbase.dll"] {
        let m = unsafe { GetModuleHandleW(wide(dll).as_ptr()) };
        let p = if m.is_null() { std::ptr::null_mut() } else { unsafe { GetProcAddress(m, c"GetProcAddress".as_ptr().cast()) } };
        if !p.is_null() {
            let _ = REAL_GET_PROC.compare_exchange(0, p as usize, Ordering::AcqRel, Ordering::Acquire);
            table.push((p as usize, hook_get_proc as *const () as usize));
        }
    }
    table
}

/// Modules whose XInput stays real: Windows' own, any XInput DLL, and SDL
/// (which reads PlayStation/Switch pads itself).
///
/// .NET is the exception among Windows' modules: XNA/FNA games (Celeste)
/// read XInput through `[DllImport("xinput1_3.dll")]`, which the runtime
/// (`clr.dll`, `coreclr.dll`) resolves, and XNA itself lives in the GAC under
/// `%SystemRoot%\Microsoft.NET\assembly\`. Skipping them left those games
/// on the real XInput, which doesn't see PlayStation pads. `path` is
/// lowercase (see `module_path`).
fn leave_alone(path: &str, windir: &str) -> bool {
    let name = file_name(path);
    let managed = path.contains("\\microsoft.net\\assembly\\") || name == "clr.dll" || name == "coreclr.dll";
    let system = !windir.is_empty() && path.starts_with(windir) && !managed;
    system || name.starts_with("xinput") || name.starts_with("sdl")
}

#[cfg(not(windows))]
fn patch_all() -> usize {
    0
}

#[cfg(windows)]
fn patch_all() -> usize {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW};
    use windows::Win32::System::ProcessStatus::EnumProcessModules;
    use windows::Win32::System::Threading::GetCurrentProcess;

    // 1. Inline-hook every currently loaded XInput DLL first (catches early-cached
    //    GetProcAddress pointers, delay-loads, and engine wrappers).
    let mut patched = hook_loaded_inline();

    // 2. Scan modules: track loaded SDL DLL address ranges (for double-input guard)
    //    and patch ordinary + delay-load IATs for any non-inline-hooked exports.
    let table = import_table();
    let mut me = HMODULE::default();
    let mut modules = vec![HMODULE::default(); 1024];
    let mut needed = 0u32;
    unsafe {
        let _ = GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, windows::core::PCWSTR(patch_all as *const u16), &mut me);
        if EnumProcessModules(GetCurrentProcess(), modules.as_mut_ptr(), (modules.len() * size_of::<HMODULE>()) as u32, &mut needed).is_err() {
            return patched;
        }
    }
    modules.truncate((needed as usize / size_of::<HMODULE>()).min(modules.len()));
    let windir = std::env::var("SystemRoot").map(|w| format!("{}\\", w.to_ascii_lowercase())).unwrap_or_default();
    let mut sdl_ranges = Vec::new();
    for m in modules {
        // Retain a temporary refcount on `m` so the module cannot unload while
        // we inspect its path and PE headers.
        let mut retained = HMODULE::default();
        if unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, windows::core::PCWSTR(m.0 as *const u16), &mut retained) }.is_err() {
            continue;
        }
        let path = module_path(retained.0);
        let fname = file_name(&path);
        if fname.starts_with("sdl") && fname.ends_with(".dll") {
            let size = pe_image_size(retained.0 as *mut u8);
            if size > 0 {
                let lo = retained.0 as usize;
                sdl_ranges.push((lo, lo.saturating_add(size)));
            }
        }
        if retained != me && !leave_alone(&path, &windir) && !table.is_empty() && REAL_GET_PROC.load(Ordering::Acquire) != 0 {
            patched += unsafe { crate::overlay::patch_module(retained.0 as *mut u8, &table) };
        }
        unsafe {
            let _ = windows::Win32::Foundation::FreeLibrary(retained);
        }
    }
    HAS_SDL_MODULES.store(!sdl_ranges.is_empty(), Ordering::Release);
    *SDL_RANGES.lock().unwrap_or_else(|e| e.into_inner()) = sdl_ranges;
    patched
}

/// Enable the launch layout worker and the game's XInput bridge (including under Proton).
#[cfg(windows)]
pub fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if !steam_input_enabled() {
            return;
        }
        let has_ipc = crate::ipc::target().is_some();
        if has_ipc {
            crate::input::load();
            crate::synthetic_input::start();
        }
        if (!has_ipc && std::env::var_os(pads_shm::ENV_PADS).is_none()) || XINPUT.is_none() {
            return;
        }
        // Attempt initial SHM connection & hook pass synchronously so the very
        // next XInput call after SteamAPI_Init() sees shared controllers.
        let _ = mapped();
        let initial = std::panic::catch_unwind(patch_all).unwrap_or(0);
        if initial > 0 {
            crate::log!("pads: {initial} XInput hook(s) active immediately");
        }
        let _ = std::thread::Builder::new().name("pads-imports".into()).spawn(|| {
            // Fast scan during game startup for late-loaded engine plugins/XInput DLLs,
            // then settle into a low-overhead 2-second cadence.
            for tick in 0u64.. {
                std::thread::sleep(Duration::from_millis(if tick < 40 { 250 } else { 2000 }));
                let n = std::panic::catch_unwind(patch_all).unwrap_or(0);
                if n > 0 {
                    crate::log!("pads: {n} additional XInput hook(s) installed");
                }
            }
        });
    });
}

#[cfg(not(windows))]
pub fn install() {
    crate::input::load();
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn hooks_dotnet_but_not_windows() {
        let win = "c:\\windows\\";
        // Windows' own modules and XInput itself stay real.
        assert!(leave_alone("c:\\windows\\system32\\user32.dll", win));
        assert!(leave_alone("c:\\windows\\system32\\xinput1_4.dll", win));
        assert!(leave_alone("c:\\games\\foo\\sdl3.dll", win));
        // The .NET runtime and the GAC's XNA are hooked (Celeste).
        assert!(!leave_alone("c:\\windows\\microsoft.net\\framework\\v4.0.30319\\clr.dll", win));
        assert!(!leave_alone("c:\\windows\\microsoft.net\\assembly\\gac_32\\microsoft.xna.framework\\v4.0_4.0.0.0__842cf8be1de50553\\microsoft.xna.framework.dll", win));
        assert!(!leave_alone("c:\\program files\\dotnet\\shared\\microsoft.netcore.app\\8.0.0\\coreclr.dll", win));
        // The game's own modules are hooked.
        assert!(!leave_alone("c:\\games\\celeste\\celeste.exe", win));
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(file: isize, attrs: *const c_void, protect: u32, size_hi: u32, size_lo: u32, name: *const u16) -> *mut c_void;
    }

    /// A pad the app shares reaches Steam Input and the game's XInput, and
    /// rumble goes back.
    #[test]
    fn shared_pad_reaches_the_game() {
        if XINPUT.is_none() {
            eprintln!("no XInput DLL on this system: skipped");
            return;
        }
        let name = format!("Local\\sir-pads-test-{}", std::process::id());
        let h = unsafe { CreateFileMappingW(-1, std::ptr::null(), 0x4, 0, std::mem::size_of::<Shared>() as u32, wide(&name).as_ptr()) };
        assert!(!h.is_null());
        unsafe { std::env::set_var(pads_shm::ENV_PADS, &name) };
        let s = mapped().expect("the shared memory opens");
        assert!(shared().is_none(), "not live before the app writes");
        s.magic.store(pads_shm::MAGIC, Ordering::Release);
        s.heartbeat.store(pads_shm::now_ms(), Ordering::Relaxed);
        s.slots[0].write(Some(State { packet: 3, buttons: 0x1000 | GUIDE, lt: 200, ly: 32767, ..Default::default() }));

        let pads = all();
        let i = pads_shm::seats(real_connected(), &[0]).iter().position(Option::is_some).expect("a free slot");
        assert_eq!(pads[i].map(|p| (p.buttons, p.lt, p.ly)), Some((0x1000 | GUIDE, 200, 32767)));

        let mut p = Pad::default();
        assert_eq!(unsafe { hook_get_state(i as u32, &mut p) }, 0);
        assert_eq!((p.packet, p.buttons, p.lt), (3, 0x1000, 200), "no guide button without the Ex call");
        assert_eq!(unsafe { hook_get_state_ex(i as u32, &mut p) }, 0);
        assert_eq!(p.buttons, 0x1000 | GUIDE);

        assert_eq!(unsafe { hook_set_state(i as u32, &mut [100, 200]) }, 0);
        assert_eq!(s.slots[0].rumble(), (100, 200));
        rumble(i, 7, 8);
        assert_eq!(s.slots[0].rumble(), (7, 8));

        let mut caps = [0u8; 20];
        assert_eq!(unsafe { hook_get_caps(i as u32, 0, &mut caps) }, 0);
        assert_eq!(caps[0], 1);
        let mut battery = [0u8; 2];
        assert_eq!(unsafe { hook_get_battery(i as u32, 0, &mut battery) }, 0);

        // XInputEnable(FALSE) neutralizes synthetic pad state.
        unsafe { hook_enable(0) };
        assert_eq!(unsafe { hook_get_state(i as u32, &mut p) }, 0);
        assert_eq!(p.buttons, 0);
        unsafe { hook_enable(1) };

        // GetProcAddress on XInput hands out ours (before inline hooking); other lookups are untouched.
        assert!(XINPUT.is_some());
        let _ = import_table();
        let x = unsafe { GetModuleHandleW(wide("xinput1_4.dll").as_ptr()) };
        assert!(!x.is_null());
        assert_eq!(unsafe { hook_get_proc(x, c"XInputGetState".as_ptr()) } as usize, hook_get_state as *const () as usize);
        assert_eq!(unsafe { hook_get_proc(x, 100 as *const c_char) } as usize, hook_get_state_ex as *const () as usize);
        let k = unsafe { GetModuleHandleW(wide("kernel32.dll").as_ptr()) };
        assert_eq!(unsafe { hook_get_proc(k, c"GetTickCount".as_ptr()) }, unsafe { GetProcAddress(k, c"GetTickCount".as_ptr().cast()) });
        let _ = patch_all();

        // The app gone: the pad goes with it.
        s.heartbeat.store(0, Ordering::Relaxed);
        let mut p = Pad::default();
        assert_ne!(unsafe { hook_get_state(i as u32, &mut p) }, 0);
    }

    #[test]
    fn open_shm_rejects_non_sir_pads_names() {
        let rogue = format!("Local\\evil-pads-{}", std::process::id());
        let h = unsafe { CreateFileMappingW(-1, std::ptr::null(), 0x4, 0, std::mem::size_of::<Shared>() as u32, wide(&rogue).as_ptr()) };
        assert!(!h.is_null());
        assert!(open_shm(&rogue).is_none(), "must reject mapping names outside Local\\sir-pads-");
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn steam_input_pads_respects_enabled_and_family_mask() {
        let shm: Shared = unsafe { std::mem::zeroed() };
        shm.magic.store(pads_shm::MAGIC, Ordering::Release);
        shm.heartbeat.store(pads_shm::now_ms(), Ordering::Relaxed);
        shm.slots[0].write(Some(State {
            buttons: 0x1000,
            calibrated: true,
            family: pads_shm::SLOT_FAMILY_PLAYSTATION,
            ..Default::default()
        }));
        shm.slots[1].write(Some(State {
            buttons: 0x2000,
            calibrated: true,
            family: pads_shm::SLOT_FAMILY_SWITCH_PRO,
            ..Default::default()
        }));
        let real = [Some(Pad { buttons: 0x4000, ..Default::default() }), None, None, None];

        assert_eq!(steam_input_pads_with(false, pads_shm::FAMILY_ALL, real, Some(&shm)), [None; 4]);

        let all = steam_input_pads_with(true, pads_shm::FAMILY_ALL, real, Some(&shm));
        assert_eq!(all[0].map(|p| p.pad.buttons), Some(0x4000));
        assert_eq!(all[1].map(|p| (p.pad.buttons, p.family)), Some((0x1000, pads_shm::SLOT_FAMILY_PLAYSTATION)));
        assert_eq!(all[2].map(|p| (p.pad.buttons, p.family)), Some((0x2000, pads_shm::SLOT_FAMILY_SWITCH_PRO)));

        let no_xbox = steam_input_pads_with(true, pads_shm::FAMILY_PLAYSTATION | pads_shm::FAMILY_SWITCH_PRO, real, Some(&shm));
        assert!(no_xbox[0].is_none());
        assert_eq!(no_xbox[1].map(|p| p.pad.buttons), Some(0x1000));
        assert_eq!(no_xbox[2].map(|p| p.pad.buttons), Some(0x2000));

        let only_switch = steam_input_pads_with(true, pads_shm::FAMILY_SWITCH_PRO, real, Some(&shm));
        assert!(only_switch[0].is_none());
        assert_eq!(only_switch[1].map(|p| (p.pad.buttons, p.family)), Some((0x2000, pads_shm::SLOT_FAMILY_SWITCH_PRO)));
        assert!(only_switch[2].is_none());
    }

    #[test]
    fn keystroke_tracker_synthesizes_press_and_release() {
        let mut t = KeystrokeTracker::default();
        t.update_slot(0, 0x1000, 128, 0); // A + LT
        let k1 = t.pop(0).expect("A keydown");
        assert_eq!((k1.virtual_key, k1.flags, k1.user_index), (0x5800, KEYSTROKE_KEYDOWN, 0));
        let k2 = t.pop(XUSER_INDEX_ANY).expect("LT keydown");
        assert_eq!((k2.virtual_key, k2.flags, k2.user_index), (VK_PAD_LTRIGGER, KEYSTROKE_KEYDOWN, 0));
        assert!(t.pop(0).is_none());

        t.update_slot(0, 0, 0, 0);
        let k3 = t.pop(0).expect("A keyup");
        assert_eq!((k3.virtual_key, k3.flags), (0x5800, KEYSTROKE_KEYUP));
        let k4 = t.pop(0).expect("LT keyup");
        assert_eq!((k4.virtual_key, k4.flags), (VK_PAD_LTRIGGER, KEYSTROKE_KEYUP));
    }

    #[test]
    fn xinput_dll_list_includes_xinputuap() {
        assert!(XINPUT_DLLS.contains(&"xinputuap.dll"));
        assert!(ours_for(c"XInputGetKeystroke".as_ptr()).is_some());
        assert!(ours_for(c"XInputEnable".as_ptr()).is_some());
    }
}
