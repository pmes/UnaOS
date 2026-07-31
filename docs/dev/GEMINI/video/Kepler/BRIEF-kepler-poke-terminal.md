# BRIEF — kepler: split the falcon-execution test from the 0x409504 read

**Your tree:** `~/src/github.com/pmes/UnaOS-gemini-kepler`  ·  **branch** `wt/kepler-poke-x86`  ·  already at trunk `ce5c6f49`.
**Read [`../../RELAY.md`](../../RELAY.md) first.** One file is yours:
`unaos/crates/kernel/src/drivers/gpu/kepler.rs`.

**Your gate** — run exactly this, from `<your tree>/unaos`, and confirm the banner lists
`nvidia-kepler`:

```
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check
```

**Where your artifacts go, committed, in your tree:**

| What | Path |
|---|---|
| Your plan, before you write code | `docs/dev/GEMINI/video/Kepler/PROPOSAL-kepler-poke-terminal.md` |
| What you did, after | `docs/dev/GEMINI/video/Kepler/WALKTHROUGH-kepler-poke-terminal.md` |
| Anything you found but did not fix | `docs/dev/GEMINI/video/Kepler/FINDINGS-kepler-poke-terminal.md` |

---

## The goal

`ECHO_A_BYTES` currently reads `0x409504` from inside a microcode image that runs
**mid-sequence**. That read poisons the FECS unit for the rest of the boot. Split it:

- **`ECHO_A_BYTES`** — the falcon-execution test. Command in, ACK out, phase stamps. **No
  `$r8` setup, no `iord I[$r8]`, no `0x409504` in any form.**
- **`POKE_A_BYTES`** — a second image that does carry the `$r8` = `falcon_io(0x504)` setup
  and the read, executed **once, at the terminal phase**, immediately before the host's
  existing terminal `fecs_write(bar0, 0x409504, 0)`.

## Why — the poison law, from your own spec

`docs/dev/OS/08_VIDEO/falcon_microcode_spec.md`:

```
:68   | +0x504 | WRCMD_CMD | ⛔ FAULTS — poisons the unit | s31, s32, s34
:253  The first access to 0x409504 (WRCMD_CMD) faults immediately and wedges
      every subsequent read in the FECS unit for the rest of the boot
:537  §5.4 established the poison law and pull 28 turned it into a standing ban
:535  ## 10. The terminal poke — 0x409504, once, last
```

Reading that register falcon-side, where the host side faults, is a legitimate experiment.
It must simply be the **last** thing the kepler leg does.

## A large part of this is already done — start from it, do not redo it

`docs/dev/GEMINI/salvage/kepler-echo-poke-split.patch` is the previous session's split,
which was **verified correct at the byte level** and then abandoned uncommitted. Its byte
arrays are good:

- envydis: **zero** unknown instructions in either image's executable region.
- All six branch displacements resolve by arithmetic — ECHO `0x34+0x2b=0x5f`,
  `0x3a+0x14=0x4e`, `0x54−0x2f=0x25`; POKE `0x3b+0x2e=0x69`, `0x41+0x17=0x58`,
  `0x5e−0x32=0x2c`.
- `$r8 = 0x14100 = falcon_io(0x504)` confirmed.
- Assertion coverage has **zero unpinned holes** across all 128 bytes of both arrays.

Apply it, keep the arrays, and fix what is listed below. Re-verify with envydis yourself —
do not take the above on trust.

## ⛔ What is broken in that patch, and must be fixed

**1. The POKE block addresses raw BAR0 with no `0x409000` FECS base.** Every access —
`0x104`, `0x180`, `0x184`, `0x804`, `0x800`, `0x100`, `0x044`, `0x040` — lands in the **PMC
master-control block**, not the FECS falcon. That is ~66 wild MMIO writes into the GPU's
master control at boot, and because they bypass `fecs_read`/`fecs_write` the FECS access
ledger never counts them. The ECHO block does the identical sequence correctly, with
`let base = 0x409000;`. Use `fecs_write(bar0, base + …)` throughout.

**2. CPUCTL and BOOTVEC are swapped.** The POKE block treats `0x104` as CPUCTL and `0x100`
as BOOTVEC. The map is **CPUCTL = `0x100`, BOOTVEC = `0x104`**, as the ECHO block has it on
the s37-metal-proven path. As written, POKE writes START_TRIGGER into the wrong register.

**3. The upload omits the IMEMT tag and the page padding.** ECHO writes `0x188` (tag = 0)
and pads to `IMEM_PAGE_WORDS`; POKE writes only IMEMC/IMEMD for its 32 words. Your own
comment states the rule: the code TLB marks a page usable only when the last word of the
0x40-word page is written.

Any one of those three alone prevents the falcon from executing.

**4. The verdict is too wide.** `if ack != MB_SEED` with only `0xBADF0000` carved out means
`0xFFFFFFFF` (bus float), `0xBAD0BA20` and `0x00000000` all print SUCCESS. This file already
classifies all three correctly — `kepler.rs:697` treats them as ABSENT, and `:1129` uses
`(x >> 16) == 0xBADF || (x >> 16) == 0xBAD0`. Reuse that predicate. **A poison read must
print POISON, never SUCCESS.**

**5. `FECS_504_READ_TOUCHED` must be set on the falcon-side read.** The ledger only watches
host `fecs_read`/`fecs_write`, so a falcon `iord` is invisible to it and it will report
`504_read_touched=false` on exactly the boot where the falcon touched it first. A watcher
that certifies the event it cannot see is worse than no watcher.

**6. Restore the guards the patch deleted.** All three existed and were stronger:
- **Arithmetic** branch assertions (`0x3b + slice3(..)[2] as i8 == 0x69`), not byte literals
  with the target in a comment. A literal says nothing about where the branch lands.
- `IO_MAILBOX0/1`, `IO_CC_SCRATCH0/1` derived via `falcon_io()`. These existed specifically
  to catch a raw-host-offset listing — which is exactly what defect 1 is.
- Delete the second `falcon_io` the patch adds inside `mod ucode`. It **shadows**
  `regs::falcon_io` and drops the `& 0xffc` mask, giving two sources of truth against a law
  your module doc calls single-point.

**7. Smaller, all real:** `mailbox0={:08X}` prints `ack`, which is CC_SCRATCH[1], not
MAILBOX0. The POKE host poll spins to `ECHO_BOUND` (1,048,576 MMIO reads, ~1 s of boot) with
no `spin_loop`, reusing a falcon instruction bound as a host read count. `#[rustfmt::skip]`
was dropped from both arrays and a `cargo fmt` would destroy the one-instruction-per-line
layout the review method depends on. And `mod tests` still does not compile — it calls
`pack92` (only `pack128` exists) and asserts POKE offsets against `ECHO_A_BYTES`. Either
make it compile and get it into a gate, or delete it; it currently reads as coverage and is
none.

## Verification — this is the bar, not a suggestion

Build `envydis` from the in-repo `envytools` (cmake + gcc; you may need
`-DCMAKE_POLICY_VERSION_MINIMUM=3.5`), extract **both** byte arrays mechanically from the
source, and disassemble each as falcon v4. Put both full listings in your walkthrough.

**Zero unknown instructions in each executable region. Every branch displacement resolved by
arithmetic. Every port immediate derived from `falcon_io()`.** Nothing less.

## Out of scope — raise, do not touch

The runlist submitted with `LEN=3` contains beacon words: `kepler.rs` writes 3 entries, then
overwrites `runlist_off+0..+31` with `0xBEAC0001..0xBEAC0008`, and nothing restores it before
the submit. Real, known, and a separate arc. Record it in your FINDINGS file.

## Done gate

Your plan committed before you write code · the fixes above · both envydis listings in your
walkthrough · the gate above green on **both** arches · everything committed on
`wt/kepler-poke-x86` · and a commit body that says plainly what you did not do.
