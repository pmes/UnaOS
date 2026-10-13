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
