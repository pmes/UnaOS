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

`tools/aethervideo-check` (`tests/oracle.rs` runs it under `cargo test -p aethervideo-check`;
`oracle/run.sh <dir> [N]` by hand). Chromium cannot decode Stria's `utp1` test-pattern codec, and
Stria has no VP9/AV1 decoder yet, so the fixture carries the SAME frames twice:

1. `make` — `pattern-utp.webm` (demux_core's Matroska writer, `utp1`, 10 frames at 10 fps,
   320×240, every frame a keyframe) and `frames.rgba` (the same `TestPattern::render` frames raw).
2. `oracle/encode.cjs` — Chromium's WebCodecs VP9 encoder, profile 0, quantizer 0 (VP9
   lossless, but in YUV 4:2:0), every frame a keyframe → `chunks.bin` (20 KB);
   `mux` → `pattern-vp9.webm` with demux_core's writer (Chromium plays our container).
3. `oracle/page.html`, one page for both browsers: (1) a 320×240 `<video>` whose `<source>`s are
   `utp1` then `vp9` — Aether's resource selection takes the `utp1` (Stria decodes it for real),
   Chromium's `canPlayType` refuses it and takes the VP9; (2) the same pair in a 300×100 box on a
   grey background (object-fit contain → 133×100 letterboxed); (3) `src="pattern-vp9.webm"` in
   both — Chromium decodes, Stria paints its labelled stand-in at the frame's pts.
4. `oracle/shot.cjs` — Chromium at 640×500 from `file://`, every video seeked to the MIDDLE of
   frame N ((N + 0.5) / fps), `seeked` + a presented frame, screenshot.
5. `render` — Aether headless with Stria's real `MediaService` on a bandy bus: load, pump
   requests/replies until every poster landed, `media_seek` to the same mid-frame target, wait
   for the frame, render, PNG.
6. `compare` — per box: PSNR, max channel difference, % pixels within 8, % exact, and the
   counter read back from both PNGs (`TestPattern::read_counter`, exact glyph match).

| frame N | box | counter Aether / Chromium | PSNR | within 8 | exact |
|---|---|---|---|---|---|
| 0, 7, 9 | (1) 320×240 1:1, utp1 vs VP9 | 0/0, 7/7, 9/9 | 29.06 dB | 98.75 % | 52.08 % |
| 0, 7, 9 | (2) 300×100 contain | (scaled: not read) | 21.27–21.53 dB | 96.36–96.48 % | 76.1–76.2 % |
| 0, 7, 9 | (3) 320×240, same VP9 file | 0/0, 7/7, 9/9 | 29.06 dB | 98.75 % | 52.08 % |
| 7 | whole 640×500 viewport | | 28.99 dB | 99.07 % | 74.77 % |

Box geometry agrees exactly (Chromium `getBoundingClientRect` = Aether `box_of`: 0,0,320,240 /
330,0,300,100 / 0,250,320,240). The diff image of box (1) is black except three one-pixel
columns at the bar edges that fall on odd x (182, 228, 274: 4:2:0 chroma straddles them) and a
uniform 1–3 level offset inside three bars (BT.601 YUV rounding) — Chromium's VP9 round trip, not
Aether: the digits are exact in both, and Aether's own box holds Stria's frame byte-for-byte (unit
test `layout_fires_media_poster_and_paints_the_first_frame`). Box (2)'s residual is the scaling
filter (Aether: bilinear at pixel centres; Chromium: its own) along bar edges and glyph strokes;
the letterbox bars and picture rectangle coincide. Go-red: Aether rendered at frame 6 against
Chromium's frame 7 → `counter_a 6, counter_b 7, ok:false`, exit 1.

What this proves: Aether's resource selection, intrinsic sizing, MediaPoster → MediaSeek → frame
round trip over the real bus with the real Stria, the paint at the right pts (mid-frame seek
included — the Stria fix), object-fit contain geometry, and that it agrees with Chromium on the
same page. What it does not prove: decoding of a real codec (VP9/AV1 frames come from Chromium
only; Stria's side of box (3) is the stand-in, which for a test-pattern file draws the true
picture by construction).

## KATs

`cargo test -p aether --lib media::` — 12, against a FAKE Stria thread answering on a real
bandy `Synapse` with `TestPattern` frames:

- `layout_fires_media_poster_and_paints_the_first_frame` — exactly one `MediaPoster` on load,
  300×150 before the reply, 160×120 after `MediaOpened`, box == frame 0 byte-for-byte, no play.
- `poster_attribute_wins_until_playback` — data: PNG poster painted (contain-letterboxed), frames
  after play.
- `play_paints_each_frame_then_ended_keeps_the_last` — click → `PlayMedia`; counter in the box
  == pts/frame for every `MediaFrame`; `MediaEnded` keeps the last frame; replay = `MediaSeek 0` +
  `PlayMedia`.
- `click_toggles_pause_and_seek_paints_the_target` — second click → `MediaPause`; seek → frame 2.
- `media_error_text_paints_in_the_box`, `autoplay_policy_muted_runs_bare_does_not`,
  `source_selection_skips_types_stria_cannot_play` (and fallback content has no box),
  `video_with_source_children_keeps_its_attribute_size` (the oracle's find),
  `navigation_stops_the_previous_pages_sessions`, `object_fit_geometry` (all five keywords +
  position), `object_fit_cover_paints_clipped_to_the_box` (exact centre rows, nothing spills),
  `controls_strip_paints_play_then_pause` (triangle; two 4×14 bars; frame above untouched).

`cargo test -p stria --test media` — new `seek_inside_a_frame_shows_the_covering_frame` (five
targets incl. past the end; go-red: without the fix the first seek shows nothing).
`cargo test -p aethervideo-check` — the oracle above, frames 0, 7, 9.

## Third-party crates

None added to Aether, Stria or bandy. `tools/aethervideo-check` adds `image 0.25.10` (PNG
read/write for the comparison — a utility) and `tokio 1.53.2` (`sync`, the bus receiver type —
a utility), both the latest stable at the time of adding (R83). Chromium/Playwright are the
oracle's toolchain (including WebCodecs' VP9 encoder for the fixture), never linked.

## Ceiling and owed

- Urls: Stria opens local files only, so an `http(s)` page's `<video>` gets `MediaError` ("only
  local files") painted in its box. Owed: Aether fetches the media into a cache file and hands
  Stria the path (PLAYBACK's stated ceiling; Aether's net lane).
- `<audio>`: Stria refuses audio-only files ("no video track"), so `<audio>` sends no poster and a
  play answers with an error in its controls box. Owed to Stria: audio-only sessions.
- JS: `HTMLMediaElement` (`play()`, `pause()`, `currentTime`, `paused`, events) is not wired to
  this module (`api/video.rs` is still an inert factory). Owed.
- `controls` is our own strip (no Chromium pixel parity claimed); seeking from the strip, volume,
  fullscreen are absent; `loop`, `preload`, `playbackRate`, `<track>` not handled.
- Scaling filter differs from Chromium's (box (2) at 96 % within 8); posters on `file://` pages
  only via data: URIs (the page image fetcher is http-only; `poster` urls are not yet added to it).
- A seek within half a display period before a frame's start presents that next frame (Stria's
  scheduler early window).
- The oracle's VP9 side is Chromium's decode only; when AVCODEC lands a VP9/AV1 decoder in Stria,
  box (3) becomes a real decode-vs-decode comparison with no change to the harness.
- EYES (SR23, branch `exec-aether-see`, not on this branch): its comparator scores whole pages
  (SSIM/mismatch); this arc's per-box comparator lives in `aethervideo-check`. At the fold the
  page and `render` verb can become an EYES suite case (`cmd` subject, `chromium` oracle).
