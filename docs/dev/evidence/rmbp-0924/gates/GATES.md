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
