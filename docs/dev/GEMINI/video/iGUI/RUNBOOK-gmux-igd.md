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
5. The mux is switched to the integrated GPU **for DISPLAY, EXTERNAL, and DDC** (`GMUX_DISPLAY_IGD`, `GMUX_EXTERNAL_IGD`, `GMUX_DDC_IGD`).
6. **THE PANEL WILL BLANK OR FLICKER.** We are moving the DISPLAY mux to route the AUX channel. This is an expected, by-design blanking window.
7. We inherit the AUX clock divider, read DPCD, and read the 128-byte EDID over the I2C-over-AUX protocol.
8. The EDID hex dump is printed to serial — **after** the restore, not during the dark window.
   Nothing at all is printed between the first gmux write and the restore; every result is
   buffered so that a failure leaves a readable capture instead of a hang behind a black screen.
9. The `DisplayUnwind` stack forcefully restores the muxes from CONSTANTS, LIFO: `DDC`, then
   `DISPLAY`, then `EXTERNAL` — the reverse of the forward write order, matching upstream
   `apple-gmux`. It never re-reads the live mux to decide what to write back: a timed-out gmux
   read returns `0xFFFFFFFF`, which truncates to `0xFF` and would leave a dark panel.
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

1. **Wait 5 seconds.** The expected blind window is under 2 seconds. If the panel blanks and stays blank longer than 5 seconds, the hardware assumption was wrong or the parachute failed.
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
:: igpu-dpy: pre-switch state DDC=0x02 SW_DISP=0x03 SW_EXT=0x21 DISP=0x03 EXT=0x21 sw_ext_state=kepler-owned ext_state=kepler-owned
```

**The EXTERNAL pre-state has two accepted values, and the flight restores the one it found.**

`EXT=0x21` is the **Boot AK metal norm** — what this machine actually reads when the firmware
leaves the port Kepler-owned. `EXT=0x03` (fully-DIS) is equally accepted. The gate validates
EXTERNAL against exactly those two named constants and the unwind writes back **the member it
validated**, never a blanket DIS: forcing a Kepler-owned port to DIS is not a restore, it is a
silent state change. `sw_ext_state=` / `ext_state=` on the pre-switch line name which one was
found, and the `EXTERNAL restored to ...` line after the revert names which one was written back.

`DDC` and `DISPLAY` stay strict DIS — every capture the tree records reads `DDC=0x02 DISP=0x03`,
so there is no second legitimate value to admit for either.

Anything outside the accepted set — including the `0xFFFFFFFF` gmux-timeout sentinel, which is
neither constant — REFUSES with `why=pre-switch-not-accepted` **before any mux is touched**: panel
untouched, boot continues. Safe, but the flight did not fly. The refusal line and the `pre-switch
state` line together name the value that blocked it.

```
:: igpu-dpy: rung=00 name=census ok=1 bdsm=... ggc=... ggtt0=... ggtt1=... aux_ctl=... frmcnt=...
:: igpu: [GMUX] running Unwind stack self-test
:: igpu: [GMUX] Unwind stack MMIO self-test passed
:: igpu: [GMUX] Unwind stack gmux-dispatch=REACHED (Gmux restore path executed without faulting, not implying restore verified)
:: igpu: [AUX] PRE-SWITCH DPCD Read Failed: ... (or PRE-SWITCH DPCD REV: ...)
:: igpu: [GMUX] switched DISPLAY, EXTERNAL, and DDC to IGD (panel BLANKED/FLICKERED)
:: igpu: [AUX] PCH_PP_STATUS/CONTROL Before AUX: STATUS=... CONTROL=...
:: igpu: [AUX] PCH_PP_STATUS/CONTROL After AUX:  STATUS=... CONTROL=...
:: igpu: [AUX] DPCD REV: 0x11
:: igpu: [AUX] EDID Dump
:: igpu: [AUX] 00: 00 FF FF FF FF FF FF 00 ...
...
:: igpu: [GMUX] revert read-back: DDC=0x02 SWITCH_DISP=0x03 READ_DISP=0x03 SWITCH_EXT=0x03 READ_EXT=0x03 (TBV)
:: igpu-dpy: LADDER highest=05/10 name=end ok=1 pending=1 gmux=MATCH why=none elapsed_ms=...
```

| Line you see | What it means | What to do |
|---|---|---|
| `LADDER highest=05/10 name=end ok=1` | The whole experiment succeeded. The mux write lands; EDID was read. | Nothing. Pull the stick. |
| `LADDER ... why=edid-header-corrupt` | The EDID was read but lacks the valid 8-byte header. | Safe. Pull the stick (unless `gmux=FAILED` co-fired, which demands a power cycle). |
| `LADDER ... why=edid-checksum-bad` | The EDID was read but its checksum failed. | Safe. Pull the stick (unless `gmux=FAILED` co-fired, which demands a power cycle). |
| `REFUSED: pre-switch-not-accepted` | A pre-switch read was outside the accepted set: `DDC`/`DISPLAY` must be DIS, `EXTERNAL` must be DIS **or** Kepler-owned `0x21`. Also fires on the `0xFFFFFFFF` timeout sentinel. **No write was issued**, the panel never blanked. | Safe. Report the `pre-switch state` line verbatim — `sw_ext_state=UNACCEPTED` / `ext_state=UNACCEPTED` names the culprit. |
| `EXTERNAL restored to kepler-owned` | The port was Kepler-owned going in and was put back Kepler-owned. **`READ_EXT=0x21` afterwards is CORRECT, not a failure.** | Nothing. |
| `PRE-SWITCH DPCD REV: 0x..` | **The positive control answered BEFORE any mux moved.** AUX already reaches the panel without the switch; the flight's question is answered by this line alone. | Note it. This is the headline result either way. |
| `PRE-SWITCH DPCD Read Failed: ...` | The control did not answer pre-switch. Only now does the post-switch attempt mean anything: if the post-switch read succeeds, the muxes carry AUX. | Note it. Compare against the post-switch `DPCD REV` line. |
| `why=aux-short-read` | **KNOWN AND ACCEPTED — not a defect.** A legal partial I2C reply; upstream i915 clamps here instead of erroring. Seeing it is itself proof that AUX *answered*. | Nothing. Record the line. |
| `REFUSED: aux_ctl=... SEND_BUSY is set at boot` | AUX channel busy. **No write was issued**. `bdsm`/`ggc`/`ggtt0`/`ggtt1` show if memory is mapped; `frmcnt` shows if the pipe is running. | Safe. Power cycle and try again. |
| `REFUSED: aux_ctl=... clock divider is 0` | AUX clock missing. **No write was issued**. `bdsm`/`ggc`/`ggtt0`/`ggtt1` show if memory is mapped; `frmcnt` shows if the pipe is running. | Safe. Power cycle and try again. |
| `LADDER ... gmux=FAILED` | The mux was **not proven** back. | Power cycle. Report the whole `[GMUX]` block. |
| `LADDER ... gmux=UNTOUCHED` | The harness aborted before switching the mux; the DDC channel was never moved to the IGD. | Safe. Power cycle if needed or pull stick. |
| No `[GMUX]` lines at all on an armed build | The probe never reached the arm, or the build was not actually armed. | Check the boot banner really ends `...,unaos_ivb,gmux_igd`. |

---

## Before you start — checklist

- [ ] Serial capture armed and **growing** (growth, not a process ID, is what proves a
      watcher is alive).
- [ ] You are ready to OBSERVE and RECORD whether the panel actually blanks/flickers.
- [ ] The stick you are about to boot is the armed one, and you know which it is.
- [ ] You accept that this stick must be re-flashed or removed afterwards.
- [ ] You know the panel WILL BLANK OR FLICKER by design, and there is no dwell.
