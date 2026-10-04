# PLAYBACK (LEDGER SR26) — video playback as a system capability

Branch `exec-media-playback`, cut from `0686cc1b`. Family media, 2026-10-04.

## Finding

Before this arc no container parser, playback clock or video path existed anywhere in the tree:
Stria (`handlers/stria`) drove the resonance tone graph only, and Aether staged a
`SMessage::PlayMedia { url, title, mime }` on a click on a `<video>`/`<audio>` element
(`handlers/aether/src/lib.rs`, the `MouseUp` arm) that no one consumed.

## M1 — `unaos/libs/media/demux_core` (`no_std` + `alloc`, no dependencies)

Spec sections implemented (the section maps are in the module heads of `src/mp4.rs` and
`src/mkv.rs`):

- ISO/IEC 14496-12: box header (32-bit, 64-bit largesize, size 0, `uuid`), `ftyp`, `moov/mvhd`,
  `trak/tkhd`, `edts/elst` (one optional empty edit + one media edit), `mdia/mdhd`, `hdlr`,
  `minf/stbl/stsd` (VisualSampleEntry and AudioSampleEntry incl. QuickTime v1/v2 sound
  descriptions; `av01`/`av1C`, `avc1`/`avc3`/`avcC`, `hvc1`/`hev1`/`hvcC`, `vp08`/`vp09`/`vpcC`,
  `mp4a`/`esds` (ISO/IEC 14496-1 ES_Descriptor), `Opus`/`dOps` with pre-skip, `fLaC`/`dfLa`,
  `utp1` test pattern), `stts`, `ctts` v0/v1, `stsc`, `stsz`/`stz2` (4/8/16-bit), `stco`/`co64`,
  `stss`, `mvex/mehd/trex`, `moof/traf/tfhd` (every optional field, base-data-offset,
  default-base-is-moof and the implicit "continue after the previous traf" rule), `tfdt` v0/v1,
  `trun` (data offset, first-sample flags, per-sample duration/size/flags/composition offset v0/v1),
  sample flags `sample_is_non_sync_sample`.
- RFC 9559 (Matroska) / RFC 8794 (EBML): vint IDs and sizes, unknown sizes (Segment and Cluster,
  the Cluster ending at the next Segment-level element), EBML header + DocType, `Info`
  (TimestampScale, 4- or 8-byte float Duration), `TrackEntry` (TrackNumber, TrackType, CodecID,
  CodecPrivate, DefaultDuration, CodecDelay, Video PixelWidth/Height, Audio SamplingFrequency /
  Channels, ContentEncodings with header stripping), `Cluster/Timestamp`, `SimpleBlock`,
  `BlockGroup` (`Block`, `BlockDuration`, `ReferenceBlock`), Xiph / fixed / EBML lacing. CodecIDs
  mapped: `V_AV1`, `V_VP8`, `V_VP9`, `V_MPEG4/ISO/AVC`, `V_MPEGH/ISO/HEVC`, `A_OPUS`,
  `A_VORBIS`, `A_AAC*`, `A_FLAC`, `V_UNAOS/TESTPATTERN`.

One API: `Demuxer::open(Vec<u8>)` → `tracks()` (id, kind, codec, raw codec name, config record,
timebase, size, rate/channels, codec delay, sample count, span) → `next_packet()` yielding
`Packet { track, pts, dts, duration, keyframe, data }` in decode-time order merged across tracks;
`seek(ns)` lands on the last reference-track keyframe at or before the target. `build` holds
minimal MP4 (progressive and fragmented) and WebM/Matroska writers, the `utp1` test-pattern
packet format, and `remux_tracks` (MP4 source → either container, payloads byte-exact).

### KATs (`cargo test -p demux_core --release`: 15 KATs + 3 oracle/sample tests, all green)

Writer round trips, each asserting the exact packet table: progressive MP4 test pattern
(chunking, stss); B-frames with `ctts` v1 + edit list; an empty edit delaying presentation;
A/V interleave and merge order with Opus `dOps` pre-skip; fragmented MP4 (A+V, 3 fragments);
seek to keyframe; WebM SimpleBlocks across clusters; Matroska with unknown-size Segment and
Clusters, BlockGroups, no DefaultDuration; Xiph, EBML and fixed lacing. Byte-literal files
(assembled in the test, not by the writer): a fragmented MP4 with two trafs and no base offsets,
tfhd defaults, first-sample flags only, a 64-bit largesize `mdat`; a progressive MP4 with `stz2`,
`ctts` v0, two `stsc` runs and `stss`; a WebM with an unknown-size Segment, an 8-byte Cluster
size, Xiph and EBML lacing, header stripping and a 4-byte float Duration. Refusals: every
truncation and every single-byte flip of a valid MP4 and WebM returns an error or a table,
never a panic. Go-red proven by mutation (Matroska block time +20 ticks: 3 KATs and the oracle go
red; fragmented sample flags ignored: 2 KATs go red).

### Oracle (Chromium, `tools/play-check/oracle/chromium-oracle.js`)

Chromium (Playwright build at `/opt/pw-browsers`) plays each file muted at 0.25x and reports
`video.duration`, `videoWidth/Height`, `presentedFrames`, the `requestVideoFrameCallback`
`mediaTime` sequence, and an FNV-1a hash of each presented frame's RGBA. The result is committed
as `unaos/libs/media/demux_core/tests/data/chromium-oracle.jsonl`; the sample test fetches the
vectors (`tests/data/vectors.txt`: URL + sha256, W3C web-platform-tests corpus, nothing committed)
and asserts duration within 1.5 ms, exact size, frame count == `presentedFrames`, and every
`mediaTime` on a demuxed pts within 1 ms (the brief asked for one frame).

| file | container / codec | duration (ours / Chromium) | frames (ours / presented) | max \|mediaTime − pts\| |
|---|---|---|---|---|
| av1.mp4 | MP4 / AV1 | 1.000 / 1.000 | 10 / 10 | 0 |
| vp9.mp4 | MP4 / VP9 | 1.000 / 1.000 | 10 / 10 | 0 |
| test-av-384k-…-10kfr.webm | WebM / VP8 + Vorbis | 2.023 / 2.023 | 60 / 60 | 0 |
| test-1s.webm | WebM / VP9 + Opus | 1.008 / 1.008 | 30 / 30 | 0 |
| av1-remux.webm (our writer) | WebM / AV1 | 1.000 / 1.000 | 10 / 10 | 0 |
| av1-frag.mp4 (our writer) | fragmented MP4 / AV1 | 1.000 / 1.000 | 10 / 10 | 0 |
| vp9-frag.mp4 (our writer) | fragmented MP4 / VP9 | 1.000 / 1.000 | 10 / 10 (9 callbacks) | 0 |

The remuxes are how Chromium judges this crate's fragmented-MP4 and WebM paths on a real AV1
stream: the test rebuilds them, checks their sha256 against the bytes Chromium played, checks
every payload byte-exact against the source, and Chromium's per-frame pixel hashes for the remux
equal the source's (av1.mp4 frame 5 viewed: the test card's counter reads 5). The fifth vector,
a fragmented H.264 MP4, has no browser oracle (this Chromium build ships no H.264 decoder); its
structure is asserted (48 frames, 2.000 s, sorted pts a gapless 24 fps ladder after B-frame
reorder).

### Ceiling (M1)

Whole file in memory (no streaming reader); first `stsd` entry only; edit lists beyond "empty +
one media edit" (dwell, rate ≠ 1, multiple segments); `sidx`/`mfra` ignored (fragments are found
by walking); encrypted tracks (`encv`/`enca`, Matroska ContentEncryption) and zlib/bzlib/LZO
ContentCompression are refused or reported as `Codec::Other`; Matroska Cues are not used (seek
walks the in-memory table); Matroska dts = pts (exact for AV1/VP8/VP9/Opus/Vorbis, not for AVC
B-frames in Matroska); remux of Matroska-sourced VP9/Opus to MP4 is refused (CodecPrivate is not
the MP4 record).
