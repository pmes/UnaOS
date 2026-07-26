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
