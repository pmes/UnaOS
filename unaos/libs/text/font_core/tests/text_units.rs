//! FONTBIDI M2: UAX #29 grapheme clusters on GraphemeBreakTest-17.0.0.txt in full, UAX #15 NFD/NFC on
//! NormalizationTest-17.0.0.txt in full (parts 0–3 plus the invariance of every code point outside part 1), and
//! known-answer runs for the script itemizer.

mod common;
use common::{vector_file, UCD_BASE};
use font_core::{grapheme, normalize, script};

#[test]
fn grapheme_break_test_in_full() {
    let Some(data) = vector_file("GraphemeBreakTest.txt", &format!("{UCD_BASE}auxiliary/GraphemeBreakTest.txt"),
        "e2d134d2c52919bace503ebb6a551c1855fe1a1faec18478c78fff254a1793ec") else { return };
    let text = String::from_utf8(data).unwrap();
    let (mut pass, mut total) = (0, 0);
    let mut fails = Vec::new();
    for line in text.lines() {
        let body = line.split('#').next().unwrap().trim();
        if body.is_empty() {
            continue;
        }
        let mut s = String::new();
        let mut want = Vec::new();
        for tok in body.split_whitespace() {
            match tok {
                "÷" => want.push(true),
                "×" => want.push(false),
                h => s.push(char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap()),
            }
        }
        total += 1;
        let got = grapheme::boundaries(&s);
        if got == want {
            pass += 1;
        } else if fails.len() < 10 {
            fails.push(format!("{line}\n  got {got:?}"));
        }
    }
    eprintln!("GraphemeBreakTest: {pass}/{total}");
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(total > 700);
    assert_eq!(pass, total);
}

#[test]
fn normalization_test_in_full() {
    let Some(data) = vector_file("NormalizationTest.txt", &format!("{UCD_BASE}NormalizationTest.txt"),
        "5019ffd530751a741900c849c0e010332f142a3612234639bd200b82138a87db") else { return };
    let text = String::from_utf8(data).unwrap();
    let parse = |f: &str| -> String {
        f.split_whitespace().map(|h| char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap()).collect()
    };
    let (mut pass, mut total) = (0, 0);
    let mut fails = Vec::new();
    let mut part1 = std::collections::HashSet::new();
    let mut part = String::new();
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("@Part") {
            part = p.split_whitespace().next().unwrap_or("").to_string();
            continue;
        }
        let body = line.split('#').next().unwrap().trim();
        if body.is_empty() {
            continue;
        }
        let f: Vec<String> = body.split(';').take(5).map(parse).collect();
        if part == "1" {
            part1.insert(f[0].chars().next().unwrap());
        }
        let (c1, c2, c3, c4, c5) = (&f[0], &f[1], &f[2], &f[3], &f[4]);
        // NFC: c2 == toNFC(c1) == toNFC(c2) == toNFC(c3); c4 == toNFC(c4) == toNFC(c5).
        // NFD: c3 == toNFD(c1) == toNFD(c2) == toNFD(c3); c5 == toNFD(c4) == toNFD(c5).
        let ok = normalize::nfc(c1) == *c2 && normalize::nfc(c2) == *c2 && normalize::nfc(c3) == *c2
            && normalize::nfc(c4) == *c4 && normalize::nfc(c5) == *c4
            && normalize::nfd(c1) == *c3 && normalize::nfd(c2) == *c3 && normalize::nfd(c3) == *c3
            && normalize::nfd(c4) == *c5 && normalize::nfd(c5) == *c5;
        total += 1;
        if ok {
            pass += 1;
        } else if fails.len() < 10 {
            fails.push(format!("{line}\n  nfc {:X?} nfd {:X?}", normalize::nfc(c1).chars().map(|c| c as u32).collect::<Vec<_>>(), normalize::nfd(c1).chars().map(|c| c as u32).collect::<Vec<_>>()));
        }
    }
    // Every code point not in part 1 is invariant under NFC and NFD.
    let mut inv = 0;
    for cp in 0..=0x10FFFFu32 {
        let Some(c) = char::from_u32(cp) else { continue };
        if part1.contains(&c) {
            continue;
        }
        let s = c.to_string();
        total += 1;
        if normalize::nfc(&s) == s && normalize::nfd(&s) == s {
            pass += 1;
            inv += 1;
        } else if fails.len() < 10 {
            fails.push(format!("invariant U+{cp:04X}"));
        }
    }
    eprintln!("NormalizationTest: {pass}/{total} ({inv} invariant code points)");
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert_eq!(pass, total);
}

#[test]
fn script_itemizer_runs() {
    let runs = |t: &str| script::itemize(t).into_iter().map(|(s, e, sc)| (t[s..e].to_string(), String::from_utf8(sc.to_vec()).unwrap())).collect::<Vec<_>>();
    let r = runs("Hello مرحبا world");
    assert_eq!(r.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["Latn", "Arab", "Latn"]);
    assert_eq!(r[0].0, "Hello ");
    // A closing bracket follows its opener's script; combining marks (Inherited) stay with their base.
    let r = runs("abc (שלום) d\u{301}");
    assert_eq!(r.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["Latn", "Hebr", "Latn"]);
    assert_eq!(r[1].0, "שלום");
    assert_eq!(r[2].0, ") d\u{301}");
    // Danda (Script_Extensions Beng Deva …) stays inside a Devanagari run; leading digits join it.
    let r = runs("१२ नमस्ते। ok");
    assert_eq!(r.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["Deva", "Latn"]);
    assert_eq!(r[0].0, "१२ नमस्ते। ");
    // Arabic tatweel / comma (Script_Extensions) inside Arabic.
    assert_eq!(runs("كتـاب، كتب").len(), 1);
    assert_eq!(runs("123 !?").first().unwrap().1, "Zyyy");
    assert_eq!(script::ot_script_tag(*b"Deva"), *b"dev2");
    assert_eq!(script::ot_script_tag(*b"Arab"), *b"arab");
    assert_eq!(script::script('ก'), *b"Thai");
    assert_eq!(script::script('\u{0300}'), *b"Zinh");
    assert_eq!(script::script('\u{0378}'), *b"Zzzz");
}

#[test]
fn grapheme_clusters_kat() {
    // Devanagari conjunct (GB9c), an emoji ZWJ family (GB11), flags (GB12/13), Hangul (GB6–8), CRLF.
    let c = |t: &str| grapheme::clusters(t).len();
    assert_eq!(c("क्षि"), 1);
    assert_eq!(c("👨\u{200D}👩\u{200D}👧"), 1);
    assert_eq!(c("🇩🇪🇫🇷"), 2);
    assert_eq!(c("\u{1100}\u{1161}\u{11A8}"), 1);
    assert_eq!(c("a\r\nb"), 3);
    assert_eq!(normalize::nfc("e\u{301}"), "é");
    assert_eq!(normalize::nfd("ệ"), "e\u{323}\u{302}");
    assert_eq!(normalize::nfc("\u{1100}\u{1161}\u{11A8}"), "각");
}
