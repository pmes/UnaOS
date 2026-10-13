//! The font: table directory (OpenType spec, "Organization of an OpenType Font"), TrueType Collections,
//! and the metric tables `head`, `hhea`, `maxp`, `hmtx`, `OS/2`, `post`.

use crate::cff::Cff;
use crate::cmap::{Cmap, CmapSubtable};
use crate::glyf::Glyf;
use crate::layout::{Gdef, Gpos, Gsub, Kern};
use crate::path::{OutlineSink, Path};
use crate::reader::{i16_at, slice, u16_at, u32_at};
use crate::var::{Avar, Axis, Fvar, Gvar, Hvar, ItemVariationStore, Mvar, MAX_AXES};
use crate::var::{MVAR_CPHT, MVAR_HASC, MVAR_HCLA, MVAR_HCLD, MVAR_HDSC, MVAR_HLGP, MVAR_UNDO, MVAR_UNDS, MVAR_XHGT};
use alloc::string::String;
use alloc::vec::Vec;

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
    /// FONTSCRAFT (B640): the variation tables (all `None` for a static face) …
    pub fvar: Option<Fvar<'a>>,
    pub avar: Option<Avar<'a>>,
    pub gvar: Option<Gvar<'a>>,
    pub hvar: Option<Hvar<'a>>,
    /// FONTS2 (B650): `MVAR`, the metrics variations — line metrics, x-height, cap height, underline per instance …
    pub mvar: Option<Mvar<'a>>,
    /// … `GDEF`'s ItemVariationStore (GPOS ValueRecord / Anchor device deltas) …
    pub layout_store: Option<ItemVariationStore<'a>>,
    /// … and this instance's normalized coordinates, one per `fvar` axis (all 0 = the default instance).
    coords: [f32; MAX_AXES],
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
            fvar: None,
            avar: None,
            gvar: None,
            hvar: None,
            mvar: None,
            layout_store: None,
            coords: [0.0; MAX_AXES],
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
        // FONTSCRAFT (B640): variations. A face whose `fvar` does not parse is read as static.
        f.fvar = f.table(b"fvar").and_then(Fvar::parse);
        if f.fvar.is_some() {
            f.avar = f.table(b"avar").and_then(Avar::parse);
            if matches!(f.outlines, Outlines::Glyf(_)) {
                f.gvar = f.table(b"gvar").and_then(Gvar::parse);
            }
            f.hvar = f.table(b"HVAR").and_then(Hvar::parse);
            f.mvar = f.table(b"MVAR").and_then(Mvar::parse); // FONTS2 (B650)
            f.layout_store = f.table(b"GDEF").and_then(gdef_var_store);
        }
        Ok(f)
    }

    // ------------------------------------------------------------------------------------- variations (B640)

    /// The face has an `fvar` table.
    pub fn is_variable(&self) -> bool {
        self.fvar.is_some()
    }

    /// The variation axes (empty for a static face).
    pub fn axes(&self) -> Vec<Axis> {
        self.fvar.map_or_else(Vec::new, |f| f.axes())
    }

    /// The face has axis `tag` (`wght`, `opsz`, `wdth`, …).
    pub fn has_axis(&self, tag: [u8; 4]) -> bool {
        self.fvar.and_then(|f| f.index_of(tag)).is_some()
    }

    /// This instance's normalized coordinates (one per axis, up to [`MAX_AXES`]).
    pub fn coords(&self) -> &[f32] {
        let n = self.fvar.map_or(0, |f| (f.axis_count as usize).min(MAX_AXES));
        &self.coords[..n]
    }

    /// Any coordinate is off the default.
    pub fn is_instanced(&self) -> bool {
        self.coords.iter().any(|&c| c != 0.0)
    }

    /// Set axis `tag` to user value `value` (clamped to the axis, normalized through `avar`). Unknown axes and
    /// static faces are ignored. Other axes keep their value.
    pub fn set_variation(&mut self, tag: [u8; 4], value: f32) {
        let Some(fv) = self.fvar else { return };
        let Some(i) = fv.index_of(tag) else { return };
        if i < MAX_AXES {
            self.coords[i] = fv.normalize(i, value, self.avar.as_ref());
        }
    }

    /// A copy of this face at the instance `settings` (each `(axis tag, user value)`), other axes as they are.
    pub fn with_variations(&self, settings: &[([u8; 4], f32)]) -> Font<'a> {
        let mut f = *self;
        for &(t, v) in settings {
            f.set_variation(t, v);
        }
        f
    }

    /// Back to the default instance.
    pub fn reset_variations(&mut self) {
        self.coords = [0.0; MAX_AXES];
    }

    /// The user value of axis `tag` at this instance (the inverse of the default normalization; `avar` is not
    /// inverted, so this is exact only where its map is the identity) — for witnesses.
    pub fn variation_value(&self, tag: [u8; 4]) -> Option<f32> {
        let fv = self.fvar?;
        let i = fv.index_of(tag)?;
        let a = fv.axis(i)?;
        let c = *self.coords.get(i)?;
        Some(if c < 0.0 { a.default + c * (a.default - a.min) } else { a.default + c * (a.max - a.default) })
    }

    /// The device delta of a VariationIndex table at `d` (Device table with deltaFormat 0x8000) for this instance,
    /// in font units; 0 for a hinting Device table, a static face or the default instance.
    pub fn device_delta(&self, d: &[u8]) -> f32 {
        if !self.is_instanced() {
            return 0.0;
        }
        let Some(store) = self.layout_store else { return 0.0 };
        if u16_at(d, 4) != Some(0x8000) {
            return 0.0;
        }
        match (u16_at(d, 0), u16_at(d, 2)) {
            (Some(outer), Some(inner)) => store.delta(outer, inner, self.coords()),
            _ => 0.0,
        }
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

    /// Every (code point, glyph) pair of the Unicode cmap, sorted by code point (empty when the face has no
    /// Unicode cmap). The auto-hinter assigns glyphs to scripts from it.
    pub fn unicode_map(&self) -> Vec<(u32, u16)> {
        let mut v = Vec::new();
        if let Some(st) = self.best_cmap {
            if st.platform_id == 0 || (st.platform_id == 3 && (st.encoding_id == 1 || st.encoding_id == 10)) {
                st.for_each(|cp, g| {
                    if g != 0 {
                        v.push((cp, g))
                    }
                });
            }
        }
        v.sort_unstable();
        v.dedup_by_key(|e| e.0);
        v
    }

    /// Advance width in font units (at this instance: `HVAR`'s delta, else `gvar`'s phantom points, rounded).
    pub fn advance(&self, gid: u16) -> u16 {
        let i = (gid as usize).min(self.num_hmetrics as usize - 1);
        let base = u16_at(self.hmtx, 4 * i).unwrap_or(0);
        if !self.is_instanced() {
            return base;
        }
        let d = self.advance_delta(gid);
        crate::fmath::round(base as f32 + d).clamp(0.0, 65535.0) as u16
    }

    /// The advance's variation delta at this instance, fractional font units.
    pub fn advance_delta(&self, gid: u16) -> f32 {
        if !self.is_instanced() {
            return 0.0;
        }
        if let Some(h) = &self.hvar {
            return h.advance_delta(gid, self.coords());
        }
        if let (Some(gv), Outlines::Glyf(g)) = (&self.gvar, &self.outlines) {
            if let Some(n) = g.var_point_count(gid) {
                if let Some(d) = gv.deltas(gid, n + 4, None, self.coords()) {
                    // Phantom points 1 and 2 (no inference: an unreferenced phantom point does not move).
                    return d[n + 1].0 - d[n].0;
                }
            }
        }
        0.0
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
            Outlines::Glyf(g) => match (&self.gvar, self.is_instanced()) {
                (Some(gv), true) => g.outline_var(gid, sink, Some((gv, self.coords()))),
                _ => g.outline(gid, sink),
            },
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

    /// FONTS2 (B650): the `MVAR` delta of metric `tag` at this instance, in fractional font units — 0 for a static
    /// face, the default instance, a face without `MVAR`, or a tag the table does not carry.
    pub fn metric_delta(&self, tag: [u8; 4]) -> f32 {
        if !self.is_instanced() {
            return 0.0;
        }
        match &self.mvar {
            Some(m) => m.delta(tag, self.coords()),
            None => 0.0,
        }
    }

    /// `v` moved by the rounded `MVAR` delta of `tag` at this instance (the spec: apply the delta, then round).
    fn metric(&self, v: i16, tag: [u8; 4]) -> i16 {
        let d = self.metric_delta(tag);
        if d == 0.0 {
            return v;
        }
        crate::fmath::round(v as f32 + d).clamp(-32768.0, 32767.0) as i16
    }

    /// Line metrics in font units, chosen the way browsers do: OS/2 typo metrics when USE_TYPO_METRICS
    /// (fsSelection bit 7) is set, else hhea. FONTS2 (B650): at an instance the `MVAR` deltas `hasc` / `hdsc` /
    /// `hlgp` move whichever source was chosen, HarfBuzz's `hb_ot_metrics_get_position` rule — the tags name the
    /// OS/2 typo fields in the spec, and a face that varies its typo metrics means its line box to follow.
    pub fn line_metrics(&self) -> (i16, i16, i16) {
        let (a, d, g) = match self.os2 {
            Some(o) if o.fs_selection & 0x80 != 0 => (o.typo_ascender, o.typo_descender, o.typo_line_gap),
            _ => (self.ascender, self.descender, self.line_gap),
        };
        (self.metric(a, MVAR_HASC), self.metric(d, MVAR_HDSC), self.metric(g, MVAR_HLGP))
    }

    /// FONTS2 (B650): OS/2 `usWinAscent` / `usWinDescent` (the clipping box) at this instance (`hcla`, `hcld`).
    pub fn win_metrics(&self) -> Option<(u16, u16)> {
        let o = self.os2?;
        let f = |v: u16, t: [u8; 4]| crate::fmath::round(v as f32 + self.metric_delta(t)).clamp(0.0, 65535.0) as u16;
        Some((f(o.win_ascent, MVAR_HCLA), f(o.win_descent, MVAR_HCLD)))
    }

    /// FONTS2 (B650): OS/2 `sxHeight` at this instance (`xhgt`); `None` when the OS/2 table is older than version 2.
    pub fn x_height(&self) -> Option<i16> {
        Some(self.metric(self.os2?.x_height?, MVAR_XHGT))
    }

    /// FONTS2 (B650): OS/2 `sCapHeight` at this instance (`cpht`).
    pub fn cap_height(&self) -> Option<i16> {
        Some(self.metric(self.os2?.cap_height?, MVAR_CPHT))
    }

    /// FONTS2 (B650): `post` underline (position, thickness) at this instance (`undo`, `unds`).
    pub fn underline(&self) -> Option<(i16, i16)> {
        let p = self.post?;
        Some((self.metric(p.underline_position, MVAR_UNDO), self.metric(p.underline_thickness, MVAR_UNDS)))
    }
}

/// `GDEF` 1.3's ItemVariationStore (offset at byte 14 when minor >= 3).
fn gdef_var_store(d: &[u8]) -> Option<ItemVariationStore<'_>> {
    if u16_at(d, 0)? != 1 || u16_at(d, 2)? < 3 {
        return None;
    }
    let off = u32_at(d, 14)? as usize;
    if off == 0 {
        return None;
    }
    ItemVariationStore::parse(d.get(off..)?)
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
