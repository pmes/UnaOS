// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! M3 robustness: malformed safetensors / ONNX / tokenizer.json / vocab.txt / config.json — every
//! input is either accepted or answered with an `Error`; nothing panics, hangs or over-allocates.
//! Deterministic (a fixed-seed generator), so a failure reproduces. The mutations: bit flips, byte
//! overwrites with boundary values, truncation, duplication of a span, header-length corruption,
//! random garbage — on the committed KAT file and, when fetched, on the real model files' heads.

mod common;

use infer_core::bert::{Bert, Config};
use infer_core::safetensors::SafeTensors;
use infer_core::tokenizer::Tokenizer;
use infer_core::{json, onnx};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(r: &mut Rng, src: &[u8]) -> Vec<u8> {
    let mut v = src.to_vec();
    for _ in 0..1 + r.below(4) {
        if v.is_empty() {
            v.push(r.next() as u8);
            continue;
        }
        match r.below(7) {
            0 => {
                let i = r.below(v.len());
                v[i] ^= 1 << r.below(8);
            }
            1 => {
                let i = r.below(v.len());
                v[i] = [0, 0xff, 0x7f, 0x80, b'"', b'{', b'[', b'9', b'-', b'\\'][r.below(10)];
            }
            2 => v.truncate(r.below(v.len())),
            3 => {
                let a = r.below(v.len());
                let b = (a + r.below(64)).min(v.len());
                let span = v[a..b].to_vec();
                let at = r.below(v.len());
                v.splice(at..at, span);
            }
            4 if v.len() >= 8 => {
                // Header length (safetensors) / leading bytes.
                let n = [0u64, 1, 7, u64::MAX, 1 << 40, v.len() as u64, (v.len() as u64).wrapping_sub(9)][r.below(7)];
                v[..8].copy_from_slice(&n.to_le_bytes());
            }
            5 => {
                let i = r.below(v.len());
                v.remove(i);
            }
            _ => {
                let n = r.below(32);
                let at = r.below(v.len());
                let junk: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
                v.splice(at..at, junk);
            }
        }
    }
    v
}

#[test]
fn malformed_safetensors_never_panic() {
    let seed = common::kat("reader_kat.safetensors");
    let mut r = Rng(0x5eed_0001);
    let mut ok = 0;
    for _ in 0..20000 {
        let m = mutate(&mut r, &seed);
        if let Ok(st) = SafeTensors::parse(&m) {
            ok += 1;
            for t in &st.tensors {
                let _ = t.to_f32();
            }
        }
    }
    for _ in 0..5000 {
        let n = r.below(300);
        let junk: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
        assert!(SafeTensors::parse(&junk).is_err() || n >= 10);
    }
    eprintln!("fuzz safetensors: 20000 mutants + 5000 random, {ok} still parse, no panic");
}

#[test]
fn malformed_json_config_and_tokenizer_never_panic() {
    let cfg = br#"{"vocab_size":11,"hidden_size":8,"num_hidden_layers":2,"num_attention_heads":2,"intermediate_size":12,"max_position_embeddings":16,"type_vocab_size":2,"layer_norm_eps":1e-12,"hidden_act":"gelu"}"#;
    let tj = r###"{"added_tokens":[{"id":0,"content":"[PAD]","normalized":false},{"id":1,"content":"[UNK]","lstrip":true,"rstrip":true},{"id":2,"content":"[CLS]","single_word":true},{"id":3,"content":"[SEP]"}],"normalizer":{"type":"BertNormalizer","strip_accents":null},"pre_tokenizer":{"type":"BertPreTokenizer"},"post_processor":{"type":"BertProcessing","sep":["[SEP]",3],"cls":["[CLS]",2]},"model":{"type":"WordPiece","unk_token":"[UNK]","max_input_chars_per_word":5,"vocab":{"[PAD]":0,"[UNK]":1,"[CLS]":2,"[SEP]":3,"a":4,"##b":5,"é":6}}}"###.as_bytes();
    let vocab = b"[PAD]\n[UNK]\n[CLS]\n[SEP]\na\n##b\n";
    let mut r = Rng(0x5eed_0002);
    let texts = ["ab [UNK] a", "[CLS]ab", "", "ééé", "\u{0}\u{e000}x", "a\u{301}b"];
    for i in 0..20000 {
        let m = mutate(&mut r, cfg);
        if let Ok(s) = std::str::from_utf8(&m) {
            let _ = Config::from_json(s);
        }
        let m = mutate(&mut r, tj);
        if let Ok(s) = std::str::from_utf8(&m) {
            if let Ok(t) = Tokenizer::from_tokenizer_json(s) {
                let _ = t.encode(texts[i % texts.len()]);
            }
        }
        let m = mutate(&mut r, vocab);
        if let Ok(s) = std::str::from_utf8(&m) {
            if let Ok(t) = Tokenizer::from_vocab_txt(s) {
                let _ = t.encode_truncated(texts[i % texts.len()], 4);
            }
        }
    }
    // The pristine tokenizer.json works, with its flags.
    let t = Tokenizer::from_tokenizer_json(std::str::from_utf8(tj).unwrap()).unwrap();
    assert_eq!(t.encode("ab  [UNK]  a"), vec![2, 4, 5, 1, 4, 3]);
    // single_word: glued to "x", so not special — normalised to "x[cls]" → x [ cls ] → four [UNK].
    assert_eq!(t.encode("x[CLS]"), vec![2, 1, 1, 1, 1, 3]);
    assert_eq!(t.encode("x [CLS]"), vec![2, 1, 2, 3]);
    assert_eq!(t.encode("abbbbb"), vec![2, 1, 3]); // over max_input_chars_per_word
    // Deep nesting and huge numbers.
    assert!(json::parse(&"[".repeat(100_000)).is_err());
    assert!(Config::from_json(r#"{"vocab_size":1e400}"#).is_err());
    eprintln!("fuzz json/config/tokenizer.json/vocab.txt: 3 × 20000 mutants, no panic");
}

#[test]
fn malformed_onnx_and_model_heads_never_panic() {
    let mut r = Rng(0x5eed_0003);
    // A small valid ModelProto (two initializers + two nodes), then mutants of it.
    fn len(field: u64, body: &[u8]) -> Vec<u8> {
        let mut v = vec![((field << 3) | 2) as u8];
        let mut n = body.len();
        while n >= 0x80 {
            v.push((n as u8 & 0x7f) | 0x80);
            n >>= 7;
        }
        v.push(n as u8);
        v.extend_from_slice(body);
        v
    }
    let mut t = len(1, &[2, 3]);
    t.extend([0x10, 1]);
    t.extend(len(8, b"onnx::MatMul_1"));
    t.extend(len(9, &[0u8; 24]));
    let mut b = vec![0x08, 3, 0x10, 1];
    b.extend(len(8, b"l.q.bias"));
    b.extend(len(9, &[0u8; 12]));
    let mut mm = len(1, b"x");
    mm.extend(len(1, b"onnx::MatMul_1"));
    mm.extend(len(2, b"y"));
    mm.extend(len(4, b"MatMul"));
    let mut add = len(1, b"l.q.bias");
    add.extend(len(1, b"y"));
    add.extend(len(4, b"Add"));
    let mut g = len(1, &mm);
    g.extend(len(1, &add));
    g.extend(len(5, &t));
    g.extend(len(5, &b));
    let model = len(7, &g);
    assert!(onnx::parse_model(&model).is_ok());
    for _ in 0..20000 {
        let m = mutate(&mut r, &model);
        if let Ok(mut g) = onnx::parse_model(&m) {
            let _ = onnx::name_linear_weights(&mut g);
        }
    }
    // The real files, when fetched: mutate their first 64 KB (header, first tensors).
    if let Some(dir) = common::minilm_f16() {
        let st = common::read(&dir, "model.fp16.safetensors");
        let head = &st[..65536];
        for _ in 0..300 {
            let _ = SafeTensors::parse(&mutate(&mut r, head));
        }
        // A real header with a wrong config: a clean error, not a panic.
        let full = SafeTensors::parse(&st).unwrap();
        let bad = Config::from_json(r#"{"vocab_size":30522,"hidden_size":384,"num_hidden_layers":7,"num_attention_heads":12,"intermediate_size":1536,"max_position_embeddings":512}"#).unwrap();
        assert!(Bert::load(bad, &full, false).is_err());
    }
    if let Some(dir) = common::minilm() {
        let o = common::read(&dir, "model.onnx");
        for _ in 0..100 {
            let _ = onnx::parse_model(&mutate(&mut r, &o[..65536]));
        }
    }
    eprintln!("fuzz onnx: 20000 mutants (+ real-file heads when fetched), no panic");
}
