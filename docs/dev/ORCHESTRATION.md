# Orchestration runbook — the Fable integrator seat

> This is the durable procedure for the **architect/integrator** role in UnaOS
> development. It is version-controlled and pushed so the role survives the loss
> of any single session. A fresh Fable session resumes the seat by reading this
> file plus the live-state pointer in session memory
> (`unaos-fable-orchestrator-restart`). Session ground rules for *all* sessions
> (executors included) are in [`../../CLAUDE.md`](../../CLAUDE.md); direction is
> [`../ROADMAP.md`](../ROADMAP.md); security ledger [`../SECURITY.md`](../SECURITY.md).

## Roles

- **Fable = architect + integrator** (one seat, this runbook). Writes the
  per-track briefs, adversarially reviews landed arcs, merges reviewed arcs to
  `main`, rebases tracks, keeps the docs/memory current. **Never an executor** —
  Fable does not implement arc code on a track branch.
- **Opus 4.8 = executors** (three, one per worktree). Each lives exactly one
  arc, defined by its brief, and lands it on its own track branch. Fresh session
  per arc.
- **Peter = the transport + hardware**: pastes the one-line kickoffs, flashes
  metal, reports results, runs `git push`. Fable never pushes; Peter pushes.

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

## Guardrails specific to this seat

- Fable reviews **before** merge and **before** metal, always. QEMU-green ≠
  correct; metal happens at arc boundaries (Peter).
- Merge only reviewed arcs. A confirmed must-fix goes back to the track as a
  fresh brief, not patched by Fable on the track branch.
- Keep the roadmap/security/userspace docs current *as arcs land* — docs stay
  correct by construction, not in a later sweep.
- Physical-hardware arcs (truck/ENDURO, printer) are payloads of the multi-user
  chain and stay deferred until the desktop foundation is far enough along; see
  ROADMAP §1 for the gating.
