STATUS: APPROVED WITH AMENDMENTS (2026-07-22 — the two refutations are the
best part: 0x616100=HEAD_CAP (RO capabilities) and 0x490 unmapped in nv_evo
both explain the sitting-5 zeros with citations. Module split accepted as
milestone 1. Amendments:
A1 — EVO offsets (0x400 OFFSET_ORIGIN, 0x80 UPDATE, 0x84 PRESENT_CTRL) are
push-buffer METHODS, not MMIO addresses: "probe the MMIO mirror" needs the
CITED mirror base where GF119- armed/assembly method state is readable. If
the cleanroom sources give one, cite it; if ambiguous, dump BOTH candidate
mirror bases as separate labeled rows (read-only) so a zero row refutes a
mirror hypothesis instead of the whole decode — we are not burning another
boot on an uncited base address.
A2 — HEAD_STAT (0x616000) rows ride along as proposed; keep the trace array
sized for all rows and sentinel any fallible read.
ABI law for pdisp_trace_0; `:: kdisp:` prefix; full-knob land-review with
strings-proof both artifacts; arch gate untouched. Metal owed: sitting #11.)
Note: submitted as PROPOSAL-video-pull6 before the display-lane renumber;
this file name stands. Successors use kepler-display-pull<N>.

# Kepler Pull 6: Scanout Re-derivation & Display Refactor

## Objective
Transition to the Kepler display engine (GF119-family) re-derivation. The primary goals are to split the monolithic `kepler.rs` module, cleanly re-derive the scanout surface address from `envytools/rnndb` facts (refuting old guesses), and establish read-only boot-time trace points for the PDISPLAY engine.

## 1. Mechanical Milestone 1: Module Split
We will first split `unaos/crates/kernel/src/drivers/gpu/kepler.rs`.
- Extract all display-specific logic (surface copying, EVO channel control, display initialization) into a new module: `kepler_display.rs`.
- Keep the core GPU initialization (BAR mapping, device discovery) in `kepler.rs`.
- Ensure clean module boundaries and un-broken builds.

## 2. Refuting Pull-4/5-Era Guesses
Our prior attempts to read the active scanout address relied on undocumented offsets `0x616100` and the EVO core channel `0x610490`. Both returned zeros or static values because they were not the correct scanout registers.

### Why 0x616100 failed:
According to `g80_pdisplay.xml` (line 651), the `0x6100` block inside the `PDISPLAY` array (`0x610000` base) is `HEAD_CAP` (Head Capabilities).
```xml
<stripe offset="0x6100" name="HEAD_CAP" stride="0x800" length="2">
```
This register only stores read-only hardware capabilities (like `DP_INTERLACE` support). Reading this block yielded a static capability bitmask (or zeros for unsupported bits), completely unrelated to the dynamic scanout surface address.

### Why EVO 0x610490 failed:
We previously guessed `0x610490` as the EVO core channel control. However, reviewing `nv_evo.xml`, there is no display configuration method mapped at `0x490`. 
The true offset for the surface address configuration is defined in `nv_evo.xml` under the `G80_EVO_FB_SETTINGS` group (offset `0x0` as `OFFSET_ORIGIN`). For `GF119-` (Kepler), this group is mapped into the `NV_EVO_BASE` (Base Channel) domain at offset `0x400` (line 155):
```xml
<stripe offset="0x400" variants="GF119-">
	<use-group name="G80_EVO_FB_SETTINGS" />
</stripe>
```
Thus, the method for configuring the scanout origin is actually `0x400`, not `0x490`. Reading `0x610490` targeted an unmapped or unrelated channel control space, returning zeros.

## 3. Re-derivation of GK107 (GF119+) Scanout State
Using the `rnndb` facts, we will implement read-only tracing for the true NVD0 display state:
- **Scanout Surface Address (`OFFSET_ORIGIN`)**: Derived from `NV_EVO_BASE` offset `0x400`. We will probe the MMIO mirror for this base channel method.
- **Active Head State & Enable Bits**: Derived from the `UPDATE` method (offset `0x80`) and `PRESENT_CTRL` (offset `0x84`) in `NV_EVO_BASE`.
- **Head Status**: We will also trace `HEAD_STAT` at `0x616000` (stride `0x800`) to observe `REPORT_UNDERFLOW` and vertical blanks (`VERT`), validating active head timing.

## 4. Boot-Time Trace Points
Mirroring the iGPU pattern, we will:
1. Extend `unaos/crates/boot-info/src/lib.rs` with a `pdisp_trace_0` array (e.g., `[u32; 7]`).
2. In `kepler_display.rs`, before taking over the GOP framebuffer, read the true scanout registers (and `HEAD_STAT`) and populate the trace array.
3. Implement `:: kdisp:` log prefixes to print the decoded trace array during early boot, proving we can observe the hardware's active state without writing to it (Read-only).
