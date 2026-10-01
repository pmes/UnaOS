# UNAFSX86 — UnaFS mounts on the rMBP and becomes `/` when the boot disk carries a volume

Ledger: rmbp-ledger B298. Answers the audit (`rmbp-0929/AUDIT-HANDLERS-UNAFS.md` §2, row B290, and
the volume half of B294). Branch `exec-rmbp-unafsx86`, cut from 55352efc.

## Finding

`fs/unafs.rs` and `NativeBackend` were `cfg(target_arch = "aarch64")`, the kernel's `unafs`
dependency sat in the aarch64-only `[target...dependencies]` table, `unafs_state()` answered
`unbuilt` on x86 and `bind_root` therefore forced a FAT `/`. `/home/<user>` was `HOME/<8.3>` on FAT.
ROADMAP §1c SH-2 (UnaFS system volume as root, FAT demoted to the ESP shim) was open.

## The seam

**fs-core.** `unaos/libs/fs/unafs` is already the shared `no_std` core (the host `tools/unafs` CLI and
the kernel link the same on-disk format; it carries no arch cfg). The kernel stays the *fulfiller*
over its block layer through `fs/unafs.rs`'s SDSEAM handle routing. No second store, no new format.

## Milestones

* **M1 — the gates.** Cargo feature `unafs = ["dep:unafs"]`; the x86 dependency is `optional`.
  Every cfg that existed only because unafs was aarch64-only is now
  `any(target_arch = "aarch64", feature = "unafs")` — aarch64 keeps it unconditionally (so Pi and
  Orin images do not depend on a feature list to stay byte-identical in behaviour), x86 arms it with
  the knob **`UNAOS_UNAFS=1`** (arroyo `_feats`, builder, a `K8_FEATS` arm). The SDSEAM matches gain
  the `Ahci { port }` arm (read routed to `read_block_ahci_port`, write to the refusing
  `write_block_ahci_port`; the bind probe does NOT admit AHCI — a medium with no write opcode cannot
  host the read-write mount). `bench_ticks()` on x86 reads `arch::now_cycles()` (TSC) instead of a 0.
* **M2 — the root.** `unafs_state()`/`unafs_present()` answer from `locate_on` on x86 too, so
  `bind_root` mounts `NativeBackend` at `/` when the root disk (`pick_root`'s choice, unchanged;
  `UNAOS_ROOT_PREFER` stays about the disk) carries a volume; `/boot` and `/apps` stay the FAT ESP.
  `users::ensure_home` asks the mount table who owns `/`: on a native root it creates `/home` and
  `/home/<user>` through the VFS (`MountTable::create(.., Dir, ..)`); on FAT it keeps the 8.3 path.
  One witness at bind:
  `:: UNAFSX86: root=unafs|fat disk=<handle> blocks=<n> gen=<g> home=<path> -> PASS|SKIP ::`
  (decimal; `blocks`/`gen` are 0 on a FAT root; SKIP = FAT root because the disk has no volume).
* **M3 — media.** `UNAOS_UNAFS=1 ./arroyo esp-x86` additionally builds
  `target/unaos-x86-card.img`: GPT, p1 = EFI System (FAT32 holding the ESP + data trees — the
  bootloader still finds the ESP as partition 1), p2 = a `tools/unafs init` volume (size
  `UNAOS_X86_UNAFS_MB`, default 512, capped at 2048 MiB). `install ssd` does NOT yet mirror p2: its
  dry run says so in a line of its own when the image carries `unafs`.
* **M4 — fixture.** `tests unafs`: create `/home/.unafsx86` on the root, write, re-read through
  `with_unafs` (the remount half: drop the cached mount, rebind), set+get `owner` through the
  crate's existing attribute API, delete. SKIP on a FAT root.

## The contract PREFS consumes

The home path shape: **`/home/<user>` lowercase, on the UnaFS root**, created at the first login.
The upper-case 8.3 leaf (`HOME/<NAME>`) exists **only** on the FAT fallback (a card with no UnaFS
partition, or a build without `UNAOS_UNAFS`). Writes reach whichever backend owns `/` through the
mount table — PREFS writes `<home>/.config/unaos/preferences.toml` there without knowing which.

## Owed (said out loud)

* ATTRSURF (parallel) adds set/get/list/query to the VFS trait; this arc adds no attribute method
  (the fixture uses the crate API under `with_unafs`, the same as the existing K-series fixtures).
* `install ssd --write` does not mirror the UnaFS partition yet (the dry run says so).
* `USERS.DAT` stays on the FAT boot volume (`users::store_mount` is FAT-direct): the credential
  store's move is Holocron's arc (B296), not this one.
* The ACL store (`UNAFS.ATR`) and the x86 `U10_NAMES` ACL remain path/name keyed (B294's identity half).
* No metal boot yet (R78: the metal boot is the gate).
