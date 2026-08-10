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
| `+0x110` | `DMATRFBASE` | — | envytools falcon register map — **DERIVED**, never read at rest in a captured boot before Boot AS's sweep; the FENCE `dmatrf-row` line now prints it every boot |
| `+0x114` | `DMATRFMOFFS` | — | envytools falcon register map — **DERIVED**, same standing |
| `+0x118` | `DMATRFCMD` | `0x00000002` | Boot AS (2026-08-09) — read as `00000002` on a parked falcon. Bit 1 is `IDLE` in the envytools falcon map, so this is the **rest value of an idle DMA engine**, corroborated by the fact that the FENCE leg uploads IMEM by PIO (`IMEMC`/`IMEMD`) and never arms the DMA engine at all. See §11.6 — this register cost a round |
| `+0x11C` | `DMATRFFBOFFS` | — | envytools falcon register map — **DERIVED**, same standing |
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
| `0x800` | `CC_SCRATCH[0]` | `0x800` | | `0x20000` | **PROVEN s37** — pull 33 image A acked on the first poll (`host-ack CC_SCRATCH[1]=00000001 iters=0`) |
| `0x804` | `CC_SCRATCH[1]` | `0x804` | | `0x20100` | **PROVEN s37** — the ucode wrote its ack here; image B (flat ports) never ran |

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

> ⚠ **This verdict is now conditional, and the condition is being tested.** A
> competing root cause was raised after Boot AS: the driver feeds *BAR1
> offsets* to PFIFO as physical VRAM page numbers, and if BAR1 is a paged
> window on this part then every FIFO pointer the arc ever measured through was
> wrong — which would explain `err=0x2`, `PBDMA ACTIVE=0` and `gp_get=0` on its
> own. `PLAYLIST_RD` echoing our runlist proves a *register* echoed what we
> wrote, not that the address was right. The eliminations are not retracted;
> they are provisional until §11.6.1's `bar1-identity` probe reports
> `IDENTITY`. If it reports `PAGED`, re-read this section before acting on it.

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

## 9. Pull 34 (R3-AMEND) — the bounded echo with split observables

This section appends to §3, §5.1 and §8; nothing above it is superseded. It
records the retrofit of the s37-acked echo skeleton with the three things its
approval required, and it discharges the standing bound-every-loop defect.

Source of record: `unaos/crates/kernel/src/drivers/gpu/kepler.rs`, module
`ucode`. The **byte listing is authoritative** there — the `[u32]` images are
produced from it by `ucode::pack92()` at compile time, so the listing below and
the words the host uploads cannot drift apart. That is the structural answer to
the two §3.3 errors: an image whose ports disagree with `regs::falcon_io()` no
longer compiles.

### 9.1 What was added, and why each is an observable and not a decoration

1. **The split observable.** The s37 ack was a single bit of information:
   `CC_SCRATCH[1]` moved. That is consistent with "our ucode read the command
   and echoed it" *and* with "something else wrote that register". The ucode now
   also writes the **value it read** into `MAILBOX0` via `I[0x1000]`
   (`falcon_io(0x040)`, PROVEN s29). `ack=1` with `mb0=1` is an echo; `ack=1`
   with `mb0=A5A50000` (the seed) would be a *different* mechanism, and we would
   now see that rather than credit it to ourselves.
2. **The phase counter.** `MAILBOX1` (`I[0x1100]`, `falcon_io(0x044)`, PROVEN
   s30) is stamped before and after each risky IO step. A ucode that dies mid-
   sequence now names the instruction it died on instead of returning silence.
   Image A stamps `0x01..0x04`, image B `0x11..0x14`, so `MAILBOX1` alone
   identifies the winner without reference to the ack (§5.2 distinct magics).
3. **The bound.** §5.1's law, owed since land review `bc5fe3fc`. See §9.4.

Host-side, both mailboxes are seeded with `MB_SEED = 0xA5A5_0000` before the
launch, so "unchanged" keeps its single meaning (§4 step 1) for the new
observables too.

### 9.2 Image A — indexed ports, down-counting bound (92 bytes = 23 words)

```assembly
// Addr | Bytes       | Instruction         | Note
// -----|-------------|---------------------|-------------------------------------
// 0x00 | f0 17 00    | mov   $r1, 0x00     | low half of I[CC_SCRATCH[0]]
// 0x03 | f0 13 02    | sethi $r1, 0x02     | $r1 = 0x20000                  (s37)
// 0x06 | f1 27 00 01 | mov   $r2, 0x0100   | low half of I[CC_SCRATCH[1]]
// 0x0a | f0 23 02    | sethi $r2, 0x02     | $r2 = 0x20100                  (s37)
// 0x0d | f0 37 01    | mov   $r3, 0x1      | the ack value
// 0x10 | f1 67 00 10 | mov   $r6, 0x1000   | $r6 = I[MAILBOX0]              (s29)
// 0x14 | f1 77 00 11 | mov   $r7, 0x1100   | $r7 = I[MAILBOX1]              (s30)
// 0x18 | f0 57 00    | mov   $r5, 0x00     | loop counter, low half
// 0x1b | f1 53 10 00 | sethi $r5, 0x0010   | $r5 = 0x00100000 = ECHO_BOUND
// 0x1f | f0 07 01    | mov   $r0, 0x01     |
// 0x22 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase 01 (pre-loop)
// poll:
// 0x25 | cf 14 00    | iord  $r4, I[$r1]   | RISKY: read the command word
// 0x28 | d0 64 00    | iowr  I[$r6], $r4   | MAILBOX0 = VALUE READ  <-- split obs.
// 0x2b | f0 07 02    | mov   $r0, 0x02     |
// 0x2e | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase 02 (post-read)
// 0x31 | b0 44 01    | cmpu b32 $r4, 0x1   | is it the test command?
// 0x34 | f4 1b 14    | bra ne, +0x14       | -> 0x48 (dec), keep polling
// 0x37 | f0 07 03    | mov   $r0, 0x03     |
// 0x3a | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase 03 (pre-ack)
// 0x3d | d0 23 00    | iowr  I[$r2], $r3   | RISKY: CC_SCRATCH[1] = 1 (ACK)
// 0x40 | f0 07 04    | mov   $r0, 0x04     |
// 0x43 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase 04 (post-ack)
// 0x46 | f8 02       | exit                | terminal state: ack=1 mb0=1 phase=04
// dec:
// 0x48 | b0 52 01    | sub  b32 $r5, 0x1   | A's variable: 0xb0-form subop 2
// 0x4b | b0 54 00    | cmpu b32 $r5, 0x0   |
// 0x4e | f4 1b d7    | bra ne, -0x29       | -> 0x25 (poll)
// 0x51 | f0 07 bd    | mov   $r0, 0xbd     | EXIT BY BOUND
// 0x54 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = 0xBD
// 0x57 | f8 02       | exit                |
// 0x59 | 00 00 00    | (padding)           | 92 bytes = 23 words
```

Packed (`ucode::UCODE_CTX_ECHO_A`):

```
F00017F0 27F10213 23F00100 0137F002 100067F1 110077F1 F10057F0 F0001053
70D00107 0014CF00 F00064D0 70D00207 0144B000 F0141BF4 70D00307 0023D000
D00407F0 02F80070 B00152B0 1BF40054 BD07F0D7 F80070D0 00000002
```

The first four words are byte-identical to the s37 image — the acked prologue
did not move, and a `const _: () = assert!` in `kepler.rs` holds it there.

### 9.3 Image B — the A/B fallback, on exactly one variable

Every instruction in this program except one has already run on metal or is
cited in §8. The exception is the counter arithmetic. `cmpu` is metal-proven at
subopcode **4** in the `0xb0` form (s37, `b0 44 01`), which fixes that form's
subop table as `0=add, 1=adc, 2=sub, 3=sbb, 4=cmpu`. Image A takes `sub`
(subop 2) and counts **down** from `+ECHO_BOUND`; image B takes `add`
(subop 0 — the least disputable entry of any ALU table) and counts **up** from
`−ECHO_BOUND` = `0xFFF0_0000`. Both exit the loop when `$r5 == 0`; both bound at
exactly `ECHO_BOUND` iterations. One boot settles the subop question either way
(§5.2), and if A is wrong we still get the split observables from B.

Deltas from image A — same addresses throughout, since every substituted
instruction is the same length:

```assembly
// 0x1b | f1 53 f0 ff | sethi $r5, 0xfff0   | $r5 = -ECHO_BOUND
// 0x21 |          11 | mov   $r0, 0x11     | phase (pre-loop)
// 0x2d |          12 | mov   $r0, 0x12     | phase (post-read)
// 0x39 |          13 | mov   $r0, 0x13     | phase (pre-ack)
// 0x42 |          14 | mov   $r0, 0x14     | phase (post-ack)
// 0x48 | b0 50 01    | add b32 $r5, 0x1    | B's variable: 0xb0-form subop 0
// 0x53 |          be | mov   $r0, 0xbe     | EXIT BY BOUND
```

Packed (`ucode::UCODE_CTX_ECHO_B`):

```
F00017F0 27F10213 23F00100 0137F002 100067F1 110077F1 F10057F0 F0FFF053
70D01107 0014CF00 F00064D0 70D01207 0144B000 F0141BF4 70D01307 0023D000
D01407F0 02F80070 B00150B0 1BF40054 BE07F0D7 F80070D0 00000002
```

A `const` block in `kepler.rs` proves the pair diverges at **exactly** eight
byte positions (the counter seed, the five phase magics, the subop nibble), so
the A/B experiment cannot silently acquire a second variable.

### 9.4 How the bound was chosen

`ECHO_BOUND = 0x0010_0000` = 1,048,576 iterations.

- The loop body is 8 instructions, so the bound is ~8.4M Falcon instructions —
  milliseconds of Falcon time.
- The window it must cover is tiny: the host writes the command word one
  `mmio_write` plus one `serial_println!` after `CPUCTL <= 2`, and at s37 the
  ucode had already consumed it by the host's **first** poll (`iters=0`).
- The bound is therefore ~3 orders of magnitude larger than the window. It
  exists to guarantee termination (§5.1), not to be reached. `0x0010_0000` also
  costs nothing to encode: it is one `sethi` immediate, and it is the same
  order as the heartbeat's authored bound `0x00500000`, which terminated exactly
  on schedule at s33boot2.
- If `phase` ever comes back `0xBD`/`0xBE`, the command never arrived. That is a
  **real finding** about the host↔FECS channel, not a tuning problem — do not
  raise the bound in response to it.

The echo exits after a successful ack rather than looping back, which gives a
deterministic terminal state (`ack=1 mb0=1 phase=04`) for the host to read.
Pull 33's post-ack `bra` back to the poll is what made the loop unbounded; the
host-commandable exit that would let it keep polling safely is still pull 34+
work. **The §5.1 defect raised at land review `bc5fe3fc` is discharged by this
section**: both images terminate by construction, on every path.

### 9.5 The witness line

```
:: kepler: ctx-echo img=A ack=00000001 mb0=00000001 phase=00000004 ::
```

printed immediately after the existing `host-ack CC_SCRATCH[1]` line, and, on a
bound exit only:

```
:: kepler: ctx-echo EXIT-BY-BOUND img=A iters=1048576 — command never observed ::
```
## 10. The terminal poke — `0x409504`, once, last

§5.4 established the poison law and pull 28 turned it into a standing ban on
unproven writes into this unit. Peter lifted that ban on 2026-07-26 for exactly
one write, and this section records its terms so the exemption cannot spread.

**What it is.** A single write-only poke of `0` to host `0x409504`
(`WRCMD_CMD`), as the **last kepler statement of the boot**.

**Why `0`.** It is the least-assumptive value available: it asserts no command
encoding, no bit layout, no field. We are testing whether the offset accepts a
*write* at all, given that every *read* of it faults (s31/s32/s34) and that
nouveau drives this register on gk104 (§5.4's "surprise value" note).

**The ordering contract.** The poke sits at the end of the kepler leg sequence,
after the late display recap, which is after the `ucode-post` sweep, the
witness rematch and every other `0x409xxx` read in the boot. Nothing in
`kepler::init()` touches the unit after it. This is the §5.4 rule applied
literally — *put unproven offsets last, after every proven read has completed* —
and it is why a poison, if the write triggers one, cannot confound a single
earlier datum.

**No readback.** A readback would be a read of the poisoning offset, which is
the exact access s31 convicted. There is none. The witness is therefore printed
**before** the write:

```
:: kepler: terminal-poke 0x409504 wr=0 (post: no further FECS reads this boot) ::
```

so the capture proves the ordering: the line appears, the write happens, and
whether the boot continues cleanly past it is itself the observation. The
evidence available from this leg is (a) the line appears at all, (b) the boot
survives to its normal end markers, and (c) the *next* boot's rest values are
unchanged. Nothing more is claimed.

## 11. FENCE — the falcon asserts CHAN_VALID

This section appends to §3, §5 and §8. Nothing above it is superseded.

**Status: FLOWN ONCE, VOID. Round 2 is UNFLOWN.** Boot AS (2026-08-09,
`~/unaos-bench/capture/gr24-bootAS/ttyUSB0.log`) executed the round-1 image and
returned `FENCE VOID (treatment not applied)`. It settled nothing about
candidate 1 — by design, the gates refused to interpret `err=2` on a boot where
the treatment never landed — and it exposed a defect in the leg's own
instruments. §11.6 records what it found; the image and host leg in the tree
today are **round 2**, and round 2 has never executed on hardware.

They cannot be exercised in emulation either — **QEMU has no Kepler**. A green
`test-x86` on this code means it took a path that never touched a GPU, which is
worse than a hang because it reads like evidence. Do not cite emulation for any
claim in this section. Predictions for the first capture are recorded outside
the tree, before the fact, at
`~/unaos-bench/scratch/gr23/fence-predictions.md`.

### 11.1 The experiment

§7 named the wall: `ENGINE_STATUS.CHAN_VALID` is the bit PFIFO's validation
plausibly keys on, and it is set by nothing reachable from the host (refutation
7). Candidate 1 is that the assertion must **originate from the falcon**,
mimicking a real context-switch completion. `ucode::FENCE_A_BYTES` writes
`CHAN_VALID` into `ENGINE_STATUS` from inside FECS, reads it back into MAILBOX0,
holds it while the host re-validates the channel, and clears it on host
command. What it measures is **channel validation** — `PFIFO_CHAN[1]` written,
the error read at `0x252c` — apples-to-apples with the existing validate legs.
It does **not** submit a runlist; the submit (`0x2270`/`0x2274`) is downstream of
this leg and neither reached nor perturbed by it.

It is decisive both ways. `err=0` proves PFIFO only trusts falcon-originated
state. `err=0x2` eliminates the candidate and points at engine binding at submit
— and, because the readback tells us whether the bit was even set, that
elimination is sharper than the eleven strip eliminations before it.

### 11.2 A new port, DERIVED

| Host offset | Register | `& 0xffc` | `<< 6` | Falcon index | Status |
| --- | --- | --- | --- | --- | --- |
| `0xC00` | `ENGINE_STATUS` | `0xC00` | | **`0x30000`** | **DERIVED — one null observation, Boot AS (§11.6)** |
| `0xB00` | `CHAN_CUR` | `0xB00` | | **`0x2C000`** | **DERIVED (untested)** — round 2's mapping-ladder rung |

Both derived by the §3 rule, asserted at compile time in `regs`, and never
hand-written at a call site. They have the same standing the `0x800`/`0x804`
pair had before s37: correct by the only rule that has ever been right, and
unproven.

The paragraph that used to end this subsection predicted the ambiguity exactly,
and Boot AS delivered it: *"if the falcon's readback of this port returns `0`
while the ack says the assert executed, 'wrong port' and 'the bit is not
falcon-assertable' are not yet distinguishable — that ambiguity is a finding,
not a defect."* It is still true, and it is still not a defect. What round 2
adds is the control that resolves it, because a finding you cannot act on is
where a campaign stalls.

**Why `CHAN_CUR` is the right rung.** Every port the `<< 6` rule has ever been
proven at — `0x1000`, `0x1100` (s29/s30), `0x20000`, `0x20100` (s37) — derives
from a host offset at or below `0x804`. `0xC00` is the first port any image has
used from the region above it, so Boot AS's null tests the rule and the
hypothesis at the same time and can attribute the result to neither. `CHAN_CUR`
splits them: its IO index `0x2C000` sits in the same unexplored high region,
and unlike `ENGINE_STATUS` it is **proven host-writable** (§2, s35 — a host
write took), which is what lets the *host* stage the experiment and the falcon
merely read it. A magic the falcon reads back through `I[0x2C000]` proves the
derivation reaches the high region; anything else lands the fault on the
mapping. See §11.6 for why the probe reads rather than writes.

### 11.3 The assertion lattice — the actual gate

The listing in `kepler.rs` is a doc comment on the byte array, and a block of
`const _: () = { … }` assertions checks the bytes back against it. Both images
that preceded FENCE carry the same treatment. The property being built is that
**the bytes are checked against the listing by arithmetic the author did not
perform**:

- listing → bytes (typed by hand, from the mnemonics), and
- bytes → decoded fields → listing (computed by the assertions).

The assertions therefore check **decoded properties, never literal bytes**. This
distinction is the whole point and it is not stylistic:

```rust
// A CHECKSUM OF YOUR OWN TYPING — would not catch a wrong displacement,
// because you would type the same wrong number in both places:
assert!(b[0x43] == 0xf4 && b[0x44] == 0x0b && b[0x45] == 0x14);

// VERIFICATION — contains no displacement at all. It computes one from the
// two addresses the listing names, and `bra_target` recovers the address
// from the byte by the opposite arithmetic:
assert!(eq3(&slice3(b, 0x43), &bra_to(BRA_EQ, 0x43, 0x57))); // -> do_assert
assert!(bra_target(b, 0x43) == 0x57);
```

**Be precise about what the second assertion buys**, because the loose version of
this claim is itself a trap. `bra_to` and `bra_target` are exact inverses, so for
a branch the second assert is logically *implied* by the first — it is not an
independent derivation and calling it one overstates the lattice. What it
actually catches is **one-sided redefinition**: if someone changes `bra_target`
alone (to `at + 3 + disp`, the historical bounce), the two stop agreeing and the
build fails, naming every branch. That is a real and historically-attested
failure mode, and it is worth two lines — but it is a consistency check between
two helpers, not a second opinion about the bytes.

The branch convention is settled by **metal**, not by preference: under
`at + 3 + disp` the s37 ECHO loop could never have re-executed its `iord` and so
could never have acked, yet s37 observed `ucode-echo SUCCESS`. Hardware refutes
the alternative.

Standing requirements for any future image in this module:

1. Every branch gets **both** a `bra_to(cc, at, label)` and a `bra_target`
   assertion. ⚠ **Falcon `bra` displacements are relative to the address of the
   branch instruction itself**, not the following instruction — envydis resolves
   them that way and every image here is authored under that rule. Re-deriving
   it as `at + 3 + disp` (the intuition from most other ISAs) shifts every
   target by the instruction width; it reads perfectly in a listing and lands
   mid-instruction on silicon.
2. Every port immediate comes from `regs::falcon_io()`. Never a typed hex
   literal — that rule predates this section and has been violated once.
3. Assert decoded **fields** (opcode via the constructor, register index via
   `reg << 4`, immediate via the named constant) wherever a decoder can be
   written.
4. Keep the zero-tail guard and the anti-`0x409504` guards. The tail helpers are
   bounded by `b.len()`, not a literal: they once took `[u8; 128]`, and a
   192-byte image checked under a `< 128` bound would leave its last 64 bytes
   unexamined while still reporting a clean tail.
5. Every store uses `iowrs` (`0xd1`), and the lattice asserts the async `iowr`
   (`0xd0`) form appears nowhere. The two differ by one bit, read identically in
   a hex blob, and the async form yields a program that "runs" while its
   observable never arrives.
6. **An unproven IO port may be READ but not WRITTEN** (added after Boot AS,
   §11.6). `+0x118` in the standard falcon block is `DMATRFCMD`, so an address
   whose decode has not been established can be a **command register**: a write
   to one does not merely fail to land, it may start something, and from the
   mailboxes those two look identical. Prove a mapping by having the host plant
   an authored magic in a register §2 shows host-writable and the falcon `iord`
   the derived port — never by having the falcon write it. Where an image must
   write an unproven port because that write *is* the experiment, the host gates
   it on a prior read-side confirmation. The lattice enforces the read-only rule
   by enumerating all sixteen source registers against `iowr`/`iowrs` on the
   probe port, not by spot-checking the ones the listing happens to use.
7. **No external assembler.** Images are hand-authored against the listing. A
   script outside the tree that produces shipped kernel bytes has no provenance
   and no review; if a helper is ever written it belongs under `unaos/tools/`
   with its own tests, and the lattice must still assert independently of it.

A green `./arroyo check` means the lattice agreed — that is the test passing,
not a formality. **A bare `./arroyo check` DOES evaluate these assertions**, with
no knob required: `kepler.rs` is behind the `nvidia-kepler` feature, but
`check_both` runs `check_kernel_cfg`, and two legs of `KERNEL_CFG_MATRIX` —
`x86-all` and `x86-nopace` — carry that feature. Breaking a single image byte
turns `x86-all`, `x86-nopace` and `x86-mix-0` red on a plain `check`.

That was not always true, and the history is worth keeping because it is the
reason to state the current fact precisely. The ECHO lattice landed at
`a93e7927` (2026-07-26); the cfg-coverage gate that compiles it by default
landed at `944b853e` (2026-08-03, "arroyo: close two vacuities in the check
gate"). For those eight days the assertions were real but **default-unchecked** —
green on a bare `check` that never compiled them. Since `944b853e` they have
been checked by every run.

⚠ Do not restate the stale version. A future session that reads "the bare gate
does not compile this" will conclude its lattice is dead when it is live, and
either stop trusting a working gate or rebuild one that already exists. If you
need to confirm the property rather than take it on faith, the check is cheap:
perturb one byte of any image and run a plain `./arroyo check`.

### 11.4 The ritual, restated as the FENCE leg implements it

§4 in full, plus three things §4 did not say out loud:

- **Halt the core before every upload, and prove the halt by readback.**
  `CPUCTL` bit 4 is STOPPED — §2.1 documents it as a **status** bit ("set when
  the core is halted"), and *writing* it as a halt request is **DERIVED,
  uncited**: no sitting has demonstrated the write-side behaviour, and the only
  documented write to CPUCTL is bit 1 (START_TRIGGER). The readback is also
  weak by construction — the rest value is `0x10`, so on a core already parked
  the "proof of halt" passes trivially and is unproven in exactly the case
  where it matters. It is kept as a gate because every image ends in `exit`
  (which parks at `0x10` per s33boot2) so the gate's failure mode is a skipped
  leg, not a lie — but a future sitting should cite or falsify the write-side
  claim. Rewriting IMEM under a running falcon leaves it executing a half-old,
  half-new program that goes on writing the very mailboxes the next leg reads
  as its own result. The ECHO leg was doing exactly this and now halts too.
- **Seed all four observables** (MAILBOX0, MAILBOX1, CC_SCRATCH[0],
  CC_SCRATCH[1]) before each launch, so "unchanged" keeps one meaning for every
  one of them.
- **Per-image magic as the first executed instruction.** FENCE writes
  `A55E7A55` to MAILBOX0 before anything else. With the host seed `A5A50000`
  this separates three otherwise identical silences: *uploaded and running*,
  *nothing ran*, and *IMEM held stale bytes from another leg*. Without it, "the
  mailbox did not change" and "a different program is running" read alike.
- **The ack gets its own register.** `CC_SCRATCH[1]` carries "I reached the
  assert"; MAILBOX0 carries the ENGINE_STATUS readback. Two facts, two
  registers — on one register they are untellable.
- **Two loops, two counters, two give-up markers.** poll1 exhausting means the
  ASSERT command was never observed (`FFFFFFBD`); poll2 exhausting means it
  asserted and then never observed the CLEAR (`FFFFFFBC`). Those license
  opposite conclusions about the host↔FECS channel. Both are `u32` constants and
  the host gate compares them for **equality, first** — as sign-extended words
  they satisfy any naive `> 0` or `>= 3` progress test placed ahead of them.

### 11.5 The verdict is gated on the channel readback

After rewriting `PFIFO_CHAN[1]` the host **reads it back** and prints
`VALID-stuck=`. If the hardware dropped VALID immediately, `err` describes some
other channel state and means nothing about this experiment; the leg prints
`FENCE VOID` and draws no conclusion in either direction. The restore of
`inst_off+0x0C` reproduces the canonical write **including `| 0x80000000`** — the
pre-existing restore dropped that bit, so a "restored" instance block was never
the one the channel was built with.

Two more VOID arms sit ahead of the readback gate (adoption-review conditions):

- **`took_host`** — the falcon-side readback travels through the DERIVED port
  0x30000 twice, so on a wrong derivation `took=Y` proves only self-consistency.
  The host's own read of `fb+0xC00` (§2-metal-proven) must carry the bit too;
  `FENCE VOID (port unconfirmed)` otherwise, because a verdict measured on the
  wrong register would be *believed* — err=2 is the expected answer.
- **`held`** — the treatment is re-read at the moment of the stimulus
  (`hold-recheck` line): between the ack and the channel write sit ~13 ms serial
  lines and the 134-offset delta sweep (which since round 2 runs on **every**
  FENCE boot, not only the ambiguous branch), while poll2's budget is tens of
  ms. A lapsed hold prints `FENCE VOID (hold lapsed)`.

**Unwind is guaranteed host-side.** The falcon's `do_clear` is the polite path
and timing can defeat it (giveup2 exits without clearing). On `cleared=N` the
host writes `0` to `fb+0xC00` itself and prints `HOST-FORCED clear`. This is
also the note the instrument-baseline law demands: on a FENCE boot, the
downstream `bind-post ENGINE_STATUS=` and `witness pre-rewrite PFIFO_CHAN[1]=`
lines read state FENCE touched (and then restored/cleared) rather than what the
previous leg left — their baselines are FENCE-relative on such boots.

The leg is placed after every ECHO observable is harvested and **before** the
`0x409504` recon block, because the first access to that offset wedges every
later read in the unit for the boot (§5.4) and would void the verdict.

### 11.6 Boot AS (2026-08-09) — what flew, and what `0x118` actually was

Capture: `~/unaos-bench/capture/gr24-bootAS/ttyUSB0.log`. Read it with `awk`,
not `grep` (control bytes). The leg ran end to end in about 2 ms of boot time.

**What worked.** Everything up to the assert. `halted=Y`; `verify ok=Y words=48`
— every uploaded word matched; `tlb page0=01000000 usable=Y`; `gate1 PROGRESS
mb1=00000001`; `magic mb0=A55E7A55` — the image was uploaded and running and was
*this* image. The unwind worked too (`unwind DONE mb1=00000005`,
`cleared=Y`), so `do_clear` executed and the leg left the unit as it found it.
The lattice, the halt gate, the seed discipline and the phase stamps all did
their jobs on first contact with hardware.

**The result.**

```
FENCE assert ack=00000002 iters=1 phase=00000004
      eng-status-per-falcon=00000000 eng-status-per-host=00000000
FENCE assert acked=Y took=N took_host=N class=ZERO
FENCE VOID (treatment not applied)
```

The falcon reached `do_assert`, executed `iowrs I[0x30000], 2`, read the same
port straight back, and got **zero**. The host's independent read of `fb+0xC00`
also read zero. The gates then refused to interpret `err=00000002`, which is
the behaviour the adoption review demanded and the single most valuable thing
the boot did: `err=2` is the expected answer after 28 sittings, so an ungated
leg would have printed `CANDIDATE-1 ELIMINATED` on a boot where the treatment
never existed.

**The instrument defect: `HIT off=118`.** The stray sweep reported

```
FENCE stray-sweep HIT off=118 val=00000002
FENCE stray-sweep HIT off=804 val=00000002
```

`0x804` is `CC_SCRATCH[1]`, the ack — the falcon wrote `2` there itself, and it
is legitimate.

**First, the methodological fault, because it is the one that generalises.** The
sweep hunts for the *value* `0x00000002`. Two is the lowest-entropy needle
available on a chip full of small status words, and §4.2 already says what
counts as proof of a write: *the exact authored magic arriving where predicted*,
never a merely-plausible value. A hit at `0x118` is **consistent with** our write
landing there and is **not proof of it**. Round 1 shipped a sweep that could not
have distinguished the two, and the fix is not a better argument about `0x118` —
it is that no future port experiment may plant `CHAN_VALID`'s bit pattern, or
any low-entropy value, as its needle.

**Second, the safety consequence, which must be read before anyone repeats the
experiment.** `+0x118` in the standard falcon register block is `DMATRFCMD` —
the DMA-transfer command register, three slots past the `DMACTL` at `+0x10C`
that §2 already documents. *If* our write did land there, Boot AS issued a
falcon DMA-transfer command by accident. That reframes what an unproven IO port
is: not a passive mailbox that either latches or does not, but possibly a
**command register**, where "the write did not land" and "the write started
something" are indistinguishable from the mailboxes. Every port experiment from
here treats an unverified IO address as potentially command-issuing.

**Third, and separately: `0x118` is probably not where the write went anyway.**
Three arguments, and they agree:

1. **`0x118` is `DMATRFCMD`** in the envytools falcon register map, and bit 1 of
   that register is `IDLE`. `0x00000002` is what it reads on a falcon whose DMA
   engine is idle — which this one is, because the FENCE upload uses the PIO
   path (`IMEMC`/`IMEMD`, §4) and never arms DMA at all. The value is the
   register's rest state.
2. **The sweep had no baseline.** It ran once, after the treatment, and compared
   against nothing. An instrument that reads its own pre-run state as a live
   effect cannot falsify anything — the standing instrument-baseline law, and
   the third time this campaign has been bitten by it.
3. **The falcon's own readback refutes the alias.** If `I[0x30000]` were an
   alias of host `0x118`, the `iord` three instructions later would have
   returned `DMATRFCMD`'s value, `2`. It returned `0`. A port that swallows a
   write and reads back zero is not `DMATRFCMD`, which latches and reports.

**Therefore the derivation is NOT convicted.** The honest reading of Boot AS is
narrower and more useful than "the port is wrong": *the falcon wrote 2 through
`I[0x30000]`, no host-visible register in the swept window took that value as a
change, and the same port read back zero.* That is consistent with two states of
the world, and Boot AS distinguishes neither:

- **(a) the port is right and `ENGINE_STATUS` is not falcon-writable** — which
  would extend refutation 7 to the falcon side and close candidate 1 by
  mechanism, a much stronger result than closing it by `err`; or
- **(b) IO addresses above `0x20100` do not decode on this unit**, in which case
  the write went to an unmapped port that swallows writes and reads as zero, and
  nothing about `ENGINE_STATUS` was tested.

One datum worth keeping either way: the `iowrs` **completed**. It is the
synchronous form, and the falcon went on to stamp phase `04` and later reach
`do_clear`, so `I[0x30000]` did not stall the core. Whatever that address is, it
is not a bus that hangs.

Note also that `0x30000` does not decode to `0x118` under any single shift or
mask. How a `2` would have got there is **UNKNOWN**, and that is a finding, not
a defect — §11.2's standing.

#### What round 2 changes

1. **A mapping ladder, run before the treatment, and READ-ONLY.** The host
   plants `PROBE_B_MAGIC = 0x0B005EED` at `fb+0xB00` (`CHAN_CUR` — §2 proves it
   host-writable at s35) before the core starts, and verifies the plant. The
   falcon then only `iord`s `I[0x2C000]` and reports what came back, into
   `CC_SCRATCH[1]` — a port proven at s37.

   **The direction matters and it was got wrong in a first draft of this
   round.** Having the falcon *write* the magic through the unproven port would
   have repeated Boot AS's real hazard: an IO address of unknown decode may be a
   command register, and a write to one does not merely fail, it *starts
   something*. A read cannot. So the only falcon-side writes in the image go to
   `CC_SCRATCH[1]`, `MAILBOX0` and `MAILBOX1`, all proven, and the lattice
   enumerates all sixteen source registers to assert that **no** `iowr`/`iowrs`
   to the ladder port exists anywhere in the image.

   Honest limit: this proves the mapping for the **read** path. The write decode
   is the same address decode, so a confirmed read is strong evidence for the
   write — but that is an inference, and the witness line says so.
2. **The assert is GATED on the ladder.** If the ladder does not confirm the
   mapping, the host never sends `CMD_FENCE_ASSERT`; the falcon exits poll1 by
   bound, and the leg prints `FENCE assert WITHHELD` with the reason. Boot AS
   wrote to an address of unknown decode without knowing that was what it was
   doing. Round 2 declines to. **A boot that answers the mapping question and
   refuses the unsafe write is a successful boot.**
3. **The ack carries a magic.** `do_assert` writes `ACK_MAGIC = 0xACC0ACC0`, not
   `2`, and the host's ack gate tests **equality** against it. This is not
   cosmetic: the ladder writes `CC_SCRATCH[1]` before poll1 begins, so the old
   `ack != MB_SEED` test would have fired on its first read and reported an ack
   the falcon had not yet given.
4. **The sweep became a delta sweep.** A 134-offset baseline is captured with
   the core halted and the image verified, immediately before `CPUCTL <= START`;
   the post-treatment pass reports only **changes**, each tagged with whether any
   leg of the boot admits to writing that offset. `0x504` is excluded by
   construction (§5.4). It runs on every FENCE boot, not only the ambiguous
   branch.
5. **`DMATRFCMD` and its neighbours are printed every boot**, before and after,
   on a `dmatrf-row` line that names what the register is — so the next session
   reads `0x118` as a rest value on sight instead of re-deriving it.
6. **`CHAN_CUR` is planted and restored host-side**, both from `fb+0xB00`. Same
   doctrine as the `ENGINE_STATUS` unwind: a restore that depends on the
   falcon's timing is one that timing can defeat.

#### What round 2's boot must show

The leg is decisive on the mapping in a single boot. `FENCE ladder VERDICT`
prints exactly one of four states, and each licenses a different next move:

| Verdict | Condition | What it settles |
| --- | --- | --- |
| `MAPPED` | plant took, falcon read the exact magic | `(off & 0xffc) << 6` reaches host `0xB00` from inside the falcon. The rule extends above `0x804`, `0x30000` is a plausible `ENGINE_STATUS`, the assert proceeds — and if it then reads back null, that is **state (a)**: `ENGINE_STATUS` is not falcon-writable, which extends refutation 7 to the falcon side and closes candidate 1 by *mechanism* rather than by `err` |
| `UNMAPPED` | plant took, falcon read something else | The rule does not reach `0xB00`. **State (b)**: `0x30000` is unproven, the assert is withheld, and round 3 is a mapping arc, not a fence arc |
| `UNRUN` | `CC_SCRATCH[1]` still holds the host seed | The falcon never executed the ladder read. A phase problem, not a mapping result |
| `VOID (plant refused)` | the host could not write `fb+0xB00` | Says nothing about the derivation — and puts §2's s35 "write took" claim in question |

Supporting lines the boot must also carry, or the ladder itself is not trusted:

- `FENCE ladder plant off=B00 … planted=Y` — before the start.
- `FENCE sweep-baseline n=134 captured` — before the start. Without it every
  delta below is unfalsifiable.
- `FENCE dmatrf-row pre …` and `… post …`. **Prediction, recorded before the
  fact:** `118=00000002` in both, unchanged, because it is `DMATRFCMD` idling.
  If the post row differs from the pre row, argument 1 above is wrong and this
  section needs rewriting.
- `FENCE ladder restore CHAN_CUR=00000000 restored=Y` — the perturbation is
  undone before the stimulus.
- `FENCE delta-sweep moved=N strays=M`. Under the read-only ladder the only
  value this leg can send to an unproven port is `CHAN_VALID`, and only on a
  `MAPPED` boot where the assert actually runs — so a **stray whose delta is
  `0 -> 00000002`** is what would name where `I[0x30000]` points. That is now a
  *delta* claim, not a value match, which is the whole difference from round 1.
  `strays=0`, with every `moved` offset accounted for by an admitted writer, is
  the clean outcome; `0x0B005EED` should appear at `0xB00` and nowhere else.
- `FENCE VOID cause — ladder mapped=…` on any void boot, naming which of states
  (a) and (b) the evidence supports. A void that does not say why is what round
  1 delivered, and it is what cost this round.

One observed point does not fix a whole map, and round 2 does not claim to fix
one. It proves or refutes a single property — *does the `<< 6` rule reach host
offsets above `0x804` from inside the falcon* — on a register chosen so that the
answer cannot be confounded by the register's own behaviour, and by a method
that cannot issue a command on a wrong guess. That property is the one thing
standing between Boot AS's null and a conclusion.

#### 11.6.1 A competing root cause, probed in the same boot

**Scope note: this rung is not part of the FENCE experiment**, does not touch
the FECS unit, and is documented here only because the same boot carries it and
because it may moot everything above.

`kepler.rs` hands `VramAllocator` offsets — which are offsets into the **BAR1
aperture** — to PFIFO as if they were physical VRAM page numbers
(`0x800000+8 <- 0xC0000000 | (inst_off >> 12)`, `0x2270 <- runlist_off >> 12`).
On this generation BAR1 is generally understood to be a **paged window** with
its own instance block and page tables (EXT, **UNPINNED**). If that holds here,
a BAR1 offset is a virtual address in the BAR1 space, `inst_off >> 12` is not
the page the FIFO fetches, and **every FIFO pointer this driver has ever written
was wrong** — which independently explains `err=0x2`, `ACTIVE=0` on all three
PBDMAs, and `gp_get=0`. That would not retract the fence arc's eliminations, but
it would **re-scope** them: they were measured through a broken pointer.

The probe plants `0xCEA50BA5` through BAR1 at a **freshly allocated scratch
page** (never inst/gpfifo/userd/runlist), points the PRAMIN window at the
physical page of the same number, and reads the dword back through BAR0.

- `IDENTITY` — the magic returns. BAR1 offsets are physical addresses here, the
  existing FIFO pointers are fine, and this root cause closes as a false alarm.
  **Record that as a positive result, not a wasted read.**
- `PAGED` — the magic is at the BAR1 offset but not at the physical page of the
  same number. The root cause is live and takes priority over the port question.
- `VOID` — the BAR1 write did not stick, `0x001700` refused the window value, or
  `0x700000` read `0xBADFxxxx`. The rung was mis-derived; the question is still
  open. `NV_PBUS_BAR0_WINDOW = 0x001700` is **DERIVED, UNPINNED**.

⛔ **Risk, stated.** This makes **one write to an unproven PBUS register**. It is
a deliberate, single, reversible exception to the pull-28 ban: the pre-value is
read first, the window is restored immediately, and the restore is verified in
the witness line. It sits after every proven read of the boot and immediately
above the terminal poke, under the §5.4 ordering contract.
