# line-butler: `NVIDIA` in ORIN_MARKS mis-routes 81% of an rMBP boot to `orin.log`

**Found:** 2026-08-31, rmbp 10 seat. **Tool:** `~/unaos-bench/tools/line-butler.py:28`.
**Status:** reported to orin 11 (their executor holds the file); rmbp has not edited it.

## The defect

```python
ORIN_MARKS = (b"tegra", b"Tegra", b"Jetson", b"NVIDIA")   # :28
```

The 2012 rMBP has an **NVIDIA Kepler discrete GPU**. Its driver tags every line `[NVIDIA]`.
So the bench has **two NVIDIA boards**, and a vendor name cannot discriminate them.

## Measured — real rMBP boot, `capture/rmbp9-flight5/ttyUSB0.log`, 8651 lines

```
ORIN marks on x86 output:  tegra 0 · Tegra 0 · Jetson 0 · NVIDIA 12
PI   marks on x86 output:  all five ZERO                    (pi side is clean)
first [NVIDIA]:            line 1601  "[NVIDIA] Initializing Kepler GPU at BDF 1:0:0"
lines 1601..8651:          7051 = 81% routed to orin.log and attributed to the Jetson
```

Worse than the `unknown.log` silence that cost orin 11 a night: `unknown` is honest about not
knowing. This is **confident false attribution**.

## Adding X86_MARKS does not fix it — it makes it ping-pong

Per-line test: does each `[NVIDIA]` line also carry an x86 mark
(`kepler|Kepler|igpu|ehci|x86_64|gmux|wcser`)?

| line | verdict | text |
|---|---|---|
| 1601 | SAFE | `[NVIDIA] Initializing Kepler GPU at BDF 1:0:0` (carries "Kepler") |
| 1610, 1611, 1612, 1613, 1614, 1810, 1865, 1868, 1869, 2121, 2147 | **FLIPS TO ORIN** | no x86 mark on the line |

**11 of 12 flip.** Under `if is_orin and not is_pi` each steals state until the next x86 mark, so
installing X86_MARKS without removing `NVIDIA` yields alternating attribution across the Kepler
bringup — harder to diagnose than the current single mis-attributed block.

## The fix is a DELETION and it is free

Re-derived on `capture/line-acm0/orin.log` (35370 lines) rather than relayed from orin 11:

| mark | hits | first | last |
|---|---|---|---|
| `tegra` | 4382 | 3 | 35240 |
| `Tegra` | 143 | 93 | 35238 |
| `Jetson` | 34 | 93 | 35238 |
| `NVIDIA` | 42 | 8778 | 22681 |

`NVIDIA`'s entire span is a **strict subset** of `tegra`'s, and `tegra` is 100× denser with its
first hit at line 3. **Dropping `NVIDIA` leaves zero Orin lines unattributable.**
Benefit measured: nil. Cost measured: 81% of an rMBP boot.

## The transferable rule

**A marker must name a MACHINE, not a VENDOR or a SUBSYSTEM.** `tegra`/`Tegra`/`Jetson` are
board/SoC names and all measure zero on x86. `NVIDIA` was the set's only vendor name and its only
collision. The tool's own header (`:15`) says routing is by content *"never by label, never by
claim"* — the principle is right; **a vendor name is a label wearing content's clothes.**

## X86_MARKS, if wanted — all 0 hits on both `orin.log` and `pi.log`

| mark | x86 | orin | pi | first | last | spread |
|---|---|---|---|---|---|---|
| `ehci` | 29 | 0 | 0 | 299 | 7511 | 83% |
| `igpu` | 57 | 0 | 0 | 1149 | 2209 | 12% |
| `x86_64` | 4 | 0 | 0 | 1079 | 2208 | 13% |
| `kepler` | 327 | 0 | 0 | 1615 | 2210 | 6% |
| `wcser` | 133 | 0 | 0 | 1759 | 8620 | 79% |

Rejected on evidence: `EHCI` uppercase (1 hit on orin.log — not clean; lowercase is),
`UEFI` (0 x86 / 126 orin), `Apple`, `BDF` (0 x86 / 18 orin each).

## Honest limits

- **One boot, one knob line.** Flight 5 ran `ehci-hid` hard, so `ehci`'s line-299 earliness is a
  property of that boot. A `UNAOS_SKIP_XHCI` boot may not print it. Do not let the set depend on
  one subsystem being armed — `x86_64` (1079) and `igpu` (1149) are least knob-contingent and both
  still beat the 1601 hijack.
- `orin.log`/`pi.log` are products of the broken router, so a NON-zero there needs interpretation.
  Every recommended marker measured **zero**, which is clean regardless of routing.
- No independent Pi boot corpus of my own; the pi column comes from `pi.log`.
