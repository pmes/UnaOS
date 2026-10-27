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

/// HEVCHEIC (SR220): decode an HEIC still — demux_core's HEIF reader finds the primary item, hevc_core decodes each
/// `hvc1` image (one IDR access unit; 8-bit 4:2:0 intra), a `grid` item's tiles are composited row-major into its
/// canvas and cut to the grid's output size, the ONE YCbCr->RGB conversion is [`super::video::Frame::to_rgba`]
/// (BT.601 limited range; a full-range picture — `colr` nclx's flag, else the stream's VUI — is first mapped into
/// limited range), then the primary item's `clap` and `irot` apply in association order (`irot` through
/// [`Image::apply_orientation`]). Refused by name: 10-bit HEVC (HEVCMAIN10 — most iPhone HEICs are 10-bit), a colour
/// matrix other than BT.601 (HEVCHEIC-COLR), `imir`, a primary item that is not `hvc1` / `grid`, and whatever hevc_core
/// refuses (P / B slices do not occur in a still). The returned image has `orientation` 1.
#[cfg(feature = "hevc")]
pub fn decode_heic(bytes: &[u8]) -> Result<Image, Error> {
    use demux_core::heif::{self, ItemKind, Transform};
    let h = heif::parse_heif(bytes).map_err(heic_demux_err)?;
    let primary = h.item(h.primary).ok_or(Error::Malformed("heic: primary item missing"))?;
    let (mut w, mut ht, mut rgba) = match primary.kind {
        ItemKind::Hvc1 => heic_tile(&h, bytes, primary, None)?,
        ItemKind::Grid => {
            let g = heif::parse_grid(&h.data(bytes, primary).map_err(heic_demux_err)?).map_err(heic_demux_err)?;
            heic_dims(g.width, g.height)?;
            if primary.dimg.len() != (g.rows * g.columns) as usize {
                return Err(Error::Malformed("heic: grid tile count is not rows x columns"));
            }
            let (gw, gh) = (g.width as usize, g.height as usize);
            let mut canvas = Vec::new();
            canvas.try_reserve_exact(gw * gh * 4).map_err(|_| Error::OutOfMemory)?;
            canvas.resize(gw * gh * 4, 0);
            let mut size = None;
            for (i, &id) in primary.dimg.iter().enumerate() {
                let tile = h.item(id).ok_or(Error::Malformed("heic: grid tile item missing"))?;
                if tile.kind != ItemKind::Hvc1 {
                    return Err(Error::Unsupported("heic: grid tile is not an hvc1 item"));
                }
                let (tw, th, px) = heic_tile(&h, bytes, tile, primary.nclx)?;
                if *size.get_or_insert((tw, th)) != (tw, th) {
                    return Err(Error::Malformed("heic: grid tiles differ in size"));
                }
                if (tw * g.columns) < g.width || (th * g.rows) < g.height {
                    return Err(Error::Malformed("heic: grid tiles do not cover the output"));
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
        ItemKind::Exif => return Err(Error::Malformed("heic: the primary item is Exif metadata")),
        ItemKind::Other(k) => {
            return Err(Error::Unsupported(match &k {
                b"av01" => "heic: the primary item is av01 (AVIF)",
                b"jpeg" => "heic: the primary item is jpeg",
                b"iovl" => "heic: the primary item is an iovl overlay",
                b"iden" => "heic: the primary item is an iden derivation",
                _ => "heic: the primary item type has no decoder",
            }));
        }
    };
    for t in &primary.transforms {
        match *t {
            Transform::Clap(c) => {
                let (x, y, cw, ch) = c.rect(w, ht).ok_or(Error::Malformed("heic: clap outside the image"))?;
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
            Transform::Imir(_) => return Err(Error::Unsupported("heic: imir mirroring (owed)")),
        }
    }
    Ok(Image { width: w, height: ht, rgba, frames: None, loop_count: None, orientation: 1 })
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
    let mut d = hevc_core::Decoder::new();
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

#[cfg(feature = "hevc")]
fn heic_dims(w: u32, h: u32) -> Result<(), Error> {
    if w == 0 || h == 0 {
        return Err(Error::Malformed("heic: zero image dimension"));
    }
    if w > MAX_DIM || h > MAX_DIM || w as u64 * h as u64 > MAX_PIXELS as u64 {
        return Err(Error::TooLarge);
    }
    Ok(())
}

#[cfg(feature = "hevc")]
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
        let img = decode_heic(&flat_heic()).unwrap();
        assert_eq!((img.width, img.height, img.orientation, img.frames.is_none()), (64, 48, 1, true));
        // Y = Cb = Cr = 128, limited range: (298 * 112 + 128) >> 8 = 130.
        assert!(img.rgba.chunks_exact(4).all(|p| p == [130, 130, 130, 255]));
        assert_eq!(rgb(128, 128, 128), [130, 130, 130, 255]);
    }

    #[test]
    fn pcm_heic_block_is_its_samples_through_to_rgba() {
        let img = decode_heic(&pcm_heic()).unwrap();
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
        let flat = decode_heic(&flat_heic()).unwrap();
        let pcm = decode_heic(&pcm_heic()).unwrap();
        let g = decode_heic(&grid_2x2_heic()).unwrap();
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
        let pcm = decode_heic(&pcm_heic()).unwrap();
        let clap = build::clap((56, 1), (40, 1), (0, 1), (0, 1));
        let c = decode_heic(&build::still(1, &[tile(true, vec![clap.clone()])])).unwrap();
        assert_eq!((c.width, c.height), (56, 40));
        assert_eq!(px(&c, 0, 0), px(&pcm, 4, 4));
        assert_eq!(px(&c, 55, 39), px(&pcm, 59, 43));
        // irot 1 = 90 degrees anticlockwise: stored (x, y) shows at (y, 63 - x).
        let r = decode_heic(&build::still(1, &[tile(true, vec![build::irot(1)])])).unwrap();
        assert_eq!((r.width, r.height, r.orientation), (48, 64, 1));
        for (x, y) in [(0, 0), (31, 0), (5, 17), (63, 47)] {
            assert_eq!(px(&r, y, 63 - x), px(&pcm, x, y));
        }
        // clap then irot: crop to 56x40 first, then rotate to 40x56.
        let cr = decode_heic(&build::still(1, &[tile(true, vec![clap, build::irot(1)])])).unwrap();
        assert_eq!((cr.width, cr.height), (40, 56));
        assert_eq!(px(&cr, 0, 55), px(&pcm, 4, 4));
    }

    #[test]
    fn colr_range_and_matrix() {
        // Full range: Y 128 -> 16 + round(219 * 128 / 255) = 126 -> (298 * 110 + 128) >> 8 = 128.
        let full = decode_heic(&build::still(1, &[tile(false, vec![build::nclx(1, 13, 6, true)])])).unwrap();
        assert!(full.rgba.chunks_exact(4).all(|p| p == [128, 128, 128, 255]));
        let limited = decode_heic(&build::still(1, &[tile(false, vec![build::nclx(1, 13, 5, false)])])).unwrap();
        assert!(limited.rgba.chunks_exact(4).all(|p| p == [130, 130, 130, 255]));
        let e = decode_heic(&build::still(1, &[tile(false, vec![build::nclx(1, 1, 1, false)])])).unwrap_err();
        assert!(matches!(e, Error::Unsupported(s) if s.contains("BT.601")), "{e:?}");
    }

    #[test]
    fn refusals_are_named() {
        // 10-bit: the hvcC says bitDepthLumaMinus8 = 2.
        let mut t = tile(false, vec![]);
        let mut rec = flat_hvcc();
        rec[17] = 0xFA;
        t.props[0] = build::hvcc(&rec);
        assert_eq!(decode_heic(&build::still(1, &[t])), Err(Error::Unsupported("heic: 10-bit HEVC (HEVCMAIN10)")));
        // A non-hvc1 primary.
        let av01 = ItemSpec { kind: *b"av01", data: vec![0], ..Default::default() };
        assert_eq!(decode_heic(&build::still(1, &[av01])), Err(Error::Unsupported("heic: the primary item is av01 (AVIF)")));
        // imir is owed.
        let e = decode_heic(&build::still(1, &[tile(false, vec![build::imir(0)])])).unwrap_err();
        assert_eq!(e, Error::Unsupported("heic: imir mirroring (owed)"));
        // A grid whose dimg count disagrees with rows x columns.
        let grid = ItemSpec { kind: *b"grid", data: build::grid_data(2, 2, 120, 90), dimg: vec![2], construction: 1, ..Default::default() };
        let e = decode_heic(&build::still(1, &[grid, tile(false, vec![])])).unwrap_err();
        assert_eq!(e, Error::Malformed("heic: grid tile count is not rows x columns"));
        // A grid claiming more than MAX_DIM is refused before any allocation.
        let grid = ItemSpec { kind: *b"grid", data: build::grid_data(1, 1, MAX_DIM + 1, 8), dimg: vec![2], construction: 1, ..Default::default() };
        assert_eq!(decode_heic(&build::still(1, &[grid, tile(false, vec![])])), Err(Error::TooLarge));
        // A cut access unit surfaces hevc_core's refusal (never a panic).
        let mut t = tile(false, vec![]);
        t.data = access_unit(&SLICE_FLAT[..4]);
        assert!(decode_heic(&build::still(1, &[t])).is_err());
        // Not a HEIF at all.
        assert!(decode_heic(b"\x89PNG\r\n\x1a\n").is_err());
    }
}
