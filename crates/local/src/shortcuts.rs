//! Non-Steam games added in the official client ("Add a Non-Steam Game"),
//! from `userdata/<account>/config/shortcuts.vdf` (binary KeyValues).

use std::path::{Path, PathBuf};

/// One non-Steam game from Steam's list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shortcut {
    pub name: String,
    /// The program, without Steam's quotes.
    pub exe: String,
    pub start_dir: String,
    pub launch_options: String,
    pub icon: String,
    pub hidden: bool,
    /// Steam's id for it (a 32-bit app id of its own).
    pub app_id: u32,
}

impl Shortcut {
    /// The name to show: Steam names games added from a file after the
    /// file (`Game.exe`), so the extension goes.
    pub fn display_name(&self) -> String {
        let n = self.name.trim();
        let n = if n.is_empty() { Path::new(&self.exe).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default() } else { n.to_string() };
        match n.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && ["exe", "bat", "cmd", "lnk", "sh", "appimage", "x86_64"].contains(&ext.to_lowercase().as_str()) => stem.to_string(),
            _ => n,
        }
    }
}

/// Steam keeps paths in double quotes.
fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Parse a `shortcuts.vdf`.
pub fn parse(data: &[u8]) -> Vec<Shortcut> {
    let Ok(root) = vdf::binary::parse(data) else { return Vec::new() };
    let Some(list) = root.obj("shortcuts") else { return Vec::new() };
    list.iter()
        .filter_map(|(_, v)| v.as_obj())
        .map(|o| Shortcut {
            // Steam has written both spellings over the years.
            name: o.str("AppName").or_else(|| o.str("appname")).unwrap_or_default().trim().to_string(),
            exe: unquote(o.str("Exe").or_else(|| o.str("exe")).unwrap_or_default()),
            start_dir: unquote(o.str("StartDir").unwrap_or_default()),
            launch_options: o.str("LaunchOptions").unwrap_or_default().trim().to_string(),
            icon: unquote(o.str("icon").unwrap_or_default()),
            hidden: o.str("IsHidden").is_some_and(|h| h != "0"),
            app_id: o.str("appid").and_then(|a| a.parse::<i64>().ok()).map(|a| a as u32).unwrap_or(0),
        })
        .filter(|s| !s.exe.is_empty())
        .collect()
}

/// The `shortcuts.vdf` files of a Steam install: this account's first,
/// else every account's on this PC.
pub fn files(steam_root: &Path, account_id: Option<u32>) -> Vec<PathBuf> {
    let userdata = steam_root.join("userdata");
    if let Some(id) = account_id {
        let f = userdata.join(id.to_string()).join("config").join("shortcuts.vdf");
        if f.is_file() {
            return vec![f];
        }
    }
    let mut out: Vec<PathBuf> = std::fs::read_dir(&userdata)
        .map(|d| d.flatten().map(|e| e.path().join("config").join("shortcuts.vdf")).filter(|f| f.is_file()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// Every non-Steam game in a Steam install (see [`files`]), without
/// duplicates (same program and options).
pub fn load(steam_root: &Path, account_id: Option<u32>) -> Vec<Shortcut> {
    let mut out: Vec<Shortcut> = Vec::new();
    for f in files(steam_root, account_id) {
        let Ok(data) = std::fs::read(&f) else { continue };
        for s in parse(&data) {
            if !out.iter().any(|o| o.exe == s.exe && o.launch_options == s.launch_options) {
                out.push(s);
            }
        }
    }
    out
}

/// Look up Steam's non-Steam shortcut app id for `exe` (if registered in
/// `shortcuts.vdf` with a non-zero id), so Aurora can reuse Steam's Proton prefix.
pub fn app_id_for_exe(steam_root: &Path, account_id: Option<u32>, exe: &Path) -> Option<u32> {
    if steam_root.as_os_str().is_empty() {
        return None;
    }
    load(steam_root, account_id)
        .into_iter()
        .find(|s| s.app_id != 0 && Path::new(&s.exe) == exe)
        .map(|s| s.app_id)
}

#[cfg(test)]
mod tests {
    /// A `shortcuts.vdf` the way Steam writes it.
    fn sample() -> Vec<u8> {
        let mut v = Vec::new();
        let obj = |v: &mut Vec<u8>, k: &str| {
            v.push(0x00);
            v.extend_from_slice(k.as_bytes());
            v.push(0);
        };
        let s = |v: &mut Vec<u8>, k: &str, val: &str| {
            v.push(0x01);
            v.extend_from_slice(k.as_bytes());
            v.push(0);
            v.extend_from_slice(val.as_bytes());
            v.push(0);
        };
        let i = |v: &mut Vec<u8>, k: &str, val: i32| {
            v.push(0x02);
            v.extend_from_slice(k.as_bytes());
            v.push(0);
            v.extend_from_slice(&val.to_le_bytes());
        };
        obj(&mut v, "shortcuts");
        obj(&mut v, "0");
        i(&mut v, "appid", -1_234_567_890);
        s(&mut v, "AppName", "Heroic Game");
        s(&mut v, "Exe", "\"/opt/heroic/game.exe\"");
        s(&mut v, "StartDir", "\"/opt/heroic/\"");
        s(&mut v, "icon", "");
        s(&mut v, "LaunchOptions", "-windowed");
        i(&mut v, "IsHidden", 0);
        obj(&mut v, "tags");
        v.push(0x08);
        v.push(0x08);
        obj(&mut v, "1");
        s(&mut v, "appname", "Old Style");
        s(&mut v, "exe", "\"C:\\Games\\old.exe\"");
        i(&mut v, "IsHidden", 1);
        v.push(0x08);
        obj(&mut v, "2");
        s(&mut v, "AppName", "No program");
        v.push(0x08);
        v.push(0x08);
        v.push(0x08);
        v
    }

    #[test]
    fn reads_steams_shortcuts() {
        let list = super::parse(&sample());
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Heroic Game");
        assert_eq!(list[0].exe, "/opt/heroic/game.exe");
        assert_eq!(list[0].start_dir, "/opt/heroic/");
        assert_eq!(list[0].launch_options, "-windowed");
        assert_eq!(list[0].app_id, (-1_234_567_890i32) as u32);
        assert!(!list[0].hidden);
        assert_eq!(list[1].name, "Old Style");
        assert_eq!(list[1].exe, "C:\\Games\\old.exe");
        assert!(list[1].hidden);
    }

    /// This PC's real Steam list: `cargo test -p local real_shortcuts -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_shortcuts() {
        let root = crate::find_steam_root().expect("Steam installed");
        let list = super::load(&root, None);
        println!("{} non-Steam games in Steam", list.len());
        for s in &list {
            assert!(!s.exe.is_empty() && !s.exe.starts_with('"'), "{s:?}");
        }
    }

    #[test]
    fn display_names_drop_program_extensions() {
        let s = |name: &str, exe: &str| super::Shortcut { name: name.into(), exe: exe.into(), ..Default::default() };
        assert_eq!(s("KSP_x64.exe", "/g/KSP_x64.exe").display_name(), "KSP_x64");
        assert_eq!(s("Heroic Game", "/g/x").display_name(), "Heroic Game");
        assert_eq!(s("", "/g/Run.sh").display_name(), "Run");
        assert_eq!(s("v1.2", "/g/x").display_name(), "v1.2");
    }

    #[test]
    fn broken_files_give_nothing() {
        assert!(super::parse(b"\x00shortcuts\x00\x01").is_empty());
        assert!(super::parse(b"").is_empty());
    }

    #[test]
    fn finds_app_id_for_exe() {
        let root = std::env::temp_dir().join(format!("sir-shortcuts-appid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = root.join("userdata").join("99").join("config");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("shortcuts.vdf"), sample()).unwrap();

        let expected = (-1_234_567_890i32) as u32;
        assert_eq!(super::app_id_for_exe(&root, Some(99), std::path::Path::new("/opt/heroic/game.exe")), Some(expected));
        assert_eq!(super::app_id_for_exe(&root, None, std::path::Path::new("/opt/heroic/game.exe")), Some(expected));
        // Second entry has no appid (0), so it returns None.
        assert_eq!(super::app_id_for_exe(&root, Some(99), std::path::Path::new("C:\\Games\\old.exe")), None);
        assert_eq!(super::app_id_for_exe(&root, Some(99), std::path::Path::new("/missing.exe")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
