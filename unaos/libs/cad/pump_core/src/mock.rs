// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `MockGrbl`: a scripted grbl 1.1 behind the [`Transport`] trait, for the pump's tests and the handler's.
//!
//! It keeps grbl's 128-byte receive buffer and REFUSES overflow (the byte is dropped and `overflowed` is set, the way a
//! real serial buffer loses it), so a test that streams through it and reads `!overflowed` witnesses the window. The
//! realtime bytes are intercepted before the buffer, as grbl does. It answers one buffered line every `delay_reads`
//! reads (the mock has no clock: the delay is counted in reads), by the per-line script (`ok` when unscripted).

use alloc::{collections::BTreeMap, collections::VecDeque, format, string::String, vec::Vec};

use crate::{LinkError, Transport};

/// grbl's serial receive buffer.
pub const RX_BUFFER: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Ok,
    Error(u16),
    Alarm(u16),
}

pub struct MockGrbl {
    rx: VecDeque<u8>,
    out: VecDeque<u8>,
    script: BTreeMap<usize, Reply>,
    /// Every line the mock took out of its buffer, in order.
    pub executed: Vec<String>,
    /// Every realtime byte it saw, in order.
    pub realtime: Vec<u8>,
    pub overflowed: bool,
    pub max_fill: usize,
    pub held: bool,
    pub alarmed: bool,
    /// Answer nothing at all (a dead controller).
    pub dead: bool,
    /// The link is gone: every call errs.
    pub closed: bool,
    /// Bytes taken per write call (partial writes).
    pub accept_per_write: usize,
    pub delay_reads: u32,
    reads: u32,
}

impl MockGrbl {
    pub fn new(delay_reads: u32) -> MockGrbl {
        MockGrbl {
            rx: VecDeque::new(),
            out: VecDeque::new(),
            script: BTreeMap::new(),
            executed: Vec::new(),
            realtime: Vec::new(),
            overflowed: false,
            max_fill: 0,
            held: false,
            alarmed: false,
            dead: false,
            closed: false,
            accept_per_write: usize::MAX,
            delay_reads: delay_reads.max(1),
            reads: 0,
        }
    }

    /// Answer the `idx`-th executed line (0-based) with `r`.
    pub fn script(mut self, idx: usize, r: Reply) -> MockGrbl {
        self.script.insert(idx, r);
        self
    }

    /// Bytes waiting in the receive buffer.
    pub fn fill(&self) -> usize {
        self.rx.len()
    }

    /// Say something unprompted (a welcome, a push message).
    pub fn say(&mut self, s: &str) {
        self.out.extend(s.as_bytes());
    }

    fn status_line(&self) -> String {
        let st = if self.alarmed {
            "Alarm"
        } else if self.held {
            "Hold:0"
        } else if self.rx.is_empty() {
            "Idle"
        } else {
            "Run"
        };
        format!("<{}|MPos:0.000,0.000,0.000|Bf:15,{}|FS:0,0>\r\n", st, RX_BUFFER - self.rx.len())
    }
}

impl Transport for MockGrbl {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, LinkError> {
        if self.closed {
            return Err(LinkError::Closed);
        }
        let n = bytes.len().min(self.accept_per_write);
        for &b in &bytes[..n] {
            match b {
                b'?' | b'!' | b'~' | 0x18 => {
                    self.realtime.push(b);
                    if self.dead {
                        continue;
                    }
                    match b {
                        b'?' => {
                            let s = self.status_line();
                            self.out.extend(s.as_bytes());
                        }
                        b'!' => self.held = true,
                        b'~' => self.held = false,
                        _ => {
                            self.rx.clear();
                            self.held = false;
                            self.out.extend(b"\r\nGrbl 1.1h ['$' for help]\r\n".iter());
                        }
                    }
                }
                b if b >= 0x80 => self.realtime.push(b),
                b => {
                    if self.rx.len() >= RX_BUFFER {
                        self.overflowed = true;
                    } else {
                        self.rx.push_back(b);
                        self.max_fill = self.max_fill.max(self.rx.len());
                    }
                }
            }
        }
        Ok(n)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        if self.closed {
            return Err(LinkError::Closed);
        }
        self.reads += 1;
        if !self.dead && !self.held && !self.alarmed && self.reads >= self.delay_reads {
            if let Some(pos) = self.rx.iter().position(|&b| b == b'\n') {
                self.reads = 0;
                let raw: Vec<u8> = self.rx.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&raw[..raw.len() - 1]).into_owned();
                let idx = self.executed.len();
                self.executed.push(line);
                match self.script.get(&idx).copied().unwrap_or(Reply::Ok) {
                    Reply::Ok => self.out.extend(b"ok\r\n".iter()),
                    Reply::Error(c) => self.out.extend(format!("error:{}\r\n", c).as_bytes()),
                    Reply::Alarm(c) => {
                        self.alarmed = true;
                        self.out.extend(format!("ALARM:{}\r\n", c).as_bytes());
                    }
                }
            }
        }
        let n = buf.len().min(self.out.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.out.pop_front().unwrap_or(0);
        }
        Ok(n)
    }
}
