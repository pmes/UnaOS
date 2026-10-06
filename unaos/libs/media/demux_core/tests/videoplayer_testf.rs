// SPDX-License-Identifier: LGPL-3.0-or-later
//! VIDEOPLAYER (rmbp-ledger B434): the kernel Player's chain (`crates/kernel/src/video/vplay.rs`) over the test-f
//! video samples (`$UNAOS_TESTF_DIR` or `unaos/target/testf`): every picture packet decodes through vp8_core /
//! av1_core, and TEST.WEBM's Vorbis track (Matroska CodecPrivate, Xiph-laced headers) decodes through
//! audio_core::vorbis. Absent samples are SKIPPED out loud.
use demux_core::{Codec, Demuxer};
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

/// The Matroska codec mapping's Xiph lacing (the same arithmetic as `vplay::xiph_split`).
fn xiph_split(c: &[u8]) -> Option<Vec<&[u8]>> {
    let n = *c.first()? as usize + 1;
    let mut i = 1;
    let mut sizes = Vec::new();
    for _ in 0..n - 1 {
        let mut s = 0usize;
        loop {
            let b = *c.get(i)?;
            i += 1;
            s += b as usize;
            if b != 255 {
                break;
            }
        }
        sizes.push(s);
    }
    let mut out = Vec::new();
    for s in sizes {
        out.push(c.get(i..i + s)?);
        i += s;
    }
    out.push(c.get(i..)?);
    Some(out)
}

#[test]
fn testf_video_decodes() {
    for (name, codec) in [("TEST.WEBM", Codec::Vp8), ("TEST.MP4", Codec::Av1)] {
        let Ok(b) = std::fs::read(dir().join(name)) else {
            eprintln!("SKIP {name}: not fetched");
            continue;
        };
        let mut d = Demuxer::open(b).unwrap();
        let vt = d.video_track().unwrap().clone();
        assert_eq!(vt.codec, codec, "{name}");
        let mut vp8 = vp8_core::Decoder::new();
        let mut av1 = (codec == Codec::Av1).then(|| av1_core::image::StreamDecoder::new(&vt.config).unwrap());
        let mut shown = 0u64;
        while let Some(p) = d.next_packet() {
            if p.track != vt.id {
                continue;
            }
            let (w, h) = match av1.as_mut() {
                Some(a) => {
                    let img = av1_core::image::planes_to_rgba(&a.decode_temporal_unit(&p.data).unwrap(), av1_core::image::Upsampling::Bilinear);
                    (img.w, img.h)
                }
                None => match vp8.decode(&p.data).unwrap() {
                    Some(pic) => {
                        let y = vp8_core::Yuv420::from_picture(&pic);
                        (y.width, y.height)
                    }
                    None => continue,
                },
            };
            assert_eq!((w, h), (vt.width, vt.height), "{name}");
            shown += 1;
        }
        eprintln!("{name}: {codec:?} {}x{} shown={shown} of {}", vt.width, vt.height, vt.sample_count);
        assert_eq!(shown, vt.sample_count, "{name}: every packet a picture");
    }
}

#[test]
fn testf_webm_vorbis_sound() {
    let Ok(b) = std::fs::read(dir().join("TEST.WEBM")) else {
        eprintln!("SKIP TEST.WEBM: not fetched");
        return;
    };
    let mut d = Demuxer::open(b).unwrap();
    let at = d.audio_track().unwrap().clone();
    assert_eq!(at.codec, Codec::Vorbis);
    let h = xiph_split(&at.config).unwrap();
    assert_eq!(h.len(), 3);
    let s = audio_core::vorbis::Setup::parse(h[0], h[2]).unwrap();
    let (rate, ch) = (s.rate, s.channels);
    let mut dec = audio_core::vorbis::VorbisDecoder::new(s);
    let (mut out, mut n) = (Vec::new(), 0usize);
    while let Some(p) = d.next_packet() {
        if p.track == at.id {
            n += dec.decode(&p.data, &mut out).unwrap();
        }
    }
    let secs_x1000 = n as u64 * 1000 / rate as u64;
    eprintln!("TEST.WEBM vorbis rate={rate} ch={ch} samples={n} ms={secs_x1000} track_ms={}", at.duration_ns / 1_000_000);
    assert!(secs_x1000.abs_diff(at.duration_ns / 1_000_000) < 100, "the sound spans the track");
}
