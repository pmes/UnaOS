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

//! Cosine-similarity known-answer tests: the query-score contract.
//!
//! `cosine_similarity` routes its FP `sqrt` through `libm` on EVERY build
//! (`std` and `no_std` alike), so the function these tests exercise is the
//! exact code the kernel's `no_std` build compiles — one path, one answer.
//! The golden bit patterns below were computed once from an independent
//! reference (sequential IEEE 754 f32 folds + the host's correctly-rounded
//! `f32::sqrt`) and are asserted BIT-FOR-BIT.
//!
//! Like `kat_vectors.rs` for the on-disk format, these vectors are a
//! contract: a scoring change that shifts any bit pattern is a divergence
//! between kernel-answered and host-answered queries. Do not edit the
//! goldens to make a change pass.

use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, MemDevice, UnaFS, cosine_similarity};

/// (name, a, b, golden f32 bits)
const GOLDEN: &[(&str, &[f32], &[f32], u32)] = &[
    // Orthogonal: dot = 0 exactly.
    ("orthogonal", &[1.0, 0.0], &[0.0, 1.0], 0x0000_0000),
    // Identical 3-4-5 vector: dot = 25, mags = sqrt(25) = 5 exactly -> 1.0.
    ("identical_345", &[3.0, 4.0], &[3.0, 4.0], 0x3f80_0000),
    // Swapped components: 24 / 25 = 0.96 (correctly rounded).
    ("swapped_345", &[3.0, 4.0], &[4.0, 3.0], 0x3f75_c28f),
    // Anti-parallel: -25 / 25 = -1.0 exactly.
    ("antiparallel", &[3.0, 4.0], &[-3.0, -4.0], 0xbf80_0000),
    // 1 / sqrt(2): irrational, pins the rounding of the sqrt itself.
    ("inv_sqrt2", &[1.0, 1.0], &[1.0, 0.0], 0x3f35_04f3),
    // Generic 3-dim case: 32 / (sqrt(14) * sqrt(77)).
    ("one_two_three", &[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0], 0x3f79_8178),
    // Embedding-like small floats, mixed signs.
    (
        "embedding_like",
        &[0.5, -0.25, 0.125, 1.0],
        &[0.25, 0.5, -0.125, 0.75],
        0x3f2c_dbcc,
    ),
    // Self-similarity where the magnitude is NOT exactly representable:
    // sqrt(14)^2 != 14 in f32, so the honest answer is 0.99999994, not 1.0.
    ("self_123", &[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0], 0x3f7f_ffff),
];

#[test]
fn cosine_similarity_golden_kats() {
    for (name, a, b, bits) in GOLDEN {
        let score = cosine_similarity(a, b);
        assert_eq!(
            score.to_bits(),
            *bits,
            "KAT '{name}': got {score:?} (0x{:08x}), golden 0x{bits:08x}",
            score.to_bits()
        );
    }
}

#[test]
fn cosine_similarity_degenerate_inputs() {
    // Zero-magnitude vectors score 0.0 (guard, not NaN).
    assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]).to_bits(), 0);
    assert_eq!(cosine_similarity(&[1.0, 1.0], &[0.0, 0.0]).to_bits(), 0);
    // Mismatched dimensions score 0.0.
    assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]).to_bits(), 0);
    // Empty vectors: equal lengths but zero magnitude -> 0.0.
    assert_eq!(cosine_similarity(&[], &[]).to_bits(), 0);
}

/// Independent reference: same sequential fold order, but the square roots
/// use `std`'s `f32::sqrt` (IEEE 754 correctly rounded on the host) instead
/// of `libm::sqrtf`. Bit-equality here proves the libm-unified library path
/// and host-native std math agree exactly — the std-vs-no_std score-identity
/// witness for the kernel arc.
fn ref_cosine_std(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    let mut dot = 0.0f32;
    let mut sa = 0.0f32;
    let mut sb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        sa += a[i] * a[i];
        sb += b[i] * b[i];
    }
    let (mag_a, mag_b) = (sa.sqrt(), sb.sqrt());
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    dot / (mag_a * mag_b)
}

#[test]
fn libm_path_matches_std_math_bit_for_bit() {
    // The golden table first.
    for (name, a, b, _) in GOLDEN {
        assert_eq!(
            cosine_similarity(a, b).to_bits(),
            ref_cosine_std(a, b).to_bits(),
            "std/libm divergence on KAT '{name}'"
        );
    }

    // Then a deterministic pseudo-random sweep: 256 vector pairs, dims 1..=32,
    // components in about [-2.0, 2.0]. Any single divergent bit fails.
    let mut state = 0x2026_0713u32;
    let mut next_f32 = move || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        ((state >> 8) as f32 / (1u32 << 22) as f32) - 2.0
    };
    for case in 0..256 {
        let dim = (case % 32) + 1;
        let a: Vec<f32> = (0..dim).map(|_| next_f32()).collect();
        let b: Vec<f32> = (0..dim).map(|_| next_f32()).collect();
        let lib = cosine_similarity(&a, &b);
        let refv = ref_cosine_std(&a, &b);
        assert_eq!(
            lib.to_bits(),
            refv.to_bits(),
            "std/libm divergence on sweep case {case} (dim {dim}): lib={lib:?} ref={refv:?}"
        );
    }
}

#[test]
fn query_engine_end_to_end_golden_scores() {
    let block_count = 5000;
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");

    let mut fs = UnaFS::format(device, 20).expect("Format failed");
    let root_id = fs.superblock.root_inode;

    // a: inline vector scoring exactly 0.96 against the target [4, 3].
    let a_id = fs.create_file(root_id, "a.vec".to_string()).unwrap();
    fs.set_attribute(
        a_id,
        "embedding".to_string(),
        AttributeValue::Vector(vec![3.0, 4.0]),
    )
    .unwrap();

    // b: anti-parallel to the target — scores -0.96, must be excluded.
    let b_id = fs.create_file(root_id, "b.vec".to_string()).unwrap();
    fs.set_attribute(
        b_id,
        "embedding".to_string(),
        AttributeValue::Vector(vec![-3.0, -4.0]),
    )
    .unwrap();

    // c: dimension mismatch against a 2-dim target — scores 0.0, excluded.
    let c_id = fs.create_file(root_id, "c.vec".to_string()).unwrap();
    fs.set_attribute(
        c_id,
        "embedding".to_string(),
        AttributeValue::Vector(vec![0.5, -0.25, 0.125, 1.0]),
    )
    .unwrap();

    // d: 100 elements — over the 64-float inline threshold, so this vector
    // SPILLS to extents; the query engine must fetch and score it identically.
    let big: Vec<f32> = (0..100).map(|i| i as f32 * 0.1).collect();
    let d_id = fs.create_file(root_id, "d.vec".to_string()).unwrap();
    fs.set_attribute(d_id, "embedding".to_string(), AttributeValue::Vector(big))
        .unwrap();

    // 2-dim target: only `a` clears the 0.5 threshold, with the golden score.
    let results = fs
        .query("similarity(embedding, [4.0, 3.0]) > 0.5")
        .expect("similarity query failed");
    assert_eq!(results.len(), 1, "expected exactly one match");
    assert_eq!(results[0].0.id, a_id);
    assert_eq!(
        results[0].1.to_bits(),
        0x3f75_c28f, // 0.96, the swapped_345 golden
        "end-to-end score diverged from the golden: got {:?}",
        results[0].1
    );

    // Strict `>`: a threshold exactly equal to the score must exclude it.
    let strict = fs
        .query("similarity(embedding, [4.0, 3.0]) > 0.96")
        .expect("strict-threshold query failed");
    assert!(
        strict.iter().all(|(inode, _)| inode.id != a_id),
        "score 0.96 must NOT clear the strict threshold 0.96"
    );

    // Spilled path: self-similarity of the 100-dim vector is the honest
    // 0.99999994 (0x3f7fffff), not 1.0 — magnitude rounding pinned.
    let target: Vec<String> = (0..100).map(|i| format!("{:?}", i as f32 * 0.1)).collect();
    let q = format!("similarity(embedding, [{}]) > 0.9999", target.join(", "));
    let spilled = fs.query(&q).expect("spilled similarity query failed");
    assert_eq!(spilled.len(), 1, "expected exactly the spilled match");
    assert_eq!(spilled[0].0.id, d_id);
    assert_eq!(
        spilled[0].1.to_bits(),
        0x3f7f_ffff,
        "spilled-vector score diverged from the golden: got {:?}",
        spilled[0].1
    );
}
