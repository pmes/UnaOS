// SPDX-License-Identifier: LGPL-3.0-or-later
//! Known-answer tests on hand-built containers. Two kinds: files written by `build` (each KAT
//! states the exact packet table it must read back), and files assembled byte by byte here, so
//! the reader is not only checked against its own writer.

use demux_core::build::{self, Lacing, MediaTrack, MkvOptions, Mp4Options, SampleSpec, TestPatternPacket, TrackSpec};
use demux_core::{Codec, Demuxer, Error, Format, Packet, TrackKind};

fn all(d: &mut Demuxer) -> Vec<Packet> {
    let mut v = Vec::new();
    while let Some(p) = d.next_packet() {
        v.push(p);
    }
    v
}

fn audio_track(id: u32, n: usize, size_of: impl Fn(usize) -> usize) -> MediaTrack {
    // 20 ms Opus-like frames at 48 kHz timescale.
    MediaTrack {
        spec: TrackSpec {
            id,
            kind: TrackKind::Audio,
            fourcc: *b"Opus",
            config_box: Some(*b"dOps"),
            codec_id: "A_OPUS",
            config: vec![0, 2, 0x01, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 0], // pre-skip 312
            timescale: 48_000,
            width: 0,
            height: 0,
            sample_rate: 48_000,
            channels: 2,
            bit_depth: 0,
            default_duration_ns: 20_000_000,
        },
        samples: (0..n)
            .map(|i| SampleSpec {
                data: (0..size_of(i)).map(|b| (b + i) as u8).collect(),
                dts: i as i64 * 960,
                pts: i as i64 * 960,
                duration: 960,
                keyframe: true,
            })
            .collect(),
    }
}

// ----------------------------------------------------------------- MP4, via the writer

#[test]
fn kat_mp4_progressive_test_pattern() {
    let t = build::test_pattern_track(1, 64, 48, 25, 50, 10);
    let file = build::mp4(&[t], &Mp4Options { chunk: 4, ..Default::default() });
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.format(), Format::Mp4);
    let tr = &d.tracks()[0];
    assert_eq!((tr.kind.clone(), tr.codec.clone(), tr.width, tr.height), (TrackKind::Video, Codec::TestPattern, 64, 48));
    assert_eq!(tr.sample_count, 50);
    assert_eq!(tr.duration_ns, 2_000_000_000);
    assert_eq!(d.duration_ns(), 2_000_000_000);
    let p = all(&mut d);
    assert_eq!(p.len(), 50);
    for (i, pk) in p.iter().enumerate() {
        assert_eq!(pk.pts, i as i64 * 1000);
        assert_eq!(pk.dts, pk.pts);
        assert_eq!(pk.duration, 1000);
        assert_eq!(pk.keyframe, i % 10 == 0, "frame {i}");
        assert_eq!(TestPatternPacket::decode(&pk.data).unwrap().frame, i as u32);
    }
}

#[test]
fn kat_mp4_bframes_ctts_and_edit_list() {
    // Decode order I P B B; presentation 0 3 1 2 (in frame units of 512), ctts shifts by 1024
    // and an edit list with media_time 1024 cancels it — the shape every H.264/AV1 muxer writes.
    let order = [0i64, 3, 1, 2, 4, 7, 5, 6];
    let spec = TrackSpec { fourcc: *b"av01", config_box: Some(*b"av1C"), codec_id: "V_AV1", config: vec![0x81, 0, 0, 0], timescale: 12_288, ..TrackSpec::test_pattern(1, 32, 32, 24) };
    let samples = order
        .iter()
        .enumerate()
        .map(|(i, &pres)| SampleSpec { data: vec![i as u8; 3 + i], dts: i as i64 * 512, pts: pres * 512 + 1024, duration: 512, keyframe: i % 4 == 0 })
        .collect();
    let file = build::mp4(&[MediaTrack { spec, samples }], &Mp4Options { edits: vec![(1, 0, 1024)], co64: true, ..Default::default() });
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.tracks()[0].codec, Codec::Av1);
    assert_eq!(d.tracks()[0].config, vec![0x81, 0, 0, 0]);
    let p = all(&mut d);
    let pts: Vec<i64> = p.iter().map(|x| x.pts / 512).collect();
    assert_eq!(pts, order.to_vec());
    let dts: Vec<i64> = p.iter().map(|x| x.dts / 512).collect();
    assert_eq!(dts, (0..8).collect::<Vec<_>>());
    assert_eq!(p[5].data, vec![5u8; 8]);
    assert_eq!(d.tracks()[0].duration_ns, 8 * 512 * 1_000_000_000 / 12_288);
}

#[test]
fn kat_mp4_empty_edit_delays_presentation() {
    // A 500 ms empty edit (movie timescale 1000) then the media from 0: pts move +0.5 s.
    let t = build::test_pattern_track(1, 16, 16, 10, 5, 5);
    let file = build::mp4(&[t], &Mp4Options { edits: vec![(1, 500, 0)], ..Default::default() });
    let mut d = Demuxer::open(file).unwrap();
    let tr = d.tracks()[0].clone();
    let p = all(&mut d);
    assert_eq!(tr.to_ns(p[0].pts), 500_000_000);
    assert_eq!(tr.to_ns(p[4].pts), 900_000_000);
}

#[test]
fn kat_mp4_av_interleave_and_merge_order() {
    let v = build::test_pattern_track(1, 32, 32, 25, 10, 5); // 40 ms frames
    let a = audio_track(2, 20, |i| 10 + i); // 20 ms frames
    let file = build::mp4(&[v, a], &Mp4Options { chunk: 3, ..Default::default() });
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.tracks().len(), 2);
    let at = d.audio_track().unwrap().clone();
    assert_eq!((at.codec.clone(), at.sample_rate, at.channels), (Codec::Opus, 48_000, 2));
    assert_eq!(at.codec_delay_ns, 312 * 1_000_000_000 / 48_000);
    let p = all(&mut d);
    assert_eq!(p.len(), 30);
    // Decode-time order across tracks, ties to the first track.
    let mut last = i64::MIN;
    for pk in &p {
        let tb = d.tracks()[d.track_index(pk.track).unwrap()].timebase;
        let ns = tb.to_ns(pk.dts);
        assert!(ns >= last);
        last = ns;
    }
    let audio: Vec<&Packet> = p.iter().filter(|x| x.track == 2).collect();
    assert_eq!(audio[7].data.len(), 17);
    assert_eq!(audio[7].data[0], 7);
}

#[test]
fn kat_mp4_fragmented_round_trip() {
    let v = build::test_pattern_track(1, 32, 32, 30, 45, 15);
    let a = audio_track(2, 60, |i| 5 + i % 7);
    let file = build::mp4(&[v.clone(), a.clone()], &Mp4Options { fragment: 15, ..Default::default() });
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.declared_duration_ns(), Some(1_500_000_000));
    let p = all(&mut d);
    let vp: Vec<&Packet> = p.iter().filter(|x| x.track == 1).collect();
    let ap: Vec<&Packet> = p.iter().filter(|x| x.track == 2).collect();
    assert_eq!((vp.len(), ap.len()), (45, 60));
    for (i, pk) in vp.iter().enumerate() {
        assert_eq!(pk.pts, i as i64 * 1000);
        assert_eq!(pk.keyframe, i % 15 == 0);
        assert_eq!(pk.data, v.samples[i].data);
    }
    for (i, pk) in ap.iter().enumerate() {
        assert_eq!(pk.data, a.samples[i].data);
        assert_eq!(pk.dts, i as i64 * 960);
    }
}

#[test]
fn kat_seek_lands_on_keyframe() {
    let v = build::test_pattern_track(1, 32, 32, 10, 30, 8); // keys at 0, 8, 16, 24
    let a = audio_track(2, 150, |_| 4);
    let file = build::mp4(&[v, a], &Mp4Options::default());
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.seek(1_950_000_000), Some(1_600_000_000));
    let first_video = loop {
        let p = d.next_packet().unwrap();
        if p.track == 1 {
            break p;
        }
    };
    assert!(first_video.keyframe);
    assert_eq!(TestPatternPacket::decode(&first_video.data).unwrap().frame, 16);
    assert_eq!(d.seek(-5), Some(0));
    assert_eq!(d.seek(10_000_000_000), Some(2_400_000_000));
}

// ----------------------------------------------------------------- Matroska, via the writer

#[test]
fn kat_webm_simpleblocks_round_trip() {
    let v = build::test_pattern_track(1, 64, 48, 25, 50, 10);
    let file = build::mkv(&[v], &MkvOptions { cluster_blocks: 7, ..Default::default() });
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.format(), Format::WebM);
    let t = d.tracks()[0].clone();
    assert_eq!((t.codec.clone(), t.width, t.height, t.sample_count), (Codec::TestPattern, 64, 48, 50));
    assert_eq!(d.duration_ns(), 2_000_000_000);
    let p = all(&mut d);
    for (i, pk) in p.iter().enumerate() {
        assert_eq!(pk.pts, i as i64 * 40_000_000);
        assert_eq!(pk.duration, 40_000_000);
        assert_eq!(pk.keyframe, i % 10 == 0);
        assert_eq!(TestPatternPacket::decode(&pk.data).unwrap().frame, i as u32);
    }
}

#[test]
fn kat_mkv_unknown_sizes_blockgroups_no_default_duration() {
    let mut v = build::test_pattern_track(1, 32, 32, 25, 20, 5);
    v.spec.default_duration_ns = 0;
    let file = build::mkv(
        &[v],
        &MkvOptions { webm: false, unknown_size_segment: true, unknown_size_cluster: true, block_groups: true, cluster_blocks: 6, ..Default::default() },
    );
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.format(), Format::Matroska);
    let p = all(&mut d);
    assert_eq!(p.len(), 20);
    for (i, pk) in p.iter().enumerate() {
        assert_eq!(pk.pts, i as i64 * 40_000_000);
        assert_eq!(pk.duration, 40_000_000, "BlockDuration");
        assert_eq!(pk.keyframe, i % 5 == 0, "ReferenceBlock absent ⇔ key");
    }
}

fn lacing_case(lacing: Lacing, sizes: fn(usize) -> usize) {
    let a = audio_track(2, 23, sizes);
    let v = build::test_pattern_track(1, 16, 16, 10, 5, 5);
    let file = build::mkv(&[v, a.clone()], &MkvOptions { lace: 4, lacing, ..Default::default() });
    let mut d = Demuxer::open(file).unwrap();
    let p = all(&mut d);
    let ap: Vec<&Packet> = p.iter().filter(|x| x.track == 2).collect();
    assert_eq!(ap.len(), 23, "{lacing:?}");
    for (i, pk) in ap.iter().enumerate() {
        assert_eq!(pk.data, a.samples[i].data, "{lacing:?} frame {i}");
        assert_eq!(pk.pts, i as i64 * 20_000_000, "{lacing:?} frame {i}");
    }
}

#[test]
fn kat_mkv_xiph_lacing() {
    lacing_case(Lacing::Xiph, |i| [3, 254, 255, 256, 600, 1][i % 6]);
}
#[test]
fn kat_mkv_ebml_lacing() {
    lacing_case(Lacing::Ebml, |i| [300, 2, 9000, 127, 128, 16_383][i % 6]);
}
#[test]
fn kat_mkv_fixed_lacing() {
    lacing_case(Lacing::Fixed, |_| 37);
}

// ----------------------------------------------------------------- byte-literal files

fn b(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}
fn fb(kind: &[u8; 4], ver: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut x = vec![ver];
    x.extend_from_slice(&flags.to_be_bytes()[1..]);
    x.extend_from_slice(body);
    b(kind, &x)
}
fn be(xs: &[u32]) -> Vec<u8> {
    xs.iter().flat_map(|x| x.to_be_bytes()).collect()
}

/// A fragmented file assembled by hand: two trafs in one moof, neither with a base offset nor
/// default-base-is-moof (so the second continues after the first's data), tfhd defaults, a trun
/// with first-sample-flags only, a 64-bit largesize mdat, and an `stz2`-free empty moov table.
#[test]
fn kat_literal_fragmented_mp4() {
    let mvhd = fb(b"mvhd", 0, 0, &[be(&[0, 0, 1000, 0]), vec![0; 80]].concat());
    let trak = |id: u32, hdlr: &[u8; 4], entry: Vec<u8>| {
        let tkhd = fb(b"tkhd", 0, 3, &[be(&[0, 0, id]), vec![0; 68]].concat());
        let mdhd = fb(b"mdhd", 0, 0, &be(&[0, 0, 1000, 0, 0x55C4_0000]));
        let hd = fb(b"hdlr", 0, 0, &[be(&[0]), hdlr.to_vec(), vec![0; 13]].concat());
        let stsd = fb(b"stsd", 0, 0, &[be(&[1]), entry].concat());
        let empty = |k: &[u8; 4], extra: u32| fb(k, 0, 0, &be(&vec![0; extra as usize]));
        let stbl = b(b"stbl", &[stsd, empty(b"stts", 1), empty(b"stsc", 1), empty(b"stsz", 2), empty(b"stco", 1)].concat());
        let minf = b(b"minf", &stbl);
        b(b"trak", &[tkhd, b(b"mdia", &[mdhd, hd, minf].concat())].concat())
    };
    let vp09 = b(b"vp09", &[vec![0; 6], vec![0, 1], vec![0; 16], vec![0, 8, 0, 6], vec![0; 50], b(b"vpcC", &[1, 0, 0, 0])].concat());
    let opus = b(b"Opus", &[vec![0; 6], vec![0, 1], vec![0; 8], vec![0, 1, 0, 16, 0, 0, 0, 0], be(&[48_000 << 16]), b(b"dOps", &[0, 1, 0, 0, 0, 0, 0xBB, 0x80, 0, 0, 0])].concat());
    let trex = |id: u32| fb(b"trex", 0, 0, &be(&[id, 1, 100, 0, 0x0101_0000]));
    let moov = b(b"moov", &[mvhd, trak(7, b"vide", vp09), trak(9, b"soun", opus), b(b"mvex", &[trex(7), trex(9)].concat())].concat());
    let ftyp = b(b"ftyp", b"iso6\0\0\0\0iso6");

    // traf 7: tfhd with default size 4 (0x10); trun: 3 samples, data-offset + first-sample-flags.
    // traf 9: tfhd with default duration 20 + default size 2, no trun data offset → continues.
    let build_moof = |data_off: i32| {
        let mfhd = fb(b"mfhd", 0, 0, &be(&[1]));
        let tfhd7 = fb(b"tfhd", 0, 0x10, &be(&[7, 4]));
        let tfdt7 = fb(b"tfdt", 0, 0, &be(&[5000]));
        let trun7 = fb(b"trun", 0, 0x1 | 0x4, &[be(&[3]), (data_off).to_be_bytes().to_vec(), be(&[0x0200_0000])].concat());
        let tfhd9 = fb(b"tfhd", 0, 0x8 | 0x10, &be(&[9, 20, 2]));
        let trun9 = fb(b"trun", 0, 0, &be(&[4]));
        b(b"moof", &[mfhd, b(b"traf", &[tfhd7, tfdt7, trun7].concat()), b(b"traf", &[tfhd9, trun9].concat())].concat())
    };
    let moof_len = build_moof(0).len();
    let moof = build_moof(moof_len as i32 + 16); // + largesize mdat header
    let payload: Vec<u8> = (0u8..20).collect(); // 3×4 video bytes then 4×2 audio bytes
    let mut mdat = 1u32.to_be_bytes().to_vec();
    mdat.extend_from_slice(b"mdat");
    mdat.extend_from_slice(&(16 + payload.len() as u64).to_be_bytes());
    mdat.extend_from_slice(&payload);
    let file = [ftyp, moov, moof, mdat].concat();

    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.tracks()[0].codec, Codec::Vp9);
    assert_eq!((d.tracks()[0].width, d.tracks()[0].height), (8, 6));
    assert_eq!(d.tracks()[1].codec, Codec::Opus);
    let p = all(&mut d);
    let v: Vec<&Packet> = p.iter().filter(|x| x.track == 7).collect();
    let a: Vec<&Packet> = p.iter().filter(|x| x.track == 9).collect();
    assert_eq!(v.iter().map(|x| x.dts).collect::<Vec<_>>(), vec![5000, 5100, 5200], "tfdt + trex duration");
    assert_eq!(v.iter().map(|x| x.keyframe).collect::<Vec<_>>(), vec![true, false, false], "first-sample-flags vs trex");
    assert_eq!(v[2].data, vec![8, 9, 10, 11]);
    assert_eq!(a.len(), 4);
    assert_eq!(a.iter().map(|x| x.dts).collect::<Vec<_>>(), vec![0, 20, 40, 60]);
    assert_eq!(a[0].data, vec![12, 13], "second traf continues after the first's data");
    assert_eq!(a[3].data, vec![18, 19]);
}

/// A progressive file by hand: `stz2` 8-bit sizes, `ctts` version 0, two `stsc` runs, `stss`.
#[test]
fn kat_literal_progressive_mp4_stz2() {
    let entry = b(b"avc1", &[vec![0; 6], vec![0, 1], vec![0; 16], vec![0, 4, 0, 4], vec![0; 50], b(b"avcC", &[1, 0x42, 0, 0x1E])].concat());
    let stsd = fb(b"stsd", 0, 0, &[be(&[1]), entry].concat());
    let stts = fb(b"stts", 0, 0, &be(&[1, 5, 10]));
    let ctts = fb(b"ctts", 0, 0, &be(&[2, 1, 20, 4, 0]));
    let stsc = fb(b"stsc", 0, 0, &be(&[2, 1, 2, 1, 2, 3, 1]));
    let stz2 = fb(b"stz2", 0, 0, &[vec![0, 0, 0, 8], be(&[5]), vec![1, 2, 3, 4, 5]].concat());
    let stss = fb(b"stss", 0, 0, &be(&[2, 1, 4]));
    let mk = |off: u32| {
        let stco = fb(b"stco", 0, 0, &be(&[2, off, off + 3]));
        let stbl = b(b"stbl", &[stsd.clone(), stts.clone(), ctts.clone(), stsc.clone(), stz2.clone(), stco, stss.clone()].concat());
        let mdhd = fb(b"mdhd", 0, 0, &be(&[0, 0, 100, 50, 0x55C4_0000]));
        let hd = fb(b"hdlr", 0, 0, &[be(&[0]), b"vide".to_vec(), vec![0; 13]].concat());
        let tkhd = fb(b"tkhd", 0, 3, &[be(&[0, 0, 1]), vec![0; 68]].concat());
        let trak = b(b"trak", &[tkhd, b(b"mdia", &[mdhd, hd, b(b"minf", &stbl)].concat())].concat());
        let mvhd = fb(b"mvhd", 0, 0, &[be(&[0, 0, 100, 50]), vec![0; 80]].concat());
        b(b"moov", &[mvhd, trak].concat())
    };
    let ftyp = b(b"ftyp", b"isom\0\0\0\0");
    let moov_len = mk(0).len();
    let off = (ftyp.len() + moov_len + 8) as u32;
    let mdat = b(b"mdat", &(100u8..115).collect::<Vec<_>>());
    let file = [ftyp, mk(off), mdat].concat();
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.tracks()[0].codec, Codec::Avc);
    assert_eq!(d.duration_ns(), 500_000_000);
    let p = all(&mut d);
    assert_eq!(p.iter().map(|x| x.data.len()).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
    assert_eq!(p[0].data, vec![100]);
    assert_eq!(p[1].data, vec![101, 102]);
    assert_eq!(p[2].data, vec![103, 104, 105], "second chunk starts at stco[1]");
    assert_eq!(p[4].data, vec![110, 111, 112, 113, 114]);
    assert_eq!(p.iter().map(|x| x.pts).collect::<Vec<_>>(), vec![20, 10, 20, 30, 40]);
    assert_eq!(p.iter().map(|x| x.keyframe).collect::<Vec<_>>(), vec![true, false, false, true, false]);
}

/// A WebM by hand: unknown-size Segment, a Cluster with an 8-byte size field, a Xiph-laced
/// SimpleBlock and EBML-laced SimpleBlock, header stripping on the video track, and a 4-byte
/// float Duration.
#[test]
fn kat_literal_webm_lacing_header_stripping() {
    let e = |id: &[u8], body: &[u8]| {
        let mut v = id.to_vec();
        assert!(body.len() < 127);
        v.push(0x80 | body.len() as u8);
        v.extend_from_slice(body);
        v
    };
    let ebml = e(&[0x1A, 0x45, 0xDF, 0xA3], &e(&[0x42, 0x82], b"webm"));
    let info = e(&[0x15, 0x49, 0xA9, 0x66], &[e(&[0x2A, 0xD7, 0xB1], &[0x0F, 0x42, 0x40]), e(&[0x44, 0x89], &100.0f32.to_bits().to_be_bytes())].concat());
    let strip = e(&[0x6D, 0x80], &e(&[0x62, 0x40], &e(&[0x50, 0x34], &[e(&[0x42, 0x54], &[3]), e(&[0x42, 0x55], &[0xAA, 0xBB])].concat())));
    let vtrack = e(&[0xAE], &[e(&[0xD7], &[1]), e(&[0x83], &[1]), e(&[0x86], b"V_VP9"), e(&[0xE0], &[e(&[0xB0], &[0x01, 0x40]), e(&[0xBA], &[0xF0])].concat()), strip].concat());
    let atrack = e(&[0xAE], &[e(&[0xD7], &[2]), e(&[0x83], &[2]), e(&[0x86], b"A_VORBIS"), e(&[0x23, 0xE3, 0x83], &[0x01, 0x31, 0x2D, 0x00]), e(&[0xE1], &[e(&[0xB5], &44100.0f64.to_bits().to_be_bytes()), e(&[0x9F], &[1])].concat())].concat());
    let tracks = e(&[0x16, 0x54, 0xAE, 0x6B], &[vtrack, atrack].concat());
    // Video SimpleBlock track 1, rel +0, key; payload 0x01 0x02 (prefix AA BB prepended).
    let sb_v = e(&[0xA3], &[0x81, 0x00, 0x00, 0x80, 0x01, 0x02]);
    // Audio Xiph lace: 3 frames sizes 2,1,rest(3): header count-1 = 2, sizes 2,1.
    let sb_a = e(&[0xA3], &[0x82, 0x00, 0x05, 0x82, 0x02, 0x02, 0x01, 9, 9, 8, 7, 7, 7]);
    // Audio EBML lace at rel +80: 3 frames sizes 2, 4 (diff +2: 0x80|(63+2)=0xC1), rest 1.
    let sb_e = e(&[0xA3], &[0x82, 0x00, 0x50, 0x86, 0x02, 0x82, 0xC1, 1, 1, 2, 2, 2, 2, 3]);
    let cluster_body = [e(&[0xE7], &[10]), sb_v, sb_a, sb_e].concat();
    let mut cluster = vec![0x1F, 0x43, 0xB6, 0x75, 0x01];
    cluster.extend_from_slice(&(cluster_body.len() as u64).to_be_bytes()[1..]);
    cluster.extend_from_slice(&cluster_body);
    let mut seg = vec![0x18, 0x53, 0x80, 0x67, 0xFF];
    seg.extend_from_slice(&[info, tracks, cluster].concat());
    let file = [ebml, seg].concat();

    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.format(), Format::WebM);
    assert_eq!(d.duration_ns(), 100_000_000);
    assert_eq!((d.tracks()[0].width, d.tracks()[0].height), (320, 240));
    assert_eq!(d.tracks()[1].codec, Codec::Vorbis);
    assert_eq!(d.tracks()[1].sample_rate, 44100);
    let p = all(&mut d);
    let v: Vec<&Packet> = p.iter().filter(|x| x.track == 1).collect();
    assert_eq!(v[0].data, vec![0xAA, 0xBB, 0x01, 0x02], "header stripping restored");
    assert_eq!(v[0].pts, 10_000_000);
    let a: Vec<&Packet> = p.iter().filter(|x| x.track == 2).collect();
    assert_eq!(a.iter().map(|x| x.data.clone()).collect::<Vec<_>>(), vec![vec![9, 9], vec![8], vec![7, 7, 7], vec![1, 1], vec![2, 2, 2, 2], vec![3]]);
    // DefaultDuration 20 ms (0x01312D00 ns) spaces laced frames.
    assert_eq!(a.iter().map(|x| x.pts / 1_000_000).collect::<Vec<_>>(), vec![15, 35, 55, 90, 110, 130]);
}

// ----------------------------------------------------------------- refusals

#[test]
fn refusals_are_errors_not_panics() {
    assert_eq!(Demuxer::open(b"hello world, not media".to_vec()).err(), Some(Error::UnknownFormat));
    let good = build::mp4(&[build::test_pattern_track(1, 8, 8, 10, 10, 5)], &Mp4Options::default());
    // Every truncation of a valid file is an error or a shorter valid table, never a panic.
    for cut in 0..good.len() {
        let _ = Demuxer::open(good[..cut].to_vec());
    }
    let web = build::mkv(&[build::test_pattern_track(1, 8, 8, 10, 10, 5)], &MkvOptions::default());
    for cut in 0..web.len() {
        let _ = Demuxer::open(web[..cut].to_vec());
    }
    // Byte flips: still no panic.
    for i in 0..good.len() {
        let mut x = good.clone();
        x[i] ^= 0xFF;
        let _ = Demuxer::open(x);
    }
    for i in 0..web.len() {
        let mut x = web.clone();
        x[i] ^= 0xFF;
        let _ = Demuxer::open(x);
    }
}

// ----------------------------------------------------------------- PCM

#[test]
fn pcm_tracks_in_both_containers() {
    let pcm: Vec<i16> = (0..4800).map(|i| ((i * 37) % 65536 - 32768) as i16).collect();
    let t = build::pcm16_track(2, 48_000, 2, &pcm, 480);
    for file in [build::mp4(&[t.clone()], &Mp4Options::default()), build::mkv(&[t.clone()], &MkvOptions::default())] {
        let mut d = Demuxer::open(file).unwrap();
        let tr = d.tracks()[0].clone();
        assert_eq!(tr.codec, Codec::Pcm { bits: 16, float: false, big_endian: false }, "{}", tr.codec_name);
        assert_eq!((tr.sample_rate, tr.channels), (48_000, 2));
        let p = all(&mut d);
        assert_eq!(p.len(), 5);
        assert_eq!(p.iter().map(|x| tr.to_ns(x.pts)).collect::<Vec<_>>(), vec![0, 10_000_000, 20_000_000, 30_000_000, 40_000_000]);
        let bytes: Vec<u8> = p.iter().flat_map(|x| x.data.clone()).collect();
        assert_eq!(bytes, pcm.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>());
    }
    // 24-bit big-endian in Matroska and `twos` in MP4 map their depth and byte order.
    let mut t24 = t.clone();
    t24.spec.codec_id = "A_PCM/INT/BIG";
    t24.spec.bit_depth = 24;
    let d = Demuxer::open(build::mkv(&[t24.clone()], &MkvOptions::default())).unwrap();
    assert_eq!(d.tracks()[0].codec, Codec::Pcm { bits: 24, float: false, big_endian: true });
    t24.spec.fourcc = *b"twos";
    let d = Demuxer::open(build::mp4(&[t24], &Mp4Options::default())).unwrap();
    assert_eq!(d.tracks()[0].codec, Codec::Pcm { bits: 24, float: false, big_endian: true });
}
