#!/usr/bin/env python3
"""Emit the set of kernel features a leg list actually COMPILES, one per line.

KNOBLEG (2026-08-31). arroyo's knob->leg coverage check asks "is every aarch64-qualified feature
named by a leg". It answered that question with a LITERAL substring match over the leg feature
strings, and got two things wrong:

  1. It matched against `_rows` = KERNEL_CFG_MATRIX + KERNEL_CFG_MIX + KERNEL_CFG_SWEEP, and the
     `x86-mix-N` legs are MANUFACTURED AT RUNTIME by `build_cfg_legs` out of feature unions that
     include aarch64 features. So every feature was "covered" and the red branch was unreachable.
     Proven by execution, not reading: a probe on `_kl_covered` named x86-mix-1/2/5/6 as the legs
     carrying `aarch64_el0`. Three seats had previously produced three wrong explanations by
     grepping arroyo's TEXT -- and the text is innocent, because the value is computed, not written.

  2. A literal match also cannot see Cargo feature IMPLICATIONS. `aarch64_el0` is named by no leg
     at all, yet 11 board legs name `tegra_el0`, and `tegra_el0 = ["tegra", "aarch64_el0"]`
     (it is also implied by `baremetal`). Fixing (1) alone therefore REDS a feature that is
     genuinely compiled -- a false positive on the one gate whose credibility is the point.

So coverage is the TRANSITIVE CLOSURE of each leg's feature list over [features], and this script
computes exactly that. Cross-crate deps (`crate/feature`) and `dep:` entries are not kernel
features and are dropped.

usage: knob-leg-covered.py <path-to-kernel-Cargo.toml>   < legs-on-stdin (one leg row per line)
"""
import re, sys

cargo = sys.argv[1]
deps, inf = {}, False
for line in open(cargo, encoding="utf8", errors="replace"):
    line = line.rstrip("\n")
    if line.startswith("[features]"):
        inf = True; continue
    if line.startswith("["):
        inf = False; continue
    if not inf:
        continue
    m = re.match(r'^([A-Za-z0-9_-]+)[ \t]*=[ \t]*\[(.*)\]', line)
    if m:
        raw = [d.strip().strip('"') for d in m.group(2).split(",") if d.strip()]
        deps[m.group(1)] = [d for d in raw if d and not d.startswith("dep:") and "/" not in d]

seen = set()
work = []
for row in sys.stdin:
    row = row.strip()
    if not row:
        continue
    parts = row.split()
    if len(parts) < 3:          # "<name> <target> <features>"; a leg with no features is legal
        continue
    work.extend(f for f in parts[2].split(",") if f)

while work:
    f = work.pop()
    if f in seen:
        continue
    seen.add(f)
    work.extend(deps.get(f, []))

for f in sorted(seen):
    print(f)
