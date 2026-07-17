# K9-MASKCUT landing report — the ACL persist adopts the staged-batch shape (hw-pi4 track)

## Summary
The K7 metal sitting measured ~0.7 s of per-core IRQ-masked hold at the three fused ACL persist
sites (polled SD I/O across a journaled multi-sector write inside `with_unafs`). K5B unfroze the
NAMESPACE but ruled the per-core mask a LEDGERED residual whose "true closure is out of the pi lane
(crate batched-sync)". That crate machinery now exists (UNAFS-BATCH, main `50e8875`): autocommit-off
staging + ONE commit = one root flip.

K9 adopts that shape at the kernel ACL-persist choke point `native_acl_write_on` — through which ALL
three fused sites (`sys_fgrant_revoke_2phase`, `native_persist_grants`, `native_persist_grow`, plus
create/rename) funnel. The row's create/resolve + stale-grant removal + `name`/`fc`/`owner`/`grant`
rewrites are STAGED with `set_autocommit(false)` and landed with a SINGLE `commit()`, instead of one
root flip per attribute. This CUTS THE SECTOR/FLIP COUNT inside the K5B masked window — it does NOT
remove the window (the IRQ mask is `with_unafs`'s K4 keystone; the write is still polled/synchronous).

## Changes
Lane: `unaos/crates/kernel/src/fs/unafs.rs` + `unaos/crates/kernel/src/arch/aarch64/syscall.rs` +
docs. `unaos/libs/fs/unafs` READ-ONLY (untouched — the crate exposes the staging API used verbatim).

- `native_acl_write_on` split into a **scope-guard wrapper** + `native_acl_stage_row`:
  - `native_acl_stage_row` = the pre-K9 write body VERBATIM (create/resolve + stale-grant removal +
    `name`/`fc`/`owner`/`grant` `set_attribute` sequence). No change to WHAT is written. Owns every
    early return; returns `true` iff every op staged.
  - the wrapper: `set_autocommit(false)` → `native_acl_stage_row(...)` → `commit()` **only on full
    staging success** → unconditional `set_autocommit(true)`. No early return between the toggle and
    the restore.
- `nsspan_report` (syscall.rs) gains a second emit, `NS-SPAN-K9`, carrying worst flips/blocks per
  single ACL row persist (EOF atomics `ACL_PERSIST_FLIPS`/`ACL_PERSIST_BLOCKS` in fs/unafs.rs). The
  production staging change is ALWAYS-ON (not knob-gated); the flip/block capture is `nsspan`-gated.
- SECURITY.md §K1: the K5B residual entry gains the K9 note (residual NARROWED-AGAIN, CLOSURE-pending-
  metal). MILESTONES.md: hw-pi4 K9 entry.

## How the autocommit scope-guard works
The MOUNT is process-wide and cached (`with_unafs` reuses one instance until `force_remount`).
`set_autocommit` mutates that shared instance, so a leaked autocommit-OFF state would silently drop a
later writer's commit. The wrapper is written so autocommit is ALWAYS restored:

```
#[cfg(feature = "nsspan")] let _cs0 = fs.commit_stats(); fs.set_autocommit(false);
let staged = native_acl_stage_row(fs, ...);
let ok = staged && fs.commit().is_ok();      // commit ONLY on full staging success
fs.set_autocommit(true); #[cfg(feature = "nsspan")] { ...record flips/blocks... }
ok
```

- All early returns live in `native_acl_stage_row`, which returns a `bool`. The wrapper has NO early
  return between `set_autocommit(false)` and `set_autocommit(true)` — control ALWAYS reaches the
  restore. (The kernel is `no_std`/panic=abort, so there is no unwind path to leak past either; the
  only leak risk is a source-level early return, which the split structurally removes.)
- Production always ENTERS autocommit-ON: the default is ON, and the only other toggler (the K8a-cow
  witness) sets it OFF then `force_remount`s — it never nests an ACL persist inside its hold. So
  restoring to ON is the invariant, not a guess. (I did NOT add a crate autocommit getter — the crate
  is read-only, and none was needed.)

## Invariants — how each was kept (the review lenses attack these)
- **K5B fusion (single uninterrupted `with_unafs` hold):** staging lives ENTIRELY inside the caller's
  existing MOUNT hold. `native_acl_write_on` is called by every site INSIDE its one `with_unafs`
  closure; the split moved nothing across the hold boundary. The snapshot → staged write → in-RAM
  commit sequence is unchanged; only this write's internal flip count shrank. Proven UNCHANGED by
  `k5_lockspan_check` (control leg still resurrects with a decomposed hold; fix leg stays narrowed).
- **K3 durable-first (`-EIO`, in-RAM intact):** on staging FAILURE no `commit()` fires → no root flip
  → nothing durable changes → old row stands → caller `-EIO`, in-RAM grant intact. This composes with
  the brief's "a failed staged persist = no root flip = nothing durable changed". On SUCCESS the single
  commit is an ATOMIC row (a crash lands the old row or the whole new row, never a per-attribute
  partial — strictly better than the pre-K9 path). Proven by `k3_revoke_check` incl. the forced-fail
  `-EIO` leg (the K3 knob fires in `native_write_grant_row_on` BEFORE staging, so the forced-fail leg
  never enters the staged sequence — nothing to unwind).
- **`with_unafs` IRQ-mask / non-reentrancy (K4 keystone) + lock chain `NAMESPACE ⊃ MOUNT ⊃
  OWNED_FILES`:** untouched. No new `with_unafs` entry, no re-entry, no lock taken.
- **CoW commit atomicity as the durability boundary:** the change RESTS on it — the single commit is
  the atomic point. No pre-K8a journal-era sequencing assumptions were reintroduced.

## Gate results (verbatim)

`./arroyo check` (both arches):
```
✅ x86_64 OK
✅ aarch64 OK
```

`./arroyo test-arm 22`:
```
xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<
```

`./arroyo kernel8-test 30` — `mbench.py --replay ... --spec pi4-regression.spec`:
```
✅ MBENCH PASS — 45/45 required witnesses, 0 forbidden hit(s), 188 lines scanned
```
Key witness lines (all PASS, unchanged):
```
:: K1-persist: native unafs owner/grants SURVIVE REBOOT — rebuild+enforce PASS ... [w=0x3fff] ::
:: K2-liveenf: cross-reboot ACL LIVE via REAL programs ... rebuild+enforce PASS [w=0x7f] ::
:: K3-revoke: SYS_FGRANT revoke commit-ordering — two-phase durable-first PASS (revoke survives
   reboot; kept grant intact; forced persist-fail -> -EIO with in-RAM grant left intact,
   RAM/disk consistent) [w=0x7f] ::
:: K5-lockspan: native unafs revoke/re-persist SMP window — ... control ... production revoke+re-persist
   fused under ONE with_unafs hold stays narrowed (K5B); kept grant intact; create-gate not leaked
   PASS [w=0x3f] ::
:: K6-migrate: ... PASS [w=0xff] ::
:: K8a-cow: ... PASS [w=0x7f] ...   :: K8b-snap: ... PASS [w=0x7f] ...   :: K8c-snapread: ... PASS [w=0xff] ...
```

## M2 — before/after (QEMU, `UNAOS_NSSPAN=1`; TCG, NOT the verdict)
Both runs identical instrumentation; the BEFORE run is a measurement-only local revert of the staging
(autocommit-ON per-op), discarded via `git checkout` — the committed tip is the staged form.

| metric (worst per single ACL row persist) | BEFORE (per-op) | AFTER (staged) | reduction |
| --- | --- | --- | --- |
| root flips | 5 | **1** | 5× |
| blocks written | 35 | **19** | ~1.8× |
| masked `with_unafs`-hold — revoke (ticks @62.5 MHz) | 13,941,875 | 10,398,500 | ~25 % |
| masked `with_unafs`-hold — grants | 14,525,438 | 9,939,438 | ~32 % |
| masked `with_unafs`-hold — grow | 11,812,625 | 9,640,188 | ~18 % |

Emit lines (AFTER, verbatim):
```
:: NS-SPAN: K5B ns-hold across ACL persist = 0 (not taken — NAMESPACE unfrozen) BUT per-core IRQ-masked
   with_unafs-hold worst revoke=10398500 grants=9939438 grow=9640188 ticks (freq 62500000) — polled SD
   write still masks the holding core (ledgered residual) ::
:: NS-SPAN-K9: single ACL row persist (staged batch) worst flips=1 blocks=19 — one root flip per
   persist vs the pre-K9 per-attribute regime ::
```
Emit lines (BEFORE, verbatim):
```
:: NS-SPAN: ... worst revoke=13941875 grants=14525438 grow=11812625 ticks (freq 62500000) ... ::
:: NS-SPAN-K9: single ACL row persist (staged batch) worst flips=5 blocks=35 ... ::
```
The flip cut is exact and QEMU-provable; the tick drop (~25–30 % even under TCG — fewer barrier/flush/
root-flip cycles inside the mask) previews the metal win but is not the verdict.

**Metal (rides the next Pi sitting — NOT this arc's gate):** the after-flips=1 on silicon and the
real-SD masked-window number against K5B's metal before-pair `revoke=38077992 grants=38616777
grow=23392723` ticks @54 MHz. Each avoided flip is a polled root-sector write + barrier, so the 5→1
flip cut should translate to a materially shorter masked window on real SD.

## Flagged (finding — pre-existing, not introduced)
A mid-op I/O/`NoSpace` failure inside the staged sequence leaves uncommitted in-flight blocks on the
shared cached mount that a later persist's `commit()` would flush. The crate exposes NO public in-place
unwind: `txn_unwind` is private, and `create_files_batch` (the only self-unwinding public mutation)
cannot express create-or-replace-with-removal — so the brief's "reload from committed root" is not
expressible from this raw-op composition. The pre-K9 autocommit-ON path shares this EXACT class (a
failed op's own writes are equally left uncommitted-in-flight, since `set_attribute` uses `?` and never
self-unwinds); K9 does not weaken it, and the failure is an I/O/full-volume error (rare on the
dedicated tiny-ACL volume). Not a STOP: no invariant is weakened relative to base, and K3 durable-first
holds (failure changes nothing durable). True closure = a crate-side PUBLIC rollback primitive (a
`discard_uncommitted` / public `txn_unwind`), out of the pi lane — drop-box, exactly as K5B's residual
was routed.

## Commits (hw-pi4)
- `56ea9d4` fs/acl: K9-MASKCUT M1 — adopt UNAFS-BATCH staged persist at the ACL choke point
- (this report + docs) — M3
