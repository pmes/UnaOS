# ORIN — WHAT IS IN FLIGHT AND WHAT IS OWED

One file, fixed path, not per-round. Kept current AS WORK HAPPENS, never "at close":
a list written only at close is never written at all by a session that does not reach one,
and a session ends when the work is done or when it is not worth continuing.
`✓` = verified in this tree.  `·` = inherited, NOT re-checked.  Re-derive every sha before acting.
Last touched 2026-09-12 by orin 27: rung 4a+4b FLOWN in one boot, PASS/PASS (A51); tear and unafsroot still uncommitted in their orin26 worktrees.

## STATE — re-derived 2026-09-11T16:36Z against origin, not relayed
✓ origin/main 084b79ac · origin/hw-jetson 44218ab4 · origin/hw-rmbp 02a6896a · origin/hw-pi4 98914795
✓ hw-jetson 0 behind trunk, 0 unpushed. The three-way sync is DONE (it needed four steps, not three).
⚠ **exec-orin24-fold `63654d93` is GATED GREEN and UNPUSHED** — 54 above hw-jetson: the render13 base
  merged with both landings, four conflicts resolved by provenance, and the first gate that branch
  has ever had. Also unpushed: exec-orin25-shotverb `edf8483a` (WIP, does not compile, preserved).
  Push line: `git push origin hw-jetson exec-orin24-fold exec-orin25-shotverb`

## RENDER13 IS HELD — Peter 2026-09-11
✓ Specified as fold + core0 + el0slot + mem + tear + unafsroot. Measured: core0/el0slot/mem are at
  ZERO commits, tear is diagnosis-only, unafsroot incomplete. It would have fixed NONE of the four
  glass defects. Held rather than fly a number implying progress it does not have.
✓ orin 27 2026-09-12: RUNG 4 FLOWN, ONE BOOT (Peter 2026-09-11: combine 4a and 4b). Image ga10b4ab-20260912T0019Z-277e2ca at hw-jetson 277e2cab, KELF max=0x238368. 4a `bcrheld=7/7 -> BCR-ALLHELD`; 4b `br_retcode=0x00000002 -> BROM-VERDICT-FAIL` (the PASS), SYSTEM_OFF. Scorer PASS 4a exit 0, PASS 4b exit 0. Evidence docs/dev/evidence/orin27/ga10b4ab-boot1.log; A51 flown. Boot 1 (2026-09-11) REFUSED no-dma-window, fixed by 1629d449. Next rung is a clean-room question (what the ROM wants at fmccode/fmcdata/pkcparam), brief §7.
· (history) orin 26: Peter 2026-09-11 "do the 4 boots". Rung 4a/4b BUILT (exec-orin26-ga10b4 a39fd32d, A51), two images STAGED
  (`ga10b4a-20260911T1809Z-a39fd32` max_vaddr 0x23cae8 → `ga10b4b-20260911T1809Z-a39fd32` 0x23f120), card line dry-run
  refuses only on device readability (needs sudo) and geometry (`--expect-geom 1:0b:2048:63401984`, from render12's FLIGHTID).
  Butler on ttyACM0, MARK ga10b4a written, waker armed. Boot 1 = 4a; boot 2 = 4b only on `-> BCR-ALLHELD`.
✓ Four defect executors spawned 2026-09-11 off the fold 63654d93 in ~/unaos-bench/scratch/orin26/{core0,el0mem,tear,unafsroot}
  on branches exec-orin26-*; tear carries 7b0a9804 merged, unafsroot carries a81f38c7 merged (one spec conflict, the executor resolves).

## THE FOLD'S FIRST GATE — seven findings, none of them code
✓ 3 malformed ledger rows (SR11: `raised=yes|no`, `src=scene\|flat`, a `|` where an em-dash belongs)
✓ 3 unregistered knobs, all orin's, all NA — `kernel8()` builds its own K8_FEATS and never reads
  the `_feats` block they write (arroyo:6249)
✓ 1 STALE registry row (UNAOS_SDMMCROOT) visible only with UNAOS_K8REACH_STRICT=1 forced — SR13,
  and it was orin 25's own cleared SP5 hit coming back: a feature lives in THREE places
  (declaration, cfg sites, registry) and only two were enumerated.

## NEEDS PETER
· "One file is the rulebook, EVERYTHING ELSE DELETED" — the absorbing half ran at 3cfe28ad; the
  deletion half has never started. Destructive, R20, and the union is not yet correct.
· D1 (orin-ledger §D) loader `SetMode` to the widest console mode — knob-gated, one power cycle.
· D2 (→S7) `render_service` size-3 family convergence arc. Entry carries an expiry.
· S27 138 prune-candidate origin refs, never OK'd.

## THREE-WAY, AFTER THE PUSH — needs all three seats, nobody edits alone
✓ P14's sentence "no gate can fix this: a checker cannot see another branch" is FALSE of this script
  now: `ledger-check.sh:118` already enumerates `refs/remotes/origin/hw-*` and `origin/main`. rmbp 19
  found it, declined to edit a shared protocol row alone, pi agrees it is stale. **Orin seconds it.**

## ORIN'S OWN, OPEN
✓ `tegra_shell_remint` — R16 board-named AND named after a function it does not call
  (`wm::shell_remint` re-points a row; this mints via `create_at`). Rename is orin's; SR9's
  caller-vs-callee discriminator in GATE-FAMILY is rmbp's.
✓ **SO ids get no suffix form, ever** — a naming constraint, deliberately NOT a gate. The id
  extractor `([A-Z]+[0-9]+)` is unanchored on purpose (to admit `A36 (→ SR2)`), so `SO21b` silently
  aliases to `SO21` and the uniqueness check cannot tell them apart. Found by rmbp 19's mutation.
✓ SO6's row exists now (rmbp filed it under orin's granted id). Four days unwritten, not six weeks —
  the "six weeks" was S27's age, relayed onto the wrong finding.

## STARTED, NOT FINISHED
✓ exec-orin24-tear        7b0a9804  1 commit, diagnosis only. No instrument, no fix.
✓ exec-orin24-unafsroot   a81f38c7  3 commits, incomplete. Owes rmbp the main.rs /
                                    drivers/block.rs diffs BEFORE it moves.
✓ exec-orin24-fold        0cdb3654  holds sdv1 + evidence. NO GATE has ever run on it.
· exec-orin22-bootroot    600887c2  B98 clone-merge fix HALF-APPLIED, uncommitted, in an agent
                                    worktree. Landing-blocking for bootroot.

## CUT AND NEVER BEGUN — zero commits, all at 1b50376a
✓ exec-orin24-{core0, el0slot, mem, small, tools}

## DEFECTS OFF RENDER12 (flown 2026-09-09 · scorer11 6/6 · 0 faults, 0 panics, 3 boots)
✓ EL0 never leaves core 0 (`el0cpus=0x1` every sample)               → core0
✓ 7× `slot 4 backing allocation FAILED`; apps refuse to launch       → el0slot / mem
✓ tearing on the glass while every rollup reads `torn=0`             → tear
✓ `unafs=absent`; `/` does not mount UnaFS                           → unafsroot

## OWED FLIGHTS AND CONTROLS
· FRIEND-DIFF positive control — owed since render11. Needs the data card mounted AND root
  write-locked. Without it that leg has never been shown able to go red.
· Glass checks never evidenced: cursor sweep (no `restore src=` has fired), taskbar order across
  quarry open/close, EL0 placement off core 0 (blocked by core0/el0slot/mem).
· render13 = the fold + core0 + el0slot + mem + tear + unafsroot, one gate, one card line.
✓ GA10B rung 4: FLOWN 2026-09-12, PASS both rungs in one boot (A51). Nothing owed on this rung.

## SMALL FIXES — none started
· shell.rs `screenshot` prints OK with no device (shared core — ask rmbp).
· x86 `[wm] close-scope win=0` on the close-box path (rmbp flag).
· wintitle out-of-lane leftovers (SO22): `app_name_forget` from `teardown_user_slot`, two x86
  launchers minting unnamed windows, `b"orin"` title on the display_tegra test pattern.
· `read_block_at` `(lba*512) as u32` truncation above 4 GiB — flagged by sdv1, never ledgered.
· NIC descriptor-17 OPEN (R19); net5V REFETCH-WRONGSLOT=3 carried since render8.

## TOOLING / AUDIT
· scorer11 STAMP-MATCH leg (wire `sha=` == card elf stamp, WRONG-IMAGE exit 1).
· orin-specscore.py / mbench --self-test are run by NO verb (ARCH-CONFORMANCE 14).
· 28 board-named shared scripts — R4 scour, its own arc.
· media-writer.sh is un-versioned (rmbp B97).
· winmenu APP_OWNER unregistered WinId holder (pi 10's question) — prove or refute.

## STANDING — how this seat reads, after a day of getting it wrong
    git log --all --not HEAD -- <file>                    # what I lack: an enumeration, not a pick
    git ls-tree -r --name-only <ref> -- docs/ | comm -23 - <mine>   # files I don't know exist
    UNAOS_LEDGER_STRICT=1 ./arroyo check                  # a detached HEAD gets the WEAK gate
  Never `| head` on evidence you are about to quantify over. Never a number from memory.
  The population a gate checks is not the population a seat greps (pi 10, 2026-09-10).
