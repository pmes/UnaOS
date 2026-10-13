//! Intl.Segmenter (ECMA-402 §18): grapheme, word and sentence segmentation after UAX #29 over the UCD
//! tables already in `unicode/` — extended grapheme clusters (GB3–GB13 with Grapheme_Extend, SpacingMark,
//! ZWJ + Extended_Pictographic, regional-indicator pairs, Hangul L/V/T), word boundaries (WB5–WB13b: letters,
//! digits, MidLetter / MidNum / MidNumLet between them, Katakana runs, whitespace runs, ideographs singly),
//! sentence boundaries (SB8–SB11, simplified: a terminator plus closers and spaces ends a sentence unless a
//! lowercase letter follows a full stop).

use super::*;
use crate::string::code_point_at;
use crate::unicode;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gran {
    Grapheme,
    Word,
    Sentence,
}

#[derive(Clone, Debug)]
pub struct Seg {
    pub locale: String,
    pub gran: Gran,
}

fn has(prop: &str, cp: u32) -> bool {
    match unicode::binary_property(prop) {
        Some(t) => unicode::in_table(t, cp),
        None => false,
    }
}

fn cp_at(s: &[u16], i: usize) -> (u32, usize) {
    code_point_at(s, i)
}

fn is_ri(cp: u32) -> bool {
    (0x1F1E6..=0x1F1FF).contains(&cp)
}

fn hangul(cp: u32) -> u8 {
    match cp {
        0x1100..=0x115F | 0xA960..=0xA97C => b'L',
        0x1160..=0x11A7 | 0xD7B0..=0xD7C6 => b'V',
        0x11A8..=0x11FF | 0xD7CB..=0xD7FB => b'T',
        0xAC00..=0xD7A3 => {
            if (cp - 0xAC00) % 28 == 0 {
                b'v' // LV
            } else {
                b't' // LVT
            }
        }
        _ => 0,
    }
}

fn is_extend(cp: u32) -> bool {
    cp == 0x200D || has("Grapheme_Extend", cp) || unicode::general_category(cp) == 10 || (0x1F3FB..=0x1F3FF).contains(&cp)
}

/// The end of the grapheme cluster starting at `i`.
pub fn next_grapheme(s: &[u16], i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let (cp, n) = cp_at(s, i);
    let mut j = i + n;
    if cp == 0x0D {
        if j < s.len() && s[j] == 0x0A {
            return j + 1;
        }
        return j;
    }
    if cp == 0x0A || unicode::general_category(cp) == 0 {
        return j;
    }
    if is_ri(cp) {
        if j < s.len() {
            let (c2, n2) = cp_at(s, j);
            if is_ri(c2) {
                j += n2;
            }
        }
    } else {
        let h = hangul(cp);
        if h != 0 {
            let mut state = h;
            while j < s.len() {
                let (c2, n2) = cp_at(s, j);
                let h2 = hangul(c2);
                let ok = match (state, h2) {
                    (b'L', b'L' | b'V' | b'v' | b't') => true,
                    (b'V' | b'v', b'V' | b'T') => true,
                    (b'T' | b't', b'T') => true,
                    _ => false,
                };
                if !ok {
                    break;
                }
                state = h2;
                j += n2;
            }
        }
    }
    // GB11: Extended_Pictographic Extend* ZWJ × Extended_Pictographic.
    let pict = has("Extended_Pictographic", cp);
    while j < s.len() {
        let (c2, n2) = cp_at(s, j);
        if !is_extend(c2) {
            break;
        }
        if c2 == 0x200D && pict && j + n2 < s.len() {
            let (c3, n3) = cp_at(s, j + n2);
            if has("Extended_Pictographic", c3) {
                j += n2 + n3;
                continue;
            }
        }
        j += n2;
    }
    j
}

#[derive(PartialEq, Clone, Copy)]
enum W {
    Letter,
    Digit,
    Kana,
    Ideo,
    MidLetter,
    MidNum,
    MidNumLet,
    Space,
    Newline,
    Other,
}

fn wclass(cp: u32) -> W {
    match cp {
        0x0A | 0x0B | 0x0C | 0x0D | 0x85 | 0x2028 | 0x2029 => return W::Newline,
        0x27 | 0x2E | 0x2018 | 0x2019 | 0x2024 | 0xFE52 | 0xFF07 | 0xFF0E => return W::MidNumLet,
        0x3A | 0xB7 | 0x387 | 0x5F4 | 0x2027 | 0xFE13 | 0xFE55 | 0xFF1A => return W::MidLetter,
        0x2C | 0x3B | 0x37E | 0x589 | 0x60C | 0x60D | 0x66C | 0x7F8 | 0x2044 | 0xFE10 | 0xFE14 | 0xFE50 | 0xFE54 | 0xFF0C | 0xFF1B => {
            return W::MidNum
        }
        0x5F => return W::Letter, // ExtendNumLet joins words
        _ => {}
    }
    if (0x30A0..=0x30FF).contains(&cp) || (0x31F0..=0x31FF).contains(&cp) || (0xFF66..=0xFF9F).contains(&cp) {
        return W::Kana;
    }
    let gc = unicode::general_category(cp);
    match gc {
        13 => W::Digit,
        5..=9 | 14 => {
            if (0x3040..=0x309F).contains(&cp) || (0x3400..=0x9FFF).contains(&cp) || (0x20000..=0x3FFFF).contains(&cp) || (0xF900..=0xFAFF).contains(&cp) {
                W::Ideo
            } else {
                W::Letter
            }
        }
        10..=12 => W::Letter,
        29 => W::Space,
        _ => {
            if cp == 0x09 || cp == 0x20 {
                W::Space
            } else {
                W::Other
            }
        }
    }
}

/// The end of the word segment starting at `i` and whether it is word-like.
pub fn next_word(s: &[u16], i: usize) -> (usize, bool) {
    if i >= s.len() {
        return (s.len(), false);
    }
    let first_end = next_grapheme(s, i);
    let (cp, _) = cp_at(s, i);
    let c = wclass(cp);
    match c {
        W::Space => {
            let mut j = first_end;
            while j < s.len() && wclass(cp_at(s, j).0) == W::Space {
                j = next_grapheme(s, j);
            }
            (j, false)
        }
        W::Kana => {
            let mut j = first_end;
            while j < s.len() && wclass(cp_at(s, j).0) == W::Kana {
                j = next_grapheme(s, j);
            }
            (j, true)
        }
        W::Ideo => (first_end, true),
        W::Letter | W::Digit => {
            let mut j = first_end;
            let mut last = c;
            while j < s.len() {
                let k = wclass(cp_at(s, j).0);
                match k {
                    W::Letter | W::Digit => {
                        last = k;
                        j = next_grapheme(s, j);
                    }
                    W::MidLetter | W::MidNum | W::MidNumLet => {
                        let after = next_grapheme(s, j);
                        if after >= s.len() {
                            break;
                        }
                        let k2 = wclass(cp_at(s, after).0);
                        let joins = match k {
                            W::MidLetter => last == W::Letter && k2 == W::Letter,
                            W::MidNum => last == W::Digit && k2 == W::Digit,
                            _ => last == k2 && matches!(k2, W::Letter | W::Digit),
                        };
                        if !joins {
                            break;
                        }
                        j = after;
                    }
                    _ => break,
                }
            }
            (j, true)
        }
        _ => (first_end, false),
    }
}

fn is_sterm(cp: u32) -> bool {
    matches!(cp, 0x21 | 0x3F | 0x589 | 0x61F | 0x6D4 | 0x700..=0x702 | 0x964 | 0x965 | 0x203C | 0x203D | 0x2047..=0x2049 | 0x3002 | 0xFE56 | 0xFE57 | 0xFF01 | 0xFF1F | 0xFF61)
}

/// The end of the sentence segment starting at `i`.
pub fn next_sentence(s: &[u16], i: usize) -> usize {
    let mut j = i;
    while j < s.len() {
        let (cp, n) = cp_at(s, j);
        if matches!(cp, 0x0A | 0x0D | 0x85 | 0x2029) {
            j += n;
            if cp == 0x0D && j < s.len() && s[j] == 0x0A {
                j += 1;
            }
            return j;
        }
        if cp == 0x2E || is_sterm(cp) {
            let mut k = j + n;
            while k < s.len() {
                let (c2, n2) = cp_at(s, k);
                if c2 == 0x2E || is_sterm(c2) || matches!(unicode::general_category(c2), 18 | 19 | 20) || c2 == 0x22 || c2 == 0x27 || c2 == 0x29 || c2 == 0x5D {
                    k += n2;
                } else {
                    break;
                }
            }
            let mut sp = k;
            while sp < s.len() {
                let (c2, n2) = cp_at(s, sp);
                if unicode::general_category(c2) == 29 || c2 == 0x20 || c2 == 0x09 {
                    sp += n2;
                } else if matches!(c2, 0x0A | 0x0D | 0x85 | 0x2029) {
                    sp += n2;
                    if c2 == 0x0D && sp < s.len() && s[sp] == 0x0A {
                        sp += 1;
                    }
                    return sp;
                } else {
                    break;
                }
            }
            if sp >= s.len() {
                return s.len();
            }
            let (next, _) = cp_at(s, sp);
            let lower_follows = unicode::general_category(next) == 5 || unicode::general_category(next) == 13;
            if is_sterm(cp) || (!lower_follows && sp > k) {
                return sp;
            }
            j = k;
            continue;
        }
        j += n;
    }
    s.len()
}

pub fn next_segment(seg: &Seg, s: &[u16], i: usize) -> (usize, bool) {
    match seg.gran {
        Gran::Grapheme => (next_grapheme(s, i), false),
        Gran::Word => next_word(s, i),
        Gran::Sentence => (next_sentence(s, i), false),
    }
}

// ============================================================================================ JS surface

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "Segmenter", 0, seg_ctor);
    method(vm, p, "segment", 1, seg_segment);
    method(vm, p, "resolvedOptions", 0, seg_resolved_options);
    let op = vm.intr().object_proto;
    let segs = vm.new_object(Some(op));
    method(vm, segs, "containing", 1, segs_containing);
    let isym = vm.wk.iterator.clone();
    method_sym(vm, segs, isym, "[Symbol.iterator]", 0, segs_iterator, WC);
    let ip = vm.intr().iterator_proto;
    let sit = vm.new_object(Some(ip));
    method(vm, sit, "next", 0, seg_iter_next);
    to_str_tag(vm, sit, "Segmenter String Iterator");
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_segmenter_proto = p;
    vm.realms[r].intrinsics.intl_segments_proto = segs;
    vm.realms[r].intrinsics.intl_segment_iterator_proto = sit;
}

fn seg_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_new(vm, ctx, "Segmenter")?;
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.intl_segmenter_proto)?;
    let l = vm.arg(ctx, 0);
    let requested = canonicalize_locale_list(vm, &l)?;
    let ov = vm.arg(ctx, 1);
    let o = options_object(vm, &ov)?;
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], "Intl.Segmenter")?;
    let r = resolve_locale(&requested, &[]);
    let g = get_string_option(vm, o, "granularity", &["grapheme", "word", "sentence"], "Intl.Segmenter")?;
    let seg = Seg {
        locale: String::from(r.data_locale),
        gran: match g.as_deref() {
            Some("word") => Gran::Word,
            Some("sentence") => Gran::Sentence,
            _ => Gran::Grapheme,
        },
    };
    Ok(Value::Object(alloc_intl(vm, p, IntlData::Segmenter(seg))))
}

fn seg_segment(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::Segmenter(_)), "Intl.Segmenter.prototype.segment")?;
    let sv = vm.arg(ctx, 0);
    let s = vm.to_string(&sv)?;
    let seg = match intl_ref(vm, o) {
        IntlData::Segmenter(s) => s.clone(),
        _ => unreachable!(),
    };
    let p = vm.intr().intl_segments_proto;
    Ok(Value::Object(alloc_intl(vm, p, IntlData::Segments { seg, s })))
}

fn seg_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::Segmenter(_)), "Intl.Segmenter.prototype.resolvedOptions")?;
    let seg = match intl_ref(vm, o) {
        IntlData::Segmenter(s) => s.clone(),
        _ => unreachable!(),
    };
    let g = match seg.gran {
        Gran::Grapheme => "grapheme",
        Gran::Word => "word",
        Gran::Sentence => "sentence",
    };
    let r = object_from(vm, vec![("locale", Value::str(&seg.locale)), ("granularity", Value::str(g))]);
    Ok(Value::Object(r))
}

/// CreateSegmentDataObject (§18.7.1).
fn segment_data(vm: &mut Vm, seg: &Seg, s: &JsStr, start: usize, end: usize, word_like: bool) -> Value {
    let mut props: Vec<(&str, Value)> = vec![
        ("segment", Value::String(s.slice(start, end))),
        ("index", Value::Number(start as f64)),
        ("input", Value::String(s.clone())),
    ];
    if seg.gran == Gran::Word {
        props.push(("isWordLike", Value::Bool(word_like)));
    }
    Value::Object(object_from(vm, props))
}

fn segs_containing(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::Segments { .. }), "%Segments.prototype%.containing")?;
    let (seg, s) = match intl_ref(vm, o) {
        IntlData::Segments { seg, s } => (seg.clone(), s.clone()),
        _ => unreachable!(),
    };
    let iv = vm.arg(ctx, 0);
    let n = vm.to_integer_or_infinity(&iv)?;
    if n < 0.0 || n >= s.len() as f64 {
        return Ok(Value::Undefined);
    }
    let n = n as usize;
    let u = s.units();
    let mut start = 0;
    loop {
        vm.tick()?;
        let (end, wl) = next_segment(&seg, u, start);
        if n < end || end >= u.len() {
            return Ok(segment_data(vm, &seg, &s, start, end.max(start + 1).min(u.len()), wl));
        }
        start = end;
    }
}

fn segs_iterator(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::Segments { .. }), "%Segments.prototype%[@@iterator]")?;
    let (seg, s) = match intl_ref(vm, o) {
        IntlData::Segments { seg, s } => (seg.clone(), s.clone()),
        _ => unreachable!(),
    };
    let p = vm.intr().intl_segment_iterator_proto;
    Ok(Value::Object(alloc_intl(vm, p, IntlData::SegmentIterator { seg, s, pos: 0 })))
}

fn seg_iter_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::SegmentIterator { .. }), "%SegmentIterator.prototype%.next")?;
    let (seg, s, pos) = match intl_ref(vm, o) {
        IntlData::SegmentIterator { seg, s, pos } => (seg.clone(), s.clone(), *pos),
        _ => unreachable!(),
    };
    if pos >= s.len() {
        return Ok(Value::Object(vm.iter_result(Value::Undefined, true)));
    }
    let (end, wl) = next_segment(&seg, s.units(), pos);
    let end = end.max(pos + 1).min(s.len());
    if let Kind::Intl(i) = &mut vm.heap.get_mut(o).kind {
        if let IntlData::SegmentIterator { pos, .. } = &mut i.data {
            *pos = end;
        }
    }
    let d = segment_data(vm, &seg, &s, pos, end, wl);
    Ok(Value::Object(vm.iter_result(d, false)))
}
