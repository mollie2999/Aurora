//! Steam-compatible server browser Favorites & History persistence
//! (`userdata/<account_id>/7/remote/serverbrowser_hist.vdf`).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};

const MAX_HISTORY: usize = 200;

/// A saved server entry in Favorites or History.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedServer {
    pub addr: String,
    pub name: String,
    pub app_id: u32,
    pub game_dir: String,
    pub last_played: u64,
}

/// Persisted Favorite & History server lists matching Steam's `serverbrowser_hist.vdf`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerBrowserLists {
    pub favorites: Vec<SavedServer>,
    pub history: Vec<SavedServer>,
}

/// Path to Steam's server browser VDF for `account_id`.
pub fn history_path(steam_root: &Path, account_id: u32) -> PathBuf {
    steam_root
        .join("userdata")
        .join(account_id.to_string())
        .join("7")
        .join("remote")
        .join("serverbrowser_hist.vdf")
}

/// Parse an `"ip:port"` IPv4 string into `(ip_u32, port)`.
pub fn parse_ipv4_port(addr: &str) -> Option<(u32, u16)> {
    let v4: SocketAddrV4 = addr.trim().parse().ok()?;
    Some((u32::from(*v4.ip()), v4.port()))
}

/// Format `(ip_u32, port)` into `"a.b.c.d:port"`.
pub fn format_ipv4_port(ip: u32, port: u16) -> String {
    format!("{}:{port}", Ipv4Addr::from(ip))
}

fn parse_entry_list(obj: &vdf::Obj) -> Vec<SavedServer> {
    let mut numbered: Vec<(usize, SavedServer)> = Vec::new();
    for (idx, (key, val)) in obj.iter().enumerate() {
        let Some(entry) = val.as_obj() else { continue };
        let Some(addr) = entry.str("address").map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let order = key.trim().parse::<usize>().unwrap_or(usize::MAX / 2 + idx);
        numbered.push((
            order,
            SavedServer {
                addr: addr.to_string(),
                name: entry.str("name").unwrap_or_default().to_string(),
                app_id: entry.u64("appid").and_then(|v| u32::try_from(v).ok()).unwrap_or(0),
                game_dir: entry.str("gamedir").unwrap_or_default().to_string(),
                last_played: entry.u64("LastPlayed").unwrap_or(0),
            },
        ));
    }
    numbered.sort_by_key(|(order, _)| *order);
    numbered.into_iter().map(|(_, s)| s).collect()
}

/// Parse a `serverbrowser_hist.vdf` text document.
pub fn parse_vdf(text: &str) -> std::result::Result<ServerBrowserLists, vdf::Error> {
    if text.trim().is_empty() {
        return Ok(ServerBrowserLists::default());
    }
    let root = vdf::parse(text)?;
    let filters = root.obj("Filters").unwrap_or(&root);
    let favorites = filters.obj("Favorites").map(parse_entry_list).unwrap_or_default();
    let history = filters.obj("History").map(parse_entry_list).unwrap_or_default();
    Ok(ServerBrowserLists { favorites, history })
}

fn encode_entry_list(items: &[SavedServer]) -> vdf::Obj {
    let mut obj = vdf::Obj::default();
    for (i, s) in items.iter().enumerate() {
        let entry = vdf::Obj::default()
            .with("name", &s.name)
            .with("address", &s.addr)
            .with("LastPlayed", s.last_played.to_string())
            .with("appid", s.app_id.to_string())
            .with("gamedir", &s.game_dir);
        obj = obj.with_obj(&i.to_string(), entry);
    }
    obj
}

/// Serialize `lists` back into a `serverbrowser_hist.vdf` string, preserving any
/// sibling keys inside `"Filters"` from `existing_text`.
pub fn write_vdf(lists: &ServerBrowserLists, existing_text: Option<&str>) -> String {
    let mut existing_root = existing_text
        .and_then(|t| vdf::parse(t).ok())
        .unwrap_or_default();
    let mut filters = existing_root.obj("Filters").cloned().unwrap_or_default();
    filters
        .0
        .retain(|(k, _)| !k.eq_ignore_ascii_case("Favorites") && !k.eq_ignore_ascii_case("History"));
    filters = filters
        .with_obj("Favorites", encode_entry_list(&lists.favorites))
        .with_obj("History", encode_entry_list(&lists.history));

    existing_root
        .0
        .retain(|(k, _)| !k.eq_ignore_ascii_case("Filters"));
    let root = existing_root.with_obj("Filters", filters);
    vdf::write(&root)
}

fn no_symlink(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(m) => !m.file_type().is_symlink(),
        Err(_) => true,
    }
}

/// Load `serverbrowser_hist.vdf` if present and non-symlinked.
pub fn load_lists(steam_root: &Path, account_id: u32) -> ServerBrowserLists {
    let path = history_path(steam_root, account_id);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return ServerBrowserLists::default();
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return ServerBrowserLists::default();
    }
    let Ok(text) = fs::read_to_string(&path) else {
        return ServerBrowserLists::default();
    };
    parse_vdf(&text).unwrap_or_default()
}

/// Persist `serverbrowser_hist.vdf` atomically, refusing symlink targets.
pub fn save_lists(steam_root: &Path, account_id: u32, lists: &ServerBrowserLists) -> std::io::Result<()> {
    let path = history_path(steam_root, account_id);
    if !no_symlink(&path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing symlinked serverbrowser_hist.vdf",
        ));
    }
    let Some(dir) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid serverbrowser_hist.vdf path",
        ));
    };
    fs::create_dir_all(dir)?;
    let existing = fs::read_to_string(&path).ok();
    let content = write_vdf(lists, existing.as_deref());

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        ".serverbrowser_hist-{}-{nanos}.tmp",
        std::process::id()
    ));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write_res = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Insert or update a server in Favorites, avoiding duplicate addresses.
pub fn add_favorite(steam_root: &Path, account_id: u32, server: SavedServer) -> std::io::Result<ServerBrowserLists> {
    let mut lists = load_lists(steam_root, account_id);
    if let Some(existing) = lists.favorites.iter_mut().find(|s| s.addr == server.addr) {
        if !server.name.is_empty() {
            existing.name = server.name;
        }
        if server.app_id != 0 {
            existing.app_id = server.app_id;
        }
        if !server.game_dir.is_empty() {
            existing.game_dir = server.game_dir;
        }
        if server.last_played > 0 {
            existing.last_played = server.last_played;
        }
    } else {
        lists.favorites.push(server);
    }
    save_lists(steam_root, account_id, &lists)?;
    Ok(lists)
}

/// Remove a favorite matching `addr`.
pub fn remove_favorite(steam_root: &Path, account_id: u32, addr: &str) -> std::io::Result<ServerBrowserLists> {
    let mut lists = load_lists(steam_root, account_id);
    lists.favorites.retain(|s| s.addr != addr);
    save_lists(steam_root, account_id, &lists)?;
    Ok(lists)
}

/// Prepend or update a server in History (capped at 200 newest-first).
pub fn record_history(steam_root: &Path, account_id: u32, mut server: SavedServer) -> std::io::Result<ServerBrowserLists> {
    let mut lists = load_lists(steam_root, account_id);
    if let Some(prev) = lists.history.iter().find(|s| s.addr == server.addr).cloned() {
        if server.name.is_empty() {
            server.name = prev.name;
        }
        if server.app_id == 0 {
            server.app_id = prev.app_id;
        }
        if server.game_dir.is_empty() {
            server.game_dir = prev.game_dir;
        }
    }
    lists.history.retain(|s| s.addr != server.addr);
    lists.history.insert(0, server);
    lists.history.truncate(MAX_HISTORY);
    save_lists(steam_root, account_id, &lists)?;
    Ok(lists)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_round_trip_case_insensitive_and_preserves_siblings() {
        let input = r#""Filters"
{
	"CustomKey"		"keep-me"
	"favorites"
	{
		"1"
		{
			"name"		"Second"
			"address"		"198.51.100.2:27016"
			"LastPlayed"		"100"
			"appid"		"730"
			"gamedir"		"csgo"
		}
		"0"
		{
			"name"		"First"
			"address"		"198.51.100.1:27015"
			"LastPlayed"		"200"
			"appid"		"440"
			"gamedir"		"tf"
		}
	}
	"history"
	{
		"0"
		{
			"name"		"Recent"
			"address"		"203.0.113.5:27015"
			"LastPlayed"		"300"
			"appid"		"730"
			"gamedir"		"csgo"
		}
	}
}
"#;
        let parsed = parse_vdf(input).unwrap();
        assert_eq!(parsed.favorites.len(), 2);
        assert_eq!(parsed.favorites[0].addr, "198.51.100.1:27015");
        assert_eq!(parsed.favorites[1].addr, "198.51.100.2:27016");
        assert_eq!(parsed.history.len(), 1);

        let serialized = write_vdf(&parsed, Some(input));
        assert!(serialized.contains("\"CustomKey\"\t\t\"keep-me\""));
        let reparsed = parse_vdf(&serialized).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn add_remove_and_history_cap_work() {
        let dir = std::env::temp_dir().join(format!("sir-servers-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        add_favorite(
            &dir,
            42,
            SavedServer {
                addr: "198.51.100.10:27015".into(),
                name: "Test Fav".into(),
                app_id: 730,
                game_dir: "csgo".into(),
                last_played: 10,
            },
        )
        .unwrap();
        add_favorite(
            &dir,
            42,
            SavedServer {
                addr: "198.51.100.10:27015".into(),
                name: "Updated Fav".into(),
                app_id: 730,
                game_dir: "csgo".into(),
                last_played: 20,
            },
        )
        .unwrap();
        let after = load_lists(&dir, 42);
        assert_eq!(after.favorites.len(), 1);
        assert_eq!(after.favorites[0].name, "Updated Fav");

        remove_favorite(&dir, 42, "198.51.100.10:27015").unwrap();
        assert!(load_lists(&dir, 42).favorites.is_empty());

        for i in 0..210u32 {
            record_history(
                &dir,
                42,
                SavedServer {
                    addr: format!("10.0.0.{}:{}", i / 10, 27000 + i),
                    name: format!("S{i}"),
                    app_id: 730,
                    game_dir: "csgo".into(),
                    last_played: u64::from(i),
                },
            )
            .unwrap();
        }
        let hist = load_lists(&dir, 42).history;
        assert_eq!(hist.len(), MAX_HISTORY);
        assert_eq!(hist[0].name, "S209");

        let (ip, port) = parse_ipv4_port("192.0.2.15:27015").unwrap();
        assert_eq!(format_ipv4_port(ip, port), "192.0.2.15:27015");

        let _ = fs::remove_dir_all(&dir);
    }
}
