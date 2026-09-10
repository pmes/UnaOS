# rmbp 11 — LANDING REPORT (2026-09-03 → 2026-09-06; bench; orin held the focus; support pace, no fleet)

## Landed on hw-rmbp (all on origin at close except reconcile #3 if its gate was still running — see baton rmbp-12 J0)
| sha | what |
|---|---|
| 857c6dc8 | x86/power: BOOTFADT — FADT reset facts printed once at boot on the normal console path |
| 21b792b0 | docs: flight 6 metal results |
| bc10a469 | power: PWRNAME — `[orinreboot]`/`[orinshutoff]` → `[pwrreboot]`/`[pwrshutoff]`; Tegra wdt `[orinwdt]` (orin 13's grant) |
| 8c559329 | reconcile #1: trunk be3b027e → hw-rmbp (wm.rs WEDGESRC×WMPAR, arroyo GATE-ROOTS leg, GATE-FAMILY render_service 2→3) |
| 1a0a7046 | docs: rmbp-ledger created; S12 checked and dropped for x86 |
| e693056a | gates: GATE-LEDGER; RULINGS.md; evidence into git |
| 8a2bfcb3 | gates: GATE-KNOB trailing-comment phantom; GATE-LEDGER evidence anchors + RULINGS supersession |
| 88849a68 | docs: legacy sweep — E1 dropped, E2 found landed, E3 captures closed |
| d815e659 | gates: GATE-LEDGER accepts the Orin `KELF` loader identity as an anchor |
| c9242a54 | reconcile #2: trunk d11cd56e → hw-rmbp (arroyo duplicate GATE-KNOB dropped, ledger-check.sh, RULINGS union R1–R17) |
| 68bfc01f · a7095fdd · c257cb61 | docs: rmbp-ledger links/rows S27, B9, B10 |
| (reconcile #3) | trunk f49ea1e7 → hw-rmbp; prtscr.rs + screenshot.md doc unions (PRTSCR2 ∘ PRTSCR-VOL) — see baton J0 |

## Gate results
Every commit: `./arroyo check` exit 0 both arches (66–68 legs, 0 red; FAMILY/KNOB/ROOTS/LEDGER OK), `UNAOS_WC=1 ./arroyo
test 150` exit 0 with `wc` in the banner, `./arroyo test-arm 60` exit 0. GATE-LEDGER and the GATE-KNOB increment each went
red by tree mutation before shipping (12 and 2 fixtures respectively; recorded in STRUCTURAL_GATES.md).

## Metal (2012 rMBP, bench, FTDI on /dev/ttyUSB1)
Flight 6 (f751cb78, 3 boots): chords CONFIRMED (SCREEN3/4.PNG verified); reboot verb reset the machine but the ladder is
unobservable on this console (A3); absence controls hold; 70 s per capture with input dead (A2); BAR1 wedge + recovery held
(A1). Flight 7 (bc10a469, 1 boot): `[pwrreboot] FADT RESET_REG discovered at boot: space=SystemIO addr=0xcf9 value=0x6`.
Postmortems: FLIGHT6-POSTMORTEM.md, FLIGHT7-POSTMORTEM.md (this dir). MANIFEST lines 637, 638.

## Peer work (all over ccd, same turn)
Three peer acks given to orin (be3b027e, d11cd56e, f49ea1e7 landings), each verified in this tree (merge-base, ancestry
of named shas, review + flight files, their battery logs). Two grants: PWRNAME (in, consumed), prtscr.rs A17 (out,
consumed by f0db58bf, reviewed same turn, flew on render4). Tracking overhaul converged with orin 13 and pi 6 on Peter's
R5 and built the same day (GATE-LEDGER, status enum, evidence-in-git with anchors, RULINGS supersession, shrunk batons).

## Flagged for rmbp 12 / Peter
- Landing of the rmbp arc (~45 commits) needs the adversarial panel = a fleet = the focus (Peter's call).
- A4 default-startup-volume, B7 vug arbiter, S27 branch prune: Peter decisions, surfaced once.
- B9 `[ptrdead]` flake fired 1-in-2 on orin's proof run; rate unmeasured.
- pi 6's findings live on LEDGER.md (S16–S24); pi-ledger.md deferred by Peter until pi has the focus.
