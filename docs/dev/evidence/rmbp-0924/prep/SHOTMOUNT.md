# SHOTMOUNT — prep

## The finding

`docs/dev/LEDGER.md`, `| SO19 |`: **"`screenshot` still writes FAT-direct."** Every file VERB
routes through the mount table since VFSROUTE, but the capture path does not:
`video::prtscr::capture()` walks `fat.rs` inside `video/prtscr.rs`, so a `SCREEN<n>.PNG` lands on
the FAT volume whatever the namespace says the root is. On a board whose `/` is NOT the FAT volume
(any UnaFS root) the capture and `ls /` disagree about where it went. Filed by VFSROUTE (orin 17)
while routing the shell verbs; left open because `video/prtscr.rs` was outside that arc's lane.

Peter's words (exec brief, relayed): since flight 15 a user session (`una`) exists on the rMBP, so
Print Screen should write to `/home/una/Desktop` — cite the path that decides the directory, don't
re-derive it.

## Mechanism

- **FAT-direct write path.** `prtscr.rs:958` `capture()` -> `:978` `capture_inner()` ->
  `Job::begin()`/`Job::slice()`. Volume from `:752-780` `mount_capture_target() ->
  Result<FatFs, Refusal>` — a two-rung ladder, `fs::fat::mount_program_source()` then, on veto,
  `fs::fat::mount_source(BlockSource::Usb)`. Both return a raw `fat::FatFs`, never
  `fs::vfs::MountTable`. Bytes go out via `FatFs::write_grow` (`:1265`) and `FatFs::create_in_dir`
  (`:907`) — `grep -n "vfs::" video/prtscr.rs` returns nothing.
- **Directory decision (PRTSCR-HOME / SCRSHOT-DESKTOP, R60) — unchanged by this fix.**
  `prtscr.rs:1732` `logged_in_user()` -> `:1743` `home_of(name)` (`fs::users` store) -> `:1806`
  path-to-8.3 (`/home/una` -> `HOME/UNA`) -> `:1852` `ensure_capture_dir` walks/creates
  `HOME/UNA/DESKTOP` on the mounted `FatFs` (`theme::CAPTURE_DIR` = `Desktop`, `:96`). This is the
  citation for "why `/home/una/Desktop`" — SO19 is the volume the walk runs on, not the path.
- **Mount-table write API.** `fs/vfs.rs:560` `create(&self, path, kind, principal) ->
  Result<Stat, VfsError>`, `:565` `write(&self, path, offset, data, principal) ->
  Result<usize, VfsError>` — both `resolve(path)` (`:510`, longest-prefix over `self.mounts`) then
  dispatch to the resolved `VfsBackend`. `:594` `write_veto(&self, path)` answers the same veto
  question `mount_capture_target` asks today, per-path instead of per-`FatFs`-handle.
- **Existing writer over the mount table (pattern to copy).** `shell.rs:567` `fs_write` — `mt` from
  `:7427` `vfs_mount_table()`, then `mt.unlink`, `mt.create(&path, NodeKind::File,
  SHELL_PRINCIPAL)`, `mt.write(&path, 0, data, SHELL_PRINCIPAL)`; no `fs::fat` name in the function.
  `SHELL_PRINCIPAL` (`:430`) is `vfs::KERNEL_PRINCIPAL` — the shell acts as the machine, and a
  capture (keypress or verb) should use the same principal, not `una`'s name.
- **The table.** `shell.rs:7427` `vfs_mount_table()` builds a fresh `MountTable`, calls
  `fs::bootdisk::bind(&mut mt)` (HOMESOIL: mounts `/`, `/boot`, `/apps` over the disk the kernel
  was found on, with that source's own write posture). Resolving `/home/una/Desktop` through this
  `mt` gets whichever backend (`FatBackend` `vfs.rs:1129` / `NativeBackend` `:1583`) actually owns
  the path — agreeing with `ls /`, which is the whole bug.
- **Verb entry point, unchanged.** `shell.rs:5556-5580` `"screenshot" =>` calls `prtscr::capture()`
  directly (`:5559-5561`: a verb reimplementing this would be a second capture path). Fix stays
  inside `prtscr.rs`.

## Plan

**M1 — resolve through the mount table, not `fs::fat`, directly.**
`prtscr.rs` `mount_capture_target()` (`:752`) and every `FatFs` call in `Job::begin`/`Job::slice`
(`create_in_dir`, `write_grow`, `locate_in_dir`). Replace the ladder with a `vfs::MountTable` (via
`shell::vfs_mount_table()`, pending the layering question below) and drive the destination path
through `mt.write_veto`, `mt.create(path, NodeKind::File, principal)`,
`mt.write(path, offset, data, principal)` — the same four-step recipe `shell.rs:567` uses, which
this file's own `:1227` comment already names as the target shape.
Witness: `:: SHOTMOUNT: via=vfs path=/home/una/Desktop/<name> bytes=<n> -> PASS ::`, printed where
`shot.report_ok()` runs today (near `:1421`).
Go-red: reinstate a direct `fs::fat::mount_source`/`create_in_dir`/`write_grow` call — `via=vfs`
must then read `via=fat` or vanish, and any fixture asserting `via=vfs` goes red.

**M2 — never-overwrite and slot naming over `VfsBackend`.**
`next_free_name` (`:829`, today `fs.locate_in_dir` on a `FatFs`) becomes an `mt.stat(path)` probe
per candidate (`Err(NoSuchPath)` = free). `ensure_capture_dir` (`:1852`) becomes one
`mt.create(path, NodeKind::Dir, principal)` per path component, tolerating
`VfsError::Backend("exists")` as `shell.rs:634` `fs_mkdir` already does.
Witness: `:: SHOTMOUNT-NAME: dir=/home/una/Desktop candidate=SCREEN0.PNG taken=no -> PASS ::`
Go-red: drop the `Err(NoSuchPath)` match arm so any non-match reads "free" — a second capture then
targets the first capture's own name and the never-overwrite fixture (`:933`) goes red.

**M3 — point the existing session/path fixture at the new route.**
The `una` -> `/home/una/Desktop` decision (`:1732`, `:1743`) is UNCHANGED. M3 only repoints the
`:2004` "fixture proves BOTH paths" selftest at the `mt`-routed write so it still proves the
no-session fallback and the `una` arm after M1/M2.
Witness: existing fixture line (`:1964` region already carries `dir=/home/una/Desktop`) gains
`via=vfs`.
Go-red: same mutation as M1 — one call site, one flip.

## Spec pins

`unaos/scripts/specs/x86-wc.spec` (screenshot needs `UNAOS_WC=1` + the kepler knobs to be reachable
at all, per CLAUDE.md's video-gate rule):

```
REQUIRE :: SHOTMOUNT: via=vfs path=/home/una/Desktop/[A-Za-z0-9 ._-]+ bytes=[0-9]+ -> PASS ::
FORBID  :: SHOTMOUNT: via=fat
```

Plain literal-anchored lines, no look-around, matching `x86-fat.spec:309,320`'s shape.

## Open questions

- Does `video/prtscr.rs` depend on `crate::shell` already, or does calling
  `shell::vfs_mount_table()` from `video/` add a layering dependency LAWS forbids? If so the
  builder needs a new home (`fs::vfs` or `fs::bootdisk`) callable from both.
- Write principal: `KERNEL_PRINCIPAL` (matches `shell.rs`) or `una`'s own name? Affects any
  per-object ACL the native backend puts on `/home/una/Desktop` later.
- Does `mount_capture_target`'s two-rung ladder (program source, then USB) become dead code once
  routing is through `vfs_mount_table()`'s own HOMESOIL binding, or does it still add something?

## Next-session start

1. `sed -n '740,835p' unaos/crates/kernel/src/video/prtscr.rs` — reread `mount_capture_target` and
   `usb_backed` in full before touching either.
2. `sed -n '1050,1270p' unaos/crates/kernel/src/video/prtscr.rs` — find every direct `FatFs` call
   M1/M2 must replace.
3. `grep -rn "^use crate::shell" unaos/crates/kernel/src/video/` — settle the layering question
   before drafting real code.

## Draft code (unbuilt)

```rust
// prtscr.rs — replaces mount_capture_target() (:752). UNBUILT: principal/error-adapter unsettled.
fn resolve_capture_target(path: &str) -> Result<crate::fs::vfs::MountTable, Refusal> {
    let mt = crate::shell::vfs_mount_table(); // OPEN Q: layering — may need to move
    match mt.write_veto(path) {
        Ok(None) => Ok(mt),
        Ok(Some(why)) => Err(Refusal::ReadOnly(mt.volume_name(path).unwrap_or_default(), "", why)),
        Err(e) => Err(Refusal::NoVolume(e)), // VfsError vs FatError differ; needs a real adapter
    }
}
```

```rust
// prtscr.rs — replaces the FatFs create_in_dir/write_grow calls in Job::slice (near :1265).
use crate::fs::vfs::NodeKind;
mt.create(&full_path, NodeKind::File, crate::fs::vfs::KERNEL_PRINCIPAL).map_err(Refusal::from_vfs)?;
let written = mt.write(&full_path, at as u64, chunk, crate::fs::vfs::KERNEL_PRINCIPAL)
    .map_err(Refusal::from_vfs)?; // Refusal::from_vfs does not exist yet — M1 adds it
```
