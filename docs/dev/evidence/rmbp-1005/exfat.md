# EXFAT (B392) — a removable exFAT medium mounts read-only through a shared core

## Finding (read off FLIGHT25 and the code)
`fs/removable.rs` (USBSTOR) mounts only through `fat::mount_source`; on `Err` it sniffs the boot-sector OEM id and
prints `[volumes] not mounted … fs=exfat … reason=no-exfat-reader`. The tree has no exFAT code anywhere
(`unaos/libs/fs` holds only `unafs`). Peter's 64 GB card (SDXC, MBR part_type=7) is exFAT by the SD Association's
rule, so a medium most people own never reaches `/volumes`.

## The seam (R79)
Shared — a `no_std` + `alloc`, `forbid(unsafe_code)`, dependency-free core `unaos/libs/fs/exfat_core` (a root
workspace member, the kernel's path dep with `default-features = false`, the `midden_core` shape). It is written
from Microsoft's published exFAT specification (R83; no driver source read). The core never touches a device: it
asks a `SectorRead` trait for 512-byte sectors, so the host KATs read an image file and the kernel reads a USB disk
through the one code path. The kernel side is `fs/exfat.rs` (`//! CHARTER: Kernel — fs-core`): the sector adapter
over a `BlockSource`, a per-disk mount cache (the upcase table is read once per insert, not once per verb), and an
`ExfatBackend: VfsBackend` (read-only; every write verb answers the trait's `Unsupported` default and
`write_veto` names the reason). `fs/removable.rs` gains the exFAT arm: when `fat::mount_source` fails and the sniff
says `exfat`, the core mounts it and the volume takes its label from the label directory entry.

No knob: the removable path is unconditional on this tree (USBSTOR has none), and the reader only runs when a disk
that is not FAT and carries `EXFAT   ` attaches. Nothing runs at boot (R80).

## Milestones
- **M1** `exfat_core`: boot sector + the boot-region checksum sector, the FAT, the allocation bitmap (located,
  counted), the up-case table (decompressed, checksum-verified), directory entry sets (file + stream extension +
  file name; set checksum; name hash), cluster chains with NoFatChain, the label entry; read-only `open`, `read`,
  `read_dir`, `stat`; UTF-16 names → UTF-8 and case-insensitive lookup through the up-case table; volume location
  (superfloppy, GPT, MBR). `Volume::audit` is the spec checklist the metal `tests exfat` and the host KATs share.
  Host KATs: unit tests on synthetic bytes always; image KATs on the fixture `EXFAT_FIXTURE` names, made by
  `unaos/libs/fs/exfat_core/tests/mkfixture.sh` (`mkfs.exfat` + the host's exFAT FUSE writer; 4 MiB, in the
  scratchpad, never committed).
- **M2** `fs/exfat.rs` + the `removable.rs` arm: mount at `/volumes/<label>`, bind through `ExfatBackend`.
- **M3** witness `tests exfat`.

## Witness (a metal boot with the exFAT card in the hub reader)
`[volumes] mounted /volumes/<label> source=usb slot=<n> fs=exfat removable=1 ::` at attach, and on `tests exfat`:
`:: EXFAT: kat=<n>/<n> mounted=<label> files=<n> -> PASS ::` (SKIP with `reason=no-exfat-medium` when no exFAT
removable is mounted).

## Owed
Write (bitmap allocation, the entry-set rewrite and its checksum, the FAT chain when a contiguous file grows past
NoFatChain, the VolumeDirty flag); TexFAT (second FAT, NumberOfFats=2: read uses the active FAT only); a FAT-walk
cursor cache for long chained reads (each read walks the chain from its first cluster, one FAT sector per 128 links);
exFAT on aarch64's USB path (the Pi's loop runs `piusb27_service`, not `removable::service`).
