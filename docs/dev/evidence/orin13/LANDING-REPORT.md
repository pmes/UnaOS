# orin 13 — LANDING REPORT (2026-09-05)

## What landed
- **Trunk**: `main` = `be3b027e` (`--no-ff`, parents `0ed6fee2` + `077a8fa1`), 64 commits from
  `hw-jetson` (orin 10 → orin 13), merge-base `0ed6fee2`; tree identical to `077a8fa1`'s (rmbp 11
  verified). Peer ack: rmbp 11 (8085c9c8 at 17:20Z, extended to 077a8fa1 with a push-order condition,
  satisfied: hw-jetson pushed before main). **On origin** (fresh ls-remote at write time, below).
- **This arc's commits** (all on hw-jetson, all gated CHECK_EXIT=0 at fold time):
  NOTMP `ac27b8d2` · STAGECENSUS `a5a66fc1` · PAINTPULSE `7ffd2122` · STACKSEED `01739a93` ·
  CAPREVOKE `06858185` · REVIEWFIX `8085c9c8` · docs `077a8fa1` (orin-desktop.md §3.14).
- **Reviews** (independent, adversarial): `review/RENDER-REVIEW.md` (no blocker; 2 MEDIUM → fixed in
  REVIEWFIX), `review/CAPREVOKE-REVIEW.md` (no blocker; F1/F2 inherited x86 semantics, ledgered).
- **Flight**: render2 on the Orin, all seven questions answered — `FLIGHT-RESULT.md`.

## Trunk battery on be3b027e (../UnaOS)
| leg | exit | evidence |
|---|---|---|
| `./arroyo check` | **0** | 62 legs ✅, 0 ❌ (`trunk-check.log`) |
| `UNAOS_WC=1 ./arroyo test 150` | **0** | banner `kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc` — `wc` present; run completed; serial.log 1482 lines (`trunk-test-x86.log`) |
| `./arroyo test-arm 60` | **0** | serial-arm.log 479 lines, 3 positive witnesses (heartbeat + PASS verdicts) (`trunk-test-arm.log`) |
Caveat that applies to every green above: `test`/`test-arm` are negative-only scans plus the positive
witness counted here; no QEMU leg compiles `tegra`. The Orin evidence is the flight, not the battery.

## GATE-FAMILY — the three-part answer for `render_service` size 3 (quote from merge commit be3b027e)
1. **Shared part**: the pass loop — notice dirty state, composite, present, service the furniture
   (pulsewin), print a census. Not extracted because the three members do not share the two things
   that decide the loop's shape: how the pass WAITS and who OWNS INPUT. Designing the abstraction from
   the thinnest member (the Orin's) would be designing from the least-constrained instance.
2. **The axis that differs**: waiting and input ownership. Pi: blocking `GUI_CHANNEL.recv()` + the sole
   `shell_inbox` drain, `baremetal`-gated. x86: its own `desktop_uefi`/Kepler-ignited path. Orin:
   cooperative busy-poll on a terminus with no sleeper drain and NO input half (`jd2_console_pump`
   owns the keys). Not instance identity, not focus.
3. **Would a parameterised call of the existing member have worked?** No, as the code stands:
   `render_service` is `#[cfg(baremetal)]` and `baremetal`+`tegra` is a `compile_error!`; its blocking
   recv would park forever on the tegra terminus. It WOULD work once the waiting axis is lifted into
   a parameter/trait — that is the convergence arc, owed. **The size-3 ledger entry is justified WITH
   AN EXPIRY**: until the convergence arc lands; not to be cited as permanent.

## Flagged
- `presents` stays at 2 on the Orin after pass 2 (load sampler reads zero) — LOADSAMPLER executor running.
- Print Screen on the Orin: key edge already decoded by the shared xHCI driver; missing a
  `prtscr::service()` call on the terminus + `holocron` in the image — PRTSCR-ORIN executor running.
- CAPREVOKE moves the Pi's knob-off `kernel8.img` byte-identity baseline (+29 lines); pi 6 granted
  and will re-base at the fold.
- GATE-NEUTRAL exposure on this branch: 12 `[orin…]` families / 43 sites in main.rs+video, 13
  `orin_`/`tegra_` symbols in main.rs, the `orin-render` task name, the `PIUSB` family in the shared
  USB driver. Awaiting rmbp's gate before any rename.
- The orin-13 baton's "bootloader was the only invisible binary" was wrong: `builder` was a second
  (rmbp GATE-ROOTS `e1bff790` names all nine).
- The knob→leg coverage line in every check log above is still the check that cannot fail on this
  branch (rmbp's KNOBLEG fix is on hw-rmbp, not yet in trunk).

## Pushes
Both done by Peter and verified: origin/main = be3b027e, origin/hw-jetson = 077a8fa1.

## Handoff
Baton: `~/.claude/plans/unaos/batons/orin-14.md`. Resume memory updated. Card holds render2.
