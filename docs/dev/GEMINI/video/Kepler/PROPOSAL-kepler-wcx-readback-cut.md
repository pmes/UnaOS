# PROPOSAL: WCX Readback Cut and Phase Ledger Clock Fix

## Goal
Reduce the ~190 ms uncached readback cost inside `desktop_uefi::activate()` without blinding the compositor's witness instrumentation, and fix the `phase!` macro's 13 ms APIC tick deficit against the TSC wall clock.

## 1. Phase Ledger Clock Fix
**Issue:** `crate::arch::ms()` reads the APIC tick counter, which loses ticks when interrupts are masked (e.g., during the 14 ms `fill_screen`). This causes a systematic ~13 ms under-reporting of the `kdisp_takeover` span on both boots.
**Proposed Change:** Update the `phase!` macro in `kepler.rs` to read from the invariant TSC. We will sample `crate::clock::monotonic()` (or its equivalent for millisecond resolution) instead of `arch::ms()`, ensuring `d=...` aligns perfectly with the `[NNNNNNms]` log prefix.

## 2. Sparsifying Witness Readbacks
Every uncached BAR0 read of the panel costs ~1.7–2.3 µs. Deletion is not on the table, but we can pay less by sparsifying the probe lattices.

### A. `wcg::PAYGO_LATTICE_N` (The Guard)
- **Current:** `lattice16` (1 in 256 pixels). For `win=1` (1312x736), this scans 60,352 probes and costs ~102.5 ms (1.70 µs/probe).
- **Proposal:** Increase the sparsity to `lattice64` (1 in 4096 pixels) for large desktop-class windows during paygo initialization.
- **Predicted Saving:** Probes drop from 60,352 to ~3,772. At 1.7 µs/probe, the cost drops to ~6.4 ms. **Saving: ~96 ms.**

### B. `wc-d` Verify Cadence / Coverage
- **Current:** `lattice16` verify for `win=1` (5,248 probes, 23 ms) and `win=2` (24,576 probes, 56 ms).
- **Proposal:** Sparsify the background verify sweep to `lattice64` for stable windows, or limit the per-frame verify budget to a fixed number of probes (e.g., 4096 probes max per verify pass).
- **Predicted Saving:** If bounded to 4096 probes at ~2.2 µs/probe, the sweep costs ~9 ms total instead of 79 ms. **Saving: ~70 ms.**

### C. `move_vacate_probe` (Feature `witness`)
- **Current:** `coverage=full` for `win=3` (8x8 window). It scanned 4,096 probes and cost 8 ms (1.95 µs/probe).
- **Proposal:** Reduce the vacate probe coverage from `full` to `lattice16` or `lattice4`, as a full readback of even small evicted regions is disproportionately expensive.
- **Predicted Saving:** 4,096 probes drops to 256 probes (at `lattice4`). Cost drops to ~0.5 ms. **Saving: ~7.5 ms.**

## Total Predicted Savings
By tuning these three knobs, the `desktop_uefi::activate()` readback cost will drop from **~190 ms** to **~16 ms**, predicting a net saving of **~174 ms** on the `kdisp_takeover` span without blinding the compositor.
