# RUNBOOK — GMUX to IGD, and what to expect

Operator procedure for a boot of the `UNAOS_GMUX_IGD=1` build on the 2012 Retina
MacBookPro bench machine. Read the whole page before inserting the stick.

---

## ⚠ THE MEDIA IS SINGLE-USE. RE-FLASH IT AFTER THE SITTING.

Nothing in the kernel guards this knob across boots. The in-boot guard (`PROBED`) only
stops the probe running twice **within one boot**. It is not persistent state.

**Every subsequent boot from an armed stick switches the display mux again**, whether or not you wanted an experiment that
time. If that stick is left in the machine and the machine is rebooted for an unrelated
reason, you will get the experiment running again with no warning.

After the sitting: re-flash the stick with a normal build, or pull it out and label it.
**Note: gmux_igd media is a special flight, not regression media!**

---

## What is going to happen, in order

1. Boot proceeds normally to the iGPU probe inside `pci::init` (after SMP, after the
   APIC timer is calibrated, **before** xHCI enumeration).
2. The gmux protocol check runs. If it reports `PROTOCOL UNPROVEN`, **nothing is
   written** and the boot continues — that is a valid, safe outcome.
3. On `PROTOCOL PROVEN`, the pre-switch mux state is read and saved.
4. **Flight 1b Specific:** We run the Unwind Stack self-test. If it fails, we abort.
5. The mux is switched to the integrated GPU **for DDC ONLY** (`GMUX_SWITCH_DDC = 0x1`).
6. **THE PANEL SHOULD REMAIN ON.** Since we are only switching the DDC/AUX channel and leaving the DISPLAY channel on the discrete GPU, the screen should not go black. If the panel *does* blank, it proves our assumption about DDC-only switching is wrong, and this flight has 1c's risk profile.
7. We inherit the AUX clock divider, read DPCD, and read the 128-byte EDID over the I2C-over-AUX protocol.
8. The EDID hex dump is printed to serial.
9. The `DisplayUnwind` stack forcefully reverts the DDC switch back to the guarded pre-switch DDC state (`pre_ddc.unwrap()`).
10. Boot continues into xHCI enumeration and the GUI.

Total added time: minimal. If the panel goes dark and stays dark, treat it as the failure case below.

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

## If the panel goes black and does not come back

In order:

1. **Wait briefly.** The experiment should be nearly instantaneous. If the panel blanks and stays blank, the hardware assumption was wrong or the parachute failed.
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

A successful run reads roughly (PREDICTED TRANSCRIPT):

```
:: igpu: PROTOCOL PROVEN (version plausible)
:: igpu-dpy: pre-switch state DDC=0x02 DISP=0x03 EXT=0x03
:: igpu-dpy: rung=00 name=census ok=0 bdsm=... ggc=... ggtt0=... ggtt1=... aux_ctl=... frmcnt=...
:: igpu: [GMUX] running Unwind stack self-test
:: igpu: [GMUX] Unwind stack MMIO self-test passed
:: igpu: [GMUX] Unwind stack gmux-dispatch=REACHED (Gmux restore path executed without faulting, not implying restore verified)
:: igpu: [GMUX] switching DDC to IGD (0x01) — panel should REMAIN ON since DISPLAY is not moved
:: igpu: [AUX] DPCD REV: 0x11
:: igpu: [AUX] EDID Dump
:: igpu: [AUX] 00: 00 FF FF FF FF FF FF 00 ...
...
:: igpu: [GMUX] revert read-back: DDC=0x02 DISP=0x03 (TBV) EXT=0x03 (TBV)
:: igpu-dpy: LADDER highest=05/10 name=edid ok=1 unwound=2 gmux=MATCH why=none elapsed_ms=...
```

| Line you see | What it means | What to do |
|---|---|---|
| `LADDER highest=05/10 name=edid ok=1` | The whole experiment succeeded. The mux write lands; EDID was read. | Nothing. Pull the stick. |
| `REFUSED: pre-switch state is not fully DIS` | The gmux was not in the expected discrete state. **No write was issued**. | Safe. Power cycle and try again. |
| `LADDER ... gmux=FAILED` | The mux was **not proven** back. | Power cycle. Report the whole `[GMUX]` block. |
| `LADDER ... gmux=UNTOUCHED` | The harness aborted before switching, touching no registers and attempting no revert. | Safe. Power cycle if needed or pull stick. |
| No `[GMUX]` lines at all on an armed build | The probe never reached the arm, or the build was not actually armed. | Check the boot banner really ends `...,unaos_ivb,gmux_igd`. |

---

## Before you start — checklist

- [ ] Serial capture armed and **growing** (growth, not a process ID, is what proves a
      watcher is alive).
- [ ] The stick you are about to boot is the armed one, and you know which it is.
- [ ] You accept that this stick must be re-flashed or removed afterwards.
- [ ] You know the panel should remain on, and there is no 10-second dwell.
