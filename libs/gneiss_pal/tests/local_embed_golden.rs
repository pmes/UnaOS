// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! EMBED (B317) M2 golden test: the local embedder against a reference.
//!
//! The fixture (`fixtures/minilm_golden.json`) is 20 sentences with their
//! token ids and 384-dim vectors from the reference stack — onnxruntime +
//! HF `tokenizers` on the SAME pinned files `tools/una-models` installs,
//! mean-pooled and L2-normalised. Runs ONLY when the model is installed;
//! otherwise it prints why it skipped.
#![cfg(feature = "local-embed")]

use gneiss_pal::api::local::{LocalEmbedder, model_dir};

fn cos(a: &[f32], b: &[f32]) -> f32 {
    let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    d / (a.iter().map(|x| x * x).sum::<f32>().sqrt() * b.iter().map(|x| x * x).sum::<f32>().sqrt())
}

#[test]
fn minilm_matches_the_reference_on_20_sentences() {
    let name = "all-MiniLM-L6-v2";
    if !model_dir(name).join("model.onnx").is_file() {
        eprintln!("SKIP minilm golden: {name} is not installed in {} — run `tools/una-models fetch {name}`", model_dir(name).display());
        return;
    }
    let e = LocalEmbedder::load(name, 384).expect("load");
    let fx: serde_json::Value = serde_json::from_str(include_str!("fixtures/minilm_golden.json")).unwrap();
    let sents = fx["sentences"].as_array().unwrap();
    assert_eq!(sents.len(), 20);
    let mut vecs = Vec::new();
    let mut worst = 1.0f32;
    for s in sents {
        let text = s["text"].as_str().unwrap();
        let want_ids: Vec<u32> = s["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
        assert_eq!(e.token_ids(text), want_ids, "token ids for {text:?}");
        let want: Vec<f32> = s["vec"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let got = e.embed_text(text).unwrap();
        assert_eq!(got.len(), 384);
        let c = cos(&got, &want);
        worst = worst.min(c);
        assert!(c > 0.9999, "cosine to reference {c} for {text:?}");
        let maxdiff = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(maxdiff < 1e-3, "max |diff| {maxdiff} for {text:?}");
        vecs.push(got);
    }
    eprintln!("minilm golden: 20/20 token-exact, worst cosine to reference {worst:.7}");
    // Meaning, not keywords: the paraphrase pairs are each other's nearest neighbour.
    for (a, b) in [(0, 1), (2, 3), (6, 7)] {
        let near = (0..20).filter(|&j| j != a).max_by(|&x, &y| cos(&vecs[a], &vecs[x]).total_cmp(&cos(&vecs[a], &vecs[y]))).unwrap();
        assert_eq!(near, b, "nearest to {:?}", sents[a]["text"]);
    }
}
