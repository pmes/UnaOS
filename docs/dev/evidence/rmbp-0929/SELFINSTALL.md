# SELFINSTALL — the card installs itself onto the rMBP's internal SSD (ROADMAP §1c rung SH-3)

Direction (Peter, 2026-10-01): "what about self-hosting so we can build the OS with the OS and then boot the OS".

## Finding
No `install ssd` exists. The `install` verb (shell.rs INSTALLVERB) is partition-only by design (R25) and the
disk-wide `InstallTarget` (`BlockTarget`, install/mod.rs) refuses every SATA write (B91). The pieces for the
rest exist: the AHCI read arm is open under `ahci-write` (the census needs it), `block::write_sectors_granted`
is the one SATA write door, `clone::snapshot`/`write_snapshot` mirror a FAT tree, and the loader's root walk
(`fs/bootdisk.rs` `walk_and_witness`) already finds a SATA root by content (`push_ahci_sources`).

## Mechanism
- M1 `install ssd --dry-run`: `install/selfinstall.rs` `probe` classifies the first AHCI disk. blank = zero head
  or empty valid GPT; ours = every partition ESP-typed AND the FAT volume carries `EFI/BOOT/BOOTX64.EFI` +
  `kernel.elf` (what arroyo `esp_x86` stages); stranger = anything else. Prints
  `[install] target=ahci:<p> model= sectors= gpt=none|valid|unreadable verdict= plan=`.
- M2 `install ssd --write` (build needs `UNAOS_AHCI_WRITE=1`; knob already wired in arroyo, builder, k8-reach):
  stranger -> `[install] REFUSED ... saw: ...` and nothing written (R20). Else snapshot the running card's
  volume (found by `bootdisk::locate`, `clone::snapshot_capped`, caps raised to the 256 MiB x86 heap), check it
  has the boot files, mint a whole-disk grant (`partition::mint_disk_grant`, only from a Blank/Ours `Verdict`,
  refused if the boot volume is on that disk), `gpt::write_gpt_sized(512 MiB ESP, no data)`, then the existing
  `partition::write_partition` (zero FAT metadata, `format_esp`, `write_snapshot`, per-file `verify_extents`).
  `AhciDisk: InstallTarget` writes only through `write_sectors_granted`. The single `WriteGrant::new` site is now
  `partition::grant_range`, called by both minters, so the audit grep still shows declaration + one call.
- M3: `[bootdisk] root candidates: sdhc= ahci= usb= chose= prefer=` after the plan; `UNAOS_ROOT_PREFER=ahci|sdhc`
  -> cargo features `root-prefer-ahci|sdhc` -> `pick_root` (tail of bootdisk.rs). Three places: arroyo map,
  builder/src/main.rs read, k8-reach.registry row (+ banner-cert row for the line).
- `tests selfinstall` = `selfinstall::selftest` (the dry run), registered by `tests.rs::ensure_selfinstall`.

## Witness lines
`[install] target=ahci:0 model=... verdict=blank|ours|stranger plan=...` · per file `[install] file= bytes= ok= sha=` ·
`:: SELFINSTALL: files= bytes= ms= verified= -> PASS|FAIL ::` · `:: SELFINSTALL: dry-run ... -> PASS ::` (SKIP with no AHCI disk).
Spec pins: none new (QEMU has no default AHCI lane).

## Written
Boot 17/18 (rMBP, build `UNAOS_WC=1 UNAOS_INSTALLDEMO=1 UNAOS_AHCI=1 UNAOS_AHCI_WRITE=1`): from the desktop shell,
`install ssd --dry-run` must print `verdict=stranger` on the bench disk (Catalina) with `saw: foreign=…APFS…`, and
`install ssd --write` must REFUSE with `nothing was written (R20)`. On a blank/ours SSD: `:: SELFINSTALL: files=N
bytes=B ms=T verified=N -> PASS ::`. Every boot now shows `[bootdisk] root candidates: …`.
Not exercised: nothing here was compiled or run (R76/R78).
