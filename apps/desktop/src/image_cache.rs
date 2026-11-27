//! A disk cache for every picture the app shows (store art, library art,
//! avatars, item icons, screenshots).
//!
//! It's an egui bytes loader registered in front of the normal HTTP one, so
//! all existing image code uses it without changes. A picture is downloaded
//! once, then read from disk on later starts (and offline). After a few days
//! it's revalidated in the background (ETag / Last-Modified) while the saved
//! copy keeps showing. Downloads run a few at a time, the folder is capped,
//! and the oldest pictures go first. Pictures that don't exist (a 404, e.g.
//! a game without library art) are remembered too, so the fallback art
//! shows at once on later starts instead of after another failed request.
//!
//! In memory, a picture is kept only as its GPU texture (no decoded or
//! downloaded copy), and only while it's on screen: a texture unused for
//! [`UNUSED_FOR`] is dropped and comes back from the disk cache when needed.

use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Pictures older than this are checked again (in the background).
const FRESH_FOR: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// Cache size limit; trimmed back to 80% of it, oldest first.
const MAX_BYTES: u64 = 512 * 1024 * 1024;
/// Downloads at once (Steam's CDN drops connections when flooded).
const PARALLEL: usize = 8;
/// Textures not drawn (or prefetched) for this long are unloaded.
const MAX_TEXTURE_BYTES: usize = 96 * 1024 * 1024;
const UNUSED_FOR: Duration = Duration::from_secs(5);
/// Failed in-memory entries expire after this TTL so transient errors can retry,
/// and the failed-entry set is capped to avoid unbounded growth in long sessions.
const FAILED_TTL: Duration = Duration::from_secs(120);
const MAX_FAILED_ENTRIES: usize = 2048;
/// Cap retained in-memory `Entry::Ready` bytes (e.g. animated WebP/GIF) so
/// long sessions do not grow unboundedly.
const MAX_READY_BYTES: usize = 32 * 1024 * 1024;
static WRITES_SINCE_TRIM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static TRIM_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn maybe_schedule_trim(rt: &tokio::runtime::Handle, dir: &PathBuf) {
    use std::sync::atomic::Ordering;
    if WRITES_SINCE_TRIM.fetch_add(1, Ordering::Relaxed) >= 64
        && !TRIM_RUNNING.swap(true, Ordering::AcqRel)
    {
        WRITES_SINCE_TRIM.store(0, Ordering::Relaxed);
        let trim_dir = dir.clone();
        rt.spawn_blocking(move || {
            trim(&trim_dir);
            TRIM_RUNNING.store(false, Ordering::Release);
        });
    }
}

#[derive(Clone)]
enum Entry {
    Pending,
    Ready(Bytes, Option<String>, std::time::Instant),
    /// `disk_cached`: true for persistent 403/404/410 `.missing` markers; false
    /// for transient failures that should auto-retry after `FAILED_TTL`.
    Failed { err: String, at: std::time::Instant, disk_cached: bool },
}

#[derive(Serialize, Deserialize, Default)]
struct Meta {
    mime: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
}

pub struct ImageCache {
    dir: PathBuf,
    rt: tokio::runtime::Handle,
    http: reqwest::Client,
    trusted_http: reqwest::Client,
    slots: Arc<tokio::sync::Semaphore>,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

/// Where pictures are kept: `<cache dir>/images`.
pub fn dir() -> PathBuf {
    directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust").map(|d| d.cache_dir().join("images")).unwrap_or_else(|| PathBuf::from("cache/images"))
}

/// Total size of the saved pictures.
pub fn size() -> u64 {
    std::fs::read_dir(dir()).map(|r| r.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// Delete every saved picture (Settings → Storage).
pub fn clear(ctx: &egui::Context) {
    let d = dir();
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    ctx.forget_all_images();
}

impl ImageCache {
    pub fn install(ctx: &egui::Context, rt: tokio::runtime::Handle) {
        let dir = dir();
        let _ = std::fs::create_dir_all(&dir);
        static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(build_http_client);
        static TRUSTED_HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(build_trusted_https_client);
        static TRIMMED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        let http = HTTP.clone();
        let trusted_http = TRUSTED_HTTP.clone();
        // A second egui context shares the disk cache and the connection pool.
        TRIMMED.get_or_init(|| {
            let trim_dir = dir.clone();
            rt.spawn_blocking(move || trim(&trim_dir));
        });
        ctx.add_bytes_loader(Arc::new(Self {
            dir,
            rt,
            http,
            trusted_http,
            slots: Arc::new(tokio::sync::Semaphore::new(PARALLEL)),
            entries: Default::default(),
        }));
        // Once a picture is a texture, drop its downloaded and decoded copies
        // (the disk cache has it), and unload textures that are off screen.
        ctx.options_mut(|o| o.reduce_texture_memory = true);
        ctx.add_texture_loader(Arc::new(UnloadUnused::default()));
        // Still pictures decoded at the size they're drawn at (sized_images.rs).
        crate::sized_images::SizedLoader::install(ctx);
    }

    fn key(uri: &str) -> String {
        const LUT: &[u8; 16] = b"0123456789abcdef";
        let digest = Sha256::digest(uri.as_bytes());
        let mut hex = [0u8; 32];
        for (i, &b) in digest[..16].iter().enumerate() {
            hex[i * 2] = LUT[(b >> 4) as usize];
            hex[i * 2 + 1] = LUT[(b & 0x0f) as usize];
        }
        // SAFETY: LUT contains only ASCII hex bytes.
        unsafe { String::from_utf8_unchecked(hex.to_vec()) }
    }
}

fn read_meta(path: &PathBuf) -> Meta {
    std::fs::read_to_string(path.with_extension("json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn age(path: &PathBuf) -> Option<Duration> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok())
}

impl BytesLoader for ImageCache {
    fn id(&self) -> &str {
        egui::generate_loader_id!(ImageCache)
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !(uri.starts_with("https://") || uri.starts_with("http://")) {
            return Err(LoadError::NotSupported);
        }
        let trusted_cdn = {
            let mut map = self.entries.lock().unwrap();
            if let Some(e) = map.get(uri) {
                match e {
                    Entry::Pending => return Ok(BytesPoll::Pending { size: None }),
                    Entry::Ready(bytes, mime, _) => return Ok(BytesPoll::Ready { size: None, bytes: bytes.clone(), mime: mime.clone() }),
                    Entry::Failed { err, at, disk_cached } => {
                        let ttl = if *disk_cached { FRESH_FOR } else { FAILED_TTL };
                        let elapsed = at.elapsed();
                        if elapsed < ttl {
                            if !*disk_cached {
                                ctx.request_repaint_after(ttl.saturating_sub(elapsed).max(Duration::from_millis(100)));
                            }
                            return Err(LoadError::Loading(err.clone()));
                        }
                        map.remove(uri);
                    }
                }
            }
            let parsed = match validate_image_url(uri) {
                Ok(u) => u,
                Err(err) => {
                    tracing::warn!("image_cache: blocked image URL {uri}: {err}");
                    map.insert(uri.to_string(), Entry::Failed { err: err.clone(), at: std::time::Instant::now(), disk_cached: false });
                    return Err(LoadError::Loading(err));
                }
            };
            map.insert(uri.to_string(), Entry::Pending);
            is_trusted_https_steam_cdn_url(&parsed)
        };
        let (uri, ctx) = (uri.to_string(), ctx.clone());
        let cache_dir = self.dir.clone();
        let path = self.dir.join(Self::key(&uri));
        let http = if trusted_cdn { self.trusted_http.clone() } else { self.http.clone() };
        let (slots, entries, rt_handle) = (self.slots.clone(), self.entries.clone(), self.rt.clone());
        self.rt.spawn(async move {
            let done = |e: Entry| {
                let mut map = entries.lock().unwrap();
                if matches!(e, Entry::Failed { .. }) {
                    let failed_count = map.values().filter(|v| matches!(v, Entry::Failed { .. })).count();
                    if failed_count >= MAX_FAILED_ENTRIES {
                        let now = std::time::Instant::now();
                        map.retain(|_, v| {
                            !matches!(v, Entry::Failed { at, disk_cached, .. }
                                if now.duration_since(*at) >= if *disk_cached { FRESH_FOR } else { FAILED_TTL })
                        });
                        let remaining_failed = map.values().filter(|v| matches!(v, Entry::Failed { .. })).count();
                        if remaining_failed >= MAX_FAILED_ENTRIES {
                            let mut oldest: Vec<(std::time::Instant, String)> = map
                                .iter()
                                .filter_map(|(k, v)| match v {
                                    Entry::Failed { at, .. } => Some((*at, k.clone())),
                                    _ => None,
                                })
                                .collect();
                            oldest.sort_unstable_by_key(|(at, _)| *at);
                            let drop_n = remaining_failed - (MAX_FAILED_ENTRIES / 2);
                            for (_, k) in oldest.into_iter().take(drop_n) {
                                map.remove(&k);
                            }
                        }
                    }
                }
                map.insert(uri.clone(), e);
                let mut ready_bytes: usize = map
                    .values()
                    .map(|v| if let Entry::Ready(b, _, _) = v { b.len() } else { 0 })
                    .sum();
                if ready_bytes > MAX_READY_BYTES {
                    let mut oldest: Vec<(std::time::Instant, usize, String)> = map
                        .iter()
                        .filter_map(|(k, v)| match v {
                            Entry::Ready(b, _, at) if k != &uri => Some((*at, b.len(), k.clone())),
                            _ => None,
                        })
                        .collect();
                    oldest.sort_unstable_by_key(|(at, _, _)| *at);
                    for (_, len, k) in oldest {
                        if ready_bytes <= MAX_READY_BYTES / 2 {
                            break;
                        }
                        map.remove(&k);
                        ready_bytes = ready_bytes.saturating_sub(len);
                    }
                }
                ctx.request_repaint();
            };
            // 1. Check disk cache off the async worker threads so bursts of cached
            // pictures never stall network / Steam CM tasks.
            let probe_path = path.clone();
            enum DiskHit {
                Fresh(Vec<u8>, Option<String>),
                Stale(Vec<u8>, Meta),
                KnownMissing(String),
                Miss,
            }
            let disk = tokio::task::spawn_blocking(move || {
                if let Ok(bytes) = std::fs::read(&probe_path) {
                    let meta = read_meta(&probe_path);
                    if age(&probe_path).is_some_and(|a| a < FRESH_FOR) {
                        DiskHit::Fresh(bytes, meta.mime)
                    } else {
                        DiskHit::Stale(bytes, meta)
                    }
                } else {
                    let missing = probe_path.with_extension("missing");
                    if age(&missing).is_some_and(|a| a < FRESH_FOR) {
                        let status = std::fs::read_to_string(&missing).unwrap_or_default();
                        DiskHit::KnownMissing(status)
                    } else {
                        DiskHit::Miss
                    }
                }
            })
            .await
            .unwrap_or(DiskHit::Miss);

            let (has_saved, meta) = match disk {
                DiskHit::Fresh(bytes, mime) => {
                    done(Entry::Ready(Bytes::from(bytes), mime, std::time::Instant::now()));
                    return;
                }
                DiskHit::KnownMissing(status) => {
                    done(Entry::Failed { err: format!("HTTP {}", status.trim()), at: std::time::Instant::now(), disk_cached: true });
                    return;
                }
                DiskHit::Stale(bytes, meta) => {
                    let mime = meta.mime.clone();
                    done(Entry::Ready(Bytes::from(bytes), mime, std::time::Instant::now()));
                    (true, meta)
                }
                DiskHit::Miss => (false, Meta::default()),
            };
            // 2. Download, or revalidate an old copy.
            let _slot = slots.acquire().await;
            let mut req = http.get(&uri);
            if has_saved {
                if let Some(etag) = &meta.etag {
                    req = req.header(reqwest::header::IF_NONE_MATCH, etag);
                }
                if let Some(lm) = &meta.last_modified {
                    req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
                }
            }
            match req.send().await {
                Ok(resp) if resp.remote_addr().is_some_and(|a| {
                    if is_trusted_https_steam_cdn_url(resp.url()) {
                        !is_allowed_trusted_cdn_ip(a.ip())
                    } else {
                        !is_public_ip(a.ip())
                    }
                }) => {
                    if let Some(addr) = resp.remote_addr() {
                        tracing::warn!("image_cache: blocked response from non-public IP {addr} for {uri}");
                    }
                    if !has_saved {
                        done(Entry::Failed { err: "blocked non-public image host".into(), at: std::time::Instant::now(), disk_cached: false });
                    }
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_MODIFIED => {
                    // Still current: mark it fresh again.
                    let touch_path = path.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(f) = std::fs::File::options().append(true).open(&touch_path) {
                            let _ = f.set_modified(SystemTime::now());
                        }
                    })
                    .await;
                }
                Ok(resp) if resp.status().is_success() => {
                    let header = |name| resp.headers().get(name).and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok()).map(String::from);
                    let meta = Meta { mime: header(reqwest::header::CONTENT_TYPE), etag: header(reqwest::header::ETAG), last_modified: header(reqwest::header::LAST_MODIFIED) };
                    match read_image_bytes(resp).await {
                        Ok(bytes) => {
                            let shared = Bytes::from(bytes);
                            let mime = meta.mime.clone();
                            done(Entry::Ready(shared.clone(), mime, std::time::Instant::now()));
                            let write_path = path.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                if let Some(parent) = write_path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let tmp = write_path.with_extension("part");
                                if std::fs::write(&tmp, &shared[..]).is_ok() {
                                    let _ = std::fs::rename(&tmp, &write_path);
                                    let _ = std::fs::write(write_path.with_extension("json"), serde_json::to_string(&meta).unwrap_or_default());
                                }
                            })
                            .await;
                            maybe_schedule_trim(&rt_handle, &cache_dir);
                        }
                        Err(e) if !has_saved => done(Entry::Failed { err: e, at: std::time::Instant::now(), disk_cached: false }),
                        Err(_) => {}
                    }
                }
                Ok(resp) if !has_saved => {
                    let status = resp.status();
                    // Remember pictures that don't exist (not server hiccups).
                    let disk_cached = matches!(status.as_u16(), 403 | 404 | 410);
                    if disk_cached {
                        let missing = path.with_extension("missing");
                        let text = status.to_string();
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Some(parent) = missing.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(&missing, text);
                        })
                        .await;
                        maybe_schedule_trim(&rt_handle, &cache_dir);
                    }
                    done(Entry::Failed { err: format!("HTTP {status}"), at: std::time::Instant::now(), disk_cached });
                }
                Err(e) if !has_saved => done(Entry::Failed { err: e.to_string(), at: std::time::Instant::now(), disk_cached: false }),
                // Offline or the server hiccuped: keep showing the saved copy.
                _ => {}
            }
        });
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.entries.lock().unwrap().remove(uri);
    }

    fn forget_all(&self) {
        self.entries.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.entries.lock().unwrap().values().map(|e| if let Entry::Ready(b, _, _) = e { b.len() } else { 0 }).sum()
    }
}

/// The standard texture cache, plus unloading: a texture that hasn't been
/// drawn for [`UNUSED_FOR`] is dropped (scrolled away, another page open).
#[derive(Default)]
struct UnloadUnused {
    inner: egui::load::DefaultTextureLoader,
    /// When each loaded texture was last asked for.
    used: Mutex<HashMap<String, std::time::Instant>>,
    last_sweep: Mutex<Option<std::time::Instant>>,
}

impl egui::load::TextureLoader for UnloadUnused {
    fn id(&self) -> &str {
        egui::generate_loader_id!(UnloadUnused)
    }

    fn load(&self, ctx: &egui::Context, uri: &str, texture_options: egui::TextureOptions, size_hint: egui::SizeHint) -> egui::load::TextureLoadResult {
        let result = self.inner.load(ctx, uri, texture_options, size_hint);
        if matches!(result, Ok(egui::load::TexturePoll::Ready { .. })) {
            let now = std::time::Instant::now();
            let mut used = self.used.lock().unwrap();
            match used.get_mut(uri) {
                Some(t) => *t = now,
                None => {
                    used.insert(uri.to_owned(), now);
                }
            }
        }
        result
    }

    fn forget(&self, uri: &str) {
        self.used.lock().unwrap().remove(uri);
        self.inner.forget(uri);
    }

    fn forget_all(&self) {
        self.used.lock().unwrap().clear();
        self.inner.forget_all();
    }

    fn end_pass(&self, pass_index: u64) {
        self.inner.end_pass(pass_index);
        let now = std::time::Instant::now();
        {
            let mut last = self.last_sweep.lock().unwrap();
            if last.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
                return;
            }
            *last = Some(now);
        }
        let stale: Vec<String> = {
            let mut used = self.used.lock().unwrap();
            let stale: Vec<String> = used.iter().filter(|(_, t)| now.duration_since(**t) > UNUSED_FOR).map(|(uri, _)| uri.clone()).collect();
            for uri in &stale {
                used.remove(uri);
            }
            stale
        };
        for uri in stale {
            self.inner.forget(&uri);
        }
        if self.inner.byte_size() > MAX_TEXTURE_BYTES {
            let mut used = self.used.lock().unwrap();
            // Keep textures used in the current pass. Evict older ones first.
            let mut candidates: Vec<_> = used.iter()
                .filter(|(_, t)| now.duration_since(**t) > Duration::from_millis(200))
                .map(|(uri, t)| (*t, uri.clone()))
                .collect();
            candidates.sort_unstable_by_key(|(t, _)| *t);
            for (_, uri) in candidates {
                if self.inner.byte_size() <= MAX_TEXTURE_BYTES {
                    break;
                }
                used.remove(&uri);
                self.inner.forget(&uri);
            }
        }
    }

    fn byte_size(&self) -> usize {
        self.inner.byte_size()
    }
}

/// Delete expired `.missing` markers and the oldest pictures until the folder
/// is under 80% of the limit.
fn trim(dir: &PathBuf) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for e in read.flatten() {
        let path = e.path();
        match path.extension().and_then(|s| s.to_str()) {
            Some("missing") => {
                if age(&path).is_some_and(|a| a >= FRESH_FOR) {
                    let _ = std::fs::remove_file(&path);
                }
            }
            None => {
                if let Ok(m) = e.metadata()
                    && let Ok(mod_t) = m.modified()
                {
                    files.push((mod_t, m.len(), path));
                }
            }
            _ => {}
        }
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= MAX_BYTES {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= MAX_BYTES / 10 * 8 {
            break;
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json"));
        total = total.saturating_sub(len);
    }
}

// Steam Workshop's animated GIF artwork is routinely larger than the 16 MiB
// limit that is sensible for static thumbnails. Keep a firm bound for remote
// data, but allow a complete GIF through to egui's animated-image loader.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

fn is_public_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || a == 0
        || (a == 100 && (b & 0b1100_0000) == 64)
        || (a == 198 && (b & 0b1111_1110) == 18)
        || a >= 240)
}

fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_public_ipv4(v4),
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            if let Some(v4) = v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
                return is_public_ipv4(v4);
            }
            let segs = v6.segments();
            !((segs[0] & 0xfe00) == 0xfc00
                || (segs[0] & 0xffc0) == 0xfe80
                || (segs[0] == 0x2001 && segs[1] == 0x0db8))
        }
    }
}

fn is_allowed_trusted_cdn_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let [a, _, _, _] = ip.octets();
    !(ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || a == 0
        || a >= 240)
}

fn is_allowed_trusted_cdn_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_allowed_trusted_cdn_ipv4(v4),
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            if let Some(v4) = v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
                return is_allowed_trusted_cdn_ipv4(v4);
            }
            let segs = v6.segments();
            (segs[0] & 0xffc0) != 0xfe80
        }
    }
}

fn validate_parsed_image_url(url: &reqwest::Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("unsupported image scheme".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("image URLs cannot carry credentials".into());
    }
    let Some(host) = url.host_str() else {
        return Err("image URL missing host".into());
    };
    let clean = host.trim_end_matches('.');
    let unbracketed = clean.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(clean);
    if let Ok(ip) = unbracketed.parse::<std::net::IpAddr>() {
        if !is_public_ip(ip) {
            return Err("blocked non-public IP address".into());
        }
        return Ok(());
    }
    let lower = unbracketed.to_ascii_lowercase();
    if lower.is_empty()
        || lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
    {
        return Err("blocked local image hostname".into());
    }
    Ok(())
}

fn resolve_public_addrs(host: &str) -> Result<Vec<std::net::SocketAddr>, String> {
    use std::net::ToSocketAddrs;
    let mut out = Vec::new();
    for addr in (host, 0).to_socket_addrs().map_err(|e| e.to_string())? {
        if !is_public_ip(addr.ip()) {
            tracing::warn!("image_cache: blocked non-public DNS resolution for {host} -> {}", addr.ip());
            return Err("blocked non-public DNS resolution".into());
        }
        out.push(addr);
    }
    if out.is_empty() {
        return Err("DNS returned no addresses".into());
    }
    Ok(out)
}

/// Resolver for trusted HTTPS Steam CDN hosts: allows non-public IPs such as
/// TUN `fake-ip` pools (`198.18.0.0/15`, `fc00::/18`) because TLS certificate
/// validation binds the connection to the Steam CDN hostname. Non-trusted
/// hosts still go through `resolve_public_addrs` as defense in depth.
fn resolve_trusted_cdn_addrs(host: &str) -> Result<Vec<std::net::SocketAddr>, String> {
    if !is_trusted_steam_cdn_host(host) {
        return resolve_public_addrs(host);
    }
    use std::net::ToSocketAddrs;
    let mut out = Vec::new();
    for addr in (host, 0).to_socket_addrs().map_err(|e| e.to_string())? {
        if !is_allowed_trusted_cdn_ip(addr.ip()) {
            tracing::warn!("image_cache: blocked disallowed DNS resolution for trusted CDN {host} -> {}", addr.ip());
            return Err("blocked disallowed DNS resolution".into());
        }
        out.push(addr);
    }
    if out.is_empty() {
        return Err("DNS returned no addresses".into());
    }
    Ok(out)
}

struct PublicDnsResolver;

impl reqwest::dns::Resolve for PublicDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = tokio::task::spawn_blocking(move || resolve_public_addrs(&host))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::PermissionDenied, e)) as Box<dyn std::error::Error + Send + Sync>)?;
            let iter: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

struct TrustedSteamCdnDnsResolver;

impl reqwest::dns::Resolve for TrustedSteamCdnDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = tokio::task::spawn_blocking(move || resolve_trusted_cdn_addrs(&host))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::PermissionDenied, e)) as Box<dyn std::error::Error + Send + Sync>)?;
            let iter: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

fn is_trusted_steam_cdn_host(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty() || h.starts_with('.') || h.contains("..") {
        return false;
    }
    matches!(
        h.as_str(),
        "steamusercontent-a.akamaihd.net"
            | "steamuserimages-a.akamaihd.net"
            | "steamcdn-a.akamaihd.net"
            | "steamvideo-a.akamaihd.net"
            | "media.steampowered.com"
            | "cdn.steampowered.com"
            | "video.steampowered.com"
            | "steamcommunity.com"
    ) || h.ends_with(".steamusercontent.com")
        || h.ends_with(".steamstatic.com")
}

fn is_trusted_https_steam_cdn_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(is_trusted_steam_cdn_host)
}

fn build_http_client() -> reqwest::Client {
    let redirect = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            tracing::warn!("image_cache: blocked redirect (too many redirects) to {}", attempt.url());
            return attempt.error("too many redirects");
        }
        if let Err(err) = validate_parsed_image_url(attempt.url()) {
            tracing::warn!("image_cache: blocked redirect to {}: {err}", attempt.url());
            return attempt.error(err);
        }
        if let Some(origin) = attempt.previous().first() {
            if origin.scheme() == "https" && attempt.url().scheme() != "https" {
                tracing::warn!("image_cache: blocked HTTPS-to-HTTP redirect from {origin} to {}", attempt.url());
                return attempt.error("refusing HTTPS-to-HTTP image redirect");
            }
            if origin.host_str().is_some_and(is_trusted_steam_cdn_host)
                && !attempt.url().host_str().is_some_and(is_trusted_steam_cdn_host)
            {
                tracing::warn!("image_cache: blocked redirect off Steam CDN from {origin} to {}", attempt.url());
                return attempt.error("refusing redirect off Steam CDN");
            }
        }
        attempt.follow()
    });
    reqwest::Client::builder()
        .user_agent(concat!("Aurora/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .redirect(redirect)
        .dns_resolver(Arc::new(PublicDnsResolver))
        .build()
        .unwrap_or_default()
}

fn build_trusted_https_client() -> reqwest::Client {
    let redirect = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            tracing::warn!("image_cache: blocked Steam CDN redirect (too many redirects) to {}", attempt.url());
            return attempt.error("too many redirects");
        }
        if let Err(err) = validate_parsed_image_url(attempt.url()) {
            tracing::warn!("image_cache: blocked Steam CDN redirect to {}: {err}", attempt.url());
            return attempt.error(err);
        }
        if !is_trusted_https_steam_cdn_url(attempt.url()) {
            tracing::warn!("image_cache: blocked redirect off trusted HTTPS Steam CDN to {}", attempt.url());
            return attempt.error("refusing redirect off trusted HTTPS Steam CDN");
        }
        attempt.follow()
    });
    reqwest::Client::builder()
        .user_agent(concat!("Aurora/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .https_only(true)
        .redirect(redirect)
        .dns_resolver(Arc::new(TrustedSteamCdnDnsResolver))
        .build()
        .unwrap_or_default()
}

fn validate_image_url(uri: &str) -> Result<reqwest::Url, String> {
    if uri.bytes().any(|b| b < 0x20 || b == 0x7f) || uri.contains('\\') {
        return Err("invalid character in image URL".into());
    }
    let url = reqwest::Url::parse(uri).map_err(|e| e.to_string())?;
    validate_parsed_image_url(&url)?;
    Ok(url)
}

async fn read_image_bytes(mut resp: reqwest::Response) -> Result<Vec<u8>, String> {
    if resp.content_length().is_some_and(|len| len > MAX_IMAGE_BYTES as u64) {
        return Err("image exceeds 16 MiB limit".into());
    }
    let cap = resp.content_length().map(|n| (n as usize).min(MAX_IMAGE_BYTES)).unwrap_or(0);
    let mut buf = Vec::with_capacity(cap);
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if buf.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES {
            return Err("image exceeds 16 MiB limit".into());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn rejects_ssrf_private_loopback_and_link_local_urls() {
        for bad in [
            "http://127.0.0.1/a.png",
            "https://127.0.1.1:8080/a.png",
            "http://[::1]/a.png",
            "http://[::ffff:127.0.0.1]/a.png",
            "http://localhost/a.png",
            "https://sub.localhost/a.png",
            "http://169.254.169.254/latest/meta-data/",
            "http://[fe80::1]/a.png",
            "http://10.0.0.1/a.png",
            "http://172.16.0.1/a.png",
            "http://192.168.1.1/a.png",
            "http://[fc00::1]/a.png",
            "http://[fd12:3456::1]/a.png",
            "http://0.0.0.0/a.png",
        ] {
            assert!(validate_image_url(bad).is_err(), "must reject SSRF url {bad}");
        }
        assert!(validate_image_url("https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/440/header.jpg").is_ok());
    }

    #[tokio::test]
    async fn blocks_redirect_ssrf_and_caps_streaming_body_at_64_mib() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req);
            let too_large = MAX_IMAGE_BYTES + 1024 * 1024;
            let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {too_large}\r\nConnection: close\r\n\r\n");
            let _ = sock.write_all(header.as_bytes());
            let block = vec![0u8; 1024 * 1024];
            for _ in 0..(MAX_IMAGE_BYTES / block.len() + 1) {
                if sock.write_all(&block).is_err() {
                    break;
                }
            }
        });
        let raw_client = reqwest::Client::new();
        let resp = raw_client.get(format!("http://{addr}/bomb.png")).send().await.unwrap();
        assert!(read_image_bytes(resp).await.is_err(), "image > 64 MiB must be rejected");
        tokio::task::spawn_blocking(move || srv.join().unwrap()).await.unwrap();

        // Also test a chunked body without Content-Length exceeding 64 MiB.
        let chunked = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = chunked.local_addr().unwrap();
        let csrv = std::thread::spawn(move || {
            let (mut sock, _) = chunked.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req);
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
            let chunk_hdr = format!("{:x}\r\n", 1024 * 1024);
            let block = vec![b'a'; 1024 * 1024];
            for _ in 0..(MAX_IMAGE_BYTES / block.len() + 1) {
                if sock.write_all(chunk_hdr.as_bytes()).is_err()
                    || sock.write_all(&block).is_err()
                    || sock.write_all(b"\r\n").is_err()
                {
                    break;
                }
            }
            let _ = sock.write_all(b"0\r\n\r\n");
        });
        let resp = raw_client.get(format!("http://{caddr}/chunked.png")).send().await.unwrap();
        assert!(read_image_bytes(resp).await.is_err(), "chunked image > 64 MiB must be rejected");
        drop(raw_client);
        tokio::task::spawn_blocking(move || csrv.join().unwrap()).await.unwrap();

        // ImageCache HTTP client redirect policy must reject loopback/private redirects.
        let redir = TcpListener::bind("127.0.0.1:0").unwrap();
        let raddr = redir.local_addr().unwrap();
        let rsrv = std::thread::spawn(move || {
            let (mut sock, _) = redir.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req);
            let _ = sock.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        });
        let cache_http = build_http_client();
        let err = cache_http.get(format!("http://{raddr}/redir")).send().await.unwrap_err();
        assert!(err.is_redirect(), "redirect to link-local IP must be blocked, got {err}");
        tokio::task::spawn_blocking(move || rsrv.join().unwrap()).await.unwrap();

        assert!(validate_image_url("http://2130706433/a.png").is_err());
        assert!(validate_image_url("http://0177.0.0.1/a.png").is_err());
        assert!(resolve_public_addrs("127.0.0.1").is_err());
        assert!(resolve_public_addrs("169.254.169.254").is_err());
        assert!(resolve_public_addrs("198.18.0.1").is_err());
        assert!(resolve_trusted_cdn_addrs("127.0.0.1").is_err());
        assert!(resolve_trusted_cdn_addrs("198.18.0.1").is_err());
    }

    #[test]
    fn trusted_https_steam_cdn_classification() {
        for trusted in [
            "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/440/header.jpg",
            "https://shared.fastly.steamstatic.com:443/store_item_assets/steam/apps/440/header.jpg",
            "https://avatars.fastly.steamstatic.com/abc_medium.jpg",
            "https://images.steamusercontent.com/ugc/1/shot.png",
            "https://steamuserimages-a.akamaihd.net/ugc/1/shot.png",
            "https://steamcdn-a.akamaihd.net/steam/apps/440/header.jpg",
            "https://media.steampowered.com/steamcommunity/public/images/apps/440/a.jpg",
            "https://video.akamai.steamstatic.com/store_trailers/1/hls_master.m3u8",
        ] {
            let url = reqwest::Url::parse(trusted).unwrap();
            assert!(is_trusted_https_steam_cdn_url(&url), "expected trusted HTTPS Steam CDN: {trusted}");
        }
        for untrusted in [
            "http://shared.fastly.steamstatic.com/header.jpg",
            "https://shared.fastly.steamstatic.com:8443/header.jpg",
            "https://evil-steamstatic.com/header.jpg",
            "https://steamstatic.com.evil.com/header.jpg",
            "https://example.com/header.jpg",
            "https://198.18.0.1/header.jpg",
            "https://127.0.0.1/header.jpg",
        ] {
            let url = reqwest::Url::parse(untrusted).unwrap();
            assert!(!is_trusted_https_steam_cdn_url(&url), "must not treat as trusted HTTPS Steam CDN: {untrusted}");
        }
        assert!(!is_trusted_steam_cdn_host(".steamstatic.com"));
        assert!(!is_trusted_steam_cdn_host("foo..steamstatic.com"));
        assert!(!is_trusted_steam_cdn_host(""));

        for allowed_ip in ["1.1.1.1", "198.18.0.1", "100.64.0.1", "10.0.0.1", "192.168.1.1", "fc00::1", "fd12::1"] {
            let ip: std::net::IpAddr = allowed_ip.parse().unwrap();
            assert!(is_allowed_trusted_cdn_ip(ip), "expected allowed trusted CDN IP: {allowed_ip}");
        }
        for blocked_ip in ["127.0.0.1", "::1", "::ffff:127.0.0.1", "169.254.169.254", "fe80::1", "0.0.0.0", "::", "224.0.0.1", "255.255.255.255"] {
            let ip: std::net::IpAddr = blocked_ip.parse().unwrap();
            assert!(!is_allowed_trusted_cdn_ip(ip), "must reject disallowed trusted CDN IP: {blocked_ip}");
        }
    }
}
