# MP3HANG (rmbp-ledger B373) — flight 23's `tests play mp3`: the decoder ran the render task off its stack

## Finding (read, then measured on the host and on the kernel target — no QEMU)
- The wire (f23-boots.log 3206..3208): `:: [midden] cmd="tests play mp3"` → `:: TESTS: run play ::` → one `[wc-h]`
  rollup → nothing. No `[play] arm`, no `[play] open`: the machine died INSIDE `audio_core::Decoder::open`, before
  `start()` (the FLAC/OPUS runs print `[play] arm` from `start()`, which `open_coded` calls only after `open`).
- Where it ran: `tests` dispatches through `handle_key` → `shell::dispatch_command` on the x86 **render task**
  (`main.rs` `x86_render_service`, spawned with `RENDER_PATH_STACK_SIZE` = 32 KiB; RENDSTACK measured its own
  high-water at 15600). The render task is the shell's key drain and the console/serial-shell sink — the two
  things that died; the compositor and the EHCI interrupt path are other contexts — the two things that lived.
- The bytes: TEST.MP3 = Chromium `sfx.mp3` (sha256 pin 46364ebc…, 2189 bytes): ID3v2.3 (22-byte body, TENC
  "Amadeus Pro"), MPEG-1 Layer III 44.1 kHz **mono** 128 kb/s CBR, a Xing tag (flags 15: frames=12) + LAME
  gapless (11025 frames out). The host decodes it clean (`audio-check`: 11025 frames, 236x realtime) — the
  framing is not the fault, and no loop in the frame walk is unbounded (`sync` caps at 1 MiB, the stream EOFs).
- The fault is STACK, measured with `-Z emit-stack-sizes` on `x86_64-unaos.json` (release, soft-float):
  `Decoder::open` frame **92504 B** (every format's constructor inlined into one frame), `Mp3Stream::new`
  **32872 B** (an on-stack 16 KiB `Mp3Decoder`: `overlap` 2x576 f32 + `v` 2x1024 f32 + tables), and
  `Mp3Decoder::decode_frame` **16488 B** (on-stack `xr`/`sub`/`e` scratch). The target has inline stack
  probes: the 92.5 KiB prologue writes a zero qword every 4 KiB down through the 4 KiB poisoned guard and ~60
  KiB of the heap blocks below the 32 KiB render stack (x86 kernel stacks are plain heap allocations — the
  RENDSTACK note), and the MP3 arm then writes its 16 KiB decoder and frames INTO that region. FLAC/Opus took the
  same 92.5 KiB prologue (sparse zero qwords — survived by luck); MP3 is the first arm that fills the frame with
  data. Silent heap corruption under the render task: no panic, no exception, the render task's sink and key
  drain gone, everything not on it alive — exactly the glass.

## The seam
- `audio_core` is the shared `no_std` core both rings link (the midden_core shape; AUDIOCODEC SR30): the frame
  sizes are fixed THERE, once, for Ring 0 and Ring 3 alike (CHARTER: shared-core). Large decoder state lives on
  the heap; `Decoder::open`'s arms are outlined so its frame is the largest arm, not the sum.
- The kernel's `play` stops decoding on whatever task calls it: decode runs on its own `play-dec` kernel task
  with a right-sized stack (RENDSTACK's rule: a deep path takes a sized stack of its own via `spawn_stack`,
  with the measurement), feeding a bounded PCM queue the existing service tick drains into `feed()`. The
  service tick (render/shell side) never calls into a decoder again; it watches the decoder's heartbeat and
  names a stall on the wire, then aborts the play — the shell, its keys and its sink never wait on a codec.

## Milestones
- M1 (audio_core, shared-core): `Mp3Decoder` state + per-frame scratch on the heap, `Mp3Stream.dec` boxed,
  `Decoder::open` arms `#[inline(never)]`; re-measure the kernel-target frames; host `cargo test -p audio_core`.
- M2 (kernel `drivers/hda_play.rs` tail): the `play-dec` task (sized stack, measured), the bounded PCM queue,
  the stage/frame heartbeat, the watchdog (`[play] mp3 stall stage=<demux/frame/synth/dma> frame=<n> ms=<n>`,
  abort after a bounded silence), `[play] open … codec=Mp3 …` before the first decode, `[play] mp3 frames=<n>`
  every N frames, and the MP3GUARD verdict in `tests play`.

## Witness (the next flight reads; all under `hda-tone`, which is in the metal line)
- `[play] dec spawn path=/system/test-f/TEST.MP3 stack=<n>` then `[play] open path=… codec=Mp3 rate=44100 …`
- `[play] mp3 frames=<n> out=<n> ms=<n>` (every 32 MPEG frames, and the last)
- `:: PLAYCODEC: path=/system/test-f/TEST.MP3 format=mp3 codec=Mp3 rate=44100 frames=11025 … -> PASS ::`
- `:: MP3GUARD: stall=none keys_alive=1 sink_alive=1 -> PASS :: …` (or `stall=<stage>` → FAIL with the stall line)

## Owed after this arc
- Realtime soft-float MP3/AAC/Vorbis on metal for a LONG file (sfx.mp3 is 0.25 s); the decode task yields per
  block, so a slow decode underruns rather than wedges — the underrun count is on the PLAYCODEC line.
- The render task still runs every other `tests` verb on 32 KiB; a stack-room assertion at `dispatch_command`
  is the general cure (out of this arc's files).
