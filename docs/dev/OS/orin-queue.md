# ORIN — WHAT IS IN FLIGHT AND WHAT IS OWED

One file, fixed path, not per-round. Kept current AS WORK HAPPENS, never "at close":
a list written only at close is never written at all by a session that does not reach one,
and a session ends when the work is done or when it is not worth continuing.
`✓` = verified in this tree 2026-09-09.  `·` = inherited from orin 24's list, NOT re-checked.
Re-derive every sha before acting.

## NEEDS PETER
✓ The one sentence from all three seats: LAWS §1, what a support seat may PRODUCE. Focus
  rotation untouched. §3 (text-only landing) named as the consequence, not asked for.
✓ "One file is the rulebook, EVERYTHING ELSE DELETED" — the absorbing half ran at 3cfe28ad;
  the deletion half has never started. Destructive, R20, and the union is not correct yet.
· D1 (orin-ledger §D) loader `SetMode` to the widest console mode — knob-gated, one power cycle.
· D2 (→S7) `render_service` size-3 family convergence arc. Entry carries an expiry.
· S27 (shared LEDGER) 138 prune-candidate origin refs, never OK'd, list now six weeks old.

## ANSWERED BY PETER TODAY — now owed as WORK, not as a question
✓ GA10B rung 4: "let's do the boots." exec-orin23-ga10b4 `e7c8eb24` is the brief (docs only).
  4a is free and answers the gate (do the BCR registers accept a CCPLEX write?). 4b spends the
  power cycle and ends in SYSTEM_OFF — attended only.
✓ A33/SO4 crystal placement: RULED AND FIXED. The ledger row still says "awaiting re-ruling."
  That row is wrong and needs closing.

## STARTED, NOT FINISHED
✓ exec-orin24-tear        7b0a9804  1 commit, diagnosis only. No instrument, no fix.
✓ exec-orin24-unafsroot   a81f38c7  3 commits, incomplete. Owes rmbp 18 the main.rs /
                                    drivers/block.rs diffs BEFORE it moves.
✓ exec-orin24-fold        0cdb3654  holds sdv1 + evidence. NO GATE has ever run on it.
· exec-orin22-bootroot    600887c2  B98 clone-merge fix HALF-APPLIED in an agent worktree,
                                    uncommitted. Landing-blocking for bootroot.

## CUT AND NEVER BEGUN — zero commits, all still at 1b50376a
✓ exec-orin24-{core0, el0slot, mem, small, tools}

## DEFECTS OFF RENDER12 (flown 2026-09-09 · scorer11 6/6 · 0 faults, 0 panics, 3 boots)
✓ EL0 never leaves core 0 (`el0cpus=0x1` every sample)               → core0
✓ 7× `slot 4 backing allocation FAILED`; apps refuse to launch       → el0slot / mem
✓ tearing on the glass while every rollup reads `torn=0`             → tear
✓ `unafs=absent`; `/` does not mount UnaFS                           → unafsroot

## OWED FLIGHTS AND CONTROLS
· FRIEND-DIFF positive control — owed since render11. Needs the data card mounted (sdv1 flies
  it) AND root write-locked. Without it that leg has never been shown able to go red.
· Glass checks never evidenced: cursor sweep (no `restore src=` line has fired), taskbar order
  across quarry open/close, EL0 placement off core 0 (blocked by core0/el0slot/mem).
· render13 = 1b50376a + sdv1 + core0 + el0slot + mem + tear + unafsroot, one gate, one card line.

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
· 28 board-named shared scripts (orin-/pi-/rmbp-/jetson-) — R4 scour, its own arc.
· media-writer.sh is un-versioned (rmbp B97).
· winmenu APP_OWNER unregistered WinId holder (pi 10's question) — prove or refute.

## RULEBOOK DEFECTS FOUND TODAY, UNFIXED ON PURPOSE
✓ LAWS.md cites R20 twice (:140, :373 — the never-trash-work law) and R20 exists ONLY on
  hw-rmbp. Not fixed: not editing the rulebook while the three seats decide how it lands.
✓ The consolidation dropped the `STRUCTURAL_GATES.md` pointer (355 lines, hw-rmbp only).
  Cite it; do not copy it — a second copy is how we get two divergent ones.

## OWED TO PEERS
✓ rmbp 18: at the first joining fold, ledger-check.sh + arch-families.ledger take hw-rmbp's
  version WHOLE, gated by symbol count 5/2/2, never by a clean apply. arroyo takes theirs plus
  one token: `sed -i 's/|esp-jetson|/|esp-jetson|esp-jetson-img|/'`. Also owed: the x86
  `wc_close_click` hunk for ack.
✓ pi 10 / rmbp 18: the S34 → SO25 rename belongs in the landing commit.

## LANDING — the arc's actual end, not started
· Land exec-orin22-bootroot + the fold on hw-jetson `--no-ff`: adversarial panel, announce tip
  and `origin/main..<tip>` to rmbp 18, their ack, trunk battery, shape check.
✓ Nothing on this track is pushed. `git ls-remote` fails from this seat (publickey).

## STANDING — read this way, never `cat`
    git log --all --not HEAD -- <file>                     # what I lack (enumeration, not pick)
    git ls-tree -r --name-only <ref> -- docs/ | comm -23 - <mine>   # files I don't know exist
