# TASKEXIT — the per-task retirement line (B211, 2026-09-24)

Built for SMPBALK8's open half (LEDGER SR16): a kernel task that vanishes leaves no witness. Two emitters in
`arch/aarch64/sched.rs`, witness-gated, kernel threads only:
- `exit()` → `[taskexit] tid=N name='X' core=C reason=exit` (on-CPU; `core=` is the core running the death)
- `retire_killed` → `[taskexit] tid=N name='X' core=C reason=killed` (off-CPU reap; `core=` is the recorded placement)

## Measured — the virt login lane (`UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_FATIMG=sf ./arroyo test-arm 120`)
Features `witness,ehcihid,kbdwit,sdw,sdhcblk,login,loginst,smolnet,virt_el0,sdwrite`; rc=0; ladder tail `:: CAPSTONE COMPLETE` at line 396/397.
Sixteen `[taskexit]` lines, one per kernel thread that returned, every one on the core it died on; no `reason=killed` on
virt (nothing is killed there), which is what a virt boot should read:
```
[taskexit] tid=3 name='sec-probe' core=2 reason=exit
[taskexit] tid=5 name='sec-probe' core=3 reason=exit
[taskexit] tid=1 name='sec-probe' core=1 reason=exit
[taskexit] tid=8 name='virt-el0-verdict' core=0 reason=exit
[taskexit] tid=10 name='cap-sem' core=0 reason=exit
[taskexit] tid=11 name='cap-mtx-a' core=0 reason=exit
…
:: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::
[taskexit] tid=9 name='capstone' core=0 reason=exit
```
```
$ mbench --replay target/serial-arm.log --spec scripts/specs/arm-login.spec --quiet   → ✅ MBENCH PASS — 19/19 required witnesses
$ foreman --log … --spec …                                                          → 19/19, identical
go-red: the same spec against the previous login capture (armlogin, no emitter)         → ❌ 18/19, FIRST-SHORTFALL arm-login.spec:109 [taskexit]
```
`./arroyo check` on the tip before the edit: green but for the container's root-user host test; the edit is two same-line
folds (`git diff --numstat` 2/2 on sched.rs), so no panic `Location` moved. The Pi half is PENDING in `pi4-regression.spec`:
the pi seat's first `kernel8-test` after this fold answers SMPBALK8's open question by reading the stolen verdict's own row.
