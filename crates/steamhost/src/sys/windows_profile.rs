//! Forward profile requests at the host end of Valve's Friends IPC.

use super::{BASE, GetCurrentProcess, GetLastError, MemoryInfo, VirtualQuery, in_steamclient_range};
use sha2::{Digest, Sha256};
use std::ffi::{c_char, c_void};
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{SyncSender, sync_channel};

// Valve steamclient64.dll 11.23.65.88. The server dispatcher has already
// decoded the dialog and Steam ID at this site. It calls IClientFriends
// slot 185 with (this, dialog, SteamID), then writes the usual IPC reply.
const STEAMCLIENT_SHA256: [u8; 32] = [
    0x66, 0x9f, 0xa0, 0x8e, 0x25, 0x92, 0x77, 0xfb, 0xf5, 0xa1, 0xdf, 0xe1, 0x5c, 0xe8, 0x8a, 0xfe,
    0x73, 0x4e, 0x89, 0x15, 0x11, 0x94, 0x30, 0x4b, 0x44, 0xd9, 0x2f, 0x41, 0x8d, 0xcc, 0x98, 0x5b,
];
const CALL_RVA: usize = 0x7ec507;
const FRIENDS_PROFILE_SLOT: usize = 0x5c8 / size_of::<usize>();
const ORIGINAL_CALL: [u8; 6] = [0xff, 0x90, 0xc8, 0x05, 0, 0];

const MEM_RESERVE: u32 = 0x2000;
const MEM_COMMIT: u32 = 0x1000;
const MEM_FREE: u32 = 0x10000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const ALLOCATION_GRANULARITY: usize = 0x10000;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protection: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> i32;
    fn VirtualProtect(addr: *mut c_void, size: usize, protection: u32, old: *mut u32) -> i32;
    fn FlushInstructionCache(process: *mut c_void, addr: *const c_void, size: usize) -> i32;
}

type ActivateProfile = unsafe extern "system" fn(*mut c_void, *const c_char, u64);
static PROFILES: OnceLock<SyncSender<u64>> = OnceLock::new();

unsafe extern "system" fn forward_profile(this: *mut c_void, dialog: *const c_char, target: u64) {
    if !dialog.is_null()
        && target >> 52 == 0x11
        && target as u32 != 0
        && unsafe { std::ffi::CStr::from_ptr(dialog) } == c"steamid"
        && PROFILES.get().is_some_and(|sender| sender.try_send(target).is_ok())
    {
        // Aurora owns this request. Activating a second overlay can steal
        // the focus Aurora just took from the game.
        return;
    }
    // Preserve Valve's dispatch for unsupported or undeliverable requests.
    let original = unsafe { *(*(this as *const *const ActivateProfile)).add(FRIENDS_PROFILE_SLOT) };
    unsafe { original(this, dialog, target) };
}

fn relative_call(call: usize, pointer: usize) -> Option<[u8; 6]> {
    let displacement = i32::try_from(pointer as i128 - (call as i128 + 6)).ok()?;
    let mut bytes = [0xff, 0x15, 0, 0, 0, 0];
    bytes[2..].copy_from_slice(&displacement.to_le_bytes());
    Some(bytes)
}

/// The replacement is a six-byte indirect call through an ordinary data
/// pointer. Place the pointer within RIP-relative addressing distance.
fn nearby_pointer(call: usize) -> Result<*mut *const c_void, String> {
    let reach = i32::MAX as usize;
    let mut addr = call.saturating_sub(reach).max(ALLOCATION_GRANULARITY);
    let end = call.saturating_add(reach);
    while addr < end {
        let mut info: MemoryInfo = unsafe { std::mem::zeroed() };
        if unsafe { VirtualQuery(addr as *const c_void, &mut info, size_of::<MemoryInfo>()) } == 0 {
            break;
        }
        let next = (info.base as usize).saturating_add(info.region_size);
        let aligned = addr.next_multiple_of(ALLOCATION_GRANULARITY);
        if info.state == MEM_FREE && aligned < end && next.saturating_sub(aligned) >= size_of::<usize>() {
            let pointer = unsafe {
                VirtualAlloc(aligned as *mut c_void, size_of::<usize>(), MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE)
            };
            if !pointer.is_null() {
                return Ok(pointer.cast());
            }
            addr = aligned.saturating_add(ALLOCATION_GRANULARITY);
        } else if next > addr {
            addr = next;
        } else {
            break;
        }
    }
    Err("could not allocate a nearby profile forwarding pointer".into())
}

/// SAFETY: `call` points to the verified instruction. No thread may execute
/// it until the patch and instruction-cache flush are complete.
unsafe fn redirect_profile_call(call: *mut u8) -> Result<*mut *const c_void, String> {
    let pointer = nearby_pointer(call as usize)?;
    let Some(bytes) = relative_call(call as usize, pointer as usize) else {
        unsafe { VirtualFree(pointer.cast(), 0, MEM_RELEASE) };
        return Err("profile forwarding pointer is outside call range".into());
    };
    unsafe { pointer.write(forward_profile as *const c_void) };
    let mut protection = 0;
    if unsafe { VirtualProtect(call.cast(), bytes.len(), PAGE_EXECUTE_READWRITE, &mut protection) } == 0 {
        let error = unsafe { GetLastError() };
        unsafe { VirtualFree(pointer.cast(), 0, MEM_RELEASE) };
        return Err(format!("could not update the profile dispatcher: Win32 error {error}"));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), call, bytes.len());
        if FlushInstructionCache(GetCurrentProcess(), call.cast(), bytes.len()) == 0 {
            std::ptr::copy_nonoverlapping(ORIGINAL_CALL.as_ptr(), call, ORIGINAL_CALL.len());
            FlushInstructionCache(GetCurrentProcess(), call.cast(), bytes.len());
            VirtualProtect(call.cast(), bytes.len(), protection, &mut 0);
            VirtualFree(pointer.cast(), 0, MEM_RELEASE);
            return Err("could not flush the updated Steam profile dispatcher".into());
        }
        if VirtualProtect(call.cast(), bytes.len(), protection, &mut 0) == 0 {
            std::ptr::copy_nonoverlapping(ORIGINAL_CALL.as_ptr(), call, ORIGINAL_CALL.len());
            FlushInstructionCache(GetCurrentProcess(), call.cast(), bytes.len());
            VirtualProtect(call.cast(), bytes.len(), protection, &mut 0);
            VirtualFree(pointer.cast(), 0, MEM_RELEASE);
            return Err("could not restore the Steam profile dispatcher's page protection".into());
        }
    }
    Ok(pointer)
}

/// Install before opening Valve's global pipe so no live request can execute
/// this call site during the write. On-disk Steam files are never changed.
pub fn install_profile_forwarding(root: &Path) -> Result<(), String> {
    let mut dll = std::fs::File::open(root.join("steamclient64.dll")).map_err(|e| e.to_string())?;
    let mut sha = Sha256::new();
    std::io::copy(&mut dll, &mut sha).map_err(|e| e.to_string())?;
    if <[u8; 32]>::from(sha.finalize()) != STEAMCLIENT_SHA256 {
        return Err("installed steamclient64.dll is outside the verified profile forwarding build".into());
    }
    let base = BASE.load(Ordering::Relaxed);
    let call = base.checked_add(CALL_RVA).ok_or("invalid Steam client base address")? as *mut u8;
    if base == 0 || !in_steamclient_range(call.cast(), ORIGINAL_CALL.len())
        || unsafe { std::slice::from_raw_parts(call, ORIGINAL_CALL.len()) } != ORIGINAL_CALL
    {
        return Err("Steam profile dispatcher does not match the verified call site".into());
    }

    let (sender, receiver) = sync_channel(16);
    std::thread::Builder::new().name("steamhost-profiles".into()).spawn(move || {
        for target in receiver {
            let mut stdout = std::io::stdout().lock();
            if writeln!(stdout, "STEAMHOST-PROFILE {target}").and_then(|_| stdout.flush()).is_err() {
                break;
            }
        }
    }).map_err(|e| e.to_string())?;
    // Steam's global pipe is opened only after this function returns.
    unsafe { redirect_profile_call(call)? };
    let _ = PROFILES.set(sender);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ipc_hook_routes_each_request_once() {
        use std::sync::atomic::AtomicUsize;
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "system" fn original(_: *mut c_void, _: *const c_char, _: u64) {
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        let (sender, receiver) = sync_channel(4);
        PROFILES.set(sender).unwrap();
        let table = [original as ActivateProfile; FRIENDS_PROFILE_SLOT + 1];
        let mut object = table.as_ptr();
        let this = (&mut object as *mut *const ActivateProfile).cast();
        let lobby_target = 0x0110_0001_0000_002a;
        let post_match_target = 0x0110_0001_0000_002b;
        // Recreate the verified x64 call with a local vtable. The surrounding
        // instructions reserve the Win64 shadow space and load its address.
        let code: [u8; 18] = [
            0x48, 0x83, 0xec, 0x28, 0x48, 0x8b, 0x01,
            0xff, 0x90, 0xc8, 0x05, 0, 0,
            0x48, 0x83, 0xc4, 0x28, 0xc3,
        ];
        unsafe {
            let memory = VirtualAlloc(std::ptr::null_mut(), code.len(), MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);
            assert!(!memory.is_null());
            std::ptr::copy_nonoverlapping(code.as_ptr(), memory.cast(), code.len());
            let mut old = 0;
            assert_ne!(VirtualProtect(memory, code.len(), 0x20, &mut old), 0); // PAGE_EXECUTE_READ
            let pointer = redirect_profile_call(memory.cast::<u8>().add(7)).unwrap();
            let invoke: ActivateProfile = std::mem::transmute(memory);
            invoke(this, c"steamid".as_ptr(), lobby_target);
            invoke(this, c"steamid".as_ptr(), post_match_target);
            invoke(this, c"chat".as_ptr(), lobby_target);
            invoke(this, c"steamid".as_ptr(), 42);
            assert_eq!(receiver.try_recv().unwrap(), lobby_target);
            assert_eq!(receiver.try_recv().unwrap(), post_match_target);
            assert!(receiver.try_recv().is_err());
            assert_eq!(CALLS.load(Ordering::Relaxed), 2);
            drop(receiver);
            invoke(this, c"steamid".as_ptr(), lobby_target);
            VirtualFree(memory, 0, MEM_RELEASE);
            VirtualFree(pointer.cast(), 0, MEM_RELEASE);
        }
        assert_eq!(CALLS.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn indirect_call_encodes_rip_relative_pointer() {
        assert_eq!(relative_call(0x10000, 0x10020), Some([0xff, 0x15, 0x1a, 0, 0, 0]));
        assert_eq!(relative_call(0x10000, 0x0fff0), Some([0xff, 0x15, 0xea, 0xff, 0xff, 0xff]));
        assert_eq!(relative_call(0x10000, 0x1_0000_0000), None);
    }
}
