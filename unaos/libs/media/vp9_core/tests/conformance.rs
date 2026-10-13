// SPDX-License-Identifier: LGPL-3.0-or-later
//! VP9CORE KATs: the libvpx VP9 conformance vectors must decode BIT-EXACT — every shown frame's
//! MD5 (I420 bytes, or 16-bit LE samples at 10/12 bit) equal to the reference decoder's `.md5` —
//! across the feature surface: lossless and quantizer extremes, sharpness, odd sizes (8x8, 66x66,
//! 351x287), delta-q, reference scaling (resize, resize_inter), bilinear, frame-parallel mode, 4x4
//! tiles, loop-filter deltas, segmentation (aq2, segkey), show_existing_frame, droppable frames,
//! intra-only frames, skip, superframes (big_superframe, svc), profile 2 at 10 and 12 bit. The
//! profile-1 4:4:4 vector is refused by name. Offline => SKIP (vectors are fetched, never committed).

mod common;

/// Decode `name`; Some((frames, expected)) when the vector was available.
fn run(name: &str) -> Option<(usize, usize)> {
    let data = common::load(&format!("vp9/{name}"))?;
    let md5s = common::load(&format!("vp9/{name}.md5"))?;
    let want: Vec<String> = std::str::from_utf8(&md5s).unwrap().lines().filter_map(|l| l.split_whitespace().next().map(str::to_string)).collect();
    let mut dec = vp9_core::Decoder::new();
    let mut got = 0usize;
    for (i, chunk) in common::chunks(name, data).iter().enumerate() {
        match dec.decode(chunk) {
            Ok(Some(f)) => {
                let h = common::md5_hex(&f.md5_layout());
                assert_eq!(want.get(got).map(String::as_str), Some(h.as_str()), "{name}: shown frame {got} (chunk {i}, {}x{}) MD5", f.width, f.height);
                got += 1;
            }
            Ok(None) => {}
            Err(e) => panic!("{name} chunk {i}: {e}"),
        }
    }
    assert_eq!(got, want.len(), "{name}: shown frame count");
    Some((got, want.len()))
}

macro_rules! vectors {
    ($($t:ident = $n:expr),* $(,)?) => {$(
        #[test]
        fn $t() {
            match run($n) {
                Some((g, _)) => eprintln!("{}: {g} frames bit-exact", $n),
                None => eprintln!("SKIP {}", $n),
            }
        }
    )*};
}

vectors!(
    quantizer_00_lossless = "vp90-2-00-quantizer-00.webm",
    quantizer_32 = "vp90-2-00-quantizer-32.webm",
    quantizer_63 = "vp90-2-00-quantizer-63.webm",
    sharpness_4 = "vp90-2-01-sharpness-4.webm",
    size_08x08 = "vp90-2-02-size-08x08.webm",
    size_66x66 = "vp90-2-02-size-66x66.webm",
    size_130x132 = "vp90-2-02-size-130x132.webm",
    size_196x198 = "vp90-2-03-size-196x198.webm",
    size_226x226 = "vp90-2-03-size-226x226.webm",
    deltaq = "vp90-2-03-deltaq.webm",
    resize = "vp90-2-05-resize.ivf",
    bilinear = "vp90-2-06-bilinear.webm",
    frame_parallel = "vp90-2-07-frame_parallel.webm",
    tile_4x4 = "vp90-2-08-tile-4x4.webm",
    tile_4x1 = "vp90-2-08-tile-4x1.webm",
    tile_1x2_frame_parallel = "vp90-2-08-tile_1x2_frame_parallel.webm",
    lf_deltas = "vp90-2-09-lf_deltas.webm",
    aq2 = "vp90-2-09-aq2.webm",
    show_existing_frame = "vp90-2-10-show-existing-frame.webm",
    show_existing_frame2 = "vp90-2-10-show-existing-frame2.webm",
    size_351x287 = "vp90-2-11-size-351x287.webm",
    droppable_1 = "vp90-2-12-droppable_1.ivf",
    resize_fp_tiles_1_2 = "vp90-2-14-resize-fp-tiles-1-2.webm",
    segkey = "vp90-2-15-segkey.webm",
    intra_only = "vp90-2-16-intra-only.webm",
    show_existing_frame_17 = "vp90-2-17-show-existing-frame.webm",
    resize_18 = "vp90-2-18-resize.ivf",
    skip = "vp90-2-19-skip.webm",
    big_superframe = "vp90-2-20-big_superframe-01.webm",
    resize_inter_320x180 = "vp90-2-21-resize_inter_320x180_5_1-2.webm",
    svc_1280x720_3 = "vp90-2-22-svc_1280x720_3.ivf",
    profile2_10bit = "vp92-2-20-10bit-yuv420.webm",
    profile2_12bit = "vp92-2-20-12bit-yuv420.webm",
);

/// Profile 1 (4:4:4 here) is refused by name, never mis-decoded.
#[test]
fn profile1_yuv444_is_refused_by_name() {
    let Some(data) = common::load("vp9/vp91-2-04-yuv444.webm") else {
        eprintln!("SKIP vp91-2-04-yuv444.webm");
        return;
    };
    let chunk = common::chunks("vp91-2-04-yuv444.webm", data).into_iter().next().unwrap();
    match vp9_core::Decoder::new().decode(&chunk) {
        Err(vp9_core::Error::Unsupported(msg)) => assert!(msg.contains("profile 1/3"), "{msg}"),
        other => panic!("profile 1 not refused: {other:?}"),
    }
}
