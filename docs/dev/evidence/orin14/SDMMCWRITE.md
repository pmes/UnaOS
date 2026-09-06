# SDMMCWRITE — the on-die SDMMC write probe (gap #3's first metal question)

Executor SDMMCWRITE, seat orin 14, track `hw-jetson`, base `2a04fb4a`. Ledger: `docs/dev/OS/orin-ledger.md`
A23 and §F "SD card — on-die SDMMC"; the question is §"The five gaps" #3.

Files touched: `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs` (a `sdmmcwrite`-gated tail section
inside `mod metal` + one line-neutral `pub use` append + one in-place cfg widening on the `INT_WRITE_RDY`
constant + an in-place header-comment correction), `unaos/crates/kernel/Cargo.toml` (`sdmmcwrite = ["sdmmc"]`),
`unaos/arroyo` (`UNAOS_SDMMCWRITE=1` mapping + the `arm-tegra-sdmmcwrite` leg), `unaos/crates/kernel/src/main.rs`
(one line-neutral append on the `sdmmc_census` call line), the ledger, this file. `drivers/block.rs` and
`fs/fat.rs` are untouched (rmbp's): `program_source` still refuses `TegraSd`, and the block layer's
`write_block_tegra_sd` still refuses unconditionally — this probe is the ONLY writer and it is knob-gated.

## The question, exactly

> On `mmc@3400000`, with the vendor pad block left DISABLED (the SError conviction), does a CMD24
> single-block write to a scratch sector followed by CMD17 read-back return the written bytes, at
> 1-bit default speed?

Why the existing `sdmmc_arm` ladder does not answer it: its scratch region is the card's LAST LBA and it
refuses outright on a GPT card; it also stashes and restores, so the sector holds nothing across boots.
This probe writes ONE witness block to ONE sector proven outside every partition and outside the FAT
volume, never restores it (the next boot's `prior … = witness(tick=…)` line is the persistence proof),
and prints one verdict line. Nothing in it touches `base+0x100` or beyond, changes the bus width, the clock,
or uses DMA.

## Build

    UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 \
    UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_SDMMC=1 UNAOS_SDMMCWRITE=1 ./arroyo esp-jetson

The banner must show `sdmmc` and `sdmmcwrite`. Reachability, not just compilation:

    grep -a -c '\[sdmmc\] write' target/aarch64_esp/kernel.elf      # >= 1

`UNAOS_SDMMCWRITE=1` alone pulls `sdmmc` (Cargo: `sdmmcwrite = ["sdmmc"]`; arroyo appends both). It does
NOT pull `sdmmc_arm` — see "Why not `sdmmc_arm`" in the section header of `sdmmc_tegra.rs`.

## Where it runs

`main.rs`, tegra region, the `sdmmc_census` call line (~:2371): `sdmmc_write_probe()` is appended to that
line under `#[cfg(all(feature = "sdmmcwrite", feature = "tegra"))]`, so it runs immediately after the census
and before the JB2b USB pump — at EL2, on the boot core, with the JM4 timer live (the same conditions the
census's own CMD17 reads ran under). It consumes the census's published `SdBlk` (`SD_BLK`), which exists
only if M1 (live SDHCI), M2 (card identified) and M3 (sector 0 read) all passed; otherwise it refuses.

## The scratch-sector rule (binding)

Printed before any write; refused (named) when no sector can be PROVEN free.

| sector 0 says | scratch sector | proof | refusals |
|---|---|---|---|
| classic MBR (0x55AA, no 0xEE) | `first_partition_start − 1` (the TOP of the post-MBR gap) | below every partition's start; farthest from sector 1 where an embedded boot image would begin | `mbr-first-partition-at-lba-1-no-gap`, `mbr-partition-start-beyond-card` |
| classic MBR, empty table | the card's last LBA | last LBA read first; must not carry `EFI PART` | `orphan-gpt-backup-header-at-last-lba`, `last-lba-unreadable` |
| 0xEE in any entry (GPT) | `first_partition_start − 1`, and it must be ≥ max(entry-array end, FirstUsableLBA) | header at LBA 1 signature-checked; entry array read (entry size 128..512 dividing 512, ≤ 64 sectors); every used entry's start considered | `gpt-header-unreadable`, `gpt-header-signature-missing`, `gpt-entry-size-unsupported`, `gpt-entry-array-geometry-unsupported`, `gpt-entry-array-unreadable`, `gpt-partition-start-beyond-card`, `gpt-no-gap-before-first-partition` |
| FAT boot sector at LBA 0 (no table) | the card's last LBA | BPB total sectors must end below it; last LBA not `EFI PART` | `fat-volume-spans-card`, (+ the last-LBA refusals) |
| anything else | — | — | `unknown-sector0-layout` |

The last LBA is NEVER used on a GPT card (it is the backup header). On the bench card (render2/3 census:
32 GB SS32G, sector 0 = classic MBR, 62,333,952 blocks) the expected branch is `mbr-gap-top`; with a
first partition at the usual LBA 2048 the target is LBA 2047.

## The written block

512 bytes: `UNAOS-SDMMC-W1` at [0..14], version byte 1 at [14], 0 at [15], the CNTPCT tick at write time
at [16..24] LE, the LBA at [24..32] LE, then `(i*7) ^ 0xa5 ^ (lba as u8)` over the rest. The read-back
compares all 512 bytes; a mismatch prints the count and the first differing offset with both bytes.

## SDHCI register facts relied on

Offsets are the standard SD Host Controller block as the Pi `drivers/emmc2.rs` model and this file's
constants already use them (the BCM2711 "32-bit view" names ARE the SDHCI layout). Section numbers are
those of the SD Host Controller Simplified Specification, Version 3.00 (the layout is unchanged in 4.x):

| register | offset | spec | use in the probe |
|---|---|---|---|
| Block Size / Block Count | 0x004 / 0x006 (`BLKSIZECNT`) | §2.2.2, §2.2.3 | `(1 << 16) \| 512`: one 512-byte block |
| Argument 1 | 0x008 | §2.2.4 | the LBA (block addressing, CCS=1) or byte offset (SDSC) |
| Transfer Mode | 0x00C low half (`CMDTM`) | §2.2.5 | bit 4 Data Transfer Direction Select = **0 = write (host→card)** — the ONE bit that differs from the CMD17 word; Block Count Enable / Multi Block = 0 (single block, as the reads) |
| Command | 0x00E high half (`CMDTM`) | §2.2.6 | index 24, Response Type Select 10b (48-bit, R1), CRC check + index check enable, Data Present Select = 1 |
| Response | 0x010 (`RESP0`) | §2.2.7 | R1 card status, masked with `R1_ERROR_MASK` (SD Physical Layer §4.10.1) |
| Buffer Data Port | 0x020 (`DATA`) | §2.2.8 | 128 × 32-bit LE writes after Buffer Write Ready |
| Present State | 0x024 (`STATUS`) | §2.2.9 | bit 1 Command Inhibit (DAT): held while the card signals busy on DAT0 after the block — cleared = programming complete |
| Normal Interrupt Status | 0x030 (`INTERRUPT`, W1C) | §2.2.17 | bit 0 Command Complete (inside `send_command`), bit 4 **Buffer Write Ready**, bit 1 Transfer Complete, bit 15 Error summary; bits [31:16] are the Error Interrupt Status register (§2.2.18) |
| Normal/Error Interrupt Status Enable | 0x034 (`IRPT_MASK`) | §2.2.19, §2.2.20 | already all-ones from the census's M2 step 2 (status latches only if enabled) |

The sequence is the spec's non-DMA write transaction (§3.7.2, "Not Using DMA"): set block size/count,
argument, transfer mode + command → wait Command Complete → wait Buffer Write Ready → write one block
through the Buffer Data Port → wait Transfer Complete → then, because the card programs the flash while
holding DAT0 low (SD Physical Layer §4.3.4, "Data Write"), wait Command Inhibit (DAT) clear before the
read-back. Timeouts: `CMD_TIMEOUT_MS` = 100 (command), `DATA_TIMEOUT_MS` = 200 (Buffer Write Ready and
Transfer Complete — the same budgets the reads use), and `PROG_TIMEOUT_MS` = 500 for the busy release —
longer on purpose: the SD Physical Layer's write-timeout ceiling for an SDHC/SDXC card is 250 ms, so the
read budget could report a healthy slow card as `FAIL at dat0-busy`. On any failure after the command
was issued the CMD/DAT lines are reset (`reset_cmd_dat`, §3.10 error interrupt recovery: Software Reset For
CMD/DAT) so the controller is not left inhibited for whatever runs next.

Card commands used, and nothing else: CMD17 (READ_SINGLE_BLOCK, through the unarmed `read_block_ro`) and
CMD24 (WRITE_BLOCK; SD Physical Layer §4.7.4, adtc, R1). No CMD25, no ACMD6, no CMD6, no erase.

## Expected wire (armed, bench card)

    [sdmmc] write probe armed (UNAOS_SDMMCWRITE=1): CMD24 then CMD17 read-back, 1-bit default speed, PIO, vendor block untouched, card_blocks=62333952
    [sdmmc] write target=lba 2047 reason=mbr-gap-top layout=MBR (0x55AA boot signature; classic partition table) first_part_lba=2048 card_blocks=62333952
    [sdmmc] write prior lba=2047 = zero
    [sdmmc] write lba=2047 -> OK (512/512 match, t=<ticks> ticks = <us> us @ 31250000 Hz)

Second and later boots: `prior lba=2047 = witness(tick=<previous boot's tick> lba=2047)` — the persistence
proof. Negative shapes, each one line:

    [sdmmc] write lba=<n> -> FAIL status=0x<INTERRUPT or STATUS or R1> at <cmd24-issue|cmd24-r1|buffer-write-ready|buffer-write-ready-error|transfer-complete|data-error|dat0-busy|cmd17-prior|cmd17-readback>
    [sdmmc] write lba=<n> -> FAIL mismatch=<n>/512 first_diff=<off> want=0x.. got=0x..
    [sdmmc] write -> REFUSED reason=<name> layout=<class> card_blocks=<n>
    [sdmmc] write -> REFUSED reason=no-published-card (census did not reach M3 this boot)

A `-> FAIL status=0x… at cmd24-issue` with an EL3 SError instead of the line is the FWALL shape and would
mean the standard window is ALSO firewalled for writes — read the `esr_el3` and stop; do not touch the
vendor block to "fix" it.

## Scorer

    awk '/\[sdmmc\] write/' <boot log>
    # PASS predicate (one line, exit 0 = pass):
    awk '/\[sdmmc\] write lba=[0-9]+ -> OK \(512\/512 match/{ok=1} END{exit !ok}' <boot log>
    # persistence (second boot):
    awk '/\[sdmmc\] write prior lba=[0-9]+ = witness\(tick=/{p=1} END{exit !p}' <boot log>

Ticks: `[sdmmc] write lba=… -> OK` ticks A23 `flown`; a `REFUSED` line is a scorer result too (the card's
layout on that day), recorded verbatim, and A23 stays `fixed-unflown` with the reason.

## Gate results (this executor's tree, base 2a04fb4a, 2026-09-05)

| gate | command | result |
|---|---|---|
| type-check both arches | `cd unaos && ./arroyo check` | exit 0 — `✅ x86_64 OK`, `✅ aarch64 OK`, `✅ kernel cfg coverage OK (46 legs)` incl. `✅ arm-tegra-sdmmcwrite` (new leg, 45 → 46); `GATE-KNOB: OK — 155 features declared, 154 named by a cfg, 0 phantom, 0 dead, 0 trailing-comment cfg`; `GATE-LEDGER: OK — 73 rows in 2 ledger file(s)` |
| armed leg alone | `cargo +nightly check --release --target ../../aarch64-unaos.json … --features <arm-tegra list>,sdmmcwrite` | exit 0, no warning in `sdmmc_tegra.rs` |
| aarch64 QEMU regression | `./arroyo test-arm 60` | exit 0 — `✅ aarch64 test complete`, `SERWIT-2 … -> PASS`; default features (no sdmmc knob), 0 `SDMMC` lines as expected |
| armed jetson image | the Build line above | exit 0; banner `⚡ kernel features (jetson): witness,ehcihid,kbdwit,sdhcblk,holocron,smolnet,tegra,orinclick,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,deskcascade,sdmmc,sdmmcwrite` |
| reachability | `grep -a -c '\[sdmmc\] write' target/aarch64_esp/kernel.elf` | 1 (the `WPS` literal); `grep -a -c 'probe armed (UNAOS_SDMMCWRITE=1)'` 1, `'target=lba'` 2, `'UNAOS-SDMMC-W1'` 1 |
| P7 position (main.rs append) | `sed -n 2371p main.rs` → index of `#[cfg(all(feature = "sdmmcwrite"` (89) < index of first `//` (198) | CODE, not prose; `knob-hygiene.sh` trailing-comment probe 0 |
| knob-off identity, Pi | `./arroyo kernel8` at 2a04fb4a vs this tree, `sha256sum target/pi_baremetal/kernel8.img` | `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` both |
| knob-off identity, Jetson | `./arroyo esp-jetson` (no knobs; banner `ehcihid,kbdwit,sdhcblk,smolnet,tegra,tegrasmp`) in a throwaway worktree at 2a04fb4a vs this tree | `kernel.elf` whole-file `9dce2347a346cfb2…` both; `llvm-objcopy -O binary` `a052a27502b2fefe…` both (1,559,640 B); `--strip-all` `68fde8f564ca573d…` both |

The whole-file ELF is identical (not merely the stripped image) because every new byte in `sdmmc_tegra.rs`
sits under the file-level `#![cfg(feature = "sdmmc")]` — knob-off, the module does not enter the build at
all — and the `main.rs`/`Cargo.toml`/`arroyo` edits are a same-line append, a new `[features]` entry and a
new mapping/leg, none of which reach the compiled feature set.
