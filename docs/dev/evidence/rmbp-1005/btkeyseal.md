# BTKEYSEAL (rmbp-ledger B446, ARCHREVIEW F2) — Bluetooth link keys move into the sealed keyring

**Finding.** `drivers/ehci/bthid.rs` `store_write` kept each bond's 16-byte BR/EDR link key as a plain
`bt.linkkey` Blob attribute on `<home>/.config/unaos/bt/<addr12>`, readable by anything with read on the
home; whoever reads it impersonates the keyboard. Holocron exists with sealed records (HOLOCRON2 B355:
`APPS/HOLOCRON.ELF` fulfils verbs 144..=151 over `holocron_core`'s ring, Argon2id ring key, a fresh
HKDF file key and ChaCha20-Poly1305 per secret). The "until Holocron exists" in bthid's charter has passed.

**The seam (R79).** The kernel is a CLIENT of Holocron's bus verbs, never a second keyring: the record is
`bt/<addr12>` (kind `bt.linkkey`, data = the 16 key bytes), sealed and opened by HOLOCRON.ELF with the
session user's ring. The request is built with `holocron_core::wire::Request` (the one codec) and relayed by
`bus_route` from the kernel-client row (`prefs_client::KCLIENT_ROW`, SETTINGSBUS's row, which already
receives relay answers) stamped `user:<name>#<uid>` — the owner principal Holocron compares, built by the
kernel from the session table, never claimed. The non-secret bond facts (`bt.keytype`, `bt.class`,
`bt.name`, `bt.hiddesc`) stay typed attributes on the object; only the key moves.

**Behaviour.**
- Pairing (unchanged flow): the key is held in RAM as before; the storage pass writes the object WITHOUT a
  key attribute and asks Holocron SecretPut. Refused/locked/absent -> the key stays in RAM for the session,
  the seal is retried every 3 s (one witness per reason change). NEVER a plain write.
- Load: an object with no plain key -> SecretGet; found -> bonded; locked/absent -> pending, retried; a bond
  unsealed after the reconnect ran re-arms the reconnect (connected links are skipped).
- Migration: an object still carrying `bt.linkkey` is read ONCE (the bond is usable this boot), sealed, and
  the attribute removed only after Holocron answered OK; until then it is counted `plain_left`.
- Forget: unlink the object AND SecretDelete `bt/<addr12>`.

**Milestones.** M1 the client (`drivers/ehci/btkeyseal.rs`) + `prefs_client::relay_tag`; M2 bthid's store
seals/unseals/migrates/forgets through it; M3 `tests btkeyseal`, SECURITY.md row.

**Witness.** `:: BTKEYSEAL: seal <addr> -> sealed bt/<addr12> ::`, `:: BTKEYSEAL: migrate <addr> plain=read
sealed=ok plain_removed=ok ::`, `:: BTKEYSEAL: unseal <addr> -> ok ::`, `:: BTKEYSEAL: waiting reason=<r>
pending=<n> unsealed=<n> plain_left=<n> ::`; `tests btkeyseal` ->
`:: BTKEYSEAL: sealed=<n> plain_left=0 pending=<n> holocron=<state> codec=ok -> PASS ::`.

**Owed / named.** (1) Holocron opens only on `holocron unlock <pw>` and starts only when the user HAS a ring:
until then a new bond lives in RAM for the session and a sealed bond does not reconnect — unlocking the ring
with the login password at login is SECLOGIN/HOLOCRONROOT's, not this arc's. (2) The store root is the
kernel's `<home>/.config/unaos/holocron/` as today (F4, HOLOCRONROOT B448 decides one root; the record name
`bt/<addr12>` does not change with it). (3) `fs/holocron.rs` (`/HCRON/BTBOND.DAT`, BT-BOND M1, knob
`holocron`, default OFF) is a third plaintext bond store on FAT — not armed on the metal line, named for
deletion. (4) x86 only (bthid is x86 + `btc`); without `lumen`+`busreg`+`login` there is no Holocron and
bonds are RAM-only.

CHARTER of the new file: `drivers/ehci/btkeyseal.rs` — `//! CHARTER: Holocron — shared-core`.
