# SDMMC-T1 — pre-flight adjudication of PANEL-REVIEW finding T-1

Executor SDMMCT1, seat orin 16, track `hw-jetson`. Read-only investigation; no repo edit, no
privileged command, no card touched.

**VERDICT: SAFE-TO-FLY.** The staged probe image
`~/unaos-bench/flash/orin/sdmmcwrite-20260906T0137Z-a05c2c8/` may be booted on the bench Orin's
UNAOS-ORIN card. On that card the probe's target sector is provably outside every partition AND
provably empty of any embedded boot image, so nothing the card carries is clobbered. Two of T-1's
premises are factually wrong (see §4).

---

## 1. What the code actually does

File: `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs` (worktree
`/home/pmes/src/github.com/pmes/UnaOS-orin`, branch `hw-jetson` @ `37c78ad7`).

Gating (verified structural, matches the panel):

- `sdmmc_tegra.rs:125` — `#[cfg(all(feature = "tegra", feature = "sdmmcwrite"))] pub use metal::sdmmc_write_probe;`
- `main.rs:2371` — the single call site, `#[cfg(all(feature = "sdmmcwrite", feature = "tegra"))]`,
  appended to the `sdmmc_census` line, i.e. it runs immediately after the census.
- Section `sdmmc_tegra.rs:2874-3260`, all `#[cfg(feature = "sdmmcwrite")]` inside `#[cfg(feature = "tegra")] mod metal`.

Scratch selection — `fn choose_scratch` (`sdmmc_tegra.rs:2963-3010`), classic-MBR branch:

```
sdmmc_tegra.rs:2993-2997
            return match first {
                Some(s) if s >= 2 => Scratch::Lba { lba: s - 1, reason: "mbr-gap-top", first_part: Some(s) },
                Some(_) => Scratch::Refused("mbr-first-partition-at-lba-1-no-gap"),
                None => last_lba_if_free(base, blk, num_blocks, "mbr-empty-table-last-lba"),
            };
```

`first` is the **minimum** start LBA over all four MBR entries with `type != 0 && sectors != 0`
(`:2981-2992`). So the target is `min_partition_start − 1`:

| card layout | target | note |
|---|---|---|
| classic MBR, first partition at 2048 (modern alignment) | **LBA 2047** | the documented expectation for the bench card |
| classic MBR, first partition at 63 (legacy alignment) | LBA 62 | the case T-1 names |
| classic MBR, first partition at LBA 1 | — | **refused** `mbr-first-partition-at-lba-1-no-gap` |
| 0xEE in any of the 4 entries | GPT branch (`:3025-3095`), target = `first − 1` with floor `max(array_end, FirstUsableLBA)` | last LBA never used on GPT |
| FAT at LBA 0, no table | last LBA, only if BPB total ends below it and it is not `EFI PART` | |
| anything else | — | **refused** `unknown-sector0-layout` |

Reads before the write: sector 0 (`:3192`), then the target sector itself (`:3217`), both through the
unarmed `read_block_ro` (CMD17). On GPT it also reads LBA 1 and the entry array.

**What refuses the write**: only the layout refusals in the table above (plus
`no-published-card`, `sector0-unreadable`, `cmd17-prior` read failure). There is **no** content
gate — T-1 is correct here:

```
sdmmc_tegra.rs:3221-3241
        if &prior[..WITNESS_MAGIC.len()] == WITNESS_MAGIC { … }        // print only
        else if prior.iter().all(|b| *b == 0) { … }                    // print only
        else { … "other(first8=…)" … }                                 // print only
        // The write.
        let tick = crate::arch::timer::cntpct();
        …
        let ticks = match write_block_probe(base, blk, lba, &pattern) { … };
```

`prior` is printed and then the write proceeds unconditionally; there is no stash/restore (deliberate
— the un-restored witness is the next boot's persistence proof, `:2900-2903`,
`docs/dev/evidence/orin14/SDMMCWRITE.md` §"The question, exactly"). Ledger row: `docs/dev/OS/orin-ledger.md`
A23 (`fixed-unflown`, sdmmcwrite `33dc7811`).

## 2. The bench card's actual layout

**It is a classic MBR card, metal-confirmed, and it is NOT GPT.**

`~/unaos-bench/capture/orin2-boot5c-gui.log` (2026-08-22, bench Orin, `UNAOS_SDMMC=1`):

```
:: SDMMC:   M2: CID manufacturer(MID)=0x03 OEM(OID)='SD' product(PNM)='SS32G' rev=0x80 serial(PSN)=0x19f1bca4 date=12/2014 ::
:: SDMMC:   M2: capacity 62333952 blocks (30436 MiB, CSD v2), addressing block (SDHC/SDXC), v2 (CMD8 ok) ::
:: SDMMC:   M3: sector 0 first 16 bytes = 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 ::
:: SDMMC:   M3: sector-0 signature = MBR (0x55AA boot signature; classic partition table) ::
```

Card identity matches the staged MANIFEST exactly ("32 GB SS32G, sector 0 = classic MBR,
62,333,952 blocks"). `classify_sector0` (`sdmmc_tegra.rs:2612…`) returns "MBR (0x55AA …)" only when
byte 450 is **not** 0xEE, so a GPT-protective/hybrid MBR is excluded on that boot, and
`choose_scratch` additionally scans all four entries for 0xEE (`:2971`).

**Two decisive consequences of `sector 0 first 16 bytes = 00 ×16`:**

1. There is **no MBR boot code** on this card. Any x86 boot block (GRUB `boot.img`, syslinux, a DOS
   MBR) begins with executable code at byte 0 (`0xEB…`, `0xFA…`); sixteen zero bytes is what
   `fdisk`/`parted` leave when no bootloader was ever installed. `grub-install` has never run on this
   card, so **no `core.img` is embedded in the post-MBR gap** — the exact hazard T-1 raises.
2. The card is a UEFI boot volume (`EFI/BOOT/BOOTAA64.EFI` in the FAT partition, per every staged
   MANIFEST and `load-card.sh`'s sha match), and the Orin's own boot chain lives in QSPI flash
   (`capture/orin2-boot5c-gui.log`: "Found 60 partitions in QSPI_FLASH"). Nothing in the boot path
   needs a raw sector in the gap.

**The card is never repartitioned by the bench tooling.** `~/unaos-bench/scratch/orin11/load-card.sh`
finds the partition by LABEL `UNAOS-ORIN`, `udisksctl mount`s it, copies the staged directory over
the mounted filesystem, `sync`s, and sha-verifies each file against the MANIFEST. No `mkfs`, no `dd`,
no `sfdisk`/`sgdisk`/`wipefs` anywhere in it or in `scratch/orin14/stage-render*.sh` /
`scratch/orin15/stage-render7.sh` (those only assemble the staged directory under
`~/unaos-bench/flash/orin/`). Confirmed by grep across `scratch/orin1[0-6]/*.sh`: zero hits. So the
partition table and the gap are as originally created and unchanged since the census above.

**Beware a look-alike in the record**: `capture/orin-r23s1/cu.usbmodem143402.log` shows
`M3: sector-0 signature = GPT-protective MBR (0xEE partition; GPT header at LBA 1)` — that is a
**different card** (SS16G, 31,116,288 blocks, serial 0x2ae1301f, 12/2017) that the old `sdmmc_arm`
ladder deliberately repartitioned in that session ("ABOUT TO DESTROY … the entire card is about to be
repartitioned"). It is the scratch card, not the boot card. Also,
`docs/dev/OS/01_BOOT_HAL/arch_arm64.md:6505-6506` shows `GPT-protective MBR (…)` inside an
**illustrative** expected-wire block with `xx xx …` placeholders — not a card fact. Either is a
plausible source of T-1's "the bench card is GPT".

**What is still unproven**: the first partition's exact start LBA (hence whether the target is 2047 or
62 or another value). No `sfdisk -d` / `fdisk -l` / `lsblk START` capture of this card exists anywhere
under `~/unaos-bench/`; the harvest directories (`scratch/orin11/harvest-*`) are file-level copies of
the mounted FAT volume and contain no partition table. The card is not in this host's reader right now
(`flatpak-spawn --host lsblk -o NAME,LABEL,SIZE,TYPE,START` lists only `nvme0n1`/`zram0`).

**This does not change the verdict**, because the safety argument holds for *every* value the MBR
branch can accept: the target is always `min_start − 1 ∈ [1, min_start)`, i.e. always inside the
pre-partition gap, never sector 0 (refused at `min_start == 1`) and never inside any partition — and
the gap on this card is provably free of any embedded image by (1) above.

**Command that would pin the exact LBA** (optional, non-destructive, only when the card is in the
host's reader — the label is the router, `sdX`/`mmcblkN` varies):

```
flatpak-spawn --host lsblk -o NAME,LABEL,SIZE,TYPE,START            # unprivileged, gives the start LBA
flatpak-spawn --host sudo sfdisk -d /dev/mmcblk0                    # authoritative table dump
flatpak-spawn --host sudo dd if=/dev/mmcblk0 bs=512 count=1 status=none | xxd | head -32   # boot-code bytes 0..439
```

Expected: `label: dos`, one entry `start=2048`, and an all-zero first 440 bytes.

## 3. Blast radius of the write, if it happens

One 512-byte block at `min_start − 1`, written once per boot, not restored. It cannot reach:

- **sector 0 / the MBR table** — `min_start == 1` is a refusal; `min_start >= 2` ⇒ `lba >= 1`.
- **any partition** — `lba < min_start <= every partition start`.
- **the FAT volume and every staged file** — inside partition 1, hence above `min_start`.
- **both GPT headers / the GPT entry array** — the card is not GPT; and on a GPT card the branch
  floors at `max(array_end, FirstUsableLBA)` and never uses the last LBA.

Residual, on this card: 512 bytes of unallocated gap are overwritten permanently. That is the design
intent (the persistence proof), and it destroys nothing the card carries.

## 4. Where T-1 is wrong, and where it is right

| T-1 claim | adjudication |
|---|---|
| "picks **LBA 62** on legacy-MBR cards, which is where GRUB's core.img lives" | **Conditional and not this card.** The code picks `min_start − 1`; LBA 62 requires a first partition at 63. The documented expectation here is LBA 2047, and no bootloader exists on the card in any case (sector-0 bytes 0..15 are zero). |
| "*Why it does not block the merge*: **the bench card is GPT** (the GPT branch is sound)" | **False.** Metal census says classic MBR. The premise is inverted: the MBR branch T-1 criticises is exactly the branch that will run. (The merge is still unaffected — the knob is default-off and the call site is double-gated.) |
| "`s == 2` yields `lba = 1`, the first sector of any embedded image, which the header claims is refused" | **Misreading.** The header refuses a *first partition at LBA 1* (`mbr-first-partition-at-lba-1-no-gap`), which the code does at `:2995`. `s == 2` gives the single available gap sector; on a card with no embedded image that is correct, and it is not this card anyway. |
| "the rationale 'the farthest point from sector 1 where an embedded boot image would begin' has the direction backwards" | **Wrong-way-round reading of the rationale.** Embedded images grow *up* from sector 1, so the top of the gap is the *last* place they reach — picking the top maximises distance from the image, which is what the header says. It is nonetheless true that a maximally large `core.img` can fill the whole gap, which is why (1) above (no boot code at all) is the load-bearing evidence, not the top-of-gap heuristic. |
| "`prior` is printed and then `write_block_probe` is called **unconditionally**" | **Correct**, verified at `:3221-3241`. Deliberate, documented at `:2900-2903`. |
| "does not stash and restore, so the damage is unrecoverable" | **Correct** and deliberate (`SDMMCWRITE.md`: "never restores it … the next boot's `prior … = witness(tick=…)` line is the persistence proof"). |

## 5. Optional hardening (not required for this flight)

T-1's one-line suggestion — refuse unless `prior` is all-zero or carries `WITNESS_MAGIC` — is cheap,
does not break the persistence design (the witness case stays allowed), and would make the probe safe
on an *unknown* card as well as this one. It is worth taking as a follow-up; it is **not** a
precondition for the scheduled boot, because on this card the target sector is proven free by evidence
independent of its content.

---

*Sources:* `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs:125, 2874-3260` (esp. `2963-3010`,
`2993-2997`, `3179-3260`), `unaos/crates/kernel/src/main.rs:2371`,
`docs/dev/evidence/orin14/SDMMCWRITE.md`, `docs/dev/OS/orin-ledger.md` (A23, §F SD card),
`docs/dev/OS/01_BOOT_HAL/arch_arm64.md:6480-6530`, `~/unaos-bench/capture/orin2-boot5c-gui.log`,
`~/unaos-bench/capture/orin-r23s1/cu.usbmodem143402.log`, `~/unaos-bench/scratch/orin11/load-card.sh`,
`~/unaos-bench/scratch/orin14/stage-render4.sh`, `~/unaos-bench/scratch/orin15/stage-render7.sh`,
`~/unaos-bench/flash/orin/sdmmcwrite-20260906T0137Z-a05c2c8/MANIFEST`.
