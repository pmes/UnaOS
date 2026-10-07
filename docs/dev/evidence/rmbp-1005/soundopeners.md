# SOUNDOPENERS (rmbp-ledger B500) — design

Cut from 668cdd95 (the merge19 fold + flight 27). Wire: `docs/dev/evidence/rmbp-0915/flight27/f27-boot1.log`.

## Finding (from the wire, then the tree)

1. **OPENERS 17/24 is the fixture's predicate, not the Player.** `[openers] TEST.WAV … handler=player
   core=demux_core=REFUSED`: `fs/filetype.rs::openers_witness` asks `demux_core::probe` of every file whose
   handler is `player`. The Player does not demux a sound: `video/player.rs` sends anything not `video/*` to
   its audio leg (`hda::play` → `audio_core::Decoder::open`, AUDIOCORE B396), which reads RIFF, AIFF, FLAC,
   Ogg (FLAC/Opus/Vorbis), MP3 and ADTS itself and MP4/M4A through `demux_core` (MP4ONE B464). The seven
   REFUSED (WAV FLAC OPUS OGG MP3 AAC — and AIF, the seventh of the 7 failures) are the predicate's error.
2. **`tests play`/`playwav`/`hda` ran=0: the registrations are unreachable on metal.** All eight
   (`hda hdaboth hda220 hda880 hda1 hda2 playwav play`) are one line inside `u8x_launcher`
   (`arch/x86_64/syscall.rs`), the tail of the U7x→U8x demo chain; flight 27 has no `:: U7x`/`:: U8x` line,
   so the chain never ran and the fixtures were never in the table. Nothing retired them: they fell off.
3. **`play TEST.M4A` was a path, not the demuxer.** The wire's next line: `:: PLAYWAV: path=/TEST.M4A
   reason=stat: NoSuchPath; audio_core: Invalid("vfs read") -> REFUSED ::` — cwd `/`, the sample lives in
   `/system/test-f`. `dec exit … stage=demux` is the stage the job was in when the VFS read failed.

## The seam (R79)

`audio_core::route(head)` — one predicate in the shared `no_std` core both rings link: `AudioCore(format)` for
what audio_core's own decoders read, `Demux` for ISO-BMFF/Matroska (through `demux_core`), `None` otherwise.
The OPENERS fixture, `play`'s route line and `tests soundopeners` all ask it; no second implementation.

## Milestones

- **M1** — `audio_core::route` + host test; the OPENERS predicate for `player` asks `demux_core` for `video/*`
  and `audio_core::route` for a sound (`core=audio_core=ok` / `core=demux_core=ok` for M4A).
- **M2** — `play <file>`: `[play] route type=<t> via=<audiocore|demux|riff> dec=<codec>` once the decoder opened
  (`riff` = the kernel's direct PCM WAV reader, which never meets a demuxer); a bare name that misses the cwd is
  looked up in system/test-f and said (`[play] resolve`); a missing file is refused before a job is spawned.
- **M3** — `kernel/src/soundopeners.rs`: the eight HDA/play fixtures registered behind the `tests` verb
  (idempotent: `tests::register` refuses a duplicate name, so the demo chain's line stays harmless), and
  `tests soundopeners`: each of the seven samples typed, its opener read, and OPENED by audio_core on a
  160 KiB task (the `play-dec` stack), never the shell's.

## Witness (the next flight reads)

    [openers] TEST.WAV type=audio/wav … handler=player core=audio_core=ok        (×7 sounds; M4A core=demux_core=ok)
    :: OPENERS: test_f=24 typed=24 unhandled=0 -> PASS :: opened=24 cores=24/24 …
    [play] route type=audio/flac via=audiocore dec=flac rate=… ch=…
    :: TESTS: run play ::  then  :: PLAYCODEC: … ::  per format
    [soundopeners] TEST.FLAC type=audio/flac opener=player handler=player via=audiocore dec=flac rate=44100 ch=2 -> opened
    :: SOUNDOPENERS: formats=wav,flac,opus,ogg,mp3,aac,m4a opened=7/7 via=audiocore,audiocore,audiocore,audiocore,audiocore,audiocore,demux -> PASS ::

## Owed

- The `u8x_launcher` line still registers the same eight (now deduplicated); the seat may drop it at a fold.
- The kernel's own RIFF reader (`hda_play::parse`) beside audio_core's WAV decoder is a second reader (R79);
  it is the PCM fast path and stays this arc — named `via=riff` on the wire, not hidden.
