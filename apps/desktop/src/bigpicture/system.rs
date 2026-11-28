//! Cached host system status and serialized non-blocking controls for Big Picture /
//! Steam Deck Game Mode: local clock, battery, Linux audio volume/mute, backlight
//! brightness, and session info. All device writes are serialized and coalesced on a
//! single background worker so rapid controller steps never land out of order.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

static RUNTIME_TOUCHSCREEN: AtomicBool = AtomicBool::new(false);

struct HostHardwareInfo {
    session_label: String,
    is_steam_deck: bool,
    has_touchscreen: bool,
    display_ppi: Option<f32>,
}

fn host_hardware_info() -> &'static HostHardwareInfo {
    static INFO: OnceLock<HostHardwareInfo> = OnceLock::new();
    INFO.get_or_init(|| {
        let is_steam_deck = detect_steam_deck();
        let has_touchscreen = detect_touchscreen(is_steam_deck);
        let display_ppi = detect_display_ppi(is_steam_deck);
        let session_label = detect_session_label();
        HostHardwareInfo { session_label, is_steam_deck, has_touchscreen, display_ppi }
    })
}

/// Read-only snapshot rendered by the top bar, Quick Access drawer, and System settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemStatus {
    /// Local wall-clock time formatted as `HH:MM`.
    pub clock: String,
    /// Seconds until the next minute boundary (for idle repaint scheduling).
    pub secs_until_next_minute: u64,
    /// Battery charge 0..=100 if a battery supply is present.
    pub battery_percent: Option<u8>,
    /// True when plugged in / charging / full on AC.
    pub battery_charging: bool,
    /// Master output volume 0..=100 if readable.
    pub volume_percent: Option<u8>,
    /// Master output mute state.
    pub volume_muted: bool,
    /// Display backlight brightness 0..=100 if readable.
    pub brightness_percent: Option<u8>,
    /// Whether the current host exposes writable backlight controls.
    pub can_set_brightness: bool,
    /// Whether the current host exposes volume controls.
    pub can_set_volume: bool,
    /// Short display/session description (e.g. "Gamescope", "Wayland", "X11", "Windows").
    pub session_label: String,
    /// True when running on Steam Deck hardware or a SteamOS/Gamescope handheld session.
    pub is_steam_deck: bool,
    /// True when a direct touchscreen is detected (or touch input has been observed).
    pub has_touchscreen: bool,
    /// Estimated physical display density in pixels per inch (from EDID or Deck hardware).
    pub display_ppi: Option<f32>,
}

#[derive(Default)]
struct SharedAudio {
    volume_percent: Option<u8>,
    volume_muted: bool,
    can_control: bool,
    last_refresh: Option<Instant>,
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct PendingControls {
    refresh_audio: bool,
    set_volume: Option<u8>,
    set_mute: Option<bool>,
    set_brightness: Option<(Option<std::path::PathBuf>, u8)>,
    ctx: Option<egui::Context>,
    shutdown: bool,
}

#[cfg(target_os = "linux")]
struct ControlWorker {
    queue: Arc<(Mutex<PendingControls>, Condvar)>,
}

#[cfg(target_os = "linux")]
impl ControlWorker {
    fn spawn(audio: Arc<Mutex<SharedAudio>>) -> Self {
        let queue = Arc::new((Mutex::new(PendingControls::default()), Condvar::new()));
        let worker_q = Arc::clone(&queue);
        let _ = std::thread::Builder::new()
            .name("bp-sys-ctrl".into())
            .spawn(move || loop {
                let (refresh, vol, mute, bright, ctx) = {
                    let (lock, cv) = &*worker_q;
                    let mut st = lock.lock().unwrap();
                    while !st.shutdown
                        && !st.refresh_audio
                        && st.set_volume.is_none()
                        && st.set_mute.is_none()
                        && st.set_brightness.is_none()
                    {
                        st = cv.wait(st).unwrap();
                    }
                    if st.shutdown {
                        return;
                    }
                    let refresh = std::mem::take(&mut st.refresh_audio);
                    let vol = st.set_volume.take();
                    let mute = st.set_mute.take();
                    let bright = st.set_brightness.take();
                    let ctx = st.ctx.take();
                    (refresh, vol, mute, bright, ctx)
                };

                if let Some((dir, pct)) = bright {
                    linux_set_brightness(dir.as_deref(), pct);
                }
                if let Some(pct) = vol {
                    linux_set_volume(pct);
                }
                if let Some(muted) = mute {
                    linux_set_mute(muted);
                }
                if refresh || vol.is_some() || mute.is_some() {
                    let result = linux_read_volume();
                    // Only overwrite displayed audio state if no newer write was queued while we ran.
                    let newer_pending = {
                        let st = worker_q.0.lock().unwrap();
                        st.set_volume.is_some() || st.set_mute.is_some()
                    };
                    if !newer_pending
                        && let Ok(mut shared) = audio.lock()
                    {
                        match result {
                            Some((v, m)) => {
                                shared.volume_percent = Some(v);
                                shared.volume_muted = m;
                                shared.can_control = true;
                            }
                            None => {
                                shared.can_control = false;
                            }
                        }
                        shared.last_refresh = Some(Instant::now());
                    }
                }
                if let Some(c) = ctx {
                    c.request_repaint_of(egui::ViewportId::ROOT);
                }
            });
        Self { queue }
    }

    fn update(&self, f: impl FnOnce(&mut PendingControls)) {
        let (lock, cv) = &*self.queue;
        let mut st = lock.lock().unwrap();
        f(&mut st);
        cv.notify_one();
    }
}

#[cfg(target_os = "linux")]
impl Drop for ControlWorker {
    fn drop(&mut self) {
        self.update(|st| st.shutdown = true);
    }
}

/// Manages TTL-cached system readings and serialized non-blocking audio/brightness adjustments.
pub struct SystemMonitor {
    last_sysfs_poll: Option<Instant>,
    battery_percent: Option<u8>,
    battery_charging: bool,
    brightness_percent: Option<u8>,
    can_set_brightness: bool,
    #[cfg(target_os = "linux")]
    backlight_dir: Option<std::path::PathBuf>,
    audio: Arc<Mutex<SharedAudio>>,
    #[cfg(target_os = "linux")]
    worker: Option<ControlWorker>,
    session_label: String,
    is_steam_deck: bool,
    has_touchscreen: bool,
    display_ppi: Option<f32>,
}

impl Default for SystemMonitor {
    fn default() -> Self {
        let host = host_hardware_info();
        Self {
            last_sysfs_poll: None,
            battery_percent: None,
            battery_charging: false,
            brightness_percent: None,
            can_set_brightness: false,
            #[cfg(target_os = "linux")]
            backlight_dir: None,
            audio: Arc::new(Mutex::new(SharedAudio::default())),
            #[cfg(target_os = "linux")]
            worker: None,
            session_label: host.session_label.clone(),
            is_steam_deck: host.is_steam_deck,
            has_touchscreen: host.has_touchscreen,
            display_ppi: host.display_ppi,
        }
    }
}

impl SystemMonitor {
    pub fn is_steam_deck(&self) -> bool {
        self.is_steam_deck
    }

    pub fn has_touchscreen(&self) -> bool {
        self.has_touchscreen || RUNTIME_TOUCHSCREEN.load(Ordering::Relaxed)
    }

    pub fn display_ppi(&self) -> Option<f32> {
        self.display_ppi
    }

    /// Latch touchscreen presence once a real touch event is observed at runtime
    /// (covers sandboxed environments where `/sys/class/input` is hidden).
    pub fn mark_touchscreen(&mut self) {
        self.has_touchscreen = true;
        RUNTIME_TOUCHSCREEN.store(true, Ordering::Relaxed);
    }

    #[cfg(target_os = "linux")]
    fn worker(&mut self) -> &ControlWorker {
        let audio = Arc::clone(&self.audio);
        self.worker.get_or_insert_with(|| ControlWorker::spawn(audio))
    }

    /// Return the current cached status, refreshing lightweight sysfs files at most every 15s
    /// and kicking a coalesced async audio probe when `want_audio` is true and stale (>4s).
    pub fn snapshot(&mut self, want_audio: bool, ctx: Option<&egui::Context>) -> SystemStatus {
        let now = Instant::now();
        if self.last_sysfs_poll.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(15)) {
            self.last_sysfs_poll = Some(now);
            self.refresh_sysfs();
        }
        if want_audio {
            self.maybe_refresh_audio(ctx);
        }
        let (clock, secs_until_next_minute) = local_clock_hm();
        let audio = self.audio.lock().unwrap();
        SystemStatus {
            clock,
            secs_until_next_minute,
            battery_percent: self.battery_percent,
            battery_charging: self.battery_charging,
            volume_percent: audio.volume_percent,
            volume_muted: audio.volume_muted,
            brightness_percent: self.brightness_percent,
            can_set_brightness: self.can_set_brightness,
            can_set_volume: audio.can_control,
            session_label: self.session_label.clone(),
            is_steam_deck: self.is_steam_deck,
            has_touchscreen: self.has_touchscreen(),
            display_ppi: self.display_ppi,
        }
    }

    /// Force a sysfs + audio refresh (e.g. when Quick Access opens).
    pub fn refresh_now(&mut self, ctx: Option<&egui::Context>) {
        self.last_sysfs_poll = Some(Instant::now());
        self.refresh_sysfs();
        if let Ok(mut audio) = self.audio.lock() {
            audio.last_refresh = None;
        }
        self.maybe_refresh_audio(ctx);
    }

    /// Adjust system master volume to `percent` (0..=100), coalesced on the control worker.
    pub fn set_volume(&mut self, percent: u8, ctx: Option<egui::Context>) {
        let clamped = percent.min(100);
        if let Ok(mut audio) = self.audio.lock() {
            audio.volume_percent = Some(clamped);
            if clamped > 0 {
                audio.volume_muted = false;
            }
        }
        #[cfg(target_os = "linux")]
        {
            self.worker().update(|st| {
                st.set_volume = Some(clamped);
                if clamped > 0 {
                    st.set_mute = Some(false);
                }
                if let Some(c) = ctx {
                    st.ctx = Some(c);
                }
            });
        }
        #[cfg(not(target_os = "linux"))]
        let _ = ctx;
    }

    /// Toggle system master mute, coalesced on the control worker.
    pub fn toggle_mute(&mut self, ctx: Option<egui::Context>) {
        let target_muted = if let Ok(mut audio) = self.audio.lock() {
            audio.volume_muted = !audio.volume_muted;
            audio.volume_muted
        } else {
            return;
        };
        #[cfg(target_os = "linux")]
        {
            self.worker().update(|st| {
                st.set_mute = Some(target_muted);
                if let Some(c) = ctx {
                    st.ctx = Some(c);
                }
            });
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (target_muted, ctx);
        }
    }

    /// Adjust display backlight brightness (5..=100), coalesced on the control worker.
    pub fn set_brightness(&mut self, percent: u8, ctx: Option<egui::Context>) {
        let clamped = percent.clamp(5, 100);
        self.brightness_percent = Some(clamped);
        #[cfg(target_os = "linux")]
        {
            let dir = self.backlight_dir.clone();
            self.worker().update(|st| {
                st.set_brightness = Some((dir, clamped));
                if let Some(c) = ctx {
                    st.ctx = Some(c);
                }
            });
        }
        #[cfg(not(target_os = "linux"))]
        let _ = ctx;
    }

    fn refresh_sysfs(&mut self) {
        #[cfg(target_os = "linux")]
        {
            if let Some((pct, charging)) = linux_read_battery() {
                self.battery_percent = Some(pct);
                self.battery_charging = charging;
            } else {
                self.battery_percent = None;
                self.battery_charging = false;
            }
            if let Some((dir, pct, writable)) = linux_read_backlight() {
                self.backlight_dir = Some(dir);
                self.brightness_percent = Some(pct);
                self.can_set_brightness = writable;
            } else {
                self.backlight_dir = None;
                self.brightness_percent = None;
                self.can_set_brightness = false;
            }
        }
    }

    fn maybe_refresh_audio(&mut self, ctx: Option<&egui::Context>) {
        #[cfg(target_os = "linux")]
        {
            let stale = self
                .audio
                .lock()
                .ok()
                .is_none_or(|a| a.last_refresh.is_none_or(|t| t.elapsed() >= Duration::from_secs(4)));
            if stale {
                let ctx = ctx.cloned();
                self.worker().update(|st| {
                    st.refresh_audio = true;
                    if let Some(c) = ctx {
                        st.ctx = Some(c);
                    }
                });
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = ctx;
    }
}

fn detect_session_label() -> String {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_CURRENT_DESKTOP")
                .is_ok_and(|v| v.to_ascii_lowercase().contains("gamescope"))
        {
            return "Gamescope".into();
        }
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return "Wayland".into();
        }
        if std::env::var_os("DISPLAY").is_some() {
            return "X11".into();
        }
        "Linux".into()
    }
    #[cfg(windows)]
    {
        "Windows".into()
    }
    #[cfg(target_os = "macos")]
    {
        "macOS".into()
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        std::env::consts::OS.into()
    }
}

/// Read a small UTF-8 file (capped to avoid unbounded reads from unexpected files).
#[cfg(target_os = "linux")]
fn read_small_text(path: &std::path::Path, max_bytes: usize) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(max_bytes.min(256));
    f.take(max_bytes as u64).read_to_end(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

/// Read a small binary file (capped to `max_bytes`).
#[cfg(target_os = "linux")]
fn read_small_bytes(path: &std::path::Path, max_bytes: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(max_bytes.min(512));
    f.take(max_bytes as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn parse_env_bool(key: &str) -> Option<bool> {
    let val = std::env::var(key).ok()?;
    match val.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Pure Steam Deck / handheld session detector for deterministic unit testing.
pub(crate) fn detect_steam_deck_from(
    env_override: Option<bool>,
    dmi_vendor: Option<&str>,
    dmi_product: Option<&str>,
    dmi_board: Option<&str>,
    dmi_family: Option<&str>,
    gamescope_session: bool,
    os_release: Option<&str>,
) -> bool {
    if let Some(forced) = env_override {
        return forced;
    }
    let vendor_is_valve = dmi_vendor.is_some_and(|v| v.trim().to_ascii_lowercase().contains("valve"));
    if vendor_is_valve {
        let is_deck_model = [dmi_product, dmi_board]
            .into_iter()
            .flatten()
            .any(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "jupiter" | "galileo"));
        let is_deck_family = dmi_family.is_some_and(|f| {
            let low = f.trim().to_ascii_lowercase();
            low.contains("steam deck") || low.contains("aerith") || low.contains("sephiroth")
        });
        if is_deck_model || is_deck_family {
            return true;
        }
    }
    if gamescope_session {
        return true;
    }
    if let Some(text) = os_release {
        for line in text.lines() {
            let line = line.trim();
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if (k == "ID" && v.eq_ignore_ascii_case("steamos"))
                    || (k == "VARIANT_ID" && v.eq_ignore_ascii_case("steamdeck"))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Detect whether Aurora is running on Steam Deck hardware or a SteamOS/Gamescope session.
/// Works in both Game Mode (Gamescope) and Desktop Mode (KDE Plasma), including inside Flatpak.
pub fn detect_steam_deck() -> bool {
    let env_override = parse_env_bool("AURORA_STEAM_DECK")
        .or_else(|| parse_env_bool("STEAM_DECK"))
        .or_else(|| parse_env_bool("SteamDeck"));
    #[cfg(target_os = "linux")]
    {
        let dmi = std::path::Path::new("/sys/devices/virtual/dmi/id");
        let vendor = read_small_text(&dmi.join("sys_vendor"), 256);
        let product = read_small_text(&dmi.join("product_name"), 256);
        let board = read_small_text(&dmi.join("board_name"), 256);
        let family = read_small_text(&dmi.join("product_family"), 256);
        let gamescope = std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_CURRENT_DESKTOP")
                .is_ok_and(|v| v.to_ascii_lowercase().contains("gamescope"));
        let os_release = read_small_text(std::path::Path::new("/run/host/os-release"), 8192)
            .or_else(|| read_small_text(std::path::Path::new("/etc/os-release"), 8192));
        detect_steam_deck_from(
            env_override,
            vendor.as_deref(),
            product.as_deref(),
            board.as_deref(),
            family.as_deref(),
            gamescope,
            os_release.as_deref(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        detect_steam_deck_from(env_override, None, None, None, None, false, None)
    }
}

/// Test whether `bit` is set in a Linux sysfs space-separated hex bitmask
/// (MSB word first, `word_bits` per word: 64 on 64-bit kernels, 32 on 32-bit kernels).
#[cfg(any(target_os = "linux", test))]
pub(crate) fn hex_bitmask_has_bit(mask: &str, bit: usize, word_bits: usize) -> bool {
    if word_bits == 0 {
        return false;
    }
    let words: Vec<&str> = mask.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }
    let word_idx_from_lsb = bit / word_bits;
    if word_idx_from_lsb >= words.len() {
        return false;
    }
    let word_str = words[words.len() - 1 - word_idx_from_lsb];
    let Ok(val) = u64::from_str_radix(word_str, 16) else {
        return false;
    };
    let shift = bit % word_bits;
    shift < 64 && ((val >> shift) & 1) != 0
}

/// Determine whether a Linux `/sys/class/input/inputN` device is a direct touchscreen
/// (excluding indirect trackpads and pen-only graphics tablets).
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_input_has_touchscreen(
    properties_hex: &str,
    ev_hex: &str,
    abs_hex: &str,
    key_hex: &str,
    word_bits: usize,
) -> bool {
    const INPUT_PROP_DIRECT: usize = 1;
    const EV_ABS: usize = 3;
    const ABS_MT_POSITION_X: usize = 0x35;
    const BTN_TOOL_PEN: usize = 0x140;
    const BTN_TOUCH: usize = 0x14a;

    if !hex_bitmask_has_bit(properties_hex, INPUT_PROP_DIRECT, word_bits)
        || !hex_bitmask_has_bit(ev_hex, EV_ABS, word_bits)
    {
        return false;
    }
    let has_mt = hex_bitmask_has_bit(abs_hex, ABS_MT_POSITION_X, word_bits);
    let has_touch = hex_bitmask_has_bit(key_hex, BTN_TOUCH, word_bits);
    let is_pen = hex_bitmask_has_bit(key_hex, BTN_TOOL_PEN, word_bits);
    has_mt || (has_touch && !is_pen)
}

/// Detect whether a direct touchscreen is attached (or Steam Deck hardware is present).
pub fn detect_touchscreen(is_steam_deck: bool) -> bool {
    if let Some(forced) = parse_env_bool("AURORA_TOUCHSCREEN") {
        return forced;
    }
    if is_steam_deck {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        let word_bits = usize::BITS as usize;
        if let Ok(entries) = std::fs::read_dir("/sys/class/input") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(s) = name.to_str() else { continue };
                if !s.starts_with("input") || !s[5..].bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                let dir = entry.path();
                let props = read_small_text(&dir.join("properties"), 256).unwrap_or_default();
                let ev = read_small_text(&dir.join("capabilities/ev"), 256).unwrap_or_default();
                let abs = read_small_text(&dir.join("capabilities/abs"), 512).unwrap_or_default();
                let key = read_small_text(&dir.join("capabilities/key"), 1024).unwrap_or_default();
                if parse_input_has_touchscreen(&props, &ev, &abs, &key, word_bits) {
                    return true;
                }
            }
        }
    }
    false
}

/// Parse physical display density (PPI) from a standard 128+ byte EDID block.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_edid_ppi(edid: &[u8]) -> Option<f32> {
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid.len() < 128 || edid[..8] != HEADER {
        return None;
    }
    // Detailed Timing Descriptor 0 (bytes 54..72): native pixel size & millimeter dimensions.
    let pixel_clock = u16::from_le_bytes([edid[54], edid[55]]);
    if pixel_clock != 0 {
        let px_w = u32::from(edid[56]) | (u32::from(edid[58] & 0xF0) << 4);
        let px_h = u32::from(edid[59]) | (u32::from(edid[61] & 0xF0) << 4);
        let mm_w = u32::from(edid[66]) | (u32::from(edid[68] & 0xF0) << 4);
        let mm_h = u32::from(edid[67]) | (u32::from(edid[68] & 0x0F) << 8);
        let (mm_w, mm_h) = if mm_w > 0 && mm_h > 0 {
            (mm_w as f32, mm_h as f32)
        } else if edid[21] > 0 && edid[22] > 0 {
            (f32::from(edid[21]) * 10.0, f32::from(edid[22]) * 10.0)
        } else {
            return None;
        };
        if px_w >= 320 && px_h >= 240 && mm_w >= 40.0 && mm_h >= 40.0 {
            let ppi_x = px_w as f32 / (mm_w / 25.4);
            let ppi_y = px_h as f32 / (mm_h / 25.4);
            let ppi = (ppi_x + ppi_y) * 0.5;
            if (45.0..=500.0).contains(&ppi) {
                return Some(ppi);
            }
        }
    }
    None
}

/// Detect physical display PPI (Linux DRM EDID, falling back to Steam Deck panel density).
pub fn detect_display_ppi(is_steam_deck: bool) -> Option<f32> {
    if let Ok(raw) = std::env::var("AURORA_DISPLAY_PPI")
        && let Ok(v) = raw.trim().parse::<f32>()
        && (45.0..=500.0).contains(&v)
    {
        return Some(v);
    }
    #[cfg(target_os = "linux")]
    {
        let mut fallback_ppi: Option<f32> = None;
        if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(s) = name.to_str() else { continue };
                if !s.starts_with("card") || !s.contains('-') {
                    continue;
                }
                let dir = entry.path();
                let status = read_small_text(&dir.join("status"), 64).unwrap_or_default();
                if !status.trim().eq_ignore_ascii_case("connected") {
                    continue;
                }
                let enabled = read_small_text(&dir.join("enabled"), 64).unwrap_or_default();
                if !enabled.trim().eq_ignore_ascii_case("enabled") {
                    continue;
                }
                let Some(edid) = read_small_bytes(&dir.join("edid"), 512) else {
                    continue;
                };
                let Some(ppi) = parse_edid_ppi(&edid) else {
                    continue;
                };
                // Prefer built-in panel (eDP / DSI) if present; otherwise remember first connected display.
                if s.contains("-eDP-") || s.contains("-DSI-") {
                    return Some(ppi);
                }
                fallback_ppi.get_or_insert(ppi);
            }
        }
        if fallback_ppi.is_some() {
            return fallback_ppi;
        }
    }
    // Steam Deck LCD (7.0", 215 PPI) and OLED (7.4", 204 PPI) fallback when EDID is unavailable.
    is_steam_deck.then_some(210.0)
}

/// Format local wall-clock time as `HH:MM` and return seconds remaining until the next minute.
pub fn local_clock_hm() -> (String, u64) {
    #[cfg(target_os = "linux")]
    {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let t = unix as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
        if ok {
            let h = tm.tm_hour.clamp(0, 23);
            let m = tm.tm_min.clamp(0, 59);
            let s = tm.tm_sec.clamp(0, 59) as u64;
            return (format!("{h:02}:{m:02}"), (60 - s).max(1));
        }
    }
    #[cfg(windows)]
    {
        let mut st = windows_sys::Win32::Foundation::SYSTEMTIME {
            wYear: 0,
            wMonth: 0,
            wDayOfWeek: 0,
            wDay: 0,
            wHour: 0,
            wMinute: 0,
            wSecond: 0,
            wMilliseconds: 0,
        };
        unsafe {
            windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st);
        }
        let h = st.wHour.min(23);
        let m = st.wMinute.min(59);
        let s = u64::from(st.wSecond.min(59));
        return (format!("{h:02}:{m:02}"), (60 - s).max(1));
    }
    #[cfg(not(windows))]
    {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let secs_in_day = unix % 86_400;
        let h = secs_in_day / 3600;
        let m = (secs_in_day % 3600) / 60;
        let s = secs_in_day % 60;
        (format!("{h:02}:{m:02}"), (60 - s).max(1))
    }
}

#[cfg(target_os = "linux")]
fn linux_read_battery() -> Option<(u8, bool)> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let mut best: Option<(u8, bool)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        if !kind.trim().eq_ignore_ascii_case("battery") {
            continue;
        }
        // Skip peripheral/controller batteries (they expose scope = Device).
        let scope = std::fs::read_to_string(path.join("scope")).unwrap_or_default();
        if scope.trim().eq_ignore_ascii_case("device") {
            continue;
        }
        let cap_str = std::fs::read_to_string(path.join("capacity")).ok()?;
        let status_str = std::fs::read_to_string(path.join("status")).unwrap_or_default();
        if let Some(parsed) = parse_battery_reading(&cap_str, &status_str) {
            best = Some(parsed);
            break;
        }
    }
    best
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_battery_reading(capacity: &str, status: &str) -> Option<(u8, bool)> {
    let pct: u8 = capacity.trim().parse::<u16>().ok()?.min(100) as u8;
    let st = status.trim();
    let charging = st.eq_ignore_ascii_case("charging")
        || st.eq_ignore_ascii_case("full")
        || st.eq_ignore_ascii_case("not charging");
    Some((pct, charging))
}

#[cfg(target_os = "linux")]
fn linux_read_backlight() -> Option<(std::path::PathBuf, u8, bool)> {
    let entries = std::fs::read_dir("/sys/class/backlight").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let cur: u64 = std::fs::read_to_string(path.join("brightness")).ok()?.trim().parse().ok()?;
        let max: u64 = std::fs::read_to_string(path.join("max_brightness")).ok()?.trim().parse().ok()?;
        if max == 0 {
            continue;
        }
        let pct = scale_brightness_percent(cur, max);
        let direct_writable = std::fs::OpenOptions::new().write(true).open(path.join("brightness")).is_ok();
        let can_set = direct_writable || command_exists("brightnessctl");
        return Some((path, pct, can_set));
    }
    None
}

#[cfg(target_os = "linux")]
fn linux_set_brightness(dir: Option<&std::path::Path>, percent: u8) {
    let clamped = percent.clamp(5, 100);
    if let Some(path) = dir
        && let Ok(max_s) = std::fs::read_to_string(path.join("max_brightness"))
        && let Ok(max) = max_s.trim().parse::<u64>()
        && max > 0
    {
        let raw = ((u64::from(clamped) * max) / 100).max(1).min(max);
        if std::fs::write(path.join("brightness"), raw.to_string()).is_ok() {
            return;
        }
    }
    let _ = std::process::Command::new("brightnessctl")
        .args(["set", &format!("{clamped}%")])
        .output();
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn scale_brightness_percent(current: u64, max: u64) -> u8 {
    if max == 0 {
        return 100;
    }
    ((current.min(max) * 100 + max / 2) / max).min(100) as u8
}

#[cfg(target_os = "linux")]
fn command_exists(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
    })
}

#[cfg(target_os = "linux")]
fn linux_read_volume() -> Option<(u8, bool)> {
    if let Ok(out) = std::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        && out.status.success()
        && let Some(parsed) = parse_wpctl_volume(&String::from_utf8_lossy(&out.stdout))
    {
        return Some(parsed);
    }
    if let Ok(vol_out) = std::process::Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .output()
        && vol_out.status.success()
        && let Some(vol) = parse_pactl_volume(&String::from_utf8_lossy(&vol_out.stdout))
    {
        let muted = std::process::Command::new("pactl")
            .args(["get-sink-mute", "@DEFAULT_SINK@"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains("yes"));
        return Some((vol, muted));
    }
    None
}

#[cfg(target_os = "linux")]
fn linux_set_volume(percent: u8) {
    let pct = percent.min(100);
    let ok = std::process::Command::new("wpctl")
        .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{pct}%")])
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        if pct > 0 {
            let _ = std::process::Command::new("wpctl")
                .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "0"])
                .status();
        }
        return;
    }
    let _ = std::process::Command::new("pactl")
        .args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{pct}%")])
        .status();
    if pct > 0 {
        let _ = std::process::Command::new("pactl")
            .args(["set-sink-mute", "@DEFAULT_SINK@", "0"])
            .status();
    }
}

#[cfg(target_os = "linux")]
fn linux_set_mute(muted: bool) {
    let val = if muted { "1" } else { "0" };
    let ok = std::process::Command::new("wpctl")
        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", val])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        let _ = std::process::Command::new("pactl")
            .args(["set-sink-mute", "@DEFAULT_SINK@", val])
            .status();
    }
}

/// Parse `wpctl get-volume @DEFAULT_AUDIO_SINK@` output, e.g. `"Volume: 0.72 [MUTED]"`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_wpctl_volume(output: &str) -> Option<(u8, bool)> {
    let line = output.lines().next()?.trim();
    let rest = line.strip_prefix("Volume:")?.trim();
    let mut parts = rest.split_whitespace();
    let ratio: f32 = parts.next()?.parse().ok()?;
    let pct = (ratio * 100.0).round().clamp(0.0, 100.0) as u8;
    let muted = rest.to_ascii_uppercase().contains("MUTED");
    Some((pct, muted))
}

/// Parse `pactl get-sink-volume @DEFAULT_SINK@` output, e.g. `"Volume: front-left: 45875 /  70% / -9.3 dB"`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_pactl_volume(output: &str) -> Option<u8> {
    for token in output.split_whitespace() {
        if let Some(num) = token.strip_suffix('%')
            && let Ok(v) = num.parse::<u16>()
        {
            return Some(v.min(100) as u8);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpctl_and_pactl_volumes() {
        assert_eq!(parse_wpctl_volume("Volume: 0.65\n"), Some((65, false)));
        assert_eq!(parse_wpctl_volume("Volume: 0.00 [MUTED]\n"), Some((0, true)));
        assert_eq!(parse_wpctl_volume("Volume: 1.25\n"), Some((100, false)));
        assert_eq!(parse_wpctl_volume("bad"), None);

        let pactl = "Volume: front-left: 45875 /  70% / -9.30 dB,   front-right: 45875 /  70% / -9.30 dB";
        assert_eq!(parse_pactl_volume(pactl), Some(70));
        assert_eq!(parse_pactl_volume("Volume: 150%"), Some(100));
    }

    #[test]
    fn parses_battery_and_scales_brightness() {
        assert_eq!(parse_battery_reading("84\n", "Discharging\n"), Some((84, false)));
        assert_eq!(parse_battery_reading("100", "Full"), Some((100, true)));
        assert_eq!(parse_battery_reading("42", "Charging"), Some((42, true)));
        assert_eq!(parse_battery_reading("nope", "Charging"), None);

        assert_eq!(scale_brightness_percent(500, 1000), 50);
        assert_eq!(scale_brightness_percent(0, 1000), 0);
        assert_eq!(scale_brightness_percent(1200, 1000), 100);
        assert_eq!(scale_brightness_percent(10, 0), 100);
    }

    #[test]
    fn local_clock_formats_hm_and_bounds_minute_wait() {
        let (hm, wait) = local_clock_hm();
        assert_eq!(hm.len(), 5);
        assert_eq!(&hm[2..3], ":");
        assert!((1..=60).contains(&wait));
    }

    #[test]
    fn detects_steam_deck_dmi_os_release_and_overrides() {
        // LCD (Jupiter) and OLED (Galileo) via DMI even in KDE Desktop Mode
        assert!(detect_steam_deck_from(None, Some("Valve\n"), Some("Jupiter\n"), None, None, false, None));
        assert!(detect_steam_deck_from(None, Some("Valve"), None, Some("Galileo"), None, false, None));
        assert!(detect_steam_deck_from(None, Some("Valve"), None, None, Some("Steam Deck"), false, None));
        // Non-Valve vendor does not match Jupiter
        assert!(!detect_steam_deck_from(None, Some("ASUS"), Some("Jupiter"), None, None, false, None));
        // Gamescope session & SteamOS os-release
        assert!(detect_steam_deck_from(None, None, None, None, None, true, None));
        assert!(detect_steam_deck_from(None, None, None, None, None, false, Some("NAME=\"SteamOS\"\nID=steamos\n")));
        assert!(detect_steam_deck_from(None, None, None, None, None, false, Some("ID=bazzite\nVARIANT_ID=\"steamdeck\"\n")));
        assert!(!detect_steam_deck_from(None, Some("LENOVO"), Some("ThinkPad"), None, None, false, Some("ID=arch\n")));
        // Explicit env overrides win over hardware/session
        assert!(!detect_steam_deck_from(Some(false), Some("Valve"), Some("Jupiter"), None, None, true, Some("ID=steamos")));
        assert!(detect_steam_deck_from(Some(true), Some("LENOVO"), Some("ThinkPad"), None, None, false, None));
    }

    #[test]
    fn parses_linux_touchscreen_bitmasks() {
        // Direct multi-touch touchscreen: INPUT_PROP_DIRECT (bit 1 = 0x2), EV_ABS (bit 3 = 0x8), ABS_MT_POSITION_X (bit 53 = 0x20000000000000)
        assert!(parse_input_has_touchscreen("2", "b", "20000000000000", "0", 64));
        // Indirect touchpad (no INPUT_PROP_DIRECT)
        assert!(!parse_input_has_touchscreen("1", "b", "20000000000000", "0", 64));
        // Pen-only graphics tablet without MT (BTN_TOUCH = 330, BTN_TOOL_PEN = 320 -> word 5 = 0x401)
        assert!(!parse_input_has_touchscreen("2", "b", "3", "401 0 0 0 0 0", 64));
        // Single-touch direct screen without pen (BTN_TOUCH = 330 -> word 5 = 0x400)
        assert!(parse_input_has_touchscreen("2", "b", "3", "400 0 0 0 0 0", 64));
    }

    #[test]
    fn parses_steam_deck_edid_ppi() {
        let mut edid = [0u8; 128];
        edid[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // Steam Deck LCD panel: 800x1280 portrait, 94mm x 151mm (~215.5 PPI)
        edid[54] = 1; // non-zero pixel clock
        let (pw, ph, mw, mh): (u16, u16, u16, u16) = (800, 1280, 94, 151);
        edid[56] = (pw & 0xFF) as u8;
        edid[58] = ((pw >> 4) & 0xF0) as u8;
        edid[59] = (ph & 0xFF) as u8;
        edid[61] = ((ph >> 4) & 0xF0) as u8;
        edid[66] = (mw & 0xFF) as u8;
        edid[67] = (mh & 0xFF) as u8;
        edid[68] = (((mw >> 4) & 0xF0) | ((mh >> 8) & 0x0F)) as u8;
        let ppi = parse_edid_ppi(&edid).expect("valid Deck EDID");
        assert!((214.0..=217.0).contains(&ppi), "unexpected ppi {ppi}");

        // Invalid header rejected
        edid[0] = 0xFF;
        assert!(parse_edid_ppi(&edid).is_none());
    }
}
