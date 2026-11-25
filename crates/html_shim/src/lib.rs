//! Linux `LD_PRELOAD` shim for games that call `ISteamHTMLSurface::CreateBrowser`
//! while running under Aurora's headless `steamhost` (which runs Valve's
//! `steamclient.so` without `steamui.so` or `steamwebhelper`).
//!
//! Without `steamwebhelper`, `ISteamHTMLSurface::CreateBrowser()` returns a
//! `SteamAPICall_t` that never completes (`HTML_BrowserReady_t`, callback 4501),
//! causing games such as Microsoft Flight Simulator (2020) to hang indefinitely
//! on their boot splash screen.
//!
//! This shim intercepts only `steamclient.so`'s `CreateInterface` and callback
//! exports via `dlsym`, wraps `ISteamClient` (`SteamClient016`..`023`) to
//! return a headless `ISteamHTMLSurface` implementation whose `Init()` returns
//! `true` and whose `CreateBrowser()` immediately completes with a synthetic
//! `HTML_BrowserReady_t` handle. All other interfaces and callbacks are
//! forwarded untouched to Valve's `steamclient.so`.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

const K_I_STEAM_API_CALL_COMPLETED: i32 = 703;
const K_I_HTML_BROWSER_READY: i32 = 4501;

/// Synthetic `SteamAPICall_t` range unlikely to collide with Valve's sequential IDs.
const FAKE_CALL_BASE: u64 = 0x7A7A_0000_0000_0001;
const MAX_COMPLETED: usize = 64;

#[repr(C)]
struct CallbackMsg {
    h_steam_user: i32,
    i_callback: i32,
    pub_param: *mut u8,
    cub_param: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SteamApiCallCompleted {
    h_async_call: u64,
    i_callback: i32,
    cub_param: u32,
}

#[derive(Default)]
struct ShimState {
    pending: VecDeque<(u64, u32)>,
    completed: VecDeque<(u64, u32)>,
    delivering: Option<SteamApiCallCompleted>,
}

static STATE: Mutex<ShimState> = Mutex::new(ShimState {
    pending: VecDeque::new(),
    completed: VecDeque::new(),
    delivering: None,
});
static NEXT_CALL: AtomicU64 = AtomicU64::new(FAKE_CALL_BASE);
static NEXT_HANDLE: AtomicU32 = AtomicU32::new(1);

fn is_fake_call(call: u64) -> bool {
    call >= FAKE_CALL_BASE && call < FAKE_CALL_BASE.saturating_add(0x1000_0000)
}

fn enqueue_browser_ready() -> u64 {
    let call = NEXT_CALL.fetch_add(1, Ordering::Relaxed);
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed).max(1);
    if let Ok(mut st) = STATE.lock() {
        st.pending.push_back((call, handle));
        if st.completed.len() >= MAX_COMPLETED {
            st.completed.pop_front();
        }
        st.completed.push_back((call, handle));
    }
    call
}

fn pop_completed_callback() -> Option<*mut SteamApiCallCompleted> {
    let mut st = STATE.lock().ok()?;
    if st.delivering.is_some() {
        return None;
    }
    let (call, _handle) = st.pending.pop_front()?;
    let payload = SteamApiCallCompleted {
        h_async_call: call,
        i_callback: K_I_HTML_BROWSER_READY,
        cub_param: 4,
    };
    st.delivering = Some(payload);
    // Point to a dedicated static buffer that stays valid until FreeLastCallback.
    // SAFETY: only mutated while `st.delivering` is None and read until cleared.
    unsafe {
        DELIVERING_PAYLOAD = payload;
        Some(&raw mut DELIVERING_PAYLOAD)
    }
}

static mut DELIVERING_PAYLOAD: SteamApiCallCompleted = SteamApiCallCompleted {
    h_async_call: 0,
    i_callback: 0,
    cub_param: 0,
};

fn finish_delivering() -> bool {
    let Ok(mut st) = STATE.lock() else { return false };
    st.delivering.take().is_some()
}

fn take_api_call_result(call: u64) -> Option<u32> {
    let mut st = STATE.lock().ok()?;
    let pos = st.completed.iter().position(|(c, _)| *c == call)?;
    Some(st.completed.remove(pos)?.1)
}

fn has_api_call_result(call: u64) -> bool {
    STATE.lock().ok().is_some_and(|st| st.completed.iter().any(|(c, _)| *c == call))
}

// ------------------------------------------------ ISteamHTMLSurface vtable

extern "C" fn html_init(_self: *mut c_void) -> u8 {
    1
}

extern "C" fn html_shutdown(_self: *mut c_void) -> u8 {
    1
}

extern "C" fn html_create_browser(_self: *mut c_void, _ua: *const c_char, _css: *const c_char) -> u64 {
    enqueue_browser_ready()
}

extern "C" fn html_stub(_self: *mut c_void) -> usize {
    0
}

// Itanium C++ ABI: virtual destructor occupies slots 0 and 1,
// so Init=2, Shutdown=3, CreateBrowser=4, RemoveBrowser=5, ...
const HTML_VT_LEN: usize = 48;

#[repr(C)]
struct HtmlVtable([*const c_void; HTML_VT_LEN]);
unsafe impl Sync for HtmlVtable {}

static HTML_VTABLE: HtmlVtable = {
    let mut vt = [html_stub as *const c_void; HTML_VT_LEN];
    vt[2] = html_init as *const c_void;
    vt[3] = html_shutdown as *const c_void;
    vt[4] = html_create_browser as *const c_void;
    HtmlVtable(vt)
};

#[repr(C)]
struct HtmlObject {
    vptr: *const *const c_void,
}
unsafe impl Sync for HtmlObject {}

static HTML_OBJ: HtmlObject = HtmlObject { vptr: HTML_VTABLE.0.as_ptr() };

// ------------------------------------------------ ISteamClient wrapper

static REAL_CLIENT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CLIENT_WRAPPED: AtomicBool = AtomicBool::new(false);

type GetGenericFn = unsafe extern "C" fn(*mut c_void, i32, i32, *const c_char) -> *mut c_void;
type GetHtmlFn = unsafe extern "C" fn(*mut c_void, i32, i32, *const c_char) -> *mut c_void;

unsafe extern "C" fn my_get_generic(
    _self: *mut c_void,
    user: i32,
    pipe: i32,
    ver: *const c_char,
) -> *mut c_void {
    if !ver.is_null() {
        if let Ok(s) = unsafe { CStr::from_ptr(ver) }.to_str() {
            if s.starts_with("STEAMHTMLSURFACE_INTERFACE_VERSION") {
                return &raw const HTML_OBJ as *mut c_void;
            }
        }
    }
    let real = REAL_CLIENT.load(Ordering::Acquire);
    if real.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let vt = *(real as *const *const *const c_void);
        let f: GetGenericFn = std::mem::transmute(*vt.add(12));
        f(real, user, pipe, ver)
    }
}

unsafe extern "C" fn my_get_html_surface(
    _self: *mut c_void,
    _user: i32,
    _pipe: i32,
    ver: *const c_char,
) -> *mut c_void {
    if !ver.is_null() {
        if let Ok(s) = unsafe { CStr::from_ptr(ver) }.to_str() {
            if s.starts_with("STEAMHTMLSURFACE_INTERFACE_VERSION") {
                return &raw const HTML_OBJ as *mut c_void;
            }
        }
    }
    let real = REAL_CLIENT.load(Ordering::Acquire);
    if real.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let vt = *(real as *const *const *const c_void);
        let f: GetHtmlFn = std::mem::transmute(*vt.add(30));
        f(real, _user, _pipe, ver)
    }
}

macro_rules! client_thunk {
    ($name:ident, $slot:expr) => {
        unsafe extern "C" fn $name(
            _self: *mut c_void,
            a: usize,
            b: usize,
            c: usize,
            d: usize,
            e: usize,
        ) -> usize {
            let real = REAL_CLIENT.load(Ordering::Acquire);
            if real.is_null() {
                return 0;
            }
            type Fn5 = unsafe extern "C" fn(*mut c_void, usize, usize, usize, usize, usize) -> usize;
            unsafe {
                let vt = *(real as *const *const *const c_void);
                let f: Fn5 = std::mem::transmute(*vt.add($slot));
                f(real, a, b, c, d, e)
            }
        }
    };
}

client_thunk!(ct_0, 0);
client_thunk!(ct_1, 1);
client_thunk!(ct_2, 2);
client_thunk!(ct_3, 3);
client_thunk!(ct_4, 4);
client_thunk!(ct_5, 5);
client_thunk!(ct_6, 6);
client_thunk!(ct_7, 7);
client_thunk!(ct_8, 8);
client_thunk!(ct_9, 9);
client_thunk!(ct_10, 10);
client_thunk!(ct_11, 11);
client_thunk!(ct_13, 13);
client_thunk!(ct_14, 14);
client_thunk!(ct_15, 15);
client_thunk!(ct_16, 16);
client_thunk!(ct_17, 17);
client_thunk!(ct_18, 18);
client_thunk!(ct_19, 19);
client_thunk!(ct_20, 20);
client_thunk!(ct_21, 21);
client_thunk!(ct_22, 22);
client_thunk!(ct_23, 23);
client_thunk!(ct_24, 24);
client_thunk!(ct_25, 25);
client_thunk!(ct_26, 26);
client_thunk!(ct_27, 27);
client_thunk!(ct_28, 28);
client_thunk!(ct_29, 29);
client_thunk!(ct_31, 31);
client_thunk!(ct_32, 32);
client_thunk!(ct_33, 33);
client_thunk!(ct_34, 34);
client_thunk!(ct_35, 35);
client_thunk!(ct_36, 36);
client_thunk!(ct_37, 37);
client_thunk!(ct_38, 38);
client_thunk!(ct_39, 39);
client_thunk!(ct_40, 40);
client_thunk!(ct_41, 41);
client_thunk!(ct_42, 42);
client_thunk!(ct_43, 43);

#[repr(C)]
struct ClientVtable([*const c_void; 44]);
unsafe impl Sync for ClientVtable {}

static CLIENT_VTABLE: ClientVtable = ClientVtable([
    ct_0 as *const c_void,
    ct_1 as *const c_void,
    ct_2 as *const c_void,
    ct_3 as *const c_void,
    ct_4 as *const c_void,
    ct_5 as *const c_void,
    ct_6 as *const c_void,
    ct_7 as *const c_void,
    ct_8 as *const c_void,
    ct_9 as *const c_void,
    ct_10 as *const c_void,
    ct_11 as *const c_void,
    my_get_generic as *const c_void,
    ct_13 as *const c_void,
    ct_14 as *const c_void,
    ct_15 as *const c_void,
    ct_16 as *const c_void,
    ct_17 as *const c_void,
    ct_18 as *const c_void,
    ct_19 as *const c_void,
    ct_20 as *const c_void,
    ct_21 as *const c_void,
    ct_22 as *const c_void,
    ct_23 as *const c_void,
    ct_24 as *const c_void,
    ct_25 as *const c_void,
    ct_26 as *const c_void,
    ct_27 as *const c_void,
    ct_28 as *const c_void,
    ct_29 as *const c_void,
    my_get_html_surface as *const c_void,
    ct_31 as *const c_void,
    ct_32 as *const c_void,
    ct_33 as *const c_void,
    ct_34 as *const c_void,
    ct_35 as *const c_void,
    ct_36 as *const c_void,
    ct_37 as *const c_void,
    ct_38 as *const c_void,
    ct_39 as *const c_void,
    ct_40 as *const c_void,
    ct_41 as *const c_void,
    ct_42 as *const c_void,
    ct_43 as *const c_void,
]);

#[repr(C)]
struct ClientObject {
    vptr: *const *const c_void,
}
unsafe impl Sync for ClientObject {}

static CLIENT_OBJ: ClientObject = ClientObject { vptr: CLIENT_VTABLE.0.as_ptr() };

// ------------------------------------------------ steamclient.so hooks

type CreateInterfaceFn = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;
type BGetCallbackFn = unsafe extern "C" fn(i32, *mut CallbackMsg) -> u8;
type FreeLastCallbackFn = unsafe extern "C" fn(i32);
type GetApiCallResultFn = unsafe extern "C" fn(i32, u64, *mut c_void, c_int, c_int, *mut u8) -> u8;
type IsApiCallCompletedFn = unsafe extern "C" fn(i32, u64, *mut u8) -> u8;

static REAL_CREATE_IFACE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static REAL_BGET_CB: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static REAL_FREE_CB: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static REAL_GET_API_RES: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static REAL_IS_API_DONE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn should_wrap_steamclient_version(name: &str) -> bool {
    matches!(
        name,
        "SteamClient016"
            | "SteamClient017"
            | "SteamClient018"
            | "SteamClient019"
            | "SteamClient020"
            | "SteamClient021"
            | "SteamClient022"
            | "SteamClient023"
    )
}

unsafe extern "C" fn my_create_interface(name: *const c_char, rc: *mut c_int) -> *mut c_void {
    let real = REAL_CREATE_IFACE.load(Ordering::Acquire);
    if real.is_null() {
        return std::ptr::null_mut();
    }
    let f: CreateInterfaceFn = unsafe { std::mem::transmute(real) };
    let res = unsafe { f(name, rc) };
    if res.is_null() || name.is_null() {
        return res;
    }
    if let Ok(s) = unsafe { CStr::from_ptr(name) }.to_str() {
        if should_wrap_steamclient_version(s) {
            REAL_CLIENT.store(res, Ordering::Release);
            CLIENT_WRAPPED.store(true, Ordering::Release);
            return &raw const CLIENT_OBJ as *mut c_void;
        }
    }
    res
}

unsafe extern "C" fn my_bget_callback(pipe: i32, msg: *mut CallbackMsg) -> u8 {
    if !msg.is_null() {
        if let Some(payload) = pop_completed_callback() {
            unsafe {
                (*msg).h_steam_user = 1;
                (*msg).i_callback = K_I_STEAM_API_CALL_COMPLETED;
                (*msg).pub_param = payload.cast::<u8>();
                (*msg).cub_param = size_of::<SteamApiCallCompleted>() as i32;
            }
            return 1;
        }
    }
    let real = REAL_BGET_CB.load(Ordering::Acquire);
    if real.is_null() {
        return 0;
    }
    let f: BGetCallbackFn = unsafe { std::mem::transmute(real) };
    unsafe { f(pipe, msg) }
}

unsafe extern "C" fn my_free_last_callback(pipe: i32) {
    if finish_delivering() {
        return;
    }
    let real = REAL_FREE_CB.load(Ordering::Acquire);
    if real.is_null() {
        return;
    }
    let f: FreeLastCallbackFn = unsafe { std::mem::transmute(real) };
    unsafe { f(pipe) }
}

unsafe extern "C" fn my_get_api_call_result(
    pipe: i32,
    call: u64,
    buf: *mut c_void,
    cub: c_int,
    expected: c_int,
    failed: *mut u8,
) -> u8 {
    if is_fake_call(call) {
        if expected == K_I_HTML_BROWSER_READY {
            if let Some(handle) = take_api_call_result(call) {
                if !buf.is_null() && cub >= 4 {
                    unsafe {
                        std::ptr::copy_nonoverlapping(handle.to_le_bytes().as_ptr(), buf.cast::<u8>(), 4);
                    }
                }
                if !failed.is_null() {
                    unsafe { *failed = 0 };
                }
                return 1;
            }
        }
        if !failed.is_null() {
            unsafe { *failed = 1 };
        }
        return 0;
    }
    let real = REAL_GET_API_RES.load(Ordering::Acquire);
    if real.is_null() {
        return 0;
    }
    let f: GetApiCallResultFn = unsafe { std::mem::transmute(real) };
    unsafe { f(pipe, call, buf, cub, expected, failed) }
}

unsafe extern "C" fn my_is_api_call_completed(pipe: i32, call: u64, failed: *mut u8) -> u8 {
    if is_fake_call(call) {
        let done = has_api_call_result(call);
        if !failed.is_null() {
            unsafe { *failed = 0 };
        }
        return u8::from(done);
    }
    let real = REAL_IS_API_DONE.load(Ordering::Acquire);
    if real.is_null() {
        return 0;
    }
    let f: IsApiCallCompletedFn = unsafe { std::mem::transmute(real) };
    unsafe { f(pipe, call, failed) }
}

// ------------------------------------------------ Linux dlsym interposer

#[cfg(target_os = "linux")]
mod interpose {
    use super::*;

    const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;

    #[repr(C)]
    struct DlInfo {
        dli_fname: *const c_char,
        dli_fbase: *mut c_void,
        dli_sname: *const c_char,
        dli_saddr: *mut c_void,
    }

    unsafe extern "C" {
        fn dlvsym(handle: *mut c_void, symbol: *const c_char, version: *const c_char) -> *mut c_void;
        fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int;
    }

    fn is_from_steamclient_so(addr: *const c_void) -> bool {
        let mut info = DlInfo {
            dli_fname: std::ptr::null(),
            dli_fbase: std::ptr::null_mut(),
            dli_sname: std::ptr::null(),
            dli_saddr: std::ptr::null_mut(),
        };
        if unsafe { dladdr(addr, &mut info) } == 0 || info.dli_fname.is_null() {
            return false;
        }
        let Ok(path) = unsafe { CStr::from_ptr(info.dli_fname) }.to_str() else {
            return false;
        };
        path.ends_with("/steamclient.so") || path == "steamclient.so"
    }

    type DlsymFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;
    static REAL_DLSYM: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    fn real_dlsym() -> Option<DlsymFn> {
        let mut p = REAL_DLSYM.load(Ordering::Acquire);
        if p.is_null() {
            // Try GLIBC_2.2.5 (x86_64) and GLIBC_2.17 (aarch64) before giving up.
            for ver in [c"GLIBC_2.2.5", c"GLIBC_2.17", c"GLIBC_2.34"] {
                p = unsafe { dlvsym(RTLD_NEXT, c"dlsym".as_ptr(), ver.as_ptr()) };
                if !p.is_null() {
                    REAL_DLSYM.store(p, Ordering::Release);
                    break;
                }
            }
        }
        if p.is_null() { None } else { Some(unsafe { std::mem::transmute(p) }) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void {
        let Some(next) = real_dlsym() else {
            return std::ptr::null_mut();
        };
        let res = unsafe { next(handle, symbol) };
        if res.is_null() || symbol.is_null() {
            return res;
        }
        let Ok(name) = unsafe { CStr::from_ptr(symbol) }.to_str() else {
            return res;
        };
        match name {
            "CreateInterface" if is_from_steamclient_so(res) => {
                REAL_CREATE_IFACE.store(res, Ordering::Release);
                my_create_interface as *mut c_void
            }
            "Steam_BGetCallback" if is_from_steamclient_so(res) => {
                REAL_BGET_CB.store(res, Ordering::Release);
                my_bget_callback as *mut c_void
            }
            "Steam_FreeLastCallback" if is_from_steamclient_so(res) => {
                REAL_FREE_CB.store(res, Ordering::Release);
                my_free_last_callback as *mut c_void
            }
            "Steam_GetAPICallResult" if is_from_steamclient_so(res) => {
                REAL_GET_API_RES.store(res, Ordering::Release);
                my_get_api_call_result as *mut c_void
            }
            "Steam_IsAPICallCompleted" if is_from_steamclient_so(res) => {
                REAL_IS_API_DONE.store(res, Ordering::Release);
                my_is_api_call_completed as *mut c_void
            }
            _ => res,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_browser_completes_with_html_browser_ready() {
        let call = html_create_browser(std::ptr::null_mut(), std::ptr::null(), std::ptr::null());
        assert!(is_fake_call(call));
        assert!(has_api_call_result(call));

        let mut msg = CallbackMsg {
            h_steam_user: 0,
            i_callback: 0,
            pub_param: std::ptr::null_mut(),
            cub_param: 0,
        };
        assert_eq!(unsafe { my_bget_callback(1, &mut msg) }, 1);
        assert_eq!(msg.i_callback, K_I_STEAM_API_CALL_COMPLETED);
        assert_eq!(msg.cub_param as usize, size_of::<SteamApiCallCompleted>());
        let completed = unsafe { *(msg.pub_param as *const SteamApiCallCompleted) };
        assert_eq!(completed.h_async_call, call);
        assert_eq!(completed.i_callback, K_I_HTML_BROWSER_READY);
        assert_eq!(completed.cub_param, 4);

        unsafe { my_free_last_callback(1) };

        let mut handle: u32 = 0;
        let mut failed: u8 = 1;
        let ok = unsafe {
            my_get_api_call_result(
                1,
                call,
                (&raw mut handle).cast::<c_void>(),
                4,
                K_I_HTML_BROWSER_READY,
                &mut failed,
            )
        };
        assert_eq!(ok, 1);
        assert_eq!(failed, 0);
        assert!(handle >= 1);
    }

    #[test]
    fn wraps_expected_steamclient_versions_only() {
        assert!(should_wrap_steamclient_version("SteamClient017"));
        assert!(should_wrap_steamclient_version("SteamClient020"));
        assert!(should_wrap_steamclient_version("SteamClient023"));
        assert!(!should_wrap_steamclient_version("CLIENTENGINE_INTERFACE_VERSION005"));
    }
}
