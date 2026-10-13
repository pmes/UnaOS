//! Content codings (RFC 9110 §8.4): `gzip` (RFC 1952), `deflate` (RFC 9110 §8.4.1.2 = zlib, RFC 1950, with the
//! bare-RFC 1951 fallback browsers apply), `br` (RFC 7932, Brotli) and `identity`. gzip/deflate go through
//! pixel_core's inflater — the ONE DEFLATE decoder in UnaOS, shared with the kernel's SELFHOST and Facet's PNG path;
//! `br` is decoded here, from the RFC (NETDEBT2, LEDGER SR113): streaming over the same `ByteSource` / `Sink` pair,
//! no_std + alloc, the static dictionary as data (`brotli_dictionary.dict`). All three are advertised in
//! [`ACCEPT_ENCODING`], which BOTH rings send (the host transport, http_core `host`; AETHER.ELF, user-aether net.rs).

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use pixel_core::inflate::{self, ByteSource, InflateError, Sink};

use crate::headers::Headers;

/// What we put in `Accept-Encoding` (only what we can decode) — on the host and on the image alike.
pub const ACCEPT_ENCODING: &str = "gzip, deflate, br";

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
    /// A coding we do not implement (`zstd`, `compress`, …), named.
    Unsupported(String),
    /// The inflater refused the stream.
    Inflate(&'static str),
    /// The Brotli decoder refused the stream (RFC 7932), with the rule it broke.
    Brotli(&'static str),
    /// The source failed mid-stream (the transport's error is reported by the caller).
    Source,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Unsupported(c) => write!(f, "unsupported content-coding {c}"),
            DecodeError::Inflate(r) => write!(f, "content decoding failed: {r}"),
            DecodeError::Brotli(r) => write!(f, "content decoding failed (br): {r}"),
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
        Coding::Brotli => brotli(src, sink).map_err(|e| match e {
            BrErr::Format(r) => DecodeError::Brotli(r),
            BrErr::Sink => DecodeError::Source,
        }),
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

// ==== br — Brotli (RFC 7932) =====================================================================================
//
// MECHANISM (written before the code, R106). One pass over the coded bytes, LSB-first bits (§2), pulling from the same
// `ByteSource` the inflater reads, pushing every output byte to the caller's `Sink` AND to a history ring of
// `1 << WBITS` bytes (grown as output arrives, so a 9-byte page never allocates the 16 MiB window):
// 1. Stream header (§9.1): WBITS from the 1/3/3-bit ladder; the large-window escape (RFC 7932 has none) is refused.
// 2. Meta-blocks (§9.2) until ISLAST: ISLASTEMPTY, MNIBBLES (3 = metadata: skipped to the next byte boundary, padding
//    checked zero), MLEN, ISUNCOMPRESSED (bytes copied after the boundary); else a compressed header — per category
//    L/I/D: NBLTYPES (VarLenUint8 + 1) with its block-type and block-count prefix codes and first count (§6); NPOSTFIX,
//    NDIRECT; one context mode per literal block type (§7.1); the literal and distance context maps (§7.3: RLE of
//    zeros, inverse move-to-front); then the prefix codes (§3: simple NSYM 1..4 with tree-select, or complex — the
//    code-length code read through the fixed variable-length code in the order 1,2,3,4,0,5,17,6,16,7,8..15, then
//    the symbol lengths with repeat codes 16/17, each code's Kraft sum checked exactly).
// 3. Commands (§5, §9.3): an insert-and-copy symbol (its cell gives the insert/copy length codes and whether the
//    distance is the implicit last one), ILEN literals each under the tree chosen by the 64-entry context of the two
//    previous bytes (LSB6/MSB6/UTF8/Signed with the RFC's Lut0/Lut1/Lut2), then a distance (§4: the 16 short codes
//    over the four-distance ring {4,11,15,16}, NDIRECT direct codes, then the postfix/extra-bit codes) under the tree
//    the copy length's distance context chooses. A distance past min(window, bytes produced) is a static dictionary
//    reference (§8): word = dictionary[DOFFSET[len] + idx * len], idx/transform split by NDBITS[len], one of the 121
//    transforms (prefix, identity / omit-first-n / omit-last-n / uppercase-first / uppercase-all, suffix). The ring
//    takes every distance except code 0 and dictionary references. Every length is checked against what the
//    meta-block has left; a bad code, a broken Kraft sum, an over-long run or a truncated stream is a named refusal.
//
// DATA (§8, Appendix A/B): the dictionary is `brotli_dictionary.dict` (122,784 bytes, sha256
// 20e42eb1b511c21806d4d227d07e5dd06877d8ce7b3a817f378f313653f35c70 — Appendix A's own checksum); NDBITS/DOFFSET are
// Appendix A's size table; the 121 transforms are Appendix B's list; Lut0/Lut1/Lut2 are §7.1's tables. Each table was
// taken out of the system's reference libbrotlicommon and checked against the RFC text (the transforms against
// `BrotliTransformDictionaryWord` over every id), the KATs below decode against the reference decoder.

/// RFC 7932 Appendix A: the static dictionary.
static BR_DICT: &[u8; 122_784] = include_bytes!("brotli_dictionary.dict");
/// Appendix A: NDBITS by word length (4..=24).
const BR_NDBITS: [u8; 25] = [0, 0, 0, 0, 10, 10, 11, 11, 10, 10, 10, 10, 10, 9, 9, 8, 7, 7, 8, 7, 7, 6, 6, 5, 5];
/// Appendix A: DOFFSET by word length (4..=24).
const BR_DOFFSET: [u32; 25] = [
    0, 0, 0, 0, 0, 4096, 9216, 21504, 35840, 44032, 53248, 63488, 74752, 87040, 93696, 100864, 104704, 106752, 108928,
    113536, 115968, 118528, 119872, 121280, 122016,
];
const _: () = assert!(BR_DOFFSET[24] as usize + (24 << BR_NDBITS[24]) == 122_784);

/// §6: block count codes — (base, extra bits).
const BR_BLEN: [(u32, u32); 26] = [
    (1, 2), (5, 2), (9, 2), (13, 2), (17, 3), (25, 3), (33, 3), (41, 3), (49, 4), (65, 4), (81, 4), (97, 4), (113, 5),
    (145, 5), (177, 5), (209, 5), (241, 6), (305, 6), (369, 7), (497, 8), (753, 9), (1265, 10), (2289, 11), (4337, 12),
    (8433, 13), (16625, 24),
];
/// §5: insert length codes — (base, extra bits).
const BR_INS: [(u32, u32); 24] = [
    (0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 1), (8, 1), (10, 2), (14, 2), (18, 3), (26, 3), (34, 4), (50, 4),
    (66, 5), (98, 5), (130, 6), (194, 7), (322, 8), (578, 9), (1090, 10), (2114, 12), (6210, 14), (22594, 24),
];
/// §5: copy length codes — (base, extra bits).
const BR_CPY: [(u32, u32); 24] = [
    (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0), (9, 0), (10, 1), (12, 1), (14, 2), (18, 2), (22, 3), (30, 3),
    (38, 4), (54, 4), (70, 5), (102, 5), (134, 6), (198, 7), (326, 8), (582, 9), (1094, 10), (2118, 24),
];
/// §5: the insert-and-copy cells (symbol >> 6) — (insert code base, copy code base); cells 0 and 1 carry the
/// implicit distance code 0.
const BR_CELL: [(usize, usize); 11] = [(0, 0), (0, 8), (0, 0), (0, 8), (8, 0), (8, 8), (0, 16), (16, 0), (8, 16), (16, 8), (16, 16)];
/// §3.5: the order the code-length code lengths are sent in.
const BR_CL_ORDER: [usize; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

// §7.1: the context lookup tables.
const LUT0: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    8, 12, 16, 12, 12, 20, 12, 16, 24, 28, 12, 12, 32, 12, 36, 12, 44, 44, 44, 44, 44, 44, 44, 44, 44, 44, 32, 32, 24, 40, 28, 12,
    12, 48, 52, 52, 52, 48, 52, 52, 52, 48, 52, 52, 52, 52, 52, 48, 52, 52, 52, 52, 52, 48, 52, 52, 52, 52, 52, 24, 12, 28, 12, 12,
    12, 56, 60, 60, 60, 56, 60, 60, 60, 56, 60, 60, 60, 60, 60, 56, 60, 60, 60, 60, 60, 56, 60, 60, 60, 60, 60, 24, 12, 28, 12, 0,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3,
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3,
];
const LUT1: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1,
    1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1,
    1, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 1, 1, 1, 1, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
];
const LUT2: [u8; 256] = [
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7,
];
// Appendix B: transform types.
const ID: u8 = 0;
const OL1: u8 = 1;
const OL2: u8 = 2;
const OL3: u8 = 3;
const OL4: u8 = 4;
const OL5: u8 = 5;
const OL6: u8 = 6;
const OL7: u8 = 7;
const OL8: u8 = 8;
const OL9: u8 = 9;
const UF: u8 = 10;
const UA: u8 = 11;
const OF1: u8 = 12;
const OF2: u8 = 13;
const OF3: u8 = 14;
const OF4: u8 = 15;
const OF5: u8 = 16;
const OF6: u8 = 17;
const OF7: u8 = 18;
// 19 (omit-first-8) is a type no transform of Appendix B uses.
const OF9: u8 = 20;

/// Appendix B: the 121 word transforms — (prefix, type, suffix).
const TRANSFORMS: [(&[u8], u8, &[u8]); 121] = [
    (b"", ID, b""), (b"", ID, b" "), (b" ", ID, b" "),
    (b"", OF1, b""), (b"", UF, b" "), (b"", ID, b" the "),
    (b" ", ID, b""), (b"s ", ID, b" "), (b"", ID, b" of "),
    (b"", UF, b""), (b"", ID, b" and "), (b"", OF2, b""),
    (b"", OL1, b""), (b", ", ID, b" "), (b"", ID, b", "),
    (b" ", UF, b" "), (b"", ID, b" in "), (b"", ID, b" to "),
    (b"e ", ID, b" "), (b"", ID, b"\""), (b"", ID, b"."),
    (b"", ID, b"\">"), (b"", ID, b"\n"), (b"", OL3, b""),
    (b"", ID, b"]"), (b"", ID, b" for "), (b"", OF3, b""),
    (b"", OL2, b""), (b"", ID, b" a "), (b"", ID, b" that "),
    (b" ", UF, b""), (b"", ID, b". "), (b".", ID, b""),
    (b" ", ID, b", "), (b"", OF4, b""), (b"", ID, b" with "),
    (b"", ID, b"'"), (b"", ID, b" from "), (b"", ID, b" by "),
    (b"", OF5, b""), (b"", OF6, b""), (b" the ", ID, b""),
    (b"", OL4, b""), (b"", ID, b". The "), (b"", UA, b""),
    (b"", ID, b" on "), (b"", ID, b" as "), (b"", ID, b" is "),
    (b"", OL7, b""), (b"", OL1, b"ing "), (b"", ID, b"\n\t"),
    (b"", ID, b":"), (b" ", ID, b". "), (b"", ID, b"ed "),
    (b"", OF9, b""), (b"", OF7, b""), (b"", OL6, b""),
    (b"", ID, b"("), (b"", UF, b", "), (b"", OL8, b""),
    (b"", ID, b" at "), (b"", ID, b"ly "), (b" the ", ID, b" of "),
    (b"", OL5, b""), (b"", OL9, b""), (b" ", UF, b", "),
    (b"", UF, b"\""), (b".", ID, b"("), (b"", UA, b" "),
    (b"", UF, b"\">"), (b"", ID, b"=\""), (b" ", ID, b"."),
    (b".com/", ID, b""), (b" the ", ID, b" of the "), (b"", UF, b"'"),
    (b"", ID, b". This "), (b"", ID, b","), (b".", ID, b" "),
    (b"", UF, b"("), (b"", UF, b"."), (b"", ID, b" not "),
    (b" ", ID, b"=\""), (b"", ID, b"er "), (b" ", UA, b" "),
    (b"", ID, b"al "), (b" ", UA, b""), (b"", ID, b"='"),
    (b"", UA, b"\""), (b"", UF, b". "), (b" ", ID, b"("),
    (b"", ID, b"ful "), (b" ", UF, b". "), (b"", ID, b"ive "),
    (b"", ID, b"less "), (b"", UA, b"'"), (b"", ID, b"est "),
    (b" ", UF, b"."), (b"", UA, b"\">"), (b" ", ID, b"='"),
    (b"", UF, b","), (b"", ID, b"ize "), (b"", UA, b"."),
    (b"\xc2\xa0", ID, b""), (b" ", ID, b","), (b"", UF, b"=\""),
    (b"", UA, b"=\""), (b"", ID, b"ous "), (b"", UA, b", "),
    (b"", UF, b"='"), (b" ", UF, b","), (b" ", UA, b"=\""),
    (b" ", UA, b", "), (b"", UA, b","), (b"", UA, b"("),
    (b"", UA, b". "), (b" ", UA, b"."), (b"", UA, b"='"),
    (b" ", UA, b". "), (b" ", UF, b"=\""), (b" ", UA, b"='"),
    (b" ", UF, b"='"),
];
enum BrErr {
    Format(&'static str),
    Sink,
}

type BrResult<T> = Result<T, BrErr>;

fn bad<T>(why: &'static str) -> BrResult<T> {
    Err(BrErr::Format(why))
}

/// LSB-first bits over a pull source (§2).
struct Bits<'a, S: ByteSource> {
    src: &'a mut S,
    acc: u64,
    n: u32,
}

impl<S: ByteSource> Bits<'_, S> {
    fn read(&mut self, k: u32) -> BrResult<u32> {
        if k == 0 {
            return Ok(0);
        }
        while self.n < k {
            let Some(b) = self.src.next() else { return bad("truncated stream") };
            self.acc |= (b as u64) << self.n;
            self.n += 8;
        }
        let v = (self.acc & ((1u64 << k) - 1)) as u32;
        self.acc >>= k;
        self.n -= k;
        Ok(v)
    }

    /// Skip to the next byte boundary; the skipped bits must be zero.
    fn align(&mut self) -> BrResult<()> {
        let r = self.n % 8;
        if self.read(r)? != 0 {
            return bad("nonzero padding bits");
        }
        Ok(())
    }
}

/// A canonical prefix code (§3.2), decoded MSB-of-code first — one symbol of zero length when `single`.
struct Huff {
    count: [u16; 16],
    sym: Vec<u16>,
    single: Option<u16>,
}

impl Huff {
    fn from_lengths(lens: &[u8]) -> BrResult<Huff> {
        let mut count = [0u16; 16];
        let mut last = 0usize;
        for (s, &l) in lens.iter().enumerate() {
            if l > 15 {
                return bad("code length over 15");
            }
            if l != 0 {
                count[l as usize] += 1;
                last = s;
            }
        }
        let used: u32 = count[1..].iter().map(|&c| c as u32).sum();
        if used == 0 {
            return bad("empty prefix code");
        }
        if used == 1 {
            return Ok(Huff { count, sym: Vec::new(), single: Some(last as u16) });
        }
        let mut offs = [0usize; 16];
        for l in 1..15 {
            offs[l + 1] = offs[l] + count[l] as usize;
        }
        let mut sym = alloc::vec![0u16; used as usize];
        for (s, &l) in lens.iter().enumerate() {
            if l != 0 {
                sym[offs[l as usize]] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Huff { count, sym, single: None })
    }

    fn single(s: u16) -> Huff {
        Huff { count: [0; 16], sym: Vec::new(), single: Some(s) }
    }

    fn decode<S: ByteSource>(&self, b: &mut Bits<'_, S>) -> BrResult<u16> {
        if let Some(s) = self.single {
            return Ok(s);
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.read(1)? as i32;
            let c = self.count[len] as i32;
            if code - first < c {
                return Ok(self.sym[(index + code - first) as usize]);
            }
            index += c;
            first += c;
            first <<= 1;
            code <<= 1;
        }
        bad("invalid prefix code")
    }
}

/// §3.5: one code length of the code-length code, through its fixed variable-length code.
fn br_cl_len<S: ByteSource>(b: &mut Bits<'_, S>) -> BrResult<u8> {
    Ok(match b.read(2)? {
        0 => 0,
        1 => 4,
        2 => 3,
        _ => match b.read(1)? {
            0 => 2,
            _ => {
                if b.read(1)? == 0 {
                    1
                } else {
                    5
                }
            }
        },
    })
}

/// §3.2–§3.5: one prefix code over an alphabet of `size` symbols.
fn br_prefix<S: ByteSource>(b: &mut Bits<'_, S>, size: usize) -> BrResult<Huff> {
    let hskip = b.read(2)?;
    if hskip == 1 {
        // §3.4 simple prefix code.
        let nsym = b.read(2)? as usize + 1;
        let mut abits = 0;
        while (1usize << abits) < size {
            abits += 1;
        }
        let mut syms = [0u16; 4];
        for i in 0..nsym {
            let s = b.read(abits)? as usize;
            if s >= size {
                return bad("simple code symbol out of range");
            }
            if syms[..i].contains(&(s as u16)) {
                return bad("simple code symbol repeated");
            }
            syms[i] = s as u16;
        }
        if nsym == 1 {
            return Ok(Huff::single(syms[0]));
        }
        let mut lens = alloc::vec![0u8; size];
        let shape: &[u8] = match nsym {
            2 => &[1, 1],
            3 => &[1, 2, 2],
            _ => {
                if b.read(1)? == 0 {
                    &[2, 2, 2, 2]
                } else {
                    &[1, 2, 3, 3]
                }
            }
        };
        for (i, &l) in shape.iter().enumerate() {
            lens[syms[i] as usize] = l;
        }
        return Huff::from_lengths(&lens);
    }
    // §3.5 complex prefix code: the code-length code first.
    let mut cl = [0u8; 18];
    let (mut space, mut num) = (32i32, 0);
    for &idx in &BR_CL_ORDER[hskip as usize..] {
        let v = br_cl_len(b)?;
        cl[idx] = v;
        if v != 0 {
            space -= 32 >> v;
            num += 1;
            if space <= 0 {
                break;
            }
        }
    }
    if num != 1 && space != 0 {
        return bad("code length code is not complete");
    }
    let clh = Huff::from_lengths(&cl)?;
    let mut lens = alloc::vec![0u8; size];
    let (mut sym, mut prev, mut repeat, mut repeat_len) = (0usize, 8u8, 0usize, 0u8);
    let mut space = 32768i32;
    while sym < size && space > 0 {
        let c = clh.decode(b)? as u8;
        if c < 16 {
            lens[sym] = c;
            sym += 1;
            repeat = 0;
            if c != 0 {
                prev = c;
                space -= 32768 >> c;
            }
            continue;
        }
        let (new_len, extra) = if c == 16 { (prev, 2) } else { (0, 3) };
        if repeat_len != new_len {
            repeat = 0;
            repeat_len = new_len;
        }
        let old = repeat;
        if repeat > 0 {
            repeat = (repeat - 2) << extra;
        }
        repeat += b.read(extra)? as usize + 3;
        let delta = repeat - old;
        if sym + delta > size {
            return bad("code length repeat past the alphabet");
        }
        lens[sym..sym + delta].fill(new_len);
        sym += delta;
        if new_len != 0 {
            space -= (delta as i32) * (32768 >> new_len);
        }
    }
    if space != 0 {
        return bad("prefix code is not complete");
    }
    Huff::from_lengths(&lens)
}

/// §9.2 VarLenUint8.
fn br_varlen8<S: ByteSource>(b: &mut Bits<'_, S>) -> BrResult<u32> {
    if b.read(1)? == 0 {
        return Ok(0);
    }
    let n = b.read(3)?;
    if n == 0 {
        return Ok(1);
    }
    Ok((1 << n) + b.read(n)?)
}

/// One category's block switching (§6): its type count, codes, the remaining count and the last two types.
struct Blocks {
    n: u32,
    types: Option<(Huff, Huff)>,
    left: u32,
    last: u32,
    prev: u32,
}

impl Blocks {
    fn read<S: ByteSource>(b: &mut Bits<'_, S>) -> BrResult<Blocks> {
        let n = br_varlen8(b)? + 1;
        if n < 2 {
            return Ok(Blocks { n, types: None, left: u32::MAX, last: 0, prev: 1 });
        }
        let t = br_prefix(b, n as usize + 2)?;
        let c = br_prefix(b, 26)?;
        let left = br_block_len(b, &c)?;
        Ok(Blocks { n, types: Some((t, c)), left, last: 0, prev: 1 })
    }

    /// Count one command (or literal, or distance) against the current block, switching first when it is used up.
    fn step<S: ByteSource>(&mut self, b: &mut Bits<'_, S>) -> BrResult<usize> {
        if self.left == 0 {
            let Some((t, c)) = &self.types else { return bad("block count exhausted") };
            let code = t.decode(b)? as u32;
            let ty = match code {
                0 => self.prev,
                1 => (self.last + 1) % self.n,
                c => c - 2,
            };
            if ty >= self.n {
                return bad("block type out of range");
            }
            self.prev = self.last;
            self.last = ty;
            self.left = br_block_len(b, c)?;
        }
        self.left -= 1;
        Ok(self.last as usize)
    }
}

fn br_block_len<S: ByteSource>(b: &mut Bits<'_, S>, c: &Huff) -> BrResult<u32> {
    let (base, extra) = BR_BLEN[c.decode(b)? as usize];
    Ok(base + b.read(extra)?)
}

/// §7.3: a context map of `size` entries over `ntrees` trees.
fn br_context_map<S: ByteSource>(b: &mut Bits<'_, S>, size: usize, ntrees: u32) -> BrResult<Vec<u8>> {
    let rlemax = if b.read(1)? == 1 { b.read(4)? + 1 } else { 0 };
    let h = br_prefix(b, (ntrees + rlemax) as usize)?;
    let mut map = alloc::vec![0u8; size];
    let mut i = 0;
    while i < size {
        let s = h.decode(b)? as u32;
        if s == 0 {
            i += 1;
        } else if s <= rlemax {
            let run = ((1u32 << s) + b.read(s)?) as usize;
            if i + run > size {
                return bad("context map run past its end");
            }
            i += run; // already zero
        } else {
            map[i] = (s - rlemax) as u8;
            i += 1;
        }
    }
    if b.read(1)? == 1 {
        // Inverse move-to-front.
        let mut mtf = [0u8; 256];
        for (k, m) in mtf.iter_mut().enumerate() {
            *m = k as u8;
        }
        for v in map.iter_mut() {
            let idx = *v as usize;
            let val = mtf[idx];
            *v = val;
            mtf.copy_within(0..idx, 1);
            mtf[0] = val;
        }
    }
    Ok(map)
}

/// The output side: the caller's sink, the history ring (`cap` = 1 << WBITS, grown on demand), bytes produced and
/// the last two bytes (the literal context).
struct BrOut<'k, K: Sink> {
    sink: &'k mut K,
    hist: Vec<u8>,
    cap: usize,
    pos: u64,
    p1: u8,
    p2: u8,
}

impl<K: Sink> BrOut<'_, K> {
    fn put(&mut self, c: u8) -> BrResult<()> {
        self.sink.push(c).map_err(|_| BrErr::Sink)?;
        if self.hist.len() < self.cap {
            self.hist.push(c);
        } else {
            let k = (self.pos as usize) & (self.cap - 1);
            self.hist[k] = c;
        }
        self.pos += 1;
        self.p2 = self.p1;
        self.p1 = c;
        Ok(())
    }

    /// The byte `d` back (1 <= d <= min(pos, cap)).
    fn back(&self, d: usize) -> u8 {
        self.hist[((self.pos as usize).wrapping_sub(d)) & (self.cap - 1)]
    }
}

/// §7.1: the literal context of the two previous bytes under `mode`.
fn br_lit_ctx(mode: u8, p1: u8, p2: u8) -> usize {
    (match mode {
        0 => p1 & 0x3f,
        1 => p1 >> 2,
        2 => LUT0[p1 as usize] | LUT1[p2 as usize],
        _ => (LUT2[p1 as usize] << 3) | LUT2[p2 as usize],
    }) as usize
}

/// §8: ToUpperCase on `w[i..]`; returns how many bytes the step covers.
fn br_upper(w: &mut [u8], i: usize) -> usize {
    let c = w[i];
    if c < 0xc0 {
        if c.is_ascii_lowercase() {
            w[i] ^= 32;
        }
        1
    } else if c < 0xe0 {
        if i + 1 < w.len() {
            w[i + 1] ^= 32;
        }
        2
    } else {
        if i + 2 < w.len() {
            w[i + 2] ^= 5;
        }
        3
    }
}

/// §8: transform `word` by transform `id` into `out`; returns the length.
fn br_transform(word: &[u8], id: usize, out: &mut [u8; 48]) -> usize {
    let (pre, ty, suf) = TRANSFORMS[id];
    out[..pre.len()].copy_from_slice(pre);
    let mut n = pre.len();
    let (mut lo, mut hi) = (0usize, word.len());
    match ty {
        OL1..=OL9 => hi = hi.saturating_sub(ty as usize),
        OF1..=OF9 => lo = ((ty - OF1 + 1) as usize).min(hi),
        _ => {}
    }
    let start = n;
    out[n..n + (hi - lo)].copy_from_slice(&word[lo..hi]);
    n += hi - lo;
    if ty == UF && n > start {
        br_upper(&mut out[start..n], 0);
    } else if ty == UA {
        let mut i = 0;
        while start + i < n {
            i += br_upper(&mut out[start..n], i);
        }
    }
    out[n..n + suf.len()].copy_from_slice(suf);
    n + suf.len()
}

/// Decode one whole Brotli stream from `src` into `sink`; returns the bytes produced.
fn brotli<S: ByteSource, K: Sink>(src: &mut S, sink: &mut K) -> BrResult<u64> {
    let mut b = Bits { src, acc: 0, n: 0 };
    // §9.1 WBITS.
    let wbits = if b.read(1)? == 0 {
        16
    } else {
        match b.read(3)? {
            0 => match b.read(3)? {
                0 => 17,
                1 => return bad("large-window stream (not RFC 7932)"),
                m => 8 + m,
            },
            n => 17 + n,
        }
    };
    let window = (1u64 << wbits) - 16;
    let mut out = BrOut { sink, hist: Vec::new(), cap: 1usize << wbits, pos: 0, p1: 0, p2: 0 };
    // §4: the four last distances, oldest first in ring order (last = rb[(idx - 1) & 3]).
    let mut rb: [u64; 4] = [16, 15, 11, 4];
    let mut rb_idx: usize = 0;
    loop {
        let islast = b.read(1)? == 1;
        if islast && b.read(1)? == 1 {
            break; // ISLASTEMPTY
        }
        let mn = b.read(2)?;
        if mn == 3 {
            // Metadata: skipped.
            if b.read(1)? != 0 {
                return bad("reserved bit set");
            }
            let skipbytes = b.read(2)?;
            let mut skip = 0u32;
            for i in 0..skipbytes {
                let v = b.read(8)?;
                if i + 1 == skipbytes && skipbytes > 1 && v == 0 {
                    return bad("metadata length has a zero top byte");
                }
                skip |= v << (8 * i);
            }
            if skipbytes > 0 {
                skip += 1;
            }
            b.align()?;
            for _ in 0..skip {
                b.read(8)?;
            }
            if islast {
                break;
            }
            continue;
        }
        let nibbles = mn + 4;
        let mut mlen = 0u32;
        for i in 0..nibbles {
            let v = b.read(4)?;
            if i + 1 == nibbles && nibbles > 4 && v == 0 {
                return bad("MLEN has a zero top nibble");
            }
            mlen |= v << (4 * i);
        }
        let mut left = mlen as usize + 1;
        if !islast && b.read(1)? == 1 {
            // ISUNCOMPRESSED.
            b.align()?;
            for _ in 0..left {
                let c = b.read(8)? as u8;
                out.put(c)?;
            }
            continue;
        }
        // The compressed meta-block header.
        let mut bl = Blocks::read(&mut b)?;
        let mut bi = Blocks::read(&mut b)?;
        let mut bd = Blocks::read(&mut b)?;
        let npostfix = b.read(2)?;
        let ndirect = b.read(4)? << npostfix;
        let mut cmode = Vec::with_capacity(bl.n as usize);
        for _ in 0..bl.n {
            cmode.push(b.read(2)? as u8);
        }
        let ntreesl = br_varlen8(&mut b)? + 1;
        let cmapl = if ntreesl >= 2 { br_context_map(&mut b, 64 * bl.n as usize, ntreesl)? } else { alloc::vec![0u8; 64 * bl.n as usize] };
        let ntreesd = br_varlen8(&mut b)? + 1;
        let cmapd = if ntreesd >= 2 { br_context_map(&mut b, 4 * bd.n as usize, ntreesd)? } else { alloc::vec![0u8; 4 * bd.n as usize] };
        let mut tl = Vec::with_capacity(ntreesl as usize);
        for _ in 0..ntreesl {
            tl.push(br_prefix(&mut b, 256)?);
        }
        let mut ti = Vec::with_capacity(bi.n as usize);
        for _ in 0..bi.n {
            ti.push(br_prefix(&mut b, 704)?);
        }
        let dsize = 16 + ndirect as usize + (48usize << npostfix);
        let mut td = Vec::with_capacity(ntreesd as usize);
        for _ in 0..ntreesd {
            td.push(br_prefix(&mut b, dsize)?);
        }
        // §9.3 the commands.
        while left > 0 {
            let t = bi.step(&mut b)?;
            let cmd = ti[t].decode(&mut b)? as usize;
            let (ib, cb) = BR_CELL[cmd >> 6];
            let (ibase, iextra) = BR_INS[ib + ((cmd >> 3) & 7)];
            let (cbase, cextra) = BR_CPY[cb + (cmd & 7)];
            let ilen = (ibase + b.read(iextra)?) as usize;
            let clen = (cbase + b.read(cextra)?) as usize;
            if ilen > left {
                return bad("insert length past the meta-block");
            }
            for _ in 0..ilen {
                let t = bl.step(&mut b)?;
                let tree = cmapl[64 * t + br_lit_ctx(cmode[t], out.p1, out.p2)] as usize;
                let c = tl[tree].decode(&mut b)? as u8;
                out.put(c)?;
            }
            left -= ilen;
            if left == 0 {
                break; // the copy length of the last command is ignored (§9.3)
            }
            let dcode = if cmd < 128 {
                0
            } else {
                let t = bd.step(&mut b)?;
                let cidd = if clen > 4 { 3 } else { clen - 2 };
                td[cmapd[4 * t + cidd] as usize].decode(&mut b)? as u32
            };
            let last = |k: usize| rb[rb_idx.wrapping_sub(k) & 3] as i64;
            let dist: i64 = match dcode {
                0..=3 => last(dcode as usize + 1),
                4..=15 => {
                    const DELTA: [i64; 6] = [-1, 1, -2, 2, -3, 3];
                    let base = if dcode < 10 { last(1) } else { last(2) };
                    base + DELTA[((dcode - 4) % 6) as usize]
                }
                d if d < 16 + ndirect => (d - 15) as i64,
                d => {
                    let x = d - ndirect - 16;
                    let ndistbits = 1 + (x >> (npostfix + 1));
                    let extra = b.read(ndistbits)? as i64;
                    let hcode = (x >> npostfix) as i64;
                    let lcode = (x & ((1 << npostfix) - 1)) as i64;
                    let offset = ((2 + (hcode & 1)) << ndistbits) - 4;
                    ((offset + extra) << npostfix) + lcode + ndirect as i64 + 1
                }
            };
            if dist <= 0 {
                return bad("distance is not positive");
            }
            let dist = dist as u64;
            let max = window.min(out.pos);
            if dist > max {
                // §8 a static dictionary reference.
                if !(4..=24).contains(&clen) {
                    return bad("dictionary word length out of range");
                }
                let wid = dist - max - 1;
                let nb = BR_NDBITS[clen] as u32;
                let idx = (wid & ((1u64 << nb) - 1)) as usize;
                let tid = (wid >> nb) as usize;
                if tid >= TRANSFORMS.len() {
                    return bad("dictionary transform out of range");
                }
                let off = BR_DOFFSET[clen] as usize + idx * clen;
                let mut w = [0u8; 48];
                let n = br_transform(&BR_DICT[off..off + clen], tid, &mut w);
                if n > left {
                    return bad("dictionary word past the meta-block");
                }
                for &c in &w[..n] {
                    out.put(c)?;
                }
                left -= n;
            } else {
                if clen > left {
                    return bad("copy length past the meta-block");
                }
                for _ in 0..clen {
                    let c = out.back(dist as usize);
                    out.put(c)?;
                }
                left -= clen;
                if dcode != 0 {
                    rb[rb_idx & 3] = dist;
                    rb_idx = rb_idx.wrapping_add(1);
                }
            }
        }
        if islast {
            break;
        }
    }
    // The bits after the last meta-block up to the byte boundary are zero (§9.1); bytes past the stream are ignored.
    b.align()?;
    Ok(out.pos)
}

#[cfg(test)]
mod tests {
    //! NETDEBT2 (SR113) — the Brotli KATs. RFC 7932 carries no example streams, so the vectors are of two kinds, every
    //! one decoded by the reference decoder (libbrotlidec) to the bytes asserted here: streams assembled by hand from
    //! §9.1/§9.2 (the empty stream at two window sizes; a metadata block that must be skipped), and the reference
    //! encoder's output (libbrotlienc) over fixed inputs chosen to reach every part of the format — quality 11 over
    //! English (static dictionary words and their transforms, context modelling), quality 1 (the fast encoder's
    //! simple codes), a 1 KiB window (WBITS 10), incompressible bytes (an ISUNCOMPRESSED meta-block), and 2 KiB of
    //! text + noise + markup + a byte ramp at qualities 9 and 11 (block switching in all three categories, context
    //! maps with RLE and move-to-front, WBITS 16 and 24).
    use super::*;
    use alloc::vec::Vec;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn br(hex: &str) -> Result<Vec<u8>, DecodeError> {
        decode(&[Coding::Brotli], &unhex(hex))
    }

    /// The inputs' noise: a 31-bit LCG, one byte per step (bits 16..24).
    fn lcg(n: usize, x: &mut u64) -> Vec<u8> {
        (0..n)
            .map(|_| {
                *x = (*x * 1_103_515_245 + 12_345) & 0x7fff_ffff;
                (*x >> 16) as u8
            })
            .collect()
    }

    const PARA: &[u8] = b"Brotli is a generic-purpose lossless compression algorithm that compresses data using a combination of a modern variant of the LZ77 algorithm, Huffman coding and 2nd order context modeling. ";
    const FOX: &[u8] = b"The quick brown fox jumps over the lazy dog. The Quick Brown Fox Jumps Over The Lazy Dog! THE QUICK BROWN FOX, the lazy dog's time of the year and the people for the world.";

    fn mixed() -> Vec<u8> {
        let mut s = Vec::new();
        for _ in 0..3 {
            s.extend_from_slice(PARA);
        }
        let mut x = 12_345;
        s.extend(lcg(600, &mut x));
        s.extend_from_slice(b"<html><head><title>UnaOS</title></head><body><p>");
        s.extend_from_slice(PARA);
        s.extend_from_slice(b"</p></body></html>");
        for i in 0..400u32 {
            s.push(i as u8);
            s.push((i * 7) as u8);
        }
        s
    }

    const UKKO: &str = concat!(
        "0b0480756b6b6f206e6f6f6103",
    );
    const FOX_Q11: &str = concat!(
        "1bab0000e4705daab7b47709002a609b5d581da8555b5b1ae4bdc983bee9bb8d9d23a8d46a38aa13b61578fa58b4f5f2421d",
        "7380cb67ccfe0fcaa5e180000fbcc8d0d591504d1619b52a3d33d3372ea6d74919cd1b27012c9f68401c3792fd2c0e",
    );
    const FOX_Q1: &str = concat!(
        "8355000080aaaaaaeaff74653fd9c5c12e07737703dfc015ccc11cccafeaaea2a6e6ae66a22ea6e6666aa0836b6c3147c704",
        "03cc31c0308cb1c13843addfda117e53f7fee2253c0fb0bce033f56104ff49101dc1eb35c1709be3c0544105aae4056748b1",
        "981caf7a4dd873bb417d3ca07a9c76171477d5dc50aae7f6a71fb211b1eb096c2b5a222dd08309d805e2e0099605d1116616",
        "6ff201",
    );
    const REP_LGWIN10: &str = concat!(
        "a1b82100201d263646a63669ac880966fa03",
    );
    const LCG_Q2: &str = concat!(
        "b012106c4e74921325222e31a1cd13be12ed426966ce24fc23d7da8d2097616a06956ec28ad403136828d4571e3c5dee6e5e",
        "c04a91115f5d3b513ec253a416ad6ee5389411d0289aa34cf5c0347c59caf08495f3611b0b5068d59804f92eb72999555779",
        "99be78c01066870299e57f6cd235bcfb8f449deee23be1eccc8cbff5bfbec20bdbf86b9ce54f85b607cf46e94a4b2afbd4e5",
        "8f4fe15c11128218a32b18f772df8fd57a475bdee4743493265c919dd98be654598a9d0f1f0ed42adde0dcd85e906ead1cd9",
        "abea9ed3da8898dbe003c1427eeb72b84d2c03777b18e52f43397fb42ed9ca6a0b4dab6caf06137f6d55d9b9540152f12b8b",
        "b6e52d3c322e85f3cce488b0fb11b4df02d66c65105f716c198820ef724c6e042ff2a3ec3cf6d9dc3eb8348927e7de769bab",
        "c9fd0603",
    );
    const MIXED_Q11: &str = concat!(
        "221601c80cc00100a066b903d6fefeaf565bc225f87ba330216eb9264a99bb1e7b7b5b9aba6e0c0d7b21277b171b4b823729",
        "128b9d76bc838f5bf7ec5b72a3e6ca66baa268d00d27a7a43a174d1f325aececb7ad9812ae6666b61ad2003685562651b319",
        "f0d07e8ac2b5a123b0ff59d80131adc2afc5d5da3aadc1c6d34776f0b4f945f8c7ac37c005a969c105a216f370e67adad7af",
        "6b87d05cbce3449896486d9d89a444fc076b33a2dbe7f964fd954ea19882f4116906d24bd36fbeb2ad59a10db4b3ac3f380b",
        "97c901395f62d3a53198bc7af17c0b2bb84e263b31425a859d6e7647b5b2e2b2e88adb768110cb7b9e6b86fa58d8b5e8fed0",
        "ff22795afac108ffd609ef6ae1602acdc6e645f079f758f33fb2fd404ca4f44b4e9d461a559fd37a5c41c358c7ec01d1f6cb",
        "347d2c4a711ca1aab1aae418711daf65e200c207e5a6c9c4fab946dd9bb7e83af0e959c34bc7ce82ceed4b375e4efaa9df2f",
        "8a49271635bade053775f8d788b49a7f9fbae3c7b93c81b238dd4b8ee5c8f1ed5dd7865c393f6216e1978a19294c58ebe8d8",
        "31d45f8ca3e8d384ab52c05688b262a33c73d85775efda11b115eeae84405f1b78f3863074d8a49d4472d226f11b3ada6a6c",
        "02c923087061cfb24f80d9a5d3737a859468d3584cbf1c8e69b45bde45123162824129afe7ff82d95b439e430e9b562f103f",
        "d1ee5b146d615b8df85b02107c0a6f840fd67731eda2104353da8f2842deec17ba6f476cdd03f79ff4ec3aaed090aabca0ad",
        "9055c83dcbb03eaacc9a279431a38a594d4064cf308fecae7d568c45ae7dc529044ca96908b1798d7aa5f24a7c7daea60f63",
        "ea0a1425e01d364789f0724d98301e073fc01b7200ce5a8c46e327791bdea3bed7d55a3c7343f69c1242eb44a802dd0c6618",
        "879a0f7d309bbeb9af8994f96ac7c7e81a66d96f33906a0a609ae52cb4db4f1141397c2ebb197ce114061bc836ca3de65a36",
        "bc1f56b65e904fc3bf93ed4d8d453f6544b6a8da19ca2b9f63f20497cac11df85800fe3d700cdf570b0000080e04078c0a82",
        "034a0c4605ce08c101c90f25062d0ba302ab0d67046f89e080e88ee4871c8a12839a8c96855e885181598fd586dd8b33823b",
        "8db784bf497040784e7447fc4af2430a4c06458e488141894f45464d4bc342cb4d27442fc9a0c0a8cea4c76cca62c3eacce6",
        "c51ec811c119cf95c69dcb53c25bcdd7c4df293020382e3427bc2ab2237a2c7625fe28f121f92f05260d2b83228b2d47244f",
        "a9c0a0c8aec4a72caa22a3aaaca6a56ea861a169afe5a6edab13a21bad97a49f695060586e5467dc6ad2633a6c3665be68b1",
        "61b96f75667d6bf362fb6d07640fe980e088ee84e74cea42e3caecc6e52ee821e129efa5e6adeb63e26bede7e4ef19101018",
        "1e14179c1a92135a1c5615de18d111d91f35163d1bb312bb1d77147f99f090f89ef4970c9a02938a9c86954e984191499fc5",
        "96cd9b23922b9da794af596050685e6457ec5ae2531a5c16559e589151995f55565d5bd352db5d37543fd9b0d0b8deb4d77c",
        "da72d3fadcf6d50ed801d109df85d68ddb43d24bddc7d4cf392030283e2437ac3aa2336a3c6635ee38e131e93f15361d3b93",
        "329b3d57345fb9d0b0d8bed4b73cba32b3babcb6b57eb871b179bff5b6fdbb03b20bbd87b48f794070487e4477cc7ac2732a",
        "7c2675ae78a171a97f65766d7be372eb7d17741ff990f098fe94f75cfa52f3dafcd6f53ef831f139ffb5f6bdfb73f27bfdf7",
        "f4ff8900",
    );
    const MIXED_Q9: &str = concat!(
        "1fb108006480ce05ea22b18445eac1684eddd407d5b5a931694cacf330c130f129e2016798a8e5dc7c071d5dc19ab9fcc26e",
        "24dd5d3c9dec017b0fc0852eb7b76441ac00ea50b90560e9e2bc7b63ef02064e070021c87339b5f600dec418a07915283c0b",
        "7b9626808b0d000228b100b116d0c295b2211f5fda7d6640decbc6c61904062c5dac728dc1560027d80a281c8e01ec69edeb",
        "395f364ef6605b5600c167611dc6ba9ebc89d470f5a09198cc0cdbd5cfe903e38bb0c59c14a6f117291493acaf87e0ed9659",
        "f35f0483994b018c31f9acdfe3710c985f707f2c58b7845f783e9432e9471103a57f4299669bfd3c756e9c670903dbb76c1f",
        "c62817c21ccdf29a309b8f9cdefb3d46979e65d0ee5b602da08efdf4c1757010b46b4d5ef344902dbe9bfc89f43cbd61d7f6",
        "6cf64a7d3bfb4e10f3661cd3fa8b9fb3aa80580423f0c9773164d7f919338d2138236b0c6b406fd2f0661e8f6089b01dc628",
        "38cffaa63cd3ad9fb1d1a18ebfdf6ba889931f8a1c5688398fe8fd0c7773caae753bb07d912c87381fb9a833c7722efdbd65",
        "ab3c31874ac6e26c03dedaf54b3fce61ea69f4528ce4641fc9ee60332389bf30a863c90871fe68974b5a4c31b1bf2c75f1df",
        "bf7590d94c327da7015abd3ab17abc032fc3a1eb968e4febbab4b3545b1d02fe62f7630002c875856504cbdd883ab87777f5",
        "fb0f986220d4932fdf25156583d5d4fd0151fe8ffe4cce495fe277f0dc0696b405270a4e4d8bd82bcae622f62476f4985ea9",
        "dcbe51e78ccc2fde85d8cf590889db98efe811669ed6adf4b2789a5bb1e5d82b5d92577f317f6fc2878cf8b5eed5ea2bd61d",
        "b3d4aaa8171de63aeed6e54f5128db30262ea462f2e5dac6b535dd692461373d90944166367e8b747a887f60f44f7bb86b60",
        "8be66c0d930144fa6c7162fdbd52872d87e0b7c97a5da1f36712f0c4a81ffb3699ae4c0c670ebdb1fd4625f1fe63d4c3ae44",
        "b00f214d455ddb8c9d5f35c629bffe252768ede3defa63aef823947342a00cbc95298db373577dde732f2005d75d6051b427",
        "3807baead3729c56cb26bf54e9f06d71a26e9454ad363a60909a9661e15258e21e5c54d855f426cd3e8e4bf06836f4bfaf16",
        "0202121e0a0d1a178604960a8e119e03811f510c49165905451bd508cd02dd1ec315d3072b183b0a271137032f1fbf8ca096",
        "b085e80f7107493fe918d92cf90ab04d71447949f540fd4e034d8b4487494fc040ce48cbc4c2cccd22c42ac9a6c0aecea1c7",
        "69ca65cdedc4e3c1ebcf17c61f2b9022982d54245c29d220fa43ecbf78b7c490e4a4d482f4bacc9eeca9dc8dfcb3c297229c",
        "12aa328e0ab12aa51a833abb069fa6a8968cb6b28e96aea11e48dfcec0c5d0db28c838d224c134dd2ccfbc145463d16cf9db",
        "aaddbacf66d476c66ed97ecbe1d0f1c2e9def90d0ce582e88ae186ef4ee641e3c9ecc5e52de823e12befa7e6af1b60126815",
        "e418ec1ee2171a1a16139e1c9115591855115d1ff33df65f5c57fc60c244e27cd25af26eca49ea75da53fa67066c264a1676",
        "36510e452e7d1e5b3e6f8148a1749152b1668941a979996d39b8c2ab32b02aa23abe26ad36b7aea4bebaa1a9f157535b736f",
        "cbc8b7e9ef4b3f367f1efc3aff7df7e7f52fe43f84ffe8ad786da4edd41d4c9d9c5d02dde23d72bdaa7d3afdc60396830e43",
        "6ec3be2321a3d16349e399130593e55375d3df66fece76ce0dcc8f2fcc2dae2eed2c1faf5cad3eae7dacc36c206f626d116e",
        "033b74bbac7b3cfbc20752878a471ac7fa2766a73667cee79e170197e15771d7a93739b7c57755f78d0f3f1f5b9f7a9e875f",
        "a65e17df36def73fce3e6fbf5e7a00",
    );
    #[test]
    fn hand_assembled_streams() {
        assert_eq!(br("06").unwrap(), b""); // WBITS 16, ISLAST, ISLASTEMPTY
        assert_eq!(br("3b").unwrap(), b""); // WBITS 22, ISLAST, ISLASTEMPTY
        assert_eq!(br("2c0161626303").unwrap(), b""); // a 3-byte metadata block ("abc") skipped, then the empty last
    }

    #[test]
    fn reference_encoder_streams() {
        assert_eq!(br(UKKO).unwrap(), b"ukko nooa");
        assert_eq!(br(FOX_Q11).unwrap(), FOX);
        assert_eq!(br(FOX_Q1).unwrap(), FOX);
        assert_eq!(br(REP_LGWIN10).unwrap(), b"abcabcabd".repeat(120));
        assert_eq!(br(LCG_Q2).unwrap(), lcg(300, &mut 7));
        assert_eq!(br(MIXED_Q11).unwrap(), mixed());
        assert_eq!(br(MIXED_Q9).unwrap(), mixed());
    }

    #[test]
    fn refusals_are_named() {
        let mut cut = unhex(FOX_Q11);
        cut.truncate(cut.len() - 10);
        assert_eq!(decode(&[Coding::Brotli], &cut), Err(DecodeError::Brotli("truncated stream")));
        // The reference decoder refuses both too: a reserved bit / a broken code, and a truncated header.
        assert!(matches!(br("a10200"), Err(DecodeError::Brotli(_))));
        assert!(matches!(br("0b00"), Err(DecodeError::Brotli(_))));
    }

    #[test]
    fn br_is_advertised_and_named() {
        assert!(ACCEPT_ENCODING.split(',').any(|c| c.trim() == "br"));
        let mut h = Headers::new();
        h.append("Content-Encoding", "br").unwrap();
        assert_eq!(content_codings(&h), alloc::vec![Coding::Brotli]);
    }

    #[test]
    fn dictionary_is_appendix_a() {
        assert_eq!(BR_DICT.len(), 122_784);
        assert_eq!(&BR_DICT[..4], b"time"); // the first 4-byte word
        assert_eq!(&BR_DICT[BR_DOFFSET[5] as usize..][..10], b"firstvideo"); // the first two 5-byte words
        let mut w = [0u8; 48];
        let n = br_transform(b"time", 9, &mut w); // uppercase-first, no affixes
        assert_eq!(&w[..n], b"Time");
        let n = br_transform(b"time", 73, &mut w); // " the " + word + " of the "
        assert_eq!(&w[..n], b" the time of the ");
    }
}
