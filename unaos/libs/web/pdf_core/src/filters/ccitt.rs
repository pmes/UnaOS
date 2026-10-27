// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CCITTFaxDecode (ISO 32000-1 §7.4.6): ITU-T T.4 one-dimensional (Modified Huffman, /K 0) and two-dimensional
//! (Modified READ, /K > 0) coding, and ITU-T T.6 (MMR, /K < 0) — PDFFAX SR331.
//!
//! CHARTER: Aether — shared-core. pdf_core's ONE fax decoder: the PDF filter (through `filters::bilevel`) and JBIG2's
//! MMR generic regions (`jbig2.rs`, T.88 6.2.6) both decode through [`decode`].
//!
//! A line is held as its CHANGING ELEMENTS (T.4 4.2.1.3.1): the columns where the colour changes, white first. The
//! 2-D modes code the coding line against the reference line (the line above; all white before the first): b1 is
//! the first changing element on the reference line right of a0 and of the colour opposite a0's, b2 the next one.
//! EOLs and fill bits before a line are skipped (so /EndOfLine true and false both read); two EOLs in a row are the
//! end of the data (T.4 RTC, T.6 EOFB). The uncompressed-mode extension is refused by name.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, MAX_DECODED};

/// The /DecodeParms of a CCITTFaxDecode filter (Table 11).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// /K: < 0 T.6 (MMR); 0 T.4 1-D; > 0 T.4 mixed 1-D / 2-D (a tag bit before each line).
    pub k: i64,
    /// /Columns (default 1728).
    pub columns: usize,
    /// /Rows (0 = until the data or an RTC / EOFB ends).
    pub rows: usize,
    /// /BlackIs1: false (the default) writes black as 0.
    pub black_is_1: bool,
    /// /EncodedByteAlign: each line's code starts on a byte boundary.
    pub byte_align: bool,
    /// /EndOfLine and /EndOfBlock: read for completeness; EOLs are accepted whether or not they are promised, and a
    /// double EOL ends the data either way.
    pub end_of_line: bool,
    pub end_of_block: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { k: 0, columns: 1728, rows: 0, black_is_1: false, byte_align: false, end_of_line: false, end_of_block: true }
    }
}

// The code tables, (code, bit length), MSB-first. Provenance: ITU-T T.4 (02/2004) Table 2 (terminating codes),
// Table 3 (make-up codes), Table 4 / T.6 Table 3 (the extended make-up codes shared by both colours), and T.4
// Table 5 / T.6 Table 1 (the 2-D mode codes).

/// White terminating codes, index = run length 0-63.
pub const WHITE_TERM: [(u16, u8); 64] = [
    (0b00110101, 8), (0b000111, 6), (0b0111, 4), (0b1000, 4), (0b1011, 4), (0b1100, 4), (0b1110, 4), (0b1111, 4),
    (0b10011, 5), (0b10100, 5), (0b00111, 5), (0b01000, 5), (0b001000, 6), (0b000011, 6), (0b110100, 6), (0b110101, 6),
    (0b101010, 6), (0b101011, 6), (0b0100111, 7), (0b0001100, 7), (0b0001000, 7), (0b0010111, 7), (0b0000011, 7), (0b0000100, 7),
    (0b0101000, 7), (0b0101011, 7), (0b0010011, 7), (0b0100100, 7), (0b0011000, 7), (0b00000010, 8), (0b00000011, 8), (0b00011010, 8),
    (0b00011011, 8), (0b00010010, 8), (0b00010011, 8), (0b00010100, 8), (0b00010101, 8), (0b00010110, 8), (0b00010111, 8), (0b00101000, 8),
    (0b00101001, 8), (0b00101010, 8), (0b00101011, 8), (0b00101100, 8), (0b00101101, 8), (0b00000100, 8), (0b00000101, 8), (0b00001010, 8),
    (0b00001011, 8), (0b01010010, 8), (0b01010011, 8), (0b01010100, 8), (0b01010101, 8), (0b00100100, 8), (0b00100101, 8), (0b01011000, 8),
    (0b01011001, 8), (0b01011010, 8), (0b01011011, 8), (0b01001010, 8), (0b01001011, 8), (0b00110010, 8), (0b00110011, 8), (0b00110100, 8),
];

/// Black terminating codes, index = run length 0-63.
pub const BLACK_TERM: [(u16, u8); 64] = [
    (0b0000110111, 10), (0b010, 3), (0b11, 2), (0b10, 2), (0b011, 3), (0b0011, 4), (0b0010, 4), (0b00011, 5),
    (0b000101, 6), (0b000100, 6), (0b0000100, 7), (0b0000101, 7), (0b0000111, 7), (0b00000100, 8), (0b00000111, 8), (0b000011000, 9),
    (0b0000010111, 10), (0b0000011000, 10), (0b0000001000, 10), (0b00001100111, 11), (0b00001101000, 11), (0b00001101100, 11), (0b00000110111, 11), (0b00000101000, 11),
    (0b00000010111, 11), (0b00000011000, 11), (0b000011001010, 12), (0b000011001011, 12), (0b000011001100, 12), (0b000011001101, 12), (0b000001101000, 12), (0b000001101001, 12),
    (0b000001101010, 12), (0b000001101011, 12), (0b000011010010, 12), (0b000011010011, 12), (0b000011010100, 12), (0b000011010101, 12), (0b000011010110, 12), (0b000011010111, 12),
    (0b000001101100, 12), (0b000001101101, 12), (0b000011011010, 12), (0b000011011011, 12), (0b000001010100, 12), (0b000001010101, 12), (0b000001010110, 12), (0b000001010111, 12),
    (0b000001100100, 12), (0b000001100101, 12), (0b000001010010, 12), (0b000001010011, 12), (0b000000100100, 12), (0b000000110111, 12), (0b000000111000, 12), (0b000000100111, 12),
    (0b000000101000, 12), (0b000001011000, 12), (0b000001011001, 12), (0b000000101011, 12), (0b000000101100, 12), (0b000001011010, 12), (0b000001100110, 12), (0b000001100111, 12),
];

/// White make-up codes, index i = run length 64 * (i + 1), 64-1728.
pub const WHITE_MAKEUP: [(u16, u8); 27] = [
    (0b11011, 5), (0b10010, 5), (0b010111, 6), (0b0110111, 7), (0b00110110, 8), (0b00110111, 8), (0b01100100, 8), (0b01100101, 8),
    (0b01101000, 8), (0b01100111, 8), (0b011001100, 9), (0b011001101, 9), (0b011010010, 9), (0b011010011, 9), (0b011010100, 9), (0b011010101, 9),
    (0b011010110, 9), (0b011010111, 9), (0b011011000, 9), (0b011011001, 9), (0b011011010, 9), (0b011011011, 9), (0b010011000, 9), (0b010011001, 9),
    (0b010011010, 9), (0b011000, 6), (0b010011011, 9),
];

/// Black make-up codes, index i = run length 64 * (i + 1), 64-1728.
pub const BLACK_MAKEUP: [(u16, u8); 27] = [
    (0b0000001111, 10), (0b000011001000, 12), (0b000011001001, 12), (0b000001011011, 12), (0b000000110011, 12), (0b000000110100, 12), (0b000000110101, 12), (0b0000001101100, 13),
    (0b0000001101101, 13), (0b0000001001010, 13), (0b0000001001011, 13), (0b0000001001100, 13), (0b0000001001101, 13), (0b0000001110010, 13), (0b0000001110011, 13), (0b0000001110100, 13),
    (0b0000001110101, 13), (0b0000001110110, 13), (0b0000001110111, 13), (0b0000001010010, 13), (0b0000001010011, 13), (0b0000001010100, 13), (0b0000001010101, 13), (0b0000001011010, 13),
    (0b0000001011011, 13), (0b0000001100100, 13), (0b0000001100101, 13),
];

/// Extended make-up codes (both colours), index i = run length 1792 + 64 * i, 1792-2560.
pub const EXT_MAKEUP: [(u16, u8); 13] = [
    (0b00000001000, 11), (0b00000001100, 11), (0b00000001101, 11), (0b000000010010, 12), (0b000000010011, 12), (0b000000010100, 12),
    (0b000000010101, 12), (0b000000010110, 12), (0b000000010111, 12), (0b000000011100, 12), (0b000000011101, 12), (0b000000011110, 12),
    (0b000000011111, 12),
];

/// EOL: eleven 0 bits and a 1.
pub const EOL: (u16, u8) = (1, 12);
/// Pass mode.
pub const PASS: (u16, u8) = (0b0001, 4);
/// Horizontal mode (followed by two run codes, a0's colour first).
pub const HORIZONTAL: (u16, u8) = (0b001, 3);
/// Vertical modes, index = a1 - b1 + 3: VL3 VL2 VL1 V0 VR1 VR2 VR3.
pub const VERTICAL: [(u16, u8); 7] =
    [(0b0000010, 7), (0b000010, 6), (0b010, 3), (0b1, 1), (0b011, 3), (0b000011, 6), (0b0000011, 7)];
/// The extension prefix (0000001xxx); xxx = 111 is uncompressed mode.
pub const EXTENSION: (u16, u8) = (0b0000001, 7);

const PEEK: u32 = 13;

/// A run table: the next 13 bits -> (run, code length); length 0 = no code.
fn run_table(term: &[(u16, u8); 64], makeup: &[(u16, u8); 27]) -> Vec<(u16, u8)> {
    let mut t = vec![(0u16, 0u8); 1 << PEEK];
    let mut put = |(code, len): (u16, u8), run: u16| {
        let shift = PEEK - len as u32;
        let base = (code as usize) << shift;
        for e in &mut t[base..base + (1 << shift)] {
            *e = (run, len);
        }
    };
    for (r, &c) in term.iter().enumerate() {
        put(c, r as u16);
    }
    for (i, &c) in makeup.iter().enumerate() {
        put(c, 64 * (i as u16 + 1));
    }
    for (i, &c) in EXT_MAKEUP.iter().enumerate() {
        put(c, 1792 + 64 * i as u16);
    }
    t
}

#[derive(Clone, Copy)]
enum Mode {
    None,
    Pass,
    Horizontal,
    Vertical(isize),
    Extension,
}

/// The mode table over the next 7 bits.
fn mode_table() -> [(Mode, u8); 128] {
    let mut t = [(Mode::None, 0u8); 128];
    let mut put = |(code, len): (u16, u8), m: Mode| {
        let shift = 7 - len as u32;
        let base = (code as usize) << shift;
        for e in &mut t[base..base + (1 << shift)] {
            *e = (m, len);
        }
    };
    put(PASS, Mode::Pass);
    put(HORIZONTAL, Mode::Horizontal);
    for (i, &c) in VERTICAL.iter().enumerate() {
        put(c, Mode::Vertical(i as isize - 3));
    }
    put(EXTENSION, Mode::Extension);
    t
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }
    /// The next `n` (<= 24) bits, zeros past the end.
    fn peek(&self, n: u32) -> u32 {
        let mut v = 0u32;
        for k in 0..n as usize {
            let p = self.pos + k;
            let b = self.data.get(p / 8).map(|&b| (b >> (7 - p % 8)) & 1).unwrap_or(0);
            v = v << 1 | b as u32;
        }
        v
    }
    fn skip(&mut self, n: usize) {
        self.pos += n;
    }
    fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }
}

struct Tables {
    white: Vec<(u16, u8)>,
    black: Vec<(u16, u8)>,
    modes: [(Mode, u8); 128],
}

const BAD: Error = Error::Syntax("CCITTFaxDecode code", 0);

/// One run of `black` or white: make-up codes then a terminating code.
fn run(b: &mut Bits, t: &Tables, black: bool) -> Result<usize, Error> {
    let table = if black { &t.black } else { &t.white };
    let mut total = 0usize;
    loop {
        if b.left() == 0 {
            return Err(BAD);
        }
        let (r, len) = table[b.peek(PEEK) as usize];
        if len == 0 || len as usize > b.left() {
            return Err(BAD);
        }
        b.skip(len as usize);
        total += r as usize;
        if r < 64 {
            return Ok(total);
        }
    }
}

/// Changing elements: equal neighbours cancel (a zero-length run) and columns past the line drop out.
fn push(cur: &mut Vec<usize>, p: usize, cols: usize) {
    if p >= cols {
        // Keep the parity honest: a change at the edge is no change.
        return;
    }
    if cur.last() == Some(&p) {
        cur.pop();
    } else {
        cur.push(p);
    }
}

fn row_1d(b: &mut Bits, t: &Tables, cols: usize) -> Result<Vec<usize>, Error> {
    let mut cur = Vec::new();
    let (mut pos, mut black) = (0usize, false);
    while pos < cols {
        pos = (pos + run(b, t, black)?).min(cols);
        push(&mut cur, pos, cols);
        black = !black;
    }
    Ok(cur)
}

fn row_2d(b: &mut Bits, t: &Tables, refl: &[usize], cols: usize) -> Result<Vec<usize>, Error> {
    let mut cur: Vec<usize> = Vec::new();
    let mut a0: isize = -1;
    let mut black = false;
    let mut i = 0usize;
    while a0 < cols as isize {
        // b1: right of a0, of the colour opposite a0's (even index = a white-to-black change). The search index only
        // moves forward along the reference line except when a0 steps back (VL), which the rewind covers.
        while i > 0 && refl[i - 1] as isize > a0 {
            i -= 1;
        }
        while i < refl.len() && (refl[i] as isize <= a0 || (i % 2 == 1) != black) {
            i += 1;
        }
        let b1 = refl.get(i).copied().unwrap_or(cols);
        let b2 = refl.get(i + 1).copied().unwrap_or(cols);
        if b.left() == 0 {
            return Err(BAD);
        }
        let (mode, len) = t.modes[b.peek(7) as usize];
        if len as usize > b.left() {
            return Err(BAD);
        }
        b.skip(len as usize);
        match mode {
            Mode::Pass => a0 = b2 as isize,
            Mode::Horizontal => {
                let start = a0.max(0) as usize;
                let a1 = (start + run(b, t, black)?).min(cols);
                let a2 = (a1 + run(b, t, !black)?).min(cols);
                push(&mut cur, a1, cols);
                push(&mut cur, a2, cols);
                a0 = a2 as isize;
            }
            Mode::Vertical(d) => {
                let a1 = b1 as isize + d;
                if a1 < a0.max(0) || a1 > cols as isize {
                    return Err(BAD);
                }
                push(&mut cur, a1 as usize, cols);
                a0 = a1;
                black = !black;
            }
            Mode::Extension => return Err(Error::Unsupported("CCITT uncompressed mode")),
            Mode::None => return Err(BAD),
        }
    }
    Ok(cur)
}

/// Undo CCITTFaxDecode: `columns`-pixel rows packed MSB-first, each padded to a byte, black written as 1 when
/// `black_is_1` and as 0 otherwise. Rows decoded before a corrupt code are kept (the Flate rule); a stream whose
/// first row is corrupt is refused.
pub fn decode(data: &[u8], p: &Params) -> Result<Vec<u8>, Error> {
    let cols = p.columns;
    if cols == 0 || cols > 1 << 20 {
        return Err(Error::Syntax("CCITTFaxDecode /Columns", 0));
    }
    let row_bytes = cols.div_ceil(8);
    let t = Tables {
        white: run_table(&WHITE_TERM, &WHITE_MAKEUP),
        black: run_table(&BLACK_TERM, &BLACK_MAKEUP),
        modes: mode_table(),
    };
    let mut b = Bits { data, pos: 0 };
    let mut out: Vec<u8> = Vec::new();
    let mut refl: Vec<usize> = Vec::new();
    let mut nrows = 0usize;
    'rows: while p.rows == 0 || nrows < p.rows {
        if p.byte_align {
            b.align();
        }
        // Fill bits and EOLs; a second EOL in a row (RTC / EOFB) ends the data.
        let mut eols = 0;
        loop {
            if b.left() == 0 {
                break 'rows;
            }
            match b.peek(12) {
                1 => {
                    b.skip(12);
                    eols += 1;
                    if eols == 2 {
                        break 'rows;
                    }
                    if p.k > 0 && b.peek(13) == 0b1_0000_0000_0001 {
                        // EOL+1 EOL+1 ...: T.4 RTC in 2-D mode (the tag bit between the EOLs).
                        b.skip(1);
                    }
                }
                0 => b.skip(1),
                _ => break,
            }
        }
        let two_d = if p.k > 0 {
            let tag = b.peek(1);
            b.skip(1);
            tag == 0
        } else {
            p.k < 0
        };
        let r = if two_d { row_2d(&mut b, &t, &refl, cols) } else { row_1d(&mut b, &t, cols) };
        let cur = match r {
            Ok(c) => c,
            Err(e @ Error::Unsupported(_)) => return Err(e),
            Err(_) if nrows > 0 => break,
            Err(_) => return Err(Error::Syntax("CCITTFaxDecode code", b.pos / 8)),
        };
        if out.len() + row_bytes > MAX_DECODED {
            return Err(Error::TooLarge);
        }
        let start = out.len();
        out.resize(start + row_bytes, if p.black_is_1 { 0 } else { 0xFF });
        let row = &mut out[start..];
        for pair in cur.chunks(2) {
            let from = pair[0];
            let to = pair.get(1).copied().unwrap_or(cols);
            for x in from..to {
                let m = 0x80u8 >> (x % 8);
                if p.black_is_1 {
                    row[x / 8] |= m;
                } else {
                    row[x / 8] &= !m;
                }
            }
        }
        refl = cur;
        nrows += 1;
    }
    Ok(out)
}

/// JBIG2's MMR generic region (T.88 6.2.6): a T.6 picture `w` x `h`, 1 = black, rows white-padded to `h`.
pub(crate) fn mmr(data: &[u8], w: usize, h: usize) -> Result<Vec<u8>, Error> {
    let p = Params { k: -1, columns: w, rows: h, black_is_1: true, ..Params::default() };
    let mut out = decode(data, &p)?;
    out.resize(w.div_ceil(8) * h, 0);
    Ok(out)
}
