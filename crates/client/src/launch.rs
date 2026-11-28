//! Starting installed games through the user's windowless Steam host by default.
//! The official client remains available for compatibility. When Steam is
//! unavailable, compatible games can still use Aurora's steamclient or start
//! without Steam features.

use model::{AppId, OwnedApp};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    /// Steam features come from Aurora's own steamclient.
    Aurora,
    /// Runs through the official Steam client. `anti_cheat` names the reason.
    OfficialSteam { anti_cheat: String },
    /// Runs on a headless instance of the user's own Steam engine.
    HeadlessHost { steam_drm: bool },
    /// Starts without Steam features; `reason` says why.
    NoSteam { reason: String },
}

impl LaunchMode {
    /// Keep Aurora's Cloud and Workshop sync for ordinary games on the host.
    pub(crate) fn uses_aurora_sync(&self) -> bool {
        !matches!(self, Self::OfficialSteam { .. } | Self::HeadlessHost { steam_drm: true })
    }
}

/// How Play will start this game. Reads the install folder (anti-cheat
/// files, the executable's bitness), so callers should cache it.
pub fn mode(app: &OwnedApp, install_dir: &Path) -> LaunchMode {
    mode_for_launch(app, install_dir, 0)
}

pub(crate) fn mode_for_launch(app: &OwnedApp, install_dir: &Path, controller: u8) -> LaunchMode {
    mode_with_host(app, install_dir, controller, headless_host_available() && !official_steam_open())
}

pub(crate) fn mode_with_host(app: &OwnedApp, install_dir: &Path, controller: u8, host_ready: bool) -> LaunchMode {
    #[cfg(not(windows))]
    let force_official = prefer_official();
    #[cfg(windows)]
    let force_official = false;

    let executable = launch_exe(app, install_dir).filter(|path| path.is_file());
    let use_host = host_ready && !force_official && executable.is_some();
    let ac = anti_cheat(app, install_dir);
    if !use_host {
        if let Some(ac) = ac {
            return LaunchMode::OfficialSteam { anti_cheat: ac };
        }
    }
    let steam_drm = steam_drm_marked(app.app_id) || executable.is_some_and(|exe| has_steam_drm(&exe));
    if use_host {
        // Demand-driven hybrid routing (without OfficialSteam):
        // Keep HeadlessHost as default when no controller translation/custom
        // layout is needed, or whenever Steam DRM or anti-cheat requires Valve's
        // engine (those launches use our external OS virtual gamepad instead).
        // For non-DRM, non-anti-cheat Windows/Proton games that actively need
        // Steam Input translation or custom bindings, route to Aurora's
        // steamclient so ISteamInput + layouts + XInput SHM run natively.
        if !steam_drm && ac.is_none() && needs_aurora_input_route(app.app_id, controller) {
            #[cfg(windows)]
            if windows_mode(app, install_dir) == LaunchMode::Aurora {
                return LaunchMode::Aurora;
            }
            #[cfg(not(windows))]
            if let Ok(p) = linux::plan(app, install_dir)
                && linux::client_dll(&p, install_dir).is_some()
            {
                return LaunchMode::Aurora;
            }
        }
        return LaunchMode::HeadlessHost { steam_drm };
    }
    if steam_drm {
        return LaunchMode::OfficialSteam { anti_cheat: STEAM_DRM.into() };
    }
    #[cfg(not(windows))]
    {
        if force_official {
            return LaunchMode::OfficialSteam { anti_cheat: "Steam for Linux".into() };
        }
        return match linux::plan(app, install_dir) {
            Ok(p) if linux::client_dll(&p, install_dir).is_some() => LaunchMode::Aurora,
            Ok(p) => LaunchMode::NoSteam { reason: p.describe() },
            Err(_) => LaunchMode::OfficialSteam { anti_cheat: "Steam for Linux".into() },
        };
    }
    #[cfg(windows)]
    return windows_mode(app, install_dir);
}

/// Whether this launch actively needs controller translation / Steam Input
/// (non-Xbox enabled pad connected, custom layout chosen, or per-game Enable).
pub(crate) fn needs_aurora_input_route(app_id: AppId, controller: u8) -> bool {
    let eff = crate::pads::effective_controller_override(app_id, controller);
    let families = crate::pads::settings().families;
    let has_custom = crate::input_config::app_binding(app_id) != crate::input_config::BindingChoice::Auto;
    let summary = crate::pads::connected_summary();
    summary.needs_translation(eff, &families, has_custom)
}

/// Windows: our steamclient DLL when it can serve the game.
#[cfg(windows)]
fn windows_mode(app: &OwnedApp, install_dir: &Path) -> LaunchMode {
    let Some(exe) = executable(app, install_dir) else {
        return LaunchMode::NoSteam { reason: "no launch option for this system".into() };
    };
    let Some(is64) = pe_is_64bit(&exe) else {
        return LaunchMode::NoSteam { reason: "game file not found".into() };
    };
    if steamclient_dll(is64).is_none() {
        return LaunchMode::NoSteam {
            reason: if is64 { "steamclient.dll not found next to Aurora" } else { "steamclient32.dll (for 32-bit games) not found next to Aurora" }.into(),
        };
    }
    LaunchMode::Aurora
}

/// What [`mode_cached`] remembers.
static MODE_CACHE: Mutex<Option<std::collections::HashMap<(AppId, PathBuf), (Instant, LaunchMode)>>> = Mutex::new(None);
/// Games whose mode is being worked out in the background.
static MODE_PENDING: Mutex<Option<std::collections::HashSet<(AppId, PathBuf)>>> = Mutex::new(None);
/// How long a remembered mode counts as current.
const MODE_FRESH: Duration = Duration::from_secs(60);

fn mode_lookup(key: &(AppId, PathBuf)) -> Option<(bool, LaunchMode)> {
    let cache = MODE_CACHE.lock().unwrap();
    cache.as_ref()?.get(key).map(|(at, m)| (at.elapsed() < MODE_FRESH, m.clone()))
}

fn mode_store(key: (AppId, PathBuf), m: LaunchMode) {
    MODE_CACHE.lock().unwrap().get_or_insert_with(Default::default).insert(key, (Instant::now(), m));
}

/// Work the mode out on a background thread (once at a time per game,
/// capped at a small number of concurrent refresh threads).
fn mode_refresh(app: &OwnedApp, install_dir: &Path) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const MAX_MODE_WORKERS: usize = 4;
    static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);

    let key = (app.app_id, install_dir.to_path_buf());
    let prev = ACTIVE_WORKERS.fetch_add(1, Ordering::AcqRel);
    if prev >= MAX_MODE_WORKERS {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
        return;
    }
    if !MODE_PENDING.lock().unwrap().get_or_insert_with(Default::default).insert(key.clone()) {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
        return;
    }
    let (owned, k) = (app.clone(), key.clone());
    let spawned = std::thread::Builder::new().name("launch-mode".into()).spawn(move || {
        let m = mode(&owned, &k.1);
        mode_store(k.clone(), m);
        MODE_PENDING.lock().unwrap().get_or_insert_with(Default::default).remove(&k);
        ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    });
    if spawned.is_err() {
        MODE_PENDING.lock().unwrap().get_or_insert_with(Default::default).remove(&key);
        ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// [`mode`], remembered: a stale answer is returned at once and refreshed in
/// the background. Only the very first ask for a game waits for it.
pub fn mode_cached(app: &OwnedApp, install_dir: &Path) -> LaunchMode {
    let key = (app.app_id, install_dir.to_path_buf());
    match mode_lookup(&key) {
        Some((fresh, m)) => {
            if !fresh {
                mode_refresh(app, install_dir);
            }
            m
        }
        None => {
            let m = mode(app, install_dir);
            mode_store(key, m.clone());
            m
        }
    }
}

/// [`mode_cached`] for UI drawn every frame: never waits (reading the install
/// folder can take tens of milliseconds); `None` until the first answer is in.
pub fn mode_peek(app: &OwnedApp, install_dir: &Path) -> Option<LaunchMode> {
    let found = mode_lookup(&(app.app_id, install_dir.to_path_buf()));
    if !found.as_ref().is_some_and(|(fresh, _)| *fresh) {
        mode_refresh(app, install_dir);
    }
    found.map(|(_, m)| m)
}

// ---------------------------------------------------------- anti-cheat

/// Files and folders that mean an anti-cheat ships with the game
/// (lower-case, matched against names in the install folder).
const ANTI_CHEAT_MARKERS: &[(&str, &str)] = &[
    ("easyanticheat", "Easy Anti-Cheat"),
    ("start_protected_game.exe", "Easy Anti-Cheat"),
    ("eac_launcher", "Easy Anti-Cheat"),
    ("battleye", "BattlEye"),
    ("beservice", "BattlEye"),
    ("beclient", "BattlEye"),
    ("eaanticheat", "EA Anti-Cheat"),
    ("pnkbstr", "PunkBuster"),
    ("punkbuster", "PunkBuster"),
    ("pbsvc", "PunkBuster"),
    ("xigncode", "XIGNCODE3"),
    ("gameguard", "nProtect GameGuard"),
    ("npgamemon", "nProtect GameGuard"),
    ("equ8", "EQU8"),
    ("mhyprot", "miHoYo Protect"),
    ("vgk.sys", "Vanguard"),
    ("anticheatexpert", "Anti-Cheat Expert"),
    ("ace-base", "Anti-Cheat Expert"),
    ("tp3helper", "TenProtect"),
    ("faceit", "FACEIT"),
    ("randgrid", "RICOCHET"),
    ("blackcipher", "BlackCipher"),
    ("nexonguard", "Nexon Guard"),
    ("hackshield", "HackShield"),
    ("anticheat", "anti-cheat"),
    ("anti-cheat", "anti-cheat"),
    ("anti_cheat", "anti-cheat"),
];

/// Steam category for "Valve Anti-Cheat enabled".
const CATEGORY_VAC: u32 = 8;

/// The anti-cheat this game uses, if any. An unreadable install folder
/// is treated conservatively as a possible anti-cheat install.
pub fn anti_cheat(app: &OwnedApp, install_dir: &Path) -> Option<String> {
    if app.categories.contains(&CATEGORY_VAC) {
        return Some("Valve Anti-Cheat".into());
    }
    for opt in app.launch.iter().filter(|o| o.runs_on_this_os() || (cfg!(target_os = "linux") && o.os_list.split(',').any(|os| os.trim() == "windows"))) {
        let text = format!("{} {}", opt.executable, opt.arguments).to_lowercase();
        if let Some((_, name)) = ANTI_CHEAT_MARKERS.iter().find(|(m, _)| text.contains(m)) {
            return Some(name.to_string());
        }
    }
    if std::fs::read_dir(install_dir).is_err() {
        return Some("unknown (install folder unreadable)".into());
    }
    scan_markers(install_dir, 3).map(String::from)
}

fn scan_markers(dir: &Path, depth: u32) -> Option<&'static str> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if let Some((_, ac)) = ANTI_CHEAT_MARKERS.iter().find(|(m, _)| name.contains(m)) {
            return Some(ac);
        }
        if depth > 0 && e.file_type().is_ok_and(|t| t.is_dir()) {
            subdirs.push(e.path());
        }
    }
    subdirs.iter().find_map(|d| scan_markers(d, depth - 1))
}

// ---------------------------------------------------------- spawning

fn safe_install_rel(install_dir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains('\0') || rel.contains(':') || rel.starts_with('/') || rel.starts_with('\\') {
        return None;
    }
    let mut out = install_dir.to_path_buf();
    let mut pushed = false;
    for seg in rel.split(['/', '\\']) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return None;
        }
        out.push(seg);
        pushed = true;
    }
    pushed.then_some(out)
}

fn executable(app: &OwnedApp, install_dir: &Path) -> Option<PathBuf> {
    let opt = app.launch.iter().find(|o| o.runs_on_this_os() && !o.executable.is_empty())?;
    safe_install_rel(install_dir, &opt.executable)
}

/// What [`LaunchMode::OfficialSteam`] names for DRM-wrapped games.
pub const STEAM_DRM: &str = "Steam DRM";

/// The program Play starts (on Linux, the one the plan picked).
fn launch_exe(app: &OwnedApp, install_dir: &Path) -> Option<PathBuf> {
    #[cfg(not(windows))]
    return linux::plan(app, install_dir).ok().map(|p| p.exe(install_dir));
    #[allow(unreachable_code)]
    executable(app, install_dir)
}

/// Games found to carry Steam's DRM while running on our steamclient (see
/// `Request::SteamDrm`): wrappers without a `.bind` section, such as
/// PRAGMATA's packer. One app id per line.
fn steam_drm_file() -> PathBuf {
    crate::store::config_dir().join("steam-drm-apps.txt")
}

fn steam_drm_marked(app_id: AppId) -> bool {
    std::fs::read_to_string(steam_drm_file()).is_ok_and(|t| t.lines().any(|l| l.trim().parse() == Ok(app_id)))
}

/// Remember that this game carries Steam's DRM, so Play starts it on Valve's
/// engine from now on. False if it was already known.
pub fn mark_steam_drm(app_id: AppId) -> bool {
    if steam_drm_marked(app_id) {
        return false;
    }
    let path = steam_drm_file();
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{app_id}\n"));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, text);
    if let Some(map) = MODE_CACHE.lock().unwrap().as_mut() {
        map.retain(|(id, _), _| *id != app_id);
    }
    true
}

/// Wrapped with Steam's DRM (SteamStub), which adds a `.bind` section to the
/// executable.
fn has_steam_drm(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let read = || -> Option<bool> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut mz = [0u8; 64];
        f.read_exact(&mut mz).ok()?;
        if &mz[..2] != b"MZ" {
            return None;
        }
        let pe = u32::from_le_bytes(mz[60..64].try_into().ok()?) as u64;
        f.seek(SeekFrom::Start(pe)).ok()?;
        let mut hdr = [0u8; 24];
        f.read_exact(&mut hdr).ok()?;
        if &hdr[..4] != b"PE\0\0" {
            return None;
        }
        let sections = u16::from_le_bytes([hdr[6], hdr[7]]) as usize;
        let optional = u16::from_le_bytes([hdr[20], hdr[21]]) as i64;
        f.seek(SeekFrom::Current(optional)).ok()?;
        let mut table = vec![0u8; sections.min(96) * 40];
        f.read_exact(&mut table).ok()?;
        Some(table.chunks(40).any(|s| &s[..8] == b".bind\0\0\0"))
    };
    read().unwrap_or(false)
}

/// 64-bit Windows executable? `None` if unreadable or not a PE file.
fn pe_is_64bit(path: &Path) -> Option<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut mz = [0u8; 64];
    f.read_exact(&mut mz).ok()?;
    if &mz[..2] != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(mz[60..64].try_into().ok()?) as u64;
    f.seek(SeekFrom::Start(pe)).ok()?;
    let mut hdr = [0u8; 6];
    f.read_exact(&mut hdr).ok()?;
    if &hdr[..4] != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes([hdr[4], hdr[5]]) == 0x8664)
}

/// Our steamclient DLL for 64-bit or 32-bit games, next to the app:
/// `steamclient64.dll` / `steamclient.dll` (the cargo build name) and
/// `steamclient32.dll`; while developing, also the cargo i686 build output
/// (`target/i686-pc-windows-gnu/<profile>/steamclient.dll`).
pub fn steamclient_dll(is64: bool) -> Option<PathBuf> {
    let var = if is64 { "STEAMINRUST_STEAMCLIENT64" } else { "STEAMINRUST_STEAMCLIENT32" };
    if let Some(p) = std::env::var_os(var).map(PathBuf::from) {
        return (pe_is_64bit(&p) == Some(is64)).then_some(p);
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut candidates: Vec<PathBuf> = if is64 {
        vec![dir.join("steamclient64.dll"), dir.join("steamclient.dll")]
    } else {
        vec![dir.join("steamclient32.dll"), dir.join("steamclient.dll")]
    };
    if !is64 {
        if let (Some(profile), Some(target)) = (dir.file_name(), dir.parent()) {
            candidates.push(target.join("i686-pc-windows-gnu").join(profile).join("steamclient.dll"));
        }
    }
    candidates.into_iter().find(|p| pe_is_64bit(p) == Some(is64))
}

/// Arguments for starting an installed game: `user` comes from the player's
/// local Launch Options (eligible for `VAR=val` and `%command%` wrappers),
/// while `connect` comes from an untrusted remote invite, link, or Rich
/// Presence and is strictly validated by [`safe_connect_args`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchArgs {
    pub user: String,
    pub connect: String,
    /// Per-game Steam Input override (`0` = default, `1` = enable, `2` = disable).
    #[serde(default)]
    pub controller: u8,
}

impl LaunchArgs {
    pub fn user(user: impl Into<String>) -> Self {
        Self { user: user.into(), connect: String::new(), controller: 0 }
    }

    pub fn connect(connect: impl Into<String>) -> Self {
        Self { user: String::new(), connect: connect.into(), controller: 0 }
    }

    pub fn with_controller(mut self, controller: u8) -> Self {
        self.controller = controller;
        self
    }
}

/// Validate untrusted remote `connect` arguments fail-closed before launching.
pub fn safe_connect_args(connect: &str) -> Result<Vec<String>, String> {
    model::safe_connect_tokens(connect).map_err(|why| format!("refusing launch: {why}"))
}

pub fn spawn(app: &OwnedApp, install_dir: &Path, args: &LaunchArgs, overlay: Option<(u16, &str)>) -> Result<Child, String> {
    let connect = safe_connect_args(&args.connect)?;
    let opts = UserOptions::parse(&args.user);
    let mut cmd = finish(app, install_dir, &opts, command(app, install_dir, &opts, &connect)?);
    #[cfg(target_os = "linux")]
    apply_vk_overlay_env(&mut cmd, overlay, app_overlay_enabled(app.app_id));
    #[cfg(not(target_os = "linux"))]
    let _ = overlay;
    cmd.spawn().map_err(|e| format!("could not start {}: {e}", app.name))
}

/// Environment variables controlling Steam Input and controller bridging for
/// a launch with our `steamclient`.
pub(crate) fn steam_input_env_vars(
    app_override: u8,
    settings: &crate::pads::ControllerSettings,
    pads: Option<&str>,
) -> Vec<(&'static str, String)> {
    let decision = crate::pads::resolve_steam_input(app_override, &settings.families);
    let mut env = vec![
        (crate::pads::ENV_STEAM_INPUT, if decision.enabled { "1" } else { "0" }.to_string()),
        (crate::pads::ENV_INPUT_FAMILIES, format!("0x{:x}", decision.families_mask)),
        (crate::pads::ENV_INPUT_CALIBRATION, settings.calibration.encode()),
    ];
    if decision.enabled
        && decision.use_shm_bridge
        && let Some(name) = pads
    {
        env.push((crate::pads::ENV_PADS, name.to_string()));
    }
    env
}

/// Pass Aurora's SDL3 controller mappings (`SDL_GAMECONTROLLERCONFIG`) and,
/// when an external OS virtual Xbox 360 pad is translating non-Xbox controllers,
/// suppress raw physical non-Xbox VID:PIDs in SDL games (`SDL_GAMECONTROLLER_IGNORE_DEVICES`)
/// to prevent double input without any DLL injection.
fn apply_sdl_controller_env(cmd: &mut Command, ignore_translated_physical: bool) {
    let summary = crate::pads::connected_summary();
    if !summary.sdl_mappings.is_empty() {
        let mut lines: Vec<String> = std::env::var("SDL_GAMECONTROLLERCONFIG")
            .ok()
            .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
            .unwrap_or_default();
        for m in &summary.sdl_mappings {
            if !lines.iter().any(|existing| existing == m) {
                lines.push(m.clone());
            }
        }
        cmd.env("SDL_GAMECONTROLLERCONFIG", lines.join("\n"));
    }
    if ignore_translated_physical && !summary.non_xbox_vid_pids.is_empty() {
        let mut entries: Vec<String> = std::env::var("SDL_GAMECONTROLLER_IGNORE_DEVICES")
            .ok()
            .map(|s| s.split(',').map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
            .unwrap_or_default();
        for (vid, pid) in &summary.non_xbox_vid_pids {
            let item = format!("0x{vid:04X}/0x{pid:04X}");
            if !entries.iter().any(|e| e.eq_ignore_ascii_case(&item)) {
                entries.push(item);
            }
        }
        if !entries.is_empty() {
            cmd.env("SDL_GAMECONTROLLER_IGNORE_DEVICES", entries.join(","));
        }
    }
}

/// Start a game with our steamclient: environment like a Steam launch, plus
/// the pipe and token for the DLL. The registry swap must be in place.
/// `pads` names the shared memory with the controllers XInput can't see.
pub fn spawn_with_steamclient(app: &OwnedApp, install_dir: &Path, args: &LaunchArgs, ipc_env: &str, account: &str, pads: Option<&str>, overlay: Option<(u16, &str)>, ignore_physical_pads: bool) -> Result<Child, String> {
    let connect = safe_connect_args(&args.connect)?;
    let opts = UserOptions::parse(&args.user);
    let mut cmd = command(app, install_dir, &opts, &connect)?;
    let id = app.app_id.to_string();
    let overlay_on = app_overlay_enabled(app.app_id);
    cmd.env(crate::ipc::protocol::ENV_IPC, ipc_env)
        .env("SteamOverlayGameId", &id)
        .env("SteamClientLaunch", "1")
        .env("SteamEnv", "1")
        .env("SteamAppUser", account)
        .env("SteamUser", account);
    if !overlay_on {
        cmd.env("AURORA_DISABLE_OVERLAY", "1");
    }
    apply_hotkey_env(&mut cmd);
    let ctrl_override = crate::pads::effective_controller_override(app.app_id, args.controller);
    let ctrl_settings = crate::pads::settings();
    for (k, v) in steam_input_env_vars(ctrl_override, &ctrl_settings, pads) {
        cmd.env(k, v);
    }
    if let Some(log) = std::env::var_os("STEAMINRUST_STEAMCLIENT_LOG") {
        cmd.env(crate::ipc::protocol::ENV_LOG, log);
    }
    // Proton: a Steam folder whose legacycompat/ DLLs are ours, which Proton
    // copies into the prefix. Proton's Wine also swaps any "steamclient64"
    // the game loads for its own bridge to the Steam client (lsteamclient)
    // unless PROTON_DISABLE_LSTEAMCLIENT is set. That bridge is also what
    // writes the registry entry naming the DLL, so a prefix without it yet
    // keeps the bridge for this one launch. The DLL draws the Shift+Tab
    // overlay inside D3D11 games and reaches the app over loopback.
    #[cfg(not(windows))]
    {
        let root = local::find_steam_root().ok_or("Steam Play needs a Steam install for Proton")?;
        cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", linux::client_shim(&root)?);
        if prefix_knows_client(app, install_dir) {
            cmd.env("PROTON_DISABLE_LSTEAMCLIENT", "1");
        } else {
            // `prefix_setup` normally ran first; if it failed, this launch keeps the bridge.
            tracing::info!("{}: new Proton prefix, Steam features from our steamclient from the next launch", app.name);
        }
    }
    let mut cmd = finish(app, install_dir, &opts, cmd);
    apply_sdl_controller_env(&mut cmd, ignore_physical_pads);
    #[cfg(target_os = "linux")]
    apply_vk_overlay_env(&mut cmd, overlay, overlay_on);
    #[cfg(not(target_os = "linux"))]
    let _ = overlay;
    cmd.spawn().map_err(|e| format!("could not start {}: {e}", app.name))
}

/// Start a non-Steam game the player added (program path, launch options).
/// A path pasted with quotes ("Copy as path") works too.
///
/// Linux: Windows programs run with Proton (the global choice, else Steam's
/// default), in a prefix of their own like Steam's shortcuts; anything else
/// runs directly (made executable first if it isn't). Windows: shortcuts,
/// batch files and programs that need admin rights start through the shell,
/// which gives no process to wait for (`None`).
pub fn spawn_non_steam(exe: &str, args: &str) -> Result<Option<Child>, String> {
    let exe = PathBuf::from(exe.trim().trim_matches('"'));
    if !exe.exists() {
        return Err(format!("{} doesn't exist", exe.display()));
    }
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let opts = UserOptions::parse(args);
    #[cfg(not(windows))]
    {
        let mut cmd = linux::non_steam(&exe, &opts.args)?;
        cmd.current_dir(&dir);
        return opts.wrap(cmd).spawn().map(Some).map_err(|e| format!("couldn't start {}: {e}", exe.display()));
    }
    #[cfg(windows)]
    {
        let ext = exe.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == "exe" {
            let mut cmd = Command::new(&exe);
            cmd.args(&opts.args).current_dir(&dir);
            match opts.wrap(cmd).spawn() {
                Ok(child) => return Ok(Some(child)),
                // ERROR_ELEVATION_REQUIRED: the shell asks for admin rights.
                Err(e) if e.raw_os_error() == Some(740) => {}
                Err(e) => return Err(format!("couldn't start {}: {e}", exe.display())),
            }
        }
        if !opts.args_only() {
            tracing::warn!("launch options: variables and wrappers don't apply to {} (opened through the Windows shell)", exe.display());
        }
        win::shell_open(&exe, &opts.args_line(), &dir).map(|()| None)
    }
}

/// The finished game command: the player's variables and wrapper
/// (`UserOptions::wrap`), then, for a native game under a path with a colon,
/// the private mount that lets it start (see `colon`), which must come last:
/// rebuilding the command for a wrapper would drop it.
fn finish(app: &OwnedApp, install_dir: &Path, opts: &UserOptions, cmd: Command) -> Command {
    let cmd = opts.wrap(cmd);
    #[cfg(target_os = "linux")]
    if colon::colon_folder(install_dir).is_some() && linux::plan(app, install_dir).is_ok_and(|p| !p.uses_proton()) {
        return colon::redirect(cmd, install_dir);
    }
    let _ = app;
    cmd
}

/// The game's own command plus the player's arguments; their variables and
/// wrapper come last, with `UserOptions::wrap`.
fn command(app: &OwnedApp, install_dir: &Path, opts: &UserOptions, connect: &[String]) -> Result<Command, String> {
    #[cfg(not(windows))]
    {
        let mut cmd = linux::plan(app, install_dir)?.command(app, install_dir);
        cmd.args(&opts.args).args(connect);
        return Ok(cmd);
    }
    #[allow(unreachable_code)]
    let opt = app
        .launch
        .iter()
        .find(|o| o.runs_on_this_os() && !o.executable.is_empty())
        .ok_or_else(|| format!("{} has no launch option for this OS", app.name))?;

    let exe = safe_install_rel(install_dir, &opt.executable)
        .ok_or_else(|| format!("{} has an unsafe launch executable path", app.name))?;
    let cwd = if opt.working_dir.is_empty() {
        exe.parent().unwrap_or(install_dir).to_path_buf()
    } else {
        safe_install_rel(install_dir, &opt.working_dir)
            .ok_or_else(|| format!("{} has an unsafe working directory path", app.name))?
    };
    tracing::info!("launching {} ({})", app.name, exe.display());

    let mut cmd = Command::new(&exe);
    cmd.args(split_args(&opt.arguments))
        .args(&opts.args)
        .args(connect)
        .current_dir(cwd)
        .env("SteamAppId", app.app_id.to_string())
        .env("SteamGameId", app.app_id.to_string());
    Ok(cmd)
}

fn normalize(p: &str) -> String {
    if cfg!(windows) { p.replace('/', "\\") } else { p.replace('\\', "/") }
}

/// Minimal shell-style split that respects double quotes.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The launch options the player typed, read like Steam's: with
/// `%command%`, the words before it are `NAME=value` variables and then a
/// wrapper program (`MANGOHUD=1 gamemoderun %command% -novid`), and the
/// words after it are game arguments. Without it, every word is a game
/// argument. Single and double quotes group words, as in a shell.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct UserOptions {
    env: Vec<(String, String)>,
    wrapper: Vec<String>,
    args: Vec<String>,
}

impl UserOptions {
    pub(crate) fn parse(s: &str) -> UserOptions {
        let words = shell_words(s);
        let Some(at) = words.iter().position(|w| w == "%command%") else {
            return UserOptions { args: words, ..Default::default() };
        };
        let mut before = words[..at].iter().peekable();
        let mut env = Vec::new();
        while let Some((name, value)) = before.peek().and_then(|w| w.split_once('=')).filter(|(n, _)| is_env_name(n)) {
            env.push((name.to_string(), value.to_string()));
            before.next();
        }
        let wrapper = before.cloned().collect();
        // A second %command% means nothing here; drop it rather than pass it on.
        let args = words[at + 1..].iter().filter(|w| *w != "%command%").cloned().collect();
        UserOptions { env, wrapper, args }
    }

    /// Only game arguments (no variables or wrapper).
    fn args_only(&self) -> bool {
        self.env.is_empty() && self.wrapper.is_empty()
    }

    /// The game arguments as one command line (for the Windows shell).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn args_line(&self) -> String {
        self.args.iter().map(|a| if a.is_empty() || a.contains(char::is_whitespace) { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ")
    }

    /// The finished game command with the player's variables on top
    /// (they win over ours, as in Steam) and their wrapper in front of it.
    fn wrap(&self, mut cmd: Command) -> Command {
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let Some((program, rest)) = self.wrapper.split_first() else { return cmd };
        tracing::info!("launch options: running the game through {}", self.wrapper.join(" "));
        let mut w = Command::new(program);
        w.args(rest).arg(cmd.get_program()).args(cmd.get_args());
        for (k, v) in cmd.get_envs() {
            match v {
                Some(v) => w.env(k, v),
                None => w.env_remove(k),
            };
        }
        if let Some(dir) = cmd.get_current_dir() {
            w.current_dir(dir);
        }
        // Every game command here gets its own process group (see `kill_tree`).
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut w, 0);
        w
    }
}

fn is_env_name(n: &str) -> bool {
    n.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Shell-style split for text the player typed: single quotes keep
/// everything literally, double quotes group words (both can sit inside a
/// word: `WINEDLLOVERRIDES="dinput8=n,b"`).
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut word = false;
    let mut quote = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                word = true;
            }
            (None, c) if c.is_whitespace() => {
                if word {
                    out.push(std::mem::take(&mut cur));
                    word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                word = true;
            }
        }
    }
    if word {
        out.push(cur);
    }
    out
}

/// Kill a process and everything it started.
pub fn kill_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").args(["-TERM", &format!("-{pid}")]).status();
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    }
}

/// Close every process running from a game's folder (for games the
/// official client started, whose process ids we don't know).
pub fn kill_in_folder(dir: &Path) {
    for pid in win::processes_in(dir) {
        kill_tree(pid);
    }
}

/// Adaptive post-exit grace period for launcher-to-game handoffs.
///
/// - If the user explicitly clicked Stop, return as soon as no process is alive.
/// - If a process lived long enough to be the real game (>= 15s initial process
///   or >= 10s descendant), only wait briefly (250ms) once nothing is alive.
/// - If the initial process exited quickly (< 15s, typical of a bootstrapper
///   like Paradox's `dowser` or a game launcher), wait up to 2s for the child
///   game process to appear.
fn post_exit_grace(initial_lifetime: Duration, descendant_lifetime: Option<Duration>, stop_requested: bool) -> Duration {
    if stop_requested {
        return Duration::ZERO;
    }
    const LONG_RUN: Duration = Duration::from_secs(15);
    const LONG_DESCENDANT: Duration = Duration::from_secs(10);
    if initial_lifetime >= LONG_RUN || descendant_lifetime.is_some_and(|d| d >= LONG_DESCENDANT) {
        Duration::from_millis(250)
    } else {
        Duration::from_secs(2)
    }
}

/// Wait for a game process and any launcher-spawned child processes. On
/// Windows, attaches the child to a Job Object so descendant processes
/// (even outside `install_dir`) keep IPC and shared controller publishing
/// alive through launcher handoff. Also check the install directory when the job
/// is empty, since a launcher can spawn the game before job assignment.
#[cfg(test)]
pub fn wait_in_dir(app_id: AppId, install_dir: &Path, child: Child) -> AppId {
    wait_in_dir_cancellable(app_id, install_dir, child, None)
}

pub(crate) fn wait_in_dir_cancellable(
    app_id: AppId,
    install_dir: &Path,
    mut child: Child,
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> AppId {
    #[cfg(windows)]
    let job = win::JobGuard::attach(&child);
    let started = Instant::now();
    wait_all_inner(&mut child, started, stop);
    #[cfg(windows)]
    {
        let initial_lifetime = started.elapsed();
        let mut logged = false;
        let mut descendant_since: Option<Instant> = None;
        let mut descendant_lifetime: Option<Duration> = None;
        let mut empty_since: Option<Instant> = None;
        loop {
            let alive = job.as_ref().is_some_and(win::JobGuard::has_processes)
                || !win::processes_in(install_dir).is_empty();
            if alive {
                if !logged {
                    tracing::info!("initial game process exited; waiting for launcher-spawned processes");
                    logged = true;
                }
                descendant_since.get_or_insert_with(Instant::now);
                empty_since = None;
            } else {
                if let Some(since) = descendant_since.take() {
                    let dur = since.elapsed();
                    descendant_lifetime = Some(descendant_lifetime.map_or(dur, |prev| prev.max(dur)));
                }
                let stopped = stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Acquire));
                let grace = post_exit_grace(initial_lifetime, descendant_lifetime, stopped);
                if grace.is_zero() || empty_since.get_or_insert_with(Instant::now).elapsed() >= grace {
                    break;
                }
            }
            let stopped = stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Acquire));
            let poll_delay = if !stopped && empty_since.is_none() && descendant_since.is_some_and(|s| s.elapsed() >= Duration::from_secs(2)) {
                Duration::from_secs(1)
            } else {
                Duration::from_millis(100)
            };
            std::thread::sleep(poll_delay);
        }
    }
    #[cfg(not(windows))]
    let _ = install_dir;
    app_id
}

/// Wait for a game: its first process, then (Linux) everything it started
/// that's still in its process group, like Steam's reaper. Launchers
/// (Cities: Skylines' Paradox Launcher, War Thunder's) start the game and
/// exit at once; the game, and the Steam host it needs, must outlive them.
#[cfg(test)]
pub fn wait_all(child: &mut Child) {
    wait_all_inner(child, Instant::now(), None);
}

fn wait_all_inner(child: &mut Child, started: Instant, stop: Option<&std::sync::atomic::AtomicBool>) {
    let _ = child.wait();
    #[cfg(not(windows))]
    {
        // A launcher handing over can leave its group empty for a moment
        // (dowser: none for ~0.1 s, then the Paradox Launcher), so the game
        // only counts as closed once the group stays empty for the adaptive grace.
        let initial_lifetime = started.elapsed();
        let group = child.id();
        let mut logged = false;
        let mut descendant_since: Option<Instant> = None;
        let mut descendant_lifetime: Option<Duration> = None;
        let mut empty_since: Option<Instant> = None;
        loop {
            if group_alive(group) {
                if !logged {
                    tracing::info!("process {group} exited; waiting for the processes it started");
                    logged = true;
                }
                descendant_since.get_or_insert_with(Instant::now);
                empty_since = None;
            } else {
                if let Some(since) = descendant_since.take() {
                    let dur = since.elapsed();
                    descendant_lifetime = Some(descendant_lifetime.map_or(dur, |prev| prev.max(dur)));
                }
                let stopped = stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Acquire));
                let grace = post_exit_grace(initial_lifetime, descendant_lifetime, stopped);
                if grace.is_zero() || empty_since.get_or_insert_with(Instant::now).elapsed() >= grace {
                    break;
                }
            }
            let stopped = stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Acquire));
            let poll_delay = if !stopped && empty_since.is_none() && descendant_since.is_some_and(|s| s.elapsed() >= Duration::from_secs(2)) {
                Duration::from_secs(1)
            } else {
                Duration::from_millis(100)
            };
            std::thread::sleep(poll_delay);
        }
    }
    #[cfg(windows)]
    let _ = (started, stop);
}

/// Any live (not zombie) process in process group `pgid`?
#[cfg(not(windows))]
fn group_alive(pgid: u32) -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else { return false };
    dir.flatten().filter(|e| e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))).any(|e| {
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { return false };
        // `pid (comm) state ppid pgrp …`; comm may hold spaces or parentheses.
        let Some((_, rest)) = stat.rsplit_once(')') else { return false };
        let mut f = rest.split_whitespace();
        let state = f.next();
        let pgrp = f.nth(1).and_then(|v| v.parse::<u32>().ok());
        pgrp == Some(pgid) && state != Some("Z") && state != Some("X")
    })
}

// ------------------------------------------------- the registry swap

/// `HKCU\Software\Valve\Steam\ActiveProcess`: steam_api reads the steamclient
/// path (`SteamClientDll64`) and Steam's process id (`pid`, must be alive)
/// from here.
const ACTIVE: &str = "Software\\Valve\\Steam\\ActiveProcess";

#[derive(Debug, Clone, Default)]
struct Saved {
    pid: Option<u32>,
    user: Option<u32>,
    dll64: Option<String>,
    /// `SteamClientDll`, the one 32-bit games load.
    dll32: Option<String>,
}

struct Swap {
    saved: Saved,
    /// What we wrote, so we only put back values nobody changed since
    /// (the official client rewrites them when it starts).
    ours: Saved,
    /// Launches whose game hasn't loaded the DLL yet.
    pending: std::collections::HashSet<String>,
    /// Launches still running.
    running: std::collections::HashSet<String>,
}

static SWAP: Mutex<Option<Swap>> = Mutex::new(None);

/// Point steam_api at our 64-bit and/or 32-bit DLLs for one launch so a 32-bit
/// launcher spawning a 64-bit game (or vice versa) still loads Aurora's DLL.
/// Any partial registry writes are rolled back if a subsequent write fails.
pub fn swap_begin_both(token: &str, dll64: Option<&Path>, dll32: Option<&Path>, account_id: u32) -> Result<(), String> {
    // Linux: Proton's own prefix registry already points at the DLL.
    if cfg!(not(windows)) {
        return Ok(());
    }
    if dll64.is_none() && dll32.is_none() {
        return Err("steamclient DLL missing".into());
    }
    let mut swap = SWAP.lock().unwrap();
    let created = swap.is_none();
    let state = swap.get_or_insert_with(|| Swap {
        saved: Saved {
            pid: win::reg_dword(ACTIVE, "pid"),
            user: win::reg_dword(ACTIVE, "ActiveUser"),
            dll64: win::reg_string(ACTIVE, "SteamClientDll64"),
            dll32: win::reg_string(ACTIVE, "SteamClientDll"),
        },
        ours: Saved::default(),
        pending: Default::default(),
        running: Default::default(),
    });
    let pid = std::process::id();
    let apply = (|| -> Result<(), String> {
        if let Some(d64) = dll64 {
            let s = d64.to_string_lossy().into_owned();
            win::set_string(ACTIVE, "SteamClientDll64", &s)?;
            state.ours.dll64 = Some(s);
        }
        if let Some(d32) = dll32 {
            let s = d32.to_string_lossy().into_owned();
            win::set_string(ACTIVE, "SteamClientDll", &s)?;
            state.ours.dll32 = Some(s);
        }
        win::set_dword(ACTIVE, "pid", pid)?;
        state.ours.pid = Some(pid);
        win::set_dword(ACTIVE, "ActiveUser", account_id)?;
        state.ours.user = Some(account_id);
        Ok(())
    })();
    if let Err(e) = apply {
        if created {
            restore_all(state);
            *swap = None;
        }
        return Err(e);
    }
    state.pending.insert(token.to_string());
    state.running.insert(token.to_string());
    Ok(())
}

/// The game loaded our DLL (or never will): put the DLL path back right
/// away so nothing else picks it up. Steam's process id stays ours while
/// our games run, unless the official client is running.
pub fn swap_loaded(token: &str) {
    let mut swap = SWAP.lock().unwrap();
    let Some(s) = swap.as_mut() else { return };
    if !s.pending.remove(token) || !s.pending.is_empty() {
        return;
    }
    restore_dll(s);
}

/// One of our games closed.
pub fn swap_exited(token: &str) {
    let mut swap = SWAP.lock().unwrap();
    let Some(s) = swap.as_mut() else { return };
    s.pending.remove(token);
    s.running.remove(token);
    if s.pending.is_empty() {
        restore_dll(s);
    }
    if s.running.is_empty() {
        restore_all(s);
        *swap = None;
    }
}

/// Undo everything now (before the official client launches a game).
pub fn swap_restore_now() {
    let mut swap = SWAP.lock().unwrap();
    if let Some(s) = swap.as_mut() {
        restore_all(s);
    }
    *swap = None;
}

fn restore_dll(s: &Swap) {
    if s.ours.dll64.is_some() && win::reg_string(ACTIVE, "SteamClientDll64") == s.ours.dll64 {
        put_back_string("SteamClientDll64", &s.saved.dll64);
    }
    if s.ours.dll32.is_some() && win::reg_string(ACTIVE, "SteamClientDll") == s.ours.dll32 {
        put_back_string("SteamClientDll", &s.saved.dll32);
    }
    if win::reg_dword(ACTIVE, "ActiveUser") == s.ours.user {
        put_back_dword("ActiveUser", s.saved.user);
    }
    // The official client is running: its process id goes back too.
    if s.saved.pid.is_some_and(win::is_steam_process) && win::reg_dword(ACTIVE, "pid") == s.ours.pid {
        put_back_dword("pid", s.saved.pid);
    }
}

fn restore_all(s: &Swap) {
    restore_dll(s);
    if win::reg_dword(ACTIVE, "pid") == s.ours.pid {
        put_back_dword("pid", s.saved.pid);
    }
}

fn put_back_string(name: &str, v: &Option<String>) {
    let _ = match v {
        Some(v) => win::set_string(ACTIVE, name, v),
        None => win::delete_value(ACTIVE, name),
    };
}

fn put_back_dword(name: &str, v: Option<u32>) {
    let _ = match v {
        Some(v) => win::set_dword(ACTIVE, name, v),
        None => win::delete_value(ACTIVE, name),
    };
}

/// After a crash, the registry may still point at our DLL: put Valve's
/// back (only values whose process is gone).
pub fn repair_registry() {
    // Sign-in settings left from an earlier official-Steam launch.
    restore_login_later();
    if SWAP.lock().unwrap().is_some() {
        return;
    }
    let Some(root) = win::reg_string("Software\\Valve\\Steam", "SteamPath") else { return };
    let root = PathBuf::from(root.replace('/', "\\"));
    let pid = win::reg_dword(ACTIVE, "pid").unwrap_or(0);
    if win::is_alive(pid) {
        return;
    }
    let mut fixed = false;
    for (value, file) in [("SteamClientDll64", "steamclient64.dll"), ("SteamClientDll", "steamclient.dll")] {
        let Some(dll) = win::reg_string(ACTIVE, value) else { continue };
        let in_steam = dll.to_lowercase().starts_with(&root.to_string_lossy().to_lowercase());
        if !in_steam {
            let _ = win::set_string(ACTIVE, value, &root.join(file).to_string_lossy());
            fixed = true;
        }
    }
    if fixed {
        tracing::info!("putting Valve's steamclient back in the registry");
        let _ = win::set_dword(ACTIVE, "pid", 0);
        let _ = win::set_dword(ACTIVE, "ActiveUser", 0);
    }
}

/// Is the game's executable 64-bit? (`None`: no executable / not a program.)
pub fn exe_is_64bit(app: &OwnedApp, install_dir: &Path) -> Option<bool> {
    #[cfg(not(windows))]
    return pe_is_64bit(&linux::plan(app, install_dir).ok()?.exe(install_dir));
    #[allow(unreachable_code)]
    pe_is_64bit(&executable(app, install_dir)?)
}

pub fn reg_string(sub: &str, value: &str) -> Option<String> {
    win::reg_string(sub, value)
}

// -------------------------------------------- the official Steam client

/// Where the official client is, from its own registry entry.
#[cfg(windows)]
pub fn steam_exe() -> Option<PathBuf> {
    let p = win::reg_string("Software\\Valve\\Steam", "SteamExe")?;
    let p = PathBuf::from(p.replace('/', "\\"));
    p.is_file().then_some(p)
}

/// `steam` on the PATH, else the launcher script in the Steam folder.
#[cfg(not(windows))]
pub fn steam_exe() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join("steam")).find(|p| p.is_file()));
    on_path.or_else(|| local::find_steam_root().map(|r| r.join("steam.sh")).filter(|p| p.is_file()))
}

fn steam_running() -> bool {
    win::reg_dword(ACTIVE, "pid").is_some_and(win::is_steam_process) || win::steam_pid_file().is_some_and(win::is_steam_process)
}

fn signed_in() -> bool {
    // Linux Steam may not publish ActiveUser; a running client then counts
    // (it queues -applaunch until it has signed in).
    steam_running() && win::reg_dword(ACTIVE, "ActiveUser").map_or(!cfg!(windows), |u| u != 0)
}

fn app_running(app_id: AppId) -> bool {
    win::reg_dword(&format!("Software\\Valve\\Steam\\Apps\\{app_id}"), "Running").unwrap_or(0) == 1 || win::steam_launch_running(app_id)
}

fn wait_until_every(limit: Duration, poll: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if f() {
            return true;
        }
        std::thread::sleep(poll);
    }
    f()
}

fn wait_until(limit: Duration, f: impl FnMut() -> bool) -> bool {
    wait_until_every(limit, Duration::from_millis(150), f)
}

/// Run a game through the official client (started with `-silent` if it
/// isn't running) and block until the game closes. `started` is called once
/// Steam reports the game running. `steam_id` is the Aurora account:
/// Steam signs in to it by itself when it remembers it. With `offline`,
/// Steam starts in its own offline mode (if it isn't running already).
/// Errors are for the player to read.
pub fn run_through_steam(app_id: AppId, args: &LaunchArgs, steam_id: u64, offline: bool, started: impl FnOnce()) -> Result<(), String> {
    let connect = safe_connect_args(&args.connect)?;
    swap_restore_now();
    // However this ends, Steam's sign-in settings go back once it closes.
    struct RestoreLogin;
    impl Drop for RestoreLogin {
        fn drop(&mut self) {
            restore_login_later();
        }
    }
    let _restore = RestoreLogin;
    let exe = steam_exe().ok_or_else(|| "This game runs through the official Steam client, which isn't installed. Install Steam from store.steampowered.com, sign in, then press Play again.".to_string())?;
    if offline && !steam_running() && remembered_account(steam_id).is_none() {
        return Err("This game runs through the official Steam client. Steam can only start offline for an account it remembers: sign in to Steam once with \"Remember me\" while online.".into());
    }
    // Steam started here only for this game closes again with it.
    let ours = !steam_running();
    if ours {
        start_steam(&exe, steam_id, offline)?;
    }
    if !wait_until(Duration::from_secs(20), signed_in) && win::reg_dword(ACTIVE, "ActiveUser").unwrap_or(0) == 0 {
        // Waiting at the account picker or the sign-in window: nobody is
        // signed in, so restarting it signed in to this account loses nothing.
        if remembered_account(steam_id).is_some() {
            tracing::info!("Steam is waiting for an account; restarting it signed in to this one");
            quiet(Command::new(&exe).arg("-shutdown")).spawn().map_err(|e| format!("Couldn't restart Steam: {e}"))?;
            wait_until(Duration::from_secs(60), || !steam_running());
            start_steam(&exe, steam_id, offline)?;
        }
    }
    if !wait_until(Duration::from_secs(60), signed_in) {
        return Err("Steam is running but not signed in. Sign in to the Steam window, then press Play again.".into());
    }
    restore_login_user();
    // Steam starts the game itself: it only takes arguments, its own launch
    // options (Properties in Steam) do the rest.
    let opts = UserOptions::parse(&args.user);
    if !opts.args_only() {
        tracing::warn!("launch options: variables and wrappers don't apply when the official Steam client starts the game; set them in Steam's launch options");
    }
    let mut cmd = Command::new(&exe);
    cmd.arg("-applaunch").arg(app_id.to_string()).args(&opts.args).args(&connect);
    quiet(&mut cmd).spawn().map_err(|e| format!("Couldn't ask Steam to start the game: {e}"))?;
    // Steam may update the game first; give it a while.
    if !wait_until(Duration::from_secs(600), || app_running(app_id)) {
        return Err("Steam didn't start the game. Check the Steam window for a message.".into());
    }
    started();
    while app_running(app_id) {
        std::thread::sleep(Duration::from_millis(250));
    }
    if ours {
        tracing::info!("the game closed; shutting down the Steam client started for it");
        let _ = quiet(Command::new(&exe).arg("-shutdown")).spawn();
    }
    Ok(())
}

fn start_steam(exe: &Path, steam_id: u64, offline: bool) -> Result<(), String> {
    match auto_login(steam_id, offline) {
        Some(account) => tracing::info!("starting the official Steam client in the background, signed in as {account}"),
        None => tracing::info!("starting the official Steam client in the background (it doesn't remember this account)"),
    }
    // Only there for the game (anti-cheat, Steam DRM): no windows, and its
    // web layer kept small (about 500 MB in all with Teardown running).
    let lean = ["-silent", "-nofriendsui", "-nochatui", "-cef-single-process", "-cef-disable-gpu", "-cef-disable-gpu-compositing"];
    quiet(Command::new(exe).args(lean)).spawn().map_err(|e| format!("Couldn't start Steam: {e}"))?;
    if !wait_until(Duration::from_secs(90), steam_running) {
        return Err("Steam didn't start. Open Steam yourself, then press Play again.".into());
    }
    Ok(())
}

// ------------------------------------------------------------ headless host

/// The `steamhost` helper: shipped next to the app, or (in dev) found in the
/// build output or named by `STEAMHOST_BIN`.
fn steamhost_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("STEAMHOST_BIN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    // Beside the app (in dev, both are in target/<profile>/).
    let beside = std::env::current_exe().ok()?.parent()?.join(format!("steamhost{}", std::env::consts::EXE_SUFFIX));
    beside.is_file().then_some(beside)
}

/// The headless host can run games here: we have the helper and a
/// Steam install with the engine it loads.
fn headless_host_available() -> bool {
    steamhost_bin().is_some() && local::find_steam_root().is_some_and(|r| r.join(ENGINE_LIB).is_file())
}

/// Valve's engine library in the Steam folder, which the host loads.
const ENGINE_LIB: &str = if cfg!(windows) { "steamclient64.dll" } else { "linux64/steamclient.so" };

/// The official client is open, so a second global instance can't start.
fn official_steam_open() -> bool {
    #[cfg(windows)]
    return win::steam_exe_running();
    #[cfg(not(windows))]
    steam_running()
}

pub use local::steam_settings::Setting as SteamSetting;

/// Lifecycle of the single global headless Steam host.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HostLifecycle {
    Idle,
    Running,
    Stopping,
}

static HOST_STATE: std::sync::LazyLock<(Mutex<HostLifecycle>, std::sync::Condvar)> =
    std::sync::LazyLock::new(|| (Mutex::new(HostLifecycle::Idle), std::sync::Condvar::new()));

/// Why Steam's own settings can't be changed right now: Steam (the official
/// client or the headless host) is running and would overwrite them.
pub fn steam_settings_locked() -> Option<&'static str> {
    if *HOST_STATE.0.lock().unwrap_or_else(|e| e.into_inner()) != HostLifecycle::Idle {
        Some("Steam is running for a game. Change these after it closes.")
    } else if official_steam_open() {
        Some("Close the official Steam client to change these.")
    } else {
        None
    }
}

/// Cached reads of Steam's settings (the Settings page asks every frame).
static SETTINGS_CACHE: Mutex<Option<(std::time::Instant, Vec<(SteamSetting, Option<bool>)>)>> = Mutex::new(None);

/// One of Steam's own settings, `None` without a Steam install.
pub fn steam_setting(s: SteamSetting) -> Option<bool> {
    let mut cache = SETTINGS_CACHE.lock().unwrap();
    if cache.as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(2)) {
        let root = local::find_steam_root();
        let all = [SteamSetting::DownloadsDuringGameplay, SteamSetting::ShaderPreCaching, SteamSetting::ShaderBackgroundProcessing];
        *cache = Some((std::time::Instant::now(), all.iter().map(|&s| (s, root.as_deref().and_then(|r| local::steam_settings::get(r, s)))).collect()));
    }
    cache.as_ref().and_then(|(_, v)| v.iter().find(|(k, _)| *k == s)).and_then(|(_, v)| *v)
}

/// Change one of Steam's own settings (only while Steam isn't running).
pub fn set_steam_setting(s: SteamSetting, on: bool) -> Result<(), String> {
    if let Some(why) = steam_settings_locked() {
        return Err(why.into());
    }
    let root = local::find_steam_root().ok_or("no Steam install found")?;
    local::steam_settings::set(&root, s, on)?;
    *SETTINGS_CACHE.lock().unwrap() = None;
    Ok(())
}

/// The player's choice for Steam's "Allow downloads during gameplay", which
/// Aurora applies before each game (off by default: with it on, Valve's
/// engine keeps starting and suspending updates and shader downloads while
/// the game runs, a few hundred MB on the headless host).
static GAMEPLAY_DOWNLOADS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_gameplay_downloads(on: bool) {
    GAMEPLAY_DOWNLOADS.store(on, std::sync::atomic::Ordering::Relaxed);
    crate::preferences_changed();
}

pub fn gameplay_downloads() -> bool {
    GAMEPLAY_DOWNLOADS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Before each game starts: put Steam's download and shader settings the way
/// the player chose them in Aurora. Downloads during gameplay as chosen;
/// Steam's background shader processing off while Aurora compiles
/// shaders before launch (so the engine doesn't do the same work again while
/// the game runs). Shader pre-caching (the downloads) is left as it is.
/// Skipped while Steam is running, since it would overwrite the file.
pub fn apply_steam_prefs() {
    if steam_settings_locked().is_some() {
        return;
    }
    let Some(root) = local::find_steam_root() else { return };
    let mut wanted = vec![(SteamSetting::DownloadsDuringGameplay, gameplay_downloads())];
    #[cfg(not(windows))]
    if crate::shaders::precompile() {
        wanted.push((SteamSetting::ShaderBackgroundProcessing, false));
    }
    for (s, on) in wanted {
        match local::steam_settings::set(&root, s, on) {
            Ok(true) => tracing::info!("Steam setting {s:?} set to {on}"),
            Ok(false) => {}
            Err(e) => tracing::warn!("couldn't set Steam's {s:?}: {e}"),
        }
    }
    *SETTINGS_CACHE.lock().unwrap() = None;
}

/// Tracks the active headless host lifecycle in [`HOST_STATE`] while alive.
struct HostCount;

impl HostCount {
    fn acquire() -> Result<Self, &'static str> {
        let (lock, cv) = &*HOST_STATE;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            match *state {
                HostLifecycle::Idle => {
                    *state = HostLifecycle::Running;
                    return Ok(Self);
                }
                HostLifecycle::Running => {
                    return Err("Close the game using the windowless Steam host before starting another.");
                }
                HostLifecycle::Stopping => {
                    let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                        return Err("Previous headless Steam host is still shutting down; try again in a moment.");
                    };
                    let (next, _) = cv.wait_timeout(state, rem).unwrap_or_else(|e| e.into_inner());
                    state = next;
                }
            }
        }
    }

    fn begin_stopping(&self) {
        let (lock, _) = &*HOST_STATE;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        if *state == HostLifecycle::Running {
            *state = HostLifecycle::Stopping;
        }
    }
}

impl Drop for HostCount {
    fn drop(&mut self) {
        let (lock, cv) = &*HOST_STATE;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = HostLifecycle::Idle;
        cv.notify_all();
    }
}

/// Printed by `steamhost serve` once it is signed in and serving.
const STEAMHOST_READY: &str = "STEAMHOST-READY";

fn read_steamhost_messages(mut reader: impl std::io::BufRead, ready: impl FnOnce(), mut profile: impl FnMut(u64)) {
    let mut ready = Some(ready);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).is_ok_and(|len| len > 0) {
        let parsed = std::str::from_utf8(&line).unwrap_or_default();
        let parsed = parsed.trim_end_matches(['\r', '\n']);
        if parsed.trim() == STEAMHOST_READY {
            if let Some(ready) = ready.take() {
                ready();
            }
        } else if ready.is_none()
            && let Some(target) = parsed.strip_prefix("STEAMHOST-PROFILE ").and_then(|id| id.parse::<u64>().ok())
            && target >> 52 == 0x11
            && target as u32 != 0
        {
            profile(target);
        }
        line.clear();
    }
}

/// Run a game on a headless instance of the user's own Steam engine.
///
/// Starts `steamhost serve` (signed in with the user's refresh token, fed on
/// stdin — never argv/env), waits until it is ready, launches the game through
/// Proton with lsteamclient left on (Linux) or natively with Steam's registry
/// entry naming the host (Windows) so Valve's library inside the game reaches
/// the host, waits for the game to close, then drops the pipe so the host
/// shuts down. The installed Valve engine performs its own ownership check.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn run_through_host(
    app: &OwnedApp,
    install_dir: &Path,
    args: &LaunchArgs,
    account: &str,
    token: &str,
    steam_id: u64,
    overlay: Option<(u16, &str)>,
    ignore_physical_pads: bool,
    started: impl FnOnce(),
) -> Result<(), String> {
    run_through_host_cancellable(app, install_dir, args, account, token, steam_id, overlay, ignore_physical_pads, None, started, |_| {})
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_through_host_cancellable(
    app: &OwnedApp,
    install_dir: &Path,
    args: &LaunchArgs,
    account: &str,
    token: &str,
    steam_id: u64,
    overlay: Option<(u16, &str)>,
    ignore_physical_pads: bool,
    stop: Option<&std::sync::atomic::AtomicBool>,
    started: impl FnOnce(),
    profile: impl FnMut(u64) + Send + 'static,
) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc;

    let connect = safe_connect_args(&args.connect)?;
    let bin = steamhost_bin().ok_or("the steamhost helper is missing")?;
    let root = local::find_steam_root().ok_or("no Steam install found for the headless host")?;

    let mut cmd = Command::new(&bin);
    // Valve's switches for no assert minidumps, crash dialog or monitor.
    cmd.arg("serve").arg(&root).args(["-noassert", "-nocrashdialog", "-nocrashmonitor"]);
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let disable_profile_forwarding = std::env::var_os("AURORA_STEAMHOST_PROFILE_FORWARDING").is_some_and(|value| value == "0");
    let forward_profiles = cfg!(all(windows, target_arch = "x86_64")) && app_overlay_enabled(app.app_id) && !disable_profile_forwarding;
    if disable_profile_forwarding {
        tracing::info!("steamhost profile forwarding disabled for this launch");
    }
    cmd.env("AURORA_STEAMHOST_PROFILE_FORWARDING", if forward_profiles { "1" } else { "0" });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let counted = HostCount::acquire()?;
    let mut host = cmd.spawn().map_err(|e| format!("couldn't start the headless Steam host: {e}"))?;

    // Its diagnostics go to our log; the last line explains a failure.
    let last_err = std::sync::Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = host.stderr.take() {
        let last = last_err.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(reason) = line.strip_prefix("profile forwarding unavailable: ") {
                    tracing::warn!("steamhost profile forwarding unavailable: {reason}");
                } else if line == "profile forwarding enabled" {
                    tracing::info!("steamhost profile forwarding enabled");
                } else {
                    tracing::debug!("steamhost: {line}");
                }
                *last.lock().unwrap() = line;
            }
        });
    }

    // Credentials over the pipe, which we hold open to keep the host alive.
    let mut stdin = host.stdin.take().ok_or("host stdin unavailable")?;
    stdin.write_all(format!("{account}\n{token}\n{steam_id}\n").as_bytes()).map_err(|e| format!("couldn't send credentials to the host: {e}"))?;
    stdin.flush().ok();

    // Keep reading after the ready marker so the running host can send profile requests.
    let stdout = host.stdout.take().ok_or("host stdout unavailable")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        read_steamhost_messages(BufReader::new(stdout), || {
            let _ = tx.send(true);
        }, profile);
    });

    let wait_start = Instant::now();
    let mut ready = false;
    let mut cancelled = false;
    while wait_start.elapsed() < Duration::from_secs(90) {
        if stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Acquire)) {
            cancelled = true;
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(v) => {
                ready = v;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if cancelled {
        drop(stdin);
        let _ = host.kill();
        let _ = host.wait();
        return Ok(());
    }
    if !ready {
        let _ = host.kill();
        let _ = host.wait();
        let why = last_err.lock().unwrap().trim_start_matches("steamhost: ").to_string();
        tracing::warn!("headless Steam host failed: {why}");
        return Err(if why.starts_with("Steam is running") {
            "Close the official Steam first: only one Steam can run at a time.".into()
        } else {
            format!("the headless Steam host didn't sign in ({why}). Check that this account is signed in to Aurora, then press Play again.")
        });
    }
    // Windows: steam_api finds Steam through this registry entry; point it at
    // the host (and Valve's own DLLs) while the game runs.
    #[cfg(windows)]
    let active = HostRegistry::point_at(host.id(), &root, model::account_id(steam_id));
    // Linux: newer steam_api checks that ~/.steam/steam.pid names a live
    // process before it connects ("did not locate a running instance of
    // Steam": the Paradox Launcher of Cities: Skylines); name the host.
    #[cfg(not(windows))]
    let pid_file = HostPidFile::point_at(host.id());
    tracing::info!("headless Steam host ready; launching {}", app.name);

    // Launch the game through Proton with lsteamclient ON (the default), so the
    // game's steamclient reaches the host. `command` already points
    // STEAM_COMPAT_CLIENT_INSTALL_PATH at the real Steam root.
    #[cfg(windows)]
    match crate::pads::native_xinput_slots() {
        Some(mask) => tracing::info!("controllers: headless launch, native XInput slot mask {mask:#06b} (bit 0 = player 1)"),
        None => tracing::warn!("controllers: headless launch, XInputGetState unavailable"),
    }
    let game = spawn_for_host(app, install_dir, &args.user, &connect, account, overlay, ignore_physical_pads);
    let child = match game {
        Ok(c) => c,
        Err(e) => {
            drop(stdin);
            let _ = host.wait();
            return Err(e);
        }
    };
    started();
    wait_in_dir_cancellable(app.app_id, install_dir, child, stop);

    // Game closed: drop the pipe so the host signs out and exits, and reap it
    // asynchronously so the launcher's Play/Stop state updates right away.
    tracing::info!("{} closed; shutting down the headless Steam host", app.name);
    counted.begin_stopping();
    drop(stdin);
    let reap = move || {
        let _counted = counted;
        #[cfg(windows)]
        let _active = active;
        #[cfg(not(windows))]
        let _pid_file = pid_file;
        if !wait_until_every(Duration::from_secs(5), Duration::from_millis(50), || host.try_wait().ok().flatten().is_some()) {
            let _ = host.kill();
        }
        let _ = host.wait();
    };
    if std::thread::Builder::new().name("steamhost-reap".into()).spawn(reap).is_err() {
        // Fallback if thread creation fails: clean up synchronously (rare).
        tracing::warn!("couldn't spawn steamhost-reap thread");
    }
    Ok(())
}

/// The game launch for the headless-host path: the normal command (Proton on
/// Linux, with lsteamclient left enabled) plus the `Steam*` environment a
/// game expects.
fn spawn_for_host(
    app: &OwnedApp,
    install_dir: &Path,
    user_args: &str,
    connect: &[String],
    account: &str,
    overlay: Option<(u16, &str)>,
    ignore_physical_pads: bool,
) -> Result<Child, String> {
    let opts = UserOptions::parse(user_args);
    let mut cmd = command(app, install_dir, &opts, connect)?;
    let id = app.app_id.to_string();
    cmd.env("SteamOverlayGameId", &id)
        .env("SteamClientLaunch", "1")
        .env("SteamEnv", "1")
        .env("SteamAppUser", account)
        .env("SteamUser", account);
    let mut cmd = finish(app, install_dir, &opts, cmd);
    apply_sdl_controller_env(&mut cmd, ignore_physical_pads);
    #[cfg(target_os = "linux")]
    {
        apply_vk_overlay_env(&mut cmd, overlay, app_overlay_enabled(app.app_id));
        apply_html_shim_env(&mut cmd, app, install_dir);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = overlay;
    cmd.spawn().map_err(|e| format!("could not start {}: {e}", app.name))
}

static OVERLAY_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static OVERLAY_DISABLED_APPS: Mutex<std::collections::BTreeSet<AppId>> = Mutex::new(std::collections::BTreeSet::new());
static OVERLAY_HOTKEY_ENV: Mutex<Option<String>> = Mutex::new(None);
static SCREENSHOT_HOTKEY_ENV: Mutex<Option<String>> = Mutex::new(None);

pub fn set_overlay_enabled(on: bool) {
    OVERLAY_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn overlay_enabled() -> bool {
    OVERLAY_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_overlay_disabled_apps(apps: std::collections::BTreeSet<AppId>) {
    *OVERLAY_DISABLED_APPS.lock().unwrap() = apps;
}

pub fn set_app_overlay(app: AppId, on: bool) {
    let mut disabled = OVERLAY_DISABLED_APPS.lock().unwrap();
    if on {
        disabled.remove(&app);
    } else {
        disabled.insert(app);
    }
}

pub fn app_overlay_enabled(app: AppId) -> bool {
    overlay_enabled() && !OVERLAY_DISABLED_APPS.lock().unwrap().contains(&app)
}

pub fn set_overlay_hotkeys(overlay_mods: u8, overlay_vk: u32, shot_mods: u8, shot_vk: u32) {
    *OVERLAY_HOTKEY_ENV.lock().unwrap() = Some(format!("{overlay_mods}:{overlay_vk}"));
    *SCREENSHOT_HOTKEY_ENV.lock().unwrap() = Some(format!("{shot_mods}:{shot_vk}"));
}

fn apply_hotkey_env(cmd: &mut Command) {
    if let Some(v) = OVERLAY_HOTKEY_ENV.lock().unwrap().as_deref() {
        cmd.env("AURORA_OVERLAY_HOTKEY", v);
    }
    if let Some(v) = SCREENSHOT_HOTKEY_ENV.lock().unwrap().as_deref() {
        cmd.env("AURORA_SCREENSHOT_KEY", v);
    }
}

#[cfg(target_os = "linux")]
fn vk_overlay_lib() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("STEAMINRUST_VK_OVERLAY_LIB").filter(|s| !s.is_empty()).map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(std::fs::canonicalize(&p).unwrap_or(p));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for candidate_dir in [Some(dir), dir.parent()] {
        let Some(d) = candidate_dir else { continue };
        let lib = d.join("libVkLayer_aurora_overlay.so");
        if lib.is_file() {
            return Some(std::fs::canonicalize(&lib).unwrap_or(lib));
        }
    }
    None
}

/// Does the game executable request Steam's `ISteamHTMLSurface` interface?
/// Under `steamhost` (no `steamwebhelper`), `ISteamHTMLSurface::CreateBrowser`
/// never delivers `HTML_BrowserReady_t`, causing games like Microsoft Flight
/// Simulator (2020) to hang on their splash screen.
#[cfg(target_os = "linux")]
fn uses_steam_html_surface(path: &Path) -> bool {
    use std::io::Read;
    const NEEDLE: &[u8] = b"STEAMHTMLSURFACE_INTERFACE_VERSION";
    const CHUNK: usize = 64 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let overlap = NEEDLE.len() - 1;
    let mut buf = vec![0u8; overlap + CHUNK];
    let mut carry = 0usize;
    loop {
        let Ok(n) = f.read(&mut buf[carry..]) else { return false };
        if n == 0 {
            return false;
        }
        let end = carry + n;
        if contains_subslice(&buf[..end], NEEDLE) {
            return true;
        }
        if end >= overlap {
            buf.copy_within(end - overlap..end, 0);
            carry = overlap;
        } else {
            carry = end;
        }
    }
}

#[cfg(target_os = "linux")]
fn contains_subslice(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if hay.len() < needle.len() {
        return false;
    }
    let first = needle[0];
    let limit = hay.len() - needle.len() + 1;
    let mut i = 0;
    while i < limit {
        let Some(pos) = hay[i..limit].iter().position(|&b| b == first) else {
            return false;
        };
        let at = i + pos;
        if &hay[at..at + needle.len()] == needle {
            return true;
        }
        i = at + 1;
    }
    false
}

#[cfg(target_os = "linux")]
fn html_shim_lib() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AURORA_HTML_SHIM_LIB").filter(|s| !s.is_empty()).map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(std::fs::canonicalize(&p).unwrap_or(p));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for candidate_dir in [Some(dir), dir.parent()] {
        let Some(d) = candidate_dir else { continue };
        let lib = d.join("libaurora_htmlsurface.so");
        if lib.is_file() {
            return Some(std::fs::canonicalize(&lib).unwrap_or(lib));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn apply_html_shim_env_with(cmd: &mut Command, lib: &Path) {
    let canon_lib = std::fs::canonicalize(lib).unwrap_or_else(|_| lib.to_path_buf());
    let preload = append_colon_paths(existing_cmd_env(cmd, "LD_PRELOAD").as_deref(), &[&canon_lib]);
    cmd.env("LD_PRELOAD", preload);
    if let Some(lib_dir) = canon_lib.parent().filter(|p| !p.as_os_str().is_empty() && *p != Path::new("/") && *p != Path::new("/usr")) {
        let pv_ro = append_colon_paths(existing_cmd_env(cmd, "PRESSURE_VESSEL_FILESYSTEMS_RO").as_deref(), &[lib_dir]);
        if !pv_ro.is_empty() {
            cmd.env("PRESSURE_VESSEL_FILESYSTEMS_RO", pv_ro);
        }
    }
    tracing::info!("enabling headless ISteamHTMLSurface shim ({})", canon_lib.display());
}

#[cfg(target_os = "linux")]
fn apply_html_shim_env(cmd: &mut Command, app: &OwnedApp, install_dir: &Path) {
    match std::env::var("AURORA_HTML_SHIM").as_deref() {
        Ok("0") => return,
        Ok("1") => {}
        _ => {
            if !launch_exe(app, install_dir).is_some_and(|exe| uses_steam_html_surface(&exe)) {
                return;
            }
        }
    }
    let Some(lib) = html_shim_lib() else {
        tracing::warn!("headless ISteamHTMLSurface shim unavailable: libaurora_htmlsurface.so not found beside aurora executable");
        return;
    };
    apply_html_shim_env_with(cmd, &lib);
}

#[cfg(target_os = "linux")]
fn vk_layer_manifest_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(xdg).join("vulkan").join("implicit_layer.d");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(home).join(".local").join("share").join("vulkan").join("implicit_layer.d");
    }
    directories::ProjectDirs::from("", "", "aurora")
        .map(|p| p.cache_dir().join("vk_layer"))
        .unwrap_or_else(|| std::env::temp_dir().join("aurora-vk-layer"))
}

#[cfg(target_os = "linux")]
fn write_vk_layer_manifest_in(dir: &Path, lib_path: &Path) -> Option<PathBuf> {
    let canon_lib = std::fs::canonicalize(lib_path).unwrap_or_else(|_| lib_path.to_path_buf());
    std::fs::create_dir_all(dir).ok()?;
    let canon_dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let manifest_path = canon_dir.join("VkLayer_aurora_overlay.json");
    let lib_str = serde_json::to_string(&canon_lib.to_string_lossy()).ok()?;
    let json = format!(
        r#"{{
  "file_format_version": "1.1.2",
  "layer": {{
    "name": "VK_LAYER_AURORA_overlay",
    "type": "GLOBAL",
    "library_path": {lib_str},
    "api_version": "1.3.0",
    "implementation_version": "1",
    "description": "Aurora in-game Vulkan overlay",
    "enable_environment": {{
      "AURORA_VK_OVERLAY": "1"
    }},
    "disable_environment": {{
      "DISABLE_AURORA_OVERLAY": "1"
    }}
  }}
}}
"#
    );
    if std::fs::read_to_string(&manifest_path).ok().as_deref() != Some(&json) {
        std::fs::write(&manifest_path, json).ok()?;
    }
    Some(canon_dir)
}

#[cfg(target_os = "linux")]
fn write_vk_layer_manifest(lib_path: &Path) -> Option<PathBuf> {
    let primary = vk_layer_manifest_dir();
    if let Some(dir) = write_vk_layer_manifest_in(&primary, lib_path) {
        return Some(dir);
    }
    let fallback = directories::ProjectDirs::from("", "", "aurora")
        .map(|p| p.cache_dir().join("vk_layer"))
        .unwrap_or_else(|| std::env::temp_dir().join("aurora-vk-layer"));
    write_vk_layer_manifest_in(&fallback, lib_path)
}

#[cfg(target_os = "linux")]
fn existing_cmd_env(cmd: &Command, key: &str) -> Option<String> {
    if let Some((_, val)) = cmd.get_envs().find(|(k, _)| *k == std::ffi::OsStr::new(key)) {
        return val.map(|v| v.to_string_lossy().into_owned());
    }
    std::env::var(key).ok()
}

#[cfg(target_os = "linux")]
fn append_colon_paths(existing: Option<&str>, add: &[&Path]) -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut push_unique = |s: &str| {
        if !s.is_empty() && !entries.iter().any(|e| e == s) {
            entries.push(s.to_string());
        }
    };
    if let Some(ex) = existing {
        for part in ex.split(':') {
            push_unique(part);
        }
    }
    for path in add {
        push_unique(&path.to_string_lossy());
    }
    entries.join(":")
}

#[cfg(target_os = "linux")]
fn apply_vk_overlay_env_with(cmd: &mut Command, port: u16, token: &str, lib: &Path, manifest_dir: &Path) {
    let canon_lib = std::fs::canonicalize(lib).unwrap_or_else(|_| lib.to_path_buf());
    let canon_manifest = std::fs::canonicalize(manifest_dir).unwrap_or_else(|_| manifest_dir.to_path_buf());
    let vk_layers = append_colon_paths(existing_cmd_env(cmd, "VK_ADD_IMPLICIT_LAYER_PATH").as_deref(), &[&canon_manifest]);

    let mut pv_add: Vec<&Path> = Vec::new();
    if let Some(lib_dir) = canon_lib.parent().filter(|p| !p.as_os_str().is_empty() && *p != Path::new("/") && *p != Path::new("/usr")) {
        pv_add.push(lib_dir);
    }
    if canon_manifest != Path::new("/") && canon_manifest != Path::new("/usr") {
        pv_add.push(&canon_manifest);
    }
    let pv_ro = append_colon_paths(existing_cmd_env(cmd, "PRESSURE_VESSEL_FILESYSTEMS_RO").as_deref(), &pv_add);

    tracing::info!("enabling Linux Vulkan overlay layer ({})", canon_lib.display());
    cmd.env("VK_ADD_IMPLICIT_LAYER_PATH", vk_layers)
        .env("AURORA_VK_OVERLAY", "1")
        .env("AURORA_OVERLAY_PORT", port.to_string())
        .env("AURORA_OVERLAY_TOKEN", token);
    if !pv_ro.is_empty() {
        cmd.env("PRESSURE_VESSEL_FILESYSTEMS_RO", pv_ro);
    }
    apply_hotkey_env(cmd);
}

#[cfg(target_os = "linux")]
fn apply_vk_overlay_env(cmd: &mut Command, overlay: Option<(u16, &str)>, enabled: bool) {
    if !enabled {
        cmd.env("DISABLE_AURORA_OVERLAY", "1")
            .env("AURORA_DISABLE_OVERLAY", "1")
            .env_remove("AURORA_VK_OVERLAY");
        return;
    }
    let Some((port, token)) = overlay else { return };
    let Some(lib) = vk_overlay_lib() else {
        tracing::warn!("Linux Vulkan overlay unavailable: libVkLayer_aurora_overlay.so not found beside aurora executable");
        return;
    };
    let Some(manifest_dir) = write_vk_layer_manifest(&lib) else {
        tracing::warn!("Linux Vulkan overlay unavailable: could not write VkLayer_aurora_overlay.json");
        return;
    };
    apply_vk_overlay_env_with(cmd, port, token, &lib, &manifest_dir);
}

/// `~/.steam/steam.pid` naming the headless host while it runs; what was
/// there goes back when this drops (unless Steam itself wrote it since).
#[cfg(not(windows))]
struct HostPidFile {
    path: PathBuf,
    saved: Option<String>,
    ours: String,
}

#[cfg(not(windows))]
impl HostPidFile {
    fn point_at(pid: u32) -> Option<HostPidFile> {
        // A running official client keeps its own (the host doesn't start then anyway).
        if win::steam_pid_file().is_some_and(win::is_steam_process) {
            return None;
        }
        HostPidFile::write(win::dot_steam()?.join("steam.pid"), pid)
    }

    fn write(path: PathBuf, pid: u32) -> Option<HostPidFile> {
        let saved = std::fs::read_to_string(&path).ok();
        let _ = std::fs::create_dir_all(path.parent()?);
        let ours = format!("{pid}\n");
        if let Err(e) = std::fs::write(&path, &ours) {
            tracing::warn!("couldn't write {}: {e}", path.display());
            return None;
        }
        Some(HostPidFile { path, saved, ours })
    }
}

#[cfg(not(windows))]
impl Drop for HostPidFile {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).ok().as_deref() != Some(self.ours.as_str()) {
            return;
        }
        let _ = match &self.saved {
            Some(text) => std::fs::write(&self.path, text),
            None => std::fs::remove_file(&self.path),
        };
    }
}

/// Steam's `ActiveProcess` registry entry pointed at the headless host for
/// one game; the values that were there go back when this drops (unless
/// something else changed them since).
#[cfg(windows)]
struct HostRegistry {
    saved: Saved,
    ours: Saved,
}

#[cfg(windows)]
impl HostRegistry {
    fn point_at(pid: u32, root: &Path, account_id: u32) -> HostRegistry {
        let saved = Saved {
            pid: win::reg_dword(ACTIVE, "pid"),
            user: win::reg_dword(ACTIVE, "ActiveUser"),
            dll64: win::reg_string(ACTIVE, "SteamClientDll64"),
            dll32: win::reg_string(ACTIVE, "SteamClientDll"),
        };
        let ours = Saved {
            pid: Some(pid),
            user: Some(account_id),
            dll64: Some(root.join("steamclient64.dll").to_string_lossy().into_owned()),
            dll32: Some(root.join("steamclient.dll").to_string_lossy().into_owned()),
        };
        let _ = win::set_dword(ACTIVE, "pid", pid);
        let _ = win::set_dword(ACTIVE, "ActiveUser", account_id);
        let _ = win::set_string(ACTIVE, "SteamClientDll64", ours.dll64.as_deref().unwrap_or_default());
        let _ = win::set_string(ACTIVE, "SteamClientDll", ours.dll32.as_deref().unwrap_or_default());
        HostRegistry { saved, ours }
    }
}

#[cfg(windows)]
impl Drop for HostRegistry {
    fn drop(&mut self) {
        if win::reg_dword(ACTIVE, "pid") == self.ours.pid {
            put_back_dword("pid", self.saved.pid);
        }
        if win::reg_dword(ACTIVE, "ActiveUser") == self.ours.user {
            put_back_dword("ActiveUser", self.saved.user);
        }
        // Valve's DLLs stay named if nothing was there before.
        if self.saved.dll64.is_some() && win::reg_string(ACTIVE, "SteamClientDll64") == self.ours.dll64 {
            put_back_string("SteamClientDll64", &self.saved.dll64);
        }
        if self.saved.dll32.is_some() && win::reg_string(ACTIVE, "SteamClientDll") == self.ours.dll32 {
            put_back_string("SteamClientDll", &self.saved.dll32);
        }
    }
}

/// Steam's output isn't ours to show.
fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
}

fn steam_config(file: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let root = PathBuf::from(win::reg_string("Software\\Valve\\Steam", "SteamPath")?.replace('/', "\\"));
    #[cfg(not(windows))]
    let root = local::find_steam_root()?;
    Some(root.join("config").join(file))
}

/// The account's login name, if the official client remembers its login
/// ("Remember me" in `config/loginusers.vdf`).
fn remembered_account(steam_id: u64) -> Option<String> {
    let text = std::fs::read_to_string(steam_config("loginusers.vdf")?).ok()?;
    let kv = vdf::parse(&text).ok()?;
    let user = kv.obj("users")?.obj(&steam_id.to_string())?;
    (user.str("RememberPassword") == Some("1")).then(|| user.str("AccountName").map(String::from)).flatten()
}

/// Make the official client sign in to this account when it starts, the
/// way it does after you pick it in its account list. Steam must not be
/// running. Only accounts Steam already remembers; no password or token is
/// ever handed over. Returns the account name when set up.
///
/// The changes are temporary: what they replaced is saved to disk first,
/// `AutoLoginUser` goes back once Steam has signed in
/// ([`restore_login_user`]), the two files once Steam has closed
/// ([`restore_login_later`]); Steam rewrites them on exit, so earlier would
/// be undone.
fn auto_login(steam_id: u64, offline: bool) -> Option<String> {
    // Linux keeps AutoLoginUser in ~/.steam/registry.vdf, which we only
    // read: Steam signs in to whichever account it remembers.
    if !cfg!(windows) {
        return None;
    }
    let account = remembered_account(steam_id)?;
    let users_path = steam_config("loginusers.vdf")?;
    let config_path = steam_config("config.vdf");
    let users = std::fs::read_to_string(&users_path).ok()?;
    let config = config_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let (new_users, new_config, mut changes) = plan_auto_login(&users, &config, steam_id, offline);
    if offline {
        tracing::info!("asking Steam to start in its offline mode");
    }
    let before_user = win::reg_string("Software\\Valve\\Steam", "AutoLoginUser");
    if before_user.as_deref() != Some(account.as_str()) {
        changes.auto_login_user = Some((before_user, account.clone()));
    }
    // Remember first, change second: a crash in between can only leave
    // nothing to undo. Refuse to mutate Steam's files if journaling fails.
    if let Err(e) = save_pending(&merge_changes(load_pending(), changes)) {
        tracing::warn!("could not save Steam sign-in restore journal; skipping auto-login override: {e}");
        return None;
    }
    win::set_string("Software\\Valve\\Steam", "AutoLoginUser", &account).ok()?;
    if new_users != users {
        let _ = std::fs::write(&users_path, new_users);
    }
    if let (Some(p), true) = (config_path, new_config != config) {
        tracing::info!("turning off Steam's account picker until Steam closes");
        let _ = std::fs::write(p, new_config);
    }
    Some(account)
}

/// What auto sign-in changed, to put back later. Each entry is
/// (value before, value we wrote); a value is only put back while it still
/// is ours, so anything the player changes in between stays.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct LoginChanges {
    /// Registry `AutoLoginUser` (`None` before = it didn't exist).
    auto_login_user: Option<(Option<String>, String)>,
    /// `loginusers.vdf` `AutoLogin` per account: (steam id, before, ours).
    auto_login: Vec<(String, String, String)>,
    /// `config.vdf` `AlwaysShowUserChooser`.
    chooser: Option<(String, String)>,
    /// `loginusers.vdf` `WantsOfflineMode` / `SkipOfflineModeWarning` for a
    /// game started while Aurora is offline: (key, steam id, before, ours).
    #[serde(default)]
    offline: Vec<(String, String, String, String)>,
}

impl LoginChanges {
    fn is_empty(&self) -> bool {
        self.auto_login_user.is_none() && self.auto_login.is_empty() && self.chooser.is_none() && self.offline.is_empty()
    }
}

/// Steam's own switches for starting in offline mode without asking.
const OFFLINE_KEYS: [&str; 2] = ["WantsOfflineMode", "SkipOfflineModeWarning"];

/// The two files with auto sign-in for `steam_id`, and what changed.
fn plan_auto_login(users: &str, config: &str, steam_id: u64, offline: bool) -> (String, String, LoginChanges) {
    let id = steam_id.to_string();
    let mut new_users = edit_key(users, "AutoLogin", |block, _| Some(if block == id { "1" } else { "0" }.to_string()));
    let mut offline_changes = Vec::new();
    if offline {
        for key in OFFLINE_KEYS {
            let before = key_values(&new_users, key).into_iter().find(|(b, _)| *b == id).map(|(_, v)| v);
            if let Some(before) = before.filter(|v| v != "1") {
                new_users = edit_key(&new_users, key, |block, _| (block == id).then(|| "1".to_string()));
                offline_changes.push((key.to_string(), id.clone(), before, "1".to_string()));
            }
        }
    }
    let new_config = edit_key(config, "AlwaysShowUserChooser", |_, _| Some("0".to_string()));
    let auto_login = key_values(users, "AutoLogin")
        .into_iter()
        .zip(key_values(&new_users, "AutoLogin"))
        .filter(|((_, before), (_, ours))| before != ours)
        .map(|((block, before), (_, ours))| (block, before, ours))
        .collect();
    let chooser = key_values(config, "AlwaysShowUserChooser").into_iter().next().map(|(_, v)| v).filter(|v| v != "0").map(|before| (before, "0".to_string()));
    (new_users, new_config, LoginChanges { auto_login_user: None, auto_login, chooser, offline: offline_changes })
}

/// Put back what [`plan_auto_login`] changed, where it's still ours.
fn undo_auto_login(users: &str, config: &str, c: &LoginChanges) -> (String, String) {
    let mut users = edit_key(users, "AutoLogin", |block, cur| {
        c.auto_login.iter().find(|(b, _, ours)| b == block && ours == cur).map(|(_, before, _)| before.clone())
    });
    for key in OFFLINE_KEYS {
        users = edit_key(&users, key, |block, cur| {
            c.offline.iter().find(|(k, b, _, ours)| k == key && b == block && ours == cur).map(|(_, _, before, _)| before.clone())
        });
    }
    let config = edit_key(config, "AlwaysShowUserChooser", |_, cur| c.chooser.as_ref().filter(|(_, ours)| ours == cur).map(|(before, _)| before.clone()));
    (users, config)
}

/// A second sign-in before the first was undone keeps the first "before".
fn merge_changes(old: Option<LoginChanges>, new: LoginChanges) -> LoginChanges {
    let Some(mut m) = old else { return new };
    match (&mut m.auto_login_user, new.auto_login_user) {
        (Some(o), Some(n)) => o.1 = n.1,
        (o @ None, n) => *o = n,
        _ => {}
    }
    for (block, before, ours) in new.auto_login {
        match m.auto_login.iter_mut().find(|(b, _, _)| *b == block) {
            Some(e) => e.2 = ours,
            None => m.auto_login.push((block, before, ours)),
        }
    }
    match (&mut m.chooser, new.chooser) {
        (Some(o), Some(n)) => o.1 = n.1,
        (o @ None, n) => *o = n,
        _ => {}
    }
    for (key, block, before, ours) in new.offline {
        match m.offline.iter_mut().find(|(k, b, _, _)| *k == key && *b == block) {
            Some(e) => e.3 = ours,
            None => m.offline.push((key, block, before, ours)),
        }
    }
    m
}

/// `"key" "value"` pairs in a KeyValues file, with the Steam id of the
/// account block they're in ("" outside one).
fn key_values(text: &str, key: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut block = String::new();
    for line in text.lines() {
        if let Some(id) = account_block(line) {
            block = id;
        }
        if let Some(v) = value_of(line, key) {
            out.push((block.clone(), v));
        }
    }
    out
}

/// Rewrite the value of `key` on every line where `f(block, current)` says
/// so; everything else stays byte for byte.
fn edit_key(text: &str, key: &str, mut f: impl FnMut(&str, &str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut block = String::new();
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        if let Some(id) = account_block(body) {
            block = id;
        }
        match value_of(body, key).and_then(|cur| f(&block, &cur).filter(|new| *new != cur).map(|new| (cur, new))) {
            Some((cur, new)) => {
                let at = body.rfind(&format!("\"{cur}\"")).unwrap_or(body.len());
                out.push_str(&body[..at]);
                out.push_str(&format!("\"{new}\""));
                out.push_str(&line[at + cur.len() + 2..]);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// `"7656…"` on its own line opens an account block in loginusers.vdf.
fn account_block(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix('"')?.strip_suffix('"')?;
    (inner.len() == 17 && inner.starts_with("7656") && inner.bytes().all(|b| b.is_ascii_digit())).then(|| inner.to_string())
}

fn value_of(line: &str, key: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(&format!("\"{key}\""))?;
    let v = rest.trim().strip_prefix('"')?.strip_suffix('"')?;
    Some(v.to_string())
}

fn pending_path() -> PathBuf {
    crate::store::config_dir().join("steam-sign-in-restore.json")
}

fn load_pending() -> Option<LoginChanges> {
    serde_json::from_str(&std::fs::read_to_string(pending_path()).ok()?).ok()
}

fn save_pending(c: &LoginChanges) -> std::io::Result<()> {
    let path = pending_path();
    if c.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
    }
    let json = serde_json::to_string_pretty(c)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::store::write_private(&path, json.as_bytes())
}

static PENDING_LOCK: Mutex<()> = Mutex::new(());

/// Steam has signed in: `AutoLoginUser` goes back (Steam only reads it at start).
fn restore_login_user() {
    let _l = PENDING_LOCK.lock().unwrap();
    let Some(mut c) = load_pending() else { return };
    if let Some((before, ours)) = c.auto_login_user.clone() {
        let ok = if win::reg_string("Software\\Valve\\Steam", "AutoLoginUser").as_deref() == Some(ours.as_str()) {
            match before {
                Some(v) => win::set_string("Software\\Valve\\Steam", "AutoLoginUser", &v).is_ok(),
                None => win::delete_value("Software\\Valve\\Steam", "AutoLoginUser").is_ok(),
            }
        } else {
            true
        };
        if ok {
            c.auto_login_user = None;
            let _ = save_pending(&c);
        }
    }
}

/// Put everything back if Steam is closed. True when nothing is left to do.
fn restore_login_files() -> bool {
    let _l = PENDING_LOCK.lock().unwrap();
    let Some(mut c) = load_pending() else { return true };
    if steam_running() {
        return false;
    }
    if let Some((before, ours)) = c.auto_login_user.clone() {
        let ok = if win::reg_string("Software\\Valve\\Steam", "AutoLoginUser").as_deref() == Some(ours.as_str()) {
            match before {
                Some(v) => win::set_string("Software\\Valve\\Steam", "AutoLoginUser", &v).is_ok(),
                None => win::delete_value("Software\\Valve\\Steam", "AutoLoginUser").is_ok(),
            }
        } else {
            true
        };
        if ok {
            c.auto_login_user = None;
        }
    }
    let read = |f: &str| steam_config(f).and_then(|p| Some((std::fs::read_to_string(&p).ok()?, p)));
    let (users, config) = (read("loginusers.vdf"), read("config.vdf"));
    let (u_text, c_text) = (users.as_ref().map(|u| u.0.as_str()).unwrap_or(""), config.as_ref().map(|c| c.0.as_str()).unwrap_or(""));
    let (new_users, new_config) = undo_auto_login(u_text, c_text, &c);
    if let Some((old, p)) = &users {
        if *old == new_users || std::fs::write(p, &new_users).is_ok() {
            c.auto_login.clear();
            c.offline.clear();
        }
    }
    if let Some((old, p)) = &config {
        if *old == new_config || std::fs::write(p, &new_config).is_ok() {
            c.chooser = None;
        }
    }
    if save_pending(&c).is_err() {
        return false;
    }
    if c.is_empty() {
        tracing::info!("put Steam's sign-in settings back as they were");
        true
    } else {
        tracing::warn!("some Steam sign-in settings could not be restored yet; keeping restore journal");
        false
    }
}

/// Restore the sign-in settings as soon as Steam is closed: now, or from a
/// background thread that waits for Steam to exit (one at a time).
pub fn restore_login_later() {
    static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if restore_login_files() || WATCHING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        while !restore_login_files() {
            std::thread::sleep(Duration::from_secs(5));
        }
        WATCHING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

// ------------------------------------------------- Linux: native or Proton

static PREFER_OFFICIAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Linux: start every game through the official Steam client (for games
/// that refuse to run without it). Off by default.
pub fn set_prefer_official(on: bool) {
    if PREFER_OFFICIAL.swap(on, std::sync::atomic::Ordering::Relaxed) != on {
        *MODE_CACHE.lock().unwrap() = None;
    }
}

fn prefer_official() -> bool {
    PREFER_OFFICIAL.load(std::sync::atomic::Ordering::Relaxed)
}

static ANTI_CHEAT_ACKNOWLEDGED: Mutex<std::collections::BTreeSet<AppId>> = Mutex::new(std::collections::BTreeSet::new());

pub fn set_anti_cheat_acknowledged(apps: std::collections::BTreeSet<AppId>) {
    *ANTI_CHEAT_ACKNOWLEDGED.lock().unwrap() = apps;
}

pub fn acknowledge_anti_cheat(app_id: AppId) {
    ANTI_CHEAT_ACKNOWLEDGED.lock().unwrap().insert(app_id);
}

pub(crate) fn needs_anti_cheat_warning(app: &OwnedApp, install_dir: &Path, mode: &LaunchMode) -> bool {
    if !matches!(mode, LaunchMode::HeadlessHost { .. }) {
        return false;
    }
    let acknowledged = ANTI_CHEAT_ACKNOWLEDGED.lock().unwrap().contains(&app.app_id);
    !acknowledged && anti_cheat(app, install_dir).is_some()
}

/// Linux: a Proton/compat tool (its internal name) the player chose in
/// Aurora to run Windows games with, instead of Steam's pick.
static PROTON_OVERRIDE: Mutex<Option<String>> = Mutex::new(None);

/// Force a specific Proton for every Proton launch; `None`/empty = automatic.
pub fn set_proton_override(name: Option<String>) {
    *PROTON_OVERRIDE.lock().unwrap() = name.filter(|s| !s.is_empty());
    crate::preferences_changed();
}

/// The forced Proton's internal name, if the player chose one.
pub fn proton_override() -> Option<String> {
    PROTON_OVERRIDE.lock().unwrap().clone()
}

/// Linux: a Proton/compat tool chosen for one game (its Manage menu), like
/// Steam's "Force the use of a specific Steam Play compatibility tool". It
/// beats the global choice and Steam's mapping, and runs the Windows build
/// even when the game has a Linux one.
static PROTON_PER_APP: Mutex<std::collections::BTreeMap<AppId, String>> = Mutex::new(std::collections::BTreeMap::new());

/// Per-game choice: run the Linux build, even if Steam maps a tool to it.
pub const NATIVE: &str = "@native";
/// Per-game choice: run the Windows build with the default Proton, for a
/// game that has a Linux build too.
pub const DEFAULT_PROTON: &str = "@proton";

/// Linux: see [`linux::native_build`]; `None` elsewhere.
pub fn native_build(app: &OwnedApp, install_dir: &Path) -> Option<(bool, bool)> {
    #[cfg(not(windows))]
    return linux::native_build(app, install_dir);
    #[allow(unreachable_code)]
    None
}

/// Force a Proton for one game ([`NATIVE`], [`DEFAULT_PROTON`] or a tool's
/// internal name); `None`/empty = the default.
pub fn set_app_proton(app: AppId, name: Option<String>) {
    let mut map = PROTON_PER_APP.lock().unwrap();
    let changed = match name.filter(|s| !s.is_empty()) {
        Some(n) => map.insert(app, n.clone()).as_ref() != Some(&n),
        None => map.remove(&app).is_some(),
    };
    drop(map);
    if changed {
        // A forced Proton can turn a native game into a Proton one.
        *MODE_CACHE.lock().unwrap() = None;
    }
    crate::preferences_changed();
}

/// The Proton forced for this game, if any.
pub fn app_proton(app: AppId) -> Option<String> {
    PROTON_PER_APP.lock().unwrap().get(&app).cloned()
}

/// Every per-game choice (saved with the preferences).
pub fn app_protons() -> std::collections::BTreeMap<AppId, String> {
    PROTON_PER_APP.lock().unwrap().clone()
}

pub fn set_app_protons(map: std::collections::BTreeMap<AppId, String>) {
    *PROTON_PER_APP.lock().unwrap() = map;
    *MODE_CACHE.lock().unwrap() = None;
    crate::preferences_changed();
}

/// What a game runs with when nothing is forced for it here: the global
/// choice, else Steam's (its mapping or default), else the best installed
/// Proton. Display name, for "Default (…)".
pub fn default_proton(app: AppId) -> Option<String> {
    static CACHE: Mutex<Option<(Instant, AppId, Option<String>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, a, d)) = cache.as_ref() {
        if *a == app && at.elapsed() < Duration::from_secs(5) {
            return d.clone();
        }
    }
    let d = default_proton_uncached(app);
    *cache = Some((Instant::now(), app, d.clone()));
    d
}

fn default_proton_uncached(app: AppId) -> Option<String> {
    let root = local::find_steam_root()?;
    let tools = local::compat::compat_tools(&root);
    proton_override()
        .and_then(|n| tools.iter().find(|t| t.name.eq_ignore_ascii_case(&n)).cloned())
        .or_else(|| local::compat::tool_for(&root, app))
        .map(|t| t.display)
}

/// The installed Proton/compat tools as (internal name, display name), for a
/// chooser. Empty off Linux or without a Steam install.
pub fn proton_builds() -> Vec<(String, String)> {
    // Menus and Settings ask every frame; this reads every app manifest.
    static CACHE: Mutex<Option<(Instant, Vec<(String, String)>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if cache.as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(5)) {
        let builds = match local::find_steam_root() {
            Some(root) => local::compat::compat_tools(&root).into_iter().map(|t| (t.name, t.display)).collect(),
            None => Vec::new(),
        };
        *cache = Some((Instant::now(), builds));
    }
    cache.as_ref().map(|(_, b)| b.clone()).unwrap_or_default()
}

/// Linux: the Windows user folder in the Proton prefix of a game that runs
/// with Proton (for Steam Cloud), once the prefix exists.
pub fn proton_user_dir(app: &OwnedApp, install_dir: &Path) -> Option<PathBuf> {
    #[cfg(not(windows))]
    {
        let p = linux::plan(app, install_dir).ok()?;
        return p.uses_proton().then(|| local::compat::prefix_user_dir(install_dir, app.app_id)).flatten();
    }
    #[cfg(windows)]
    {
        let _ = (app, install_dir);
        None
    }
}

/// Has Proton's Steam bridge (lsteamclient) run in the game's prefix yet?
/// It writes `ActiveProcess\SteamClientDll64` into the prefix's registry,
/// which the game's steam_api reads to find our steamclient once the bridge
/// is turned off.
#[cfg(not(windows))]
fn prefix_knows_client(app: &OwnedApp, install_dir: &Path) -> bool {
    let user_reg = local::compat::compat_data(install_dir, app.app_id).map(|d| d.join("pfx").join("user.reg"));
    user_reg.and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|r| r.contains("\"SteamClientDll64\""))
}

/// Before the first launch of an Aurora Proton game in a new prefix: run
/// `cmd /c exit` there with the bridge on (about 15 s, Steam not needed),
/// which creates the prefix and writes the registry entry. Without it that
/// first launch would keep the bridge, which finds no Steam: no online, and
/// games that need Steam quit (Crab Game, Bloons TD 6). `None` when there's
/// nothing to do.
#[cfg(not(windows))]
pub(crate) fn prefix_setup(app: &OwnedApp, install_dir: &Path) -> Option<Command> {
    if prefix_knows_client(app, install_dir) {
        return None;
    }
    let mut cmd = in_prefix(app, install_dir, "cmd", &["/c".into(), "exit".into()])?;
    // Our DLLs, copied into the prefix as on a real launch.
    let root = local::find_steam_root()?;
    cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", linux::client_shim(&root).ok()?);
    Some(cmd)
}

/// Run a helper to the end with its output dropped; killed after `limit`.
pub(crate) fn run_quietly(cmd: &mut Command, limit: Duration) -> Result<(), String> {
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let end = Instant::now() + limit;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) if s.success() => return Ok(()),
            Some(s) => return Err(format!("exited with {s}")),
            None if Instant::now() > end => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("still running after {}s, stopped", limit.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// Linux: a command running `program` in the game's Proton prefix, or
/// `None` when the game runs natively (or can't start at all).
#[cfg(not(windows))]
pub(crate) fn in_prefix(app: &OwnedApp, install_dir: &Path, program: &str, args: &[String]) -> Option<Command> {
    linux::plan(app, install_dir).ok()?.in_prefix(app, install_dir, program, args)
}

#[cfg(target_os = "linux")]
#[path = "launch_colon.rs"]
mod colon;

#[cfg(not(windows))]
mod linux {
    use super::{normalize, split_args};
    use local::compat::{self, CompatTool};
    use model::{LaunchOption, OwnedApp};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// How a game starts here: its executable, and the Proton to run it
    /// with when it's a Windows program.
    pub struct Plan {
        option: LaunchOption,
        tool: Option<CompatTool>,
        steam_root: Option<PathBuf>,
    }

    /// Run a game's Linux build? Yes, unless the player forced a compatibility
    /// tool in Steam and the Windows build is what's installed. A forced
    /// Proton with only the Linux files on disk (downloaded as a native game,
    /// or before the choice) runs the Linux build: the Windows one isn't there.
    /// Likewise, nothing forced but only the Windows files on disk (Crab
    /// Game, downloaded for Windows) runs those with Proton.
    pub(super) fn use_native(forced: bool, native_installed: bool, windows_installed: bool) -> bool {
        if !native_installed && windows_installed {
            return false;
        }
        !forced || (native_installed && !windows_installed)
    }

    fn is_windows_exe(o: &LaunchOption) -> bool {
        o.executable.to_lowercase().ends_with(".exe") || o.executable.to_lowercase().ends_with(".bat")
    }

    /// The Linux and the Windows launch options, if the game has them.
    fn builds(app: &OwnedApp) -> (Option<&LaunchOption>, Option<&LaunchOption>) {
        let usable = |o: &&LaunchOption| !o.executable.is_empty();
        let native = app.launch.iter().filter(usable).find(|o| o.os_list.split(',').any(|x| x.trim() == "linux") || (o.os_list.is_empty() && !is_windows_exe(o)));
        let windows = app
            .launch
            .iter()
            .filter(usable)
            .find(|o| o.os_list.split(',').any(|x| x.trim() == "windows") || (o.os_list.is_empty() && is_windows_exe(o)))
            .or_else(|| app.launch.iter().filter(usable).find(|o| o.os_list.is_empty()));
        (native, windows)
    }

    /// For the Compatibility menu: `None` if no Linux build is installed,
    /// else (the Linux build is what runs without a choice here, the Windows
    /// build is installed too).
    pub fn native_build(app: &OwnedApp, install_dir: &Path) -> Option<(bool, bool)> {
        let installed = |o: &LaunchOption| install_dir.join(normalize(&o.executable)).is_file();
        let (native, windows) = builds(app);
        let native = native.is_some_and(installed);
        let windows = windows.is_some_and(installed);
        if !native {
            return None;
        }
        let steam_forced = local::find_steam_root().is_some_and(|r| compat::forced_tool(&r, app.app_id));
        Some((use_native(steam_forced, native, windows), windows))
    }

    pub fn plan(app: &OwnedApp, install_dir: &Path) -> Result<Plan, String> {
        let root = local::find_steam_root();
        // The choice for this game here: the Linux build, Proton, or one
        // Proton build (if that tool is still installed).
        let choice = super::app_proton(app.app_id);
        let want_native = choice.as_deref() == Some(super::NATIVE);
        let own = choice
            .clone()
            .filter(|c| c != super::NATIVE && c != super::DEFAULT_PROTON)
            .and_then(|name| root.as_deref().and_then(|r| compat::compat_tools(r).into_iter().find(|t| t.name.eq_ignore_ascii_case(&name))));
        let forced = !want_native
            && (choice.as_deref() == Some(super::DEFAULT_PROTON) || own.is_some() || root.as_deref().is_some_and(|r| compat::forced_tool(r, app.app_id)));
        // A Linux build, unless the player forced a compatibility tool.
        let (native, windows) = builds(app);
        let installed = |o: &LaunchOption| install_dir.join(normalize(&o.executable)).is_file();
        if let Some(o) = native {
            if use_native(forced, installed(o), windows.is_some_and(installed)) {
                return Ok(Plan { option: o.clone(), tool: None, steam_root: root });
            }
        }
        let windows = windows.ok_or_else(|| format!("{} has no launch option for this system", app.name))?;
        let root = root.ok_or("Steam Play needs a Steam install for Proton")?;
        // The player's choice for this game wins, then their global choice in
        // Aurora, then Steam's mapping, then the best installed Proton.
        let tool = own
            .or_else(|| super::proton_override().and_then(|name| compat::compat_tools(&root).into_iter().find(|t| t.name.eq_ignore_ascii_case(&name))))
            .or_else(|| compat::tool_for(&root, app.app_id))
            .ok_or("No Proton is installed. Install one in Steam (Library → Tools → Proton).")?;
        if !install_dir.join(normalize(&windows.executable)).exists() {
            return Err(format!("{} isn't installed completely", app.name));
        }
        Ok(Plan { option: windows.clone(), tool: Some(tool), steam_root: Some(root) })
    }

    /// A Windows program (by extension, or a PE file's "MZ")?
    fn is_windows_program(exe: &Path) -> bool {
        let ext = exe.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if matches!(ext.as_str(), "exe" | "bat" | "cmd" | "msi" | "com") {
            return true;
        }
        let mut head = [0u8; 2];
        std::fs::File::open(exe).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok() && &head == b"MZ"
    }

    /// The command for a non-Steam game (see `super::spawn_non_steam`).
    pub fn non_steam(exe: &Path, args: &[String]) -> Result<Command, String> {
        use std::os::unix::process::CommandExt;
        let mut cmd = if is_windows_program(exe) {
            let root = local::find_steam_root().ok_or("Windows games need a Steam install for Proton")?;
            // Steam's way of numbering shortcuts: a stable id from the path, top bit set.
            let id = exe.to_string_lossy().bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193)) | 0x8000_0000;
            let tool = super::proton_override()
                .and_then(|name| compat::compat_tools(&root).into_iter().find(|t| t.name.eq_ignore_ascii_case(&name)))
                .or_else(|| compat::tool_for(&root, id))
                .ok_or("No Proton is installed. Install one in Steam (Library → Tools → Proton).")?;
            let data = directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust").ok_or("no data folder")?.data_dir().join("compatdata").join(id.to_string());
            std::fs::create_dir_all(&data).map_err(|e| format!("couldn't create the Proton prefix: {e}"))?;
            tracing::info!("launching non-Steam game {} with {}", exe.display(), tool.display);
            let mut c = proton_base(&tool, &root);
            c.arg("waitforexitandrun").arg(exe).args(args);
            c.env("STEAM_COMPAT_DATA_PATH", &data)
                .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &root)
                .env("STEAM_COMPAT_INSTALL_PATH", exe.parent().unwrap_or(exe))
                .env("STEAM_COMPAT_APP_ID", id.to_string());
            c
        } else {
            // A game unpacked from an archive or an AppImage often isn't executable yet.
            make_executable(exe);
            tracing::info!("launching non-Steam game {}", exe.display());
            let mut c = Command::new(exe);
            c.args(args);
            c
        };
        cmd.process_group(0);
        Ok(cmd)
    }

    /// Give a program its x bits (where it has r bits) if it has none.
    fn make_executable(exe: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(exe) {
            let mode = meta.permissions().mode();
            if mode & 0o100 == 0 {
                tracing::info!("{} wasn't executable; fixing that", exe.display());
                let _ = std::fs::set_permissions(exe, std::fs::Permissions::from_mode(mode | ((mode & 0o444) >> 2)));
            }
        }
    }

    /// `proton`, inside the Steam Linux Runtime container when the tool
    /// asks for one (how Steam runs it); the verb and program come after.
    fn proton_base(tool: &CompatTool, root: &Path) -> Command {
        let runtime = tool.runtime_app.and_then(|rt| compat::app_dir(root, rt)).map(|d| d.join("_v2-entry-point")).filter(|p| p.is_file());
        match runtime {
            Some(entry) => {
                let mut c = Command::new(entry);
                c.arg("--verb=waitforexitandrun").arg("--").arg(tool.script());
                c
            }
            None => Command::new(tool.script()),
        }
    }

    /// Our steamclient DLL for a Proton game (matching its bitness), unless
    /// it's a native game, the DLL isn't there, or
    /// `STEAMINRUST_PROTON_CLIENT=0` turns this off.
    pub fn client_dll(plan: &Plan, install_dir: &Path) -> Option<PathBuf> {
        if !plan.uses_proton() || std::env::var("STEAMINRUST_PROTON_CLIENT").as_deref() == Ok("0") {
            return None;
        }
        super::steamclient_dll(super::pe_is_64bit(&plan.exe(install_dir))?)
    }

    /// A stand-in for the Steam folder, given to Proton as
    /// `STEAM_COMPAT_CLIENT_INSTALL_PATH`: every entry links to the real
    /// one, except `legacycompat/steamclient{,64}.dll`, which are ours.
    /// Proton copies those two into the prefix's Steam folder on every
    /// launch, and the game's steam_api loads them from there. A later
    /// launch through Steam itself copies Valve's back.
    pub fn client_shim(root: &Path) -> Result<PathBuf, String> {
        let dir = directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust").ok_or("no data folder")?.data_dir().join("proton-steam");
        let compat = dir.join("legacycompat");
        std::fs::create_dir_all(&compat).map_err(|e| format!("{}: {e}", compat.display()))?;
        link_entries(root, &dir, &["legacycompat"])?;
        let _ = link_entries(&root.join("legacycompat"), &compat, &["steamclient.dll", "steamclient64.dll"]);
        for (is64, name) in [(true, "steamclient64.dll"), (false, "steamclient.dll")] {
            let dst = compat.join(name);
            match super::steamclient_dll(is64) {
                Some(src) if !up_to_date(&src, &dst) => {
                    let tmp = compat.join(format!(".{name}.tmp"));
                    std::fs::copy(&src, &tmp).and_then(|_| std::fs::rename(&tmp, &dst)).map_err(|e| format!("{}: {e}", dst.display()))?;
                }
                Some(_) => {}
                // No DLL of that bitness: Proton copies nothing, the prefix keeps what it has.
                None => {
                    let _ = std::fs::remove_file(&dst);
                }
            }
        }
        Ok(dir)
    }

    /// Symlink each entry of `from` into `to` (except `skip`), fixing links
    /// that point elsewhere (Steam moved).
    fn link_entries(from: &Path, to: &Path, skip: &[&str]) -> Result<(), String> {
        let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
        for e in entries.flatten() {
            let name = e.file_name();
            if skip.iter().any(|s| name == *s) {
                continue;
            }
            let link = to.join(&name);
            if std::fs::read_link(&link).is_ok_and(|t| t == e.path()) {
                continue;
            }
            if std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()) {
                let _ = std::fs::remove_file(&link);
            }
            let _ = std::os::unix::fs::symlink(e.path(), &link);
        }
        Ok(())
    }

    /// `dst` is a copy of `src` made after `src` last changed.
    fn up_to_date(src: &Path, dst: &Path) -> bool {
        let (Ok(s), Ok(d)) = (std::fs::metadata(src), std::fs::metadata(dst)) else { return false };
        s.len() == d.len() && matches!((s.modified(), d.modified()), (Ok(sm), Ok(dm)) if dm >= sm)
    }

    impl Plan {
        pub fn uses_proton(&self) -> bool {
            self.tool.is_some()
        }

        /// `program args…` inside the game's Proton prefix (install scripts:
        /// redistributables, anti-cheat setup, `reg import`). `None` for a
        /// native game.
        pub fn in_prefix(&self, app: &OwnedApp, install_dir: &Path, program: &str, args: &[String]) -> Option<Command> {
            let tool = self.tool.as_ref()?;
            let root = self.steam_root.clone().unwrap_or_default();
            let data = compat::compat_data(install_dir, app.app_id).unwrap_or_else(|| install_dir.join("compatdata"));
            let _ = std::fs::create_dir_all(&data);
            let mut cmd = proton_base(tool, &root);
            cmd.arg("waitforexitandrun").arg(program).args(args);
            let library = install_dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
            cmd.env("STEAM_COMPAT_DATA_PATH", &data)
                .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &root)
                .env("STEAM_COMPAT_INSTALL_PATH", install_dir)
                .env("STEAM_COMPAT_LIBRARY_PATHS", &library)
                .env("STEAM_COMPAT_APP_ID", app.app_id.to_string())
                .env("SteamAppId", app.app_id.to_string())
                .current_dir(install_dir);
            Some(cmd)
        }

        pub fn exe(&self, install_dir: &Path) -> PathBuf {
            install_dir.join(normalize(&self.option.executable))
        }

        pub fn describe(&self) -> String {
            match &self.tool {
                Some(t) => format!("Runs with {}", t.display),
                None => "Native Linux version".to_string(),
            }
        }

        pub fn command(&self, app: &OwnedApp, install_dir: &Path) -> Command {
            use std::os::unix::process::CommandExt;
            let o = &self.option;
            let exe = install_dir.join(normalize(&o.executable));
            let cwd = if o.working_dir.is_empty() { exe.parent().unwrap_or(install_dir).to_path_buf() } else { install_dir.join(normalize(&o.working_dir)) };
            let id = app.app_id.to_string();
            let mut cmd = match &self.tool {
                None => {
                    // Installed before Aurora kept the files' executable flag
                    // ("Permission denied"): fix it, like Steam would have set it.
                    make_executable(&exe);
                    let mut c = Command::new(&exe);
                    c.args(split_args(&o.arguments));
                    c
                }
                Some(tool) => self.proton(tool, app, install_dir, &exe),
            };
            tracing::info!("launching {} ({}) {}", app.name, exe.display(), self.describe());
            cmd.current_dir(cwd).env("SteamAppId", &id).env("SteamGameId", &id).env("SteamOverlayGameId", &id);
            // Like Steam: the driver reads and writes the game's shader cache.
            crate::shaders::apply_env(&mut cmd, install_dir, app.app_id);
            // Its own process group, so stopping the game stops everything it started.
            cmd.process_group(0);
            cmd
        }

        /// `proton waitforexitandrun <exe>`, inside the Steam Linux Runtime
        /// container when the tool asks for one (how Steam runs it).
        fn proton(&self, tool: &CompatTool, app: &OwnedApp, install_dir: &Path, exe: &Path) -> Command {
            let root = self.steam_root.clone().unwrap_or_default();
            let data = compat::compat_data(install_dir, app.app_id).unwrap_or_else(|| install_dir.join("compatdata"));
            let _ = std::fs::create_dir_all(&data);
            let mut cmd = proton_base(tool, &root);
            cmd.arg("waitforexitandrun").arg(exe).args(split_args(&self.option.arguments));
            let library = install_dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
            cmd.env("STEAM_COMPAT_DATA_PATH", &data)
                .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &root)
                .env("STEAM_COMPAT_INSTALL_PATH", install_dir)
                .env("STEAM_COMPAT_LIBRARY_PATHS", &library)
                .env("STEAM_COMPAT_APP_ID", app.app_id.to_string());
            cmd
        }
    }
}

// ------------------------------------------------------------ Win32

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};

    const HKCU: isize = 0x8000_0001u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x2;
    const RRF_RT_REG_DWORD: u32 = 0x10;
    const REG_SZ: u32 = 1;
    const REG_DWORD: u32 = 4;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const TH32CS_SNAPPROCESS: u32 = 0x2;

    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        exe: [u16; 260],
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(key: isize, sub: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
        fn RegSetKeyValueW(key: isize, sub: *const u16, value: *const u16, kind: u32, data: *const c_void, len: u32) -> i32;
        fn RegDeleteKeyValueW(key: isize, sub: *const u16, value: *const u16) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn QueryFullProcessImageNameW(process: *mut c_void, flags: u32, name: *mut u16, len: *mut u32) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(snap: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snap: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn CreateJobObjectW(attrs: *const c_void, name: *const u16) -> *mut c_void;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn QueryInformationJobObject(job: *mut c_void, class: u32, info: *mut c_void, len: u32, ret_len: *mut u32) -> i32;
    }

    /// Windows Job Object handle that tracks the spawned game process and all
    /// of its descendants across launcher handoffs.
    pub struct JobGuard(*mut c_void);

    unsafe impl Send for JobGuard {}

    impl JobGuard {
        pub fn attach(child: &std::process::Child) -> Option<Self> {
            use std::os::windows::io::AsRawHandle;
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return None;
            }
            let ok = unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) };
            if ok == 0 {
                unsafe { CloseHandle(job) };
                return None;
            }
            Some(Self(job))
        }

        pub fn has_processes(&self) -> bool {
            // JobObjectBasicAccountingInformation (class 1): ActiveProcesses is a u32
            // at byte offset 36 (after four 8-byte LARGE_INTEGERs and one u32 TotalProcesses).
            const JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION: u32 = 1;
            #[repr(C)]
            struct BasicAccounting {
                total_user_time: i64,
                total_kernel_time: i64,
                this_period_user_time: i64,
                this_period_kernel_time: i64,
                total_page_fault_count: u32,
                total_processes: u32,
                active_processes: u32,
                total_terminated_processes: u32,
            }
            let mut info: BasicAccounting = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION,
                    (&mut info as *mut BasicAccounting).cast(),
                    std::mem::size_of::<BasicAccounting>() as u32,
                    std::ptr::null_mut(),
                )
            };
            ok != 0 && info.active_processes > 0
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(hwnd: isize, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
    }

    /// Open a program or shortcut the way Explorer does (UAC prompt included).
    pub fn shell_open(file: &Path, args: &str, dir: &Path) -> Result<(), String> {
        const SW_SHOWNORMAL: i32 = 1;
        let (op, f, a, d) = (wide("open"), wide(&file.to_string_lossy()), wide(args), wide(&dir.to_string_lossy()));
        // SAFETY: all strings are NUL-terminated UTF-16 that outlive the call.
        let r = unsafe { ShellExecuteW(0, op.as_ptr(), f.as_ptr(), a.as_ptr(), d.as_ptr(), SW_SHOWNORMAL) };
        // Values above 32 mean success; 1223 is the player cancelling the UAC prompt.
        if r > 32 { Ok(()) } else { Err(format!("couldn't start {} (error {r})", file.display())) }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn reg_string(sub: &str, value: &str) -> Option<String> {
        let (sub, value) = (wide(sub), wide(value));
        let mut buf = vec![0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        let r = unsafe { RegGetValueW(HKCU, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
        (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1).min(buf.len())]))
    }

    pub fn reg_dword(sub: &str, value: &str) -> Option<u32> {
        let (sub, value) = (wide(sub), wide(value));
        let mut v = 0u32;
        let mut len = 4u32;
        let r = unsafe { RegGetValueW(HKCU, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut v as *mut u32).cast(), &mut len) };
        (r == 0).then_some(v)
    }

    pub fn set_string(sub: &str, value: &str, data: &str) -> Result<(), String> {
        let (s, v, d) = (wide(sub), wide(value), wide(data));
        let r = unsafe { RegSetKeyValueW(HKCU, s.as_ptr(), v.as_ptr(), REG_SZ, d.as_ptr().cast(), (d.len() * 2) as u32) };
        if r == 0 { Ok(()) } else { Err(format!("registry {sub}\\{value}: error {r}")) }
    }

    pub fn set_dword(sub: &str, value: &str, data: u32) -> Result<(), String> {
        let (s, v) = (wide(sub), wide(value));
        let r = unsafe { RegSetKeyValueW(HKCU, s.as_ptr(), v.as_ptr(), REG_DWORD, (&data as *const u32).cast(), 4) };
        if r == 0 { Ok(()) } else { Err(format!("registry {sub}\\{value}: error {r}")) }
    }

    pub fn delete_value(sub: &str, value: &str) -> Result<(), String> {
        let (s, v) = (wide(sub), wide(value));
        unsafe { RegDeleteKeyValueW(HKCU, s.as_ptr(), v.as_ptr()) };
        Ok(())
    }

    fn image(pid: u32) -> Option<(bool, String)> {
        if pid == 0 {
            return None;
        }
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut code = 0u32;
            let alive = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let name = if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 { String::from_utf16_lossy(&buf[..len as usize]) } else { String::new() };
            CloseHandle(h);
            Some((alive, name))
        }
    }

    pub fn is_alive(pid: u32) -> bool {
        image(pid).is_some_and(|(alive, _)| alive)
    }

    pub fn is_steam_process(pid: u32) -> bool {
        image(pid).is_some_and(|(alive, name)| alive && name.to_lowercase().ends_with("\\steam.exe"))
    }

    pub fn processes_in(dir: &Path) -> Vec<u32> {
        let want_trimmed = dir.to_string_lossy().to_lowercase();
        let want_trimmed = want_trimmed.trim_end_matches(['\\', '/']);
        let want_prefix = format!("{want_trimmed}\\");
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap as isize == -1 {
                return out;
            }
            let mut e: ProcessEntry = std::mem::zeroed();
            e.size = std::mem::size_of::<ProcessEntry>() as u32;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                if let Some((true, name)) = image(e.pid) {
                    let lower = name.to_lowercase();
                    if PathBuf::from(&name).starts_with(dir) || lower.starts_with(&want_prefix) {
                        out.push(e.pid);
                    }
                }
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        out
    }

    /// steam.exe is running, whatever the registry says.
    pub fn steam_exe_running() -> bool {
        let mut found = false;
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap as isize == -1 {
                return false;
            }
            let mut e: ProcessEntry = std::mem::zeroed();
            e.size = std::mem::size_of::<ProcessEntry>() as u32;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok && !found {
                let len = e.exe.iter().position(|&c| c == 0).unwrap_or(e.exe.len());
                found = String::from_utf16_lossy(&e.exe[..len]).eq_ignore_ascii_case("steam.exe");
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        found
    }

    // Linux only (see the other `win`).
    pub fn steam_pid_file() -> Option<u32> {
        None
    }
    pub fn steam_launch_running(_: model::AppId) -> bool {
        false
    }
}

/// Linux: Steam mirrors its registry in `~/.steam/registry.vdf` (read only
/// here) and the running client in `~/.steam/steam.pid`.
#[cfg(not(windows))]
mod win {
    use model::AppId;
    use std::path::{Path, PathBuf};

    pub fn dot_steam() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".steam"))
    }

    fn registry_value(sub: &str, value: &str) -> Option<String> {
        let text = std::fs::read_to_string(dot_steam()?.join("registry.vdf")).ok()?;
        let kv = vdf::parse(&text).ok()?;
        let mut keys = vec!["Registry", "HKCU"];
        keys.extend(sub.split('\\').filter(|k| !k.is_empty()));
        // Key case differs between Steam versions (Apps / apps).
        let mut obj = kv.obj(keys[0])?;
        for k in &keys[1..] {
            obj = obj.iter().find(|(name, _)| name.eq_ignore_ascii_case(k))?.1.as_obj()?;
        }
        obj.iter().find(|(name, _)| name.eq_ignore_ascii_case(value))?.1.as_str().map(String::from)
    }

    pub fn reg_string(sub: &str, value: &str) -> Option<String> {
        registry_value(sub, value)
    }
    pub fn reg_dword(sub: &str, value: &str) -> Option<u32> {
        registry_value(sub, value)?.trim().parse().ok()
    }
    pub fn set_string(_: &str, _: &str, _: &str) -> Result<(), String> {
        Err("no registry".into())
    }
    pub fn set_dword(_: &str, _: &str, _: u32) -> Result<(), String> {
        Err("no registry".into())
    }
    pub fn delete_value(_: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn is_alive(pid: u32) -> bool {
        pid != 0 && Path::new(&format!("/proc/{pid}")).exists()
    }
    pub fn is_steam_process(pid: u32) -> bool {
        is_alive(pid) && std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "steam")
    }
    /// The pid the running client wrote to `~/.steam/steam.pid`.
    pub fn steam_pid_file() -> Option<u32> {
        std::fs::read_to_string(dot_steam()?.join("steam.pid")).ok()?.trim().parse().ok()
    }

    fn pids() -> impl Iterator<Item = u32> {
        std::fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.parse().ok())
    }

    /// Steam starts every game as `reaper SteamLaunch AppId=<id> -- ...`.
    pub fn steam_launch_running(app_id: AppId) -> bool {
        let want = format!("AppId={app_id}");
        pids().any(|pid| {
            let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return false };
            let mut args = raw.split(|b| *b == 0).take_while(|a| *a != b"--");
            let (mut launch, mut id) = (false, false);
            for a in &mut args {
                launch |= a == b"SteamLaunch";
                id |= a == want.as_bytes();
            }
            launch && id
        })
    }

    /// Processes whose program or working folder is inside `dir`.
    pub fn processes_in(dir: &Path) -> Vec<u32> {
        let me = std::process::id();
        pids()
            .filter(|&pid| pid != me)
            .filter(|pid| ["exe", "cwd"].iter().any(|l| std::fs::read_link(format!("/proc/{pid}/{l}")).is_ok_and(|p| p.starts_with(dir))))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_profile_messages_preserve_the_target_after_ready() {
        let ready = std::cell::Cell::new(false);
        let mut profiles = Vec::new();
        let mut messages = b"STEAMHOST-PROFILE 76561197960265769\nstartup info\nSTEAMHOST-READY\r\n".to_vec();
        messages.extend_from_slice(b"STEAMHOST-PROFILE 76561197960265770\n\xff\nSTEAMHOST-PROFILE 42\nSTEAMHOST-PROFILE 76561197960265771\r\n");
        super::read_steamhost_messages(std::io::Cursor::new(messages), || ready.set(true), |target| {
            assert!(ready.get());
            profiles.push(target);
        });
        assert!(ready.get());
        assert_eq!(profiles, [76561197960265770, 76561197960265771]);
    }

    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn forced_proton_with_only_the_linux_build_runs_native() {
        use super::linux::use_native;
        // Nothing forced: the Linux build.
        assert!(use_native(false, true, false));
        assert!(use_native(false, true, true));
        // Proton forced and the Windows build installed: Proton.
        assert!(!use_native(true, false, true));
        assert!(!use_native(true, true, true));
        // Proton forced, only the Linux build on disk (Dead Island Riptide DE).
        assert!(use_native(true, true, false));
        // Proton forced, nothing installed: Proton's error, as before.
        assert!(!use_native(true, false, false));
        // Nothing forced, only the Windows build on disk (Crab Game): Proton.
        assert!(!use_native(false, false, true));
        // Nothing forced, nothing installed: the Linux build's error, as before.
        assert!(use_native(false, false, false));
    }

    /// Crab Game's case end to end: Linux and Windows launch options, only
    /// the Windows files downloaded, no choice made. Needs a Steam install
    /// with a Proton (skipped otherwise).
    #[cfg(not(windows))]
    #[test]
    fn windows_files_only_run_with_proton() {
        if local::find_steam_root().is_none_or(|r| local::compat::tool_for(&r, 0).is_none()) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("sir-crab-{}", std::process::id())).join("steamapps").join("common").join("Crab Game");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Crab Game.exe"), b"MZ").unwrap();
        let opt = |exe: &str, os: &str| model::LaunchOption { executable: exe.into(), arguments: String::new(), working_dir: String::new(), description: String::new(), os_list: os.into() };
        let app = OwnedApp { app_id: 4_000_000_001, name: "Crab".into(), launch: vec![opt("Crab Game.exe", "windows"), opt("Crab Game.x86_64", "linux")], ..Default::default() };
        let plan = super::linux::plan(&app, &dir).expect("a plan");
        assert!(plan.uses_proton(), "{}", plan.describe());
        let _ = std::fs::remove_dir_all(dir.ancestors().nth(3).unwrap());
    }

    /// A game with both builds on disk (like BeamNG.drive): native unless
    /// the Compatibility menu picks Proton.
    #[cfg(not(windows))]
    #[test]
    fn native_is_the_default_choice_for_games_with_a_linux_build() {
        let dir = std::env::temp_dir().join(format!("sir-native-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("BinLinux")).unwrap();
        std::fs::write(dir.join("Game.exe"), b"MZ").unwrap();
        std::fs::write(dir.join("BinLinux/Game.x64"), b"").unwrap();
        let opt = |exe: &str, os: &str| model::LaunchOption { executable: exe.into(), arguments: String::new(), working_dir: String::new(), description: String::new(), os_list: os.into() };
        // An app id no real Steam config maps a tool to.
        let app = OwnedApp { app_id: 4_000_000_123, name: "Both".into(), launch: vec![opt("Game.exe", "windows"), opt("BinLinux/Game.x64", "linux")], ..Default::default() };
        let runs_native = || linux::plan(&app, &dir).is_ok_and(|p| !p.uses_proton());
        assert_eq!(native_build(&app, &dir), Some((true, true)));
        assert!(runs_native());
        set_app_proton(app.app_id, Some(NATIVE.into()));
        assert!(runs_native());
        // Proton (fails without a Proton installed, but never runs native).
        set_app_proton(app.app_id, Some(DEFAULT_PROTON.into()));
        assert!(!runs_native());
        set_app_proton(app.app_id, None);
        // Only the Windows build: no Native choice.
        let win = OwnedApp { launch: vec![opt("Game.exe", "windows")], ..app.clone() };
        assert_eq!(native_build(&win, &dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn defaults_to_headless_for_regular_and_anti_cheat_games() {
        let dir = std::env::temp_dir().join(format!("aurora-headless-default-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (executable, os) = if cfg!(windows) { ("game.exe", "windows") } else { ("game", "linux") };
        std::fs::write(dir.join(executable), b"").unwrap();
        let mut app = OwnedApp {
            app_id: 4_000_000_221,
            launch: vec![model::LaunchOption {
                executable: executable.into(),
                os_list: os.into(),
                arguments: String::new(),
                working_dir: String::new(),
                description: String::new(),
            }],
            ..Default::default()
        };
        let regular = mode_with_host(&app, &dir, 2, true);
        app.categories.push(CATEGORY_VAC);
        let protected = mode_with_host(&app, &dir, 2, true);
        let fallback = mode_with_host(&app, &dir, 2, false);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(regular, LaunchMode::HeadlessHost { steam_drm: false });
        assert_eq!(protected, LaunchMode::HeadlessHost { steam_drm: false });
        assert_eq!(fallback, LaunchMode::OfficialSteam { anti_cheat: "Valve Anti-Cheat".into() });
    }

    #[test]
    fn anti_cheat_acknowledgement_is_per_game() {
        let dir = std::env::temp_dir().join(format!("aurora-anti-cheat-warning-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = OwnedApp { app_id: 4_000_000_222, ..Default::default() };
        let host = LaunchMode::HeadlessHost { steam_drm: false };
        assert!(!needs_anti_cheat_warning(&app, &dir, &host));

        app.categories.push(CATEGORY_VAC);
        let steam = LaunchMode::OfficialSteam { anti_cheat: "Valve Anti-Cheat".into() };
        assert!(!needs_anti_cheat_warning(&app, &dir, &steam));
        assert!(needs_anti_cheat_warning(&app, &dir, &host));
        assert!(needs_anti_cheat_warning(&app, &dir, &host));
        acknowledge_anti_cheat(app.app_id);
        assert!(!needs_anti_cheat_warning(&app, &dir, &host));

        app.app_id += 1;
        assert!(needs_anti_cheat_warning(&app, &dir, &host));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A minimal PE file with the given section names.
    fn pe_with_sections(names: &[&str]) -> Vec<u8> {
        let mut f = vec![0u8; 64];
        f[..2].copy_from_slice(b"MZ");
        f[60..64].copy_from_slice(&64u32.to_le_bytes());
        f.extend_from_slice(b"PE\0\0");
        let mut coff = [0u8; 20];
        coff[..2].copy_from_slice(&0x8664u16.to_le_bytes());
        coff[2..4].copy_from_slice(&(names.len() as u16).to_le_bytes());
        coff[16..18].copy_from_slice(&240u16.to_le_bytes());
        f.extend_from_slice(&coff);
        f.extend(std::iter::repeat_n(0u8, 240));
        for n in names {
            let mut s = [0u8; 40];
            s[..n.len()].copy_from_slice(n.as_bytes());
            f.extend_from_slice(&s);
        }
        f
    }

    #[test]
    fn finds_steam_drm_section() {
        let dir = std::env::temp_dir().join(format!("sir-drm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (wrapped, plain) = (dir.join("wrapped.exe"), dir.join("plain.exe"));
        std::fs::write(&wrapped, pe_with_sections(&[".text", ".rdata", ".bind"])).unwrap();
        std::fs::write(&plain, pe_with_sections(&[".text", ".rdata", ".binder"])).unwrap();
        assert!(has_steam_drm(&wrapped));
        assert!(!has_steam_drm(&plain));
        assert!(!has_steam_drm(&dir.join("missing.exe")));
        assert_eq!(pe_is_64bit(&wrapped), Some(true));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_steam_html_surface_across_chunk_boundary_and_sets_ld_preload() {
        let dir = std::env::temp_dir().join(format!("aurora-html-shim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let needle = b"STEAMHTMLSURFACE_INTERFACE_VERSION_005";

        let boundary = dir.join("boundary.exe");
        let mut data = vec![b'x'; 64 * 1024 - 10];
        data.extend_from_slice(needle);
        data.extend_from_slice(b"\0trailing");
        std::fs::write(&boundary, &data).unwrap();

        let plain = dir.join("plain.exe");
        std::fs::write(&plain, b"ordinary game binary").unwrap();

        assert!(uses_steam_html_surface(&boundary));
        assert!(!uses_steam_html_surface(&plain));
        assert!(!uses_steam_html_surface(&dir.join("missing.exe")));

        let lib = dir.join("libaurora_htmlsurface.so");
        std::fs::write(&lib, b"").unwrap();
        let mut cmd = Command::new("/bin/true");
        cmd.env("LD_PRELOAD", "/existing/lib.so");
        apply_html_shim_env_with(&mut cmd, &lib);
        let envs: std::collections::HashMap<String, String> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
            .collect();
        let canon = std::fs::canonicalize(&lib).unwrap();
        assert_eq!(envs.get("LD_PRELOAD").map(String::as_str), Some(format!("/existing/lib.so:{}", canon.display()).as_str()));
        assert!(envs.get("PRESSURE_VESSEL_FILESYSTEMS_RO").is_some_and(|v| v.contains(&canon.parent().unwrap().to_string_lossy().to_string())));

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Exercise Play's route with a real or virtual pad, Steam closed, and
    /// steam.exe, the engine and steamhost available in the test environment.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires Windows controller and Steam install fixtures"]
    fn windows_drm_controller_launch() {
        let connected: bool = std::env::var("AURORA_TEST_CONNECTED_PAD").unwrap().parse().unwrap();
        let dir = std::env::temp_dir().join(format!("aurora-controller-route-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("game.exe"), pe_with_sections(&[".text", ".bind"])).unwrap();
        let app = OwnedApp {
            app_id: 1234567,
            launch: vec![model::LaunchOption { executable: "game.exe".into(), os_list: "windows".into(), arguments: String::new(), working_dir: String::new(), description: String::new() }],
            ..Default::default()
        };
        assert_eq!(mode(&app, &dir), LaunchMode::HeadlessHost { steam_drm: true });
        assert_eq!(mode_for_launch(&app, &dir, 1), LaunchMode::HeadlessHost { steam_drm: true });
        assert_eq!(mode_for_launch(&app, &dir, 2), LaunchMode::HeadlessHost { steam_drm: true });
        crate::pads::set_app_controller(app.app_id, 1);
        assert_eq!(mode(&app, &dir), LaunchMode::HeadlessHost { steam_drm: true });
        assert_eq!(mode_for_launch(&app, &dir, 2), LaunchMode::HeadlessHost { steam_drm: true });
        crate::pads::set_app_controller(app.app_id, 0);
        let _ = connected;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn waits_for_game_outside_launcher_job() {
        // Simulate a game started before the launcher was assigned to its job.
        let dir = std::env::temp_dir().join(format!("aurora-handoff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let game_exe = dir.join("game.exe");
        let shell = std::env::var_os("ComSpec").unwrap();
        std::fs::copy(&shell, &game_exe).unwrap();
        let mut game = Command::new(game_exe)
            .args(["/d", "/q"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn().unwrap();
        let launcher = Command::new(shell).args(["/d", "/c", "exit"]).spawn().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let install_dir = dir.clone();
        std::thread::spawn(move || tx.send(wait_in_dir(1, &install_dir, launcher)).ok());
        let while_running = rx.recv_timeout(Duration::from_secs(7));
        drop(game.stdin.take());
        game.wait().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(while_running, Err(std::sync::mpsc::RecvTimeoutError::Timeout));
        assert_eq!(rx.recv_timeout(Duration::from_secs(15)).unwrap(), 1);
    }

    /// A launcher that starts the game in the background and exits.
    #[cfg(not(windows))]
    #[test]
    fn waits_for_what_a_launcher_started() {
        use std::os::unix::process::CommandExt;
        let mut c = Command::new("sh");
        c.args(["-c", "sleep 2 & exit 0"]).process_group(0);
        let mut child = c.spawn().unwrap();
        let t = Instant::now();
        super::wait_all(&mut child);
        assert!(t.elapsed() >= Duration::from_millis(1500), "returned after {:?}", t.elapsed());
        assert!(!super::group_alive(child.id()));
    }

    /// The real hand-off: Cities: Skylines' dowser leaves its group empty for
    /// a moment before the Paradox Launcher shows up in it. Needs the game
    /// (`AURORA_TEST_DOWSER=<game folder>`) and a display; kills the launcher.
    #[cfg(not(windows))]
    #[test]
    #[ignore]
    fn waits_through_a_launcher_hand_off() {
        use std::os::unix::process::CommandExt;
        let Ok(game) = std::env::var("AURORA_TEST_DOWSER") else { return };
        let mut c = Command::new(std::path::Path::new(&game).join("dowser"));
        c.current_dir(&game).process_group(0);
        let mut child = c.spawn().unwrap();
        let group = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            super::wait_all(&mut child);
            let _ = tx.send(());
        });
        // Still "running" well after dowser exits: the launcher is up.
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_err(), "treated the game as closed during the hand-off");
        assert!(super::group_alive(group));
        let _ = Command::new("kill").args(["-KILL", &format!("-{group}")]).status();
        assert!(rx.recv_timeout(Duration::from_secs(15)).is_ok(), "didn't notice the launcher closing");
    }

    #[cfg(not(windows))]
    #[test]
    fn host_pid_file_is_put_back() {
        let dir = std::env::temp_dir().join(format!("sir-pid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(".steam").join("steam.pid");
        // No file before: written, then removed.
        let f = super::HostPidFile::write(path.clone(), 4242).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "4242\n");
        drop(f);
        assert!(!path.exists());
        // A stale pid before: it comes back.
        std::fs::write(&path, "629628").unwrap();
        drop(super::HostPidFile::write(path.clone(), 4242).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "629628");
        // Steam wrote its own meanwhile: left alone.
        let f = super::HostPidFile::write(path.clone(), 4242).unwrap();
        std::fs::write(&path, "777\n").unwrap();
        drop(f);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "777\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn splits_quoted_args() {
        assert_eq!(split_args(r#"-a "b c" d"#), vec!["-a", "b c", "d"]);
    }

    fn words(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn launch_options_like_steam() {
        // No %command%: all game arguments, as before.
        assert_eq!(UserOptions::parse("-novid -fullscreen"), UserOptions { args: words(&["-novid", "-fullscreen"]), ..Default::default() });
        assert_eq!(UserOptions::parse(""), UserOptions::default());
        let o = UserOptions::parse(r#"MANGOHUD=1 WINEDLLOVERRIDES="dinput8=n,b" gamemoderun mangohud %command% -dx11 'a b'"#);
        assert_eq!(o.env, vec![("MANGOHUD".into(), "1".into()), ("WINEDLLOVERRIDES".into(), "dinput8=n,b".into())]);
        assert_eq!(o.wrapper, words(&["gamemoderun", "mangohud"]));
        assert_eq!(o.args, words(&["-dx11", "a b"]));
        // Variables only after a wrapper belong to the wrapper (`env A=1`).
        let o = UserOptions::parse("env A=1 %command%");
        assert!(o.env.is_empty());
        assert_eq!(o.wrapper, words(&["env", "A=1"]));
        let o = UserOptions::parse("PROTON_LOG=1 %command%");
        assert_eq!((o.env.len(), o.wrapper.len(), o.args.len()), (1, 0, 0));
        assert_eq!(shell_words(r#"a "" b"#), words(&["a", "", "b"]));
        assert_eq!(UserOptions::parse(r#"%command% "x y" -z"#).args_line(), r#""x y" -z"#);
    }

    #[test]
    fn wrapper_goes_in_front() {
        let mut game = Command::new("/games/proton");
        game.args(["waitforexitandrun", "game.exe"]).current_dir("/games").env("SteamAppId", "1").env("MANGOHUD", "0");
        let w = UserOptions::parse("MANGOHUD=1 mangohud --dlsym %command% -x").wrap({
            let mut c = game;
            c.arg("-x");
            c
        });
        assert_eq!(w.get_program(), "mangohud");
        assert_eq!(w.get_args().collect::<Vec<_>>(), ["--dlsym", "/games/proton", "waitforexitandrun", "game.exe", "-x"]);
        assert_eq!(w.get_current_dir(), Some(Path::new("/games")));
        let envs: Vec<_> = w.get_envs().collect();
        assert!(envs.contains(&("MANGOHUD".as_ref(), Some("1".as_ref()))));
        assert!(envs.contains(&("SteamAppId".as_ref(), Some("1".as_ref()))));
    }

    /// A real launch: variables and wrapper reach the game, arguments follow it.
    #[cfg(unix)]
    #[test]
    fn non_steam_game_runs_through_the_wrapper() {
        let dir = std::env::temp_dir().join(format!("sir-wrap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let game = dir.join("game.sh");
        let out = dir.join("out");
        std::fs::write(&game, format!("#!/bin/sh\necho \"$AURORA_T $WRAPPED $*\" > '{}'\n", out.display())).unwrap();
        let wrapper = dir.join("wrap.sh");
        std::fs::write(&wrapper, "#!/bin/sh\nWRAPPED=yes exec \"$@\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let opts = format!("AURORA_T=on '{}' %command% -a \"b c\"", wrapper.display());
        let mut child = spawn_non_steam(&game.to_string_lossy(), &opts).unwrap().unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "on yes -a b c");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn app(categories: Vec<u32>, exe: &str) -> OwnedApp {
        OwnedApp {
            app_id: 1,
            name: "Test".into(),
            categories,
            launch: vec![model::LaunchOption { executable: exe.into(), arguments: String::new(), working_dir: String::new(), description: String::new(), os_list: "windows".into() }],
            ..Default::default()
        }
    }

    #[test]
    fn detects_anti_cheat() {
        let dir = std::env::temp_dir().join(format!("sir-ac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        // VAC comes from the store category.
        assert_eq!(anti_cheat(&app(vec![8], "game.exe"), &dir).as_deref(), Some("Valve Anti-Cheat"));
        // Clean folder: none.
        assert_eq!(anti_cheat(&app(vec![2, 22], "game.exe"), &dir), None);
        // Launch option through EAC's bootstrapper.
        assert_eq!(anti_cheat(&app(vec![], "start_protected_game.exe"), &dir).as_deref(), Some("Easy Anti-Cheat"));
        // A BattlEye folder a level down.
        std::fs::create_dir_all(dir.join("bin").join("BattlEye")).unwrap();
        assert_eq!(anti_cheat(&app(vec![], "game.exe"), &dir).as_deref(), Some("BattlEye"));
        // Unreadable folder: treated as anti-cheat.
        assert!(anti_cheat(&app(vec![], "game.exe"), &dir.join("missing")).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const USERS: &str = "\"users\"\r\n{\r\n\t\"76561198000000001\"\r\n\t{\r\n\t\t\"AccountName\"\t\t\"a\"\r\n\t\t\"AutoLogin\"\t\t\"1\"\r\n\t}\r\n\t\"76561198000000002\"\r\n\t{\r\n\t\t\"AccountName\"\t\t\"b\"\r\n\t\t\"AutoLogin\"\t\t\"0\"\r\n\t}\r\n}\r\n";
    const CONFIG: &str = "\"InstallConfigStore\"\n{\n\t\"AlwaysShowUserChooser\"\t\t\"1\"\n\t\"Other\"\t\t\"1\"\n}\n";

    #[test]
    fn auto_login_edits() {
        let (users, config, changes) = plan_auto_login(USERS, CONFIG, 76561198000000002, false);
        let kv = vdf::parse(&users).unwrap();
        let u = kv.obj("users").unwrap();
        assert_eq!(u.obj("76561198000000001").unwrap().str("AutoLogin"), Some("0"));
        assert_eq!(u.obj("76561198000000002").unwrap().str("AutoLogin"), Some("1"));
        assert!(users.contains("\r\n"), "line endings kept");
        assert!(config.contains("\"AlwaysShowUserChooser\"\t\t\"0\""));
        assert!(config.contains("\"Other\"\t\t\"1\""));
        assert_eq!(changes.auto_login.len(), 2);
        assert_eq!(changes.chooser, Some(("1".into(), "0".into())));
    }

    #[test]
    fn auto_login_restores_exactly() {
        let (users, config, changes) = plan_auto_login(USERS, CONFIG, 76561198000000002, false);
        // The changes survive a trip through the file on disk.
        let changes: LoginChanges = serde_json::from_str(&serde_json::to_string(&changes).unwrap()).unwrap();
        let (u, c) = undo_auto_login(&users, &config, &changes);
        assert_eq!(u, USERS);
        assert_eq!(c, CONFIG);
        // Values the player changed meanwhile stay as they are.
        let mine = config.replace("\"AlwaysShowUserChooser\"\t\t\"0\"", "\"AlwaysShowUserChooser\"\t\t\"2\"");
        let (_, c) = undo_auto_login(&users, &mine, &changes);
        assert!(c.contains("\"AlwaysShowUserChooser\"\t\t\"2\""));
        // A second sign-in before the undo keeps the very first values.
        let (users2, config2, again) = plan_auto_login(&users, &config, 76561198000000001, false);
        let merged = merge_changes(Some(changes), again);
        let (u, c) = undo_auto_login(&users2, &config2, &merged);
        assert_eq!(u, USERS);
        assert_eq!(c, CONFIG);
        // Nothing to do when everything already matches.
        let (_, _, none) = plan_auto_login(&users, &config, 76561198000000002, false);
        assert!(none.is_empty());
    }

    #[test]
    fn offline_mode_edits_and_restores() {
        let users = USERS.replace("\"AutoLogin\"\t\t\"0\"", "\"AutoLogin\"\t\t\"0\"\r\n\t\t\"WantsOfflineMode\"\t\t\"0\"\r\n\t\t\"SkipOfflineModeWarning\"\t\t\"0\"");
        let (new_users, config, changes) = plan_auto_login(&users, CONFIG, 76561198000000002, true);
        let kv = vdf::parse(&new_users).unwrap();
        let b = kv.obj("users").unwrap().obj("76561198000000002").unwrap();
        assert_eq!(b.str("WantsOfflineMode"), Some("1"));
        assert_eq!(b.str("SkipOfflineModeWarning"), Some("1"));
        assert_eq!(changes.offline.len(), 2);
        let (u, _) = undo_auto_login(&new_users, &config, &changes);
        assert_eq!(u, users);
        // Online launches leave Steam's offline switches alone.
        let (_, _, online) = plan_auto_login(&users, CONFIG, 76561198000000002, false);
        assert!(online.offline.is_empty());
    }

    #[test]
    fn reads_pe_bitness() {
        let dir = std::env::temp_dir().join(format!("sir-pe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let make_pe = |machine: u16, magic: u16| {
            let mut buf = vec![0u8; 0x100];
            buf[0..2].copy_from_slice(b"MZ");
            buf[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
            buf[0x80..0x84].copy_from_slice(b"PE\0\0");
            buf[0x80 + 4..0x80 + 6].copy_from_slice(&machine.to_le_bytes());
            buf[0x80 + 24..0x80 + 26].copy_from_slice(&magic.to_le_bytes());
            buf
        };
        let pe64 = dir.join("game64.exe");
        let pe32 = dir.join("game32.exe");
        std::fs::write(&pe64, make_pe(0x8664, 0x20b)).unwrap();
        std::fs::write(&pe32, make_pe(0x014c, 0x10b)).unwrap();
        assert_eq!(pe_is_64bit(&pe64), Some(true));
        assert_eq!(pe_is_64bit(&pe32), Some(false));
        #[cfg(windows)]
        {
            let exe = std::env::current_exe().unwrap();
            assert_eq!(pe_is_64bit(&exe), Some(cfg!(target_pointer_width = "64")));
        }
        assert_eq!(pe_is_64bit(Path::new("Cargo.toml")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_connect_args_allowlist() {
        assert_eq!(safe_connect_args("").unwrap(), Vec::<String>::new());
        assert_eq!(safe_connect_args("   ").unwrap(), Vec::<String>::new());
        assert_eq!(safe_connect_args("+connect_lobby 109775241000000001").unwrap(), words(&["+connect_lobby", "109775241000000001"]));
        assert_eq!(safe_connect_args("+invite 30095787509687845").unwrap(), words(&["+invite", "30095787509687845"]));
        assert_eq!(safe_connect_args("+connect 192.168.1.10:27015").unwrap(), words(&["+connect", "192.168.1.10:27015"]));
        assert_eq!(safe_connect_args("+connect [2001:db8::1]:27015 +password s3cr3t_pw").unwrap(), words(&["+connect", "[2001:db8::1]:27015", "+password", "s3cr3t_pw"]));
        assert_eq!(safe_connect_args("+connect play.example-srv_1.com:27015").unwrap(), words(&["+connect", "play.example-srv_1.com:27015"]));

        for bad in [
            "LD_PRELOAD=/tmp/evil.so /bin/sh %command%",
            "%command%",
            "MANGOHUD=1",
            "--gpu-launcher=/bin/sh",
            "-mono-assembly-path=/tmp/evil",
            "+exec autoexec.cfg",
            "+connect -evil:27015",
            "+connect +evil:27015",
            "+connect 1.2.3.4:27015 --gpu-launcher=calc",
            "+connect 1.2.3.4:27015 +exec evil",
            "+connect 1.2.3.4:27015 %command%",
            "+connect 1.2.3.4:27015 +password",
            "+connect 1.2.3.4:27015 +password -flag",
            "+connect 1.2.3.4:27015 +password +exec",
            "+connect 1.2.3.4:27015 +password %command%",
            "+connect 1.2.3.4:27015 +password a\nb",
            "+connect 1.2.3.4:27015 +password a\"+exec",
            "+connect 1.2.3.4:27015 +password a;quit",
            "+connect 1.2.3.4:27015 +password pw extra",
            "+password secret",
            "+connect_lobby +123",
            "+connect_lobby -1",
            "+connect_lobby abc",
            "+connect_lobby 99999999999999999999999",
            "+invite +123",
            "+invite 30095787509687845 -launchsource2",
            "+connect\t1.2.3.4:27015",
        ] {
            assert!(safe_connect_args(bad).is_err(), "expected rejection for {bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn invite_connect_args_cannot_inject_wrapper_or_flags() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sir-aur01-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let game = dir.join("game.sh");
        let out = dir.join("out");
        let pwned = dir.join("pwned");
        let evil = dir.join("evil.sh");
        let good_wrap = dir.join("good_wrap.sh");
        std::fs::write(&game, format!("#!/bin/sh\necho \"$SAFE_VAR|$WRAPPED|$*\" > '{}'\n", out.display())).unwrap();
        std::fs::write(&evil, format!("#!/bin/sh\necho pwned > '{}'\n", pwned.display())).unwrap();
        std::fs::write(&good_wrap, "#!/bin/sh\nWRAPPED=1 exec \"$@\"\n").unwrap();
        std::fs::set_permissions(&game, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&evil, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&good_wrap, std::fs::Permissions::from_mode(0o755)).unwrap();

        let test_app = OwnedApp {
            app_id: 480,
            name: "TestNative".into(),
            launch: vec![model::LaunchOption {
                executable: "game.sh".into(),
                arguments: String::new(),
                working_dir: String::new(),
                description: String::new(),
                os_list: "linux".into(),
            }],
            ..Default::default()
        };

        for payload in [
            format!("LD_PRELOAD=/tmp/x.so '{}' %command%", evil.display()),
            "--gpu-launcher=/bin/sh".into(),
            "-mono-assembly-path=/tmp/evil".into(),
            "+exec autoexec.cfg".into(),
        ] {
            let args = LaunchArgs { user: "-novid".into(), connect: payload.clone(), controller: 0 };
            assert!(spawn(&test_app, &dir, &args, None).is_err(), "spawn must fail closed for {payload}");
            assert!(spawn_with_steamclient(&test_app, &dir, &args, "127.0.0.1:1:tok", "acct", None, None, false).is_err());
            assert!(run_through_steam(480, &args, 1, false, || {}).unwrap_err().contains("refusing launch"));
            assert!(run_through_host(&test_app, &dir, &args, "acct", "tok", 1, None, false, || {}).unwrap_err().contains("refusing launch"));
            assert!(!pwned.exists(), "evil wrapper executed!");
            assert!(!out.exists(), "game started despite unsafe connect!");
        }

        // Valid user wrapper + valid remote +connect works and preserves separation.
        let ok_args = LaunchArgs {
            user: format!("SAFE_VAR=ok '{}' %command% -novid", good_wrap.display()),
            connect: "+connect 127.0.0.1:27015 +password pass123".into(),
            controller: 0,
        };
        let mut child = spawn(&test_app, &dir, &ok_args, None).unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "ok|1|-novid +connect 127.0.0.1:27015 +password pass123");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn steam_input_env_wires_per_game_override_and_global_toggles() {
        let mut cfg = crate::pads::ControllerSettings::default();
        // Disabled per game (2): STEAMINRUST_STEAM_INPUT=0 and STEAMINRUST_PADS omitted even if passed.
        let disabled = steam_input_env_vars(2, &cfg, Some("Local\\sir-pads-1"));
        assert!(disabled.contains(&(crate::pads::ENV_STEAM_INPUT, "0".into())));
        assert!(disabled.contains(&(crate::pads::ENV_INPUT_FAMILIES, "0x0".into())));
        assert!(!disabled.iter().any(|(k, _)| *k == crate::pads::ENV_PADS));

        // Default (0) or Enable (1) with non-Xbox enabled passes STEAMINRUST_PADS and STEAMINRUST_STEAM_INPUT=1.
        let enabled = steam_input_env_vars(1, &cfg, Some("Local\\sir-pads-1"));
        assert!(enabled.contains(&(crate::pads::ENV_STEAM_INPUT, "1".into())));
        assert!(enabled.contains(&(crate::pads::ENV_INPUT_FAMILIES, "0xf".into())));
        assert!(enabled.contains(&(crate::pads::ENV_PADS, "Local\\sir-pads-1".into())));

        // Xbox-only globally: Steam Input and SHM bridge remain enabled so
        // Xbox-classified third-party pads that native XInput misses still work;
        // disabling all families omits STEAMINRUST_PADS.
        cfg.families.playstation = false;
        cfg.families.switch_pro = false;
        cfg.families.generic = false;
        let xbox_only = steam_input_env_vars(0, &cfg, Some("Local\\sir-pads-1"));
        assert!(xbox_only.contains(&(crate::pads::ENV_STEAM_INPUT, "1".into())));
        assert!(xbox_only.contains(&(crate::pads::ENV_INPUT_FAMILIES, "0x1".into())));
        assert!(xbox_only.contains(&(crate::pads::ENV_PADS, "Local\\sir-pads-1".into())));

        cfg.families.xbox = false;
        let none_enabled = steam_input_env_vars(0, &cfg, Some("Local\\sir-pads-1"));
        assert!(none_enabled.contains(&(crate::pads::ENV_STEAM_INPUT, "0".into())));
        assert!(!none_enabled.iter().any(|(k, _)| *k == crate::pads::ENV_PADS));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn vk_overlay_manifest_and_pressure_vessel_env_preserve_existing() {
        let root = std::env::temp_dir().join(format!("aurora-vk-env-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let lib_dir = root.join("opt/aurora");
        let manifest_dir = root.join("xdg/vulkan/implicit_layer.d");
        std::fs::create_dir_all(&lib_dir).unwrap();
        let lib = lib_dir.join("libVkLayer_aurora_overlay.so");
        std::fs::write(&lib, b"elf").unwrap();

        let written_dir = write_vk_layer_manifest_in(&manifest_dir, &lib).unwrap();
        let manifest_text = std::fs::read_to_string(written_dir.join("VkLayer_aurora_overlay.json")).unwrap();
        assert!(manifest_text.contains("VK_LAYER_AURORA_overlay"));
        assert!(manifest_text.contains("AURORA_VK_OVERLAY"));
        assert!(manifest_text.contains(&std::fs::canonicalize(&lib).unwrap().to_string_lossy().as_ref()));

        let mut cmd = Command::new("/bin/true");
        cmd.env("VK_ADD_IMPLICIT_LAYER_PATH", "/user/layers:/other/layers");
        cmd.env("PRESSURE_VESSEL_FILESYSTEMS_RO", "/existing/ro");
        apply_vk_overlay_env_with(&mut cmd, 41234, "tok-xyz", &lib, &written_dir);
        // Applying twice must deduplicate rather than duplicate paths.
        apply_vk_overlay_env_with(&mut cmd, 41234, "tok-xyz", &lib, &written_dir);

        let envs: std::collections::HashMap<_, _> = cmd.get_envs().filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned()))).collect();
        assert_eq!(envs.get("AURORA_VK_OVERLAY").map(String::as_str), Some("1"));
        assert_eq!(envs.get("AURORA_OVERLAY_PORT").map(String::as_str), Some("41234"));
        assert_eq!(envs.get("AURORA_OVERLAY_TOKEN").map(String::as_str), Some("tok-xyz"));

        let layers = envs.get("VK_ADD_IMPLICIT_LAYER_PATH").unwrap();
        assert!(layers.starts_with("/user/layers:/other/layers:"));
        let canon_manifest = std::fs::canonicalize(&written_dir).unwrap();
        let canon_lib_dir = std::fs::canonicalize(&lib_dir).unwrap();
        assert_eq!(layers.split(':').filter(|p| *p == canon_manifest.to_string_lossy()).count(), 1);

        let ro = envs.get("PRESSURE_VESSEL_FILESYSTEMS_RO").unwrap();
        assert!(ro.starts_with("/existing/ro:"));
        assert_eq!(ro.split(':').filter(|p| *p == canon_lib_dir.to_string_lossy()).count(), 1);
        assert_eq!(ro.split(':').filter(|p| *p == canon_manifest.to_string_lossy()).count(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn overlay_toggle_respects_global_and_per_app() {
        set_overlay_enabled(true);
        set_overlay_disabled_apps(Default::default());
        assert!(app_overlay_enabled(553850));

        set_app_overlay(553850, false);
        assert!(!app_overlay_enabled(553850));
        assert!(app_overlay_enabled(730));

        set_app_overlay(553850, true);
        assert!(app_overlay_enabled(553850));

        set_overlay_enabled(false);
        assert!(!app_overlay_enabled(553850));
        set_overlay_enabled(true);
    }

    #[test]
    fn post_exit_grace_is_short_for_normal_games_and_stop_requests() {
        // Explicit Stop returns immediately once no processes remain.
        assert_eq!(post_exit_grace(Duration::from_secs(1), None, true), Duration::ZERO);
        // Short-lived launcher/bootstrapper keeps a 2s handoff window.
        assert_eq!(post_exit_grace(Duration::from_secs(2), None, false), Duration::from_secs(2));
        // Long-running initial game process drops to a 250ms confirmation window.
        assert_eq!(post_exit_grace(Duration::from_secs(30), None, false), Duration::from_millis(250));
        // Short launcher followed by a long-running child game also drops to 250ms.
        assert_eq!(post_exit_grace(Duration::from_secs(1), Some(Duration::from_secs(20)), false), Duration::from_millis(250));
    }

    #[test]
    fn host_count_waits_for_stopping_host_and_rejects_running_host() {
        let first = HostCount::acquire().expect("idle host acquires");
        assert!(HostCount::acquire().is_err(), "running host must reject concurrent acquire");
        first.begin_stopping();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            drop(first);
        });
        let second = HostCount::acquire().expect("stopping host should hand off once dropped");
        releaser.join().unwrap();
        drop(second);
    }
}

/// Prints how a game would start on this machine, without starting it:
/// `cargo test -p client --release launch::dry_run -- --ignored --nocapture`
/// (STEAMINRUST_DRY_APP / _EXE / _DIR pick the game; defaults to Microsoft
/// Flight Simulator 2020, a Windows-only game).
#[cfg(all(test, not(windows)))]
mod dry_run {
    #[test]
    #[ignore]
    fn dry_run() {
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        let app_id: model::AppId = env("STEAMINRUST_DRY_APP", "1250410").parse().unwrap();
        let exe = env("STEAMINRUST_DRY_EXE", "FlightSimulator.exe");
        let dir = std::path::PathBuf::from(env("STEAMINRUST_DRY_DIR", &format!("{}/.local/share/Steam/steamapps/common/MicrosoftFlightSimulator", std::env::var("HOME").unwrap())));
        let app = model::OwnedApp {
            app_id,
            name: "dry run".into(),
            launch: vec![model::LaunchOption { executable: exe, arguments: String::new(), working_dir: String::new(), description: String::new(), os_list: "windows".into() }],
            ..Default::default()
        };
        println!("mode: {:?}", super::mode(&app, &dir));
        let cmd = super::command(&app, &dir, &super::UserOptions::default(), &[]).expect("a command");
        println!("program: {}", cmd.get_program().to_string_lossy());
        println!("args: {:?}", cmd.get_args().collect::<Vec<_>>());
        println!("cwd: {:?}", cmd.get_current_dir());
        for (k, v) in cmd.get_envs() {
            println!("env {}={}", k.to_string_lossy(), v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default());
        }
    }
}
