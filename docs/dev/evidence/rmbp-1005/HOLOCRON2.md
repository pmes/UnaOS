# HOLOCRON2 — the metal gets its secrets handler (rmbp-ledger B355)

Branch `exec-rmbp-holocron2`, cut from c3635e88, with `exec-rmbp-merge12` (boot-22 integration) and
`exec-host-merge1` (HOLOCRON1's `holocron_core` + `handlers/holocron`) merged in first. No new knob: the
kernel half rides `lumen` (UNAOS_LUMEN); the ring-3 store needs `selfdiag` (SYS_PATH_*), the relay needs
`busreg`. x86_64 only (aarch64 compiles the una-abi additions; nothing dispatches there).

## Design

**Finding (B355).** HOLOCRON1 left the metal with no fulfiller of verbs 144–151: LUMEN reads the Claude key
from a plain file (`vein.key_file`, default `<home>/.config/unaos/vein.key`), and nothing on the rMBP can
seal or sign. Three things block a straight port of the host daemon into ring 3:
1. **The relay stamp.** x86's BANDY3 relay stamps the CALLER as `row:<r>/gen:<g>` (kind 3), which
   `holocron_core::wire::principal_from_record` refuses by design — every caller would be DENIED.
2. **Argon2id does not fit ring 3.** The ELF window is 4 MiB (una-abi `USER_WINDOW_BYTES`); the ring KDF's
   floor is 19 MiB and the default 64 MiB.
3. **No directory listing for ring 3.** `SYS_PATH_READ` answers `-EISDIR` on a directory, so `SecretList`
   has no source.

**Seams.**
* *Holocron — shared-core*: `holocron_core` is the ONE implementation (formats, ring, service, codec). New
  module `holocron_core::frame` = the BANDY v1 frame around a Holocron body (request, relayed request,
  reply) — the codec HOLOCRON.ELF and LUMEN link, KAT'd on the host against the host daemon's own bytes.
* *Kernel — fulfiller* (BANDY3): the x86 relay stamps a caller that runs in the open session with its
  user record, kind `PRIN_USER` (5), value `user:<name>#<uid>` — the ATTRSURF principal, the same string
  aarch64 already mints; every other caller keeps `row:<r>/gen:<g>`.
* *Kernel — shared-core*: `SYS_KDF` (66) = Argon2id through `holocron_core::cc::CryptoCore::derive_key`
  (CRYPTOCORE's code, not a second KDF), fallible allocation in the kernel heap, parameters bounded to
  `[FLOOR, 64 MiB / t 10 / p 8]`. HOLOCRON.ELF's `Sealer` is `CryptoCore` with `derive_key` routed here.
* *Kernel — fs-core*: `PATH_R_LIST` (flag bit 3 on `SYS_PATH_READ`): a directory read answers its entry
  names, `\n`-joined, directories with a trailing `/`, only inside the caller's own home.
* *Holocron — fulfiller*: `crates/user-holocron` → `APPS/HOLOCRON.ELF` (ELF window, app note RESIDENT).

**HOLOCRON.ELF.** Owner = `user:<name>#<uid>` from `SYS_WHOAMI`. Store = `<home>/.config/unaos/holocron/`
(`.ring`, `<ns>/<name>`) over SYS_PATH_READ/WRITE (`PATH_W_TRUNC|MKDIRS`, `PATH_W_UNLINK`, `PATH_R_LIST`),
refused unless `<home>` stats with an inode id (UnaFS; FAT has no owner). Entropy =
`DrbgEntropy<GetrandomEntropy>` over SYS_GETRANDOM. It REGISTERs 144–151:
* `0` → it is the fulfiller: applies its own argv verb first (so `holocron init <pw>` / `holocron unlock
  <pw>` leave the key in THIS process), then serves relayed requests (caller = the relayed header's record
  through `principal_from_record`; unlock backoff = the service's limiter) until the session ends (SECLOGIN
  ends a session's programs at logout).
* `-EEXIST` → a fulfiller runs: it is a client — sends its argv verb over the bus, prints the answer (never
  a secret's bytes: `get` prints the length) and raises it as a NOTICE, exits.
Verbs: `init <pw>`, `unlock <pw>`, `lock`, `status`, `put <ns> <name> <value> [label]`, `get <ns> <name>`,
`list <ns>`, `delete <ns> <name>`; bare `holocron` = `status` (and serve).

**Milestones.**
- M1 — `holocron_core::frame` + its host KAT (`handlers/holocron/tests/m3_metal_frame.rs`: frames built by
  the ring-3 codec drive the REAL host daemon; its reply bytes parse back through the ring-3 parser; one
  golden frame); kernel: relay stamp, `SYS_KDF`, `PATH_R_LIST`; `crates/user-holocron` → HOLOCRON.ELF,
  arroyo `build_user_holocron_x86`, builder staging (ESP + DATA `APPS/HOLOCRON.ELF`).
- M2 — `vein_ring3::holocron::claude_key` (SecretGet over the bus, `keysource::decide`); LUMEN asks it
  first, falls back to `vein.key_file` / the default path only on NotFound or no fulfiller; the start line
  says `key=holocron`, or `key=<file state> holocron=<why>`, and the window notes which.
- M3 — the bare name `holocron …` (RING3ABI2 argv; RESIDENT note ⇒ detached) and the login-path launch:
  `users::login` → `keyring::after_login` spawns `/apps/HOLOCRON.ELF` when the user's `.ring` exists.
- M4 — `tests holocron` (`keyring::selftest`, the kernel linking `holocron_core` on a RAM ring) + this doc.

**Witness.** `tests holocron`: `[holocron] fx lock-then-get=<status>` then
`:: HOLOCRON: ring=<unafs|none> verbs=<n> owner=<user> put=ok get=ok denied=1 corrupt=1 -> PASS ::`.
HOLOCRON.ELF: `:: HOLOCRON: serve ring=<unafs|none> verbs=8 owner=<user> state=<none|locked|unlocked> ::`,
per verb `[holocron] <verb> -> <status>`; LUMEN: `:: LUMEN: start … key=holocron …`.

**Stays owed.** The metal boot (R78). A password typed as argv (the args page and shell history see it; no
pinentry); LOGIN does not hand Holocron the login password. "Exit when the last client leaves" is
approximated by session end (the kernel cannot tell a fulfiller its callers left). The SSH agent has no
socket on the metal. `SYS_KDF` runs Argon2 synchronously in the caller's syscall (≈0.5 s at 64 MiB).

## Results (compile legs; R78 — no QEMU, the metal boot is the seat's)

Commits: M1a `4fd34750` (frame + host KAT, relay stamp, SYS_KDF, PATH_R_LIST, keyring) · M1 `5247b264`
(user-holocron, arroyo, builder) · M2 `fb8f947b` (LUMEN asks Holocron first) · M3 `599d4148` (bare name in
`tests exec` + midden_core) · M4 this doc.

- x86 metal shape + `selfdiag,ahciroot,btc` (`wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,
  hda,hda-tone,facet,beam,sdw,sdwrite,sdhcblk,selfhost,linuxabi,ahci,unafs,busreg,lumen,netring3,prefs_reset,
  census,installdemo,instgui,witness,selfdiag,ahciroot,btc`): exit 0.
- aarch64 `login,loginst,virt_el0,lumen` (user blob rebuilt, head 280080d2): exit 0.
- `build_user_holocron_x86` (the arroyo function, run verbatim): exit 0 — `HOLOCRON-X86.ELF: 70184 B file,
  model=elf span=69936 stack=262144`, `:: ELFENTRY: HOLOCRON-X86.ELF entry=0x10000200000 … -> PASS ::`, PT_NOTE
  `UnaOS` desc `02` (RESIDENT) kept.
- `build_user_lumen_x86`: exit 0 — `LUMEN-X86.ELF: 215408 B file, model=elf span=3285184 stack=262144`, ELFENTRY
  PASS; `holocron=` and `key: from Holocron` present in the image (`LC_ALL=C grep -a -o -F`).
- `cargo test -p holocron_core -p holocron -p una-abi`: exit 0 (holocron_core + 3 frame tests; holocron +
  `m3_metal_frame`: `:: HOLOCRON2-KAT: client-bytes=8 …`, `:: HOLOCRON2-KAT: daemon round trips=8 put=ok get=ok
  locked=-37 notfound=-2 -> PASS ::`; `:: HOLOCRON2-ABI: kdf=66 path_r_list=8 verbs=144..=151 -> PASS ::`).
  `cargo test -p vein_ring3`, `-p midden_core holocron2`: exit 0. `tools/prefs-schema-check.py`: PASS 29/29.
- `charter-check.sh`: exit 0.

**Wire a metal boot should print.** `tests holocron` (session open, UNAOS_LUMEN=1):
`[holocron] fx kdf=argon2id m=19456KiB t=2 create=0 lock=0 lock-then-get=-37 ms=<n>` then
`:: HOLOCRON: ring=<unafs|none> verbs=<0|8> owner=<user>#<uid> put=ok get=ok denied=1 corrupt=1 -> PASS ::`.
`holocron init <pw>` (first, no ring): `[holocron] unlock -> ok (in this Holocron)` then
`:: HOLOCRON: serve ring=unafs verbs=8 owner=<user> state=unlocked ::`; `holocron put vein claude.api_key <key>`:
`[holocron] put -> ok (via the running Holocron)` and the fulfiller's `[holocron] relay verb=put caller=owner ->
ok`; then `lumen`: `:: LUMEN: start provider=claude … key=holocron transport=tls …`. Next login with a ring:
`[holocron] login user=<u> ring=unafs -> started pid=<p> slot=<s>`. `tests exec`: `resolved=6/6`.

**Not finished / owed.** The metal boot. Password as argv (args page, shell line); LOGIN does not hand Holocron
the login password. The fulfiller lives until the session ends (no "last client" signal from the kernel).
SYS_KDF blocks its caller ≈0.5 s at 64 MiB/t 3/p 4. No SSH-agent socket on the metal; no UnaFS typed-attribute
mirror (`created/kind/label`) on metal-written secrets; a secret is replaced by truncate+write (not atomic).
user-holocron is not in arroyo's USER_CHECK_MATRIX type-check list. aarch64: SYS_KDF / PATH_R_LIST not dispatched.
