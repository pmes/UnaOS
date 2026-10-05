//! Frame orchestration (§7.4 decode_frame_wrapup for intra frames: deblock → CDEF → loop
//! restoration → output), and the `pixel_core`-shaped entry point `decode_avif` that converts the
//! decoded Y'CbCr planes to 8-bit RGBA using the AVIF `colr`/nclx (or sequence header) matrix.

use crate::avif::avif_payload;
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

/// Decode an AVIF file's primary item to planes (container colour info applied to the fields).
pub fn decode_avif_planes(file: &[u8], filters: Filters) -> Result<Planes> {
    let item = avif_payload(file)?;
    let cfg = item.av1c.as_ref().map(|c| c.config_obus.as_slice()).unwrap_or(&[]);
    let mut p = decode_obus(&item.data, cfg, filters)?;
    if item.width != 0 && (item.width != p.width || item.height != p.height) {
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
        let frames = d.decode(&item.data, false)?;
        p = frames
            .into_iter()
            .filter(|f| f.width == item.width && f.height == item.height)
            .next_back()
            .ok_or(Error::Unsupported("no layer of the image's size (ispe)"))?;
    }
    if let Some(n) = item.nclx {
        // The container's nclx box is authoritative for presentation (AVIF §2.2.1).
        p.color_primaries = n.colour_primaries as u8;
        p.transfer_characteristics = n.transfer_characteristics as u8;
        p.matrix_coefficients = n.matrix_coefficients as u8;
        p.full_range = n.full_range;
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

/// Kr, Kb for a matrix_coefficients value (H.273); identity returns None.
fn kr_kb(mc: u8) -> Option<(f64, f64)> {
    match mc {
        0 => None,
        1 => Some((0.2126, 0.0722)),
        4 => Some((0.30, 0.11)),
        5 | 6 | 2 => Some((0.299, 0.114)), // 2 = unspecified: BT.601 like libavif
        7 => Some((0.212, 0.087)),
        9 | 10 => Some((0.2627, 0.0593)),
        _ => Some((0.299, 0.114)),
    }
}

/// How the Y'CbCr → R'G'B' matrix is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conversion {
    /// The H.273 equations in floating point (what `decode_avif` uses).
    Exact,
    /// ORACLE ONLY: the same equations with the chroma coefficients quantised the way libyuv
    /// (Chromium's converter) stores them — 6 fractional bits in an int8, so a coefficient above
    /// 2.0 in 8-bit sample units is clamped to 128/64. Fitting Chromium's screenshots of the
    /// BT.2020-matrix test files gives exactly these values (B = 1.7563·Cb vs H.273's 1.8814).
    /// Using it lets the oracle measure the decoder instead of Chromium's rounding.
    Libyuv,
}

/// Convert planes to 8-bit RGBA (H.273 equations, floating point).
pub fn planes_to_rgba(p: &Planes, up: Upsampling) -> Image {
    planes_to_rgba_with(p, up, Conversion::Exact)
}

/// Convert planes to 8-bit RGBA with an explicit [`Conversion`].
pub fn planes_to_rgba_with(p: &Planes, up: Upsampling, conv: Conversion) -> Image {
    let (w, h) = (p.width as usize, p.height as usize);
    let maxv = ((1u32 << p.bit_depth) - 1) as f64;
    let half = (1u32 << (p.bit_depth - 1)) as f64;
    let scale = (1u32 << (p.bit_depth - 8)) as f64;
    let cw = p.chroma_width() as usize;
    let ch = p.chroma_height() as usize;
    let mut rgba = vec![0u8; w * h * 4];
    let chroma = |plane: &[u16], x: usize, y: usize| -> f64 {
        if p.ss_x == 0 && p.ss_y == 0 {
            return plane[y * cw + x] as f64;
        }
        match up {
            Upsampling::Nearest => plane[(y >> p.ss_y) * cw + (x >> p.ss_x)] as f64,
            Upsampling::Bilinear => {
                // position of luma sample in chroma coordinates (centre siting)
                let fx = if p.ss_x != 0 { (x as f64 - 0.5) / 2.0 } else { x as f64 };
                let fy = if p.ss_y != 0 { (y as f64 - 0.5) / 2.0 } else { y as f64 };
                let x0 = libm_floor(fx);
                let y0 = libm_floor(fy);
                let ax = fx - x0;
                let ay = fy - y0;
                let g = |xx: f64, yy: f64| -> f64 {
                    let xi = (xx as isize).clamp(0, cw as isize - 1) as usize;
                    let yi = (yy as isize).clamp(0, ch as isize - 1) as usize;
                    plane[yi * cw + xi] as f64
                };
                let a = g(x0, y0) * (1.0 - ax) + g(x0 + 1.0, y0) * ax;
                let b = g(x0, y0 + 1.0) * (1.0 - ax) + g(x0 + 1.0, y0 + 1.0) * ax;
                a * (1.0 - ay) + b * ay
            }
        }
    };
    let kk = kr_kb(p.matrix_coefficients);
    for y in 0..h {
        for x in 0..w {
            let yv = p.y[y * w + x] as f64;
            let (r, g, b);
            if p.mono {
                let l = if p.full_range { yv / maxv } else { (yv - 16.0 * scale) / (219.0 * scale) };
                r = l;
                g = l;
                b = l;
            } else {
                let u = chroma(&p.u, x, y);
                let v = chroma(&p.v, x, y);
                match kk {
                    None => {
                        // identity (GBR)
                        let n = |c: f64| if p.full_range { c / maxv } else { (c - 16.0 * scale) / (219.0 * scale) };
                        g = n(yv);
                        b = n(u);
                        r = n(v);
                    }
                    Some((kr, kb)) => {
                        let (yy, cb, cr) = if p.full_range {
                            (yv / maxv, (u - half) / maxv, (v - half) / maxv)
                        } else {
                            ((yv - 16.0 * scale) / (219.0 * scale), (u - half) / (224.0 * scale), (v - half) / (224.0 * scale))
                        };
                        let kg = 1.0 - kr - kb;
                        // R = Y + a*Cr, G = Y - b*Cb - c*Cr, B = Y + d*Cb
                        let mut a = 2.0 - 2.0 * kr;
                        let mut d = 2.0 - 2.0 * kb;
                        let mut bb = kb * d / kg;
                        let mut c = kr * a / kg;
                        if conv == Conversion::Libyuv {
                            // coefficients in 8-bit sample units, 6 fractional bits, int8 storage
                            let unit = if p.full_range { 1.0 } else { 255.0 / 224.0 };
                            let qz = |x: f64| -> f64 {
                                let v = libm_floor(x * unit * 64.0 + 0.5).min(128.0);
                                v / 64.0 / unit
                            };
                            a = qz(a);
                            d = qz(d);
                            bb = qz(bb);
                            c = qz(c);
                        }
                        r = yy + a * cr;
                        b = yy + d * cb;
                        g = yy - bb * cb - c * cr;
                    }
                }
            }
            let q = |c: f64| -> u8 {
                let v = c * 255.0 + 0.5;
                if v <= 0.0 {
                    0
                } else if v >= 255.0 {
                    255
                } else {
                    v as u8
                }
            };
            let o = (y * w + x) * 4;
            rgba[o] = q(r);
            rgba[o + 1] = q(g);
            rgba[o + 2] = q(b);
            rgba[o + 3] = 255;
        }
    }
    Image { w: p.width, h: p.height, rgba }
}

fn libm_floor(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x { t - 1.0 } else { t }
}

/// The `pixel_core` entry point: an AVIF file to 8-bit RGBA.
pub fn decode_avif(file: &[u8]) -> Result<Image> {
    let p = decode_avif_planes(file, Filters::default())?;
    Ok(planes_to_rgba(&p, Upsampling::Bilinear))
}
