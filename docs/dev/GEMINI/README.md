# Gemini specialist sessions — START HERE

This file is the reliable entry point for a **new Gemini session**. Read it
top to bottom, then read the files it names, in order. Do not start from
memory of a previous session.

## 1. Who you are

You are ONE specialist on ONE lane, working on branch **`UnaOS-gemini`**
(worktree `~/src/github.com/pmes/UnaOS-gemini`). The lanes:

| Lane | Code file you own | Doc dir | Brief/proposal file pattern |
|---|---|---|---|
| **kepler-fence** (PFIFO/scheduler) | `unaos/crates/kernel/src/gpu/kepler.rs` | `video/Kepler/` | `*kepler-fence-pull<N>*` (pulls ≤13 used `*kepler-pull<N>*`) |
| **kepler-display** (scanout) | `unaos/crates/kernel/src/gpu/kepler_display.rs` | `video/Kepler/` | `*kepler-display-pull<N>*` |
| iGPU (Intel HD 4000) | — | `video/iGUI/` | **ARC CLOSED** (s10) |
| aether | — | `www/aether/` | **ON HOLD** (Peter lifts it) |

You touch ONLY your lane's code file (plus your own docs in your lane dir).
Anything else — bootloader, builder, main.rs, the other lane's file — is a
STOP: say so in your proposal/report; the coordinator handles it.

## 2. Read order at session start

1. This README.
2. `RELAY.md` (this directory) — the coordinator's current message to each
   lane, overwritten every round. After EVERY `git pull`, read your lane's
   section first; it is the freshest instruction and outranks stale briefs.
3. `video/INDEX.md` — the map: every pull, its status, the naming authority.
3. Your lane's newest `BRIEF-*` (the coordinator writes these; your work is
   defined by the newest brief with no `PROPOSAL` answering it).
4. `docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` — silicon facts of record,
   newest sitting first. **Trust it; do not re-litigate a refuted hypothesis.**

## 3. Workflow (review-before-run, standing since 2026-07-21)

1. Write `PROPOSAL-<lane>-pull<N>.md` in your lane dir, first line
   `STATUS: PROPOSED`. Commit and push it. **No implementation commits until
   the proposal is approved.**
2. The reviewer (Claude coordinator session) answers with amendments (a
   `REVIEW-*` note or relayed by Peter) or approval; the header becomes
   `STATUS: APPROVED (<date>)`.
3. Implement exactly the approved text. Deviations discovered mid-pull go in
   your report, never silently into code.
4. The proposal stays forever as the record (`STATUS: LANDED <commit>`
   appended at top after landing). Never delete it.

## 4. Commit discipline (hard rules)

- **Commit EVERYTHING you produce that is meant to exist**: code, your
  proposal, walkthroughs, review replies, doc updates. Work that exists only
  in your working tree does not exist — sessions end without warning.
- **Delete what is NOT meant to be committed** before you finish a turn of
  work: scratch scripts, experiment dumps, editor litter, abandoned drafts.
  `git status` must show a clean tree after each commit — no untracked
  leftovers for the coordinator to guess about. If you are unsure whether a
  file is worth keeping, say so in your report instead of leaving it silently.
- Commit on `UnaOS-gemini` only. Never merge, never force-push, never rebase.
  Message style: `gpu: <imperative summary> [knobs touched]`.
- **You do not push. Peter pushes.** End every report with the line
  "PUSH OWED: <n> commit(s) on UnaOS-gemini" so he knows the branch is ahead —
  work he hasn't pushed is work that can be lost.
- `git pull` before you start and before any file reorganization — the
  coordinator and the other specialist commit to this branch too.

## 5. Engineering laws (violations here have each cost a metal boot)

- **Full-knob gate**: green means
  `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check`
  for both arches, AND the marker strings visible in the builder-path
  artifacts. Plain `./arroyo check` proves nothing about gated code.
- Every probe prints **exact grep-able serial markers** (`:: kepler: ... ::`
  / `:: kdisp: ... ::`) — the brief names them; the bench greps the capture.
- **Bounded polls** everywhere (a 10M-read poll once masqueraded as a hard
  hang). Sentinels (0xBAD0BA20-style) on fallible reads — never ambiguous
  zeros. Absence-honesty (`absent?` labels) on registers that may not exist
  on GK107.
- **nouveau/rnndb register SEMANTICS are forbidden** on the Kepler lanes
  (the IDLE_FILTER incident); register ADDRESSES with citation are fine, and
  gmux/Intel hardware facts with attribution are fine.
- Keep the `all(unaos_ivb, intel-ivb, x86_64)` gate on the main.rs boot-trace
  handoff (dropped twice already; a keep-this comment marks it).
- `unaos_ivb` changes the shared BootInfo ABI — never arm it on one side of
  the bootloader/kernel pair.
- Claiming a compile you never ran is a firing offense for a proposal: run
  the gate, paste the result.

Plans-of-record from the integrator side (`PLAN-GEMINI-*.md`) stay in
`docs/dev/USERLAND/` / `docs/dev/OS/08_VIDEO/`; this tree holds the lanes'
briefs, proposals, reviews, and landed-pull records.
