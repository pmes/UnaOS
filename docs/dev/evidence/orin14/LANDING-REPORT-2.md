# orin 14 — LANDING REPORT 2 (the orin 13 second batch, 2026-09-05)

## What landed
- **Trunk**: `main` = `d11cd56e` (`--no-ff`, parents `be3b027e` + `6cc8de8c`), 31 commits from
  `hw-jetson` (orin 13 second batch: LOADSAMPLER, PRTSCR-ORIN/PRTSCRLIVE, ORINRX, CASCADE/CASCADEFIX,
  APTEXT, GATE-LEDGER adoption, REVIEW3, the ledgers and rulings), merge-base `077a8fa1`. Merged in
  `../UnaOS` at 2026-09-05T19:21Z after a fresh `flatpak-spawn --host git ls-remote` at 19:21:28Z
  (main `be3b027e`, hw-jetson `6cc8de8c`, unmoved). **Local, unpushed at write time** — Peter's line:
  `git push origin main`.
- **Peer acks**: rmbp 11 (`6cc8de8c`, their ls-remote 19:10Z, unconditional; they are not landing
  first and reconcile after this report); pi 6 (`6cc8de8c`, their own ls-remote, conditional on the
  trunk-battery exit lines below; scope stated: predicates, pi-lane exposure and knob-off identity —
  not the review's quality nor the A16/M2 "undetermined" call).
- **Panel**: `~/unaos-bench/scratch/orin13/review3/BATCH2-REVIEW.md` — no blocker; M1 (APTEXT range
  wrap, `enable_mmu_virt` inline) and M3 (`knob-hygiene.sh` unwired) fixed in `b7679a4e`; M2 folded as
  orin-ledger A16 "open — mechanism undetermined".
- **Flight**: render3b on the Orin (kernel `fef6a184`) — `docs/dev/evidence/orin13/FLIGHT-RESULT-render3b.md`.

## Landing-merge shape check (LAWS §Code and history, pi 6's discriminator)
| fact | command | result |
|---|---|---|
| two parents | `git log --pretty=%p -1 d11cd56e` | `be3b027e 6cc8de8c` |
| merged tree == arc tip | `git diff 6cc8de8c d11cd56e \| wc -l` | `0` |
| safe because trunk added nothing since the base | `git log --no-merges --oneline 077a8fa1..be3b027e \| wc -l` | `0` (trunk's only commit since the base was the prior landing of this branch) |
| pi-lane exposure (pi 6, `origin/main..6cc8de8c`) | `git diff --stat` | serial.rs 126+/2- (124 appended at the tail), sched.rs 50, main.rs 257, arroyo 99, Cargo.toml 50; `pi4-regression.spec`, `video/fbcon.rs`, `syscall.rs` untouched. CAPREVOKE `06858185` is NOT in this delta (already on `be3b027e`) |

## Preview battery (merge `33a9afdd` on `be3b027e`, tree identical to `6cc8de8c`; executor TRUNKPREVIEW; logs `~/unaos-bench/scratch/orin14/trunkpreview/`)
| leg | exit | evidence |
|---|---|---|
| `./arroyo check` | **0** | 66 ✅ / 0 ❌; cfg coverage 45 legs; GATE-KNOB OK (154 features, 153 named, 0 phantom, 0 dead); GATE-LEDGER OK (68 rows) (`trunkpreview-check.log`) |
| `UNAOS_WC=1 ./arroyo test 150` | **0** | banner `kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`; serial.log 1480 lines; 35 `-> PASS ::`; 0 FAIL/panic (`trunkpreview-test-x86.log`, `serial-x86.log`) |
| `./arroyo test-arm 60` | **0** | serial-arm.log 479 lines; 3 positive witnesses (heartbeat, BOT-PARK PASS, SERWIT-2 PASS) (`trunkpreview-test-arm.log`, `serial-arm.log`) |
Deltas vs the orin 13 battery on `be3b027e`: check legs 62 → 66 (+`arm-tegra-orinrx`, `knob hygiene`,
`ledgers`, `EL0 user blob`); cfg legs 44 → 45; the two gates arrive with this arc's `arroyo`; x86
serial 1482 → 1480 lines (timing), PASS 35 = 35; arm 479 = 479, witnesses 3 = 3.

## Trunk battery on `d11cd56e` (`../UnaOS`; `~/unaos-bench/scratch/orin14/trunk-battery.sh`, logs `trunk-*.log` there)
| leg | exit | evidence |
|---|---|---|
| `./arroyo check` | **0** | 65 ✅ / 0 ❌; GATE-KNOB OK; GATE-LEDGER OK (68 rows) (`trunk-check.log`). The 66th green line of the preview, `EL0 user blob: 51 bytes`, is the blob build step, which printed only where the target was cold — not a leg |
| `UNAOS_WC=1 ./arroyo test 150` | **0** | banner `⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc` (`trunk-test-x86.log`); serial 1474 lines; 52 `PASS ::`; 0 `panicked at|PANIC:|FAIL` (`trunk-serial-x86.log`) |
| `./arroyo test-arm 60` | **0** | serial-arm 479 lines; 3 positive witnesses (heartbeat + PASS) (`trunk-test-arm.log`, `trunk-serial-arm.log`); battery done 19:32:21Z |
| `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` (Pi bare-metal, raspi4b — pi 6 named the gap: `test-arm` is the virt board) | **0** | the gate's own line (`mbench.py`): `✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 20539 lines scanned` (`trunk-k8.log`; 19:34→19:40Z, host load 8.05→6.88). **Green under load, single run — evidence, not a clearance** (pi's convicted false-red-under-contention family cuts both ways). Denominator derivation: the battery counts `REQUIRE` + `COUNT` directives — at `d11cd56e` `pi4-regression.spec` has REQUIRE 117 + COUNT 2 = 119 (pi 6's reconciliation: its own tree 116 + 2 = 118, the number its baton documents); a grep of `^REQUIRE` alone answers a different question and would read a truncated 117 as a pass |
Caveat that applies to every green above: `test`/`test-arm` are negative-only scans plus the positive
witnesses counted here; no QEMU leg compiles `tegra`. The Orin evidence is the flight.

## Flagged
- The rmbp landing (PWRNAME, KNOBLEG, GATE-ROOTS, GATE-KNOB, GATE-LEDGER `e693056a..d815e659`) has not
  landed; when it does, `jetson-sync1.spec` is taken from trunk (tokens `[orinreboot]`→`[pwrreboot]`).
- `docs/dev/evidence/orin13/pulseoccl-fbcon.patch` is NOT landing with rmbp's arc (rmbp 11, this
  session); S15/B1 stay open with the Orin measurement from this arc.
- A17 grant (rmbp 11): `A17-prtscr.patch` APPROVED for prtscr.rs with two conditions — the same commit extends `docs/dev/OS/08_VIDEO/screenshot.md` §9 (the `-> capturing` line; 0 bytes = interrupted write), and the commit body pastes the four chord lines from the two-QMP-chord `UNAOS_WC=1 ./arroyo test-fat sf 200` proof (SCREEN0 and SCREEN1 both inflating).
- LEDGER S18 (pi 6): root cause is the `pidesk`→`desktop_firmware` rename that the trunk fold `a9449785`
  left 6 sites short on hw-pi4; fix is 6 mechanical lines, owner pi, gate rmbp — folded into LEDGER.md
  in this arc's docs commit.

## This arc (orin 14, hw-jetson, UNLANDED — lands after render4 scores it)
Code: DESKSCENE `8cbfaadf` (A18: the cascaded scene retires the strip and its embedded pulse, the pulse
window is serviced again; overlap now 24 px of frame) · RXDISCRIM `fe6fe3b5` (A16: `ovrf=` on the census,
`iir=/fifo=` once, `~/unaos-bench/tools/inject-paced.sh`; knob-off kernel8.img and stripped knob-off ELF
byte-identical) · PRTSCR2 (A17: the code commit lands via the two-chord proof; see §Flagged).
Docs/ledger: LAWS `15bd2fd7` (landing-merge shape check) · FLIGHTPREP4 `8682f142` · P5SWEEP `3dcade58`
(41 stale unmarked orin files → 30 already landed, 2 dropped, 0 still open; S27, C10) · NEUTRALTABLE
`ebd98270` (7 families / 34 sites stripped vs the naive 12/43; 29 families / 265 sites with the colon-prefix
witnesses; S6) · OCCLUDE13 `60fe2747` `da2b4583` `5e0740dc` (S15 measured, S13 landed in scheduler.md §2.z)
· LEDGERTICK `8b6651ef` (§A–F reconciled against render3b) · S7CONVERGE `398fae07` `74ac7f26` (design) ·
fold `1aaf71f5` (S18 root cause from pi 6, S26 range from rmbp 11, A19).
Union gate on the fold: `./arroyo check` on `1aaf71f5` → CHECK EXIT 0, 65 ✅ / 0 ❌ (`~/unaos-bench/scratch/orin14/union-check.log`).
Executors: nine spawned in turn one, one re-spawned; the session limit at ~19:40Z killed three mid-gate
(PRTSCRGATE, OCCLUDE13, LEDGERTICK) — every one had its work committed under the pause clause and was
folded by hand, two ledger-row conflicts resolved by taking the later measurement (B1) and the
reconcile (A8/A9/B2/B3).

## Handoff
Baton: `~/.claude/plans/unaos/batons/orin-15.md`. Resume memory updated. Card holds render3b + the two
SCREEN files + UPD0.TMP (tidy before the render4 load). Pushes: `git push origin main` (d11cd56e),
`git push origin hw-jetson`.

## Pushes
`git push origin main` (d11cd56e) — DONE by Peter (origin main = d11cd56e, ls-remote 22:17Z). `git push origin hw-jetson` — owed, after the union check (green at `1aaf71f5`).
