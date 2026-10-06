# RINGLOGIN (rmbp-ledger B465) — the Holocron ring opens with the login

**Finding** (BTKEYSEAL B446, flights 24/25): Holocron's ring opened only on a typed `holocron unlock`, and
`keyring::after_login` started HOLOCRON.ELF only when a ring file already existed. Flights 24/25 read
`ring=none`: no user ever had a ring, so every sealed record (a Bluetooth bond first) stayed RAM-only and a
sealed bond could not reconnect before a typed unlock. The Mac's keychain opens with the login password.

**Seam** (R79): the kernel is NOT a second Holocron. `holocron_core` stays the one ring implementation;
the kernel is the login's half of a door:

1. `users::login` (the one path that holds a verified password; it runs on the `login-submit` worker core,
   never the render core) calls `keyring::ring_login(name, pw)`. The kernel reads the ring HEADER only
   (`holocron_core::format::parse_ring` on `<home>/.holocron/.ring`, or a legacy root's), and derives the
   ring key ONCE with `keyring::kdf` — SYS_KDF's body, the exact Argon2id call `CryptoCore::derive_key`
   makes — at the header's salt and parameters (open), or at a fresh kernel-entropy salt and una-abi's
   `WINDOW2_KDF_*` (= HOLOCRON.ELF's `METAL_KDF`) when the user has no ring (create). The password is
   dropped on return; the kernel keeps only the KDF's 32-byte output, in one take-once slot (the DOOR).
2. `SYS_RINGKEY` (67, x86, `lumen`): HOLOCRON.ELF — and ONLY the process registered as Holocron's bus
   fulfiller, running as the door's user — takes the door (the slot is wiped as it is copied out) and
   reports the outcome back. The kernel prints the witness on the report.
3. HOLOCRON.ELF polls the door at its start and on every idle pass of its loop, and applies it through
   `holocron_core::service::Holocron::unlock_with_key` → `Ring::create_keyed` / `Ring::unlock_keyed` —
   the SAME code `create` / `unlock` now call after deriving (one ring implementation).
4. Lock: Log Out ends HOLOCRON.ELF (SECLOGIN M3) and wipes the door; the lock screen posts a LOCK on the
   door, which HOLOCRON.ELF takes on its next idle pass (no kernel spin on the input path); the screen's
   unlock opens the ring again through the same derivation (`Job::Unlock` on the worker).
5. `after_login` starts HOLOCRON.ELF when the user has a ring OR the door holds a create.
6. `holocron unlock <pw>` / `holocron init <pw>` stay for the console.

A user created before the fold gets the ring at the first login after it (the password is typed then).
A ring whose password differs from the login's (made by a typed `holocron init`) stays locked:
`ring=refused reason=bad-password`, and `holocron unlock` opens it.

**Milestones**: M1 holocron_core keyed create/unlock + host test; M2 una-abi SYS_RINGKEY and the kernel
door (`keyring.rs` tail, `sys_ringkey` at the syscall.rs tail, `bus_route::fulfiller_row`, the
`users::login` / `login::lock` / logout / screen-unlock calls); M3 HOLOCRON.ELF takes the door;
M4 `tests ringlogin`; M5 SECURITY.md row.

**Witness** (no password, no key, no salt on the wire):
- kernel at login: `[holocron] door kdf=kernel mode=<create|open> m_kib=49152 t=3 p=4 ms=<n> -> posted`
  (or `door none reason=<home-not-unafs|ring-unreadable|kdf-enomem|...>`)
- on HOLOCRON.ELF's report: `[holocron] ring=<created|opened> at=login user=<u> ms=<n>` (ms = login → ring
  open), or `[holocron] ring=refused at=login user=<u> status=<s>`
- lock: `[holocron] door lock reason=<lock-screen> -> posted`, `[holocron] ring=locked at=<lock-screen>`
- `tests ringlogin`: `:: RINGLOGIN: ring=open at=login reconnect_before_unlock=ok typed_unlock=0 -> PASS ::`

**Owed**: aarch64 (no SYS_RINGKEY arm; HOLOCRON.ELF is x86 today); a password CHANGE re-keys the ring
(today the ring keeps the old password until `holocron` re-wraps it — the next arc); metal unflown.
