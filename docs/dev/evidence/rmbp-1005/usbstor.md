# USBSTOR (B384) — the boot medium is never displaced; a removable disk mounts when it attaches

## Finding (read off f25-boots.log and the code, not the row's hypotheses)
- **USBSLOT, confirmed.** `drivers/block.rs` `publish_usb_geometry_lun` (x86 arm): registry index 0 claims the
  GLOBAL slot unconditionally. On the rMBP the boot card is the INTERNAL Sdhc handle, so a stick at boot became
  `BlockSource::Default`. `fs/users.rs` `store_via_from` prefers `Global` whenever it exists, so the users store
  mounted the STICK (f25:437 `0 FAT volume(s)` on it) — `set-password … NOT written reason=volume` ×2 (f25:560,
  628). FRGUARD (f25:437) refused Default writes correctly; the fault is that the stick was ever in the global slot.
- **USBVOL, confirmed, plus a second cause.** `fs/bootdisk.rs` `survey()` caches the first walk that bound a root,
  so `others` (HOMESOIL) never sees a disk that attaches later; nothing listens for an attach after that. AND the
  flight-25 medium is a 64 GB SDXC card in the hub's reader (`USBLUN … "USB3.0 CRW -SD" capacity=124735488x512`)
  that `fat::volume_serials` found **0 FAT volumes** on (f25:437, both claims): a 64 GB card ships exFAT, and this
  tree has NO exFAT reader. So even the boot-time walk (stick in at power-on) gave it no `/volumes` point
  (`VOLUMES … removable=-`). The attach path below says so on the wire instead of being silent.

## The seam (R79)
Kernel — fs-core. No second store: the block registry (`USB_DISKS`, `USB_PUBLISH_GEN`) stays the one record of
what is attached, the mount table stays rebuilt per verb from `fs::bootdisk::bind`, and Quarry's existing
`volume_gen` (publish gen + `NS_GEN`) carries the live update. New file `fs/removable.rs`
(`//! CHARTER: Kernel — fs-core`) holds the attach/detach service; `bootdisk::bind` asks it for the removable
mounts instead of the boot-time cache.

## Milestones
- **M1 (USBSLOT)** `block.rs`: `pin_boot_medium_once()` (main loop, lock-free) proves once whether the boot volume
  serial lives on the Sdhc card; once pinned there, a USB publish never claims the global slot, and a stick that
  claimed it before the pin is EVICTED (it stays registry index 0 / `BlockSource::Usb`). `alternate_program_source`
  falls back to the USB handle so the wifi firmware-on-a-stick search keeps its second volume. Wire:
  `[block] boot medium=sdhc kept; usb slot=<n> separate (R95)`.
- **M2 (USBVOL)** `fs/removable.rs`: on every change of `USB_PUBLISH_GEN` re-read the registry; mount each live USB
  disk that is not the root disk (first FAT volume: superfloppy, GPT or MBR, through `fat::mount_source`) at
  `/volumes/<label or Untitled>` (unique against the fixed volumes), unmount what left, bump `NS_GEN`. A disk with
  no FAT says why, naming exFAT/NTFS by the boot-sector OEM id. `bootdisk::bind` (x86) skips USB `others` from the
  cached survey and mounts the live removable list instead.
- **M3 (witness)** `:: USBSTOR: boot_medium_kept=<0/1> stick_at_boot=<0/1> pw_write=<ok/refused> mounted=<label or -> hotplug=<ok or -> -> PASS|FAIL ::`
  printed once per removable change (an event line, R80) and by `tests usbstor`.

## Witness lines a metal boot should print (stick in from power-on, then out, then in)
`[block] boot medium=sdhc kept; usb slot=4 separate (R95)` · `[volumes] mounted /volumes/<LABEL> source=usb slot=4 fs=fat32 removable=1`
(or `[volumes] not mounted source=usb slot=4 fs=exfat reason=no-exfat-reader`) · `[login] set-password …` with no
`NOT written` · `[volumes] unmounted /volumes/<LABEL> reason=detached` · `:: USBSTOR: boot_medium_kept=1 … -> PASS ::`
· `VOLUMES … removable=<LABEL>`.

## Owed
exFAT reader (the flight-25 card); more than the first FAT volume per disk; an aarch64 hook (the Pi's loop runs
`piusb27_service`, not this); `BlockSource::Usb` index reuse after a detach (an open handle re-resolves through
the mount table by path, so it errors ENOENT, but a `FatBackend` held across a replug would name index 0 again).
