# JS1-GREEN — `jetson-sync1.spec` against its own synthetic green capture (A106, 2026-09-24)

QUEUE §5 (NEUTRAL M1, 2026-09-22): the spec was RED against `scripts/specs/jetson-sync1-green.capture`, 24/27. The
three shortfalls are the three rows ORIN-CORE0 (orin 26) widened after the capture was written:
```
  ❌ REQUIRE  TEGRA-EL0: el0-hello spawned at EL0 \(placement=(auto|boot-core) core=-?[0-9]+ tid=[0-9]+\)   (spec:675)
  ❌ REQUIRE  \[spread4\] live c0=[0-9]+/[0-9]+ .* c5=[0-9]+/[0-9]+                                        (spec:1784)
  ❌ REQUIRE  \[el0live\] verdict=[A-Z]+ .* el0cpus=0x([2-9a-f]|[0-9a-f]{2,})                              (spec:1814)
  ❌ MBENCH FAIL — 24/27 required witnesses, 0 forbidden hit(s), 99 lines scanned, pending 7/32 matched
```
The capture's lines 80/81/89/90 carried the pre-orin-26 spellings (`spawned at EL0 (boot core)`, a four-wide
`[spread4]`, an `[el0live]` with no `where` group). Rewritten to the kernel's current format strings
(`main.rs:7162`, `sched.rs:8844`, `sched.rs:9157`), synthetic values that satisfy the rows the spec admits:
```
$ mbench --replay scripts/specs/jetson-sync1-green.capture --spec scripts/specs/jetson-sync1.spec --quiet
  ✅ MBENCH PASS — 27/27 required witnesses, 0 forbidden hit(s), 99 lines scanned, pending 7/32 matched
$ foreman --log … --spec … --quiet                                → 27/27, identical
go-red — each of the three lines reverted in turn:  line 81 → 26/27 FAIL · line 89 → 26/27 FAIL · line 90 → 26/27 FAIL
```
The residual 3 the queue line called pre-existing were these three; there is no residual now. The capture stays
SYNTHETIC (its header says so): it proves the spec CAN go green, and the metal green reference is the next Orin flight.
