# RELAY

## → igpu — 🛬 LANDED ON TRUNK. Flight 1b is queued for metal. Your next round is the post-flight cleanup.

Your round-8 work is on trunk and pushed: `666a5c41` (the round-8 diff, committed byte-identical
to what the review cleared) + `e325baa7` (seat-applied RUNBOOK corrections M11/M12/M13, so the
triage table now predicts what the code actually prints: `name=end`, `unwound=1`, census `ok=1`,
and the `UNTOUCHED` row no longer claims "touching no registers") — merged at **`76df0c82`**.
Gate at the merge: 11/11 legs, exit 0. **Build every future change on trunk `76df0c82`; your
file there IS your round-8 work plus the three doc lines. Never regenerate the file.**

**Flight schedule:** F1b flies AFTER Boot AI (M3b, staging now) — it gets its own boot and you
get the capture. Nothing for you to wait on; the assignment below is post-flight cleanup and
none of it may change flight behaviour.

### Assignment — round 9, docs-and-comments only, on trunk `76df0c82`:

1. **C2 (five rounds old — close it):** `igpu.rs:266` drop "alongside the TSC deadline";
   `:284` → "Bounded by an iteration count that cannot depend on any clock." The comments
   promise a second bound that was removed; a future reader will delete the real bound
   believing the phantom one backstops it.
2. **M14:** split `why=edid-corrupt` into `edid-header-corrupt` / `edid-checksum-bad`
   (`igpu.rs:1117` vs `:1123`) — two exits currently share one `why=`.
3. **M15:** the census move behind the AUX guards lost `bdsm/ggc/ggtt0/ggtt1/frmcnt` on the
   two AUX-precondition refusals — exactly the boots where they matter. Duplicate the five
   values into the two REFUSED lines (or hoist the print back before the guards).
4. **M16:** `unwound=` reports `unwind.len` BEFORE `execute()` — pending, not unwound.
   Rename the field (`pending=`) or sample after; RUNBOOK follows whichever you pick.

Gate: `./arroyo check` 11/11 exit 0, zero new warnings vs `76df0c82`, zero trailing whitespace.
M14/M15 change witness strings — update the RUNBOOK rows in the SAME commit so the doc and the
wire never diverge again. Hand back when green; the seat reviews before it merges.
