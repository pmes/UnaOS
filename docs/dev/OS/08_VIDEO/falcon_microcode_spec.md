# Falcon Microcode Specification — NVIDIA Kepler (GK107) PGRAPH

> [!WARNING]
> **CLEANROOM POLICY NOTICE**
> NO proprietary firmware blobs may enter the UnaOS source tree. Extracting and distributing the NVIDIA microcode from macOS binaries is strictly prohibited. This document specifies only the neutral hardware interfaces and behavioral requirements of the Falcon microcontroller. The target is a 100% from-scratch, open-source reimplementation of the initialization firmware. Instruction encodings are cited from envytools Falcon ISA documentation only.

## 0. How to read this document

Every factual claim below is tagged with the sitting that established it
(`s29` = sitting #29 in [`KEPLER-METAL-LOG.md`](KEPLER-METAL-LOG.md)) or with
the code that implements it. **A claim with no citation is not in this
document.** That is the whole point: each line here cost a boot on the rMBP
bench, and the two most expensive errors of the campaign were both cases of
someone deriving a Falcon IO port from memory instead of from §3.

Claims still awaiting metal are labelled **DERIVED (untested)**. Do not
promote them without a sitting.

## 1. The Falcon microcontroller

NVIDIA GPUs from the Fermi/Kepler era use custom 32-bit microcontrollers
called **Falcon** (FAst Logic CONtroller) to manage complex engines. The
`PGRAPH` block (2D, 3D, Compute) is managed by two of them: the context-switch
Falcons **FECS** and **GPCCS**.

At GPU boot `PGRAPH` is in a reset state and refuses to process PFIFO
pushbuffer commands until its Falcon boots, initializes the internal pipeline
state, and signals ready.

### Unit bases (metal-proven)

| Falcon | Unit base | Evidence |
| --- | --- | --- |
| **FECS** | `0x409000` | s26 — `cpuctl=00000010`, `imemc/dmemc=00000000` true zeros; first non-poison Falcon reads of the campaign |
| **GPCCS** | `0x41A000` | s26 — identical verdict line at this base |

Both bases accept the full IMEM/DMEM sentinel probe (s27: all sixteen sentinel
words returned exactly, IMEM **and** DMEM, FECS **and** GPCCS). All microcode
execution to date is **FECS only** — GPCCS is proven addressable but has never
been given a program (pull 25 discipline: FECS first, GPCCS only after FECS
behaves).

Prerequisite: `NV_PMC_ENABLE` bit 12 (PGRAPH) must be set before any of these
registers respond. The enable is accepted on this part
(`pre=E011216D → rb=E011316D`, s22/s23).

## 2. The register file, from unit base

Offsets are relative to the unit base (§1). "Rest value" is what the register
read **before UnaOS wrote anything** in that boot.

| Offset | Name | Rest value | Evidence |
| --- | --- | --- | --- |
| `+0x040` | `MAILBOX0` | (seeded, see §4) | s29 — the `ucode-post off=040 val=F00DFACE SENTINEL` sweep line places MAILBOX0 here |
| `+0x044` | `MAILBOX1` | (seeded, see §4) | s30 — heartbeat counter observed monotonic here |
| `+0x100` | `CPUCTL` | `0x00000010` | s26, s31, s34 — stable across boots |
| `+0x104` | `BOOTVEC` | — | s27, s29 — written 0, honoured |
| `+0x108` | `IDLESTATE` | `0x20402050` | s28 post-sweep |
| `+0x10C` | `DMACTL` | `0x00000001` | s28 — **REQUIRE_CTX is SET at rest**; see §4 |
| `+0x12C` | (unnamed) | `0x00081103` | s28 post-sweep — recorded, not decoded |
| `+0x140` | `TLB_CMD` | — | s28/s32 — write `0x02000000` to query virtual page 0 |
| `+0x144` | `TLB_DATA` | `0x01000000` after page-pad = **usable** | s28, s32 |
| `+0x180` | `IMEMC` | `0x00000000` (rest, s26) / `0x02000014` (post-upload, s28) | s26, s28 |
| `+0x184` | `IMEMD` | — | s27 sentinel probe |
| `+0x188` | `IMEMT` | — | code — tag write, must match `BOOTVEC` page |
| `+0x1C0` | `DMEMC` | `0x00000000` (rest, s26) / `0x02000010` (post-upload, s28) | s26, s28 |
| `+0x1C4` | `DMEMD` | — | s27 sentinel probe |
| `+0x504` | `WRCMD_CMD` | ⛔ **FAULTS — poisons the unit** | s31, s32, s34 — see §5.4 |
| `+0x800` | `CC_SCRATCH[0]` | `0x00000000` | s33boot1 — read first in the rotation, real zero |
| `+0x804` | `CC_SCRATCH[1]` | `0x00000000` | s34 |
| `+0xB00` | `CHAN_CUR` | `0x00000000`, host-writable | s34 (rest), s35 (write took) |
| `+0xB04` | `CHAN_NEXT` | `0x00000000`, host-writable | s34 (rest), s35 (write took) |
| `+0xC00` | `ENGINE_STATUS` | `0x00000000` | s34 (rest), s35 (CHAN_VALID not host-assertable) |
| `+0xC08` | `ENGINE_TRIGGER` | `0x00000000` | s34 |

### 2.1 CPUCTL bit meanings

Read as rnndb documents them, corroborated on metal at s28:

- **bit 1 — `START_TRIGGER`**: write to start the core.
- **bit 4 — `STOPPED`**: set when the core is halted. `0x10` at rest therefore
  means *halted*, and `0x00` means *running* (s30: `cpuctl=00000000` for the
  whole heartbeat run).
- **bit 6** is **clear** on this part (s28). That is what proves writing
  `CPUCTL` at `+0x100` is correct here, rather than the GM107+ alias at
  `+0x130`.

Observed transition on a refused start (s28): `0x00000010 → 0x00000012` — the
start trigger latched (bit 1) while the core stayed stopped (bit 4). That
signature means "trigger accepted, core refused", and its cause was `DMACTL`
(§4 step 7).

## 3. ⭐ THE IO DERIVATION — read this before writing a single instruction

Falcon microcode does **not** reach the unit registers at their host MMIO
offset. It reaches them through Falcon IO space with `iowr`/`iord`, at an
index derived from the host offset:

```
falcon IO index  =  (host_register_offset & 0xffc) << 6
```

### 3.1 The proof (s29)

Five hand-assembled instructions were uploaded to FECS. `MAILBOX0` was seeded
with `0xA5A50000`. The program executed `iowrs I[0x1000], 0xF00DFACE` and the
host read back:

```
:: kepler: ucode end img=A cpuctl=00000010 mailbox0=F00DFACE halt-iters=0 ::
:: kepler: ucode EXECUTED img=A mailbox0=F00DFACE ::
:: kepler: ucode-post off=040 val=F00DFACE SENTINEL ::
```

The mailbox held the **exact authored magic**, not merely a changed value —
so the write landed at the intended register, through the intended port, and
`(0x040 & 0xffc) << 6 = 0x1000` is the correct mapping. This is the strongest
form of evidence available: a value we chose, arriving where we predicted.

Independent second instance: the pull 27 heartbeat ucode targets `MAILBOX1`
via `I[0x1100]` (`UCODE_HB[0] = 0x110017f1` = `mov $r1, 0x1100`) and the host
observed the counter advancing at `+0x044` — `0x4 → 0x5750 → 0x5AA5 → 0x34328`
(s30), and terminating at its exact authored bound `0x00500000` (s33boot2).
Two registers, two boots, one rule.

### 3.2 Worked table

| Host offset | Register | `& 0xffc` | `<< 6` | Falcon index | Status |
| --- | --- | --- | --- | --- | --- |
| `0x040` | `MAILBOX0` | `0x040` | | **`0x1000`** | ✅ PROVEN s29 |
| `0x044` | `MAILBOX1` | `0x044` | | **`0x1100`** | ✅ PROVEN s30, s33boot2 |
| `0x504` | `WRCMD_CMD` | `0x504` | | `0x14100` | DERIVED (untested — and the host side of this register faults, §5.4) |
| `0x800` | `CC_SCRATCH[0]` | `0x800` | | `0x20000` | DERIVED (untested — pull 33 aboard, awaiting s37) |
| `0x804` | `CC_SCRATCH[1]` | `0x804` | | `0x20100` | DERIVED (untested — pull 33 aboard, awaiting s37) |

### 3.3 The two errors this section exists to prevent

This derivation has been got wrong twice, at real cost. Both are recorded so
the third attempt does not happen.

1. **Pull 25's binding amendment asserted `0x40`.** The proposal originally
   derived the port as `0x040 / 4 = 0x10`; the coordinator amendment overrode
   that to "use **0x40**, not 0x10", on the reasoning that Falcon IO space is
   "a BYTE offset matching the host MMIO offset". Both the proposal's `/4` and
   the amendment's flat `0x40` are wrong — the mapping is `<< 6`
   (`PROPOSAL-kepler-fence-pull25.md`, refuted by s29). The A/B fallback is
   what rescued the boot: image A carried the indexed port and ran, so image B
   (flat `I[0x0040]`) **never executed and has never been tested on metal**.
   We know the indexed scheme is right; we do not know what `I[0x0040]`
   decodes to, and this document will not guess.
2. **Pull 33's listing used raw host offsets `0x800`/`0x804` as Falcon IO port
   indices.** Caught at proposal review, before it reached the bench, and
   corrected to `I[0x20000]`/`I[0x20100]` (commit `0bb305a9`). A side effect
   worth knowing: the corrected indices **do not fit the I16 immediate form**
   the original listing used, so the `mov`s had to be re-encoded (the landed
   image A uses a `mov`/`sethi` pair — `f0 17 00` then `f0 13 02`,
   `sethi $r1, 0x20000`). Getting the port right can change the instruction
   encoding, not just an immediate byte.

**Rule for authors:** derive every port with
`regs::falcon_io()` in `unaos/crates/kernel/src/drivers/gpu/kepler.rs`, which
carries compile-time assertions for both proven mappings. Never hand-write a
port immediate, and always print the port you used in the boot marker
(`:: kepler: ucode img=A ioport=1000 want=F00DFACE ::`) so the capture is
self-documenting.

## 4. The upload and execute ritual

This is the sequence that works, in order. It is implemented in
`kepler.rs::init()`; deviations have all cost boots.

1. **Seed the target mailbox** with a known value (`MB_SEED = 0xA5A5_0000`)
   before anything else, so "unchanged" has exactly one meaning and a
   coincidental non-zero cannot be mistaken for success (s29 discipline).
2. **`IMEMC ← 1 << 24`** — offset 0, **AINCW** (auto-increment on *writes*).
3. **`IMEMT ← 0`** — the code page tag, matching `BOOTVEC = 0`.
4. **Write the program words to `IMEMD`**, then **pad to a full `0x40`-word
   page** with zeros. This is not optional: the code TLB marks a page usable
   only when the last word of the page has been written. Nouveau pads for the
   same reason; the page-pad was added at s28 land-review and the TLB
   attestation went usable immediately (s28).
5. **Attest the page** — `TLB_CMD ← 0x02000000`, read `TLB_DATA`. `0x01000000`
   means page 0 is usable (s28, s32). A ucode that will not run is very often
   a ucode whose page was never marked usable.
6. **`IMEMC ← 1 << 25`** — offset 0, **AINCR** — then read back through
   `IMEMD` and compare against the image. **Abort the launch on any
   mismatch**, without writing `BOOTVEC` or `CPUCTL`. This gate is load-
   bearing: at s31 the unit was poisoned mid-boot, readback returned
   `BADF1000`, and both ucode A and the heartbeat aborted cleanly rather than
   starting a core blind against garbage IMEM.
7. **Clear `DMACTL` bit 0 (`REQUIRE_CTX`)** — read `+0x10C`, write back
   `& !1`, print both. **This was the entire block at s28/s29.** At rest
   `DMACTL = 0x00000001`: the Falcon refuses to run until a context is bound.
   Clearing bit 0 let the core run on the first attempt (s29). Nouveau clears
   exactly this bit in its no-context Falcon path. If the post-read still has
   bit 0 set, do not start the core.
8. **`BOOTVEC ← 0`**, then **`CPUCTL ← 2`** (`START_TRIGGER`).
9. **Bounded poll for `CPUCTL` bit 4 (`STOPPED`)**, then read the mailbox.

### 4.1 The bit-24 / bit-25 trap

`IMEMC`/`DMEMC` carry **two different auto-increment bits**:

- **bit 24 — AINCW**: auto-increments on **writes only**.
- **bit 25 — AINCR**: auto-increments on **reads only**.

Setting bit 24 and then reading back through the data port reads the *same
word* repeatedly. This cost a fix at s24 (`+AINCR fix` in that sitting's
commit reference); with the discipline correct, s27 returned all sixteen
sentinels exactly and recorded "AINCW(24)/AINCR(25) discipline works as
specced". Always re-write the control register with the *other* bit before
switching direction.

### 4.2 What is *not* evidence

`halt-iters` is uninformative by design (s29): `0` means the poll proved
nothing. The proof of execution is **the exact authored magic in the
mailbox**, never the poll, never a merely-changed value, and never a readback
of plain VRAM (see §6, refutation 8).

## 5. Laws for microcode authors

### 5.1 Bound every loop

Pull 27 established this and stated the reason plainly: *"Bound it: a finite
iteration count, then `exit`. Do NOT write an unbounded loop — a Falcon
spinning forever through the rest of boot is"* a hazard to every subsequent
leg of that boot. The bounded heartbeat behaved exactly as authored: it ran
across the entire witness sequence (s30) and, given enough wall-clock,
terminated at precisely its authored bound `0x00500000` and parked with a
clean `cpuctl=00000010` (s33boot2).

**Pull 33 violated this law** — its command-echo loop shipped unbounded, and
that was raised as a land-review flag (`bc5fe3fc`): "the echo loop is
UNBOUNDED (pull 27 discipline was a bounded loop) — pull 34 must add a
host-commandable exit". A command loop needs a host-commandable exit *and* a
fallback bound; "the host will tell it to stop" is not a bound.

### 5.2 One variable per boot, with an A/B fallback

Ship two images with distinct magics when a single binary question is open
(pull 25). Run A, fall back to B only if A produces nothing, and label the
attempt in the marker. One boot then settles the question regardless of which
derivation was right. This is what converted the pull 25 port dispute into a
single decisive sitting (s29) instead of a two-sitting bisect.

### 5.3 Verify before you launch

See §4 step 6. Never write `BOOTVEC`/`CPUCTL` on an unverified image.

### 5.4 ⚠ The poison law — one bad offset kills the unit for the boot

**The first access to `0x409504` (`WRCMD_CMD`) faults immediately and wedges
every subsequent read in the FECS unit for the rest of the boot.**

- s31 discovered it: `recon WRCMD_CMD=BADF1000`, and every following
  `0x409xxx` read — mailboxes, `CPUCTL`, IMEM readback — returned `BADF1000`.
- s32 confirmed it with its own control frame: `recon-pre cpuctl=00000000`
  (real) and `recon-post cpuctl=BADF1000` on the *same register*, microseconds
  apart. Immediate, not cumulative.
- s34 convicted it by elimination: the other six gf100-era offsets
  (`0x800/0x804/0xb00/0xb04/0xc00/0xc08`) all exist and read zero at rest.
  `0x409504` is the only offset ever observed to fault when accessed first.

Consequences for anyone probing this unit:

- PFIFO (`0x2xxx`) is **unaffected** by the poison (s31) — witness signatures
  stay trustworthy across it.
- Put unproven offsets **last** in a boot, after every proven read has
  completed, and bracket the block with a control read of a known-good
  register at both ends (the s31→s32 fix).
- Only the **first** recon datum in a poisoned boot is clean; everything after
  the fault is confounded, not proven absent. This retroactively colors the
  s24/s25 "all `BADF1000`" sweeps — those may equally have been first-fault
  poison rather than per-offset truth (s31).
- `0xBADF1000` is the nonexistent-PRI-register signature, not a gate (s25).

Note the surprise value of this fact: gf100 ctxctl documentation places the
FECS host-interface registers exactly here, and nouveau drives `0x409504` on
gk104 — so a faulting `0x409504` on GK107 is itself a load-bearing
observation, not merely an obstacle (s32).

## 6. REFUTED — do not re-propose

Each of these was a live hypothesis that metal killed. They are listed so that
no future pull spends a boot re-testing them.

1. **PGRAPH Falcon at base `0x400000`** (`IMEMC 0x400180`, `DMEMC 0x4001C0`,
   `CPUCTL 0x400100`). Every access returned `BADF1000` (s24). The real bases
   are FECS `0x409000` / GPCCS `0x41A000` (s25 inference, s26 proof). This
   document's earlier revision specified `0x400000`; that section is dead.
2. **"The PMC enable alone leaves the Falcon behind a second gate"** (s24's
   reading). Superseded: the ports were not gated, they were at the wrong base
   (s26). See also refutation 5.
3. **`0x040 / 4 = 0x10` as the IO port** (pull 25 proposal). Refuted by s29.
4. **Flat IO ports — host offset used directly as the Falcon index**
   (pull 25 binding amendment; pull 33 listing). The indexed scheme
   `(X & 0xffc) << 6` is correct (s29). See §3.3.
5. **CTXCTL subunit gating** — the theory that the `0x409xxx` space was
   disabled wholesale. Refuted both ways at s33boot1: the enable bit is
   already SET (`PIBUS_MMIO_HUB_ENABLE1=FFF9F4B0`, bit 4 set), and
   `CC_SCRATCH[0]` read *first* returns a real zero rather than `BADF`. The
   poison is per-offset, not a space-wide gate.
6. **Engine liveness as the cause of the PFIFO fence wall** — refutation #8,
   the cleanest. The heartbeat ucode kept FECS demonstrably executing
   (`cpuctl=00000000` throughout) across the whole witness sequence, and PFIFO
   stripped the channel anyway, with a byte-identical signature
   (`err=00000002 stat=00000005 valid=00002000`) (s30). Confirmed from the
   other side at s33boot2: a **halted** FECS produces the identical signature.
7. **Binding a context by poking CTXCTL registers from the host.**
   `CHAN_CUR`/`CHAN_NEXT` are host-writable and hold a channel id — the writes
   take, no fault, no poison (s35) — but `ENGINE_STATUS` stays `0`:
   **CHAN_VALID is not asserted**, and the PFIFO strip is unchanged with both
   populated (s35, ninth confirmation; s36, tenth, with a host-populated
   CHAN_CUR/CHAN_NEXT). CTXCTL state is not built by poking its registers;
   something must *run* to accept a context.
8. **Reading back plain VRAM as a witness.** The s35 amendment aimed the
   post-bind witness at `inst_off+0x0C` — an instance-block word in ordinary
   VRAM, where a readback trivially returns what was written. That leg
   observed nothing and its result line is void. The historic strip lives in
   the PFIFO channel-table **register** (`0x800008`). Recorded as a coordinator
   amendment error, alongside the pull 25 port amendment: **amendments must be
   derived against the code, not from memory of it** (s35).

## 7. What the microcode must still do

The fence arc's verdict after ten strip eliminations (s36): **the wall is the
absent FECS context machinery, and nothing else.** Every constructive fact
points the same way — the submit path works (`PLAYLIST_RD` echoes our
runlist), the Falcon executes our code (§3), the CTXCTL register surface is
mapped and writable (§2) — and `ENGINE_STATUS.CHAN_VALID`, the bit PFIFO's
validation plausibly keys on, is set by nothing reachable from the host.

So the open-source firmware must implement:

1. **Pipeline initialization** — write the internal, undocumented `PGRAPH`
   state registers that clear the invalid state left by hardware reset.
2. **Context acceptance and switching (CTXPROG)** — accept a context so
   `CHAN_VALID` asserts (the s35/s36 finding), and on channel switch save the
   3D pipeline state of the outgoing channel to VRAM and restore the
   incoming one. `DMACTL.REQUIRE_CTX` being set at rest (§2) was the hint all
   along: the chip wants a context, not a heartbeat (s30).
3. **Interrupt handling** — trap illegal 3D commands, page faults, and
   synchronization fences, reporting to the host through the `NV_PMC_INTR`
   master interrupt tree.

Milestone status: M1 upload path proven (s27); M2 first UnaOS code executed on
GPU silicon (s29); M5 first FECS command-loop ucode aboard and awaiting metal
(pull 33, s37). The next era is authoring the minimal FECS ucode that brings
up the context machinery (STUDY-fecs-ctx-init phases).

## 8. Development toolchain

An open-source toolchain exists for the Falcon ISA, from the Nouveau project:

- **Envytools (`envyas`)**: assembles Falcon assembly (`.fuc`) into the binary
  format IMEM expects.
- **Instruction set**: 32-bit, 16 general-purpose registers (`$r0`–`$r15`), a
  stack pointer (`$sp`), a program counter (`$pc`), standard ALU operations
  and branching, and the `iowr`/`iord` pair for the IO space of §3.

Encodings currently in use in `kepler.rs`, cited from
`docs/hw/falcon/{arith,io,proc}.rst` (envytools Falcon ISA v4):

```
f1 17 <lo> <hi>   mov   $r1, PORT      I16 immediate
f1 27 ce fa       mov   $r2, 0xface    I16, sign-extended
f1 23 0d f0       sethi $r2, 0xf00d    replaces the high half
d1 12 00          iowrs I[$r1], $r2    synchronous IO write
f8 02             exit
f0 13 02          sethi $r1, 0x20000   I16 form, sets high 16 bits (pull 33)
```

Note that Falcon instructions are variable-length byte sequences. The images
in `kepler.rs` are `[u32]` arrays of the **packed byte stream**, so
instructions straddle word boundaries — do not read a word as an instruction.
