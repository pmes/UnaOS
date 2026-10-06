# SEEKTABLE (rmbp-ledger B433) — a coded file seeks from the container's own index

CHARTER: Stria — shared-core (`unaos/libs/media/audio_core`, the decoders both rings link; R79). No new kernel file.

**Finding.** PLAYER (B419) seeks a coded file by superseding the `play-dec` job and decoding from the top, dropping
frames (`[play] seek … method=decode-skip table=none`): a seek at minute 40 decodes 40 minutes. `audio_core::Read`
is forward-only, no `Source` can reposition, and `demux_core::Demuxer::seek` lands only on the first video track's
keyframes (track 0 when there is no video) — an audio track inside a movie has no seek of its own.

**The seam.** One seek in the shared core, from each container's own data, both rings:
- `io::Read::seek(off) -> bool` / `len()` (default: not seekable; `VecReader` and the kernel's `VfsSrc` are);
  `ByteStream::seek` repositions (inside the window, or through the source).
- `Source::seek(sample) -> Option<SeekPoint>`; `Decoder::seek(ms) -> Result<Option<SeekPoint>>`, `SeekPoint { byte,
  sample, exact, table }` — the decoder restarts at a sync point at or before the target and drops the residual
  (at most one frame of decode), so an `exact` seek lands on the target sample.
- Per format: **FLAC** the SEEKTABLE block (RFC 9639 §8.5), else a bisection over frame headers (their coded
  sample/frame numbers, CRC-8 checked) → `table=flac`, exact; **MP3** the Xing/Info TOC, the VBRI TOC (exact: its
  entries are frame counts), else a CBR estimate from the first frame → `table=xing|vbri|cbr`, Xing/CBR `exact=0`
  (within one frame of the estimate), with a two-frame pre-roll (bit reservoir, IMDCT overlap, synthesis window);
  **MP4/M4A AAC** the `stts`+`stsc`+`stco`/`co64`+`stsz` sample table (the same table `demux_core` builds; the KAT
  checks both land on the same sample offset) with a one-AU pre-roll → `table=mp4`, exact; **WAV/AIFF** the byte
  offset → `table=pcm`, exact. **demux_core**: `Demuxer::seek_track(track, ns)` — any track as the reference (audio
  samples are all sync samples); `seek` is `seek_track` on the video track, unchanged.
- Kernel: `VfsSrc` seeks; `play-dec` calls `dec.seek(ms)` once after the open instead of `seek_skip` (that stays
  the fallback when the core has no table: Ogg Opus/Vorbis, ADTS).

**Milestones.** M1 io seek + `SeekPoint` + PCM (WAV/AIFF). M2 FLAC (SEEKTABLE + bisection). M3 MP3 (Xing, VBRI, CBR).
M4 MP4 AAC + `demux_core::seek_track`. M5 host KATs over test-f + the FLAC subset. M6 kernel: `VfsSrc` seek,
`play-dec` table seek, `tests player` `method=table`.

**Witness (the wire a metal boot prints).** A coded seek from the Player:
`[play] seek to_ms=<n> landed_ms=<n> method=table table=<flac|xing|vbri|cbr|mp4|pcm> exact=<0|1> byte=<n> sample=<n> jid=<n>`;
a WAV seek `… method=table table=pcm exact=1 …`; `tests player` → `:: PLAYER: open=ok transport=ok seek=ok method=table
… -> PASS ::`. `tests player` does NOT seek a coded file (R80, and DECJOBHANG: a coded job still wedges on metal);
the coded witness is the Player scrubbing a FLAC/MP3/M4A by hand. `play::seek_result()` holds the job's verdict.

**Host proof (M5).** `cargo test --release -p audio_core -p demux_core` exit 0: WAV/AIF/synth bit-exact; TEST.FLAC
(no SEEKTABLE: header bisection) and 54 FLAC subset files (34 with SEEKTABLE) bit-exact at 4 targets each; TEST.MP3
`xing` (exact=0, within one frame), the same frames under a built VBRI tag (exact=1, PCM-identical) and untagged
(`cbr`, within one frame); TEST.M4A `mp4` exact=1, PCM-identical; demux_core `seek_track` on TEST.M4A pins the unit
whose pre-roll is audio_core's (byte 997 at 200 ms).

**Owed.** Ogg (Opus/Vorbis/Ogg-FLAC: a granule bisection over pages — falls back to decode-skip, said on the wire as
`table=none`), ADTS (no index; a header walk), MP3-in-MP4 seeks through the concatenated stream's CBR/Xing path;
DECSTALL/DECJOBHANG on metal fly first (B386/B396): a table seek is only seen once the coded job leaves demux.
