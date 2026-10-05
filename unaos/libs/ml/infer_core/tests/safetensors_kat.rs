// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! M1 oracle for the safetensors reader:
//! - `kat/reader_kat.safetensors`, written by HF's own `safetensors` 0.8.0 writer (kat/record.py):
//!   F32, F16, BF16, I64 and a scalar; its metadata carries the f32 bits numpy / ml_dtypes give
//!   for every stored float — the reader must reproduce them exactly.
//! - a real model file: all-MiniLM-L6-v2 as F16 safetensors (HF layout, 103 F16 tensors + an
//!   I64 one), each F16 tensor but the pooler's two checked against the ONNX export's f32 weight: it must be exactly
//!   the round-to-nearest binary16 of it (its writer breaks exact ties away from zero; numpy and
//!   INFERCORE break them to even — the handful of ties is counted) (which proves the reader, the f16 conversion, the
//!   ONNX reader + renaming, and the file's provenance at once).

mod common;

use infer_core::bert::graph_from_onnx;
use infer_core::half::f32_to_f16;
use infer_core::safetensors::{Dtype, SafeTensors};

#[test]
fn reads_the_reference_writers_file() {
    let b = common::pinned("reader_kat.safetensors", "3dd18bed4b5b04021e187281293775b5a0c2b04d1a064e0b195d128a23346448");
    let st = SafeTensors::parse(&b).unwrap();
    assert_eq!(st.tensors.len(), 5);
    let meta = |k: &str| st.metadata.iter().find(|(m, _)| m == k).map(|(_, v)| v.clone()).unwrap();
    assert_eq!(meta("format"), "pt");
    for (name, dtype, shape) in [("f32", Dtype::F32, vec![8, 8]), ("f16", Dtype::F16, vec![4, 16]), ("bf16", Dtype::BF16, vec![2, 4, 8]), ("scalar", Dtype::F32, vec![])] {
        let t = st.get(name).unwrap();
        assert_eq!((t.dtype, &t.shape), (dtype, &shape), "{name}");
        let want: Vec<u32> = meta(&format!("expect.{name}")).split(' ').map(|h| u32::from_str_radix(h, 16).unwrap()).collect();
        let got: Vec<u32> = t.to_f32().unwrap().iter().map(|v| v.to_bits()).collect();
        assert_eq!(got, want, "{name}");
    }
    let ids = st.get("ids").unwrap();
    assert_eq!((ids.dtype, ids.numel()), (Dtype::I64, 12));
    assert!(ids.to_f32().is_err());
    eprintln!("safetensors reader KAT: 4 float tensors (F32/F16/BF16/scalar) bit-exact vs numpy/ml_dtypes; I64 listed");
}

#[test]
fn f16_model_file_is_the_rounded_onnx_weights() {
    let (Some(f16dir), Some(onnx)) = (common::minilm_f16(), common::minilm()) else { return };
    let bytes = common::read(&f16dir, "model.fp16.safetensors");
    let st = SafeTensors::parse(&bytes).unwrap();
    let g = graph_from_onnx(&common::read(&onnx, "model.onnx")).unwrap();
    let (mut checked, mut exact, mut ties) = (0, 0usize, 0usize);
    for t in &st.tensors {
        // The pooler (unused by sentence embeddings) is not in the ONNX export.
        if t.dtype != Dtype::F16 || t.name.starts_with("pooler.") {
            continue;
        }
        let w = g.initializers.get(&t.name).unwrap_or_else(|| panic!("ONNX has no {}", t.name));
        assert_eq!(w.dims, t.shape, "{}", t.name);
        let h = t.f16_bits().unwrap();
        for (f, hf) in w.data.iter().zip(&h) {
            let rne = f32_to_f16(*f);
            if rne == *hf {
                exact += 1;
                continue;
            }
            // The file's writer rounds exact ties away from zero (numpy agrees with OUR result):
            // allowed only for a value exactly halfway between two binary16 neighbours.
            let tie = f.to_bits() & 0x1fff == 0x1000 && *hf == rne + 1;
            assert!(tie, "{}: f32 {f:e} ({:08x}) → ours {rne:04x}, file {hf:04x}", t.name, f.to_bits());
            ties += 1;
        }
        checked += 1;
    }
    assert_eq!(checked, 101);
    eprintln!("f16 safetensors: {checked} tensors, {exact} values == f16_rne(onnx f32), {ties} exact ties rounded away from zero by the file's writer");
    assert!(ties * 1000 < exact, "too many ties: {ties}");
}
