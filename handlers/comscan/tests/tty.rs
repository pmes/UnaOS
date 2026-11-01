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

use comscan::tty::{Baud, CANDIDATES, Flow, Port, Refusal, list, list_in, opener, refusal_of};
use make_core::gcode::Dialect;
use pump_core::{Event, LinkError, Pump, Transport};

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
/// (and, with `banner_on_reset`, the reset was seen).
fn fake_grbl(mut m: File, banner_on_reset: bool, lines: usize) -> JoinHandle<(Vec<String>, File)> {
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
                        got.push(String::from_utf8_lossy(&cur).trim().to_string());
                        cur.clear();
                        m.write_all(b"ok\r\n").unwrap();
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
