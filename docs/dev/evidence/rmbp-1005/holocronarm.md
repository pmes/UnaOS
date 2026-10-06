# HOLOCRONARM (rmbp-ledger B484) — Holocron takes the door on the Pi and the Orin

**Finding** (read, not flown; flights 24/25 are rMBP-only): RINGLOGIN (B465) / RINGLOGIN2 (B479) gave aarch64
the door and `SYS_RINGKEY`, but nothing on aarch64 takes it. Four gaps, not one:
1. HOLOCRON.ELF is built for x86 only (`build_user_holocron_x86`); no aarch64 link script, no staging.
   (The brief's "LUMEN-ARM.ELF shape" does not exist: user-lumen is only TYPE-CHECKED for aarch64. The
   aarch64 ELF window does exist — RING3ABI2 M5, `arch/aarch64/xwin.rs`, VA `una_abi::USER_XWIN_VA_ARM`,
   image cap 12 MiB — so Holocron links there like LUMEN's aarch64 script; the 16 KiB classic window and
   the vug's page cliff (WIREDIET B461) do not apply to an elf-model image.)
2. aarch64 has no `SYS_PATH_READ`/`SYS_PATH_WRITE` (59/60) arm: Holocron's whole store is those two numbers,
   so even a running HOLOCRON.ELF could neither read nor write the ring. x86 arms them under `selfdiag`.
3. aarch64 has no `SYS_KDF` (66) arm (x86: `lumen`).
4. The create door's Argon2id memory: una-abi's WINDOW2 48 MiB. The aarch64 kernel heap IS 48 MiB
   (`allocator::HEAP_SIZE`), and both the kernel's login KDF and the ring-3 window's frames come from it,
   so a Pi login could only ever print `door none reason=kdf-enomem`.

**Seam** (R79): unchanged — `holocron_core` is the one ring; HOLOCRON.ELF the one fulfiller; the kernel is the
login's half of the door and the volume's fulfiller (`selfdiag::path_fulfil`, the SAME body x86 calls).
No second store, no kernel ring code.

**Milestones**
- M1 una-abi `RING_KDF_M_KIB/T/P`: the parameters a NEW ring is made at, per arch — x86 = WINDOW2's (48 MiB,
  t 3, p 4: no x86 change); aarch64 = 19 MiB (holocron_core's FLOOR memory, OWASP's minimum), t 3, p 4.
  `keyring::ring_login` (create) and HOLOCRON.ELF's `METAL_KDF` take them. An OPEN always uses the ring
  header's parameters, so a ring is portable across arches.
- M2 aarch64 kernel: `SYS_PATH_READ|SYS_PATH_WRITE` (`selfdiag`, as x86) and `SYS_KDF` (`lumen`) folded onto
  the `SYS_CLOSE` dispatch line before its `//`; bodies at the syscall.rs tail.
- M3 user-holocron aarch64: `user-holocron.ld` at `0x7840200000`, `build_user_holocron_aarch64` →
  `target/HOLOCRON.ELF` (e_machine b700, elf window), a USER_CHECK_MATRIX aarch64 row, staged as
  `APPS/HOLOCRON.ELF` by `kernel8` and `esp-jetson` when `lumen` is armed.

**Card line (Pi)**: the door needs `lumen,login,busreg,unafs` and the store needs `selfdiag` — x86's gate
line carries the same five. Pi: `UNAOS_LUMEN=1 UNAOS_LOGIN=1 UNAOS_BUSREG=1 UNAOS_SELFDIAG=1` plus its
UnaFS home (`door none reason=home-not-unafs` otherwise). No new knob.

**Witness** (existing lines; the Pi/Orin boot reads them):
- `[holocron] door kdf=kernel mode=create m_kib=19456 t=3 p=4 ms=<n> -> posted …`
- `[holocron] login user=<u> ring=unafs -> started pid=<p> slot=<s>`
- `[holocron] ring=created at=login user=<u> ms=<n>` (a second login: `mode=open`, `ring=opened`)
- `tests ringlogin` → `:: RINGLOGIN: ring=open at=login reconnect_before_unlock=ok rekey=ok arm=aarch64 … -> PASS ::`
- a heap that cannot lend 19 MiB says `[holocron] door none reason=kdf-enomem m_kib=19456 t=3 p=4`.

**Owed**: metal unflown (Pi and Orin); LUMEN.ELF for aarch64 (same window, its own arc); `selfdiag` being the
switch for ring-3 path I/O on both arches (a path-I/O fulfiller under its own feature is the seat's call).
