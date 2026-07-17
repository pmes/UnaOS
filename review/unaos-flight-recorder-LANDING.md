# FLIGHT-RECORDER landing report — UNAOS.LOG on the vm-image (hw-rmbp, R20)

## Verdict

**Full arc (M1–M3) landed on `hw-rmbp`, DONE gate green.** A remote tester who boots
the shareable `vm-image` in stock QEMU/UTM/VirtualBox/VMware — with ZERO UnaOS tooling
and no serial capture — now finds the whole boot log sitting on the image as a plain
file, **`UNAOS.LOG`** at the FAT root, ready to copy off and send back. Proven
end-to-end in stock QEMU+OVMF with no harness flags. Pairs with VMIMAGE-1.

Commits on `hw-rmbp` (base `main` `534c55b`), newest last:
- `77c61e3` M1 — capture ring + serial-seam tap
- `fef4ec5` M2 — flush to UNAOS.LOG from the x86 main loop (incl. the NotFat silent-skip)
- `02b8558` M3 — vm-image consumer docs (UNAOS.LOG + usb-storage attach) + MILESTONES
- `487c967` review fold — snapshot length under one lock (durability)

## What it does

Two x86-only halves, both riding proven seams (no new kernel subsystem, no `fat.rs`
edit, no format change):

- **M1 — capture** (`crates/kernel/src/flight_recorder.rs`, new). A fixed 64 KiB static
  byte ring. A THIRD additive tap on the single x86 serial print seam
  (`arch/x86_64/serial.rs::_print`), alongside `ftdi::mirror` and `selftest::capture`,
  copies each formatted line's bytes into it. Alloc-free, `try_lock` only, drop-on-full
  (counting dropped bytes), safe from the IRQ-masked print context, zero change to what
  is printed. **On by default (no knob)** — the whole point is a consumer captures the
  log with no flags; the capture is a bounded memcpy under a `try_lock`, and the ring
  holds the EARLIEST bytes (banner + self-tests), stopping rather than evicting the head.

- **M2 — flush.** `flight_recorder::service()` runs from both x86 main-loop bodies right
  after `fs::fat::probe_once()`. Once a block device is up it writes the ring to
  `/UNAOS.LOG` via the SAME public `fat.rs` entry points the shell's `write` uses
  (`find_located` / `create_in_root` / `delete_located` / `write_grow` — call-never-edit).
  Write-through and bounded (a stalled USB write is `FatError::Io`, never a hang);
  re-flushes on growth, throttled (`FLUSH_EVERY_ITERS`), so late lines reach the disk
  before a hard power-off without churning the FAT. Honest-and-silent: a non-FAT/absent
  volume is a QUIET skip (the normal case for the default `test`'s raw usb.img and for
  NOSTORAGE), a real write error is one witness line — never a panic, never blocks boot.
  The file carries a self-identifying header line (in the file only; the live serial
  stream is byte-unchanged).

- **M3 — proof + docs.** The vm-image consumer recipe (`README-VM.txt` baked into the
  image, root `README.md`, and the `arroyo vm-image` printed command) now attaches the
  `.img` over USB and explains UNAOS.LOG. `docs/MILESTONES.md` gets the newest-first entry.

aarch64 is byte-untouched: the module, the `_print` tap, and the two main-loop calls are
all `#[cfg(target_arch = "x86_64")]`.

## The one load-bearing design fact (resolved)

The kernel's block layer (`drivers/block.rs`) is **xHCI USB Mass Storage (BOT) only** —
there is NO IDE/SATA/AHCI/NVMe driver. VMIMAGE-1 booted the `.img` as an IDE/SATA disk,
which the kernel cannot write. So the flight recorder writes UNAOS.LOG **only when the
`.img` is attached over USB** (`qemu-xhci` + `usb-storage`); any attachment still BOOTS,
but USB is what lets the log be written back. This is a kernel-storage-architecture fact,
not a removable limitation (an AHCI/SATA driver is explicitly out of lane). The consumer
docs now recommend the usb-storage attachment; **M3 empirically confirms stock QEMU+OVMF
boots the single `.img` over USB and the kernel enumerates + mounts + writes it** — the
documented "OVMF-USB-boot destabilizes BOT reads" hazard did NOT bite here. UTM/VBox/VMware
USB-disk boot are attended (Peter's legs); the docs flag the USB requirement for the log.

The kernel FAT driver already mounts GPT+FAT32 (`fat::mount()` → `scan_gpt`), so no
fat.rs change was needed to find the volume. The vm-image uses the DEFAULT `esp-x86`
build (no `irqstorage`), so the flush uses the POLLED `fat.rs`→`block.rs` path (what the
shell/`probe_once` use), not the `irqstorage` service task.

## Gate results (verbatim)

- `./arroyo check` — ✅ x86_64 OK, ✅ aarch64 OK (aarch64 byte-untouched).
- Knob-OFF byte-identity: **N/A** — the capture is on-by-default (not knob-gated), by
  design (a consumer needs no flags). aarch64 is unaffected by construction (cfg-gated).
- `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test 200` — **0 FAIL, 24 PASS**,
  `:: FLIGHTREC: boot log -> UNAOS.LOG (15437 bytes) -> PASS ::`, 0 panic.
- `./arroyo test 40` — `MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED.`, 0 FAIL, 0 panic;
  flight recorder self-skips SILENTLY (raw non-FAT usb.img → NotFat → quiet skip, no line).
- `UNAOS_NOSTORAGE=1 ./arroyo test 40` — 11 PASS, 0 FAIL, 0 panic; service self-skips (no
  block device), 0 FLIGHTREC lines.

## M3 end-to-end proof transcript

Built `./arroyo vm-image` → `target/vm/unaos-x86-487c967.img` (tip). Booted in STOCK QEMU+OVMF
as a usb-storage device (`-device qemu-xhci -device usb-storage,drive=…,bootindex=0`),
NO arroyo harness, serial to a file, then powered off (killed). Serial:
```
[ INFO]: crates/bootloader/src/main.rs@280: UnaOS UEFI Bootloader Started
FS: FAT mounted: FAT32 vol@LBA2048 bps=512 spc=1 nfat=2 fatsz=1009sec … rootclus=2
:: FLIGHTREC: boot log -> UNAOS.LOG (9926 bytes) -> PASS ::
```
Host-mounted the `.img` after shutdown; root listing:
`EFI/  HELLO.BIN  README-VM.txt  UNAOS.LOG  hello.txt  kernel.elf`. `UNAOS.LOG` (11572
bytes) starts with the self-identifying header `:: UnaOS flight-recorder boot log
(UNAOS.LOG) ::`, followed by the kernel boot log and `-> PASS` witnesses; `README-VM.txt`
inside the image carries the updated UNAOS.LOG / usb-storage instructions. The round-trip
is closed.

## Review lens (thin-glue, durability-focused)

One lens at landing. The arc is thin glue (a ring tap + a call-never-edit reuse of the
shell's FAT write path) but M2 rides a write path, so the lens focused on durability. It
found and fixed one real wrinkle (fold `487c967`): `captured_len()` and `snapshot()` took
the ring lock separately, so a rare snapshot-time contention could persist a header-only
UNAOS.LOG marked as a full flush. `snapshot()` now returns the bytes AND the length under
one lock; `service()` records exactly what it wrote. No other must-fix. The NotFat
silent-skip (folded into M2) came from the first gate run — the default `test`'s raw
usb.img is not a FAT volume, and reporting that as "FAILED" was misleading; it is now a
quiet skip.

## Flags / notes

- **No metal media for this arc** (say so honestly): the `target/vm/` `.img` is the QEMU
  proof; nothing was staged to `~/unaos-bench/flash/rmbp/`. The recorder benefits a future
  attended rMBP sitting (a USB-booted single stick writes UNAOS.LOG on real Panther Point),
  but that rides an existing sitting; it is not a dedicated metal ask.
- **Consumer USB requirement** is the one thing for Peter to know: the log needs a USB
  attachment. Non-QEMU legs (UTM/VBox/VMware) USB-boot are attended and documented, not
  gated here.
- `arroyo`/`builder`/`vm_image.rs` are host-lane glue the integrator reconciles at merge;
  I touched only the vm-image consumer recipe + the printed command.
