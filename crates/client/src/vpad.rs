//! A host-side virtual Xbox 360 pad. Both sticks use the XInput convention (+Y up).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VirtualPadState {
    pub buttons: u16,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub lt: u8,
    pub rt: u8,
}

#[cfg(target_os = "linux")]
#[path = "vpad_linux.rs"]
mod linux;
#[cfg(any(windows, test))]
#[path = "vpad_windows.rs"]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{VirtualXboxPad, is_available};
#[cfg(windows)]
pub use windows::{VirtualXboxPad, is_available};

pub fn backend_name() -> &'static str {
    if cfg!(windows) {
        "ViGEmBus"
    } else if cfg!(target_os = "linux") {
        "uinput"
    } else {
        "unsupported"
    }
}

pub fn unavailable_hint() -> &'static str {
    if cfg!(windows) {
        "Install the ViGEmBus driver."
    } else if cfg!(target_os = "linux") {
        "Load the uinput module and grant your user read/write access to /dev/uinput."
    } else {
        "Use Linux or Windows for virtual controller output."
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod unsupported {
    use super::*;
    pub fn is_available() -> bool {
        false
    }
    pub struct VirtualXboxPad(());
    impl VirtualXboxPad {
        pub fn create(_: &str) -> Result<Self, String> {
            Err(unavailable_hint().into())
        }
        pub fn update(&mut self, _: VirtualPadState) -> Result<(), String> {
            Err(unavailable_hint().into())
        }
        pub fn poll_rumble(&mut self) -> Option<(u16, u16)> {
            None
        }
    }
}
#[cfg(not(any(windows, target_os = "linux")))]
pub use unsupported::{VirtualXboxPad, is_available};
