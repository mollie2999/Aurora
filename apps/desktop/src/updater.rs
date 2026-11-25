//! Updates from GitHub Releases, like a launcher: check on start and every few
//! hours, download the new setup in the background, verify its SHA-256, then
//! "Restart to update" (or it installs on the next start, like Steam).
//!
//! Uses GitHub's public API with no token: nothing secret ships in the app.
//! While the repository is private the check finds nothing and stays quiet.

use crate::app::SteamApp;
use egui::{Align, Layout, Ui};
use i18n::t;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use ui::{Theme, icons, widgets as w};

const REPO: &str = "mollie2999/Aurora";
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    setup_url: String,
    setup_name: String,
    sums_url: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, f32),
    Ready(Release, PathBuf, String),
    Failed(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PendingUpdate {
    pub version: String,
    pub path: PathBuf,
    pub sha256: String,
}

fn deserialize_pending<'de, D>(deserializer: D) -> Result<Option<PendingUpdate>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Current(PendingUpdate),
        Tuple3((String, PathBuf, String)),
        LegacyTuple2((serde::de::IgnoredAny, serde::de::IgnoredAny)),
    }
    Ok(match Option::<Repr>::deserialize(deserializer)? {
        Some(Repr::Current(p)) => Some(p),
        Some(Repr::Tuple3((version, path, sha256))) => Some(PendingUpdate { version, path, sha256 }),
        // Legacy entries without an expected SHA-256 are discarded fail-closed.
        Some(Repr::LegacyTuple2(_)) | None => None,
    })
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// Download updates by themselves and install them on restart.
    pub auto: bool,
    /// A version the user chose to skip.
    pub skipped: Option<String>,
    /// Downloaded and verified, waiting for the next start.
    #[serde(default, deserialize_with = "deserialize_pending")]
    pub pending: Option<PendingUpdate>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto: true, skipped: None, pending: None }
    }
}

struct Inner {
    status: Status,
    last_check: Option<Instant>,
    settings: Settings,
    /// The "update ready" window was shown for this version.
    prompted: Option<String>,
    /// The update window is open.
    open: bool,
}

static STATE: LazyLock<Arc<Mutex<Inner>>> = LazyLock::new(|| Arc::new(Mutex::new(Inner { status: Status::Idle, last_check: None, settings: load(), prompted: None, open: false })));

fn settings_path() -> PathBuf {
    client::store::config_dir().join("updater.json")
}

fn updates_dir() -> PathBuf {
    client::store::config_dir().join("updates")
}

pub(crate) fn is_safe_setup_name(name: &str) -> bool {
    if name.len() <= 4
        || name.starts_with('.')
        || name.ends_with(['.', ' '])
        || name != name.trim()
        || name.contains(['/', '\\', ':'])
        || name.contains("..")
        || name.chars().any(char::is_control)
    {
        return false;
    }
    if !name.to_ascii_lowercase().ends_with(".exe") {
        return false;
    }
    let mut comps = std::path::Path::new(name).components();
    matches!((comps.next(), comps.next()), (Some(std::path::Component::Normal(_)), None))
}

fn is_valid_sha256(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

fn ct_eq_hex(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x.to_ascii_lowercase() ^ y.to_ascii_lowercase())) == 0
}

fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(dir)?;
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "directory is a symlink"));
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

fn write_atomic_private(dir: &std::path::Path, file_name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    ensure_private_dir(dir)?;
    let dst = dir.join(file_name);
    if let Ok(meta) = std::fs::symlink_metadata(&dst) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "target is a symlink"));
        }
    }
    let hex = client::ipc::os_random_hex(8);
    let tmp = dir.join(format!(".{file_name}.{}-{hex}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let res = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(data)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(dst)
}

fn stage_installer_in(updates_root: &std::path::Path, setup_name: &str, data: &[u8]) -> Result<PathBuf, String> {
    if !is_safe_setup_name(setup_name) {
        return Err("unsafe installer filename".into());
    }
    if let Some(parent) = updates_root.parent() {
        ensure_private_dir(parent).map_err(|e| e.to_string())?;
    }
    write_atomic_private(updates_root, setup_name, data).map_err(|e| e.to_string())
}

fn verify_staged_setup_in(updates_root: &std::path::Path, setup: &std::path::Path, expected_sha256: &str) -> bool {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    if !is_valid_sha256(expected_sha256) {
        return false;
    }
    let Some(name) = setup.file_name().and_then(|n| n.to_str()) else { return false };
    if !is_safe_setup_name(name) {
        return false;
    }
    let Ok(dir_meta) = std::fs::symlink_metadata(updates_root) else { return false };
    if !dir_meta.is_dir() || dir_meta.file_type().is_symlink() {
        return false;
    }
    let Ok(file_meta) = std::fs::symlink_metadata(setup) else { return false };
    if !file_meta.is_file() || file_meta.file_type().is_symlink() {
        return false;
    }
    let (Ok(canon_root), Some(Ok(canon_parent))) = (updates_root.canonicalize(), setup.parent().map(|p| p.canonicalize())) else {
        return false;
    };
    if canon_parent != canon_root {
        return false;
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let Ok(mut f) = opts.open(setup) else { return false };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return false,
        }
    }
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !ct_eq_hex(&got, expected_sha256) {
        let _ = std::fs::remove_file(setup);
        return false;
    }
    true
}

fn load() -> Settings {
    std::fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(s: &Settings) {
    if let Ok(text) = serde_json::to_string_pretty(s) {
        let _ = write_atomic_private(&client::store::config_dir(), "updater.json", text.as_bytes());
    }
}

/// The folder the running app lives in.
fn install_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(PathBuf::from)
}

/// Installed with the setup (it leaves `uninstall.exe` next to the app).
/// Copies run from a build folder or a portable zip aren't replaced.
pub fn is_installed() -> bool {
    install_dir().is_some_and(|d| d.join("uninstall.exe").exists())
}

/// Parse a version into a comparable tuple `(major, minor, patch, stage, pre_label, num)`.
///
/// - `stage = 0`: prerelease suffixes (`-alpha`, `-beta`, `-rc1`, `-rc.2`, `-test.5`)
///   sort strictly below the final release of the same `major.minor.patch`.
/// - `stage = 1`: final release (`1.0.0`, `num = 0`) and post-release numeric
///   revisions (`0.7.1-1`, `num = 1`), where `1.0.0-rc1 < 1.0.0 < 1.0.0-1`.
fn parse(v: &str) -> (u32, u32, u32, u8, String, u32) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let v = v.split('+').next().unwrap_or(v);
    let (core, suffix) = v.split_once('-').unwrap_or((v, ""));
    let mut n = core.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (maj, min, pat) = (n.next().unwrap_or(0), n.next().unwrap_or(0), n.next().unwrap_or(0));
    if suffix.is_empty() {
        return (maj, min, pat, 1, String::new(), 0);
    }
    if let Ok(rev) = suffix.parse::<u32>() {
        return (maj, min, pat, 1, String::new(), rev);
    }
    let trimmed = suffix.trim_end_matches(|c: char| c.is_ascii_digit());
    let num: u32 = suffix[trimmed.len()..].parse().unwrap_or(0);
    let label = trimmed.trim_end_matches('.').to_ascii_lowercase();
    (maj, min, pat, 0, label, num)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    parse(candidate) > parse(current)
}

/// On start: an update downloaded earlier gets installed now, before the app
/// opens (Steam does the same). Returns true when the app should quit.
pub fn apply_pending_on_start() -> bool {
    let mut s = load();
    let Some(pending) = s.pending.take() else { return false };
    save(&s);
    if !is_installed() || !is_newer(&pending.version, env!("CARGO_PKG_VERSION")) {
        return false;
    }
    run_setup(&pending.path, &pending.sha256)
}

fn run_setup(setup: &std::path::Path, expected_sha256: &str) -> bool {
    let Some(dir) = install_dir() else { return false };
    if !verify_staged_setup_in(&updates_dir(), setup, expected_sha256) {
        tracing::warn!("refusing to run unverified or tampered installer {}", setup.display());
        return false;
    }
    std::process::Command::new(setup).args(["--update", "--dir"]).arg(&dir).spawn().is_ok()
}

/// Called every frame: schedules checks and shows the update window.
/// Built from source (`cargo run`, or anything in a cargo `target` folder):
/// updates come through git, so don't offer installers on our own.
fn is_dev_build() -> bool {
    static IS_DEV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IS_DEV.get_or_init(|| {
        cfg!(debug_assertions)
            || std::env::current_exe().is_ok_and(|p| p.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target")))
    })
}

pub fn tick(app: &mut SteamApp, ctx: &egui::Context) {
    let state = STATE.clone();
    let mut g = state.lock().unwrap();
    // Dev builds only check when asked (Settings > Check for updates).
    let due = !is_dev_build() && g.last_check.is_none_or(|t| t.elapsed() > EVERY);
    if due && !matches!(g.status, Status::Checking | Status::Downloading(..)) {
        g.last_check = Some(Instant::now());
        g.status = Status::Checking;
        drop(g);
        check(ctx.clone(), false);
        g = state.lock().unwrap();
    }
    // Ready: offer a restart once per version.
    if let Status::Ready(r, _, _) = &g.status {
        if g.prompted.as_deref() != Some(&r.version) {
            g.prompted = Some(r.version.clone());
            g.open = true;
        }
    }
    let open = g.open;
    drop(g);
    if open {
        window(app, ctx);
    }
}

/// Look for a new release; with `manual` the result is always shown.
pub fn check(ctx: egui::Context, manual: bool) {
    let state = STATE.clone();
    if let Ok(mut g) = state.lock() {
        g.status = Status::Checking;
    }
    std::thread::spawn(move || {
        let result = block_on(latest());
        let mut g = state.lock().unwrap();
        g.status = match result {
            Ok(Some(r)) if is_newer(&r.version, env!("CARGO_PKG_VERSION")) => {
                let skipped = g.settings.skipped.as_deref() == Some(&r.version) && !manual;
                if skipped {
                    Status::UpToDate
                } else if g.settings.auto && is_installed() {
                    if manual {
                        g.open = true;
                    }
                    drop(g);
                    download(ctx.clone(), r);
                    return;
                } else {
                    if manual || g.prompted.as_deref() != Some(&r.version) {
                        g.prompted = Some(r.version.clone());
                        g.open = true;
                    }
                    Status::Available(r)
                }
            }
            Ok(_) => Status::UpToDate,
            Err(e) => {
                tracing::debug!("update check: {e}");
                if manual { Status::Failed(e) } else { Status::Idle }
            }
        };
        // Asked from the menu: always say how it went, like Steam does.
        if manual {
            g.open = true;
        }
        ctx.request_repaint();
    });
}

fn download(ctx: egui::Context, r: Release) {
    let state = STATE.clone();
    if let Ok(mut g) = state.lock() {
        g.status = Status::Downloading(r.clone(), 0.0);
    }
    std::thread::spawn(move || {
        let (s2, c2, r2) = (state.clone(), ctx.clone(), r.clone());
        let progress = move |f: f32| {
            if let Ok(mut g) = s2.lock() {
                g.status = Status::Downloading(r2.clone(), f);
            }
            c2.request_repaint();
        };
        let result = block_on(fetch_setup(&r, progress));
        let mut g = state.lock().unwrap();
        g.status = match result {
            Ok((path, sha256)) => {
                g.settings.pending = Some(PendingUpdate { version: r.version.clone(), path: path.clone(), sha256: sha256.clone() });
                save(&g.settings);
                Status::Ready(r, path, sha256)
            }
            Err(e) => Status::Failed(e),
        };
        ctx.request_repaint();
    });
}

fn block_on<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime").block_on(f)
}

fn http() -> reqwest::Client {
    static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(concat!("Aurora/", env!("CARGO_PKG_VERSION"), " (updater)"))
            .https_only(true)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default()
    });
    HTTP.clone()
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

async fn latest() -> Result<Option<Release>, String> {
    let resp = http().get(format!("https://api.github.com/repos/{REPO}/releases/latest")).header("Accept", "application/vnd.github+json").send().await.map_err(|e| e.to_string())?;
    // Private repository or no release yet.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let rel: ApiRelease = resp.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    if rel.draft || rel.prerelease {
        return Ok(None);
    }
    // "SteamInRust-Setup…" is the same setup under its name from before the rename.
    let setup_named = |prefix: &str| rel.assets.iter().find(|a| a.name.starts_with(prefix) && is_safe_setup_name(&a.name));
    let Some(setup) = setup_named("Aurora-Setup").or_else(|| setup_named("SteamInRust-Setup")) else { return Ok(None) };
    Ok(Some(Release {
        version: rel.tag_name.trim_start_matches('v').to_string(),
        notes: rel.body.unwrap_or_default(),
        page: rel.html_url,
        setup_url: setup.browser_download_url.clone(),
        setup_name: setup.name.clone(),
        sums_url: rel.assets.iter().find(|a| a.name == "SHA256SUMS.txt").map(|a| a.browser_download_url.clone()),
    }))
}

/// Download the setup into the private updates folder (streaming to disk while
/// hashing) and check it against SHA256SUMS.txt.
async fn fetch_setup(r: &Release, progress: impl Fn(f32)) -> Result<(PathBuf, String), String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    const MAX_INSTALLER_BYTES: usize = 512 * 1024 * 1024;
    if !is_safe_setup_name(&r.setup_name) {
        return Err("unsafe installer filename".into());
    }
    let client = http();
    let sums_url = r.sums_url.as_ref().ok_or_else(|| t!("The release has no checksums; not installing it.").to_string())?;
    if !sums_url.starts_with("https://") || !r.setup_url.starts_with("https://") {
        return Err("refusing non-HTTPS update URL".into());
    }
    let sums = client.get(sums_url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    let want = sums
        .lines()
        .find_map(|l| {
            let (hash, name) = l.split_once(char::is_whitespace)?;
            (name.trim().trim_start_matches('*') == r.setup_name).then(|| hash.to_lowercase())
        })
        .filter(|h| is_valid_sha256(h))
        .ok_or_else(|| t!("The release has no checksums; not installing it.").to_string())?;
    let mut resp = client.get(&r.setup_url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    if resp.content_length().is_some_and(|len| len > MAX_INSTALLER_BYTES as u64) {
        return Err(t!("The download was damaged (checksum mismatch). Try again later.").to_string());
    }
    let total = resp.content_length().unwrap_or(0).max(1);
    let dir = updates_dir();
    if let Some(parent) = dir.parent() {
        ensure_private_dir(parent).map_err(|e| e.to_string())?;
    }
    ensure_private_dir(&dir).map_err(|e| e.to_string())?;
    let dst = dir.join(&r.setup_name);
    if let Ok(meta) = std::fs::symlink_metadata(&dst)
        && meta.file_type().is_symlink()
    {
        return Err("refusing symlink update destination".into());
    }
    let hex = client::ipc::os_random_hex(8);
    let tmp = dir.join(format!(".{}.{}-{hex}.tmp", r.setup_name, std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = file.set_permissions(std::fs::Permissions::from_mode(0o600)) {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(e.to_string());
        }
    }
    let mut hasher = Sha256::new();
    let mut written = 0usize;
    let stream_res: Result<(), String> = async {
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            if written.saturating_add(chunk.len()) > MAX_INSTALLER_BYTES {
                return Err(t!("The download was damaged (checksum mismatch). Try again later.").to_string());
            }
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            hasher.update(&chunk);
            written += chunk.len();
            progress(written as f32 / total as f32);
        }
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = stream_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !ct_eq_hex(&got, &want) {
        let _ = std::fs::remove_file(&tmp);
        return Err(t!("The download was damaged (checksum mismatch). Try again later.").to_string());
    }
    if let Err(e) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok((dst, want))
}

/// Close the app and let the setup replace it, then it starts again.
fn restart_to_update(app: &mut SteamApp, ctx: &egui::Context, setup: &std::path::Path, expected_sha256: &str) {
    if let Ok(mut g) = STATE.lock() {
        g.settings.pending = None;
        save(&g.settings);
    }
    if run_setup(setup, expected_sha256) {
        app.exiting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// The update window: what's new, and install now or later.
fn window(app: &mut SteamApp, ctx: &egui::Context) {
    let t = app.prefs.theme.clone();
    let status = STATE.lock().unwrap().status.clone();
    let (release, ready) = match &status {
        Status::Available(r) | Status::Downloading(r, _) => (r.clone(), None),
        Status::Ready(r, p, sha) => (r.clone(), Some((p.clone(), sha.clone()))),
        Status::Checking | Status::UpToDate | Status::Failed(_) => {
            // The answer to "Check for updates…".
            let mut close = false;
            ui::widgets::modal(ctx, egui::Id::unique("update-check")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.horizontal(|ui| {
                    w::logo(ui, &t, 28.0);
                    ui.label(w::text_bold(t!("Aurora update"), 16.0, t.text));
                });
                ui.add_space(10.0);
                let text = match &status {
                    Status::Checking => t!("Checking for updates…").to_string(),
                    Status::UpToDate => t!("Your Aurora client is already up to date ({v}).", v = env!("CARGO_PKG_VERSION")),
                    Status::Failed(e) => t!("Couldn't check for updates: {error}", error = e),
                    _ => String::new(),
                };
                ui.add(egui::Label::new(w::text(text, 13.5, t.text_dim)).wrap());
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::button(ui, &t, None, t!("OK"), w::Kind::Primary, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            });
            if close {
                STATE.lock().unwrap().open = false;
            }
            return;
        }
        _ => {
            STATE.lock().unwrap().open = false;
            return;
        }
    };
    let mut close = false;
    ui::widgets::modal(ctx, egui::Id::unique("update-window")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            w::logo(ui, &t, 34.0);
            ui.vertical(|ui| {
                ui.label(w::text_bold(t!("Aurora {v} is available", v = release.version), 17.0, t.text));
                ui.label(w::text(t!("You have {v}", v = env!("CARGO_PKG_VERSION")), 12.5, t.text_dim));
            });
        });
        ui.add_space(10.0);
        ui.label(w::text_bold(t!("WHAT'S NEW"), 10.5, t.text_faint));
        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| notes(ui, &t, &release.notes));
        ui.add_space(12.0);
        if let Status::Downloading(_, f) = &status {
            ui.label(w::text(t!("Downloading… {pct}%", pct = (f * 100.0) as u32), 12.5, t.text_dim));
            w::progress(ui, &t, *f, ui.available_width());
            ui.add_space(8.0);
        }
        // Small links on their own row, so long labels (Russian) never overlap.
        ui.horizontal_wrapped(|ui| {
            if ui.link(w::text(t!("Release page"), 12.5, t.accent)).clicked() {
                crate::links::open_http(&release.page);
            }
            if let (None, Status::Available(r)) = (&ready, &status) {
                ui.label(w::text("·", 12.5, t.text_faint));
                if ui.link(w::text(t!("Skip this version"), 12.5, t.text_dim)).clicked() {
                    if let Ok(mut g) = STATE.lock() {
                        g.settings.skipped = Some(r.version.clone());
                        save(&g.settings);
                    }
                    close = true;
                }
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match (&ready, &status) {
                    (Some((setup, sha)), _) => {
                        if w::button(ui, &t, Some(icons::REFRESH), t!("Restart and update"), w::Kind::Primary, w::Size::Md).clicked() {
                            restart_to_update(app, ctx, setup, sha);
                            close = true;
                        }
                        if w::button(ui, &t, None, t!("On next start"), w::Kind::Ghost, w::Size::Md).on_hover_text(t!("It installs the next time you open Aurora")).clicked() {
                            close = true;
                        }
                    }
                    (None, Status::Available(r)) => {
                        if is_installed() {
                            if w::button(ui, &t, Some(icons::DOWNLOAD), t!("Download and install"), w::Kind::Primary, w::Size::Md).clicked() {
                                download(ctx.clone(), r.clone());
                            }
                        } else if w::button(ui, &t, Some(icons::DOWNLOAD), t!("Get the installer"), w::Kind::Primary, w::Size::Md).on_hover_text(t!("This copy wasn't installed with the setup, so it can't replace itself")).clicked() {
                            crate::links::open_http(&r.page);
                        }
                        if w::button(ui, &t, None, t!("Later"), w::Kind::Ghost, w::Size::Md).clicked() {
                            close = true;
                        }
                    }
                    _ => {
                        if w::button(ui, &t, None, t!("Hide"), w::Kind::Ghost, w::Size::Md).clicked() {
                            close = true;
                        }
                    }
                }
            });
        });
    });
    if close {
        STATE.lock().unwrap().open = false;
    }
}

/// Release notes are Markdown; show headings, bullets and text.
fn notes(ui: &mut Ui, t: &Theme, text: &str) {
    if text.trim().is_empty() {
        ui.label(w::text(t!("Bug fixes and improvements."), 13.0, t.text_dim));
        return;
    }
    for line in text.lines() {
        let l = line.trim();
        // The release download table is for visitors to GitHub.
        if l.starts_with('|') && l.ends_with('|') {
            continue;
        }
        if let Some(h) = l.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim();
            // The version heading ("[0.4.0](compare link) (date)"): the title already says it.
            if h.starts_with('[') {
                continue;
            }
            let h = plain(h);
            let h = match h.as_str() {
                "New" | "Features" => t!("New").to_string(),
                "Fixes" | "Bug Fixes" => t!("Fixes").to_string(),
                "Faster" | "Performance Improvements" => t!("Faster").to_string(),
                _ => h,
            };
            ui.add_space(6.0);
            ui.label(w::text_semibold(h, 14.0, t.text));
        } else if let Some(b) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(w::text("•", 13.0, t.accent));
                ui.add(egui::Label::new(w::text(plain(b), 13.0, t.text_dim)).wrap());
            });
        } else if !l.is_empty() {
            ui.add(egui::Label::new(w::text(plain(l), 13.0, t.text_dim)).wrap());
        }
    }
}

/// Release-note Markdown as plain text: "[#17](url)" -> "#17", commit links
/// ("([7562a6d](url))") dropped, bold and code marks removed.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|i| open + i) else { break };
        let Some(end) = rest[close..].find(')').map(|i| close + i) else { break };
        out.push_str(&rest[..open]);
        let text = &rest[open + 1..close];
        // A bare commit hash: leave it out.
        let commit = text.len() >= 7 && text.chars().all(|c| c.is_ascii_hexdigit());
        if !commit {
            out.push_str(text);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    let out = out.replace("**", "").replace('`', "").replace(" ()", "").replace("()", "");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The Updates card in Settings > About.
pub fn settings_ui(ui: &mut Ui, t: &Theme) {
    let state = STATE.clone();
    let (status, mut auto) = {
        let g = state.lock().unwrap();
        (g.status.clone(), g.settings.auto)
    };
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(w::text_semibold(t!("Updates"), 15.0, t.text));
        ui.add_space(4.0);
        let line = match &status {
            Status::Idle => t!("Version {v}", v = env!("CARGO_PKG_VERSION")),
            Status::Checking => t!("Checking for updates…").to_string(),
            Status::UpToDate => t!("You're up to date ({v})", v = env!("CARGO_PKG_VERSION")),
            Status::Available(r) => t!("Version {v} is available", v = r.version),
            Status::Downloading(r, f) => t!("Downloading {v}… {pct}%", v = r.version, pct = (f * 100.0) as u32),
            Status::Ready(r, _, _) => t!("Version {v} is ready to install", v = r.version),
            Status::Failed(e) => t!("Couldn't check for updates: {error}", error = e),
        };
        ui.label(w::text(line, 13.0, t.text_dim));
        if !is_installed() {
            ui.label(w::text(t!("This copy wasn't installed with the setup, so updates are only announced, not installed."), 12.0, t.text_faint));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let busy = matches!(status, Status::Checking | Status::Downloading(..));
            if ui.add_enabled_ui(!busy, |ui| w::button(ui, t, Some(icons::REFRESH), t!("Check for updates"), w::Kind::Secondary, w::Size::Sm)).inner.clicked() {
                check(ui.ctx().clone(), true);
            }
            if matches!(status, Status::Available(_) | Status::Ready(..)) && w::button(ui, t, None, t!("Show"), w::Kind::Ghost, w::Size::Sm).clicked() {
                state.lock().unwrap().open = true;
            }
        });
        ui.add_space(4.0);
        if w::check(ui, t, &mut auto, t!("Download updates automatically and install them on restart")).changed() {
            let mut g = state.lock().unwrap();
            g.settings.auto = auto;
            save(&g.settings);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::plain;

    #[test]
    fn release_notes_become_plain_text() {
        let line = "compact Workshop filters ([#17](https://github.com/x/y/issues/17)) ([7562a6d](https://github.com/x/y/commit/7562a6d30e))";
        assert_eq!(plain(line), "compact Workshop filters (#17)");
        assert_eq!(plain("**bold** and `code`"), "bold and code");
        assert_eq!(plain("no links here"), "no links here");
    }

    #[test]
    fn compares_versions() {
        assert!(super::is_newer("v0.2.0", "0.1.9"));
        assert!(super::is_newer("1.0.0", "0.9.12"));
        assert!(!super::is_newer("v0.1.0", "0.1.0"));
        assert!(!super::is_newer("0.1.0-beta", "0.1.0"));
        assert!(super::is_newer("0.1.10", "0.1.9"));
        assert!(super::is_newer("v0.7.1-1", "0.7.1"));
        assert!(super::is_newer("0.7.2", "0.7.1-1"));
        assert!(!super::is_newer("0.7.1-test.5", "0.7.1"));
        assert!(super::is_newer("1.0.0-rc1", "0.9.2"));
        assert!(super::is_newer("1.0.0-rc2", "1.0.0-rc1"));
        assert!(super::is_newer("1.0.0-rc10", "1.0.0-rc2"));
        assert!(super::is_newer("1.0.0-rc.2", "1.0.0-rc.1"));
        assert!(super::is_newer("1.0.0", "1.0.0-rc1"));
        assert!(super::is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!super::is_newer("1.0.0-rc1", "1.0.0"));
    }

    #[test]
    fn validates_setup_names_private_staging_and_sha256_before_exec() {
        use sha2::{Digest, Sha256};

        assert!(super::is_safe_setup_name("Aurora-Setup-0.9.1.exe"));
        assert!(super::is_safe_setup_name("SteamInRust-Setup.EXE"));
        for bad in [
            "",
            ".exe",
            "..",
            "../Aurora-Setup.exe",
            "..\\Aurora-Setup.exe",
            "sub/Aurora-Setup.exe",
            "sub\\Aurora-Setup.exe",
            "C:Aurora-Setup.exe",
            "Aurora-Setup.bat",
            "Aurora-Setup.exe ",
            " Aurora-Setup.exe",
            "Aurora-Setup.exe.",
            "Aurora\0Setup.exe",
        ] {
            assert!(!super::is_safe_setup_name(bad), "expected unsafe setup_name rejection for {bad:?}");
        }

        // Updates dir lives under config_dir/updates, never std::env::temp_dir().
        assert_eq!(super::updates_dir(), client::store::config_dir().join("updates"));

        let base = std::env::temp_dir().join(format!("sir-upd-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let updates = base.join("cfg").join("updates");

        let payload = b"MZ-fake-aurora-installer-binary";
        let want: String = Sha256::digest(payload).iter().map(|b| format!("{b:02x}")).collect();

        assert!(super::stage_installer_in(&updates, "../evil.exe", payload).is_err());
        let staged = super::stage_installer_in(&updates, "Aurora-Setup-0.9.1.exe", payload).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let d_mode = std::fs::metadata(&updates).unwrap().permissions().mode() & 0o777;
            let f_mode = std::fs::metadata(&staged).unwrap().permissions().mode() & 0o777;
            assert_eq!(d_mode, 0o700, "updates staging dir must be 0700");
            assert_eq!(f_mode, 0o600, "staged setup file must be 0600");
        }

        // Settings JSON round-trips SHA-256 and drops legacy unhashed 2-tuples fail-closed.
        let s = super::Settings {
            auto: true,
            skipped: Some("0.8.0".into()),
            pending: Some(super::PendingUpdate { version: "0.9.1".into(), path: staged.clone(), sha256: want.clone() }),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: super::Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pending.as_ref().unwrap().sha256, want);

        let legacy = r#"{"auto":false,"skipped":"0.8.0","pending":["0.9.1","/tmp/Aurora-Setup.exe"]}"#;
        let legacy_parsed: super::Settings = serde_json::from_str(legacy).unwrap();
        assert!(!legacy_parsed.auto);
        assert_eq!(legacy_parsed.skipped.as_deref(), Some("0.8.0"));
        assert!(legacy_parsed.pending.is_none(), "legacy pending without SHA-256 must be discarded");

        // Pre-spawn verification succeeds on matching hash inside updates_root...
        assert!(super::verify_staged_setup_in(&updates, &staged, &want));

        // ...refuses files outside updates_root even with valid hash...
        let outside = base.join("Aurora-Setup-0.9.1.exe");
        std::fs::write(&outside, payload).unwrap();
        assert!(!super::verify_staged_setup_in(&updates, &outside, &want));

        // ...and detects TOCTOU tampering on disk, deleting the tampered installer!
        std::fs::write(&staged, b"MZ-tampered-by-local-attacker").unwrap();
        assert!(!super::verify_staged_setup_in(&updates, &staged, &want));
        assert!(!staged.exists(), "tampered installer must be unlinked");

        let _ = std::fs::remove_dir_all(&base);
    }
}
