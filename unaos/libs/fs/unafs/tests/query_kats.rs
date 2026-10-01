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
    assert_eq!(results[0].inode_id, a_id);
    assert_eq!(
        results[0].score.to_bits(),
        0x3f75_c28f, // 0.96, the swapped_345 golden
        "end-to-end score diverged from the golden: got {:?}",
        results[0].score
    );

    // Strict `>`: a threshold exactly equal to the score must exclude it.
    let strict = fs
        .query("similarity(embedding, [4.0, 3.0]) > 0.96")
        .expect("strict-threshold query failed");
    assert!(
        strict.iter().all(|h| h.inode_id != a_id),
        "score 0.96 must NOT clear the strict threshold 0.96"
    );

    // Spilled path: self-similarity of the 100-dim vector is the honest
    // 0.99999994 (0x3f7fffff), not 1.0 — magnitude rounding pinned.
    let target: Vec<String> = (0..100).map(|i| format!("{:?}", i as f32 * 0.1)).collect();
    let q = format!("similarity(embedding, [{}]) > 0.9999", target.join(", "));
    let spilled = fs.query(&q).expect("spilled similarity query failed");
    assert_eq!(spilled.len(), 1, "expected exactly the spilled match");
    assert_eq!(spilled[0].inode_id, d_id);
    assert_eq!(
        spilled[0].score.to_bits(),
        0x3f7f_ffff,
        "spilled-vector score diverged from the golden: got {:?}",
        spilled[0].score
    );
}

// =============================================================================
// B302 M2 — F4: the ordered operators, ranges, OR/parentheses, typed operands.
// Every new operator has a known-answer case here, and every case is asserted
// on a v6 volume (B+tree candidates) AND a v5 volume (flat-list candidates):
// the index may only change the COST of an answer, never the answer.
// =============================================================================

use unafs::{Expr, Query, QueryOp};

/// The fixture: eight named files with typed attributes. Returns (fs, ids by
/// name in creation order).
fn kat_fixture(version: u32) -> (UnaFS<MemDevice>, Vec<u64>) {
    let mut dev = MemDevice::new();
    dev.write_block(8191, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    let mut fs = UnaFS::format_with_version(dev, 0, version).unwrap();
    let root = fs.superblock.root_inode;
    let dir = fs.mkdir(root, "kat".into()).unwrap();
    use AttributeValue::{Float, Int, String as S};
    let long_a = format!("{}A", "p".repeat(100)); // past the 96 B key cap…
    let long_b = format!("{}B", "p".repeat(100)); // …sharing the capped prefix
    let rows: Vec<(&str, Vec<(&str, AttributeValue)>)> = vec![
        ("f0", vec![("n", Int(-5)), ("kind", S("doc".into())), ("rating", Float(1.5))]),
        ("f1", vec![("n", Int(0)), ("kind", S("memo".into())), ("rating", Int(4))]),
        ("f2", vec![("n", Int(3)), ("kind", S("doc".into())), ("rating", Float(4.0))]),
        ("f3", vec![("n", Int(7)), ("kind", S("img".into())), ("rating", Float(4.5))]),
        ("f4", vec![("n", Int(10)), ("kind", S("doc".into())), ("rating", Float(-0.0))]),
        ("f5", vec![("n", Int(i64::MAX)), ("tag", S(long_a))]),
        ("f6", vec![("n", Int(i64::MIN)), ("tag", S(long_b))]),
        ("f7", vec![("kind", S("a\0b".into())), ("rating", Float(f64::NAN))]),
    ];
    let mut ids = Vec::new();
    for (name, attrs) in rows {
        let id = fs.create_file(dir, name.into()).unwrap();
        for (k, v) in attrs {
            fs.set_attribute(id, k.into(), v).unwrap();
        }
        ids.push(id);
    }
    (fs, ids)
}

/// (query, expected file indexes in ascending id order)
const ORDER_KATS: &[(&str, &[usize])] = &[
    // Single-bound ordering, both strict and inclusive, signed.
    ("n > 3", &[3, 4, 5]),
    ("n >= 3", &[2, 3, 4, 5]),
    ("n < 0", &[0, 6]),
    ("n <= 0", &[0, 1, 6]),
    ("n >= -9223372036854775808", &[0, 1, 2, 3, 4, 5, 6]),
    // Two-sided ranges: BETWEEN (inclusive) and the chained forms.
    ("n BETWEEN 0 AND 7", &[1, 2, 3]),
    ("n between 0 and 7", &[1, 2, 3]),
    ("0 <= n <= 7", &[1, 2, 3]),
    ("0 < n < 7", &[2]),
    ("0 < n <= 7", &[2, 3]),
    ("7 >= n >= 0", &[1, 2, 3]),
    ("7 > n > 0", &[2]),
    ("n BETWEEN 7 AND 0", &[]),
    // Typed: Int and Float order numerically across the two types…
    ("rating >= 4", &[1, 2, 3]),
    ("rating > 4", &[3]),
    ("rating BETWEEN 1 AND 4.0", &[0, 1, 2]),
    ("rating < 0.5", &[4]),
    ("rating <= -0.0", &[4]),
    // …but equality stays typed (value hash): Int(4) is not Float(4.0).
    ("rating == 4", &[1]),
    ("rating == 4.0", &[2]),
    // NaN never orders and never equals.
    ("rating > -1000000", &[0, 1, 2, 3, 4]),
    // Strings order bytewise; NUL is a byte like any other.
    ("kind >= \"doc\" AND kind < \"img\"", &[0, 2, 4]),
    ("kind > \"a\"", &[0, 1, 2, 3, 4, 7]),
    ("kind < \"a\\\\\"", &[7]),
    ("kind BETWEEN \"e\" AND \"j\"", &[3]),
    // Ordering against an unordered type matches nothing.
    ("n > [1.0]", &[]),
    ("kind > 3", &[]),
    // != needs the key present.
    ("kind != \"doc\"", &[1, 3, 7]),
    // OR, AND, parentheses, precedence (AND binds tighter).
    ("kind == \"img\" OR n == 0", &[1, 3]),
    ("(kind == \"doc\" OR kind == \"memo\") AND n >= 0", &[1, 2, 4]),
    ("kind == \"doc\" OR kind == \"memo\" AND n >= 3", &[0, 2, 4]),
    ("((n < 0) OR (n > 7)) AND kind == \"doc\"", &[0, 4]),
    ("n == 3 AND rating == 4.0 AND kind == \"doc\"", &[2]),
    ("missing == 1 OR n == 10", &[4]),
];

#[test]
fn order_range_and_boolean_kats_v6_and_v5_agree() {
    for version in [6u32, 5] {
        let (mut fs, ids) = kat_fixture(version);
        for (q, want) in ORDER_KATS {
            let got: Vec<u64> = fs
                .query(q)
                .unwrap_or_else(|e| panic!("v{version} '{q}': {e:?}"))
                .into_iter()
                .map(|h| h.inode_id)
                .collect();
            let want: Vec<u64> = want.iter().map(|&i| ids[i]).collect();
            assert_eq!(got, want, "v{version} KAT '{q}'");
        }
        // Past the 96 B key cap the two `tag`s tie in the index (same capped
        // prefix, spill marker); the verifier separates them.
        let mid = format!("{}A", "p".repeat(100));
        let ids_of = |fs: &mut UnaFS<MemDevice>, q: String| -> Vec<u64> {
            fs.query(&q).unwrap().into_iter().map(|h| h.inode_id).collect()
        };
        assert_eq!(ids_of(&mut fs, format!("tag > \"{mid}\"")), vec![ids[6]], "v{version} spill >");
        assert_eq!(ids_of(&mut fs, format!("tag <= \"{mid}\"")), vec![ids[5]], "v{version} spill <=");
        assert_eq!(ids_of(&mut fs, format!("tag == \"{mid}\"")), vec![ids[5]], "v{version} spill ==");
        assert_eq!(ids_of(&mut fs, format!("tag >= \"{}\"", "p".repeat(96))).len(), 2, "v{version} cap tie");
    }
}

#[test]
fn boolean_scores_and_paths() {
    let (mut fs, ids) = kat_fixture(6);
    let a = ids[1];
    fs.set_attribute(a, "embedding".into(), AttributeValue::Vector(vec![3.0, 4.0])).unwrap();
    // AND multiplies: a lone similarity score survives `x · 1.0` bit-exactly.
    let hits = fs
        .query("similarity(embedding, [4.0, 3.0]) > 0.5 AND (kind == \"memo\" OR kind == \"doc\")")
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].score.to_bits(), 0x3f75_c28f);
    assert_eq!(hits[0].path, "/kat/f1");
    // OR takes the max of the matching branches.
    let hits = fs.query("similarity(embedding, [4.0, 3.0]) > 0.5 OR n == 0").unwrap();
    assert_eq!(hits[0].score, 1.0);
}

#[test]
fn grammar_parses_and_refuses() {
    let q = Query::parse("(a == 1 OR b >= 2.5) AND 1 <= c < 9").unwrap();
    match &q.expr {
        Expr::And(v) => {
            assert!(matches!(&v[0], Expr::Or(o) if o.len() == 2));
            match &v[1] {
                Expr::Pred(p) => {
                    assert_eq!(p.key, "c");
                    assert_eq!(p.op, QueryOp::Range { lo_inclusive: true, hi_inclusive: false });
                    assert_eq!(p.value, AttributeValue::Int(1));
                    assert_eq!(p.value_hi, Some(AttributeValue::Int(9)));
                }
                e => panic!("{e:?}"),
            }
        }
        e => panic!("{e:?}"),
    }
    for bad in [
        "",
        "a ==",
        "(a == 1",
        "a == 1)",
        "a = 1",
        "a == 1 AND",
        "a BETWEEN 1 2",
        "1 < a > 2",
        "a == \"unterminated",
        "similarity(e, \"x\") > 0.5",
    ] {
        assert!(Query::parse(bad).is_err(), "must refuse {bad:?}");
    }
    let deep = format!("{}a == 1{}", "(".repeat(40), ")".repeat(40));
    assert!(Query::parse(&deep).is_err(), "nesting bomb refused");
    assert!(Query::parse(&"a == 1 OR ".repeat(30_000)).is_err(), "length bound");
    // A real embedding query (384 dims inline) is well inside the bound.
    let emb: Vec<String> = (0..384).map(|i| format!("{:?}", i as f32 * -0.001)).collect();
    assert!(Query::parse(&format!("similarity(e, [{}]) > 0.5 AND type == \"engram\"", emb.join(", "))).is_ok());
}

/// Differential sweep: 400 objects with pseudo-random Int/Float/String values
/// under one key, 300 pseudo-random range queries, each answered by the
/// B+tree planner and checked against a brute-force evaluation of the same
/// predicate over the ground-truth values.
#[test]
fn ordered_index_matches_brute_force() {
    let mut dev = MemDevice::new();
    dev.write_block(16383, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    let mut fs = UnaFS::format(dev, 0).unwrap();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut truth: Vec<(u64, AttributeValue)> = Vec::new();
    for i in 0..400 {
        let v = match i % 3 {
            0 => AttributeValue::Int((rnd() % 200) as i64 - 100),
            1 => AttributeValue::Float(((rnd() % 4000) as f64 - 2000.0) / 19.0),
            _ => AttributeValue::String(format!("{:x}", rnd() % 4096)),
        };
        let id = fs.create_inode(Default::default()).unwrap();
        fs.set_attribute(id, "v".into(), v.clone()).unwrap();
        truth.push((id, v));
    }
    let num = |r: u64| -> String {
        if r % 2 == 0 {
            format!("{}", (r % 220) as i64 - 110)
        } else {
            format!("{:.3}", ((r % 4400) as f64 - 2200.0) / 19.0)
        }
    };
    for i in 0..300 {
        let (a, b) = (rnd(), rnd());
        let q = match i % 6 {
            0 => format!("v > {}", num(a)),
            1 => format!("v <= {}", num(a)),
            2 => format!("v BETWEEN {} AND {}", num(a), num(b)),
            3 => format!("{} < v < {}", num(a), num(b)),
            4 => format!("v >= \"{:x}\"", a % 4096),
            _ => format!("\"{:x}\" <= v <= \"{:x}\"", a % 4096, b % 4096),
        };
        let parsed = Query::parse(&q).unwrap();
        let want: Vec<u64> = truth
            .iter()
            .filter(|(_, v)| parsed.expr.eval(&mut |_k: &str| Some(v.clone())).is_some())
            .map(|(id, _)| *id)
            .collect();
        let got: Vec<u64> = fs.query(&q).unwrap().into_iter().map(|h| h.inode_id).collect();
        assert_eq!(got, want, "differential '{q}'");
    }
}
