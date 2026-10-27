//! Frame orchestration (§7.4 decode_frame_wrapup for intra frames: deblock → CDEF → loop
//! restoration → output), the `av01` item decode `decode_item_planes` (the container read by demux_core's HEIF
//! reader), and `planes_to_rgba`, which converts the decoded Y'CbCr planes to 8-bit RGBA using the AVIF `colr`/nclx
//! (or sequence header) matrix.

use crate::cdf::CdfContext;
use crate::decode::{Dec, FrameState, ToolStats};
use crate::obu::*;
use crate::refs::{RefFrame, RefStore};
use crate::tables::*;
use crate::{Error, Result};
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

/// Post-filter switches. A conformant decode uses `Filters::default()` (all on); the oracle
/// harness switches them off one by one to show each filter's effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filters {
    pub deblock: bool,
    pub cdef: bool,
    pub restoration: bool,
    /// §7.18.3 film grain synthesis on the output frames (off = the intermediate frames of §7.18.2).
    pub film_grain: bool,
}
impl Default for Filters {
    fn default() -> Self {
        Filters { deblock: true, cdef: true, restoration: true, film_grain: true }
    }
}

/// Decoded planes of one frame (the spec's OutY / OutU / OutV, §7.18.2).
#[derive(Debug, Clone)]
pub struct Planes {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u32,
    pub mono: bool,
    pub ss_x: u32,
    pub ss_y: u32,
    /// Row-major, `width` samples per row (chroma: `(width + ss_x) >> ss_x`).
    pub y: Vec<u16>,
    pub u: Vec<u16>,
    pub v: Vec<u16>,
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub full_range: bool,
    pub seq: SequenceHeader,
    pub frame: FrameHeader,
    pub stats: ToolStats,
}

impl Planes {
    pub fn chroma_width(&self) -> u32 {
        (self.width + self.ss_x) >> self.ss_x
    }
    pub fn chroma_height(&self) -> u32 {
        (self.height + self.ss_y) >> self.ss_y
    }
}

/// An RGBA image (8 bits per channel, row-major, `w * h * 4` bytes) — the `pixel_core` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Decode the first shown frame of an AV1 OBU stream. `config_obus` (from `av1C`) supply
/// the sequence header when the stream itself lacks one.
pub fn decode_obus(data: &[u8], config_obus: &[u8], filters: Filters) -> Result<Planes> {
    let mut d = Decoder::new();
    d.filters = filters;
    if !config_obus.is_empty() {
        for o in split_obus(config_obus)? {
            if o.obu_type == OBU_SEQUENCE_HEADER_T {
                d.seq = Some(SequenceHeader::parse(o.payload)?);
            }
        }
    }
    let mut frames = d.decode(data, true)?;
    if frames.is_empty() {
        return Err(Error::Invalid("no shown frame"));
    }
    Ok(frames.swap_remove(0))
}

/// The general decoding process (§7.2) over a sequence of OBUs: the sequence header, the
/// reference frame store, and every frame type (key, intra-only, inter, switch,
/// show_existing_frame).
#[derive(Clone, Default)]
pub struct Decoder {
    pub seq: Option<SequenceHeader>,
    pub refs: RefStore,
    pub filters: Filters,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder { seq: None, refs: RefStore::default(), filters: Filters::default() }
    }

    /// Decode every OBU in `data`; returns the shown frames in output order. With
    /// `first_only` it stops after the first shown frame.
    pub fn decode(&mut self, data: &[u8], first_only: bool) -> Result<Vec<Planes>> {
        let obus = split_obus(data)?;
        let mut out = Vec::new();
        let mut i = 0;
        while i < obus.len() {
            let o = obus[i];
            i += 1;
            match o.obu_type {
                OBU_SEQUENCE_HEADER_T => {
                    let s = SequenceHeader::parse(o.payload)?;
                    self.seq = Some(s);
                }
                OBU_FRAME_T | OBU_FRAME_HEADER_T => {
                    let s = self.seq.clone().ok_or(Error::Invalid("frame before sequence header"))?;
                    if s.operating_point_idc != 0 && o.has_extension {
                        let in_t = (s.operating_point_idc >> o.temporal_id) & 1;
                        let in_s = (s.operating_point_idc >> (o.spatial_id + 8)) & 1;
                        if in_t == 0 || in_s == 0 {
                            continue;
                        }
                    }
                    let hdr = parse_frame_header_with_refs(o.payload, &s, o.temporal_id, o.spatial_id, &mut self.refs)?;
                    if hdr.show_existing_frame {
                        out.push(self.show_existing(&s, &hdr)?);
                        if first_only {
                            return Ok(out);
                        }
                        continue;
                    }
                    // collect the tile groups of this frame
                    let mut groups: Vec<&[u8]> = Vec::new();
                    let mut done = false;
                    if o.obu_type == OBU_FRAME_T {
                        let g = &o.payload[hdr.header_bytes..];
                        done = tile_group_ends_frame(g, &hdr)?;
                        groups.push(g);
                    }
                    while !done {
                        let t = obus.get(i).ok_or(Error::Truncated)?;
                        i += 1;
                        if t.obu_type == OBU_TILE_GROUP_T {
                            done = tile_group_ends_frame(t.payload, &hdr)?;
                            groups.push(t.payload);
                        } else if t.obu_type == OBU_REDUNDANT_FRAME_HEADER_T || t.obu_type == OBU_METADATA_T || t.obu_type == OBU_PADDING_T {
                            continue;
                        } else {
                            return Err(Error::Invalid("missing tile group"));
                        }
                    }
                    if let Some(p) = self.decode_frame(&s, &hdr, &groups)? {
                        out.push(p);
                        if first_only {
                            return Ok(out);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// show_existing_frame (§7.21 when it is a key frame, then §7.20 and the output process).
    fn show_existing(&mut self, seq: &SequenceHeader, hdr: &FrameHeader) -> Result<Planes> {
        let idx = hdr.frame_to_show_map_idx as usize;
        let f = self.refs.frames[idx].clone().ok_or(Error::Invalid("show_existing_frame of an empty slot"))?;
        if hdr.frame_type == KEY_FRAME as u8 {
            // reference frame loading process + update with refresh_frame_flags = allFrames
            let order_hint = self.refs.order_hint[idx];
            let frame_id = self.refs.frame_id[idx];
            for i in 0..NUM_REF_FRAMES {
                self.refs.valid[i] = true;
                self.refs.order_hint[i] = order_hint;
                self.refs.frame_id[i] = frame_id;
                self.refs.frames[i] = Some(f.clone());
            }
        }
        let mut fh = hdr.clone();
        fh.upscaled_width = f.upscaled_width;
        fh.frame_width = f.frame_width;
        fh.frame_height = f.frame_height;
        fh.render_width = f.render_width;
        fh.render_height = f.render_height;
        fh.order_hint = f.order_hint;
        Ok(output_planes(seq, &fh, &f.planes, ToolStats::default(), self.filters))
    }

    /// Decode one frame (header already parsed) from its tile groups; returns the output if shown.
    fn decode_frame(&mut self, seq: &SequenceHeader, hdr: &FrameHeader, groups: &[&[u8]]) -> Result<Option<Planes>> {
        let mut fs = FrameState::new(seq, hdr);
        // the frame CDFs: init_non_coeff_cdfs + init_coeff_cdfs, or load_cdfs(prev)
        let frame_cdf = match hdr.prev_frame {
            None => CdfContext::new(hdr.base_q_idx),
            Some(prev) => {
                let f = self.refs.frames[prev].as_ref().ok_or(Error::Invalid("primary_ref_frame slot empty"))?;
                let mut c = (*f.cdfs).clone();
                c.reset_counts();
                c
            }
        };
        // PrevSegmentIds: setup_past_independence / load_previous_segment_ids
        if let Some(prev) = hdr.prev_frame {
            let f = self.refs.frames[prev].as_ref().unwrap();
            if hdr.segmentation_enabled && f.mi_cols == hdr.mi_cols && f.mi_rows == hdr.mi_rows {
                fs.prev_segment_ids.copy_from_slice(&f.segment_ids);
            }
        }
        if hdr.use_ref_frame_mvs {
            crate::mvpred::motion_field_estimation(seq, hdr, &self.refs, &mut fs);
        }
        let saved_cdf;
        {
            let refs = &self.refs;
            let mut dec = Dec::new(seq, hdr, &mut fs, self.filters, refs, frame_cdf.clone());
            dec.init_lr();
            for g in groups {
                dec.decode_tile_group(g)?;
            }
            saved_cdf = dec.saved_cdf.take();
        }
        // frame_end_update_cdf (§7.4)
        let final_cdf = if !hdr.disable_frame_end_update_cdf { saved_cdf.unwrap_or(frame_cdf) } else { frame_cdf };
        // decode_frame_wrapup (§7.4): post filters
        let lr = postfilter(hdr, &mut fs, self.filters);
        // motion field motion vector storage (§7.19)
        let n = fs.mi_rows * fs.mi_cols;
        let mut mf_ref_frames = vec![NONE as i8; n];
        let mut mf_mvs = vec![[0i32; 2]; n];
        for k in 0..n {
            for list in 0..2 {
                let r = fs.ref_frames[k][list] as i32;
                if r > INTRA_FRAME as i32 {
                    let ref_idx = hdr.ref_frame_idx[r as usize - LAST_FRAME];
                    let dist = get_relative_dist(seq, self.refs.order_hint[ref_idx], hdr.order_hint);
                    if dist < 0 {
                        let mv = fs.mvs[k][list];
                        if mv[0].abs() <= REFMVS_LIMIT as i32 && mv[1].abs() <= REFMVS_LIMIT as i32 {
                            mf_ref_frames[k] = r as i8;
                            mf_mvs[k] = mv;
                        }
                    }
                }
            }
        }
        if hdr.segmentation_enabled && !hdr.segmentation_update_map {
            fs.segment_ids.copy_from_slice(&fs.prev_segment_ids);
        }
        let cc = &seq.color_config;
        let stats = fs.stats.clone();
        let out = if hdr.show_frame { Some(output_planes(seq, hdr, &lr, stats, self.filters)) } else { None };
        // reference frame update process (§7.20)
        if hdr.refresh_frame_flags != 0 {
            let rf = Arc::new(RefFrame {
                frame_type: hdr.frame_type,
                upscaled_width: hdr.upscaled_width,
                frame_width: hdr.frame_width,
                frame_height: hdr.frame_height,
                render_width: hdr.render_width,
                render_height: hdr.render_height,
                mi_cols: hdr.mi_cols,
                mi_rows: hdr.mi_rows,
                subsampling_x: cc.subsampling_x,
                subsampling_y: cc.subsampling_y,
                bit_depth: cc.bit_depth,
                order_hint: hdr.order_hint,
                saved_order_hints: hdr.order_hints,
                planes: lr,
                mf_ref_frames,
                mf_mvs,
                gm_params: hdr.gm_params,
                segment_ids: core::mem::take(&mut fs.segment_ids),
                cdfs: Box::new(final_cdf),
                film_grain: hdr.film_grain.clone(),
                loop_filter_ref_deltas: hdr.loop_filter_ref_deltas,
                loop_filter_mode_deltas: hdr.loop_filter_mode_deltas,
                feature_enabled: hdr.feature_enabled,
                feature_data: hdr.feature_data,
                showable_frame: hdr.showable_frame,
            });
            for i in 0..NUM_REF_FRAMES {
                if (hdr.refresh_frame_flags >> i) & 1 == 1 {
                    self.refs.valid[i] = true;
                    self.refs.frame_id[i] = hdr.current_frame_id;
                    self.refs.order_hint[i] = hdr.order_hint;
                    self.refs.frames[i] = Some(rf.clone());
                }
            }
        }
        Ok(out)
    }
}

/// Whether a tile group's tg_end is the last tile (it then ends the frame, §5.11.1).
fn tile_group_ends_frame(g: &[u8], hdr: &FrameHeader) -> Result<bool> {
    let ti = &hdr.tile_info;
    let num_tiles = ti.tile_cols * ti.tile_rows;
    let mut r = crate::bits::BitReader::new(g);
    let mut flag = false;
    if num_tiles > 1 {
        flag = r.flag()?;
    }
    let end = if num_tiles == 1 || !flag {
        num_tiles - 1
    } else {
        let bits = ti.tile_cols_log2 + ti.tile_rows_log2;
        r.f(bits)?;
        r.f(bits)?
    };
    Ok(end == num_tiles - 1)
}

/// A decoder for a stream of temporal units (one per container packet), keeping the sequence
/// header and the reference frames across them — the shape a video player's decoder seam needs.
#[derive(Clone, Default)]
pub struct StreamDecoder {
    pub dec: Decoder,
    pub filters: Filters,
}

impl StreamDecoder {
    /// `av1c` is the container's AV1CodecConfigurationRecord body (MP4 `av1C`, Matroska
    /// CodecPrivate) or empty.
    pub fn new(av1c: &[u8]) -> Result<StreamDecoder> {
        let mut d = StreamDecoder { dec: Decoder::new(), filters: Filters::default() };
        if !av1c.is_empty() {
            let cfg = crate::avif::Av1Config::parse(av1c)?;
            for o in split_obus(&cfg.config_obus)? {
                if o.obu_type == OBU_SEQUENCE_HEADER_T {
                    d.dec.seq = Some(SequenceHeader::parse(o.payload)?);
                }
            }
        }
        Ok(d)
    }
    /// Decode one temporal unit; `Ok(planes)` for its (last) shown frame.
    pub fn decode_temporal_unit(&mut self, tu: &[u8]) -> Result<Planes> {
        let mut v = self.decode_temporal_unit_all(tu)?;
        v.pop().ok_or(Error::Invalid("temporal unit without a shown frame"))
    }
    /// Decode one temporal unit; every shown frame in it (one per shown spatial layer).
    pub fn decode_temporal_unit_all(&mut self, tu: &[u8]) -> Result<Vec<Planes>> {
        self.dec.filters = self.filters;
        self.dec.decode(tu, false)
    }
    /// After a seek: drop every reference frame (decoding resumes at the next key frame); the
    /// sequence header stays valid.
    pub fn reset(&mut self) {
        self.dec.refs.clear();
    }
}

/// decode_frame_wrapup (§7.4) steps 1–5: deblock → CDEF → superres upscaling → loop restoration.
fn postfilter(hdr: &FrameHeader, fs: &mut FrameState, filters: Filters) -> [crate::decode::Plane; 3] {
    if filters.deblock && (hdr.loop_filter_level[0] != 0 || hdr.loop_filter_level[1] != 0) {
        crate::loopfilter::loop_filter_frame(fs, hdr);
    }
    let cdef_planes = if filters.cdef {
        crate::cdef::cdef_frame(fs, hdr)
    } else {
        [fs.planes[0].clone(), fs.planes[1].clone(), fs.planes[2].clone()]
    };
    let up_cdef = crate::superres::upscale(fs, hdr, &cdef_planes);
    if filters.restoration && hdr.uses_lr {
        let up_cur = crate::superres::upscale(fs, hdr, &fs.planes);
        crate::restoration::lr_frame(fs, hdr, &up_cur, &up_cdef)
    } else {
        up_cdef
    }
}

/// The output process (§7.18): crop the planes to UpscaledWidth x FrameHeight.
fn output_planes(seq: &SequenceHeader, hdr: &FrameHeader, lr: &[crate::decode::Plane; 3], stats: ToolStats, filters: Filters) -> Planes {
    let cc = &seq.color_config;
    let w = hdr.upscaled_width;
    let h = hdr.frame_height;
    let (ssx, ssy) = (cc.subsampling_x, cc.subsampling_y);
    let crop = |p: &crate::decode::Plane, pw: u32, ph: u32| -> Vec<u16> {
        let mut v = Vec::with_capacity((pw * ph) as usize);
        for y in 0..ph as usize {
            v.extend_from_slice(&p.data[y * p.stride..y * p.stride + pw as usize]);
        }
        v
    };
    let (cw, ch) = ((w + ssx) >> ssx, (h + ssy) >> ssy);
    let mut y = crop(&lr[0], w, h);
    let mut u = if cc.mono_chrome { Vec::new() } else { crop(&lr[1], cw, ch) };
    let mut v = if cc.mono_chrome { Vec::new() } else { crop(&lr[2], cw, ch) };
    let mut stats = stats;
    if seq.film_grain_params_present && hdr.film_grain.apply_grain && filters.film_grain {
        crate::filmgrain::apply(&hdr.film_grain, cc.bit_depth, cc.mono_chrome, ssx, ssy, cc.matrix_coefficients, w as usize, h as usize, &mut y, &mut u, &mut v);
        stats.film_grain = true;
    }
    Planes {
        width: w,
        height: h,
        bit_depth: cc.bit_depth,
        mono: cc.mono_chrome,
        ss_x: ssx,
        ss_y: ssy,
        y,
        u,
        v,
        color_primaries: cc.color_primaries,
        transfer_characteristics: cc.transfer_characteristics,
        matrix_coefficients: cc.matrix_coefficients,
        full_range: cc.color_range,
        seq: seq.clone(),
        frame: hdr.clone(),
        stats,
    }
}

/// HEIFONE (LEDGER SR303): decode one `av01` image item's bytes (a low-overhead OBU stream) to planes, whatever read the
/// container (demux_core's HEIF reader: HEIFONE-RETIRE SR318 retired this crate's own walk). `av1c` is the item's raw
/// AV1CodecConfigurationRecord (parsed here for its configOBUs, [`crate::avif::Av1Config`]); `ispe` the item's spatial
/// extents (a width of 0 is no `ispe`) — when the first shown frame is not that size the item is a layered image and the
/// last shown frame of the `ispe` size is presented; `nclx` the container's `colr` (primaries, transfer, matrix, full
/// range), which is authoritative for presentation (AVIF §2.2.1). A container caller only forwards its item's fields.
pub fn decode_item_planes(
    data: &[u8],
    av1c: Option<&[u8]>,
    ispe: Option<(u32, u32)>,
    nclx: Option<(u16, u16, u16, bool)>,
    filters: Filters,
) -> Result<Planes> {
    let config_obus = match av1c {
        Some(rec) => crate::avif::Av1Config::parse(rec)?.config_obus,
        None => Vec::new(),
    };
    let cfg = config_obus.as_slice();
    let ispe = ispe.filter(|&(w, _)| w != 0);
    let mut p = decode_obus(data, cfg, filters)?;
    if let Some((iw, ih)) = ispe.filter(|&(iw, ih)| iw != p.width || ih != p.height) {
        // A layered image (a1lx / a1op / lsel): the item holds several frames, the upper spatial
        // layers predicted from the lower ones. Decode them all (operating point 0) and present the
        // highest layer — the last shown frame at the image's own size (ispe).
        let mut d = Decoder::new();
        d.filters = filters;
        if !cfg.is_empty() {
            for o in split_obus(cfg)? {
                if o.obu_type == OBU_SEQUENCE_HEADER_T {
                    d.seq = Some(SequenceHeader::parse(o.payload)?);
                }
            }
        }
        let frames = d.decode(data, false)?;
        p = frames
            .into_iter()
            .filter(|f| f.width == iw && f.height == ih)
            .next_back()
            .ok_or(Error::Unsupported("no layer of the image's size (ispe)"))?;
    }
    if let Some((primaries, transfer, matrix, full_range)) = nclx {
        // The container's nclx box is authoritative for presentation (AVIF §2.2.1).
        p.color_primaries = primaries as u8;
        p.transfer_characteristics = transfer as u8;
        p.matrix_coefficients = matrix as u8;
        p.full_range = full_range;
    }
    Ok(p)
}

/// How chroma is brought to luma resolution before the matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsampling {
    /// Each chroma sample covers its 2x2 (or 2x1) luma block.
    Nearest,
    /// Bilinear 9-3-3-1 interpolation assuming centre-sited (MPEG-1 style) 4:2:0 chroma — the
    /// "fancy" upsampling of libyuv / libavif's best quality mode.
    Bilinear,
}

/// How the Y'CbCr → R'G'B' matrix is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conversion {
    /// The H.273 equations through Stria's ONE integer kernel, `yuv_core::to_rgba` (AV1COLOURONE SR340; what the AVIF
    /// still path, gneiss_pal `decode_heif`, the host Player and the kernel Player use).
    Exact,
    /// ORACLE ONLY: the same equations with the chroma coefficients quantised the way libyuv
    /// (Chromium's converter) stores them — 6 fractional bits in an int8, so a coefficient above
    /// 2.0 in 8-bit sample units is clamped to 128/64. Fitting Chromium's screenshots of the
    /// BT.2020-matrix test files gives exactly these values (B = 1.7563·Cb vs H.273's 1.8814).
    /// Using it lets the oracle measure the decoder instead of Chromium's rounding.
    Libyuv,
}

/// Convert planes to 8-bit RGBA (H.273 equations, Stria's one integer kernel).
pub fn planes_to_rgba(p: &Planes, up: Upsampling) -> Image {
    planes_to_rgba_with(p, up, Conversion::Exact)
}

/// Convert planes to 8-bit RGBA with an explicit [`Conversion`]. AV1COLOURONE (LEDGER SR340): the f64 converter retired
/// onto `yuv_core::to_rgba`, the kernel gneiss_pal's `Frame::to_rgba_colour` runs. The matrix pick is the stills
/// convention, unchanged: 0 identity (GBR), 8 YCgCo (MATRIX-MORE; before SR340 it fell to BT.601), 10 (BT.2020
/// constant-luminance) as BT.2020's Kr / Kb, 2 (unspecified) and every matrix without a stated Kr / Kb BT.601 (as libavif).
pub fn planes_to_rgba_with(p: &Planes, up: Upsampling, conv: Conversion) -> Image {
    use yuv_core::{Chroma, Matrix};
    let (w, h) = (p.width as usize, p.height as usize);
    let m = match p.matrix_coefficients {
        0 => Matrix::Identity,
        8 => Matrix::YCgCo,
        mc => {
            let kk = yuv_core::kr_kb(if mc == 10 { 9 } else { mc }).unwrap_or(yuv_core::KR_KB_601);
            match conv {
                Conversion::Exact => Matrix::Kr(kk),
                Conversion::Libyuv => Matrix::Table(libyuv_coeffs(kk, p.full_range, p.bit_depth)),
            }
        }
    };
    let pic = picture(p);
    let chroma = match up {
        Upsampling::Nearest => Chroma::Nearest,
        Upsampling::Bilinear => Chroma::Bilinear,
    };
    let mut rgba = vec![0u8; w * h * 4];
    yuv_core::to_rgba(&pic, m, chroma, false, &mut rgba);
    Image { w: p.width, h: p.height, rgba }
}

/// AV1COLOURONE (LEDGER SR340): decoded planes as the one kernel's `yuv_core::Picture` (luma `width` per row, chroma
/// `chroma_width`, the stream's subsampling, mono, depth and range) — the picture `planes_to_rgba_with` converts and the
/// kernel Player hands `yuv_core::to_rgba` with its own pick.
pub fn picture(p: &Planes) -> yuv_core::Picture<'_, u16> {
    yuv_core::Picture {
        w: p.width as usize,
        h: p.height as usize,
        y: &p.y,
        u: &p.u,
        v: &p.v,
        y_stride: p.width as usize,
        uv_stride: p.chroma_width() as usize,
        ss: (p.ss_x, p.ss_y),
        mono: p.mono,
        bit_depth: p.bit_depth,
        full: p.full_range,
    }
}

/// ORACLE ONLY ([`Conversion::Libyuv`]): `yuv_core::coeffs` with the four chroma coefficients quantised as libyuv stores
/// them — q = round(x · unit · 64) capped at 128, unit 255 / 224 limited and 1 full, in 8-bit sample units — then
/// rescaled to the kernel's shift: limited q << (s - 6 - (n - 8)), full round(q · 255 · 2^s / (64 (2^n - 1))). Luma stays exact.
fn libyuv_coeffs((kr, kb): (i64, i64), full: bool, n: u32) -> yuv_core::Coeffs {
    let (s, [yc, _, _, _, _], yoff, coff) = yuv_core::coeffs((kr, kb), full, n);
    let rnd = |a: i64, b: i64| (2 * a + b) / (2 * b);
    let kg = 10000 - kr - kb;
    // Each coefficient as (numerator, denominator) of x, then q.
    let (num_den, unit) = ([(2 * (10000 - kr), 10000), (2 * kb * (10000 - kb), 10000 * kg), (2 * kr * (10000 - kr), 10000 * kg), (2 * (10000 - kb), 10000)], if full { (1, 1) } else { (255, 224) });
    let c = num_den.map(|(a, b)| {
        let q = rnd(a * unit.0 * 64, b * unit.1).min(128);
        (if full { rnd(q * (255 << s), 64 * ((1i64 << n) - 1)) } else { q << (s - 6 - (n - 8)) }) as i32
    });
    (s, [yc, c[0], c[1], c[2], c[3]], yoff, coff)
}

/// DEMUXSEEK (LEDGER SR200): the reorder depth of an AV1 track from its AV1CodecConfigurationRecord (`av1C`, Matroska
/// CodecPrivate) — the number `demux_core::Demuxer::derive_dts` takes. A container sample is one temporal unit, which holds
/// exactly one shown frame (AV1 §7.5; AV1-ISOBMFF §2.4), so decode order is presentation order: `Some(0)` when the record's
/// configOBUs carry a sequence header that parses, `None` otherwise.
pub fn av1c_reorder_depth(av1c: &[u8]) -> Option<u32> {
    av1c_colour(av1c).map(|_| 0)
}

/// NLEPROBEHDR (LEDGER SR226): the colour an AV1CodecConfigurationRecord states — the color_config of the sequence header in
/// its configOBUs (§5.5.2) and every METADATA OBU there that parses ([`parse_metadata`]: HDR_CLL, HDR_MDCV). The one walk of
/// the configOBUs [`av1c_reorder_depth`] also takes. `None` when the record holds no sequence header that parses.
pub fn av1c_colour(av1c: &[u8]) -> Option<(ColorConfig, Vec<Metadata>)> {
    let cfg = crate::avif::Av1Config::parse(av1c).ok()?;
    let obus = split_obus(&cfg.config_obus).ok()?;
    let o = obus.iter().find(|o| o.obu_type == OBU_SEQUENCE_HEADER_T)?;
    let seq = SequenceHeader::parse(o.payload).ok()?;
    let meta = obus.iter().filter(|o| o.obu_type == OBU_METADATA_T).filter_map(|o| parse_metadata(o.payload).ok()).collect();
    Some((seq.color_config, meta))
}
