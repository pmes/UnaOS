// SPDX-License-Identifier: LGPL-3.0-or-later
//! H264CORE KATs: the JVT conformance bitstreams of ITU-T H.264.1 must decode BIT-EXACT — every
//! output frame, in output order, byte-identical to the reference decoder's YUV in the same
//! package — across Baseline (CAVLC, multiple slices, nal_ref_idc variants, multiple IDRs,
//! non-reference frames, several parameter sets, constrained intra, I_PCM, multiple references,
//! per-macroblock QP), Main (CABAC, B slices with direct prediction, explicit weighted prediction,
//! cropping, chroma QP offsets, CABAC I_PCM) and High (8x8 transform, Intra_8x8, scaling matrices,
//! CAVLC and CABAC). The interlaced (MBAFF, field), FMO and 4:2:2 packages must be refused by
//! name. Offline => SKIP (vectors are fetched, never committed; see vectors.txt for the pins).

mod common;

/// Decode package `rel`; Some(frames) when it was available.
fn run(rel: &str) -> Option<usize> {
    let p = common::package(rel)?;
    let frames = match common::decode_all(&p.stream) {
        Ok(f) => f,
        Err(e) => panic!("{rel} ({}): {e}", p.stream_name),
    };
    Some(common::compare(rel, &frames, &p.yuv))
}

macro_rules! vectors {
    ($($t:ident = $n:expr),* $(,)?) => {$(
        #[test]
        fn $t() {
            match run($n) {
                Some(n) => eprintln!("{}: {n} frames bit-exact", $n),
                None => eprintln!("SKIP {}", $n),
            }
        }
    )*};
}

vectors!(
    ba1_sony_d = "jvt/BA1_Sony_D.zip",
    ba2_sony_f = "jvt/BA2_Sony_F.zip",
    ba_mw_d = "jvt/BA_MW_D.zip",
    banm_mw_d = "jvt/BANM_MW_D.zip",
    ba1_ft_c = "jvt/BA1_FT_C.zip",
    nl1_sony_d = "jvt/NL1_Sony_D.zip",
    nl2_sony_h = "jvt/NL2_Sony_H.zip",
    sva_ba1_b = "jvt/SVA_BA1_B.zip",
    sva_nl1_b = "jvt/SVA_NL1_B.zip",
    midr_mw_d = "jvt/MIDR_MW_D.zip",
    nrf_mw_e = "jvt/NRF_MW_E.zip",
    mps_mw_a = "jvt/MPS_MW_A.zip",
    ci_mw_d = "jvt/CI_MW_D.zip",
    ls_sva_d = "jvt/LS_SVA_D.zip",
    cvpcmnl1_sva_c = "jvt/CVPCMNL1_SVA_C.zip",
    mr1_bt_a = "jvt/MR1_BT_A.zip",
    bamq1_jvc_c = "jvt/BAMQ1_JVC_C.zip",
    caba1_sva_b = "jvt/CABA1_SVA_B.zip",
    caba2_sony_e = "jvt/CABA2_Sony_E.zip",
    caba3_sva_b = "jvt/CABA3_SVA_B.zip",
    caba3_toshiba_e = "jvt/CABA3_TOSHIBA_E.zip",
    cacqp3_sony_d = "jvt/CACQP3_Sony_D.zip",
    capcm1_sand_e = "jvt/CAPCM1_Sand_E.zip",
    cabast3_sony_e = "jvt/CABAST3_Sony_E.zip",
    cvbs3_sony_c = "jvt/CVBS3_Sony_C.zip",
    cvwp1_toshiba_e = "jvt/CVWP1_TOSHIBA_E.zip",
    cvwp2_toshiba_e = "jvt/CVWP2_TOSHIBA_E.zip",
    cawp1_toshiba_e = "jvt/CAWP1_TOSHIBA_E.zip",
    mr2_tandberg_e = "jvt/MR2_TANDBERG_E.zip",
    cvse2_sony_b = "jvt/CVSE2_Sony_B.zip",
    cvfc1_sony_c = "jvt/CVFC1_Sony_C.zip",
    frext1_panasonic_d = "jvt/FRExt/FRExt1_Panasonic_D.zip",
    frext2_panasonic_c = "jvt/FRExt/FRExt2_Panasonic_C.zip",
    hcafr1_hhi = "jvt/FRExt/HCAFR1_HHI.zip",
    hpcv_brcm_a = "jvt/FRExt/HPCV_BRCM_A.zip",
    hpcvnl_brcm_a = "jvt/FRExt/HPCVNL_BRCM_A.zip",
    hpcadq_brcm_b = "jvt/FRExt/HPCADQ_BRCM_B.zip",
    hpcalq_brcm_b = "jvt/FRExt/HPCALQ_BRCM_B.zip",
);

/// A package outside the decoded set is refused with `Error::Unsupported` naming it.
fn refused(rel: &str, why: &str) {
    let Some(p) = common::package(rel) else {
        eprintln!("SKIP {rel}");
        return;
    };
    match common::decode_all(&p.stream) {
        Err(h264_core::Error::Unsupported(msg)) => {
            assert!(msg.contains(why), "{rel}: refused for {msg:?}, expected {why:?}");
            eprintln!("{rel}: refused by name ({msg})");
        }
        other => panic!("{rel} not refused: {:?}", other.map(|f| f.len())),
    }
}

#[test]
fn mbaff_is_refused_by_name() {
    refused("jvt/CAMA1_Sony_C.zip", "interlace");
}

#[test]
fn field_pictures_are_refused_by_name() {
    refused("jvt/CVFI1_Sony_D.zip", "interlace");
}

#[test]
fn fmo_is_refused_by_name() {
    refused("jvt/FM1_BT_B.zip", "FMO");
}

#[test]
fn chroma_422_is_refused_by_name() {
    refused("jvt/FRExt/Hi422FR1_SONY_A.zip", "4:2:2");
}
