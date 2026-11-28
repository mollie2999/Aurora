//! The engine loop: idle -> connect -> authenticate -> logon -> online -> idle.

use crate::{Command, Error, Event, Link, Sink, launch, offline, pics, store};
use auth::{Device, Guard, PendingAuth};
use model::{AppId, OwnedApp, Persona, PersonaState, Relationship, SteamId};
use net::{Connection, Packet, emsg, eresult};
use proto::*;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;

const PROTOCOL_VERSION: u32 = 65580;
const OS_TYPE_WIN11: u32 = 20;
/// Status | PlayerName | Presence | LastSeen | GameExtraInfo | RichPresence
const PERSONA_FLAGS: u32 = 1 | 2 | 16 | 64 | 256 | 4096;

type Cmds = mpsc::UnboundedReceiver<Command>;

enum Login {
    Password { account: String, password: String },
    Qr,
    Saved(store::SavedAccount),
    /// Offline mode from the cached sign-in.
    Offline(store::SavedAccount),
}

#[derive(Default)]
struct StorageJobsInner {
    moving: HashMap<AppId, std::sync::Arc<local::manage::Progress>>,
    backing_up: HashMap<AppId, std::sync::Arc<local::manage::Progress>>,
    restoring: Option<(AppId, std::sync::Arc<local::manage::Progress>)>,
}

type SharedStorageJobs = std::sync::Arc<std::sync::Mutex<StorageJobsInner>>;

fn storage_busy(jobs: &SharedStorageJobs, app_id: AppId) -> bool {
    let g = jobs.lock().unwrap();
    g.moving.contains_key(&app_id)
        || g.backing_up.contains_key(&app_id)
        || g.restoring.as_ref().is_some_and(|(id, _)| *id == app_id)
}

fn spawn_storage_ticker(
    progress: std::sync::Arc<local::manage::Progress>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mut on_tick: impl FnMut(u64, u64) + Send + 'static,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if done.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let d = progress.done_bytes.load(std::sync::atomic::Ordering::Relaxed);
            let t = progress.total_bytes.load(std::sync::atomic::Ordering::Relaxed);
            on_tick(d, t);
        }
    });
}

fn handle_storage_command(
    cmd: Command,
    jobs: &SharedStorageJobs,
    is_running: impl FnOnce(AppId) -> bool,
    is_downloading: impl FnOnce(AppId) -> bool,
    is_verifying: impl FnOnce(AppId) -> bool,
    sink: &Sink,
) -> Option<Command> {
    match cmd {
        Command::MoveGame { game, dest_library } => {
            let app_id = game.app_id;
            if is_running(app_id) {
                sink(Event::MoveFinished { app_id, result: Err("Close the game before moving it.".into()) });
                return None;
            }
            if is_downloading(app_id) {
                sink(Event::MoveFinished { app_id, result: Err("Wait for the download to finish or cancel it before moving this game.".into()) });
                return None;
            }
            if is_verifying(app_id) || storage_busy(jobs, app_id) {
                sink(Event::MoveFinished { app_id, result: Err("Another file operation is already in progress for this game.".into()) });
                return None;
            }
            let progress = std::sync::Arc::new(local::manage::Progress::default());
            progress.total_bytes.store(game.size_on_disk.max(1), std::sync::atomic::Ordering::Relaxed);
            jobs.lock().unwrap().moving.insert(app_id, progress.clone());
            sink(Event::MoveProgress { app_id, done_bytes: 0, total_bytes: game.size_on_disk.max(1) });
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (p_tick, d_tick, s_tick) = (progress.clone(), done.clone(), sink.clone());
            spawn_storage_ticker(p_tick, d_tick, move |done_bytes, total_bytes| {
                s_tick(Event::MoveProgress { app_id, done_bytes, total_bytes });
            });
            let (jobs2, sink2) = (jobs.clone(), sink.clone());
            tokio::task::spawn_blocking(move || {
                let result = local::manage::move_game(&game, &dest_library, &progress);
                done.store(true, std::sync::atomic::Ordering::Relaxed);
                jobs2.lock().unwrap().moving.remove(&app_id);
                sink2(Event::MoveFinished { app_id, result });
            });
            None
        }
        Command::CancelMove { app_id } => {
            if let Some(p) = jobs.lock().unwrap().moving.get(&app_id) {
                p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            None
        }
        Command::BackupGame { game, dest_dir } => {
            let app_id = game.app_id;
            if is_running(app_id) {
                sink(Event::BackupFinished { app_id, result: Err("Close the game before backing it up.".into()) });
                return None;
            }
            if is_downloading(app_id) {
                sink(Event::BackupFinished { app_id, result: Err("Wait for the download to finish before backing up this game.".into()) });
                return None;
            }
            if is_verifying(app_id) || storage_busy(jobs, app_id) {
                sink(Event::BackupFinished { app_id, result: Err("Another file operation is already in progress for this game.".into()) });
                return None;
            }
            let progress = std::sync::Arc::new(local::manage::Progress::default());
            progress.total_bytes.store(game.size_on_disk.max(1), std::sync::atomic::Ordering::Relaxed);
            jobs.lock().unwrap().backing_up.insert(app_id, progress.clone());
            sink(Event::BackupProgress { app_id, done_bytes: 0, total_bytes: game.size_on_disk.max(1) });
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (p_tick, d_tick, s_tick) = (progress.clone(), done.clone(), sink.clone());
            spawn_storage_ticker(p_tick, d_tick, move |done_bytes, total_bytes| {
                s_tick(Event::BackupProgress { app_id, done_bytes, total_bytes });
            });
            let (jobs2, sink2) = (jobs.clone(), sink.clone());
            tokio::task::spawn_blocking(move || {
                let result = local::manage::backup_game(&game, &dest_dir, &progress);
                done.store(true, std::sync::atomic::Ordering::Relaxed);
                jobs2.lock().unwrap().backing_up.remove(&app_id);
                sink2(Event::BackupFinished { app_id, result });
            });
            None
        }
        Command::CancelBackup { app_id } => {
            if let Some(p) = jobs.lock().unwrap().backing_up.get(&app_id) {
                p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            None
        }
        Command::RestoreGame { backup_dir, dest_library } => {
            if jobs.lock().unwrap().restoring.is_some() {
                sink(Event::RestoreFinished { result: Err("Another backup restore is already in progress.".into()) });
                return None;
            }
            let info = match local::manage::inspect_backup(&backup_dir) {
                Ok(i) => i,
                Err(e) => {
                    sink(Event::RestoreFinished { result: Err(e) });
                    return None;
                }
            };
            let app_id = info.app_id;
            if is_running(app_id) {
                sink(Event::RestoreFinished { result: Err("Close the game before restoring a backup for it.".into()) });
                return None;
            }
            if is_downloading(app_id) {
                sink(Event::RestoreFinished { result: Err("Wait for the download to finish before restoring a backup for this game.".into()) });
                return None;
            }
            if is_verifying(app_id) || storage_busy(jobs, app_id) {
                sink(Event::RestoreFinished { result: Err("Another file operation is already in progress for this game.".into()) });
                return None;
            }
            let progress = std::sync::Arc::new(local::manage::Progress::default());
            progress.total_bytes.store(info.total_bytes.max(1), std::sync::atomic::Ordering::Relaxed);
            jobs.lock().unwrap().restoring = Some((app_id, progress.clone()));
            let name = info.name.clone();
            sink(Event::RestoreProgress { app_id, name: name.clone(), done_bytes: 0, total_bytes: info.total_bytes.max(1) });
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (p_tick, d_tick, s_tick, n_tick) = (progress.clone(), done.clone(), sink.clone(), name);
            spawn_storage_ticker(p_tick, d_tick, move |done_bytes, total_bytes| {
                s_tick(Event::RestoreProgress { app_id, name: n_tick.clone(), done_bytes, total_bytes });
            });
            let (jobs2, sink2) = (jobs.clone(), sink.clone());
            tokio::task::spawn_blocking(move || {
                let result = local::manage::restore_game(&backup_dir, &dest_library, &progress);
                done.store(true, std::sync::atomic::Ordering::Relaxed);
                jobs2.lock().unwrap().restoring = None;
                sink2(Event::RestoreFinished { result });
            });
            None
        }
        Command::CancelRestore => {
            if let Some((_, p)) = &jobs.lock().unwrap().restoring {
                p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            None
        }
        other => Some(other),
    }
}

pub async fn run(mut cmds: Cmds, sink: Sink) {
    // A login command that arrived while another login was waiting.
    let mut next: Option<Command> = None;
    let storage: SharedStorageJobs = Default::default();
    loop {
        let cmd = match next.take() {
            Some(c) => c,
            None => match cmds.recv().await {
                Some(c) => c,
                None => return,
            },
        };
        let Some(cmd) = handle_storage_command(cmd, &storage, |_| false, |_| false, |_| false, &sink) else {
            continue;
        };
        let login = match cmd {
            Command::LoginPassword { account, password } => Login::Password { account, password },
            Command::LoginQr => Login::Qr,
            Command::LoginSaved { account } => match store::credentials(&account) {
                // No token in the credential store (none on this system, or
                // it was cleared): the password is needed again.
                Some(saved) if saved.refresh_token.is_empty() => {
                    sink(Event::LoginFailed(format!("The saved sign-in for {account} isn't available. Sign in with your password.")));
                    continue;
                }
                Some(saved) => Login::Saved(saved),
                None => {
                    sink(Event::LoginFailed(format!("No saved login for {account}")));
                    continue;
                }
            },
            Command::StartOffline { account } => match store::credentials(&account) {
                Some(saved) => Login::Offline(saved),
                None => {
                    sink(Event::LoginFailed(format!("No saved login for {account}")));
                    continue;
                }
            },
            Command::ForgetAccount { account } => {
                store::remove(&account);
                continue;
            }
            _ => continue,
        };
        // A saved login that can't reach Steam isn't a failed login: offer
        // offline mode instead, like Steam's "Connection error" window.
        let saved = match &login {
            Login::Saved(a) => Some(a.clone()),
            _ => None,
        };
        sink(Event::Connecting);
        match session(login, &mut cmds, &sink, &mut next, &storage).await {
            Ok(reason) => sink(Event::LoggedOut { reason }),
            Err(_) if next.is_some() => {} // replaced by a newer login
            Err(e) if saved.is_some() && is_network(&e) => {
                let Some(a) = saved else { continue };
                tracing::warn!("can't reach Steam for {}: {e}", a.account_name);
                let offline_ready = offline::load_account(a.steam_id).is_some();
                sink(Event::ConnectionError { account: a.account_name, message: e.to_string(), offline_ready });
            }
            Err(e) => sink(Event::LoginFailed(e.to_string())),
        }
    }
}

async fn connect_any() -> Result<(Connection, net::connection::Events), Error> {
    let servers = net::directory::websocket_servers().await?;
    let mut last = None;
    for server in servers.iter().take(4) {
        match tokio::time::timeout(Duration::from_secs(10), Connection::connect(server)).await {
            Ok(Ok(c)) => return Ok(c),
            Ok(Err(e)) => {
                tracing::warn!("{server}: {e}");
                last = Some(e);
            }
            Err(_) => {
                tracing::warn!("{server}: no answer");
                last = Some(net::Error::Timeout);
            }
        }
    }
    Err(last.unwrap_or(net::Error::NoServers).into())
}

/// Returns `Ok(reason)` once a logged-in session ends, `Err` if login failed.
/// The network failed (no connection, timeout, Steam down), as opposed to
/// Steam saying no to the login.
fn is_network(e: &Error) -> bool {
    match e {
        Error::Net(net::Error::EResult(code, _)) | Error::Auth(auth::Error::Net(net::Error::EResult(code, _))) => {
            // NoConnection, Timeout, ServiceUnavailable, TryAnotherCM.
            matches!(*code, 3 | 16 | eresult::SERVICE_UNAVAILABLE | eresult::TRY_ANOTHER_CM)
        }
        Error::Net(net::Error::Decode(_) | net::Error::Truncated) => false,
        Error::Net(_) | Error::Auth(auth::Error::Net(_)) => true,
        _ => false,
    }
}

/// Worth trying again later (while reconnecting): network trouble, or
/// Steam being busy. Anything else means the saved login is no good.
fn is_transient(e: &Error) -> bool {
    is_network(e) || matches!(e, Error::Net(net::Error::EResult(code, _)) if matches!(*code, eresult::FAIL | eresult::RATE_LIMIT_EXCEEDED | eresult::LOGIN_THROTTLED))
}

/// What a successful `ClientLogon` told us.
#[derive(Clone, Copy)]
struct Logon {
    steam_id: SteamId,
    heartbeat: i32,
    cell_id: u32,
    public_ip: u32,
}

/// `ClientLogon` with the saved refresh token, on a fresh connection.
async fn logon(conn: &Connection, events: &mut net::connection::Events, account: &store::SavedAccount) -> Result<Logon, Error> {
    let device = Device::default();
    conn.set_session(account.steam_id, 0);
    conn.send(
        emsg::CLIENT_LOGON,
        &CMsgClientLogon {
            protocol_version: Some(PROTOCOL_VERSION),
            client_os_type: Some(OS_TYPE_WIN11),
            client_language: Some("english".into()),
            should_remember_password: Some(true),
            account_name: Some(account.account_name.clone()),
            access_token: Some(account.refresh_token.clone()),
            machine_name: Some(device.name.clone()),
            chat_mode: Some(2),
            // Steam tells sessions on one account apart by this id: a second
            // sign-in with the same id replaces the first (error 34). Each run
            // of the app gets its own, like separate PCs; reconnecting keeps it,
            // so the new session replaces only our own old one.
            obfuscated_private_ip: Some(CMsgIpAddress {
                ip: Some(c_msg_ip_address::Ip::V4(logon_id() ^ 0xBAAD_F00D)),
            }),
            ..Default::default()
        },
    )?;
    let reply = wait_for(events, emsg::CLIENT_LOGON_RESPONSE).await?;
    let resp: CMsgClientLogonResponse = reply.body()?;
    if resp.eresult() != eresult::OK {
        if matches!(resp.eresult(), eresult::ACCESS_DENIED | eresult::INVALID_PASSWORD | eresult::EXPIRED) {
            store::remove(&account.account_name);
        }
        return Err(net::Error::EResult(resp.eresult(), String::new()).into());
    }
    let steam_id = reply.header.steamid.filter(|&s| s != 0).unwrap_or(account.steam_id);
    conn.set_session(steam_id, reply.header.client_sessionid());
    let public_ip = match resp.public_ip.as_ref().and_then(|ip| ip.ip.as_ref()) {
        Some(c_msg_ip_address::Ip::V4(v)) => *v,
        _ => 0,
    };
    Ok(Logon { steam_id, heartbeat: resp.heartbeat_seconds(), cell_id: resp.cell_id(), public_ip })
}

/// A new connection and sign-in for a session that lost its connection.
async fn reconnect(account: store::SavedAccount) -> Result<(Connection, net::connection::Events, Logon), Error> {
    let (conn, mut events) = connect_any().await?;
    conn.send(emsg::CLIENT_HELLO, &CMsgClientHello { protocol_version: Some(PROTOCOL_VERSION) })?;
    let info = logon(&conn, &mut events, &account).await?;
    Ok((conn, events, info))
}

async fn session(login: Login, cmds: &mut Cmds, sink: &Sink, next: &mut Option<Command>, storage: &SharedStorageJobs) -> Result<String, Error> {
    if let Login::Offline(account) = &login {
        if offline::load_account(account.steam_id).is_none() {
            return Err(Error::Other("This account hasn't signed in online on this PC yet, so it can't start in offline mode.".into()));
        }
        return online(None, cmds, sink, account, storage).await;
    }
    let (conn, mut events) = connect_any().await?;
    conn.send(emsg::CLIENT_HELLO, &CMsgClientHello { protocol_version: Some(PROTOCOL_VERSION) })?;
    let device = Device::default();

    let account = match login {
        Login::Saved(saved) | Login::Offline(saved) => saved,
        Login::Qr => {
            let mut pending = auth::begin_qr(&conn, &device).await?;
            let Some(tokens) = guard_flow(&conn, &mut pending, cmds, sink, next).await? else {
                return Err(Error::Other("Login cancelled".into()));
            };
            store::SavedAccount {
                account_name: tokens.account_name,
                steam_id: tokens.steam_id,
                persona_name: String::new(),
                avatar_hash: None,
                refresh_token: tokens.refresh_token,
                guard_data: tokens.guard_data,
            }
        }
        Login::Password { account, password } => {
            let guard_data = store::credentials(&account).and_then(|a| a.guard_data);
            let mut pending = auth::begin(&conn, &account, &password, guard_data, &device).await?;
            drop(password);
            let Some(tokens) = guard_flow(&conn, &mut pending, cmds, sink, next).await? else {
                return Err(Error::Other("Login cancelled".into()));
            };
            store::SavedAccount {
                account_name: if tokens.account_name.is_empty() { account } else { tokens.account_name },
                steam_id: tokens.steam_id,
                persona_name: String::new(),
                avatar_hash: None,
                refresh_token: tokens.refresh_token,
                guard_data: tokens.guard_data,
            }
        }
    };

    let info = logon(&conn, &mut events, &account).await?;
    store::upsert(account.clone());
    sink(Event::LoggedIn { steam_id: info.steam_id, account: account.account_name.clone() });
    online(Some((conn, events, info)), cmds, sink, &account, storage).await
}

async fn guard_flow(
    conn: &Connection,
    pending: &mut PendingAuth,
    cmds: &mut Cmds,
    sink: &Sink,
    next: &mut Option<Command>,
) -> Result<Option<auth::Tokens>, Error> {
    let mut shown_url = pending.challenge_url.clone();
    if let Some(url) = &shown_url {
        sink(Event::QrChallenge { url: url.clone() });
    } else if pending.preferred() != Guard::None {
        sink(Event::GuardRequired { guards: pending.guards.clone() });
    }
    let mut tick = tokio::time::interval(pending.interval);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some(t) = pending.poll(conn).await? {
                    return Ok(Some(t));
                }
                if pending.challenge_url != shown_url {
                    shown_url = pending.challenge_url.clone();
                    if let Some(url) = &shown_url {
                        sink(Event::QrChallenge { url: url.clone() });
                    }
                }
            }
            cmd = cmds.recv() => match cmd {
                Some(Command::SubmitCode(code)) => match pending.submit_code(conn, &code).await {
                    Ok(()) => {
                        if let Some(t) = pending.poll(conn).await? {
                            return Ok(Some(t));
                        }
                    }
                    Err(e) => sink(Event::GuardCodeRejected(e.to_string())),
                },
                Some(Command::CancelLogin) | None => return Ok(None),
                Some(c @ (Command::LoginPassword { .. } | Command::LoginSaved { .. } | Command::LoginQr)) => {
                    *next = Some(c);
                    return Ok(None);
                }
                Some(_) => {}
            }
        }
    }
}

async fn wait_for(events: &mut net::connection::Events, want: u32) -> Result<Packet, Error> {
    let deadline = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            p = events.recv() => match p {
                Some(p) if p.emsg == want => return Ok(p),
                Some(p) if p.emsg == emsg::CLIENT_LOGGED_OFF => {
                    let off: CMsgClientLoggedOff = p.body()?;
                    return Err(net::Error::EResult(off.eresult(), String::new()).into());
                }
                Some(_) => {}
                None => return Err(net::Error::Disconnected.into()),
            },
            _ = &mut deadline => return Err(net::Error::Timeout.into()),
        }
    }
}

/// Detached tasks can outlive their online session during reconnect.
fn cloud_lock(account: SteamId, app: AppId) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    use std::sync::{Arc, LazyLock, Mutex, Weak};
    type Locks = HashMap<(SteamId, AppId), Weak<tokio::sync::Mutex<()>>>;
    static LOCKS: LazyLock<Mutex<Locks>> = LazyLock::new(Mutex::default);
    let mut locks = LOCKS.lock().unwrap();
    if let Some(lock) = locks.get(&(account, app)).and_then(Weak::upgrade) {
        return lock;
    }
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert((account, app), Arc::downgrade(&lock));
    lock
}

/// Data the online loop keeps between messages.
struct Online {
    conn: Connection,
    me: SteamId,
    account: String,
    friends: HashMap<SteamId, Relationship>,
    personas: HashMap<SteamId, Persona>,
    apps: HashMap<AppId, OwnedApp>,
    playing: Option<AppId>,
    /// Process ids of games we started.
    running: HashMap<AppId, u32>,
    /// Per-game stop flags to cancel launch waits or post-exit grace immediately.
    stop_flags: HashMap<AppId, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    cell_id: u32,
    /// Content host the player picked (a partner CDN), tried first.
    download_host: Option<String>,
    /// Games whose names we've looked up (not in the library).
    names_asked: std::collections::HashSet<AppId>,
    /// Your nicknames for people.
    nicknames: HashMap<SteamId, String>,
    /// Download region the player picked (overrides the login's region).
    download_cell: Option<u32>,
    /// Steam Cloud: where `userdata` is, and games the player turned it off for.
    steam_root: Option<std::path::PathBuf>,
    cloud_off: std::collections::HashSet<AppId>,
    /// Install folders of games we started (for the upload after they close).
    launched_dirs: HashMap<AppId, std::path::PathBuf>,
    queue: std::collections::VecDeque<crate::download::InstallRequest>,
    active: Option<(AppId, std::sync::Arc<depot::Progress>)>,
    /// The running download's request, kept so it can be saved and resumed.
    active_req: Option<crate::download::InstallRequest>,
    /// Unfinished downloads from the last run were put back in the queue.
    downloads_restored: bool,
    /// The library came from Steam this session (not just the saved copy
    /// shown at start-up). Installs wait for it: the saved depot info may be
    /// out of date.
    apps_fresh: bool,
    /// Games whose files are being verified, with the check's progress.
    verifying: HashMap<AppId, std::sync::Arc<local::manage::Progress>>,
    limit_kbps: u64,
    concurrency: u32,
    refresh_token: String,
    /// Short-lived web token for steamcommunity.com (memory only).
    web: crate::market::WebCache,
    /// Pipe that games with our steamclient DLL talk to.
    ipc: crate::ipc::Server,
    /// Each running game's token on that pipe.
    launch_tokens: HashMap<AppId, String>,
    launch_layouts: HashMap<AppId, std::sync::Arc<tokio::sync::Mutex<crate::input_config::LaunchLayout>>>,
    /// Games we read non-Xbox controllers for (see pads.rs).
    pad_users: HashSet<AppId>,
    /// Games backed by OS virtual Xbox 360 pads (`app_id -> include_xbox`).
    vpad_users: HashMap<AppId, bool>,
    /// Games the official Steam client runs (anti-cheat), by install folder.
    official: HashMap<AppId, std::path::PathBuf>,
    /// Games stopped because their Steam DRM refused our steamclient: Play
    /// starts them again (on Valve's engine) once they've closed.
    drm_relaunch: std::collections::HashSet<AppId>,
    /// Two-letter country of this connection.
    country: String,
    /// The wallet's Steam currency code, once known (prices and cart country).
    wallet_currency: Option<i32>,
    /// Auth session tickets for games with our steamclient.
    auth: crate::ipc::SharedAuth,
    /// Online, offline mode or reconnecting.
    link: Link,
    /// The player wants to be online (false after "Go offline").
    want_online: bool,
    /// The reconnect running now was a "Go online" from offline mode.
    from_offline: bool,
    /// Bumped for every new connection (tells stale checks apart).
    generation: u64,
    /// When Steam last sent anything (to spot a connection gone silent).
    last_rx: std::time::Instant,
    probing: bool,
    /// The status the player chose (set again after reconnecting).
    persona_state: PersonaState,
    /// Games running while not connected: (start, in offline mode).
    unconnected_play: HashMap<AppId, (u64, bool)>,
    /// The running download was paused because the connection dropped.
    auto_paused: bool,
    /// Automatic downloads are allowed (the "Automatic updates" setting). When
    /// off, downloads aren't resumed by themselves on startup.
    auto_downloads: bool,
    /// The offline cache needs writing; when it was last written.
    cache_dirty: bool,
    cache_saved: std::time::Instant,
    active_groups: crate::chat_groups::ActiveGroups,
    /// Games compiling their shaders before launch; set to skip the rest.
    shader_jobs: HashMap<AppId, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Games whose new Proton prefix was set up this session (once, even if
    /// it didn't take; see `launch::prefix_setup`).
    prefix_setups: std::collections::HashSet<AppId>,
}

/// Seconds before reconnect attempt `n` (1, 2, …), like Steam: quick at
/// first, then at most once a minute.
fn backoff(attempt: u32) -> u32 {
    match attempt {
        0 | 1 => 2,
        n => (5u32 << (n - 2).min(4)).min(60),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A packet stream that never delivers anything (while not connected).
fn quiet_events() -> (mpsc::UnboundedSender<Packet>, net::connection::Events) {
    mpsc::unbounded_channel()
}

/// How a server message ends the session.
enum Ended {
    /// Signed out for good (logged off, replaced): back to sign-in.
    Logout(String),
    /// Steam dropped us for a reason that passes (maintenance, busy server).
    Dropped,
}

/// The signed-in session, connected or not. `link` is `None` for offline
/// mode: everything comes from the cache saved at the last sign-in. A lost
/// connection doesn't end it: running games and the UI carry on while it
/// reconnects in the background.
async fn online(
    link: Option<(Connection, net::connection::Events, Logon)>,
    cmds: &mut Cmds,
    sink: &Sink,
    account: &store::SavedAccount,
    storage: &SharedStorageJobs,
) -> Result<String, Error> {
    launch::repair_registry();
    let (ipc_server, mut ipc_rx) = crate::ipc::Server::start();
    // Put Valve's steamclient back in the registry however this session ends.
    struct RestoreRegistry;
    impl Drop for RestoreRegistry {
        fn drop(&mut self) {
            launch::swap_restore_now();
        }
    }
    let _restore = RestoreRegistry;
    let offline_start = link.is_none();
    // Keeps the quiet stream open while not connected.
    let mut quiet_tx: Option<mpsc::UnboundedSender<Packet>> = None;
    let (conn, mut events, info) = match link {
        Some(l) => l,
        None => {
            let (tx, rx) = quiet_events();
            quiet_tx = Some(tx);
            (Connection::closed(), rx, Logon { steam_id: account.steam_id, heartbeat: 9, cell_id: 0, public_ip: 0 })
        }
    };
    // Publish family snapshots and accepted preferences in order.
    let family_gate = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let me = info.steam_id;
    let mut st = Online {
        conn: conn.clone(),
        me,
        account: account.account_name.clone(),
        friends: HashMap::new(),
        personas: HashMap::new(),
        apps: HashMap::new(),
        playing: None,
        running: HashMap::new(),
        stop_flags: HashMap::new(),
        cell_id: info.cell_id,
        download_cell: None,
        names_asked: Default::default(),
        nicknames: Default::default(),
        download_host: None,
        steam_root: None,
        cloud_off: Default::default(),
        launched_dirs: HashMap::new(),
        queue: Default::default(),
        active_req: None,
        downloads_restored: false,
        apps_fresh: false,
        verifying: HashMap::new(),
        active: None,
        limit_kbps: 0,
        concurrency: 16,
        refresh_token: account.refresh_token.clone(),
        web: Default::default(),
        ipc: ipc_server,
        launch_tokens: HashMap::new(),
        launch_layouts: HashMap::new(),
        pad_users: HashSet::new(),
        vpad_users: HashMap::new(),
        official: HashMap::new(),
        drm_relaunch: Default::default(),
        country: String::new(),
        wallet_currency: None,
        auth: crate::ipc::Auth::new(info.public_ip),
        link: if offline_start { Link::Offline } else { Link::Online },
        want_online: !offline_start,
        generation: 0,
        last_rx: std::time::Instant::now(),
        probing: false,
        persona_state: PersonaState::Online,
        unconnected_play: HashMap::new(),
        auto_paused: false,
        auto_downloads: true,
        cache_dirty: false,
        cache_saved: std::time::Instant::now(),
        active_groups: Default::default(),
        shader_jobs: Default::default(),
        prefix_setups: Default::default(),
        from_offline: false,
    };
    let (dl_tx, mut dl_rx) = mpsc::unbounded_channel::<(AppId, Result<String, String>)>();
    let (apps_tx, mut apps_rx) = mpsc::unbounded_channel::<Vec<OwnedApp>>();
    let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<(AppId, Option<std::path::PathBuf>)>();
    // Launches that waited for a Steam Cloud download.
    let (launch_tx, mut launch_rx) = mpsc::unbounded_channel::<(AppId, std::path::PathBuf, crate::LaunchArgs)>();
    // Launches that waited for Workshop downloads (Steam Cloud comes next).
    let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<(AppId, std::path::PathBuf, crate::LaunchArgs)>();
    // Launches that waited for their shaders to compile.
    let (sh_tx, mut sh_rx) = mpsc::unbounded_channel::<(AppId, std::path::PathBuf, crate::LaunchArgs)>();
    // Verifications that ended.
    let (vf_tx, mut vf_rx) = mpsc::unbounded_channel::<(AppId, local::manage::VerifyReport)>();
    // Reconnect attempts, and checks of a connection that went quiet.
    let (rc_tx, mut rc_rx) = mpsc::unbounded_channel::<Result<(Connection, net::connection::Events, Logon), Error>>();
    let (probe_tx, mut probe_rx) = mpsc::unbounded_channel::<(u64, bool)>();
    let (flush_tx, mut flush_rx) = mpsc::unbounded_channel::<()>();
    let mut retry_at: Option<tokio::time::Instant> = None;
    let mut connecting = false;
    let mut hb = tokio::time::interval(Duration::from_secs(info.heartbeat.clamp(5, 60) as u64));
    hb.tick().await;

    if offline_start {
        match offline::load_account(me) {
            Some(cache) => st.start_offline(cache, sink),
            None => return Err(Error::Other("No offline data for this account".into())),
        }
    } else {
        sink(Event::Link(Link::Online));
        st.connected(sink);
        // Show the library saved at the last sign-in right away, like Steam;
        // the fresh one from Steam replaces it in a few seconds.
        if let Some(cache) = offline::load_account(me).filter(|c| !c.apps.is_empty()) {
            tracing::info!("showing the saved library ({} apps) while Steam sends the current one", cache.apps.len());
            st.apps = cache.apps.iter().map(|a| (a.app_id, a.clone())).collect();
            sink(Event::Apps(cache.apps));
        }
        // Play time and stats from an earlier offline run go to Steam now.
        st.flush_offline(sink);
    }
    st.send_offline_lists(sink);

    // The connection is gone: carry on without it and try again soon.
    macro_rules! lost {
        ($why:expr) => {{
            tracing::warn!("lost the connection to Steam: {}", $why);
            let (tx, rx) = st.drop_connection(sink, false);
            events = rx;
            quiet_tx = Some(tx);
            retry_at = Some(tokio::time::Instant::now() + Duration::from_secs(backoff(1) as u64));
        }};
    }

    // The last step before a game starts, like Steam's "Processing Vulkan
    // shaders": compile its shaders (Linux). Games the official client runs
    // get Steam's own.
    macro_rules! launch_now {
        ($app_id:expr, $install_dir:expr, $args:expr) => {{
            let (app_id, install_dir, args): (AppId, std::path::PathBuf, crate::LaunchArgs) = ($app_id, $install_dir, $args);
            let app = st.apps.get(&app_id);
            let mode = app.map(|app| st.game_launch_mode(app, &install_dir, args.controller));
            if let Err(e) = launch::safe_connect_args(&args.connect) {
                sink(Event::Error(e));
            } else if !st.running.contains_key(&app_id)
                && app.zip(mode.as_ref()).is_some_and(|(app, mode)| launch::needs_anti_cheat_warning(app, &install_dir, mode))
            {
                sink(Event::AntiCheatWarning { app_id, install_dir, args, skip_cloud: true });
            } else {
            // Steam's download and shader settings as chosen in Aurora,
            // before anything of Steam's starts for this game.
            if !st.shader_jobs.contains_key(&app_id) && !st.running.contains_key(&app_id) {
                launch::apply_steam_prefs();
            }
            // First launch: the game's install script (registry keys,
            // anti-cheat setup…), like Steam's "Running install script".
            // The official client runs its own.
            let busy = st.running.contains_key(&app_id) || st.shader_jobs.contains_key(&app_id);
            let starts_here = !matches!(mode, Some(crate::LaunchMode::OfficialSteam { .. }));
            let script = app.filter(|_| !busy && starts_here).and_then(|a| crate::installscript::pending(a, &install_dir));
            // First launch in a new Proton prefix, no install script to
            // create it: set it up so our steamclient answers from the start.
            #[cfg(not(windows))]
            let setup = app
                .filter(|_| mode == Some(crate::LaunchMode::Aurora) && script.is_none() && !busy && !st.prefix_setups.contains(&app_id))
                .and_then(|a| launch::prefix_setup(a, &install_dir));
            #[cfg(windows)]
            let setup: Option<std::process::Command> = None;
            if let Some(mut cmd) = setup {
                st.prefix_setups.insert(app_id);
                sink(Event::InstallScript { app_id, step: Some("Proton setup".into()) });
                let (sink, tx) = (sink.clone(), launch_tx.clone());
                tokio::task::spawn_blocking(move || {
                    let started = std::time::Instant::now();
                    match launch::run_quietly(&mut cmd, Duration::from_secs(180)) {
                        Ok(()) => tracing::info!("Proton prefix for {app_id} set up in {:.1}s", started.elapsed().as_secs_f32()),
                        Err(e) => tracing::warn!("Proton prefix setup for {app_id}: {e}"),
                    }
                    sink(Event::InstallScript { app_id, step: None });
                    let _ = tx.send((app_id, install_dir, args));
                });
            } else if let Some(p) = script {
                sink(Event::InstallScript { app_id, step: Some(String::new()) });
                let (sink, tx) = (sink.clone(), launch_tx.clone());
                tokio::task::spawn_blocking(move || {
                    crate::installscript::run(p, |s| sink(Event::InstallScript { app_id, step: Some(s.to_string()) }));
                    sink(Event::InstallScript { app_id, step: None });
                    // Back through launch_now: shaders, then the game.
                    let _ = tx.send((app_id, install_dir, args));
                });
            } else {
            #[cfg(not(windows))]
            {
                let pending = app.filter(|_| !busy && starts_here)
                    .and_then(|_| crate::shaders::pending(&install_dir, app_id));
                match pending {
                    // Already compiling: Play again doesn't start it twice.
                    _ if st.shader_jobs.contains_key(&app_id) => {}
                    Some(p) => {
                        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                        st.shader_jobs.insert(app_id, cancel.clone());
                        sink(Event::ShaderCompile { app_id, done: 0, total: 0 });
                        let (sink, tx) = (sink.clone(), sh_tx.clone());
                        tokio::task::spawn_blocking(move || {
                            let mut last = None;
                            let result = crate::shaders::compile(&p, &cancel, |done, total| {
                                if last != Some((done, total)) {
                                    last = Some((done, total));
                                    sink(Event::ShaderCompile { app_id, done, total });
                                }
                            });
                            if let Err(e) = result {
                                tracing::warn!("shaders for {app_id}: {e}");
                            }
                            sink(Event::ShaderCompileDone { app_id });
                            let _ = tx.send((app_id, install_dir, args));
                        });
                    }
                    None => {
                        let _ = st.launch(app_id, &install_dir, &args, sink, &exit_tx);
                    }
                }
            }
            #[cfg(windows)]
            {
                let _ = st.launch(app_id, &install_dir, &args, sink, &exit_tx);
            }
            }
            }
        }};
    }

    // Launch with Steam Cloud first, like Steam: bring down newer cloud
    // saves (games the official client runs get its own cloud sync).
    macro_rules! launch_cloud {
        ($app_id:expr, $install_dir:expr, $args:expr) => {{
            let (app_id, install_dir, args): (AppId, std::path::PathBuf, crate::LaunchArgs) = ($app_id, $install_dir, $args);
            if let Err(e) = launch::safe_connect_args(&args.connect) {
                sink(Event::Error(e));
            } else {
                // Like Steam: bring down newer cloud saves first.
                let sync = st.apps.get(&app_id).is_some_and(|app| st.game_launch_mode(app, &install_dir, args.controller).uses_aurora_sync());
                match st.cloud_target(app_id, &install_dir).filter(|_| sync) {
                    // Offline: play with the saves on this PC; they go up once online.
                    Some(_) if st.link != Link::Online && !st.running.contains_key(&app_id) => {
                        sink(Event::CloudSkipped { app_id });
                        launch_now!(app_id, install_dir, args);
                    }
                    Some(target) if !st.running.contains_key(&app_id) => {
                        let Ok(guard) = cloud_lock(st.me, app_id).try_lock_owned() else {
                            sink(Event::Error("Steam Cloud is already syncing this game.".into()));
                            continue;
                        };
                        sink(Event::CloudSyncing { app_id });
                        let (conn, sink, tx) = (st.conn.clone(), sink.clone(), launch_tx.clone());
                        tokio::spawn(async move {
                            let _guard = guard;
                            let report = crate::cloud::sync(&conn, &target, crate::cloud::Mode::Download, &[], &[]).await;
                            let blocked = !report.conflicts.is_empty();
                            sink(Event::CloudReport { report, held_launch: blocked.then(|| args.clone()) });
                            if !blocked {
                                let _ = tx.send((app_id, install_dir, args));
                            }
                        });
                    }
                    _ => launch_now!(app_id, install_dir, args),
                }
            }
        }};
    }

    loop {
        let conn = st.conn.clone();
        tokio::select! {
            p = events.recv() => match p {
                Some(p) => {
                    st.last_rx = std::time::Instant::now();
                    match st.handle(p, sink, &apps_tx) {
                        Ok(None) => {}
                        Ok(Some(Ended::Logout(reason))) => return Ok(reason),
                        Ok(Some(Ended::Dropped)) => lost!("Steam ended the session for now"),
                        Err(e) => tracing::warn!("bad message from Steam: {e}"),
                    }
                }
                None => lost!("the server closed the connection"),
            },
            cmd = cmds.recv() => match cmd {
                None => {
                    st.record_active_unconnected_play();
                    return Ok("Shutting down".into());
                }
                Some(Command::LogOut) => {
                    st.record_active_unconnected_play();
                    let _ = conn.send(emsg::CLIENT_LOG_OFF, &CMsgClientLogOff {});
                    return Ok("Logged out".into());
                }
                Some(Command::GoOnline) => {
                    if st.link != Link::Online && !connecting {
                        st.from_offline = st.link == Link::Offline;
                        st.want_online = true;
                        retry_at = Some(tokio::time::Instant::now());
                    }
                }
                Some(Command::GoOffline) => {
                    st.want_online = false;
                    retry_at = None;
                    if st.link == Link::Online {
                        let _ = conn.send(emsg::CLIENT_LOG_OFF, &CMsgClientLogOff {});
                        let (tx, rx) = st.drop_connection(sink, true);
                        events = rx;
                        quiet_tx = Some(tx);
                    } else {
                        st.set_link(Link::Offline, sink);
                    }
                }
                Some(Command::SetPersonaState(s)) => {
                    st.persona_state = s;
                    let _ = set_persona(&conn, s);
                }
                Some(Command::JoinRunningGame { app_id, friend, connect }) => {
                    let lobby = lobby_of(&connect);
                    st.ipc.push(app_id, crate::ipc::protocol::GameEvent::JoinRequested { lobby, friend, connect });
                }
                Some(Command::AttachGame { app_id, install_dir }) => {
                    st.launched_dirs.insert(app_id, install_dir);
                    let token = st.ipc.register(app_id);
                    sink(Event::GameAttached { app_id, env: st.ipc.env_value(&token) });
                }
                // Like Steam: no playing while the game's files are changing.
                Some(Command::Launch { app_id, .. } | Command::LaunchWithoutCloud { app_id, .. })
                    if st.verifying.contains_key(&app_id)
                        || st.active.as_ref().is_some_and(|(a, _)| *a == app_id)
                        || storage_busy(storage, app_id) =>
                {
                    sink(Event::StoreError("This game is being updated, verified, or moved. You can play when that finishes.".into()));
                }
                Some(Command::VerifyGame { game, steam_root }) => {
                    let app_id = game.app_id;
                    if st.running.contains_key(&app_id) {
                        sink(Event::VerifyFinished { app_id, result: Err("Close the game before verifying its files.".into()) });
                    } else if st.verifying.contains_key(&app_id)
                        || st.active.as_ref().is_some_and(|(a, _)| *a == app_id)
                        || storage_busy(storage, app_id)
                    {
                        sink(Event::VerifyFinished { app_id, result: Err("This game is already being updated, verified, or moved.".into()) });
                    } else {
                        let progress = std::sync::Arc::new(local::manage::Progress::default());
                        st.verifying.insert(app_id, progress.clone());
                        // The check runs on a blocking thread; this task shows its
                        // progress on the Downloads page twice a second and then
                        // reports the result, so no progress arrives after it.
                        let (p2, sink2, tx) = (progress.clone(), sink.clone(), vf_tx.clone());
                        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
                        let result2 = result.clone();
                        tokio::spawn(async move {
                            loop {
                                if let Some(report) = result2.lock().unwrap().take() {
                                    let _ = tx.send((app_id, report));
                                    break;
                                }
                                sink2(Event::Download(model::DownloadStatus {
                                    app_id,
                                    phase: model::DownloadPhase::Verifying,
                                    disk_done: p2.done_bytes.load(std::sync::atomic::Ordering::Relaxed),
                                    disk_total: p2.total_bytes.load(std::sync::atomic::Ordering::Relaxed),
                                    ..Default::default()
                                }));
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        });
                        tokio::task::spawn_blocking(move || {
                            let report = local::manage::verify(&steam_root, &game, &progress);
                            *result.lock().unwrap() = Some(report);
                        });
                    }
                }
                Some(Command::CancelVerify { app_id }) => {
                    if let Some(p) = st.verifying.get(&app_id) {
                        p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Some(Command::Launch { app_id, install_dir, args }) => {
                    if let Err(e) = launch::safe_connect_args(&args.connect) {
                        sink(Event::Error(e));
                    } else {
                        // Like Steam: subscribed Workshop items are downloaded
                        // (or updated) before the game starts, then cloud saves.
                        let sync = if let Some(app) = st.apps.get(&app_id) {
                            let mode = st.game_launch_mode(app, &install_dir, args.controller);
                            if !st.running.contains_key(&app_id) && launch::needs_anti_cheat_warning(app, &install_dir, &mode) {
                                sink(Event::AntiCheatWarning { app_id, install_dir, args, skip_cloud: false });
                                continue;
                            }
                            mode.uses_aurora_sync()
                        } else {
                            false
                        };
                        if !st.running.contains_key(&app_id) {
                            sink(Event::GameLaunching { app_id });
                        }
                        let library = crate::workshop::library_of(&install_dir).filter(|_| sync && st.link == Link::Online && !st.running.contains_key(&app_id));
                        match library {
                            Some(library) => {
                                let (conn, sink, tx, me, cell, opts) = (
                                    st.conn.clone(),
                                    sink.clone(),
                                    ws_tx.clone(),
                                    st.me,
                                    st.download_cell.unwrap_or(st.cell_id),
                                    st.workshop_options(),
                                );
                                tokio::spawn(async move {
                                    let result = crate::workshop::sync(&conn, me, app_id, &library, cell, Vec::new(), opts).await;
                                    if !matches!(&result, Ok(r) if r.downloaded.is_empty() && r.failed.is_empty()) {
                                        sink(Event::WorkshopSync { app_id, result });
                                    }
                                    let _ = tx.send((app_id, install_dir, args));
                                });
                            }
                            None => launch_cloud!(app_id, install_dir, args),
                        }
                    }
                }
                Some(Command::SyncWorkshop { app_id }) => st.sync_workshop(app_id, Vec::new(), sink),
                Some(Command::InspectInputConfig { key, install_dir }) => {
                    if key.account != model::account_id(st.me) {
                        continue;
                    }
                    let root = st
                        .steam_root
                        .clone()
                        .or_else(|| launch::reg_string("Software\\Valve\\Steam", "SteamPath").map(|p| std::path::PathBuf::from(p.replace('/', "\\"))))
                        .unwrap_or_default();
                    let (conn, me, sink) = (st.conn.clone(), st.me, sink.clone());
                    tokio::spawn(async move {
                        let report = crate::input_config::inspect(&conn, key.app_id, me, &root, &install_dir).await;
                        sink(Event::InputConfigInspected { key, report });
                    });
                }
                Some(Command::QueryControllerLayouts { key, cursor }) => {
                    if key.account != model::account_id(st.me) {
                        continue;
                    }
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    let online = st.link == Link::Online;
                    tokio::spawn(async move {
                        let result = if online {
                            crate::input_config::community::query(&conn, key.app_id, cursor).await
                        } else { Err("Steam is offline".into()) };
                        sink(Event::ControllerLayouts { key, result });
                    });
                }
                Some(Command::DownloadControllerLayout { key, id }) => {
                    if key.account != model::account_id(st.me) {
                        continue;
                    }
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    let online = st.link == Link::Online;
                    tokio::spawn(async move {
                        let result = if online {
                            crate::input_config::community::download(&conn, key, id).await
                        } else { Err("Steam is offline".into()) };
                        sink(Event::ControllerLayoutDownloaded { key, result });
                    });
                }
                Some(Command::LaunchWithoutCloud { app_id, install_dir, args }) => {
                    if !st.running.contains_key(&app_id) {
                        sink(Event::GameLaunching { app_id });
                    }
                    launch_now!(app_id, install_dir, args);
                }
                Some(Command::SkipShaderCompile { app_id }) => {
                    if let Some(cancel) = st.shader_jobs.get(&app_id) {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Some(Command::ProbeRegions) => {
                    let (sink, root) = (sink.clone(), st.steam_root.clone());
                    tokio::spawn(async move {
                        sink(Event::Regions(crate::regions::probe(root).await));
                    });
                }
                Some(Command::DownloadRegion { cell_id, host }) => {
                    st.download_cell = cell_id;
                    st.download_host = host;
                }
                Some(Command::CloudSettings { steam_root, disabled }) => {
                    st.steam_root = steam_root;
                    st.cloud_off = disabled.into_iter().collect();
                }
                Some(Command::CloudSync { app_id, install_dir, mode, keep_local, keep_cloud, then_launch }) => {
                    if let Some(Err(e)) = then_launch.as_ref().map(|a| launch::safe_connect_args(&a.connect)) {
                        sink(Event::Error(e));
                    } else if mode != crate::cloud::Mode::Check && st.playing == Some(app_id) {
                        sink(Event::Error("Close the game before syncing Steam Cloud.".into()));
                    } else if let Some(target) = st.cloud_target_any(app_id, &install_dir) {
                        let Ok(guard) = cloud_lock(st.me, app_id).try_lock_owned() else {
                            sink(Event::Error("Steam Cloud is already syncing this game.".into()));
                            continue;
                        };
                        // A manual sync works even when automatic sync is off.
                        sink(Event::CloudSyncing { app_id });
                        let (conn, sink, tx, me) = (conn.clone(), sink.clone(), launch_tx.clone(), st.me);
                        tokio::spawn(async move {
                            let _guard = guard;
                            let report = crate::cloud::sync(&conn, &target, mode, &keep_local, &keep_cloud).await;
                            let blocked = !report.conflicts.is_empty();
                            if matches!(mode, crate::cloud::Mode::Upload | crate::cloud::Mode::Sync) && report.errors.is_empty() && !blocked {
                                offline::remove_cloud_pending(me, app_id);
                                sink(Event::CloudPending(offline::cloud_pending(me).iter().map(|p| p.0).collect()));
                            }
                            sink(Event::CloudReport { report, held_launch: then_launch.clone().filter(|_| blocked) });
                            if let (Some(args), false) = (then_launch, blocked) {
                                let _ = tx.send((app_id, install_dir, args));
                            }
                        });
                    }
                }
                Some(Command::CloudDelete { app_id, install_dir, names }) => {
                    if st.playing == Some(app_id) {
                        sink(Event::Error("Close the game before deleting Steam Cloud files.".into()));
                    } else if let Some(target) = st.cloud_target_any(app_id, &install_dir) {
                        let Ok(guard) = cloud_lock(st.me, app_id).try_lock_owned() else {
                            sink(Event::Error("Steam Cloud is already syncing this game.".into()));
                            continue;
                        };
                        sink(Event::CloudSyncing { app_id });
                        let (conn, sink) = (conn.clone(), sink.clone());
                        tokio::spawn(async move {
                            let _guard = guard;
                            let deleted = crate::cloud::delete(&conn, &target, &names).await;
                            let mut report = crate::cloud::sync(&conn, &target, crate::cloud::Mode::Check, &[], &[]).await;
                            if let Err(e) = deleted {
                                report.errors.push((String::new(), format!("Couldn't delete from Steam Cloud: {e}")));
                            }
                            sink(Event::CloudReport { report, held_launch: None });
                        });
                    }
                }
                Some(Command::StopGame { app_id }) => {
                    if let Some(flag) = st.stop_flags.get(&app_id) {
                        flag.store(true, std::sync::atomic::Ordering::Release);
                    }
                    if let Some(dir) = st.official.get(&app_id).cloned() {
                        tokio::task::spawn_blocking(move || launch::kill_in_folder(&dir));
                    } else if let Some(pid) = st.running.get(&app_id).copied() {
                        tokio::task::spawn_blocking(move || launch::kill_tree(pid));
                    }
                }
                Some(Command::ForgetAccount { account }) => store::remove(&account),
                Some(Command::SendMessage { to, text }) => st.send_message(to, text, sink),
                Some(Command::LoadChatGroups) => crate::chat_groups::load_my_groups(st.conn.clone(), st.me, st.active_groups.clone(), sink.clone()),
                Some(Command::OpenChatGroup { group_id }) => crate::chat_groups::load_group_state(st.conn.clone(), st.me, group_id, st.active_groups.clone(), sink.clone()),
                Some(Command::LoadGroupHistory { group_id, chat_id, before }) => crate::chat_groups::load_history(st.conn.clone(), st.me, group_id, chat_id, before, sink.clone()),
                Some(Command::SendGroupMessage { group_id, chat_id, text }) => crate::chat_groups::send_message(st.conn.clone(), st.me, group_id, chat_id, text, sink.clone()),
                Some(Command::AckGroupMessage { group_id, chat_id, timestamp }) => crate::chat_groups::ack_message(&st.conn, group_id, chat_id, timestamp),
                Some(Command::CreateChatGroup { name, invitees }) => crate::chat_groups::create_group(st.conn.clone(), st.me, st.active_groups.clone(), name, invitees, sink.clone()),
                Some(Command::InviteToChatGroup { group_id, chat_id, friend }) => crate::chat_groups::invite_friend(st.conn.clone(), group_id, chat_id, friend, sink.clone()),
                Some(Command::JoinChatGroup { group_id, chat_id, invite_code }) => crate::chat_groups::join_group(st.conn.clone(), st.me, st.active_groups.clone(), group_id, chat_id, invite_code, sink.clone()),
                Some(Command::LeaveChatGroup { group_id }) => crate::chat_groups::leave_group(st.conn.clone(), st.active_groups.clone(), group_id, sink.clone()),
                Some(Command::SaveChatGroup { group_id, name }) => crate::chat_groups::save_group(st.conn.clone(), st.me, st.active_groups.clone(), group_id, name, sink.clone()),
                Some(Command::RenameChatGroup { group_id, name }) => crate::chat_groups::rename_group(st.conn.clone(), st.me, st.active_groups.clone(), group_id, name, sink.clone()),
                Some(Command::SetChatGroupTagline { group_id, tagline }) => crate::chat_groups::set_tagline(st.conn.clone(), st.me, st.active_groups.clone(), group_id, tagline, sink.clone()),
                Some(Command::CreateChatChannel { group_id, name, allow_voice }) => crate::chat_groups::create_channel(st.conn.clone(), st.me, st.active_groups.clone(), group_id, name, allow_voice, sink.clone()),
                Some(Command::RenameChatChannel { group_id, chat_id, name }) => crate::chat_groups::rename_channel(st.conn.clone(), st.me, st.active_groups.clone(), group_id, chat_id, name, sink.clone()),
                Some(Command::DeleteChatChannel { group_id, chat_id }) => crate::chat_groups::delete_channel(st.conn.clone(), st.me, st.active_groups.clone(), group_id, chat_id, sink.clone()),
                Some(Command::CreateGroupInviteLink { group_id, chat_id, seconds_valid }) => crate::chat_groups::create_invite_link(st.conn.clone(), group_id, chat_id, seconds_valid, sink.clone()),
                Some(Command::LoadGroupInviteLinks { group_id }) => crate::chat_groups::load_invite_links(st.conn.clone(), group_id, sink.clone()),
                Some(Command::DeleteGroupInviteLink { group_id, code }) => crate::chat_groups::delete_invite_link(st.conn.clone(), group_id, code, sink.clone()),
                Some(Command::ResolveGroupInvite { invite }) => crate::chat_groups::resolve_invite(st.conn.clone(), st.me, invite, sink.clone()),
                Some(Command::KickGroupMember { group_id, steam_id, expiration_secs }) => crate::chat_groups::kick_member(st.conn.clone(), group_id, steam_id, expiration_secs, sink.clone()),
                Some(Command::BanGroupMember { group_id, steam_id, ban }) => crate::chat_groups::ban_member(st.conn.clone(), group_id, steam_id, ban, sink.clone()),
                Some(Command::LoadGroupBanList { group_id }) => crate::chat_groups::load_ban_list(st.conn.clone(), group_id, sink.clone()),
                Some(Command::UpdateGroupReaction { group_id, chat_id, timestamp, ordinal, reaction_type, reaction, add }) => {
                    crate::chat_groups::update_reaction(st.conn.clone(), st.me, group_id, chat_id, timestamp, ordinal, reaction_type, reaction, add, sink.clone());
                }
                Some(Command::DeleteGroupMessages { group_id, chat_id, messages }) => crate::chat_groups::delete_messages(st.conn.clone(), group_id, chat_id, messages, sink.clone()),
                Some(Command::SetGroupNotificationPrefs { group_id, unread_muted, desktop_level }) => {
                    crate::chat_groups::set_preferences(st.conn.clone(), st.me, st.active_groups.clone(), group_id, unread_muted, desktop_level, sink.clone());
                }
                Some(Command::SearchGroupMembers { group_id, query }) => crate::chat_groups::search_members(st.conn.clone(), group_id, query, sink.clone()),
                Some(Command::InviteToLobby { app_id, lobby, friend }) => {
                    // Steam's own lobby invite: it lets them in (private and
                    // friends-only lobbies) and Steam writes the [lobbyinvite]
                    // chat message every client shows.
                    crate::ipc::lobby_invite(&st.conn, app_id, lobby, friend);
                }
                Some(Command::LoadHistory { friend }) => st.load_history(friend, sink),
                Some(Command::LoadProfile { steam_id }) => st.load_profile(steam_id, sink),
                Some(Command::AddFriend { steam_id }) => {
                    let _ = conn.send(emsg::CLIENT_ADD_FRIEND, &CMsgClientAddFriend { steamid_to_add: Some(steam_id), ..Default::default() });
                }
                Some(Command::RemoveFriend { steam_id }) => {
                    let _ = conn.send(emsg::CLIENT_REMOVE_FRIEND, &CMsgClientRemoveFriend { friendid: Some(steam_id) });
                }
                Some(Command::LoadInventory { steam_id, app_id, context_id, language }) => st.load_inventory(steam_id, app_id, context_id, language, sink),
                Some(Command::LoadInventoryApps { steam_id }) => st.load_inventory_apps(steam_id, sink),
                Some(Command::MarketPrice { app_id, hash_name, currency, country, full }) => st.market_price(app_id, hash_name, currency, country, full, sink),
                Some(Command::SellItem { app_id, context_id, asset_id, amount, receive }) => st.sell_item(app_id, context_id, asset_id, amount, receive, sink),
                Some(Command::LoadMyListings) => st.my_listings(sink),
                Some(Command::LoadProfileEdit { language }) => {
                    let conn = st.conn.clone();
                    st.with_web(sink, move |web, sink| async move {
                        let r = match web {
                            Ok(w) => crate::profile_edit::load(&w, &conn, &language).await,
                            Err(e) => Err(e),
                        };
                        sink(Event::ProfileEdit(r));
                    })
                }
                Some(Command::SaveProfileInfo(e)) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::profile_edit::save_info(&w, &e).await,
                        Err(e) => Err(e),
                    };
                    sink(Event::ProfileSaved { what: model::ProfileSave::Info, result });
                }),
                Some(Command::SaveProfilePrivacy(p)) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::profile_edit::save_privacy(&w, &p).await,
                        Err(e) => Err(e),
                    };
                    sink(Event::ProfileSaved { what: model::ProfileSave::Privacy, result });
                }),
                Some(Command::UploadAvatar { path }) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => match crate::profile_edit::read_avatar_file(&path) {
                            Ok((bytes, name)) => crate::profile_edit::upload_avatar(&w, bytes, &name).await,
                            Err(e) => Err(e),
                        },
                        Err(e) => Err(e),
                    };
                    sink(Event::ProfileSaved { what: model::ProfileSave::Avatar, result });
                }),
                Some(Command::EquipProfileItem { kind, id }) => {
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        let result = crate::profile_edit::equip(&conn, kind, id).await;
                        sink(Event::ProfileSaved { what: model::ProfileSave::Item, result });
                    });
                }
                Some(Command::SetBackgroundFullScreen { id, full }) => {
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        let result = crate::profile_edit::set_background_full(&conn, id, full).await;
                        sink(Event::ProfileSaved { what: model::ProfileSave::BackgroundFlags, result });
                    });
                }
                Some(Command::SetFavoriteBadge { badge }) => {
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        let result = crate::profile_edit::set_favorite_badge(&conn, badge).await;
                        sink(Event::ProfileSaved { what: model::ProfileSave::Badge, result });
                    });
                }
                Some(Command::SetProfileTheme(theme)) => {
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        let result = crate::profile_edit::set_theme(&conn, &theme).await;
                        sink(Event::ProfileSaved { what: model::ProfileSave::Theme, result });
                    });
                }
                Some(Command::LoadLocations { country, state }) => st.with_web(sink, move |web, sink| async move {
                    let result = crate::profile_edit::locations(web.as_deref().ok(), country.as_deref(), state.as_deref()).await;
                    sink(Event::Locations { country, state, result });
                }),
                Some(Command::LoadSteamCollections) => {
                    let (conn, sink) = (conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        let req = proto::webui::CCloudConfigStoreDownloadRequest { versions: vec![proto::webui::CCloudConfigStoreNamespaceVersion { enamespace: Some(1), version: Some(0) }] };
                        let result = conn
                            .call::<_, proto::webui::CCloudConfigStoreDownloadResponse>("CloudConfigStore.Download#1", &req)
                            .await
                            .map_err(|e| e.to_string())
                            .map(|r| crate::social::collections(r.data.iter().flat_map(|d| d.entries.iter()).map(|e| (e.key(), e.is_deleted(), e.value()))));
                        sink(Event::SteamCollections(result));
                    });
                }
                Some(Command::SaveSteamCollections { save, delete }) => {
                    let (conn, sink) = (conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        let result = crate::collections::save(&conn, &save, &delete).await;
                        if let Err(e) = &result {
                            tracing::warn!("saving collections to Steam failed: {e}");
                        }
                        sink(Event::SteamCollectionsSaved { save, delete, result });
                    });
                }
                Some(Command::LoadComments { steam_id, start }) => st.with_web(sink, move |web, sink| async move {
                    let result = crate::social::comments(web.as_deref().ok(), steam_id, start).await;
                    sink(Event::Comments { steam_id, result });
                }),
                Some(Command::LoadProfilePage { steam_id, language }) => st.with_web(sink, move |web, sink| async move {
                    let result = crate::profile_page::load(web.as_deref().ok(), steam_id, &language).await;
                    sink(Event::ProfilePage { steam_id, result });
                }),
                Some(Command::LoadAliases { steam_id }) => st.with_web(sink, move |web, sink| async move {
                    let result = crate::social::aliases(web.as_deref().ok(), steam_id).await;
                    sink(Event::Aliases { steam_id, result });
                }),
                Some(Command::ResolveProfileUrl { vanity }) => st.with_web(sink, move |web, sink| async move {
                    let vanity = vanity.to_ascii_lowercase();
                    let result = crate::social::resolve_vanity(web.as_deref().ok(), &vanity).await;
                    sink(Event::ProfileUrlResolved { vanity, result });
                }),
                Some(Command::ClearAliases) => {
                    let me = st.me;
                    st.with_web(sink, move |web, sink| async move {
                        let result = match web {
                            Ok(w) => crate::social::clear_aliases(&w, me).await,
                            Err(e) => Err(e),
                        };
                        sink(Event::AliasesCleared(result));
                    })
                }
                Some(Command::LoadShowcases { steam_id, language }) => {
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        let result = crate::showcases::load(&conn, steam_id, &language).await;
                        sink(Event::Showcases { steam_id, result });
                    });
                }
                Some(Command::SetNickname { steam_id, nickname }) => {
                    let nick = nickname.clone().unwrap_or_default();
                    let _ = st.conn.send(emsg::AM_CLIENT_SET_PLAYER_NICKNAME, &CMsgClientSetPlayerNickname { steamid: Some(steam_id), nickname: Some(nick) });
                    match nickname.filter(|n| !n.is_empty()) {
                        Some(n) => st.nicknames.insert(steam_id, n),
                        None => st.nicknames.remove(&steam_id),
                    };
                    let entry = st.personas.entry(steam_id).or_insert_with(|| Persona { steam_id, ..Default::default() });
                    entry.nickname = st.nicknames.get(&steam_id).cloned();
                    sink(Event::Personas(vec![entry.clone()]));
                }
                Some(Command::BlockFriend { steam_id, block }) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::social::block_user(&w, steam_id, block).await,
                        Err(e) => Err(e),
                    };
                    sink(Event::Blocked { steam_id, result });
                }),
                Some(Command::QueryServer { addr }) => {
                    let sink = sink.clone();
                    tokio::task::spawn_blocking(move || {
                        let result = crate::a2s::query(&addr);
                        sink(Event::ServerInfo { addr, result });
                    });
                }
                Some(Command::BrowseGameServers { request_id, filter, max }) => {
                    let conn = st.conn.clone();
                    let sink = sink.clone();
                    let app_id = filter
                        .split('\\')
                        .collect::<Vec<_>>()
                        .windows(2)
                        .find(|w| w[0].eq_ignore_ascii_case("appid"))
                        .and_then(|w| w[1].parse::<AppId>().ok())
                        .unwrap_or(0);
                    tokio::spawn(async move {
                        let result = crate::game_servers::browse(&conn, app_id, &filter, max)
                            .await
                            .map(|list| list.into_iter().map(crate::game_servers::entry_from_wire).collect());
                        sink(Event::GameServerList { request_id, result });
                    });
                }
                Some(Command::DiscoverLanServers { request_id }) => {
                    let sink = sink.clone();
                    tokio::task::spawn_blocking(move || {
                        let found = crate::a2s::discover_lan(Duration::from_millis(1500));
                        let servers = found
                            .into_iter()
                            .map(|(addr, info)| {
                                let port = addr
                                    .rsplit_once(':')
                                    .and_then(|(_, p)| p.parse::<u16>().ok())
                                    .unwrap_or(27015);
                                let ip = local::servers::parse_ipv4_port(&addr).map(|(ip, _)| ip).unwrap_or(0);
                                model::GameServerEntry {
                                    addr,
                                    ip,
                                    game_port: port,
                                    query_port: port,
                                    steam_id: 0,
                                    app_id: 0,
                                    name: info.name,
                                    map: info.map,
                                    game_dir: String::new(),
                                    description: info.game,
                                    players: u32::from(info.players),
                                    max_players: u32::from(info.max_players),
                                    bots: u32::from(info.bots),
                                    password: info.password,
                                    secure: info.vac,
                                    ping_ms: info.ping_ms,
                                    tags: String::new(),
                                }
                            })
                            .collect();
                        sink(Event::LanServerList { request_id, servers });
                    });
                }
                Some(Command::QueryServerDetails { addr }) => {
                    let sink = sink.clone();
                    tokio::task::spawn_blocking(move || {
                        let info = crate::a2s::query(&addr);
                        sink(Event::ServerInfo { addr: addr.clone(), result: info });
                        let players = crate::a2s::query_players(&addr);
                        sink(Event::ServerPlayers { addr: addr.clone(), result: players });
                        let rules = crate::a2s::query_rules(&addr);
                        sink(Event::ServerRules { addr, result: rules });
                    });
                }
                Some(Command::LoadRecentPlayers) => {
                    let me = st.me;
                    st.with_web(sink, move |web, sink| async move {
                        let result = crate::social::recent_players(web.as_deref().ok(), me).await;
                        sink(Event::RecentPlayers(result));
                    });
                }
                Some(Command::PostComment { steam_id, text }) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::social::post_comment(&w, steam_id, &text).await,
                        Err(e) => Err(e),
                    };
                    let ok = result.is_ok();
                    sink(Event::CommentPosted { steam_id, result: result.map(|_| ()) });
                    if ok {
                        let w = crate::market::Web::anonymous();
                        sink(Event::Comments { steam_id, result: crate::social::comments(Some(&w), steam_id, 0).await });
                    }
                }),
                Some(Command::DeleteComment { steam_id, comment_id }) => {
                    let conn = st.conn.clone();
                    st.with_web(sink, move |web, sink| async move {
                    // Steam's comment service first (what its web pages use
                    // now), the old form endpoint if that fails.
                    let result = match crate::social::delete_comment_service(&conn, steam_id, comment_id).await {
                        Ok(()) => {
                            let anon = crate::market::Web::anonymous();
                            let w: &crate::market::Web = match &web {
                                Ok(w) => w,
                                Err(_) => &anon,
                            };
                            crate::social::comments(Some(w), steam_id, 0).await
                        }
                        Err(service) => {
                            tracing::warn!("Community.DeleteCommentFromThread: {service}");
                            match web {
                                Ok(w) => crate::social::delete_comment(&w, steam_id, comment_id).await,
                                Err(e) => Err(e),
                            }
                        }
                    };
                    match result {
                        Ok(page) => sink(Event::Comments { steam_id, result: Ok(page) }),
                        Err(e) => sink(Event::Error(format!("Couldn't delete the comment: {e}"))),
                    }
                    });
                }
                Some(Command::LoadBadges { steam_id }) => st.with_web(sink, move |web, sink| async move {
                    let result = crate::social::badges(web.as_deref().ok(), steam_id).await;
                    sink(Event::Badges { steam_id, result });
                }),
                Some(Command::LoadReviews { steam_id, page }) => st.with_web(sink, move |web, sink| async move {
                    let result = crate::social::reviews(web.as_deref().ok(), steam_id, page).await;
                    sink(Event::Reviews { steam_id, page, result });
                }),
                Some(Command::LoadGroups { steam_id }) => st.with_web(sink, move |web, sink| async move {
                    let result = crate::social::groups(web.as_deref().ok(), steam_id).await;
                    sink(Event::Groups { steam_id, result });
                }),
                Some(Command::LoadAppReviews { app_id, language, cursor, filter }) => {
                    let sink = sink.clone();
                    tokio::spawn(async move {
                        let more = cursor.is_some();
                        let result = crate::reviews::load_filtered(app_id, &language, cursor.as_deref().unwrap_or("*"), filter).await;
                        sink(Event::AppReviews { app_id, filter, more, result });
                    });
                }
                Some(Command::LoadMyReview { app_id }) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::reviews::mine(&w, app_id).await,
                        Err(e) => Err(e),
                    };
                    sink(Event::MyReview { app_id, result });
                }),
                Some(Command::PostReview { app_id, text, recommended, public, allow_comments, language }) => st.with_web(sink, move |web, sink| async move {
                    let draft = crate::reviews::Draft { text, recommended, public, allow_comments, language };
                    let result = match &web {
                        Ok(w) => crate::reviews::post(w, app_id, &draft).await,
                        Err(e) => Err(e.clone()),
                    };
                    let ok = result.is_ok();
                    sink(Event::ReviewSaved { app_id, deleted: false, result });
                    if let (true, Ok(w)) = (ok, web) {
                        sink(Event::MyReview { app_id, result: crate::reviews::mine(&w, app_id).await });
                    }
                }),
                Some(Command::DeleteReview { app_id }) => st.with_web(sink, move |web, sink| async move {
                    let result = match &web {
                        Ok(w) => crate::reviews::delete(w, app_id).await,
                        Err(e) => Err(e.clone()),
                    };
                    let ok = result.is_ok();
                    sink(Event::ReviewSaved { app_id, deleted: true, result });
                    if ok {
                        sink(Event::MyReview { app_id, result: Ok(None) });
                    }
                }),
                Some(Command::LoadStoreRecommendations { language }) => {
                    let (conn, sink) = (conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        sink(Event::StoreRecommendations(crate::store_recs::load(&conn, &language).await));
                    });
                }
                Some(Command::SendTradeOffer { partner, token, message, give, take }) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::market::send_trade_offer(&w, partner, token.as_deref(), &message, &give, &take).await,
                        Err(e) => Err(e),
                    };
                    sink(Event::TradeOfferSent { partner, result });
                }),
                Some(Command::LoadCardSet { steam_id, app_id, foil }) => st.with_web(sink, move |web, sink| async move {
                    let result = crate::social::card_set(web.as_deref().ok(), steam_id, app_id, foil).await;
                    sink(Event::CardSet { steam_id, app_id, foil, result });
                }),
                Some(Command::LoadNotifications) => st.load_notifications(sink),
                Some(Command::MarkNotificationsRead { ids }) => st.mark_read(ids, sink),
                Some(Command::LoadSupportTickets) => st.support_tickets(sink),
                Some(Command::LoadAccountSecurity) => {
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    tokio::spawn(async move { sink(Event::AccountSecurity(crate::account::security(&conn).await)) });
                }
                Some(Command::LoadFamily { language }) => {
                    let (conn, sink, me, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move { reload_family(&family_gate, &conn, &sink, me, &language).await });
                }
                Some(Command::CreateFamily { name, language }) => {
                    let (conn, sink, me, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move {
                        match crate::account::create_family(&conn, me, &name).await {
                            Ok(_) => {
                                sink(Event::FamilyAction(Ok(model::FamilyActionDone::Created)));
                                reload_family(&family_gate, &conn, &sink, me, &language).await;
                            }
                            Err(e) => sink(Event::FamilyAction(Err(e))),
                        }
                    });
                }
                Some(Command::RenameFamily { family_id, name, language }) => {
                    let (conn, sink, me, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move {
                        match crate::account::rename_family(&conn, family_id, &name).await {
                            Ok(()) => {
                                sink(Event::FamilyAction(Ok(model::FamilyActionDone::Renamed)));
                                reload_family(&family_gate, &conn, &sink, me, &language).await;
                            }
                            Err(e) => sink(Event::FamilyAction(Err(e))),
                        }
                    });
                }
                Some(Command::InviteToFamily { family_id, steam_id, role, language }) => {
                    let (conn, sink, me, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move {
                        match crate::account::invite_to_family(&conn, family_id, steam_id, role).await {
                            Ok(needs_2fa) => {
                                sink(Event::FamilyAction(Ok(model::FamilyActionDone::Invited { needs_2fa })));
                                reload_family(&family_gate, &conn, &sink, me, &language).await;
                            }
                            Err(e) => sink(Event::FamilyAction(Err(e))),
                        }
                    });
                }
                Some(Command::CancelFamilyInvite { family_id, steam_id, language }) => {
                    let (conn, sink, me, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move {
                        match crate::account::cancel_family_invite(&conn, family_id, steam_id).await {
                            Ok(()) => {
                                sink(Event::FamilyAction(Ok(model::FamilyActionDone::InviteCancelled)));
                                reload_family(&family_gate, &conn, &sink, me, &language).await;
                            }
                            Err(e) => sink(Event::FamilyAction(Err(e))),
                        }
                    });
                }
                Some(Command::ResendFamilyInvite { family_id, steam_id }) => {
                    let (conn, sink) = (st.conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        match crate::account::resend_family_invite(&conn, family_id, steam_id).await {
                            Ok(()) => sink(Event::FamilyAction(Ok(model::FamilyActionDone::InviteResent))),
                            Err(e) => sink(Event::FamilyAction(Err(e))),
                        }
                    });
                }
                Some(Command::JoinFamily { family_id, language }) => {
                    let (conn, sink, me, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move {
                        match crate::account::join_family(&conn, family_id).await {
                            Ok(needs_2fa) => {
                                sink(Event::FamilyAction(Ok(model::FamilyActionDone::Joined { needs_2fa })));
                                reload_family(&family_gate, &conn, &sink, me, &language).await;
                            }
                            Err(e) => sink(Event::FamilyAction(Err(e))),
                        }
                    });
                }
                Some(Command::RemoveFromFamily { family_id, steam_id, language }) => {
                    let (conn, sink, me, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move {
                        match crate::account::remove_from_family(&conn, family_id, steam_id).await {
                            Ok(()) => {
                                let done = if steam_id == me { model::FamilyActionDone::Left } else { model::FamilyActionDone::Removed };
                                sink(Event::FamilyAction(Ok(done)));
                                reload_family(&family_gate, &conn, &sink, me, &language).await;
                            }
                            Err(e) => sink(Event::FamilyAction(Err(e))),
                        }
                    });
                }
                Some(Command::DeleteFamily { family_id, language }) => {
                    let (conn, sink, me, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move {
                        match crate::account::delete_family(&conn, family_id).await {
                            Ok(()) => {
                                sink(Event::FamilyAction(Ok(model::FamilyActionDone::Deleted)));
                                reload_family(&family_gate, &conn, &sink, me, &language).await;
                            }
                            Err(e) => sink(Event::FamilyAction(Err(e))),
                        }
                    });
                }
                Some(Command::SetPreferredLender { request_id, family_id, app_id, lender }) => {
                    let (conn, sink, steam_id, family_gate) = (st.conn.clone(), sink.clone(), st.me, family_gate.clone());
                    tokio::spawn(async move {
                        let _guard = family_gate.lock().await;
                        let result = crate::account::set_preferred_lender(&conn, family_id, app_id, lender).await;
                        sink(Event::PreferredCopyChanged { request_id, steam_id, family_id, app_id, lender, result });
                    });
                }
                Some(Command::LoadPurchases { language, cursor }) => st.with_web(sink, move |web, sink| async move {
                    let more = cursor.is_some();
                    let result = match web {
                        Ok(w) => crate::account::purchases(&w, &language, cursor).await,
                        Err(e) => Err(e),
                    };
                    sink(Event::Purchases { more, result });
                }),
                Some(Command::LoadLicenses { language }) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::account::licenses(&w, &language).await,
                        Err(e) => Err(e),
                    };
                    sink(Event::LicenseList(result));
                }),
                Some(Command::LoadSupportTicket { id, url }) => st.support_ticket(id, url, sink),
                Some(Command::CancelListing { listing_id }) => st.cancel_listing(listing_id, sink),
                Some(Command::CancelBuyOrder { order_id }) => st.cancel_buy_order(order_id, sink),
                Some(Command::LoadPlayerGames { steam_id, language }) => st.load_player_games(steam_id, language, sink),
                Some(Command::LoadAchievementProgress { app_ids }) => st.load_achievement_progress(app_ids, sink),
                Some(Command::LoadAchievements { app_id, language }) => st.load_achievements(app_id, language, sink),
                Some(Command::ActivateKey { key }) => st.activate_key(key, sink),
                Some(Command::Install(req)) => {
                    if storage_busy(storage, req.app_id) {
                        sink(Event::DownloadFinished { app_id: req.app_id, result: Err("Wait for the file operation to finish before updating this game.".into()) });
                    } else {
                        // A download being cancelled doesn't count: "Apply now" restarts it.
                        let running = st.active.as_ref().is_some_and(|(a, p)| *a == req.app_id && !p.cancel.load(std::sync::atomic::Ordering::Relaxed));
                        if !running && !st.queue.iter().any(|q| q.app_id == req.app_id) {
                            st.queue.push_back(req);
                        }
                        st.pump(sink, &dl_tx);
                        st.save_downloads();
                    }
                }
                Some(Command::PauseDownload { app_id }) => {
                    st.auto_paused = false;
                    st.with_progress(app_id, |p| p.paused.store(true, std::sync::atomic::Ordering::Relaxed));
                }
                Some(Command::ResumeDownload { app_id }) => st.with_progress(app_id, |p| p.paused.store(false, std::sync::atomic::Ordering::Relaxed)),
                Some(Command::SetAutoDownloads(on)) => {
                    st.auto_downloads = on;
                    // Turned back on: pick up anything saved and get it going
                    // (restore_downloads skips anything already in flight).
                    if on {
                        st.restore_downloads(sink, &dl_tx);
                    }
                }
                Some(Command::CancelDownload { app_id }) => {
                    st.queue.retain(|q| q.app_id != app_id);
                    st.with_progress(app_id, |p| p.cancel.store(true, std::sync::atomic::Ordering::Relaxed));
                    if st.active_req.as_ref().is_some_and(|r| r.app_id == app_id) {
                        st.active_req = None;
                    }
                    st.save_downloads();
                    sink(Event::DownloadQueue(st.queue.iter().map(|q| q.app_id).collect()));
                }
                Some(Command::ReorderDownloads(order)) => {
                    st.queue = reorder(std::mem::take(&mut st.queue), &order);
                    st.save_downloads();
                    sink(Event::DownloadQueue(st.queue.iter().map(|q| q.app_id).collect()));
                }
                Some(Command::DownloadLimits { limit_kbps, concurrency }) => {
                    st.limit_kbps = limit_kbps;
                    st.concurrency = concurrency.clamp(1, 64);
                    if let Some((_, p)) = &st.active {
                        p.limit_kbps.store(limit_kbps, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Some(Command::UnsubscribeWorkshop { app_id, file_id }) => st.unsubscribe(app_id, file_id, sink),
                Some(Command::WorkshopQuery { request_id, app_id, query_type, page, search, days, tags }) => {
                    st.workshop_query(request_id, app_id, query_type, page, search, days, tags, sink)
                }
                Some(Command::WorkshopSubscribe { app_id, file_id }) => st.workshop_subscribe(app_id, file_id, sink),
                Some(Command::LoadWorkshopTags { app_id, language }) => {
                    let sink = sink.clone();
                    tokio::spawn(async move {
                        let result = crate::social::workshop_tags(app_id, &language).await;
                        sink(Event::WorkshopTags { app_id, result });
                    });
                }
                Some(Command::LoadWishlist) => st.load_wishlist(sink),
                Some(Command::WishlistAdd { app_id }) => st.wishlist_change(app_id, true, sink),
                Some(Command::WishlistRemove { app_id }) => st.wishlist_change(app_id, false, sink),
                Some(Command::WishlistMove { app_id, to }) => st.wishlist_move(app_id, to, sink),
                Some(Command::LoadDiscoveryQueue { country, rebuild }) => st.load_discovery_queue(country, rebuild, sink),
                Some(Command::LoadPointsBalance) => {
                    let conn = conn.clone();
                    let refresh = st.refresh_token.clone();
                    let me = st.me;
                    let sink = sink.clone();
                    tokio::spawn(async move {
                        let res = fetch_points_balance(&conn, &refresh, me).await;
                        sink(Event::PointsBalance(res));
                    });
                }
                Some(Command::SkipDiscoveryQueueItem { app_id }) => {
                    let conn = conn.clone();
                    tokio::spawn(async move {
                        if let Err(e) = crate::discovery::skip(&conn, app_id).await {
                            tracing::warn!("discovery skip {app_id}: {e}");
                        }
                    });
                }
                Some(Command::IgnoreApp { app_id, ignore }) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::discovery::ignore(&w, app_id, ignore).await,
                        Err(e) => Err(e),
                    };
                    if let Err(e) = &result {
                        tracing::warn!("ignore app {app_id}: {e}");
                        sink(Event::StoreError(e.clone()));
                    }
                    sink(Event::AppIgnored { app_id, ignored: ignore, result });
                }),
                Some(Command::FollowApp { app_id, follow }) => st.with_web(sink, move |web, sink| async move {
                    let result = match web {
                        Ok(w) => crate::discovery::follow(&w, app_id, follow).await,
                        Err(e) => Err(e),
                    };
                    if let Err(e) = &result {
                        tracing::warn!("follow app {app_id}: {e}");
                        sink(Event::StoreError(e.clone()));
                    }
                    sink(Event::AppFollowed { app_id, result });
                }),
                Some(Command::LoadCart) => st.load_cart(sink),
                Some(Command::CartAdd { package_id, gift, gift_to }) => st.cart_add(package_id, gift, gift_to, sink),
                Some(Command::CartModify { line_item_id, gift, gift_to }) => st.cart_modify(line_item_id, gift, gift_to, sink),
                Some(Command::RequestFreeLicense { app_id }) => {
                    let (conn, sink) = (conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        let result = match conn.request(emsg::CLIENT_REQUEST_FREE_LICENSE, &CMsgClientRequestFreeLicense { appids: vec![app_id] }).await {
                            Ok(p) => match p.body::<CMsgClientRequestFreeLicenseResponse>() {
                                // Steam answers OK even when it grants nothing (not free here).
                                Ok(r) if r.eresult() == 1 && (r.granted_appids.contains(&app_id) || !r.granted_packageids.is_empty()) => Ok(()),
                                Ok(r) if r.eresult() == 1 => Err("Steam didn't add it: this game isn't free in your region, or you already have it.".to_string()),
                                Ok(r) => Err(format!("Steam refused (error {})", r.eresult())),
                                Err(e) => Err(e.to_string()),
                            },
                            Err(e) => Err(e.to_string()),
                        };
                        sink(Event::FreeLicense { app_id, result });
                    });
                }
                Some(Command::CartRemove { line_item_id }) => st.cart_remove(line_item_id, sink),
                Some(Command::Hub(req)) => {
                    let (conn, me) = (conn.clone(), st.me);
                    st.with_web(sink, move |web, sink| crate::hub::handle(conn, me, web, req, sink));
                }
                Some(Command::Typing { to }) => {
                    let conn = conn.clone();
                    tokio::spawn(async move {
                        let _: Result<CFriendMessagesSendMessageResponse, _> = conn
                            .call("FriendMessages.SendMessage#1", &CFriendMessagesSendMessageRequest {
                                steamid: Some(to), chat_entry_type: Some(2), ..Default::default()
                            })
                            .await;
                    });
                }
                Some(other) => {
                    if let Some(unhandled) = handle_storage_command(
                        other,
                        storage,
                        |id| st.running.contains_key(&id),
                        |id| st.active.as_ref().is_some_and(|(a, _)| *a == id) || st.queue.iter().any(|q| q.app_id == id),
                        |id| st.verifying.contains_key(&id),
                        sink,
                    ) {
                        tracing::debug!("ignored while signed in: {unhandled:?}");
                    }
                }
            },
            Some(apps) = apps_rx.recv() => {
                st.apps = apps.iter().map(|a| (a.app_id, a.clone())).collect();
                sink(Event::Apps(apps));
                // Installs queued while the saved library was showing go now.
                if !st.apps_fresh {
                    st.apps_fresh = true;
                    st.pump(sink, &dl_tx);
                }
                // Downloads that were running or queued when the app closed
                // carry on (files already on disk are checked and kept).
                if !st.downloads_restored {
                    st.downloads_restored = true;
                    st.restore_downloads(sink, &dl_tx);
                }
                st.cache_dirty = true;
                st.save_cache();
                // Saves changed offline go up once the library has sent its
                // cloud settings for these games (a moment later).
                let tx = flush_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    let _ = tx.send(());
                });
            }
            Some(()) = flush_rx.recv() => st.flush_cloud(sink),
            Some((app_id, result)) = dl_rx.recv() => {
                st.active = None;
                st.active_req = None;
                st.auto_paused = false;
                sink(Event::DownloadFinished { app_id, result });
                st.pump(sink, &dl_tx);
                st.save_downloads();
            }
            Some((app_id, dir, args)) = launch_rx.recv() => launch_now!(app_id, dir, args),
            Some((app_id, dir, args)) = sh_rx.recv() => {
                st.shader_jobs.remove(&app_id);
                let _ = st.launch(app_id, &dir, &args, sink, &exit_tx);
            }
            Some((app_id, dir, args)) = ws_rx.recv() => launch_cloud!(app_id, dir, args),
            Some((app_id, report)) = vf_rx.recv() => {
                st.verifying.remove(&app_id);
                sink(Event::VerifyFinished { app_id, result: Ok(report) });
            }
            Some(call) = ipc_rx.recv() => st.ipc_call(call, sink),
            Some((app_id, host_cloud_dir)) = exit_rx.recv() => {
                st.running.remove(&app_id);
                st.stop_flags.remove(&app_id);
                st.launch_layouts.remove(&app_id);
                st.official.remove(&app_id);
                if st.pad_users.remove(&app_id) {
                    crate::pads::stop();
                }
                if let Some(include_xbox) = st.vpad_users.remove(&app_id) {
                    crate::pads::stop_virtual_pads(include_xbox);
                }
                if let Some(token) = st.launch_tokens.remove(&app_id) {
                    launch::swap_exited(&token);
                    st.ipc.forget(&token);
                    crate::ipc::game_closed(&conn, &st.auth, app_id);
                }
                // Played while not connected: keep the time here (Steam never saw it).
                if let Some((start, offline_mode)) = st.unconnected_play.remove(&app_id) {
                    offline::record_play(st.me, offline::PlaySession { app_id, start, seconds: unix_now().saturating_sub(start), offline: offline_mode, reported: false });
                    st.send_offline_lists(sink);
                }
                // Like Steam: send changed saves up after the game closes,
                // or remember to once back online.
                if let Some(dir) = st.launched_dirs.remove(&app_id).or(host_cloud_dir) {
                    if let Some(target) = st.cloud_target(app_id, &dir) {
                        if st.link == Link::Online {
                            st.upload_saves(app_id, target, sink, false);
                        } else {
                            offline::add_cloud_pending(st.me, app_id, &dir);
                            st.send_offline_lists(sink);
                        }
                    }
                }
                if st.playing == Some(app_id) {
                    st.playing = None;
                    let _ = games_played(&conn, None);
                }
                sink(Event::GameStopped { app_id });
                if st.drm_relaunch.remove(&app_id) {
                    sink(Event::RelaunchForSteamDrm { app_id });
                }
            }
            _ = sleep_until(retry_at), if retry_at.is_some() && !connecting => {
                retry_at = None;
                connecting = true;
                let attempt = match st.link {
                    Link::Reconnecting { attempt, .. } => attempt,
                    _ => 1,
                };
                st.set_link(Link::Reconnecting { attempt, retry_in: 0 }, sink);
                let mut acc = store::find(&st.account).unwrap_or_else(|| account.clone());
                acc.refresh_token = st.refresh_token.clone();
                let tx = rc_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(reconnect(acc).await);
                });
            }
            Some(result) = rc_rx.recv() => {
                connecting = false;
                match result {
                    // "Go offline" was picked while this attempt ran.
                    Ok((c, _, _)) if !st.want_online => {
                        let _ = c.send(emsg::CLIENT_LOG_OFF, &CMsgClientLogOff {});
                    }
                    Ok((c, e, info)) => {
                        events = e;
                        quiet_tx = None;
                        hb = tokio::time::interval(Duration::from_secs(info.heartbeat.clamp(5, 60) as u64));
                        hb.tick().await;
                        st.reconnected(c, info, sink);
                        st.pump(sink, &dl_tx);
                    }
                    Err(e) if is_transient(&e) => {
                        tracing::info!("still can't reach Steam: {e}");
                        match st.link {
                            // A "Go online" from offline mode that didn't work: stay offline.
                            Link::Reconnecting { .. } if st.from_offline => {
                                st.want_online = false;
                                st.from_offline = false;
                                st.set_link(Link::Offline, sink);
                                sink(Event::Error(format!("Couldn't connect to Steam: {e}")));
                            }
                            Link::Reconnecting { attempt, .. } => {
                                let next = attempt + 1;
                                let wait = backoff(next);
                                st.set_link(Link::Reconnecting { attempt: next, retry_in: wait }, sink);
                                retry_at = Some(tokio::time::Instant::now() + Duration::from_secs(wait as u64));
                            }
                            _ => {}
                        }
                    }
                    // The saved login doesn't work any more: sign in again.
                    Err(e) => return Ok(e.to_string()),
                }
            }
            Some((generation, dead)) = probe_rx.recv() => {
                if generation == st.generation {
                    st.probing = false;
                    if dead && st.link == Link::Online {
                        lost!("Steam stopped answering");
                    } else {
                        st.last_rx = std::time::Instant::now();
                    }
                }
            }
            _ = hb.tick() => {
                if st.link == Link::Online {
                    if conn.send(emsg::CLIENT_HEARTBEAT, &CMsgClientHeartBeat { send_reply: Some(false) }).is_err() {
                        lost!("the connection is closed");
                    } else if st.last_rx.elapsed() > Duration::from_secs(60) && !st.probing {
                        // Quiet for a minute: make sure the connection still works.
                        st.probing = true;
                        let (c, tx, generation) = (conn.clone(), probe_tx.clone(), st.generation);
                        tokio::spawn(async move {
                            let r = c.call::<_, CPlayerGetGameBadgeLevelsResponse>("Player.GetGameBadgeLevels#1", &CPlayerGetGameBadgeLevelsRequest { appid: Some(0) }).await;
                            let _ = tx.send((generation, matches!(r, Err(net::Error::Timeout | net::Error::Disconnected))));
                        });
                    }
                }
                if st.cache_dirty && st.cache_saved.elapsed() > Duration::from_secs(60) {
                    st.save_cache();
                }
            }
        }
        let _ = &quiet_tx;
    }
}

/// Wait until `at` (forever when `None`; the branch is disabled then anyway).
async fn sleep_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

fn set_persona(conn: &Connection, s: PersonaState) -> Result<(), Error> {
    conn.send(
        emsg::CLIENT_CHANGE_STATUS,
        &CMsgClientChangeStatus {
            persona_state: Some(s.as_u32()),
            persona_set_by_user: Some(true),
            ..Default::default()
        },
    )?;
    Ok(())
}

/// `ClientGamesPlayedWithDataBlob`: the message Steam's client sends. With the
/// older `ClientGamesPlayed` (742) Steam marks you "In-Game" but shows no game.
const CLIENT_GAMES_PLAYED_WITH_DATA_BLOB: u32 = 5410;

fn games_played(conn: &Connection, app: Option<AppId>) -> Result<(), Error> {
    conn.send(
        CLIENT_GAMES_PLAYED_WITH_DATA_BLOB,
        &CMsgClientGamesPlayed {
            games_played: app
                .map(|id| c_msg_client_games_played::GamePlayed {
                    game_id: Some(id as u64),
                    ..Default::default()
                })
                .into_iter()
                .collect(),
            client_os_type: Some(OS_TYPE_WIN11),
            ..Default::default()
        },
    )?;
    Ok(())
}

fn is_individual(id: SteamId) -> bool {
    (id >> 52) & 0xF == 1
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Online {
    /// Returns `Some` when the server ends the session.
    fn handle(
        &mut self,
        p: Packet,
        sink: &Sink,
        apps_tx: &mpsc::UnboundedSender<Vec<OwnedApp>>,
    ) -> Result<Option<Ended>, Error> {
        match p.emsg {
            emsg::CLIENT_LICENSE_LIST => {
                let list: CMsgClientLicenseList = p.body()?;
                let licenses: Vec<(u32, u64)> = list
                    .licenses
                    .iter()
                    .map(|l| (l.package_id(), l.access_token()))
                    .collect();
                sink(Event::Licenses { count: licenses.len() });
                let (conn, tx, sink, me) = (self.conn.clone(), apps_tx.clone(), sink.clone(), self.me);
                tokio::spawn(async move {
                    match pics::resolve(&conn, &licenses, me).await {
                        Ok(apps) => {
                            let _ = tx.send(apps);
                        }
                        Err(e) => sink(Event::Error(format!("Could not load library: {e}"))),
                    }
                });
            }
            emsg::CLIENT_FRIENDS_LIST => {
                let list: CMsgClientFriendsList = p.body()?;
                if !list.bincremental() {
                    self.friends.clear();
                }
                let mut new_ids = Vec::new();
                for f in &list.friends {
                    let id = f.ulfriendid();
                    if !is_individual(id) {
                        continue;
                    }
                    let rel = Relationship::from_u32(f.efriendrelationship());
                    if rel == Relationship::None {
                        self.friends.remove(&id);
                    } else {
                        // Unknown, or known without a name (e.g. from the offline cache).
                        if self.personas.get(&id).is_none_or(|p| p.name.is_empty()) {
                            new_ids.push(id);
                        }
                        self.friends.insert(id, rel);
                    }
                }
                sink(Event::Friends(self.friends.iter().map(|(k, v)| (*k, *v)).collect()));
                self.cache_dirty = true;
                if !new_ids.is_empty() {
                    self.conn.send(
                        emsg::CLIENT_REQUEST_FRIEND_DATA,
                        &CMsgClientRequestFriendData {
                            persona_state_requested: Some(PERSONA_FLAGS),
                            friends: new_ids,
                        },
                    )?;
                }
            }
            emsg::CLIENT_PLAYER_NICKNAME_LIST => {
                let list: CMsgClientPlayerNicknameList = p.body()?;
                let removal = list.removal();
                if !list.incremental() && !removal {
                    self.nicknames.clear();
                }
                let mut changed = Vec::new();
                for n in list.nicknames {
                    let id = n.steamid();
                    if removal || n.nickname().is_empty() {
                        self.nicknames.remove(&id);
                    } else {
                        self.nicknames.insert(id, n.nickname().to_string());
                    }
                    let entry = self.personas.entry(id).or_insert_with(|| Persona { steam_id: id, ..Default::default() });
                    entry.nickname = self.nicknames.get(&id).cloned();
                    changed.push(entry.clone());
                }
                sink(Event::Personas(changed));
            }
            emsg::CLIENT_PERSONA_STATE => {
                let msg: CMsgClientPersonaState = p.body()?;
                let mut changed = Vec::new();
                let mut unknown_games = Vec::new();
                for f in msg.friends {
                    let id = f.friendid();
                    let entry = self
                        .personas
                        .entry(id)
                        .or_insert_with(|| Persona { steam_id: id, ..Default::default() });
                    if let Some(n) = f.player_name {
                        entry.name = n;
                    }
                    if let Some(s) = f.persona_state {
                        entry.state = PersonaState::from_u32(s);
                    }
                    if let Some(g) = f.game_played_app_id {
                        entry.game_app_id = (g != 0).then_some(g);
                    }
                    // Some games only fill the 64-bit game id (app id in the low 24 bits).
                    if entry.game_app_id.is_none() {
                        if let Some(gid) = f.gameid.filter(|g| *g != 0) {
                            let app = (gid & 0xFF_FFFF) as AppId;
                            entry.game_app_id = (app != 0 && (gid >> 56) == 0).then_some(app);
                        }
                    }
                    if f.persona_state.is_some() || f.game_played_app_id.is_some() {
                        let status = f.rich_presence.iter().find(|kv| kv.key() == "status").map(|kv| kv.value().trim().to_string());
                        entry.rich_status = status.filter(|s| !s.is_empty() && !s.starts_with('#'));
                        // How to join them: server, lobby, or the game's own "connect" line.
                        entry.connect = f.rich_presence.iter().find(|kv| kv.key() == "connect").map(|kv| kv.value().trim().to_string()).filter(|c| !c.is_empty());
                        entry.server = match (f.game_server_ip.unwrap_or(0), f.game_server_port.unwrap_or(0)) {
                            (ip, port) if ip != 0 && port != 0 => Some(format!("{}:{port}", std::net::Ipv4Addr::from(ip))),
                            _ => None,
                        };
                        entry.lobby = f.game_lobby_id.filter(|l| *l != 0);
                        entry.broadcasting = f.broadcast_id.is_some_and(|b| b != 0);
                    }
                    if let Some(nick) = self.nicknames.get(&id) {
                        entry.nickname = Some(nick.clone());
                    }
                    if let Some(app) = entry.game_app_id {
                        if !self.apps.contains_key(&app) && self.names_asked.insert(app) {
                            unknown_games.push(app);
                        }
                    }
                    if let Some(n) = f.game_name {
                        entry.game_name = (!n.is_empty()).then_some(n);
                    }
                    if let Some(h) = f.avatar_hash {
                        entry.avatar_hash = Some(hex(&h));
                    }
                    if let Some(t) = f.last_logoff {
                        entry.last_logoff = Some(t as u64);
                    }
                    if id == self.me {
                        let (name, avatar) = (entry.name.clone(), entry.avatar_hash.clone());
                        store::update(&self.account, |a| {
                            if !name.is_empty() {
                                a.persona_name = name;
                            }
                            a.avatar_hash = avatar.or(a.avatar_hash.take());
                        });
                    }
                    changed.push(entry.clone());
                }
                sink(Event::Personas(changed));
                self.cache_dirty = true;
                // Names for games friends play that aren't in our library.
                if !unknown_games.is_empty() {
                    let (conn, sink) = (self.conn.clone(), sink.clone());
                    tokio::spawn(async move {
                        if let Ok(names) = pics::names(&conn, &unknown_games).await {
                            sink(Event::AppNames(names));
                        }
                    });
                }
            }
            crate::ipc::EMSG_MMS_LOBBY_DATA
            | crate::ipc::EMSG_MMS_LOBBY_CHAT
            | crate::ipc::EMSG_MMS_USER_JOINED_LOBBY
            | crate::ipc::EMSG_MMS_USER_LEFT_LOBBY
            | crate::ipc::EMSG_MMS_LOBBY_GAME_SERVER_SET
            | crate::ipc::EMSG_P2P_CONNECTION_INFO => {
                // Lobby changes, lobby chat and P2P signals for running games.
                if let Some((app_id, event)) = crate::ipc::game_event(&p) {
                    self.ipc.push(app_id, event);
                }
            }
            crate::ipc::EMSG_GAME_CONNECT_TOKENS => {
                let m: CMsgClientGameConnectTokens = p.body()?;
                self.auth.lock().unwrap().add_tokens(&m);
            }
            crate::ipc::EMSG_AUTH_LIST_ACK => {
                let m: CMsgClientAuthListAck = p.body()?;
                self.auth.lock().unwrap().on_ack(&m);
            }
            crate::ipc::EMSG_TICKET_AUTH_COMPLETE => {
                let m: CMsgClientTicketAuthComplete = p.body()?;
                self.auth.lock().unwrap().on_auth_complete(&m);
            }
            emsg::CLIENT_ACCOUNT_INFO => {
                let info: CMsgClientAccountInfo = p.body()?;
                let name = info.persona_name().to_string();
                store::update(&self.account, |a| a.persona_name = name.clone());
                self.country = info.ip_country().to_string();
                self.cache_dirty = true;
                sink(Event::AccountInfo { persona_name: name, country: info.ip_country().into() });
                // Ask for our own persona so the avatar shows up.
                self.conn.send(
                    emsg::CLIENT_REQUEST_FRIEND_DATA,
                    &CMsgClientRequestFriendData {
                        persona_state_requested: Some(PERSONA_FLAGS),
                        friends: vec![self.me],
                    },
                )?;
            }
            emsg::SERVICE_METHOD => {
                let name = p.header.target_job_name().to_string();
                if name.starts_with("SteamNotificationClient.NotificationsReceived") {
                    let n: proto::webui::CSteamNotificationNotificationsReceivedNotification = p.body()?;
                    let items: Vec<model::Notification> = n.notifications.iter().map(crate::notify::convert).collect();
                    if !items.is_empty() {
                        sink(Event::NotificationsArrived(items));
                    }
                    self.load_notifications(sink);
                } else if name.starts_with("FriendMessagesClient.IncomingMessage") {
                    let n: CFriendMessagesIncomingMessageNotification = p.body()?;
                    match n.chat_entry_type() {
                        1 => {
                            // The markup keeps invites (game, lobby, group); the plain text loses them.
                            let parsed = model::chat::parse_message(n.message());
                            let text = if parsed.text.is_empty() && parsed.invite.is_none() && parsed.group_invite.is_none() && parsed.media.is_empty() {
                                n.message_no_bbcode().to_string()
                            } else {
                                parsed.text
                            };
                            sink(Event::Message(model::ChatMessage {
                                friend: n.steamid_friend(),
                                from: if n.local_echo() { self.me } else { n.steamid_friend() },
                                text,
                                time: n.rtime32_server_timestamp() as u64,
                                invite: parsed.invite,
                                group_invite: parsed.group_invite,
                                media: parsed.media,
                            }))
                        }
                        2 => sink(Event::Typing { from: n.steamid_friend() }),
                        _ => {}
                    }
                } else if name.starts_with("ChatRoomClient.") {
                    crate::chat_groups::handle_notification(&self.conn, self.me, &self.active_groups, &name, &p, sink)?;
                } else {
                    sink(Event::Unhandled { emsg: p.emsg, name });
                }
            }
            emsg::CLIENT_EMAIL_ADDR_INFO => {
                let e: CMsgClientEmailAddrInfo = p.body()?;
                sink(Event::Email { address: e.email_address().to_string(), validated: e.email_is_validated() });
            }
            emsg::CLIENT_WALLET_INFO_UPDATE => {
                let w: CMsgClientWalletInfoUpdate = p.body()?;
                if w.has_wallet() {
                    let balance = w.balance64.unwrap_or(w.balance() as i64);
                    let pending = w.balance64_delayed.unwrap_or(w.balance_delayed() as i64);
                    self.wallet_currency = Some(w.currency());
                    sink(Event::Wallet { balance, currency: w.currency(), pending });
                }
            }
            emsg::CLIENT_LOGGED_OFF => {
                let off: CMsgClientLoggedOff = p.body()?;
                // Maintenance or a busy server: reconnect instead of signing out.
                if matches!(off.eresult(), 3 | 16 | eresult::SERVICE_UNAVAILABLE | eresult::TRY_ANOTHER_CM) {
                    return Ok(Some(Ended::Dropped));
                }
                return Ok(Some(Ended::Logout(eresult::describe(off.eresult(), ""))));
            }
            other => sink(Event::Unhandled { emsg: other, name: emsg::name(other) }),
        }
        Ok(None)
    }

    fn send_message(&self, to: SteamId, text: String, sink: &Sink) {
        let (conn, sink, me) = (self.conn.clone(), sink.clone(), self.me);
        tokio::spawn(async move {
            let r: Result<CFriendMessagesSendMessageResponse, _> = conn
                .call(
                    "FriendMessages.SendMessage#1",
                    &CFriendMessagesSendMessageRequest {
                        steamid: Some(to),
                        chat_entry_type: Some(1),
                        message: Some(text.clone()),
                        contains_bbcode: Some(false),
                        ..Default::default()
                    },
                )
                .await;
            match r {
                Ok(r) => {
                    let parsed = model::chat::parse_message(if r.modified_message().is_empty() { &text } else { r.modified_message() });
                    sink(Event::Message(model::ChatMessage {
                        friend: to,
                        from: me,
                        text: parsed.text,
                        time: r.server_timestamp() as u64,
                        invite: parsed.invite,
                        group_invite: parsed.group_invite,
                        media: parsed.media,
                    }))
                }
                Err(e) => sink(Event::Error(format!("Message not sent: {e}"))),
            }
        });
    }

    fn with_progress(&self, app_id: AppId, f: impl FnOnce(&depot::Progress)) {
        if let Some((id, p)) = &self.active {
            if *id == app_id {
                f(p);
            }
        }
    }

    /// Start the next queued download if nothing is running.
    fn pump(&mut self, sink: &Sink, done: &mpsc::UnboundedSender<(AppId, Result<String, String>)>) {
        // The queue is reported after the next game leaves it, so the one
        // that's downloading isn't also listed as "queued".
        if self.active.is_some() || self.link != Link::Online || !self.apps_fresh {
            sink(Event::DownloadQueue(self.queue.iter().map(|q| q.app_id).collect()));
            return;
        }
        let next = self.queue.pop_front();
        sink(Event::DownloadQueue(self.queue.iter().map(|q| q.app_id).collect()));
        let Some(req) = next else { return };
        let app_id = req.app_id;
        let Some(app) = self.apps.get(&app_id).cloned() else {
            let _ = done.send((app_id, Err("Your library is still loading. Try again in a moment.".into())));
            return;
        };
        self.active_req = Some(req.clone());
        let dest = req.library.join("steamapps").join("common").join(if app.install_dir.is_empty() { &app.name } else { &app.install_dir });
        let progress = std::sync::Arc::new(depot::Progress::default());
        progress.limit_kbps.store(self.limit_kbps, std::sync::atomic::Ordering::Relaxed);
        progress.concurrency.store(self.concurrency, std::sync::atomic::Ordering::Relaxed);
        self.active = Some((app_id, progress.clone()));
        let owned: std::collections::HashSet<AppId> = self.apps.keys().copied().collect();
        let (conn, cell, me, done, sink2) = (self.conn.clone(), self.download_cell.unwrap_or(self.cell_id), self.me, done.clone(), sink.clone());
        let host = self.download_host.clone();

        // Progress ticker: speed over a sliding window, ~2 updates per second.
        let (p2, sink3) = (progress.clone(), sink.clone());
        tokio::spawn(async move {
            // (time, bytes from the network, bytes on disk)
            let mut samples: std::collections::VecDeque<(std::time::Instant, u64, u64)> = Default::default();
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let now = std::time::Instant::now();
                let net = p2.downloaded.load(std::sync::atomic::Ordering::Relaxed);
                // Real writes only: files found complete aren't disk activity.
                let disk = p2.written.load(std::sync::atomic::Ordering::Relaxed);
                samples.push_back((now, net, disk));
                while samples.len() > 10 {
                    samples.pop_front();
                }
                let (speed, disk_speed) = match (samples.front(), samples.back()) {
                    (Some(a), Some(b)) if b.0 > a.0 => {
                        let secs = (b.0 - a.0).as_secs_f64();
                        (((b.1 - a.1) as f64 / secs) as u64, (b.2.saturating_sub(a.2) as f64 / secs) as u64)
                    }
                    _ => (0, 0),
                };
                // Checked before sending: once a download has ended (failed
                // included), a late "Downloading" tick must not cover the result.
                if p2.ended.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let mut status = crate::download::status(app_id, &p2, speed);
                status.install_dir = Some(dest.clone());
                status.disk_speed = disk_speed;
                sink3(Event::Download(status));
                if p2.phase() == depot::Phase::Done || p2.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            let ended = progress.clone();
            let result = crate::download::run(conn, app, owned, req, cell, host, me, progress).await;
            ended.ended.store(true, std::sync::atomic::Ordering::Relaxed);
            let msg = result.map(|r| {
                format!(
                    "{} MB downloaded, {} MB reused, {} files unchanged",
                    r.downloaded_bytes / 1_048_576,
                    r.reused_bytes / 1_048_576,
                    r.skipped
                )
            });
            if let Err(e) = &msg {
                tracing::warn!("download {app_id} failed: {e}");
            }
            let _ = sink2;
            let _ = done.send((app_id, msg));
        });
    }

    /// Unfinished downloads (running first, then the queue), per account.
    fn downloads_path(&self) -> std::path::PathBuf {
        store::config_dir().join(format!("downloads-{}.json", self.me))
    }

    fn save_downloads(&self) {
        let list: Vec<&crate::download::InstallRequest> = self.active_req.iter().chain(self.queue.iter()).collect();
        let path = self.downloads_path();
        if list.is_empty() {
            let _ = std::fs::remove_file(&path);
        } else if let Ok(text) = serde_json::to_string_pretty(&list) {
            if let Err(e) = store::write_private(&path, text.as_bytes()) {
                tracing::warn!("couldn't save {}: {e}", path.display());
            }
        }
    }

    fn restore_downloads(&mut self, sink: &Sink, done: &mpsc::UnboundedSender<(AppId, Result<String, String>)>) {
        // With automatic downloads off, don't resume by ourselves on startup;
        // the saved list stays on disk and comes back when they're turned on
        // again (or the player starts the game).
        if !self.auto_downloads {
            tracing::info!("automatic downloads are off; not resuming saved downloads");
            return;
        }
        let Ok(text) = std::fs::read_to_string(self.downloads_path()) else { return };
        let Ok(list) = serde_json::from_str::<Vec<crate::download::InstallRequest>>(&text) else { return };
        for req in list {
            let busy = self.active.as_ref().is_some_and(|a| a.0 == req.app_id) || self.queue.iter().any(|q| q.app_id == req.app_id);
            if !busy && self.apps.contains_key(&req.app_id) {
                tracing::info!("resuming the download of {}", req.app_id);
                self.queue.push_back(req);
            }
        }
        self.pump(sink, done);
        self.save_downloads();
    }

    fn unsubscribe(&self, app_id: AppId, file_id: u64, sink: &Sink) {
        let (conn, sink) = (self.conn.clone(), sink.clone());
        tokio::spawn(async move {
            let r: Result<CPublishedFileUnsubscribeResponse, _> = conn
                .call(
                    "PublishedFile.Unsubscribe#1",
                    &CPublishedFileUnsubscribeRequest { publishedfileid: Some(file_id), list_type: Some(1), appid: Some(app_id as i32), notify_client: Some(true) },
                )
                .await;
            sink(Event::WorkshopUnsubscribed { file_id, result: r.map(|_| ()).map_err(|e| e.to_string()) });
        });
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn workshop_query(&self, request_id: u64, app_id: AppId, query_type: u32, page: u32, search: String, days: u32, tags: Vec<String>, sink: &Sink) {
        let (conn, sink) = (self.conn.clone(), sink.clone());
        tokio::spawn(async move {
            let req = CPublishedFileQueryFilesRequest {
                query_type: Some(if search.trim().is_empty() { query_type } else { 12 }),
                page: Some(page.max(1)),
                numperpage: Some(30),
                creator_appid: Some(app_id),
                appid: Some(app_id),
                search_text: Some(search).filter(|s| !s.trim().is_empty()),
                days: Some(days).filter(|d| *d > 0),
                return_previews: Some(true),
                return_short_description: Some(true),
                return_vote_data: Some(true),
                return_tags: Some(true),
                requiredtags: tags,
                match_all_tags: Some(true),
                ..Default::default()
            };
            match conn.call::<_, CPublishedFileQueryFilesResponse>("PublishedFile.QueryFiles#1", &req).await {
                Ok(r) => {
                    let items = r
                        .publishedfiledetails
                        .iter()
                        .map(|d| model::WorkshopEntry {
                            id: d.publishedfileid(),
                            title: d.title().to_string(),
                            preview: Some(d.preview_url().to_string()).filter(|p| !p.is_empty()),
                            description: d.short_description().to_string(),
                            size: d.file_size(),
                            subscriptions: d.subscriptions() as u64,
                            favorited: d.favorited() as u64,
                            updated: d.time_updated() as u64,
                            votes_up: d.vote_data.as_ref().map(|v| v.votes_up()).unwrap_or(0),
                            votes_down: d.vote_data.as_ref().map(|v| v.votes_down()).unwrap_or(0),
                            tags: d.tags.iter().filter(|t| !t.adminonly()).map(|t| t.tag().to_string()).filter(|t| !t.is_empty()).collect(),
                        })
                        .collect();
                    sink(Event::WorkshopResults { request_id, total: r.total() as u64, items });
                }
                Err(e) => sink(Event::StoreError(e.to_string())),
            }
        });
    }

    fn workshop_options(&self) -> crate::workshop::SyncOptions {
        crate::workshop::SyncOptions {
            concurrency: self.concurrency,
            limit_kbps: self.limit_kbps,
            preferred_host: self.download_host.clone(),
        }
    }

    fn workshop_subscribe(&self, app_id: AppId, file_id: u64, sink: &Sink) {
        let (conn, sink, me, cell, root, opts) = (
            self.conn.clone(),
            sink.clone(),
            self.me,
            self.download_cell.unwrap_or(self.cell_id),
            self.steam_root.clone(),
            self.workshop_options(),
        );
        tokio::spawn(async move {
            let r: Result<CPublishedFileSubscribeResponse, _> = conn
                .call(
                    "PublishedFile.Subscribe#1",
                    &CPublishedFileSubscribeRequest { publishedfileid: Some(file_id), list_type: Some(1), appid: Some(app_id as i32), notify_client: Some(true), include_dependencies: Some(true) },
                )
                .await;
            let ok = r.is_ok();
            sink(Event::WorkshopSubscribed { file_id, result: r.map(|_| ()).map_err(|e| e.to_string()) });
            // Like Steam: an installed game gets the item right away.
            if ok {
                sync_installed(conn, me, cell, root, app_id, vec![file_id], opts, sink).await;
            }
        });
    }

    /// Download `items` (or every subscribed item) for an installed game.
    fn sync_workshop(&self, app_id: AppId, items: Vec<u64>, sink: &Sink) {
        let (conn, sink, me, cell, root, opts) = (
            self.conn.clone(),
            sink.clone(),
            self.me,
            self.download_cell.unwrap_or(self.cell_id),
            self.steam_root.clone(),
            self.workshop_options(),
        );
        tokio::spawn(sync_installed(conn, me, cell, root, app_id, items, opts, sink));
    }

    fn load_discovery_queue(&self, country: String, rebuild: bool, sink: &Sink) {
        let (conn, sink, me) = (self.conn.clone(), sink.clone(), self.me);
        tokio::spawn(async move {
            let result = crate::discovery::queue(&conn, &country, rebuild).await;
            sink(Event::DiscoveryQueue(result));
            if let Ok(followed) = crate::discovery::followed(&conn, me).await {
                sink(Event::FollowedApps(followed));
            }
        });
    }

    fn load_wishlist(&self, sink: &Sink) {
        let (conn, sink, me) = (self.conn.clone(), sink.clone(), self.me);
        tokio::spawn(async move {
            use proto::webui::*;
            match conn.call::<_, CWishlistGetWishlistResponse>("Wishlist.GetWishlist#1", &CWishlistGetWishlistRequest { steamid: Some(me) }).await {
                Ok(r) => sink(Event::Wishlist(wishlist_in_order(&r.items))),
                Err(e) => sink(Event::StoreError(e.to_string())),
            }
        });
    }

    /// Add to or remove from the wishlist the way the store page does (Steam
    /// can refuse the wishlist service from a client), with the service as a
    /// fallback. Steam's list is sent again afterwards either way.
    fn wishlist_change(&self, app_id: AppId, add: bool, sink: &Sink) {
        sink(Event::WishlistChanging { app_id, add });
        let (conn, me) = (self.conn.clone(), self.me);
        self.with_web(sink, move |web, sink| async move {
            use proto::webui::*;
            let via_web = match &web {
                Ok(w) => {
                    let url = format!("https://store.steampowered.com/api/{}", if add { "addtowishlist" } else { "removefromwishlist" });
                    match w.http.post(url).form(&[("sessionid", w.session_id.clone()), ("appid", app_id.to_string())]).send().await {
                        Ok(r) => r.json::<serde_json::Value>().await.ok().and_then(|v| v.get("success").and_then(|s| s.as_bool().or_else(|| s.as_i64().map(|n| n == 1)))).unwrap_or(false),
                        Err(_) => false,
                    }
                }
                Err(_) => false,
            };
            if !via_web {
                let r = if add {
                    conn.call::<_, CWishlistAddToWishlistResponse>("Wishlist.AddToWishlist#1", &CWishlistAddToWishlistRequest { appid: Some(app_id), navdata: None }).await.map(|_| ())
                } else {
                    conn.call::<_, CWishlistRemoveFromWishlistResponse>("Wishlist.RemoveFromWishlist#1", &CWishlistRemoveFromWishlistRequest { appid: Some(app_id) }).await.map(|_| ())
                };
                if let Err(e) = r {
                    tracing::warn!("wishlist {}: {e}", if add { "add" } else { "remove" });
                    sink(Event::StoreError(if add {
                        "Couldn't add it to your wishlist. Try again in a moment.".into()
                    } else {
                        "Couldn't remove it from your wishlist. Try again in a moment.".into()
                    }));
                }
            }
            if let Ok(r) = conn.call::<_, CWishlistGetWishlistResponse>("Wishlist.GetWishlist#1", &CWishlistGetWishlistRequest { steamid: Some(me) }).await {
                sink(Event::Wishlist(wishlist_in_order(&r.items)));
            }
        });
    }

    fn wishlist_move(&self, app_id: AppId, to: usize, sink: &Sink) {
        let (conn, me) = (self.conn.clone(), self.me);
        self.with_web(sink, move |web, sink| async move {
            use proto::webui::*;
            let current = match conn.call::<_, CWishlistGetWishlistResponse>("Wishlist.GetWishlist#1", &CWishlistGetWishlistRequest { steamid: Some(me) }).await {
                Ok(r) => r.items,
                Err(e) => {
                    sink(Event::StoreError(e.to_string()));
                    return;
                }
            };
            let ranked: Vec<crate::wishlist::Ranked> =
                current.iter().map(|i| crate::wishlist::Ranked { app_id: i.appid(), priority: i.priority(), added: i.date_added() as u64 }).collect();
            let updates = crate::wishlist::move_updates(&ranked, app_id, to);
            let save = if updates.is_empty() {
                Ok(())
            } else {
                match &web {
                    Ok(w) => crate::wishlist::reorder(w, &updates).await,
                    Err(e) => Err(e.clone()),
                }
            };
            if let Err(e) = save {
                tracing::warn!("wishlist move {app_id} -> {to}: {e}");
                sink(Event::StoreError("Couldn't save your wishlist's order. Try again in a moment.".into()));
                sink(Event::Wishlist(wishlist_in_order(&current)));
                return;
            }
            match conn.call::<_, CWishlistGetWishlistResponse>("Wishlist.GetWishlist#1", &CWishlistGetWishlistRequest { steamid: Some(me) }).await {
                Ok(r) => sink(Event::Wishlist(wishlist_in_order(&r.items))),
                Err(_) => sink(Event::Wishlist(wishlist_in_order(&current))),
            }
        });
    }

    /// The country the cart prices in: the wallet's (like Steam's client),
    /// else where Steam sees us. Steam needs it to add anything.
    fn cart_country(&self) -> Option<String> {
        model::store_country(self.wallet_currency, &self.country).map(|c| c.to_uppercase())
    }

    fn load_cart(&self, sink: &Sink) {
        let (conn, sink, country) = (self.conn.clone(), sink.clone(), self.cart_country());
        tokio::spawn(async move {
            use proto::webui::*;
            match conn.call::<_, CAccountCartGetCartResponse>("AccountCart.GetCart#1", &CAccountCartGetCartRequest { user_country: country }).await {
                Ok(r) => sink(Event::Cart(cart_lines(r.cart))),
                Err(e) => sink(Event::StoreError(e.to_string())),
            }
        });
    }

    fn cart_add(&self, package_id: u32, gift: bool, gift_to: Option<SteamId>, sink: &Sink) {
        let (conn, sink, country) = (self.conn.clone(), sink.clone(), self.cart_country());
        tokio::spawn(async move {
            use proto::webui::*;
            let gift = gift || gift_to.is_some();
            let item = CAccountCartAddItemsToCartRequestItemToAdd {
                packageid: Some(package_id),
                gift_info: gift_to.map(|id| CartGiftInfo { accountid_giftee: Some(model::account_id(id) as i32), ..Default::default() }),
                flags: Some(AccountCartLineItemFlags { is_gift: Some(gift), is_private: None }),
                ..Default::default()
            };
            let req = CAccountCartAddItemsToCartRequest { user_country: country, items: vec![item], ..Default::default() };
            match conn.call::<_, CAccountCartAddItemsToCartResponse>("AccountCart.AddItemsToCart#1", &req).await {
                Ok(r) => {
                    let lines = cart_lines(r.cart);
                    // Steam can answer "OK" without adding anything: check.
                    let added = !r.line_item_ids.is_empty() && lines.iter().any(|l| r.line_item_ids.contains(&l.line_item_id));
                    if !added {
                        tracing::warn!("cart: package {package_id} not added (line items {:?})", r.line_item_ids);
                        sink(Event::StoreError("Steam didn't add it to your cart. It may not be sold in your region, or gifting it isn't allowed.".into()));
                    }
                    sink(Event::Cart(lines));
                }
                Err(e) => sink(Event::StoreError(format!("Couldn't add it to your cart: {e}"))),
            }
        });
    }

    fn cart_modify(&self, line_item_id: u64, gift: bool, gift_to: Option<SteamId>, sink: &Sink) {
        let (conn, sink, country) = (self.conn.clone(), sink.clone(), self.cart_country());
        tokio::spawn(async move {
            use proto::webui::*;
            let gift = gift || gift_to.is_some();
            let req = CAccountCartModifyLineItemRequest {
                line_item_id: Some(line_item_id),
                user_country: country,
                // No recipient (or "For my account") clears it.
                gift_info: Some(CartGiftInfo { accountid_giftee: Some(gift_to.map(|id| model::account_id(id) as i32).unwrap_or(0)), ..Default::default() }),
                flags: Some(AccountCartLineItemFlags { is_gift: Some(gift), is_private: None }),
                ..Default::default()
            };
            match conn.call::<_, CAccountCartModifyLineItemResponse>("AccountCart.ModifyLineItem#1", &req).await {
                Ok(r) => sink(Event::Cart(cart_lines(r.cart))),
                Err(e) => sink(Event::StoreError(format!("Couldn't change that cart item: {e}"))),
            }
        });
    }

    fn cart_remove(&self, line_item_id: u64, sink: &Sink) {
        let (conn, sink, country) = (self.conn.clone(), sink.clone(), self.cart_country());
        tokio::spawn(async move {
            use proto::webui::*;
            match conn.call::<_, CAccountCartRemoveItemFromCartResponse>("AccountCart.RemoveItemFromCart#1", &CAccountCartRemoveItemFromCartRequest { line_item_id: Some(line_item_id), user_country: country }).await {
                Ok(r) => sink(Event::Cart(cart_lines(r.cart))),
                Err(e) => sink(Event::StoreError(e.to_string())),
            }
        });
    }

    fn activate_key(&self, key: String, sink: &Sink) {
        let (conn, sink) = (self.conn.clone(), sink.clone());
        tokio::spawn(async move {
            let req = CStoreRegisterCdKeyRequest {
                activation_code: Some(key.trim().to_string()),
                purchase_platform: None,
                is_request_from_client: Some(true),
            };
            let result = match conn.call_packet("Store.RegisterCDKey#1", &req).await {
                Ok(p) => {
                    let body: CStoreRegisterCdKeyResponse = p.body().unwrap_or_default();
                    let detail = body.purchase_result_details();
                    let items: Vec<String> = body
                        .purchase_receipt_info
                        .as_ref()
                        .map(|r| r.line_items.iter().map(|l| l.line_item_description().to_string()).collect())
                        .unwrap_or_default();
                    if p.eresult() == eresult::OK && detail == 0 {
                        Ok(items)
                    } else {
                        Err(key_error(detail, p.eresult()))
                    }
                }
                Err(e) => Err(e.to_string()),
            };
            sink(Event::KeyActivated(result));
        });
    }

    fn load_profile(&self, steam_id: SteamId, sink: &Sink) {
        let (conn, sink) = (self.conn.clone(), sink.clone());
        tokio::spawn(async move {
            let mut info = model::ProfileInfo { steam_id, ..Default::default() };
            let items: Result<CPlayerGetProfileItemsEquippedResponse, _> = conn
                .call(
                    "Player.GetProfileItemsEquipped#1",
                    &CPlayerGetProfileItemsEquippedRequest { steamid: Some(steam_id), language: Some("english".into()) },
                )
                .await;
            if let Ok(items) = items {
                let url = |i: &Option<ProfileItem>| {
                    i.as_ref().map(|i| i.image_large()).filter(|s| !s.is_empty()).map(model::art::community_item)
                };
                info.background = url(&items.profile_background);
                info.background_full = items.profile_background.as_ref().is_some_and(|i| i.equipped_flags() & 1 != 0);
                info.mini_background = url(&items.mini_profile_background);
                info.animated_avatar = items
                    .animated_avatar
                    .as_ref()
                    .map(|i| i.image_small())
                    .filter(|s| !s.is_empty())
                    .map(model::art::community_item);
                info.avatar_frame = items
                    .avatar_frame
                    .as_ref()
                    .map(|i| i.image_small())
                    .filter(|s| !s.is_empty())
                    .map(model::art::community_item);
            }
            let custom: Result<CPlayerGetProfileCustomizationResponse, _> = conn
                .call("Player.GetProfileCustomization#1", &CPlayerGetProfileCustomizationRequest { steamid: Some(steam_id), ..Default::default() })
                .await;
            if let Ok(c) = custom {
                info.theme = c.profile_theme.as_ref().map(|t| t.theme_id().to_string()).unwrap_or_default();
            }
            let reply = conn
                .request(emsg::CLIENT_FRIEND_PROFILE_INFO, &CMsgClientFriendProfileInfo { steamid_friend: Some(steam_id) })
                .await;
            if let Ok(r) = reply.and_then(|p| p.body::<CMsgClientFriendProfileInfoResponse>()) {
                info.real_name = r.real_name().into();
                info.summary = r.summary().into();
                info.location = [r.city_name(), r.state_name(), r.country_name()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
                info.member_since = Some(r.time_created() as u64).filter(|t| *t > 0);
            }
            (info.visibility, info.vac_banned) = public_summary(steam_id).await;
            sink(Event::Profile(info));
        });
    }

    fn load_inventory(&self, steam_id: SteamId, app_id: AppId, context_id: u64, language: String, sink: &Sink) {
        let (conn, sink) = (self.conn.clone(), sink.clone());
        tokio::spawn(async move {
            let mut inv = model::Inventory { steam_id, app_id, context_id, ..Default::default() };
            let mut start = None;
            // Steam caps a page at 2000 items; stop at a sane total.
            while inv.items.len() < 20_000 {
                let req = CEconGetInventoryItemsWithDescriptionsRequest {
                    steamid: Some(steam_id),
                    appid: Some(app_id),
                    contextid: Some(context_id),
                    get_descriptions: Some(true),
                    language: Some(language.clone()),
                    start_assetid: start,
                    count: Some(2000),
                    ..Default::default()
                };
                match conn.call::<_, CEconGetInventoryItemsWithDescriptionsResponse>("Econ.GetInventoryItemsWithDescriptions#1", &req).await {
                    Ok(r) => {
                        inv.total = r.total_inventory_count();
                        let descs: HashMap<(u64, u64), &CEconItemDescription> = r.descriptions.iter().map(|d| ((d.classid(), d.instanceid()), d)).collect();
                        for a in &r.assets {
                            let Some(d) = descs.get(&(a.classid(), a.instanceid())) else { continue };
                            inv.items.push(econ_item(steam_id, a, d));
                        }
                        if r.more_items() && r.last_assetid() != 0 {
                            start = Some(r.last_assetid());
                        } else {
                            break;
                        }
                    }
                    Err(net::Error::EResult(code, _)) if code == eresult::ACCESS_DENIED => {
                        inv.error = Some("private".into());
                        break;
                    }
                    Err(e) => {
                        inv.error = Some(e.to_string());
                        break;
                    }
                }
            }
            if inv.total == 0 {
                inv.total = inv.items.len() as u32;
            }
            sink(Event::Inventory(inv));
        });
    }

    /// Run `f` with the signed-in web session (made on first use).
    fn with_web<F, Fut>(&self, sink: &Sink, f: F)
    where
        F: FnOnce(Result<std::sync::Arc<crate::market::Web>, String>, Sink) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let (conn, sink, me, refresh, cache) = (self.conn.clone(), sink.clone(), self.me, self.refresh_token.clone(), self.web.clone());
        tokio::spawn(async move {
            let web = crate::market::web(&conn, me, &refresh, &cache).await;
            if let Err(e) = &web {
                tracing::warn!("web sign-in: {e}");
            }
            f(web, sink).await;
        });
    }

    fn load_inventory_apps(&self, steam_id: SteamId, sink: &Sink) {
        self.with_web(sink, move |web, sink| async move {
            let (result, rules) = crate::market::inventory_apps(web.as_deref().ok(), steam_id).await;
            if let Some(rules) = rules {
                sink(Event::FeeRules(rules));
            }
            if let Err(e) = &result {
                tracing::info!("inventory list for {steam_id}: {e}");
            }
            sink(Event::InventoryApps { steam_id, result });
        });
    }

    fn market_price(&self, app_id: AppId, hash_name: String, currency: i32, country: String, full: bool, sink: &Sink) {
        self.with_web(sink, move |web, sink| async move {
            let result = crate::market::price(web.as_deref().ok(), app_id, &hash_name, currency, &country, full).await;
            sink(Event::MarketPrice { app_id, hash_name, result });
        });
    }

    fn sell_item(&self, app_id: AppId, context_id: u64, asset_id: u64, amount: u64, receive: u64, sink: &Sink) {
        self.with_web(sink, move |web, sink| async move {
            let result = match web {
                Ok(w) => crate::market::sell(&w, app_id, context_id, asset_id, amount, receive).await,
                Err(e) => Err(e),
            };
            sink(Event::ItemListed { asset_id, result });
        });
    }

    fn my_listings(&self, sink: &Sink) {
        self.with_web(sink, move |web, sink| async move {
            let result = match web {
                Ok(w) => crate::market::my_listings(&w).await,
                Err(e) => Err(e),
            };
            sink(Event::MyListings(result));
        });
    }

    fn load_notifications(&self, sink: &Sink) {
        tokio::spawn(Online::fetch_notifications(self.conn.clone(), sink.clone()));
    }

    async fn fetch_notifications(conn: Connection, sink: Sink) {
        let req = proto::webui::CSteamNotificationGetSteamNotificationsRequest { include_hidden: Some(false), include_read: Some(true), ..Default::default() };
        match conn.call::<_, proto::webui::CSteamNotificationGetSteamNotificationsResponse>("SteamNotification.GetSteamNotifications#1", &req).await {
            Ok(r) => {
                let mut items: Vec<model::Notification> = r.notifications.iter().map(crate::notify::convert).collect();
                items.sort_by_key(|n| std::cmp::Reverse(n.time));
                sink(Event::Notifications(model::Notifications {
                    unread: r.unread_count.unwrap_or_else(|| items.iter().filter(|n| !n.read).count() as u32),
                    items,
                    pending_gifts: r.pending_gift_count(),
                    pending_friends: r.pending_friend_count(),
                    confirmations: r.confirmation_count().max(0) as u32,
                }));
            }
            Err(e) => tracing::warn!("notifications: {e}"),
        }
    }

    fn mark_read(&self, ids: Vec<u64>, sink: &Sink) {
        let req = proto::webui::CSteamNotificationMarkNotificationsReadNotification {
            mark_all_read: Some(ids.is_empty()),
            notification_ids: ids,
            ..Default::default()
        };
        if let Err(e) = self.conn.notify_service("SteamNotification.MarkNotificationsRead#1", &req) {
            tracing::warn!("mark read: {e}");
        }
        // Reload once Steam has applied it, so the counts are right.
        let (st, sink) = (self.conn.clone(), sink.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(800)).await;
            Online::fetch_notifications(st, sink).await;
        });
    }

    fn support_tickets(&self, sink: &Sink) {
        self.with_web(sink, move |web, sink| async move {
            let result = match web {
                Ok(w) => match crate::market::page(&w, "https://help.steampowered.com/en/wizard/HelpRequests").await {
                    Ok(body) => {
                        Ok(crate::support::parse_tickets(&body))
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            sink(Event::SupportTickets(result));
        });
    }

    fn support_ticket(&self, id: String, url: String, sink: &Sink) {
        self.with_web(sink, move |web, sink| async move {
            let result = match web {
                Ok(w) => match validate_support_ticket_url(&url) {
                    Ok(valid) => crate::market::page(&w, valid.as_str()).await.map(|b| crate::support::parse_messages(&b)),
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            sink(Event::SupportTicket { id, result });
        });
    }

    fn cancel_listing(&self, listing_id: u64, sink: &Sink) {
        self.with_web(sink, move |web, sink| async move {
            let result = match web {
                Ok(w) => crate::market::cancel_listing(&w, listing_id).await,
                Err(e) => Err(e),
            };
            sink(Event::ListingCancelled { listing_id, result });
        });
    }

    fn cancel_buy_order(&self, order_id: u64, sink: &Sink) {
        self.with_web(sink, move |web, sink| async move {
            let result = match web {
                Ok(w) => crate::market::cancel_buy_order(&w, order_id).await,
                Err(e) => Err(e),
            };
            sink(Event::BuyOrderCancelled { order_id, result });
        });
    }

    fn load_achievement_progress(&self, app_ids: Vec<AppId>, sink: &Sink) {
        let (conn, sink, me) = (self.conn.clone(), sink.clone(), self.me);
        tokio::spawn(async move {
            let req = CPlayerGetAchievementsProgressRequest { steamid: Some(me), language: Some("english".into()), appids: app_ids, ..Default::default() };
            match conn.call::<_, CPlayerGetAchievementsProgressResponse>("Player.GetAchievementsProgress#1", &req).await {
                Ok(r) => sink(Event::AchievementProgress(r.achievement_progress.iter().map(|a| (a.appid() as AppId, a.unlocked(), a.total())).collect())),
                Err(e) => tracing::debug!("achievement progress: {e}"),
            }
        });
    }

    fn load_achievements(&self, app_id: AppId, language: String, sink: &Sink) {
        let (conn, sink, me) = (self.conn.clone(), sink.clone(), self.me);
        tokio::spawn(async move {
            let schema = conn
                .call::<_, CPlayerGetGameAchievementsResponse>("Player.GetGameAchievements#1", &CPlayerGetGameAchievementsRequest { appid: Some(app_id), language: Some(language), hash_only: Some(false) })
                .await;
            let mine = conn.call::<_, CPlayerGetUserAchievementsResponse>("Player.GetUserAchievements#1", &CPlayerGetUserAchievementsRequest { steamid: Some(me), appid: Some(app_id) }).await;
            let result = match schema {
                Err(e) => Err(e.to_string()),
                Ok(schema) => {
                    let unlocked: HashMap<u32, u64> = mine.map(|m| m.achievements.iter().filter(|a| a.unlocked()).map(|a| (a.internal_key(), a.unlock_time() as u64)).collect()).unwrap_or_default();
                    let image = |file: &str| if file.is_empty() { String::new() } else { format!("https://cdn.akamai.steamstatic.com/steamcommunity/public/images/apps/{app_id}/{file}") };
                    Ok(schema
                        .achievements
                        .iter()
                        .filter(|a| !a.archived())
                        .map(|a| model::Achievement {
                            name: a.localized_name().to_string(),
                            description: a.localized_desc().to_string(),
                            icon: image(a.icon()),
                            icon_gray: image(a.icon_gray()),
                            hidden: a.hidden(),
                            unlocked: unlocked.contains_key(&a.internal_key()),
                            unlock_time: unlocked.get(&a.internal_key()).copied().unwrap_or(0),
                            global_percent: a.player_percent_unlocked().parse().unwrap_or(0.0),
                        })
                        .collect())
                }
            };
            sink(Event::Achievements { app_id, result });
        });
    }

    fn load_player_games(&self, steam_id: SteamId, language: String, sink: &Sink) {
        let (conn, sink) = (self.conn.clone(), sink.clone());
        tokio::spawn(async move {
            let req = CPlayerGetOwnedGamesRequest {
                steamid: Some(steam_id),
                include_appinfo: Some(true),
                include_played_free_games: Some(true),
                language: Some(language),
                ..Default::default()
            };
            let result = match conn.call::<_, CPlayerGetOwnedGamesResponse>("Player.GetOwnedGames#1", &req).await {
                // Steam answers a private profile with an empty reply.
                Ok(r) if r.game_count.is_none() && r.games.is_empty() => Err("private".to_string()),
                Ok(r) => Ok(r
                    .games
                    .iter()
                    .map(|g| model::PlayerGame {
                        app_id: g.appid() as AppId,
                        name: g.name().to_string(),
                        playtime_minutes: g.playtime_forever().max(0) as u64,
                        playtime_2weeks: g.playtime_2weeks().max(0) as u64,
                        last_played: Some(g.rtime_last_played() as u64).filter(|t| *t > 0),
                        icon: Some(g.img_icon_url().to_string()).filter(|s| !s.is_empty()),
                    })
                    .collect()),
                Err(net::Error::EResult(code, _)) if code == eresult::ACCESS_DENIED => Err("private".to_string()),
                Err(e) => Err(e.to_string()),
            };
            sink(Event::PlayerGames { steam_id, result });
        });
    }

    fn load_history(&self, friend: SteamId, sink: &Sink) {
        let (conn, sink, me) = (self.conn.clone(), sink.clone(), self.me);
        tokio::spawn(async move {
            let r: Result<CFriendMessagesGetRecentMessagesResponse, _> = conn
                .call(
                    "FriendMessages.GetRecentMessages#1",
                    &CFriendMessagesGetRecentMessagesRequest {
                        steamid1: Some(me),
                        steamid2: Some(friend),
                        count: Some(50),
                        most_recent_conversation: Some(false),
                        // With markup, so invites keep their game and lobby.
                        bbcode_format: Some(true),
                        ..Default::default()
                    },
                )
                .await;
            let base = me & 0xFFFF_FFFF_0000_0000;
            match r {
                Ok(r) => {
                    let mut messages: Vec<model::ChatMessage> = r
                        .messages
                        .iter()
                        .map(|m| {
                            let parsed = model::chat::parse_message(m.message());
                            model::ChatMessage {
                                friend,
                                from: base | m.accountid() as u64,
                                text: parsed.text,
                                time: m.timestamp() as u64,
                                invite: parsed.invite,
                                group_invite: parsed.group_invite,
                                media: parsed.media,
                            }
                        })
                        .collect();
                    messages.sort_by_key(|m| m.time);
                    sink(Event::History { friend, messages });
                }
                Err(e) => tracing::warn!("chat history: {e}"),
            }
        });
    }

    /// What Steam Cloud needs for this game, if it uses the cloud and the
    /// player hasn't turned it off.
    fn cloud_target(&self, app_id: AppId, install_dir: &std::path::Path) -> Option<crate::cloud::Target> {
        if self.cloud_off.contains(&app_id) {
            return None;
        }
        self.cloud_target_any(app_id, install_dir)
    }

    fn cloud_target_any(&self, app_id: AppId, install_dir: &std::path::Path) -> Option<crate::cloud::Target> {
        let config = self.apps.get(&app_id)?.cloud.clone()?;
        let prefix_user = launch::proton_user_dir(self.apps.get(&app_id)?, install_dir);
        Some(crate::cloud::Target { app_id, steam_id: self.me, install_dir: install_dir.to_path_buf(), steam_root: self.steam_root.clone(), config: Some(config), cell_id: self.cell_id, prefix_user })
    }

    fn game_launch_mode(&self, app: &OwnedApp, dir: &std::path::Path, controller: u8) -> crate::LaunchMode {
        if self.link == Link::Online {
            launch::mode_for_launch(app, dir, controller)
        } else {
            // The headless host must sign in to Steam before it can serve a game.
            launch::mode_with_host(app, dir, controller, false)
        }
    }

    fn launch(
        &mut self,
        app_id: AppId,
        dir: &std::path::Path,
        args: &crate::LaunchArgs,
        sink: &Sink,
        exit_tx: &mpsc::UnboundedSender<(AppId, Option<std::path::PathBuf>)>,
    ) -> Result<(), Error> {
        if let Err(e) = launch::safe_connect_args(&args.connect) {
            sink(Event::Error(e));
            return Ok(());
        }
        let Some(app) = self.apps.get(&app_id) else {
            sink(Event::Error("Library is still loading, try again in a moment".into()));
            return Ok(());
        };
        if self.running.contains_key(&app_id) {
            sink(Event::Error(format!("{} is already running", app.name)));
            return Ok(());
        }
        let mode = self.game_launch_mode(app, dir, args.controller);
        // The available Steam route can change during shader compilation.
        if launch::needs_anti_cheat_warning(app, dir, &mode) {
            sink(Event::AntiCheatWarning { app_id, install_dir: dir.to_path_buf(), args: args.clone(), skip_cloud: true });
            return Ok(());
        }
        tracing::info!("{} starts as {mode:?}", app.name);
        sink(Event::LaunchMode { app_id, mode: mode.clone() });
        sink(Event::GameLaunching { app_id });
        let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        match mode {
            crate::LaunchMode::OfficialSteam { .. } => {
                // The official client runs it; we only follow along. No
                // GamesPlayed from us (Steam sends its own) and no cloud sync.
                self.running.insert(app_id, 0);
                self.stop_flags.insert(app_id, stop_flag);
                self.official.insert(app_id, dir.to_path_buf());
                let (tx, sink, args, me, offline) = (exit_tx.clone(), sink.clone(), args.clone(), self.me, self.link != Link::Online);
                tokio::task::spawn_blocking(move || {
                    let started = || sink(Event::GameStarted { app_id });
                    if let Err(e) = launch::run_through_steam(app_id, &args, me, offline, started) {
                        sink(Event::Error(e));
                    }
                    let _ = tx.send((app_id, None));
                });
                return Ok(());
            }
            crate::LaunchMode::HeadlessHost { steam_drm } => {
                // The user's own Steam engine, headless, signs in on a second
                // CM session and reports ClientGamesPlayed itself once the game
                // connects via Steamworks. Do NOT call games_played() on
                // Aurora's CM connection here — Steam enforces single-session
                // gaming and will kick Aurora with EResult::LoggedInElsewhere
                // ("Logged in from another location") as soon as the game
                // initializes Steamworks (#139). It needs the live refresh token.
                let app = app.clone();
                let (account, token, me) = (self.account.clone(), self.refresh_token.clone(), self.me);
                self.running.insert(app_id, 0);
                self.stop_flags.insert(app_id, stop_flag.clone());
                self.official.insert(app_id, dir.to_path_buf());
                #[cfg(not(windows))]
                let overlay = if launch::app_overlay_enabled(app_id) {
                    self.ipc.overlay_port().map(|port| {
                        let tok = self.ipc.register(app_id);
                        self.launch_tokens.insert(app_id, tok.clone());
                        (port, tok)
                    })
                } else {
                    None
                };
                #[cfg(windows)]
                let overlay: Option<(u16, String)> = None;
                let needs_vpad = launch::needs_aurora_input_route(app_id, args.controller);
                let ctrl_override = crate::pads::effective_controller_override(app_id, args.controller);
                let include_xbox = ctrl_override == 1
                    || crate::input_config::app_binding(app_id) != crate::input_config::BindingChoice::Auto;
                let vpad_started = if needs_vpad {
                    if crate::pads::start_virtual_pads(include_xbox) {
                        self.vpad_users.insert(app_id, include_xbox);
                        tracing::info!(
                            "controllers: HeadlessHost launch for {} using {} virtual Xbox 360 pad",
                            app.name,
                            crate::pads::virtual_pad_backend()
                        );
                        true
                    } else {
                        tracing::warn!(
                            "controllers: HeadlessHost launch for {} needs controller translation, but {} is unavailable ({})",
                            app.name,
                            crate::pads::virtual_pad_backend(),
                            crate::pads::virtual_pad_hint()
                        );
                        false
                    }
                } else {
                    false
                };
                sink(Event::ControllerLayoutRoute { app_id, enabled: false });
                let (tx, sink, args, dir2) = (exit_tx.clone(), sink.clone(), args.clone(), dir.to_path_buf());
                tokio::task::spawn_blocking(move || {
                    let started = || sink(Event::GameStarted { app_id });
                    let ov = overlay.as_ref().map(|(p, t)| (*p, t.as_str()));
                    let cloud_dir = match launch::run_through_host_cancellable(&app, &dir2, &args, &account, &token, me, ov, vpad_started, Some(&stop_flag), started) {
                        Ok(()) if !steam_drm => Some(dir2),
                        Ok(()) => None,
                        Err(e) => {
                            sink(Event::Error(e));
                            None
                        }
                    };
                    let _ = tx.send((app_id, cloud_dir));
                });
                return Ok(());
            }
            crate::LaunchMode::Aurora => {
                let token = self.ipc.register(app_id);
                let is64 = launch::exe_is_64bit(app, dir).unwrap_or(true);
                let dll64 = launch::steamclient_dll(true);
                let dll32 = launch::steamclient_dll(false);
                let primary_ok = if is64 { dll64.is_some() } else { dll32.is_some() };
                let account_id = model::account_id(self.me);
                let swapped = if primary_ok {
                    launch::swap_begin_both(&token, dll64.as_deref(), dll32.as_deref(), account_id)
                } else {
                    Err("steamclient DLL missing".to_string())
                };
                let ctrl_override = crate::pads::effective_controller_override(app_id, args.controller);
                let input_decision = crate::pads::resolve_steam_input(ctrl_override, &crate::pads::settings().families);
                let pads = (swapped.is_ok() && input_decision.enabled && input_decision.use_shm_bridge)
                    .then(crate::pads::start)
                    .flatten();
                #[cfg(not(windows))]
                let (vpad_started, vpad_include_xbox) = {
                    let include_xbox = ctrl_override == 1
                        || crate::input_config::app_binding(app_id) != crate::input_config::BindingChoice::Auto;
                    let ok = swapped.is_ok()
                        && input_decision.enabled
                        && launch::needs_aurora_input_route(app_id, args.controller)
                        && crate::pads::start_virtual_pads(include_xbox);
                    (ok, include_xbox)
                };
                #[cfg(windows)]
                let (vpad_started, vpad_include_xbox) = (false, false);
                #[cfg(target_os = "linux")]
                let overlay = if launch::app_overlay_enabled(app_id) {
                    self.ipc.overlay_port().map(|port| (port, token.clone()))
                } else {
                    None
                };
                #[cfg(not(target_os = "linux"))]
                let overlay: Option<(u16, String)> = None;
                let started = swapped.and_then(|_| {
                    let ov = overlay.as_ref().map(|(p, t)| (*p, t.as_str()));
                    launch::spawn_with_steamclient(app, dir, args, &self.ipc.env_value(&token), &self.account, pads.as_deref(), ov, vpad_started)
                });
                if started.is_err() {
                    if pads.is_some() {
                        crate::pads::stop();
                    }
                    if vpad_started {
                        crate::pads::stop_virtual_pads(vpad_include_xbox);
                    }
                }
                match started {
                    Ok(child) => {
                        if pads.is_some() {
                            self.pad_users.insert(app_id);
                        }
                        if vpad_started {
                            self.vpad_users.insert(app_id, vpad_include_xbox);
                        }
                        self.launch_tokens.insert(app_id, token.clone());
                        if input_decision.enabled {
                            self.launch_layouts.insert(app_id, Default::default());
                        }
                        sink(Event::ControllerLayoutRoute { app_id, enabled: input_decision.enabled });
                        // A game that never loads the DLL mustn't keep it in the registry.
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(300)).await;
                            launch::swap_loaded(&token);
                        });
                        self.track(app_id, dir, child, sink, exit_tx)?;
                    }
                    Err(e) => {
                        launch::swap_exited(&token);
                        self.ipc.forget(&token);
                        sink(Event::Error(e));
                    }
                }
            }
            crate::LaunchMode::NoSteam { .. } => {
                #[cfg(target_os = "linux")]
                let overlay = if launch::app_overlay_enabled(app_id) {
                    self.ipc.overlay_port().map(|port| {
                        let tok = self.ipc.register(app_id);
                        (port, tok)
                    })
                } else {
                    None
                };
                #[cfg(not(target_os = "linux"))]
                let overlay: Option<(u16, String)> = None;
                let ov = overlay.as_ref().map(|(p, t)| (*p, t.as_str()));
                match launch::spawn(app, dir, args, ov) {
                    Ok(child) => {
                        if let Some((_, tok)) = overlay {
                            self.launch_tokens.insert(app_id, tok);
                        }
                        self.track(app_id, dir, child, sink, exit_tx)?;
                    }
                    Err(e) => {
                        if let Some((_, tok)) = overlay {
                            self.ipc.forget(&tok);
                        }
                        sink(Event::Error(e));
                    }
                }
            }
        }
        Ok(())
    }

    /// A game we started ourselves: we're the one "playing" it.
    fn track(&mut self, app_id: AppId, dir: &std::path::Path, child: std::process::Child, sink: &Sink, exit_tx: &mpsc::UnboundedSender<(AppId, Option<std::path::PathBuf>)>) -> Result<(), Error> {
        let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.running.insert(app_id, child.id());
        self.stop_flags.insert(app_id, stop_flag.clone());
        self.launched_dirs.insert(app_id, dir.to_path_buf());
        self.playing = Some(app_id);
        if self.link == Link::Online {
            let _ = games_played(&self.conn, Some(app_id));
        } else {
            // Steam won't see this; keep the time here.
            self.unconnected_play.insert(app_id, (unix_now(), self.link == Link::Offline));
        }
        sink(Event::GameStarted { app_id });
        let tx = exit_tx.clone();
        let dir_owned = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send((launch::wait_in_dir_cancellable(app_id, &dir_owned, child, Some(&stop_flag)), None));
        });
        Ok(())
    }

    /// A request from a game running with our steamclient DLL.
    fn ipc_call(&mut self, call: crate::ipc::Call, _sink: &Sink) {
        use crate::ipc::protocol::{Reply, Request};
        let crate::ipc::Call { app_id, token, pid, req, reply } = call;
        let (conn, me) = (self.conn.clone(), self.me);
        let mark_swap_loaded = |tok: String| {
            #[cfg(windows)]
            {
                // Keep our steamclient registered briefly after the first Hello
                // so a launcher that spawns the actual gameplay child process
                // (possibly of a different bitness) still loads Aurora's DLL.
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(15)).await;
                    launch::swap_loaded(&tok);
                });
            }
            #[cfg(not(windows))]
            launch::swap_loaded(&tok);
        };
        match req {
            Request::Hello if self.link != Link::Online => {
                tracing::info!("game {app_id} (pid {pid}) connected to Aurora's steamclient (offline)");
                mark_swap_loaded(token);
                let mut session = self.ipc_session(app_id);
                session.stats = offline::offline_view(&offline::load_stats(me, app_id));
                let _ = reply.send(Reply::Hello(Box::new(session)));
            }
            Request::UserStats { steam_id } if self.link != Link::Online => {
                let result = if steam_id == me { offline::offline_view(&offline::load_stats(me, app_id)).ok_or_else(|| "offline".to_string()) } else { Err("offline".into()) };
                let _ = reply.send(Reply::Stats { result });
            }
            Request::StoreStats { stats, crc, reset } if self.link != Link::Online => {
                // Like Steam offline: kept here, sent once back online.
                offline::store_offline(me, app_id, &stats, reset);
                tracing::info!("game {app_id} stored {} stat(s) while offline", stats.len());
                let _ = reply.send(Reply::Stored { eresult: eresult::OK, crc });
            }
            Request::Hello => {
                tracing::info!("game {app_id} (pid {pid}) connected to Aurora's steamclient");
                mark_swap_loaded(token);
                let session = self.ipc_session(app_id);
                let language = session.game_language.clone();
                tokio::spawn(async move {
                    let mut session = session;
                    let badges = async {
                        conn.call::<_, CPlayerGetGameBadgeLevelsResponse>("Player.GetGameBadgeLevels#1", &CPlayerGetGameBadgeLevelsRequest { appid: Some(0) })
                            .await
                            .ok()
                            .map(|r| r.player_level())
                    };
                    let (stats, level) = tokio::join!(crate::ipc::user_stats(&conn, app_id, me, &language), badges);
                    let mut cached = offline::load_stats(me, app_id);
                    match stats {
                        // Changes from an offline session not sent yet still count.
                        Ok(s) if !cached.pending.is_empty() || cached.reset => {
                            cached.stats = Some(s);
                            session.stats = offline::offline_view(&cached);
                        }
                        Ok(s) => {
                            offline::remember_stats(me, app_id, &s);
                            session.stats = Some(s);
                        }
                        Err(e) => {
                            tracing::info!("no stats for {app_id}: {e}");
                            session.stats = offline::offline_view(&cached);
                        }
                    }
                    session.level = level.unwrap_or(0);
                    let _ = reply.send(Reply::Hello(Box::new(session)));
                });
            }
            Request::UserStats { steam_id } => {
                let language = self.ipc_session(app_id).game_language;
                tokio::spawn(async move {
                    let result = crate::ipc::user_stats(&conn, app_id, steam_id, &language).await;
                    let _ = reply.send(Reply::Stats { result });
                });
            }
            Request::StoreStats { stats, crc, reset } => {
                tokio::spawn(async move {
                    let (mut eresult, crc) = crate::ipc::store_stats(&conn, app_id, me, &stats, crc, reset).await;
                    tracing::info!("game {app_id} stored {} stat(s): result {eresult}", stats.len());
                    if eresult == 3 {
                        // The connection is gone: keep them for later, like Steam.
                        offline::store_offline(me, app_id, &stats, reset);
                        eresult = net::eresult::OK;
                    }
                    let _ = reply.send(Reply::Stored { eresult, crc });
                });
            }
            Request::SteamDrm => {
                let _ = reply.send(Reply::Ok);
                // Its DRM is about to refuse to start: stop it and start it
                // again on Valve's engine (remembered for next time).
                if launch::mark_steam_drm(app_id) {
                    tracing::info!("game {app_id} carries Steam DRM: restarting it on Valve's engine");
                    self.drm_relaunch.insert(app_id);
                    let (pid, dir) = (self.running.get(&app_id).copied(), self.launched_dirs.get(&app_id).cloned());
                    tokio::task::spawn_blocking(move || {
                        if let Some(pid) = pid {
                            launch::kill_tree(pid);
                        }
                        if let Some(dir) = dir {
                            launch::kill_in_folder(&dir);
                        }
                    });
                }
            }
            Request::RichPresence { pairs } => {
                // "connect" is how friends join the game ("Invite to game").
                let connect = pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case("connect")).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
                _sink(Event::GameConnect { app_id, connect });
                crate::ipc::rich_presence(&conn, &pairs);
                let _ = reply.send(Reply::Ok);
            }
            Request::CurrentPlayers => {
                tokio::spawn(async move {
                    let count = crate::ipc::current_players(&conn, app_id).await;
                    let _ = reply.send(Reply::Players { count });
                });
            }
            Request::AuthTicket { identity } => {
                let auth = self.auth.clone();
                tokio::spawn(async move {
                    if !identity.is_empty() {
                        tracing::info!("game {app_id} wants a web API ticket for {identity}");
                    }
                    let r = match crate::ipc::auth_ticket(&conn, &auth, app_id, pid).await {
                        Ok((ticket, crc, seq)) => Reply::Ticket { ticket, crc, seq },
                        Err(message) => Reply::Err { message },
                    };
                    let _ = reply.send(r);
                });
            }
            Request::AuthTicketWait { seq } => {
                let auth = self.auth.clone();
                tokio::spawn(async move {
                    let ok = crate::ipc::wait_ack(&auth, seq).await;
                    let _ = reply.send(if ok { Reply::Ok } else { Reply::Err { message: "Steam didn't confirm the ticket".into() } });
                });
            }
            Request::CancelTicket { crc } => {
                crate::ipc::cancel_ticket(&conn, &self.auth, crc);
                let _ = reply.send(Reply::Ok);
            }
            Request::BeginAuth { steam_id, ticket } => {
                let auth = self.auth.clone();
                tokio::spawn(async move {
                    let (response, owner) = crate::ipc::begin_auth(&conn, &auth, app_id, pid, steam_id, &ticket).await;
                    let _ = reply.send(Reply::AuthResult { response, owner });
                });
            }
            Request::EndAuth { steam_id } => {
                crate::ipc::end_auth(&conn, &self.auth, app_id, steam_id);
                let _ = reply.send(Reply::Ok);
            }
            Request::Poll => {
                let _ = reply.send(Reply::Events { events: Vec::new() });
            }
            Request::LobbyCreate { kind, max_members } => {
                let name = self.my_name();
                let sink = _sink.clone();
                tokio::spawn(async move {
                    let (eresult, lobby) = crate::ipc::lobby_create(&conn, app_id, kind, max_members, &name).await;
                    tracing::info!("game {app_id} created lobby {lobby}: result {eresult}");
                    if eresult == eresult::OK {
                        crate::ipc::lobby_refresh(&conn, app_id, lobby);
                        sink(Event::GameLobby { app_id, lobby: Some(lobby) });
                    }
                    let _ = reply.send(Reply::LobbyCreated { eresult, lobby });
                });
            }
            Request::LobbyJoin { lobby } => {
                let name = self.my_name();
                let sink = _sink.clone();
                tokio::spawn(async move {
                    let id = lobby;
                    let (response, lobby) = crate::ipc::lobby_join(&conn, app_id, lobby, &name).await;
                    // EChatRoomEnterResponse 1 = success.
                    if response == 1 {
                        sink(Event::GameLobby { app_id, lobby: Some(id) });
                    }
                    let _ = reply.send(Reply::LobbyJoined { response, lobby });
                });
            }
            Request::LobbyLeave { lobby } => {
                _sink(Event::GameLobby { app_id, lobby: None });
                crate::ipc::lobby_leave(&conn, app_id, lobby);
                let _ = reply.send(Reply::Ok);
            }
            Request::LobbyList { filters, count } => {
                tokio::spawn(async move {
                    let lobbies = crate::ipc::lobby_list(&conn, app_id, &filters, count).await;
                    let _ = reply.send(Reply::LobbyList { lobbies });
                });
            }
            Request::LobbySetData { lobby, member, data, max_members, kind, flags } => {
                tokio::spawn(async move {
                    let eresult = crate::ipc::lobby_set_data(&conn, app_id, me, lobby, member, &data, max_members, kind, flags).await;
                    let _ = reply.send(Reply::Result { eresult });
                });
            }
            Request::LobbyRefresh { lobby } => {
                crate::ipc::lobby_refresh(&conn, app_id, lobby);
                let _ = reply.send(Reply::Ok);
            }
            Request::LobbyChat { lobby, data } => {
                crate::ipc::lobby_chat(&conn, app_id, lobby, 0, &data);
                let _ = reply.send(Reply::Ok);
            }
            Request::LobbySetOwner { lobby, owner } => {
                crate::ipc::lobby_set_owner(&conn, app_id, lobby, owner);
                let _ = reply.send(Reply::Ok);
            }
            Request::LobbyInvite { lobby, user } => {
                crate::ipc::lobby_invite(&conn, app_id, lobby, user);
                let _ = reply.send(Reply::Ok);
            }
            Request::Signal { to, data, lobby } => {
                crate::ipc::signal(&conn, app_id, me, to, &data, lobby);
                let _ = reply.send(Reply::Ok);
            }
            Request::LbFind { name, create, sort, display } => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::ipc::lb_find(&conn, app_id, &name, create, sort, display).await);
                });
            }
            Request::LbEntries { lb, request, start, end, users } => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::ipc::lb_entries(&conn, app_id, lb, request, start, end, &users).await);
                });
            }
            Request::LbUpload { lb, method, score, details } => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::ipc::lb_upload(&conn, app_id, lb, method, score, &details).await);
                });
            }
            Request::GlobalPercentages => {
                tokio::spawn(async move {
                    let list = crate::ipc::global_percentages(&conn, app_id).await;
                    let _ = reply.send(Reply::Percentages { list });
                });
            }
            Request::Avatar { steam_id, size } => {
                // Default avatar for players we have no persona data for.
                let hash = self.personas.get(&steam_id).and_then(|p| p.avatar_hash.clone()).unwrap_or_else(|| "fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb".into());
                tokio::spawn(async move {
                    let r = match crate::ipc::avatar(&hash, size).await {
                        Some((width, height, rgba)) => Reply::Image { width, height, rgba },
                        None => Reply::Err { message: "avatar unavailable".into() },
                    };
                    let _ = reply.send(r);
                });
            }
            Request::UgcQuery(q) => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_ugc::query(&conn, app_id, &q).await);
                });
            }
            Request::UgcSubscribed => {
                tokio::spawn(async move {
                    let _ = reply.send(Reply::UgcIds { ids: crate::game_ugc::subscribed(&conn, app_id, me).await });
                });
            }
            Request::UgcSubscribe { id, list, on } => {
                tokio::spawn(async move {
                    let _ = reply.send(Reply::Result { eresult: crate::game_ugc::subscribe(&conn, app_id, id, list, on).await });
                });
            }
            Request::UgcVote { id, up } => {
                tokio::spawn(async move {
                    let _ = reply.send(Reply::Result { eresult: crate::game_ugc::vote(&conn, id, up).await });
                });
            }
            Request::UgcGetVote { id } => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_ugc::get_vote(&conn, id).await);
                });
            }
            Request::UgcCreateItem { consumer_app, file_type } => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_ugc::create_item(&conn, app_id, consumer_app, file_type).await);
                });
            }
            Request::UgcUpdateItem(upd) => {
                let cell_id = self.cell_id;
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_ugc::update_item(&conn, app_id, cell_id, &upd).await);
                });
            }
            Request::UgcDeleteItem { id } => {
                tokio::spawn(async move {
                    let _ = reply.send(Reply::Result { eresult: crate::game_ugc::delete_item(&conn, app_id, id).await });
                });
            }
            Request::Inventory(op) => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_inventory::run(&conn, app_id, me, op).await);
                });
            }
            Request::ItemDefs => {
                let refresh = self.refresh_token.clone();
                let cache_dir = crate::store::config_dir().join("itemdefs");
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_inventory::item_defs(&conn, app_id, me, &refresh, &cache_dir).await);
                });
            }
            Request::InputConfig | Request::InputLayout { .. } => {
                let Some(cache) = self.launch_layouts.get(&app_id).cloned()
                    .filter(|_| self.launch_tokens.get(&app_id) == Some(&token))
                else {
                    let _ = reply.send(Reply::InputConfig { bindings: String::new(), source: String::new() });
                    return;
                };
                let root = self.steam_root.clone()
                    .or_else(|| launch::reg_string("Software\\Valve\\Steam", "SteamPath").map(std::path::PathBuf::from))
                    .unwrap_or_default();
                let dir = self.launched_dirs.get(&app_id).cloned().unwrap_or_default();
                let online = self.link == Link::Online;
                tokio::spawn(async move {
                    let mut cache = cache.lock().await;
                    let (mut generation, mut choice) = crate::input_config::selection(app_id);
                    loop {
                        if cache.current.as_ref().is_some_and(|(current, _)| *current == generation) {
                            break;
                        }
                        let layout = crate::input_config::resolve_layout(
                            online.then_some(&conn), app_id, model::account_id(me), &root, &dir, &choice, None,
                        ).await;
                        let latest = crate::input_config::selection(app_id);
                        if latest.0 == generation {
                            cache.current = Some((generation, layout));
                            break;
                        }
                        (generation, choice) = latest;
                    }
                    let (revision, layout) = cache.current.as_ref().unwrap();
                    let response = match req {
                        Request::InputConfig => Reply::InputConfig { bindings: layout.text.clone(), source: layout.source.clone() },
                        Request::InputLayout { known_revision } if known_revision == Some(*revision) => Reply::Ok,
                        _ => Reply::InputLayout { revision: *revision, bindings: layout.text.clone(), source: layout.source.clone() },
                    };
                    let _ = reply.send(response);
                });
            }
            Request::InputLayoutApplied { revision } => {
                if self.launch_tokens.get(&app_id) == Some(&token)
                    && let Some(mut cache) = self.launch_layouts.get(&app_id).and_then(|c| c.try_lock().ok())
                    && crate::input_config::selection(app_id).0 == revision
                    && let Some((_, layout)) = cache.current.as_ref().filter(|(current, _)| *current == revision)
                {
                    if cache.acknowledged != Some(revision) {
                        _sink(Event::ControllerLayoutApplied { app_id, source: layout.source.clone() });
                        cache.acknowledged = Some(revision);
                    }
                    let _ = reply.send(Reply::Ok);
                }
            }
            Request::Overlay { dialog, target } => {
                _sink(Event::GameOverlay { app_id, dialog, target });
                let _ = reply.send(Reply::Ok);
            }
            Request::GameInvite { friend, connect } => {
                // Official clients drop invite markup from other clients; a
                // steam://run link arrives as a normal, clickable message.
                self.send_message(friend, invite_link(app_id, &connect), _sink);
                let _ = reply.send(Reply::Ok);
            }
            Request::ServerList { filter, max } => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_servers::list(&conn, app_id, &filter, max).await);
                });
            }
            Request::FavoriteServers { history } => {
                let root = self.steam_root.clone().unwrap_or_else(|| store::config_dir().join("steam"));
                let account_id = model::account_id(self.me);
                let lists = local::servers::load_lists(&root, account_id);
                let src = if history { &lists.history } else { &lists.favorites };
                let flag = if history { 2 } else { 1 };
                let servers = src
                    .iter()
                    .filter_map(|s| {
                        let (ip, port) = local::servers::parse_ipv4_port(&s.addr)?;
                        Some(crate::ipc::protocol::FavoriteServer {
                            app_id: s.app_id,
                            ip,
                            conn_port: port,
                            query_port: port,
                            flags: flag,
                            last_played: u32::try_from(s.last_played).unwrap_or(u32::MAX),
                        })
                    })
                    .collect();
                let _ = reply.send(Reply::FavoriteServers { servers });
            }
            Request::FavoriteServerAdd(srv) => {
                let root = self.steam_root.clone().unwrap_or_else(|| store::config_dir().join("steam"));
                let account_id = model::account_id(self.me);
                let port = if srv.query_port != 0 { srv.query_port } else { srv.conn_port };
                let addr = local::servers::format_ipv4_port(srv.ip, port);
                let entry = local::servers::SavedServer {
                    addr,
                    name: String::new(),
                    app_id: srv.app_id,
                    game_dir: String::new(),
                    last_played: u64::from(srv.last_played),
                };
                if srv.flags & 2 != 0 {
                    let _ = local::servers::record_history(&root, account_id, entry.clone());
                }
                if srv.flags & 1 != 0 || srv.flags == 0 {
                    let _ = local::servers::add_favorite(&root, account_id, entry);
                }
                let _ = reply.send(Reply::Ok);
            }
            Request::FavoriteServerRemove { app_id: _, ip, conn_port, query_port, flags } => {
                let root = self.steam_root.clone().unwrap_or_else(|| store::config_dir().join("steam"));
                let account_id = model::account_id(self.me);
                let q_addr = local::servers::format_ipv4_port(ip, query_port);
                let c_addr = local::servers::format_ipv4_port(ip, conn_port);
                let mut lists = local::servers::load_lists(&root, account_id);
                if flags & 1 != 0 || flags == 0 {
                    lists.favorites.retain(|s| s.addr != q_addr && s.addr != c_addr);
                }
                if flags & 2 != 0 {
                    lists.history.retain(|s| s.addr != q_addr && s.addr != c_addr);
                }
                let _ = local::servers::save_lists(&root, account_id, &lists);
                let _ = reply.send(Reply::Ok);
            }
            Request::FriendServers { app_id: filter_app } => {
                let servers = self
                    .personas
                    .values()
                    .filter(|p| filter_app == 0 || p.game_app_id == Some(filter_app))
                    .filter_map(|p| {
                        let srv = p.server.as_deref()?;
                        let (ip, port) = local::servers::parse_ipv4_port(srv)?;
                        Some(crate::ipc::protocol::GameServer {
                            ip,
                            query_port: port,
                            game_port: port,
                            app_id: p.game_app_id.unwrap_or(filter_app),
                            name: p.name.clone(),
                            ..Default::default()
                        })
                    })
                    .collect();
                let _ = reply.send(Reply::Servers { servers });
            }
            Request::PlayedWith { steam_id } => {
                let time = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                _sink(Event::CoplayRecorded { steam_id, app_id, time });
                let _ = reply.send(Reply::Ok);
            }
            Request::PromoItems => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_inventory::promo_items(&conn, app_id, me).await);
                });
            }
            Request::UgcDownload { id } => {
                let dir = self.launched_dirs.get(&app_id).cloned().unwrap_or_default();
                let cell = self.download_cell.unwrap_or(self.cell_id);
                tokio::spawn(async move {
                    let _ = reply.send(crate::game_ugc::download(&conn, app_id, me, id, &dir, cell).await);
                });
            }
            Request::Http { method, url, headers, params, body, timeout_ms } => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::ipc::http_request(method, &url, &headers, &params, body, timeout_ms).await);
                });
            }
            Request::EncryptedTicket { data } => {
                tokio::spawn(async move {
                    let r = match crate::ipc::encrypted_ticket(&conn, app_id, &data).await {
                        Ok(data) => Reply::Bytes { data },
                        Err(message) => Reply::Err { message },
                    };
                    let _ = reply.send(r);
                });
            }
            Request::NetworkingCert { public_key } => {
                tokio::spawn(async move {
                    let _ = reply.send(crate::ipc::networking_cert(&conn, app_id, &public_key).await);
                });
            }
            Request::Rendezvous { to, data } => {
                crate::ipc::rendezvous(&conn, app_id, me, to, &data);
                let _ = reply.send(Reply::Ok);
            }
        }
    }

    fn my_name(&self) -> String {
        self.personas.get(&self.me).map(|p| p.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| self.account.clone())
    }

    /// What a game learns about the player at start-up.
    fn ipc_session(&self, app_id: AppId) -> crate::ipc::protocol::Session {
        use crate::ipc::protocol::{Friend, Session};
        let install_dir = self.launched_dirs.get(&app_id).cloned().unwrap_or_default();
        let (language, beta, build_id) = crate::ipc::manifest_info(app_id, &install_dir);
        let ui_language = crate::ipc::steam_ui_language().unwrap_or_else(|| "english".into());
        let me = self.personas.get(&self.me);
        Session {
            app_id,
            steam_id: self.me,
            account: self.account.clone(),
            persona_name: me
                .map(|p| p.name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| store::find(&self.account).map(|a| a.persona_name).filter(|n| !n.is_empty()))
                .unwrap_or_else(|| self.account.clone()),
            country: self.country.clone(),
            game_language: language.unwrap_or_else(|| "english".into()),
            ui_language,
            level: 0,
            owned: self.apps.keys().copied().collect(),
            dlc: self.apps.values().filter(|a| a.parent == Some(app_id)).map(|a| (a.app_id, a.name.clone())).collect(),
            friends: self
                .friends
                .iter()
                .map(|(id, rel)| {
                    let p = self.personas.get(id);
                    Friend {
                        steam_id: *id,
                        name: p.map(|p| p.name.clone()).unwrap_or_default(),
                        state: p.map(|p| p.state.as_u32()).unwrap_or(0),
                        relationship: relationship_code(*rel),
                        game: p.and_then(|p| p.game_app_id).unwrap_or(0),
                    }
                })
                .collect(),
            install_dir: install_dir.to_string_lossy().into_owned(),
            steam_root: self.steam_root.clone().unwrap_or_else(|| store::config_dir().join("steam")).to_string_lossy().into_owned(),
            build_id,
            beta,
            stats: None,
            offline: self.link != Link::Online,
        }
    }
}

/// Connection changes and the offline cache.
impl Online {
    fn set_link(&mut self, link: Link, sink: &Sink) {
        if self.link != link {
            self.link = link;
            sink(Event::Link(link));
        }
    }

    /// Stop using the connection: it dropped, or the player chose "Go offline".
    /// Games keep running; their time from now on is kept here.
    fn drop_connection(&mut self, sink: &Sink, offline_mode: bool) -> (mpsc::UnboundedSender<Packet>, net::connection::Events) {
        if self.cache_dirty {
            self.save_cache();
        }
        self.conn = Connection::closed();
        self.generation += 1;
        self.probing = false;
        self.from_offline = false;
        let now = unix_now();
        for app in self.running.keys() {
            if !self.official.contains_key(app) {
                self.unconnected_play.entry(*app).or_insert((now, offline_mode));
            }
        }
        // Downloads wait for the connection (Steam pauses them too).
        if let Some((_, p)) = &self.active {
            if !p.paused.swap(true, std::sync::atomic::Ordering::Relaxed) {
                self.auto_paused = true;
            }
        }
        self.set_link(if offline_mode { Link::Offline } else { Link::Reconnecting { attempt: 1, retry_in: backoff(1) } }, sink);
        quiet_events()
    }

    /// Signed in again on a new connection: same session for the UI.
    fn reconnected(&mut self, conn: Connection, info: Logon, sink: &Sink) {
        tracing::info!("reconnected to Steam");
        self.conn = conn;
        self.generation += 1;
        self.last_rx = std::time::Instant::now();
        self.probing = false;
        self.from_offline = false;
        self.me = info.steam_id;
        self.cell_id = info.cell_id;
        self.auth = crate::ipc::Auth::new(info.public_ip);
        self.web = Default::default();
        self.record_active_unconnected_play();
        self.set_link(Link::Online, sink);
        sink(Event::LoggedIn { steam_id: self.me, account: self.account.clone() });
        self.connected(sink);
        if self.auto_paused {
            self.auto_paused = false;
            if let Some((_, p)) = &self.active {
                p.paused.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.flush_offline(sink);
        self.flush_cloud(sink);
        self.send_offline_lists(sink);
    }

    /// What a fresh connection needs: our status, the game we're in, level
    /// and notifications. Steam sends licenses, friends and personas itself.
    fn connected(&self, sink: &Sink) {
        let _ = set_persona(&self.conn, self.persona_state);
        if let Some(app) = self.playing {
            let _ = games_played(&self.conn, Some(app));
        }
        let (conn, sink2) = (self.conn.clone(), sink.clone());
        tokio::spawn(async move {
            let r: Result<CPlayerGetGameBadgeLevelsResponse, _> = conn.call("Player.GetGameBadgeLevels#1", &CPlayerGetGameBadgeLevelsRequest { appid: Some(0) }).await;
            if let Ok(r) = r {
                sink2(Event::Level(r.player_level()));
            }
        });
        self.load_notifications(sink);
        crate::chat_groups::load_my_groups(self.conn.clone(), self.me, self.active_groups.clone(), sink.clone());
    }

    /// Offline mode: the library and friends as they were at the last sign-in.
    fn start_offline(&mut self, cache: offline::AccountCache, sink: &Sink) {
        tracing::info!("starting {} in offline mode ({} apps from {})", self.account, cache.apps.len(), cache.saved);
        self.apps = cache.apps.iter().map(|a| (a.app_id, a.clone())).collect();
        self.friends = cache.friends.iter().copied().collect();
        self.country = cache.country.clone();
        for mut p in cache.personas {
            // Nobody's status is known offline.
            p.state = PersonaState::Offline;
            p.game_app_id = None;
            p.game_name = None;
            p.rich_status = None;
            self.personas.insert(p.steam_id, p);
        }
        let saved = store::find(&self.account);
        let name = Some(cache.persona_name.clone()).filter(|n| !n.is_empty()).or_else(|| saved.as_ref().map(|a| a.persona_name.clone())).unwrap_or_else(|| self.account.clone());
        let avatar = cache.avatar_hash.clone().or_else(|| saved.and_then(|a| a.avatar_hash));
        self.personas.insert(self.me, Persona { steam_id: self.me, name: name.clone(), state: PersonaState::Offline, avatar_hash: avatar, ..Default::default() });
        sink(Event::Link(Link::Offline));
        sink(Event::LoggedIn { steam_id: self.me, account: self.account.clone() });
        sink(Event::AccountInfo { persona_name: name, country: cache.country });
        sink(Event::Apps(cache.apps));
        sink(Event::Friends(self.friends.iter().map(|(k, v)| (*k, *v)).collect()));
        sink(Event::Personas(self.personas.values().cloned().collect()));
    }

    /// Offline play time and waiting cloud uploads, for the UI.
    fn send_offline_lists(&self, sink: &Sink) {
        sink(Event::OfflinePlaytime(offline::playtime_totals(&offline::play_sessions(self.me))));
        sink(Event::CloudPending(offline::cloud_pending(self.me).into_iter().map(|p| p.0).collect()));
    }

    /// Write the offline cache (only from live data).
    fn save_cache(&mut self) {
        if self.link != Link::Online || self.apps.is_empty() {
            return;
        }
        self.cache_dirty = false;
        self.cache_saved = std::time::Instant::now();
        let saved = store::find(&self.account);
        let me = self.personas.get(&self.me);
        let cache = offline::AccountCache {
            steam_id: self.me,
            account: self.account.clone(),
            persona_name: me.map(|p| p.name.clone()).filter(|n| !n.is_empty()).or_else(|| saved.as_ref().map(|a| a.persona_name.clone())).unwrap_or_default(),
            avatar_hash: me.and_then(|p| p.avatar_hash.clone()).or_else(|| saved.and_then(|a| a.avatar_hash)),
            country: self.country.clone(),
            apps: self.apps.values().cloned().collect(),
            friends: self.friends.iter().map(|(k, v)| (*k, *v)).collect(),
            personas: self
                .friends
                .keys()
                .filter_map(|id| self.personas.get(id))
                .map(|p| Persona { steam_id: p.steam_id, name: p.name.clone(), avatar_hash: p.avatar_hash.clone(), last_logoff: p.last_logoff, ..Default::default() })
                .collect(),
            saved: unix_now(),
        };
        tokio::task::spawn_blocking(move || offline::save_account(&cache));
    }

    /// Send a game's changed saves up; if that fails, try again once online.
    fn upload_saves(&self, app_id: AppId, target: crate::cloud::Target, sink: &Sink, retry: bool) {
        let lock = cloud_lock(self.me, app_id);
        let guard = lock.clone().try_lock_owned().ok();
        // A retry remains pending when busy. A newly finished game must upload
        // even when another task is already syncing an older version.
        if retry && guard.is_none() {
            return;
        }
        sink(Event::CloudSyncing { app_id });
        let (conn, sink, me) = (self.conn.clone(), sink.clone(), self.me);
        tokio::spawn(async move {
            let _guard = match guard { Some(guard) => guard, None => lock.lock_owned().await };
            let dir = target.install_dir.clone();
            let report = crate::cloud::sync(&conn, &target, crate::cloud::Mode::Upload, &[], &[]).await;
            if report.errors.is_empty() && report.conflicts.is_empty() {
                offline::remove_cloud_pending(me, app_id);
            } else {
                // Not reachable, or the player must pick a side first.
                offline::add_cloud_pending(me, app_id, &dir);
            }
            sink(Event::CloudPending(offline::cloud_pending(me).into_iter().map(|p| p.0).collect()));
            sink(Event::CloudReport { report, held_launch: None });
        });
    }

    /// Upload the saves of games played while offline.
    fn flush_cloud(&self, sink: &Sink) {
        // Needs the cloud settings of the library (and where userdata is).
        if self.link != Link::Online || self.apps.is_empty() || self.steam_root.is_none() {
            return;
        }
        for (app_id, dir) in offline::cloud_pending(self.me) {
            if self.running.contains_key(&app_id) {
                continue; // goes up when it closes
            }
            match self.cloud_target(app_id, &dir) {
                Some(t) => {
                    tracing::info!("uploading saves of {app_id} changed while offline");
                    self.upload_saves(app_id, t, sink, true);
                }
                // Cloud turned off for it, or no longer owned.
                None => offline::remove_cloud_pending(self.me, app_id),
            }
        }
    }

    /// Flush any currently running offline/disconnected game sessions into
    /// `playtime.json` (used on reconnect, logout, and shutdown).
    fn record_active_unconnected_play(&mut self) {
        let now = unix_now();
        for (app_id, (start, offline_mode)) in std::mem::take(&mut self.unconnected_play) {
            offline::record_play(self.me, offline::PlaySession { app_id, start, seconds: now.saturating_sub(start), offline: offline_mode, reported: false });
        }
    }

    /// Back online: send play time and game stats recorded while offline.
    fn flush_offline(&self, sink: &Sink) {
        let (conn, sink, me) = (self.conn.clone(), sink.clone(), self.me);
        tokio::spawn(async move {
            let sessions = offline::play_sessions(me);
            if offline::SEND_TO_STEAM {
                if let Some((req, unsent)) = build_disconnected_playtime_request(me, &sessions) {
                    // What Steam's own client sends for time played while disconnected.
                    match conn.call::<_, CPlayerRecordDisconnectedPlaytimeResponse>("Player.RecordDisconnectedPlaytime#1", &req).await {
                        Ok(_) => {
                            tracing::info!("sent {} offline play session(s) to Steam", unsent.len());
                            offline::mark_reported(me, &unsent);
                        }
                        Err(e) => tracing::warn!("offline play time not sent: {e}"),
                    }
                    sink(Event::OfflinePlaytime(offline::playtime_totals(&offline::play_sessions(me))));
                }
            }
            if offline::SEND_TO_STEAM {
                for app_id in offline::stats_pending(me) {
                    send_offline_stats(&conn, me, app_id).await;
                }
            }
        });
    }
}

/// Build a `Player.RecordDisconnectedPlaytime#1` request from unreported,
/// non-zero play sessions, returning `None` if nothing needs sending.
fn build_disconnected_playtime_request(
    me: SteamId,
    sessions: &[offline::PlaySession],
) -> Option<(CPlayerRecordDisconnectedPlaytimeRequest, Vec<offline::PlaySession>)> {
    let unsent: Vec<offline::PlaySession> = sessions.iter().filter(|s| !s.reported && s.seconds > 0).cloned().collect();
    if unsent.is_empty() {
        return None;
    }
    let owner = model::account_id(me);
    let req = CPlayerRecordDisconnectedPlaytimeRequest {
        play_sessions: unsent
            .iter()
            .map(|s| c_player_record_disconnected_playtime_request::PlayHistory {
                appid: Some(s.app_id),
                session_time_start: Some(s.start.min(u32::MAX as u64) as u32),
                seconds: Some(s.seconds.min(u32::MAX as u64) as u32),
                offline: Some(s.offline),
                owner: Some(owner),
            })
            .collect(),
    };
    Some((req, unsent))
}

/// Send stats and achievements a game stored while offline.
async fn send_offline_stats(conn: &Connection, me: SteamId, app_id: AppId) {
    let pending = offline::load_stats(me, app_id);
    if pending.pending.is_empty() && !pending.reset {
        return;
    }
    let current = match crate::ipc::user_stats(conn, app_id, me, "english").await {
        Ok(s) => s,
        Err(e) => return tracing::warn!("offline stats of {app_id} not sent: {e}"),
    };
    let values: Vec<(u32, u32)> = pending
        .pending
        .iter()
        .map(|(&id, &v)| match current.stats.iter().find(|s| s.id == id) {
            // Achievements unlocked elsewhere in the meantime stay unlocked.
            Some(s) if s.kind == 4 && !pending.reset => (id, v | s.value),
            _ => (id, v),
        })
        .collect();
    let (result, _) = crate::ipc::store_stats(conn, app_id, me, &values, current.crc, pending.reset).await;
    if result == eresult::OK {
        tracing::info!("sent {} stat(s) of {app_id} stored while offline", values.len());
        offline::stats_sent(me, app_id);
        if let Ok(fresh) = crate::ipc::user_stats(conn, app_id, me, "english").await {
            offline::remember_stats(me, app_id, &fresh);
        }
    } else {
        tracing::warn!("Steam didn't take the offline stats of {app_id}: result {result}");
    }
}


/// EFriendRelationship number.
fn relationship_code(r: Relationship) -> u32 {
    match r {
        Relationship::None => 0,
        Relationship::Blocked => 1,
        Relationship::RequestRecipient => 2,
        Relationship::Friend => 3,
        Relationship::RequestInitiator => 4,
        Relationship::Ignored => 5,
        Relationship::Other(v) => v,
    }
}


/// Steam's purchase result details for key activation (EPurchaseResultDetail).
fn key_error(detail: i32, result: i32) -> String {
    match detail {
        9 => "You already own this product",
        13 => "This product isn't available in your region",
        14 => "That key isn't valid",
        15 => "That key has already been used",
        24 => "This needs a game you don't own (for example, the base game)",
        53 => "Too many attempts. Try again in an hour",
        54 => "You own something that prevents this activation",
        _ => return net::eresult::describe(result, &format!("purchase detail {detail}")),
    }
    .into()
}

fn cart_lines(cart: Option<proto::webui::AccountCartContents>) -> Vec<model::CartLine> {
    cart.map(|c| {
        c.line_items
            .iter()
            .map(|l| model::CartLine {
                line_item_id: l.line_item_id(),
                package_id: l.packageid(),
                price: l.price_when_added.as_ref().map(|p| p.formatted_amount().to_string()).unwrap_or_default(),
                gift_to: l.gift_info.as_ref().map(|g| g.accountid_giftee() as u32).filter(|a| *a != 0),
                is_gift: l.flags.as_ref().is_some_and(|f| f.is_gift()) || l.gift_info.as_ref().is_some_and(|g| g.accountid_giftee() != 0),
            })
            .collect()
    })
    .unwrap_or_default()
}

fn econ_item(owner: SteamId, a: &CEconAsset, d: &CEconItemDescription) -> model::InventoryItem {
    let color = |c: &str| Some(c.trim_start_matches('#').to_string()).filter(|c| !c.is_empty());
    model::InventoryItem {
        asset_id: a.assetid(),
        class_id: a.classid(),
        amount: a.amount().max(1) as u64,
        name: if d.name().is_empty() { d.market_name().to_string() } else { d.name().to_string() },
        kind: d.r#type().to_string(),
        name_color: color(d.name_color()),
        background: color(d.background_color()),
        icon: d.icon_url().to_string(),
        icon_large: Some(d.icon_url_large().to_string()).filter(|s| !s.is_empty()),
        tradable: d.tradable(),
        marketable: d.marketable(),
        market_hash_name: d.market_hash_name().to_string(),
        description: d.descriptions.iter().filter(|l| !l.value().trim().is_empty()).map(|l| (l.value().to_string(), color(l.color()))).collect(),
        tags: d
            .tags
            .iter()
            .map(|t| (t.localized_category_name().to_string(), t.localized_tag_name().to_string(), color(t.color())))
            .collect(),
        actions: d
            .actions
            .iter()
            .map(|x| (x.name().to_string(), x.link().replace("%owner_steamid%", &owner.to_string()).replace("%assetid%", &a.assetid().to_string())))
            .collect(),
        context_id: a.contextid(),
        owner_notes: d.owner_descriptions.iter().map(|l| l.value().trim().to_string()).filter(|v| !v.is_empty()).collect(),
        trade_hold_days: d.market_tradable_restriction().max(0) as u32,
        market_hold_days: d.market_marketable_restriction().max(0) as u32,
        commodity: d.commodity(),
        market_fee: d.market_fee().parse().ok(),
        card_set: card_set(d.tags.iter().map(|t| (t.category(), t.internal_name())), d.market_hash_name()),
    }
}

/// A trading card's game and foil flag, from Steam's language-independent
/// tag names (`item_class_2` = trading card, `app_<id>`, `cardborder_1` =
/// foil); the market name ("<app>-Card name") is the fallback for the game.
fn card_set<'a>(tags: impl Iterator<Item = (&'a str, &'a str)> + Clone, hash_name: &str) -> Option<(AppId, bool)> {
    let tag = |cat: &str| tags.clone().find(|(c, _)| c.eq_ignore_ascii_case(cat)).map(|(_, n)| n);
    if tag("item_class")? != "item_class_2" {
        return None;
    }
    let app = tag("Game").and_then(|n| n.strip_prefix("app_")?.parse().ok()).or_else(|| hash_name.split_once('-')?.0.parse().ok())?;
    Some((app, tag("cardborder") == Some("cardborder_1")))
}

/// A profile's community visibility, from its public XML summary.
/// Visibility and VAC status from the profile's public XML summary.
async fn public_summary(steam_id: SteamId) -> (model::Visibility, bool) {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let http = HTTP.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap_or_default());
    let url = format!("https://steamcommunity.com/profiles/{steam_id}/?xml=1");
    let Ok(body) = async { http.get(url).send().await?.text().await }.await else { return (model::Visibility::Unknown, false) };
    let tag = |name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let start = body.find(&open)? + open.len();
        let end = body[start..].find('<')? + start;
        Some(body[start..end].trim().to_string())
    };
    let vis = match tag("visibilityState").as_deref() {
        Some("3") => model::Visibility::Public,
        Some("2") => model::Visibility::FriendsOnly,
        Some("1") => model::Visibility::Private,
        _ => model::Visibility::Unknown,
    };
    (vis, tag("vacBanned").as_deref() == Some("1"))
}


/// This run's session id: stable while the app runs (so reconnecting replaces
/// our own old session), different from any other run or tool signed in to
/// the same account.
fn logon_id() -> u32 {
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let mixed = (nanos as u64) ^ ((std::process::id() as u64) << 32);
        let mixed = mixed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        ((mixed >> 32) as u32).max(1)
    })
}

/// Workshop downloads for a game, if it's installed (found in the Steam libraries).
async fn sync_installed(
    conn: Connection,
    me: SteamId,
    cell: u32,
    root: Option<std::path::PathBuf>,
    app_id: AppId,
    items: Vec<u64>,
    options: crate::workshop::SyncOptions,
    sink: Sink,
) {
    let library = tokio::task::spawn_blocking(move || local::load(root).ok().and_then(|l| l.games.into_iter().find(|g| g.app_id == app_id)).map(|g| g.library)).await.ok().flatten();
    let Some(library) = library else { return };
    let result = crate::workshop::sync(&conn, me, app_id, &library, cell, items, options).await;
    sink(Event::WorkshopSync { app_id, result });
}

/// "+connect_lobby <id>" in a connect string or command line → the lobby.
fn lobby_of(connect: &str) -> u64 {
    let mut words = connect.split_whitespace();
    while let Some(w) = words.next() {
        if w == "+connect_lobby" {
            return words.next().and_then(|id| id.parse().ok()).unwrap_or(0);
        }
    }
    0
}

/// `steam://run/<app>//<args>`: Steam starts the game with these arguments.
fn invite_link(app_id: AppId, connect: &str) -> String {
    let args: String = connect
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~+".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("steam://run/{app_id}//{args}")
}

/// Fetch the user's Steam Family status, request personas for non-friend
/// members/inviters, and emit `Event::Family`.
async fn reload_family(gate: &tokio::sync::Mutex<()>, conn: &Connection, sink: &Sink, me: SteamId, language: &str) {
    let _guard = gate.lock().await;
    let r = crate::account::family(conn, me, language).await;
    if let Ok(f) = &r {
        let mut ids: Vec<SteamId> = f.invites.iter().map(|i| i.inviter).collect();
        if let Some(g) = &f.family {
            ids.extend(g.members.iter().map(|m| m.steam_id).chain(g.invites.iter().map(|i| i.0)));
        }
        ids.retain(|id| *id != me && *id != 0);
        if !ids.is_empty() {
            let _ = conn.send(emsg::CLIENT_REQUEST_FRIEND_DATA, &CMsgClientRequestFriendData { persona_state_requested: Some(PERSONA_FLAGS), friends: ids });
        }
    }
    sink(Event::Family { steam_id: me, result: r });
}

#[cfg(test)]
mod invite_tests {
    #[test]
    fn connect_strings() {
        assert_eq!(super::lobby_of("+connect_lobby 109775243904435019"), 109775243904435019);
        assert_eq!(super::lobby_of("-dev +connect 1.2.3.4"), 0);
        assert_eq!(super::invite_link(477160, "+connect_lobby 5"), "steam://run/477160//+connect_lobby%205");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cloud_operations_share_a_lock_only_for_the_same_account_and_app() {
        let guard = super::cloud_lock(10, 20).try_lock_owned().unwrap();
        assert!(super::cloud_lock(10, 20).try_lock_owned().is_err());
        assert!(super::cloud_lock(11, 20).try_lock_owned().is_ok());
        assert!(super::cloud_lock(10, 21).try_lock_owned().is_ok());
        drop(guard);
        assert!(super::cloud_lock(10, 20).try_lock_owned().is_ok());
    }

    #[test]
    fn trading_cards_know_their_set() {
        let card = [("Game", "app_365360"), ("item_class", "item_class_2"), ("cardborder", "cardborder_0")];
        assert_eq!(super::card_set(card.iter().copied(), "365360-The Goblins"), Some((365360, false)));
        let foil = [("item_class", "item_class_2"), ("cardborder", "cardborder_1")];
        assert_eq!(super::card_set(foil.iter().copied(), "20920-Zoltan (Foil)"), Some((20920, true)));
        let emoticon = [("Game", "app_365360"), ("item_class", "item_class_4")];
        assert_eq!(super::card_set(emoticon.iter().copied(), "365360-:goblin:"), None);
        assert_eq!(super::card_set([("Rarity", "Rare")].iter().copied(), "AK-47 | Redline"), None);
    }
}

fn validate_support_ticket_url(url: &str) -> Result<reqwest::Url, String> {
    crate::market::validate_steam_url(url, "help.steampowered.com").map_err(|_| "not a Steam Support link".to_string())
}

/// The queue in the order asked for; games not in `order` follow, in the
/// order they were, so nothing is ever dropped.
fn reorder(queue: std::collections::VecDeque<crate::download::InstallRequest>, order: &[AppId]) -> std::collections::VecDeque<crate::download::InstallRequest> {
    let mut rest: Vec<_> = queue.into_iter().collect();
    let mut out = std::collections::VecDeque::with_capacity(rest.len());
    for id in order {
        if let Some(i) = rest.iter().position(|r| r.app_id == *id) {
            out.push_back(rest.remove(i));
        }
    }
    out.extend(rest);
    out
}

#[cfg(test)]
mod reorder_tests {
    use super::reorder;

    fn q(ids: &[u32]) -> std::collections::VecDeque<crate::download::InstallRequest> {
        ids.iter().map(|&app_id| crate::download::InstallRequest { app_id, library: Default::default(), steam_root: Default::default(), branch: String::new(), language: String::new(), skip_dlc: Vec::new(), add_dlc: Vec::new(), platform: String::new() }).collect()
    }
    fn ids(q: &std::collections::VecDeque<crate::download::InstallRequest>) -> Vec<u32> {
        q.iter().map(|r| r.app_id).collect()
    }

    #[test]
    fn moves_and_keeps_the_rest() {
        assert_eq!(ids(&reorder(q(&[1, 2, 3, 4]), &[3, 1])), vec![3, 1, 2, 4]);
        // Unknown ids are ignored; nothing is dropped or duplicated.
        assert_eq!(ids(&reorder(q(&[1, 2]), &[9, 2, 2])), vec![2, 1]);
        assert_eq!(ids(&reorder(q(&[]), &[1])), Vec::<u32>::new());
    }

    #[test]
    fn support_ticket_url_rejects_traversal_and_bad_origins() {
        for bad in [
            "https://help.steampowered.com/en/%2e%2e/admin",
            "https://help.steampowered.com/en/../admin",
            "https://help.steampowered.com.evil.tld/en/",
            "https://user@help.steampowered.com/en/",
            "http://help.steampowered.com/en/",
        ] {
            assert!(super::validate_support_ticket_url(bad).is_err(), "must reject {bad}");
        }
        assert!(super::validate_support_ticket_url("https://help.steampowered.com/en/wizard/HelpRequest/HT-1234").is_ok());
    }

    #[test]
    fn disconnected_playtime_request_filters_and_maps_fields() {
        let steam_id: model::SteamId = 76561198000000123;
        let expected_owner = model::account_id(steam_id);
        let sessions = vec![
            crate::offline::PlaySession { app_id: 440, start: 1_700_000_000, seconds: 300, offline: true, reported: false },
            // Already reported: must be skipped.
            crate::offline::PlaySession { app_id: 570, start: 1_700_000_500, seconds: 600, offline: true, reported: true },
            // Zero seconds: must be skipped.
            crate::offline::PlaySession { app_id: 730, start: 1_700_000_900, seconds: 0, offline: false, reported: false },
            // Disconnected (not offline mode) with large seconds clamped to u32::MAX.
            crate::offline::PlaySession { app_id: 730, start: 1_700_001_000, seconds: u64::from(u32::MAX) + 50, offline: false, reported: false },
        ];

        let (req, unsent) = super::build_disconnected_playtime_request(steam_id, &sessions).expect("should build request");
        assert_eq!(unsent.len(), 2);
        assert_eq!(req.play_sessions.len(), 2);

        assert_eq!(req.play_sessions[0].appid, Some(440));
        assert_eq!(req.play_sessions[0].session_time_start, Some(1_700_000_000));
        assert_eq!(req.play_sessions[0].seconds, Some(300));
        assert_eq!(req.play_sessions[0].offline, Some(true));
        assert_eq!(req.play_sessions[0].owner, Some(expected_owner));

        assert_eq!(req.play_sessions[1].appid, Some(730));
        assert_eq!(req.play_sessions[1].session_time_start, Some(1_700_001_000));
        assert_eq!(req.play_sessions[1].seconds, Some(u32::MAX));
        assert_eq!(req.play_sessions[1].offline, Some(false));
        assert_eq!(req.play_sessions[1].owner, Some(expected_owner));

        // When all sessions are already reported or zero-second, returns None.
        let none = super::build_disconnected_playtime_request(steam_id, &sessions[1..3]);
        assert!(none.is_none());
    }
}

/// The wishlist in its own order ("Your rank"), as (game, date added).
fn wishlist_in_order(items: &[proto::webui::CWishlistGetWishlistResponseWishlistItem]) -> Vec<(AppId, u64)> {
    let mut ranked: Vec<crate::wishlist::Ranked> =
        items.iter().map(|i| crate::wishlist::Ranked { app_id: i.appid(), priority: i.priority(), added: i.date_added() as u64 }).collect();
    crate::wishlist::sort(&mut ranked);
    ranked.into_iter().map(|r| (r.app_id, r.added)).collect()
}

/// Fetch the signed-in user's Steam Points balance via `ILoyaltyRewardsService/GetSummary/v1/`.
async fn fetch_points_balance(conn: &net::Connection, refresh_token: &str, me: SteamId) -> Result<u64, String> {
    if refresh_token.is_empty() || me == 0 {
        return Err("Not signed in".into());
    }
    let token_req = proto::CAuthenticationAccessTokenGenerateForAppRequest {
        refresh_token: Some(refresh_token.to_string()),
        steamid: Some(me),
        ..Default::default()
    };
    let tok: proto::CAuthenticationAccessTokenGenerateForAppResponse = conn
        .call("Authentication.GenerateAccessTokenForApp#1", &token_req)
        .await
        .map_err(|e| e.to_string())?;
    let access = tok.access_token();
    if access.is_empty() {
        return Err("No access token".into());
    }
    let v: serde_json::Value = crate::http()
        .get("https://api.steampowered.com/ILoyaltyRewardsService/GetSummary/v1/")
        .timeout(std::time::Duration::from_secs(12))
        .query(&[("access_token", access), ("steamid", &me.to_string())])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let pts = v
        .get("response")
        .and_then(|r| r.get("summary"))
        .and_then(|s| s.get("points"))
        .and_then(|p| p.as_u64().or_else(|| p.as_str().and_then(|s| s.parse().ok())))
        .ok_or_else(|| "Steam didn't return a points balance".to_string())?;
    Ok(pts)
}
