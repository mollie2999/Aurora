//! The app's end of the in-game overlay pipe (see
//! `crates/steamclient/src/overlay`).
//!
//! A game whose frames go through our steamclient DLL's graphics hook
//! (D3D11, D3D12, D3D9, OpenGL, or Vulkan) connects to `<ipc pipe>-overlay`
//! with its launch token (on Linux, where Proton games can't reach a Linux
//! named pipe, to a loopback TCP port instead). Each accepted connection
//! becomes a [`Conn`] the UI picks up with [`take_new`]: it runs the overlay
//! UI for that game and sends frames back with [`Conn::send`].

#[path = "../../steamclient/src/overlay/wire.rs"]
pub mod wire;

#[cfg(all(test, not(windows)))]
#[path = "../../steamclient/src/overlay/util.rs"]
mod util;

#[cfg(all(test, not(windows)))]
#[path = "../../steamclient/src/overlay/vulkan.rs"]
mod overlay_vulkan;

use model::AppId;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use wire::{FromGame, ToGame};

const MAX_RELIABLE_QUEUE: usize = 64;
const MAX_OUTBOUND_BYTES: usize = 64 * 1024 * 1024;

struct OutboundState {
    reliable: VecDeque<Vec<u8>>,
    reliable_bytes: usize,
    latest_frame: Option<Vec<u8>>,
    closed: bool,
}

struct Outbound {
    state: Mutex<OutboundState>,
    notify: tokio::sync::Notify,
    closed_notify: tokio::sync::Notify,
}

impl Outbound {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(OutboundState {
                reliable: VecDeque::new(),
                reliable_bytes: 0,
                latest_frame: None,
                closed: false,
            }),
            notify: tokio::sync::Notify::new(),
            closed_notify: tokio::sync::Notify::new(),
        })
    }

    fn push_frame_only(&self, body: Vec<u8>) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.closed {
            return false;
        }
        s.latest_frame = Some(body);
        drop(s);
        self.notify.notify_one();
        true
    }

    fn push_reliable(&self, body: Vec<u8>, supersedes_frame: bool) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.closed {
            return false;
        }
        let next_bytes = s.reliable_bytes.saturating_add(body.len());
        // Allow a single wire-valid message on an empty queue even if large;
        // enforce queue depth and cumulative byte cap when backlog builds up.
        if s.reliable.len() >= MAX_RELIABLE_QUEUE || (!s.reliable.is_empty() && next_bytes > MAX_OUTBOUND_BYTES) {
            // Game stalled and stopped draining the overlay pipe: close both
            // directions cleanly so the game sees EOF and can reconnect instead
            // of growing memory unboundedly or dropping texture deltas.
            s.closed = true;
            s.reliable.clear();
            s.reliable_bytes = 0;
            s.latest_frame = None;
            drop(s);
            self.notify.notify_waiters();
            self.closed_notify.notify_waiters();
            return false;
        }
        if supersedes_frame {
            s.latest_frame = None;
        }
        s.reliable_bytes = next_bytes;
        s.reliable.push_back(body);
        drop(s);
        self.notify.notify_one();
        true
    }

    fn close(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.closed = true;
        s.reliable.clear();
        s.reliable_bytes = 0;
        s.latest_frame = None;
        drop(s);
        self.notify.notify_waiters();
        self.closed_notify.notify_waiters();
    }

    fn is_closed(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed
    }

    async fn wait_closed(&self) {
        loop {
            let notified = self.closed_notify.notified();
            if self.is_closed() {
                return;
            }
            notified.await;
        }
    }

    async fn recv(&self) -> Option<Vec<u8>> {
        loop {
            let notified = self.notify.notified();
            {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(body) = s.reliable.pop_front() {
                    s.reliable_bytes = s.reliable_bytes.saturating_sub(body.len());
                    return Some(body);
                }
                if let Some(body) = s.latest_frame.take() {
                    return Some(body);
                }
                if s.closed {
                    return None;
                }
            }
            notified.await;
        }
    }
}

/// One game process drawing the overlay itself.
pub struct Conn {
    pub app_id: AppId,
    pub pid: u32,
    /// Back buffer size from the game's hello.
    pub size: (u32, u32),
    pub from_game: std::sync::mpsc::Receiver<FromGame>,
    to_game: Arc<Outbound>,
}

impl Conn {
    /// Queue a message; false once the game is gone.
    /// Mesh-only frames coalesce into a single latest-frame slot so a stalled
    /// game never causes unbounded memory growth.
    pub fn send(&self, m: &ToGame) -> bool {
        let body = m.encode();
        match m {
            ToGame::Frame { set, free, .. } if set.is_empty() && free.is_empty() => {
                self.to_game.push_frame_only(body)
            }
            // Texture-bearing frames and closing the overlay supersede any older
            // queued mesh-only frame so a stale frame never renders after close.
            ToGame::Frame { .. } | ToGame::SetOpen(false) => self.to_game.push_reliable(body, true),
            _ => self.to_game.push_reliable(body, false),
        }
    }

    /// Queue an already encoded message.
    pub fn send_encoded(&self, body: Vec<u8>) -> bool {
        self.to_game.push_reliable(body, false)
    }

    pub fn is_closed(&self) -> bool {
        self.to_game.is_closed()
    }
}

static NEW: Mutex<Vec<Conn>> = Mutex::new(Vec::new());
type Waker = Arc<dyn Fn() + Send + Sync>;
static WAKER: OnceLock<Waker> = OnceLock::new();

/// Called whenever a game connects or sends input (the UI repaints).
pub fn set_waker(f: impl Fn() + Send + Sync + 'static) {
    let _ = WAKER.set(Arc::new(f));
}

fn wake() {
    if let Some(w) = WAKER.get() {
        w();
    }
}

/// Connections made since the last call.
pub fn take_new() -> Vec<Conn> {
    std::mem::take(&mut *NEW.lock().unwrap())
}

/// A game connected and nobody picked it up yet.
pub fn has_new() -> bool {
    !NEW.lock().unwrap().is_empty()
}

/// Accept overlay connections on `<pipe>-overlay`. `app_for` checks a
/// launch token (the same tokens as the steamclient pipe).
#[cfg(windows)]
pub async fn accept(pipe: String, app_for: impl Fn(&str) -> Option<AppId> + Send + Sync + 'static) {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = wire::pipe_name(&pipe);
    let app_for: Arc<dyn Fn(&str) -> Option<AppId> + Send + Sync> = Arc::new(app_for);
    let mut first = true;
    loop {
        let server = match ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create(&name) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("overlay pipe {name}: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        first = false;
        if server.connect().await.is_err() {
            continue;
        }
        tokio::spawn(serve(server, app_for.clone()));
    }
}

/// Accept overlay connections from Proton games on a loopback socket.
#[cfg(not(windows))]
pub async fn accept_tcp(listener: tokio::net::TcpListener, app_for: impl Fn(&str) -> Option<AppId> + Send + Sync + 'static) {
    let app_for: Arc<dyn Fn(&str) -> Option<AppId> + Send + Sync> = Arc::new(app_for);
    loop {
        match listener.accept().await {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                let _ = stream.set_nodelay(true);
                tokio::spawn(serve(stream, app_for.clone()));
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("overlay socket: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

const MAX_HELLO_BYTES: usize = 512;
const MAX_FROM_GAME_BYTES: usize = 64 * 1024;

async fn read_message(r: &mut (impl tokio::io::AsyncRead + Unpin), max_bytes: usize) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let n = r.read_u32_le().await? as usize;
    if n == 0 || n > max_bytes {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad overlay message"));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await?;
    Ok(body)
}

async fn serve<S>(pipe: S, app_for: Arc<dyn Fn(&str) -> Option<AppId> + Send + Sync>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
{
    use tokio::io::AsyncWriteExt;
    let (mut r, mut w) = tokio::io::split(pipe);
    let hello = tokio::time::timeout(std::time::Duration::from_secs(10), read_message(&mut r, MAX_HELLO_BYTES)).await;
    let Ok(Ok(Ok(FromGame::Hello { token, pid, width, height }))) = hello.map(|m| m.map(|b| FromGame::decode(&b))) else { return };
    let Some(app_id) = app_for(&token) else {
        tracing::warn!("overlay: connection with an unknown launch token");
        return;
    };
    tracing::info!("in-game overlay: app {app_id} (pid {pid}) draws it, {width}x{height}");
    let (in_tx, in_rx) = std::sync::mpsc::channel();
    let outbound = Outbound::new();
    NEW.lock().unwrap().push(Conn { app_id, pid, size: (width, height), from_game: in_rx, to_game: outbound.clone() });
    wake();
    let writer_out = outbound.clone();
    let writer = tokio::spawn(async move {
        while let Some(body) = writer_out.recv().await {
            let mut buf = Vec::with_capacity(body.len() + 4);
            buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
            buf.extend_from_slice(&body);
            if w.write_all(&buf).await.is_err() || w.flush().await.is_err() {
                break;
            }
        }
        writer_out.close();
    });
    loop {
        let body = tokio::select! {
            biased;
            _ = outbound.wait_closed() => break,
            res = read_message(&mut r, MAX_FROM_GAME_BYTES) => match res {
                Ok(b) => b,
                Err(_) => break,
            },
        };
        match FromGame::decode(&body) {
            Ok(m) => {
                if in_tx.send(m).is_err() {
                    break;
                }
                wake();
            }
            Err(_) => break,
        }
    }
    outbound.close();
    writer.abort();
    let _ = writer.await;
    // Dropping `r`, `w`, and `in_tx` closes both transport halves and tells the UI the game is gone.
    drop(in_tx);
    wake();
}

/// A saved local screenshot (main JPEG/PNG and optional 200px thumbnail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalScreenshot {
    pub path: std::path::PathBuf,
    pub thumb: Option<std::path::PathBuf>,
    pub name: String,
    pub modified: u64,
}

/// Folder where screenshots for `app_id` live (Steam's `userdata` folder when
/// available, otherwise Aurora's own config data folder).
pub fn screenshots_dir(steam_root: &std::path::Path, account_id: u32, app_id: AppId) -> std::path::PathBuf {
    if !steam_root.as_os_str().is_empty() && account_id != 0 {
        steam_root
            .join("userdata")
            .join(account_id.to_string())
            .join("760")
            .join("remote")
            .join(app_id.to_string())
            .join("screenshots")
    } else {
        crate::store::config_dir().join("screenshots").join(app_id.to_string())
    }
}

fn screenshot_dirs_for(steam_root: &std::path::Path, account_id: u32, app_id: AppId) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if !steam_root.as_os_str().is_empty() && account_id != 0 {
        dirs.push(screenshots_dir(steam_root, account_id, app_id));
    }
    let fallback = crate::store::config_dir().join("screenshots").join(app_id.to_string());
    if !dirs.contains(&fallback) {
        dirs.push(fallback);
    }
    dirs
}

fn reserve_screenshot_file(dir: &std::path::Path) -> Result<(std::path::PathBuf, std::fs::File), String> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    let stamp = format!("{y:04}{m:02}{d:02}{:02}{:02}{:02}", rem / 3600, rem / 60 % 60, rem % 60);
    for n in 1..=10_000u32 {
        let path = dir.join(format!("{stamp}_{n}.jpg"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("could not allocate unique screenshot filename".into())
}

/// Encode an RGB frame as JPEG + 200px wide thumbnail into Steam's screenshot library.
pub fn save_screenshot_rgb(steam_root: &std::path::Path, account_id: u32, app_id: AppId, width: u32, height: u32, rgb: Vec<u8>) -> Result<std::path::PathBuf, String> {
    save_screenshot_rgb_in(&screenshots_dir(steam_root, account_id, app_id), width, height, rgb)
}

fn save_screenshot_rgb_in(dir: &std::path::Path, width: u32, height: u32, rgb: Vec<u8>) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    const THUMB_WIDTH: u32 = 200;
    if width == 0 || height == 0 {
        return Err("empty screenshot dimensions".into());
    }
    let expected = (width as usize).checked_mul(height as usize).and_then(|px| px.checked_mul(3)).ok_or("screenshot dimensions overflow")?;
    if rgb.len() != expected {
        return Err(format!("bad screenshot buffer size: {} != {expected}", rgb.len()));
    }
    std::fs::create_dir_all(dir.join("thumbnails")).map_err(|e| e.to_string())?;
    let img = image::RgbImage::from_raw(width, height, rgb).ok_or("bad image size")?;
    let (path, file) = reserve_screenshot_file(dir)?;
    let write_res = (|| -> Result<(), String> {
        let mut writer = std::io::BufWriter::new(file);
        img.write_to(&mut writer, image::ImageFormat::Jpeg).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    let th = THUMB_WIDTH.min(width).max(1);
    let thumb = image::imageops::thumbnail(&img, th, (height as u64 * th as u64 / width.max(1) as u64).max(1) as u32);
    let _ = thumb.save_with_format(dir.join("thumbnails").join(path.file_name().unwrap_or_default()), image::ImageFormat::Jpeg);
    Ok(path)
}

/// List local screenshots for `app_id` (newest first).
pub fn list_screenshots(steam_root: &std::path::Path, account_id: u32, app_id: AppId) -> Vec<LocalScreenshot> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for dir in screenshot_dirs_for(steam_root, account_id, app_id) {
        collect_screenshots_in(&dir, &mut seen, &mut out);
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.name.cmp(&a.name)));
    out
}

fn collect_screenshots_in(dir: &std::path::Path, seen: &mut std::collections::BTreeSet<std::path::PathBuf>, out: &mut Vec<LocalScreenshot>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let thumb_path = dir.join("thumbnails").join(&name);
        let thumb = thumb_path.is_file().then_some(thumb_path);
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        out.push(LocalScreenshot { path, thumb, name, modified });
    }
}

/// List all local screenshots recorded across every game on this machine (newest first).
pub fn list_all_screenshots() -> Vec<(AppId, LocalScreenshot)> {
    let base = crate::store::config_dir().join("screenshots");
    let Ok(rd) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let Some(app_id) = entry.file_name().to_str().and_then(|s| s.parse::<AppId>().ok()) else {
            continue;
        };
        let mut per_app = Vec::new();
        collect_screenshots_in(&entry.path(), &mut seen, &mut per_app);
        for s in per_app {
            out.push((app_id, s));
        }
    }
    out.sort_by(|a, b| b.1.modified.cmp(&a.1.modified).then_with(|| b.1.name.cmp(&a.1.name)));
    out
}

/// Delete a screenshot and its thumbnail from disk.
pub fn delete_screenshot(path: &std::path::Path) -> Result<(), String> {
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        let thumb = parent.join("thumbnails").join(name);
        if thumb.is_file() {
            let _ = std::fs::remove_file(thumb);
        }
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_pre_hello_to_512_and_post_hello_to_64k() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            use tokio::io::AsyncWriteExt;

            // 1. Pre-Hello > 512 bytes must be rejected before decoding/auth.
            let (mut client, server) = tokio::io::duplex(4096);
            let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let seen2 = seen.clone();
            let task = tokio::spawn(serve(server, Arc::new(move |_| {
                seen2.store(true, std::sync::atomic::Ordering::SeqCst);
                Some(480)
            })));
            let big_hello = FromGame::Hello { token: "t".repeat(600), pid: 1, width: 800, height: 600 }.encode();
            assert!(big_hello.len() > 512);
            let mut framed = Vec::new();
            framed.extend_from_slice(&(big_hello.len() as u32).to_le_bytes());
            framed.extend_from_slice(&big_hello);
            let _ = client.write_all(&framed).await;
            drop(client);
            task.await.unwrap();
            assert!(!seen.load(std::sync::atomic::Ordering::SeqCst), "oversized pre-Hello must be rejected before token lookup");

            // 2. Valid Hello (<= 512 bytes) followed by > 64 KiB FromGame frame must fail closed.
            let (mut client2, server2) = tokio::io::duplex(128 * 1024);
            let task2 = tokio::spawn(serve(server2, Arc::new(|tok| (tok == "ok-tok").then_some(730))));
            let ok_hello = FromGame::Hello { token: "ok-tok".into(), pid: 42, width: 1280, height: 720 }.encode();
            assert!(ok_hello.len() <= 512);
            let mut framed2 = Vec::new();
            framed2.extend_from_slice(&(ok_hello.len() as u32).to_le_bytes());
            framed2.extend_from_slice(&ok_hello);
            // Post-hello frame claiming 65537 bytes (> 64 KiB) without sending body:
            // capped reader rejects length header immediately without waiting for 65537 bytes!
            let too_big = 64 * 1024 + 1usize;
            framed2.extend_from_slice(&(too_big as u32).to_le_bytes());
            let _ = client2.write_all(&framed2).await;
            let finished = tokio::time::timeout(std::time::Duration::from_millis(300), task2).await;
            assert!(finished.is_ok(), "serve must terminate immediately on >64KiB post-Hello frame");
        });
    }

    #[test]
    fn saves_lists_and_deletes_screenshots() {
        let root = std::env::temp_dir().join(format!("aurora-shots-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = screenshots_dir(&root, 12345, 553850);
        assert!(dir.ends_with("userdata/12345/760/remote/553850/screenshots"));

        let saved = save_screenshot_rgb(&root, 12345, 553850, 320, 180, vec![180; 320 * 180 * 3]).unwrap();
        assert!(saved.is_file());
        let list = list_screenshots(&root, 12345, 553850);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, saved);
        assert!(list[0].thumb.as_ref().is_some_and(|t| t.is_file()));

        delete_screenshot(&saved).unwrap();
        assert!(list_screenshots(&root, 12345, 553850).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn outbound_coalesces_mesh_frames_and_preserves_reliable_order() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let out = Outbound::new();
            assert!(out.push_reliable(b"ctrl-1".to_vec(), false));
            assert!(out.push_frame_only(b"frame-old".to_vec()));
            assert!(out.push_frame_only(b"frame-new".to_vec()));
            assert_eq!(out.recv().await.as_deref(), Some(b"ctrl-1".as_slice()));
            assert_eq!(out.recv().await.as_deref(), Some(b"frame-new".as_slice()));

            // A texture-bearing frame supersedes any older pending mesh-only frame.
            assert!(out.push_frame_only(b"frame-stale".to_vec()));
            assert!(out.push_reliable(b"tex-frame".to_vec(), true));
            assert_eq!(out.recv().await.as_deref(), Some(b"tex-frame".as_slice()));

            // Overflowing reliable queue closes cleanly instead of growing unboundedly.
            for _ in 0..MAX_RELIABLE_QUEUE {
                assert!(out.push_reliable(vec![1; 16], false));
            }
            assert!(!out.push_reliable(vec![1; 16], false));
            assert!(out.is_closed());
            assert!(out.recv().await.is_none());
        });
    }
}
