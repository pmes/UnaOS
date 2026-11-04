// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Comscan (CODEX §2 "IO Bridge"), seam host handler — the host serial port (termios, macOS and Linux) as a
//! `pump_core::Transport`: open, configure, enumerate, refuse by name. It owns no protocol (pump_core's) and no gate
//! (Helm's): it carries the bytes the pump sends and returns what the controller says.
//!
//! VUGCOMSCANTTY (LEDGER SR484). The port shape, measured against the pump that drives it:
//!
//! - Opened `O_RDWR | O_NOCTTY | O_NONBLOCK` and LEFT nonblocking, raw (`cfmakeraw(3)`), 8N1, `CLOCAL | CREAD`, no
//!   flow control (grbl) or `CRTSCTS` ([`Flow::RtsCts`], Marlin on some boards), `VMIN = 1, VTIME = 0`.
//!   `pump_core::Transport` says "Neither call blocks" and `Pump::tick` reads until `Ok(0)`: with `VTIME = 1` every
//!   tick would stall 100 ms on its last read inside the desk's Mutex. With `O_NONBLOCK` and `VMIN > 0` (termios(3),
//!   "MIN > 0, TIME == 0"), no data is `EAGAIN` (-> `Ok(0)`) and `read(2)` returning 0 is the end of file after a
//!   hangup (-> `LinkError::Closed`), as is `EIO`.
//! - `TIOCEXCL` after open: a second opener gets `EBUSY` ([`Refusal::Busy`]); ioctl_tty(2): CAP_SYS_ADMIN bypasses it.
//! - The baud by name ([`Baud`]). The standard rates go through `cfsetispeed(3)` / `cfsetospeed(3)`; Marlin's 250000
//!   is no `B` constant on Linux (libc has none) and is set with `termios2` + `BOTHER` via `TCGETS2` / `TCSETS2`
//!   (ioctl_tty(2)); on macOS with `IOSSIOSPEED` AFTER `tcsetattr` (which resets the speed).
//! - DTR: `open(2)` already raises DTR (the Arduino auto-reset edge), so no explicit pulse. A reset controller's
//!   bootloader eats what arrives in its first second or two, and `Pump::welcome` halts on a `Grbl ` banner after the
//!   first line went out; so [`Port::ready`] waits for the banner before a pump ever sees the port, and a grbl that
//!   stays silent (no auto-reset: grblHAL native USB) is sent make_core's `Realtime::SoftReset` once.
//! - Candidates by readdir of `/dev` (no daemon): macOS `cu.usbserial*` / `cu.usbmodem*`, Linux `ttyUSB*` / `ttyACM*`.
//! - A port lost mid-stream (EIO, end of file) is dropped by the desk, which releases the TIOCEXCL fd, and reopened
//!   by path through [`opener`] with the desk lock released, 250 ms doubling to 4 s (VUGCOMSCANRECONNECT, SR521); a
//!   device whose sysfs gave a VID:PID[:serial] at the first open is reopened by that identity wherever it came back
//!   ([`UsbId`], [`enumerate`], the `tty:usb:` scheme: VUGCOMSCANTTYUSBID, SR529).
//!
//! `unsafe` lives in four private fns, each with its SAFETY line: `termios_get`, `termios_set`, `exclusive`,
//! `nonstandard_speed`.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use make_core::gcode::Dialect;
use pump_core::{LinkError, Transport};

#[cfg(target_os = "linux")]
use libc::TIOCEXCL;
/// macOS `<sys/ttycom.h>`: `#define TIOCEXCL _IO('t', 13)` = 0x2000_0000 (IOC_VOID) | 't' << 8 | 13 (not in libc).
#[cfg(target_os = "macos")]
const TIOCEXCL: libc::c_ulong = 0x2000_740d;
/// macOS `<IOKit/serial/ioss.h>`: `#define IOSSIOSPEED _IOW('T', 2, speed_t)` = 0x8000_0000 (IOC_IN) |
/// sizeof(speed_t) = 8 << 16 | 'T' << 8 | 2 (speed_t is unsigned long; not in libc).
#[cfg(target_os = "macos")]
const IOSSIOSPEED: libc::c_ulong = 0x8008_5402;

/// How long [`Port::ready`] waits for a banner by default: an Arduino bootloader holds the line for ~0.5 s
/// (optiboot) to ~2 s (the old one) after the DTR reset.
pub const READY_WAIT: Duration = Duration::from_millis(2500);

/// The port names the host's controllers enumerate as.
#[cfg(target_os = "macos")]
pub const CANDIDATES: &[&str] = &["cu.usbserial", "cu.usbmodem"];
/// The port names the host's controllers enumerate as.
#[cfg(target_os = "linux")]
pub const CANDIDATES: &[&str] = &["ttyUSB", "ttyACM"];

/// The line rate by name: 115200 is grbl's, 250000 Marlin's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Baud {
    B9600,
    B19200,
    B38400,
    B57600,
    B115200,
    B230400,
    B250000,
}

impl Baud {
    pub const ALL: [Baud; 7] =
        [Baud::B9600, Baud::B19200, Baud::B38400, Baud::B57600, Baud::B115200, Baud::B230400, Baud::B250000];

    /// Bits per second.
    pub fn rate(self) -> u32 {
        match self {
            Baud::B9600 => 9600,
            Baud::B19200 => 19200,
            Baud::B38400 => 38400,
            Baud::B57600 => 57600,
            Baud::B115200 => 115200,
            Baud::B230400 => 230400,
            Baud::B250000 => 250000,
        }
    }

    /// The named rate for `rate`, or `BadBaud` by name.
    pub fn from_rate(rate: u32) -> Result<Baud, Refusal> {
        Baud::ALL.iter().copied().find(|b| b.rate() == rate).ok_or(Refusal::BadBaud { rate })
    }

    /// The termios `B` constant, or `None` for a rate set by the nonstandard ioctl.
    fn standard(self) -> Option<libc::speed_t> {
        match self {
            Baud::B9600 => Some(libc::B9600),
            Baud::B19200 => Some(libc::B19200),
            Baud::B38400 => Some(libc::B38400),
            Baud::B57600 => Some(libc::B57600),
            Baud::B115200 => Some(libc::B115200),
            Baud::B230400 => Some(libc::B230400),
            Baud::B250000 => None,
        }
    }
}

/// Hardware flow control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// grbl: none.
    None,
    /// RTS / CTS (`CRTSCTS`).
    RtsCts,
}

/// Why a port did not open or stopped answering, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// ENOENT / ENXIO / ENODEV: nothing at that path (unplugged).
    NoSuchPort { path: PathBuf },
    /// EBUSY: another opener holds it exclusively (TIOCEXCL).
    Busy { path: PathBuf },
    /// EACCES / EPERM: the user may not open it (on Linux, not in the `dialout` group).
    Denied { path: PathBuf },
    /// ENOTTY: the path is not a terminal.
    NotATty { path: PathBuf },
    /// A rate with no name in [`Baud`].
    BadBaud { rate: u32 },
    /// grbl said no banner, even after a soft reset.
    NoBanner { path: PathBuf, waited_ms: u64 },
    /// EIO or end of file: the device went away.
    Disconnected { path: PathBuf },
    /// Any other errno.
    Io { path: PathBuf, errno: i32 },
    /// VUGCOMSCANTTYUSBID (SR529): `n` ttys carry the asked VID:PID (and serial, when one was asked); never guessed.
    AmbiguousDevice { vid: u16, pid: u16, n: usize },
    /// VUGCOMSCANTTYUSBID (SR529): a `tty:usb:` port string that does not parse.
    BadUsbId { text: String },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoSuchPort { path } => write!(f, "no such port: {}", path.display()),
            Refusal::Busy { path } => write!(f, "port busy (held exclusively): {}", path.display()),
            Refusal::Denied { path } => write!(f, "permission denied: {}", path.display()),
            Refusal::NotATty { path } => write!(f, "not a terminal: {}", path.display()),
            Refusal::BadBaud { rate } => write!(f, "bad baud: {rate}"),
            Refusal::NoBanner { path, waited_ms } => {
                write!(f, "no banner from {} in {waited_ms} ms (after a soft reset)", path.display())
            }
            Refusal::Disconnected { path } => write!(f, "disconnected: {}", path.display()),
            Refusal::Io { path, errno } => write!(f, "i/o error {errno}: {}", path.display()),
            Refusal::AmbiguousDevice { vid, pid, n } => {
                write!(f, "ambiguous device {vid:04x}:{pid:04x}: {n} ttys match (name the serial)")
            }
            Refusal::BadUsbId { text } => write!(f, "bad usb id: {text:?} (tty:usb:<vid>:<pid>[:<serial>])"),
        }
    }
}

/// The refusal an errno names for `path`.
pub fn refusal_of(errno: i32, path: &Path) -> Refusal {
    let path = path.to_path_buf();
    match errno {
        libc::ENOENT | libc::ENXIO | libc::ENODEV => Refusal::NoSuchPort { path },
        libc::EBUSY => Refusal::Busy { path },
        libc::EACCES | libc::EPERM => Refusal::Denied { path },
        libc::ENOTTY => Refusal::NotATty { path },
        libc::EIO => Refusal::Disconnected { path },
        _ => Refusal::Io { path, errno },
    }
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// An open, configured serial port.
pub struct Port {
    file: File,
    path: PathBuf,
}

impl Port {
    /// Open `path` at `baud` with no flow control (grbl).
    pub fn open(path: &Path, baud: Baud) -> Result<Port, Refusal> {
        Port::open_flow(path, baud, Flow::None)
    }

    /// Open `path` at `baud` with `flow`: exclusive, raw 8N1, nonblocking.
    pub fn open_flow(path: &Path, baud: Baud, flow: Flow) -> Result<Port, Refusal> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| refusal_of(e.raw_os_error().unwrap_or(0), path))?;
        let fd = file.as_raw_fd();
        exclusive(fd).map_err(|e| refusal_of(e, path))?;
        let mut t = termios_get(fd).map_err(|e| refusal_of(e, path))?;
        shape(&mut t, flow);
        termios_set(fd, &mut t, baud.standard().unwrap_or(libc::B9600)).map_err(|e| refusal_of(e, path))?;
        if baud.standard().is_none() {
            nonstandard_speed(fd, baud.rate()).map_err(|e| refusal_of(e, path))?;
        }
        Ok(Port { file, path: path.to_path_buf() })
    }

    /// The path it was opened by.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Wait for the controller's banner before any pump sees the port: `crate::pump::ready` (the transport-neutral
    /// wait, lifted from here by SR487) with its outcome named for this path.
    pub fn ready(&mut self, dialect: Dialect, wait: Duration) -> Result<Option<String>, Refusal> {
        crate::pump::ready(self, dialect, wait).map_err(|u| match u {
            crate::pump::Unready::NoBanner { waited_ms } => Refusal::NoBanner { path: self.path.clone(), waited_ms },
            crate::pump::Unready::Stalled => Refusal::Io { path: self.path.clone(), errno: libc::EAGAIN },
            crate::pump::Unready::Closed => Refusal::Disconnected { path: self.path.clone() },
        })
    }
}

impl AsRawFd for Port {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

impl Transport for Port {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, LinkError> {
        match self.file.write(bytes) {
            Ok(n) => Ok(n),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(0),
            Err(_) => Err(LinkError::Closed),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.file.read(buf) {
            // VMIN = 1 with O_NONBLOCK: 0 is the end of file after a hangup, never "nothing yet".
            Ok(0) => Err(LinkError::Closed),
            Ok(n) => Ok(n),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(0),
            Err(_) => Err(LinkError::Closed),
        }
    }
}

/// The candidate controller ports under `/dev`, sorted.
pub fn list() -> Vec<PathBuf> {
    list_in(Path::new("/dev"))
}

/// The entries of `dir` whose names start with a [`CANDIDATES`] prefix, sorted (an unreadable dir is empty).
pub fn list_in(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|n| CANDIDATES.iter().any(|p| n.starts_with(p))))
            .map(|e| e.path())
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// The host's opener for `crate::pump::serve_pump`: a port name opened at `baud` / `flow` and made ready for the
/// dialect each open is handed (VUGCOMSCANHOSTDIALECT, SR566); a refusal is its name.
pub fn opener(baud: Baud, flow: Flow) -> Box<crate::pump::Opener> {
    opener_in(PathBuf::from("/sys"), PathBuf::from("/dev"), baud, flow) // VUGCOMSCANTTYUSBID (SR529); VUGCOMSCANHOSTDIALECT (SR566)
}

/// The flags `cfmakeraw` leaves alone: the receiver on, modem lines ignored, one stop bit, the flow control, no
/// software flow, and a read that returns what is there (VMIN = 1, VTIME = 0 under O_NONBLOCK).
fn shape(t: &mut libc::termios, flow: Flow) {
    t.c_cflag |= libc::CLOCAL | libc::CREAD;
    t.c_cflag &= !libc::CSTOPB;
    match flow {
        Flow::None => t.c_cflag &= !libc::CRTSCTS,
        Flow::RtsCts => t.c_cflag |= libc::CRTSCTS,
    }
    t.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
    t.c_cc[libc::VMIN] = 1;
    t.c_cc[libc::VTIME] = 0;
}

fn termios_get(fd: RawFd) -> Result<libc::termios, i32> {
    // SAFETY: termios is plain old data (all-zero is a valid value) and tcgetattr only writes our local struct for fd,
    // an open descriptor the caller's File owns.
    let (rc, t) = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        let rc = libc::tcgetattr(fd, &mut t);
        (rc, t)
    };
    if rc != 0 { Err(errno()) } else { Ok(t) }
}

fn termios_set(fd: RawFd, t: &mut libc::termios, speed: libc::speed_t) -> Result<(), i32> {
    // SAFETY: t is an initialised termios we own (cfmakeraw / cfset*speed only edit it), and tcsetattr / tcflush act
    // only on fd, an open descriptor the caller's File owns.
    let rc = unsafe {
        libc::cfmakeraw(&mut *t);
        if libc::cfsetispeed(&mut *t, speed) != 0 || libc::cfsetospeed(&mut *t, speed) != 0 {
            -1
        } else if libc::tcsetattr(fd, libc::TCSANOW, &*t) != 0 {
            -1
        } else {
            libc::tcflush(fd, libc::TCIOFLUSH)
        }
    };
    if rc != 0 { Err(errno()) } else { Ok(()) }
}

fn exclusive(fd: RawFd) -> Result<(), i32> {
    // SAFETY: TIOCEXCL takes no argument and only marks fd (an open tty the caller's File owns) exclusive.
    let rc = unsafe { libc::ioctl(fd, TIOCEXCL) };
    if rc != 0 { Err(errno()) } else { Ok(()) }
}

#[cfg(target_os = "linux")]
fn nonstandard_speed(fd: RawFd, rate: u32) -> Result<(), i32> {
    // SAFETY: termios2 is plain old data (all-zero is valid); TCGETS2 writes it and TCSETS2 reads it, both on fd, an
    // open tty the caller's File owns.
    let rc = unsafe {
        let mut t2: libc::termios2 = std::mem::zeroed();
        if libc::ioctl(fd, libc::TCGETS2, &mut t2 as *mut libc::termios2) != 0 {
            -1
        } else {
            t2.c_cflag &= !libc::CBAUD;
            t2.c_cflag |= libc::BOTHER;
            t2.c_ispeed = rate as libc::speed_t;
            t2.c_ospeed = rate as libc::speed_t;
            libc::ioctl(fd, libc::TCSETS2, &t2 as *const libc::termios2)
        }
    };
    if rc != 0 { Err(errno()) } else { Ok(()) }
}

#[cfg(target_os = "macos")]
fn nonstandard_speed(fd: RawFd, rate: u32) -> Result<(), i32> {
    let speed: libc::speed_t = rate as libc::speed_t;
    // SAFETY: IOSSIOSPEED reads one speed_t from the pointer we pass (a live local) and acts only on fd, an open tty
    // the caller's File owns.
    let rc = unsafe { libc::ioctl(fd, IOSSIOSPEED, &speed as *const libc::speed_t) };
    if rc != 0 { Err(errno()) } else { Ok(()) }
}

// --- VUGCOMSCANTTYUSBID (LEDGER SR529): the device by VID:PID[:serial] from sysfs, reopened wherever it came back (named tail hunk) ---

/// The port scheme that names a USB serial device by identity: `tty:usb:<vid>:<pid>[:<serial>]` (hex vid / pid).
pub const USB_SCHEME: &str = "tty:usb:";

/// A USB serial device's identity as sysfs gives it: `idVendor`, `idProduct` and, when the device has one, `serial`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbId {
    pub vid: u16,
    pub pid: u16,
    pub serial: Option<String>,
}

impl UsbId {
    /// The identity a `tty:usb:<vid>:<pid>[:<serial>]` string names (the serial is the rest, colons kept; empty is
    /// none), or `BadUsbId` by name.
    pub fn parse(text: &str) -> Result<UsbId, Refusal> {
        let bad = || Refusal::BadUsbId { text: text.to_string() };
        let rest = text.strip_prefix(USB_SCHEME).ok_or_else(bad)?;
        let mut it = rest.splitn(3, ':');
        let vid = it.next().and_then(hex16).ok_or_else(bad)?;
        let pid = it.next().and_then(hex16).ok_or_else(bad)?;
        let serial = it.next().filter(|s| !s.is_empty()).map(|s| s.to_string());
        Ok(UsbId { vid, pid, serial })
    }

    /// The port string that names this identity on the bus.
    pub fn scheme(&self) -> String {
        match &self.serial {
            Some(s) => format!("{USB_SCHEME}{:04x}:{:04x}:{s}", self.vid, self.pid),
            None => format!("{USB_SCHEME}{:04x}:{:04x}", self.vid, self.pid),
        }
    }
}

impl fmt::Display for UsbId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.scheme())
    }
}

/// One to four hex digits as a u16 (sysfs writes `idVendor` as `2341`).
fn hex16(s: &str) -> Option<u16> {
    let s = s.trim();
    if s.is_empty() || s.len() > 4 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(s, 16).ok()
}

/// The identity of the tty `name` under the sysfs root `sys`: `sys/class/tty/<name>/device` resolved, then that dir,
/// its parent (cdc_acm: `device` is the interface) and its grandparent (usb-serial: `device` is the port under the
/// interface) searched for the first holding `idVendor`. `None` for a tty with no USB device (a pty, ttyS, macOS).
pub fn usb_id(sys: &Path, name: &str) -> Option<UsbId> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let mut d = std::fs::canonicalize(sys.join("class/tty").join(name).join("device")).ok()?;
    for _ in 0..3 {
        if d.join("idVendor").is_file() {
            let vid = hex16(&std::fs::read_to_string(d.join("idVendor")).ok()?)?;
            let pid = hex16(&std::fs::read_to_string(d.join("idProduct")).ok()?)?;
            let serial = std::fs::read_to_string(d.join("serial"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return Some(UsbId { vid, pid, serial });
        }
        if !d.pop() {
            return None;
        }
    }
    None
}

/// The node under `dev` that carries `want`, by reading `sys/class/tty` (sorted): its VID:PID and, when `want` names
/// one, its serial. None is `NoSuchPort` by the scheme string; more than one is `AmbiguousDevice` (a serial-less
/// pair, or two ttys sharing a serial), never a guess.
pub fn enumerate(sys: &Path, dev: &Path, want: &UsbId) -> Result<PathBuf, Refusal> {
    let mut names: Vec<String> = match std::fs::read_dir(sys.join("class/tty")) {
        Ok(rd) => rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().to_str().map(|n| n.to_string())).collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    let hits: Vec<String> = names
        .into_iter()
        .filter(|n| {
            usb_id(sys, n).is_some_and(|u| {
                u.vid == want.vid && u.pid == want.pid && (want.serial.is_none() || u.serial == want.serial)
            })
        })
        .collect();
    match hits.len() {
        0 => Err(Refusal::NoSuchPort { path: PathBuf::from(want.scheme()) }),
        1 => Ok(dev.join(&hits[0])),
        n => Err(Refusal::AmbiguousDevice { vid: want.vid, pid: want.pid, n }),
    }
}

/// The desk's identity hook (`crate::pump::PumpDesk::identify_by`) over the host's `/sys`.
pub fn identify() -> Box<crate::pump::Identify> {
    identify_in(PathBuf::from("/sys"))
}

/// The identity hook over the sysfs root `sys`: a port path's file name looked up as a tty, its [`UsbId`] as the
/// `tty:usb:` string the redial opens; `None` for a port already named by identity or a tty with no USB device.
pub fn identify_in(sys: PathBuf) -> Box<crate::pump::Identify> {
    Box::new(move |port: &str| -> Option<String> {
        if port.starts_with(USB_SCHEME) {
            return None;
        }
        let name = Path::new(port).file_name()?.to_str()?;
        usb_id(&sys, name).map(|u| u.scheme())
    })
}

/// [`opener`] over the roots `sys` and `dev`: a port starting [`USB_SCHEME`] is resolved by [`enumerate`] to its node
/// first, any other port is a path; then opened at `baud` / `flow` and made ready for the dialect it is handed.
pub fn opener_in(sys: PathBuf, dev: PathBuf, baud: Baud, flow: Flow) -> Box<crate::pump::Opener> {
    Box::new(move |port: &str, dialect: Dialect| -> Result<Box<dyn Transport + Send>, String> { // VUGCOMSCANHOSTDIALECT (SR566)
        let path = if port.starts_with(USB_SCHEME) {
            let id = UsbId::parse(port).map_err(|r| r.to_string())?;
            enumerate(&sys, &dev, &id).map_err(|r| r.to_string())?
        } else {
            PathBuf::from(port)
        };
        let mut p = Port::open_flow(&path, baud, flow).map_err(|r| r.to_string())?;
        p.ready(dialect, READY_WAIT).map_err(|r| r.to_string())?;
        Ok(Box::new(p))
    })
}
