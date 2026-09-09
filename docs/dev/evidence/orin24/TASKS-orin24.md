# orin 24 — everything in flight or owed, one list (2026-09-09 ~17:30Z; filed to docs/dev/evidence/orin24/ at close)

## A. Landing (the arc's actual end, not yet started)
- [ ] A1 fold render13 = 1b50376a + sdv1 (in) + core0 + el0slot + mem + tear + unafsroot (as they land); one gate; stage render13; card line.
- [ ] A2 S34 → SO25 rename in the landing commit (rmbp 18 agreed).
- [ ] A3 land exec-orin22-bootroot + the fold on hw-jetson --no-ff: adversarial panel, announce tip + `origin/main..<tip>` to rmbp 18, their ack, trunk battery, shape check. rmbp's landing rules recorded in LANDING-NOTES.md (ledger-check.sh/arch-families.ledger whole from hw-rmbp; arroyo union with verb-count gate).
- [ ] A4 pushes named: hw-jetson exec-orin23-fold exec-orin24-{b98,sdv1,unafsroot,core0,el0slot,tear,mem,fold}.

## B. Executors live (from disk)
- [x] B1 sdv1 abfa3e55 — done, folded.
- [ ] B2 unafsroot — ec687e18 first commit (card image from esp-jetson); M2 kernel roots on UnaFS pending; may STOP-and-diff on main.rs/drivers/block.rs → rmbp.
- [ ] B3 tear — 7b0a9804 diagnosis; instrument + fix pending. Must account for boot-3 `[wc-d] verify win=2 128x128 scale=4x bad_cache=1632 bad_ram=2176 moved=2208` — the one corruption witness that fired.
- [ ] B4 core0 — building.
- [ ] B5 el0slot — building.
- [ ] B6 mem — editing.

## C. Flight results owed a record
- [ ] C1 file boot-render12-A1/A2/A3 logs + scorer11 output + census under docs/dev/evidence/orin24/ with a FLIGHT-render12-RESULT.md (R32).
- [ ] C2 FRIEND-DIFF positive control: needs the data card mounted (sdv1 flies render13) AND root write-locked; then PrtScr must land on the friend or refuse. Still owed from render11.
- [ ] C3 glass checks not yet evidenced: cursor sweep (no `restore src=` line fired), taskbar order across quarry open/close (no dock press witness), EL0 placement off core 0 (needs core0 + apps launching, blocked by el0slot/mem).

## D. Small owed fixes (each a line, none started)
- [ ] D1 shell.rs `screenshot` verb prints OK with no device (Shot::report_ok) — shell.rs is shared core; ask rmbp.
- [ ] D2 x86 `[wm] close-scope win=0` on the close-box path (rmbp flag) — caller-side, video lane.
- [ ] D3 wintitle out-of-lane leftovers (SO22): `app_name_forget` from `teardown_user_slot` (arch/), two x86 launchers minting unnamed windows, `b"orin"` title on display_tegra test pattern (R16).
- [ ] D4 `read_block_at` `(lba*512) as u32` truncation above 4 GiB on byte-addressed cards (sdv1 flagged, not ledgered).
- [ ] D5 NIC descriptor-17 behaviour OPEN (R19); net5V REFETCH-WRONGSLOT carried.

## E. Owed to Peter / needs his word
- [ ] E1 GA10B rung 4 brief (exec-orin23-ga10b4 e7c8eb24) has three questions for him in its §8; do not fly 4b unattended.
- [ ] E2 manual mode: settings.json deny/ask rules for Agent + writes outside the repo — I offered, he did not say do it. Standing: I ask before launching/creating unless he has said go on the work.
- [ ] E3 R24/R25 double-booking — Peter's record to rule; both positions in the resume.

## F. Tooling/audit owed (from the resume, unchanged)
- [ ] F1 scorer11 STAMP-MATCH leg (wire sha == card elf stamp, `grep -a -o -F`, WRONG-IMAGE exit 1).
- [ ] F2 orin-specscore.py / mbench --self-test run by no verb (ARCH-CONFORMANCE 14): a DISCOVERY-shaped leg.
- [ ] F3 28 board-named shared scripts (orin-/pi-/rmbp-/jetson-) — R4 scour, its own arc after landing.
- [ ] F4 media-writer.sh un-versioned (rmbp B97).
- [ ] F5 winmenu APP_OWNER unregistered WinId holder (pi 10 question) — prove or refute; ledger row open.

## G. Peers
- rmbp 18: acked closemin syscall.rs hunk; wants landing tip + scope; arroyo grant (UNAFSGROW shape) recorded; ledger-check.sh resolution rule recorded; wants main.rs/block.rs diffs before merge.
- pi 10: support, nothing owed either way.
