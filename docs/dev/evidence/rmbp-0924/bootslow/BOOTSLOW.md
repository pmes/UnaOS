# BOOTSLOW: the root pass runs before the probes that do not serve it (B201)

Folded from the WIP tip the seat committed on 2026-09-23 (branch `exec-rmbp-bootslow`), 2026-09-24, cloud
session. Mechanism: rmbp-ledger B201; spec block: `x86-wc.spec` (BOOTSLOW). The executor stopped before
writing this record; the gates below are the fold session's.

## Gates

Recorded as they report: the x86 `wc` lane (`root-pass BOUND`, `root-bind d=`, `fixture n=1 … verdict=bound`)
and the go-red (`root_pass_open` returning `true` unconditionally).
