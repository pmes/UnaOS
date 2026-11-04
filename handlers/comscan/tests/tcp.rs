// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VUGCOMSCANTCP (SR487): a networked grbl (grblHAL / FluidNC raw TCP) as the pump's Transport, exercised against a
// std TcpListener fake grbl on 127.0.0.1:0. No real controller is touched.

use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bandy::{Origin, PumpEvent};
use comscan::pump::{Opener, PumpDesk, TCP_SCHEME, Verdict, by_scheme, digest_hex};
use comscan::tcp::{CONNECT_WAIT, Port, Refusal, opener, parse, refusal_of};
use make_core::gcode::Dialect;
use make_core::{
    Aabb3, Axis, AxisId, Kinematics, Machine, MachineKind, Move, Power, PowerKind, Program, Tool, ToolKind, Units, Vec3,
};
use pump_core::{LinkError, Pump, Transport};

const GRBL: Dialect = Dialect::Grbl { laser: false };
const BANNER: &[u8] = b"Grbl 1.1h ['$' for help]\r\n";

/// A fake grbl behind one accepted connection: says its banner at once (or only after a soft reset when
/// `banner_on_reset`), answers `ok` per line, ignores the realtime bytes; returns the lines it executed once the client
/// closes (or `stop_after` lines, and with `banner_on_reset` the reset, were seen). It answers `$I` as build `VER_A`
/// (VUGCOMSCANHELLOID, SR546).
fn fake_grbl(l: TcpListener, banner_on_reset: bool, stop_after: Option<usize>) -> JoinHandle<Vec<String>> {
    fake_grbl_as(l, banner_on_reset, stop_after, VER_A)
}

/// [`fake_grbl`] answering `$I` as build `ver` (`$I` is neither recorded nor counted as a line).
fn fake_grbl_as(l: TcpListener, banner_on_reset: bool, stop_after: Option<usize>, ver: &'static str) -> JoinHandle<Vec<String>> {
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        if !banner_on_reset {
            s.write_all(BANNER).unwrap();
        }
        let mut got = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        let mut reset_seen = false;
        let mut buf = [0u8; 256];
        loop {
            if stop_after.is_some_and(|n| got.len() >= n) && (!banner_on_reset || reset_seen) {
                break;
            }
            let n = match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                match b {
                    0x18 => {
                        reset_seen = true;
                        let _ = s.write_all(b"\r\n");
                        let _ = s.write_all(BANNER);
                    }
                    b'?' | b'!' | b'~' => {}
                    b'\n' => {
                        let line = String::from_utf8_lossy(&cur).trim().to_string();
                        cur.clear();
                        if !said_hello(&mut s, &line, ver) {
                            got.push(line);
                            let _ = s.write_all(b"ok\r\n");
                        }
                    }
                    _ => cur.push(b),
                }
            }
        }
        got
    })
}

fn listener() -> (TcpListener, u16) {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = l.local_addr().unwrap().port();
    (l, port)
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

/// The desk's declared machine (VUG-HELM-BIND-MACHINE, SR498): a small mill.
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

fn no_serial() -> Box<Opener> {
    Box::new(|port: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> { Err(format!("serial asked for {port}")) })
}

#[test]
fn an_armed_program_streams_to_done_over_tcp_through_the_scheme_arm() {
    let (l, port) = listener();
    let fake = fake_grbl(l, false, None);
    let p = program();
    let mut desk = PumpDesk::new(machine());
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = by_scheme(no_serial(), opener(CONNECT_WAIT));
    let start = PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: format!("{TCP_SCHEME}127.0.0.1:{port}"),
    };
    let out = desk.apply(&start, &mut *open);
    assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }] if e == "started"), "{out:?}");

    let t0 = Instant::now();
    let mut last = None;
    while t0.elapsed() < Duration::from_secs(10) {
        for s in desk.tick(t0.elapsed().as_micros() as u64) {
            last = Some(s);
        }
        if matches!(&last, Some(PumpEvent::Squawk { event: Some(e), .. }) if e == "done" || e == "link lost") {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    match last {
        Some(PumpEvent::Squawk { sent, acked, total, event: Some(e), .. }) => {
            assert_eq!(e, "done");
            assert_eq!((sent, acked), (total, total));
        }
        other => panic!("no done squawk: {other:?}"),
    }
    drop(desk); // the link closes: the fake sees its FIN and returns
    let lines = fake.join().unwrap();
    let want = Pump::from_program(&p, GRBL).unwrap().lines().to_vec();
    assert_eq!(lines, want, "the controller ran make_core's text, line for line, over TCP");
}

#[test]
fn a_silent_grbl_is_soft_reset_once_over_tcp_and_says_its_banner() {
    let (l, port) = listener();
    let fake = fake_grbl(l, true, Some(0));
    let mut link = Port::connect("127.0.0.1", port, CONNECT_WAIT).expect("connect");
    assert_eq!(link.peer(), format!("127.0.0.1:{port}"));
    assert_eq!(link.ready(Dialect::Marlin, Duration::from_millis(50)), Ok(None), "a silent Marlin proceeds, no reset sent");
    let banner = link.ready(GRBL, Duration::from_millis(300)).expect("ready after the reset");
    assert!(banner.as_deref().is_some_and(|b| b.starts_with("Grbl 1.1h")), "banner {banner:?}");
    fake.join().unwrap();
}

#[test]
fn a_closed_peer_reads_closed_and_nothing_yet_is_ok_zero() {
    let (l, port) = listener();
    let mut link = Port::connect("127.0.0.1", port, CONNECT_WAIT).expect("connect");
    let (server, _) = l.accept().unwrap();
    let mut buf = [0u8; 16];
    assert_eq!(Transport::read(&mut link, &mut buf), Ok(0), "nothing yet is Ok(0), not a block");
    drop(server);
    drop(l);
    let t0 = Instant::now();
    let got = loop {
        match Transport::read(&mut link, &mut buf) {
            Ok(0) if t0.elapsed() < Duration::from_secs(5) => thread::sleep(Duration::from_millis(5)),
            r => break r,
        }
    };
    assert_eq!(got, Err(LinkError::Closed));
}

#[test]
fn a_port_nobody_listens_on_refuses_by_name_within_the_timeout() {
    let (l, port) = listener();
    drop(l);
    let wait = Duration::from_secs(2);
    let t0 = Instant::now();
    match Port::connect("127.0.0.1", port, wait) {
        Err(Refusal::Refused { peer }) => assert_eq!(peer, format!("127.0.0.1:{port}")),
        Err(r) => panic!("wrong refusal: {r}"),
        Ok(_) => panic!("connected to nobody"),
    }
    assert!(t0.elapsed() < wait, "refused inside the timeout");

    // The scheme arm: tcp:// goes to the network opener whole, a /dev path to the serial one.
    let mut open = by_scheme(no_serial(), opener(wait));
    match open("/dev/ttyUSB0", GRBL) {
        Err(why) => assert_eq!(why, "serial asked for /dev/ttyUSB0"),
        Ok(_) => panic!("opened a serial port"),
    }
    match open(&format!("tcp://127.0.0.1:{port}"), GRBL) {
        Err(why) => assert!(why.contains("connection refused"), "{why}"),
        Ok(_) => panic!("connected to nobody"),
    }
    match open("tcp://127.0.0.1", GRBL) {
        Err(why) => assert!(why.contains("no :port"), "{why}"),
        Ok(_) => panic!("opened without a port"),
    }
    match Port::connect("comscan-no-such-host.invalid", 23, wait) {
        Err(Refusal::Unresolved { host }) => assert_eq!(host, "comscan-no-such-host.invalid"),
        Err(r) => panic!("wrong refusal: {r}"),
        Ok(_) => panic!("resolved .invalid"),
    }
}

#[test]
fn the_port_grammar_parses_and_refuses_by_name() {
    assert_eq!(parse("tcp://grblhal.local:23"), Ok(("grblhal.local".to_string(), 23)));
    assert_eq!(parse("tcp://192.168.0.1:8080"), Ok(("192.168.0.1".to_string(), 8080)));
    assert_eq!(parse("tcp://[::1]:23"), Ok(("::1".to_string(), 23)));
    for (text, why) in [
        ("/dev/ttyUSB0", "no tcp:// scheme"),
        ("ws://fluidnc.local:81", "no tcp:// scheme"),
        ("tcp://grblhal.local", "no :port"),
        ("tcp://[::1]", "no :port"),
        ("tcp://:23", "empty host"),
        ("tcp://host:0", "port not 1-65535"),
        ("tcp://host:65536", "port not 1-65535"),
        ("tcp://host:+23", "port not 1-65535"),
        ("tcp://host:23/x", "port not 1-65535"),
    ] {
        assert_eq!(parse(text), Err(Refusal::BadAddress { text: text.to_string(), why }), "{text}");
    }
    let p = "10.0.0.9:23";
    assert_eq!(refusal_of(io::ErrorKind::TimedOut, p, 3000), Refusal::TimedOut { peer: p.to_string(), waited_ms: 3000 });
    assert_eq!(refusal_of(io::ErrorKind::ConnectionRefused, p, 3000), Refusal::Refused { peer: p.to_string() });
    assert_eq!(refusal_of(io::ErrorKind::ConnectionReset, p, 3000), Refusal::Closed { peer: p.to_string() });
    assert_eq!(
        refusal_of(io::ErrorKind::PermissionDenied, p, 3000),
        Refusal::Io { peer: p.to_string(), kind: io::ErrorKind::PermissionDenied }
    );
    assert!(refusal_of(io::ErrorKind::TimedOut, p, 3000).to_string().contains("timed out after 3000 ms"));
}

// --- VUGCOMSCANRECONNECT (SR521): a lost link is LinkLost at its line, redialled with backoff, resumed only after a re-arm ---

use comscan::pump::Refused;
use pump_core::PumpState;

/// A fake grbl that says its banner, acks exactly `n` lines, then FINs (shutdown Write) and reads to the client's
/// end; its listener goes at the accept, so a redial finds nobody. Returns every line it saw. It answers `$I` as build
/// `VER_A` (VUGCOMSCANHELLOID, SR546).
fn fake_grbl_lost_after(l: TcpListener, n: usize) -> JoinHandle<Vec<String>> {
    fake_grbl_lost_after_as(l, n, VER_A)
}

/// [`fake_grbl_lost_after`] answering `$I` as build `ver` (not recorded, not counted toward `n`).
fn fake_grbl_lost_after_as(l: TcpListener, n: usize, ver: &'static str) -> JoinHandle<Vec<String>> {
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        drop(l);
        s.write_all(BANNER).unwrap();
        let mut got = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        let mut acked = 0usize;
        let mut buf = [0u8; 256];
        loop {
            let k = match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(k) => k,
            };
            for &b in &buf[..k] {
                match b {
                    b'?' | b'!' | b'~' | 0x18 => {}
                    b'\n' => {
                        let line = String::from_utf8_lossy(&cur).trim().to_string();
                        cur.clear();
                        if said_hello(&mut s, &line, ver) {
                            continue;
                        }
                        got.push(line);
                        if acked < n {
                            let _ = s.write_all(b"ok\r\n");
                            acked += 1;
                            if acked == n {
                                let _ = s.shutdown(std::net::Shutdown::Write);
                            }
                        }
                    }
                    _ => cur.push(b),
                }
            }
        }
        got
    })
}

fn refused_why(out: &[PumpEvent]) -> String {
    match out {
        [PumpEvent::PumpRefused { why }] => why.clone(),
        other => panic!("expected one PumpRefused, got {other:?}"),
    }
}

/// Tick the desk and run its redials (as `serve_pump` does) until a Squawk word satisfies `want`; every word seen
/// is appended to `words`.
fn drive_until(
    desk: &mut PumpDesk,
    open: &mut Opener,
    t0: Instant,
    words: &mut Vec<String>,
    want: impl Fn(&str) -> bool,
) -> PumpEvent {
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
                    return s;
                }
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("no wanted squawk in 10 s; saw {words:?}");
}

#[test]
fn a_lost_tcp_link_is_link_lost_at_its_line_redials_with_backoff_and_resumes_only_after_a_rearm() {
    let (l, port) = listener();
    let fake1 = fake_grbl_lost_after(l, 5);
    let p = program();
    let want = Pump::from_program(&p, GRBL).unwrap();
    let at = want.line_of(5).unwrap();
    let lines = want.lines().to_vec();
    let mut desk = PumpDesk::new(machine());
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = by_scheme(no_serial(), opener(Duration::from_millis(500)));
    let who = Origin::LocalUser("una".to_string());
    let start = PumpEvent::PumpStart {
        principal: who.clone(),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: format!("{TCP_SCHEME}127.0.0.1:{port}"),
    };
    let out = desk.apply(&start, &mut *open);
    assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }] if e == "started"), "{out:?}");

    // The FIN mid-program: LinkLost at the oldest unacknowledged line, the five acks counted.
    let t0 = Instant::now();
    let mut words = Vec::new();
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("link lost")) {
        PumpEvent::Squawk { acked, state, event: Some(e), .. } => {
            assert_eq!(e, format!("link lost at line {at}; reconnect attempt 1 in 250 ms"));
            assert_eq!((acked, state.as_str()), (5, "link-lost"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(desk.state(), Some(PumpState::LinkLost { at_line: at, program_digest: p.digest() }));
    let seen = fake1.join().unwrap();
    assert_eq!(&seen[..5], &lines[..5]);

    // Lost: Resume is refused by name with its line, a re-arm waits for the link, a new start is busy.
    let resume = PumpEvent::PumpResume { principal: who.clone() };
    let why = refused_why(&desk.apply(&resume, &mut *open));
    assert!(why.contains("ResumeAfterLinkLost") && why.contains(&format!("at_line: {at}")), "{why}");
    assert_eq!(desk.rearm(&pass, &p, GRBL), Err(Refused::LinkDown { attempt: 1 }));
    assert!(refused_why(&desk.apply(&start, &mut *open)).contains("busy"));

    // Attempt 1 (250 ms after the loss) finds nobody; the next waits the doubled 500 ms.
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("reconnect attempt 1 failed")) {
        PumpEvent::Squawk { event: Some(e), .. } => {
            assert!(e.contains("connection refused") && e.ends_with("attempt 2 in 500 ms"), "{e}")
        }
        other => panic!("{other:?}"),
    }
    let l2 = TcpListener::bind(("127.0.0.1", port)).expect("rebind the lost port");
    let fake2 = fake_grbl(l2, false, None);
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("relinked")) {
        PumpEvent::Squawk { event: Some(e), .. } => {
            assert_eq!(e, format!("relinked on attempt 2; re-arm to resume from line {at}"))
        }
        other => panic!("{other:?}"),
    }

    // Relinked (the banner handshake ran) but not re-armed: still refused.
    assert!(refused_why(&desk.apply(&resume, &mut *open)).contains("ResumeAfterLinkLost"));
    let mut q = program();
    q.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 10.0) });
    let pass_q = Verdict::Pass { digest: q.digest(), machine: machine().digest() };
    assert_eq!(desk.rearm(&pass_q, &q, GRBL), Err(Refused::NotTheLostProgram { lost: p.digest(), program: q.digest() }));
    let mut other = machine();
    other.envelope.max = Vec3::new(100.0, 100.0, 50.0);
    let pass_other = Verdict::Pass { digest: p.digest(), machine: other.digest() };
    assert_eq!(
        desk.rearm(&pass_other, &p, GRBL),
        Err(Refused::WrongMachine { passed: other.digest(), desk: machine().digest() }),
        "the machine digest must match before any motion"
    );
    assert_eq!(desk.state(), Some(PumpState::LinkLost { at_line: at, program_digest: p.digest() }));

    // Re-armed by a fresh pass, then Resume streams from the named line to done.
    assert_eq!(desk.rearm(&pass, &p, GRBL), Ok(at));
    assert_eq!(desk.state(), Some(PumpState::Rearmed { at_line: at }));
    let out = desk.apply(&resume, &mut *open);
    assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }] if e == "resume"), "{out:?}");
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "done" || e.starts_with("link lost")) {
        PumpEvent::Squawk { sent, acked, total, event: Some(e), .. } => {
            assert_eq!(e, "done");
            assert_eq!((sent, acked), (total, total));
        }
        other => panic!("{other:?}"),
    }
    drop(desk);
    assert_eq!(fake2.join().unwrap(), lines[5..].to_vec(), "the second link ran only the lines from the lost one on");
}

// --- VUGCOMSCANHELLOID (SR546): the controller's own identity at the first open, a reopen onto another board refused ---

use std::net::TcpStream;

/// The fakes' default build: grbl 1.1h with the stored user string `mill-a` (`$I=mill-a`).
const VER_A: &str = "1.1h.20190825:mill-a";
/// Another board on the same firmware build, its user string `mill-b`.
const VER_B: &str = "1.1h.20190825:mill-b";

/// grbl 1.1's `$I` answer for build `ver`, written when `line` is `$I`; true when it was.
fn said_hello(s: &mut TcpStream, line: &str, ver: &str) -> bool {
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
fn hello_sends_the_dialects_identity_verb_once_and_keeps_its_answer() {
    use pump_core::pump::identity_verb;
    assert_eq!(identity_verb(GRBL), Some("$I"));
    assert_eq!(identity_verb(Dialect::Grbl { laser: true }), Some("$I"));
    assert_eq!(identity_verb(Dialect::Marlin), Some("M115"));
    assert_eq!(identity_verb(Dialect::LinuxCnc), None, "a dialect without an identity verb");

    let (l, port) = listener();
    let fake = fake_grbl(l, false, None);
    let mut link = Port::connect("127.0.0.1", port, CONNECT_WAIT).expect("connect");
    let banner = link.ready(GRBL, Duration::from_millis(500)).expect("banner");
    assert!(banner.as_deref().is_some_and(|b| b.starts_with("Grbl 1.1h")), "banner {banner:?}");
    let said = comscan::pump::hello(&mut link, GRBL, Duration::from_millis(500));
    assert_eq!(said, Some(hello_of(VER_A)));
    assert_eq!(comscan::pump::hello(&mut link, Dialect::LinuxCnc, Duration::from_millis(50)), None, "no verb, nothing sent");
    drop(link);
    assert!(fake.join().unwrap().is_empty(), "the hello is no program line");
}

#[test]
fn a_reopen_onto_another_board_is_refused_by_name_and_keeps_redialing_the_same_board_relinks() {
    let (l, port) = listener();
    let fake1 = fake_grbl_lost_after(l, 5);
    let p = program();
    let want = Pump::from_program(&p, GRBL).unwrap();
    let at = want.line_of(5).unwrap();
    let lines = want.lines().to_vec();
    let mut desk = PumpDesk::new(machine());
    desk.hello_within(Duration::from_millis(500));
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");
    let mut open = by_scheme(no_serial(), opener(Duration::from_millis(500)));
    let who = Origin::LocalUser("una".to_string());
    let start = PumpEvent::PumpStart {
        principal: who.clone(),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: format!("{TCP_SCHEME}127.0.0.1:{port}"),
    };

    // The first open keeps the hello and prints it once, after "started".
    let out = desk.apply(&start, &mut *open);
    let words: Vec<String> =
        out.iter().filter_map(|s| match s { PumpEvent::Squawk { event: Some(e), .. } => Some(e.clone()), _ => None }).collect();
    assert_eq!(words, vec!["started".to_string(), format!("hello {}", hello_of(VER_A))], "{out:?}");
    assert_eq!(desk.hello(), Some(hello_of(VER_A).as_str()));

    // The FIN after five lines: lost at the oldest unacknowledged line.
    let t0 = Instant::now();
    let mut words = Vec::new();
    drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("link lost"));
    assert_eq!(desk.state(), Some(PumpState::LinkLost { at_line: at, program_digest: p.digest() }));
    let seen = fake1.join().unwrap();
    assert_eq!(&seen[..5], &lines[..5], "the hello was no program line on the first link");

    // Another board of the same build takes the port: refused by name, the link dropped, the redial goes on.
    let lb = TcpListener::bind(("127.0.0.1", port)).expect("rebind the lost port");
    let fake_b = fake_grbl_as(lb, false, None, VER_B);
    let refused = drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("reconnect attempt 1"));
    match refused {
        PumpEvent::Squawk { event: Some(e), .. } => {
            let another = Refused::AnotherBoard { was: hello_of(VER_A), now: Some(hello_of(VER_B)) };
            assert_eq!(e, format!("reconnect attempt 1 refused: {another:?}; attempt 2 in 500 ms"))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(desk.state(), Some(PumpState::LinkLost { at_line: at, program_digest: p.digest() }));
    assert_eq!(desk.rearm(&pass, &p, GRBL), Err(Refused::LinkDown { attempt: 2 }), "no re-arm onto another board");
    assert!(fake_b.join().unwrap().is_empty(), "another board ran no program line");

    // The same board comes back: relinked as today, re-armed, resumed to done from the lost line.
    let la = TcpListener::bind(("127.0.0.1", port)).expect("rebind for the same board");
    let fake_a = fake_grbl(la, false, None);
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("relinked") || e.starts_with("reconnect attempt 2")) {
        PumpEvent::Squawk { event: Some(e), .. } => {
            assert_eq!(e, format!("relinked on attempt 2; re-arm to resume from line {at}"))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(desk.rearm(&pass, &p, GRBL), Ok(at));
    let out = desk.apply(&PumpEvent::PumpResume { principal: who }, &mut *open);
    assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }] if e == "resume"), "{out:?}");
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "done" || e.starts_with("link lost")) {
        PumpEvent::Squawk { sent, acked, total, event: Some(e), .. } => {
            assert_eq!(e, "done");
            assert_eq!((sent, acked), (total, total));
        }
        other => panic!("{other:?}"),
    }
    drop(desk);
    assert_eq!(fake_a.join().unwrap(), lines[5..].to_vec(), "the same board ran only the lines from the lost one on");
}

// --- VUGCOMSCANHOSTDESK (SR561): the first bind commissions an unnamed board once; a reopen compares the commissioned name ---

use std::sync::{Arc, Mutex};

use comscan::pump::{Identify, compose};

/// A fake grbl whose `$I` user string is the shared `name`: says its banner, answers `$I` as build
/// `1.1h.20190825:<name>`, takes `$I=<s>` through grbl's line intake (a-z upper-cased, bytes <= ' ' dropped) into
/// `name` with `ok`, acks program lines; with `lost_after` it FINs after that many acks (its listener goes at the
/// accept). Returns (the program lines, the `$I=` writes) at the client's end.
fn fake_grbl_named(l: TcpListener, name: Arc<Mutex<String>>, lost_after: Option<usize>) -> JoinHandle<(Vec<String>, Vec<String>)> {
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        drop(l);
        s.write_all(BANNER).unwrap();
        let mut got = Vec::new();
        let mut writes = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        let mut acked = 0usize;
        let mut buf = [0u8; 256];
        loop {
            let k = match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(k) => k,
            };
            for &b in &buf[..k] {
                match b {
                    b'?' | b'!' | b'~' | 0x18 => {}
                    b'\n' => {
                        let raw = String::from_utf8_lossy(&cur).trim().to_string();
                        cur.clear();
                        let up: String = raw.chars().filter(|c| *c > ' ').map(|c| c.to_ascii_uppercase()).collect();
                        if up == "$I" {
                            let ver = format!("1.1h.20190825:{}", name.lock().unwrap());
                            let _ = s.write_all(format!("[VER:{ver}]\r\n[OPT:V,15,128]\r\nok\r\n").as_bytes());
                        } else if let Some(n) = up.strip_prefix("$I=") {
                            *name.lock().unwrap() = n.to_string();
                            writes.push(up.clone());
                            let _ = s.write_all(b"ok\r\n");
                        } else {
                            got.push(raw);
                            if lost_after.is_none_or(|n| acked < n) {
                                let _ = s.write_all(b"ok\r\n");
                                acked += 1;
                                if lost_after == Some(acked) {
                                    let _ = s.shutdown(std::net::Shutdown::Write);
                                }
                            }
                        }
                    }
                    _ => cur.push(b),
                }
            }
        }
        (got, writes)
    })
}

fn no_identity() -> Box<Identify> {
    Box::new(|_: &str| -> Option<String> { None })
}

fn no_ws() -> Box<Opener> {
    Box::new(|port: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> { Err(format!("ws asked for {port}")) })
}

/// The desk and opener through THE composition (`compose`, what `host_parts` builds over the host's links).
fn composed() -> (PumpDesk, Box<Opener>) {
    compose(machine(), no_identity(), no_serial(), opener(Duration::from_millis(500)), no_ws())
}

fn start_at(digest: u64, port: u16) -> PumpEvent {
    PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: format!("{TCP_SCHEME}127.0.0.1:{port}"),
    }
}

fn squawk_words(out: &[PumpEvent]) -> Vec<String> {
    out.iter().filter_map(|s| match s { PumpEvent::Squawk { event: Some(e), .. } => Some(e.clone()), _ => None }).collect()
}

fn ver_of(name: &str) -> String {
    format!("[VER:1.1h.20190825:{name}] [OPT:V,15,128]")
}

#[test]
fn the_first_bind_commissions_an_unnamed_board_once_and_keeps_the_name() {
    use pump_core::pump::{hello_name, name_verb};
    assert_eq!(name_verb(GRBL, "UNA-1"), Some("$I=UNA-1".to_string()));
    assert_eq!(name_verb(Dialect::Marlin, "UNA-1"), None, "Marlin has no writable name");
    assert_eq!(name_verb(Dialect::LinuxCnc, "UNA-1"), None);
    assert_eq!(hello_name(GRBL, &ver_of("")), Some(""), "an uncommissioned board");
    assert_eq!(hello_name(GRBL, &hello_of(VER_A)), Some("mill-a"));
    assert_eq!(hello_name(GRBL, "[OPT:V,15,128]"), None, "no VER line");

    let (l, port) = listener();
    let name = Arc::new(Mutex::new(String::new()));
    let fake = fake_grbl_named(l, name.clone(), None);
    let p = program();
    let lines = Pump::from_program(&p, GRBL).unwrap().lines().to_vec();
    let (mut desk, mut open) = composed();
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");

    // The first bind: the unnamed answer, the commission, the readback kept and printed once.
    let out = desk.apply(&start_at(digest, port), &mut *open);
    let kept = name.lock().unwrap().clone();
    assert!(kept.starts_with("UNA-") && kept.len() == 12, "the commissioned name {kept:?}");
    assert_eq!(squawk_words(&out), vec!["started".to_string(), format!("hello {}", ver_of(&kept))], "{out:?}");
    assert_eq!(desk.hello(), Some(ver_of(&kept).as_str()), "the readback is the identity the reopen compares");

    let t0 = Instant::now();
    let mut words = Vec::new();
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "done" || e.starts_with("link lost")) {
        PumpEvent::Squawk { event: Some(e), .. } => assert_eq!(e, "done"),
        other => panic!("{other:?}"),
    }
    drop(desk);
    let (got, writes) = fake.join().unwrap();
    assert_eq!(writes, vec![format!("$I={kept}")], "commissioned exactly once");
    assert_eq!(got, lines, "neither the hello nor the commission is a program line");

    // A board already named is never written.
    let (l2, port2) = listener();
    let named = Arc::new(Mutex::new("MILL-A".to_string()));
    let fake2 = fake_grbl_named(l2, named.clone(), None);
    let (mut desk2, mut open2) = composed();
    let digest2 = desk2.arm(&pass, &p, GRBL).expect("armed by a pass");
    let out = desk2.apply(&start_at(digest2, port2), &mut *open2);
    assert_eq!(squawk_words(&out), vec!["started".to_string(), format!("hello {}", ver_of("MILL-A"))], "{out:?}");
    drop(desk2);
    let (_, writes2) = fake2.join().unwrap();
    assert!(writes2.is_empty(), "a named board is not commissioned: {writes2:?}");
    assert_eq!(named.lock().unwrap().as_str(), "MILL-A");
}

#[test]
fn a_reopen_onto_a_board_answering_another_name_refuses_another_board() {
    let (l, port) = listener();
    let name_a = Arc::new(Mutex::new(String::new()));
    let fake1 = fake_grbl_named(l, name_a.clone(), Some(5));
    let p = program();
    let want = Pump::from_program(&p, GRBL).unwrap();
    let at = want.line_of(5).unwrap();
    let lines = want.lines().to_vec();
    let (mut desk, mut open) = composed();
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.arm(&pass, &p, GRBL).expect("armed by a pass");

    let out = desk.apply(&start_at(digest, port), &mut *open);
    let kept = name_a.lock().unwrap().clone();
    let said_a = ver_of(&kept);
    assert_eq!(squawk_words(&out), vec!["started".to_string(), format!("hello {said_a}")], "{out:?}");

    // The FIN after five lines: lost at the oldest unacknowledged line.
    let t0 = Instant::now();
    let mut words = Vec::new();
    drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("link lost"));
    assert_eq!(desk.state(), Some(PumpState::LinkLost { at_line: at, program_digest: p.digest() }));
    let (seen, writes) = fake1.join().unwrap();
    assert_eq!(&seen[..5], &lines[..5]);
    assert_eq!(writes, vec![format!("$I={kept}")]);

    // An unnamed board of the same build takes the port: refused by the commissioned name, and never commissioned.
    let lb = TcpListener::bind(("127.0.0.1", port)).expect("rebind the lost port");
    let name_b = Arc::new(Mutex::new(String::new()));
    let fake_b = fake_grbl_named(lb, name_b.clone(), None);
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("reconnect attempt 1")) {
        PumpEvent::Squawk { event: Some(e), .. } => {
            let another = Refused::AnotherBoard { was: said_a.clone(), now: Some(ver_of("")) };
            assert_eq!(e, format!("reconnect attempt 1 refused: {another:?}; attempt 2 in 500 ms"))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(desk.rearm(&pass, &p, GRBL), Err(Refused::LinkDown { attempt: 2 }), "no re-arm onto another board");
    let (ran_b, writes_b) = fake_b.join().unwrap();
    assert!(ran_b.is_empty() && writes_b.is_empty(), "another board ran nothing and was not named: {ran_b:?} {writes_b:?}");
    assert_eq!(name_b.lock().unwrap().as_str(), "", "a reopen never commissions");

    // The commissioned board comes back: relinked, re-armed, resumed to done from the lost line.
    let la = TcpListener::bind(("127.0.0.1", port)).expect("rebind for the commissioned board");
    let fake_a = fake_grbl_named(la, name_a.clone(), None);
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e.starts_with("relinked") || e.starts_with("reconnect attempt 2")) {
        PumpEvent::Squawk { event: Some(e), .. } => {
            assert_eq!(e, format!("relinked on attempt 2; re-arm to resume from line {at}"))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(desk.rearm(&pass, &p, GRBL), Ok(at));
    let out = desk.apply(&PumpEvent::PumpResume { principal: Origin::LocalUser("una".to_string()) }, &mut *open);
    assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }] if e == "resume"), "{out:?}");
    match drive_until(&mut desk, &mut *open, t0, &mut words, |e| e == "done" || e.starts_with("link lost")) {
        PumpEvent::Squawk { event: Some(e), .. } => assert_eq!(e, "done"),
        other => panic!("{other:?}"),
    }
    drop(desk);
    let (ran_a, writes_a) = fake_a.join().unwrap();
    assert_eq!(ran_a, lines[5..].to_vec(), "the commissioned board ran only the lines from the lost one on");
    assert!(writes_a.is_empty(), "a reopen never commissions");
}

// --- VUGCOMSCANHOSTDIALECT (SR566): serve_pump's open runs on tokio's blocking pool, the worker stays free ---

use bandy::{SMessage, Synapse};
use comscan::pump::serve_pump;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast::error::RecvError;

#[tokio::test(flavor = "current_thread")] // ONE worker: a blocking open on it would hold every task
async fn serve_pump_leaves_the_worker_free_while_an_open_sleeps() {
    let (opening, opened) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let (o1, o2) = (opening.clone(), opened.clone());
    let slow: Box<Opener> = Box::new(move |port: &str, d: Dialect| -> Result<Box<dyn Transport + Send>, String> {
        o1.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(300));
        o2.store(true, Ordering::SeqCst);
        Err(format!("slow board at {port} handed {d:?}"))
    });
    let (desk, open) = compose(machine(), no_identity(), slow, opener(Duration::from_millis(500)), no_ws());
    let desk = Arc::new(Mutex::new(desk));
    let p = program();
    let pass = Verdict::Pass { digest: p.digest(), machine: machine().digest() };
    let digest = desk.lock().unwrap().arm(&pass, &p, Dialect::Marlin).expect("armed by a pass");

    let syn = Synapse::new();
    let rx = syn.subscribe();
    let mut watch = syn.subscribe();
    let (mut probe_rx, probe_syn) = (syn.subscribe(), syn.clone());
    // Another task on the same worker, answering a bus event serve_pump ignores.
    let probe = tokio::spawn(async move {
        loop {
            match probe_rx.recv().await {
                Ok(SMessage::Kill(w)) if w == "tcp-probe" => probe_syn.fire(SMessage::Kill("tcp-probe-answered".to_string())),
                Ok(SMessage::Kill(w)) if w == "comscan" => break,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
    let task = tokio::spawn(serve_pump(syn.clone(), rx, desk.clone(), open, Duration::from_millis(5)));
    syn.fire(SMessage::Pump(PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "marlin".to_string(),
        port: "/dev/comscan-slow-board".to_string(),
    }));

    tokio::time::timeout(Duration::from_secs(2), async {
        while !opening.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the open began");
    assert!(!opened.load(Ordering::SeqCst), "the test task ran while the open sleeps (a blocking open holds the worker)");
    assert!(desk.try_lock().is_ok(), "the desk lock is not held across the open");

    syn.fire(SMessage::Kill("tcp-probe".to_string()));
    tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            match watch.recv().await {
                Ok(SMessage::Kill(w)) if w == "tcp-probe-answered" => return,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the bus closed"),
            }
        }
    })
    .await
    .expect("the probe was answered while the open sleeps");
    assert!(!opened.load(Ordering::SeqCst), "answered before the open returned");

    // The handback on the runtime thread: the refused open named by the opener (handed the PumpStart's dialect).
    let why = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match watch.recv().await {
                Ok(SMessage::Pump(PumpEvent::PumpRefused { why })) => return why,
                Ok(SMessage::Pump(PumpEvent::Squawk { event: Some(e), .. })) => panic!("unexpected squawk {e}"),
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the bus closed"),
            }
        }
    })
    .await
    .expect("the open's answer");
    assert!(opened.load(Ordering::SeqCst));
    assert!(why.contains("slow board at /dev/comscan-slow-board handed Marlin"), "{why}");
    assert_eq!(desk.lock().unwrap().armed(), vec![digest], "a refused open leaves the arming");

    syn.fire(SMessage::Kill("comscan".to_string()));
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("stops on Kill").unwrap();
    tokio::time::timeout(Duration::from_secs(5), probe).await.expect("the probe stops").unwrap();
}
