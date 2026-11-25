//! Managing installed games: installed depots / DLC, file verification, uninstall.
//!
//! Verification uses the depot manifests Steam keeps in `<steam>/depotcache`
//! (`<depot>_<manifest>.manifest`). Each lists every file with its size and
//! SHA-1, so we can check an install without any network access.

use model::{AppId, InstallState, InstalledGame};
use prost::Message;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A depot listed under `InstalledDepots` in an app manifest.
#[derive(Debug, Clone)]
pub struct InstalledDepot {
    pub depot: u32,
    pub manifest: u64,
    pub size: u64,
    /// Set when the depot belongs to a DLC.
    pub dlc_app: Option<AppId>,
}

fn manifest_file(game: &InstalledGame) -> PathBuf {
    game.library.join("steamapps").join(format!("appmanifest_{}.acf", game.app_id))
}

pub fn installed_depots(game: &InstalledGame) -> Vec<InstalledDepot> {
    let Ok(text) = fs::read_to_string(manifest_file(game)) else { return Vec::new() };
    let Ok(doc) = vdf::parse(&text) else { return Vec::new() };
    let Some(depots) = doc.path(&["AppState", "InstalledDepots"]) else { return Vec::new() };
    depots
        .iter()
        .filter_map(|(id, v)| {
            let o = v.as_obj()?;
            Some(InstalledDepot {
                depot: id.parse().ok()?,
                manifest: o.u64("manifest")?,
                size: o.u64("size").unwrap_or(0),
                dlc_app: o.u64("dlcappid").map(|v| v as AppId),
            })
        })
        .collect()
}

/// DLC app ids that have content installed for this game.
pub fn installed_dlc(game: &InstalledGame) -> Vec<AppId> {
    let mut v: Vec<AppId> = installed_depots(game).into_iter().filter_map(|d| d.dlc_app).collect();
    v.sort();
    v.dedup();
    v
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha1: Vec<u8>,
}

const PAYLOAD_MAGIC: u32 = 0x71F6_17D0;
const METADATA_MAGIC: u32 = 0x1F48_12BE;
const SIGNATURE_MAGIC: u32 = 0x1B81_B817;
const END_MAGIC: u32 = 0x32C4_15AB;

/// Parse a depot manifest. Returns `None` if filenames are encrypted
/// (Steam stores those decrypted, so this is rare).
pub fn read_manifest(data: &[u8]) -> Result<Option<Vec<FileEntry>>, String> {
    let mut pos = 0;
    let mut payload = None;
    let mut encrypted = false;
    let u32_at = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    while let Some(magic) = u32_at(pos) {
        pos += 4;
        if magic == END_MAGIC {
            break;
        }
        let len = u32_at(pos).ok_or("truncated manifest")? as usize;
        pos += 4;
        let body = data.get(pos..pos + len).ok_or("truncated manifest section")?;
        pos += len;
        match magic {
            PAYLOAD_MAGIC => payload = Some(proto::ContentManifestPayload::decode(body).map_err(|e| e.to_string())?),
            METADATA_MAGIC => {
                encrypted = proto::ContentManifestMetadata::decode(body).map(|m| m.filenames_encrypted()).unwrap_or(false);
            }
            SIGNATURE_MAGIC => {}
            other => return Err(format!("unknown manifest section {other:#x}")),
        }
    }
    if encrypted {
        return Ok(None);
    }
    let payload = payload.ok_or("manifest has no file list")?;
    const DIRECTORY: u32 = 64;
    Ok(Some(
        payload
            .mappings
            .into_iter()
            .filter(|m| m.flags() & DIRECTORY == 0 && m.linktarget().is_empty())
            .map(|m| FileEntry { path: m.filename().replace('\\', "/"), size: m.size(), sha1: m.sha_content().to_vec() })
            .collect(),
    ))
}

/// Shared progress for a running verification.
#[derive(Debug, Default)]
pub struct Progress {
    pub total_bytes: AtomicU64,
    pub done_bytes: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.total_bytes.load(Ordering::Relaxed).max(1);
        self.done_bytes.load(Ordering::Relaxed) as f32 / total as f32
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub files: usize,
    pub ok: usize,
    pub missing: Vec<String>,
    pub corrupt: Vec<String>,
    /// Depots whose manifest isn't on disk (needs the downloader to fetch it).
    pub unchecked_depots: Vec<u32>,
    pub cancelled: bool,
}

impl VerifyReport {
    pub fn healthy(&self) -> bool {
        self.missing.is_empty() && self.corrupt.is_empty()
    }
}

/// Hash every file of every installed depot and compare with the manifests.
pub fn verify(steam_root: &Path, game: &InstalledGame, progress: &Progress) -> VerifyReport {
    let mut report = VerifyReport::default();
    let mut work: Vec<FileEntry> = Vec::new();
    for d in installed_depots(game) {
        let path = steam_root.join("depotcache").join(format!("{}_{}.manifest", d.depot, d.manifest));
        match fs::read(&path).map_err(|e| e.to_string()).and_then(|b| read_manifest(&b)) {
            Ok(Some(files)) => work.extend(files),
            Ok(None) | Err(_) => report.unchecked_depots.push(d.depot),
        }
    }
    work.sort_by(|a, b| a.path.cmp(&b.path));
    work.dedup_by(|a, b| a.path == b.path);
    progress.total_bytes.store(work.iter().map(|f| f.size).sum(), Ordering::Relaxed);
    report.files = work.len();

    let canon_install = ensure_real_dir(&game.install_dir, "install_dir").ok();
    let mut buf = vec![0u8; 256 * 1024];
    for f in work {
        if progress.cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let Ok(rel) = validate_rel_path(&f.path) else {
            report.corrupt.push(f.path);
            progress.done_bytes.fetch_add(f.size, Ordering::Relaxed);
            continue;
        };
        let Some(ref canon_root) = canon_install else {
            report.corrupt.push(f.path);
            progress.done_bytes.fetch_add(f.size, Ordering::Relaxed);
            continue;
        };
        let Ok(full) = resolve_inside_no_symlinks(&game.install_dir, canon_root, &rel) else {
            report.corrupt.push(f.path);
            progress.done_bytes.fetch_add(f.size, Ordering::Relaxed);
            continue;
        };
        let Ok(meta_pre) = fs::symlink_metadata(&full) else {
            report.missing.push(f.path);
            progress.done_bytes.fetch_add(f.size, Ordering::Relaxed);
            continue;
        };
        if crate::is_symlink_or_reparse(&meta_pre) || !meta_pre.is_file() {
            report.corrupt.push(f.path);
            progress.done_bytes.fetch_add(f.size, Ordering::Relaxed);
            continue;
        }
        let Ok(mut file) = fs::File::open(&full) else {
            report.missing.push(f.path);
            progress.done_bytes.fetch_add(f.size, Ordering::Relaxed);
            continue;
        };
        let Ok(fmeta) = file.metadata() else {
            report.corrupt.push(f.path);
            progress.done_bytes.fetch_add(f.size, Ordering::Relaxed);
            continue;
        };
        if !fmeta.is_file() || fmeta.len() != f.size {
            report.corrupt.push(f.path);
            progress.done_bytes.fetch_add(f.size, Ordering::Relaxed);
            continue;
        }
        let mut hasher = Sha1::new();
        loop {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    hasher.update(&buf[..n]);
                    progress.done_bytes.fetch_add(n as u64, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
        if f.sha1.is_empty() || hasher.finalize().as_slice() == f.sha1.as_slice() {
            report.ok += 1;
        } else {
            report.corrupt.push(f.path);
        }
    }
    report
}

/// Delete a game's files and its app manifest. Refuses anything that isn't
/// a folder inside `<library>/steamapps/common`.
pub fn uninstall(game: &InstalledGame) -> Result<(), String> {
    uninstall_with(game, &Progress::default())
}

/// Uninstall file by file, counting deleted bytes in `progress` (run it
/// off the UI thread: a big game takes a while).
pub fn uninstall_with(game: &InstalledGame, progress: &Progress) -> Result<(), String> {
    let common = game.library.join("steamapps").join("common");
    let dir = &game.install_dir;
    crate::validate_common_child(&common, dir)?;

    match fs::symlink_metadata(&common) {
        Ok(cm) => {
            if crate::is_symlink_or_reparse(&cm) || !cm.is_dir() {
                return Err(format!("refusing to delete {}: {} is not a real directory", dir.display(), common.display()));
            }
            match fs::symlink_metadata(dir) {
                Ok(dm) if crate::is_symlink_or_reparse(&dm) => {
                    // Never walk or recurse into a symlink/junction target: unlink only the link itself.
                    crate::unlink_symlink_or_junction(dir, &dm)
                        .map_err(|e| format!("could not unlink {}: {e}", dir.display()))?;
                }
                Ok(dm) if dm.is_dir() => {
                    let canon_common = common
                        .canonicalize()
                        .map_err(|e| format!("could not resolve {}: {e}", common.display()))?;
                    let canon_dir = dir
                        .canonicalize()
                        .map_err(|e| format!("could not resolve {}: {e}", dir.display()))?;
                    if canon_dir == canon_common || canon_dir.parent() != Some(canon_common.as_path()) {
                        return Err(format!(
                            "refusing to delete {}: canonical path escapes {}",
                            dir.display(),
                            common.display()
                        ));
                    }

                    // List the files first, so the total is known before deleting.
                    let mut files = Vec::new();
                    let mut stack = vec![dir.clone()];
                    while let Some(d) = stack.pop() {
                        let Ok(dm) = fs::symlink_metadata(&d) else { continue };
                        if crate::is_symlink_or_reparse(&dm) || !dm.is_dir() {
                            continue;
                        }
                        let Ok(entries) = fs::read_dir(&d) else { continue };
                        for e in entries.flatten() {
                            let p = e.path();
                            let Ok(em) = fs::symlink_metadata(&p) else { continue };
                            if crate::is_symlink_or_reparse(&em) {
                                files.push((p, 0, true));
                            } else if em.is_dir() {
                                stack.push(p);
                            } else {
                                files.push((p, em.len(), false));
                            }
                        }
                    }
                    progress.total_bytes.store(files.iter().map(|f| f.1).sum::<u64>().max(1), Ordering::Relaxed);
                    for (path, size, is_link) in files {
                        if is_link {
                            if let Ok(m) = fs::symlink_metadata(&path)
                                && crate::is_symlink_or_reparse(&m)
                            {
                                let _ = crate::unlink_symlink_or_junction(&path, &m);
                            }
                        } else if fs::remove_file(&path).is_err() {
                            // Read-only files: clear the flag (only if not a symlink) and try again.
                            if let Ok(m) = fs::symlink_metadata(&path)
                                && !crate::is_symlink_or_reparse(&m)
                            {
                                let mut perms = m.permissions();
                                #[allow(clippy::permissions_set_readonly_false)]
                                perms.set_readonly(false);
                                let _ = fs::set_permissions(&path, perms);
                                let _ = fs::remove_file(&path);
                            }
                        }
                        progress.done_bytes.fetch_add(size, Ordering::Relaxed);
                    }
                    fs::remove_dir_all(dir).map_err(|e| format!("could not delete {}: {e}", dir.display()))?;
                }
                Ok(_) => {
                    return Err(format!("refusing to delete {}: not a directory", dir.display()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("could not inspect {}: {e}", dir.display())),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("could not inspect {}: {e}", common.display())),
    }

    let manifest = manifest_file(game);
    if fs::symlink_metadata(&manifest).is_ok() {
        fs::remove_file(&manifest).map_err(|e| format!("could not delete {}: {e}", manifest.display()))?;
    }
    Ok(())
}

/// Metadata written to `aurora-backup.json` inside an Aurora game backup folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    pub version: u32,
    pub app_id: AppId,
    pub name: String,
    pub install_dir: String,
    pub build_id: u64,
    pub created_at: u64,
    pub total_bytes: u64,
    pub appmanifest: String,
    pub appmanifest_sha256: String,
    #[serde(default)]
    pub dirs: Vec<String>,
    pub files: Vec<BackupFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appworkshop: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appworkshop_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workshop_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workshop_files: Vec<BackupFile>,
}

/// Single file entry recorded in [`BackupManifest`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub executable: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Summary returned by [`inspect_backup`] without copying or walking all game files for Aurora backups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub app_id: AppId,
    pub name: String,
    pub install_dir: String,
    pub build_id: u64,
    pub total_bytes: u64,
    pub is_aurora: bool,
    pub file_count: usize,
}

#[derive(Debug, Clone)]
pub struct MoveReport {
    pub app_id: AppId,
    pub from_library: PathBuf,
    pub to_library: PathBuf,
    pub new_install_dir: PathBuf,
    pub bytes_moved: u64,
    pub files_moved: usize,
    pub workshop_moved: bool,
    pub used_rename: bool,
}

#[derive(Debug, Clone)]
pub struct BackupReport {
    pub app_id: AppId,
    pub backup_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub bytes_copied: u64,
    pub files_copied: usize,
}

#[derive(Debug, Clone)]
pub struct RestoreReport {
    pub app_id: AppId,
    pub name: String,
    pub library: PathBuf,
    pub install_dir: PathBuf,
    pub bytes_restored: u64,
    pub files_restored: usize,
    pub verified_aurora: bool,
}

/// Free disk space in bytes on the volume containing `path` (walking up missing components).
pub fn available_space(path: &Path) -> Option<u64> {
    let mut cur = path;
    loop {
        if cur.exists() {
            return fs4::available_space(cur).ok();
        }
        cur = cur.parent()?;
    }
}

fn next_temp_tag() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{seq}", std::process::id())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn ensure_safe_input_path(path: &Path, what: &str) -> Result<(), String> {
    if !crate::has_only_normal_components(path) {
        return Err(format!("refusing {what}: unsafe path components in {}", path.display()));
    }
    Ok(())
}

fn ensure_real_dir(path: &Path, what: &str) -> Result<PathBuf, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("could not inspect {what} {}: {e}", path.display()))?;
    if crate::is_symlink_or_reparse(&meta) || !meta.is_dir() {
        return Err(format!("refusing {what} {}: not a real directory", path.display()));
    }
    path.canonicalize().map_err(|e| format!("could not resolve {what} {}: {e}", path.display()))
}

fn ensure_real_file(path: &Path, what: &str) -> Result<PathBuf, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("could not inspect {what} {}: {e}", path.display()))?;
    if crate::is_symlink_or_reparse(&meta) || !meta.is_file() {
        return Err(format!("refusing {what} {}: not a regular file", path.display()));
    }
    path.canonicalize().map_err(|e| format!("could not resolve {what} {}: {e}", path.display()))
}

fn ensure_not_exists_or_symlink(path: &Path, what: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(m) if crate::is_symlink_or_reparse(&m) => Err(format!("refusing {what} {}: path is a symlink", path.display())),
        Ok(_) => Err(format!("{what} already exists at {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not inspect {what} {}: {e}", path.display())),
    }
}

fn ensure_real_subdir(parent: &Path, canon_parent: &Path, name: &str, what: &str) -> Result<(PathBuf, PathBuf), String> {
    if !crate::is_valid_folder_name(name) {
        return Err(format!("unsafe folder name {name:?} for {what}"));
    }
    let child = parent.join(name);
    match fs::symlink_metadata(&child) {
        Ok(m) if crate::is_symlink_or_reparse(&m) || !m.is_dir() => {
            return Err(format!("refusing {what} {}: not a real directory", child.display()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&child).map_err(|e| format!("could not create {what} {}: {e}", child.display()))?;
        }
        Err(e) => return Err(format!("could not inspect {what} {}: {e}", child.display())),
    }
    let canon_child = ensure_real_dir(&child, what)?;
    if canon_child == *canon_parent || canon_child.parent() != Some(canon_parent) {
        return Err(format!("refusing {what} {}: canonical path escapes {}", child.display(), parent.display()));
    }
    Ok((child, canon_child))
}

fn validate_rel_path(rel: &str) -> Result<PathBuf, String> {
    if rel.is_empty() || rel.contains('\0') {
        return Err(format!("unsafe relative path {rel:?}"));
    }
    let norm = rel.replace('\\', "/");
    let mut out = PathBuf::new();
    for seg in norm.split('/') {
        if !crate::is_valid_folder_name(seg) {
            return Err(format!("unsafe relative path segment {seg:?} in {rel:?}"));
        }
        out.push(seg);
    }
    if out.as_os_str().is_empty() || !out.components().all(|c| matches!(c, Component::Normal(s) if !s.is_empty())) {
        return Err(format!("unsafe relative path {rel:?}"));
    }
    Ok(out)
}

fn resolve_inside_no_symlinks(root: &Path, canon_root: &Path, rel: &Path) -> Result<PathBuf, String> {
    let mut cur = root.to_path_buf();
    let mut comps = rel.components().peekable();
    while let Some(c) = comps.next() {
        let Component::Normal(seg) = c else {
            return Err(format!("unsafe path component in {}", rel.display()));
        };
        cur.push(seg);
        let is_last = comps.peek().is_none();
        match fs::symlink_metadata(&cur) {
            Ok(m) => {
                if crate::is_symlink_or_reparse(&m) {
                    return Err(format!("refusing symlink at {}", cur.display()));
                }
                if !is_last && !m.is_dir() {
                    return Err(format!("non-directory ancestor at {}", cur.display()));
                }
                let canon = cur.canonicalize().map_err(|e| format!("could not resolve {}: {e}", cur.display()))?;
                if !canon.starts_with(canon_root) || canon == *canon_root {
                    return Err(format!("path escapes root: {}", cur.display()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                for rest in comps {
                    let Component::Normal(s) = rest else {
                        return Err(format!("unsafe path component in {}", rel.display()));
                    };
                    cur.push(s);
                }
                break;
            }
            Err(e) => return Err(format!("could not inspect {}: {e}", cur.display())),
        }
    }
    Ok(cur)
}

fn create_dirs_inside(root: &Path, canon_root: &Path, rel: &Path) -> Result<PathBuf, String> {
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        let Component::Normal(seg) = c else {
            return Err(format!("unsafe path component in {}", rel.display()));
        };
        cur.push(seg);
        match fs::symlink_metadata(&cur) {
            Ok(m) if crate::is_symlink_or_reparse(&m) || !m.is_dir() => {
                return Err(format!("refusing non-directory or symlink at {}", cur.display()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&cur).map_err(|e| format!("could not create directory {}: {e}", cur.display()))?;
            }
            Err(e) => return Err(format!("could not inspect {}: {e}", cur.display())),
        }
        let canon = cur.canonicalize().map_err(|e| format!("could not resolve {}: {e}", cur.display()))?;
        if !canon.starts_with(canon_root) || canon == *canon_root {
            return Err(format!("directory escapes target root: {}", cur.display()));
        }
    }
    Ok(cur)
}

#[derive(Debug, Clone)]
struct WalkedTree {
    dirs: Vec<PathBuf>,
    files: Vec<(PathBuf, u64, bool)>,
    total_bytes: u64,
}

fn walk_dir_no_symlinks(root: &Path) -> Result<WalkedTree, String> {
    let canon_root = ensure_real_dir(root, "directory")?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(root.to_path_buf(), PathBuf::new())];

    while let Some((abs_dir, rel_dir)) = stack.pop() {
        let canon_dir = ensure_real_dir(&abs_dir, "subdirectory")?;
        if canon_dir != canon_root && !canon_dir.starts_with(&canon_root) {
            return Err(format!("directory escapes root: {}", abs_dir.display()));
        }
        let entries = fs::read_dir(&abs_dir).map_err(|e| format!("could not read {}: {e}", abs_dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("could not read entry in {}: {e}", abs_dir.display()))?;
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                return Err(format!("non-UTF8 file name in {}", abs_dir.display()));
            };
            if !crate::is_valid_folder_name(name_str) {
                return Err(format!("unsafe file or folder name {name_str:?} in {}", abs_dir.display()));
            }
            let abs_path = entry.path();
            let rel_path = if rel_dir.as_os_str().is_empty() { PathBuf::from(name_str) } else { rel_dir.join(name_str) };
            let meta = fs::symlink_metadata(&abs_path).map_err(|e| format!("could not inspect {}: {e}", abs_path.display()))?;
            if crate::is_symlink_or_reparse(&meta) {
                return Err(format!("refusing symlink at {}", abs_path.display()));
            }
            let canon_entry = abs_path.canonicalize().map_err(|e| format!("could not resolve {}: {e}", abs_path.display()))?;
            if canon_entry == canon_root || !canon_entry.starts_with(&canon_root) {
                return Err(format!("entry escapes root: {}", abs_path.display()));
            }
            if meta.is_dir() {
                dirs.push(rel_path.clone());
                stack.push((abs_path, rel_path));
            } else if meta.is_file() {
                let len = meta.len();
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = false;
                total_bytes = total_bytes.checked_add(len).ok_or("directory size overflow")?;
                files.push((rel_path, len, executable));
            } else {
                return Err(format!("refusing non-regular file at {}", abs_path.display()));
            }
        }
    }

    dirs.sort();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(WalkedTree { dirs, files, total_bytes })
}

const IO_BUF_BYTES: usize = 256 * 1024;

fn sha256_file(path: &Path) -> Result<(u64, String), String> {
    let mut buf = vec![0u8; IO_BUF_BYTES];
    sha256_file_with_buf(path, &mut buf)
}

fn sha256_file_with_buf(path: &Path, buf: &mut [u8]) -> Result<(u64, String), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("could not inspect {}: {e}", path.display()))?;
    if crate::is_symlink_or_reparse(&meta) || !meta.is_file() {
        return Err(format!("refusing non-regular file or symlink at {}", path.display()));
    }
    let mut f = fs::File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut len = 0u64;
    loop {
        let n = f.read(buf).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    Ok((len, hex_encode(&hasher.finalize())))
}

fn copy_file_verified_with_buf(
    src: &Path,
    dst: &Path,
    expected_len: u64,
    executable: bool,
    progress: &Progress,
    buf: &mut [u8],
) -> Result<String, String> {
    if progress.cancel.load(Ordering::Relaxed) {
        return Err("Operation cancelled.".into());
    }
    let src_meta = fs::symlink_metadata(src).map_err(|e| format!("could not inspect {}: {e}", src.display()))?;
    if crate::is_symlink_or_reparse(&src_meta) || !src_meta.is_file() {
        return Err(format!("refusing symlink or non-regular file {}", src.display()));
    }
    if src_meta.len() != expected_len {
        return Err(format!("file size changed for {} (expected {expected_len}, found {})", src.display(), src_meta.len()));
    }
    let mut src_file = fs::File::open(src).map_err(|e| format!("could not open {}: {e}", src.display()))?;
    let mut dst_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(|e| format!("could not create {}: {e}", dst.display()))?;

    let mut hasher = Sha256::new();
    let mut copied = 0u64;
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            drop(dst_file);
            let _ = fs::remove_file(dst);
            return Err("Operation cancelled.".into());
        }
        let n = src_file.read(buf).map_err(|e| format!("could not read {}: {e}", src.display()))?;
        if n == 0 {
            break;
        }
        dst_file.write_all(&buf[..n]).map_err(|e| format!("could not write {}: {e}", dst.display()))?;
        hasher.update(&buf[..n]);
        copied += n as u64;
        progress.done_bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
    dst_file.flush().map_err(|e| format!("could not flush {}: {e}", dst.display()))?;
    drop(dst_file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = src_meta.permissions().mode() & 0o777;
        if executable {
            mode |= 0o111;
        }
        let _ = fs::set_permissions(dst, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = executable;

    if copied != expected_len {
        let _ = fs::remove_file(dst);
        return Err(format!("short copy for {} ({copied} of {expected_len} bytes)", dst.display()));
    }
    let src_sha = hex_encode(&hasher.finalize());
    let (dst_len, dst_sha) = sha256_file_with_buf(dst, buf)?;
    if dst_len != expected_len || dst_sha != src_sha {
        let _ = fs::remove_file(dst);
        return Err(format!("destination verification failed for {}", dst.display()));
    }
    Ok(src_sha)
}

fn write_bytes_new_verified(dst: &Path, bytes: &[u8]) -> Result<String, String> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(|e| format!("could not create {}: {e}", dst.display()))?;
    f.write_all(bytes).map_err(|e| format!("could not write {}: {e}", dst.display()))?;
    f.sync_all().map_err(|e| format!("could not sync {}: {e}", dst.display()))?;
    drop(f);
    let expected = hex_encode(&Sha256::digest(bytes));
    let (len, got) = sha256_file(dst)?;
    if len != bytes.len() as u64 || got != expected {
        let _ = fs::remove_file(dst);
        return Err(format!("verification failed for {}", dst.display()));
    }
    Ok(expected)
}

fn remove_tree_safe(root: &Path) -> Result<(), String> {
    match fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("could not inspect {}: {e}", root.display())),
        Ok(m) if crate::is_symlink_or_reparse(&m) => return Err(format!("refusing to remove symlink {}", root.display())),
        Ok(m) if !m.is_dir() => return Err(format!("refusing to remove non-directory {}", root.display())),
        Ok(_) => {}
    }
    let canon_root = root.canonicalize().map_err(|e| format!("could not resolve {}: {e}", root.display()))?;
    let mut dirs = vec![root.to_path_buf()];
    let mut post_order = Vec::new();
    while let Some(d) = dirs.pop() {
        let dm = fs::symlink_metadata(&d).map_err(|e| format!("could not inspect {}: {e}", d.display()))?;
        if crate::is_symlink_or_reparse(&dm) || !dm.is_dir() {
            continue;
        }
        let canon_d = d.canonicalize().map_err(|e| format!("could not resolve {}: {e}", d.display()))?;
        if canon_d != canon_root && !canon_d.starts_with(&canon_root) {
            return Err(format!("refusing directory outside root: {}", d.display()));
        }
        post_order.push(d.clone());
        let entries = fs::read_dir(&d).map_err(|e| format!("could not read {}: {e}", d.display()))?;
        for e in entries.flatten() {
            let p = e.path();
            let em = fs::symlink_metadata(&p).map_err(|err| format!("could not inspect {}: {err}", p.display()))?;
            if crate::is_symlink_or_reparse(&em) {
                crate::unlink_symlink_or_junction(&p, &em).map_err(|err| format!("could not unlink {}: {err}", p.display()))?;
            } else if em.is_dir() {
                dirs.push(p);
            } else if fs::remove_file(&p).is_err() {
                let mut perms = em.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = fs::set_permissions(&p, perms);
                fs::remove_file(&p).map_err(|err| format!("could not remove {}: {err}", p.display()))?;
            }
        }
    }
    for d in post_order.into_iter().rev() {
        if fs::remove_dir(&d).is_err() {
            if let Ok(m) = fs::symlink_metadata(&d) {
                let mut perms = m.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = fs::set_permissions(&d, perms);
            }
            fs::remove_dir(&d).map_err(|e| format!("could not remove {}: {e}", d.display()))?;
        }
    }
    Ok(())
}

fn same_filesystem(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) {
            return ma.dev() == mb.dev();
        }
    }
    #[cfg(windows)]
    {
        if let (Ok(ca), Ok(cb)) = (a.canonicalize(), b.canonicalize()) {
            return ca.components().next() == cb.components().next();
        }
    }
    let _ = (a, b);
    false
}

fn is_cross_device_err(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::CrossesDevices {
        return true;
    }
    #[cfg(unix)]
    if e.raw_os_error() == Some(18) {
        return true;
    }
    #[cfg(windows)]
    if e.raw_os_error() == Some(17) {
        return true;
    }
    false
}

struct ValidatedSource {
    folder_name: String,
    canon_lib: PathBuf,
    canon_common: PathBuf,
    canon_dir: PathBuf,
    src_manifest: PathBuf,
    manifest_bytes: Vec<u8>,
    build_id: u64,
    game_name: String,
    game_tree: WalkedTree,
    ws_acf: Option<(PathBuf, Vec<u8>)>,
    ws_content: Option<(PathBuf, WalkedTree)>,
}

fn parse_and_validate_acf(path: &Path, expected_app: Option<AppId>) -> Result<(AppId, String, String, u64, InstallState, Vec<u8>), String> {
    let _ = ensure_real_file(path, "appmanifest")?;
    let bytes = fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("{} is not valid UTF-8", path.display()))?;
    let doc = vdf::parse(text).map_err(|e| format!("could not parse {}: {e}", path.display()))?;
    let app = doc.obj("AppState").ok_or_else(|| format!("{} has no AppState section", path.display()))?;
    let app_id = app.u64("appid").and_then(|v| AppId::try_from(v).ok()).ok_or_else(|| format!("{} has no valid appid", path.display()))?;
    if let Some(exp) = expected_app
        && app_id != exp
    {
        return Err(format!("{} appid mismatch (expected {exp}, found {app_id})", path.display()));
    }
    let installdir = app.str("installdir").ok_or_else(|| format!("{} has no installdir", path.display()))?.to_string();
    if !crate::is_valid_folder_name(&installdir) {
        return Err(format!("{} has unsafe installdir {installdir:?}", path.display()));
    }
    let name = app.str("name").filter(|s| !s.is_empty()).unwrap_or("Unknown").to_string();
    let build_id = app.u64("buildid").unwrap_or(0);
    let state = InstallState::from_flags(app.u64("StateFlags").unwrap_or(0));
    Ok((app_id, name, installdir, build_id, state, bytes))
}

fn validate_game_source(game: &InstalledGame) -> Result<ValidatedSource, String> {
    if game.state == InstallState::Downloading {
        return Err("Wait for the download to finish before modifying this game.".into());
    }
    ensure_safe_input_path(&game.library, "source library")?;
    ensure_safe_input_path(&game.install_dir, "install folder")?;

    let src_steamapps = game.library.join("steamapps");
    let src_common = src_steamapps.join("common");
    crate::validate_common_child(&src_common, &game.install_dir)?;
    let folder_name = game
        .install_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid install folder name".to_string())?
        .to_string();

    let canon_lib = ensure_real_dir(&game.library, "source library")?;
    let canon_steamapps = ensure_real_dir(&src_steamapps, "source steamapps")?;
    let canon_common = ensure_real_dir(&src_common, "source common")?;
    let canon_dir = ensure_real_dir(&game.install_dir, "install folder")?;

    if canon_steamapps.parent() != Some(canon_lib.as_path())
        || canon_common.parent() != Some(canon_steamapps.as_path())
        || canon_dir.parent() != Some(canon_common.as_path())
    {
        return Err(format!("refusing {}: canonical path escapes {}", game.install_dir.display(), game.library.display()));
    }

    let src_manifest = src_steamapps.join(format!("appmanifest_{}.acf", game.app_id));
    let (_, acf_name, acf_dir, build_id, acf_state, manifest_bytes) = parse_and_validate_acf(&src_manifest, Some(game.app_id))?;
    if acf_state == InstallState::Downloading {
        return Err("Wait for the download to finish before modifying this game.".into());
    }
    let dir_matches = if cfg!(windows) { acf_dir.eq_ignore_ascii_case(&folder_name) } else { acf_dir == folder_name };
    if !dir_matches {
        return Err(format!("manifest installdir {acf_dir:?} does not match {}", game.install_dir.display()));
    }
    let game_tree = walk_dir_no_symlinks(&game.install_dir)?;

    let src_ws = src_steamapps.join("workshop");
    let mut ws_acf = None;
    let mut ws_content = None;
    match fs::symlink_metadata(&src_ws) {
        Ok(m) if crate::is_symlink_or_reparse(&m) || !m.is_dir() => {
            return Err(format!("refusing workshop symlink or non-directory {}", src_ws.display()));
        }
        Ok(_) => {
            let acf_p = src_ws.join(format!("appworkshop_{}.acf", game.app_id));
            match fs::symlink_metadata(&acf_p) {
                Ok(_) => {
                    let _ = ensure_real_file(&acf_p, "workshop manifest")?;
                    let bytes = fs::read(&acf_p).map_err(|e| format!("could not read {}: {e}", acf_p.display()))?;
                    ws_acf = Some((acf_p, bytes));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("could not inspect {}: {e}", acf_p.display())),
            }
            let content_root = src_ws.join("content");
            match fs::symlink_metadata(&content_root) {
                Ok(cm) if crate::is_symlink_or_reparse(&cm) || !cm.is_dir() => {
                    return Err(format!("refusing workshop content symlink {}", content_root.display()));
                }
                Ok(_) => {
                    let app_ws = content_root.join(game.app_id.to_string());
                    match fs::symlink_metadata(&app_ws) {
                        Ok(_) => {
                            let tree = walk_dir_no_symlinks(&app_ws)?;
                            ws_content = Some((app_ws, tree));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(format!("could not inspect {}: {e}", app_ws.display())),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("could not inspect {}: {e}", content_root.display())),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("could not inspect {}: {e}", src_ws.display())),
    }

    Ok(ValidatedSource {
        folder_name,
        canon_lib,
        canon_common,
        canon_dir,
        src_manifest,
        manifest_bytes,
        build_id,
        game_name: if game.name.is_empty() { acf_name } else { game.name.clone() },
        game_tree,
        ws_acf,
        ws_content,
    })
}

fn copy_walked_tree(
    src_root: &Path,
    dst_root: &Path,
    tree: &WalkedTree,
    progress: &Progress,
) -> Result<(), String> {
    let canon_dst = ensure_real_dir(dst_root, "destination staging folder")?;
    for rel_dir in &tree.dirs {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err("Operation cancelled.".into());
        }
        create_dirs_inside(dst_root, &canon_dst, rel_dir)?;
    }
    let mut io_buf = vec![0u8; IO_BUF_BYTES];
    for (rel_file, size, exec) in &tree.files {
        let src_file = src_root.join(rel_file);
        let dst_file = resolve_inside_no_symlinks(dst_root, &canon_dst, rel_file)?;
        if let Some(parent) = rel_file.parent()
            && !parent.as_os_str().is_empty()
        {
            create_dirs_inside(dst_root, &canon_dst, parent)?;
        }
        copy_file_verified_with_buf(&src_file, &dst_file, *size, *exec, progress, &mut io_buf)?;
    }
    Ok(())
}

struct DstTargets {
    dst_steamapps: PathBuf,
    canon_dst_common: PathBuf,
    dst_common: PathBuf,
    dst_dir: PathBuf,
    dst_manifest: PathBuf,
    dst_ws_root: Option<PathBuf>,
    dst_ws_content_parent: Option<PathBuf>,
    dst_ws_acf: Option<PathBuf>,
    dst_ws_content: Option<PathBuf>,
}

fn prepare_dst_targets(
    dst_library: &Path,
    canon_dst_lib: &Path,
    app_id: AppId,
    folder_name: &str,
    has_ws_acf: bool,
    has_ws_content: bool,
) -> Result<DstTargets, String> {
    let (dst_steamapps, canon_dst_steamapps) = ensure_real_subdir(dst_library, canon_dst_lib, "steamapps", "destination steamapps")?;
    let (dst_common, canon_dst_common) = ensure_real_subdir(&dst_steamapps, &canon_dst_steamapps, "common", "destination common")?;
    let dst_dir = dst_common.join(folder_name);
    crate::validate_common_child(&dst_common, &dst_dir)?;
    ensure_not_exists_or_symlink(&dst_dir, "destination game folder")?;
    let dst_manifest = dst_steamapps.join(format!("appmanifest_{app_id}.acf"));
    ensure_not_exists_or_symlink(&dst_manifest, "destination appmanifest")?;

    let (dst_ws_root, dst_ws_content_parent, dst_ws_acf, dst_ws_content) = if has_ws_acf || has_ws_content {
        let (ws_root, canon_ws) = ensure_real_subdir(&dst_steamapps, &canon_dst_steamapps, "workshop", "destination workshop")?;
        let acf_dst = if has_ws_acf {
            let p = ws_root.join(format!("appworkshop_{app_id}.acf"));
            ensure_not_exists_or_symlink(&p, "destination workshop manifest")?;
            Some(p)
        } else {
            None
        };
        let (content_parent, content_dst) = if has_ws_content {
            let (cp, _) = ensure_real_subdir(&ws_root, &canon_ws, "content", "destination workshop content")?;
            let cd = cp.join(app_id.to_string());
            ensure_not_exists_or_symlink(&cd, "destination workshop folder")?;
            (Some(cp), Some(cd))
        } else {
            (None, None)
        };
        (Some(ws_root), content_parent, acf_dst, content_dst)
    } else {
        (None, None, None, None)
    };

    Ok(DstTargets {
        dst_steamapps,
        canon_dst_common,
        dst_common,
        dst_dir,
        dst_manifest,
        dst_ws_root,
        dst_ws_content_parent,
        dst_ws_acf,
        dst_ws_content,
    })
}

struct StagedInstall {
    stage_game: PathBuf,
    stage_ws: Option<PathBuf>,
    stage_ws_acf: Option<PathBuf>,
    stage_manifest: PathBuf,
}

impl StagedInstall {
    fn new(targets: &DstTargets, prefix: &str, app_id: AppId, has_ws_content: bool, has_ws_acf: bool) -> Self {
        let tag = next_temp_tag();
        Self {
            stage_game: targets.dst_common.join(format!(".aurora-{prefix}-{app_id}-{tag}")),
            stage_ws: targets.dst_ws_content_parent.as_ref().filter(|_| has_ws_content).map(|p| p.join(format!(".aurora-{prefix}-ws-{app_id}-{tag}"))),
            stage_ws_acf: targets.dst_ws_root.as_ref().filter(|_| has_ws_acf).map(|p| p.join(format!(".aurora-{prefix}-wsacf-{app_id}-{tag}.tmp"))),
            stage_manifest: targets.dst_steamapps.join(format!(".aurora-{prefix}-acf-{app_id}-{tag}.tmp")),
        }
    }

    fn cleanup(&self) {
        let _ = remove_tree_safe(&self.stage_game);
        if let Some(p) = &self.stage_ws {
            let _ = remove_tree_safe(p);
        }
        if let Some(p) = &self.stage_ws_acf {
            let _ = fs::remove_file(p);
        }
        let _ = fs::remove_file(&self.stage_manifest);
    }

    fn commit(self, targets: &DstTargets) -> Result<(), String> {
        if let Err(e) = fs::rename(&self.stage_game, &targets.dst_dir) {
            self.cleanup();
            return Err(format!("could not finalize {}: {e}", targets.dst_dir.display()));
        }
        if let (Some(sws), Some(dws)) = (&self.stage_ws, &targets.dst_ws_content)
            && let Err(e) = fs::rename(sws, dws)
        {
            let _ = remove_tree_safe(&targets.dst_dir);
            self.cleanup();
            return Err(format!("could not finalize workshop {}: {e}", dws.display()));
        }
        if let (Some(sws_acf), Some(dws_acf)) = (&self.stage_ws_acf, &targets.dst_ws_acf)
            && let Err(e) = fs::rename(sws_acf, dws_acf)
        {
            if let Some(dws) = &targets.dst_ws_content {
                let _ = remove_tree_safe(dws);
            }
            let _ = remove_tree_safe(&targets.dst_dir);
            self.cleanup();
            return Err(format!("could not finalize workshop manifest {}: {e}", dws_acf.display()));
        }
        if let Err(e) = fs::rename(&self.stage_manifest, &targets.dst_manifest) {
            if let Some(dws_acf) = &targets.dst_ws_acf {
                let _ = fs::remove_file(dws_acf);
            }
            if let Some(dws) = &targets.dst_ws_content {
                let _ = remove_tree_safe(dws);
            }
            let _ = remove_tree_safe(&targets.dst_dir);
            self.cleanup();
            return Err(format!("could not finalize manifest {}: {e}", targets.dst_manifest.display()));
        }
        Ok(())
    }
}

/// Move an installed game (plus its manifest and any workshop content) to `dst_library`.
pub fn move_game(game: &InstalledGame, dst_library: &Path, progress: &Progress) -> Result<MoveReport, String> {
    move_game_impl(game, dst_library, progress, true)
}

fn move_game_impl(game: &InstalledGame, dst_library: &Path, progress: &Progress, allow_rename: bool) -> Result<MoveReport, String> {
    let src = validate_game_source(game)?;
    ensure_safe_input_path(dst_library, "destination library")?;
    let canon_dst_lib = ensure_real_dir(dst_library, "destination library")?;
    if canon_dst_lib == src.canon_lib {
        return Err("Source and destination library folders are the same.".into());
    }
    if canon_dst_lib.starts_with(&src.canon_dir) || src.canon_dir.starts_with(&canon_dst_lib) {
        return Err("Destination library overlaps with the game folder.".into());
    }

    let targets = prepare_dst_targets(
        dst_library,
        &canon_dst_lib,
        game.app_id,
        &src.folder_name,
        src.ws_acf.is_some(),
        src.ws_content.is_some(),
    )?;
    let has_ws = src.ws_acf.is_some() || src.ws_content.is_some();

    let ws_bytes = src.ws_content.as_ref().map_or(0, |(_, t)| t.total_bytes);
    let acf_bytes = src.manifest_bytes.len() as u64 + src.ws_acf.as_ref().map_or(0, |(_, b)| b.len() as u64);
    let total_bytes = src.game_tree.total_bytes.saturating_add(ws_bytes).saturating_add(acf_bytes);
    progress.total_bytes.store(total_bytes.max(1), Ordering::Relaxed);
    progress.done_bytes.store(0, Ordering::Relaxed);

    if progress.cancel.load(Ordering::Relaxed) {
        return Err("Operation cancelled.".into());
    }

    let files_moved = src.game_tree.files.len() + src.ws_content.as_ref().map_or(0, |(_, t)| t.files.len());

    let ws_same_fs = targets
        .dst_ws_content_parent
        .as_ref()
        .is_none_or(|p| same_filesystem(&src.canon_common, p));

    // Fast path on the same filesystem: atomic rename after full source validation.
    if allow_rename && same_filesystem(&src.canon_common, &targets.canon_dst_common) && ws_same_fs {
        match fs::rename(&game.install_dir, &targets.dst_dir) {
            Ok(()) => {
                let res = (|| -> Result<(), String> {
                    if let (Some((src_ws_dir, _)), Some(dst_ws_dir)) = (&src.ws_content, &targets.dst_ws_content)
                        && let Err(e) = fs::rename(src_ws_dir, dst_ws_dir)
                    {
                        let _ = fs::rename(&targets.dst_dir, &game.install_dir);
                        return Err(format!("could not move workshop content: {e}"));
                    }
                    if let (Some((src_ws_acf, ws_bytes)), Some(dst_acf_p)) = (&src.ws_acf, &targets.dst_ws_acf) {
                        let tmp = targets.dst_ws_root.as_ref().unwrap().join(format!(".aurora-move-wsacf-{}.tmp", next_temp_tag()));
                        if let Err(e) = write_bytes_new_verified(&tmp, ws_bytes).and_then(|_| fs::rename(&tmp, dst_acf_p).map_err(|e| e.to_string())) {
                            let _ = fs::remove_file(&tmp);
                            if let (Some((src_ws_dir, _)), Some(dst_ws_dir)) = (&src.ws_content, &targets.dst_ws_content) {
                                let _ = fs::rename(dst_ws_dir, src_ws_dir);
                            }
                            let _ = fs::rename(&targets.dst_dir, &game.install_dir);
                            return Err(e);
                        }
                        let _ = fs::remove_file(src_ws_acf);
                    }
                    let tmp_acf = targets.dst_steamapps.join(format!(".aurora-move-acf-{}.tmp", next_temp_tag()));
                    if let Err(e) = write_bytes_new_verified(&tmp_acf, &src.manifest_bytes).and_then(|_| fs::rename(&tmp_acf, &targets.dst_manifest).map_err(|e| e.to_string())) {
                        let _ = fs::remove_file(&tmp_acf);
                        if let Some(dst_acf_p) = &targets.dst_ws_acf {
                            let _ = fs::remove_file(dst_acf_p);
                        }
                        if let (Some((src_ws_dir, _)), Some(dst_ws_dir)) = (&src.ws_content, &targets.dst_ws_content) {
                            let _ = fs::rename(dst_ws_dir, src_ws_dir);
                        }
                        let _ = fs::rename(&targets.dst_dir, &game.install_dir);
                        return Err(e);
                    }
                    fs::remove_file(&src.src_manifest).map_err(|e| format!("could not remove old manifest {}: {e}", src.src_manifest.display()))?;
                    Ok(())
                })();
                res?;
                progress.done_bytes.store(total_bytes.max(1), Ordering::Relaxed);
                return Ok(MoveReport {
                    app_id: game.app_id,
                    from_library: game.library.clone(),
                    to_library: dst_library.to_path_buf(),
                    new_install_dir: targets.dst_dir,
                    bytes_moved: total_bytes,
                    files_moved,
                    workshop_moved: has_ws,
                    used_rename: true,
                });
            }
            Err(e) if is_cross_device_err(&e) => {}
            Err(e) => return Err(format!("could not move {}: {e}", game.install_dir.display())),
        }
    }

    // Cross-filesystem path: check free space, copy & SHA-256 verify in staging, swap, then remove source.
    if let Some(avail) = available_space(&targets.canon_dst_common)
        && avail < total_bytes
    {
        return Err(format!("Not enough free space in destination library (need {total_bytes} bytes, {avail} available)."));
    }

    let staged = StagedInstall::new(&targets, "move", game.app_id, src.ws_content.is_some(), src.ws_acf.is_some());
    let stage_res = (|| -> Result<(), String> {
        fs::create_dir(&staged.stage_game).map_err(|e| format!("could not create staging dir {}: {e}", staged.stage_game.display()))?;
        copy_walked_tree(&game.install_dir, &staged.stage_game, &src.game_tree, progress)?;

        if let (Some((src_ws_dir, ws_tree)), Some(sws)) = (&src.ws_content, &staged.stage_ws) {
            fs::create_dir(sws).map_err(|e| format!("could not create workshop staging {}: {e}", sws.display()))?;
            copy_walked_tree(src_ws_dir, sws, ws_tree, progress)?;
        }
        if let (Some((_, ws_bytes)), Some(sws_acf)) = (&src.ws_acf, &staged.stage_ws_acf) {
            write_bytes_new_verified(sws_acf, ws_bytes)?;
            progress.done_bytes.fetch_add(ws_bytes.len() as u64, Ordering::Relaxed);
        }
        write_bytes_new_verified(&staged.stage_manifest, &src.manifest_bytes)?;
        progress.done_bytes.fetch_add(src.manifest_bytes.len() as u64, Ordering::Relaxed);

        if progress.cancel.load(Ordering::Relaxed) {
            return Err("Operation cancelled.".into());
        }
        Ok(())
    })();

    if let Err(e) = stage_res {
        staged.cleanup();
        return Err(e);
    }

    staged.commit(&targets)?;

    // Verify destination manifest parses cleanly before removing source.
    if let Err(e) = parse_and_validate_acf(&targets.dst_manifest, Some(game.app_id)) {
        return Err(format!("destination verification failed: {e}"));
    }

    // Remove source only after destination succeeded and verified.
    fs::remove_file(&src.src_manifest).map_err(|e| format!("moved game, but could not remove {}: {e}", src.src_manifest.display()))?;
    if let Some((src_ws_acf, _)) = &src.ws_acf {
        fs::remove_file(src_ws_acf).map_err(|e| format!("moved game, but could not remove {}: {e}", src_ws_acf.display()))?;
    }
    if let Some((src_ws_dir, _)) = &src.ws_content {
        remove_tree_safe(src_ws_dir)?;
    }
    remove_tree_safe(&game.install_dir)?;

    if total_bytes == 0 {
        progress.done_bytes.store(1, Ordering::Relaxed);
    }

    Ok(MoveReport {
        app_id: game.app_id,
        from_library: game.library.clone(),
        to_library: dst_library.to_path_buf(),
        new_install_dir: targets.dst_dir,
        bytes_moved: total_bytes,
        files_moved,
        workshop_moved: has_ws,
        used_rename: false,
    })
}

/// Back up an installed game (`appmanifest_<id>.acf`, `common/<installdir>`, and `aurora-backup.json`)
/// into `backup_dir`.
pub fn backup_game(game: &InstalledGame, backup_dir: &Path, progress: &Progress) -> Result<BackupReport, String> {
    let src = validate_game_source(game)?;
    ensure_safe_input_path(backup_dir, "backup folder")?;

    match fs::symlink_metadata(backup_dir) {
        Ok(m) if crate::is_symlink_or_reparse(&m) || !m.is_dir() => {
            return Err(format!("refusing backup folder {}: not a real directory", backup_dir.display()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = backup_dir.parent().ok_or_else(|| format!("backup folder {} has no parent", backup_dir.display()))?;
            let _ = ensure_real_dir(parent, "backup parent folder")?;
            fs::create_dir(backup_dir).map_err(|e| format!("could not create backup folder {}: {e}", backup_dir.display()))?;
        }
        Err(e) => return Err(format!("could not inspect backup folder {}: {e}", backup_dir.display())),
    }

    let canon_backup = ensure_real_dir(backup_dir, "backup folder")?;
    if canon_backup.starts_with(&src.canon_dir) || src.canon_dir.starts_with(&canon_backup) {
        return Err("Backup folder cannot overlap with the game install folder.".into());
    }

    struct Reservation(PathBuf);
    impl Drop for Reservation {
        fn drop(&mut self) {
            let _ = fs::remove_dir(&self.0);
        }
    }
    let lock = backup_dir.join(".aurora-backup.lock");
    fs::create_dir(&lock).map_err(|e| format!("could not reserve backup destination {}: {e}", lock.display()))?;
    let _reservation = Reservation(lock);

    let final_meta = backup_dir.join("aurora-backup.json");
    let acf_name = format!("appmanifest_{}.acf", game.app_id);
    let final_acf = backup_dir.join(&acf_name);
    let final_common = backup_dir.join("common");
    let final_game_dir = final_common.join(&src.folder_name);

    ensure_not_exists_or_symlink(&final_meta, "aurora-backup.json")?;
    ensure_not_exists_or_symlink(&final_acf, "backup appmanifest")?;
    if fs::symlink_metadata(&final_common).is_ok() {
        let _ = ensure_real_dir(&final_common, "backup common")?;
        ensure_not_exists_or_symlink(&final_game_dir, "backup game folder")?;
    }

    let has_ws = src.ws_acf.is_some() || src.ws_content.is_some();
    if has_ws && fs::symlink_metadata(backup_dir.join("workshop")).is_ok() {
        ensure_not_exists_or_symlink(&backup_dir.join("workshop"), "backup workshop folder")?;
    }

    let ws_bytes = src.ws_content.as_ref().map_or(0, |(_, t)| t.total_bytes);
    let acf_bytes = src.manifest_bytes.len() as u64 + src.ws_acf.as_ref().map_or(0, |(_, b)| b.len() as u64);
    let total_bytes = src.game_tree.total_bytes.saturating_add(ws_bytes).saturating_add(acf_bytes);
    if let Some(avail) = available_space(&canon_backup)
        && avail < total_bytes
    {
        return Err(format!("Not enough free space in backup folder (need {total_bytes} bytes, {avail} available)."));
    }

    progress.total_bytes.store(total_bytes.max(1), Ordering::Relaxed);
    progress.done_bytes.store(0, Ordering::Relaxed);

    let stage_root = backup_dir.join(format!(".aurora-backup-{}-{}", game.app_id, next_temp_tag()));
    fs::create_dir(&stage_root).map_err(|e| format!("could not create staging dir {}: {e}", stage_root.display()))?;
    let cleanup = || {
        let _ = remove_tree_safe(&stage_root);
    };

    let stage_res = (|| -> Result<BackupManifest, String> {
        let canon_stage = ensure_real_dir(&stage_root, "backup staging")?;
        let (stage_common, canon_stage_common) = ensure_real_subdir(&stage_root, &canon_stage, "common", "staging common")?;
        let (stage_game, canon_stage_game) = ensure_real_subdir(&stage_common, &canon_stage_common, &src.folder_name, "staging game")?;

        let mut dirs = Vec::with_capacity(src.game_tree.dirs.len());
        for rel_dir in &src.game_tree.dirs {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err("Operation cancelled.".into());
            }
            create_dirs_inside(&stage_game, &canon_stage_game, rel_dir)?;
            dirs.push(rel_dir.to_string_lossy().replace('\\', "/"));
        }

        let mut io_buf = vec![0u8; IO_BUF_BYTES];
        let mut files = Vec::with_capacity(src.game_tree.files.len());
        for (rel_file, size, exec) in &src.game_tree.files {
            if let Some(p) = rel_file.parent()
                && !p.as_os_str().is_empty()
            {
                create_dirs_inside(&stage_game, &canon_stage_game, p)?;
            }
            let src_file = game.install_dir.join(rel_file);
            let dst_file = resolve_inside_no_symlinks(&stage_game, &canon_stage_game, rel_file)?;
            let sha256 = copy_file_verified_with_buf(&src_file, &dst_file, *size, *exec, progress, &mut io_buf)?;
            files.push(BackupFile {
                path: rel_file.to_string_lossy().replace('\\', "/"),
                size: *size,
                sha256,
                executable: *exec,
            });
        }

        let mut appworkshop = None;
        let mut appworkshop_sha256 = None;
        let mut workshop_dirs = Vec::new();
        let mut workshop_files = Vec::new();
        if has_ws {
            let (stage_ws, canon_stage_ws) = ensure_real_subdir(&stage_root, &canon_stage, "workshop", "staging workshop")?;
            if let Some((_, ws_acf_bytes)) = &src.ws_acf {
                let ws_acf_name = format!("appworkshop_{}.acf", game.app_id);
                let sha = write_bytes_new_verified(&stage_ws.join(&ws_acf_name), ws_acf_bytes)?;
                progress.done_bytes.fetch_add(ws_acf_bytes.len() as u64, Ordering::Relaxed);
                appworkshop = Some(ws_acf_name);
                appworkshop_sha256 = Some(sha);
            }
            if let Some((src_ws_dir, ws_tree)) = &src.ws_content {
                let (stage_ws_content, canon_ws_content) = ensure_real_subdir(&stage_ws, &canon_stage_ws, "content", "staging workshop content")?;
                let (stage_ws_app, canon_ws_app) = ensure_real_subdir(&stage_ws_content, &canon_ws_content, &game.app_id.to_string(), "staging workshop app")?;
                for rel_dir in &ws_tree.dirs {
                    create_dirs_inside(&stage_ws_app, &canon_ws_app, rel_dir)?;
                    workshop_dirs.push(rel_dir.to_string_lossy().replace('\\', "/"));
                }
                for (rel_file, size, exec) in &ws_tree.files {
                    if let Some(p) = rel_file.parent()
                        && !p.as_os_str().is_empty()
                    {
                        create_dirs_inside(&stage_ws_app, &canon_ws_app, p)?;
                    }
                    let src_file = src_ws_dir.join(rel_file);
                    let dst_file = resolve_inside_no_symlinks(&stage_ws_app, &canon_ws_app, rel_file)?;
                    let sha256 = copy_file_verified_with_buf(&src_file, &dst_file, *size, *exec, progress, &mut io_buf)?;
                    workshop_files.push(BackupFile {
                        path: rel_file.to_string_lossy().replace('\\', "/"),
                        size: *size,
                        sha256,
                        executable: *exec,
                    });
                }
            }
        }

        let appmanifest_sha256 = write_bytes_new_verified(&stage_root.join(&acf_name), &src.manifest_bytes)?;
        progress.done_bytes.fetch_add(src.manifest_bytes.len() as u64, Ordering::Relaxed);

        let mut seen_files = HashSet::with_capacity(files.len());
        for f in &files {
            let rel = validate_rel_path(&f.path)?;
            if !seen_files.insert(dedup_rel_key(&rel)) {
                return Err(format!("cannot create cross-platform backup: case-colliding file path {:?}", f.path));
            }
        }
        let mut seen_ws = HashSet::with_capacity(workshop_files.len());
        for f in &workshop_files {
            let rel = validate_rel_path(&f.path)?;
            if !seen_ws.insert(dedup_rel_key(&rel)) {
                return Err(format!("cannot create cross-platform backup: case-colliding workshop path {:?}", f.path));
            }
        }

        let created_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let manifest = BackupManifest {
            version: 1,
            app_id: game.app_id,
            name: src.game_name.clone(),
            install_dir: src.folder_name.clone(),
            build_id: src.build_id,
            created_at,
            total_bytes: src.game_tree.total_bytes.saturating_add(ws_bytes),
            appmanifest: acf_name.clone(),
            appmanifest_sha256,
            dirs,
            files,
            appworkshop,
            appworkshop_sha256,
            workshop_dirs,
            workshop_files,
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| format!("could not serialize backup manifest: {e}"))?;
        if json.len() as u64 > MAX_BACKUP_MANIFEST_BYTES {
            return Err(format!("backup manifest exceeds {} byte limit", MAX_BACKUP_MANIFEST_BYTES));
        }
        write_bytes_new_verified(&stage_root.join("aurora-backup.json"), &json)?;

        if progress.cancel.load(Ordering::Relaxed) {
            return Err("Operation cancelled.".into());
        }
        Ok(manifest)
    })();

    let manifest = match stage_res {
        Ok(m) => m,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };

    // Move staged artifacts into backup_dir cleanly.
    let (_, canon_backup_common) = match ensure_real_subdir(backup_dir, &canon_backup, "common", "backup common") {
        Ok(x) => x,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    let _ = canon_backup_common;
    let mut artifacts = vec![(stage_root.join("common").join(&src.folder_name), final_game_dir, true)];
    if has_ws {
        artifacts.push((stage_root.join("workshop"), backup_dir.join("workshop"), true));
    }
    artifacts.push((stage_root.join(&acf_name), final_acf, false));
    artifacts.push((stage_root.join("aurora-backup.json"), final_meta.clone(), false));
    let published = publish_backup(&artifacts);
    cleanup();
    published?;

    if total_bytes == 0 {
        progress.done_bytes.store(1, Ordering::Relaxed);
    }

    Ok(BackupReport {
        app_id: game.app_id,
        backup_dir: backup_dir.to_path_buf(),
        manifest_path: final_meta,
        bytes_copied: total_bytes,
        files_copied: manifest.files.len() + manifest.workshop_files.len(),
    })
}

fn publish_backup(artifacts: &[(PathBuf, PathBuf, bool)]) -> Result<(), String> {
    for (index, (source, dest, _)) in artifacts.iter().enumerate() {
        if let Err(e) = fs::rename(source, dest) {
            for (_, moved, directory) in artifacts[..index].iter().rev() {
                if *directory {
                    let _ = remove_tree_safe(moved);
                } else {
                    let _ = fs::remove_file(moved);
                }
            }
            return Err(format!("could not finalize {}: {e}", dest.display()));
        }
    }
    Ok(())
}

enum BackupLayout {
    Aurora {
        manifest: BackupManifest,
        game_dir: PathBuf,
        acf_path: PathBuf,
        ws_acf_path: Option<PathBuf>,
        ws_content_dir: Option<PathBuf>,
    },
    Standard {
        app_id: AppId,
        name: String,
        install_dir: String,
        build_id: u64,
        acf_bytes: Vec<u8>,
        game_dir: PathBuf,
        ws_acf: Option<(PathBuf, Vec<u8>)>,
        ws_content_dir: Option<PathBuf>,
        estimated_bytes: u64,
    },
}

const MAX_BACKUP_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

fn dedup_rel_key(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn read_and_validate_aurora_backup(backup_dir: &Path) -> Result<BackupLayout, String> {
    let meta_path = backup_dir.join("aurora-backup.json");
    let _ = ensure_real_file(&meta_path, "aurora-backup.json")?;
    let file = fs::File::open(&meta_path).map_err(|e| format!("could not read aurora-backup.json: {e}"))?;
    let mut raw_bytes = Vec::new();
    file.take(MAX_BACKUP_MANIFEST_BYTES + 1)
        .read_to_end(&mut raw_bytes)
        .map_err(|e| format!("could not read aurora-backup.json: {e}"))?;
    if raw_bytes.len() as u64 > MAX_BACKUP_MANIFEST_BYTES {
        return Err(format!("aurora-backup.json exceeds {} byte limit", MAX_BACKUP_MANIFEST_BYTES));
    }
    let raw = String::from_utf8(raw_bytes).map_err(|e| format!("invalid UTF-8 in aurora-backup.json: {e}"))?;
    let manifest: BackupManifest = serde_json::from_str(&raw).map_err(|e| format!("invalid aurora-backup.json: {e}"))?;

    if manifest.version != 1 {
        return Err(format!("unsupported backup manifest version {}", manifest.version));
    }
    if manifest.app_id == 0 {
        return Err("invalid app_id in aurora-backup.json".into());
    }
    if !crate::is_valid_folder_name(&manifest.install_dir) {
        return Err(format!("unsafe install_dir {:?} in aurora-backup.json", manifest.install_dir));
    }
    let expected_acf = format!("appmanifest_{}.acf", manifest.app_id);
    if manifest.appmanifest != expected_acf {
        return Err(format!("unsafe appmanifest {:?} in aurora-backup.json", manifest.appmanifest));
    }
    if !is_valid_sha256_hex(&manifest.appmanifest_sha256) {
        return Err("invalid appmanifest_sha256 in aurora-backup.json".into());
    }

    let acf_path = backup_dir.join(&expected_acf);
    let (_, _, acf_installdir, _, _, _) = parse_and_validate_acf(&acf_path, Some(manifest.app_id))?;
    let dir_matches = if cfg!(windows) {
        acf_installdir.eq_ignore_ascii_case(&manifest.install_dir)
    } else {
        acf_installdir == manifest.install_dir
    };
    if !dir_matches {
        return Err("appmanifest installdir does not match aurora-backup.json".into());
    }
    let (_, actual_acf_sha) = sha256_file(&acf_path)?;
    if !actual_acf_sha.eq_ignore_ascii_case(&manifest.appmanifest_sha256) {
        return Err("backup integrity check failed: appmanifest SHA-256 mismatch".into());
    }

    let common_dir = backup_dir.join("common");
    let _ = ensure_real_dir(&common_dir, "backup common")?;
    let game_dir = common_dir.join(&manifest.install_dir);
    crate::validate_common_child(&common_dir, &game_dir)?;
    let canon_common = ensure_real_dir(&common_dir, "backup common")?;
    let canon_game = ensure_real_dir(&game_dir, "backup game folder")?;
    if canon_game.parent() != Some(canon_common.as_path()) {
        return Err("backup game folder escapes common directory".into());
    }

    for d in &manifest.dirs {
        validate_rel_path(d)?;
    }
    let mut seen = HashSet::with_capacity(manifest.files.len());
    for f in &manifest.files {
        let rel = validate_rel_path(&f.path)?;
        if !is_valid_sha256_hex(&f.sha256) {
            return Err(format!("invalid sha256 for {:?} in aurora-backup.json", f.path));
        }
        if !seen.insert(dedup_rel_key(&rel)) {
            return Err(format!("duplicate file entry {:?} in aurora-backup.json", f.path));
        }
    }

    let mut ws_acf_path = None;
    let mut ws_content_dir = None;
    if manifest.appworkshop.is_some() || !manifest.workshop_files.is_empty() || !manifest.workshop_dirs.is_empty() {
        let ws_root = backup_dir.join("workshop");
        let _ = ensure_real_dir(&ws_root, "backup workshop")?;
        if let Some(ws_acf_name) = &manifest.appworkshop {
            let expected_ws = format!("appworkshop_{}.acf", manifest.app_id);
            if ws_acf_name != &expected_ws {
                return Err(format!("unsafe appworkshop name {ws_acf_name:?}"));
            }
            let sha_want = manifest.appworkshop_sha256.as_deref().ok_or("missing appworkshop_sha256")?;
            if !is_valid_sha256_hex(sha_want) {
                return Err("invalid appworkshop_sha256".into());
            }
            let p = ws_root.join(ws_acf_name);
            let (_, actual) = sha256_file(&p)?;
            if !actual.eq_ignore_ascii_case(sha_want) {
                return Err("backup integrity check failed: appworkshop SHA-256 mismatch".into());
            }
            ws_acf_path = Some(p);
        }
        for d in &manifest.workshop_dirs {
            validate_rel_path(d)?;
        }
        let mut ws_seen = HashSet::with_capacity(manifest.workshop_files.len());
        for f in &manifest.workshop_files {
            let rel = validate_rel_path(&f.path)?;
            if !is_valid_sha256_hex(&f.sha256) {
                return Err(format!("invalid workshop sha256 for {:?}", f.path));
            }
            if !ws_seen.insert(dedup_rel_key(&rel)) {
                return Err(format!("duplicate workshop file {:?}", f.path));
            }
        }
        if !manifest.workshop_files.is_empty() || !manifest.workshop_dirs.is_empty() {
            let content = ws_root.join("content");
            let _ = ensure_real_dir(&content, "backup workshop content")?;
            let app_ws = content.join(manifest.app_id.to_string());
            let _ = ensure_real_dir(&app_ws, "backup workshop app folder")?;
            ws_content_dir = Some(app_ws);
        }
    }

    Ok(BackupLayout::Aurora { manifest, game_dir, acf_path, ws_acf_path, ws_content_dir })
}

fn find_standard_backup(backup_dir: &Path) -> Result<BackupLayout, String> {
    let mut candidates = Vec::new();
    let mut search_roots = vec![backup_dir.to_path_buf()];
    let steamapps_sub = backup_dir.join("steamapps");
    if let Ok(m) = fs::symlink_metadata(&steamapps_sub) {
        if crate::is_symlink_or_reparse(&m) {
            return Err(format!("refusing symlink at {}", steamapps_sub.display()));
        }
        if m.is_dir() {
            search_roots.push(steamapps_sub);
        }
    }

    for root in search_roots {
        let entries = fs::read_dir(&root).map_err(|e| format!("could not read {}: {e}", root.display()))?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(id_str) = name.strip_prefix("appmanifest_").and_then(|s| s.strip_suffix(".acf")) else {
                continue;
            };
            let Ok(id) = id_str.parse::<AppId>() else { continue };
            let acf_path = entry.path();
            let (parsed_id, game_name, install_dir, build_id, _, acf_bytes) = parse_and_validate_acf(&acf_path, Some(id))?;
            let common_candidate = root.join("common").join(&install_dir);
            let direct_candidate = root.join(&install_dir);
            let game_dir = if fs::symlink_metadata(&common_candidate).is_ok() {
                if let Ok(cm) = fs::symlink_metadata(&root.join("common"))
                    && (crate::is_symlink_or_reparse(&cm) || !cm.is_dir())
                {
                    return Err(format!("refusing symlink common directory in {}", root.display()));
                }
                let _ = ensure_real_dir(&common_candidate, "backup game folder")?;
                common_candidate
            } else if root == backup_dir && fs::symlink_metadata(&direct_candidate).is_ok() {
                let _ = ensure_real_dir(&direct_candidate, "backup game folder")?;
                direct_candidate
            } else {
                continue;
            };

            let ws_root = root.join("workshop");
            let mut ws_acf = None;
            let mut ws_content_dir = None;
            if let Ok(wm) = fs::symlink_metadata(&ws_root)
                && !crate::is_symlink_or_reparse(&wm)
                && wm.is_dir()
            {
                let wp = ws_root.join(format!("appworkshop_{parsed_id}.acf"));
                if fs::symlink_metadata(&wp).is_ok() {
                    let _ = ensure_real_file(&wp, "workshop manifest")?;
                    if let Ok(bytes) = fs::read(&wp) {
                        ws_acf = Some((wp, bytes));
                    }
                }
                let ws_content_root = ws_root.join("content");
                if fs::symlink_metadata(&ws_content_root).is_ok() {
                    let _ = ensure_real_dir(&ws_content_root, "workshop content root")?;
                    let wc = ws_content_root.join(parsed_id.to_string());
                    if fs::symlink_metadata(&wc).is_ok() {
                        let _ = ensure_real_dir(&wc, "workshop content")?;
                        ws_content_dir = Some(wc);
                    }
                }
            }
            let estimated_bytes = vdf::parse(std::str::from_utf8(&acf_bytes).unwrap_or(""))
                .ok()
                .and_then(|d| d.obj("AppState").and_then(|a| a.u64("SizeOnDisk")))
                .unwrap_or(0);
            candidates.push(BackupLayout::Standard {
                app_id: parsed_id,
                name: game_name,
                install_dir,
                build_id,
                acf_bytes,
                game_dir,
                ws_acf,
                ws_content_dir,
                estimated_bytes,
            });
        }
    }

    match candidates.len() {
        0 => {
            if backup_dir.join("sku.sis").exists() {
                Err("Valve compressed .sis backup archives aren't supported; choose an Aurora backup or a folder containing appmanifest_*.acf and game files.".into())
            } else {
                Err("No valid game backup found in this folder (expected aurora-backup.json or appmanifest_*.acf with game files).".into())
            }
        }
        1 => Ok(candidates.into_iter().next().unwrap()),
        _ => Err("Multiple game backups found in this folder; choose a specific game backup folder.".into()),
    }
}

fn inspect_backup_layout(backup_dir: &Path) -> Result<BackupLayout, String> {
    ensure_safe_input_path(backup_dir, "backup folder")?;
    let _ = ensure_real_dir(backup_dir, "backup folder")?;
    if fs::symlink_metadata(backup_dir.join("aurora-backup.json")).is_ok() {
        read_and_validate_aurora_backup(backup_dir)
    } else {
        find_standard_backup(backup_dir)
    }
}

/// Inspect a backup folder quickly for UI confirmation before restoring.
pub fn inspect_backup(backup_dir: &Path) -> Result<BackupInfo, String> {
    match inspect_backup_layout(backup_dir)? {
        BackupLayout::Aurora { manifest, .. } => Ok(BackupInfo {
            app_id: manifest.app_id,
            name: manifest.name,
            install_dir: manifest.install_dir,
            build_id: manifest.build_id,
            total_bytes: manifest.total_bytes,
            is_aurora: true,
            file_count: manifest.files.len(),
        }),
        BackupLayout::Standard { app_id, name, install_dir, build_id, estimated_bytes, .. } => Ok(BackupInfo {
            app_id,
            name,
            install_dir,
            build_id,
            total_bytes: estimated_bytes,
            is_aurora: false,
            file_count: 0,
        }),
    }
}

/// Restore a game backup (Aurora `aurora-backup.json` or standard Steam `.acf` + game files) into `dst_library`.
pub fn restore_game(backup_dir: &Path, dst_library: &Path, progress: &Progress) -> Result<RestoreReport, String> {
    let layout = inspect_backup_layout(backup_dir)?;
    ensure_safe_input_path(dst_library, "destination library")?;
    let canon_backup = ensure_real_dir(backup_dir, "backup folder")?;
    let canon_dst_lib = ensure_real_dir(dst_library, "destination library")?;

    match layout {
        BackupLayout::Aurora { manifest, game_dir, acf_path, ws_acf_path, ws_content_dir } => {
            // Reject any symlinks or unsafe entries inside the backup tree before copying.
            let _ = walk_dir_no_symlinks(&game_dir)?;
            if let Some(ws_dir) = &ws_content_dir {
                let _ = walk_dir_no_symlinks(ws_dir)?;
            }

            let targets = prepare_dst_targets(
                dst_library,
                &canon_dst_lib,
                manifest.app_id,
                &manifest.install_dir,
                ws_acf_path.is_some(),
                ws_content_dir.is_some(),
            )?;
            if targets.canon_dst_common.starts_with(&canon_backup) || canon_backup.starts_with(&targets.canon_dst_common) {
                return Err("Backup folder and destination library common folder cannot overlap.".into());
            }

            let acf_bytes = fs::read(&acf_path).map_err(|e| format!("could not read {}: {e}", acf_path.display()))?;
            let ws_acf_bytes = match &ws_acf_path {
                Some(p) => Some(fs::read(p).map_err(|e| format!("could not read {}: {e}", p.display()))?),
                None => None,
            };
            let total_bytes = manifest
                .files
                .iter()
                .map(|f| f.size)
                .sum::<u64>()
                .saturating_add(manifest.workshop_files.iter().map(|f| f.size).sum::<u64>())
                .saturating_add(acf_bytes.len() as u64)
                .saturating_add(ws_acf_bytes.as_ref().map_or(0, |b| b.len() as u64));
            if let Some(avail) = available_space(&targets.canon_dst_common)
                && avail < total_bytes
            {
                return Err(format!("Not enough free space in destination library (need {total_bytes} bytes, {avail} available)."));
            }

            progress.total_bytes.store(total_bytes.max(1), Ordering::Relaxed);
            progress.done_bytes.store(0, Ordering::Relaxed);

            let staged = StagedInstall::new(&targets, "restore", manifest.app_id, ws_content_dir.is_some(), ws_acf_bytes.is_some());
            let canon_backup_game = ensure_real_dir(&game_dir, "backup game folder")?;
            let stage_res = (|| -> Result<(), String> {
                fs::create_dir(&staged.stage_game).map_err(|e| format!("could not create staging dir {}: {e}", staged.stage_game.display()))?;
                let canon_stage = ensure_real_dir(&staged.stage_game, "restore staging")?;
                for d in &manifest.dirs {
                    let rel = validate_rel_path(d)?;
                    create_dirs_inside(&staged.stage_game, &canon_stage, &rel)?;
                }
                let mut io_buf = vec![0u8; IO_BUF_BYTES];
                for f in &manifest.files {
                    let rel = validate_rel_path(&f.path)?;
                    if let Some(p) = rel.parent()
                        && !p.as_os_str().is_empty()
                    {
                        create_dirs_inside(&staged.stage_game, &canon_stage, p)?;
                    }
                    let src_file = resolve_inside_no_symlinks(&game_dir, &canon_backup_game, &rel)?;
                    let dst_file = resolve_inside_no_symlinks(&staged.stage_game, &canon_stage, &rel)?;
                    let got_sha = copy_file_verified_with_buf(&src_file, &dst_file, f.size, f.executable, progress, &mut io_buf)?;
                    if !got_sha.eq_ignore_ascii_case(&f.sha256) {
                        return Err(format!("Backup integrity check failed for {}: SHA-256 mismatch.", f.path));
                    }
                }
                if let (Some(src_ws_dir), Some(sws)) = (&ws_content_dir, &staged.stage_ws) {
                    let canon_ws_src = ensure_real_dir(src_ws_dir, "backup workshop content")?;
                    fs::create_dir(sws).map_err(|e| format!("could not create workshop staging {}: {e}", sws.display()))?;
                    let canon_sws = ensure_real_dir(sws, "workshop staging")?;
                    for d in &manifest.workshop_dirs {
                        let rel = validate_rel_path(d)?;
                        create_dirs_inside(sws, &canon_sws, &rel)?;
                    }
                    for f in &manifest.workshop_files {
                        let rel = validate_rel_path(&f.path)?;
                        if let Some(p) = rel.parent()
                            && !p.as_os_str().is_empty()
                        {
                            create_dirs_inside(sws, &canon_sws, p)?;
                        }
                        let src_file = resolve_inside_no_symlinks(src_ws_dir, &canon_ws_src, &rel)?;
                        let dst_file = resolve_inside_no_symlinks(sws, &canon_sws, &rel)?;
                        let got_sha = copy_file_verified_with_buf(&src_file, &dst_file, f.size, f.executable, progress, &mut io_buf)?;
                        if !got_sha.eq_ignore_ascii_case(&f.sha256) {
                            return Err(format!("Backup integrity check failed for workshop {}: SHA-256 mismatch.", f.path));
                        }
                    }
                }
                if let (Some(bytes), Some(sws_acf)) = (&ws_acf_bytes, &staged.stage_ws_acf) {
                    let got = write_bytes_new_verified(sws_acf, bytes)?;
                    if !got.eq_ignore_ascii_case(manifest.appworkshop_sha256.as_deref().unwrap_or("")) {
                        return Err("Backup integrity check failed for appworkshop manifest.".into());
                    }
                    progress.done_bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                }
                let got_acf = write_bytes_new_verified(&staged.stage_manifest, &acf_bytes)?;
                if !got_acf.eq_ignore_ascii_case(&manifest.appmanifest_sha256) {
                    return Err("Backup integrity check failed for appmanifest.".into());
                }
                progress.done_bytes.fetch_add(acf_bytes.len() as u64, Ordering::Relaxed);
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err("Operation cancelled.".into());
                }
                Ok(())
            })();

            if let Err(e) = stage_res {
                staged.cleanup();
                return Err(e);
            }

            staged.commit(&targets)?;
            if total_bytes == 0 {
                progress.done_bytes.store(1, Ordering::Relaxed);
            }

            Ok(RestoreReport {
                app_id: manifest.app_id,
                name: manifest.name,
                library: dst_library.to_path_buf(),
                install_dir: targets.dst_dir,
                bytes_restored: total_bytes,
                files_restored: manifest.files.len() + manifest.workshop_files.len(),
                verified_aurora: true,
            })
        }
        BackupLayout::Standard { app_id, name, install_dir, acf_bytes, game_dir, ws_acf, ws_content_dir, .. } => {
            let game_tree = walk_dir_no_symlinks(&game_dir)?;
            let ws_tree = match &ws_content_dir {
                Some(p) => Some((p.clone(), walk_dir_no_symlinks(p)?)),
                None => None,
            };

            let targets = prepare_dst_targets(
                dst_library,
                &canon_dst_lib,
                app_id,
                &install_dir,
                ws_acf.is_some(),
                ws_tree.is_some(),
            )?;
            if targets.canon_dst_common.starts_with(&canon_backup) || canon_backup.starts_with(&targets.canon_dst_common) {
                return Err("Backup folder and destination library common folder cannot overlap.".into());
            }

            let total_bytes = game_tree
                .total_bytes
                .saturating_add(ws_tree.as_ref().map_or(0, |(_, t)| t.total_bytes))
                .saturating_add(acf_bytes.len() as u64)
                .saturating_add(ws_acf.as_ref().map_or(0, |(_, b)| b.len() as u64));
            if let Some(avail) = available_space(&targets.canon_dst_common)
                && avail < total_bytes
            {
                return Err(format!("Not enough free space in destination library (need {total_bytes} bytes, {avail} available)."));
            }

            progress.total_bytes.store(total_bytes.max(1), Ordering::Relaxed);
            progress.done_bytes.store(0, Ordering::Relaxed);

            let staged = StagedInstall::new(&targets, "restore", app_id, ws_tree.is_some(), ws_acf.is_some());
            let stage_res = (|| -> Result<(), String> {
                fs::create_dir(&staged.stage_game).map_err(|e| format!("could not create staging dir {}: {e}", staged.stage_game.display()))?;
                copy_walked_tree(&game_dir, &staged.stage_game, &game_tree, progress)?;
                if let (Some((src_ws_dir, wt)), Some(sws)) = (&ws_tree, &staged.stage_ws) {
                    fs::create_dir(sws).map_err(|e| format!("could not create workshop staging {}: {e}", sws.display()))?;
                    copy_walked_tree(src_ws_dir, sws, wt, progress)?;
                }
                if let (Some((_, bytes)), Some(sws_acf)) = (&ws_acf, &staged.stage_ws_acf) {
                    write_bytes_new_verified(sws_acf, bytes)?;
                    progress.done_bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                }
                write_bytes_new_verified(&staged.stage_manifest, &acf_bytes)?;
                progress.done_bytes.fetch_add(acf_bytes.len() as u64, Ordering::Relaxed);
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err("Operation cancelled.".into());
                }
                Ok(())
            })();

            if let Err(e) = stage_res {
                staged.cleanup();
                return Err(e);
            }

            staged.commit(&targets)?;
            if total_bytes == 0 {
                progress.done_bytes.store(1, Ordering::Relaxed);
            }

            let files_restored = game_tree.files.len() + ws_tree.as_ref().map_or(0, |(_, t)| t.files.len());
            Ok(RestoreReport {
                app_id,
                name,
                library: dst_library.to_path_buf(),
                install_dir: targets.dst_dir,
                bytes_restored: total_bytes,
                files_restored,
                verified_aurora: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_manifest() {
        let payload = proto::ContentManifestPayload {
            mappings: vec![
                proto::content_manifest_payload::FileMapping { filename: Some("bin\\game.exe".into()), size: Some(3), sha_content: Some(vec![1; 20]), ..Default::default() },
                proto::content_manifest_payload::FileMapping { filename: Some("bin".into()), flags: Some(64), ..Default::default() },
            ],
        };
        let body = payload.encode_to_vec();
        let mut data = PAYLOAD_MAGIC.to_le_bytes().to_vec();
        data.extend((body.len() as u32).to_le_bytes());
        data.extend(body);
        data.extend(END_MAGIC.to_le_bytes());
        let files = read_manifest(&data).unwrap().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "bin/game.exe");
    }

    #[test]
    fn refuses_to_uninstall_outside_common() {
        let g = InstalledGame {
            app_id: 1,
            name: "x".into(),
            install_dir: PathBuf::from("C:/Windows"),
            library: PathBuf::from("C:/Games"),
            size_on_disk: 0,
            last_updated: None,
            last_played: None,
            playtime_minutes: None,
            state: model::InstallState::Installed,
            last_update_bytes: None,
        };
        assert!(uninstall(&g).is_err());
    }

    #[test]
    fn refuses_dotdot_traversal_and_unlinks_symlink_without_deleting_target() {
        let lib = std::env::temp_dir().join(format!("sir-uninstall-sec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&lib);
        let apps = lib.join("steamapps");
        let common = apps.join("common");
        let compat = apps.join("compatdata");
        fs::create_dir_all(&common).unwrap();
        fs::create_dir_all(&compat).unwrap();
        let save = compat.join("precious.sav");
        fs::write(&save, b"important save").unwrap();

        for bad_dir in [common.join(".."), common.join("."), common.join("sub").join("..")] {
            let g = InstalledGame {
                app_id: 99,
                name: "Evil".into(),
                install_dir: bad_dir.clone(),
                library: lib.clone(),
                size_on_disk: 0,
                last_updated: None,
                last_played: None,
                playtime_minutes: None,
                state: model::InstallState::Installed,
                last_update_bytes: None,
            };
            assert!(uninstall(&g).is_err(), "accepted {bad_dir:?}");
            assert!(save.exists(), "deleted steamapps/compatdata via {bad_dir:?}!");
            assert!(common.exists(), "deleted steamapps/common via {bad_dir:?}!");
        }

        #[cfg(unix)]
        {
            let outside = std::env::temp_dir().join(format!("sir-uninstall-ext-{}", std::process::id()));
            let _ = fs::remove_dir_all(&outside);
            fs::create_dir_all(&outside).unwrap();
            let victim = outside.join("victim.txt");
            fs::write(&victim, b"do not delete").unwrap();
            let sym_game = common.join("SymGame");
            std::os::unix::fs::symlink(&outside, &sym_game).unwrap();
            fs::write(apps.join("appmanifest_77.acf"), "\"AppState\" {}").unwrap();

            let g = InstalledGame {
                app_id: 77,
                name: "SymGame".into(),
                install_dir: sym_game.clone(),
                library: lib.clone(),
                size_on_disk: 0,
                last_updated: None,
                last_played: None,
                playtime_minutes: None,
                state: model::InstallState::Installed,
                last_update_bytes: None,
            };
            uninstall(&g).unwrap();
            assert!(victim.exists(), "uninstall followed top-level symlink and deleted target!");
            assert!(fs::symlink_metadata(&sym_game).is_err(), "symlink itself should be unlinked");
            assert!(!apps.join("appmanifest_77.acf").exists());
            let _ = fs::remove_dir_all(&outside);
        }

        let _ = fs::remove_dir_all(&lib);
    }

    fn temp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("aurora-manage-{tag}-{}-{}", std::process::id(), next_temp_tag()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_test_game(lib: &Path, app_id: AppId, folder: &str, name: &str, with_workshop: bool) -> InstalledGame {
        let apps = lib.join("steamapps");
        let common = apps.join("common");
        let dir = common.join(folder);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("empty_sub")).unwrap();
        fs::write(dir.join("readme.txt"), b"hello aurora").unwrap();
        fs::write(dir.join("bin").join("run.sh"), b"#!/bin/sh\necho ok\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.join("bin").join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let acf = format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{app_id}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{folder}\"\n\t\"buildid\"\t\t\"4242\"\n\t\"SizeOnDisk\"\t\t\"30\"\n\t\"StateFlags\"\t\t\"4\"\n}}\n"
        );
        fs::write(apps.join(format!("appmanifest_{app_id}.acf")), acf).unwrap();

        if with_workshop {
            let ws = apps.join("workshop");
            let item_dir = ws.join("content").join(app_id.to_string()).join("9001");
            fs::create_dir_all(&item_dir).unwrap();
            fs::write(item_dir.join("mod.pak"), b"workshop-payload").unwrap();
            let ws_acf = format!(
                "\"AppWorkshop\"\n{{\n\t\"appid\"\t\t\"{app_id}\"\n\t\"WorkshopItemsInstalled\"\n\t{{\n\t\t\"9001\"\n\t\t{{\n\t\t\t\"size\"\t\t\"16\"\n\t\t\t\"timeupdated\"\t\t\"1700000000\"\n\t\t}}\n\t}}\n}}\n"
            );
            fs::write(ws.join(format!("appworkshop_{app_id}.acf")), ws_acf).unwrap();
        }

        InstalledGame {
            app_id,
            name: name.into(),
            install_dir: dir,
            library: lib.to_path_buf(),
            size_on_disk: 30,
            last_updated: None,
            last_played: None,
            playtime_minutes: None,
            state: InstallState::Installed,
            last_update_bytes: None,
        }
    }

    #[test]
    fn move_game_cross_dir_copy_and_same_fs_rename_with_workshop() {
        let root = temp_root("move");
        let lib1 = root.join("lib1");
        let lib2 = root.join("lib2");
        let lib3 = root.join("lib3");
        fs::create_dir_all(&lib1).unwrap();
        fs::create_dir_all(&lib2).unwrap();
        fs::create_dir_all(&lib3).unwrap();

        let game = make_test_game(&lib1, 620, "Portal 2", "Portal 2", true);

        // 1) Cross-filesystem copy+verify path (allow_rename = false).
        let p_copy = Progress::default();
        let rep1 = move_game_impl(&game, &lib2, &p_copy, false).unwrap();
        assert!(!rep1.used_rename);
        assert!(rep1.workshop_moved);
        assert_eq!(rep1.files_moved, 3);
        assert!((p_copy.fraction() - 1.0).abs() < 0.001);
        assert!(!lib1.join("steamapps/appmanifest_620.acf").exists());
        assert!(!lib1.join("steamapps/common/Portal 2").exists());
        assert!(!lib1.join("steamapps/workshop/appworkshop_620.acf").exists());
        assert!(!lib1.join("steamapps/workshop/content/620").exists());

        assert_eq!(fs::read(lib2.join("steamapps/common/Portal 2/readme.txt")).unwrap(), b"hello aurora");
        assert!(lib2.join("steamapps/common/Portal 2/empty_sub").is_dir());
        assert_eq!(fs::read(lib2.join("steamapps/workshop/content/620/9001/mod.pak")).unwrap(), b"workshop-payload");
        assert!(lib2.join("steamapps/workshop/appworkshop_620.acf").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(lib2.join("steamapps/common/Portal 2/bin/run.sh")).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0, "executable bit should be preserved");
        }
        let scanned2 = crate::scan_library(&lib2);
        assert_eq!(scanned2.len(), 1);
        assert_eq!(scanned2[0].app_id, 620);
        assert!(crate::scan_library(&lib1).is_empty());

        // 2) Same-filesystem atomic rename path (`move_game`).
        let moved_game = scanned2.into_iter().next().unwrap();
        let p_ren = Progress::default();
        let rep2 = move_game(&moved_game, &lib3, &p_ren).unwrap();
        assert!(rep2.used_rename);
        assert!(rep2.workshop_moved);
        assert!((p_ren.fraction() - 1.0).abs() < 0.001);
        assert!(!lib2.join("steamapps/appmanifest_620.acf").exists());
        assert!(!lib2.join("steamapps/common/Portal 2").exists());
        assert_eq!(fs::read(lib3.join("steamapps/common/Portal 2/bin/run.sh")).unwrap(), b"#!/bin/sh\necho ok\n");
        assert_eq!(fs::read(lib3.join("steamapps/workshop/content/620/9001/mod.pak")).unwrap(), b"workshop-payload");
        assert_eq!(crate::scan_library(&lib3).len(), 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn move_game_refuses_downloading_same_lib_existing_dest_and_cancel() {
        let root = temp_root("move-guards");
        let lib1 = root.join("lib1");
        let lib2 = root.join("lib2");
        fs::create_dir_all(&lib1).unwrap();
        fs::create_dir_all(&lib2).unwrap();

        let mut game = make_test_game(&lib1, 440, "TF2", "Team Fortress 2", false);
        game.state = InstallState::Downloading;
        assert!(move_game(&game, &lib2, &Progress::default()).is_err());
        game.state = InstallState::Installed;

        // Same library refused.
        assert!(move_game(&game, &lib1, &Progress::default()).is_err());

        // Existing destination dir refused without deleting destination or source.
        fs::create_dir_all(lib2.join("steamapps/common/TF2")).unwrap();
        fs::write(lib2.join("steamapps/common/TF2/keep.txt"), b"keep").unwrap();
        assert!(move_game(&game, &lib2, &Progress::default()).is_err());
        assert!(lib2.join("steamapps/common/TF2/keep.txt").exists());
        assert!(game.install_dir.join("readme.txt").exists());
        fs::remove_dir_all(lib2.join("steamapps/common/TF2")).unwrap();

        // Cancellation preserves source and cleans staging.
        let p = Progress::default();
        p.cancel.store(true, Ordering::Relaxed);
        assert!(move_game_impl(&game, &lib2, &p, false).is_err());
        assert!(game.install_dir.join("readme.txt").exists());
        assert!(lib1.join("steamapps/appmanifest_440.acf").exists());
        assert!(!lib2.join("steamapps/appmanifest_440.acf").exists());
        let left: Vec<_> = fs::read_dir(lib2.join("steamapps/common")).unwrap().flatten().collect();
        assert!(left.is_empty(), "staging should be cleaned up: {left:?}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn backup_reservation_and_rollback_preserve_other_artifacts() {
        let root = temp_root("backup-reservation");
        let lib = root.join("lib");
        let game = make_test_game(&lib, 730, "CS2", "CS2", false);
        let dest = root.join("dest");
        fs::create_dir_all(dest.join(".aurora-backup.lock")).unwrap();
        assert!(backup_game(&game, &dest, &Progress::default()).unwrap_err().contains("reserve"));
        assert!(dest.join(".aurora-backup.lock").is_dir());
        fs::write(root.join("source"), b"new").unwrap();
        fs::write(dest.join("other"), b"previous").unwrap();
        let artifacts = [
            (root.join("source"), dest.join("new"), false),
            (root.join("missing"), dest.join("other"), false),
        ];
        assert!(publish_backup(&artifacts).is_err());
        assert!(!dest.join("new").exists());
        assert_eq!(fs::read(dest.join("other")).unwrap(), b"previous");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_dotdot_and_symlinks_in_move_backup_and_restore() {
        let root = temp_root("sec");
        let lib1 = root.join("lib1");
        let lib2 = root.join("lib2");
        let outside = root.join("outside");
        let bkp = root.join("backup");
        fs::create_dir_all(&lib1).unwrap();
        fs::create_dir_all(&lib2).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();

        let game = make_test_game(&lib1, 730, "CS2", "Counter-Strike 2", false);

        // `..` traversal rejected everywhere.
        let mut bad = game.clone();
        bad.install_dir = lib1.join("steamapps/common/..").join("common/CS2");
        assert!(move_game(&bad, &lib2, &Progress::default()).is_err());
        assert!(backup_game(&bad, &bkp, &Progress::default()).is_err());
        assert!(move_game(&game, &lib2.join("..").join("lib2"), &Progress::default()).is_err());
        assert!(backup_game(&game, &bkp.join("..").join("backup"), &Progress::default()).is_err());
        assert!(restore_game(&bkp.join("..").join("backup"), &lib2, &Progress::default()).is_err());
        assert!(restore_game(&bkp, &lib2.join("..").join("lib2"), &Progress::default()).is_err());

        #[cfg(unix)]
        {
            // Nested symlink inside game folder must be rejected by move & backup.
            let nested_sym = game.install_dir.join("escape");
            std::os::unix::fs::symlink(&outside, &nested_sym).unwrap();
            assert!(move_game(&game, &lib2, &Progress::default()).is_err());
            assert!(backup_game(&game, &bkp, &Progress::default()).is_err());
            assert!(outside.join("secret.txt").exists());
            fs::remove_file(&nested_sym).unwrap();

            // Clean backup first, then plant a symlink inside the backup and verify restore rejects it.
            backup_game(&game, &bkp, &Progress::default()).unwrap();
            std::os::unix::fs::symlink(outside.join("secret.txt"), bkp.join("common/CS2/planted")).unwrap();
            assert!(restore_game(&bkp, &lib2, &Progress::default()).is_err());
            assert!(!lib2.join("steamapps/common/CS2").exists());

            // Symlinked destination library must be rejected.
            let sym_lib = root.join("sym_lib");
            std::os::unix::fs::symlink(&lib2, &sym_lib).unwrap();
            assert!(move_game(&game, &sym_lib, &Progress::default()).is_err());
            assert!(restore_game(&bkp, &sym_lib, &Progress::default()).is_err());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn backup_and_restore_aurora_round_trip_and_integrity_checks() {
        let root = temp_root("backup-rt");
        let lib1 = root.join("lib1");
        let lib2 = root.join("lib2");
        let bkp = root.join("backups").join("Hades (1145360)");
        fs::create_dir_all(&lib1).unwrap();
        fs::create_dir_all(&lib2).unwrap();
        fs::create_dir_all(root.join("backups")).unwrap();

        let game = make_test_game(&lib1, 1145360, "Hades", "Hades", true);
        let p_bkp = Progress::default();
        let brep = backup_game(&game, &bkp, &p_bkp).unwrap();
        assert_eq!(brep.files_copied, 3);
        assert!((p_bkp.fraction() - 1.0).abs() < 0.001);
        assert!(bkp.join("aurora-backup.json").is_file());
        assert!(bkp.join("appmanifest_1145360.acf").is_file());
        assert!(bkp.join("common/Hades/readme.txt").is_file());

        let info = inspect_backup(&bkp).unwrap();
        assert!(info.is_aurora);
        assert_eq!(info.app_id, 1145360);
        assert_eq!(info.name, "Hades");
        assert_eq!(info.install_dir, "Hades");
        assert_eq!(info.build_id, 4242);
        assert_eq!(info.file_count, 2);

        // Corrupt a backed-up file: restore MUST fail SHA-256 check and leave lib2 clean.
        let bad_bkp = root.join("bad_bkp");
        fs::create_dir_all(&bad_bkp).unwrap();
        let p_corrupt = Progress::default();
        backup_game(&game, &bad_bkp, &p_corrupt).unwrap();
        fs::write(bad_bkp.join("common/Hades/readme.txt"), b"TAMPERED!!!!").unwrap();
        let err = restore_game(&bad_bkp, &lib2, &Progress::default()).unwrap_err();
        assert!(err.contains("SHA-256") || err.contains("mismatch") || err.contains("size"), "unexpected err: {err}");
        assert!(!lib2.join("steamapps/common/Hades").exists());
        assert!(!lib2.join("steamapps/appmanifest_1145360.acf").exists());

        // Corrupt appmanifest_1145360.acf without updating aurora-backup.json: MUST fail.
        let bad_acf_bkp = root.join("bad_acf_bkp");
        backup_game(&game, &bad_acf_bkp, &Progress::default()).unwrap();
        let mut acf_text = fs::read_to_string(bad_acf_bkp.join("appmanifest_1145360.acf")).unwrap();
        acf_text.push_str("\n// tampered\n");
        fs::write(bad_acf_bkp.join("appmanifest_1145360.acf"), acf_text).unwrap();
        let err = restore_game(&bad_acf_bkp, &lib2, &Progress::default()).unwrap_err();
        assert!(err.contains("SHA-256") || err.contains("mismatch"), "unexpected err: {err}");

        // Valid round-trip restore into lib2.
        uninstall(&game).unwrap();
        assert!(crate::scan_library(&lib1).is_empty());

        let p_res = Progress::default();
        let rrep = restore_game(&bkp, &lib2, &p_res).unwrap();
        assert!(rrep.verified_aurora);
        assert_eq!(rrep.app_id, 1145360);
        assert_eq!(rrep.files_restored, 3);
        assert!((p_res.fraction() - 1.0).abs() < 0.001);

        assert_eq!(fs::read(lib2.join("steamapps/common/Hades/readme.txt")).unwrap(), b"hello aurora");
        assert!(lib2.join("steamapps/common/Hades/empty_sub").is_dir());
        assert_eq!(fs::read(lib2.join("steamapps/workshop/content/1145360/9001/mod.pak")).unwrap(), b"workshop-payload");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(lib2.join("steamapps/common/Hades/bin/run.sh")).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0, "executable bit should be preserved on restore");
        }
        let scanned = crate::scan_library(&lib2);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].app_id, 1145360);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_supports_standard_steam_backup_layouts() {
        let root = temp_root("std-bkp");

        // Layout A: <bkp>/appmanifest_<id>.acf + <bkp>/common/<installdir>
        let bkp_a = root.join("layout_a");
        fs::create_dir_all(bkp_a.join("common/GameA")).unwrap();
        fs::write(bkp_a.join("common/GameA/data.bin"), b"payload-a").unwrap();
        fs::write(
            bkp_a.join("appmanifest_101.acf"),
            "\"AppState\" { \"appid\" \"101\" \"name\" \"Game A\" \"installdir\" \"GameA\" \"StateFlags\" \"4\" }",
        )
        .unwrap();
        let lib_a = root.join("lib_a");
        fs::create_dir_all(&lib_a).unwrap();
        let info_a = inspect_backup(&bkp_a).unwrap();
        assert!(!info_a.is_aurora);
        assert_eq!(info_a.app_id, 101);
        let rep_a = restore_game(&bkp_a, &lib_a, &Progress::default()).unwrap();
        assert!(!rep_a.verified_aurora);
        assert_eq!(fs::read(lib_a.join("steamapps/common/GameA/data.bin")).unwrap(), b"payload-a");
        assert_eq!(crate::scan_library(&lib_a).len(), 1);

        // Layout B: <bkp>/steamapps/appmanifest_<id>.acf + <bkp>/steamapps/common/<installdir>
        let bkp_b = root.join("layout_b");
        fs::create_dir_all(bkp_b.join("steamapps/common/GameB")).unwrap();
        fs::write(bkp_b.join("steamapps/common/GameB/data.bin"), b"payload-b").unwrap();
        fs::write(
            bkp_b.join("steamapps/appmanifest_102.acf"),
            "\"AppState\" { \"appid\" \"102\" \"name\" \"Game B\" \"installdir\" \"GameB\" \"StateFlags\" \"4\" }",
        )
        .unwrap();
        let lib_b = root.join("lib_b");
        fs::create_dir_all(&lib_b).unwrap();
        restore_game(&bkp_b, &lib_b, &Progress::default()).unwrap();
        assert_eq!(fs::read(lib_b.join("steamapps/common/GameB/data.bin")).unwrap(), b"payload-b");
        assert_eq!(crate::scan_library(&lib_b)[0].app_id, 102);

        // Layout C: <bkp>/appmanifest_<id>.acf + <bkp>/<installdir>
        let bkp_c = root.join("layout_c");
        fs::create_dir_all(bkp_c.join("GameC")).unwrap();
        fs::write(bkp_c.join("GameC/data.bin"), b"payload-c").unwrap();
        fs::write(
            bkp_c.join("appmanifest_103.acf"),
            "\"AppState\" { \"appid\" \"103\" \"name\" \"Game C\" \"installdir\" \"GameC\" \"StateFlags\" \"4\" }",
        )
        .unwrap();
        let lib_c = root.join("lib_c");
        fs::create_dir_all(&lib_c).unwrap();
        restore_game(&bkp_c, &lib_c, &Progress::default()).unwrap();
        assert_eq!(fs::read(lib_c.join("steamapps/common/GameC/data.bin")).unwrap(), b"payload-c");
        assert_eq!(crate::scan_library(&lib_c)[0].app_id, 103);

        // Tampered aurora-backup.json traversal rejected.
        let bkp_bad_json = root.join("bad_json");
        fs::create_dir_all(bkp_bad_json.join("common/Evil")).unwrap();
        fs::write(bkp_bad_json.join("appmanifest_104.acf"), "\"AppState\" { \"appid\" \"104\" \"installdir\" \"Evil\" }").unwrap();
        fs::write(
            bkp_bad_json.join("aurora-backup.json"),
            r#"{"version":1,"app_id":104,"name":"Evil","install_dir":"../Evil","build_id":1,"created_at":1,"total_bytes":0,"appmanifest":"appmanifest_104.acf","appmanifest_sha256":"0000000000000000000000000000000000000000000000000000000000000000","files":[]}"#,
        )
        .unwrap();
        assert!(inspect_backup(&bkp_bad_json).is_err());

        // Refuses restoring onto an existing installation without clobbering it.
        assert!(restore_game(&bkp_c, &lib_c, &Progress::default()).is_err());
        assert_eq!(fs::read(lib_c.join("steamapps/common/GameC/data.bin")).unwrap(), b"payload-c");

        // Refuses symlink inside a standard Steam backup folder.
        #[cfg(unix)]
        {
            let outside = root.join("outside_std");
            fs::create_dir_all(&outside).unwrap();
            fs::write(outside.join("leak.txt"), b"secret").unwrap();
            std::os::unix::fs::symlink(outside.join("leak.txt"), bkp_c.join("GameC/sym")).unwrap();
            let lib_c2 = root.join("lib_c2");
            fs::create_dir_all(&lib_c2).unwrap();
            assert!(restore_game(&bkp_c, &lib_c2, &Progress::default()).is_err());
            assert!(!lib_c2.join("steamapps/common/GameC").exists());
        }

        let _ = fs::remove_dir_all(&root);
    }
}

/// A workshop item installed for a game (from `appworkshop_<app>.acf`).
#[derive(Debug, Clone)]
pub struct WorkshopItem {
    pub id: u64,
    pub size: u64,
    /// Unix seconds.
    pub updated: u64,
    pub folder: PathBuf,
}

/// A value parsed from a file, re-read only when the file changes (its size
/// or modification time): pages call these every frame, and a workshop
/// manifest can list thousands of mods.
fn cached_read<T: Clone + Send + 'static>(cache: &'static std::sync::Mutex<Option<std::collections::HashMap<PathBuf, (Option<std::time::SystemTime>, u64, T)>>>, path: &Path, parse: impl FnOnce(Option<String>) -> T) -> T {
    let stamp = fs::metadata(path).ok().map(|m| (m.modified().ok(), m.len()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(Default::default);
    if let (Some((modified, len)), Some((m, l, v))) = (stamp, map.get(path)) {
        if *m == modified && *l == len {
            return v.clone();
        }
    }
    let value = parse(fs::read_to_string(path).ok());
    match stamp {
        Some((modified, len)) => {
            map.insert(path.to_path_buf(), (modified, len, value.clone()));
        }
        None => {
            map.remove(path);
        }
    }
    value
}

pub fn workshop_items(game: &InstalledGame) -> Vec<WorkshopItem> {
    static CACHE: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, (Option<std::time::SystemTime>, u64, Vec<WorkshopItem>)>>> = std::sync::Mutex::new(None);
    let ws = game.library.join("steamapps").join("workshop");
    let acf = ws.join(format!("appworkshop_{}.acf", game.app_id));
    cached_read(&CACHE, &acf, |text| parse_workshop_items(game, &ws, text))
}

fn parse_workshop_items(game: &InstalledGame, ws: &Path, text: Option<String>) -> Vec<WorkshopItem> {
    let Some(text) = text else { return Vec::new() };
    let Ok(doc) = vdf::parse(&text) else { return Vec::new() };
    let content = ws.join("content").join(game.app_id.to_string());
    let mut items: Vec<WorkshopItem> = doc
        .path(&["AppWorkshop", "WorkshopItemsInstalled"])
        .map(|o| {
            o.iter()
                .filter_map(|(id, v)| {
                    let o = v.as_obj()?;
                    let id: u64 = id.parse().ok()?;
                    Some(WorkshopItem { id, size: o.u64("size").unwrap_or(0), updated: o.u64("timeupdated").unwrap_or(0), folder: content.join(id.to_string()) })
                })
                .collect()
        })
        .unwrap_or_default();
    items.sort_by_key(|i| std::cmp::Reverse(i.updated));
    items
}

/// Delete a workshop item's downloaded files (Steam re-downloads it unless
/// you also unsubscribe).
pub fn delete_workshop_files(item: &WorkshopItem) -> Result<(), String> {
    // Only ever delete <library>/steamapps/workshop/content/<app>/<item>.
    if !crate::has_only_normal_components(&item.folder) {
        return Err("unexpected workshop folder".into());
    }
    let named_right = item.folder.file_name().is_some_and(|n| n.to_string_lossy() == item.id.to_string());
    let Some(app_dir) = item.folder.parent() else {
        return Err("unexpected workshop folder".into());
    };
    let in_content = app_dir
        .parent()
        .is_some_and(|p| p.file_name().is_some_and(|n| n == "content") && p.parent().is_some_and(|w| w.file_name().is_some_and(|n| n == "workshop")));
    if !named_right || !in_content {
        return Err("unexpected workshop folder".into());
    }
    match fs::symlink_metadata(&item.folder) {
        Ok(m) if crate::is_symlink_or_reparse(&m) => {
            crate::unlink_symlink_or_junction(&item.folder, &m).map_err(|e| e.to_string())?;
        }
        Ok(m) if m.is_dir() => {
            let canon_parent = app_dir.canonicalize().map_err(|e| e.to_string())?;
            let canon_item = item.folder.canonicalize().map_err(|e| e.to_string())?;
            if canon_item == canon_parent || canon_item.parent() != Some(canon_parent.as_path()) {
                return Err("workshop folder escapes content directory".into());
            }
            fs::remove_dir_all(&item.folder).map_err(|e| e.to_string())?;
        }
        Ok(_) => return Err("unexpected workshop folder".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(())
}

/// Build id and branch Steam has installed (from the app manifest).
pub fn installed_build(game: &InstalledGame) -> (u64, String) {
    static CACHE: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, (Option<std::time::SystemTime>, u64, (u64, String))>>> = std::sync::Mutex::new(None);
    cached_read(&CACHE, &manifest_file(game), parse_installed_build)
}

fn parse_installed_build(text: Option<String>) -> (u64, String) {
    let Some(text) = text else { return (0, "public".into()) };
    let Ok(doc) = vdf::parse(&text) else { return (0, "public".into()) };
    let app = doc.obj("AppState");
    let build = app.and_then(|a| a.u64("buildid")).unwrap_or(0);
    let branch = app.and_then(|a| a.path(&["UserConfig"])).and_then(|u| u.str("BetaKey")).filter(|b| !b.is_empty()).unwrap_or("public").to_string();
    (build, branch)
}

#[cfg(test)]
mod cached_read_tests {
    use super::*;

    #[test]
    fn rereads_only_when_the_file_changes() {
        static CACHE: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, (Option<std::time::SystemTime>, u64, String)>>> = std::sync::Mutex::new(None);
        let dir = std::env::temp_dir().join(format!("sir-cached-read-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.acf");
        fs::write(&path, "one").unwrap();
        let read = |calls: &mut u32| cached_read(&CACHE, &path, |t| {
            *calls += 1;
            t.unwrap_or_default()
        });
        let mut calls = 0;
        assert_eq!(read(&mut calls), "one");
        assert_eq!(read(&mut calls), "one");
        assert_eq!(calls, 1, "unchanged file: parsed once");
        fs::write(&path, "two!").unwrap();
        assert_eq!(read(&mut calls), "two!");
        assert_eq!(calls, 2);
        fs::remove_file(&path).unwrap();
        assert_eq!(read(&mut calls), "");
        let _ = fs::remove_dir_all(&dir);
    }
}
