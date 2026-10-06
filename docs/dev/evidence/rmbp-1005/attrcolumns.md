# ATTRCOLUMNS (rmbp-ledger B402) — the Be inheritance B2: typed facts as attributes, columns, edit in place, Get Info

Cut from 1a035683 (exec-rmbp-merge17). MACPARITY §16 B2 (and the seed of B6 FOLDERVIEW), GETINFO row 29 first form.

## Finding (read from the code)
- The sniffers learn the facts and throw them away: `pixel_core::mime_of`, `audio_core::mime_of`, `demux_core::mime_of`
  answer a MIME string only; `fs/filetype.rs` stamps `una:type` and nothing else.
- UnaFS already carries typed attributes (`set_attribute` -> inode + the eq/ord B+tree index in one call) and the VFS
  surface (`MountTable::{set,get,list}_attr`), but every multi-key writer flips the root once per key.
- Quarry's list (QUARRY2 `columns.rs`) has five fixed columns; the right-click menu is the file menu everywhere,
  including the header; `Show Info` is a one-line NOTICE (size, owner); there is no attribute view and no Cmd-I.
- The index cannot answer "which keys exist in this folder": the eq index is keyed by a HASH of the key name. The
  folder's key set is read from the listing's own inodes (`list_attrs`, bounded), not from the index.

## The seam (R79)
- **Facts: shared-core.** `pixel_core::facts_of` (width, height, animated — header walk: PNG IHDR/acTL, JPEG SOFn,
  GIF block walk, BMP DIB, QOI, WebP VP8/VP8L/VP8X, SVG root attributes), `audio_core::facts_of` (duration, codec,
  title — WAV fmt/data, AIFF COMM, FLAC STREAMINFO + VORBIS_COMMENT, Ogg Opus/Vorbis last granule + comment header,
  MP3 ID3v2 TIT2 + Xing/VBRI or CBR, ADTS frame walk), `demux_core::facts_of` (MP4/M4A/WebM through `Demuxer::open`).
  Both rings link the same functions; the kernel adds no second parser.
- **The write: Kernel — fs-core.** New `fs/attrfacts.rs`: on Quarry's open and on Quarry's service pass for a listed
  folder (bounded per pass, never at boot — R80), the facts are written ONCE with `una:type`, `una:facts-mtime` (the
  file mtime they were taken at: a changed mtime refreshes them) and `una:attrtimes` (key -> unix seconds, the
  "time it changed" Get Info shows) in ONE transaction: a new `VfsBackend::set_attrs` (UnaFS: autocommit off, the
  keys, one commit — the K9 batch shape). FAT answers `Unsupported` and gets nothing.
- **Columns, edit, Get Info: Matrix — kernel-by-ruling R50** (Quarry is the Finder). New `video/quarry/attrcols.rs`
  (the attribute columns: header right-click `Add column…` menu, typed cells, sort, inline edit, the `una:view`
  folder attribute) and `video/quarry/getinfo.rs` (the inspector). Hooks in `columns.rs`/`live.rs`/`ops.rs` only.
- Keys: `una:` the system's (`una:type`, `una:view`, `una:facts-mtime`, `una:attrtimes` — APPRES's `una:icon` shares
  the prefix); `media:width` `media:height` `media:duration_ms` (int) `media:codec` (string), `doc:title` (string),
  `image:animated` (int 0/1 — AttrValue has no bool).

## Milestones
- M1 the cores' `facts_of` + host tests over the test-f samples, cross-checked against each core's own decoder.
- M2 the kernel write: `VfsBackend::set_attrs` (one transaction), `fs/attrfacts.rs` (refresh, edit, view, info rows),
  the open hook.
- M3 Quarry: attribute columns (Add column…, typed cells, ints right-aligned, durations m:ss, sort, `una:view`),
  the service-pass refresh of a listed folder.
- M4 inline edit (click the selected row's cell, type, Return; `doc:title`, `una:type`) and Get Info (menu row,
  `i` key, Cmd-I as `Action::GetInfo`).
- M5 `tests attrcolumns` and the witness.

## Witness (the next flight)
`tests attrcolumns` ->
`[attrcolumns] TEST.FLAC facts=media:duration_ms=…,media:codec=flac …` (one per sample), then
`:: ATTRCOLUMNS: typed_facts=21/24 columns_added=2 inline_edit=ok view_saved=1 getinfo=ok -> PASS :: nofacts=TEST.TXT,TEST.JSON,TEST.CSV(plain text names no fact) dir=/system/test-f ::`
On the glass: `[attrcols] view dir=… cols=…`, `[attrcols] edit path=… key=doc:title ok=1`, `[getinfo] path=… attrs=<n>`.

## Owed
- Sort by an attribute column reads the listing's values (bounded), not an ordered index scan: the index is
  volume-wide and the list is one folder (design question for the seat).
- FAT volumes get no facts (UNAFS.ATR sidecar is the bridge, out of scope).
- `doc:title` from plain text/JSON/CSV: none (no title in the format).
