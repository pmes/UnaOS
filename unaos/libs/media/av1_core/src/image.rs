//! Frame orchestration (§7.4 decode_frame_wrapup for intra frames: deblock → CDEF → loop
//! restoration → output), and the `pixel_core`-shaped entry point `decode_avif` that converts the
//! decoded Y'CbCr planes to 8-bit RGBA using the AVIF `colr`/nclx (or sequence header) matrix.

use crate::avif::avif_payload;
use crate::decode::{Dec, FrameState, ToolStats};
use crate::obu::*;
use crate::{Error, Result};
use alloc::vec;
use alloc::vec::Vec;

/// Post-filter switches. A conformant decode uses `Filters::default()` (all on); the oracle
/// harness switches them off one by one to show each filter's effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filters {
    pub deblock: bool,
    pub cdef: bool,
    pub restoration: bool,
}
impl Default for Filters {
    fn default() -> Self {
        Filters { deblock: true, cdef: true, restoration: true }
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

/// Decode the first shown intra frame of an AV1 OBU stream. `config_obus` (from `av1C`) supply
/// the sequence header when the stream itself lacks one.
pub fn decode_obus(data: &[u8], config_obus: &[u8], filters: Filters) -> Result<Planes> {
    let mut seq: Option<SequenceHeader> = None;
    if !config_obus.is_empty() {
        for o in split_obus(config_obus)? {
            if o.obu_type == OBU_SEQUENCE_HEADER_T {
                seq = Some(SequenceHeader::parse(o.payload)?);
            }
        }
    }
    decode_with_seq(data, &mut seq, filters)
}

/// A decoder for a stream of temporal units (one per container packet), keeping the sequence
/// header across them — the shape a video player's decoder seam needs. Key frames and intra-only
/// frames decode; an inter frame returns `Err(Unsupported("inter frame"))` (owed, see the doc).
#[derive(Debug, Clone, Default)]
pub struct StreamDecoder {
    seq: Option<SequenceHeader>,
    pub filters: Filters,
}

impl StreamDecoder {
    /// `av1c` is the container's AV1CodecConfigurationRecord body (MP4 `av1C`, Matroska
    /// CodecPrivate) or empty.
    pub fn new(av1c: &[u8]) -> Result<StreamDecoder> {
        let mut d = StreamDecoder { seq: None, filters: Filters::default() };
        if !av1c.is_empty() {
            let cfg = crate::avif::Av1Config::parse(av1c)?;
            for o in split_obus(&cfg.config_obus)? {
                if o.obu_type == OBU_SEQUENCE_HEADER_T {
                    d.seq = Some(SequenceHeader::parse(o.payload)?);
                }
            }
        }
        Ok(d)
    }
    /// Decode one temporal unit; `Ok(planes)` for its shown frame.
    pub fn decode_temporal_unit(&mut self, tu: &[u8]) -> Result<Planes> {
        decode_with_seq(tu, &mut self.seq, self.filters)
    }
    /// After a seek. Intra-only decoding keeps no reference frames, so there is nothing to drop
    /// yet; the sequence header stays valid. (Inter will clear the reference slots here.)
    pub fn reset(&mut self) {}
}

fn decode_with_seq(data: &[u8], seq: &mut Option<SequenceHeader>, filters: Filters) -> Result<Planes> {
    let obus = split_obus(data)?;
    let mut i = 0;
    while i < obus.len() {
        let o = obus[i];
        i += 1;
        match o.obu_type {
            OBU_SEQUENCE_HEADER_T => *seq = Some(SequenceHeader::parse(o.payload)?),
            OBU_FRAME_T | OBU_FRAME_HEADER_T => {
                let s = seq.as_ref().ok_or(Error::Invalid("frame before sequence header"))?;
                if s.operating_point_idc != 0 && o.has_extension {
                    let in_t = (s.operating_point_idc >> o.temporal_id) & 1;
                    let in_s = (s.operating_point_idc >> (o.spatial_id + 8)) & 1;
                    if in_t == 0 || in_s == 0 {
                        continue;
                    }
                }
                let hdr = parse_frame_header(o.payload, s, o.temporal_id, o.spatial_id)?;
                if hdr.use_superres {
                    return Err(Error::Unsupported("superres"));
                }
                let mut fs = FrameState::new(s, &hdr);
                {
                    let mut dec = Dec::new(s, &hdr, &mut fs, filters);
                    dec.init_lr();
                    let mut done = false;
                    if o.obu_type == OBU_FRAME_T {
                        done = dec.decode_tile_group(&o.payload[hdr.header_bytes..])?;
                    }
                    while !done {
                        let t = obus.get(i).ok_or(Error::Truncated)?;
                        i += 1;
                        if t.obu_type == OBU_TILE_GROUP_T {
                            done = dec.decode_tile_group(t.payload)?;
                        } else if t.obu_type == OBU_REDUNDANT_FRAME_HEADER_T || t.obu_type == OBU_METADATA_T || t.obu_type == OBU_PADDING_T {
                            continue;
                        } else {
                            return Err(Error::Invalid("missing tile group"));
                        }
                    }
                }
                let out = wrapup(s, &hdr, fs, filters);
                if hdr.show_frame {
                    return Ok(out);
                }
                // An unshown intra frame is only useful as a reference (inter: owed).
            }
            _ => {}
        }
    }
    Err(Error::Invalid("no shown frame"))
}

/// decode_frame_wrapup (§7.4) for an intra frame followed by the output process (§7.18).
fn wrapup(seq: &SequenceHeader, hdr: &FrameHeader, mut fs: FrameState, filters: Filters) -> Planes {
    if filters.deblock && (hdr.loop_filter_level[0] != 0 || hdr.loop_filter_level[1] != 0) {
        crate::loopfilter::loop_filter_frame(&mut fs, hdr);
    }
    let cdef_planes = if filters.cdef {
        crate::cdef::cdef_frame(&fs, hdr)
    } else {
        [fs.planes[0].clone(), fs.planes[1].clone(), fs.planes[2].clone()]
    };
    // (no superres: UpscaledCdefFrame = CdefFrame, UpscaledCurrFrame = CurrFrame)
    let lr = if filters.restoration { crate::restoration::lr_frame(&fs, hdr, &cdef_planes) } else { cdef_planes };
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
    Planes {
        width: w,
        height: h,
        bit_depth: cc.bit_depth,
        mono: cc.mono_chrome,
        ss_x: ssx,
        ss_y: ssy,
        y: crop(&lr[0], w, h),
        u: if cc.mono_chrome { Vec::new() } else { crop(&lr[1], cw, ch) },
        v: if cc.mono_chrome { Vec::new() } else { crop(&lr[2], cw, ch) },
        color_primaries: cc.color_primaries,
        transfer_characteristics: cc.transfer_characteristics,
        matrix_coefficients: cc.matrix_coefficients,
        full_range: cc.color_range,
        seq: seq.clone(),
        frame: hdr.clone(),
        stats: fs.stats.clone(),
    }
}

/// Decode an AVIF file's primary item to planes (container colour info applied to the fields).
pub fn decode_avif_planes(file: &[u8], filters: Filters) -> Result<Planes> {
    let item = avif_payload(file)?;
    let cfg = item.av1c.as_ref().map(|c| c.config_obus.as_slice()).unwrap_or(&[]);
    let mut p = decode_obus(&item.data, cfg, filters)?;
    if item.width != 0 && (item.width != p.width || item.height != p.height) {
        // e.g. a layered image (a1lx/lsel) whose base layer is smaller than the image: the upper
        // layers are predicted from the base (inter prediction) — owed, never silently downsized.
        return Err(Error::Unsupported("image size differs from the first frame (layered AVIF needs inter)"));
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
