//! Steam install scripts (`installscript.vdf`): what Steam does on a game's
//! first launch ("Running install script…"). A script sets registry keys the
//! game reads and runs programs once (anti-cheat setup, redistributables).
//! Some games don't start without them (e.g. EasyAntiCheat isn't set up).
//!
//! Like Steam, a "Run Process" step is skipped once its `HasRunKey` registry
//! value is set (Steam ran it before), and we remember what we ran ourselves
//! (`install-scripts.txt` in the data folder) instead of writing Steam's
//! machine-wide keys, which needs admin rights. On Windows that's once per
//! PC (a redistributable installs for every game); on Linux, once per Proton
//! prefix (each game has its own Windows). Native Linux games have none.
//!
//! The registry is written in its 32-bit view (`/reg:32`), as Steam does.

use model::{AppId, OwnedApp};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Games whose scripts were run (or tried) this session: a step that fails
/// (admin rights refused) doesn't hold up every later Play.
static TRIED: Mutex<BTreeSet<AppId>> = Mutex::new(BTreeSet::new());

/// Script work left for a game, found by [`pending`].
pub struct Pending {
    app: OwnedApp,
    install_dir: PathBuf,
    steps: Vec<Step>,
}

struct Step {
    /// For the progress text, e.g. "EasyAntiCheat".
    label: String,
    /// Remembered once done.
    key: String,
    kind: Kind,
}

enum Kind {
    /// A `.reg` file's text (REGEDIT4).
    Registry(String),
    /// One or more programs (real path, args) in a single `Run Process` group.
    Run(Vec<(PathBuf, String)>),
}

/// Steps still to do before `app` starts, or `None`.
pub fn pending(app: &OwnedApp, install_dir: &Path) -> Option<Pending> {
    let mut tried = TRIED.lock().unwrap();
    if tried.contains(&app.app_id) {
        return None;
    }
    // Native Linux games don't run Windows scripts.
    #[cfg(not(windows))]
    if crate::launch::in_prefix(app, install_dir, "cmd", &[]).is_none() {
        return None;
    }
    let done = load_done();
    let scope = scope(app.app_id);
    let mut steps = Vec::new();
    for script in find(app, install_dir) {
        let Some(kv) = std::fs::read_to_string(&script).ok().and_then(|t| vdf::parse(t.trim_start_matches('\u{feff}')).ok()) else { continue };
        let Some(root) = kv.obj("installscript") else { continue };
        steps.extend(plan(root, &script, install_dir, &scope, app, &done));
    }
    if steps.is_empty() {
        return None;
    }
    // Once a session: a step that fails (admin rights refused) doesn't hold
    // up every later Play, and Play pressed again doesn't run it twice.
    tried.insert(app.app_id);
    Some(Pending { app: app.clone(), install_dir: install_dir.to_path_buf(), steps })
}

/// Run the steps (blocking). `progress` gets each step's label.
pub fn run(p: Pending, mut progress: impl FnMut(&str)) {
    for step in &p.steps {
        progress(&step.label);
        let ok = match &step.kind {
            Kind::Registry(text) => import(&p.app, &p.install_dir, text),
            Kind::Run(procs) => {
                let mut res = Ok(());
                for (exe, args) in procs {
                    if let Err(e) = run_process(&p.app, &p.install_dir, exe, args) {
                        res = Err(e);
                        break;
                    }
                }
                res
            }
        };
        match ok {
            Ok(()) => {
                tracing::info!("install script for {}: {} done", p.app.app_id, step.label);
                mark_done(&step.key);
            }
            Err(e) => tracing::warn!("install script for {}: {} failed: {e}", p.app.app_id, step.label),
        }
    }
}

/// Resolve a relative path strictly inside `install_dir` (no `..`, no absolute
/// paths, no symlink escape).
fn resolve_inside_install_dir(install_dir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains('\0') || rel.contains(':') || rel.starts_with('/') || rel.starts_with('\\') {
        return None;
    }
    let canon_root = std::fs::canonicalize(install_dir).ok()?;
    let mut cur = canon_root.clone();
    let mut pushed = false;
    for seg in rel.split(['/', '\\']) {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        cur.push(seg);
        let meta = std::fs::symlink_metadata(&cur).ok()?;
        if meta.file_type().is_symlink() {
            return None;
        }
        pushed = true;
    }
    if !pushed {
        return None;
    }
    let canon = std::fs::canonicalize(&cur).ok()?;
    (canon.starts_with(&canon_root) && canon != canon_root && canon.is_file()).then_some(canon)
}

/// The game's scripts: the one its app info names, and any other
/// `installscript` files near the top of its folder (most games only flag
/// theirs in the depot manifest).
fn find(app: &OwnedApp, install_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(rel) = &app.install_script {
        if let Some(p) = resolve_inside_install_dir(install_dir, rel) {
            out.push(p);
        } else {
            tracing::warn!("ignoring unsafe install_script path {:?} for app {}", rel, app.app_id);
        }
    }
    walk(install_dir, 0, &mut out);
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let path = e.path();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() && depth < 3 {
            walk(&path, depth + 1, out);
        } else if kind.is_file() && path.extension().is_some_and(|x| x.eq_ignore_ascii_case("vdf")) && is_script(&path) {
            out.push(path);
        }
    }
}

/// Starts with the `"installscript"` key (any case)?
fn is_script(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 256];
    let Ok(n) = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) else { return false };
    let text = String::from_utf8_lossy(&head[..n]);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.get(..15).is_some_and(|t| t.eq_ignore_ascii_case("\"installscript\""))
}

/// Where "already done" applies: the whole PC on Windows, the game's
/// prefix on Linux.
fn scope(app: AppId) -> String {
    if cfg!(windows) { "pc".into() } else { format!("app{app}") }
}

fn plan(root: &vdf::Obj, script: &Path, install_dir: &Path, scope: &str, app: &OwnedApp, done: &BTreeSet<String>) -> Vec<Step> {
    let mut steps = Vec::new();
    let vars = Vars::new(install_dir);
    // Registry keys: once per script version.
    if let Some(reg) = root.obj("registry") {
        let stamp = std::fs::metadata(script).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let key = format!("{scope}|reg|{}|{stamp}", script.display());
        let text = reg_file(reg, &vars);
        if !done.contains(&key) && text.lines().count() > 2 {
            steps.push(Step { label: "Registry".into(), key, kind: Kind::Registry(text) });
        }
    }
    let Some(runs) = root.obj("run process") else { return steps };
    for (name, v) in runs.iter() {
        let Some(o) = v.as_obj() else { continue };
        if !os_ok(o) {
            continue;
        }
        let has_run = o.str("hasrunkey").unwrap_or_default();
        let key = format!("{scope}|run|{}|{name}", if has_run.is_empty() { script.display().to_string() } else { has_run.to_lowercase() });
        let minimum = o.u64("minimumhasrunvalue").unwrap_or(1);
        if done.contains(&key) || (!has_run.is_empty() && has_run_value(app, install_dir, has_run, name).is_some_and(|v| v >= minimum)) {
            continue;
        }
        let mut procs = Vec::new();
        for n in 1.. {
            let Some(process) = o.str(&format!("process {n}")) else { break };
            let Some(exe) = vars.real_path(&vars.expand(process)) else {
                tracing::warn!("install script step {name}: refusing unsafe process path {:?}", process);
                continue;
            };
            let args = vars.expand(o.str(&format!("command {n}")).unwrap_or_default());
            if !exe.is_file() {
                tracing::debug!("install script step {name}: {} isn't there", exe.display());
                continue;
            }
            procs.push((exe, args));
        }
        if !procs.is_empty() {
            steps.push(Step { label: name.to_string(), key, kind: Kind::Run(procs) });
        }
    }
    steps
}

/// `Requirement_OS`: only "64-bit Windows" (or 32-bit) matters here; the
/// prefix and every Windows we run on is 64-bit.
fn os_ok(o: &vdf::Obj) -> bool {
    match o.obj("requirement_os").and_then(|r| r.str("is64bitwindows")) {
        Some(v) => v.trim() != "0",
        None => true,
    }
}

/// `%INSTALLDIR%` and friends, as the game sees them.
struct Vars {
    install_dir: PathBuf,
}

impl Vars {
    fn new(install_dir: &Path) -> Self {
        Self { install_dir: install_dir.to_path_buf() }
    }

    /// A Windows path for something on this disk (Linux: Wine's `Z:` drive).
    fn win_path(p: &Path) -> String {
        if cfg!(windows) { p.display().to_string() } else { format!("Z:{}", p.display().to_string().replace('/', "\\")) }
    }

    fn expand(&self, s: &str) -> String {
        let install = Self::win_path(&self.install_dir);
        let windir = if cfg!(windows) { std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) } else { "C:\\windows".into() };
        let mut out = s.to_string();
        for (var, value) in [("%INSTALLDIR%", install.as_str()), ("%ROOTDRIVE%", "C:"), ("%WINDIR%", windir.as_str()), ("%LANGUAGE%", "english")] {
            out = replace_ci(&out, var, value);
        }
        out
    }

    /// The real file for an expanded Windows path strictly inside the install folder.
    fn real_path(&self, win: &str) -> Option<PathBuf> {
        let install = Self::win_path(&self.install_dir);
        let prefix = win.get(..install.len())?;
        if !prefix.eq_ignore_ascii_case(&install) {
            return None;
        }
        let rest = &win[install.len()..];
        if !rest.is_empty() && !rest.starts_with(['\\', '/']) {
            return None;
        }
        let rel = rest.trim_start_matches(['\\', '/']);
        resolve_inside_install_dir(&self.install_dir, rel)
    }
}

fn is_safe_reg_key(key: &str) -> bool {
    if key.is_empty()
        || key.contains(['\r', '\n', '\0', '[', ']'])
        || key.chars().any(char::is_control)
    {
        return false;
    }
    let norm = key.replace('/', "\\");
    let lower = norm.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("hkey_local_machine\\software\\")
        .or_else(|| lower.strip_prefix("hkey_current_user\\software\\"))
    else {
        return false;
    };
    !rest.is_empty()
        && !rest.starts_with("microsoft\\windows\\currentversion\\run")
        && !rest.starts_with("microsoft\\windows nt\\currentversion\\image file execution options")
        && !rest.starts_with("wow6432node\\microsoft\\windows\\currentversion\\run")
        && !rest.starts_with("wow6432node\\microsoft\\windows nt\\currentversion\\image file execution options")
}

fn has_reg_control(s: &str) -> bool {
    s.contains(['\r', '\n', '\0']) || s.chars().any(char::is_control)
}

fn replace_ci(s: &str, var: &str, value: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let needle = var.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while let Some(at) = lower[i..].find(&needle) {
        out.push_str(&s[i..i + at]);
        out.push_str(value);
        i += at + needle.len();
    }
    out.push_str(&s[i..]);
    out
}

/// The `Registry` section as a `.reg` file: string and dword values, with
/// the English ones of language-specific keys.
fn reg_file(reg: &vdf::Obj, vars: &Vars) -> String {
    let mut out = String::from("REGEDIT4\r\n\r\n");
    for (key, v) in reg.iter() {
        if !is_safe_reg_key(key) {
            tracing::warn!("install script: skipping disallowed registry key {:?}", key);
            continue;
        }
        let Some(o) = v.as_obj() else { continue };
        let mut lines = Vec::new();
        for src in [Some(o), o.obj("english")].into_iter().flatten() {
            for (kind, values) in src.iter() {
                let Some(values) = values.as_obj() else { continue };
                for (name, value) in values.iter() {
                    let Some(value) = value.as_str() else { continue };
                    if has_reg_control(name) {
                        continue;
                    }
                    let name = if name.is_empty() { "@".to_string() } else { format!("\"{}\"", esc(name)) };
                    if kind.eq_ignore_ascii_case("string") {
                        let expanded = vars.expand(value);
                        if has_reg_control(&expanded) {
                            continue;
                        }
                        lines.push(format!("{name}=\"{}\"", esc(&expanded)));
                    } else if kind.eq_ignore_ascii_case("dword") {
                        let n: i64 = value.trim().parse().unwrap_or(0);
                        lines.push(format!("{name}=dword:{:08x}", n as u32));
                    }
                }
            }
        }
        if !lines.is_empty() {
            out.push_str(&format!("[{key}]\r\n{}\r\n\r\n", lines.join("\r\n")));
        }
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The value `name` under a `HasRunKey` (Steam sets it once a step ran).
fn has_run_value(app: &OwnedApp, install_dir: &Path, key: &str, name: &str) -> Option<u64> {
    #[cfg(windows)]
    {
        let _ = (app, install_dir);
        let out = quiet(Command::new("reg").args(["query", key, "/v", name, "/reg:32"]))
            .stdout(std::process::Stdio::piped())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let hex = text.lines().find(|l| l.contains("REG_DWORD"))?.split_whitespace().last()?;
        return u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok();
    }
    #[cfg(not(windows))]
    {
        // The prefix's registry files: HKLM in system.reg, HKCU in user.reg.
        let pfx = local::compat::compat_data(install_dir, app.app_id)?.join("pfx");
        let (file, sub) = match key.split_once('\\') {
            Some((hive, sub)) if hive.eq_ignore_ascii_case("HKEY_LOCAL_MACHINE") => ("system.reg", sub),
            Some((hive, sub)) if hive.eq_ignore_ascii_case("HKEY_CURRENT_USER") => ("user.reg", sub),
            _ => return None,
        };
        let text = std::fs::read_to_string(pfx.join(file)).ok()?;
        // Steam writes the 32-bit view (Wow6432Node); look in both.
        let wow = match sub.split_once('\\') {
            Some((first, rest)) if first.eq_ignore_ascii_case("software") => format!("Software\\Wow6432Node\\{rest}"),
            _ => sub.to_string(),
        };
        [sub.to_string(), wow].iter().find_map(|k| reg_text_dword(&text, k, name))
    }
}

/// A dword in a Wine `.reg` file (`[Key\\Sub] …` then `"name"=dword:…`).
#[cfg(not(windows))]
fn reg_text_dword(text: &str, key: &str, name: &str) -> Option<u64> {
    let header = format!("[{}]", key.replace('\\', "\\\\")).to_ascii_lowercase();
    let mut lines = text.lines().skip_while(|l| !l.to_ascii_lowercase().starts_with(&header));
    lines.next()?;
    let want = format!("\"{}\"=dword:", name.to_ascii_lowercase());
    lines.take_while(|l| !l.starts_with('[')).find_map(|l| l.to_ascii_lowercase().strip_prefix(&want).and_then(|h| u64::from_str_radix(h.trim(), 16).ok()))
}

/// `reg import` of the script's keys (an admin prompt on Windows when it
/// writes to HKEY_LOCAL_MACHINE and a plain import isn't allowed).
fn import(app: &OwnedApp, install_dir: &Path, text: &str) -> Result<(), String> {
    let file = data_dir().join(format!("installscript-{}.reg", app.app_id));
    std::fs::write(&file, text).map_err(|e| e.to_string())?;
    let result = (|| {
        #[cfg(windows)]
        {
            let _ = install_dir;
            let path = file.display().to_string();
            let status = quiet(Command::new("reg").args(["import", &path, "/reg:32"])).status().map_err(|e| e.to_string())?;
            if status.success() {
                return Ok(());
            }
            return elevated::run_wait(Path::new("reg.exe"), &format!("import \"{path}\" /reg:32"), &data_dir());
        }
        #[cfg(not(windows))]
        {
            let args = ["import".to_string(), Vars::win_path(&file), "/reg:32".to_string()];
            let mut cmd = crate::launch::in_prefix(app, install_dir, "reg", &args).ok_or("not a Proton game")?;
            let status = quiet(&mut cmd).status().map_err(|e| e.to_string())?;
            if status.success() { Ok(()) } else { Err(format!("reg import exited with {status}")) }
        }
    })();
    let _ = std::fs::remove_file(&file);
    result
}

fn installer_exit_ok(code: Option<i32>) -> bool {
    // 0 = success, 3010 = ERROR_SUCCESS_REBOOT_REQUIRED (common for VC++/DX redists).
    matches!(code, Some(0 | 3010))
}

/// Run one program and wait for it (an admin prompt on Windows when it asks
/// for one, like most redistributable installers).
fn run_process(app: &OwnedApp, install_dir: &Path, exe: &Path, args: &str) -> Result<(), String> {
    let dir = exe.parent().unwrap_or(install_dir).to_path_buf();
    let split = split_args(args);
    #[cfg(windows)]
    {
        let _ = app;
        match quiet(Command::new(exe).args(&split).current_dir(&dir)).status() {
            Ok(s) => {
                tracing::info!("{} exited with {s}", exe.display());
                if installer_exit_ok(s.code()) {
                    Ok(())
                } else {
                    Err(format!("{} exited with {s}", exe.display()))
                }
            }
            // ERROR_ELEVATION_REQUIRED
            Err(e) if e.raw_os_error() == Some(740) => elevated::run_wait(exe, args, &dir),
            Err(e) => Err(e.to_string()),
        }
    }
    #[cfg(not(windows))]
    {
        let is_batch = exe.extension().is_some_and(|x| x.eq_ignore_ascii_case("cmd") || x.eq_ignore_ascii_case("bat"));
        let mut cmd = if is_batch {
            let mut a = vec!["/c".to_string(), Vars::win_path(exe)];
            a.extend(split);
            crate::launch::in_prefix(app, install_dir, "cmd", &a)
        } else {
            crate::launch::in_prefix(app, install_dir, &exe.display().to_string(), &split)
        }
        .ok_or("not a Proton game")?;
        cmd.current_dir(&dir);
        let status = quiet(&mut cmd).status().map_err(|e| e.to_string())?;
        tracing::info!("{} exited with {status}", exe.display());
        if installer_exit_ok(status.code()) {
            Ok(())
        } else {
            Err(format!("{} exited with {status}", exe.display()))
        }
    }
}

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

/// No console window on Windows; output isn't ours to show.
fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
}

fn data_dir() -> PathBuf {
    let dir = directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust").map(|d| d.data_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn done_file() -> PathBuf {
    data_dir().join("install-scripts.txt")
}

fn load_done() -> BTreeSet<String> {
    std::fs::read_to_string(done_file()).map(|t| t.lines().map(String::from).collect()).unwrap_or_default()
}

fn mark_done(key: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(done_file()) {
        let _ = writeln!(f, "{key}");
    }
}

/// Programs that need admin rights: started through the shell's "runas"
/// (the UAC prompt), and waited for.
#[cfg(windows)]
mod elevated {
    use std::ffi::c_void;
    use std::path::Path;

    #[repr(C)]
    struct ShellExecuteInfo {
        size: u32,
        mask: u32,
        hwnd: isize,
        verb: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
        inst_app: isize,
        id_list: *mut c_void,
        class: *const u16,
        hkey_class: isize,
        hot_key: u32,
        icon_or_monitor: isize,
        process: isize,
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
        fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn run_wait(exe: &Path, args: &str, dir: &Path) -> Result<(), String> {
        const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;
        const SEE_MASK_NOASYNC: u32 = 0x100;
        const SW_HIDE: i32 = 0;
        let (verb, file, params, d) = (wide("runas"), wide(&exe.to_string_lossy()), wide(args), wide(&dir.to_string_lossy()));
        let mut info = ShellExecuteInfo {
            size: std::mem::size_of::<ShellExecuteInfo>() as u32,
            mask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            hwnd: 0,
            verb: verb.as_ptr(),
            file: file.as_ptr(),
            params: params.as_ptr(),
            dir: d.as_ptr(),
            show: SW_HIDE,
            inst_app: 0,
            id_list: std::ptr::null_mut(),
            class: std::ptr::null(),
            hkey_class: 0,
            hot_key: 0,
            icon_or_monitor: 0,
            process: 0,
        };
        // SAFETY: the strings outlive the call; `info` is a valid SHELLEXECUTEINFOW.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            // 1223: the player said no to the admin prompt.
            return Err(format!("couldn't start {} with admin rights (error {})", exe.display(), unsafe { GetLastError() }));
        }
        let mut exit_code: u32 = 0;
        let mut got_code = false;
        if info.process != 0 {
            // SAFETY: a process handle we own (SEE_MASK_NOCLOSEPROCESS).
            unsafe {
                WaitForSingleObject(info.process, u32::MAX);
                got_code = GetExitCodeProcess(info.process, &mut exit_code) != 0;
                CloseHandle(info.process);
            }
        }
        if got_code && !super::installer_exit_ok(Some(exit_code as i32)) {
            return Err(format!("{} exited with code {exit_code}", exe.display()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_and_maps_install_dir() {
        let dir = std::env::temp_dir().join(format!("aurora-inst-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let eac = dir.join("EasyAntiCheat").join("EasyAntiCheat_EOS_Setup.exe");
        std::fs::create_dir_all(eac.parent().unwrap()).unwrap();
        std::fs::write(&eac, b"ok").unwrap();
        let v = Vars::new(&dir);
        let win = v.expand("%INSTALLDIR%\\EasyAntiCheat\\EasyAntiCheat_EOS_Setup.exe");
        assert_eq!(v.real_path(&win), Some(std::fs::canonicalize(&eac).unwrap()));
        assert_eq!(v.expand("%installdir%"), Vars::win_path(&dir));
        // Reject traversal and arbitrary external executables.
        assert_eq!(v.real_path(&v.expand("%INSTALLDIR%\\..\\escape.exe")), None);
        assert_eq!(v.real_path("C:\\Windows\\System32\\cmd.exe"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reg_file_rejects_disallowed_keys_and_newline_injection() {
        let kv = vdf::parse(
            r#""installscript" { "Registry" { "HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion\\Run" { "string" { "Evil" "calc.exe" } } "HKEY_LOCAL_MACHINE\\Software\\Game" { "string" { "Good" "ok" } } } }"#,
        )
        .unwrap();
        let v = Vars::new(Path::new("/g"));
        let text = reg_file(kv.path(&["installscript", "registry"]).unwrap(), &v);
        assert!(!text.contains("CurrentVersion\\Run"));
        assert!(text.contains("[HKEY_LOCAL_MACHINE\\Software\\Game]"));
        assert!(!is_safe_reg_key("HKEY_LOCAL_MACHINE\\Software\\Game]\r\n[HKEY_LOCAL_MACHINE\\Bad"));
    }

    #[test]
    fn reg_file_from_script() {
        let kv = vdf::parse(
            r#""installscript" { "Registry" { "HKEY_LOCAL_MACHINE\\Software\\Game" { "string" { "InstallPath" "%INSTALLDIR%" } "dword" { "Version" "3" } "english" { "string" { "Language" "english" } } } } }"#,
        )
        .unwrap();
        let v = Vars::new(Path::new("/g"));
        let text = reg_file(kv.path(&["installscript", "registry"]).unwrap(), &v);
        assert!(text.contains("[HKEY_LOCAL_MACHINE\\Software\\Game]"));
        assert!(text.contains("\"Version\"=dword:00000003"));
        assert!(text.contains("\"Language\"=\"english\""));
        assert!(text.contains(&format!("\"InstallPath\"=\"{}\"", esc(&Vars::win_path(Path::new("/g"))))));
    }

    #[test]
    fn skips_32_bit_only_steps_and_finds_has_run() {
        let kv = vdf::parse(r#""a" { "Requirement_OS" { "Is64BitWindows" "0" } }"#).unwrap();
        assert!(!os_ok(kv.obj("a").unwrap()));
        #[cfg(not(windows))]
        {
            let reg = "[Software\\\\Wow6432Node\\\\Valve\\\\Steam\\\\Apps\\\\1304930] 1700000000\n\"EasyAntiCheat\"=dword:00000001\n";
            assert_eq!(reg_text_dword(reg, "Software\\Wow6432Node\\Valve\\Steam\\Apps\\1304930", "EasyAntiCheat"), Some(1));
        }
    }
}
