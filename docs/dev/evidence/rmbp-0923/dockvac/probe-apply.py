#!/usr/bin/env python3
# DOCKVAC decline probe — SCRATCH, applied to BOTH trees (parent and fix), reverted before any commit.
#
# FIXTURE_FLAKES §1e's red: the three `wm::close` calls in `dock::selftest` leg 6 each end in
# `wm::composite()`, and on x86 that call DECLINES when another core holds `COMP_GATE` (the decline
# arm returns in microseconds, "composites nothing and CLEARS NOTHING"). With every one of them
# declined, `dock::compose` never runs between the closes and the after-sample, so `SLOT.packed()`
# still holds the pre-close rect and the leg reads `vacate=false`.
#
# This probe reproduces exactly that on demand: it makes every `composite()` on every core take the
# DECLINED arm for 80 ms from the moment the fixture samples `rect_before` — a COMP_GATE held by a
# stalled holder, which is what gate10's red capture shows (a sibling core's pass printed
# `[dock] tile remove win=1` and then did not print its census until after the verdict).
# 80 ms is DMGFLAKE's forced-fold hold (FIXTURE_FLAKES §1c), under DOCKID2's 250 ms budget.
#
# Same file, same hunks, same fixture on the parent tree and on the fixed tree: one variable.
import sys

root = sys.argv[1]
wm = root + "/unaos/crates/kernel/src/video/wm.rs"
dock = root + "/unaos/crates/kernel/src/video/dock.rs"

s = open(wm, encoding="utf-8").read()
old = "static COMP_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);"
assert s.count(old) == 1, "COMP_PENDING anchor"
s = s.replace(old, old + " pub static DOCKVAC_PROBE_UNTIL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // DOCKVAC-PROBE (scratch)")
old = """        if !stole
            && COMP_GATE
                .compare_exchange(false, true, AcqRel, Relaxed)
                .is_err()
        {"""
assert s.count(old) == 1, "decline-arm anchor"
new = """        if !stole
            && (crate::arch::ms() < DOCKVAC_PROBE_UNTIL.load(Relaxed) || COMP_GATE
                .compare_exchange(false, true, AcqRel, Relaxed)
                .is_err())
        {"""
s = s.replace(old, new)
open(wm, "w", encoding="utf-8").write(s)

s = open(dock, encoding="utf-8").read()
old = "    let rect_before = SLOT.packed();\n"
assert s.count(old) == 1, "rect_before anchor"
new = old + ("    wm::DOCKVAC_PROBE_UNTIL.store(crate::arch::ms() + 80, Ordering::Relaxed); "
             "serial_println!(\":: DOCKVAC-PROBE: every composite declines for 80 ms from ms={} (COMP_GATE held, as under a stalled holder) ::\", crate::arch::ms());\n")
s = s.replace(old, new)
open(dock, "w", encoding="utf-8").write(s)
print("probe applied under", root)
