// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The chat verbs' BUS BODY CODEC (BANDY v1 frames; bodies only — the 52-byte header is `crate::bus` in
//! the kernel and the hand-built header in each ring-3 program). All integers little-endian. Every body
//! fits the 4 KiB frame body ([`BODY_MAX`]); a long answer is a SEQUENCE of ChatReply frames on one
//! correlation id, `done = 0` (header status [`STATUS_MORE`]) until the last (`done = 1`, status 0).
//!
//! | verb | tag | body |
//! | :--- | ---: | :--- |
//! | ChatSend | 130 | `conv u32` · `text` (1..=[`SEND_TEXT_MAX`]) |
//! | ChatReply | 130 (REPLY) / 131 (kernel REQUEST) | `conv u32` · `seq u16` · `done u8` · `rsvd u8`=0 · `text` (0..=[`REPLY_TEXT_MAX`]) |
//! | ChatCancel | 132 | `conv u32` |
//! | ChatStatus | 133 | request empty; reply `ready u8` · `plen u8` · `mlen u8` · `rsvd u8`=0 · `provider` · `model` |

/// Mirrors of the una-abi tags (this crate has no deps; the kernel const-asserts the two agree).
pub const VERB_CHAT_SEND: u8 = 130;
pub const VERB_CHAT_REPLY: u8 = 131;
pub const VERB_CHAT_CANCEL: u8 = 132;
pub const VERB_CHAT_STATUS: u8 = 133;
/// The one positive header status: "a non-final frame of a multi-frame answer — more follow".
pub const STATUS_MORE: i32 = 1;
/// `-ECANCELED`: the stream a ChatCancel cut ends with this status.
pub const ECANCELED: i32 = -125;

/// The BANDY v1 body ceiling (una-abi `BUS_BODY_MAX`).
pub const BODY_MAX: usize = 4096;
pub const SEND_HDR: usize = 4;
pub const REPLY_HDR: usize = 8;
pub const STATUS_HDR: usize = 4;
pub const SEND_TEXT_MAX: usize = BODY_MAX - SEND_HDR;
pub const REPLY_TEXT_MAX: usize = BODY_MAX - REPLY_HDR;
/// Provider / model name ceiling in a ChatStatus reply.
pub const NAME_MAX: usize = 64;

/// Decode refusals (each maps to `-EINVAL` on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireErr {
    /// Shorter than the fixed part.
    Short,
    /// Over the body ceiling, or a field over its bound.
    TooBig,
    /// A reserved byte nonzero, `done` not 0/1, empty send text, trailing bytes.
    Malformed,
    /// The destination buffer cannot hold the encoding.
    NoRoom,
}

fn rd32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

// ── ChatSend ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatSend<'a> {
    pub conv: u32,
    pub text: &'a [u8],
}

impl<'a> ChatSend<'a> {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireErr> {
        if self.text.is_empty() {
            return Err(WireErr::Malformed);
        }
        if self.text.len() > SEND_TEXT_MAX {
            return Err(WireErr::TooBig);
        }
        let n = SEND_HDR + self.text.len();
        if out.len() < n {
            return Err(WireErr::NoRoom);
        }
        out[..4].copy_from_slice(&self.conv.to_le_bytes());
        out[4..n].copy_from_slice(self.text);
        Ok(n)
    }
    pub fn decode(b: &'a [u8]) -> Result<Self, WireErr> {
        if b.len() > BODY_MAX {
            return Err(WireErr::TooBig);
        }
        if b.len() < SEND_HDR {
            return Err(WireErr::Short);
        }
        if b.len() == SEND_HDR {
            return Err(WireErr::Malformed); // a send says something
        }
        Ok(ChatSend { conv: rd32(b), text: &b[SEND_HDR..] })
    }
}

// ── ChatReply ─────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatReply<'a> {
    pub conv: u32,
    pub seq: u16,
    pub done: bool,
    pub text: &'a [u8],
}

impl<'a> ChatReply<'a> {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireErr> {
        if self.text.len() > REPLY_TEXT_MAX {
            return Err(WireErr::TooBig);
        }
        let n = REPLY_HDR + self.text.len();
        if out.len() < n {
            return Err(WireErr::NoRoom);
        }
        out[..4].copy_from_slice(&self.conv.to_le_bytes());
        out[4..6].copy_from_slice(&self.seq.to_le_bytes());
        out[6] = self.done as u8;
        out[7] = 0;
        out[8..n].copy_from_slice(self.text);
        Ok(n)
    }
    pub fn decode(b: &'a [u8]) -> Result<Self, WireErr> {
        if b.len() > BODY_MAX {
            return Err(WireErr::TooBig);
        }
        if b.len() < REPLY_HDR {
            return Err(WireErr::Short);
        }
        if b[6] > 1 || b[7] != 0 {
            return Err(WireErr::Malformed);
        }
        Ok(ChatReply { conv: rd32(b), seq: u16::from_le_bytes([b[4], b[5]]), done: b[6] == 1, text: &b[REPLY_HDR..] })
    }
    /// The header status this frame rides: [`STATUS_MORE`] for `done = 0`, 0 for the final frame.
    pub fn status(&self) -> i32 {
        if self.done { 0 } else { STATUS_MORE }
    }
}

// ── ChatCancel ────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatCancel {
    pub conv: u32,
}

impl ChatCancel {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireErr> {
        if out.len() < 4 {
            return Err(WireErr::NoRoom);
        }
        out[..4].copy_from_slice(&self.conv.to_le_bytes());
        Ok(4)
    }
    pub fn decode(b: &[u8]) -> Result<Self, WireErr> {
        match b.len() {
            0..=3 => Err(WireErr::Short),
            4 => Ok(ChatCancel { conv: rd32(b) }),
            _ => Err(WireErr::Malformed),
        }
    }
}

// ── ChatStatus ────────────────────────────────────────────────────────────────────────────────

/// The ChatStatus REQUEST body is empty; anything else is refused.
pub fn status_request_ok(b: &[u8]) -> bool {
    b.is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatStatus<'a> {
    pub ready: bool,
    pub provider: &'a [u8],
    pub model: &'a [u8],
}

impl<'a> ChatStatus<'a> {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireErr> {
        if self.provider.len() > NAME_MAX || self.model.len() > NAME_MAX {
            return Err(WireErr::TooBig);
        }
        let n = STATUS_HDR + self.provider.len() + self.model.len();
        if out.len() < n {
            return Err(WireErr::NoRoom);
        }
        out[0] = self.ready as u8;
        out[1] = self.provider.len() as u8;
        out[2] = self.model.len() as u8;
        out[3] = 0;
        out[4..4 + self.provider.len()].copy_from_slice(self.provider);
        out[4 + self.provider.len()..n].copy_from_slice(self.model);
        Ok(n)
    }
    pub fn decode(b: &'a [u8]) -> Result<Self, WireErr> {
        if b.len() < STATUS_HDR {
            return Err(WireErr::Short);
        }
        let (p, m) = (b[1] as usize, b[2] as usize);
        if b[0] > 1 || b[3] != 0 {
            return Err(WireErr::Malformed);
        }
        if p > NAME_MAX || m > NAME_MAX {
            return Err(WireErr::TooBig);
        }
        if b.len() != STATUS_HDR + p + m {
            return Err(WireErr::Malformed);
        }
        Ok(ChatStatus { ready: b[0] == 1, provider: &b[4..4 + p], model: &b[4 + p..] })
    }
}

// ── base64 (RFC 4648 standard alphabet, padded) ──────────────────────────────────────────────

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encoded length of `n` input bytes.
pub const fn b64_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

/// Encode `src` 3 bytes at a time, handing each 4-character group to `put` (streaming: no buffer).
pub fn b64_encode_with(src: &[u8], put: &mut dyn FnMut(&[u8])) {
    for c in src.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let mut g = [
            B64[(b[0] >> 2) as usize],
            B64[(((b[0] & 3) << 4) | (b[1] >> 4)) as usize],
            B64[(((b[1] & 15) << 2) | (b[2] >> 6)) as usize],
            B64[(b[2] & 63) as usize],
        ];
        if c.len() < 3 {
            g[3] = b'=';
        }
        if c.len() < 2 {
            g[2] = b'=';
        }
        put(&g);
    }
}

/// Encode into `out`; `None` if it does not fit.
pub fn b64_encode(src: &[u8], out: &mut [u8]) -> Option<usize> {
    let need = b64_len(src.len());
    if out.len() < need {
        return None;
    }
    let mut n = 0usize;
    b64_encode_with(src, &mut |g| {
        out[n..n + 4].copy_from_slice(g);
        n += 4;
    });
    Some(n)
}

fn b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode padded standard base64 into `out`. `None` on a bad length, a bad character, misplaced padding
/// or no room. Fail-closed: nothing partial is promised on `None`.
pub fn b64_decode(src: &[u8], out: &mut [u8]) -> Option<usize> {
    if src.len() % 4 != 0 {
        return None;
    }
    let mut n = 0usize;
    let groups = src.len() / 4;
    for (i, g) in src.chunks(4).enumerate() {
        let last = i + 1 == groups;
        let pad = g.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut v = [0u8; 4];
        for k in 0..4 - pad {
            v[k] = b64_val(g[k])?;
        }
        let bytes = [(v[0] << 2) | (v[1] >> 4), (v[1] << 4) | (v[2] >> 2), (v[2] << 6) | v[3]];
        let take = 3 - pad;
        if out.len() < n + take {
            return None;
        }
        out[n..n + take].copy_from_slice(&bytes[..take]);
        n += take;
    }
    Some(n)
}

// ── the [vein-relay] serial line format (the bench bridge) ───────────────────────────────────

/// The prefix VEIN.BIN writes before a relay request (the host bridge greps for it).
pub const RELAY_REQ_PREFIX: &[u8] = b"[vein-relay] REQ ";
/// Max base64 characters of one `vein rsp` shell line's body (a line stays ≤ 200 characters).
pub const RSP_B64_MAX: usize = 160;

/// Stream `[vein-relay] REQ <conv> <base64(text)>\n` to `put` in pieces (no buffer the size of the line).
pub fn relay_req_line(conv: u32, text: &[u8], put: &mut dyn FnMut(&[u8])) {
    put(RELAY_REQ_PREFIX);
    let mut d = [0u8; 10];
    put(dec_u32(conv, &mut d));
    put(b" ");
    b64_encode_with(text, put);
    put(b"\n");
}

/// Decimal of `v` into `buf`, returning the used tail.
pub fn dec_u32(mut v: u32, buf: &mut [u8; 10]) -> &[u8] {
    let mut i = buf.len();
    if v == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    &buf[i..]
}

fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: u64 = s.bytes().fold(0u64, |a, c| a * 10 + (c - b'0') as u64);
    u32::try_from(v).ok()
}

/// Parse the arguments of `vein rsp <conv> <seq> <done> <base64|->` and build the ChatReply BODY into
/// `out` (the frame the kernel injects as verb 131). Returns the body length.
pub fn rsp_args_to_body(args: &[&str], out: &mut [u8]) -> Result<usize, WireErr> {
    if args.len() != 4 {
        return Err(WireErr::Malformed);
    }
    let conv = parse_u32(args[0]).ok_or(WireErr::Malformed)?;
    let seq = parse_u32(args[1]).and_then(|s| u16::try_from(s).ok()).ok_or(WireErr::Malformed)?;
    let done = match args[2] {
        "0" => false,
        "1" => true,
        _ => return Err(WireErr::Malformed),
    };
    if out.len() < REPLY_HDR {
        return Err(WireErr::NoRoom);
    }
    let n = if args[3] == "-" {
        0
    } else {
        if args[3].len() > RSP_B64_MAX {
            return Err(WireErr::TooBig);
        }
        b64_decode(args[3].as_bytes(), &mut out[REPLY_HDR..]).ok_or(WireErr::Malformed)?
    };
    // The header is written last so the text decoded in place is not disturbed.
    let mut hdr = [0u8; REPLY_HDR];
    ChatReply { conv, seq, done, text: &[] }.encode(&mut hdr)?;
    out[..REPLY_HDR].copy_from_slice(&hdr);
    Ok(REPLY_HDR + n)
}

// ── KATs (frozen goldens, self-authored at this commit) ──────────────────────────────────────

/// ChatSend conv 7, "hi".
pub const GOLDEN_SEND: &[u8] = &[7, 0, 0, 0, b'h', b'i'];
/// ChatReply conv 7, seq 2, done 0, "ih".
pub const GOLDEN_REPLY_MORE: &[u8] = &[7, 0, 0, 0, 2, 0, 0, 0, b'i', b'h'];
/// ChatReply conv 7, seq 3, done 1, empty.
pub const GOLDEN_REPLY_DONE: &[u8] = &[7, 0, 0, 0, 3, 0, 1, 0];
/// ChatCancel conv 258.
pub const GOLDEN_CANCEL: &[u8] = &[2, 1, 0, 0];
/// ChatStatus ready, provider "echo", model "rev".
pub const GOLDEN_STATUS: &[u8] = &[1, 4, 3, 0, b'e', b'c', b'h', b'o', b'r', b'e', b'v'];

/// Run every codec KAT; returns `(passed, total)`. Pure — the kernel's `tests vein` and the host tests
/// call the same function.
pub fn kats() -> (u32, u32) {
    let mut pass = 0u32;
    let mut total = 0u32;
    let mut k = |ok: bool| {
        total += 1;
        pass += ok as u32;
    };
    let mut b = [0u8; 64];
    // send
    k(ChatSend { conv: 7, text: b"hi" }.encode(&mut b) == Ok(6) && &b[..6] == GOLDEN_SEND);
    k(ChatSend::decode(GOLDEN_SEND) == Ok(ChatSend { conv: 7, text: b"hi" }));
    k(ChatSend::decode(&GOLDEN_SEND[..4]) == Err(WireErr::Malformed) && ChatSend::decode(&[1, 2]) == Err(WireErr::Short));
    // reply
    let more = ChatReply { conv: 7, seq: 2, done: false, text: b"ih" };
    k(more.encode(&mut b) == Ok(10) && &b[..10] == GOLDEN_REPLY_MORE && more.status() == STATUS_MORE);
    let done = ChatReply { conv: 7, seq: 3, done: true, text: b"" };
    k(done.encode(&mut b) == Ok(8) && &b[..8] == GOLDEN_REPLY_DONE && done.status() == 0);
    k(ChatReply::decode(GOLDEN_REPLY_MORE) == Ok(more) && ChatReply::decode(GOLDEN_REPLY_DONE) == Ok(done));
    let mut bad = [0u8; 8];
    bad.copy_from_slice(GOLDEN_REPLY_DONE);
    bad[6] = 2;
    let mut bad2 = bad;
    bad2[6] = 1;
    bad2[7] = 9;
    k(ChatReply::decode(&bad) == Err(WireErr::Malformed) && ChatReply::decode(&bad2) == Err(WireErr::Malformed));
    // cancel
    k(ChatCancel { conv: 258 }.encode(&mut b) == Ok(4) && &b[..4] == GOLDEN_CANCEL && ChatCancel::decode(GOLDEN_CANCEL) == Ok(ChatCancel { conv: 258 }));
    k(ChatCancel::decode(&[1, 2, 3, 4, 5]) == Err(WireErr::Malformed));
    // status
    let st = ChatStatus { ready: true, provider: b"echo", model: b"rev" };
    k(st.encode(&mut b) == Ok(11) && &b[..11] == GOLDEN_STATUS && ChatStatus::decode(GOLDEN_STATUS) == Ok(st));
    k(status_request_ok(b"") && !status_request_ok(b"x") && ChatStatus::decode(&GOLDEN_STATUS[..10]) == Err(WireErr::Malformed));
    // base64 (RFC 4648 §10 vectors)
    let mut o = [0u8; 16];
    k(b64_encode(b"foobar", &mut o) == Some(8) && &o[..8] == b"Zm9vYmFy");
    k(b64_encode(b"fo", &mut o) == Some(4) && &o[..4] == b"Zm8=");
    k(b64_decode(b"Zm9vYg==", &mut o) == Some(4) && &o[..4] == b"foob");
    k(b64_decode(b"Zm9=vYg=", &mut o).is_none() && b64_decode(b"Zm9", &mut o).is_none() && b64_decode(b"Zm9!", &mut o).is_none());
    // relay line + rsp args
    let mut line = [0u8; 64];
    let mut n = 0usize;
    relay_req_line(7, b"hi", &mut |p| {
        line[n..n + p.len()].copy_from_slice(p);
        n += p.len();
    });
    k(&line[..n] == b"[vein-relay] REQ 7 aGk=\n");
    let mut body = [0u8; 64];
    k(rsp_args_to_body(&["7", "2", "0", "aWg="], &mut body) == Ok(10) && &body[..10] == GOLDEN_REPLY_MORE);
    k(rsp_args_to_body(&["7", "3", "1", "-"], &mut body) == Ok(8) && &body[..8] == GOLDEN_REPLY_DONE);
    k(rsp_args_to_body(&["7", "3", "2", "-"], &mut body).is_err() && rsp_args_to_body(&["x", "0", "1", "-"], &mut body).is_err());
    (pass, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kat_passes() {
        let (p, t) = kats();
        assert_eq!(p, t, "wire KATs {p}/{t}");
        assert!(t >= 19);
    }

    #[test]
    fn ceilings() {
        let big = [b'a'; SEND_TEXT_MAX + 1];
        let mut out = [0u8; BODY_MAX + 8];
        assert_eq!(ChatSend { conv: 1, text: &big }.encode(&mut out), Err(WireErr::TooBig));
        assert_eq!(ChatSend { conv: 1, text: &big[..SEND_TEXT_MAX] }.encode(&mut out), Ok(BODY_MAX));
        assert_eq!(ChatReply { conv: 1, seq: 0, done: true, text: &big[..REPLY_TEXT_MAX + 1] }.encode(&mut out), Err(WireErr::TooBig));
        assert_eq!(ChatSend::decode(&out[..BODY_MAX + 1]), Err(WireErr::TooBig));
    }

    #[test]
    fn b64_roundtrip_all_lengths() {
        let src: [u8; 40] = core::array::from_fn(|i| (i * 37 + 11) as u8);
        for n in 0..=src.len() {
            let mut e = [0u8; 64];
            let m = b64_encode(&src[..n], &mut e).unwrap();
            assert_eq!(m, b64_len(n));
            let mut d = [0u8; 40];
            assert_eq!(b64_decode(&e[..m], &mut d), Some(n));
            assert_eq!(&d[..n], &src[..n]);
        }
    }
}
