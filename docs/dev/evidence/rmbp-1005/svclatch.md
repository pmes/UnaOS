# SVCLATCH (rmbp-ledger B462) — PERFREVIEW F6/F7/F8 + F3's SESSIONGEN

Cut from merge18 @f4e0613c. PERFREVIEW's branch (exec-rmbp-perfreview) is NOT on this tip: its F3 patch
(384db2e6, `prefs::user_name_in`) is cherry-picked here as the base SESSIONGEN builds on; its `[perf]` line
(d85a863e) is not — the before/after numbers come when both are folded.

## Finding (from docs/dev/review/PERF-2026-10-06.md, on exec-rmbp-perfreview)
- F8: six desktop service passes (player, textedit, facet, fileview, launcher, attrcols) take a spin lock on
  every pass to find an empty latch (nine mutexes in all, two in player/facet/fileview each).
- F7: `wm::service_damage` takes the window-table lock (IRQs masked) on every flush to scan for `damaged`.
- F6: `composite_inner` allocates `paint`, `dirty`, `bands`, `order` and the `seed` copy on every pass.
- F3 follow-up: prefs, Settings (twice) and login items read `whoami` under its lock and compare it to a
  locked `LOADED_FOR` on every pass, to find a session that changes twice per login.

## The seam
Kernel plumbing of the window manager's service chain (no handler store is touched; CHARTER `Kernel — wm`).
QUERYFOLDER's counter model: the quiet pass is one atomic load.
- SVCLATCH: `video/svclatch.rs` `Latch` (an AtomicBool posted by the producer AFTER it stores under its lock;
  the pass swaps it and only then locks). One latch per mutex; a producer that races the drain leaves the
  flag up, so the worst case is one extra (counted) empty lock, never a lost request.
- DAMAGEGEN: `wm::DMG_GEN`, bumped by `TableGuard::deref_mut` — EVERY mutable use of the table, a superset of
  the five `.damaged = true` sites, so no damage path can be missed (behaviour-neutral by construction).
  `service_damage` walks only when the generation moved since its last empty walk.
- COMPSCRATCH: `wm::Scratch<T>`, the `RowsSnap` pool shape for the four pass buffers (`paint`, `dirty`,
  `bands`, `order`) and the `seed` copy; a pool buffer keeps its capacity, so a steady pass allocates nothing.
  The early return before the snapshot is NOT taken (the pass does more than windows; not provably neutral).
- SESSIONGEN: `users::SESSION_GEN` bumped under `SESSION_LOCAL` at login and logout; `prefs::SessionSeen`
  keys each consumer's last compare by (generation, its own forget epoch), so a quiet pass is two loads.

## Milestones
M0 F3 base (cherry-pick). M1 SVCLATCH latches. M2 DAMAGEGEN. M3 COMPSCRATCH. M4 SESSIONGEN + `tests svclatch`.

## Witness (`tests svclatch`, R80: never at boot)
`:: SVCLATCH: passes_idle_locks=0 comp_alloc_per_pass=0 damage_gen=ok -> PASS :: opens=<n> comp_passes=<n>
comp_grows=<n> heap=<n> dmg_walks=<n> dmg_skips=<n> session_gen=<n> ::`
The flight's measure is PERFREVIEW's `[perf]` line: `svc_idle_us` and `frame_us` before (flight 24/25) and after.

## Owed
- player's `STATE.try_lock()` when no player window (an idle lock, not a latch; left as it was).
- F6's early return when nothing is damaged (needs a read of every pass tail it would skip).
