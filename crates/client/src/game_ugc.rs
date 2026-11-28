//! Workshop for games (ISteamUGC in our steamclient): queries, the player's
//! subscriptions, votes and item downloads, through Steam's PublishedFile
//! service. Downloads go through [`crate::workshop`], like the library's.

use crate::ipc::protocol::{Reply, UgcItem, UgcQuery, UgcUpdate};
use model::AppId;
use net::{Connection, eresult};
use proto::*;
use std::path::Path;

/// Steam's page size for UGC queries (kNumUGCResultsPerPage).
const PER_PAGE: u32 = 50;

fn code(e: &net::Error) -> i32 {
    match e {
        net::Error::EResult(c, _) => *c,
        _ => eresult::FAIL,
    }
}

fn steam_id_of(account: u32) -> u64 {
    0x0110_0001_0000_0000 | account as u64
}

/// EUGCMatchingUGCType → EPublishedFileInfoMatchingFileType.
fn file_type(matching: i32) -> u32 {
    match matching {
        1 => 17,  // Items_Mtx
        2 => 18,  // Items_ReadyToUse
        3 => 1,   // Collections
        4 => 2,   // Artwork
        5 => 3,   // Videos
        6 => 4,   // Screenshots
        7 => 10,  // AllGuides
        8 => 11,  // WebGuides
        9 => 12,  // IntegratedGuides
        10 => 13, // UsableInGame
        11 => 15, // ControllerBindings
        12 => 20, // GameManagedItems
        _ => 0,   // Items (and "all")
    }
}

/// EUserUGCList → GetUserFiles' list name.
fn user_list(list: i32) -> &'static str {
    match list {
        1 => "votedon",
        2 => "votedup",
        3 => "voteddown",
        4 => "votelater",
        5 => "myfavorites",
        6 => "mysubscriptions",
        7 => "usedorplayed",
        8 => "followed",
        _ => "myfiles",
    }
}

/// EUserUGCListSortOrder → GetUserFiles' sort name.
fn user_sort(sort: i32) -> &'static str {
    match sort {
        1 => "createdasc",
        2 => "title",
        3 => "lastupdated",
        4 => "subscriptiondate",
        5 => "score",
        6 => "formoderation",
        _ => "created",
    }
}

pub fn item(d: &PublishedFileDetails) -> UgcItem {
    let votes = d.vote_data.as_ref();
    let mut stats = vec![
        (0, d.subscriptions() as u64),
        (1, d.favorited() as u64),
        (2, d.followers() as u64),
        (3, d.lifetime_subscriptions() as u64),
        (4, d.lifetime_favorited() as u64),
        (5, d.lifetime_followers() as u64),
        (6, d.views() as u64),
        (8, d.lifetime_playtime()),
        (9, d.lifetime_playtime_sessions()),
        (10, d.num_comments_public().max(0) as u64),
    ];
    if let Some(p) = &d.playtime_stats {
        stats.push((11, p.playtime_seconds()));
        stats.push((12, p.num_sessions()));
    }
    UgcItem {
        id: d.publishedfileid(),
        result: d.result() as i32,
        file_type: d.file_type() as i32,
        creator_app: d.creator_appid(),
        consumer_app: d.consumer_appid(),
        title: d.title().to_string(),
        description: if d.file_description().is_empty() { d.short_description().to_string() } else { d.file_description().to_string() },
        owner: d.creator(),
        created: d.time_created(),
        updated: d.time_updated(),
        visibility: d.visibility() as i32,
        banned: d.banned(),
        accepted: d.workshop_accepted(),
        tags: d.tags.iter().filter(|t| !t.adminonly()).map(|t| (t.tag().to_string(), t.display_name().to_string())).collect(),
        file: d.hcontent_file(),
        preview_file: d.hcontent_preview(),
        file_name: d.filename().to_string(),
        file_size: d.file_size(),
        preview_size: d.preview_file_size(),
        url: d.url().to_string(),
        votes_up: votes.map(|v| v.votes_up()).unwrap_or(0),
        votes_down: votes.map(|v| v.votes_down()).unwrap_or(0),
        score: votes.map(|v| v.score()).unwrap_or(0.0),
        children: d.children.iter().map(|c| c.publishedfileid()).collect(),
        preview_url: d.preview_url().to_string(),
        metadata: d.metadata().to_string(),
        kv: d.kvtags.iter().map(|t| (t.key().to_string(), t.value().to_string())).collect(),
        previews: d
            .previews
            .iter()
            .map(|p| if p.youtubevideoid().is_empty() { (p.url().to_string(), p.preview_type() as i32, p.filename().to_string()) } else { (p.youtubevideoid().to_string(), 1, String::new()) })
            .collect(),
        stats,
    }
}

pub async fn query(conn: &Connection, app_id: AppId, q: &UgcQuery) -> Reply {
    let language = crate::hub::language_id(if q.language.is_empty() { "english" } else { &q.language });
    let consumer = if q.consumer_app == 0 { app_id } else { q.consumer_app };
    let creator = if q.creator_app == 0 { consumer } else { q.creator_app };
    let fail = |e: net::Error| {
        tracing::info!("workshop query failed: {e}");
        Reply::UgcResults { eresult: code(&e), total: 0, items: Vec::new(), cursor: String::new() }
    };
    let ok = |total: u32, details: &[PublishedFileDetails], cursor: String| Reply::UgcResults { eresult: eresult::OK, total, items: details.iter().map(item).collect(), cursor };
    match q.kind {
        2 => {
            let req = CPublishedFileGetDetailsRequest {
                publishedfileids: q.ids.clone(),
                includetags: Some(true),
                includeadditionalpreviews: Some(q.previews),
                includechildren: Some(q.children),
                includekvtags: Some(q.kv_tags),
                includevotes: Some(true),
                short_description: Some(!q.long_description),
                includemetadata: Some(q.metadata),
                language: Some(language),
                return_playtime_stats: Some(q.playtime_days).filter(|d| *d > 0),
                appid: Some(consumer),
                ..Default::default()
            };
            match conn.call::<_, CPublishedFileGetDetailsResponse>("PublishedFile.GetDetails#1", &req).await {
                Ok(r) => ok(r.publishedfiledetails.len() as u32, &r.publishedfiledetails, String::new()),
                Err(e) => fail(e),
            }
        }
        0 => {
            let req = CPublishedFileGetUserFilesRequest {
                steamid: Some(steam_id_of(q.account)),
                appid: Some(consumer),
                creator_appid: Some(creator),
                page: Some(q.page.max(1)),
                numperpage: Some(PER_PAGE),
                r#type: Some(user_list(q.list).into()),
                sortmethod: Some(user_sort(q.sort).into()),
                filetype: Some(file_type(q.matching)),
                requiredtags: q.required_tags.clone(),
                excludedtags: q.excluded_tags.clone(),
                match_cloud_filename: Some(q.cloud_filename.clone()).filter(|s| !s.is_empty()),
                cache_max_age_seconds: Some(q.max_age),
                language: Some(language),
                totalonly: Some(q.total_only),
                ids_only: Some(q.only_ids),
                return_vote_data: Some(true),
                return_tags: Some(true),
                return_kv_tags: Some(q.kv_tags),
                return_previews: Some(q.previews),
                return_children: Some(q.children),
                return_short_description: Some(!q.long_description),
                return_metadata: Some(q.metadata),
                return_playtime_stats: Some(q.playtime_days).filter(|d| *d > 0),
                ..Default::default()
            };
            match conn.call::<_, CPublishedFileGetUserFilesResponse>("PublishedFile.GetUserFiles#1", &req).await {
                Ok(r) => ok(r.total(), &r.publishedfiledetails, String::new()),
                Err(e) => fail(e),
            }
        }
        _ => {
            let req = CPublishedFileQueryFilesRequest {
                query_type: Some(q.query.max(0) as u32),
                page: if q.cursor.is_some() { None } else { Some(q.page.max(1)) },
                cursor: q.cursor.clone(),
                numperpage: Some(PER_PAGE),
                creator_appid: Some(creator),
                appid: Some(consumer),
                requiredtags: q.required_tags.clone(),
                excludedtags: q.excluded_tags.clone(),
                match_all_tags: Some(!q.match_any),
                search_text: Some(q.search.clone()).filter(|s| !s.is_empty()),
                filetype: Some(file_type(q.matching)),
                days: Some(q.trend_days).filter(|d| *d > 0),
                cache_max_age_seconds: Some(q.max_age),
                language: Some(language),
                required_kv_tags: q.kv.iter().map(|(k, v)| c_published_file_query_files_request::KvTag { key: Some(k.clone()), value: Some(v.clone()) }).collect(),
                taggroups: q.tag_groups.iter().map(|g| c_published_file_query_files_request::TagGroup { tags: g.clone() }).collect(),
                date_range_created: (q.created != (0, 0))
                    .then_some(c_published_file_query_files_request::DateRange { timestamp_start: Some(q.created.0), timestamp_end: Some(q.created.1) }),
                date_range_updated: (q.updated != (0, 0))
                    .then_some(c_published_file_query_files_request::DateRange { timestamp_start: Some(q.updated.0), timestamp_end: Some(q.updated.1) }),
                totalonly: Some(q.total_only),
                ids_only: Some(q.only_ids),
                return_vote_data: Some(true),
                return_tags: Some(true),
                return_kv_tags: Some(q.kv_tags),
                return_previews: Some(q.previews),
                return_children: Some(q.children),
                return_short_description: Some(!q.long_description),
                return_metadata: Some(q.metadata),
                return_playtime_stats: Some(q.playtime_days).filter(|d| *d > 0),
                return_details: Some(true),
                ..Default::default()
            };
            match conn.call::<_, CPublishedFileQueryFilesResponse>("PublishedFile.QueryFiles#1", &req).await {
                Ok(r) => ok(r.total(), &r.publishedfiledetails, r.next_cursor().to_string()),
                Err(e) => fail(e),
            }
        }
    }
}

/// Every item the player subscribed to for this game.
pub async fn subscribed(conn: &Connection, app_id: AppId, me: u64) -> Vec<u64> {
    crate::workshop::subscribed(conn, me, app_id).await.unwrap_or_else(|e| {
        tracing::info!("workshop subscriptions: {e}");
        Vec::new()
    })
}

pub async fn subscribe(conn: &Connection, app_id: AppId, id: u64, list: u32, on: bool) -> i32 {
    let r = if on {
        conn.call::<_, CPublishedFileSubscribeResponse>(
            "PublishedFile.Subscribe#1",
            &CPublishedFileSubscribeRequest { publishedfileid: Some(id), list_type: Some(list), appid: Some(app_id as i32), notify_client: Some(true), include_dependencies: Some(false) },
        )
        .await
        .map(|_| ())
    } else {
        conn.call::<_, CPublishedFileUnsubscribeResponse>(
            "PublishedFile.Unsubscribe#1",
            &CPublishedFileUnsubscribeRequest { publishedfileid: Some(id), list_type: Some(list), appid: Some(app_id as i32), notify_client: Some(true) },
        )
        .await
        .map(|_| ())
    };
    r.map(|_| eresult::OK).unwrap_or_else(|e| code(&e))
}

pub async fn vote(conn: &Connection, id: u64, up: bool) -> i32 {
    conn.call::<_, CPublishedFileVoteResponse>("PublishedFile.Vote#1", &CPublishedFileVoteRequest { publishedfileid: Some(id), vote_up: Some(up) })
        .await
        .map(|_| eresult::OK)
        .unwrap_or_else(|e| code(&e))
}

pub async fn get_vote(conn: &Connection, id: u64) -> Reply {
    match conn.call::<_, CPublishedFileGetUserVoteSummaryResponse>("PublishedFile.GetUserVoteSummary#1", &CPublishedFileGetUserVoteSummaryRequest { publishedfileids: vec![id] }).await {
        Ok(r) => {
            let s = r.summaries.iter().find(|s| s.publishedfileid() == id);
            Reply::UgcVote { eresult: eresult::OK, up: s.is_some_and(|s| s.vote_for()), down: s.is_some_and(|s| s.vote_against()), skipped: s.is_some_and(|s| s.vote_later()) }
        }
        Err(e) => Reply::UgcVote { eresult: code(&e), up: false, down: false, skipped: false },
    }
}

const MAX_DIR_DEPTH: usize = 32;

fn dir_size(p: &Path) -> u64 {
    fn walk(p: &Path, depth: usize) -> u64 {
        if depth > MAX_DIR_DEPTH {
            return 0;
        }
        let Ok(meta) = std::fs::symlink_metadata(p) else { return 0 };
        if is_symlink_or_reparse(&meta) || !meta.is_dir() {
            return 0;
        }
        let Ok(rd) = std::fs::read_dir(p) else { return 0 };
        let mut total = 0u64;
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                total = total.saturating_add(walk(&e.path(), depth + 1));
            } else if ft.is_file() {
                let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
                if !is_symlink_or_reparse(&m) && m.is_file() {
                    total = total.saturating_add(m.len());
                }
            }
        }
        total
    }
    walk(p, 0)
}

/// Download (or update) one item for the game, through the same path (and
/// `appworkshop_<app>.acf` record) as the library's Workshop downloads.
pub async fn download(conn: &Connection, app_id: AppId, me: u64, id: u64, install_dir: &Path, cell_id: u32) -> Reply {
    let failed = |why: String| {
        tracing::info!("workshop item {id}: {why}");
        Reply::UgcDownloaded { eresult: eresult::FAIL, folder: String::new(), size: 0, updated: 0 }
    };
    let Some(library) = crate::workshop::library_of(install_dir) else { return failed("the game isn't in a Steam library folder".into()) };
    match crate::workshop::sync(conn, me, app_id, &library, cell_id, vec![id], crate::workshop::SyncOptions::default()).await {
        Ok(r) if r.failed.is_empty() => {
            let mut dir = library.clone();
            for seg in ["steamapps", "workshop", "content", &app_id.to_string(), &id.to_string()] {
                dir.push(seg);
                let Ok(m) = std::fs::symlink_metadata(&dir) else {
                    return failed("not available".into());
                };
                if is_symlink_or_reparse(&m) || !m.is_dir() {
                    return failed("refusing symlinked workshop folder".into());
                }
            }
            let updated = std::fs::symlink_metadata(&dir).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as u32).unwrap_or(0);
            Reply::UgcDownloaded { eresult: eresult::OK, folder: dir.to_string_lossy().into_owned(), size: dir_size(&dir), updated }
        }
        Ok(r) => failed(r.failed.iter().map(|f| f.2.clone()).collect::<Vec<_>>().join("; ")),
        Err(e) => failed(e),
    }
}

/// Maximum size of a Workshop preview image (10 MiB) or single-file content upload (100 MiB).
const MAX_UGC_PREVIEW_BYTES: u64 = 10 * 1024 * 1024;
const MAX_UGC_CONTENT_BYTES: u64 = 100 * 1024 * 1024;

fn is_symlink_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if (meta.file_attributes() & 0x0400) != 0 {
            return true;
        }
    }
    false
}

/// Verify no existing path component (ancestors or leaf) is a symlink/reparse point.
#[cfg(not(unix))]
fn reject_symlink_ancestors(path: &Path) -> Result<(), String> {
    let mut cur = std::path::PathBuf::new();
    for comp in path.components() {
        cur.push(comp);
        if matches!(comp, std::path::Component::RootDir | std::path::Component::Prefix(_)) {
            continue;
        }
        let meta = std::fs::symlink_metadata(&cur).map_err(|e| format!("cannot stat {}: {e}", cur.display()))?;
        if is_symlink_or_reparse(&meta) {
            return Err(format!("refusing symlink/reparse point in path: {}", cur.display()));
        }
    }
    Ok(())
}

/// Open `path` on Unix by walking every component with `openat(..., O_NOFOLLOW)`,
/// preventing ancestor-swap symlink races, and passing `O_NONBLOCK` on the leaf
/// so opening a FIFO never blocks the upload worker.
#[cfg(unix)]
fn open_no_symlinks_unix(path: &Path) -> Result<std::fs::File, String> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    let mut comps: Vec<Component<'_>> = Vec::new();
    let mut is_abs = false;
    for c in path.components() {
        match c {
            Component::RootDir => is_abs = true,
            Component::CurDir => {}
            Component::ParentDir | Component::Normal(_) => comps.push(c),
            Component::Prefix(_) => return Err("unsupported path prefix".into()),
        }
    }
    if comps.is_empty() {
        return Err("empty upload path".into());
    }
    let start_c = if is_abs { c"/" } else { c"." };
    let fd = unsafe { libc::open(start_c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(format!("cannot open root for {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    let mut dirfd = unsafe { OwnedFd::from_raw_fd(fd) };

    for (idx, comp) in comps.iter().enumerate() {
        let is_last = idx + 1 == comps.len();
        let cseg = match comp {
            Component::ParentDir => CString::new("..").unwrap(),
            Component::Normal(s) => CString::new(s.as_bytes()).map_err(|_| "invalid NUL in upload path".to_string())?,
            _ => continue,
        };
        let flags = if is_last {
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
        } else {
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
        };
        let next = unsafe { libc::openat(dirfd.as_raw_fd(), cseg.as_ptr(), flags) };
        if next < 0 {
            let err = std::io::Error::last_os_error();
            return Err(format!("cannot open {} without following symlinks: {err}", path.display()));
        }
        let owned = unsafe { OwnedFd::from_raw_fd(next) };
        if is_last {
            return Ok(std::fs::File::from(owned));
        }
        dirfd = owned;
    }
    Err("empty upload path".into())
}

fn read_bounded_regular_file(file: std::fs::File, display: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let meta = file.metadata().map_err(|e| format!("cannot stat {}: {e}", display.display()))?;
    if !meta.is_file() || is_symlink_or_reparse(&meta) {
        return Err(format!("{} is not a regular file", display.display()));
    }
    if meta.len() > max_bytes {
        return Err(format!("file exceeds {} byte upload limit", max_bytes));
    }
    let mut data = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("cannot read {}: {e}", display.display()))?;
    if data.len() as u64 > max_bytes {
        return Err(format!("file exceeds {} byte upload limit", max_bytes));
    }
    Ok(data)
}

fn valid_upload_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && !n.contains('/') && !n.contains('\\') && *n != "." && *n != "..")
        .map(str::to_string)
        .ok_or_else(|| "invalid file name".to_string())
}

/// Read a local file for Workshop upload without following symlinks in any
/// ancestor or leaf, never blocking on FIFOs, and enforcing `max_bytes`
/// atomically via a bounded `.take()` read on the opened descriptor.
fn read_upload_file(path: &Path, max_bytes: u64) -> Result<(String, Vec<u8>), String> {
    let name = valid_upload_name(path)?;
    #[cfg(unix)]
    let file = open_no_symlinks_unix(path)?;
    #[cfg(not(unix))]
    let file = {
        reject_symlink_ancestors(path)?;
        let pre = std::fs::symlink_metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
        if is_symlink_or_reparse(&pre) || !pre.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let f = opts.open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        reject_symlink_ancestors(path)?;
        f
    };
    let data = read_bounded_regular_file(file, path, max_bytes)?;
    Ok((name, data))
}

/// Resolve a Workshop `SetItemContent` path: accepts either a single regular file
/// or a directory containing a single regular file (legacy cloud-backed UGC).
fn resolve_content_file(path: &Path) -> Result<(String, Vec<u8>), String> {
    #[cfg(unix)]
    {
        use std::ffi::{CStr, CString};
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        let file = open_no_symlinks_unix(path)?;
        let meta = file.metadata().map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
        if meta.is_file() {
            let name = valid_upload_name(path)?;
            let data = read_bounded_regular_file(file, path, MAX_UGC_CONTENT_BYTES)?;
            return Ok((name, data));
        }
        if !meta.is_dir() {
            return Err("unsupported Workshop content path".into());
        }
        // Enumerate entries via fdopendir(dup(dirfd)) and open the single child via openat(dirfd, ...).
        let dup_fd = unsafe { libc::dup(file.as_raw_fd()) };
        if dup_fd < 0 {
            return Err(format!("cannot read {}: {}", path.display(), std::io::Error::last_os_error()));
        }
        let dirp = unsafe { libc::fdopendir(dup_fd) };
        if dirp.is_null() {
            unsafe { libc::close(dup_fd) };
            return Err(format!("cannot read {}: {}", path.display(), std::io::Error::last_os_error()));
        }
        let mut names: Vec<CString> = Vec::new();
        let mut err_msg: Option<String> = None;
        loop {
            let ent = unsafe { libc::readdir(dirp) };
            if ent.is_null() {
                break;
            }
            let cname = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
            if cname.to_bytes() == b"." || cname.to_bytes() == b".." {
                continue;
            }
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::fstatat(file.as_raw_fd(), cname.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
            if rc != 0 {
                err_msg = Some(format!("cannot stat entry in {}: {}", path.display(), std::io::Error::last_os_error()));
                break;
            }
            let mode = st.st_mode & libc::S_IFMT;
            if mode == libc::S_IFLNK {
                err_msg = Some("refusing to upload symlink inside content folder".into());
                break;
            }
            if mode == libc::S_IFDIR {
                err_msg = Some("multi-directory Workshop depot uploads are not supported yet".into());
                break;
            }
            if mode == libc::S_IFREG {
                names.push(cname.to_owned());
            } else {
                err_msg = Some("refusing non-regular file inside content folder".into());
                break;
            }
        }
        unsafe { libc::closedir(dirp) };
        if let Some(e) = err_msg {
            return Err(e);
        }
        if names.len() != 1 {
            return Err(format!("multi-file Workshop depot uploads ({} files) are not supported yet", names.len()));
        }
        let cname = &names[0];
        let name = cname
            .to_str()
            .ok()
            .filter(|n| !n.is_empty() && !n.contains('/') && !n.contains('\\') && *n != "." && *n != "..")
            .ok_or_else(|| "invalid file name".to_string())?
            .to_string();
        let child_fd = unsafe {
            libc::openat(
                file.as_raw_fd(),
                cname.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if child_fd < 0 {
            return Err(format!("cannot open {}: {}", path.join(&name).display(), std::io::Error::last_os_error()));
        }
        let child = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(child_fd) });
        let data = read_bounded_regular_file(child, &path.join(&name), MAX_UGC_CONTENT_BYTES)?;
        Ok((name, data))
    }
    #[cfg(not(unix))]
    {
        reject_symlink_ancestors(path)?;
        let meta = std::fs::symlink_metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
        if is_symlink_or_reparse(&meta) {
            return Err("refusing to upload symlinked content path".into());
        }
        if meta.is_file() {
            return read_upload_file(path, MAX_UGC_CONTENT_BYTES);
        }
        if meta.is_dir() {
            let mut files = Vec::new();
            for entry in std::fs::read_dir(path).map_err(|e| format!("cannot read {}: {e}", path.display()))? {
                let entry = entry.map_err(|e| e.to_string())?;
                let m = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
                if is_symlink_or_reparse(&m) {
                    return Err("refusing to upload symlink inside content folder".into());
                }
                if m.is_file() {
                    files.push(entry.path());
                } else if m.is_dir() {
                    return Err("multi-directory Workshop depot uploads are not supported yet".into());
                }
            }
            if files.len() == 1 {
                return read_upload_file(&files[0], MAX_UGC_CONTENT_BYTES);
            }
            return Err(format!("multi-file Workshop depot uploads ({} files) are not supported yet", files.len()));
        }
        Err("unsupported Workshop content path".into())
    }
}

/// Create a new Workshop item (`ISteamUGC::CreateItem`) via `PublishedFile.Publish#1`.
pub async fn create_item(conn: &Connection, app_id: AppId, consumer_app: AppId, file_type: i32) -> Reply {
    let consumer = if consumer_app == 0 { app_id } else { consumer_app };
    let req = CPublishedFilePublishRequest {
        appid: Some(app_id),
        consumer_appid: Some(consumer),
        file_type: Some(file_type.max(0) as u32),
        // Start private (2) until the game submits title/content/visibility.
        visibility: Some(2),
        ..Default::default()
    };
    match conn.call::<_, CPublishedFilePublishResponse>("PublishedFile.Publish#1", &req).await {
        Ok(r) => Reply::UgcCreated {
            eresult: eresult::OK,
            id: r.publishedfileid(),
            needs_agreement: !r.redirect_uri().is_empty(),
        },
        Err(e) => {
            let c = code(&e);
            tracing::info!("workshop create_item failed: {e}");
            // EResult 24 = InsufficientPrivilege (often legal agreement not accepted).
            Reply::UgcCreated { eresult: c, id: 0, needs_agreement: c == 24 }
        }
    }
}

/// Submit staged updates to a Workshop item (`ISteamUGC::SubmitItemUpdate`) via
/// Steam Cloud shared file upload + `PublishedFile.Update#1`.
pub async fn update_item(conn: &Connection, app_id: AppId, cell_id: u32, upd: &UgcUpdate) -> Reply {
    let target_app = if upd.consumer_app == 0 { app_id } else { upd.consumer_app };
    let mut preview_filename: Option<String> = None;
    if let Some(p) = &upd.preview_path
        && !p.is_empty()
    {
        match read_upload_file(Path::new(p), MAX_UGC_PREVIEW_BYTES) {
            Ok((name, data)) => {
                let cloud_name = format!("ugc_preview_{}_{name}", upd.id);
                if let Err(e) = crate::cloud::upload_shared_file(conn, target_app, cell_id, &cloud_name, &data).await {
                    tracing::info!("workshop preview upload failed for {}: {e}", upd.id);
                    return Reply::UgcUpdated { eresult: eresult::FAIL, id: upd.id, needs_agreement: false };
                }
                preview_filename = Some(cloud_name);
            }
            Err(e) => {
                tracing::info!("workshop preview file rejected for {}: {e}", upd.id);
                return Reply::UgcUpdated { eresult: eresult::FAIL, id: upd.id, needs_agreement: false };
            }
        }
    }
    let mut content_filename: Option<String> = None;
    if let Some(p) = &upd.content_path
        && !p.is_empty()
    {
        match resolve_content_file(Path::new(p)) {
            Ok((name, data)) => {
                let cloud_name = format!("ugc_content_{}_{name}", upd.id);
                if let Err(e) = crate::cloud::upload_shared_file(conn, target_app, cell_id, &cloud_name, &data).await {
                    tracing::info!("workshop content upload failed for {}: {e}", upd.id);
                    return Reply::UgcUpdated { eresult: eresult::FAIL, id: upd.id, needs_agreement: false };
                }
                content_filename = Some(cloud_name);
            }
            Err(e) => {
                tracing::info!("workshop content rejected for {}: {e}", upd.id);
                return Reply::UgcUpdated { eresult: eresult::FAIL, id: upd.id, needs_agreement: false };
            }
        }
    }
    let req = CPublishedFileUpdateRequest {
        appid: Some(target_app),
        publishedfileid: Some(upd.id),
        title: upd.title.clone(),
        file_description: upd.description.clone(),
        visibility: upd.visibility.map(|v| v.clamp(0, 3) as u32),
        tags: upd.tags.clone().unwrap_or_default(),
        update_tags: upd.tags.as_ref().map(|_| true),
        filename: content_filename,
        preview_filename,
        language: upd.language.as_deref().map(crate::hub::language_id),
        ..Default::default()
    };
    match conn.call::<_, CPublishedFileUpdateResponse>("PublishedFile.Update#1", &req).await {
        Ok(_) => Reply::UgcUpdated { eresult: eresult::OK, id: upd.id, needs_agreement: false },
        Err(e) => {
            let c = code(&e);
            tracing::info!("workshop update_item {} failed: {e}", upd.id);
            Reply::UgcUpdated { eresult: c, id: upd.id, needs_agreement: c == 24 }
        }
    }
}

/// Delete a Workshop item (`ISteamUGC::DeleteItem`) via `PublishedFile.Delete#1`.
pub async fn delete_item(conn: &Connection, app_id: AppId, id: u64) -> i32 {
    let req = CPublishedFileDeleteRequest { publishedfileid: Some(id), appid: Some(app_id) };
    conn.call::<_, CPublishedFileDeleteResponse>("PublishedFile.Delete#1", &req)
        .await
        .map(|_| eresult::OK)
        .unwrap_or_else(|e| code(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_steamworks_enums() {
        assert_eq!(file_type(0), 0);
        assert_eq!(file_type(3), 1);
        assert_eq!(file_type(-1), 0);
        assert_eq!(user_list(6), "mysubscriptions");
        assert_eq!(steam_id_of(1), 76561197960265729);
        let d = PublishedFileDetails { publishedfileid: Some(5), title: Some("Map".into()), short_description: Some("short".into()), ..Default::default() };
        let i = item(&d);
        assert_eq!((i.id, i.title.as_str(), i.description.as_str()), (5, "Map", "short"));
    }

    #[test]
    fn validates_upload_files_and_folders() {
        let dir = std::env::temp_dir().join(format!("aurora-test-ugc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mod.pak");
        std::fs::write(&file, b"hello workshop").unwrap();
        let (name, data) = read_upload_file(&file, 1024).unwrap();
        assert_eq!(name, "mod.pak");
        assert_eq!(data, b"hello workshop");
        // Rejects oversized file.
        assert!(read_upload_file(&file, 4).is_err());
        // Single-file folder works.
        let (fname, fdata) = resolve_content_file(&dir).unwrap();
        assert_eq!(fname, "mod.pak");
        assert_eq!(fdata, b"hello workshop");
        // Multi-file folder fails cleanly.
        std::fs::write(dir.join("extra.bin"), b"x").unwrap();
        assert!(resolve_content_file(&dir).is_err());

        // Symlinks (leaf and ancestor) are rejected for uploads and skipped by dir_size.
        #[cfg(unix)]
        {
            let link = dir.join("link.pak");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(read_upload_file(&link, 1024).is_err());
            let dir_link = std::env::temp_dir().join(format!("aurora-test-ugc-link-{}", std::process::id()));
            let _ = std::fs::remove_file(&dir_link);
            std::os::unix::fs::symlink(&dir, &dir_link).unwrap();
            assert!(read_upload_file(&dir_link.join("mod.pak"), 1024).is_err());
            assert_eq!(dir_size(&dir_link), 0);
            let _ = std::fs::remove_file(&dir_link);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
