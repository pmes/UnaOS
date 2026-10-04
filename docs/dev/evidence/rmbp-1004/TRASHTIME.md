# TRASHTIME — the Trash as attributes on the object, and UnaFS time in the kernel (B308)

Answers audit B288 (trash half), B292 (kernel half), B294 (trash by inode id). Branch
`exec-rmbp-trashtime`, cut from b42b87cc.

## Finding

* `fs/trash.rs` keeps `/home/<user>/.Trash/.index` as TAB lines of `orig, name, unixtime` and finds a
  trashed item by its NAME; Matrix (the CODEX Files handler) has Finder verbs and no trash: two
  Finders, two ideas of what a trashed file is.
* F3F4 gave UnaFS v6 inodes `ctime/mtime/atime` and a clock hook (`unafs::clock::set_clock_hook`);
  the kernel never installed it (a bare `no_std` build stamps 0), and `NativeBackend` still answers
  `DirEnt.mtime = None`, `Stat.mtime = None`.

## The seam

* Time: **Kernel — fs-core.** The kernel is the clock the crate asks; the crate keeps the stamps.
  `fs/unafstime.rs` installs the hook at the first mount (beside the warn hook in `mount_on`) with
  `clock::try_unix_now` (the hook runs inside `with_unafs`'s masked hold, so it must never spin; a
  contended read reuses the last good second, an unanchored clock answers 0 = "unknown").
  `NativeBackend::read_dir/stat` read `inode.mtime` (already on the inode `read_inode` returns).
* Trash: **Matrix — kernel-by-ruling R50** (Quarry is the Finder). The definition of a trashed
  object is ONE set of literals in `una-abi` (zero-dependency, consts only, linked by both rings):
  `una:trash-origin` (Str, the original absolute path), `una:trash-time` (Int, unix seconds),
  `una:trash-by` (Str, the session user). The kernel Trash and Matrix's host Finder both stamp and
  query those keys; neither keeps a second store.

## Milestones

* **M1 — the clock hook.** Hook installed at mount; native `DirEnt.mtime` and `Stat.mtime` from the
  inode; `stat` prints `ctime:` on native (read through `UnaFS::stat`; `vfs::Stat` gains no field so
  the five-arc fold stays keep-both). FAT path unchanged. Witness (run first inside `tests trash`):
  `:: UNAFSTIME: hook=1 mtime=<ok> ctime=<ok> -> PASS ::`, `-> SKIP reason=no-unafs-volume` on FAT.
* **M2 — trash by attributes.** On a volume with object ids (`Stat.id.is_some()`), trash stamps the
  three keys ON the object and renames it into `.Trash/` (same inode id); no `.index` is written.
  `entries()` = `query("una:trash-origin != \"\"")` scoped to direct children of the user's Trash;
  `restore(name)` resolves the name to an inode id and moves the id's CURRENT path back to the
  origin, dropping the keys; `empty` drops the keys and unlinks. FAT keeps the `.index` path as the
  fallback, chosen in ONE function (`store()`), each op printing `[trash] store=attrs|index`.
* **M3 — Matrix.** `handlers/matrix/src/trash.rs`: `VaultTrash` over a host `unafs::UnaFS` vault
  (trash/restore/empty with the same keys, by inode id); `FsVerb::{Trash, Restore, Empty}` appended
  to bandy's enum (tail; existing wire indices unchanged) and routed by `Finder` when it carries a
  vault. `cargo test -p matrix`.
* **M4 — `tests trash`.** Attribute legs: trash → query finds it (by id) → rename inside Trash →
  restore to the origin → empty → query finds nothing; the fallback leg on FAT. Witness
  `:: TRASH: store=<attrs|index> trashed= restored= emptied= query_ok= -> PASS ::`.

## Owed

* The `.index` / `.Trash/.index` rows in `unaos/scripts/charter.registry` should be re-worded at the
  fold to "FAT fallback only (TRASHTIME)"; not edited here.
* Atime is never stamped by reads (the crate's noatime posture); `ls -l` shows mtime only.
* Matrix's legacy `delete` → `.una-trash/` on a plain host directory stays (no attributes there).
