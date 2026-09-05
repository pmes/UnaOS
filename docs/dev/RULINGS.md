# RULINGS — Peter's words, verbatim, dated, with the session that heard them

> A relay of a ruling is a report, not the word (LEDGER.md P4). This file holds the WORD: the quote as
> typed, the date, and which seat heard it in its own session. Every other document — LAWS.md,
> CLAUDE.md, batons, memory files, ledgers — links a ruling by id (`R<n>`) instead of paraphrasing it.
> A paraphrase, where one is unavoidable, is marked *(paraphrase)*. Never edit a quote. Rulings get
> REVERSED (the cube ruling was retracted; EVAC was rejected at its premise), so every row carries a
> `status` — **live** · **superseded** · **retracted** — and a `superseded-by` naming the R-id that
> replaced it. A reader who finds a dead ruling must be able to see it is dead. GATE-LEDGER checks both.

> Numbering: rmbp 11 seeded R1–R7 on hw-rmbp; orin 13 appended R8–R17 on hw-jetson. One sequence after the union
> merge (reconcile #2, 2026-09-05); never renumber an id that has been cited. Each seat appends what it heard in its
> own session; merges are by union.

| id | date | heard by | quote (verbatim) | applies to | status | superseded-by |
|---|---|---|---|---|---|---|
| R1 | 2026-09-03 | rmbp 11 | "orin is the focus." | focus assignment for the week of 2026-09-03 (bench) | live | — |
| R2 | 2026-09-03 | rmbp 11 | "orin 13 is right fucking there we do now wait for i do not even know what arc you refer to" | peers are live: a cross-seat ask goes over ccd in the same turn, never "noted for the arc" (LAWS §COORDINATION; memory `message-peers-same-turn`) | live | — |
| R3 | 2026-09-03 | rmbp 11 | "if you want to do unattended reboots you cannot because there would be nobody to hold down option" | the rMBP's ⌥ picker is the human step; unattended x86 reboots need the card as the default startup volume first (rmbp-ledger A4) | live | — |
| R4 | 2026-09-03 | rmbp 11 | "how do we stop poisoning non-arch specific code? clear from the start orin would never be the only machine in unaos to reboot for fucks sake" | board names out of arch-neutral code; subsystem names the witness (LEDGER S6, GATE-NEUTRAL; memory `name-by-subsystem-not-board`) | live | — |
| R5 | 2026-09-05 | rmbp 11 | "and adversarially look for ways to improve discussing among your teammates" → clarified: "by that i mean look for ways to improve your internal issue tracking" → "don't discuss it with me discuss it with your teammates" | the seats converge on tracking changes among themselves and report OUTCOMES; the result is GATE-LEDGER, the status enum, evidence-in-git, this file | live | — |
| R6 | 2026-09-05 | orin 13 *(rmbp relays; orin appends the verbatim line)* | *(paraphrase)* audits and inventories are high value; the waste is trashing results in scratch and re-running rather than ticking things off a list; each track keeps its arch ledger and there is one over-arching ledger | LAWS §Ledgers; LEDGER.md; the arch ledgers | live | — |
| R7 | 2026-09-05 | orin 13 *(rmbp relays; orin appends the verbatim line)* | *(paraphrase)* make sure your teammates know you are probably finding things they need to know | LAWS §COORDINATION; LEDGER.md owner column + same-turn message | live | — |
| R8 | 2026-09-05 | orin 13 | "already pushed what are you waiting for there is tons of work to do" | a pushed landing is not a stop; the focus seat keeps the floor full | live | — |
| R9 | 2026-09-05 | orin 13 | "that's minor tho so much is broken on orin" / "the panel looked fine i just mean all the hardware is just barely supported" | ORIN-HW inventory (orin-ledger §F); the five ranked gaps | live | — |
| R10 | 2026-09-05 | orin 13 | "yes, it is trying to load the old background pulse" | the pulse window is retired from the Orin's scaffold pass (orin-ledger A10, B1) | live | — |
| R11 | 2026-09-05 | orin 13 | "but the shell needs to go into its window and the desktop made a normal desktop so is the work worthwhile?" | scaffold polish stops; the cascade (`deskcascade`) is the arc | live | — |
| R12 | 2026-09-05 | orin 13 | "we are most certainly going to hit the 5 hour limit as we ate thru 2/3 in 1/2 hour!! i am watching. run high value jobs so we can feel the glory" | budget discipline: no exploratory fleets; one high-value executor at a time | live | — |
| R13 | 2026-09-05 | orin 13 | "inventories and audits are high value i just feel like you trash your results and have to rerun rather than ticking things off a list" | LEDGER.md + per-arch ledgers; every audit briefed with the ledger | live | — |
| R14 | 2026-09-05 | orin 13 | "make sure your teammates know about keeping better track and that you are probably finding things they need to know" | cross-lane findings go on LEDGER.md with an owner AND to that seat the same turn | live | — |
| R15 | 2026-09-05 | orin 13 | "each of you should have your arch list and the over-arching list for all" | `docs/dev/OS/<track>-ledger.md` ×3 + `docs/dev/LEDGER.md` | live | — |
| R16 | 2026-09-03 | rmbp 11 (relayed to orin 13) | (paraphrase, as relayed) platform names must stop leaking into arch-neutral code — "clear from the start orin would never be the only machine in UnaOS to reboot" | subsystem-named witnesses/symbols in shared files (PWRNAME; GATE-NEUTRAL) — rmbp holds the verbatim | live | — |
| R17 | 2026-09-05 | orin 13 | "good work! card in. prt sc worked! once. hit the button twice and the 2nd file is empty. it looks like you killed the wrong pulse. the windowed one is gone, the embedded one is there and so is the old status bar at the bottom. both must go." | A17 (second capture empty); A18 (embedded pulse + bottom status bar go; windowed pulse returns) | live | — |
