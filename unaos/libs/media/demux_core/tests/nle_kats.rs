// SPDX-License-Identifier: LGPL-3.0-or-later
//! NLEPROBE (LEDGER SR195): the editor's probe (`demux_core::survey`) and its two seek reads
//! (`Demuxer::keyframe_before`, `Demuxer::samples_from`) over synthetic files the tree's own writers make
//! (`build::mp4` writes the stsd/stts/stss/ctts, `build::mkv` the WebM) — no committed media.

use demux_core::build::{self, MediaTrack, MkvOptions, Mp4Options, SampleSpec, TestPatternPacket, TrackSpec};
use demux_core::survey::survey_of;
use demux_core::{Codec, Demuxer, Error, Format, Timebase, TrackKind};

/// 20 ms Opus-like frames at a 48 kHz timescale, every one a sync sample.
fn opus_track(id: u32, n: usize) -> MediaTrack {
    MediaTrack {
        spec: TrackSpec {
            id,
            kind: TrackKind::Audio,
            fourcc: *b"Opus",
            config_box: Some(*b"dOps"),
            codec_id: "A_OPUS",
            config: vec![0, 2, 0x01, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 0],
            timescale: 48_000,
            width: 0,
            height: 0,
            sample_rate: 48_000,
            channels: 2,
            bit_depth: 0,
            default_duration_ns: 20_000_000,
        },
        samples: (0..n)
            .map(|i| SampleSpec { data: vec![i as u8; 4], dts: i as i64 * 960, pts: i as i64 * 960, duration: 960, keyframe: true })
            .collect(),
    }
}

/// Decode order I P B B P B B I P B B at 24 fps (timescale 24000, 1000 ticks a frame); presentation order in
/// frame units below, pts = (pres + 1) x 1000 (the ctts the writer emits; no edit list), sync samples 0 and 7.
const PRES: [i64; 11] = [0, 3, 1, 2, 6, 4, 5, 7, 10, 8, 9];

fn bframe_video() -> MediaTrack {
    let spec = TrackSpec { fourcc: *b"av01", config_box: Some(*b"av1C"), codec_id: "V_AV1", config: vec![0x81, 0, 0, 0], ..TrackSpec::test_pattern(1, 64, 48, 24) };
    let samples = PRES
        .iter()
        .enumerate()
        .map(|(i, &p)| SampleSpec { data: vec![i as u8; 3 + i], dts: i as i64 * 1000, pts: (p + 1) * 1000, duration: 1000, keyframe: i == 0 || i == 7 })
        .collect();
    MediaTrack { spec, samples }
}

fn mp4_file(fragment: usize) -> Vec<u8> {
    build::mp4(&[bframe_video(), opus_track(2, 25)], &Mp4Options { fragment, ..Default::default() })
}

#[test]
fn nle_survey_mp4_stsd_stts_stss_ctts() {
    let file = mp4_file(0);
    let d = Demuxer::open(file.clone()).unwrap();
    let s = d.survey();
    assert_eq!(s, survey_of(&file).unwrap(), "survey_of and Demuxer::survey are one read");
    assert_eq!(s.format, Format::Mp4);
    assert_eq!(s.streams.len(), 2);

    let v = &s.streams[0];
    assert_eq!((v.track_index, v.id, v.kind), (0, 1, TrackKind::Video));
    assert_eq!(v.codec, Codec::Av1);
    assert_eq!(v.codec_name, "av01");
    assert_eq!(v.timebase, Timebase { num: 1, den: 24_000 });
    assert_eq!((v.width, v.height), (64, 48));
    assert_eq!(v.frame_count, 11);
    assert_eq!(v.first_pts, 1000);
    assert_eq!(v.keyframes, vec![1000, 8000]);
    let offs: Vec<i64> = PRES.iter().enumerate().map(|(i, &p)| (p + 1 - i as i64) * 1000).collect();
    assert_eq!(offs, vec![1000, 3000, 0, 0, 3000, 0, 0, 1000, 3000, 0, 0]);
    assert_eq!(v.composition_offsets, offs);
    assert!(v.reordered);
    // span: max(pts + duration) - min(pts) = 12000 - 1000 ticks
    assert_eq!(v.duration_ns, v.timebase.to_ns(11_000) as u64);

    let a = &s.streams[1];
    assert_eq!((a.track_index, a.id, a.kind), (1, 2, TrackKind::Audio));
    assert_eq!(a.codec, Codec::Opus);
    assert_eq!(a.timebase, Timebase { num: 1, den: 48_000 });
    assert_eq!((a.sample_rate, a.channels), (48_000, 2));
    assert_eq!(a.frame_count, 25);
    assert_eq!(a.keyframes.len(), 25, "every audio unit is a sync sample");
    assert_eq!(a.keyframes[1] - a.keyframes[0], 960);
    assert!(a.composition_offsets.iter().all(|&o| o == 0));
    assert!(!a.reordered);
    assert_eq!(a.duration_ns, 500_000_000);
    assert_eq!(s.duration_ns, d.duration_ns());
}

#[test]
fn nle_survey_fragmented_mp4_matches_progressive() {
    let p = survey_of(&mp4_file(0)).unwrap();
    let f = survey_of(&mp4_file(4)).unwrap();
    assert_eq!(f.streams.len(), 2);
    for (a, b) in p.streams.iter().zip(&f.streams) {
        assert_eq!(a.keyframes, b.keyframes, "trun sync flags give the stss index");
        assert_eq!(a.composition_offsets, b.composition_offsets, "trun cto gives the ctts offsets");
        assert_eq!((a.frame_count, a.reordered, a.first_pts), (b.frame_count, b.reordered, b.first_pts));
    }
}

#[test]
fn nle_mp4_keyframe_before_and_samples_from() {
    let d = Demuxer::open(mp4_file(0)).unwrap();
    let tb = d.tracks()[0].timebase;
    let (k0, k1) = (tb.to_ns(1000), tb.to_ns(8000));
    let key = |t: i64| {
        let (i, p) = d.keyframe_before(0, t).expect("a keyframe");
        let s = d.samples()[i];
        assert!(s.keyframe && s.track_index == 0);
        assert_eq!(tb.to_ns(s.pts), p);
        p
    };
    assert_eq!(key(0), k0, "before every keyframe: the first");
    assert_eq!(key(k0), k0);
    assert_eq!(key(200_000_000), k0, "between the keys");
    assert_eq!(key(k1 - 1), k0);
    assert_eq!(key(k1), k1, "exactly on a key");
    assert_eq!(key(10_000_000_000), k1, "past the end: the last");
    assert_eq!(d.keyframe_before(9, 0), None, "no such track");

    // From pts 5000: the GOP from I(0) in decode order; the frames presented before 5000 are preroll —
    // the P presented at 4000 and both Bs (2000, 3000) included.
    let r: Vec<_> = d.samples_from(0, tb.to_ns(5000)).collect();
    assert!(r.iter().all(|x| x.sample.track_index == 0), "one track only");
    assert_eq!(r.iter().map(|x| x.sample.dts).collect::<Vec<_>>(), (0..11).map(|i| i * 1000).collect::<Vec<_>>());
    assert_eq!(r.iter().map(|x| x.preroll).collect::<Vec<_>>(), vec![true, true, true, true, false, false, false, false, false, false, false]);
    assert!(r[0].sample.keyframe);
    assert_eq!(d.packet_at(&r[0].sample).data, vec![0u8; 3]);

    // From pts 9000: the second GOP; its I (8000) is preroll, the P (11000) and Bs (9000, 10000) are shown.
    let r: Vec<_> = d.samples_from(0, tb.to_ns(9000)).collect();
    assert_eq!(r.iter().map(|x| x.sample.pts).collect::<Vec<_>>(), vec![8000, 11_000, 9000, 10_000]);
    assert_eq!(r.iter().map(|x| x.preroll).collect::<Vec<_>>(), vec![true, false, false, false]);
    assert_eq!(d.sample_data(&r[0].sample), &[7u8; 10][..]);

    // An audio read: every unit is a sync sample, so it starts at the unit at or before t and nothing is preroll.
    let r: Vec<_> = d.samples_from(1, 100_000_000).collect();
    assert_eq!(r.len(), 20);
    assert_eq!(r[0].sample.pts, 4800);
    assert!(r.iter().all(|x| !x.preroll && x.sample.track_index == 1));

    assert_eq!(d.position(), 0, "the editor's reads never move the playback cursor");
}

fn webm_file(block_groups: bool) -> Vec<u8> {
    let v = build::test_pattern_track(1, 32, 32, 10, 30, 8); // keys at frames 0, 8, 16, 24; 100 ms a frame
    build::mkv(&[v, opus_track(2, 150)], &MkvOptions { block_groups, ..Default::default() })
}

#[test]
fn nle_survey_webm_simpleblocks_and_blockgroups() {
    for bg in [false, true] {
        let file = webm_file(bg);
        let s = survey_of(&file).unwrap();
        assert_eq!(s.format, Format::WebM);
        assert_eq!(s.streams.len(), 2);
        let v = &s.streams[0];
        assert_eq!((v.id, v.kind, v.codec.clone()), (1, TrackKind::Video, Codec::TestPattern), "block_groups={bg}");
        assert_eq!(v.codec_name, "V_UNAOS/TESTPATTERN");
        assert_eq!(v.timebase, Timebase::NANOS);
        assert_eq!(v.frame_count, 30);
        assert_eq!(v.first_pts, 0);
        assert_eq!(v.keyframes, vec![0, 800_000_000, 1_600_000_000, 2_400_000_000], "block_groups={bg}");
        assert!(v.composition_offsets.iter().all(|&o| o == 0), "Matroska stores no decode time");
        assert_eq!(v.composition_offsets.len(), 30);
        assert!(!v.reordered);
        assert_eq!(v.duration_ns, 3_000_000_000);
        let a = &s.streams[1];
        assert_eq!((a.kind, a.codec.clone(), a.frame_count), (TrackKind::Audio, Codec::Opus, 150));
        assert_eq!(a.keyframes.len(), 150);
        assert!(!a.reordered);
    }
    assert_eq!(survey_of(b"not a container at all").unwrap_err(), Error::UnknownFormat);
}

#[test]
fn nle_webm_keyframe_before_and_samples_from() {
    for bg in [false, true] {
        let d = Demuxer::open(webm_file(bg)).unwrap();
        assert_eq!(d.keyframe_before(0, 1_950_000_000).map(|(_, p)| p), Some(1_600_000_000), "block_groups={bg}");
        assert_eq!(d.keyframe_before(0, -5).map(|(_, p)| p), Some(0));
        assert_eq!(d.keyframe_before(0, 99_000_000_000).map(|(_, p)| p), Some(2_400_000_000));
        let r: Vec<_> = d.samples_from(0, 1_950_000_000).collect();
        assert_eq!(r.len(), 14, "frames 16..29");
        assert!(r.iter().all(|x| x.sample.track_index == 0));
        let frames: Vec<u32> = r.iter().map(|x| TestPatternPacket::decode(&d.packet_at(&x.sample).data).unwrap().frame).collect();
        assert_eq!(frames, (16..30).collect::<Vec<_>>());
        assert_eq!(r.iter().filter(|x| x.preroll).count(), 4, "frames 16..19 are presented before 1.95 s");
        assert!(r[0].sample.keyframe && r[0].preroll && !r[4].preroll);
        assert_eq!(d.keyframe_before(1, 1_950_000_000).map(|(_, p)| p), Some(1_940_000_000), "the audio unit at or before");
        assert_eq!(d.position(), 0);
    }
}

#[test]
fn nle_reads_agree_with_seek_track() {
    // key_at_or_before is the ONE search: the playback seek lands where the editor's read starts.
    let mut d = Demuxer::open(mp4_file(0)).unwrap();
    for t in [0i64, 100_000_000, 333_333_333, 400_000_000, 5_000_000_000] {
        let (_, p) = d.keyframe_before(0, t).unwrap();
        assert_eq!(d.seek_track(0, t), Some(p));
        let first = d.samples_from(0, t).next().unwrap().sample;
        let pk = loop {
            let pk = d.next_packet().unwrap();
            if pk.track == 1 {
                break pk;
            }
        };
        assert_eq!((pk.pts, pk.dts, pk.keyframe), (first.pts, first.dts, true));
    }
}
