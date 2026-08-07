# WHITE BOARD — 2026-08-07 (GR19 closed)

## 1. iGPU Flight 1 cannot light the panel as scoped. Grow the arc, or rescope it?

**Not urgent — GR20 may be able to settle this from the code. It is here because it is a
scope decision with a real cost either way, and because the answer came from the lane
itself rather than from the seat.**

Background. Flight 1 (`wt/gmux-igd-x86`, unreviewed) flips the Apple gmux to route the
panel to the Intel IGD, then waits on `PIPE_FRMCOUNT_A` advancing and `DSPACNTR` bit 31 to
declare the iGPU scanout live. Asked what actually brings that pipe up, the igpu lane
answered in the tree (`PROPOSAL-igpu-flight1-liveness.md`) — and the answer is that
**nothing does**:

> "nothing automatically brings the iGPU pipe or plane up … The firmware did not leave the
> display pipeline configured. Therefore, the answer is: **we must program it** … Flight 1
> must explicitly program the iGPU display pipeline (PLLs, panel power sequencer, link
> training, pipe timings, and plane configuration) to drive the panel."

Two metal boots agree: Boot Y and Boot Z both printed `igpu-blt: ring=absent
why=no-active-surface — every iGPU display plane is off`.

If that holds, Flight 1's liveness test can only ever read zero, its revert always fires,
and the re-census and blitter arming are unreachable **even on a perfect mux switch** — the
arc cannot demonstrate its own objective. The two options are not close in size:

- **A — grow the arc.** Flight 1 gains full IVB display bring-up (PLLs, panel power
  sequencer, link training, pipe timings, plane config). That is a substantial arc of its
  own, on hardware where a mistake is a black panel with only serial to debug it.
- **B — rescope Flight 1 to "prove the mux moved."** Keep the switch, drop the lit-panel
  expectation, and pick a criterion that can read non-zero — the gmux's own
  `GET_DISPLAY`/`GET_EXTERNAL` read-backs rather than iGPU pipe registers. The RUNBOOK stops
  promising a lit panel and an automatic recovery. Display bring-up becomes Flight 1b.

The seat's read is **B first** — it makes the existing work landable and produces the
evidence that would justify A — but the call belongs to whoever is paying for the panel
risk. **A one-word answer is enough; GR20 will carry it into the lane's RELAY.**
