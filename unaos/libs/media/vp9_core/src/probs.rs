// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Frame contexts (spec §7.3 and §10.5 default probabilities), the symbol counts the tile decode
//! accumulates (§9.3.4), and backward adaptation (§8.4.2 coefficient, §8.4.3 non-coefficient /
//! merge_probs, merge_prob with COUNT_SAT 20 / MAX_UPDATE_FACTOR 128, coefficients 24 / 112 and
//! 128 after a key frame).

use crate::consts::*;
use crate::tables::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MvComponentProbs {
    pub sign: u8,
    pub classes: [u8; 10],
    pub class0: [u8; 1],
    pub bits: [u8; 10],
    pub class0_fp: [[u8; 3]; 2],
    pub fp: [u8; 3],
    pub class0_hp: u8,
    pub hp: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MvProbs {
    pub joints: [u8; 3],
    pub comps: [MvComponentProbs; 2],
}

pub type CoefProbs = [[[[[[u8; 3]; 6]; 6]; 2]; 2]; 4];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameContext {
    pub tx8: [[u8; 1]; 2],
    pub tx16: [[u8; 2]; 2],
    pub tx32: [[u8; 3]; 2],
    pub coef: CoefProbs,
    pub skip: [u8; 3],
    pub inter_mode: [[u8; 3]; 7],
    pub interp_filter: [[u8; 2]; 4],
    pub is_inter: [u8; 4],
    pub comp_mode: [u8; 5],
    pub single_ref: [[u8; 2]; 5],
    pub comp_ref: [u8; 5],
    pub y_mode: [[u8; 9]; 4],
    pub uv_mode: [[u8; 9]; 10],
    pub partition: [[u8; 3]; 16],
    pub mv: MvProbs,
}

impl Default for FrameContext {
    /// setup_past_independence's probability half: every table at its §10.5 default.
    fn default() -> Self {
        let t = &DEFAULT_TX_PROBS;
        let n = &DEFAULT_NMV_CONTEXT;
        let comp = |o: usize| MvComponentProbs {
            sign: n[o],
            classes: n[o + 1..o + 11].try_into().unwrap(),
            class0: [n[o + 11]],
            bits: n[o + 12..o + 22].try_into().unwrap(),
            class0_fp: [[n[o + 22], n[o + 23], n[o + 24]], [n[o + 25], n[o + 26], n[o + 27]]],
            fp: [n[o + 28], n[o + 29], n[o + 30]],
            class0_hp: n[o + 31],
            hp: n[o + 32],
        };
        FrameContext {
            tx32: [[t[0], t[1], t[2]], [t[3], t[4], t[5]]],
            tx16: [[t[6], t[7]], [t[8], t[9]]],
            tx8: [[t[10]], [t[11]]],
            coef: DEFAULT_COEF_PROBS,
            skip: DEFAULT_SKIP_PROBS,
            inter_mode: DEFAULT_INTER_MODE_PROBS,
            interp_filter: DEFAULT_SWITCHABLE_INTERP_PROB,
            is_inter: DEFAULT_INTRA_INTER_P,
            comp_mode: DEFAULT_COMP_INTER_P,
            single_ref: DEFAULT_SINGLE_REF_P,
            comp_ref: DEFAULT_COMP_REF_P,
            y_mode: DEFAULT_IF_Y_PROBS,
            uv_mode: DEFAULT_IF_UV_PROBS,
            partition: DEFAULT_PARTITION_PROBS,
            mv: MvProbs { joints: [n[0], n[1], n[2]], comps: [comp(3), comp(36)] },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MvComponentCounts {
    pub sign: [u32; 2],
    pub classes: [u32; 11],
    pub class0: [u32; 2],
    pub bits: [[u32; 2]; 10],
    pub class0_fp: [[u32; 4]; 2],
    pub fp: [u32; 4],
    pub class0_hp: [u32; 2],
    pub hp: [u32; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MvCounts {
    pub joints: [u32; 4],
    pub comps: [MvComponentCounts; 2],
}

/// Symbol counts for one frame (§9.3.4 "counting process").
#[derive(Clone, Debug)]
pub struct Counts {
    pub y_mode: [[u32; 10]; 4],
    pub uv_mode: [[u32; 10]; 10],
    pub partition: [[u32; 4]; 16],
    pub coef: [[[[[[u32; 4]; 6]; 6]; 2]; 2]; 4],
    pub eob_branch: [[[[[u32; 6]; 6]; 2]; 2]; 4],
    pub interp_filter: [[u32; 3]; 4],
    pub inter_mode: [[u32; 4]; 7],
    pub is_inter: [[u32; 2]; 4],
    pub comp_mode: [[u32; 2]; 5],
    pub single_ref: [[[u32; 2]; 2]; 5],
    pub comp_ref: [[u32; 2]; 5],
    pub tx32: [[u32; 4]; 2],
    pub tx16: [[u32; 3]; 2],
    pub tx8: [[u32; 2]; 2],
    pub skip: [[u32; 2]; 3],
    pub mv: MvCounts,
}

impl Default for Counts {
    fn default() -> Self {
        Counts {
            y_mode: [[0; 10]; 4],
            uv_mode: [[0; 10]; 10],
            partition: [[0; 4]; 16],
            coef: [[[[[[0; 4]; 6]; 6]; 2]; 2]; 4],
            eob_branch: [[[[[0; 6]; 6]; 2]; 2]; 4],
            interp_filter: [[0; 3]; 4],
            inter_mode: [[0; 4]; 7],
            is_inter: [[0; 2]; 4],
            comp_mode: [[0; 2]; 5],
            single_ref: [[[0; 2]; 2]; 5],
            comp_ref: [[0; 2]; 5],
            tx32: [[0; 4]; 2],
            tx16: [[0; 3]; 2],
            tx8: [[0; 2]; 2],
            skip: [[0; 2]; 3],
            mv: MvCounts::default(),
        }
    }
}

impl MvCounts {
    /// Count one MV difference the way the reference decoder does (vp9_inc_mv): high-precision
    /// bits are counted whether or not they were coded (an uncoded hp bit is 1).
    pub fn add(&mut self, diff_row: i32, diff_col: i32) {
        let j = ((diff_col != 0) as usize) | (((diff_row != 0) as usize) << 1);
        self.joints[j] += 1;
        if diff_row != 0 {
            self.comps[0].add(diff_row);
        }
        if diff_col != 0 {
            self.comps[1].add(diff_col);
        }
    }
}

impl MvComponentCounts {
    fn add(&mut self, v: i32) {
        let s = (v < 0) as usize;
        self.sign[s] += 1;
        let z = v.unsigned_abs() as i32 - 1;
        let c = mv_class(z);
        self.classes[c] += 1;
        let o = z - mv_class_base(c);
        let d = (o >> 3) as usize;
        let f = ((o >> 1) & 3) as usize;
        let e = (o & 1) as usize;
        if c == 0 {
            self.class0[d] += 1;
            self.class0_fp[d][f] += 1;
            self.class0_hp[e] += 1;
        } else {
            for i in 0..c {
                self.bits[i][(d >> i) & 1] += 1;
            }
            self.fp[f] += 1;
            self.hp[e] += 1;
        }
    }
}

fn mv_class_base(c: usize) -> i32 {
    if c == 0 { 0 } else { 2 << (c + 2) }
}

/// The MV class of magnitude-minus-one `z` (vp9_get_mv_class: log2 of z >> 3, class 10 from 8192).
pub fn mv_class(z: i32) -> usize {
    if z >= 2 * 4096 {
        return 10;
    }
    let t = (z >> 3) as u32;
    if t == 0 { 0 } else { (31 - t.leading_zeros()) as usize }
}

// ---- adaptation -----------------------------------------------------------------------------------

fn get_prob(num: u32, den: u32) -> u8 {
    let p = ((num as u64 * 256 + (den as u64 >> 1)) / den as u64) as i64;
    p.clamp(1, 255) as u8
}

fn get_binary_prob(n0: u32, n1: u32) -> u8 {
    let den = n0 + n1;
    if den == 0 { 128 } else { get_prob(n0, den) }
}

fn weighted_prob(p1: i32, p2: i32, factor: i32) -> u8 {
    ((p1 * (256 - factor) + p2 * factor + 128) >> 8) as u8
}

fn merge_probs(pre: u8, ct: [u32; 2], count_sat: u32, max_update_factor: u32) -> u8 {
    let prob = get_binary_prob(ct[0], ct[1]);
    let count = (ct[0] + ct[1]).min(count_sat);
    let factor = max_update_factor * count / count_sat;
    weighted_prob(pre as i32, prob as i32, factor as i32)
}

const COUNT_TO_UPDATE_FACTOR: [i32; 21] = [0, 6, 12, 19, 25, 32, 38, 44, 51, 57, 64, 70, 76, 83, 89, 96, 102, 108, 115, 121, 128];

/// merge_prob for mode and MV probabilities (MODE_MV_COUNT_SAT 20, MODE_MV_MAX_UPDATE_FACTOR 128).
pub fn mode_mv_merge(pre: u8, ct: [u32; 2]) -> u8 {
    let den = ct[0] + ct[1];
    if den == 0 {
        return pre;
    }
    let count = den.min(20) as usize;
    let prob = get_prob(ct[0], den);
    weighted_prob(pre as i32, prob as i32, COUNT_TO_UPDATE_FACTOR[count])
}

/// merge_probs over a tree: each node's probability adapts from the counts of its two subtrees.
pub fn tree_merge(tree: &[i8], pre: &[u8], counts: &[u32], out: &mut [u8]) {
    fn walk(i: usize, tree: &[i8], pre: &[u8], counts: &[u32], out: &mut [u8]) -> u32 {
        let l = tree[i];
        let lc = if l <= 0 { counts[(-l) as usize] } else { walk(l as usize, tree, pre, counts, out) };
        let r = tree[i + 1];
        let rc = if r <= 0 { counts[(-r) as usize] } else { walk(r as usize, tree, pre, counts, out) };
        out[i >> 1] = mode_mv_merge(pre[i >> 1], [lc, rc]);
        lc + rc
    }
    walk(0, tree, pre, counts, out);
}

/// §8.4.2: coefficient probability adaptation. `intra` = this frame is intra-only (key or
/// intra_only); `last_key` = the previous decoded frame was a key frame.
pub fn adapt_coef(fc: &mut FrameContext, pre: &FrameContext, counts: &Counts, intra: bool, last_key: bool) {
    let (count_sat, update_factor) = if intra {
        (24, 112)
    } else if last_key {
        (24, 128)
    } else {
        (24, 112)
    };
    for t in 0..4 {
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..6 {
                    let nctx = if k == 0 { 3 } else { 6 };
                    for l in 0..nctx {
                        let c = &counts.coef[t][i][j][k][l];
                        let (n0, n1, n2, neob) = (c[0], c[1], c[2], c[3]);
                        let eob = counts.eob_branch[t][i][j][k][l];
                        let branch = [[neob, eob.wrapping_sub(neob)], [n0, n1 + n2], [n1, n2]];
                        for m in 0..3 {
                            fc.coef[t][i][j][k][l][m] = merge_probs(pre.coef[t][i][j][k][l][m], branch[m], count_sat, update_factor);
                        }
                    }
                }
            }
        }
    }
}

/// §8.4.3: mode probability adaptation (inter frames only).
pub fn adapt_mode(fc: &mut FrameContext, pre: &FrameContext, counts: &Counts, interp_switchable: bool, tx_select: bool) {
    for i in 0..4 {
        fc.is_inter[i] = mode_mv_merge(pre.is_inter[i], counts.is_inter[i]);
    }
    for i in 0..5 {
        fc.comp_mode[i] = mode_mv_merge(pre.comp_mode[i], counts.comp_mode[i]);
        fc.comp_ref[i] = mode_mv_merge(pre.comp_ref[i], counts.comp_ref[i]);
        for j in 0..2 {
            fc.single_ref[i][j] = mode_mv_merge(pre.single_ref[i][j], counts.single_ref[i][j]);
        }
    }
    for i in 0..7 {
        tree_merge(&INTER_MODE_TREE, &pre.inter_mode[i], &counts.inter_mode[i], &mut fc.inter_mode[i]);
    }
    for i in 0..4 {
        tree_merge(&INTRA_MODE_TREE, &pre.y_mode[i], &counts.y_mode[i], &mut fc.y_mode[i]);
    }
    for i in 0..10 {
        tree_merge(&INTRA_MODE_TREE, &pre.uv_mode[i], &counts.uv_mode[i], &mut fc.uv_mode[i]);
    }
    for i in 0..16 {
        tree_merge(&PARTITION_TREE, &pre.partition[i], &counts.partition[i], &mut fc.partition[i]);
    }
    if interp_switchable {
        for i in 0..4 {
            tree_merge(&SWITCHABLE_INTERP_TREE, &pre.interp_filter[i], &counts.interp_filter[i], &mut fc.interp_filter[i]);
        }
    }
    if tx_select {
        for i in 0..2 {
            let c8 = &counts.tx8[i];
            fc.tx8[i][0] = mode_mv_merge(pre.tx8[i][0], [c8[0], c8[1]]);
            let c16 = &counts.tx16[i];
            let b16 = [[c16[0], c16[1] + c16[2]], [c16[1], c16[2]]];
            for j in 0..2 {
                fc.tx16[i][j] = mode_mv_merge(pre.tx16[i][j], b16[j]);
            }
            let c32 = &counts.tx32[i];
            let b32 = [[c32[0], c32[1] + c32[2] + c32[3]], [c32[1], c32[2] + c32[3]], [c32[2], c32[3]]];
            for j in 0..3 {
                fc.tx32[i][j] = mode_mv_merge(pre.tx32[i][j], b32[j]);
            }
        }
    }
    for i in 0..3 {
        fc.skip[i] = mode_mv_merge(pre.skip[i], counts.skip[i]);
    }
}

/// §8.4.4: MV probability adaptation.
pub fn adapt_mv(fc: &mut FrameContext, pre: &FrameContext, counts: &Counts, allow_hp: bool) {
    tree_merge(&MV_JOINT_TREE, &pre.mv.joints, &counts.mv.joints, &mut fc.mv.joints);
    for i in 0..2 {
        let p = &pre.mv.comps[i];
        let c = &counts.mv.comps[i];
        let o = &mut fc.mv.comps[i];
        o.sign = mode_mv_merge(p.sign, c.sign);
        tree_merge(&MV_CLASS_TREE, &p.classes, &c.classes, &mut o.classes);
        tree_merge(&MV_CLASS0_TREE, &p.class0, &c.class0, &mut o.class0);
        for j in 0..10 {
            o.bits[j] = mode_mv_merge(p.bits[j], c.bits[j]);
        }
        for j in 0..2 {
            tree_merge(&MV_FP_TREE, &p.class0_fp[j], &c.class0_fp[j], &mut o.class0_fp[j]);
        }
        tree_merge(&MV_FP_TREE, &p.fp, &c.fp, &mut o.fp);
        if allow_hp {
            o.class0_hp = mode_mv_merge(p.class0_hp, c.class0_hp);
            o.hp = mode_mv_merge(p.hp, c.hp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_land_where_the_spec_prints_them() {
        let fc = FrameContext::default();
        assert_eq!(fc.tx8, [[100], [66]]);
        assert_eq!(fc.tx16, [[20, 152], [15, 101]]);
        assert_eq!(fc.tx32, [[3, 136, 37], [5, 52, 13]]);
        assert_eq!(fc.mv.joints, [32, 64, 96]);
        assert_eq!(fc.mv.comps[0].classes[0], 224);
        assert_eq!(fc.mv.comps[1].classes[0], 216);
        assert_eq!(fc.mv.comps[0].class0_hp, 160);
        assert_eq!(fc.mv.comps[1].hp, 128);
        assert_eq!(fc.coef[0][0][0][0][0], [195, 29, 183]);
    }

    #[test]
    fn mv_classes() {
        assert_eq!(mv_class(0), 0);
        assert_eq!(mv_class(15), 0);
        assert_eq!(mv_class(16), 1);
        assert_eq!(mv_class(8191), 9);
        assert_eq!(mv_class(8192), 10);
    }

    #[test]
    fn merge_without_counts_keeps_the_prior() {
        assert_eq!(mode_mv_merge(77, [0, 0]), 77);
        // 20+ observations of a zero move the probability all the way (factor 128 = half way).
        assert_eq!(mode_mv_merge(128, [40, 0]), weighted_prob(128, 255, 128));
    }
}
