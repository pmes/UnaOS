# Multi-user login — the design (SECLOGIN, 2026-09-22)

RULINGS R62 (Peter, 2026-09-22): *"where are we with having me log in to my home dir? make sure
we're ready to go on all that I just mentioned, but let's really hone in on true secure multi-user
login."* This file is the design the SECLOGIN arc builds against. It is written BEFORE any gate
(LAWS §4: the design closes first), and every claim below about the tree is a measurement at
`86f33c93`, the LOGINFLOW fold, taken with the command that measured it. The mechanism as it stands
is `permission_model.md` §5; the "true secure" gap list this file answers is rmbp-ledger B157's
status cell, which re-measured orin's A91 at that tip.

## 1. Threat model, in this kernel's own terms

Three actors, each named by what they HOLD, because that is what decides what the kernel can promise
them.

**Whoever holds the card.** The boot volume is a FAT volume on removable media (a card or a stick on
every board this kernel boots from) and it carries `/USERS.DAT`. With the card in another machine the
attacker reads every byte of it. The kernel promises: no plaintext password exists anywhere (not on
disk, not on the wire, not in a fixture line), and after M1 each guess against a row costs a
calibrated PBKDF2 run (~250 ms on the rMBP's own CPU at creation time) rather than one SHA-256. The
kernel does NOT promise confidentiality of the user roster, the salts, the iteration counts or the
home paths, and it does NOT promise anything about the files in `/home/<name>` — FAT carries no owner
attribute and nothing is encrypted at rest. That is owed to a hardware-secret arc, and the honest
sentence about the 2012 rMBP is that it has no TPM and no Secure Enclave: the only hardware secret
available would be one derived from the machine itself (a serial, a MAC), which is not a secret
against anyone who has the machine. Encryption at rest on this board is therefore a password-derived
key (the same PBKDF2 output, a second block) over a per-user volume, with the property that losing the
password loses the files. §4 says what that arc needs; this arc does not build it and says so.

**Whoever sits at the glass.** The login screen is the first thing on the panel and it is modal to
the pointer and the keyboard (SO36/SO44, SESSGATE and LOGINFLOW). The kernel promises: nothing launches
under no principal before a session exists; a wrong name and a wrong password are one indistinguishable
refusal on the glass and on the wire; and — after M1 — a wrong guess costs the same time whether or not
the name exists. It does NOT promise that the roster is hidden: the user rows are on the pre-session
glass by design (the Mac model, LOGINFLOW M1), and M6 makes that a stated policy predicate rather than
an accident. Whoever sits at the glass and knows a password is that user; there is no second factor.

**Whoever runs a program in a session.** A program launched while a session is open is stamped with
the session's principal at load and the stamp is qualified by the session epoch (SO37). The kernel
promises: the program reaches what the user owns and nothing another user owns; when the session ends
the program ENDS (M3 — until this arc it merely lost its stamp and kept running); and no program in any
session can open the credential file (M4). It does NOT promise isolation between two programs of the
same user beyond what the per-slot `(asid, gen)` ACL already gives (a program's own private creates),
and it does not promise anything about a program's memory once it has been killed (no scrub of freed
user pages is claimed here; `free_user_space_by_cr3` is what it is).

What the kernel promises NOBODY: that `/USERS.DAT` is tamper-proof. Whoever holds the card can
rewrite it — delete a row, plant a row with a known hash, reset an iteration count to the floor. CRC-32
detects damage, not malice. A signed or MAC'd credential file needs a key the attacker does not hold,
which is the same hardware-secret gap as encryption at rest, and this design does not pretend a CRC is
that key.

## 2. The one identity rule, for both arches

**A user's identity is a `uid`: a 32-bit number allocated from a counter in the credential file that
only ever increases, never reused, persisted in the file's header (`next_uid`) and in each row.** The
slot a row occupies in the 8-row table is storage, not identity. On both arches the ACL compares the
`uid`.

- **x86** keys its ACL by `(slot, gen)` and carries the user as a u32 in `SESSION_USER` /
  `SLOT_USER[slot]` / `OWNED_USER[nameid]` (`arch/x86_64/syscall.rs:23820-23826`). Those three carry
  the `uid` after M2; the type does not change, so every fold is same-line. `owned_user_ok` (`:23909`)
  compares `uid`s: `u != 0 && u == OWNED_USER[nameid]` is the same line with a different meaning for
  the number, which is exactly why the rule has to be written down — the code cannot show which
  number it holds.
- **aarch64** carries the user as a `PrincipalRecord` of kind `PRIN_USER` whose value is the canonical
  string, compared by whole-record equality everywhere (`owned_access_ok`, the K4 codec, the
  `UNAFS.ATR` rows). The canonical string becomes **`user:<name>#<uid>`** — `#` is not in the name
  alphabet (`[a-z0-9_-]`, `name_ok`), so the split is unambiguous; `5 + 8 + 1 + 10 = 24 <= 30`, inside
  the value field. A persisted owner row therefore carries the uid, and a recreated user of the same
  name (a new uid) presents a different principal and is refused by the existing equality.

**Why the name loses.** With the name as identity, a user deleted and recreated under the same name
inherits everything the old one owned. That is the recycled-slot hole one step over — anyone who can
create users can claim any deleted user's files by choosing the name — and it is precisely the
property POSIX uids exist to refuse. The name is the thing a person types; it must be reusable
(Peter deletes `una` and creates `una` again) without the second `una` being the first.

**Why `(id, gen)` as a pair loses to a single counter.** The brief offered `(slot, gen)` with `gen` a
per-row creation counter. A single monotone counter IS that pair with the slot dropped: the slot adds
no information once the counter is unique, it fits the u32 the x86 tables already hold (no widening,
no same-line fold that changes a type), and it reads on the wire as one number (`uid=3`) rather than a
tuple a reader has to decode.

**What `uid` is NOT.** It is not the session: the same user logging in twice presents the same uid
(`/home/<name>` keeps working across logout/login, the property SO37 preserved by keeping the epoch
out of the record). It is not secret. It is not stable across a reformat of the credential file — a
fresh `/USERS.DAT` starts its counter at 1 again, and old owner rows on a unafs volume written under
the previous file then name a uid that is about to be reissued. The rule for that case: a rebuilt
credential file is a new domain, and the owner rows of the old one are orphaned by the aarch64
mount-time rebuild only if the `UNAFS.ATR` volume binding still matches (it does; the binding is the
volume's serial and cluster count, not the credential file's). This is the one place the design is
weaker than a POSIX system with a persistent uid namespace, and it is stated rather than hidden: the
remedy is to seed `next_uid` from the volume rather than from 1 (a random 24-bit base from `rand.rs`,
M5), which this arc adopts — a fresh store starts at a random uid, so a reissue after a reformat is
improbable rather than certain.

**The migration of the principal string.** No flight has carried a session (`awk
'index($0,"[users]")'` over the flight-11 capture = 0 lines; every `[users]` line that exists is a
QEMU capture), so no `UNAFS.ATR` row on any card carries a `user:<name>` owner. A row that did would
match no live principal after M2 and would be OWNED-BY-NOBODY: refused to every caller, never public.
Fail-closed, and the case is theoretical.

## 3. The credential file, v2

### 3.1 Format

```text
header (32): magic "UNAUSR1\0" | ver u8 = 2 | count u8 | seq u16 LE | next_uid u32 LE | reserved[12] | crc32 LE over [0..28)
row   (128): name_len u8 | name[24] | home_len u8 | home[32] | salt[16] | hash[32]
             | uid u32 LE @106 | iters u32 LE @110 | kdf u8 @114 | reserved[5] | crc32 LE @120 over [0..120) | pad[4]
```

The magic is the FILE's identity and stays; the `ver` byte is the LAYOUT's and goes to 2. The v1
header was 16 bytes with its CRC at 12; v2's is 32 with `next_uid` in the space and its CRC at 28, so
a v1 reader refuses v2 on `ver` (as its doc promised) and a v2 reader parses v1 by `ver`. Row offsets
0..106 are v1's exactly; v2 appends the three new fields in v1's pad and moves the CRC to cover them.
`kdf` is 1 for the legacy `sha256(salt || password)` and 2 for PBKDF2-HMAC-SHA256; `iters` is the
row's own count, so a row hashed on the rMBP and a row hashed on a slower box verify at their own
cost. A v2 row with `kdf = 2` and `iters` below the floor (`PBKDF2_ITERS_MIN = 10_000`) is a BAD ROW
and refuses the whole image, exactly as a bad CRC does: a floor that only applied at creation could
be edited off the card.

### 3.2 Migration

There is no plaintext, so a v1 row cannot be re-hashed at load. The rule: **a v1 image is ADOPTED into
RAM as v2** — each row gets `kdf = 1`, `iters = 1`, `uid = its index + 1` (the number the x86 tables
carried for it, so a live session's stamps read the same), and `next_uid = count + 1` — and **the file
is rewritten as v2 at the first flush**, whichever comes first: a user's next SUCCESSFUL login
(`login`, which holds the password, re-hashes the row to `kdf = 2` at the calibrated count, and
flushes — `[users] rehash user=<n> v1->v2 iters=<n> ms=<n>`), or any create or delete. The invariant
that matters — no uid is ever reissued — holds in RAM from the moment of load, because `next_uid` is
set at adoption; the disk catches up at the first write through the existing `USERS.NEW` swap path,
which is already atomic (temp written, read back and parsed, then renamed over the live leaf).

`verify` stays pure (it is what the screen asks first, LOGINFLOW M2) and never writes; the migration
lives in `login`, the one path that has a verified password in hand. Until a v1 row migrates it is
distinguishable from a v2 row by timing (one SHA-256 versus a PBKDF2 run). That reveals the row's
VERSION, not its password, it is temporary, and it is stated.

### 3.3 Verification is constant-time end to end

`digest_eq` (a fold over all 32 bytes, no early return) already exists. What M1 adds: an unknown name
runs the SAME KDF against a fixed dummy salt at the table's calibrated count and compares against a
digest that cannot match, so "no such user" costs what "wrong password" costs. Without this, the
one-answer denial LOGINFLOW built on the glass would leak the roster through the clock.

## 4. Per gap: the fix, its wire, its fixture, its go-red, and which legs prove it

| # | Gap (B157, measured at 86f33c93) | Fix (milestone) | Wire witness | Fixture and go-red | Proves on |
|---|---|---|---|---|---|
| 1 | `users.rs:205 password_hash` is one SHA-256 per guess | **M1** PBKDF2-HMAC-SHA256 on `hash::sha256`; count calibrated at first use to ~250 ms and stored per row; migration at the next successful login | `[users] kdf calibrated iters=<n> ms=<n>` once; `[users] rehash user=<n> v1->v2 iters=<n> ms=<n>` per migrated row; `:: LOGIN-HARD: iters=<n> ms=<n> v2_rows=<n> migrated=<n> … -> PASS ::` | The fixture writes a v1 row for a scratch user, verifies it through the legacy path, logs in (migrates), verifies again as v2, refuses a wrong password, deletes the scratch user. Go-red: `calibrate` mutated to answer 1 → `create_user` refuses `reason=weak-kdf` and the wire quotes `iters=1 floor=10000` | x86 lane (`loginst`); `test-arm` compiles and runs the same store code |
| 2 | `/USERS.DAT` readable with no session by construction; nothing encrypted at rest | **M4** the two leaves are kernel-owned: no EL0 open may resolve them, wherever in the tree they are named; the users service keeps reading through `locate_in_dir(0, leaf)`, which is not the EL0 resolver. Encryption at rest NOT built; §5 names what it needs | `[users] open REFUSED leaf=USERS.DAT reason=kernel-owned` from the fixture; the resolver's own errno is `-EACCES` | The fixture opens `/USERS.DAT` and `/USERS.NEW` through the real EL0 resolver as a session-stamped slot and expects the refusal; go-red: the predicate answers `false` → `resolver=OPENED -> FAIL` | aarch64 via `open_locate` (the same seam `home_acl_fixture` drives); x86 via `fs::vfs::el0_locate`, which is the one shared resolver and is OUTSIDE this arc's file grant — see §6 |
| 3 | x86 `owned_user_ok` compares a recyclable users-table id; aarch64 compares the name — two arches, two rules. ⚠ LATENT at 86f33c93: `grep -n 'fn delete_user' fs/users.rs` = 0, so no slot can be recycled until a delete exists | **M2** the `uid` rule of §2 on both arches, and `delete_user` lands in the same milestone so the hole never becomes reachable | `[users] delete user=<n> uid=<n> (slot <i> freed; uid never reissued)`; `:: LOGIN-IDENT: … same_slot_refused=true same_name_refused=true owner_ok=true -> PASS ::` | Create A (owns a row), delete A, create B (lands in A's slot, new uid) → B refused; recreate A (new uid again) → refused; the live owner admitted before the delete is the control. Go-red: the uid allocator mutated to `slot + 1` (v1's rule) → `same_slot_refused=false -> FAIL` on BOTH arches from one mutation | x86 lane AND `test-arm` (`UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_FATIMG=sf`) |
| 4 | Log Out kills stamps, not processes | **M3** `session_logout` walks the process table for every running row whose slot carries a user stamp of the closing epoch, closes its windows (`wm::close_owner`) and kills it (`bg_kill`, the metal-proven path the close box already takes), THEN bumps the epoch and drops the stamps | `[users] logout epoch=<n> ended=<n> windows=<n>` | x86: the fixture opens a session, launches `STAT.ELF` through `spawn_user_image_bg` (the desktop's own launcher), logs out, proves the pid is gone from the table and the owner holds zero windows. Go-red: the kill skipped → `ended=0 pid_gone=false -> FAIL` | x86 lane; aarch64 twin compiled by `check` and run by `test-arm` as a regression (its own leg is the walk over scratch slots) |
| 5 | The epoch is `AtomicU32` | **M5** `SESSION_EPOCH` and `SLOT_EPOCH` to `AtomicU64` on both arches, `session_epoch() -> u64` | `[users] logout epoch=<n>` unchanged in form | The type change is proven by `check` on both arches; the `LOGIN-EPOCH` legs read the new type | both arches |
| 6 | The salt is `sha256(cycles ‖ name ‖ seq)` with no entropy source | **M5** `rand.rs`: RDRAND where CPUID.01H:ECX[30] says so (retry ten times, then refuse — Intel's discipline), RNDR where `ID_AA64ISAR0_EL1[63:60]` says so (retry on Z, then refuse), else a timer-jitter sampler folded through SHA-256; the source is printed once at first use | `[rand] source=rdrand\|rndr\|jitter probe=<what the CPU said> bits=256`; `:: LOGIN-RAND: source=<s> distinct=true salts_differ=true -> PASS ::` | Two draws differ; two rows' salts differ; the source named. Go-red: the jitter sampler mutated to a constant → `distinct=false -> FAIL` (the QEMU lane runs `-cpu qemu64,+x2apic`, which has no RDRAND, so the mutation bites there). The source FLIP is measured with the builder's existing `UNAOS_CPU=qemu64,+x2apic,+rdrand` override — no new knob — and reads `source=rdrand` | x86 lane (jitter by default; rdrand under the override); `test-arm` on `cortex-a72` reads `jitter` (no RNDR on A72); the rMBP's Ivy Bridge reads `rdrand`; the Pi 4 (A72) and the Orin (A78AE, ARMv8.2) read `jitter` |
| 7 | The user rows put names on the pre-session glass | **M6** `login::user_rows()` consults a documented policy predicate; default unchanged (the Mac model) | none — a policy, not a behaviour change | `LOGIN-CONTROL`'s `rows=` term keeps reading the store's count under the default | x86 lane |
| 8 | `ensure_home` prints `volume=el0-fat`, the role, not the device | **M6** the line carries the mounted volume's serial (`FatFs::volume_fingerprint`), so a flight capture tells the card from a stick without cross-reading the block registry | `[users] home=/home/<n> created volume=<serial-hex>` | The x86 replay's `[users] home=` line carries an 8-hex-digit serial; the `:: LOGIN:` verdict's `volume=` stays the fixture's own field | x86 lane |

## 5. What stays out of scope, and why

- **Encryption at rest.** Needs: a per-user volume (or a per-user directory on a unafs volume with a
  keyed extent layer), a key derived from the password (a second PBKDF2 block, never the credential
  hash itself), an AEAD (this kernel has SHA-256 and CRC-32 in `hash.rs` and nothing symmetric), and a
  decision about what happens to a program's files when the user logs out (the key leaves RAM; the
  program is ended, M3, which is a prerequisite). It is a separate arc; this file is its threat model.
- **A second factor, lockout after N failures, password policy.** Lockout is a denial-of-service
  against the person at the glass by anyone who can type; the calibrated KDF is the rate limit this
  design chooses. Policy is Principia's (LAWS §9) and is not decided in-session.
- **A `deluser` shell verb and an "account settings" screen.** `delete_user` exists after M2 because
  the identity rule cannot be proven without it and because a delete with a recyclable identity is
  the hole; the verbs that expose it to a person are a queue item, not this arc.
- **The aarch64 users service site.** `fs::users::service()` has three call sites and none is
  reachable on a Pi or Orin image (B157: `main.rs:1214/1704` sit inside `usbdebug` blocks, `:5965`
  inside `x86_usb_pump`), so the login battery runs on x86 and on `test-arm` only. `main.rs` is not in
  this arc's grant; the one-line tegra-loop site A91 named remains owed there.

## 6. The one file this arc may not touch, and the exact change it needs

M4's refusal has one right home on BOTH arches: `crate::fs::vfs::el0_locate` (`fs/vfs.rs:2993`), the
shared resolver every EL0 open walks — aarch64's `sys_open` through `open_locate`, x86's
`sys_open_dynamic` through the storage service task's `resolve_path`
(`drivers/xhci/irqstorage.rs:437`). `fs/vfs.rs` is outside this arc's file grant (EXECUTOR-BRIEF rule
3), so the arc lands the predicate (`users::kernel_owned_leaf`), the aarch64 path-form guard beside
the existing `UNAFS.ATR` guard in `open_locate` (the K1 M4 precedent: an up-front cheap check and a
path-form check a spelling cannot slip past), the fixture, and the design — and STOPS on the x86 half
with the exact patch for the seat, quoted in the arc's report. Until that line lands, an x86 program
with `irqstorage` armed can open `/USERS.DAT` read-only through the dynamic on-disk arm; nothing in
that arm can write it (`sys_write_file`'s dynamic branch is overwrite-only on a file the caller
opened RW, and the users service replaces the file by rename, so an overwrite lands on a dead
directory entry at worst) — but reading the roster and hashes from ring 3 is exactly what M4 refuses,
and the report says so in those words.

## 7. Prediction for flight 12, stated so it can be falsified

Peter boots the card with `UNAOS_LOGIN=1` and no `loginst`. On the wire, in this order:

```text
[users] load volume=el0-fat(rw) src=none users=0 (fresh store)
[login] ignition desktop_up=true console_routed=… -> OPEN
[login] screen open window=1 box=450x284 at (…)
[users] kdf calibrated iters=<n> ms=<~250>            <- the first create, once per boot that creates
[rand] source=rdrand probe=cpuid.01h.ecx.30=1 bits=256   <- the salt's source, once per boot that creates
[login] first user created id=<uid>
[users] home=/home/<name> created volume=<8 hex digits>
[users] login ok user=<name> id=<uid> principal=user:<name>#<uid>
[login] session open user=<name>
      … Quarry opens his home; files created inside are owned user:<name>#<uid> …
[users] logout epoch=2 ended=<n> windows=<n>          <- n = the programs he launched in the session
[login] logged out — screen returns
```

On the glass: the create-first-user form (no user rows, because the store is empty), his name in a
row on every later boot, the password field, the button, and after Log Out the same screen with his
row on it and every window of the session gone. A second login is `home=/home/<name> exists` with the
same serial. If any of these lines differ in ORDER, or `iters=` reads below 10000, or `source=` reads
`jitter` on the rMBP, the prediction is false and the row that owns the line is B169.

### 7.1 The aarch64 reading (ARMUSERS, rmbp-ledger B188, 2026-09-23)

Until B188 the aarch64 halves of M1-M5 were type-checked and never run: `test-arm` under
`login,loginst,virt_el0` booted to `:: CAPSTONE COMPLETE` with zero `[users]` lines. The GICv3 virt
boot (`UNAOS_GICV3=1`, which `virt_el0` needs) drops EL2 -> EL1 inside `kernel_main`'s `is_v3()` branch
and diverges into `run_capstone_boot_core`, before `arch::pci::init` and before the shared main loop
whose storage pass carries `users::service()`. `main.rs::virt_users_pass` is that storage pass, run
bounded at EL2 before the drop. The tegra console pump (`jd2_console_pump`) gets the per-pass call
orin-ledger A91 named, compile-proven only (no QEMU models Tegra234).

Measured on `env UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_FATIMG=sf
./arroyo test-arm 120` (QEMU `virt`, cortex-a72), the first boot on a fresh stick — selected lines,
in wire order (the full capture is quoted in B188's commit message):

```text
[rand] source=jitter probe=id_aa64isar0_el1.rndr=0 bits=256
[users] load volume=el0-fat(rw) src=none users=0 (fresh store) next_uid=16415479
[users] kdf calibrated iters=60606 ms=250 (probe=8000 took 33 ms; target 250 ms; floor 10000)
[users] home=/home/una created volume=24b4bba3
[users] login ok user=una id=16415479 principal=user:una#16415479
[users] logout epoch=3 ended=0 windows=0 (SO37: …; M3: …)
[armusers] virt storage pass serviced=true passes=185 ms=3643 block=up wall=60000 (…)
[login] ignition desktop_up=false console_routed=false -> HELD (SO43: …)
```

The second boot on the same stick reads `src=dat users=1 seq=12 ver=2`, and the home line
`exists` with the same serial. Every `loginst` verdict the lane can run is PASS (`LOGIN`,
`LOGIN-EPOCH`, `LOGIN-HARD`, `LOGIN-IDENT`, `LOGIN-KOWN`, `LOGIN-RAND`); `unaos/scripts/specs/arm-login.spec`
pins them. How this differs from the flight-12 prediction above, and why:

- **No screen.** The virt lane compiles no desktop (`desktop_firmware` is not armed there), so the
  SO43 seam HOLDS. The screen fixtures stay x86-only on QEMU; the Orin's render card is where the
  aarch64 screen runs.
- **`source=jitter`.** The QEMU CPU has no FEAT_RNG. An ARMv8.5 core with RNDR reads `rndr`.
- **`ended=0`.** No program is launched under a session on this lane; `LOGIN-END`'s launch leg is
  x86-only (`login_end_fixture`), and the aarch64 walk runs on every Log Out with nothing to end.
- **`LOGIN-KOWN` is a verdict here** (`resolver=refused errno=-13,-13`): the aarch64 `open_locate`
  guard refuses the credential file.

**The Orin's half is owed to a flight.** A card with `UNAOS_LOGIN=1` should print `[users] load
volume=el0-fat(…)` once its block device answers, before or after the screen's first open (the
screen's `submit` loads the store itself), and with `loginst` the same verdict lines as above. If no
`[users] load` line appears on an Orin login boot, the console pump never saw a block device, and the
row that owns the line is B188.
