# BP-storage — block, FAT, UnaFS, VFS, root, prefs, users, install (read at 1719fe54)

## 1. What it is
- Block layer: `unaos/crates/kernel/src/drivers/block.rs` with handles `Global`, `Usb`, `Sdhc` (x86 `sdhcblk`), `TegraSd` (`tegra`+`sdmmc`) (`unaos/crates/kernel/src/drivers/block.rs:563-585`); AHCI (`unaos/crates/kernel/src/drivers/ahci.rs`), USB mass storage on xHCI, Pi `unaos/crates/kernel/src/drivers/emmc2.rs`.
- Filesystems: FAT (`unaos/crates/kernel/src/fs/fat.rs`), exFAT (`exfat_core`), UnaFS (`unaos/libs/fs/unafs`, on-disk v7; kernel glue `unaos/crates/kernel/src/fs/unafs.rs`).
- ONE namespace: the VFS `MountTable` (`unaos/crates/kernel/src/fs/vfs.rs`) is the only place a name is decided; `/volumes/UnaOS` IS `/` (`unaos/crates/kernel/src/fs/rootdisk.rs:7-13`).
- The root is the disk the kernel was found on, by content (`unaos/crates/kernel/src/fs/bootdisk.rs:592-612`); on the rMBP image it is UnaFS on the card, or the internal SSD's UnaFS partition when it is OURS (`unaos/crates/kernel/src/install/ahciroot.rs:6-20`).
- Stores on the root: Principia's preferences (`<home>/settings/<domain>`), the users table `/USERS.DAT`, homes `/home/<name>`, `/apps`, `/lib`, `/var`.

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-storage-1 | The mount table is the ONE name authority; no second store of names, no copy; `/volumes/UnaOS/<p>` resolves to `/<p>` (same backend, same inode). | `unaos/crates/kernel/src/fs/rootdisk.rs:7-13` | `tests rootdisk` (`:: ROOTDISK: … -> PASS ::`) |
| BP-storage-2 | On a native UnaFS root, `/apps`, `/lib`, `/var`, `/home` are directories of the UnaFS volume; the root's only links are `boot` and `volumes` (R94). | `unaos/crates/kernel/src/fs/rootdisk.rs:20-25` | `tests rootdisk` (`ROOTDISK2 … links=volumes-only`) |
| BP-storage-3 | The kernel finds the disk with THIS kernel on it by content; no board, slot, serial, knob or boot method decides it; nothing found is a witnessed refusal, never a guess. A walk that saw disks and no root is re-surveyed only when the disk set changes. | `docs/dev/LAWS.md:190-228`; `unaos/crates/kernel/src/fs/bootdisk.rs:592-612` | `[vfs] root -> …` witness (first call) |
| BP-storage-4 | The boot FAT (ESP) is read-only for every principal, root included, once a native root binds; its one writer is the installer/updater through `bootfat::fat_unlock`, said on the wire. It holds only loader, kernel, boot config and firmware (R99). | `unaos/crates/kernel/src/fs/bootfat.rs:1-15` | `tests bootfat` |
| BP-storage-5 | A volume's write posture has a single definition (`fat::BlockSource::write_veto` / `block::handle_write_veto`); the default is read-write with named opt-outs (R59), subject to BP-storage-4. | `unaos/crates/kernel/src/fs/fat.rs:711`; `unaos/crates/kernel/src/drivers/block.rs:2613`; R59 | unheld |
| BP-storage-6 | UnaFS on-disk version is 7 (paged maps); this build mounts v3..v7; every bump is INCOMPAT and stamped, so an older reader rejects rather than misreads. | `unaos/libs/fs/unafs/src/superblock.rs:56-94` | unafs crate tests (host) |
| BP-storage-7 | One UnaFS implementation: the kernel, `tools/unafs` and the installer's `format_fresh` call the same crate function. | `unaos/crates/kernel/src/install/ahciroot.rs:18-21` | compile |
| BP-storage-8 | The UnaFS mount is ONE lock (`unaos/crates/kernel/src/fs/unafs.rs:567`, taken IRQ-masked); long work over it (the name index build) runs in chunks at login, never at boot (R80, R93). | `unaos/crates/kernel/src/fs/unafs.rs:567`; `unaos/crates/kernel/src/fs/nameindex.rs:6-12`; `unaos/crates/kernel/src/video/renderwedge.rs:16-25` | `tests nameindex`; `tests renderwedge` |
| BP-storage-9 | The SSD gets a root grant only when every partition is ours (UnaOS ESP with `BOOTX64.EFI` + `kernel.elf`, UnaFS), never a foreign or blank disk; a WRITE needs a minted grant AND the operator's typed token. The negative answer is cached. | `unaos/crates/kernel/src/install/ahciroot.rs:6-25` `unaos/crates/kernel/src/install/ahciroot.rs:39` `unaos/crates/kernel/src/install/ahciroot.rs:74`; `unaos/scripts/image-line.env:13-17` | `tests ahciw`; `tests dongleboot` |
| BP-storage-10 | The installer engine runs only when armed, and refuses a target whose leading sectors are not blank; it verifies by content (re-read + re-CRC). | `unaos/crates/kernel/src/install/mod.rs:3-20` | `./arroyo test-install` (QEMU) |
| BP-storage-11 | Preferences: ONE store, Principia's, as `<home>/settings/<domain>` TOML files (R98) in `prefs_core`'s format; the kernel writes `.new`, reads it back, parses, then swaps; a refused file is never overwritten. Only `unaos/crates/kernel/src/prefs.rs` writes a `/settings/` path. | `unaos/crates/kernel/src/prefs.rs:3-45`; `unaos/scripts/arch-check.py:22` | `gates` rows `prefs`, `arch` (`settings` leg) |
| BP-storage-12 | Once the root is bound, every prefs read/write/stat resolves against the root (no re-walk of other disks per call). | commit `715c69ff` (DONGLEBOOT B658) | `tests dongleboot` |
| BP-storage-13 | The users table is `/USERS.DAT` at the root of the root volume, swapped through `/USERS.NEW`; at most 8 users, names ≤ 8 bytes; homes are `/home/<name>`, spelt by `users::home_of`, never a literal. | `unaos/crates/kernel/src/fs/users.rs:140-157` `unaos/crates/kernel/src/fs/users.rs:740`; `unaos/crates/kernel/src/prefs.rs:26-28` | `tests login` family (unverified names) |
| BP-storage-14 | Trash is `/home/<user>/.Trash/` through the mount table; `trash_core` is the one Trash both rings share (seam `shared-core`, owner Matrix). | `unaos/crates/kernel/src/fs/trash.rs:1-10` | `tests trash` (unverified name) |
| BP-storage-15 | An app is its `/apps` entry (the folder `/apps/<name>/`, or today's flat `/apps/<NAME>.ELF`) plus its settings domain `settings/<name>`; `uninstall` removes the entry as the administrator (ROOTACL decides), the domain file under EVERY settings folder (the no-session store's and each user's `<home>/settings`, as the user's own Quarry delete, which prefs' delete watch resets in RAM), and its dock pin; a system domain or a name with no `/apps` entry is refused and nothing is removed (BP-bar-11). | `unaos/crates/kernel/src/install/uninstall.rs:1-25` | `tests uninstall` (`:: UNINSTALL: … leftover=0 … -> PASS ::`), unflown |
| BP-storage-15 | The name index is written with the name, never after it: the shared core applies the `una:fsname` facts (at most `NAME_WORDS_MAX` per name) in the SAME transaction as every create, mkdir, batch create, rename, unlink, rmdir and fsck relink; no caller and no VFS layer writes a name fact; no rebuild follows a write. The one build is the login migration of an UNMARKED (pre-B432) volume (BP-storage-8). | `unaos/libs/fs/unafs/src/fs.rs:1729` `unaos/libs/fs/unafs/src/fs.rs:1864-1865` `unaos/libs/fs/unafs/src/fs.rs:2017` `unaos/libs/fs/unafs/src/fs.rs:2126` `unaos/libs/fs/unafs/src/fs.rs:2242` `unaos/libs/fs/unafs/src/fs.rs:3528`; `unaos/crates/kernel/src/fs/nameindex.rs` (NAMEINDEXTX, file tail) | `tests nameindextx` (`:: NAMEINDEXTX: created=2000 renamed=200 deleted=200 hits=1800 misses=0 stale=0 per_op_us=<n> -> PASS ::`) |

## 3. Seams
- May call storage: VFS clients (shell verbs, `SYS_OPEN`/`SYS_PATH_*`, Quarry, prefs, users, wallpaper, apptrust), the installer.
- Storage may call: `amber_core::gpt` (all GPT reads), `unafs`, `exfat_core`, `trash_core`, `type_core`, the block layer. The render task touches storage only through bounded accessors (cores.md BP-cores-15).
- A new kernel store needs a CHARTER and an owner (`gates` arch row `store` leg, `unaos/scripts/arch-check.py:23-24`).

## 4. Wire
`:: UNAFSX86: root=unafs src=<ahci:<p>|card> … ::` · `[boot] step=root-mount …` · `[boot] fat write refused path= by= (R99: sacred)` · `[boot] fat unlocked by=<installer|updater> for=` ·
`[prefs] settings/<d> absent -> defaults …` · `[prefs] save held` · `[uninstall] app=<n> apps_files=<n> apps_dirs=<n> settings_removed=<n> pin_removed=<0|1> by=<p>` · `[unafs] name-index built names= ms= chunks=` · `AHCI: selfcheck port=0`.

## 5. Ruled, unbuilt / open
- R94's hot swap of the root volume: design only (`unaos/crates/kernel/src/fs/rootdisk.rs:24`).
- `unaos/crates/kernel/src/fs/users.rs:30-40` header still places `USERS.DAT` on "the EL0 FAT volume"; the constants say root of the root volume. Stale comment, owed.
- Pi storage (emmc2/SD root on bare metal) at HEAD: `unverified` (not read for this sheet).
- `layout.md` §5.2 EL0 path namespace (ring3.md §5).

## 6. Read further
- `docs/dev/OS/09_FILESYSTEM/vfs.md` (mechanism), `layout.md` (names), `unafs-records.md`, `btree.md`, `partitions.md`.
- `docs/dev/OS/10_INSTALL/partition-install.md`, `FLIGHT35-INSTALL.md`.
- `docs/dev/PREFS-SCHEMA.md`, `docs/dev/OS/05_USER_EXPERIENCE/PREFS.md`.
