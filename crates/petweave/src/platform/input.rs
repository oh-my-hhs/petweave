//! Global keyboard capture via evdev (`/dev/input/event*`).
//!
//! Wayland has no protocol for passive global keyboard monitoring, so — like
//! `wayland-bongocat` — we read evdev devices directly. This needs permission
//! on `/dev/input` (udev `uaccess` rule or the `input` group); `petweave
//! list-devices` reports what is visible.
//!
//! Each device gets a reader thread; events are pushed into the host's
//! calloop channel. Only key press/release events are forwarded — key *codes*
//! are not logged anywhere (see docs/TECH_STACK.md §4.3 privacy).

use std::fs;
use std::os::fd::{AsFd, AsRawFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use nix::errno::Errno;
use nix::fcntl::{open, OFlag};
use nix::poll::{poll, PollFd, PollFlags};
use nix::sys::stat::Mode;

use petweave_core::events::Event;

/// A discovered/configured keyboard device.
#[derive(Debug, Clone)]
pub struct KeyboardDevice {
    pub path: PathBuf,
    pub name: String,
}

const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;

/// ioctl request codes (Linux uapi/linux/input.h):
/// `EVIOCGNAME(len) = _IOC(READ, 'E', 0x06, len)`.
const EVIOCGNAME: libc::c_ulong = (2 << 30) | (256 << 16) | (0x45 << 8) | 0x06;
/// `EVIOCGBIT(ev, len) = _IOC(READ, 'E', 0x20 + ev, len)`.
const fn eviocgbit(ev: u16, len: usize) -> libc::c_ulong {
    (2 << 30) | ((len as libc::c_ulong) << 16) | (0x45 << 8) | (0x20 + ev as libc::c_ulong)
}

/// ACPI/system-button devices that are not keyboards.
///
/// NOTE: "AT Translated Set 2 keyboard" (real laptop keyboards) must NOT be
/// filtered out.
const NON_KEYBOARD_NAMES: &[&str] = &[
    "Power Button",
    "Video Bus",
    "Sleep Button",
    "Lid Switch",
    "WMI hotkeys",
    "ThinkPad Extra Buttons",
];

fn is_keyboard_name(name: &str) -> bool {
    !NON_KEYBOARD_NAMES.iter().any(|b| name.contains(b))
}

/// Scan `/dev/input/event*` and return devices that look like keyboards.
pub fn scan_keyboards() -> Vec<KeyboardDevice> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/dev/input") else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("event"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for p in paths {
        if let Some(dev) = probe_device(&p) {
            out.push(dev);
        }
    }
    out
}

/// Probe a single device path; returns `Some` if it looks like a keyboard.
pub fn probe_device(path: &Path) -> Option<KeyboardDevice> {
    let fd = open(
        path,
        OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .ok()?;

    // Name (EVIOCGNAME).
    let mut name_buf = [0u8; 256];
    let n = unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            EVIOCGNAME,
            name_buf.as_mut_ptr() as *mut libc::c_char,
        )
    };
    if n < 0 {
        return None;
    }
    let name = String::from_utf8_lossy(&name_buf[..n as usize])
        .trim()
        .to_string();

    // Supported event types bitmap (EVIOCGBIT(0, …)): keyboard = has EV_KEY,
    // not EV_REL (mouse/touchpad).
    let mut evbits = [0u8; 8];
    let r = unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            eviocgbit(0, evbits.len()),
            evbits.as_mut_ptr() as *mut libc::c_char,
        )
    };
    if r < 0 {
        return None;
    }
    let has_key = evbits[(EV_KEY / 8) as usize] & (1 << (EV_KEY % 8)) != 0;
    let has_rel = evbits[(EV_REL / 8) as usize] & (1 << (EV_REL % 8)) != 0;
    if !has_key || has_rel || !is_keyboard_name(&name) {
        return None;
    }

    Some(KeyboardDevice {
        path: path.to_path_buf(),
        name,
    })
}

/// Owns the reader threads for a set of devices.
pub struct InputReader {
    stop: Arc<AtomicBool>,
    _handles: Vec<thread::JoinHandle<()>>,
}

impl InputReader {
    /// Spawn one reader thread per device.
    pub fn start(devices: Vec<KeyboardDevice>, tx: calloop::channel::Sender<Event>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();
        for dev in devices {
            let tx = tx.clone();
            let stop = stop.clone();
            tracing::info!("watching keyboard {}", dev.path.display());
            handles.push(thread::spawn(move || read_loop(dev, tx, stop)));
        }
        Self {
            stop,
            _handles: handles,
        }
    }

    /// Ask all reader threads to exit (they poll with a 200ms timeout).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for InputReader {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Linux `struct input_event` (24 bytes on 64-bit LE).
#[repr(C)]
struct InputEventRaw {
    tv_sec: i64,
    tv_usec: i64,
    type_: u16,
    code: u16,
    value: i32,
}

fn read_loop(dev: KeyboardDevice, tx: calloop::channel::Sender<Event>, stop: Arc<AtomicBool>) {
    let Ok(fd) = open(
        &dev.path,
        OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) else {
        tracing::warn!("cannot open {} — check /dev/input permissions", dev.path.display());
        return;
    };
    let mut buf = [0u8; 24];
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut fds = [PollFd::new(fd.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, 200u16) {
            Ok(0) => continue, // timeout — re-check stop flag
            Ok(_) => {}
            Err(_) => break,
        }
        match nix::unistd::read(fd.as_fd(), &mut buf) {
            Ok(0) => break, // device closed
            Ok(n) if n == buf.len() => {
                let ev = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const InputEventRaw) };
                if ev.type_ == EV_KEY && (ev.value == 1 || ev.value == 0) {
                    let event = Event::Input(petweave_core::events::InputEvent {
                        device: dev.path.display().to_string(),
                        code: ev.code as u32,
                        pressed: ev.value == 1,
                    });
                    if tx.send(event).is_err() {
                        break; // host is gone
                    }
                }
            }
            Ok(_) => continue, // partial read (should not happen at 24B)
            Err(Errno::EAGAIN) => continue,
            Err(_) => break,
        }
    }
    tracing::debug!("input reader for {} stopped", dev.path.display());
}
