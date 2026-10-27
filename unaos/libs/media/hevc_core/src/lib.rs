// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (A/V) — seam `shared-core` (a `no_std` lib both rings link, beside h264_core).
//!
//! HEVCPARSE (LEDGER SR199): an H.265 / HEVC bitstream parser and intra decoder written from ITU-T Rec. H.265
//! ("High efficiency video coding"), `no_std` + `alloc`. Every iPhone recording is HEVC; this is the first half of the
//! decoder: the whole bitstream, and pictures made of I slices. HEVCINTER (LEDGER SR216) adds P and B slices.
//!
//! | module          | Recommendation                                                                  |
//! |-----------------|---------------------------------------------------------------------------------|
//! | [`nal`]         | 7.3.1.2 NAL unit header, Table 7-1, ISO/IEC 14496-15 8.3.3.1 `hvcC`              |
//! | [`ps`]          | 7.3.3 profile_tier_level, 7.3.2.1-3 VPS / SPS / PPS, 7.3.4 scaling lists, 7.3.7 st_ref_pic_set, E.2 VUI / HRD, 6.5 scans and tiles |
//! | [`slice`]       | 7.3.6 slice segment header, pred_weight_table, lists modification, entry points |
//! | [`tables`]      | the constant tables (9.3.2.2 init values, 8.4.4.2.6, 8.6.2, 8.6.4.2, 8-10, 8-12) |
//! | [`cabac`]       | 9.3 CABAC over h264_core's engine: initialisation, WPP storage, binarizations   |
//! | [`ctu`]         | 7.3.8 slice segment data: SAO syntax, coding quadtree, coding unit, transform tree, residual coding; 8.6.1 QP |
//! | [`intra`]       | 8.4.2 / 8.4.3 intra modes, 8.4.4.2 reference samples, filtering, planar / DC / angular |
//! | [`transform`]   | 8.6.2 scaling, 8.6.4 inverse DCT 4/8/16/32 and DST 4x4, transform skip           |
//! | [`picture`]     | the picture under decoding and the per-block facts the loop filters read        |
//! | [`deblock`]     | 8.7.2 deblocking filter                                                         |
//! | [`sao`]         | 8.7.3 sample adaptive offset                                                    |
//! | [`dpb`]         | 8.3.2 reference picture set, 8.3.4 reference picture lists, C.5.2 DPB output    |
//! | [`inter`]       | 8.5.3 merge / AMVP / TMVP, 8-tap / 4-tap interpolation, weighted prediction      |
//! | [`decoder`]     | access units, 8.3.1 POC, NoRaslOutputFlag, C.5.2 output order, the API          |
//!
//! REUSED from h264_core, not copied: [`h264_core::cabac::Cabac`] (the arithmetic decoding engine: H.265 Tables 9-46 /
//! 9-47 and 9.3.4.3.2-5 equal H.264's 9.3.3.2, and 9.3.2.2's preCtxState gives the same state byte),
//! [`h264_core::bits::BitReader`], `h264_core::nal::{unescape, annexb_units, length_prefixed_units}`, and
//! [`h264_core::Frame`] (re-exported as [`Frame`]).
//!
//! THE ctxIdx LAYOUT (the engine's array has `h264_core::cabac::NUM_CTX` = 460 slots; hevc_core uses 0..157 and leaves
//! 157..460 free, so a later arc adds contexts above 157 without a collision). One range per syntax element, in
//! Table 9-4's order, each initialised from its own table in [`tables`]:
//!
//! | slots     | syntax element                                   | slots     | syntax element                     |
//! |-----------|--------------------------------------------------|-----------|------------------------------------|
//! | 0         | sao_merge_left_flag / sao_merge_up_flag          | 27..30    | split_transform_flag               |
//! | 1         | sao_type_idx_luma / sao_type_idx_chroma          | 30..32    | cbf_luma                           |
//! | 2..5      | split_cu_flag                                    | 32..37    | cbf_cb / cbf_cr (4 + the RExt 5th) |
//! | 5         | cu_transquant_bypass_flag                        | 37        | abs_mvd_greater0_flag              |
//! | 6..9      | cu_skip_flag                                     | 38        | abs_mvd_greater1_flag              |
//! | 9         | pred_mode_flag                                   | 39..41    | cu_qp_delta_abs                    |
//! | 10..14    | part_mode                                        | 41..43    | transform_skip_flag (luma, chroma) |
//! | 14        | prev_intra_luma_pred_flag                        | 43..61    | last_sig_coeff_x_prefix            |
//! | 15        | intra_chroma_pred_mode                           | 61..79    | last_sig_coeff_y_prefix            |
//! | 16        | rqt_root_cbf                                     | 79..83    | coded_sub_block_flag               |
//! | 17        | merge_flag                                       | 83..127   | sig_coeff_flag (42 + the RExt 2)   |
//! | 18        | merge_idx                                        | 127..151  | coeff_abs_level_greater1_flag      |
//! | 19..24    | inter_pred_idc                                   | 151..157  | coeff_abs_level_greater2_flag      |
//! | 24..26    | ref_idx_l0 / ref_idx_l1                          | 157..460  | FREE (HEVCINTER, RExt)             |
//! | 26        | mvp_l0_flag / mvp_l1_flag                        |           |                                    |
//!
//! The inter elements' slots are already laid out and initialised (initTypes 1 and 2) so the inter arc only adds the
//! syntax.
//!
//! Decoded: Main, Main 10 (HEVCMAIN10 SR311: 16-bit sample planes) and Main Still Picture streams at 8- and 10-bit 4:2:0 made of I, P and B slices — CTB 16/32/64 down
//! to an 8x8 CU, 2Nx2N and NxN intra, I_PCM, the 35 intra modes with strong intra smoothing, the 4x4 DST and the
//! 4..32 DCTs with scaling lists, transform skip, cu_transquant_bypass, sign data hiding, cu_qp_delta, chroma QP
//! offsets, tiles, wavefront (entropy_coding_sync) and dependent slice segments, deblocking, SAO, the conformance
//! window and POC output order; and (HEVCINTER) skip / merge / AMVP prediction units of every PartMode with TMVP,
//! uni- and bi-prediction with default or explicit weights, the inter residual, short- and long-term references and
//! deblocking bS 0 / 1 from motion.
//!
//! Refused by name ([`Error::Unsupported`]): constrained_intra_pred_flag in P / B slices (HEVCINTER-CIP), bit depths
//! above 10 or unequal luma / chroma bit depths (HEVCREXT), chroma other than 4:2:0, separate colour planes, every range-extension and screen-content
//! tool. Layers above the base (nuh_layer_id > 0) are ignored.

#![no_std]
#![forbid(unsafe_code)]
// The decoder indexes arrays the way the Recommendation's equations do.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

pub mod cabac;
pub mod ctu;
pub mod deblock;
pub mod decoder;
pub mod dpb;
pub mod inter;
pub mod intra;
pub mod nal;
pub mod picture;
pub mod ps;
pub mod sao;
pub mod slice;
pub mod tables;
pub mod transform;

pub use decoder::Decoder;
pub use h264_core::Frame;

/// Every way a parse or decode can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Ran out of bytes.
    Truncated,
    /// The bitstream violates the Recommendation.
    Invalid(&'static str),
    /// A valid stream this decoder refuses by name (high bit depth, 4:2:2 / 4:4:4, range extensions, constrained intra
    /// prediction in inter slices).
    Unsupported(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("hevc: truncated"),
            Error::Invalid(s) => write!(f, "hevc: invalid ({s})"),
            Error::Unsupported(s) => write!(f, "hevc: unsupported ({s})"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

impl From<h264_core::Error> for Error {
    fn from(e: h264_core::Error) -> Error {
        match e {
            h264_core::Error::Truncated => Error::Truncated,
            h264_core::Error::Invalid(s) => Error::Invalid(s),
            h264_core::Error::Unsupported(s) => Error::Unsupported(s),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;
