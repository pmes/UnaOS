//! Known-answer tests for geom_core's shared dense kernels (VUGGEOMLINALG, LEDGER SR520).

use geom_core::linalg::{SplitMix64, cholesky_profile, cholesky_solve, cholesky_substitute, jacobi_sym};

fn col(v: &[f64], n: usize, j: usize) -> Vec<f64> {
    (0..n).map(|r| v[r * n + j]).collect()
}

/// Equal up to an overall sign.
fn same_dir(a: &[f64], b: &[f64], eps: f64) -> bool {
    let d: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let s = if d < 0.0 { -1.0 } else { 1.0 };
    a.iter().zip(b).all(|(x, y)| (x - s * y).abs() <= eps)
}

#[test]
fn jacobi_3x3_known_eigenpairs() {
    let a = [2.0, -1.0, 0.0, -1.0, 2.0, -1.0, 0.0, -1.0, 2.0];
    let (vals, vecs) = jacobi_sym(&a, 3);
    let r2 = core::f64::consts::SQRT_2;
    let want = [2.0 - r2, 2.0, 2.0 + r2];
    for i in 0..3 {
        assert!((vals[i] - want[i]).abs() <= 1e-12, "value {i}: {} vs {}", vals[i], want[i]);
    }
    let wv = [[0.5, r2 / 2.0, 0.5], [r2 / 2.0, 0.0, -r2 / 2.0], [0.5, -r2 / 2.0, 0.5]];
    for j in 0..3 {
        assert!(same_dir(&col(&vecs, 3, j), &wv[j], 1e-12), "vector {j}: {:?}", col(&vecs, 3, j));
    }
    // A short slice is a named empty answer, never a panic.
    let (v0, e0) = jacobi_sym(&a[..8], 3);
    assert!(v0.is_empty() && e0.is_empty());
}

#[test]
fn jacobi_9x9_dlt_shape_reconstructs() {
    // The normalised-DLT shape (photo_core homography.rs `dlt`): two rows of A per exact correspondence of a known H.
    let h = [[1.1, 0.2, 0.1], [-0.1, 0.9, -0.2], [0.05, -0.03, 1.0]];
    let paper = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (0.0, 0.5), (0.7, -0.3)];
    let mut ata = [0.0f64; 81];
    for &(xx, yy) in &paper {
        let w = h[2][0] * xx + h[2][1] * yy + h[2][2];
        let u = (h[0][0] * xx + h[0][1] * yy + h[0][2]) / w;
        let v = (h[1][0] * xx + h[1][1] * yy + h[1][2]) / w;
        let r1 = [0.0, 0.0, 0.0, -xx, -yy, -1.0, v * xx, v * yy, v];
        let r2 = [xx, yy, 1.0, 0.0, 0.0, 0.0, -u * xx, -u * yy, -u];
        for r in [r1, r2] {
            for i in 0..9 {
                for j in 0..9 {
                    ata[i * 9 + j] += r[i] * r[j];
                }
            }
        }
    }
    let (vals, vecs) = jacobi_sym(&ata, 9);
    assert_eq!(vals.len(), 9);
    for i in 1..9 {
        assert!(vals[i - 1] <= vals[i], "ascending: {vals:?}");
    }
    // V orthonormal.
    for i in 0..9 {
        for j in 0..9 {
            let d: f64 = (0..9).map(|k| vecs[k * 9 + i] * vecs[k * 9 + j]).sum();
            let want = if i == j { 1.0 } else { 0.0 };
            assert!((d - want).abs() <= 1e-12, "VᵀV[{i}][{j}] = {d}");
        }
    }
    // V diag(λ) Vᵀ = AᵀA.
    let big = ata.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    for i in 0..9 {
        for j in 0..9 {
            let r: f64 = (0..9).map(|k| vecs[i * 9 + k] * vals[k] * vecs[j * 9 + k]).sum();
            assert!((r - ata[i * 9 + j]).abs() <= 1e-12 * big, "reconstruct [{i}][{j}]: {r} vs {}", ata[i * 9 + j]);
        }
    }
    // The null vector is H.
    assert!(vals[0].abs() <= 1e-12 * vals[8], "λ0 = {} of λ8 = {}", vals[0], vals[8]);
    let c0 = col(&vecs, 9, 0);
    for i in 0..9 {
        let got = c0[i] / c0[8];
        assert!((got - h[i / 3][i % 3]).abs() <= 1e-9, "h[{i}] = {got}");
    }
}

#[test]
fn splitmix64_reference_words() {
    // Vigna's splitmix64.c from state 0: the first ten outputs.
    let want: [u64; 10] = [
        0xE220_A839_7B1D_CDAF,
        0x6E78_9E6A_A1B9_65F4,
        0x06C4_5D18_8009_454F,
        0xF88B_B8A8_724C_81EC,
        0x1B39_896A_51A8_749B,
        0x53CB_9F0C_747E_A2EA,
        0x2C82_9ABE_1F45_32E1,
        0xC584_133A_C916_AB3C,
        0x3EE5_7890_41C9_8AC3,
        0xF3B8_488C_368C_B0A6,
    ];
    let mut r = SplitMix64::new(0);
    for (i, &w) in want.iter().enumerate() {
        assert_eq!(r.next_u64(), w, "word {i}");
    }
    let mut r = SplitMix64::new(0x9A9E_4D06_0503_0001);
    for _ in 0..1000 {
        let x = r.next_f64();
        assert!((0.0..1.0).contains(&x));
        assert!(r.below(7) < 7);
    }
    let mut a = SplitMix64::new(42);
    let mut b = SplitMix64::new(42);
    assert_eq!(a.below(0), 0);
    assert_eq!(a.next_u64(), b.next_u64(), "below(0) draws nothing");
}

#[test]
fn cholesky_solves_spd_and_refuses() {
    // a = [[4,2,0],[2,5,1],[0,1,3]], x = [1, -2, 3]: b = a x.
    let a0 = [4.0, 2.0, 0.0, 2.0, 5.0, 1.0, 0.0, 1.0, 3.0];
    let x = [1.0, -2.0, 3.0];
    let mut b: Vec<f64> = (0..3).map(|i| (0..3).map(|k| a0[i * 3 + k] * x[k]).sum::<f64>()).collect();
    let mut a = a0;
    assert!(cholesky_solve(&mut a, 3, &mut b));
    for i in 0..3 {
        assert!((b[i] - x[i]).abs() <= 1e-12, "x[{i}] = {}", b[i]);
    }
    let mut ind = [1.0, 2.0, 2.0, 1.0];
    let mut bb = [1.0, 1.0];
    assert!(!cholesky_solve(&mut ind, 2, &mut bb), "indefinite");
    let mut short = [1.0, 0.0, 0.0];
    let mut bs = [1.0, 1.0];
    assert!(!cholesky_solve(&mut short, 2, &mut bs), "short slice");
}

/// VUGGEOMLINALGSCAN (SR540): the envelope Cholesky (SR522's `factor` / `substitute`, moved from scan_core graph.rs).
/// NaN sentinels left of the envelope are never read (x is exact) and never written (still NaN); the all-zero profile
/// is `cholesky_solve` bit for bit; the refusals are named `false`s.
#[test]
fn cholesky_profile_keeps_the_envelope() {
    const M: usize = 6;
    let first = [0usize, 0, 1, 1, 3, 2];
    // A diagonally dominant SPD over the envelope: row i is non-zero only in columns first[i] ..= i (and by symmetry).
    let mut a0 = [0.0f64; M * M];
    for i in 0..M {
        for k in first[i]..i {
            let v = 0.5 + 0.25 * (i + k) as f64;
            a0[i * M + k] = v;
            a0[k * M + i] = v;
        }
    }
    for i in 0..M {
        let off: f64 = (0..M).filter(|&k| k != i).map(|k| a0[i * M + k]).sum();
        a0[i * M + i] = off + 2.0 + i as f64;
    }
    let x = [1.0, -2.0, 3.0, -0.5, 0.25, 4.0];
    let b0: Vec<f64> = (0..M).map(|i| (0..M).map(|k| a0[i * M + k] * x[k]).sum::<f64>()).collect();

    // The envelope road with NaN left of the envelope in the lower triangle.
    let mut a = a0;
    for i in 0..M {
        for k in 0..first[i] {
            a[i * M + k] = f64::NAN;
        }
    }
    let mut b = b0.clone();
    assert!(cholesky_profile(&mut a, M, &first));
    assert!(cholesky_substitute(&a, M, &first, &mut b));
    for i in 0..M {
        assert!((b[i] - x[i]).abs() <= 1e-12, "x[{i}] = {}", b[i]);
        for k in 0..first[i] {
            assert!(a[i * M + k].is_nan(), "a[{i}][{k}] outside the envelope was written");
        }
    }

    // The all-zero profile is the dense solve, bit for bit.
    let (mut ad, mut bd) = (a0, b0.clone());
    assert!(cholesky_solve(&mut ad, M, &mut bd));
    let (mut ap, mut bp) = (a0, b0.clone());
    let zero = [0usize; M];
    assert!(cholesky_profile(&mut ap, M, &zero));
    assert!(cholesky_substitute(&ap, M, &zero, &mut bp));
    for i in 0..M {
        assert_eq!(bd[i].to_bits(), bp[i].to_bits(), "x[{i}]");
    }
    for k in 0..M * M {
        assert_eq!(ad[k].to_bits(), ap[k].to_bits(), "L[{k}]");
    }

    // Refusals: short a / first / b, an indefinite pivot.
    let mut short = [1.0, 0.0, 0.0];
    assert!(!cholesky_profile(&mut short, 2, &[0, 0]), "short a");
    let mut a2 = [4.0, 0.0, 0.0, 4.0];
    assert!(!cholesky_profile(&mut a2, 2, &[0]), "short first");
    let mut b1 = [1.0];
    assert!(!cholesky_substitute(&a2, 2, &[0, 0], &mut b1), "short b");
    let mut ind = [1.0, 2.0, 2.0, 1.0];
    assert!(!cholesky_profile(&mut ind, 2, &[0, 0]), "indefinite");
}
