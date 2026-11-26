//! Steam Play compatibility tools on Linux (Proton and friends): which ones
//! are installed, which one a game is set to use, and the Steam Linux
//! Runtime a tool asks to run inside. Read from the Steam install, the same
//! files Steam itself uses.

use model::AppId;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct CompatTool {
    /// Internal name, as in config.vdf's CompatToolMapping (`proton_11`,
    /// `GE-Proton10-34`, `proton-cachyos-native`).
    pub name: String,
    /// Name to show (`Proton 11.0`).
    pub display: String,
    /// Direct Wine executable on FreeBSD. Proton tools use their script instead.
    pub wine_bin: Option<PathBuf>,
    /// Folder holding the `proton` script.
    pub dir: PathBuf,
    /// Steam Linux Runtime app the tool asks to run inside (toolmanifest.vdf
    /// `require_tool_appid`), if any.
    pub runtime_app: Option<AppId>,
}

impl CompatTool {
    pub fn script(&self) -> PathBuf {
        self.dir.join("proton")
    }
}

/// FreeBSD also supports standalone Wine without a Steam installation. Use
/// the conventional Steam tools directory for optional custom Proton scripts.
pub fn tool_root() -> Option<PathBuf> {
    let root = crate::find_steam_root();
    #[cfg(target_os = "freebsd")]
    let root = root.or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/Steam")))
        .or_else(|| Some(PathBuf::from("/usr/local/share/steam")));
    root
}

#[cfg(any(target_os = "freebsd", all(test, unix)))]
fn discover_wine(port_root: &Path, path: &[PathBuf]) -> Vec<CompatTool> {
    use std::os::unix::fs::PermissionsExt;
    let mut tools = Vec::new();
    for (name, display) in [("wine-proton", "Wine Proton"), ("wine", "Wine"), ("wine-devel", "Wine Development")] {
        let binary = std::iter::once(port_root.join(name).join("bin/wine"))
            .chain(path.iter().filter(|dir| dir.is_absolute()).map(|dir| dir.join(name)))
            .chain(std::iter::once(port_root.join("bin").join(name)))
            .find(|bin| bin.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0));
        if let Some(bin) = binary {
            tools.push(CompatTool {
                dir: bin.parent().unwrap().to_owned(),
                wine_bin: Some(bin),
                display: display.into(),
                name: name.into(),
                runtime_app: None,
            });
        }
    }
    tools
}

fn read_vdf(path: &Path) -> Option<vdf::Obj> {
    vdf::parse(&std::fs::read_to_string(path).ok()?).ok()
}

fn runtime_of(dir: &Path) -> Option<AppId> {
    let m = read_vdf(&dir.join("toolmanifest.vdf"))?;
    m.obj("manifest")?.u64("require_tool_appid").map(|v| v as AppId)
}

/// Every installed tool: Valve's Proton apps in the Steam libraries, then
/// custom tools in `compatibilitytools.d` (the user's and the system's).
pub fn compat_tools(root: &Path) -> Vec<CompatTool> {
    #[cfg(target_os = "freebsd")]
    let mut out = discover_wine(Path::new("/usr/local"), &std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>());
    #[cfg(not(target_os = "freebsd"))]
    let mut out = Vec::new();
    for lib in crate::library_folders(root).unwrap_or_default() {
        let apps = lib.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&apps) else { continue };
        for e in entries.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            let Some(m) = read_vdf(&e.path()) else { continue };
            let Some(state) = m.obj("AppState") else { continue };
            let name = state.str("name").unwrap_or_default();
            let Some(internal) = valve_internal_name(name) else { continue };
            let Some(installdir) = state.str("installdir").filter(|s| crate::is_valid_folder_name(s)) else { continue };
            let dir = apps.join("common").join(installdir);
            if dir.join("proton").is_file() {
                out.push(CompatTool { wine_bin: None, name: internal, display: name.to_string(), runtime_app: runtime_of(&dir), dir });
            }
        }
    }
    let mut dirs = vec![root.join("compatibilitytools.d")];
    dirs.push(PathBuf::from("/usr/share/steam/compatibilitytools.d"));
    dirs.push(PathBuf::from("/usr/local/share/steam/compatibilitytools.d"));
    for base in dirs {
        let Ok(entries) = std::fs::read_dir(&base) else { continue };
        for e in entries.flatten() {
            let folder = e.path();
            if !folder.file_name().and_then(|n| n.to_str()).is_some_and(crate::is_valid_folder_name) {
                continue;
            }
            let Some(m) = read_vdf(&folder.join("compatibilitytool.vdf")) else { continue };
            let Some(tools) = m.obj("compatibilitytools").and_then(|c| c.obj("compat_tools")) else { continue };
            for (name, v) in tools.iter() {
                let Some(t) = v.as_obj() else { continue };
                let dir = match t.str("install_path") {
                    None | Some(".") => folder.clone(),
                    Some(sub) if crate::is_valid_folder_name(sub) => folder.join(sub),
                    Some(_) => continue,
                };
                if !dir.join("proton").is_file() || out.iter().any(|x: &CompatTool| x.name == name) {
                    continue;
                }
                out.push(CompatTool { wine_bin: None, name: name.to_string(), display: t.str("display_name").unwrap_or(name).to_string(), runtime_app: runtime_of(&dir), dir });
            }
        }
    }
    out
}

/// Steam's internal name for one of its own Proton apps, from the app name:
/// "Proton 11.0" -> proton_11, "Proton - Experimental" -> proton_experimental,
/// "Proton 9.0 (Beta)" -> proton_9, "Proton Hotfix" -> proton_hotfix.
fn valve_internal_name(app_name: &str) -> Option<String> {
    let rest = app_name.strip_prefix("Proton")?.trim();
    let lower = rest.trim_start_matches('-').trim().to_lowercase();
    if lower.starts_with("experimental") {
        return Some("proton_experimental".into());
    }
    if lower.starts_with("hotfix") {
        return Some("proton_hotfix".into());
    }
    let major: String = lower.chars().take_while(|c| c.is_ascii_digit()).collect();
    if major.is_empty() {
        return None;
    }
    // Proton 3.x and 4.x used dotted names (proton_316, proton_411).
    let minor: String = lower[major.len()..].trim_start_matches('.').chars().take_while(|c| c.is_ascii_digit()).collect();
    match major.parse::<u32>() {
        Ok(m) if m < 5 && !minor.is_empty() => Some(format!("proton_{major}{minor}")),
        _ => Some(format!("proton_{major}")),
    }
}

/// The tool set for `app` in Steam (Properties -> Compatibility), and
/// whether it was chosen for this game specifically (`false` = the global
/// default for all games).
fn mapped_tool(root: &Path, app: AppId) -> Option<(String, bool)> {
    let cfg = read_vdf(&root.join("config").join("config.vdf"))?;
    let map = cfg.path(&["InstallConfigStore", "Software", "Valve", "Steam", "CompatToolMapping"])?;
    let name_of = |key: &str| map.obj(key).and_then(|o| o.str("name")).filter(|n| !n.is_empty()).map(String::from);
    name_of(&app.to_string()).map(|n| (n, true)).or_else(|| name_of("0").map(|n| (n, false)))
}

/// Whether the player forced a tool on this game in Steam (e.g. Proton for
/// a game that also has a Linux version).
pub fn forced_tool(root: &Path, app: AppId) -> bool {
    mapped_tool(root, app).is_some_and(|(_, own)| own)
}

/// The tool to run `app` with: its own choice in Steam, else Steam's
/// default, else the best installed Proton (Experimental, then the newest).
pub fn tool_for(root: &Path, app: AppId) -> Option<CompatTool> {
    let tools = compat_tools(root);
    if let Some((name, _)) = mapped_tool(root, app) {
        if let Some(t) = tools.iter().find(|t| t.name.eq_ignore_ascii_case(&name)) {
            return Some(t.clone());
        }
    }
    #[cfg(target_os = "freebsd")]
    if let Some(tool) = tools.iter().find(|t| t.wine_bin.is_some()) {
        return Some(tool.clone());
    }
    let rank = |t: &CompatTool| -> (u8, u32) {
        if t.name == "proton_experimental" {
            return (2, 0);
        }
        let n = t.name.strip_prefix("proton_").and_then(|v| v.parse().ok()).unwrap_or(0);
        (u8::from(n > 0), n)
    };
    tools.into_iter().max_by_key(rank)
}

/// Install folder of any app (e.g. a Steam Linux Runtime), from its
/// appmanifest in whichever library holds it.
pub fn app_dir(root: &Path, app: AppId) -> Option<PathBuf> {
    crate::library_folders(root).unwrap_or_default().into_iter().find_map(|lib| {
        let apps = lib.join("steamapps");
        let m = read_vdf(&apps.join(format!("appmanifest_{app}.acf")))?;
        let installdir = m.obj("AppState")?.str("installdir").filter(|s| crate::is_valid_folder_name(s))?;
        let dir = apps.join("common").join(installdir);
        dir.is_dir().then_some(dir)
    })
}

/// The game's Proton prefix folder, `<library>/steamapps/compatdata/<app>`
/// (Steam keeps it in the library the game is installed in).
pub fn compat_data(install_dir: &Path, app: AppId) -> Option<PathBuf> {
    let leaf = install_dir.file_name()?.to_str()?;
    if !crate::is_valid_folder_name(leaf) {
        return None;
    }
    if !install_dir.components().all(|c| matches!(c, std::path::Component::Prefix(_) | std::path::Component::RootDir | std::path::Component::Normal(_))) {
        return None;
    }
    let common = install_dir.parent()?;
    let steamapps = common.parent()?;
    Some(steamapps.join("compatdata").join(app.to_string()))
}

/// The Windows user folder inside a game's Proton prefix, where its saves
/// go (`…/pfx/drive_c/users/steamuser`), if the prefix exists.
pub fn prefix_user_dir(install_dir: &Path, app: AppId) -> Option<PathBuf> {
    let dir = compat_data(install_dir, app)?.join("pfx").join("drive_c").join("users").join("steamuser");
    #[cfg(target_os = "freebsd")]
    if !dir.is_dir() {
        let user = std::env::var("USER").ok().filter(|name| crate::is_valid_folder_name(name))?;
        let native = dir.parent()?.join(user);
        return native.is_dir().then_some(native);
    }
    dir.is_dir().then_some(dir)
}

#[cfg(any(target_os = "freebsd", all(test, unix)))]
const RUNTIME_PINNED_SUBDIRS: &[&str] = &["pinned_libs_64", "pinned_libs_32"];

#[cfg(any(target_os = "freebsd", all(test, unix)))]
const SCOUT_LIB_SUBDIRS: &[&str] = &[
    "lib/x86_64-linux-gnu",
    "usr/lib/x86_64-linux-gnu",
    "lib/i386-linux-gnu",
    "usr/lib/i386-linux-gnu",
    "lib",
    "usr/lib",
    "amd64/lib/x86_64-linux-gnu",
    "amd64/usr/lib/x86_64-linux-gnu",
    "amd64/lib",
    "amd64/usr/lib",
    "i386/lib/i386-linux-gnu",
    "i386/usr/lib/i386-linux-gnu",
    "i386/lib",
    "i386/usr/lib",
];

#[cfg(any(target_os = "freebsd", all(test, unix)))]
const CONTAINER_LIB_SUBDIRS: &[&str] = &[
    "lib/x86_64-linux-gnu",
    "usr/lib/x86_64-linux-gnu",
    "lib/i386-linux-gnu",
    "usr/lib/i386-linux-gnu",
    "lib",
    "usr/lib",
    "lib64",
    "usr/lib64",
];

#[cfg(any(target_os = "freebsd", all(test, unix)))]
const LSU_LIB_SUBDIRS: &[&str] = &["lib64", "lib32", "lib", "lib/x86_64-linux-gnu", "lib/i386-linux-gnu"];

#[cfg(any(target_os = "freebsd", all(test, unix)))]
const LINUX_COMPAT_SUBDIRS: &[&str] = &[
    "lib64",
    "usr/lib64",
    "lib/x86_64-linux-gnu",
    "usr/lib/x86_64-linux-gnu",
    "lib",
    "usr/lib",
    "lib/i386-linux-gnu",
    "usr/lib/i386-linux-gnu",
];

#[cfg(any(target_os = "freebsd", all(test, unix)))]
fn push_unique_dir(
    out: &mut Vec<PathBuf>,
    seen: &mut std::collections::HashSet<PathBuf>,
    dir: PathBuf,
    resolve_symlinks: bool,
) {
    if !dir.is_dir() {
        return;
    }
    let key = if resolve_symlinks {
        std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())
    } else {
        dir.clone()
    };
    if seen.insert(key) {
        out.push(dir);
    }
}

#[cfg(any(target_os = "freebsd", all(test, unix)))]
fn push_existing_subdirs(
    out: &mut Vec<PathBuf>,
    seen: &mut std::collections::HashSet<PathBuf>,
    base: &Path,
    subdirs: &[&str],
    resolve_symlinks: bool,
) {
    for sub in subdirs {
        push_unique_dir(out, seen, base.join(sub), resolve_symlinks);
    }
}

#[cfg(any(target_os = "freebsd", all(test, unix)))]
fn manifest_install_dir(lib: &Path, app_id: AppId) -> Option<PathBuf> {
    let apps = lib.join("steamapps");
    let m = read_vdf(&apps.join(format!("appmanifest_{app_id}.acf")))?;
    let installdir = m.obj("AppState")?.str("installdir").filter(|s| crate::is_valid_folder_name(s))?;
    let dir = apps.join("common").join(installdir);
    dir.is_dir().then_some(dir)
}

#[cfg(any(target_os = "freebsd", all(test, unix)))]
fn scout_dirs_for_root(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for cand in [
        Some(root.join("ubuntu12_32/steam-runtime")),
        Some(root.join("steamapps/common/SteamLinuxRuntime/steam-runtime")),
        manifest_install_dir(root, 1070560).map(|d| d.join("steam-runtime")),
    ]
    .into_iter()
    .flatten()
    {
        if cand.is_dir() && !dirs.iter().any(|d| d == &cand) {
            dirs.push(cand);
        }
    }
    dirs
}

#[cfg(any(target_os = "freebsd", all(test, unix)))]
fn has_container_lib_dir(files_dir: &Path) -> bool {
    files_dir.is_dir() && CONTAINER_LIB_SUBDIRS.iter().any(|sub| files_dir.join(sub).is_dir())
}

#[cfg(any(target_os = "freebsd", all(test, unix)))]
fn container_files_dir(runtime_base: &Path, platform_prefix: &str) -> Option<PathBuf> {
    // Steam's generated `run` script names the active platform directory directly.
    if let Ok(run_script) = std::fs::read_to_string(runtime_base.join("run")) {
        for line in run_script.lines() {
            if let Some(dir_name) = line.trim().strip_prefix("dir=") {
                let dir_name = dir_name.trim_matches(['"', '\'']);
                if crate::is_valid_folder_name(dir_name) {
                    let files = runtime_base.join(dir_name).join("files");
                    if has_container_lib_dir(&files) {
                        return Some(files);
                    }
                }
            }
        }
    }
    let mut platform_dirs: Vec<String> = std::fs::read_dir(runtime_base)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name.starts_with(platform_prefix) && entry.path().is_dir()).then_some(name)
        })
        .collect();
    platform_dirs.sort_by(|a, b| b.cmp(a));
    for name in platform_dirs {
        let files = runtime_base.join(name).join("files");
        if has_container_lib_dir(&files) {
            return Some(files);
        }
    }
    let direct = runtime_base.join("files");
    has_container_lib_dir(&direct).then_some(direct)
}

#[cfg(any(target_os = "freebsd", all(test, unix)))]
fn container_runtime_dirs_for_root(root: &Path, default_name: &str, app_id: AppId) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for cand in [
        Some(root.join("steamapps/common").join(default_name)),
        manifest_install_dir(root, app_id),
    ]
    .into_iter()
    .flatten()
    {
        if cand.is_dir() && !dirs.iter().any(|d| d == &cand) {
            dirs.push(cand);
        }
    }
    dirs
}

/// Collect Steam Linux Runtime (Scout / Soldier / Sniper / SteamRT 4) and
/// FreeBSD Linuxulator / `linux-steam-utils` library directories from explicit
/// roots.
///
/// Order prioritizes FreeBSD shims, Scout pinned libs, and `/compat/linux`
/// base system libraries before Steam runtime directories (so Linuxulator's
/// `libc.so.6`, `libGL.so.1`, and `libstdc++.so.6` are never shadowed by
/// container copies), followed by Scout -> Soldier -> Sniper -> SteamRT 4.
#[cfg(any(target_os = "freebsd", all(test, unix)))]
pub fn collect_linux_runtime_library_paths(
    steam_roots: &[PathBuf],
    lsu_roots: &[PathBuf],
    linux_compat_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for lsu in lsu_roots {
        push_existing_subdirs(&mut out, &mut seen, lsu, LSU_LIB_SUBDIRS, true);
    }

    let scout_dirs: Vec<PathBuf> = steam_roots.iter().flat_map(|r| scout_dirs_for_root(r)).collect();
    for scout in &scout_dirs {
        push_existing_subdirs(&mut out, &mut seen, scout, RUNTIME_PINNED_SUBDIRS, true);
    }

    // Keep /compat/linux paths un-canonicalized: absolute symlinks inside
    // /compat/linux (e.g. /usr/lib64) belong to Linuxulator, not the FreeBSD host.
    for compat in linux_compat_roots {
        push_existing_subdirs(&mut out, &mut seen, compat, LINUX_COMPAT_SUBDIRS, false);
    }

    for scout in &scout_dirs {
        push_existing_subdirs(&mut out, &mut seen, scout, SCOUT_LIB_SUBDIRS, true);
    }

    for &(default_name, app_id, platform_prefix) in &[
        ("SteamLinuxRuntime_soldier", 1391110, "soldier_platform_"),
        ("SteamLinuxRuntime_sniper", 1628350, "sniper_platform_"),
        ("SteamLinuxRuntime_4", 4183110, "steamrt4_platform_"),
    ] {
        for root in steam_roots {
            for rt_base in container_runtime_dirs_for_root(root, default_name, app_id) {
                if let Some(files) = container_files_dir(&rt_base, platform_prefix) {
                    push_existing_subdirs(&mut out, &mut seen, &files, CONTAINER_LIB_SUBDIRS, true);
                }
            }
        }
    }

    out
}

/// Discover Steam Linux Runtime and FreeBSD Linuxulator / LSU library paths
/// across all known Steam and Aurora library roots on this machine.
#[cfg(any(target_os = "freebsd", all(test, unix)))]
pub fn linux_runtime_library_paths(install_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen_roots = std::collections::HashSet::new();
    let mut push_root = |cand: PathBuf| {
        if !cand.is_dir() {
            return;
        }
        let key = std::fs::canonicalize(&cand).unwrap_or_else(|_| cand.clone());
        if seen_roots.insert(key) {
            roots.push(cand);
        }
    };

    if let Some(dir) = install_dir {
        if let Some(common) = dir.parent().filter(|p| p.file_name() == Some(std::ffi::OsStr::new("common")))
            && let Some(steamapps) = common.parent().filter(|p| p.file_name() == Some(std::ffi::OsStr::new("steamapps")))
            && let Some(lib) = steamapps.parent()
        {
            push_root(lib.to_path_buf());
        }
    }

    let mut base_roots: Vec<PathBuf> = Vec::new();
    if let Some(r) = crate::find_steam_root() {
        base_roots.push(r);
    }
    if let Some(r) = tool_root() {
        base_roots.push(r);
    }
    if let Some(r) = crate::existing_own_root() {
        base_roots.push(r);
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        base_roots.push(home.join(".local/share/Steam"));
        base_roots.push(home.join(".steam/steam"));
    }
    base_roots.push(PathBuf::from("/usr/local/share/steam"));

    for base in base_roots {
        if !base.is_dir() {
            continue;
        }
        if let Ok(libs) = crate::library_folders(&base) {
            for lib in libs {
                push_root(lib);
            }
        }
        push_root(base);
    }
    for lib in crate::extra_libraries() {
        push_root(lib);
    }

    let lsu_roots = [PathBuf::from("/usr/local/steam-utils")];
    let linux_compat_roots = [PathBuf::from("/compat/linux")];
    collect_linux_runtime_library_paths(&roots, &lsu_roots, &linux_compat_roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn standalone_wine_discovery() {
        use std::os::unix::fs::PermissionsExt;
        let temp = std::env::temp_dir().join(format!("aurora-wine-tools-{}", std::process::id()));
        let path = temp.join("custom bin");
        let proton = temp.join("wine-proton/bin/wine");
        std::fs::create_dir_all(proton.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        for bin in [&proton, &path.join("wine")] {
            std::fs::write(bin, b"").unwrap();
            std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(path.join("wine-devel"), b"").unwrap();
        let tools = discover_wine(&temp, std::slice::from_ref(&path));
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].wine_bin.as_ref(), Some(&proton));
        assert_eq!(tools[1].wine_bin, Some(path.join("wine")));
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn valve_names() {
        assert_eq!(valve_internal_name("Proton 11.0").as_deref(), Some("proton_11"));
        assert_eq!(valve_internal_name("Proton 9.0 (Beta)").as_deref(), Some("proton_9"));
        assert_eq!(valve_internal_name("Proton - Experimental").as_deref(), Some("proton_experimental"));
        assert_eq!(valve_internal_name("Proton Hotfix").as_deref(), Some("proton_hotfix"));
        assert_eq!(valve_internal_name("Proton 3.16").as_deref(), Some("proton_316"));
        assert_eq!(valve_internal_name("Proton EasyAntiCheat Runtime"), None);
        assert_eq!(valve_internal_name("Steam Linux Runtime 3.0 (sniper)"), None);
    }

    #[test]
    fn rejects_unsafe_installdir_and_install_path() {
        let root = std::env::temp_dir().join(format!("sir-compat-unsafe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("steamapps");
        let common = apps.join("common");
        std::fs::create_dir_all(&common).unwrap();
        // Plant `proton` in `steamapps/` so `common/..` would match `dir.join("proton").is_file()`!
        std::fs::write(apps.join("proton"), b"#!/bin/sh\n").unwrap();
        std::fs::write(
            apps.join("appmanifest_1493710.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\"1493710\"\n\t\"name\"\t\"Proton - Experimental\"\n\t\"installdir\"\t\"..\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            apps.join("appmanifest_1628350.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\"1628350\"\n\t\"name\"\t\"SteamLinuxRuntime_sniper\"\n\t\"installdir\"\t\"..\"\n}\n",
        )
        .unwrap();

        let custom = root.join("compatibilitytools.d");
        let escape_tool = custom.join("escape");
        std::fs::create_dir_all(&escape_tool).unwrap();
        std::fs::write(custom.join("proton"), b"#!/bin/sh\n").unwrap();
        std::fs::write(
            escape_tool.join("compatibilitytool.vdf"),
            "\"compatibilitytools\"\n{\n\t\"compat_tools\"\n\t{\n\t\t\"EscapeProton\"\n\t\t{\n\t\t\t\"install_path\"\t\"..\"\n\t\t}\n\t}\n}\n",
        )
        .unwrap();

        let empty_tool = custom.join("empty-path");
        std::fs::create_dir_all(&empty_tool).unwrap();
        std::fs::write(empty_tool.join("proton"), b"#!/bin/sh\n").unwrap();
        std::fs::write(
            empty_tool.join("compatibilitytool.vdf"),
            "\"compatibilitytools\"\n{\n\t\"compat_tools\"\n\t{\n\t\t\"EmptyPathProton\"\n\t\t{\n\t\t\t\"install_path\"\t\"\"\n\t\t}\n\t}\n}\n",
        )
        .unwrap();

        let valid_tool = custom.join("ge-proton");
        std::fs::create_dir_all(&valid_tool).unwrap();
        std::fs::write(valid_tool.join("proton"), b"#!/bin/sh\n").unwrap();
        std::fs::write(
            valid_tool.join("compatibilitytool.vdf"),
            "\"compatibilitytools\"\n{\n\t\"compat_tools\"\n\t{\n\t\t\"GE-Proton-Test\"\n\t\t{\n\t\t\t\"install_path\"\t\".\"\n\t\t}\n\t}\n}\n",
        )
        .unwrap();

        let tools: Vec<_> = compat_tools(&root)
            .into_iter()
            .filter(|t| matches!(t.name.as_str(), "proton_experimental" | "EscapeProton" | "EmptyPathProton" | "GE-Proton-Test"))
            .collect();
        assert_eq!(tools.len(), 1, "accepted unsafe compat tools: {tools:?}");
        assert_eq!(tools[0].name, "GE-Proton-Test");
        assert!(app_dir(&root, 1628350).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Reads this machine's Steam install when there is one (no assertions
    /// that depend on what's installed).
    #[test]
    #[ignore]
    fn lists_installed_tools() {
        let Some(root) = crate::find_steam_root() else { return };
        for t in compat_tools(&root) {
            println!("{} | {} | {} | runtime {:?}", t.name, t.display, t.dir.display(), t.runtime_app);
        }
        println!("default for 0: {:?}", tool_for(&root, 0).map(|t| t.name));
    }

    #[cfg(unix)]
    #[test]
    fn collects_steam_linux_runtime_and_freebsd_paths_in_priority_order() {
        let temp = std::env::temp_dir().join(format!("aurora-runtime-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);

        let lsu = temp.join("lsu");
        let compat_linux = temp.join("compat_linux");
        let steam_main = temp.join("steam_main");
        let steam_extra = temp.join("steam_extra");

        // LSU paths
        std::fs::create_dir_all(lsu.join("lib64")).unwrap();
        std::fs::create_dir_all(lsu.join("lib32")).unwrap();

        // /compat/linux paths
        std::fs::create_dir_all(compat_linux.join("lib64")).unwrap();
        std::fs::create_dir_all(compat_linux.join("usr/lib64")).unwrap();
        std::fs::create_dir_all(compat_linux.join("usr/lib")).unwrap();

        // Main Steam root: Scout (with pinned_libs + symlinked amd64/lib -> ../lib), Soldier, Sniper
        let scout = steam_main.join("ubuntu12_32/steam-runtime");
        std::fs::create_dir_all(scout.join("pinned_libs_64")).unwrap();
        std::fs::create_dir_all(scout.join("pinned_libs_32")).unwrap();
        std::fs::create_dir_all(scout.join("lib/x86_64-linux-gnu")).unwrap();
        std::fs::create_dir_all(scout.join("usr/lib/x86_64-linux-gnu")).unwrap();
        std::fs::create_dir_all(scout.join("lib/i386-linux-gnu")).unwrap();
        std::fs::create_dir_all(scout.join("amd64")).unwrap();
        std::os::unix::fs::symlink("../lib", scout.join("amd64/lib")).unwrap();

        // Soldier: empty newer dir + populated older dir
        let soldier = steam_main.join("steamapps/common/SteamLinuxRuntime_soldier");
        std::fs::create_dir_all(soldier.join("soldier_platform_2.0.9999")).unwrap();
        std::fs::create_dir_all(soldier.join("soldier_platform_2.0.1000/files/lib/x86_64-linux-gnu")).unwrap();
        std::fs::create_dir_all(soldier.join("soldier_platform_2.0.1000/files/lib/i386-linux-gnu")).unwrap();

        // Sniper: `run` script selects specific platform dir over another populated dir
        let sniper = steam_main.join("steamapps/common/SteamLinuxRuntime_sniper");
        std::fs::create_dir_all(sniper.join("sniper_platform_3.0.1/files/lib/x86_64-linux-gnu")).unwrap();
        std::fs::create_dir_all(sniper.join("sniper_platform_3.0.2/files/lib/x86_64-linux-gnu")).unwrap();
        std::fs::write(sniper.join("run"), "#!/bin/sh\ndir=sniper_platform_3.0.1\n").unwrap();

        // Extra library: Scout depot using amd64/i386 layout directly
        let extra_scout = steam_extra.join("steamapps/common/SteamLinuxRuntime/steam-runtime");
        std::fs::create_dir_all(extra_scout.join("amd64/lib/x86_64-linux-gnu")).unwrap();
        std::fs::create_dir_all(extra_scout.join("i386/lib/i386-linux-gnu")).unwrap();

        let paths = collect_linux_runtime_library_paths(
            &[steam_main.clone(), steam_extra.clone()],
            &[lsu.clone()],
            &[compat_linux.clone()],
        );

        let expected = vec![
            lsu.join("lib64"),
            lsu.join("lib32"),
            scout.join("pinned_libs_64"),
            scout.join("pinned_libs_32"),
            compat_linux.join("lib64"),
            compat_linux.join("usr/lib64"),
            compat_linux.join("usr/lib"),
            scout.join("lib/x86_64-linux-gnu"),
            scout.join("usr/lib/x86_64-linux-gnu"),
            scout.join("lib/i386-linux-gnu"),
            scout.join("lib"),
            scout.join("usr/lib"),
            extra_scout.join("amd64/lib/x86_64-linux-gnu"),
            extra_scout.join("amd64/lib"),
            extra_scout.join("i386/lib/i386-linux-gnu"),
            extra_scout.join("i386/lib"),
            soldier.join("soldier_platform_2.0.1000/files/lib/x86_64-linux-gnu"),
            soldier.join("soldier_platform_2.0.1000/files/lib/i386-linux-gnu"),
            soldier.join("soldier_platform_2.0.1000/files/lib"),
            sniper.join("sniper_platform_3.0.1/files/lib/x86_64-linux-gnu"),
            sniper.join("sniper_platform_3.0.1/files/lib"),
        ];
        assert_eq!(paths, expected);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
