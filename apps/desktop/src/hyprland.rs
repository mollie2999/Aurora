//! Minimal Hyprland IPC helpers for locating game/overlay windows,
//! moving/raising/focusing Aurora's fallback overlay across workspaces, and
//! temporarily clearing/restoring a game's fullscreen layer when needed.

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
    pub monitor: i64,
    pub fullscreen: u8,
    pub fullscreen_client: u8,
    pub pinned: bool,
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
    pub monitor: i64,
    pub mapped: bool,
    pub hidden: bool,
    pub floating: bool,
    pub pinned: bool,
    pub fullscreen: u8,
    pub fullscreen_client: u8,
    pub at: [i32; 2],
    pub size: [i32; 2],
}

/// Hyprland monitor metadata from `j/monitors`.
#[derive(Debug, Clone, PartialEq)]
pub struct HyprMonitor {
    pub id: i64,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f32,
    pub transform: u8,
    pub active_workspace_id: i64,
}

impl HyprMonitor {
    /// Logical monitor rectangle `[x, y, width, height]` in Hyprland coordinates.
    pub fn logical_geom(&self) -> Option<[i32; 4]> {
        let scale = if self.scale.is_finite() && self.scale > 0.0 {
            self.scale
        } else {
            1.0
        };
        let (raw_w, raw_h) = if self.transform % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        if raw_w <= 0 || raw_h <= 0 {
            return None;
        }
        let w = (f64::from(raw_w) / f64::from(scale)).round() as i32;
        let h = (f64::from(raw_h) / f64::from(scale)).round() as i32;
        if w > 0 && h > 0 {
            Some([self.x, self.y, w, h])
        } else {
            None
        }
    }
}

/// Saved fullscreen state of a game window whose internal fullscreen layer was
/// temporarily cleared while the fallback overlay is visible above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullscreenRestore {
    pub address: String,
    pub internal: u8,
    pub client: u8,
}

/// Parameters for placing and raising Aurora's fallback overlay window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPlacement<'a> {
    pub overlay_addr: &'a str,
    pub overlay_floating: bool,
    pub overlay_fullscreen: u8,
    pub ws_id: i64,
    pub ws_name: &'a str,
    pub monitor_geom: Option<[i32; 4]>,
    /// `(address, client_fullscreen_mode)` of a fullscreen window on the target
    /// workspace whose internal fullscreen must be cleared so the overlay can
    /// render above it.
    pub fullscreen_blocker: Option<(&'a str, u8)>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWorkspaceRef {
    id: i64,
    #[serde(default)]
    name: String,
}

fn deserialize_fullscreen_mode<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ModeVisitor;
    impl<'de> serde::de::Visitor<'de> for ModeVisitor {
        type Value = u8;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a fullscreen integer (0..=3) or boolean")
        }

        fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
            Ok(if v { 2 } else { 0 })
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v.min(3) as u8)
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
            Ok(if v <= 0 { 0 } else { v.min(3) as u8 })
        }

        fn visit_none<E>(self) -> Result<Self::Value, E> {
            Ok(0)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(0)
        }
    }
    deserializer.deserialize_any(ModeVisitor)
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
    #[serde(default = "default_monitor_id")]
    monitor: i64,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    floating: bool,
    #[serde(default)]
    pinned: bool,
    #[serde(default, deserialize_with = "deserialize_fullscreen_mode")]
    fullscreen: u8,
    #[serde(default, deserialize_with = "deserialize_fullscreen_mode")]
    fullscreen_client: u8,
    #[serde(default)]
    at: [i32; 2],
    #[serde(default)]
    size: [i32; 2],
}

fn default_monitor_id() -> i64 {
    -1
}

fn default_scale() -> f32 {
    1.0
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMonitor {
    id: i64,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    width: i32,
    #[serde(default)]
    height: i32,
    #[serde(default = "default_scale")]
    scale: f32,
    #[serde(default)]
    transform: u8,
    #[serde(default)]
    active_workspace: Option<RawWorkspaceRef>,
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
            monitor: self.monitor,
            mapped: self.mapped,
            hidden: self.hidden,
            floating: self.floating,
            pinned: self.pinned,
            fullscreen: self.fullscreen,
            fullscreen_client: self.fullscreen_client,
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
            .any(|b| matches!(b, b',' | b';') || b.is_ascii_control())
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
        monitor: w.monitor,
        fullscreen: w.fullscreen,
        fullscreen_client: w.fullscreen_client,
        pinned: w.pinned,
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

/// Parse Hyprland `j/monitors` JSON into monitor entries.
pub fn parse_monitors(json: &str) -> Vec<HyprMonitor> {
    let raw: Vec<RawMonitor> = match serde_json::from_str(json) {
        Ok(list) => list,
        Err(_) => return Vec::new(),
    };
    raw.into_iter()
        .map(|m| HyprMonitor {
            id: m.id,
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
            scale: m.scale,
            transform: m.transform,
            active_workspace_id: m.active_workspace.map_or(0, |ws| ws.id),
        })
        .collect()
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

/// Query Hyprland for all connected monitors.
pub fn monitors() -> Vec<HyprMonitor> {
    match ipc_query("j/monitors") {
        Ok(json) => parse_monitors(&json),
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

/// Find any fullscreen window on `ws_id` (preferring `game_addr` if present)
/// that currently occupies the workspace's top fullscreen layer.
pub fn find_fullscreen_blocker<'a>(
    windows: &'a [HyprWindow],
    overlay_addr: &str,
    ws_id: i64,
    game_addr: Option<&str>,
) -> Option<&'a HyprWindow> {
    if let Some(addr) = game_addr {
        if let Some(game) = windows
            .iter()
            .find(|w| w.address == addr && w.address != overlay_addr && w.fullscreen != 0)
        {
            return Some(game);
        }
    }
    windows.iter().find(|w| {
        w.address != overlay_addr
            && w.workspace_id == ws_id
            && w.mapped
            && !w.hidden
            && w.fullscreen != 0
    })
}

/// Select the logical `[x, y, width, height]` geometry for the target monitor.
pub fn select_monitor_geom(
    monitors: &[HyprMonitor],
    game: Option<&HyprWindow>,
    ws_id: i64,
) -> Option<[i32; 4]> {
    if let Some(g) = game {
        if g.monitor >= 0 {
            if let Some(geom) = monitors
                .iter()
                .find(|m| m.id == g.monitor)
                .and_then(HyprMonitor::logical_geom)
            {
                return Some(geom);
            }
        }
    }
    if let Some(geom) = monitors
        .iter()
        .find(|m| m.active_workspace_id == ws_id)
        .and_then(HyprMonitor::logical_geom)
    {
        return Some(geom);
    }
    if let Some(g) = game {
        if g.fullscreen != 0 && g.size[0] > 0 && g.size[1] > 0 {
            return Some([g.at[0], g.at[1], g.size[0], g.size[1]]);
        }
    }
    None
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

/// Build the `[[BATCH]]` dispatch command that clears any blocking fullscreen
/// layer on the target workspace (preserving the game's client-side state),
/// moves `overlay_addr` silently to the workspace, ensures it floats at
/// `monitor_geom`, focuses it, and raises it to the top of the z-order.
pub fn build_place_overlay_batch(p: &OverlayPlacement<'_>) -> Result<String, String> {
    if !is_valid_address(p.overlay_addr) {
        return Err("invalid hyprland overlay window address".into());
    }
    let ws = workspace_selector(p.ws_id, p.ws_name)
        .ok_or_else(|| "invalid hyprland target workspace".to_string())?;
    let mut cmds = Vec::new();
    if let Some((blocker_addr, client_fs)) = p.fullscreen_blocker {
        if !is_valid_address(blocker_addr) {
            return Err("invalid hyprland fullscreen blocker address".into());
        }
        if client_fs > 3 {
            return Err("invalid hyprland fullscreen mode".into());
        }
        cmds.push(format!("dispatch focuswindow address:{blocker_addr}"));
        cmds.push(format!("dispatch fullscreenstate 0 {client_fs} set"));
    }
    cmds.push(format!(
        "dispatch movetoworkspacesilent {ws},address:{}",
        p.overlay_addr
    ));
    if p.overlay_fullscreen != 0 {
        cmds.push(format!("dispatch focuswindow address:{}", p.overlay_addr));
        cmds.push("dispatch fullscreenstate 0 0 set".to_string());
    }
    if !p.overlay_floating {
        cmds.push(format!(
            "dispatch togglefloating address:{}",
            p.overlay_addr
        ));
    }
    if let Some([x, y, w, h]) = p.monitor_geom {
        if w > 0 && h > 0 {
            cmds.push(format!(
                "dispatch resizewindowpixel exact {w} {h},address:{}",
                p.overlay_addr
            ));
            cmds.push(format!(
                "dispatch movewindowpixel exact {x} {y},address:{}",
                p.overlay_addr
            ));
        }
    }
    cmds.push(format!("dispatch focuswindow address:{}", p.overlay_addr));
    cmds.push(format!(
        "dispatch alterzorder top,address:{}",
        p.overlay_addr
    ));
    Ok(format!("[[BATCH]]{}", cmds.join(";")))
}

/// Build the IPC command that returns focus to `game_addr` and optionally
/// restores its Hyprland `fullscreenstate` `(internal, client)`.
pub fn build_restore_window_batch(
    game_addr: &str,
    fullscreen: Option<(u8, u8)>,
) -> Result<String, String> {
    if !is_valid_address(game_addr) {
        return Err("invalid hyprland window address".into());
    }
    if let Some((internal, client)) = fullscreen {
        if internal > 3 || client > 3 {
            return Err("invalid hyprland fullscreen mode".into());
        }
        Ok(format!(
            "[[BATCH]]dispatch focuswindow address:{game_addr};dispatch fullscreenstate {internal} {client} set"
        ))
    } else {
        Ok(format!("dispatch focuswindow address:{game_addr}"))
    }
}

/// Build the IPC command that focuses `game_addr` while keeping `overlay_addr`
/// at the top of the floating z-order (e.g. for click-through pinned windows).
pub fn build_focus_game_keeping_overlay_top_batch(
    game_addr: &str,
    overlay_addr: Option<&str>,
) -> Result<String, String> {
    if !is_valid_address(game_addr) {
        return Err("invalid hyprland window address".into());
    }
    if let Some(ov_addr) = overlay_addr {
        if !is_valid_address(ov_addr) {
            return Err("invalid hyprland overlay window address".into());
        }
        Ok(format!(
            "[[BATCH]]dispatch focuswindow address:{game_addr};dispatch alterzorder top,address:{ov_addr}"
        ))
    } else {
        Ok(format!("dispatch focuswindow address:{game_addr}"))
    }
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

/// Place Aurora's overlay window on the target workspace above the game.
pub fn place_overlay_on_workspace(p: &OverlayPlacement<'_>) -> Result<(), String> {
    let batch = build_place_overlay_batch(p)?;
    let reply = ipc_query(&batch)?;
    check_dispatch_ok(&reply)
}

/// Return keyboard/window focus to the given Hyprland window address and
/// optionally restore its `fullscreenstate`.
pub fn restore_game_window(
    addr: &str,
    fullscreen: Option<(u8, u8)>,
) -> Result<(), String> {
    let cmd = build_restore_window_batch(addr, fullscreen)?;
    let reply = ipc_query(&cmd)?;
    check_dispatch_ok(&reply)
}

/// Return keyboard/window focus to `game_addr` while keeping `overlay_addr`
/// raised at the top of the floating stack.
pub fn focus_game_keeping_overlay_top(
    game_addr: &str,
    overlay_addr: Option<&str>,
) -> Result<(), String> {
    let cmd = build_focus_game_keeping_overlay_top_batch(game_addr, overlay_addr)?;
    let reply = ipc_query(&cmd)?;
    check_dispatch_ok(&reply)
}

/// Return keyboard/window focus to the given Hyprland window address.
pub fn focus_window(addr: &str) -> Result<(), String> {
    restore_game_window(addr, None)
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
    fn valid_address_and_workspace_reject_injections() {
        assert!(is_valid_address("0x55a1b2c3"));
        assert!(is_valid_address("0xabcdef0123456789"));
        assert!(!is_valid_address("55a1b2c3"));
        assert!(!is_valid_address("0x"));
        assert!(!is_valid_address("0x1234;dispatch"));
        assert!(!is_valid_address("0x1234,foo"));

        assert!(is_valid_workspace_name("special:game"));
        assert!(!is_valid_workspace_name(""));
        assert!(!is_valid_workspace_name("ws,1"));
        assert!(!is_valid_workspace_name("ws;dispatch"));
        assert!(!is_valid_workspace_name("ws\n2"));
    }

    #[test]
    fn parse_active_window_handles_empty_and_fullscreen_fields() {
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
            "workspace": { "id": 3, "name": "3" },
            "monitor": 1,
            "fullscreen": 2,
            "fullscreenClient": 2,
            "pinned": false
        }"#;
        let aw = parse_active_window(sample).expect("valid active window");
        assert_eq!(aw.address, "0x55e12340");
        assert_eq!(aw.pid, 4242);
        assert_eq!(aw.title, "Hades II");
        assert_eq!(aw.workspace_id, 3);
        assert_eq!(aw.workspace_name, "3");
        assert_eq!(aw.monitor, 1);
        assert_eq!(aw.fullscreen, 2);
        assert_eq!(aw.fullscreen_client, 2);
        assert!(!aw.pinned);

        let legacy_bool = r#"{
            "address": "0x55e12340",
            "pid": 4242,
            "title": "Legacy",
            "workspace": { "id": 1, "name": "1" },
            "fullscreen": true
        }"#;
        let aw_legacy = parse_active_window(legacy_bool).expect("legacy bool fullscreen");
        assert_eq!(aw_legacy.fullscreen, 2);
        assert_eq!(aw_legacy.fullscreen_client, 0);
    }

    #[test]
    fn select_window_matches_pid_then_title_and_finds_fullscreen_blocker() {
        let json = r#"[
            {
                "address": "0x10",
                "pid": 100,
                "title": "Aurora",
                "class": "aurora",
                "workspace": { "id": 1, "name": "1" },
                "monitor": 0,
                "mapped": true,
                "hidden": false,
                "floating": false,
                "fullscreen": 0,
                "fullscreenClient": 0,
                "at": [0, 0],
                "size": [1280, 800]
            },
            {
                "address": "0x20",
                "pid": 200,
                "title": "Launcher",
                "class": "game",
                "workspace": { "id": 2, "name": "2" },
                "monitor": 0,
                "mapped": true,
                "hidden": false,
                "floating": false,
                "fullscreen": 0,
                "fullscreenClient": 0,
                "at": [0, 0],
                "size": [400, 300]
            },
            {
                "address": "0x30",
                "pid": 200,
                "title": "My Game",
                "class": "game",
                "workspace": { "id": 2, "name": "2" },
                "monitor": 0,
                "mapped": true,
                "hidden": false,
                "floating": false,
                "fullscreen": 2,
                "fullscreenClient": 2,
                "at": [0, 0],
                "size": [1920, 1080]
            },
            {
                "address": "0x40",
                "pid": 100,
                "title": "Aurora Overlay",
                "class": "aurora",
                "workspace": { "id": 1, "name": "1" },
                "monitor": 0,
                "mapped": true,
                "hidden": false,
                "floating": true,
                "fullscreen": 0,
                "fullscreenClient": 0,
                "at": [0, 0],
                "size": [1920, 1080]
            }
        ]"#;
        let clients = parse_clients(json);
        let picked = select_window_by_pid_title(&clients, &[200], "").expect("match by pid");
        assert_eq!(picked.address, "0x30");
        assert_eq!(picked.fullscreen, 2);
        let by_title =
            select_window_by_pid_title(&clients, &[999], "My Game").expect("fallback title");
        assert_eq!(by_title.address, "0x30");
        let ov = select_overlay_window(&clients, 100, "Aurora Overlay").expect("overlay window");
        assert_eq!(ov.address, "0x40");
        let blocker = find_fullscreen_blocker(&clients, "0x40", 2, Some("0x30"))
            .expect("fullscreen blocker");
        assert_eq!(blocker.address, "0x30");
        assert_eq!(
            find_fullscreen_blocker(&clients, "0x40", 1, None),
            None
        );
    }

    #[test]
    fn parse_monitors_and_select_logical_geom_respects_scale_and_transform() {
        let json = r#"[
            {
                "id": 0,
                "x": 0,
                "y": 0,
                "width": 3840,
                "height": 2160,
                "scale": 1.5,
                "transform": 0,
                "activeWorkspace": { "id": 1, "name": "1" }
            },
            {
                "id": 1,
                "x": 2560,
                "y": 0,
                "width": 1920,
                "height": 1080,
                "scale": 1.0,
                "transform": 1,
                "activeWorkspace": { "id": 2, "name": "2" }
            }
        ]"#;
        let mons = parse_monitors(json);
        assert_eq!(mons.len(), 2);
        assert_eq!(mons[0].logical_geom(), Some([0, 0, 2560, 1440]));
        assert_eq!(mons[1].logical_geom(), Some([2560, 0, 1080, 1920]));
        assert_eq!(select_monitor_geom(&mons, None, 2), Some([2560, 0, 1080, 1920]));
    }

    #[test]
    fn batch_builder_places_raises_and_restores_fullscreen() {
        assert!(
            build_place_overlay_batch(&OverlayPlacement {
                overlay_addr: "bad",
                overlay_floating: false,
                overlay_fullscreen: 0,
                ws_id: 2,
                ws_name: "2",
                monitor_geom: None,
                fullscreen_blocker: None,
            })
            .is_err()
        );
        assert!(
            build_place_overlay_batch(&OverlayPlacement {
                overlay_addr: "0x123",
                overlay_floating: false,
                overlay_fullscreen: 0,
                ws_id: 0,
                ws_name: "bad,name",
                monitor_geom: None,
                fullscreen_blocker: None,
            })
            .is_err()
        );
        assert!(
            build_place_overlay_batch(&OverlayPlacement {
                overlay_addr: "0x123",
                overlay_floating: false,
                overlay_fullscreen: 0,
                ws_id: 2,
                ws_name: "2",
                monitor_geom: None,
                fullscreen_blocker: Some(("bad;cmd", 2)),
            })
            .is_err()
        );
        assert!(
            build_place_overlay_batch(&OverlayPlacement {
                overlay_addr: "0x123",
                overlay_floating: false,
                overlay_fullscreen: 0,
                ws_id: 2,
                ws_name: "2",
                monitor_geom: None,
                fullscreen_blocker: Some(("0x456", 9)),
            })
            .is_err()
        );

        // Fullscreen game on workspace 2: clears internal fullscreen while preserving
        // client state, moves & floats overlay, fits monitor, focuses, and raises top.
        let fs_cmd = build_place_overlay_batch(&OverlayPlacement {
            overlay_addr: "0xabc123",
            overlay_floating: false,
            overlay_fullscreen: 0,
            ws_id: 2,
            ws_name: "2",
            monitor_geom: Some([0, 0, 2560, 1440]),
            fullscreen_blocker: Some(("0xdeadbeef", 2)),
        })
        .unwrap();
        assert_eq!(
            fs_cmd,
            "[[BATCH]]dispatch focuswindow address:0xdeadbeef;\
            dispatch fullscreenstate 0 2 set;\
            dispatch movetoworkspacesilent 2,address:0xabc123;\
            dispatch togglefloating address:0xabc123;\
            dispatch resizewindowpixel exact 2560 1440,address:0xabc123;\
            dispatch movewindowpixel exact 0 0,address:0xabc123;\
            dispatch focuswindow address:0xabc123;\
            dispatch alterzorder top,address:0xabc123"
        );

        // Windowed/borderless floating game + already-floating overlay on special workspace:
        // does not toggle floating back to tiled, still raises z-order above the game.
        let floating_cmd = build_place_overlay_batch(&OverlayPlacement {
            overlay_addr: "0xabc123",
            overlay_floating: true,
            overlay_fullscreen: 2,
            ws_id: -98,
            ws_name: "special:game",
            monitor_geom: Some([1920, 0, 1920, 1080]),
            fullscreen_blocker: None,
        })
        .unwrap();
        assert!(!floating_cmd.contains("togglefloating"));
        assert!(floating_cmd.contains("movetoworkspacesilent special:game,address:0xabc123"));
        assert!(floating_cmd.contains("fullscreenstate 0 0 set"));
        assert!(floating_cmd.contains("movewindowpixel exact 1920 0,address:0xabc123"));
        assert!(floating_cmd.ends_with("dispatch alterzorder top,address:0xabc123"));
    }

    #[test]
    fn restore_and_pinned_focus_batches_validate_and_format() {
        assert!(build_restore_window_batch("bad", Some((2, 2))).is_err());
        assert!(build_restore_window_batch("0x123", Some((4, 2))).is_err());
        assert_eq!(
            build_restore_window_batch("0xdeadbeef", None).unwrap(),
            "dispatch focuswindow address:0xdeadbeef"
        );
        assert_eq!(
            build_restore_window_batch("0xdeadbeef", Some((2, 2))).unwrap(),
            "[[BATCH]]dispatch focuswindow address:0xdeadbeef;dispatch fullscreenstate 2 2 set"
        );

        assert!(build_focus_game_keeping_overlay_top_batch("0xdeadbeef", Some("bad")).is_err());
        assert_eq!(
            build_focus_game_keeping_overlay_top_batch("0xdeadbeef", Some("0xabc123")).unwrap(),
            "[[BATCH]]dispatch focuswindow address:0xdeadbeef;dispatch alterzorder top,address:0xabc123"
        );
    }
}
