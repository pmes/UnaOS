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

fn decode(b: &[u8]) -> (u32, u16, Vec<f32>) {
    let (i, pcm) = audio_core::decode_all(b).expect("decode");
    (i.rate, i.channels, pcm)
}

fn desc(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![tag, body.len() as u8];
    v.extend_from_slice(body);
    v
}

/// An `esds` body (ISO/IEC 14496-1 §7.2.6.5) around a DecoderSpecificInfo.
fn esds(oti: u8, dsi: &[u8]) -> Vec<u8> {
    let mut dcd = vec![oti, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    if !dsi.is_empty() { dcd.extend_from_slice(&desc(5, dsi)); }
    let mut es = vec![0, 1, 0];
    es.extend_from_slice(&desc(4, &dcd));
    es.extend_from_slice(&desc(6, &[2]));
    let mut v = vec![0, 0, 0, 0];
    v.extend_from_slice(&desc(3, &es));
    v
}

fn audio_track(id: u32, fourcc: [u8; 4], oti: u8, dsi: &[u8], ts: u32, rate: u32, ch: u16, units: Vec<(Vec<u8>, u32)>) -> demux_core::build::MediaTrack {
    use demux_core::build::{MediaTrack, SampleSpec, TrackSpec};
    let mut dts = 0i64;
    let samples = units
        .into_iter()
        .map(|(data, dur)| {
            let s = SampleSpec { data, dts, pts: dts, duration: dur, keyframe: true };
            dts += dur as i64;
            s
        })
        .collect();
    let spec = TrackSpec {
        id,
        kind: demux_core::TrackKind::Audio,
        fourcc,
        config_box: (&fourcc == b"mp4a").then_some(*b"esds"),
        codec_id: "A_AAC",
        config: if &fourcc == b"mp4a" { esds(oti, dsi) } else { Vec::new() },
        timescale: ts,
        width: 0,
        height: 0,
        sample_rate: rate,
        channels: ch,
        bit_depth: 16,
        default_duration_ns: 0,
    };
    MediaTrack { spec, samples }
}

fn with_video(audio: demux_core::build::MediaTrack, edit: Option<i64>, fragment: usize) -> Vec<u8> {
    let video = demux_core::build::test_pattern_track(1, 64, 48, 30, 12, 6);
    let opts = demux_core::build::Mp4Options {
        movie_timescale: 1000,
        chunk: 4,
        edits: edit.map(|m| vec![(audio.spec.id, 0, m)]).unwrap_or_default(),
        co64: false,
        fragment,
    };
    demux_core::build::mp4(&[video, audio], &opts)
}

/// VIDEOPLAYER (B434) left AAC beside a video track unproven: TEST.M4A's AAC access units, rewritten beside a
/// test-pattern video track (progressive and fragmented), decode to the audio-only file's PCM.
#[test]
fn aac_beside_video() {
    let Ok(b) = std::fs::read(testf().join("TEST.M4A")) else { eprintln!("SKIP TEST.M4A: not staged"); return };
    let (rate, ch, want) = decode(&b);
    let d = demux_core::Demuxer::open(b).unwrap();
    let ai = d.tracks().iter().position(|t| t.kind == demux_core::TrackKind::Audio).unwrap();
    let t = d.tracks()[ai].clone();
    let units: Vec<_> = d.track_samples(ai).map(|s| (d.packet_at(s).data, s.duration as u32)).collect();
    let mt = t.edit.map(|e| e.media_time).expect("TEST.M4A carries an edit list");
    for frag in [0usize, 5] {
        let f = with_video(audio_track(2, *b"mp4a", 0x40, &t.config, t.timebase.den as u32, t.sample_rate, t.channels, units.clone()), Some(mt), frag);
        let v = demux_core::Demuxer::open(f.clone()).unwrap();
        assert_eq!(v.tracks()[0].kind, demux_core::TrackKind::Video);
        let (r2, c2, got) = decode(&f);
        assert_eq!((r2, c2), (rate, ch));
        let n = want.len().min(got.len());
        assert!(n >= want.len() - ch as usize * 1024, "aac beside video (fragment={frag}): {} of {}", got.len(), want.len());
        assert!(want[..n] == got[..n], "aac beside video (fragment={frag}): PCM differs");
        eprintln!("MP4ONE aac beside video fragment={frag}: {} frames identical", n / ch as usize);
    }
}

/// MP3 in MP4 (`.mp3` sample entry, and `mp4a` with ObjectTypeIndication 0x6B) beside a video track: TEST.MP3's
/// frames, one access unit each, decode to the bare frames' PCM.
#[test]
fn mp3_beside_video() {
    let Ok(b) = std::fs::read(testf().join("TEST.MP3")) else { eprintln!("SKIP TEST.MP3: not staged"); return };
    let mut p = if b.len() >= 10 && &b[..3] == b"ID3" { 10 + ((b[6] as usize & 127) << 21 | (b[7] as usize & 127) << 14 | (b[8] as usize & 127) << 7 | (b[9] as usize & 127)) } else { 0 };
    let mut frames = Vec::new();
    let mut rate = 0;
    while p + 4 <= b.len() {
        let Some(h) = audio_core::mp3::Header::parse(u32::from_be_bytes(b[p..p + 4].try_into().unwrap())) else { p += 1; continue };
        let n = h.frame_len();
        if n == 0 || p + n > b.len() { break; }
        rate = h.rate();
        frames.push((b[p..p + n].to_vec(), h.samples() as u32));
        p += n;
    }
    assert!(frames.len() > 4, "TEST.MP3: frames");
    let bare: Vec<u8> = frames.iter().flat_map(|f| f.0.clone()).collect();
    let (r0, c0, want) = decode(&bare);
    for (fourcc, oti) in [(*b".mp3", 0u8), (*b"mp4a", 0x6B)] {
        let f = with_video(audio_track(2, fourcc, oti, &[], rate, rate, c0, frames.clone()), None, 0);
        let (r, c, got) = decode(&f);
        assert_eq!((r, c), (r0, c0));
        assert!(got == want, "mp3 beside video ({}): PCM differs", String::from_utf8_lossy(&fourcc));
        eprintln!("MP4ONE mp3 beside video entry={}: {} frames identical", String::from_utf8_lossy(&fourcc), got.len() / c as usize);
    }
}
