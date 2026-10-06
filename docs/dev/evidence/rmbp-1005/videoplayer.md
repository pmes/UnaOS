# VIDEOPLAYER (rmbp-ledger B434) — the Player plays moving pictures

## Finding (the wire, then the code)
- Flights 24/25: `:: OPENERS: … opened=22 cores=22/22 owed=TEST.WEBM,TEST.MP4 reason=no-opener-in-this-tree(video: Stria's player)`;
  QUARRY3's Quick Look owes the same two (22/24). `fs/assoc.rs` BUILTIN gives `video/webm`, `video/mp4`,
  `video/x-matroska` the opener `none`, and `seed_in` never rewrites an existing object: the card's UnaFS already
  carries `una:opener=none` for them, so a new BUILTIN row alone would never reach the bench.
- The cores are in the tree and decode the staged samples (host, this tree): TEST.WEBM (Chromium bear-320x240) =
  VP8 320x240 82 frames + Vorbis 44.1 kHz stereo 166 packets (CodecPrivate = Xiph-laced id/comment/setup);
  TEST.MP4 (bear-av1) = AV1 320x240 82 frames, no audio. vp8_core: 82/82 frames, av1_core: 82/82 frames,
  audio_core::vorbis over the Matroska packets: 121024 samples (2.744 s). The kernel links demux_core and
  (through pixel_core) vp8_core; av1_core is not linked.
- `hda_play` opens a coded file through `audio_core::Decoder::open` (sniff: WAV/AIFF/FLAC/Ogg/MP3/ADTS/MP4):
  a WebM is `unrecognised format`, so the Vorbis track of a WebM cannot reach the ring today.

## Seam
R79: the kernel is Stria's FULFILLER over the shared cores — demux_core (container), vp8_core / av1_core
(pictures), audio_core::vorbis (sound) — and no second decoder. New file `video/vplay.rs`
(`CHARTER: Stria — fulfiller`): the decode job, the frame queue, the clock arithmetic, the Matroska-audio
`audio_core::Source` adapter (packets → `VorbisDecoder`, the Matroska codec mapping's Xiph lacing), the Quick
Look poster. PLAYER's window (video/player.rs) gains the video surface; hda_play's `play-dec` takes the adapter
when `audio_core` refuses a container (one call). Knob: `UNAOS_VIDEO=1` → feature `videoplayer` (links
av1_core; x86 metal; the aarch64 arm compiles the dependency only).

## Milestones
- M1 knob + `video/vplay.rs`: `play-vdec` on a worker core (never cpu 0, never the caller's; 256 KiB stack,
  high-water on the wire), reads the file, demuxes, decodes every video packet in decode order (VP8 or AV1) to
  0RGB frames into a bounded queue (3 frames); the consumer (the Player's pass, at most one present per pass =
  per vblank) presents the newest frame whose pts the clock has reached and counts the ones it skipped as drops.
- M2 the Player's window: a video surface above the transport (the frame scaled nearest into the window's
  rect at the window's size), the info line `VP8  320x240  30 fps  0:02`, pause/play drives both the audio and
  the video clock; video seek is OWED (SEEKTABLE B433): the scrubber on a video says so; F7 restarts.
- M3 sound from the container: `play-dec` falls back to `vplay::container_audio` (Matroska Vorbis) when
  `audio_core` refuses the bytes; the clock is the audio ring's position while it runs (a/v sync by the audio
  clock), the wall clock (pause-aware) without audio (TEST.MP4) or after it ends.
- M4 openers: BUILTIN `video/*` → `player`; `seed_in` upgrades an existing `none` row of those three types once
  (`[assoc] upgrade type=… none->player`); `openers::available("player")`; OPENERS' core arm `demux_core`;
  Quick Look's card for video names codec/size/fps/duration and blits the first frame (the poster job).
- M5 `tests videoplayer` (R80: typed) on TEST.WEBM (then TEST.MP4 when staged).

## Witness (metal, `UNAOS_VIDEO=1` with the seat's x86 line)
```
[player] video codec=vp8 size=320x240 fps=30 path=/system/test-f/TEST.WEBM audio=vorbis
[vplay] job spawn jid=1 path=… stack=262144 cpu=<n> on=worker
[vplay] job exit jid=1 why=eos decoded=82 err=0 ms=<n> stack high=<n> of 262144
:: VIDEOPLAYER: open=ok frames=82 dropped=<n> fps=30 av_sync_ms=<n> clock=audio -> PASS ::
:: VIDEOPLAYER: open=ok frames=82 dropped=<n> fps=30 av_sync_ms=<n> clock=wall -> PASS :: (TEST.MP4, av1)
:: OPENERS: test_f=24 typed=24 unhandled=0 -> PASS :: opened=24 cores=24/24 owed=- …
```
and from Quarry: double-click TEST.WEBM → `[quarry] open VIDEO … -> player`, the bear moves with its sound.

## Owed
- Video keyframe seek (SEEKTABLE B433's container tables); hardware decode (no GPU path); Opus/AAC tracks inside
  WebM/MP4 video (Vorbis-in-Matroska only here; AAC-in-MP4 rides audio_core's own MP4 reader); scaling beyond
  nearest (the compositor's path); the first metal line.
