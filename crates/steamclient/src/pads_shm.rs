//! Pads XInput can't see (PlayStation, Switch, Steam Controller, generic
//! DirectInput pads) as the app reads them with SDL3 on Windows, shared with
//! games through a named file mapping. Shared source: the app includes this
//! file with `#[path]`, so both sides always agree on the layout.
//!
//! The app writes each slot's state (a seqlock keeps a read whole) and a
//! heartbeat; a game writes the rumble it wants on a slot.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

/// Environment variable naming the file mapping (set only on Windows, for
/// games started with our steamclient while the app reads pads).
pub const ENV_PADS: &str = "STEAMINRUST_PADS";
/// `"1"` (or unset) = Steam Input enabled; `"0"` = disabled for this game.
pub const ENV_STEAM_INPUT: &str = "STEAMINRUST_STEAM_INPUT";
/// Bitmask of enabled controller families (`FAMILY_*`, decimal or `0x` hex).
pub const ENV_INPUT_FAMILIES: &str = "STEAMINRUST_INPUT_FAMILIES";
/// Encoded [`PadCalibration`] parameters for this launch.
pub const ENV_INPUT_CALIBRATION: &str = "STEAMINRUST_INPUT_CFG";

pub const FAMILY_XBOX: u32 = 1 << 0;
pub const FAMILY_PLAYSTATION: u32 = 1 << 1;
pub const FAMILY_SWITCH_PRO: u32 = 1 << 2;
pub const FAMILY_GENERIC: u32 = 1 << 3;
pub const FAMILY_ALL: u32 = FAMILY_XBOX | FAMILY_PLAYSTATION | FAMILY_SWITCH_PRO | FAMILY_GENERIC;

/// Slot family code stored inside [`Slot::motion_flags`] (bits 8..16).
pub const SLOT_FAMILY_UNKNOWN: u8 = 0;
pub const SLOT_FAMILY_XBOX: u8 = 1;
pub const SLOT_FAMILY_PLAYSTATION: u8 = 2;
pub const SLOT_FAMILY_SWITCH_PRO: u8 = 3;
pub const SLOT_FAMILY_GENERIC: u8 = 4;

pub const SHM_PREFIX: &str = "Local\\sir-pads-";
pub const MAGIC: u32 = 0x3244_4150; // "PAD2"
pub const SLOTS: usize = 4;
/// A heartbeat older than this means the app stopped reading pads.
pub const STALE_MS: u64 = 3000;

/// XInput's guide button bit (only XInputGetStateEx reports it).
pub const GUIDE: u16 = 0x0400;

fn os_random_16() -> [u8; 16] {
    let mut random = [0u8; 16];
    #[cfg(windows)]
    {
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn SystemFunction036(buf: *mut u8, len: u32) -> u8;
        }
        // SAFETY: RtlGenRandom (`SystemFunction036`) fills `random.len()` bytes with CSPRNG entropy.
        if unsafe { SystemFunction036(random.as_mut_ptr(), random.len() as u32) } != 0 {
            return random;
        }
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom")
            && f.read_exact(&mut random).is_ok()
        {
            return random;
        }
    }
    let _ = random;
    panic!("OS CSPRNG unavailable for pad mapping name")
}

/// Build an unguessable session-local mapping name: `Local\sir-pads-<pid>-<128-bit hex>`.
pub fn new_mapping_name(pid: u32) -> String {
    let random = os_random_16();
    let mut hex = String::with_capacity(32);
    for b in random {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    format!("{SHM_PREFIX}{pid}-{hex}")
}

/// Only accept single-segment `Local\sir-pads-<alnum/hyphen>` names (no NULs, `\`, `/`, or `..`).
pub fn valid_mapping_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(SHM_PREFIX) else {
        return false;
    };
    !rest.is_empty() && rest.len() <= 128 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[repr(C)]
pub struct Shared {
    pub magic: AtomicU32,
    pub _reserved: AtomicU32,
    /// Milliseconds since the Unix epoch, written by the app every poll.
    pub heartbeat: AtomicU64,
    pub slots: [Slot; SLOTS],
}

#[repr(C)]
pub struct Slot {
    /// Odd while the app writes the slot.
    seq: AtomicU32,
    connected: AtomicU32,
    packet: AtomicU32,
    device_id: AtomicU32,
    /// XInput button bits (plus [`GUIDE`]).
    buttons: AtomicU32,
    /// Left trigger | right trigger << 8.
    triggers: AtomicU32,
    /// X | Y << 16, XInput's sign (Y up is positive).
    left: AtomicU32,
    right: AtomicU32,
    /// Bit 0: motion available. Bits 16–19: R4, L4, R5, L5.
    motion_flags: AtomicU32,
    /// Accelerometer (m/s^2) as f32 bit patterns.
    accel: [AtomicU32; 3],
    /// Gyroscope (rad/s) as f32 bit patterns.
    gyro: [AtomicU32; 3],
    /// Low-frequency motor | high-frequency motor << 16, set by the game.
    // Upper word is the connection generation, so cached commands cannot cross hotplug.
    rumble: AtomicU64,
}

/// One pad's state, in XINPUT_GAMEPAD terms, plus optional SDL3 motion sensors.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct State {
    /// Stable host device identity, or zero for a slot-based identity.
    pub device_id: u32,
    pub packet: u32,
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub has_motion: bool,
    pub paddles: u8,
    /// Calibration (deadzones, trigger threshold, Nintendo swap, gyro scale) was already applied.
    pub calibrated: bool,
    /// Controller family code (`SLOT_FAMILY_*`).
    pub family: u8,
    /// Accelerometer (m/s^2) stored as f32 bit patterns so `State` stays `Eq`.
    pub accel_bits: [u32; 3],
    /// Gyroscope (rad/s) stored as f32 bit patterns so `State` stays `Eq`.
    pub gyro_bits: [u32; 3],
}

impl State {
    pub fn accel(&self) -> [f32; 3] {
        [f32::from_bits(self.accel_bits[0]), f32::from_bits(self.accel_bits[1]), f32::from_bits(self.accel_bits[2])]
    }
    pub fn gyro(&self) -> [f32; 3] {
        [f32::from_bits(self.gyro_bits[0]), f32::from_bits(self.gyro_bits[1]), f32::from_bits(self.gyro_bits[2])]
    }
    pub fn set_motion(&mut self, accel: [f32; 3], gyro: [f32; 3]) {
        self.has_motion = true;
        self.accel_bits = [accel[0].to_bits(), accel[1].to_bits(), accel[2].to_bits()];
        self.gyro_bits = [gyro[0].to_bits(), gyro[1].to_bits(), gyro[2].to_bits()];
    }
}

/// Calibration, deadzone, gyro, rumble, and layout preferences shared between
/// Aurora and the injected `steamclient` DLL.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PadCalibration {
    pub left_deadzone: f32,
    pub right_deadzone: f32,
    pub trigger_threshold: f32,
    pub gyro_enabled: bool,
    pub gyro_sensitivity: f32,
    pub rumble_enabled: bool,
    pub nintendo_layout: bool,
}

impl Default for PadCalibration {
    fn default() -> Self {
        Self {
            left_deadzone: 0.15,
            right_deadzone: 0.15,
            trigger_threshold: 0.10,
            gyro_enabled: true,
            gyro_sensitivity: 1.0,
            rumble_enabled: true,
            nintendo_layout: false,
        }
    }
}

impl PadCalibration {
    pub fn clamped(mut self) -> Self {
        self.left_deadzone = if self.left_deadzone.is_finite() { self.left_deadzone.clamp(0.0, 0.50) } else { 0.15 };
        self.right_deadzone = if self.right_deadzone.is_finite() { self.right_deadzone.clamp(0.0, 0.50) } else { 0.15 };
        self.trigger_threshold = if self.trigger_threshold.is_finite() { self.trigger_threshold.clamp(0.0, 0.50) } else { 0.10 };
        self.gyro_sensitivity = if self.gyro_sensitivity.is_finite() { self.gyro_sensitivity.clamp(0.1, 4.0) } else { 1.0 };
        self
    }

    pub fn encode(&self) -> String {
        let c = self.clamped();
        format!(
            "{:.3},{:.3},{:.3},{},{:.3},{},{}",
            c.left_deadzone,
            c.right_deadzone,
            c.trigger_threshold,
            u8::from(c.gyro_enabled),
            c.gyro_sensitivity,
            u8::from(c.rumble_enabled),
            u8::from(c.nintendo_layout)
        )
    }

    pub fn decode(s: &str) -> Self {
        let mut d = Self::default();
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if let Some(v) = parts.first().and_then(|x| x.parse::<f32>().ok()) {
            d.left_deadzone = v;
        }
        if let Some(v) = parts.get(1).and_then(|x| x.parse::<f32>().ok()) {
            d.right_deadzone = v;
        }
        if let Some(v) = parts.get(2).and_then(|x| x.parse::<f32>().ok()) {
            d.trigger_threshold = v;
        }
        if let Some(v) = parts.get(3).and_then(|x| x.parse::<u8>().ok()) {
            d.gyro_enabled = v != 0;
        }
        if let Some(v) = parts.get(4).and_then(|x| x.parse::<f32>().ok()) {
            d.gyro_sensitivity = v;
        }
        if let Some(v) = parts.get(5).and_then(|x| x.parse::<u8>().ok()) {
            d.rumble_enabled = v != 0;
        }
        if let Some(v) = parts.get(6).and_then(|x| x.parse::<u8>().ok()) {
            d.nintendo_layout = v != 0;
        }
        d.clamped()
    }

    pub fn apply_to_state(&self, mut s: State) -> State {
        let c = self.clamped();
        if c.nintendo_layout {
            s.buttons = swap_nintendo_buttons(s.buttons);
        }
        (s.lx, s.ly) = apply_stick_deadzone(s.lx, s.ly, c.left_deadzone);
        (s.rx, s.ry) = apply_stick_deadzone(s.rx, s.ry, c.right_deadzone);
        s.lt = apply_trigger_threshold(s.lt, c.trigger_threshold);
        s.rt = apply_trigger_threshold(s.rt, c.trigger_threshold);
        if !c.gyro_enabled {
            s.has_motion = false;
            s.accel_bits = [0; 3];
            s.gyro_bits = [0; 3];
        } else if s.has_motion && (c.gyro_sensitivity - 1.0).abs() > f32::EPSILON {
            let g = s.gyro();
            let scaled = [g[0] * c.gyro_sensitivity, g[1] * c.gyro_sensitivity, g[2] * c.gyro_sensitivity];
            s.gyro_bits = [scaled[0].to_bits(), scaled[1].to_bits(), scaled[2].to_bits()];
        }
        s.calibrated = true;
        s
    }
}

pub fn apply_stick_f32(x: f32, y: f32, deadzone: f32) -> (f32, f32) {
    let dz = deadzone.clamp(0.0, 0.95);
    let mag = (x * x + y * y).sqrt();
    if mag <= dz || mag == 0.0 {
        return (0.0, 0.0);
    }
    let scale = ((mag - dz) / (1.0 - dz)).min(1.0) / mag;
    ((x * scale).clamp(-1.0, 1.0), (y * scale).clamp(-1.0, 1.0))
}

pub fn apply_stick_deadzone(x: i16, y: i16, deadzone: f32) -> (i16, i16) {
    if deadzone <= 0.0 {
        return (x, y);
    }
    let fx = (x as f32 / 32767.0).clamp(-1.0, 1.0);
    let fy = (y as f32 / 32767.0).clamp(-1.0, 1.0);
    let (ox, oy) = apply_stick_f32(fx, fy, deadzone);
    (
        (ox * 32767.0).round().clamp(-32768.0, 32767.0) as i16,
        (oy * 32767.0).round().clamp(-32768.0, 32767.0) as i16,
    )
}

pub fn apply_trigger_f32(v: f32, threshold: f32) -> f32 {
    let th = threshold.clamp(0.0, 0.95);
    let v = v.clamp(0.0, 1.0);
    if v <= th { 0.0 } else { ((v - th) / (1.0 - th)).clamp(0.0, 1.0) }
}

pub fn apply_trigger_threshold(v: u8, threshold: f32) -> u8 {
    if threshold <= 0.0 {
        return v;
    }
    let f = apply_trigger_f32(v as f32 / 255.0, threshold);
    (f * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Swap A<->B (`0x1000` <-> `0x2000`) and X<->Y (`0x4000` <-> `0x8000`).
pub fn swap_nintendo_buttons(buttons: u16) -> u16 {
    const A: u16 = 0x1000;
    const B: u16 = 0x2000;
    const X: u16 = 0x4000;
    const Y: u16 = 0x8000;
    let mut out = buttons & !(A | B | X | Y);
    if buttons & A != 0 { out |= B; }
    if buttons & B != 0 { out |= A; }
    if buttons & X != 0 { out |= Y; }
    if buttons & Y != 0 { out |= X; }
    out
}

pub fn family_allowed(mask: u32, slot_family: u8) -> bool {
    let bit = match slot_family {
        SLOT_FAMILY_XBOX => FAMILY_XBOX,
        SLOT_FAMILY_PLAYSTATION => FAMILY_PLAYSTATION,
        SLOT_FAMILY_SWITCH_PRO => FAMILY_SWITCH_PRO,
        _ => FAMILY_GENERIC,
    };
    mask & bit != 0
}

pub fn parse_family_mask(raw: Option<&str>) -> u32 {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return FAMILY_ALL;
    };
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).unwrap_or(FAMILY_ALL)
    } else {
        s.parse::<u32>().unwrap_or(FAMILY_ALL)
    }
}

pub fn steam_input_env_enabled(raw: Option<&str>) -> bool {
    !matches!(raw.map(str::trim), Some("0" | "false" | "FALSE" | "off" | "OFF"))
}

fn pair(a: i16, b: i16) -> u32 {
    a as u16 as u32 | (b as u16 as u32) << 16
}

impl Slot {
    /// App side: publish a pad (or its absence).
    pub fn write(&self, s: Option<State>) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1) | 1, Ordering::Relaxed);
        fence(Ordering::Release);
        let s0 = s.unwrap_or_default();
        let was_connected = self.connected.load(Ordering::Relaxed) != 0;
        if was_connected != s.is_some() || self.device_id.load(Ordering::Relaxed) != s0.device_id {
            let generation = self.rumble_generation().wrapping_add(1);
            self.rumble.store(u64::from(generation) << 32, Ordering::Release);
        }
        self.device_id.store(s0.device_id, Ordering::Relaxed);
        self.connected.store(s.is_some() as u32, Ordering::Relaxed);
        self.packet.store(s0.packet, Ordering::Relaxed);
        self.buttons.store(s0.buttons as u32, Ordering::Relaxed);
        self.triggers.store(s0.lt as u32 | (s0.rt as u32) << 8, Ordering::Relaxed);
        self.left.store(pair(s0.lx, s0.ly), Ordering::Relaxed);
        self.right.store(pair(s0.rx, s0.ry), Ordering::Relaxed);
        let flags = (s0.has_motion as u32) | ((s0.calibrated as u32) << 1) | ((s0.family as u32) << 8) | ((u32::from(s0.paddles) & 15) << 16);
        self.motion_flags.store(flags, Ordering::Relaxed);
        for i in 0..3 {
            self.accel[i].store(s0.accel_bits[i], Ordering::Relaxed);
            self.gyro[i].store(s0.gyro_bits[i], Ordering::Relaxed);
        }
        self.seq.store((seq | 1).wrapping_add(1), Ordering::Release);
    }

    /// Game side: the pad in this slot, read whole.
    pub fn read(&self) -> Option<State> {
        for _ in 0..64 {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let connected = self.connected.load(Ordering::Relaxed) != 0;
            let (t, l, r) = (self.triggers.load(Ordering::Relaxed), self.left.load(Ordering::Relaxed), self.right.load(Ordering::Relaxed));
            let flags = self.motion_flags.load(Ordering::Relaxed);
            let s = State {
                device_id: self.device_id.load(Ordering::Relaxed),
                packet: self.packet.load(Ordering::Relaxed),
                buttons: self.buttons.load(Ordering::Relaxed) as u16,
                lt: t as u8,
                rt: (t >> 8) as u8,
                lx: l as u16 as i16,
                ly: (l >> 16) as u16 as i16,
                rx: r as u16 as i16,
                ry: (r >> 16) as u16 as i16,
                has_motion: flags & 1 != 0,
                paddles: ((flags >> 16) & 15) as u8,
                calibrated: flags & 2 != 0,
                family: ((flags >> 8) & 0xFF) as u8,
                accel_bits: [
                    self.accel[0].load(Ordering::Relaxed),
                    self.accel[1].load(Ordering::Relaxed),
                    self.accel[2].load(Ordering::Relaxed),
                ],
                gyro_bits: [
                    self.gyro[0].load(Ordering::Relaxed),
                    self.gyro[1].load(Ordering::Relaxed),
                    self.gyro[2].load(Ordering::Relaxed),
                ],
            };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return connected.then_some(s);
            }
        }
        None
    }

    pub fn rumble_generation(&self) -> u32 {
        (self.rumble.load(Ordering::Acquire) >> 32) as u32
    }

    pub fn set_rumble_for(&self, generation: u32, low: u16, high: u16) {
        let motors = u64::from(low) | (u64::from(high) << 16);
        let _ = self.rumble.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            ((current >> 32) as u32 == generation).then_some((u64::from(generation) << 32) | motors)
        });
    }

    pub fn set_rumble(&self, low: u16, high: u16) {
        self.set_rumble_for(self.rumble_generation(), low, high);
    }

    pub fn rumble(&self) -> (u16, u16) {
        let v = self.rumble.load(Ordering::Relaxed);
        (v as u16, (v >> 16) as u16)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Shared {
    /// The pads, if the app is still publishing them.
    pub fn live(&self) -> bool {
        self.magic.load(Ordering::Acquire) == MAGIC && now_ms().saturating_sub(self.heartbeat.load(Ordering::Relaxed)) < STALE_MS
    }
}

/// Which shared pad sits in each of the four XInput slots: the app's pads
/// fill the slots real XInput pads leave free, in order. `real[i]`: a real
/// pad holds slot i; `shared`: the shared slots that have a pad.
pub fn seats(real: [bool; SLOTS], shared: &[usize]) -> [Option<usize>; SLOTS] {
    let mut out = [None; SLOTS];
    let mut next = shared.iter().copied();
    for (i, seat) in out.iter_mut().enumerate() {
        if !real[i] {
            *seat = next.next();
        }
    }
    out
}

#[cfg(windows)]
mod win_shm {
    use super::{MAGIC, Shared, new_mapping_name, valid_mapping_name};
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(file: isize, attrs: *const c_void, protect: u32, size_hi: u32, size_lo: u32, name: *const u16) -> *mut c_void;
        fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
        fn MapViewOfFile(map: *mut c_void, access: u32, off_hi: u32, off_lo: u32, bytes: usize) -> *mut c_void;
        fn UnmapViewOfFile(view: *const c_void) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
        fn GetLastError() -> u32;
        fn SetLastError(err: u32);
    }

    const PAGE_READWRITE: u32 = 0x04;
    const FILE_MAP_WRITE: u32 = 0x02;
    const FILE_MAP_READ: u32 = 0x04;
    const FILE_MAP_ALL_ACCESS: u32 = 0xF001F;
    const ERROR_ALREADY_EXISTS: u32 = 183;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    pub struct Shm {
        handle: *mut c_void,
        view: *mut Shared,
        name: String,
    }

    unsafe impl Send for Shm {}
    unsafe impl Sync for Shm {}

    impl Shm {
        pub fn create() -> Option<Self> {
            let name = new_mapping_name(std::process::id());
            Self::create_named(&name)
        }

        pub fn create_named(name: &str) -> Option<Self> {
            if !valid_mapping_name(name) {
                return None;
            }
            let wide_name = wide(name);
            let size = std::mem::size_of::<Shared>();
            let (h, err) = unsafe {
                SetLastError(0);
                let h = CreateFileMappingW(-1, std::ptr::null(), PAGE_READWRITE, 0, size as u32, wide_name.as_ptr());
                let err = GetLastError();
                (h, err)
            };
            if h.is_null() {
                return None;
            }
            if err == ERROR_ALREADY_EXISTS {
                unsafe { CloseHandle(h) };
                return None;
            }
            let view = unsafe { MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, size) } as *mut Shared;
            if view.is_null() {
                unsafe { CloseHandle(h) };
                return None;
            }
            let shared = unsafe { &*view };
            shared.magic.store(MAGIC, Ordering::Release);
            Some(Self { handle: h, view, name: name.to_owned() })
        }

        pub fn shared(&self) -> &Shared {
            unsafe { &*self.view }
        }

        pub fn name(&self) -> &str {
            &self.name
        }
    }

    impl Drop for Shm {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(self.view.cast());
                CloseHandle(self.handle);
            }
        }
    }

    #[derive(Clone, Copy)]
    pub struct MappedShm {
        pub shared: &'static Shared,
        pub writable: bool,
    }

    pub fn open_shm_with_error(name: &str) -> Result<MappedShm, u32> {
        if !valid_mapping_name(name) {
            return Err(87); // ERROR_INVALID_PARAMETER
        }
        let wide_name = wide(name);
        let mut last_err = 0u32;
        for (access, writable) in [(FILE_MAP_READ | FILE_MAP_WRITE, true), (FILE_MAP_READ, false)] {
            let h = unsafe { OpenFileMappingW(access, 0, wide_name.as_ptr()) };
            if h.is_null() {
                last_err = unsafe { GetLastError() };
                continue;
            }
            let view = unsafe { MapViewOfFile(h, access, 0, 0, std::mem::size_of::<Shared>()) } as *const Shared;
            if view.is_null() {
                last_err = unsafe { GetLastError() };
                unsafe { CloseHandle(h) };
                continue;
            }
            // The mapped view keeps its own kernel reference to the section object.
            unsafe { CloseHandle(h) };
            let shared = unsafe { view.as_ref() }.ok_or(last_err)?;
            return Ok(MappedShm { shared, writable });
        }
        Err(last_err)
    }

    pub fn open_shm(name: &str) -> Option<&'static Shared> {
        open_shm_with_error(name).ok().map(|m| m.shared)
    }
}

#[cfg(windows)]
pub use win_shm::{MappedShm, Shm, open_shm, open_shm_with_error};

#[cfg(not(windows))]
#[derive(Clone, Copy)]
pub struct MappedShm {
    pub shared: &'static Shared,
    pub writable: bool,
}

#[cfg(not(windows))]
pub fn open_shm(_name: &str) -> Option<&'static Shared> {
    None
}

#[cfg(not(windows))]
pub fn open_shm_with_error(_name: &str) -> Result<MappedShm, u32> {
    Err(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_round_trip() {
        let slot: Slot = unsafe { std::mem::zeroed() };
        assert_eq!(slot.read(), None);
        let s = State { device_id: 73, packet: 7, buttons: 0x1000 | GUIDE, lt: 255, rt: 1, lx: -32768, ly: 32767, rx: -1, ry: 0, ..Default::default() };
        slot.write(Some(s));
        assert_eq!(slot.read(), Some(s));
        let original = slot.rumble_generation();
        slot.set_rumble(40, 80);
        slot.write(Some(s));
        assert_eq!(slot.rumble(), (40, 80));
        slot.write(Some(State { device_id: 2, ..s }));
        assert_eq!(slot.rumble(), (0, 0));
        slot.set_rumble_for(original, 50, 90);
        assert_eq!(slot.rumble(), (0, 0));
        slot.set_rumble(10, 20);
        slot.write(None);
        assert_eq!(slot.rumble(), (0, 0));
        assert_eq!(slot.read(), None);
        slot.set_rumble(0xFFFF, 0x1234);
        assert_eq!(slot.rumble(), (0xFFFF, 0x1234));
    }

    #[test]
    fn shared_pads_fill_free_slots() {
        assert_eq!(seats([true, false, false, false], &[2]), [None, Some(2), None, None]);
        assert_eq!(seats([false, true, false, true], &[0, 1, 3]), [Some(0), None, Some(1), None]);
        assert_eq!(seats([true; 4], &[0]), [None; 4]);
    }

    #[test]
    fn mapping_name_has_128_bit_random_suffix_and_strict_prefix_validation() {
        let n1 = new_mapping_name(4242);
        let n2 = new_mapping_name(4242);
        assert_ne!(n1, n2, "random 128-bit suffix must differ across calls");
        let suffix = n1.strip_prefix("Local\\sir-pads-4242-").expect("expected Local\\sir-pads-<pid>-<hex>");
        assert_eq!(suffix.len(), 32);
        assert!(suffix.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert!(valid_mapping_name(&n1));
        assert!(valid_mapping_name("Local\\sir-pads-test-1234"));

        for bad in [
            "",
            "Local\\sir-pads-",
            "Global\\sir-pads-4242-0123456789abcdef0123456789abcdef",
            "sir-pads-4242-0123456789abcdef0123456789abcdef",
            "Local\\evil-pads-4242",
            "Local\\sir-pads-4242\\nested",
            "Local\\sir-pads-..\\escape",
            "Local\\sir-pads-4242\0extra",
            "Local\\sir-pads-4242 space",
        ] {
            assert!(!valid_mapping_name(bad), "expected {bad:?} to be rejected");
        }
    }

    #[cfg(windows)]
    #[test]
    fn shm_create_fails_when_pre_squatted_and_open_shm_validates_prefix() {
        use std::ffi::c_void;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateFileMappingW(file: isize, attrs: *const c_void, protect: u32, size_hi: u32, size_lo: u32, name: *const u16) -> *mut c_void;
            fn MapViewOfFile(map: *mut c_void, access: u32, off_hi: u32, off_lo: u32, bytes: usize) -> *mut c_void;
            fn UnmapViewOfFile(view: *const c_void) -> i32;
            fn CloseHandle(h: *mut c_void) -> i32;
        }

        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain([0]).collect() };
        let squat_name = format!("Local\\sir-pads-squat-{}", std::process::id());
        let size = std::mem::size_of::<Shared>();
        let squatter = unsafe { CreateFileMappingW(-1, std::ptr::null(), 0x04, 0, size as u32, wide(&squat_name).as_ptr()) };
        assert!(!squatter.is_null());
        let squatter_view = unsafe { MapViewOfFile(squatter, 0xF001F, 0, 0, size) } as *mut Shared;
        assert!(!squatter_view.is_null());
        unsafe { (*squatter_view).magic.store(0xDEAD_BEEF, Ordering::SeqCst) };

        // Pre-squatted mapping must fail closed and leave squatter's mapping untouched.
        assert!(Shm::create_named(&squat_name).is_none());
        assert_eq!(unsafe { (*squatter_view).magic.load(Ordering::SeqCst) }, 0xDEAD_BEEF);

        unsafe {
            UnmapViewOfFile(squatter_view.cast());
            CloseHandle(squatter);
        }

        // Once released, creating the mapping succeeds cleanly.
        let shm = Shm::create_named(&squat_name).expect("fresh mapping succeeds");
        assert_eq!(shm.shared().magic.load(Ordering::Acquire), MAGIC);

        // Random Shm::create() round-trips through open_shm.
        let rand_shm = Shm::create().expect("random Shm::create");
        assert!(valid_mapping_name(rand_shm.name()));
        let opened = open_shm_with_error(rand_shm.name()).expect("open_shm accepts valid Local\\sir-pads- name");
        assert!(opened.writable);
        assert_eq!(opened.shared.magic.load(Ordering::Acquire), MAGIC);

        assert!(open_shm("Global\\sir-pads-1234").is_none());
        assert!(open_shm("Local\\other-1234").is_none());
    }

    #[test]
    fn slot_motion_round_trip() {
        let slot: Slot = unsafe { std::mem::zeroed() };
        let mut s = State { packet: 7, buttons: 0x1000, ..Default::default() };
        s.set_motion([0.0, 9.80665, 0.5], [0.25, -0.5, 1.0]);
        slot.write(Some(s));
        let got = slot.read().expect("slot should be connected");
        assert!(got.has_motion);
        assert_eq!(got.accel(), [0.0, 9.80665, 0.5]);
        assert_eq!(got.gyro(), [0.25, -0.5, 1.0]);
    }

    #[test]
    fn calibration_round_trip_and_apply() {
        let cal = PadCalibration {
            left_deadzone: 0.25,
            right_deadzone: 0.10,
            trigger_threshold: 0.20,
            gyro_enabled: false,
            gyro_sensitivity: 2.5,
            rumble_enabled: false,
            nintendo_layout: true,
        };
        let encoded = cal.encode();
        assert_eq!(PadCalibration::decode(&encoded), cal);
        // Invalid values clamp safely.
        let clamped = PadCalibration::decode("0.99,-0.5,nan,0,9.0,0,1");
        assert_eq!(clamped.left_deadzone, 0.50);
        assert_eq!(clamped.right_deadzone, 0.0);
        assert_eq!(clamped.gyro_sensitivity, 4.0);
        assert!(!clamped.gyro_enabled);
        assert!(clamped.nintendo_layout);

        // Apply swaps A/B and X/Y, zeros inside deadzone/trigger threshold, strips motion when disabled.
        let mut raw = State {
            buttons: 0x1000 | 0x4000 | 0x0010, // A | X | START
            lx: 4000, // ~0.122 < 0.25 deadzone -> 0
            ly: 0,
            rx: 32767,
            ry: 0,
            lt: 40, // ~0.157 < 0.20 threshold -> 0
            rt: 255,
            family: SLOT_FAMILY_SWITCH_PRO,
            ..Default::default()
        };
        raw.set_motion([0.0, 9.8, 0.0], [1.0, 2.0, 3.0]);
        let applied = cal.apply_to_state(raw);
        assert!(applied.calibrated);
        assert_eq!(applied.family, SLOT_FAMILY_SWITCH_PRO);
        assert_eq!(applied.buttons, 0x2000 | 0x8000 | 0x0010); // B | Y | START
        assert_eq!((applied.lx, applied.ly), (0, 0));
        assert_eq!(applied.rx, 32767);
        assert_eq!(applied.lt, 0);
        assert_eq!(applied.rt, 255);
        assert!(!applied.has_motion);
    }

    #[test]
    fn family_mask_and_env_helpers() {
        assert!(steam_input_env_enabled(None));
        assert!(steam_input_env_enabled(Some("1")));
        assert!(!steam_input_env_enabled(Some("0")));
        assert!(!steam_input_env_enabled(Some("false")));
        assert_eq!(parse_family_mask(None), FAMILY_ALL);
        assert_eq!(parse_family_mask(Some("0x6")), FAMILY_PLAYSTATION | FAMILY_SWITCH_PRO);
        assert_eq!(parse_family_mask(Some("5")), FAMILY_XBOX | FAMILY_SWITCH_PRO);
        assert!(family_allowed(FAMILY_PLAYSTATION, SLOT_FAMILY_PLAYSTATION));
        assert!(!family_allowed(FAMILY_PLAYSTATION, SLOT_FAMILY_XBOX));
        assert!(!family_allowed(FAMILY_PLAYSTATION, SLOT_FAMILY_SWITCH_PRO));
        assert!(family_allowed(FAMILY_GENERIC, SLOT_FAMILY_UNKNOWN));
    }
}
