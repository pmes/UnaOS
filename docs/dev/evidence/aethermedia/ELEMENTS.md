# AETHERMEDIA (SR107) — the element table

What each media element does after the arc, and what it refuses and why. Built on
exec-host-aethermedia from 98d9f842; compiled only (R107: `cargo check -p aether -p stria -p bandy
--tests` rc=0), proof owed to the fold. Aether renders, Stria plays (CODEX Amendment II); Stria
decodes through gneiss_pal::dsp, which re-exports the owned cores.

| element / rule | plays | refused, and why |
|---|---|---|
| `<video>` WebM/MP4/MKV, VP8 | frames from vp8_core via Stria, painted at render/mod.rs:2098 (`media::paint_for`) | — |
| `<video>` AV1 | frames from av1_core via Stria (handlers/stria/Cargo.toml now builds gneiss_pal with `av1`; dsp/video.rs:112) | what av1_core itself refuses comes back as `MediaError` ("AVCODEC: owed") |
| `<video>` VP9 / H.264 / HEVC | the labelled stand-in (canPlayType says yes so the box is not empty) | no owned decoder |
| `<audio>` / audio tracks: Opus, Vorbis, AAC-LC, MP3, FLAC, PCM | PCM from audio_core via Stria's audio_track to the host sink | — |
| `src` / `<source type>` selection | first playable source | `video/ogg`, Theora, unknown codecs: skipped; none left → `error` at the last `<source>`, networkState NO_SOURCE |
| preload | `metadata` default (MediaPoster), `none` waits (`suspend`) | `auto` = `metadata` |
| autoplay | `autoplay muted` plays | bare `autoplay`: Chromium's policy (no media engagement index) |
| loop | seek 0 + play at MediaEnded | — |
| controls | Chromium-style strip / audio control (render/) | — |
| readyState / networkState and events | loadstart, suspend, durationchange, resize, loadedmetadata, loadeddata, canplay, canplaythrough, play, playing, timeupdate (250 ms), seeking, seeked, pause, ended, error, volumechange, ratechange; the JS getters read media/'s machine | — |
| currentTime / duration / seeking / error | target read at once while seeking; `error` is `{code, message, MEDIA_ERR_*}` (4 before metadata, 3 after) | `error` is a plain object, not a `MediaError` interface instance |
| volume / muted | `MediaVolume` → Stria scales the samples (mute separate, wins); a pre-open volume rides the opening request | — |
| playbackRate | `MediaRate`: 0 holds clock and device with the element still playing; 1 runs | other rates: Stria plays at 1x and logs it — gneiss_pal's master clock has no rate and the audio path no time-stretch |
| captions (`<track>`) | — | off by the brief |
| `<img srcset sizes>` | HTML parser + sizes, fetch and paint agree (800 px selection viewport) | — |
| `<picture>` | `images::img_source` at net, layout and render | — |
| `loading=lazy` | skipped at page fetch; fetched off-thread when its box is within 1250 px of the viewport (load, scroll), relayout on arrival | http(s) only; a lazy `file:` image is never fetched (as non-lazy `file:` images today) |
| AVIF in `<img>` / `<picture type=image/avif>` | av1_core::image::decode_avif (ftyp avif/avis) | what av1_core refuses is ledgered `img-avif-refused` |
| WebP in `<img>` | pixel_core (lossless, lossy via vp8_core, animated first frame) | — |

Tests: handlers/aether/tests/media_element.rs (five tests, no binary fixture).

## AETHERMEDIA2 (SR134) — the refused column, taken

Built on exec-host-aethermedia2 from ef3fe22d; compiled only (R107: `CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave31/target
cargo check -p demux_core -p audio_core -p gneiss_pal -p stria -p aether --tests` rc=0), proof owed to the fold. This table
supersedes the rows above that it names.

| element / rule | plays | refused, and why |
|---|---|---|
| Media Source Extensions: `MediaSource` | `readyState` (closed / open / ended), `duration` (NaN, from the init segment, +Infinity without one, extended by media past it, the highest end time at endOfStream), `sourceBuffers` / `activeSourceBuffers`, `addSourceBuffer` (type gate below), `removeSourceBuffer`, `endOfStream(error?)`, `setLiveSeekableRange` / `clear…` (kept, not used by seekable), `sourceopen` / `sourceended` / `sourceclose`, `canConstructInDedicatedWorker` = false | MSE in workers (`MediaSourceHandle`), `ManagedMediaSource` |
| `MediaSource.isTypeSupported` | ISO BMFF (`video/mp4`, `audio/mp4`) and WebM (`video/webm`, `audio/webm`) byte streams whose codecs Stria decodes: VP8, VP9, AV1, Opus, Vorbis, AAC-LC, MP3, FLAC, PCM, `utp1` | H.264 / HEVC: answered false so an adaptive player picks a codec that plays (they would be the labelled stand-in); Matroska, MPEG-2 TS, ADTS byte streams |
| `SourceBuffer` | `appendBuffer` (segment parser loop: fMP4 `moov` init + `moof`/`mdat` media segments with `tfdt`; WebM EBML/Info/Tracks init + known- or unknown-size Clusters, any chunking), coded frame processing (`timestampOffset`, `mode` segments / sequence, append window, random-access gating, overlap + dependency removal), `remove(start, end)` to the next random access point, `abort()`, `buffered` (intersection of its track buffers), `updating`, `updatestart` / `update` / `updateend` / `error` / `abort`; an append error ends the stream with `decode` | `changeType` checks the type only (no codec switch inside one buffer); `audioTracks` / `videoTracks`; generated timestamps for MPEG audio byte streams |
| `URL.createObjectURL(mediaSource)` → `src` | the element attaches the MediaSource; Stria plays the appended segments through `MediaSourceFeed` (bus key `mediasource:<journal>`; the page's journal of the same `demux_core::fragment` records); seeks land in buffered ranges; at the buffered end Stria holds its clock and the element fires `waiting`; `ended` after endOfStream | `srcObject = mediaSource`; the records ride a journal file, not the bus (a bandy verb is owed — see SR134) |
| `buffered` / `seekable` | `TimeRanges` from the MediaSource, or [0, duration) for a file Stria opened | — |
| `<video>` VP9 | real frames from vp9_core (VP9CORE SR116); `canPlayType` "probably" | profiles 1 / 3 (vp9_core refuses them by name) |
| `canPlayType` | "probably" for codecs with a real decoder, "maybe" with no `codecs` parameter or a stand-in codec (H.264, HEVC), "" otherwise | — |
| playbackRate | 0.25–4: gneiss_pal's master clock runs at the rate (wall and audio clocks), frames are chosen by the rated clock, audio is time-stretched by WSOLA (`audio_core::stretch`, pitch kept); 0 holds as before | outside 1/16–16 (non-zero): NotSupportedError, Chromium's range; 1/16–0.25 and 4–16 play clamped into 0.25–4 |
| bare `autoplay` / `play()` | plays once the document has sticky user activation (HTML §6.4.1); muted always | without activation: `play()` rejects NotAllowedError and the element paints the play affordance; only a click on a media element (and tests) activates until the events.rs hook lands (SR134) |
| preload | `none`: an http(s) source is not fetched until play; `metadata` (default) as before; `auto`: `progress` then `suspend` once the whole resource is local | — |
| captions (`<track>`) | `kind` subtitles / captions / descriptions / chapters / metadata, `default` selection, WebVTT (`demux_core::vtt`), `TextTrack` / `TextTrackList` / `VTTCue` / `TextTrackCueList`, `addTextTrack`, `addCue` / `removeCue`, `cuechange` / `enter` / `exit`, `HTMLTrackElement.track` / `readyState`; the showing cues paint in the UA caption box (render/media_overlay.rs) | cue settings (line, position, size, align, vertical, regions) are stored, not laid out: every cue is a centred bottom box; `::cue` styling; in-band tracks |
| `requestVideoFrameCallback` | runs in the first animation frame after a new picture: presentedFrames, mediaTime, width, height | processingDuration 0, captureTime / receiveTime absent |
| Picture-in-Picture | — | `requestPictureInPicture` rejects NotSupportedError; `document.pictureInPictureEnabled` false: UnaOS has no floating media window |
| `HTMLMediaElement.captureStream` | — | throws NotSupportedError: no MediaStream in Aether |

Tests: handlers/aether/tests/media2.rs (`mse_appends_fragmented_mp4_and_the_journal_matches`,
`mse_type_gate_exceptions_and_append_error`, `can_play_type_reports_vp9_real_and_stand_ins_maybe`,
`webvtt_track_cues_cuechange_and_caption_box`, `autoplay_needs_sticky_activation`,
`preload_auto_fires_progress_then_suspend`, `rate_range_video_frame_callback_and_refusals`) and
handlers/stria/tests/media.rs (`media_source_feed_plays_appended_fmp4_waits_at_the_buffered_end_and_ends_at_end_of_stream`,
`webm_appended_in_chunks_parses_into_the_file_frames`, `coded_frame_processing_offset_sequence_and_remove`,
`playback_rate_scales_the_clock_and_the_frames`, `wsola_time_stretch_keeps_the_pitch`). The fragmented MP4 is
`demux_core::build::mp4` with `Mp4Options::fragment` (the builder already fragments), cut at its box boundaries; no
binary fixture.
