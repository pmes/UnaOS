# FILETYPE — files have a type, types have an opener, Quarry opens by type (B307; ticks B293)

## Design

**Finding (audit §2, B293).** No file type exists anywhere. Quarry opened by extension through
hard-coded if-chains (`activate_row`, `open_kind`, `open_handler`, `fileview::is_text_name`), and
two of those chains disagreed (`.sha/.cfg/.ini` were "text" to `open_kind` and "no opener" to the
router). A dotless file was text by fiat. BeFS had `BEOS:TYPE`, a MIME database type → preferred
application overridable per file, and content sniffing; Tracker opened by type.

**Seam.** Type and association are **fs-core** (`fs/filetype.rs`, `fs/assoc.rs`, CHARTER
`Kernel — fs-core`): they are attributes on the volume through the ATTRSURF surface
(`MountTable::{get,set,list}_attr`), not a store of their own. Routing a file to an opener is
**Matrix — kernel-by-ruling R50** (`video/quarry/openers.rs`): Quarry is the Finder by ruling.
Every attribute call has an honest FAT fallback (the one static table), and every answer says
which source decided.

**Milestones.**
- **M1 `fs/filetype.rs`.** `una:type` (a MIME string). `type_of(path) -> (Mime, Source)` with
  Source ∈ {Attribute, Sniffed, Extension, Unknown}: the attribute, else a magic-byte sniff of the
  first 512 bytes (PNG, `RIFF....WAVE`, ELF with the UnaOS/Linux split the loaders make — UnaOS
  images link at vaddr 0 and are biased into a slot window, `linuxabi::elf` maps static Linux
  images at their fixed vaddrs >= 0x10000 — gzip, `ustar`, then a UTF-8/ASCII text heuristic),
  else ONE static extension table, else `application/octet-stream`. `stamp(path)` /
  `stamp_as(path, mime)` write `una:type` or print `[filetype] stamp=skip reason=enotsup`.
  Writers that stamp: Print Screen / region shots (image/png, at the SHOTMOUNT verdict), the text
  editor's save (text/plain), `cp` (copies `una:type` explicitly). `mv` and Trash use
  `MountTable::rename`, which keeps attributes by inode on UnaFS. Verb `file <path>`.
- **M2 `fs/assoc.rs`.** The type database as attributes on `/system/types/<mime, / as .>`
  (`una:opener`, `una:icon`, `una:name`), seeded once per boot when the root takes attributes and
  the object is absent (never overwriting an operator's edit); per-file `una:preferred` wins; on FAT
  the same defaults are the builtin table and the source says `builtin`. Verb
  `assoc [<mime> [<opener>]]`.
- **M3 Quarry opens by type.** `activate_row` = `type_of` → `opener_for` → an `Act::Open`; the
  if-chain is deleted; `open_kind`/`open_handler`/the no-opener sentence derive from the same two
  calls; `openers::open(id, path)` is the ONE dispatch (facet, fileview, textedit, play, launch,
  linux, or a ring-3 program path). Dotless files are decided by the sniff.
- **M4 `tests filetype`.** Witness:
  `:: FILETYPE: typed=<n> sniffed=<n> ext=<n> assoc=<src> override=<ok> quarry=<ok> -> PASS ::`.

**Owed.** A ring-3 program opener cannot be handed an argv (`quarry.md` §7 item 8, no `SYS_EXEC`
with argv): the path is delivered as a `BUS_VERB_NOTICE` frame in the spawned program's mailbox
on x86 and said so on the wire; aarch64 launches without it and says so. The `linux` opener refuses
from the desktop (the Linux ABI runs a foreground session at the shell) and names the verb.
App signatures (`una:signature`) and the dock pinning by them are not in this arc.
