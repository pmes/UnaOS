# AETHERVIDEO (LEDGER SR39) — `<video>` in Aether, played by Stria

Branch `exec-host-aethervideo`, cut from `7c7fa62b`, with `exec-media-playback` (SR26, tip
`63582d93`) merged in first (`f78e219b`) because the bus verbs consumed here are PLAYBACK's.
Family host, 2026-10-05.

## Finding

PLAYBACK made Stria play a video track and answer on the bus (`MediaPoster`, `MediaPause`,
`MediaResume`, `MediaSeek`, `MediaStop`, `PlayMedia` in; `MediaOpened`, `MediaFrame`,
`MediaEnded`, `MediaError` out, keyed by url), but Aether never asked: a `<video>` laid out as an
ordinary inline box whose `<source>` children and fallback text rendered as page content, a click
staged a `PlayMedia` that the shell fired and nothing came back onto the page, and the handler
mode (`aether open`) fired `PlayMedia` for every media url on load — ignoring the autoplay
policy entirely.

## What the arc built

`handlers/aether/src/media/mod.rs` — the page side of the Aether-renders / Stria-plays split
(CODEX Amendment II). No new handler: playback stays Stria's; this is Aether's layout/paint lane.

| page event | out (Aether → Stria) | back | page effect |
|---|---|---|---|
| a `<video>` with a playable source registers (load, or script-inserted at relayout) | `MediaPoster` | `MediaOpened`, one `MediaFrame` | natural size → relayout; first frame painted |
| `autoplay muted` | `PlayMedia` | `MediaFrame`… | plays from load |
| bare `autoplay` | poster only (ledger `media-autoplay-blocked-unmuted`) | | Chromium's policy |
| click on the box / `media_play` | `PlayMedia` (`MediaSeek 0` first when ended) | `MediaFrame`… | each frame painted at its pts |
| click while playing / `media_pause` | `MediaPause` | | the last frame stays |
| `media_seek` | `MediaSeek` | the frame at the target | show-poster flag cleared |
| navigation away | `MediaStop` per open url | `MediaEnded` | |
| | | `MediaEnded` | state ended, last frame stays; play restarts from 0 |
| | | `MediaError` | the error text paints in the box |

Spec sections:

- **HTML §4.8.11.5 resource selection** — `src`, else the first `<source>` whose `type` is
  absent or playable (`media::can_play_type`: the containers demux_core parses; the codecs Stria
  opens, real or stand-in; anything else is `""` and the next `<source>` is tried).
- **HTML §4.8.12 / Chromium `LayoutVideo` intrinsic size** — the video's natural size once
  `MediaOpened` (or the first frame) says it, else the poster's, else 300×150; `width`/`height`
  attributes win, a single attribute or a CSS-sized axis keeps the intrinsic ratio
  (CSS 2 §10.3.2/§10.6.2 for replaced elements); `<audio controls>` is Chromium's 300×54 box,
  `<audio>` without `controls` is `display:none`; children never render (replaced element).
- **HTML show-poster flag** — the `poster` image paints while the flag is set (until play or a
  seek); then the frame on glass.
- **CSS Images 3 §5.5–5.6 `object-fit` / `object-position`** — fill / contain (the video
  default: letterboxed) / cover / none / scale-down, keywords and percentages; the picture is
  clipped to the content box and bilinear-sampled at pixel centres (exact at 1:1).
- **Autoplay** — Chromium's policy without a media-engagement index: muted autoplay runs, unmuted
  autoplay does not.
- **`controls`** — a 32 px translucent strip: play triangle / pause bars, `m:ss / m:ss`, a
  progress track. Not a copy of Chromium's control UI (no oracle claims it).

Wiring: `AetherEngine::{take_media_requests, on_media_message, media_elements, media_play,
media_pause, media_seek, box_of}`; a reply marks only the element's box as damage (a size
change relayouts). `vessels/aether-shell` now hosts Stria's `MediaService` on its bus and pumps
both directions; `aether open` (handler mode) does the same instead of the old fire-everything
loop. `take_pending_media` remains, now reading the same outbox.

Stria fix found on the way (`handlers/stria/src/media.rs`): a seek to a time INSIDE a frame's
interval discarded that frame and showed nothing until the next one was due; HTML shows the frame
whose interval holds the position. `Player` now holds the last pre-target frame and presents it
(with its own ordinal) when the next decoded frame starts after the target, or at end of stream.

## Oracle method

ORACLE_PLACEHOLDER

## KATs

KAT_PLACEHOLDER

## Third-party crates

None added to Aether, Stria or bandy. `tools/aethervideo-check` adds `image 0.25.10` (PNG
read/write for the comparison — a utility) and `tokio 1.53.2` (`sync`, the bus receiver type —
a utility), both the latest stable at the time of adding (R83). Chromium/Playwright are the
oracle's toolchain (including WebCodecs' VP9 encoder for the fixture), never linked.

## Ceiling and owed

CEILING_PLACEHOLDER
