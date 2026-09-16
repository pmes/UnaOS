# W5I2 — NMI probe of a declared-dead core: the x86 witness self-test, its go-red, and the knob-off measurement

Branch `exec-rmbp-w5i2`, parent 891c4dec, 2026-09-16. Worktree `~/unaos-bench/scratch/rmbp-0915/w5i2`;
build logs `~/unaos-bench/scratch/rmbp-0915/w5i2-logs/` (bench scratch, not in git — every line a row cites
is quoted here). Design: `docs/dev/OS/08_VIDEO/PCIE-RP-RECOVERY.md` §12.3 (I2, the "built" paragraph).

## 1. The one QEMU run (R38: the executor's own fixture, one run)

Command, from `unaos/`: `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_QEMU_FULL=1 ./arroyo test 120` — `witness` is
armed by the verb itself. Banner: `⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc,quarry,sdwrite`.
rc=0. Sidecar `serial.log.run`: `mode=full`, `wall=126.7`, `cap=120` — the whole wall, not a truncated run.
mbench: `PASS — 6/6 required witnesses, 0 forbidden hit(s), 2522 lines scanned [full wall 126.7s]`.

The four lines, `awk 'index($0,":: W5:")' target/serial.log`, verbatim:

```text
:: W5: nmi core=c4 taken=y rip=0x3c473119 in_blit=n cs=0x8 memcpy=0x3c4eaaf0..0x3c4eab1f icr=ok ::
:: W5: nmi selftest spin core=c4 taken=y rip_in_spin=y in_blit=n spin=0x3c473114..0x3c47311f released=y -> PASS ::
:: W5: nmi core=c4 taken=y rip=0x3c487103 in_blit=n cs=0x8 memcpy=0x3c4eaaf0..0x3c4eab1f icr=ok ::
:: W5: nmi selftest idle core=c4 taken=y rip_in_spin=n -> PASS (NMI wakes HLT; QEMU cannot produce taken=n, only metal can) ::
```

Read: the parked core (c4, the last worker) took the NMI at `rip=0x3c473119` = `unaos_nmi_spin + 5`, the
`cmp byte ptr [rdi], 0` after `mov byte ptr [rsi], 1` (3 bytes) and `pause` (2 bytes) — inside the loop's
11-byte range; `in_blit=n` because the rip is not in `memcpy`'s 47-byte range (`0x3c4eaaf0..0x3c4eab1f` =
0x2f bytes, which is `nm -S`'s size for `t memcpy` on the same artifact); `cs=0x8` ring 0. Released, the task
exited (`released=y`). Idle probe: `taken=y` at `rip=0x3c487103` (the scheduler's idle path, not the loop):
an NMI wakes HLT. What this run cannot show is `taken=n`: TCG holds no store in flight.

Artifact certification (`LC_ALL=C grep -a -o -F`, not `strings`): `':: W5: nmi core=c'` = 4 in
`target/x86_64-unaos/release/unaos-kernel`; control `':: W5: nmi core=ZZZ'` = 0. `nm -S`: `t memcpy` size
`0x2f`; `T unaos_nmi_spin` / `T unaos_nmi_spin_end` 0xb apart.

Pre-existing on this run and unrelated to W5I2 (report-not-touch): `:: STACK: task=u7x-launch overflow guard
hit sp=… cpu=3 id=20 low=… size=16384 guard=4096 entered ::` — present identically on the go-red run.

## 2. Go-red (LAWS §5: verify a gate by making it fail)

Mutation: the three record stores in `interrupts.rs` `w5nmi::record` (`RIP[slot].store`, `CS[slot].store`,
`TAKEN[slot].fetch_add`) commented out; same command. rc=1, `mode=full`. The handler still runs and the
NMI is still taken, but the caller sees no record — the exact wire shape a hardware-parked core produces:

```text
:: W5: nmi core=c4 taken=n rip=- in_blit=? cs=- memcpy=0x3c4e5e40..0x3c4e5e6f icr=ok ::
:: W5: nmi selftest spin core=c4 taken=n rip_in_spin=n in_blit=? spin=0x3c46c2dc..0x3c46c2e7 released=y -> FAIL ::
:: W5: nmi core=c4 taken=n rip=- in_blit=? cs=- memcpy=0x3c4e5e40..0x3c4e5e6f icr=ok ::
:: W5: nmi selftest idle core=c4 taken=n rip_in_spin=n -> FAIL (NMI wakes HLT; QEMU cannot produce taken=n, only metal can) ::
```

`arroyo test`: `❌ x86_64 test FAILED — the serial log carries fault text` (the `-> FAIL` FORBID). So the
`taken=n` printing path — the only verdict metal can add — has executed once, as a false negative by
construction. Mutation reverted; `git diff` carries no `GO-RED` marker; `./arroyo check` re-run (§4).

## 3. Knob-off byte identity (EXECUTOR-BRIEF §5)

`./arroyo knoboff witness 891c4dec` → exit 0. `knoboff: warm=yes compiled_baseline=[unaos-kernel|unaos-kernel]
compiled_tree=[unaos-kernel|unaos-kernel]`; control `x86 armed≠off: YES  arm armed≠off: YES`;
`✅ x86 knob-off image BYTE-IDENTICAL to baseline — 93c34b8ce5c2748a568caf7eaeeb6759f4dbd5851a260b6005328d5e11ba667b 1616504`;
`✅ arm knob-off image BYTE-IDENTICAL to baseline — 53eb8e8827b18dfcf3422435fcea77db10b68d30ab5409ed1e09d691268636ef 1618064`.
Why it holds: the probe is under `any(witness, bar1wedge)`, both off in the knob-off build; the three
source edits are a tail block (`interrupts.rs`), a tail append (`apic.rs`), and a same-line fold at
`sched.rs:1560` — the file keeps 7590 lines, so no `panic::Location` moves.

## 4. Gates

| gate | rc | log (bench scratch) |
|---|---|---|
| `./arroyo check` (before the go-red; both arches, all legs ✅) | 0 | `w5i2-logs/check1.log` (two `function_casts_as_integer` warnings in the new code, fixed before knoboff; `apic.rs:393 elapsed_pm` is pre-existing) |
| `./arroyo knoboff witness 891c4dec` | 0 | `w5i2-logs/knoboff-witness.log` |
| `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_QEMU_FULL=1 ./arroyo test 120` | 0 | `w5i2-logs/test-run1.log`, `serial-run1.log(.run)` |
| same, go-red mutation | 1 (expected) | `w5i2-logs/test-run2-gored.log`, `serial-run2-gored.log` |
| `./arroyo check` after the revert (both arches, 157 legs ✅, 0 errors, 0 warnings in the W5I2 files) | 0 | `w5i2-logs/check2.log` |
| `bash unaos/scripts/ledger-check.sh` before / after the doc edits | 0 / 0 | `w5i2-logs/ledger-check-{before,after}.log` |
