// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The constant tables of ITU-T Rec. H.265, transcribed from the Recommendation; each block names its table.
//! No reference decoder's source was read for this.

/// One syntax element's initValue table (9.3.2.2): `slot` is its first ctxIdx in hevc_core's layout (the crate doc),
/// `init[t]` its initValues for initType `t` (Table 9-4: 0 = I, 1 and 2 = P / B by cabac_init_flag). An empty row is an
/// initType the element is never decoded in (it is initialised with 154, the equiprobable state).
pub struct CtxTable {
    pub name: &'static str,
    pub slot: usize,
    pub init: [&'static [u8]; 3],
}

impl CtxTable {
    /// The number of contexts the element has (per initType).
    pub fn len(&self) -> usize {
        self.init.iter().map(|r| r.len()).max().unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The equiprobable initValue (slope 0 x qp, offset giving preCtxState 64): pStateIdx 0, valMps 1.
pub const CNU: u8 = 154;

// The slots of hevc_core's ctxIdx layout (see the crate doc's table).
pub const SAO_MERGE: usize = 0;
pub const SAO_TYPE: usize = 1;
pub const SPLIT_CU: usize = 2;
pub const TRANSQUANT_BYPASS: usize = 5;
pub const CU_SKIP: usize = 6;
pub const PRED_MODE: usize = 9;
pub const PART_MODE: usize = 10;
pub const PREV_INTRA_LUMA: usize = 14;
pub const INTRA_CHROMA: usize = 15;
pub const RQT_ROOT_CBF: usize = 16;
pub const MERGE_FLAG: usize = 17;
pub const MERGE_IDX: usize = 18;
pub const INTER_PRED_IDC: usize = 19;
pub const REF_IDX: usize = 24;
pub const MVP_FLAG: usize = 26;
pub const SPLIT_TRANSFORM: usize = 27;
pub const CBF_LUMA: usize = 30;
pub const CBF_CHROMA: usize = 32;
pub const ABS_MVD_GT0: usize = 37;
pub const ABS_MVD_GT1: usize = 38;
pub const CU_QP_DELTA_ABS: usize = 39;
pub const TRANSFORM_SKIP: usize = 41;
pub const LAST_X: usize = 43;
pub const LAST_Y: usize = 61;
pub const CSBF: usize = 79;
pub const SIG: usize = 83;
pub const GT1: usize = 127;
pub const GT2: usize = 151;
/// One past the last slot hevc_core uses; slots from here to `h264_core::cabac::NUM_CTX` are free.
pub const CTX_END: usize = 157;

const LAST_I: &[u8] = &[110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143, 127, 111, 79, 108, 123, 63];
const LAST_P: &[u8] = &[125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95, 94, 108, 123, 108];
const LAST_B: &[u8] = &[125, 110, 124, 110, 95, 94, 125, 111, 111, 79, 125, 126, 111, 111, 79, 108, 123, 93];

/// Every context table of 9.3.2.2 (Tables 9-5 .. 9-37 of the 2013+ numbering), in Table 9-4's order.
pub const CTX_TABLES: &[CtxTable] = &[
    // Table 9-5 sao_merge_left_flag and sao_merge_up_flag.
    CtxTable { name: "sao_merge_flag", slot: SAO_MERGE, init: [&[153], &[153], &[153]] },
    // Table 9-6 sao_type_idx_luma and sao_type_idx_chroma.
    CtxTable { name: "sao_type_idx", slot: SAO_TYPE, init: [&[200], &[185], &[160]] },
    // Table 9-7 split_cu_flag.
    CtxTable { name: "split_cu_flag", slot: SPLIT_CU, init: [&[139, 141, 157], &[107, 139, 126], &[107, 139, 126]] },
    // Table 9-8 cu_transquant_bypass_flag.
    CtxTable { name: "cu_transquant_bypass_flag", slot: TRANSQUANT_BYPASS, init: [&[154], &[154], &[154]] },
    // Table 9-9 cu_skip_flag.
    CtxTable { name: "cu_skip_flag", slot: CU_SKIP, init: [&[], &[197, 185, 201], &[197, 185, 201]] },
    // Table 9-10 pred_mode_flag.
    CtxTable { name: "pred_mode_flag", slot: PRED_MODE, init: [&[], &[149], &[134]] },
    // Table 9-11 part_mode.
    CtxTable { name: "part_mode", slot: PART_MODE, init: [&[184], &[154, 139, 154, 154], &[154, 139, 154, 154]] },
    // Table 9-12 prev_intra_luma_pred_flag.
    CtxTable { name: "prev_intra_luma_pred_flag", slot: PREV_INTRA_LUMA, init: [&[184], &[154], &[183]] },
    // Table 9-13 intra_chroma_pred_mode.
    CtxTable { name: "intra_chroma_pred_mode", slot: INTRA_CHROMA, init: [&[63], &[152], &[152]] },
    // Table 9-14 rqt_root_cbf.
    CtxTable { name: "rqt_root_cbf", slot: RQT_ROOT_CBF, init: [&[], &[79], &[79]] },
    // Table 9-15 merge_flag.
    CtxTable { name: "merge_flag", slot: MERGE_FLAG, init: [&[], &[110], &[154]] },
    // Table 9-16 merge_idx.
    CtxTable { name: "merge_idx", slot: MERGE_IDX, init: [&[], &[122], &[137]] },
    // Table 9-17 inter_pred_idc.
    CtxTable { name: "inter_pred_idc", slot: INTER_PRED_IDC, init: [&[], &[95, 79, 63, 31, 31], &[95, 79, 63, 31, 31]] },
    // Table 9-18 ref_idx_l0 and ref_idx_l1.
    CtxTable { name: "ref_idx", slot: REF_IDX, init: [&[], &[153, 153], &[153, 153]] },
    // Table 9-19 mvp_l0_flag and mvp_l1_flag.
    CtxTable { name: "mvp_flag", slot: MVP_FLAG, init: [&[], &[168], &[168]] },
    // Table 9-20 split_transform_flag.
    CtxTable { name: "split_transform_flag", slot: SPLIT_TRANSFORM, init: [&[153, 138, 138], &[124, 138, 94], &[224, 167, 122]] },
    // Table 9-21 cbf_luma.
    CtxTable { name: "cbf_luma", slot: CBF_LUMA, init: [&[111, 141], &[153, 111], &[153, 111]] },
    // Table 9-22 cbf_cb and cbf_cr (the fifth context is the range extension's, ChromaArrayType 2).
    CtxTable {
        name: "cbf_chroma",
        slot: CBF_CHROMA,
        init: [&[94, 138, 182, 154, 154], &[149, 107, 167, 154, 154], &[149, 92, 167, 154, 154]],
    },
    // Table 9-23 abs_mvd_greater0_flag.
    CtxTable { name: "abs_mvd_greater0_flag", slot: ABS_MVD_GT0, init: [&[], &[140], &[169]] },
    // Table 9-24 abs_mvd_greater1_flag.
    CtxTable { name: "abs_mvd_greater1_flag", slot: ABS_MVD_GT1, init: [&[], &[198], &[198]] },
    // Table 9-25 cu_qp_delta_abs.
    CtxTable { name: "cu_qp_delta_abs", slot: CU_QP_DELTA_ABS, init: [&[154, 154], &[154, 154], &[154, 154]] },
    // Table 9-26 transform_skip_flag (luma, then chroma).
    CtxTable { name: "transform_skip_flag", slot: TRANSFORM_SKIP, init: [&[139, 139], &[139, 139], &[139, 139]] },
    // Table 9-27 last_sig_coeff_x_prefix.
    CtxTable { name: "last_sig_coeff_x_prefix", slot: LAST_X, init: [LAST_I, LAST_P, LAST_B] },
    // Table 9-28 last_sig_coeff_y_prefix (the same values as x).
    CtxTable { name: "last_sig_coeff_y_prefix", slot: LAST_Y, init: [LAST_I, LAST_P, LAST_B] },
    // Table 9-29 coded_sub_block_flag.
    CtxTable { name: "coded_sub_block_flag", slot: CSBF, init: [&[91, 171, 134, 141], &[121, 140, 61, 154], &[121, 140, 61, 154]] },
    // Table 9-30 sig_coeff_flag: 0..26 luma, 27..41 chroma, 42..43 the range extension's transform_skip_context.
    CtxTable {
        name: "sig_coeff_flag",
        slot: SIG,
        init: [
            &[
                111, 111, 125, 110, 110, 94, 124, 108, 124, 107, 125, 141, 179, 153, 125, 107, 125, 141, 179, 153, 125, 107,
                125, 141, 179, 153, 125, 140, 139, 182, 182, 152, 136, 152, 136, 153, 136, 139, 111, 136, 139, 111, 141,
                111,
            ],
            &[
                155, 154, 139, 153, 139, 123, 123, 63, 153, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 166,
                183, 140, 136, 153, 154, 170, 153, 123, 123, 107, 121, 107, 121, 167, 151, 183, 140, 151, 183, 140, 140,
                140,
            ],
            &[
                170, 154, 139, 153, 139, 123, 123, 63, 124, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 166,
                183, 140, 136, 153, 154, 170, 153, 138, 138, 122, 121, 122, 121, 167, 151, 183, 140, 151, 183, 140, 140,
                140,
            ],
        ],
    },
    // Table 9-31 coeff_abs_level_greater1_flag: 0..15 luma, 16..23 chroma.
    CtxTable {
        name: "coeff_abs_level_greater1_flag",
        slot: GT1,
        init: [
            &[140, 92, 137, 138, 140, 152, 138, 139, 153, 74, 149, 92, 139, 107, 122, 152, 140, 179, 166, 182, 140, 227, 122, 197],
            &[154, 196, 196, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 137, 169, 194, 166, 167, 154, 167, 137, 182],
            &[154, 196, 167, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 122, 169, 208, 166, 167, 154, 152, 167, 182],
        ],
    },
    // Table 9-32 coeff_abs_level_greater2_flag: 0..3 luma, 4..5 chroma.
    CtxTable {
        name: "coeff_abs_level_greater2_flag",
        slot: GT2,
        init: [&[138, 153, 136, 167, 152, 152], &[107, 167, 91, 122, 107, 167], &[107, 167, 91, 107, 107, 167]],
    },
];

/// 9.3.2.2: the state byte (pStateIdx << 1 | valMps, h264_core's encoding) of initValue `v` at SliceQpY `qp`.
pub fn init_state(v: u8, qp: i32) -> u8 {
    let slope = (v >> 4) as i32;
    let offset = (v & 15) as i32;
    let m = slope * 5 - 45;
    let n = (offset << 3) - 16;
    let pre = (((m * qp.clamp(0, 51)) >> 4) + n).clamp(1, 126);
    if pre <= 63 { ((63 - pre) << 1) as u8 } else { (((pre - 64) << 1) | 1) as u8 }
}

/// 9.3.4.2.5: ctxIdxMap for sig_coeff_flag in a 4x4 transform block, indexed by (yC << 2) + xC.
pub const CTX_IDX_MAP: [u8; 15] = [0, 1, 4, 5, 2, 3, 4, 5, 6, 6, 8, 8, 7, 7, 8];

/// Table 8-5 (8.4.4.2.6): intraPredAngle for predModeIntra 2..=34 (index = mode; 0 and 1 unused).
pub const INTRA_PRED_ANGLE: [i32; 35] = [
    0, 0, 32, 26, 21, 17, 13, 9, 5, 2, 0, -2, -5, -9, -13, -17, -21, -26, -32, -26, -21, -17, -13, -9, -5, -2, 0, 2, 5, 9, 13,
    17, 21, 26, 32,
];

/// Table 8-6 (8.4.4.2.6): invAngle for predModeIntra 11..=25 (index = mode - 11).
pub const INV_ANGLE: [i32; 15] = [-4096, -1638, -910, -630, -482, -390, -315, -256, -315, -390, -482, -630, -910, -1638, -4096];

/// 8.4.4.2.3: intraHorVerDistThres[nTbS] for nTbS 8, 16, 32 (index log2(nTbS) - 3).
pub const INTRA_HOR_VER_DIST_THRES: [i32; 3] = [7, 1, 0];

/// 8.6.2: levelScale[qP % 6].
pub const LEVEL_SCALE: [i32; 6] = [40, 45, 51, 57, 64, 72];

/// 8.6.4.2: the coefficient magnitudes of transMatrix. Every entry of the 32x32 matrix is
/// `±DCT_COEF[t]` with t the folded phase of (2n + 1) k (mod 128) in units of pi / 64 (see [`TRANS_MATRIX`]); the
/// Recommendation's columns 0..15 list exactly these values (row 1: 90 90 88 85 82 78 73 67 61 54 46 38 31 22 13 4).
pub const DCT_COEF: [i32; 33] = [
    64, 90, 90, 90, 89, 88, 87, 85, 83, 82, 80, 78, 75, 73, 70, 67, 64, 61, 57, 54, 50, 46, 43, 38, 36, 31, 25, 22, 18, 13, 9,
    4, 0,
];

const fn build_trans_matrix() -> [[i8; 32]; 32] {
    let mut m = [[0i8; 32]; 32];
    let mut k = 0;
    while k < 32 {
        let mut n = 0;
        while n < 32 {
            let t = ((2 * n + 1) * k) % 128;
            let v = if t <= 32 {
                DCT_COEF[t]
            } else if t <= 64 {
                -DCT_COEF[64 - t]
            } else if t <= 96 {
                -DCT_COEF[t - 64]
            } else {
                DCT_COEF[128 - t]
            };
            m[k][n] = v as i8;
            n += 1;
        }
        k += 1;
    }
    m
}

/// 8.6.4.2 transMatrix: row k is the k-th basis function of the 32-point inverse DCT (coefficient k contributes
/// `TRANS_MATRIX[k][n]` to sample n). The nTbS-point transform uses rows k * (32 / nTbS), columns 0..nTbS.
pub const TRANS_MATRIX: [[i8; 32]; 32] = build_trans_matrix();

/// 8.6.4.2: the 4x4 DST-VII transMatrix (row k = basis function k), for intra luma 4x4 blocks.
pub const DST_MATRIX: [[i32; 4]; 4] = [[29, 55, 74, 84], [74, 74, 0, -74], [84, -29, -74, 55], [55, -84, 74, -29]];

/// Table 8-10: QpC as a function of qPi for ChromaArrayType 1 (qPi < 30: QpC = qPi; qPi > 43: qPi - 6).
pub fn qpc_420(qpi: i32) -> i32 {
    const T: [i32; 14] = [29, 30, 31, 32, 33, 33, 34, 34, 35, 35, 36, 36, 37, 37];
    if qpi < 30 {
        qpi
    } else if qpi > 43 {
        qpi - 6
    } else {
        T[(qpi - 30) as usize]
    }
}

/// Table 8-12: beta' for Q = 0..51.
pub const BETA_TABLE: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 20, 22, 24, 26, 28, 30,
    32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64,
];

/// Table 8-12: tC' for Q = 0..53.
pub const TC_TABLE: [u8; 54] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 5, 5,
    6, 6, 7, 8, 9, 10, 11, 13, 14, 16, 18, 20, 22, 24,
];

/// Table 7-6: the default 8x8 intra scaling list (sizeId 1..3, matrixId 0..2), in up-right diagonal order.
pub const DEFAULT_8X8_INTRA: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 16, 17, 16, 17, 18, 17, 18, 18, 17, 18, 21, 19, 20, 21, 20, 19, 21, 24, 22,
    22, 24, 24, 22, 22, 24, 25, 25, 27, 30, 27, 25, 25, 29, 31, 35, 35, 31, 29, 36, 41, 44, 41, 36, 47, 54, 54, 47, 65, 70,
    65, 88, 88, 115,
];

/// Table 7-6: the default 8x8 inter scaling list (sizeId 1..3, matrixId 3..5), in up-right diagonal order.
pub const DEFAULT_8X8_INTER: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 17, 18, 18, 18, 18, 18, 18, 20, 20, 20, 20, 20, 20, 20, 24, 24,
    24, 24, 24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 28, 28, 28, 28, 28, 28, 33, 33, 33, 33, 33, 41, 41, 41, 41, 54, 54,
    54, 71, 71, 91,
];

/// 8.5.3.3.3.1 Table 8-11: the luma interpolation filter coefficients fL[xFracL][i], taps at xInt + i - 3 (row 0 is
/// the full-sample position, never filtered).
pub const LUMA_FILTER: [[i32; 8]; 4] =
    [[0, 0, 0, 64, 0, 0, 0, 0], [-1, 4, -10, 58, 17, -5, 1, 0], [-1, 4, -11, 40, 40, -11, 4, -1], [0, 1, -5, 17, 58, -10, 4, -1]];

/// 8.5.3.3.3.2 Table 8-12: the chroma interpolation filter coefficients fC[xFracC][i] (eighth samples), taps at
/// xInt + i - 1 (row 0 is the full-sample position).
pub const CHROMA_FILTER: [[i32; 4]; 8] = [
    [0, 64, 0, 0],
    [-2, 58, 10, -2],
    [-4, 54, 16, -2],
    [-6, 46, 28, -4],
    [-4, 36, 36, -4],
    [-4, 28, 46, -6],
    [-2, 16, 54, -4],
    [-2, 10, 58, -2],
];

/// 8.5.3.2.4 Table 8-7: (l0CandIdx, l1CandIdx) for combIdx 0..11 of the combined bi-predictive merging candidates.
pub const COMB_IDX: [(usize, usize); 12] = [(0, 1), (1, 0), (0, 2), (2, 0), (1, 2), (2, 1), (0, 3), (3, 0), (1, 3), (3, 1), (2, 3), (3, 2)];

/// HEVCREXT (SR334) 8.4.3 Table 8-3: the 4:2:2 IntraPredModeC from the mode X of Table 8-2 (ChromaArrayType 2).
pub const CHROMA_422_MODE: [u8; 35] =
    [0, 1, 2, 2, 2, 2, 3, 5, 7, 8, 10, 11, 13, 15, 16, 18, 19, 20, 21, 22, 23, 23, 24, 24, 25, 25, 26, 27, 27, 28, 28, 29, 29, 30, 31];
