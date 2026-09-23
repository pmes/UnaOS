// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! KVBLANK — the Kepler head's VBLANK EDGE, counted, timed and phased, and the wait `video/beam.rs`
//! takes instead of spinning on the raster position.
//!
//! ## The defect this rung answers (B135 §7, VUGPERF, fold `4a283725`)
//!
//! VUGPERF decomposed flight 11's 139.75 ms composite pass and found the residual charged to
//! neither half of the clock — `blit_us - compose_us - present_us` = 46 488 us per pass — is
//! `video::beam::hold` SPINNING on an MMIO read. The census: `[wc-h] win=8 beamwaits=4336
//! beamwait_us=10766899` (2.48 ms mean per hold, against a 16.667 ms frame), `win=3
//! beamwait_us=49113217` over 13971 waits, `win=2 beammaxwait_us=62007`. The beam wait alone
//! exceeds the presenter's entire 500 us budget by 5x, and it is spent re-reading ONE register.
//!
//! A display engine has a vblank for exactly this. Two rungs:
//!
//! * **RUNG 1 `kvblank-measure`** (this module's [`note`] / [`arm`] / [`arm_pmc_pdisplay`]) — count
//!   the edges, timestamp each one, and publish the PHASE at which the edge is observed. No
//!   compositor change; the compositor's own `scanout_beam` reads feed it.
//! * **RUNG 2 `kvblank-wait`** ([`wait_next_edge`], called from `video::beam::hold`) — when the
//!   hazard zone is WIDE and rung 1's measured phase is OUTSIDE it, wait for the next edge on a
//!   counter compare instead of spinning on the raster, then confirm with ONE raster read.
//!
//! ## THE REGISTERS, and their cleanroom provenance
//!
//! Sources of record, as `kepler_display.rs` already names them: envytools rnndb
//! `display/g80_pdisplay.xml` and NVIDIA open-gpu-doc (`dev_master` for PMC). No nouveau code was
//! read or transcribed. Every register below is named with its bit and its document; where this
//! tree has NO cited offset, the register is printed as `NOT-IN-TREE` and NOTHING is read at a
//! guessed address (the `igpu-dpy: … NOT-IN-TREE` idiom, `igpu.rs:1731`).
//!
//! | register | offset | bits | citation | class |
//! |---|---|---|---|---|
//! | `NV_PMC_INTR_0` | BAR0 `0x000100` | `[26]` = PDISPLAY | open-gpu-doc `dev_master`; envytools `docs/hw/bus/pmc.txt` NV50+ source table | offset **[TREE]** (`kepler.rs:9`, `gpu_spec.md` §2.1); **bit 26 is [EXT] and UNVERIFIED on GK107** |
//! | `NV_PMC_INTR_EN` | BAR0 `0x000140` | `[26]` = PDISPLAY | same | same |
//! | `HEAD_STAT` | PDISPLAY `0x6000`, stride `0x800`, 4 heads (GK104-) | — | rnndb `display/g80_pdisplay.xml:647` | **[TREE]** (`kepler_display.rs` BEAMX86) |
//! | `HEAD_STAT.VERT` | `HEAD_STAT + 0x340` | `vline[15:0]`, **`vblank_count[31:16]`** | same | **[TREE]**, and BEHAVIOURALLY validated every boot by `beam_probe`'s `vbd >= 2` test |
//! | ~~PDISPLAY per-head vblank interrupt ENABLE / STATUS~~ | — | — | — | **WAS `NOT-IN-TREE`. RESOLVED BY KVBLANK2 — see the block below and the six rows under it.** |
//! | `DISP.INTR_HEAD_STATUS` | PDISPLAY `+0x074 + head*0x800`, 4 heads | `[0]` = `VBLANK` | rnndb `display/g80_pdisplay.xml:523` (register), `:489-490` (bitset `gf119_pdisplay_intr_head`, `pos="0" name="VBLANK"`) | **[EXT], GF119- STRIPE** |
//! | `DISP.INTR_HEAD_TRIGGER` | PDISPLAY `+0x078 + head*0x800`, 4 heads | same bitset | rnndb `g80_pdisplay.xml:524` | **[EXT]** — named for completeness; KVBLANK2 NEVER writes it |
//! | `DISP.INTR_HOST_HEAD` | PDISPLAY `+0x0BC + head*0x800`, 4 heads | `[0]` = `VBLANK` | rnndb `g80_pdisplay.xml:541` | **[EXT]** — the HOST-directed per-head STATUS |
//! | `DISP.INTR_HOST_HEAD_EN` | PDISPLAY `+0x0C0 + head*0x800`, 4 heads | `[0]` = `VBLANK` | rnndb `g80_pdisplay.xml:542` | **[EXT]** — **THE ENABLE.** The one register KVBLANK2 rung 2 writes |
//! | `DISP.INTR_HOST_HEAD_DISPATCH` | PDISPLAY `+0x0C8 + head*0x800`, 4 heads | same bitset | rnndb `g80_pdisplay.xml:544` | **[EXT]** — read-only in this rung |
//! | `DISP.INTR_SUMMARY` / `DISP.INTR_HOST_SUMMARY` | PDISPLAY `+0x058` / `+0x088` | `[24..27]` = `HEAD_0..HEAD_3` | rnndb `g80_pdisplay.xml:516` / `:528` (registers), `:455-458` (bitset `gf119_pdisplay_intr_summary`) | **[EXT]** — read-only |
//!
//! ## KVBLANK2 §R1 — THE CITATION, AND WHERE KVBLANK'S SEARCH STOPPED
//!
//! KVBLANK's row above said, verbatim, *"No offset for this pair exists in this tree or in the two
//! documents as this seat read them"*. **That was wrong, and the way it was wrong is the finding.**
//! The pair is in the FILE KVBLANK ALREADY CITED — `envytools/rnndb/display/g80_pdisplay.xml`, the
//! same file whose line 647 gives `HEAD_STAT`. That file carries TWO generations of PDISPLAY in one
//! `<array name="PDISPLAY" offset="0x610000" …>` (line 153): a `<stripe variants="G80:GF119">` that
//! OPENS AT LINE 182 and CLOSES AT LINE 446, and a `<stripe variants="GF119-">` that OPENS AT LINE
//! 448 and CLOSES AT LINE 603. KVBLANK read the FIRST stripe — whose interrupt registers are
//! `INTR_0`/`INTR_1`/`INTR_EN_0`/`INTR_EN_1` at `+0x0020..0x002c` (lines 214-217), NV50-shaped, with
//! no per-head VBLANK bit — found no per-head pair there, and reported the absence. The GK107 is a
//! GF119-class display and its registers begin two lines past where that read ended.
//!
//! **THE GENERATION IS NOT ASSUMED, IT IS READ OFF THE BITSET.** `gf119_pdisplay_intr_head` (line
//! 489) carries two bitfields marked `variants="GK104-"` — `pos="2" name="UNK2"` (line 492) and
//! `pos="27" name="UNK27"` (line 507). A bitset that names GK104-only bits is a bitset rnndb
//! asserts over GK104-generation parts, and the GK107 in this laptop is one. `VBLANK` at `pos="0"`
//! (line 490) carries no `variants` qualifier, so it holds across the whole `GF119-` stripe.
//!
//! **THE BASE IS [TREE] AND UNCHANGED.** The GF119- stripe carries no `offset=` attribute, so its
//! register offsets are PDISPLAY-relative exactly as `HEAD_STAT`'s `0x6000` is, and PDISPLAY is
//! `0x610000` — `regs::NV_PDISPLAY_BASE`, the constant `kepler_display.rs` has read through since
//! BEAMX86. Not one new base is introduced by this rung.
//!
//! **WHAT IS STILL NOT CITED, AND IS THEREFORE NOT DONE.** rnndb names the registers and the bits;
//! it does NOT state the ACK protocol for a latched head status bit. `INTR_HEAD_STATUS` has an
//! `INTR_HEAD_TRIGGER` partner, which is suggestive of a write-1-to-set / write-1-to-clear pair, but
//! *suggestive* is not a citation. **So KVBLANK2's ISR never writes a status or trigger register.**
//! It acknowledges by DISARMING at the cited ENABLE — writing `INTR_HOST_HEAD_EN` back to the value
//! captured before the arm — which makes a storm impossible by construction and touches only a
//! register this table cites. See [`kepler_vblank_isr`].
//!
//! ## WHY `mode=poll`, and what the interrupt path would need
//!
//! The brief's rung 1 is an interrupt if the tree has a vector/MSI helper for a PCI function and a
//! POLL if it does not. It does not. `arch/x86_64/interrupts.rs` has exactly three device vectors,
//! each a HARD-CODED constant with its own `extern "x86-interrupt"` handler installed at
//! `idt[..].set_handler_fn` (`XHCI_MSI_VECTOR` 0x40, `NIC_MSI_VECTOR` 0x41, `EHCI_MSI_VECTOR`
//! 0x43); there is no `alloc_vector`, no `register_irq` and no vector table a driver can join.
//! `PciScanner::enable_msi`/`enable_msix` exist, but they take a vector a caller must already own.
//! Giving the GK107 one would mean a FOURTH constant, a fourth handler and a fourth IDT entry —
//! a new interrupt mechanism, which this rung's brief forbids. So:
//!
//! **THE EDGE SOURCE IS `HEAD_STAT.VERT[31:16]`, POLLED.** That is not a weaker reading than the
//! interrupt would have given: it is the SAME counter the display engine would have raised the
//! interrupt from, it is already cited, and `beam_probe` already proves per boot that it ticks. It
//! costs NOTHING extra to read, because [`note`] is fed the word `scanout_beam` was already
//! reading — the compositor's own hold loop is the sampler, at ~400 kHz during a hold.
//!
//! What the interrupt path would need, named so the next rung is a shopping list and not a hunt:
//! (a) a `KEPLER_MSI_VECTOR` constant and handler in `arch/x86_64/interrupts.rs`; (b) a
//! `PciScanner::enable_msi` call on the GK107 function with that vector; (c) the PDISPLAY-side
//! per-head vblank interrupt ENABLE and STATUS offsets, which are the `NOT-IN-TREE` row above and
//! are the REAL blocker — without them the engine never raises the event, whatever PMC says.
//!
//! **KVBLANK2 ANSWERS ALL THREE OF THOSE, and the section above is left standing as the record of
//! why the first pass could not.** (a) is answered by VECTORS (rmbp-ledger **B168**, fold
//! `05f26717`): `interrupts::vectors::alloc(name, handler)` registers the IDT entry BEFORE it
//! returns the number, and the IDT is mutable after `lidt`, so a driver that probes late can join —
//! there is no const to add and no IDT edit to make. (b) is answered by `PciScanner::enable_msi`
//! against the GK107 function, with IOAPIC (**B147**) routing INTx when MSI is refused. (c) is
//! answered by the six cited rows above. See the three KVBLANK2 rungs at this module's tail.
//!
//! ## KVBLANK2 §R2b — THE DEFECT KVBLANK'S RUNG 2 SHIPPED, FOUND BY READING THE WAIT'S SOURCE
//!
//! **[`wait_next_edge`] as KVBLANK shipped it could never have advanced on hardware, and the
//! fixture could not see it.** The wait spins on [`counter()`], which answers from `VB_COUNT`.
//! `VB_COUNT` is advanced by exactly one thing: [`note`], fed the word `scanout_beam()` read. And
//! the wait arm in `beam::hold` calls NEITHER — it snapshots `counter()`, then spins on `counter()`
//! alone while the spin arm below it (the one that *does* re-read `scanout_beam()`) has not been
//! entered yet. Inside the wait, `VB_COUNT` is FROZEN. `hold` runs IRQ-masked on the presenting core
//! inside `COMP_GATE` (`wm.rs:8582`, `:6950`) and `COMP_GATE` serialises composite passes, so no
//! sibling core is sampling either. The wait therefore burns its whole `GIVEUP_FRAMES` budget —
//! 33.3 ms — and falls through to the spin, on EVERY present that takes the arm: not a saving, a
//! 33 ms REGRESSION, on exactly the wide-zone presents B135 §7 measured as the expensive ones.
//!
//! **Why no gate caught it.** QEMU q35 has no Kepler, so every hardware path of the rung is
//! unreachable there, and the fixture drives the arm through `SIM_MODE = 1` — a counter computed
//! from `now_cycles()`, which advances with WALL TIME and needs no sampler at all. The fixture's
//! green half passed for a reason that does not exist on the bench. That is the `ungated gate`
//! shape from the other side: the fixture was real, its SOURCE was not.
//!
//! **THE FIX, and it is in this file.** [`wait_next_edge`] now SAMPLES: one `scanout_beam()` per
//! iteration, which feeds [`note`] and lets `VB_COUNT` advance. The wait costs the same MMIO the
//! spin does — so the arm buys the EDGE (a counter compare, one wake per frame) and NOT the saving
//! VUGPERF asked for. **The saving needs the ISR, and the ISR needs `hold` to run unmasked**, which
//! is a `video/wm.rs` change this brief fences off and which is REPORTED, not taken. Under the
//! fixture (`SIM_MODE != 0`) the sample is skipped, so both halves score exactly as B145 recorded.
//!
//! ## What `arm_pmc_pdisplay` does, and what it does not
//!
//! `kepler::init` has written `NV_PMC_INTR_EN = 0` since the driver's first day ("Disable
//! Interrupts", `kepler.rs:1429`). Under this knob, and ONLY under it, the rung then sets exactly
//! one bit — PDISPLAY — observes `NV_PMC_INTR_0` across a bounded window to see whether the
//! display engine latches anything there at all, and RESTORES the captured value with a READ-BACK.
//! One register, one bit, restored and read back: the `ctrlbind chan-restored` idiom. Delivery is
//! impossible by construction (no vector), which is the point — the rung asks whether the SOURCE
//! is alive, not whether an interrupt arrives.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::kepler::{mmio_read, mmio_write, regs};

// ── Cited constants ───────────────────────────────────────────────────────────────────────────

/// PDISPLAY's source bit in `NV_PMC_INTR_0` / `NV_PMC_INTR_EN`.
///
/// CITATION CLASS **[EXT], UNVERIFIED ON GK107** (`falcon_microcode_spec.md` §0.1). The offsets of
/// both PMC words are [TREE] (`kepler.rs:9-10`, `gpu_spec.md` §2.1); the SOURCE-BIT assignment is
/// read from the public NV50+ PMC interrupt source table (envytools `docs/hw/bus/pmc.txt`, mirrored
/// by open-gpu-doc's `dev_master` `NV_PMC_INTR_0_PDISP`). Nothing in this tree has ever observed it
/// set, which is exactly what rung 1 is flying to find out — so the witness line prints the RAW
/// words beside the decoded bit and a flight may re-derive the assignment from them.
const PMC_INTR_BIT_PDISPLAY: u32 = 26;

// ── KVBLANK2 §R1 — the GF119- PDISPLAY interrupt block, every offset with its document line ────
//
// ALL SIX are PDISPLAY-relative, i.e. `regs::NV_PDISPLAY_BASE` (`0x610000`, rnndb
// `display/g80_pdisplay.xml:153`) + the offset below + `head * DISP_HEAD_STRIDE`. They live in the
// `<stripe variants="GF119-">` that opens at LINE 448 of that file and closes at LINE 603; that
// stripe carries no `offset=` attribute, so its offsets are PDISPLAY-relative exactly as
// `HEAD_STAT`'s `0x6000` (line 647) is. An offset with no line number here is NOT-IN-TREE and is
// never read (KVBLANK's idiom, `igpu.rs:1731`). No nouveau and no Linux source was read.

/// `INTR_SUMMARY` — rnndb `g80_pdisplay.xml:516`. Bits `[24..27]` = `HEAD_0..HEAD_3` (bitset
/// `gf119_pdisplay_intr_summary`, lines 449-459). READ-ONLY in this rung.
const DISP_INTR_SUMMARY: usize = 0x058;
/// `INTR_HEAD_STATUS` — rnndb `:523`, `stride="0x800" length="4"`. Bitset
/// `gf119_pdisplay_intr_head`. READ-ONLY in this rung (see the ACK note in the module doc).
const DISP_INTR_HEAD_STATUS: usize = 0x074;
/// `INTR_HOST_SUMMARY` — rnndb `:528`. Same summary bitset, HOST-directed. READ-ONLY.
const DISP_INTR_HOST_SUMMARY: usize = 0x088;
/// `INTR_HOST_HEAD` — rnndb `:541`, `stride="0x800" length="4"`. The HOST-directed per-head STATUS
/// word; bit 0 is `VBLANK`. READ-ONLY.
const DISP_INTR_HOST_HEAD: usize = 0x0BC;
/// `INTR_HOST_HEAD_EN` — rnndb `:542`, `stride="0x800" length="4"`. **THE ENABLE, and the ONLY
/// register in this block KVBLANK2 ever writes** — one bit, for a bounded window, restored from the
/// value captured before the arm and read back.
const DISP_INTR_HOST_HEAD_EN: usize = 0x0C0;
/// `INTR_HOST_HEAD_DISPATCH` — rnndb `:544`, `stride="0x800" length="4"`. Which destination a head
/// interrupt is delivered to. READ-ONLY: this rung REPORTS it and never steers it.
const DISP_INTR_HOST_HEAD_DISPATCH: usize = 0x0C8;
/// The per-head stride of the whole GF119- interrupt block — `stride="0x800"` on every `_HEAD_`
/// register above (rnndb `:523`, `:524`, `:541`-`:544`). Identical to `HEAD_STAT`'s, and that is
/// rnndb's statement rather than an inference from it.
const DISP_HEAD_STRIDE: usize = 0x800;
/// `VBLANK` in bitset `gf119_pdisplay_intr_head` — rnndb `g80_pdisplay.xml:490`, `pos="0"`. It
/// carries no `variants` qualifier, so it holds across the whole `GF119-` stripe. The bitset is
/// GK104-aware on rnndb's own say-so: it declares `pos="2"` (`:492`) and `pos="27"` (`:507`) as
/// `variants="GK104-"`, and the GK107 is a GK104-generation part.
const DISP_INTR_HEAD_BIT_VBLANK: u32 = 0;
/// `HEAD_0` in bitset `gf119_pdisplay_intr_summary` — rnndb `:455`; heads 1..3 follow at `:456`-`:458`.
const DISP_INTR_SUMMARY_HEAD0_SHIFT: u32 = 24;

/// PDISPLAY-relative address of a per-head register in the GF119- interrupt block.
#[inline]
fn disp_head(off: usize, head: usize) -> usize {
    regs::NV_PDISPLAY_BASE + off + head * DISP_HEAD_STRIDE
}

// ── KVBLANK2 — rung budgets, every one bounded by construction ─────────────────────────────────

/// §R1 census samples per boot. Eight is enough to say whether a status word ever moves and few
/// enough that the whole rung costs 8 x 6 = 48 MMIO reads for the life of the boot.
const CENSUS_SAMPLES: u32 = 8;
/// Vblank edges between census samples — ~0.5 s at 60 Hz, so the census spans ~4 s of scanout.
const CENSUS_SPACING: u64 = 30;
/// §R2 — vblanks the per-head VBLANK enable bit is held for. 16 frames is ~267 ms: long enough that
/// a latching status word has had sixteen chances, short enough to be one paragraph of a boot.
const WINDOW_VBLANKS: u64 = 16;
/// §R3 — vblanks the MSI/INTx window is held for, ~1 s at 60 Hz. `irq=` against `vbl_delta=` over
/// this window is the whole of rung 3's verdict.
const IRQ_WINDOW_VBLANKS: u64 = 60;
/// §R3 — ISR entries past which the handler cuts PDISPLAY at PMC and stops. A level-triggered INTx
/// whose line the enable-disarm does not deassert would otherwise live-lock the core. Nothing in
/// rnndb promises the disarm deasserts, so the cap is the thing that makes the rung safe to fly.
const IRQ_STORM_CAP: u64 = 4_096;
/// §R3 — highest PCI bus the GK107 hunt walks. The hunt matches on BAR0, at `kepler::init` time,
/// off every hot path; the rMBP's discrete GPU sits at `01:00.0` and the bound is REPORTED on the
/// witness so a function that was not found is attributable to the bound rather than to the match.
const GK107_BUS_MAX: u8 = 16;

/// The measurement window `arm_pmc_pdisplay` holds the bit for, in ms: three frames at 60 Hz, so a
/// latching status word has had at least two whole vblanks to latch in.
const PMC_WINDOW_MS: u64 = 50;

/// Hard iteration cap for that window — `crate::arch::ms()` is the APIC tick and a pure
/// time-budget loop inside `init` would spin forever if it were ever stopped. Bounded by
/// construction, exactly as `beam_probe`'s `BEAM_SPIN_CAP` is.
const PMC_SPIN_CAP: u32 = 4_000_000;

/// Frames the rung-2 wait may burn before it gives up. **MUST equal `video::beam::GIVEUP_FRAMES`** —
/// the go-red half of the fixture asserts the wait gives up on the SAME budget as the spin, so the
/// two cannot be allowed to drift. Checked at compile time below.
const GIVEUP_FRAMES: u64 = 2;
const _: () = assert!(GIVEUP_FRAMES == crate::video::beam::GIVEUP_FRAMES);

/// Rung 1 prints at 1 Hz for the first `FAST_PRINTS` seconds, then drops to [`SLOW_MS`].
const FAST_MS: u64 = 1_000;
const FAST_PRINTS: u32 = 10;
/// The slow cadence — the display trace cadence, i.e. once per ten seconds, which is the rate the
/// compositor's own rollups settle to on a quiet desktop.
const SLOW_MS: u64 = 10_000;

// ── Rung 1 state ──────────────────────────────────────────────────────────────────────────────

/// Head index `beam_probe` ARMED, plus 1. Zero = rung 1 is not counting (no beam source this boot).
static VB_HEAD1: AtomicU32 = AtomicU32::new(0);
/// Lines per frame, as `beam_probe` measured them.
static VB_VTOTAL: AtomicU32 = AtomicU32::new(0);
/// `vblank_count[31:16]` as last seen, plus the 1-bit "seen at all" flag in bit 16.
static VB_LAST: AtomicU32 = AtomicU32::new(0);
/// Monotonic edge count. THE counter rung 2 compares against; it does not wrap at 16 bits the way
/// the hardware field does.
static VB_COUNT: AtomicU64 = AtomicU64::new(0);
/// `now_cycles()` at the last edge.
static VB_LAST_CYC: AtomicU64 = AtomicU64::new(0);
/// Sum / min / max of the per-edge cycle deltas, for `period_us=` and `jitter_us=`.
static VB_SUM_CYC: AtomicU64 = AtomicU64::new(0);
static VB_MIN_CYC: AtomicU64 = AtomicU64::new(u64::MAX);
static VB_MAX_CYC: AtomicU64 = AtomicU64::new(0);
/// `vline` read INSIDE the edge — THE PHASE, and the one number rung 2's threshold is derived from.
/// Stored plus 1 so that zero means "no edge has been phased yet" and line 0 stays a legal reading.
static VB_PHASE1: AtomicU32 = AtomicU32::new(0);
/// `crate::arch::ms()` at the last print, and how many fast prints have gone out.
static VB_LAST_PRINT_MS: AtomicU64 = AtomicU64::new(0);
static VB_PRINTS: AtomicU32 = AtomicU32::new(0);

// ── Rung 2 accounting ─────────────────────────────────────────────────────────────────────────

/// Holds that took the WAIT arm, and the microseconds they spent in it. Reported on this module's
/// own witness line as `vbwaits=` / `vbwait_us=`, beside `beamwaits=` on `[wc-h]`.
///
/// ⚠ OWED: the brief asks for these two fields ON `[wc-h]` itself. That line is emitted from
/// `video/wm.rs`, which this rung's brief does not name (PTRPAINT/VUGPROBE hold it), so the census
/// ships on the `:: kepler: vblank` line instead and the `[wc-h]` fields are owed to the fold.
static VB_WAITS: AtomicU64 = AtomicU64::new(0);
static VB_WAIT_US: AtomicU64 = AtomicU64::new(0);
/// Waits that ran out the GIVEUP budget without the counter moving, and waits whose confirming
/// raster read landed back inside the hazard zone and fell through to the spin.
static VB_WAIT_GAVEUP: AtomicU64 = AtomicU64::new(0);
static VB_WAIT_RECHECK: AtomicU64 = AtomicU64::new(0);

// ── KVBLANK2 state — the ladder, and every register value it must restore ─────────────────────

/// BAR0 as `kepler::init` handed it to [`arm_pmc_pdisplay`]: the PHYSICAL base, identity-mapped
/// (`kepler.rs` maps it with `map_mmio_window` and aborts the probe if `translate` says no). Zero
/// until that call, and EVERY KVBLANK2 rung refuses on zero rather than reading at 0 + an offset.
static VB_BAR0: AtomicUsize = AtomicUsize::new(0);
/// The GK107's `bus<<16 | slot<<8 | func`, plus 1 so zero means "not found". Resolved ONCE, in
/// [`arm_pmc_pdisplay`], by matching BAR0 — see [`find_gk107`].
static VB_BDF1: AtomicU32 = AtomicU32::new(0);

/// The ladder's position. Rungs run in order, each exactly once per boot, each driven by [`edge`]
/// so that no rung can run before there is a raster to measure it against.
const LADDER_CENSUS: u32 = 0;
const LADDER_WINDOW_ARM: u32 = 1;
const LADDER_WINDOW_RUN: u32 = 2;
const LADDER_IRQ_ARM: u32 = 3;
const LADDER_IRQ_RUN: u32 = 4;
const LADDER_DONE: u32 = 5;
static LADDER: AtomicU32 = AtomicU32::new(LADDER_CENSUS);

/// §R1 — census samples taken, and the edge count at the last one (for `count_delta=`).
static CENSUS_N: AtomicU32 = AtomicU32::new(0);
static CENSUS_AT: AtomicU64 = AtomicU64::new(0);

/// §R2/§R3 — the captured `INTR_HOST_HEAD_EN` word, the edge count the window opened at, the OR of
/// every status word read inside it, and how many samples saw the VBLANK bit set.
static WIN_EN_ENTRY: AtomicU32 = AtomicU32::new(0);
static WIN_AT: AtomicU64 = AtomicU64::new(0);
static WIN_STATUS_OR: AtomicU32 = AtomicU32::new(0);
static WIN_HOST_OR: AtomicU32 = AtomicU32::new(0);
static WIN_SEEN: AtomicU32 = AtomicU32::new(0);
static WIN_SAMPLES: AtomicU32 = AtomicU32::new(0);

/// §R3 — ISR entries. **THE counter the mode is decided by**, and the one number that says whether
/// the GK107 delivered a vblank to this kernel at all.
static IRQ_COUNT: AtomicU64 = AtomicU64::new(0);
/// §R3 — the allocated vector (plus 1; zero = none), the head the ISR disarms, the captured
/// `NV_PMC_INTR_EN` word, and how the interrupt was wired.
static IRQ_VECTOR1: AtomicU32 = AtomicU32::new(0);
static IRQ_HEAD: AtomicU32 = AtomicU32::new(0);
static IRQ_PMC_ENTRY: AtomicU32 = AtomicU32::new(0);
/// 0 = not wired, 1 = MSI, 2 = INTx through the IOAPIC.
static IRQ_WIRE: AtomicU32 = AtomicU32::new(0);
/// Set while the ISR may touch MMIO — cleared before the rung restores, so a late delivery after
/// the window closes cannot write a register the rung has already put back.
static IRQ_LIVE: AtomicBool = AtomicBool::new(false);
/// The ISR hit [`IRQ_STORM_CAP`] and cut PDISPLAY at PMC. A result, printed, never a panic.
static IRQ_STORMED: AtomicBool = AtomicBool::new(false);
/// **THE MODE, DECIDED BY THE WIRE.** Set only when [`IRQ_COUNT`] moved inside §R3's window.
static IRQ_MODE: AtomicBool = AtomicBool::new(false);

/// `mode=` for every witness line in this module. One function so the three rungs, [`report`] and
/// the `arm` line can never disagree about what the wire said.
#[inline]
fn mode_str() -> &'static str {
    if IRQ_MODE.load(Ordering::Relaxed) {
        "irq"
    } else {
        "poll"
    }
}

// ── The fixture's simulated source ────────────────────────────────────────────────────────────

/// Fixture mode: 0 = off (the hardware counter is the source), 1 = a counter the TIMER advances at
/// one simulated frame period, 2 = a counter that never advances (the go-red).
static SIM_MODE: AtomicU32 = AtomicU32::new(0);
/// `now_cycles()` when the simulation was armed, and the cycles in one simulated period.
static SIM_T0: AtomicU64 = AtomicU64::new(0);
static SIM_PERIOD_CYC: AtomicU64 = AtomicU64::new(0);
/// The fixture runs once per boot.
static SELFTEST_DONE: AtomicBool = AtomicBool::new(false);

// ── Helpers ───────────────────────────────────────────────────────────────────────────────────

/// `now_cycles()` deltas to microseconds. A private twin of `beam::cycles_to_us` for the same
/// reason that one is a twin of `wcg`'s: the owner module is gated differently.
#[inline]
fn cycles_to_us(dt: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    dt.saturating_mul(1_000_000) / hz
}

#[inline]
fn us_to_cycles(us: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    us.saturating_mul(hz) / 1_000_000
}

// ── RUNG 1 — the PMC half, called from `kepler::init`'s interrupt-disable statement ────────────

/// KVBLANK rung 1, PMC half. Set the PDISPLAY source bit in `NV_PMC_INTR_EN`, watch
/// `NV_PMC_INTR_0` for a bounded window, RESTORE the captured value and read it back.
///
/// Called from the `kepler.rs` statement that has written `NV_PMC_INTR_EN = 0` since the driver's
/// first day — so the captured value is that zero and the restore puts the driver back exactly
/// where it was. Exactly one bit is ever set, it is named and cited, and the restore is READ BACK
/// and printed: `restored=<v> verdict=clean|DIRTY`.
///
/// # Safety
/// `bar0` must be the mapped BAR0 base `kepler::init`'s other accesses already use.
pub unsafe fn arm_pmc_pdisplay(bar0: usize) {
    // KVBLANK2 — THE STASH, and it is the whole reason the later rungs need no new call site.
    // `kepler::init`'s call hands this rung BAR0 and nothing else, and that call is a SAME-LINE
    // append in a file this brief fences off (`kepler.rs`'s init), so its signature cannot change.
    // Parking BAR0 here lets §R1/§R2/§R3 run from `edge()` — which `kepler_display.rs` already
    // feeds — without one new line in any file but this one. Identity-mapped physical, so it is
    // valid on every core, which the §R3 ISR depends on.
    VB_BAR0.store(bar0, Ordering::Release);
    // And THE BDF, resolved once, here, off every hot path. `find_gk107` matches on BAR0, so the
    // function it names is BY CONSTRUCTION the function whose registers this rung has been reading
    // since KVBLANK — a mismatch between the MMIO base and the config-space function is impossible.
    let bdf = find_gk107(bar0);
    if let Some((b, s, f)) = bdf {
        VB_BDF1.store((((b as u32) << 16) | ((s as u32) << 8) | f as u32) + 1, Ordering::Release);
    }
    serial_println!(
        ":: kepler: vblank bdf-hunt bar0={:08X} bus_max={} found={} bdf={}:{}.{} :: — the GK107's PCI function, matched by BAR0 against config-space offset 0x10 on every NVIDIA (0x10DE) function up to the bound. Needed by KVBLANK2 §R3 (MSI/INTx) and resolved HERE because `kepler::init`'s call site is a fenced same-line append that hands this rung BAR0 alone. READ-ONLY: config writes=0 ::",
        bar0, GK107_BUS_MAX, bdf.is_some() as u32,
        bdf.map(|v| v.0).unwrap_or(0xFF), bdf.map(|v| v.1).unwrap_or(0xFF), bdf.map(|v| v.2).unwrap_or(0xFF),
    );

    let bit = 1u32 << PMC_INTR_BIT_PDISPLAY;
    let en_entry = mmio_read(bar0, regs::NV_PMC_INTR_EN);
    let st_entry = mmio_read(bar0, regs::NV_PMC_INTR_0);
    mmio_write(bar0, regs::NV_PMC_INTR_EN, en_entry | bit);
    let en_armed = mmio_read(bar0, regs::NV_PMC_INTR_EN);

    // Observe. Delivery is impossible (no vector is wired for this function — see the module doc),
    // so what is under test is whether the SOURCE latches in the status word at all.
    let (mut seen, mut samples, mut spins) = (0u32, 0u32, 0u32);
    let mut or_all = 0u32;
    let t0 = crate::arch::ms();
    loop {
        if crate::arch::ms().wrapping_sub(t0) > PMC_WINDOW_MS {
            break;
        }
        spins = spins.saturating_add(1);
        if spins >= PMC_SPIN_CAP {
            break;
        }
        let st = mmio_read(bar0, regs::NV_PMC_INTR_0);
        samples = samples.saturating_add(1);
        or_all |= st;
        if st & bit != 0 {
            seen = seen.saturating_add(1);
        }
        core::hint::spin_loop();
    }

    mmio_write(bar0, regs::NV_PMC_INTR_EN, en_entry);
    let en_back = mmio_read(bar0, regs::NV_PMC_INTR_EN);
    let clean = en_back == en_entry;

    serial_println!(
        ":: kepler: vblank pmc-arm bit={} en_entry={:08X} en_armed={:08X} intr_entry={:08X} intr_or={:08X} pdisplay_seen={}/{} window_ms={} deliver=none reason=no-vector-helper restored={:08X} verdict={} :: — NV_PMC_INTR_EN/INTR_0 offsets are [TREE] (kepler.rs:9-10, gpu_spec 2.1); BIT {} = PDISPLAY is [EXT] and UNVERIFIED on GK107 (public NV50+ PMC source table), so the RAW words are printed beside the decode and a flight may re-derive it. The PDISPLAY-side per-head vblank ENABLE/STATUS pair is NOT-IN-TREE and is neither read nor written here. writes=2 (set, restore), restored+read-back ::",
        PMC_INTR_BIT_PDISPLAY, en_entry, en_armed, st_entry, or_all, seen, samples, PMC_WINDOW_MS,
        en_back, if clean { "clean" } else { "DIRTY" }, PMC_INTR_BIT_PDISPLAY,
    );
}

/// KVBLANK rung 1 — ARM the counter. Called from `beam_probe`'s ARMED branch with the head it chose
/// and the lines-per-frame it measured, so this module counts edges on the SAME head the beam gate
/// reads and a disagreement between the two is impossible by construction.
pub fn arm(head: u32, vtotal: u32) {
    VB_VTOTAL.store(vtotal, Ordering::Relaxed);
    VB_LAST_PRINT_MS.store(crate::arch::ms(), Ordering::Relaxed);
    VB_HEAD1.store(head + 1, Ordering::Release);
    serial_println!(
        ":: kepler: vblank arm head={} vt={} mode={} src=HEAD_STAT.VERT[31:16] :: — rnndb display/g80_pdisplay.xml:647 (HEAD_STAT 0x6000, stride 0x800, +0x340 VERT: vline[15:0], vblank_count[31:16]). mode= starts at poll and is set to irq ONLY by KVBLANK2 rung 3 observing the wire deliver (see the `vblank-intr vector close` line); KVBLANK's reason for poll — no vector allocator a PCI function could join — was closed by VECTORS (rmbp-ledger B168) and the NOT-IN-TREE enable/status pair by KVBLANK2's citation (B179). The polled sampler is the compositor's own scanout_beam read: zero extra MMIO. READ-ONLY: writes=0 ::",
        head, vtotal, mode_str(),
    );
}

/// KVBLANK rung 1 — the edge detector, fed the RAW `HEAD_STAT.VERT` word `scanout_beam` just read.
///
/// FAST PATH: one relaxed load and a 16-bit compare, then return. `now_cycles()` and the witness
/// print are reached only ON an edge — at most 60 times a second on a 60 Hz panel — so this costs
/// the hold loop a compare per iteration and nothing else.
#[inline]
pub fn note(word: u32) {
    if VB_HEAD1.load(Ordering::Acquire) == 0 {
        return;
    }
    let vb = (word >> 16) & 0xFFFF;
    let prev = VB_LAST.load(Ordering::Relaxed);
    if prev & 0x1_0000 != 0 && (prev & 0xFFFF) == vb {
        return;
    }
    VB_LAST.store(vb | 0x1_0000, Ordering::Relaxed);
    if prev & 0x1_0000 == 0 {
        // First reading of the field is not an edge — there is no previous timestamp to subtract.
        VB_LAST_CYC.store(crate::arch::now_cycles(), Ordering::Relaxed);
        return;
    }
    edge(word & 0xFFFF);
}

/// The edge itself: timestamp it, fold it into the period/jitter accumulators, record the PHASE and
/// print on the cadence.
fn edge(vline: u32) {
    let now = crate::arch::now_cycles();
    let last = VB_LAST_CYC.swap(now, Ordering::Relaxed);
    let n = VB_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    // THE PHASE. `vline` here is the raster position read in the SAME word as the counter tick, so
    // it is where in the frame the edge was observed — `raster_at_irq`, and the number rung 2's
    // threshold is derived from.
    VB_PHASE1.store(vline + 1, Ordering::Relaxed);
    // KVBLANK2 — the three rungs, driven by the EDGE and by nothing else. Sited here because an
    // edge is the only moment at which "a bounded number of vblanks" is a measurable quantity, and
    // because this is the one place per frame that is already off the fast path (`note`'s compare
    // returns before reaching `edge` on every non-edge iteration).
    ladder_tick(n);
    if last != 0 {
        let dt = now.saturating_sub(last);
        VB_SUM_CYC.fetch_add(dt, Ordering::Relaxed);
        VB_MIN_CYC.fetch_min(dt, Ordering::Relaxed);
        VB_MAX_CYC.fetch_max(dt, Ordering::Relaxed);
    }
    let prints = VB_PRINTS.load(Ordering::Relaxed);
    let due = if prints < FAST_PRINTS { FAST_MS } else { SLOW_MS };
    let ms = crate::arch::ms();
    if ms.wrapping_sub(VB_LAST_PRINT_MS.load(Ordering::Relaxed)) < due {
        return;
    }
    VB_LAST_PRINT_MS.store(ms, Ordering::Relaxed);
    VB_PRINTS.store(prints.saturating_add(1), Ordering::Relaxed);
    report(n, vline);
}

/// The witness line. Split out so the fixture can print the same shape without an edge behind it.
fn report(n: u64, vline: u32) {
    let sum = VB_SUM_CYC.load(Ordering::Relaxed);
    let mn = VB_MIN_CYC.load(Ordering::Relaxed);
    let mx = VB_MAX_CYC.load(Ordering::Relaxed);
    let period_us = if n > 1 { cycles_to_us(sum / (n - 1)) } else { 0 };
    let jitter_us = if mx >= mn && mn != u64::MAX { cycles_to_us(mx - mn) } else { 0 };
    serial_println!(
        ":: kepler: vblank head={} count={} period_us={} jitter_us={} raster_at_irq={} vt={} mode={} vbwaits={} vbwait_us={} vbgaveup={} vbrecheck={} ::",
        VB_HEAD1.load(Ordering::Relaxed).saturating_sub(1),
        n, period_us, jitter_us, vline, VB_VTOTAL.load(Ordering::Relaxed), mode_str(),
        VB_WAITS.load(Ordering::Relaxed), VB_WAIT_US.load(Ordering::Relaxed),
        VB_WAIT_GAVEUP.load(Ordering::Relaxed), VB_WAIT_RECHECK.load(Ordering::Relaxed),
    );
}

// ══ KVBLANK2 — the three rungs ════════════════════════════════════════════════════════════════
//
// Every rung is driven by [`edge`], runs EXACTLY ONCE per boot, and is bounded in vblanks rather
// than in wall time, because a vblank is the unit the thing under test is measured in. Every
// register they touch is in the table at the head of this file with its document and its line.

/// The GK107's `(bus, slot, func)`, found by matching BAR0 against config-space offset `0x10` on
/// every NVIDIA function up to [`GK107_BUS_MAX`].
///
/// **THE MATCH IS ON BAR0 AND THAT IS THE POINT.** `PciScanner::find_device(0x03, 0x00)` would
/// answer with the FIRST display-class function, and on this laptop that is the Intel IGD — arming
/// MSI on the wrong function would be indistinguishable on the wire from a GK107 that never
/// delivers. Matching the base this rung has been reading through since KVBLANK makes the two
/// impossible to confuse.
fn find_gk107(bar0_phys: usize) -> Option<(u8, u8, u8)> {
    let want = bar0_phys & !0xFusize;
    for bus in 0..=GK107_BUS_MAX {
        for slot in 0..32u8 {
            for func in 0..8u8 {
                let id = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x00) };
                if id == 0xFFFF_FFFF || (id & 0xFFFF) != 0x10DE {
                    continue;
                }
                let b0 = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x10) };
                if (b0 as usize) & !0xFusize == want {
                    return Some((bus, slot, func));
                }
            }
        }
    }
    None
}

/// Read the six cited registers for `head` and return
/// `(summary, host_summary, head_status, host_head, host_head_en, host_head_dispatch)`.
///
/// # Safety
/// `bar0` must be the mapped BAR0 base the rest of this module uses.
unsafe fn read_intr_block(bar0: usize, head: usize) -> (u32, u32, u32, u32, u32, u32) {
    (
        mmio_read(bar0, regs::NV_PDISPLAY_BASE + DISP_INTR_SUMMARY),
        mmio_read(bar0, regs::NV_PDISPLAY_BASE + DISP_INTR_HOST_SUMMARY),
        mmio_read(bar0, disp_head(DISP_INTR_HEAD_STATUS, head)),
        mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD, head)),
        mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head)),
        mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD_DISPATCH, head)),
    )
}

/// The ladder. One step per vblank edge, never more.
fn ladder_tick(n: u64) {
    if SIM_MODE.load(Ordering::Relaxed) != 0 {
        return; // The fixture drives `wait_next_edge`, never the hardware ladder.
    }
    let bar0 = VB_BAR0.load(Ordering::Acquire);
    if bar0 == 0 {
        return;
    }
    let head = VB_HEAD1.load(Ordering::Relaxed).saturating_sub(1) as usize;
    if head >= 4 {
        return;
    }
    match LADDER.load(Ordering::Relaxed) {
        LADDER_CENSUS => rung1_census(bar0, head, n),
        LADDER_WINDOW_ARM => rung2_arm(bar0, head, n),
        LADDER_WINDOW_RUN => rung2_run(bar0, head, n),
        LADDER_IRQ_ARM => rung3_arm(bar0, head, n),
        LADDER_IRQ_RUN => rung3_run(bar0, head, n),
        _ => {}
    }
}

/// **§R1 — THE CENSUS. WRITES NOTHING.** Read the DISP interrupt status and enable registers
/// [`CENSUS_SAMPLES`] times across the boot and print them with the vblank-count delta beside them.
///
/// This is the rung that makes §R2 interpretable: a status word that is ALREADY moving before
/// anything is enabled means the engine latches regardless of the enable, and a status word frozen
/// at the same value for eight samples across four seconds of live scanout means the enable is
/// load-bearing. Either answer is a result.
fn rung1_census(bar0: usize, head: usize, n: u64) {
    let taken = CENSUS_N.load(Ordering::Relaxed);
    let at = CENSUS_AT.load(Ordering::Relaxed);
    if taken > 0 && n.saturating_sub(at) < CENSUS_SPACING {
        return;
    }
    CENSUS_AT.store(n, Ordering::Relaxed);
    CENSUS_N.store(taken + 1, Ordering::Relaxed);
    let (sum, hsum, st, hst, en, disp) = unsafe { read_intr_block(bar0, head) };
    let vb = 1u32 << DISP_INTR_HEAD_BIT_VBLANK;
    serial_println!(
        ":: kepler: vblank-intr census sample={}/{} head={} status={:08X} en={:08X} count_delta={} host_status={:08X} host_dispatch={:08X} summary={:08X} host_summary={:08X} vblank_bit={} host_vblank_bit={} summary_head_bit={} :: — READ-ONLY, writes=0. Offsets: INTR_HEAD_STATUS +0x074 (rnndb display/g80_pdisplay.xml:523), INTR_HOST_HEAD +0x0BC (:541), INTR_HOST_HEAD_EN +0x0C0 (:542), INTR_HOST_HEAD_DISPATCH +0x0C8 (:544), INTR_SUMMARY +0x058 (:516), INTR_HOST_SUMMARY +0x088 (:528); all PDISPLAY-relative (base 0x610000, :153), stride 0x800 x 4 heads, GF119- stripe (:448-603). VBLANK is bit 0 of bitset gf119_pdisplay_intr_head (:490); summary HEAD_n at bit {}+n (:455-458). This pair was KVBLANK's NOT-IN-TREE row and is now cited ::",
        taken + 1, CENSUS_SAMPLES, head, st, en, n.saturating_sub(at), hst, disp, sum, hsum,
        (st & vb != 0) as u32, (hst & vb != 0) as u32,
        ((hsum >> (DISP_INTR_SUMMARY_HEAD0_SHIFT + head as u32)) & 1),
        DISP_INTR_SUMMARY_HEAD0_SHIFT,
    );
    if taken + 1 >= CENSUS_SAMPLES {
        LADDER.store(LADDER_WINDOW_ARM, Ordering::Release);
    }
}

/// **§R2 — OPEN THE ENABLE WINDOW.** Set the per-head `VBLANK` enable bit, exactly one bit, in the
/// one cited enable register, and capture what was there before it.
fn rung2_arm(bar0: usize, head: usize, n: u64) {
    let bit = 1u32 << DISP_INTR_HEAD_BIT_VBLANK;
    let en_entry = unsafe { mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head)) };
    WIN_EN_ENTRY.store(en_entry, Ordering::Relaxed);
    WIN_AT.store(n, Ordering::Relaxed);
    WIN_STATUS_OR.store(0, Ordering::Relaxed);
    WIN_HOST_OR.store(0, Ordering::Relaxed);
    WIN_SEEN.store(0, Ordering::Relaxed);
    WIN_SAMPLES.store(0, Ordering::Relaxed);
    unsafe { mmio_write(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head), en_entry | bit) };
    let en_armed = unsafe { mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head)) };
    serial_println!(
        ":: kepler: vblank-intr window open head={} bit={} en_entry={:08X} en_armed={:08X} took={} window_vblanks={} :: — ONE bit set in INTR_HOST_HEAD_EN (PDISPLAY +0x0C0 + head*0x800, rnndb display/g80_pdisplay.xml:542), bit 0 = VBLANK (:490). The captured word is restored with a read-back after {} vblanks, counted on HEAD_STAT.VERT[31:16] — the ctrlbind chan-restored idiom. writes=1 so far. `took=0` means the enable did NOT latch, which is itself the answer ::",
        head, DISP_INTR_HEAD_BIT_VBLANK, en_entry, en_armed,
        ((en_armed ^ en_entry) & bit != 0) as u32, WINDOW_VBLANKS, WINDOW_VBLANKS,
    );
    LADDER.store(LADDER_WINDOW_RUN, Ordering::Release);
}

/// **§R2 — RUN AND CLOSE.** Sample the status words once per vblank for [`WINDOW_VBLANKS`] edges,
/// then restore the captured enable and read it back.
fn rung2_run(bar0: usize, head: usize, n: u64) {
    let vb = 1u32 << DISP_INTR_HEAD_BIT_VBLANK;
    let (_, _, st, hst, _, _) = unsafe { read_intr_block(bar0, head) };
    WIN_STATUS_OR.fetch_or(st, Ordering::Relaxed);
    WIN_HOST_OR.fetch_or(hst, Ordering::Relaxed);
    WIN_SAMPLES.fetch_add(1, Ordering::Relaxed);
    if (st | hst) & vb != 0 {
        WIN_SEEN.fetch_add(1, Ordering::Relaxed);
    }
    if n.saturating_sub(WIN_AT.load(Ordering::Relaxed)) < WINDOW_VBLANKS {
        return;
    }
    let en_entry = WIN_EN_ENTRY.load(Ordering::Relaxed);
    unsafe { mmio_write(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head), en_entry) };
    let en_back = unsafe { mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head)) };
    let seen = WIN_SEEN.load(Ordering::Relaxed);
    let samples = WIN_SAMPLES.load(Ordering::Relaxed);
    serial_println!(
        ":: kepler: vblank-intr window close head={} vblanks={} samples={} vblank_seen={}/{} status_or={:08X} host_status_or={:08X} restored={:08X} readback={:08X} verdict={} :: — writes=2 total (set, restore), restored and READ BACK. A status bit that never toggles across {} whole vblanks with the enable held is a RESULT and not a failure: it says the GF119- per-head VBLANK status does not latch for this head under this enable alone, and it is the evidence §R3 needs before it wires a vector. Registers as cited on the census line ::",
        head, WINDOW_VBLANKS, samples, seen, samples,
        WIN_STATUS_OR.load(Ordering::Relaxed), WIN_HOST_OR.load(Ordering::Relaxed),
        en_entry, en_back,
        if en_back == en_entry { "clean" } else { "DIRTY" },
        WINDOW_VBLANKS,
    );
    LADDER.store(LADDER_IRQ_ARM, Ordering::Release);
}

/// **§R3 — THE VECTOR.** Allocate `kepler-vblank` from the VECTORS allocator (B168), program the
/// GK107 function with it through `enable_msi` — or route its INTx through the IOAPIC (B147) when
/// MSI is refused — unmask PMC bit 26 and the per-head VBLANK enable, and open the window.
///
/// Everything captured here is restored by [`rung3_run`]; nothing is left armed past the window.
fn rung3_arm(bar0: usize, head: usize, n: u64) {
    LADDER.store(LADDER_IRQ_RUN, Ordering::Release); // one shot, whatever happens below
    IRQ_HEAD.store(head as u32, Ordering::Relaxed);
    WIN_AT.store(n, Ordering::Relaxed);
    IRQ_COUNT.store(0, Ordering::Relaxed);

    let bdf1 = VB_BDF1.load(Ordering::Acquire);
    if bdf1 == 0 {
        serial_println!(
            ":: kepler: vblank-intr vector REFUSED reason=no-bdf — the BAR0 match at kepler::init named no NVIDIA function up to bus {}, so there is no config space to program and NOTHING was armed. mode={} ::",
            GK107_BUS_MAX, mode_str(),
        );
        LADDER.store(LADDER_DONE, Ordering::Release);
        return;
    }
    let bdf = bdf1 - 1;
    let (bus, slot, func) = ((bdf >> 16) as u8, (bdf >> 8) as u8, bdf as u8);

    let vec = match crate::arch::interrupts::vectors::alloc("kepler-vblank", kepler_vblank_isr) {
        Some(v) => v,
        None => {
            serial_println!(
                ":: kepler: vblank-intr vector REFUSED reason=alloc — vectors::alloc(\"kepler-vblank\") answered None and has printed its own witness above. NOTHING was armed; mode stays {} ::",
                mode_str(),
            );
            LADDER.store(LADDER_DONE, Ordering::Release);
            return;
        }
    };
    IRQ_VECTOR1.store(vec as u32 + 1, Ordering::Relaxed);

    let msg_addr = 0xFEE0_0000u32 | ((crate::arch::x86_64::apic::apic_id() as u32) << 12);
    let wire = if crate::drivers::pci::PciScanner::enable_msi(bus, slot, func, msg_addr, vec as u32)
    {
        1
    } else if crate::arch::x86_64::ioapic_route_intx(bus, slot, func, vec) {
        2
    } else {
        0
    };
    IRQ_WIRE.store(wire, Ordering::Relaxed);
    if wire == 0 {
        serial_println!(
            ":: kepler: vblank-intr vector REFUSED reason=no-msi-no-intx bdf={}:{}.{} vector={:#04x} — this function offers no usable MSI capability and the IOAPIC would not take its INTx. The vector stays allocated (it is registered in the IDT and owned by name, which is the allocator's contract) and NOTHING on the GK107 was armed. mode stays {} ::",
            bus, slot, func, vec, mode_str(),
        );
        LADDER.store(LADDER_DONE, Ordering::Release);
        return;
    }

    // PMC bit 26 — unmask PDISPLAY for the window only. The captured word is what
    // `arm_pmc_pdisplay` restored, i.e. the `0` `kepler::init` has written since day one.
    let pmc_bit = 1u32 << PMC_INTR_BIT_PDISPLAY;
    let pmc_entry = unsafe { mmio_read(bar0, regs::NV_PMC_INTR_EN) };
    IRQ_PMC_ENTRY.store(pmc_entry, Ordering::Relaxed);
    // The per-head VBLANK enable — the same one bit §R2 proved restorable, in the same register.
    let en_entry = unsafe { mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head)) };
    WIN_EN_ENTRY.store(en_entry, Ordering::Relaxed);
    WIN_SEEN.store(0, Ordering::Relaxed);
    WIN_SAMPLES.store(0, Ordering::Relaxed);
    IRQ_STORMED.store(false, Ordering::Relaxed);
    IRQ_LIVE.store(true, Ordering::Release);
    unsafe {
        mmio_write(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head), en_entry | (1 << DISP_INTR_HEAD_BIT_VBLANK));
        mmio_write(bar0, regs::NV_PMC_INTR_EN, pmc_entry | pmc_bit);
    }
    serial_println!(
        ":: kepler: vblank-intr vector armed bdf={}:{}.{} vector={:#04x} wire={} pmc_entry={:08X} pmc_bit={} en_entry={:08X} head={} window_vblanks={} storm_cap={} :: — the vector came from interrupts::vectors::alloc (rmbp-ledger B168): no new const, no IDT edit, and the entry was registered BEFORE the number was returned. wire=1 MSI, wire=2 INTx via IOAPIC (B147). The ISR acknowledges by DISARMING at INTR_HOST_HEAD_EN (rnndb display/g80_pdisplay.xml:542) and NEVER by writing a status or trigger register, because rnndb names no ack protocol for them — an uncited write is not made. writes=2 (PMC unmask, head enable), both restored at window close ::",
        bus, slot, func, vec, wire, pmc_entry, PMC_INTR_BIT_PDISPLAY, en_entry, head,
        IRQ_WINDOW_VBLANKS, IRQ_STORM_CAP,
    );
}

/// **§R3 — RUN AND RESTORE.** Count ISR entries against the vblank count, re-arm the enable the ISR
/// disarms, and at the end put PMC and the head enable back and read them back.
fn rung3_run(bar0: usize, head: usize, n: u64) {
    let vb = 1u32 << DISP_INTR_HEAD_BIT_VBLANK;
    WIN_SAMPLES.fetch_add(1, Ordering::Relaxed);
    let elapsed = n.saturating_sub(WIN_AT.load(Ordering::Relaxed));
    if elapsed < IRQ_WINDOW_VBLANKS && !IRQ_STORMED.load(Ordering::Relaxed) {
        // RE-ARM. The ISR disarms at the enable (the only ack this rung is licensed to make), so
        // without this the window would measure exactly one delivery. One write per DELIVERY, not
        // per vblank: an enable that is already set is left alone.
        let en = unsafe { mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head)) };
        if en & vb == 0 {
            unsafe { mmio_write(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head), en | vb) };
            WIN_SEEN.fetch_add(1, Ordering::Relaxed);
        }
        return;
    }

    // ── CLOSE. Order matters: stop the ISR touching MMIO, then mask, then restore. ──
    IRQ_LIVE.store(false, Ordering::Release);
    let pmc_entry = IRQ_PMC_ENTRY.load(Ordering::Relaxed);
    let en_entry = WIN_EN_ENTRY.load(Ordering::Relaxed);
    unsafe {
        mmio_write(bar0, regs::NV_PMC_INTR_EN, pmc_entry);
        mmio_write(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head), en_entry);
    }
    let pmc_back = unsafe { mmio_read(bar0, regs::NV_PMC_INTR_EN) };
    let en_back = unsafe { mmio_read(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head)) };
    let irq = IRQ_COUNT.load(Ordering::Relaxed);

    // **THE MODE IS DECIDED HERE, BY THE WIRE.** Not by the knob, not by a `cfg`, not by whether
    // the registers were cited: by whether the GK107 delivered an interrupt to this kernel.
    IRQ_MODE.store(irq > 0, Ordering::Release);

    let clean = pmc_back == pmc_entry && en_back == en_entry;
    serial_println!(
        ":: kepler: vblank-intr vector close head={} irq={} vbl_delta={} rearms={} wire={} vector={:#04x} storm={} pmc_restored={:08X} pmc_readback={:08X} en_restored={:08X} en_readback={:08X} verdict={} mode={} :: — irq= is ISR ENTRIES and vbl_delta= is HEAD_STAT.VERT[31:16] edges over the same window, so irq/vbl_delta is the delivery ratio and irq=0 with vbl_delta>0 is the HONEST REFUSAL: the raster ran, the source was enabled and cited, and nothing was delivered. writes=4 total, all restored and read back. mode= is set by THIS number and by nothing else ::",
        head, irq, elapsed, WIN_SEEN.load(Ordering::Relaxed), IRQ_WIRE.load(Ordering::Relaxed),
        IRQ_VECTOR1.load(Ordering::Relaxed).saturating_sub(1), IRQ_STORMED.load(Ordering::Relaxed) as u32,
        pmc_entry, pmc_back, en_entry, en_back,
        if clean { "clean" } else { "DIRTY" }, mode_str(),
    );
    LADDER.store(LADDER_DONE, Ordering::Release);
}

/// **§R3's ISR.** Count the entry, DISARM at the cited enable, EOI. Nothing else, ever.
///
/// **THE ACK IS A DISARM AND THAT IS A CITATION DECISION, not a style one.** rnndb gives
/// `INTR_HEAD_STATUS` (`:523`) an `INTR_HEAD_TRIGGER` partner (`:524`) and says nothing about how a
/// latched bit is cleared. Writing a status register on the strength of a naming convention is
/// exactly the guess this module's `NOT-IN-TREE` idiom exists to refuse — so the handler instead
/// writes `INTR_HOST_HEAD_EN` (`:542`) back to the value [`rung3_arm`] captured, which is a
/// register this file cites, a value this file owns, and a de-assertion that cannot storm.
/// [`rung3_run`] re-arms on the next vblank, so the window still measures many deliveries.
///
/// [`IRQ_STORM_CAP`] is the second floor: if the disarm turns out NOT to deassert a level-triggered
/// INTx, the handler cuts PDISPLAY at PMC bit 26 and the boot survives with a printed result.
extern "x86-interrupt" fn kepler_vblank_isr(_f: x86_64::structures::idt::InterruptStackFrame) {
    let n = IRQ_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let bar0 = VB_BAR0.load(Ordering::Acquire);
    if bar0 != 0 && IRQ_LIVE.load(Ordering::Acquire) {
        let head = IRQ_HEAD.load(Ordering::Relaxed) as usize;
        if head < 4 {
            unsafe {
                mmio_write(
                    bar0,
                    disp_head(DISP_INTR_HOST_HEAD_EN, head),
                    WIN_EN_ENTRY.load(Ordering::Relaxed),
                );
                if n >= IRQ_STORM_CAP {
                    mmio_write(bar0, regs::NV_PMC_INTR_EN, IRQ_PMC_ENTRY.load(Ordering::Relaxed));
                    IRQ_STORMED.store(true, Ordering::Relaxed);
                }
            }
        }
    }
    crate::arch::apic::eoi();
}

// ── RUNG 2 — the wait arm's source and its primitive ──────────────────────────────────────────

/// The monotonic vblank counter, or `None` when no source is counting this boot. `video::beam::hold`
/// reads this to decide whether the wait arm exists at all.
///
/// Under the fixture this answers from the SIMULATED source, which is what makes the fixture drive
/// the real arm rather than a copy of it.
#[inline]
pub fn counter() -> Option<u64> {
    match SIM_MODE.load(Ordering::Relaxed) {
        0 => {
            if VB_HEAD1.load(Ordering::Acquire) == 0 || VB_COUNT.load(Ordering::Relaxed) < 2 {
                None
            } else {
                // KVBLANK2 — THE UNION OF THE TWO SOURCES, and it is a union rather than a switch
                // on purpose. Both terms are monotonic, so the sum is monotonic, and it advances
                // when EITHER the polled counter ticks or the §R3 ISR fires. A switch would have
                // made `mode=irq` a cliff: the moment the wire delivered one interrupt the wait
                // would have stopped watching the counter that actually advances inside a hold
                // (`hold` runs IRQ-masked — see §R2b), and every wide-zone present would have
                // regressed to the give-up budget. The sum can only ever shorten a wait.
                Some(
                    VB_COUNT.load(Ordering::Relaxed)
                        .wrapping_add(IRQ_COUNT.load(Ordering::Relaxed)),
                )
            }
        }
        // A counter the TIMER advances, one tick per simulated period.
        1 => {
            let p = SIM_PERIOD_CYC.load(Ordering::Relaxed).max(1);
            Some(crate::arch::now_cycles().saturating_sub(SIM_T0.load(Ordering::Relaxed)) / p)
        }
        // THE GO-RED: a counter that never advances. A counter that does not move is not a raster,
        // and the wait must give up on the spin's own budget rather than hang the present.
        _ => Some(0),
    }
}

/// Rung 1's measured phase — the raster line at which the vblank edge is OBSERVED, `None` until an
/// edge has been phased. `video::beam::hold` takes the wait arm only when this line is outside the
/// rect's hazard zone, which is the sense in which rung 2's threshold is DERIVED from rung 1.
#[inline]
pub fn edge_phase() -> Option<u32> {
    match VB_PHASE1.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v - 1),
    }
}

/// RUNG 2's primitive: wait for the vblank counter to leave `from`, never past `deadline_cyc`.
///
/// THE DEADLINE IS THE CALLER'S, and that is load-bearing: `beam::hold` passes `t0 + GIVEUP_FRAMES *
/// FRAME_US` — its OWN give-up deadline, taken from its OWN `t0` — so a wait that runs out lands in
/// the spin arm whose first budget check fires immediately. The present therefore costs ONE give-up
/// budget whichever arm it took, never two, and the spin arm needed not one character changed.
///
/// Returns `(advanced, waited_cyc)`. There is no blocking primitive on this path — `hold` runs
/// IRQ-masked on the presenting core inside `COMP_GATE`, so there is no scheduler to yield to and
/// no wait queue to sleep on — so this is a `spin_loop` yield between counter reads. The difference
/// from the arm it replaces is NOT that the CPU stops: it is that each iteration reads an atomic
/// this core already owns instead of issuing an uncached BAR0 MMIO read across the PCIe link, and
/// that it is woken by ONE event instead of polling a position.
///
/// The budget is `GIVEUP_FRAMES * FRAME_US` — the SAME budget the spin arm gives up on, asserted
/// equal at compile time above — so a source that stops counting costs a present exactly what a
/// raster that stops moving costs it, and not one microsecond more.
pub fn wait_next_edge(from: u64, deadline_cyc: u64) -> (bool, u64) {
    let t0 = crate::arch::now_cycles();
    // KVBLANK2 §R2b — THE SAMPLER, and without it this loop could never have returned `advanced`
    // on hardware. `counter()`'s polled term is advanced by `note`, `note` is fed by
    // `scanout_beam()`, and NOTHING in this loop or in `beam::hold`'s wait arm above it called
    // `scanout_beam()` — so `VB_COUNT` was frozen for the whole wait and every armed present paid
    // the full `GIVEUP_FRAMES` budget before falling through to the spin. See the module doc.
    // Under the fixture the hardware source is not the one being driven, so the sample is skipped
    // and B145's two recorded verdicts are unchanged.
    let sample = SIM_MODE.load(Ordering::Relaxed) == 0;
    loop {
        if sample {
            let _ = crate::arch::scanout_beam();
        }
        match counter() {
            Some(c) if c != from => {
                let dt = crate::arch::now_cycles().saturating_sub(t0);
                VB_WAITS.fetch_add(1, Ordering::Relaxed);
                VB_WAIT_US.fetch_add(cycles_to_us(dt), Ordering::Relaxed);
                return (true, dt);
            }
            Some(_) => {}
            // The source went away mid-wait. Fall back to the spin rather than invent an edge.
            None => {
                let dt = crate::arch::now_cycles().saturating_sub(t0);
                VB_WAIT_GAVEUP.fetch_add(1, Ordering::Relaxed);
                return (false, dt);
            }
        }
        let now = crate::arch::now_cycles();
        let dt = now.saturating_sub(t0);
        if now > deadline_cyc {
            VB_WAITS.fetch_add(1, Ordering::Relaxed);
            VB_WAIT_US.fetch_add(cycles_to_us(dt), Ordering::Relaxed);
            VB_WAIT_GAVEUP.fetch_add(1, Ordering::Relaxed);
            return (false, dt);
        }
        core::hint::spin_loop();
    }
}

/// Rung 2 bookkeeping: the wait returned, but the confirming raster read landed back inside the
/// hazard zone, so `hold` fell through to the spin. Correctness is the spin's; this counts the
/// times the phase prediction did not hold.
#[inline]
pub fn note_recheck() {
    VB_WAIT_RECHECK.fetch_add(1, Ordering::Relaxed);
}

// ── THE FIXTURE (rung 2) ──────────────────────────────────────────────────────────────────────

/// KVBLANK rung 2's self-test: drive [`wait_next_edge`] — the arm `video::beam::hold` calls, not a
/// copy of it — from a SIMULATED counter the timer advances, and prove both halves.
///
/// * **PASS** when the wait returns `advanced = true` within one simulated period.
/// * **GO-RED** (`SIM_MODE = 2`, a counter that never advances) when the wait gives up on the SAME
///   `GIVEUP_FRAMES` budget the spin uses and says so.
///
/// Both halves run on EVERY armed boot and both are printed, because a fixture that only ever
/// exercises its green half is not a gate (LAWS §5). The go-red costs one give-up budget —
/// 2 x 16.667 ms — once per boot.
///
/// It runs on a machine with NO Kepler: q35 answers `:: kepler: no-device ::` and neither
/// `arm_pmc_pdisplay` nor `arm` is ever reached there, so a simulated source is the ONLY way this
/// property is verified anywhere but on the bench.
pub fn selftest_once() {
    if SELFTEST_DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let period = us_to_cycles(crate::video::beam::FRAME_US);

    // ── PASS half — a counter the timer advances at one tick per simulated frame ──
    SIM_PERIOD_CYC.store(period, Ordering::Relaxed);
    SIM_T0.store(crate::arch::now_cycles(), Ordering::Relaxed);
    SIM_MODE.store(1, Ordering::Release);
    let from = counter().unwrap_or(0);
    let deadline = crate::arch::now_cycles()
        .saturating_add(us_to_cycles(GIVEUP_FRAMES.saturating_mul(crate::video::beam::FRAME_US)));
    let (adv, cyc) = wait_next_edge(from, deadline);
    let waited_us = cycles_to_us(cyc);
    let bound_us = crate::video::beam::FRAME_US;
    let pass = adv && waited_us <= bound_us;
    serial_println!(
        ":: kepler: vblank selftest arm=wait sim=timer period_us={} from={} advanced={} waited_us={} bound=waited_us<={} :: {} ::",
        crate::video::beam::FRAME_US, from, adv as u32, waited_us, bound_us,
        if pass { "PASS" } else { "FAIL" },
    );

    // ── GO-RED half — a counter that never advances ──
    let g0 = VB_WAIT_GAVEUP.load(Ordering::Relaxed);
    SIM_MODE.store(2, Ordering::Release);
    let deadline2 = crate::arch::now_cycles()
        .saturating_add(us_to_cycles(GIVEUP_FRAMES.saturating_mul(crate::video::beam::FRAME_US)));
    let (adv2, cyc2) = wait_next_edge(0, deadline2);
    let waited2_us = cycles_to_us(cyc2);
    let budget_us = GIVEUP_FRAMES.saturating_mul(crate::video::beam::FRAME_US);
    let gaveup = VB_WAIT_GAVEUP.load(Ordering::Relaxed) > g0;
    // THE BOUND IS ONE FRAME WIDE, AND THAT IS THE CLAIM'S OWN RESOLUTION, not a slackened test.
    // What is under test is "the wait gives up on the SAME GIVEUP_FRAMES budget as the spin" — a
    // statement about a count of FRAMES. Asserting `waited_us >= budget_us` exactly measured the
    // wrong thing and FAILED on its first armed gate run at `waited_us=33295 budget_us=33334`: the
    // deadline is computed from a `now_cycles()` taken in this function and the elapsed time from a
    // second one taken inside `wait_next_edge`, and `us_to_cycles`/`cycles_to_us` truncate in
    // opposite directions — 39 us of clock plumbing on a 33 334 us budget, 0.12%, and on TCG at
    // that. So the bound is BOTH-SIDED and its unit is the frame: the wait must have burned at
    // least `GIVEUP_FRAMES - 1` whole frames (it did not return early) and at most
    // `GIVEUP_FRAMES + 1` (it did not overrun the spin's budget). A wait that returns in one frame
    // instead of two still FAILS this, which is the regression the go-red exists to catch.
    let lo = budget_us.saturating_sub(crate::video::beam::FRAME_US);
    let hi = budget_us.saturating_add(crate::video::beam::FRAME_US);
    let red_ok = !adv2 && gaveup && waited2_us >= lo && waited2_us <= hi;
    serial_println!(
        ":: kepler: vblank selftest arm=wait sim=stuck advanced={} gaveup={} waited_us={} budget_us={} bound={}<=waited_us<={} (GIVEUP_FRAMES budget, +/- one frame) :: {} ::",
        adv2 as u32, gaveup as u32, waited2_us, budget_us, lo, hi,
        if red_ok { "GO-RED-OK" } else { "GO-RED-FAILED" },
    );

    // Put the source back and un-count the fixture's own two waits, so the boot's `vbwaits=` census
    // measures the COMPOSITOR and not this function.
    SIM_MODE.store(0, Ordering::Release);
    VB_WAITS.store(0, Ordering::Relaxed);
    VB_WAIT_US.store(0, Ordering::Relaxed);
    VB_WAIT_GAVEUP.store(0, Ordering::Relaxed);
    VB_WAIT_RECHECK.store(0, Ordering::Relaxed);
}
