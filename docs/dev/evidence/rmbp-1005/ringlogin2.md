# RINGLOGIN2 (rmbp-ledger B479) — the ring follows the password, and the Pi and the Orin get the door

**Finding** (RINGLOGIN B465's owed list; flights 24/25 predate the door — their only `[holocron]` lines are
`tests holocron`'s `ring=none`): (a) aarch64's dispatcher has no `SYS_RINGKEY` arm, so a ring-3 Holocron there
gets `-ENOSYS` and the login's key dies in the door; (b) `passwd` writes the new credential and leaves the ring
wrapped under the old password, so the next login reads `ring=refused … bad-password`; (c) `user=` on the
`[holocron]` lines — no ruling forbids it: RULINGS R65 (LOGIN14) and SECURITY.md's PWWIRE rows forbid the
PASSWORD on the wire, and every `[users]`/`[login]` line names the user. `user=` STAYS.

**Seam** (R79): unchanged from B465 — `holocron_core` is the one ring implementation, HOLOCRON.ELF (the
registered fulfiller) is the only one that touches the ring files, the kernel is the login's (and now the
password change's) half of the door.

1. `holocron_core::ring::Ring::rewrap_keyed(ring_file, old, new, secrets, rng)` beside `create_keyed` and
   `unlock_keyed`: the OLD key must open the verifier; every secret is opened under the old key and resealed
   under the new; the ring file is rebuilt with the SAME salt, parameters and owner (a fresh verifier nonce).
   All in memory, all or nothing. `service::Holocron::rekey_with_keys(old, new)` gathers the secrets
   (`Store::namespaces` + `list` + `read`), calls it, writes every secret then the ring LAST; a failure before
   the ring write keeps the old ring (secrets already rewritten are put back from the bytes read).
2. una-abi: `RINGKEY_MODE_REKEY` (4); the door grows an `old_key` field (`RINGKEY_LEN` 72 → 104).
3. `passwd` (own row, not root): a third prompt stage FIRST — the current password, verified against the store
   (`bad-old-password` refuses the change at once, like the Mac). On commit, after the credential is written,
   `keyring::ring_rekey(name, old, new)` derives BOTH keys at the ring header's salt and parameters (SYS_KDF's
   body, twice) and posts a REKEY door; HOLOCRON.ELF is started if it is not running (it takes the door at its
   start). The passwords are dropped on return.
4. `passwd <name>` by the administrator (no old password): the ring is NOT re-keyed — one line at the change,
   and the next login's refusal prints once per boot with the way back.
5. aarch64: `SYS_RINGKEY` folded onto the `SYS_CLOSE` arm (code before the `//`), body at the file tail: the
   holder is `bus_route::fulfiller_row(VERB_UNLOCK) == (asid, ASID_GEN[asid])`; the user is the session's uid
   only when the caller's stamped principal (`slot_ppid_of`, epoch-qualified) IS the session's.

**Milestones**: M1 holocron_core `rewrap_keyed` + `rekey_with_keys` + `Store::namespaces` (four stores) + host
test; M2 una-abi REKEY + kernel `ring_rekey` + report witness + `passwd` old-password stage; M3 HOLOCRON.ELF
applies REKEY; M4 aarch64 arm; M5 `tests ringlogin` grows `rekey= arm=` + SECURITY.md row.

**Witness** (no password, no key, no salt on the wire):
- at the change: `[holocron] door kdf=kernel mode=rekey m_kib=<m> t=<t> p=<p> ms=<n> -> posted`, or
  `[holocron] ring=none at=passwd (nothing to re-key …)`, or `[holocron] ring=kept at=passwd reason=admin-reset …`
- on HOLOCRON.ELF's report: `[holocron] ring=rekeyed at=passwd user=<u> ms=<n>` or
  `[holocron] ring=rekey-refused at=passwd user=<u> status=<s> (the old ring is kept …)`
- `tests ringlogin`: `:: RINGLOGIN: ring=… rekey=ok arm=<x86|aarch64> … ::` (`rekey=` is holocron_core's
  rewrap on a RAM ring: old key opens, new key opens after, old key refused after, a wrong old key keeps the ring).

**Owed**: HOLOCRON.ELF is x86-only, so aarch64 has the door and the arm but no taker (the user-holocron
aarch64 build is its own arc); the Settings password form (SETTINGS2) asks no current password and so is an
admin-reset for the ring; metal unflown.
