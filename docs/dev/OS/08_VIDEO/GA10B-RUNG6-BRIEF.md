# GA10B-RUNG6-BRIEF — after the ROM accepts the ACR: WPR, the PMU, GR, and a channel, with no GSP-RM

Status: DESIGN + rung 5b built (`UNAOS_GA10B_PROBE5=2`, read-only oracles). Written from PUBLIC
sources (this section, §0). Companion to GA10B-RUNG4-BRIEF (rungs 4a–4f) and GA10B-RUNG5-BRIEF
(rung 5a, the vendor ignition, and §5.5's split of the old "rung 5" into 5a and 5b). Ledger A65.

## 0. Provenance and clean-room posture (read first)

Every mechanism here is from one of three PUBLIC classes; nothing is read from an ACKED-secret file
and nothing is copied from a GPL-2.0-only tree.

- **nvgpu (the L4T kernel driver, GPL-2.0)** is read FOR FACTS ONLY — the ORDER of a bootstrap, the
  NAME of a register, the SHAPE of a mailbox handshake — and no line is copied. It is the
  authoritative account of how the Tegra part is actually driven, and it is the one that establishes
  the load-bearing negative in §1.
- **open-gpu-kernel-modules (NVIDIA, MIT)** and **nouveau (MIT/GPL-2.0 dual where marked)** headers
  are QUOTABLE for register offsets and field names; where an offset here is recalled from them and
  is NOT in the ACKED facts file it is marked PUBLIC-RECALLED, exactly as rung 3/4/5 mark theirs.
- **The firmware bytes themselves are never read for their content** — they are opaque input, staged
  under R52 and loaded as data (GA10B-RUNG5-BRIEF §4.2, §5.5). This brief names files, sizes and
  roles; it decrypts, disassembles and inspects nothing.

Public references consulted for the sequence and the no-GSP-RM finding: the OE4T/linux-nvgpu tree
(rel-36, `common/acr`, `common/pmu`, `common/gr`), the NvGPU device-tree docs, the
open-gpu-kernel-modules falcon headers, and the OE4T meta-tegra issue threads on ACR/LSF bootstrap
failures. Cited, not copied.

## 1. The load-bearing fact: GA10B on Tegra has no GSP-RM

A discrete Ampere/Ada board boots its GSP into GSP-RM, and the CPU driver then talks to RM over an
RPC channel. **The Tegra part does not.** nvgpu on GA10B drives the engines directly from the CCPLEX
the way it drove every Tegra GPU before it: there is no GSP-RM image in the L4T firmware set (the 17
`ga10b` files, GA10B-RUNG5-BRIEF §4.1.1, contain an ACR/GSP-FMC boot triple, a PMU ucode + desc +
sig, FECS/GPCCS ucodes + sigs, and a second "safety-scheduler" triple — no `gsp-rm` image), and
nvgpu's GA10B support path is the legacy ACR→PMU→GR path, not the GSP-RM RPC path. **This is why a
blob-free engine was never possible (GA10B-RUNG5-BRIEF §3.3) and equally why, once the ROM accepts
the ACR, the rest is a documented CCPLEX-driven sequence rather than an opaque RPC.** Measured
against public sources and stated as the null hypothesis this brief rests on; a metal flight that
contradicts it is the finding.

## 2. The sequence after ACR-ACCEPTED (nvgpu's order, named not copied)

Each rung is one boot's worth of work; each names the rungs below it that must be open first.

1. **WPR is carved and the ACR runs (rung 5a's result).** The FMC the boot ROM launched sets up the
   Write-Protected Region and is the thing that will bootstrap the LS falcons. Rung 5a's
   `ACR-ACCEPTED` (lockdown dropped OR the v1 mirror readable) is the precondition; rung 5b reads
   whether the ACR then moved the PMU.
2. **The PMU is booted (rung 6a).** `gpmu_ucode_next_prod_image.bin` + `gpmu_ucode_next_prod_desc.bin`
   + `pmu_pkc_prod_sig.bin` are handed to the ACR/PMU bootstrap; the PMU falcon comes out of reset
   and runs its RTOS. The CPU↔PMU channel is the falcon MAILBOX/DMEM queue protocol (§3).
3. **GR's context-switch ucode is loaded (rung 6b).** `fecs_encrypt_prod.bin` + `fecs_pkc_sig_encrypt.bin`
   and `gpccs_encrypt_prod.bin` + `gpccs_pkc_sig_encrypt.bin` are bootstrapped (LS falcons under the
   ACR), then FECS/GPCCS are started and their mailboxes report ready.
4. **The golden context is built (rung 6c).** FECS builds the golden context image once; every
   channel's context is a copy of it. This needs GR fully powered and the ucodes running.
5. **A channel is set up (rung 6d = the ladder's old "rung 5" / GA10B-RUNG5-BRIEF §5.5's rung 5b
   FIFO question).** PBDMA, runlist, RAMFC, USERD, a GPU MMU page table, and then one submitted
   copy-engine job. This is the first rung that puts work on an engine.

**What is NOT here:** no display path (this SoC's display is not in the GPU — GA10B-RUNG4-BRIEF §3.1);
no GSP-RM RPC (§1); no second triple (`safety-scheduler.*` is named and tried by nobody, R19).

## 3. Rung 5b — the ACR→PMU handshake, READ-ONLY (built, `UNAOS_GA10B_PROBE5=2`)

Rung 5b is the first, cheapest, zero-write probe of step 2: after rung 5a's `ACR-ACCEPTED`, did the
ACR actually move the PMU? It is built (`ga10b_ignite.rs::rung5b`, feature `ga10bprobe5b`, implies
`ga10bprobe5a`) and adds ZERO write classes.

- **Precondition:** rung 5a's verdict is `ACR-ACCEPTED`. On any other verdict rung 5b prints
  `-> PMU-NOT-ATTEMPTED reason=not-accepted` and reads nothing — there is no handshake to observe.
- **What it samples**, each announced before the read (`about-to-read handshake <name>`), over a
  bounded series (10 samples, 20 ms apart):
  - the GSP falcon MAILBOX0/MAILBOX1 (the FMC's status/error channel);
  - the PMU legacy-falcon block at base `0x10a000` (PUBLIC-RECALLED, open-gpu-kernel-modules
    `dev_pwr_pri.h`; NOT in the ACKED facts file) — `cpuctl`, `hwcfg2`, `mailbox0`, `mailbox1`;
  - the PMU falcon2 `cpuctl` at `0x10b388`, which rung 3 already read on this die.
- **Verdict vocabulary:** `PMU-RUNNING` (either PMU cpuctl aperture reads not-halted) | `PMU-HALTED`
  | `PMU-UNREADABLE` (both apertures fault) | `PMU-NOT-ATTEMPTED reason=<not-accepted>`. It prints a
  per-register change/unreadable count over the series so a value that MOVES (the FMC writing a
  status word) is visible even when cpuctl stays halted.
- **Why read-only first:** the DMEM/mailbox WRITE protocol that would command the PMU (step 2's
  active half) needs the PMU already running; probing whether it is running costs nothing and
  bounds the next rung. This is the same "read-only oracle before the write rung" discipline rungs
  3→3b and 4a→4b used.

## 4. Per-rung bounding table (GA10B-RUNG4 §4 form — the sibling each witness must exclude)

| rung | new witness | sibling it must exclude | the bound that HOLDS |
|---|---|---|---|
| 5b | `[ga10bprobe5b] … -> PMU-RUNNING` | rung 5a's `ACR-ACCEPTED` (a different falcon), and a stale rung-3 PMU read | key on `[ga10bprobe5b]` + `PMU-RUNNING`; the PMU cpuctl not-halted bit, TWO apertures agreeing or one moving |
| 6a | PMU RTOS alive — a PMU mailbox INIT message | the ACR's own mailbox traffic, and rung 5b's read-only samples | the INIT message's documented tag in a WRITTEN exchange, not a passive read |
| 6b | FECS/GPCCS ready | the PMU's mailbox, and each other | per-falcon base + the ready code in its own mailbox |
| 6c | golden context built | a zeroed context image, and a partial build | FECS's golden-image-done status, and a non-zero context CRC |
| 6d | one CE job retired | a runlist that never ran, and a PBDMA error | the semaphore release the job writes, at the address the CPU chose |

## 5. Fail shapes, named ahead (R19: each is "failed under <conditions>", knob and code KEPT)

- **F31** `PMU-HALTED` after `ACR-ACCEPTED` — the ROM verified the ACR but the ACR did not boot the
  PMU (wrong PMU image/sig for this die, or the ACR needs an argument rung 5a did not pass, §5.2
  delta 3). A datum: record it; the next step is the boot-params buffer, not another image.
- **F32** `PMU-UNREADABLE` — both PMU apertures fault post-accept; the accept may be a measurement
  error (GA10B-RUNG5 F20) or the PMU block is gated. Re-fly 5a from cold before building on it.
- **F33** a mailbox value MOVES but cpuctl stays halted — the FMC wrote a status/error word and
  stopped. The single most informative failure: the error code names the next question.
- **F34** rung 6a's write exchange hangs the boot — the DMEM/mailbox protocol assumed here is wrong
  for GA10B. STOP; the write half is not designable further without a metal read of the PMU's queue
  head/tail registers, which rung 5b is built to capture first.

## 6. What each rung does NOT attempt

Rung 5b: no write of any kind. Rung 6a: no GR, no channel. Rung 6b: no golden context. Rung 6c: no
channel. Rung 6d: no display, no second triple, no GSP-RM. No rung decrypts, disassembles, inspects,
patches or renames a vendor byte (R52 "unmodified"); no vendor file enters git or is linked; the
LAWS §3 enforcer (no binary firmware under `unaos/`) stays green through every rung.

## 7. Verification posture of this document

The sequence in §2 is nvgpu's, read for facts and cited, copied nowhere. §1's no-GSP-RM claim is the
null hypothesis, measured against the firmware file list and nvgpu's GA10B path; a metal flight is
what can refute it. Rung 5b is BUILT and compiles ARMED (`arm-tegra-ga10bprobe5b`); rungs 6a–6d are
DESIGN only (A65 `open`), each blocked on the metal read the rung below it provides — R53's ladder,
one rung and the one after, stopping where the next step is a metal boot.
