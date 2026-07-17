# VMIMAGE-1 landing report — `arroyo vm-image`: a shareable bootable disk image (hw-jetson, host lane)

## Summary

**Full arc (M1–M3).** `./arroyo vm-image` packages the SAME x86 build products
the `esp-x86` path proves into **one self-contained, distributable disk image**:
`target/vm/unaos-x86-<git7>.img` — a GPT disk with a single FAT32 EFI System
Partition carrying the boot tree (`EFI/BOOT/BOOTX64.EFI` + `kernel.elf` + the
small companion files) and a `README-VM.txt`. It boots in stock QEMU+OVMF, and
(attended, Peter's legs) UTM / VirtualBox / VMware with EFI enabled, with **zero
UnaOS tooling on the consumer's machine**.

No parallel build logic: packaging reuses the builder's existing
kernel/bootloader/ESP path and runs strictly after it. Pure-Rust packaging (the
`fatfs` crate + a hand-written GPT/protective-MBR) — no `mkfs.vfat`/`hdiutil`.
Deterministic disk/partition GUIDs from the git hash. Own output path only
(`target/vm/`), so the `target/` state the test harness uses is never disturbed.

## What landed

### The subcommand + packaging path
- **`unaos/arroyo`** — new `vm-image` (aliases `vm`, `vm_image`, `vmimage`)
  subcommand + `vm_image()` function. Computes `<git7>` from `git rev-parse`,
  runs `build_user_hello_x86`, then `UNAOS_VM_IMAGE=1 UNAOS_VM_GIT7=<git7> cargo
  run` in the builder. On success it prints the image path, size, sha256
  (detects `shasum` then `sha256sum`; honest skip note if neither exists), and
  the exact stock-QEMU boot command. Added to the usage line.
- **`unaos/builder/src/main.rs`** — a `UNAOS_VM_IMAGE` early-return mode added
  right before the existing `UNAOS_PACKAGE_ONLY` block. It fires AFTER the ESP
  tree is fully packed (same products as `esp-x86`), calls `vm_image::build(…)`,
  and stops — no QEMU, no rebuild.
- **`unaos/builder/src/vm_image.rs`** (new) — the packager:
  - FAT32 built in memory with the `fatfs` crate, `FatType::Fat32` forced (so a
    64 MiB volume never degrades to FAT16), volume label `UNAOS`; the ESP tree is
    copied in (sorted, `._*`/`.DS_Store` skipped for a clean distributed tree),
    plus `README-VM.txt` at the root.
  - GPT + protective MBR **hand-written** for deterministic GUIDs: `derive_guid`
    seeds disk/partition GUIDs from a label + the git hash (RFC-4122 variant/
    version nibbles stamped). Primary header @LBA1, 128-entry array @LBA2, ESP @
    LBA2048 (1 MiB aligned, 64 MiB), backup array + header at the disk tail.
    Header + entry-array CRC-32s via `crc32fast`.
- **`unaos/builder/Cargo.toml`** — added `fatfs` (default-features off; `std` +
  `alloc`) and `crc32fast`.

### Docs (M3)
- `README-VM.txt` — generated INTO the image root (`readme_text` in
  `vm_image.rs`): QEMU+OVMF one-liner, UTM/VirtualBox/VMware click-paths, what to
  expect, where to report. One screen.
- Root `README.md` — new **"Try UnaOS in a VM"** section.
- `docs/MILESTONES.md` — VMIMAGE-1 entry (newest first).
- This landing report.

## Gate transcript

- **`./arroyo check`** — ✅ x86_64 OK, ✅ aarch64 OK (proves zero kernel surface).
- **`./arroyo test 40`** — `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET
  ACQUIRED. <<<`, zero FAIL lines (existing x86 QEMU suite unbroken).

### M1 — image checks (built image `unaos-x86-e8f2c2b.img`)
- GPT primary header @LBA1: `EFI PART`, revision `0001 0000`, header size `5c`
  (92) — valid.
- Protective MBR partition type byte @0x1c2 = `ee` — GPT protective.
- FAT32 volume mounts host-side (macOS `hdiutil`/`diskutil`), tree =
  `EFI/BOOT/BOOTX64.EFI`, `kernel.elf`, `HELLO.BIN`, `hello.txt`, `README-VM.txt`.
- Byte-identity vs build products: `cmp` `BOOTX64.EFI` ==
  `target/x86_64-unknown-uefi/release/bootloader.efi` (IDENTICAL); `cmp`
  `kernel.elf` == `target/x86_64-unaos/release/unaos-kernel` (IDENTICAL).

### M2 — stock QEMU+OVMF boot (no arroyo harness flags)
Consumer command (split OVMF needs a writable vars store as a second pflash):
```
qemu-system-x86_64 -machine q35 -m 1G \
  -drive if=pflash,format=raw,readonly=on,file=<OVMF_CODE.fd> \
  -drive if=pflash,format=raw,file=<OVMF_VARS.fd copy> \
  -drive format=raw,file=target/vm/unaos-x86-<git7>.img \
  -serial stdio
```
Serial evidence (head + body):
```
BdsDxe: loading Boot0001 "UEFI QEMU HARDDISK QM00001 " from PciRoot(0x0)/Pci(0x1F,0x2)/Sata(0x0,0xFFFF,0x0)
BdsDxe: starting Boot0001 ...
[ INFO]: crates/bootloader/src/main.rs@280: UnaOS UEFI Bootloader Started
[ INFO]: crates/bootloader/src/main.rs@360: GOP: 30 modes (firmware current 1280x800)
...
:: KERNEL HEAP ALLOCATED ::
ACPI: 1 CPU(s) discovered ...
:: VUG Init ::
:: FB Size: 1280x800 (stride 1280) ::
:: Framebuffer painted #1E1E1E ::
```
OVMF's BdsDxe booted the removable-media fallback `\EFI\BOOT\BOOTX64.EFI` off
the image's GPT/FAT32 ESP; the UnaOS bootloader + kernel came up through heap /
ACPI / VUG init / framebuffer paint / self-tests — a complete boot from the
distributable image with no UnaOS tooling in the invocation.

## Notes / flags

- **sha256 host tool.** The image sha256 is computed in `arroyo` via `shasum`
  (macOS) or `sha256sum` (Linux), detected at runtime with an honest skip note
  if neither exists. The image packaging itself (GPT + FAT32) is fully in-Rust;
  only the printed checksum uses a host utility.
- **OVMF vars store.** The `README-VM.txt` / README one-liner shows the minimal
  `OVMF_CODE.fd` pflash. Split-firmware OVMF builds (macOS Homebrew, modern
  Linux) also need the writable `OVMF_VARS.fd` as a second pflash unit to boot
  (used in the M2 run above); this is standard OVMF usage, not UnaOS-specific.
- **UTM / VirtualBox / VMware legs** are attended items for Peter (documented
  click-paths in `README-VM.txt`), not part of this arc's gate.
- Image size 64 MiB ESP + GPT overhead ≈ 68 MiB; the boot tree is ~0.7 MiB, so
  the volume is comfortably above the FAT32 floor.
