// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The `Provider` trait and the two metal providers of v1. No allocation: a provider streams its answer
//! as ChatReply chunks through the caller's [`ProviderIo`], sized by the caller's chunk buffer (VEIN.BIN
//! uses a small one; the 4 KiB body ceiling is the upper bound).
//!
//! * [`Echo`] — answers at once: `echo: ` + the prompt reversed by characters. Proves the whole bus path
//!   on the glass with no network.
//! * [`Relay`] — writes `[vein-relay] REQ <conv> <base64>` on the console (the bench machine runs the host
//!   Vein and answers with `vein rsp …` shell lines); the answer arrives later as verb-131 frames.
//!
//! Later: `Claude` (needs TLS + DNS on the metal: NETRING3) — on the host, VEINPROV's `ClaudeProvider`
//! becomes one more `Provider` at the fold.

use crate::wire::{ChatReply, REPLY_TEXT_MAX};

/// What a provider talks to: the ChatReply frames back to the caller, and a console line out.
pub trait ProviderIo {
    /// Send one ChatReply frame to the waiting caller. `false` = it could not be delivered.
    fn reply(&mut self, r: &ChatReply<'_>) -> bool;
    /// Write bytes to the console (pieces of one line; the provider ends it with `\n`).
    fn line(&mut self, bytes: &[u8]);
}

/// The outcome of [`Provider::begin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begin {
    /// The whole answer was sent (the last frame carried `done = 1`).
    Done { frames: u32 },
    /// The answer will arrive later (the caller keeps the correlation open).
    Pending,
    /// The provider refused (a negative errno for the caller's final frame).
    Err(i32),
}

pub trait Provider {
    /// The Principia `vein.provider` value naming this provider.
    fn name(&self) -> &'static str;
    fn model(&self) -> &'static str;
    fn ready(&self) -> bool;
    /// Start answering `text` for `conv`.
    fn begin(&mut self, conv: u32, text: &[u8], io: &mut dyn ProviderIo) -> Begin;
}

/// Splits an answer into ChatReply frames over a caller-supplied buffer. Never splits a UTF-8 character;
/// the final frame (`done = 1`) is always sent by [`Chunker::finish`], possibly empty.
pub struct Chunker<'b> {
    conv: u32,
    seq: u16,
    buf: &'b mut [u8],
    len: usize,
    pub frames: u32,
    pub failed: bool,
}

impl<'b> Chunker<'b> {
    /// `buf` is the per-frame text capacity (clamped to [`REPLY_TEXT_MAX`]); it must hold one 4-byte char.
    pub fn new(conv: u32, buf: &'b mut [u8]) -> Self {
        let cap = core::cmp::min(buf.len(), REPLY_TEXT_MAX);
        Chunker { conv, seq: 0, buf: &mut buf[..cap], len: 0, frames: 0, failed: false }
    }
    fn flush(&mut self, done: bool, io: &mut dyn ProviderIo) {
        if self.failed {
            self.len = 0; // a lost frame ends the stream; later text is dropped, never overflowed
            return;
        }
        let r = ChatReply { conv: self.conv, seq: self.seq, done, text: &self.buf[..self.len] };
        if io.reply(&r) {
            self.frames += 1;
        } else {
            self.failed = true;
        }
        self.seq = self.seq.wrapping_add(1);
        self.len = 0;
    }
    /// Append one indivisible unit (a whole UTF-8 character, or any byte run that may not be split).
    pub fn push_unit(&mut self, unit: &[u8], io: &mut dyn ProviderIo) {
        if self.len + unit.len() > self.buf.len() {
            self.flush(false, io);
        }
        let take = core::cmp::min(unit.len(), self.buf.len());
        self.buf[self.len..self.len + take].copy_from_slice(&unit[..take]);
        self.len += take;
    }
    /// Append text, splitting only at character boundaries.
    pub fn push_str(&mut self, s: &str, io: &mut dyn ProviderIo) {
        let mut u = [0u8; 4];
        for c in s.chars() {
            self.push_unit(c.encode_utf8(&mut u).as_bytes(), io);
        }
    }
    /// Send the final frame (`done = 1`).
    pub fn finish(mut self, io: &mut dyn ProviderIo) -> (u32, bool) {
        self.flush(true, io);
        (self.frames, !self.failed)
    }
}

/// The per-frame text size VEIN.BIN uses (small: its whole program lives in a 16 KiB window).
pub const METAL_CHUNK: usize = 256;

/// Echo: `echo: ` + the prompt reversed by characters, streamed in `chunk`-sized frames.
pub struct Echo<'b> {
    pub chunk: &'b mut [u8],
}

pub const ECHO_PREFIX: &str = "echo: ";

impl Provider for Echo<'_> {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn model(&self) -> &'static str {
        "reverse"
    }
    fn ready(&self) -> bool {
        true
    }
    fn begin(&mut self, conv: u32, text: &[u8], io: &mut dyn ProviderIo) -> Begin {
        let Ok(s) = core::str::from_utf8(text) else {
            return Begin::Err(-22); // -EINVAL: the wire carries UTF-8
        };
        let mut c = Chunker::new(conv, self.chunk);
        c.push_str(ECHO_PREFIX, io);
        let mut u = [0u8; 4];
        for ch in s.chars().rev() {
            c.push_unit(ch.encode_utf8(&mut u).as_bytes(), io);
        }
        match c.finish(io) {
            (frames, true) => Begin::Done { frames },
            (_, false) => Begin::Err(-5), // -EIO: a frame could not be delivered
        }
    }
}

/// The answer [`Echo`] gives, as a plain string — for the host tests and the kernel fixture's check.
#[cfg(feature = "alloc")]
pub fn echo_answer(text: &str) -> alloc::string::String {
    let mut s = alloc::string::String::from(ECHO_PREFIX);
    s.extend(text.chars().rev());
    s
}

/// Relay: hands the request to the bench machine over the console line; the answer comes back later.
pub struct Relay;

impl Provider for Relay {
    fn name(&self) -> &'static str {
        "relay"
    }
    fn model(&self) -> &'static str {
        "host-vein"
    }
    fn ready(&self) -> bool {
        true
    }
    fn begin(&mut self, conv: u32, text: &[u8], io: &mut dyn ProviderIo) -> Begin {
        crate::wire::relay_req_line(conv, text, &mut |p| io.line(p));
        Begin::Pending
    }
}

/// Which provider a Principia `vein.provider` value names. The PREF_GET reply is a TOML literal, so a
/// string comes back quoted (`"relay"`); both forms are accepted. Anything else (or unset) is Echo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Echo,
    Relay,
}

pub fn choose(pref: Option<&[u8]>) -> Choice {
    let Some(v) = pref else { return Choice::Echo };
    let v = if v.len() >= 2 && v[0] == b'"' && v[v.len() - 1] == b'"' { &v[1..v.len() - 1] } else { v };
    match v {
        b"relay" => Choice::Relay,
        _ => Choice::Echo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::ChatReply;
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    #[derive(Default)]
    struct Cap {
        frames: Vec<(u32, u16, bool, Vec<u8>)>,
        line: Vec<u8>,
        fail_after: Option<usize>,
    }
    impl ProviderIo for Cap {
        fn reply(&mut self, r: &ChatReply<'_>) -> bool {
            if self.fail_after.is_some_and(|n| self.frames.len() >= n) {
                return false;
            }
            self.frames.push((r.conv, r.seq, r.done, r.text.to_vec()));
            true
        }
        fn line(&mut self, b: &[u8]) {
            self.line.extend_from_slice(b);
        }
    }

    #[test]
    fn echo_streams_in_order_and_never_splits_a_char() {
        let mut buf = [0u8; 8];
        let mut e = Echo { chunk: &mut buf };
        let mut cap = Cap::default();
        let text = "héllo wörld ✓ abc";
        let r = e.begin(9, text.as_bytes(), &mut cap);
        let n = cap.frames.len() as u32;
        assert_eq!(r, Begin::Done { frames: n });
        assert!(n >= 3);
        let mut all = Vec::new();
        for (i, (conv, seq, done, t)) in cap.frames.iter().enumerate() {
            assert_eq!(*conv, 9);
            assert_eq!(*seq as usize, i);
            assert_eq!(*done, i + 1 == cap.frames.len());
            assert!(core::str::from_utf8(t).is_ok(), "frame {i} splits a char");
            all.extend_from_slice(t);
        }
        assert_eq!(String::from_utf8(all).unwrap(), echo_answer(text));
    }

    #[test]
    fn echo_refuses_bad_utf8_and_reports_a_lost_frame() {
        let mut buf = [0u8; 8];
        let mut cap = Cap::default();
        assert_eq!(Echo { chunk: &mut buf }.begin(1, &[0xff, 0xfe], &mut cap), Begin::Err(-22));
        let mut cap = Cap { fail_after: Some(1), ..Default::default() };
        assert_eq!(Echo { chunk: &mut buf }.begin(1, b"a long enough prompt", &mut cap), Begin::Err(-5));
    }

    #[test]
    fn relay_writes_the_documented_line() {
        let mut cap = Cap::default();
        assert_eq!(Relay.begin(42, b"what is UnaOS?", &mut cap), Begin::Pending);
        assert_eq!(cap.line, b"[vein-relay] REQ 42 d2hhdCBpcyBVbmFPUz8=\n");
        assert!(cap.frames.is_empty());
    }

    #[test]
    fn provider_choice_from_principia() {
        assert_eq!(choose(None), Choice::Echo);
        assert_eq!(choose(Some(b"\"relay\"")), Choice::Relay);
        assert_eq!(choose(Some(b"relay")), Choice::Relay);
        assert_eq!(choose(Some(b"\"echo\"")), Choice::Echo);
        assert_eq!(choose(Some(b"\"claude\"")), Choice::Echo); // reserved until TLS (NETRING3)
    }
}
