# INSTALL-CORE — the storage-agnostic installer engine

Status: QEMU-proven on a scratch block device (x86_64), 2026-07-18. Arc INSTALL-CORE.
Wired to the Orin microSD (metal-pending) by **INSTALL-1**, made a real self-clone by **INSTALL-2**, and given
multi-block SD transfers + multi-cluster directories by **ORIN-SDMMC-3**
(§INSTALL-1 / §INSTALL-2 / §ORIN-SDMMC-3 below; `UNAOS_INSTALL_TARGET_SD=1`).
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

## §INSTALL-1 — the engine wired to the Orin microSD (the first real installer flow)

**Landed 2026-07-18** (aarch64/tegra; metal-pending). `UNAOS_INSTALL_TARGET_SD=1` (feature `install_target`,
⇒ `sdmmc_arm` ⇒ `sdmmc`), default OFF. Glue: `crates/kernel/src/arch/aarch64/sdmmc_tegra.rs`
(`SdInstallTarget` + `install_to_sd`). Full aarch64 detail: `arch_arm64.md` §ORIN-INSTALL-1.

INSTALL-1 points the (unchanged) engine at a REAL card for the first time: boot from the USB stick, install
UnaOS onto the seated microSD from inside UnaOS. The engine's `InstallTarget` trait is the whole seam — the SD
side implements it over the rung-2 armed single-block CMD24/CMD17 path, and the engine's `write_gpt` /
`format_esp` / `write_payload_file` / `verify_extents` run verbatim.

**Additive-only engine changes** (write/verify semantics untouched):
- `verify_extents` is now `pub` so the SD flow verifies through the SAME primitive the x86 witness trusts.
- `fat32::blank_region_sectors(esp_sectors)` — a new pub helper returning the exact leading-ESP sector count
  (reserved + both FAT copies) the formatter's blank-precondition requires to be zero. The demo target is
  always blank so it never zeroes; a real (possibly non-blank) card must zero exactly this region first.

**Escalation ladder (three gates).** Unlike the engine demo's blank-only `blank_check` refusal, an installer
handles a non-blank card: it does not refuse, it **announces what it will destroy** (sector-0 class + card
CID) behind a third `install_target` destructive-confirmation gate, then zeroes the metadata region and runs
the flow: GPT → zero-ESP-metadata → FAT32 → payload copy → sha extent-verify → `SD install … => PASS`.

**Payload (M2).** At the pre-xHCI-takeover install site the USB boot stick is not yet a block device, so
self-read of the running boot volume is unreachable this arc; v1 writes a generated `UNAOS.IMG` marker and the
**self-clone is the named follow-up, INSTALL-2**.

**Witness.** On virt (both arches) the flow is compiled-present-metal-only (one honest line). On x86 the
`UNAOS_INSTALLDEMO` witness above still covers the engine end-to-end. The tegra flow's **first execution is
the attended Orin sitting** (runbook `scripts/orin-sdmmc1-bench.md`, install leg; landing
`review/unaos-install1-LANDING.md`).

## §INSTALL-2 — the self-clone: the installer copies the running system's real boot payload

**Landed 2026-07-18** (aarch64/tegra; metal-pending). Same `UNAOS_INSTALL_TARGET_SD=1` gate. Glue:
`sdmmc_tegra.rs` (`sdmmc_install_from_usb` + the rewritten `install_to_sd`/`install_flow` + `copy_dir`);
engine: a new `TreeWriter` in `install/fat32.rs`. Full aarch64 detail: `arch_arm64.md` §ORIN-INSTALL-2.

INSTALL-2 replaces INSTALL-1's synthetic `UNAOS.IMG` marker with the **real thing**: the installer mounts the
USB boot stick's own ESP and mirrors its boot tree onto the microSD ESP, every file sha-extent-verified.

**Position adjudication (INSTALL-1's named blocker, resolved).** The install act is split: the read-only
census still runs pre-JB2b and now **stashes** the card identity; the destructive install is **deferred** to
`sdmmc_install_from_usb`, called from the boot sequence right after the JB2b pump window — the earliest
position where the USB stick is a block device (`drivers::block::info()` is `Some`), the SDMMC MMIO is still
mapped, and the core is still at EL2 (timer live for the SD bounded waits). No self to clone (no card stashed,
or stick not enumerated) → honest SKIP, nothing destructive. See §ORIN-INSTALL-2 for the full constraint proof.

**Additive engine change — the single-FAT-sector bound lifted.** INSTALL-1's `write_payload_file` (kept
verbatim; still the x86 witness's path) capped a file chain at FAT sector 0 (≤125 clusters ≈ 64 KiB). INSTALL-2
adds a `TreeWriter` (additive):
- a running **free-cluster cursor** (many files/subdirectories allocate distinct chains);
- `set_fat_run` links a chain across **every FAT sector it touches, in both FAT copies** — a multi-MB
  `kernel.elf` links correctly (multi-FAT-sector chains, the flagged extension, implemented additively);
- **directory clusters built wholly in memory** then written once, so a stale data cluster on a non-blank card
  never leaks bytes into a directory (each dir assumed ≤ one cluster — the boot tree is; overflow = honest
  error, not truncation).

Verify discipline is unchanged: every copied file is re-read off the card and SHA-checked through
`verify_extents`, and the flow prints a **per-file `sha256=… VERIFIED` manifest** — the installer's
content-verify IS the bench's content-verify, now native.

**Flow:** GPT → zero-ESP-metadata → FAT32 → **mount USB stick + clone its boot tree file-by-file** → per-file
sha manifest → `ORIN-INSTALL-2 SD install — gpt+zero+fat32+clone(N files) verify => PASS`.

**Witness.** Virt: one honest metal-only line (both arches). x86 `UNAOS_INSTALLDEMO` still covers the engine
end-to-end (the `TreeWriter` additions do not perturb it — `write_payload_file` is unchanged). First execution
is the attended Orin sitting (runbook install leg; landing `review/unaos-install2-LANDING.md`).

## §ORIN-SDMMC-3 — multi-block SD transfers + multi-cluster directories (INSTALL-2's perf/size follow-ups)

**Landed 2026-07-18** (aarch64/tegra SD path metal-pending; the multi-cluster-directory logic proven on the x86
`installdemo` witness). Same `UNAOS_INSTALL_TARGET_SD=1` gate. Glue: `sdmmc_tegra.rs` (multi-block primitives +
`copy_dir`); engine: additive `TreeWriter` methods in `install/fat32.rs`. Full aarch64 detail: `arch_arm64.md`
§ORIN-SDMMC-3. Closes two INSTALL-2 follow-ups:

- **Throughput — multi-block CMD18/CMD25.** New `read_blocks_at`/`write_blocks_at` move a run of contiguous
  blocks in one command (block-count in `BLKSIZECNT[31:16]`, Transfer-Mode Block-Count + Multi-Block bits,
  **auto-CMD12** completion — the controller issues STOP itself, no second round-trip). `SdInstallTarget` loops
  a **bounded 64-block (32 KiB) chunk**, keeping single-block CMD17/CMD24 as the 1-block/metadata fallback.
  `TreeWriter::write_file` writes each file's contiguous chain in **one multi-sector call**, so a real
  `kernel.elf` copies as a few CMD25 bursts, not one CMD24 per 512 bytes. The **rung-2 witness ladder stays
  single-block** — its metal-verified semantics are unchanged.
- **Size — multi-cluster directories.** `TreeWriter` gains `alloc_dir_clusters`/`write_dir_image`/`reserve_root`
  and `dir_clusters_for_slots`; a directory's image is built wholly in memory across its whole (contiguous)
  cluster chain and written once. The INSTALL-2 >16-entry `NoSpace` is lifted — `NoSpace` now means the volume
  is genuinely full. `put_dir_entry` takes a `&mut [u8]` slice (the whole image).

**Witness (x86 `UNAOS_INSTALLDEMO`).** A final step builds a `SUB/` directory of 20 files (22 slots → 2
clusters) through the `TreeWriter`, then the in-tree FAT reader mounts, walks `SUB/`'s cluster chain, and
re-reads + SHA-verifies every file: `:: INSTALL: multi-cluster dir — SUB/ 20 entries across 2 clusters, all
re-read + sha-verified (dirs=1) => PASS ::`. Byte-identity: the `sdmmc_arm` binary carries zero
`ORIN-SDMMC-3`/`mb: CMD18`/`mb: CMD25` strings and rebuilds identical; the `install_target` binary carries
them. Landing: `review/unaos-orin-sdmmc3-LANDING.md`.

## What later rungs still owe

- **Bootability:** the cloned card carries a faithful `/EFI/BOOT/BOOTAA64.EFI` + `/kernel.elf` tree; making the
  Orin actually boot from it (GPT ESP type/attributes, firmware boot-order) is the next rung's metal question.
- **Cross-platform:** the same `InstallTarget`/`TreeWriter` generalizes to Pi `emmc2` and an x86 USB stick.
- **Throughput:** multi-block CMD25/CMD18 on the SD path (single-block is correct but slower on the zero pass
  and the per-cluster payload writes).
## §INSTALL-PI — the engine wired to the Pi 4 emmc2 microSD (the first LIVE, benchless install)

**Landed 2026-07-18** (aarch64/Pi bare-metal; QEMU-live). `UNAOS_PIINSTALL_CONFIRM=1` (feature
`piinstall_confirm` ⇒ `piinstall_arm` ⇒ `piinstall` ⇒ `baremetal`), default OFF. Glue:
`crates/kernel/src/install/pi.rs` (`EmmcInstallTarget` + the three-gate flow). Called from the Pi BSP boot
path (`main.rs`, right after `emmc2::probe`). Full aarch64 detail: `arch_arm64.md` §INSTALL-PI.

**Why this is the first LIVE install (no bench wait).** Unlike the Orin flow (§INSTALL-1), which is metal-only
because QEMU models no Tegra234 SDMMC, QEMU's `raspi4b` **models the BCM2711 SD controller and attaches a real
emulated card image**. So the in-tree `drivers::emmc2` read/write path this drives is genuinely exercised, and
the full engine flow — GPT → FAT32 → payload copy → sha extent-verify — runs and PASSes under CI-grade QEMU.
This is the installer engine's first complete end-to-end execution against a real (emulated) card.

**`EmmcInstallTarget`.** An `InstallTarget` over `drivers::emmc2`: `read_sectors` / `write_sectors` loop the
proven single-block `read_block_512` / `write_block_512` (CMD17/CMD24) primitives (the same read path M6g
census uses and the write path U9/U10 exercised). No engine change was needed — `write_gpt` / `format_esp` /
`write_payload_file` / `verify_extents` and the `blank_region_sectors` helper (added by INSTALL-1) run verbatim.

**Escalation ladder (three gates), mirroring `sdmmc/sdmmc_arm/install_target`:**
- **Gate 1 `piinstall`** — census the seated card (read-only) + announce identity/capacity/sector-0 class.
- **Gate 2 `piinstall_arm`** — arm the write path: a NON-destructive scratch write/verify/restore ladder on the
  card's LAST block (stashed first, restored after, every step verified; REFUSED if a GPT is present, whose
  backup header lives in that block). Proves the installer can write+verify without repartitioning.
- **Gate 3 `piinstall_confirm`** — the destructive-confirmation gate: the ABOUT-TO-DESTROY line, then the full
  GPT → zero-ESP-metadata → FAT32 → payload → sha extent-verify install, ending
  `:: INSTALL: pi emmc2 gpt+fat32+copy verify => PASS ::`.

**QEMU witness (the benchless first).** `raspi4b` has one SD slot, so the install-witness run is its OWN QEMU
invocation with a **dedicated BLANK scratch image** in the slot — never the `kernel8-test` battery fixture
(which carries HELLO.BIN / the unafs volume the battery reads back). `./arroyo kernel8-install [secs]`
(function `install_pi`) arms all three gates, generates a blank 128 MiB scratch image, boots against it, then
**re-reads the scratch image ON THE HOST** (protective-MBR 0x55AA + 0xEE, primary GPT `EFI PART` @LBA1, FAT32
boot-sector `FAT32` fs-type + `UNAOS` volume label + 0x55AA at the ESP's first LBA) — the installer's claim
verified from OUTSIDE the kernel. Both the in-kernel PASS line and the host-side `HOST-VERIFY: PASS` are green.

**Payload (M2 adjudication).** A Pi "install" payload is ultimately the boot volume's FAT files (kernel8.img /
start4.elf / config.txt — what the GPU ROM loads). At the pre-shell BSP call site those are not reachable as a
readable clone source, so v1 writes a generated `UNAOS.IMG` marker (the honest-and-sufficient choice for the
QEMU witness) and the **self-clone of the boot FAT files is the named metal follow-up** (the Pi analogue of
INSTALL-2). **Metal note:** on real hardware the seated card IS the running system's card — the three gates +
about-to-destroy announcement are exactly the guard for that; a metal install leg wants a dedicated erasable
card, never the boot card.

**Knob-off identity.** `piinstall*` default OFF ⇒ the `install/pi` module, the `main.rs` call site, and the
arch-neutral engine all compile out; all machine code + data are unchanged and the `kernel8-test` battery is
0 FAIL. As with PI-USB, the only possible delta from baseline is embedded panic-`Location.line` u32s shifted
by the 8-line gated insertion in `kernel_main` — a source-line number, never code or behavior.

## What later rungs still owe

- **INSTALL-2 (self-clone):** copy the running system's own boot volume as the payload (needs the boot media
  readable as a block device at the install site — a post-takeover install position or a second block backend).
  The Pi analogue: clone the boot FAT files (kernel8.img/start4.elf/config.txt) rather than a marker.
- **Cross-platform:** the same `InstallTarget` generalizes to the x86 USB stick (installer_engine
  line seed rung 4); the Pi `emmc2` rung landed as §INSTALL-PI.
- **Throughput:** multi-block CMD25/CMD18 on the SD path (single-block is correct but slower on the zero pass).
- **Metal SD throughput:** the multi-block CMD18/CMD25 path is compiled + verified off-metal (QEMU models no
  Tegra234 SDMMC); its first metal exercise is the attended Orin sitting.
