# WHITE BOARD — 2026-08-06 (GR18 closed)

## 1. The two Gemini lane branches fork 341 commits back. Rebase, or re-cut?

`wt/gmux-igd-x86` (igpu, 3 commits, `igpu.rs +386`) and `wt/kepler-poke-x86` (kepler,
3 commits, `kepler.rs +752/−178`) both branch from `0913b91e` — **341 commits behind
trunk**. Neither can be merged as-is: the diff against `776fb13c` reads as a deletion of
the entire GR18 round (`x86-witness.spec` −769 lines, the other three x86 specs, ~51 700
lines total).

Background you need to answer it: their real work is small and good (the gmux
revert-in-the-arming-stream fix + black-panel runbook; the ECHO/POKE split putting the
0x409504 read in a terminal image). Trunk has since rewritten the same two files —
`kepler.rs` gained the phase instrument and the seat's `phase!` scope fix, `igpu.rs`
gained the whole BLT ring. A rebase is a real conflict resolution in both files, not a
formality.

**Option A** — the seat rebases both onto `776fb13c` and reviews the result (costs seat
time, keeps their commit history and their walkthrough docs intact).
**Option B** — the lanes re-cut their arcs on current trunk from their own walkthroughs
(costs their time, gives clean history, risks losing detail only the code carries).

The seat's recommendation is **A**, because the walkthroughs and the runbook are the
expensive artifacts and they survive a rebase untouched. Say the word either way — this
is the only thing blocking Gemini's work from landing.
