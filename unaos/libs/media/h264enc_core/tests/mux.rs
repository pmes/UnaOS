// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENC M3 (LEDGER SR197): the mux round trip. Ten encoded frames go into an MP4 through
//! demux_core's writer (`build::mp4`: an `avc1` sample entry whose `avcC` is the encoder's own
//! AVCDecoderConfigurationRecord, written verbatim; one length-prefixed sample per access unit with
//! its sync flag), are opened by demux_core's `Demuxer`, and are decoded by h264_core sample by
//! sample: one frame per sample, each the encoder's reconstruction.

use demux_core::build::{self, MediaTrack, Mp4Options, SampleSpec, TrackSpec};
use demux_core::{Codec, Demuxer, TrackKind};
use h264_core::Decoder;
use h264enc_core::{Config, Encoder, Yuv420};

const W: usize = 64;
const H: usize = 48;

fn frame(t: usize) -> [Vec<u8>; 3] {
    let mut p = [vec![0u8; W * H], vec![0u8; W * H / 4], vec![0u8; W * H / 4]];
    let (sx, sy) = (4 + 3 * t, 10 + t);
    for y in 0..H {
        for x in 0..W {
            let inside = x >= sx && x < sx + 16 && y >= sy && y < sy + 16;
            p[0][y * W + x] = if inside { 210 } else { (40 + x * 100 / W + y * 60 / H) as u8 };
        }
    }
    for y in 0..H / 2 {
        for x in 0..W / 2 {
            p[1][y * (W / 2) + x] = (100 + x) as u8;
            p[2][y * (W / 2) + x] = (150 - y) as u8;
        }
    }
    p
}

#[test]
fn m3_ten_frames_through_mp4_demux_and_decode() {
    let mut enc = Encoder::new(Config { width: W as u32, height: H as u32, qp: 26, gop: 4, ..Default::default() }).unwrap();
    let mut samples = Vec::new();
    let mut recon = Vec::new();
    for t in 0..10 {
        let p = frame(t);
        let au = enc
            .encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 })
            .unwrap();
        samples.push(SampleSpec { data: au.avcc_sample(), dts: t as i64 * 1000, pts: t as i64 * 1000, duration: 1000, keyframe: au.keyframe });
        let r = enc.reconstruction();
        recon.push([r[0].to_vec(), r[1].to_vec(), r[2].to_vec()]);
    }
    let spec = TrackSpec {
        fourcc: *b"avc1",
        config_box: Some(*b"avcC"),
        codec_id: "V_MPEG4/ISO/AVC",
        config: enc.avcc(),
        timescale: 25000,
        default_duration_ns: 40_000_000,
        ..TrackSpec::test_pattern(1, W as u16, H as u16, 25)
    };
    let file = build::mp4(&[MediaTrack { spec, samples }], &Mp4Options::default());

    let d = Demuxer::open(file).unwrap();
    let tr = &d.tracks()[0];
    assert_eq!((tr.kind, tr.codec.clone(), tr.width, tr.height), (TrackKind::Video, Codec::Avc, W as u32, H as u32));
    assert_eq!(tr.config, enc.avcc(), "the avcC box carries the encoder's record verbatim");
    assert_eq!(tr.sample_count, 10);

    let mut dec = Decoder::new();
    dec.configure_avcc(&tr.config).unwrap();
    let samples: Vec<_> = d.track_samples(0).copied().collect();
    assert_eq!(samples.len(), 10);
    for (t, s) in samples.iter().enumerate() {
        let pk = d.packet_at(s);
        assert_eq!(pk.keyframe, t % 4 == 0, "sample {t} sync flag");
        assert_eq!(pk.pts, t as i64 * 1000);
        dec.decode_sample(&pk.data).unwrap();
        let mut f = dec.take_frames();
        assert_eq!(f.len(), 1, "sample {t}: one frame out per sample");
        let f = f.pop().unwrap();
        assert_eq!((f.width, f.height), (W as u32, H as u32));
        for c in 0..3 {
            assert_eq!(f.planes[c], recon[t][c], "sample {t} plane {c}: decode differs from the reconstruction");
        }
    }
}

/// H264ENCCOLOURHOST (LEDGER SR377): `Config::colour` is written as the SPS VUI's video_signal_type + colour_description and
/// h264_core's reader (`ps::avcc_sps`, the one the Player's `colour_of` calls) reads the same codes back; the access units
/// are byte for byte the uncoloured stream's (the VUI lives in the avcC only); without a colour the SPS is the one measured
/// at 2e1698471 before the writer changed (Baseline and the B pyramid's Main).
#[test]
fn h264enccolourhost_vui_colour_reads_back() {
    let code = |c: Option<(u8, u8, u8, bool)>, bframes: u8| {
        let mut enc = Encoder::new(Config { width: W as u32, height: H as u32, qp: 26, gop: 4, bframes, colour: c, ..Default::default() }).unwrap();
        let mut units = Vec::new();
        for t in 0..6 {
            let p = frame(t);
            let yuv = Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 };
            units.extend(enc.push(&yuv).unwrap().iter().map(|a| a.avcc_sample()));
        }
        units.extend(enc.flush().unwrap().iter().map(|a| a.avcc_sample()));
        (enc.sps_nal().to_vec(), enc.avcc(), units)
    };
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let (sps0, avcc0, units0) = code(None, 0);
    assert_eq!(hex(&sps0), "6742c00ada11e806d0442350", "the uncoloured Baseline SPS is unchanged");
    assert_eq!(hex(&code(None, 2).0), "674d400af208f4036822114580", "the uncoloured Main (B pyramid) SPS is unchanged");
    let s = h264_core::ps::avcc_sps(&avcc0).unwrap();
    assert!(!s.video_signal_type);
    for c in [(1u8, 1u8, 1u8, false), (2, 2, 6, false), (9, 16, 9, true)] {
        for bframes in [0u8, 2] {
            let (sps, avcc, units) = code(Some(c), bframes);
            let s = h264_core::ps::avcc_sps(&avcc).expect("the coloured SPS parses");
            assert!(s.video_signal_type, "{c:?} b{bframes}: video_signal_type_present_flag");
            assert_eq!((s.colour_primaries, s.transfer_characteristics, s.matrix_coefficients, s.full_range), c, "b{bframes}");
            assert_eq!((s.width_mbs, s.height_map_units), (W as u32 / 16, H as u32 / 16), "the VUI follows the picture size fields intact");
            if bframes == 0 {
                assert_ne!(sps, sps0);
                assert_eq!(units, units0, "{c:?}: the access units are the uncoloured stream's");
            }
        }
    }
}
