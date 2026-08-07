# RUNBOOK — GMUX to IGD, and what to do at a black panel

Operator procedure for a boot of the `UNAOS_GMUX_IGD=1` build on the 2012 Retina
MacBookPro bench machine. Read the whole page before inserting the stick.

---

## ⚠ THE MEDIA IS SINGLE-USE. RE-FLASH IT AFTER THE SITTING.

Nothing in the kernel guards this knob across boots. The in-boot guard (`PROBED`) only
stops the probe running twice **within one boot**. It is not persistent state.

**Every subsequent boot from an armed stick switches the display mux again**, and stalls
for another 10 seconds with the panel dark, whether or not you wanted an experiment that
time. If that stick is left in the machine and the machine is rebooted for an unrelated
reason, you will get the black panel again with no warning.

After the sitting: re-flash the stick with a normal build, or pull it out and label it.
**Note: gmux_igd media is a special flight, not regression media! The boot time cost is ~10s.**

---

## What is going to happen, in order

1. Boot proceeds normally to the iGPU probe inside `pci::init` (after SMP, after the
   APIC timer is calibrated, **before** xHCI enumeration).
2. The gmux protocol check runs. If it reports `PROTOCOL UNPROVEN`, **nothing is
   written** and the boot continues — that is a valid, safe outcome.
3. On `PROTOCOL PROVEN`, the pre-switch mux state is read and saved.
4. The mux is switched to the integrated GPU.
5. **THE PANEL GOES BLACK. THIS IS THE EXPECTED RESULT.** It is not a crash and it is
   not the experiment failing. Every iGPU pipe, plane and PLL on this machine reads
   zero, so nothing on the integrated side is driving the panel — pointing the mux at
   IGD points it at an unconfigured display engine. A later arc configures pipes; this
   one only proves the mux write lands.
6. **The machine sits with a dark panel for about 10 seconds.** Boot is genuinely
   stalled during this window — that is by design, because the code holding the mux on
   IGD is the same code that will put it back. Do not touch anything.
7. The mux is reverted to the saved pre-switch state.
8. **The panel comes back**, and boot continues into xHCI enumeration and the GUI.

Total added time: ~10 s. If the panel is dark for **substantially longer than 15
seconds**, treat it as the failure case below.

---

## Recovery is AUTOMATIC. There is no key to press.

**There is no `gmux-revert` shell verb in this build.** The seam that would add one
lives in `shell.rs`, which is outside the lane that produced this code, so it was not
written. Do not go looking for a verb to type; there isn't one, and this page will not
tell you to type something that does nothing.

This is not a gap in the recovery — it is why the recovery was built the way it is. The
revert is issued by the same function, on the same call stack, as part of the same sequence
as the switch. There is no task to fail to spawn and no interrupt hook to be missing.

**A related claim is deliberately NOT made here.** Whether the input chain — EHCI-HID
through `handle_key` — is still alive with the mux switched away has **not been
verified**, on metal or anywhere else. Because this build has no verb to type, nothing
depends on that claim. Do not treat the internal keyboard as a recovery path.

Also note this rig's serial console is **kernel-TX-only**. The kernel talks; you cannot
type back over the wire. Serial is an instrument, not a console.

---

## If the panel does not come back

In order:

1. **Wait to 30 seconds.** The dwell is bounded twice — by a TSC deadline and
   by an iteration cap that depends on no clock — but if the TSC has stopped, the
   iteration cap governs and its wall-clock length is not yet known (this is exactly
   what the `iters=` field in the log exists to measure). It may simply be longer.
2. **Read the serial capture.** It will say which of these happened. Use `awk`, never
   `grep` — control bytes in the capture break grep:
   ```
   awk '/\[GMUX\]|igpu-dpy:/' <capture>
   ```
3. **Power cycle.** Hold the power button. There is nothing to type and nothing to
   press; the mux state does not survive a power cycle (asserted-not-verified), so the machine comes back on the
   discrete GPU regardless of what the kernel did.
4. **Pull the stick before the next boot**, or the next boot repeats the whole thing
   (see the single-use warning above).

**Do not** re-insert and re-boot the armed stick "to see if it clears". It will not
clear; it will switch the mux again.

---

## What to capture, and how to read it

Arm the serial capture **before** boot. The whole experiment is ten or so lines and they
all carry the tag `[GMUX]` or `igpu-dpy:`:

```
awk '/\[GMUX\]|igpu-dpy:/' ~/unaos-bench/capture/<session>/ttyUSB1.log
```

A successful run reads roughly:

```
:: igpu: PROTOCOL PROVEN (version plausible)
:: igpu-dpy: pre-switch state DDC=0x02 DISP=0x03 EXT=0x03
:: igpu-dpy: rung=00 name=census ok=1 bdsm=... ggc=... ggtt0=... ggtt1=... aux_ctl=... frmcnt=...
:: igpu: [GMUX] the panel is EXPECTED to go black now — switching to IGD
:: igpu: [GMUX] switch write: ddc=ok disp=ok ext=ok ...
:: igpu: [GMUX] switch read-back: DDC=0x01 DISP=0x02 EXT=0x02
:: igpu: [GMUX] switch verdict: MATCH (all three registers read back as written)
:: igpu-dpy: rung=00 name=mux ok=1 ddc=0x01 disp=0x02 ext=0x02 verdict=MATCH
:: igpu: [GMUX] switch successful (10s dwell expected)
:: igpu: [GMUX] beginning success-path dwell
:: igpu: [GMUX] dwell ended by=deadline elapsed_ms=10001 iters=...
:: igpu: [GMUX] reverting to pre-switch state DDC=0x02 DISP=0x03 EXT=0x03
:: igpu: [GMUX] revert write: ddc=ok disp=ok ext=ok ...
:: igpu: [GMUX] revert read-back: DDC=0x02 DISP=0x03 EXT=0x03
:: igpu: [GMUX] revert verdict: MATCH (all three registers read back as written)
:: igpu: [GMUX] success-path dwell finished, reverted=true
:: igpu-dpy: LADDER highest=00/10 name=harness ok=1 unwound=0 gmux=MATCH why=none elapsed_ms=10020
```

| Line you see | What it means | What to do |
|---|---|---|
| `LADDER highest=00/10 name=harness ok=1` | The whole experiment succeeded. The mux write lands; a future arc can configure pipes. | Nothing. Pull the stick. |
| `REFUSED: pre-switch state is not fully DIS` | The gmux was not in the expected discrete state. **No write was issued**. | Safe. Power cycle and try again. |
| `rung=00 name=mux ok=0` | The mux is in an unknown or partial state. The revert still runs. | Read which register disagreed. If `LADDER` then says `gmux=MATCH`, the machine is fine. |
| `LADDER ... gmux=FAILED` | The mux was **not proven** back. | Power cycle. Report the whole `[GMUX]` block. |
| `dwell ended by=itercap` | The TSC clock stopped advancing during the dwell. The revert still ran. | Report it. |
| No `[GMUX]` lines at all on an armed build | The probe never reached the arm, or the build was not actually armed. | Check the boot banner really ends `...,unaos_ivb,gmux_igd`. |

The `iters=` number on the dwell line is worth recording on the first successful boot:
it is the only thing that will ever tell us how long the iteration-cap backstop actually
runs in wall-clock terms.

---

## Before you start — checklist

- [ ] Serial capture armed and **growing** (growth, not a process ID, is what proves a
      watcher is alive).
- [ ] The stick you are about to boot is the armed one, and you know which it is.
- [ ] You accept that this stick must be re-flashed or removed afterwards.
- [ ] You know the panel will be dark for ~10 s and that this is the expected result.
