# SERWIT-2 — skip_xhci capture and promotion evidence

Input to the queued lead ruling recorded in `unaos/scripts/specs/pi4-regression.spec` (the
`COUNT 25` comment block, lines 93–98):

> SERWIT-2 is excluded for a different and weaker reason, stated so it can be revisited: it has NO
> skip path (`mirror_verdict_once`, one-shot on main-loop entry, prints PASS or `FAIL ::`) and its
> FAIL is already convicted by the default `FAIL ::` FORBID, but its ABSENCE is not, and this arc
> had no UNAOS_SKIP_XHCI capture to prove the ftdi tap's presence is unconditional.
> Promoting it to a REQUIRE is the open ruling.

This document is evidence only. **The spec is not modified here** — the ruling is Peter's.

---

## 1. What SERWIT-2 asserts

Emitter: `unaos/crates/kernel/src/serial_ring.rs`, `mirror_verdict_once()` (lines 1425–1476),
reached one-shot from `mirror_service()` (line 1393).

The doc comment on the emitter states the assertion:

> PASS demands, for every tap, that the conservation law balances — no line is unaccounted for — AND
> that the three EVIDENCE taps lost nothing. `fbcon`'s misses are reported but are not fatal: the
> panel is a view, the line is on the wire regardless, and the property being proven for it is that a
> miss is visible rather than that it never happens.

Mechanically, for each of the four mirror taps (`fbcon`, `ftdi`, `tste`, `flightrec`):

```rust
let accounted = absorbed + dropped + suppressed + inflight;
let gap = submitted.wrapping_sub(accounted);
if accounted > submitted || gap > MIRROR_WINDOW {   // MIRROR_WINDOW = 64
    balanced = false;
}
...
if name != "fbcon" { evidence_lost += dropped; }
```

`MIRROR_WINDOW = 64` is documented as the core-count ceiling on the lock-free sampling artefact, not
a tolerance: "a sampling artefact is bounded by the core count and disappears on the next sample; a
genuine accounting hole … grows without bound with traffic".

The two verdict shapes are disjoint:

```
:: SERWIT-2: mirror taps — every line accounted for on all 4 taps, 0 lost on the 3 evidence taps (ftdi/tste/flightrec) -> PASS ::
:: SERWIT-2: FAIL — balanced={} evidence_lost={} ::
```

**Reachability on Pi bare-metal.** The aarch64 baremetal builds never reach the shared BSP loop, so
the verdict rides `pump_usb_into_gui()` (`crates/kernel/src/main.rs:3165`), which calls
`mirror_service()` as its **first statement — before** the `xhci::claim()` that follows it. That
call site is fed from two places (`main.rs:4063` `usb_pump`, ~4 ms cadence, metal; and
`main.rs:3702`, the `input_service` poll-nap branch, every cooperative pass, QEMU raspi4b), i.e. on
every bare-metal variant. Nothing on that path is gated on the xHCI existing.

---

## 2. The capture

Command (worktree `unaos-wt-exec-serwit2`, branch `exec-serwit2`, base `9259c2bf`):

```
UNAOS_SKIP_XHCI=1 ./arroyo kernel8-test 210
```

Serial capture: `unaos/target/serial-pi.log`, 14,666 lines, copied to the worktree root as
`serwit2-capture-skipxhci.log` (944,269 bytes, sha256 prefix `b1dd874d7500475a`; left **untracked** —
too large to commit, the load-bearing lines are quoted below in full).

Suite result: **107/107 required witnesses green**, 1 forbidden hit — `[pstrip] rollup … skipped=0
srcdelta=26` at capture line 14182. That is the FORBID whose own spec comment (lines 908–913) already
records it as capable of an honest false red on a loaded boot ("a one-off honest zero on a heavily
loaded boot is therefore possible, and this line would call it a fault. The residual is the
EMITTER's"). It is unrelated to SERWIT-2 and outside this arc.

Proof the boot had no xHCI:

```
:: xHCI bring-up SKIPPED (skip_xhci feature): video only, no USB ::
```

**Every SERWIT / serwit-family line in the capture** (`awk 'tolower($0) ~ /serwit/'`, with the
preceding `[mirror]` line for context), verbatim, with capture line numbers:

```
195: [mirror] fbcon: 8 line(s) dropped, 0 truncated since boot (sink contended or full)
196: :: SERWIT-2 tap fbcon: submitted=194 absorbed=181 staged=0 dropped=8 suppressed=5 torn=0 inflight=0 in_progress=0 ::
197: :: SERWIT-2 tap ftdi: submitted=0 absorbed=0 staged=0 dropped=0 suppressed=0 torn=0 inflight=0 in_progress=0 ::
198: :: SERWIT-2 tap tste: submitted=194 absorbed=1 staged=0 dropped=0 suppressed=193 torn=0 inflight=0 in_progress=0 ::
199: :: SERWIT-2 tap flightrec: submitted=0 absorbed=0 staged=0 dropped=0 suppressed=0 torn=0 inflight=0 in_progress=0 ::
200: :: SERWIT-2: mirror taps — every line accounted for on all 4 taps, 0 lost on the 3 evidence taps (ftdi/tste/flightrec) -> PASS ::
```

There are no `[serwit]` / `SERWIT3-END` burst lines in a Pi capture — the SERWIT-1/-3 multi-core
stress fixture is an x86 fixture; SERWIT-2's verdict is the only member of the family that prints
here.

---

## 3. Did the witness print with the xHCI skipped?

**Yes, and unconditionally.** The verdict landed at capture line 200 of 14,666 — early, immediately
after the `input`/`render` task-spawn lines and long before the fixture battery — on a boot whose own
wire says `xHCI bring-up SKIPPED`. A `REQUIRE :: SERWIT-2: mirror taps .* -> PASS ::` would have been
green on this run. **A REQUIRE cannot false-red a skip_xhci boot.**

Two things the capture also settles, both relevant to the ruling:

**(a) The premise was already satisfied by every pi4 capture, and the env knob was a no-op.**
`arroyo:2405` sets `local K8_FEATS="baremetal,skip_xhci"` — the kernel8 feature list is CURATED and
carries `skip_xhci` **unconditionally**. `UNAOS_SKIP_XHCI` is consumed at `arroyo:125`, which appends
to the general `_feats` string that `K8_FEATS` explicitly "does not draw from" (comment at
`arroyo:2414`). So `UNAOS_SKIP_XHCI=1 ./arroyo kernel8-test` and a plain `./arroyo kernel8-test`
build the same image, and **every pi4 regression capture ever taken is a skip_xhci capture**. The
xHCI is only present on this path when `UNAOS_PIUSB=1` adds `piusb` on top (`arroyo:2690`).

**(b) On a skip_xhci boot the `ftdi` tap's PASS is vacuous.** `ftdi: submitted=0` — with no VL805
there is no FTDI cable, so nothing is ever submitted to that tap and its "0 lost" is trivially true.
Same for `flightrec: submitted=0`. The assertion that actually has traffic behind it on this boot is
`fbcon` (194 submitted, 8 dropped — reported, non-fatal by design) and `tste` (194 submitted, 0
dropped, 193 suppressed). So the spec's stated worry — "prove the ftdi tap's presence is
unconditional" — resolves as: the **verdict's** presence is unconditional (proven), the **ftdi tap's
conservation** is only non-vacuously exercised on a `UNAOS_PIUSB=1` / metal boot.

**(c) A correction to the spec comment, found while reading the emitter.** The comment asserts
SERWIT-2's "FAIL is already convicted by the default `FAIL ::` FORBID". It is not. The default
FORBIDs are `-> FAIL`, `FAIL ::`, `PANIC` (`unaos/scripts/mbench.py:135`), and the emitter's FAIL
text is `:: SERWIT-2: FAIL — balanced={} evidence_lost={} ::` — `FAIL` is followed by ` — `, not by
` ::`, and there is no `-> FAIL`. Neither default pattern matches. **SERWIT-2's FAIL is currently
unconvicted, and so is its absence**: today the fixture cannot red the suite by any route.

---

## 4. Recommendation for the lead ruling

**PROMOTE — conditional on landing three lines together, in one commit.**

The absence-blocker is cleared by the capture above, and finding (c) turns the promotion from a
tidiness improvement into a real gap-closure: right now a SERWIT-2 FAIL passes the suite silently.

The three lines, matching the spec's own 2026-08-04 convictability rule (each REQUIRE paired with a
FORBID against the emitter's literal FAIL text):

1. `REQUIRE :: SERWIT-2: mirror taps .* -> PASS ::`
2. `FORBID :: SERWIT-2: FAIL —` (the emitter's literal FAIL shape; the PASS and FAIL shapes are
   disjoint, so the pair cannot both fire)
3. **Keep `SERWIT-2:` in the `COUNT 25` negative lookahead**, OR delete it from the lookahead and
   raise the floor to 26 — one or the other, in the same commit. The PASS line is of the
   doubly-framed `:: LABEL: … -> PASS ::` form the COUNT regex counts, so the spec's own MAINTENANCE
   RULE (lines 99–100) applies. Recommended: keep the exclusion. The REQUIRE is the sharper
   assertion, and the aggregate floor is meant to track the fixture battery, not the transport
   witness.

Failure mode each option risks:

| Option | Risk |
| --- | --- |
| **Promote (recommended)** | A boot on which `mirror_service` genuinely never runs would red the suite. Measured reachability says that cannot happen on a bare-metal aarch64 build (two independent call sites, `usb_pump` and the `input_service` poll-nap, cover metal and QEMU raspi4b respectively, and both call it before touching the xHCI). Residual: if a future arc moves or cfg-gates `pump_usb_into_gui`, the REQUIRE reds — which is the correct outcome, since losing the transport witness silently is the defect it guards. |
| **Do not promote** | Status quo, and the status quo is worse than the comment believed: neither the absence nor the FAIL of SERWIT-2 is convictable (finding (c)). The mirror-tap conservation law — the thing that proves the log you are reading is not lying about what was lost — can regress to FAIL, or stop printing entirely, with the pi4 suite still reporting green. |
| **Conditional (promote REQUIRE only, no paired FORBID)** | Closes the absence hole, leaves the FAIL hole open. A FAIL boot still emits no PASS line, so the REQUIRE would in fact red it — but by inference rather than by conviction, and the spec's stated rule is that the FAIL text gets its own FORBID so the verdict names itself. Weaker than option 1 for no saving. |
| **Promote AND drop the COUNT exclusion without raising the floor to 26** | The aggregate silently gains a line of slack — a real fixture could stop printing with `COUNT 25` still green. This is exactly the R23S1Y defect the re-scope was written to remove. |

Not recommended either way: extending SERWIT-2's REQUIRE to assert `ftdi: submitted=[1-9]`. On the
default pi4 path that would red every boot (finding (b)) — the FTDI tap has no traffic without the
VL805. If a non-vacuous ftdi assertion is wanted, it belongs in a `UNAOS_PIUSB=1` / metal spec, not
in `pi4-regression.spec`.
