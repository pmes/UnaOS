# K9-PARITY landing report — mid-staging-failure discard, the K9 lens-B residual closed in-lane (hw-pi4)

## Summary
K9-MASKCUT (merged `e6cbd9d`) named an honest, consciously-deferred residual: a staged ACL persist
through `native_acl_write_on` that fails PARTWAY leaves UNCOMMITTED in-flight blocks on the shared,
process-wide cached `with_unafs` mount, and a LATER unrelated persist's `commit()` would flush that
orphaned residue as a partial-durable row. The K9 lens-B fold offered two closure options and deferred
both to hold that arc at exact parity with base: (a) a crate-side public rollback primitive (out of the
pi lane); (b) IN-LANE — a staging-failure-path discard. K9-PARITY delivers (b), WITH the mid-staging
fault-injection witness the fold said any such closure must land with.

## Mechanism (in-lane, `libs/fs/unafs` untouched)
- `native_acl_write_on` (the staging wrapper), on ANY failure (`!ok` = staging aborted OR the single
  `commit()` errored), calls `request_mount_discard()` — sets a new module-static
  `MOUNT_DISCARD: AtomicBool` in `fs/unafs.rs`.
- `with_unafs` now runs the closure, then — AFTER `f` returns but BEFORE releasing the `MOUNT` lock —
  `swap(false)`s the flag and, if it was set, drops the cached mount (`*guard = None`). The discard is
  therefore ATOMIC within the SAME uninterrupted (IRQ-masked + `MOUNT`-locked) hold that ran the failing
  op: no SMP window in which another core could observe or commit the dirty mount. A bare post-hold
  `force_remount()` would have that race — this is why the discard lives inside `with_unafs`, not at the
  call sites.
- The next `with_unafs` re-mounts fresh from the committed root (`mount()` on a `None` cache).

## Why it is correct/safe (durable-first / unwind lens)
- **CoW makes discard the right unwind.** On the failing path the root NEVER flipped (K3 durable-first:
  nothing durable changed; the committed tree is ground truth). The uncommitted blocks are a
  power-cut-equivalent LEAK, never a dangle — exactly the residue class the CoW format already tolerates.
  Dropping the mount's `Drop` only `device.flush()`es (no commit), so nothing partial reaches a live root.
- **Flag can never leak.** It is set ONLY inside `native_acl_write_on`, which ALWAYS runs inside a
  `with_unafs` hold; every `with_unafs` (reads included) consumes it via `swap(false)`. Set and cleared
  strictly within one serialized hold — no cross-hold, no cross-boot persistence.
- **Set only on failure.** `if !ok { request_mount_discard() }`; on success the flag is untouched, the
  clean committed mount is retained (the K4 coherence point is preserved).
- **Only the disk-mount cache is dropped**, never in-RAM `OWNED_FILES` (the ACL). A failed persist already
  returns `false` → caller fails closed with the in-RAM grant intact (K3); the discard is orthogonal.
- **Robust to future misuse (noted):** no current site issues two mount-writes per hold. Even if one did
  and a post-failure write then succeeded+committed, the discard would only force a harmless re-read of
  the now-committed root — never data loss. The invariants that matter (K5B fusion, K4 IRQ-mask keystone,
  the mask itself, K3 durable-first) are all untouched; the discard only reloads the committed root,
  exactly as the existing `force_remount` primitive already does.

Lens verdict: **PASS**, zero must-fix.

## Witness
`k9_parity_check` → `:: K9-parity: … PASS [w=0x7f] ::` (7 bits, uncounted; `REQUIRE` +
`FORBID …FAIL` in `pi4-regression.spec`). Rides the U7 kernel task after K3/K5, fully self-cleaning
(no owned row left on the metal card). Drives a REAL mid-staging failure via a test-only
`fs::unafs::TEST_FAIL_MIDSTAGE` knob that aborts `native_acl_stage_row` AFTER the fresh inode +
`name`/`fc`/`owner` have staged (worst-case near-complete, uncommitted residue). Bits:
- b0 clean control row for file A commits;
- b1 the mid-staged persist of file B fails CLOSED (`native_persist_create` returns false);
- b2 a subsequent REAL commit (re-persist A) lands — the volume is usable after the discard;
- b3 after a simulated reboot (`force_remount` + native rebuild) A's row is intact;
- **b4 file B has NO durable row — the discriminator. FAILS pre-fix (A's later commit would flush B's
  residue), PASSES post-fix;**
- b5 a clean re-persist of B now succeeds;
- b6 B reads back as exactly the intended owner row (no garbage).

This fills the gap the K9 note named: the pre-existing K3 forced-fail leg fires BEFORE staging, so a
true mid-staging failure was unwitnessed until now.

## Doc rider (K8c, Peter 2026-07-17)
`docs/SECURITY.md` §K8c: the "open ruling" framing is dropped — deleted-object snapshot read is RULED
**fail-closed, final**; the K8 CoW DESIGN doc's weaker "owner-only fallback" is superseded, not a
pending alternative. Stamped at consequence (c) and reconciled at the K8b-ledger "must settle" line
(now "SETTLED"). `docs/MILESTONES.md` deleted-object "Flagged" note updated to match.

## Gate results (verbatim)
`./arroyo check`:
```
✅ x86_64 OK
✅ aarch64 OK
```
`./arroyo kernel8-test 35` — `mbench.py --spec pi4-regression.spec`:
```
✅ MBENCH PASS — 46/46 required witnesses, 0 forbidden hit(s), 189 lines scanned
```
K9 witness line (verbatim):
```
:: K9-parity: staged ACL persist mid-staging-failure discard — no partial-durable row + later commit
   carries no discarded residue PASS (fail-closed; A intact; B absent post-reboot; volume recovers) [w=0x7f] ::
```
All prior K-witnesses PASS UNCHANGED (K1-persist/K2-liveenf/K3-revoke incl. forced-fail `-EIO`/
K5-lockspan control+fix/K6-migrate/K8a/K8b/K8c); CAPSTONE 6/6; F2/F3 locked 240000/240000.

`./arroyo test-arm 22`:
```
xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<
```

## Metal ledger (accrues to the next Pi sitting — NOT this arc's gate)
- K9-PARITY `K9-parity [w=0x7f]` on silicon (the discard is QEMU-observable via bit 4; the CoW
  power-cut-leak equivalence is design-level, no separate metal step required, but the witness should
  ride the next boot's battery for record).
- Carries with the standing K9-MASKCUT metal item (after-flips=1 on real SD; masked-window vs K5B
  before-pair) — same sitting.

## Lane / scope
Changed: `unaos/crates/kernel/src/fs/unafs.rs` (aarch64-only kernel module — `MOUNT_DISCARD` +
`request_mount_discard` + the `with_unafs` discard + the `TEST_FAIL_MIDSTAGE` injection),
`unaos/crates/kernel/src/arch/aarch64/syscall.rs` (the witness + launcher), `pi4-regression.spec`,
`docs/SECURITY.md`, `docs/MILESTONES.md`. **`libs/fs/unafs` READ-ONLY (untouched).** No shared
kernel-core file touched. Zero x86 (module aarch64-gated).

## Commit (hw-pi4)
- `326e592` fs/acl: K9-PARITY — close the K9 lens-B mid-staging-failure residual in-lane
