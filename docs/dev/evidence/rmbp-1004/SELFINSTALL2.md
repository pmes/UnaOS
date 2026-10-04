# SELFINSTALL2 — the installer mirrors the UnaFS rung and gets its first shared core (B310; ticks audit B296)

Seam: `CHARTER: Amber Bytes — shared-core`. Branch `exec-rmbp-selfinstall2`, cut from b42b87cc.

## Design (written before the code)

**Finding.** The disk-layout logic of the installer ensemble existed three times with no shared code:
the kernel's `install/gpt.rs` (writer + reader + type-GUID editor, CRC via `crate::hash::crc32`), the
host's `unaos/scripts/make-x86-card.py` (a hand-written Python GPT writer for the UNAFSX86 card image),
and `handlers/amber_bytes` (none — its README says GPT/MBR editing is not implemented). Further GPT
*readers* live in `fs/fat.rs` (`scan_gpt`), `drivers/ahci.rs`, `arch/aarch64/sdmmc_tegra.rs`, the unafs
crate's `adapter.rs::parse_partitions`, `builder/src/vm_image.rs` and `scripts/make-gpt-fixture.py`.
And `install ssd --write` lays ONE partition: the card's p2 (the UnaFS root UNAFSX86 made) is not
mirrored, so the installed SSD boots with a FAT root.

**Seam.** `unaos/libs/sys/amber_core` — `#![no_std]` + alloc, `#![forbid(unsafe_code)]`, zero deps, the
`midden_core` shape (root-workspace member, kernel path dep). It holds the pure disk-layout logic both
rings need: CRC-32, GPT encode/decode (protective MBR, headers, entry array, LBA bounds), the partition
`Plan` (`Plan{disk_sectors, parts:[{kind: Esp|UnaFS|Data, first, last, guid, name}]}`) and its `Display`,
the FAT32 BPB/FSInfo layout the kernel writes (encode only), and the `ClonePlan` (source span → target
span, bytes, ETA). Callers: the kernel's `install/gpt.rs` + `install/fat32.rs` (thin), a new host tool
`tools/una-card` (REPLACES `make-x86-card.py`: same core lays the card image's GPT), and
`handlers/amber_bytes` (`amber_bytes gpt show <image>` — CLI only, no bus).

**Milestones.**
- M1 the core + KATs (golden GPT reproduced byte-for-byte from the retired Python writer), kernel
  `gpt.rs` thin, `tools/una-card`, `amber_bytes gpt show`.
- M2 `install ssd --write` lays ESP + UnaFS from `amber_core::Plan`, mirrors the ESP files (as before)
  and sector-clones the UnaFS span (fenced on the source's root block: a commit during the copy fails
  the clone), then mounts the copy through the write grant and runs `unafs::fsck(false)`. Refuses the
  boot disk (selfguard / `mint_disk_grant`) and an existing UnaFS volume on the target without
  `--force`. The dry run prints the whole plan, the clone byte count and an ETA from a measured read.
- M3 `video/instgui.rs`: key `i` on the chooser shows the SSD plan, text from the same `Plan` Display.
- M4 `tests install`: planner on a synthetic disk + the core's GPT KATs in-kernel, no writes.

**Witness.**
`:: SELFINSTALL2: plan=<n parts> esp=<MiB> unafs=<MiB> cloned=<MiB> fsck=<ok|skip|fail> boot_pick=<ahci|sdhc> -> PASS|DRY|FAIL ::`

**Owed (stays owed).** The SSD's UnaFS is NOT lazily bound as `/` on the next boot: `bind_probe_admitted`
refuses the AHCI handle because the shared mount is read-write and SATA ordinary writes are refused
(B91). `boot_pick` names the FAT root pick the next boot makes (`UNAOS_ROOT_PREFER`); a native `/` from
the SSD needs a granted AHCI write path for the shared mount (a later rung). The other GPT readers listed
above are not yet callers of the core. Geode/Vein/Principia/Holocron/Comscan legs: untouched.
