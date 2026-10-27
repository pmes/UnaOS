# BP-gates — arroyo verbs, the host gates, fold legs, ledgers, evidence, vocabulary (read at dee2d6ed)

## 1. What it is
- `unaos/arroyo` is the one build/test entry point (a bash script; verbs at its `case` near the file tail).
- `./arroyo check` type-checks both arches (every kernel cfg leg) and runs GATE-LEDGER (`unaos/arroyo:8854`, `unaos/arroyo:9480-9517`).
- `./arroyo gates` runs every host gate in `GATES_SET` (no cargo, ~30 s), one `gate=<name> rc=<n>` line each, then `GATES: <n> run <n> green -> PASS|FAIL`; exit = the worst rc (`unaos/arroyo:14426-14443`).
- Fold legs beside the compile legs: `foldleg` (the x86 kernel + loader built under `image-line.env`), `twoboot` (the image booted twice under QEMU), `knoboff <feature>` (knob-off byte identity, `unaos/arroyo:13888`).
- The image legs of `./arroyo check` (KERNEL_CFG_MATRIX tail): each is the knob line a card flies, so the fold type-checks it.

  | leg | the line it compiles | anchor |
  |---|---|---|
  | `arm-tegra-image` | `unaos/scripts/image-line-orin.env` minus the four MANIFEST knobs | `unaos/arroyo:8297` |
  | `arm-pi-image` | `unaos/scripts/image-line-pi.env`; with `arm-pi` it compiles the Pi's `baremetal` arms (no separate Pi leg: SUPSTATELEG measured it) | `unaos/arroyo:8298` |
  | `x86-image` | the image-24 set on a literal x86 leg (keeps `x86_cfg_universe` whole) | `unaos/arroyo:8299` |
  | `arm-tegra-image-supstate` | the Orin line + orindesk/orinclick/orinconwin/orinrender + `supstate` (SR168) | `unaos/arroyo:8317` |

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
| BP-gates-14 | No fn anywhere under `video/` REACHES an outbound act — a socket opened, an active connect, the stack's outbound ledger (BP-net-11), a DNS query built, `http_core`/`tls_core` — through path calls into any module, net modules included (a read-only accessor is reached freely; the act is the sink, not the module name); a user verb's entry would sit on the gate's BOUNDARY table with its reason (none today) (BP-bar-10). | `unaos/scripts/nonet-reach.py:1-30` | `gates` row `nonetreach` (`NONET-REACH: roots= edges= reach=0 -> PASS`; control: a synthetic dock -> helper -> socket-open tree and a DNS alias, judged exactly; plant: a video fn -> a helper outside `video/` -> `smolnet::stack_open_tcp`) |

## 3. The host gates (`GATES_SET`, `unaos/arroyo:14354`)
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
| nonetreach (B697) | `unaos/scripts/nonet-reach.py` (imports nonet-check.py's walker) | BP-gates-14 |
| neutral (SR166) | `tools/neutral-check.py`; ruled exceptions `tools/neutral.allow` (row + reason, shrink-only) | BP-shape-4: no board word part in an identifier or string of `unaos/crates/kernel/src` minus `arch/` (no drivers/ dir is board-specific today) |
| firmware (SR166) | `tools/firmware-census.py` | BP-shape-11: the `git ls-files` census of `unaos/firmware/` (pins digest rows + a resolving ledger row; no binary) |
| locktype (SR188) | `unaos/scripts/lock-type.py` (imports `tools/neutral-check.py`'s lexer) | BP-cores-20: no raw `spin::Mutex`/`MutexGuard`/`spin::mutex::*`/`spin::RwLock*` in the code of `unaos/crates/kernel/src`, save sync.rs's six allow rows (the registry's inner lock, exact path+line, shrink-only); `spin::Once` and `spin::Spin` pass |

## 4. Verbs (`unaos/arroyo` tail `case`)
`check` · `state` · `gates [--list|--selftest]` · `test` (at `UNAOS_FBW`/`UNAOS_FBH`/`UNAOS_PPI` with `UNAOS_WC`+`UNAOS_FACET` it replays `unaos/scripts/specs/x86-panel.spec`, ZOOMPANEL B842) / `test-arm` / `test-fat` / `test-install` / `test-ptr` / `test-splash` / `test-selfhost` (QEMU) · `x86` / `arm` (QEMU GUI) ·
`esp-x86` (stages `unaos/fixtures/TESTS.LIST` + `TESTS.HEAVY` onto the ESP when present, the media verb only — BOOTLIST B817, `unaos/arroyo:14559`) / `esp-arm` / `esp-jetson` / `esp-jetson-img` (media) · `kernel8` / `kernel8-run` / `kernel8-test` (`--desk` = `UNAOS_PIDESK=1`; with the knob it replays `unaos/scripts/specs/pi4-desktop.spec` after pi4-regression.spec, PIDESK-SPEC B878) / `kernel8-install` (Pi) · `foldleg` · `twoboot` · `knoboff` · `bootleg` · `battery` · `mbench` · `jobs-image` · `vm-image` · `fat-img` · `selfup-pak` · `clean`.

## 5. Ruled, unbuilt / open
- GATE-NEUTRAL (BP-shape-4): built as the `neutral` row (SR166); 110 findings at fb8ad2f4 sit on `tools/neutral.allow`, each reason naming its owed rename.
- The firmware census (BP-shape-11): built as the `firmware` row (SR166); GATE-LEDGER (`unaos/scripts/ledger-check.sh`, after the branch census) runs it too, its exit folded the same way (LEDGERFW).
- BOOTLIST (B817, R132): the flight's list is `unaos/fixtures/TESTS.LIST` (flight 37's, light then `HEAVY:`), run by the kernel at `[desktop] built` (boot.md BP-boot-4); its probe `tests bootlist` is pinned in `unaos/scripts/specs/x86-wc.spec`. Owed: a list generator from the ledger's open rows (the seat's), and `esp-arm`/`kernel8` staging.
- VALUEKNOB (KEYBOOT3 B856): `scripts/knob-parity.sh --line` parses an image-line knob only as `export UNAOS_X=1` and arroyo's map only as `[ -n "${UNAOS_X:-}" ]`, so a value-polarity knob (`=0` meaning off) cannot ride image-line.env; KEYBOOT3 took the positive `UNAOS_KEYBOOT_DEFER=1` instead of `UNAOS_KEYBOOT_FIRSTUSER=0`. Whether a value knob is ever allowed (and its grammar) is owed a ruling.

## 6. Read further
- `docs/dev/LAWS.md` §5, `docs/dev/PLAYBOOK.md` §1-§3, `docs/dev/EXECUTOR-BRIEF.md`.
- `docs/dev/STRUCTURAL_GATES.md` (each gate's go-red record), `docs/dev/STATUS.md`.
