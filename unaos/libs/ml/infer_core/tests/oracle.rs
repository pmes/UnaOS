// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! M2/M3 oracle: all-MiniLM-L6-v2 through INFERCORE vs the reference.
//!
//! Reference (`kat/minilm_ort.f32`, sha-pinned, recorded once by kat/record.py): HF `tokenizers`
//! 0.23.2 (truncation 256) → onnxruntime 1.30.0 on the SAME pinned model.onnx → masked mean →
//! L2 normalise, in f32 — sentence-transformers' recipe. 100 sentences (kat/sentences.txt).
//! Gate: every sentence cosine ≥ 0.9999 and max |diff| ≤ 1e-4 (f32 weights). The f16 weight paths
//! are measured and gated more loosely (the weights themselves differ by up to 2^-11 relative).
//! The model is fetched at test time (vectors.txt), never committed; offline → SKIP.

mod common;

use std::time::Instant;

use infer_core::bert::{Bert, Config, graph_from_onnx};
use infer_core::json;
use infer_core::safetensors::SafeTensors;
use infer_core::tokenizer::Tokenizer;

pub const MAX_TOKENS: usize = 256;

fn sentences() -> Vec<String> {
    let b = common::pinned("sentences.txt", "521798aeb06f33aac6ff4842dc4a5006e952e872145a8c3c7424093575e3dc7a");
    String::from_utf8(b).unwrap().lines().map(|l| json::parse(l).unwrap().as_str().unwrap().to_string()).collect()
}

fn reference() -> Vec<Vec<f32>> {
    let b = common::pinned("minilm_ort.f32", "8db514f130caff2090a34eb6d368fa4219bc0fd46ea6596aa7650dffa82628cf");
    let f: Vec<f32> = b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    f.chunks_exact(384).map(|c| c.to_vec()).collect()
}

fn cos(a: &[f32], b: &[f32]) -> f64 {
    let d: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let na: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    d / (na * nb)
}

fn maxdiff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// (worst cosine, worst max |diff|) of `got` against `want`.
fn compare(got: &[Vec<f32>], want: &[Vec<f32>]) -> (f64, f32) {
    let mut w = (1.0f64, 0.0f32);
    for (g, r) in got.iter().zip(want) {
        w.0 = w.0.min(cos(g, r));
        w.1 = w.1.max(maxdiff(g, r));
    }
    w
}

fn embed_all(b: &Bert, ids: &[Vec<u32>], batch: usize) -> Vec<Vec<f32>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(batch) {
        let seqs: Vec<&[u32]> = chunk.iter().map(|v| v.as_slice()).collect();
        out.extend(b.embed(&seqs).unwrap());
    }
    out
}

#[test]
fn minilm_matches_the_reference_on_100_sentences() {
    let sents = sentences();
    let want = reference();
    assert_eq!((sents.len(), want.len()), (100, 100));
    let Some(dir) = common::minilm() else { return };
    let tok = Tokenizer::from_tokenizer_json(std::str::from_utf8(&common::read(&dir, "tokenizer.json")).unwrap()).unwrap();
    let cfg = Config::from_json(std::str::from_utf8(&common::read(&dir, "config.json")).unwrap()).unwrap();
    let t0 = Instant::now();
    let graph = graph_from_onnx(&common::read(&dir, "model.onnx")).unwrap();
    let bert = Bert::load(cfg.clone(), &graph, false).unwrap();
    let load_ms = t0.elapsed().as_secs_f64() * 1e3;
    let ids: Vec<Vec<u32>> = sents.iter().map(|s| tok.encode_truncated(s, MAX_TOKENS)).collect();
    let tokens: usize = ids.iter().map(Vec::len).sum();

    // f32 weights, batch 1 (timed).
    let t = Instant::now();
    let single = embed_all(&bert, &ids, 1);
    let ms1 = t.elapsed().as_secs_f64() * 1e3 / 100.0;
    let (c, d) = compare(&single, &want);
    eprintln!("oracle f32 vs onnxruntime: 100 sentences ({tokens} tokens), worst cosine {c:.9}, worst max|diff| {d:.3e}");
    for (i, (g, r)) in single.iter().zip(&want).enumerate() {
        assert!(cos(g, r) >= 0.9999 && maxdiff(g, r) <= 1e-4, "sentence {i} {:?}: cos {} maxdiff {}", sents[i], cos(g, r), maxdiff(g, r));
    }

    // Batch 16: the same bits as batch 1.
    let t = Instant::now();
    let batched = embed_all(&bert, &ids, 16);
    let ms16 = t.elapsed().as_secs_f64() * 1e3 / 100.0;
    assert!(batched.iter().flatten().zip(single.iter().flatten()).all(|(a, b)| a.to_bits() == b.to_bits()), "batch 16 != batch 1");
    eprintln!("timing (this host, 1 thread, f32): load {load_ms:.0} ms; batch 1 {ms1:.2} ms/sentence; batch 16 {ms16:.2} ms/sentence (bit-identical)");

    // f16 weights from the f32 file (RNE at load).
    let half = Bert::load(cfg.clone(), &graph, true).unwrap();
    let t = Instant::now();
    let h = embed_all(&half, &ids, 16);
    let msh = t.elapsed().as_secs_f64() * 1e3 / 100.0;
    let (c, d) = compare(&h, &want);
    eprintln!("oracle f16 weights (rounded at load) vs onnxruntime: worst cosine {c:.9}, worst max|diff| {d:.3e}; batch 16 {msh:.2} ms/sentence");
    assert!(c >= 0.999 && d <= 5e-3);

    // The F16 safetensors file, both as f16 weights and widened to f32.
    if let Some(fdir) = common::minilm_f16() {
        let bytes = common::read(&fdir, "model.fp16.safetensors");
        let st = SafeTensors::parse(&bytes).unwrap();
        for half in [true, false] {
            let b = Bert::load(cfg.clone(), &st, half).unwrap();
            let v = embed_all(&b, &ids, 16);
            let (c, d) = compare(&v, &want);
            eprintln!("oracle F16 safetensors (half={half}) vs onnxruntime: worst cosine {c:.9}, worst max|diff| {d:.3e}");
            assert!(c >= 0.999 && d <= 5e-3);
        }
    }
}
