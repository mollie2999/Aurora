//! Installing and updating games: picks depots, gets keys and manifests
//! through the CM connection, then hands the transfer to the `depot` crate.

use model::{AppId, DownloadPhase, DownloadStatus, OwnedApp};
use net::{Connection, emsg, eresult};
use proto::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstallRequest {
    pub app_id: AppId,
    /// Library folder (the one that contains `steamapps`).
    pub library: PathBuf,
    /// Steam install root (for `depotcache`).
    pub steam_root: PathBuf,
    pub branch: String,
    /// Steam language name, e.g. "english", "russian".
    pub language: String,
    pub skip_dlc: Vec<AppId>,
    /// DLC the user turned on after installing: added on this update.
    #[serde(default)]
    pub add_dlc: Vec<AppId>,
    /// Which build to download: "windows" or "linux". Empty: keep what's
    /// installed (updates), else Windows.
    #[serde(default)]
    pub platform: String,
}

/// The build an install or update downloads: the one asked for, else the
/// one on disk (a Linux install stays Linux), else Windows.
pub fn platform_for<'a>(app: &OwnedApp, req: &'a InstallRequest, installed: &HashSet<u32>) -> &'a str {
    if !req.platform.is_empty() {
        return &req.platform;
    }
    let on = |os: &str| app.depots.iter().any(|d| installed.contains(&d.id) && d.os.split(',').any(|o| o.trim() == os));
    if on("linux") && !on("windows") { "linux" } else { "windows" }
}

/// Does the game have its own depots for `os` (a build for that system)?
pub fn has_build(app: &OwnedApp, os: &str) -> bool {
    app.depots.iter().any(|d| d.dlc_app.is_none() && !d.manifests.is_empty() && d.os.split(',').any(|o| o.trim() == os))
}

/// Choose the depots a 64-bit install of the chosen build needs (see `platform_for`). `installed` = the
/// depots on disk now (empty for a new install). Like Steam, a new install
/// gets the owned DLC, and an update only keeps the DLC already installed
/// (plus any the user turned on), so owned-but-uninstalled DLC never turns
/// an update into a multi-gigabyte download.
pub fn pick_depots<'a>(app: &'a OwnedApp, owned: &HashSet<AppId>, req: &InstallRequest, installed: &HashSet<u32>) -> Vec<&'a model::DepotInfo> {
    let platform = platform_for(app, req, installed);
    let os_ok = |d: &&model::DepotInfo| d.os.is_empty() || d.os.split(',').any(|o| o.trim() == platform);
    let lang_ok = |d: &&model::DepotInfo| d.language.is_empty() || d.language == req.language;
    // An update keeps every depot already installed (its language, 32 or
    // 64-bit), like Steam; the OS/language/arch choice is for new content.
    let was = |d: &&model::DepotInfo| installed.contains(&d.id);
    let mut out: Vec<&model::DepotInfo> = app
        .depots
        .iter()
        .filter(|d| was(d) || (os_ok(d) && lang_ok(d)))
        .filter(|d| !d.low_violence && !d.shared_install && !d.manifests.is_empty())
        .filter(|d| match d.dlc_app {
            Some(dlc) => owned.contains(&dlc) && !req.skip_dlc.contains(&dlc) && (installed.is_empty() || was(d) || req.add_dlc.contains(&dlc)),
            None => true,
        })
        .collect();
    // Prefer 64-bit depots when both exist (but never drop an installed one).
    let has64 = out.iter().any(|d| d.arch == "64");
    if has64 {
        out.retain(|d| d.arch != "32" || was(d));
    }
    // Fall back to English content if the chosen language has none.
    if out.iter().all(|d| d.language.is_empty()) && req.language != "english" {
        let mut en = req.clone();
        en.language = "english".into();
        let extra: Vec<&model::DepotInfo> = app.depots.iter().filter(os_ok).filter(|d| d.language == "english").collect();
        out.extend(extra);
    }
    out
}

pub async fn content_servers(conn: &Connection, cell_id: u32) -> Result<Vec<depot::Server>, String> {
    let r: CContentServerDirectoryGetServersForSteamPipeResponse = conn
        .call(
            "ContentServerDirectory.GetServersForSteamPipe#1",
            &CContentServerDirectoryGetServersForSteamPipeRequest { cell_id: Some(cell_id), max_servers: Some(20), ..Default::default() },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(r.servers
        .into_iter()
        .filter(|s| matches!(s.r#type(), "SteamCache" | "CDN"))
        .map(|s| depot::Server {
            host: if s.vhost().is_empty() { s.host().to_string() } else { s.vhost().to_string() },
            https: !s.https_support().is_empty() && !s.https_support().eq_ignore_ascii_case("none"),
            load: s.weighted_load(),
        })
        .filter(|s| s.https && depot::cdn::valid_host(&s.host))
        .collect())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DepotKeyError {
    #[error("your license doesn't include depot {0}")]
    Excluded(u32),
    #[error("{0}")]
    Failed(String),
}

pub(crate) async fn depot_key(conn: &Connection, app: AppId, depot: u32) -> Result<Vec<u8>, DepotKeyError> {
    let reply = conn
        .request(emsg::CLIENT_GET_DEPOT_DECRYPTION_KEY, &CMsgClientGetDepotDecryptionKey { depot_id: Some(depot), app_id: Some(app) })
        .await
        .map_err(|e| DepotKeyError::Failed(e.to_string()))?;
    let r: CMsgClientGetDepotDecryptionKeyResponse = reply.body().map_err(|e| DepotKeyError::Failed(e.to_string()))?;
    classify_depot_key(r, depot)
}

fn classify_depot_key(r: CMsgClientGetDepotDecryptionKeyResponse, depot: u32) -> Result<Vec<u8>, DepotKeyError> {
    if r.depot_id.is_some_and(|id| id != depot) {
        return Err(DepotKeyError::Failed(format!("depot {depot}: mismatched response")));
    }
    if r.eresult() == eresult::ACCESS_DENIED {
        // For content this means the license doesn't include this depot
        // (optional content, another region's files, a DLC you don't own).
        return Err(DepotKeyError::Excluded(depot));
    }
    if r.eresult() != eresult::OK {
        return Err(DepotKeyError::Failed(eresult::describe(r.eresult(), &format!("depot {depot}"))));
    }
    if r.depot_encryption_key().len() != 32 {
        return Err(DepotKeyError::Failed(format!("depot {depot}: invalid key length")));
    }
    Ok(r.depot_encryption_key().to_vec())
}

pub(crate) async fn request_code(conn: &Connection, app: AppId, depot: u32, gid: u64, branch: &str) -> u64 {
    conn.call::<_, CContentServerDirectoryGetManifestRequestCodeResponse>(
        "ContentServerDirectory.GetManifestRequestCode#1",
        &CContentServerDirectoryGetManifestRequestCodeRequest {
            app_id: Some(app),
            depot_id: Some(depot),
            manifest_id: Some(gid),
            app_branch: Some(branch.to_string()),
            branch_password_hash: None,
        },
    )
    .await
    .map(|r| r.manifest_request_code())
    .unwrap_or(0)
}

/// Read what's installed now: depot -> manifest id (from the app manifest).
fn installed_manifests(req: &InstallRequest) -> Vec<(u32, u64)> {
    let path = req.library.join("steamapps").join(format!("appmanifest_{}.acf", req.app_id));
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(doc) = vdf::parse(&text) else { return Vec::new() };
    doc.path(&["AppState", "InstalledDepots"])
        .map(|d| d.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_obj()?.u64("manifest")?))).collect())
        .unwrap_or_default()
}

pub fn status(app_id: AppId, p: &depot::Progress, speed: u64) -> DownloadStatus {
    DownloadStatus {
        app_id,
        phase: match p.phase() {
            depot::Phase::Preparing => DownloadPhase::Preparing,
            depot::Phase::Checking => DownloadPhase::Checking,
            depot::Phase::Downloading => DownloadPhase::Downloading,
            depot::Phase::Finishing => DownloadPhase::Finishing,
            depot::Phase::Done => DownloadPhase::Done,
        },
        downloaded: p.downloaded.load(Ordering::Relaxed),
        download_total: p.download_total.load(Ordering::Relaxed),
        disk_done: p.disk_done.load(Ordering::Relaxed),
        disk_total: p.disk_total.load(Ordering::Relaxed),
        speed,
        paused: p.paused.load(Ordering::Relaxed),
        error: None,
        reused: 0,
        notes: p.notes.lock().map(|n| n.clone()).unwrap_or_default(),
        install_dir: None,
        disk_speed: 0,
    }
}

pub(crate) fn install_folder_name(app: &OwnedApp) -> Result<String, String> {
    if !app.install_dir.is_empty() {
        return depot::install::validate_folder_name(&app.install_dir).map_err(|e| e.to_string());
    }
    if app.name.is_empty() || app.name == "." || app.name == ".." || app.name.contains(['/', '\\', '\0']) {
        return Err(format!("unsafe app folder name: {:?}", app.name));
    }
    let sanitized: String = app
        .name
        .chars()
        .map(|c| if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 0x20 { '_' } else { c })
        .collect();
    let trimmed = sanitized.trim_end_matches([' ', '.']);
    depot::install::validate_folder_name(trimmed).map_err(|e| e.to_string())
}

/// Run one install/update to completion.
pub async fn run(
    conn: Connection,
    app: OwnedApp,
    owned: HashSet<AppId>,
    req: InstallRequest,
    cell_id: u32,
    preferred_host: Option<String>,
    owner: u64,
    progress: Arc<depot::Progress>,
) -> Result<depot::Report, String> {
    progress.set_phase(depot::Phase::Preparing);
    let folder = install_folder_name(&app)?;
    let common = req.library.join("steamapps").join("common");
    let dir = depot::install::resolve_inside(&common, &folder).map_err(|e| e.to_string())?;
    let old = installed_manifests(&req);
    let on_disk: HashSet<u32> = old.iter().map(|(d, _)| *d).collect();
    let depots = pick_depots(&app, &owned, &req, &on_disk);
    if depots.is_empty() {
        let system = if platform_for(&app, &req, &on_disk) == "linux" { "Linux" } else { "Windows" };
        return Err(format!("{} has no downloadable content for {system}", app.name));
    }
    let mut servers = content_servers(&conn, cell_id).await?;
    // A host the player picked goes first (the others stay as fallbacks).
    if let Some(host) = preferred_host.filter(|h| depot::cdn::valid_host(h)) {
        servers.retain(|s| s.host != host);
        servers.insert(0, depot::Server { host, https: true, load: 0.0 });
    }
    let cdn = Arc::new(depot::Cdn::new(servers));
    if cdn.server_count() == 0 {
        return Err("Steam returned no HTTPS content servers".into());
    }
    cdn.rank().await;

    let mut jobs = Vec::new();
    let mut installed = Vec::new();
    // Installed depots left untouched this time (couldn't get their key).
    let mut kept: Vec<(u32, u64, u64, Option<AppId>)> = Vec::new();
    let picked: HashSet<u32> = depots.iter().map(|d| d.id).collect();
    for d in depots {
        let Some((gid, size)) = d.manifest(&req.branch) else { continue };
        let key = match depot_key(&conn, app.app_id, d.id).await {
            Ok(k) => k,
            Err(DepotKeyError::Failed(e)) => return Err(e),
            Err(e @ DepotKeyError::Excluded(_)) => {
                // Usually a depot this account doesn't own (optional content);
                // Steam's client skips these too. Shown on the Downloads page.
                tracing::info!("skipping depot {}: {e}", d.id);
                if let Ok(mut n) = progress.notes.lock() {
                    n.push(e.to_string());
                }
                // Already installed: keep it (and its record) as it is.
                if let Some((_, m)) = old.iter().find(|(dep, _)| *dep == d.id) {
                    kept.push((d.id, *m, 0, d.dlc_app));
                }
                continue;
            }
        };
        let code = request_code(&conn, app.app_id, d.id, gid, &req.branch).await;
        let raw = cdn.manifest(d.id, gid, code).await.map_err(|e| e.to_string())?;
        let mut manifest = depot::Manifest::from_cdn(&raw).map_err(|e| e.to_string())?;
        manifest.decrypt_names(&key).map_err(|e| e.to_string())?;
        let old_manifest = old
            .iter()
            .find(|(dep, _)| *dep == d.id)
            .and_then(|(dep, m)| std::fs::read(req.steam_root.join("depotcache").join(format!("{dep}_{m}.manifest"))).ok())
            .and_then(|b| depot::Manifest::parse(&b).ok());
        installed.push((d.id, gid, size.max(manifest.original_size), d.dlc_app));
        jobs.push(depot::DepotJob { depot: d.id, manifest, key, old: old_manifest });
    }
    if jobs.is_empty() {
        return Err("This account can't download any of this game's content".into());
    }

    // Make sure it fits (a rough upper bound: the full new size).
    let need: u64 = jobs.iter().map(|j| j.manifest.original_size).sum();
    if let Ok(free) = fs4::available_space(&req.library) {
        let have: u64 = dir_size(&dir);
        if need > free + have {
            return Err(format!("Not enough disk space: needs {} MB more", (need - free - have) / 1_048_576));
        }
    }

    let manifests: Vec<depot::Manifest> = jobs.iter().map(|j| j.manifest.clone()).collect();
    // Depots installed before but not wanted now: only DLC the user turned
    // off. Anything else not picked (unknown to Steam's app info now, say)
    // is left alone rather than deleted.
    let dropped: Vec<(u32, u64)> = old
        .iter()
        .filter(|(d, _)| !picked.contains(d))
        .filter(|(d, _)| app.depots.iter().find(|x| x.id == *d).and_then(|x| x.dlc_app).is_some_and(|dlc| req.skip_dlc.contains(&dlc) || !owned.contains(&dlc)))
        .copied()
        .collect();
    // Installed depots neither updated nor removed keep their record.
    for (d, m) in &old {
        if !picked.contains(d) && !dropped.iter().any(|(x, _)| x == d) {
            kept.push((*d, *m, 0, app.depots.iter().find(|x| x.id == *d).and_then(|x| x.dlc_app)));
        }
    }
    installed.extend(kept.iter().copied());
    // A new install creates its folder; an update writes into the game's.
    let fresh = !dir.exists();
    let report = match depot::install(cdn, &dir, jobs, progress).await {
        Ok(r) => r,
        Err(depot::Error::Cancelled) => {
            // Unfinished files are reserved at full size: give the space back.
            remove_unfinished(&req, &dir, fresh);
            // An update that was adding depots (e.g. DLC): undo those too,
            // finished files included, so the game is as it was.
            let added: Vec<&depot::Manifest> = manifests.iter().filter(|m| !on_disk.contains(&m.depot)).collect();
            if !fresh && !added.is_empty() {
                remove_added(&req, &dir, &added, &old);
            }
            return Err(depot::Error::Cancelled.to_string());
        }
        Err(e) => return Err(e.to_string()),
    };

    // Leftovers of earlier interrupted attempts are junk once it's complete.
    remove_unfinished(&req, &dir, false);
    if !dropped.is_empty() {
        remove_dropped(&req, &dir, &dropped, &manifests);
    }
    // Leave the same records Steam does, so both clients agree.
    let cache = req.steam_root.join("depotcache");
    let _ = std::fs::create_dir_all(&cache);
    for m in &manifests {
        let _ = std::fs::write(cache.join(format!("{}_{}.manifest", m.depot, m.gid)), m.to_bytes());
    }
    write_app_manifest(&req, &app, &folder, &installed, need, owner).map_err(|e| e.to_string())?;
    Ok(report)
}

/// Delete the files of depots that are no longer installed (a DLC turned
/// off), except files another installed depot still has.
fn remove_dropped(req: &InstallRequest, dir: &Path, dropped: &[(u32, u64)], kept: &[depot::Manifest]) {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    let keep: HashSet<String> = kept.iter().flat_map(|m| m.files.iter().map(|f| norm(&f.name))).collect();
    let cache = req.steam_root.join("depotcache");
    for (depot, gid) in dropped {
        let Some(m) = std::fs::read(cache.join(format!("{depot}_{gid}.manifest"))).ok().and_then(|b| depot::Manifest::parse(&b).ok()) else {
            tracing::warn!("depot {depot} dropped but its manifest isn't cached; its files stay");
            continue;
        };
        let mut removed = 0;
        for f in m.files.iter().filter(|f| !f.directory) {
            if keep.contains(&norm(&f.name)) {
                continue;
            }
            if let Ok(path) = depot::install::resolve_inside(dir, &f.name)
                && std::fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
        tracing::info!("depot {depot} no longer installed: removed {removed} files");
    }
}

/// After a cancelled update: delete the files of depots it was adding,
/// except files the depots installed before also have.
fn remove_added(req: &InstallRequest, dir: &Path, added: &[&depot::Manifest], before: &[(u32, u64)]) {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    let cache = req.steam_root.join("depotcache");
    let keep: HashSet<String> = before
        .iter()
        .filter_map(|(d, g)| std::fs::read(cache.join(format!("{d}_{g}.manifest"))).ok().and_then(|b| depot::Manifest::parse(&b).ok()))
        .flat_map(|m| m.files.into_iter().map(|f| norm(&f.name)))
        .collect();
    let mut removed = 0;
    for m in added {
        for f in m.files.iter().filter(|f| !f.directory && !keep.contains(&norm(&f.name))) {
            if let Ok(path) = depot::install::resolve_inside(dir, &f.name)
                && std::fs::remove_file(path).is_ok()
            {
                removed += 1;
            }
        }
    }
    tracing::info!("cancelled: removed {removed} files of depots this update was adding");
}

/// After a cancelled download: delete the folder of a new install, or just
/// the unfinished `*.sirpart` files of an update (the game stays playable).
fn remove_unfinished(req: &InstallRequest, dir: &Path, fresh: bool) {
    // Only ever inside <library>/steamapps/common/<game>.
    let common = req.library.join("steamapps").join("common");
    if let Err(e) = local::validate_common_child(&common, dir) {
        tracing::warn!("cancelled: {e}");
        return;
    }
    let Ok(common_meta) = std::fs::symlink_metadata(&common) else {
        return;
    };
    if local::is_symlink_or_reparse(&common_meta) || !common_meta.is_dir() {
        tracing::warn!("cancelled: refusing symlink or non-directory {}", common.display());
        return;
    }
    let dir_meta = match std::fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!("cancelled: couldn't inspect {}: {e}", dir.display());
            return;
        }
    };
    if local::is_symlink_or_reparse(&dir_meta) {
        if fresh {
            match local::unlink_symlink_or_junction(dir, &dir_meta) {
                Ok(()) => tracing::info!("cancelled: unlinked symlink {}", dir.display()),
                Err(e) => tracing::warn!("cancelled: couldn't unlink {}: {e}", dir.display()),
            }
        } else {
            tracing::warn!("cancelled: refusing to walk symlink {}", dir.display());
        }
        return;
    }
    if !dir_meta.is_dir() {
        return;
    }
    let (Ok(canon_common), Ok(canon_dir)) = (common.canonicalize(), dir.canonicalize()) else {
        tracing::warn!("cancelled: couldn't canonicalize {}", dir.display());
        return;
    };
    if canon_dir == canon_common || canon_dir.parent() != Some(canon_common.as_path()) {
        tracing::warn!("cancelled: refusing {} outside {}", dir.display(), common.display());
        return;
    }
    if fresh {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => tracing::info!("cancelled: removed {}", dir.display()),
            Err(e) => tracing::warn!("cancelled: couldn't remove {}: {e}", dir.display()),
        }
        return;
    }
    let mut stack = vec![dir.to_path_buf()];
    let mut freed = 0u64;
    while let Some(d) = stack.pop() {
        let Ok(dm) = std::fs::symlink_metadata(&d) else { continue };
        if local::is_symlink_or_reparse(&dm) || !dm.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(em) = std::fs::symlink_metadata(&path) else { continue };
            if local::is_symlink_or_reparse(&em) {
                continue;
            }
            if em.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "sirpart") {
                freed += em.len();
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    tracing::info!("cancelled: removed {} MB of unfinished files in {}", freed / 1_048_576, dir.display());
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn write_app_manifest(req: &InstallRequest, app: &OwnedApp, install_dir: &str, depots: &[(u32, u64, u64, Option<AppId>)], size: u64, owner: u64) -> std::io::Result<()> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let build = app.branches.iter().find(|b| b.name == req.branch).map(|b| b.build_id).unwrap_or(0);
    let mut installed = vdf::Obj::default();
    for (depot, gid, size, dlc) in depots {
        let mut d = vdf::Obj::default().with("manifest", gid.to_string()).with("size", size.to_string());
        if let Some(dlc) = dlc {
            d = d.with("dlcappid", dlc.to_string());
        }
        installed = installed.with_obj(&depot.to_string(), d);
    }
    let mut user = vdf::Obj::default().with("language", &req.language);
    if req.branch != "public" {
        user = user.with("BetaKey", &req.branch);
    }
    let state = vdf::Obj::default()
        .with("appid", app.app_id.to_string())
        .with("universe", "1")
        .with("name", &app.name)
        .with("StateFlags", "4")
        .with("installdir", install_dir)
        .with("LastUpdated", now.to_string())
        .with("SizeOnDisk", size.to_string())
        .with("buildid", build.to_string())
        .with("LastOwner", owner.to_string())
        .with("AutoUpdateBehavior", "0")
        .with_obj("InstalledDepots", installed)
        .with_obj("UserConfig", user.clone())
        .with_obj("MountedConfig", user);
    let doc = vdf::Obj::default().with_obj("AppState", state);
    std::fs::write(req.library.join("steamapps").join(format!("appmanifest_{}.acf", app.app_id)), vdf::write(&doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_confirmed_key_denials_exclude_depots() {
        let reply = |result, len| CMsgClientGetDepotDecryptionKeyResponse {
            eresult: result, depot_id: Some(42), depot_encryption_key: Some(vec![0; len]),
        };
        assert!(matches!(classify_depot_key(reply(Some(eresult::ACCESS_DENIED), 0), 42), Err(DepotKeyError::Excluded(42))));
        for response in [reply(None, 0), reply(Some(2), 0), reply(Some(eresult::OK), 12)] {
            assert!(matches!(classify_depot_key(response, 42), Err(DepotKeyError::Failed(_))));
        }
        assert!(matches!(classify_depot_key(reply(Some(eresult::ACCESS_DENIED), 0), 1), Err(DepotKeyError::Failed(_))));
        assert!(classify_depot_key(reply(Some(eresult::OK), 32), 42).is_ok());
    }

    #[test]
    fn picks_windows_64bit_language_depots() {
        let d = |id, os: &str, arch: &str, lang: &str, dlc: Option<u32>| model::DepotInfo {
            id,
            os: os.into(),
            arch: arch.into(),
            language: lang.into(),
            dlc_app: dlc,
            manifests: vec![("public".into(), 1, 1)],
            ..Default::default()
        };
        let app = OwnedApp {
            app_id: 10,
            depots: vec![d(11, "windows", "64", "", None), d(12, "windows", "32", "", None), d(13, "linux", "", "", None), d(14, "", "", "german", None), d(15, "windows", "", "", Some(99))],
            ..Default::default()
        };
        let req = InstallRequest { app_id: 10, library: ".".into(), steam_root: ".".into(), branch: "public".into(), language: "english".into(), skip_dlc: vec![], add_dlc: vec![], platform: String::new() };
        let fresh = HashSet::new();
        let ids: Vec<u32> = pick_depots(&app, &HashSet::new(), &req, &fresh).iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![11]);
        let owned: HashSet<u32> = [99].into_iter().collect();
        // A new install gets the owned DLC.
        let ids: Vec<u32> = pick_depots(&app, &owned, &req, &fresh).iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![11, 15]);
        // An update of an install without it doesn't add it...
        let installed: HashSet<u32> = [11].into_iter().collect();
        let ids: Vec<u32> = pick_depots(&app, &owned, &req, &installed).iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![11]);
        // ...unless the user turned it on.
        let on = InstallRequest { add_dlc: vec![99], ..req.clone() };
        let ids: Vec<u32> = pick_depots(&app, &owned, &on, &installed).iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![11, 15]);
        // Installed DLC stays, and goes when turned off.
        let both: HashSet<u32> = [11, 15].into_iter().collect();
        assert_eq!(pick_depots(&app, &owned, &req, &both).len(), 2);
        let off = InstallRequest { skip_dlc: vec![99], ..req.clone() };
        assert_eq!(pick_depots(&app, &owned, &off, &both).iter().map(|d| d.id).collect::<Vec<_>>(), vec![11]);
        // An update keeps an installed depot in another language, or 32-bit.
        let ru = InstallRequest { language: "russian".into(), ..req.clone() };
        let with_lang: HashSet<u32> = [11, 14].into_iter().collect();
        let ids: Vec<u32> = pick_depots(&app, &HashSet::new(), &ru, &with_lang).iter().map(|d| d.id).collect();
        assert!(ids.contains(&14), "kept the installed german depot: {ids:?}");
        let with32: HashSet<u32> = [11, 12].into_iter().collect();
        let ids: Vec<u32> = pick_depots(&app, &HashSet::new(), &req, &with32).iter().map(|d| d.id).collect();
        assert!(ids.contains(&12), "kept the installed 32-bit depot: {ids:?}");
        // The Linux build when asked for, and its updates stay Linux.
        let linux = InstallRequest { platform: "linux".into(), ..req.clone() };
        let ids: Vec<u32> = pick_depots(&app, &HashSet::new(), &linux, &fresh).iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![13]);
        let on_disk: HashSet<u32> = [13].into_iter().collect();
        assert_eq!(platform_for(&app, &req, &on_disk), "linux");
        let ids: Vec<u32> = pick_depots(&app, &HashSet::new(), &req, &on_disk).iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![13]);
        assert_eq!(platform_for(&app, &req, &installed), "windows");
        assert!(has_build(&app, "linux") && has_build(&app, "windows") && !has_build(&app, "macos"));
    }
}

#[cfg(test)]
mod uninstall_tests {
    use super::*;

    #[test]
    fn remove_unfinished_rejects_dotdot_and_unlinks_only_symlink() {
        let lib = std::env::temp_dir().join(format!("sir-cancel-sec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&lib);
        let apps = lib.join("steamapps");
        let common = apps.join("common");
        let compat = apps.join("compatdata");
        std::fs::create_dir_all(&common).unwrap();
        std::fs::create_dir_all(&compat).unwrap();
        let keep = compat.join("keep.sirpart");
        std::fs::write(&keep, b"keep").unwrap();

        let req = InstallRequest {
            app_id: 1,
            library: lib.clone(),
            steam_root: lib.clone(),
            branch: "public".into(),
            language: "english".into(),
            skip_dlc: vec![],
            add_dlc: vec![],
            platform: String::new(),
        };

        for bad_dir in [common.join(".."), common.join("."), common.join("sub").join("..")] {
            remove_unfinished(&req, &bad_dir, true);
            remove_unfinished(&req, &bad_dir, false);
            assert!(keep.exists(), "remove_unfinished deleted outside common via {bad_dir:?}!");
            assert!(common.exists(), "remove_unfinished deleted common via {bad_dir:?}!");
        }

        #[cfg(unix)]
        {
            let outside = std::env::temp_dir().join(format!("sir-cancel-ext-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&outside);
            std::fs::create_dir_all(&outside).unwrap();
            let ext_part = outside.join("ext.sirpart");
            let ext_file = outside.join("ext.exe");
            std::fs::write(&ext_part, b"part").unwrap();
            std::fs::write(&ext_file, b"exe").unwrap();
            let sym = common.join("SymGame");
            std::os::unix::fs::symlink(&outside, &sym).unwrap();

            remove_unfinished(&req, &sym, false);
            assert!(ext_part.exists(), "remove_unfinished(fresh=false) walked into symlink!");
            assert!(ext_file.exists());

            remove_unfinished(&req, &sym, true);
            assert!(ext_part.exists(), "remove_unfinished(fresh=true) recursed into symlink target!");
            assert!(ext_file.exists(), "remove_unfinished(fresh=true) deleted symlink target!");
            assert!(std::fs::symlink_metadata(&sym).is_err(), "symlink itself was not unlinked!");
            let _ = std::fs::remove_dir_all(&outside);
        }

        let _ = std::fs::remove_dir_all(&lib);
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;

    #[test]
    fn cancel_removes_only_unfinished_files() {
        let lib = std::env::temp_dir().join(format!("sir-cancel-{}", std::process::id()));
        let game = lib.join("steamapps").join("common").join("Game");
        std::fs::create_dir_all(game.join("data")).unwrap();
        std::fs::write(game.join("game.exe"), b"done").unwrap();
        std::fs::write(game.join("data").join("big.pak.sirpart"), b"half").unwrap();
        let req = InstallRequest { app_id: 1, library: lib.clone(), steam_root: lib.clone(), branch: "public".into(), language: "english".into(), skip_dlc: vec![], add_dlc: vec![], platform: String::new() };
        // An update: the finished file stays, the unfinished one goes.
        remove_unfinished(&req, &game, false);
        assert!(game.join("game.exe").exists());
        assert!(!game.join("data").join("big.pak.sirpart").exists());
        // A new install: the whole folder goes.
        remove_unfinished(&req, &game, true);
        assert!(!game.exists());
        // Never outside steamapps/common.
        let other = lib.join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        remove_unfinished(&req, &other, true);
        assert!(other.exists());
        let _ = std::fs::remove_dir_all(lib);
    }

    #[test]
    fn dropped_and_added_cleanup_rejects_traversal() {
        let root = std::env::temp_dir().join(format!("sir-dl-aur02-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("steamapps").join("common").join("Game");
        let cache = root.join("depotcache");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let victim = root.join("steamapps").join("common").join("victim.txt");
        std::fs::write(&victim, b"precious").unwrap();
        std::fs::write(game.join("dlc.pak"), b"dlc").unwrap();

        let bad_manifest = depot::Manifest {
            depot: 77,
            gid: 1,
            files: vec![
                depot::manifest::FileEntry {
                    name: "../victim.txt".into(),
                    size: 8,
                    sha: vec![0; 20],
                    chunks: vec![],
                    directory: false,
                    executable: false,
                    link: None,
                },
                depot::manifest::FileEntry {
                    name: "dlc.pak".into(),
                    size: 3,
                    sha: vec![0; 20],
                    chunks: vec![],
                    directory: false,
                    executable: false,
                    link: None,
                },
            ],
            ..Default::default()
        };
        std::fs::write(cache.join("77_1.manifest"), bad_manifest.to_bytes()).unwrap();
        let req = InstallRequest {
            app_id: 1,
            library: root.clone(),
            steam_root: root.clone(),
            branch: "public".into(),
            language: "english".into(),
            skip_dlc: vec![],
            add_dlc: vec![],
            platform: String::new(),
        };
        remove_dropped(&req, &game, &[(77, 1)], &[]);
        assert!(victim.exists(), "remove_dropped deleted outside install dir!");
        assert!(!game.join("dlc.pak").exists());

        std::fs::write(game.join("dlc.pak"), b"dlc").unwrap();
        remove_added(&req, &game, &[&bad_manifest], &[]);
        assert!(victim.exists(), "remove_added deleted outside install dir!");
        assert!(!game.join("dlc.pak").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validates_install_folder_name() {
        let mk = |install_dir: &str, name: &str| OwnedApp {
            app_id: 1,
            install_dir: install_dir.into(),
            name: name.into(),
            ..Default::default()
        };
        for bad in ["..", ".", "../outside", "a/b", r"a\b", "/etc", r"C:\Windows", "CON", "game.sirpart"] {
            assert!(install_folder_name(&mk(bad, "Good")).is_err(), "accepted {bad:?}");
        }
        assert!(install_folder_name(&mk("", "..")).is_err());
        assert!(install_folder_name(&mk("", "../outside")).is_err());
        assert!(install_folder_name(&mk("", "NUL")).is_err());
        assert_eq!(install_folder_name(&mk("Half-Life 2", "HL2")).unwrap(), "Half-Life 2");
        assert_eq!(install_folder_name(&mk("", "Half-Life: Source")).unwrap(), "Half-Life_ Source");
    }
}
