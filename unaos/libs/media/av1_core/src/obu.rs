//! §5.3–§5.9 (syntax) and §6.2–§6.8 (semantics): OBU framing, the sequence header, and the
//! uncompressed frame header including tile info. Field names are the spec's syntax element
//! names; derived variables keep their CamelCase spelling in snake_case.

use crate::bits::{BitReader, floor_log2};
use crate::tables::*;
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

/// Parse a frame_header_obu / the header part of a frame_obu. Only intra frames are supported
/// (inter frames return `Unsupported("inter frame")`).
pub fn parse_frame_header(payload: &[u8], seq: &SequenceHeader, temporal_id: u8, spatial_id: u8) -> Result<FrameHeader> {
    let mut r = BitReader::new(payload);
    let mut h = FrameHeader::default();
    let cc = &seq.color_config;
    let id_len = if seq.frame_id_numbers_present_flag {
        seq.additional_frame_id_length_minus_1 as u32 + seq.delta_frame_id_length_minus_2 as u32 + 3
    } else {
        0
    };
    let all_frames: u8 = 0xff;
    if seq.reduced_still_picture_header {
        h.show_existing_frame = false;
        h.frame_type = KEY_FRAME as u8;
        h.frame_is_intra = true;
        h.show_frame = true;
        h.showable_frame = false;
    } else {
        h.show_existing_frame = r.flag()?;
        if h.show_existing_frame {
            // A shown copy of a stored frame: needs the reference store (owed with inter).
            return Err(Error::Unsupported("show_existing_frame"));
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
    if !h.frame_is_intra {
        return Err(Error::Unsupported("inter frame"));
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
    }
    if h.frame_is_intra {
        h.force_integer_mv = true;
    }
    if seq.frame_id_numbers_present_flag {
        h.current_frame_id = r.f(id_len)?;
        // mark_ref_frames only affects RefValid (reference store, owed with inter).
    }
    if h.frame_type == SWITCH_FRAME as u8 {
        h.frame_size_override_flag = true;
    } else if seq.reduced_still_picture_header {
        h.frame_size_override_flag = false;
    } else {
        h.frame_size_override_flag = r.flag()?;
    }
    h.order_hint = r.f(seq.order_hint_bits)?;
    h.primary_ref_frame = PRIMARY_REF_NONE as u32; // FrameIsIntra
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
    if h.frame_type == SWITCH_FRAME as u8 || (h.frame_type == KEY_FRAME as u8 && h.show_frame) {
        h.refresh_frame_flags = all_frames;
    } else {
        h.refresh_frame_flags = r.f(8)? as u8;
    }
    if (!h.frame_is_intra || h.refresh_frame_flags != all_frames) && h.error_resilient_mode && seq.enable_order_hint {
        for _ in 0..NUM_REF_FRAMES {
            r.f(seq.order_hint_bits)?; // ref_order_hint[i]
        }
    }
    // FrameIsIntra: frame_size(), render_size(), allow_intrabc
    frame_size(&mut r, seq, &mut h)?;
    render_size(&mut r, &mut h)?;
    if h.allow_screen_content_tools && h.upscaled_width == h.frame_width {
        h.allow_intrabc = r.flag()?;
    }
    if seq.reduced_still_picture_header || h.disable_cdf_update {
        h.disable_frame_end_update_cdf = true;
    } else {
        h.disable_frame_end_update_cdf = r.flag()?;
    }
    // primary_ref_frame == NONE: init_non_coeff_cdfs() + setup_past_independence()
    h.loop_filter_delta_enabled = true;
    h.loop_filter_ref_deltas = [1, 0, 0, 0, -1, 0, -1, -1];
    h.loop_filter_mode_deltas = [0, 0];
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
        // primary_ref_frame == PRIMARY_REF_NONE
        h.segmentation_update_map = true;
        h.segmentation_temporal_update = false;
        h.segmentation_update_data = true;
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
    // frame_reference_mode(): FrameIsIntra -> reference_select = 0
    // skip_mode_params(): FrameIsIntra -> skip_mode_present = 0
    // allow_warped_motion = 0 (FrameIsIntra)
    h.reduced_tx_set = r.flag()?;
    // global_motion_params(): FrameIsIntra -> identity, nothing read
    film_grain_params(&mut r, seq, &mut h)?;
    h.header_bytes = (r.position() + 7) >> 3;
    Ok(h)
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
    // compute_image_size()
    h.mi_cols = 2 * ((h.frame_width + 7) >> 3);
    h.mi_rows = 2 * ((h.frame_height + 7) >> 3);
    Ok(())
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

fn film_grain_params(r: &mut BitReader, seq: &SequenceHeader, h: &mut FrameHeader) -> Result<()> {
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
        return Err(Error::Unsupported("film grain load_grain_params"));
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
