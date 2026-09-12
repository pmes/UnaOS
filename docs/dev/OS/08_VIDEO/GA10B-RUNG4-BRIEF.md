# GA10B-RUNG4-BRIEF — ignite the GSP boot ROM with no vendor blob, and read its verdict

**Status: FROZEN DESIGN. No code in this arc.** Design only, per LAWS §Verification and the
"the design closes first" rule (orin 22 BULLETIN §19 — `docs/dev/evidence/orin22/BULLETIN.md`, which lands on trunk at `815086e4`, **after** this arc's base `600887c2`, so it is cited by path rather than linked) — a battery is not spent on a design that
has not closed. This document is the closed design; a Group B seat implements it from here.

**Headline.** Rung 4 does **not** need Peter's blob ruling and does **not** need a new Group A
extraction pass. Every register, bit, constant and ordering it uses is already in the tree's ACKED
facts file. The rung is redesigned around one measured fact from render8/render11: **`br_retcode`
has never left `0x0` on this die.** Moving it off `0x0` — to `0x2` (FAIL) — is the first
observable execution of GPU silicon under UnaOS's direction, and it requires **no correct image**,
because the rung's PASS predicate is *"the boot ROM reached a verdict"*, not *"the boot ROM
accepted our payload"*. A FAIL verdict is this rung's success.

If the ROM cannot be made to reach a verdict, the rung names which specific gate stopped it —
BCR writes refused under priv-lockdown, the `bcr_ctrl` trigger inference wrong, the lock latching
early, or the fabric refusing the ignition write — each with its own wire arm.

---

## 0. Provenance and clean-room posture (read first)

**This brief performs NO fact extraction and imports NO new fact.** Everything below is drawn from,
in order of authority:

1. **The ACKED facts file** —
   [`ga10b-probe-rung1.facts.md`](../../../../unaos/docs/dev/OS/09_PLATFORM/ga10b-facts/ga10b-probe-rung1.facts.md),
   ACKED under [`CLEAN_ROOM_POLICY.md`](../../../MANIFESTO/CLEAN_ROOM_POLICY.md) §6 on 2026-08-25,
   independently COI-reviewed (26 entries, ACK-WITH-EDITS applied). Its §(b) "RISC-V boot-ROM
   interface" and "Boot-ROM handshake ordering (SEQ)" carry **every** register, bit, constant and
   ordering step rung 4 uses. Nothing new is needed from `nvgpu`.
2. **Metal measurement** — the render8 and render11 flights of `ga10bprobe3` / `ga10bprobe3b` ([`ga10b_probe.rs`](../../../../unaos/crates/kernel/src/arch/aarch64/ga10b_probe.rs)),
   tonight's wire `~/unaos-bench/scratch/orin23/boot-render11-A1.log`, read with
   `LC_ALL=C awk 'index($0,"[ga10bprobe")'`.
3. **The live DTB (EXT)** and this tree's own carveout/MMU witnesses.

**No NVIDIA driver text — source, comment prose, macro, struct or transliteration — appears in this
document.** Register offsets and the ordering are hardware facts already pointer-backed in the ACKED
file; this brief cites the ACKED file, never the quarantine. **No blob is named as a file, fetched,
staged or embedded**, and the L4T firmware set is deliberately absent from this design — see §5.

**Group boundary.** The author of this brief performed no extraction and read no `nvgpu`; this is a
Group B design document written from an already-reviewed Group A handoff. It therefore imposes no
new group constraint on the implementing seat beyond the one already standing.

---

## 1. What the hardware said, and the one fact everybody missed

Three flights, identical readings (render8 2026-09-06; render11 tonight):

```
[ga10bprobe3] pg=0x1 clk=2/3 regs=16 of 25 readable, 9 UNREADABLE -> COMPLETE
[ga10bprobe3] opt_wpr_enabled=0x00000001
[ga10bprobe3] bcr_dmacfg lock_locked=0 (raw=0x00000000)
[ga10bprobe3b] mailbox0 wrote=0x5a5aa5a5 read=0x5a5aa5a5 -> MAILBOX-HELD
```

### 1.1 The readability map is per-register, not per-block

"Priv lockdown over the GSP block" is the right summary and the wrong resolution. Sorted from the
render11 wire, the **9 UNREADABLE** (`pri-error 0xbadf5620`) are:

| unreadable | class | what it is |
|---|---|---|
| `gsp_falcon_dmactl` `0x11010c` | gsp-falcon-v1 | legacy falcon DMA control |
| `gsp_falcon_idlestate` `0x11004c` | gsp-falcon-v1 | legacy falcon idle |
| `gsp_falcon_irqmask` `0x110018` | gsp-falcon-v1 | legacy falcon IRQ |
| `gsp_falcon_irqdest` `0x11001c` | gsp-falcon-v1 | legacy falcon IRQ |
| `gsp_falcon_cpuctl_v1` `0x110100` | gsp-falcon-v1 | the **v1** view of halted |
| `priscv_boot_vector_lo` `0x111380` | gsp-priscv-bcr | optional boot vector |
| `priscv_boot_vector_hi` `0x111384` | gsp-priscv-bcr | optional boot vector |
| `priscv_riscv_irqmask` `0x111528` | gsp-priscv-bcr | RISC-V IRQ |
| `priscv_riscv_irqdest` `0x11152c` | gsp-priscv-bcr | RISC-V IRQ |

**Every register the boot-ROM handshake (facts §(b) SEQ) needs to WRITE is READABLE:**

| SEQ step | register | offset (BAR0-abs) | render11 read |
|---|---|---|---|
| 1 | `priscv_bcr_fmccode_lo` / `_hi` | `0x17111678` / `0x1711167c` | `0x00000000` / `0x00000000` |
| 1 | `priscv_bcr_fmcdata_lo` / `_hi` | `0x17111680` / `0x17111684` | `0x00000000` / `0x00000000` |
| 1 | `priscv_bcr_pkcparam_lo` / `_hi` | `0x17111670` / `0x17111674` | `0x00000000` / `0x00000000` |
| 1 | `priscv_bcr_dmacfg` | `0x1711166c` | `0x00000000` (**lock_locked = 0**) |
| 1 | `priscv_bcr_ctrl` | `0x17111668` | **`0x00000110`** |
| 4 | `priscv_cpuctl` | `0x17111388` | `0x00000010` (halted), re-read sane after 3b's engine reset |
| 5 | `priscv_br_retcode` | `0x1711165c` | `0x00000000` (rung 1 — never re-read since) |
| 7 | `falcon_hwcfg2` | `0x171100f4` | `0x0001b733`, bit13 = 1 (rung 1) |

The unreadable set is the **legacy-v1 mirror and the RISC-V interrupt / boot-vector page** — the
*observation* surface and the *optional* step 3 — not the ignition path. The ignition path is open.

### 1.2 `bcr_ctrl` is NOT zero, and nobody scored it

Rung 3's own expectation string, printed beside the value:

```
[ga10bprobe3] priscv_bcr_ctrl @0x111668 = 0x00000110 expect=0 expected — no BCR programmed (rung 1: br_retcode=0)
```

`expect=` is a **label string**, not a comparison — `ga10b_probe.rs` never compares it — so
`0x00000110` sat on the wire under an expectation of `0` across two flights and was folded into
"16 of 25 readable". It is a first-class datum and it is the pivot of this brief:

> The ACKED SEQ's step-4 value for `bcr_ctrl` is **`0x111`**. The register already reads
> **`0x110`**. The delta is **exactly bit 0.**

**INFERENCE, marked as such (no provenance, per §6 NOTE discipline):** bit 0 of `bcr_ctrl` is the
trigger/valid bit and `0x110` is the reset or firmware-left configuration of the remaining fields.
The facts file gives no bit decomposition for `bcr_ctrl` — only the offset and the two SEQ values
`0x111` (brom_config) and `0x11` (set_bcr). **This inference is not load-bearing:** rung 4b writes
the full SEQ value `0x111` and reads it back, so the wire settles the question either way, and the
inference only explains *why* the step is small. If the readback is neither `0x110` nor `0x111`,
that is the datum and the rung says so.

### 1.3 Rung 3b already proved the CCPLEX can write this engine

`MAILBOX-HELD` (`wrote=0x5a5aa5a5 read=0x5a5aa5a5`) at `0x17110040`, with the GSP halted and BR
priv-lockdown engaged. So "priv-lockdown blocks all writes from the CCPLEX" is already **false as a
blanket claim**. What is unmeasured is whether the *BCR* registers specifically accept a write.
That is rung 4a's entire question.

---

## 2. WPR-enabled + BCR-unlocked: where a buffer can go, and who may write it

### 2.1 What `opt_wpr_enabled = 1` does and does not say

`opt_wpr_enabled` is a **fuse** (facts §(b) Security-state fuses, `0x8205ec`). Reading `1` says the
die **supports** a write-protected region. It does **not** say a WPR has been programmed, where it
is, or how large it is. Three consequences, each derived from in-tree measurement:

1. **UnaOS cannot place a WPR and must not try.** A write-protected region is, by construction,
   programmed by the secure world or by an ACR image running on a verified engine. UnaOS runs at
   NS-EL1/EL2 under BL31. There is no path from here to a WPR, and this brief proposes none.
2. **Rung 4 must not go looking for the WPR.** The MC GSC / carveout-config registers are the
   honest source for the WPR extent — and this tree already **rejected that probe with a reason**:
   XCARVE-8 ([`01_BOOT_HAL/arch_arm64.md`](../01_BOOT_HAL/arch_arm64.md) §JETSON-XCARVE) records
   that reading firewalled MC/IMPDEF state from NS-EL2 is the proven JB1d/JX1 class on this
   firmware — EL3-gated access, BL31 crash / SError, box reboots — and that no such probe is
   verifiable in QEMU. **That decision stands and rung 4 inherits it: no MC GSC read.** (R19: the
   path is *failed under those conditions*, not ruled out; a later rung with an EL3-side story may
   reopen it.)
3. **The buffer rung 4 hands the boot ROM is therefore NOT the WPR.** It is ordinary CPU-owned
   non-secure DRAM. Its placement is governed entirely by this tree's own carveout machinery, and
   that machinery has already done the hard part.

### 2.2 Where a CPU-written, GPU-readable buffer can actually live on this box

Measured on the render11 wire (`awk 'index($0,"carveout")'`), this is the DRAM map rung 4 must
respect:

```
NSDRAM base: 0x80000000, end: 0x26b5f0000, size: 0x1eb5f0000        (MB2)
NSDRAM carveout encryption is enabled                                (MB2)
XCARVE-6 carveout exclusion — 6 protected window(s) UNMAPPED from the cacheable map
  window[0] [0x26b800000,0x26c180000)   9728 KiB @ GiB 9  QUIRK (extent = bounded GUESS)
  window[1] [0xbe000000,0xc4000000)    98304 KiB @ GiB 2  QUIRK (extent = bounded GUESS)
  window[2] [0x26c400000,0x279e00000) 223232 KiB @ GiB 9  QUIRK (extent = bounded GUESS)
  window[3] [0x26c180000,0x26c400000)   2560 KiB @ GiB 9  DTB /reserved-memory
  window[4] [0x26b5f0000,0x26b7f0000)   2048 KiB @ GiB 9  DTB /reserved-memory
  window[5] [0x279e00000,0x27a760000)   9600 KiB @ GiB 9  DTB /reserved-memory   (the framebuffer)
HEAP-GUARD — DTB /reserved-memory contributed 3 carveout range(s) (cap 48)
HEAP-GUARD — kernel heap [0x2683ca000, 0x26b3ca000) (48 MiB) … clear of 165 carveout range(s)
[net4A] sub-4GiB Normal-NC DMA window reserved [0x80000000, 0x80200000) (2048 KiB)
```

**Rules the buffer must satisfy, each with the in-tree fact that imposes it:**

| rule | why (measured) |
|---|---|
| **Derived, never hardcoded.** Seat it through the same law `mmu_tegra::seat_net4a_low_nc` uses: lowest 2 MiB-aligned, 2 MiB, L2-split, entirely inside one UEFI `Usable` region, clear of every carveout the heap dodges, clear of the heap span and of both NC windows. | Usable DRAM tops *exactly* at the heap top (`NET4B` header, `mmu_tegra.rs`); a fixed PA above it lands in reserved DRAM. Two of the six exclusion windows are **bounded GUESSES** whose extent has already been refuted once (XCARVE-8: the `0xbe` family reappeared 8.5 MiB above the previous top). A hardcoded PA is a bet against a guess. |
| **Sub-4 GiB.** Prefer `[0x80000000, 0x100000000)`. | The BCR DMA addresses are lo/hi pairs so the path is ≥ 32-bit, but the *effective* width of the boot-ROM DMA path is **UNKNOWN**. Sub-4 GiB is free insurance and the `[net4A]` census proves a clean block exists there. |
| **Normal-NC mapped, or explicitly cleaned to PoC before the ignition write.** | The SEQ's aperture is `target_noncoherent_system` (facts §(b), value `0x2`) — the ROM's fetch is **non-coherent**. NC is the safer of the two and the tree already has `mmu_tegra::install_nc_window` for exactly this shape. |
| **Rung 4 seats its OWN block; it never borrows the NIC's.** REFUSE (`no-dma-window`) if the law finds none. | `[net4A]`'s `[0x80000000, 0x80200000)` and the `net4B` high window are owned by `rtl8168_tegra`. Sharing a DMA window between a NIC's live rings and a GPU boot fetch is a race with no witness. |
| **Never inside, never adjacent to, a carveout window.** | Cleaning or touching a SNOC-firewalled line is *itself* the FillWrite RAS (XCARVE-3/6/7/8) — the failure family that has killed the most boots on this box. |

### 2.3 The confound nobody can remove inside rung 4 — say it before the flight, not after

```
[0001.022] I> NSDRAM carveout encryption is enabled
```

MB2 enables **encryption on the non-secure DRAM carveout**. Whether the GA10B's SNOC client sees the
same encryption context as the CCPLEX is **UNKNOWN** and is not answerable from any source this seat
holds. If it does not, the boot ROM's DMA returns bytes that are not the bytes the CPU wrote.

**Consequence, stated ahead:** a `br_retcode` FAIL (`0x2`) proves **the ROM executed**. It does
**not** distinguish

- "our payload is unsigned" (the expected, designed-for cause),
- "the payload is structurally malformed" (also expected — we do not know the manifest layout),
- "the GPU read different bytes than the CPU wrote" (the encryption confound),
- "the GPU could not read at all and the ROM timed out into FAIL".

That ambiguity is **fine for rung 4**, whose predicate is execution, not acceptance. It is **fatal
to any later rung** that tries to read a FAIL as a statement about signatures. Record it on the
verdict line itself so no later reader can lose it.

---

## 3. The rung, step by step

**Knob shape (the rung-3 precedent, verbatim).** One env knob, two values:
`UNAOS_GA10B_PROBE4=1` arms **rung 4a** alone; `=2` arms 4a then **4b**; **any other non-empty value
arms 4a alone**, so an unexpected value can never buy an ignition. Cargo features `ga10bprobe4a`
and `ga10bprobe4b`, both DEFAULT OFF, both implying `tegra`; 4b's writes live behind a
`#[cfg(feature = "ga10bprobe4b")]` helper so **no other configuration compiles a BCR write path
at all** — the invariant `ga10bprobe3b` established.

**Witness families.** `[ga10bprobe4a]` and `[ga10bprobe4b]` (15 bytes bracketed, well over the
8-byte LLVM immediate-encode floor from orin-6 §7). Both must be `strings`-proven present in the
flight artifact, not merely compiled (LAWS §Verification, full-knob gate).

**Announce discipline (inherited verbatim from 3b, unchanged).** Every read is preceded by
`about-to-read <name> reg=0x… — if this is the LAST line, THAT read was EL3-fatal and the boot ended
inside it`. Every write is preceded by `about-to-WRITE <name> reg=0x… val=0x… — if this is the LAST
line, THAT WRITE was fatal and the boot ended inside it`. Case is load-bearing: `WRITE` upper,
`read` lower (see §4 row C).

**Bracket.** Rung 4a and 4b run **inside rung 3's proven power+clock bracket, reusing its helpers**
(`pg_state` / `clk` / `settle_ms` / `r32` / `w32`) so the two cannot drift. The bracket is re-proven
on this boot — never inherited from a previous flight's log.

### 3.1 Preconditions — re-proven THIS boot, never inherited

| # | check | pass condition | STOP rule |
|---|---|---|---|
| P0 | rung 3's bracket: DTB `gpu@` walk → `MRQ_PG GET_STATE` pre-state → `SET_STATE ON` only if off → **explicit readback** → `MRQ_CLK` census (304, 41 enable; 236 answers `-22`, expected) | readback `state=0x1` | any other → `REFUSED reason=pg-*`, **zero writes**, RETURN |
| P1 | `about-to-read priscv_bcr_dmacfg reg=0x1711166c` | `lock_locked` (bit31) == 0 | bit31 == 1 → `REFUSED reason=bcr-locked` — the BCR is spent for this power cycle; **zero writes**, RETURN, and the operator's next boot must be **cold**. pri-error / all-ones → `REFUSED reason=bcr-dmacfg-unreadable` |
| P2 | `about-to-read priscv_bcr_ctrl reg=0x17111668` | any readable value; **baseline printed as `bcr_ctrl_before=0x########`** | pri-error / all-ones → `REFUSED reason=bcr-ctrl-unreadable`, **zero writes**. A value ≠ `0x00000110` is a **datum, not a stop** — print `bcr_ctrl_baseline_changed=1` and continue |
| P3 | seat the rung's own 2 MiB NC DMA window (§2.2) and print `dmabuf_pa=0x########` + `dmabuf_size=` | a block seated | none seated → `REFUSED reason=no-dma-window`, **zero writes**, RETURN |
| P4 | fill the window with a fixed non-signature pattern and print `dmabuf_pattern=0x########` | — | — |

### 3.2 Rung 4a — the BCR writability census. No ignition. Reversible. Boot RETURNS.

Six address writes, then one config write **without the lock bit**, then a full restore. Each write
is announced, issued, and immediately read back; the sequence **stops at the first mismatch** and
reports how far it got.

| # | announce | write | value | the readback that proves it | STOP rule |
|---|---|---|---|---|---|
| A1 | `about-to-WRITE priscv_bcr_fmccode_lo reg=0x17111678` | `w32` | `dmabuf_pa` low 32 | `r32` == written | mismatch → `held=0` for this reg, **stop the write list**, go to restore |
| A2 | `…fmccode_hi reg=0x1711167c` | `w32` | `dmabuf_pa >> 32` | == written | same |
| A3 | `…fmcdata_lo reg=0x17111680` | `w32` | `(dmabuf_pa + FMCDATA_OFF)` low 32 | == written | same |
| A4 | `…fmcdata_hi reg=0x17111684` | `w32` | `>> 32` | == written | same |
| A5 | `…pkcparam_lo reg=0x17111670` | `w32` | `(dmabuf_pa + PKC_OFF)` low 32 | == written | same |
| A6 | `…pkcparam_hi reg=0x17111674` | `w32` | `>> 32` | == written | same |
| A7 | `about-to-WRITE priscv_bcr_dmacfg reg=0x1711166c val=0x00000002` — **`target_noncoherent_system` ONLY; the `lock_locked` bit is DELIBERATELY NOT SET** | `w32` | `0x00000002` | `r32` == `0x00000002` **and bit31 still 0** | bit31 reads 1 → `-> BCR-SELFLOCKED`: the lock latched without being asked. **End the flight in PSCI `SYSTEM_OFF`** — the power cycle is spent and the next boot must be cold |
| A8 | `about-to-WRITE` ×7, restore: all six DMA addrs and `dmacfg` back to `0x00000000` | `w32` | `0` | all seven read `0x00000000` | any non-zero → `-> BCR-STICKY reg=0x…`: the BCR did not clear. **End in `SYSTEM_OFF`** |

`FMCDATA_OFF` and `PKC_OFF` are sub-offsets **inside** the rung's own 2 MiB window (e.g. 0, 512 KiB,
1 MiB). They are ours to choose — nothing on the die constrains them — and choosing them inside one
owned window is what keeps every address the ROM could fetch inside memory we control.

**`bcr_ctrl` is NOT written by 4a.** That single bit is 4b's, and withholding it is what makes 4a
free: it spends no power cycle, changes nothing the restore cannot undo, and the boot continues to
the desktop.

**4a summary line (exactly one).** Arms chosen so **no arm is a prefix or substring of another**
(§4 row B):

```
[ga10bprobe4a] bcrheld=<n>/7 dmabuf_pa=0x######## lock_after=<0|1> -> BCR-ALLHELD | BCR-SOMEHELD | BCR-NONEHELD | BCR-SELFLOCKED | BCR-STICKY | REFUSED reason=<pg-*|bcr-locked|bcr-dmacfg-unreadable|bcr-ctrl-unreadable|no-dma-window>
```

- **`BCR-ALLHELD`** = 7 of 7 written and read back, restore clean, lock still 0. This is the arming
  predicate for 4b, and **the only one**.
- **`BCR-NONEHELD`** with the writes announced and the boot alive = the gate is named:
  *priv-lockdown gates BCR writes from the CCPLEX while `MAILBOX0` accepts them*. That is a
  complete, publishable answer and rung 4 ends there.
- **`BCR-SOMEHELD`** = a per-register capability map; the per-register lines are the product.

### 3.3 Rung 4b — the ignition. One boot. Ends the machine.

Armed **only** by `UNAOS_GA10B_PROBE4=2` **and** a same-boot `BCR-ALLHELD` from 4a. Never armed by
a previous flight's log.

| # | announce | access | value | the readback that proves it | STOP rule |
|---|---|---|---|---|---|
| B0 | — | 4a's A1–A6 re-run (the addresses, not the restore) | as A1–A6 | each == written | any mismatch → `-> IGNITION-SKIPPED reason=bcr-addr-refused`, restore, `SYSTEM_OFF` |
| B1 | `about-to-WRITE priscv_bcr_dmacfg reg=0x1711166c val=0x80000002` — **`target_noncoherent_system` \| `lock_locked`. THIS SPENDS THE POWER CYCLE: the BCR cannot be reprogrammed again until a cold boot.** | `w32` | `0x80000002` | `r32`: bit31 == 1 | bit31 reads 0 → `lock_latched=0`, a **datum**; continue (the SEQ asks for the lock; whether the ROM requires it is UNKNOWN) |
| B2 | `about-to-WRITE priscv_bcr_ctrl reg=0x17111668 val=0x00000111` (the ACKED SEQ brom_config value; baseline was `0x00000110`) | `w32` | `0x00000111` | `r32` == `0x00000111` | mismatch → `-> BCR-CTRL-REFUSED read=0x########`. **Do NOT issue the ignition write.** Print the post-state block (B5), then `SYSTEM_OFF` |
| B3 | **step 3 of the SEQ (`riscv_boot_vector` lo/hi `0x111380`/`0x111384`) is DELIBERATELY OMITTED** | — | — | — | Those two registers are in the **9 UNREADABLE** set (`0xbadf5620`). A write there could not be read back, so it would be an unverifiable mutation — forbidden by this ladder's own discipline. The SEQ marks the step optional; we take the option to skip |
| B4 | `about-to-WRITE priscv_cpuctl reg=0x17111388 val=0x00000001` (`startcpu_true`) — **THE IGNITION.** `if this is the LAST line, THAT WRITE was fatal and the boot ended inside it` | `w32` | `0x00000001` | the poll below | last line → fail shape F1 (§5) |
| B5 | `about-to-read priscv_br_retcode reg=0x1711165c sample=<i>/<N>` — a **bounded** poll, `N` samples with a fixed settle between them, **every sample printed with its index**, exit early on result `0x2` or `0x3` | `r32` | — | `br_retcode=0x########` (fixed 10-char width, §4 row A) and `br_result=0x#` | poll expires with result `0x0`/`0x1` → `-> BROM-NOVERDICT`, then B6 disambiguates |
| B6 | post-state block, **each line prefixed `post-ignition `** (§4 row D): `about-to-read post-ignition priscv_cpuctl reg=0x17111388`; `… post-ignition falcon_hwcfg2 reg=0x171100f4`; `… post-ignition gsp_falcon_cpuctl_v1 reg=0x17110100` | `r32` ×3 | — | `post-ignition priscv_cpuctl halted=<0\|1>`; `post-ignition hwcfg2 lockdown=<0\|1>`; `post-ignition gsp_falcon_cpuctl_v1 readable=<0\|1>` | — |
| B7 | `[ga10bprobe4b] flight done — powering OFF; the dark board is the ready-for-cold-boot signal` | `power::shutdown()` (PSCI `SYSTEM_OFF`) | — | the board goes dark | **unconditional on every reachable path** |

**B6 is the rung's second, independent witness of execution.** `gsp_falcon_cpuctl_v1` at `0x110100`
read `0xbadf5620` on three flights. If it becomes readable after the ignition, GPU-side state
changed under our direction — a statement about execution that does not depend on `br_retcode` at
all. Likewise `hwcfg2` bit13: the ACKED SEQ step 7 records priv-lockdown as the post-boot
observable, and rung 1 measured it engaged. A drop is a datum of the first order.

**Why 4b ends the machine.** B1 sets `lock_locked` by design. Once set, the BCR is final for this
power cycle (facts §(b)), so the flight cannot be repeated warm, and a `SYSTEM_OFF` ending is the
bench law's "ready for cold boot" signal (2026-08-25). This is rung 1's shape, not rung 3's: **4b
does not return to the desktop.** 4a does.

**4b summary line (exactly one).** Arms mutually exclusive, none a prefix of another:

```
[ga10bprobe4b] br_retcode=0x######## br_result=0x# samples=<i>/<N> lock_latched=<0|1> post_lockdown=<0|1> v1_readable=<0|1> -> BROM-VERDICT-FAIL | BROM-VERDICT-PASS | BROM-NOVERDICT | BCR-CTRL-REFUSED | IGNITION-SKIPPED
```

- **`BROM-VERDICT-FAIL`** (`br_result=0x2`) — **THE RUNG'S PASS.** The GSP boot ROM executed, read
  our payload and rejected it. First execution of GA10B silicon under UnaOS. The verdict line must
  carry the §2.3 ambiguity note inline so it cannot be read as a statement about signatures.
- **`BROM-VERDICT-PASS`** (`br_result=0x3`) — a pattern we authored verified against NVIDIA's key.
  **Extraordinary; treat as a measurement error until re-flown.** STOP and report; do not build on
  it in the same session.
- **`BROM-NOVERDICT`** — the ROM did not reach a verdict in `N` samples. `post-ignition
  priscv_cpuctl halted=1` ⇒ it never started or halted again; `halted=0` ⇒ it is running and the
  poll was too short. Both are the next rung's input, neither is a failure.

---

## 4. The four-row bounding table

Form per orin 22 BULLETIN §14 (pi 9 + rmbp 16, 21:05Z; `docs/dev/evidence/orin22/BULLETIN.md`, on trunk at `815086e4`): **the table, not the prose** — a rule of
thumb travels faster than its test. Every new bound ships with **two wires**: the sibling it must
exclude (expect **no** hit) and the real one (expect a hit). A bound that only excludes could
exclude everything.

⚠ **Dialect note, and it changes the answer.** pi 9's remedy for a non-word-char sibling is
`(?=\s|$)`. That form is **unavailable in a `foreman` `.spec`** — LAWS §Verification: `preflight_spec`
refuses look-around and aborts the whole run with no verdict table. So every bound below is either
**structurally unambiguous by token design** (no arm is a prefix of another) or **fixed-width by
print format** — both of which are look-around free and work in an `awk` scorer *and* a spec.

| # | new witness | the SIBLING it must exclude | the naive bound and why it FAILS | the bound that HOLDS | how each is proven to fire |
|---|---|---|---|---|---|
| **A** | `br_retcode=0x00000002` (4b's PASS arm) | `br_retcode=0x00000000` (no verdict) and `br_retcode=0xbadf5620` (pri-error) | `index($0,"br_retcode=0x2")` — **FALSE MISS** on the real wire (the module prints `{:#010x}`, so the literal is `0x00000002`, and `0x2` is not a substring at that position); and against an unpadded print it would **FALSE HIT** any `0x2…`. `\b` is inert here: `0x` and the digits are word chars, so `br_retcode=0x2\b` still cannot see a padded field | **print fixed-width `{:#010x}` and match the full 10-char literal**: `index($0,"br_retcode=0x00000002")`. Every value is the same length ⇒ no value is a prefix of another ⇒ no bound is needed at all | **sibling wire:** a hand-written line with `br_retcode=0x00000000 br_result=0x0` → scorer exit 1, 0 hits. **real wire:** `br_retcode=0x00000002 br_result=0x2` → exit 0, 1 hit. Run both before the flight; a scorer that has only ever seen green is untested |
| **B** | `-> BROM-VERDICT-FAIL` | `-> BROM-VERDICT-PASS`, `-> BROM-NOVERDICT` | `index($0,"BROM")` — hits all three. A prefix-shaped arm set (`BCR-WRITABLE` / `BCR-WRITABLE-PARTIAL`) would be worse: `-` is a **non-word char**, so `\bBCR-WRITABLE\b` is satisfied by the hyphen and **FALSE HITS** the partial arm — this is exactly pi 9's `window` / `window-band` failure | **design the arms so none is a prefix or substring of another** (`BROM-VERDICT-FAIL` / `BROM-VERDICT-PASS` / `BROM-NOVERDICT`; `BCR-ALLHELD` / `BCR-SOMEHELD` / `BCR-NONEHELD` / `BCR-SELFLOCKED` / `BCR-STICKY`), then a **bare substring** bound is sound and look-around free | **sibling wire:** a capture whose summary is `-> BROM-VERDICT-PASS` → the FAIL row scores 0. **real wire:** `-> BROM-VERDICT-FAIL` → 1. Repeat pairwise across all five 4a arms — five wires, ten assertions; cheap, and the only thing that proves the arm set is actually exclusive |
| **C** | `write_announces=<n>` vs `read_announces=<n>` (the STOP-check accounting) | each other — and, in the hang case, the *last line of the capture* | `index($0,"about-to-")` — **FALSE HIT**: conflates the 2 reads and 9+ writes, so a fatal WRITE is scored as a fatal read and the brief's whole fail taxonomy collapses to one bucket | **two case-sensitive bounds with the trailing space**: `index($0,"about-to-WRITE ")` and `index($0,"about-to-read ")`, counted separately; then **assert `write_announces == write_results`** (every announced write has a readback line) and that the capture's last `[ga10bprobe4*]` line is **not** an announce | **sibling wire:** a rung-3-only capture (25 `about-to-read`, 0 `about-to-WRITE`) → `write_announces=0`. **real wire:** the 4a capture → `write_announces=7`, `read_announces≥3`, `write_results=7`. A hang wire (truncate the real wire at an announce) → the STOP-check must go red, naming that register |
| **D** | `post-ignition gsp_falcon_cpuctl_v1 readable=1` (the lockdown-drop oracle) | **rung 3's own line for the same register**, present in the same capture: `[ga10bprobe3] gsp_falcon_cpuctl_v1 @0x110100 = -UNREADABLE reason=pri-error val=0xbadf5620` | `index($0,"gsp_falcon_cpuctl_v1")` — **FALSE HIT** on the pre-ignition line and scores the PRE-state as the POST-state. Two objects, one name (memory: *matrix leg is not the image*); the register is read twice in one boot with opposite meaning | **the post read carries its own phase token and the bound includes it**: `index($0,"[ga10bprobe4b] post-ignition gsp_falcon_cpuctl_v1")`. The phase token is printed by the code, not inferred by the scorer | **sibling wire:** the render11 capture as it stands (rung 3's line only, no rung 4) → 0 hits. **real wire:** a 4b capture → exactly 1 hit. Also assert **exactly one** — two hits means the phase token was reused and the oracle is ambiguous |

**Standing addition for the implementing seat:** run every row **both directions before the
flight**, from files, capturing the *scorer's own* exit code — not a pipeline's last stage. A
`scorer … | head && echo PASS` prints PASS over a scorer that died (LAWS §Verification, pi 8's
near-miss). If you cannot name the exit code, you do not have the claim.

---

## 5. Fail shapes, named ahead

Each is a *shape on the wire*, so the operator can classify the board's behaviour without a second
opinion. Per R19, every one of these is recorded as **"failed under \<conditions\>"** with the knob
and code **KEPT** — never "ruled out".

| id | shape on the wire | reading | what rung 4 does about it |
|---|---|---|---|
| **F1** | **Hang with last announce.** The capture's final `[ga10bprobe4*]` line is an `about-to-WRITE <name> reg=0x…` (or `about-to-read`), with nothing after it and no further boot output. | **That exact access was fatal in this state.** The announce names the killer register — the whole point of the discipline. | STOP. Record the register, the value, and the full precondition state. Re-run with that one step removed (the step becomes the datum). The board likely needs a power cut. |
| **F2** | **Fabric RAS after the ignition.** `about-to-WRITE priscv_cpuctl … val=0x00000001` is followed by an SNOC/ACI RAS record (`Carveout` / `Illegal address`), an `=== AARCH64 EXCEPTION`, a BL31 "Unhandled Exception in EL3", or a spontaneous reboot. | The GSP's DMA reached memory the fabric protects — the **XCARVE family arriving from the GPU side** instead of the CPU side. The most likely cause is a payload-driven length overrun out of our 2 MiB window (the manifest layout is UNKNOWN, so the ROM's fetch extent is unbounded by anything we control). | STOP; this is the one fail shape that can take the box down hard. Record the RAS `ADDR` and compare it against the §2.2 window map. Mitigation for a re-run: seat the buffer with the *most* headroom above it, and shrink `FMCDATA_OFF`/`PKC_OFF` so a forward overrun stays inside the window longest. |
| **F3** | **Write accepted, readback zero.** `wrote=0x######## read=0x00000000` on a BCR register, boot alive. | The register is **write-masked under priv-lockdown**. Not a crash and not a mystery — it is the answer: the gate is named. | 4a stops the write list and reports `BCR-SOMEHELD`/`BCR-NONEHELD` with the per-register map. Rung 4 **ends here**, successfully. |
| **F4** | **Write accepted, readback pri-error.** `read=0xbadf5620` on a register that was readable a moment earlier. | **Ambiguous by construction** — the write may or may not have landed; the register merely stopped answering. | Report as `-UNREADABLE reason=pri-error`, **never** fold it into `held` or `not-held`. An ambiguous datum scored as either arm is worse than no datum. |
| **F5** | **`br_retcode` never leaves `0x0`.** The bounded poll prints all `N` samples, all `0x00000000`. | The ROM did not start, or is still running. | B6 disambiguates: `post-ignition priscv_cpuctl halted=1` ⇒ never started (the ignition write was masked, or `bcr_ctrl` bit0 is not the trigger); `halted=0` ⇒ **running** — and a running RISC-V core is itself execution, so this arm is *not* a failure. Report `BROM-NOVERDICT` with the halted bit. |
| **F6** | **`BCR-SELFLOCKED`** — `dmacfg` bit31 reads 1 after 4a's A7 wrote `0x00000002`. | The lock latches on any config write. 4a is then **not** free — it spends the power cycle like 4b. | End 4a in `SYSTEM_OFF` (the A7 STOP rule). Fold the finding back into the design: 4a and 4b merge into one flight and the ladder loses its cheap rung. |
| **F7** | **`BCR-STICKY`** — a DMA-address register will not clear to `0x00000000` in the restore. | The board is **not** left as found. This ladder's symmetry claim fails. | End in `SYSTEM_OFF`; the next boot must be cold. Say plainly in the report that the restore did not hold — never print `restored:` over it. |
| **F8** | **`BROM-VERDICT-PASS`** (`br_result=0x3`). | A pattern we authored verified against NVIDIA's key. Effectively impossible. | Treat as a **measurement error** until independently re-flown from a cold boot. STOP and report; build nothing on it in the same session. |
| **F9** | **The boot never reaches the rung.** No `[ga10bprobe4*]` line at all. | The knob did not arm, or the feature did not reach the artifact. | LAWS §Verification, full-knob gate: `⚡ kernel features:` must carry `ga10bprobe4a`/`4b`, and `strings kernel.elf` must find both witness families. A compiled feature is not a reachable one. |

---

## 6. What rung 4 does NOT attempt

Stated so the boundary cannot be misread as an oversight, and so no later reader treats a rung-4
result as an answer to a question rung 4 never asked.

- **It does not put a single pixel on the panel through the GPU.** The panel is the firmware's
  inherited scanout (`simple-framebuffer`, carveout `window[5] [0x279e00000, 0x27a760000)`), driven
  by the CPU. **Display through the GPU is rungs away** — it needs rung 4 to PASS with a real signed
  image, then rung 5 (host FIFO / PBDMA / runlist / RAMFC / USERD / GPU MMU), then rung 6 (GR,
  FECS/GPCCS, a golden context) — and rung 6 additionally needs a **GA10B SASS assembler, for which
  no public specification exists at all**: a second wall, independent of the firmware one.
- **It touches no display register.** No `NV_PDISP` offset, no nvdisplay aperture, no window or head
  or SOR. The S2/S4 shut-out register ([`GA10B-HISTORY.md`](../../evidence/orin14/GA10B-HISTORY.md) §2) stands untouched, and S5's hazard —
  `MRQ_PG SET_STATE` on the display domain destroys the inherited scanout — is not approached.
- **It does not fetch, stage, name-as-a-file, or embed any NVIDIA firmware blob.** The L4T GA10B
  firmware set is deliberately absent from this design. **Peter's blob ruling is not a precondition
  for rung 4 as designed** — that is the central change this brief makes to the ladder, which had
  rung 4 recorded as "blocked on Peter's blob ruling" ([`GA10B-LADDER.md`](../../evidence/orin14/GA10B-LADDER.md)).
- **It does not probe for the WPR.** No MC GSC / carveout-config register is read (§2.2, XCARVE-8's
  rejected probe). Nothing is inferred about where the WPR is; only the fuse is quoted.
- **It does not issue `MRQ_STRAP`.** Facts §(a): on silicon the PG straps are BPMP's and software
  must not program them. A read-only strap *query* is at most a rung 4c, one MRQ, its own boot.
- **It does not touch PFIFO, PBDMA, runlist, RAMFC, USERD, any CE or GR or 2D engine register, or
  the GPU MMU / SMMU.** Those are rungs 5–6 and they need a Group A pass roughly 10× the rung-1 one.
- **It does not ignite the PMU.** `pmu_falcon2_cpuctl` at `0x1710b388` stays read-only, exactly as
  rung 3 left it.
- **It does not prove the wall is the signature.** A FAIL verdict proves the ROM *ran and rejected*.
  It does not prove a correctly signed image would pass, that we could obtain one, or that the
  rejection was about signing at all (§2.3).
- **It does not claim the board is left as found on the 4b path.** 4b spends the power cycle by
  design and says so before it writes.

---

## 7. Preconditions on the ladder, and what rung 4 leaves open

**Rungs this one depends on being OPEN** ([`RULINGS.md`](../../RULINGS.md) R18/R19: probe boot by boot; a failed path stays open, and every rung names the earlier rungs it needs open). Ledger rows: [`orin-ledger.md`](../orin-ledger.md) A24, A39.

- **Rung 1** — the rail is ON at handoff without any MRQ; `opt_priv_sec_en=1`; BR never ran.
- **Rung 2** — BPMP permits the CCPLEX to drive the GPU domain and clocks (`err=0` throughout, no
  `-EACCES` class anywhere); `PMC_BOOT_0 = 0xb7b000a1`.
- **Rung 3** — the 16-of-25 readability map, `lock_locked=0`, `bcr_ctrl=0x00000110`.
- **Rung 3b** — a GA10B engine register accepts and holds a CCPLEX write with the GSP halted
  (`MAILBOX-HELD`), and the engine survives an assert→hold→deassert reset.

If any of those is later found to have depended on a condition that has changed (a firmware update,
a different UEFI, a different power state), **rung 4 re-runs its preconditions rather than assuming
them** — which is why §3.1 re-proves all four on the boot, from the wire, every time.

**What rung 4 leaves for whoever comes next:** the manifest layout at `fmccode`/`fmcdata`/`pkcparam`
(sizes, alignments, order) is UNKNOWN and stays UNKNOWN; `br_retcode`'s upper bits may or may not
carry a reason code (UNKNOWN — print the full 32 bits and find out); whether the boot-ROM DMA path
is pre-SMMU physical is UNKNOWN (rung 4's REFUSE-on-no-window and its own owned buffer are the
hedge); and the NSDRAM-encryption confound of §2.3 is not resolvable inside this rung.

---

## 8. Questions only Peter can answer

These are asked, not guessed. Nothing in this brief is implemented before they are answered.

1. **Does the redesign stand?** The ladder ([`GA10B-LADDER.md`](../../evidence/orin14/GA10B-LADDER.md) §Rung 4) records rung 4 as *"blocked on Peter's blob ruling"*.
   This brief unblocks it by changing what rung 4 **is**: a blob-free ignition whose success
   condition is a **FAIL** verdict. That changes the rung's meaning, not just its schedule, so it
   wants ratification rather than a seat's own call. Should rung 4 be the blob-free ignition, or do
   you want the blob path pursued first (which needs the licence ruling and an enumeration of the
   L4T firmware set on a JetPack 6.2 rootfs)?

2. **Is 4b flown at all, and attended?** 4b **spends the power cycle** (`lock_locked` is set by
   design) and **ends the machine** in `SYSTEM_OFF`. Fail shape **F2** — a GPU-side fabric RAS —
   can take the box down hard enough to need a power cut. That is the same risk class as the XCARVE
   family that has killed the most boots on this board. Do you want 4b flown attended at the bench,
   and is an unrecoverable boot needing a manual power cut an acceptable cost for the first
   execution of GPU silicon?

3. **4a alone first, or 4a+4b in one sitting?** 4a is free — it changes nothing the restore cannot
   undo, and the desktop comes up behind it. It answers *"do the BCR registers accept a CCPLEX
   write?"*, which is the whole gate. Flying 4a alone costs one boot and could end the ladder with
   a clean, publishable `BCR-NONEHELD`. Flying both costs the machine. Preference?

---

## 9. Verification posture of this document

Per orin 22 BULLETIN §14 as sharpened by pi 9: a gate is skipped only when the change cannot affect **what
the gate asserts**, and the assertion must be **named**.

- This arc adds **one Markdown file** and touches **no `.rs` file**. The kernel batteries assert
  properties of compiled kernel images (`arroyo check`'s per-leg type-check; the QEMU specs'
  runtime witnesses; `kernel8.img` byte identity). **No assertion any of them makes can be affected
  by a file the build does not read** — this is the "no `.rs` file touched" safe form, not a bare
  "n/a", and specifically not the rmbp B94 case (a comment inside a `.rs` file moves
  `panic::Location` line numbers and therefore image bytes; this file is not in a `.rs`).
- The document's own claims are verified by re-reading the render11 wire with
  `LC_ALL=C awk 'index($0,"[ga10bprobe")'` and `index($0,"carveout")` (control bytes in these logs
  break `grep`), and every register/bit/constant is cross-checked against the ACKED facts file and
  `ga10b_probe.rs`.
- **The four-row table in §4 is a design, not a run.** Its two-wire executions are owed by the
  implementing seat **before the flight**, per BULLETIN §14 (21:55Z) — "a check being CORRECT and a
  check being RUN are different questions."

---

## 10. Rung 4c — the post-ignition census (and the deferred arm)

**Status: designed and built by exec-orin27-ga10b4c (2026-09-12), from two rulings the same day — Peter:
"can you add more GPU probes to the next boot", then "the next boot has a huge list of things to test?" —
the second of which makes the next boot a FULL desktop session first and the GPU probe LAST.** Rungs 4a+4b
flew once (2026-09-12T00:21Z, ledger A51, `docs/dev/evidence/orin27/ga10b4ab-boot1.log`) and answered the one
question they asked. Each further boot spends a power cycle on the ignition, so the next boot repeats 4a+4b
unchanged and wraps them in **rung 4c: more READ-ONLY probes around the ignition**, so one boot answers more
of §7's open questions and pre-surveys rung 5. 4c adds **zero write classes**: 4b's seven BCR writes, the
lock, the trigger and the ignition remain the only GA10B writes in the boot, and the flight still ends in
`SYSTEM_OFF` on every path (4c implies 4b, and 4b ends the machine).

### 10.1 Knob shape

One env knob, now four values (`unaos/arroyo`; Cargo features `ga10bprobe4c = ["ga10bprobe4b"]`,
`ga10bprobe4d = ["ga10bprobe4c"]`, both DEFAULT OFF):

| `UNAOS_GA10B_PROBE4=` | features | what the boot does |
|---|---|---|
| `1` | `4a` | 4a alone, RETURNS to the desktop (unchanged) |
| `2` | `4a 4b` | 4a then the ignition, `SYSTEM_OFF` before the desktop (unchanged — the flown shape) |
| `3` | `4a 4b 4c` | 4a, 4c pass 1, 4b, 4c pass 2, `SYSTEM_OFF` before the desktop |
| `4` | `4a 4b 4c 4d` | **DEFERRED**: the post-heap-init call arms a flag and RETURNS; the whole desktop session runs; the rung (4a → 4c-1 → 4b → 4c-2) fires when a PSCI `SYSTEM_OFF` is requested, then the OFF |
| anything else | `4a` | 4a alone — an unexpected value never buys an ignition (the PROBE3 shape) |

**The knob line for the next boot** (Peter's ruling: the fifteen-knob flight line of the last render
MANIFEST, `~/unaos-bench/flash/orin/render12-20260909T1556Z-1b50376/MANIFEST`, plus `UNAOS_GA10B_PROBE4=4`):

```
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 UNAOS_TCURX=1 UNAOS_BSPTICK=1 UNAOS_BSPRUN=1 UNAOS_NET4=1 UNAOS_NET5=1 UNAOS_GA10B_PROBE3=2 UNAOS_GA10B_PROBE4=4 ./arroyo esp-jetson
```

That line carries `UNAOS_GA10B_PROBE3=2` because render12's did — see §10.7, the one decision this section
puts to Peter.

### 10.2 The probes — taken, and dropped with the reason

Candidates (a)–(f) from the executor brief, each judged by one test: **can the ACKED facts file bound every
offset it reads?** (§0; the ladder's provenance table names that file as the source every register offset
must trace to).

| # | probe | verdict | what it reads | facts-file citation |
|---|---|---|---|---|
| (a) | `br_retcode` over TIME | **taken** | 20 samples, 10 ms apart, after 4b's poll exited; every DISTINCT value with its sample index and elapsed ms; bits[31:2] printed as `reason_bits` and OR-ed across the series | §(b) RISC-V boot-ROM interface: `br_retcode 0x65c`, result bits[1:0] |
| (b) | the full BCR readback AFTER ignition | **taken, folded into (c)'s post pass** | `bcr_ctrl`, `bcr_dmacfg`, the six DMA address registers — each judged `intact=` against the value 4b WROTE (`0x111`, `0x80000002`, the six halves of the window addresses), never against the pre pass | §(b): `bcr_ctrl 0x668`, `bcr_dmacfg 0x66c`, addrs `0x670..0x684` |
| (c) | the rung-3 readability map re-run post-ignition | **taken, as TWO passes** | rung 3's 25 registers (built from the SAME offset constants rung 3 reads — the cfgs were widened in place, so "same offsets" holds by construction) **plus five**: `top_num_gpcs` (rung 1's), `gsp_falcon_hwcfg2` (rung 1's, 4b's), `priscv_cpuctl` and `priscv_br_retcode` (rung 1's, 4b's), `gsp_falcon_mailbox0` (rung 3b's) — 30 in all, in rung 3's risk order, PRE-ignition (after 4a's restore, before 4b, same bracket) and POST-ignition, each post line carrying `diff=same | changed | became-readable | became-unreadable | written-by-4b` beside its pre value | §(b) Security-state fuses; §(b) Die-characterization; §(b) Legacy Falcon regs; §(b) RISC-V boot-ROM interface; §Aperture framing (PMU falcon2 base). **`mailbox0` is the one non-facts-file offset**: PUBLIC-RECALLED (nouveau `nvkm/falcon`, open-gpu-kernel-modules `dev_falcon_v4.h`, MIT), metal-proven on this die by rung 3b (render11 `MAILBOX-HELD` — a WRITE and a read of that exact address). A read is strictly less than what 3b already did there; the recalled status is printed on its announce line, as 3b printed it |
| (d) | mailbox0/mailbox1 and ROM-written status registers | **mailbox0 taken (in (c)); mailbox1 DROPPED; "status/debuginfo" DROPPED** | — | The facts file names NO mailbox and NO ROM status/debuginfo register. `mailbox1` (+0x044) is recalled only and has never been touched on this die: no metal proof, no facts-file line — dropped. The only ROM-written word the facts file names is `br_retcode` itself, which (a) covers |
| (e) | our DMA window post-ignition | **taken** | a CPU read (through the rung's own Normal-NC mapping — DRAM, not a GPU register) of the first four words at `fmccode` (+0), `fmcdata` (+512 KiB) and `pkcparam` (+1 MiB), then a scan of every word of the 2 MiB window against the fill pattern `0x4a10b4a5`: `words_changed=`, `first_changed_off=`, `first_changed_val=` | not a register; the window is 4a's own block (`[ga10b4nc]`), `dsb sy` before the read, no cache games beyond 4a's |
| (f) | a PRE-ignition census of rung 5's apertures | **taken only where the facts file reaches; the rest DROPPED** | `mc_enable`, `mc_elpg_enable`, `top_device_info_cfg`, `top_num_gpcs` — all in (c)'s pre pass, before 4b, inside the bracket, so an F2 fabric fault after the ignition cannot take them with it | §(b) Die-characterization. **Host FIFO / PBDMA / CE / runlist registers: DROPPED** — the facts file has no offset for any of them (ladder §Rung 5: "UNKNOWN facts — Group A pass"). `mc_device_enable(i)` is named by the facts file as an indexed register but with NO offset — dropped for the same reason |
| — | `pgsp_falcon_engine` (0x1103c0), `falcon bootvec` (0x104) | **DROPPED** | — | both are in the facts file, but the engine-reset register is a WRITE-class register whose read semantics the file does not state, and `bootvec` was never read by any rung; neither answers a §7 question. Named here so the omission reads as a choice |

**One datum already on the wire that 4c is built to settle.** Rung 1 read `falcon_hwcfg2 = 0x0001b733`;
4b's post-ignition read (same register, one boot later) was `0x0001a733`: **bit 12 differs.** The facts file
decodes only bit 13 (priv-lockdown), so bit 12 is unnamed, and whether it moved across the ignition or across
the weeks between the flights is unmeasured. 4c reads `hwcfg2` in both passes of the same boot; its `diff=`
answers that directly.

### 10.3 Placement and order on the wire

```
[ga10bprobe4a] rung 4a … (unchanged)            [ga10bprobe4b] rung 4b ARMED …
[ga10bprobe4c] rung 4c ARMED … vocabulary …
[ga10bprobe4a] … bcrheld=7/7 … -> BCR-ALLHELD          (4a, unchanged)
[ga10bprobe4c] pass 1 — PRE-ignition census …
[ga10bprobe4c] about-to-read pre-ignition <name> reg=0x… — if this is the LAST line, THAT read was EL3-fatal …
[ga10bprobe4c] pre-ignition <name> @0x<off> = 0x######## | -UNREADABLE reason=<all-ones|pri-error> val=0x########
        … ×30 (each address class announced once: pre-ignition address class <class> (KNOWN …)) …
[ga10bprobe4c] pre-ignition census: readable=<n>/30 unreadable=<n> -> PRECENSUS-DONE
[ga10bprobe4b] rung 4b — the IGNITION … (unchanged: B0 … B7, the summary, the PASS text)
[ga10bprobe4c] pass 2 — POST-ignition …
[ga10bprobe4c] about-to-read series priscv_br_retcode reg=0x1711165c samples=20 settle_ms=10 …
[ga10bprobe4c] series sample=<i>/20 t_ms=<t> br_retcode=0x######## br_result=0x# reason_bits=0x######## (distinct #<k>)   … one per DISTINCT value
[ga10bprobe4c] series priscv_br_retcode @0x65c = distinct=<k> first=0x########@<i> last=0x########@<i> reason_bits_or=0x######## samples=20 elapsed_ms=<t> -> BRSERIES-STABLE | BRSERIES-CHANGED | BRSERIES-UNREADABLE
[ga10bprobe4c] about-to-read post-ignition <name> reg=0x… — …
[ga10bprobe4c] post-ignition <name> @0x<off> = 0x######## diff=<same|changed|became-readable|became-unreadable|written-by-4b> pre=0x########[ intact=<0|1>]
[ga10bprobe4c] post-ignition <bcr name> expected=0x######## intact=<0|1>          (the eight 4b wrote)
        … ×30 …
[ga10bprobe4c] bcr post-ignition: intact=<n>/8 altered=<n> unreadable=<n> -> POSTBCR-INTACT | POSTBCR-ALTERED | POSTBCR-UNREADABLE
[ga10bprobe4c] post-ignition census: readable=<n>/30 unreadable=<n> became_readable=<n> became_unreadable=<n> changed=<n> changed_ex_bcr_retcode=<n> pre_done=1 -> MAPDIFF-SAME | MAPDIFF-CHANGED
[ga10bprobe4c] about-to-read post-ignition dmabuf fmccode pa=0x80200000 … / fmcdata / pkcparam
[ga10bprobe4c] post-ignition dmabuf fmccode @0x80200000 = 0x######## ×4 (first 4 words; fill pattern=0x4a10b4a5)
[ga10bprobe4c] about-to-read post-ignition dmabuf-scan pa=0x80200000 size=0x200000 …
[ga10bprobe4c] post-ignition dmabuf-scan @0x80200000 = words_changed=<n>/524288 first_changed_off=<0x…|none> [first_changed_val=0x########] -> DMABUF-UNTOUCHED | DMABUF-ALTERED
[ga10bprobe4c] rung 4c complete: reads_announced=<n> reads_answered=<n> (zero writes) -> CENSUS-COMPLETE
[ga10bprobe4b] flight done — powering OFF …          [pwrshutoff] PSCI SYSTEM_OFF (0x84000008) via SMC …
```

Pass 1 runs on the rail the explicit `pg` readback proved ON, whatever 4a's census arm was (a read needs
only the rail; the two 4a arms that end the machine, SELFLOCKED and STICKY, do so before pass 1 and are
recorded as such). If the rail was not proven ON, pass 1 prints `-> PRECENSUS-SKIPPED reason=pg-not-on` and
reads nothing. Pass 2 runs only on the path where the ignition was issued; on `IGNITION-SKIPPED` and
`BCR-CTRL-REFUSED` 4b ends the machine as before and there is nothing to census. `MAPDIFF-SAME` means: no
register changed readability and no value moved except the eight 4b wrote and the verdict register.

**Vocabulary** (every 4c summary line ends in one; no arm is a prefix or substring of another or of any
4a/4b arm, and no 4c line carries ` wrote=0x` or `about-to-WRITE ` — the 4a/4b scorer's write-accounting
tokens, so the flown rungs score exactly as before on a 4c capture):
`PRECENSUS-DONE | PRECENSUS-SKIPPED ; BRSERIES-STABLE | BRSERIES-CHANGED | BRSERIES-UNREADABLE ;
POSTBCR-INTACT | POSTBCR-ALTERED | POSTBCR-UNREADABLE ; MAPDIFF-SAME | MAPDIFF-CHANGED ;
DMABUF-UNTOUCHED | DMABUF-ALTERED ; CENSUS-COMPLETE`.

### 10.4 The deferred arm (`=4`): the desktop first, the rung last

Peter, 2026-09-12: the next boot must run the full desktop first and the GPU probe last, so one boot carries
the render13 glass checks and rungs 4a+4b+4c. Mechanism (`ga10bprobe4d`):

- **Arm.** The post-heap-init call (`main.rs`, unchanged) still calls `ga10b_probe::ga10bprobe4_run`; under
  `ga10bprobe4d` that entry stashes `dtb_addr`, `dtb_size`, `ram_gib_mask` and a whole-blob DTB checksum in
  statics, sets an `AtomicBool`, prints `[ga10bprobe4d] DEFERRED ARM …` and RETURNS. Nothing else changes at
  boot: no BPMP transaction, no BAR0 touch, the desktop comes up.
- **Trigger.** `power::psci_call` — the ONE function every PSCI `SYSTEM_OFF` on this board goes through
  (`power::shutdown()` for the shell's `shutdown`/`off`, `power::crystal_shutdown()` for the crystal menu's
  Shut Down, and a probe's own `finish4`) — calls `ga10b_probe::ga10bprobe4_deferred_run()` (no arguments)
  when the function id is `SYSTEM_OFF`, before the SMC. The call is appended to an existing line, cfg-gated,
  so knob-off it is erased and no `panic::Location` moves. The crystal route does **not** pass through
  `power::shutdown()` (A34's fix gave it its own `[crystal]`-family terminus), which is why the hook sits in
  `psci_call` and not in `shutdown()`. `SYSTEM_RESET` (Restart / `reboot`) does NOT trigger it: after 4b's
  lock the next boot must be cold, and a warm reset would hand the next boot a locked BCR.
- **Run.** The flag is CONSUMED on entry (so the rung's own `finish4 → shutdown → psci_call` re-entry falls
  straight through to the OFF). DAIF is masked on the calling core. The first line names the arm and the
  trigger (`[ga10bprobe4d] DEFERRED RUN — triggered by a PSCI SYSTEM_OFF request reaching power::psci_call
  …`; the `[pwrshutoff]`/`[crystal]` line above it names the route). The DTB checksum is re-taken and
  compared (`REFUSED reason=dtb-changed`, zero MMIO, on a mismatch); the BPMP channel is re-derived with
  `bpmp_geometry` + `chan_reopen` from the same DTB geometry as at boot; then `ga10bprobe4_body` runs
  4a → 4c-1 → 4b → 4c-2 exactly as at boot — 4a's bracket (pg pre-state, explicit ON readback, clock census)
  is re-proven from BPMP at that moment; nothing is inherited from boot except the three DTB coordinates —
  and 4b's `finish4` ends the machine. Any refusal prints why and RETURNS so the OFF it interrupted proceeds.
- **Stated assumptions (concurrency).** At shutdown time the other cores are hosting (apsrun). The design
  assumes (1) nothing after boot uses the BPMP channel — every in-tree BPMP user (`sdmmc_tegra` CLKPROOF,
  `xusb_tegra`, `display_tegra`, the probe rungs) runs during init, `population=grep -rln 'bpmp_tegra::'
  unaos/crates/kernel/src`, and none holds a channel afterwards; (2) nothing touches the GPU aperture at
  all — `population=grep -rn '0x1700_0000\|0x17000000' unaos/crates/kernel/src --include=*.rs` outside
  `ga10b_probe.rs`, `hits=0`; (3) the DTB blob is untouched after boot — the aarch64 path has no frame
  allocator (the heap is a fixed window) and the checksum turns this from an assumption into a measurement;
  (4) the serial lock and `chan.transfer`'s 100 ms bound behave under DAIF-masked polling as they do at
  boot. The other cores are not stopped: a PSCI `SYSTEM_OFF` is a whole-system OFF whatever they are doing,
  and the rung's ~3 s of announced reads on one core changes nothing they touch.

### 10.5 The bounding table (§4 form: the sibling each new witness must exclude, two wires each)

| # | new witness | the SIBLING it must exclude | the naive bound and why it FAILS | the bound that HOLDS | how each is proven to fire |
|---|---|---|---|---|---|
| **E** | `[ga10bprobe4c] post-ignition gsp_falcon_cpuctl_v1 @0x110100 = …` (the generalised oracle) | `[ga10bprobe4b] post-ignition gsp_falcon_cpuctl_v1` (4b's own D-row line, same capture) and `[ga10bprobe4c] pre-ignition gsp_falcon_cpuctl_v1` | `index($0,"post-ignition gsp_falcon_cpuctl_v1")` hits 4b's line too and breaks the D row's "exactly one" | the family token is INSIDE the bound: `index($0,"[ga10bprobe4c] post-ignition …")`; 4b's D-row bound already carries `[ga10bprobe4b]` and is untouched — measured: the orin26 scorer's `4b` still reports `post-ignition v1 lines = 1` on the expected-4c wire | **sibling wire:** the real 4a+4b capture → the 4c rung returns NO VERDICT (exit 2), and `4b` still PASSes. **real wire:** the expected-4c wire → `4c` exit 0, `4a`/`4b` exit 0 |
| **F** | 4c read accounting: `[ga10bprobe4c] about-to-read ` vs its result lines | 4a/4b's `about-to-read ` (the same token, other families) and 4c's own non-result lines (class announces, distinct-sample lines, `expected= intact=` lines) | counting `about-to-read ` alone conflates the three families; counting ` = ` alone hits the ARMED prose | announces: `index($0,"[ga10bprobe4c] about-to-read ")`; results: `index($0,"[ga10bprobe4c]") && index($0," @0x") && index($0," = ")` — every 4c read prints exactly one line of that shape and no other 4c line has ` @0x` | **sibling:** delete one result line (fixture `m1-no-result`) → exit 1. **real:** expected-4c wire → `read_announces == read_results`, exit 0 |
| **G** | `-> CENSUS-COMPLETE` (the rung's terminal) | a truncated capture whose last 4c line is an announce (the F1 hang), and a capture missing the line | "any 4c line present" would pass a hang | exactly one `[ga10bprobe4c] … -> CENSUS-COMPLETE`, AND the orin26 STOP-check (last `[ga10bprobe4*]` line is not an announce — it already matches the 4c family by its `[ga10bprobe4` prefix), AND ≥1 `PSCI SYSTEM_OFF (0x84000008) via SMC` | **sibling:** `m3-hang` (truncated at a 4c announce) → exit 1; `m4-no-complete` → exit 1; `m2-no-off` → exit 1. **real:** exit 0 |
| **H** | the ` wrote=0x` write-accounting token of the 4a/4b scorer | 4c's BCR readback lines, which print what 4b wrote | printing `wrote=0x…` on a 4c line would inflate `write_results` and red the flown rungs' STOP-check | 4c prints `expected=0x…` and never ` wrote=0x` or `about-to-WRITE ` | measured: `4a`/`4b` exit 0 on the expected-4c wire (`write_announces=23 = write_results=23`) |

Executed before the flight, from files, capturing the scorer's own exit code (never a pipeline's last stage):
`docs/dev/evidence/orin27/scorer-ga10b4.sh --selftest docs/dev/evidence/orin27/ga10b4ab-boot1.log` builds
the expected-4c wire by splicing the §10.3 lines into a copy of the real capture before its `[pwrshutoff]`
lines, mutates it four ways, and scores six cases: `real-4a4b-only → 2`, `expected-4c → 0`, `m1-no-result
→ 1`, `m2-no-off → 1`, `m3-hang → 1`, `m4-no-complete → 1`. The orin26 scorer is untouched.

### 10.6 Fail shapes, named ahead (R19: each is "failed under \<conditions\>", knob and code kept)

| id | shape on the wire | reading | what happens |
|---|---|---|---|
| **F10** | the last `[ga10bprobe4c]` line is `about-to-read pre-ignition <name> …` | a register readable on render11 became EL3-fatal in the post-4a state (rung 3's classes are all KNOWN; the only register never read pre-reset on this die is `mailbox0`, and its announce says so) | STOP; the announce names the register; re-run with it removed (the removal is the datum). 4b never ran: the power cycle is NOT spent |
| **F11** | the last line is a `post-ignition` announce | the register became fatal after the ignition — the F2 family at register granularity | STOP; record the register; the ignition already spent the cycle, so the next boot is cold either way |
| **F12** | `PRECENSUS-DONE` with `unreadable≠9` (of the 25) | the readability map differs from render11's before anything was ignited (a firmware update, or 4a's writes changed the map) | a datum, not a stop; the per-register lines are the product |
| **F13** | `BRSERIES-CHANGED` | the verdict moved after 4b read it (the ROM re-ran, or the upper bits latched later) | a datum: the distinct lines carry index and time |
| **F14** | `POSTBCR-ALTERED` | the ROM (or the lock) rewrote its own descriptor — the BCR is not a passive mailbox | a datum for rung 5's design; `MAPDIFF` still scores the rest |
| **F15** | `DMABUF-ALTERED` | something wrote into the window: the ROM DMA'd INTO it (a status block, a manifest echo), or the NSDRAM-encryption confound (§2.3) surfaces as scrambled bytes | `first_changed_off`/`_val` locate it; the next rung reads the whole changed span |
| **F16** | `[ga10bprobe4d] DEFERRED ARM` printed, no `DEFERRED RUN` ever printed | the session ended without a `SYSTEM_OFF` request (power cut, a `reboot`, a hang elsewhere) | the rung did not run and nothing was spent; the next boot is warm-safe |
| **F17** | `DEFERRED RUN` then `REFUSED reason=dtb-changed` | the DTB blob moved after boot | zero MMIO; the OFF proceeds; the assumption in §10.4 (3) is falsified and becomes the finding |
| **F18** | `DEFERRED RUN` then 4a `REFUSED reason=pg-*` | BPMP would not answer or prove the rail at shutdown time (the channel state after a full session differs from boot) | zero BAR0 touch; the OFF proceeds; the `=3` immediate shape is the fallback for the boot after |

### 10.7 Questions only Peter can answer

None block the build. One decision rides on the knob line:

1. **`UNAOS_GA10B_PROBE3=2` in the fifteen-knob line.** render12's line carries it, so the line ruled for the
   next boot runs rungs 3 and **3b** at boot — 3b asserts and deasserts a GSP engine reset and writes
   `MAILBOX0` — hours before the deferred rung 4 runs. The flown 4a+4b boot ran WITHOUT `PROBE3` (§3's
   recipe, `# KNOBS: UNAOS_GA10B_PROBE4=2`), and no rung-4 flight has been measured from a post-reset engine
   state. Recommendation: **drop `UNAOS_GA10B_PROBE3=2` from the line** — rung 3's data are already in hand
   (render8, render11) and 4c's pass 1 re-reads all 25 of its registers in the same boot, so nothing is lost,
   and 4c's diff is then measured against the same pre-ignition state the flown boot had. Keeping it is
   also defensible (one boot, more data) — but then the 4c baseline is "post-3b-reset", and the report must
   say so. The line in §10.1 is Peter's ruling as given; the executor changed nothing in it.

### 10.8 Verification posture of this section

`UNAOS_TEGRA=1 UNAOS_LEDGER_STRICT=1 UNAOS_K8REACH_STRICT=1 ./arroyo check` and the same without `UNAOS_TEGRA`
(the `arm-tegra-ga10bprobe4c` / `arm-tegra-ga10bprobe4d` legs type-check the two new polarities; the
`arm-tegra-ga10bprobe3` leg still compiles the widened constants without 4c); `./arroyo knoboff ga10bprobe4c`
(the knob-off loadable images against the baseline); `UNAOS_GA10B_PROBE4=3` and `=4 ./arroyo esp-jetson` with
`LC_ALL=C grep -a -o -F '[ga10bprobe4c]' target/aarch64_esp/kernel.elf | wc -l` (the witness family reachable
in the flight artifact, not merely compiled); the scorer selftest above. No QEMU models the Jetson; a green
here certifies that it compiles and links (LAWS §Gates). Exit codes are in the commit and ledger A55.

## 11. Rung 5 — the sequel brief

Everything §6 and §7 left open — the FMC/BCR descriptor layout from public sources, why a `0x2` cannot be read as a statement about signatures, the honest fork on whether any blob-free GPU engine path exists on GA10B, the L4T firmware facts for Peter's ruling, and rung 5's conditional design — is in [`GA10B-RUNG5-BRIEF.md`](GA10B-RUNG5-BRIEF.md) (ledger [`orin-ledger.md`](../orin-ledger.md) A61, open).

---

## 12. Rungs 4e and 4f — the shift arm and the BRFETCH arm

**Status: built by orin-0912b (2026-09-12), ledger [`orin-ledger.md`](../orin-ledger.md) A62, from
[`GA10B-RUNG5-BRIEF.md`](GA10B-RUNG5-BRIEF.md) §2.6 and its §6 Q4/Q5 — the two questions on that page
that need no ruling from Peter, because neither spends anything but a power cycle and neither goes
near a vendor blob.** Each arm is ONE BOOT. Each is the flown `=2` rung with exactly ONE value
changed. Neither adds a write class, a register, an address class or a new fact; both end in
`SYSTEM_OFF` on every reachable path, as `=2` does.

### 12.1 What changes, and what does not

| | rung 4e — SHIFT | rung 4f — BRFETCH |
|---|---|---|
| knob | `UNAOS_GA10B_PROBE4=5` (`ga10bprobe4e`) | `UNAOS_GA10B_PROBE4=6` (`ga10bprobe4f`) |
| the one delta | the six BCR DMA address registers are written `pa >> 8`: LO = low 32 bits of `pa >> 8`, HI = the rest | `bcr_ctrl` is written `0x00000011` instead of `0x00000111` |
| why | NVIDIA's published MIT Hopper GSP-FMC bootstrap writes those registers in 256-byte units (rung-5 brief §1.2); rung 4b wrote them RAW, so under that encoding the flown ignition pointed the ROM about 512 GiB above a machine whose NSDRAM ends near 9.7 GiB | the MIT GA102 `dev_riscv_pri.h` decomposes the register exactly — BRFETCH bit 8, CORE_SELECT bit 4, VALID bit 0 — and `0x011` is the ACKED SEQ's alternate `set_bcr` value: BRFETCH FALSE (rung-5 brief §2.2) |
| the tag on the wire | `shift=8` | `brfetch=false` |
| unchanged | `bcr_dmacfg`, `bcr_ctrl` = `0x00000111`, the ignition, the 16-sample poll, the post-ignition block, `SYSTEM_OFF` | the six addresses (raw, as flown), `bcr_dmacfg`, the ignition, the poll, the post-ignition block, `SYSTEM_OFF` |

Both arms carry rung 4a's census unchanged, both are armed only by a same-boot `BCR-ALLHELD`, and
**neither carries rung 4c** — `=5` and `=6` compose `ga10bprobe4a` + `4b` + the arm, and nothing else,
so a `=5`/`=6` wire is the `=2` wire plus a tag. That is deliberate: the arm's question is one value,
and adding the census would put a second variable on the same power cycle.

⚠ **One thing on the wire reads oddly and is correct.** Rung 4b's own ARMED banner still says
`(UNAOS_GA10B_PROBE4=2)` on a `=5` or `=6` boot. It is the same rung, running the same code, and the
banner is the line whose bytes hold the flown images identical; the arm announces itself and the knob
value that armed it on its own `[ga10bprobe4e]` / `[ga10bprobe4f]` banner immediately above 4b's.

### 12.2 The registers, raw and shifted, for the window this board seats

The rung's own 2 MiB Normal-NC window has come up at `dmabuf_pa = 0x80200000` on every flight so far
(`[ga10b4nc] rung-4 DMA window reserved [0x80200000, 0x80400000)`). For that window:

| register | off | raw (flown `=2`, `=4`) | `pa >> 8` (rung 4e) |
|---|---|---|---|
| `fmccode_lo` | `0x678` | `0x80200000` | `0x00802000` |
| `fmccode_hi` | `0x67c` | `0x00000000` | `0x00000000` |
| `fmcdata_lo` | `0x680` | `0x80280000` | `0x00802800` |
| `fmcdata_hi` | `0x684` | `0x00000000` | `0x00000000` |
| `pkcparam_lo` | `0x670` | `0x80300000` | `0x00803000` |
| `pkcparam_hi` | `0x674` | `0x00000000` | `0x00000000` |

All three buffer addresses are 256-byte aligned (their low byte is zero), so the shift is lossless —
which is the precondition §1.2 of the rung-5 brief names, satisfied here by accident of the window's
2 MiB alignment and NOT by a rule this rung installs. A real FMC image would need its own.

The HI halves stay zero either way: the window is below 4 GiB, and shifting a sub-4 GiB address right
by 8 cannot reach bit 32. **So the whole of rung 4e's delta is in the three LO registers**, and the
`hi` lines are on the wire to say so, not to carry information.

### 12.3 What is measured

Every write is announced before it is issued and read back immediately after, as rung 4 has always
done. Rung 4e's announce and result lines carry BOTH numbers — `raw_pa=` beside `val=`/`wrote=`, plus
`shift=8` — so the capture states the address the rung meant and the number it actually put in the
register, and a scorer can check the arithmetic instead of trusting a label.

The oracles are rung 4's, unchanged and already flown reading the negative side (rung-5 brief §2.6):
`post_lockdown` (falcon `hwcfg2` bit 13), `v1_readable` (the legacy `gsp_falcon_cpuctl_v1` mirror),
and `br_retcode` — **which is not an oracle for either arm.** A correctly encoded but unsigned payload
returns the same `0x00000002` (§2.5 of that brief). What a `=5` boot can settle is narrower and real:
whether the registers take the shifted values at all, and whether the two execution witnesses move.

`docs/dev/evidence/orin27/scorer-ga10b4.sh` gains a `4e` and a `4f` leg. They score the same oracles,
key on the tag, and in 4e's case recompute `wrote == raw_pa >> 8` for every address write from the
capture itself. Both legs are RED — never NO VERDICT — on a capture without the tag, which is how the
two `=2` wires already in `evidence/orin27/` score: a leg that answered NO VERDICT on the wires we
have could never come out red on a wire we fly. Two wires each, run from files by
`scorer-ga10b4.sh --selftest`: the expected `4e` wire scores 0 and the expected `4f` wire scores 1 on
the `4e` leg, and the other way round; the untagged flown wire scores 1 on both; a `4e` wire with one
written value off by one scores 1 (the arithmetic, not the tag); a `4f` wire with `bcr_ctrl` back at
`0x00000111` scores 1; and the flown `4a` and `4b` legs still score 0 on the new wires.

### 12.3b The wire each boot should show

Rung 4e (`=5`), on a board that seats the window at `0x80200000` — every other line is the flown
`=2` wire, unchanged and in the same order:

```
[ga10bprobe4e] rung 4e ARMED (UNAOS_GA10B_PROBE4=5) — … shift=8, i.e. pa >> 8 …
[ga10bprobe4a] about-to-WRITE priscv_bcr_fmccode_lo  reg=0x17111678 val=0x00802000 raw_pa=0x80200000 shift=8 (BCR DMA address, A-step) — …
[ga10bprobe4a] priscv_bcr_fmccode_lo  @0x678 wrote=0x00802000 read=0x00802000 raw_pa=0x80200000 shift=8 held=1
[ga10bprobe4a] about-to-WRITE priscv_bcr_fmccode_hi  reg=0x1711167c val=0x00000000 raw_pa=0x80200000 shift=8 (BCR DMA address, A-step) — …
[ga10bprobe4a] priscv_bcr_fmccode_hi  @0x67c wrote=0x00000000 read=0x00000000 raw_pa=0x80200000 shift=8 held=1
        … fmcdata_lo val=0x00802800 raw_pa=0x80280000 · fmcdata_hi val=0x00000000 raw_pa=0x80280000 …
        … pkcparam_lo val=0x00803000 raw_pa=0x80300000 · pkcparam_hi val=0x00000000 raw_pa=0x80300000 …
[ga10bprobe4a] about-to-WRITE priscv_bcr_dmacfg reg=0x1711166c val=0x00000002 (…)      ← no raw_pa: not an address
[ga10bprobe4a] … the seven restore writes to 0x00000000, unchanged …                   ← no raw_pa: no address behind them
[ga10bprobe4a] bcrheld=7/7 dmabuf_pa=0x80200000 shift=8 lock_after=0 unreadable=0 -> BCR-ALLHELD
[ga10bprobe4b] … the same six addresses re-written, raw_pa= and shift=8, "BCR DMA address, B0 re-write" …
[ga10bprobe4b] priscv_bcr_ctrl @0x668 wrote=0x00000111 read=0x00000111 held=1          ← 4e does NOT touch bcr_ctrl
[ga10bprobe4b] br_retcode=0x######## br_result=0x# samples=<i>/16 lock_latched=1 post_lockdown=<0|1> v1_readable=<0|1> shift=8 -> BROM-VERDICT-FAIL | BROM-VERDICT-PASS | BROM-NOVERDICT | BCR-CTRL-REFUSED | IGNITION-SKIPPED
[ga10bprobe4b] flight done — powering OFF …      [pwrshutoff] PSCI SYSTEM_OFF (0x84000008) via SMC
```

Rung 4f (`=6`) is the flown `=2` wire with four lines different and nothing else:

```
[ga10bprobe4f] rung 4f ARMED (UNAOS_GA10B_PROBE4=6) — … brfetch=false …
[ga10bprobe4a] bcrheld=7/7 dmabuf_pa=0x80200000 brfetch=false lock_after=0 unreadable=0 -> BCR-ALLHELD
[ga10bprobe4b] about-to-WRITE priscv_bcr_ctrl reg=0x17111668 val=0x00000011 brfetch=false (RUNG 4f: the ACKED SEQ's ALTERNATE set_bcr value — BRFETCH FALSE, CORE_SELECT RISCV, VALID TRUE. The flown arm wrote 0x00000111, BRFETCH TRUE; baseline was 0x00000110) — …
[ga10bprobe4b] priscv_bcr_ctrl @0x668 wrote=0x00000011 read=0x00000011 held=1
[ga10bprobe4b] br_retcode=0x######## br_result=0x# samples=<i>/16 lock_latched=1 post_lockdown=<0|1> v1_readable=<0|1> brfetch=false -> BROM-VERDICT-FAIL | …
```

### 12.4 Fail shapes, named ahead (R19: each is "failed under \<conditions\>", knob and code KEPT)

| # | shape | reading |
|---|---|---|
| F27 | 4e: an address write does not read back — `held=0`, the write list STOPS, `-> BCR-SOMEHELD`, no ignition | New information about the registers, not a broken arm: a shifted value is SMALLER than the raw one the same register already accepted, so a refusal here would say the field is not what §1.1 says it is. Report; do not retry warm |
| F28 | 4e: `br_retcode=0x00000002 -> BROM-VERDICT-FAIL` again, with `post_lockdown=1` and `v1_readable=0` | **The expected outcome, and a RESULT rather than a failure.** The encoding was not the blocker: cause 5 of rung-5 brief §2.3 is retired and causes 1–4 stand undisturbed |
| F29 | 4e: `-> BROM-NOVERDICT` with post-ignition `halted=0` | The core is RUNNING — the first non-FAIL on this die. STOP and report; build nothing on it in the same session, and re-fly cold before it is believed (the 4b `BROM-VERDICT-PASS` discipline, F8) |
| F30 | 4f: `-> BCR-CTRL-REFUSED read=0x########` | The register did not take BRFETCH FALSE. The ignition write is NOT issued and the flight ends in `SYSTEM_OFF` — a complete answer about the register, and the end of Q5 |
| F31 | 4f: the ignition returns the same `0x00000002` | BRFETCH is not the door. Q5 is answered negatively and the arm is spent |
| F32 | either arm: a GPU-side fabric RAS after the ignition | §5's F2, restated because these arms issue the SAME ignition write. May need a manual power cut; fly attended |
| F33 | either arm: the capture carries no tag | The image was not the arm you think it was. Certify the artifact before the flight, not the diff: `LC_ALL=C grep -a -o -F 'shift=8' target/aarch64_esp/kernel.elf` and its `brfetch=false` sibling, and the `[ga10bprobe4e]`/`[ga10bprobe4f]` families |

### 12.5 What rungs 4e and 4f do NOT attempt

- **No vendor blob and no real FMC image.** The payload is still rung 4a's fill pattern `0x4a10b4a5`;
  nothing here touches Q1 of the rung-5 brief, and neither arm needs it answered.
- **No new register, address class or write class.** `bcr_ctrl` and the six DMA addresses are already
  4b's; the only thing that differs is the value put in them.
- **No rung 4c census**, so `POSTBCR`, `MAPDIFF` and `DMABUF` do not appear on a `=5`/`=6` wire. The
  `4e`/`4f` scorer legs report those oracles always and require them only where a capture carries 4c.
- **No `MAILBOX0`/`MAILBOX1` boot-parameters channel** (rung-5 brief §1.4). That is a second, separate
  unverified Hopper behaviour and putting it on the same boot would confound this one.
- **Nothing about signatures.** `br_retcode` cannot distinguish an unsigned payload from a
  misencoded one; §2.5 applies to these arms exactly as it applies to 4b.
- **No alignment rule.** The shift is lossless here because the window happens to be 2 MiB aligned.
  Rung 5 would have to install a real 256-byte alignment contract for an image it does not choose.

### 12.6 The knob lines, one per boot

```
UNAOS_TEGRA=1 UNAOS_GA10B_PROBE4=5 ./arroyo esp-jetson     # rung 4e, the SHIFT arm
UNAOS_TEGRA=1 UNAOS_GA10B_PROBE4=6 ./arroyo esp-jetson     # rung 4f, the BRFETCH arm
```

Each is one attended cold boot ending with the board dark. They are the `=2` line with one digit
changed, which is the point: everything the flight does differently is in the one value the arm names.

### 12.7 Verification posture of this section

Per §9 and §10.8. What this arc changes: one `.rs` file (twelve lines rewritten in
place, no hunk changing a line count, plus an 86-line block appended at the END of the file), two feature declarations, the knob's
value parse, two `KERNEL_CFG_MATRIX` legs, the scorer, this section and one ledger row. The assertions the kernel batteries make are
about compiled images: `./arroyo check` type-checks both arches with the ledger and k8reach gates
armed; the two armed polarities are type-checked by the new `arm-tegra-ga10bprobe4e` and
`arm-tegra-ga10bprobe4f` legs — which are not optional, since `check`'s knob→leg coverage gate reds
on a declared feature with aarch64-qualified `#[cfg]` sites that no leg names — and compiled again by
the `=5` and `=6` `esp-jetson` builds, with their witnesses proven present in the artifact with `LC_ALL=C grep -a -o -F` (and proven ABSENT from the
`=2` artifact, which is the control); `./arroyo knoboff ga10bprobe4a` measures the default image;
and the `=2` and `=4` loadable images are measured byte-identical across the change by knoboff's own
method — one directory, one pinned `UNAOS_GIT_SHA`, `llvm-objcopy -O binary`, baseline reached by
`git apply -R` of the snapshotted diff. No QEMU models the Jetson: a green here certifies that it
compiles and links. Exit codes are in the commit and in ledger A62.
