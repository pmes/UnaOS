# AETHERMEDIA (SR107) — the element table

What each media element does after the arc, and what it refuses and why. Built on
exec-host-aethermedia from 98d9f842; compiled only (R107: `cargo check -p aether --tests` rc=0),
proof owed to the fold. Aether renders, Stria plays (CODEX Amendment II); Stria decodes through
gneiss_pal::dsp, which re-exports the owned cores.

| element / rule | plays | refused, and why |
|---|---|---|
| `<video>` WebM/MP4/MKV, VP8 | frames from vp8_core via Stria, painted at render/mod.rs:2098 (`media::paint_for`) | — |
| `<video>` AV1 | — | Stria builds gneiss_pal without `av1` (handlers/stria/Cargo.toml), so dsp/video.rs:112 is compiled out: the labelled stand-in plays. Owed: `features = ["av1"]` on Stria's gneiss_pal dep |
| `<video>` VP9 / H.264 / HEVC | the labelled stand-in (canPlayType says yes so the box is not empty) | no owned decoder |
| `<audio>` / audio tracks: Opus, Vorbis, AAC-LC, MP3, FLAC, PCM | PCM from audio_core via Stria's audio_track to the host sink | — |
| `src` / `<source type>` selection | first playable source | `video/ogg`, Theora, unknown codecs: skipped; none left → `error` at the last `<source>`, networkState NO_SOURCE |
| preload | `metadata` default (MediaPoster), `none` waits (`suspend`) | `auto` = `metadata` |
| autoplay | `autoplay muted` plays | bare `autoplay`: Chromium's policy (no media engagement index) |
| loop | seek 0 + play at MediaEnded | — |
| controls | Chromium-style strip / audio control (render/) | — |
| readyState / networkState and events | loadstart, suspend, durationchange, resize, loadedmetadata, loadeddata, canplay, canplaythrough, play, playing, timeupdate (250 ms), seeking, seeked, pause, ended, error, volumechange, ratechange | the JS getters in js/html.rs still compute their own readyState/networkState and `seeking`/`error`: rewiring them to media::* is owed (outside the brief's files) |
| currentTime / duration / seeking | target read at once while seeking | — |
| volume / muted | muted, and volume 0, as MediaMute | levels between 0 and 1: no volume verb on the bus (page-side, ledgered `media-volume-level-page-side`) |
| playbackRate | 0 pauses the wire, back resumes | other rates: no rate verb (page-side, ledgered `media-playbackrate-page-side`); the JS setter is owed in js/html.rs |
| captions (`<track>`) | — | off by the brief |
| `<img srcset sizes>` | HTML parser + sizes, fetch and paint agree (800 px selection viewport) | — |
| `<picture>` | `images::img_source` | not yet called: net/mod.rs:535, layout/mod.rs:129, render/mod.rs:2035 still call `effective_img_src(&attrs)` (outside the brief's files) |
| `loading=lazy` | `is_lazy` / `lazy_should_load` (1250 px margin) | not yet called: the fetch side loads every image at page load (net/mod.rs:535) |
| AVIF in `<img>` | — | Aether does not link av1_core (handlers/aether/Cargo.toml); `av1_core::image::decode_avif` exists. `<picture>` falls through AVIF sources |
| WebP in `<img>` | pixel_core (lossless, lossy via vp8_core, animated first frame), already wired | — |

Tests: handlers/aether/tests/media_element.rs (five tests, no binary fixture).
