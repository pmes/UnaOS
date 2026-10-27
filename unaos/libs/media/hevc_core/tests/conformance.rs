// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCCONF (LEDGER SR321): the JCT-VC conformance bitstreams of ITU-T H.265.1 must decode to the reference decoder's
//! output — every output picture (cropped, output order; yuv420p, or yuv420p10le at Main 10) hashed into one md5 equal
//! to the listed one — and every decoded picture hash SEI (D.3.19: CRC and checksum; MD5 refused by name) must match its
//! picture. The hand KATs (decode.rs, inter.rs, main10.rs) exercise each process once; these exercise their
//! interactions. The constrained-intra-in-inter vector must be refused by name (HEVCINTER-CIP). Offline => SKIP (the
//! vectors are fetched by h264_core's fetcher, linked below, never committed; vectors.txt holds the rows and their
//! provenance). The D.3.19 hash functions and the SEI parser have offline KATs at the foot.

#[path = "../../h264_core/tests/common/mod.rs"]
mod jvt;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use hevc_core::picture::Plane;
use hevc_core::sei::{self, HashCheck, PictureHash};
use hevc_core::{Decoder, Error, Frame};

const LIST: &str = include_str!("vectors.txt");

fn cache_dir() -> PathBuf {
    std::env::var_os("HEVC_VECTORS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hevc-vectors"))
}

/// A fetched vector: its bitstream, the listed md5 of the decoded YUV, and the zip's own .yuv when it carries one.
struct Vector {
    stream: Vec<u8>,
    md5: String,
    yuv: Option<Vec<u8>>,
}

fn fetch(name: &str) -> Option<Vector> {
    let rel = format!("hevc/{name}.zip");
    let zip = jvt::fetch_in(LIST, cache_dir(), &rel)?;
    let (_, _, cols) = jvt::entry_in(LIST, &rel).unwrap();
    let [md5, bitstream] = [&cols[0], &cols[1]];
    let files = jvt::unpack(&zip, &rel)?;
    let stream = files.iter().find(|p| p.ends_with(bitstream)).unwrap_or_else(|| panic!("{rel}: {bitstream} is not in the zip"));
    let yuv = files.iter().find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("yuv"))).and_then(|p| std::fs::read(p).ok());
    Some(Vector { stream: std::fs::read(stream).ok()?, md5: md5.clone(), yuv })
}

/// Decode a whole Annex B stream: the frames in output order and every picture's hash verdict.
fn decode_all(stream: &[u8]) -> Result<(Vec<Frame>, Vec<(i32, HashCheck)>), Error> {
    let mut d = Decoder::new();
    let (mut frames, mut checks) = (Vec::new(), Vec::new());
    for u in hevc_core::nal::annexb_units(stream) {
        d.decode_nal(u)?;
        frames.extend(d.take_frames());
        checks.extend(d.take_hash_checks());
    }
    d.flush();
    frames.extend(d.take_frames());
    checks.extend(d.take_hash_checks());
    Ok((frames, checks))
}

/// A frame as the reference decoder writes it: I420 bytes, or two bytes per sample (low first) above 8 bits.
fn yuv_bytes(f: &Frame) -> Vec<u8> {
    match &f.planes16 {
        Some(p16) => p16.iter().flatten().flat_map(|v| v.to_le_bytes()).collect(),
        None => f.i420(),
    }
}

/// The md5 of `data` through the host's md5sum (the harness's tools are host tools, as sha256sum is for the fetcher).
fn md5(data: &[u8]) -> Option<String> {
    let mut c = Command::new("md5sum").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok()?;
    c.stdin.take()?.write_all(data).ok()?;
    let out = c.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).split_whitespace().next()?.to_string())
}

/// Decode vector `name` and check it; `None` when it was not available (SKIP).
fn run(name: &str) -> Option<String> {
    let v = fetch(name)?;
    let (frames, checks) = decode_all(&v.stream).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(!frames.is_empty(), "{name}: no frames decoded");
    let bad: Vec<_> = checks.iter().filter(|(_, c)| matches!(c, HashCheck::Mismatch(_))).collect();
    assert!(bad.is_empty(), "{name}: decoded picture hash SEI mismatch at (POC, cIdx) {bad:?}");
    let ok = checks.iter().filter(|(_, c)| *c == HashCheck::Match).count();
    let md5_unchecked = checks.len() - ok;
    let out: Vec<Vec<u8>> = frames.iter().map(yuv_bytes).collect();
    let Some(got) = md5(&out.concat()) else {
        eprintln!("SKIP {name}: md5sum unavailable");
        return None;
    };
    if got != v.md5 {
        let at = v.yuv.as_ref().map(|yuv| {
            let mut off = 0;
            out.iter().position(|f| {
                let r = yuv.get(off..off + f.len());
                off += f.len();
                r != Some(f.as_slice())
            })
        });
        panic!("{name}: decoded YUV md5 {got}, want {} ({} frames; first differing frame against the zip's yuv: {at:?})", v.md5, frames.len());
    }
    Some(format!("{} frames md5 ok, hash SEI: {ok} CRC/checksum ok, {md5_unchecked} MD5 unchecked", frames.len()))
}

macro_rules! vectors {
    ($($t:ident = $n:expr),* $(,)?) => {$(
        #[test]
        fn $t() {
            match run($n) {
                Some(s) => eprintln!("{}: {s}", $n),
                None => eprintln!("SKIP {}", $n),
            }
        }
    )*};
}

vectors!(
    amp_a_samsung_7 = "AMP_A_Samsung_7",
    amvp_a_mtk_4 = "AMVP_A_MTK_4",
    merge_a_ti_3 = "MERGE_A_TI_3",
    merge_f_mtk_4 = "MERGE_F_MTK_4",
    tmvp_a_ms_3 = "TMVP_A_MS_3",
    mvclip_a_qualcomm_3 = "MVCLIP_A_qualcomm_3",
    mvedge_a_qualcomm_3 = "MVEDGE_A_qualcomm_3",
    wp_a_toshiba_3 = "WP_A_Toshiba_3",
    rps_a_docomo_5 = "RPS_A_docomo_5",
    ltrpsps_a_qualcomm_1 = "LTRPSPS_A_Qualcomm_1",
    poc_a_bossen_3 = "POC_A_Bossen_3",
    dblk_a_sony_3 = "DBLK_A_SONY_3",
    sao_a_mediatek_4 = "SAO_A_MediaTek_4",
    wpp_a_ericsson_main_2 = "WPP_A_ericsson_MAIN_2",
    // Main 10 (HEVCMAIN10 SR311).
    wp_a_main10_toshiba_3 = "WP_A_MAIN10_Toshiba_3",
    dblk_a_main10_vixs_4 = "DBLK_A_MAIN10_VIXS_4",
    wpp_a_ericsson_main10_2 = "WPP_A_ericsson_MAIN10_2",
    initqp_b_main10_sony_1 = "INITQP_B_Main10_Sony_1",
);

#[test]
fn constrained_intra_in_inter_slices_is_refused_by_name() {
    let name = "CIP_A_Panasonic_3";
    let Some(v) = fetch(name) else {
        eprintln!("SKIP {name}");
        return;
    };
    match decode_all(&v.stream) {
        Err(Error::Unsupported(msg)) => {
            assert!(msg.contains("HEVCINTER-CIP"), "{name}: refused for {msg:?}");
            eprintln!("{name}: refused by name ({msg})");
        }
        other => panic!("{name} not refused: {:?}", other.map(|(f, _)| f.len())),
    }
}

fn plane(w: usize, h: usize, data: &[u16]) -> Plane {
    Plane { w, h, data: data.to_vec() }
}

/// D.3.19's CRC is CRC-16/AUG-CCITT (the message then two zero bytes, polynomial 0x1021, initial 0xFFFF): its check
/// value over "123456789" is 0xE5CC.
#[test]
fn crc_kat() {
    let p = plane(9, 1, &b"123456789".map(u16::from));
    assert_eq!(sei::plane_crc(&p, 8), 0xE5CC);
    // Above 8 bits each sample is two bytes, low first: 0x3231 at 10 bits feeds the bytes 0x31 0x32 — "12".
    let two = plane(1, 1, &[0x3231]);
    assert_eq!(sei::plane_crc(&two, 10), sei::plane_crc(&plane(2, 1, &[0x31, 0x32]), 8));
}

/// The checksum by hand: xorMask is 0, 1, 1, 0 over a 2x2 plane.
#[test]
fn checksum_kat() {
    // 8 bits: (1^0) + (2^1) + (3^1) + (4^0) = 1 + 3 + 2 + 4.
    assert_eq!(sei::plane_checksum(&plane(2, 2, &[1, 2, 3, 4]), 8), 10);
    // 10 bits, low then high byte each: 0x301 (1^0 + 3^0) + 0 (0^1 + 0^1) + 1 (1^1 + 0^1) + 0x200 (0^0 + 2^0).
    assert_eq!(sei::plane_checksum(&plane(2, 2, &[0x301, 0, 1, 0x200]), 10), 4 + 2 + 1 + 2);
    // xorMask folds x >> 8: at x = 256, y = 0 the mask is 0 ^ 0 ^ 1 ^ 0 = 1.
    let mut wide = vec![0u16; 257];
    wide[256] = 5;
    assert_eq!(sei::plane_checksum(&plane(257, 1, &wide), 8), (0..256).map(|x| x as u32).sum::<u32>() + (5 ^ 1));
}

/// sei_rbsp parsing: a skipped message (and a payloadType past 255) before the decoded picture hash, each hash_type.
#[test]
fn sei_parse_kat() {
    let crc = [5, 2, 0xAA, 0xBB, 132, 7, 1, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0x80];
    assert_eq!(sei::picture_hash(&crc), Ok(Some(PictureHash::Crc([0x1234, 0x5678, 0x9ABC]))));
    let mut sum = vec![0xFF, 1, 1, 0, 132, 13, 2];
    sum.extend([0, 0, 0, 1, 0, 0, 1, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0x80]);
    assert_eq!(sei::picture_hash(&sum), Ok(Some(PictureHash::Checksum([1, 256, 0xFFFF_FFFF]))));
    let mut md5 = vec![132, 49, 0];
    md5.extend((0..48).map(|i| i as u8));
    md5.push(0x80);
    let want: [[u8; 16]; 3] = core::array::from_fn(|c| core::array::from_fn(|i| (c * 16 + i) as u8));
    assert_eq!(sei::picture_hash(&md5), Ok(Some(PictureHash::Md5(want))));
    assert_eq!(sei::check(&[plane(1, 1, &[0]), plane(1, 1, &[0]), plane(1, 1, &[0])], 8, &PictureHash::Md5(want)), HashCheck::Md5Unchecked);
    assert_eq!(sei::picture_hash(&[5, 1, 0, 0x80]), Ok(None));
    assert_eq!(sei::picture_hash(&[132, 7, 1, 0x12]), Err(Error::Truncated));
}
