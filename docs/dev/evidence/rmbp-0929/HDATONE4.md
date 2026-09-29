# HDATONE4 — boot 16's tone was silent (lpib=0, RUN clear)

## Finding
Boot 16 (f16.log): `[hda] tone stream=0 lpib=0 -> 0 (max 0) bcis=0 fifo_ready=0 run_ms=1200 sts=0x00 … ctl_running=0x00140004` and
`:: HDA-TONE: … -> FAIL :: amp=4096 ::`. Flight 15 (same machine, pre-wave tree) printed the SAME `ctl_running=0x00140004` and still ran
(`lpib=0 -> 38400 … fifo_ready=1 wraps=1`). So the immediate RUN read-back is not the discriminator. Also both boots: DAC `actual=D3 raw=0x30`.

## What the wave changed on the run path (hda.rs, git show e92500c6 / 8365a962)
- HDATONE3: rate/pcmcaps read (one extra GET_PARAMETER per member, before the SD reset), a fade + knobs in `tone::fill` (buffer content only),
  `FORCE_ONE_MEMBER`. NOTHING writes SDnCTL, SDnFMT or resets the stream after RUN; the fade is pure sample data. `amp=4096` is
  `UNAOS_HDA_AMP`'s default (HDATONE2), independent of Hz; the HDA-PCM line of boot 16 confirms `hz=440 fade_ms=20 -> PASS` (440 Hz WAS armed).
- VOLKEYS: `vol::capture` runs AFTER `run_tone` (probe tail, before `rings.stop`) — not between arm and run.
- Net: the SDnCTL sequence (SRST cycle, BDL/CBL/LVI/FMT, tag byte, IOCE, then IOCE|RUN at hda.rs `RUN` block) is byte-identical to flight 15.
  The cause is therefore not a source regression on this path; the two suspects left are a RUN write that did not latch on this PCH and the DAC
  left in D3 with a stream. Both are now guarded and made visible.

## Milestones
- M1: after the RUN write, poll SDnCTL up to 2 ms for RUN, re-assert the full 24-bit (tag byte + IOCE|RUN) up to 3x; during the run, if LPIB has not
  moved 100/200/300 ms in, re-assert again. Lines `[hda] run ctl=… readback=… run_bit=… reasserts=…` and `[hda] run stall reassert n=…`.
- M2: after each path node's SET_POWER_STATE D0, poll GET_POWER_STATE (bits 7:4) up to 50 ms until D0 (re-issue SET once at 25 ms); line
  `[hda] power settle member= nid= actual=D? settled_ms= reissued=`; the amp/format writes follow.
- M3: `run_tone` returns at its first line unless `TONE_NOW`; boot keeps walk/census/amp census. `hda::hda_tone_test()` (file tail) sets it and re-runs
  `probe()`. tests.rs did not exist in this tree: the compiler executor registers `"hda"` -> `hda::hda_tone_test` in `crate::tests::register`.

## Witness / pins
`:: HDA-TONE: … -> PASS :: amp=N :: run_bit=1 run_readback_ok=1 dac_pwr=D0|D3 settled_ms=N run_reasserts=N stall_reasserts=N ::`
x86-witness.spec: REQUIRE `… run_bit=1 run_readback_ok=1 dac_pwr=D0 settled_ms=\d+`, FORBID `run_readback_ok=0`. NB the existing
`REQUIRE :: HDA-TONE: .* -> PASS ::` now needs `tests hda` to have run (R77) — the lane that boots without the verb will miss it.

## Written
Boot 17, after `tests hda`: `[hda] power settle …`, `[hda] run ctl=… run_bit=1`, and the `:: HDA-TONE:` line above with `dac_pwr=D0`.
If `dac_pwr=D3` with `settled_ms=50` the DAC refuses D0 (pin 0x0a did on both boots) — next rung is the AFG (0x01) power state.
