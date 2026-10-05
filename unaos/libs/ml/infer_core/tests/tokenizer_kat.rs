// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! M1 oracle: the tokenizer against HF `tokenizers` 0.23.2 on 2000 strings (kat/record.py
//! recorded the reference ids once; the files are sha256-pinned). Byte-equal ids or the test fails.

mod common;

use infer_core::json;
use infer_core::tokenizer::Tokenizer;

const KAT: [(&str, &str); 2] = [
    ("tokenizer_kat_0.txt", "7d78a2be047444c41621de368d20428632027cfe885ac781ecbb6c7c614b73d6"),
    ("tokenizer_kat_1.txt", "43aed5913b47583e9c4598866da2c5d4225f70a3ac74e3d87e880b2d5228229d"),
];

fn cases() -> Vec<(String, Vec<u32>)> {
    let mut out = Vec::new();
    for (f, sha) in KAT {
        let txt = String::from_utf8(common::pinned(f, sha)).unwrap();
        for line in txt.lines() {
            let (s, ids) = line.split_once('\t').unwrap();
            let s = json::parse(s).unwrap().as_str().unwrap().to_string();
            let ids = ids.split(' ').map(|v| v.parse().unwrap()).collect();
            out.push((s, ids));
        }
    }
    out
}

fn check(name: &str, t: &Tokenizer, cases: &[(String, Vec<u32>)]) {
    let mut bad = Vec::new();
    for (s, want) in cases {
        let got = t.encode(s);
        if &got != want {
            bad.push((s, got, want));
        }
    }
    for (s, got, want) in bad.iter().take(12) {
        eprintln!("MISMATCH {name} {s:?}\n   got  {got:?}\n   want {want:?}");
    }
    eprintln!("tokenizer KAT ({name}): {}/{} strings byte-equal to HF tokenizers", cases.len() - bad.len(), cases.len());
    assert!(bad.is_empty(), "{} of {} strings differ", bad.len(), cases.len());
}

#[test]
fn ids_equal_hf_tokenizers_on_2000_strings() {
    let cases = cases();
    assert_eq!(cases.len(), 2000);
    let Some(dir) = common::minilm() else { return };
    let tj = Tokenizer::from_tokenizer_json(std::str::from_utf8(&common::read(&dir, "tokenizer.json")).unwrap()).unwrap();
    check("tokenizer.json", &tj, &cases);
    let tv = Tokenizer::from_vocab_txt(std::str::from_utf8(&common::read(&dir, "vocab.txt")).unwrap()).unwrap();
    check("vocab.txt", &tv, &cases);
}
