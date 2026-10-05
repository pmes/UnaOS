# AUDIOTRACK (LEDGER SR45) — compressed audio through Stria, `<audio>` in Aether, http(s) media

Branch `exec-host-audiotrack`, cut from `8efadd95`; merged first, in order: `exec-media-audio`
(AUDIOCODEC @76ee6f8d), `exec-media-playback` (PLAYBACK @63582d93; `dsp/mod.rs` hand-joined to
keep every `pub mod`), `exec-host-aethervideo` (AETHERVIDEO @6846da6d). Family host, 2026-10-05.

## Finding

Three ends met and none was tied: AUDIOCODEC had per-packet decoders for Opus/Vorbis/MP3/AAC/FLAC;
PLAYBACK's `dsp::audio_track` played only PCM and reported every compressed track `Unsupported`
(the player then ran silent on the wall clock) and downmixed to mono because resonance's graph was
mono; a seek left up to ½ s of stale samples in the device ring; Stria refused files without a
video track, so `<audio>` could not play; Aether's http(s) `<video>` showed "only local files".

## What the arc built (no new crate)

| layer | change |
|---|---|
| `unaos/libs/media/demux_core` | `Codec::Mp3` (MP4 OTI 0x69/0x6B and `.mp3`, Matroska `A_MPEG/L3`); Matroska BlockGroup `DiscardPadding` (RFC 9559 §5.1.3.5.6) → `Packet::discard_ns`; the media edit's segment duration and Apple `iTunSMPB` (priming, total) → `Track::{trim_start_ns, play_ns}` |
| `libs/gneiss_pal::dsp::audio_track` | the registry: Opus (`OpusHead`/`dOps`, output gain, PLC on empty packets), Vorbis (Xiph-laced headers, WAVE channel order), AAC-LC (ASC, synthesised when absent), MP3 (one frame per packet, reservoir across packets), FLAC (`dfLa`/`fLaC` STREAMINFO), PCM; every compressed decoder behind one gapless trimmer, `Timing` |
| `libs/resonance` | `AudioNode::channels()`; the graph carries a right buffer per node (`process_stereo`), the engine plays L/R; `StreamSource` of 1 or 2 channels with lock-free **flush** (seek) and **pause** |
| `handlers/stria` | `Player`: audio-only sessions (container tracks, or bare MP3/Ogg/FLAC/WAV/AIFF/ADTS files through `dsp::audio::Decoder`), stereo to the device (ITU downmix above 2 ch), sample-accurate timeline placement (drop before the next expected sample, silence across a gap), ring flush on seek, pause pauses the device, mute, peak L/R meters per 50 ms; a video ends with its audio |
| `libs/bandy` | `MediaFrame.levels` (omitted from the wire when empty: video frames unchanged); `MediaMute { url, muted }` |
| `handlers/aether` | `<audio>` asks for metadata on load (`preload="none"` waits); meter frames move the control's time; `<audio controls>` paints Chromium's default audio control; `muted` → `MediaMute`; http(s) sources fetched into `~/.cache/unaos/aether/media/<sha256(url)>` (byte ranges when the server allows, resuming `.part`; else one GET) and handed to Stria as `file://…` |
| `tools/play-check` | a headless device (consumes stereo at the stream rate on the manual clock: audio is master), `--audio-out`, and `--audio-ref/--audio-floor` — the sample-for-sample oracle |
| `tools/aethervideo-check` | the page gains `<audio controls>` (case 4) and an http `<video>` from a local `python3 -m http.server` (case 5) |

### Gapless (`dsp::audio_track::Timing`)

Output samples are time-stamped by counting from the stream's first packet (after a seek, from the
first packet that yields samples) — never from each packet's rounded stamp — then shifted by the
stated codec delay and trimmed to the presented window:

* Matroska/WebM: shift = `CodecDelay`, else the `OpusHead` pre-skip; floor = max(first block, 0);
  each block's `DiscardPadding` tail dropped.
* MP4 with an edit list (first pts < 0): floor 0, end = the media edit's segment duration.
* MP4 without one: shift = `dOps` pre-skip (Opus) + `iTunSMPB` priming; end = `iTunSMPB` total.
* Bare files: `audio_core`'s own rules (Ogg granules, LAME/Xing, MP4 inside `decode_all`).

The Vorbis anchor rule was found by the oracle: the libwebm-muxed `bear-320x240.webm` and WPT
`test-av-…-10kfr.webm` stamp the second packet 3 ms (132 frames) after the first, whose decode is
empty; anchoring on the first output-producing packet put 132 frames of silence in front.
Anchoring on the stream's first packet is right for both muxing conventions (FFmpeg-made
`bear.webm` too).

## M1 — KATs (`cargo test -p gneiss_pal --test audio_track`, 5 tests)

Each codec's packets go through demux_core's own MP4 and Matroska writers and back:

* **Opus** — the 12 libopus conformance streams (`audio_core/tests/data/opus`, 7,330 packets):
  untrimmed decode MD5 == libopus 1.5.2's in both containers (12/12 × 2); trimmed == that PCM
  minus the 312-sample pre-skip whether stated by `OpusHead` (Matroska), an MP4 edit list, or
  `dOps` alone — 36/36 exact, frame counts equal.
* **Vorbis** — the 10 libvorbis streams re-muxed into WebM: sample-for-sample equal to
  `audio_core`'s Ogg decode (10/10), longer only by the Ogg end trim WebM cannot carry.
* **AAC** — `m01/m04/m07/m10.m4a` (edit list; `iTunSMPB` with the moov last; fragmented; plain)
  through `Timing::of`: bit-identical to `audio_core`'s file decode (proven vs faad2), frame counts
  equal — 4/4.
* **MP3** — Chromium's `sfx.mp3` frames in MP4 with an edit list of LAME delay + 529: equal to
  the file decode; in Matroska (no edit) equal at offset = that delay.
* **FLAC** — Chromium's `sfx.flac` frames with `dfLa` / `fLaC`: exact in both.

Go-red: dropping the MP4 shift fails Opus (`mp4 dOps: frames`) and AAC.

`resonance` (+3 KATs): stereo L/R bit-exact and frames counted; flush drops 236 queued frames
unplayed and uncounted; pause is silent, takes nothing, resumes on the next sample; the graph
carries a stereo node and copies mono ones. `stria --test audio` (6): an audio-only Opus WebM
(t06, stereo) through a real resonance stereo graph is both channels sample-exact with the
registry decode, every 50 ms meter exact, ends; a seek's first sound is the target's (aligned,
40 dB to a continuous decode — CELT's energy predictor re-converges after a decoder reset) —
**go-red without the flush: offset −229, 0 dB**; pause/resume continuous; a bare WAV exact in
stereo; the bus answers a 0×0 `MediaOpened`, ten meter windows, `MediaEnded`; mute zeroes the
device while the clock runs.

## M1 — the oracle (`cargo test -p play-check --test audio_oracle`)

`play-check <file> --audio-out` plays through Stria's `Player` with a headless device, audio as
master clock; what the device played is compared with Chromium's `decodeAudioData`
(`audio_core/oracle/chromium-oracle.cjs`, `OfflineAudioContext` at the file's rate) per channel,
offset searched ±2048 frames and required to be 0. AAC (this Chromium has none) is held to
`audio_core`'s file decode. 18 vectors (Chromium and WPT media test data, URL + sha256 in
`tools/play-check/tests/data/audio-vectors.txt`, fetched at test time):

| vector | codec | floor | frames ours/ref | SNR dB | max LSB |
|---|---|---|---|---|---|
| bear-opus.webm | Opus | 50 dB | 131200/131200 | 59.13 | 1.04 |
| bear-opus.mp4 | Opus | 50 dB | 131208/131208 | 59.13 | 1.04 |
| opus-trimming-test.webm (CodecDelay + DiscardPadding) | Opus | 50 dB | 545026/545026 | 86.97 | 1.53 |
| opus-trimming-test.mp4 (edit list) | Opus | 50 dB | 550785/550785 | 87.89 | 1.53 |
| bear-vp9-opus.webm (A/V) | Opus | 50 dB | 130048/130048 | 69.87 | 13.0 |
| sfx-opus.ogg (bare) | Opus | 50 dB | 13844/13844 | 75.89 | 9.74 |
| test-1s.webm (A/V, +pts oracle) | Opus | 50 dB | 48000/48000 | exact | 0 |
| bear.webm (A/V) | Vorbis | 130 dB | 45632/45632 | 136.04 | 0.00 |
| bear-320x240.webm (A/V) | Vorbis | 130 dB | 121024/121024 | 136.19 | 0.00 |
| test-av-…-10kfr.webm (A/V, +pts oracle) | Vorbis | 130 dB | 89088/89088 | 136.94 | 0.02 |
| sfx.ogg (bare) | Vorbis | 130 dB | 15435/15936 ¹ | 137.69 | 0.01 |
| sfx.mp3 (bare, LAME gapless) | MP3 | 2 LSB | 11025/11025 | 90.06 | 1.67 |
| sfx-mp3-in.mp4 (made here, OTI 0x6B) | MP3 | 2 LSB | 13824/13824 | 89.68 | 1.67 |
| sfx.flac, sfx-flac.mp4, bear-flac.mp4 | FLAC | exact | equal | exact | 0 |
| sfx.m4a (edit list), bear-mpeg2-aac-only_frag.mp4 | AAC-LC | exact vs audio_core | equal | exact | 0 |

¹ Chromium skips the Ogg Vorbis end trim (AUDIOCODEC's finding); ours equals libvorbis.

The floors are AUDIOCODEC's: Opus 50 dB because Chromium runs libopus in float and ours is the
normative fixed-point reference (bit-exact by KAT); the ledger's "Opus exact" holds against libopus
fixed, not against Chromium. The two `+av` vectors also keep PLAYBACK's video oracle with audio now
driving the clock: every presented pts within one frame of Chromium's mediaTime, counts equal, 0
dropped.

## M2 — Stria audio-only sessions

`MediaPoster` on an audio-only stream answers `MediaOpened { width: 0, height: 0, video: "" }`
with the duration; ticks fire 0×0 `MediaFrame`s whose `levels` are the peak L/R of each 50 ms
window that came due (a video session attaches its meters to its next frame); `MediaEnded` when
the device has played every pushed frame. KATs above (`stria --test audio`), bandy wire KATs
`kat_media_frame_levels`, `kat_media_mute`.

## M3 — Aether `<audio controls>`, `<video>` audio

`handlers/aether/src/media/mod.rs` + `render::paint_audio_controls`:

* `<audio>` registers like `<video>`: `MediaPoster` on load unless `preload="none"` (Chromium's
  default preload is `metadata`); `MediaOpened`'s duration feeds the control; 0×0 meter frames
  set the element's `levels` and its time (window start + windows × 50 ms); a meter frame is never
  taken for a picture.
* `<audio controls>` paints Chromium's default audio control, geometry measured from Chromium
  1194's own rendering of a 300×54 control: the rgb(241,243,244) pill with fully rounded ends,
  play triangle / pause bars (Material icons at 20 px, x+16⅓), `m:ss / m:ss` at x+47 (13 px), the
  timeline x+129 … right−91 (4 px, round caps; played rgb(11,11,11), rest rgb(88,89,89)), the
  speaker (`volume_up`, 24 px, right−69), the overflow dots (`more_vert`, right−37.5), all
  4×4-sample antialiased. `<video controls>` keeps AETHERVIDEO's own strip.
* `<video>` with an audio track now plays it (aether-shell's `MediaService` runs with audio;
  the registry decodes it); `muted` sends `MediaMute { muted: true }` right after the opening
  request (Stria zeroes the samples, the clock runs on, so `autoplay muted` stays in sync and
  silent). `can_play_type` accepts the bare audio types (`audio/mpeg`, `audio/ogg`, `audio/wav`,
  `audio/flac`, `audio/aac`, `audio/aiff`) and the `mp3`/`1` codec names.

KATs (`cargo test -p aether --lib media::`, 16, 5 new): the control box is 300×54 and its pixels
at the pill, the page corner, the play triangle, the played / unplayed timeline at 1.1 s of 2.5 s,
the speaker and a menu dot are the measured colours; playing swaps in the pause bars;
`preload="none"` waits for play and `muted` follows with `MediaMute`; the autoplay test now sees
`poster c.wav` and the mutes.

Oracle (`cargo test -p aethervideo-check`, page case 4: `<audio controls src="tone.wav">`, a
2.5 s stereo WAV `make` writes): box geometry equal to Chromium's `getBoundingClientRect`
(330,110,300,54), Aether's duration (Stria's audio-only `MediaOpened`) == Chromium's
`audio.duration` to < 1 ms, and the control's pixels **94.80 % within 8, 91.75 % exact**
(18.9 dB) at frames 0/7/9 — the residual is the time text (different font rasterisers; Chromium's
glyphs are LCD-antialiased) and the speaker's sub-pixel edges. The video boxes are unchanged
(98.75 % within 8, counters equal).

## M4 — http(s) media through the cache

`media::fetch_to_cache` / `ensure_cached` / `take_outbox`: an http(s) source's bus key becomes
`file://<cache>/<sha256(url)>` (lowercase hex, `crypto_core::sha2`), `src` keeps the page's url;
the fetch runs on its own thread with Aether's blocking reqwest client: `Range: bytes=a-b`
requests of 1 MiB while the server answers 206 + `Content-Range` (resuming any `.part` a broken
fetch left), one whole GET when it answers 200, a 416 on a complete partial accepted; 1 GiB cap;
the file is renamed into place only when whole. The element's opening request (poster or play,
plus its mute) is held until `take_outbox` sees the completion; a failure becomes a
`MediaError` for the key that round-trips over the bus and paints in the box. A cached url opens
at once on the next page.

KATs: `fetch_uses_byte_ranges_resumes_a_partial_file_and_falls_back_to_one_get` (a std-only HTTP
server in the test: 2,500 bytes in exactly `bytes=0-999`, `1000-1999`, `2000-2999`; a 1,200-byte
`.part` resumes with `1200-2199`, `2200-3199`; a no-range server → one GET, a stale `.part`
discarded; 404 → error, no file) and `http_media_is_cached_then_handed_to_stria_as_a_file_and_a_failure_paints`.
Oracle: page case 5 is the VP9 `<video>` at `http://127.0.0.1:<port>/pattern-vp9.webm` served by
python's `http.server` with byte ranges (`oracle/serve.py`, a 40-line subclass — plain
`python3 -m http.server` has no ranges, and Chromium then cannot seek: its box stayed at frame 0
while Aether's read 7, the first run's finding). Aether fetched it into the cache over ranges,
Stria played the file: counter N == N in both at frames 0/7/9, 98.75 % within 8, 29.06 dB —
identical to the same file loaded locally.

## Third-party crates

None added. Aether gains a path dependency on `crypto_core` (UnaOS's own SHA-256, SR27) for the
cache names. Existing utilities only: `ringbuf 0.5` (resonance's lock-free ring), `cpal 0.18`
(device I/O), Aether's `reqwest 0.12` (the HTTP client for the media fetch — a utility), the
check tools' `image 0.25.10` (PNG). Nothing decodes media but UnaOS's own `audio_core` and
`demux_core`. Chromium/Playwright and `python3 -m http.server` are test oracles/fixtures only.

## Ceiling and owed

* Opus multistream (mapping family 1/255, > 2 channels), AAC SBR/PS (HE-AAC plays its LC core at
  half rate), AAC Main/LTP, MPEG Layer I/II, ALAC — AUDIOCODEC's ceiling, unchanged.
* A Matroska Vorbis/Opus stream seeked mid-stream re-anchors on the first output-producing
  packet's (ms-rounded) stamp: within one block, not sample-exact; a reset Opus decoder
  re-converges over its first frames (40 dB at the target in the KAT, 100 ms pre-roll decoded and
  dropped). Bare audio files seek by re-decoding from the start (forward-only `audio_core`
  sources): exact, O(position).
* An MP3 without a Xing header is decoded once at open to learn its duration.
* `AudioClock` latency is 0 (cpal reports none portably); the device ring holds ≤ ½ s, so a mute
  toggled mid-play takes up to that long to be heard (the flush is reserved for seeks).
* No resampling to the device beyond `StreamSource`'s linear interpolation; no volume verb
  (`MediaMute` only); a sample-rate change mid-stream is skipped with a warning.
* The media cache has no HTTP freshness (ETag/Expires/Vary): a cached url is reused until the file
  is deleted; no eviction; no progressive playback while fetching (Stria opens whole files —
  demux_core's whole-file ceiling).
* `<audio controls>` is drawn, not interactive beyond the click toggle (no seek from the
  timeline, no volume or overflow menu); its text is our rasteriser's, not Chromium's.
* JS `HTMLMediaElement` (play/pause/currentTime/volume/muted/events) still owed (AETHERVIDEO's
  ceiling); `loop`, `playbackRate`, `<track>` not handled.
* Metal: none of this is flown; the kernel's `play` (AUDIOCODEC M5) is a separate path.
