//! The shaper. `shape` runs UAX #9 over the text, splits each level run into script runs (Scripts.txt +
//! Script_Extensions), and shapes every run in its direction through one OpenType pipeline: font-aware
//! normalization (decompose what the font lacks, reorder marks by — modified — combining class, recompose what the
//! font has), the script's complex shaper (Arabic joining from ArabicShaping.txt; Indic syllables, reph and matra
//! reordering for Devanagari from IndicSyllabicCategory / IndicPositionalCategory; Thai SARA AM decomposition),
//! RTL mirroring (Bidi_Mirroring_Glyph), GSUB features in staged order with per-glyph masks, GPOS (kerning, marks,
//! cursive) with the legacy `kern` table as the fallback exactly when GPOS has no `kern` feature, mark advance
//! zeroing and attachment propagation, default-ignorable hiding. Glyphs come out in VISUAL order (left to right)
//! with `cluster` naming the source byte offset — what `draw_text`, `measure` and svg_core's `<text>` consume.

use crate::bidi::{BidiInfo, Direction};
use crate::cache::{split_position, GlyphCache};
use crate::complex;
use crate::layout::{LayoutTable, Tag};
use crate::linebreak::{breaks, Break};
use crate::normalize;
use crate::ot::{self, Apply, GlyphInfo, GlyphPosition, LookupReq, IGN_NONE, IGN_OTHER, IGN_ZWJ, IGN_ZWNJ};
use crate::script::{self, Script};
use crate::ucd::{self, Gc};
use crate::Font;
use alloc::vec::Vec;

/// A positioned glyph, in font units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphPos {
    pub glyph: u16,
    /// Byte offset of the first source character this glyph represents.
    pub cluster: usize,
    pub x_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct ShapeOptions {
    pub kerning: bool,
    pub ligatures: bool,
}

impl Default for ShapeOptions {
    fn default() -> Self {
        ShapeOptions { kerning: true, ligatures: true }
    }
}

/// Split text into (byte start, byte end, ISO 15924 script) runs — [`script::itemize`].
pub fn itemize(text: &str) -> Vec<(usize, usize, Script)> {
    script::itemize(text)
}

// ---------------------------------------------------------------------------------------------------------- plan

pub(crate) const F_GLOBAL: u8 = 1;
pub(crate) const F_MANUAL_ZWJ: u8 = 2;
pub(crate) const F_MANUAL_ZWNJ: u8 = 4;
pub(crate) const F_PER_SYLLABLE: u8 = 8;
pub(crate) const F_MANUAL_JOINERS: u8 = F_MANUAL_ZWJ | F_MANUAL_ZWNJ;

/// What runs between GSUB stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pause {
    None,
    IndicInitial,
    IndicFinal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shaper {
    Default,
    Arabic,
    Indic,
    Thai,
    Hebrew,
}

impl Shaper {
    fn for_script(s: Script) -> Shaper {
        match &s {
            b"Arab" | b"Syrc" | b"Nkoo" | b"Mong" | b"Phag" | b"Mand" | b"Mani" | b"Adlm" | b"Rohg" | b"Sogd"
            | b"Chrs" | b"Ougr" => Shaper::Arabic,
            b"Deva" => Shaper::Indic,
            b"Thai" | b"Laoo" => Shaper::Thai,
            b"Hebr" => Shaper::Hebrew,
            _ => Shaper::Default,
        }
    }
}

struct FeatureReq {
    tag: Tag,
    flags: u8,
    stage: usize,
}

/// A compiled shaping plan for (font, script, direction, options).
pub(crate) struct Plan {
    pub script_tags: Vec<Tag>,
    pub shaper: Shaper,
    /// GSUB lookups per stage, sorted by lookup index.
    pub gsub: Vec<Vec<LookupReq>>,
    /// Callback after each stage.
    pub pauses: Vec<Pause>,
    pub gpos: Vec<LookupReq>,
    /// (feature tag, mask bit) for masked features.
    pub masks: Vec<(Tag, u32)>,
    pub apply_kern_table: bool,
}

pub(crate) const GLOBAL_MASK: u32 = 1;

impl Plan {
    pub fn mask(&self, tag: Tag) -> u32 {
        self.masks.iter().find(|m| m.0 == tag).map_or(0, |m| m.1)
    }

    fn new(font: &Font, script: Script, rtl: bool, opts: &ShapeOptions) -> Plan {
        let shaper = Shaper::for_script(script);
        let mut script_tags = script::ot_script_tags(script);
        // Indic: the font may only carry the old tag; Default shapers fall back inside lookups_for.
        script_tags.dedup();
        let mut feats: Vec<FeatureReq> = Vec::new();
        let mut stage = 0usize;
        let mut pauses: Vec<Pause> = Vec::new();
        let add = |feats: &mut Vec<FeatureReq>, stage: usize, tag: &[u8; 4], flags: u8| {
            feats.push(FeatureReq { tag: *tag, flags, stage });
        };
        macro_rules! pause {
            ($p:expr) => {{
                pauses.push($p);
                stage += 1;
            }};
        }
        add(&mut feats, stage, b"rvrn", F_GLOBAL);
        pause!(Pause::None);
        if rtl {
            add(&mut feats, stage, b"rtla", F_GLOBAL);
            add(&mut feats, stage, b"rtlm", 0);
        } else {
            add(&mut feats, stage, b"ltra", F_GLOBAL);
            add(&mut feats, stage, b"ltrm", F_GLOBAL);
        }
        match shaper {
            Shaper::Arabic => {
                add(&mut feats, stage, b"stch", F_GLOBAL);
                pause!(Pause::None);
                add(&mut feats, stage, b"ccmp", F_GLOBAL | F_MANUAL_ZWJ);
                add(&mut feats, stage, b"locl", F_GLOBAL | F_MANUAL_ZWJ);
                pause!(Pause::None);
                for t in [b"isol", b"fina", b"fin2", b"fin3", b"medi", b"med2", b"init"] {
                    add(&mut feats, stage, t, F_MANUAL_ZWJ);
                    pause!(Pause::None);
                }
                pause!(Pause::None);
                add(&mut feats, stage, b"rlig", F_GLOBAL | F_MANUAL_ZWJ);
                pause!(Pause::None);
                add(&mut feats, stage, b"calt", F_GLOBAL | F_MANUAL_ZWJ);
                if font.gsub.is_none_or(|g| g.0.feature_lookups(&script_tags, *b"rclt").is_none()) {
                    pause!(Pause::None);
                }
                add(&mut feats, stage, b"liga", F_GLOBAL | F_MANUAL_ZWJ);
                add(&mut feats, stage, b"clig", F_GLOBAL | F_MANUAL_ZWJ);
                add(&mut feats, stage, b"mset", F_GLOBAL | F_MANUAL_ZWJ);
            }
            Shaper::Indic => {
                pause!(Pause::None);
                add(&mut feats, stage, b"locl", F_GLOBAL | F_PER_SYLLABLE);
                add(&mut feats, stage, b"ccmp", F_GLOBAL | F_PER_SYLLABLE);
                pause!(Pause::IndicInitial);
                for (t, g) in [
                    (b"nukt", true),
                    (b"akhn", true),
                    (b"rphf", false),
                    (b"rkrf", true),
                    (b"pref", false),
                    (b"blwf", false),
                    (b"abvf", false),
                    (b"half", false),
                    (b"pstf", false),
                    (b"vatu", true),
                    (b"cjct", true),
                ] {
                    add(&mut feats, stage, t, F_MANUAL_JOINERS | F_PER_SYLLABLE | if g { F_GLOBAL } else { 0 });
                    pause!(Pause::None);
                }
                pause!(Pause::IndicFinal);
                add(&mut feats, stage, b"init", F_MANUAL_JOINERS | F_PER_SYLLABLE);
                for t in [b"pres", b"abvs", b"blws", b"psts", b"haln"] {
                    add(&mut feats, stage, t, F_GLOBAL | F_MANUAL_JOINERS | F_PER_SYLLABLE);
                }
            }
            _ => {}
        }
        for t in [b"abvm", b"blwm", b"ccmp", b"locl", b"rlig"] {
            add(&mut feats, stage, t, F_GLOBAL);
        }
        for t in [b"mark", b"mkmk"] {
            add(&mut feats, stage, t, F_GLOBAL | F_MANUAL_JOINERS);
        }
        for t in [b"calt", b"clig", b"curs", b"dist", b"kern", b"liga", b"rclt"] {
            add(&mut feats, stage, t, F_GLOBAL);
        }
        // Disabled features: Indic turns `liga` off; options turn off ligatures / kerning.
        let mut disabled: Vec<Tag> = Vec::new();
        if shaper == Shaper::Indic {
            disabled.push(*b"liga");
        }
        if !opts.ligatures {
            disabled.extend([*b"liga", *b"clig"]);
        }
        if !opts.kerning {
            disabled.push(*b"kern");
        }
        feats.retain(|f| !disabled.contains(&f.tag));
        // Merge duplicate tags: earliest stage, union of globality, union of flags.
        let mut merged: Vec<FeatureReq> = Vec::new();
        for f in feats {
            if let Some(m) = merged.iter_mut().find(|m| m.tag == f.tag) {
                m.stage = m.stage.min(f.stage);
                m.flags |= f.flags;
            } else {
                merged.push(f);
            }
        }
        let nstages = stage + 1;
        // Masks: global features share bit 0; each masked feature gets its own bit.
        let mut masks: Vec<(Tag, u32)> = Vec::new();
        let mut bit = 1u32;
        for f in &merged {
            let m = if f.flags & F_GLOBAL != 0 {
                GLOBAL_MASK
            } else {
                let m = 1u32 << bit;
                bit += 1;
                m
            };
            masks.push((f.tag, m));
        }
        let mut gsub: Vec<Vec<LookupReq>> = (0..nstages).map(|_| Vec::new()).collect();
        let mut gpos: Vec<LookupReq> = Vec::new();
        let collect = |t: &LayoutTable, out: &mut Vec<LookupReq>, f: &FeatureReq, mask: u32| {
            if let Some(ls) = t.feature_lookups(&script_tags, f.tag) {
                for li in ls {
                    out.push(LookupReq {
                        index: li,
                        mask,
                        auto_zwj: f.flags & F_MANUAL_ZWJ == 0,
                        auto_zwnj: f.flags & F_MANUAL_ZWNJ == 0,
                        per_syllable: f.flags & F_PER_SYLLABLE != 0,
                    });
                }
            }
        };
        for (f, &(_, mask)) in merged.iter().zip(masks.iter()) {
            if let Some(g) = font.gsub {
                collect(&g.0, &mut gsub[f.stage], f, mask);
            }
            if let Some(g) = font.gpos {
                collect(&g.0, &mut gpos, f, mask);
            }
        }
        let finish = |v: &mut Vec<LookupReq>| {
            v.sort_by_key(|l| l.index);
            let mut out: Vec<LookupReq> = Vec::new();
            for l in v.drain(..) {
                if let Some(last) = out.last_mut() {
                    if last.index == l.index {
                        last.mask |= l.mask;
                        last.auto_zwj &= l.auto_zwj;
                        last.auto_zwnj &= l.auto_zwnj;
                        last.per_syllable |= l.per_syllable;
                        continue;
                    }
                }
                out.push(l);
            }
            *v = out;
        };
        for s in gsub.iter_mut() {
            finish(s);
        }
        finish(&mut gpos);
        pauses.push(Pause::None);
        let has_gpos_kern = opts.kerning
            && font.gpos.is_some_and(|g| g.0.feature_lookups(&script_tags, *b"kern").is_some());
        let apply_kern_table = opts.kerning && !has_gpos_kern && font.kern.is_some() && font.gpos.is_none_or(|_| true);
        Plan { script_tags, shaper, gsub, pauses, gpos, masks, apply_kern_table }
    }
}

// ---------------------------------------------------------------------------------------------------------- run

/// The glyph for `cp` (0 when the font has none).
fn nominal(font: &Font, cp: u32) -> u16 {
    char::from_u32(cp).map_or(0, |c| font.glyph_index(c))
}

fn has_glyph(font: &Font, cp: u32) -> bool {
    nominal(font, cp) != 0
}

/// HarfBuzz's modified combining classes (traditional mark orders for Hebrew, Arabic, Thai, Lao, Tibetan).
pub(crate) fn modified_ccc(c: char) -> u8 {
    let ccc = normalize::ccc(c);
    match ccc {
        10 => 22,
        11 => 15,
        12 => 16,
        13 => 17,
        14 => 23,
        15 => 18,
        16 => 19,
        17 => 20,
        18 => 21,
        19 => 14,
        20 => 24,
        21 => 12,
        22 => 25,
        23 => 13,
        24 => 10,
        25 => 11,
        26 => 26,
        27 => 28,
        28 => 29,
        29 => 30,
        30 => 31,
        31 => 32,
        32 => 33,
        33 => 27,
        34 => 34,
        35 => 35,
        36 => 36,
        84 => 4,
        91 => 5,
        103 => 3,
        130 => 132,
        132 => 131,
        x => x,
    }
}

fn ignorable_kind(c: char) -> u8 {
    if c == '\u{200C}' {
        IGN_ZWNJ
    } else if c == '\u{200D}' {
        IGN_ZWJ
    } else if ucd::is_default_ignorable(c) {
        if matches!(c as u32, 0x180B..=0x180D | 0x180F | 0xE0020..=0xE007F | 0x034F) {
            ot::IGN_HIDDEN
        } else {
            IGN_OTHER
        }
    } else {
        IGN_NONE
    }
}

/// Decompose `cp` as far as the font needs (HarfBuzz's `decompose`): returns the parts when it decomposed.
fn decompose_for_font(font: &Font, cp: u32, shortest: bool, out: &mut Vec<u32>) -> bool {
    let Some(c) = char::from_u32(cp) else { return false };
    let parts: Vec<u32> = if (0xAC00..0xAC00 + 11172).contains(&cp) {
        normalize::nfd_chars(c.encode_utf8(&mut [0; 4])).iter().map(|&x| x as u32).collect()
    } else {
        match normalize::decomposition(c) {
            Some(d) => d.to_vec(),
            None => return false,
        }
    };
    let (a, rest) = (parts[0], &parts[1..]);
    if rest.iter().any(|&b| !has_glyph(font, b)) {
        return false;
    }
    let has_a = has_glyph(font, a);
    if shortest && has_a {
        out.push(a);
        out.extend_from_slice(rest);
        return true;
    }
    let mut tmp = Vec::new();
    if decompose_for_font(font, a, shortest, &mut tmp) {
        out.extend(tmp);
        out.extend_from_slice(rest);
        return true;
    }
    if has_a {
        out.push(a);
        out.extend_from_slice(rest);
        return true;
    }
    false
}

/// Font-aware normalization (HarfBuzz's three rounds) over (char, cluster) pairs.
fn normalize_run(font: &Font, chars: &mut Vec<(u32, usize)>, short_circuit: bool, shaper: Shaper) {
    let _ = shaper;
    // Round 1: decompose.
    let mut out: Vec<(u32, usize)> = Vec::with_capacity(chars.len());
    for &(cp, cl) in chars.iter() {
        if short_circuit && has_glyph(font, cp) {
            out.push((cp, cl));
            continue;
        }
        let mut parts = Vec::new();
        if decompose_for_font(font, cp, short_circuit, &mut parts) {
            out.extend(parts.into_iter().map(|p| (p, cl)));
        } else {
            out.push((cp, cl));
        }
    }
    // Round 2: reorder marks by modified combining class, then the shaper's mark tweaks.
    let mut mcc: Vec<u8> = out.iter().map(|x| char::from_u32(x.0).map_or(0, modified_ccc)).collect();
    let mut i = 0;
    while i < out.len() {
        if mcc[i] == 0 {
            i += 1;
            continue;
        }
        let s = i;
        while i < out.len() && mcc[i] != 0 {
            i += 1;
        }
        if i - s > 1 && i - s <= 32 {
            let mut seg: Vec<((u32, usize), u8)> = out[s..i].iter().copied().zip(mcc[s..i].iter().copied()).collect();
            seg.sort_by_key(|x| x.1);
            for (k, (o, m)) in seg.into_iter().enumerate() {
                out[s + k] = o;
                mcc[s + k] = m;
            }
            match shaper {
                Shaper::Hebrew => complex::reorder_marks_hebrew(&mut out, &mcc, s, i),
                Shaper::Arabic => complex::reorder_marks_arabic(&mut out, &mut mcc, s, i),
                _ => {}
            }
        }
    }
    let cc_at = |mcc: &[u8], k: usize| mcc[k];
    // Round 3: recompose marks onto their starter when the font has the composite.
    if out.len() > 1 {
        let mut res: Vec<(u32, usize)> = Vec::with_capacity(out.len());
        let mut rcc: Vec<u8> = Vec::with_capacity(out.len());
        let mut starter: Option<usize> = None;
        for (k, &(cp, cl)) in out.iter().enumerate() {
            let c = char::from_u32(cp).unwrap_or('\0');
            let cc = cc_at(&mcc, k);
            if ucd::is_mark(c) {
                if let Some(st) = starter {
                    let last = res.len() - 1;
                    let unblocked = st == last || rcc[last] < cc;
                    let composed = if unblocked {
                        char::from_u32(res[st].0)
                            .and_then(|a| normalize::compose_pair(a, c))
                            .filter(|&p| has_glyph(font, p as u32))
                    } else {
                        None
                    };
                    if let Some(p) = composed {
                        res[st].0 = p as u32;
                        res[st].1 = res[st].1.min(cl);
                        rcc[st] = modified_ccc(p);
                        continue;
                    }
                    if st < last && rcc[last] > cc {
                        starter = None;
                    }
                }
            }
            res.push((cp, cl));
            rcc.push(cc);
            if cc == 0 {
                starter = Some(res.len() - 1);
            }
        }
        out = res;
    }
    *chars = out;
}

/// Shape one run (single script, single direction). `start..end` is a byte range of `text`; clusters are byte
/// offsets into `text`. Returns glyphs in visual order.
pub fn shape_run(font: &Font, text: &str, start: usize, end: usize, script: Script, rtl: bool, opts: &ShapeOptions) -> Vec<GlyphPos> {
    let plan = Plan::new(font, script, rtl, opts);
    let mut chars: Vec<(u32, usize)> = text[start..end].char_indices().map(|(i, c)| (c as u32, start + i)).collect();
    if chars.is_empty() {
        return Vec::new();
    }
    // Clusters: a combining mark continues its base's cluster (grapheme-level clusters).
    for k in 1..chars.len() {
        if chars[k].0 == 0x200D || char::from_u32(chars[k].0).is_some_and(ucd::is_mark) {
            chars[k].1 = chars[k - 1].1;
        }
    }
    if plan.shaper == Shaper::Thai {
        complex::thai_preprocess(&mut chars);
    }
    complex::vowel_constraints(&mut chars);
    normalize_run(font, &mut chars, plan.shaper != Shaper::Indic, plan.shaper);
    let mut info: Vec<GlyphInfo> = chars
        .iter()
        .map(|&(cp, cl)| GlyphInfo {
            cp,
            cluster: cl,
            mask: GLOBAL_MASK,
            ignorable: char::from_u32(cp).map_or(IGN_NONE, ignorable_kind),
            ..Default::default()
        })
        .collect();
    match plan.shaper {
        Shaper::Arabic => complex::arabic_setup_masks(&plan, &mut info),
        Shaper::Indic => complex::indic_setup(font, &plan, &mut info),
        _ => {}
    }
    // Mirroring (L4) for RTL runs, else the `rtlm` mask.
    if rtl {
        let rtlm = plan.mask(*b"rtlm");
        for g in info.iter_mut() {
            match char::from_u32(g.cp).and_then(crate::bidi::mirrored) {
                Some(m) if has_glyph(font, m as u32) => g.cp = m as u32,
                _ => g.mask |= rtlm,
            }
        }
    }
    for g in info.iter_mut() {
        g.glyph = nominal(font, g.cp);
        let c = char::from_u32(g.cp).unwrap_or('\0');
        let synth_mark = ucd::general_category(c) == Gc::Mn && g.ignorable == IGN_NONE;
        g.props = ot::glyph_props(font, g.glyph, synth_mark);
    }
    let mut pos: Vec<GlyphPosition> = alloc::vec![GlyphPosition::default(); info.len()];
    // GSUB.
    let mut lig_id = 1u8;
    for (s, stage) in plan.gsub.iter().enumerate() {
        if let Some(g) = font.gsub {
            let mut a = Apply::new(font, g.0, false, &mut info, &mut pos, rtl, lig_id);
            for l in stage {
                a.apply_lookup(l);
            }
            lig_id = a.next_lig_id();
        }
        match plan.pauses.get(s).copied().unwrap_or(Pause::None) {
            Pause::IndicInitial => complex::indic_initial_reordering(font, &plan, &mut info),
            Pause::IndicFinal => complex::indic_final_reordering(font, &plan, &mut info),
            Pause::None => {}
        }
    }
    pos.resize(info.len(), GlyphPosition::default());
    // GPOS.
    for (g, p) in info.iter().zip(pos.iter_mut()) {
        *p = GlyphPosition { x_advance: font.advance(g.glyph) as i32, ..Default::default() };
    }
    let zero_marks = plan.shaper != Shaper::Indic;
    if let Some(gp) = font.gpos {
        let mut a = Apply::new(font, gp.0, true, &mut info, &mut pos, rtl, lig_id);
        for l in &plan.gpos {
            a.apply_lookup(l);
        }
    }
    if plan.apply_kern_table {
        if let Some(k) = font.kern {
            // Legacy `kern`: adjacent non-mark pairs.
            let idx: Vec<usize> = (0..info.len()).filter(|&i| !info[i].is_mark()).collect();
            for w in idx.windows(2) {
                pos[w[0]].x_advance += k.pair(info[w[0]].glyph, info[w[1]].glyph) as i32;
            }
        }
    }
    if zero_marks {
        let adjust = font.gpos.is_none() && !rtl;
        for (g, p) in info.iter().zip(pos.iter_mut()) {
            if g.is_mark() {
                if adjust {
                    p.x_offset -= p.x_advance;
                }
                p.x_advance = 0;
                p.y_advance = 0;
            }
        }
    }
    // Default ignorables: zero width, invisible (the space glyph), as engines hide them.
    let space = font.glyph_index(' ');
    for (g, p) in info.iter_mut().zip(pos.iter_mut()) {
        if g.ignorable != IGN_NONE && g.subst & ot::ST_SUBSTITUTED == 0 {
            p.x_advance = 0;
            p.x_offset = 0;
            p.y_offset = 0;
            g.glyph = space;
        }
    }
    ot::propagate_attachments(&mut pos, rtl);
    let mut out: Vec<GlyphPos> = info
        .iter()
        .zip(pos.iter())
        .map(|(g, p)| GlyphPos { glyph: g.glyph, cluster: g.cluster, x_advance: p.x_advance, x_offset: p.x_offset, y_offset: p.y_offset })
        .collect();
    if rtl {
        out.reverse();
    }
    out
}

/// Shape `text` with an explicit paragraph direction: bidi runs in visual order, each split into script runs.
pub fn shape_dir(font: &Font, text: &str, opts: &ShapeOptions, dir: Direction) -> Vec<GlyphPos> {
    let mut out = Vec::new();
    if text.is_empty() {
        return out;
    }
    let bidi = BidiInfo::new(text, dir);
    for &(ps, pe, _) in &bidi.paragraphs {
        for (rs, re, level) in bidi.visual_runs(ps, pe) {
            let (bs, be) = (bidi.offsets[rs], bidi.offsets[re]);
            let rtl = level & 1 == 1;
            let mut runs = script::itemize(&text[bs..be]);
            if rtl {
                runs.reverse();
            }
            for (s, e, sc) in runs {
                out.extend(shape_run(font, text, bs + s, bs + e, sc, rtl, opts));
            }
        }
    }
    out
}

/// Shape `text` into positioned glyphs (font units), visual order, paragraph direction by UAX #9 P2/P3.
pub fn shape(font: &Font, text: &str, opts: &ShapeOptions) -> Vec<GlyphPos> {
    shape_dir(font, text, opts, Direction::Auto)
}

/// Advance width of `text` at `size` px (fractional, unhinted — what Chromium's measureText reports with
/// subpixel positioning).
pub fn measure(font: &Font, text: &str, size: f32, opts: &ShapeOptions) -> f32 {
    let units: i64 = shape(font, text, opts).iter().map(|g| g.x_advance as i64).sum();
    units as f32 * size / font.units_per_em as f32
}

/// One laid-out line: byte range into the source and its width in px (trailing spaces excluded).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    pub width: f32,
}

fn trim_end_width(font: &Font, text: &str, size: f32, opts: &ShapeOptions) -> f32 {
    measure(font, text.trim_end_matches([' ', '\t', '\n', '\r', '\u{2028}', '\u{2029}', '\u{0085}', '\u{000B}', '\u{000C}']), size, opts)
}

/// Greedy line layout on UAX #14 opportunities: each line takes as many break-delimited segments as fit
/// `max_width`; a mandatory break ends a line; a single segment wider than the line overflows on its own.
pub fn layout_lines(font: &Font, text: &str, size: f32, max_width: f32, opts: &ShapeOptions) -> Vec<Line> {
    let b = breaks(text);
    let offs: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(core::iter::once(text.len())).collect();
    let mut lines = Vec::new();
    let mut start = 0usize; // byte
    let mut last_fit: Option<usize> = None; // byte offset of the last opportunity that fits
    for (ci, &br) in b.iter().enumerate().skip(1) {
        if br == Break::None {
            continue;
        }
        let pos = offs[ci];
        let w = trim_end_width(font, &text[start..pos], size, opts);
        if w <= max_width || last_fit.is_none() {
            last_fit = Some(pos);
            if br == Break::Mandatory {
                lines.push(Line { start, end: pos, width: w });
                start = pos;
                last_fit = None;
            }
            continue;
        }
        // Overflow: end the line at the last fitting opportunity and retry this one on the new line.
        let end = last_fit.unwrap();
        lines.push(Line { start, end, width: trim_end_width(font, &text[start..end], size, opts) });
        start = end;
        let w2 = trim_end_width(font, &text[start..pos], size, opts);
        last_fit = Some(pos);
        if br == Break::Mandatory {
            lines.push(Line { start, end: pos, width: w2 });
            start = pos;
            last_fit = None;
        }
    }
    if start < text.len() && lines.last().is_none_or(|l: &Line| l.end < text.len()) {
        lines.push(Line { start, end: text.len(), width: trim_end_width(font, &text[start..], size, opts) });
    }
    lines
}

/// An 8-bit coverage canvas (row-major, `width * height`).
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Canvas { width, height, data: alloc::vec![0; width * height] }
    }
}

/// Draw shaped `text` with its baseline origin at (`x`, `y`) px (y down) into `canvas`, compositing glyph
/// coverage with source-over (`1 - (1-a)(1-b)`). Glyph origins snap to the cache's subpixel grid in x and to
/// whole pixels in y (as Skia does for horizontal text). Returns the pen advance in px.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    cache: &mut GlyphCache,
    font_id: u32,
    font: &Font,
    text: &str,
    size: f32,
    x: f32,
    y: f32,
    canvas: &mut Canvas,
    opts: &ShapeOptions,
) -> f32 {
    let scale = size / font.units_per_em as f32;
    let mut pen = x;
    for g in shape(font, text, opts) {
        let gx = pen + g.x_offset as f32 * scale;
        let gy = y - g.y_offset as f32 * scale;
        let (ix, sx) = split_position(gx);
        let iy = crate::fmath::round(gy) as i32;
        if let Some(bm) = cache.get(font_id, font, g.glyph, size, sx, 0) {
            for row in 0..bm.height as i32 {
                let cy = iy + bm.top + row;
                if cy < 0 || cy >= canvas.height as i32 {
                    continue;
                }
                for col in 0..bm.width as i32 {
                    let cx = ix + bm.left + col;
                    if cx < 0 || cx >= canvas.width as i32 {
                        continue;
                    }
                    let a = bm.data[(row * bm.width as i32 + col) as usize] as u32;
                    let d = &mut canvas.data[cy as usize * canvas.width + cx as usize];
                    let b = *d as u32;
                    *d = (a + b - (a * b + 127) / 255).min(255) as u8;
                }
            }
        }
        pen += g.x_advance as f32 * scale;
    }
    pen - x
}
