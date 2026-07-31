# BRIEF — igpu: a GMUX switch that cannot strand the panel

**Your tree:** `~/src/github.com/pmes/UnaOS-gemini-igpu`  ·  **branch** `wt/gmux-igd-x86`  ·  already at trunk `ce5c6f49`.
**Read [`../../RELAY.md`](../../RELAY.md) first.** Your file is
`unaos/crates/kernel/src/drivers/gpu/igpu.rs`.

**Your gate** — run exactly this, from `<your tree>/unaos`, and confirm the banner ends
`…,unaos_ivb,gmux_igd`:

```
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 UNAOS_GMUX_IGD=1 ./arroyo check
```

Run it **without** `UNAOS_GMUX_IGD=1` as well. Both must be green, and the knob-off build
must be behaviourally identical to trunk.

**Where your artifacts go, committed, in your tree:**

| What | Path |
|---|---|
| Your plan, before you write code | `docs/dev/GEMINI/video/iGUI/PROPOSAL-igpu-gmux-igd.md` |
| What you did, after | `docs/dev/GEMINI/video/iGUI/WALKTHROUGH-igpu-gmux-igd.md` |
| The operator procedure for a black panel | `docs/dev/GEMINI/video/iGUI/RUNBOOK-gmux-igd.md` |
| Anything found but not fixed | `docs/dev/GEMINI/video/iGUI/FINDINGS-igpu-gmux-igd.md` |

---

## The goal

Point the display mux at the integrated GPU, prove the write landed, and **get back**
without human intervention. The knob `UNAOS_GMUX_IGD=1` exists and is now wired in all
three places (trunk `ce5c6f49`) — you do not need to add it.

**The panel WILL go black, and that is not the experiment failing.** Your own census shows
every iGPU pipe, plane and PLL reading zero, so nothing is driving the panel from that side.
**The deliverable is the read-back proving the mux write landed** — which a future round
needs before it configures pipes. Say that plainly in your proposal so the result is not
read as a defeat.

## What is already right — keep it, do not rewrite it

These were hard-won over three rounds. Preserve them:

- **The ISR hook is trivial and must stay trivial.** `gmux_tick()` loads state, unpacks,
  compares a deadline, sets a flag, stores. **No port I/O, no loop, no blocking wait.** It
  runs at 1 kHz on an interrupt gate with IF=0, before `eoi()`, on the only core that
  advances the global ms clock — anything that blocks there stalls the clock it depends on.
- **`RevertState` pack/unpack.** One encode/decode point. Every mutation routes through it.
  This is what stopped a saved byte being lost to a mask.
- **The `0xFFFFFFFF` timeout sentinel refuses to arm.** A pre-switch read that timed out
  means there is no known state to return to, so there is nothing safe to switch away from.
- **Port constants and write order match upstream** (`0x7C2` value, `0x7D0` read-index,
  `0x7D4` write-index/status; DDC `0x28` → DISPLAY `0x10` → EXTERNAL `0x40`), including the
  `wait_ready()` between the value and index writes. An earlier review told you to remove
  that wait; **that instruction was wrong and is retracted** — upstream `apple-gmux.c` has
  it exactly there. Keep it, and cite your reference in a comment so it is not re-raised.

## ⛔ What must change

**1. The switch must not arm where its revert cannot run.** Today the arm fires from
`igpu::init()` inside `pci::init`, while the only revert executor — `gmux_task_tick()` —
lives in `x86_usb_pump`, spawned ~350 lines of boot later and only when *(not `rast`)* and
*(framebuffer non-zero)* and *(two distinct APs online)*. Three live paths therefore end
with the mux switched and no revert, permanently, until power cycle:
- the inline-BSP fallback path, where the pump never spawns at all;
- `rast` builds;
- any wedge between the arm and the pump's first pass — SDHC, storage, SMP, xHCI
  enumeration and the GUI handoff all sit in that gap.

**Arm from a context whose revert driver is provably live, or refuse to arm.** A switch
whose recovery might not exist is not an experiment, it is a coin toss with the bench
machine's only display.

**2. The manual trigger must complete the revert itself.** `gmux-revert` currently sets
`state.due = true` and returns; every port write is in `gmux_task_tick()`. On exactly the
paths where the automatic revert is already dead, the operator types the verb, sees
*"Manual GMUX revert triggered (if armed)"*, and nothing moves. **A recovery path that
reports success while doing nothing is the worst failure mode available here** — it will be
typed blind at a black panel by someone who then believes it worked.

Note the constraint: this rig's serial console is **kernel-TX-only**, so there is no typing
over the wire. The operator is typing blind on the internal keyboard. Everything from
EHCI-HID through `handle_key` must still be alive with the mux switched away — verify that
and state it in the RUNBOOK.

**3. Refuse to arm on an unproven protocol.** `boot_ver_ok`/`kern_ver_ok` are computed and
the if/else closes at `igpu.rs:410`; the arm block opens at `:413`, outside both branches.
It arms whether the driver printed `PROTOCOL PROVEN` or `PROTOCOL UNPROVEN`. A gmux that
answers the handshake but reports an implausible version passes the `0xFFFFFFFF` sentinel
and gets its display mux written anyway.

**4. Leave the knob-off build identical to trunk.** `gmux_wait_ready`/`gmux_wait_complete`/
`gmux_index_read` are gated on `target_arch` only, **not** on `feature = "gmux_igd"`, and
`read_gmux_trace()` calls them on every `unaos_ivb` build. You changed the timeout from a
bounded iteration count to an `arch::ms()` deadline — and `ms()` only advances if the BSP
timer ISR is running. **The old bound could not hang; the new one can.** Gate the helpers
behind the feature, or keep an unconditional iteration cap alongside the deadline. Either
way the disarmed build must not be more fragile than trunk.

**5. A failed write must not be treated as a switch.** `gmux_index_write` failures are
logged and then ignored: a timed-out DDC write with a landed DISPLAY write leaves the panel
on IGD with DDC on discrete, and the code prints `Revert Complete` regardless. **Compare the
read-back against the intended values** and say on the wire whether it matched. Without
that, a black screen cannot be distinguished from a write that never happened, and the
experiment is unfalsifiable in both directions.

**6. `REVERT_STATE` is read-modify-written from three contexts** — the BSP timer ISR, the
pump on an AP, and the shell. `SeqCst` on the individual load and store does not make the
sequence atomic; two contexts can interleave and re-run a revert. Use a compare-exchange
loop.

## Single-use media — put this in the RUNBOOK

Nothing guards the knob across boots. `PROBED` only prevents re-entry within one boot, so
**every subsequent boot from that stick switches the mux again.** The stick must be
re-flashed after the sitting. State it where the operator will read it, not only in a
comment.

## Done gate

Plan committed before code · items 1–6 · both gates green on both arches, with the banner
checked · a RUNBOOK an operator can follow at a black panel · everything committed on
`wt/gmux-igd-x86` · and a commit body that says plainly what you did not do. **Do not title
a commit after a fix it does not contain.**
