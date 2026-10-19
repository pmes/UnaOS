# BP-security — principals, users, login, ACL, secrets, trust, Helm (read at 1719fe54)

## 1. What it is
- Model: capabilities first, POSIX-layerable; every program is a principal with explicit grants; handles are capabilities checked at one lookup point (`docs/SECURITY.md:54-70`).
- A human user IS a principal (`user:<name>`) on the same `owner`/`grants:*` attributes; `unaos/crates/kernel/src/fs/users.rs` holds only name, credential and home, and says who is logged in (`unaos/crates/kernel/src/fs/users.rs:17-26`; R51).
- The root volume's system trees (`/system`, `/system/types`, `/apps`, `/lib`) carry the owner `system`, written only by `kernel` and the administrator (`unaos/crates/kernel/src/fs/rootacl.rs:3-12`).
- Secrets: Holocron is a ring-3 fulfiller (`APPS/HOLOCRON.ELF`, verbs 144-151, `holocron_core`); the kernel supplies only `SYS_KDF` (66, Argon2id) and `SYS_RINGKEY` (67) (`unaos/crates/kernel/src/keyring.rs:1-12`).
- Helm (the control interlock, DISARM/MANUAL/AUTO + FAILSAFE) is `unaos/libs/sys/helm`, a host-workspace crate; the kernel does not link it at 1719fe54.

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-security-1 | No second identity machinery: the session principal is stamped into the same per-slot principal the loader stamps, and the same `SYS_OPEN` owner/grants check enforces it; `users.rs` never decides an access. | `unaos/crates/kernel/src/fs/users.rs:17-26`; `unaos/crates/kernel/src/arch/x86_64/syscall.rs:16892`; `unaos/crates/kernel/src/arch/aarch64/syscall.rs:7767` | `tests ring3abi` (`SYS_WHOAMI`) |
| BP-security-2 | No plaintext password anywhere (disk, wire, fixture line); each guess costs a PBKDF2-HMAC-SHA256 run calibrated to ~250 ms on the creating CPU, never below a floor. | `unaos/crates/kernel/src/fs/users.rs:59-60`; `docs/dev/OS/04_SECURITY_IMMUNITY/multiuser.md` §1 | unverified gate |
| BP-security-3 | The first user is the administrator; root is an internal principal with no password (no login, no prompt); the administrator's authority is what `install`, the updater and the boot-FAT unlock check (R100). | `unaos/crates/kernel/src/video/login.rs:662` `unaos/crates/kernel/src/video/login.rs:1156`; `unaos/crates/kernel/src/fs/rootacl.rs:8-9` | twoboot boot 1; `tests rootacl` |
| BP-security-4 | One answer for every login refusal (`users::verify` decides); a lock screen keeps the session and sweeps nothing. | `unaos/crates/kernel/src/video/login.rs:1205` `unaos/crates/kernel/src/video/login.rs:1873-1913` | `[login] denied user=… (one answer …)` |
| BP-security-5 | While the screen is up no typed byte reaches the wire (BP-input-4). | `unaos/crates/kernel/src/video/login.rs:17-27` | `[login] key taken by the screen` |
| BP-security-6 | System trees are owned by `system` through the NEAREST `owner` row; anonymous writes are refused, admin writes allowed, reads public. No path list in the ACL. | `unaos/crates/kernel/src/fs/rootacl.rs:3-12` | `tests rootacl` (`anon_write=refused admin_write=ok`) |
| BP-security-7 | The boot FAT is read-only to every principal including root; the only writer is the named installer/updater unlock (R99). | `unaos/crates/kernel/src/fs/bootfat.rs:1-15` (BP-storage-4) | `tests bootfat` |
| BP-security-8 | Root takes no screenshots: a capture belongs to a user's Desktop; no-session = refused (R67). | `unaos/crates/kernel/src/video/prtscr.rs:83` `unaos/crates/kernel/src/video/prtscr.rs:175`; R67 | `tests prtscr` |
| BP-security-9 | A program sighted on a removable/foreign volume is not a registrant: it writes nothing, lives in RAM for the boot, and opens only after ONE ask (Copy to Apps / Open / Cancel). Ring-3 input never widens kernel trust by itself. | `unaos/crates/kernel/src/fs/apptrust.rs:1-20` | `tests apptrust` (unverified name) |
| BP-security-10 | A preference is Principia's (its store), never a `<home>/.name` dotfile; the gate reds a dotfile literal with no allowlist row. | `docs/dev/EXECUTOR-BRIEF.md:49-51`; `unaos/scripts/arch-check.py:21` | `gates` rows `charter`, `arch` (`dotslash` leg) |
| BP-security-11 | The kernel keeps no secret and answers no Holocron verb; the formats, ring and policy are `holocron_core`, one implementation. | `unaos/crates/kernel/src/keyring.rs:4-12` | compile |
| BP-security-12 | A workaround that weakens a protection (SMEP, NXE, WXN, page permissions, checksums) is a STOP tripwire, never an executor's call. | `docs/dev/LAWS.md` §3 STOP tripwires; aarch64: `unaos/crates/kernel/src/arch/aarch64/mmu_wxn.rs` (the carve, B685) | review; `:: WXAUDIT-CORES …` (`unaos/crates/kernel/src/arch/x86_64/smp.rs:199`); aarch64 `tests wxaudit` → `:: WXAUDIT: wxn=1 wx_pages=0 -> PASS ::` (Pi, virt EL1, Orin) |
| BP-security-13 | The SATA write capability needs a minted grant on one port and LBA range (RAII hold, `-EPERM` without it) AND the operator's typed token for an install write. | `unaos/crates/kernel/src/install/ahciroot.rs:9-12`; `unaos/scripts/image-line.env:13-17` | `tests ahciw` |
| BP-security-14 | The volume key is the USER's: one random 32-byte data key per volume, kept only WRAPPED (Argon2id of the login secret at the slot's costs, SYS_KDF's call; and the 24-word recovery key, HKDF) behind ChaCha20-Poly1305 tags; the recovery words come from the kernel RNG, are shown once and are never written or sent; an unwrapped key is wiped on drop and the kernel keeps none (BP-security-11). | `unaos/libs/fs/unafs/src/keyslot.rs:330`; `unaos/crates/kernel/src/keyring.rs:717` | `tests volkey` (`wrapped=1 … sends=0`) |
| BP-security-15 | An ASK the user answered is never asked again unless revoked (R130 law 2): the answer is stored ONCE in Principia's store as `system.decisions.<domain>.<key>` (bound, for a program, to its volume and stamp), consulted before the ask is raised, never expired by the clock, dropped only by Settings > Privacy's Revoke. Every ask seam (apptrust's Open, the refusal alert, FWPIN's toast) records there; none keeps a private memory that outlives the boot. | `unaos/crates/kernel/src/prefs.rs`; `unaos/crates/kernel/src/fs/apptrust.rs`; `unaos/crates/kernel/src/video/dialog.rs` | `tests reask` (`:: REASK: domains=4 first_ask=1 reasks=0 after_revoke=1 -> PASS ::`, unflown) |

## 3. Seams
- Identity flows: `login` → `users::login` → `boot::session_opened` → loader stamps (`slot_user_stamp`/`session_restamp`).
- Access decisions: `vfs::native_write_authz` → `rootacl::write_verdict`; `unafs::read_authz`; handle table (`handle_resolve`, `SYS_CAP`).
- Secrets: ring 3 ↔ Holocron over bus verbs 144-151; the kernel's `kdf`/ring key bodies only.
- Helm/Principia safety levels gate physical actions (CODEX §5); no kernel caller at HEAD.

## 4. Wire
`[login] installer: first-user form open …` · `[login] session open user=<n> role=admin …` · `[login] denied user=<n> …` · `[login] screen locked user=` ·
`[rootacl] owner=system trees=<n> stamped=<m> root=<vol>` · `[boot] fat write refused …` · `[rand] source=` ·
`[wxaudit] arm64 el= sctlr= roots= leaves= pages= el1x_pages= el0x_pages= rx=[…) carve= text_rx= data_xn= classifier=` · `:: WXAUDIT: wxn=<0|1> wx_pages=<n> -> PASS|FAIL|SKIP ::`.

## 5. Ruled, unbuilt / open
- Helm's kernel core linked into Ring 0 (CODEX §2 Helm row says "Its kernel core (`unaos/libs/sys/helm/`)"): not a kernel dependency at HEAD.
- `permission_model.md` §2 "ask on use" and §3 "Glass Room" mock data: design text; status beyond §4 `unverified`.
- aarch64 W^X (B685 WXN64): the kernel image's executable pages are carved EL1-RO+X and every other EL1&0 leaf is execute-never, then `SCTLR_EL1.WXN` is set — Pi (`boot::build_l1`, bounds `[_start, __rx_end)` from `pi-baremetal.ld`), QEMU `virt` (`boot_virt::build_l1`) and the Orin (`mmu_tegra::init`), bounds from the image's own phdrs. Compiled; the QEMU and metal `:: WXAUDIT:` lines are owed to the fold.

## 6. Read further
- `docs/SECURITY.md` (model + hardening ledger), `docs/dev/review/SEC-2026-10-06.md`.
- `docs/dev/OS/04_SECURITY_IMMUNITY/multiuser.md`, `permission_model.md`.
