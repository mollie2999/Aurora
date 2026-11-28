//! `steam://` links, single-instance forwarding, and desktop shortcuts.
//!
//! - Aurora can register itself as the handler for `steam://` links
//!   (Settings → Interface). The previous handler is saved so it can be
//!   restored with one click.
//! - Only one copy runs: a second launch (e.g. from a link or shortcut)
//!   forwards its link to the running window over localhost and exits.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::sync::mpsc;

const PORT: u16 = 47_630;

/// Validate an external web URL, allowing only `https://` and `http://` URLs
/// with a non-empty host and no Windows backslash/UNC authority tricks.
pub fn validate_http_url(raw: &str) -> Option<reqwest::Url> {
    let s = raw.trim();
    if s.chars().any(|c| c.is_control()) {
        return None;
    }
    let rest = if s.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://")) {
        &s[8..]
    } else if s.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://")) {
        &s[7..]
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('\\') {
        return None;
    }
    let parsed = reqwest::Url::parse(s).ok()?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return None;
    }
    if parsed.host_str().is_none_or(|h| h.is_empty()) {
        return None;
    }
    Some(parsed)
}

/// Open an external `https://` or `http://` URL in the user's browser.
pub fn open_http(url: &str) -> bool {
    let Some(parsed) = validate_http_url(url) else { return false };
    open::that_detached(parsed.as_str()).is_ok()
}

/// The page in Aurora a Steam web link stands for, if Aurora has one:
/// profiles (`/profiles/<id>` and custom `/id/<name>` URLs), store pages,
/// and the community pages the hub shows (workshop, guides, discussions…).
pub fn in_app(url: &str) -> Option<(String, Option<String>)> {
    let url = url.trim();
    if let Some(rest) = url.trim_matches('"').strip_prefix("steam://") {
        let (command, target) = rest.split_once('/')?;
        if command.eq_ignore_ascii_case("url") {
            let (destination, id) = target.trim_end_matches('/').split_once('/')?;
            if destination.eq_ignore_ascii_case("SteamIDPage") {
                let id = id.parse::<u64>().ok().filter(|&id| id != 0)?;
                return Some(("profile".into(), Some(id.to_string())));
            }
        }
        if command.eq_ignore_ascii_case("openurl") && validate_http_url(target).is_some() {
            return in_app(target);
        }
        return None;
    }
    let scheme_len = if url.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://")) {
        8
    } else if url.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://")) {
        7
    } else {
        return None;
    };
    let rest = &url[scheme_len..];
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let parts: Vec<&str> = path.split(['?', '#']).next().unwrap_or_default().split('/').filter(|p| !p.is_empty()).collect();
    match (host, parts.as_slice()) {
        ("steamcommunity.com", [kind, who, more @ ..]) if kind.eq_ignore_ascii_case("profiles") || kind.eq_ignore_ascii_case("id") => {
            // Profile lists Aurora shows itself; anything else opens the profile.
            let section = match more.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("games") => "/games",
                Some("badges") => "/badges",
                Some("recommended" | "reviews") => "/reviews",
                Some("groups") => "/groups",
                Some("groupscommon") => "/groupscommon",
                _ => "",
            };
            let who = if kind.eq_ignore_ascii_case("id") { format!("id:{who}") } else { who.parse::<u64>().ok()?.to_string() };
            return Some(("profile".into(), Some(format!("{who}{section}"))));
        }
        ("store.steampowered.com", [app, id, ..]) if app.eq_ignore_ascii_case("app") => {
            return Some(("store".into(), Some(format!("app:{}", id.parse::<u64>().ok()?))));
        }
        _ => {}
    }
    let view = crate::builtin::hub::route(url)?;
    Some(view.to_page())
}

/// Where a link clicked in Steam text goes: a page in Aurora when it has one
/// (see [`in_app`]), otherwise the browser.
pub fn web_action(url: &str) -> Option<extensions::Action> {
    if let Some((page, arg)) = in_app(url) {
        return Some(extensions::Action::Navigate { page, arg });
    }
    open_http(url);
    None
}

/// Right-clicking empty space on a page offers "Copy page URL" (its address
/// on Steam), like a browser. Call it before drawing the page: its widgets
/// are added after, so they keep their own clicks and menus.
pub fn page_url_menu(ui: &mut egui::Ui, url: &str) {
    let resp = ui.interact(ui.max_rect(), ui.id().with("aurora-page-url-menu"), egui::Sense::click());
    resp.context_menu(|ui| {
        if ui.button(format!("{}  {}", ui::icons::COPY, i18n::t!("Copy page URL"))).clicked() {
            ui.ctx().copy_text(url.to_string());
            ui.close();
        }
    });
}

/// `base` with these query parameters, encoded.
pub fn with_query(base: &str, params: &[(&str, &str)]) -> String {
    reqwest::Url::parse_with_params(base, params).map(String::from).unwrap_or_else(|_| base.to_string())
}

/// Whether Aurora can open this Steam destination.
pub(crate) fn is_aurora_link(url: &str) -> bool {
    in_app(url).is_some() || parse(url).is_some_and(|link| link != Link::Show) || big_picture_mode(url).is_some()
}

/// Keep the destination of a Steam link without re-running its command.
pub(crate) fn retained_page_arg<'a>(page: &str, arg: Option<&'a str>) -> Option<&'a str> {
    let arg = arg?;
    match (page, arg.split_once(':')) {
        ("library", Some(("install" | "uninstall" | "verify", id))) => Some(id),
        ("servers", Some(("connect", _))) => None,
        ("library", _) if matches!(arg, "restore" | "add") => None,
        _ => Some(arg),
    }
}

/// Validate a YouTube video ID (`[A-Za-z0-9_-]+`).
pub fn valid_youtube_id(id: &str) -> Option<&str> {
    let id = id.trim();
    (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).then_some(id)
}

/// Open a validated YouTube video ID on `youtube.com`.
pub fn open_youtube(id: &str) -> bool {
    let Some(id) = valid_youtube_id(id) else { return false };
    open_http(&format!("https://www.youtube.com/watch?v={id}"))
}

/// What a `steam://` link asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Run(u32),
    Install(u32),
    Uninstall(u32),
    Validate(u32),
    Store(u32),
    Connect { addr: String, password: Option<String> },
    Page(&'static str),
    Show,
}

fn valid_connect_addr(raw: &str) -> Option<String> {
    let addr = raw.trim();
    if addr.is_empty()
        || addr.len() > 253
        || addr.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, ';' | '"' | '\'' | '\\' | '/'))
    {
        return None;
    }
    let (host, port) = addr.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    let port_num: u16 = port.parse().ok()?;
    if port_num == 0 {
        return None;
    }
    if host.starts_with('[') && host.ends_with(']') {
        let inner = &host[1..host.len() - 1];
        inner.parse::<std::net::Ipv6Addr>().ok()?;
    } else if host.parse::<std::net::Ipv4Addr>().is_err() {
        if !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
            || host.starts_with('.')
            || host.ends_with('.')
        {
            return None;
        }
    }
    Some(addr.to_string())
}

fn valid_connect_password(raw: Option<&str>) -> Option<String> {
    let p = raw?.trim();
    if p.is_empty() || p.len() > 128 || p.chars().any(|c| c.is_control() || matches!(c, ';' | '"' | '\n' | '\r')) {
        return None;
    }
    Some(p.to_string())
}

pub fn parse(url: &str) -> Option<Link> {
    let rest = url.trim().trim_matches('"');
    let rest = rest.strip_prefix("steam://")?.trim_end_matches('/');
    let mut parts = rest.split('/');
    let verb = parts.next()?.to_lowercase();
    let id = || parts.clone().next().and_then(|p| p.parse::<u32>().ok());
    Some(match verb.as_str() {
        "rungameid" | "run" | "launch" => Link::Run(id()?),
        "install" => Link::Install(id()?),
        "uninstall" => Link::Uninstall(id()?),
        "validate" => Link::Validate(id()?),
        "store" | "advertise" => Link::Store(id()?),
        "connect" => Link::Connect {
            addr: valid_connect_addr(parts.next()?)?,
            password: valid_connect_password(parts.next()),
        },
        "nav" | "open" => match parts.next().unwrap_or_default() {
            "downloads" => Link::Page("downloads"),
            "friends" => Link::Page("community"),
            "settings" => Link::Page("settings"),
            "games" | "library" => Link::Page("library"),
            "servers" => Link::Page("servers"),
            "screenshots" => Link::Page("screenshots"),
            _ => Link::Show,
        },
        "friends" => Link::Page("community"),
        _ => Link::Show,
    })
}

const LISTEN_TIMEOUT: Duration = Duration::from_millis(500);
const FORWARD_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_LINE_BYTES: u64 = 2048;

fn token_path() -> PathBuf {
    client::store::config_dir().join("links.token")
}

fn generate_token() -> String {
    client::ipc::os_random_hex(32)
}

fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(dir)?;
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "config dir is a symlink"));
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

fn write_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    let Some(dir) = path.parent() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing token parent dir"));
    };
    ensure_private_dir(dir)?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "token path is a symlink"));
        }
    }
    let tmp = dir.join(format!(".links-token-{}-{}.tmp", std::process::id(), &generate_token()[..16]));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write_res = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn read_token_file(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return None;
    }
    let tok = std::fs::read_to_string(path).ok()?;
    let tok = tok.trim();
    (tok.len() >= 32 && tok.bytes().all(|b| b.is_ascii_hexdigit())).then(|| tok.to_string())
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) const BIG_PICTURE: &str = "bigpicture";
pub(crate) const BIG_PICTURE_WINDOW: &str = "bigpicture-window";
pub(crate) const DECK_PREVIEW: &str = "deck-preview";
pub(crate) const DECK_PREVIEW_WINDOW: &str = "deck-preview-window";

pub(crate) fn big_picture_request(args: impl IntoIterator<Item = impl AsRef<str>>) -> Option<&'static str> {
    let (mut deck, mut window, mut fullscreen) = (false, false, false);
    for arg in args {
        match arg.as_ref() {
            "--steam-deck" => deck = true,
            "--steam-deck=window" => {
                deck = true;
                window = true;
            }
            "--bigpicture=window" => window = true,
            "--bigpicture" => fullscreen = true,
            _ => {}
        }
    }
    match (deck, window, fullscreen) {
        (true, true, _) => Some(DECK_PREVIEW_WINDOW),
        (true, false, _) => Some(DECK_PREVIEW),
        (false, true, _) => Some(BIG_PICTURE_WINDOW),
        (false, false, true) => Some(BIG_PICTURE),
        _ => None,
    }
}

pub(crate) fn big_picture_mode(request: &str) -> Option<(bool, bool)> {
    match request {
        BIG_PICTURE => Some((false, false)),
        BIG_PICTURE_WINDOW => Some((false, true)),
        DECK_PREVIEW => Some((true, false)),
        DECK_PREVIEW_WINDOW => Some((true, true)),
        _ => None,
    }
}


pub(crate) fn is_allowed_payload(payload: &str) -> bool {
    let p = payload.trim();
    p == "show" || big_picture_mode(p).is_some() || parse(p).is_some()
}

pub struct LinkRequest {
    pub arg: String,
    pub activation_token: Option<String>,
}

fn valid_activation_token(token: &str) -> bool {
    !token.is_empty() && !token.chars().any(char::is_control)
}

fn parse_request(payload: &str) -> Option<LinkRequest> {
    let (arg, activation_token) = if let Some(encoded) = payload.strip_prefix("activate ") {
        let (arg, token): (String, String) = serde_json::from_str(encoded).ok()?;
        if !valid_activation_token(&token) {
            return None;
        }
        (arg, Some(token))
    } else {
        (payload.to_owned(), None)
    };
    let arg = arg.trim();
    (!arg.contains(['\r', '\n']) && is_allowed_payload(arg)).then(|| LinkRequest { arg: arg.to_owned(), activation_token })
}

fn accept_one(mut stream: TcpStream, expected_token: &str) -> Option<LinkRequest> {
    if !stream.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
        return None;
    }
    stream.set_read_timeout(Some(LISTEN_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(LISTEN_TIMEOUT)).ok()?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_LINE_BYTES)).read_line(&mut line).ok()?;
    if !line.ends_with('\n') {
        return None;
    }
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let (got_token, payload) = trimmed.split_once(' ')?;
    if expected_token.is_empty() || !ct_eq(got_token.as_bytes(), expected_token.as_bytes()) {
        return None;
    }
    let request = parse_request(payload.trim())?;
    let _ = stream.write_all(b"ok\n").and_then(|_| stream.flush());
    Some(request)
}

fn forward_line(addr: SocketAddr, line: &str) -> std::io::Result<bool> {
    let mut stream = TcpStream::connect_timeout(&addr, FORWARD_TIMEOUT)?;
    stream.set_read_timeout(Some(FORWARD_TIMEOUT))?;
    stream.set_write_timeout(Some(FORWARD_TIMEOUT))?;
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    let mut ack = String::new();
    match BufReader::new(stream.take(16)).read_line(&mut ack)? {
        0 => Ok(false),
        _ if ack.trim() == "ok" => Ok(true),
        _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid link acknowledgement")),
    }
}

fn forward_with(addr: SocketAddr, token_file: &Path, arg: &str, activation_token: Option<&str>) -> bool {
    let arg = arg.trim();
    if arg.contains(['\r', '\n']) || !is_allowed_payload(arg) {
        return false;
    }
    let Some(token) = read_token_file(token_file) else { return false };
    if let Some(activation_token) = activation_token.filter(|token| valid_activation_token(token))
        && let Ok(encoded) = serde_json::to_string(&(arg, activation_token))
    {
        let line = format!("{token} activate {encoded}\n");
        if line.len() as u64 <= MAX_LINE_BYTES {
            match forward_line(addr, &line) {
                // Older listeners close an unrecognized request without an acknowledgement.
                Ok(false) => {}
                result => return result.unwrap_or(false),
            }
        }
    }
    forward_line(addr, &format!("{token} {arg}\n")).unwrap_or(false)
}

/// Try to hand `arg` to an already-running copy. Returns true if it did.
pub fn forward_to_running(arg: &str, activation_token: Option<&str>) -> bool {
    forward_with(SocketAddr::from(([127, 0, 0, 1], PORT)), &token_path(), arg, activation_token)
}

/// Listen for authenticated links from later launches. Each line received
/// carries `<token> <link|show>` or `<token> activate ["<link|show>","<activation token>"]`.
pub fn listen(ctx: egui::Context) -> mpsc::Receiver<LinkRequest> {
    let (tx, rx) = mpsc::channel();
    if let Ok(listener) = TcpListener::bind(("127.0.0.1", PORT)) {
        let token = generate_token();
        if let Err(e) = write_token_file(&token_path(), &token) {
            tracing::warn!("links listener disabled: couldn't write instance token: {e}");
            return rx;
        }
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(payload) = accept_one(stream, &token) {
                    let _ = tx.send(payload);
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
        });
    }
    rx
}

fn exe() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
}

fn config_backup() -> PathBuf {
    client::store::config_dir().join("steam-protocol-backup.txt")
}

#[cfg(windows)]
fn reg(args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("reg").args(args).creation_flags(0x0800_0000).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

const KEY: &str = r"HKCU\Software\Classes\steam\shell\open\command";

/// The command currently registered for `steam://` links, if any.
#[cfg(windows)]
pub fn current_handler() -> Option<String> {
    let out = reg(&["query", KEY, "/ve"])?;
    let line = out.lines().find(|l| l.contains("REG_SZ"))?;
    Some(line.split("REG_SZ").nth(1)?.trim().to_string())
}

#[cfg(windows)]
pub fn is_default_handler() -> bool {
    current_handler().is_some_and(|h| h.to_lowercase().contains(&exe().to_lowercase()))
}

/// Make Aurora open `steam://` links (saves the old handler first).
#[cfg(windows)]
pub fn register() -> Result<(), String> {
    if let Some(old) = current_handler().filter(|h| !h.to_lowercase().contains(&exe().to_lowercase())) {
        let _ = std::fs::create_dir_all(client::store::config_dir());
        let _ = std::fs::write(config_backup(), old);
    }
    let cmd = format!("\"{}\" \"%1\"", exe());
    let ok = reg(&["add", r"HKCU\Software\Classes\steam", "/ve", "/d", "URL:steam protocol", "/f"]).is_some()
        && reg(&["add", r"HKCU\Software\Classes\steam", "/v", "URL Protocol", "/d", "", "/f"]).is_some()
        && reg(&["add", KEY, "/ve", "/d", &cmd, "/f"]).is_some();
    if ok { Ok(()) } else { Err("Windows refused the change".into()) }
}

/// Give `steam://` links back to whatever handled them before.
#[cfg(windows)]
pub fn unregister() -> Result<(), String> {
    match std::fs::read_to_string(config_backup()) {
        Ok(old) if !old.trim().is_empty() => {
            reg(&["add", KEY, "/ve", "/d", old.trim(), "/f"]).ok_or("Windows refused the change")?;
        }
        _ => {
            // Nothing saved: point back at the official client if it's installed.
            if let Some(root) = local::find_steam_root() {
                let cmd = format!("\"{}\" -- \"%1\"", root.join("steam.exe").display());
                reg(&["add", KEY, "/ve", "/d", &cmd, "/f"]).ok_or("Windows refused the change")?;
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn is_default_handler() -> bool {
    false
}
#[cfg(not(windows))]
pub fn register() -> Result<(), String> {
    Err("Only supported on Windows for now".into())
}
#[cfg(not(windows))]
pub fn unregister() -> Result<(), String> {
    Ok(())
}

/// Put a shortcut to a game on the desktop (an internet shortcut to
/// `steam://rungameid/<id>`, with the game's own icon).
pub fn desktop_shortcut(app_id: u32, name: &str, icon: Option<PathBuf>) -> Result<PathBuf, String> {
    let desktop = directories::UserDirs::new().and_then(|d| d.desktop_dir().map(|p| p.to_path_buf())).ok_or("No desktop folder found")?;
    let safe: String = name
        .chars()
        .map(|c| if c.is_control() || r#"\/:*?"<>|"#.contains(c) { ' ' } else { c })
        .collect();
    let trimmed = safe.trim().trim_matches('.');
    let file_stem = if trimmed.is_empty() { format!("app-{app_id}") } else { trimmed.to_string() };
    let path = desktop.join(format!("{file_stem}.url"));
    let mut text = format!("[InternetShortcut]\r\nURL=steam://rungameid/{app_id}\r\n");
    if let Some(icon) = icon {
        let icon_str = icon.display().to_string();
        if !icon_str.chars().any(char::is_control) {
            text.push_str(&format!("IconFile={icon_str}\r\nIconIndex=0\r\n"));
        }
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Delete desktop shortcuts that start this game (ours and Steam's both use
/// `steam://rungameid/<id>`). Returns how many were removed.
pub fn remove_desktop_shortcuts(app_id: u32) -> usize {
    let mut dirs = Vec::new();
    if let Some(d) = directories::UserDirs::new().and_then(|d| d.desktop_dir().map(|p| p.to_path_buf())) {
        dirs.push(d);
    }
    if let Some(public) = std::env::var_os("PUBLIC") {
        dirs.push(PathBuf::from(public).join("Desktop"));
    }
    let mut removed = 0;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()).is_none_or(|x| !x.eq_ignore_ascii_case("url")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if shortcut_runs(&text, app_id) && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Whether a .url file's target is exactly this game.
fn shortcut_runs(text: &str, app_id: u32) -> bool {
    text.lines().any(|l| {
        let l = l.trim();
        l.strip_prefix("URL=").is_some_and(|u| {
            let u = u.trim().trim_end_matches('/');
            u == format!("steam://rungameid/{app_id}") || u == format!("steam://run/{app_id}")
        })
    })
}

#[cfg(test)]
mod ipc_auth_tests {
    use super::*;

    #[test]
    fn activation_payload_preserves_legacy_messages_and_rejects_line_breaks() {
        let message = parse_request("steam://run/570").unwrap();
        assert_eq!(message.arg, "steam://run/570");
        assert!(message.activation_token.is_none());
        let message = parse_request(r#"activate ["show","desktop-123"]"#).unwrap();
        assert_eq!(message.arg, "show");
        assert_eq!(message.activation_token.as_deref(), Some("desktop-123"));
        assert!(parse_request(r#"activate ["show","bad\ntoken"]"#).is_none());
    }

    #[test]
    fn activation_forwarding_falls_back_to_a_legacy_listener() {
        let dir = std::env::temp_dir().join(format!("sir-links-legacy-test-{}", std::process::id()));
        let path = dir.join("links.token");
        let token = generate_token();
        write_token_file(&path, &token).unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let receiver = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(LISTEN_TIMEOUT)).unwrap();
            let mut attempt = String::new();
            BufReader::new(stream).read_line(&mut attempt).unwrap();
            assert!(attempt.starts_with(&format!("{token} activate ")));

            let (stream, _) = listener.accept().unwrap();
            let request = accept_one(stream, &token).unwrap();
            assert_eq!(request.arg, "show");
            assert!(request.activation_token.is_none());
        });

        assert!(forward_with(addr, &path, "show", Some("desktop-123")));
        receiver.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn authenticates_and_bounds_localhost_forwarding() {
        let dir = std::env::temp_dir().join(format!("sir-links-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tok_path = dir.join("cfg").join("links.token");
        let token = generate_token();
        write_token_file(&tok_path, &token).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let f_mode = std::fs::metadata(&tok_path).unwrap().permissions().mode() & 0o777;
            let d_mode = std::fs::metadata(tok_path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
            assert_eq!(f_mode, 0o600, "token file must be 0600");
            assert_eq!(d_mode, 0o700, "token dir must be 0700");
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let tok_srv = token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(p) = accept_one(stream, &tok_srv) {
                    let _ = tx.send(p);
                }
            }
        });

        {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(b"steam://rungameid/570\n").unwrap();
        }
        {
            let mut s = TcpStream::connect(addr).unwrap();
            let bad = "0".repeat(64);
            s.write_all(format!("{bad} steam://rungameid/570\n").as_bytes()).unwrap();
        }
        {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(format!("{token} GET / HTTP/1.1\n").as_bytes()).unwrap();
        }
        {
            let mut s = TcpStream::connect(addr).unwrap();
            let big = format!("{token} steam://rungameid/{}\n", "9".repeat(2500));
            let _ = s.write_all(big.as_bytes());
        }
        let idle = TcpStream::connect(addr).unwrap();
        let t0 = std::time::Instant::now();

        assert!(forward_with(addr, &tok_path, "show", None));
        assert!(t0.elapsed() < Duration::from_secs(3), "idle client hung listener!");
        drop(idle);
        assert!(forward_with(addr, &tok_path, "steam://rungameid/570", Some("desktop-123")));
        assert!(!forward_with(addr, &tok_path, "https://evil.example", Some("desktop-123")));

        let legacy = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(legacy.arg, "show");
        assert!(legacy.activation_token.is_none());
        let activated = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(activated.arg, "steam://rungameid/570");
        assert_eq!(activated.activation_token.as_deref(), Some("desktop-123"));
        assert!(rx.try_recv().is_err(), "no invalid/unauthenticated messages may pass");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Best icon for a game: its main executable.
pub fn game_icon(app: Option<&model::OwnedApp>, install_dir: &std::path::Path) -> Option<PathBuf> {
    let exe = app?.launch.iter().find(|l| l.runs_on_this_os() && l.executable.to_lowercase().ends_with(".exe"))?;
    if exe.executable.is_empty()
        || exe.executable.contains('\0')
        || exe.executable.contains(':')
        || exe.executable.starts_with('/')
        || exe.executable.starts_with('\\')
    {
        return None;
    }
    let mut p = install_dir.to_path_buf();
    for seg in exe.executable.split(['/', '\\']) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return None;
        }
        p.push(seg);
    }
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_web_links_open_in_aurora() {
        let page = |u: &str| in_app(u).map(|(p, a)| (p, a.unwrap_or_default()));
        let p = |page: &str, arg: &str| Some((page.to_string(), arg.to_string()));
        assert_eq!(page("https://steamcommunity.com/profiles/76561198000000001"), p("profile", "76561198000000001"));
        // Custom profile URLs keep their name's case for Steam to resolve.
        assert_eq!(page("https://steamcommunity.com/id/MissColourz/"), p("profile", "id:MissColourz"));
        assert_eq!(page("https://steamcommunity.com/id/yayazzli/groupscommon/"), p("profile", "id:yayazzli/groupscommon"));
        assert_eq!(page("https://www.steamcommunity.com/id/x/games/?tab=all"), p("profile", "id:x/games"));
        assert_eq!(page("https://store.steampowered.com/app/570/Dota_2/"), p("store", "app:570"));
        assert_eq!(page("https://steamcommunity.com/sharedfiles/filedetails/?id=123"), p("workshop_item", "123"));
        assert_eq!(page("https://example.com/id/x"), None);
        assert_eq!(page("https://steamcommunity.com/profiles/notanumber"), None);
    }

    #[test]
    fn parses_links() {
        assert_eq!(parse("steam://rungameid/570"), Some(Link::Run(570)));
        assert_eq!(parse("\"steam://launch/730/\""), Some(Link::Run(730)));
        assert_eq!(parse("steam://install/440"), Some(Link::Install(440)));
        assert_eq!(parse("steam://store/620"), Some(Link::Store(620)));
        assert_eq!(parse("steam://open/downloads"), Some(Link::Page("downloads")));
        assert_eq!(
            parse("steam://connect/198.51.100.1:27015"),
            Some(Link::Connect { addr: "198.51.100.1:27015".into(), password: None })
        );
        assert_eq!(
            parse("steam://connect/play.example.com:27015/secret"),
            Some(Link::Connect { addr: "play.example.com:27015".into(), password: Some("secret".into()) })
        );
        assert_eq!(parse("steam://connect/1.2.3.4:27015;quit"), None);
        assert_eq!(parse("steam://connect/notaport"), None);
        assert_eq!(parse("https://example.com"), None);
        assert!(shortcut_runs("[InternetShortcut]\r\nURL=steam://rungameid/570\r\n", 570));
        assert!(!shortcut_runs("[InternetShortcut]\r\nURL=steam://rungameid/5700\r\n", 570));
    }

    #[test]
    fn reopening_a_steam_link_tab_does_not_replay_the_command() {
        assert_eq!(retained_page_arg("library", Some("install:440")), Some("440"));
        assert_eq!(retained_page_arg("library", Some("uninstall:440")), Some("440"));
        assert_eq!(retained_page_arg("library", Some("verify:440")), Some("440"));
        assert_eq!(retained_page_arg("servers", Some("connect:play.example.com:27015")), None);
        assert_eq!(retained_page_arg("store", Some("app:440")), Some("app:440"));
    }

    #[test]
    fn recognizes_supported_game_links_for_the_overlay() {
        assert!(is_aurora_link("steam://store/620"));
        assert!(is_aurora_link("steam://open/downloads"));
        assert!(is_aurora_link("https://store.steampowered.com/app/620/"));
        assert!(!is_aurora_link("steam://unknown"));
        assert!(!is_aurora_link("https://example.org/"));
    }

    #[test]
    fn steam_profile_protocol_links_open_the_native_profile() {
        let expected = Some(("profile".to_string(), Some("76561197999984396".to_string())));
        assert_eq!(in_app("steam://url/SteamIDPage/76561197999984396"), expected);
        assert_eq!(in_app("steam://openurl/https://steamcommunity.com/profiles/76561197999984396"), expected);
        assert!(!is_aurora_link("steam://url/SteamIDPage/not-an-id"));
        assert!(!is_aurora_link("steam://openurl/https://example.org/"));
    }

    #[test]
    fn validates_http_urls_and_youtube_ids() {
        assert!(validate_http_url("https://store.steampowered.com/app/440").is_some());
        assert!(validate_http_url("http://example.com/path?q=1").is_some());
        for bad in [
            "",
            "file:///etc/passwd",
            "file://attacker/share/a.exe",
            r"\\attacker\share\a.exe",
            r"http:\\attacker\share",
            "http:/attacker/share",
            "https://",
            "http://",
            "javascript:alert(1)",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=calc",
            "steam://run/440",
            "https://example.com/\nfoo",
        ] {
            assert!(validate_http_url(bad).is_none(), "should reject {bad:?}");
        }
        assert_eq!(valid_youtube_id("dQw4w9WgXcQ"), Some("dQw4w9WgXcQ"));
        assert_eq!(valid_youtube_id("ZF09w_v7-hw"), Some("ZF09w_v7-hw"));
        for bad_yt in ["", "abc&list=evil", "../x", "a/b", "a?b=1", "a b"] {
            assert!(valid_youtube_id(bad_yt).is_none(), "should reject yt id {bad_yt:?}");
        }
    }

    #[test]
    fn file_image_loader_is_disabled() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        for uri in ["file:///etc/passwd", "file://attacker/share/pwn.png", "https://example.com/a.svg"] {
            let res = ctx.try_load_bytes(uri);
            assert!(matches!(res, Err(egui::load::LoadError::NoMatchingBytesLoader)), "expected NoMatchingBytesLoader for {uri}, got {:?}", res.err());
        }
    }

    #[test]
    fn aurora_logo_svg_loads() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        ctx.include_bytes(ui::brand::LOGO_URI, ui::brand::LOGO_SVG);
        let poll = ctx.try_load_image(ui::brand::LOGO_URI, egui::SizeHint::Size { width: 32, height: 32, maintain_aspect_ratio: true });
        assert!(matches!(poll, Ok(egui::load::ImagePoll::Ready { .. })), "expected Aurora SVG logo to decode, got {:?}", poll.err());
    }
}

#[cfg(test)]
mod deck_launch_tests {
    use super::*;

    #[test]
    fn deck_flags_request_distinct_preview_modes() {
        assert_eq!(big_picture_request(["--steam-deck"]), Some(DECK_PREVIEW));
        assert_eq!(big_picture_request(["--steam-deck=window"]), Some(DECK_PREVIEW_WINDOW));
        assert_eq!(big_picture_request(["--bigpicture=window", "--steam-deck"]), Some(DECK_PREVIEW_WINDOW));
        assert_eq!(big_picture_request(["--bigpicture"]), Some(BIG_PICTURE));
        assert_eq!(big_picture_request(["--steam-deckish"]), None);
        assert!(is_allowed_payload(DECK_PREVIEW));
        assert!(is_allowed_payload(DECK_PREVIEW_WINDOW));
    }
}
