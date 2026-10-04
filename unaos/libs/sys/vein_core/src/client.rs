// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! One exchange with the provider over a [`Transport`]: send the request, read the response head, frame
//! the body (chunked or not), decode the event stream, hand every [`Event`] to the caller as it arrives.
//! The transport is the caller's: `vein_ring3` gives the TCP and TLS ones over the ring-3 syscalls, a
//! host test gives a fake. Nothing here blocks except inside the transport.

use crate::claude::{self, Event, Stop, StreamDecoder};
use crate::http::{self, Chunked};

/// A byte stream to the endpoint (already connected; plain TCP or TLS).
pub trait Transport {
    /// Write every byte, or fail with a negative errno.
    fn send_all(&mut self, b: &[u8]) -> Result<(), i64>;
    /// Read at least one byte; `Ok(0)` only at end of stream.
    fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    Send(i64),
    Recv(i64),
    /// No complete, parseable head in the receive buffer.
    Head,
    /// Malformed chunked framing.
    Framing,
    /// A non-200 status (the message, if any, went out as [`Event::Error`]).
    Http(u16),
    /// The stream ended before `message_stop`.
    Truncated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub status: u16,
    pub stop: Option<Stop>,
    pub text_bytes: usize,
    pub retry_after: Option<u32>,
    pub fail: Option<Fail>,
}

enum Body<'b> {
    Sse(StreamDecoder<'b>),
    Err { buf: &'b mut [u8], n: usize },
}

/// Run one request. `rx` is the receive buffer (it must hold the whole response head; 2 KiB is ample),
/// `line` the stream decoder's line buffer (also the error body's buffer on a non-200).
pub fn exchange<T: Transport + ?Sized>(t: &mut T, head: &[u8], body: &[u8], rx: &mut [u8], line: &mut [u8], on: &mut dyn FnMut(Event<'_>)) -> Outcome {
    let mut out = Outcome { status: 0, stop: None, text_bytes: 0, retry_after: None, fail: None };
    if let Err(e) = t.send_all(head).and_then(|_| t.send_all(body)) {
        out.fail = Some(Fail::Send(e));
        return out;
    }
    // The head.
    let mut have = 0;
    let h = loop {
        if let Some(h) = http::parse_head(&rx[..have]) {
            break h;
        }
        if have == rx.len() || (have >= 4 && http::head_end(&rx[..have]).is_some()) {
            out.fail = Some(Fail::Head);
            return out;
        }
        match t.recv(&mut rx[have..]) {
            Ok(0) => {
                out.fail = Some(Fail::Head);
                return out;
            }
            Ok(n) => have += n,
            Err(e) => {
                out.fail = Some(Fail::Recv(e));
                return out;
            }
        }
    };
    out.status = h.status;
    out.retry_after = h.retry_after;
    let mut b = if h.status == 200 { Body::Sse(StreamDecoder::new(line)) } else { Body::Err { buf: line, n: 0 } };
    let mut ck = Chunked::new();
    let mut left = h.content_length;
    let mut framing_bad = false;
    let feed = |b: &mut Body<'_>, p: &[u8], on: &mut dyn FnMut(Event<'_>)| match b {
        Body::Sse(d) => d.feed(p, on),
        Body::Err { buf, n } => {
            let k = p.len().min(buf.len() - *n);
            buf[*n..*n + k].copy_from_slice(&p[..k]);
            *n += k;
        }
    };
    let finished = |b: &Body<'_>, ck: &Chunked, left: Option<usize>| -> bool {
        matches!(b, Body::Sse(d) if d.done) || (h.chunked && ck.done()) || (!h.chunked && left == Some(0))
    };
    let mut start = h.head_len;
    let mut end = have;
    loop {
        let chunk = &rx[start..end];
        if h.chunked {
            if !ck.feed(chunk, &mut |p| feed(&mut b, p, on)) {
                framing_bad = true;
                break;
            }
        } else {
            let take = left.map_or(chunk.len(), |l| l.min(chunk.len()));
            feed(&mut b, &chunk[..take], on);
            left = left.map(|l| l - take);
        }
        if finished(&b, &ck, left) {
            break;
        }
        match t.recv(rx) {
            Ok(0) => break,
            Ok(n) => {
                start = 0;
                end = n;
            }
            Err(e) => {
                out.fail = Some(Fail::Recv(e));
                break;
            }
        }
    }
    match b {
        Body::Sse(mut d) => {
            d.finish(on);
            out.stop = d.stop;
            out.text_bytes = d.text_bytes;
            if out.fail.is_none() {
                out.fail = if framing_bad {
                    Some(Fail::Framing)
                } else if !d.done {
                    Some(Fail::Truncated)
                } else {
                    None
                };
            }
        }
        Body::Err { buf, n } => {
            match claude::error_message(&mut buf[..n]) {
                Some(m) => on(Event::Error(m)),
                None => on(Event::Error("the endpoint answered with an error status")),
            }
            out.fail = Some(Fail::Http(h.status));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    /// A fake transport: records what was sent, replays `wire` in `step`-byte reads.
    struct Fake {
        sent: Vec<u8>,
        wire: Vec<u8>,
        pos: usize,
        step: usize,
    }
    impl Transport for Fake {
        fn send_all(&mut self, b: &[u8]) -> Result<(), i64> {
            self.sent.extend_from_slice(b);
            Ok(())
        }
        fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64> {
            let n = self.step.min(b.len()).min(self.wire.len() - self.pos);
            b[..n].copy_from_slice(&self.wire[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn chunk(s: &str) -> String {
        let mut o = String::new();
        for c in s.as_bytes().chunks(37) {
            o.push_str(&std::format!("{:x}\r\n", c.len()));
            o.push_str(core::str::from_utf8(c).unwrap());
            o.push_str("\r\n");
        }
        o.push_str("0\r\n\r\n");
        o
    }

    const SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi from \"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Claude\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    fn go(wire: String, step: usize) -> (Outcome, String, Vec<String>, Vec<u8>) {
        let mut f = Fake { sent: Vec::new(), wire: wire.into_bytes(), pos: 0, step };
        let (mut rx, mut line) = ([0u8; 512], [0u8; 512]);
        let (mut text, mut errs) = (String::new(), Vec::new());
        let o = exchange(&mut f, b"HEAD", b"BODY", &mut rx, &mut line, &mut |e| match e {
            Event::Text(t) => text.push_str(t),
            Event::Error(m) => errs.push(String::from(m)),
            _ => {}
        });
        (o, text, errs, f.sent)
    }

    #[test]
    fn chunked_sse_streams_text_in_any_read_size() {
        for step in [1, 3, 64, 4096] {
            let wire = std::format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{}", chunk(SSE));
            let (o, text, errs, sent) = go(wire, step);
            assert_eq!(sent, b"HEADBODY");
            assert_eq!(text, "Hi from Claude", "step {step}");
            assert!(errs.is_empty());
            assert_eq!(o, Outcome { status: 200, stop: Some(Stop::EndTurn), text_bytes: 14, retry_after: None, fail: None });
        }
    }

    #[test]
    fn plain_body_and_truncation() {
        let wire = std::format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", SSE.len(), SSE);
        assert_eq!(go(wire, 7).0.fail, None);
        let cut = &SSE[..SSE.len() - 40];
        let wire = std::format!("HTTP/1.1 200 OK\r\n\r\n{}", cut);
        let (o, text, _, _) = go(wire, 7);
        assert_eq!(text, "Hi from Claude");
        assert_eq!(o.fail, Some(Fail::Truncated));
    }

    #[test]
    fn http_errors_carry_the_message_and_retry_after() {
        let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
        let wire = std::format!("HTTP/1.1 429 Too Many Requests\r\nretry-after: 12\r\ncontent-length: {}\r\n\r\n{}", body.len(), body);
        let (o, text, errs, _) = go(wire, 5);
        assert!(text.is_empty());
        assert_eq!(errs, ["slow down"]);
        assert_eq!((o.status, o.retry_after, o.fail), (429, Some(12), Some(Fail::Http(429))));
        let (o, _, _, _) = go(String::from("garbage with no head"), 5);
        assert_eq!(o.fail, Some(Fail::Head));
    }
}
