//! The setup's file work: unpack the app, shortcuts, the "Apps & features"
//! entry, updating in place and uninstalling. Per-user only (no admin).

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const APP_EXE: &str = "aurora.exe";
pub const UNINSTALL_EXE: &str = "uninstall.exe";
const INSTALL_MANIFEST: &str = "aurora-files.txt";
/// Known Aurora-owned files in the install directory (used when uninstalling
/// installs created before `aurora-files.txt` existed, or as an explicit
/// allowlist so uninstall never wipes unrelated user files).
const OWNED_FILES: &[&str] = &[
    APP_EXE,
    OLD_EXE,
    "steamclient.dll",
    "steamclient32.dll",
    "steamclient64.dll",
    "steamhost.exe",
    "SDL3.dll",
    "SDL3-LICENSE.txt",
    "aurora.log",
    "steamclient.log",
    INSTALL_MANIFEST,
];
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Aurora";
const APP_PATHS_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths\aurora.exe";
/// Names from before the rename to Aurora (0.7.2 and older): an update
/// moves an install over from them.
const OLD_EXE: &str = "steam-rs.exe";
const OLD_UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\SteamInRust";
const OLD_APP_PATHS_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths\steam-rs.exe";
const OLD_LINK: &str = "SteamInRust.lnk";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const STEAM_LINK_KEY: &str = r"HKCU\Software\Classes\steam\shell\open\command";

/// The app files, zipped (see build.rs).
pub static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));

pub fn has_payload() -> bool {
    PAYLOAD.len() > 22
}

#[derive(Default, Clone)]
pub struct Progress {
    pub fraction: f32,
    pub file: String,
    /// What happened so far, for "Show details" (like Steam's setup).
    pub log: Vec<String>,
    /// Bytes unpacked / total (the updater shows "x of y KB").
    pub bytes: (u64, u64),
    pub done: bool,
    pub error: Option<String>,
}

pub type Shared = Arc<Mutex<Progress>>;

#[derive(Clone)]
pub struct Options {
    pub dir: PathBuf,
    pub desktop: bool,
    pub start_menu: bool,
    /// Updating: keep shortcuts as they are.
    pub update: bool,
}

fn set(p: &Shared, f: impl FnOnce(&mut Progress)) {
    if let Ok(mut g) = p.lock() {
        f(&mut g);
    }
}

fn log(p: &Shared, line: String) {
    set(p, |g| g.log.push(line));
}

pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Programs").join("Aurora")
}

/// Where an earlier install lives, from its "Apps & features" entry.
pub fn installed_dir() -> Option<PathBuf> {
    [UNINSTALL_KEY, OLD_UNINSTALL_KEY].iter().find_map(|key| {
        let out = reg(&["query", key, "/v", "InstallLocation"])?;
        let line = out.lines().find(|l| l.contains("REG_SZ"))?;
        let path = PathBuf::from(line.split("REG_SZ").nth(1)?.trim());
        (path.join(APP_EXE).exists() || path.join(OLD_EXE).exists()).then_some(path)
    })
}

/// Install or update. Waits up to `wait` for a running copy to close.
pub fn install(opts: &Options, p: &Shared, wait: Duration) -> Result<(), String> {
    std::fs::create_dir_all(&opts.dir).map_err(|e| format!("{}: {e}", opts.dir.display()))?;
    log(p, format!("Output folder: {}", opts.dir.display()));
    set(p, |g| g.file = i18n::tr_dyn("Waiting for Aurora to close…"));
    wait_unlocked(&opts.dir.join(APP_EXE), wait)?;
    // An update from before the rename: the running copy is steam-rs.exe.
    wait_unlocked(&opts.dir.join(OLD_EXE), wait)?;

    let mut zip = zip::ZipArchive::new(Cursor::new(PAYLOAD)).map_err(|e| format!("payload: {e}"))?;
    let total: u64 = (0..zip.len()).filter_map(|i| zip.by_index(i).ok().map(|f| f.size())).sum::<u64>().max(1);
    let mut written = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { continue };
        let out = opts.dir.join(&rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let name = rel.display().to_string();
        set(p, |g| g.file = format!("Extract: {name}"));
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        replace_file(&out, &data)?;
        written += data.len() as u64;
        log(p, format!("Extract: {}... 100%", rel.display()));
        set(p, |g| {
            g.fraction = 0.9 * written as f32 / total as f32;
            g.bytes = (written, total);
        });
    }

    // The uninstaller is this setup (it knows `--uninstall`).
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let un = opts.dir.join(UNINSTALL_EXE);
    if me != un {
        let bytes = std::fs::read(&me).map_err(|e| e.to_string())?;
        replace_file(&un, &bytes)?;
        log(p, format!("Created uninstaller: {}", un.display()));
    }
    set(p, |g| {
        g.fraction = 0.95;
        g.file = i18n::tr_dyn("Creating shortcuts…");
    });
    let exe = opts.dir.join(APP_EXE);
    migrate_old_names(&opts.dir, &exe, p);
    if !opts.update {
        let _ = remove_shortcuts();
        if opts.start_menu {
            let link = start_menu_link();
            shortcut(&link, &exe)?;
            log(p, format!("Create shortcut: {}", link.display()));
        }
        if opts.desktop {
            let link = desktop_dir()?.join("Aurora.lnk");
            shortcut(&link, &exe)?;
            log(p, format!("Create shortcut: {}", link.display()));
        }
    } else {
        // Ensure the Start Menu shortcut exists and points to the updated exe
        // (repairs earlier installs where PowerShell shortcut creation silently failed).
        let link = start_menu_link();
        if shortcut(&link, &exe).is_ok() {
            log(p, format!("Create shortcut: {}", link.display()));
        }
        if let Ok(desktop) = desktop_dir() {
            let link = desktop.join("Aurora.lnk");
            if link.exists() && shortcut(&link, &exe).is_ok() {
                log(p, format!("Update shortcut: {}", link.display()));
            }
        }
    }
    register(&opts.dir)?;
    log(p, "Completed".into());
    set(p, |g| {
        g.fraction = 1.0;
        g.file = i18n::tr_dyn("Completed");
        g.done = true;
    });
    Ok(())
}

/// Remove the app. `data`: also delete settings, saved sign-ins and caches.
pub fn uninstall(dir: &Path, data: bool, p: &Shared) -> Result<(), String> {
    set(p, |g| g.file = i18n::tr_dyn("Waiting for Aurora to close…"));
    wait_unlocked(&dir.join(APP_EXE), Duration::from_secs(20))?;
    set(p, |g| {
        g.fraction = 0.2;
        g.file = i18n::tr_dyn("Removing shortcuts…");
    });
    let _ = remove_shortcuts();
    restore_steam_links(dir);
    let _ = reg(&["delete", UNINSTALL_KEY, "/f"]);
    let _ = reg(&["delete", OLD_UNINSTALL_KEY, "/f"]);
    let _ = reg(&["delete", APP_PATHS_KEY, "/f"]);
    let _ = reg(&["delete", OLD_APP_PATHS_KEY, "/f"]);
    // Startup entry, if the app added one (under either name).
    for name in ["Aurora", "SteamInRust"] {
        let _ = reg(&["delete", RUN_KEY, "/v", name, "/f"]);
    }
    set(p, |g| {
        g.fraction = 0.4;
        g.file = i18n::tr_dyn("Removing files…");
    });
    remove_aurora_files(dir, p);
    if data {
        set(p, |g| {
            g.fraction = 0.7;
            g.file = i18n::tr_dyn("Removing your settings…");
        });
        for d in data_dirs() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
    // This program can't delete itself while running: a short-lived helper
    // removes uninstall.exe and deletes `dir` ONLY if it is now empty.
    schedule_delete(dir);
    log(p, "Completed".into());
    set(p, |g| {
        g.fraction = 1.0;
        g.file = i18n::tr_dyn("Completed");
        g.done = true;
    });
    Ok(())
}

fn is_symlink_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return true;
        }
    }
    false
}

/// Validate that a relative file path from the embedded payload has only plain
/// normal components with no traversal (`..`), root, drive prefix (`:`), or
/// embedded separators.
fn safe_manifest_rel(entry: &str) -> Option<PathBuf> {
    use std::path::Component;
    let trimmed = entry.trim();
    if trimmed.is_empty() || trimmed.contains(['\0', '\\']) {
        return None;
    }
    let rel = Path::new(trimmed);
    if rel.is_absolute() {
        return None;
    }
    let mut clean = PathBuf::new();
    for c in rel.components() {
        match c {
            Component::Normal(part) => {
                let s = part.to_str()?;
                if s.is_empty() || s.contains(':') || s.contains('/') || s.contains('\\') {
                    return None;
                }
                clean.push(part);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!clean.as_os_str().is_empty()).then_some(clean)
}

/// Return `true` only if `dir` and all of its existing ancestors are real
/// non-reparse directories (refuses symlinks and Windows NTFS junctions).
fn dir_and_ancestors_are_real(dir: &Path) -> bool {
    use std::path::Component;
    let mut cur = PathBuf::new();
    for comp in dir.components() {
        cur.push(comp);
        if matches!(comp, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&cur) else {
            return false;
        };
        if is_symlink_or_reparse(&meta) || !meta.is_dir() {
            return false;
        }
    }
    std::fs::symlink_metadata(dir)
        .is_ok_and(|m| !is_symlink_or_reparse(&m) && m.is_dir())
}

/// Build the trusted allowlist of Aurora-owned relative paths strictly from
/// `OWNED_FILES` plus entries embedded in this binary's own `PAYLOAD` (never
/// from any writable file inside the install directory).
fn trusted_owned_rels() -> Vec<PathBuf> {
    let mut rels: Vec<PathBuf> = OWNED_FILES.iter().map(PathBuf::from).collect();
    if let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(PAYLOAD)) {
        for i in 0..zip.len() {
            let Ok(f) = zip.by_index(i) else { continue };
            if f.is_dir() {
                continue;
            }
            let Some(enclosed) = f.enclosed_name() else { continue };
            let Some(raw) = enclosed.to_str() else { continue };
            if let Some(rel) = safe_manifest_rel(raw)
                && rel.file_name().is_none_or(|n| !n.eq_ignore_ascii_case(UNINSTALL_EXE))
                && !rels.contains(&rel)
            {
                rels.push(rel);
            }
        }
    }
    rels
}

/// Delete only Aurora-owned files inside `dir`, verifying no ancestor or target
/// is a symlink/reparse point, and remove only now-empty subdirectories.
fn remove_aurora_files(dir: &Path, p: &Shared) {
    if !dir_and_ancestors_are_real(dir) {
        return;
    }
    let rels = trusted_owned_rels();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for rel in rels {
        // Walk ancestors under `dir` and refuse to traverse any symlink/reparse point.
        let mut cur = dir.to_path_buf();
        let mut safe = true;
        let comps: Vec<_> = rel.components().collect();
        for (i, comp) in comps.iter().enumerate() {
            cur.push(comp);
            match std::fs::symlink_metadata(&cur) {
                Ok(m) if is_symlink_or_reparse(&m) => {
                    safe = false;
                    break;
                }
                Ok(m) if i + 1 < comps.len() && !m.is_dir() => {
                    safe = false;
                    break;
                }
                Ok(_) => {
                    if i + 1 < comps.len() {
                        subdirs.push(cur.clone());
                    }
                }
                Err(_) => {
                    safe = false;
                    break;
                }
            }
        }
        if safe && let Ok(m) = std::fs::symlink_metadata(&cur) && !is_symlink_or_reparse(&m) && m.is_file() {
            if std::fs::remove_file(&cur).is_ok() {
                log(p, format!("Delete file: {}", cur.display()));
            }
        }
    }
    // Remove only empty Aurora-owned subdirectories (deepest first).
    subdirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
    subdirs.dedup();
    for sd in subdirs {
        if let Ok(m) = std::fs::symlink_metadata(&sd) && !is_symlink_or_reparse(&m) && m.is_dir() {
            let _ = std::fs::remove_dir(&sd);
        }
    }
}

/// Settings, sign-ins and caches the app keeps outside its folder.
fn data_dirs() -> Vec<PathBuf> {
    // %APPDATA%\SteamInRust and %LOCALAPPDATA%\SteamInRust (the program
    // itself is in %LOCALAPPDATA%\Programs, a different folder).
    ["APPDATA", "LOCALAPPDATA"].iter().filter_map(|v| std::env::var_os(v)).map(|b| PathBuf::from(b).join("SteamInRust")).collect()
}

/// Write `data` to `path` safely without following or truncating pre-existing
/// symlinks/reparse points, also when an old copy is in the way.
fn replace_file(path: &Path, data: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new().prefix(".aurora-update-").tempdir_in(parent).map_err(|e| e.to_string())?;
    let tmp = stage.path().join("new");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    install_staged_file(path, &tmp, stage)
}

fn install_staged_file(path: &Path, tmp: &Path, stage: tempfile::TempDir) -> Result<(), String> {
    let old = stage.path().join("previous.old");
    match std::fs::symlink_metadata(path) {
        Ok(m) if is_symlink_or_reparse(&m) || !m.is_file() => return Err(format!("refusing to overwrite {}", path.display())),
        Ok(_) => std::fs::rename(path, &old).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return std::fs::rename(tmp, path).map_err(|e| e.to_string()),
        Err(e) => return Err(e.to_string()),
    }
    if let Err(e) = std::fs::rename(tmp, path) {
        if let Err(restore) = std::fs::rename(&old, path) {
            let _ = stage.keep();
            return Err(format!("{e}; restore failed: {restore}; backup at {}", old.display()));
        }
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(())
}

/// Wait until `exe` isn't running (it can be renamed again).
fn wait_unlocked(exe: &Path, wait: Duration) -> Result<(), String> {
    wait_unlocked_with(exe, wait, |from, to| std::fs::rename(from, to))
}

fn wait_unlocked_with(exe: &Path, wait: Duration, rename: impl Fn(&Path, &Path) -> std::io::Result<()>) -> Result<(), String> {
    if !exe.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let probe = exe.with_extension("probe");
    let start = Instant::now();
    loop {
        if rename(exe, &probe).is_ok() {
            return rename(&probe, exe).map_err(|e| format!("could not restore {} from {}: {e}", exe.display(), probe.display()));
        }
        if start.elapsed() > wait {
            return Err(i18n::tr_dyn("Aurora is still running. Close it (also from the tray) and try again."));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn register(dir: &Path) -> Result<(), String> {
    let exe = dir.join(APP_EXE).display().to_string();
    let un = dir.join(UNINSTALL_EXE).display().to_string();
    let size_kb: u64 = std::fs::read_dir(dir).map(|r| r.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum::<u64>() / 1024).unwrap_or(0);
    let values: [(&str, &str, String); 10] = [
        ("DisplayName", "REG_SZ", "Aurora".into()),
        ("DisplayVersion", "REG_SZ", env!("CARGO_PKG_VERSION").into()),
        ("Publisher", "REG_SZ", "Aurora".into()),
        ("DisplayIcon", "REG_SZ", format!("{exe},0")),
        ("InstallLocation", "REG_SZ", dir.display().to_string()),
        ("UninstallString", "REG_SZ", format!("\"{un}\" --uninstall")),
        ("QuietUninstallString", "REG_SZ", format!("\"{un}\" --uninstall --quiet")),
        ("EstimatedSize", "REG_DWORD", size_kb.to_string()),
        ("NoModify", "REG_DWORD", "1".into()),
        ("NoRepair", "REG_DWORD", "1".into()),
    ];
    for (name, kind, value) in values {
        reg(&["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", &value, "/f"]).ok_or_else(|| i18n::tr_dyn("Windows refused the change"))?;
    }
    let dir_str = dir.display().to_string();
    let _ = reg(&["add", APP_PATHS_KEY, "/ve", "/t", "REG_SZ", "/d", &exe, "/f"]);
    let _ = reg(&["add", APP_PATHS_KEY, "/v", "Path", "/t", "REG_SZ", "/d", &dir_str, "/f"]);
    Ok(())
}

/// If the app took over `steam://` links, give them back.
fn restore_steam_links(dir: &Path) {
    let Some(out) = reg(&["query", STEAM_LINK_KEY, "/ve"]) else { return };
    let ours = out.to_lowercase().contains(&dir.display().to_string().to_lowercase());
    if !ours {
        return;
    }
    let backup = directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust").and_then(|d| std::fs::read_to_string(d.config_dir().join("steam-protocol-backup.txt")).ok());
    match backup.filter(|b| !b.trim().is_empty()) {
        Some(old) => {
            let _ = reg(&["add", STEAM_LINK_KEY, "/ve", "/d", old.trim(), "/f"]);
        }
        None => {
            let _ = reg(&["delete", r"HKCU\Software\Classes\steam", "/f"]);
        }
    }
}

fn start_menu_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(dir) = win_shell::known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Programs) {
        return dir;
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    base.join(r"Microsoft\Windows\Start Menu\Programs")
}

fn start_menu_link() -> PathBuf {
    start_menu_dir().join("Aurora.lnk")
}

fn desktop_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        if let Some(dir) = win_shell::known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Desktop) {
            return Ok(dir);
        }
        if let Ok(out) = powershell("[Environment]::GetFolderPath('Desktop')") {
            let trimmed = out.trim();
            if !trimmed.is_empty() {
                return Ok(PathBuf::from(trimmed));
            }
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            return Ok(PathBuf::from(profile).join("Desktop"));
        }
        return Err("could not locate Desktop folder".into());
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Desktop")).ok_or_else(|| "could not locate Desktop folder".into())
    }
}

fn remove_shortcuts() -> Result<(), String> {
    let sm = start_menu_dir();
    let sm_link = sm.join("Aurora.lnk");
    let _ = std::fs::remove_file(&sm_link);
    let _ = std::fs::remove_file(sm.join(OLD_LINK));
    let sub = sm.join("Aurora");
    let _ = std::fs::remove_file(sub.join("Aurora.lnk"));
    let _ = std::fs::remove_dir(&sub);
    #[cfg(windows)]
    win_shell::notify_dir(&sm);
    if let Ok(desktop) = desktop_dir() {
        let _ = std::fs::remove_file(desktop.join("Aurora.lnk"));
        let _ = std::fs::remove_file(desktop.join(OLD_LINK));
        #[cfg(windows)]
        win_shell::notify_dir(&desktop);
    }
    Ok(())
}

/// Move an install from before the rename over to Aurora's names: the old
/// program file, its "Apps & features" entry, shortcuts (kept where they
/// were, under the new name) and `steam://` links pointing at steam-rs.exe.
fn migrate_old_names(dir: &Path, exe: &Path, p: &Shared) {
    let old = dir.join(OLD_EXE);
    if old.exists() && std::fs::remove_file(&old).is_ok() {
        log(p, format!("Delete file: {}", old.display()));
    }
    let _ = reg(&["delete", OLD_UNINSTALL_KEY, "/f"]);
    let _ = reg(&["delete", OLD_APP_PATHS_KEY, "/f"]);
    let start_old = start_menu_link().with_file_name(OLD_LINK);
    if start_old.exists() {
        let _ = std::fs::remove_file(&start_old);
        let sm = start_menu_link();
        let _ = shortcut(&sm, exe);
        log(p, format!("Create shortcut: {}", sm.display()));
    }
    if let Ok(desktop) = desktop_dir() {
        let old_link = desktop.join(OLD_LINK);
        if old_link.exists() {
            let _ = std::fs::remove_file(&old_link);
            let _ = shortcut(&desktop.join("Aurora.lnk"), exe);
            log(p, format!("Create shortcut: {}", desktop.join("Aurora.lnk").display()));
        }
    }
    if let Some(out) = reg(&["query", STEAM_LINK_KEY, "/ve"]) {
        if let Some(cmd) = out.lines().find(|l| l.contains("REG_SZ")).and_then(|l| l.split("REG_SZ").nth(1)).map(str::trim) {
            let old_path = old.display().to_string();
            if cmd.to_lowercase().contains(&old_path.to_lowercase()) {
                let new = format!("\"{}\" \"%1\"", exe.display());
                let _ = reg(&["add", STEAM_LINK_KEY, "/ve", "/d", &new, "/f"]);
                log(p, "steam:// links now open Aurora".into());
            }
        }
    }
}

fn shortcut(link: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = link.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
    }
    #[cfg(windows)]
    {
        // Prefer native Win32 COM (IShellLinkW + IPersistFile): no PowerShell
        // subprocess, works under Constrained Language Mode / Defender ASR,
        // and handles Unicode paths natively.
        if let Err(com_err) = win_shell::create_shortcut(link, target) {
            // Fallback to hardened PowerShell if COM somehow failed.
            if let Err(ps_err) = shortcut_powershell(link, target) {
                return Err(format!("could not create shortcut {}: COM: {com_err}; PowerShell: {ps_err}", link.display()));
            }
        }
        if !link.is_file() {
            return Err(format!("shortcut was not created: {}", link.display()));
        }
        win_shell::notify_created(link);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        shortcut_powershell(link, target)
    }
}

fn shortcut_powershell(link: &Path, target: &Path) -> Result<(), String> {
    let dir = target.parent().unwrap_or(target);
    // Pass paths strictly via environment variables (never interpolated into
    // single-quoted PowerShell literals, which are vulnerable to Unicode smart-quote
    // delimiter injection like U+2018/U+2019).
    let script = "$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:AURORA_PS_LINK);$s.TargetPath=$env:AURORA_PS_TARGET;$s.WorkingDirectory=$env:AURORA_PS_DIR;$s.IconLocation=($env:AURORA_PS_TARGET + ',0');$s.Description='Aurora';$s.Save()";
    powershell_with_env(
        script,
        &[
            ("AURORA_PS_LINK", link.as_os_str()),
            ("AURORA_PS_TARGET", target.as_os_str()),
            ("AURORA_PS_DIR", dir.as_os_str()),
        ],
    )
    .map(|_| ())
}

fn schedule_delete(dir: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let uninstaller = dir.join(UNINSTALL_EXE);
        // Waits until this uninstaller exits, verifies neither `dir`, any of its
        // ancestors, nor `uninstall.exe` is a reparse point/junction, deletes
        // ONLY uninstall.exe, and removes `dir` ONLY if it is now empty (never `-Recurse`).
        let script = format!(
            "function Test-NoReparse($p) {{ \
               $cur = $p; \
               while ($cur) {{ \
                 $it = Get-Item -LiteralPath $cur -Force -ErrorAction Stop; \
                 if ($it.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {{ return $false }}; \
                 $parent = Split-Path -Path $cur -Parent; \
                 if (-not $parent -or $parent -eq $cur) {{ break }}; \
                 $cur = $parent \
               }}; \
               return $true \
             }}; \
             Wait-Process -Id {pid} -ErrorAction SilentlyContinue; \
             for ($i = 0; $i -lt 20; $i++) {{ \
               Start-Sleep -Milliseconds 500; \
               try {{ \
                 if (-not (Test-NoReparse $env:AURORA_PS_DIR)) {{ exit 0 }}; \
                 if (Test-Path -LiteralPath $env:AURORA_PS_UNINSTALLER) {{ \
                   $u = Get-Item -LiteralPath $env:AURORA_PS_UNINSTALLER -Force -ErrorAction Stop; \
                   if ($u.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {{ exit 0 }}; \
                   Remove-Item -LiteralPath $env:AURORA_PS_UNINSTALLER -Force -ErrorAction SilentlyContinue \
                 }}; \
                 if (-not (Test-Path -LiteralPath $env:AURORA_PS_UNINSTALLER)) {{ break }} \
               }} catch {{ exit 0 }} \
             }}; \
             try {{ \
               if (Test-NoReparse $env:AURORA_PS_DIR) {{ \
                 [System.IO.Directory]::Delete($env:AURORA_PS_DIR, $false) \
               }} \
             }} catch {{}}",
            pid = std::process::id(),
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .env("AURORA_PS_UNINSTALLER", &uninstaller)
            .env("AURORA_PS_DIR", dir)
            .creation_flags(0x0800_0000 | 0x0000_0008)
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = dir;
}

/// The language picked in setup becomes the app's language on its first
/// start (the app reads this file only while it has no saved choice).
pub fn save_language(code: &str) {
    if let Some(d) = directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust") {
        let _ = std::fs::create_dir_all(d.config_dir());
        let _ = std::fs::write(d.config_dir().join("language"), code);
    }
}

/// Bytes the app needs (for "Space required").
pub fn payload_size() -> u64 {
    zip::ZipArchive::new(Cursor::new(PAYLOAD)).map(|mut z| (0..z.len()).filter_map(|i| z.by_index(i).ok().map(|f| f.size())).sum()).unwrap_or(0)
}

/// Free space on the drive of `dir` (for "Space available").
pub fn free_space(dir: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        if let Some(bytes) = win_shell::free_space(dir) {
            return Some(bytes);
        }
    }
    let drive = dir.components().next()?.as_os_str().to_string_lossy().trim_end_matches(':').to_string();
    if !drive.chars().all(|c| c.is_ascii_alphanumeric()) || drive.is_empty() {
        return None;
    }
    let out = powershell_with_env("(Get-PSDrive -Name $env:AURORA_PS_DRIVE).Free", &[("AURORA_PS_DRIVE", std::ffi::OsStr::new(&drive))]).ok()?;
    out.trim().parse().ok()
}

/// Start the installed app.
pub fn launch(dir: &Path) {
    let _ = std::process::Command::new(dir.join(APP_EXE)).current_dir(dir).spawn();
}

fn hidden(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn reg(args: &[&str]) -> Option<String> {
    let out = hidden(std::process::Command::new("reg").args(args)).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn powershell_with_env(script: &str, envs: &[(&str, &std::ffi::OsStr)]) -> Result<String, String> {
    // Fail on any cmdlet/COM error (default Continue exits 0!) and ignore
    // invalid console handles in GUI builds with CREATE_NO_WINDOW.
    let script = format!("$ErrorActionPreference = 'Stop'; try {{ [Console]::OutputEncoding = [Text.Encoding]::UTF8 }} catch {{}}; {script}");
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = hidden(&mut cmd).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(windows)]
fn powershell(script: &str) -> Result<String, String> {
    powershell_with_env(script, &[])
}

#[cfg(windows)]
mod win_shell {
    use std::ffi::{OsStr, OsString, c_void};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows_sys::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize};
    use windows_sys::Win32::UI::Shell::{SHCNE_CREATE, SHCNE_UPDATEDIR, SHCNF_PATHW, SHChangeNotify, SHGetKnownFolderPath};
    use windows_sys::core::{GUID, HRESULT, PCWSTR, PWSTR};

    const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
    const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);

    fn wide_os(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn wide_str(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn known_folder(id: &GUID) -> Option<PathBuf> {
        let mut raw: PWSTR = std::ptr::null_mut();
        let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) };
        if hr < 0 || raw.is_null() {
            return None;
        }
        let mut len = 0usize;
        unsafe {
            while *raw.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(raw, len);
            let path = PathBuf::from(OsString::from_wide(slice));
            CoTaskMemFree(raw.cast());
            (!path.as_os_str().is_empty()).then_some(path)
        }
    }

    pub fn free_space(dir: &Path) -> Option<u64> {
        // Walk up to the nearest existing ancestor (the target folder may not
        // exist yet during setup), or fall back to the drive prefix (e.g. C:\).
        let probe: PathBuf = dir
            .ancestors()
            .find(|a| !a.as_os_str().is_empty() && a.exists())
            .map(Path::to_path_buf)
            .or_else(|| {
                let first = dir.components().next()?.as_os_str().to_string_lossy();
                Some(PathBuf::from(format!("{}\\", first.trim_end_matches(['\\', '/']))))
            })?;
        let w = wide_os(probe.as_os_str());
        let mut avail = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(avail)
    }

    pub fn notify_created(link: &Path) {
        let lw = wide_os(link.as_os_str());
        unsafe {
            SHChangeNotify(SHCNE_CREATE as i32, SHCNF_PATHW, lw.as_ptr().cast(), std::ptr::null());
        }
        if let Some(parent) = link.parent() {
            notify_dir(parent);
        }
    }

    pub fn notify_dir(dir: &Path) {
        let dw = wide_os(dir.as_os_str());
        unsafe {
            SHChangeNotify(SHCNE_UPDATEDIR as i32, SHCNF_PATHW, dw.as_ptr().cast(), std::ptr::null());
        }
    }

    #[repr(C)]
    struct IShellLinkW {
        vtbl: *const IShellLinkWVtbl,
    }

    #[repr(C)]
    struct IShellLinkWVtbl {
        query_interface: unsafe extern "system" fn(*mut IShellLinkW, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut IShellLinkW) -> u32,
        release: unsafe extern "system" fn(*mut IShellLinkW) -> u32,
        get_path: *const c_void,
        get_id_list: *const c_void,
        set_id_list: *const c_void,
        get_description: *const c_void,
        set_description: unsafe extern "system" fn(*mut IShellLinkW, PCWSTR) -> HRESULT,
        get_working_directory: *const c_void,
        set_working_directory: unsafe extern "system" fn(*mut IShellLinkW, PCWSTR) -> HRESULT,
        get_arguments: *const c_void,
        set_arguments: *const c_void,
        get_hotkey: *const c_void,
        set_hotkey: *const c_void,
        get_show_cmd: *const c_void,
        set_show_cmd: *const c_void,
        get_icon_location: *const c_void,
        set_icon_location: unsafe extern "system" fn(*mut IShellLinkW, PCWSTR, i32) -> HRESULT,
        set_relative_path: *const c_void,
        resolve: *const c_void,
        set_path: unsafe extern "system" fn(*mut IShellLinkW, PCWSTR) -> HRESULT,
    }

    #[repr(C)]
    struct IPersistFile {
        vtbl: *const IPersistFileVtbl,
    }

    #[repr(C)]
    struct IPersistFileVtbl {
        query_interface: unsafe extern "system" fn(*mut IPersistFile, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut IPersistFile) -> u32,
        release: unsafe extern "system" fn(*mut IPersistFile) -> u32,
        get_class_id: *const c_void,
        is_dirty: *const c_void,
        load: *const c_void,
        save: unsafe extern "system" fn(*mut IPersistFile, PCWSTR, i32) -> HRESULT,
        save_completed: *const c_void,
        get_cur_file: *const c_void,
    }

    struct ComInit(bool);
    impl Drop for ComInit {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    struct ShellLinkGuard(*mut IShellLinkW);
    impl Drop for ShellLinkGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { ((*(*self.0).vtbl).release)(self.0) };
            }
        }
    }

    struct PersistFileGuard(*mut IPersistFile);
    impl Drop for PersistFileGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { ((*(*self.0).vtbl).release)(self.0) };
            }
        }
    }

    fn check(step: &str, hr: HRESULT) -> Result<(), String> {
        if hr >= 0 {
            Ok(())
        } else {
            Err(format!("{step} failed (0x{:08x})", hr as u32))
        }
    }

    pub fn create_shortcut(link: &Path, target: &Path) -> Result<(), String> {
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        let _com = if hr == S_OK || hr == S_FALSE {
            ComInit(true)
        } else if hr == RPC_E_CHANGED_MODE {
            ComInit(false)
        } else {
            return check("CoInitializeEx", hr);
        };

        let mut sl_raw: *mut c_void = std::ptr::null_mut();
        check(
            "CoCreateInstance(CLSID_ShellLink)",
            unsafe { CoCreateInstance(&CLSID_SHELL_LINK, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELL_LINK_W, &mut sl_raw) },
        )?;
        let sl = ShellLinkGuard(sl_raw.cast());

        let target_w = wide_os(target.as_os_str());
        let dir_w = wide_os(target.parent().unwrap_or(target).as_os_str());
        let desc_w = wide_str("Aurora");
        let link_w = wide_os(link.as_os_str());

        unsafe {
            let vtbl = &*(*sl.0).vtbl;
            check("IShellLinkW::SetPath", (vtbl.set_path)(sl.0, target_w.as_ptr()))?;
            check("IShellLinkW::SetWorkingDirectory", (vtbl.set_working_directory)(sl.0, dir_w.as_ptr()))?;
            check("IShellLinkW::SetIconLocation", (vtbl.set_icon_location)(sl.0, target_w.as_ptr(), 0))?;
            check("IShellLinkW::SetDescription", (vtbl.set_description)(sl.0, desc_w.as_ptr()))?;

            let mut pf_raw: *mut c_void = std::ptr::null_mut();
            check("QueryInterface(IPersistFile)", (vtbl.query_interface)(sl.0, &IID_IPERSIST_FILE, &mut pf_raw))?;
            let pf = PersistFileGuard(pf_raw.cast());
            let pf_vtbl = &*(*pf.0).vtbl;
            check("IPersistFile::Save", (pf_vtbl.save)(pf.0, link_w.as_ptr(), 1))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_publish_restores_original_and_failed_probe_restore_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("app.exe");
        std::fs::write(&exe, b"working").unwrap();
        let stage = tempfile::tempdir_in(dir.path()).unwrap();
        assert!(install_staged_file(&exe, &stage.path().join("missing"), stage).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"working");
        let result = wait_unlocked_with(&exe, Duration::ZERO, |from, to| {
            if to == exe { Err(std::io::Error::other("restore failed")) } else { std::fs::rename(from, to) }
        });
        assert!(result.unwrap_err().contains("restore failed"));
        assert_eq!(std::fs::read(exe.with_extension("probe")).unwrap(), b"working");
    }

    #[test]
    fn safe_manifest_rel_rejects_traversal_roots_and_drives() {
        assert_eq!(safe_manifest_rel("aurora.exe"), Some(PathBuf::from("aurora.exe")));
        assert_eq!(safe_manifest_rel("sub/file.dll"), Some(PathBuf::from("sub/file.dll")));
        assert!(safe_manifest_rel("").is_none());
        assert!(safe_manifest_rel("../evil.exe").is_none());
        assert!(safe_manifest_rel("sub/../../evil.exe").is_none());
        assert!(safe_manifest_rel("/etc/passwd").is_none());
        assert!(safe_manifest_rel("C:evil.exe").is_none());
        assert!(safe_manifest_rel("sub/C:evil.exe").is_none());
        assert!(safe_manifest_rel("file:ads.txt").is_none());
        assert!(safe_manifest_rel("a\\b.txt").is_none());
    }

    #[test]
    fn remove_aurora_files_preserves_unrelated_user_files() {
        let dir = std::env::temp_dir().join(format!("aurora-installer-sec04-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(APP_EXE), b"app").unwrap();
        std::fs::write(dir.join("steamclient64.dll"), b"dll").unwrap();
        std::fs::write(dir.join("aurora-files.txt"), b"user-important.docx\n").unwrap();
        std::fs::write(dir.join("user-important.docx"), b"keep me").unwrap();

        let shared = Shared::default();
        remove_aurora_files(&dir, &shared);

        assert!(!dir.join(APP_EXE).exists());
        assert!(!dir.join("steamclient64.dll").exists());
        assert!(!dir.join("aurora-files.txt").exists());
        // Untrusted manifest entry must NOT authorize deleting unrelated files.
        assert!(dir.join("user-important.docx").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
