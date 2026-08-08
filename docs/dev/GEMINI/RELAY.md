# RELAY

## → igpu — 🛬 FLIGHT 1B FLEW. Your gate held, your unwind held, and the wall is named: AUX timeout.

Boot AP (metal, 2026-08-08, capture `rmbp-gr16-s73`, boot start ~line 87k):

```
[2436ms] :: igpu-dpy: pre-switch state DDC=0x02 DISP=0x03 EXT=0x21 ::          <- ACCEPTED
[2436ms] :: igpu-dpy: rung=00 name=census ok=1 bdsm=0x8BA00001 ggc=0x00000211
          ggtt0=0x8BA00003 ggtt1=0x8BA01003 aux_ctl=0x014300C8 frmcnt=0x00000000 ::
[2438ms] :: igpu: [AUX] DPCD Read Failed: aux-timeout-error ::
[2439ms] :: igpu: [GMUX] revert read-back: DDC=0x02 DISP=0x03 (TBV) EXT=0x21 (TBV) ::
[2440ms] :: igpu-dpy: LADDER highest=03/10 name=dpcd ok=0 pending=1 gmux=MATCH
          why=aux-timeout-error elapsed_ms=9 ::
```

Round 11's relaxation worked on metal: the Kepler-owned EXT=0x21 passed the gate, the DDC
mux switched, the DPCD read ran, and on the timeout the DisplayUnwind restored the exact
pre-image — `gmux=MATCH`. Nothing to fix in the harness. The flight stopped at rung 3.

**Assignment — round 12: bring up the AUX channel. The census already convicts the lead:**
the teardown-hunt table shows `PP_STATUS=0x00000000 PP_CTRL=0x00000000` at all four probe
points — the iGPU's panel-power sequencer has never been engaged, and eDP AUX with VDD off
times out by design. Your own failure line lists it first ("VDD off"). Work the hypothesis
list in order, read-only first:
1. PP/VDD: what does engaging panel VDD for an AUX-only transaction require on IVB
   (PP_CTRL force-VDD bit, T3 wait, and the honest teardown — VDD must come back OFF in the
   unwind; extend DisplayUnwind with the PP pre-image the way DDC is handled)?
2. AUX clock divider: `aux_ctl=0x014300C8` — decode the 2X divider field against the IVB
   rawclk and say whether it is plausible before touching anything.
3. Only if 1-2 dead-end: bad offsets / panel-not-on-AUX (the gmux DDC route vs the eDP AUX
   pins — cite, don't guess).
Rules unchanged: every write carries its justifying read; every write lands in the unwind
with its pre-image; falsifiable prediction before the next flight; build on CURRENT trunk
(fetch first — it moves fast today); ./arroyo check yourself, all legs. Hand back through
this RELAY.

## → kepler — your FENCE experiment flew on Boot AO. Hypothesis 3 is REFUTED by metal.

```
[2424ms] :: kepler: recon eng_trig_pre=00000000 ::
[2424ms] :: kepler: H3 arm=Did-not-work (STRIPPED) ::
```

No null result (pre read 0 — the earlier H3 ring had been consumed), the placement write
took, and PFIFO STILL stripped VALID. The ctxctl-handshake hypothesis is dead: even rung
immediately pre-VALID, the strip stands. That is a clean negative and it narrows the space:
the strip is not gated on the host-side handshake state at write time. Your next round
starts from the remaining dynamic candidates in your own study (falcon-side context state;
the channel's engine binding at submit) — pick with evidence from the banked captures
(s34/s35/s37 + AO), propose ONE experiment with its evidence chain, prediction, and unwind,
on CURRENT trunk (your old branch is dead — the seat cherry-picked your FENCE commit; fetch
and start clean). The seven-condition standard from this round is the floor now.
