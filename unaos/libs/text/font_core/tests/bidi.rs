//! FONTBIDI M1: UAX #9 against Unicode's own conformance files, in full — BidiTest-17.0.0.txt (every class
//! sequence × every paragraph direction in its bitset: levels and visual order) and BidiCharacterTest-17.0.0.txt
//! (code points incl. paired brackets: paragraph level, levels, visual order). Fetched at test time (sha-pinned,
//! SKIP offline); `FONTBIDI_UCD_DIR` points at a local copy instead.

mod common;
use common::{vector_file, UCD_BASE};
use font_core::bidi::{self, apply_l1, bracket_of, removed_by_x9, reorder, resolve, BidiClass, BidiInfo, Direction};

fn class_from_name(s: &str) -> BidiClass {
    use BidiClass::*;
    match s {
        "L" => L, "R" => R, "AL" => AL, "EN" => EN, "ES" => ES, "ET" => ET, "AN" => AN, "CS" => CS, "NSM" => NSM,
        "BN" => BN, "B" => B, "S" => S, "WS" => WS, "ON" => ON, "LRE" => LRE, "LRO" => LRO, "RLE" => RLE,
        "RLO" => RLO, "PDF" => PDF, "LRI" => LRI, "RLI" => RLI, "FSI" => FSI, "PDI" => PDI,
        _ => panic!("class {s}"),
    }
}

/// Levels after L1 ('x' = removed) and the visual order of the kept characters.
fn run(classes: &[BidiClass], brackets: &[Option<bidi::Bracket>], dir: Option<u8>) -> (u8, Vec<Option<u8>>, Vec<usize>) {
    let r = resolve(classes, brackets, dir);
    let mut lv = r.levels.clone();
    apply_l1(classes, &mut lv, r.para_level, 0, classes.len());
    let kept: Vec<usize> = (0..classes.len()).filter(|&i| !removed_by_x9(classes[i])).collect();
    let klv: Vec<u8> = kept.iter().map(|&i| lv[i]).collect();
    let order = reorder(&klv).into_iter().map(|k| kept[k]).collect();
    let shown = (0..classes.len()).map(|i| if removed_by_x9(classes[i]) { None } else { Some(lv[i]) }).collect();
    (r.para_level, shown, order)
}

fn parse_levels(s: &str) -> Vec<Option<u8>> {
    s.split_whitespace().map(|t| if t == "x" { None } else { Some(t.parse().unwrap()) }).collect()
}
fn parse_order(s: &str) -> Vec<usize> {
    s.split_whitespace().map(|t| t.parse().unwrap()).collect()
}

#[test]
fn bidi_test_txt_in_full() {
    let Some(data) = vector_file("BidiTest.txt", &format!("{UCD_BASE}BidiTest.txt"),
        "888bdfc8090652272d1f859cdb00ae659e2dc6c26740be61ef1d03998a687620") else { return };
    let text = String::from_utf8(data).unwrap();
    let (mut levels, mut order) = (Vec::new(), Vec::new());
    let (mut pass, mut total) = (0usize, 0usize);
    let mut fails = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(v) = line.strip_prefix("@Levels:") {
            levels = parse_levels(v);
            continue;
        }
        if let Some(v) = line.strip_prefix("@Reorder:") {
            order = parse_order(v);
            continue;
        }
        if line.starts_with('@') {
            continue;
        }
        let (input, bits) = line.split_once(';').unwrap();
        let classes: Vec<BidiClass> = input.split_whitespace().map(class_from_name).collect();
        let bits = u8::from_str_radix(bits.trim(), 16).unwrap();
        let none = vec![None; classes.len()];
        for (bit, dir) in [(1u8, None), (2, Some(0u8)), (4, Some(1))] {
            if bits & bit == 0 {
                continue;
            }
            total += 1;
            let (_, lv, ord) = run(&classes, &none, dir);
            if lv == levels && ord == order {
                pass += 1;
            } else if fails.len() < 12 {
                fails.push(format!("{input} dir={dir:?}\n  want {levels:?} {order:?}\n  got  {lv:?} {ord:?}"));
            }
        }
    }
    eprintln!("BidiTest.txt: {pass}/{total} cases");
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(total > 400_000);
    assert_eq!(pass, total);
}

#[test]
fn bidi_character_test_txt_in_full() {
    let Some(data) = vector_file("BidiCharacterTest.txt", &format!("{UCD_BASE}BidiCharacterTest.txt"),
        "a3e6e905ab5afbe318a96df5401d0372a04cd73ef139ab5e3cf0ae241c255488") else { return };
    let text = String::from_utf8(data).unwrap();
    let (mut pass, mut total) = (0usize, 0usize);
    let mut fails = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split(';').collect();
        let chars: Vec<char> =
            f[0].split_whitespace().map(|h| char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap()).collect();
        let dir = match f[1].trim() {
            "0" => Some(0),
            "1" => Some(1),
            _ => None,
        };
        let want_para: u8 = f[2].trim().parse().unwrap();
        let want_lv = parse_levels(f[3]);
        let want_ord = parse_order(f[4]);
        let classes: Vec<BidiClass> = chars.iter().map(|&c| bidi::bidi_class(c)).collect();
        let brackets: Vec<_> = chars.iter().map(|&c| bracket_of(c)).collect();
        let (para, lv, ord) = run(&classes, &brackets, dir);
        total += 1;
        if para == want_para && lv == want_lv && ord == want_ord {
            pass += 1;
        } else if fails.len() < 12 {
            fails.push(format!("{line}\n  got {para} {lv:?} {ord:?}"));
        }
    }
    eprintln!("BidiCharacterTest.txt: {pass}/{total} lines");
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(total > 90_000);
    assert_eq!(pass, total);
}

#[test]
fn mirroring_and_mixed_paragraphs() {
    assert_eq!(bidi::mirrored('('), Some(')'));
    assert_eq!(bidi::mirrored('«'), Some('»'));
    assert_eq!(bidi::mirrored('a'), None);
    // "abc אבג def": the Hebrew run is reversed in place.
    let t = "abc \u{5D0}\u{5D1}\u{5D2} def";
    let b = BidiInfo::new(t, Direction::Auto);
    assert_eq!(b.paragraphs[0].2, 0);
    assert_eq!(b.visual_order(0, b.chars.len()), vec![0, 1, 2, 3, 6, 5, 4, 7, 8, 9, 10]);
    // An RTL paragraph with a number: digits keep their LTR order inside.
    let t = "\u{5D0} 123 \u{5D1}";
    let b = BidiInfo::new(t, Direction::Auto);
    assert_eq!(b.paragraphs[0].2, 1);
    assert_eq!(b.visual_order(0, b.chars.len()), vec![6, 5, 2, 3, 4, 1, 0]);
    // Two paragraphs, each with its own P2/P3 level.
    let b = BidiInfo::new("abc\u{2029}\u{5D0}\u{5D1}", Direction::Auto);
    assert_eq!(b.paragraphs.iter().map(|p| p.2).collect::<Vec<_>>(), vec![0, 1]);
}
