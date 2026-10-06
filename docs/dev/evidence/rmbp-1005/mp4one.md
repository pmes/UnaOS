# MP4ONE (rmbp-ledger B464) — one MP4 parser

**Finding.** `unaos/libs/media/audio_core/src/mp4.rs` (294 lines) was a second ISOBMFF reader beside
`demux_core::mp4`: its own box walk, `esds`, `stsc`/`stco`/`stsz` expansion, `trun` walk, `elst` and
`iTunSMPB`. SEEKTABLE (B433) had to pin the two against each other (seek_kat's byte 997 vs seek_track's
unit); VIDEOPLAYER (B434) found AAC/MP3 beside a video track unproven. R79: one parser.

**Seam (the seat's ruling).** A core depending on a core: `audio_core` takes `demux_core` as a path
dependency (both `no_std` + alloc, `forbid(unsafe_code)`, zero other deps). `audio_core::open_arm::mp4`
calls `demux_core::Demuxer` for the sample table of the first AAC/MP3 audio track (any video track beside
it is ignored) and hands `(offset, size)` units to the unchanged `AacSource`/`Mp3Stream`. The gapless trim
needs the file's raw numbers (sample units, not ns, so the decode is byte-identical): demux_core's `Track`
gains `Track::edit` (the media edit's `media_time`, segment duration, movie timescale) and `smpb`
(iTunSMPB priming/total), set by its one parser; the ns fields stay derived from them. A truncated file
(cut `mdat`) still plays to its cut: `Demuxer::open_partial` clamps a top-level box that runs past the end
and drops samples past it, as the old reader did. `audio_core::mp4` is deleted.

**Milestones.**
- M1 — the KAT pinned on the cut tip, before any change: `tests/mp4one_kat.rs` fingerprints m01/m04/m07/m10
  and TEST.M4A (full decode FNV over f32 bits, frame count, four seeks: byte/sample/landed/PCM-after).
- M2 — demux_core: `Track::edit`, `Track::smpb`, `wave`-wrapped `esds` (QuickTime), `Demuxer::open_partial`.
- M3 — audio_core over demux_core; `src/mp4.rs` deleted; the KAT byte-identical; AAC and MP3 beside a
  video track decode identically to the audio alone (built with `demux_core::build`).
- M4 — kernel legs (x86 metal shape, both aarch64 legs) over the new dependency.

**Witness.** Host: `MP4ONE: 5/5 byte-identical` (`cargo test --release -p audio_core --test mp4one_kat --
--nocapture`). Metal: no new line — the kernel's `play TEST.M4A` keeps its `[play]` lines and SEEKTABLE's
`[play] seek … table=mp4` line; their numbers equal flight 25's (the host KAT proves the bytes).

**Owed.** VIDEOPLAYER (B434, exec-rmbp-videoplayer) is not on this tip: `vplay::container_audio` stays
Matroska Vorbis there and MP4 sound reaches play-dec through `audio_core::Decoder` (now demux_core) — the
fold keeps both; demux_core's dev-dependency on audio_core there is a dev-cycle Cargo accepts. TEST.MP4 has
no audio track (video only, av01), so "TEST.MP4's audio track" is the built AAC+video file. The ARCH
baseline holds no MP4 key on this tip (`parser|…|isobmff` is fs/filetype.rs, OPENERS' sniff), so no row
leaves it here.

**Legs (M4).** `cargo test --release -p audio_core -p demux_core` exit 0 (22 suites; `MP4ONE: 5/5 byte-identical`,
AAC beside video 12701 frames identical progressive and fragmented, MP3 beside video 11025 frames identical for
`.mp3` and `mp4a`/0x6B; seek_track's byte 997 holds); `cargo check -p gneiss_pal` 0, `-p audio-check` 0; kernel
x86 metal shape 0, aarch64 `login,loginst,virt_el0,lumen,desktop_firmware,quarry,facet,usbnet` 0, aarch64
`tegra,login,loginst,virt_el0` 0; charter-check 0; arch-check 0 (28 baseline keys, unchanged).
