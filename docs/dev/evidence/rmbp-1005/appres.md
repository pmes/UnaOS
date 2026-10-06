# APPRES — a program carries its own name, signature, version, kind, icon and document types (B398)

## Design (written before the code)

**Finding (B398, MACPARITY §16 B4 and row 38).** A program today is an `.ELF` with EXECNAME's launch note
(`.note.unaos.app`, type 1, one u32 of flags) and nothing else: the dock tile is a caption (no glyph at
all — `video/dock.rs::compose_row` draws text only), Quarry's list row is a name with `ls -F`'s `*`,
the app menu's About arm prints one serial line (`[winmenu] app-menu about win= name=`), and no version
exists anywhere. `/apps` is the FAT boot volume on metal (`[vfs] apps mount /apps = fat boot volume`,
f25), so a program's own inode takes no attributes there today; the type database
(`/system/types/<mime>`, `fs/assoc.rs`) lives on the UnaFS root and does.

**Seam — Midden `shared-core` for the format, Kernel `fs-core` for the registrar.**
1. **The block** is a second UnaOS note: owner `"UnaOS"`, type 2 (`APP_RES_NOTE_TYPE`, in `una_abi` and
   `midden_core`), in a NON-ALLOC `SHT_NOTE` section `.note.unaos.res` with no program header — the loader
   maps `PT_LOAD` only, so it never loads it; the kernel reads it through the section table without loading
   the program. Desc = `b"UNARES\0\x01"`, a u32 count, then `(u16 key_len, u32 val_len, key, value)` records.
   Keys: `una:name`, `una:signature` (reverse-DNS, `org.unaos.<app>`), `una:version` (`semver+build`),
   `una:kind` (`windowed`/`resident`/`console`, today's note in words), `una:icon.svg` (the source),
   `una:icon.32` / `una:icon.64` / `una:icon.128` (RGBA PNGs rendered from the SVG by the host tool, so
   the kernel draws an icon with pixel_core's PNG decoder whether or not SVGCORE's kernel feature is on),
   `una:doctypes` (newline-separated MIME types). The parse and the builder live in `midden_core` (both
   rings and the host tool link it; `cargo test -p midden_core` proves the round trip and the malformed cases).
2. **The writer** is `tools/una-res` (host): `pack <res-dir> <out.unares>` and `stamp <res-dir> <elf>`
   (appends the section, a new `.shstrtab` and a new section-header table at the file's end, patches
   `e_shoff`/`e_shnum`/`e_shstrndx`; refuses an ELF already stamped). `res/app.res` is `key = value` lines;
   `res/icon.svg` is rendered with `svg_core` (std) and encoded with the `png` crate. arroyo's
   `una_res_stamp` runs it after the strip for every program crate with a `res/` dir (Lumen today).
3. **The registrar** is `fs/appres.rs` (new, `CHARTER: Kernel — fs-core`). At first sight of a program —
   its launch (`bare_exec` already holds the bytes) or a Quarry listing of its directory (three small
   reads: ELF header, section table, the note) — it reads the block and caches it as ATTRIBUTES: on the
   program's own inode when its volume takes attributes, and on the program's SIGNATURE object in the type
   database, `/system/types/application.x-vnd.una.<app>` (Be's registrar kept exactly this: an app's
   signature as a MIME-database entry carrying its icons and hint path), with `una:app.path` and
   `una:app.stamp` (mtime + size) so a changed ELF is re-read. Quarry and the dock read the attributes
   (decoded once into a RAM pixel cache keyed by stamp), never the ELF. The kernel's own windowed apps
   (quarry, settings, console, shell, facet) are not ELFs: their blocks are the same format, packed by
   `una-res pack` from `unaos/res/<app>/` and compiled in, so one reader serves both.
4. **The surfaces.** The dock tile draws the app's 32/64/128 icon centred (the caption is the fallback
   for a tile whose window names no known program); Quarry draws the icon after the program's name; About
   <app> prints name, version and signature and shows them in a notice. A program without a block gets the
   generic icon and its file name.
5. **Doc types.** The registrar writes the app's path into `una:apps` on each `/system/types/<mime>` it
   declares; `assoc::opener_for_in` reads it after the database's own `una:opener` and before the builtin
   table.

**Milestones.** M1 FORMAT — midden_core parse/build + una_abi constant + host tests. M2 WRITER —
`tools/una-res` (pack/stamp, round-trip test) + the six icons + arroyo hook. M3 REGISTRAR — `fs/appres.rs`
(sight, cache, attrs, refresh) + the launch and Quarry hooks + doctypes. M4 SURFACES — dock tile, Quarry
row, About. M5 WITNESS — `tests appres`.

**Witness.** `tests appres` →
`:: APPRES: <app> sig=<signature> version=<v> kind=<k> icon=<px>|generic source=<builtin|elf|attrs> ::`
per program, then `:: APPRES: programs=<n> with_res=<n> icons_drawn=<n> cached_attrs=<n> -> PASS ::`
(PASS = the five built-ins and LUMEN.ELF carry a block, every icon decodes and draws into a scratch tile,
and on a root that takes attributes every carried block is cached). Unprompted wire: `[appres] sight
path=<p> res=<yes|no> attrs=<n> on=<inode|types|none>` at first sight, `[appres] about app=<a> name=<n>
version=<v> signature=<s>` on About.

**Owed.** Every other ELF (PREFS, HOLOCRON, NET, BIG, DIAG, VUG…) gets a `res/` dir as its icon is designed;
the aarch64 programs; a bundle directory (row 38's "later"); the dock's tile width still sizes from the
caption budget (DOCK2's layout), so a tile is as wide as before with the icon centred.

## Built (M1–M4, no knob: the registrar is a few KiB of code and six ~5 KiB blocks, compiled in every image)

- M1 `midden_core` `res_records`/`res_get`/`res_build`/`app_res`/`elf_shdr_table`/`elf_note_sections`/`res_note`
  (`cargo test -p midden_core`: 29 pass, 3 new). `una_abi::APP_RES_NOTE_TYPE = 2`, asserted equal in `fs/appres.rs`.
- M2 `tools/una-res` (`cargo test -p una-res`: 2 pass — stamp round trip, every icon decodes, a second stamp refused;
  a stamped host `/bin/true` still runs and `readelf -S` lists `.note.unaos.res NOTE` with flags none).
  `pixel_core::png::encode::encode_rgba` (colour type 6; `cargo test -p pixel_core` exit 0). Icons (ours):
  `unaos/res/{quarry,settings,console,shell,facet,generic}/icon.svg` + `unaos/crates/user-lumen/res/icon.svg`;
  32/64 px PNGs fit the 3072-byte attribute bound, the 128 px ones do not always (facet 3338, lumen 3594) and stay
  in the block only. arroyo `una_res_stamp` runs after Lumen's strip.
- M3 `fs/appres.rs`: sight at `bg` spawn (shell.rs, same-line) and in Quarry's `compute_meta` for every
  `application/x-unaos-elf` row; a new signature object is created with all its attributes in ONE
  `create_files_batch`; `assoc::opener_for_in` reads `una:apps` (source `app`). `tests appres` rides
  `filetype::ensure_tests`.
- M4 dock (`dock_icon_row`, one line before the caption comment), Quarry (`blit_path_icon` after the name),
  About (`winmenu` app-menu arm, same-line) + `facet::win()`.

**What the next flight reads** (after `tests appres`, or a Quarry open of `/apps`):
`[appres] sight path=/apps/LUMEN.ELF res=yes source=elf attrs=<n> on=types sig=org.unaos.lumen` (the first time on a
card; `source=attrs … on=cached` on later boots), one `:: APPRES: <app> sig=… version=… kind=… icon=32px+|generic
source=builtin|elf|attrs drawn=true cache=types|- ::` per program, then
`:: APPRES: programs=<5 + ELFs in /apps> with_res=6 icons_drawn=<programs> cached_attrs=1 -> PASS :: lumen=staged
root_attrs=true …`; App menu → About on a Lumen window: `[appres] about app=lumen name=Lumen version=0.1.0+<sha>
signature=org.unaos.lumen res=yes` and a notice "About Lumen".
