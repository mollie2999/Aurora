//! # User plugins
//!
//! The Aurora answer to SteamBrew/Millennium, without injecting into
//! anything: plugins are folders in the user's plugin directory:
//!
//! ```text
//! plugins/friend-alerts/
//!   plugin.toml   id, name, version, permissions, optional page
//!   main.rhai     the script
//! ```
//!
//! Scripts are [Rhai](https://rhai.rs), a scripting language written in pure
//! Rust. They are sandboxed:
//! - no file, network or process access at all, only the API below;
//! - every API call is checked against the permissions the user approved;
//! - CPU (operation count), recursion, string, array and map sizes are capped.
//!
//! ## Script API
//! | Function | Permission |
//! |---|---|
//! | `log(text)`, `now()` | none |
//! | `notify(text)` | `notifications` |
//! | `me()` | `account.read` |
//! | `friends()` | `friends.read` |
//! | `library()` | `library.read` |
//! | `set_status("online" / "away" / "busy" / "invisible")` | `status.write` |
//! | `send_message(steam_id, text)` | `chat.write` |
//! | `storage_get(key)`, `storage_set(key, value)` | `storage` |
//!
//! ## Hooks (define any of these functions)
//! - `on_load()`
//! - `on_event(name, data)`: `"friend_online"`, `"friend_offline"`, `"friend_game"`
//!   (need `friends.read`), `"message"` (needs `chat.read`), `"game_started"`,
//!   `"game_stopped"`, `"logged_in"`.
//! - `page()`: returns an array of widgets for the plugin's page (needs `[page]` in the manifest).
//! - `on_click(id)`: a button on the page was clicked.
//!
//! Widgets are maps: `#{type: "heading", text: "Hi"}`, `"text"`, `"muted"`,
//! `#{type: "button", id: "go", text: "Go"}`, `#{type: "stat", label, value}`,
//! `"separator"`, `#{type: "row", items: [...]}`, `#{type: "progress", value: 0.5}`,
//! `#{type: "game", app_id: 440}` (a game card).

use rhai::{AST, Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Scope};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    AccountRead,
    FriendsRead,
    LibraryRead,
    ChatRead,
    ChatWrite,
    StatusWrite,
    Notifications,
    Storage,
}

impl Permission {
    pub const ALL: [Permission; 8] = [
        Self::AccountRead,
        Self::FriendsRead,
        Self::LibraryRead,
        Self::ChatRead,
        Self::ChatWrite,
        Self::StatusWrite,
        Self::Notifications,
        Self::Storage,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::AccountRead => "account.read",
            Self::FriendsRead => "friends.read",
            Self::LibraryRead => "library.read",
            Self::ChatRead => "chat.read",
            Self::ChatWrite => "chat.write",
            Self::StatusWrite => "status.write",
            Self::Notifications => "notifications",
            Self::Storage => "storage",
        }
    }
    pub fn describe(self) -> &'static str {
        match self {
            Self::AccountRead => "See your profile name and SteamID",
            Self::FriendsRead => "See your friends and what they're playing",
            Self::LibraryRead => "See your games and playtime",
            Self::ChatRead => "Read incoming chat messages",
            Self::ChatWrite => "Send chat messages as you",
            Self::StatusWrite => "Change your online status",
            Self::Notifications => "Show notifications",
            Self::Storage => "Save its own settings",
        }
    }
    /// Permissions that act on your behalf get a warning in the UI.
    pub fn sensitive(self) -> bool {
        matches!(self, Self::ChatRead | Self::ChatWrite | Self::StatusWrite)
    }
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == s)
    }
}

impl Serialize for Permission {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

impl<'de> Deserialize<'de> for Permission {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown permission '{s}'")))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawManifest {
    id: String,
    name: String,
    #[serde(default = "default_version")]
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default = "default_main")]
    main: String,
    page: Option<PageDecl>,
}

fn default_version() -> String {
    "0.1.0".into()
}
fn default_main() -> String {
    "main.rhai".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct PageDecl {
    pub title: String,
    #[serde(default)]
    pub icon: String,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    pub page: Option<PageDecl>,
    main: String,
}

/// Data the host shares with plugins. Filled by the app each frame a plugin runs.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub me: Option<(u64, String, String)>,
    /// (steam_id, name, status, game)
    pub friends: Vec<(u64, String, String, Option<String>)>,
    /// (app_id, name, installed, playtime minutes)
    pub library: Vec<(u32, String, bool, u64)>,
}

/// Things a plugin asked the host to do.
#[derive(Debug, Clone)]
pub enum Request {
    Notify(String),
    SetStatus(model::PersonaState),
    SendMessage(u64, String),
}

/// Events delivered to `on_event`.
#[derive(Debug, Clone)]
pub enum HostEvent {
    LoggedIn,
    FriendOnline { steam_id: u64, name: String },
    FriendOffline { steam_id: u64, name: String },
    FriendGame { steam_id: u64, name: String, game: Option<String> },
    Message { steam_id: u64, name: String, text: String },
    GameStarted { app_id: u32 },
    GameStopped { app_id: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Widget {
    Heading(String),
    Text(String),
    Muted(String),
    Button { id: String, text: String },
    Stat { label: String, value: String },
    Separator,
    Row(Vec<Widget>),
    Progress(f32),
    Game(u32),
}

const MAX_OUTBOX: usize = 32;
const MAX_WIDGET_DEPTH: usize = 8;

#[derive(Default)]
struct Host {
    granted: HashSet<Permission>,
    snapshot: Snapshot,
    outbox: Vec<Request>,
    hook_outbox: usize,
    storage: BTreeMap<String, String>,
    storage_dirty: bool,
    logs: VecDeque<String>,
}

impl Host {
    fn need(&self, p: Permission) -> Result<(), Box<EvalAltResult>> {
        if self.granted.contains(&p) {
            Ok(())
        } else {
            Err(format!("permission denied: this plugin didn't request '{}'", p.key()).into())
        }
    }
    fn push(&mut self, r: Request) -> Result<(), Box<EvalAltResult>> {
        if self.hook_outbox >= MAX_OUTBOX || self.outbox.len() >= MAX_OUTBOX {
            return Err(format!("rate limit exceeded: max {MAX_OUTBOX} actions per hook").into());
        }
        self.hook_outbox += 1;
        self.outbox.push(r);
        Ok(())
    }
    fn log(&mut self, s: String) {
        self.logs.push_back(s);
        while self.logs.len() > 200 {
            self.logs.pop_front();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("plugin.toml: {0}")]
    Manifest(String),
    #[error("script error: {0}")]
    Script(String),
}

pub struct Plugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
    engine: Engine,
    ast: Option<AST>,
    scope: Scope<'static>,
    host: Arc<Mutex<Host>>,
    running: bool,
    /// Last error (compile or runtime), shown in the plugin manager.
    pub error: Option<String>,
}

const MAX_OPERATIONS: u64 = 2_000_000;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_SCRIPT_BYTES: u64 = 512 * 1024;

fn read_bounded_string(path: &Path, max_bytes: u64, what: &str) -> Result<String, Error> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
    let mut buf = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| Error::Io(path.to_path_buf(), e))?;
    if buf.len() as u64 > max_bytes {
        return Err(Error::Manifest(format!("{what} exceeds {} KiB limit", max_bytes / 1024)));
    }
    String::from_utf8(buf).map_err(|e| Error::Manifest(format!("{what} is not valid UTF-8: {e}")))
}

fn resolve_main_path(dir: &Path, main: &str) -> Result<(PathBuf, PathBuf), Error> {
    if main.is_empty() || main.contains('\0') || main.contains(':') || main.starts_with('/') || main.starts_with('\\') {
        return Err(Error::Manifest("invalid main script path".into()));
    }
    let mut parts = Vec::new();
    for seg in main.split(['/', '\\']) {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(Error::Manifest("invalid main script path".into()));
        }
        parts.push(seg);
    }
    for comp in Path::new(main).components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return Err(Error::Manifest("invalid main script path".into()));
        }
    }
    let canon_dir = std::fs::canonicalize(dir).map_err(|e| Error::Io(dir.to_path_buf(), e))?;
    let mut cur = canon_dir.clone();
    for seg in &parts {
        cur.push(seg);
        let meta = std::fs::symlink_metadata(&cur).map_err(|e| Error::Io(cur.clone(), e))?;
        if meta.file_type().is_symlink() {
            return Err(Error::Manifest("main script path must not use symlinks".into()));
        }
    }
    let canon_file = std::fs::canonicalize(&cur).map_err(|e| Error::Io(cur, e))?;
    if !canon_file.starts_with(&canon_dir) || canon_file == canon_dir || !canon_file.is_file() {
        return Err(Error::Manifest("main script must be a file inside plugin folder".into()));
    }
    Ok((canon_dir, canon_file))
}

impl Plugin {
    pub fn load(dir: &Path) -> Result<Plugin, Error> {
        let manifest_path = dir.join("plugin.toml");
        let text = read_bounded_string(&manifest_path, MAX_MANIFEST_BYTES, "plugin.toml")?;
        let raw: RawManifest = toml::from_str(&text).map_err(|e| Error::Manifest(e.to_string()))?;
        if raw.id.is_empty() || !raw.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(Error::Manifest("id must be letters, digits, '-' or '_'".into()));
        }
        let (canon_dir, _) = resolve_main_path(dir, &raw.main)?;
        let mut permissions = Vec::new();
        for p in &raw.permissions {
            permissions.push(Permission::parse(p).ok_or_else(|| Error::Manifest(format!("unknown permission '{p}'")))?);
        }
        permissions.sort();
        permissions.dedup();
        let manifest = Manifest {
            id: raw.id,
            name: raw.name,
            version: raw.version,
            author: raw.author,
            description: raw.description,
            permissions,
            page: raw.page,
            main: raw.main,
        };
        let host = Arc::new(Mutex::new(Host::default()));
        let engine = sandboxed_engine(host.clone());
        Ok(Plugin { manifest, dir: canon_dir, engine, ast: None, scope: Scope::new(), host, running: false, error: None })
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Compile and run the script with the given approved permissions.
    /// `storage` is the plugin's saved data.
    pub fn start(&mut self, approved: &BTreeSet<Permission>, storage: BTreeMap<String, String>) {
        self.stop();
        if let Some(unapproved) = self.manifest.permissions.iter().find(|p| !approved.contains(p)) {
            self.error = Some(format!("permission '{}' has not been approved", unapproved.key()));
            return;
        }
        let (_, path) = match resolve_main_path(&self.dir, &self.manifest.main) {
            Ok(res) => res,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let src = match read_bounded_string(&path, MAX_SCRIPT_BYTES, "plugin script") {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        {
            let mut h = self.host.lock().unwrap();
            h.granted = self.manifest.permissions.iter().copied().filter(|p| approved.contains(p)).collect();
            h.storage = storage;
            h.storage_dirty = false;
            h.hook_outbox = 0;
        }
        match self.engine.compile(&src) {
            Ok(ast) => {
                self.scope = Scope::new();
                if let Err(e) = self.engine.run_ast_with_scope(&mut self.scope, &ast) {
                    self.error = Some(e.to_string());
                    self.host.lock().unwrap().granted.clear();
                    return;
                }
                self.ast = Some(ast);
                self.running = true;
                self.error = None;
                self.call("on_load", ());
            }
            Err(e) => {
                self.host.lock().unwrap().granted.clear();
                self.error = Some(format!("{e}"));
            }
        }
    }

    pub fn stop(&mut self) {
        self.running = false;
        self.ast = None;
        let mut h = self.host.lock().unwrap();
        h.granted.clear();
        h.outbox.clear();
        h.hook_outbox = 0;
    }

    fn has_fn(&self, name: &str) -> bool {
        self.ast.as_ref().is_some_and(|a| a.iter_functions().any(|f| f.name == name))
    }

    fn call(&mut self, name: &str, args: impl rhai::FuncArgs) -> Option<Dynamic> {
        if !self.running || !self.has_fn(name) {
            return None;
        }
        self.host.lock().unwrap().hook_outbox = 0;
        let ast = self.ast.as_ref()?;
        match self.engine.call_fn::<Dynamic>(&mut self.scope, ast, name, args) {
            Ok(v) => Some(v),
            Err(e) => {
                let msg = format!("{name}(): {e}");
                self.host.lock().unwrap().log(format!("error: {msg}"));
                self.error = Some(msg);
                None
            }
        }
    }

    pub fn set_snapshot(&self, s: Snapshot) {
        self.host.lock().unwrap().snapshot = s;
    }

    pub fn event(&mut self, e: &HostEvent) {
        let granted = self.host.lock().unwrap().granted.clone();
        let friends = granted.contains(&Permission::FriendsRead);
        let (name, data): (&str, Map) = match e {
            HostEvent::LoggedIn => ("logged_in", Map::new()),
            HostEvent::GameStarted { app_id } => ("game_started", map([("app_id", (*app_id as i64).into())])),
            HostEvent::GameStopped { app_id } => ("game_stopped", map([("app_id", (*app_id as i64).into())])),
            HostEvent::FriendOnline { steam_id, name } if friends => {
                ("friend_online", map([("steam_id", steam_id.to_string().into()), ("name", name.clone().into())]))
            }
            HostEvent::FriendOffline { steam_id, name } if friends => {
                ("friend_offline", map([("steam_id", steam_id.to_string().into()), ("name", name.clone().into())]))
            }
            HostEvent::FriendGame { steam_id, name, game } if friends => (
                "friend_game",
                map([
                    ("steam_id", steam_id.to_string().into()),
                    ("name", name.clone().into()),
                    ("game", game.clone().map(Dynamic::from).unwrap_or(Dynamic::UNIT)),
                ]),
            ),
            HostEvent::Message { steam_id, name, text } if granted.contains(&Permission::ChatRead) => (
                "message",
                map([("steam_id", steam_id.to_string().into()), ("name", name.clone().into()), ("text", text.clone().into())]),
            ),
            _ => return,
        };
        self.call("on_event", (ImmutableString::from(name), data));
    }

    pub fn has_page(&self) -> bool {
        self.manifest.page.is_some()
    }

    pub fn page(&mut self) -> Vec<Widget> {
        self.call("page", ()).map(|d| widgets(d)).unwrap_or_default()
    }

    pub fn click(&mut self, id: &str) {
        self.call("on_click", (ImmutableString::from(id),));
    }

    /// Take the requests made since the last call.
    pub fn drain(&self) -> Vec<Request> {
        std::mem::take(&mut self.host.lock().unwrap().outbox)
    }

    pub fn logs(&self) -> Vec<String> {
        self.host.lock().unwrap().logs.iter().cloned().collect()
    }

    /// Storage to persist, if it changed.
    pub fn take_storage(&self) -> Option<BTreeMap<String, String>> {
        let mut h = self.host.lock().unwrap();
        if h.storage_dirty {
            h.storage_dirty = false;
            Some(h.storage.clone())
        } else {
            None
        }
    }
}

fn map<const N: usize>(pairs: [(&str, Dynamic); N]) -> Map {
    pairs.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

fn widgets(d: Dynamic) -> Vec<Widget> {
    let mut budget = 500usize;
    widgets_bounded(d, 0, &mut budget)
}

fn widgets_bounded(d: Dynamic, depth: usize, budget: &mut usize) -> Vec<Widget> {
    let Some(arr) = d.try_cast::<Array>() else { return Vec::new() };
    let mut out = Vec::new();
    for item in arr {
        if *budget == 0 {
            break;
        }
        if let Some(w) = widget(item, depth, budget) {
            out.push(w);
        }
    }
    out
}

fn widget(d: Dynamic, depth: usize, budget: &mut usize) -> Option<Widget> {
    if *budget == 0 {
        return None;
    }
    let m = d.try_cast::<Map>()?;
    let s = |k: &str| m.get(k).map(|v| v.to_string()).unwrap_or_default();
    let kind = s("type");
    if kind == "row" && depth >= MAX_WIDGET_DEPTH {
        return None;
    }
    *budget -= 1;
    Some(match kind.as_str() {
        "heading" => Widget::Heading(s("text")),
        "text" => Widget::Text(s("text")),
        "muted" => Widget::Muted(s("text")),
        "button" => Widget::Button { id: s("id"), text: s("text") },
        "stat" => Widget::Stat { label: s("label"), value: s("value") },
        "separator" => Widget::Separator,
        "progress" => Widget::Progress(m.get("value").and_then(|v| v.as_float().ok()).unwrap_or(0.0).clamp(0.0, 1.0) as f32),
        "game" => Widget::Game(m.get("app_id").and_then(|v| v.as_int().ok()).unwrap_or(0) as u32),
        "row" => Widget::Row(m.get("items").cloned().map(|v| widgets_bounded(v, depth + 1, budget)).unwrap_or_default()),
        _ => {
            *budget += 1;
            return None;
        }
    })
}

fn sandboxed_engine(host: Arc<Mutex<Host>>) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(MAX_OPERATIONS);
    e.set_max_call_levels(32);
    e.set_max_expr_depths(64, 32);
    e.set_max_string_size(64 * 1024);
    e.set_max_array_size(20_000);
    e.set_max_map_size(5_000);
    e.disable_symbol("eval");
    e.disable_symbol("import");

    let h = host.clone();
    e.on_print(move |s| h.lock().unwrap().log(s.to_string()));
    let h = host.clone();
    e.on_debug(move |s, _, _| h.lock().unwrap().log(s.to_string()));

    let h = host.clone();
    e.register_fn("log", move |s: &str| h.lock().unwrap().log(s.to_string()));
    e.register_fn("now", || {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    });

    let h = host.clone();
    e.register_fn("notify", move |s: &str| -> Result<(), Box<EvalAltResult>> {
        let mut h = h.lock().unwrap();
        h.need(Permission::Notifications)?;
        h.push(Request::Notify(s.chars().take(300).collect()))
    });

    let h = host.clone();
    e.register_fn("me", move || -> Result<Dynamic, Box<EvalAltResult>> {
        let h = h.lock().unwrap();
        h.need(Permission::AccountRead)?;
        Ok(match &h.snapshot.me {
            Some((id, name, status)) => {
                map([("steam_id", id.to_string().into()), ("name", name.clone().into()), ("status", status.clone().into())]).into()
            }
            None => Dynamic::UNIT,
        })
    });

    let h = host.clone();
    e.register_fn("friends", move || -> Result<Array, Box<EvalAltResult>> {
        let h = h.lock().unwrap();
        h.need(Permission::FriendsRead)?;
        Ok(h.snapshot
            .friends
            .iter()
            .map(|(id, name, status, game)| {
                map([
                    ("steam_id", id.to_string().into()),
                    ("name", name.clone().into()),
                    ("status", status.clone().into()),
                    ("game", game.clone().map(Dynamic::from).unwrap_or(Dynamic::UNIT)),
                ])
                .into()
            })
            .collect())
    });

    let h = host.clone();
    e.register_fn("library", move || -> Result<Array, Box<EvalAltResult>> {
        let h = h.lock().unwrap();
        h.need(Permission::LibraryRead)?;
        Ok(h.snapshot
            .library
            .iter()
            .map(|(id, name, installed, minutes)| {
                map([
                    ("app_id", (*id as i64).into()),
                    ("name", name.clone().into()),
                    ("installed", (*installed).into()),
                    ("playtime_minutes", (*minutes as i64).into()),
                ])
                .into()
            })
            .collect())
    });

    let h = host.clone();
    e.register_fn("set_status", move |s: &str| -> Result<(), Box<EvalAltResult>> {
        let mut h = h.lock().unwrap();
        h.need(Permission::StatusWrite)?;
        let st = match s {
            "online" => model::PersonaState::Online,
            "away" => model::PersonaState::Away,
            "busy" => model::PersonaState::Busy,
            "invisible" => model::PersonaState::Invisible,
            other => return Err(format!("unknown status '{other}'").into()),
        };
        h.push(Request::SetStatus(st))
    });

    let h = host.clone();
    e.register_fn("send_message", move |to: &str, text: &str| -> Result<(), Box<EvalAltResult>> {
        let mut h = h.lock().unwrap();
        h.need(Permission::ChatWrite)?;
        let id: u64 = to.parse().map_err(|_| "steam_id must be a numeric string")?;
        if !h.snapshot.friends.iter().any(|f| f.0 == id) {
            return Err("can only message people on your friends list".into());
        }
        h.push(Request::SendMessage(id, text.chars().take(2000).collect()))
    });

    let h = host.clone();
    e.register_fn("storage_get", move |k: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        let h = h.lock().unwrap();
        h.need(Permission::Storage)?;
        Ok(h.storage.get(k).cloned().map(Dynamic::from).unwrap_or(Dynamic::UNIT))
    });

    let h = host;
    e.register_fn("storage_set", move |k: &str, v: Dynamic| -> Result<(), Box<EvalAltResult>> {
        let mut h = h.lock().unwrap();
        h.need(Permission::Storage)?;
        if h.storage.len() >= 200 && !h.storage.contains_key(k) {
            return Err("storage is limited to 200 keys".into());
        }
        h.storage.insert(k.chars().take(100).collect(), v.to_string().chars().take(16 * 1024).collect());
        h.storage_dirty = true;
        Ok(())
    });

    e
}

/// Load every plugin folder in `dir`. Broken plugins are returned as errors
/// so the manager can show them.
pub fn discover(dir: &Path) -> Vec<Result<Plugin, (PathBuf, Error)>> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let loaded: Vec<(PathBuf, Result<Plugin, Error>)> = entries
        .flatten()
        .filter(|e| e.path().join("plugin.toml").is_file())
        .map(|e| {
            let p = e.path();
            let res = Plugin::load(&p);
            (p, res)
        })
        .collect();
    let mut id_counts: HashMap<String, usize> = HashMap::new();
    for (_, res) in &loaded {
        if let Ok(p) = res {
            *id_counts.entry(p.manifest.id.clone()).or_insert(0) += 1;
        }
    }
    let mut out: Vec<Result<Plugin, (PathBuf, Error)>> = loaded
        .into_iter()
        .map(|(path, res)| match res {
            Ok(p) if id_counts.get(&p.manifest.id).copied().unwrap_or(0) > 1 => {
                Err((path, Error::Manifest(format!("duplicate plugin id '{}'", p.manifest.id))))
            }
            Ok(p) => Ok(p),
            Err(err) => Err((path, err)),
        })
        .collect();
    out.sort_by_key(|r| match r {
        Ok(p) => (0u8, p.manifest.name.to_lowercase()),
        Err((p, _)) => (1u8, p.display().to_string()),
    });
    out
}

/// Example plugins bundled with the app, installed on first run.
pub const EXAMPLES: &[(&str, &str, &str)] = &[
    ("friend-alerts", include_str!("../examples/friend-alerts/plugin.toml"), include_str!("../examples/friend-alerts/main.rhai")),
    ("backlog", include_str!("../examples/backlog/plugin.toml"), include_str!("../examples/backlog/main.rhai")),
];

/// Copy the bundled examples into `dir` if they aren't there yet.
pub fn install_examples(dir: &Path) {
    for (id, manifest, script) in EXAMPLES {
        let d = dir.join(id);
        if d.exists() {
            continue;
        }
        if std::fs::create_dir_all(&d).is_ok() {
            let _ = std::fs::write(d.join("plugin.toml"), manifest);
            let _ = std::fs::write(d.join("main.rhai"), script);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(toml: &str, script: &str) -> Plugin {
        let dir = std::env::temp_dir().join(format!("sir-plugin-test-{}", rand_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), toml).unwrap();
        std::fs::write(dir.join("main.rhai"), script).unwrap();
        Plugin::load(&dir).unwrap()
    }

    /// Unique per test, even when tests run at the same instant.
    fn rand_id() -> u128 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        now * 1000 + n + (std::process::id() as u128) * 1_000_000_000
    }

    #[test]
    fn permission_is_enforced() {
        let mut p = plugin("id = \"t\"\nname = \"T\"\n", "fn on_load() { notify(\"hi\"); }");
        p.start(&BTreeSet::new(), Default::default());
        assert!(p.drain().is_empty());
        assert!(p.error.as_deref().unwrap_or("").contains("permission denied"));
    }

    #[test]
    fn granted_permission_works_and_page_renders() {
        let mut p = plugin(
            "id = \"t2\"\nname = \"T\"\npermissions = [\"notifications\"]\n[page]\ntitle = \"T\"\n",
            "fn on_load() { notify(\"hi\"); }\nfn page() { [#{type: \"heading\", text: \"Hello\"}, #{type: \"button\", id: \"b\", text: \"Go\"}] }",
        );
        p.start(&BTreeSet::from([Permission::Notifications]), Default::default());
        assert!(matches!(p.drain().as_slice(), [Request::Notify(s)] if s == "hi"));
        assert_eq!(p.page(), vec![Widget::Heading("Hello".into()), Widget::Button { id: "b".into(), text: "Go".into() }]);
    }

    #[test]
    fn runaway_script_is_stopped() {
        let mut p = plugin("id = \"t3\"\nname = \"T\"\n", "fn on_load() { loop { } }");
        p.start(&BTreeSet::new(), Default::default());
        assert!(p.error.is_some());
    }

    #[test]
    fn examples_parse() {
        for (id, manifest, script) in EXAMPLES {
            let mut p = plugin(manifest, script);
            let approved: BTreeSet<Permission> = p.manifest.permissions.iter().copied().collect();
            p.start(&approved, Default::default());
            assert!(p.error.is_none(), "{id}: {:?}", p.error);
        }
    }

    #[test]
    fn unapproved_manifest_permissions_refuse_to_start() {
        let mut p = plugin(
            "id = \"esc\"\nname = \"Esc\"\npermissions = [\"notifications\", \"chat.write\"]\n",
            "fn on_load() { notify(\"hi\"); }",
        );
        p.start(&BTreeSet::from([Permission::Notifications]), Default::default());
        assert!(!p.is_running(), "plugin must not start with unapproved permission");
        assert!(p.drain().is_empty());
        assert!(p.error.as_deref().unwrap_or("").contains("chat.write"));

        // Superset approval never grants undeclared permissions either.
        let mut p2 = plugin("id = \"sub\"\nname = \"Sub\"\n", "fn on_load() { notify(\"hi\"); }");
        p2.start(&BTreeSet::from([Permission::Notifications]), Default::default());
        assert!(p2.drain().is_empty());
        assert!(p2.error.as_deref().unwrap_or("").contains("permission denied"));
    }

    #[test]
    fn import_is_rejected() {
        let dir = std::env::temp_dir().join(format!("sir-plugin-import-{}", rand_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let outside = std::env::temp_dir().join(format!("sir-outside-{}", rand_id()));
        std::fs::write(outside.with_extension("rhai"), "fn pwn() { 42 }").unwrap();
        let outside_stem = outside.display().to_string();

        // Static import
        let mut p1 = plugin("id = \"imp1\"\nname = \"Imp\"\n", &format!("import \"{outside_stem}\" as m;\nfn on_load() {{ log(\"\" + m::pwn()); }}"));
        p1.start(&BTreeSet::new(), Default::default());
        assert!(p1.error.is_some(), "static import should fail");
        assert!(!p1.logs().iter().any(|l| l == "42"), "imported code must not run");

        // Dynamic import
        let mut p2 = plugin(
            "id = \"imp2\"\nname = \"Imp2\"\n",
            &format!("fn on_load() {{ let target = \"{outside_stem}\"; import target as m; log(\"\" + m::pwn()); }}"),
        );
        p2.start(&BTreeSet::new(), Default::default());
        assert!(p2.error.is_some(), "dynamic import should fail");
        assert!(!p2.logs().iter().any(|l| l == "42"), "dynamically imported code must not run");
    }

    #[test]
    fn duplicate_manifest_ids_are_rejected() {
        let root = std::env::temp_dir().join(format!("sir-plugin-dup-{}", rand_id()));
        let a = root.join("aaa-evil");
        let b = root.join("zzz-legit");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("plugin.toml"), "id = \"same\"\nname = \"AAA\"\n").unwrap();
        std::fs::write(a.join("main.rhai"), "").unwrap();
        std::fs::write(b.join("plugin.toml"), "id = \"same\"\nname = \"ZZZ\"\n").unwrap();
        std::fs::write(b.join("main.rhai"), "").unwrap();

        let found = discover(&root);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|r| r.is_err()), "colliding plugin IDs must all fail");
    }

    #[test]
    fn manifest_main_traversal_and_symlink_are_rejected() {
        let base = std::env::temp_dir().join(format!("sir-plugin-main-{}", rand_id()));
        let pdir = base.join("plug");
        std::fs::create_dir_all(&pdir).unwrap();
        let secret = base.join("secret.rhai");
        std::fs::write(&secret, "fn on_load() { log(\"escaped\"); }").unwrap();

        // Relative .. traversal
        std::fs::write(pdir.join("plugin.toml"), "id = \"p\"\nname = \"P\"\nmain = \"../secret.rhai\"\n").unwrap();
        std::fs::write(pdir.join("main.rhai"), "").unwrap();
        assert!(Plugin::load(&pdir).is_err(), "../ traversal must fail");

        // Windows backslash traversal
        std::fs::write(pdir.join("plugin.toml"), "id = \"p\"\nname = \"P\"\nmain = \"..\\\\secret.rhai\"\n").unwrap();
        assert!(Plugin::load(&pdir).is_err(), "..\\ traversal must fail");

        // Absolute path
        std::fs::write(pdir.join("plugin.toml"), format!("id = \"p\"\nname = \"P\"\nmain = \"{}\"\n", secret.display().to_string().replace('\\', "/"))).unwrap();
        assert!(Plugin::load(&pdir).is_err(), "absolute main must fail");

        // Symlink escape (at load and TOCTOU before start)
        #[cfg(unix)]
        {
            let sdir = base.join("symplug");
            std::fs::create_dir_all(&sdir).unwrap();
            std::fs::write(sdir.join("plugin.toml"), "id = \"s\"\nname = \"S\"\n").unwrap();
            std::os::unix::fs::symlink(&secret, sdir.join("main.rhai")).unwrap();
            assert!(Plugin::load(&sdir).is_err(), "symlink main.rhai must fail");

            let tdir = base.join("toctou");
            std::fs::create_dir_all(&tdir).unwrap();
            std::fs::write(tdir.join("plugin.toml"), "id = \"tc\"\nname = \"TC\"\n").unwrap();
            std::fs::write(tdir.join("main.rhai"), "fn on_load() { log(\"ok\"); }").unwrap();
            let mut tp = Plugin::load(&tdir).unwrap();
            std::fs::remove_file(tp.dir.join("main.rhai")).unwrap();
            std::os::unix::fs::symlink(&secret, tp.dir.join("main.rhai")).unwrap();
            tp.start(&BTreeSet::new(), Default::default());
            assert!(!tp.is_running(), "TOCTOU symlink swap must fail");
            assert!(!tp.logs().iter().any(|l| l == "escaped"));
        }
    }

    #[test]
    fn widget_row_depth_is_capped_at_8() {
        let mut p = plugin(
            "id = \"rowdepth\"\nname = \"R\"\n[page]\ntitle = \"R\"\n",
            "fn page() { let w = #{type: \"text\", text: \"leaf\"}; for i in 0..12 { w = #{type: \"row\", items: [w]}; } [w] }",
        );
        p.start(&BTreeSet::new(), Default::default());
        fn max_row_depth(ws: &[Widget]) -> usize {
            ws.iter()
                .map(|w| match w {
                    Widget::Row(inner) => 1 + max_row_depth(inner),
                    _ => 0,
                })
                .max()
                .unwrap_or(0)
        }
        let d = max_row_depth(&p.page());
        assert!(d <= 8, "row depth {d} exceeded 8");
        assert_eq!(d, 8);
    }

    #[test]
    fn outbox_is_capped_at_32_per_hook() {
        let mut p = plugin(
            "id = \"spam\"\nname = \"S\"\npermissions = [\"notifications\"]\n",
            "fn on_load() { for i in 0..100 { notify(\"x\"); } }",
        );
        p.start(&BTreeSet::from([Permission::Notifications]), Default::default());
        assert_eq!(p.drain().len(), 32, "outbox must cap at 32 items");
    }
}
