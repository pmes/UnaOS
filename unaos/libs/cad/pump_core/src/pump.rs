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
    digest: u64,
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
            digest: 0,
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

    /// grbl `?`: one status report, answered as [`Event::Status`].
    pub fn status(&mut self, t: &mut dyn Transport) -> Result<(), PumpRefusal> {
        self.grbl_only("status query")?;
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
        } else if s.starts_with("ok") {
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
            self.started = true;
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
        self.next = i;
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
        self.rx.clear();
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
    /// Anything else (a push message, a `Cap:` line, an echo): not the identity.
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
            if s.contains("FIRMWARE_NAME:") {
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
/// `G91`, `G0 X.. Y.. Z.. F..`, `G90` (relative for the one move, absolute again after); LinuxCNC: `None`.
pub fn jog_lines(d: Dialect, delta: [f64; 3], feed: f64) -> Option<Vec<String>> {
    let mut mv = String::new();
    for (c, v) in [('X', delta[0]), ('Y', delta[1]), ('Z', delta[2]), ('F', feed)] {
        mv.push(' ');
        mv.push(c);
        make_core::gcode::num(&mut mv, v);
    }
    match d {
        Dialect::Grbl { .. } => Some(alloc::vec![alloc::format!("$J=G91{mv}")]),
        Dialect::Marlin => Some(alloc::vec![String::from("G91"), alloc::format!("G0{mv}"), String::from("G90")]),
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
        if let Some(v) = c.as_str().parse::<f64>().ok().filter(|v| v.is_finite()) {
            out[i] = Some(v);
        }
    }
    out
}
