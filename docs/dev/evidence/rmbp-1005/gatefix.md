# GATEFIX (rmbp-ledger B476): the seven gate arcs GATEREVIEW left unbuilt

Cut from merge19 5da57a2d. Host only: no kernel file, no knob. Source of the seven: `docs/dev/review/GATES-2026-10-06.md`
(F8, F7, F19 and the plant table rows C1, C6, S2, S4, S6, V1).

## Design (written before the code)

**Finding.** Seven holes, each a plant that a gate passed: V1 (an INNER match arm counts as a verb), C1 (a headerless
file in a new kernel directory is unseen), C6 (`kernel-by-ruling R999` passes), S2 (a quote found only in FLIGHT<N>.md
prose while the flight has a log), S4 (`verified on metal … PASS on flight 24` is no status word), S6 (a hand-typed
`f24-boot9.log` is wire), and ORIN (`orin*/` captures are outside the `f<N>` model, so four Orin rows sit `open`).

**Seam.** Each fix lives in the gate that owns the property; no new gate script except the bench's pin tool:
| arc | gate | rule | plant (selftest/control) | baseline |
|---|---|---|---|---|
| VERBDEPTH | verb-roots.sh | only depth-1 arms of `match command {` | control: an inner arm is not seen, its outer arm is | none (103/103) |
| CHARTERSCOPE | charter-check.sh | every `crates/kernel/src/**/*.rs` | `--selftest`: C1 `desktop/notes.rs` | `charter-scope.baseline` (138, shrink-only) |
| SEAMCITE2 | arch-check.py seamcite | every cited R/B id resolves in RULINGS.md / a ledger | selftest: `R999` and an unknown `B` | arch.baseline unchanged |
| STATUSWORDS | status-check.py T5 | + verified on metal, PASS(ED) on (the) metal, PASS(ED) on flight N | fixture S4 | none (cells corrected) |
| STATUSWIRE | status-check.py T3 | a flight with a log: the quote must be on a log | fixture S2 | none (rows re-quoted) |
| CAPTUREPIN | status-check.py T6 + `scripts/capture-pin.sh` | a capture log is wire only if `evidence/CAPTURES.pin` holds its sha256 | fixture S6 | the 13 rMBP + Orin logs pinned at the cut |
| STATUSORIN | status-check.py | flight `r<N>` = Orin render N: `orin*/**/render<N>-*.log`, `boot-render<N>-*.log`, `FLIGHT-RESULT-render<N>.md` | fixture | none |

**Milestones.** M1 VERBDEPTH · M2 CHARTERSCOPE · M3 SEAMCITE2 · M4 STATUSWORDS + STATUSWIRE · M5 CAPTUREPIN + STATUSORIN ·
M6 STRUCTURAL_GATES.md sections.

**Witness.** Host gates; the witness is each gate's exit code and its `--selftest` line (no metal line: R80, nothing at boot).

**Owed (the seat's documents, reported, not edited).** The ledger cells STATUSWORDS surfaces and the ST rows STATUSWIRE
and STATUSORIN re-quote: corrected text below.
