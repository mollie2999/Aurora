//! ViGEmClient include/ViGEm/km/BusShared.h, protocol version 1.
use super::VirtualPadState;

const fn ctl(function: u32, access: u32) -> u32 {
    (0x2a << 16) | (access << 14) | (function << 2)
}
const PLUGIN: u32 = ctl(0x801, 2);
const UNPLUG: u32 = ctl(0x802, 2);
const VERSION: u32 = ctl(0x803, 2);
#[cfg(windows)]
const WAIT_READY: u32 = ctl(0x804, 2);
const NOTIFY: u32 = ctl(0xa01, 3);
const SUBMIT: u32 = ctl(0xa02, 2);

#[repr(C)]
struct Plugin {
    size: u32,
    serial: u32,
    kind: u32,
    vendor: u16,
    product: u16,
}
#[repr(C)]
struct Request {
    size: u32,
    value: u32,
}
impl Request {
    fn new(value: u32) -> Self {
        Self {
            size: size_of::<Self>() as u32,
            value,
        }
    }
}
#[repr(C)]
struct Report {
    buttons: u16,
    lt: u8,
    rt: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}
#[repr(C)]
struct Submit {
    size: u32,
    serial: u32,
    report: Report,
}
impl Submit {
    fn new(serial: u32, s: VirtualPadState) -> Self {
        Self {
            size: size_of::<Self>() as u32,
            serial,
            report: Report {
                buttons: s.buttons,
                lt: s.lt,
                rt: s.rt,
                lx: s.lx,
                ly: s.ly,
                rx: s.rx,
                ry: s.ry,
            },
        }
    }
}
#[repr(C)]
struct Notification {
    size: u32,
    serial: u32,
    large: u8,
    small: u8,
    led: u8,
    padding: u8,
}

#[cfg(windows)]
pub use bus::{VirtualXboxPad, is_available};
#[cfg(windows)]
mod bus {
    use super::*;
    use std::{
        cell::UnsafeCell,
        ffi::c_void,
        io,
        ptr::{null, null_mut},
    };
    type Handle = isize;
    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const BUS: Guid = Guid(
        0x96e42b22,
        0xf5e9,
        0x42f8,
        [0xb0, 0x43, 0xed, 0x0f, 0x93, 0x2f, 0x01, 0x4f],
    );
    #[repr(C)]
    struct Interface {
        size: u32,
        guid: Guid,
        flags: u32,
        reserved: usize,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Overlapped {
        internal: usize,
        high: usize,
        offset: [u32; 2],
        event: Handle,
    }

    #[link(name = "setupapi")]
    unsafe extern "system" {
        fn SetupDiGetClassDevsW(
            guid: *const Guid,
            enumerator: *const u16,
            parent: Handle,
            flags: u32,
        ) -> Handle;
        fn SetupDiEnumDeviceInterfaces(
            list: Handle,
            dev: *const c_void,
            guid: *const Guid,
            index: u32,
            interface: *mut Interface,
        ) -> i32;
        fn SetupDiGetDeviceInterfaceDetailW(
            list: Handle,
            interface: *const Interface,
            detail: *mut c_void,
            len: u32,
            required: *mut u32,
            dev: *mut c_void,
        ) -> i32;
        fn SetupDiDestroyDeviceInfoList(list: Handle) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            path: *const u16,
            access: u32,
            share: u32,
            attrs: *const c_void,
            creation: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn CreateEventW(
            attrs: *const c_void,
            manual: i32,
            initial: i32,
            name: *const u16,
        ) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn DeviceIoControl(
            handle: Handle,
            code: u32,
            input: *const c_void,
            in_len: u32,
            output: *mut c_void,
            out_len: u32,
            bytes: *mut u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn GetOverlappedResult(
            handle: Handle,
            overlapped: *mut Overlapped,
            bytes: *mut u32,
            wait: i32,
        ) -> i32;
        fn CancelIoEx(handle: Handle, overlapped: *mut Overlapped) -> i32;
    }

    struct Owned(Handle);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    struct List(Handle);
    impl Drop for List {
        fn drop(&mut self) {
            unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }

    fn event() -> io::Result<Owned> {
        let h = unsafe { CreateEventW(null(), 1, 0, null()) };
        if h == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Owned(h))
        }
    }
    fn open() -> io::Result<Owned> {
        let list = unsafe { SetupDiGetClassDevsW(&BUS, null(), 0, 0x12) };
        if list == -1 {
            return Err(io::Error::last_os_error());
        }
        let list = List(list);
        let mut last = None;
        for index in 0.. {
            let mut iface: Interface = unsafe { std::mem::zeroed() };
            iface.size = size_of::<Interface>() as u32;
            if unsafe { SetupDiEnumDeviceInterfaces(list.0, null(), &BUS, index, &mut iface) } == 0
            {
                return Err(last.unwrap_or_else(io::Error::last_os_error));
            }
            let mut bytes = 0;
            unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    list.0,
                    &iface,
                    null_mut(),
                    0,
                    &mut bytes,
                    null_mut(),
                )
            };
            if bytes < 6 {
                continue;
            }
            // cbSize includes ABI padding on Win64; DevicePath starts at byte 4 on both ABIs.
            let mut detail = vec![0u32; (bytes as usize).div_ceil(4)];
            detail[0] = if cfg!(target_pointer_width = "64") {
                8
            } else {
                6
            };
            if unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    list.0,
                    &iface,
                    detail.as_mut_ptr().cast(),
                    bytes,
                    null_mut(),
                    null_mut(),
                )
            } == 0
            {
                last = Some(io::Error::last_os_error());
                continue;
            }
            let path = detail[1..].as_ptr().cast::<u16>();
            let h = unsafe { CreateFileW(path, 0xc0000000, 3, null(), 3, 0x40000000, 0) };
            if h != -1 {
                return Ok(Owned(h));
            }
            last = Some(io::Error::last_os_error());
        }
        unreachable!()
    }

    fn control<T>(h: Handle, ev: Handle, code: u32, input: &T) -> io::Result<()> {
        let mut ol = Overlapped {
            event: ev,
            ..Default::default()
        };
        let mut bytes = 0;
        if unsafe {
            DeviceIoControl(
                h,
                code,
                (input as *const T).cast(),
                size_of::<T>() as u32,
                null_mut(),
                0,
                &mut bytes,
                &mut ol,
            )
        } == 0
        {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(997) {
                return Err(e);
            }
            if unsafe { GetOverlappedResult(h, &mut ol, &mut bytes, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn is_available() -> bool {
        open().is_ok()
    }
    pub struct VirtualXboxPad {
        bus: Owned,
        command: Owned,
        feedback_event: Owned,
        serial: u32,
        // The kernel owns these two stable addresses until completion/cancellation.
        overlapped: Box<UnsafeCell<Overlapped>>,
        feedback: Box<UnsafeCell<Notification>>,
        pending: bool,
    }

    impl VirtualXboxPad {
        pub fn create(_slot_label: &str) -> Result<Self, String> {
            Self::attach().map_err(|e| format!("create ViGEmBus Xbox pad: {e}"))
        }
        fn attach() -> io::Result<Self> {
            let bus = open()?;
            let command = event()?;
            let feedback_event = event()?;
            control(bus.0, command.0, VERSION, &Request::new(1))?;
            let mut last = None;
            for serial in 1..=4 {
                let plug = Plugin {
                    size: size_of::<Plugin>() as u32,
                    serial,
                    kind: 0,
                    vendor: 0x045e,
                    product: 0x028e,
                };
                if let Err(e) = control(bus.0, command.0, PLUGIN, &plug) {
                    // An existing PDO with this serial belongs to another client.
                    if e.raw_os_error() == Some(87) {
                        last = Some(e);
                        continue;
                    }
                    return Err(e);
                }
                let pad = Self {
                    bus,
                    command,
                    serial,
                    overlapped: Box::new(UnsafeCell::new(Overlapped {
                        event: feedback_event.0,
                        ..Default::default()
                    })),
                    feedback: Box::new(UnsafeCell::new(Notification {
                        size: size_of::<Notification>() as u32,
                        serial,
                        large: 0,
                        small: 0,
                        led: 0,
                        padding: 0,
                    })),
                    feedback_event: feedback_event,
                    pending: false,
                };
                // WAIT_DEVICE_READY was added in v1.17. Older buses reject this optional request.
                if let Err(e) = control(pad.bus.0, pad.command.0, WAIT_READY, &Request::new(serial))
                    && !matches!(e.raw_os_error(), Some(1 | 50))
                {
                    return Err(e);
                }
                return Ok(pad);
            }
            Err(last.unwrap_or_else(|| io::Error::other("no free ViGEmBus serial")))
        }
        pub fn update(&mut self, state: VirtualPadState) -> Result<(), String> {
            control(
                self.bus.0,
                self.command.0,
                SUBMIT,
                &Submit::new(self.serial, state),
            )
            .map_err(|e| e.to_string())
        }
        pub fn poll_rumble(&mut self) -> Option<(u16, u16)> {
            let ol = self.overlapped.get();
            let msg = self.feedback.get();
            let len = size_of::<Notification>() as u32;
            let mut bytes = 0;
            if self.pending {
                if unsafe { GetOverlappedResult(self.bus.0, ol, &mut bytes, 0) } == 0 {
                    if io::Error::last_os_error().raw_os_error() != Some(996) {
                        self.pending = false;
                    }
                    return None;
                }
                self.pending = false;
            } else {
                unsafe {
                    ol.write(Overlapped {
                        event: self.feedback_event.0,
                        ..Default::default()
                    });
                }
                if unsafe {
                    DeviceIoControl(
                        self.bus.0,
                        NOTIFY,
                        msg.cast(),
                        len,
                        msg.cast(),
                        len,
                        &mut bytes,
                        ol,
                    )
                } == 0
                {
                    self.pending = io::Error::last_os_error().raw_os_error() == Some(997);
                    return None;
                }
            }
            (bytes >= len)
                .then(|| unsafe { (u16::from((*msg).large) * 257, u16::from((*msg).small) * 257) })
        }
    }
    impl Drop for VirtualXboxPad {
        fn drop(&mut self) {
            if self.pending {
                let ol = self.overlapped.get();
                unsafe {
                    CancelIoEx(self.bus.0, ol);
                    GetOverlappedResult(self.bus.0, ol, &mut 0, 1);
                }
            }
            let _ = control(
                self.bus.0,
                self.command.0,
                UNPLUG,
                &Request::new(self.serial),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vigem_v1_abi() {
        assert_eq!(
            [VERSION, PLUGIN, UNPLUG, SUBMIT, NOTIFY],
            [0x2aa00c, 0x2aa004, 0x2aa008, 0x2aa808, 0x2ae804]
        );
        assert_eq!(
            [
                size_of::<Request>(),
                size_of::<Plugin>(),
                size_of::<Report>(),
                size_of::<Submit>(),
                size_of::<Notification>()
            ],
            [8, 16, 12, 20, 12]
        );
        assert_eq!(Request::new(1).size, 8);
    }
    #[test]
    #[cfg(target_endian = "little")]
    fn wired_xusb_packet() {
        let req = Submit::new(
            7,
            VirtualPadState {
                buttons: 0x9401,
                lx: i16::MIN,
                ly: i16::MAX,
                rx: -2,
                ry: 1,
                lt: 3,
                rt: 255,
            },
        );
        let bytes = unsafe {
            std::slice::from_raw_parts((&raw const req).cast::<u8>(), size_of::<Submit>())
        };
        assert_eq!(
            bytes,
            &[
                20, 0, 0, 0, 7, 0, 0, 0, 1, 0x94, 3, 255, 0, 128, 255, 127, 254, 255, 1, 0
            ]
        );
    }
}
