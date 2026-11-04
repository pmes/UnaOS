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
    assert_eq!(p.state(), PumpState::LinkLost { at_line: 1, program_digest: 0 }); // VUGCOMSCANRECONNECT (SR521)
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

// --- VUGCOMSCANRECONNECT (LEDGER SR521): the lost link, the re-arm and the redial backoff ---

#[test]
fn the_redial_backoff_is_250_ms_doubling_to_4_s() {
    use pump_core::pump::{REDIAL_FIRST_US, REDIAL_MAX_US, Redial, backoff_us};
    let ms: Vec<u64> = (1..=7).map(|a| backoff_us(a) / 1000).collect();
    assert_eq!(ms, vec![250, 500, 1000, 2000, 4000, 4000, 4000]);
    assert_eq!((backoff_us(0), backoff_us(u32::MAX)), (REDIAL_FIRST_US, REDIAL_MAX_US));
    let mut r = Redial::new(1_000_000);
    assert_eq!((r.attempt(), r.wait_us()), (1, 250_000));
    assert!(!r.due(1_249_999) && r.due(1_250_000));
    r.failed(2_000_000);
    assert_eq!((r.attempt(), r.wait_us()), (2, 500_000));
    assert!(!r.due(2_499_999) && r.due(2_500_000));
    for _ in 0..10 {
        r.failed(10_000_000);
    }
    assert_eq!((r.attempt(), r.wait_us()), (12, REDIAL_MAX_US));
}

#[test]
fn a_link_lost_mid_program_is_link_lost_at_the_first_unacked_line_and_refuses_resume_by_name() {
    let text = program(60);
    let mut m = MockGrbl::new(1);
    let mut p = Pump::new(GRBL, &text).unwrap().with_status_every(0);
    drive(&mut p, &mut m, 500, |ev| count(ev, |e| matches!(e, Event::Ok(_))) >= 7);
    let acked = p.progress().acked;
    assert!(acked >= 7 && p.progress().sent > acked, "lines in flight at the loss");
    let at = p.line_of(acked).unwrap();
    m.closed = true;
    assert_eq!(p.tick(&mut m, 900_000), vec![Event::LinkLost]);
    assert_eq!(p.state(), PumpState::LinkLost { at_line: at, program_digest: 0 });
    assert_eq!(p.progress().acked, acked);
    // Resume is refused by name, before any byte: the second link sees nothing.
    let mut m2 = MockGrbl::new(1);
    assert_eq!(p.resume(&mut m2), Err(PumpRefusal::ResumeAfterLinkLost { at_line: at }));
    assert!(m2.realtime.is_empty());
    // A lost pump sends nothing on a new link while it waits.
    for k in 0..20 {
        p.tick(&mut m2, 1_000_000 + k * 1000);
    }
    assert!(m2.executed.is_empty() && m2.realtime.is_empty());
    // Stop ends a lost run without a byte.
    p.stop(&mut m2).unwrap();
    assert_eq!(p.state(), PumpState::Stopped);
    assert!(m2.realtime.is_empty());
}

#[test]
fn the_acks_that_arrive_with_the_loss_are_counted_before_it() {
    struct Fin(Vec<u8>);
    impl Transport for Fin {
        fn write(&mut self, b: &[u8]) -> Result<usize, LinkError> {
            Ok(b.len())
        }
        fn read(&mut self, b: &mut [u8]) -> Result<usize, LinkError> {
            if self.0.is_empty() {
                return Err(LinkError::Closed);
            }
            let n = b.len().min(self.0.len());
            b[..n].copy_from_slice(&self.0[..n]);
            self.0.drain(..n);
            Ok(n)
        }
    }
    let mut p = Pump::new(GRBL, &program(10)).unwrap().with_status_every(0);
    // First tick sends the window over a link that answers nothing yet.
    struct Quiet;
    impl Transport for Quiet {
        fn write(&mut self, b: &[u8]) -> Result<usize, LinkError> {
            Ok(b.len())
        }
        fn read(&mut self, _: &mut [u8]) -> Result<usize, LinkError> {
            Ok(0)
        }
    }
    p.tick(&mut Quiet, 1);
    assert!(p.progress().sent >= 3);
    // Two oks then the FIN in the same tick: both count, the loss is at the third line.
    let mut fin = Fin(b"ok\r\nok\r\n".to_vec());
    let ev = p.tick(&mut fin, 2);
    assert_eq!(ev, vec![Event::Ok(p.line_of(0).unwrap()), Event::Ok(p.line_of(1).unwrap()), Event::LinkLost]);
    assert_eq!(p.state(), PumpState::LinkLost { at_line: p.line_of(2).unwrap(), program_digest: 0 });
}

#[test]
fn a_rearmed_pump_resumes_from_the_named_line_and_runs_only_the_rest() {
    let text = program(40);
    let lines = Pump::new(GRBL, &text).unwrap().lines().to_vec();
    let fresh = Pump::new(GRBL, &text).unwrap().with_status_every(0);
    assert_eq!(fresh.line_of(0), Some(1));
    assert_eq!(fresh.line_of(1), Some(3), "the comment line is dropped");
    let at = fresh.line_of(12).unwrap();
    assert_eq!(
        Pump::new(GRBL, &text).unwrap().resume_at(2).err(),
        Some(PumpRefusal::NoSuchLine { line: 2 }),
        "a comment line is not a line the pump sends"
    );
    let mut p = fresh.resume_at(at).unwrap();
    assert_eq!(p.state(), PumpState::Rearmed { at_line: at });
    assert_eq!((p.progress().sent, p.progress().acked), (12, 12));
    let mut m = MockGrbl::new(1);
    for k in 0..5 {
        p.tick(&mut m, k * 1000);
    }
    assert!(m.executed.is_empty(), "a re-armed pump waits for Resume");
    p.resume(&mut m).unwrap();
    assert_eq!(p.state(), PumpState::Streaming);
    assert!(!m.realtime.contains(&b'~'), "the re-armed resume is not a cycle start byte");
    let ev = drive(&mut p, &mut m, 5_000, |ev| ev.contains(&Event::Done));
    assert!(ev.contains(&Event::Done));
    let modal: Vec<String> = lines[0].split_whitespace().map(str::to_string).collect(); // VUGCOMSCANRESUMEMODAL (SR656): the preamble's groups, one line each, first
    assert_eq!(m.executed, modal.iter().cloned().chain(lines[12..].iter().cloned()).collect::<Vec<_>>());
    let pr = p.progress();
    let n = lines.len() + modal.len();
    assert_eq!((pr.sent, pr.acked, pr.total), (n, n, n));
}

// --- VUGCOMSCANJOGFACE (SR579): grbl's jog cancel is the one realtime byte 0x85, never a feed hold ---

#[test]
fn jog_cancel_is_0x85_on_the_wire_and_no_feed_hold() {
    let mut m = MockGrbl::new(1);
    let mut p = Pump::new(GRBL, &program(3)).unwrap();
    let ev = drive(&mut p, &mut m, 2_000, |ev| ev.contains(&Event::Done));
    assert!(ev.contains(&Event::Done));
    let before = m.realtime.len();
    p.jog_cancel(&mut m).unwrap();
    assert_eq!(&m.realtime[before..], &[0x85u8], "the jog cancel byte, alone");
    assert!(!m.realtime.contains(&b'!'), "a jog cancel is not a feed hold");
    assert_eq!(p.state(), PumpState::Done, "the pump's own state is untouched");
    let mut marlin = Pump::new(Dialect::Marlin, "G28\n").unwrap();
    assert_eq!(
        marlin.jog_cancel(&mut m).err(),
        Some(PumpRefusal::NotInDialect { dialect: Dialect::Marlin, what: "jog cancel" })
    );
}

// --- VUGCOMSCANRESUMEMODAL (SR656): a resume restores the modal words the lines before its line set ---

/// `lines` with `words` spliced in at index `i`.
fn with_prefix(lines: &[String], i: usize, words: &[&str]) -> Vec<String> {
    let mut v = lines[..i].to_vec();
    v.extend(words.iter().map(|w| w.to_string()));
    v.extend_from_slice(&lines[i..]);
    v
}

#[test]
fn a_resume_restores_the_modal_words_set_before_its_line() {
    // grbl: every group the block set, the distance mode from its LAST word (G91.1 is the arc mode, not G91), program order.
    let text = "G21 G90 G17 G94\nG91.1 G1 X1 F100\nG1 X2\nG91 G1 X3\nG1 X4\n";
    let lines = Pump::new(GRBL, text).unwrap().lines().to_vec();
    let p = Pump::new(GRBL, text).unwrap().with_status_every(0).resume_at(5).unwrap();
    assert_eq!(p.lines().to_vec(), with_prefix(&lines, 4, &["G21", "G17", "G94", "G91"]));
    assert_eq!((p.progress().sent, p.progress().acked), (4, 4), "the prefix is unsent");
    assert!((4..9).all(|k| p.line_of(k) == Some(5)), "the prefix is numbered at the resume line");

    // Marlin: one command per line (the M82 after G90 is never run), M83 then G90 in the program's order.
    let text = "G21\nG91\nM83\nG1 X1 E1 F100\nG90 M82\nG1 X2 E2\nG1 X3 E3\n";
    let lines = Pump::new(Dialect::Marlin, text).unwrap().lines().to_vec();
    let p = Pump::new(Dialect::Marlin, text).unwrap().resume_at(7).unwrap();
    assert_eq!(p.lines().to_vec(), with_prefix(&lines, 6, &["G21", "M83", "G90"]));

    // No modal word before the line: nothing is added.
    let text = "G1 X1 F100\nG1 X2\nG21\nG1 X3\n";
    let lines = Pump::new(GRBL, text).unwrap().lines().to_vec();
    let p = Pump::new(GRBL, text).unwrap().resume_at(2).unwrap();
    assert_eq!(p.lines().to_vec(), lines);

    // A loss inside the prefix is a loss at the resume line: the re-arm restores again.
    struct Quiet;
    impl Transport for Quiet {
        fn write(&mut self, b: &[u8]) -> Result<usize, LinkError> {
            Ok(b.len())
        }
        fn read(&mut self, _: &mut [u8]) -> Result<usize, LinkError> {
            Ok(0)
        }
    }
    struct Fin(Vec<u8>);
    impl Transport for Fin {
        fn write(&mut self, b: &[u8]) -> Result<usize, LinkError> {
            Ok(b.len())
        }
        fn read(&mut self, b: &mut [u8]) -> Result<usize, LinkError> {
            if self.0.is_empty() {
                return Err(LinkError::Closed);
            }
            let n = b.len().min(self.0.len());
            b[..n].copy_from_slice(&self.0[..n]);
            self.0.drain(..n);
            Ok(n)
        }
    }
    let text = "G21 G90 G17 G94\nG91.1 G1 X1 F100\nG1 X2\nG91 G1 X3\nG1 X4\n";
    let mut p = Pump::new(GRBL, text).unwrap().with_status_every(0).resume_at(5).unwrap();
    p.resume(&mut Quiet).unwrap();
    p.tick(&mut Quiet, 0);
    assert_eq!(p.progress().sent, 9, "the prefix and the resume line went out");
    let ev = p.tick(&mut Fin(b"ok\r\n".to_vec()), 1);
    assert_eq!(ev, vec![Event::Ok(5), Event::LinkLost]);
    assert_eq!(p.state(), PumpState::LinkLost { at_line: 5, program_digest: 0 });
}

// --- VUGCOMSCANRESUMETOOL (SR663): a resume restores the spindle, coolant, heater, fan and tool words set before its line ---

#[test]
fn a_resume_restores_the_tool_state_set_before_its_line() {
    // grbl: the S before the M in one block, the spin-up dwell right after the spindle word, the coolant after it; the
    // whole prefix numbered at the resume line.
    let text = "G21 G90 G17 G94\nS1200 M3\nM8\nG1 X1 F100\nG1 X2\n";
    let lines = Pump::new(GRBL, text).unwrap().lines().to_vec();
    let p = Pump::new(GRBL, text).unwrap().with_status_every(0).resume_at(5).unwrap();
    assert_eq!(p.lines().to_vec(), with_prefix(&lines, 4, &["G21", "G90", "G17", "G94", "M3 S1200", "G4 P4", "M8"]));
    assert_eq!((p.progress().sent, p.progress().acked), (4, 4), "the prefix is unsent");
    assert!((4..12).all(|k| p.line_of(k) == Some(5)), "the prefix is numbered at the resume line");

    // An S alone changes a running spindle's speed; M9 clears both coolants and is said once; a later M7 stays.
    let text = "M3 S1000\nM8\nS1500\nM9\nM7\nG1 X1 F100\n";
    let lines = Pump::new(GRBL, text).unwrap().lines().to_vec();
    let p = Pump::new(GRBL, text).unwrap().resume_at(6).unwrap();
    assert_eq!(p.lines().to_vec(), with_prefix(&lines, 5, &["M3 S1500", "G4 P4", "M9", "M7"]));

    // M5 is said again, with no dwell.
    let text = "M3 S1000\nG1 X1 F100\nM5\nG0 Z5\n";
    let lines = Pump::new(GRBL, text).unwrap().lines().to_vec();
    let p = Pump::new(GRBL, text).unwrap().resume_at(4).unwrap();
    assert_eq!(p.lines().to_vec(), with_prefix(&lines, 3, &["M5"]));

    // grbl laser mode: the power word is no prefix line, the air assist is; the M4 is folded onto the first resumed
    // motion line (VUGCOMSCANRESUMELASER SR671).
    let laser = Dialect::Grbl { laser: true };
    let text = "M4 S500\nM8\nG1 X1 F100\nG1 X2\n";
    let lines = Pump::new(laser, text).unwrap().lines().to_vec();
    let p = Pump::new(laser, text).unwrap().resume_at(4).unwrap();
    let mut want = with_prefix(&lines, 3, &["M8"]);
    want[4] = String::from("M4 G1 X2 S500");
    assert_eq!(p.lines().to_vec(), want);

    // Marlin: the bed and hotend targets restored as the blocking M190 / M109, then the tool and the fan, in the
    // program's order after its modal words.
    let text = "G21\nG90\nM82\nM140 S60\nM104 S210\nT1\nM106 S128\nG1 X1 E1 F100\nG1 X2 E2\n";
    let lines = Pump::new(Dialect::Marlin, text).unwrap().lines().to_vec();
    let p = Pump::new(Dialect::Marlin, text).unwrap().resume_at(9).unwrap();
    assert_eq!(
        p.lines().to_vec(),
        with_prefix(&lines, 8, &["G21", "G90", "M82", "M190 S60", "M109 S210", "T1", "M106 S128"])
    );

    // M107 clears the fan, an S0 target stays non-blocking, a recorded wait stays; a packed command is re-spaced.
    let text = "M106 S255\nM104 S0\nM190 S60\nM107\nG1 X1 E1 F100\n";
    let lines = Pump::new(Dialect::Marlin, text).unwrap().lines().to_vec();
    let p = Pump::new(Dialect::Marlin, text).unwrap().resume_at(5).unwrap();
    assert_eq!(p.lines().to_vec(), with_prefix(&lines, 4, &["M104 S0", "M190 S60", "M107"]));
    let text = "M104T1S210\nG1 X1 E1 F100\n";
    let lines = Pump::new(Dialect::Marlin, text).unwrap().lines().to_vec();
    let p = Pump::new(Dialect::Marlin, text).unwrap().resume_at(2).unwrap();
    assert_eq!(p.lines().to_vec(), with_prefix(&lines, 1, &["M109 T1 S210"]));
}

// --- VUGCOMSCANMARLINJOGMODAL (SR664): a jog reads the run's units and distance from the lines the pump sent ---

use pump_core::pump::{Modal, jog_lines};

#[test]
fn a_jog_restores_the_runs_distance_and_says_mm_under_g20() {
    let s = |v: &[&str]| -> Option<Vec<String>> { Some(v.iter().map(|x| x.to_string()).collect()) };
    let text = "G20\nG91\nG1 X1 F100\nG90\nG1 X2\n";
    let fresh = Pump::new(Dialect::Marlin, text).unwrap();
    assert_eq!(fresh.sent_modal(), Modal::default(), "nothing sent, nothing recorded");
    assert_eq!(
        jog_lines(Dialect::Marlin, [1.0, 0.0, 0.0], 600.0, fresh.sent_modal()),
        s(&["G91", "G0 X1 Y0 Z0 F600", "G90"]),
        "no run: the board's default G90, the old three lines"
    );

    // Halted at line 3 (a resend): the board ran G20 / G91 / the move and never saw the G90.
    let mut t = Marlin { got: Vec::new(), out: Vec::new(), max_outstanding: 0, pending: 0, resend_at: Some(3) };
    let mut p = Pump::new(Dialect::Marlin, text).unwrap();
    drive(&mut p, &mut t, 100, |_| false);
    assert_eq!(p.state(), PumpState::Halted(Halt::Refused { line: 3 }));
    let m = p.sent_modal();
    assert_eq!((m.units(), m.distance()), (Some("G20"), Some("G91")), "only the lines sent count");
    assert_eq!(
        jog_lines(Dialect::Marlin, [1.0, -2.0, 0.5], 600.0, m),
        s(&["G21", "G91", "G0 X1 Y-2 Z0.5 F600", "G91", "G20"]),
        "mm for the jog, the run's relative mode and inches after"
    );
    assert_eq!(jog_lines(GRBL, [1.0, 0.0, 0.0], 600.0, Modal::of(GRBL, p.lines())), s(&["$J=G21 G91 X1 Y0 Z0 F600"]));

    // Run to Done: the G90 after the move is the run's last distance word.
    let mut t = Marlin { got: Vec::new(), out: Vec::new(), max_outstanding: 0, pending: 0, resend_at: None };
    let mut p = Pump::new(Dialect::Marlin, text).unwrap();
    let ev = drive(&mut p, &mut t, 100, |e| e.contains(&Event::Done));
    assert!(ev.contains(&Event::Done));
    let m = p.sent_modal();
    assert_eq!((m.units(), m.distance()), (Some("G20"), Some("G90")));
    assert_eq!(
        jog_lines(Dialect::Marlin, [1.0, 0.0, 0.0], 600.0, m),
        s(&["G21", "G91", "G0 X1 Y0 Z0 F600", "G90", "G20"])
    );
    let mm = Modal::of(GRBL, &[String::from("G21 G90")]);
    assert_eq!(jog_lines(GRBL, [1.0, 0.0, 0.0], 600.0, mm), s(&["$J=G91 X1 Y0 Z0 F600"]), "mm: the jog line unchanged");
}

// --- VUGCOMSCANRESUMELASER (SR671): a grbl laser resume folds its dynamic M4 power onto the first resumed motion line ---

#[test]
fn a_laser_resume_folds_dynamic_power_onto_the_first_resumed_motion() {
    let laser = Dialect::Grbl { laser: true };
    let fold = |text: &str, at: usize| Pump::new(laser, text).unwrap().resume_at(at).unwrap().lines().to_vec();
    let bare = |text: &str| Pump::new(laser, text).unwrap().lines().to_vec();

    // Lost at a G1: the prefix carries no M4 line; the first resumed G1 carries M4 and the recorded S; the next stays
    // bare; the numbering is untouched.
    let text = "G21 G90 G17 G94\nM4 S500\nM8\nG1 X1 F100\nG1 X2 F100\nG1 X3 F100\n";
    let lines = bare(text);
    let p = Pump::new(laser, text).unwrap().with_status_every(0).resume_at(5).unwrap();
    let mut want = with_prefix(&lines, 4, &["G21", "G90", "G17", "G94", "M8"]);
    want[9] = String::from("M4 G1 X2 F100 S500");
    assert_eq!(p.lines().to_vec(), want);
    assert_eq!(p.lines()[10], "G1 X3 F100", "only the first resumed motion line carries the power");
    assert!(!p.lines()[4..9].iter().any(|l| l.starts_with("M4")), "no standalone M4 line");
    assert_eq!((p.line_of(9), p.line_of(10)), (Some(5), Some(6)));
    assert_eq!((p.progress().sent, p.progress().acked, p.progress().total), (4, 4, 11));

    // A G0 first stays bare (grbl runs rapids with the laser off); the next G1 carries the power.
    let text = "M4 S500\nG1 X1 F100\nG0 X5\nG1 X6 F100\n";
    let mut want = bare(text);
    want[3] = String::from("M4 G1 X6 F100 S500");
    assert_eq!(fold(text, 3), want);

    // A G1 with its own S keeps it; an S said on a resumed line before the G1 is kept too (the M4 alone is folded).
    let text = "M4 S500\nG1 X1 F100\nG1 X2 F100 S300\nG1 X3\n";
    let mut want = bare(text);
    want[2] = String::from("M4 G1 X2 F100 S300");
    assert_eq!(fold(text, 3), want);
    let text = "M4 S500\nG1 X1 F100\nS300\nG1 X2 F100\n";
    let mut want = bare(text);
    want[3] = String::from("M4 G1 X2 F100");
    assert_eq!(fold(text, 3), want);

    // An M3 (constant power: a static beam) is never restored; an M5 before the line restores nothing.
    let text = "M3 S500\nG1 X1 F100\nG1 X2\n";
    assert_eq!(fold(text, 3), bare(text));
    let text = "M4 S500\nG1 X1 F100\nM5\nG0 X0\nG1 X2 F100\n";
    assert_eq!(fold(text, 4), bare(text));

    // A program that states its own power before its next motion wins: nothing is folded.
    let text = "M4 S500\nG1 X1 F100\nM5\nG1 X2 F100\n";
    assert_eq!(fold(text, 3), bare(text));
    let text = "M4 S500\nG1 X1 F100\nM4 S200\nG1 X2 F100\n";
    assert_eq!(fold(text, 3), bare(text));

    // No motion after the resume line: nothing is restored.
    let text = "M4 S500\nG1 X1 F100\nG0 X0 Y0\nM2\n";
    assert_eq!(fold(text, 3), bare(text));

    // Outside laser mode the spindle is a prefix line with its dwell, and no line is folded.
    let text = "M3 S1000\nG1 X1 F100\nG1 X2\n";
    let lines = Pump::new(GRBL, text).unwrap().lines().to_vec();
    let p = Pump::new(GRBL, text).unwrap().resume_at(3).unwrap();
    assert_eq!(p.lines().to_vec(), with_prefix(&lines, 2, &["M3 S1000", "G4 P4"]));

    // make_core's own laser text: the power on its own line, the motion lines bare; the resume folds the writer's S.
    let mut prog = Program::new(Units::Mm);
    prog.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 0.0) });
    prog.moves.push(Move::Power { pct: 50.0 });
    prog.moves.push(Move::Linear { to: Vec3::new(10.0, 0.0, 0.0), feed: 600.0 });
    prog.moves.push(Move::Linear { to: Vec3::new(10.0, 10.0, 0.0), feed: 600.0 });
    let lines = Pump::from_program(&prog, laser).unwrap().lines().to_vec();
    assert!(lines[2].starts_with("M4 S"), "the writer says the power on its own line: {:?}", lines);
    assert!(!lines[4].contains('S'), "the writer's motion lines carry no S");
    let s = lines[2]["M4 S".len()..].to_string();
    let p = Pump::from_program(&prog, laser).unwrap().resume_at(5).unwrap();
    let mut want = with_prefix(&lines, 4, &["G21", "G90", "G17", "G94"]);
    want[8] = format!("M4 {} S{}", lines[4], s);
    assert_eq!(p.lines().to_vec(), want);

    // A folded line past grbl's window is refused by name, never sent cut.
    let long = format!("G1 X1.{} F100", "0".repeat(109));
    assert_eq!(long.len() + 1, 121);
    let text = format!("M4 S500\nG1 X1 F100\n{long}\n");
    assert_eq!(
        Pump::new(laser, &text).unwrap().resume_at(3).err(),
        Some(PumpRefusal::LineTooLong { line: 3, len: long.len() + 1 + "M4 ".len() + " S500".len() })
    );
}
