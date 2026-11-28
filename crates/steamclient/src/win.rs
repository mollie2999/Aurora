//! The few Win32 calls the DLL needs, declared by hand (no extra crates).

#![allow(clippy::upper_case_acronyms)]

use std::ffi::c_void;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, len: u32) -> u32;
    fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut *mut c_void, hash: *mut u32) -> u16;
    fn GetProcessHeap() -> *mut c_void;
    fn HeapFree(heap: *mut c_void, flags: u32, mem: *mut c_void) -> i32;
}

#[cfg(windows)]
#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueW(key: isize, sub: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RRF_RT_REG_SZ: u32 = 0x2;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A string value under HKEY_CURRENT_USER.
#[cfg(windows)]
pub fn reg_string(sub: &str, value: &str) -> Option<String> {
    let (sub, value) = (wide(sub), wide(value));
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
    if r != 0 {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n.min(buf.len())]))
}

#[cfg(not(windows))]
pub fn reg_string(_sub: &str, _value: &str) -> Option<String> {
    None
}

/// Load Valve's own steamclient from the Steam folder (for processes we
/// didn't start, e.g. games the official Steam client runs).
#[cfg(windows)]
pub fn load_real_steamclient() -> usize {
    let Some(root) = reg_string("Software\\Valve\\Steam", "SteamPath") else { return 0 };
    let file = if cfg!(target_pointer_width = "64") { "steamclient64.dll" } else { "steamclient.dll" };
    let path = format!("{}\\{}", root.replace('/', "\\"), file);
    let w = wide(&path);
    let module = unsafe { LoadLibraryW(w.as_ptr()) };
    // In a Proton prefix Aurora may have put this DLL in the Steam
    // folder itself: forwarding to ourselves would loop forever.
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    let mut ours = std::ptr::null_mut();
    let found = unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, load_real_steamclient as *const u16, &mut ours) };
    if found != 0 && module == ours {
        return 0;
    }
    module as usize
}

#[cfg(not(windows))]
pub fn load_real_steamclient() -> usize {
    0
}

/// Keep this DLL loaded for good. Old steam_api versions FreeLibrary it when
/// their init fails (or at shutdown) while our threads still run in it, which
/// crashed the game instead of letting it show its own message.
#[cfg(windows)]
pub fn pin_self() {
    const FROM_ADDRESS: u32 = 0x4;
    const PIN: u32 = 0x1;
    let mut module = std::ptr::null_mut();
    unsafe { GetModuleHandleExW(FROM_ADDRESS | PIN, pin_self as *const u16, &mut module) };
}

#[cfg(not(windows))]
pub fn pin_self() {}

/// File name (no folder, lower case) of the loaded module holding `addr`;
/// `None` if no module does (heap, or code mapped by hand).
#[cfg(windows)]
pub fn module_of(addr: usize) -> Option<String> {
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    let mut module = std::ptr::null_mut();
    if addr == 0 || unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, addr as *const u16, &mut module) } == 0 {
        return None;
    }
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    Some(path.rsplit(['\\', '/']).next().unwrap_or_default().to_lowercase())
}

#[cfg(not(windows))]
pub fn module_of(_addr: usize) -> Option<String> {
    None
}

/// Who called into this DLL: the module (file name, lower case) of the first
/// return address on the stack outside it, `Some(None)` if that code isn't
/// in any module, `None` if the stack walk didn't get past us.
#[cfg(windows)]
pub fn caller_module() -> Option<Option<String>> {
    let mut frames = [std::ptr::null_mut::<c_void>(); 24];
    let n = unsafe { RtlCaptureStackBackTrace(0, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    let ours = module_of(caller_module as usize);
    frames[..n.min(frames.len())].iter().map(|&f| module_of(f as usize)).find(|m| *m != ours)
}

#[cfg(not(windows))]
pub fn caller_module() -> Option<Option<String>> {
    None
}

#[cfg(windows)]
pub fn proc_address(module: usize, name: &str) -> usize {
    if module == 0 {
        return 0;
    }
    let mut n = name.as_bytes().to_vec();
    n.push(0);
    unsafe { GetProcAddress(module as *mut c_void, n.as_ptr()) as usize }
}

#[cfg(not(windows))]
pub fn proc_address(_module: usize, _name: &str) -> usize {
    0
}

/// Under Wine (Proton): the Windows path of a Linux one, as Wine maps it.
/// It picks the drive letter Proton gave the game's library (E:\…) and
/// mangles names Windows can't use (a ':' in a mount point, for example),
/// so the game can open it. `None` outside Wine.
#[cfg(windows)]
pub fn wine_dos_path(unix: &str) -> Option<String> {
    // Wine declares it CDECL. "system" is stdcall on 32-bit: the stack came
    // back 4 bytes off and 32-bit games (Alien: Isolation) crashed here.
    type ToDos = unsafe extern "C" fn(*const u8) -> *mut u16;
    let kernel32 = unsafe { GetModuleHandleW(wide("kernel32.dll").as_ptr()) };
    let f = proc_address(kernel32 as usize, "wine_get_dos_file_name");
    if f == 0 {
        return None;
    }
    let unix = std::ffi::CString::new(unix).ok()?;
    let p = unsafe { std::mem::transmute::<usize, ToDos>(f)(unix.as_ptr().cast()) };
    if p.is_null() {
        return None;
    }
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) });
    unsafe { HeapFree(GetProcessHeap(), 0, p.cast()) };
    // \\?\ prefixed paths come back for names without a drive; plain is nicer.
    Some(s.strip_prefix("\\\\?\\").unwrap_or(&s).to_string())
}

#[cfg(not(windows))]
pub fn wine_dos_path(_unix: &str) -> Option<String> {
    None
}

