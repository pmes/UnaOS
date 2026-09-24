# F13FIX — the flight-13 findings, fixed in the cloud (B213 LOGIN15, B214 TPSCALE, B215 HDATONE2; 2026-09-24)

Source: `docs/dev/evidence/rmbp-0915/flight13/FLIGHT13.md` (bench, 20ce06e8). Peter, mid-turn: "please turn the
volume down on the test tone" → R66; the tone's default is 4096 again.

## The one QEMU lane that carries all three fixtures
`UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_HDA=1 UNAOS_HDATONE=1 UNAOS_HDA_AMP=8192 ./arroyo test 60` — the default stick, so
`x86-default.spec` replays (TPSCALE and USERSREADY are pinned there); the HDA knobs add `:: HDA-PCM:` and the `amp=` segment;
the non-default amplitude proves the knob reaches the ELF.

## Run A — HDA lane, `UNAOS_HDA_AMP=8192` (`test 60`): the knob reaches the ELF, the buffer reads back, the scaler is right
```
:: EHCI-HID: TPSCALE self-test: div=8 raw=128,88,-60,7,-7 -> px=16,11,-7,0,0 (clamp-step=16px/frame, toward-zero) -> PASS ::
:: HDA-PCM: amp=8192 default=4096 peak=8191 min=-8192 q27=8191 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 -> PASS ::
[hda] tone stream=0 lpib=0 -> 44408 (max 191968) bcis=2 fifo_ready=1 run_ms=1200 … consumed=236408 rate_bps=197006 expect_bps=192000 members=1
:: HDA-TONE: lpib_advanced=1 walked=1 wraps=1 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> PASS :: amp=8192 ::
```
This run ALSO reddened `:: TPFRAME: … deltas_ok=false … d=-3/-1,0/0 -> FAIL`: B197's fixture wanted the corpus's RAW deltas
(-30/-12, -3/-5) and `mt_step` now returns pixels. The fixture's truth table stays raw and is scaled at the compare
(`want = tp_scale(want_raw)`); its spec pin reads `d=-3/-1,0/0`. No `[users]`/`USERSREADY` here: `users::service` is `login`-gated,
so the USERSREADY pin lives in x86-login.spec only (a first cut pinned it in x86-default.spec — wrong lane, removed).

## Run B — the login lane, RUN-BY knobs + `UNAOS_HDA=1 UNAOS_HDATONE=1` (no amplitude knob), `UNAOS_QEMU_FULL=1 test 240`
```
:: EHCI-HID: TPSCALE self-test: div=8 raw=128,88,-60,7,-7 -> px=16,11,-7,0,0 … -> PASS ::
:: TPFRAME: frames=7 fingers_max=1 deltas_ok=true click_edges=down@4,up@5 corpus=3 d=-3/-1,0/0 lift_reset=true … -> PASS ::
:: USERSREADY: rmbp-shape(global=0 sdhc=1 ahci=1)=1 qemu-shape(global=1 sdhc=1 ahci=0)=1 none=0 old-guard-on-rmbp=0 this-boot ready-by=global=0 sdhc=1 ahci=0 -> PASS ::
[users] load volume=el0-fat(rw) src=none users=0 (fresh store) next_uid=13250050
[login] root password unset row=created -> set-password screen (LOGIN14/R65: chosen at the keyboard, twice; never on the wire)
:: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=4095 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 -> PASS ::
:: HDA-TONE: … members=1 -> PASS :: amp=4096 ::
  x86-login.spec   ✅ MBENCH PASS — 54/54 required witnesses, 0 forbidden hit(s), 3345 lines scanned
  x86-default.spec ✅ MBENCH PASS — 25/25 required witnesses, 0 forbidden hit(s)
```
Two readings from this run. (1) `this-boot ready-by=global=0 sdhc=1 ahci=0` on the first ready pass: even under QEMU the SDHC
card registers BEFORE the global, so the old guard waited for the test disk here too — the fixture line said so, which is what it
is for. (2) The fixture printed 24 times (once per pass until the mount landed); a once-latch followed (Run C).

## Go-red — the three defects put back at once (`test 90`, same knobs): old guard, `TP_MT_DIV = 1`, `>> 14` + left-only write
```
:: EHCI-HID: TPSCALE self-test: div=1 raw=128,88,-60,7,-7 -> px=128,88,-60,7,-7 (clamp-step=128px/frame, toward-zero) -> FAIL ::
:: USERSREADY: rmbp-shape(global=0 sdhc=1 ahci=1)=0 qemu-shape(global=1 sdhc=1 ahci=0)=1 none=0 old-guard-on-rmbp=0 … -> FAIL ::
:: HDA-PCM: amp=4096 default=4096 peak=8191 min=-8192 q27=8191 interleave=0 peak_ok=0 sine_ok=0 le_ok=1 frames=48000 -> FAIL ::
:: TPFRAME: … d=-30/-12,-3/-5 … -> PASS ::      ← by design: TPFRAME's truth scales with the divisor; the SCALE is TPSCALE's row to red
```
USERSREADY printed once (the latch). Mutations reverted.

## Run C — the final tree, clean (`test 90`, same knobs)
```
:: EHCI-HID: TPSCALE self-test: div=8 raw=128,88,-60,7,-7 -> px=16,11,-7,0,0 (clamp-step=16px/frame, toward-zero) -> PASS ::
:: TPFRAME: frames=7 fingers_max=1 deltas_ok=true click_edges=down@4,up@5 corpus=3 d=-3/-1,0/0 … -> PASS ::
:: USERSREADY: rmbp-shape(global=0 sdhc=1 ahci=1)=1 qemu-shape(global=1 sdhc=1 ahci=0)=1 none=0 old-guard-on-rmbp=0 this-boot ready-by=global=0 sdhc=1 ahci=0 -> PASS ::   (once)
:: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=4095 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 -> PASS ::
:: HDA-TONE: … members=1 -> PASS :: amp=4096 ::
  x86-login.spec   ✅ MBENCH PASS — 54/54 required witnesses, 0 forbidden hit(s), 2988 lines scanned
  x86-default.spec ✅ MBENCH PASS — 25/25 required witnesses, 0 forbidden hit(s)
```
Type-checked x86 (`wc,witness,login,loginst,ehcihid,hda-tone,sdhcblk,ahci,quarry,ftdirx`) and aarch64 (`witness,login,loginst,virt_el0` —
`users.rs` is shared; the SDHC/AHCI arms are cfg-false there).

## Boot 14 reads, in order
1. `:: USERSREADY: … this-boot ready-by=global=0 sdhc=1 ahci=1 -> PASS ::` — the guard opened on the metal's own registries.
2. `[users] load volume=el0-fat(rw) …`, `[login] root password unset row=created -> set-password screen` — the chain that never ran.
3. On a 1 cm stroke: `[tp] mt fingers=1 … dx= dy= div=8` — pixels now; if still too fast, the divisor doubles (a constant, B214).
4. `:: HDA-PCM: amp=4096 … -> PASS ::` then the tone at the quiet level (R66); "sandpaper" at 4096 would point past the sample layout.
