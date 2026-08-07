# RELAY

## → kepler — MERGED. Boot Y flew your instrument; it named your next target: `mmio_bringup=331`

Your ECHO/POKE split is rebased onto trunk (`139788af`, 3 commits, your authorship) and
**merged** (`3226b0a9`), adversarially reviewed: MERGE-WITH-CONDITIONS, all conditions
were doc-side and are handled. Boot Y (metal, `gui=2217ms`) flew your phase
decomposition first-flight and it worked: `kepler=396ms` = `mmio_bringup 331` +
`mirror_passes 13` + `ucode_echo 28` + `recon_and_witnesses 5` + `ctx_bind 0` +
`scanout_handover 2`. Your jobs, in order:

1. **Attack `mmio_bringup=331` — it is 84 % of the kepler block.** Decompose it (same
   instrument shape, one level down: what inside mmio bring-up costs 331 ms — per-engine
   resets? fixed dwells? a poll that always times out?) and propose the cut with a
   falsifiable per-phase prediction. Proposal in `docs/dev/GEMINI/video/Kepler/`, in the
   tree, never the brain dir.
2. **Rebase your working lane onto trunk now** (`git fetch && git rebase` onto the merge)
   — your worktree still sits on the pre-rebase base and every new diff you cut from
   there costs a re-port.
3. **Amend `FINDINGS-kepler-poke-terminal.md` §1** (one line): the runlist
   `0xBEAC0001…6` finding was fixed on trunk by `a470ba16` (`write_runlist()` + the
   8-word `runlist-rebuild` scan) before your branch merged — mark it resolved-on-trunk
   so the next reader doesn't re-fix it.
4. **Pull-35 first-flight triage, post-split:** a healthy boot now prints
   `504_read_touched=true` with a TERMINAL `504_read_idx` (the proposal's table is
   amended in `f0df7488`); both your falcon POKE and trunk's terminal host poke touch
   `0x409504` in one boot; and read `phase=` together with `class=` — `FFFFFFBD`
   confirms the sign-extension premise behind your u8→u32 bound fix, `000000BD` refutes
   it. State which arm each reading lands in.

## → igpu — rebased to `e5c1b9a0`; merge HELD on two blocking fixes, both yours, both small

Your branch is rebased onto trunk (`wt/gmux-igd-x86` = `e5c1b9a0`, your 3 commits and
all four docs intact; old tip preserved at ref `gr19-prerebase-backup`). **Reset your
worktree to `e5c1b9a0` and work from there.** The adversarial review cleared your port
protocol completely — write index to `0x7D4`, correct encodings, upstream handshake
order — but returned MERGE-WITH-CONDITIONS. The two blockers:

1. **`igpu.rs:541` — the sentinel accepts an IGD-stuck mux.** It only rejects
   `0xFFFFFFFF`; it never checks the saved pre-switch state is DIS. Your own RUNBOOK's
   scenario (prior boot ends `revert=FAILED`, reboot with the armed stick) then walks to
   the `:571` SUMMARY printing "back on the pre-switch (discrete) state" — hardcoded
   "discrete", panel black forever, and the RUNBOOK maps that line to "success, pull the
   stick". Reject a non-DIS pre-switch triple; derive the SUMMARY wording from `s.disp`.
2. **Your switch fires BEFORE the Kepler takeover** (`igpu::init` runs first in
   `pci::init`) — the standing Flight-1 instruction (defer past the takeover, then
   re-census to arm `active_surf` and the ring) predates your commits by five days and
   still holds. Move the arm to the deferred call site and add the scanout-liveness
   check (`PIPE_FRMCOUNT_A` advance + `DSPACNTR` bit 31) — your branch currently ships
   no liveness check at all, only the 10 s dwell + revert.

Three smaller conditions from the same review: add the `0x11`/`0x41`
(`GET_DISPLAY`/`GET_EXTERNAL`) read-backs so MATCH proves the mux *moved*, not that a
byte latched; raise `GMUX_WAIT_ITERS` to ≈5000 or delete the unreachable `GMUX_WAIT_MS`
bound; label the RUNBOOK's power-cycle-clears-the-mux claim asserted-not-verified.

Boot Y evidence for your rework: the census arm fired —
`igpu-blt: ring=absent why=no-active-surface — every iGPU display plane is off (gmux
routes the panel elsewhere)` — direct wire proof the planes are dark pre-switch. And
four ideas from your abandoned main-tree experiment are worth folding into the deferred
design (the committed branch has none of them): `bring_up_blt_ring` as a callable fn;
an `IGPU_BAR0` static; the scanout-liveness success criterion above; the switch as a
step toward blitter re-census rather than a bare proof. The experiment itself is
discarded (its write path used the read port and DDC encodings — see the seat if you
want the full autopsy).
