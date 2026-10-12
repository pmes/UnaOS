// SPDX-License-Identifier: LGPL-3.0-or-later
// EDITCORE (SR73): UAX #29 conformance — every line of GraphemeBreakTest-17.0.0 and WordBreakTest-17.0.0
// (tests/data/*.txt, cut by tools/gen_tables.py, comments removed) replayed against the core's segmenters.

fn replay(data: &str, seg: fn(&str) -> Vec<usize>) -> (usize, Vec<String>) {
    let mut n = 0;
    let mut fails = Vec::new();
    for line in data.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut s = String::new();
        let mut want = Vec::new();
        for tok in line.split_whitespace() {
            match tok {
                "÷" => want.push(s.len()),
                "×" => {}
                hex => s.push(char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap()),
            }
        }
        n += 1;
        let got = seg(&s);
        if got != want {
            fails.push(format!("{line}\n   want {want:?}\n   got  {got:?}"));
        }
    }
    (n, fails)
}

#[test]
fn grapheme_break_test_17() {
    let (n, fails) = replay(include_str!("data/grapheme_test.txt"), edit_core::unicode::grapheme_boundaries);
    println!("GraphemeBreakTest-17.0.0: {} cases, {} fail", n, fails.len());
    assert!(n > 700, "vector file truncated: {n}");
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails[..fails.len().min(10)].join("\n"));
}

#[test]
fn word_break_test_17() {
    let (n, fails) = replay(include_str!("data/word_test.txt"), edit_core::unicode::word_boundaries);
    println!("WordBreakTest-17.0.0: {} cases, {} fail", n, fails.len());
    assert!(n > 1500, "vector file truncated: {n}");
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails[..fails.len().min(10)].join("\n"));
}
