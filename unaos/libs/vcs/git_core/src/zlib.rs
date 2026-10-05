// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Inflate through pixel_core's RFC 1950/1951 decoder (the ONE inflater in the tree), with what a
//! packfile needs from it: the count of compressed bytes the stream consumed, since pack entries
//! are concatenated zlib streams with no stored compressed length.

use alloc::vec::Vec;

use pixel_core::inflate::{self, ByteSource, InflateError, Sink};

use crate::{Error, Result};

struct Src<'a> {
    d: &'a [u8],
    p: usize,
}

impl ByteSource for Src<'_> {
    #[inline]
    fn next(&mut self) -> Option<u8> {
        let b = self.d.get(self.p).copied();
        self.p += 1;
        b
    }
}

struct VecSink {
    out: Vec<u8>,
    max: usize,
}

impl Sink for VecSink {
    #[inline]
    fn push(&mut self, b: u8) -> core::result::Result<(), ()> {
        if self.out.len() >= self.max {
            return Err(());
        }
        self.out.push(b);
        Ok(())
    }
}

fn reason(e: InflateError) -> &'static str {
    inflate::inflate_reason(e)
}

/// Inflate the zlib stream at the start of `data`; at most `max` output bytes (a larger stream is
/// refused). Returns (output, compressed bytes consumed).
pub fn inflate(data: &[u8], size_hint: usize, max: usize) -> Result<(Vec<u8>, usize)> {
    let mut src = Src { d: data, p: 0 };
    let mut sink = VecSink { out: Vec::with_capacity(size_hint.min(max).min(1 << 26)), max };
    let rep = inflate::zlib_inflate(&mut src, &mut sink).map_err(|e| Error::Inflate(reason(e)))?;
    Ok((sink.out, rep.compressed as usize))
}

/// Inflate a zlib stream that must produce exactly `size` bytes.
pub fn inflate_exact(data: &[u8], size: usize) -> Result<(Vec<u8>, usize)> {
    let (out, used) = inflate(data, size, size)?;
    if out.len() != size {
        return Err(Error::Corrupt("inflated size differs from the declared size"));
    }
    Ok((out, used))
}
