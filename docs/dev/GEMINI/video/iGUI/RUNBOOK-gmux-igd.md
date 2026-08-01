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
revert is issued by the same function, on the same call stack, a few instructions after
the switch. There is no task to fail to spawn and no interrupt hook to be missing. The
only ways it does not run are a hard hang or a triple fault inside the dwell.

**A related claim is deliberately NOT made here.** Whether the input chain — EHCI-HID
through `handle_key` — is still alive with the mux switched away has **not been
verified**, on metal or anywhere else. Because this build has no verb to type, nothing
depends on that claim. Do not treat the internal keyboard as a recovery path.

Also note this rig's serial console is **kernel-TX-only**. The kernel talks; you cannot
type back over the wire. Serial is an instrument, not a console.

---

## If the panel does not come back

In order:

1. **Wait to 30 seconds.** The dwell is bounded twice — by a millisecond deadline and
   by an iteration cap that depends on no clock — but if `arch::ms()` has stopped, the
   iteration cap governs and its wall-clock length is not yet known (this is exactly
   what the `iters=` field in the log exists to measure). It may simply be longer.
2. **Read the serial capture.** It will say which of these happened. Use `awk`, never
   `grep` — control bytes in the capture break grep:
   ```
   awk '/\[GMUX\]/' <capture>
   ```
3. **Power cycle.** Hold the power button. There is nothing to type and nothing to
   press; the mux state does not survive a power cycle, so the machine comes back on the
   discrete GPU regardless of what the kernel did.
4. **Pull the stick before the next boot**, or the next boot repeats the whole thing
   (see the single-use warning above).

**Do not** re-insert and re-boot the armed stick "to see if it clears". It will not
clear; it will switch the mux again.

---

## What to capture, and how to read it

Arm the serial capture **before** boot. The whole experiment is ten or so lines and they
all carry the tag `[GMUX]`:

```
awk '/\[GMUX\]/' ~/unaos-bench/capture/<session>/ttyUSB1.log
```

A successful run reads roughly:

```
:: igpu: PROTOCOL PROVEN (version plausible)
:: igpu: [GMUX] pre-switch state: DDC=0x02 DISP=0x03 EXT=0x03
:: igpu: [GMUX] ARMED synchronous revert: dwell=10000ms deadline_ms=...
:: igpu: [GMUX] the panel is EXPECTED to go black now ...
:: igpu: [GMUX] switch write: ddc=ok disp=ok ext=ok (intent DDC=0x01 DISP=0x02 EXT=0x02)
:: igpu: [GMUX] switch read-back: DDC=0x01 DISP=0x02 EXT=0x02
:: igpu: [GMUX] switch verdict: MATCH (all three registers read back as written)
:: igpu: [GMUX] dwell ended by=deadline elapsed_ms=10001 iters=...
:: igpu: [GMUX] reverting to pre-switch state DDC=0x02 DISP=0x03 EXT=0x03
:: igpu: [GMUX] revert write: ddc=ok disp=ok ext=ok ...
:: igpu: [GMUX] revert read-back: DDC=0x02 DISP=0x03 EXT=0x03
:: igpu: [GMUX] revert verdict: MATCH (all three registers read back as written)
:: igpu: [GMUX] SUMMARY: switch=MATCH revert=MATCH — the mux is back on the pre-switch (discrete) state
```

| Line you see | What it means | What to do |
|---|---|---|
| `SUMMARY: switch=MATCH revert=MATCH` | The whole experiment succeeded. The mux write lands; a future arc can configure pipes. | Nothing. Pull the stick. |
| `REFUSED: pre-switch read timed out` | The gmux did not answer. **No write was issued** and the mux was never touched. | Safe. Investigate the handshake, not the panel. |
| `PROTOCOL UNPROVEN` and no `[GMUX]` lines after it | The version tuple was implausible, so the switch was correctly refused. | Safe. No write was issued. |
| `switch verdict: MISMATCH` | The mux is in an unknown or partial state. The revert still runs. | Read which register disagreed. If `SUMMARY` then says `revert=MATCH`, the machine is fine. |
| `SUMMARY: ... revert=FAILED` | The mux was **not proven** back. | Power cycle. Report the whole `[GMUX]` block. |
| `dwell ended by=itercap` | The ms clock stopped advancing during the dwell. The revert still ran. | Report it — this is a real finding about the timer, not about the mux. |
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
