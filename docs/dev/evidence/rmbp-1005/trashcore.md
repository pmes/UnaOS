# TRASHCORE (rmbp-ledger B449) — one trash core for Matrix's VaultTrash and the kernel's Trash

**Finding (ARCHREVIEW F5).** `unaos/crates/kernel/src/fs/trash.rs` (575 lines) and
`handlers/matrix/src/trash.rs` (`VaultTrash`, 292 lines) implement the same Trash twice: the same
`una:trash-*` keys (una-abi), the same rules (path refusals, collision names, stamp-then-rekey,
query scoped to the Trash's direct children, restore by inode id, strip-then-unlink on empty), written
twice and already drifting (the kernel's 8.3 collision name, `bad-name`, depth bound, case folding,
the FAT `.index` fallback exist only in the kernel). The kernel header claimed `kernel-by-ruling R50`
(R50 is Quarry, not the Trash); PATCH 64fceedf set it to `owed B288`.

**Seam (R79: shared-core).** `unaos/libs/fs/trash_core` — `#![no_std]` + `alloc`,
`#![forbid(unsafe_code)]`, depends only on `una-abi` (the key literals, re-exported). It holds the
RULES and both stores' algorithms, generic over one I/O trait, `TrashFs` (stat with object id, mkdir,
rename, attribute get/set/remove, query, list, unlink, rmdir, whole-file read/write/append, plus three
volume facts: case folding, 8.3-representable leaf, tree-depth bound). The kernel implements `TrashFs`
over the `MountTable` (and keeps its session user, clock, `[trash]` serial line and shell verb); Matrix
implements it over `UnaFS<D>` (and keeps its `TrashStore` trait for the Finder). Neither ring keeps a
Trash rule of its own. The Trash query folder (QUERYFOLDER B420) reads `entries()` and the core's
`is_trash_folder` / `origins`. Charter: `//! CHARTER: Matrix — shared-core` on both the kernel file and
the core.

**Milestones.**
- M1 — `trash_core`: `TrashFs`, `Store` (attrs/index, chosen by `store()`), `Entry`, `trash`,
  `restore`, `empty`, `entries`, `collision_name`, `parse_index`/`render_index`, `is_trash_folder`,
  `origins`; host tests over an in-memory `TrashFs` (both stores, FAT 8.3 collision, refusals, rename
  inside the Trash, case folding). Root workspace member.
- M2 — Matrix: `VaultTrash` is the `TrashFs` adapter over `UnaFS` + the core; its tests unchanged pass.
- M3 — kernel: `fs/trash.rs` is the `TrashFs` adapter over the `MountTable` + the core; public API
  (`trash`, `restore`, `empty`, `entries`, `count`, `store`, `trash_dir`, `home_base`, `Entry`,
  `shell_verb`, `selftest`) unchanged, so Quarry, DOCK2, QUERYFOLDER, facet compile untouched.
  Charter line `Matrix — shared-core`.

**Witness.** Unchanged wire: `tests trash` prints
`:: TRASH: store=attrs trashed=2 restored=1 emptied=1 query_ok=3 -> PASS ::` (UnaFS home) or
`store=index` (FAT home), and every op prints `[trash] store=… op=… path=… ok=… reason=…`. The core
adds one arming fact to that op line: `core=trash_core` — so a metal capture proves the shared core ran.

**Owed.** Matrix's workspace `.una-trash/` (the non-vault fallback in `finder.rs`) is a third, older bin
outside this arc (it is workspace housekeeping, not the user Trash); a seat decision whether it goes.
The FAT `.index` fallback lives in the core now, so a Matrix vault on FAT would get it free — none
exists yet.
