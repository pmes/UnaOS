// SPDX-License-Identifier: LGPL-3.0-or-later
//! MP4ONE (rmbp-ledger B464): audio_core reads MP4/M4A through demux_core, the one ISOBMFF parser (R79). The proof
//! is byte-identity with the retired `audio_core::mp4` reader: every MP4 vector's full decode (FNV-1a over the f32
//! bits), its frame count, and its seek points (byte, sample, landed, the PCM after the seek) hash to the values the
//! old reader produced on the cut tip (pinned below, recorded before the reader was deleted). TEST.M4A comes from
//! `$UNAOS_TESTF_DIR` or `unaos/target/testf` (absent: SKIPPED out loud).
use audio_core::{AudioDecoder, Decoder};
use std::path::PathBuf;

fn data() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/aac") }
fn testf() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

fn fnv(h: &mut u64, x: &[f32]) {
    for v in x {
        for b in v.to_bits().to_le_bytes() { *h ^= b as u64; *h = h.wrapping_mul(0x100_0000_01b3); }
    }
}

fn drain(d: &mut Decoder) -> (usize, u64) {
    let ch = d.info().channels as usize;
    let mut buf = vec![0f32; 4096 * ch];
    let (mut n, mut h) = (0usize, 0xcbf2_9ce4_8422_2325u64);
    loop {
        let k = d.next(&mut buf).expect("decode");
        if k == 0 { break; }
        fnv(&mut h, &buf[..k * ch]);
        n += k;
    }
    (n, h)
}

/// One line per file: the open's info, the full decode, and four seeks.
pub fn fingerprint(b: &[u8]) -> String {
    let mut d = Decoder::open_bytes(b.to_vec()).expect("open");
    let i = d.info();
    let (n, h) = drain(&mut d);
    let mut s = format!("rate={} ch={} frames={:?} n={n} h={h:016x}", i.rate, i.channels, i.frames);
    for ms in [0u64, 40, 200, 999] {
        let mut d = Decoder::open_bytes(b.to_vec()).expect("open");
        let p = d.seek(ms).expect("seek").expect("an mp4 table");
        let (n, h) = drain(&mut d);
        s += &format!(" @{ms}:{}/{}/{}/{n}/{h:016x}", p.byte, p.sample, p.landed);
    }
    s
}

/// What the retired reader printed (cut tip f4e0613c).
const PINNED: &[(&str, &str)] = &[
    ("m01.m4a", "rate=44100 ch=2 frames=Some(30000) n=30000 h=66cadc975171d652 @0:1276/0/0/30000/66cadc975171d652 @40:1710/1024/1764/28236/75a44f02a7f3cf7d @200:4194/8192/8820/21180/f714b6b6a2f18e1d @999:12272/29696/30000/0/cbf29ce484222325"),
    ("m04.m4a", "rate=22050 ch=2 frames=Some(19456) n=19456 h=4a9db27f03a41215 @0:748/0/0/19456/4a9db27f03a41215 @40:748/0/882/18574/05b22506affdf8d5 @200:1899/4096/4410/15046/d72342d37201646e @999:7701/19456/19456/0/cbf29ce484222325"),
    ("m07.m4a", "rate=32000 ch=6 frames=Some(18432) n=18432 h=d8f73dfda4014e28 @0:720/0/0/18432/d8f73dfda4014e28 @40:720/1024/1280/17152/28b87943aacca534 @200:4978/6144/6400/12032/064f574034d48e20 @999:15324/18432/18432/0/cbf29ce484222325"),
    ("m10.m4a", "rate=44100 ch=3 frames=Some(20000) n=20000 h=3722fcabc9d03953 @0:36/0/0/20000/3722fcabc9d03953 @40:493/1024/1764/18236/f50e2ed697bb87e0 @200:3835/8192/8820/11180/0b02e54a14ab1881 @999:9503/19456/20000/0/cbf29ce484222325"),
    ("TEST.M4A", "rate=44100 ch=1 frames=Some(12701) n=12701 h=f65343efaa6c4e22 @0:40/0/0/12701/f65343efaa6c4e22 @40:207/1024/1764/10937/4492975dd538ed11 @200:997/8192/8820/3881/1077060fb35c108b @999:1480/12288/12701/0/cbf29ce484222325"),
];

#[test]
fn mp4_byte_identical() {
    let mut seen = 0;
    for (name, want) in PINNED {
        let p = if name.starts_with("TEST.") { testf().join(name) } else { data().join(name) };
        let Ok(b) = std::fs::read(&p) else { eprintln!("SKIP {name}: not staged"); continue };
        let got = fingerprint(&b);
        eprintln!("{name} {got}");
        assert_eq!(&got, want, "{name}");
        seen += 1;
    }
    eprintln!("MP4ONE: {seen}/{} byte-identical", PINNED.len());
}
