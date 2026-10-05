//! Content codings (RFC 9110 §8.4): `gzip` (RFC 1952), `deflate` (RFC 9110 §8.4.1.2 = zlib, RFC 1950, with the
//! bare-RFC 1951 fallback browsers apply) and `identity`, decoded through pixel_core's inflater — the ONE
//! DEFLATE decoder in UnaOS, shared with the kernel's SELFHOST and Facet's PNG path. `br` (RFC 7932) is OWED:
//! it is never advertised in `Accept-Encoding`, and a server that sends it anyway gets a named refusal.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use pixel_core::inflate::{self, ByteSource, InflateError, Sink};

use crate::headers::Headers;

/// What we put in `Accept-Encoding` (only what we can decode).
pub const ACCEPT_ENCODING: &str = "gzip, deflate";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coding {
    Identity,
    Gzip,
    Deflate,
    Brotli,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// A coding we do not implement (`br`, `zstd`, `compress`, …), named.
    Unsupported(String),
    /// The inflater refused the stream.
    Inflate(&'static str),
    /// The source failed mid-stream (the transport's error is reported by the caller).
    Source,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Unsupported(c) => write!(f, "unsupported content-coding {c}"),
            DecodeError::Inflate(r) => write!(f, "content decoding failed: {r}"),
            DecodeError::Source => f.write_str("body read failed during decoding"),
        }
    }
}

/// The codings of `Content-Encoding`, in the order they were applied.
pub fn content_codings(h: &Headers) -> Vec<Coding> {
    h.get_all("content-encoding")
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .map(|t| match t.as_str() {
            "identity" => Coding::Identity,
            "gzip" | "x-gzip" => Coding::Gzip,
            "deflate" => Coding::Deflate,
            "br" => Coding::Brotli,
            _ => Coding::Other(t),
        })
        .filter(|c| *c != Coding::Identity)
        .collect()
}

fn name(c: &Coding) -> String {
    match c {
        Coding::Identity => "identity".into(),
        Coding::Gzip => "gzip".into(),
        Coding::Deflate => "deflate".into(),
        Coding::Brotli => "br".into(),
        Coding::Other(s) => s.clone(),
    }
}

/// A source with up to two bytes of look-ahead (zlib header sniffing).
struct Peek<'a, S: ByteSource> {
    inner: &'a mut S,
    ahead: [Option<u8>; 2],
    n: usize,
}

impl<S: ByteSource> ByteSource for Peek<'_, S> {
    fn next(&mut self) -> Option<u8> {
        if self.n > 0 {
            let b = self.ahead[0];
            self.ahead[0] = self.ahead[1];
            self.ahead[1] = None;
            self.n -= 1;
            return b;
        }
        self.inner.next()
    }
}

fn reason(e: InflateError) -> DecodeError {
    DecodeError::Inflate(inflate::inflate_reason(e))
}

/// Stream-decode ONE coding from `src` into `sink`.
pub fn decode_stream<S: ByteSource, K: Sink>(coding: &Coding, src: &mut S, sink: &mut K) -> Result<u64, DecodeError> {
    match coding {
        Coding::Identity => {
            let mut n = 0;
            while let Some(b) = src.next() {
                sink.push(b).map_err(|_| DecodeError::Source)?;
                n += 1;
            }
            Ok(n)
        }
        Coding::Gzip => inflate::gunzip(src, sink).map(|r| r.uncompressed).map_err(reason),
        Coding::Deflate => {
            let a = src.next();
            let b = src.next();
            let zlib = match (a, b) {
                (Some(cmf), Some(flg)) => cmf & 0x0F == 8 && cmf >> 4 <= 7 && ((cmf as u32) << 8 | flg as u32) % 31 == 0,
                _ => false,
            };
            let n = a.is_some() as usize + b.is_some() as usize;
            let mut p = Peek { inner: src, ahead: [a, b], n };
            if zlib {
                inflate::zlib_inflate(&mut p, sink).map(|r| r.uncompressed).map_err(reason)
            } else {
                inflate::inflate_raw(&mut p, sink).map(|r| r.uncompressed).map_err(reason)
            }
        }
        other => Err(DecodeError::Unsupported(name(other))),
    }
}

struct Slice<'a>(&'a [u8], usize);
impl ByteSource for Slice<'_> {
    fn next(&mut self) -> Option<u8> {
        let b = self.0.get(self.1).copied();
        self.1 += 1;
        b
    }
}
struct VecSink<'a>(&'a mut Vec<u8>);
impl Sink for VecSink<'_> {
    fn push(&mut self, b: u8) -> Result<(), ()> {
        self.0.push(b);
        Ok(())
    }
}

/// Undo every coding in `codings` (applied in order, so removed in reverse) over a whole body.
pub fn decode(codings: &[Coding], body: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut cur: Vec<u8> = body.to_vec();
    for c in codings.iter().rev() {
        let mut out = Vec::with_capacity(cur.len() * 3);
        decode_stream(c, &mut Slice(&cur, 0), &mut VecSink(&mut out))?;
        cur = out;
    }
    Ok(cur)
}
