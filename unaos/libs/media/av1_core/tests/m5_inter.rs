//! AVCODEC2 known-answer tests: intra block copy and INTER decoding.
//!
//! The oracle is libaom's own decode: every AOM conformance vector ships the MD5 of each decoded
//! frame (Y, U, V rows; 8-bit samples as bytes, deeper samples as 16-bit LE; 4:0:0 streams with
//! mid-grey 4:2:0 chroma planes, as libaom's decoder outputs them). A frame passes only when
//! every sample is identical — frame-exact, not "close". Vectors are fetched at test time with
//! sha256 checks (tests/vectors.txt) and skipped when offline; nothing is committed.
mod common;
use av1_core::decode::ToolStats;
use av1_core::image::{decode_avif_planes, Filters, Planes, StreamDecoder};
use common::{fetch, fnv64, ivf_frames, md5};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn frame_bytes(p: &Planes) -> Vec<u8> {
    let mut raw = Vec::new();
    let push = |raw: &mut Vec<u8>, v: &[u16]| {
        for &s in v {
            if p.bit_depth == 8 {
                raw.push(s as u8);
            } else {
                raw.extend_from_slice(&s.to_le_bytes());
            }
        }
    };
    push(&mut raw, &p.y);
    if p.mono {
        let gray = vec![1u16 << (p.bit_depth - 1); (p.chroma_width() * p.chroma_height()) as usize];
        push(&mut raw, &gray);
        push(&mut raw, &gray);
    } else {
        push(&mut raw, &p.u);
        push(&mut raw, &p.v);
    }
    raw
}

/// Decode an IVF vector and compare every output frame with its libaom MD5. Returns the summed
/// tool statistics, or None when offline.
fn check_vector(name: &str, frames: usize) -> Option<ToolStats> {
    let ivf = fetch(name)?;
    let md5s = fetch(&format!("{name}.md5"))?;
    let want: Vec<String> = String::from_utf8(md5s).unwrap().lines().filter_map(|l| l.split_whitespace().next().map(String::from)).collect();
    assert_eq!(want.len(), frames, "{name}: reference frame count");
    let mut dec = StreamDecoder::new(&[]).unwrap();
    let mut n = 0;
    let mut total = ToolStats::default();
    for (i, tu) in ivf_frames(&ivf).iter().enumerate() {
        let out = dec.decode_temporal_unit_all(tu).unwrap_or_else(|e| panic!("{name}: temporal unit {i}: {e}"));
        // libaom outputs one frame per temporal unit (the highest spatial layer)
        let Some(p) = out.last() else { continue };
        assert!(p.stats.tiles_exit_bad == 0, "{name} frame {n}: exit_symbol padding check failed");
        let got = hex(&md5(&frame_bytes(p)));
        assert_eq!(got, want[n], "{name}: frame {n} differs from libaom (type {}, show_existing {})", p.frame.frame_type, p.frame.show_existing_frame);
        add(&mut total, &p.stats);
        n += 1;
    }
    assert_eq!(n, frames, "{name}: frames output");
    eprintln!("{name}: {n}/{frames} frames identical to libaom");
    Some(total)
}

fn add(t: &mut ToolStats, s: &ToolStats) {
    t.inter_blocks += s.inter_blocks;
    t.intrabc_blocks += s.intrabc_blocks;
    t.compound_blocks += s.compound_blocks;
    t.skip_mode_blocks += s.skip_mode_blocks;
    t.global_mv_blocks += s.global_mv_blocks;
    t.newmv_blocks += s.newmv_blocks;
    t.obmc_blocks += s.obmc_blocks;
    t.local_warp_blocks += s.local_warp_blocks;
    t.global_warp_blocks += s.global_warp_blocks;
    t.interintra_blocks += s.interintra_blocks;
    t.wedge_interintra_blocks += s.wedge_interintra_blocks;
    t.compound_wedge_blocks += s.compound_wedge_blocks;
    t.compound_diffwtd_blocks += s.compound_diffwtd_blocks;
    t.compound_distance_blocks += s.compound_distance_blocks;
    t.dual_filter_blocks += s.dual_filter_blocks;
    t.scaled_ref_blocks += s.scaled_ref_blocks;
    t.var_tx_splits += s.var_tx_splits;
    t.temporal_mvs |= s.temporal_mvs;
    for k in 0..4 {
        t.interp_filters[k] += s.interp_filters[k];
    }
}

#[test]
fn intra_block_copy_monochrome_avif() {
    // M1: Microsoft's screen-content key frame with allow_intrabc. The Chromium oracle matched
    // this decode at 60.62 dB, max abs diff 1 (docs/dev/evidence/media-1004/AVCODEC2.md).
    let Some(f) = fetch("Monochrome") else { return };
    let p = decode_avif_planes(&f, Filters::default()).unwrap();
    assert_eq!((p.width, p.height, p.bit_depth, p.mono), (1280, 720, 8, true));
    assert!(p.frame.allow_intrabc);
    assert!(p.stats.intrabc_blocks > 0 && p.stats.tiles_exit_bad == 0);
    let h = fnv64(&[&p.y]);
    if std::env::var("AV1_PRINT_PINS").is_ok() {
        println!("PIN_MONOCHROME = 0x{h:016x}");
        return;
    }
    assert_eq!(h, PIN_MONOCHROME, "Monochrome planes changed (0x{h:016x})");
}

/// FNV-1a 64 of Monochrome.avif's Y plane as decoded on 2026-10-05 (oracle-matched).
const PIN_MONOCHROME: u64 = 0x6d45ab6564380028;

#[test]
fn intra_block_copy_extreme_dv_is_frame_exact() {
    // Two 1920x1080 intra frames made almost entirely of intra-block-copy blocks with extreme
    // displacement vectors.
    if let Some(t) = check_vector("av1-1-b8-16-intra_only-intrabc-extreme-dv", 2) {
        assert!(t.intrabc_blocks > 10_000);
    }
}

#[test]
fn inter_sizes_are_frame_exact() {
    for (name, n) in [
        ("av1-1-b8-01-size-16x16", 2),
        ("av1-1-b8-01-size-34x34", 2),
        ("av1-1-b8-01-size-66x66", 2),
        ("av1-1-b8-01-size-196x196", 2),
        ("av1-1-b8-01-size-226x226", 2),
    ] {
        check_vector(name, n);
    }
}

#[test]
fn inter_quantizers_are_frame_exact() {
    // q 0 is lossless (WHT, OBMC, local warp, inter-intra all present), q 63 the coarsest.
    let mut t = ToolStats::default();
    for (name, n) in [
        ("av1-1-b8-00-quantizer-00", 2),
        ("av1-1-b8-00-quantizer-10", 2),
        ("av1-1-b8-00-quantizer-32", 2),
        ("av1-1-b8-00-quantizer-63", 2),
        ("av1-1-b10-00-quantizer-32", 2),
    ] {
        if let Some(s) = check_vector(name, n) {
            add(&mut t, &s);
        }
    }
    if t.inter_blocks > 0 {
        assert!(t.obmc_blocks > 0 && t.local_warp_blocks > 0 && t.global_warp_blocks > 0 && t.interintra_blocks > 0 && t.wedge_interintra_blocks > 0);
        assert!(t.dual_filter_blocks > 0 && t.var_tx_splits > 0 && t.interp_filters[1] > 0 && t.interp_filters[2] > 0);
    }
}

#[test]
fn reference_management_is_frame_exact() {
    // cdfupdate: frame-end CDF update + load_cdfs; mv: compound (wedge, difference-weighted,
    // distance), skip mode, extreme MVs and show_existing_frame; mfmv: motion field projection;
    // monochrome: hidden frames + show_existing_frame on 4:0:0 (8 and 10 bit).
    let mut t = ToolStats::default();
    for (name, n) in [
        ("av1-1-b8-04-cdfupdate", 2),
        ("av1-1-b8-05-mv", 4),
        ("av1-1-b8-06-mfmv", 4),
        ("av1-1-b8-24-monochrome", 10),
        ("av1-1-b10-24-monochrome", 10),
    ] {
        if let Some(s) = check_vector(name, n) {
            add(&mut t, &s);
        }
    }
    if t.inter_blocks > 0 {
        assert!(t.compound_blocks > 0 && t.skip_mode_blocks > 0 && t.temporal_mvs);
        assert!(t.compound_wedge_blocks > 0 && t.compound_diffwtd_blocks > 0 && t.compound_distance_blocks > 0);
    }
}

#[test]
fn scalable_streams_are_frame_exact() {
    // L1T2: two temporal layers. L2T1: two spatial layers — the enhancement layer predicts from
    // the half-size base layer, so every such block goes through reference scaling (§7.11.3.3).
    if let Some(t) = check_vector("av1-1-b8-22-svc-L1T2", 8) {
        assert!(t.inter_blocks > 0);
    }
    if let Some(t) = check_vector("av1-1-b8-22-svc-L2T1", 8) {
        assert!(t.scaled_ref_blocks > 0);
    }
}
