#!/usr/bin/env python3
# DOCKSTAMP widening probe — SCRATCH, applied to BOTH trees, reverted before the commit.
#
# It is DOCKID2's probe (B165) widened from ONE hidden row to TWO: the two rows the fixture
# allocates in REVERSE TABLE ORDER (idC takes the high slot FIRST, idD then recycles the freed low
# slot) are withheld from every reconcile's own scan while they are minted, and released together.
# The next reconcile therefore admits BOTH IN ONE PASS — which is the residual B165 named verbatim
# and the only shape that can tell table order from allocation order.
#
# Same file, same hunks, same fixture on the parent tree and on the fixed tree: one variable.
import sys

p = sys.argv[1]
s = open(p, encoding='utf-8').read()

# 1. the hide mask, beside the registry's own counters
old = "static NEXT_SEQ: AtomicU64 = AtomicU64::new(1); static RECONCILES: AtomicU64 = AtomicU64::new(0);"
assert s.count(old) == 1, "NEXT_SEQ anchor"
s = s.replace(old, old + " static PROBE_HIDE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0); // DOCKSTAMP-PROBE (scratch)")

# 2. reconcile withholds the hidden ids from the model it admits from
old = "let (n, _) = wm::dock_scan(&mut scan, (0, 0, 0, 0)); let rows = &scan;"
assert s.count(old) == 1, "reconcile scan anchor"
new = ("let (mut n, _) = wm::dock_scan(&mut scan, (0, 0, 0, 0)); "
       "{ let hide = PROBE_HIDE.load(Ordering::Relaxed); if hide != 0 { let mut m = 0usize; "
       "for i in 0..n { if hide & (1u32 << (scan[i].id as u32 & 31)) == 0 { scan[m] = scan[i]; m += 1; } } n = m; } } "
       "let n = n; let rows = &scan;")
s = s.replace(old, new)

# 3. the fixture arms the hide across BOTH allocations and releases it before the pass that admits
old = """    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let _ = strip_model(&mut rows);"""
assert s.count(old) == 1, "arm anchor"
new = """    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    PROBE_HIDE.store((1u32 << (w[1] as u32 & 31)) | (1u32 << (w[2] as u32 & 31)), Ordering::Relaxed);
    serial_println!(":: DOCKSTAMP-PROBE: hide armed win={} win={} — the elder (idC, HIGH slot, allocated FIRST) and the slot idD will RECYCLE are withheld from every reconcile's scan ::", w[2], w[1]);
    let _ = strip_model(&mut rows);"""
s = s.replace(old, new)

old = """    let recycle_ok = w[3] == w[1];

    let n = strip_model(&mut rows);"""
assert s.count(old) == 1, "disarm anchor"
new = """    let recycle_ok = w[3] == w[1];

    PROBE_HIDE.store(0, Ordering::Relaxed);
    serial_println!(":: DOCKSTAMP-PROBE: hide cleared — win={} (allocated FIRST) and win={} (allocated SECOND, recycled LOWER id) are BOTH live and BOTH tileless; the next reconcile admits them in ONE pass ::", w[2], w[3]);
    let n = strip_model(&mut rows);"""
s = s.replace(old, new)

open(p, 'w', encoding='utf-8').write(s)
print("probe applied to", p)
