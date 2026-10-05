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

M1 also gained PCM (in M2's commit, because the player needed a decodable audio track):
`Codec::Pcm { bits, float, big_endian }` from MP4 `sowt`/`twos` (AudioSampleEntry samplesize) and
`ipcm`/`fpcm` (ISO/IEC 23003-5 `pcmC`), Matroska `A_PCM/INT/LIT`, `A_PCM/INT/BIG`,
`A_PCM/FLOAT/IEEE` (Audio/BitDepth `0x6264`); the writers emit samplesize / BitDepth and
`build::pcm16_track`. KAT `pcm_tracks_in_both_containers` (16 KATs in total now).

## M2 — `libs/gneiss_pal/src/dsp` (the host pipeline; Stria owns the player)

- `dsp::demux` — re-export of `demux_core`.
- `dsp::avsync` — `TimeSource` (`SystemTime`, `ManualTime`); `WallClock` (play/pause/seek/rate
  in ppm); `AudioClock` (media time = start + (frames the device consumed − latency) / rate,
  interpolated with the time source for at most one callback period, never backwards; a stalled
  device freezes it); `MasterClock::select` (audio when an audio track decodes and an output
  exists, else wall); `Scheduler::decide` → Wait / Present / Drop (present when pts ≤ now +
  early; drop when the next frame is already due) and `on_display_tick` counting repeats;
  `DriftMeter` (offset, least-squares slope in ppm, worst excursion, per-frame |pts − clock|).
- `dsp::video` — the `VideoDecoder` seam AVCODEC fills (`decode(&Packet) -> Option<Frame>`,
  `flush`, `reset`, `is_real`), `Frame` (RGBA or I420 with BT.601 limited-range conversion),
  `decoder_for`, `TestPattern` (colour bars + 6-digit counter, `render` / `read_counter`; real
  for `utp1`, a labelled stand-in — counter = round(pts / frame duration) — for codecs with no
  decoder). `video::av1` (feature `av1`) is a STUB: it names av1_core's entry point
  (`image::decode_obus(data, config_obus, filters)`) and how the fold wires it, and refuses with
  `Unsupported`, so `--features av1` compiles today and plays the stand-in.
- `dsp::audio_track` — the per-packet audio seam (`AudioTrackDecoder` → interleaved f32 +
  pts); `PcmDecoder` built in (8/16/24/32-bit int either order, f32/f64). Compressed audio is
  AUDIOCODEC's `dsp::audio` (its decoders are file-oriented today; a packet entry point adds an arm).
- `resonance::nodes::stream::StreamSource` — the graph node media audio rides: a lock-free ring
  fed by `StreamFeed`, exact integer-phase linear resampling to the graph rate, and an atomic
  count of source samples consumed (what `AudioClock` is built on; starved = silence, no count).

Synthetic-timeline KATs (`cargo test -p gneiss_pal --release --lib dsp`: 14 green): 30 fps on
60 Hz is 2:2 with 30 repeats; 24 on 60 is 3:2 with every frame within half a period; 60 on 30
presents 30, drops 29, all even; pause freezes / seek jumps / rate 2.0; the audio clock tracks
played frames, interpolates and freezes on a stall; an audio device 100 ppm fast is measured at
100 ± 2 ppm and 6 ms over 60 s while video follows it within half a period; the wall master
has zero drift; counter round trip at six sizes; bars; `utp1` ordinals; unsupported codec named
and stand-in labelled; BT.601 known answers; PCM bit-exact through both containers; PCM layout
KATs. resonance: `same_rate_is_bit_exact_and_counted`, `upsampling_interpolates_and_counts…`.

## M3 — Stria's VIDEO track (`handlers/stria/src/media.rs`, `media_bus.rs`)

`Player::open(bytes, time, audio_out, display_hz)` → `tick(&mut dyn FrameSink)` once per
display refresh: demux → decode (≤ 4 frames ahead, pts-sorted queue; audio ≤ 0.5 s ahead) →
schedule → sink gets RGBA with its presentation ordinal; audio is decoded, downmixed to mono
(resonance's graph is mono) and pushed into a `StreamFeed`; each tick forwards the device's
consumption to the audio clock. `seek` lands on the keyframe at or before the target and decodes
forward silently; `pause` freezes the clock, so a paused tick presents exactly the poster frame.
`resonance_out(rate)` opens a one-node resonance graph on the default device (None headless).

Bus verbs (`bandy::SMessage`, STRIA section, golden KATs + completeness guard updated):
`PlayMedia` (existing) and new `MediaPoster`, `MediaPause`, `MediaResume`, `MediaSeek`,
`MediaStop` in; `MediaOpened`, `MediaFrame` (RGBA per presented frame), `MediaEnded`,
`MediaError` out; keyed by url. `MediaService::spawn` runs every session on one thread at the
display cadence (cpal streams are not `Send` everywhere).

Tests (`cargo test -p stria --release`: 8 unit + 5 media, green): a 2 s `utp1` + 48 kHz PCM
stream in MP4 and WebM played with a real resonance graph pulled 800 samples per 60 Hz refresh
as the device: audio master, all 50 frames shown once in order with counter == ordinal == pts/40
ms, each within half a period of the audio clock, and the samples resonance produced equal the
source sample for sample (go-red proven: not forwarding device consumption fails it); seek to
1 s shows frame 25 with ordinal 25; poster = frame 0 while paused; VP9 without a decoder plays
the labelled stand-in with counters 0..9; the bus service answers poster → play → ended (every
frame presented or dropped, counters match pts) and a missing file with `MediaError`.

## M4 — the faces

**Headless face: `tools/play-check`** (no third-party crates; its PNG writer — stored DEFLATE,
CRC-32, Adler-32 — is in `src/png.rs` with known answers). `play-check <file> [--frame N --out
f.png] [--hz 60] [--oracle chromium-oracle.jsonl]`, `play-check --make-utp <out> [n] [fps]`.

| check | result |
|---|---|
| `utp1` WebM and MP4 (our writer), frame N → PNG, counter read back | N ∈ {0, 1, 42, 89}: counter == N, 90/90 presented, 0 dropped |
| Chromium decodes play-check's PNGs (`oracle/png-oracle.js`) | FNV-1a of Chromium's RGBA == ours for all 4 PNGs (b2930985, fbe34985) — pixel exact |
| av1.mp4 (stand-in) pts vs Chromium mediaTime | 10/10 frames, max diff 0.000 ms (one frame = 100 ms) |
| vp9.mp4 (stand-in) | 10/10, 0.000 ms |
| test-1s.webm VP9+Opus (stand-in, Opus silent → wall clock) | 30/30, 0.000 ms (one frame = 33.2 ms) |
| test-av-…-10kfr.webm VP8+Vorbis (stand-in, wall clock) | 60/60, 0.000 ms |

The pts numbers prove container timing + scheduling against Chromium, not pixels: no real codec
decodes in this tree yet, and the frame-5 PNGs of the real vectors are the stand-in card
reading 5. `cargo test -p play-check --release` runs both checks (vectors skipped loudly when
not fetched).

**GUI face — deferred, deliberately.** phonolite is the tone vessel and its only backend is
quartzite's macOS AppKit tone panel; this container cannot build or run it, and a video pane
written blind would be chicken wire by another name. The playback surface is the bus:
`MediaFrame` carries RGBA a vessel blits (aether-shell already blits `SurfaceBlit`). The
recommendation for the fold: phonolite grows a quartzite image view fed by `MediaFrame` (one
vessel = Stria's face, tone and screen), rather than a new vessel.

**Aether.** Its `<video>`/`<audio>` click already stages `PlayMedia`, which Stria now serves.
Owed: Aether firing `MediaPoster` for each `<video>` on load and painting `MediaFrame` into the
element's box (touches Aether's layout/paint, the AETHERSEE executor's lane).

## Third-party crates

None added. demux_core, dsp, play-check: no dependencies. resonance's `ringbuf 0.5` (existing,
a utility: the lock-free ring) carries the media audio; `cpal 0.18` (existing, device I/O,
utility). Chromium/Playwright are the test oracle, not linked.

## What plays today, exactly

`utp1` test-pattern streams in MP4/fMP4/WebM/Matroska play for real (pixel-exact frames, counter
== N). PCM audio in those containers plays through resonance as the master clock. AV1/VP9/VP8
video plays the labelled stand-in with Chromium-matching timing; Opus/Vorbis/AAC/FLAC audio is
reported unsupported and the player runs silent on the wall clock.

## Ceiling and owed

AV1 frames (AVCODEC's fold: replace `dsp::video::av1`'s body, add the av1_core dependency);
compressed audio (AUDIOCODEC packet entry point → `dsp::audio_track` arm; Opus pre-skip /
CodecDelay trimming then applies); output latency unknown to `AudioClock` (0 today; cpal does
not report it portably); a seek with audio leaves up to the ring (0.5 s) of stale samples, so the
audio clock is offset by that much after a seek until a ring flush is added to `StreamFeed`;
stereo is downmixed to mono (resonance graph); urls are local files only; the GUI face and
Aether's poster/paint as above; whole-file-in-memory (M1 ceiling).
