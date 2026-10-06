# VPLAYAUDIO (rmbp-ledger B475) — a video's sound through the one Demuxer and the one Decoder

CHARTER: Stria — shared-core (`unaos/libs/media/demux_core` + `audio_core`, both rings, R79); the kernel stays the
fulfiller (`video/vplay.rs`, `video/player.rs`, `drivers/hda_play.rs`: existing files). No new kernel file, no knob
(rides `videoplayer`).

**Finding.** `vplay::container_audio` re-reads the file and opens a SECOND `Demuxer` beside the picture job's, plays
only Vorbis (a decoder adapter living in the kernel, `MkvVorbis`) and Opus in Matroska, refuses AAC/MP3 beside video
there (MP4 sound reaches play-dec through `audio_core::Decoder::open`, a third whole-file read), the Matroska sound has
no seek table (`Source::seek` = None → decode-skip) and `DiscardPadding` is ignored (the Opus tail plays its padding).
A video seek is owed in the Player except to 0.

**The seam.**
- demux_core: the file's bytes are an `Arc<Vec<u8>>`; `Demuxer::share()` = the same bytes and tables, its own cursor
  (no byte copy, one parse); `Demuxer::bytes()`, `Demuxer::sample_data(&Sample)` (a slice, no copy). The writer
  learns `MkvOptions::discard_last_ns` (the last audio block as a BlockGroup with `DiscardPadding`) for the KAT.
- audio_core::container: `open_demuxed(Demuxer)` is the ONE door for a demuxed sound — MP4 (AAC over the shared bytes;
  MP3 through a unit reader over them, no concatenated copy) and Matroska/WebM (Opus through `OpusPackets`, Vorbis
  through a new `MkvVorbis` beside `OggVorbis`); `plays(format, codec)` is the predicate `Facts.audio_ok` asks.
  Matroska seek = the track's own sample table (`table=matroska`): Opus restarts a fresh decoder 80 ms before the
  target (RFC 7845 §4.6 pre-roll) at a packet whose sample is its block time; Vorbis restarts one packet early (the
  overlap pre-roll) at the sample the packet-size walk gives (`Setup::packet_blocksize`, no decode) — exact.
  `DiscardPadding` trims the packet's output tail (Opus and Vorbis).
- kernel: the picture job keeps `d.share()` with its facts; `vplay::shared_audio(path)` hands play-dec that share (no
  I/O) BEFORE `Decoder::open`; `container_audio(path)` (a Matroska/MP4 opened without a picture job) reads once and
  takes the same `open_demuxed`. The kernel's `MkvVorbis`/`MkvPackets`/`xiph_split` go (no decoder adapter in Ring 0).
  `vplay::start_at(path, ms)`: the picture job keyframe-seeks (`Demuxer::seek`), decodes up to the target without
  queueing, the clock starts at the target; the Player's `vid::seek(ms)` seeks the picture and the sound together —
  the sound through `hda_play::seek_to` → play-dec → `Decoder::seek` → the container's table.

**Milestones.** M1 demux_core (shared bytes, `share`, `sample_data`, writer DiscardPadding). M2 audio_core
(`open_demuxed` for Matroska + MP4 over shared bytes, table seek, DiscardPadding, `plays`) + host KATs. M3 kernel
(vplay/hda_play/player) + the compile legs.

**Witness (the wire a metal boot prints).** Opening TEST.WEBM in the Player:
`[vplay] sound container=matroska codec=vorbis rate=<n> ch=<n> packets=<n> delay_ns=0 parse=shared -> play-dec`;
an MP4 with AAC: `[vplay] sound container=mp4 codec=aac … parse=shared -> play-dec`; a scrub in the video:
`[player] seek to_ms=<n> video=keyframe key_ms=<n> sound=coded` then play-dec's
`[play] seek to_ms=<n> landed_ms=<n> method=table table=<matroska|mp4> exact=1 byte=<n> sample=<n> jid=<n>`.
Host: `VPLAYAUDIO:` lines from `cargo test -p audio_core --test vplayaudio_kat -- --nocapture`.

**Owed.** FLAC/PCM inside Matroska/MP4 (audio_core owns the decoders; no codec mapping yet — `plays` says no, the facts
say `audio_ok=false`); Opus multistream; a video seek's frames between the keyframe and the target are decoded
unseen (no dropping by reference); the first metal line (DECJOBHANG must fly first).
