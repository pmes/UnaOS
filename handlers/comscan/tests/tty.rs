// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VUGCOMSCANTTY (SR484): the host serial port as the pump's Transport, exercised over a pseudo-terminal pair
// (openpty(3) through the same libc): the slave is opened by path exactly as a /dev/ttyACM0 would be, and a fake grbl
// on the master answers. No real controller is touched.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bandy::{Origin, PumpEvent}; // VUGCOMSCANTTYUSBID (SR529): the desk road for the renamed-node redial
use comscan::pump::{Opener, PumpDesk, Verdict, digest_hex};
use comscan::tty::{Baud, CANDIDATES, Flow, Port, Refusal, list, list_in, opener, refusal_of};
use comscan::tty::{USB_SCHEME, UsbId, enumerate, identify_in, opener_in, usb_id};
use make_core::gcode::Dialect;
use make_core::{
    Aabb3, Axis, AxisId, Kinematics, Machine, MachineKind, Move, Power, PowerKind, Program, Tool, ToolKind, Units, Vec3,
};
use pump_core::{Event, LinkError, Pump, PumpState, Transport};

const GRBL: Dialect = Dialect::Grbl { laser: false };
const BANNER: &[u8] = b"Grbl 1.1h ['$' for help]\r\n";

/// A pty pair: the master as a File, the slave's path, and the slave fd openpty returned (kept until dropped).
fn pty() -> (File, PathBuf, OwnedFd) {
    let (mut m, mut s): (libc::c_int, libc::c_int) = (-1, -1);
    // SAFETY: openpty writes the two fds; the name / termios / winsize arguments are null (not wanted).
    let rc = unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            std::ptr::null_mut::<libc::c_char>(),
            std::ptr::null_mut::<libc::termios>(),
            std::ptr::null_mut::<libc::winsize>(),
        )
    };
    assert_eq!(rc, 0, "openpty");
    let mut name = [0 as libc::c_char; 128];
    // SAFETY: s is the slave fd just opened; name is a writable buffer of the length passed.
    let rc = unsafe { libc::ttyname_r(s, name.as_mut_ptr(), name.len()) };
    assert_eq!(rc, 0, "ttyname_r");
    // SAFETY: ttyname_r wrote a NUL-terminated string into name.
    let path = PathBuf::from(unsafe { CStr::from_ptr(name.as_ptr()) }.to_str().unwrap());
    // SAFETY: m and s are fresh fds owned by nothing else.
    let (master, slave) = unsafe { (File::from_raw_fd(m), OwnedFd::from_raw_fd(s)) };
    (master, path, slave)
}

/// A fake grbl on the master: says its banner at once (or only after a soft reset when `banner_on_reset`), answers
/// `ok` per line, ignores the realtime bytes; returns the lines it executed and the master once `lines` lines arrived
/// (and, with `banner_on_reset`, the reset was seen). It answers `$I` as build `VER_A` (VUGCOMSCANHELLOON, SR549).
fn fake_grbl(m: File, banner_on_reset: bool, lines: usize) -> JoinHandle<(Vec<String>, File)> {
    fake_grbl_as(m, banner_on_reset, lines, VER_A)
}

/// [`fake_grbl`] answering `$I` as build `ver` (`$I` is neither recorded nor counted as a line).
fn fake_grbl_as(mut m: File, banner_on_reset: bool, lines: usize, ver: &'static str) -> JoinHandle<(Vec<String>, File)> {
    thread::spawn(move || {
        if !banner_on_reset {
            m.write_all(BANNER).unwrap();
        }
        let mut got = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        let mut reset_seen = false;
        let mut buf = [0u8; 256];
        while got.len() < lines || (banner_on_reset && !reset_seen) {
            let n = match m.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                match b {
                    0x18 => {
                        reset_seen = true;
                        m.write_all(b"\r\n").unwrap();
                        m.write_all(BANNER).unwrap();
                    }
                    b'?' | b'!' | b'~' => {}
                    b'\n' => {
                        let line = String::from_utf8_lossy(&cur).trim().to_string();
                        cur.clear();
                        if !said_hello(&mut m, &line, ver) {
                            got.push(line);
                            m.write_all(b"ok\r\n").unwrap();
                        }
                    }
                    _ => cur.push(b),
                }
            }
        }
        (got, m)
    })
}

#[test]
fn a_pumped_grbl_line_and_its_ok_cross_the_port() {
    let (m, path, _s) = pty();
    let mut port = Port::open(&path, Baud::B115200).expect("open the pty slave");
    let fake = fake_grbl(m, false, 2);
    let banner = port.ready(GRBL, Duration::from_secs(2)).expect("ready");
    assert!(banner.as_deref().is_some_and(|b| b.starts_with("Grbl 1.1h")), "banner {banner:?}");

    let mut pump = Pump::new(GRBL, "G0 X1\nG1 X2 F100\n").expect("pump");
    let t0 = Instant::now();
    let (mut oks, mut done) = (0, false);
    while !done && t0.elapsed() < Duration::from_secs(5) {
        for e in pump.tick(&mut port, t0.elapsed().as_micros() as u64) {
            match e {
                Event::Ok(_) => oks += 1,
                Event::Done => done = true,
                Event::LinkLost => panic!("link lost"),
                _ => {}
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(done, "the pump reached Done");
    assert_eq!(oks, 2);
    assert_eq!(pump.progress().acked, 2);
    let (lines, _m) = fake.join().unwrap();
    assert_eq!(lines, vec!["G0 X1".to_string(), "G1 X2 F100".to_string()]);
}

#[test]
fn a_silent_grbl_is_soft_reset_once_and_then_says_its_banner() {
    let (m, path, _s) = pty();
    let mut port = Port::open(&path, Baud::B115200).expect("open");
    let fake = fake_grbl(m, true, 0);
    let banner = port.ready(GRBL, Duration::from_millis(300)).expect("ready after the reset");
    assert!(banner.as_deref().is_some_and(|b| b.starts_with("Grbl ")), "banner {banner:?}");
    fake.join().unwrap();
}

#[test]
fn a_silent_marlin_proceeds_without_a_banner() {
    let (_m, path, _s) = pty();
    let mut port = Port::open(&path, Baud::B115200).expect("open");
    assert_eq!(port.ready(Dialect::Marlin, Duration::from_millis(50)), Ok(None));
}

fn speed_of(b: Baud) -> libc::speed_t {
    match b {
        Baud::B9600 => libc::B9600,
        Baud::B19200 => libc::B19200,
        Baud::B38400 => libc::B38400,
        Baud::B57600 => libc::B57600,
        Baud::B115200 => libc::B115200,
        Baud::B230400 => libc::B230400,
        Baud::B250000 => unreachable!("set by the nonstandard ioctl"),
    }
}

#[test]
fn every_named_baud_round_trips_and_reads_back_from_the_port() {
    for b in Baud::ALL {
        assert_eq!(Baud::from_rate(b.rate()), Ok(b));
        if b == Baud::B250000 && cfg!(not(target_os = "linux")) {
            continue; // a macOS pty is no IOSSIOSPEED device; the real port is metal's
        }
        let (_m, path, _s) = pty();
        let port = Port::open(&path, b).unwrap_or_else(|r| panic!("open at {}: {r}", b.rate()));
        let fd = port.as_raw_fd();
        if b == Baud::B250000 {
            #[cfg(target_os = "linux")]
            {
                // SAFETY: termios2 is plain old data; TCGETS2 fills it from our open fd.
                let (rc, t2) = unsafe {
                    let mut t2: libc::termios2 = std::mem::zeroed();
                    let rc = libc::ioctl(fd, libc::TCGETS2, &mut t2 as *mut libc::termios2);
                    (rc, t2)
                };
                assert_eq!(rc, 0, "TCGETS2");
                assert_eq!(t2.c_ospeed, 250000);
                assert_eq!(t2.c_cflag & libc::CBAUD, libc::BOTHER);
            }
            continue;
        }
        // SAFETY: termios is plain old data; tcgetattr fills it from our open fd.
        let (rc, t) = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            let rc = libc::tcgetattr(fd, &mut t);
            (rc, t)
        };
        assert_eq!(rc, 0, "tcgetattr");
        // SAFETY: t is an initialised termios.
        assert_eq!(unsafe { libc::cfgetospeed(&t) }, speed_of(b), "rate {}", b.rate());
        assert_eq!(t.c_cflag & libc::CSIZE, libc::CS8);
        assert_eq!(t.c_cflag & (libc::PARENB | libc::CSTOPB | libc::CRTSCTS), 0);
        assert_eq!(t.c_cflag & (libc::CLOCAL | libc::CREAD), libc::CLOCAL | libc::CREAD);
        assert_eq!(t.c_lflag & (libc::ICANON | libc::ECHO), 0);
        assert_eq!((t.c_cc[libc::VMIN], t.c_cc[libc::VTIME]), (1, 0));
    }
    assert_eq!(Baud::from_rate(12345), Err(Refusal::BadBaud { rate: 12345 }));
}

#[test]
fn rts_cts_is_set_when_asked() {
    let (_m, path, _s) = pty();
    let port = Port::open_flow(&path, Baud::B115200, Flow::RtsCts).expect("open");
    // SAFETY: termios is plain old data; tcgetattr fills it from our open fd.
    let (rc, t) = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        let rc = libc::tcgetattr(port.as_raw_fd(), &mut t);
        (rc, t)
    };
    assert_eq!(rc, 0);
    assert_eq!(t.c_cflag & libc::CRTSCTS, libc::CRTSCTS);
}

#[test]
fn list_names_only_candidate_ports() {
    for p in list() {
        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(CANDIDATES.iter().any(|c| n.starts_with(c)), "{}", p.display());
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("comscan-tty-list");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = format!("{}1", CANDIDATES[1]);
    let b = format!("{}0", CANDIDATES[0]);
    for n in [a.as_str(), b.as_str(), "ttyS0", "sda", "console"] {
        File::create(dir.join(n)).unwrap();
    }
    let mut want = vec![dir.join(&a), dir.join(&b)];
    want.sort();
    assert_eq!(list_in(&dir), want);
    assert!(list_in(&dir.join("absent")).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_port_refuses_no_such_port_by_name() {
    let p = Path::new("/dev/comscan-no-such-port");
    match Port::open(p, Baud::B115200) {
        Err(Refusal::NoSuchPort { path }) => assert_eq!(path, p),
        Err(r) => panic!("wrong refusal: {r}"),
        Ok(_) => panic!("opened a missing port"),
    }
    let mut open = opener(Baud::B115200, Flow::None);
    match open("/dev/comscan-no-such-port", GRBL) {
        Err(why) => assert!(why.contains("no such port"), "{why}"),
        Ok(_) => panic!("opened a missing port"),
    }
}

#[test]
fn a_hung_up_port_reads_closed() {
    let (m, path, s) = pty();
    let mut port = Port::open(&path, Baud::B115200).expect("open");
    let mut buf = [0u8; 16];
    assert_eq!(Transport::read(&mut port, &mut buf), Ok(0), "nothing yet is Ok(0), not a block");
    drop(s);
    drop(m);
    assert_eq!(Transport::read(&mut port, &mut buf), Err(LinkError::Closed));
}

#[test]
fn errnos_map_to_refusals_by_name() {
    let p = Path::new("/dev/ttyACM9");
    let path = p.to_path_buf();
    assert_eq!(refusal_of(libc::EBUSY, p), Refusal::Busy { path: path.clone() });
    assert_eq!(refusal_of(libc::ENOENT, p), Refusal::NoSuchPort { path: path.clone() });
    assert_eq!(refusal_of(libc::ENXIO, p), Refusal::NoSuchPort { path: path.clone() });
    assert_eq!(refusal_of(libc::EACCES, p), Refusal::Denied { path: path.clone() });
    assert_eq!(refusal_of(libc::ENOTTY, p), Refusal::NotATty { path: path.clone() });
    assert_eq!(refusal_of(libc::EIO, p), Refusal::Disconnected { path: path.clone() });
    assert_eq!(refusal_of(libc::EINVAL, p), Refusal::Io { path, errno: libc::EINVAL });
    assert!(refusal_of(libc::EBUSY, p).to_string().contains("busy"));
}

// --- VUGCOMSCANTTYUSBID (LEDGER SR529): the device by VID:PID[:serial] over a fake sysfs tree (named tail hunk) ---
//
// The tree copies the kernel's layout: `devices/usb1/<dev>/{idVendor,idProduct,serial}` with its interface
// `<dev>:1.0`; `class/tty/ttyACM<n>/device` -> the interface (cdc_acm: the ids on its PARENT), `class/tty/ttyUSB<n>/
// device` -> `<dev>:1.0/ttyUSB<n>` (usb-serial: the ids on its GRANDPARENT), and a virtual tty with no `device`.

/// A fresh empty dir under the cargo target's tmp, named for the test.
fn fresh(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A USB device `usbdev` under the fake `sys` with its ids; the path of its interface `<usbdev>:1.0`.
fn usb_device(sys: &Path, usbdev: &str, vid: &str, pid: &str, serial: Option<&str>) -> PathBuf {
    let d = sys.join("devices/usb1").join(usbdev);
    let interface = d.join(format!("{usbdev}:1.0"));
    std::fs::create_dir_all(&interface).unwrap();
    std::fs::write(d.join("idVendor"), format!("{vid}\n")).unwrap();
    std::fs::write(d.join("idProduct"), format!("{pid}\n")).unwrap();
    if let Some(s) = serial {
        std::fs::write(d.join("serial"), format!("{s}\n")).unwrap();
    }
    interface
}

/// The tty `name` in the fake `sys/class/tty`, its `device` link pointing at `target` (none: a virtual tty).
fn tty_node(sys: &Path, name: &str, target: Option<&PathBuf>) {
    let n = sys.join("class/tty").join(name);
    std::fs::create_dir_all(&n).unwrap();
    if let Some(t) = target {
        std::fs::create_dir_all(t).unwrap();
        std::os::unix::fs::symlink(t, n.join("device")).unwrap();
    }
}

#[test]
fn two_boards_on_one_vid_pid_resolve_by_serial() {
    let root = fresh("comscan-usbid-serial");
    let (sys, dev) = (root.join("sys"), root.join("dev"));
    let a = usb_device(&sys, "1-1", "2341", "0043", Some("A1"));
    let b = usb_device(&sys, "1-2", "2341", "0043", Some("B2"));
    let f = usb_device(&sys, "1-3", "0403", "6001", Some("FT9X"));
    tty_node(&sys, "ttyACM0", Some(&a));
    tty_node(&sys, "ttyACM1", Some(&b));
    tty_node(&sys, "ttyUSB0", Some(&f.join("ttyUSB0")));
    tty_node(&sys, "tty0", None);

    let id = |vid, pid, s: &str| UsbId { vid, pid, serial: Some(s.to_string()) };
    assert_eq!(usb_id(&sys, "ttyACM0"), Some(id(0x2341, 0x0043, "A1")));
    assert_eq!(usb_id(&sys, "ttyACM1"), Some(id(0x2341, 0x0043, "B2")));
    assert_eq!(usb_id(&sys, "ttyUSB0"), Some(id(0x0403, 0x6001, "FT9X")), "the grandparent walk");
    assert_eq!(usb_id(&sys, "tty0"), None, "a virtual tty has no USB identity");
    assert_eq!(usb_id(&sys, "ttyACM7"), None);

    assert_eq!(enumerate(&sys, &dev, &id(0x2341, 0x0043, "B2")), Ok(dev.join("ttyACM1")));
    assert_eq!(enumerate(&sys, &dev, &id(0x2341, 0x0043, "A1")), Ok(dev.join("ttyACM0")));
    assert_eq!(enumerate(&sys, &dev, &id(0x0403, 0x6001, "FT9X")), Ok(dev.join("ttyUSB0")));
    let gone = id(0x2341, 0x0043, "C3");
    assert_eq!(enumerate(&sys, &dev, &gone), Err(Refusal::NoSuchPort { path: PathBuf::from(gone.scheme()) }));

    // The scheme string round-trips (the serial keeps its colons), and a malformed one is refused by name.
    let s = id(0x2341, 0x0043, "B2").scheme();
    assert_eq!(s, format!("{USB_SCHEME}2341:0043:B2"));
    assert_eq!(UsbId::parse(&s), Ok(id(0x2341, 0x0043, "B2")));
    assert_eq!(UsbId::parse("tty:usb:1a86:7523"), Ok(UsbId { vid: 0x1a86, pid: 0x7523, serial: None }));
    assert_eq!(UsbId::parse("tty:usb:1a86:7523:a:b"), Ok(id(0x1a86, 0x7523, "a:b")));
    for bad in ["tty:usb:", "tty:usb:2341", "tty:usb:zz:0043", "tty:usb:12345:0043", "/dev/ttyACM0"] {
        assert_eq!(UsbId::parse(bad), Err(Refusal::BadUsbId { text: bad.to_string() }), "{bad}");
    }
    let mut open = opener_in(sys.clone(), dev.clone(), Baud::B115200, Flow::None);
    match open("tty:usb:2341:0043:C3", GRBL) {
        Err(why) => assert_eq!(why, "no such port: tty:usb:2341:0043:C3"),
        Ok(_) => panic!("opened an absent device"),
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_serial_less_vid_pid_on_two_boards_is_refused_ambiguous_by_name() {
    let root = fresh("comscan-usbid-ambiguous");
    let (sys, dev) = (root.join("sys"), root.join("dev"));
    let a = usb_device(&sys, "1-1", "1a86", "7523", None);
    tty_node(&sys, "ttyUSB0", Some(&a.join("ttyUSB0")));
    let want = UsbId { vid: 0x1a86, pid: 0x7523, serial: None };
    assert_eq!(usb_id(&sys, "ttyUSB0"), Some(want.clone()));
    assert_eq!(enumerate(&sys, &dev, &want), Ok(dev.join("ttyUSB0")), "exactly one: taken");

    let b = usb_device(&sys, "1-2", "1a86", "7523", None);
    tty_node(&sys, "ttyUSB1", Some(&b.join("ttyUSB1")));
    let r = enumerate(&sys, &dev, &want);
    assert_eq!(r, Err(Refusal::AmbiguousDevice { vid: 0x1a86, pid: 0x7523, n: 2 }));
    assert_eq!(r.unwrap_err().to_string(), "ambiguous device 1a86:7523: 2 ttys match (name the serial)");
    std::fs::remove_dir_all(&root).unwrap();
}

fn program() -> Program {
    let mut p = Program::new(Units::Mm);
    p.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 5.0) });
    for i in 0..20 {
        p.moves.push(Move::Linear { to: Vec3::new(i as f64, (i * 2) as f64, -1.0), feed: 300.0 });
    }
    p
}

fn axis(id: AxisId, min: f64, max: f64) -> Axis {
    Axis { id, min, max, max_feed: 3000.0, max_accel: 100.0, rapid: 6000.0 }
}

/// The desk's declared machine (the tests/tcp.rs one): a small mill.
fn machine() -> Machine {
    Machine {
        kind: MachineKind::Mill3,
        kinematics: Kinematics::Cartesian,
        axes: vec![axis(AxisId::X, -50.0, 300.0), axis(AxisId::Y, -50.0, 300.0), axis(AxisId::Z, -50.0, 60.0)],
        power: Power { kind: PowerKind::Spindle, max: 24000.0, ask_above: 18000.0 },
        envelope: Aabb3 { min: Vec3::new(-20.0, -20.0, -20.0), max: Vec3::new(300.0, 300.0, 50.0) },
        safe_z: 5.0,
        units: Units::Mm,
        tools: vec![Tool { number: 1, kind: ToolKind::FlatEnd, dc: 4.0, lf: 20.0, apmx: 2.0, nof: 3, sig: 0.0, ramp_deg: None, centre_cutting: None }],
        stock: None,
        fixtures: Vec::new(),
        loaded: Some(1),
    }
}

/// Tick the desk and run its redials (as `serve_pump` does) until a Squawk word satisfies `want`.
fn drive_until(
    desk: &mut PumpDesk,
    open: &mut Opener,
    t0: Instant,
    words: &mut Vec<String>,
    want: impl Fn(&str) -> bool,
) -> String {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        let now = t0.elapsed().as_micros() as u64;
        let mut out = desk.tick(now);
        if let Some(port) = desk.redial_due(now) {
            let res = open(&port, GRBL);
            out.extend(desk.relinked(t0.elapsed().as_micros() as u64, res));
        }
        for s in out {
            if let PumpEvent::Squawk { event: Some(e), .. } = &s {
                words.push(e.clone());
                if want(e) {
                    return e.clone();
                }
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("no wanted squawk in 10 s; saw {words:?}");
}

#[test]
fn the_redial_after_a_renamed_node_relinks_the_same_device() {
    let root = fresh("comscan-usbid-rename");
    let (sys, dev) = (root.join("sys"), root.join("dev"));
    std::fs::create_dir_all(&dev).unwrap();
    let interface = usb_device(&sys, "1-1", "2341", "0043", Some("UNA0001"));
    let (m1, slave1, s1) = pty();
    std::os::unix::fs::symlink(&slave1, dev.join("ttyACM0")).unwrap();
    tty_node(&sys, "ttyACM0", Some(&interface));

    let p = program();
    let mut desk = PumpDesk::new(machine());
    desk.identify_by(identify_in(sys.clone()));
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = opener_in(sys.clone(), dev.clone(), Baud::B115200, Flow::None);
    // Both fakes keep silent until the soft reset: open_flow's tcflush drops a banner said before the open, and the
    // desk opens and waits in one call, so only a banner on the reset is said after the port is open.
    let fake1 = fake_grbl(m1, true, 2);
    let start = PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: dev.join("ttyACM0").to_str().unwrap().to_string(),
    };
    let out = desk.apply(&start, &mut *open);
    assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }] if e == "started"), "{out:?}");

    // Stream until the first fake has its two lines, then let the desk read their oks.
    let t0 = Instant::now();
    while !fake1.is_finished() && t0.elapsed() < Duration::from_secs(5) {
        desk.tick(t0.elapsed().as_micros() as u64);
        thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..20 {
        desk.tick(t0.elapsed().as_micros() as u64);
        thread::sleep(Duration::from_millis(2));
    }
    let (_lines, m1) = fake1.join().unwrap();

    // The replug: the same board comes back as ttyACM1 (a second pty), ttyACM0 is gone from /dev and from sysfs.
    let (m2, slave2, _s2) = pty();
    std::os::unix::fs::symlink(&slave2, dev.join("ttyACM1")).unwrap();
    std::fs::remove_file(dev.join("ttyACM0")).unwrap();
    std::fs::remove_dir_all(sys.join("class/tty/ttyACM0")).unwrap();
    tty_node(&sys, "ttyACM1", Some(&interface));
    let fake2 = fake_grbl(m2, true, 0);
    drop(m1);
    drop(s1);

    let mut words = Vec::new();
    let lost = drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("link lost"));
    let at: usize = lost
        .strip_prefix("link lost at line ")
        .and_then(|r| r.split(';').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{lost}"));
    assert!(lost.ends_with("reconnect attempt 1 in 250 ms"), "{lost}");
    assert!(matches!(desk.state(), Some(PumpState::LinkLost { at_line, .. }) if at_line == at));
    let due = desk.redial_due(u64::MAX);
    assert_eq!(due, Some("tty:usb:2341:0043:UNA0001".to_string()), "by identity, not the old name");

    let relinked =
        drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("relinked") || e.contains("failed"));
    assert_eq!(relinked, format!("relinked on attempt 1; re-arm to resume from line {at}"), "{words:?}");
    assert_eq!(desk.rearm(&pass, &p, GRBL), Ok(at), "the renamed node carries the re-armed run");
    drop(desk);
    drop(open);
    let _ = fake2.join().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

// --- VUGCOMSCANHELLOON (SR549): the tty fake answers `$I`; the first open keeps the controller's identity ---

/// The fakes' default build: grbl 1.1h with the stored user string `mill-a` (the tests/tcp.rs one).
const VER_A: &str = "1.1h.20190825:mill-a";

/// grbl 1.1's `$I` answer for build `ver`, written when `line` is `$I`; true when it was.
fn said_hello(s: &mut impl Write, line: &str, ver: &str) -> bool {
    if line == "$$" {
        let _ = s.write_all(ENVELOPE_ANSWER); // VUGCOMSCANALARMIDLE (SR591); VUGCOMSCANENVELOPEREADON (SR597): heard wherever the hello is on
        return true;
    }
    if line != "$I" {
        return false;
    }
    let _ = s.write_all(format!("[VER:{ver}]\r\n[OPT:V,15,128]\r\nok\r\n").as_bytes());
    true
}

fn hello_of(ver: &str) -> String {
    format!("[VER:{ver}] [OPT:V,15,128]")
}

#[test]
fn the_first_tty_open_keeps_the_controllers_answer_to_the_hello() {
    let (m, path, _s) = pty();
    let p = program();
    let want = Pump::from_program(&p, GRBL).unwrap().lines().to_vec();
    let mut desk = PumpDesk::new(machine());
    desk.hello_within(comscan::tty::READY_WAIT);
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = opener(Baud::B115200, Flow::None);
    // Silent until the soft reset: open_flow's tcflush drops a banner said before the open.
    let fake = fake_grbl_as(m, true, want.len(), VER_A);
    let start = PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: path.to_str().unwrap().to_string(),
    };
    let out = desk.apply(&start, &mut *open);
    let words: Vec<String> =
        out.iter().filter_map(|s| match s { PumpEvent::Squawk { event: Some(e), .. } => Some(e.clone()), _ => None }).collect();
    // VUGCOMSCANENVELOPEREADON (SR597): the travel read rides the hello; the fake's `$$` says X 200 against the declared 350.
    assert_eq!(words, vec!["started".to_string(), format!("hello {}", hello_of(VER_A)), "envelope differs axis=X declared=350 controller=200".to_string()], "{out:?}");
    assert_eq!(desk.hello(), Some(hello_of(VER_A).as_str()), "the first open keeps the answer");

    let t0 = Instant::now();
    let mut done = false;
    while !done && t0.elapsed() < Duration::from_secs(10) {
        for s in desk.tick(t0.elapsed().as_micros() as u64) {
            if let PumpEvent::Squawk { event: Some(e), .. } = &s {
                assert!(!e.starts_with("link lost"), "{e}");
                done |= e == "done";
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(done, "the run reached done");
    assert_eq!(desk.hello(), Some(hello_of(VER_A).as_str()), "kept to the end of the run");
    drop(desk);
    let (lines, _m) = fake.join().unwrap();
    assert_eq!(lines, want, "the hello is no program line");
}

// --- VUGCOMSCANHOSTDIALECT (SR566): a Marlin PumpStart on the host composition, the opener handed the dialect ---

const MARLIN: Dialect = Dialect::Marlin;
const MARLIN_FIRMWARE: &str = "FIRMWARE_NAME:Marlin 2.1.2.1 (Jun  1 2023 12:00:00) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:3D Printer EXTRUDER_COUNT:1";

/// A fake Marlin on the master: says `start` every 20 ms until its first line arrives (a booting board; a grbl wait
/// never matches it), answers `M115` with its firmware line, a `Cap:` line and `ok`, then `ok` per line; returns the
/// lines it executed and the master (kept open until the desk is done) once `lines` arrived (`M115` is neither recorded
/// nor counted).
fn fake_marlin(mut m: File, lines: usize) -> JoinHandle<(Vec<String>, File)> {
    thread::spawn(move || {
        let heard = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (mut w, h) = (m.try_clone().unwrap(), heard.clone());
        let mut spam = Some(thread::spawn(move || {
            while !h.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = w.write_all(b"start\r\n");
                thread::sleep(Duration::from_millis(20));
            }
        }));
        let mut got = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        let mut buf = [0u8; 256];
        while got.len() < lines {
            let n = match m.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                if b != b'\n' {
                    cur.push(b);
                    continue;
                }
                let line = String::from_utf8_lossy(&cur).trim().to_string();
                cur.clear();
                if let Some(t) = spam.take() {
                    heard.store(true, std::sync::atomic::Ordering::SeqCst);
                    t.join().unwrap(); // no `start` after this: a mid-stream `start` is a reset to the pump
                }
                if line == "M115" {
                    m.write_all(format!("{MARLIN_FIRMWARE}\r\nCap:SERIAL_XON_XOFF:0\r\nok\r\n").as_bytes()).unwrap();
                } else if line == "M211" {
                    m.write_all(MARLIN_SOFT_ENDSTOPS).unwrap(); // VUGCOMSCANENVELOPEREADON (SR597): neither recorded nor counted
                } else {
                    got.push(line);
                    m.write_all(b"ok\r\n").unwrap();
                }
            }
        }
        heard.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = spam.take() {
            let _ = t.join();
        }
        (got, m)
    })
}

#[test]
fn a_marlin_start_on_the_host_composition_says_m115_and_streams() {
    use pump_core::pump::{envelope_setting, envelope_verb}; // VUGCOMSCANENVELOPEREADON (SR597): Marlin's travel is M211's one soft endstops line
    assert_eq!(envelope_verb(MARLIN), Some("M211"));
    let line = "echo:  Min:  X0.00 Y0.00 Z0.00   Max:  X200.00 Y350.00 Z110.00";
    assert_eq!(envelope_setting(MARLIN, line), [Some(200.0), Some(350.0), Some(110.0)]);
    assert_eq!(envelope_setting(MARLIN, "echo:  Min:  X0.00 Y0.00   Max:  X200.00 Y350.00"), [Some(200.0), Some(350.0), None], "a 2-axis line: Z not compared");
    let (m, path, _s) = pty();
    let p = program();
    let want = Pump::from_program(&p, MARLIN).unwrap().lines().to_vec();
    // THE host composition (tty at 115200, no flow, the hello on): no dialect bound, each open is handed its PumpStart's.
    let (mut desk, mut open) = comscan::pump::host_parts(machine());
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, MARLIN).expect("armed by a pass");
    let fake = fake_marlin(m, want.len());
    let start = PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "marlin".to_string(),
        port: path.to_str().unwrap().to_string(),
    };
    let out = desk.apply(&start, &mut *open);
    let words: Vec<String> =
        out.iter().filter_map(|s| match s { PumpEvent::Squawk { event: Some(e), .. } => Some(e.clone()), _ => None }).collect();
    // A grbl-bound opener waited for `Grbl `, reset and refused NoBanner here; no commission (Marlin has no name verb).
    // VUGCOMSCANENVELOPEREADON (SR597): M211's soft endstops say X 0..200 against the declared 350; Y and Z agree.
    assert_eq!(words, vec!["started".to_string(), format!("hello {MARLIN_FIRMWARE}"), "envelope differs axis=X declared=350 controller=200".to_string()], "{out:?}");
    assert_eq!(desk.hello(), Some(MARLIN_FIRMWARE), "the M115 answer is the identity the reopen compares");

    let t0 = Instant::now();
    let mut done = false;
    while !done && t0.elapsed() < Duration::from_secs(10) {
        for s in desk.tick(t0.elapsed().as_micros() as u64) {
            if let PumpEvent::Squawk { event: Some(e), .. } = &s {
                assert!(!e.starts_with("link lost") && e != "controller reset" && !e.starts_with("envelope"), "{e}");
                done |= e == "done";
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(done, "the Marlin run reached done");
    drop(desk);
    drop(open);
    let (lines, _m) = fake.join().unwrap();
    assert_eq!(lines, want, "make_core's Marlin text, line for line, and M115 is no program line");
}

// --- VUGCOMSCANENVELOPEREADON (SR597): Marlin's travel read through M211, the one soft endstops line ---

/// Marlin 2.1's `M211` report (M211_report: the heading, the S state, `print_pos(min)` and `print_pos(max)` on one line,
/// then `ok`): soft endstops X 0..200 (the declared machine says 350), Y 0..350, Z 0..110.
const MARLIN_SOFT_ENDSTOPS: &[u8] =
    b"echo:; Soft Endstops:\r\necho:  M211 S1 ; ON\r\necho:  Min:  X0.00 Y0.00 Z0.00   Max:  X200.00 Y350.00 Z110.00\r\nok\r\n";

// --- VUGCOMSCANJOG (SR573): a lost grbl pump homes and jogs on the relinked port, before any re-arm ---

/// The one `PumpRefused` reason in `out`.
fn refused_in(out: &[PumpEvent]) -> String {
    match out {
        [PumpEvent::PumpRefused { why }] => why.clone(),
        other => panic!("expected one PumpRefused, got {other:?}"),
    }
}

/// The one Squawk word in `out`.
fn said_in(out: &[PumpEvent]) -> String {
    match out {
        [PumpEvent::Squawk { event: Some(e), .. }] => e.clone(),
        other => panic!("expected one Squawk word, got {other:?}"),
    }
}

#[test]
fn a_lost_grbl_homes_and_jogs_on_the_relinked_port() {
    let root = fresh("comscan-jog-lost");
    let (sys, dev) = (root.join("sys"), root.join("dev"));
    std::fs::create_dir_all(&dev).unwrap();
    let interface = usb_device(&sys, "1-1", "2341", "0043", Some("UNA0002"));
    let (m1, slave1, s1) = pty();
    std::os::unix::fs::symlink(&slave1, dev.join("ttyACM0")).unwrap();
    tty_node(&sys, "ttyACM0", Some(&interface));

    let p = program();
    let mut desk = PumpDesk::new(machine());
    desk.identify_by(identify_in(sys.clone()));
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = opener_in(sys.clone(), dev.clone(), Baud::B115200, Flow::None);
    // Silent until the soft reset (open_flow's tcflush drops a banner said before the open), as the renamed-node test.
    let fake1 = fake_grbl(m1, true, 2);
    let who = || Origin::LocalUser("una".to_string());
    let home = PumpEvent::PumpHome { principal: who() };
    let start = PumpEvent::PumpStart {
        principal: who(),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: dev.join("ttyACM0").to_str().unwrap().to_string(),
    };
    assert_eq!(said_in(&desk.apply(&start, &mut *open)), "started");
    let busy = refused_in(&desk.apply(&home, &mut *open));
    assert!(busy.starts_with("busy: streaming"), "busy on a job refuses: {busy}");

    // Two lines reach the first fake, then the board goes: the replug brings it back as ttyACM1.
    let t0 = Instant::now();
    while !fake1.is_finished() && t0.elapsed() < Duration::from_secs(5) {
        desk.tick(t0.elapsed().as_micros() as u64);
        thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..20 {
        desk.tick(t0.elapsed().as_micros() as u64);
        thread::sleep(Duration::from_millis(2));
    }
    let (_lines, m1) = fake1.join().unwrap();
    let (m2, slave2, _s2) = pty();
    std::os::unix::fs::symlink(&slave2, dev.join("ttyACM1")).unwrap();
    std::fs::remove_file(dev.join("ttyACM0")).unwrap();
    std::fs::remove_dir_all(sys.join("class/tty/ttyACM0")).unwrap();
    tty_node(&sys, "ttyACM1", Some(&interface));
    let fake2 = fake_grbl(m2, true, 2);
    drop(m1);
    drop(s1);

    let mut words = Vec::new();
    let lost = drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("link lost"));
    let at: usize = lost
        .strip_prefix("link lost at line ")
        .and_then(|r| r.split(';').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{lost}"));
    assert_eq!(refused_in(&desk.apply(&home, &mut *open)), "home: the link is down (reconnect attempt 1)");
    let relinked =
        drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("relinked") || e.contains("failed"));
    assert_eq!(relinked, format!("relinked on attempt 1; re-arm to resume from line {at}"), "{words:?}");

    // Lost and relinked, no start, no re-arm: the home and the jog go on the wire and their oks come back by name.
    assert_eq!(said_in(&desk.apply(&home, &mut *open)), "home sent: $H");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("home")), "home ok");
    let jog = PumpEvent::PumpJog { principal: who(), dx: 1.0, dy: -2.0, dz: 0.5, feed: 600.0 };
    assert!(jog.is_request());
    assert_eq!(said_in(&desk.apply(&jog, &mut *open)), "jog sent: $J=G91 X1 Y-2 Z0.5 F600");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("jog")), "jog ok");
    assert!(matches!(desk.state(), Some(PumpState::LinkLost { at_line, .. }) if at_line == at), "the lost run is still lost");
    assert_eq!(desk.rearm(&pass, &p, GRBL), Ok(at), "the home and the jog leave the lost run re-armable");
    drop(desk);
    drop(open);
    let (wire, _m2) = fake2.join().unwrap();
    assert_eq!(wire, vec!["$H".to_string(), "$J=G91 X1 Y-2 Z0.5 F600".to_string()], "the home and the jog, nothing else");
    std::fs::remove_dir_all(&root).unwrap();
}

// --- VUGCOMSCANJOGFACE (SR579): an alarm mid-stream leaves no window, so `$H` homes; a Hold during a jog is 0x85 ---

/// A fake grbl for the alarm and the jog cancel: silent until the soft reset, then its banner; answers `$I`, `ok` to
/// `$H` and `$J=`; the stream line `alarm_at` (0-based) is answered `ALARM:5` (probe fail, a non-critical alarm: grbl
/// hears `$H` after it) and every later G-code line `error:9` (grbl's G-code lock: the lines still on the wire when the
/// alarm flushed its buffer). Records every line and every realtime byte (`?` `!` `~` 0x85); returns once `$H` was
/// answered or 0x85 arrived.
fn fake_grbl_alarm(mut m: File, alarm_at: usize) -> JoinHandle<(Vec<String>, Vec<u8>, File)> {
    thread::spawn(move || {
        let (mut got, mut rt, mut cur) = (Vec::new(), Vec::new(), Vec::<u8>::new());
        let (mut stream, mut alarmed, mut over) = (0usize, false, false);
        let mut buf = [0u8; 256];
        while !over {
            let n = match m.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                match b {
                    0x18 => {
                        m.write_all(b"\r\n").unwrap();
                        m.write_all(BANNER).unwrap();
                    }
                    b'?' | b'!' | b'~' | 0x85 => {
                        rt.push(b);
                        over |= b == 0x85;
                    }
                    b'\n' => {
                        let line = String::from_utf8_lossy(&cur).trim().to_string();
                        cur.clear();
                        if said_hello(&mut m, &line, VER_A) {
                            continue;
                        }
                        got.push(line.clone());
                        if line == "$H" || line.starts_with("$J=") {
                            m.write_all(b"ok\r\n").unwrap();
                            over |= line == "$H";
                        } else if alarmed {
                            m.write_all(b"error:9\r\n").unwrap();
                        } else if stream == alarm_at {
                            alarmed = true;
                            m.write_all(b"ALARM:5\r\n").unwrap();
                        } else {
                            stream += 1;
                            m.write_all(b"ok\r\n").unwrap();
                        }
                    }
                    _ => cur.push(b),
                }
            }
        }
        (got, rt, m)
    })
}

/// A desk started on `path` (the tty opener, hello off) for [`program`].
fn started_desk(path: &Path) -> (PumpDesk, Box<Opener>) {
    let p = program();
    let mut desk = PumpDesk::new(machine());
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = opener(Baud::B115200, Flow::None);
    let start = PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: path.to_str().unwrap().to_string(),
    };
    assert_eq!(said_in(&desk.apply(&start, &mut *open)), "started");
    (desk, open)
}

#[test]
fn an_alarm_mid_stream_then_home_homes() {
    let (m, path, _s) = pty();
    let fake = fake_grbl_alarm(m, 2);
    let (mut desk, mut open) = started_desk(&path);
    let t0 = Instant::now();
    let mut words = Vec::new();
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("alarm")), "alarm:5 Probe fail");
    assert!(matches!(desk.state(), Some(PumpState::Halted(_))), "{:?}", desk.state());

    // The alarm flushed grbl's buffer: the window is gone, so the home is not refused busy. Its answer is claimed past
    // the stale error:9 answers (never a home's), and no feed hold followed them (the stream was no longer live).
    let home = PumpEvent::PumpHome { principal: Origin::LocalUser("una".to_string()) };
    assert_eq!(said_in(&desk.apply(&home, &mut *open)), "home sent: $H");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("home")), "home ok", "{words:?}");
    assert!(!words.iter().any(|w| w.starts_with("home error")), "a stale error:9 claimed as the home's: {words:?}");
    drop(desk);
    drop(open);
    let (lines, rt, _m) = fake.join().unwrap();
    assert_eq!(lines.last().map(String::as_str), Some("$H"), "{lines:?}");
    assert!(!rt.contains(&b'!'), "no feed hold after the alarm: {rt:?}");
}

#[test]
fn a_hold_during_a_jog_writes_jog_cancel() {
    let (m, path, _s) = pty();
    let fake = fake_grbl_alarm(m, usize::MAX);
    let (mut desk, mut open) = started_desk(&path);
    let t0 = Instant::now();
    let mut words = Vec::new();
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "done"), "done");

    let who = || Origin::LocalUser("una".to_string());
    let jog = PumpEvent::PumpJog { principal: who(), dx: 2.0, dy: 0.0, dz: 0.0, feed: 300.0 };
    assert!(said_in(&desk.apply(&jog, &mut *open)).starts_with("jog sent: $J=G91 X2"));
    // grbl says `ok` when the jog is PLANNED: the axis still moves, and the Hold is its cancel.
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("jog")), "jog ok");
    assert_eq!(said_in(&desk.apply(&PumpEvent::PumpHold { principal: who() }, &mut *open)), "jog cancelled");
    drop(desk);
    drop(open);
    let (lines, rt, _m) = fake.join().unwrap();
    assert!(lines.last().is_some_and(|l| l.starts_with("$J=G91 X2")), "{lines:?}");
    assert_eq!(rt.last(), Some(&0x85u8), "the jog cancel byte on the wire: {rt:?}");
    assert!(!rt.contains(&b'!'), "a jog cancel is not a feed hold: {rt:?}");
}

// --- VUGCOMSCANALARMRESET (SR585): Stop on a critical alarm is the soft reset at once, then `$X`; a jog past the declared travel is refused before the wire ---

/// A fake grbl with a CRITICAL alarm: silent until the open's soft reset, then its banner; answers `$I`; the stream line
/// `alarm_at` (0-based) is answered `ALARM:1` (hard limit), after which it answers no line at all and every `?` with
/// `<Alarm|..>` (grbl hears only a soft reset); the soft reset after the alarm says the banner and
/// `[MSG:'$H'|'$X' to unlock]`, and `$X` then says `[MSG:Caution: Unlocked]` and `ok` (Idle after). Returns the lines it
/// took, every realtime byte (`?` `!` `~` 0x18 0x85) and whether it ended unlocked, once `$X` unlocked it.
fn fake_grbl_locked(mut m: File, alarm_at: usize) -> JoinHandle<(Vec<String>, Vec<u8>, bool, File)> {
    thread::spawn(move || {
        let (mut got, mut rt, mut cur) = (Vec::new(), Vec::new(), Vec::<u8>::new());
        let (mut stream, mut alarmed, mut reset_after, mut unlocked) = (0usize, false, false, false);
        let mut buf = [0u8; 256];
        while !unlocked {
            let n = match m.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                match b {
                    0x18 => {
                        rt.push(b);
                        cur.clear();
                        m.write_all(b"\r\n").unwrap();
                        m.write_all(BANNER).unwrap();
                        if alarmed {
                            reset_after = true;
                            m.write_all(b"[MSG:'$H'|'$X' to unlock]\r\n").unwrap();
                        }
                    }
                    b'?' => {
                        rt.push(b);
                        let st = if alarmed { "Alarm" } else { "Idle" };
                        m.write_all(format!("<{st}|MPos:0.000,0.000,0.000|FS:0,0>\r\n").as_bytes()).unwrap();
                    }
                    b'!' | b'~' | 0x85 => rt.push(b),
                    b'\n' => {
                        let line = String::from_utf8_lossy(&cur).trim().to_string();
                        cur.clear();
                        if alarmed && !reset_after {
                            continue; // the critical alarm: nothing but a soft reset is heard
                        }
                        if said_hello(&mut m, &line, VER_A) {
                            continue;
                        }
                        got.push(line.clone());
                        if line == "$X" && alarmed {
                            alarmed = false;
                            unlocked = true;
                            m.write_all(b"[MSG:Caution: Unlocked]\r\nok\r\n").unwrap();
                        } else if stream == alarm_at && !alarmed && !reset_after {
                            alarmed = true;
                            m.write_all(b"ALARM:1\r\n").unwrap();
                        } else {
                            stream += 1;
                            m.write_all(b"ok\r\n").unwrap();
                        }
                    }
                    _ => cur.push(b),
                }
            }
        }
        (got, rt, unlocked, m)
    })
}

#[test]
fn a_hard_limit_alarm_then_stop_resets_and_unlocks() {
    let (m, path, _s) = pty();
    let fake = fake_grbl_locked(m, 2);
    let (mut desk, mut open) = started_desk(&path);
    let t0 = Instant::now();
    let mut words = Vec::new();
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("alarm")), "alarm:1 Hard limit");
    assert!(matches!(desk.state(), Some(PumpState::Halted(_))), "{:?}", desk.state());

    // The controller reports Alarm, never Idle / Hold:0: Stop is the soft reset at once, not a feed hold that waits.
    let stop = PumpEvent::PumpStop { principal: Origin::LocalUser("una".to_string()) };
    assert_eq!(said_in(&desk.apply(&stop, &mut *open)), "reset sent");
    assert_eq!(desk.state(), Some(PumpState::Stopped), "stopped, not stopping: nothing waits on a report");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "unlocked"), "unlocked", "{words:?}");
    assert!(words.iter().any(|w| w == "unlock sent: $X"), "{words:?}");
    assert_eq!(desk.state(), Some(PumpState::Stopped));
    drop(desk);
    drop(open);
    let (lines, rt, unlocked, _m) = fake.join().unwrap();
    assert_eq!(lines.last().map(String::as_str), Some("$X"), "{lines:?}");
    assert_eq!(rt.iter().filter(|&&b| b == 0x18).count(), 2, "the open's reset and the Stop's: {rt:?}");
    assert!(!rt.contains(&b'!'), "no feed hold on an alarm halt: {rt:?}");
    assert!(unlocked, "the controller is unlocked (Idle) after the `$X`");
}

#[test]
fn a_jog_past_the_declared_travel_is_refused_with_nothing_on_the_wire() {
    let (m, path, _s) = pty();
    let fake = fake_grbl_alarm(m, usize::MAX);
    let (mut desk, mut open) = started_desk(&path);
    let t0 = Instant::now();
    let mut words = Vec::new();
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "done"), "done");

    // The declared X axis runs -50..300 (travel 350): a 400 mm jog cannot fit wherever the axis stands.
    let who = || Origin::LocalUser("una".to_string());
    let far = PumpEvent::PumpJog { principal: who(), dx: 400.0, dy: 0.0, dz: 0.0, feed: 300.0 };
    assert_eq!(
        refused_in(&desk.apply(&far, &mut *open)),
        "jog: X400 is past the declared X travel 350 (the position is not polled outside a job: the jog's delta is bounded)"
    );
    let home = PumpEvent::PumpHome { principal: who() };
    assert_eq!(said_in(&desk.apply(&home, &mut *open)), "home sent: $H");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("home")), "home ok", "{words:?}");
    drop(desk);
    drop(open);
    let (lines, _rt, _m) = fake.join().unwrap();
    assert_eq!(lines.last().map(String::as_str), Some("$H"), "{lines:?}");
    assert!(!lines.iter().any(|l| l.starts_with("$J=")), "the refused jog never reached the wire: {lines:?}");
}

// --- VUGCOMSCANALARMIDLE (SR591): an alarm with no job live halts the pump, so Stop resets and unlocks; Marlin's halt and M999 S1; the controller's travel read once ---

/// grbl 1.1's `$$` answer, cut to one ordinary setting and the travel: X 200 (the declared machine says 350), Y 350, Z 110.
const ENVELOPE_ANSWER: &[u8] = b"$0=10\r\n$130=200.000\r\n$131=350.000\r\n$132=110.000\r\nok\r\n";

/// Marlin's `stop()` line (STR_ERR_STOPPED).
const MARLIN_STOPPED: &[u8] =
    b"Error:Printer stopped due to errors. Fix the error and use M999 to restart. (Temperature is reset. Set it after restarting)\r\n";

#[test]
fn an_alarm_during_a_jog_then_stop_resets_and_unlocks() {
    let (m, path, _s) = pty();
    let n = Pump::from_program(&program(), GRBL).unwrap().lines().len();
    // The fake's ALARM:1 answers its line `n`: the first line after the program, the jog (a hard limit hit while jogging).
    let fake = fake_grbl_locked(m, n);
    let (mut desk, mut open) = started_desk(&path);
    let t0 = Instant::now();
    let mut words = Vec::new();
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "done"), "done");

    let who = || Origin::LocalUser("una".to_string());
    let jog = PumpEvent::PumpJog { principal: who(), dx: 2.0, dy: 0.0, dz: 0.0, feed: 300.0 };
    assert!(said_in(&desk.apply(&jog, &mut *open)).starts_with("jog sent: $J=G91 X2"));
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("alarm")), "alarm:1 Hard limit");
    assert_eq!(desk.state(), Some(PumpState::Halted(pump_core::Halt::Alarm { code: 1 })), "a Done pump halts on the alarm");

    // Stop is the reset (the alarm is grbl's lock), not the jog cancel the jog alone would take.
    assert_eq!(said_in(&desk.apply(&PumpEvent::PumpStop { principal: who() }, &mut *open)), "reset sent");
    assert_eq!(desk.state(), Some(PumpState::Stopped));
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "unlocked"), "unlocked", "{words:?}");
    assert!(words.iter().any(|w| w == "unlock sent: $X"), "{words:?}");
    drop(desk);
    drop(open);
    let (lines, rt, unlocked, _m) = fake.join().unwrap();
    assert_eq!(lines.last().map(String::as_str), Some("$X"), "{lines:?}");
    assert_eq!(rt.iter().filter(|&&b| b == 0x18).count(), 2, "the open's reset and the Stop's: {rt:?}");
    assert!(!rt.contains(&0x85), "no jog cancel: the alarm already stopped the jog: {rt:?}");
    assert!(!rt.contains(&b'!'), "no feed hold on an alarm halt: {rt:?}");
    assert!(unlocked, "the controller is unlocked after the `$X`");
}

/// A fake Marlin, silent at the open (the opener's Marlin wait passes): `ok` per line until the stream line `halt_at`
/// (0-based), answered [`MARLIN_STOPPED`] and no `ok`; `M999 S1` answered `ok`. Returns every line once `M999 S1` arrived.
fn fake_marlin_halted(mut m: File, halt_at: usize) -> JoinHandle<(Vec<String>, File)> {
    thread::spawn(move || {
        let (mut got, mut cur, mut stream) = (Vec::<String>::new(), Vec::<u8>::new(), 0usize);
        let mut buf = [0u8; 256];
        while got.last().map(String::as_str) != Some("M999 S1") {
            let n = match m.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                if b != b'\n' {
                    cur.push(b);
                    continue;
                }
                let line = String::from_utf8_lossy(&cur).trim().to_string();
                cur.clear();
                got.push(line.clone());
                if line == "M999 S1" {
                    m.write_all(b"ok\r\n").unwrap();
                } else if stream == halt_at {
                    stream = usize::MAX;
                    m.write_all(MARLIN_STOPPED).unwrap();
                } else if stream < halt_at {
                    stream += 1;
                    m.write_all(b"ok\r\n").unwrap();
                }
            }
        }
        (got, m)
    })
}

#[test]
fn a_marlin_halt_then_stop_sends_m999() {
    let (m, path, _s) = pty();
    let p = program();
    let want = Pump::from_program(&p, MARLIN).unwrap().lines().to_vec();
    let mut desk = PumpDesk::new(machine());
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, MARLIN).expect("armed by a pass");
    let mut open = opener(Baud::B115200, Flow::None);
    let fake = fake_marlin_halted(m, 2);
    let who = || Origin::LocalUser("una".to_string());
    let start = PumpEvent::PumpStart {
        principal: who(),
        program: digest_hex(digest),
        dialect: "marlin".to_string(),
        port: path.to_str().unwrap().to_string(),
    };
    assert_eq!(said_in(&desk.apply(&start, &mut *open)), "started");
    let t0 = Instant::now();
    let mut words = Vec::new();
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("alarm")), "alarm:0 printer halted");
    assert_eq!(desk.state(), Some(PumpState::Halted(pump_core::Halt::Alarm { code: 0 })));

    // Marlin has no soft reset: Stop sends the unlock at once.
    assert_eq!(said_in(&desk.apply(&PumpEvent::PumpStop { principal: who() }, &mut *open)), "unlock sent: M999 S1");
    assert_eq!(desk.state(), Some(PumpState::Stopped));
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "unlocked"), "unlocked", "{words:?}");
    drop(desk);
    drop(open);
    let (lines, _m) = fake.join().unwrap();
    let mut expect = want[..3].to_vec();
    expect.push("M999 S1".to_string());
    assert_eq!(lines, expect, "two lines run, the third halted, then the unlock; no reset byte rides any line");
}

#[test]
fn a_differing_envelope_is_said_once_at_the_start() {
    let (m, path, _s) = pty();
    let p = program();
    let want = Pump::from_program(&p, GRBL).unwrap().lines().to_vec();
    let mut desk = PumpDesk::new(machine());
    desk.hello_within(comscan::tty::READY_WAIT); // VUGCOMSCANENVELOPEREADON (SR597): the read is on by default and rides the hello
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = opener(Baud::B115200, Flow::None);
    let fake = fake_grbl_as(m, true, want.len(), VER_A);
    let start = PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: path.to_str().unwrap().to_string(),
    };
    let out = desk.apply(&start, &mut *open);
    let words: Vec<String> =
        out.iter().filter_map(|s| match s { PumpEvent::Squawk { event: Some(e), .. } => Some(e.clone()), _ => None }).collect();
    // The declared X travel is 350 (-50..300), the controller's $130 200; Y and Z agree.
    assert_eq!(words, vec!["started".to_string(), format!("hello {}", hello_of(VER_A)), "envelope differs axis=X declared=350 controller=200".to_string()], "{out:?}");

    let t0 = Instant::now();
    let mut done = false;
    while !done && t0.elapsed() < Duration::from_secs(10) {
        for s in desk.tick(t0.elapsed().as_micros() as u64) {
            if let PumpEvent::Squawk { event: Some(e), .. } = &s {
                assert!(!e.starts_with("envelope") && !e.starts_with("link lost"), "{e}");
                done |= e == "done";
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(done, "the run reached done: the `$$` answer's `ok` was not taken as a program line's");
    drop(desk);
    let (lines, _m) = fake.join().unwrap();
    assert_eq!(lines, want, "the `$$` is no program line");
}

// --- VUGCOMSCANENVELOPEBOUND (SR602): the jog's bound is the tighter of the declared travel and the controller's read one ---

#[test]
fn a_jog_is_bounded_by_the_tighter_of_the_declared_and_the_read_travel() {
    let who = || Origin::LocalUser("una".to_string());
    let x300 = PumpEvent::PumpJog { principal: who(), dx: 300.0, dy: 0.0, dz: 0.0, feed: 300.0 };
    let home = PumpEvent::PumpHome { principal: who() };

    // Hello off: nothing is read, so the declared X travel (350) bounds and a 300 mm jog goes out.
    let (m, path, _s) = pty();
    let fake = fake_grbl_alarm(m, usize::MAX);
    let (mut desk, mut open) = started_desk(&path);
    let t0 = Instant::now();
    let mut words = Vec::new();
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "done"), "done");
    assert_eq!(said_in(&desk.apply(&x300, &mut *open)), "jog sent: $J=G91 X300 Y0 Z0 F300");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("jog")), "jog ok", "{words:?}");
    assert_eq!(said_in(&desk.apply(&home, &mut *open)), "home sent: $H");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("home")), "home ok", "{words:?}");
    drop(desk);
    drop(open);
    let (lines, _rt, _m) = fake.join().unwrap();
    assert_eq!(lines[lines.len() - 2..].to_vec(), vec!["$J=G91 X300 Y0 Z0 F300".to_string(), "$H".to_string()], "{lines:?}");

    // Hello on, on a port the redial reopens by name: the fake's `$130=200` is kept as the X bound.
    let root = fresh("comscan-envelope-bound");
    let port = root.join("ttyACM0");
    let (m1, slave1, s1) = pty();
    std::os::unix::fs::symlink(&slave1, &port).unwrap();
    let p = program();
    let mut desk = PumpDesk::new(machine());
    desk.hello_within(comscan::tty::READY_WAIT);
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = opener(Baud::B115200, Flow::None);
    let fake1 = fake_grbl(m1, true, 2);
    let start = PumpEvent::PumpStart {
        principal: who(),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: port.to_str().unwrap().to_string(),
    };
    let out = desk.apply(&start, &mut *open);
    let said: Vec<String> =
        out.iter().filter_map(|s| match s { PumpEvent::Squawk { event: Some(e), .. } => Some(e.clone()), _ => None }).collect();
    assert_eq!(said, vec!["started".to_string(), format!("hello {}", hello_of(VER_A)), "envelope differs axis=X declared=350 controller=200".to_string()], "{out:?}");

    // Two lines reach the first fake, then the board goes; the second comes back on the same name.
    let t0 = Instant::now();
    while !fake1.is_finished() && t0.elapsed() < Duration::from_secs(5) {
        desk.tick(t0.elapsed().as_micros() as u64);
        thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..20 {
        desk.tick(t0.elapsed().as_micros() as u64);
        thread::sleep(Duration::from_millis(2));
    }
    let (_lines, m1) = fake1.join().unwrap();
    let (m2, slave2, _s2) = pty();
    std::fs::remove_file(&port).unwrap();
    std::os::unix::fs::symlink(&slave2, &port).unwrap();
    let fake2 = fake_grbl_alarm(m2, usize::MAX);
    drop(m1);
    drop(s1);

    let mut words = Vec::new();
    let lost = drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("link lost"));
    assert!(lost.starts_with("link lost at line "), "{lost}");
    // Lost, the read is still kept: past the controller's 200 is refused by name; inside both bounds meets the down link.
    assert_eq!(
        refused_in(&desk.apply(&x300, &mut *open)),
        "jog: X300 is past the controller X travel 200 (the position is not polled outside a job: the jog's delta is bounded)"
    );
    let x150 = PumpEvent::PumpJog { principal: who(), dx: 150.0, dy: 0.0, dz: 0.0, feed: 300.0 };
    assert_eq!(refused_in(&desk.apply(&x150, &mut *open)), "jog: the link is down (reconnect attempt 1)");
    let relinked =
        drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("relinked") || e.contains("failed") || e.contains("refused"));
    assert!(relinked.starts_with("relinked on attempt 1;"), "{words:?}");

    // Relinked: the reopened board was not asked its travel, so the declared bound stands until the next start.
    assert_eq!(said_in(&desk.apply(&x300, &mut *open)), "jog sent: $J=G91 X300 Y0 Z0 F300");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("jog")), "jog ok", "{words:?}");
    assert_eq!(said_in(&desk.apply(&home, &mut *open)), "home sent: $H");
    assert_eq!(drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("home")), "home ok", "{words:?}");
    drop(desk);
    drop(open);
    let (lines, _rt, _m) = fake2.join().unwrap();
    assert_eq!(lines, vec!["$J=G91 X300 Y0 Z0 F300".to_string(), "$H".to_string()], "no `$$` on the relink, the jog and the home only");
}
