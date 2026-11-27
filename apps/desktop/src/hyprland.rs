//! Minimal Hyprland IPC helpers for locating game/overlay windows,
//! and moving, raising and focusing Aurora's fallback overlay across workspaces.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

const IPC_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_IPC_REPLY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchSyntax {
    Legacy = 0,
    Lua = 1,
}

static DISPATCH_SYNTAX: AtomicU8 = AtomicU8::new(DispatchSyntax::Legacy as u8);

fn load_dispatch_syntax() -> DispatchSyntax {
    match DISPATCH_SYNTAX.load(Ordering::Relaxed) {
        1 => DispatchSyntax::Lua,
        _ => DispatchSyntax::Legacy,
    }
}

fn store_dispatch_syntax(syntax: DispatchSyntax) {
    DISPATCH_SYNTAX.store(syntax as u8, Ordering::Relaxed);
}

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

/// Parameters for placing and raising Aurora's fallback overlay window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPlacement<'a> {
    pub overlay_addr: &'a str,
    pub ws_id: i64,
    pub ws_name: &'a str,
    pub monitor_geom: Option<[i32; 4]>,
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
            .any(|b| matches!(b, b',' | b';' | b'[' | b']') || b.is_ascii_control())
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
    Some(
        PathBuf::from(runtime)
            .join("hypr")
            .join(sig)
            .join(".socket.sock"),
    )
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

fn lua_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn is_lua_dispatch_error(reply: &str) -> bool {
    reply.contains("dispatch in lua is a shorthand") || reply.contains("return hl.dispatch(")
}

/// Build the commands for preparing a window that can be focused over fullscreen.
fn prepare_overlay_commands(
    p: &OverlayPlacement<'_>,
    syntax: DispatchSyntax,
) -> Result<Vec<String>, String> {
    if !is_valid_address(p.overlay_addr) {
        return Err("invalid hyprland overlay window address".into());
    }
    let ws = workspace_selector(p.ws_id, p.ws_name)
        .ok_or_else(|| "invalid hyprland target workspace".to_string())?;
    let address = format!("address:{}", p.overlay_addr);
    let mut commands = match syntax {
        DispatchSyntax::Legacy => vec![
            format!("dispatch setfloating {address}"),
            format!("dispatch movetoworkspacesilent {ws},{address}"),
        ],
        DispatchSyntax::Lua => {
            let win = lua_string(&address);
            let ws_lua = lua_string(&ws);
            vec![
                format!("dispatch hl.dsp.window.float({{ action = \"enable\", window = {win} }})"),
                format!(
                    "dispatch hl.dsp.window.move({{ workspace = {ws_lua}, follow = false, window = {win} }})"
                ),
            ]
        }
    };
    if let Some([x, y, w, h]) = p.monitor_geom {
        if w > 0 && h > 0 {
            match syntax {
                DispatchSyntax::Legacy => {
                    commands.push(format!(
                        "dispatch resizewindowpixel exact {w} {h},{address}"
                    ));
                    commands.push(format!("dispatch movewindowpixel exact {x} {y},{address}"));
                }
                DispatchSyntax::Lua => {
                    let win = lua_string(&address);
                    commands.push(format!(
                        "dispatch hl.dsp.window.resize({{ x = {w}, y = {h}, relative = false, window = {win} }})"
                    ));
                    commands.push(format!(
                        "dispatch hl.dsp.window.move({{ x = {x}, y = {y}, relative = false, window = {win} }})"
                    ));
                }
            }
        }
    }
    Ok(commands)
}

fn focus_command(addr: &str, syntax: DispatchSyntax) -> Result<String, String> {
    if !is_valid_address(addr) {
        return Err("invalid hyprland window address".into());
    }
    let address = format!("address:{addr}");
    Ok(match syntax {
        DispatchSyntax::Legacy => format!("dispatch focuswindow {address}"),
        DispatchSyntax::Lua => {
            format!("dispatch hl.dsp.focus({{ window = {} }})", lua_string(&address))
        }
    })
}

fn raise_command(addr: &str, syntax: DispatchSyntax) -> Result<String, String> {
    if !is_valid_address(addr) {
        return Err("invalid hyprland window address".into());
    }
    let address = format!("address:{addr}");
    Ok(match syntax {
        DispatchSyntax::Legacy => format!("dispatch alterzorder top,{address}"),
        DispatchSyntax::Lua => format!(
            "dispatch hl.dsp.window.alter_zorder({{ mode = \"top\", window = {} }})",
            lua_string(&address)
        ),
    })
}

fn check_dispatch_ok(reply: &str, expected_replies: usize) -> Result<(), String> {
    let replies: Vec<_> = reply
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if replies.len() == expected_replies && replies.iter().all(|line| *line == "ok") {
        Ok(())
    } else {
        Err(format!("hyprland dispatch error: {}", reply.trim()))
    }
}

fn dispatch_batch_with_fallback(
    syntax: &mut DispatchSyntax,
    build: impl Fn(DispatchSyntax) -> Result<Vec<String>, String>,
    query: &mut impl FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    let commands = build(*syntax)?;
    let batch = format!("[[BATCH]]{}", commands.join(";"));
    let reply = query(&batch)?;
    if *syntax == DispatchSyntax::Legacy && is_lua_dispatch_error(&reply) {
        *syntax = DispatchSyntax::Lua;
        let lua_commands = build(DispatchSyntax::Lua)?;
        let lua_batch = format!("[[BATCH]]{}", lua_commands.join(";"));
        let lua_reply = query(&lua_batch)?;
        return check_dispatch_ok(&lua_reply, lua_commands.len());
    }
    check_dispatch_ok(&reply, commands.len())
}

fn dispatch_one_with_fallback(
    syntax: &mut DispatchSyntax,
    build: impl Fn(DispatchSyntax) -> Result<String, String>,
    query: &mut impl FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    let command = build(*syntax)?;
    let reply = query(&command)?;
    if *syntax == DispatchSyntax::Legacy && is_lua_dispatch_error(&reply) {
        *syntax = DispatchSyntax::Lua;
        let lua_command = build(DispatchSyntax::Lua)?;
        let lua_reply = query(&lua_command)?;
        return check_dispatch_ok(&lua_reply, 1);
    }
    check_dispatch_ok(&reply, 1)
}

fn place_overlay_with(
    p: &OverlayPlacement<'_>,
    syntax: &mut DispatchSyntax,
    mut query: impl FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    dispatch_batch_with_fallback(syntax, |s| prepare_overlay_commands(p, s), &mut query)?;

    // Focusing a floating window on a fullscreen workspace enables Hyprland's
    // above-fullscreen rendering and fade. Focusing a tiled window can transfer
    // or exit fullscreen, so verify the compositor applied the preparation.
    let windows = parse_clients(&query("j/clients")?);
    if !windows.iter().any(|w| {
        w.address == p.overlay_addr
            && w.workspace_id == p.ws_id
            && w.mapped
            && !w.hidden
            && w.floating
            && w.fullscreen == 0
    }) {
        return Err(
            "hyprland overlay is not a mapped floating window on the target workspace".into(),
        );
    }
    dispatch_one_with_fallback(syntax, |s| focus_command(p.overlay_addr, s), &mut query)?;
    dispatch_one_with_fallback(syntax, |s| raise_command(p.overlay_addr, s), &mut query)
}

/// Place Aurora's overlay above the game without changing the game's fullscreen mode.
pub fn place_overlay_on_workspace(p: &OverlayPlacement<'_>) -> Result<(), String> {
    let mut syntax = load_dispatch_syntax();
    let res = place_overlay_with(p, &mut syntax, ipc_query);
    store_dispatch_syntax(syntax);
    res
}

fn focus_game_with(
    game_addr: &str,
    overlay_addr: Option<&str>,
    syntax: &mut DispatchSyntax,
    mut query: impl FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    // Validate both addresses up front before sending any IPC command.
    let _ = focus_command(game_addr, *syntax)?;
    if let Some(addr) = overlay_addr {
        let _ = raise_command(addr, *syntax)?;
    }
    dispatch_one_with_fallback(syntax, |s| focus_command(game_addr, s), &mut query)?;
    if let Some(addr) = overlay_addr {
        dispatch_one_with_fallback(syntax, |s| raise_command(addr, s), &mut query)?;
    }
    Ok(())
}

/// Return keyboard focus to the game and keep any click-through overlay above it.
pub fn focus_game_keeping_overlay_top(
    game_addr: &str,
    overlay_addr: Option<&str>,
) -> Result<(), String> {
    let mut syntax = load_dispatch_syntax();
    let res = focus_game_with(game_addr, overlay_addr, &mut syntax, ipc_query);
    store_dispatch_syntax(syntax);
    res
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
            socket_path_from(Some(OsStr::new("/run/user/1000")), Some(OsStr::new("a/b"))),
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
        assert!(!is_valid_workspace_name("ws[2]"));
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
    fn select_window_matches_pid_then_title() {
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
        assert_eq!(
            select_monitor_geom(&mons, None, 2),
            Some([2560, 0, 1080, 1920])
        );
    }

    const PREPARE_OK: &str = "ok\n\n\nok\n\n\nok\n\n\nok";

    fn placement() -> OverlayPlacement<'static> {
        OverlayPlacement {
            overlay_addr: "0x123",
            ws_id: 2,
            ws_name: "2",
            monitor_geom: Some([1920, 0, 1920, 1080]),
        }
    }

    #[test]
    fn floating_overlay_raises_without_changing_game_fullscreen() {
        for mode in [0, 1, 2, 3] {
            let mut requests = Vec::new();
            let mut syntax = DispatchSyntax::Legacy;
            place_overlay_with(&placement(), &mut syntax, |command| {
                requests.push(command.to_string());
                Ok(match command {
                    "j/clients" => format!(
                        r#"[
                            {{"address":"0x123","mapped":true,"floating":true,"workspace":{{"id":2}}}},
                            {{"address":"0x456","mapped":true,"floating":true,"workspace":{{"id":2}},
                              "fullscreen":{mode},"fullscreenClient":2}}
                        ]"#
                    ),
                    _ if command.starts_with("[[BATCH]]") => PREPARE_OK.into(),
                    _ => "ok".into(),
                })
            })
            .unwrap();
            assert_eq!(syntax, DispatchSyntax::Legacy);
            assert_eq!(
                requests,
                [
                    concat!(
                        "[[BATCH]]dispatch setfloating address:0x123;",
                        "dispatch movetoworkspacesilent 2,address:0x123;",
                        "dispatch resizewindowpixel exact 1920 1080,address:0x123;",
                        "dispatch movewindowpixel exact 1920 0,address:0x123"
                    ),
                    "j/clients",
                    "dispatch focuswindow address:0x123",
                    "dispatch alterzorder top,address:0x123",
                ]
            );
        }
    }

    #[test]
    fn lua_ipc_fallback_places_and_restores_focus() {
        const LUA_ERR: &str = "error: 3 [string \"return hl.dispatch(setfloating address:0x123\"]:1: ')' expected near 'address'\n\n → Note: dispatch in lua is a shorthand for hl.dispatch(...), your syntax might need to be updated.";
        let mut requests = Vec::new();
        let mut syntax = DispatchSyntax::Legacy;
        place_overlay_with(&placement(), &mut syntax, |command| {
            requests.push(command.to_string());
            Ok(if command == "j/clients" {
                r#"[{"address":"0x123","mapped":true,"floating":true,"workspace":{"id":2}}]"#.into()
            } else if command.contains("setfloating") {
                LUA_ERR.into()
            } else if command.starts_with("[[BATCH]]") {
                PREPARE_OK.into()
            } else {
                "ok".into()
            })
        })
        .unwrap();
        assert_eq!(syntax, DispatchSyntax::Lua);
        assert_eq!(
            requests,
            [
                concat!(
                    "[[BATCH]]dispatch setfloating address:0x123;",
                    "dispatch movetoworkspacesilent 2,address:0x123;",
                    "dispatch resizewindowpixel exact 1920 1080,address:0x123;",
                    "dispatch movewindowpixel exact 1920 0,address:0x123"
                ),
                concat!(
                    "[[BATCH]]dispatch hl.dsp.window.float({ action = \"enable\", window = \"address:0x123\" });",
                    "dispatch hl.dsp.window.move({ workspace = \"2\", follow = false, window = \"address:0x123\" });",
                    "dispatch hl.dsp.window.resize({ x = 1920, y = 1080, relative = false, window = \"address:0x123\" });",
                    "dispatch hl.dsp.window.move({ x = 1920, y = 0, relative = false, window = \"address:0x123\" })"
                ),
                "j/clients",
                "dispatch hl.dsp.focus({ window = \"address:0x123\" })",
                "dispatch hl.dsp.window.alter_zorder({ mode = \"top\", window = \"address:0x123\" })",
            ]
        );

        // Once switched to Lua, focus restore goes straight to Lua without retrying legacy.
        let mut close_requests = Vec::new();
        focus_game_with("0x456", Some("0x123"), &mut syntax, |command| {
            close_requests.push(command.to_string());
            Ok("ok".into())
        })
        .unwrap();
        assert_eq!(
            close_requests,
            [
                "dispatch hl.dsp.focus({ window = \"address:0x456\" })",
                "dispatch hl.dsp.window.alter_zorder({ mode = \"top\", window = \"address:0x123\" })",
            ]
        );
    }

    #[test]
    fn placement_never_focuses_a_tiled_fullscreen_or_misplaced_overlay() {
        for (floating, fullscreen, workspace) in [(false, 0, 2), (true, 2, 2), (true, 0, 3)] {
            let mut syntax = DispatchSyntax::Legacy;
            let result = place_overlay_with(&placement(), &mut syntax, |command| {
                Ok(if command == "j/clients" {
                    format!(
                        r#"[{{"address":"0x123","mapped":true,"floating":{floating},
                              "fullscreen":{fullscreen},"workspace":{{"id":{workspace}}}}}]"#
                    )
                } else {
                    assert!(command.starts_with("[[BATCH]]"), "{command}");
                    PREPARE_OK.into()
                })
            });
            assert!(result.is_err());
        }
    }

    #[test]
    fn placement_rejects_invalid_input_and_incomplete_dispatches() {
        let mut p = placement();
        let mut syntax = DispatchSyntax::Legacy;
        p.overlay_addr = "0x123;dispatch";
        assert!(
            place_overlay_with(&p, &mut syntax, |_| panic!("invalid address reached IPC")).is_err()
        );
        p.overlay_addr = "0x123";
        p.ws_id = 0;
        p.ws_name = "ws[;dispatch";
        assert!(
            place_overlay_with(&p, &mut syntax, |_| panic!("invalid workspace reached IPC"))
                .is_err()
        );
        p.ws_id = -98;
        p.ws_name = "special:game";
        p.monitor_geom = None;
        for reply in ["", "ok", "ok\n\n\nWindow not found"] {
            let mut syntax = DispatchSyntax::Legacy;
            let result = place_overlay_with(&p, &mut syntax, |command| {
                assert_eq!(
                    command,
                    "[[BATCH]]dispatch setfloating address:0x123;dispatch movetoworkspacesilent special:game,address:0x123"
                );
                Ok(reply.into())
            });
            assert!(result.is_err());
        }
    }

    #[test]
    fn close_restores_game_focus_and_raises_pinned_overlay_when_present() {
        for overlay in [None, Some("0x123")] {
            let mut requests = Vec::new();
            let mut syntax = DispatchSyntax::Legacy;
            focus_game_with("0x456", overlay, &mut syntax, |command| {
                requests.push(command.to_string());
                Ok("ok".into())
            })
            .unwrap();
            let mut expected = vec!["dispatch focuswindow address:0x456"];
            if overlay.is_some() {
                expected.push("dispatch alterzorder top,address:0x123");
            }
            assert_eq!(requests, expected);
        }
        let mut syntax = DispatchSyntax::Legacy;
        assert!(
            focus_game_with("0x456", Some("bad"), &mut syntax, |_| panic!(
                "invalid address reached IPC"
            ))
            .is_err()
        );
        assert!(
            focus_game_with("0x456", Some("0x123"), &mut syntax, |command| {
                assert_eq!(command, "dispatch focuswindow address:0x456");
                Ok("Window not found".into())
            })
            .is_err()
        );
    }
}
