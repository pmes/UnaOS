//! The font: table directory (OpenType spec, "Organization of an OpenType Font"), TrueType Collections,
//! and the metric tables `head`, `hhea`, `maxp`, `hmtx`, `OS/2`, `post`.

use crate::cff::Cff;
use crate::cmap::{Cmap, CmapSubtable};
use crate::glyf::Glyf;
use crate::layout::{Gdef, Gpos, Gsub, Kern};
use crate::path::{OutlineSink, Path};
use crate::reader::{i16_at, slice, u16_at, u32_at};
use alloc::string::String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data ended before a required structure.
    Truncated,
    /// Not an sfnt (TrueType / OpenType / collection) signature.
    BadMagic,
    /// A required table is missing; the tag names it.
    MissingTable([u8; 4]),
    /// A table is present but violates the spec.
    Malformed([u8; 4]),
    /// Collection index out of range.
    NoSuchFace,
}

/// `OS/2` fields the shaper and layout need.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Os2 {
    pub version: u16,
    pub x_avg_char_width: i16,
    pub weight_class: u16,
    pub width_class: u16,
    pub fs_selection: u16,
    pub typo_ascender: i16,
    pub typo_descender: i16,
    pub typo_line_gap: i16,
    pub win_ascent: u16,
    pub win_descent: u16,
    pub x_height: Option<i16>,
    pub cap_height: Option<i16>,
}

/// `post` header fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Post {
    pub version: u32,
    pub italic_angle: f32,
    pub underline_position: i16,
    pub underline_thickness: i16,
    pub is_fixed_pitch: bool,
}

/// The outline source (Copy, borrowed: the size difference between variants costs nothing worth boxing).
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug)]
pub enum Outlines<'a> {
    Glyf(Glyf<'a>),
    Cff(Cff<'a>),
    None,
}

/// A parsed font face borrowing the file bytes.
#[derive(Clone, Copy, Debug)]
pub struct Font<'a> {
    data: &'a [u8],
    dir: usize,
    num_tables: u16,
    pub units_per_em: u16,
    pub head_bbox: [i16; 4],
    pub mac_style: u16,
    pub num_glyphs: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    num_hmetrics: u16,
    hmtx: &'a [u8],
    cmap: Option<Cmap<'a>>,
    best_cmap: Option<CmapSubtable<'a>>,
    pub os2: Option<Os2>,
    pub post: Option<Post>,
    post_data: Option<&'a [u8]>,
    pub outlines: Outlines<'a>,
    pub gsub: Option<Gsub<'a>>,
    pub gpos: Option<Gpos<'a>>,
    pub gdef: Option<Gdef<'a>>,
    pub kern: Option<Kern<'a>>,
}

const TTCF: u32 = 0x7474_6366;

impl<'a> Font<'a> {
    /// Number of faces in the file (1 for a plain sfnt).
    pub fn face_count(data: &[u8]) -> u32 {
        match u32_at(data, 0) {
            Some(TTCF) => u32_at(data, 8).unwrap_or(0),
            Some(_) => 1,
            None => 0,
        }
    }

    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        Self::parse_face(data, 0)
    }

    pub fn parse_face(data: &'a [u8], face: u32) -> Result<Self, Error> {
        let tag = u32_at(data, 0).ok_or(Error::Truncated)?;
        let dir = if tag == TTCF {
            let n = u32_at(data, 8).ok_or(Error::Truncated)?;
            if face >= n {
                return Err(Error::NoSuchFace);
            }
            u32_at(data, 12 + 4 * face as usize).ok_or(Error::Truncated)? as usize
        } else if face == 0 {
            0
        } else {
            return Err(Error::NoSuchFace);
        };
        let sv = u32_at(data, dir).ok_or(Error::Truncated)?;
        if !matches!(sv, 0x0001_0000 | 0x4F54_544F | 0x7472_7565) {
            return Err(Error::BadMagic);
        }
        let num_tables = u16_at(data, dir + 4).ok_or(Error::Truncated)?;
        slice(data, dir + 12, num_tables as usize * 16).ok_or(Error::Truncated)?;
        let mut f = Font {
            data,
            dir,
            num_tables,
            units_per_em: 0,
            head_bbox: [0; 4],
            mac_style: 0,
            num_glyphs: 0,
            ascender: 0,
            descender: 0,
            line_gap: 0,
            num_hmetrics: 0,
            hmtx: &[],
            cmap: None,
            best_cmap: None,
            os2: None,
            post: None,
            post_data: None,
            outlines: Outlines::None,
            gsub: None,
            gpos: None,
            gdef: None,
            kern: None,
        };
        // head
        let head = f.table_req(b"head")?;
        let m = |t: &[u8; 4]| Error::Malformed(*t);
        if u32_at(head, 12) != Some(0x5F0F_3CF5) {
            return Err(m(b"head"));
        }
        f.units_per_em = u16_at(head, 18).ok_or(m(b"head"))?;
        if !(16..=16384).contains(&f.units_per_em) {
            return Err(m(b"head"));
        }
        for i in 0..4 {
            f.head_bbox[i] = i16_at(head, 36 + 2 * i).ok_or(m(b"head"))?;
        }
        f.mac_style = u16_at(head, 44).ok_or(m(b"head"))?;
        let loc_fmt = i16_at(head, 50).ok_or(m(b"head"))?;
        // maxp
        let maxp = f.table_req(b"maxp")?;
        f.num_glyphs = u16_at(maxp, 4).ok_or(m(b"maxp"))?;
        // hhea + hmtx
        let hhea = f.table_req(b"hhea")?;
        f.ascender = i16_at(hhea, 4).ok_or(m(b"hhea"))?;
        f.descender = i16_at(hhea, 6).ok_or(m(b"hhea"))?;
        f.line_gap = i16_at(hhea, 8).ok_or(m(b"hhea"))?;
        f.num_hmetrics = u16_at(hhea, 34).ok_or(m(b"hhea"))?;
        f.hmtx = f.table(b"hmtx").unwrap_or(&[]);
        if f.num_hmetrics == 0 || f.hmtx.len() < 4 * f.num_hmetrics as usize {
            return Err(m(b"hmtx"));
        }
        // cmap (optional — a font without one maps nothing)
        if let Some(c) = f.table(b"cmap") {
            f.cmap = Cmap::parse(c);
            f.best_cmap = f.cmap.and_then(|c| c.best());
        }
        f.os2 = f.table(b"OS/2").and_then(parse_os2);
        if let Some(p) = f.table(b"post") {
            f.post = parse_post(p);
            f.post_data = Some(p);
        }
        // outlines
        if let (Some(glyf), Some(loca)) = (f.table(b"glyf"), f.table(b"loca")) {
            if let Some(g) = Glyf::new(glyf, loca, loc_fmt == 1, f.num_glyphs) {
                f.outlines = Outlines::Glyf(g);
            } else {
                return Err(m(b"loca"));
            }
        } else if let Some(c) = f.table(b"CFF ") {
            f.outlines = Outlines::Cff(Cff::parse(c, f.num_glyphs).ok_or(m(b"CFF "))?);
        }
        f.gdef = f.table(b"GDEF").and_then(Gdef::parse);
        f.gsub = f.table(b"GSUB").and_then(Gsub::parse);
        f.gpos = f.table(b"GPOS").and_then(Gpos::parse);
        f.kern = f.table(b"kern").and_then(Kern::parse);
        Ok(f)
    }

    /// A table's bytes by tag, bounds-checked against the file.
    pub fn table(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        for i in 0..self.num_tables as usize {
            let rec = self.dir + 12 + 16 * i;
            if slice(self.data, rec, 4)? == tag {
                let off = u32_at(self.data, rec + 8)? as usize;
                let len = u32_at(self.data, rec + 12)? as usize;
                return slice(self.data, off, len);
            }
        }
        None
    }

    fn table_req(&self, tag: &[u8; 4]) -> Result<&'a [u8], Error> {
        self.table(tag).ok_or(Error::MissingTable(*tag))
    }

    /// The table tags present, in directory order.
    pub fn table_tags(&self) -> impl Iterator<Item = [u8; 4]> + '_ {
        (0..self.num_tables as usize).filter_map(move |i| {
            let s = slice(self.data, self.dir + 12 + 16 * i, 4)?;
            Some([s[0], s[1], s[2], s[3]])
        })
    }

    pub fn cmap(&self) -> Option<Cmap<'a>> {
        self.cmap
    }

    /// Unicode code point → glyph id (0 = .notdef / unmapped).
    pub fn glyph_index(&self, c: char) -> u16 {
        let Some(st) = self.best_cmap else { return 0 };
        let cp = c as u32;
        if let Some(g) = st.glyph(cp) {
            return g;
        }
        if st.platform_id == 3 && st.encoding_id == 0 && cp < 0x100 {
            // Symbol fonts park their repertoire at U+F000.
            return st.glyph(0xF000 | cp).unwrap_or(0);
        }
        0
    }

    /// Advance width in font units.
    pub fn advance(&self, gid: u16) -> u16 {
        let i = (gid as usize).min(self.num_hmetrics as usize - 1);
        u16_at(self.hmtx, 4 * i).unwrap_or(0)
    }

    /// Left side bearing in font units.
    pub fn lsb(&self, gid: u16) -> i16 {
        let n = self.num_hmetrics as usize;
        let g = gid as usize;
        if g < n {
            i16_at(self.hmtx, 4 * g + 2).unwrap_or(0)
        } else {
            i16_at(self.hmtx, 4 * n + 2 * (g - n)).unwrap_or(0)
        }
    }

    /// Emit a glyph outline (font units, y up). False if the glyph is missing or malformed.
    pub fn outline(&self, gid: u16, sink: &mut impl OutlineSink) -> bool {
        if gid >= self.num_glyphs {
            return false;
        }
        match &self.outlines {
            Outlines::Glyf(g) => g.outline(gid, sink),
            Outlines::Cff(c) => c.outline(gid, sink).is_some(),
            Outlines::None => false,
        }
    }

    /// A glyph outline recorded as a [`Path`].
    pub fn glyph_path(&self, gid: u16) -> Option<Path> {
        let mut p = Path::new();
        if self.outline(gid, &mut p) { Some(p) } else { None }
    }

    /// The name of a glyph: `post` format 2.0 (with the Macintosh standard names) or 1.0, else the CFF charset.
    pub fn glyph_name(&self, gid: u16) -> Option<String> {
        if let Some(n) = self.post_glyph_name(gid) {
            return Some(n);
        }
        match &self.outlines {
            Outlines::Cff(c) => c.glyph_name(gid),
            _ => None,
        }
    }

    fn post_glyph_name(&self, gid: u16) -> Option<String> {
        let p = self.post_data?;
        let ver = u32_at(p, 0)?;
        let mac = &crate::post_names::MAC_GLYPH_NAMES;
        match ver {
            0x0001_0000 => mac.get(gid as usize).map(|s| String::from(*s)),
            0x0002_0000 => {
                let n = u16_at(p, 32)?;
                if gid >= n {
                    return None;
                }
                let idx = u16_at(p, 34 + 2 * gid as usize)? as usize;
                if idx < 258 {
                    return Some(String::from(mac[idx]));
                }
                // Walk the Pascal strings to entry idx-258.
                let mut off = 34 + 2 * n as usize;
                for _ in 0..idx - 258 {
                    let l = *p.get(off)? as usize;
                    off += 1 + l;
                }
                let l = *p.get(off)? as usize;
                let s = p.get(off + 1..off + 1 + l)?;
                core::str::from_utf8(s).ok().map(String::from)
            }
            _ => None,
        }
    }

    /// Line metrics in font units, chosen the way browsers do: OS/2 typo metrics when USE_TYPO_METRICS
    /// (fsSelection bit 7) is set, else hhea.
    pub fn line_metrics(&self) -> (i16, i16, i16) {
        if let Some(o) = self.os2 {
            if o.fs_selection & 0x80 != 0 {
                return (o.typo_ascender, o.typo_descender, o.typo_line_gap);
            }
        }
        (self.ascender, self.descender, self.line_gap)
    }
}

fn parse_os2(d: &[u8]) -> Option<Os2> {
    let version = u16_at(d, 0)?;
    let mut o = Os2 {
        version,
        x_avg_char_width: i16_at(d, 2)?,
        weight_class: u16_at(d, 4)?,
        width_class: u16_at(d, 6)?,
        fs_selection: u16_at(d, 62)?,
        typo_ascender: i16_at(d, 68)?,
        typo_descender: i16_at(d, 70)?,
        typo_line_gap: i16_at(d, 72)?,
        win_ascent: u16_at(d, 74)?,
        win_descent: u16_at(d, 76)?,
        x_height: None,
        cap_height: None,
    };
    if version >= 2 {
        o.x_height = i16_at(d, 86);
        o.cap_height = i16_at(d, 88);
    }
    Some(o)
}

fn parse_post(d: &[u8]) -> Option<Post> {
    Some(Post {
        version: u32_at(d, 0)?,
        italic_angle: u32_at(d, 4)? as i32 as f32 / 65536.0,
        underline_position: i16_at(d, 8)?,
        underline_thickness: i16_at(d, 10)?,
        is_fixed_pitch: u32_at(d, 12)? != 0,
    })
}
