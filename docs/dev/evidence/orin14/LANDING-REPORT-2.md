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
Caveat that applies to every green above: `test`/`test-arm` are negative-only scans plus the positive
witnesses counted here; no QEMU leg compiles `tegra`. The Orin evidence is the flight.

## Flagged
- The rmbp landing (PWRNAME, KNOBLEG, GATE-ROOTS, GATE-KNOB, GATE-LEDGER `e693056a..d815e659`) has not
  landed; when it does, `jetson-sync1.spec` is taken from trunk (tokens `[orinreboot]`→`[pwrreboot]`).
- `docs/dev/evidence/orin13/pulseoccl-fbcon.patch` is NOT landing with rmbp's arc (rmbp 11, this
  session); S15/B1 stay open with the Orin measurement from this arc.
- LEDGER S18 (pi 6): root cause is the `pidesk`→`desktop_firmware` rename that the trunk fold `a9449785`
  left 6 sites short on hw-pi4; fix is 6 mechanical lines, owner pi, gate rmbp — folded into LEDGER.md
  in this arc's docs commit.

## Pushes
`git push origin main` (d11cd56e) and `git push origin hw-jetson` (after this arc's folds).
