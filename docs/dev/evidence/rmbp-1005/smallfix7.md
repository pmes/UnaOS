# SMALLFIX7 (rmbp-ledger B501) — flight 27's small reds, each cause named on the wire

Wire: `docs/dev/evidence/rmbp-0915/flight27/f27-boot1.log` (image 20, hw-rmbp@a4ad6780). Cut from 668cdd95. No knob.

## Findings (read on the wire, then the code)

1. **WINMEMORY** (wire 2674..2697): the fixture's FIRST `wmtest` row reads `[wm] place win=2 app=wmtest from=cascade
   frame=847,743,…` — the injected saved frame was honoured by `winmemory::resolve` and then moved by GLASSFIX3's
   `glassfix3_cascade` because Peter's desktop has live rows under it (QEMU's desktop is empty, so the lane never saw
   it). `restored=fail` is the cascade overriding a SAVED frame; the Mac restores an autosaved frame where it was
   saved and cascades only a new window. `saved=0` is a true census, not a defect: the store counts
   `app.<name>.window.frame` rows and no app row was moved or closed before 08:29:57 (the wire carries no
   `[winmem] save` line and no app `tile remove … reason=close`; the save trigger is move-end/close).
2. **VERDICTS** (wire 2089): the wire has NO `:: TESTS: deferred=` line — the announce waits on `SRC_DESK`, which
   `u8x_launcher` signalled; BOOTVERDICTS (B472) moved the U7x→U8x chain behind `tests ladder`, so the source never
   completes and `tally` counted every verdict of the whole session, the seat's own `tests` runs included (door,
   hidstall, sanity, loader, notice×3, battery, quarrylive's ten, smallfix6, windowcap…). The boot itself printed
   six: TPMODE (26), FIRSTUSER (458), INPUTSTALL (467), DESKTOPBUILT (516), `[wifi6] s5i post-check` (1433), U5x (1488).
3. **PRTSCR** (wire 1890..1912): `tests prtscr` → `prtscr -> NOT-FOUND` (no fixture by that name). The `refused —
   capture in flight` line printed INSIDE `tests quarrylive`: `defer_fast` lets a deferred fixture's body run while
   ANY fixture runs, so `dir_fixture` (`prtscrdir`) fired from a paint/service pass during quarrylive and its
   in-flight-door arm held the door shut on purpose — the fixture raced another fixture, not a capture.
4. **LOGINFURN** (wire 2788): `console_prefill_lines=none console=unopened` — the prefill count exists only once a
   console was minted.
5. **PREHEAP's QEMU blind spot**: arroyo's QEMU lanes export `UNAOS_TESTS_AT_BOOT=1` (arroyo line 73), so the kernel
   carries `tests-at-boot` and `tests::register` takes the RUN-NOW branch — it never touches the heap-grown table.
   The metal image has no `tests-at-boot`, so only the metal took the push-before-heap path. gate19.sh has no boot
   leg at all (compile legs + static gates), so the shape the bench boots was never booted in the cloud.

## The seam

All kernel-internal (`Kernel — kernel-by-ruling`, R80/B472/B429). The pre-heap stash becomes a pure `no_std` core in
`midden_core::fixtures` (the `tests` verb's registry is a shell-verb table) so the host test runs the same code.

## Milestones

- M1 VERDICTS: `ladder_arm` signals `SRC_DESK` (the announce prints again); the tally stops at the first `tests` run;
  `tests verdicts` names the counted tags (`tags=`); the six boot lines print `-> ok|NOT-OK` before the deferral
  point via `tests::boot_word` (PASS/FAIL after it, and always under `tests-at-boot`, so the QEMU specs keep theirs).
- M2 WINMEMORY: a row with a saved frame and no live row of its app keeps the saved origin (no cascade); the line
  carries `saves_this_boot=`; the cascade arm scores "off the first row's title band, down-right".
- M3 PRTSCR: `defer_fast` runs a body only when THAT fixture is the one running; `tests prtscr` registered: waits
  (bounded) for the door, takes it, scores the in-flight refusal, prints `:: PRTSCR: fixture door=… -> PASS|FAIL ::`.
- M4 LOGINFURN: unopened → `ring_tail_lines=<n>` from `boot_ring::tail` (what the console's prefill would replay).
- M5 PREHEAP: `midden_core::fixtures::Stash` + host tests; `tests.rs` uses it; `unaos/scripts/boot-leg.sh` (an x86
  QEMU boot with the metal feature line and `tests-at-boot` OFF, judging the serial log for a panic) in `GATES_SET`;
  arroyo line 73 honours an explicitly-empty `UNAOS_TESTS_AT_BOOT=`.
- M6 `tests smallfix7` lists what it checked.

## Witness lines (the next flight reads)

`:: TESTS: deferred=<n> fire=tests at_boot=0 ::` (back on the wire) · `:: VERDICTS: before_deferred=0 … tags=- -> PASS ::`
· `[wm] place win=<n> app=wmtest from=saved …` then `:: WINMEMORY: saved=<n> saves_this_boot=<n> restored=ok … -> PASS ::`
· `:: PRTSCR: fixture door=free waited_ms=<n> refused=1 -> PASS ::` · `:: LOGINFURN: … console_prefill_lines=none
ring_tail_lines=<n> -> PASS ::` · `:: SMALLFIX7: … -> PASS ::`.

## Owed

U5x still RUNS at boot (its line is reworded; moving it behind `tests` stalls U6x/U6bx, which gate on
`U5X_LAUNCH_DONE` — the ladder's own move is the shape). FIRSTUSER's boot-time `FAIL —` has an empty reason (a
separate defect in `login_window_word`, not this arc). The boot leg was not run here (R78): the seat's first run
calibrates its completion marker.
