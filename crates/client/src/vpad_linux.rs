use super::VirtualPadState;
use libc::{_IO, _IOW, _IOWR, input_event, uinput_ff_erase, uinput_ff_upload, uinput_user_dev};
use std::{
    fs::{File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};

const U: u32 = b'U' as u32;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const EV_FF: u16 = 21;
const FF_RUMBLE: u16 = 0x50;
const EV_UINPUT: u16 = 0x101;
// Linux xpad uses BTN_X (alias BTN_NORTH) for X and BTN_Y (BTN_WEST) for Y.
const BUTTONS: [(u16, u16); 11] = [
    (0x1000, 0x130),
    (0x2000, 0x131),
    (0x4000, 0x133),
    (0x8000, 0x134),
    (0x0100, 0x136),
    (0x0200, 0x137),
    (0x0020, 0x13a),
    (0x0010, 0x13b),
    (0x0400, 0x13c),
    (0x0040, 0x13d),
    (0x0080, 0x13e),
];
const AXES: [u16; 8] = [0, 1, 3, 4, 2, 5, 16, 17];

fn open() -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/uinput")
}
pub fn is_available() -> bool {
    open().is_ok()
}

fn checked(rc: libc::c_int) -> io::Result<()> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn axis_values(s: VirtualPadState) -> [i32; 8] {
    let bit = |mask: u16| i32::from(s.buttons & mask != 0);
    [
        s.lx.into(),
        s.ly.saturating_neg().into(),
        s.rx.into(),
        s.ry.saturating_neg().into(),
        s.lt.into(),
        s.rt.into(),
        bit(8) - bit(4),
        bit(2) - bit(1),
    ]
}

pub struct VirtualXboxPad {
    file: File,
    previous: Option<VirtualPadState>,
    effects: [(u16, u16); 16],
}

impl VirtualXboxPad {
    pub fn create(slot_label: &str) -> Result<Self, String> {
        Self::open_device(slot_label).map_err(|e| format!("create uinput Xbox pad: {e}"))
    }
    fn open_device(slot_label: &str) -> io::Result<Self> {
        let file = open()?;
        let fd = file.as_raw_fd();
        // Readers can exclude Aurora outputs without excluding real Microsoft pads.
        let phys = std::ffi::CString::new(format!("aurora-vpad/{slot_label}"))?;
        checked(unsafe { libc::ioctl(fd, _IOW::<*const libc::c_char>(U, 108), phys.as_ptr()) })?;
        let enable = |nr, bit: u16| {
            checked(unsafe { libc::ioctl(fd, _IOW::<i32>(U, nr), libc::c_int::from(bit)) })
        };
        for kind in [EV_KEY, EV_ABS, EV_FF] {
            enable(100, kind)?;
        }
        for (_, key) in BUTTONS {
            enable(101, key)?;
        }
        enable(107, FF_RUMBLE)?;
        let mut dev: uinput_user_dev = unsafe { std::mem::zeroed() };
        dev.id = libc::input_id {
            bustype: 3,
            vendor: 0x045e,
            product: 0x028e,
            version: 0x0110,
        };
        for (dst, src) in dev.name.iter_mut().zip(b"Microsoft X-Box 360 pad") {
            *dst = *src as _;
        }
        dev.ff_effects_max = 16;
        for axis in AXES {
            enable(103, axis)?;
            let i = usize::from(axis);
            (dev.absmin[i], dev.absmax[i], dev.absfuzz[i], dev.absflat[i]) = match axis {
                2 | 5 => (0, 255, 0, 0),
                16 | 17 => (-1, 1, 0, 0),
                _ => (-32768, 32767, 16, 128),
            };
        }
        write_records(&file, std::slice::from_ref(&dev))?;
        checked(unsafe { libc::ioctl(fd, _IO(U, 1)) })?;
        Ok(Self {
            file,
            previous: Some(VirtualPadState::default()),
            effects: [(0, 0); 16],
        })
    }
    pub fn update(&mut self, state: VirtualPadState) -> Result<(), String> {
        if self.previous == Some(state) {
            return Ok(());
        }
        let mut events = [unsafe { std::mem::zeroed::<input_event>() }; 20];
        let mut n = 0;
        let mut emit = |kind, code, value| {
            events[n].type_ = kind;
            events[n].code = code;
            events[n].value = value;
            n += 1;
        };
        for (mask, key) in BUTTONS {
            if self
                .previous
                .is_none_or(|s| (s.buttons ^ state.buttons) & mask != 0)
            {
                emit(EV_KEY, key, i32::from(state.buttons & mask != 0));
            }
        }
        let old = self.previous.map(axis_values);
        for (i, v) in axis_values(state).into_iter().enumerate() {
            if old.is_none_or(|a| a[i] != v) {
                emit(EV_ABS, AXES[i], v);
            }
        }
        emit(0, 0, 0);
        self.previous = None; // A partial write requires a complete state refresh.
        write_records(&self.file, &events[..n]).map_err(|e| e.to_string())?;
        self.previous = Some(state);
        Ok(())
    }
    pub fn poll_rumble(&mut self) -> Option<(u16, u16)> {
        let fd = self.file.as_raw_fd();
        for _ in 0..32 {
            let mut ev: input_event = unsafe { std::mem::zeroed() };
            if unsafe { libc::read(fd, (&raw mut ev).cast(), size_of::<input_event>()) }
                != size_of::<input_event>() as isize
            {
                return None;
            }
            match (ev.type_, ev.code) {
                (EV_UINPUT, 1) => {
                    let mut up: uinput_ff_upload = unsafe { std::mem::zeroed() };
                    up.request_id = ev.value as u32;
                    if unsafe { libc::ioctl(fd, _IOWR::<uinput_ff_upload>(U, 200), &raw mut up) }
                        < 0
                    {
                        continue;
                    }
                    up.retval = -libc::EINVAL;
                    if up.effect.type_ == FF_RUMBLE
                        && let Some(entry) = self.effects.get_mut(up.effect.id as usize)
                    {
                        // libc represents the kernel union as a suitably aligned integer array.
                        let effect =
                            unsafe { &*up.effect.u.as_ptr().cast::<libc::ff_rumble_effect>() };
                        *entry = (effect.strong_magnitude, effect.weak_magnitude);
                        up.retval = 0;
                    }
                    unsafe { libc::ioctl(fd, _IOW::<uinput_ff_upload>(U, 201), &raw const up) };
                }
                (EV_UINPUT, 2) => {
                    let mut erase: uinput_ff_erase = unsafe { std::mem::zeroed() };
                    erase.request_id = ev.value as u32;
                    if unsafe { libc::ioctl(fd, _IOWR::<uinput_ff_erase>(U, 202), &raw mut erase) }
                        < 0
                    {
                        continue;
                    }
                    erase.retval =
                        if let Some(entry) = self.effects.get_mut(erase.effect_id as usize) {
                            *entry = (0, 0);
                            0
                        } else {
                            -libc::EINVAL
                        };
                    unsafe { libc::ioctl(fd, _IOW::<uinput_ff_erase>(U, 203), &raw const erase) };
                }
                (EV_FF, id) if (id as usize) < self.effects.len() => {
                    return Some(if ev.value > 0 {
                        self.effects[id as usize]
                    } else {
                        (0, 0)
                    });
                }
                _ => {}
            }
        }
        None
    }
}

fn write_records<T>(file: &File, records: &[T]) -> io::Result<()> {
    let bytes = std::mem::size_of_val(records);
    let rc = unsafe { libc::write(file.as_raw_fd(), records.as_ptr().cast(), bytes) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else if rc as usize != bytes {
        Err(io::ErrorKind::WriteZero.into())
    } else {
        Ok(())
    }
}

impl Drop for VirtualXboxPad {
    fn drop(&mut self) {
        unsafe { libc::ioctl(self.file.as_raw_fd(), _IO(U, 2)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sticks_and_hats() {
        for bits in 0..16 {
            let axes = axis_values(VirtualPadState {
                buttons: bits,
                ly: i16::MIN,
                ry: i16::MAX,
                ..Default::default()
            });
            assert_eq!((axes[1], axes[3]), (32767, -32767));
            assert_eq!(
                axes[6],
                match bits & 12 {
                    8 => 1,
                    4 => -1,
                    _ => 0,
                }
            );
            assert_eq!(
                axes[7],
                match bits & 3 {
                    2 => 1,
                    1 => -1,
                    _ => 0,
                }
            );
        }
    }
    #[test]
    fn xpad_button_map() {
        let mut by_bit = BUTTONS;
        by_bit.sort_by_key(|(mask, _)| *mask);
        assert_eq!(
            by_bit.map(|(mask, _)| mask),
            [16, 32, 64, 128, 256, 512, 1024, 4096, 8192, 16384, 32768]
        );
        assert_eq!(
            by_bit.map(|(_, key)| key),
            [315, 314, 317, 318, 310, 311, 316, 304, 305, 307, 308]
        );
    }
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn linux_uapi_numbers() {
        assert_eq!([_IO(U, 1), _IO(U, 2)], [0x5501, 0x5502]);
        assert_eq!(
            [100, 101, 103, 107].map(|n| _IOW::<i32>(U, n)),
            [0x40045564, 0x40045565, 0x40045567, 0x4004556b]
        );
        assert_eq!(_IOWR::<uinput_ff_upload>(U, 200), 0xc06855c8);
        assert_eq!(_IOW::<uinput_ff_upload>(U, 201), 0x406855c9);
        assert_eq!(_IOWR::<uinput_ff_erase>(U, 202), 0xc00c55ca);
        assert_eq!(_IOW::<uinput_ff_erase>(U, 203), 0x400c55cb);
    }
}
