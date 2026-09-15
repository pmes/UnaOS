# GRAVEYARD SWEEP — every open A/B row in `docs/dev/OS/rmbp-ledger.md`, decided by evidence

rmbp 19's verdict, quoted by Peter: **"88 open rows is a graveyard, not a queue."** At the last count it
was 93. This file is the per-row record the sweep's DONE gate asks for: id, verdict, the command that
proved it, and what the command returned. No row was deleted, renumbered, or had its ITEM text edited.

Run 2026-09-15 against `hw-rmbp` **27716175** (`docs/rmbp-queue: STATE — the three-way fold at 160176d2`),
on branch `exec-rmbp-graveyard`. Every command below was run in THAT tree; paths are relative to the
repo root and kernel paths are under `unaos/crates/kernel/src/`.

## The fact that decided most of this file, and it was not what the brief assumed

The brief framed this tree as carrying "hw-jetson's 139 unlanded-to-main commits", so that a close would
have to be spelled `in-tree (hw-rmbp fold 160176d2)` for anything not on `main`. **Measured instead of
inherited, and it is the other way round:**

    git rev-list --count HEAD..main   ->  0
    git rev-list --count main..HEAD   ->  146

`main` (`bd17f887`) is an ANCESTOR of this tree. Trunk absorbed the jetson work; the 146 commits ahead
are the three sync merges plus orin's post-landing rounds. So for every sha this sweep tested,
`is-ancestor <sha> HEAD` and `is-ancestor <sha> main` returned the SAME answer — **59 of the 106 cited
shas resolve on both, 47 on neither**. No row needed the weaker `in-tree` spelling, and the 47 are the
whole story of the next section.

## Why 24 closed rows are spelled `dropped` and not `landed`, and the gate is the reason

GATE-LEDGER's FETCHED check requires that a row whose status is `fixed-unflown`, `flown` or `landed`
cite only shas reachable from a track head. Twenty-four of the closed rows are GRANT and REVIEW records
whose ITEM text names an **executor-branch sha that never landed under its own identity** — the content
reached `main` under a re-cut sha instead. The first run of this sweep spelled them `landed` and the
gate refused, naming 36 findings across 24 rows:

    bash unaos/scripts/ledger-check.sh   ->  rc=1
    docs/dev/OS/rmbp-ledger.md:48: B18 is `landed` but sha 8e864463 is not an ancestor of any track head

**That refusal is correct and it is B59's own finding arriving on the sweep that was closing B59.** B59
records the split verbatim: fold 8 landed SHELLRELICS' and VFSROUTE's CONTENT under re-cut shas, "so the
original commits stayed unreachable while the work became reachable — a sha-tracking close would have
reported this row still open forever; a content close reports it retired." The status enum offers only
`landed` and `dropped` for a closed row, `landed` asserts a citation this lane cannot honour, so the
honest spelling is `dropped` with **CLOSED BY LANDING** and the unreachable citations named in the
status text. Each such row says so in its own words. This is the fifth time GATE-LEDGER has corrected
this lane on a status cell (B35, B40, B46, B50, now this), and the fourth time it was taken rather than
argued with.

B43 is the sharpest instance: its unreachable token `4cd79076` is **unbackticked, inside a quoted
`git diff --stat` invocation** — a MENTION, not a reference. B88 already queued exactly that gap: the
gate has a mention-versus-reference contract for cross-refs (`→` reference, `->` mention) and a
`label:hash` escape for artifact digests, and NOTHING for a sha mention, so a row physically cannot
narrate a command it ran without the gate reading the command's argument as a citation.

## Population, counted before any claim (LAWS §5)

    awk -F'|' 'NR>=17 && NR<142 && /^\| [AB][0-9]+ \|/' docs/dev/OS/rmbp-ledger.md | wc -l   ->  115

115 A/B rows in sections A and B (B24's registered two-rows-on-one-line counts once). Of those, **93
carried a status beginning `open`** and are the population this sweep decided — the 20 `fixed-unflown`,
1 `landed` and 1 `flown` were not touched. The counting command and this table are now recorded in
`docs/dev/OS/rmbp-queue.md`'s STATE block, which carried none.

| status | before | after |
|---|---|---|
| open | 93 | 46 |
| landed | 1 | 18 |
| dropped | 0 | 30 |
| fixed-unflown | 20 | 20 |
| flown | 1 | 1 |

Section A (A1–A9) is untouched by instruction: they are metal defects and already queue rows. Of the
**84 B-rows decided: 17 `landed`, 24 `dropped (closed by landing)`, 6 `dropped` outright, 37 still
open** — and every one of the 37 now has a row in `docs/dev/QUEUE.md` or `docs/dev/OS/rmbp-queue.md`,
except B77, which is raised as a STOP question below.

## Per-row table

| id | verdict | proof command (run in this tree) | result |
|---|---|---|---|
| A1 | open | `(section A: metal defect, status untouched per the brief)` | already a queue row — rmbp-queue METAL |
| A2 | open | `(section A: promoted to SR2, which is the one home)` | already routed — rmbp-queue SHARED list, LEDGER SR2 |
| A3 | open | `(section A: metal defect, status untouched)` | already a queue row — rmbp-queue METAL (A3/A4) |
| A4 | open | `(section A: not kernel work, Peter's call on his laptop)` | already a queue row — rmbp-queue METAL (A3/A4) |
| A5 | open | `(section A: metal defect, status untouched)` | already a queue row — rmbp-queue METAL; B74's fix arm feeds it |
| A6 | open | `(section A: metal defect, status untouched)` | already a queue row — rmbp-queue METAL |
| A7 | open | `(section A: metal defect, status untouched)` | already a queue row — rmbp-queue METAL |
| A8 | open | `(section A: metal defect, status untouched)` | already a queue row — rmbp-queue METAL |
| A9 | open | `(section A: metal defect, status untouched)` | already a queue row — rmbp-queue METAL, and the x86 half of S29 in QUEUE 2 |
| B1 | open | `grep -n click_pointer_pos arch/x86_64/syscall.rs` | :6360/:6716 present — section A/B metal-and-x86 row, already in rmbp-queue X86-ONLY |
| B3 | open | `(not re-measured; rmbp-queue X86-ONLY row)` | 5 features uncovered by any board leg |
| B4 | open | `(not re-measured; rmbp-queue X86-ONLY row)` | 2 cosmetic unsafe warnings |
| B6 | open | `grep -rc splash video/fbcon.rs main.rs` | 4 / 5 hits — call sites not yet x86-gated; JOB added to QUEUE 4 |
| B7 | open | `QUEUE 6 NEEDS PETER already carries it` | blocked on Peter |
| B8 | open | `rmbp-queue METAL already carries it` | no Broadcom driver |
| B9 | open | `rmbp-queue X86-ONLY row ADDED this sweep` | 2 reds in 5 WC runs, load-correlated |
| B10 | open | `rmbp-queue METAL already carries it` | shut-out register, not started |
| B11 | open | `awk 'orin_render_service body' main.rs  or  grep -c drag_route_tail` | 0 — still structurally unfed; QUEUE 4 S7 is its home |
| B18 | dropped (closed by landing) | `grep -rc mark_online unaos/crates/kernel/src/arch/aarch64/sched.rs ; git merge-base --is-ancestor ea182855 main` | 24 hits; rc=0 (a28879de too) |
| B19 | dropped (closed by landing) | `grep -rc latch_cleared_before_close unaos/crates/kernel/src/video` | 2 hits (see B38) |
| B20 | dropped (closed by landing) | `grep -c 'Phase::Encode\bPhase::Write' (run as two greps) video/prtscr.rs ; is-ancestor d7eec583 main` | 9 hits; rc=0 |
| B23 | dropped (closed by landing) | `grep -rc winid_register_holder / SEAM_WIN unaos/crates/kernel/src` | 16 / 20 hits |
| B26 | open | `QUEUE 5 SO7/B26 already carries it` | quiet-box re-run still owed |
| B27 | dropped (closed by landing) | `grep -c 'quarry::key_route' arch/x86_64/syscall.rs main.rs` | 2 / 4 hits |
| B30 | landed | `grep -c gap_left video/crystal.rs ; grep -rc gap_right unaos/crates/kernel/src` | 2 hits present / 0 hits absent (the control) |
| B33 | landed | `grep -c UNAOS_SDMMCROOT unaos/scripts/k8-reach.registry (and TCURX, NET5)` | 3 hits |
| B35 | dropped (closed by landing) | `same as B30` | gap_right 0 tree-wide |
| B36 | landed | `grep -c for each verb in unaos/libs/sys/midden_core/src/lib.rs` | grep 3, wc 2, df 2, env 2, history 2, sleep 2, which 2, ps 2 |
| B38 | dropped (closed by landing) | `grep -rc latch_cleared_before_close unaos/crates/kernel/src/video` | 2 hits |
| B39 | landed | `grep -n 'WINID_HOLDER_MAX *[:=]' video/wm.rs ; grep -n 'twelve slots' video/wcg.rs` | wm.rs:26317 = 12 ; wcg.rs:4388 'twelve slots (eight until SO10 raised it)' |
| B42 | dropped (closed by landing) | `grep -rc PRTSCLOST unaos/crates/kernel/src` | xhci/mod.rs 11, main.rs 4, display_tegra.rs 7 |
| B43 | dropped (closed by landing) | `git rev-list --count HEAD..main` | 0 (main is an ancestor; one lineage, nothing to reconcile) |
| B44 | open | `grep -rc KBD_INFLIGHT src` | 0 — not started; QUEUE 2 A41/B44 is its home |
| B45 | open | `rmbp-queue X86-ONLY already carries it` | the x86 gate still wires HID to EHCI |
| B46 | dropped (closed by landing) | `see B27 / B36` | QUARRYDOOR and the focus gate are in-tree |
| B47 | open | `grep -rl HOST_VERBS unaos/scripts/` | NOTHING — the class is ungated; instance landed (umv/urmattr 1 hit each in midden_core); JOB added to QUEUE 5 |
| B48 | dropped (closed by landing) | `grep -rc pin_console unaos/crates/kernel/src` | 18 hits |
| B49 | dropped (closed by landing) | `grep -c compile_error arch/aarch64/mod.rs ; grep -rc virt_el0 src` | 1 / 59 hits |
| B50 | dropped (closed by landing) | `grep -n 'ten-argument' video/wm.rs` | wm.rs:6366, naming wcg::end's signature as the source of the count |
| B51 | open | `grep -rc kbd_guard_verdict src` | 0 — fold blocked on the rMBP metal flight; JOB added to rmbp-queue METAL |
| B53 | dropped (closed by landing) | `grep -rc deskcascade unaos/crates/kernel/src` | 7 files non-zero (fbcon.rs 4, main.rs 61, desktop_firmware.rs 2, ...) |
| B54 | dropped (closed by landing) | `grep -c 'win_store or glyph or CONSOLETEXT' video/fbcon.rs (run separately)` | 35 hits on the retain path |
| B55 | open | `grep -c 'any(baremetal, tegra_el0)' unaos/arroyo` | 7 — sweep not written; JOB added to QUEUE 4 |
| B56 | open | `grep -rc kbd_retire src` | 0 — same flight blocker as B51 |
| B58 | dropped (closed by landing) | `git merge-base --is-ancestor 18af05ab main ; ... c8153b4e main ; ... f3e64daf main` | rc=0 for all three |
| B59 | dropped (closed by landing) | `git branch -a --contains 28899d5c / 0019ec7a / e390721f` | all three name remotes/origin/exec-orin17-dupguard |
| B60 | dropped (closed by landing) | `git merge-base --is-ancestor e668ebde main ; grep -rc sdmmc_root_bind src` | rc=0 ; 0 hits (C1 dissolved) |
| B61 | dropped | `row's own status: no gate proposed` | a lesson record; the four axes live in LAWS and the shared memory claims-relayed-past-their-check |
| B62 | landed | `grep -rn 'VUG.ELF' unaos/scripts/specs/*.spec` | pi4-barename.spec:52 names /apps/VUG.ELF |
| B64 | open | `grep -c UNAOS_DMAWIN unaos/arroyo` | 0 — the specimen knob is armed by no command; JOB added to QUEUE 5 |
| B65 | dropped | `grep -rc 'feature = "sdmmcroot"' src ; grep -rc sdmmc_root_bind src` | 0 / 0 — the feature the grant was about no longer exists |
| B66 | dropped (closed by landing) | `grep -rc cross-mount-root unaos/crates/kernel/src` | fs/vfs.rs 1 ; mount_root 16 tree-wide |
| B67 | dropped (closed by landing) | `git merge-base --is-ancestor a62188c9 main ; grep -c inode-gone fs/vfs.rs` | rc=0 ; 3 hits |
| B68 | landed | `same as B67` | rc=0 ; 3 hits |
| B69 | landed | `git merge-base --is-ancestor 1d0d70e5 main ; grep -c layout_mv_witness shell.rs` | rc=0 (XVOL) ; 11 hits |
| B70 | open | `grep -n 'REFUSALS or InFlight' video/prtscr.rs (run separately)` | ONE fetch_add at :395 ; direct .report() at :333 and :359 — PROVEN STILL LIVE; JOB added to QUEUE 1 |
| B71 | landed | `git merge-base --is-ancestor cc283929 main ; grep -n 'u1b-' unaos/scripts/specs/x86-fat.spec` | rc=0 ; only :233 and :241, both prose — no live look-around |
| B72 | landed | `grep -c 'all were dead' arch/aarch64/sdmmc_tegra.rs` | 0 hits |
| B73 | dropped (closed by landing) | `grep -c menu_owner video/menubar.rs` | 10 hits |
| B74 | open | `grep -n 'strip::vacate' video/dock.rs video/menubar.rs` | dock.rs:853 and menubar.rs:1020 discard the return — fix arm unstarted; JOB added to QUEUE 1 |
| B76 | dropped | `the retrofit it named IS this sweep` | its live instance is B86, which stays open |
| B77 | open | `grep -n K4-write fs/unafs.rs` | :1487/:1621/:1631 — posture writable in-tree; the attended-metal condition is orin's. NO QUEUE HOLDS IT -> STOP question |
| B78 | landed | `git merge-base --is-ancestor b1885dbc main ; grep -rc boot_volume_serial src` | rc=0 ; 23 hits, no arch gate |
| B79 | dropped (closed by landing) | `three separate greps in shell.rs (re-points BOTH / re-points ALL THREE / sdmmc_root_bind)` | 0 hits each |
| B80 | landed | `same as B78 ; grep -rc 'feature = "sdmmcroot"' src` | rc=0 ; 0 hits |
| B81 | landed | `git merge-base --is-ancestor 51f06bea main ; grep -n 'fits=' fs/unafs.rs` | rc=0 ; :477 'WHY THE OLD fits= COULD NOT READ NO', wire emits sb_blocks= beside fits= |
| B82 | dropped (closed by landing) | `sed -n '8,22p' docs/dev/evidence/orin22/stage11/KNOBS-render11.env` | the env file states sdmmc is default-on since b1885dbc and that there must be no #require=sdmmc line |
| B83 | open | `git branch -a --contains dc683c40 / 1aae3459` | local exec-* only; content on main as 18af05ab/c8153b4e — citation half open; JOB added to rmbp-queue |
| B84 | dropped | `grep -n 'cfg(target_arch' unaos/crates/kernel/src/fs/mod.rs` | the gate at :35 on mod unafs is present; partitions.md:219 is correct as written |
| B85 | open | `grep -c FORBID unaos/scripts/specs/x86-wc.spec x86-witness.spec` | 13 / 90 — two of the six still unverified, and B95 makes the whole file inert |
| B86 | open | `case-insensitive fable search over unaos/scripts, unaos/arroyo, .claude` | nothing — no gate, no hook, no assert; JOB added to QUEUE 6 |
| B87 | dropped (closed by landing) | `is-ancestor b0536d83 / b1885dbc / b12decbb main ; grep -rc sdmmc_root_bind ; grep -rc locate_boot_volume` | rc=0 x3 ; 0 hits ; 7 hits |
| B88 | landed | `git cat-file -e main:unaos/scripts/<f> for the seven ; git diff --numstat main HEAD -- unaos/scripts/ledger-check.sh` | seven rc=0 ; numstat EMPTY (the 190-line gap is zero) |
| B89 | open | `sed -n '50,56p' drivers/pci.rs ; grep -n 'enum BlockSource' -A 12 fs/fat.rs` | the single-class xHCI attach at :53 stands; the source enum is still compile-time |
| B90 | open | `(B109's witness landed, so the control is now readable)` | positive control owed: friend present AND root refusing writes; JOB added to rmbp-queue METAL |
| B91 | open | `rmbp-queue METAL already carries it (B89/B91)` | ordering constraint: stranger guard before AHCI |
| B92 | open | `grep -c UNAOS_CHECK_P unaos/arroyo` | 0 — MATRIXPAR never reached this tree; JOB added to QUEUE 5 |
| B93 | landed | `grep -n semicolon unaos/scripts/append-position.sh` | :23 carries the discriminator verbatim |
| B94 | open | `ls unaos/scripts/` | no line-neutrality script; B93's twin is built, this half is not; JOB added to QUEUE 5 |
| B95 | open | `grep -n -- '--spec' unaos/arroyo` | three invocations, all Pi (:2958 qemu_await, :8102 pi4-regression, :8125 $k8_spec) — QUEUE 5 already carries it |
| B96 | open | `sed -n '29,33p' unaos/scripts/check-roots.sh` | HERE from BASH_SOURCE, usage takes no argument; JOB added to QUEUE 5 |
| B97 | open | `head -5 unaos/scripts/card-watch.sh` | still the 47-line macOS stub; QUEUE 5 already carries it |
| B98 | landed | `grep -n 'aliased= or fn admit' fs/bootdisk.rs (run separately)` | aliased=ambiguous at :183/:438/:798, same_device and merged_when_same_unit at :741 |
| B99 | open | `grep -c UNAOS_CHECK_P unaos/arroyo` | 0 — nothing landed; the ref retirement is Peter's; JOB added to QUEUE 6 |
| B100 | dropped | `grep -n 'supersedes rmbp 16' docs/dev/RULINGS.md` | R30 supersedes the position verbatim; the job is LEDGER SP17 + QUEUE 5's media-writer rows |
| B101 | open | `grep -c pi4-regression.spec unaos/crates/kernel/src/ui_status.rs` | 0 — contract comment unwritten; JOB added to QUEUE 5 |
| B102 | open | `grep -n 'scratch_ladder or all satisfied' install/pi.rs (run separately)` | :166 unconditional call, :197 no return type, :290 the false banner; JOB added to QUEUE 3 |
| B103 | open | `grep -n 'CAPSTONE COMPLETE' arch/aarch64/sched.rs` | :8333 unconditional; JOB added to QUEUE 5 |
| B105 | dropped | `grep -rn load-card10 docs/` | the two pointers are in R30's ruling annotation and B97's ITEM cell, both outside a status sweep's reach; R33 carries the rename |
| B106 | open | `(orin's scorer, not in this repo; the operand IS here)` | grep -c 'sha=' fs/bootdisk.rs = 3 — QUEUE 5 already carries the STAMP-MATCH leg |
| B108 | open | `grep -rc 'pin_pulse or pin_console or pin_quarry' src (run separately)` | 18 / 18 / 23 — four pins; QUEUE 1 SO21 already carries it |
| B109 | landed | `grep -n 'pub vol: VolId' video/prtscr.rs ; grep -n MAX_USB_DISKS drivers/block.rs` | Shot carries vol ; MAX_USB_DISKS = 4 with USB_DISKS replacing the single slot |
| B110 | landed | `git rev-list --count HEAD..main` | 0 — the sync this row scouted is done |

## The two things the sweep could not do, and why

**1. B24's registered field-count exception STAYS, and the registry line in `ledger-check.sh` is NOT
removed.** The registration says splitting is a CONTENT call resting on a pipe convention that is
Peter's open decision. Measured here rather than argued: the line has **13 fields against the table's
9**, and the four extras are not four halves of a clean second row.

    awk -F'|' 'NR==54{print NF}' docs/dev/OS/rmbp-ledger.md   ->  13

Field by field: F3 is item 1, F4/F5 its owner and flies-on, and **F6+F7 are ONE status cell split by a
literal `|` inside `git archive <sha> … | tar -x -C <tmp>`**; F8/F9/F10 are the second row's
owner/flies-on/status, F11 the evidence, F12 the closed-by. The split is **not unambiguous on three
counts**: (a) the second row's ITEM is the tail of F7 with no delimiter, so its boundary would be
inferred rather than read; (b) there is exactly ONE evidence cell and one closed-by cell for two rows;
(c) a second row needs a second id, and this brief opens NO new ids and renumbers nothing. Any one alone
would be enough. The registration's stated reason is also still true and is still the thing that has to
be settled first.

**2. B77 has no queue that can hold it.** Its remaining obligation is an ATTENDED METAL CAPTURE FROM THE
MEDIUM THE ORIN BOOTS, scored by identity of the loaded image — QEMU green is explicitly not sufficient,
the condition's first form was defective in exactly the way B76 names, and it was corrected. That is a
peer lane's metal obligation. It fits neither `docs/dev/QUEUE.md` (jobs gateable on QEMU or by compile)
nor `docs/dev/OS/rmbp-queue.md` (this board's metal), and the brief's FILES list does not include
`docs/dev/OS/orin-queue.md`. The row stays `open` with the owner named in its status; routing it is a
STOP question.

## One thing this sweep did to itself, recorded because the row it belongs to predicted it

B85 records that its own row injected a stray pipe into the ledger **three times, every one from a
regex, in a row whose subject is regexes** — and that all three were caught only because the debris
landed in a validated column. Writing B79's new status, this sweep made it a fourth: the proof was typed
as a `grep -c` alternation over three tokens, whose two pipes split that row into 11 fields. Caught the
same minute by re-running the field-count assert (`awk -F'|' 'NF>1 && NF!=9'`) rather than by reading,
and repaired by naming the three tokens instead of writing the alternation — which answers the same
question with three greps and cannot break the table. **Anything that runs `grep` for a living will keep
doing this to a pipe-delimited ledger until the convention changes**, which is B63's and B85's standing
argument and Peter's open decision.
