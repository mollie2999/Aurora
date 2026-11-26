//! Resolving owned licenses into named apps via PICS (Steam's product info service).
//!
//! licenses -> package info (binary KV, lists app ids) -> app access tokens
//! -> app info (text KV with name, type, launch options).

use crate::Error;
use model::{AppId, LaunchOption, OwnedApp};
use net::{Connection, emsg};
use proto::*;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::time::Duration;

type Result<T> = std::result::Result<T, Error>;

/// App types shown in the library.
const LIBRARY_TYPES: &[&str] = &["game", "demo", "application", "tool", "music", "video", "dlc"];

/// Bump when [`parse_app`] reads something new, so saved libraries are
/// fetched again in full instead of missing the new field.
const PARSE_VERSION: u32 = 1;
/// A full refresh at least this often, whatever the change numbers say.
const FULL_EVERY: u64 = 7 * 24 * 60 * 60;

/// What the last library refresh found, so the next sign-in only asks
/// Steam for what changed since (PICS change numbers, like Steam's own
/// appinfo cache) instead of every app the account owns.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Cache {
    parse_version: u32,
    app_version: String,
    /// Unix seconds of the last full refresh.
    full_at: u64,
    change_number: u32,
    /// Each owned package's app ids.
    packages: BTreeMap<u32, Vec<AppId>>,
    /// Library apps, as resolved.
    apps: Vec<OwnedApp>,
    /// Other app ids already looked at (not shown in the library).
    others: BTreeSet<AppId>,
}

fn cache_path(steam_id: model::SteamId) -> std::path::PathBuf {
    crate::offline::dir(steam_id).join("pics.json")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The library for these licenses. With a saved library, only the packages
/// and apps that changed since are fetched (falls back to everything).
pub async fn resolve(conn: &Connection, licenses: &[(u32, u64)], steam_id: model::SteamId) -> Result<Vec<OwnedApp>> {
    let started = std::time::Instant::now();
    let path = cache_path(steam_id);
    let saved: Option<Cache> = crate::offline::read(&path)
        .filter(|c: &Cache| c.parse_version == PARSE_VERSION && c.app_version == env!("CARGO_PKG_VERSION") && c.change_number != 0)
        .filter(|c| unix_now().saturating_sub(c.full_at) < FULL_EVERY);
    let changes = changes_since(conn, saved.as_ref().map_or(0, |c| c.change_number)).await;
    let current = changes.as_ref().map(|c| c.current_change_number()).unwrap_or(0);
    let (cache, how) = match (saved, changes) {
        (Some(c), Ok(ch)) if !(ch.force_full_update() || ch.force_full_app_update() || ch.force_full_package_update()) => {
            let (c, fetched) = update(conn, licenses, c, &ch).await?;
            (c, format!("{fetched} changed (change {})", ch.current_change_number()))
        }
        _ => (full(conn, licenses).await?, "full refresh".to_string()),
    };
    let mut cache = cache;
    cache.change_number = current;
    cache.parse_version = PARSE_VERSION;
    cache.app_version = env!("CARGO_PKG_VERSION").to_string();
    let mut apps = cache.apps.clone();
    if current != 0 {
        tokio::task::spawn_blocking(move || crate::offline::write(&path, &cache));
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    tracing::info!("library: {} apps in {:.1} s ({how})", apps.len(), started.elapsed().as_secs_f32());
    Ok(apps)
}

async fn changes_since(conn: &Connection, since: u32) -> Result<CMsgClientPicsChangesSinceResponse> {
    let req = CMsgClientPicsChangesSinceRequest { since_change_number: Some(since), send_app_info_changes: Some(true), send_package_info_changes: Some(true), ..Default::default() };
    Ok(conn.request(emsg::CLIENT_PICS_CHANGES_SINCE_REQUEST, &req).await?.body()?)
}

/// Everything, from scratch.
async fn full(conn: &Connection, licenses: &[(u32, u64)]) -> Result<Cache> {
    let packages = package_apps(conn, licenses).await?;
    let ids: BTreeSet<AppId> = packages.values().flatten().copied().collect();
    tracing::info!("{} licenses -> {} app ids", licenses.len(), ids.len());
    let mut cache = Cache { packages, full_at: unix_now(), ..Default::default() };
    fetch_apps(conn, &mut cache, ids.into_iter().collect()).await?;
    Ok(cache)
}

/// Only what changed: new or changed packages, then new or changed apps.
async fn update(conn: &Connection, licenses: &[(u32, u64)], mut cache: Cache, ch: &CMsgClientPicsChangesSinceResponse) -> Result<(Cache, usize)> {
    let changed_packages: HashSet<u32> = ch.package_changes.iter().map(|p| p.packageid()).collect();
    let stale = stale_packages(&mut cache, licenses, &changed_packages);
    if !stale.is_empty() {
        let fresh = package_apps(conn, &stale).await?;
        for (p, _) in &stale {
            cache.packages.insert(*p, fresh.get(p).cloned().unwrap_or_default());
        }
    }
    let changed_apps: HashSet<AppId> = ch.app_changes.iter().map(|a| a.appid()).collect();
    let fetch = apps_to_fetch(&mut cache, &changed_apps);
    let n = fetch.len();
    if n > 0 {
        fetch_apps(conn, &mut cache, fetch).await?;
    }
    Ok((cache, n))
}

/// Forget packages no longer owned; the owned ones that are new or changed.
fn stale_packages(cache: &mut Cache, licenses: &[(u32, u64)], changed: &HashSet<u32>) -> Vec<(u32, u64)> {
    let owned: HashSet<u32> = licenses.iter().map(|l| l.0).collect();
    cache.packages.retain(|p, _| owned.contains(p));
    licenses.iter().copied().filter(|(p, _)| !cache.packages.contains_key(p) || changed.contains(p)).collect()
}

/// Forget apps no longer owned and changed ones; the apps to fetch (new or changed).
fn apps_to_fetch(cache: &mut Cache, changed: &HashSet<AppId>) -> Vec<AppId> {
    let ids: BTreeSet<AppId> = cache.packages.values().flatten().copied().collect();
    let known: HashSet<AppId> = cache.apps.iter().map(|a| a.app_id).chain(cache.others.iter().copied()).collect();
    let fetch: Vec<AppId> = ids.iter().copied().filter(|id| !known.contains(id) || changed.contains(id)).collect();
    let refetch: HashSet<AppId> = fetch.iter().copied().collect();
    cache.apps.retain(|a| ids.contains(&a.app_id) && !refetch.contains(&a.app_id));
    cache.others.retain(|id| ids.contains(id) && !refetch.contains(id));
    fetch
}

/// Fetch these apps' info into the cache: library apps, and the rest as
/// "seen" so they aren't asked for again until they change.
async fn fetch_apps(conn: &Connection, cache: &mut Cache, ids: Vec<AppId>) -> Result<()> {
    let tokens = app_tokens(conn, &ids).await?;
    let apps = app_info(conn, &ids, &tokens).await?;
    let got: HashSet<AppId> = apps.iter().map(|a| a.app_id).collect();
    for a in apps {
        if LIBRARY_TYPES.contains(&a.kind.as_str()) {
            cache.apps.push(a);
        } else {
            cache.others.insert(a.app_id);
        }
    }
    cache.others.extend(ids.into_iter().filter(|id| !got.contains(id)));
    Ok(())
}

async fn product_info(
    conn: &Connection,
    req: &CMsgClientPicsProductInfoRequest,
) -> Result<Vec<CMsgClientPicsProductInfoResponse>> {
    conn.request_many_decoded(
        emsg::CLIENT_PICS_PRODUCT_INFO_REQUEST,
        req,
        |r: &CMsgClientPicsProductInfoResponse| !r.response_pending(),
    )
    .await
    .map_err(Error::from)
}

/// Each package's app ids.
async fn package_apps(conn: &Connection, licenses: &[(u32, u64)]) -> Result<BTreeMap<u32, Vec<AppId>>> {
    let mut out: BTreeMap<u32, Vec<AppId>> = BTreeMap::new();
    for chunk in licenses.chunks(500) {
        let req = CMsgClientPicsProductInfoRequest {
            packages: chunk
                .iter()
                .map(|&(id, token)| c_msg_client_pics_product_info_request::PackageInfo {
                    packageid: Some(id),
                    access_token: Some(token),
                })
                .collect(),
            ..Default::default()
        };
        for resp in product_info(conn, &req).await? {
            for pkg in resp.packages {
                // Buffer is a u32 package id followed by binary KV.
                let id = pkg.packageid();
                let Some(buf) = pkg.buffer.filter(|b| b.len() > 4) else { continue };
                let Ok(kv) = vdf::binary::parse(&buf[4..]) else { continue };
                let list = out.entry(id).or_default();
                for (_, v) in kv.iter() {
                    let Some(apps) = v.get("appids").and_then(|a| a.as_obj()) else { continue };
                    list.extend(apps.iter().filter_map(|(_, a)| a.as_str()?.parse::<AppId>().ok()));
                }
            }
        }
    }
    Ok(out)
}

async fn app_tokens(conn: &Connection, ids: &[AppId]) -> Result<HashMap<AppId, u64>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let reply = conn
            .request(
                emsg::CLIENT_PICS_ACCESS_TOKEN_REQUEST,
                &CMsgClientPicsAccessTokenRequest { appids: chunk.to_vec(), packageids: vec![] },
            )
            .await?;
        let resp: CMsgClientPicsAccessTokenResponse = reply.body()?;
        out.extend(resp.app_access_tokens.iter().map(|t| (t.appid(), t.access_token())));
    }
    Ok(out)
}

async fn app_info(
    conn: &Connection,
    ids: &[AppId],
    tokens: &HashMap<AppId, u64>,
) -> Result<Vec<OwnedApp>> {
    use futures_util::stream::{self, StreamExt, TryStreamExt};
    let http = crate::http();
    // A few batches at once instead of one after another.
    let requests: Vec<CMsgClientPicsProductInfoRequest> = ids
        .chunks(200)
        .map(|chunk| CMsgClientPicsProductInfoRequest {
            apps: chunk
                .iter()
                .map(|&id| c_msg_client_pics_product_info_request::AppInfo {
                    appid: Some(id),
                    access_token: tokens.get(&id).copied(),
                    only_public_obsolete: None,
                })
                .collect(),
            ..Default::default()
        })
        .collect();
    let replies: Vec<Vec<CMsgClientPicsProductInfoResponse>> =
        stream::iter(requests.into_iter().map(|req| {
            let conn = conn.clone();
            async move { product_info(&conn, &req).await }
        }))
        .buffer_unordered(4)
        .try_collect()
        .await?;
    let mut out = Vec::new();
    let mut remote = Vec::new();
    for resp in replies.into_iter().flatten() {
        let host = resp.http_host.clone().filter(|h| !h.is_empty());
        for app in resp.apps {
            let id = app.appid();
            match app.buffer.as_deref().filter(|b| !b.is_empty()) {
                Some(buf) => {
                    if let Some(a) = parse_app(id, String::from_utf8_lossy(buf).trim_end_matches('\0')) {
                        out.push(a);
                    }
                }
                // Large entries are served over HTTP instead of inline.
                None => {
                    if let (Some(h), Some(sha)) = (host.clone(), app.sha.clone()) {
                        remote.push((id, h, sha));
                    }
                }
            }
        }
    }
    // Those downloads run 16 at a time instead of one by one.
    let fetched: Vec<Option<OwnedApp>> = stream::iter(remote.into_iter().map(|(id, h, sha)| {
        let http = http.clone();
        async move { fetch_http(&http, &h, id, &sha).await.and_then(|t| parse_app(id, t.trim_end_matches('\0'))) }
    }))
    .buffer_unordered(16)
    .collect()
    .await;
    out.extend(fetched.into_iter().flatten());
    Ok(out)
}

/// One app's whole PICS app info (the `appinfo` object), with its access token.
pub async fn raw_app_info(conn: &Connection, id: AppId) -> Option<vdf::Obj> {
    let tokens = app_tokens(conn, &[id]).await.unwrap_or_default();
    let req = CMsgClientPicsProductInfoRequest {
        apps: vec![c_msg_client_pics_product_info_request::AppInfo { appid: Some(id), access_token: tokens.get(&id).copied(), only_public_obsolete: None }],
        ..Default::default()
    };
    let http = crate::http();
    for resp in product_info(conn, &req).await.ok()? {
        let host = resp.http_host.clone().filter(|h| !h.is_empty());
        if let Some(app) = resp.apps.into_iter().find(|a| a.appid() == id) {
            let text = match app.buffer.as_deref().filter(|b| !b.is_empty()) {
                Some(buf) => String::from_utf8_lossy(buf).into_owned(),
                None => fetch_http(&http, host.as_deref()?, id, app.sha.as_deref()?).await?,
            };
            return vdf::parse(text.trim_end_matches('\0')).ok()?.obj("appinfo").cloned();
        }
    }
    None
}

/// Names of any apps, from their public info (no ownership needed).
pub async fn names(conn: &Connection, ids: &[AppId]) -> Result<Vec<(AppId, String)>> {
    let apps = app_info(conn, ids, &HashMap::new()).await?;
    Ok(apps.into_iter().filter(|a| a.name != "Unknown").map(|a| (a.app_id, a.name)).collect())
}

/// `(dlc, parent game, name)` for the DLC among `ids`, from their public info.
pub async fn dlc_parents(conn: &Connection, ids: &[AppId]) -> Result<Vec<(AppId, AppId, String)>> {
    let apps = app_info(conn, ids, &HashMap::new()).await?;
    Ok(apps.into_iter().filter_map(|a| a.parent.map(|p| (a.app_id, p, a.name))).collect())
}

const MAX_PICS_COMPRESSED_BYTES: usize = 32 * 1024 * 1024;
const MAX_PICS_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;

fn is_valid_http_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 255 {
        return false;
    }
    if !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b':' | b'-')) {
        return false;
    }
    let name = match host.split_once(':') {
        Some((h, port)) => {
            if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            if port.parse::<u16>().ok().is_none_or(|p| p == 0) {
                return false;
            }
            h
        }
        None => host,
    };
    if name.is_empty() || name.starts_with('.') || name.ends_with('.') || name.contains("..") {
        return false;
    }
    name.split('.').all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'))
}

fn decode_and_verify_pics(bytes: &[u8], expected_sha: &[u8]) -> Option<String> {
    if expected_sha.len() != 20 {
        return None;
    }
    let mut decoded = Vec::new();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut gz = flate2::read::GzDecoder::new(bytes).take((MAX_PICS_DECOMPRESSED_BYTES + 1) as u64);
        gz.read_to_end(&mut decoded).ok()?;
        if decoded.len() > MAX_PICS_DECOMPRESSED_BYTES {
            return None;
        }
    } else {
        if bytes.len() > MAX_PICS_DECOMPRESSED_BYTES {
            return None;
        }
        decoded.extend_from_slice(bytes);
    }

    // Valve's PICS `app.sha` is the 20-byte SHA-1 of the uncompressed VDF
    // payload without its trailing NUL byte (`decoded[..len-1]`), matching
    // inline `app.buffer`; also accept an exact SHA-1 match on the raw
    // `.txt.gz` stream or uncompressed bytes without a NUL terminator.
    let without_nul = decoded.strip_suffix(b"\0").unwrap_or(&decoded);
    let matches_sha = Sha1::digest(without_nul).as_slice() == expected_sha
        || Sha1::digest(&decoded).as_slice() == expected_sha
        || Sha1::digest(bytes).as_slice() == expected_sha;
    if !matches_sha {
        return None;
    }
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

async fn fetch_http(http: &reqwest::Client, host: &str, id: AppId, sha: &[u8]) -> Option<String> {
    if sha.len() != 20 || !is_valid_http_host(host) {
        return None;
    }
    let sha_hex: String = sha.iter().map(|b| format!("{b:02x}")).collect();
    let url = format!("https://{host}/appinfo/{id}/sha/{sha_hex}.txt.gz");
    let parsed = reqwest::Url::parse(&url).ok()?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let mut resp = tokio::time::timeout(Duration::from_secs(30), http.get(parsed).send())
        .await
        .ok()?
        .ok()?
        .error_for_status()
        .ok()?;
    if resp.url().scheme() != "https" {
        return None;
    }
    if resp.content_length().is_some_and(|len| len > MAX_PICS_COMPRESSED_BYTES as u64) {
        return None;
    }
    let mut bytes = Vec::with_capacity(resp.content_length().unwrap_or(0).min(256 * 1024) as usize);
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(30), resp.chunk()).await.ok()?.ok()? {
        if bytes.len().checked_add(chunk.len()).is_none_or(|n| n > MAX_PICS_COMPRESSED_BYTES) {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    decode_and_verify_pics(&bytes, sha)
}

fn parse_app(id: AppId, text: &str) -> Option<OwnedApp> {
    let kv = vdf::parse(text).ok()?;
    let info = kv.obj("appinfo")?;
    let common = info.obj("common")?;
    let launch = info
        .path(&["config", "launch"])
        .map(|l| {
            l.iter()
                .filter_map(|(_, v)| {
                    let o = v.as_obj()?;
                    Some(LaunchOption {
                        executable: o.str("executable").unwrap_or_default().into(),
                        arguments: o.str("arguments").unwrap_or_default().into(),
                        working_dir: o.str("workingdir").unwrap_or_default().into(),
                        description: o.str("description").unwrap_or_default().into(),
                        os_list: o
                            .obj("config")
                            .and_then(|c| c.str("oslist"))
                            .unwrap_or_default()
                            .into(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(OwnedApp {
        app_id: id,
        name: common.str("name").unwrap_or("Unknown").to_string(),
        kind: common.str("type").unwrap_or_default().to_lowercase(),
        launch,
        icon: common.str("icon").or(common.str("clienticon")).filter(|s| !s.is_empty()).map(String::from),
        parent: common.u64("parent").map(|p| p as AppId).filter(|p| *p != 0),
        install_dir: info.path(&["config"]).and_then(|c| c.str("installdir")).unwrap_or_default().to_string(),
        install_script: info.path(&["extended"]).and_then(|e| e.str("installscript")).filter(|s| !s.is_empty()).map(String::from),
        depots: parse_depots(info),
        branches: parse_branches(info),
        cloud: parse_cloud(info),
        store_assets: common.obj("header_image").is_some() || common.str("store_asset_mtime").is_some() || common.obj("library_assets").is_some(),
        art: art_paths(common),
        categories: common.obj("category").map(|c| c.iter().filter_map(|(k, _)| k.strip_prefix("category_")?.parse().ok()).collect()).unwrap_or_default(),
        genres: common.obj("genres").map(|g| g.iter().filter_map(|(_, v)| v.as_str()?.parse().ok()).collect()).unwrap_or_default(),
        deck: common.obj("steam_deck_compatibility").and_then(|d| d.u64("category")).unwrap_or(0) as u8,
    })
}

/// Art in Steam's newer layout ("<hash>/header.jpg"): the header, and the
/// library capsule, hero and logo from `library_assets_full`. English first.
fn art_paths(common: &vdf::Obj) -> Vec<String> {
    fn walk(o: &vdf::Obj, english: bool, out: &mut Vec<(bool, String)>) {
        for (k, v) in o.iter() {
            match v {
                vdf::Value::Str(s) if s.contains('/') => out.push((english || k == "english", s.clone())),
                vdf::Value::Obj(inner) => walk(inner, english || k == "english", out),
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    for key in ["header_image", "library_assets_full"] {
        if let Some(o) = common.obj(key) {
            walk(o, false, &mut found);
        }
    }
    // English versions win when a file has several languages.
    found.sort_by_key(|(english, _)| !*english);
    let mut seen = std::collections::HashSet::new();
    found.into_iter().filter(|(_, p)| seen.insert(p.rsplit('/').next().unwrap_or("").to_string())).map(|(_, p)| p).collect()
}

/// The `ufs` section: Steam Cloud quota and Auto-Cloud save folders.
fn parse_cloud(info: &vdf::Obj) -> Option<model::CloudConfig> {
    let ufs = info.obj("ufs")?;
    let quota = ufs.u64("quota").unwrap_or(0);
    let save_files: Vec<model::SaveRule> = ufs
        .obj("savefiles")
        .map(|s| {
            s.iter()
                .filter_map(|(_, v)| {
                    let o = v.as_obj()?;
                    // Skip rules for other systems.
                    let platforms: Vec<String> = o.obj("platforms").map(|p| p.iter().filter_map(|(_, v)| v.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default();
                    if !platforms.is_empty() && !platforms.iter().any(|p| p == "windows" || p == "all") {
                        return None;
                    }
                    Some(model::SaveRule {
                        root: o.str("root").unwrap_or_default().to_string(),
                        path: o.str("path").unwrap_or_default().to_string(),
                        pattern: o.str("pattern").unwrap_or("*").to_string(),
                        recursive: o.str("recursive") == Some("1"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (quota > 0 || !save_files.is_empty()).then_some(model::CloudConfig { quota, max_files: ufs.u64("maxnumfiles").unwrap_or(0) as u32, save_files })
}

fn parse_depots(info: &vdf::Obj) -> Vec<model::DepotInfo> {
    let Some(depots) = info.obj("depots") else { return Vec::new() };
    depots
        .iter()
        .filter_map(|(id, v)| {
            let id: u32 = id.parse().ok()?;
            let o = v.as_obj()?;
            let config = o.obj("config");
            let c = |k: &str| config.and_then(|c| c.str(k)).unwrap_or_default().to_string();
            let mut manifests = Vec::new();
            if let Some(m) = o.obj("manifests") {
                for (branch, val) in m.iter() {
                    // New format: { "gid": .., "size": .. }; old: plain gid.
                    let (gid, size) = match val {
                        vdf::Value::Obj(x) => (x.u64("gid").unwrap_or(0), x.u64("size").unwrap_or(0)),
                        vdf::Value::Str(s) => (s.parse().unwrap_or(0), 0),
                    };
                    if gid != 0 {
                        manifests.push((branch.to_string(), gid, size));
                    }
                }
            }
            Some(model::DepotInfo {
                id,
                os: c("oslist"),
                arch: c("osarch"),
                language: c("language"),
                low_violence: c("lowviolence") == "1",
                dlc_app: o.u64("dlcappid").map(|d| d as AppId),
                shared_install: o.str("sharedinstall") == Some("1") || o.u64("depotfromapp").is_some(),
                manifests,
            })
        })
        .collect()
}

fn parse_branches(info: &vdf::Obj) -> Vec<model::Branch> {
    let Some(branches) = info.path(&["depots", "branches"]) else { return Vec::new() };
    let mut out: Vec<model::Branch> = branches
        .iter()
        .filter_map(|(name, v)| {
            let o = v.as_obj()?;
            Some(model::Branch {
                name: name.to_string(),
                description: o.str("description").unwrap_or_default().to_string(),
                build_id: o.u64("buildid").unwrap_or(0),
                updated: o.u64("timeupdated").unwrap_or(0),
                password: o.str("pwdrequired") == Some("1"),
            })
        })
        .collect();
    out.sort_by_key(|b| (b.name != "public", std::cmp::Reverse(b.updated)));
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_new_style_art() {
        // Like Rust's Warhammer Pack: art only under hashed folders.
        let text = r#""appinfo" { "common" { "name" "Pack" "type" "DLC"
            "header_image" { "english" "3833/header.jpg" "russian" "77aa/header.jpg" }
            "library_assets_full" { "library_capsule" { "image" { "english" "ac2f/library_600x900.jpg" } }
                "library_logo" { "image" { "english" "logo.png" } } } } }"#;
        let app = super::parse_app(3891560, text).unwrap();
        assert_eq!(app.art, vec!["3833/header.jpg".to_string(), "ac2f/library_600x900.jpg".to_string()]);
        for p in &app.art {
            model::art::remember(3891560, p);
        }
        assert!(model::art::header(3891560).ends_with("/store_item_assets/steam/apps/3891560/3833/header.jpg"));
        // Old-style apps keep the classic address.
        assert!(model::art::logo(3891560).ends_with("/steam/apps/3891560/logo.png"));
    }

    #[test]
    fn parses_appinfo_text() {
        let text = r#""appinfo" { "appid" "440" "common" { "name" "Team Fortress 2" "type" "Game" }
            "config" { "launch" { "0" { "executable" "tf_win64.exe" "arguments" "-game tf"
            "config" { "oslist" "windows" } } } } }"#;
        let app = super::parse_app(440, text).unwrap();
        assert_eq!(app.name, "Team Fortress 2");
        assert_eq!(app.kind, "game");
        assert_eq!(app.launch[0].executable, "tf_win64.exe");
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn app(id: AppId) -> OwnedApp {
        OwnedApp { app_id: id, kind: "game".into(), ..Default::default() }
    }

    #[test]
    fn only_new_or_changed_things_are_fetched() {
        let mut cache = Cache {
            packages: BTreeMap::from([(1, vec![10, 11]), (2, vec![20]), (3, vec![30])]),
            apps: vec![app(10), app(11), app(20), app(30)],
            others: BTreeSet::from([12]),
            ..Default::default()
        };
        cache.packages.get_mut(&1).unwrap().push(12);
        // Package 3 refunded, package 4 new, package 2 changed.
        let licenses = [(1, 0), (2, 0), (4, 0)];
        let stale = stale_packages(&mut cache, &licenses, &HashSet::from([2]));
        assert_eq!(stale, vec![(2, 0), (4, 0)]);
        assert!(!cache.packages.contains_key(&3));
        // Package 2 now also has 21; package 4 brings 40.
        cache.packages.insert(2, vec![20, 21]);
        cache.packages.insert(4, vec![40]);
        // App 11 changed upstream; app 99 changed but isn't ours.
        let fetch = apps_to_fetch(&mut cache, &HashSet::from([11, 99]));
        assert_eq!(fetch, vec![11, 21, 40]);
        let kept: Vec<AppId> = cache.apps.iter().map(|a| a.app_id).collect();
        assert_eq!(kept, vec![10, 20], "30 is gone with its package, 11 is fetched again");
        assert_eq!(cache.others, BTreeSet::from([12]));
    }

    #[test]
    fn nothing_changed_fetches_nothing() {
        let mut cache = Cache { packages: BTreeMap::from([(1, vec![10])]), apps: vec![app(10)], ..Default::default() };
        assert!(stale_packages(&mut cache, &[(1, 0)], &HashSet::new()).is_empty());
        assert!(apps_to_fetch(&mut cache, &HashSet::new()).is_empty());
        assert_eq!(cache.apps.len(), 1);
    }

    #[test]
    fn validates_pics_http_host_and_verifies_appinfo_sha1() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        assert!(is_valid_http_host("clientconfig.akamai.steamstatic.com"));
        assert!(is_valid_http_host("steamcache.local:8443"));
        assert!(is_valid_http_host("127.0.0.1:443"));
        assert!(!is_valid_http_host(""));
        assert!(!is_valid_http_host("evil.com/../../etc"));
        assert!(!is_valid_http_host("user:pass@evil.com"));
        assert!(!is_valid_http_host("evil.com?query=1"));
        assert!(!is_valid_http_host("evil.com#frag"));
        assert!(!is_valid_http_host("evil..com"));
        assert!(!is_valid_http_host("evil.com:0"));
        assert!(!is_valid_http_host("evil.com:99999"));
        assert!(!is_valid_http_host("evil.com:80:80"));

        let vdf_body = b"\"appinfo\"\n{\n\t\"appid\"\t\"440\"\n}\n";
        let mut with_nul = vdf_body.to_vec();
        with_nul.push(0);

        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&with_nul).unwrap();
        let gz_bytes = enc.finish().unwrap();

        // Valve PICS sha = 20-byte SHA-1 of uncompressed VDF without trailing NUL.
        let pics_sha: [u8; 20] = Sha1::digest(vdf_body).into();
        let gz_sha: [u8; 20] = Sha1::digest(&gz_bytes).into();

        let decoded = decode_and_verify_pics(&gz_bytes, &pics_sha).expect("valid Valve PICS .txt.gz should verify");
        assert_eq!(decoded.trim_end_matches('\0').as_bytes(), vdf_body);

        let decoded_gz_sha = decode_and_verify_pics(&gz_bytes, &gz_sha).expect("direct .txt.gz SHA-1 should also verify");
        assert_eq!(decoded_gz_sha.trim_end_matches('\0').as_bytes(), vdf_body);

        // Empty, short, or mismatched SHA-1 MUST be rejected.
        assert!(decode_and_verify_pics(&gz_bytes, &[]).is_none());
        assert!(decode_and_verify_pics(&gz_bytes, &pics_sha[..19]).is_none());
        let mut wrong_sha = pics_sha;
        wrong_sha[0] ^= 0x55;
        assert!(decode_and_verify_pics(&gz_bytes, &wrong_sha).is_none());

        // Tampered .txt.gz MUST be rejected.
        let mut tampered_enc = GzEncoder::new(Vec::new(), Compression::default());
        tampered_enc.write_all(b"\"appinfo\"\n{\n\t\"appid\"\t\"666\"\n}\n\0").unwrap();
        let tampered_gz = tampered_enc.finish().unwrap();
        assert!(decode_and_verify_pics(&tampered_gz, &pics_sha).is_none());

        // `fetch_http` rejects invalid host or non-20-byte sha before touching the network.
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let client = reqwest::Client::new();
            assert!(fetch_http(&client, "evil.com/escape", 440, &pics_sha).await.is_none());
            assert!(fetch_http(&client, "127.0.0.1:1", 440, &[]).await.is_none());
        });
    }
}
