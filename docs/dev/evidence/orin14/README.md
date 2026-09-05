# orin 14 evidence — render4

Files this arc produces here (the corpus convention is `docs/dev/evidence/orin13/`):

| file | what | written by |
|---|---|---|
| `FLIGHT.md` | the render4 recipe: build/stage/load/hold-the-line, the boot sequence, the pin + purity + extraction commands, one scorer per question with its PASS predicate and the ledger row it ticks | FLIGHTPREP4 (this commit) |
| `README.md` | this list | FLIGHTPREP4 |
| `A16-SCORE.md` | the A16 decision table: what each `iir=`/`fifo=`/`ovrf=` reading plus the burst-vs-paced delivery counts says about the RX loss mechanism | executor A16 |
| `render4-boot1.log` | the scored excerpt — first line is the loader anchor `KELF min=0x0 max=<render4 max vaddr>`, ANSI-stripped, unwrapped, board-pure (§C.2 of FLIGHT.md); one file per power-on if the boot had to be retried (`render4-boot2.log`, …) | the flight |
| `FLIGHT-RESULT-render4.md` | the verdicts, one row per question, excerpt line numbers as evidence; ticks A15 (pass count), A16, A17, A18 in the ledger in the same commit | the flight |

Not committed (bench scratch, `~/unaos-bench/scratch/orin14/`): `stage-render4.sh`, `NAME_RENDER4`,
`build-render4.log`, `render4-paced-inject.out` (the paced injector's per-byte stamps),
`render3b-card-harvest/` (the A17 artefacts taken off the card before the load), the render4
`SCREEN*.PNG` captures (6.9 MB each), and `flightprep/` (PROGRESS.md, the scorer dry-run).
