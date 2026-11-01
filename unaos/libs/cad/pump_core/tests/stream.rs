// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGCOMSCAN1 (SR476): the pump against MockGrbl (a 128-byte receive buffer that refuses overflow) and a scripted
//! Marlin; the grbl tables and the status grammar on the interface doc's own examples.

use make_core::gcode::Dialect;
use make_core::{Move, Program, Units, Vec3};
use pump_core::grbl::{ALARMS, ERRORS};
use pump_core::mock::{MockGrbl, Reply};
use pump_core::pump::GRBL_RX;
use pump_core::{
    Event, Halt, LinkError, Pump, PumpRefusal, PumpState, State, StatusRefusal, Transport, alarm_name, error_name,
    parse_status,
};

const GRBL: Dialect = Dialect::Grbl { laser: false };

fn program(n: usize) -> String {
    let mut s = String::from("G21 G90 G17 G94\n; a comment line\n");
    for i in 0..n {
        // Lines of varying length, so the window packs unevenly.
        s.push_str(&format!("G1 X{}.{} Y{} F{} (pass {})\n", i % 97, i % 10, (i * 7) % 113, 300 + (i % 5) * 100, i));
    }
    s
}

/// Tick until `stop(events)` or `max` ticks; every event in order. 1 ms per tick.
fn drive(p: &mut Pump, m: &mut dyn Transport, max: usize, mut stop: impl FnMut(&[Event]) -> bool) -> Vec<Event> {
    let mut all = Vec::new();
    for k in 0..max {
        let ev = p.tick(m, (k as u64 + 1) * 1000);
        all.extend(ev);
        if stop(&all) {
            break;
        }
    }
    all
}

fn count(ev: &[Event], f: impl Fn(&Event) -> bool) -> usize {
    ev.iter().filter(|e| f(e)).count()
}

#[test]
fn a_long_program_never_overflows_the_128_byte_buffer_and_every_line_is_acked() {
    let text = program(400);
    for delay in [1u32, 3, 7] {
        let mut m = MockGrbl::new(delay);
        let mut p = Pump::new(GRBL, &text).unwrap();
        let total = p.lines().len();
        assert_eq!(total, 401, "the comment line dropped, the header kept");
        let ev = drive(&mut p, &mut m, 200_000, |e| e.iter().any(|x| *x == Event::Done));
        assert!(!m.overflowed, "delay {delay}: the window overflowed the receive buffer");
        assert!(m.max_fill <= 128 && m.max_fill <= GRBL_RX, "delay {delay}: fill {}", m.max_fill);
        assert!(m.max_fill > 64, "delay {delay}: the window should keep the buffer well filled, got {}", m.max_fill);
        assert_eq!(count(&ev, |e| matches!(e, Event::Ok(_))), total);
        assert_eq!(count(&ev, |e| *e == Event::Done), 1, "Done said once");
        assert_eq!(m.executed, p.lines().to_vec(), "the controller ran exactly the program's lines, in order");
        assert!(!m.executed.iter().any(|l| l.contains("pass")), "comments never reach the wire");
        let pr = p.progress();
        assert_eq!((pr.sent, pr.acked, pr.total, pr.state), (total, total, total, PumpState::Done));
        // Ok lines are the 1-based text lines: line 2 is the comment, so the first G1 is text line 3.
        assert_eq!(ev.iter().find_map(|e| if let Event::Ok(l) = e { Some(*l) } else { None }), Some(1));
        assert!(ev.contains(&Event::Ok(3)) && !ev.contains(&Event::Ok(2)));
    }
}

#[test]
fn partial_writes_still_keep_the_window() {
    let mut m = MockGrbl::new(2);
    m.accept_per_write = 5;
    let mut p = Pump::new(GRBL, &program(60)).unwrap();
    let ev = drive(&mut p, &mut m, 100_000, |e| e.contains(&Event::Done));
    assert!(ev.contains(&Event::Done));
    assert!(!m.overflowed);
    assert_eq!(m.executed.len(), 61);
}

#[test]
fn an_error_halts_with_a_feed_hold_and_nothing_after_the_skipped_line_is_sent() {
    let text = program(50);
    let mut m = MockGrbl::new(1).script(10, Reply::Error(22));
    let mut p = Pump::new(GRBL, &text).unwrap().with_status_every(0);
    let ev = drive(&mut p, &mut m, 5_000, |_| false);
    let err = ev.iter().find(|e| matches!(e, Event::Error { .. })).cloned().unwrap();
    // Executed index 10 is the 11th line: the header (text line 1) then G1 lines from text line 3.
    assert_eq!(err, Event::Error { line: 12, code: 22, name: "Undefined feed rate" });
    assert_eq!(p.state(), PumpState::Halted(Halt::Error { line: 12, code: 22 }));
    assert!(m.realtime.contains(&b'!'), "a feed hold went out on the error");
    let sent_at_halt = p.progress().sent;
    // More ticks send nothing new.
    let more = drive(&mut p, &mut m, 1_000, |_| false);
    assert_eq!(p.progress().sent, sent_at_halt);
    assert!(!more.contains(&Event::Done));
    assert!(sent_at_halt < p.progress().total);
}

#[test]
fn an_alarm_halts_by_name() {
    let mut m = MockGrbl::new(1).script(5, Reply::Alarm(1));
    let mut p = Pump::new(GRBL, &program(40)).unwrap();
    let ev = drive(&mut p, &mut m, 5_000, |_| false);
    assert!(ev.contains(&Event::Alarm { code: 1, name: "Hard limit" }));
    assert_eq!(p.state(), PumpState::Halted(Halt::Alarm { code: 1 }));
    assert!(!ev.contains(&Event::Done));
}

#[test]
fn stop_is_a_hold_then_a_reset_once_the_hold_is_complete() {
    let mut m = MockGrbl::new(4);
    let mut p = Pump::new(GRBL, &program(300)).unwrap();
    drive(&mut p, &mut m, 50, |_| false);
    assert!(m.executed.len() < 300);
    p.stop(&mut m).unwrap();
    assert_eq!(p.state(), PumpState::Stopping);
    let i_hold = m.realtime.iter().position(|&b| b == b'!').expect("hold first");
    assert!(!m.realtime.contains(&0x18), "no reset before the hold completes");
    let ev = drive(&mut p, &mut m, 1_000, |e| e.contains(&Event::Stopped));
    assert!(ev.contains(&Event::Stopped));
    assert_eq!(p.state(), PumpState::Stopped);
    let i_reset = m.realtime.iter().position(|&b| b == 0x18).expect("reset after");
    assert!(i_hold < i_reset);
    let stopped_sent = p.progress().sent;
    let after = drive(&mut p, &mut m, 200, |_| false);
    assert_eq!(p.progress().sent, stopped_sent, "a stopped stream never resumes sending");
    // The welcome after our own reset is a message, not a mid-stream Reset.
    assert!(!after.contains(&Event::Reset));
}

#[test]
fn reset_drops_the_window_and_a_controller_reset_mid_stream_halts() {
    let mut m = MockGrbl::new(3);
    let mut p = Pump::new(GRBL, &program(100)).unwrap();
    drive(&mut p, &mut m, 20, |_| false);
    p.reset(&mut m).unwrap();
    assert_eq!(p.state(), PumpState::Stopped);
    assert_eq!(p.in_flight(), 0);
    assert!(m.realtime.contains(&0x18));

    // The controller resets itself (power blip): its welcome mid-stream halts by name.
    let mut m = MockGrbl::new(3);
    let mut p = Pump::new(GRBL, &program(100)).unwrap();
    drive(&mut p, &mut m, 20, |_| false);
    m.say("\r\nGrbl 1.1h ['$' for help]\r\n");
    let ev = drive(&mut p, &mut m, 5, |_| false);
    assert!(ev.contains(&Event::Reset));
    assert_eq!(p.state(), PumpState::Halted(Halt::Reset));
}

#[test]
fn the_boot_welcome_before_streaming_is_only_a_message() {
    let mut m = MockGrbl::new(1);
    m.say("\r\nGrbl 1.1h ['$' for help]\r\n");
    let mut p = Pump::new(GRBL, "G0 X1\n").unwrap();
    let ev = drive(&mut p, &mut m, 100, |e| e.contains(&Event::Done));
    assert!(ev.contains(&Event::Message(String::from("Grbl 1.1h ['$' for help]"))));
    assert!(ev.contains(&Event::Done));
}

#[test]
fn a_dead_controller_times_out_once() {
    let mut m = MockGrbl::new(1);
    m.dead = true;
    let mut p = Pump::new(GRBL, &program(10)).unwrap().with_timeout(50_000);
    let ev = drive(&mut p, &mut m, 500, |_| false);
    assert_eq!(count(&ev, |e| *e == Event::Timeout), 1);
    assert!(!ev.contains(&Event::Done));
}

#[test]
fn a_closed_link_is_named() {
    let mut m = MockGrbl::new(1);
    let mut p = Pump::new(GRBL, &program(10)).unwrap();
    m.closed = true;
    let ev = p.tick(&mut m, 1);
    assert_eq!(ev, vec![Event::LinkLost]);
    assert_eq!(p.state(), PumpState::Halted(Halt::Link));
    assert_eq!(m.write(b"x"), Err(LinkError::Closed));
}

#[test]
fn status_is_polled_and_parsed() {
    let mut m = MockGrbl::new(2);
    let mut p = Pump::new(GRBL, &program(30)).unwrap();
    let ev = drive(&mut p, &mut m, 2_000, |e| e.contains(&Event::Done));
    let n = count(&ev, |e| matches!(e, Event::Status(_)));
    assert!(n >= 1, "at least one report");
    // 2 ms per answered line at 1 ms ticks and a 200 ms period: the query rate stays near the doc's 5 Hz.
    assert!(m.realtime.iter().filter(|&&b| b == b'?').count() <= 3);
    assert!(p.progress().last.is_some());
}

#[test]
fn status_report_grammar_on_the_interface_docs_examples() {
    let r = parse_status("<Idle|MPos:0.000,0.000,0.000|FS:0.0,0>").unwrap();
    assert_eq!(r.state, State::Idle);
    assert_eq!(r.mpos, Some([0.0, 0.0, 0.0]));
    assert_eq!((r.feed, r.spindle), (Some(0.0), Some(0.0)));

    let r = parse_status("<Hold:1|WPos:0.000,-10.000,5.000|Bf:15,128|Ln:99999|FS:500,8000|WCO:0.000,1.551,5.664|Pn:PZ|Ov:100,100,100|A:SFM>").unwrap();
    assert_eq!(r.state, State::Hold(1));
    assert_eq!(r.wpos, Some([0.0, -10.0, 5.0]));
    assert_eq!(r.mpos, None);
    assert_eq!(r.buf, Some((15, 128)));
    assert_eq!(r.line, Some(99999));
    assert_eq!((r.feed, r.spindle), (Some(500.0), Some(8000.0)));
    assert_eq!(r.wco, Some([0.0, 1.551, 5.664]));
    assert_eq!(r.pins, "PZ");
    assert_eq!(r.ov, Some([100, 100, 100]));
    assert_eq!(r.accessories, "SFM");

    assert_eq!(parse_status("<Door:2|MPos:1,2,3|F:500>").unwrap().state, State::Door(2));
    assert_eq!(parse_status("<Run|MPos:1,2,3|F:500>").unwrap().feed, Some(500.0));
    // An unknown field (a later grbl's) is skipped; a malformed known one is refused by name.
    assert_eq!(parse_status("<Jog|MPos:1,2,3|Zz:what>").unwrap().state, State::Jog);
    assert_eq!(parse_status("<Run|MPos:1,2>"), Err(StatusRefusal::BadField("MPos")));
    assert_eq!(parse_status("<Run|MPos:1,2,3|Ov:100,100>"), Err(StatusRefusal::BadField("Ov")));
    assert_eq!(parse_status("<Fly|MPos:1,2,3>"), Err(StatusRefusal::UnknownState(String::from("Fly"))));
    assert_eq!(parse_status("Idle|MPos:1,2,3"), Err(StatusRefusal::NotChevroned));
}

#[test]
fn the_error_and_alarm_tables_by_code() {
    assert_eq!(ERRORS.len(), 36, "1-17 and 20-38");
    assert_eq!(ALARMS.len(), 10);
    assert_eq!(error_name(11), "Line overflow");
    assert_eq!(error_name(20), "Unsupported command");
    assert_eq!(error_name(38), "Invalid gcode ID:38");
    assert_eq!(error_name(18), "unknown");
    assert_eq!(alarm_name(3), "Abort during cycle");
    assert_eq!(alarm_name(10), "Homing fail");
    assert_eq!(alarm_name(11), "unknown");
}

#[test]
fn programs_the_pump_refuses_by_name() {
    let long = format!("G1 X1 {}\n", "Y1 ".repeat(60));
    assert!(matches!(Pump::new(GRBL, &long), Err(PumpRefusal::LineTooLong { line: 1, .. })));
    assert_eq!(Pump::new(GRBL, "G0 X1\nM117 ready?\n").err(), Some(PumpRefusal::RealtimeByteInLine { line: 2, byte: b'?' }));
    assert_eq!(Pump::new(GRBL, "G0 X1 ~\n").err(), Some(PumpRefusal::RealtimeByteInLine { line: 1, byte: b'~' }));
    // A `?` inside a comment never reaches the wire, so it is fine.
    assert!(Pump::new(GRBL, "G0 X1 (ready?)\n").is_ok());
    assert_eq!(Pump::new(GRBL, "; only\n\n(comments)\n").err(), Some(PumpRefusal::Empty));
    assert_eq!(
        Pump::new(Dialect::LinuxCnc, "G0 X1\n").err(),
        Some(PumpRefusal::DialectNotStreamed { dialect: Dialect::LinuxCnc })
    );
    let mut m = MockGrbl::new(1);
    let mut marlin = Pump::new(Dialect::Marlin, "G28\n").unwrap();
    assert_eq!(
        marlin.hold(&mut m).err(),
        Some(PumpRefusal::NotInDialect { dialect: Dialect::Marlin, what: "feed hold" })
    );
}

/// A scripted Marlin: one reply per line, a fixed script, no buffer of its own (send-response).
struct Marlin {
    got: Vec<String>,
    out: Vec<u8>,
    max_outstanding: usize,
    pending: usize,
    resend_at: Option<usize>,
}

impl Transport for Marlin {
    fn write(&mut self, b: &[u8]) -> Result<usize, LinkError> {
        for line in b.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
            self.got.push(String::from_utf8_lossy(line).into_owned());
            self.pending += 1;
            self.max_outstanding = self.max_outstanding.max(self.pending);
        }
        Ok(b.len())
    }
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        if self.pending > 0 && self.out.is_empty() {
            self.pending -= 1;
            let n = self.got.len();
            if Some(n) == self.resend_at {
                self.out.extend_from_slice(format!("Error:Line Number is not Last Line Number+1, Last Line: {}\nResend: {}\nok\n", n - 1, n).as_bytes());
            } else {
                self.out.extend_from_slice(b"echo:busy: processing\nok T:20.0 /0.0\n");
            }
        }
        let n = buf.len().min(self.out.len());
        buf[..n].copy_from_slice(&self.out[..n]);
        self.out.drain(..n);
        Ok(n)
    }
}

#[test]
fn marlin_keeps_one_line_outstanding_and_a_resend_halts_by_name() {
    let text = "G28\nM104 S200\nG1 X10 Y10 F1200\nG1 X20\n";
    let mut t = Marlin { got: Vec::new(), out: Vec::new(), max_outstanding: 0, pending: 0, resend_at: None };
    let mut p = Pump::new(Dialect::Marlin, text).unwrap();
    let ev = drive(&mut p, &mut t, 100, |e| e.contains(&Event::Done));
    assert!(ev.contains(&Event::Done));
    assert_eq!(t.max_outstanding, 1, "send-response");
    assert_eq!(t.got.len(), 4);
    assert!(ev.contains(&Event::Message(String::from("echo:busy: processing"))));

    let mut t = Marlin { got: Vec::new(), out: Vec::new(), max_outstanding: 0, pending: 0, resend_at: Some(2) };
    let mut p = Pump::new(Dialect::Marlin, text).unwrap();
    let ev = drive(&mut p, &mut t, 100, |_| false);
    assert!(ev.iter().any(|e| matches!(e, Event::Refused { line: 2, .. })));
    assert_eq!(p.state(), PumpState::Halted(Halt::Refused { line: 2 }));
    assert!(ev.contains(&Event::ResendAsked { line: 2 }), "the resend is named even after the halt");
    assert_eq!(t.got.len(), 2, "nothing after the faulted line");
}

#[test]
fn from_program_streams_make_cores_own_text() {
    let mut p = Program::new(Units::Mm);
    p.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 5.0) });
    p.moves.push(Move::Linear { to: Vec3::new(10.0, 0.0, -1.0), feed: 300.0 });
    p.moves.push(Move::Linear { to: Vec3::new(10.0, 10.0, -1.0), feed: 300.0 });
    let text = make_core::gcode::write(&p, GRBL).unwrap();
    let mut pump = Pump::from_program(&p, GRBL).unwrap();
    let want: Vec<String> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).map(String::from).collect();
    assert_eq!(pump.lines().to_vec(), want);
    let mut m = MockGrbl::new(1);
    let ev = drive(&mut pump, &mut m, 1_000, |e| e.contains(&Event::Done));
    assert!(ev.contains(&Event::Done));
    assert_eq!(m.executed, want);
}
