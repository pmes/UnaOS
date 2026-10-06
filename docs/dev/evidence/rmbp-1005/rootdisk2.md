# ROOTDISK2 (rmbp-ledger B401) — R94 in full: `/apps` and `/lib` live on the UnaFS volume

## Finding (sources, cut 05debd26; ROOTDISK's doc `rootdisk.md` continued)
- `bootdisk::bind_root` mounts `/apps` (boot FAT rooted at `APPS/`) and `/lib` (`rootdisk::bind_lib`, rooted at `LIB/`)
  over the native root on every table build: the two root entries are mount-table LINKS onto the ESP (Quarry `@`).
- The card (`arroyo esp_x86_unafs_card`) puts only `system/test-f` on the UnaFS volume; `tools/una-card` copies the ESP
  and data trees (APPS, LIB — rustc alone ~420 MB) into p1; `unafs_mb` defaults to 512, capped at 2048.
- The exec PROBE on x86 (`shell::FatVolume::is_file`) walks the boot FAT's `APPS/` directly, and seven ring-0
  launchers/witnesses (`lumen.rs`, desktop_uefi's desktop app, shotmask's Lumen, WINX-2/WINX-8/PULSE-W, the two
  loginst STAT fixtures) read `FatFs::find_app`, i.e. the FAT `APPS/` — none of them can see a program on UnaFS.
  The re-resolve and the load (`read_el0_image`), the loader's `LIB_DIRS` and the selfbuild stages already go through
  the mount table, so they follow whatever `/apps` and `/lib` resolve to.
- UnaFS HAS `rmdir` (crate `UnaFS::rmdir`, one CoW transaction; `NativeBackend::remove_dir`; `shell rmdir`): what was
  missing is the test using it — `tests rootdisk` leaves `/newfolder`.
- HOMESOIL leg 6 still expects `/apps` rooted at `APPS` and 3 mounts in all four root states (ROOTDISK's `/lib` made it 4).

## The seam — Kernel, fs-core (no new store; the mount table stays the one place a name is decided)
- **Native root ⇒ no `/apps` or `/lib` mount at all.** They are directories of `/` (the UnaFS root), so `/apps`,
  `/volumes/UnaOS/apps` (ROOTDISK's alias) and a directory a user makes at `/` are the same kind of thing: real.
  A FAT root (no UnaFS on the boot disk, the two-device and Pi shapes) keeps the FAT `APPS/`+`LIB/` mounts — there `/`
  IS the boot FAT, so they are its own directories, not links.
- **One program source**: `rootdisk::program_source()` answers `find_app` / `read_file` through the mount table at
  `/apps/<NAME>`; the seven FAT-direct callers swap `fat::mount_program_source()` for it on the SAME line (line-neutral).
  `FatVolume::is_file` (x86) keeps binding `mount_program_source` for the FATVERB stamp but answers through the
  namespace (cwd, then `EXEC_ROOT`) — the probe, the re-resolve and the load ask one question of one table.
- **The compat redirect is deleted** (`COMPAT`, `compat_rel`, its fold in `redirect`).
- **Root links**: `LINKS = boot, volumes`. On a native root Quarry draws `apps` and `lib` plain (`/`), only `volumes` `@`.
- **The card**: arroyo stages `apps` (ESP `APPS/` ∪ data `APPS/`) and `lib` (ESP `LIB/`) INTO the UnaFS volume with
  `tools/unafs put` (now recursive for a directory source: mkdir + files, one commit per directory); `tools/una-card
  --skip APPS --skip LIB` leaves them off p1 (the ESP keeps the loader, the kernel, `system/` and the data tree's
  fixtures; no mirror of apps/lib). `unafs_mb` = content + 25% + 512 MiB headroom, floor 4096 (`UNAOS_X86_UNAFS_MB`
  still overrides upward); una-card lays p2 from the image's size and copies it sparse (zero MiB runs are skipped).
- **rmdir**: `tests rootdisk` removes what it made with the VFS verb (`MountTable::remove_dir` → `UnaFS::rmdir`) and
  proves the name gone from both `/` and `/volumes/UnaOS`.

## R99 (Peter, 2026-10-06, mid-arc): "the boot fat partition is sacred"
- **Nothing system on it**: the card puts the ESP's and the data tree's `system/` (fonts, witness-owners.txt, test-f)
  into UnaFS `/system` beside `/apps` and `/lib`; `una-card --skip APPS --skip LIB --skip system`. p1 keeps the loader,
  the kernel, the firmware blobs (WIFI-1's `/FIRMWARE/` search) and — named, owed — the EL0 flat-namespace fixtures.
- **Read-only for every principal** (new `fs/bootfat.rs`, `//! CHARTER: Kernel — fs-core`): `bind_root` arms it on an
  x86 native root; `FatBackend::authorize_write` asks `bootfat::refuse` first (same-line fold), so every VFS write verb
  on a `boot` mount (`/boot`, `/volumes/boot`) says `[boot] fat write refused path=… by=<principal> (R99: sacred) ::`
  and fails `Unsupported`. The one writer path is `bootfat::fat_unlock(by, for)` — `[boot] fat unlocked by=… for=…`,
  relocked on drop with `[boot] fat relocked by=…`. A FAT-root boot is not armed (its FAT is `/`).
- **The boot log moves** (new `fs/bootlog.rs`, `//! CHARTER: Kernel — fs-core`): `flight_recorder::service` hands the
  flush to `bootlog::divert(snapshot)` right after the FRGUARD verdict (same-line fold); on a UnaFS build the
  `UNAOS.LOG` reservation is never taken, and the recorder's snapshot (FLIGHTRING's ring on the merged tree — the
  target changes, the ring does not) is written to `/var/log/boot-<n>.log` (n = newest + 1, 16 kept).
- **Quarry**: at `/volumes` the `boot` row draws a padlock after its name (`columns::lock_glyph`).
- **`tests bootfat`**: a create as root at `/volumes/boot/…` and `/boot/…` must be refused with nothing made; the
  installer's unlock opens the gate and its drop closes it.

## Milestones
M1 tools: `unafs put` recursive · una-card `--skip` + sparse copy · M2 arroyo card staging + `unafs_mb` derivation ·
M3 kernel: bind_root native shape, `program_source`, exec probe, compat deleted, links, HOMESOIL leg 6 ·
M4 `tests rootdisk` → ROOTDISK2 witness (rmdir cleanup) · M5 this doc's hot-swap section.

## Witness (x86 metal shape, no knob; `tests rootdisk` is registered in every build)
- boot: `[vfs] apps+lib on the native root: /apps /lib are directories of / (R94: real, no links) ::`
- `tests rootdisk`: `:: ROOTDISK: root=UnaOS lib=/lib apps_alias=same-inode user_dir_at_root=visible-under-volume -> PASS :: …`
  then `:: ROOTDISK2: apps=unafs lib=unafs var=unafs boot_fat=readonly unlock_path=installer rmdir=ok image_mb=<n> -> PASS :: links=volumes-only progs=<n> cleaned=<newfolder|-> ::`
- boot: `[boot] fat sacred: …`, `:: FR: UNAOS.LOG not reserved — R99 …`, `[boot] log -> /var/log/boot-<n>.log …`
- `tests bootfat`: `:: BOOTFAT: write_as_root=refused unlock_path=installer relocked=1 -> PASS :: refusals_said=2 ::`
- `tests volumes`: `root_view=apps,home,lib,system,var,volumes pure=true`; `tests exec`, `tests selfbuild6` from the new home.
- An OLD card (no `apps` on UnaFS) reads `apps=absent` and the desktop app DECLINEs `reason=absent`: re-image.

## Hot-swap of the system disk (R94, design only — not built)
What holds today: every root path is a mount-table name, the table is rebuilt per verb from `bootdisk::survey()`, and
after ROOTDISK2 `/apps`, `/lib`, `/home`, `/system`, `/var` are directories of whatever `/` is — no root entry names a
second volume. Stores record PATHS (users store `/var/…`, Holocron, attrs), not volume ids.
What a swap needs: (1) `/` bound through a boot-volume HANDLE that can be re-pointed atomically (today `NativeBackend`
reaches the one shared `fs::unafs` mount, `MOUNT` latched on first `locate_on`); (2) open descriptors (`linuxabi` fd
table, EL0 file slots, the users store's held handle) re-homed or refused with `-ENODEV` across the flip; (3) `/boot`
and `/volumes/boot` re-pointed at the new disk's ESP.
**The one place that would still break: the shared UnaFS mount itself** — `fs::unafs` keeps ONE mounted
`FileSystem` bound to the handle it was first located on (`mount_bound_handle`), and `bind_root` only decides
`native` by asking whether that mount rides the boot disk (`unafs_state` → `present-on-other-handle` otherwise). A new
system disk would be seen as "UnaFS on another handle" and `/` would fall back to the FAT; there is no unmount/remount
of the shared mount. That latch, not the namespace, is what a hot swap has to replace.

## Owed (named)
- The ESP still carries the data tree's EL0 fixtures (HELLO.BIN, SCRATCH.BIN, GROW.BIN, S8W.BIN, BLOCK.TXT, hello.txt,
  readme.txt) — EL0's flat `sys_open` is a FAT-root ABI (layout.md §2.1); moving them is an ABI change (R99 owes it).
- Ring-0 writers that bypass the VFS (`fat.rs` direct: the users store's FAT path, prtscr's FAT capture) are not
  behind the gate; on a native root they already write UnaFS. Fixtures that wrote `/boot/…` (VFSROUTE's
  `/boot/VFSROUTE.TXT`, Quarry ops' `/boot/QOPS.TXT`) now read the refusal on the card.
- No installer/updater VFS write to the boot FAT exists yet, so `fat_unlock` has no production caller; `install ssd`
  writes the SSD's own ESP through its granted target, not the boot FAT.
- The hot swap (above). The Pi/Orin cards keep their FAT `APPS/` (their `/apps` is the FAT program source by design).
