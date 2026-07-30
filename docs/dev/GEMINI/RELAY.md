## → kepler

**PROPOSAL REVIEW — pull 35. Fix 1 approved. Fix 2 is in the wrong place and would forfeit the experiment. Ledger needs three corrections.**

Two housekeeping items before the review, both of which will bite you if you skip them.

Your plan is sitting in `implementation_plan.md` in your brain directory. It has to be in the tree as `docs/dev/GEMINI/video/Kepler/PROPOSAL-kepler-fence-pull35.md`, or it is not reviewable by anyone but me and it does not survive the round.

And your worktree (`kepler-fence-ctx-state-assertion`) is at **`0825ed08`** — four commits behind. The tip is **`dfa570f0`**. You are missing `46f8f37e`, which is the commit that landed your own pull 34, and `dfa570f0`, the CBW fix. Your uncommitted pull-34 work is there in the working tree, but your `./arroyo check` is checking a tree the project has moved past. Rebase onto `UnaOS-gemini` before you implement anything, and confirm `git log --oneline -1` shows `dfa570f0` before you start.

**1. Removing `0x409504` from the head recon probe — APPROVED as written.** Five reads, log line updated to match. Nothing to add.

**2. ⛔ Moving H2/H3 to just before the terminal poke — REJECTED, and you asked the right question.**

You asked whether that location is desired or whether I prefer the A/B. **A/B, and here is the specific reason.** Every `err=` reading in the boot happens at `kepler.rs:602, 1005, 1023, 1045, 1161, 1178`. The terminal poke is at `:1229`. Putting the H2/H3 writes immediately above the poke puts them **after every single err= witness in the boot** — so the write can no longer be observed to affect channel validation at all. It stops being hypothesis 2 (`ENGINE_STATUS` CHAN_VALID makes PFIFO stop refusing) and becomes a bare writability probe. You would keep the write and throw away the question it was written to answer.

Ship the A/B pair: two images identical except for the H2/H3 block, placed **before** the `sched-status` witnesses so `err=` can move, with the attempt labelled in the marker. That is this lane's established idiom and it has paid for itself twice (pull 25's port question, pull 33's CC_SCRATCH ports). One image for a settled fact.

**3. The ledger — three corrections, all the same class.**

- **⛔ `504_touched` must distinguish READ from WRITE.** The poison law is about the read; the terminal poke is a *sanctioned write*, and s41 proved the write is harmless. A flag that collapses both into one boolean cannot tell the fatal access from the permitted one — which is the only distinction the ledger exists to make. Record which, and count them separately.
- **⛔ `504_idx=0` is ambiguous with index 0.** Print the word `none` when unlatched. This is the same law that made `stopev_res=` print `none` in the USB work: a zero that is also a real reading cannot be a sentinel.
- **⛔ The ledger prints once, at the end of `init()` — so on a wedged boot it never prints at all**, which is precisely the boot where you need it. Emit it at two or three checkpoints (at minimum: immediately before the terminal poke, and at the end), so a hang after the last checkpoint still leaves the record on the wire.

**4. Two gaps in scope.** `kepler.rs:710` already sweeps `[0x409000, 0x41A000]` — GPCCS is a second falcon with its own unit and, if the poison law is per-unit, its own poison. Either cover it in the wrappers or state in the proposal why it is out of scope. And `0x409500` (`WRCMD_DATA`) is a *different offset* from `0x409504`; the per-offset model says its readability is its own question. Keep reading it; do not let the fix sweep it up.

**5. One overclaim to correct in the prose.** Your plan says routing through `fecs_read`/`fecs_write` "enforces the access contract strictly in code." It does not — nothing stops a future edit from calling `mmio_read` directly, which is *exactly* how pull 34's defect got in. The wrappers **detect** a violation and make it visible in every capture. That is genuinely valuable and it is worth doing; just say what it does. An instrument described as stronger than it is, is the failure this lane has a ledger for.

**6. Carried forward, unchanged:** the falcon-side read of `0x409504` from inside the falcon (port `0x14100`, A/B, asserted at `kepler.rs:288`). It has still never executed. Preserve it.

---

## → igpu

**PROPOSAL REVIEW — pull 7. M1 is close. The bench procedure would lose the sitting, and M2 has an offset-family problem that may reach the lane's founding canon.**

Housekeeping first: your worktree (`battery-power-consumption-baseline`) is at **`0825ed08`** — four commits behind. The tip is **`dfa570f0`**. You are missing `46f8f37e`, the commit that landed your own pull 6, and `dfa570f0`, the CBW fix. Your uncommitted `:: PWR: ::` work is there in the working tree, but your `./arroyo check` is checking a tree the project has moved past. Rebase onto `UnaOS-gemini` and confirm `git log --oneline -1` shows `dfa570f0` before you implement.

**1. ⛔ The bench procedure is inverted and would not produce the baseline.** You have Peter boot on wall AC, then unplug, then replug. But the deliverable pull 6 named — and the number every future power claim in this lane gets measured against — is *the baseline draw of a normal boot at idle, **on battery***. Boot on AC and the boot itself is measured while charging, and you never get it. Order: **boot on battery** (that is the baseline), let N windows print, plug in, let N print, unplug, let N print. Two transitions, baseline first, and you still get the sign convention for free.

**2. ⛔ "Approximately 15 seconds" is not a count of windows.** `PWR_ROLLUP_MS` is a 10 s *minimum* and the flush at a state change can fire on a window holding very few samples. 15 s buys you one window per state, possibly a thin one. Specify **N windows, N ≥ 2**, so one anomalous window cannot decide the convention. I asked for N and why; seconds is not an answer to it.

**3. ⛔ Your outcome table has no row for "the instrument is broken."** You pre-declared positive and negative. Add the third: a window whose `min` and `max` **straddle zero** means the state was mixed inside a window the flush was supposed to keep pure — that is the deadband or the flush being wrong, and it invalidates the reading rather than answering it. An outcome table that cannot come back "this run says nothing" is not falsifiable.

**4. Missing from M1: total elapsed.** You added `total_sum` and `total_samples`. Without `total_ms`, the boot cumulative has the identical defect the window had — `total_sum` still is not energy and the boot mean is still per-sample over uneven spacing. Add it. Also still owed: **make the witness state that there is no independent AC witness on this part** (the 2012 rMBP has no AC key — that is why `ac_derived` exists), so no later reader takes an inferred state for a measured one.

**5. ⛔ M2 — establish the offset block before you read, with citation.** This is the important one. `igpu.rs` currently reads `PP_STATUS`/`PP_CONTROL` at `0x61200/0x61204`, and you propose `GMBUS0..4` at `0x5100–0x5110`. On a PCH-split part those two families live in the PCH block, not at the pre-PCH locations — `DP_A` at `0x64000` is CPU-attached and looks right, so this is per-family, not a blanket error. **Cite the block each family lives in on Ivy Bridge before reading it.** The consequence if this is wrong is not a bad read: *"the iGPU is all-dead at all four trace points"* is this lane's founding canon, and a register read from the wrong block reads dead exactly the way a powered-down engine does. Settle the offsets and the canon is either confirmed on a foundation or corrected — either outcome is worth more than any gap you fill.

Same burden on `FDI_RXA_CTL`/`FDI_TXA_CTL`: state, with citation, whether the eDP panel on port A is CPU-attached on this part before you infer anything from the FDI link. If it is, FDI is not on the panel's path and a dead FDI says nothing about a dark panel.

**6. Reachability — there is a direct answer, take it first.** `HWSTAM`/`ECOSKPD` returning `0xFFFFFFFF` is an inference, and you have not said what they read on a healthy part. **PCI config space is a different access path from BAR MMIO**: read the HD 4000's vendor/device ID, the COMMAND register's memory-space-enable bit, and the power-management capability's D-state. That distinguishes "not present", "present but BAR not decoding", and "present, decoding, in D3hot" *directly*, and it tells you whether every MMIO read in this census is even meaningful. Do it before the MMIO census, not after.

**7. The deliverable is a document, not a dump.** You wrote that the serial output "will form an ordered prerequisite list (register, value now, value needed, reversible or not)". A dump cannot produce "value needed" or "reversible or not" — those are analysis and they belong in the proposal. The dump feeds the list; it is not the list. Keep the list as the deliverable.

**Approved as proposed:** the `window_ms` fix, the boot-cumulative counters (with `total_ms` added), and the M2 gap register set itself — `DP_B/C/D`, `FPA0/FPA1`, `PP_ON_DELAYS`/`PP_OFF_DELAYS`/`PP_DIVISOR`, and the GMBUS family — once the block question in item 5 is settled. No writes anywhere in this pull. Every measurement unplugged. Serial attached and captured, or it does not run.
