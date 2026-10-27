// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. JBIG2's Huffman tables (T.88 Annex B) — JBIG2HUFF SR336: the fifteen standard tables,
//! table segments (B.2) and the text region's symbol ID codes (7.4.3.1.7), all coded by the one B.3 assignment and read
//! through the one fax bit reader (`ccitt::Bits`).

use alloc::vec;
use alloc::vec::Vec;

use super::super::ccitt::Bits;
use super::{be32, byte, BAD};
use crate::Error;

/// A standard table (Annex B): its lines (prefix length, range length, range low), the lower range line (prefix length,
/// its top value: the value is top - a 32-bit offset), the upper range line (prefix length, range low; 32-bit offset) and
/// the OOB line's prefix length. Codes are assigned in this order (normal lines, lower, upper, OOB) by B.3.
pub struct Std {
    pub lines: &'static [(u8, u8, i32)],
    pub lower: Option<(u8, i32)>,
    pub upper: Option<(u8, i32)>,
    pub oob: Option<u8>,
}

/// T.88 Table B.1.
pub const B1: Std = Std { lines: &[(1, 4, 0), (2, 8, 16), (3, 16, 272)], lower: None, upper: Some((3, 65808)), oob: None };
/// T.88 Table B.2.
pub const B2: Std =
    Std { lines: &[(1, 0, 0), (2, 0, 1), (3, 0, 2), (4, 3, 3), (5, 6, 11)], lower: None, upper: Some((6, 75)), oob: Some(6) };
/// T.88 Table B.3.
pub const B3: Std = Std {
    lines: &[(8, 8, -256), (1, 0, 0), (2, 0, 1), (3, 0, 2), (4, 3, 3), (5, 6, 11)],
    lower: Some((8, -257)),
    upper: Some((7, 75)),
    oob: Some(6),
};
/// T.88 Table B.4.
pub const B4: Std =
    Std { lines: &[(1, 0, 1), (2, 0, 2), (3, 0, 3), (4, 3, 4), (5, 6, 12)], lower: None, upper: Some((5, 76)), oob: None };
/// T.88 Table B.5.
pub const B5: Std = Std {
    lines: &[(7, 8, -255), (1, 0, 1), (2, 0, 2), (3, 0, 3), (4, 3, 4), (5, 6, 12)],
    lower: Some((7, -256)),
    upper: Some((6, 76)),
    oob: None,
};
/// T.88 Table B.6.
pub const B6: Std = Std {
    lines: &[
        (5, 10, -2048),
        (4, 9, -1024),
        (4, 8, -512),
        (4, 7, -256),
        (5, 6, -128),
        (5, 5, -64),
        (4, 5, -32),
        (2, 7, 0),
        (3, 7, 128),
        (3, 8, 256),
        (4, 9, 512),
        (4, 10, 1024),
    ],
    lower: Some((6, -2049)),
    upper: Some((6, 2048)),
    oob: None,
};
/// T.88 Table B.7.
pub const B7: Std = Std {
    lines: &[
        (4, 9, -1024),
        (3, 8, -512),
        (4, 7, -256),
        (5, 6, -128),
        (5, 5, -64),
        (4, 5, -32),
        (4, 5, 0),
        (5, 5, 32),
        (5, 6, 64),
        (4, 7, 128),
        (3, 8, 256),
        (3, 9, 512),
        (3, 10, 1024),
    ],
    lower: Some((5, -1025)),
    upper: Some((5, 2048)),
    oob: None,
};
/// T.88 Table B.8.
pub const B8: Std = Std {
    lines: &[
        (8, 3, -15),
        (9, 1, -7),
        (8, 1, -5),
        (9, 0, -3),
        (7, 0, -2),
        (4, 0, -1),
        (2, 1, 0),
        (5, 0, 2),
        (6, 0, 3),
        (3, 4, 4),
        (6, 1, 20),
        (4, 4, 22),
        (4, 5, 38),
        (5, 6, 70),
        (5, 7, 134),
        (6, 7, 262),
        (7, 8, 390),
        (6, 10, 646),
    ],
    lower: Some((9, -16)),
    upper: Some((9, 1670)),
    oob: Some(2),
};
/// T.88 Table B.9.
pub const B9: Std = Std {
    lines: &[
        (8, 4, -31),
        (9, 2, -15),
        (8, 2, -11),
        (9, 1, -7),
        (7, 1, -5),
        (4, 1, -3),
        (3, 1, -1),
        (3, 1, 1),
        (5, 1, 3),
        (6, 1, 5),
        (3, 5, 7),
        (6, 2, 39),
        (4, 5, 43),
        (4, 6, 75),
        (5, 7, 139),
        (5, 8, 267),
        (6, 8, 523),
        (7, 9, 779),
        (6, 11, 1291),
    ],
    lower: Some((9, -32)),
    upper: Some((9, 3339)),
    oob: Some(2),
};
/// T.88 Table B.10.
pub const B10: Std = Std {
    lines: &[
        (7, 4, -21),
        (8, 0, -5),
        (7, 0, -4),
        (5, 0, -3),
        (2, 2, -2),
        (5, 0, 2),
        (6, 0, 3),
        (7, 0, 4),
        (8, 0, 5),
        (2, 6, 6),
        (5, 5, 70),
        (6, 5, 102),
        (6, 6, 134),
        (6, 7, 198),
        (6, 8, 326),
        (6, 9, 582),
        (6, 10, 1094),
        (7, 11, 2118),
    ],
    lower: Some((8, -22)),
    upper: Some((8, 4166)),
    oob: Some(2),
};
/// T.88 Table B.11.
pub const B11: Std = Std {
    lines: &[
        (1, 0, 1),
        (2, 1, 2),
        (4, 0, 4),
        (4, 1, 5),
        (5, 1, 7),
        (5, 2, 9),
        (6, 2, 13),
        (7, 2, 17),
        (7, 3, 21),
        (7, 4, 29),
        (7, 5, 45),
        (7, 6, 77),
    ],
    lower: None,
    upper: Some((7, 141)),
    oob: None,
};
/// T.88 Table B.12.
pub const B12: Std = Std {
    lines: &[
        (1, 0, 1),
        (2, 0, 2),
        (3, 1, 3),
        (5, 0, 5),
        (5, 1, 6),
        (6, 1, 8),
        (7, 0, 10),
        (7, 1, 11),
        (7, 2, 13),
        (7, 3, 17),
        (7, 4, 25),
        (8, 5, 41),
    ],
    lower: None,
    upper: Some((8, 73)),
    oob: None,
};
/// T.88 Table B.13.
pub const B13: Std = Std {
    lines: &[
        (1, 0, 1),
        (3, 0, 2),
        (4, 0, 3),
        (5, 0, 4),
        (4, 1, 5),
        (3, 3, 7),
        (6, 1, 15),
        (6, 2, 17),
        (6, 3, 21),
        (6, 4, 29),
        (6, 5, 45),
        (7, 6, 77),
    ],
    lower: None,
    upper: Some((7, 141)),
    oob: None,
};
/// T.88 Table B.14.
pub const B14: Std = Std { lines: &[(3, 0, -2), (3, 0, -1), (1, 0, 0), (3, 0, 1), (3, 0, 2)], lower: None, upper: None, oob: None };
/// T.88 Table B.15.
pub const B15: Std = Std {
    lines: &[
        (7, 4, -24),
        (6, 2, -8),
        (5, 1, -4),
        (4, 0, -2),
        (3, 0, -1),
        (1, 0, 0),
        (3, 0, 1),
        (4, 0, 2),
        (5, 1, 3),
        (6, 2, 5),
        (7, 4, 9),
    ],
    lower: Some((7, -25)),
    upper: Some((7, 25)),
    oob: None,
};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Normal,
    Lower,
    Upper,
    Oob,
}

#[derive(Clone, Copy)]
struct Line {
    prefix: u8,
    range: u8,
    low: i64,
    kind: Kind,
}

/// A table's lines in their B.3 order, the codes they were given, and per prefix length the first code and its lines.
#[derive(Clone)]
pub(super) struct Table {
    lines: Vec<Line>,
    first: Vec<u32>,
    by_len: Vec<Vec<usize>>,
}

/// `n` (<= 32) bits, most significant first; the end of the data is an error.
pub(super) fn read(b: &mut Bits, n: u32) -> Result<u32, Error> {
    if n > 32 || b.left() < n as usize {
        return Err(BAD);
    }
    let mut v = 0u32;
    let mut k = n;
    while k > 0 {
        let c = k.min(16);
        v = ((v as u64) << c) as u32 | b.peek(c);
        b.skip(c as usize);
        k -= c;
    }
    Ok(v)
}

impl Table {
    /// B.3: codes by prefix length, in line order within a length; a prefix length of 0 is a line with no code.
    fn new(lines: Vec<Line>) -> Result<Table, Error> {
        let max = lines.iter().map(|l| l.prefix as usize).max().unwrap_or(0);
        if max > 32 || lines.iter().any(|l| l.range > 32) {
            return Err(BAD);
        }
        let mut count = vec![0u64; max + 1];
        for l in &lines {
            count[l.prefix as usize] += 1;
        }
        count[0] = 0;
        let mut first = vec![0u32; max + 1];
        let mut by_len = vec![Vec::new(); max + 1];
        let mut code = 0u64;
        for len in 1..=max {
            code = (code + count[len - 1]) << 1;
            if code + count[len] > 1u64 << len {
                return Err(BAD);
            }
            first[len] = code as u32;
            by_len[len] = lines.iter().enumerate().filter(|(_, l)| l.prefix as usize == len).map(|(i, _)| i).collect();
        }
        Ok(Table { lines, first, by_len })
    }

    fn std_lines(t: &Std) -> Vec<Line> {
        let mut v: Vec<Line> =
            t.lines.iter().map(|&(p, r, low)| Line { prefix: p, range: r, low: low as i64, kind: Kind::Normal }).collect();
        if let Some((p, top)) = t.lower {
            v.push(Line { prefix: p, range: 32, low: top as i64, kind: Kind::Lower });
        }
        if let Some((p, low)) = t.upper {
            v.push(Line { prefix: p, range: 32, low: low as i64, kind: Kind::Upper });
        }
        if let Some(p) = t.oob {
            v.push(Line { prefix: p, range: 0, low: 0, kind: Kind::Oob });
        }
        v
    }

    pub(super) fn from_std(t: &Std) -> Table {
        // The fifteen tables are complete prefix codes (tests/fax.rs checks them), so the assignment cannot fail.
        Table::new(Table::std_lines(t)).unwrap_or(Table { lines: Vec::new(), first: Vec::new(), by_len: Vec::new() })
    }

    /// `n` values `0..n` with code lengths `lens` (a length of 0: no code).
    fn of_lengths(lens: &[u8]) -> Result<Table, Error> {
        Table::new(lens.iter().enumerate().map(|(i, &p)| Line { prefix: p, range: 0, low: i as i64, kind: Kind::Normal }).collect())
    }

    /// The fixed-length symbol ID codes of a symbol dictionary's aggregate text region (6.5.8.2.1, SBSYMCODELEN bits).
    pub(super) fn fixed(n: usize, len: u32) -> Result<Table, Error> {
        Table::of_lengths(&vec![len as u8; n])
    }

    /// B.2: a table segment (type 53).
    pub(super) fn parse(d: &[u8]) -> Result<Table, Error> {
        let f = byte(d, 0)?;
        let ps = (((f >> 1) & 7) + 1) as u32;
        let rs = (((f >> 4) & 7) + 1) as u32;
        let low = be32(d, 1)? as i32 as i64;
        let high = be32(d, 5)? as i32 as i64;
        let mut b = Bits { data: d, pos: 9 * 8 };
        let mut lines = Vec::new();
        let mut cur = low;
        while cur < high {
            let prefix = read(&mut b, ps)? as u8;
            let range = read(&mut b, rs)?;
            if range > 32 || lines.len() >= 1 << 16 {
                return Err(BAD);
            }
            lines.push(Line { prefix, range: range as u8, low: cur, kind: Kind::Normal });
            cur += 1i64 << range;
        }
        lines.push(Line { prefix: read(&mut b, ps)? as u8, range: 32, low: low - 1, kind: Kind::Lower });
        lines.push(Line { prefix: read(&mut b, ps)? as u8, range: 32, low: high, kind: Kind::Upper });
        if f & 1 != 0 {
            lines.push(Line { prefix: read(&mut b, ps)? as u8, range: 0, low: 0, kind: Kind::Oob });
        }
        Table::new(lines)
    }

    /// One value; `None` is OOB.
    pub(super) fn decode(&self, b: &mut Bits) -> Result<Option<i64>, Error> {
        let mut code = 0u32;
        for len in 1..self.first.len() {
            code = (code << 1) | read(b, 1)?;
            let k = code.wrapping_sub(self.first[len]) as usize;
            if let Some(&i) = self.by_len[len].get(k) {
                if code < self.first[len] {
                    continue;
                }
                let l = self.lines[i];
                return Ok(match l.kind {
                    Kind::Oob => None,
                    Kind::Lower => Some(l.low - read(b, 32)? as i64),
                    _ => Some(l.low + read(b, l.range as u32)? as i64),
                });
            }
        }
        Err(BAD)
    }
}

/// The codes B.3 gives a standard table, in its line order (normal lines, lower, upper, OOB), as (code, length).
pub fn codes(t: &Std) -> Vec<(u32, u8)> {
    let tab = Table::from_std(t);
    let mut out = Vec::new();
    for (len, idx) in tab.by_len.iter().enumerate() {
        for (k, &i) in idx.iter().enumerate() {
            out.push((i, (tab.first[len] + k as u32, len as u8)));
        }
    }
    out.sort_by_key(|&(i, _)| i);
    out.into_iter().map(|(_, c)| c).collect()
}

/// 7.4.3.1.7: a text region's symbol ID table — 35 run-code lengths of 4 bits, then `n` code lengths through the run codes
/// (32: repeat the previous length 3-6 times; 33: 3-10 zeros; 34: 11-138 zeros), then the byte boundary.
pub(super) fn symbol_id_table(b: &mut Bits, n: usize) -> Result<Table, Error> {
    let mut rl = [0u8; 35];
    for r in rl.iter_mut() {
        *r = read(b, 4)? as u8;
    }
    let runs = Table::of_lengths(&rl)?;
    let mut lens: Vec<u8> = Vec::with_capacity(n);
    while lens.len() < n {
        let v = runs.decode(b)?.ok_or(BAD)?;
        let (len, rep) = match v {
            0..=31 => (v as u8, 1),
            32 => (*lens.last().ok_or(BAD)?, 3 + read(b, 2)? as usize),
            33 => (0, 3 + read(b, 3)? as usize),
            _ => (0, 11 + read(b, 7)? as usize),
        };
        if lens.len() + rep > n {
            return Err(BAD);
        }
        lens.extend(core::iter::repeat_n(len, rep));
    }
    b.align();
    Table::of_lengths(&lens)
}
