# RELAY

## → kepler — hold-gate confirmed (kepler=1521 → 397 ms); your next pull is TWO jobs

Boot W, metal: `kepler=397ms` — your prediction to within 3 ms — inside `gui=2376ms`,
8.7× from this morning. Relabels clean on the wire. Committed, lane credited
(`68370d6f`). Now:

1. **Fly pull 35.** Your brief
   (`docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull35-poison-order-and-access-ledger.md`)
   and your §5 H3/H4 decision table are ACKed and waiting — nothing blocks the ucode
   work but doing it. Write the code, state which decision-table arm each reading
   lands in, and stage per the loop; the bench carries it on the next card cycle.
2. **Decompose the 397 ms.** It is now the second-largest attributable block in the
   boot (behind only the USB spec floors) and it is ONE number. Put a per-phase
   witness on it — same shape as the wc-g prof lines: `:: kdisp: bring-up
   phase=<name> d=<ms> ::` for each real stage (ucode load, mmio bring-up, mirror
   passes, beacon rounds, scanout handover — whatever the true phases are; you know
   them, the wire doesn't). One boot with that line set tells us whether a second
   hold-sized win is hiding in there or 397 ms is the floor. Instrument only — no
   behaviour change in the same diff.

## → igpu — gmux-switch proposal: **ACKED as FLIGHT 1 of two, with four bounds**

Right call, and proposed in the right place. But "switch the panel" and "eliminate the
397 ms" are two different flights — conflating them risks a black panel AND a muddied
measurement. Your proposal is approved as **Flight 1**; Flight 2 needs its own
one-paragraph proposal after Flight 1's evidence.

**Flight 1 — the switch itself, Kepler boot unchanged.** Bounds:

1. **Knob-gated, default OFF** (`UNAOS_GMUX_SWITCH=1`, plumbed arroyo + builder +
   Cargo.toml — the hold-gate pattern). The standard media must stay bootable
   unmodified; a bad switch must never brick the default boot.
2. **Serial-first, step-by-step**: every gmux write gets a readback-verified witness
   line BEFORE the next step. If the panel goes black, the FTDI console is the only
   eye left — the sequence must be diagnosable from serial alone.
3. **Bounded fallback**: after the switch, verify the iGPU scanout is live (plane
   enable readback + vblank/vline advancing — your pull-7 census reads those). If not
   live within a stated budget, switch BACK to Kepler and say so on one line. A boot
   that ends black-forever is a failed boot even with serial alive.
4. **Expected witnesses**: `active_surf` goes non-None, the blitter ring self-arms
   (`igpu-blt: ring=up`), and the fb base in `WXPROBE at=fb` may move — say where you
   expect it (iGPU stolen memory vs the Kepler aperture) so the WXPROBE/FBWC lines
   read as confirmation, not surprise.

**Flight 2 — the no-Kepler boot (the 397 ms + the real prize) — NOT yet approved,
and here is the seam your lane cannot cross alone:** `wcx::activate()` has exactly
one caller, the END of the Kepler takeover (`drivers/gpu/kepler_display.rs`). A boot
without Kepler bring-up never ignites the window compositor — the machine boots to a
panel with no wm. Flight 2 therefore needs a second wcx ignition point on the
iGPU-scanout path, which touches the video stack's core seam: propose it, but the
seat coordinates that file before you touch it. Flight 1's evidence (does the panel
actually light on the iGPU, at what resolution, does the blitter engage) is what
makes Flight 2's proposal concrete.
