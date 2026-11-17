// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCCONF (LEDGER SR321): the JCT-VC conformance bitstreams of ITU-T H.265.1 must decode to the reference decoder's
//! output — every output picture (cropped, output order; yuv420p, or yuv420p10le at Main 10) hashed into one md5 equal
//! to the listed one — and every decoded picture hash SEI (D.3.19: CRC, checksum and, since HEVCLEVELLINE SR368, MD5) must match its
//! picture. The hand KATs (decode.rs, inter.rs, main10.rs, rext.rs) exercise each process once; these exercise their
//! interactions. The constrained-intra-in-inter vector decodes to its md5 (HEVCINTERCIP SR688). Offline => SKIP (the
//! vectors are fetched by h264_core's fetcher, linked below, never committed; vectors.txt holds the rows and their
//! provenance). The D.3.19 hash functions and the SEI parser have offline KATs at the foot. HEVCSCCTAIL (LEDGER SR365):
//! the output md5 is crypto_core's MD5 (HEVCCONF-MD5; no host md5sum), and every vector decodes under
//! `Decoder::set_strict_levels` (each must lie within the level it signals, A.4).

#[path = "../../h264_core/tests/common/mod.rs"]
mod jvt;

use std::path::PathBuf;

use hevc_core::picture::Plane;
use hevc_core::sei::{self, HashCheck, PictureHash};
use hevc_core::{Decoder, Error, Frame, yuv_bytes};

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
    d.set_strict_levels(true);
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

/// HEVCSCCTAIL (SR365, HEVCCONF-MD5): lowercase hex of crypto_core's MD5 of `data` (the digest the fluster lists print;
/// the twin of tools/play-check's `md5_hex`, which hashes the same `yuv_bytes`).
fn md5(data: &[u8]) -> String {
    crypto_core::md5::md5(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode vector `name` and check it; `Verdict::Skip` when it was not available.
fn run(name: &str) -> Verdict {
    let Some(v) = fetch(name) else { return Verdict::Skip };
    let (frames, checks) = match decode_all(&v.stream) { Ok(d) => d, Err(e) => return Verdict::Decode(e.to_string()) };
    check_vector(&v, frames, checks)
}

/// HEVCREXT (SR334): `run`'s check of one decoded vector — the hash SEI verdicts, then the md5 of the output YUV.
fn check_vector(v: &Vector, frames: Vec<Frame>, checks: Vec<(i32, HashCheck)>) -> Verdict {
    if frames.is_empty() { return Verdict::Decode("no frames decoded".into()) }
    let bad: Vec<_> = checks.iter().filter(|(_, c)| matches!(c, HashCheck::Mismatch(_))).copied().collect();
    if !bad.is_empty() { return Verdict::HashMismatch(bad) }
    let ok = checks.iter().filter(|(_, c)| *c == HashCheck::Match).count();
    let md5_unchecked = checks.len() - ok;
    let out: Vec<Vec<u8>> = frames.iter().map(yuv_bytes).collect();
    let got = md5(&out.concat());
    if got != v.md5 {
        let at = v.yuv.as_ref().map(|yuv| {
            let mut off = 0;
            out.iter().position(|f| {
                let r = yuv.get(off..off + f.len());
                off += f.len();
                r != Some(f.as_slice())
            })
        });
        return Verdict::Md5(got, v.md5.clone(), frames.len(), at, at.flatten().zip(v.yuv.as_ref()).map(|(i, y)| first_diff(&frames[i], y.get(out[..i].iter().map(Vec::len).sum::<usize>()..).unwrap_or(&[]))));
    }
    Verdict::Ok(format!("{} frames md5 ok, hash SEI: {ok} CRC/checksum ok, {md5_unchecked} MD5 unchecked", frames.len()))
}

macro_rules! vectors {
    ($($t:ident = $n:expr),* $(,)?) => {$(
        #[test]
        fn $t() {
            report($n, run($n))
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
    cip_a_panasonic_3 = "CIP_A_Panasonic_3",
    // Main 10 (HEVCMAIN10 SR311).
    wp_a_main10_toshiba_3 = "WP_A_MAIN10_Toshiba_3",
    dblk_a_main10_vixs_4 = "DBLK_A_MAIN10_VIXS_4",
    wpp_a_ericsson_main10_2 = "WPP_A_ericsson_MAIN10_2",
    initqp_b_main10_sony_1 = "INITQP_B_Main10_Sony_1",
);

/// HEVCREXT (SR334): every `hevc/rext/` row of vectors.txt (the JCT-VC RExt suite) decodes to its md5, or is refused by
/// a name carrying HEVCREXT (HEVCREXTTOOLS SR345: only separate colour planes and SCC remain); offline each row SKIPs.
#[test]
fn rext_vectors_decode_or_are_refused_by_name() {
    let names = LIST.lines().filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("hevc/rext/")?.strip_suffix(".zip"));
    let mut rows = Vec::new();
    for name in names {
        let Some(verdict) = row_verdict(|| Some(match fetch(&format!("rext/{name}")) {
            None => Verdict::Skip,
            Some(v) => match decode_all(&v.stream) {
                Err(Error::Unsupported(msg)) if msg.contains("HEVCREXT") => {
                    eprintln!("{name}: refused by name ({msg})");
                    return None;
                }
                Err(e) => Verdict::Decode(e.to_string()),
                Ok((frames, checks)) => check_vector(&v, frames, checks),
            },
        })) else { continue };
        match &verdict {
            Verdict::Skip => eprintln!("SKIP hevc/rext/{name}"),
            v => eprintln!("{name}: {v}"),
        }
        rows.push((name.to_string(), verdict));
    }
    if let Some(m) = red_rows("rext", &rows) { panic!("{m}") }
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
    assert_eq!(sei::check(&[plane(1, 1, &[0]), plane(1, 1, &[0]), plane(1, 1, &[0])], 8, 8, &PictureHash::Md5(want)), HashCheck::Mismatch(0b111)); // SR1090: cIdx mask
    assert_eq!(sei::picture_hash(&[5, 1, 0, 0x80]), Ok(None));
    assert_eq!(sei::picture_hash(&[132, 7, 1, 0x12]), Err(Error::Truncated));
}

/// HEVCSCCTAIL (SR365, HEVCCONF-MD5): the walker's md5 of a known picture through `yuv_bytes` — I420 bytes at 8 bits,
/// yuv420p10le above — equals the hex pinned from an independent MD5 (Python's hashlib over the same bytes).
#[test]
fn md5_kat() {
    let frame = |planes: [Vec<u8>; 3], bit_depth: u8, planes16: Option<[Vec<u16>; 3]>| Frame {
        width: 2,
        height: 2,
        planes,
        poc: 0,
        tag: 0,
        full_range: false,
        bit_depth,
        planes16,
        chroma: h264_core::ChromaFormat::Yuv420,
        bit_depth_c: bit_depth,
    };
    let f8 = frame([vec![1, 2, 3, 4], vec![5], vec![6]], 8, None);
    assert_eq!(md5(&yuv_bytes(&f8)), "6ac1e56bc78f031059be7be854522c4c");
    let f10 = frame([vec![255, 0, 0, 0], vec![128], vec![85]], 10, Some([vec![0x3FF, 0, 1, 2], vec![0x200], vec![0x155]]));
    assert_eq!(yuv_bytes(&f10), [0xFF, 0x03, 0, 0, 1, 0, 2, 0, 0, 0x02, 0x55, 0x01]);
    assert_eq!(md5(&yuv_bytes(&f10)), "194ea06078b4dbc31dbc26b29a566407");
    assert_eq!(md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
}

/// HEVCLEVELLINE (SR368, HEVCSEI-MD5): D.3.19 picture_md5 per component through crypto_core, against the hex of an
/// independent MD5 (Python's hashlib over the same pictureData bytes): a 2x2 / 1x1 / 1x1 picture at 8 bits matches, a
/// changed Cr is Mismatch(0b100); 10 bits hash low byte first; a 4:0:0 picture (empty chroma) is checked over component 0.
#[test]
fn sei_md5_kat() {
    let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let digest = |h: &str| -> [u8; 16] { core::array::from_fn(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()) };
    let pic = [plane(2, 2, &[1, 2, 3, 4]), plane(1, 1, &[5]), plane(1, 1, &[6])];
    assert_eq!(hex(sei::plane_md5(&pic[0], 8)), "08d6c05a21512a79a1dfeb9d2a8f262f");
    assert_eq!(hex(sei::plane_md5(&pic[1], 8)), "8bb6c17838643f9691cc6a4de6c51709");
    assert_eq!(hex(sei::plane_md5(&pic[2], 8)), "06eca1b437c7904cc3ce6546c8110110");
    let want = PictureHash::Md5(["08d6c05a21512a79a1dfeb9d2a8f262f", "8bb6c17838643f9691cc6a4de6c51709", "06eca1b437c7904cc3ce6546c8110110"].map(digest));
    assert_eq!(sei::check(&pic, 8, 8, &want), HashCheck::Match);
    let cr_off = [pic[0].clone(), pic[1].clone(), plane(1, 1, &[7])];
    assert_eq!(sei::check(&cr_off, 8, 8, &want), HashCheck::Mismatch(0b100)); // SR1090: cIdx mask
    assert_eq!(hex(sei::plane_md5(&plane(2, 2, &[0x3FF, 0, 1, 2]), 10)), "c0e517ad64daf3a79e06d779d07d8b26");
    let mono = [pic[0].clone(), plane(0, 0, &[]), plane(0, 0, &[])];
    let one = PictureHash::Md5([digest("08d6c05a21512a79a1dfeb9d2a8f262f"), [0; 16], [0; 16]]);
    assert_eq!(sei::check(&mono, 8, 8, &one), HashCheck::Match);
}

// ---- HEVCREXTLOOPALLROWS (LEDGER SR1094): one verdict per vector; the loops list every red row (named tail block) ----

/// HEVCREXTLOOPALLROWS (SR1094): `check_vector` / `run`'s verdict for one vector. Its Display is the old panic text
/// without the `{name}: ` prefix, which the caller adds (`report`, `red_rows`).
enum Verdict {
    Ok(String),
    Skip,
    HashMismatch(Vec<(i32, HashCheck)>),
    /// got, want, frames decoded, the first frame differing from the zip's yuv (None: the zip carries no yuv), and that frame's first differing sample per plane (SR1101 `first_diff`).
    Md5(String, String, usize, Option<Option<usize>>, Option<[Option<(usize, usize, u16, u16)>; 3]>),
    Decode(String),
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Ok(s) | Verdict::Decode(s) => f.write_str(s),
            Verdict::Skip => f.write_str("SKIP"),
            Verdict::HashMismatch(bad) => write!(f, "decoded picture hash SEI mismatch at (POC, planes) {bad:?}"),
            Verdict::Md5(got, want, n, at, d) => {
                write!(f, "decoded YUV md5 {got}, want {want} ({n} frames; first differing frame against the zip's yuv: {at:?}){}", diff_text(d))
            }
        }
    }
}

/// The per-vector test's arm: Ok and Skip print, anything else panics with `{name}: {verdict}` (the old texts).
fn report(name: &str, v: Verdict) {
    match v {
        Verdict::Ok(s) => eprintln!("{name}: {s}"),
        Verdict::Skip => eprintln!("SKIP {name}"),
        v => panic!("{name}: {v}"),
    }
}

/// Every red row (not Ok, not Skip) of a loop as ONE message, `None` when there is none.
fn red_rows(kind: &str, rows: &[(String, Verdict)]) -> Option<String> {
    let red: Vec<String> = rows.iter().filter(|(_, v)| !matches!(v, Verdict::Ok(_) | Verdict::Skip)).map(|(n, v)| format!("{n}: {v}")).collect();
    (!red.is_empty()).then(|| format!("{} {kind} rows red: [{}]", red.len(), red.join(", ")))
}

/// A 2x2 8-bit 4:2:0 frame with luma `y` and chroma 5 / 6 (`yuv_bytes` is y ++ [5] ++ [6]).
fn frame8(y: [u8; 4]) -> Frame {
    Frame {
        width: 2,
        height: 2,
        planes: [y.to_vec(), vec![5], vec![6]],
        poc: 0,
        tag: 0,
        full_range: false,
        bit_depth: 8,
        planes16: None,
        chroma: h264_core::ChromaFormat::Yuv420,
        bit_depth_c: 8,
    }
}

fn vector(md5: &str, yuv: Option<Vec<u8>>) -> Vector {
    Vector { stream: Vec::new(), md5: md5.into(), yuv }
}

#[test]
fn verdict_hash_mismatch_prints_the_old_panic_text() {
    let v = Verdict::HashMismatch(vec![(2, HashCheck::Mismatch(0b001)), (3, HashCheck::Mismatch(0b011))]);
    assert_eq!(format!("WP: {v}"), "WP: decoded picture hash SEI mismatch at (POC, planes) [(2, Mismatch(Y)), (3, Mismatch(Y+Cb))]");
}

#[test]
fn verdict_md5_names_the_frame_count_and_first_differing_frame() {
    // md5_kat pins 6ac1e56b.. for frame8([1, 2, 3, 4]).
    let ok = check_vector(&vector("6ac1e56bc78f031059be7be854522c4c", None), vec![frame8([1, 2, 3, 4])], vec![]);
    assert!(matches!(&ok, Verdict::Ok(s) if s == "1 frames md5 ok, hash SEI: 0 CRC/checksum ok, 0 MD5 unchecked"));
    let want = "0".repeat(32);
    let zip_yuv = [yuv_bytes(&frame8([1, 2, 3, 4])), yuv_bytes(&frame8([9, 9, 9, 9]))].concat();
    let red = check_vector(&vector(&want, Some(zip_yuv)), vec![frame8([1, 2, 3, 4]), frame8([1, 2, 3, 4])], vec![]);
    let Verdict::Md5(got, w, n, at, d) = &red else { panic!("expected Md5, got {red}") };
    assert_eq!((w.as_str(), *n, *at), (want.as_str(), 2, Some(Some(1))));
    assert_eq!(*d, Some([Some((0, 0, 1, 9)), None, None]));
    let old = format!("decoded YUV md5 {got}, want {} ({} frames; first differing frame against the zip's yuv: {at:?})", want, 2);
    assert_eq!(red.to_string(), old + " first diff Y (0, 0) got 1 want 9");
    assert!(red.to_string().ends_with("(2 frames; first differing frame against the zip's yuv: Some(Some(1))) first diff Y (0, 0) got 1 want 9"));
}

#[test]
fn verdict_loop_names_every_red_row() {
    let good = "6ac1e56bc78f031059be7be854522c4c";
    let rows = vec![
        ("r1".to_string(), check_vector(&vector(&"0".repeat(32), None), vec![frame8([1, 2, 3, 4])], vec![])),
        ("r2".to_string(), check_vector(&vector(good, None), vec![frame8([1, 2, 3, 4])], vec![(0, HashCheck::Match)])),
        ("r3".to_string(), check_vector(&vector(good, None), vec![frame8([1, 2, 3, 4])], vec![(0, HashCheck::Mismatch(0b100))])),
        ("r4".to_string(), Verdict::Skip),
    ];
    let m = red_rows("rext", &rows).expect("two red rows");
    assert!(m.starts_with("2 rext rows red: [r1: decoded YUV md5 "), "{m}");
    assert!(m.contains("want 00000000000000000000000000000000 (1 frames; first differing frame against the zip's yuv: None)"), "{m}");
    assert!(m.ends_with(", r3: decoded picture hash SEI mismatch at (POC, planes) [(0, Mismatch(Cr))]]"), "{m}");
    assert!(!m.contains("r2") && !m.contains("r4"), "{m}");
    assert_eq!(red_rows("rext", &rows[1..2]), None);
}

#[test]
fn verdict_per_vector_panic_text_is_verbatim() {
    let text = |v: Verdict| {
        let p = std::panic::catch_unwind(|| report("X", v)).expect_err("a red verdict panics");
        p.downcast::<String>().map(|s| *s).unwrap_or_default()
    };
    assert_eq!(text(Verdict::Decode(Error::Truncated.to_string())), "X: hevc: truncated");
    assert_eq!(text(Verdict::HashMismatch(vec![(1, HashCheck::Mismatch(0b110))])), "X: decoded picture hash SEI mismatch at (POC, planes) [(1, Mismatch(Cb+Cr))]");
    report("X", Verdict::Skip);
    report("X", Verdict::Ok("1 frames md5 ok".into()));
}

// ---- HEVCREXTLOOPCATCHPANIC (LEDGER SR1099): one rext row under catch_unwind; a panic is the row's Decode verdict (named tail block) ----

/// HEVCREXTLOOPCATCHPANIC (SR1099): run one loop row under ONE catch_unwind; a panic (fetch's "not in the zip", a decoder
/// bug) becomes the row's `Decode("panic: {msg}")`, so the loop goes on to the next row. `None` is a row refused by name.
fn row_verdict(row: impl FnOnce() -> Option<Verdict>) -> Option<Verdict> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(row)).unwrap_or_else(|p| {
        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
        Some(Verdict::Decode(format!("panic: {}", msg.as_deref().unwrap_or("<non-string payload>"))))
    })
}

/// The text of a row's verdict (`Verdict` has no Debug): `None` for a refused row.
fn caught(row: impl FnOnce() -> Option<Verdict>) -> Option<String> {
    row_verdict(row).map(|v| match v {
        Verdict::Decode(s) => format!("Decode({s})"),
        v => v.to_string(),
    })
}

#[test]
fn verdict_catch_panic_str_payload_is_a_decode_verdict() {
    assert_eq!(caught(|| std::panic::panic_any("boom")), Some("Decode(panic: boom)".into()));
}

#[test]
fn verdict_catch_panic_string_payload_is_a_decode_verdict() {
    let (rel, bitstream) = ("hevc/rext/R.zip", "r.bit");
    let got = caught(|| panic!("{rel}: {bitstream} is not in the zip"));
    assert_eq!(got, Some("Decode(panic: hevc/rext/R.zip: r.bit is not in the zip)".into()));
}

#[test]
fn verdict_catch_panic_non_string_payload_falls_back() {
    assert_eq!(caught(|| std::panic::panic_any(7u32)), Some("Decode(panic: <non-string payload>)".into()));
}

#[test]
fn verdict_catch_panic_passes_a_returned_verdict_through() {
    let ok = row_verdict(|| Some(Verdict::Ok("1 frames md5 ok".into())));
    assert!(matches!(&ok, Some(Verdict::Ok(s)) if s == "1 frames md5 ok"));
    assert!(row_verdict(|| None).is_none(), "a row refused by name stays None");
}

#[test]
fn verdict_catch_panic_red_rows_names_the_panicking_row() {
    let ok = || row_verdict(|| Some(Verdict::Ok("ok".into()))).expect("Ok row");
    let rows = vec![
        ("r1".to_string(), ok()),
        ("r2".to_string(), row_verdict(|| panic!("{}: r2.bit is not in the zip", "hevc/rext/r2.zip")).expect("panic row")),
        ("r3".to_string(), ok()),
    ];
    assert_eq!(red_rows("rext", &rows), Some("1 rext rows red: [r2: panic: hevc/rext/r2.zip: r2.bit is not in the zip]".into()));
}

// ---- HEVCSEIPERPLANEFIRSTSAMPLE (LEDGER SR1101): the first differing sample per plane of the md5 verdict's first differing frame (named tail block) ----

/// HEVCSEIPERPLANEFIRSTSAMPLE (SR1101): per plane (Y, Cb, Cr), the first sample of `frame` that differs from `yuv` (the
/// zip's bytes from that frame on, laid out as `yuv_bytes` writes them: two bytes low first when `planes16` is Some,
/// else one) as (x, y, got, want); None when the plane matches. A sample past the end of a short `yuv` is not compared.
/// A hash SEI names no sample: this is the md5 verdict's aid, `HashCheck` is unchanged.
fn first_diff(frame: &Frame, yuv: &[u8]) -> [Option<(usize, usize, u16, u16)>; 3] {
    let (sw, _) = frame.chroma.sub();
    let b = if frame.planes16.is_some() { 2 } else { 1 };
    let mut off = 0;
    std::array::from_fn(|c| {
        let got: Vec<u16> = match &frame.planes16 { Some(p) => p[c].clone(), None => frame.planes[c].iter().map(|&v| v as u16).collect() };
        let w = if c == 0 { frame.width as usize } else { frame.width as usize / sw };
        let want = |i: usize| yuv.get(off + i * b..off + i * b + b).map(|s| if b == 2 { u16::from_le_bytes([s[0], s[1]]) } else { s[0] as u16 });
        let hit = got.iter().enumerate().find_map(|(i, &g)| want(i).filter(|&w_| w_ != g).map(|w_| (i % w, i / w, g, w_)));
        off += got.len() * b;
        hit
    })
}

/// The Md5 Display's tail: ` first diff Y (x, y) got g want w; Cr ...` over the planes that differ; "" when none does.
fn diff_text(d: &Option<[Option<(usize, usize, u16, u16)>; 3]>) -> String {
    let parts: Vec<String> = d.iter().flat_map(|d| ["Y", "Cb", "Cr"].into_iter().zip(d)).filter_map(|(p, s)| s.map(|(x, y, g, w)| format!("{p} ({x}, {y}) got {g} want {w}"))).collect();
    if parts.is_empty() { String::new() } else { format!(" first diff {}", parts.join("; ")) }
}

/// A 4x4 4:2:0 frame (chroma 2x2); `wide` carries the samples in `planes16` at 10 bits (two bytes in `yuv_bytes`).
fn frame44(y: Vec<u16>, cb: Vec<u16>, cr: Vec<u16>, wide: bool) -> Frame {
    let narrow = |p: &Vec<u16>| p.iter().map(|&v| v as u8).collect::<Vec<u8>>();
    let depth = if wide { 10 } else { 8 };
    Frame {
        width: 4,
        height: 4,
        planes: [narrow(&y), narrow(&cb), narrow(&cr)],
        poc: 0,
        tag: 0,
        full_range: false,
        bit_depth: depth,
        planes16: wide.then(|| [y.clone(), cb.clone(), cr.clone()]),
        chroma: h264_core::ChromaFormat::Yuv420,
        bit_depth_c: depth,
    }
}

#[test]
fn verdict_first_diff_8bit_names_y_and_cr() {
    let (y, cb, cr) = ((0..16).collect::<Vec<u16>>(), vec![5; 4], vec![6, 7, 8, 9]);
    let want = frame44(y.clone(), cb.clone(), cr.clone(), false);
    let (mut y2, mut cr2) = (y.clone(), cr.clone());
    y2[6] = 99; // Y (2, 1)
    cr2[2] = 3; // Cr (0, 1)
    let got = frame44(y2, cb, cr2, false);
    assert_eq!(first_diff(&got, &yuv_bytes(&want)), [Some((2, 1, 99, 6)), None, Some((0, 1, 3, 8))]);
}

#[test]
fn verdict_first_diff_10bit_reads_two_byte_samples() {
    let (y, cr) = (vec![0x200; 16], vec![0x100; 4]);
    let want = frame44(y.clone(), vec![0x1FF, 0x2FF, 0x1FF, 0x1FF], cr.clone(), true);
    let got = frame44(y, vec![0x1FF, 0x1FF, 0x1FF, 0x1FF], cr, true);
    assert_eq!(first_diff(&got, &yuv_bytes(&want)), [None, Some((1, 0, 0x1FF, 0x2FF)), None]);
}

#[test]
fn verdict_first_diff_identical_is_none() {
    let f = frame44((0..16).collect(), vec![1, 2, 3, 4], vec![5, 6, 7, 8], false);
    assert_eq!(first_diff(&f, &yuv_bytes(&f)), [None; 3]);
    let f10 = frame44(vec![0x3FF; 16], vec![0x200; 4], vec![1; 4], true);
    assert_eq!(first_diff(&f10, &yuv_bytes(&f10)), [None; 3]);
}

#[test]
fn verdict_first_diff_display_appends_only_with_samples() {
    let old = "decoded YUV md5 a, want b (2 frames; first differing frame against the zip's yuv: Some(Some(1)))";
    let md5 = |d| Verdict::Md5("a".into(), "b".into(), 2, Some(Some(1)), d).to_string();
    assert_eq!(md5(Some([Some((2, 1, 12, 13)), None, Some((0, 1, 7, 9))])), format!("{old} first diff Y (2, 1) got 12 want 13; Cr (0, 1) got 7 want 9"));
    assert_eq!(md5(None), old);
    assert_eq!(md5(Some([None; 3])), old);
}
