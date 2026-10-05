# OPENERS (rmbp-ledger B379) — every test-f sample typed and routed

Cut from a60219de (hw-rmbp: flight 23 on image 16). Answers FLIGHT 23 "No opener for WEBP / BMP / M4A".

## Finding (re-read from the code, wider than the ledger row)
`fs/filetype.rs` sniffs PNG, WAV, ELF, gzip, tar, GIF and text only, and `EXT_TABLE` knows no image or audio
extension past `png`/`wav`/`gif`. So it is not three of the 24 test-f samples that read
`application/octet-stream`, it is FIFTEEN: TEST.FLAC .OPUS .OGG .MP3 .AAC .M4A .AIF, TEST.JPG, LOSSLESS/LOSSY/ANIM.WEBP,
TEST.BMP, TEST.QOI, TEST.WEBM, TEST.MP4. Flight 23 only clicked three of them. The decoders are all already in the
tree: `pixel_core` (JPEG, BMP, QOI, WebP lossless + lossy, animated through FACETANIM) behind facet, and `audio_core`
(FLAC, Ogg Opus/Vorbis, MP3, ADTS AAC, MP4/M4A AAC, AIFF) behind `play`. Facet and play choose their decoder by
magic, so the hole is only the type and the association. The ledger's (2) "a BMP decoder if none exists" and "verify
the m4a stsz/stco" are already there (`pixel_core/src/bmp.rs`, `audio_core/src/mp4.rs`); M1's host test proves them
on the sample bytes.

## The seam (R79, R83)
The magic-to-MIME knowledge lives in the cores that own the formats, not in a second sniff in the kernel:
- `pixel_core::mime_of(bytes)` — the still/animated image types, from `pixel_core::sniff`, with the BMP leg
  strengthened (BITMAPFILEHEADER reserved words zero and a DIB header size the spec defines: 12 16 40 52 56 64 108 124),
  because `BM` alone types any text file that starts "BM".
- `audio_core::mime_of(bytes)` — the audio types from `audio_core::sniff`; the frame-sync guesses (bare MP3 / ADTS /
  FLAC frame) are reported WEAK so a name the table types otherwise keeps its table type.
- `demux_core::mime_of(head)` + `demux_core::iso_kind(ftyp, moov)` — ISO-BMFF and Matroska: the `ftyp` brands
  (`M4A ` `M4B ` `M4P ` audio; `M4V ` `av01` `avc1` video), then, when the brand is general (`isom` `iso2..9` `mp41`
  `mp42`), the `moov/trak/mdia/hdlr` handler types (`soun` only = `audio/mp4`; any `vide` = `video/mp4`), from ISO/IEC
  14496-12 §8.4.3 and RFC 4337. The kernel walks the top-level boxes with 16-byte reads to find the `moov` (it may sit
  after the `mdat`), bounded.
The kernel's `filetype::sniff` calls the three cores; `audio_core` and `demux_core` become plain (non-optional) kernel
dependencies so a file's type never depends on a knob.

## Milestones
- M1 the cores: `mime_of` in pixel_core, audio_core, demux_core; host tests over the 18 fetched samples
  (`cargo test -p pixel_core -p audio_core -p demux_core`, sample-gated: absent samples SKIP loudly).
- M2 the kernel: `filetype` types (image/jpeg image/bmp image/webp image/qoi audio/flac audio/ogg audio/mpeg audio/aac
  audio/mp4 audio/aiff video/mp4 video/webm), the sniff through the cores, the extension rows; `assoc::BUILTIN` rows
  (images -> facet, audio -> play, video -> none: no video opener in this tree); Quarry's kind tokens.
- M3 the witness at `tests testf` (committed with M2: same file): one `[openers]` line per sample and
  `:: OPENERS: test_f=24 typed=24 unhandled=0 -> PASS :: opened=22 cores=22/22 owed=TEST.WEBM(video/webm),TEST.MP4(video/mp4) reason=no-opener-in-this-tree(video: Stria's player, SR26) dir=/system/test-f ::`

## Witness (what flight 24 reads, x86 metal shape)
- `tests testf` -> `:: TESTF: staged=24/24 …` then 24 lines like `[openers] TEST.M4A type=audio/mp4 src=sniffed opener=play(db) handler=play core=audio_core=ok` and
  `:: OPENERS: test_f=24 typed=24 unhandled=0 -> PASS :: …`.
- On the glass: double-click ANIM.WEBP -> `[quarry] … open kind=webp handler=facet`, facet animates; TEST.BMP ->
  `open kind=bmp handler=facet`, `[facet] open … pixel_core=127x64`; TEST.M4A -> `open kind=audio handler=play`,
  `[play] open path=/system/test-f/TEST.M4A format=mp4 codec=Aac …` (the existing play line; `format=` is the container).

## Owed
- A VIDEO opener (Stria's, PLAYBACK SR26): TEST.WEBM and TEST.MP4 are typed `video/webm` / `video/mp4` with opener
  `none`, said by name on the witness. `unhandled` counts only files with no type, or with an opener this build lacks.
- SVG stays `text/plain` (sniffed text) opening in the editor: the kernel links pixel_core without `svg`.
- MP3 playback itself is MP3HANG's arc, not this one.

## Built (a60219de -> M1 d2683b2c, M2+M3 21973414)
Host: `cargo test -p pixel_core -p audio_core -p demux_core --test mime_testf` rc=0 with all 18 fetched samples present
(TEST.M4A: AAC, 44100 Hz mono, 12701 frames decoded; 14 `stsz`/`stco` samples inside the file; TEST.BMP 127x64; ANIM.WEBP
animates). The full audio_core suite has THREE failures that predate this arc (decoders that PANIC on input:
`aac_fetched_streams` at `aac/decoder.rs:387` (usize underflow), `flac_mutations_never_panic` and
`lossless_mutations_never_panic` at `flac.rs:211` (index out of range)) — the kernel's `play` links these decoders.
