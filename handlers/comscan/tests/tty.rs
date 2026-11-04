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
    let mut open = opener(Baud::B115200, Flow::None, GRBL);
    match open("/dev/comscan-no-such-port") {
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
    let mut open = opener_in(sys.clone(), dev.clone(), Baud::B115200, Flow::None, GRBL);
    match open("tty:usb:2341:0043:C3") {
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
            let res = open(&port);
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
    let mut open = opener_in(sys.clone(), dev.clone(), Baud::B115200, Flow::None, GRBL);
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
    let mut open = opener(Baud::B115200, Flow::None, GRBL);
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
    assert_eq!(words, vec!["started".to_string(), format!("hello {}", hello_of(VER_A))], "{out:?}");
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
