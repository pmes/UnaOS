# ORIN — WHAT IS IN FLIGHT AND WHAT IS OWED

One file, fixed path, not per-round. Kept current AS WORK HAPPENS, never "at close":
a list written only at close is never written at all by a session that does not reach one,
and a session ends when the work is done or when it is not worth continuing.
`✓` = verified in this tree.  `·` = inherited, NOT re-checked.  Re-derive every sha before acting.
Last touched 2026-09-12 by orin 27: rung 4a+4b FLOWN in one boot, PASS/PASS (A51); tear and unafsroot still uncommitted in their orin26 worktrees.

## STATE — re-derived 2026-09-12T01:0xZ by orin 27 (local), not relayed
✓ origin/main 084b79ac · origin/hw-jetson 277e2cab · origin/hw-rmbp 73d9d361 · origin/hw-pi4 153c78dd
✓ LOCAL, UNPUSHED: main 50d35cbf (R45-R48 + LAWS + docs/dev/QUEUE.md, the trunk queue) · hw-jetson 059db935
  (f2794266 rung 4 flown + the unafsroot fold) · hw-pi4 3c102d2a (pi-queue.md) · hw-rmbp dc1cc3a2 (rmbp-queue.md) ·
  exec-orin26-unafsroot 5984c69c (merged) · exec-orin27-ga10b4c 6ffb5211 DONE and FOLDED (hw-jetson 9567fdbf): rung 4c + the deferred Shut Down arm (=4), A55; knoboff EXIT 0; scorer 4c go-red proven · exec-orin27-closefold (cut at dff46786, Opus executor live: the x86 WC red — closemin fixture vs VUGTAB, SO26) · exec-orin26-tear 3e9d5625 DONE and FOLDED (hw-jetson 7ff0aa9d): BEAM — gates check/knoboff 0/esp-jetson/test-arm green; A54 · exec-orin27-cmd8 28c99b9b DONE and FOLDED (hw-jetson e813d0d2): card-detect gate before CMD0 (bit18 is the signal; bit16 lies), csd-v2/byte-addressing refusal, A57 truncation row (four sites, 4 GiB ceiling); knoboff EXIT 0. Caveat: bit18=1 with a seated card unproven — render13 prints all three bits · exec-orin27-usblun (cut at 0a22ae40, Opus executor live: the USB mass-storage path addresses LUN 0 only, so a multi-slot reader hides every slot but one — Get Max LUN census, publish the slot that holds a card).
  Push line: `git push origin main hw-jetson hw-pi4 hw-rmbp exec-orin26-unafsroot exec-orin26-tear exec-orin27-ga10b4c exec-orin27-cmd8 exec-orin27-usblun exec-orin27-closefold exec-orin24-fold exec-orin25-shotverb`
✓ TRUNK FOLDED: hw-jetson dff46786 = 895b5dda + main 8c38f753 (arroyo arm-pi union, .gitignore union, LEDGER keeps main's SR12; SR7 is the S33 re-id and stays). Fold gate on dff46786: check-tegra strict rc=0 · check-plain strict rc=0 · **`UNAOS_WC=1 ./arroyo test` rc=1 — `[closemin] close-scope … shell_arm_control=0 -> FAIL` (serial.log:1022): orin's closemin fixture (S34/SO25) meets pi's VUGTAB raise rewrite (d6ed3024, via trunk) — a fold defect, executor exec-orin27-closefold** · test-arm rc=0 · esp-jetson rc=0. Logs ~/unaos-bench/scratch/orin27/fold-dff46786/.
✓ Fold gate on 059db935 GREEN: `UNAOS_TEGRA=1 UNAOS_LEDGER_STRICT=1 UNAOS_K8REACH_STRICT=1 ./arroyo check` rc=0 · plain strict check rc=0 · `./arroyo esp-jetson` rc=0.
✓ A49: three readings in one night — "v1 card" (orin 24) → "driver CMD8 defect on a 2.00 card" (orin 27, from the card's registers) → "EMPTY native slot, card was in the USB reader" (Peter). The executor rewrites the row to the third. render13: native slot EMPTY must print the SKIP line; UNAOS-DATA card in the reader's second slot must appear in the USBLUN census.
✓ Landing order (R45): rmbp LANDED (main 267bd49d) → pi LANDED (main e10d9e4e; hw-pi4 and hw-rmbp level with main) → jetson (review panel running at f0dc8058; main folds into hw-jetson first — merge-tree shows conflicts in .gitignore, LEDGER.md, arroyo).
✓ R46: one focus (orin), ALL LANES, no owner, no support seat, no lane grants. R47: Opus executors, Fable for key tasks only. R48: adversarial handoff.

## RENDER13 — the next boot (line settled 2026-09-12)
⚠ BENCH CHANGE (Peter 2026-09-12 ~03:2xZ): the Orin BOOTS FROM ITS NATIVE microSD SLOT; the boot card is the old 32 GB UNAOS-PI card (SanDisk SS32G 12/2014) re-imaged with UnaOS-orin-card.img (whole-card, Peter's go); the data card rides the multi-slot USB reader, which is then the ONLY USB storage (no two-reader coin flip). Consequences: `/`, `/boot`, `/apps` come from tegra-sd (READ-ONLY today — the wire says rw=); the empty-slot gate must not gate out a seated card (SDCMD8b: probe CMD8 once before SKIP; bit18 unproven with a card); the reader census sees the data card at whatever LUN its slot is.
✓ Knob line (render12's fifteen, minus PROBE3 per arroyo's rung-4 rule, plus the deferred rung 4 and BEAM):
  `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 UNAOS_TCURX=1 UNAOS_BSPTICK=1 UNAOS_BSPRUN=1 UNAOS_NET4=1 UNAOS_NET5=1 UNAOS_GA10B_PROBE4=4 UNAOS_BEAM=1 ./arroyo esp-jetson` then `./arroyo esp-jetson-img` for the whole-card image; write with `media-writer.sh --image … --write --erase-target` (Peter's go 2026-09-12).
✓ Glass list: EL0 on all cores (core0), apps launch (el0mem), no tearing + `BEAM VERDICT=` (A54), `/` = native unafs volume (A53, needs the re-imaged card), wintitle/closemin/dockid/cursorbg/facet/netlease checks from the render12 fold, native slot EMPTY → `M2: no card in the native slot -> SKIP` (A49), UNAOS-DATA card in the reader's second slot → USBLUN census (A56); then Shut Down → 4a/4b/4c → board off (A55).
⚠ ADDENDUM REVIEW (orin 27, second panel) OBJECTED on USBLUN: F1 ep0_resync lacks the Running arm (blind Set TR Dequeue); F3 a LUN answering a transport error made the ladder surrender the whole device — LUN 0's disk lost (its own go-red wire); F2 every stick on every board now gets +1 control +2 bulk transactions with QEMU-only evidence → USBLUN-M3 in flight (fix arms, non-fatal census, `UNAOS_NOUSBLUN=1` opt-out). Non-blocking, recorded: F4 → A54 status cell; F5 the x86 `test` leg run by this seat under R46 is named as such in the landing; F7 bit18-with-card unproven (A49); F8 the `arm-tegra-sdmmcroot` matrix leg leaves trunk with the landing (deleted with the knob at b0536d83, 55 legs vs 47); F9 power.rs names ga10b_probe under a cfg that cannot arm off tegra — borderline R16, not over. render13 is NOT flown on the staged render13-20260912T0221Z-2906747: it carries M2 without M3 and is superseded once M3 folds.
✓ ALL FOUR EXECUTORS FOLDED: tear 7ff0aa9d · 4c 9567fdbf · cmd8 e813d0d2 · closefold 4cd676c2 (SO26) · usblun c055cfdc (A56). FOLD GATE on 29067473 GREEN: check-tegra strict rc=0 · check-plain strict rc=0 · `UNAOS_WC=1 ./arroyo test` rc=0 (closemin PASS with lid_buried=1 shell_arm_unbury=1) · test-arm rc=0 · render13 esp-jetson rc=0 (banner carries beam, ga10bprobe4a/b/c/d, apsrun, sdmmc; artifact: BEAM VERDICT= x3, [ga10bprobe4c] x24, [ga10bprobe4d] x7, HOMESOIL x7, USBLUN x11, "no card in the native slot" x1, [ga10bprobe3] 0) · esp-jetson-img rc=0 (640 MiB, CARDREADY-GEOM 1:0c:2048:260096,2:7f:262144:1048576). Logs ~/unaos-bench/scratch/orin27/fold-render13/. Staged as render13-<UTC>-2906747 with the card image beside it. STOP carried from cmd8: `bootdisk.rs tegra_sd_state` cannot say "absent (no card)" without a flag from sdmmc_tegra.rs — the SKIP line is the witness for now.

## RENDER13 WAS HELD — Peter 2026-09-11 (history)
✓ Specified as fold + core0 + el0slot + mem + tear + unafsroot. Measured: core0/el0slot/mem are at
  ZERO commits, tear is diagnosis-only, unafsroot incomplete. It would have fixed NONE of the four
  glass defects. Held rather than fly a number implying progress it does not have.
✓ orin 27 2026-09-12: RUNG 4 FLOWN, ONE BOOT (Peter 2026-09-11: combine 4a and 4b). Image ga10b4ab-20260912T0019Z-277e2ca at hw-jetson 277e2cab, KELF max=0x238368. 4a `bcrheld=7/7 -> BCR-ALLHELD`; 4b `br_retcode=0x00000002 -> BROM-VERDICT-FAIL` (the PASS), SYSTEM_OFF. Scorer PASS 4a exit 0, PASS 4b exit 0. Evidence docs/dev/evidence/orin27/ga10b4ab-boot1.log; A51 flown. Boot 1 (2026-09-11) REFUSED no-dma-window, fixed by 1629d449. Next rung is a clean-room question (what the ROM wants at fmccode/fmcdata/pkcparam), brief §7.
· (history) orin 26: Peter 2026-09-11 "do the 4 boots". Rung 4a/4b BUILT (exec-orin26-ga10b4 a39fd32d, A51), two images STAGED
  (`ga10b4a-20260911T1809Z-a39fd32` max_vaddr 0x23cae8 → `ga10b4b-20260911T1809Z-a39fd32` 0x23f120), card line dry-run
  refuses only on device readability (needs sudo) and geometry (`--expect-geom 1:0b:2048:63401984`, from render12's FLIGHTID).
  Butler on ttyACM0, MARK ga10b4a written, waker armed. Boot 1 = 4a; boot 2 = 4b only on `-> BCR-ALLHELD`.
✓ tear: DONE (BEAM, A54, folded 7ff0aa9d) · unafsroot: DONE (A53, folded 059db935) · core0/el0mem: folded 277e2cab. Trap for the next image: `./arroyo test-arm` REBUILDS target/aarch64_esp/kernel.elf — build esp-jetson LAST and grep the artifact before any QEMU leg.
· (history) Four defect executors spawned 2026-09-11 off the fold 63654d93 in ~/unaos-bench/scratch/orin26/{core0,el0mem,tear,unafsroot}
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
