# PREFSKERNEL — the kernel store runs Principia's rule, not its own (rmbp-ledger B345; SR32 hand-back; B337)

Branch `exec-rmbp-prefskernel`, cut from `b8eeb37d`, SETTINGSBUS (`exec-rmbp-settingsbus`, B337) merged first.

## Design

**Finding.** PRINCIPIA2 (SR32) put the schema, the one validator (`prefs_core::schema::check`) and the
shared PREF_GET/SET/LIST fulfiller (`prefs_core::wire::fulfil`) in the `no_std` core. The kernel's
`prefs.rs` still had a body of its own for each: `prefs::set` stored whatever it was handed (the only
clamp was brightness, at the Settings call site), and `prefs::bus_fulfil` re-implemented the three
verbs. That is two implementations of one rule, which R79 forbids. The gate scan also saw only the
kernel's `mod key`. Five kernel keys were read with no schema row: `system.audio.amp_holdoff_ms`
(`drivers/hda_amp.rs`), `vein.endpoint`, `vein.tls` and `vein.key_file` (`lumen.rs`), plus the
`user-prefs` demo table's five keys with no namespace.

**The seam (shared-core).** `prefs_core::wire` gains `Persist` (a tree you can lock plus a save) and
`persisted_set`. This is the kernel store's whole write sequence: name, then `schema::check`, then the
tree; an unchanged value is not re-saved; a failed save rolls back. `TreeStore` (the reference) and the
kernel's store both run it, so the rule has one body. The kernel's `prefs::set` is `persisted_set` over a
`Persist` whose lock is `TREE` and whose save is the USERS.DAT swap. `prefs::bus_fulfil` is
`wire::fulfil` over a kernel `Store` that calls `prefs::set_applied`. Load runs
`schema::clamp_tree` once.

**Milestones.**
- M1: `prefs::set` clamps through `schema::check` and stores the clamp. `[prefs] set` and the verb-19
  PrefChanged body (`wire::changed_body_applied`: `… NUL clamped=true` when clamped) carry it. The
  Settings brightness clamp is deleted.
- M2: `bus_fulfil` is a thin call into `wire::fulfil`. Host test
  `prefs_core/tests/kernel_store.rs` runs PRINCIPIA2's 26-step script through `TreeStore` and through
  a kernel-store shim (`persisted_set` over a RAM `Persist`, the same function the kernel calls) and
  checks the bytes are equal. It also covers save-count and rollback.
- M3: load clamps out-of-range file values once (`[prefs] load clamped=<n>`; re-saved when n > 0 and
  the file was not refused).
- M4: the schema gate covers the kernel. Kernel-shaped references `prefs::get("<ns>", "<key>")`, closures
  over `prefs::get("<ns>", k)`, `prefs::int|peek_int|flag|text(CONST, …)` with an in-file
  `const …: &str`, and R3PREF bodies, plus the `user-prefs` table rows. Four schema rows are added
  (29 total). The `user-prefs` table drops its five drifted keys for schema keys.
- M5: `tests prefs` reads `… loadclamp=1 clamp=1 wire=shared schema_rows=29 -> PASS`.

**Witness.** `:: PREFS-FIXTURE: codec=1 types=1 file=1 temp_gone=1 malformed_refused=1 save_held=1
restored=1 loadclamp=1 clamp=1 wire=shared schema_rows=29 -> PASS ::`, plus `[prefs] load clamped=<n>` on
each load.

**Stays owed.** The host Principia load-clamp (its TOML reader is its own `parse_document`, outside this
kernel arc). Ring-3 delivery of verb 19 (BANDY-3). The VFS swap, which a host test cannot reach: the
shim's save is RAM.
