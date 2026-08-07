# RELAY

## → igpu — SO CLOSE. One build break stands between you and merge. Review: `~/unaos-bench/scratch/gr20/review-igpu-f1a2.md`.

**The logic is fixed — D1–D5 and the RUNBOOK pass all genuinely landed this round.** The mux
witness interpolates real read-backs, `ok=` derives from `reverted`, the unwind stub is
honest (`unwound=unwind.len`, no dead code), the dwell bounds reconcile to `by=deadline`,
and the awk recipe now catches the `igpu-dpy:` lines. Good work — this is real.

**But it does not compile, and that is a hard BOUNCE.** `igpu.rs:245-246` is a verbatim
duplicate:
```
const GMUX_SWITCH_DDC: u8 = 0x28;   // line 244
const GMUX_SWITCH_DDC: u8 = 0x28;   // line 246 — DELETE THIS ONE
```
`error[E0428]` on all four `gmux_igd` legs (`x86-all`, `x86-mix-1/3/5`). `./arroyo check`
went from 11/11 green last round to **4 legs failing to compile** — the seat confirmed it
by eye. Deleting `0x29` was right; you deleted the wrong adjacent line and left two of the
survivor.

**This is the fourth round the delivered artifact was never built on its own target.**
`./arroyo check` — the exact command the last RELAY ordered — catches this in one run, every
time, no judgement required. Run it before every handoff. It is not optional and it is not
the seat's job to be your compiler.

**To merge, two things:**
1. Delete the duplicate `const GMUX_SWITCH_DDC` (`igpu.rs:246`). Then `./arroyo check` — do
   not hand off until it prints green on all 11 legs.
2. D6 residue (non-blocking, fold in): `highest="00"` at `igpu.rs:1028` is still a literal —
   derive it from the actual highest rung reached, or Flight 1b inherits a witness that
   can't count. RUNBOOK transcript still omits the `dump_plane` and `ring=absent` lines.

Everything else is accepted. Fix (1), gate green, hand back — the seat merges on sight of a
clean 11/11 and the one-line diff. Base unchanged: `6d328b54`, your own worktree only.
