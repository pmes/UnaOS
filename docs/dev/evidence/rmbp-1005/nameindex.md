# NAMEINDEX (rmbp-ledger B432) — a volume-wide name index in the UnaFS core

## Design (written before the code)

**Finding.** QUARRY3 (B413) `UnaFS::find_names` and LAUNCHER (B417) `fs::search::by_name` are both bounded
walks of the per-directory name lists (256 hits / 512 directory reads per keystroke; 6000 entries per launcher
open). UnaFS's two B+trees are the ATTRIBUTE catalog; no name is in them. Flights 24/25 carry no name-search
line (nothing measured it on metal) — the witness below is the first.

**The seam (R79: one store, shared core).** `unaos/libs/fs/unafs` (`src/nameindex.rs`, a child module of
`fs`, so the fold with QUERYFOLDER's `fs.rs` tail and `index_apply` hunks stays apart). The name is an INDEX
FACT, `una:fsname` = the lower-cased (ASCII) name, one fact per word start (the leaf, and each suffix after
`_ - . space`, at most 8), in the existing ordered tree (String tag) and equality tree. It is NOT an inode
attribute: the exact name stays in the record (the inode's `name`, the directory entry). Only the core writes
it — `add_entry`/`create_files_batch` (create), `rename`, `unlink`, `rmdir`, fsck's `relink_inode` — through
`index_apply`, so QUERYFOLDER's `CATALOG_GEN` bump (it lives in `index_apply`) covers name writes after the
fold with no line of mine. Readiness: a marker fact `una:fsname-index` = 1 on the root inode, stamped by
`format` (a fresh v6+ volume is trivially complete) and by a finished build.

**Key collision (found by `boot80_logic`).** `una:name` is ALREADY a live attribute key — the type database's display name (`fs/assoc.rs NAME_KEY`, `midden_core RES_KEY_NAME`) — so the index fact is `una:fsname` (and the marker `una:fsname-index`); one key, one meaning.

**One query.** `find_names(prefix, limit)` = ONE ordered-tree range scan `[kh‖3‖prefix, kh‖3‖prefix‖FF)`,
at most `NAME_SCAN_MAX` keys, each candidate verified against the real name (leaf prefix = rank 2, word prefix
= rank 1), paths by parent pointers. The walk is deleted. `fs::search::by_name` and Quarry's search field call
it; the launcher asks it per keystroke (no open-time snapshot on an indexed root; a FAT root keeps its walk).

**Migration (R93/R80: a login task, not boot).** `users::login` posts `fs::nameindex::post_login`; the desktop
service pass builds in chunks (`name_index_build_step`, 512 inodes, one commit each — the IRQ-masked mount lock
is never held for the whole volume), then marks and prints
`[unafs] name-index built names=<n> ms=<n>` (or `[unafs] name-index ready names=<n>` when already marked).
Host: `unafs reindex <img>` (drop every `una:fsname` key, rebuild, mark); `fsck` reports
`name_index_missing` / `name_index_stale` (a ready index only) and `--repair` reindexes.

**Milestones.** M1 core + KATs (prefix, case, word, rename, unlink, rmdir, batch, readiness, build, fsck)
`cargo test -p unafs`. M2 host tool `reindex` + fsck print. M3 kernel: `fs/nameindex.rs` (login task, `tests
nameindex`), `fs/search.rs` + Quarry toolbar + launcher on the index, x86 leg.

**Witness (metal).** login: `[unafs] name-index built names=<n> ms=<n>`; `tests nameindex` →
`:: NAMEINDEX: names=<n> prefix_ms=<n> src=index -> PASS ::`; `tests quarry3` `[quarry3] search … src=una-name-index`;
`tests launcher` `[launcher] fixture … src=una-name-index`.

**Owed.** QUERYFOLDER's `name` fact term (a scan today) could ride this index; FAT roots keep the walk;
non-ASCII case folding is ASCII-only (as `find_names` was); substring (mid-word) search is gone by design
(prefix / word-prefix, the launcher's semantics).

## Built (M1–M3)

- M1 `unaos/libs/fs/unafs/src/nameindex.rs` (+ hooks in `fs.rs` add_entry / create_files_batch / unlink / rmdir /
  rename / relink_inode / format; `index_apply` keeps name facts out of the equality tree and `entries`;
  `fsck.rs` `name_index_missing` / `name_index_stale`, repair reindexes). QUARRY3's walk deleted.
  KATs `tests/name_index.rs` + `tests/find_names.rs` (rewritten for prefix semantics). `codec_volume` v6/v7 IMAGE
  digests re-cut (index bytes moved; READ-BACK digests unchanged — the codec oracle holds; v3/v5 untouched).
- M2 `tools/unafs`: `unafs reindex --img <img>` → `[unafs] name-index built names=<n> keys=<n> ms=<n>`; `fsck` prints
  `name index : ready= keys= expected= missing= stale=`.
- M3 kernel: `fs/nameindex.rs` (login task + `tests nameindex`), `fs/search.rs` `query`/`by_name_src`
  (`src=una-name-index`, walk only on a FAT root or an unbuilt index), Quarry toolbar, launcher (per-keystroke
  index query, no open-time walk on an indexed root; fixture prints `src=`), `users::login` post, `settings::service`
  chain, `filetype::ensure_tests` registration.

## Legs

`cargo test -p unafs -p unafs-cli`: 240 passed, 1 failed — `refmap_two_level::data_crosses_the_old_2gib_boundary`
`No space left on device` (the shared disk, 1.6 GB free; not this arc). x86 seat line exit 0 and both aarch64
legs exit 0 — x86 only after LOCAL, UNCOMMITTED patches to the tip's own merge17 reds (`video/settings.rs`
`press_appearance` missing its `}` and `TP_TAB` usize-vs-u8 at line 470; `video/paper.rs:198` and
`video/knurl.rs:421` const calls to the now non-const theme token fns) — the seat's, not in this arc.
