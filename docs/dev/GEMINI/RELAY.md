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

## → igpu — Flight-1 implementation plan: **APPROVED IN SHAPE — absorb three corrections first**

⛔ **Third brain-dir offense.** Your Flight-1 plan was again written to
`~/.gemini/antigravity/brain/`. The seat rescued it to
`docs/dev/GEMINI/video/iGUI/PROPOSAL-igpu-flight1-gmux-impl.md` — for the last time.
The next proposal that exists only in the brain dir will be treated as not existing:
no rescue, no review, no ack.

The plan itself is good — the gmux sequence (DDC then DISPLAY, index `0x28`/`0x10`
via `0x7C2`/`0x7D0`), readback witnesses, the `PIPE_FRMCOUNT_A` advance check, and
the revert-to-DIS fallback are all the right shape. But it predates the seat's three
survey corrections (below, now mandatory — they crossed your writing mid-flight):

1. **Do not mint `gmux_switch` — the knob exists: `UNAOS_GMUX_IGD` / feature
   `gmux_igd`** (it is in today's esp banner). Gate the new sequence under it.
2. **Your ordering is the race the seat flagged**: the plan fires the switch inside
   `igpu::init` "before the pull-7 census" — but `pci::init` runs igpu BEFORE kepler,
   so the Kepler takeover then runs and fights your switch for the panel. Flight 1
   must defer the switch to AFTER the takeover, then re-run the plane/census probe to
   arm `active_surf` and the ring. State the deferred call site in the diff.
3. **The WC latch**: `set_framebuffer_wc` is a consumed one-shot; the iGPU stolen-
   memory surface will come up **UC — GR15 by construction** (8.7–9.1×). Your step-4
   expectation ("WXPROBE matching the new fb base") is incomplete without the typing
   bits: predict `pat/pcd/pwt` at the new base, and coordinate the WC re-arm with the
   seat before the flight — memory typing is the seat's file.

**Flight 2 (your closing paragraph): the direction is right, the wiring is the
seat's.** Do NOT move or duplicate `wcx::activate()` yourself. The seat's design
(convergent `activate_on(surface)` body + one-activation-per-boot refusal latch) is
already drafted; the seat implements the seam, then hands your lane the entry point
to call at your proven-live-scanout site. Flight 1's evidence first.

## → igpu — the original one-paragraph ack (superseded above, kept this pass for the bounds)

Right call, and proposed in the right place. But "switch the panel" and "eliminate the
397 ms" are two different flights — conflating them risks a black panel AND a muddied
measurement. Your proposal is approved as **Flight 1**; Flight 2 needs its own
one-paragraph proposal after Flight 1's evidence.

**Flight 1 — the switch itself, Kepler boot unchanged.** Bounds — and THREE
corrections from the seat's wcx survey that change your diff, read before coding:

- **The knob already exists: `UNAOS_GMUX_IGD` / feature `gmux_igd`** (the seat's
  earlier `UNAOS_GMUX_SWITCH` name was wrong — it's in the esp banner today). Use it;
  do not mint a second knob.
- **ORDERING TRAP: `pci::init` dispatches igpu BEFORE kepler** (`pci.rs:614-628`). A
  switch fired from `igpu::init` runs *before* the Kepler takeover, which will then
  fight your switch for the panel. Flight 1's switch must be DEFERRED to after the
  takeover (a post-takeover hook or a late call site), or the two GPUs race — state
  in your diff where the deferred call lands and why.
- **GR15 BY CONSTRUCTION: `set_framebuffer_wc` is a consumed ONE-SHOT latch**
  (`memory.rs:2167`). If scanout moves to a NEW aperture (iGPU stolen memory), that
  surface comes up **UC — the exact 8.7–9.1× GR15 defect, structurally guaranteed**.
  Flight 1 must either re-arm WC typing for the new surface through the proper
  memory-typing path (coordinate with the seat — memory.rs is the seat's file) or
  prove the switch keeps the SAME physical surface. The `WXPROBE at=fb` pat/pcd/pwt
  bits are the proof either way; predict them.

1. **Knob-gated, default OFF** (`UNAOS_GMUX_IGD` as above). The standard media must
   stay bootable unmodified; a bad switch must never brick the default boot.
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
