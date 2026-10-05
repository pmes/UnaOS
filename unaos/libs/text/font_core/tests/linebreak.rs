//! UAX #14 against the official conformance file: FONTCORE's committed subset (tests/data/lbtest_subset.txt) and,
//! since FONTBIDI (SR56), LineBreakTest-17.0.0.txt in full (fetched at test time, sha-pinned, SKIP offline).

mod common;
use common::{vector_file, UCD_BASE};
use font_core::linebreak::{breaks, supported, Break};

fn check(data: &str) -> (usize, usize, Vec<String>) {
    let (mut pass, mut total) = (0, 0);
    let mut fails = Vec::new();
    for line in data.lines() {
        let body = line.split('#').next().unwrap().trim();
        if body.is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut want = Vec::new();
        for tok in body.split_whitespace() {
            match tok {
                "÷" => want.push(true),
                "×" => want.push(false),
                hex => text.push(char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap()),
            }
        }
        let got: Vec<bool> = breaks(&text).iter().map(|b| *b != Break::None).collect();
        total += 1;
        if got == want {
            pass += 1;
        } else if fails.len() < 12 {
            fails.push(format!("{line}\n   got  {got:?}"));
        }
    }
    (pass, total, fails)
}

#[test]
fn line_break_test_in_full() {
    let Some(data) = vector_file("LineBreakTest.txt", &format!("{UCD_BASE}auxiliary/LineBreakTest.txt"),
        "e69884e0dde6a8724873f885d68c52dc14518abf9ae4ca9e2283b8773db3b752") else { return };
    let (pass, total, fails) = check(&String::from_utf8(data).unwrap());
    eprintln!("UAX14 full: {pass}/{total} LineBreakTest lines");
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(total > 19_000);
    assert_eq!(pass, total);
}

#[test]
fn line_break_test_subset() {
    let data = include_str!("data/lbtest_subset.txt");
    let (mut pass, mut total) = (0, 0);
    let mut fails = Vec::new();
    for line in data.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut want = Vec::new(); // decision before each char, then at end
        for tok in line.split_whitespace() {
            match tok {
                "÷" => want.push(true),
                "×" => want.push(false),
                hex => text.push(char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap()),
            }
        }
        assert!(text.chars().all(supported));
        let got: Vec<bool> = breaks(&text).iter().map(|b| *b != Break::None).collect();
        total += 1;
        // The file marks sot as × and eot as ÷: same convention as `breaks`.
        if got == want {
            pass += 1;
        } else if fails.len() < 10 {
            fails.push(format!("{line}\n   got  {got:?}"));
        }
    }
    eprintln!("UAX14: {pass}/{total} LineBreakTest lines");
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert_eq!(pass, total);
}

#[test]
fn mandatory_and_basic_breaks() {
    let b = breaks("Hello world\nnext");
    // before 'w' (index 6) allowed, after '\n' (index 12) mandatory, inside words none.
    assert_eq!(b[6], Break::Allowed);
    assert_eq!(b[12], Break::Mandatory);
    assert_eq!(b[3], Break::None);
    // No break before closing punctuation or inside a number.
    let b = breaks("(3.14), 1,000!");
    assert_eq!(b[5], Break::None);
    assert_eq!(b[10], Break::None);
    // Hyphen: break after, not before.
    let b = breaks("well-known");
    assert_eq!(b[4], Break::None);
    assert_eq!(b[5], Break::Allowed);
}
