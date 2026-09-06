# TRUNK-PREVIEW3 — orin 14 arc battery preview (executor TRUNKPREVIEW3)

Date: 2026-09-05. Worktree: /home/pmes/src/github.com/pmes/UnaOS-hw-pi4/.claude/worktrees/agent-a27ebcf2dadf34394
(private branch `worktree-agent-a27ebcf2dadf34394`; nothing pushed, nothing touched on main or hw-jetson).

## Merge facts

| fact | value |
|---|---|
| TIP (hw-jetson, local == origin/hw-jetson) | `518dca3e docs/evidence: LANDING-REPORT-2 — PRTSCR2 f0db58bf folded; hw-jetson push owed` |
| target (origin main) | `d11cd56e Merge hw-jetson: orin 13 second batch — the Orin cascades to a real desktop, five APs come up after APTEXT, serial RX and Print Screen reach the wire` |
| merge-base d11cd56e TIP | `6cc8de8c` |
| LAWS shape check `git log --no-merges --oneline 6cc8de8c..d11cd56e \| wc -l` | **0** (main has no non-merge commits since the base; TIP is 22 commits ahead) |
| preview merge commit | `0e159688` — `git merge --no-ff 518dca3e -F merge-msg.txt` (P2: -F file, never stdin) |
| `git log --pretty=%p -1 HEAD` | `d11cd56e 518dca3e` (two parents) |
| conflicts | **none** — clean merge, no union/STOP needed |
| `git diff --stat d11cd56e..HEAD \| tail -1` | ` 20 files changed, 2400 insertions(+), 51 deletions(-)` |
| `git diff TIP HEAD --stat` | **empty** (0 lines) — merge tree == TIP tree |

Files touched by the merge (20): 3 code — `unaos/crates/kernel/src/arch/aarch64/serial.rs` (+50/-),
`unaos/crates/kernel/src/main.rs` (+92/-), `unaos/crates/kernel/src/video/prtscr.rs` (+92/-); the rest
are docs: 5 existing docs edited (`docs/dev/LAWS.md`, `docs/dev/LEDGER.md`, `docs/dev/OS/02_KERNEL_CORE/scheduler.md`, `docs/dev/OS/08_VIDEO/screenshot.md`, `docs/dev/OS/orin-ledger.md`) + 12 new `docs/dev/evidence/orin14/*` files.

## Battery (sequential, this worktree, logs in ~/unaos-bench/scratch/orin14/trunkpreview3/)

| leg | exit | evidence |
|---|---|---|
| (a) `./arroyo check` | 0 | 66 ✅ / 0 ❌ (a-check.log, 13537 lines). Gate lines verbatim: `GATE-KNOB: OK — 154 features declared, 153 named by a cfg, 0 phantom, 0 dead, 0 trailing-comment cfg` · `GATE-LEDGER: OK — 71 rows in 2 ledger file(s) + RULINGS: ids unique, status ∈ enum, owners known, cross-refs resolve, shas exist, evidence in git and anchored, rulings live or superseded-by a real R<n>` · `✅ knob→leg coverage OK (every aarch64-qualified feature is named by a leg; known holes allowlisted with owners)` · `✅ knob→builder wiring OK (every x86-leg-named knob is read by the media builder)` |
| (b) `UNAOS_WC=1 ./arroyo test 150` | 0 | banner `⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc` — contains `wc`; serial.log 1477 lines (b-serial.log); `PASS ::` = 52; panic = 0; FAIL = 0; closing line `✅ Test run complete.` (b-test.log) |
| (c) `./arroyo test-arm 60` | 0 | serial-arm.log 477 lines (c-serial-arm.log); positive witnesses: `AARCH64: timer heartbeat live (first tick).` (heartbeat) + 2 PASS — `:: BOT-PARK: selftest ... -> PASS ::` and `:: SERWIT-2: mirror taps — every line accounted for on all 4 taps, 0 lost on the 3 evidence taps (ftdi/tste/flightrec) -> PASS ::`; panic = 0; closing line `✅ aarch64 test complete.` (c-test-arm.log) |
| (d) `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` | 0 | gate line verbatim: `✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 23940 lines scanned` — denominator is the tool's (scripts/specs/pi4-regression.spec: 117 REQUIRE + 2 COUNT = 119); banner `⚡ kernel features: baremetal,skip_xhci,witness`; serial-pi.log 23940 lines (d-serial-pi.log); panic = 0; geometry reached the bench blit path: `:: MAILBOX: framebuffer 1920x1200 pitch=7680B stride=1920px base=0x3c100000 size=9216000 ::`. **Green under load, single run** (N == M at 210 s, no 420 s re-run needed). |

## Verdict

All four legs green on the preview merge `0e159688` (= main d11cd56e + hw-jetson TIP 518dca3e, clean `--no-ff`,
no conflicts, merge tree identical to TIP). Worktree clean at the merge commit; nothing pushed; main and
hw-jetson untouched. The merge commit exists only on the private branch `worktree-agent-a27ebcf2dadf34394`
— it is a PREVIEW, not the landing.
