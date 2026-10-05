//! §5.3–§5.9 (syntax) and §6.2–§6.8 (semantics): OBU framing, the sequence header, and the
//! uncompressed frame header including tile info. Field names are the spec's syntax element
//! names; derived variables keep their CamelCase spelling in snake_case.

use crate::bits::{BitReader, floor_log2};
use crate::tables::*;
use crate::refs::RefStore;
use crate::{Error, Result};
use alloc::vec::Vec;

pub const OBU_SEQUENCE_HEADER_T: u8 = 1;
pub const OBU_TEMPORAL_DELIMITER_T: u8 = 2;
pub const OBU_FRAME_HEADER_T: u8 = 3;
pub const OBU_TILE_GROUP_T: u8 = 4;
pub const OBU_METADATA_T: u8 = 5;
pub const OBU_FRAME_T: u8 = 6;
pub const OBU_REDUNDANT_FRAME_HEADER_T: u8 = 7;
pub const OBU_TILE_LIST_T: u8 = 8;
pub const OBU_PADDING_T: u8 = 15;

/// One OBU as framed by §5.3.1 (low-overhead format, obu_has_size_field may be 0 for the last).
#[derive(Debug, Clone, Copy)]
pub struct Obu<'a> {
    pub obu_type: u8,
    pub temporal_id: u8,
    pub spatial_id: u8,
    pub has_extension: bool,
    /// The OBU payload (obu_size bytes after the header).
    pub payload: &'a [u8],
}

/// Split a low-overhead OBU stream (§5.2) into OBUs.
pub fn split_obus(data: &[u8]) -> Result<Vec<Obu<'_>>> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off < data.len() {
        let mut r = BitReader::new(&data[off..]);
        let forbidden = r.f(1)?;
        if forbidden != 0 {
            return Err(Error::Invalid("obu_forbidden_bit"));
        }
        let obu_type = r.f(4)? as u8;
        let ext = r.flag()?;
        let has_size = r.flag()?;
        let _reserved = r.f(1)?;
        let (mut tid, mut sid) = (0, 0);
        if ext {
            tid = r.f(3)? as u8;
            sid = r.f(2)? as u8;
            r.f(3)?;
        }
        let size = if has_size {
            r.leb128()? as usize
        } else {
            data.len() - off - 1 - ext as usize
        };
        let start = off + r.byte_pos();
        let end = start.checked_add(size).ok_or(Error::Truncated)?;
        if end > data.len() {
            return Err(Error::Truncated);
        }
        out.push(Obu { obu_type, temporal_id: tid, spatial_id: sid, has_extension: ext, payload: &data[start..end] });
        off = end;
    }
    Ok(out)
}

/// §5.5.2 color_config (+ §6.4.2 semantics).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorConfig {
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub bit_depth: u32,
    pub mono_chrome: bool,
    pub num_planes: usize,
    pub color_description_present_flag: bool,
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub color_range: bool,
    pub subsampling_x: u32,
    pub subsampling_y: u32,
    pub chroma_sample_position: u8,
    pub separate_uv_delta_q: bool,
}

/// §5.5.3 timing_info
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimingInfo {
    pub num_units_in_display_tick: u32,
    pub time_scale: u32,
    pub equal_picture_interval: bool,
    pub num_ticks_per_picture_minus_1: u32,
}

/// §5.5.4 decoder_model_info
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecoderModelInfo {
    pub buffer_delay_length_minus_1: u8,
    pub num_units_in_decoding_tick: u32,
    pub buffer_removal_time_length_minus_1: u8,
    pub frame_presentation_time_length_minus_1: u8,
}

/// One operating point (§5.5.1 loop body, §5.5.5 operating_parameters_info).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperatingPoint {
    pub idc: u16,
    pub seq_level_idx: u8,
    pub seq_tier: u8,
    pub decoder_model_present_for_this_op: bool,
    pub decoder_buffer_delay: u32,
    pub encoder_buffer_delay: u32,
    pub low_delay_mode_flag: bool,
    pub initial_display_delay_present_for_this_op: bool,
    pub initial_display_delay_minus_1: u8,
}

/// §5.5.1 sequence_header_obu — every field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SequenceHeader {
    pub seq_profile: u8,
    pub still_picture: bool,
    pub reduced_still_picture_header: bool,
    pub timing_info_present_flag: bool,
    pub timing_info: TimingInfo,
    pub decoder_model_info_present_flag: bool,
    pub decoder_model_info: DecoderModelInfo,
    pub initial_display_delay_present_flag: bool,
    pub operating_points: Vec<OperatingPoint>,
    /// OperatingPointIdc of the chosen operating point (we choose 0, §7.1 default).
    pub operating_point_idc: u16,
    pub frame_width_bits_minus_1: u8,
    pub frame_height_bits_minus_1: u8,
    pub max_frame_width_minus_1: u32,
    pub max_frame_height_minus_1: u32,
    pub frame_id_numbers_present_flag: bool,
    pub delta_frame_id_length_minus_2: u8,
    pub additional_frame_id_length_minus_1: u8,
    pub use_128x128_superblock: bool,
    pub enable_filter_intra: bool,
    pub enable_intra_edge_filter: bool,
    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_warped_motion: bool,
    pub enable_dual_filter: bool,
    pub enable_order_hint: bool,
    pub enable_jnt_comp: bool,
    pub enable_ref_frame_mvs: bool,
    pub seq_choose_screen_content_tools: bool,
    pub seq_force_screen_content_tools: u32,
    pub seq_force_integer_mv: u32,
    pub order_hint_bits: u32,
    pub enable_superres: bool,
    pub enable_cdef: bool,
    pub enable_restoration: bool,
    pub color_config: ColorConfig,
    pub film_grain_params_present: bool,
}

impl SequenceHeader {
    pub fn parse(payload: &[u8]) -> Result<SequenceHeader> {
        let mut r = BitReader::new(payload);
        let mut s = SequenceHeader { seq_profile: r.f(3)? as u8, ..Default::default() };
        if s.seq_profile > 2 {
            return Err(Error::Invalid("seq_profile"));
        }
        s.still_picture = r.flag()?;
        s.reduced_still_picture_header = r.flag()?;
        if s.reduced_still_picture_header {
            let op = OperatingPoint { seq_level_idx: r.f(5)? as u8, ..Default::default() };
            s.operating_points.push(op);
        } else {
            s.timing_info_present_flag = r.flag()?;
            if s.timing_info_present_flag {
                s.timing_info.num_units_in_display_tick = r.f(32)?;
                s.timing_info.time_scale = r.f(32)?;
                s.timing_info.equal_picture_interval = r.flag()?;
                if s.timing_info.equal_picture_interval {
                    s.timing_info.num_ticks_per_picture_minus_1 = r.uvlc()?;
                }
                s.decoder_model_info_present_flag = r.flag()?;
                if s.decoder_model_info_present_flag {
                    s.decoder_model_info.buffer_delay_length_minus_1 = r.f(5)? as u8;
                    s.decoder_model_info.num_units_in_decoding_tick = r.f(32)?;
                    s.decoder_model_info.buffer_removal_time_length_minus_1 = r.f(5)? as u8;
                    s.decoder_model_info.frame_presentation_time_length_minus_1 = r.f(5)? as u8;
                }
            }
            s.initial_display_delay_present_flag = r.flag()?;
            let cnt = r.f(5)? + 1;
            for _ in 0..cnt {
                let mut op = OperatingPoint { idc: r.f(12)? as u16, seq_level_idx: r.f(5)? as u8, ..Default::default() };
                if op.seq_level_idx > 7 {
                    op.seq_tier = r.f(1)? as u8;
                }
                if s.decoder_model_info_present_flag {
                    op.decoder_model_present_for_this_op = r.flag()?;
                    if op.decoder_model_present_for_this_op {
                        let n = s.decoder_model_info.buffer_delay_length_minus_1 as u32 + 1;
                        op.decoder_buffer_delay = r.f(n)?;
                        op.encoder_buffer_delay = r.f(n)?;
                        op.low_delay_mode_flag = r.flag()?;
                    }
                }
                if s.initial_display_delay_present_flag {
                    op.initial_display_delay_present_for_this_op = r.flag()?;
                    if op.initial_display_delay_present_for_this_op {
                        op.initial_display_delay_minus_1 = r.f(4)? as u8;
                    }
                }
                s.operating_points.push(op);
            }
        }
        s.operating_point_idc = s.operating_points[0].idc;
        s.frame_width_bits_minus_1 = r.f(4)? as u8;
        s.frame_height_bits_minus_1 = r.f(4)? as u8;
        s.max_frame_width_minus_1 = r.f(s.frame_width_bits_minus_1 as u32 + 1)?;
        s.max_frame_height_minus_1 = r.f(s.frame_height_bits_minus_1 as u32 + 1)?;
        if !s.reduced_still_picture_header {
            s.frame_id_numbers_present_flag = r.flag()?;
        }
        if s.frame_id_numbers_present_flag {
            s.delta_frame_id_length_minus_2 = r.f(4)? as u8;
            s.additional_frame_id_length_minus_1 = r.f(3)? as u8;
        }
        s.use_128x128_superblock = r.flag()?;
        s.enable_filter_intra = r.flag()?;
        s.enable_intra_edge_filter = r.flag()?;
        if s.reduced_still_picture_header {
            s.seq_force_screen_content_tools = SELECT_SCREEN_CONTENT_TOOLS as u32;
            s.seq_force_integer_mv = SELECT_INTEGER_MV as u32;
        } else {
            s.enable_interintra_compound = r.flag()?;
            s.enable_masked_compound = r.flag()?;
            s.enable_warped_motion = r.flag()?;
            s.enable_dual_filter = r.flag()?;
            s.enable_order_hint = r.flag()?;
            if s.enable_order_hint {
                s.enable_jnt_comp = r.flag()?;
                s.enable_ref_frame_mvs = r.flag()?;
            }
            s.seq_choose_screen_content_tools = r.flag()?;
            s.seq_force_screen_content_tools =
                if s.seq_choose_screen_content_tools { SELECT_SCREEN_CONTENT_TOOLS as u32 } else { r.f(1)? };
            if s.seq_force_screen_content_tools > 0 {
                let seq_choose_integer_mv = r.flag()?;
                s.seq_force_integer_mv = if seq_choose_integer_mv { SELECT_INTEGER_MV as u32 } else { r.f(1)? };
            } else {
                s.seq_force_integer_mv = SELECT_INTEGER_MV as u32;
            }
            if s.enable_order_hint {
                s.order_hint_bits = r.f(3)? + 1;
            }
        }
        s.enable_superres = r.flag()?;
        s.enable_cdef = r.flag()?;
        s.enable_restoration = r.flag()?;
        s.color_config = parse_color_config(&mut r, s.seq_profile)?;
        s.film_grain_params_present = r.flag()?;
        Ok(s)
    }
}

fn parse_color_config(r: &mut BitReader, seq_profile: u8) -> Result<ColorConfig> {
    let mut c = ColorConfig { high_bitdepth: r.flag()?, ..Default::default() };
    if seq_profile == 2 && c.high_bitdepth {
        c.twelve_bit = r.flag()?;
        c.bit_depth = if c.twelve_bit { 12 } else { 10 };
    } else {
        c.bit_depth = if c.high_bitdepth { 10 } else { 8 };
    }
    c.mono_chrome = if seq_profile == 1 { false } else { r.flag()? };
    c.num_planes = if c.mono_chrome { 1 } else { 3 };
    c.color_description_present_flag = r.flag()?;
    if c.color_description_present_flag {
        c.color_primaries = r.f(8)? as u8;
        c.transfer_characteristics = r.f(8)? as u8;
        c.matrix_coefficients = r.f(8)? as u8;
    } else {
        c.color_primaries = CP_UNSPECIFIED as u8;
        c.transfer_characteristics = TC_UNSPECIFIED as u8;
        c.matrix_coefficients = MC_UNSPECIFIED as u8;
    }
    if c.mono_chrome {
        c.color_range = r.flag()?;
        c.subsampling_x = 1;
        c.subsampling_y = 1;
        c.chroma_sample_position = CSP_UNKNOWN as u8;
        c.separate_uv_delta_q = false;
        return Ok(c);
    } else if c.color_primaries == CP_BT_709 as u8
        && c.transfer_characteristics == TC_SRGB as u8
        && c.matrix_coefficients == MC_IDENTITY as u8
    {
        c.color_range = true;
        c.subsampling_x = 0;
        c.subsampling_y = 0;
    } else {
        c.color_range = r.flag()?;
        if seq_profile == 0 {
            c.subsampling_x = 1;
            c.subsampling_y = 1;
        } else if seq_profile == 1 {
            c.subsampling_x = 0;
            c.subsampling_y = 0;
        } else if c.bit_depth == 12 {
            c.subsampling_x = r.f(1)?;
            c.subsampling_y = if c.subsampling_x != 0 { r.f(1)? } else { 0 };
        } else {
            c.subsampling_x = 1;
            c.subsampling_y = 0;
        }
        if c.subsampling_x != 0 && c.subsampling_y != 0 {
            c.chroma_sample_position = r.f(2)? as u8;
        }
    }
    c.separate_uv_delta_q = r.flag()?;
    Ok(c)
}

/// §5.9.15 tile_info derived values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TileInfo {
    pub tile_cols_log2: u32,
    pub tile_rows_log2: u32,
    pub tile_cols: u32,
    pub tile_rows: u32,
    pub mi_col_starts: Vec<u32>,
    pub mi_row_starts: Vec<u32>,
    pub context_update_tile_id: u32,
    pub tile_size_bytes: u32,
    pub uniform_tile_spacing_flag: bool,
}

/// §5.9.18 film_grain_params (parsed and kept; synthesis is owed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilmGrainParams {
    pub apply_grain: bool,
    pub grain_seed: u16,
    pub update_grain: bool,
    pub num_y_points: u8,
    pub point_y_value: Vec<u8>,
    pub point_y_scaling: Vec<u8>,
    pub chroma_scaling_from_luma: bool,
    pub num_cb_points: u8,
    pub point_cb_value: Vec<u8>,
    pub point_cb_scaling: Vec<u8>,
    pub num_cr_points: u8,
    pub point_cr_value: Vec<u8>,
    pub point_cr_scaling: Vec<u8>,
    pub grain_scaling_minus_8: u8,
    pub ar_coeff_lag: u8,
    pub ar_coeffs_y_plus_128: Vec<u8>,
    pub ar_coeffs_cb_plus_128: Vec<u8>,
    pub ar_coeffs_cr_plus_128: Vec<u8>,
    pub ar_coeff_shift_minus_6: u8,
    pub grain_scale_shift: u8,
    pub cb_mult: u8,
    pub cb_luma_mult: u8,
    pub cb_offset: u16,
    pub cr_mult: u8,
    pub cr_luma_mult: u8,
    pub cr_offset: u16,
    pub overlap_flag: bool,
    pub clip_to_restricted_range: bool,
}

/// The uncompressed header (§5.9.2) of a KEY_FRAME or INTRA_ONLY_FRAME, plus the derived frame
/// variables the tile decode needs.
#[derive(Debug, Clone, Default)]
pub struct FrameHeader {
    pub show_existing_frame: bool,
    pub frame_to_show_map_idx: u8,
    pub frame_type: u8,
    pub frame_is_intra: bool,
    pub show_frame: bool,
    pub showable_frame: bool,
    pub error_resilient_mode: bool,
    pub disable_cdf_update: bool,
    pub allow_screen_content_tools: bool,
    pub force_integer_mv: bool,
    pub current_frame_id: u32,
    pub frame_size_override_flag: bool,
    pub order_hint: u32,
    pub primary_ref_frame: u32,
    pub refresh_frame_flags: u8,
    pub frame_width: u32,
    pub frame_height: u32,
    pub upscaled_width: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub use_superres: bool,
    pub superres_denom: u32,
    pub mi_cols: u32,
    pub mi_rows: u32,
    pub allow_intrabc: bool,
    pub disable_frame_end_update_cdf: bool,
    pub tile_info: TileInfo,
    // quantization_params
    pub base_q_idx: u32,
    pub delta_q_y_dc: i32,
    pub delta_q_u_dc: i32,
    pub delta_q_u_ac: i32,
    pub delta_q_v_dc: i32,
    pub delta_q_v_ac: i32,
    pub using_qmatrix: bool,
    pub qm_y: u32,
    pub qm_u: u32,
    pub qm_v: u32,
    // segmentation_params
    pub segmentation_enabled: bool,
    pub segmentation_update_map: bool,
    pub segmentation_temporal_update: bool,
    pub segmentation_update_data: bool,
    pub feature_enabled: [[bool; 8]; 8],
    pub feature_data: [[i32; 8]; 8],
    pub seg_id_pre_skip: bool,
    pub last_active_seg_id: u32,
    // delta q / lf
    pub delta_q_present: bool,
    pub delta_q_res: u32,
    pub delta_lf_present: bool,
    pub delta_lf_res: u32,
    pub delta_lf_multi: bool,
    // lossless
    pub coded_lossless: bool,
    pub all_lossless: bool,
    pub lossless_array: [bool; 8],
    pub seg_qm_level: [[u32; 8]; 3],
    // loop_filter_params
    pub loop_filter_level: [u32; 4],
    pub loop_filter_sharpness: u32,
    pub loop_filter_delta_enabled: bool,
    pub loop_filter_delta_update: bool,
    pub loop_filter_ref_deltas: [i32; 8],
    pub loop_filter_mode_deltas: [i32; 2],
    // cdef_params
    pub cdef_damping: u32,
    pub cdef_bits: u32,
    pub cdef_y_pri_strength: [u32; 8],
    pub cdef_y_sec_strength: [u32; 8],
    pub cdef_uv_pri_strength: [u32; 8],
    pub cdef_uv_sec_strength: [u32; 8],
    // lr_params
    pub frame_restoration_type: [u32; 3],
    pub uses_lr: bool,
    pub loop_restoration_size: [u32; 3],
    // tx mode etc.
    pub tx_mode: u32,
    pub reference_select: bool,
    pub skip_mode_present: bool,
    pub allow_warped_motion: bool,
    pub reduced_tx_set: bool,
    pub film_grain: FilmGrainParams,
    /// Byte length of the header within its OBU (after byte_alignment for OBU_FRAME).
    pub header_bytes: usize,
    // ---- inter half of uncompressed_header (§5.9.2) and derived variables
    pub display_frame_id: u32,
    pub frame_refs_short_signaling: bool,
    pub last_frame_idx: u32,
    pub gold_frame_idx: u32,
    /// ref_frame_idx[ i ] for i = 0..REFS_PER_FRAME-1 (LAST_FRAME..ALTREF_FRAME).
    pub ref_frame_idx: [usize; 7],
    /// OrderHints[ refFrame ] indexed by reference frame type (INTRA_FRAME..ALTREF_FRAME).
    pub order_hints: [u32; 8],
    pub ref_frame_sign_bias: [bool; 8],
    pub allow_high_precision_mv: bool,
    pub is_filter_switchable: bool,
    pub interpolation_filter: u8,
    pub is_motion_mode_switchable: bool,
    pub use_ref_frame_mvs: bool,
    pub skip_mode_frame: [usize; 2],
    pub gm_type: [u8; 8],
    pub gm_params: [[i32; 6]; 8],
    /// PrevGmParams (setup_past_independence / load_previous), kept for inspection.
    pub prev_gm_params: [[i32; 6]; 8],
    /// load_cdfs / load_previous source: ref_frame_idx[ primary_ref_frame ] when not NONE.
    pub prev_frame: Option<usize>,
}

impl FrameHeader {
    /// get_qindex( ignoreDeltaQ, segmentId ) with CurrentQIndex supplied by the caller (§7.12.2).
    pub fn get_qindex(&self, ignore_delta_q: bool, segment_id: usize, current_q_index: i32) -> i32 {
        if self.seg_feature_active_idx(segment_id, SEG_LVL_ALT_Q) {
            let data = self.feature_data[segment_id][SEG_LVL_ALT_Q];
            let mut qindex = self.base_q_idx as i32 + data;
            if !ignore_delta_q && self.delta_q_present {
                qindex = current_q_index + data;
            }
            return qindex.clamp(0, 255);
        }
        if !ignore_delta_q && self.delta_q_present {
            return current_q_index;
        }
        self.base_q_idx as i32
    }
    pub fn seg_feature_active_idx(&self, idx: usize, feature: usize) -> bool {
        self.segmentation_enabled && self.feature_enabled[idx][feature]
    }
}

fn tile_log2(blk_size: u32, target: u32) -> u32 {
    let mut k = 0;
    while (blk_size << k) < target {
        k += 1;
    }
    k
}

fn read_delta_q(r: &mut BitReader) -> Result<i32> {
    if r.flag()? { r.su(7) } else { Ok(0) }
}

/// Parse a frame_header_obu / the header part of a frame_obu with no reference frames available
/// (still images, the first key frame of a stream). Inter frames need [`parse_frame_header_with_refs`].
pub fn parse_frame_header(payload: &[u8], seq: &SequenceHeader, temporal_id: u8, spatial_id: u8) -> Result<FrameHeader> {
    let mut refs = RefStore::default();
    parse_frame_header_with_refs(payload, seq, temporal_id, spatial_id, &mut refs)
}

/// get_relative_dist( a, b ) (§5.9.3)
pub fn get_relative_dist(seq: &SequenceHeader, a: u32, b: u32) -> i32 {
    if !seq.enable_order_hint {
        return 0;
    }
    let diff = a as i32 - b as i32;
    let m = 1i32 << (seq.order_hint_bits - 1);
    (diff & (m - 1)) - (diff & m)
}

/// set_frame_refs (§7.8): compute ref_frame_idx from last_frame_idx and gold_frame_idx.
fn set_frame_refs(seq: &SequenceHeader, h: &mut FrameHeader, refs: &RefStore) {
    let mut idx = [-1i32; REFS_PER_FRAME];
    idx[LAST_FRAME - LAST_FRAME] = h.last_frame_idx as i32;
    idx[GOLDEN_FRAME - LAST_FRAME] = h.gold_frame_idx as i32;
    let mut used = [false; NUM_REF_FRAMES];
    used[h.last_frame_idx as usize] = true;
    used[h.gold_frame_idx as usize] = true;
    let cur_frame_hint = 1i32 << (seq.order_hint_bits - 1);
    let mut shifted = [0i32; NUM_REF_FRAMES];
    for i in 0..NUM_REF_FRAMES {
        shifted[i] = cur_frame_hint + get_relative_dist(seq, refs.order_hint[i], h.order_hint);
    }
    // ALTREF: latest backward
    {
        let mut r = -1i32;
        let mut latest = 0;
        for i in 0..NUM_REF_FRAMES {
            let hint = shifted[i];
            if !used[i] && hint >= cur_frame_hint && (r < 0 || hint >= latest) {
                r = i as i32;
                latest = hint;
            }
        }
        if r >= 0 {
            idx[ALTREF_FRAME - LAST_FRAME] = r;
            used[r as usize] = true;
        }
    }
    // BWDREF, then ALTREF2: earliest backward
    for rf in [BWDREF_FRAME, ALTREF2_FRAME] {
        let mut r = -1i32;
        let mut earliest = 0;
        for i in 0..NUM_REF_FRAMES {
            let hint = shifted[i];
            if !used[i] && hint >= cur_frame_hint && (r < 0 || hint < earliest) {
                r = i as i32;
                earliest = hint;
            }
        }
        if r >= 0 {
            idx[rf - LAST_FRAME] = r;
            used[r as usize] = true;
        }
    }
    // the rest: forward references in anti-chronological order
    for i in 0..REFS_PER_FRAME - 2 {
        let rf = REF_FRAME_LIST[i] as usize;
        if idx[rf - LAST_FRAME] < 0 {
            let mut r = -1i32;
            let mut latest = 0;
            for j in 0..NUM_REF_FRAMES {
                let hint = shifted[j];
                if !used[j] && hint < cur_frame_hint && (r < 0 || hint >= latest) {
                    r = j as i32;
                    latest = hint;
                }
            }
            if r >= 0 {
                idx[rf - LAST_FRAME] = r;
                used[r as usize] = true;
            }
        }
    }
    // finally anything left: the reference with the smallest output order
    let mut r = -1i32;
    let mut earliest = 0;
    for i in 0..NUM_REF_FRAMES {
        let hint = shifted[i];
        if r < 0 || hint < earliest {
            r = i as i32;
            earliest = hint;
        }
    }
    for i in 0..REFS_PER_FRAME {
        if idx[i] < 0 {
            idx[i] = r;
        }
        h.ref_frame_idx[i] = idx[i] as usize;
    }
}

/// uncompressed_header() (§5.9.2) for every frame type, using and updating the reference state
/// (RefValid, RefOrderHint, the saved frame sizes, loop-filter deltas, segmentation features,
/// global motion and film grain parameters of the reference slots).
pub fn parse_frame_header_with_refs(
    payload: &[u8],
    seq: &SequenceHeader,
    temporal_id: u8,
    spatial_id: u8,
    refs: &mut RefStore,
) -> Result<FrameHeader> {
    let mut r = BitReader::new(payload);
    let mut h = FrameHeader::default();
    let cc = &seq.color_config;
    let id_len = if seq.frame_id_numbers_present_flag {
        seq.additional_frame_id_length_minus_1 as u32 + seq.delta_frame_id_length_minus_2 as u32 + 3
    } else {
        0
    };
    let all_frames: u8 = 0xff;
    for rf in 0..8 {
        h.gm_params[rf] = [0, 0, 1 << WARPEDMODEL_PREC_BITS, 0, 0, 1 << WARPEDMODEL_PREC_BITS];
    }
    h.prev_gm_params = h.gm_params;
    if seq.reduced_still_picture_header {
        h.show_existing_frame = false;
        h.frame_type = KEY_FRAME as u8;
        h.frame_is_intra = true;
        h.show_frame = true;
        h.showable_frame = false;
    } else {
        h.show_existing_frame = r.flag()?;
        if h.show_existing_frame {
            h.frame_to_show_map_idx = r.f(3)? as u8;
            if seq.decoder_model_info_present_flag && !seq.timing_info.equal_picture_interval {
                r.f(seq.decoder_model_info.frame_presentation_time_length_minus_1 as u32 + 1)?;
            }
            h.refresh_frame_flags = 0;
            if seq.frame_id_numbers_present_flag {
                h.display_frame_id = r.f(id_len)?;
            }
            let idx = h.frame_to_show_map_idx as usize;
            let f = refs.frames[idx].as_ref().ok_or(Error::Invalid("show_existing_frame of an empty slot"))?;
            h.frame_type = f.frame_type;
            if h.frame_type == KEY_FRAME as u8 {
                h.refresh_frame_flags = all_frames;
            }
            if seq.film_grain_params_present {
                h.film_grain = f.film_grain.clone();
            }
            h.show_frame = true;
            h.frame_is_intra = h.frame_type == KEY_FRAME as u8 || h.frame_type == INTRA_ONLY_FRAME as u8;
            h.header_bytes = (r.position() + 7) >> 3;
            return Ok(h);
        }
        h.frame_type = r.f(2)? as u8;
        h.frame_is_intra = h.frame_type == INTRA_ONLY_FRAME as u8 || h.frame_type == KEY_FRAME as u8;
        h.show_frame = r.flag()?;
        if h.show_frame && seq.decoder_model_info_present_flag && !seq.timing_info.equal_picture_interval {
            // temporal_point_info
            r.f(seq.decoder_model_info.frame_presentation_time_length_minus_1 as u32 + 1)?;
        }
        if h.show_frame {
            h.showable_frame = h.frame_type != KEY_FRAME as u8;
        } else {
            h.showable_frame = r.flag()?;
        }
        if h.frame_type == SWITCH_FRAME as u8 || (h.frame_type == KEY_FRAME as u8 && h.show_frame) {
            h.error_resilient_mode = true;
        } else {
            h.error_resilient_mode = r.flag()?;
        }
    }
    if h.frame_type == KEY_FRAME as u8 && h.show_frame {
        for i in 0..NUM_REF_FRAMES {
            refs.valid[i] = false;
            refs.order_hint[i] = 0;
        }
        for i in 0..REFS_PER_FRAME {
            h.order_hints[LAST_FRAME + i] = 0;
        }
    }
    h.disable_cdf_update = r.flag()?;
    if seq.seq_force_screen_content_tools == SELECT_SCREEN_CONTENT_TOOLS as u32 {
        h.allow_screen_content_tools = r.flag()?;
    } else {
        h.allow_screen_content_tools = seq.seq_force_screen_content_tools != 0;
    }
    if h.allow_screen_content_tools {
        if seq.seq_force_integer_mv == SELECT_INTEGER_MV as u32 {
            h.force_integer_mv = r.flag()?;
        } else {
            h.force_integer_mv = seq.seq_force_integer_mv != 0;
        }
    } else {
        h.force_integer_mv = false;
    }
    if h.frame_is_intra {
        h.force_integer_mv = true;
    }
    if seq.frame_id_numbers_present_flag {
        h.current_frame_id = r.f(id_len)?;
        // mark_ref_frames( idLen )
        let diff_len = seq.delta_frame_id_length_minus_2 as u32 + 2;
        let cur = h.current_frame_id as i64;
        for i in 0..NUM_REF_FRAMES {
            let rid = refs.frame_id[i] as i64;
            if cur > (1i64 << diff_len) {
                if rid > cur || rid < cur - (1i64 << diff_len) {
                    refs.valid[i] = false;
                }
            } else if rid > cur && rid < (1i64 << id_len) + cur - (1i64 << diff_len) {
                refs.valid[i] = false;
            }
        }
    }
    if h.frame_type == SWITCH_FRAME as u8 {
        h.frame_size_override_flag = true;
    } else if seq.reduced_still_picture_header {
        h.frame_size_override_flag = false;
    } else {
        h.frame_size_override_flag = r.flag()?;
    }
    h.order_hint = r.f(seq.order_hint_bits)?;
    if h.frame_is_intra || h.error_resilient_mode {
        h.primary_ref_frame = PRIMARY_REF_NONE as u32;
    } else {
        h.primary_ref_frame = r.f(3)?;
    }
    if seq.decoder_model_info_present_flag {
        let buffer_removal_time_present_flag = r.flag()?;
        if buffer_removal_time_present_flag {
            for op in &seq.operating_points {
                if op.decoder_model_present_for_this_op {
                    let op_pt_idc = op.idc as u32;
                    let in_temporal_layer = (op_pt_idc >> temporal_id) & 1;
                    let in_spatial_layer = (op_pt_idc >> (spatial_id as u32 + 8)) & 1;
                    if op_pt_idc == 0 || (in_temporal_layer != 0 && in_spatial_layer != 0) {
                        r.f(seq.decoder_model_info.buffer_removal_time_length_minus_1 as u32 + 1)?;
                    }
                }
            }
        }
    }
    h.allow_high_precision_mv = false;
    h.use_ref_frame_mvs = false;
    h.allow_intrabc = false;
    if h.frame_type == SWITCH_FRAME as u8 || (h.frame_type == KEY_FRAME as u8 && h.show_frame) {
        h.refresh_frame_flags = all_frames;
    } else {
        h.refresh_frame_flags = r.f(8)? as u8;
    }
    if (!h.frame_is_intra || h.refresh_frame_flags != all_frames) && h.error_resilient_mode && seq.enable_order_hint {
        for i in 0..NUM_REF_FRAMES {
            let ref_order_hint = r.f(seq.order_hint_bits)?;
            if ref_order_hint != refs.order_hint[i] {
                refs.valid[i] = false;
            }
        }
    }
    if h.frame_is_intra {
        frame_size(&mut r, seq, &mut h)?;
        render_size(&mut r, &mut h)?;
        if h.allow_screen_content_tools && h.upscaled_width == h.frame_width {
            h.allow_intrabc = r.flag()?;
        }
    } else {
        if !seq.enable_order_hint {
            h.frame_refs_short_signaling = false;
        } else {
            h.frame_refs_short_signaling = r.flag()?;
            if h.frame_refs_short_signaling {
                h.last_frame_idx = r.f(3)?;
                h.gold_frame_idx = r.f(3)?;
                set_frame_refs(seq, &mut h, refs);
            }
        }
        for i in 0..REFS_PER_FRAME {
            if !h.frame_refs_short_signaling {
                h.ref_frame_idx[i] = r.f(3)? as usize;
            }
            if seq.frame_id_numbers_present_flag {
                let n = seq.delta_frame_id_length_minus_2 as u32 + 2;
                let _delta_frame_id_minus_1 = r.f(n)?;
            }
        }
        for i in 0..REFS_PER_FRAME {
            if refs.frames[h.ref_frame_idx[i]].is_none() {
                return Err(Error::Invalid("inter frame references an empty slot"));
            }
        }
        if h.frame_size_override_flag && !h.error_resilient_mode {
            // frame_size_with_refs()
            let mut found_ref = false;
            for i in 0..REFS_PER_FRAME {
                found_ref = r.flag()?;
                if found_ref {
                    let f = refs.frames[h.ref_frame_idx[i]].as_ref().unwrap();
                    h.upscaled_width = f.upscaled_width;
                    h.frame_width = h.upscaled_width;
                    h.frame_height = f.frame_height;
                    h.render_width = f.render_width;
                    h.render_height = f.render_height;
                    break;
                }
            }
            if !found_ref {
                frame_size(&mut r, seq, &mut h)?;
                render_size(&mut r, &mut h)?;
            } else {
                superres_params(&mut r, seq, &mut h)?;
                compute_image_size(&mut h);
            }
        } else {
            frame_size(&mut r, seq, &mut h)?;
            render_size(&mut r, &mut h)?;
        }
        if h.force_integer_mv {
            h.allow_high_precision_mv = false;
        } else {
            h.allow_high_precision_mv = r.flag()?;
        }
        // read_interpolation_filter()
        h.is_filter_switchable = r.flag()?;
        h.interpolation_filter = if h.is_filter_switchable { SWITCHABLE as u8 } else { r.f(2)? as u8 };
        h.is_motion_mode_switchable = r.flag()?;
        if h.error_resilient_mode || !seq.enable_ref_frame_mvs {
            h.use_ref_frame_mvs = false;
        } else {
            h.use_ref_frame_mvs = r.flag()?;
        }
        for i in 0..REFS_PER_FRAME {
            let ref_frame = LAST_FRAME + i;
            let hint = refs.order_hint[h.ref_frame_idx[i]];
            h.order_hints[ref_frame] = hint;
            h.ref_frame_sign_bias[ref_frame] = if !seq.enable_order_hint { false } else { get_relative_dist(seq, hint, h.order_hint) > 0 };
        }
    }
    if seq.reduced_still_picture_header || h.disable_cdf_update {
        h.disable_frame_end_update_cdf = true;
    } else {
        h.disable_frame_end_update_cdf = r.flag()?;
    }
    if h.primary_ref_frame == PRIMARY_REF_NONE as u32 {
        // init_non_coeff_cdfs() (by the caller) + setup_past_independence()
        h.prev_frame = None;
        h.loop_filter_delta_enabled = true;
        h.loop_filter_ref_deltas = [1, 0, 0, 0, -1, 0, -1, -1];
        h.loop_filter_mode_deltas = [0, 0];
        h.feature_enabled = [[false; 8]; 8];
        h.feature_data = [[0; 8]; 8];
    } else {
        // load_cdfs( ref_frame_idx[ primary_ref_frame ] ) (by the caller) + load_previous()
        let prev = h.ref_frame_idx[h.primary_ref_frame as usize];
        h.prev_frame = Some(prev);
        let f = refs.frames[prev].as_ref().ok_or(Error::Invalid("primary_ref_frame slot empty"))?;
        h.prev_gm_params = f.gm_params;
        h.loop_filter_ref_deltas = f.loop_filter_ref_deltas;
        h.loop_filter_mode_deltas = f.loop_filter_mode_deltas;
        h.feature_enabled = f.feature_enabled;
        h.feature_data = f.feature_data;
    }
    // (motion_field_estimation() runs in the decoder once the header is known)
    h.tile_info = tile_info(&mut r, seq, &h)?;
    // quantization_params()
    h.base_q_idx = r.f(8)?;
    h.delta_q_y_dc = read_delta_q(&mut r)?;
    if cc.num_planes > 1 {
        let diff_uv_delta = if cc.separate_uv_delta_q { r.flag()? } else { false };
        h.delta_q_u_dc = read_delta_q(&mut r)?;
        h.delta_q_u_ac = read_delta_q(&mut r)?;
        if diff_uv_delta {
            h.delta_q_v_dc = read_delta_q(&mut r)?;
            h.delta_q_v_ac = read_delta_q(&mut r)?;
        } else {
            h.delta_q_v_dc = h.delta_q_u_dc;
            h.delta_q_v_ac = h.delta_q_u_ac;
        }
    }
    h.using_qmatrix = r.flag()?;
    if h.using_qmatrix {
        h.qm_y = r.f(4)?;
        h.qm_u = r.f(4)?;
        h.qm_v = if !cc.separate_uv_delta_q { h.qm_u } else { r.f(4)? };
    }
    // segmentation_params()
    h.segmentation_enabled = r.flag()?;
    if h.segmentation_enabled {
        if h.primary_ref_frame == PRIMARY_REF_NONE as u32 {
            h.segmentation_update_map = true;
            h.segmentation_temporal_update = false;
            h.segmentation_update_data = true;
        } else {
            h.segmentation_update_map = r.flag()?;
            if h.segmentation_update_map {
                h.segmentation_temporal_update = r.flag()?;
            }
            h.segmentation_update_data = r.flag()?;
        }
        if h.segmentation_update_data {
            for i in 0..MAX_SEGMENTS {
                for j in 0..SEG_LVL_MAX {
                    let fe = r.flag()?;
                    h.feature_enabled[i][j] = fe;
                    let mut clipped = 0;
                    if fe {
                        let bits = SEGMENTATION_FEATURE_BITS[j] as u32;
                        let limit = SEGMENTATION_FEATURE_MAX[j] as i32;
                        if SEGMENTATION_FEATURE_SIGNED[j] == 1 {
                            clipped = r.su(1 + bits)?.clamp(-limit, limit);
                        } else {
                            clipped = (r.f(bits)? as i32).clamp(0, limit);
                        }
                    }
                    h.feature_data[i][j] = clipped;
                }
            }
        }
    } else {
        h.feature_enabled = [[false; 8]; 8];
        h.feature_data = [[0; 8]; 8];
    }
    h.seg_id_pre_skip = false;
    h.last_active_seg_id = 0;
    for i in 0..MAX_SEGMENTS {
        for j in 0..SEG_LVL_MAX {
            if h.feature_enabled[i][j] {
                h.last_active_seg_id = i as u32;
                if j >= SEG_LVL_REF_FRAME {
                    h.seg_id_pre_skip = true;
                }
            }
        }
    }
    // delta_q_params()
    if h.base_q_idx > 0 {
        h.delta_q_present = r.flag()?;
    }
    if h.delta_q_present {
        h.delta_q_res = r.f(2)?;
    }
    // delta_lf_params()
    if h.delta_q_present {
        if !h.allow_intrabc {
            h.delta_lf_present = r.flag()?;
        }
        if h.delta_lf_present {
            h.delta_lf_res = r.f(2)?;
            h.delta_lf_multi = r.flag()?;
        }
    }
    // CodedLossless / LosslessArray / SegQMLevel
    h.coded_lossless = true;
    for seg in 0..MAX_SEGMENTS {
        let qindex = h.get_qindex(true, seg, 0);
        let l = qindex == 0
            && h.delta_q_y_dc == 0
            && h.delta_q_u_ac == 0
            && h.delta_q_u_dc == 0
            && h.delta_q_v_ac == 0
            && h.delta_q_v_dc == 0;
        h.lossless_array[seg] = l;
        if !l {
            h.coded_lossless = false;
        }
        if h.using_qmatrix {
            if l {
                h.seg_qm_level[0][seg] = 15;
                h.seg_qm_level[1][seg] = 15;
                h.seg_qm_level[2][seg] = 15;
            } else {
                h.seg_qm_level[0][seg] = h.qm_y;
                h.seg_qm_level[1][seg] = h.qm_u;
                h.seg_qm_level[2][seg] = h.qm_v;
            }
        }
    }
    h.all_lossless = h.coded_lossless && h.frame_width == h.upscaled_width;
    // loop_filter_params()
    if h.coded_lossless || h.allow_intrabc {
        h.loop_filter_level[0] = 0;
        h.loop_filter_level[1] = 0;
        h.loop_filter_ref_deltas = [1, 0, 0, 0, -1, 0, -1, -1];
        h.loop_filter_mode_deltas = [0, 0];
    } else {
        h.loop_filter_level[0] = r.f(6)?;
        h.loop_filter_level[1] = r.f(6)?;
        if cc.num_planes > 1 && (h.loop_filter_level[0] != 0 || h.loop_filter_level[1] != 0) {
            h.loop_filter_level[2] = r.f(6)?;
            h.loop_filter_level[3] = r.f(6)?;
        }
        h.loop_filter_sharpness = r.f(3)?;
        h.loop_filter_delta_enabled = r.flag()?;
        if h.loop_filter_delta_enabled {
            h.loop_filter_delta_update = r.flag()?;
            if h.loop_filter_delta_update {
                for i in 0..TOTAL_REFS_PER_FRAME {
                    if r.flag()? {
                        h.loop_filter_ref_deltas[i] = r.su(7)?;
                    }
                }
                for i in 0..2 {
                    if r.flag()? {
                        h.loop_filter_mode_deltas[i] = r.su(7)?;
                    }
                }
            }
        }
    }
    // cdef_params()
    if h.coded_lossless || h.allow_intrabc || !seq.enable_cdef {
        h.cdef_bits = 0;
        h.cdef_y_pri_strength[0] = 0;
        h.cdef_y_sec_strength[0] = 0;
        h.cdef_uv_pri_strength[0] = 0;
        h.cdef_uv_sec_strength[0] = 0;
        h.cdef_damping = 3;
    } else {
        h.cdef_damping = r.f(2)? + 3;
        h.cdef_bits = r.f(2)?;
        for i in 0..(1usize << h.cdef_bits) {
            h.cdef_y_pri_strength[i] = r.f(4)?;
            h.cdef_y_sec_strength[i] = r.f(2)?;
            if h.cdef_y_sec_strength[i] == 3 {
                h.cdef_y_sec_strength[i] += 1;
            }
            if cc.num_planes > 1 {
                h.cdef_uv_pri_strength[i] = r.f(4)?;
                h.cdef_uv_sec_strength[i] = r.f(2)?;
                if h.cdef_uv_sec_strength[i] == 3 {
                    h.cdef_uv_sec_strength[i] += 1;
                }
            }
        }
    }
    // lr_params()
    if h.all_lossless || h.allow_intrabc || !seq.enable_restoration {
        h.frame_restoration_type = [RESTORE_NONE as u32; 3];
        h.uses_lr = false;
    } else {
        let mut uses_chroma_lr = false;
        for i in 0..cc.num_planes {
            let lr_type = r.f(2)? as usize;
            h.frame_restoration_type[i] = REMAP_LR_TYPE[lr_type] as u32;
            if h.frame_restoration_type[i] != RESTORE_NONE as u32 {
                h.uses_lr = true;
                if i > 0 {
                    uses_chroma_lr = true;
                }
            }
        }
        if h.uses_lr {
            let mut lr_unit_shift;
            if seq.use_128x128_superblock {
                lr_unit_shift = r.f(1)?;
                lr_unit_shift += 1;
            } else {
                lr_unit_shift = r.f(1)?;
                if lr_unit_shift != 0 {
                    lr_unit_shift += r.f(1)?;
                }
            }
            h.loop_restoration_size[0] = (RESTORATION_TILESIZE_MAX as u32) >> (2 - lr_unit_shift);
            let lr_uv_shift = if cc.subsampling_x != 0 && cc.subsampling_y != 0 && uses_chroma_lr { r.f(1)? } else { 0 };
            h.loop_restoration_size[1] = h.loop_restoration_size[0] >> lr_uv_shift;
            h.loop_restoration_size[2] = h.loop_restoration_size[0] >> lr_uv_shift;
        }
    }
    // read_tx_mode()
    if h.coded_lossless {
        h.tx_mode = ONLY_4X4 as u32;
    } else {
        h.tx_mode = if r.flag()? { TX_MODE_SELECT as u32 } else { TX_MODE_LARGEST as u32 };
    }
    // frame_reference_mode()
    h.reference_select = if h.frame_is_intra { false } else { r.flag()? };
    // skip_mode_params()
    let mut skip_mode_allowed = false;
    if !(h.frame_is_intra || !h.reference_select || !seq.enable_order_hint) {
        let mut forward_idx = -1i32;
        let mut backward_idx = -1i32;
        let (mut forward_hint, mut backward_hint) = (0u32, 0u32);
        for i in 0..REFS_PER_FRAME {
            let ref_hint = refs.order_hint[h.ref_frame_idx[i]];
            if get_relative_dist(seq, ref_hint, h.order_hint) < 0 {
                if forward_idx < 0 || get_relative_dist(seq, ref_hint, forward_hint) > 0 {
                    forward_idx = i as i32;
                    forward_hint = ref_hint;
                }
            } else if get_relative_dist(seq, ref_hint, h.order_hint) > 0 {
                if backward_idx < 0 || get_relative_dist(seq, ref_hint, backward_hint) < 0 {
                    backward_idx = i as i32;
                    backward_hint = ref_hint;
                }
            }
        }
        if forward_idx < 0 {
            skip_mode_allowed = false;
        } else if backward_idx >= 0 {
            skip_mode_allowed = true;
            h.skip_mode_frame[0] = LAST_FRAME + forward_idx.min(backward_idx) as usize;
            h.skip_mode_frame[1] = LAST_FRAME + forward_idx.max(backward_idx) as usize;
        } else {
            let mut second_forward_idx = -1i32;
            let mut second_forward_hint = 0u32;
            for i in 0..REFS_PER_FRAME {
                let ref_hint = refs.order_hint[h.ref_frame_idx[i]];
                if get_relative_dist(seq, ref_hint, forward_hint) < 0 {
                    if second_forward_idx < 0 || get_relative_dist(seq, ref_hint, second_forward_hint) > 0 {
                        second_forward_idx = i as i32;
                        second_forward_hint = ref_hint;
                    }
                }
            }
            if second_forward_idx >= 0 {
                skip_mode_allowed = true;
                h.skip_mode_frame[0] = LAST_FRAME + forward_idx.min(second_forward_idx) as usize;
                h.skip_mode_frame[1] = LAST_FRAME + forward_idx.max(second_forward_idx) as usize;
            }
        }
    }
    h.skip_mode_present = if skip_mode_allowed { r.flag()? } else { false };
    h.allow_warped_motion = if h.frame_is_intra || h.error_resilient_mode || !seq.enable_warped_motion { false } else { r.flag()? };
    h.reduced_tx_set = r.flag()?;
    global_motion_params(&mut r, &mut h)?;
    film_grain_params(&mut r, seq, &mut h, refs)?;
    h.header_bytes = (r.position() + 7) >> 3;
    Ok(h)
}

/// global_motion_params() (§5.9.24) with read_global_param (§5.9.25).
fn global_motion_params(r: &mut BitReader, h: &mut FrameHeader) -> Result<()> {
    for rf in LAST_FRAME..=ALTREF_FRAME {
        h.gm_type[rf] = IDENTITY as u8;
        h.gm_params[rf] = [0, 0, 1 << WARPEDMODEL_PREC_BITS, 0, 0, 1 << WARPEDMODEL_PREC_BITS];
    }
    if h.frame_is_intra {
        return Ok(());
    }
    for rf in LAST_FRAME..=ALTREF_FRAME {
        let is_global = r.flag()?;
        let typ = if is_global {
            if r.flag()? {
                ROTZOOM
            } else if r.flag()? {
                TRANSLATION
            } else {
                AFFINE
            }
        } else {
            IDENTITY
        };
        h.gm_type[rf] = typ as u8;
        if typ >= ROTZOOM {
            read_global_param(r, h, typ, rf, 2)?;
            read_global_param(r, h, typ, rf, 3)?;
            if typ == AFFINE {
                read_global_param(r, h, typ, rf, 4)?;
                read_global_param(r, h, typ, rf, 5)?;
            } else {
                h.gm_params[rf][4] = -h.gm_params[rf][3];
                h.gm_params[rf][5] = h.gm_params[rf][2];
            }
        }
        if typ >= TRANSLATION {
            read_global_param(r, h, typ, rf, 0)?;
            read_global_param(r, h, typ, rf, 1)?;
        }
    }
    Ok(())
}

fn read_global_param(r: &mut BitReader, h: &mut FrameHeader, typ: usize, rf: usize, idx: usize) -> Result<()> {
    let mut abs_bits = GM_ABS_ALPHA_BITS as u32;
    let mut prec_bits = GM_ALPHA_PREC_BITS as u32;
    if idx < 2 {
        if typ == TRANSLATION {
            let hp = !h.allow_high_precision_mv as u32;
            abs_bits = GM_ABS_TRANS_ONLY_BITS as u32 - hp;
            prec_bits = GM_TRANS_ONLY_PREC_BITS as u32 - hp;
        } else {
            abs_bits = GM_ABS_TRANS_BITS as u32;
            prec_bits = GM_TRANS_PREC_BITS as u32;
        }
    }
    let prec_diff = WARPEDMODEL_PREC_BITS as u32 - prec_bits;
    let round = if idx % 3 == 2 { 1i32 << WARPEDMODEL_PREC_BITS } else { 0 };
    let sub = if idx % 3 == 2 { 1i32 << prec_bits } else { 0 };
    let mx = 1i32 << abs_bits;
    let rr = (h.prev_gm_params[rf][idx] >> prec_diff) - sub;
    let v = decode_signed_subexp_with_ref(r, -mx, mx + 1, rr)?;
    h.gm_params[rf][idx] = (v << prec_diff) + round;
    Ok(())
}

fn decode_signed_subexp_with_ref(r: &mut BitReader, low: i32, high: i32, rr: i32) -> Result<i32> {
    let x = decode_unsigned_subexp_with_ref(r, high - low, rr - low)?;
    Ok(x + low)
}
fn decode_unsigned_subexp_with_ref(r: &mut BitReader, mx: i32, rr: i32) -> Result<i32> {
    let v = decode_subexp(r, mx)?;
    Ok(if (rr << 1) <= mx { crate::decode::inverse_recenter(rr, v) } else { mx - 1 - crate::decode::inverse_recenter(mx - 1 - rr, v) })
}
fn decode_subexp(r: &mut BitReader, num_syms: i32) -> Result<i32> {
    let mut i = 0u32;
    let mut mk = 0i32;
    let k = 3u32;
    loop {
        let b2 = if i != 0 { k + i - 1 } else { k };
        let a = 1i32 << b2;
        if num_syms <= mk + 3 * a {
            return Ok(r.ns((num_syms - mk) as u32)? as i32 + mk);
        } else if r.flag()? {
            i += 1;
            mk += a;
        } else {
            return Ok(r.f(b2)? as i32 + mk);
        }
    }
}

fn superres_params(r: &mut BitReader, seq: &SequenceHeader, h: &mut FrameHeader) -> Result<()> {
    h.use_superres = if seq.enable_superres { r.flag()? } else { false };
    if h.use_superres {
        h.superres_denom = r.f(SUPERRES_DENOM_BITS as u32)? + SUPERRES_DENOM_MIN as u32;
    } else {
        h.superres_denom = SUPERRES_NUM as u32;
    }
    h.upscaled_width = h.frame_width;
    h.frame_width = (h.upscaled_width * SUPERRES_NUM as u32 + (h.superres_denom / 2)) / h.superres_denom;
    Ok(())
}

fn frame_size(r: &mut BitReader, seq: &SequenceHeader, h: &mut FrameHeader) -> Result<()> {
    if h.frame_size_override_flag {
        h.frame_width = r.f(seq.frame_width_bits_minus_1 as u32 + 1)? + 1;
        h.frame_height = r.f(seq.frame_height_bits_minus_1 as u32 + 1)? + 1;
    } else {
        h.frame_width = seq.max_frame_width_minus_1 + 1;
        h.frame_height = seq.max_frame_height_minus_1 + 1;
    }
    superres_params(r, seq, h)?;
    compute_image_size(h);
    Ok(())
}

/// compute_image_size() (§5.9.9)
fn compute_image_size(h: &mut FrameHeader) {
    h.mi_cols = 2 * ((h.frame_width + 7) >> 3);
    h.mi_rows = 2 * ((h.frame_height + 7) >> 3);
}

fn render_size(r: &mut BitReader, h: &mut FrameHeader) -> Result<()> {
    if r.flag()? {
        h.render_width = r.f(16)? + 1;
        h.render_height = r.f(16)? + 1;
    } else {
        h.render_width = h.upscaled_width;
        h.render_height = h.frame_height;
    }
    Ok(())
}

fn tile_info(r: &mut BitReader, seq: &SequenceHeader, h: &FrameHeader) -> Result<TileInfo> {
    let mut t = TileInfo::default();
    let (mi_cols, mi_rows) = (h.mi_cols, h.mi_rows);
    let sb128 = seq.use_128x128_superblock;
    let sb_cols = if sb128 { (mi_cols + 31) >> 5 } else { (mi_cols + 15) >> 4 };
    let sb_rows = if sb128 { (mi_rows + 31) >> 5 } else { (mi_rows + 15) >> 4 };
    let sb_shift = if sb128 { 5 } else { 4 };
    let sb_size = sb_shift + 2;
    let max_tile_width_sb = (MAX_TILE_WIDTH as u32) >> sb_size;
    let mut max_tile_area_sb = (MAX_TILE_AREA as u32) >> (2 * sb_size);
    let min_log2_tile_cols = tile_log2(max_tile_width_sb, sb_cols);
    let max_log2_tile_cols = tile_log2(1, sb_cols.min(MAX_TILE_COLS as u32));
    let max_log2_tile_rows = tile_log2(1, sb_rows.min(MAX_TILE_ROWS as u32));
    let min_log2_tiles = min_log2_tile_cols.max(tile_log2(max_tile_area_sb, sb_rows * sb_cols));
    t.uniform_tile_spacing_flag = r.flag()?;
    if t.uniform_tile_spacing_flag {
        t.tile_cols_log2 = min_log2_tile_cols;
        while t.tile_cols_log2 < max_log2_tile_cols {
            if r.flag()? { t.tile_cols_log2 += 1 } else { break }
        }
        let tile_width_sb = (sb_cols + (1 << t.tile_cols_log2) - 1) >> t.tile_cols_log2;
        let mut start_sb = 0;
        while start_sb < sb_cols {
            t.mi_col_starts.push(start_sb << sb_shift);
            start_sb += tile_width_sb;
        }
        t.tile_cols = t.mi_col_starts.len() as u32;
        t.mi_col_starts.push(mi_cols);
        let min_log2_tile_rows = min_log2_tiles.saturating_sub(t.tile_cols_log2);
        t.tile_rows_log2 = min_log2_tile_rows;
        while t.tile_rows_log2 < max_log2_tile_rows {
            if r.flag()? { t.tile_rows_log2 += 1 } else { break }
        }
        let tile_height_sb = (sb_rows + (1 << t.tile_rows_log2) - 1) >> t.tile_rows_log2;
        let mut start_sb = 0;
        while start_sb < sb_rows {
            t.mi_row_starts.push(start_sb << sb_shift);
            start_sb += tile_height_sb;
        }
        t.tile_rows = t.mi_row_starts.len() as u32;
        t.mi_row_starts.push(mi_rows);
    } else {
        let mut widest_tile_sb = 0;
        let mut start_sb = 0;
        while start_sb < sb_cols {
            t.mi_col_starts.push(start_sb << sb_shift);
            let max_width = (sb_cols - start_sb).min(max_tile_width_sb);
            let size_sb = r.ns(max_width)? + 1;
            widest_tile_sb = widest_tile_sb.max(size_sb);
            start_sb += size_sb;
        }
        t.tile_cols = t.mi_col_starts.len() as u32;
        t.mi_col_starts.push(mi_cols);
        t.tile_cols_log2 = tile_log2(1, t.tile_cols);
        if min_log2_tiles > 0 {
            max_tile_area_sb = (sb_rows * sb_cols) >> (min_log2_tiles + 1);
        } else {
            max_tile_area_sb = sb_rows * sb_cols;
        }
        let max_tile_height_sb = (max_tile_area_sb / widest_tile_sb).max(1);
        let mut start_sb = 0;
        while start_sb < sb_rows {
            t.mi_row_starts.push(start_sb << sb_shift);
            let max_height = (sb_rows - start_sb).min(max_tile_height_sb);
            let size_sb = r.ns(max_height)? + 1;
            start_sb += size_sb;
        }
        t.tile_rows = t.mi_row_starts.len() as u32;
        t.mi_row_starts.push(mi_rows);
        t.tile_rows_log2 = tile_log2(1, t.tile_rows);
    }
    if t.tile_cols_log2 > 0 || t.tile_rows_log2 > 0 {
        t.context_update_tile_id = r.f(t.tile_rows_log2 + t.tile_cols_log2)?;
        t.tile_size_bytes = r.f(2)? + 1;
    } else {
        t.context_update_tile_id = 0;
    }
    Ok(t)
}

fn film_grain_params(r: &mut BitReader, seq: &SequenceHeader, h: &mut FrameHeader, refs: &RefStore) -> Result<()> {
    let mut g = FilmGrainParams::default();
    if !seq.film_grain_params_present || (!h.show_frame && !h.showable_frame) {
        h.film_grain = g;
        return Ok(());
    }
    g.apply_grain = r.flag()?;
    if !g.apply_grain {
        h.film_grain = FilmGrainParams::default();
        return Ok(());
    }
    g.grain_seed = r.f(16)? as u16;
    g.update_grain = if h.frame_type == INTER_FRAME as u8 { r.flag()? } else { true };
    if !g.update_grain {
        let film_grain_params_ref_idx = r.f(3)? as usize;
        let temp_grain_seed = g.grain_seed;
        // load_grain_params( film_grain_params_ref_idx )
        let f = refs.frames[film_grain_params_ref_idx].as_ref().ok_or(Error::Invalid("film_grain_params_ref_idx slot empty"))?;
        g = f.film_grain.clone();
        g.grain_seed = temp_grain_seed;
        h.film_grain = g;
        return Ok(());
    }
    g.num_y_points = r.f(4)? as u8;
    for _ in 0..g.num_y_points {
        g.point_y_value.push(r.f(8)? as u8);
        g.point_y_scaling.push(r.f(8)? as u8);
    }
    let cc = &seq.color_config;
    g.chroma_scaling_from_luma = if cc.mono_chrome { false } else { r.flag()? };
    if cc.mono_chrome || g.chroma_scaling_from_luma || (cc.subsampling_x == 1 && cc.subsampling_y == 1 && g.num_y_points == 0) {
        g.num_cb_points = 0;
        g.num_cr_points = 0;
    } else {
        g.num_cb_points = r.f(4)? as u8;
        for _ in 0..g.num_cb_points {
            g.point_cb_value.push(r.f(8)? as u8);
            g.point_cb_scaling.push(r.f(8)? as u8);
        }
        g.num_cr_points = r.f(4)? as u8;
        for _ in 0..g.num_cr_points {
            g.point_cr_value.push(r.f(8)? as u8);
            g.point_cr_scaling.push(r.f(8)? as u8);
        }
    }
    g.grain_scaling_minus_8 = r.f(2)? as u8;
    g.ar_coeff_lag = r.f(2)? as u8;
    let num_pos_luma = 2 * g.ar_coeff_lag as u32 * (g.ar_coeff_lag as u32 + 1);
    let num_pos_chroma;
    if g.num_y_points != 0 {
        num_pos_chroma = num_pos_luma + 1;
        for _ in 0..num_pos_luma {
            g.ar_coeffs_y_plus_128.push(r.f(8)? as u8);
        }
    } else {
        num_pos_chroma = num_pos_luma;
    }
    if g.chroma_scaling_from_luma || g.num_cb_points != 0 {
        for _ in 0..num_pos_chroma {
            g.ar_coeffs_cb_plus_128.push(r.f(8)? as u8);
        }
    }
    if g.chroma_scaling_from_luma || g.num_cr_points != 0 {
        for _ in 0..num_pos_chroma {
            g.ar_coeffs_cr_plus_128.push(r.f(8)? as u8);
        }
    }
    g.ar_coeff_shift_minus_6 = r.f(2)? as u8;
    g.grain_scale_shift = r.f(2)? as u8;
    if g.num_cb_points != 0 {
        g.cb_mult = r.f(8)? as u8;
        g.cb_luma_mult = r.f(8)? as u8;
        g.cb_offset = r.f(9)? as u16;
    }
    if g.num_cr_points != 0 {
        g.cr_mult = r.f(8)? as u8;
        g.cr_luma_mult = r.f(8)? as u8;
        g.cr_offset = r.f(9)? as u16;
    }
    g.overlap_flag = r.flag()?;
    g.clip_to_restricted_range = r.flag()?;
    h.film_grain = g;
    Ok(())
}

/// Silence an unused import when building without tests.
#[allow(dead_code)]
fn _fl(x: u32) -> u32 {
    floor_log2(x)
}
