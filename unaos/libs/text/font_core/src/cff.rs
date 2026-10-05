//! `CFF ` — Compact Font Format (Adobe TN #5176) with Type 2 charstrings (Adobe TN #5177).
//! Read: header, Name/Top DICT/String/Global Subr INDEXes, Top DICT (CharStrings, Private, charset,
//! ROS/FDArray/FDSelect for CID-keyed fonts), Private DICT (Subrs, defaultWidthX, nominalWidthX), FDSelect
//! formats 0 and 3, and the full Type 2 charstring operator set except `seac`-style accented `endchar`
//! (owed — needs the Standard Encoding table) and the deprecated `random`.

use crate::path::OutlineSink;
use crate::reader::{u16_at, u8_at, Reader};

/// A CFF INDEX: count + offsets + data.
#[derive(Clone, Copy, Debug, Default)]
pub struct Index<'a> {
    count: u32,
    off_size: u8,
    offsets: &'a [u8],
    data: &'a [u8],
}

impl<'a> Index<'a> {
    /// Parse an INDEX at the reader's position; advances the reader past it.
    fn parse(r: &mut Reader<'a>) -> Option<Self> {
        let count = r.u16()? as u32;
        if count == 0 {
            return Some(Index::default());
        }
        let off_size = r.u8()?;
        if !(1..=4).contains(&off_size) {
            return None;
        }
        let offsets = r.bytes((count as usize + 1) * off_size as usize)?;
        let mut idx = Index { count, off_size, offsets, data: &[] };
        let last = idx.offset(count as usize)?;
        if last == 0 {
            return None;
        }
        idx.data = r.bytes(last - 1)?;
        Some(idx)
    }
    fn offset(&self, i: usize) -> Option<usize> {
        let os = self.off_size as usize;
        let b = self.offsets.get(i * os..i * os + os)?;
        let mut v = 0usize;
        for &x in b {
            v = (v << 8) | x as usize;
        }
        Some(v)
    }
    pub fn len(&self) -> usize {
        self.count as usize
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn get(&self, i: usize) -> Option<&'a [u8]> {
        if i >= self.count as usize {
            return None;
        }
        let a = self.offset(i)?.checked_sub(1)?;
        let b = self.offset(i + 1)?.checked_sub(1)?;
        if b < a {
            return None;
        }
        self.data.get(a..b)
    }
}

/// DICT operand/operator stream decoder (TN 5176 §4).
struct Dict<'a> {
    r: Reader<'a>,
}

const MAX_OPERANDS: usize = 48;

impl<'a> Dict<'a> {
    /// Next (operator, operands). Operator 12 x is returned as 1200 + x.
    fn next(&mut self, ops: &mut [f64; MAX_OPERANDS], n: &mut usize) -> Option<u16> {
        *n = 0;
        loop {
            let b0 = self.r.u8()?;
            match b0 {
                0..=21 => {
                    return Some(if b0 == 12 { 1200 + self.r.u8()? as u16 } else { b0 as u16 });
                }
                28 => push(ops, n, self.r.i16()? as f64)?,
                29 => push(ops, n, self.r.i32()? as f64)?,
                30 => {
                    let v = self.real()?;
                    push(ops, n, v)?
                }
                32..=246 => push(ops, n, b0 as f64 - 139.0)?,
                247..=250 => {
                    let b1 = self.r.u8()? as f64;
                    push(ops, n, (b0 as f64 - 247.0) * 256.0 + b1 + 108.0)?
                }
                251..=254 => {
                    let b1 = self.r.u8()? as f64;
                    push(ops, n, -(b0 as f64 - 251.0) * 256.0 - b1 - 108.0)?
                }
                _ => return None, // 22..27, 31, 255 reserved
            }
        }
    }
    /// Real number: packed BCD nibbles (TN 5176 Table 5).
    fn real(&mut self) -> Option<f64> {
        let mut mant: f64 = 0.0;
        let mut frac_div: f64 = 0.0; // 0 = integer part
        let mut neg = false;
        let mut exp: i32 = 0;
        let mut exp_sign: i32 = 0; // 0 = no exponent yet
        let mut in_exp = false;
        loop {
            let b = self.r.u8()?;
            for nib in [b >> 4, b & 15] {
                match nib {
                    0..=9 => {
                        if in_exp {
                            exp = exp.saturating_mul(10).saturating_add(nib as i32);
                        } else if frac_div > 0.0 {
                            mant += nib as f64 / frac_div;
                            frac_div *= 10.0;
                        } else {
                            mant = mant * 10.0 + nib as f64;
                        }
                    }
                    0xa => frac_div = 10.0,
                    0xb => {
                        in_exp = true;
                        exp_sign = 1
                    }
                    0xc => {
                        in_exp = true;
                        exp_sign = -1
                    }
                    0xe => neg = true,
                    0xf => {
                        let mut v = mant;
                        let e = exp * exp_sign;
                        let mut k = 0;
                        while k < e.abs().min(64) {
                            if e > 0 { v *= 10.0 } else { v /= 10.0 }
                            k += 1;
                        }
                        return Some(if neg { -v } else { v });
                    }
                    _ => {}
                }
            }
        }
    }
}

fn push(ops: &mut [f64; MAX_OPERANDS], n: &mut usize, v: f64) -> Option<()> {
    if *n >= MAX_OPERANDS {
        return None;
    }
    ops[*n] = v;
    *n += 1;
    Some(())
}

#[derive(Clone, Copy, Debug, Default)]
struct Private<'a> {
    subrs: Index<'a>,
    default_width: f32,
    nominal_width: f32,
    hint: HintPrivate,
}

/// The Private DICT values the Adobe-style CFF hinter reads (FONTHINT SR62), as FreeType's CFF loader stores them:
/// blue arrays de-delta'd to integers (a real operand floors), BlueScale × 1000 in 16.16.
#[derive(Clone, Copy, Debug)]
pub struct HintPrivate {
    pub blue_values: [i32; 14],
    pub num_blue_values: u8,
    pub other_blues: [i32; 10],
    pub num_other_blues: u8,
    pub family_blues: [i32; 14],
    pub num_family_blues: u8,
    pub family_other_blues: [i32; 10],
    pub num_family_other_blues: u8,
    /// BlueScale × 1000, 16.16 (`cff_parse_fixed_scaled(…, 3)`).
    pub blue_scale_1000: i32,
    pub blue_shift: i32,
    pub blue_fuzz: i32,
    pub language_group: i32,
}

impl Default for HintPrivate {
    fn default() -> Self {
        HintPrivate {
            blue_values: [0; 14],
            num_blue_values: 0,
            other_blues: [0; 10],
            num_other_blues: 0,
            family_blues: [0; 14],
            num_family_blues: 0,
            family_other_blues: [0; 10],
            num_family_other_blues: 0,
            blue_scale_1000: 2_596_864, // 0.039625 × 1000 × 65536
            blue_shift: 7,
            blue_fuzz: 1,
            language_group: 0,
        }
    }
}

fn dict_num(v: f64) -> i32 {
    // cff_parse_num: an integer as is, a real truncated through 16.16 (>> 16 floors)
    crate::fmath::floor(v as f32) as i32
}

fn delta_array<const N: usize>(ops: &[f64], out: &mut [i32; N], count: &mut u8) {
    let mut acc = 0i32;
    let n = ops.len().min(N);
    for i in 0..n {
        acc = acc.wrapping_add(dict_num(ops[i]));
        out[i] = acc;
    }
    *count = n as u8;
}

fn parse_private<'a>(cff: &'a [u8], size: usize, off: usize) -> Option<Private<'a>> {
    let data = cff.get(off..off.checked_add(size)?)?;
    let mut d = Dict { r: Reader::new(data) };
    let mut ops = [0f64; MAX_OPERANDS];
    let mut n = 0;
    let mut p = Private::default();
    while d.r.remaining() > 0 {
        let op = d.next(&mut ops, &mut n)?;
        match op {
            19 if n >= 1 => {
                let so = off.checked_add(ops[0] as usize)?;
                let mut r = Reader::at(cff, so)?;
                p.subrs = Index::parse(&mut r)?;
            }
            20 if n >= 1 => p.default_width = ops[0] as f32,
            21 if n >= 1 => p.nominal_width = ops[0] as f32,
            6 => delta_array(&ops[..n], &mut p.hint.blue_values, &mut p.hint.num_blue_values),
            7 => delta_array(&ops[..n], &mut p.hint.other_blues, &mut p.hint.num_other_blues),
            8 => delta_array(&ops[..n], &mut p.hint.family_blues, &mut p.hint.num_family_blues),
            9 => delta_array(&ops[..n], &mut p.hint.family_other_blues, &mut p.hint.num_family_other_blues),
            1209 if n >= 1 => p.hint.blue_scale_1000 = (ops[0] * 1000.0 * 65536.0 + 0.5) as i32,
            1210 if n >= 1 => p.hint.blue_shift = dict_num(ops[0]),
            1211 if n >= 1 => p.hint.blue_fuzz = dict_num(ops[0]),
            1217 if n >= 1 => p.hint.language_group = dict_num(ops[0]),
            _ => {}
        }
    }
    Some(p)
}

#[derive(Clone, Copy, Debug)]
enum FdSelect<'a> {
    None,
    F0(&'a [u8]),
    F3(&'a [u8], u16),
}

/// A parsed CFF (version 1) font.
#[derive(Clone, Copy, Debug)]
pub struct Cff<'a> {
    data: &'a [u8],
    charstrings: Index<'a>,
    gsubrs: Index<'a>,
    private: Private<'a>,
    fd_array: Index<'a>,
    fd_select: FdSelect<'a>,
    num_glyphs: u16,
    strings: Index<'a>,
    /// Top DICT charset: 0 ISOAdobe, 1 Expert, 2 ExpertSubset (predefined), else an offset.
    charset: usize,
    is_cid: bool,
    /// Top DICT FontMatrix (default 0.001 = 1/1000 em units) — only its scale is honoured.
    pub font_matrix_scale: f32,
}

impl<'a> Cff<'a> {
    pub fn parse(data: &'a [u8], num_glyphs: u16) -> Option<Self> {
        let major = u8_at(data, 0)?;
        let hdr = u8_at(data, 2)? as usize;
        if major != 1 {
            return None;
        }
        let mut r = Reader::at(data, hdr)?;
        let _names = Index::parse(&mut r)?;
        let tops = Index::parse(&mut r)?;
        let strings = Index::parse(&mut r)?;
        let gsubrs = Index::parse(&mut r)?;
        let top = tops.get(0)?;
        let mut d = Dict { r: Reader::new(top) };
        let mut ops = [0f64; MAX_OPERANDS];
        let mut n = 0;
        let mut cs_off = None;
        let mut priv_rng = None;
        let mut fd_array_off = None;
        let mut fd_select_off = None;
        let mut cstype = 2;
        let mut charset = 0usize;
        let mut is_cid = false;
        let mut fm = 0.001f32;
        while d.r.remaining() > 0 {
            let op = d.next(&mut ops, &mut n)?;
            match op {
                15 if n >= 1 => charset = ops[0] as usize,
                17 if n >= 1 => cs_off = Some(ops[0] as usize),
                1230 => is_cid = true,
                18 if n >= 2 => priv_rng = Some((ops[0] as usize, ops[1] as usize)),
                1206 if n >= 1 => cstype = ops[0] as i32,
                1207 if n >= 6 => fm = ops[0] as f32,
                1236 if n >= 1 => fd_array_off = Some(ops[0] as usize),
                1237 if n >= 1 => fd_select_off = Some(ops[0] as usize),
                _ => {}
            }
        }
        if cstype != 2 {
            return None;
        }
        let mut r = Reader::at(data, cs_off?)?;
        let charstrings = Index::parse(&mut r)?;
        let private = match priv_rng {
            Some((size, off)) => parse_private(data, size, off)?,
            None => Private::default(),
        };
        let (fd_array, fd_select) = match (fd_array_off, fd_select_off) {
            (Some(fa), Some(fs)) => {
                let mut r = Reader::at(data, fa)?;
                let fda = Index::parse(&mut r)?;
                let fmt = u8_at(data, fs)?;
                let sel = match fmt {
                    0 => FdSelect::F0(data.get(fs + 1..)?),
                    3 => FdSelect::F3(data.get(fs + 3..)?, u16_at(data, fs + 1)?),
                    _ => return None,
                };
                (fda, sel)
            }
            _ => (Index::default(), FdSelect::None),
        };
        Some(Cff {
            data,
            charstrings,
            gsubrs,
            private,
            fd_array,
            fd_select,
            num_glyphs,
            strings,
            charset,
            is_cid,
            font_matrix_scale: fm,
        })
    }

    /// The SID (or CID, for CID-keyed fonts) the charset assigns to `gid`.
    pub fn glyph_sid(&self, gid: u16) -> Option<u16> {
        if gid == 0 {
            return Some(0);
        }
        if gid as usize >= self.charstrings.len() {
            return None;
        }
        match self.charset {
            0 => return if gid <= 228 { Some(gid) } else { None }, // ISOAdobe: SID = GID
            1 | 2 => return None,                                    // Expert sets: owed
            _ => {}
        }
        let d = self.data;
        let off = self.charset;
        match u8_at(d, off)? {
            0 => u16_at(d, off + 1 + 2 * (gid as usize - 1)),
            f @ (1 | 2) => {
                let mut p = off + 1;
                let mut g = 1u32;
                let wide = f == 2;
                while g < self.charstrings.len() as u32 {
                    let first = u16_at(d, p)? as u32;
                    let left = if wide { u16_at(d, p + 2)? as u32 } else { u8_at(d, p + 2)? as u32 };
                    p += if wide { 4 } else { 3 };
                    if (gid as u32) < g + left + 1 {
                        return u16::try_from(first + (gid as u32 - g)).ok();
                    }
                    g += left + 1;
                }
                None
            }
            _ => None,
        }
    }

    /// The glyph name from the charset and String INDEX (`cidN` for CID-keyed fonts).
    pub fn glyph_name(&self, gid: u16) -> Option<alloc::string::String> {
        let sid = self.glyph_sid(gid)?;
        if self.is_cid {
            return Some(alloc::format!("cid{sid:05}"));
        }
        let s = if (sid as usize) < 391 {
            crate::cff_strings::STANDARD_STRINGS[sid as usize]
        } else {
            core::str::from_utf8(self.strings.get(sid as usize - 391)?).ok()?
        };
        Some(alloc::string::String::from(s))
    }

    pub fn glyph_count(&self) -> usize {
        self.charstrings.len()
    }

    fn private_for(&self, gid: u16) -> Option<Private<'a>> {
        let fd = match self.fd_select {
            FdSelect::None => return Some(self.private),
            FdSelect::F0(s) => *s.get(gid as usize)? as usize,
            FdSelect::F3(s, nr) => {
                let mut found = None;
                for i in 0..nr as usize {
                    let first = u16_at(s, i * 3)?;
                    let fd = *s.get(i * 3 + 2)?;
                    let next = u16_at(s, i * 3 + 3)?;
                    if gid >= first && gid < next {
                        found = Some(fd as usize);
                        break;
                    }
                }
                found?
            }
        };
        let fdict = self.fd_array.get(fd)?;
        let mut d = Dict { r: Reader::new(fdict) };
        let mut ops = [0f64; MAX_OPERANDS];
        let mut n = 0;
        while d.r.remaining() > 0 {
            let op = d.next(&mut ops, &mut n)?;
            if op == 18 && n >= 2 {
                return parse_private(self.data, ops[0] as usize, ops[1] as usize);
            }
        }
        Some(Private::default())
    }

    /// What the CFF hinter needs for `gid`: its charstring, the local and global subroutines, the Private DICT
    /// hint values and nominalWidthX.
    pub fn hint_source(&self, gid: u16) -> Option<(&'a [u8], Index<'a>, Index<'a>, HintPrivate)> {
        if gid >= self.num_glyphs.max(self.charstrings.len() as u16) {
            return None;
        }
        let cs = self.charstrings.get(gid as usize)?;
        let p = self.private_for(gid)?;
        Some((cs, p.subrs, self.gsubrs, p.hint))
    }

    /// Emit the outline of `gid`; returns the charstring's advance width (font units) on success.
    pub fn outline(&self, gid: u16, sink: &mut impl OutlineSink) -> Option<f32> {
        if gid >= self.num_glyphs.max(self.charstrings.len() as u16) {
            return None;
        }
        let cs = self.charstrings.get(gid as usize)?;
        let private = self.private_for(gid)?;
        let mut st = CsState {
            stack: [0.0; 48],
            sp: 0,
            x: 0.0,
            y: 0.0,
            nstems: 0,
            width: None,
            width_seen: false,
            open: false,
            done: false,
            transient: [0.0; 32],
            depth: 0,
            ops: 0,
        };
        let mut sink = Tracking { inner: sink };
        st.run(cs, &private, &self.gsubrs, &mut sink)?;
        if st.open {
            sink.inner.close();
        }
        Some(st.width.map(|w| w + private.nominal_width).unwrap_or(private.default_width))
    }
}

struct Tracking<'s, S: OutlineSink> {
    inner: &'s mut S,
}

fn bias(n: usize) -> i32 {
    if n < 1240 {
        107
    } else if n < 33900 {
        1131
    } else {
        32768
    }
}

struct CsState {
    stack: [f32; 48],
    sp: usize,
    x: f32,
    y: f32,
    nstems: usize,
    width: Option<f32>,
    width_seen: bool,
    open: bool,
    done: bool,
    transient: [f32; 32],
    depth: u8,
    ops: u32,
}

impl CsState {
    fn push(&mut self, v: f32) -> Option<()> {
        if self.sp >= self.stack.len() {
            return None;
        }
        self.stack[self.sp] = v;
        self.sp += 1;
        Some(())
    }
    fn pop(&mut self) -> Option<f32> {
        if self.sp == 0 {
            return None;
        }
        self.sp -= 1;
        Some(self.stack[self.sp])
    }
    /// The first stack-clearing operator may carry the width as an extra leading operand.
    fn take_width(&mut self, has_extra: bool) {
        if !self.width_seen {
            self.width_seen = true;
            if has_extra && self.sp > 0 {
                self.width = Some(self.stack[0]);
                self.stack.copy_within(1..self.sp, 0);
                self.sp -= 1;
            }
        }
    }
    fn move_to<S: OutlineSink>(&mut self, s: &mut Tracking<S>, dx: f32, dy: f32) {
        if self.open {
            s.inner.close();
        }
        self.x += dx;
        self.y += dy;
        s.inner.move_to(self.x, self.y);
        self.open = true;
    }
    fn line_to<S: OutlineSink>(&mut self, s: &mut Tracking<S>, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
        s.inner.line_to(self.x, self.y);
    }
    #[allow(clippy::too_many_arguments)]
    fn curve<S: OutlineSink>(&mut self, s: &mut Tracking<S>, dxa: f32, dya: f32, dxb: f32, dyb: f32, dxc: f32, dyc: f32) {
        let x1 = self.x + dxa;
        let y1 = self.y + dya;
        let x2 = x1 + dxb;
        let y2 = y1 + dyb;
        self.x = x2 + dxc;
        self.y = y2 + dyc;
        s.inner.curve_to(x1, y1, x2, y2, self.x, self.y);
    }

    fn run<S: OutlineSink>(&mut self, cs: &[u8], pv: &Private, g: &Index, s: &mut Tracking<S>) -> Option<()> {
        if self.depth > 10 {
            return None;
        }
        let mut r = Reader::new(cs);
        while r.remaining() > 0 && !self.done {
            self.ops += 1;
            if self.ops > 100_000 {
                return None;
            }
            let b0 = r.u8()?;
            match b0 {
                32..=246 => self.push(b0 as f32 - 139.0)?,
                247..=250 => {
                    let b1 = r.u8()? as f32;
                    self.push((b0 as f32 - 247.0) * 256.0 + b1 + 108.0)?
                }
                251..=254 => {
                    let b1 = r.u8()? as f32;
                    self.push(-(b0 as f32 - 251.0) * 256.0 - b1 - 108.0)?
                }
                28 => {
                    let v = r.i16()? as f32;
                    self.push(v)?
                }
                255 => {
                    let v = r.i32()? as f32 / 65536.0;
                    self.push(v)?
                }
                1 | 3 | 18 | 23 => {
                    // hstem / vstem / hstemhm / vstemhm
                    self.take_width(self.sp % 2 == 1);
                    self.nstems += self.sp / 2;
                    self.sp = 0;
                }
                19 | 20 => {
                    // hintmask / cntrmask: pending operands are an implicit vstem.
                    self.take_width(self.sp % 2 == 1);
                    self.nstems += self.sp / 2;
                    self.sp = 0;
                    r.skip(self.nstems.div_ceil(8))?;
                }
                21 => {
                    self.take_width(self.sp > 2);
                    if self.sp < 2 {
                        return None;
                    }
                    let (dx, dy) = (self.stack[0], self.stack[1]);
                    self.move_to(s, dx, dy);
                    self.sp = 0;
                }
                22 => {
                    self.take_width(self.sp > 1);
                    if self.sp < 1 {
                        return None;
                    }
                    let dx = self.stack[0];
                    self.move_to(s, dx, 0.0);
                    self.sp = 0;
                }
                4 => {
                    self.take_width(self.sp > 1);
                    if self.sp < 1 {
                        return None;
                    }
                    let dy = self.stack[0];
                    self.move_to(s, 0.0, dy);
                    self.sp = 0;
                }
                5 => {
                    let mut i = 0;
                    while i + 2 <= self.sp {
                        let (dx, dy) = (self.stack[i], self.stack[i + 1]);
                        self.line_to(s, dx, dy);
                        i += 2;
                    }
                    self.sp = 0;
                }
                6 | 7 => {
                    // hlineto / vlineto: alternating
                    let mut horiz = b0 == 6;
                    for i in 0..self.sp {
                        let v = self.stack[i];
                        if horiz { self.line_to(s, v, 0.0) } else { self.line_to(s, 0.0, v) }
                        horiz = !horiz;
                    }
                    self.sp = 0;
                }
                8 => {
                    let mut i = 0;
                    while i + 6 <= self.sp {
                        let a = &self.stack;
                        let (p0, p1, p2, p3, p4, p5) = (a[i], a[i + 1], a[i + 2], a[i + 3], a[i + 4], a[i + 5]);
                        self.curve(s, p0, p1, p2, p3, p4, p5);
                        i += 6;
                    }
                    self.sp = 0;
                }
                24 => {
                    // rcurveline: curves then one line
                    if self.sp < 8 {
                        return None;
                    }
                    let mut i = 0;
                    while i + 6 <= self.sp - 2 {
                        let a = self.stack;
                        self.curve(s, a[i], a[i + 1], a[i + 2], a[i + 3], a[i + 4], a[i + 5]);
                        i += 6;
                    }
                    let a = self.stack;
                    self.line_to(s, a[i], a[i + 1]);
                    self.sp = 0;
                }
                25 => {
                    // rlinecurve: lines then one curve
                    if self.sp < 8 {
                        return None;
                    }
                    let mut i = 0;
                    while i + 2 <= self.sp - 6 {
                        let a = self.stack;
                        self.line_to(s, a[i], a[i + 1]);
                        i += 2;
                    }
                    let a = self.stack;
                    self.curve(s, a[i], a[i + 1], a[i + 2], a[i + 3], a[i + 4], a[i + 5]);
                    self.sp = 0;
                }
                26 => {
                    // vvcurveto: dx1? {dya dxb dyb dyc}+
                    let a = self.stack;
                    let mut i = 0;
                    let mut dx1 = 0.0;
                    if self.sp % 4 == 1 {
                        dx1 = a[0];
                        i = 1;
                    }
                    while i + 4 <= self.sp {
                        self.curve(s, dx1, a[i], a[i + 1], a[i + 2], 0.0, a[i + 3]);
                        dx1 = 0.0;
                        i += 4;
                    }
                    self.sp = 0;
                }
                27 => {
                    // hhcurveto: dy1? {dxa dxb dyb dxc}+
                    let a = self.stack;
                    let mut i = 0;
                    let mut dy1 = 0.0;
                    if self.sp % 4 == 1 {
                        dy1 = a[0];
                        i = 1;
                    }
                    while i + 4 <= self.sp {
                        self.curve(s, a[i], dy1, a[i + 1], a[i + 2], a[i + 3], 0.0);
                        dy1 = 0.0;
                        i += 4;
                    }
                    self.sp = 0;
                }
                30 | 31 => {
                    // vhcurveto / hvcurveto: alternating start tangent, optional final df
                    let a = self.stack;
                    let n = self.sp;
                    let mut i = 0;
                    let mut horiz = b0 == 31;
                    while i + 4 <= n {
                        let df = if i + 5 == n { a[i + 4] } else { 0.0 };
                        if horiz {
                            self.curve(s, a[i], 0.0, a[i + 1], a[i + 2], df, a[i + 3]);
                        } else {
                            self.curve(s, 0.0, a[i], a[i + 1], a[i + 2], a[i + 3], df);
                        }
                        horiz = !horiz;
                        i += 4;
                    }
                    self.sp = 0;
                }
                10 | 29 => {
                    let idx = self.pop()? as i32;
                    let (subrs, b) = if b0 == 10 { (&pv.subrs, bias(pv.subrs.len())) } else { (g, bias(g.len())) };
                    let i = idx.checked_add(b)?;
                    if i < 0 {
                        return None;
                    }
                    let sub = subrs.get(i as usize)?;
                    self.depth += 1;
                    self.run(sub, pv, g, s)?;
                    self.depth -= 1;
                }
                11 => return Some(()),
                14 => {
                    // endchar (4-arg seac form is owed: needs Standard Encoding)
                    self.take_width(self.sp == 1 || self.sp == 5);
                    if self.sp >= 4 {
                        return None;
                    }
                    if self.open {
                        s.inner.close();
                        self.open = false;
                    }
                    self.done = true;
                    self.sp = 0;
                }
                12 => {
                    let b1 = r.u8()?;
                    self.escape(b1, s)?;
                }
                _ => return None,
            }
        }
        Some(())
    }

    fn escape<S: OutlineSink>(&mut self, op: u8, s: &mut Tracking<S>) -> Option<()> {
        let a = self.stack;
        let n = self.sp;
        match op {
            35 => {
                // flex: 12 args + fd
                if n < 13 {
                    return None;
                }
                self.curve(s, a[0], a[1], a[2], a[3], a[4], a[5]);
                self.curve(s, a[6], a[7], a[8], a[9], a[10], a[11]);
                self.sp = 0;
            }
            34 => {
                // hflex: dx1 dx2 dy2 dx3 dx4 dx5 dx6
                if n < 7 {
                    return None;
                }
                let y0 = self.y;
                self.curve(s, a[0], 0.0, a[1], a[2], a[3], 0.0);
                let dy = y0 - self.y;
                self.curve(s, a[4], 0.0, a[5], dy, a[6], 0.0);
                self.sp = 0;
            }
            36 => {
                // hflex1: dx1 dy1 dx2 dy2 dx3 dx4 dx5 dy5 dx6
                if n < 9 {
                    return None;
                }
                let y0 = self.y;
                self.curve(s, a[0], a[1], a[2], a[3], a[4], 0.0);
                // dy6 returns the pen to the starting y: -(dy1 + dy2 + dy5).
                let dy6 = y0 - (self.y + a[7]);
                self.curve(s, a[5], 0.0, a[6], a[7], a[8], dy6);
                self.sp = 0;
            }
            37 => {
                // flex1: dx1 dy1 ... dx5 dy5 d6
                if n < 11 {
                    return None;
                }
                let dx = a[0] + a[2] + a[4] + a[6] + a[8];
                let dy = a[1] + a[3] + a[5] + a[7] + a[9];
                self.curve(s, a[0], a[1], a[2], a[3], a[4], a[5]);
                let (d6x, d6y) = if dx.abs() > dy.abs() { (a[10], -dy) } else { (-dx, a[10]) };
                self.curve(s, a[6], a[7], a[8], a[9], d6x, d6y);
                self.sp = 0;
            }
            // Arithmetic and storage operators (TN 5177 §4.4–4.6).
            3 => {
                let b = self.pop()?;
                let c = self.pop()?;
                self.push(if b != 0.0 && c != 0.0 { 1.0 } else { 0.0 })?
            }
            4 => {
                let b = self.pop()?;
                let c = self.pop()?;
                self.push(if b != 0.0 || c != 0.0 { 1.0 } else { 0.0 })?
            }
            5 => {
                let b = self.pop()?;
                self.push(if b == 0.0 { 1.0 } else { 0.0 })?
            }
            9 => {
                let b = self.pop()?;
                self.push(b.abs())?
            }
            10 => {
                let b = self.pop()?;
                let c = self.pop()?;
                self.push(c + b)?
            }
            11 => {
                let b = self.pop()?;
                let c = self.pop()?;
                self.push(c - b)?
            }
            12 => {
                let b = self.pop()?;
                let c = self.pop()?;
                self.push(if b == 0.0 { 0.0 } else { c / b })?
            }
            14 => {
                let b = self.pop()?;
                self.push(-b)?
            }
            15 => {
                let b = self.pop()?;
                let c = self.pop()?;
                self.push(if b == c { 1.0 } else { 0.0 })?
            }
            18 => {
                self.pop()?;
            }
            20 => {
                let i = self.pop()? as i32;
                let v = self.pop()?;
                *self.transient.get_mut(usize::try_from(i).ok()?)? = v;
            }
            21 => {
                let i = self.pop()? as i32;
                let v = *self.transient.get(usize::try_from(i).ok()?)?;
                self.push(v)?
            }
            22 => {
                let v2 = self.pop()?;
                let v1 = self.pop()?;
                let s2 = self.pop()?;
                let s1 = self.pop()?;
                self.push(if v1 <= v2 { s1 } else { s2 })?
            }
            24 => {
                let b = self.pop()?;
                let c = self.pop()?;
                self.push(c * b)?
            }
            26 => {
                let b = self.pop()?;
                self.push(crate::fmath::sqrt(b))?
            }
            27 => {
                let b = self.pop()?;
                self.push(b)?;
                self.push(b)?
            }
            28 => {
                let b = self.pop()?;
                let c = self.pop()?;
                self.push(b)?;
                self.push(c)?
            }
            29 => {
                let i = self.pop()?;
                if self.sp == 0 {
                    return None;
                }
                let i = if i < 0.0 { 0 } else { (i as usize).min(self.sp - 1) };
                let v = self.stack[self.sp - 1 - i];
                self.push(v)?
            }
            30 => {
                let j = self.pop()? as i32;
                let nn = self.pop()? as i32;
                if nn <= 0 || nn as usize > self.sp {
                    return None;
                }
                let nn = nn as usize;
                let base = self.sp - nn;
                let k = j.rem_euclid(nn as i32) as usize;
                self.stack[base..self.sp].rotate_right(k);
            }
            _ => return None, // 23 random and reserved
        }
        Some(())
    }
}
