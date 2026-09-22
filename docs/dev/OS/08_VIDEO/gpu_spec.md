# Cleanroom GPU Specification: 2012 Retina MacBook Pro

This document specifies the hardware interface for the dual-GPU setup in the 2012 Retina MacBook Pro (board-ID: `Mac-6F01561E16C75D06`). It is intended to serve as the sole reference for implementing native driver support in UnaOS, ensuring a cleanroom design without referencing proprietary driver code.

Data in this document is sourced from:
1. Public hardware documentation (Intel PRMs, NVIDIA open-gpu-doc).
2. Open-source driver code (Nouveau, i915).
3. Metadata extracted from macOS 10.15 driver `Info.plist` files (device IDs, power thresholds).

---

## 1. Device Identification

### NVIDIA GeForce GT 650M (Kepler / GK107)
- **PCI Class**: `0x03` (Display Controller), Subclass `0x00` (VGA Compatible) or `0x80` (Other/3D)
- **Vendor ID**: `0x10DE` (NVIDIA)
- **Device ID**: `0x0FD5`
- **Architecture**: Kepler (GK1xx)

### Intel HD Graphics 4000 (Ivy Bridge / Gen7)
- **PCI Class**: `0x03` (Display Controller), Subclass `0x00` (VGA Compatible)
- **Vendor ID**: `0x8086` (Intel)
- **Device ID**: `0x0166`
- **Architecture**: Ivy Bridge (Gen7)

---

## 2. NVIDIA Kepler (GK107) Register Map

NVIDIA uses a single large MMIO region (BAR0) for all registers, typically 16MB or 32MB in size. Registers are accessed via 32-bit read/writes.

### 2.1 Master Control (PMC) - Base `0x000000`
- `0x000000` **NV_PMC_BOOT_0**: Chip identification and stepping.
  - Bits [27:20]: Chipset ID (GK107 = `0xE7`)
  - Bits [19:16]: Major revision
  - Bits [15:0]: Minor revision
- `0x000004` **NV_PMC_BOOT_1**: Additional revision info.
- `0x000200` **NV_PMC_ENABLE**: Master engine enable mask. Indicates if the GPU is initialized/POST'd.
- `0x000100` **NV_PMC_INTR_0**: Global interrupt status.
- `0x000140` **NV_PMC_INTR_EN**: Global interrupt enable. Write `0` to disable all interrupts during init.

### 2.2 Bus Control (PBUS) - Base `0x001000`
- `0x001800` **NV_PBUS_PCI_NV_0**: Mirror of PCI config space `0x00` (Vendor/Device ID).
- `0x001804` **NV_PBUS_PCI_NV_1**: Mirror of PCI config space `0x04` (Command/Status).

### 2.3 Display Engine (PDISPLAY) - Base `0x610000`
*(To be detailed in Phase 2 for Modesetting)*
- Kepler uses a sophisticated display engine supporting multiple CRTCs (heads) and output resources (SORs).
- Display heads control timings, while SORs control the physical encoders (eDP, HDMI).

### 2.4 Host / PFIFO runlist submit - Base `0x002000`

The registers `kepler.rs` uses to hand a runlist to the scheduler:

- `0x002270` **RUNLIST_BASE**: written `runlist_phys >> 12`; bits [31:28] carry the
  aperture/target (we write target = 0 = VRAM).
- `0x002274` **RUNLIST_SUBMIT**: written `engine << 20 | length`. `kepler.rs:786`
  writes the literal `3` — i.e. LEN = 3, ENG = 0. Earlier revisions wrote `1`.
- `0x002280` / `0x002284` **PLAYLIST_RD / PLAYLIST_RD_LEN**: read-only status;
  polled at `kepler.rs:789-798`.

On gk104-shaped parts the host runlist controls are usually described as an
*array* — `RUNLIST[i]` at `0x2270 + i*8` with its length word at `+4`. That
shape and the observed readbacks are in tension; §2.4.1 resolves what the
captures can and cannot decide, and §2.4.2 names the probe that closes it.

#### 2.4.1 ⭐ The bit-20 derivation (desk analysis, GR6)

**The observation.** `kepler.rs` never writes bit 20 of anything in this block,
yet `PLAYLIST_RD_LEN` always reads back with bit 20 (`0x00100000`) set.

**Every PLAYLIST capture in `KEPLER-METAL-LOG.md`, tabulated:**

| Sitting / boot | Written to `0x2274` | `PLAYLIST_RD` (`0x2280`) | `PLAYLIST_RD_LEN` (`0x2284`) | log line |
| --- | --- | --- | --- | --- |
| #4 (pull 5, `44cf4387`) | `1` (LEN=1 ENG=0) | `00002013` | `00100001` | 1436 |
| #5 (pull 8) | `1` | `00002013` | `00100001` | 1410 |
| #5 boot 2 (wall-2 fold) | `1` | `00002013` | `00100001` | 1319 |
| #5 boot 2r (`94b0ed0c`) | `1` | `00002013` | `00100001` | 1373 |
| pull-10 boot 2 | `3` (LEN=3 ENG=0) | `00002013` | `00100003` | 1268 |
| pull-13 boot 2 | `3` | `00002013` | `00100003` | 1106 |
| #23 (pull 13/20) | `3` | `00002013` | `00100003` | 716 |
| GR5 bonus line (s37 era) | `3` | `00002013` | `00100003` | 140 |

**What the table already proves.**

1. **Bits [11:0] are a faithful echo of the written length.** LEN = 1 → `…001`,
   LEN = 3 → `…003`, across four code revisions and eight boots. No exceptions.
2. **`PLAYLIST_RD` holds *our* runlist page.** The VRAM bump layout is
   inst `0x2000000` / gpfifo `0x2001000` / userd `0x2002000` / pushbuf
   `0x2003000` (64 KiB) / **runlist `0x2013000`** / fence `0x2014000` — and the
   log's own `fifo-layout userd=2002000 fence=2014000` confirms it. `0x2013` is
   `runlist_off >> 12` exactly. The scheduler is reading the buffer we gave it.
3. **Bit 20 is hardware-authored, not an echo of ours.** This is the decisive
   step. We write `engine = 0`, which puts a **0** at bit 20 of the *write*
   layout. The readback has bit 20 **set**. Therefore `0x2284` is **not** a
   field-for-field mirror of `0x2274`; whatever sets bit 20, it is the host, not us.
4. **Bit 20 is invariant under everything we have varied** — length (1 vs 3),
   PGRAPH power state (s22/s23 pulsed it), engine mask, channel-enable, bind
   state, and ~100 000 polling reads of wall time (in the LEN=3 boots the poll
   predicate `(len & 0xFFF) == 1` is unsatisfiable, so the loop ran to its bound
   and bit 20 was *still* set at the end).

**The hypotheses, and what each requires.**

| # | Hypothesis for bit 20 | Consistent with the table? | Implication if true |
| --- | --- | --- | --- |
| H-ID (strong) | `0x2284` mirrors `0x2274`; bits [23:20] are the engine/runlist id **we selected** | ❌ **REFUTED** — we wrote ENG = 0, readback shows 1 | — |
| H-ID (weak) | Bits [23:20] are the engine/runlist id the **hardware assigned**, i.e. runlist **1**, not the runlist 0 we submitted to | ✅ compatible | Our channel *is* counted against a runlist we never selected; the submit lands on runlist 0 and the scheduler files it under 1 |
| H-BUSY | Bit 20 is a per-runlist **commit-pending / BUSY** status that should self-clear when the scheduler finishes ingesting the list, and here never does | ✅ compatible | The runlist was *accepted for commit* but the commit **never retires** — which would make every "runlist accepted, as always" line in the metal log an overclaim |
| H-STICKY | Bit 20 is an unrelated always-set status/valid bit on this part | ✅ compatible | Bit 20 carries no information; the wall is elsewhere |

**Verdict on the brief's question — "is our channel being counted against a
runlist we never selected?"** *Not established, and the strong form of the
claim is refuted.* What the eight captures **do** establish is narrower and
firmer: bit 20 is written by the host, not by us, and it is invariant under
every variable we have moved so far. H-ID(weak), H-BUSY and H-STICKY are all
still standing, and **no existing capture can separate them** — every capture
reads the same one register pair after the same one submit. Any claim beyond
this is a guess.

**Consequences worth flagging regardless of which survives:**

- The acceptance poll at `kepler.rs:795` demands `(pl_rd_len & 0xFFF) == 1`.
  Since `kepler.rs:786` now writes LEN = 3, **that predicate can never be
  satisfied** — the loop always burns its full 100 000-iteration bound and the
  printed value is simply the last read. The log's "runlist accepted" readings
  rest on the *address* match (point 2 above), not on this predicate. The
  predicate is stale relative to the LEN it is checking. *Not fixed here — this
  is an analysis arc, and changing the poll changes the experiment.*
- Under H-BUSY the correct poll is "wait for bit 20 to **clear**", the exact
  opposite of waiting for it to appear.

#### 2.4.2 The discriminating probe (read-only, landed)

One read-only sweep separates the survivors. Read `0x2270 + i*8` and its `+4`
length word for `i = 1..3` — the sibling runlists under the array shape — once,
after the submit:

- **If the array shape holds and bit 20 is per-runlist BUSY/status**, the idle
  siblings read base = 0 with bit 20 **clear**. H-BUSY confirmed, H-STICKY dead.
- **If bit 20 is an id field**, sibling `i` reads its own id in bits [23:20]
  (`i << 20`). H-ID(weak) confirmed.
- **If every sibling reads bit 20 set with base = 0**, it is an unconditional
  status bit. H-STICKY confirmed, and bit 20 stops being evidence.
- **`i = 2` is a built-in cross-check**: `0x2270 + 2*8 = 0x2280`, the very pair
  we already call `PLAYLIST_RD`. If the two disagree, the array-stride
  assumption for this block is wrong.

Note the array shape is *already* under strain: `0x2280` holds our runlist page,
which we only ever wrote to `0x2270`. Either `0x2280/0x2284` are genuine
readback registers rather than array element 2, or the host mirrors. The probe
decides.

**Second purpose — a non-PGRAPH runlist.** If a sibling runlist reads as a real,
distinct, populated engine slot, a copy-engine (non-PGRAPH) runlist exists on
this part, and FIFO-level method execution becomes reachable **without** the
Falcon ucode era. That is the strategic payoff of the sweep.

The probe writes **nothing**: the pull-28 no-unproven-writes rule is in force.

---

## 3. Power Management Profiles (from AGPM)

AppleGraphicsPowerManagement dictates specific heuristics for the GT 650M (`0x0fd5`) on this board:

- **Power States**: Typically 4 states (0-3), with 0 being highest performance and 3 being deepest sleep/idle.
- **Thresholds**: State transitions are based on core/memory clock thresholds and utilization percentages.
- **Heuristic ID**: `-1` (custom Apple heuristic logic).

---

## 4. Hardware Performance Counters

GPU profiling uses specific performance counters to measure utilization and identify bottlenecks.

### Key Metrics:
- **SM Utilization (%)**: Percentage of time Streaming Multiprocessors are active.
- **TEX Utilization (%)**: Texture unit utilization. High values (>25% stalls, >128 bytes/thread) indicate texture-bound workloads.
- **ROP Utilization (%)**: Raster Operations Pipeline activity (ZROP, CROP).
- **Cache Hit Rates**: L1 and L2 cache efficiency.

---

## 5. UnaOS Driver Architecture Mapping

Based on the capabilities and requirements:

1. **Detection**: `PciScanner` matches Vendor/Device IDs during boot.
2. **Initialization**: The driver maps BAR0 (MMIO), verifies chip ID (`NV_PMC_BOOT_0`), checks POST status, and disables interrupts (`NV_PMC_INTR_EN = 0`).
3. **Display Takeover**: The driver queries the current scanout address programmed by the GOP, allocates its own `FrameBuffer`, programs the display head to use the new buffer, and enables the display.
4. **Integration**: The new `FrameBuffer` is passed to the `video` subsystem, replacing the GOP's buffer.

---

## 6. Intel Gen7 (Ivy Bridge) Display Register Map — PPS, pipe timing, pipe M/N, DP port

**Added by GMUX8, 2026-09-22.** This section exists because the iGPU display ladder in
`unaos/crates/kernel/src/drivers/gpu/igpu.rs` had, for four flights running, refused to touch the
panel-power sequencer and refused to name a mode-set register, on the stated ground that *no
citation existed in tree* (R19: **failed under: no citation**, never "ruled out"). The citation
now exists, and it is here.

### 6.1 Provenance, and the clean-room line

Every row below is transcribed from one of two public Intel documents, fetched from `intel.com`
for this pass:

| Short name used in rows | Document | Doc Ref # | URL |
| --- | --- | --- | --- |
| **V3Pt3** | Intel® OpenSource HD Graphics PRM, Volume 3 Part 3: **North Display Engine Registers** (Ivy Bridge), May 2012 Rev 1.0 | `IHD-OS-V3 Pt 3 – 05 12` | `cdrdv2-public.intel.com/690915/ivb-ihd-os-vol3-part3.pdf` |
| **V3Pt4** | Intel® OpenSource HD Graphics PRM, Volume 3 Part 4: **South Display Engine Registers** (Ivy Bridge), May 2012 Rev 1.0 | `IHD-OS-V3 Pt 4 – 05 12` | `cdrdv2-public.intel.com/690917/ivb-ihd-os-vol3-part4.pdf` |

Both page numbers and section numbers below are **as printed in the document** (in these two PDFs
the printed footer page number and the PDF page index coincide, which is why a row can be checked
by opening the PDF at the stated page).

**Neither `i915` nor `i965` was opened for any row in this section**, and the TBV bit map in
`docs/dev/GEMINI/video/iGUI/LADDER-igpu-bringup.md` rung 2 — which is sourced to i915
`intel_pps.c` *naming* and is precisely why GMUX7 refused to promote it — is **not** a source
here and was not consulted. The rows were read out of the PRM directly. §1's list of sources for
this document as a whole (which includes open-source drivers) does **not** apply to this section:
this section is PRM-only, by construction, and a row that cannot be sourced to a PRM page is not
written.

### 6.2 Panel Power Sequencer (PCH / South Display Engine) — V3Pt4 §2.4, pp.38–43

`Register Space: MMIO 0/2/0` on every row. These are the five registers the ladder reads at rung
`07b name=pps-read`, and the five a future `pps-on` write rung must program.

| Register | Offset | Access | Reset | Cite | Fields |
| --- | --- | --- | --- | --- | --- |
| `PP_STATUS` | `C7200h`–`C7203h` | **RO** | `0x08000000` | V3Pt4 §2.4.1 pp.38–39 | `[31]` Panel Power On Status — `0b` Off ("power down sequencing has completed… safe and allowed to program timing, port, and DPLL registers"), `1b` On (register write protect is active). `[30]` Require Asset Status — `0b` Not Ready / `1b` Ready; **"This bit should be ignored when using DisplayPort A"** (p.38), which is our port. `[29:28]` Power Sequence Progress — `00b` None, `01b` Power Up, `10b` Power Down, `11b` Reserved. `[27]` Power Cycle Delay Active — `1b` a T4 power-cycle delay is running. `[26:4]` Reserved |
| `PP_CONTROL` | `C7204h`–`C7207h` | **R/W** | `0x00000000` | V3Pt4 §2.4.2 pp.39–41 | `[31:16]` Write Protect Key — `0000h` Enable Write Protect, **`ABCDh` Disable Write Protect**, others Enable. `[15:4]` Reserved. `[3]` **eDP VDD Override for AUX** — `1b` "Force panel VDD on to allow AUX transaction"; the register page states the purpose outright: *to force on VDD for the embedded DisplayPort panel so AUX transactions can occur **without enabling the panel power sequence***. `[2]` Backlight Enable. `[1]` Power Down on Reset. `[0]` Power State Target — `0b` Off, `1b` On |
| `PP_ON_DELAYS` | `C7208h`–`C720Bh` | R/W **Protect** | `0x00000000` | V3Pt4 §2.4.3 pp.41–42 | `[31:30]` Panel control port select — `00b` LVDS, `01b` **DisplayPort A**, `10b` DisplayPort C, `11b` DisplayPort D. `[29]` Reserved. `[28:16]` Power up delay, **unit = the 100 µs timer**; LVDS: T1+T2; **DisplayPort: the eDP T3 value** — "the time from the source enabling panel power to when the sink HPD and AUX channel are ready". `[15:13]` Reserved. `[12:0]` Power on to backlight on, 100 µs; LVDS: T5; DisplayPort: program `1b` for the hardware minimum |
| `PP_OFF_DELAYS` | `C720Ch`–`C720Fh` | R/W **Protect** | `0x00000000` | V3Pt4 §2.4.4 p.42 | `[31:29]` Reserved. `[28:16]` Power Down delay, 100 µs; LVDS: T3; **DisplayPort: eDP T10** — source ending valid video to source disabling panel power. `[15:13]` Reserved. `[12:0]` Backlight off to power down, 100 µs; LVDS: Tx; **DisplayPort: eDP T9** — backlight disable to source ending valid video |
| `PP_DIVISOR` | `C7210h`–`C7213h` | R/W **Protect** | `0x00186904` | V3Pt4 §2.4.5 p.43 | `[31:8]` Reference divider, default `001869h` for a 125 MHz reference — "when it is desired to divide by N, the actual value to be programmed is (N/2) − 1. The value should be (100 * Ref clock frequency in MHz / 2) − 1"; zero must not be used. `[7:5]` Reserved. `[4:0]` Power Cycle Delay, **unit = the 100 ms timer**, default `4h` = 300 ms, and the page's worked example is "to achieve 400 ms, program a value of 5"; `0` selects no delay / aborts an active one; LVDS: SPWG T4; **DisplayPort: the eDP T12 value**, the shortest time from panel power disable to enable |

**⚠ Two programming notes on these pages are load-bearing and are recorded as rows in their own
right, because a write rung that misses either produces a sequencer that silently does nothing:**

1. **The `0xABCD` key is REQUIRED on DisplayPort A, not optional.** V3Pt4 p.40 and p.41 both carry
   it: *"Write Protect Key must be programmed to 0xABCD when using panel power sequencing on
   DisplayPort A (`PP_ON_DELAYS` bits 31:30 Panel Control port select is set to `01b`
   DisplayPort A)"*. This retires the standing tree claim that `0xABCD` is an **entropy pattern**
   with no semantic (`gen7.rs`'s `PCH_PP_CONTROL_KEY`): it is a documented magic value with a
   documented effect, and a machine reading `PP_CONTROL = 0xABCD0008` has had that key installed
   by firmware on purpose.
2. **`PP_CONTROL` write-protects the mode-set.** V3Pt4 p.39 lists the protected set explicitly:
   LVDS Port Control (entire register), DisplayPort Control (port enable and port-to-transcoder
   select bits), the three PP delay/divisor registers, the DPLL Control DPLL Divisors, and
   **HTOTAL, HBLANK, HSYNC, VTOTAL, VBLANK and VSYNC** — i.e. exactly §6.3's timing block. Writes
   to a protected register "will complete as normal" and will not change the register. Any future
   mode-set write rung must therefore read `PP_STATUS[31]` and the key *before* it concludes that
   a timing write landed, or it will be fooled by a silent no-op.

### 6.3 Pipe timing (CPU / North Display Engine) — V3Pt3 §4.1, pp.66–73

`Register Space: MMIO 0/2/0`, `Access: R/W`, reset `0x00000000` on every row. The PRM prints three
`Address:` lines per register (Pipe A / Pipe B / Pipe C); all three are reproduced so no stride is
inferred anywhere in this tree.

| Register | Pipe A | Pipe B | Pipe C | Cite | Fields |
| --- | --- | --- | --- | --- | --- |
| `PIPE_HTOTAL` | `60000h` | `61000h` | `62000h` | §4.1.1 pp.66–67 | `[28:16]` Horizontal Total (= active + blank, **programmed as pixels − 1**), `[11:0]` Horizontal Active (**− 1**; minimum 64 px). Must equal Horizontal Blank End / Start respectively |
| `PIPE_HBLANK` | `60004h` | `61004h` | `62004h` | §4.1.2 p.67 | `[28:16]` Horizontal Blank End, `[12:0]` Horizontal Blank Start, both **relative to horizontal active display start**; minimum blank 32 px |
| `PIPE_HSYNC` | `60008h` | `61008h` | `62008h` | §4.1.3 p.68 | `[28:16]` Horizontal Sync End = `HActive + FrontPorch + Sync − 1`, `[12:0]` Horizontal Sync Start = `HActive + FrontPorch − 1` |
| `PIPE_VTOTAL` | `6000Ch` | `6100Ch` | `6200Ch` | §4.1.4 p.69 | `[28:16]` Vertical Total (**lines − 1** progressive, **− 2** interlaced), `[11:0]` Vertical Active (**− 1**) |
| `PIPE_VBLANK` | `60010h` | `61010h` | `62010h` | §4.1.5 p.70 | `[28:16]` Vertical Blank End, `[12:0]` Vertical Blank Start; minimum vertical blank 5 lines |
| `PIPE_VSYNC` | `60014h` | `61014h` | `62014h` | §4.1.6 p.71 | `[28:16]` Vertical Sync End = `VActive + FrontPorch + Sync − 1`, `[12:0]` Vertical Sync Start = `VActive + FrontPorch − 1` |
| `PIPE_SRCSZ` | `6001Ch` | `6101Ch` | `6201Ch` | §4.1.7 p.72 | `[27:16]` Horizontal Source Size (**− 1**), `[11:0]` Vertical Source Size (**− 1**); must equal H/V Active except with panel fitting enabled. **This is the register `regs::PIPEASRC` has named since the first census — this row PINS that offset without moving it** |
| `PIPE_VSYNCSHIFT` | `60028h` | `61028h` | `62028h` | §4.1.8 p.73 | `[12:0]` Second Field VSync Shift; used **only** in interlaced modes. Cited here for completeness and **deliberately not taken into `regs`**: the ladder reads no interlaced mode and an unread constant is one more thing to keep true |

### 6.4 Pipe M/N (embedded DisplayPort and FDI) — V3Pt3 §4.2, pp.73–77

`R/W`, reset `0`, double-buffer update at start of vertical blank, **armed by writing `LINKN`**
(§4.2.4 p.77: *"Writes to this register arm M/N registers for this pipe"*) — the write rung's
ordering constraint, recorded here so it is not rediscovered on metal. The `1` set is the normal
refresh rate and the `2` set the low-power one; `PIPE_CONF[20]` selects between them (§6.5).

| Register | Pipe A (set 1 / set 2) | Pipe B | Pipe C | Cite | Fields |
| --- | --- | --- | --- | --- | --- |
| `PIPE_DATAM` | `60030h` / `60038h` | `61030h` / `61038h` | `62030h` / `62038h` | §4.2.1 pp.74–75 | `[31]` MBZ, `[30:25]` **TU Size, minus one** (default programming "111111" = TU 64), `[24]` MBZ, `[23:0]` Data M |
| `PIPE_DATAN` | `60034h` / `6003Ch` | `61034h` / `6103Ch` | `62034h` / `6203Ch` | §4.2.2 pp.75–76 | `[31:24]` MBZ, `[23:0]` Data N |
| `PIPE_LINKM` | `60040h` / `60048h` | `61040h` / `61048h` | `62040h` / `62048h` | §4.2.3 p.76 | `[31:24]` MBZ, `[23:0]` Link M — "the m value for external transmission in the Main Stream Attributes" |
| `PIPE_LINKN` | `60044h` / `6004Ch` | `61044h` / `6104Ch` | `62044h` / `6204Ch` | §4.2.4 p.77 | `[31:24]` MBZ, `[23:0]` Link N. **Writing this arms the whole M/N set for the pipe** |

**The two laws, quoted, because the ladder derives a refresh rate from them** (V3Pt3 §4.2 pp.73–74):

- `Active/TU = Payload/Capacity = Data M/N = dot clock * bytes per pixel / ls_clk * number of lanes`
- **`Link M/N = dot clock / ls_clk`** — the one rung `08b` uses, since it needs the dot clock and
  has `ls_clk` from §6.5's `DP_CTL[17:16]`.
- Restrictions on the same page: `number of lanes >= INT(dot clock * bytes per pixel / ls_clk)`
  and `Pcdclk * number of lanes >= dot clock * bytes per pixel`. The PRM also notes that what it
  calls *dot clock* the DisplayPort specification calls `strm_clk`.

### 6.5 The eDP port and the pipe control word — V3Pt3 §4.4.1 and §5.1.3

| Register | Offset | Access | Reset | Cite | Fields |
| --- | --- | --- | --- | --- | --- |
| `DP_CTL_A` (`regs::DP_A`) | `64000h`–`64003h` | R/W | `0x00000018` | §4.4.1 pp.82–86 | `[31]` DisplayPort Enable. **`[30:29]` Pipe Select — `00b` Pipe A, `01b` Pipe B, `10b` Pipe C, `11b` Reserved** (this is how the ladder learns which pipe the panel is on, rather than assuming). `[27:22]` Vswing/Emphasis (27 = AFE fullSwingMode, 26:24 = AFE sel, 23 = dmpen, 22 = dmplv; the encoding table spans pp.83–85). **`[21:19]` Port Width — `000b` x1, `001b` x2, `011b` x4, others Reserved**; locked once the port is enabled. `[18]` Enhanced Framing Enable; locked once enabled. **`[17:16]` DP PLL Frequency — `00b` 270 MHz, `01b` 162 MHz**, others Reserved. `[15]` Port reversal. `[14]` DisplayPort PLL enable (wait for PLL warm-up before setting bit 31). **`[9:8]` Link training pattern enable — `00b` Pattern 1, `01b` Pattern 2, `10b` Idle, `11b` Normal (send normal pixels)**; "when enabling the port, it must be turned on with pattern 1 enabled", and a retrain requires disable → re-enable with pattern 1. `[6]` Alternate SR Enable (eDP alternate scrambler reset). `[4:3]` Sync Polarity for the MSA. `[2]` **Digital Display Detected (RO)** — the detect pin level at boot, "valid regardless of whether the port is enabled" |
| `PIPE_CONF` (`regs::PIPExCONF`) | `70008h` / `71008h` / `72008h` | R/W | `0x00000000` | §5.1.3 pp.100–103 | `[31]` Pipe Enable ("pipe timing registers must contain valid values before this bit is enabled"). `[30]` **Pipe State (RO)** — the actual state, as distinct from the request. `[28:27]` Frame start delay (named on p.101's DRRS workaround, which gives the offsets `0x70008`/`0x71008`/`0x72008` bits 28:27 explicitly). `[25:24]` Palette/Gamma mode. `[23:21]` Interlaced Mode — `000b` PF-PD progressive, `001b` PF-ID, `011b` IF-ID, others Reserved. **`[20]` Display Power Mode Switch (software-controlled DRRS) — `0b` Normal uses link and data M/N **1** values and FP0, `1b` Low Power uses M/N **2** and FP1**. `[19:18]` MSA Timing Delay. `[15:14]` Rotation info (informative only). `[13]` Color Range Select. `[12:11]` Pipe output colour space — `00b` RGB, `01b` YUV 601, `10b` YUV 709. `[10]` xcYCC range limit. `[8]` BFI enable. **`[7:5]` Bits Per Color — `000b` 8 bpc, `001b` 10 bpc, `010b` 6 bpc, `011b` 12 bpc** (note the ordering is not monotonic). `[4]` Dithering enable. `[3:2]` Dithering type — `00b` Spatial, `10b` ST2. `[1:0]` MBZ |

### 6.6 ⚠ NOT-IN-IVB-PRM — three registers a Haswell-shaped mode-set would look for and this part does not have

`DP_TP_CTL`, `DP_TP_STATUS` and `TRANS_DDI_FUNC_CTL` occur **zero times** in either Ivy Bridge
display volume (V3Pt3 North, V3Pt4 South; searched as whole-document text). They are Haswell
DDI-era registers. On Ivy Bridge:

- **link training is `DP_CTL_A[9:8]`** (§6.5 above, V3Pt3 §4.4.1 pp.85–86) — that *is* this part's
  `DP_TP_CTL`, and there is no separate status register for it;
- **the eDP port is CPU-attached**, so it is driven straight off the §6.3 pipe-timing block with
  **no PCH transcoder in the path**. The PCH transcoder registers exist, in V3Pt4, for
  PCH-attached ports; they are not part of an eDP mode-set on `DP_A` and a write rung must not go
  looking for one.

This is the R19 distinction stated precisely: these three are not *failed under: no citation* —
they are **absent from the silicon's own public specification**, which is a stronger and different
finding, and the ladder prints them under the separate token `NOT-IN-IVB-PRM` for exactly that
reason.
