// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Applying a parsed diff to a file the caller can only READ AT AN OFFSET (a ring-3 program cannot hold a
//! 2 MiB kernel source file; the kernel fixture hands a slice). Two passes per file, nothing written until
//! every hunk of every file resolved:
//!
//! 1. [`resolve`] — for each hunk (ascending), find the byte offset of line `target - FUZZ`, read a window
//!    from there into `scratch`, and try the hunk's old side at offsets 0, -1, +1, … ±[`FUZZ`] lines from the
//!    stated line. Every old line must match its window line exactly (a trailing `\r` ignored on both). The
//!    first match is the hunk's [`Span`] (bytes of the original it replaces). No match → `Mismatch`.
//! 2. [`emit`] — stream the original to `sink`, replacing each span by the hunk's new side.

use crate::diff::{Hunk, Refuse};

/// How far (in lines) a hunk may have drifted from its stated line.
pub const FUZZ: u32 = 3;

/// A source the caller reads at byte offsets: `Ok(0)` = end of file.
pub trait ReadAt {
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<usize, i64>;
}

impl ReadAt for &[u8] {
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<usize, i64> {
        let o = (off as usize).min(self.len());
        let n = (self.len() - o).min(buf.len());
        buf[..n].copy_from_slice(&self[o..o + n]);
        Ok(n)
    }
}

/// The original bytes `[start, end)` one hunk replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

/// A line cursor over a source: line `line` (1-based) begins at byte `off`.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub line: u32,
    pub off: u64,
}

impl Cursor {
    pub const START: Cursor = Cursor { line: 1, off: 0 };
}

/// Move `c` forward to line `want` (no-op if already there or past). `Ok(false)` = the file ends first.
pub fn seek_line(src: &mut dyn ReadAt, c: &mut Cursor, want: u32, scratch: &mut [u8]) -> Result<bool, Refuse> {
    while c.line < want {
        let n = src.read_at(c.off, scratch).map_err(Refuse::Io)?;
        if n == 0 {
            return Ok(false);
        }
        let mut adv = n as u64;
        for (i, &b) in scratch[..n].iter().enumerate() {
            if b == b'\n' {
                c.line += 1;
                if c.line == want {
                    adv = i as u64 + 1;
                    break;
                }
            }
        }
        c.off += adv;
    }
    Ok(true)
}

/// Fill `buf` from `off` as far as the file goes. Returns `(bytes, hit_eof)`.
fn fill(src: &mut dyn ReadAt, off: u64, buf: &mut [u8]) -> Result<(usize, bool), Refuse> {
    let mut n = 0;
    while n < buf.len() {
        let k = src.read_at(off + n as u64, &mut buf[n..]).map_err(Refuse::Io)?;
        if k == 0 {
            return Ok((n, true));
        }
        n += k;
    }
    Ok((n, false))
}

fn strip_cr(b: &[u8]) -> &[u8] {
    b.strip_suffix(b"\r").unwrap_or(b)
}

/// Resolve every hunk of one file into `spans` (same length as `hunks`). `scratch` bounds a window
/// (a hunk's old side plus 2·FUZZ lines must fit); `file` is the file's index for the refusal.
pub fn resolve(src: &mut dyn ReadAt, hunks: &[Hunk<'_>], spans: &mut [Span], scratch: &mut [u8], file: usize) -> Result<(), Refuse> {
    let mut cur = Cursor::START;
    let mut prev_end = 0u64;
    let mut prev_target = 0u32;
    for (hi, h) in hunks.iter().enumerate() {
        let target = h.target();
        if target < prev_target {
            return Err(Refuse::Overlap);
        }
        prev_target = target;
        let first = target.saturating_sub(FUZZ).max(1);
        if !seek_line(src, &mut cur, first, scratch)? {
            return Err(Refuse::Mismatch(file as u8, hi as u8));
        }
        let base = cur.off;
        let (n, eof) = fill(src, base, scratch)?;
        let win = &scratch[..n];
        // Whole lines inside the window: `[starts[k], ends[k])` without the `\n`. A last line with no
        // `\n` counts only when the file ends there.
        const WL: usize = 512;
        let mut starts = [0usize; WL];
        let mut ends = [0usize; WL];
        let mut nlines = 0usize;
        let mut s = 0usize;
        let mut capped = false;
        for (i, &b) in win.iter().enumerate() {
            if b == b'\n' {
                if nlines == WL {
                    capped = true;
                    break;
                }
                starts[nlines] = s;
                ends[nlines] = i;
                nlines += 1;
                s = i + 1;
            }
        }
        if eof && !capped && s < win.len() && nlines < WL {
            starts[nlines] = s;
            ends[nlines] = win.len();
            nlines += 1;
            s = win.len();
        }
        let all_seen = eof && !capped; // every remaining line of the file is in `starts`
        // Byte offset (in the window) where line k begins; k == nlines is the end of the last whole line.
        let byte_of = |k: usize| -> u64 { if k < nlines { starts[k] as u64 } else { s as u64 } };
        let old_len = h.old_len as usize;
        let mut found: Option<usize> = None;
        for d in [0i64, -1, 1, -2, 2, -3, 3] {
            let c = target as i64 + d;
            if c < first as i64 {
                continue;
            }
            let idx = (c - first as i64) as usize;
            if idx + old_len > nlines {
                if !all_seen {
                    return Err(Refuse::Window); // the window could not hold this candidate
                }
                continue; // past the end of the file
            }
            if h.old_lines().zip(idx..).all(|(t, k)| strip_cr(&win[starts[k]..ends[k]]) == strip_cr(t.as_bytes())) {
                found = Some(idx);
                break;
            }
        }
        let Some(idx) = found else { return Err(Refuse::Mismatch(file as u8, hi as u8)) };
        let sp = Span { start: base + byte_of(idx), end: base + byte_of(idx + old_len) };
        if sp.start < prev_end {
            return Err(Refuse::Overlap);
        }
        prev_end = sp.end;
        spans[hi] = sp;
    }
    Ok(())
}

/// Copy `[from, to)` of `src` to `sink` (`to = None`: to the end). Returns the bytes copied.
fn copy(src: &mut dyn ReadAt, from: u64, to: Option<u64>, scratch: &mut [u8], sink: &mut dyn FnMut(&[u8]) -> Result<(), i64>) -> Result<u64, Refuse> {
    let mut off = from;
    loop {
        let want = match to {
            Some(t) if off >= t => break,
            Some(t) => ((t - off) as usize).min(scratch.len()),
            None => scratch.len(),
        };
        let n = src.read_at(off, &mut scratch[..want]).map_err(Refuse::Io)?;
        if n == 0 {
            if to.is_some() {
                return Err(Refuse::Io(-5));
            }
            break;
        }
        sink(&scratch[..n]).map_err(Refuse::Io)?;
        off += n as u64;
    }
    Ok(off - from)
}

/// Stream the patched file to `sink`. Returns the bytes emitted.
pub fn emit(src: &mut dyn ReadAt, hunks: &[Hunk<'_>], spans: &[Span], scratch: &mut [u8], sink: &mut dyn FnMut(&[u8]) -> Result<(), i64>) -> Result<u64, Refuse> {
    let mut pos = 0u64;
    let mut total = 0u64;
    for (h, sp) in hunks.iter().zip(spans) {
        total += copy(src, pos, Some(sp.start), scratch, sink)?;
        for l in h.new_lines() {
            sink(l.as_bytes()).map_err(Refuse::Io)?;
            sink(b"\n").map_err(Refuse::Io)?;
            total += l.len() as u64 + 1;
        }
        pos = sp.end;
    }
    total += copy(src, pos, None, scratch, sink)?;
    Ok(total)
}

/// Read up to `n` whole lines starting at line `first` into `buf` (cut at its end). Returns
/// `(bytes, lines)`; `lines == 0` when the file is shorter than `first`.
pub fn read_lines(src: &mut dyn ReadAt, first: u32, n: u32, buf: &mut [u8], scratch: &mut [u8]) -> Result<(usize, u32), Refuse> {
    let mut c = Cursor::START;
    if !seek_line(src, &mut c, first.max(1), scratch)? {
        return Ok((0, 0));
    }
    let (got, _) = fill(src, c.off, buf)?;
    let mut lines = 0u32;
    let mut end = 0usize;
    for (i, &b) in buf[..got].iter().enumerate() {
        if b == b'\n' {
            lines += 1;
            end = i + 1;
            if lines == n {
                break;
            }
        }
    }
    if lines < n && got > end && got < buf.len() {
        // The last line of the file without a newline.
        lines += 1;
        end = got;
    }
    Ok((end, lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff;
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    fn file(n: u32) -> String {
        let mut s = String::new();
        for i in 1..=n {
            s.push_str(&std::format!("line {}\n", i));
        }
        s
    }

    fn run(orig: &str, d: &str, scratch_len: usize) -> Result<String, Refuse> {
        let p = diff::parse(d)?;
        let f = p.files[0];
        let hs = p.hunks_of(&f);
        let mut spans = [Span::default(); 8];
        let mut scratch = std::vec![0u8; scratch_len];
        let mut src: &[u8] = orig.as_bytes();
        resolve(&mut src, hs, &mut spans[..hs.len()], &mut scratch, 0)?;
        let mut out = Vec::new();
        let mut src: &[u8] = orig.as_bytes();
        emit(&mut src, hs, &spans[..hs.len()], &mut scratch, &mut |b| {
            out.extend_from_slice(b);
            Ok(())
        })?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn exact_and_fuzzed() {
        let orig = file(30);
        let d = "--- a/f\n+++ b/f\n@@ -9,3 +9,3 @@\n line 9\n-line 10\n+LINE TEN\n line 11\n@@ -20,2 +20,3 @@\n line 20\n+inserted\n line 21\n";
        let got = run(&orig, d, 4096).unwrap();
        assert!(got.contains("line 9\nLINE TEN\nline 11\n"));
        assert!(got.contains("line 20\ninserted\nline 21\n"));
        assert_eq!(got.lines().count(), 31);
        // stated two lines off: still applies (fuzz)
        let d2 = "--- a/f\n+++ b/f\n@@ -11,3 +11,3 @@\n line 9\n-line 10\n+X\n line 11\n";
        assert!(run(&orig, d2, 4096).unwrap().contains("line 9\nX\nline 11\n"));
        // four lines off: refused
        let d3 = "--- a/f\n+++ b/f\n@@ -14,3 +14,3 @@\n line 9\n-line 10\n+X\n line 11\n";
        assert_eq!(run(&orig, d3, 4096).err(), Some(Refuse::Mismatch(0, 0)));
        // context mismatch: refused
        let d4 = "--- a/f\n+++ b/f\n@@ -9,3 +9,3 @@\n line 9\n-line 99\n+X\n line 11\n";
        assert_eq!(run(&orig, d4, 4096).err(), Some(Refuse::Mismatch(0, 0)));
    }

    #[test]
    fn small_scratch_streams() {
        // scratch smaller than the file: seek and copy still stream; a window that cannot fit refuses.
        let orig = file(2000);
        let d = "--- a/f\n+++ b/f\n@@ -1500,3 +1500,3 @@\n line 1500\n-line 1501\n+Z\n line 1502\n";
        let got = run(&orig, d, 256).unwrap();
        assert!(got.contains("line 1500\nZ\nline 1502\n"));
        assert_eq!(got.len(), orig.len() - "line 1501".len() + 1);
        assert_eq!(run(&orig, d, 16).err(), Some(Refuse::Window));
    }

    #[test]
    fn eof_and_lines() {
        let orig = "a\nb\nc"; // no trailing newline
        let got = run(orig, "--- a/f\n+++ b/f\n@@ -2,2 +2,2 @@\n b\n-c\n+C\n", 64).unwrap();
        assert_eq!(got, "a\nb\nC\n");
        let mut src: &[u8] = file(10).as_bytes().to_vec().leak();
        let mut buf = [0u8; 64];
        let mut sc = [0u8; 8];
        let (n, l) = read_lines(&mut src, 4, 3, &mut buf, &mut sc).unwrap();
        assert_eq!((&buf[..n], l), (&b"line 4\nline 5\nline 6\n"[..], 3));
        let (_, l) = read_lines(&mut src, 40, 3, &mut buf, &mut sc).unwrap();
        assert_eq!(l, 0);
    }
}
