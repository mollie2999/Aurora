//! Calling the engine interface of Valve's steamclient (loaded by `sys`).
//!
//! Slot numbers are for the pinned build only (see `layout`): Valve reorders
//! these interfaces between builds, so nothing here may run on another one.

use crate::sys::{self, Lib, class_name};
use std::ffi::{c_char, c_int, c_void};
use std::path::Path;

/// `IClientEngine` (CLIENTENGINE_INTERFACE_VERSION005) slots, recovered from
/// this build's `CSteamClient` vtable: the three that open the `Steam3Master`
/// IPC server, and the assert strings in slots 5 and 12.
mod layout {
    pub const CREATE_STEAM_PIPE: usize = 0;
    pub const CREATE_GLOBAL_USER: usize = 2;
    /// Getters shaped `(hUser, hPipe) -> object` (lookup under the engine lock).
    pub const USER_GETTERS: &[usize] = &[
        8, 9, 10, 13, 15, 16, 17, 18, 21, 22, 23, 24, 25, 27, 34, 35, 43, 44, 46, 47, 50, 51, 52, 54, 55, 57, 58,
        59, 60, 62, 63, 64, 65, 67, 69, 70, 71, 72, 74, 75, 76, 79,
    ];
    /// Getters shaped `(hPipe) -> object` (lookup in m_mapClientInterfaces).
    pub const PIPE_GETTERS: &[usize] = &[14, 36, 37, 38, 39, 40, 41, 53, 56, 61, 66, 77, 78];
}

type HSteamPipe = c_int;
type HSteamUser = c_int;

/// Function pointer in slot `i` of `obj`'s vtable.
///
/// SAFETY: `obj` must be a live C++ object whose vtable has more than `i`
/// slots, and F must match that method's real signature.
unsafe fn slot<F: Copy>(obj: *mut c_void, i: usize) -> F {
    unsafe {
        let vt = *(obj as *const *const *const c_void);
        std::mem::transmute_copy(&*vt.add(i))
    }
}

fn engine(lib: &Lib) -> Result<*mut c_void, String> {
    type CreateInterface = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;
    let create: CreateInterface = lib.sym("CreateInterface")?;
    let mut rc: c_int = 0;
    // SAFETY: Valve's exported factory, called with a valid version string.
    let e = unsafe { create(c"CLIENTENGINE_INTERFACE_VERSION005".as_ptr(), &mut rc) };
    if e.is_null() {
        return Err(format!("CreateInterface(CLIENTENGINE_INTERFACE_VERSION005) failed ({rc})"));
    }
    match class_name(e).as_deref() {
        Some(c) if c == sys::ENGINE_CLASS => Ok(e),
        other => Err(format!("unexpected engine class {other:?}: not the pinned build")),
    }
}

/// Become the global Steam instance: Valve's engine opens its `Steam3Master`
/// IPC server in this process, which games then connect to.
fn global_instance(lib: &Lib) -> Result<(*mut c_void, HSteamPipe, HSteamUser), String> {
    let e = engine(lib)?;
    sys::verify_slot(e, layout::CREATE_STEAM_PIPE)?;
    sys::verify_slot(e, layout::CREATE_GLOBAL_USER)?;
    // SAFETY: slots and signatures from `layout`, verified inside steamclient code.
    let (pipe, user) = unsafe {
        let create_pipe: unsafe extern "C" fn(*mut c_void) -> HSteamPipe = slot(e, layout::CREATE_STEAM_PIPE);
        let create_global: unsafe extern "C" fn(*mut c_void, *mut HSteamPipe) -> HSteamUser =
            slot(e, layout::CREATE_GLOBAL_USER);
        let mut pipe = create_pipe(e);
        let user = create_global(e, &mut pipe);
        (pipe, user)
    };
    println!("global instance: pipe={pipe} user={user}");
    if user == 0 {
        return Err("CreateGlobalUser returned 0".into());
    }
    Ok((e, pipe, user))
}

fn hold() {
    let secs: u64 = std::env::var("STEAMHOST_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    println!("holding the global instance for {secs}s");
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

/// `IClientUser` slots in this build, named from the ("IClientUser",
/// "<method>") strings its IPC wrappers pass; signatures from how each one
/// serializes its arguments and reply.
mod user {
    pub const GETTER: usize = 8; // IClientEngine::GetIClientUser(hUser, hPipe)
    pub const LOG_ON: usize = 1; // EResult LogOn(CSteamID)
    pub const B_LOGGED_ON: usize = 4; // bool BLoggedOn()
    pub const GET_LOGON_STATE: usize = 5; // int GetLogonState()
    pub const SET_LOGIN_TOKEN: usize = 56; // void SetLoginToken(const char* token, const char* account)
}

/// The printed line that tells a parent process the host is signed in and
/// ready for games to connect. Diagnostics go to stderr and profile requests
/// use their own stdout prefix.
pub const READY: &str = "STEAMHOST-READY";

fn zeroize_slice(buf: &mut [u8]) {
    for b in buf {
        // SAFETY: volatile write to valid mutable byte prevents dead-store elimination.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Refresh token held as a NUL-terminated buffer that volatile-zeroizes on use
/// and on drop.
struct SecretCString(Vec<u8>);

impl SecretCString {
    fn read(input: &mut impl std::io::BufRead, what: &str) -> Result<Self, String> {
        use std::io::Read;
        // Reserve once so growing the buffer never leaves a freed token copy.
        const MAX: usize = 64 * 1024;
        let mut secret = Self(Vec::with_capacity(MAX + 1));
        let n = std::io::BufRead::read_until(&mut input.take(MAX as u64 + 1), b'\n', &mut secret.0)
            .map_err(|e| e.to_string())?;
        if n == 0 { return Err(format!("stdin: missing {what}")); }
        if n > MAX { return Err(format!("{what} exceeds length limit")); }
        let text = std::str::from_utf8(&secret.0).map_err(|_| format!("invalid UTF-8 in {what}"))?;
        let start = text.len() - text.trim_start().len();
        let len = text.trim().len();
        if text.as_bytes().contains(&0) { return Err(format!("nul byte in {what}")); }
        secret.0.copy_within(start..start + len, 0);
        zeroize_slice(&mut secret.0[len..]);
        secret.0.truncate(len);
        secret.0.push(0);
        Ok(secret)
    }

    fn as_ptr(&self) -> *const c_char {
        self.0.as_ptr().cast()
    }

    fn zeroize(&mut self) {
        zeroize_slice(&mut self.0);
    }
}

impl Drop for SecretCString {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Account name, refresh token and SteamID, read one per line from stdin —
/// never argv/env, which other processes can read.
fn read_credentials(
    input: &mut impl std::io::BufRead,
) -> Result<(SecretCString, SecretCString, u64), String> {
    let account = SecretCString::read(input, "account name")?;
    let token = SecretCString::read(input, "refresh token")?;
    let id = SecretCString::read(input, "SteamID")?;
    let steam_id = std::str::from_utf8(&id.0[..id.0.len()-1]).unwrap().parse().map_err(|_| "invalid SteamID")?;
    Ok((account, token, steam_id))
}

/// A signed-in headless instance. Holding it keeps Valve's library loaded and
/// the global instance alive; the process exits to end the session.
struct Session {
    _lib: Lib,
}

/// Become the global instance and sign in with the given credentials, waiting
/// until the engine reports logged-on (or timing out).
fn sign_in(root: &Path, account: SecretCString, mut token: SecretCString, steam_id: u64, forward_profiles: bool) -> Result<Session, String> {
    let lib = Lib::open(root)?;
    if forward_profiles {
        #[cfg(all(windows, target_arch = "x86_64"))]
        match sys::install_profile_forwarding(root) {
            Ok(()) => eprintln!("profile forwarding enabled"),
            Err(e) => eprintln!("profile forwarding unavailable: {e}"),
        }
    }
    let (e, pipe, h_user) = global_instance(&lib)?;
    sys::verify_slot(e, user::GETTER)?;
    // SAFETY: getter slot shaped (this, hUser, hPipe), verified in `steamclient`.
    let cu = unsafe {
        let get: unsafe extern "C" fn(*mut c_void, HSteamUser, HSteamPipe) -> *mut c_void = slot(e, user::GETTER);
        get(e, h_user, pipe)
    };
    if class_name(cu).as_deref() != Some(sys::USER_CLASS) {
        return Err("unsupported Steam build: GetIClientUser didn't return IClientUser".into());
    }
    // Fail closed if any required IClientUser vtable slot (0..=SET_LOGIN_TOKEN)
    // is null or outside steamclient's executable code segment.
    sys::verify_vtable(cu, user::SET_LOGIN_TOKEN)?;
    // SAFETY: IClientUser vtable slots 0..=SET_LOGIN_TOKEN verified above.
    unsafe {
        let set_token: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) = slot(cu, user::SET_LOGIN_TOKEN);
        set_token(cu, token.as_ptr(), account.as_ptr());
    }
    token.zeroize();
    drop(token);
    unsafe {
        let log_on: unsafe extern "C" fn(*mut c_void, u64) -> c_int = slot(cu, user::LOG_ON);
        eprintln!("LogOn -> EResult {}", log_on(cu, steam_id));
    }
    let logged_on: unsafe extern "C" fn(*mut c_void) -> bool = unsafe { slot(cu, user::B_LOGGED_ON) };
    let state: unsafe extern "C" fn(*mut c_void) -> c_int = unsafe { slot(cu, user::GET_LOGON_STATE) };
    let mut last = -1;
    for _ in 0..120 {
        // SAFETY: as above.
        let (s, ok) = unsafe { (state(cu), logged_on(cu)) };
        if s != last {
            eprintln!("logon state {s}");
            last = s;
        }
        if ok {
            return Ok(Session { _lib: lib });
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err("not logged on after 60s".into())
}

/// Sign in, then hold the instance for a while (manual testing).
pub fn login(root: &Path) -> Result<(), String> {
    let mut lines = std::io::stdin().lock();
    let (account, token, steam_id) = read_credentials(&mut lines)?;
    let _session = sign_in(root, account, token, steam_id, false)?;
    eprintln!("logged on");
    hold();
    Ok(())
}

/// Sign in, print [`READY`], then serve games until the parent closes our
/// stdin (or sends EOF); then shut down by exiting. This is what the app runs
/// for a DRM game: it stays up only as long as the parent holds the pipe.
pub fn serve(root: &Path) -> Result<(), String> {
    let mut lines = std::io::stdin().lock();
    let (account, token, steam_id) = read_credentials(&mut lines)?;
    let forward_profiles = std::env::var_os("AURORA_STEAMHOST_PROFILE_FORWARDING").is_some_and(|value| value == "1");
    let _session = sign_in(root, account, token, steam_id, forward_profiles)?;
    println!("{READY}");
    use std::io::Write;
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    eprintln!("ready; serving until stdin closes");
    // Block until the parent drops the pipe (EOF) or sends a line.
    std::io::copy(&mut lines, &mut std::io::sink()).map_err(|e| e.to_string())?;
    eprintln!("stdin closed; shutting down");
    Ok(())
}

pub fn probe(root: &Path) -> Result<(), String> {
    let lib = Lib::open(root)?;
    let (e, pipe, user) = global_instance(&lib)?;
    for &i in layout::USER_GETTERS {
        sys::verify_slot(e, i)?;
        // SAFETY: lookup-only getter shaped (this, hUser, hPipe), slot verified.
        let f: unsafe extern "C" fn(*mut c_void, HSteamUser, HSteamPipe) -> *mut c_void = unsafe { slot(e, i) };
        let o = unsafe { f(e, user, pipe) };
        println!("slot {i:2} (user,pipe) -> {}", class_name(o).unwrap_or_else(|| format!("{o:?}")));
    }
    for &i in layout::PIPE_GETTERS {
        sys::verify_slot(e, i)?;
        // SAFETY: lookup-only getter shaped (this, hPipe), slot verified.
        let f: unsafe extern "C" fn(*mut c_void, HSteamPipe) -> *mut c_void = unsafe { slot(e, i) };
        let o = unsafe { f(e, pipe) };
        println!("slot {i:2} (pipe)      -> {}", class_name(o).unwrap_or_else(|| format!("{o:?}")));
    }
    hold();
    Ok(())
}

pub fn client_test(root: &Path) -> Result<(), String> {
    // A game process loads the same library and connects over IPC.
    let lib = sys::for_client(root)?;
    let create_pipe: unsafe extern "C" fn() -> HSteamPipe = lib.sym("Steam_CreateSteamPipe")?;
    let connect: unsafe extern "C" fn(HSteamPipe) -> HSteamUser = lib.sym("Steam_ConnectToGlobalUser")?;
    // SAFETY: Valve's flat exports with their public Steamworks signatures.
    let pipe = unsafe { create_pipe() };
    println!("client pipe={pipe}");
    if pipe == 0 {
        return Err("no Steam3Master to connect to".into());
    }
    let user = unsafe { connect(pipe) };
    println!("client user={user}");
    if user == 0 { Err("ConnectToGlobalUser refused".into()) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_cstring_zeroizes_bytes_immediately() {
        let mut secret = SecretCString::read(&mut &b"  eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.secret \r\n"[..], "token").unwrap();
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(secret.as_ptr()).to_str().unwrap() }, "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.secret");
        secret.zeroize();
        assert!(secret.0.iter().all(|&b| b == 0), "token buffer must be completely zeroed");
    }

    #[test]
    fn verify_vtable_fails_closed_on_invalid_object() {
        let err = sys::verify_vtable(std::ptr::null_mut(), user::SET_LOGIN_TOKEN).unwrap_err();
        assert!(err.contains("unsupported Steam build"), "got: {err}");
    }
}
