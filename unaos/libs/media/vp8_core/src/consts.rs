// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The small RFC 6386 constants: mode numbering, the token trees, the scan order and the
//! sub-pixel filter taps. (The large probability tables live in the generated `tables.rs`.)

// Macroblock luma modes (§8.1 / §16.1). Intra first, then the inter modes.
pub const DC_PRED: u8 = 0;
pub const V_PRED: u8 = 1;
pub const H_PRED: u8 = 2;
pub const TM_PRED: u8 = 3;
pub const B_PRED: u8 = 4;
pub const NEARESTMV: u8 = 5;
pub const NEARMV: u8 = 6;
pub const ZEROMV: u8 = 7;
pub const NEWMV: u8 = 8;
pub const SPLITMV: u8 = 9;

// Sub-block intra modes (§12.3), in the order the probability tables index them.
pub const B_DC_PRED: u8 = 0;
pub const B_TM_PRED: u8 = 1;
pub const B_VE_PRED: u8 = 2;
pub const B_HE_PRED: u8 = 3;
pub const B_LD_PRED: u8 = 4;
pub const B_RD_PRED: u8 = 5;
pub const B_VR_PRED: u8 = 6;
pub const B_VL_PRED: u8 = 7;
pub const B_HD_PRED: u8 = 8;
pub const B_HU_PRED: u8 = 9;

// Reference frames (§9.7 / §16.2).
pub const INTRA_FRAME: u8 = 0;
pub const LAST_FRAME: u8 = 1;
pub const GOLDEN_FRAME: u8 = 2;
pub const ALTREF_FRAME: u8 = 3;

/// `ymode_tree` (§11.2, inter frames).
pub const YMODE_TREE: [i8; 8] = [-(DC_PRED as i8), 2, 4, 6, -(V_PRED as i8), -(H_PRED as i8), -(TM_PRED as i8), -(B_PRED as i8)];
/// `kf_ymode_tree` (§11.2).
pub const KF_YMODE_TREE: [i8; 8] = [-(B_PRED as i8), 2, 4, 6, -(DC_PRED as i8), -(V_PRED as i8), -(H_PRED as i8), -(TM_PRED as i8)];
/// `uv_mode_tree` (§11.2).
pub const UV_MODE_TREE: [i8; 6] = [-(DC_PRED as i8), 2, -(V_PRED as i8), 4, -(H_PRED as i8), -(TM_PRED as i8)];
/// `bmode_tree` (§11.2).
pub const BMODE_TREE: [i8; 18] = [
    -(B_DC_PRED as i8), 2,
    -(B_TM_PRED as i8), 4,
    -(B_VE_PRED as i8), 6,
    8, 12,
    -(B_HE_PRED as i8), 10,
    -(B_RD_PRED as i8), -(B_VR_PRED as i8),
    -(B_LD_PRED as i8), 14,
    -(B_VL_PRED as i8), 16,
    -(B_HD_PRED as i8), -(B_HU_PRED as i8),
];
/// `small_mvtree` (§17.2): short motion-vector magnitudes 0..7.
pub const SMALL_MV_TREE: [i8; 14] = [2, 8, 4, 6, -0, -1, -2, -3, 10, 12, -4, -5, -6, -7];

/// The mode contexts for the inter mode tree (§16.3 `vp8_mode_contexts`).
pub const MODE_CONTEXTS: [[u8; 4]; 6] = [
    [7, 1, 1, 143],
    [14, 18, 14, 107],
    [135, 64, 57, 68],
    [60, 56, 128, 65],
    [159, 134, 128, 34],
    [234, 188, 128, 28],
];

/// Zig-zag scan (§13): coefficient `i` in token order lands at `ZIGZAG[i]` in raster order.
pub const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
/// Coefficient band of token position `i` (§13.3).
pub const BANDS: [usize; 17] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7, 0];

/// DCT_CAT3..6 extra-bit probabilities (§13.2); CAT1/CAT2 are written inline.
pub const PCAT3: [u8; 3] = [173, 148, 140];
pub const PCAT4: [u8; 4] = [176, 155, 140, 135];
pub const PCAT5: [u8; 5] = [180, 157, 141, 134, 130];
pub const PCAT6: [u8; 11] = [254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129];

/// Six-tap sub-pixel filters, indexed by eighth-pel position (§18.3; luma uses the even ones).
pub const SIXTAP: [[i32; 6]; 8] = [
    [0, 0, 128, 0, 0, 0],
    [0, -6, 123, 12, -1, 0],
    [2, -11, 108, 36, -8, 1],
    [0, -9, 93, 50, -6, 0],
    [3, -16, 77, 77, -16, 3],
    [0, -6, 50, 93, -9, 0],
    [1, -8, 36, 108, -11, 2],
    [0, -1, 12, 123, -6, 0],
];
/// Bilinear filters (§18.3, versions 1 and 2).
pub const BILINEAR: [[i32; 2]; 8] = [[128, 0], [112, 16], [96, 32], [80, 48], [64, 64], [48, 80], [32, 96], [16, 112]];
