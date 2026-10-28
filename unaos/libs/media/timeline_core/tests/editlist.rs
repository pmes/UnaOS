// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! EDITMAP (LEDGER SR223): an MP4 edit list laid as a timeline over one source (`Timeline::lay_edit_list`).

use demux_core::EditSegment;
use timeline_core::{Clip, EditError, Kind, SourceId, Timebase, Timeline};

const MS: i64 = 1_000_000;
/// The media time base the KATs use: 1 ms ticks.
const MEDIA: Timebase = Timebase { num: 1, den: 1000 };

/// A 25 fps timeline (40 ms ticks) with one 50-tick source and one video track.
fn base() -> (Timeline, SourceId) {
    let mut tl = Timeline::new(25);
    let src = tl.add_source("AX", 50);
    tl.add_track(Kind::Video);
    (tl, src)
}

fn seg(start_ms: i64, dur_ms: u64, media_ms: Option<i64>, rate: i32) -> EditSegment {
    EditSegment { start_ns: start_ms * MS, duration_ns: dur_ms * MS as u64, media_pts: media_ms, rate }
}

fn clips(edits: &[EditSegment], origin_ns: i64) -> Result<Vec<Clip>, EditError> {
    let (tl, src) = base();
    tl.lay_edit_list(0, src, edits, MEDIA, origin_ns).map(|t| t.tracks[0].clips.clone())
}

#[test]
fn two_segments_are_two_clips() {
    let got = clips(&[seg(0, 400, Some(1000), 0x1_0000), seg(400, 400, Some(200), 0x1_0000)], 0).unwrap();
    assert_eq!(got, vec![Clip::new(SourceId(0), 25, 35, 0), Clip::new(SourceId(0), 5, 15, 10)]);
}

#[test]
fn an_empty_first_edit_starts_the_first_clip_at_the_dwell() {
    // 200 ms empty, then the media from its first frame: demux_core's shift puts that frame's pts at the dwell (200 ms),
    // which is also the stream's first pts (the origin), so the clip shows source tick 0 from tick 5.
    let got = clips(&[seg(0, 200, None, 0x1_0000), seg(200, 400, Some(200), 0x1_0000)], 200 * MS).unwrap();
    assert_eq!(got, vec![Clip::new(SourceId(0), 0, 10, 5)]);
}

#[test]
fn a_repeat_is_two_clips_over_the_same_source_range() {
    let got = clips(&[seg(0, 200, Some(400), 0x1_0000), seg(200, 200, Some(400), 0x1_0000)], 0).unwrap();
    assert_eq!(got, vec![Clip::new(SourceId(0), 10, 15, 0), Clip::new(SourceId(0), 10, 15, 5)]);
}

#[test]
fn a_rate_is_refused_by_name() {
    assert_eq!(clips(&[seg(0, 400, Some(0), 0x2_0000)], 0), Err(EditError::Rate), "double speed");
    assert_eq!(clips(&[seg(0, 400, Some(0), 0)], 0), Err(EditError::Rate), "a rate-0 dwell hold");
    assert_eq!(EditError::Rate.to_string(), "edit list rate is not 1 (no speed op)");
}

#[test]
fn an_empty_list_is_the_identity() {
    assert_eq!(clips(&[], 0).unwrap(), vec![Clip::new(SourceId(0), 0, 50, 0)]);
}

#[test]
fn duration_zero_is_the_rest_of_the_source() {
    assert_eq!(clips(&[seg(0, 0, Some(800), 0x1_0000)], 0).unwrap(), vec![Clip::new(SourceId(0), 20, 50, 0)]);
}

#[test]
fn a_trim_maps_from_the_negative_pts_origin() {
    // A camera trim: the first media edit starts 1 s in, so demux_core's shift leaves the first sample at -1 s (the
    // origin) and the edit's own media_pts at 0: the clip shows source ticks 25.. (1 s from the stream's start).
    let got = clips(&[seg(0, 600, Some(0), 0x1_0000)], -1000 * MS).unwrap();
    assert_eq!(got, vec![Clip::new(SourceId(0), 25, 40, 0)]);
}

#[test]
fn one_tick_of_rounding_past_the_end_is_clamped_and_more_is_refused() {
    // From media 1920 ms (tick 48) on a 50-tick source: 80 ms fits (48..50), 120 ms runs one tick over (48..51, clamped to
    // 50), 160 ms two ticks over (48..52, refused).
    assert_eq!(clips(&[seg(0, 80, Some(1920), 0x1_0000)], 0).unwrap(), vec![Clip::new(SourceId(0), 48, 50, 0)]);
    assert_eq!(clips(&[seg(0, 120, Some(1920), 0x1_0000)], 0).unwrap(), vec![Clip::new(SourceId(0), 48, 50, 0)], "one tick over: clamped");
    assert_eq!(clips(&[seg(0, 160, Some(1920), 0x1_0000)], 0), Err(EditError::OutOfSource), "two ticks over");
}

#[test]
fn an_edit_list_over_clips_already_there_is_validated() {
    let (tl, src) = base();
    let tl = tl.place(0, Clip::new(src, 0, 10, 0), timeline_core::Mode::Overwrite).unwrap();
    assert_eq!(tl.lay_edit_list(0, src, &[seg(0, 400, Some(0), 0x1_0000)], MEDIA, 0), Err(EditError::Overlap));
    assert_eq!(tl.lay_edit_list(3, src, &[], MEDIA, 0), Err(EditError::NoTrack));
    assert_eq!(tl.lay_edit_list(0, SourceId(7), &[], MEDIA, 0), Err(EditError::NoSource));
}
