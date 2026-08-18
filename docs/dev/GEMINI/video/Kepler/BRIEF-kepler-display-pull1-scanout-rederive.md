STATUS: BRIEF — answered by `PROPOSAL-kepler-display-pull1.md` (this directory)

# BRIEF — Kepler display pull 1: the panel is Kepler's — re-derive GK107 scanout

Coordinator-authored (2026-07-22, post sitting #10 boot 1). The display lane
(this directory) pivots from Intel to Kepler: the gmux, on a PROVEN protocol,
says the discrete GPU owns the panel and is powered at every observed
instant. The iGPU arc is CLOSED (its all-dead state is correct hardware
behavior). Sitting #5's redirect is reversed.

## The anomaly to explain

Sitting #5 read `evo=00000000 crtc=00000000` on all 4 heads and called the
Kepler display engine idle — but the panel provably belongs to Kepler and the
GOP console scans from 0x90020000 during Option-boot. Either:
(a) the pull-5/6-era PDISPLAY head/scanout register decode is wrong for this
    exact part (we've been reading the wrong offsets — precedent: every other
    wall on this branch so far was a wrong-decode), or
(b) firmware tears the Kepler display engine down at ExitBootServices while
    the mux stays pointed at it — the black panel then IS the teardown.
The Point-0/1/2/3 trace pattern that settled this question for the iGPU is
the template: same three-to-four-point snapshot, Kepler PDISPLAY edition.

## What the proposal must cover

1. **Module split FIRST (mechanical milestone 1):** display-side code moves
   out of kepler.rs into its own file (e.g. `kepler_display.rs`) so this lane
   and the PFIFO/fence lane stop sharing a file. No behavior change; both
   feature gates unchanged; full-knob build proves byte-equivalent serial
   output. This unblocks parallel pulls in the two Kepler lanes.
2. **Scanout register re-derivation for GK107 (GF119-family disp):** the
   armed/active head state, scanout surface address, and enable bits —
   re-derived from rnndb disp XMLs (nv_d0_disp or equivalent) rather than
   the pull-4/5-era guesses; each offset cited. Explicitly diff the new
   decode against the refuted candidates (0x616100-block slicing, the evo
   0x490 channel) and say why the old reads returned zeros.
3. **Boot-time trace points:** add PDISPLAY snapshots at the bootloader
   Point-0/1/2 (same boot-info carry pattern, feature-gated; ABI law) so one
   boot splits hypothesis (a) from (b) exactly like the iGPU trace did.
4. **Read-only.** No display writes this pull — decode + trace only.

Standing rules: cleanroom (rnndb/envytools facts; nouveau forbidden), bounded
polls, `:: kdisp:` prefix for new rows, full-knob land-review with
strings-proof in BOTH artifacts, main.rs arch gate untouched.

Metal owed: sitting #11.
