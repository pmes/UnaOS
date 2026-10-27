// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Still images — a thin re-export of `pixel_core` (unaos/libs/media/pixel_core, PIXELCORE / SR25):
//! PNG (and APNG, animated), JPEG (baseline + progressive), GIF (animated), BMP, QOI and WebP (lossless,
//! lossy through vp8_core — VP8CORE / SR40 — and animated, ANIMWEBP / SR44), written from the
//! specifications, the same code the kernel's Facet viewer runs. `decode` sniffs the format; anything it
//! refuses is `Error::Unsupported` / `Error::UnknownFormat` and the caller decides what to fall back to.

pub use pixel_core::{
    Error, Format, Frame, Image, MAX_DIM, MAX_PIXELS, blend_nonpremult, decode, decode_bmp, decode_gif, decode_jpeg, decode_png,
    decode_qoi, decode_webp, sniff,
};
/// NLE5 (SR204): pixel_core's PNG writer (APPRES B398 / SHOTZIP), the one Stria's editor export encodes with.
pub use pixel_core::png::encode::{PngError, encode_rgba};

/// HEVCHEIC (SR220): is this an HEIF still that may hold HEVC images (brand `heic` / `heix`, or `mif1` without AVIF)?
/// demux_core's own answer ([`demux_core::heif::is_heic`]).
#[cfg(feature = "hevc")]
pub use demux_core::heif::is_heic;

/// HEIFONE (SR303): is this an AVIF still (brand `avif`)? demux_core's own answer ([`demux_core::heif::is_avif`]).
#[cfg(feature = "av1")]
pub use demux_core::heif::is_avif;

/// HEVCHEIC (SR220) / HEIFONE (SR303): decode an HEIF still — HEIC or AVIF. demux_core's HEIF reader finds the primary
/// item; each coded image decodes through its codec's core: `hvc1` through hevc_core (one IDR access unit; 8-bit 4:2:0
/// intra; the `hevc` feature) and the ONE YCbCr->RGB conversion of the HEVC face, [`super::video::Frame::to_rgba`]
/// (BT.601 limited range; a full-range picture — `colr` nclx's flag, else the stream's VUI — is first mapped into
/// limited range); `av01` through av1_core (`decode_item_planes`, the `av1` feature) and the AV1 face's conversion,
/// `av1_core::image::planes_to_rgba` with bilinear chroma (the one `dsp::video::av1` uses: any matrix,
/// range, depth and subsampling; so an AVIF still's pixels are av1_core's own). A `grid` item's tiles are composited
/// row-major into its canvas and cut to the grid's output size, then the primary item's `clap` and `irot` apply in
/// association order (`irot` through [`Image::apply_orientation`]). Refused by name: 10-bit HEVC (HEVCMAIN10 — most
/// iPhone HEICs are 10-bit), an HEVC colour matrix other than BT.601 (HEVCHEIC-COLR), `imir`, a primary item that is not
/// `hvc1` / `av01` / `grid`, a codec whose feature this build lacks, and whatever the codec core refuses. The returned
/// image has `orientation` 1.
#[cfg(any(feature = "hevc", feature = "av1"))]
pub fn decode_heif(bytes: &[u8]) -> Result<Image, Error> {
    use demux_core::heif::{self, ItemKind, Transform};
    let h = heif::parse_heif(bytes).map_err(heic_demux_err)?;
    let primary = h.item(h.primary).ok_or(Error::Malformed("heif: primary item missing"))?;
    let (mut w, mut ht, mut rgba) = match primary.kind {
        ItemKind::Hvc1 | ItemKind::Av01 => heif_tile(&h, bytes, primary, None)?,
        ItemKind::Grid => {
            let g = heif::parse_grid(&h.data(bytes, primary).map_err(heic_demux_err)?).map_err(heic_demux_err)?;
            heic_dims(g.width, g.height)?;
            if primary.dimg.len() != (g.rows * g.columns) as usize {
                return Err(Error::Malformed("heif: grid tile count is not rows x columns"));
            }
            let (gw, gh) = (g.width as usize, g.height as usize);
            let mut canvas = Vec::new();
            canvas.try_reserve_exact(gw * gh * 4).map_err(|_| Error::OutOfMemory)?;
            canvas.resize(gw * gh * 4, 0);
            let mut size = None;
            for (i, &id) in primary.dimg.iter().enumerate() {
                let tile = h.item(id).ok_or(Error::Malformed("heif: grid tile item missing"))?;
                if !matches!(tile.kind, ItemKind::Hvc1 | ItemKind::Av01) {
                    return Err(Error::Unsupported("heif: grid tile is not an hvc1 / av01 item"));
                }
                let (tw, th, px) = heif_tile(&h, bytes, tile, primary.nclx)?;
                if *size.get_or_insert((tw, th)) != (tw, th) {
                    return Err(Error::Malformed("heif: grid tiles differ in size"));
                }
                if (tw * g.columns) < g.width || (th * g.rows) < g.height {
                    return Err(Error::Malformed("heif: grid tiles do not cover the output"));
                }
                let (tw, th) = (tw as usize, th as usize);
                let (x0, y0) = ((i % g.columns as usize) * tw, (i / g.columns as usize) * th);
                for row in 0..th.min(gh.saturating_sub(y0)) {
                    let n = tw.min(gw.saturating_sub(x0)) * 4;
                    let d = ((y0 + row) * gw + x0) * 4;
                    canvas[d..d + n].copy_from_slice(&px[row * tw * 4..row * tw * 4 + n]);
                }
            }
            (g.width, g.height, canvas)
        }
        ItemKind::Exif => return Err(Error::Malformed("heif: the primary item is Exif metadata")),
        ItemKind::Other(k) => {
            return Err(Error::Unsupported(match &k {
                b"jpeg" => "heif: the primary item is jpeg",
                b"iovl" => "heif: the primary item is an iovl overlay",
                b"iden" => "heif: the primary item is an iden derivation",
                _ => "heif: the primary item type has no decoder",
            }));
        }
    };
    for t in &primary.transforms {
        match *t {
            Transform::Clap(c) => {
                let (x, y, cw, ch) = c.rect(w, ht).ok_or(Error::Malformed("heif: clap outside the image"))?;
                let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
                for row in y..y + ch {
                    let o = (row as usize * w as usize + x as usize) * 4;
                    out.extend_from_slice(&rgba[o..o + cw as usize * 4]);
                }
                (w, ht, rgba) = (cw, ch, out);
            }
            Transform::Irot(a) => {
                let mut img = Image { width: w, height: ht, rgba, frames: None, loop_count: None, orientation: [1, 8, 3, 6][a as usize & 3] };
                img.apply_orientation();
                (w, ht, rgba) = (img.width, img.height, img.rgba);
            }
            Transform::Imir(_) => return Err(Error::Unsupported("heif: imir mirroring (owed)")),
        }
    }
    Ok(Image { width: w, height: ht, rgba, frames: None, loop_count: None, orientation: 1 })
}

/// HEIFONE (SR303): one coded image item to straight RGBA through its codec's core — the per-codec tile fn
/// [`decode_heif`]'s compositor calls for a primary item and for every grid tile. `fallback` is the grid's `colr` for a
/// tile that carries none.
#[cfg(any(feature = "hevc", feature = "av1"))]
fn heif_tile(
    h: &demux_core::heif::Heif,
    bytes: &[u8],
    item: &demux_core::heif::Item,
    fallback: Option<demux_core::heif::Nclx>,
) -> Result<(u32, u32, Vec<u8>), Error> {
    use demux_core::heif::ItemKind;
    match item.kind {
        #[cfg(feature = "hevc")]
        ItemKind::Hvc1 => heic_tile(h, bytes, item, fallback),
        #[cfg(not(feature = "hevc"))]
        ItemKind::Hvc1 => Err(Error::Unsupported("heif: hvc1 needs gneiss_pal's hevc feature")),
        #[cfg(feature = "av1")]
        ItemKind::Av01 => avif_tile(h, bytes, item, fallback),
        #[cfg(not(feature = "av1"))]
        ItemKind::Av01 => Err(Error::Unsupported("heif: av01 needs gneiss_pal's av1 feature")),
        _ => Err(Error::Unsupported("heif: grid tile is not an hvc1 / av01 item")),
    }
}

/// HEIFONE (SR303): one `av01` item to straight RGBA: its fields forwarded to av1_core's `decode_item_planes` (the av1C
/// record's configOBUs, the layered-image pick and the container's nclx are av1_core's; HEIFONE-RETIRE SR318), then
/// `planes_to_rgba` with bilinear chroma — the pixels av1_core's retired `decode_avif` gave, byte for byte.
#[cfg(feature = "av1")]
fn avif_tile(
    h: &demux_core::heif::Heif,
    bytes: &[u8],
    item: &demux_core::heif::Item,
    fallback: Option<demux_core::heif::Nclx>,
) -> Result<(u32, u32, Vec<u8>), Error> {
    use av1_core::image::{Filters, Upsampling, decode_item_planes, planes_to_rgba};
    let ispe = item.ispe.filter(|&(w, _)| w != 0);
    if let Some((w, ht)) = ispe {
        heic_dims(w, ht)?;
    }
    let nclx = item.nclx.or(fallback).map(|n| (n.colour_primaries, n.transfer_characteristics, n.matrix_coefficients, n.full_range));
    let data = h.data(bytes, item).map_err(heic_demux_err)?;
    let p = decode_item_planes(&data, item.av1c.as_deref(), ispe, nclx, Filters::default()).map_err(avif_err)?;
    heic_dims(p.width, p.height)?;
    let img = planes_to_rgba(&p, Upsampling::Bilinear);
    Ok((img.w, img.h, img.rgba))
}

#[cfg(feature = "av1")]
fn avif_err(e: av1_core::Error) -> Error {
    match e {
        av1_core::Error::Unsupported(s) => Error::Unsupported(s),
        av1_core::Error::Truncated => Error::Truncated,
        av1_core::Error::Invalid(s) => Error::Malformed(s),
    }
}

/// One `hvc1` item to straight RGBA: the hvcC bit depth checked first, hevc_core decodes the one access unit, the range
/// mapped, then the one `to_rgba`. `fallback` is the grid's `colr` for a tile that carries none.
#[cfg(feature = "hevc")]
fn heic_tile(
    h: &demux_core::heif::Heif,
    bytes: &[u8],
    item: &demux_core::heif::Item,
    fallback: Option<demux_core::heif::Nclx>,
) -> Result<(u32, u32, Vec<u8>), Error> {
    let mut d = hevc_core::Decoder::new(); d.set_chroma_420_only(true); // HEVCREXT (SR334): to_rgba is 4:2:0 only until COLOURONE-422
    use super::video::{Frame as VideoFrame, Pixels};
    let rec = item.hvcc.as_deref().ok_or(Error::Malformed("heic: hvc1 item has no hvcC"))?;
    let cfg = hevc_core::nal::parse_hvcc(rec).map_err(heic_hevc_err)?;
    if cfg.bit_depth_luma > 8 || cfg.bit_depth_chroma > 8 {
        return Err(Error::Unsupported("heic: 10-bit HEVC (HEVCMAIN10)"));
    }
    if let Some((w, ht)) = item.ispe {
        heic_dims(w, ht)?;
    }
    let nclx = item.nclx.or(fallback);
    if nclx.is_some_and(|n| !matches!(n.matrix_coefficients, 2 | 5 | 6)) {
        return Err(Error::Unsupported("heic: colr matrix is not BT.601 (to_rgba is BT.601 only; HEVCHEIC-COLR)"));
    }
    d.configure_hvcc(rec).map_err(heic_hevc_err)?;
    d.decode_sample(&h.data(bytes, item).map_err(heic_demux_err)?).map_err(heic_hevc_err)?;
    d.flush();
    let mut frames = d.take_frames();
    if frames.len() != 1 {
        return Err(Error::Malformed("heic: an image item must decode to exactly one picture"));
    }
    let f = frames.remove(0);
    heic_dims(f.width, f.height)?;
    let [mut y, mut u, mut v] = f.planes;
    if nclx.map_or(f.full_range, |n| n.full_range) {
        // Full range (Y and C 0..255) into limited (Y 16..235, C 16..240), rounded half away from zero.
        let scale = |s: &mut u8, num: i32, zero: i32, base: i32| {
            let p = (*s as i32 - zero) * num;
            *s = (base + (p + if p >= 0 { 127 } else { -127 }) / 255) as u8;
        };
        y.iter_mut().for_each(|s| scale(s, 219, 0, 16));
        u.iter_mut().chain(v.iter_mut()).for_each(|s| scale(s, 224, 128, 128));
    }
    let (w, ht) = (f.width, f.height);
    let frame = VideoFrame {
        width: w,
        height: ht,
        pts_ns: 0,
        pixels: Pixels::I420 { y, u, v, y_stride: w as usize, uv_stride: w as usize / 2 },
    };
    Ok((w, ht, frame.to_rgba()))
}

#[cfg(any(feature = "hevc", feature = "av1"))]
fn heic_dims(w: u32, h: u32) -> Result<(), Error> {
    if w == 0 || h == 0 {
        return Err(Error::Malformed("heic: zero image dimension"));
    }
    if w > MAX_DIM || h > MAX_DIM || w as u64 * h as u64 > MAX_PIXELS as u64 {
        return Err(Error::TooLarge);
    }
    Ok(())
}

#[cfg(any(feature = "hevc", feature = "av1"))]
fn heic_demux_err(e: demux_core::Error) -> Error {
    match e {
        demux_core::Error::Truncated => Error::Truncated,
        demux_core::Error::UnknownFormat => Error::UnknownFormat,
        demux_core::Error::Invalid(s) => Error::Malformed(s),
        demux_core::Error::Unsupported(s) => Error::Unsupported(s),
    }
}

#[cfg(feature = "hevc")]
fn heic_hevc_err(e: hevc_core::Error) -> Error {
    match e {
        hevc_core::Error::Truncated => Error::Truncated,
        hevc_core::Error::Invalid(s) => Error::Malformed(s),
        hevc_core::Error::Unsupported(s) => Error::Unsupported(s),
    }
}

/// HEVCHEIC (SR220) test fixture — the ONE copy, shared by this module's tests and Facet's: two hand-assembled HEVC
/// pictures (the bytes hevc_core's own test kit, `unaos/libs/media/hevc_core/tests/common`, writes for its decode.rs
/// KATs, embedded here) and HEIC files built around them with demux_core's HEIF writer. (a) FLAT: a 64x48 picture of
/// INTRA_DC coding units with no residual, 128 in every plane. (b) PCM: the same size, a 32x32 I_PCM block at (0,0)
/// holding Y = 7x + 3y, Cb = 40 + 3x + y, Cr = 200 - x - 2y (chroma 16x16), the rest predicted from it; deblocking off.
#[cfg(feature = "hevc")]
#[doc(hidden)]
pub mod heic_kat {
    use demux_core::heif::build::{self, ItemSpec};

    pub const VPS: [u8; 23] = [
        0x40, 0x01, 0x0C, 0x01, 0xFF, 0xFF, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3C, 0xAC, 0x09, 
    ];
    pub const SPS_FLAT: [u8; 27] = [
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3C, 0xA0, 0x20, 0x83, 0x16, 0x5F, 0x92, 
        0x4C, 0x20, 0x80, 
    ];
    pub const PPS_FLAT: [u8; 6] = [
        0x44, 0x01, 0xC0, 0x71, 0x81, 0x12, 
    ];
    pub const SLICE_FLAT: [u8; 11] = [
        0x26, 0x01, 0xAF, 0x80, 0x26, 0x1D, 0x2D, 0x2C, 0xBD, 0x47, 0x30, 
    ];
    pub const SPS_PCM: [u8; 28] = [
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3C, 0xA0, 0x20, 0x83, 0x16, 0x5F, 0x92, 
        0x4C, 0x5D, 0xEF, 0x04, 
    ];
    pub const PPS_PCM: [u8; 7] = [
        0x44, 0x01, 0xC0, 0x71, 0x81, 0xA4, 0x80, 
    ];
    pub const SLICE_PCM_HEAD: [u8; 5] = [
        0x26, 0x01, 0xAF, 0x86, 0x80, 
    ];
    pub const SLICE_PCM_TAIL: [u8; 6] = [
        0x29, 0x37, 0x0D, 0x3A, 0xE4, 0x98, 
    ];

    pub fn pcm_y(x: usize, y: usize) -> u8 {
        (x * 7 + y * 3) as u8
    }
    pub fn pcm_cb(x: usize, y: usize) -> u8 {
        (40 + x * 3 + y) as u8
    }
    pub fn pcm_cr(x: usize, y: usize) -> u8 {
        (200 - x - 2 * y) as u8
    }

    /// The flat picture's hvcC (VPS, SPS, PPS).
    pub fn flat_hvcc() -> Vec<u8> {
        build::hvcc_record(&[&VPS, &SPS_FLAT, &PPS_FLAT])
    }
    pub fn pcm_hvcc() -> Vec<u8> {
        build::hvcc_record(&[&VPS, &SPS_PCM, &PPS_PCM])
    }
    /// The slice NAL of the PCM picture: its head, the 1536 raw PCM samples (Y 32x32, Cb 16x16, Cr 16x16), its tail.
    pub fn pcm_slice() -> Vec<u8> {
        let mut v = SLICE_PCM_HEAD.to_vec();
        v.extend((0..32 * 32).map(|i| pcm_y(i % 32, i / 32)));
        v.extend((0..16 * 16).map(|i| pcm_cb(i % 16, i / 16)));
        v.extend((0..16 * 16).map(|i| pcm_cr(i % 16, i / 16)));
        v.extend_from_slice(&SLICE_PCM_TAIL);
        v
    }
    /// One access unit, 4-byte length prefix (the hvcC writer's lengthSizeMinusOne is 3).
    pub fn access_unit(nal: &[u8]) -> Vec<u8> {
        let mut v = (nal.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(nal);
        v
    }
    /// An `hvc1` item of the flat (`pcm == false`) or PCM picture with `ispe` 64x48 and then `extra` properties.
    pub fn tile(pcm: bool, extra: Vec<Vec<u8>>) -> ItemSpec {
        let (rec, au) = if pcm { (pcm_hvcc(), access_unit(&pcm_slice())) } else { (flat_hvcc(), access_unit(&SLICE_FLAT)) };
        let mut props = vec![build::hvcc(&rec), build::ispe(64, 48)];
        props.extend(extra);
        ItemSpec { kind: *b"hvc1", data: au, props, ..Default::default() }
    }
    pub fn flat_heic() -> Vec<u8> {
        build::still(1, &[tile(false, vec![])])
    }
    pub fn pcm_heic() -> Vec<u8> {
        build::still(1, &[tile(true, vec![])])
    }
    /// A 2x2 grid (output 120x90 of the 128x96 canvas, ImageGrid in `idat`) of tiles flat, PCM / PCM, flat.
    pub fn grid_2x2_heic() -> Vec<u8> {
        let grid = ItemSpec {
            kind: *b"grid",
            data: build::grid_data(2, 2, 120, 90),
            props: vec![build::ispe(120, 90)],
            dimg: vec![2, 3, 4, 5],
            construction: 1,
        };
        build::still(1, &[grid, tile(false, vec![]), tile(true, vec![]), tile(true, vec![]), tile(false, vec![])])
    }
}

#[cfg(all(test, feature = "hevc"))]
mod heic_tests {
    use super::heic_kat::*;
    use super::*;
    use demux_core::heif::build::{self, ItemSpec};

    fn px(img: &Image, x: usize, y: usize) -> [u8; 4] {
        let o = (y * img.width as usize + x) * 4;
        img.rgba[o..o + 4].try_into().unwrap()
    }

    /// to_rgba of one (Y, Cb, Cr) sample, through the one conversion.
    fn rgb(y: u8, cb: u8, cr: u8) -> [u8; 4] {
        let f = super::super::video::Frame {
            width: 2,
            height: 2,
            pts_ns: 0,
            pixels: super::super::video::Pixels::I420 { y: vec![y; 4], u: vec![cb], v: vec![cr], y_stride: 2, uv_stride: 1 },
        };
        f.to_rgba()[..4].try_into().unwrap()
    }

    #[test]
    fn flat_heic_is_bt601_limited_grey() {
        assert!(is_heic(&flat_heic()));
        let img = decode_heif(&flat_heic()).unwrap();
        assert_eq!((img.width, img.height, img.orientation, img.frames.is_none()), (64, 48, 1, true));
        // Y = Cb = Cr = 128, limited range: (298 * 112 + 128) >> 8 = 130.
        assert!(img.rgba.chunks_exact(4).all(|p| p == [130, 130, 130, 255]));
        assert_eq!(rgb(128, 128, 128), [130, 130, 130, 255]);
    }

    #[test]
    fn pcm_heic_block_is_its_samples_through_to_rgba() {
        let img = decode_heif(&pcm_heic()).unwrap();
        assert_eq!((img.width, img.height), (64, 48));
        for y in 0..32 {
            for x in 0..32 {
                assert_eq!(px(&img, x, y), rgb(pcm_y(x, y), pcm_cb(x / 2, y / 2), pcm_cr(x / 2, y / 2)), "({x},{y})");
            }
        }
        // Below it the INTRA_ANGULAR26 block copies the PCM block's bottom row (hevc_core's decode.rs KAT).
        for y in 32..48 {
            for x in 0..16 {
                assert_eq!(px(&img, x, y), rgb(pcm_y(x, 31), pcm_cb(x / 2, 15), pcm_cr(x / 2, 15)), "({x},{y})");
            }
        }
    }

    #[test]
    fn grid_2x2_composites_row_major_and_cuts_to_the_output() {
        let flat = decode_heif(&flat_heic()).unwrap();
        let pcm = decode_heif(&pcm_heic()).unwrap();
        let g = decode_heif(&grid_2x2_heic()).unwrap();
        assert_eq!((g.width, g.height), (120, 90));
        for y in 0..90 {
            for x in 0..120 {
                let tile = if (x < 64) == (y < 48) { &flat } else { &pcm };
                assert_eq!(px(&g, x, y), px(tile, x % 64, y % 48), "({x},{y})");
            }
        }
    }

    #[test]
    fn clap_and_irot_apply_in_association_order() {
        let pcm = decode_heif(&pcm_heic()).unwrap();
        let clap = build::clap((56, 1), (40, 1), (0, 1), (0, 1));
        let c = decode_heif(&build::still(1, &[tile(true, vec![clap.clone()])])).unwrap();
        assert_eq!((c.width, c.height), (56, 40));
        assert_eq!(px(&c, 0, 0), px(&pcm, 4, 4));
        assert_eq!(px(&c, 55, 39), px(&pcm, 59, 43));
        // irot 1 = 90 degrees anticlockwise: stored (x, y) shows at (y, 63 - x).
        let r = decode_heif(&build::still(1, &[tile(true, vec![build::irot(1)])])).unwrap();
        assert_eq!((r.width, r.height, r.orientation), (48, 64, 1));
        for (x, y) in [(0, 0), (31, 0), (5, 17), (63, 47)] {
            assert_eq!(px(&r, y, 63 - x), px(&pcm, x, y));
        }
        // clap then irot: crop to 56x40 first, then rotate to 40x56.
        let cr = decode_heif(&build::still(1, &[tile(true, vec![clap, build::irot(1)])])).unwrap();
        assert_eq!((cr.width, cr.height), (40, 56));
        assert_eq!(px(&cr, 0, 55), px(&pcm, 4, 4));
    }

    #[test]
    fn colr_range_and_matrix() {
        // Full range: Y 128 -> 16 + round(219 * 128 / 255) = 126 -> (298 * 110 + 128) >> 8 = 128.
        let full = decode_heif(&build::still(1, &[tile(false, vec![build::nclx(1, 13, 6, true)])])).unwrap();
        assert!(full.rgba.chunks_exact(4).all(|p| p == [128, 128, 128, 255]));
        let limited = decode_heif(&build::still(1, &[tile(false, vec![build::nclx(1, 13, 5, false)])])).unwrap();
        assert!(limited.rgba.chunks_exact(4).all(|p| p == [130, 130, 130, 255]));
        let e = decode_heif(&build::still(1, &[tile(false, vec![build::nclx(1, 1, 1, false)])])).unwrap_err();
        assert!(matches!(e, Error::Unsupported(s) if s.contains("BT.601")), "{e:?}");
    }

    #[test]
    fn refusals_are_named() {
        // 10-bit: the hvcC says bitDepthLumaMinus8 = 2.
        let mut t = tile(false, vec![]);
        let mut rec = flat_hvcc();
        rec[17] = 0xFA;
        t.props[0] = build::hvcc(&rec);
        assert_eq!(decode_heif(&build::still(1, &[t])), Err(Error::Unsupported("heic: 10-bit HEVC (HEVCMAIN10)")));
        // A primary item no core decodes (HEIFONE SR303: av01 decodes now, through av1_core).
        let jpeg = ItemSpec { kind: *b"jpeg", data: vec![0], ..Default::default() };
        assert_eq!(decode_heif(&build::still(1, &[jpeg])), Err(Error::Unsupported("heif: the primary item is jpeg")));
        // imir is owed.
        let e = decode_heif(&build::still(1, &[tile(false, vec![build::imir(0)])])).unwrap_err();
        assert_eq!(e, Error::Unsupported("heif: imir mirroring (owed)"));
        // A grid whose dimg count disagrees with rows x columns.
        let grid = ItemSpec { kind: *b"grid", data: build::grid_data(2, 2, 120, 90), dimg: vec![2], construction: 1, ..Default::default() };
        let e = decode_heif(&build::still(1, &[grid, tile(false, vec![])])).unwrap_err();
        assert_eq!(e, Error::Malformed("heif: grid tile count is not rows x columns"));
        // A grid claiming more than MAX_DIM is refused before any allocation.
        let grid = ItemSpec { kind: *b"grid", data: build::grid_data(1, 1, MAX_DIM + 1, 8), dimg: vec![2], construction: 1, ..Default::default() };
        assert_eq!(decode_heif(&build::still(1, &[grid, tile(false, vec![])])), Err(Error::TooLarge));
        // A cut access unit surfaces hevc_core's refusal (never a panic).
        let mut t = tile(false, vec![]);
        t.data = access_unit(&SLICE_FLAT[..4]);
        assert!(decode_heif(&build::still(1, &[t])).is_err());
        // Not a HEIF at all.
        assert!(decode_heif(b"\x89PNG\r\n\x1a\n").is_err());
    }
}

/// HEIFONE (SR303) test fixture — the ONE copy, shared by this module's tests and Facet's: the AV1 payloads (OBU
/// streams) of two public AVIF files from libavif's test data (the vectors av1_core's `tests/vectors.txt` fetches by
/// sha256): `white_1x1.avif` (1x1, profile 1 4:4:4, nclx 1/13/6 full range) and `extended_pixi.avif` (4x4, profile 0
/// 4:2:0, nclx 2/2/2 full range), each wrapped in an AVIF file by demux_core's HEIF writer.
#[cfg(feature = "av1")]
#[doc(hidden)]
pub mod avif_kat {
    use demux_core::heif::build::{self, ItemSpec};

    pub const WHITE_OBUS: [u8; 23] = [
        0x12, 0x00, 0x0A, 0x07, 0x38, 0x00, 0x06, 0x10, 0x10, 0xD0, 0x69, 0x32, 0x0A, 0x1F, 0xF0, 0x3F, 0xFF, 0xFF, 0xC4, 0x00,
        0x00, 0xAF, 0x70,
    ];
    pub const WHITE_AV1C: [u8; 4] = [0x81, 0x20, 0x00, 0x00];
    pub const PIXI_OBUS: [u8; 49] = [
        0x12, 0x00, 0x0A, 0x05, 0x18, 0x04, 0x7D, 0x81, 0x48, 0x32, 0x26, 0x17, 0xC0, 0x09, 0x24, 0x92, 0x44, 0x00, 0x4E, 0x42,
        0x9B, 0x26, 0x5F, 0x06, 0xA5, 0xE1, 0xCD, 0x5E, 0xE3, 0x9D, 0x5A, 0xEF, 0x91, 0x71, 0x5A, 0xD4, 0x17, 0x3A, 0xB2, 0x17,
        0xD1, 0x31, 0x85, 0x5B, 0x26, 0xF2, 0xEE, 0x9B, 0xD6,
    ];
    pub const PIXI_AV1C: [u8; 4] = [0x81, 0x00, 0x0D, 0x00];

    /// An `av01` item of the white 1x1 (`pixi4 == false`) or the 4x4 picture, its properties as the source file
    /// carries them (ispe, pixi, av1C, colr), then `extra`.
    pub fn tile(pixi4: bool, extra: Vec<Vec<u8>>) -> ItemSpec {
        let mut props = if pixi4 {
            vec![build::ispe(4, 4), build::pixi(&[8, 8, 8]), build::av1c(&PIXI_AV1C), build::nclx(2, 2, 2, true)]
        } else {
            vec![build::ispe(1, 1), build::pixi(&[8, 8, 8]), build::av1c(&WHITE_AV1C), build::nclx(1, 13, 6, true)]
        };
        props.extend(extra);
        let data = if pixi4 { PIXI_OBUS.to_vec() } else { WHITE_OBUS.to_vec() };
        ItemSpec { kind: *b"av01", data, props, ..Default::default() }
    }
    pub fn white_avif() -> Vec<u8> {
        build::avif(1, &[tile(false, vec![])])
    }
    pub fn pixi_avif() -> Vec<u8> {
        build::avif(1, &[tile(true, vec![])])
    }
    /// A 2x2 grid of the 4x4 tile (output 7x6 of the 8x8 canvas, ImageGrid in `idat`).
    pub fn grid_2x2_avif() -> Vec<u8> {
        let grid = ItemSpec {
            kind: *b"grid",
            data: build::grid_data(2, 2, 7, 6),
            props: vec![build::ispe(7, 6)],
            dimg: vec![2, 3, 4, 5],
            construction: 1,
        };
        build::avif(1, &[grid, tile(true, vec![]), tile(true, vec![]), tile(true, vec![]), tile(true, vec![])])
    }
}

#[cfg(all(test, feature = "av1"))]
mod avif_tests {
    use super::avif_kat::*;
    use super::*;
    use demux_core::heif::build;

    fn fnv(b: &[u8]) -> u64 {
        b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3))
    }

    fn px(img: &Image, x: usize, y: usize) -> [u8; 4] {
        let o = (y * img.width as usize + x) * 4;
        img.rgba[o..o + 4].try_into().unwrap()
    }

    /// FNV-1a 64 of the RGBA the OLD path (`av1_core::image::decode_avif`, av1_core's own item walk, retired by
    /// HEIFONE-RETIRE SR318) gave for the two KAT files, pinned before decode_heif existed.
    const WHITE_FNV: u64 = 0x2fa3_2458_7145_aacb;
    const PIXI_FNV: u64 = 0x0aca_7e31_0b81_f28e;

    #[test]
    fn avif_stills_are_av1_cores_own_pixels() {
        for (f, w, h, pin) in [(white_avif(), 1, 1, WHITE_FNV), (pixi_avif(), 4, 4, PIXI_FNV)] {
            assert!(is_avif(&f));
            let new = decode_heif(&f).unwrap();
            assert_eq!((new.width, new.height, new.orientation, new.frames.is_none()), (w, h, 1, true));
            assert_eq!(fnv(&new.rgba), pin, "decode_heif is byte-identical to the retired decode_avif");
        }
    }

    #[test]
    fn avif_grid_2x2_composites_a_new_capability() {
        let f = grid_2x2_avif();
        // av1_core's retired walk refused a grid primary; decode_heif composites it.
        let tile = decode_heif(&pixi_avif()).unwrap();
        let g = decode_heif(&f).unwrap();
        assert_eq!((g.width, g.height), (7, 6));
        for y in 0..6 {
            for x in 0..7 {
                assert_eq!(px(&g, x, y), px(&tile, x % 4, y % 4), "({x},{y})");
            }
        }
    }

    #[test]
    fn avif_clap_and_irot_apply() {
        let base = decode_heif(&pixi_avif()).unwrap();
        let c = decode_heif(&build::avif(1, &[tile(true, vec![build::clap((2, 1), (2, 1), (0, 1), (0, 1))])])).unwrap();
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(px(&c, 0, 0), px(&base, 1, 1));
        assert_eq!(px(&c, 1, 1), px(&base, 2, 2));
        // irot 1 = 90 degrees anticlockwise: stored (x, y) shows at (y, 3 - x).
        let r = decode_heif(&build::avif(1, &[tile(true, vec![build::irot(1)])])).unwrap();
        for (x, y) in [(0, 0), (3, 0), (1, 2), (3, 3)] {
            assert_eq!(px(&r, y, 3 - x), px(&base, x, y));
        }
        let e = decode_heif(&build::avif(1, &[tile(true, vec![build::imir(1)])])).unwrap_err();
        assert_eq!(e, Error::Unsupported("heif: imir mirroring (owed)"));
    }

    #[test]
    fn avif_refusals_are_named() {
        // A cut OBU stream surfaces av1_core's refusal, never a panic.
        let mut t = tile(true, vec![]);
        t.data.truncate(10);
        assert!(decode_heif(&build::avif(1, &[t])).is_err());
        // A grid mixing tile sizes.
        let grid = build::ItemSpec {
            kind: *b"grid",
            data: build::grid_data(1, 2, 5, 4),
            dimg: vec![2, 3],
            construction: 1,
            ..Default::default()
        };
        let e = decode_heif(&build::avif(1, &[grid, tile(true, vec![]), tile(false, vec![])])).unwrap_err();
        assert_eq!(e, Error::Malformed("heif: grid tiles differ in size"));
        // Every prefix of the grid file is an Err or an image, never a panic.
        let f = grid_2x2_avif();
        for n in 0..f.len() {
            let _ = decode_heif(&f[..n]);
        }
    }
}
