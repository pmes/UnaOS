// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! INFERCORE (SR57) M3: candle (the path EMBED B317 shipped) vs infer_core on the 100 oracle
//! sentences, the SAME ids (infer_core's tokenizer, == HF tokenizers) into both encoders, and the
//! timing table. With RECORD_CANDLE=<file> it writes candle's vectors (100 × 384 f32 LE) — recorded
//! once into infer_core/kat/minilm_candle.f32 before M4 took candle out of gneiss_pal.
#![cfg(feature = "local-embed")]

use std::path::PathBuf;
use std::time::Instant;

use gneiss_pal::api::local::LocalEmbedder;
use infer_core::bert::{Bert, Config, graph_from_onnx};
use infer_core::tokenizer::Tokenizer;

fn cos(a: &[f32], b: &[f32]) -> f64 {
    let d: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    d / (a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt() * b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt())
}

#[test]
fn candle_vs_infer_core() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::var_os("INFER_CORE_VECTORS").map(PathBuf::from).unwrap_or_else(|| root.join("target/infer_core-vectors")).join("minilm-onnx/onnx");
    if !dir.join("model.onnx").is_file() {
        eprintln!("SKIP: run `cargo test -p infer_core` first (it fetches the model into {})", dir.display());
        return;
    }
    let kat = root.join("unaos/libs/ml/infer_core/kat");
    let sents: Vec<String> = std::fs::read_to_string(kat.join("sentences.txt"))
        .unwrap()
        .lines()
        .map(|l| infer_core::json::parse(l).unwrap().as_str().unwrap().to_string())
        .collect();
    let tok = Tokenizer::from_tokenizer_json(&std::fs::read_to_string(dir.join("tokenizer.json")).unwrap()).unwrap();
    let ids: Vec<Vec<u32>> = sents.iter().map(|s| tok.encode_truncated(s, 256)).collect();

    let candle = LocalEmbedder::load_from("all-MiniLM-L6-v2", &dir, 384).unwrap();
    let cfg = Config::from_json(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    let bert = Bert::load(cfg, &graph_from_onnx(&std::fs::read(dir.join("model.onnx")).unwrap()).unwrap(), false).unwrap();

    // The shipped path's own tokenizer vs HF ids.
    let differ = sents.iter().zip(&ids).filter(|(s, i)| &candle.token_ids(s) != *i).count();
    eprintln!("EMBED B317 wordpiece vs HF tokenizers on the 100 sentences: {differ} differ");

    let time = |f: &mut dyn FnMut(&[Vec<u32>]) -> Vec<Vec<f32>>, batch: usize| {
        let t = Instant::now();
        let mut out = Vec::new();
        for c in ids.chunks(batch) {
            out.extend(f(c));
        }
        (out, t.elapsed().as_secs_f64() * 1e3 / ids.len() as f64)
    };
    let mut c_run = |c: &[Vec<u32>]| candle.embed_ids_batch(c).unwrap();
    let mut i_run = |c: &[Vec<u32>]| bert.embed(&c.iter().map(|v| v.as_slice()).collect::<Vec<_>>()).unwrap();
    let (cv1, c1) = time(&mut c_run, 1);
    let (cv16, c16) = time(&mut c_run, 16);
    let (iv1, i1) = time(&mut i_run, 1);
    let (_, i16) = time(&mut i_run, 16);
    let mut worst = (1.0f64, 0.0f32);
    for (a, b) in iv1.iter().zip(&cv1) {
        worst.0 = worst.0.min(cos(a, b));
        worst.1 = worst.1.max(a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max));
    }
    let b16 = cv1.iter().flatten().zip(cv16.iter().flatten()).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    eprintln!("infer_core vs candle (same ids): worst cosine {:.9}, worst max|diff| {:.3e}; candle batch16 vs batch1 max|diff| {b16:.3e}", worst.0, worst.1);
    eprintln!("| path | batch 1 ms/sentence | batch 16 ms/sentence |");
    eprintln!("| candle 0.11 (CPU, its thread pool) | {c1:.2} | {c16:.2} |");
    eprintln!("| infer_core (1 thread) | {i1:.2} | {i16:.2} |");
    assert!(worst.0 >= 0.9999 && worst.1 <= 1e-4);
    if let Some(out) = std::env::var_os("RECORD_CANDLE") {
        let bytes: Vec<u8> = cv1.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(out, bytes).unwrap();
    }
}
