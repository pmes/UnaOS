// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HTTP/1.1 framing for one request/response over a byte stream: the response head parser and the
//! incremental chunked transfer decoder. No allocation; the caller owns every buffer.

/// The parts of a response head the client acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub chunked: bool,
    pub content_length: Option<usize>,
    /// `retry-after` in whole seconds, when the server sent one.
    pub retry_after: Option<u32>,
    /// Bytes of head including the blank line.
    pub head_len: usize,
}

/// Where the head ends (index just past `\r\n\r\n`), if it has fully arrived.
pub fn head_end(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn eq_ic(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn trim(mut v: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', r @ ..] = v {
        v = r;
    }
    while let [r @ .., b' ' | b'\t' | b'\r'] = v {
        v = r;
    }
    v
}

fn num(v: &[u8]) -> Option<usize> {
    if v.is_empty() || v.len() > 18 {
        return None;
    }
    v.iter().try_fold(0usize, |a, &c| if c.is_ascii_digit() { Some(a * 10 + (c - b'0') as usize) } else { None })
}

/// Parse a complete head (`b` holds at least [`head_end`] bytes).
pub fn parse_head(b: &[u8]) -> Option<ResponseHead> {
    let head_len = head_end(b)?;
    let mut lines = b[..head_len - 4].split(|&c| c == b'\n');
    let st = trim(lines.next()?);
    if st.len() < 12 || !st.starts_with(b"HTTP/1.") || st[8] != b' ' {
        return None;
    }
    let status = num(&st[9..12])? as u16;
    let mut h = ResponseHead { status, chunked: false, content_length: None, retry_after: None, head_len };
    for l in lines {
        let Some(c) = l.iter().position(|&x| x == b':') else { continue };
        let (k, v) = (trim(&l[..c]), trim(&l[c + 1..]));
        if eq_ic(k, b"transfer-encoding") {
            h.chunked = v.split(|&x| x == b',').any(|t| eq_ic(trim(t), b"chunked"));
        } else if eq_ic(k, b"content-length") {
            h.content_length = num(v);
        } else if eq_ic(k, b"retry-after") {
            h.retry_after = num(v).map(|n| n.min(u32::MAX as usize) as u32);
        }
    }
    Some(h)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ck {
    Size,
    Ext,
    SizeLf,
    Data(usize),
    DataCr,
    DataLf,
    Trailer { blank: bool },
    Done,
}

/// The `Transfer-Encoding: chunked` decoder, fed arbitrary slices.
pub struct Chunked {
    st: Ck,
    size: usize,
    digits: u8,
    pub bad: bool,
}

impl Default for Chunked {
    fn default() -> Self {
        Self::new()
    }
}

impl Chunked {
    pub const fn new() -> Self {
        Chunked { st: Ck::Size, size: 0, digits: 0, bad: false }
    }
    pub fn done(&self) -> bool {
        self.st == Ck::Done
    }
    /// Feed bytes; each run of payload is handed to `out`. Returns false once the framing is malformed.
    pub fn feed(&mut self, mut b: &[u8], out: &mut dyn FnMut(&[u8])) -> bool {
        while !b.is_empty() && !self.bad && self.st != Ck::Done {
            match self.st {
                Ck::Data(rem) => {
                    let n = rem.min(b.len());
                    out(&b[..n]);
                    b = &b[n..];
                    self.st = if rem == n { Ck::DataCr } else { Ck::Data(rem - n) };
                    continue;
                }
                _ => {}
            }
            let c = b[0];
            b = &b[1..];
            self.st = match (self.st, c) {
                (Ck::Size, b'\r') if self.digits > 0 => Ck::SizeLf,
                (Ck::Size, b';' | b' ' | b'\t') if self.digits > 0 => Ck::Ext,
                (Ck::Size, _) => match (c as char).to_digit(16) {
                    Some(d) if self.digits < 15 => {
                        self.size = self.size * 16 + d as usize;
                        self.digits += 1;
                        Ck::Size
                    }
                    _ => {
                        self.bad = true;
                        Ck::Size
                    }
                },
                (Ck::Ext, b'\r') => Ck::SizeLf,
                (Ck::Ext, _) => Ck::Ext,
                (Ck::SizeLf, b'\n') => {
                    let s = self.size;
                    self.size = 0;
                    self.digits = 0;
                    if s == 0 { Ck::Trailer { blank: true } } else { Ck::Data(s) }
                }
                (Ck::DataCr, b'\r') => Ck::DataLf,
                (Ck::DataLf, b'\n') => Ck::Size,
                (Ck::Trailer { blank }, b'\n') => {
                    if blank { Ck::Done } else { Ck::Trailer { blank: true } }
                }
                (Ck::Trailer { .. }, b'\r') => self.st,
                (Ck::Trailer { .. }, _) => Ck::Trailer { blank: false },
                _ => {
                    self.bad = true;
                    self.st
                }
            };
        }
        !self.bad
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    #[test]
    fn head_parses_status_chunked_and_retry_after() {
        let h = b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nRetry-After: 7\r\n\r\nrest";
        let p = parse_head(h).unwrap();
        assert_eq!((p.status, p.chunked, p.retry_after, p.content_length), (429, true, Some(7), None));
        assert_eq!(&h[p.head_len..], b"rest");
        assert!(parse_head(b"HTTP/1.1 200 OK\r\n").is_none());
        assert_eq!(parse_head(b"HTTP/1.0 200 OK\r\ncontent-length: 12\r\n\r\n").unwrap().content_length, Some(12));
    }

    #[test]
    fn chunked_decodes_across_every_split() {
        let wire = b"4\r\nWiki\r\n6;ext=1\r\npedia \r\nE\r\nin \r\n\r\nchunks.\r\n0\r\nX-T: y\r\n\r\n";
        for split in 0..wire.len() {
            let mut d = Chunked::new();
            let mut got = Vec::new();
            assert!(d.feed(&wire[..split], &mut |p| got.extend_from_slice(p)));
            assert!(d.feed(&wire[split..], &mut |p| got.extend_from_slice(p)));
            assert!(d.done(), "split {split}");
            assert_eq!(got, b"Wikipedia in \r\n\r\nchunks.");
        }
        let mut d = Chunked::new();
        assert!(!d.feed(b"zz\r\n", &mut |_| {}));
    }
}
