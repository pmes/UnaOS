//! The CDF arrays of §6.8.2 (init_non_coeff_cdfs / init_coeff_cdfs) — every array the spec
//! names, intra and inter. A [`CdfContext`] is used three ways, exactly as the spec does:
//!
//! * the frame CDFs ("no prefix"): initialised from the defaults or loaded from a reference slot
//!   (`load_cdfs`, §6.8.2, which also zeroes every symbol counter);
//! * the tile CDFs ("Tile" prefix, §8.2.2 init_symbol): a copy of the frame CDFs per tile;
//! * the saved CDFs ("Saved" prefix, §8.2.4 exit_symbol for TileNum == context_update_tile_id)
//!   which become the frame CDFs in frame_end_update_cdf (§7.4) and are stored with the frame by
//!   save_cdfs (§7.20).
//!
//! AV1 has no CDF averaging: the frame-end update is that copy of one tile's adapted CDFs.
//! The default tables are generated from the spec into `tables.rs`.

use crate::tables::*;

/// Visit every CDF array (innermost `[u16; N]`, the last entry being the symbol counter).
pub trait CdfArray {
    fn reset_counts(&mut self);
}
impl<const N: usize> CdfArray for [u16; N] {
    #[inline]
    fn reset_counts(&mut self) {
        self[N - 1] = 0;
    }
}
impl<const N: usize, const M: usize> CdfArray for [[u16; N]; M] {
    fn reset_counts(&mut self) {
        for a in self.iter_mut() {
            a.reset_counts();
        }
    }
}
impl<const N: usize, const M: usize, const L: usize> CdfArray for [[[u16; N]; M]; L] {
    fn reset_counts(&mut self) {
        for a in self.iter_mut() {
            a.reset_counts();
        }
    }
}
impl<const N: usize, const M: usize, const L: usize, const K: usize> CdfArray for [[[[u16; N]; M]; L]; K] {
    fn reset_counts(&mut self) {
        for a in self.iter_mut() {
            a.reset_counts();
        }
    }
}

macro_rules! cdf_context {
    ($($name:ident : $ty:ty = $init:expr),* $(,)?) => {
        #[derive(Clone)]
        pub struct CdfContext {
            $(pub $name: $ty,)*
            // coefficient cdfs (init_coeff_cdfs, indexed by the base_q_idx context)
            pub txb_skip: [[[u16; 3]; 13]; 5],
            pub eob_pt_16: [[[u16; 6]; 2]; 2],
            pub eob_pt_32: [[[u16; 7]; 2]; 2],
            pub eob_pt_64: [[[u16; 8]; 2]; 2],
            pub eob_pt_128: [[[u16; 9]; 2]; 2],
            pub eob_pt_256: [[[u16; 10]; 2]; 2],
            pub eob_pt_512: [[u16; 11]; 2],
            pub eob_pt_1024: [[u16; 12]; 2],
            pub eob_extra: [[[[u16; 3]; 9]; 2]; 5],
            pub dc_sign: [[[u16; 3]; 3]; 2],
            pub coeff_base_eob: [[[[u16; 4]; 4]; 2]; 5],
            pub coeff_base: [[[[u16; 5]; 42]; 2]; 5],
            pub coeff_br: [[[[u16; 5]; 21]; 2]; 5],
        }
        impl CdfContext {
            fn non_coeff_defaults() -> CdfContext {
                CdfContext {
                    $($name: $init,)*
                    txb_skip: DEFAULT_TXB_SKIP_CDF[0],
                    eob_pt_16: DEFAULT_EOB_PT_16_CDF[0],
                    eob_pt_32: DEFAULT_EOB_PT_32_CDF[0],
                    eob_pt_64: DEFAULT_EOB_PT_64_CDF[0],
                    eob_pt_128: DEFAULT_EOB_PT_128_CDF[0],
                    eob_pt_256: DEFAULT_EOB_PT_256_CDF[0],
                    eob_pt_512: DEFAULT_EOB_PT_512_CDF[0],
                    eob_pt_1024: DEFAULT_EOB_PT_1024_CDF[0],
                    eob_extra: DEFAULT_EOB_EXTRA_CDF[0],
                    dc_sign: DEFAULT_DC_SIGN_CDF[0],
                    coeff_base_eob: DEFAULT_COEFF_BASE_EOB_CDF[0],
                    coeff_base: DEFAULT_COEFF_BASE_CDF[0],
                    coeff_br: DEFAULT_COEFF_BR_CDF[0],
                }
            }
            /// The counter-reset half of load_cdfs (§6.8.2): "the last entry in each array,
            /// representing the symbol count for that context, is set to 0".
            pub fn reset_counts(&mut self) {
                $(self.$name.reset_counts();)*
                self.txb_skip.reset_counts();
                self.eob_pt_16.reset_counts();
                self.eob_pt_32.reset_counts();
                self.eob_pt_64.reset_counts();
                self.eob_pt_128.reset_counts();
                self.eob_pt_256.reset_counts();
                self.eob_pt_512.reset_counts();
                self.eob_pt_1024.reset_counts();
                self.eob_extra.reset_counts();
                self.dc_sign.reset_counts();
                self.coeff_base_eob.reset_counts();
                self.coeff_base.reset_counts();
                self.coeff_br.reset_counts();
            }
        }
    };
}

const fn mv2<const N: usize>(d: [u16; N]) -> [[[u16; N]; 2]; 2] {
    [[d, d], [d, d]]
}

cdf_context! {
    intra_frame_y_mode: [[[u16; 14]; 5]; 5] = DEFAULT_INTRA_FRAME_Y_MODE_CDF,
    y_mode: [[u16; 14]; 4] = DEFAULT_Y_MODE_CDF,
    uv_mode_cfl_not_allowed: [[u16; 14]; 13] = DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF,
    uv_mode_cfl_allowed: [[u16; 15]; 13] = DEFAULT_UV_MODE_CFL_ALLOWED_CDF,
    angle_delta: [[u16; 8]; 8] = DEFAULT_ANGLE_DELTA_CDF,
    intrabc: [u16; 3] = DEFAULT_INTRABC_CDF,
    partition_w8: [[u16; 5]; 4] = DEFAULT_PARTITION_W8_CDF,
    partition_w16: [[u16; 11]; 4] = DEFAULT_PARTITION_W16_CDF,
    partition_w32: [[u16; 11]; 4] = DEFAULT_PARTITION_W32_CDF,
    partition_w64: [[u16; 11]; 4] = DEFAULT_PARTITION_W64_CDF,
    partition_w128: [[u16; 9]; 4] = DEFAULT_PARTITION_W128_CDF,
    segment_id: [[u16; 9]; 3] = DEFAULT_SEGMENT_ID_CDF,
    segment_id_predicted: [[u16; 3]; 3] = DEFAULT_SEGMENT_ID_PREDICTED_CDF,
    tx_8x8: [[u16; 3]; 3] = DEFAULT_TX_8X8_CDF,
    tx_16x16: [[u16; 4]; 3] = DEFAULT_TX_16X16_CDF,
    tx_32x32: [[u16; 4]; 3] = DEFAULT_TX_32X32_CDF,
    tx_64x64: [[u16; 4]; 3] = DEFAULT_TX_64X64_CDF,
    txfm_split: [[u16; 3]; 21] = DEFAULT_TXFM_SPLIT_CDF,
    filter_intra_mode: [u16; 6] = DEFAULT_FILTER_INTRA_MODE_CDF,
    filter_intra: [[u16; 3]; 22] = DEFAULT_FILTER_INTRA_CDF,
    interp_filter: [[u16; 4]; 16] = DEFAULT_INTERP_FILTER_CDF,
    motion_mode: [[u16; 4]; 22] = DEFAULT_MOTION_MODE_CDF,
    new_mv: [[u16; 3]; 6] = DEFAULT_NEW_MV_CDF,
    zero_mv: [[u16; 3]; 2] = DEFAULT_ZERO_MV_CDF,
    ref_mv: [[u16; 3]; 6] = DEFAULT_REF_MV_CDF,
    compound_mode: [[u16; 9]; 8] = DEFAULT_COMPOUND_MODE_CDF,
    drl_mode: [[u16; 3]; 3] = DEFAULT_DRL_MODE_CDF,
    is_inter: [[u16; 3]; 4] = DEFAULT_IS_INTER_CDF,
    comp_mode: [[u16; 3]; 5] = DEFAULT_COMP_MODE_CDF,
    skip_mode: [[u16; 3]; 3] = DEFAULT_SKIP_MODE_CDF,
    skip: [[u16; 3]; 3] = DEFAULT_SKIP_CDF,
    comp_ref: [[[u16; 3]; 3]; 3] = DEFAULT_COMP_REF_CDF,
    comp_bwd_ref: [[[u16; 3]; 2]; 3] = DEFAULT_COMP_BWD_REF_CDF,
    single_ref: [[[u16; 3]; 6]; 3] = DEFAULT_SINGLE_REF_CDF,
    // MV cdfs: [MvCtx][comp] (MV_CONTEXTS = 2: normal, intra block copy)
    mv_joint: [[u16; 5]; 2] = [DEFAULT_MV_JOINT_CDF, DEFAULT_MV_JOINT_CDF],
    mv_class: [[[u16; 12]; 2]; 2] = [DEFAULT_MV_CLASS_CDF, DEFAULT_MV_CLASS_CDF],
    mv_class0_bit: [[[u16; 3]; 2]; 2] = mv2(DEFAULT_MV_CLASS0_BIT_CDF),
    mv_fr: [[[u16; 5]; 2]; 2] = [DEFAULT_MV_FR_CDF, DEFAULT_MV_FR_CDF],
    mv_class0_fr: [[[[u16; 5]; 2]; 2]; 2] = [DEFAULT_MV_CLASS0_FR_CDF, DEFAULT_MV_CLASS0_FR_CDF],
    mv_class0_hp: [[[u16; 3]; 2]; 2] = mv2(DEFAULT_MV_CLASS0_HP_CDF),
    mv_sign: [[[u16; 3]; 2]; 2] = mv2(DEFAULT_MV_SIGN_CDF),
    mv_bit: [[[[u16; 3]; 10]; 2]; 2] = [[DEFAULT_MV_BIT_CDF, DEFAULT_MV_BIT_CDF], [DEFAULT_MV_BIT_CDF, DEFAULT_MV_BIT_CDF]],
    mv_hp: [[[u16; 3]; 2]; 2] = mv2(DEFAULT_MV_HP_CDF),
    palette_y_mode: [[[u16; 3]; 3]; 7] = DEFAULT_PALETTE_Y_MODE_CDF,
    palette_uv_mode: [[u16; 3]; 2] = DEFAULT_PALETTE_UV_MODE_CDF,
    palette_y_size: [[u16; 8]; 7] = DEFAULT_PALETTE_Y_SIZE_CDF,
    palette_uv_size: [[u16; 8]; 7] = DEFAULT_PALETTE_UV_SIZE_CDF,
    palette_size_2_y_color: [[u16; 3]; 5] = DEFAULT_PALETTE_SIZE_2_Y_COLOR_CDF,
    palette_size_3_y_color: [[u16; 4]; 5] = DEFAULT_PALETTE_SIZE_3_Y_COLOR_CDF,
    palette_size_4_y_color: [[u16; 5]; 5] = DEFAULT_PALETTE_SIZE_4_Y_COLOR_CDF,
    palette_size_5_y_color: [[u16; 6]; 5] = DEFAULT_PALETTE_SIZE_5_Y_COLOR_CDF,
    palette_size_6_y_color: [[u16; 7]; 5] = DEFAULT_PALETTE_SIZE_6_Y_COLOR_CDF,
    palette_size_7_y_color: [[u16; 8]; 5] = DEFAULT_PALETTE_SIZE_7_Y_COLOR_CDF,
    palette_size_8_y_color: [[u16; 9]; 5] = DEFAULT_PALETTE_SIZE_8_Y_COLOR_CDF,
    palette_size_2_uv_color: [[u16; 3]; 5] = DEFAULT_PALETTE_SIZE_2_UV_COLOR_CDF,
    palette_size_3_uv_color: [[u16; 4]; 5] = DEFAULT_PALETTE_SIZE_3_UV_COLOR_CDF,
    palette_size_4_uv_color: [[u16; 5]; 5] = DEFAULT_PALETTE_SIZE_4_UV_COLOR_CDF,
    palette_size_5_uv_color: [[u16; 6]; 5] = DEFAULT_PALETTE_SIZE_5_UV_COLOR_CDF,
    palette_size_6_uv_color: [[u16; 7]; 5] = DEFAULT_PALETTE_SIZE_6_UV_COLOR_CDF,
    palette_size_7_uv_color: [[u16; 8]; 5] = DEFAULT_PALETTE_SIZE_7_UV_COLOR_CDF,
    palette_size_8_uv_color: [[u16; 9]; 5] = DEFAULT_PALETTE_SIZE_8_UV_COLOR_CDF,
    delta_q: [u16; 5] = DEFAULT_DELTA_Q_CDF,
    delta_lf: [u16; 5] = DEFAULT_DELTA_LF_CDF,
    delta_lf_multi: [[u16; 5]; 4] = [DEFAULT_DELTA_LF_CDF; 4],
    intra_tx_type_set1: [[[u16; 8]; 13]; 2] = DEFAULT_INTRA_TX_TYPE_SET1_CDF,
    intra_tx_type_set2: [[[u16; 6]; 13]; 3] = DEFAULT_INTRA_TX_TYPE_SET2_CDF,
    inter_tx_type_set1: [[u16; 17]; 2] = DEFAULT_INTER_TX_TYPE_SET1_CDF,
    inter_tx_type_set2: [u16; 13] = DEFAULT_INTER_TX_TYPE_SET2_CDF,
    inter_tx_type_set3: [[u16; 3]; 4] = DEFAULT_INTER_TX_TYPE_SET3_CDF,
    use_obmc: [[u16; 3]; 22] = DEFAULT_USE_OBMC_CDF,
    inter_intra: [[u16; 3]; 3] = DEFAULT_INTER_INTRA_CDF,
    comp_ref_type: [[u16; 3]; 5] = DEFAULT_COMP_REF_TYPE_CDF,
    cfl_sign: [u16; 9] = DEFAULT_CFL_SIGN_CDF,
    uni_comp_ref: [[[u16; 3]; 3]; 3] = DEFAULT_UNI_COMP_REF_CDF,
    wedge_inter_intra: [[u16; 3]; 22] = DEFAULT_WEDGE_INTER_INTRA_CDF,
    comp_group_idx: [[u16; 3]; 6] = DEFAULT_COMP_GROUP_IDX_CDF,
    compound_idx: [[u16; 3]; 6] = DEFAULT_COMPOUND_IDX_CDF,
    compound_type: [[u16; 3]; 22] = DEFAULT_COMPOUND_TYPE_CDF,
    inter_intra_mode: [[u16; 5]; 3] = DEFAULT_INTER_INTRA_MODE_CDF,
    wedge_index: [[u16; 17]; 22] = DEFAULT_WEDGE_INDEX_CDF,
    cfl_alpha: [[u16; 17]; 6] = DEFAULT_CFL_ALPHA_CDF,
    use_wiener: [u16; 3] = DEFAULT_USE_WIENER_CDF,
    use_sgrproj: [u16; 3] = DEFAULT_USE_SGRPROJ_CDF,
    restoration_type: [u16; 4] = DEFAULT_RESTORATION_TYPE_CDF,
}

/// The coefficient-cdf context index for `base_q_idx` (§6.8.2 init_coeff_cdfs).
pub fn coeff_cdf_q_ctx(base_q_idx: u32) -> usize {
    if base_q_idx <= 20 {
        0
    } else if base_q_idx <= 60 {
        1
    } else if base_q_idx <= 120 {
        2
    } else {
        3
    }
}

impl CdfContext {
    /// init_non_coeff_cdfs() followed by init_coeff_cdfs() for `base_q_idx`.
    pub fn new(base_q_idx: u32) -> CdfContext {
        let mut c = CdfContext::non_coeff_defaults();
        c.init_coeff_cdfs(base_q_idx);
        c
    }

    /// init_coeff_cdfs() (§6.8.2): reset the coefficient cdfs to the defaults for `base_q_idx`.
    pub fn init_coeff_cdfs(&mut self, base_q_idx: u32) {
        let idx = coeff_cdf_q_ctx(base_q_idx);
        self.txb_skip = DEFAULT_TXB_SKIP_CDF[idx];
        self.eob_pt_16 = DEFAULT_EOB_PT_16_CDF[idx];
        self.eob_pt_32 = DEFAULT_EOB_PT_32_CDF[idx];
        self.eob_pt_64 = DEFAULT_EOB_PT_64_CDF[idx];
        self.eob_pt_128 = DEFAULT_EOB_PT_128_CDF[idx];
        self.eob_pt_256 = DEFAULT_EOB_PT_256_CDF[idx];
        self.eob_pt_512 = DEFAULT_EOB_PT_512_CDF[idx];
        self.eob_pt_1024 = DEFAULT_EOB_PT_1024_CDF[idx];
        self.eob_extra = DEFAULT_EOB_EXTRA_CDF[idx];
        self.dc_sign = DEFAULT_DC_SIGN_CDF[idx];
        self.coeff_base_eob = DEFAULT_COEFF_BASE_EOB_CDF[idx];
        self.coeff_base = DEFAULT_COEFF_BASE_CDF[idx];
        self.coeff_br = DEFAULT_COEFF_BR_CDF[idx];
    }

    /// The per-tile copy made by init_symbol (§8.2.2): every array is copied from the frame
    /// CDFs except TileIntraFrameYModeCdf, which always starts from the default.
    pub fn for_tile(frame: &CdfContext) -> CdfContext {
        let mut t = frame.clone();
        t.intra_frame_y_mode = DEFAULT_INTRA_FRAME_Y_MODE_CDF;
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_counts_zeroes_every_counter() {
        let mut c = CdfContext::new(100);
        c.skip[1][2] = 17;
        c.coeff_base[4][1][41][4] = 31;
        c.mv_bit[1][1][9][2] = 5;
        c.reset_counts();
        assert_eq!(c.skip[1][2], 0);
        assert_eq!(c.coeff_base[4][1][41][4], 0);
        assert_eq!(c.mv_bit[1][1][9][2], 0);
        // the cdf values themselves are untouched
        assert_eq!(c.skip[1][1], 32768);
    }
}
