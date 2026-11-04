// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The pump: make_core's dialect text, one line at a time, kept inside the controller's receive window.
//!
//! grbl 1.1 is streamed by character counting (gnea/grbl `doc/markdown/interface.md` "Streaming Protocol:
//! Character-Counting"): a line is sent while the bytes of every unacknowledged line plus its own stay within
//! [`GRBL_RX`]; each `ok` or `error:N` frees the oldest. Marlin is send-response: one line outstanding. LinuxCNC loads a
//! program, it is not streamed, and is refused by name.
//!
//! The safety rules this module keeps:
//! - an `error:N` means grbl skipped that line: nothing after it is sent, and a feed hold goes out at once (the lines
//!   already in the controller's buffer would otherwise run without the skipped one);
//! - an `ALARM:N`, a controller reset mid-stream or a Marlin resend halts the stream by name; a halted stream never
//!   resumes sending (the remainder would start from the wrong place);
//! - STOP is a feed hold, then a soft reset once a status report shows the hold complete (`Hold:0`) or `Idle`: grbl's
//!   alarm 3 "Abort during cycle" is a reset during motion, which loses the position; a run halted by an alarm is the
//!   soft reset at once (the controller reports `Alarm`, never `Idle`, and hears nothing else; VUGCOMSCANALARMRESET SR585);
//! - an alarm (grbl `ALARM:N`, Marlin's halt) halts the pump in every state the link reaches, a job or not (a hard limit
//!   during a jog after the run), so Stop is the reset and the unlock (VUGCOMSCANALARMIDLE SR591).

use alloc::{collections::VecDeque, string::String, vec::Vec};

use make_core::Program;
use make_core::gcode::{Dialect, Realtime, write};
use make_core::validate::Refusal;

use crate::Transport;
use crate::grbl::{State, StatusReport, alarm_name, error_name, parse_status};

/// The grbl window in bytes: the 128-character receive buffer less one ("stores up to 127 characters").
pub const GRBL_RX: usize = 127;
/// The status query period: the interface doc asks for no more than 5 Hz.
pub const STATUS_EVERY_US: u64 = 200_000;
/// Silence with lines outstanding before [`Event::Timeout`] (a long dwell or a slow move holds an `ok`).
pub const TIMEOUT_US: u64 = 30_000_000;

/// A program the pump will not take, or a request its dialect cannot say, by name. Lines are 1-based text lines.
#[derive(Clone, Debug, PartialEq)]
pub enum PumpRefusal {
    /// No line left after comments and blank lines are dropped.
    Empty,
    /// The line with its `\n` does not fit grbl's window, so it could never be sent.
    LineTooLong { line: usize, len: usize },
    /// A byte grbl takes out of band (`?` `!` `~` 0x18, or >= 0x80) or a non-ASCII byte inside a line.
    RealtimeByteInLine { line: usize, byte: u8 },
    /// LinuxCNC runs a loaded program; it has no streaming protocol here.
    DialectNotStreamed { dialect: Dialect },
    /// The request has no realtime byte in this dialect (Marlin has none).
    NotInDialect { dialect: Dialect, what: &'static str },
    /// make_core's writer refused the program.
    Program(Refusal),
    /// The link went away while sending a realtime byte.
    Link,
    /// VUGCOMSCANRECONNECT (SR521): the link was lost at `at_line` (the oldest unacknowledged text line); the pump
    /// resumes only after a person re-arms it ([`Pump::resume_at`]), never on a bare Resume.
    ResumeAfterLinkLost { at_line: usize },
    /// VUGCOMSCANRECONNECT (SR521): [`Pump::resume_at`] named a text line the pump does not send.
    NoSuchLine { line: usize },
}

/// Why a stream stopped sending before its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    Error { line: usize, code: u16 },
    Alarm { code: u16 },
    /// The controller said its welcome mid-stream: its buffer is gone.
    Reset,
    /// Marlin asked for a line again (VUG-COMSCAN-MARLIN-RESEND owed).
    Resend { line: usize },
    /// Marlin said `Error:`.
    Refused { line: usize },
    /// No longer entered: a lost link is [`PumpState::LinkLost`] (VUGCOMSCANRECONNECT, SR521).
    Link,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpState {
    Streaming,
    Halted(Halt),
    /// A feed hold is out; the soft reset follows the hold's completion.
    Stopping,
    Stopped,
    Done,
    /// VUGCOMSCANRECONNECT (SR521): the link went away mid-stream. `at_line` is the 1-based text line of the oldest
    /// line the controller never acknowledged; `program_digest` the `Program::digest` this pump streams (0 for a pump
    /// made from text by [`Pump::new`]). Nothing is sent until a person re-arms ([`Pump::resume_at`]).
    LinkLost { at_line: usize, program_digest: u64 },
    /// VUGCOMSCANRECONNECT (SR521): a fresh pump positioned at `at_line` by [`Pump::resume_at`]; [`Pump::resume`]
    /// starts it streaming from there.
    Rearmed { at_line: usize },
}

/// What the controller said, and what the pump did about it. `line` is the 1-based line of the program text.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Ok(usize),
    Error { line: usize, code: u16, name: &'static str },
    Alarm { code: u16, name: &'static str },
    Status(StatusReport),
    /// A `[...]` push, a welcome before streaming, Marlin's `echo:`, or a status the parser refused: shown, not obeyed.
    Message(String),
    Reset,
    ResendAsked { line: usize },
    Refused { line: usize, text: String },
    Timeout,
    Stopped,
    Done,
    LinkLost,
    /// VUGCOMSCANMARLINAUTOREPORT (SR625): Marlin's answer to the link-up `M154 S<n>` ([`Pump::autoreport`]): `on` (an `ok`),
    /// else the build lacks AUTO_REPORT_POSITION (`echo:Unknown command`, then its `ok`). Said once per pump.
    AutoReport { on: bool },
    /// VUGCOMSCANAUTOREPORTCAPOFF (SR634): the hello carried no `Cap:AUTOREPORT_POS:1` ([`Pump::autoreport_hello`]): the
    /// M154 is never asked. Said once per pump, on its first tick.
    AutoReportAbsent,
}

/// Lines sent (started) / acknowledged / total, the state and the last status report.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub sent: usize,
    pub acked: usize,
    pub total: usize,
    pub state: PumpState,
    pub last: Option<StatusReport>,
}

pub struct Pump {
    dialect: Dialect,
    lines: Vec<String>,
    src: Vec<usize>,
    next: usize,
    tx: Vec<u8>,
    rt_pending: Vec<u8>,
    /// (line index, bytes with its `\n`) of every line sent and not yet answered, oldest first.
    outstanding: VecDeque<(usize, usize)>,
    acked: usize,
    state: PumpState,
    rx: Vec<u8>,
    started: bool,
    last_rx_us: u64,
    last_query_us: Option<u64>,
    timed_out: bool,
    last: Option<StatusReport>,
    status_every_us: u64,
    timeout_us: u64,
    /// VUGCOMSCANRECONNECT (SR521): the `Program::digest` streamed (0 for a pump made from text).
    digest: u64, status_asked: bool, autoreport: AutoAsk, // VUGCOMSCANMARLINPOSITION (SR620): status_asked: a Marlin `M114` is out and its `ok` not yet read ([`Pump::status`]); VUGCOMSCANMARLINAUTOREPORT (SR625): autoreport: the link-up M154's place
}

fn is_grbl(d: Dialect) -> bool {
    matches!(d, Dialect::Grbl { .. })
}

impl Pump {
    /// A pump over `text` in dialect `d`: comments (`;` to the end, `( ... )`) and blank lines dropped, each line
    /// trimmed, every line checked before the first byte is sent.
    pub fn new(d: Dialect, text: &str) -> Result<Pump, PumpRefusal> {
        if d == Dialect::LinuxCnc {
            return Err(PumpRefusal::DialectNotStreamed { dialect: d });
        }
        let grbl = is_grbl(d);
        let mut lines = Vec::new();
        let mut src = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let mut l = String::new();
            let mut depth = 0u32;
            for c in raw.chars() {
                match c {
                    '(' => depth += 1,
                    ')' if depth > 0 => depth -= 1,
                    ';' if depth == 0 => break,
                    _ if depth == 0 => l.push(c),
                    _ => {}
                }
            }
            let l = l.trim();
            if l.is_empty() {
                continue;
            }
            for b in l.bytes() {
                let rt = matches!(b, b'?' | b'!' | b'~' | 0x18);
                if b >= 0x80 || (grbl && rt) {
                    return Err(PumpRefusal::RealtimeByteInLine { line: i + 1, byte: b });
                }
            }
            if grbl && l.len() + 1 > GRBL_RX {
                return Err(PumpRefusal::LineTooLong { line: i + 1, len: l.len() + 1 });
            }
            lines.push(String::from(l));
            src.push(i + 1);
        }
        if lines.is_empty() {
            return Err(PumpRefusal::Empty);
        }
        Ok(Pump {
            dialect: d,
            lines,
            src,
            next: 0,
            tx: Vec::new(),
            rt_pending: Vec::new(),
            outstanding: VecDeque::new(),
            acked: 0,
            state: PumpState::Streaming,
            rx: Vec::new(),
            started: false,
            last_rx_us: 0,
            last_query_us: None,
            timed_out: false,
            last: None,
            status_every_us: STATUS_EVERY_US,
            timeout_us: TIMEOUT_US,
            digest: 0, status_asked: false, autoreport: AutoAsk::Off, // VUGCOMSCANMARLINPOSITION (SR620); VUGCOMSCANMARLINAUTOREPORT (SR625)
        })
    }

    /// make_core's text for `p` in `d` (the one writer), then [`Pump::new`].
    pub fn from_program(p: &Program, d: Dialect) -> Result<Pump, PumpRefusal> {
        let text = write(p, d).map_err(PumpRefusal::Program)?;
        let mut pump = Pump::new(d, &text)?;
        pump.digest = p.digest();
        Ok(pump)
    }

    pub fn with_timeout(mut self, us: u64) -> Pump {
        self.timeout_us = us;
        self
    }

    /// The status query period; 0 never queries on its own.
    pub fn with_status_every(mut self, us: u64) -> Pump {
        self.status_every_us = us;
        self
    }

    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    pub fn state(&self) -> PumpState {
        self.state
    }

    /// The lines the pump will send, after comments are dropped.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn progress(&self) -> Progress {
        Progress {
            sent: self.next,
            acked: self.acked,
            total: self.lines.len(),
            state: self.state,
            last: self.last.clone(),
        }
    }

    /// Bytes of unanswered lines the controller holds (or is being sent).
    pub fn in_flight(&self) -> usize {
        self.outstanding.iter().map(|(_, n)| *n).sum()
    }

    fn fits(&self, len: usize) -> bool {
        if is_grbl(self.dialect) {
            self.in_flight() + len <= GRBL_RX
        } else {
            self.outstanding.is_empty()
        }
    }

    fn rt(&mut self, t: &mut dyn Transport, r: Realtime) -> Result<(), PumpRefusal> {
        self.rt_pending.push(r.byte());
        self.flush_rt(t)
    }

    fn flush_rt(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        while !self.rt_pending.is_empty() {
            match t.write(&self.rt_pending) {
                Ok(0) => break,
                Ok(n) => {
                    self.rt_pending.drain(..n);
                }
                Err(_) => return Err(PumpRefusal::Link),
            }
        }
        Ok(())
    }

    fn grbl_only(&self, what: &'static str) -> Result<(), PumpRefusal> {
        if is_grbl(self.dialect) {
            Ok(())
        } else {
            Err(PumpRefusal::NotInDialect { dialect: self.dialect, what })
        }
    }

    /// grbl `!`: feed hold, sent at once.
    pub fn hold(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        self.grbl_only("feed hold")?;
        self.rt(t, Realtime::FeedHold)
    }

    /// grbl `~`: cycle start / resume. A halted stream stays halted: the controller resumes what it holds, the pump
    /// sends nothing more.
    pub fn resume(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        if let Some(r) = self.resume_lost() {
            return r;
        }
        self.grbl_only("cycle start")?;
        self.rt(t, Realtime::CycleStart)
    }

    /// grbl `?`: one status report, answered as [`Event::Status`]. VUGCOMSCANMARLINPOSITION (SR620): Marlin `M114`, a line answered by
    /// the position and `ok`, read into the same [`Event::Status`] (`Pump::marlin_status`); LinuxCNC is refused by name.
    pub fn status(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        if self.dialect == Dialect::Marlin { return self.marlin_status(t); } self.grbl_only("status query")?; // VUGCOMSCANMARLINPOSITION (SR620): Marlin asks M114 as a line
        self.rt(t, Realtime::StatusQuery)
    }

    /// grbl 0x18: soft reset now. The controller drops its buffer, so the pump drops its window and stops; the
    /// remainder is never sent. In motion this costs the position (alarm 3): [`Pump::stop`] is the gentle road.
    pub fn reset(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        self.grbl_only("soft reset")?;
        self.tx.clear();
        self.outstanding.clear();
        self.state = PumpState::Stopped;
        self.rt(t, Realtime::SoftReset)
    }

    /// Stop the stream. grbl: a feed hold now, the soft reset once a report shows `Hold:0` or `Idle` (polled while
    /// stopping). Marlin has no realtime byte: the pump stops sending, and the few lines Marlin holds finish.
    pub fn stop(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        if matches!(self.state, PumpState::Stopped | PumpState::Done) {
            return Ok(());
        }
        if matches!(self.state, PumpState::Halted(Halt::Alarm { .. })) && is_grbl(self.dialect) { return self.reset(t); } // VUGCOMSCANALARMRESET (SR585): an alarm reports `Alarm`, never Idle / Hold:0, and grbl hears only the soft reset; VUGCOMSCANALARMIDLE (SR591): grbl only, a Marlin alarm halt takes the Marlin arm below (no reset byte)
        if matches!(self.state, PumpState::LinkLost { .. } | PumpState::Rearmed { .. }) {
            self.state = PumpState::Stopped;
            return Ok(());
        }
        if !is_grbl(self.dialect) {
            self.state = PumpState::Stopped;
            return Ok(());
        }
        self.state = PumpState::Stopping;
        self.last_query_us = None;
        self.rt(t, Realtime::FeedHold)
    }

    fn halt(&mut self, h: Halt, t: &mut dyn Transport) {
        let live = matches!(self.state, PumpState::Streaming | PumpState::Stopping); // VUGCOMSCANJOGALARM (SR579): the error feed hold's reason (buffered lines running without the skipped one) exists only on a live stream
        if live || matches!(h, Halt::Alarm { .. }) { // VUGCOMSCANALARMIDLE (SR591): an alarm halts in every state the link reaches (Done, Stopped, Halted, relinked, Rearmed)
            self.state = PumpState::Halted(h);
        }
        self.tx.clear();
        if is_grbl(self.dialect) && matches!(h, Halt::Error { .. }) && live {
            let _ = self.rt(t, Realtime::FeedHold);
        }
    }

    fn pop(&mut self) -> Option<usize> {
        let (i, _) = self.outstanding.pop_front()?;
        Some(self.src[i])
    }

    fn front_line(&self) -> usize {
        self.outstanding.front().map(|(i, _)| self.src[*i]).unwrap_or(0)
    }

    fn welcome(&mut self, s: &str, t: &mut dyn Transport, ev: &mut Vec<Event>) {
        match self.state {
            PumpState::Streaming if self.next > 0 => {
                self.outstanding.clear();
                self.halt(Halt::Reset, t);
                ev.push(Event::Reset);
            }
            PumpState::Stopping => {
                self.outstanding.clear();
                self.state = PumpState::Stopped;
                ev.push(Event::Stopped);
            }
            _ => ev.push(Event::Message(String::from(s))),
        }
    }

    fn classify(&mut self, s: &str, t: &mut dyn Transport, ev: &mut Vec<Event>) {
        if is_grbl(self.dialect) {
            if s == "ok" {
                match self.pop() {
                    Some(line) => {
                        self.acked += 1;
                        ev.push(Event::Ok(line));
                    }
                    None => ev.push(Event::Message(String::from(s))),
                }
            } else if let Some(n) = s.strip_prefix("error:") {
                let code = n.trim().parse::<u16>().unwrap_or(0);
                let line = self.pop().unwrap_or(0);
                ev.push(Event::Error { line, code, name: error_name(code) });
                self.halt(Halt::Error { line, code }, t);
            } else if let Some(n) = s.strip_prefix("ALARM:") {
                let code = n.trim().parse::<u16>().unwrap_or(0);
                self.alarmed(code, alarm_name(code), t, ev); // VUGCOMSCANALARMIDLE (SR591): the one alarm road (grbl ALARM:N, Marlin's halt)
            } else if s.starts_with('<') {
                match parse_status(s) {
                    Ok(r) => {
                        let settled = matches!(r.state, State::Idle | State::Hold(0));
                        self.last = Some(r.clone());
                        ev.push(Event::Status(r));
                        if self.state == PumpState::Stopping && settled {
                            self.tx.clear();
                            self.outstanding.clear();
                            self.state = PumpState::Stopped;
                            let _ = self.rt(t, Realtime::SoftReset);
                            ev.push(Event::Stopped);
                        }
                    }
                    Err(_) => ev.push(Event::Message(String::from(s))),
                }
            } else if s.starts_with("Grbl ") {
                self.welcome(s, t, ev);
            } else {
                ev.push(Event::Message(String::from(s)));
            }
        } else if self.marlin_status_line(s, ev) {} else if s.starts_with("ok") { // VUGCOMSCANMARLINPOSITION (SR620): the M114 position line and its own `ok` first
            match self.pop() {
                Some(line) => {
                    self.acked += 1;
                    ev.push(Event::Ok(line));
                }
                None => ev.push(Event::Message(String::from(s))),
            }
        } else if let Some(n) = s.strip_prefix("Resend:").or_else(|| s.strip_prefix("rs ")) {
            let line = n.trim().parse::<usize>().unwrap_or(0);
            ev.push(Event::ResendAsked { line });
            self.halt(Halt::Resend { line }, t);
        } else if marlin_halted(s) {
            self.alarmed(0, MARLIN_HALTED, t, ev); // VUGCOMSCANALARMIDLE (SR591): Marlin's kill() / stop() is its alarm
        } else if s.starts_with("Error:") {
            let line = self.front_line();
            ev.push(Event::Refused { line, text: String::from(s) });
            self.halt(Halt::Refused { line }, t);
        } else if s == "start" {
            self.welcome(s, t, ev);
        } else {
            ev.push(Event::Message(String::from(s)));
        }
    }

    /// One turn: read and classify what arrived, send what the window allows, query status on the period, and say
    /// a timeout or the end once. `now_us` is the caller's clock (the core has none).
    pub fn tick(&mut self, t: &mut dyn Transport, now_us: u64) -> Vec<Event> {
        let mut ev = Vec::new();
        if !self.started {
            self.started = true; self.autoreport_due(&mut ev); // VUGCOMSCANMARLINAUTOREPORT (SR625): the armed M154, once, ahead of the first line; VUGCOMSCANAUTOREPORTCAPOFF (SR634): or `absent` said once
            self.last_rx_us = now_us;
        }
        let mut buf = [0u8; 256];
        let mut lost = false;
        loop {
            match t.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => self.rx.extend_from_slice(&buf[..n]),
                Err(_) => {
                    lost = true; // the bytes already read are classified first (an `ok` that came with the FIN counts)
                    break;
                }
            }
        }
        while let Some(pos) = self.rx.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.rx.drain(..=pos).collect();
            let s = String::from_utf8_lossy(&raw);
            let s = s.trim();
            if s.is_empty() {
                continue;
            }
            let s = String::from(s);
            self.last_rx_us = now_us;
            self.timed_out = false;
            self.classify(&s, t, &mut ev);
        }
        // A controller that never sends a newline is not a controller; bound what it can make us hold.
        if self.rx.len() > 4096 {
            self.rx.clear();
        }
        if lost || self.flush_rt(t).is_err() {
            self.lose_link();
            ev.push(Event::LinkLost);
            return ev;
        }
        if self.state == PumpState::Streaming {
            loop {
                if !self.tx.is_empty() {
                    match t.write(&self.tx) {
                        Ok(n) => {
                            self.tx.drain(..n);
                        }
                        Err(_) => {
                            self.lose_link();
                            ev.push(Event::LinkLost);
                            return ev;
                        }
                    }
                    if !self.tx.is_empty() {
                        break;
                    }
                }
                if self.next >= self.lines.len() {
                    break;
                }
                let len = self.lines[self.next].len() + 1;
                if !self.fits(len) {
                    break;
                }
                self.tx.extend_from_slice(self.lines[self.next].as_bytes());
                self.tx.push(b'\n');
                self.outstanding.push_back((self.next, len));
                self.next += 1;
            }
        }
        let live = matches!(self.state, PumpState::Streaming | PumpState::Stopping);
        if live && is_grbl(self.dialect) && (self.status_every_us > 0 || self.state == PumpState::Stopping) {
            let every = if self.status_every_us > 0 { self.status_every_us } else { STATUS_EVERY_US };
            let due = match self.last_query_us {
                None => true,
                Some(t0) => now_us.saturating_sub(t0) >= every,
            };
            if due {
                self.last_query_us = Some(now_us);
                if self.rt(t, Realtime::StatusQuery).is_err() {
                    self.lose_link();
                    ev.push(Event::LinkLost);
                    return ev;
                }
            }
        }
        if live && !self.outstanding.is_empty() && !self.timed_out && now_us.saturating_sub(self.last_rx_us) >= self.timeout_us {
            self.timed_out = true;
            ev.push(Event::Timeout);
        }
        if self.state == PumpState::Streaming && self.next == self.lines.len() && self.outstanding.is_empty() && self.tx.is_empty() {
            self.state = PumpState::Done;
            ev.push(Event::Done);
        }
        ev
    }
}

// --- VUGCOMSCANRECONNECT (LEDGER SR521): the lost link, the re-arm and the redial backoff (named tail hunk) ---

/// The first redial waits this long after the loss.
pub const REDIAL_FIRST_US: u64 = 250_000;
/// The redial wait doubles up to this.
pub const REDIAL_MAX_US: u64 = 4_000_000;

/// The wait before redial `attempt` (1-based): 250 ms, doubling, capped at 4 s.
pub fn backoff_us(attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(16);
    (REDIAL_FIRST_US << shift).min(REDIAL_MAX_US)
}

/// The redial schedule for a lost link: which attempt is next and when it is due. The caller owns the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Redial {
    attempt: u32,
    due_us: u64,
}

impl Redial {
    /// Attempt 1, due [`REDIAL_FIRST_US`] after the loss at `lost_us`.
    pub fn new(lost_us: u64) -> Redial {
        Redial { attempt: 1, due_us: lost_us.saturating_add(backoff_us(1)) }
    }

    /// True once the next attempt is due.
    pub fn due(&self, now_us: u64) -> bool {
        now_us >= self.due_us
    }

    /// The next attempt's number (1-based).
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The wait before the next attempt.
    pub fn wait_us(&self) -> u64 {
        backoff_us(self.attempt)
    }

    /// The attempt failed at `now_us`: the next one is due after the doubled wait.
    pub fn failed(&mut self, now_us: u64) {
        self.attempt = self.attempt.saturating_add(1);
        self.due_us = now_us.saturating_add(backoff_us(self.attempt));
    }
}

impl Pump {
    /// The `Program::digest` this pump streams (0 for a pump made from text).
    pub fn digest(&self) -> u64 {
        self.digest
    }

    /// The 1-based text line of the pump's `index`-th line.
    pub fn line_of(&self, index: usize) -> Option<usize> {
        self.src.get(index).copied()
    }

    /// A FRESH pump (the one a re-arm made) positioned at text line `at_line`: the lines before it count as sent and
    /// acknowledged, and the state is [`PumpState::Rearmed`] until [`Pump::resume`].
    pub fn resume_at(mut self, at_line: usize) -> Result<Pump, PumpRefusal> {
        let Some(i) = self.src.iter().position(|&l| l == at_line) else {
            return Err(PumpRefusal::NoSuchLine { line: at_line });
        };
        self.modal_prefix(i, at_line)?; self.next = i; // VUGCOMSCANRESUMEMODAL (SR656): the modal words the lines before at_line set go first, numbered at_line
        self.acked = i;
        self.state = PumpState::Rearmed { at_line };
        Ok(self)
    }

    /// The link went away: a live stream is [`PumpState::LinkLost`] at its oldest unacknowledged line (a Rearmed
    /// pump keeps its line); nothing more is sent and no feed hold is tried (the link is gone).
    fn lose_link(&mut self) {
        let at_line = match self.state {
            PumpState::Rearmed { at_line } => Some(at_line),
            PumpState::Streaming | PumpState::Stopping => {
                let i = self.outstanding.front().map(|(i, _)| *i).unwrap_or(self.next);
                Some(self.src.get(i).copied().unwrap_or_else(|| self.src.last().copied().unwrap_or(0)))
            }
            _ => None,
        };
        if let Some(at_line) = at_line {
            self.state = PumpState::LinkLost { at_line, program_digest: self.digest };
        }
        self.tx.clear();
        self.rt_pending.clear();
        self.outstanding.clear();
        self.rx.clear(); self.status_asked = false; self.autoreport_lost(); // VUGCOMSCANMARLINPOSITION (SR620): a lost link answers no M114; VUGCOMSCANMARLINAUTOREPORT (SR625): nor an M154
    }

    /// Resume after a loss: refused by name while lost (no byte); a re-armed pump starts streaming (no `~` byte).
    fn resume_lost(&mut self) -> Option<Result<(), PumpRefusal>> {
        match self.state {
            PumpState::LinkLost { at_line, .. } => Some(Err(PumpRefusal::ResumeAfterLinkLost { at_line })),
            PumpState::Rearmed { .. } => {
                self.state = PumpState::Streaming;
                self.started = false;
                self.timed_out = false;
                self.last_query_us = None;
                Some(Ok(()))
            }
            _ => None,
        }
    }
}

// --- VUGCOMSCANHELLOID (LEDGER SR546): the controller's own identity verb and its answer grammar (named tail hunk) ---

/// The verb a dialect answers with its own identity, sent once after the banner: grbl 1.1 `$I` (the build info:
/// `[VER:<version>.<date>:<user string>]` and `[OPT:<flags>,<blocks>,<rx>]`, then `ok`), Marlin / Klipper `M115`
/// (`FIRMWARE_NAME:...` then the `Cap:` lines and `ok`); LinuxCNC is not streamed and has none.
pub fn identity_verb(d: Dialect) -> Option<&'static str> {
    match d {
        Dialect::Grbl { .. } => Some("$I"),
        Dialect::Marlin => Some("M115"),
        Dialect::LinuxCnc => None,
    }
}

/// One line of the answer to [`identity_verb`], classified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelloLine {
    /// Part of the identity: kept.
    Keep,
    /// The answer is complete (`ok`).
    End,
    /// The controller refused the verb (`error:N`, Marlin's `Error:` or unknown command).
    Refused,
    /// Anything else (a push message, a `Cap:` line, an echo): not the identity. VUGCOMSCANAUTOREPORTCAPOFF (SR634): Marlin's
    /// one [`AUTOREPORT_CAP`] line is kept, so the hello carries the position auto-report capability.
    Other,
}

/// Classify one trimmed line of the identity answer in dialect `d`.
pub fn hello_line(d: Dialect, s: &str) -> HelloLine {
    match d {
        Dialect::Grbl { .. } => {
            if s.starts_with("[VER:") || s.starts_with("[OPT:") {
                HelloLine::Keep
            } else if s == "ok" {
                HelloLine::End
            } else if s.starts_with("error:") {
                HelloLine::Refused
            } else {
                HelloLine::Other
            }
        }
        Dialect::Marlin => {
            if s.contains("FIRMWARE_NAME:") || s == AUTOREPORT_CAP { // VUGCOMSCANAUTOREPORTCAPOFF (SR634): the one Cap line kept
                HelloLine::Keep
            } else if s.starts_with("ok") {
                HelloLine::End
            } else if s.starts_with("Error:") || s.starts_with("echo:Unknown command") {
                HelloLine::Refused
            } else {
                HelloLine::Other
            }
        }
        Dialect::LinuxCnc => HelloLine::Other,
    }
}

// --- VUGCOMSCANHOSTDESK (LEDGER SR561): the controller's commissioned name, written once and read back (named tail hunk) ---

/// The verb that commissions a controller's own name, sent once at the first bind of an unnamed board: grbl 1.1
/// `$I=<name>` (system.c `settings_store_build_info`, answered `ok`; the line intake upper-cases a-z and drops bytes
/// <= ' ', so a name is `[A-Z0-9-]`); Marlin's `M115` has no writable name and LinuxCNC no verb: `None`.
pub fn name_verb(d: Dialect, name: &str) -> Option<String> {
    match d {
        Dialect::Grbl { .. } => Some(alloc::format!("$I={name}")),
        Dialect::Marlin | Dialect::LinuxCnc => None,
    }
}

/// The commissioned name inside a kept identity answer (`said`, the hello's kept lines joined by one space): grbl's
/// `[VER:<version>.<date>:<user string>]` user string, `Some("")` for an uncommissioned board; `None` with no VER line
/// or in a dialect without a name.
pub fn hello_name(d: Dialect, said: &str) -> Option<&str> {
    match d {
        Dialect::Grbl { .. } => {
            let at = said.find("[VER:")? + "[VER:".len();
            let inner = &said[at..];
            let inner = &inner[..inner.find(']')?];
            Some(inner.split_once(':').map(|(_, name)| name).unwrap_or(""))
        }
        Dialect::Marlin | Dialect::LinuxCnc => None,
    }
}

// --- VUGCOMSCANJOG (LEDGER SR573): the controller's home and jog lines per dialect (named tail hunk) ---

/// The lines that home the machine, a controller action outside the program (make_core refuses grbl homing in a
/// `Program`): grbl 1.1 `$H` (the homing cycle, answered `ok` when it is over; the way out of an alarm), Marlin `G28`;
/// LinuxCNC is not streamed: `None`.
pub fn home_lines(d: Dialect) -> Option<Vec<String>> {
    match d {
        Dialect::Grbl { .. } => Some(alloc::vec![String::from("$H")]),
        Dialect::Marlin => Some(alloc::vec![String::from("G28")]),
        Dialect::LinuxCnc => None,
    }
}

/// The lines that jog by `delta` (mm, relative) at `feed` (mm/min), the numbers in make_core's one grammar
/// (`make_core::gcode::num`): grbl 1.1 `$J=G91 X.. Y.. Z.. F..` (one jog line; the modal state is untouched), Marlin
/// `G91`, `G0 X.. Y.. Z.. F..`, then the run's distance word (relative for the one move, the run's mode again after);
/// LinuxCNC: `None`. VUGCOMSCANMARLINJOGMODAL (SR664): `run` is the modal state the run left on the board
/// ([`Pump::sent_modal`]; `Modal::default()` with no run): Marlin closes with its distance word (`G90`, the board's
/// default, when none is recorded), and under a `G20` run the jog still says mm: grbl's jog-local `$J=G21 G91 ..`,
/// Marlin `G21` before and `G20` after.
pub fn jog_lines(d: Dialect, delta: [f64; 3], feed: f64, run: Modal) -> Option<Vec<String>> {
    let mut mv = String::new();
    for (c, v) in [('X', delta[0]), ('Y', delta[1]), ('Z', delta[2]), ('F', feed)] {
        mv.push(' ');
        mv.push(c);
        make_core::gcode::num(&mut mv, v);
    }
    let inch = run.units() == Some("G20"); // VUGCOMSCANMARLINJOGMODAL (SR664): VUG-COMSCAN-JOG-UNITS
    match d {
        Dialect::Grbl { .. } => Some(alloc::vec![alloc::format!("$J={}G91{mv}", if inch { "G21 " } else { "" })]),
        Dialect::Marlin => {
            let mut v = Vec::new();
            if inch {
                v.push(String::from("G21"));
            }
            v.push(String::from("G91"));
            v.push(alloc::format!("G0{mv}"));
            v.push(String::from(run.distance().unwrap_or("G90")));
            if inch {
                v.push(String::from("G20"));
            }
            Some(v)
        }
        Dialect::LinuxCnc => None,
    }
}

// --- VUGCOMSCANJOGFACE (LEDGER SR579): grbl's jog cancel on the realtime road (named tail hunk) ---

impl Pump {
    /// grbl 0x85: jog cancel, sent at once on the realtime road (as the feed hold is). grbl decelerates the running jog
    /// and drops the jogs it holds; with no jog running it ignores the byte. Marlin has no realtime byte: refused by name.
    pub fn jog_cancel(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        self.grbl_only("jog cancel")?;
        self.rt(t, Realtime::JogCancel)
    }
}

// --- VUGCOMSCANALARMRESET (LEDGER SR585): the welcome banner's key and the unlock line per dialect (named tail hunk) ---

/// The start of the line a controller says when it comes up (after a power-on, a reset-on-open or a soft reset): grbl
/// `Grbl ` (`Grbl 1.1h ['$' for help]`), Marlin `start`; LinuxCNC is not streamed: `None`.
pub fn banner_key(d: Dialect) -> Option<&'static str> {
    match d {
        Dialect::Grbl { .. } => Some("Grbl "),
        Dialect::Marlin => Some("start"),
        Dialect::LinuxCnc => None,
    }
}

/// The line that clears a controller's alarm lock after the soft reset: grbl 1.1 `$X` (the kill-alarm-lock, answered
/// `[MSG:Caution: Unlocked]` and `ok`); VUGCOMSCANALARMIDLE (SR591): Marlin `M999 S1` (clears `stop()`'s "use M999 to
/// restart" with no reboot; `S1` resumes without the flush whose `Resend:` the pump would read as a resend ask; a kill()
/// needs a hardware reset); LinuxCNC is not streamed: `None`.
pub fn unlock_line(d: Dialect) -> Option<&'static str> {
    match d {
        Dialect::Grbl { .. } => Some("$X"),
        Dialect::Marlin => Some("M999 S1"),
        Dialect::LinuxCnc => None,
    }
}

// --- VUGCOMSCANALARMIDLE (LEDGER SR591): one alarm road in every linked state, Marlin's halt and its M999, grbl's travel settings (named tail hunk) ---

impl Pump {
    /// The one alarm road: said as [`Event::Alarm`], the window dropped (VUGCOMSCANJOGALARM SR579: grbl flushes its receive
    /// buffer at every alarm and Marlin's halt flushes its queue, so no line of the window is answered after it), and
    /// [`PumpState::Halted`] by [`Halt::Alarm`] whatever the state (a hard limit during a jog halts a Done pump).
    fn alarmed(&mut self, code: u16, name: &'static str, t: &mut dyn Transport, ev: &mut Vec<Event>) {
        ev.push(Event::Alarm { code, name });
        self.outstanding.clear();
        self.halt(Halt::Alarm { code }, t);
    }
}

/// The name a Marlin halt is said by (`alarm:0 printer halted`): Marlin numbers no alarm.
pub const MARLIN_HALTED: &str = "printer halted";

/// A Marlin line that halts the machine: `Error:` carrying `kill() called` (STR_ERR_KILLED, "Printer halted. kill()
/// called!") or `M999` (STR_ERR_STOPPED, "Printer stopped due to errors. Fix the error and use M999 to restart.").
pub fn marlin_halted(s: &str) -> bool {
    s.starts_with("Error:") && (s.contains("kill() called") || s.contains("M999"))
}

/// The verb that reports the controller's own travel, sent once after the hello when the read is on: grbl 1.1 `$$` (every
/// setting, one `$<n>=<value>` line each, then `ok`); Marlin `M211` with no argument (VUGCOMSCANENVELOPEREADON SR597: the
/// soft endstops report, `Min:` and `Max:` on one line, then `ok`; compiled out it is an unknown command, a refusal);
/// LinuxCNC is not streamed: `None`.
pub fn envelope_verb(d: Dialect) -> Option<&'static str> {
    match d {
        Dialect::Grbl { .. } => Some("$$"),
        Dialect::Marlin => Some("M211"),
        Dialect::LinuxCnc => None,
    }
}

/// One line of the [`envelope_verb`] answer read for the travel only, X / Y / Z in mm (`None` for an axis the line does not
/// carry): grbl `$130=` / `$131=` / `$132=` (X / Y / Z max travel) fill one slot; VUGCOMSCANENVELOPEREADON (SR597): Marlin's
/// soft endstops line (`echo:  Min:  X0.00 Y0.00 Z0.00   Max:  X200.00 Y200.00 Z200.00`) gives `Max - Min` per axis both
/// sides carry (a 2-axis machine's line leaves Z `None`). A value that is not a finite number, every other line, or
/// another dialect: all `None`.
pub fn envelope_setting(d: Dialect, s: &str) -> [Option<f64>; 3] {
    let mut out = [None; 3];
    match d {
        Dialect::Grbl { .. } => {
            let Some((k, v)) = s.strip_prefix("$13").and_then(|r| r.split_once('=')) else { return out };
            let i = match k {
                "0" => 0,
                "1" => 1,
                "2" => 2,
                _ => return out,
            };
            out[i] = v.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        }
        Dialect::Marlin => {
            let (Some(lo), Some(hi)) = (s.find("Min:"), s.find("Max:")) else { return out };
            if lo >= hi {
                return out;
            }
            let (min, max) = (marlin_xyz(&s[lo + 4..hi]), marlin_xyz(&s[hi + 4..]));
            for (o, (a, b)) in out.iter_mut().zip(min.into_iter().zip(max)) {
                *o = a.zip(b).map(|(a, b)| b - a);
            }
        }
        Dialect::LinuxCnc => {}
    }
    out
}

// VUGCOMSCANENVELOPEREADON (SR597): Marlin's soft endstops read through the one envelope_setting

/// The `X<n>` / `Y<n>` / `Z<n>` words of one side of Marlin's soft endstops line (`print_pos`: ` X0.00 Y0.00 Z0.00`), finite only.
fn marlin_xyz(seg: &str) -> [Option<f64>; 3] {
    let mut out = [None; 3];
    for w in seg.split_whitespace() {
        let mut c = w.chars();
        let i = match c.next() {
            Some('X') => 0,
            Some('Y') => 1,
            Some('Z') => 2,
            _ => continue,
        };
        if let Some(v) = c.as_str().strip_prefix(':').unwrap_or(c.as_str()).parse::<f64>().ok().filter(|v| v.is_finite()) { // VUGCOMSCANMARLINPOSITION (SR620): + M114's `X:<n>`
            out[i] = Some(v);
        }
    }
    out
}

// --- VUGCOMSCANMARLINPOSITION (LEDGER SR620): Marlin's M114 as the status road (named tail hunk) ---

/// The position in one Marlin `M114` answer line (`report_current_position`: `X:0.00 Y:0.00 Z:0.00 E:0.00 Count X:0 Y:0 Z:0`):
/// the FIRST triple, the logical position (where the last planned move ends; after a jog's `ok`s, the jog's end), read
/// through the one word reader [`marlin_xyz`]; the `Count` stepper triple after it is not read. `None`: not a position
/// line, or X / Y / Z not each a finite number.
pub fn marlin_position(s: &str) -> Option<[f64; 3]> {
    if !s.starts_with("X:") {
        return None;
    }
    let logical = s.split(" Count").next().unwrap_or(s);
    let [Some(x), Some(y), Some(z)] = marlin_xyz(logical) else { return None };
    Some([x, y, z])
}

impl Pump {
    /// Marlin's status query: `M114` (no realtime byte: a line, answered by the position line and `ok`), written at once on
    /// the realtime road. Refused by name while a line of a job is live or unanswered (its `ok` would answer the job's
    /// line) and while halted by an alarm (a halted Marlin answers nothing until `M999`, whose `ok` an M114 left out would
    /// take); at most one out at a time (a second ask while one is out writes nothing).
    fn marlin_status(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        let halted = matches!(self.state, PumpState::Halted(Halt::Alarm { .. }));
        if halted || matches!(self.state, PumpState::Streaming | PumpState::Stopping) || !self.outstanding.is_empty() {
            return Err(PumpRefusal::NotInDialect { dialect: self.dialect, what: "status query while a line is unanswered" });
        }
        if self.status_asked {
            return Ok(());
        }
        self.status_asked = true;
        self.rt_pending.extend_from_slice(b"M114\n");
        self.flush_rt(t)
    }

    /// One Marlin line read for the M114 road, ahead of `classify`'s `ok` arm: a position line ([`marlin_position`])
    /// is [`Event::Status`] (`Idle` with no line of a job live or unanswered, Marlin's idle having no word of its own; else
    /// `Run`), the position as `mpos`; an `ok` while an `M114` is out is that M114's own and is swallowed (it never reaches a
    /// job's line or the caller's home / jog lines). `start` (the buffer is gone) and a halt forget the M114 and go on to
    /// their own roads. `true`: the line was taken here.
    fn marlin_status_line(&mut self, s: &str, ev: &mut Vec<Event>) -> bool {
        if let Some(at) = marlin_position(s) {
            let live = matches!(self.state, PumpState::Streaming | PumpState::Stopping) || !self.outstanding.is_empty();
            let r = StatusReport {
                state: if live { State::Run } else { State::Idle },
                mpos: Some(at),
                wpos: None,
                wco: None,
                buf: None,
                line: None,
                feed: None,
                spindle: None,
                ov: None,
                pins: String::new(),
                accessories: String::new(),
            };
            self.last = Some(r.clone());
            ev.push(Event::Status(r));
            return true;
        }
        if self.autoreport_line(s, ev) { return true; } // VUGCOMSCANMARLINAUTOREPORT (SR625): the M154's own answer, sent first, read first
        if s.starts_with("ok") && self.status_asked {
            self.status_asked = false;
            return true;
        }
        if s == "start" || marlin_halted(s) {
            self.status_asked = false;
        }
        false
    }
}

// --- VUGCOMSCANMARLINAUTOREPORT (LEDGER SR625): Marlin's position auto-report asked once at the link-up (named tail hunk) ---

/// The period, in seconds, of the position auto-report the link-up asks Marlin for (`M154 S1`).
pub const AUTOREPORT_S: u8 = 1;

/// Where a pump's one `M154 S<n>` stands: not asked, armed for the first tick, out (and whether Marlin called it an
/// unknown command), answered. VUGCOMSCANAUTOREPORTCAPOFF (SR634): `Absent` (the hello carried no cap: never asked,
/// `absent` said on the first tick); `Answered { on }` (the report runs: the desk's close turns it off).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutoAsk {
    Off,
    Due(u8),
    Absent,
    Out { unknown: bool },
    Answered { on: bool },
}

impl Pump {
    /// Arm Marlin's position auto-report: the pump's first tick writes `M154 S<s>` on the realtime road, ahead of the first
    /// job line (Marlin builds with AUTO_REPORT_POSITION then say `report_current_position_projected`'s line, the M114
    /// shape, every `s` seconds, read by `marlin_status_line` as [`Event::Status`], `Run` during a job). Marlin only; `s`
    /// 0 never asks; called before the first tick (a pump asks at most once). The answer is swallowed and said once as
    /// [`Event::AutoReport`].
    pub fn autoreport(&mut self, s: u8) {
        if self.dialect == Dialect::Marlin && s > 0 && self.autoreport == AutoAsk::Off {
            self.autoreport = AutoAsk::Due(s);
        }
    }

    /// The first tick: an armed `M154 S<s>` onto `rt_pending` (flushed this tick before the window writes a line).
    /// VUGCOMSCANAUTOREPORTCAPOFF (SR634): an `Absent` ask says [`Event::AutoReportAbsent`] once instead.
    fn autoreport_due(&mut self, ev: &mut Vec<Event>) {
        match self.autoreport {
            AutoAsk::Due(s) => {
                self.rt_pending.extend_from_slice(alloc::format!("M154 S{s}\n").as_bytes());
                self.autoreport = AutoAsk::Out { unknown: false };
            }
            AutoAsk::Absent => {
                self.autoreport = AutoAsk::Answered { on: false };
                ev.push(Event::AutoReportAbsent);
            }
            _ => {}
        }
    }

    /// One Marlin line while the M154 is out, ahead of the M114 `ok` arm (the M154 went out first, so its answer comes
    /// first): `echo:Unknown command` naming M154 (a build without AUTO_REPORT_POSITION; Marlin still says `ok` after it)
    /// is taken; the `ok` is taken and said as [`Event::AutoReport`]; `start` or a halt forget it silently and go on to
    /// their own roads. `true`: the line was taken here.
    fn autoreport_line(&mut self, s: &str, ev: &mut Vec<Event>) -> bool {
        let AutoAsk::Out { unknown } = self.autoreport else { return false };
        if s.starts_with("echo:Unknown command") && s.contains("M154") {
            self.autoreport = AutoAsk::Out { unknown: true };
            return true;
        }
        if s.starts_with("ok") {
            self.autoreport = AutoAsk::Answered { on: !unknown };
            ev.push(Event::AutoReport { on: !unknown });
            return true;
        }
        if s == "start" || marlin_halted(s) {
            self.autoreport = AutoAsk::Answered { on: false };
        }
        false
    }

    /// A lost link answers no M154: an M154 out is forgotten, so a later home / jog line's `ok` is never swallowed.
    /// VUGCOMSCANAUTOREPORTCAPOFF (SR634): a running report is forgotten too, so a lost link never hears `M154 S0`.
    fn autoreport_lost(&mut self) {
        if matches!(self.autoreport, AutoAsk::Out { .. } | AutoAsk::Answered { on: true }) {
            self.autoreport = AutoAsk::Answered { on: false };
        }
    }
}

// --- VUGCOMSCANAUTOREPORTCAPOFF (LEDGER SR634): the M154 asked only where M115 says the cap; `M154 S0` at the desk's close (named tail hunk) ---

/// The one M115 capability line the hello keeps ([`hello_line`]): Marlin built with AUTO_REPORT_POSITION.
pub const AUTOREPORT_CAP: &str = "Cap:AUTOREPORT_POS:1";

/// The kept hello answer (`said`, the kept lines joined by one space) carries [`AUTOREPORT_CAP`].
pub fn autoreport_cap(said: &str) -> bool {
    said.split(' ').any(|w| w == AUTOREPORT_CAP)
}

impl Pump {
    /// Arm the position auto-report from the link's hello: with [`AUTOREPORT_CAP`] in `said`, SR625's
    /// [`Pump::autoreport`] at [`AUTOREPORT_S`]; without it (or no hello kept) the M154 is never asked and the first tick
    /// says [`Event::AutoReportAbsent`] once. Marlin only; called before the first tick (a pump arms at most once).
    pub fn autoreport_hello(&mut self, said: Option<&str>) {
        if self.dialect != Dialect::Marlin || self.autoreport != AutoAsk::Off {
            return;
        }
        if said.is_some_and(autoreport_cap) {
            self.autoreport(AUTOREPORT_S);
        } else {
            self.autoreport = AutoAsk::Absent;
        }
    }

    /// The desk's own close of a live link: `M154 S0` on the realtime road when the report was turned on (an `ok` to
    /// the M154), so the controller is not left reporting into a dead line. One try, never a wait on its answer: a
    /// short or failed write is dropped (the link is closing). Nothing after a loss (`autoreport_lost` forgot the on).
    pub fn autoreport_off(&mut self, t: &mut dyn Transport) {
        if self.autoreport != (AutoAsk::Answered { on: true }) {
            return;
        }
        self.autoreport = AutoAsk::Answered { on: false };
        self.rt_pending.extend_from_slice(b"M154 S0\n");
        let _ = self.flush_rt(t);
    }
}

// --- VUGCOMSCANLINKREUSEINFLIGHT (LEDGER SR646): a restart on the held link carries the in-flight M114 / M154 answer (named tail hunk) ---

impl Pump {
    /// The arming's pump takes over the live link `from` streamed on (comscan's `PumpDesk::restarted`, SR641): what is
    /// still on the wire for `from` is read by this pump as `from` would. `status_asked` (an `M114` whose `ok` is not yet
    /// read: swallowed here, never a job line's), `autoreport` (an `M154` out keeps its `ok` swallowed; an `Answered`
    /// report is not asked again, [`Pump::autoreport_hello`] arming only from `Off`, and the desk's close still turns it
    /// off) and the partial line in `rx` (an `ok` split across two reads stays whole). Nothing unless both speak the same
    /// dialect and this pump has not ticked.
    pub fn inherit(&mut self, from: &Pump) {
        if self.dialect != from.dialect || self.started {
            return;
        }
        self.status_asked = from.status_asked;
        self.autoreport = from.autoreport;
        self.rx = from.rx.clone();
    }
}

// --- VUGCOMSCANRESUMEMODAL (LEDGER SR656): a resume restores the modal state the lines before its line set (named tail hunk) ---

/// The modal groups a resume restores: 0 units, 1 distance, 2 plane, 3 feed mode, 4 work system, 5 extrusion.
pub const MODAL_GROUPS: usize = 6;

/// grbl 1.1's restored words as (letter, code, group, word); it boots in `G0 G54 G17 G21 G90 G94 M5 M9`. Motion and F
/// need none (make_core's writer says them on every motion line); `G91.1` is not `G91` (its code is not an integer).
const MODAL_GRBL: &[(u8, u16, usize, &str)] = &[
    (b'G', 20, 0, "G20"),
    (b'G', 21, 0, "G21"),
    (b'G', 90, 1, "G90"),
    (b'G', 91, 1, "G91"),
    (b'G', 17, 2, "G17"),
    (b'G', 18, 2, "G18"),
    (b'G', 19, 2, "G19"),
    (b'G', 93, 3, "G93"),
    (b'G', 94, 3, "G94"),
    (b'G', 54, 4, "G54"),
    (b'G', 55, 4, "G55"),
    (b'G', 56, 4, "G56"),
    (b'G', 57, 4, "G57"),
    (b'G', 58, 4, "G58"),
    (b'G', 59, 4, "G59"),
];

/// Marlin 2's restored words; it boots in `G21 G90 M82` (the planes and work systems are compiled out by default, G93
/// does not exist).
const MODAL_MARLIN: &[(u8, u16, usize, &str)] = &[
    (b'G', 20, 0, "G20"),
    (b'G', 21, 0, "G21"),
    (b'G', 90, 1, "G90"),
    (b'G', 91, 1, "G91"),
    (b'M', 82, 5, "M82"),
    (b'M', 83, 5, "M83"),
];

fn modal_table(d: Dialect) -> &'static [(u8, u16, usize, &'static str)] {
    match d {
        Dialect::Grbl { .. } => MODAL_GRBL,
        Dialect::Marlin => MODAL_MARLIN,
        Dialect::LinuxCnc => &[],
    }
}

/// Each lettered word on `line` (spaces dropped, upper-cased) as (letter, its number bytes, the bytes after it), in
/// order: grbl reads every word of a block and its `$` system lines carry none; Marlin runs ONE command per line, so
/// only its first word past an N word (VUGCOMSCANRESUMETOOL SR663: every letter and the rest, for the valued words).
fn modal_words(d: Dialect, line: &str, mut f: impl FnMut(u8, &[u8], &[u8])) {
    let marlin = d == Dialect::Marlin;
    let b: Vec<u8> = line.bytes().filter(|c| !c.is_ascii_whitespace()).map(|c| c.to_ascii_uppercase()).collect();
    if b.first() == Some(&b'$') {
        return;
    }
    let mut k = 0;
    while k < b.len() {
        let letter = b[k];
        k += 1;
        if !letter.is_ascii_alphabetic() {
            continue;
        }
        let s = k;
        while k < b.len() && matches!(b[k], b'0'..=b'9' | b'.' | b'-' | b'+') {
            k += 1;
        }
        if marlin && letter == b'N' {
            continue;
        }
        f(letter, &b[s..k], &b[k..]);
        if marlin {
            return;
        }
    }
}

/// A word's integer code (1-4 digits, no point or sign): `G91.1` has none, so it is not `G91`.
fn int_code(num: &[u8]) -> Option<u16> {
    if num.is_empty() || num.len() > 4 || !num.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(num.iter().fold(0u16, |a, c| a * 10 + u16::from(c - b'0')))
}

/// The modal state some lines set: per group the last word and the index of its line; per valued group (SR663) the
/// whole last word with its value, in the form a resume sends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Modal {
    last: [Option<(usize, &'static str)>; MODAL_GROUPS],
    tool: [Option<(usize, String)>; TOOL_GROUPS],
    power: Option<Option<String>>, // VUGCOMSCANRESUMELASER (SR671): grbl laser mode's last M4 with its modal S, folded at a resume, never a prefix line
    ix: ToolIndex, // VUGCOMSCANRESUMETOOLINDEX (SR672): Marlin's hotend and fan words keyed by index, below
}

impl Modal {
    /// The fold over `lines` in dialect `d` (LinuxCNC, not streamed, sets nothing).
    pub fn of(d: Dialect, lines: &[String]) -> Modal {
        let t = modal_table(d);
        let mut m = Modal::default();
        let mut run = ToolRun::default();
        for (i, l) in lines.iter().enumerate() {
            modal_words(d, l, |letter, num, _| {
                let Some(code) = int_code(num).filter(|_| letter == b'G' || letter == b'M') else { return };
                if let Some(&(_, _, g, w)) = t.iter().find(|e| e.0 == letter && e.1 == code) {
                    m.last[g] = Some((i, w));
                }
            });
            tool_fold(d, i, l, &mut m.tool, &mut run); // VUGCOMSCANRESUMETOOL (SR663): the valued words, below
        }
        m.power = laser_power(d, &run); // VUGCOMSCANRESUMELASER (SR671): read from the one spindle fold, below
        m.ix = run.ix; // VUGCOMSCANRESUMETOOLINDEX (SR672): the keyed hotends and fans the fold gathered
        m
    }

    /// One line per group set, in the program order of each group's last word (ties in group order, the valued groups
    /// after the modal ones): replayed so, the words leave a controller as the program left it, on a Marlin whose
    /// G90 / G91 also set E's mode or one whose do not. The same word at the same line is said once (an `M9` is the
    /// last word of both coolant groups).
    pub fn lines(&self) -> Vec<String> {
        let keyed = self.ix.words(); // VUGCOMSCANRESUMETOOLINDEX (SR672): one line per (hotend | fan, index)
        let mut v: Vec<(usize, usize, &str)> = self
            .last
            .iter()
            .enumerate()
            .filter_map(|(g, x)| x.map(|(i, w)| (i, g, w)))
            .chain(self.tool.iter().enumerate().filter_map(|(g, x)| x.as_ref().map(|(i, w)| (*i, MODAL_GROUPS + g, w.as_str()))))
            .chain(keyed.iter().map(|(i, g, w)| (*i, *g, w.as_str()))) // VUGCOMSCANRESUMETOOLINDEX (SR672)
            .collect();
        v.sort_unstable_by(|a, b| (tool_rank(a.1), a.0, a.1, a.2).cmp(&(tool_rank(b.1), b.0, b.1, b.2))); // VUGCOMSCANRESUMETOOLINDEX (SR672): heaters, then T, then fans
        v.dedup_by(|a, b| a.0 == b.0 && a.2 == b.2);
        v.into_iter().map(|(_, _, w)| String::from(w)).collect()
    }
}

impl Pump {
    /// [`Pump::resume_at`]'s prefix: every group the lines before index `i` set, as lines at `i` numbered `at_line` (the
    /// controller that lost the link most often rebooted into its defaults; one that did not, or a jog between, may sit
    /// anywhere). An `ok`, an `error:N` or a loss inside the prefix names `at_line`, so a re-arm restores again.
    fn modal_prefix(&mut self, i: usize, at_line: usize) -> Result<(), PumpRefusal> {
        let m = Modal::of(self.dialect, &self.lines[..i]);
        let words = m.lines();
        let n = words.len();
        for (k, w) in words.into_iter().enumerate() {
            self.lines.insert(i + k, w);
            self.src.insert(i + k, at_line);
        }
        self.laser_fold(i + n, &m) // VUGCOMSCANRESUMELASER (SR671): grbl laser power onto the first resumed motion line, below
    }
}

// --- VUGCOMSCANRESUMETOOL (LEDGER SR663): a resume restores the spindle, coolant, heater, fan and tool words the lines before its line set (named tail hunk) ---

/// The valued groups a resume restores (after [`MODAL_GROUPS`]'s): 0 spindle, 1 its spin-up dwell, 2 mist, 3 flood,
/// 4 hotend, 5 bed, 6 fan, 7 tool. A rebooted grbl sits in `M5 M9`; a rebooted Marlin has no heater target, no fan
/// and tool 0. A clearing word (`M5`, `M9`, `M107`, an `S0` target) is kept and replayed like any other: a board that
/// did not reboot may hold what a later, sent but unacknowledged line set. The prefix runs only inside the person's
/// own re-arm, restoring what their program had already set before the lost line.
pub const TOOL_GROUPS: usize = 8;
const SPINDLE: usize = 0;
const SPINUP: usize = 1;
const MIST: usize = 2;
const FLOOD: usize = 3;
const HOTEND: usize = 4;
const BED: usize = 5;
const FAN: usize = 6;
const TOOL: usize = 7;

/// The dwell after a restored grbl `M3 S` / `M4 S`: grbl 1.1's M3 mid-stream synchronises the planner and sets the
/// PWM, it does not wait for the spindle to reach speed, so the next move would plunge at a spinning-up tool. 4 s is
/// grbl's own number for the same case (config.h `SAFETY_DOOR_SPINDLE_DELAY`, the dwell it inserts when it restores
/// the spindle itself on a safety-door resume before motion); a constant, not a knob. Marlin's M3 dwells its own
/// `SPINDLE_LASER_POWERUP_DELAY`, so it gets none.
pub const GRBL_SPINUP_DWELL: &str = "G4 P4";

/// The grbl spindle carried across lines: its direction (3 / 4 / 5) and its modal `S` as written.
#[derive(Default)]
struct ToolRun {
    dir: Option<u16>,
    s: Option<String>,
    ix: ToolIndex, // VUGCOMSCANRESUMETOOLINDEX (SR672): Marlin's recorded T and its keyed hotend / fan words
}

/// Line `i`'s valued words into `tool`, in their resume form (LinuxCNC, not streamed, sets nothing).
fn tool_fold(d: Dialect, i: usize, line: &str, tool: &mut [Option<(usize, String)>; TOOL_GROUPS], run: &mut ToolRun) {
    match d {
        Dialect::Grbl { laser } => grbl_tool(laser, i, line, tool, run),
        Dialect::Marlin => marlin_tool(i, line, tool, run), // VUGCOMSCANRESUMETOOLINDEX (SR672): the run carries the active tool
        Dialect::LinuxCnc => {}
    }
}

/// grbl 1.1: the block's M3 / M4 / M5 with the modal `S` (`S12000 M3`, `M3 S1200`, or an `S` alone changing a running
/// spindle), then the dwell; M7 / M8 each its own group, M9 both. In laser mode ($32=1) the power word is not a
/// prefix line (a beam on before the move burns a spot): its M4 is folded onto the first resumed motion line
/// (VUGCOMSCANRESUMELASER SR671, [`Pump::laser_fold`]); the air assist is a prefix line. `T` is not restored: grbl 1.1
/// acts on nothing for it and refuses M6.
fn grbl_tool(laser: bool, i: usize, line: &str, tool: &mut [Option<(usize, String)>; TOOL_GROUPS], run: &mut ToolRun) {
    let mut spin = None;
    let mut s: Option<String> = None;
    let mut cool: Vec<u16> = Vec::new();
    modal_words(Dialect::Grbl { laser }, line, |letter, num, _| match (letter, int_code(num)) {
        (b'M', Some(c @ (3 | 4 | 5))) => spin = Some(c),
        (b'M', Some(c @ (7 | 8 | 9))) => cool.push(c),
        (b'S', _) if !num.is_empty() => s = core::str::from_utf8(num).ok().map(String::from),
        _ => {}
    });
    for c in cool {
        let w = alloc::format!("M{c}");
        match c {
            7 => tool[MIST] = Some((i, w)),
            8 => tool[FLOOD] = Some((i, w)),
            _ => {
                tool[MIST] = Some((i, w.clone()));
                tool[FLOOD] = Some((i, w));
            }
        }
    }
    let said_s = s.is_some();
    if s.is_some() {
        run.s = s;
    }
    if spin.is_some() {
        run.dir = spin;
    }
    if laser || (spin.is_none() && !said_s) {
        return;
    }
    match run.dir {
        Some(c @ (3 | 4)) => {
            let w = match &run.s {
                Some(v) => alloc::format!("M{c} S{v}"),
                None => alloc::format!("M{c}"),
            };
            tool[SPINDLE] = Some((i, w));
            tool[SPINUP] = Some((i, String::from(GRBL_SPINUP_DWELL)));
        }
        Some(_) if spin.is_some() => {
            tool[SPINDLE] = Some((i, String::from("M5")));
            tool[SPINUP] = None;
        }
        _ => {}
    }
}

/// Marlin 2: the line's one command with its parameters, re-spaced; a hotend / bed target above 0 set by `M104` /
/// `M140` is restored as the blocking `M109` / `M190` (a resumed print must not extrude cold; `M109 S` waits only
/// when heating), a recorded `M109` / `M190` stays, an `S0` stays non-blocking; `M106` / `M107` the fan, `T<n>` the
/// tool, `M3` / `M4` / `M5` the spindle, `M7` / `M8` / `M9` the coolant. VUGCOMSCANRESUMETOOLINDEX (SR672): the
/// hotend and fan words are keyed by index into `run.ix` ([`ToolIndex`]), the `T` word records the active tool.
fn marlin_tool(i: usize, line: &str, tool: &mut [Option<(usize, String)>; TOOL_GROUPS], run: &mut ToolRun) {
    modal_words(Dialect::Marlin, line, |letter, num, rest| {
        let rest = spaced(rest);
        let word = |c: &str| alloc::format!("{c}{rest}");
        match (letter, int_code(num)) {
            (b'M', Some(c @ (3 | 4))) => tool[SPINDLE] = Some((i, word(&alloc::format!("M{c}")))),
            (b'M', Some(5)) => tool[SPINDLE] = Some((i, String::from("M5"))),
            (b'M', Some(7)) => tool[MIST] = Some((i, String::from("M7"))),
            (b'M', Some(8)) => tool[FLOOD] = Some((i, String::from("M8"))),
            (b'M', Some(9)) => {
                tool[MIST] = Some((i, String::from("M9")));
                tool[FLOOD] = Some((i, String::from("M9")));
            }
            (b'M', Some(c @ (104 | 109))) => run.ix.set(i, HOTEND, if c == 109 || heats(&rest) { "M109" } else { "M104" }, b'T', &rest), // SR672: by index
            (b'M', Some(c @ (140 | 190))) => tool[BED] = Some((i, word(if c == 190 || heats(&rest) { "M190" } else { "M140" }))),
            (b'M', Some(106)) => run.ix.set(i, FAN, "M106", b'P', &rest), // SR672: by index
            (b'M', Some(107)) => run.ix.set(i, FAN, "M107", b'P', &rest), // SR672: by index
            (b'T', Some(n)) => {
                tool[TOOL] = Some((i, alloc::format!("T{n}")));
                run.ix.active = Some(n); // VUGCOMSCANRESUMETOOLINDEX (SR672): the active tool from here on
            }
            _ => {}
        }
    });
}

/// The bytes after a Marlin command (spaces dropped, upper-cased) said as ` T1 S210`: one space before each letter.
fn spaced(rest: &[u8]) -> String {
    let mut o = String::new();
    for &c in rest {
        if c.is_ascii_alphabetic() {
            o.push(' ');
        }
        o.push(char::from(c));
    }
    o
}

/// A Marlin heater command's target above 0 (its `S`, or the `R` that waits either way).
fn heats(rest: &str) -> bool {
    rest.split(' ').any(|p| matches!(p.as_bytes().first(), Some(b'S' | b'R')) && p[1..].parse::<f64>().is_ok_and(|t| t > 0.0))
}

// --- VUGCOMSCANMARLINJOGMODAL (LEDGER SR664): the run's units and distance words for a jog (named tail hunk) ---

impl Modal {
    /// Group 0's last word (`G20` / `G21`), `None` when the lines set none.
    pub fn units(&self) -> Option<&'static str> {
        self.last[0].map(|(_, w)| w)
    }

    /// Group 1's last word (`G90` / `G91`), `None` when the lines set none.
    pub fn distance(&self) -> Option<&'static str> {
        self.last[1].map(|(_, w)| w)
    }
}

impl Pump {
    /// The modal state the lines sent so far set ([`Modal::of`] over them): with nothing in flight (a jog's
    /// precondition) the state the run left on the board, read by [`jog_lines`].
    pub fn sent_modal(&self) -> Modal {
        Modal::of(self.dialect, &self.lines[..self.next])
    }
}

// --- VUGCOMSCANRESUMELASER (LEDGER SR671): a grbl laser resume folds its dynamic power onto the first resumed motion line (named tail hunk) ---

/// The power a grbl laser-mode ($32=1) resume restores, read from [`Modal::of`]'s one spindle fold: `Some(s)` when the
/// last spindle word before the resume line was `M4` (with its modal `S` as written, if any). gnea/grbl
/// `doc/markdown/laser_mode.md`: `M4` is dynamic laser power, scaled by the current speed against the programmed rate,
/// so zero at rest; `M3` is constant power whether the machine moves, accelerates or stands still. A recorded `M3` is
/// therefore never restored (a static beam: restored with the head standing it would burn a spot); the resumed lines run
/// unpowered until the program says its own power. `M5`, no word, grbl outside laser mode and Marlin restore nothing here.
fn laser_power(d: Dialect, run: &ToolRun) -> Option<Option<String>> {
    match (d, run.dir) {
        (Dialect::Grbl { laser: true }, Some(4)) => Some(run.s.clone()),
        _ => None,
    }
}

impl Pump {
    /// The restored laser power, folded in place onto the first resumed motion line from index `from` (the line after
    /// the prefix): that line becomes `M4 <line>` plus ` S<power>` when neither it nor a resumed line before it said an
    /// `S` (its own `S` is kept). No line is inserted, so the numbering is untouched. A `G0` or a line with no `G1` /
    /// `G2` / `G3` word stays bare (grbl's laser mode runs rapids with the laser off); a resumed line that says `M3` /
    /// `M4` / `M5` before any motion ends the scan with nothing folded (the program states its own power); no motion
    /// after the resume line, nothing restored. A folded line past grbl's window is refused as [`PumpRefusal::LineTooLong`].
    /// grbl needs no warm-up dwell: M4's power follows the motion.
    fn laser_fold(&mut self, from: usize, m: &Modal) -> Result<(), PumpRefusal> {
        let Some(power) = &m.power else { return Ok(()) };
        let d = self.dialect;
        let mut said_s = false;
        for j in from..self.lines.len() {
            let (mut spin, mut s, mut cut) = (false, false, false);
            modal_words(d, &self.lines[j], |letter, num, _| match (letter, int_code(num)) {
                (b'M', Some(3 | 4 | 5)) => spin = true,
                (b'G', Some(1 | 2 | 3)) => cut = true,
                (b'S', _) if !num.is_empty() => s = true,
                _ => {}
            });
            if spin {
                return Ok(());
            }
            said_s |= s;
            if !cut {
                continue;
            }
            let mut l = alloc::format!("M4 {}", self.lines[j]);
            if let (false, Some(v)) = (said_s, power) {
                l.push_str(" S");
                l.push_str(v);
            }
            if l.len() + 1 > GRBL_RX {
                return Err(PumpRefusal::LineTooLong { line: self.src[j], len: l.len() + 1 });
            }
            self.lines[j] = l;
            return Ok(());
        }
        Ok(())
    }
}

// --- VUGCOMSCANRESUMETOOLINDEX (LEDGER SR672): a resume restores every hotend and fan index the lines before its line set (named tail hunk) ---

/// Marlin's per-index heater and fan words, one per (group, index), and the tool the program made active. The index is
/// the word's own `T` (M104 / M109) or `P` (M106 / M107), else the ACTIVE tool at that line (none = 0, Marlin boots in
/// T0): Marlin 2.1 `get_target_extruder_from_command` (gcode.cpp, "T<index> : Tool index. If omitted, applies to the
/// active tool", M104_M109.cpp) and M106 / M107's `parser.byteval('P', _MIN(active_extruder, FAN_COUNT - 1))`
/// (M106_M107.cpp; M106's `T` is EXTRA_FAN_SPEED's temporary speed, not an index). A Vec keyed by index, so no cap
/// here: Marlin bounds EXTRUDERS at 8 (15 with an extendable MMU, SanityCheck.h) and FAN_COUNT at 12
/// (Conditionals_post.h), and refuses an index past its build on the replay as it did the first time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ToolIndex {
    active: Option<u16>,
    words: Vec<IxWord>,
}

/// One keyed word: its line, group ([`HOTEND`] or [`FAN`]), index, command, whether the line said its index, and its
/// other parameters re-spaced (` S210`) with the index token taken out.
#[derive(Clone, Debug, PartialEq, Eq)]
struct IxWord {
    i: usize,
    group: usize,
    index: u16,
    cmd: &'static str,
    said: bool,
    rest: String,
}

impl ToolIndex {
    /// Line `i`'s `cmd` with its re-spaced parameters `rest` into the key (`group`, the `letter` word's index or the
    /// active tool's): a later word for the same key replaces it (an `M107` or an `S0` target clears that index only).
    fn set(&mut self, i: usize, group: usize, cmd: &'static str, letter: u8, rest: &str) {
        let (said, rest) = index_of(rest, letter);
        let index = said.unwrap_or(self.active.unwrap_or(0));
        let w = IxWord { i, group, index, cmd, said: said.is_some(), rest };
        match self.words.iter_mut().find(|x| x.group == group && x.index == index) {
            Some(x) => *x = w,
            None => self.words.push(w),
        }
    }

    /// Each key's word as (line, the [`Modal::lines`] group, the line a resume sends). A hotend says ` T<n>` first
    /// whenever the program is multi-tool (a `T` was recorded, a hotend index past 0, or the line said its own): the
    /// heaters run before the restored `T`, on a board whose active tool may be any; a single-tool program keeps its
    /// bytes. A fan that said its `P` keeps it; an index-less fan set under the FINAL active tool stays index-less (it
    /// runs after that `T` and Marlin resolves it to the same fan it drove, whatever FAN_COUNT); one set under another
    /// tool says ` P<that tool>` (exact on a fan per tool; a board with fewer fans ignores it:
    /// VUG-COMSCAN-RESUME-TOOL-FANCOUNT owed).
    fn words(&self) -> Vec<(usize, usize, String)> {
        let multi = self.active.is_some() || self.words.iter().any(|w| w.group == HOTEND && (w.index != 0 || w.said));
        let fin = self.active.unwrap_or(0);
        self.words
            .iter()
            .map(|w| {
                let named = if w.group == HOTEND { multi || w.said } else { w.said || w.index != fin };
                let letter = if w.group == HOTEND { 'T' } else { 'P' };
                let line = if named {
                    alloc::format!("{} {letter}{}{}", w.cmd, w.index, w.rest)
                } else {
                    alloc::format!("{}{}", w.cmd, w.rest)
                };
                (w.i, MODAL_GROUPS + w.group, line)
            })
            .collect()
    }
}

/// The `letter` word's integer index out of re-spaced parameters (` T1 S210` -> (Some(1), ` S210`)); none, or one that
/// is not an integer, leaves the parameters as they are.
fn index_of(rest: &str, letter: u8) -> (Option<u16>, String) {
    let mut index = None;
    let mut o = String::new();
    for p in rest.split(' ').filter(|p| !p.is_empty()) {
        let b = p.as_bytes();
        if index.is_none() && b[0] == letter {
            if let Some(n) = int_code(&b[1..]) {
                index = Some(n);
                continue;
            }
        }
        o.push(' ');
        o.push_str(p);
    }
    (index, o)
}

/// [`Modal::lines`]'s order rank: every heater (rank 0, with the modal words, in program order) before the restored
/// `T` (1), the fans after it (2) — an index-less fan resolves against the active tool.
fn tool_rank(g: usize) -> u8 {
    if g == MODAL_GROUPS + TOOL {
        1
    } else if g == MODAL_GROUPS + FAN {
        2
    } else {
        0
    }
}
