# orin 14 — LANDING REPORT 3 (the R17 arc, 2026-09-06)

## What landed
- **Trunk**: `main` = `f49ea1e7` (`--no-ff`, parents `d11cd56e` + `671a0334`), 26 commits from `hw-jetson`,
  merge-base `6cc8de8c`. Merged in `../UnaOS` at 2026-09-06T00:20Z after a fresh ls-remote at 00:20:27Z
  (main `d11cd56e`, hw-jetson `2a37aaeb`, unmoved). **Local, unpushed at write time.** Push ORDER (rmbp's
  condition): `git push origin hw-jetson` (671a0334) then `git push origin main` (f49ea1e7).
- **Peer acks**: rmbp 11 (671a0334, their ls-remote 00:19Z; their lane = f0db58bf's approved content only,
  checked by `git diff --stat origin/main 671a0334 -- video/ screenshot.md arroyo scripts/`); pi 6 (their
  ls-remote 00:2xZ; scope: predicates, merge shape, the battery-to-tip gap, pi-lane exposure, byte identity —
  not the panel, L1, or R18/R19's wording; conditional on the four trunk-battery exits below).
- **Panel**: `docs/dev/evidence/orin14/review/PANEL4-REVIEW.md` — no blocker; M1 (Pi bare-metal leg on the
  merge) answered by TRUNK-PREVIEW3 leg (d); L2/L3/L4/L5 applied in REVIEWFIX4 `671a0334`; L1 tabled in
  NEUTRAL-TABLE.md for the S6 batch.
- **Flight**: render4 — `docs/dev/evidence/orin14/FLIGHT-RESULT-render4.md` (every scorer PASS; A16 decided).

## Landing-merge shape check (LAWS §Code and history)
| fact | command | result |
|---|---|---|
| two parents | `git log --pretty=%p -1 f49ea1e7` | `d11cd56e 671a0334` |
| merged tree == arc tip | `git diff 671a0334 f49ea1e7 \| wc -l` | `0` |
| safe: trunk added nothing since the base | `git log --no-merges --oneline 6cc8de8c..d11cd56e \| wc -l` | `0` |

## Preview battery (merge `0e159688` = d11cd56e + 518dca3e; executor TRUNKPREVIEW3; `TRUNK-PREVIEW3.md`)
| leg | exit | evidence |
|---|---|---|
| `./arroyo check` | **0** | 66 ✅ / 0 ❌; GATE-KNOB OK (154/153/0/0/0); GATE-LEDGER OK (71 rows) |
| `UNAOS_WC=1 ./arroyo test 150` | **0** | banner `witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`; 1477 lines; 52 PASS; 0 panic |
| `./arroyo test-arm 60` | **0** | 477 lines; heartbeat + 2 PASS; 0 panic |
| `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` | **0** | `✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 23940 lines scanned` (denominator = REQUIRE 117 + COUNT 2); green under load, single run |
The preview ran at `518dca3e`; the landed tip `671a0334` is four commits later and the only code file in
the gap is `arch/aarch64/serial.rs` (REVIEWFIX4, five hunks all inside `pub mod serialrx`, cfg
`all(tegra, orinrx)`, zero Location-bearing constructs below :700). pi 6 verified: **the Pi's kernel8.img at
671a0334 is byte-identical to 518dca3e's, so 119/119 is a measurement of the landed Pi image**, not an inference.
Named gap (pi 6): `pi4-regression.spec` has 0 prtscr assertions — this landing's sole Pi-compiled change
(`video/prtscr.rs`, PRTSCR2 `f0db58bf`) is ungated by pi's battery. The closer is METAL + an arroyo wiring
(`UNAOS_PRTSCRST` reaches only the x86/virt `$_feats` map, arroyo:541, not `K8_FEATS`, :5419; `selftest_once`
parks without a writable volume on QEMU raspi4b); owner pi (S14).

## Trunk battery on `f49ea1e7` (`../UnaOS`; `~/unaos-bench/scratch/orin14/trunk-battery3.sh`, logs `trunk3-*.log`)
| leg | exit | evidence |
|---|---|---|
| `./arroyo check` | **0** | 65 ✅ / 0 ❌ (`trunk3-check.log`) |
| `UNAOS_WC=1 ./arroyo test 150` | **1** on run 1, **0** on the re-run (00:38Z, `trunk3-test-x86-rerun1.log`: 52 PASS, 0 panic, 0 `[ptrdead]`) | run 1 (host load 24): banner `witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`, 1475 lines, 52 PASS, 0 panic, the one fault line is `[ptrdead] backlog … fpop3=2 -> FAIL` at serial.log:1003 — the known x86 foreign-drain flake under contention (`0d509431`/`badc8732`; PRTSCRGATE saw it on its run 1 and not on run 2; the preview battery on the identical tree was clean). No single-run red convicts (pi's rule); the re-run is queued behind the battery (`trunk-x86-rerun.out`) |
| `./arroyo test-arm 60` | **0** | 478 lines; 3 witnesses; 0 panic (`trunk3-test-arm.log`) |
| `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` | **0** | `✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 21481 lines scanned` (`trunk3-k8.log`; done 00:35:53Z, load 14.3) — green under load, single run; the Pi image at f49ea1e7 equals 518dca3e's (pi 6) |
Caveat: `test`/`test-arm` are negative-only scans plus the positive witnesses counted; no QEMU leg compiles
`tegra`. The Orin evidence is render4.

## This arc (orin 14) — what the 26 commits are
Code: DESKSCENE `8cbfaadf` (A18) · RXDISCRIM `fe6fe3b5` (A16 discriminators) · PRTSCR2 `f0db58bf` (A17) ·
REVIEWFIX4 `671a0334`. Docs/evidence/ledger: LAWS `15bd2fd7`, FLIGHTPREP4 `8682f142`, LANDING-REPORT-2
(`50e337e8`…), P5SWEEP `3dcade58`, NEUTRALTABLE `ebd98270`, OCCLUDE13 (`60fe2747` `da2b4583` `5e0740dc`),
LEDGERTICK `8b6651ef`, S7CONVERGE `398fae07`, the fold `1aaf71f5`, A17 evidence `c2bd516d`, render4
`2a04fb4a`, PANEL4 `2a37aaeb`, BSPTICK `c3cf355e`. Rulings R18, R19 recorded verbatim.

## Round 2 (Peter: "do another massive round of work"), NOT in this landing — lands with orin 15
Folded on hw-jetson after the landing (tip: see `git log`), all gated by their executors (check both arches, test-arm, armed esp-jetson, knob-off kernel8.img byte identity) and by the seat's union gate:
- ORINCLICK `9d3ed9ef` (A20): `orinclick` composes with `deskcascade`; the fix is the recipe (`UNAOS_ORINCLICK=1`). render5 staged: `render5-20260906T0023Z-84b8299` (render4 + click only).
- BSPTICK `c3cf355e` (A21): `tick1-20260906T0013Z-6139327` staged — gap 1's first metal question (`[orinbsptick]` past tick 1 across the JM6 latch); a SEPARATE boot, never on the desktop line.
- SDMMCWRITE `33dc7811` + `95965213` (A23): one CMD24 to a proven-free scratch sector + CMD17 read-back behind `UNAOS_SDMMC=1 UNAOS_SDMMCWRITE=1` — a hardware WRITE, so a probe boot of its own.
- TCURX `5936f239` + `af2c610b` (A16 fix path, A22): the console's RX is the TCU mailbox in HSP (edk2-nvidia, BSD), not UARTC; `tcuprobe` reads the RX mailbox read-only — ON the full-boot image.
- NET4A `a45af90e` + `cf02c6f6` (A12): RX ring + 32 buffers below 4 GiB in a Normal-NC window; NET-4o's 'no clean sub-4 GiB span' was never a measurement (the scan ran over an empty window set). Needs `UNAOS_NET4=1` and the RJ45 cabled.
- GA10B rung 2 `a5a62ffb` + `bcdb23a6` (A24; §F GPU row OPEN per R18): BPMP power + clocks on the gpu@ domain, one PMC_BOOT_0 read behind a pg=ON readback, symmetric restore, RETURNS (probe1's SYSTEM_OFF dropped from the line) — `UNAOS_GA10B_PROBE2=1`; ladder `GA10B-LADDER.md` (R19 vocabulary), record `GA10B-HISTORY.md` `31613555`.
- A19FIX `6f56eff8` + `f2eae02a` (A19): the band is jd2's post-cascade panel paint (case b), not residue; the shell now mints its own wm row from the pump; `[realdesk] band-cleared` + `shell-present` on the wire; scorer `A19-pngband.py`.
- S7STEP1 docs `f6b7a665`: `S7-STEP1.patch` (main.rs Pi region; x86 kernel identical, Pi kernel8.img moves by design, every byte classified) — awaits rmbp's grant; code `102304e6` on `worktree-agent-a02292887f3387e5d`. rmbp 12's review conditions for S7 step 1 (rmbp 11, 2026-09-06, queued as their J6): (a) x86 UNAOS_WC=1 byte identity verified by a two-build diff in the rmbp tree; (b) pi 6's ack in THEIR session (the body converted is the Pi's; main.rs is shared — both lanes sign); (c) GATE-FAMILY stays 3 at step 1 and S7 keeps its expiry; (d) every mid-file hunk N→N lines, growth only at the tail; (e) S7-STEP1.md §3 read against the patch.
- GA10BHIST `31613555`, SERIAL-WATCH `aac72cdc`, REVIEWFIX4 `671a0334` (in the landing), rulings R18/R19.
Full-boot image for orin 15: **`render6-20260906T0049Z-f2eae02` IS ON THE CARD** (written 00:49Z, 10/10 sha match, unmounted; kernel.elf `bec22fd0…`, ELF max vaddr `0x2db188` — pin the boot by that value). Knob line: render4's + `UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1`; effective features `witness,ehcihid,holocron,tegra,orinclick,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,tcuprobe,deskcascade`; union gate on the tip check 68/0, test-arm 0. Probe tokens (`[sdmmc] write`, `[ga10bprobe2]`, `buffers-written`, `[orinbsptick]`) are absent from this ELF by construction — their boots are separate. Watch it per `SERIAL-WATCH.md`.

## Pushes
`git push origin hw-jetson` (671a0334, first) · `git push origin main` (f49ea1e7, second).

## Handoff
Baton `~/.claude/plans/unaos/batons/orin-15.md`; resume memory current. Card holds render6 (the full boot); orin 15 boots it (Peter). Butler pid 34809 held the port at 00:49:33Z — re-check by `lsof` before power-on.
