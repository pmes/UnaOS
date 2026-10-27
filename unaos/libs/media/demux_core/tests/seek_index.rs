// SPDX-License-Identifier: LGPL-3.0-or-later
//! DEMUXSEEK (LEDGER SR200): Matroska decode times derived from the codec's reorder depth (`Demuxer::derive_dts` with
//! `h264_core::ps::avcc_reorder_depth` / `av1_core::image::av1c_reorder_depth`), the open by the file's own index
//! (`Demuxer::open_indexed`: Matroska Cues, MP4 `sidx` / `mfra`), every `elst` entry (`Demuxer::edit_list`), `sdtp` and the
//! trun dependency bits (`Sample::depends`), and the LEADSKIP rule (`SampleRead::skip`) — over files the tree's own writers
//! make (`build::mkv` with `cues`, `build::mp4_with`). No committed media.

use std::time::Instant;

use demux_core::build::{self, MediaTrack, MkvOptions, Mp4Extras, Mp4Options, SampleSpec, TrackSpec};
use demux_core::survey::survey_of;
use demux_core::{Codec, DecodeTime, Demuxer, EditSegment, Packet, TrackKind};

// ----- bit writer for the synthetic SPS and AV1 sequence header -----

struct Bits {
    out: Vec<u8>,
    cur: u8,
    n: u32,
}

impl Bits {
    fn new() -> Bits {
        Bits { out: Vec::new(), cur: 0, n: 0 }
    }
    fn bit(&mut self, b: u32) {
        self.cur = (self.cur << 1) | (b & 1) as u8;
        self.n += 1;
        if self.n == 8 {
            self.out.push(self.cur);
            self.cur = 0;
            self.n = 0;
        }
    }
    fn u(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1);
        }
    }
    fn ue(&mut self, v: u32) {
        let x = v + 1;
        let len = 32 - x.leading_zeros();
        self.u(0, len - 1);
        self.u(x, len);
    }
    /// rbsp_trailing_bits.
    fn trailing(mut self) -> Vec<u8> {
        self.bit(1);
        while self.n != 0 {
            self.bit(0);
        }
        self.out
    }
    fn padded(mut self, bytes: usize) -> Vec<u8> {
        while self.n != 0 {
            self.bit(0);
        }
        self.out.resize(bytes.max(self.out.len()), 0);
        self.out
    }
}

/// A Main-profile SPS RBSP (4x3 macroblocks, level 3.0, two reference frames). `reorder`: VUI bitstream_restriction with
/// max_num_reorder_frames / max_dec_frame_buffering; `poc_type` 0 or 2.
fn sps_rbsp(reorder: Option<(u32, u32)>, poc_type: u32) -> Vec<u8> {
    let mut b = Bits::new();
    b.u(77, 8); // profile_idc: Main
    b.u(0, 8); // constraint_set flags
    b.u(30, 8); // level_idc
    b.ue(0); // seq_parameter_set_id
    b.ue(0); // log2_max_frame_num_minus4
    b.ue(poc_type);
    if poc_type == 0 {
        b.ue(2); // log2_max_pic_order_cnt_lsb_minus4
    }
    b.ue(2); // max_num_ref_frames
    b.bit(0); // gaps_in_frame_num_value_allowed_flag
    b.ue(3); // pic_width_in_mbs_minus1
    b.ue(2); // pic_height_in_map_units_minus1
    b.bit(1); // frame_mbs_only_flag
    b.bit(1); // direct_8x8_inference_flag
    b.bit(0); // frame_cropping_flag
    match reorder {
        None => b.bit(0),
        Some((reorder, dpb)) => {
            b.bit(1); // vui_parameters_present_flag
            for _ in 0..4 {
                b.bit(0); // aspect_ratio / overscan / video_signal_type / chroma_loc info absent
            }
            b.bit(0); // timing_info_present_flag
            b.bit(0); // nal_hrd
            b.bit(0); // vcl_hrd
            b.bit(0); // pic_struct_present_flag
            b.bit(1); // bitstream_restriction_flag
            b.bit(1); // motion_vectors_over_pic_boundaries_flag
            b.ue(0); // max_bytes_per_pic_denom
            b.ue(0); // max_bits_per_mb_denom
            b.ue(16); // log2_max_mv_length_horizontal
            b.ue(16); // log2_max_mv_length_vertical
            b.ue(reorder);
            b.ue(dpb);
        }
    }
    b.trailing()
}

/// An AVCDecoderConfigurationRecord carrying that SPS and a stand-in PPS.
fn avcc(sps: &[u8]) -> Vec<u8> {
    let mut nal = vec![0x67];
    nal.extend_from_slice(sps);
    let pps = [0x68u8, 0xCE, 0x38, 0x80];
    let mut c = vec![1, 77, 0, 30, 0xFF, 0xE1];
    c.extend_from_slice(&(nal.len() as u16).to_be_bytes());
    c.extend_from_slice(&nal);
    c.push(1);
    c.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    c.extend_from_slice(&pps);
    c
}

/// An av1C whose configOBUs hold a reduced still-picture sequence header (profile 0, 64x48, 8-bit 4:2:0).
fn av1c_with_sequence_header() -> Vec<u8> {
    let mut b = Bits::new();
    b.u(0, 3); // seq_profile
    b.bit(1); // still_picture
    b.bit(1); // reduced_still_picture_header
    b.u(0, 5); // seq_level_idx[0]
    b.u(7, 4); // frame_width_bits_minus_1
    b.u(7, 4); // frame_height_bits_minus_1
    b.u(63, 8); // max_frame_width_minus_1
    b.u(47, 8); // max_frame_height_minus_1
    b.bit(0); // use_128x128_superblock
    b.bit(0); // enable_filter_intra
    b.bit(0); // enable_intra_edge_filter
    b.bit(0); // enable_superres
    b.bit(0); // enable_cdef
    b.bit(0); // enable_restoration
    b.bit(0); // high_bitdepth
    b.bit(0); // mono_chrome
    b.bit(0); // color_description_present_flag
    b.bit(0); // color_range
    b.u(0, 2); // chroma_sample_position
    b.bit(0); // separate_uv_delta_q
    b.bit(0); // film_grain_params_present
    let payload = b.padded(8);
    let mut c = vec![0x81, 0x00, 0x0C, 0x00];
    c.push((1 << 3) | 0x02); // OBU_SEQUENCE_HEADER, obu_has_size_field
    c.push(payload.len() as u8);
    c.extend_from_slice(&payload);
    c
}

// ----- media -----

/// 20 ms Opus-like units at 48 kHz, every one a sync sample.
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

/// NLEPROBE's decode order I P B B P B B I P B B; presentation order in frame units.
const PRES: [i64; 11] = [0, 3, 1, 2, 6, 4, 5, 7, 10, 8, 9];

/// An AVC track at 25 fps in a 1 ms timescale (pts = (pres + 1) x 40), no DefaultDuration: the durations come from the fill.
fn avc_bframes(config: Vec<u8>) -> MediaTrack {
    let spec = TrackSpec {
        id: 1,
        kind: TrackKind::Video,
        fourcc: *b"avc1",
        config_box: Some(*b"avcC"),
        codec_id: "V_MPEG4/ISO/AVC",
        config,
        timescale: 1000,
        width: 64,
        height: 48,
        sample_rate: 0,
        channels: 0,
        bit_depth: 0,
        default_duration_ns: 0,
    };
    let samples = PRES
        .iter()
        .enumerate()
        .map(|(i, &p)| SampleSpec { data: vec![i as u8; 5 + i], dts: i as i64 * 40, pts: (p + 1) * 40, duration: 40, keyframe: i == 0 || i == 7 })
        .collect();
    MediaTrack { spec, samples }
}

const MS: i64 = 1_000_000;

fn avc_depth(t: &demux_core::Track) -> Option<u32> {
    match t.codec {
        Codec::Avc => h264_core::ps::avcc_reorder_depth(&t.config),
        Codec::Av1 => av1_core::image::av1c_reorder_depth(&t.config),
        _ => None,
    }
}

// ----- M1 MKVDTS -----

#[test]
fn mkvdts_reorder_depth_accessors() {
    assert_eq!(h264_core::ps::avcc_reorder_depth(&avcc(&sps_rbsp(Some((2, 3)), 0))), Some(2), "VUI max_num_reorder_frames");
    // No VUI: E.2.1 infers MaxDpbFrames — level 3.0's 8100 MaxDpbMbs over 12 macroblocks, capped at 16.
    assert_eq!(h264_core::ps::avcc_reorder_depth(&avcc(&sps_rbsp(None, 0))), Some(16));
    assert_eq!(h264_core::ps::avcc_reorder_depth(&avcc(&sps_rbsp(None, 2))), Some(0), "poc type 2: output order is decode order");
    assert_eq!(h264_core::ps::avcc_reorder_depth(&[1, 2, 3]), None);
    assert_eq!(av1_core::image::av1c_reorder_depth(&av1c_with_sequence_header()), Some(0));
    assert_eq!(av1_core::image::av1c_reorder_depth(&[0x81, 0, 0, 0]), None, "an av1C without configOBUs");
}

#[test]
fn mkvdts_presentation_order_durations() {
    let file = build::mkv(&[avc_bframes(avcc(&sps_rbsp(Some((2, 3)), 0)))], &MkvOptions { webm: false, ..Default::default() });
    let d = Demuxer::open(file).unwrap();
    let v: Vec<_> = d.track_samples(0).copied().collect();
    assert_eq!(v.len(), 11);
    // Every frame lasts one 40 ms step (the decode-order gap gave 120 ms to the I and 0 to the Bs).
    assert!(v.iter().all(|s| s.duration == 40 * MS as u64), "{:?}", v.iter().map(|s| s.duration).collect::<Vec<_>>());
    assert_eq!(d.tracks()[0].duration_ns, 440 * MS as u64, "span: 40 .. 480 ms");
    let s = d.survey();
    assert_eq!(s.streams[0].decode_time, DecodeTime::Presentation, "no depth given: dts = pts, and the survey says so");
    assert!(s.streams[0].composition_offsets.iter().all(|&o| o == 0));
    assert!(s.streams[0].reordered, "pts not monotone in decode order");
}

#[test]
fn mkvdts_derived_from_the_sps() {
    let file = build::mkv(&[avc_bframes(avcc(&sps_rbsp(Some((2, 3)), 0))), opus_track(2, 25)], &MkvOptions::default());
    let mut d = Demuxer::open(file).unwrap();
    assert_eq!(d.derive_dts(avc_depth), 1, "the video track only (audio is not reordered)");
    let s = d.survey();
    assert_eq!(s.streams[0].decode_time, DecodeTime::Derived(2));
    assert_eq!(s.streams[1].decode_time, DecodeTime::Presentation);
    let v: Vec<_> = d.track_samples(0).copied().collect();
    let dts: Vec<i64> = v.iter().map(|s| s.dts / MS).collect();
    assert_eq!(dts, vec![-40, 0, 40, 80, 120, 160, 200, 240, 280, 320, 360], "dts = sorted pts two frames back");
    assert!(v.iter().all(|s| s.dts <= s.pts));
    assert_eq!(s.streams[0].composition_offsets, [80, 160, 40, 40, 160, 40, 40, 80, 160, 40, 40].map(|o| o * MS).to_vec());
    // The merge follows the derived decode times: every track's dts in ns is non-decreasing over the table.
    let ns: Vec<i64> = d.samples().iter().map(|s| d.tracks()[s.track_index].to_ns(s.dts)).collect();
    assert!(ns.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(d.position(), 0);
    assert_eq!(d.derive_dts(avc_depth), 0, "a derived track is not derived twice");
}

#[test]
fn mkvdts_refuses_a_depth_the_samples_contradict() {
    let file = build::mkv(&[avc_bframes(avcc(&sps_rbsp(Some((0, 1)), 0)))], &MkvOptions::default());
    let mut d = Demuxer::open(file).unwrap();
    let before: Vec<_> = d.samples().to_vec();
    assert_eq!(d.derive_dts(avc_depth), 0, "depth 0 would put a dts past its pts");
    assert_eq!(d.samples(), &before[..]);
    assert_eq!(d.survey().streams[0].decode_time, DecodeTime::Presentation);
    // MP4 stores its decode times: nothing to derive.
    let mp4 = build::mp4(&[avc_bframes(avcc(&sps_rbsp(Some((2, 3)), 0)))], &Mp4Options::default());
    let mut m = Demuxer::open(mp4).unwrap();
    assert_eq!(m.derive_dts(avc_depth), 0);
    assert_eq!(m.survey().streams[0].decode_time, DecodeTime::Stored);
}

// ----- M2 CUES -----

fn key(p: &Packet) -> (u32, i64, i64, bool, u64, Vec<u8>) {
    (p.track, p.pts, p.dts, p.keyframe, p.duration, p.data.clone())
}

fn drain(d: &mut Demuxer) -> Vec<Packet> {
    let mut v = Vec::new();
    while let Some(p) = d.next_packet() {
        v.push(p);
    }
    v
}

fn cues_webm() -> Vec<u8> {
    let v = build::test_pattern_track(1, 32, 32, 10, 60, 10); // keys at 0, 1, .. 5 s
    build::mkv(&[v, opus_track(2, 300)], &MkvOptions { cues: true, cluster_blocks: 7, ..Default::default() })
}

#[test]
fn cues_open_reads_the_index_only() {
    let file = cues_webm();
    let mut full = Demuxer::open(file.clone()).unwrap();
    let mut idx = Demuxer::open_indexed(file.clone()).unwrap();
    assert!(idx.indexed() && !full.indexed());
    assert!(idx.samples().is_empty(), "no Cluster was walked");
    assert_eq!(idx.index().iter().map(|x| (x.track_index, x.pts_ns)).collect::<Vec<_>>(), (0..6).map(|k| (0, k * 1000 * MS)).collect::<Vec<_>>());
    let s = idx.survey();
    assert!(s.streams[0].indexed);
    assert_eq!(s.streams[0].keyframes, (0..6).map(|k| k * 1000 * MS).collect::<Vec<_>>(), "the keyframe index is the Cues");
    assert_eq!(s.duration_ns, full.duration_ns());
    // Read through, span after span: the very packets of the full open, in the same order.
    let a: Vec<_> = drain(&mut full).iter().map(key).collect();
    let b: Vec<_> = drain(&mut idx).iter().map(key).collect();
    assert_eq!(a.len(), 360);
    assert_eq!(a, b);
}

#[test]
fn cues_seek_jumps_to_the_cluster() {
    let file = cues_webm();
    let mut full = Demuxer::open(file.clone()).unwrap();
    let mut idx = Demuxer::open_indexed(file).unwrap();
    for t in [0i64, 2_500 * MS, 3_000 * MS, 5_900 * MS, 99_000 * MS] {
        let a = full.seek_track(0, t);
        let b = idx.seek_track(0, t);
        assert_eq!(a, b, "t={t}");
        let pa: Vec<_> = (0..20).filter_map(|_| full.next_packet()).map(|p| key(&p)).collect();
        let pb: Vec<_> = (0..20).filter_map(|_| idx.next_packet()).map(|p| key(&p)).collect();
        assert_eq!(pa, pb, "t={t}");
    }
    // The editor's reads over the loaded window agree with the full table.
    let mut ed = Demuxer::open_indexed(cues_webm()).unwrap();
    ed.load_at(0, 3_500 * MS).unwrap();
    let full = Demuxer::open(cues_webm()).unwrap();
    assert_eq!(ed.keyframe_before(0, 3_500 * MS).map(|x| x.1), Some(3_000 * MS));
    let r: Vec<_> = ed.samples_from(0, 3_500 * MS).map(|x| (x.sample.pts, x.preroll, x.skip)).collect();
    let f: Vec<_> = full.samples_from(0, 3_500 * MS).map(|x| (x.sample.pts, x.preroll, x.skip)).collect();
    assert!(!r.is_empty() && f.starts_with(&r), "the window's read is a prefix of the full read");
}

#[test]
fn cues_open_time_on_ten_thousand_clusters() {
    // 10,000 Clusters of ten frames (100 fps, a key opening each Cluster): one CuePoint per Cluster.
    let v = build::test_pattern_track(1, 16, 16, 100, 100_000, 10);
    let file = build::mkv(&[v], &MkvOptions { cues: true, cluster_blocks: 10, ..Default::default() });
    let t0 = Instant::now();
    let full = Demuxer::open(file.clone()).unwrap();
    let t_full = t0.elapsed();
    let t1 = Instant::now();
    let mut idx = Demuxer::open_indexed(file).unwrap();
    let t_idx = t1.elapsed();
    eprintln!("[demuxseek] open 10000 clusters: full={t_full:?} indexed={t_idx:?} file_samples={} bytes={}", full.samples().len(), full.bytes().len());
    assert_eq!(full.samples().len(), 100_000);
    assert!(idx.samples().is_empty(), "the index open walks no Cluster");
    assert_eq!(idx.index().len(), 10_000);
    assert_eq!(idx.seek_track(0, 500_050 * MS / 100), Some(5_000 * MS));
    assert_eq!(idx.samples().len(), 10, "one span (one Cluster) read, not the file");
}

fn mp4_frag(x: &Mp4Extras) -> Vec<u8> {
    let spec = TrackSpec { fourcc: *b"av01", config_box: Some(*b"av1C"), codec_id: "V_AV1", config: vec![0x81, 0, 0, 0], ..TrackSpec::test_pattern(1, 64, 48, 24) };
    let samples = PRES
        .iter()
        .enumerate()
        .map(|(i, &p)| SampleSpec { data: vec![i as u8; 3 + i], dts: i as i64 * 1000, pts: (p + 1) * 1000, duration: 1000, keyframe: i == 0 || i == 7 })
        .collect();
    build::mp4_with(&[MediaTrack { spec, samples }, opus_track(2, 25)], &Mp4Options { fragment: 4, ..Default::default() }, x)
}

fn per_track(d: &mut Demuxer) -> Vec<Vec<(i64, i64, bool, Vec<u8>)>> {
    let mut out = vec![Vec::new(), Vec::new()];
    for p in drain(d) {
        out[(p.track - 1) as usize].push((p.pts, p.dts, p.keyframe, p.data));
    }
    out
}

#[test]
fn sidx_and_mfra_open_reads_the_index_only() {
    for (name, x) in [("sidx", Mp4Extras { sidx: true, ..Default::default() }), ("mfra", Mp4Extras { mfra: true, ..Default::default() })] {
        let file = mp4_frag(&x);
        let mut full = Demuxer::open(file.clone()).unwrap();
        let mut idx = Demuxer::open_indexed(file).unwrap();
        assert!(idx.indexed(), "{name}");
        assert!(idx.samples().is_empty(), "{name}: no fragment was walked");
        assert!(!idx.index().is_empty());
        // Each track's packets in decode order, exactly; spans interleave in file order.
        assert_eq!(per_track(&mut full), per_track(&mut idx), "{name}");
        let mut a = Demuxer::open(mp4_frag(&x)).unwrap();
        let mut b = Demuxer::open_indexed(mp4_frag(&x)).unwrap();
        for t in [0i64, 200 * MS, 300 * MS, 9_000 * MS] {
            assert_eq!(a.seek_track(0, t), b.seek_track(0, t), "{name} t={t}");
            let first_a = (0..30).filter_map(|_| a.next_packet()).find(|p| p.track == 1).map(|p| key(&p));
            let first_b = (0..30).filter_map(|_| b.next_packet()).find(|p| p.track == 1).map(|p| key(&p));
            assert_eq!(first_a, first_b, "{name} t={t}");
        }
    }
    assert_eq!(build::mp4(&[build::test_pattern_track(1, 8, 8, 10, 5, 2)], &Mp4Options { fragment: 2, ..Default::default() }),
        build::mp4_with(&[build::test_pattern_track(1, 8, 8, 10, 5, 2)], &Mp4Options { fragment: 2, ..Default::default() }, &Mp4Extras::default()),
        "default extras write exactly what mp4 writes");
}

#[test]
fn index_open_falls_back_to_the_whole_file() {
    for file in [mp4_frag(&Mp4Extras::default()), build::mp4(&[build::test_pattern_track(1, 8, 8, 10, 30, 5)], &Mp4Options::default()),
        build::mkv(&[build::test_pattern_track(1, 8, 8, 10, 30, 5)], &MkvOptions::default())] {
        let a = Demuxer::open(file.clone()).unwrap();
        let b = Demuxer::open_indexed(file).unwrap();
        assert!(!b.indexed());
        assert_eq!(a.samples(), b.samples());
        assert_eq!(a.survey(), b.survey());
    }
}

// ----- M3 EDITS -----

fn bframe_mp4_track(pres: &[i64], keys: &[usize], fourcc: [u8; 4], config_box: [u8; 4], config: Vec<u8>, data: &dyn Fn(usize) -> Vec<u8>) -> MediaTrack {
    let spec = TrackSpec { fourcc, config_box: Some(config_box), codec_id: "V_AV1", config, ..TrackSpec::test_pattern(1, 64, 48, 24) };
    let samples = pres
        .iter()
        .enumerate()
        .map(|(i, &p)| SampleSpec { data: data(i), dts: i as i64 * 1000, pts: (p + 1) * 1000, duration: 1000, keyframe: keys.contains(&i) })
        .collect();
    MediaTrack { spec, samples }
}

fn av1_bframes() -> MediaTrack {
    bframe_mp4_track(&PRES, &[0, 7], *b"av01", *b"av1C", vec![0x81, 0, 0, 0], &|i| vec![i as u8; 3 + i])
}

#[test]
fn edits_every_elst_entry_is_a_segment() {
    let list = vec![(500u32, -1i64, 0x1_0000i32), (200, 1000, 0x1_0000), (100, 5000, 0x1_0000), (50, 3000, 0)];
    let x = Mp4Extras { edit_lists: vec![(1, list)], ..Default::default() };
    let file = build::mp4_with(&[av1_bframes()], &Mp4Options::default(), &x);
    let d = Demuxer::open(file.clone()).unwrap();
    // the first media edit (1000 media ticks after a 500 ms empty = 12000 ticks at 24 kHz) shifts pts by -11000
    let shift = -11_000i64;
    let want = vec![
        EditSegment { start_ns: 0, duration_ns: 500 * MS as u64, media_pts: None, rate: 0x1_0000 },
        EditSegment { start_ns: 500 * MS, duration_ns: 200 * MS as u64, media_pts: Some(1000 - shift), rate: 0x1_0000 },
        EditSegment { start_ns: 700 * MS, duration_ns: 100 * MS as u64, media_pts: Some(5000 - shift), rate: 0x1_0000 },
        EditSegment { start_ns: 800 * MS, duration_ns: 50 * MS as u64, media_pts: Some(3000 - shift), rate: 0 },
    ];
    assert_eq!(d.edit_list(0), &want[..]);
    assert_eq!(survey_of(&file).unwrap().streams[0].edits, want);
    // The pts still follow the first media edit, exactly as a file holding only it.
    let one = Demuxer::open(build::mp4(&[av1_bframes()], &Mp4Options { edits: vec![(1, 500, 1000)], ..Default::default() })).unwrap();
    let p = |d: &Demuxer| d.track_samples(0).map(|s| s.pts).collect::<Vec<_>>();
    assert_eq!(p(&d), p(&one));
    assert_eq!(d.tracks()[0].edit.map(|e| e.media_time), Some(1000));
    assert!(d.edit_list(9).is_empty());
    assert!(Demuxer::open(cues_webm()).unwrap().edit_list(0).is_empty(), "Matroska has no edit list");
}

#[test]
fn sdtp_and_trun_flags_reach_the_samples() {
    let bytes: Vec<u8> = (0..11).map(|i| if i == 0 || i == 7 { 0x24 } else if i % 3 == 0 { 0x58 } else { 0x99 }).collect();
    let x = Mp4Extras { sdtp: vec![(1, bytes.clone())], ..Default::default() };
    for fragment in [0usize, 4] {
        let d = Demuxer::open(build::mp4_with(&[av1_bframes(), opus_track(2, 25)], &Mp4Options { fragment, ..Default::default() }, &x)).unwrap();
        assert_eq!(d.track_samples(0).map(|s| s.depends).collect::<Vec<_>>(), bytes, "fragment={fragment}");
        assert_eq!(d.track_samples(0).map(|s| s.keyframe).collect::<Vec<_>>(), (0..11).map(|i| i == 0 || i == 7).collect::<Vec<_>>());
    }
    let plain = Demuxer::open(build::mp4(&[av1_bframes()], &Mp4Options::default())).unwrap();
    assert!(plain.track_samples(0).all(|s| s.depends == 0), "no sdtp: unknown");
}

/// Open GOP: I0 P3 B1 B2 | I6 B4 B5 P9 B7 B8 — the second key's B4 / B5 are decoded after it and presented before it.
const OPEN_GOP: [i64; 10] = [0, 3, 1, 2, 6, 4, 5, 9, 7, 8];

#[test]
fn leadskip_marks_the_leading_pictures_of_an_open_gop() {
    let t = |sdtp: Option<u8>| {
        let track = bframe_mp4_track(&OPEN_GOP, &[0, 4], *b"av01", *b"av1C", vec![0x81, 0, 0, 0], &|i| vec![i as u8; 4]);
        let x = Mp4Extras { sdtp: sdtp.map(|b| vec![(1, (0..10).map(|i| if i == 5 || i == 6 { b } else { 0 }).collect())]).unwrap_or_default(), ..Default::default() };
        let d = Demuxer::open(build::mp4_with(&[track], &Mp4Options::default(), &x)).unwrap();
        let tb = d.tracks()[0].timebase;
        d.samples_from(0, tb.to_ns(7000)).map(|r| (r.sample.pts, r.skip)).collect::<Vec<_>>()
    };
    let structural = t(None);
    assert_eq!(structural, vec![(7000, false), (5000, true), (6000, true), (10_000, false), (8000, false), (9000, false)], "rule 3");
    assert_eq!(t(Some(0x50)).iter().filter(|x| x.1).count(), 2, "rule 1: is_leading 1 skips");
    assert_eq!(t(Some(0xD0)).iter().filter(|x| x.1).count(), 0, "rule 1: is_leading 3 (decodable) keeps");

    // Rule 2: HEVC's NAL type — sample 5 RASL_N (8) skips, sample 6 RADL_N (6) keeps.
    let mut hvcc = vec![0u8; 23];
    hvcc[0] = 1;
    hvcc[21] = 0x03;
    let nal = |ty: u8| vec![0, 0, 0, 2, ty << 1, 1];
    let track = bframe_mp4_track(&OPEN_GOP, &[0, 4], *b"hvc1", *b"hvcC", hvcc, &|i| match i {
        5 => nal(8),
        6 => nal(6),
        _ => nal(1),
    });
    let d = Demuxer::open(build::mp4(&[track], &Mp4Options::default())).unwrap();
    assert_eq!(d.tracks()[0].codec, Codec::Hevc);
    let tb = d.tracks()[0].timebase;
    let r: Vec<_> = d.samples_from(0, tb.to_ns(7000)).map(|r| (r.sample.pts, r.skip)).collect();
    assert_eq!(r[1..3], [(5000, true), (6000, false)]);

    // A closed GOP has no leading pictures: nothing is skipped (NLEPROBE's file).
    let d = Demuxer::open(build::mp4(&[av1_bframes()], &Mp4Options::default())).unwrap();
    for t in [0i64, 200 * MS, 400 * MS] {
        assert!(d.samples_from(0, t).all(|r| !r.skip));
    }
}
