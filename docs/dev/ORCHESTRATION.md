# Orchestration runbook — the Maestro integrator seat

> ⚠ **REFRESHED 2026-07-17 (wolfpack handoff).** This doc had drifted a model-generation + an
> org-model behind — it called the seat "Fable," declared a dissolved 3-parallel-executor /
> single-seat-Fable-executes model, "independent tracks / no rounds," and a renamed restart
> pointer. The current truth is in this header + Roles; where OLDER SECTIONS DOWNSTREAM still say
> "Fable"/those models/the old battery, **this header and the memories [[unaos-seat]] +
> [[unaos-review-calibration]] OVERRIDE them.**
>
> This is the durable procedure for the **Maestro orchestrator seat** (architect + integrator +
> reviewer) in UnaOS development. Version-controlled + pushed so the role survives the loss of any
> session. A fresh seat (ANY model) resumes by reading THIS file + the live-state memory
> **`unaos-seat.md`** and the authoritative round baton it names (`active/unaos-maestro-r<n>.md`).
> Ground rules for all sessions: [`../../CLAUDE.md`](../../CLAUDE.md); direction [`../ROADMAP.md`](../ROADMAP.md);
> security [`../SECURITY.md`](../SECURITY.md).

## Roles

- **Maestro = architect + integrator + reviewer** (one long-running seat; named *Una(Maestro)* — a
  seat, not a model; whatever model holds the chair is Maestro). Writes the per-lane briefs,
  adversarially reviews before every merge, merges reviewed arcs to `main` (`--no-ff`), keeps
  docs/memory current. **Never an executor** (implements no arc code), **never a bench-debugger**
  (triage coordinator only at a live bench), **never a hands-on LC** (never builds media / relays
  or authors physical test plans).
- **Executors = Opus, `effort:'medium'`, spawned via the WORKFLOW WRAPPER** (the wrapper is the
  effort dial — a plain-Agent spawn inherits full session effort regardless of model). One arc
  each, one per lane, concurrent across lanes, serialized within a lane. `effort:'low'` for
  mechanical arcs. **NO Fable spawns** — the seat model is the seat's own judgment only.
- **LCs = per-platform sub-commanders** — own their platform THROUGH metal, including Peter's
  per-sitting brief + media staging; they brief + spawn executors, review, coordinate benches.
  Maestro never does an LC's hands-on work.
- **Peter = direction + hardware + push:** approves every new LINE of work, attends metal, runs
  `git push`. **Maestro merges; Peter pushes** — the full push line (all branches) at every landing.

**Work is organized in numbered ROUNDS (R<n>), milestoned internally. A round closes ONLY when its
METAL LEDGER is EMPTY (rule 0) — not at the last merge** (supersedes any downstream "independent
tracks / no synchronized rounds"). Concurrency rule: **one executor per lane, up to ~3 lanes,
serialize within a lane** (supersedes the older scattered one/two/three-arc numbers). Keep the
session STEERABLE ([[unaos-keep-it-steerable]]): don't bury it in concurrent background agents.

## The loop

```
executor lands arc on its track branch
        │
        ▼
Peter: "<track> landed"  ──►  Fable: review → merge → verify → rebase that track → write next brief → update memory + give push list
        │                                                                                                        │
        └──────────────────────────────── same one-line kickoff, next arc ◄─────────────────────────────────────┘
```

Tracks are **independent**: no synchronized rounds, no track waits for another.
Fable integrates each track's arc as it lands and rebases only that track. Cap:
**one unmerged arc per track**.

### The landing ping (end-of-round message, executor → integrator)

An arc round ends with a **cross-session message from the executor to the
integrator session** (Claude Code cross-session messaging; Peter relays if the
seat is cold). Timing rule: **the ping goes out AFTER the operator's metal
attempt** when the arc has a metal half and the flash is imminent — the ping is
the "review me now" signal, and a merge should carry the silicon verdict (and
any metal-only fix) inside the arc, not trailing it. If metal is genuinely
deferred (no hardware day planned), send the ping with an explicit
**METAL PENDING + why + when**. Content: commit hash(es), DONE-gate results,
metal verdict or the pending marker, any lane flags to ratify, and next-arc
input for the re-brief. The integrator holds the merge for an imminent metal
verdict; review can start immediately either way.

## Integrator procedure (step by step)

### 0. Cold-start — assess reality before trusting memory

Memory reflects the moment it was written; verify against the repo:

```
git -C <repo> log --oneline -6 main
for wt in UnaOS-rmbp UnaOS-pi4 UnaOS-jetson; do
  p="$(dirname <repo>)/$wt"
  echo "$wt: $(git -C "$p" rev-parse --short HEAD) ($(git -C "$p" branch --show-current))"
  git -C "$p" log --oneline main..$(git -C "$p" branch --show-current)   # unmerged arc(s)
done
grep -H '^\*\*LANDED\|^\*\*READY\|^\*\*HOLD' ~/.claude/plans/unaos-opus-*.md  # brief STATUS lines
```

A track whose HEAD is ahead of `main` has an unmerged arc waiting for review.

### 1. Review a landed arc (before any merge)

Adversarial, read-only, one reviewer per arc plus a refuter panel per must-fix.
For a substantial arc use the Workflow pattern (see the review workflow from
integration round 1 — one `agent()` per arc with a findings schema, then
`parallel()` refuters per must-fix, `must-fix` = would block merge). For a
small, well-described follow-up (e.g. a metal-visibility fix touching 2–3
files), a direct read of the diff + `./arroyo check` is proportionate. Check:
gate honesty (does the diff actually produce the brief's claimed DONE output),
lane compliance, arch correctness, cross-arc collisions (shared files:
`arroyo`, the two userspace/security docs, shared kernel-core). Only a
confirmed must-fix blocks the merge; notes fold into the next brief.

### 2. Merge (Fable, on `main`, from the main checkout)

```
git merge --no-ff <track> -m "Merge <track>: <arc> — <one-line what+why>
<short body: what landed, review verdict, deferred notes>
Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

`--no-ff` matches the trunk's merge-commit style. Expect clean auto-merges —
lanes (x86 vs aarch64) rarely collide; the shared docs have merged cleanly in
non-adjacent hunks. If `arroyo`/docs conflict, resolve by union (keep both
tracks' additions).

### 3. Verify the merged `main` (full battery)

From `<repo>/unaos`:

```
./arroyo check                    # both arches
./arroyo test 22                  # x86: expect the U-arc PASS lines + SMP + no faults
./arroyo test-arm 22              # aarch64 virt GICv2: self-SGI v2 + timer live
UNAOS_GICV3=1 ./arroyo test-arm 22   # aarch64 virt GICv3: self-SGI v3 + timer live
./arroyo kernel8-test 30          # Pi raspi4b: blob + hello from EL0 + M6b PASS + CAPSTONE 6/6
```

Inspect serial logs with `awk`/`grep -a`, never plain `grep` (control bytes).
All pass lines present + no new fault lines = green.

### 4. Rebase the tracks

```
for wt in UnaOS-rmbp UnaOS-pi4 UnaOS-jetson; do
  git -C "$(dirname <repo>)/$wt" merge --ff-only main
done
```

A track mid-arc (HEAD ahead of the old main) won't fast-forward — that's a
signal it has unmerged work; review/merge it first, then it ff's.

### 5. Author the next brief

Rewrite `~/.claude/plans/unaos-opus-<track>.md` to the next arc, keeping the
self-driving template:

- **Kickoff one-liner** at the top (Peter's convention):
  `Read ~/.claude/plans/unaos-opus-<track>.md and follow it. If no questions — have at it.`
- **"To the session reading this"** contract: worktree/branch pinning, read
  repo `CLAUDE.md` first, refuse-if-STATUS-says-LANDED, base gate, ask-only-if-
  ambiguous-else-proceed, land-and-update-STATUS.
- **STATUS**: READY/HOLD + base gate (which `main` commit must be in history).
- **The arc**: register/sequence-exact where possible; name an in-tree pattern
  to mirror by `file:line`.
- **DONE gate**: exact expected serial/QEMU output strings + `./arroyo check`
  both arches + the track's regression + the doc update.
- **Lane** (files touchable / forbidden) + **STOP tripwires**.
- **Fold in the review notes** from step 1 that belong to this arc.
- Mark **Opus-ready** only if: registers/sequences fully specified, an in-tree
  pattern to mirror, a QEMU/host-verifiable gate, no open hardware unknowns.
  Otherwise keep it Fable-led (novel bring-up, hardware decisions, metal-only).

### 6. Update state + hand Peter the push list

- Update the live-state memory (`unaos-fable-orchestrator-restart` +
  `unaos-multiuser-redirect`): new `main` tip, what landed, what's in flight.
- Give Peter the push list: `git push origin main <ff'd track branches>`, and
  `git push --force-with-lease origin <branch>` for any branch whose history was
  rewritten (e.g. a reset+cherry-pick). **Never** advise a plain `git pull` on a
  rewritten branch — force-with-lease only.

## Single-seat CAMPAIGN mode (adopted 2026-07-07, Peter's call — the current mode)

The 3-parallel-Opus-executor model is dissolved: **Fable executes the arcs
directly** as well as reviewing and merging, working with Peter in **campaigns**
— long focused runs, each a coherent chapter with a base goal + stretch,
leapfrogging across tracks by chapters (bring one track up, the next catches up
with its twin or passes). What changes and what doesn't:

- **The unit is the campaign, not the arc.** Internally it stays milestone'd
  exactly as before: commit-sized milestones, each green + committed on its
  track branch, targeted gates per milestone, one full battery at the
  merge-window close. Batteries batch; blast radius doesn't grow.
- **The adversarial multi-agent review is now the COI guard** — the seat
  reviewing wrote the code, so the independent Workflow panel (lens reviewers +
  refuter panels per must-fix) runs before EVERY merge, and confirmed must-fixes
  are fixed in-arc and re-gated before the merge proceeds. Campaign 1's U7
  panel confirming 2 must-fixes the author-seat had already gated green is the
  standing proof this discipline is load-bearing.
- **Checkpoint discipline:** a clean baton (commit + resume docs + memory) at
  every milestone boundary, so context/credit exhaustion never strands work.
- **Jetson campaigns are sized by attended-boot batches** (Peter's bench time is
  the scarce complement): prep everything offline, batch questions per boot.
- Unchanged: track branches + `--no-ff` merges to main, the DONE gates, docs
  current by construction, **Peter attends metal and pushes — Fable never
  pushes.** The one-unmerged-arc cap generalizes to one unmerged CAMPAIGN
  milestone chain per track, merged in-order within the campaign.
- The per-track briefs remain the arc contracts (STATUS flips to LANDED at
  close); when a campaign spans tracks, the campaign plan file names the order.

## The metal-verification gate (Peter's directive, 2026-07-11)

**A round does not CLOSE until every metal-dependent arc in it has been
bench-verified with the seat LIVE.** The old pattern — land QEMU-green, defer the
metal confirm to "the next attended bench, someday" — grew an unbounded backlog
(round 5 closed with S4 races, the S1–S3 re-bench, the VPERF readout, scale-2, and
the K1 survive-reboot proof all still pending). That stops. Per arc, at brief time,
the seat picks one of two patterns:

- **(1) bench-FIRST** — when a *metal unknown gates the design*: run the bench
  before coding the dependent arc. Example: VPERF-WC's whole existence depends on
  the real framebuffer memory-type readout; a driver bring-up depends on the real
  device's behavior. Don't write code whose shape a metal fact will decide.
- **(2) bench-as-CLOSE-gate** (Peter's preference where possible) — the arc codes
  QEMU-green, then the round's attended metal bench runs as a **required step
  before round close, with the seat live** so any metal-only fix folds in-round
  (the STOR-1 precedent: fixes fold + re-gate before the merge proceeds). The seat
  holds the round open for the bench; it does not declare the round done on
  QEMU-green alone.

**Batching** (Peter's bench time is the scarce complement): the seat collects a
round's metal-dependent items into ONE attended bench per platform and runs them
together — the jetson track already sizes work this way ("attended-boot batches").
Prep everything offline; batch the questions per boot.

**Bench-session etiquette** (Peter, 2026-07-14):
- **Every platform metal runbook carries a paste-ready OPENER at its top** — the
  kickoff prompt Peter hands a fresh bench session. Whoever adds a pending item
  to a runbook refreshes its opener to match; a bench must never start with the
  session (or Peter) surprised about role, media state, or what's pending.
- **After a bench session's final landing report, the Maestro replies ONLY if
  something needs fixing or deciding.** A clean report needs no acknowledgment —
  the session is expected to be over; the fold happens on the Maestro's side and
  the docs are the receipt. The why (Peter): a closing ack between coordination
  surfaces is internal dialog published as if it were signal — the work product
  (folds, commits, docs) is the communication. (Mid-bench questions — a paused
  board waiting on a call — are the opposite: answer those immediately, they
  block silicon time.)

**Code-prerequisite sequencing**: metal that is blocked on a code prerequisite is
sequenced explicitly, not counted against the gate for the current round. Example:
K1 survive-reboot enforcement can only be metal-proven once K2 lands a second
launchable named program (flips `by_name_spawn_multivalued` live) — so K1's
survive-reboot metal rides the round that lands K2, not before. The gate applies
to metal that *can* run given the round's code.

**Tooling to make the gate cheap**: **`mbench` — landed (round 6)**. The
metal-bench harness (`unaos/scripts/mbench.py`; witness specs in
`unaos/scripts/specs/*.spec`) asserts a serial capture — a live bridge log or a
finished QEMU log — against a checked-in spec and prints one battery-style
verdict table. Usage:
`./arroyo mbench --follow ~/pi-serial.log --spec scripts/specs/pi4-regression.spec --timeout 120`
(`--replay <log>` for finished captures; `--self-test` needs no hardware; spec
directives are REQUIRE / COUNT / OPTIONAL / FORBID / PENDING — PENDING ships a
witness ahead of its bench and is promoted to REQUIRE once first captured). It
reads bridge LOG FILES only, never serial devices, and `--inject` is
pi/jetson-only (the x86 FTDI console is TX-only). An attended bench is now
run-the-script-get-pass/fail — which is what makes a per-round metal gate
affordable rather than a tax.

### The bench-debug boundary (Peter's calibration, 2026-07-11)

Metal debugging is EXECUTOR work. When a bench goes sideways (unexpected fault,
metal-vs-QEMU divergence), the seat's live role is **triage coordinator**: watch
serial, decode and adjudicate the evidence, launch read-only diagnosis panels,
write the fix brief, review and merge the fix. The hands-on loop — building
candidate kernels, flashing media, driving bisect boots — belongs to the track's
executor session or, when the bug spans lanes, a **dedicated debugging session**
(Peter-kickoff, cross-lane grant written in its brief). Precedent: the round-6
jetson boot-crash triage, where the seat drove the bisect itself — ratified
afterward as the wrong altitude; the diagnosis was sound but the driving seat
belonged to a debug session.

## Guardrails specific to this seat

- Fable reviews **before** merge and **before** metal, always. QEMU-green ≠
  correct; metal happens at arc boundaries (Peter).
- Merge only reviewed arcs. In campaign mode a confirmed must-fix is fixed
  in-arc by the executing seat and re-gated before the merge (the review panel
  re-verifies the fix path); in executor mode it goes back to the track as a
  fresh brief, not patched by Fable on the track branch.
- Keep the roadmap/security/userspace docs current *as arcs land* — docs stay
  correct by construction, not in a later sweep.
- Physical-hardware arcs (truck/TALUS, printer) are payloads of the multi-user
  chain and stay deferred until the desktop foundation is far enough along; see
  ROADMAP §1 for the gating.
