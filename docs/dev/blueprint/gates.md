# BP-gates — arroyo verbs, the host gates, fold legs, ledgers, evidence, vocabulary (read at dee2d6ed)

## 1. What it is
- `unaos/arroyo` is the one build/test entry point (a bash script; verbs at its `case` near the file tail).
- `./arroyo check` type-checks both arches (every kernel cfg leg) and runs GATE-LEDGER (`unaos/arroyo:8814`, `unaos/arroyo:9440-9477`).
- `./arroyo gates` runs every host gate in `GATES_SET` (no cargo, ~30 s), one `gate=<name> rc=<n>` line each, then `GATES: <n> run <n> green -> PASS|FAIL`; exit = the worst rc (`unaos/arroyo:14335-14353`).
- Fold legs beside the compile legs: `foldleg` (the x86 kernel + loader built under `image-line.env`), `twoboot` (the image booted twice under QEMU), `knoboff <feature>` (knob-off byte identity, `unaos/arroyo:13848`).
- Records: ledgers (`docs/dev/LEDGER.md`, `docs/dev/OS/rmbp-ledger.md` and its pi/orin siblings), queues (`docs/dev/QUEUE.md`, `docs/dev/OS/rmbp-queue.md` …), status rows (`docs/dev/STATUS.tsv`), evidence (`docs/dev/evidence/`).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-gates-1 | The pre-commit leg for an executor and for every fold commit is `./arroyo gates`, scored by exit code into a log, never through a pipe. | `docs/dev/PLAYBOOK.md:32-35`; `docs/dev/LAWS.md` §5 "A pipe launders the verdict" | GATEPATH `gates --selftest` (every row plants a defect and must catch it) |
| BP-gates-2 | Every host gate carries a control: a broken pattern is exit 2 (no verdict), never a green. | `docs/dev/LAWS.md` §5 "A check that cannot fire is an absent one"; `unaos/scripts/appearance-check.py:28-31` | each gate's control probe |
| BP-gates-3 | A capture judged by a verb must reach the ladder's LAST line; a truncated run is exit 5, never a pass. | `docs/dev/LAWS.md` §5 "A TRUNCATED RUN IS NEVER A PASS" | `ladder_tail_check` in `unaos/arroyo` |
| BP-gates-4 | An instrument's presence is certified on the ARTIFACT (`LC_ALL=C grep -a -o -F <witness>`) with a control string, never by the diff, the check or the banner; a video gate carries `UNAOS_WC=1`. | `docs/dev/PLAYBOOK.md:64-65`; `docs/dev/LAWS.md` §5 | `gates` row `banner` (`unaos/scripts/banner-cert.sh`) |
| BP-gates-5 | Knob-off byte identity: a new statement goes inside an existing cfg region or LINE-NEUTRAL before the line's first `//`; `./arroyo knoboff <f>` exit 0 = identical, 1 = moved, 2 = no verdict (never a pass). | `docs/dev/EXECUTOR-BRIEF.md:18-24` | `./arroyo knoboff`; `unaos/scripts/line-neutral.sh` |
| BP-gates-6 | A status word (flew, unflown, refuted, landed on metal, and the three claim words) in a ledger or a queue STATE cites an `ST<n>` row whose quoted line exists byte-for-byte in that flight's capture. | `docs/dev/STATUS.md:1-25` | `gates` row `status` (`tools/status-check.py`) |
| BP-gates-7 | Every finding is one ledger row (7 cells, no literal pipe, no scratch path, no unpushed sha); ids unique per file; the arc that fixes, flies or drops it ticks it in the same commit. | `docs/dev/PLAYBOOK.md:98-101`; `unaos/scripts/ledger-check.sh:1-20` | GATE-LEDGER in `./arroyo check` |
| BP-gates-8 | A fold is UNION BY ROW ID; a `.rs` union is followed by brace/line arithmetic against both parents; the fold's own falsifiers re-run on the folded tree. | `docs/dev/LAWS.md` §3 "Folds", "A clean merge is where composition defects hide" | GATE-BRACES, GATE-SELFSYNTAX in `./arroyo check` (names from LAWS; implementation `unverified`) |
| BP-gates-9 | The rMBP image's knob line is one file, held equal to arroyo's knob map and the builder's; Pi and Orin lines likewise. | `unaos/scripts/image-line.env:1-17` | `gates` rows `image-line`, `knob-parity-orin`, `knob-parity-pi` |
| BP-gates-10 | Durable records go to a round directory under `docs/dev/evidence/`; nothing durable lives in scratch; nothing under `/tmp` on the bench. | `docs/dev/LAWS.md` §3 "Nothing durable lives in a round's scratch" | GATE-LEDGER (unanchored excerpt refused, `docs/dev/PLAYBOOK.md:95-96`) |
| BP-gates-11 | On the rMBP track the gate is the compile legs plus the doc gates; QEMU legs are not run per arc (R78, R113; R114 a one-time exception). The metal boot is the proof. | R78, R113, R114 (`docs/dev/RULINGS.md`) | review |
| BP-gates-12 | The design is written in the commit body BEFORE the code; the executor compiles what it touched and runs `./arroyo gates` (R106). | R106; `docs/dev/PLAYBOOK.md:32-35` | review |
| BP-gates-13 | No fn reachable from the `video/` furniture (menubar, dock, crystal, notify, strip, settings, login) calls `smolnet`, `net_fetch`, `net_dns` or `netring3`, save a local read-only accessor on the gate's ALLOW table with its reason (BP-bar-10). | `unaos/scripts/nonet-check.py:29-34` | `gates` row `nonet` (a call walk over `use` aliases and paths; control: a synthetic dock -> helper -> `smolnet` chain) |

## 3. The host gates (`GATES_SET`, `unaos/arroyo:14279`)
| row | script | holds |
|---|---|---|
| charter | `unaos/scripts/charter-check.sh` | CHARTER lines, no dotfile prefs (BP-shape-8) |
| arch | `unaos/scripts/arch-check.py` | seamcite, sharedcore, dotslash, settings, store, parser legs (baseline only shrinks) |
| attrkeys | `unaos/scripts/attrkeys-check.py` | attribute keys against their baseline |
| status | `tools/status-check.py` | BP-gates-6 |
| prefs | `tools/prefs-schema-check.py` | every pref key is a schema row |
| appearance | `unaos/scripts/appearance-check.py` | BP-video-1 |
| deps | `tools/deps-audit/run.py` | BP-shape-10 |
| knob | `unaos/scripts/knob-hygiene.sh` | knob names/map hygiene (detail `unverified`) |
| verbs | `unaos/scripts/verb-roots.sh` | shell verb table roots |
| banner | `unaos/scripts/banner-cert.sh` | banner vs artifact (BP-gates-4) |
| k8reach | `unaos/scripts/k8-reach.py` | Pi image reachability |
| sanity | `unaos/scripts/sanity-legs.py` | leg sanity (detail `unverified`) |
| image-line, knob-parity-orin, knob-parity-pi | `unaos/scripts/knob-parity.sh` | BP-gates-9 |
| mbench-selftest, specscore | `unaos/scripts/mbench.py`, `unaos/scripts/orin-specscore.py` | the scorers' self-tests |
| blueprint (added by `dee2d6ed`) | `unaos/scripts/blueprint-check.py` | these sheets: anchors exist, ids unique and stem-named, sheets listed, ≤ 120 lines, no claim words |
| nonet (B672) | `unaos/scripts/nonet-check.py` | BP-gates-13 |

## 4. Verbs (`unaos/arroyo` tail `case`)
`check` · `state` · `gates [--list|--selftest]` · `test` / `test-arm` / `test-fat` / `test-install` / `test-ptr` / `test-splash` / `test-selfhost` (QEMU) · `x86` / `arm` (QEMU GUI) ·
`esp-x86` / `esp-arm` / `esp-jetson` / `esp-jetson-img` (media) · `kernel8` / `kernel8-run` / `kernel8-test` / `kernel8-install` (Pi) · `foldleg` · `twoboot` · `knoboff` · `bootleg` · `battery` · `mbench` · `jobs-image` · `vm-image` · `fat-img` · `selfup-pak` · `clean`.

## 5. Ruled, unbuilt / open
- GATE-NEUTRAL (board names in shared files, BP-shape-4): no implementation found.
- The firmware `git ls-files` census in ledger-check (BP-shape-11): owed.

## 6. Read further
- `docs/dev/LAWS.md` §5, `docs/dev/PLAYBOOK.md` §1-§3, `docs/dev/EXECUTOR-BRIEF.md`.
- `docs/dev/STRUCTURAL_GATES.md` (each gate's go-red record), `docs/dev/STATUS.md`.
