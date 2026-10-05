//! `glyf` + `loca` — TrueType outlines (OpenType spec, "glyf — Glyph Data", "loca — Index to Location").
//! Simple glyphs (flags with repeats, short/same coordinate encodings) and composite glyphs (word/byte args,
//! x/y offsets or point matching, scale / x-y scale / 2x2 transforms, SCALED/UNSCALED_COMPONENT_OFFSET),
//! recursion bounded at depth 8. Instructions are skipped: FONTCORE does not hint.

use crate::path::OutlineSink;
use crate::reader::{u16_at, u32_at, Reader};
use alloc::vec::Vec;

const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME_OR_POS: u8 = 0x10;
const Y_SAME_OR_POS: u8 = 0x20;

pub const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
pub const ARGS_ARE_XY_VALUES: u16 = 0x0002;
pub const WE_HAVE_A_SCALE: u16 = 0x0008;
pub const MORE_COMPONENTS: u16 = 0x0020;
pub const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
pub const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
pub const SCALED_COMPONENT_OFFSET: u16 = 0x0800;
pub const UNSCALED_COMPONENT_OFFSET: u16 = 0x1000;

const MAX_DEPTH: u8 = 8;
const MAX_POINTS: usize = 1 << 16;

/// One component record of a composite glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Component {
    pub glyph: u16,
    pub flags: u16,
    /// arg1/arg2: an (dx, dy) offset when `ARGS_ARE_XY_VALUES`, else (parent point, child point).
    pub arg1: i32,
    pub arg2: i32,
    /// [a, b, c, d] — x' = a*x + c*y, y' = b*x + d*y.
    pub transform: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct Glyf<'a> {
    glyf: &'a [u8],
    loca: &'a [u8],
    long: bool,
    num_glyphs: u16,
}

#[derive(Clone, Copy, Debug)]
struct Pt {
    x: f32,
    y: f32,
    on: bool,
}

impl<'a> Glyf<'a> {
    pub fn new(glyf: &'a [u8], loca: &'a [u8], long: bool, num_glyphs: u16) -> Option<Self> {
        let need = (num_glyphs as usize + 1) * if long { 4 } else { 2 };
        if loca.len() < need {
            return None;
        }
        Some(Glyf { glyf, loca, long, num_glyphs })
    }

    /// The raw bytes of a glyph (empty slice for a glyph with no outline, e.g. space).
    pub fn glyph_data(&self, gid: u16) -> Option<&'a [u8]> {
        if gid >= self.num_glyphs {
            return None;
        }
        let i = gid as usize;
        let (a, b) = if self.long {
            (u32_at(self.loca, 4 * i)? as usize, u32_at(self.loca, 4 * i + 4)? as usize)
        } else {
            (u16_at(self.loca, 2 * i)? as usize * 2, u16_at(self.loca, 2 * i + 2)? as usize * 2)
        };
        if b < a {
            return None;
        }
        if a == b {
            return Some(&[]);
        }
        self.glyf.get(a..b)
    }

    /// The header bounding box (xMin, yMin, xMax, yMax), `None` for an empty glyph.
    pub fn bbox(&self, gid: u16) -> Option<[i16; 4]> {
        let d = self.glyph_data(gid)?;
        let mut r = Reader::new(d);
        r.i16()?;
        Some([r.i16()?, r.i16()?, r.i16()?, r.i16()?])
    }

    /// numberOfContours (negative = composite). `None` for an empty or missing glyph.
    pub fn contour_count(&self, gid: u16) -> Option<i16> {
        let d = self.glyph_data(gid)?;
        Reader::new(d).i16()
    }

    /// The component records of a composite glyph (empty for a simple glyph).
    pub fn components(&self, gid: u16) -> Option<Vec<Component>> {
        let d = self.glyph_data(gid)?;
        let mut r = Reader::new(d);
        let n = r.i16()?;
        let mut out = Vec::new();
        if n >= 0 {
            return Some(out);
        }
        r.skip(8)?;
        loop {
            let flags = r.u16()?;
            let glyph = r.u16()?;
            let xy = flags & ARGS_ARE_XY_VALUES != 0;
            let (arg1, arg2) = if flags & ARG_1_AND_2_ARE_WORDS != 0 {
                if xy { (r.i16()? as i32, r.i16()? as i32) } else { (r.u16()? as i32, r.u16()? as i32) }
            } else if xy {
                (r.i8()? as i32, r.i8()? as i32)
            } else {
                (r.u8()? as i32, r.u8()? as i32)
            };
            let transform = if flags & WE_HAVE_A_SCALE != 0 {
                let s = r.f2dot14()?;
                [s, 0.0, 0.0, s]
            } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                let sx = r.f2dot14()?;
                let sy = r.f2dot14()?;
                [sx, 0.0, 0.0, sy]
            } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
                [r.f2dot14()?, r.f2dot14()?, r.f2dot14()?, r.f2dot14()?]
            } else {
                [1.0, 0.0, 0.0, 1.0]
            };
            out.push(Component { glyph, flags, arg1, arg2, transform });
            if flags & MORE_COMPONENTS == 0 {
                break;
            }
        }
        Some(out)
    }

    fn load(&self, gid: u16, depth: u8, pts: &mut Vec<Pt>, ends: &mut Vec<usize>) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        let d = self.glyph_data(gid)?;
        if d.is_empty() {
            return Some(());
        }
        let mut r = Reader::new(d);
        let n = r.i16()?;
        r.skip(8)?;
        if n >= 0 {
            let n = n as usize;
            let base = pts.len();
            let mut last_end: Option<usize> = None;
            let mut my_ends = Vec::with_capacity(n);
            for _ in 0..n {
                let e = r.u16()? as usize;
                if let Some(l) = last_end {
                    if e <= l {
                        return None; // endPts must increase strictly
                    }
                }
                last_end = Some(e);
                my_ends.push(e);
            }
            let np = match last_end {
                Some(e) => e + 1,
                None => return Some(()),
            };
            if base + np > MAX_POINTS {
                return None;
            }
            let ilen = r.u16()? as usize;
            r.skip(ilen)?;
            let mut flags = Vec::with_capacity(np);
            while flags.len() < np {
                let f = r.u8()?;
                flags.push(f);
                if f & REPEAT != 0 {
                    let cnt = r.u8()?;
                    for _ in 0..cnt {
                        if flags.len() >= np {
                            break;
                        }
                        flags.push(f);
                    }
                }
            }
            let mut xs = Vec::with_capacity(np);
            let mut v: i32 = 0;
            for &f in &flags {
                if f & X_SHORT != 0 {
                    let dx = r.u8()? as i32;
                    v += if f & X_SAME_OR_POS != 0 { dx } else { -dx };
                } else if f & X_SAME_OR_POS == 0 {
                    v += r.i16()? as i32;
                }
                xs.push(v);
            }
            v = 0;
            for (i, &f) in flags.iter().enumerate() {
                if f & Y_SHORT != 0 {
                    let dy = r.u8()? as i32;
                    v += if f & Y_SAME_OR_POS != 0 { dy } else { -dy };
                } else if f & Y_SAME_OR_POS == 0 {
                    v += r.i16()? as i32;
                }
                pts.push(Pt { x: xs[i] as f32, y: v as f32, on: f & ON_CURVE != 0 });
            }
            for e in my_ends {
                ends.push(base + e);
            }
            Some(())
        } else {
            for c in self.components(gid)? {
                if c.glyph == gid {
                    return None;
                }
                let start = pts.len();
                let mut sub_pts = Vec::new();
                let mut sub_ends = Vec::new();
                self.load(c.glyph, depth + 1, &mut sub_pts, &mut sub_ends)?;
                let [a, b, cc, dd] = c.transform;
                for p in sub_pts.iter_mut() {
                    let (x, y) = (p.x, p.y);
                    p.x = a * x + cc * y;
                    p.y = b * x + dd * y;
                }
                let (dx, dy) = if c.flags & ARGS_ARE_XY_VALUES != 0 {
                    let (mut dx, mut dy) = (c.arg1 as f32, c.arg2 as f32);
                    // Offsets are unscaled (Microsoft default) unless SCALED_COMPONENT_OFFSET says otherwise.
                    if c.flags & SCALED_COMPONENT_OFFSET != 0 && c.flags & UNSCALED_COMPONENT_OFFSET == 0 {
                        let (x, y) = (dx, dy);
                        dx = a * x + cc * y;
                        dy = b * x + dd * y;
                    }
                    (dx, dy)
                } else {
                    // Point matching: parent point arg1 (already loaded) meets child point arg2.
                    let pp = *pts.get(c.arg1 as usize)?;
                    let cp = *sub_pts.get(c.arg2 as usize)?;
                    (pp.x - cp.x, pp.y - cp.y)
                };
                if start + sub_pts.len() > MAX_POINTS {
                    return None;
                }
                for p in sub_pts {
                    pts.push(Pt { x: p.x + dx, y: p.y + dy, on: p.on });
                }
                for e in sub_ends {
                    ends.push(start + e);
                }
            }
            Some(())
        }
    }

    /// The glyph as FreeType loads it with `FT_LOAD_NO_SCALE` (what the auto-hinter analyses): raw points in font
    /// units with their on/off-curve tags and contour ends, composites assembled — component transforms applied in
    /// 16.16 with `FT_MulFix` (F2Dot14 << 2, `FT_Vector_Transform`), x/y offsets unscaled unless
    /// SCALED_COMPONENT_OFFSET (then scaled by the column lengths), or point matching on the assembled points.
    /// `None` for a missing or malformed glyph; an empty outline for a glyph without contours.
    pub fn raw_outline(&self, gid: u16) -> Option<crate::hint::Outline> {
        let mut o = crate::hint::Outline::default();
        self.load_raw(gid, 0, &mut o)?;
        Some(o)
    }

    fn load_raw(&self, gid: u16, depth: u8, o: &mut crate::hint::Outline) -> Option<()> {
        use crate::hint::fixed::mul_fix;
        if depth > MAX_DEPTH {
            return None;
        }
        let d = self.glyph_data(gid)?;
        if d.is_empty() {
            return Some(());
        }
        let mut r = Reader::new(d);
        let n = r.i16()?;
        r.skip(8)?;
        if n >= 0 {
            let n = n as usize;
            let base = o.points.len();
            let mut ends = Vec::with_capacity(n);
            let mut last: Option<usize> = None;
            for _ in 0..n {
                let e = r.u16()? as usize;
                if let Some(l) = last {
                    if e <= l {
                        return None;
                    }
                }
                last = Some(e);
                ends.push(e);
            }
            let np = match last {
                Some(e) => e + 1,
                None => return Some(()),
            };
            if base + np > MAX_POINTS {
                return None;
            }
            let ilen = r.u16()? as usize;
            r.skip(ilen)?;
            let mut flags = Vec::with_capacity(np);
            while flags.len() < np {
                let f = r.u8()?;
                flags.push(f);
                if f & REPEAT != 0 {
                    let cnt = r.u8()?;
                    for _ in 0..cnt {
                        if flags.len() >= np {
                            break;
                        }
                        flags.push(f);
                    }
                }
            }
            let mut xs = Vec::with_capacity(np);
            let mut v: i32 = 0;
            for &f in &flags {
                if f & X_SHORT != 0 {
                    let dx = r.u8()? as i32;
                    v += if f & X_SAME_OR_POS != 0 { dx } else { -dx };
                } else if f & X_SAME_OR_POS == 0 {
                    v += r.i16()? as i32;
                }
                xs.push(v);
            }
            v = 0;
            for (i, &f) in flags.iter().enumerate() {
                if f & Y_SHORT != 0 {
                    let dy = r.u8()? as i32;
                    v += if f & Y_SAME_OR_POS != 0 { dy } else { -dy };
                } else if f & Y_SAME_OR_POS == 0 {
                    v += r.i16()? as i32;
                }
                o.points.push((xs[i] as i64, v as i64));
                o.tags.push(if f & ON_CURVE != 0 { crate::hint::TAG_ON } else { crate::hint::TAG_CONIC });
            }
            for e in ends {
                o.ends.push(base + e);
            }
            Some(())
        } else {
            let comp_start = o.points.len();
            for c in self.components(gid)? {
                if c.glyph == gid {
                    return None;
                }
                let base = o.points.len();
                let mut sub = crate::hint::Outline::default();
                self.load_raw(c.glyph, depth + 1, &mut sub)?;
                // 16.16 matrix from the F2Dot14 values (exact: f32 holds every F2Dot14).
                let m = |v: f32| (v * 16384.0) as i64 * 4;
                let [a, b, cc, dd] = c.transform;
                let have_xform = c.flags & (WE_HAVE_A_SCALE | WE_HAVE_AN_X_AND_Y_SCALE | WE_HAVE_A_TWO_BY_TWO) != 0;
                let (xx, yx, xy, yy) = (m(a), m(b), m(cc), m(dd));
                if have_xform {
                    for p in sub.points.iter_mut() {
                        let (x, y) = *p;
                        *p = (mul_fix(x, xx) + mul_fix(y, xy), mul_fix(x, yx) + mul_fix(y, yy));
                    }
                }
                let (dx, dy) = if c.flags & ARGS_ARE_XY_VALUES != 0 {
                    let (mut dx, mut dy) = (c.arg1 as i64, c.arg2 as i64);
                    if have_xform && c.flags & SCALED_COMPONENT_OFFSET != 0 && c.flags & UNSCALED_COMPONENT_OFFSET == 0 {
                        let len = |p: i64, q: i64| crate::fmath::sqrt((p as f32) * (p as f32) + (q as f32) * (q as f32)) as i64;
                        dx = mul_fix(dx, len(xx, xy));
                        dy = mul_fix(dy, len(yy, yx));
                    }
                    (dx, dy)
                } else {
                    let pp = *o.points.get(comp_start + c.arg1 as usize)?;
                    let cp = *sub.points.get(c.arg2 as usize)?;
                    (pp.0 - cp.0, pp.1 - cp.1)
                };
                if base + sub.points.len() > MAX_POINTS {
                    return None;
                }
                for (p, t) in sub.points.iter().zip(sub.tags.iter()) {
                    o.points.push((p.0 + dx, p.1 + dy));
                    o.tags.push(*t);
                }
                for e in sub.ends {
                    o.ends.push(base + e);
                }
            }
            Some(())
        }
    }

    /// Emit the outline of `gid` (font units). Returns false if the glyph is malformed.
    pub fn outline(&self, gid: u16, sink: &mut impl OutlineSink) -> bool {
        let mut pts = Vec::new();
        let mut ends = Vec::new();
        if self.load(gid, 0, &mut pts, &mut ends).is_none() {
            return false;
        }
        let mut start = 0usize;
        for &end in &ends {
            if end < start || end >= pts.len() {
                return false;
            }
            emit_contour(&pts[start..=end], sink);
            start = end + 1;
        }
        true
    }
}

/// TrueType quadratic contour → path: consecutive off-curve points imply an on-curve midpoint.
fn emit_contour(c: &[Pt], s: &mut impl OutlineSink) {
    let n = c.len();
    if n == 0 {
        return;
    }
    let mid = |a: Pt, b: Pt| Pt { x: (a.x + b.x) * 0.5, y: (a.y + b.y) * 0.5, on: true };
    // Find a starting on-curve point.
    let (first, first_idx) = if c[0].on {
        (c[0], 0usize)
    } else if c[n - 1].on {
        (c[n - 1], n - 1)
    } else {
        (mid(c[n - 1], c[0]), usize::MAX)
    };
    s.move_to(first.x, first.y);
    let mut ctrl: Option<Pt> = None;
    // Walk all points once, starting after the chosen start.
    let begin = if first_idx == usize::MAX { 0 } else { first_idx + 1 };
    for k in 0..n {
        let i = (begin + k) % n;
        if first_idx != usize::MAX && i == first_idx {
            continue;
        }
        let p = c[i];
        if p.on {
            match ctrl.take() {
                Some(q) => s.quad_to(q.x, q.y, p.x, p.y),
                None => s.line_to(p.x, p.y),
            }
        } else {
            if let Some(q) = ctrl {
                let m = mid(q, p);
                s.quad_to(q.x, q.y, m.x, m.y);
            }
            ctrl = Some(p);
        }
    }
    match ctrl {
        Some(q) => s.quad_to(q.x, q.y, first.x, first.y),
        None => s.line_to(first.x, first.y),
    }
    s.close();
}
