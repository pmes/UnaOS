// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Symbolic constants and the small block-geometry lookups (spec §3 and §10.5). The large tables
//! are generated into [`crate::tables`].

pub const BLOCK_4X4: u8 = 0;
pub const BLOCK_4X8: u8 = 1;
pub const BLOCK_8X4: u8 = 2;
pub const BLOCK_8X8: u8 = 3;
pub const BLOCK_64X64: u8 = 12;
pub const BLOCK_INVALID: u8 = 13;

pub const TX_4X4: u8 = 0;
pub const TX_8X8: u8 = 1;
pub const TX_16X16: u8 = 2;
pub const TX_32X32: u8 = 3;

/// tx_mode (§7.3.1).
pub const ONLY_4X4: u8 = 0;
pub const ALLOW_32X32: u8 = 3;
pub const TX_MODE_SELECT: u8 = 4;

pub const PARTITION_NONE: u8 = 0;
pub const PARTITION_HORZ: u8 = 1;
pub const PARTITION_VERT: u8 = 2;
pub const PARTITION_SPLIT: u8 = 3;

/// Prediction modes: the ten intra modes, then the four inter modes.
pub const DC_PRED: u8 = 0;
pub const V_PRED: u8 = 1;
pub const H_PRED: u8 = 2;
pub const D45_PRED: u8 = 3;
pub const D135_PRED: u8 = 4;
pub const D117_PRED: u8 = 5;
pub const D153_PRED: u8 = 6;
pub const D207_PRED: u8 = 7;
pub const D63_PRED: u8 = 8;
pub const TM_PRED: u8 = 9;
pub const NEARESTMV: u8 = 10;
pub const NEARMV: u8 = 11;
pub const ZEROMV: u8 = 12;
pub const NEWMV: u8 = 13;

/// Reference frames as stored per block (INTRA_FRAME = 0; NONE = -1).
pub const NONE_FRAME: i8 = -1;
pub const INTRA_FRAME: i8 = 0;
pub const LAST_FRAME: i8 = 1;
pub const GOLDEN_FRAME: i8 = 2;
pub const ALTREF_FRAME: i8 = 3;

/// interp_filter values (§7.2 literal_to_type maps the header's 2-bit literal onto these).
pub const EIGHTTAP: u8 = 0;
pub const EIGHTTAP_SMOOTH: u8 = 1;
pub const EIGHTTAP_SHARP: u8 = 2;
pub const BILINEAR: u8 = 3;
pub const SWITCHABLE: u8 = 4;

/// Transform types (row/column 1-D kernels): DCT_DCT, ADST_DCT (ADST down the columns),
/// DCT_ADST, ADST_ADST.
pub const DCT_DCT: u8 = 0;
pub const ADST_DCT: u8 = 1;
pub const DCT_ADST: u8 = 2;
pub const ADST_ADST: u8 = 3;

/// reference_select (§7.3.4 reference_mode).
pub const SINGLE_REFERENCE: u8 = 0;
pub const COMPOUND_REFERENCE: u8 = 1;
pub const REFERENCE_MODE_SELECT: u8 = 2;

/// Segment features (§7.2.13).
pub const SEG_LVL_ALT_Q: usize = 0;
pub const SEG_LVL_ALT_L: usize = 1;
pub const SEG_LVL_REF_FRAME: usize = 2;
pub const SEG_LVL_SKIP: usize = 3;
pub const SEG_FEATURE_BITS: [u32; 4] = [8, 6, 2, 0];
pub const SEG_FEATURE_SIGNED: [bool; 4] = [true, true, false, false];
pub const SEG_FEATURE_MAX: [i32; 4] = [255, 63, 3, 0];

pub const MAX_LOOP_FILTER: i32 = 63;

pub const B_WIDTH_LOG2: [u8; 13] = [0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4];
pub const B_HEIGHT_LOG2: [u8; 13] = [0, 1, 0, 1, 2, 1, 2, 3, 2, 3, 4, 3, 4];
pub const NUM_4X4_W: [u8; 13] = [1, 1, 2, 2, 2, 4, 4, 4, 8, 8, 8, 16, 16];
pub const NUM_4X4_H: [u8; 13] = [1, 2, 1, 2, 4, 2, 4, 8, 4, 8, 16, 8, 16];
pub const NUM_8X8_W: [u8; 13] = [1, 1, 1, 1, 1, 2, 2, 2, 4, 4, 4, 8, 8];
pub const NUM_8X8_H: [u8; 13] = [1, 1, 1, 1, 2, 1, 2, 4, 2, 4, 8, 4, 8];
pub const SIZE_GROUP: [u8; 13] = [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3];
pub const MAX_TXSIZE: [u8; 13] = [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3];
pub const TX_MODE_TO_BIGGEST_TX_SIZE: [u8; 5] = [0, 1, 2, 3, 3];

/// Partition context written above / left after a block (§9.3.1 above_partition_context).
pub const PARTITION_CONTEXT_ABOVE: [u8; 13] = [15, 15, 14, 14, 14, 12, 12, 12, 8, 8, 8, 0, 0];
pub const PARTITION_CONTEXT_LEFT: [u8; 13] = [15, 14, 15, 14, 12, 14, 12, 8, 12, 8, 0, 8, 0];

/// Intra mode -> transform type for luma (§8.6.4 mode2txfm_map).
pub const MODE_TO_TX_TYPE: [u8; 10] =
    [DCT_DCT, ADST_DCT, DCT_ADST, DCT_DCT, ADST_ADST, ADST_DCT, DCT_ADST, DCT_ADST, ADST_DCT, ADST_ADST];

/// Trees (§9.3.1), libvpx-shaped: positive = next node pair, <= 0 = leaf value negated.
pub const INTRA_MODE_TREE: [i8; 18] = [
    -(DC_PRED as i8), 2, -(TM_PRED as i8), 4, -(V_PRED as i8), 6, 8, 12, -(H_PRED as i8), 10,
    -(D135_PRED as i8), -(D117_PRED as i8), -(D45_PRED as i8), 14, -(D63_PRED as i8), 16,
    -(D153_PRED as i8), -(D207_PRED as i8),
];
/// Leaves are inter_mode offsets: 0 NEARESTMV, 1 NEARMV, 2 ZEROMV, 3 NEWMV.
pub const INTER_MODE_TREE: [i8; 6] = [-2, 2, 0, 4, -1, -3];
pub const PARTITION_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];
pub const SEGMENT_TREE: [i8; 14] = [2, 4, 6, 8, 10, 12, 0, -1, -2, -3, -4, -5, -6, -7];
pub const SWITCHABLE_INTERP_TREE: [i8; 4] = [-(EIGHTTAP as i8), 2, -(EIGHTTAP_SMOOTH as i8), -(EIGHTTAP_SHARP as i8)];
pub const MV_JOINT_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];
pub const MV_CLASS_TREE: [i8; 20] = [0, 2, -1, 4, 6, 8, -2, -3, 10, 12, -4, -5, -6, 14, 16, 18, -7, -8, -9, -10];
pub const MV_CLASS0_TREE: [i8; 2] = [0, -1];
pub const MV_FP_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];
