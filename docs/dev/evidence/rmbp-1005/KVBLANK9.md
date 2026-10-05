# KVBLANK9 — rungs 1–2 behind the test, the sim fixture behind `tests`, rung 3 armed by the compositor's first need (ledger B341)

Branch `exec-rmbp-kvblank9`, cut from 48391969, then `git merge exec-rmbp-gputests` (clean, 277f3b5c) so this
arc builds on GPUTESTS' parked-rung-3 shape. Seam: `CHARTER: Kernel — driver` (`drivers/gpu/kepler_vblank.rs`,
`video/beam.rs`); no new file, no new knob, no new verb (`tests kvblank` / `tests kvblank8` exist), no dotfile.
R80: nothing runs at boot but the boot.

## Finding

What GPUTESTS (B334) left: KVBLANK rung 1 (`bdf-hunt`, `pmc-arm` probe with its 50 ms PMC window, the head
`vblank arm` line, eight `vblank-intr census` samples) and rung 2 (`vblank-intr window open/close`, a 16-vblank
write/restore of `INTR_HOST_HEAD_EN`) still ran and printed at boot on the edge driver; the
`kvblank selftest arm=wait sim=timer|stuck` fixture still ran inside `beam::hold` on `witness` builds; and the
compositor never armed rung 3 — it paced on the poll source until an operator typed `tests kvblank8`.

## Milestones

- **M1 — rungs 1 and 2 behind `tests kvblank8`.** Boot banks, silently: BAR0 and the GK107 BDF (`find_gk107`,
  read-only), the PMC entry words (`INTR_EN`, `INTR_MASK_HOST`, `INTR_0`) at `kepler::init`, and the head, vtotal and
  the six-word PDISPLAY interrupt block at the head arm (post-takeover). The ladder starts PARKED. `tests kvblank8`
  first prints `:: kepler: vblank bank source=boot-bank … ::`, the `bdf-hunt` and `vblank arm` lines from the bank,
  runs the PMC probe live, then walks census (sample `boot` from the bank, then the eight live samples) and the
  enable window on the edge driver, bounded 10 s, before rung 3. Under `kvblank_trace` the boot walks the whole
  ladder as before (a knob is R80-admissible).
- **M2 — the sim fixture out of `beam::hold`.** The `selftest_once` call leaves `beam::hold`; `tests kvblank`
  (registered on every `nvidia-kepler-vblank` build) runs it. A `UNAOS_TESTS_AT_BOOT=1` capture (what
  `x86-witness.spec` scores) still carries both verdicts and the KVBLANK4 line.
- **M3 — first need.** `beam::hold`, once it has a beam source (i.e. after the Kepler takeover's head arm), calls
  `kepler_vblank::first_need()`: one CAS PARKED→IRQ_ARM with `armed_by=compositor`, no wait in the present. Rung 3
  runs unchanged on the edge driver with its own lines quiet; its close applies the same 90 % keep check
  (`irq*100 >= vbl*90`, no storm, ≥ 1 re-arm, a wire) and restores on failure (poll source). One line:
  `[wc-h] vbl_src=irq why=first-need irq=<n> vbl_delta=<v> ratio_pct=<p>` or
  `[wc-h] vbl_src=poll why=<reason> …` (`no-bdf|alloc|no-msi-no-intx|storm|below-90pct|<classify reason>`);
  bounded: a window not closed 5 s after the arm prints `vbl_src=poll why=window-timeout` from the next present.
  `tests kvblank8` reports `:: KVBLANK8: rung3 armed_by=compositor|test|trace irq= … -> PASS|FAIL ::`.
- **M4 — specs.** No spec REQUIREs a rung-1/2 line (searched `scripts/specs/*`); the two `vblank selftest`
  REQUIREs in `x86-witness.spec` are annotated tests-run (that spec's build already carries
  `UNAOS_TESTS_AT_BOOT=1`); `x86-witness.spec` gains OPTIONALs for the first-need line and the `rung3 armed_by=` verdict. `spec-roots.sh` and
  `fixture-reachable.sh` exit 0.

## Witness (a metal boot of the x86 shape)

Boot: no `vblank bdf-hunt`, `pmc-arm`, `vblank arm`, `vblank-intr census`, `window open|close`, `vblank selftest`,
`rung3 scheduled` or `vector armed|close` line; exactly one `[wc-h] vbl_src=irq why=first-need …` (or `vbl_src=poll
why=<reason> …`); the `[wc-h] vbl win=… vbl_src=irq` rollup companions then read `irq` from the first rollup.
`tests kvblank8`: the bank line, bdf-hunt, vblank arm, pmc-arm, `census sample=boot/8` + `1/8..8/8`, window
open/close, `:: KVBLANK8: rung3 armed_by=compositor … -> PASS ::`, then the KVBLANK8 instrument lines as before.

## Owed

- The periodic `:: kepler: vblank head=… count=… period_us=…` sampler line still prints on the `edge()` cadence
  (driver telemetry, not a rung; QUIETBOOT's `census` gate is the natural home).
- `vectors::alloc` prints its own `[vectors] alloc name=kepler-vblank` witness when first need allocates.
