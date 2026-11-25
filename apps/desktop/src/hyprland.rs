//! Minimal Hyprland IPC helpers for locating game/overlay windows and
//! moving/focusing Aurora's fallback overlay across workspaces.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

const IPC_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_IPC_REPLY_BYTES: usize = 2 * 1024 * 1024;

/// Currently focused Hyprland window metadata from `j/activewindow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveWindow {
    pub address: String,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub workspace_id: i64,
    pub workspace_name: String,
}

/// Hyprland client metadata from `j/clients`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyprWindow {
    pub address: String,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub workspace_id: i64,
    pub workspace_name: String,
    pub mapped: bool,
    pub hidden: bool,
    pub floating: bool,
    pub at: [i32; 2],
    pub size: [i32; 2],
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWorkspaceRef {
    id: i64,
    #[serde(default)]
    name: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWindow {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    pid: Option<i64>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    class: String,
    #[serde(default)]
    workspace: Option<RawWorkspaceRef>,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    floating: bool,
    #[serde(default)]
    at: [i32; 2],
    #[serde(default)]
    size: [i32; 2],
}

impl RawWindow {
    fn into_hypr(self) -> Option<HyprWindow> {
        let address = self.address?;
        if !is_valid_address(&address) {
            return None;
        }
        let pid = self.pid.and_then(|p| u32::try_from(p).ok()).unwrap_or(0);
        let (workspace_id, workspace_name) = match self.workspace {
            Some(ws) => (ws.id, ws.name),
            None => (0, String::new()),
        };
        Some(HyprWindow {
            address,
            pid,
            title: self.title,
            class: self.class,
            workspace_id,
            workspace_name,
            mapped: self.mapped,
            hidden: self.hidden,
            floating: self.floating,
            at: self.at,
            size: self.size,
        })
    }
}

/// Validate a Hyprland window address (`0x` followed by 1..=32 hex chars).
pub fn is_valid_address(addr: &str) -> bool {
    let Some(hex) = addr.strip_prefix("0x") else {
        return false;
    };
    !hex.is_empty() && hex.len() <= 32 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Validate a Hyprland workspace name used in IPC dispatch arguments.
pub fn is_valid_workspace_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name
            .bytes()
            .any(|b| matches!(b, b',' | b';' | b'\n' | b'\r' | b'\0'))
}

fn socket_path_from(runtime_dir: Option<&OsStr>, instance_sig: Option<&OsStr>) -> Option<PathBuf> {
    let runtime = runtime_dir?;
    if runtime.is_empty() {
        return None;
    }
    let sig = instance_sig?.to_str()?;
    if sig.is_empty()
        || sig.contains('/')
        || sig.contains('\\')
        || sig.contains("..")
        || sig.contains('\0')
    {
        return None;
    }
    Some(PathBuf::from(runtime).join("hypr").join(sig).join(".socket.sock"))
}

/// Resolve the active Hyprland command socket path, if running under Hyprland.
pub fn socket_path() -> Option<PathBuf> {
    socket_path_from(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
    )
}

/// Return true if a Hyprland IPC socket is configured and present on disk.
pub fn is_available() -> bool {
    socket_path().is_some_and(|p| p.exists())
}

fn ipc_query_at(path: &Path, command: &str) -> Result<String, String> {
    let mut stream =
        UnixStream::connect(path).map_err(|e| format!("hyprland ipc connect failed: {e}"))?;
    let _ = stream.set_read_timeout(Some(IPC_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IPC_TIMEOUT));
    stream
        .write_all(command.as_bytes())
        .map_err(|e| format!("hyprland ipc write failed: {e}"))?;
    let _ = stream.shutdown(Shutdown::Write);

    let mut buf = Vec::new();
    stream
        .take((MAX_IPC_REPLY_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("hyprland ipc read failed: {e}"))?;
    if buf.len() > MAX_IPC_REPLY_BYTES {
        return Err("hyprland ipc reply exceeded limit".into());
    }
    String::from_utf8(buf).map_err(|e| format!("hyprland ipc invalid utf-8: {e}"))
}

fn ipc_query(command: &str) -> Result<String, String> {
    let path = socket_path().ok_or_else(|| "hyprland ipc socket unavailable".to_string())?;
    ipc_query_at(&path, command)
}

/// Parse Hyprland `j/activewindow` JSON into `ActiveWindow`.
pub fn parse_active_window(json: &str) -> Option<ActiveWindow> {
    let raw: RawWindow = serde_json::from_str(json).ok()?;
    let w = raw.into_hypr()?;
    if w.pid == 0 {
        return None;
    }
    Some(ActiveWindow {
        address: w.address,
        pid: w.pid,
        title: w.title,
        class: w.class,
        workspace_id: w.workspace_id,
        workspace_name: w.workspace_name,
    })
}

/// Parse Hyprland `j/clients` JSON into mapped/raw window entries.
pub fn parse_clients(json: &str) -> Vec<HyprWindow> {
    let raw: Vec<RawWindow> = match serde_json::from_str(json) {
        Ok(list) => list,
        Err(_) => return Vec::new(),
    };
    raw.into_iter().filter_map(RawWindow::into_hypr).collect()
}

/// Query Hyprland for the currently focused window.
pub fn active_window() -> Option<ActiveWindow> {
    let json = ipc_query("j/activewindow").ok()?;
    parse_active_window(&json)
}

/// Query Hyprland for all client windows.
pub fn clients() -> Vec<HyprWindow> {
    match ipc_query("j/clients") {
        Ok(json) => parse_clients(&json),
        Err(_) => Vec::new(),
    }
}

fn window_area(w: &HyprWindow) -> i64 {
    i64::from(w.size[0].max(0)) * i64::from(w.size[1].max(0))
}

/// Select the best matching window from a list by PID set and optional title.
pub fn select_window_by_pid_title<'a>(
    windows: &'a [HyprWindow],
    pids: &[u32],
    title: &str,
) -> Option<&'a HyprWindow> {
    if !pids.is_empty() {
        if let Some(best) = windows
            .iter()
            .filter(|w| w.mapped && !w.hidden && w.pid != 0 && pids.contains(&w.pid))
            .max_by_key(|w| window_area(w))
        {
            return Some(best);
        }
    }
    if !title.is_empty() {
        if let Some(best) = windows
            .iter()
            .filter(|w| w.mapped && !w.hidden && w.title == title)
            .max_by_key(|w| window_area(w))
        {
            return Some(best);
        }
    }
    None
}

/// Find a game/client window matching any candidate PID, falling back to exact title.
pub fn find_window_by_pid_title(pids: &[u32], title: &str) -> Option<HyprWindow> {
    let all = clients();
    select_window_by_pid_title(&all, pids, title).cloned()
}

/// Select Aurora's overlay viewport window belonging to `our_pid`.
pub fn select_overlay_window<'a>(
    windows: &'a [HyprWindow],
    our_pid: u32,
    overlay_title: &str,
) -> Option<&'a HyprWindow> {
    windows
        .iter()
        .filter(|w| w.pid == our_pid && w.title == overlay_title)
        .max_by_key(|w| (w.mapped as u8, !w.hidden as u8, window_area(w)))
}

/// Find Aurora's overlay window managed by Hyprland.
pub fn find_overlay_window(our_pid: u32, overlay_title: &str) -> Option<HyprWindow> {
    let all = clients();
    select_overlay_window(&all, our_pid, overlay_title).cloned()
}

/// Build the target workspace selector passed to Hyprland dispatchers.
pub fn workspace_selector(ws_id: i64, ws_name: &str) -> Option<String> {
    if ws_id > 0 {
        Some(ws_id.to_string())
    } else if ws_id < 0 && ws_name.starts_with("special:") && is_valid_workspace_name(ws_name) {
        Some(ws_name.to_string())
    } else if is_valid_workspace_name(ws_name) {
        Some(format!("name:{ws_name}"))
    } else {
        None
    }
}

/// Build the `[[BATCH]]` dispatch command that places and focuses `overlay_addr`.
pub fn build_place_overlay_batch(
    overlay_addr: &str,
    ws_id: i64,
    ws_name: &str,
    monitor_geom: Option<[i32; 4]>,
) -> Result<String, String> {
    if !is_valid_address(overlay_addr) {
        return Err("invalid hyprland overlay window address".into());
    }
    let ws = workspace_selector(ws_id, ws_name)
        .ok_or_else(|| "invalid hyprland target workspace".to_string())?;
    let mut cmds = vec![format!(
        "dispatch movetoworkspacesilent {ws},address:{overlay_addr}"
    )];
    if let Some([x, y, w, h]) = monitor_geom {
        if w > 0 && h > 0 {
            cmds.push(format!("dispatch setfloating address:{overlay_addr}"));
            cmds.push(format!(
                "dispatch resizewindowpixel exact {w} {h},address:{overlay_addr}"
            ));
            cmds.push(format!(
                "dispatch movewindowpixel exact {x} {y},address:{overlay_addr}"
            ));
        }
    }
    cmds.push(format!("dispatch focuswindow address:{overlay_addr}"));
    Ok(format!("[[BATCH]]{}", cmds.join(";")))
}

fn check_dispatch_ok(reply: &str) -> Result<(), String> {
    let trimmed = reply.trim();
    if trimmed == "ok"
        || trimmed
            .lines()
            .all(|line| line.trim().eq_ignore_ascii_case("ok"))
    {
        Ok(())
    } else if trimmed.is_empty() {
        Ok(())
    } else {
        Err(format!("hyprland dispatch error: {trimmed}"))
    }
}

/// Move Aurora's overlay window to the target workspace without switching workspaces
/// away unnecessarily, optionally apply floating geometry, and focus it.
pub fn place_overlay_on_workspace(
    overlay_addr: &str,
    ws_id: i64,
    ws_name: &str,
    monitor_geom: Option<[i32; 4]>,
) -> Result<(), String> {
    let batch = build_place_overlay_batch(overlay_addr, ws_id, ws_name, monitor_geom)?;
    let reply = ipc_query(&batch)?;
    check_dispatch_ok(&reply)
}

/// Return keyboard/window focus to the given Hyprland window address.
pub fn focus_window(addr: &str) -> Result<(), String> {
    if !is_valid_address(addr) {
        return Err("invalid hyprland window address".into());
    }
    let reply = ipc_query(&format!("dispatch focuswindow address:{addr}"))?;
    check_dispatch_ok(&reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn socket_path_rejects_missing_or_unsafe_env() {
        assert_eq!(socket_path_from(None, Some(OsStr::new("sig"))), None);
        assert_eq!(
            socket_path_from(Some(OsStr::new("")), Some(OsStr::new("sig"))),
            None
        );
        assert_eq!(
            socket_path_from(Some(OsStr::new("/run/user/1000")), None),
            None
        );
        assert_eq!(
            socket_path_from(Some(OsStr::new("/run/user/1000")), Some(OsStr::new(""))),
            None
        );
        assert_eq!(
            socket_path_from(
                Some(OsStr::new("/run/user/1000")),
                Some(OsStr::new("../evil"))
            ),
            None
        );
        assert_eq!(
            socket_path_from(
                Some(OsStr::new("/run/user/1000")),
                Some(OsStr::new("a/b"))
            ),
            None
        );
        assert_eq!(
            socket_path_from(
                Some(OsStr::new("/run/user/1000")),
                Some(OsStr::new("abc123_def"))
            ),
            Some(PathBuf::from("/run/user/1000/hypr/abc123_def/.socket.sock"))
        );
    }

    #[test]
    fn valid_address_checks_hex() {
        assert!(is_valid_address("0x55a1b2c3"));
        assert!(is_valid_address("0xabcdef0123456789"));
        assert!(!is_valid_address("55a1b2c3"));
        assert!(!is_valid_address("0x"));
        assert!(!is_valid_address("0x1234;dispatch"));
        assert!(!is_valid_address("0x1234,foo"));
    }

    #[test]
    fn parse_active_window_handles_empty_and_valid() {
        assert_eq!(parse_active_window("{}"), None);
        assert_eq!(
            parse_active_window(r#"{"address":"","pid":-1,"title":""}"#),
            None
        );
        let sample = r#"{
            "address": "0x55e12340",
            "pid": 4242,
            "title": "Hades II",
            "class": "steam_app_1145350",
            "workspace": { "id": 3, "name": "3" }
        }"#;
        let aw = parse_active_window(sample).expect("valid active window");
        assert_eq!(aw.address, "0x55e12340");
        assert_eq!(aw.pid, 4242);
        assert_eq!(aw.title, "Hades II");
        assert_eq!(aw.workspace_id, 3);
        assert_eq!(aw.workspace_name, "3");
    }

    #[test]
    fn select_window_matches_pid_then_title() {
        let json = r#"[
            {
                "address": "0x10",
                "pid": 100,
                "title": "Aurora",
                "class": "aurora",
                "workspace": { "id": 1, "name": "1" },
                "mapped": true,
                "hidden": false,
                "floating": false,
                "at": [0, 0],
                "size": [1280, 800]
            },
            {
                "address": "0x20",
                "pid": 200,
                "title": "Launcher",
                "class": "game",
                "workspace": { "id": 2, "name": "2" },
                "mapped": true,
                "hidden": false,
                "floating": false,
                "at": [0, 0],
                "size": [400, 300]
            },
            {
                "address": "0x30",
                "pid": 200,
                "title": "My Game",
                "class": "game",
                "workspace": { "id": 2, "name": "2" },
                "mapped": true,
                "hidden": false,
                "floating": false,
                "at": [0, 0],
                "size": [1920, 1080]
            },
            {
                "address": "0x40",
                "pid": 100,
                "title": "Aurora Overlay",
                "class": "aurora",
                "workspace": { "id": 1, "name": "1" },
                "mapped": true,
                "hidden": false,
                "floating": true,
                "at": [0, 0],
                "size": [1920, 1080]
            }
        ]"#;
        let clients = parse_clients(json);
        let picked = select_window_by_pid_title(&clients, &[200], "").expect("match by pid");
        assert_eq!(picked.address, "0x30");
        let by_title =
            select_window_by_pid_title(&clients, &[999], "My Game").expect("fallback title");
        assert_eq!(by_title.address, "0x30");
        let ov = select_overlay_window(&clients, 100, "Aurora Overlay").expect("overlay window");
        assert_eq!(ov.address, "0x40");
    }

    #[test]
    fn batch_builder_rejects_bad_inputs_and_formats_commands() {
        assert!(build_place_overlay_batch("bad", 2, "2", None).is_err());
        assert!(build_place_overlay_batch("0x123", 0, "bad,name", None).is_err());

        let cmd = build_place_overlay_batch("0xabc123", 2, "2", None).unwrap();
        assert_eq!(
            cmd,
            "[[BATCH]]dispatch movetoworkspacesilent 2,address:0xabc123;dispatch focuswindow address:0xabc123"
        );

        let special =
            build_place_overlay_batch("0xabc123", -98, "special:game", Some([0, 0, 1920, 1080]))
                .unwrap();
        assert!(special.contains("movetoworkspacesilent special:game,address:0xabc123"));
        assert!(special.contains("resizewindowpixel exact 1920 1080,address:0xabc123"));
    }
}
