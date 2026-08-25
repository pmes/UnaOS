# INSTALL-CORE — the storage-agnostic installer engine

Status: QEMU-proven on a scratch block device (x86_64), 2026-07-18. Arc INSTALL-CORE.
Wired to the Orin microSD (metal-pending) by **INSTALL-1**, made a real self-clone by **INSTALL-2**, and given
multi-block SD transfers + multi-cluster directories by **ORIN-SDMMC-3**
(§INSTALL-1 / §INSTALL-2 / §ORIN-SDMMC-3 below; `UNAOS_INSTALL_TARGET_SD=1`). Wired to the Pi 4 `emmc2` (the
first LIVE, benchless install) by **INSTALL-PI**, and made a real self-clone there by **INSTALL-PI-2**
(§INSTALL-PI / §INSTALL-PI-2 below; `UNAOS_PIINSTALL_CONFIRM=1`).
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

**Flow:** UnaFS sizing gate 1 (pre-GPT, whole-card) → GPT → zero-ESP-metadata → FAT32 → **UnaFS sizing gate 2 +
format partition 2 as a native UnaFS volume + mount it back** → **mount USB stick + clone its boot tree
file-by-file** → per-file sha manifest → `ORIN-INSTALL-2 SD install — gpt+zero+fat32+clone(N files)+unafs
verify => PASS — p1 UNAOS-ESP = FAT32 (N files sha-verified) · p2 UNAOS-DATA = UnaFS v5 (131072 blk / 512 MiB,
mounted back off the card)`.

**The second volume (TEGRA-UNAFS-FMT).** INSTALL-2 originally carved `UNAOS-DATA` and left it raw; it is now
formatted as a **native UnaFS volume**, through a private `unafs::storage::BlockDevice` adapter over
`SdInstallTarget`'s already-armed CMD24/CMD25 primitives (`arch/aarch64/sdmmc_tegra.rs`, `install_target`-gated).
The adapter is never registered as a `drivers::block` backend, so the unconditional
`drivers::block::write_block_tegra_sd` refusal is untouched and the card's only write door is still the
`sdmmc` → `sdmmc_arm` → `install_target` ladder.

- **Sizing.** The volume is **capped at 512 MiB (131,072 blocks)** inside whatever span p2 has. Three reasons,
  all in `sdmmc_tegra.rs`'s section header: the in-RAM refcount map costs 8 B per 4096 B block against a 48 MiB
  aarch64 heap (a full-span volume on the bench card would want 1.24× the whole heap; at the cap it is 1 MiB =
  2.0 %); the boot-path probe-mount re-reads every refmap leaf on every `sdmmc` boot through a single-sector
  adapter, and leaves scale with the volume; and 131,072 stays inside `MAX_BLOCK_COUNT_ONE_LEVEL`, keeping the
  refcount map single-level. `UnaFS::mount` sizes its map from the **superblock**, not the partition span, so a
  capped volume inside a larger partition mounts correctly. The cap is a policy constant in one place.
- **Ordering.** `unafs_sizing_guard` runs once on the whole-card capacity **before the first byte of the GPT**
  (`SIZING-GATE-1`) and once on the real p2 span (`SIZING-GATE-2`). What that buys is ordering — an over-large
  geometry is refused while the card is still exactly as it was found, rather than on a half-installed card. It
  is **not** a proof that the format cannot fail for heap reasons: the comparison is against total `HEAP_SIZE`,
  never free heap, and `RefMap::try_new` still needs two contiguous runs out of a linked-list allocator. A
  format-time heap failure fails the install, closed.
- **Verification is a mount.** The step re-reads the static superblock for a precise diagnostic, then calls
  `UnaFS::mount` on a fresh handle over the same span — which reads and bound-validates the root record, the
  imap index and leaves, the refmap index and all 128 refmap leaves, and rebuilds the refcount map from the
  card — and then `ls(ROOT_INODE_ID)` walks the root directory through that reconstructed state. Checking a
  superblock and a root record alone would assert far more than two blocks can carry. The mount is read-only by
  construction (a fresh volume's reclaim queue is empty, so `reclaim_drain` returns without committing) and its
  1 MiB is the same 1 MiB the format just released. Witness terminator: `=> UNAFS-VERIFIED ::`, deliberately
  distinct from the per-file `=> VERIFIED ::`.
- **Floor.** A p2 span under **12 blocks** is an honest SKIP, not a failure — the install continues and still
  ends `=> PASS`, with the verdict naming `p2 UNAOS-DATA = NO NATIVE VOLUME`. 12 is measured against this build
  of the crate: 1–2 blocks fail `Superblock::validate` and 3–11 fail the format commit with `NoSpace`. The
  floor is load-bearing because `install/gpt.rs` really can emit a **1-sector** data partition (a card of
  exactly 133,154 sectors puts `data_first == data_last == 133,120`), which is 0 whole blocks.

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

**Payload (INSTALL-PI v1 → self-clone).** INSTALL-PI v1 wrote a generated `UNAOS.IMG` marker, because at the
pre-shell BSP call site a readable clone source was thought unavailable; the real self-clone was flagged as the
named follow-up. **§INSTALL-PI-2 below replaces the marker with the real self-clone.** **Metal note:** on real
hardware the seated card IS the running system's card — the three gates + about-to-destroy announcement are
exactly the guard for that; a metal install leg wants a dedicated erasable card, never the boot card.

**Knob-off identity.** `piinstall*` default OFF ⇒ the `install/pi` + `install/clone` modules, the `main.rs`
call site, and the arch-neutral engine all compile out; all machine code + data are unchanged and the
`kernel8-test` battery is 0 FAIL. As with PI-USB, the only possible delta from baseline is embedded
panic-`Location.line` u32s shifted by the gated insertion in `kernel_main` — a source-line number, never code
or behavior.

## §INSTALL-PI-2 — the Pi self-clone: the booted system reproduces its own boot media

**Landed 2026-07-20** (aarch64/Pi bare-metal; QEMU-live). Same `UNAOS_PIINSTALL_CONFIRM=1` gate. Glue:
`install/pi.rs` (the rewritten Gate-3 `install_flow`); engine: a new `install/clone.rs` (the buffered
self-clone primitive). Replaces INSTALL-PI's synthetic `UNAOS.IMG` marker with the **real thing** — the
Pi analogue of §INSTALL-2, cloning the running system's own boot media (`kernel8.img` + `config.txt` + the
GPU firmware files) onto a fresh GPT + FAT32 layout, every file sha-extent-verified.

**Same-device clone (the Pi's defining constraint).** §INSTALL-2's Orin clones a SEPARATE USB stick onto the
microSD, so it can stream read-source→write-target. The Pi has **one** block device: the seated `emmc2` card
is BOTH the source boot media AND the install target (Pi USB is a honesty stub — no second readable backend).
A streaming copy would read the source AFTER the GPT write had destroyed it. So the Pi clone is **two phases**,
the new engine seam in `install/clone.rs`:
1. **SNAPSHOT** — `clone::snapshot` reads the WHOLE source boot tree into the kernel heap through the in-tree
   FAT reader (`fs::fat::mount` on the seated card's own boot partition), BEFORE any destructive write.
   Bounded on every axis (per-file 32 MiB, total 40 MiB, depth 8); a short read is a malformed source, refused.
2. **WRITE** — after the GPT + zero-ESP + FAT32 pass, `clone::write_snapshot` mirrors the buffered tree onto
   the fresh ESP via the engine's `TreeWriter` (multi-cluster directories, multi-FAT-sector chains — the same
   writer §INSTALL-2/§ORIN-SDMMC-3 added), recording each file's extents.
Both phases are engine-level and target-agnostic (any `InstallTarget`, any `fs::fat` source) — the payload
seam, **not** a Pi-only fork; the Orin's streaming clone could adopt the buffered path later unchanged.

**Byte-clone vs fresh-format ruling (the partition question).** The boot FAT tree is cloned **BY CONTENT** —
a fresh FAT32 ESP, files re-written and per-file sha-verified — **never a raw byte-clone**. A source data /
`unafs` partition is **NOT** byte-copied: `write_gpt` lays a fresh, empty data partition. This follows §INSTALL-2
and the engine's verify discipline: every cloned byte must be file-sha-verifiable, which a raw partition image
is not, and the boot media the GPU ROM needs is the FAT tree, not the data volume. A fresh install's data
volume is empty by design.

**Verify (unchanged discipline, now over real files).** Every cloned file is re-read off the card and
SHA-checked through `verify_extents`; the flow prints a per-file `sha256=… VERIFIED` manifest, then
`:: INSTALL: pi emmc2 gpt+zero+fat32+clone(N files) verify => PASS ::`.

**QEMU witness.** `./arroyo kernel8-install [secs]` now stages the scratch card as a throwaway COPY of the
running system's own boot media (`scripts/make-pi-install-src.sh`: an MBR/FAT32 boot partition carrying the
freshly-built `KERNEL8.IMG` + `CONFIG.TXT` + `START4.ELF` + `FIXUP4.DAT` + an `OVERLAYS/` subdir, all 8.3-clean
so the clone round-trips exactly) — never the `kernel8-test` battery fixture. The installer mounts it,
snapshots, repartitions the same card, clones the tree back. **HOST-VERIFY is extended to the payload:** a
minimal FAT32 reader walks the resulting ESP root (and `OVERLAYS/`), reads each cloned file's cluster chain,
and byte-compares its SHA against the `KERNEL8_DIR` source — the clone proven from OUTSIDE the kernel, not just
the GPT/FAT32 structures. Both the in-kernel PASS line and host-side `HOST-VERIFY: PASS` are green.

**Metal-owed (flagged, not this arc's job).**
- **Cloned-card bootability** — whether a real Pi GPU ROM actually BOOTS from the cloned card. The QEMU witness
  boots via `-kernel`, so bootability of the written FAT tree is unverified here.
- **Long-name (LFN) preservation** — the QEMU source stages 8.3-clean names for an exact round-trip; a real Pi
  card carries long names (`bcm2711-rpi-4-b.dtb`, `overlays/miniuart-bt.dtbo`) the GPU ROM needs verbatim. The
  8.3 `TreeWriter` clones via mangled short names; LFN write-back is the follow-up that bootability depends on.
- **Dedicated target card** — on metal the seated card is the boot card; a real install leg wants a separate
  erasable target (the single-block-device constraint the same-device snapshot works around in QEMU).

## What later rungs still owe

- **Cross-platform:** the same `InstallTarget`/`TreeWriter`/`clone` seam generalizes to the x86 USB stick
  (installer_engine line seed rung 4); the Pi `emmc2` rung landed as §INSTALL-PI / §INSTALL-PI-2.
- **Throughput:** multi-block CMD25/CMD18 on the SD path (single-block is correct but slower on the zero pass
  and the per-cluster clone writes).
- **Metal SD throughput:** the multi-block CMD18/CMD25 path is compiled + verified off-metal (QEMU models no
  Tegra234 SDMMC); its first metal exercise is the attended Orin sitting.

## §INSTALL-SELF — the installer never offers, selects, or erases the device it booted from

**Observed at the bench** (rMBP 2012, 2026-07-29): the machine booted from an SD card in a USB
reader, and the graphical installer listed *that same card* as a target and offered to erase it.

§INSTALL-SEL had already made target selection real — the engine binds the disk the operator chose,
not "whatever disk is present" — so the offer was truthful about *which* disk it would destroy. It was
still an offer to destroy the running system. **Selection correctness and target eligibility are two
different properties**; this section supplies the second one.

### The identity, and why it is a FAT volume serial

Nothing in this tree carries a block-device identity across the boot handoff. The UEFI bootloader
knows a firmware handle; the kernel knows an xHCI slot; no mapping exists between them. (The builder's
own comment said as much: `BootInfo` carried no boot-device handle, "so the kernel cannot learn what
it booted from".)

What *does* cross the handoff is a byte written on the medium itself — the FAT `BS_VolID` the
formatter stamped into the boot sector:

| Stage | Where | What happens |
|-------|-------|--------------|
| Read | `crates/bootloader` → `read_boot_volume_serial` | Opens `LoadedImage` on the image handle, takes its **device** handle — the same one `get_image_file_system` resolves `kernel.elf` through — opens `BlockIO` on it **non-exclusively** (`GetProtocol`), reads LBA 0, and lifts `BS_VolID` out of the extended BPB. On a partition handle LBA 0 *is* the volume's boot sector. |
| Carry | `crates/boot-info` → `BootInfo::boot_volume_serial: u32` | New field. **0 is the absent sentinel.** aarch64's `build_boot_info` fills 0 (it does not boot through this bootloader). |
| Publish | `crates/kernel/src/main.rs` | One call to `install::selfguard::set_boot_volume_serial`, before `memory::init` consumes `boot_info`. Gated exactly like `crate::install`, so a build without an installer is byte-identical to baseline. |
| Match | `install::selfguard` + `fs::fat::volume_serials` | Reads every FAT volume serial off each candidate disk and compares. |

The `BlockIO` open is deliberately non-exclusive: the firmware's FAT driver holds `BlockIO` on that
handle `BY_DRIVER`, and an `Exclusive` open would call its `Stop` — tearing down the filesystem the
bootloader is about to read `kernel.elf` from. Every failure path returns 0. This code exists to stop
an *erase*; it must never be able to stop a *boot*.

It is a **volume** serial, not a device serial, and the difference is load-bearing in both directions:

- A **byte clone** of the boot media carries the same serial. Our own installer clones boot media
  (§INSTALL-2, §INSTALL-PI-2), so the collision is real, not theoretical.
- A **reformat** changes it, so a disk that once held our boot volume and has since been reformatted
  is a legitimate target again.

### The two layers

1. **UI — shown, marked, not selectable.** `video/instgui.rs` rows carry the verdict, resolved through
   the same `selfguard::classify` the engine consults (cached per block-registry signature, so the
   per-frame repaint costs nothing). A matching row is **kept on screen** and tagged ` BOOT` in dimmed
   text, with a legend (`BOOT = the disk this system booted from. Not installable.`). Excluding it
   from the list was the alternative; showing it was chosen because an installer that silently hides
   the operator's own disk sends them hunting for a disk that is not there. Selection *steps over*
   marked rows (`step_selectable`), the dialog opens on the first selectable row, the shrink-clamp in
   `service` lands on a selectable row, and `Enter` on a marked row refuses to advance with a witness.
   When every attached disk is marked, the `Continue` affordance is withdrawn and the screen says to
   attach another disk.
2. **Engine — the actual guard.** `install::run_engine` calls `selfguard::refuses` on the target it
   **bound**, before its first write and *before* the blank-check: "this is the disk you are running
   from" outranks "this disk is not blank" as a reason to stop, because a blank boot device is still a
   boot device. Refusal is `InstallError::BootDevice`; nothing is written. The UI filter is not the
   guard — this is. The unattended witness path has no UI at all and is covered by the same check.

### Matching reads ALL volumes, not the first

`fs::fat::mount_source` is first-match-wins: it returns the first volume that parses and stops. Right
rule for "mount the boot media", wrong rule for "is this the disk we booted from" — a device whose
*second* partition is the ESP we booted would go unrecognized and be offered as a target. So
`fs::fat::volume_serials` enumerates all of them, in the same superfloppy → GPT → MBR order, through
the same `parse_bpb` gates. The partition-table walks were extracted into `gpt_volume_starts` /
`mbr_volume_starts` and are shared with the mount path, so the guard can never see a partition the
mount would not, or miss one it would. The direction of the difference is the safe one: a superset of
serials can only cause *more* candidates to be refused, never fewer.

### Edge cases, all handled

| Case | Behavior |
|------|----------|
| Serial absent / 0 (pre-guard bootloader, non-FAT boot path, aarch64) | Guard **DISARMS** with `:: install: boot volume serial ABSENT (0) — INSTALL-SELF boot-device guard DISARMED ::`. Every candidate stays eligible. Bricking the installer is not a safe failure mode; announcing that the guard protects nothing is. |
| Two attached volumes with the SAME serial (clones) | **Both refused**, plus a distinct witness naming the collision. The rule is per-candidate, so refusing both is the *absence* of a special case. If two disks both claim to be the volume we are running from, we cannot tell which we would erase, and the safe direction is to erase neither. |
| Candidate carries no FAT at all | Cannot match; stays a valid target (witnessed as such). |
| Candidate's `BS_VolID` is 0 | Never excluded. 0 is the absent sentinel on both sides, so an *unstamped* volume is not evidence of anything. |
| Candidate not in the live registry | Not judged here — the engine's own `bind_id` already refuses it as `TargetGone`. |

### Witness formats

At disk-list build (one line per candidate, emitted once per block-registry signature):

```
:: install: boot volume serial=0xfabe1afd — INSTALL-SELF boot-device guard ARMED ::
:: install: boot device global/slot1 (262144 sectors) serial=0xfabe1afd EXCLUDED ::
:: install: candidate global/slot1 (262144 sectors) 1 FAT volume(s), first serial=0x554e4153 != boot 0xfabe1afd, ELIGIBLE ::
:: install: candidate global/slot1 (262144 sectors) — no FAT volume, cannot be the boot device, ELIGIBLE ::
:: install: boot serial=0x… matches N attached volumes (CLONES) — ALL EXCLUDED, refusing to guess which one we booted ::
```

Engine refusal (defense in depth; should be unreachable through the UI):

```
:: INSTALL: refusal — target '<vendor>' '<product>' slot=N carries the BOOT volume serial 0x…; refusing to erase the device we booted from => guard OK ::
```

The bootloader also states its own result, including *why* a disarmed guard is disarmed, so that is a
diagnosable fact at the bench rather than a mystery:

```
[ INFO]: boot volume FAT serial 0xfabe1afd (extended BPB BS_VolID)
[ INFO]: boot volume FAT serial unavailable: <reason> — installer boot-device guard will disarm
```

### QEMU coverage, and what it cannot cover

`UNAOS_INSTALLDEMO=1 ./arroyo test` runs `selfguard::selftest` before the engine and
`selfguard::live_media_leg` after it.

- **Decision table (synthetic, substantive).** The rule is pinned over synthetic serial sets: disarmed,
  match, multi-volume match, no match, no FAT, 0-sentinel, clone collision. Verdict:
  `:: INSTALL-SELF: guard decision table (…) => PASS ::`
- **Live media.** The harness boots from an `ide-hd` ESP the kernel has no driver for, and the only
  disk it enumerates is the installer's own **blank** scratch — so at list-build time there is no FAT
  volume to match and the live leg **SKIPs** with that reason recorded. *After* the engine formats the
  scratch, that disk carries a real FAT32 volume, and `live_media_leg` reads its serial off the wire
  (`0x554e4153`, the formatter's `VOL_ID`) and confirms the guard's answer against the real boot
  serial (`0xfabe1afd`, QEMU's vvfat constant).
- **Exclusion on live media, role-swapped.** The harness *cannot attach* a disk whose serial matches
  the boot volume, so the exclusion path has no live fixture. The leg therefore asks the real
  comparison with the roles swapped: take a serial actually read off live media and ask whether a boot
  volume carrying *that* serial excludes this disk. Real bytes, real comparison, and the answer must be
  `BootDevice` — if it is ever anything else, the guard cannot exclude a boot device no matter what the
  bootloader reports. Verdict:
  `:: INSTALL-SELF: live-media exclusion (role-swapped: boot serial := 0x… read off …) => EXCLUDED, PASS ::`

### Known limitations

- **The FAT32 formatter stamps a constant.** `install::fat32::VOL_ID` is `0x554E_4153` ("UNAS") for
  every volume it writes. So a machine booted from UnaOS-installed media reports that serial, and every
  *other* UnaOS-installed disk attached to it is excluded as a clone. That is the safe direction and
  the documented clone behavior, but it is over-broad; giving the formatter a per-install serial (and a
  matching write-back to the media it clones) is the follow-up.
- **`parse_bpb` does not gate on `BS_BootSig`.** The kernel's BPB parser reads `BS_VolID` at
  0x27/0x43 unconditionally, so an unstamped volume yields whatever bytes live there. The bootloader
  side *does* gate on `BS_BootSig == 0x29` (the byte immediately before `BS_VolID`, at 0x26/0x42). The
  asymmetry only ever over-matches on the kernel side, and over-matching in a guard costs an excluded
  target, not an erased one.
- **Metal is the only place the exclusion path runs end-to-end.** Everything above is QEMU- and
  synthetic-verified; the bench case that motivated the arc (boot from a USB SD reader, confirm the
  card is listed, marked, unselectable, and refused by the engine) is an arc-boundary hardware check.
