# STAGE11 — PROGRESS (orin 22, bench-side; nothing written to the repo)

Output dir: `~/unaos-bench/scratch/orin22/stage11/`. Repo read-only at `hw-jetson 98213b7f`.

## Step 1 — reading (done)
* `orin21/stage10/`: `FLIGHT-render10.md`, `KNOBS-render10.env`, `bootid-leg.sh`, `mutate.sh`,
  `mut/MATRIX.txt` (the 34-mutation audit), `mut/BOOTID-SELFTEST.txt`.
* `orin21/BULLETIN.md` §31 (scorer10's two FALSE REDs + four holes + the missing BOOTID leg) and
  §44–§47 (render10 candidate staged; Peter's ruling and the seat's §46 inventions struck by §47).
* `~/.claude/plans/unaos/batons/orin-22.md` §B (B1–B10, the checks that could not fire) and §A/§C/§D.
* render9 wire read with `awk 'index($0,"…"))` on a NUL-stripped copy (never grep). Facts taken:
  loader line at 19999 `crates/bootloader/src/main.rs@961: boot volume FAT serial 0xde001a13`;
  `[sdmmc] root` at 503/862/20518/20877 (TWO boots in that file — the last boot starts ~19900).

## Step 2 — `scorer11.sh` + `mutate11.sh` (done)
Six legs, verdict vocabulary = scorer10's. Key decisions and what each one repairs:

| decision | repairs |
|---|---|
| verdict is a SEPARATE argument to `leg()`, never re-parsed out of the printed line | scorer10 classified a green leg RED by splitting on an arrow; its "first ` -> `" fix would have read this file's ROOT-NONE datum (`[vfs] root -> NONE`) as the verdict "NONE" — observed, then removed by construction |
| positive control `crates/kernel/src/` (panic::Location paths; measured 32 / 47 / 53 across three real builds) | baton A11 — `KELF`/`ORIN-CARD` are LOADER strings, so scorer10's artifact zeros had no passing control |
| every leg gated on a control that must fire (loader lines; kernel lines) | B4/B5 — a zero without a passing control is NOT-SCORED, never PASS and never FAIL |
| hex compared by VALUE (leading zeros stripped), not as a string | the loader's formatter and the kernel's `%08x` need not agree on width |
| retired family checked on the WIRE **and** in the ARTIFACT | a boot that dies above the mount table cannot hide a pre-BOOTROOT image |
| ROOT-BIND scores NOTE (not a second red) when the NONE line carried the capture | one defect must not count twice |
| **zero** sector counts, serials, labels, card names, geometries | B3 — scorer10's 62333952 hardcode and its pre-fitsland literal, the two FALSE REDs |

Exit ladder 0/1/2 as briefed, **plus a documented exit 3** for a NOT-EXERCISED family with no reds:
n-ex must not exit 0 (that is how scorer10's n-ex rows read as green) and must not exit 1 (that
teaches the next seat to suppress the rule). Two `EXIT=3` sites, collapsible to 2 in one edit.

## Step 3 — can-fire proof (done)
`bash mutate11.sh` → 21 rows, `MUTATIONS.tsv` / `MUTATIONS.md`, per-row output in `out/`, fixtures in
`fix/`. Data mutated, scorer never touched. Coverage: ARMING 4 outcomes · LOADER-SERIAL 4 ·
ROOT-BIND 6 · ROOT-NONE 4 · OLD-BIND-ABSENT 4 · SOURCE-VOCAB 5; exits 0, 1, 2 and 3 all reached.

Real-data control run (`REALDATA-render9.out`): the UNMUTATED render9 last boot scored against the
real render10 candidate `kernel.elf` → `ARMING n-ex · LOADER-SERIAL PASS · ROOT-BIND n-ex ·
ROOT-NONE n-ex · OLD-BIND-ABSENT WRONG-IMAGE · SOURCE-VOCAB n-ex`, exit 1. Correct: that image is
pre-BOOTROOT, so the retired family is on its wire and the new emitter is in neither wire nor artifact.

## Step 4 — `FLIGHT-render11.md` (done)
One page: build (15 knobs = render10's set minus `UNAOS_SDMMCROOT`, which BOOTROOT deletes — the
knob and the `sdmmcroot` feature were both still present at `98213b7f`, verified this turn) · banner
set-compare with the one acceptable delta · arming on the artifact · card write as **Peter's action**
on the host under sudo via `load-card10.sh --target /dev/mmcblk0 --allow-geom-absent` · serial capture
against the butler already holding the port (host pid **81753**, `lsof -t` agrees — verified this turn)
· score · stop rules. **No card operation is proposed:** no regrow, no reformat, no `esp-jetson-img`,
no moving a card. render10 §3c is struck for this round.

## Open / not mine
* `exec-orin22-bootroot` is at `98213b7f` with 0 commits above `hw-jetson` — the contract's emitters
  do not exist yet. Every literal this scorer keys on comes from the brief's fixed contract, not from
  code; re-verify the four literals at the emit site once BOOTROOT commits.
* Whether BOOTROOT keeps the `sdmmc` feature (block driver) decides whether the banner loses one
  feature or two. The first render11 build measures it; that measurement is `KNOBS-render11.env`.
* `KNOBS-render11.env` / `stage-render11.sh` / `QUESTIONS-render11.md` — the staging convention's
  files, not in this brief. The knob line is written out verbatim in FLIGHT §1 so none is needed to fly.
