# INSTALL-CORE — the storage-agnostic installer engine

Status: QEMU-proven on a scratch block device (x86_64), 2026-07-18. Arc INSTALL-CORE.
Knob: `UNAOS_INSTALLDEMO=1` (feature `installdemo`), default OFF. Module: `crates/kernel/src/install/`.

## What it is

The installer engine is the RUNG-3 ENGINE of the installer line (`~/.claude/plans/unaos/future/unaos-installer.md`):
a **GPT writer + FAT32 formatter + extent-level copy-and-verify**, built and proven on a scratch block
device *before* it is ever pointed at a real card. It is the machinery an installer flow (enumerate
target → partition → format → copy the boot payload → content-verify → report) is built from.

Everything is expressed over one small abstraction, the **`InstallTarget` trait**, so the same engine
drives any block target: the QEMU scratch disk this arc proves it on, and — in later arcs — the Orin
microSD (SDMMC), the Pi `emmc2`, or an x86 USB stick.

External standards bind; everything else is ours to do right:
- **GPT** per the UEFI 2.x spec: protective MBR + primary/backup headers + partition entry array, with
  the mandated CRC-32/ISO-HDLC over the header (92 bytes, CRC field zeroed) and over the entry array.
- **FAT32** per the Microsoft FAT spec: BPB + FSInfo + two FATs + a FAT32 root cluster; the standard
  `fatgen` FAT-size computation.

## The safety discipline (never-touch-a-seated-card, from birth)

1. **Armed, not ambient.** The engine only runs when explicitly armed. In this arc the arm is the
   `installdemo` feature + the `UNAOS_INSTALLDEMO=1` knob, and the target is ONLY a dedicated blank
   scratch disk the build attaches over the usb-storage slot. The boot volume is a *separate* device
   (an `ide-hd` ESP), so the engine physically cannot reach it.
2. **Blank-check guard.** Before the first write, `blank_check` reads the target's leading 64 sectors
   and REFUSES (`InstallError::NotBlank`, no writes performed, honest serial line) unless they are
   blank. Any real partition table or filesystem writes into that window, so a non-zero byte there
   means "occupied — do not touch". The witness re-runs the guard *after* writing a GPT and confirms
   it now refuses — the discipline holds on a written volume, not just an empty one.
3. **Verify by content, always.** The GPT writer re-reads and re-CRCs everything it wrote before
   returning success (self-verify is part of the write API, not optional). The copy primitive re-reads
   *every written extent* and SHA-256-checks the reassembled bytes. A **negative test** flips one byte
   and proves the verifier REJECTS the corrupted content, then restores it and re-verifies.

## The `InstallTarget` trait

```rust
pub trait InstallTarget {
    fn sector_size(&self) -> usize;                 // 512 for every target this arc supports
    fn capacity_sectors(&self) -> u64;
    fn id(&self) -> String;                          // human identity for the witness log
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), InstallError>;
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), InstallError>;
}
```

`buf` lengths are a whole number of 512-byte sectors. `BlockTarget` is the one implementation this
arc ships: it wraps the in-tree single-device block layer (`drivers::block`), looping its
single-block `read_block`/`write_block` over a sector range. In `installdemo` mode that device is the
QEMU usb-storage scratch disk (see "Wiring" below).

The engine is **arch-neutral** — it drives only `drivers::block`, which exists on both arches — so the
whole module COMPILES on x86_64 and aarch64 under the `installdemo` feature (its `pub` API raises no
dead-code warning on aarch64). The witness driver `install::run_demo` is invoked only from the x86_64
boot path this arc, because the QEMU scratch disk lives on x86.

## Layout choices

GPT (512-byte logical sectors throughout; mirrors the host-side hand-written GPT in
`builder/src/vm_image.rs` so an image the kernel writes and one the builder ships are interchangeable,
and the in-tree FAT reader's `scan_gpt` mounts either):

| Region                | LBA                                   |
|-----------------------|---------------------------------------|
| Protective MBR        | 0 (single 0xEE partition, spans disk) |
| Primary GPT header    | 1                                     |
| Primary entry array   | 2 .. 33 (128 entries × 128 B = 32 sec)|
| First usable          | 34                                    |
| ESP (partition 1)     | 2048 (1 MiB aligned), ≤ 64 MiB        |
| Data (partition 2)    | 1 MiB-aligned after the ESP … last usable |
| Backup entry array    | total − 33 .. total − 2               |
| Backup GPT header     | total − 1                             |

- **Two partition entries** are written: an EFI System Partition (type `C12A7328-…`) and a Microsoft
  Basic Data partition (type `EBD0A0A2-…`), so the platform boot layout (a firmware-bootable ESP + a
  data area) has room from the very first write.
- GUIDs are **deterministic** (derived from fixed label seeds), so a given build writes a byte-stable
  partition table — reproducible and diffable.

FAT32 ESP:

| Field              | Value                                      |
|--------------------|--------------------------------------------|
| bytes/sector       | 512                                        |
| sectors/cluster    | 1 (extent == sector; simplest deterministic layout) |
| reserved sectors   | 32                                         |
| number of FATs     | 2 (every FAT mutation writes both copies)  |
| FAT size           | standard `fatgen` computation              |
| root cluster       | 2                                          |
| FSInfo / backup boot| sector 1 / sector 6 (+ backup FSInfo at 7)|

**Blank-precondition optimization.** Because the engine only ever runs against a blank, armed target
(the guard enforces it), the FAT + data region is already zeroed. The formatter therefore writes ONLY
the *defining* structures — boot sector, FSInfo, backup copies, reserved FAT entries — and leaves the
guaranteed-zero remainder untouched (an empty FAT is all-free = 0; an empty root cluster is all-zero).
A general-purpose formatter on unknown media would zero the FAT region explicitly; here the blank
contract makes it unnecessary and keeps the write count to a handful of sectors. The produced volume
is a full, valid FAT32 — the in-tree reader (`fs::fat::parse_bpb` / `scan_gpt`) mounts it, and that
mount + read-back is the formatter's interop self-check.

## The copy-and-verify primitive

`fat32::write_payload_file` writes a payload as an 8.3 file in the root directory, chaining data
clusters from cluster 3 (cluster 2 is the root dir), and RETURNS the exact byte extents written. The
verifier re-reads precisely those extents and SHA-256-checks the reassembled bytes against the source
digest. This is the bench law made into an API: verify by content, over exactly what was written.

Hashing (`install::hash`) carries self-contained, no_std CRC-32/ISO-HDLC and SHA-256 implementations
(the arch-neutral engine does not reach into the aarch64-private `sha256` in `arch/aarch64/syscall.rs`).
Both are gated by known-answer tests at witness time before anything relies on them.

## Wiring (the QEMU scratch disk)

`UNAOS_INSTALLDEMO=1`:
- The builder (`builder/src/main.rs`) creates a fresh, all-zero **128 MiB** scratch image in
  `target/installscratch.img` each run and backs the usb-storage slot with it. The boot ESP stays on
  the separate `ide-hd`, so the engine writes ONLY this scratch disk. This overrides `UNAOS_FATIMG`
  (the two are exclusive — the installer demo owns the block device).
- Why the usb-storage *slot* and not a genuinely-second drive: the block layer is single-device (a
  second usb-storage would need xHCI multi-device support, out of this arc's lane). Reusing the
  usb-storage slot as the scratch keeps the boot ESP cleanly separate while giving the engine a real,
  writable block target.
- `arroyo` and `builder/src/main.rs` both map the knob to the `installdemo` kernel feature (kept in
  sync, as with the other knobs). Default OFF ⇒ the module + its call site vanish; media byte-identical.

## Witness

`UNAOS_INSTALLDEMO=1 ./arroyo test 22` boots x86_64 headless and runs the engine end-to-end, emitting:

```
:: INSTALL: engine start — hash self-test (sha256 + crc32 KATs) PASS — armed target = QEMU QEMU HARDDISK (262144 x 512B sectors) ::
:: INSTALL: blank-check (pre-write) => BLANK, armed ::
:: INSTALL: GPT written + parse-back verified — ESP LBA 2048..133119, data LBA 133120..262110 of 262144 sectors ::
:: INSTALL: ESP formatted FAT32 — fat_sz=1016sec clusters=129008 data@vol+2064 ::
:: INSTALL: copied PAYLOAD.BIN (6007 bytes, 12 extents) ::
:: INSTALL: extent sha-verify (re-read every extent) => PASS ::
:: INSTALL: in-tree FAT mount + read-back of PAYLOAD.BIN (6007 B) => PASS ::
:: INSTALL: negative test — 1-byte corruption CAUGHT (verify REJECTED as it must) => PASS ::
:: INSTALL: restore + re-verify => PASS ::
:: INSTALL: blank-check (post-write) => NOT blank, engine would REFUSE => guard OK ::
:: INSTALL: gpt+fat32+copy verify => PASS ::
```

The final line is the load-bearing verdict. (The two opening prints are folded into ONE line on
purpose: this point is a serial burst boundary — the first prints after the block device enumerates —
where the console reliably drops the second of two back-to-back, pre-I/O writes; every later `INSTALL`
line is separated from the previous by real block I/O, which spaces them safely.)

## What this arc did NOT do (next rungs)

- No real-hardware write path (Orin SDMMC / Pi emmc2 / x86 USB stick) — the engine is target-agnostic
  and proven; wiring a real `InstallTarget` and cloning an actual boot volume is the next rung.
- The scratch disk is the *only* device the engine touches; it never clones the running system's boot
  tree yet (that is INSTALL-1 in the line seed).
