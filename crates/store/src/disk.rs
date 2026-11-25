//! Steam's answers for game pages (store details, news, store info) saved on
//! disk. A game opened before shows at once, even after a restart or offline,
//! instead of waiting for Steam, so the app can keep only a few games in
//! memory. Off until the app calls [`init`] (tests never touch the disk).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

static DIR: OnceLock<PathBuf> = OnceLock::new();
static WRITES_SINCE_TRIM: AtomicUsize = AtomicUsize::new(0);
static TRIM_RUNNING: AtomicBool = AtomicBool::new(false);

/// Folder size limit; trimmed back to 80% of it at start, oldest first.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Turn the cache on, in `dir`, and trim it in the background.
pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let trim_dir = dir.clone();
    std::thread::spawn(move || trim(&trim_dir));
    let _ = DIR.set(dir);
}

/// Total size of the saved answers (Settings → Storage).
pub fn size() -> u64 {
    let Some(dir) = DIR.get() else { return 0 };
    std::fs::read_dir(dir).map(|r| r.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// Delete every saved answer.
pub fn clear() {
    if let Some(dir) = DIR.get() {
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::create_dir_all(dir);
    }
}

/// FNV-1a: a stable file name for a URL.
fn path(key: &str) -> Option<PathBuf> {
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    DIR.get().map(|d| d.join(format!("{hash:016x}.json")))
}

fn age(path: &Path) -> Option<Duration> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok())
}

/// The saved answer for `key`, if it's younger than `max_age`.
pub(crate) fn read(key: &str, max_age: Duration) -> Option<Vec<u8>> {
    let path = path(key)?;
    age(&path).filter(|a| *a <= max_age)?;
    std::fs::read(path).ok()
}

/// The saved answer for `key`, however old (when Steam can't be reached).
pub(crate) fn read_any(key: &str) -> Option<Vec<u8>> {
    std::fs::read(path(key)?).ok()
}

pub(crate) fn write(key: &str, bytes: &[u8]) {
    let Some(path) = path(key) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("part");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
        if WRITES_SINCE_TRIM.fetch_add(1, Ordering::Relaxed) >= 64
            && !TRIM_RUNNING.swap(true, Ordering::AcqRel)
            && let Some(dir) = DIR.get().cloned()
        {
            WRITES_SINCE_TRIM.store(0, Ordering::Relaxed);
            std::thread::spawn(move || {
                trim(&dir);
                TRIM_RUNNING.store(false, Ordering::Release);
            });
        }
    }
}

fn trim(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = read
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= MAX_BYTES {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= MAX_BYTES * 8 / 10 {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total -= len;
        }
    }
}
