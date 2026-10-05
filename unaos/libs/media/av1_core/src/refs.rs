//! The reference frame store: everything §7.20 (reference frame update) saves per slot and §7.21
//! (reference frame loading) / load_previous / load_cdfs / load_grain_params read back.
//!
//! A slot holds an `Arc` so `refresh_frame_flags` with several bits set stores one frame in
//! several slots without copying it.

use crate::cdf::CdfContext;
use crate::decode::Plane;
use crate::obu::FilmGrainParams;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// A motion vector [row, col] in units of 1/8 luma sample.
pub type Mv = [i32; 2];

/// One saved reference frame (§7.20).
pub struct RefFrame {
    pub frame_type: u8,
    pub upscaled_width: u32,
    pub frame_width: u32,
    pub frame_height: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub mi_cols: u32,
    pub mi_rows: u32,
    pub subsampling_x: u32,
    pub subsampling_y: u32,
    pub bit_depth: u32,
    pub order_hint: u32,
    /// SavedOrderHints[ i ][ refFrame ] indexed by reference frame type.
    pub saved_order_hints: [u32; 8],
    /// FrameStore[ i ]: the final (post loop restoration, pre film grain) planes.
    pub planes: [Plane; 3],
    /// SavedRefFrames / SavedMvs (the motion field motion vector storage process, §7.19).
    pub mf_ref_frames: Vec<i8>,
    pub mf_mvs: Vec<Mv>,
    pub gm_params: [[i32; 6]; 8],
    pub segment_ids: Vec<u8>,
    pub cdfs: Box<CdfContext>,
    pub film_grain: FilmGrainParams,
    pub loop_filter_ref_deltas: [i32; 8],
    pub loop_filter_mode_deltas: [i32; 2],
    pub feature_enabled: [[bool; 8]; 8],
    pub feature_data: [[i32; 8]; 8],
    pub showable_frame: bool,
}

/// RefValid / RefOrderHint / RefFrameId and the saved frames of the NUM_REF_FRAMES slots.
#[derive(Default, Clone)]
pub struct RefStore {
    pub valid: [bool; 8],
    pub order_hint: [u32; 8],
    pub frame_id: [u32; 8],
    pub frames: [Option<Arc<RefFrame>>; 8],
}

impl RefStore {
    /// Drop every reference (a seek, or a new coded video sequence).
    pub fn clear(&mut self) {
        *self = RefStore::default();
    }
}
