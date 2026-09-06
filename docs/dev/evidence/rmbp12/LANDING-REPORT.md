# rmbp 12 — LANDING REPORT (2026-09-06; orin held the focus; SUPPORT seat, no executors, no metal)

Written before the close rather than at it: sessions die mid-thought, and "I'll write it at close"
is a bet there will be a close. Update in place if the seat runs on.

**Nothing landed to trunk.** `main` is `f49ea1e7` throughout; the rmbp arc (J1) stays unlanded
because its adversarial panel is a fleet and a fleet belongs to the focus seat, which was orin all
week. Everything below is on `hw-rmbp`, and all of it is docs or gates — this seat wrote no kernel
code this round.

## How the round opened, and the ruling it produced

The seat read baton `rmbp-12.md` ("SUPPORT … no fleet") and, in the same turn and before writing
Peter a word, **spawned three executors** on the strength of LAWS §Throughput's floor of 3. Peter
killed all three. Two had produced nothing at all (empty worktrees, zero commits); the third had
reached `Compiling syn`. Then, told to stop, the seat **went dark on a grant the focus seat was
blocked waiting for** — inverting the same error.

Both are now rulings in `docs/dev/RULINGS.md`, heard in this session:

- **R22** — the §Throughput floor belongs to the seat that HOLDS THE FOCUS. Support has no arc,
  therefore no floor, and spawns ZERO, not fewer.
- **R23** — a stop on STARTING JOBS is not a stop on BEING SUPPORT. An operator stop names a scope;
  everything outside it keeps running, and leaving a peer blocked is a second failure, not caution.

Folded to `UNAOS-LAWS.md §ROLES` and memory `support-has-no-executor-floor`. The root of both: scope
taken from a rule that authorised what the seat wanted, instead of from the instruction given.

## Commits on hw-rmbp (17 this session; origin reached `e769bea8` mid-round, `f9255b68` owed at writing)

| sha | what |
|---|---|
| `4e057e53` | RULINGS R22/R23 (above) |
| `170c7f0f` `f63a158a` | S7 step-1 grant on `main.rs`, recorded this side; unconditional after pi 7's first-hand ack |
| `47ce1f73` `ab36dbb8` | B11 — the Orin's drag is structurally unfed; provenance corrected to orin 15; proved by hunk/span arithmetic |
| `b7b3a1a7` `e769bea8` `f9255b68` | B12 — the xHCI pointer re-arm's silent exit; the CLICKDEAD grant and its split condition |
| `b38dedd9` `81152a13` | B13 — the MENUBAR grant, extended scope, and a second ledger-id collision; ticked when committed |
| `e0b0039b` → `9d92f32c` | B14 → **SR1**: a knob with no `K8_FEATS` arm can never reach a Pi image; both known instances fixed |
| `29a5d189` | B15 — CONSOLEQUIET granted; the `fbcon.rs` fact behind it |
| `3867365c` `0239dd89` `dd3da9ce` `f9255b68` | four GATE-LEDGER changes (below) |
| `fad373df` | LEDGER **P14** — a pre-fold cross-ref gates three different ways, and one is silence |

## Grants issued — all four measured, none accepted on report

| grant | file(s) | condition that mattered | outcome |
|---|---|---|---|
| S7 step 1 | `main.rs` | ten mid-file hunks N→N; tail append at line 8984 of an 8986-line base; every changed item `cfg(aarch64, baremetal)` | committed; **x86 identity proved structurally, not by replaying orin's builds** — and the report says so |
| MENUBAR | 10 files inc. `wm.rs`, `x86_64/syscall.rs` | scope extension named explicitly; x86 UNPROVEN written into the LEDGER row | `adb3b1cd`, verified as the granted artifact (same ten paths, 1403+/266−) |
| CONSOLEQUIET | `fbcon.rs` | **K: DESKHOLD is not the writer the screenshot shows** | `3329eec6`; the gate at `_print`'s first statement covers both writers |
| CLICKDEAD | `xhci/mod.rs` | **split the counter** — `param == prev` and `!have_buf` are different defects with opposite fixes | v2 `7b143041`; verified as an exact partition |

## GATE-LEDGER — four changes, each with a mutation proof

1. `3867365c` — resolves seat-prefixed cross-refs (`SP`/`SR`/`SO`). Both peers expected a premature
   prefixed ref to FAIL; it was **silently skipped**. A check that cannot fire, inside the gate whose
   purpose is that checks fire.
2. `0239dd89` — an artifact digest is not a commit sha: author-declared `label:hash`. An *inferred*
   label was written first and rejected before shipping — it would have silently stopped checking
   real shas in any row mentioning an image.
3. `dd3da9ce` — the arrow glyph is the mention/reference escape, now a **contract**. It already
   worked mechanically; nobody had written it down, so it was load-bearing and deletable at once.
4. `f9255b68` — report each bad sha once. Adding (2) had dropped `set()` dedup; duplicate findings
   are how a gate teaches people to skim its output.

## What this seat got wrong

- Spawned three executors as support, before speaking to Peter (R22).
- Went dark on a peer's grant when told to stop the jobs (R23).
- Put four standing decisions to Peter in one modal, including a focus question settled for two weeks
  and recorded in this seat's own resume file.
- Told Peter the agent worktrees were a mess made in pi's lane; the harness puts every agent worktree
  under `UnaOS-hw-pi4/.claude/worktrees/` regardless, and 33 predate this session.
- Inferred "the S7 artifacts are on no ref" from an `ls-tree` that could only ever prove "not pushed";
  pi 7 relayed the error onward before either seat checked. They were committed all along.
- Counted `panel_*` fns with a pattern that cannot match `pub(crate) fn`, got 3 where the answer was
  5, and nearly told pi 7 their correct count was wrong.
- Committed once on a RED gate: `bash ledger-check.sh | tail -2 && git commit` takes *tail's* exit
  status. A gate whose result is piped is not a gate.
- Wrote `→ S32` in prose one row after warning about exactly that, and reddened the gate.

## Flagged for rmbp 13 / Peter

- **J1, the rmbp landing, is still owed** and still needs a fleet: S9 S10 S26 + PWRNAME/BOOTFADT and
  the commit that stops trunk printing `[orinreboot]`.
- **SR1's class is open**: one gate would assert every Pi-eligible knob has a `K8_FEATS` arm or an
  explicit allowlist. Two instances found a week apart by two different seats.
- **The behavioural re-arm** on `xhci/mod.rs` is the next ask if `dup=` climbs on render7 — a change
  to this driver's pointer semantics, so it returns to this seat.
- **B10, the R19 shut-out register, was never started** — the executor doing it was killed at minute
  four. It remains a reading task.
- Peter's standing, unasked: **A4** (card as default startup volume), **B7** (vug arbiter placement),
  **S27** (138 prune-candidate refs, list six weeks old).
