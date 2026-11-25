//! Cross-platform controller management, detection, live input testing,
//! calibration, and Steam Input override resolution.
//!
//! - **Windows**: reads non-Xbox controllers (PlayStation, Switch, Steam
//!   Controller, generic DirectInput pads) via dynamically loaded `SDL3.dll`
//!   while a game with our `steamclient` runs (publishing enabled families
//!   to shared memory `Local\sir-pads-*` with deadzones/calibration applied),
//!   and enumerates/tests all controllers (including Xbox via SDL3 or XInput)
//!   when Settings → Controller is open.
//! - **Linux**: enumerates/tests controllers via dynamically loaded
//!   `libSDL3.so.0` when installed, falling back cleanly to kernel
//!   `/sys/class/input` + `/dev/input/event*` / `/dev/input/js*` when SDL3
//!   is unavailable.

#[path = "../../steamclient/src/pads_shm.rs"]
#[allow(dead_code)]
mod shm;

pub use shm::{
    ENV_INPUT_CALIBRATION, ENV_INPUT_FAMILIES, ENV_PADS, ENV_STEAM_INPUT, FAMILY_GENERIC,
    FAMILY_PLAYSTATION, FAMILY_SWITCH_PRO, FAMILY_XBOX, PadCalibration,
};

use model::AppId;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ControllerFamily {
    Xbox,
    PlayStation,
    SwitchPro,
    Generic,
}

impl ControllerFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::Xbox => "Xbox",
            Self::PlayStation => "PlayStation",
            Self::SwitchPro => "Switch Pro",
            Self::Generic => "Generic",
        }
    }

    pub fn slot_family(self) -> u8 {
        match self {
            Self::Xbox => shm::SLOT_FAMILY_XBOX,
            Self::PlayStation => shm::SLOT_FAMILY_PLAYSTATION,
            Self::SwitchPro => shm::SLOT_FAMILY_SWITCH_PRO,
            Self::Generic => shm::SLOT_FAMILY_GENERIC,
        }
    }

    pub fn bit(self) -> u32 {
        match self {
            Self::Xbox => FAMILY_XBOX,
            Self::PlayStation => FAMILY_PLAYSTATION,
            Self::SwitchPro => FAMILY_SWITCH_PRO,
            Self::Generic => FAMILY_GENERIC,
        }
    }

    /// Classify a controller from its SDL3 `SDL_GamepadType` (if known), USB/BT
    /// vendor ID, and device name.
    pub fn classify(sdl_type: i32, vendor: u16, name: &str) -> Self {
        match sdl_type {
            2 | 3 => return Self::Xbox,           // XBOX360 / XBOXONE
            4 | 5 | 6 => return Self::PlayStation, // PS3 / PS4 / PS5
            7..=10 => return Self::SwitchPro,      // SWITCH_PRO / JOYCON_*
            _ => {}
        }
        match vendor {
            0x045e => return Self::Xbox,        // Microsoft
            0x054c => return Self::PlayStation, // Sony
            0x057e => return Self::SwitchPro,   // Nintendo
            _ => {}
        }
        let lower = name.to_ascii_lowercase();
        if lower.contains("xbox") || lower.contains("x-box") || lower.contains("xinput") {
            Self::Xbox
        } else if lower.contains("dualsense")
            || lower.contains("dualshock")
            || lower.contains("playstation")
            || lower.contains("sony")
            || lower.contains("ps3")
            || lower.contains("ps4")
            || lower.contains("ps5")
        {
            Self::PlayStation
        } else if lower.contains("nintendo")
            || lower.contains("switch")
            || lower.contains("joy-con")
            || lower.contains("pro controller")
        {
            Self::SwitchPro
        } else {
            Self::Generic
        }
    }
}

fn yes() -> bool {
    true
}

/// Global Steam Input enable toggles per controller family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FamilyToggles {
    #[serde(default = "yes")]
    pub xbox: bool,
    #[serde(default = "yes")]
    pub playstation: bool,
    #[serde(default = "yes")]
    pub switch_pro: bool,
    #[serde(default = "yes")]
    pub generic: bool,
}

impl Default for FamilyToggles {
    fn default() -> Self {
        Self { xbox: true, playstation: true, switch_pro: true, generic: true }
    }
}

impl FamilyToggles {
    pub fn is_enabled(&self, family: ControllerFamily) -> bool {
        match family {
            ControllerFamily::Xbox => self.xbox,
            ControllerFamily::PlayStation => self.playstation,
            ControllerFamily::SwitchPro => self.switch_pro,
            ControllerFamily::Generic => self.generic,
        }
    }

    pub fn bitmask(&self) -> u32 {
        let mut mask = 0u32;
        if self.xbox {
            mask |= FAMILY_XBOX;
        }
        if self.playstation {
            mask |= FAMILY_PLAYSTATION;
        }
        if self.switch_pro {
            mask |= FAMILY_SWITCH_PRO;
        }
        if self.generic {
            mask |= FAMILY_GENERIC;
        }
        mask
    }

    pub fn any_enabled(&self) -> bool {
        self.bitmask() != 0
    }

    pub fn any_non_xbox(&self) -> bool {
        self.playstation || self.switch_pro || self.generic
    }
}

/// Persisted global controller settings (Steam Input family toggles + calibration).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ControllerSettings {
    pub families: FamilyToggles,
    pub calibration: PadCalibration,
}

/// Per-launch resolution of `GameSettings.controller` (`0` default, `1` enable, `2` disable)
/// combined with global controller family toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamInputDecision {
    pub enabled: bool,
    pub families_mask: u32,
    pub use_shm_bridge: bool,
}

/// Resolve whether Aurora's Steam Input and XInput SHM bridge are active for a launch.
///
/// - `2` ("Disable Steam Input"): disables Steam Input and XInput bridging unconditionally.
/// - `0` ("Use default settings") or `1` ("Enable Steam Input"): honors global controller family toggles.
pub fn resolve_steam_input(app_override: u8, global: &FamilyToggles) -> SteamInputDecision {
    if app_override == 2 {
        return SteamInputDecision { enabled: false, families_mask: 0, use_shm_bridge: false };
    }
    let mask = global.bitmask();
    SteamInputDecision {
        enabled: mask != 0,
        families_mask: mask,
        use_shm_bridge: mask != 0,
    }
}

/// Decide whether an SDL-detected controller should be published into Windows
/// shared memory. Non-Xbox families are always published when enabled; Xbox-
/// classified controllers backed by native XInput (no HID path) are skipped up
/// to the number of connected native XInput slots, while third-party HID/BT
/// controllers that SDL classifies as Xbox (but native XInput misses) still
/// reach the game without duplicating genuine XInput pads.
#[cfg(any(windows, test))]
pub(crate) fn should_publish_to_shm(
    family: ControllerFamily,
    has_hid_path: bool,
    families: &FamilyToggles,
    real_xinput_count: usize,
    xbox_skipped: &mut usize,
) -> bool {
    if !families.is_enabled(family) {
        return false;
    }
    if family == ControllerFamily::Xbox && !has_hid_path && *xbox_skipped < real_xinput_count {
        *xbox_skipped += 1;
        return false;
    }
    true
}

static SETTINGS: OnceLock<Mutex<ControllerSettings>> = OnceLock::new();
static APP_CONTROLLERS: OnceLock<Mutex<BTreeMap<AppId, u8>>> = OnceLock::new();

fn settings_lock() -> &'static Mutex<ControllerSettings> {
    SETTINGS.get_or_init(|| Mutex::new(ControllerSettings::default()))
}

fn app_controllers_lock() -> &'static Mutex<BTreeMap<AppId, u8>> {
    APP_CONTROLLERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn settings() -> ControllerSettings {
    settings_lock().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn set_settings(mut next: ControllerSettings) {
    next.calibration = next.calibration.clamped();
    *settings_lock().lock().unwrap_or_else(|e| e.into_inner()) = next;
    crate::preferences_changed();
}

pub fn set_app_controllers(map: BTreeMap<AppId, u8>) {
    let mut guard = app_controllers_lock().lock().unwrap_or_else(|e| e.into_inner());
    *guard = map.into_iter().filter(|(_, v)| *v != 0).collect();
}

pub fn set_app_controller(app_id: AppId, mode: u8) {
    let mut guard = app_controllers_lock().lock().unwrap_or_else(|e| e.into_inner());
    if mode == 0 {
        guard.remove(&app_id);
    } else {
        guard.insert(app_id, mode);
    }
}

pub fn app_controller(app_id: AppId) -> u8 {
    app_controllers_lock().lock().unwrap_or_else(|e| e.into_inner()).get(&app_id).copied().unwrap_or(0)
}

/// Effective per-game override: an explicit non-default launch argument wins,
/// otherwise falls back to the stored per-game override.
pub fn effective_controller_override(app_id: AppId, launch_arg: u8) -> u8 {
    if launch_arg != 0 { launch_arg } else { app_controller(app_id) }
}

// ------------------------------------------------------------ UI inspection types

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Unknown,
    Wired,
    Wireless,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveInput {
    /// XInput button bitmask (plus `shm::GUIDE` = `0x0400`).
    pub buttons: u16,
    pub misc: bool,
    pub touchpad: bool,
    /// `[R1, L1, R2, L2]` back paddles.
    pub paddles: [bool; 4],
    /// Calibrated normalized sticks in `[-1.0, 1.0]` (X right +, Y up +).
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    /// Raw sticks before deadzone.
    pub raw_lx: f32,
    pub raw_ly: f32,
    pub raw_rx: f32,
    pub raw_ry: f32,
    /// Calibrated triggers in `[0.0, 1.0]`.
    pub lt: f32,
    pub rt: f32,
    pub raw_lt: f32,
    pub raw_rt: f32,
    pub has_motion: bool,
    pub accel: [f32; 3],
    pub gyro: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct DetectedController {
    pub id: String,
    pub name: String,
    pub family: ControllerFamily,
    pub vendor_id: u16,
    pub product_id: u16,
    pub connection: ConnectionKind,
    pub slot: Option<usize>,
    pub battery_percent: Option<u8>,
    pub backend: &'static str,
    pub has_rumble: bool,
    pub has_gyro: bool,
    pub live: LiveInput,
}

#[derive(Debug, Clone, Default)]
pub struct DetectionSnapshot {
    pub backend: &'static str,
    pub controllers: Vec<DetectedController>,
    pub note: Option<String>,
}

/// Normalized single-finger touchpad contact for a handheld/controller trackpad (`0.0..=1.0`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TouchpadFinger {
    pub down: bool,
    pub click: bool,
    pub x: f32,
    pub y: f32,
}

/// Normalized gamepad snapshot streamed to Aurora's launcher / Big Picture UI.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LauncherPadSample {
    pub id: u32,
    /// XInput button bitmask (plus `0x0400` for Guide).
    pub buttons: u16,
    /// Dedicated misc / Share / Steam Deck Quick Access (`⋯`) button.
    pub misc: bool,
    /// Left stick in `[-1.0, 1.0]` (`+Y` is up).
    pub lx: f32,
    pub ly: f32,
    /// Right stick in `[-1.0, 1.0]` (`+Y` is up).
    pub rx: f32,
    pub ry: f32,
    /// Triggers in `[0.0, 1.0]`.
    pub lt: f32,
    pub rt: f32,
    /// Up to 2 trackpad contacts (`[left, right]` on dual-pad controllers like Steam Deck; `[0]` on DualSense).
    pub touchpads: [TouchpadFinger; 2],
}

/// One controller state frame emitted by Aurora's unified SDL3 host reader.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LauncherPadFrame {
    pub connected: usize,
    pub pads: Vec<LauncherPadSample>,
}

/// Live inventory of connected controllers used by launch routing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectedPadSummary {
    pub total: usize,
    pub xbox_enabled: usize,
    pub non_xbox_enabled: usize,
    /// Physical `(vendor_id, product_id)` pairs of enabled non-Xbox controllers.
    pub non_xbox_vid_pids: Vec<(u16, u16)>,
    /// SDL3 mapping strings for connected controllers (`SDL_GAMECONTROLLERCONFIG`).
    pub sdl_mappings: Vec<String>,
    pub sdl_active: bool,
}

impl ConnectedPadSummary {
    /// Whether this launch needs Aurora's controller translation (either a
    /// non-Xbox enabled pad is connected, a custom layout is selected, or
    /// Steam Input was explicitly forced on for the game).
    pub fn needs_translation(
        &self,
        app_override: u8,
        families: &FamilyToggles,
        has_custom_binding: bool,
    ) -> bool {
        if !resolve_steam_input(app_override, families).enabled {
            return false;
        }
        app_override == 1 || has_custom_binding || self.non_xbox_enabled > 0
    }
}

pub fn virtual_pad_available() -> bool {
    crate::vpad::is_available()
}

pub fn virtual_pad_backend() -> &'static str {
    crate::vpad::backend_name()
}

pub fn virtual_pad_hint() -> &'static str {
    crate::vpad::unavailable_hint()
}

/// Build a [`LiveInput`] from raw normalized axes + XInput buttons and apply [`PadCalibration`].
#[allow(clippy::too_many_arguments)]
fn build_live_input(
    buttons: u16,
    misc: bool,
    touchpad: bool,
    paddles: [bool; 4],
    raw_lx: f32,
    raw_ly: f32,
    raw_rx: f32,
    raw_ry: f32,
    raw_lt: f32,
    raw_rt: f32,
    accel: [f32; 3],
    gyro: [f32; 3],
    has_motion: bool,
    cal: &PadCalibration,
) -> LiveInput {
    let c = cal.clamped();
    let buttons = if c.nintendo_layout { shm::swap_nintendo_buttons(buttons) } else { buttons };
    let (lx, ly) = shm::apply_stick_f32(raw_lx, raw_ly, c.left_deadzone);
    let (rx, ry) = shm::apply_stick_f32(raw_rx, raw_ry, c.right_deadzone);
    let lt = shm::apply_trigger_f32(raw_lt, c.trigger_threshold);
    let rt = shm::apply_trigger_f32(raw_rt, c.trigger_threshold);
    let (has_motion, accel, gyro) = if c.gyro_enabled && has_motion {
        (true, accel, [gyro[0] * c.gyro_sensitivity, gyro[1] * c.gyro_sensitivity, gyro[2] * c.gyro_sensitivity])
    } else {
        (false, [0.0; 3], [0.0; 3])
    };
    LiveInput {
        buttons,
        misc,
        touchpad,
        paddles,
        lx,
        ly,
        rx,
        ry,
        raw_lx: raw_lx.clamp(-1.0, 1.0),
        raw_ly: raw_ly.clamp(-1.0, 1.0),
        raw_rx: raw_rx.clamp(-1.0, 1.0),
        raw_ry: raw_ry.clamp(-1.0, 1.0),
        lt,
        rt,
        raw_lt: raw_lt.clamp(0.0, 1.0),
        raw_rt: raw_rt.clamp(0.0, 1.0),
        has_motion,
        accel,
        gyro,
    }
}

// ------------------------------------------------------------ Shared SDL3 loader

mod sdl_dyn {
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::OnceLock;

    pub const INIT_GAMEPAD: u32 = 0x2000;
    pub const INIT_HAPTIC: u32 = 0x1000;
    pub const SENSOR_ACCEL: i32 = 1;
    pub const SENSOR_GYRO: i32 = 2;

    pub type Gp = *mut c_void;

    pub struct Sdl {
        pub set_hint: unsafe extern "C" fn(*const c_char, *const c_char) -> bool,
        pub init: unsafe extern "C" fn(u32) -> bool,
        pub init_subsystem: Option<unsafe extern "C" fn(u32) -> bool>,
        pub quit_subsystem: unsafe extern "C" fn(u32),
        pub pump_events: Option<unsafe extern "C" fn()>,
        pub get_error: unsafe extern "C" fn() -> *const c_char,
        pub free: unsafe extern "C" fn(*mut c_void),
        pub set_joystick_events: unsafe extern "C" fn(bool),
        pub set_gamepad_events: unsafe extern "C" fn(bool),
        pub get_gamepads: unsafe extern "C" fn(*mut i32) -> *mut u32,
        pub type_for_id: unsafe extern "C" fn(u32) -> i32,
        pub vendor_for_id: unsafe extern "C" fn(u32) -> u16,
        pub product_for_id: Option<unsafe extern "C" fn(u32) -> u16>,
        pub name_for_id: unsafe extern "C" fn(u32) -> *const c_char,
        pub open: unsafe extern "C" fn(u32) -> Gp,
        pub close: unsafe extern "C" fn(Gp),
        pub connected: unsafe extern "C" fn(Gp) -> bool,
        pub update: unsafe extern "C" fn(),
        pub button: unsafe extern "C" fn(Gp, i32) -> bool,
        pub axis: unsafe extern "C" fn(Gp, i32) -> i16,
        pub rumble: unsafe extern "C" fn(Gp, u16, u16, u32) -> bool,
        pub has_sensor: Option<unsafe extern "C" fn(Gp, i32) -> bool>,
        pub set_sensor_enabled: Option<unsafe extern "C" fn(Gp, i32, bool) -> bool>,
        pub get_sensor_data: Option<unsafe extern "C" fn(Gp, i32, *mut f32, i32) -> bool>,
        pub connection_state: Option<unsafe extern "C" fn(Gp) -> i32>,
        pub power_info: Option<unsafe extern "C" fn(Gp, *mut i32) -> i32>,
        pub get_path: Option<unsafe extern "C" fn(Gp) -> *const c_char>,
        pub add_mappings_from_file: Option<unsafe extern "C" fn(*const c_char) -> i32>,
        pub add_mapping: Option<unsafe extern "C" fn(*const c_char) -> i32>,
        pub get_mapping_for_id: Option<unsafe extern "C" fn(u32) -> *mut c_char>,
        pub get_joysticks: Option<unsafe extern "C" fn(*mut i32) -> *mut u32>,
        pub joystick_name_for_id: Option<unsafe extern "C" fn(u32) -> *const c_char>,
        pub joystick_vendor_for_id: Option<unsafe extern "C" fn(u32) -> u16>,
        pub joystick_product_for_id: Option<unsafe extern "C" fn(u32) -> u16>,
        pub num_touchpads: Option<unsafe extern "C" fn(Gp) -> i32>,
        pub num_touchpad_fingers: Option<unsafe extern "C" fn(Gp, i32) -> i32>,
        pub touchpad_finger: Option<unsafe extern "C" fn(Gp, i32, i32, *mut bool, *mut f32, *mut f32, *mut f32) -> bool>,
    }

    unsafe impl Send for Sdl {}
    unsafe impl Sync for Sdl {}

    #[cfg(windows)]
    fn load_lib() -> Option<*mut c_void> {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        }
        const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;
        let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        let mut candidates = vec![exe_dir.join("SDL3.dll")];
        if cfg!(debug_assertions) {
            let mut cur = exe_dir.as_path();
            for _ in 0..4 {
                candidates.push(cur.join("vendor").join("sdl3").join("SDL3.dll"));
                let Some(parent) = cur.parent() else { break };
                cur = parent;
            }
        }
        let Some(path) = candidates.into_iter().find(|p| p.is_file()) else {
            tracing::warn!("controllers: SDL3.dll not found next to the app; falling back to XInput only");
            return None;
        };
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
        let m = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        if m.is_null() {
            tracing::warn!("controllers: couldn't load {}", path.display());
            return None;
        }
        tracing::info!("controllers: loaded {}", path.display());
        Some(m)
    }

    #[cfg(windows)]
    unsafe fn sym(m: *mut c_void, name: *const c_char) -> *mut c_void {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        unsafe { GetProcAddress(m, name) }
    }

    #[cfg(not(windows))]
    fn load_lib() -> Option<*mut c_void> {
        use std::ffi::CString;
        let mut candidates: Vec<CString> = Vec::new();
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            for name in ["libSDL3.so.0", "libSDL3.so"] {
                let p = dir.join(name);
                if p.is_file()
                    && let Ok(c) = CString::new(p.to_string_lossy().as_bytes())
                {
                    candidates.push(c);
                }
            }
        }
        candidates.push(CString::new("libSDL3.so.0").unwrap());
        candidates.push(CString::new("libSDL3.so").unwrap());
        for c in candidates {
            let m = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
            if !m.is_null() {
                return Some(m);
            }
        }
        tracing::info!("controllers: libSDL3 not found; using Linux sysfs/evdev fallback");
        None
    }

    #[cfg(not(windows))]
    unsafe fn sym(m: *mut c_void, name: *const c_char) -> *mut c_void {
        unsafe { libc::dlsym(m, name) }
    }

    fn load_sdl() -> Option<Sdl> {
        let m = load_lib()?;
        macro_rules! f {
            ($name:literal) => {{
                let p = unsafe { sym(m, concat!($name, "\0").as_ptr().cast()) };
                if p.is_null() {
                    tracing::warn!("controllers: SDL3 library has no {}", $name);
                    return None;
                }
                unsafe { std::mem::transmute::<*mut c_void, _>(p) }
            }};
        }
        macro_rules! opt {
            ($name:literal) => {{
                let p = unsafe { sym(m, concat!($name, "\0").as_ptr().cast()) };
                if p.is_null() { None } else { Some(unsafe { std::mem::transmute::<*mut c_void, _>(p) }) }
            }};
        }
        Some(Sdl {
            set_hint: f!("SDL_SetHint"),
            init: f!("SDL_Init"),
            init_subsystem: opt!("SDL_InitSubSystem"),
            quit_subsystem: f!("SDL_QuitSubSystem"),
            pump_events: opt!("SDL_PumpEvents"),
            get_error: f!("SDL_GetError"),
            free: f!("SDL_free"),
            set_joystick_events: f!("SDL_SetJoystickEventsEnabled"),
            set_gamepad_events: f!("SDL_SetGamepadEventsEnabled"),
            get_gamepads: f!("SDL_GetGamepads"),
            type_for_id: f!("SDL_GetGamepadTypeForID"),
            vendor_for_id: f!("SDL_GetGamepadVendorForID"),
            product_for_id: opt!("SDL_GetGamepadProductForID"),
            name_for_id: f!("SDL_GetGamepadNameForID"),
            open: f!("SDL_OpenGamepad"),
            close: f!("SDL_CloseGamepad"),
            connected: f!("SDL_GamepadConnected"),
            update: f!("SDL_UpdateGamepads"),
            button: f!("SDL_GetGamepadButton"),
            axis: f!("SDL_GetGamepadAxis"),
            rumble: f!("SDL_RumbleGamepad"),
            has_sensor: opt!("SDL_GamepadHasSensor"),
            set_sensor_enabled: opt!("SDL_SetGamepadSensorEnabled"),
            get_sensor_data: opt!("SDL_GetGamepadSensorData"),
            connection_state: opt!("SDL_GetGamepadConnectionState"),
            power_info: opt!("SDL_GetGamepadPowerInfo"),
            get_path: opt!("SDL_GetGamepadPath"),
            add_mappings_from_file: opt!("SDL_AddGamepadMappingsFromFile"),
            add_mapping: opt!("SDL_AddGamepadMapping"),
            get_mapping_for_id: opt!("SDL_GetGamepadMappingForID"),
            get_joysticks: opt!("SDL_GetJoysticks"),
            joystick_name_for_id: opt!("SDL_GetJoystickNameForID"),
            joystick_vendor_for_id: opt!("SDL_GetJoystickVendorForID"),
            joystick_product_for_id: opt!("SDL_GetJoystickProductForID"),
            num_touchpads: opt!("SDL_GetNumGamepadTouchpads"),
            num_touchpad_fingers: opt!("SDL_GetNumGamepadTouchpadFingers"),
            touchpad_finger: opt!("SDL_GetGamepadTouchpadFinger"),
        })
    }

    /// Load optional user-supplied SDL gamepad mappings from
    /// `<config_dir>/gamecontrollerdb.txt`, `SDL_GAMECONTROLLERCONFIG_FILE`,
    /// and `SDL_GAMECONTROLLERCONFIG`.
    pub fn load_user_mappings(sdl: &Sdl) {
        use std::ffi::CString;
        if let Some(add_file) = sdl.add_mappings_from_file {
            let mut paths = vec![crate::store::config_dir().join("gamecontrollerdb.txt")];
            if let Some(env_file) = std::env::var_os("SDL_GAMECONTROLLERCONFIG_FILE") {
                paths.push(std::path::PathBuf::from(env_file));
            }
            for path in paths {
                if !path.is_file() {
                    continue;
                }
                if let Ok(cpath) = CString::new(path.to_string_lossy().as_bytes()) {
                    let added = unsafe { add_file(cpath.as_ptr()) };
                    if added >= 0 {
                        tracing::info!("controllers: loaded {added} SDL mapping(s) from {}", path.display());
                    } else {
                        tracing::warn!("controllers: failed loading mappings from {}: {}", path.display(), cstr_lossy(unsafe { (sdl.get_error)() }));
                    }
                }
            }
        }
        if let Some(add_one) = sdl.add_mapping
            && let Ok(env_cfg) = std::env::var("SDL_GAMECONTROLLERCONFIG")
        {
            let mut count = 0usize;
            for line in env_cfg.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                if let Ok(c) = CString::new(line)
                    && unsafe { add_one(c.as_ptr()) } >= 0
                {
                    count += 1;
                }
            }
            if count > 0 {
                tracing::info!("controllers: loaded {count} SDL mapping(s) from SDL_GAMECONTROLLERCONFIG");
            }
        }
    }

    pub fn mapping_for_id(sdl: &Sdl, id: u32) -> Option<String> {
        let f = sdl.get_mapping_for_id?;
        let p = unsafe { f(id) };
        if p.is_null() {
            return None;
        }
        let s = cstr_lossy(p);
        unsafe { (sdl.free)(p.cast()) };
        (!s.is_empty()).then_some(s)
    }

    /// Log any connected raw SDL joystick that did not match a gamepad mapping
    /// so users/maintainers can see exact VID:PID details in `aurora.log`.
    pub fn log_unmapped_joysticks(sdl: &Sdl, mapped_ids: &[u32], seen: &mut std::collections::HashSet<u32>) {
        let Some(get_joys) = sdl.get_joysticks else { return };
        let mut n = 0;
        let ids = unsafe { get_joys(&mut n) };
        if ids.is_null() {
            return;
        }
        let list: Vec<u32> = unsafe { std::slice::from_raw_parts(ids, n.max(0) as usize) }.to_vec();
        unsafe { (sdl.free)(ids.cast()) };
        seen.retain(|id| list.contains(id));
        for id in list {
            if mapped_ids.contains(&id) || !seen.insert(id) {
                continue;
            }
            let name = sdl.joystick_name_for_id.map(|f| cstr_lossy(unsafe { f(id) })).unwrap_or_default();
            let vid = sdl.joystick_vendor_for_id.map_or(0, |f| unsafe { f(id) });
            let pid = sdl.joystick_product_for_id.map_or(0, |f| unsafe { f(id) });
            tracing::warn!(
                "controllers: unmapped joystick \"{name}\" (VID:PID {vid:04x}:{pid:04x}, id={id}) — add an SDL mapping to {}",
                crate::store::config_dir().join("gamecontrollerdb.txt").display()
            );
        }
    }

    pub fn sdl() -> Option<&'static Sdl> {
        static SDL: OnceLock<Option<Sdl>> = OnceLock::new();
        SDL.get_or_init(load_sdl).as_ref()
    }

    pub fn cstr_lossy(p: *const c_char) -> String {
        if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
    }

    pub const BUTTONS: [(i32, u16); 15] = [
        (0, 0x1000),      // south -> A
        (1, 0x2000),      // east -> B
        (2, 0x4000),      // west -> X
        (3, 0x8000),      // north -> Y
        (4, 0x0020),      // back
        (5, super::shm::GUIDE), // guide
        (6, 0x0010),      // start
        (7, 0x0040),      // left stick
        (8, 0x0080),      // right stick
        (9, 0x0100),      // left shoulder
        (10, 0x0200),     // right shoulder
        (11, 0x0001),     // d-pad up
        (12, 0x0002),     // d-pad down
        (13, 0x0004),     // d-pad left
        (14, 0x0008),     // d-pad right
    ];

    pub fn flip(v: i16) -> i16 {
        (-(v as i32)).clamp(-32768, 32767) as i16
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn trigger_u8(v: i16) -> u8 {
        (v.max(0) as u32 * 255 / 32767) as u8
    }

    pub fn read_raw(sdl: &Sdl, gp: Gp) -> (u16, bool, bool, [bool; 4], [i16; 4], [i16; 2], Option<([f32; 3], [f32; 3])>) {
        let b = |i| unsafe { (sdl.button)(gp, i) };
        let a = |i| unsafe { (sdl.axis)(gp, i) };
        let buttons = BUTTONS.iter().filter(|(i, _)| b(*i)).fold(0, |acc, (_, bit)| acc | bit);
        let misc = b(15);
        let paddles = [b(16), b(17), b(18), b(19)];
        let touchpad = b(20);
        let sticks = [a(0), flip(a(1)), a(2), flip(a(3))];
        let triggers = [a(4), a(5)];
        let motion = sdl.get_sensor_data.and_then(|get_data| {
            let mut accel = [0.0f32; 3];
            let mut gyro = [0.0f32; 3];
            let has_a = unsafe { get_data(gp, SENSOR_ACCEL, accel.as_mut_ptr(), 3) };
            let has_g = unsafe { get_data(gp, SENSOR_GYRO, gyro.as_mut_ptr(), 3) };
            (has_a || has_g).then_some((accel, gyro))
        });
        (buttons, misc, touchpad, paddles, sticks, triggers, motion)
    }

    pub fn read_touchpads(sdl: &Sdl, gp: Gp, touchpad_btn: bool) -> [super::TouchpadFinger; 2] {
        let mut out = [super::TouchpadFinger::default(); 2];
        let (Some(n_pads), Some(n_fingers), Some(get_finger)) =
            (sdl.num_touchpads, sdl.num_touchpad_fingers, sdl.touchpad_finger)
        else {
            return out;
        };
        let count = unsafe { n_pads(gp) }.clamp(0, 2) as usize;
        for (pad_idx, slot) in out.iter_mut().take(count).enumerate() {
            let fingers = unsafe { n_fingers(gp, pad_idx as i32) };
            for f_idx in 0..fingers {
                let (mut down, mut x, mut y, mut pressure) = (false, 0.0f32, 0.0f32, 0.0f32);
                if unsafe { get_finger(gp, pad_idx as i32, f_idx, &mut down, &mut x, &mut y, &mut pressure) } && down {
                    slot.down = true;
                    slot.x = x.clamp(0.0, 1.0);
                    slot.y = y.clamp(0.0, 1.0);
                    break;
                }
            }
        }
        // SDL3's HIDAPI Steam Deck driver maps left pad click to `SDL_GAMEPAD_BUTTON_TOUCHPAD`
        // (index 20) and right pad click to `SDL_GAMEPAD_BUTTON_MISC2` (index 21).
        let right_btn = unsafe { (sdl.button)(gp, 21) };
        out[0].click = touchpad_btn && (out[0].down || !out[1].down || right_btn);
        if count >= 2 {
            out[1].click = right_btn || (touchpad_btn && out[1].down && !out[0].down);
        }
        out
    }

    pub fn enable_sensors(sdl: &Sdl, gp: Gp) -> bool {
        let (Some(has), Some(enable)) = (sdl.has_sensor, sdl.set_sensor_enabled) else {
            return false;
        };
        unsafe {
            let mut any = false;
            if has(gp, SENSOR_ACCEL) {
                any |= enable(gp, SENSOR_ACCEL, true);
            }
            if has(gp, SENSOR_GYRO) {
                any |= enable(gp, SENSOR_GYRO, true);
            }
            any
        }
    }

    pub fn connection_and_battery(sdl: &Sdl, gp: Gp) -> (super::ConnectionKind, Option<u8>) {
        let conn = match sdl.connection_state.map(|f| unsafe { f(gp) }) {
            Some(1) => super::ConnectionKind::Wired,
            Some(2) => super::ConnectionKind::Wireless,
            _ => super::ConnectionKind::Unknown,
        };
        let batt = sdl.power_info.and_then(|f| {
            let mut pct = -1i32;
            let state = unsafe { f(gp, &mut pct) };
            matches!(state, 1 | 3 | 4).then(|| (0..=100).contains(&pct).then_some(pct as u8)).flatten()
        });
        (conn, batt)
    }
}

// ------------------------------------------------------------ Shared UI, launcher & virtual pad state

static SNAPSHOT: OnceLock<Mutex<DetectionSnapshot>> = OnceLock::new();
static SUMMARY: OnceLock<Mutex<ConnectedPadSummary>> = OnceLock::new();
static UI_LAST_POLL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FORCE_RESCAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TEST_RUMBLE_ID: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static LAUNCHER_SUB: OnceLock<Mutex<Option<std::sync::mpsc::Sender<LauncherPadFrame>>>> = OnceLock::new();
static VPAD_USERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static VPAD_INCLUDE_XBOX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn snapshot_lock() -> &'static Mutex<DetectionSnapshot> {
    SNAPSHOT.get_or_init(|| Mutex::new(DetectionSnapshot::default()))
}

fn summary_lock() -> &'static Mutex<ConnectedPadSummary> {
    SUMMARY.get_or_init(|| Mutex::new(ConnectedPadSummary::default()))
}

fn rumble_lock() -> &'static Mutex<Option<String>> {
    TEST_RUMBLE_ID.get_or_init(|| Mutex::new(None))
}

fn launcher_sub_lock() -> &'static Mutex<Option<std::sync::mpsc::Sender<LauncherPadFrame>>> {
    LAUNCHER_SUB.get_or_init(|| Mutex::new(None))
}

fn has_launcher_subscriber() -> bool {
    launcher_sub_lock().lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

fn emit_launcher_frame(last: &mut Option<LauncherPadFrame>, frame: LauncherPadFrame) {
    if last.as_ref() == Some(&frame) {
        return;
    }
    let mut guard = launcher_sub_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tx) = guard.as_ref() {
        if tx.send(frame.clone()).is_err() {
            *guard = None;
            return;
        }
    }
    *last = Some(frame);
}

fn publish_summary(next: ConnectedPadSummary) {
    *summary_lock().lock().unwrap_or_else(|e| e.into_inner()) = next;
}

pub fn connected_summary() -> ConnectedPadSummary {
    summary_lock().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Subscribe to normalized gamepad frames from Aurora's single SDL3 host reader.
pub fn subscribe_launcher_pads() -> std::sync::mpsc::Receiver<LauncherPadFrame> {
    let (tx, rx) = std::sync::mpsc::channel();
    *launcher_sub_lock().lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    platform::ensure_ui_reader();
    rx
}

fn vpad_active() -> bool {
    VPAD_USERS.load(std::sync::atomic::Ordering::Acquire) > 0
}

fn vpad_include_xbox() -> bool {
    VPAD_INCLUDE_XBOX.load(std::sync::atomic::Ordering::Acquire) > 0
}

/// Start forwarding translated physical controllers to OS virtual Xbox 360 pads
/// (`uinput` on Linux, optional `ViGEmBus` on Windows) for the duration of a launch.
pub(crate) fn start_virtual_pads(include_xbox: bool) -> bool {
    if !crate::vpad::is_available() {
        return false;
    }
    VPAD_USERS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    if include_xbox {
        VPAD_INCLUDE_XBOX.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    platform::ensure_ui_reader();
    true
}

pub(crate) fn stop_virtual_pads(include_xbox: bool) {
    let mut n = VPAD_USERS.load(std::sync::atomic::Ordering::Acquire);
    while n > 0 {
        match VPAD_USERS.compare_exchange_weak(
            n,
            n - 1,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(now) => n = now,
        }
    }
    if include_xbox {
        let mut x = VPAD_INCLUDE_XBOX.load(std::sync::atomic::Ordering::Acquire);
        while x > 0 {
            match VPAD_INCLUDE_XBOX.compare_exchange_weak(
                x,
                x - 1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(now) => x = now,
            }
        }
    }
}

fn ui_active() -> bool {
    let last = UI_LAST_POLL_MS.load(std::sync::atomic::Ordering::Relaxed);
    last != 0 && shm::now_ms().saturating_sub(last) < 2000
}

pub fn poll_ui() -> DetectionSnapshot {
    UI_LAST_POLL_MS.store(shm::now_ms(), std::sync::atomic::Ordering::Relaxed);
    platform::ensure_ui_reader();
    snapshot_lock().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn refresh_ui() {
    FORCE_RESCAN.store(true, std::sync::atomic::Ordering::Release);
    UI_LAST_POLL_MS.store(shm::now_ms(), std::sync::atomic::Ordering::Relaxed);
    platform::ensure_ui_reader();
}

pub fn trigger_test_rumble(controller_id: &str) {
    *rumble_lock().lock().unwrap_or_else(|e| e.into_inner()) = Some(controller_id.to_string());
}

#[cfg(not(windows))]
pub fn start() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn stop() {}

#[cfg(windows)]
pub use platform::{start, stop};
#[cfg(windows)]
pub(crate) use platform::native_xinput_slots;

#[cfg(windows)]
mod platform {
    use super::shm::{self, Shared, State};
    use super::{
        ConnectedPadSummary, ConnectionKind, ControllerFamily, DetectedController, DetectionSnapshot, FORCE_RESCAN,
        LauncherPadFrame, LauncherPadSample, build_live_input, emit_launcher_frame, has_launcher_subscriber,
        publish_summary, rumble_lock, sdl_dyn::{self, Gp, Sdl}, settings, snapshot_lock, ui_active, vpad_active,
        vpad_include_xbox,
    };
    use std::collections::HashSet;
    use std::ffi::{c_char, c_void};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    fn mapping() -> Option<(&'static Shared, &'static str)> {
        static MAP: OnceLock<Option<shm::Shm>> = OnceLock::new();
        let shm = MAP
            .get_or_init(|| {
                let created = shm::Shm::create();
                if created.is_none() {
                    tracing::warn!("controllers: couldn't create the shared memory");
                }
                created
            })
            .as_ref()?;
        Some((shm.shared(), shm.name()))
    }

    static USERS: AtomicUsize = AtomicUsize::new(0);
    static RUNNING: Mutex<bool> = Mutex::new(false);

    fn reader_needed() -> bool {
        USERS.load(Ordering::Acquire) != 0 || ui_active() || has_launcher_subscriber() || vpad_active()
    }

    pub fn ensure_ui_reader() {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if !*running {
            if std::thread::Builder::new().name("pads".into()).spawn(move || read_loop(false)).is_ok() {
                *running = true;
            }
        }
    }

    pub fn start() -> Option<String> {
        let _sdl = sdl_dyn::sdl()?;
        let (_, name) = mapping()?;
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        USERS.fetch_add(1, Ordering::AcqRel);
        if !*running {
            if std::thread::Builder::new().name("pads".into()).spawn(move || read_loop(true)).is_err() {
                USERS.fetch_sub(1, Ordering::AcqRel);
                return None;
            }
            *running = true;
        }
        Some(name.to_string())
    }

    pub fn stop() {
        let mut n = USERS.load(Ordering::Acquire);
        while n > 0 {
            match USERS.compare_exchange_weak(n, n - 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(now) => n = now,
            }
        }
    }

    struct OpenPad {
        id: u32,
        gp: Gp,
        name: String,
        family: ControllerFamily,
        has_hid_path: bool,
        vendor: u16,
        product: u16,
        mapping: Option<String>,
        has_gyro: bool,
        last_shm: State,
        rumble: (u16, u16),
        rumbled: Instant,
        vpad: Option<crate::vpad::VirtualXboxPad>,
    }

    fn read_loop(has_shm_user: bool) {
        let _ = has_shm_user;
        let sdl = sdl_dyn::sdl();
        let shared = mapping().map(|(s, _)| s);
        if let Some(sdl) = sdl {
            run_sdl_loop(sdl, shared);
        } else {
            run_xinput_ui_loop();
        }
    }

    /// Bit i is set when XInputGetState succeeds for player index i.
    pub(crate) fn native_xinput_slots() -> Option<u8> {
        type GetState = unsafe extern "system" fn(u32, *mut [u8; 16]) -> u32;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryW(name: *const u16) -> *mut c_void;
            fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        static GET_STATE: OnceLock<Option<GetState>> = OnceLock::new();
        let get = GET_STATE
            .get_or_init(|| {
                for dll in ["xinput1_4.dll", "xinput1_3.dll", "xinput9_1_0.dll"] {
                    let wide: Vec<u16> = dll.encode_utf16().chain([0]).collect();
                    let m = unsafe { LoadLibraryW(wide.as_ptr()) };
                    if m.is_null() {
                        continue;
                    }
                    let g = unsafe { GetProcAddress(m, c"XInputGetState".as_ptr()) };
                    if !g.is_null() {
                        return Some(unsafe { std::mem::transmute::<*mut c_void, GetState>(g) });
                    }
                }
                None
            })
            .as_ref()?;
        let mut slots = 0u8;
        for i in 0..4u32 {
            let mut buf = [0u8; 16];
            if unsafe { get(i, &mut buf) } == 0 {
                slots |= 1 << i;
            }
        }
        Some(slots)
    }

    fn run_sdl_loop(sdl: &'static Sdl, shared: Option<&'static Shared>) {
        unsafe {
            for (k, v) in [
                (c"SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", c"1"),
                (c"SDL_JOYSTICK_THREAD", c"1"),
                (c"SDL_JOYSTICK_HIDAPI_STEAMDECK", c"1"),
            ] {
                (sdl.set_hint)(k.as_ptr(), v.as_ptr());
            }
        }
        if !unsafe { (sdl.init)(sdl_dyn::INIT_GAMEPAD) } {
            let e = sdl_dyn::cstr_lossy(unsafe { (sdl.get_error)() });
            tracing::warn!("controllers: SDL couldn't start: {e}");
            run_xinput_ui_loop();
            return;
        }
        unsafe {
            (sdl.set_joystick_events)(false);
            (sdl.set_gamepad_events)(false);
        }
        sdl_dyn::load_user_mappings(sdl);
        tracing::info!("controllers: SDL3 unified host reader active (Windows)");
        let mut pads: Vec<OpenPad> = Vec::new();
        let mut seen_unmapped: HashSet<u32> = HashSet::new();
        let mut last_launcher: Option<LauncherPadFrame> = None;
        let mut scanned: Option<Instant> = None;
        let mut real_xinput_count = 0usize;
        loop {
            if !reader_needed() {
                let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
                if !reader_needed() {
                    for o in pads.drain(..) {
                        drop(o.vpad);
                        unsafe { (sdl.close)(o.gp) };
                    }
                    if let Some(s) = shared {
                        for slot in &s.slots {
                            slot.write(None);
                        }
                    }
                    unsafe { (sdl.quit_subsystem)(sdl_dyn::INIT_GAMEPAD) };
                    *running = false;
                    tracing::info!("controllers: stopped reading pads");
                    return;
                }
            }
            if let Some(pump) = sdl.pump_events {
                unsafe { pump() };
            }
            unsafe { (sdl.update)() };
            let force = FORCE_RESCAN.swap(false, Ordering::AcqRel);
            if force {
                sdl_dyn::load_user_mappings(sdl);
            }
            if force || scanned.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
                scanned = Some(Instant::now());
                real_xinput_count = native_xinput_slots().unwrap_or_default().count_ones() as usize;
                let mut n = 0;
                let ids = unsafe { (sdl.get_gamepads)(&mut n) };
                let list: Vec<u32> = if !ids.is_null() {
                    let v = unsafe { std::slice::from_raw_parts(ids, n.max(0) as usize) }.to_vec();
                    unsafe { (sdl.free)(ids.cast()) };
                    v
                } else {
                    Vec::new()
                };
                sdl_dyn::log_unmapped_joysticks(sdl, &list, &mut seen_unmapped);
                for id in list {
                    if pads.iter().any(|o| o.id == id) {
                        continue;
                    }
                    let gp = unsafe { (sdl.open)(id) };
                    if gp.is_null() {
                        continue;
                    }
                    let name = sdl_dyn::cstr_lossy(unsafe { (sdl.name_for_id)(id) });
                    let kind = unsafe { (sdl.type_for_id)(id) };
                    let vendor = unsafe { (sdl.vendor_for_id)(id) };
                    let product = sdl.product_for_id.map_or(0, |f| unsafe { f(id) });
                    // Skip newly-appeared ViGEmBus virtual Xbox 360 pads created by Aurora
                    // while virtual pad forwarding is active so we never re-read our own output.
                    if vpad_active()
                        && pads.iter().any(|p| p.vpad.is_some())
                        && vendor == 0x045e
                        && product == 0x028e
                    {
                        unsafe { (sdl.close)(gp) };
                        continue;
                    }
                    let family = ControllerFamily::classify(kind, vendor, &name);
                    let mapping = sdl_dyn::mapping_for_id(sdl, id);
                    let has_hid_path = sdl.get_path.is_some_and(|f| {
                        let s = sdl_dyn::cstr_lossy(unsafe { f(gp) }).to_ascii_lowercase();
                        s.contains("hid#") || s.contains("usb#") || s.contains("bthenum#")
                    });
                    let has_gyro = sdl_dyn::enable_sensors(sdl, gp);
                    tracing::info!(
                        "controllers: detected \"{}\" ({}, vid={:04x}, pid={:04x}, hid_path={})",
                        name,
                        family.label(),
                        vendor,
                        product,
                        has_hid_path
                    );
                    pads.push(OpenPad {
                        id,
                        gp,
                        name,
                        family,
                        has_hid_path,
                        vendor,
                        product,
                        mapping,
                        has_gyro,
                        last_shm: State::default(),
                        rumble: (0, 0),
                        rumbled: Instant::now(),
                        vpad: None,
                    });
                }
            }

            let cfg = settings();
            let cal = cfg.calibration;
            let rumble_target = rumble_lock().lock().unwrap_or_else(|e| e.into_inner()).take();
            let game_active = USERS.load(Ordering::Acquire) > 0;
            let virtual_on = vpad_active();
            let virtual_include_xbox = vpad_include_xbox();
            let mut ui_list = Vec::with_capacity(pads.len());
            let mut launcher_pads = Vec::with_capacity(pads.len());
            let mut summary = ConnectedPadSummary { sdl_active: true, ..Default::default() };
            let mut shm_idx = 0usize;
            let mut vpad_idx = 0usize;
            let mut xbox_skipped = 0usize;

            pads.retain_mut(|o| {
                if !unsafe { (sdl.connected)(o.gp) } {
                    tracing::info!("controllers: {} disconnected", o.name);
                    drop(o.vpad.take());
                    unsafe { (sdl.close)(o.gp) };
                    return false;
                }
                let (buttons, misc, touchpad, paddles, sticks, triggers, motion) = sdl_dyn::read_raw(sdl, o.gp);
                let lx_f = sticks[0] as f32 / 32767.0;
                let ly_f = sticks[1] as f32 / 32767.0;
                let rx_f = sticks[2] as f32 / 32767.0;
                let ry_f = sticks[3] as f32 / 32767.0;
                let lt_f = triggers[0].max(0) as f32 / 32767.0;
                let rt_f = triggers[1].max(0) as f32 / 32767.0;
                let touchpads = sdl_dyn::read_touchpads(sdl, o.gp, touchpad);
                launcher_pads.push(LauncherPadSample {
                    id: o.id,
                    buttons,
                    misc,
                    lx: lx_f.clamp(-1.0, 1.0),
                    ly: ly_f.clamp(-1.0, 1.0),
                    rx: rx_f.clamp(-1.0, 1.0),
                    ry: ry_f.clamp(-1.0, 1.0),
                    lt: lt_f.clamp(0.0, 1.0),
                    rt: rt_f.clamp(0.0, 1.0),
                    touchpads,
                });

                summary.total += 1;
                if let Some(m) = &o.mapping {
                    if !summary.sdl_mappings.contains(m) {
                        summary.sdl_mappings.push(m.clone());
                    }
                }
                if cfg.families.is_enabled(o.family) {
                    if o.family == ControllerFamily::Xbox && !o.has_hid_path {
                        summary.xbox_enabled += 1;
                    } else {
                        summary.non_xbox_enabled += 1;
                        if o.vendor != 0 || o.product != 0 {
                            let vp = (o.vendor, o.product);
                            if !summary.non_xbox_vid_pids.contains(&vp) {
                                summary.non_xbox_vid_pids.push(vp);
                            }
                        }
                    }
                }

                let raw_state = {
                    let mut st = State {
                        packet: 0,
                        buttons,
                        paddles: paddles.iter().enumerate().fold(0, |mask, (i, down)| mask | (u8::from(*down) << i)),
                        lx: sticks[0],
                        ly: sticks[1],
                        rx: sticks[2],
                        ry: sticks[3],
                        lt: sdl_dyn::trigger_u8(triggers[0]),
                        rt: sdl_dyn::trigger_u8(triggers[1]),
                        family: o.family.slot_family(),
                        device_id: o.id,
                        ..Default::default()
                    };
                    if let Some((accel, gyro)) = motion {
                        st.set_motion(accel, gyro);
                    }
                    st
                };

                let id_str = format!("sdl-{}", o.id);
                if rumble_target.as_deref() == Some(id_str.as_str()) && cal.rumble_enabled {
                    unsafe { (sdl.rumble)(o.gp, 0xFFFF, 0xFFFF, 400) };
                }

                // Publish enabled controllers not already backed by a native XInput slot.
                let mut assigned_slot = None;
                let want_shm = game_active
                    && shm_idx < shm::SLOTS
                    && super::should_publish_to_shm(o.family, o.has_hid_path, &cfg.families, real_xinput_count, &mut xbox_skipped);
                if want_shm {
                    if let Some(s) = shared {
                        let mut calibrated = cal.apply_to_state(raw_state);
                        calibrated.packet = o.last_shm.packet;
                        if calibrated != o.last_shm {
                            calibrated.packet = calibrated.packet.wrapping_add(1);
                        }
                        o.last_shm = calibrated;
                        s.slots[shm_idx].write(Some(calibrated));
                        let want = if cal.rumble_enabled { s.slots[shm_idx].rumble() } else { (0, 0) };
                        if want != o.rumble || (want != (0, 0) && o.rumbled.elapsed() > Duration::from_secs(20)) {
                            let ms = if want == (0, 0) { 0 } else { 30_000 };
                            unsafe { (sdl.rumble)(o.gp, want.0, want.1, ms) };
                            o.rumble = want;
                            o.rumbled = Instant::now();
                        }
                        assigned_slot = Some(shm_idx);
                        shm_idx += 1;
                    }
                }

                // Optional OS virtual Xbox 360 gamepad (ViGEmBus) for HeadlessHost launches.
                let want_vpad = virtual_on
                    && !game_active
                    && vpad_idx < 4
                    && cfg.families.is_enabled(o.family)
                    && (virtual_include_xbox || o.family != ControllerFamily::Xbox || o.has_hid_path);
                if want_vpad {
                    if o.vpad.is_none() {
                        match crate::vpad::VirtualXboxPad::create(&o.name) {
                            Ok(vp) => {
                                tracing::info!("controllers: created {} virtual Xbox pad for {}", crate::vpad::backend_name(), o.name);
                                o.vpad = Some(vp);
                            }
                            Err(e) => tracing::warn!("controllers: virtual pad for {} failed: {e}", o.name),
                        }
                    }
                    if let Some(vp) = o.vpad.as_mut() {
                        let calibrated = cal.apply_to_state(raw_state);
                        let _ = vp.update(crate::vpad::VirtualPadState {
                            buttons: calibrated.buttons,
                            lx: calibrated.lx,
                            ly: calibrated.ly,
                            rx: calibrated.rx,
                            ry: calibrated.ry,
                            lt: calibrated.lt,
                            rt: calibrated.rt,
                        });
                        if let Some((low, high)) = vp.poll_rumble() {
                            if cal.rumble_enabled {
                                let ms = if low == 0 && high == 0 { 0 } else { 30_000 };
                                unsafe { (sdl.rumble)(o.gp, low, high, ms) };
                            }
                        }
                        assigned_slot = Some(vpad_idx);
                        vpad_idx += 1;
                    }
                } else if o.vpad.is_some() {
                    o.vpad = None;
                }

                if ui_active() {
                    let (conn, batt) = sdl_dyn::connection_and_battery(sdl, o.gp);
                    let (accel, gyro, has_m) = match motion {
                        Some((a, g)) => (a, g, true),
                        None => ([0.0; 3], [0.0; 3], false),
                    };
                    let live = build_live_input(
                        buttons,
                        misc,
                        touchpad,
                        paddles,
                        lx_f,
                        ly_f,
                        rx_f,
                        ry_f,
                        lt_f,
                        rt_f,
                        accel,
                        gyro,
                        has_m,
                        &cal,
                    );
                    ui_list.push(DetectedController {
                        id: id_str,
                        name: o.name.clone(),
                        family: o.family,
                        vendor_id: o.vendor,
                        product_id: o.product,
                        connection: conn,
                        slot: assigned_slot,
                        battery_percent: batt,
                        backend: "SDL3",
                        has_rumble: true,
                        has_gyro: o.has_gyro,
                        live,
                    });
                }
                true
            });

            publish_summary(summary);
            emit_launcher_frame(&mut last_launcher, LauncherPadFrame { connected: launcher_pads.len(), pads: launcher_pads });

            if let Some(s) = shared {
                for slot in shm_idx..shm::SLOTS {
                    s.slots[slot].write(None);
                }
                if game_active {
                    s.heartbeat.store(shm::now_ms(), Ordering::Relaxed);
                }
            }
            if ui_active() {
                *snapshot_lock().lock().unwrap_or_else(|e| e.into_inner()) = DetectionSnapshot {
                    backend: "SDL3",
                    controllers: ui_list,
                    note: None,
                };
            }
            let interval = if game_active || virtual_on { 2 } else if pads.is_empty() { 100 } else { 12 };
            std::thread::sleep(Duration::from_millis(interval));
        }
    }

    /// Fallback when `SDL3.dll` is missing next to Aurora on Windows: expose Xbox XInput pads.
    fn run_xinput_ui_loop() {
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct XPad {
            packet: u32,
            buttons: u16,
            lt: u8,
            rt: u8,
            lx: i16,
            ly: i16,
            rx: i16,
            ry: i16,
        }
        type GetState = unsafe extern "system" fn(u32, *mut XPad) -> u32;
        type SetState = unsafe extern "system" fn(u32, *mut [u16; 2]) -> u32;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryW(name: *const u16) -> *mut c_void;
            fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        let mut funcs: Option<(GetState, Option<SetState>)> = None;
        for dll in ["xinput1_4.dll", "xinput1_3.dll", "xinput9_1_0.dll"] {
            let wide: Vec<u16> = dll.encode_utf16().chain([0]).collect();
            let m = unsafe { LoadLibraryW(wide.as_ptr()) };
            if m.is_null() {
                continue;
            }
            // The extended function returns the Guide bit alongside the normal buttons.
            let mut g = unsafe { GetProcAddress(m, 100usize as *const c_char) };
            if g.is_null() {
                g = unsafe { GetProcAddress(m, c"XInputGetState".as_ptr()) };
            }
            if !g.is_null() {
                let s = unsafe { GetProcAddress(m, c"XInputSetState".as_ptr()) };
                let get: GetState = unsafe { std::mem::transmute(g) };
                let set = (!s.is_null()).then(|| unsafe { std::mem::transmute(s) });
                funcs = Some((get, set));
                break;
            }
        }
        let mut last_launcher: Option<LauncherPadFrame> = None;
        loop {
            if !reader_needed() {
                let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
                if !reader_needed() {
                    *running = false;
                    return;
                }
            }
            let cfg = settings();
            let cal = cfg.calibration;
            let rumble_target = rumble_lock().lock().unwrap_or_else(|e| e.into_inner()).take();
            let mut list = Vec::new();
            let mut launcher_pads = Vec::new();
            if let Some((get, set)) = funcs {
                for i in 0..4usize {
                    let mut xp = XPad::default();
                    if unsafe { get(i as u32, &mut xp) } == 0 {
                        let lx_f = (xp.lx as f32 / 32767.0).clamp(-1.0, 1.0);
                        let ly_f = (xp.ly as f32 / 32767.0).clamp(-1.0, 1.0);
                        let rx_f = (xp.rx as f32 / 32767.0).clamp(-1.0, 1.0);
                        let ry_f = (xp.ry as f32 / 32767.0).clamp(-1.0, 1.0);
                        let lt_f = (xp.lt as f32 / 255.0).clamp(0.0, 1.0);
                        let rt_f = (xp.rt as f32 / 255.0).clamp(0.0, 1.0);
                        launcher_pads.push(LauncherPadSample {
                            id: i as u32 + 1,
                            buttons: xp.buttons,
                            misc: false,
                            lx: lx_f,
                            ly: ly_f,
                            rx: rx_f,
                            ry: ry_f,
                            lt: lt_f,
                            rt: rt_f,
                            touchpads: [TouchpadFinger::default(); 2],
                        });
                        let id = format!("xinput-{i}");
                        if rumble_target.as_deref() == Some(id.as_str()) && cal.rumble_enabled
                            && let Some(set_fn) = set
                        {
                            let mut v = [0x5000u16, 0x9000u16];
                            unsafe { set_fn(i as u32, &mut v) };
                        }
                        if ui_active() {
                            let live = build_live_input(
                                xp.buttons,
                                false,
                                false,
                                [false; 4],
                                lx_f,
                                ly_f,
                                rx_f,
                                ry_f,
                                lt_f,
                                rt_f,
                                [0.0; 3],
                                [0.0; 3],
                                false,
                                &cal,
                            );
                            list.push(DetectedController {
                                id,
                                name: format!("Xbox Controller #{}", i + 1),
                                family: ControllerFamily::Xbox,
                                vendor_id: 0x045e,
                                product_id: 0,
                                connection: ConnectionKind::Unknown,
                                slot: Some(i),
                                battery_percent: None,
                                backend: "XInput",
                                has_rumble: set.is_some(),
                                has_gyro: false,
                                live,
                            });
                        }
                    }
                }
            }
            let total = launcher_pads.len();
            publish_summary(ConnectedPadSummary {
                total,
                xbox_enabled: if cfg.families.xbox { total } else { 0 },
                non_xbox_enabled: 0,
                non_xbox_vid_pids: Vec::new(),
                sdl_mappings: Vec::new(),
                sdl_active: false,
            });
            emit_launcher_frame(&mut last_launcher, LauncherPadFrame { connected: total, pads: launcher_pads });
            let interval = if total == 0 { 100 } else { 16 };
            if ui_active() {
                *snapshot_lock().lock().unwrap_or_else(|e| e.into_inner()) = DetectionSnapshot {
                    backend: "XInput",
                    controllers: list,
                    note: Some("SDL3.dll is missing next to Aurora; only Xbox XInput controllers are visible.".into()),
                };
            }
            std::thread::sleep(Duration::from_millis(interval));
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::shm::State;
    use super::{
        ConnectedPadSummary, ConnectionKind, ControllerFamily, DetectedController, DetectionSnapshot, FORCE_RESCAN,
        LauncherPadFrame, LauncherPadSample, LiveInput, build_live_input, emit_launcher_frame, has_launcher_subscriber,
        publish_summary, rumble_lock, sdl_dyn::{self, Gp, Sdl}, settings, snapshot_lock, ui_active, vpad_active,
        vpad_include_xbox,
    };
    use std::collections::{HashMap, HashSet};
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    static RUNNING: Mutex<bool> = Mutex::new(false);

    fn reader_needed() -> bool {
        ui_active() || has_launcher_subscriber() || vpad_active()
    }

    pub fn ensure_ui_reader() {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if !*running {
            if std::thread::Builder::new().name("pads-linux".into()).spawn(read_loop).is_ok() {
                *running = true;
            }
        }
    }

    struct SdlPad {
        id: u32,
        gp: Gp,
        name: String,
        family: ControllerFamily,
        vendor: u16,
        product: u16,
        mapping: Option<String>,
        has_gyro: bool,
        vpad: Option<crate::vpad::VirtualXboxPad>,
    }

    fn read_loop() {
        if let Some(sdl) = sdl_dyn::sdl() {
            unsafe {
                for (k, v) in [
                    (c"SDL_NO_SIGNAL_HANDLERS", c"1"),
                    (c"SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", c"1"),
                    (c"SDL_JOYSTICK_HIDAPI_STEAMDECK", c"1"),
                ] {
                    (sdl.set_hint)(k.as_ptr(), v.as_ptr());
                }
            }
            let init_flags = sdl_dyn::INIT_GAMEPAD | sdl_dyn::INIT_HAPTIC;
            if unsafe { (sdl.init)(init_flags) || (sdl.init)(sdl_dyn::INIT_GAMEPAD) } {
                run_sdl_linux(sdl);
                return;
            }
            tracing::warn!("controllers: SDL3 gamepad init failed ({}); falling back to Linux evdev/js", sdl_dyn::cstr_lossy(unsafe { (sdl.get_error)() }));
        }
        run_native_linux();
    }

    /// Trigger a short rumble pulse directly via Linux evdev `EVIOCSFF` + `EV_FF`
    /// as a fallback when SDL3 rumble returns false (common on Bluetooth Xbox pads).
    fn evdev_rumble_fallback(vendor: u16, product: u16) {
        static RUMBLE_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if RUMBLE_BUSY.swap(true, Ordering::AcqRel) {
            return;
        }
        let pads = discover_sysfs_pads(Path::new("/sys/class/input"), Path::new("/dev/input"));
        let target = pads
            .iter()
            .find(|p| (vendor == 0 || p.vendor_id == vendor) && (product == 0 || p.product_id == product))
            .or_else(|| pads.first());
        let Some(ev_path) = target.and_then(|p| p.event_node.clone()) else {
            RUMBLE_BUSY.store(false, Ordering::Release);
            return;
        };
        if std::thread::Builder::new().name("pad-ff".into()).spawn(move || {
            struct ClearGuard;
            impl Drop for ClearGuard {
                fn drop(&mut self) {
                    RUMBLE_BUSY.store(false, Ordering::Release);
                }
            }
            let _guard = ClearGuard;
            use std::io::Write;
            let Ok(mut f) = File::options().read(true).write(true).open(&ev_path) else {
                tracing::debug!("controllers: evdev rumble cannot open {} rw", ev_path.display());
                return;
            };
            // Linux `struct ff_effect` (x86_64: 48 bytes, FF_RUMBLE = 0x50).
            #[repr(C)]
            struct FfEffect {
                kind: u16,
                id: i16,
                direction: u16,
                trigger_button: u16,
                trigger_interval: u16,
                replay_length: u16,
                replay_delay: u16,
                strong_magnitude: u16,
                weak_magnitude: u16,
                _pad: [u8; 28],
            }
            let mut eff = FfEffect {
                kind: 0x50, // FF_RUMBLE
                id: -1,
                direction: 0,
                trigger_button: 0,
                trigger_interval: 0,
                replay_length: 400,
                replay_delay: 0,
                strong_magnitude: 0xC000,
                weak_magnitude: 0xC000,
                _pad: [0u8; 28],
            };
            // EVIOCSFF = _IOW('E', 0x80, struct ff_effect)
            let eviocsff = ((1u32 << 30)
                | ((std::mem::size_of::<FfEffect>() as u32) << 16)
                | ((b'E' as u32) << 8)
                | 0x80) as libc::Ioctl;
            // EVIOCRMFF = _IOW('E', 0x81, int)
            let eviocrmff = ((1u32 << 30) | (4u32 << 16) | ((b'E' as u32) << 8) | 0x81) as libc::Ioctl;
            if unsafe { libc::ioctl(f.as_raw_fd(), eviocsff, &mut eff as *mut FfEffect) } < 0 {
                return;
            }
            let play = InputEvent {
                time: libc::timeval { tv_sec: 0, tv_usec: 0 },
                kind: 0x15, // EV_FF
                code: eff.id as u16,
                value: 1,
            };
            let bytes = unsafe {
                std::slice::from_raw_parts((&play as *const InputEvent).cast::<u8>(), std::mem::size_of::<InputEvent>())
            };
            let _ = f.write_all(bytes);
            std::thread::sleep(Duration::from_millis(450));
            let _ = unsafe { libc::ioctl(f.as_raw_fd(), eviocrmff, eff.id as libc::c_int) };
        }).is_err() {
            RUMBLE_BUSY.store(false, Ordering::Release);
        }
    }

    fn run_sdl_linux(sdl: &'static Sdl) {
        sdl_dyn::load_user_mappings(sdl);
        tracing::info!("controllers: SDL3 unified host reader active (Linux)");
        let mut pads: Vec<SdlPad> = Vec::new();
        let mut seen_unmapped: HashSet<u32> = HashSet::new();
        let mut last_launcher: Option<LauncherPadFrame> = None;
        let mut scanned: Option<Instant> = None;
        let init_flags = sdl_dyn::INIT_GAMEPAD | sdl_dyn::INIT_HAPTIC;
        loop {
            if !reader_needed() {
                let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
                if !reader_needed() {
                    for o in pads.drain(..) {
                        drop(o.vpad);
                        unsafe { (sdl.close)(o.gp) };
                    }
                    unsafe { (sdl.quit_subsystem)(init_flags) };
                    *running = false;
                    return;
                }
            }
            if let Some(pump) = sdl.pump_events {
                unsafe { pump() };
            }
            unsafe { (sdl.update)() };
            let force = FORCE_RESCAN.swap(false, Ordering::AcqRel);
            if force {
                for o in pads.drain(..) {
                    drop(o.vpad);
                    unsafe { (sdl.close)(o.gp) };
                }
                unsafe {
                    (sdl.quit_subsystem)(init_flags);
                    if let Some(init_sub) = sdl.init_subsystem {
                        if !init_sub(init_flags) {
                            let _ = init_sub(sdl_dyn::INIT_GAMEPAD);
                        }
                    } else if !(sdl.init)(init_flags) {
                        let _ = (sdl.init)(sdl_dyn::INIT_GAMEPAD);
                    }
                    if let Some(pump) = sdl.pump_events {
                        pump();
                    }
                    (sdl.update)();
                }
                sdl_dyn::load_user_mappings(sdl);
                scanned = None;
            }
            if force || scanned.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
                scanned = Some(Instant::now());
                let mut n = 0;
                let ids = unsafe { (sdl.get_gamepads)(&mut n) };
                let list: Vec<u32> = if !ids.is_null() {
                    let v = unsafe { std::slice::from_raw_parts(ids, n.max(0) as usize) }.to_vec();
                    unsafe { (sdl.free)(ids.cast()) };
                    v
                } else {
                    Vec::new()
                };
                sdl_dyn::log_unmapped_joysticks(sdl, &list, &mut seen_unmapped);
                for id in list {
                    if pads.iter().any(|o| o.id == id) {
                        continue;
                    }
                    let gp = unsafe { (sdl.open)(id) };
                    if gp.is_null() {
                        continue;
                    }
                    let name = sdl_dyn::cstr_lossy(unsafe { (sdl.name_for_id)(id) });
                    // Skip Aurora's own virtual Xbox 360 pads (identified by sysfs phys prefix
                    // `aurora-vpad/`) so physical Xbox 360 controllers are never filtered out.
                    let path_str = sdl.get_path.map(|f| sdl_dyn::cstr_lossy(unsafe { f(gp) })).unwrap_or_default();
                    if is_aurora_virtual_event_node(&path_str) {
                        unsafe { (sdl.close)(gp) };
                        continue;
                    }
                    let kind = unsafe { (sdl.type_for_id)(id) };
                    let vendor = unsafe { (sdl.vendor_for_id)(id) };
                    let product = sdl.product_for_id.map_or(0, |f| unsafe { f(id) });
                    let family = ControllerFamily::classify(kind, vendor, &name);
                    let mapping = sdl_dyn::mapping_for_id(sdl, id);
                    let has_gyro = sdl_dyn::enable_sensors(sdl, gp);
                    tracing::info!(
                        "controllers: detected \"{}\" ({}, vid={:04x}, pid={:04x})",
                        name,
                        family.label(),
                        vendor,
                        product
                    );
                    pads.push(SdlPad { id, gp, name, family, vendor, product, mapping, has_gyro, vpad: None });
                }
            }
            let cfg = settings();
            let cal = cfg.calibration;
            let rumble_target = rumble_lock().lock().unwrap_or_else(|e| e.into_inner()).take();
            let virtual_on = vpad_active();
            let virtual_include_xbox = vpad_include_xbox();
            let mut ui_list = Vec::with_capacity(pads.len());
            let mut launcher_pads = Vec::with_capacity(pads.len());
            let mut summary = ConnectedPadSummary { sdl_active: true, ..Default::default() };
            let mut slot_idx = 0usize;
            let mut vpad_idx = 0usize;
            pads.retain_mut(|o| {
                if !unsafe { (sdl.connected)(o.gp) } {
                    tracing::info!("controllers: {} disconnected", o.name);
                    drop(o.vpad.take());
                    unsafe { (sdl.close)(o.gp) };
                    return false;
                }
                let id_str = format!("sdl-{}", o.id);
                if rumble_target.as_deref() == Some(id_str.as_str()) && cal.rumble_enabled {
                    let ok = unsafe { (sdl.rumble)(o.gp, 0xFFFF, 0xFFFF, 400) };
                    if !ok {
                        evdev_rumble_fallback(o.vendor, o.product);
                    }
                }
                let (buttons, misc, touchpad, paddles, sticks, triggers, motion) = sdl_dyn::read_raw(sdl, o.gp);
                let lx_f = (sticks[0] as f32 / 32767.0).clamp(-1.0, 1.0);
                let ly_f = (sticks[1] as f32 / 32767.0).clamp(-1.0, 1.0);
                let rx_f = (sticks[2] as f32 / 32767.0).clamp(-1.0, 1.0);
                let ry_f = (sticks[3] as f32 / 32767.0).clamp(-1.0, 1.0);
                let lt_f = (triggers[0].max(0) as f32 / 32767.0).clamp(0.0, 1.0);
                let rt_f = (triggers[1].max(0) as f32 / 32767.0).clamp(0.0, 1.0);
                let touchpads = sdl_dyn::read_touchpads(sdl, o.gp, touchpad);
                launcher_pads.push(LauncherPadSample {
                    id: o.id,
                    buttons,
                    misc,
                    lx: lx_f,
                    ly: ly_f,
                    rx: rx_f,
                    ry: ry_f,
                    lt: lt_f,
                    rt: rt_f,
                    touchpads,
                });

                summary.total += 1;
                if let Some(m) = &o.mapping {
                    if !summary.sdl_mappings.contains(m) {
                        summary.sdl_mappings.push(m.clone());
                    }
                }
                if cfg.families.is_enabled(o.family) {
                    if o.family == ControllerFamily::Xbox {
                        summary.xbox_enabled += 1;
                    } else {
                        summary.non_xbox_enabled += 1;
                        if o.vendor != 0 || o.product != 0 {
                            let vp = (o.vendor, o.product);
                            if !summary.non_xbox_vid_pids.contains(&vp) {
                                summary.non_xbox_vid_pids.push(vp);
                            }
                        }
                    }
                }

                // Drive /dev/uinput virtual Xbox 360 gamepad when active for a game launch.
                let want_vpad = virtual_on
                    && vpad_idx < 4
                    && cfg.families.is_enabled(o.family)
                    && (virtual_include_xbox || o.family != ControllerFamily::Xbox);
                if want_vpad {
                    if o.vpad.is_none() {
                        match crate::vpad::VirtualXboxPad::create(&o.name) {
                            Ok(vp) => {
                                tracing::info!("controllers: created {} virtual Xbox pad for {}", crate::vpad::backend_name(), o.name);
                                o.vpad = Some(vp);
                            }
                            Err(e) => tracing::warn!("controllers: virtual pad for {} failed: {e}", o.name),
                        }
                    }
                    if let Some(vp) = o.vpad.as_mut() {
                        let raw_state = State {
                            buttons,
                            lx: sticks[0],
                            ly: sticks[1],
                            rx: sticks[2],
                            ry: sticks[3],
                            lt: sdl_dyn::trigger_u8(triggers[0]),
                            rt: sdl_dyn::trigger_u8(triggers[1]),
                            family: o.family.slot_family(),
                            ..Default::default()
                        };
                        let calibrated = cal.apply_to_state(raw_state);
                        let _ = vp.update(crate::vpad::VirtualPadState {
                            buttons: calibrated.buttons,
                            lx: calibrated.lx,
                            ly: calibrated.ly,
                            rx: calibrated.rx,
                            ry: calibrated.ry,
                            lt: calibrated.lt,
                            rt: calibrated.rt,
                        });
                        if let Some((low, high)) = vp.poll_rumble() {
                            if cal.rumble_enabled {
                                let ms = if low == 0 && high == 0 { 0 } else { 30_000 };
                                let ok = unsafe { (sdl.rumble)(o.gp, low, high, ms) };
                                if !ok && (low != 0 || high != 0) {
                                    evdev_rumble_fallback(o.vendor, o.product);
                                }
                            }
                        }
                        vpad_idx += 1;
                    }
                } else if o.vpad.is_some() {
                    o.vpad = None;
                }

                if ui_active() {
                    let (conn, batt) = sdl_dyn::connection_and_battery(sdl, o.gp);
                    let (accel, gyro, has_m) = match motion {
                        Some((a, g)) => (a, g, true),
                        None => ([0.0; 3], [0.0; 3], false),
                    };
                    let live = build_live_input(
                        buttons,
                        misc,
                        touchpad,
                        paddles,
                        lx_f,
                        ly_f,
                        rx_f,
                        ry_f,
                        lt_f,
                        rt_f,
                        accel,
                        gyro,
                        has_m,
                        &cal,
                    );
                    let slot = (slot_idx < 4).then(|| {
                        let s = slot_idx;
                        slot_idx += 1;
                        s
                    });
                    ui_list.push(DetectedController {
                        id: id_str,
                        name: o.name.clone(),
                        family: o.family,
                        vendor_id: o.vendor,
                        product_id: o.product,
                        connection: conn,
                        slot,
                        battery_percent: batt,
                        backend: "SDL3",
                        has_rumble: true,
                        has_gyro: o.has_gyro,
                        live,
                    });
                }
                true
            });
            publish_summary(summary);
            emit_launcher_frame(&mut last_launcher, LauncherPadFrame { connected: launcher_pads.len(), pads: launcher_pads });
            if ui_active() {
                *snapshot_lock().lock().unwrap_or_else(|e| e.into_inner()) = DetectionSnapshot {
                    backend: "SDL3",
                    controllers: ui_list,
                    note: None,
                };
            }
            let interval = if virtual_on { 2 } else if pads.is_empty() { 100 } else { 12 };
            std::thread::sleep(Duration::from_millis(interval));
        }
    }

    // -------------------------------------------------------- Linux sysfs / evdev / js fallback

    #[derive(Debug, Clone)]
    pub(super) struct SysfsPad {
        pub id: String,
        pub name: String,
        pub family: ControllerFamily,
        pub vendor_id: u16,
        pub product_id: u16,
        pub connection: ConnectionKind,
        pub event_node: Option<PathBuf>,
        pub js_node: Option<PathBuf>,
    }

    /// Check if a bit in Linux `/sys/class/input/*/device/capabilities/key` is set.
    /// Words are hex `usize` values listed most-significant word first.
    pub(super) fn has_cap_bit(caps: &str, bit: usize) -> bool {
        let bits_per_word = usize::BITS as usize;
        let words: Vec<&str> = caps.split_whitespace().collect();
        let word_from_lsb = bit / bits_per_word;
        if word_from_lsb >= words.len() {
            return false;
        }
        let idx = words.len() - 1 - word_from_lsb;
        usize::from_str_radix(words[idx], 16).is_ok_and(|w| (w >> (bit % bits_per_word)) & 1 != 0)
    }

    pub(super) fn is_gamepad_caps(key_caps: &str, props: &str) -> bool {
        // Skip dedicated accelerometer/IMU nodes (`INPUT_PROP_ACCELEROMETER = 6`).
        if has_cap_bit(props, 6) {
            return false;
        }
        // BTN_JOYSTICK (0x120..=0x12f) or BTN_GAMEPAD (0x130..=0x13e).
        (0x120usize..=0x13e).any(|b| has_cap_bit(key_caps, b))
    }

    fn read_hex_u16(path: &Path) -> u16 {
        std::fs::read_to_string(path).ok().and_then(|s| u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()).unwrap_or(0)
    }

    fn is_aurora_virtual_phys(phys: &str) -> bool {
        phys.trim().starts_with("aurora-vpad/")
    }

    fn is_aurora_virtual_event_node(dev_path: &str) -> bool {
        let Some(fname) = Path::new(dev_path).file_name().and_then(|s| s.to_str()) else {
            return false;
        };
        let phys_path = Path::new("/sys/class/input").join(fname).join("device/phys");
        std::fs::read_to_string(phys_path).is_ok_and(|p| is_aurora_virtual_phys(&p))
    }

    pub(super) fn discover_sysfs_pads(sys_class_input: &Path, dev_input: &Path) -> Vec<SysfsPad> {
        let Ok(rd) = std::fs::read_dir(sys_class_input) else {
            return Vec::new();
        };
        let mut js_by_dev: HashMap<PathBuf, PathBuf> = HashMap::new();
        let mut events: Vec<(String, PathBuf)> = Vec::new();
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            if fname.starts_with("js") && fname[2..].bytes().all(|b| b.is_ascii_digit()) {
                if let Ok(dev) = std::fs::canonicalize(path.join("device")) {
                    js_by_dev.insert(dev, dev_input.join(&fname));
                }
            } else if fname.starts_with("event") && fname[5..].bytes().all(|b| b.is_ascii_digit()) {
                events.push((fname, path));
            }
        }
        events.sort_by(|a, b| a.0.cmp(&b.0));

        let mut out = Vec::new();
        for (fname, path) in events {
            let dev_dir = path.join("device");
            if std::fs::read_to_string(dev_dir.join("phys")).is_ok_and(|p| is_aurora_virtual_phys(&p)) {
                continue;
            }
            let key_caps = std::fs::read_to_string(dev_dir.join("capabilities/key")).unwrap_or_default();
            let props = std::fs::read_to_string(dev_dir.join("properties")).unwrap_or_default();
            if !is_gamepad_caps(&key_caps, &props) {
                continue;
            }
            let name = std::fs::read_to_string(dev_dir.join("name")).map(|s| s.trim().to_string()).unwrap_or_else(|_| fname.clone());
            let vendor_id = read_hex_u16(&dev_dir.join("id/vendor"));
            let product_id = read_hex_u16(&dev_dir.join("id/product"));
            let bustype = read_hex_u16(&dev_dir.join("id/bustype"));
            let connection = match bustype {
                0x0005 => ConnectionKind::Wireless,
                0x0003 => ConnectionKind::Wired,
                _ => ConnectionKind::Unknown,
            };
            let canon = std::fs::canonicalize(&dev_dir).ok();
            let js_node = canon.as_ref().and_then(|c| js_by_dev.get(c).cloned());
            let ev_path = dev_input.join(&fname);
            let family = ControllerFamily::classify(0, vendor_id, &name);
            out.push(SysfsPad {
                id: format!("linux-{fname}"),
                name,
                family,
                vendor_id,
                product_id,
                connection,
                event_node: Some(ev_path),
                js_node,
            });
        }
        out
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InputEvent {
        time: libc::timeval,
        kind: u16,
        code: u16,
        value: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct JsEvent {
        time: u32,
        value: i16,
        kind: u8,
        number: u8,
    }

    #[derive(Default)]
    pub(super) struct RawLinuxState {
        pub(super) buttons: u16,
        pub(super) misc: bool,
        pub(super) lx: f32,
        pub(super) ly: f32,
        pub(super) rx: f32,
        pub(super) ry: f32,
        pub(super) lt: f32,
        pub(super) rt: f32,
        hat_x: i32,
        hat_y: i32,
        pub(super) touchpads: [super::TouchpadFinger; 2],
    }

    pub(super) fn is_steam_deck_evdev(vendor_id: u16, product_id: u16, name: &str) -> bool {
        vendor_id == 0x28de && (product_id == 0x1205 || name.to_ascii_lowercase().contains("steam deck"))
    }

    enum OpenNode {
        Evdev { file: File, bounds: HashMap<u16, (i32, i32)>, raw: RawLinuxState, is_deck: bool },
        Js { file: File, raw: RawLinuxState },
        MetadataOnly,
    }

    fn query_abs_bounds(fd: i32) -> HashMap<u16, (i32, i32)> {
        let mut map = HashMap::new();
        for code in [0u16, 1, 2, 3, 4, 5, 16, 17, 18, 19, 20, 21] {
            let mut info = [0i32; 6];
            let req = ((2u32 << 30) | (24u32 << 16) | ((b'E' as u32) << 8) | (0x40 + code as u32)) as libc::Ioctl;
            if unsafe { libc::ioctl(fd, req, info.as_mut_ptr()) } >= 0 {
                map.insert(code, (info[1], info[2]));
            }
        }
        map
    }

    fn norm_axis(val: i32, min: i32, max: i32) -> f32 {
        if max <= min {
            return 0.0;
        }
        let span = (max - min) as f32;
        (((val - min) as f32 / span) * 2.0 - 1.0).clamp(-1.0, 1.0)
    }

    fn norm_trigger(val: i32, min: i32, max: i32) -> f32 {
        if max <= min {
            return 0.0;
        }
        ((val - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
    }

    fn set_bit(buttons: &mut u16, bit: u16, down: bool) {
        if down {
            *buttons |= bit;
        } else {
            *buttons &= !bit;
        }
    }

    fn open_node(p: &SysfsPad) -> OpenNode {
        if let Some(ev) = &p.event_node
            && let Ok(f) = File::options().read(true).custom_flags(libc::O_NONBLOCK).open(ev)
        {
            let bounds = query_abs_bounds(f.as_raw_fd());
            let is_deck = is_steam_deck_evdev(p.vendor_id, p.product_id, &p.name);
            return OpenNode::Evdev { file: f, bounds, raw: RawLinuxState::default(), is_deck };
        }
        if let Some(js) = &p.js_node
            && let Ok(f) = File::options().read(true).custom_flags(libc::O_NONBLOCK).open(js)
        {
            return OpenNode::Js { file: f, raw: RawLinuxState::default() };
        }
        OpenNode::MetadataOnly
    }

    pub(super) fn apply_evdev_event(
        bounds: &HashMap<u16, (i32, i32)>,
        raw: &mut RawLinuxState,
        is_deck: bool,
        kind: u16,
        code: u16,
        value: i32,
    ) {
        match kind {
            0x01 => {
                let down = value != 0;
                match code {
                    0x100 => raw.misc = down,                           // BTN_MISC (Deck Quick Access `⋯`)
                    0x121 if is_deck => raw.touchpads[0].click = down,  // BTN_THUMB (Deck left pad click)
                    0x122 if is_deck => raw.touchpads[1].click = down,  // BTN_THUMB2 (Deck right pad click)
                    0x126 if is_deck => raw.touchpads[0].down = down,   // BTN_BASE (Deck left pad touch)
                    0x127 if is_deck => raw.touchpads[1].down = down,   // BTN_BASE2 (Deck right pad touch)
                    0x130 => set_bit(&mut raw.buttons, 0x1000, down),   // BTN_SOUTH
                    0x131 => set_bit(&mut raw.buttons, 0x2000, down),   // BTN_EAST
                    0x134 => set_bit(&mut raw.buttons, 0x4000, down),   // BTN_WEST (X)
                    0x133 => set_bit(&mut raw.buttons, 0x8000, down),   // BTN_NORTH (Y)
                    0x136 => set_bit(&mut raw.buttons, 0x0100, down),   // BTN_TL
                    0x137 => set_bit(&mut raw.buttons, 0x0200, down),   // BTN_TR
                    0x13a => set_bit(&mut raw.buttons, 0x0020, down),   // BTN_SELECT
                    0x13b => set_bit(&mut raw.buttons, 0x0010, down),   // BTN_START
                    0x13c => set_bit(&mut raw.buttons, super::shm::GUIDE, down), // BTN_MODE
                    0x13d => set_bit(&mut raw.buttons, 0x0040, down),   // BTN_THUMBL
                    0x13e => set_bit(&mut raw.buttons, 0x0080, down),   // BTN_THUMBR
                    0x220 => set_bit(&mut raw.buttons, 0x0001, down),   // DPAD_UP
                    0x221 => set_bit(&mut raw.buttons, 0x0002, down),   // DPAD_DOWN
                    0x222 => set_bit(&mut raw.buttons, 0x0004, down),   // DPAD_LEFT
                    0x223 => set_bit(&mut raw.buttons, 0x0008, down),   // DPAD_RIGHT
                    0x138 => raw.lt = if down { 1.0 } else { 0.0 },     // BTN_TL2
                    0x139 => raw.rt = if down { 1.0 } else { 0.0 },     // BTN_TR2
                    _ => {}
                }
            }
            0x03 => {
                let (min, max) = bounds.get(&code).copied().unwrap_or((-32768, 32767));
                match code {
                    0x00 => raw.lx = norm_axis(value, min, max),
                    0x01 => raw.ly = -norm_axis(value, min, max),
                    0x03 => raw.rx = norm_axis(value, min, max),
                    0x04 => raw.ry = -norm_axis(value, min, max),
                    0x02 if !is_deck => raw.lt = norm_trigger(value, if min < 0 { 0 } else { min }, max),
                    0x05 if !is_deck => raw.rt = norm_trigger(value, if min < 0 { 0 } else { min }, max),
                    // Linux `hid-steam` maps Deck RT to ABS_HAT2X (0x14) and LT to ABS_HAT2Y (0x15),
                    // and puts Left/Right Trackpad axes on ABS_HAT0X/Y (0x10/0x11) and ABS_HAT1X/Y (0x12/0x13).
                    0x14 if is_deck => raw.rt = norm_trigger(value, if min < 0 { 0 } else { min }, max),
                    0x15 if is_deck => raw.lt = norm_trigger(value, if min < 0 { 0 } else { min }, max),
                    0x10 if is_deck => raw.touchpads[0].x = norm_trigger(value, min, max),
                    0x11 if is_deck => raw.touchpads[0].y = norm_trigger(value, min, max),
                    0x12 if is_deck => raw.touchpads[1].x = norm_trigger(value, min, max),
                    0x13 if is_deck => raw.touchpads[1].y = norm_trigger(value, min, max),
                    0x10 => {
                        raw.hat_x = value;
                        set_bit(&mut raw.buttons, 0x0004, value < 0);
                        set_bit(&mut raw.buttons, 0x0008, value > 0);
                    }
                    0x11 => {
                        raw.hat_y = value;
                        set_bit(&mut raw.buttons, 0x0001, value < 0);
                        set_bit(&mut raw.buttons, 0x0002, value > 0);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn poll_open_node(node: &mut OpenNode) {
        match node {
            OpenNode::Evdev { file, bounds, raw, is_deck } => {
                let mut buf = [0u8; std::mem::size_of::<InputEvent>() * 16];
                while let Ok(n) = file.read(&mut buf) {
                    let sz = std::mem::size_of::<InputEvent>();
                    if n < sz {
                        break;
                    }
                    for chunk in buf[..n].chunks_exact(sz) {
                        let ev = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<InputEvent>()) };
                        apply_evdev_event(bounds, raw, *is_deck, ev.kind, ev.code, ev.value);
                    }
                }
            }
            OpenNode::Js { file, raw } => {
                let mut buf = [0u8; std::mem::size_of::<JsEvent>() * 16];
                while let Ok(n) = file.read(&mut buf) {
                    let sz = std::mem::size_of::<JsEvent>();
                    if n < sz {
                        break;
                    }
                    for chunk in buf[..n].chunks_exact(sz) {
                        let ev = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<JsEvent>()) };
                        match ev.kind & !0x80 {
                            1 => {
                                let down = ev.value != 0;
                                match ev.number {
                                    0 => set_bit(&mut raw.buttons, 0x1000, down),
                                    1 => set_bit(&mut raw.buttons, 0x2000, down),
                                    2 => set_bit(&mut raw.buttons, 0x4000, down),
                                    3 => set_bit(&mut raw.buttons, 0x8000, down),
                                    4 => set_bit(&mut raw.buttons, 0x0100, down),
                                    5 => set_bit(&mut raw.buttons, 0x0200, down),
                                    6 => set_bit(&mut raw.buttons, 0x0020, down),
                                    7 => set_bit(&mut raw.buttons, 0x0010, down),
                                    8 => set_bit(&mut raw.buttons, super::shm::GUIDE, down),
                                    9 => set_bit(&mut raw.buttons, 0x0040, down),
                                    10 => set_bit(&mut raw.buttons, 0x0080, down),
                                    _ => {}
                                }
                            }
                            2 => {
                                let v = ev.value as f32 / 32767.0;
                                match ev.number {
                                    0 => raw.lx = v.clamp(-1.0, 1.0),
                                    1 => raw.ly = (-v).clamp(-1.0, 1.0),
                                    2 => raw.lt = ((v + 1.0) * 0.5).clamp(0.0, 1.0),
                                    3 => raw.rx = v.clamp(-1.0, 1.0),
                                    4 => raw.ry = (-v).clamp(-1.0, 1.0),
                                    5 => raw.rt = ((v + 1.0) * 0.5).clamp(0.0, 1.0),
                                    6 => {
                                        set_bit(&mut raw.buttons, 0x0004, ev.value < -16000);
                                        set_bit(&mut raw.buttons, 0x0008, ev.value > 16000);
                                    }
                                    7 => {
                                        set_bit(&mut raw.buttons, 0x0001, ev.value < -16000);
                                        set_bit(&mut raw.buttons, 0x0002, ev.value > 16000);
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            OpenNode::MetadataOnly => {}
        }
    }

    fn run_native_linux() {
        let mut nodes: Vec<(SysfsPad, OpenNode)> = Vec::new();
        let mut scanned: Option<Instant> = None;
        let mut last_launcher: Option<LauncherPadFrame> = None;
        loop {
            if !reader_needed() {
                let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
                if !reader_needed() {
                    *running = false;
                    return;
                }
            }
            let force = FORCE_RESCAN.swap(false, Ordering::AcqRel);
            if force || scanned.is_none_or(|t| t.elapsed() > Duration::from_millis(800)) {
                scanned = Some(Instant::now());
                let found = discover_sysfs_pads(Path::new("/sys/class/input"), Path::new("/dev/input"));
                let mut next = Vec::with_capacity(found.len());
                for pad in found {
                    if let Some(pos) = nodes.iter().position(|(existing, _)| existing.id == pad.id) {
                        let (_, existing_node) = nodes.swap_remove(pos);
                        next.push((pad, existing_node));
                    } else {
                        let node = open_node(&pad);
                        let kind = match node {
                            OpenNode::Evdev { .. } => "evdev",
                            OpenNode::Js { .. } => "js",
                            OpenNode::MetadataOnly => "sysfs",
                        };
                        tracing::info!(
                            "controllers: detected \"{}\" via fallback ({}, vid={:04x}, pid={:04x})",
                            pad.name,
                            kind,
                            pad.vendor_id,
                            pad.product_id
                        );
                        next.push((pad, node));
                    }
                }
                nodes = next;
            }

            let cfg = settings();
            let cal = cfg.calibration;
            let mut list = Vec::with_capacity(nodes.len());
            let mut launcher_pads = Vec::with_capacity(nodes.len());
            let mut summary = ConnectedPadSummary { sdl_active: false, ..Default::default() };
            let mut any_readonly = false;
            for (idx, (pad, node)) in nodes.iter_mut().enumerate() {
                poll_open_node(node);
                let backend = match node {
                    OpenNode::Evdev { .. } => "evdev",
                    OpenNode::Js { .. } => "js",
                    OpenNode::MetadataOnly => {
                        any_readonly = true;
                        "sysfs"
                    }
                };
                summary.total += 1;
                if cfg.families.is_enabled(pad.family) {
                    if pad.family == ControllerFamily::Xbox {
                        summary.xbox_enabled += 1;
                    } else {
                        summary.non_xbox_enabled += 1;
                        if pad.vendor_id != 0 || pad.product_id != 0 {
                            let vp = (pad.vendor_id, pad.product_id);
                            if !summary.non_xbox_vid_pids.contains(&vp) {
                                summary.non_xbox_vid_pids.push(vp);
                            }
                        }
                    }
                }
                if let OpenNode::Evdev { raw, .. } | OpenNode::Js { raw, .. } = node {
                    launcher_pads.push(LauncherPadSample {
                        id: idx as u32 + 1,
                        buttons: raw.buttons,
                        misc: raw.misc,
                        lx: raw.lx,
                        ly: raw.ly,
                        rx: raw.rx,
                        ry: raw.ry,
                        lt: raw.lt,
                        rt: raw.rt,
                        touchpads: raw.touchpads,
                    });
                }
                if ui_active() {
                    let live = match node {
                        OpenNode::Evdev { raw, .. } | OpenNode::Js { raw, .. } => build_live_input(
                            raw.buttons,
                            raw.misc,
                            raw.touchpads[0].click || raw.touchpads[1].click,
                            [false; 4],
                            raw.lx,
                            raw.ly,
                            raw.rx,
                            raw.ry,
                            raw.lt,
                            raw.rt,
                            [0.0; 3],
                            [0.0; 3],
                            false,
                            &cal,
                        ),
                        OpenNode::MetadataOnly => LiveInput::default(),
                    };
                    list.push(DetectedController {
                        id: pad.id.clone(),
                        name: pad.name.clone(),
                        family: pad.family,
                        vendor_id: pad.vendor_id,
                        product_id: pad.product_id,
                        connection: pad.connection,
                        slot: (idx < 4).then_some(idx),
                        battery_percent: None,
                        backend,
                        has_rumble: false,
                        has_gyro: false,
                        live,
                    });
                }
            }
            publish_summary(summary);
            emit_launcher_frame(&mut last_launcher, LauncherPadFrame { connected: launcher_pads.len(), pads: launcher_pads });
            if ui_active() {
                let note = Some(if any_readonly {
                    "libSDL3.so.0 not found and device permissions restrict evdev reading; check udev / input group permissions.".to_string()
                } else {
                    "libSDL3.so.0 not found next to Aurora; using basic Linux evdev/js fallback.".to_string()
                });
                *snapshot_lock().lock().unwrap_or_else(|e| e.into_inner()) = DetectionSnapshot {
                    backend: "Linux evdev/js",
                    controllers: list,
                    note,
                };
            }
            let interval = if nodes.is_empty() { 100 } else { 16 };
            std::thread::sleep(Duration::from_millis(interval));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shm::FAMILY_ALL;

    #[test]
    fn family_classification_handles_sdl_vendor_and_name() {
        assert_eq!(ControllerFamily::classify(3, 0, "Controller"), ControllerFamily::Xbox);
        assert_eq!(ControllerFamily::classify(6, 0, "Controller"), ControllerFamily::PlayStation);
        assert_eq!(ControllerFamily::classify(7, 0, "Controller"), ControllerFamily::SwitchPro);
        assert_eq!(ControllerFamily::classify(0, 0x054c, "Wireless Controller"), ControllerFamily::PlayStation);
        assert_eq!(ControllerFamily::classify(0, 0x057e, "Pro Controller"), ControllerFamily::SwitchPro);
        assert_eq!(ControllerFamily::classify(0, 0x045e, "Pad"), ControllerFamily::Xbox);
        assert_eq!(ControllerFamily::classify(0, 0x1234, "8BitDo Ultimate Xbox"), ControllerFamily::Xbox);
        assert_eq!(ControllerFamily::classify(0, 0x1234, "DualSense Wireless Controller"), ControllerFamily::PlayStation);
        assert_eq!(ControllerFamily::classify(0, 0x1234, "Generic USB Gamepad"), ControllerFamily::Generic);
    }

    #[test]
    fn resolve_steam_input_honors_override_and_global_toggles() {
        let all = FamilyToggles::default();
        assert_eq!(
            resolve_steam_input(0, &all),
            SteamInputDecision { enabled: true, families_mask: FAMILY_ALL, use_shm_bridge: true }
        );
        assert_eq!(
            resolve_steam_input(1, &all),
            SteamInputDecision { enabled: true, families_mask: FAMILY_ALL, use_shm_bridge: true }
        );
        // Disable Steam Input for game (2) disables both ISteamInput and SHM bridge.
        assert_eq!(
            resolve_steam_input(2, &all),
            SteamInputDecision { enabled: false, families_mask: 0, use_shm_bridge: false }
        );

        // Only Xbox enabled globally -> SHM bridge stays active so Xbox-classified
        // third-party pads that native XInput misses can still reach the game.
        let xbox_only = FamilyToggles { xbox: true, playstation: false, switch_pro: false, generic: false };
        assert_eq!(
            resolve_steam_input(0, &xbox_only),
            SteamInputDecision { enabled: true, families_mask: FAMILY_XBOX, use_shm_bridge: true }
        );
        assert_eq!(
            resolve_steam_input(1, &xbox_only),
            SteamInputDecision { enabled: true, families_mask: FAMILY_XBOX, use_shm_bridge: true }
        );

        // All global families disabled -> Default and Enable both stay disabled.
        let none = FamilyToggles { xbox: false, playstation: false, switch_pro: false, generic: false };
        assert_eq!(
            resolve_steam_input(0, &none),
            SteamInputDecision { enabled: false, families_mask: 0, use_shm_bridge: false }
        );
        assert_eq!(
            resolve_steam_input(1, &none),
            SteamInputDecision { enabled: false, families_mask: 0, use_shm_bridge: false }
        );
    }

    #[test]
    fn settings_persistence_roundtrip_and_clamping() {
        let s = ControllerSettings {
            families: FamilyToggles { xbox: false, playstation: true, switch_pro: false, generic: true },
            calibration: PadCalibration {
                left_deadzone: 0.22,
                right_deadzone: 0.18,
                trigger_threshold: 0.12,
                gyro_enabled: false,
                gyro_sensitivity: 1.75,
                rumble_enabled: false,
                nintendo_layout: true,
            },
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ControllerSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);

        set_settings(ControllerSettings {
            families: s.families,
            calibration: PadCalibration { left_deadzone: 0.99, gyro_sensitivity: 9.0, ..s.calibration },
        });
        let stored = settings();
        assert_eq!(stored.calibration.left_deadzone, 0.50);
        assert_eq!(stored.calibration.gyro_sensitivity, 4.0);
        set_settings(ControllerSettings::default());
    }

    #[test]
    fn build_live_input_applies_calibration() {
        let cal = PadCalibration {
            left_deadzone: 0.25,
            right_deadzone: 0.10,
            trigger_threshold: 0.20,
            gyro_enabled: true,
            gyro_sensitivity: 2.0,
            rumble_enabled: true,
            nintendo_layout: true,
        };
        let live = build_live_input(
            0x1000 | 0x4000,
            true,
            false,
            [true, false, false, false],
            0.15,
            0.0,
            0.55,
            0.0,
            0.10,
            0.60,
            [0.0, 9.8, 0.0],
            [0.5, -0.25, 0.0],
            true,
            &cal,
        );
        assert_eq!(live.buttons, 0x2000 | 0x8000);
        assert_eq!((live.lx, live.ly), (0.0, 0.0));
        assert!((live.raw_lx - 0.15).abs() < 0.001);
        assert!(live.rx > 0.49 && live.rx < 0.51);
        assert_eq!(live.lt, 0.0);
        assert!((live.rt - 0.5).abs() < 0.001);
        assert!(live.has_motion);
        assert_eq!(live.gyro, [1.0, -0.5, 0.0]);
    }

    #[cfg(not(windows))]
    #[test]
    fn linux_sysfs_discovery_filters_gamepads_and_ignores_mice_and_imus() {
        let root = std::env::temp_dir().join(format!("sir-sysfs-pads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sys = root.join("sys");
        let dev = root.join("dev");
        std::fs::create_dir_all(&dev).unwrap();

        // Gamepad event0 (DualSense, BTN_SOUTH = 0x130 -> word index 4 from LSB on 64-bit, bit 0x10000).
        let gp_dev = sys.join("event0/device");
        std::fs::create_dir_all(gp_dev.join("id")).unwrap();
        std::fs::create_dir_all(gp_dev.join("capabilities")).unwrap();
        std::fs::write(gp_dev.join("name"), "Sony Interactive Entertainment DualSense Wireless Controller\n").unwrap();
        std::fs::write(gp_dev.join("id/vendor"), "054c\n").unwrap();
        std::fs::write(gp_dev.join("id/product"), "0ce6\n").unwrap();
        std::fs::write(gp_dev.join("id/bustype"), "0005\n").unwrap();
        // Bit 0x130: word 4 (bits 256..319) on 64-bit, or word 9 (bits 288..319) on 32-bit.
        let bits = usize::BITS as usize;
        let word_idx = 0x130 / bits;
        let bit_val = 1usize << (0x130 % bits);
        let mut words = vec!["0".to_string(); word_idx + 1];
        words[0] = format!("{bit_val:x}");
        std::fs::write(gp_dev.join("capabilities/key"), words.join(" ")).unwrap();
        std::fs::write(gp_dev.join("capabilities/abs"), "0\n").unwrap();
        std::fs::write(gp_dev.join("properties"), "0\n").unwrap();

        // IMU event1 (same key/abs or none, with INPUT_PROP_ACCELEROMETER bit 6 = 0x40).
        let imu_dev = sys.join("event1/device");
        std::fs::create_dir_all(imu_dev.join("id")).unwrap();
        std::fs::create_dir_all(imu_dev.join("capabilities")).unwrap();
        std::fs::write(imu_dev.join("name"), "DualSense Motion Sensors\n").unwrap();
        std::fs::write(imu_dev.join("capabilities/key"), words.join(" ")).unwrap();
        std::fs::write(imu_dev.join("properties"), "40\n").unwrap();

        // Mouse event2 (BTN_LEFT = 0x110).
        let mouse_dev = sys.join("event2/device");
        std::fs::create_dir_all(mouse_dev.join("id")).unwrap();
        std::fs::create_dir_all(mouse_dev.join("capabilities")).unwrap();
        std::fs::write(mouse_dev.join("name"), "USB Gaming Mouse\n").unwrap();
        let m_idx = 0x110 / bits;
        let m_bit = 1usize << (0x110 % bits);
        let mut m_words = vec!["0".to_string(); m_idx + 1];
        m_words[0] = format!("{m_bit:x}");
        std::fs::write(mouse_dev.join("capabilities/key"), m_words.join(" ")).unwrap();
        std::fs::write(mouse_dev.join("properties"), "0\n").unwrap();

        let pads = platform::discover_sysfs_pads(&sys, &dev);
        assert_eq!(pads.len(), 1);
        assert_eq!(pads[0].family, ControllerFamily::PlayStation);
        assert_eq!(pads[0].vendor_id, 0x054c);
        assert_eq!(pads[0].product_id, 0x0ce6);
        assert_eq!(pads[0].connection, ConnectionKind::Wireless);
        assert_eq!(pads[0].id, "linux-event0");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shm_publish_gate_publishes_unbacked_xbox_pads_without_duplicating_real_xinput() {
        let all = FamilyToggles::default();
        // 1 real native XInput pad + 1 HID Xbox-classified pad enumerated FIRST:
        // the HID pad must be published and must NOT consume the native XInput skip!
        let mut skipped = 0;
        assert!(should_publish_to_shm(ControllerFamily::Xbox, true, &all, 1, &mut skipped));
        assert_eq!(skipped, 0);
        assert!(!should_publish_to_shm(ControllerFamily::Xbox, false, &all, 1, &mut skipped));
        assert_eq!(skipped, 1);
        assert!(should_publish_to_shm(ControllerFamily::Xbox, false, &all, 1, &mut skipped));
        assert!(should_publish_to_shm(ControllerFamily::PlayStation, true, &all, 1, &mut skipped));

        // 0 real native XInput pads connected: Xbox-classified pad is published.
        let mut skipped_zero = 0;
        assert!(should_publish_to_shm(ControllerFamily::Xbox, false, &all, 0, &mut skipped_zero));
        assert_eq!(skipped_zero, 0);

        // Disabled family stays blocked.
        let no_xbox = FamilyToggles { xbox: false, ..all };
        let mut skipped_off = 0;
        assert!(!should_publish_to_shm(ControllerFamily::Xbox, true, &no_xbox, 0, &mut skipped_off));
        assert!(should_publish_to_shm(ControllerFamily::SwitchPro, true, &no_xbox, 0, &mut skipped_off));
    }

    #[test]
    fn demand_driven_translation_only_triggers_when_needed() {
        let all = FamilyToggles::default();
        let xbox_only = ConnectedPadSummary {
            total: 1,
            xbox_enabled: 1,
            non_xbox_enabled: 0,
            ..Default::default()
        };
        // Native Xbox pad + default Auto layout stays on HeadlessHost (no translation needed).
        assert!(!xbox_only.needs_translation(0, &all, false));
        // Custom layout or explicit per-game Enable triggers translation.
        assert!(xbox_only.needs_translation(0, &all, true));
        assert!(xbox_only.needs_translation(1, &all, false));
        // Per-game Disable suppresses translation even with a non-Xbox pad.
        let switch_pad = ConnectedPadSummary {
            total: 1,
            xbox_enabled: 0,
            non_xbox_enabled: 1,
            non_xbox_vid_pids: vec![(0x057e, 0x2009)],
            ..Default::default()
        };
        assert!(switch_pad.needs_translation(0, &all, false));
        assert!(!switch_pad.needs_translation(2, &all, true));
    }

    #[cfg(not(windows))]
    #[test]
    fn steam_deck_evdev_maps_triggers_touchpads_and_qam_without_dpad_collision() {
        let mut bounds = std::collections::HashMap::new();
        bounds.insert(0x10, (-32767, 32767));
        bounds.insert(0x11, (-32767, 32767));
        bounds.insert(0x12, (-32767, 32767));
        bounds.insert(0x13, (-32767, 32767));
        bounds.insert(0x14, (0, 32767));
        bounds.insert(0x15, (0, 32767));

        let mut raw = platform::RawLinuxState::default();
        assert!(platform::is_steam_deck_evdev(0x28de, 0x1205, "Steam Deck"));
        // ABS_HAT2Y (LT) and ABS_HAT2X (RT) drive triggers.
        platform::apply_evdev_event(&bounds, &mut raw, true, 0x03, 0x15, 32767);
        platform::apply_evdev_event(&bounds, &mut raw, true, 0x03, 0x14, 16384);
        assert!((raw.lt - 1.0).abs() < 0.001);
        assert!((raw.rt - 0.5).abs() < 0.01);
        // ABS_HAT0X/Y drives left trackpad on Deck, NOT D-pad.
        platform::apply_evdev_event(&bounds, &mut raw, true, 0x03, 0x10, -32767);
        platform::apply_evdev_event(&bounds, &mut raw, true, 0x03, 0x11, 32767);
        platform::apply_evdev_event(&bounds, &mut raw, true, 0x01, 0x126, 1);
        platform::apply_evdev_event(&bounds, &mut raw, true, 0x01, 0x121, 1);
        assert_eq!(raw.buttons, 0);
        assert!(raw.touchpads[0].down && raw.touchpads[0].click);
        assert!(raw.touchpads[0].x < 0.01 && raw.touchpads[0].y > 0.99);
        // BTN_MISC maps QAM (`⋯`).
        platform::apply_evdev_event(&bounds, &mut raw, true, 0x01, 0x100, 1);
        assert!(raw.misc);

        // Generic evdev controller still uses ABS_HAT0X/Y for D-pad and ABS_Z/RZ for LT/RT.
        let mut generic = platform::RawLinuxState::default();
        bounds.insert(0x02, (0, 255));
        bounds.insert(0x05, (0, 255));
        platform::apply_evdev_event(&bounds, &mut generic, false, 0x03, 0x10, -1);
        platform::apply_evdev_event(&bounds, &mut generic, false, 0x03, 0x02, 255);
        assert_eq!(generic.buttons & 0x0004, 0x0004);
        assert!((generic.lt - 1.0).abs() < 0.001);
    }
}
