# GATES — B103 (the CAPSTONE tail that could not fail), WCLANE (x86-wc.spec replays itself), B96 (check-roots.sh takes a tree) — 2026-09-24

Cloud session, QEMU 8.2.2 TCG. Serial logs read with awk.

## B103 — `:: CAPSTONE COMPLETE` was unconditional
`arch/aarch64/sched.rs`: `cap_report` now counts FAILs and the closing line is the six verdicts' sum.
INCOMPLETE is declared as a second ladder tail (`LADDER_TAIL_test_arm_gicv3`) and the census's last rung
is the closing phrase both lines share (`6 sync primitives`); arm-login.spec's COMPLETE marker accepts
both lines. The pi and jetson specs' `REQUIRE CAPSTONE COMPLETE` are unchanged: they red on INCOMPLETE,
as they should.

ON — `UNAOS_GICV3=1 ./arroyo test-arm 150`, rc=0:
```
:: CAPSTONE: verifying all 6 sync primitives (workers on cores 0 + 0) ::
:: CAPSTONE Semaphore: PASS ::
:: CAPSTONE Mutex: PASS ::
:: CAPSTONE Channel: PASS ::
:: CAPSTONE Condvar: PASS ::
:: CAPSTONE RwLock: PASS ::
:: CAPSTONE join: PASS ::
:: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::
⚡ ladder [test-arm/gicv3]: reached its declared tail ":: CAPSTONE COMPLETE" at line 107 of 107 — the capture is COMPLETE (census 4/4 rungs present).
```
Go-red — `cap_report("join", false)`, same command, rc=1 (the verb's fault scan names both lines):
```
:: CAPSTONE join: FAIL ::
:: CAPSTONE INCOMPLETE — 1/6 sync primitives FAILED in this boot -> FAIL ::
  ✖ serial-arm.log:108: :: CAPSTONE join: FAIL ::
  ✖ serial-arm.log:109: :: CAPSTONE INCOMPLETE — 1/6 sync primitives FAILED in this boot -> FAIL ::
```
The verb reds on the fault text before its ladder step, so the tail verdict on that capture was taken
by running `ladder_tail_check` in isolation (the function and the two LADDER_ variables sourced from
arroyo) against both captures:
```
⚡ ladder [test-arm/gicv3]: reached its declared tail ":: CAPSTONE INCOMPLETE" at line 109 of 109 — the capture is COMPLETE (census 4/4 rungs present).
⚡ ladder [test-arm/gicv3]: reached its declared tail ":: CAPSTONE COMPLETE" at line 107 of 107 — the capture is COMPLETE (census 4/4 rungs present).
```
Before this change the go-red capture would have been called TRUNCATED (no verdict) instead of RED.
Source restored (grep GO-RED MUTATION = 0).

## WCLANE — x86-wc.spec is in the verb's automatic replay set (rmbp-ledger B156's finding)
`x86_pick_capture_spec` now selects `X86_WC_SPEC` for `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1` with no
medium or lane knob. In this container every wc run carries the environmental `:: SOCK-3:` fault text and
the verb returns before its replay step, so the selection was proven by running the picker's body in
isolation under five knob shapes:
```
UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1                       -> x86-wc.spec|
UNAOS_WC=1                                                     -> x86-default.spec|
UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_FATIMG=sf       -> x86-fat.spec|
UNAOS_USBNET=1 UNAOS_NOE1000=1 UNAOS_WC=1                      -> x86-usbnet.spec|
UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_LOGIN=1         -> x86-default.spec|
```
(The login lane keeps its hand replay of x86-login.spec, as before.) The same wc leg replayed by hand
on this tree: 41/41 (SO22.md). GATE-SPECROOTS: OK, 21 specs, x86-wc.spec now GATED by name.

## B96 — `check-roots.sh [<unaos dir>]`
```
GATE-ROOTS: tree=/home/user/UnaOS/unaos
GATE-ROOTS: binary targets and the check leg(s) naming each —
GATE-ROOTS: OK — 9 binary targets, every one named by a leg of check_both
rc=0
GATE-ROOTS: tree=/home/user/UnaOS/unaos
GATE-ROOTS: binary targets and the check leg(s) naming each —
GATE-ROOTS: OK — 9 binary targets, every one named by a leg of check_both
rc=0
GATE-ROOTS: control FAILED — /nonexistent is not a directory. No verdict.
rc=2
GATE-ROOTS: usage: check-roots.sh [<unaos dir>] — got 2 arguments. No verdict.
rc=2
```

## K8REACH — the two new knobs (SR1's class, caught)
`python3 unaos/scripts/k8-reach.py` on the tip before this commit: `❌ k8-reach UNREGISTERED: UNAOS_HDASIE
UNAOS_USBNET — knob(s) with no K8_FEATS arm and no registry row`. USBNET got an arm (`kernel8` carries
`usbnet` when the knob is set — a Pi with a dongle is the target); HDASIE an `NA` registry row with its
`--evidence` (4 sites, all inside drivers/hda.rs, never lexed on aarch64). After:
```
  ✅ k8 reachability (163 knobs: 16 armed, 147 registered unarmed — 102 still TODO)
```

## B101 — the pstrip FORBID, gap-tolerant in the emitter's order
Old: `\[pstrip\] rollup samples=[1-9][0-9]* redraws=[0-9]+ skipped=0 srcdelta=0` (four adjacent fields).
First cut: three lookaheads, order-free — **refused by GATE-FOREMAN**: `foreman` (the Rust twin of mbench,
`regex` crate) has no look-around; `every_checked_in_spec_parses` and `agrees_with_mbench_on_the_shared_corpus`
both failed on the strict check with `error: look-around, including look-ahead and look-behind, is not
supported`. Specs may not use look-around; the pi4-regression.spec comment now says so.
New: `\[pstrip\] rollup .*\bsamples=[1-9][0-9]*\b.*\bskipped=0\b.*\bsrcdelta=0\b` — whole tokens, the
emitter's order, `.*` gaps. Nine lines through Python `re.search` (mbench's matcher), old versus new:
```
case                                                               old    new
collapse line (real emitter)                                       FIRE   FIRE
paced= appended at the tail                                        FIRE   FIRE
paced= INSERTED between redraws and skipped                        silent FIRE
a field inserted between skipped and srcdelta                      silent FIRE
honest line (skipped=90)                                           silent silent
honest line (srcdelta=7)                                           silent silent
degenerate zero-sample rollup                                      silent silent
skipped=05 (not zero)                                              silent silent
fields REORDERED (breaks the emitter's contract — silent by design) silent silent
NEW REGEX VERDICT: PASS
```
`mbench.py --self-test` 46/46; `cargo test -p foreman` 38 + 1 + 2 tests green. The emitter line in
`ui_status.rs` carries a same-line comment naming the spec and the three tokens it may never rename or reorder.

## B94 — GATE-LINENEUTRAL on this session's own commits
```
$ line-neutral.sh 5f95e340 fe119dac   (USBNET)
  unaos/crates/kernel/src/drivers/e1000.rs: neutral (3 hunk(s), every one same-line)
  unaos/crates/kernel/src/drivers/xhci/mod.rs: moved +211 line(s), first at new line 17151 — no panic-bearing site below it (Location-neutral)
  unaos/crates/kernel/src/drivers/xhci/usbnet.rs: moved +544 line(s), first at new line 1 — no panic-bearing site below it (Location-neutral)
GATE-LINENEUTRAL: 5f95e340..fe119dac files=3 neutral=1 moved=2 moved-above-panic=0
$ line-neutral.sh 9d7ffd3e a087eeb0   (SO22)
  unaos/crates/kernel/src/arch/aarch64/boot.rs: neutral (1 hunk(s), every one same-line)
  unaos/crates/kernel/src/arch/aarch64/mmu_tegra_el0.rs: neutral (1 hunk(s), every one same-line)
  unaos/crates/kernel/src/arch/x86_64/memory.rs: neutral (1 hunk(s), every one same-line)
  unaos/crates/kernel/src/arch/x86_64/syscall.rs: neutral (6 hunk(s), every one same-line)
  unaos/crates/kernel/src/video/wm.rs: MOVED +6 line(s), first at new line 964 — 26 panic-bearing site(s) below it (first: line 1862) -> their `Location` literals moved
GATE-LINENEUTRAL: 9d7ffd3e..a087eeb0 files=5 neutral=4 moved=1 moved-above-panic=1
$ line-neutral.sh 86a8ecf2 9d7ffd3e --strict   (the settle relocation)
  unaos/crates/kernel/src/video/strip.rs: MOVED +0 line(s), first at new line 1310 — 6 panic-bearing site(s) below it (first: line 1476) -> their `Location` literals mo
GATE-LINENEUTRAL: 86a8ecf2..9d7ffd3e files=1 neutral=0 moved=1 moved-above-panic=1 — STRICT: RED
exit=0
$ line-neutral.sh nope
GATE-LINENEUTRAL: control FAILED — nope is not a commit. No verdict.
exit=2
```
Reading: same-line hooks are neutral; a tail append moves nothing that matters; a mid-file helper (SO22's
`app_name_armed`) moved 26 sites — harmless here since no knob-off identity claim rides on wm.rs, and now
visible rather than assumed; a relocated block is a +0 MOVE that still shifts what lies between its homes.

## B64 — the option_env! half of GATE-K8REACH
```
$ k8-reach.py   (before the registry rows)
❌ k8-reach ENV-UNNAMED: UNAOS_DMAWIN UNAOS_NET4_BUF1 UNAOS_NET4_DHCP_MS UNAOS_NET4_RINGDUMP — option_env! knob(s) that NO command in arroyo names (code or comment) and no registry row carries as `ENV`. They reach every image the build's environment carries them into, and nothing in the repo says

$ k8-reach.py   (after)
  ✅ k8 reachability (163 knobs: 16 armed, 147 registered unarmed — 102 still TODO; env knobs: 12 option_env!, 6 named by arroyo, 2 dual-keyed with a _feats line, 4 unnamed and registered ENV)

$ k8-reach.py --evidence UNAOS_DMAWIN
  option_env! sites (B64 — build-time env knob, no _feats/K8_FEATS arm needed to reach an image):
    ENV            arch/aarch64/rtl8168_tegra.rs:5286
UNAOS_DMAWIN: 0 site(s), 0 Pi-live, 0 negated
```
Registered ENV: UNAOS_DMAWIN, UNAOS_NET4_RINGDUMP, UNAOS_NET4_BUF1, UNAOS_NET4_DHCP_MS (each with its option_env! site). Named by arroyo (6): UNAOS_FBH, UNAOS_FBW, UNAOS_GIT_SHA, UNAOS_NOJB11, UNAOS_V3D81_SETTLE_MS, UNAOS_V3D_FIRSTKICK. Dual-keyed with a _feats line (2): UNAOS_NET4, UNAOS_SMPPROBE. (Lists computed with the tool's own parsers.)

## PIDESKCOND (B208) — the `desktop_firmware` row scoped to its arch
```
$ banner-cert.sh target/pi_baremetal/kernel8.img baremetal,skip_xhci,witness,desktop_firmware,login,loginst aarch64
feature=desktop_firmware witness=[deskfw] activate DECLINE reason=no-panel hits=1 -> OK
⚡ banner-cert: ok=5 missing/leak=0 registered-divergences=0 unverifiable=1 nowitness=0 noverdict=0
$ banner-cert.sh target/x86_64-unaos/release/unaos-kernel wc,witness,desktop_firmware
feature=desktop_firmware witness=[deskfw] activate DECLINE reason=no-panel hits=- -> UNVERIFIABLE (its literal exists only in a aarch64 artifact; this one is x86_64 — the module is compiled on that arch alone, so the row cannot fire here and says so)
⚡ banner-cert: ok=2 missing/leak=0 registered-divergences=0 unverifiable=1 nowitness=0 noverdict=0
✅ banner-cert: every feature the banner named is in the artifact.   exit 0
```
Before: the same x86 shape exited 2 NO VERDICT (NEUTRAL M1, `docs/dev/evidence/rmbp-0915/neutral/m1-bannercert-pidesk-x86-RED.txt`).

## QUIETKILL (B209) — the completion kill waits for a quiet, newline-terminated tail
Synthetic writer: the COMPLETE marker at t=0, then `[click2] depth gui_chan=0 (se` with a byte every 0.3 s for ~2.7 s, then ` tail)\n`.
```
$ qemu_await.py --log qk.log --spec qk.spec --cap 20 --grace 1 --quiet 0.6 --label qk
⚡ qk: run complete at +0.0s — holding 1s grace with every FORBID live.
⚡ qk: QUIETKILL held the kill 2.4s past the grace until the capture went quiet (0.6s without a byte) on a newline-terminated tail
AWAIT status=complete complete_at=0.0 forbid_hits=0 wall=3.6 quiet_held=2.4        (tail byte at AWAIT: \n)
$ … --quiet 0          (the newline half alone; before this change the AWAIT line came at the grace, +1.5 s, mid-line)
AWAIT status=complete complete_at=0.0 forbid_hits=0 wall=3.0 quiet_held=1.8
$ … --cap 3 --grace 0.5 --quiet 0.6   against a writer that never stops or ends its line
⚡ qk-cap: QUIETKILL held the kill 2.3s past the grace until the capture went quiet (0.6s without a byte) on a newline-terminated tail — cap reached while still writing
AWAIT status=complete complete_at=0.0 forbid_hits=0 wall=3.0 quiet_held=2.3
```
Live measurement is the next bench `kernel8-test` (the leg that produced the false red); this container's QEMU has no raspi4b.
