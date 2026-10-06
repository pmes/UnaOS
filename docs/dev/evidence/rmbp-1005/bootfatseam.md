# BOOTFATSEAM (rmbp-ledger B453) — R99's sacred boot FAT enforced at the FAT layer; the direct writers on UnaFS

Cut from 82319dd6 (exec-rmbp-merge17). SECREVIEW F3 (MED).

## Finding
R99's gate (`bootfat::refuse` in `FatBackend::authorize_write`, SMALLFIX3's `bootfat::veto` in `FatBackend::write_veto`)
sits on the VFS path only. Code holding a raw `FatFs` never asks it: `fs/users.rs` (USERS.DAT, the password save, on
the store volume = the boot card on the rMBP, `source=sdhc` in f24/f25), `fs/holocron.rs` (`/HCRON/BTBOND.DAT` through
`fat::mount()`), `fs/fat.rs` `fatlfn_witness_once` (a test at boot, R80), `selfhost/extract.rs` (`src extract`). Each
already asks `FatFs::write_veto()` — which forwards to `BlockSource::write_veto` and knows nothing of R99.

## The seam (R79: fs-core, the kernel is the one owner of both stores)
1. `fs/sysleaf.rs` (new, `Kernel — fs-core`): kernel-owned leaves on UnaFS `/system` through the one mount table —
   publish (temp, read back, swap), read (refused unless the leaf carries `owner=kernel`), unlink. Every leaf it writes is
   stamped `owner=kernel`, so the native ACL (`read_authz` / `native_write_authz`) refuses every other principal.
2. USERS.DAT → `/system/USERS.DAT`; the holocron store → `/system/BTBOND.DAT` — only when the boot FAT is sacred (an x86
   native UnaFS root). The MIGRATION: the store loads from the FAT exactly as before (the bare-boot path, before the root
   binds); the first users pass after the root binds (`root_credential_ignition`, or the first save, whichever is first)
   adopts the UnaFS leaf when it exists, else writes the FAT-loaded table to it once. The FAT copy is never written again
   and stays for the installer's bare-boot path; the wire says so.
3. The FAT layer itself: `bootdisk::bind_root` hands bootfat the boot FAT's `BlockSource` (`arm_on`); `FatFs::write_veto`
   asks `bootfat::veto_source` first (every raw writer's existing question now answers R99), and `fat::write_sector` /
   `write_sectors` refuse a write to that source while the gate is shut (`bootfat::raw_guard`, counted: a writer that did
   not ask). `fat_unlock` opens both layers.
4. FAT-LFN: SKIPPED `reason=r99-sacred`. `src extract` onto the sacred FAT: refused by its existing veto (it names R99).
5. `fat_unlock` takes a typed `Writer` (`Installer` | `Updater`) — no other caller can name itself — and still asks
   `admin_authority` (SMALLFIX3); the fixture counts that it asked.

## Milestones (ordered so no commit refuses the password save)
- M1 — `fs/sysleaf.rs`; users and holocron stores seated on UnaFS with the one-time migration. Nothing refused yet.
- M2 — the FAT-layer gate (`arm_on`, `veto_source`, `raw_guard`), FAT-LFN `reason=r99-sacred`, typed `fat_unlock`.
- M3 — `tests bootfatseam`, SECURITY.md row.

## Witness (the next flight reads)
- `[users] seat store=unafs:/system/USERS.DAT src=<unafs|migrated|fresh> users=<n> (FAT copy read-only: installer bare-boot path, R99) ::`
- `[users] migrate USERS.DAT fat -> unafs:/system/USERS.DAT users=<n> -> <ok|FAILED reason>` (once, the first boot after the fold)
- `[hcron] seat store=unafs:/system/BTBOND.DAT src=<...> ::` (holocron knob)
- `[boot] fat raw write refused source=<src> lba=<n> site=<write_sector|write_sectors> (R99: sacred; a direct writer) ::` (must NOT appear)
- `tests bootfatseam` → `:: BOOTFATSEAM: direct_writers=0 users=unafs holocron=unafs lfn=skip fat_unlock=authority -> PASS ::`
