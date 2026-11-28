//! Windows: Valve's `steamclient64.dll` from the user's Steam folder, loaded
//! with `LoadLibraryExW`, and its MSVC RTTI class names.
//!
//! The engine's interface layouts match the Linux build's (Valve generates
//! both from the same definitions); only the loader and the RTTI format
//! differ. Each object is still checked by class name before it's used.

use std::ffi::{CString, c_char, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "x86_64")]
#[path = "windows_profile.rs"]
mod profile;
#[cfg(target_arch = "x86_64")]
pub use profile::install_profile_forwarding;

/// Class names [`class_name`] gives for the engine and its `IClientUser`.
pub const ENGINE_CLASS: &str = ".?AVCSteamClient@@";
pub const USER_CLASS: &str = ".?AVIClientUserMap@@";

const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
const MEM_COMMIT: u32 = 0x1000;
const PAGE_GUARD: u32 = 0x100;
const READABLE: u32 = 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[repr(C)]
struct MemoryInfo {
    base: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn GetLastError() -> u32;
    fn SetDllDirectoryW(dir: *const u16) -> i32;
    fn GetModuleHandleExW(flags: u32, addr: *const c_void, module: *mut *mut c_void) -> i32;
    fn VirtualQuery(addr: *const c_void, info: *mut MemoryInfo, len: usize) -> usize;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
    fn Process32FirstW(snap: *mut c_void, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snap: *mut c_void, entry: *mut ProcessEntry) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, code: u32) -> i32;
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// The loaded steamclient64.dll's base address, for [`in_steamclient`].
static BASE: AtomicUsize = AtomicUsize::new(0);

pub struct Lib(*mut c_void);

impl Lib {
    /// Loads `<root>\steamclient64.dll` as the global instance's engine.
    pub fn open(root: &Path) -> Result<Lib, String> {
        if let Some(pid) = steam_running() {
            return Err(format!("Steam is running (pid {pid}); only one global instance can exist"));
        }
        for_client(root)
    }

    fn load(root: &Path) -> Result<Lib, String> {
        let dll = root.join("steamclient64.dll");
        // Its own dependencies (tier0_s64.dll, vstdlib_s64.dll, …) sit beside it.
        // SAFETY: plain Win32 calls with NUL-terminated wide strings we built.
        let h = unsafe {
            SetDllDirectoryW(wide(root.as_os_str()).as_ptr());
            LoadLibraryExW(wide(dll.as_os_str()).as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH)
        };
        if h.is_null() {
            return Err(format!("LoadLibrary {}: error {}", dll.display(), unsafe { GetLastError() }));
        }
        BASE.store(h as usize, Ordering::Relaxed);
        Ok(Lib(h))
    }

    pub fn sym<T: Copy>(&self, name: &str) -> Result<T, String> {
        let c = CString::new(name).unwrap();
        // SAFETY: module from LoadLibraryExW; caller picks the matching fn type.
        let p = unsafe { GetProcAddress(self.0, c.as_ptr()) };
        if p.is_null() {
            return Err(format!("{name} missing (error {})", unsafe { GetLastError() }));
        }
        assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
        // SAFETY: T is a function pointer type of the same size.
        Ok(unsafe { std::mem::transmute_copy(&p) })
    }
}

/// Breakpad's dump folder: `|` can't be in a Windows path, so no folder can
/// be made there and no crash or assert report is written or uploaded.
pub const NO_DUMPS: &str = "C:\\steamhost|no-dumps";

/// The environment Steam gives steamclient, then the library itself.
/// Must run before any other thread starts (it sets env vars).
pub fn for_client(root: &Path) -> Result<Lib, String> {
    let root = steam_root(root)?;
    // SAFETY: single-threaded at this point (called first thing from main).
    unsafe {
        std::env::set_var("SteamPath", &root);
        std::env::set_var("SteamAppId", "");
        std::env::set_var("BREAKPAD_DUMP_LOCATION", NO_DUMPS);
    }
    // steamclient writes some files to the working directory.
    std::env::set_current_dir(&root).map_err(|e| format!("chdir {}: {e}", root.display()))?;
    Lib::load(&root)
}

/// The Steam folder without the `\\?\` prefix `canonicalize` adds, which
/// Valve's path handling doesn't expect.
fn steam_root(root: &Path) -> Result<PathBuf, String> {
    let c = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    let s = c.to_string_lossy();
    Ok(PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s)))
}

const MAX_CLASS_NAME: usize = 256;
const EXECUTABLE_READABLE: u32 = 0x20 | 0x40 | 0x80;

/// Committed, readable memory (so reading `len` bytes at `p` can't fault).
fn readable(p: *const c_void, len: usize) -> bool {
    if p.is_null() || len == 0 {
        return false;
    }
    let Some(req_end) = (p as usize).checked_add(len) else {
        return false;
    };
    let mut cur = p as usize;
    while cur < req_end {
        let mut info: MemoryInfo = unsafe { std::mem::zeroed() };
        // SAFETY: VirtualQuery only inspects the address space.
        if unsafe { VirtualQuery(cur as *const c_void, &mut info, size_of::<MemoryInfo>()) } == 0 {
            return false;
        }
        let Some(end) = (info.base as usize).checked_add(info.region_size) else {
            return false;
        };
        if info.state != MEM_COMMIT || info.protect & READABLE == 0 || info.protect & PAGE_GUARD != 0 || end <= cur {
            return false;
        }
        cur = end;
    }
    true
}

/// True if `p` lies inside the loaded steamclient64.dll and `len` bytes are readable.
fn in_steamclient_range(p: *const c_void, len: usize) -> bool {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 || p.is_null() || len == 0 {
        return false;
    }
    let Some(last) = (p as usize).checked_add(len - 1) else {
        return false;
    };
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    let mut m_start = std::ptr::null_mut();
    let mut m_end = std::ptr::null_mut();
    // SAFETY: lookup only; the refcount is left unchanged.
    unsafe {
        GetModuleHandleExW(flags, p, &mut m_start) != 0
            && GetModuleHandleExW(flags, last as *const c_void, &mut m_end) != 0
            && m_start as usize == base
            && m_end as usize == base
            && readable(p, len)
    }
}

/// True if `p` lies inside the loaded steamclient64.dll.
fn in_steamclient(p: *const c_void) -> bool {
    in_steamclient_range(p, 8)
}

fn executable_in_steamclient(p: *const c_void) -> bool {
    if !in_steamclient_range(p, 1) {
        return false;
    }
    let mut info: MemoryInfo = unsafe { std::mem::zeroed() };
    if unsafe { VirtualQuery(p, &mut info, size_of::<MemoryInfo>()) } == 0 {
        return false;
    }
    info.state == MEM_COMMIT && info.protect & EXECUTABLE_READABLE != 0 && info.protect & PAGE_GUARD == 0
}

/// The MSVC RTTI class name (`.?AV<name>@@`) of a polymorphic object from
/// steamclient64.dll: vtable[-1] is its CompleteObjectLocator, whose type
/// descriptor (an image-relative offset on x64) holds the decorated name.
pub fn class_name(obj: *mut c_void) -> Option<String> {
    if !readable(obj, 8) || (obj as usize) % 8 != 0 {
        return None;
    }
    let base = BASE.load(Ordering::Relaxed);
    // SAFETY: every pointer is checked to be readable (and, past the object
    // itself, inside steamclient64.dll) before it is read.
    unsafe {
        let vt = *(obj as *const *const *const c_void);
        if (vt as usize) % 8 != 0 || !in_steamclient(vt as *const c_void) || !in_steamclient(vt.offset(-1) as *const c_void) {
            return None;
        }
        let col = *vt.offset(-1) as *const u32;
        // signature 1 = the x64 layout with image-relative offsets.
        if (col as usize) % 4 != 0 || !in_steamclient_range(col as *const c_void, 24) || *col != 1 {
            return None;
        }
        let name_addr = base.checked_add(*col.add(3) as usize)?.checked_add(16)?;
        let mut bytes = Vec::new();
        for i in 0..MAX_CLASS_NAME {
            let b_addr = name_addr.checked_add(i)?;
            if !in_steamclient_range(b_addr as *const c_void, 1) {
                return None;
            }
            let b = *(b_addr as *const u8);
            if b == 0 {
                return (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned());
            }
            bytes.push(b);
        }
        None
    }
}

/// Verify that `obj`'s vtable slots `0..=max_slot` are non-null readable code
/// pointers inside the loaded `steamclient64.dll` executable segment.
pub fn verify_vtable(obj: *mut c_void, max_slot: usize) -> Result<(), String> {
    let ptr_size = size_of::<usize>();
    if !readable(obj, ptr_size) || (obj as usize) % align_of::<usize>() != 0 {
        return Err("unsupported Steam build: interface object pointer is invalid or unreadable".into());
    }
    let vt = unsafe { *(obj as *const *const *const c_void) };
    let table_bytes = max_slot
        .checked_add(1)
        .and_then(|n| n.checked_mul(ptr_size))
        .ok_or_else(|| "unsupported Steam build: vtable size overflow".to_string())?;
    if vt.is_null() || (vt as usize) % align_of::<usize>() != 0 || !in_steamclient_range(vt as *const c_void, table_bytes) {
        return Err("unsupported Steam build: vtable is outside readable steamclient memory".into());
    }
    for slot in 0..=max_slot {
        let fp = unsafe { *vt.add(slot) };
        if fp.is_null() || !executable_in_steamclient(fp) {
            return Err(format!(
                "unsupported Steam build: vtable slot {slot} is not an executable steamclient code pointer"
            ));
        }
    }
    Ok(())
}

/// Verify a single vtable slot on `obj` before invoking it.
pub fn verify_slot(obj: *mut c_void, slot: usize) -> Result<(), String> {
    let ptr_size = size_of::<usize>();
    if !readable(obj, ptr_size) || (obj as usize) % align_of::<usize>() != 0 {
        return Err("unsupported Steam build: interface object pointer is invalid or unreadable".into());
    }
    let vt = unsafe { *(obj as *const *const *const c_void) };
    let slot_addr = slot
        .checked_mul(ptr_size)
        .and_then(|off| (vt as usize).checked_add(off))
        .ok_or_else(|| "unsupported Steam build: vtable slot overflow".to_string())?;
    if vt.is_null() || (vt as usize) % align_of::<usize>() != 0 || !in_steamclient_range(slot_addr as *const c_void, ptr_size) {
        return Err(format!("unsupported Steam build: vtable slot {slot} is outside readable steamclient memory"));
    }
    let fp = unsafe { *(slot_addr as *const *const c_void) };
    if fp.is_null() || !executable_in_steamclient(fp) {
        return Err(format!(
            "unsupported Steam build: vtable slot {slot} is not an executable steamclient code pointer"
        ));
    }
    Ok(())
}

/// The official client (or its web helper) is running.
fn steam_running() -> Option<u32> {
    // SAFETY: Toolhelp snapshot walk with a correctly sized entry.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return None;
        }
        let mut e: ProcessEntry = std::mem::zeroed();
        e.size = size_of::<ProcessEntry>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        let mut found = None;
        while ok {
            let len = e.exe.iter().position(|&c| c == 0).unwrap_or(e.exe.len());
            let name = String::from_utf16_lossy(&e.exe[..len]).to_lowercase();
            if name == "steam.exe" || name == "steamwebhelper.exe" {
                found = Some(e.pid);
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// Valve's own switches, read from the command line: no assert minidumps
/// and no crash dialog or monitor.
pub const QUIET_FLAGS: [&str; 3] = ["-noassert", "-nocrashdialog", "-nocrashmonitor"];

/// Make sure the quiet flags really are on our command line. Windows has no
/// exec: without them, run ourselves again with them (same pipes, no
/// window) and leave with its exit code.
pub fn ensure_quiet_flags(args: &[String]) {
    if args.iter().any(|a| a == QUIET_FLAGS[0]) {
        return;
    }
    use std::os::windows::process::CommandExt;
    let code = std::env::current_exe()
        .and_then(|exe| std::process::Command::new(exe).args(&args[1..]).args(QUIET_FLAGS).creation_flags(CREATE_NO_WINDOW).status())
        .map(|s| s.code().unwrap_or(1));
    match code {
        Ok(c) => quit(c),
        Err(e) => {
            eprintln!("steamhost: restart failed: {e}");
            quit(1)
        }
    }
}

/// Leave without DLL detach or static destructors: Valve's engine threads
/// are still running under them.
pub fn quit(status: i32) -> ! {
    // SAFETY: ends this process right away.
    unsafe {
        TerminateProcess(GetCurrentProcess(), status as u32);
    }
    unreachable!("TerminateProcess returned")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
        fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> i32;
    }

    #[test]
    fn class_name_and_vtable_reject_unreadable_and_bogus_pointers() {
        // MEM_COMMIT | MEM_RESERVE (0x3000), PAGE_NOACCESS (0x01)
        let p = unsafe { VirtualAlloc(std::ptr::null_mut(), 4096, 0x3000, 0x01) };
        assert!(!p.is_null());
        assert_eq!(class_name(p), None);
        assert!(verify_vtable(p, 56).is_err());
        assert_eq!(class_name(0x1000usize as *mut c_void), None);
        unsafe {
            VirtualFree(p, 0, 0x8000);
        }
    }
}
