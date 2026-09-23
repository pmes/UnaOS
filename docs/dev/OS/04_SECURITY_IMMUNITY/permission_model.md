# Permission Model: Capabilities & Intents

## 1. The Death of "Root"
In unaOS, there is no "Root" user that can do everything. Even the Kernel has restrictions.
* **Principle of Least Privilege:** Apps start with **zero** permissions. They cannot see the webcam, the microphone, or your documents.

## 2. Dynamic Intents ("Ask on Use")
We do not ask for permissions at install time (when users blindly click "Yes"). We ask at **usage time**.
* **Scenario:** A photo editor wants to save a file.
* **The Old Way:** The app has access to your whole `Documents` folder.
* **The unaOS Way:** The OS opens a "Save" dialog. The user picks a file. The OS passes *only that specific file handle* to the app. The app never sees the rest of the folder.

## 3. The "Glass Room" (Privacy)
For apps that demand invasive permissions (like social media apps wanting your contacts):
* **Data Mocking:** The user can choose to feed "Mock Data" to the app.
* **The Result:** The app thinks it uploaded your contacts, but it actually uploaded a generated list of fake names ("John Doe," "Jane Smith"). The app functions, but your privacy remains intact.

## 4. Kernel Mechanism: Handles as Capabilities (implementation status)
The "specific file handle, never the whole folder" model of §2 is not just a UI convention — it
is the kernel's enforcement mechanism. A **handle** is an unforgeable, per-process reference that
carries **rights** and is **checked at the point of use**. The chain lands incrementally on the
aarch64 (Pi 4) track and ports to x86/Jetson after:

* **U4 (landed)** — the *structure*: a per-process handle table, keyed by address-space id (ASID).
  A child process is named by a handle into the spawner's table, so ownership is structural — a
  process can only act on handles in its own table.
* **U5 (landed, 2026-07-05)** — the *check*. A handle now carries a **rights bitmask**
  (`read`/`write`/`exec`/`grant`/`revoke`) and names a **target** (a child process, or a resource
  such as the console). Every resource syscall resolves its handle through one enforcement point
  that requires the needed right, else `-EACCES`. This makes the three capability operations real:
  * **Grant / attenuate** — a process holding `grant` on a handle can mint a *new* handle to the
    same resource for another table, but only with a **subset** of its own rights. A grant can
    **never amplify** rights (the monotonic-decrease invariant). This is exactly the "pass only
    that file handle, read-only" story, enforced.
  * **Revoke** — a process can drop a handle it owns; any later use fails.
  * **Bounded lifetime** — a process's whole handle table is cleared when its address space is torn
    down, so no capability outlives the process that held it.
  As the first routed resource, `write` to the console is now a capability (`CONSOLE` + `write`),
  granted to a process at launch — there is no ambient "stdout everyone can write".
* **U6a (landed, 2026-07-06)** — the *general object table*. A handle is now a
  `(kind, target, rights)` descriptor: the **kind** (`Child` / `Console` / and the scaffolds `File` /
  `Socket`) rides in a parallel sidecar so the value word keeps its `Empty`/`in-flight` sentinels
  untouched, and **all** kinds are first-free-allocated by one allocator. This retires U5's fixed
  console index (`CONSOLE_FD`): that index is now a *reserved* slot the allocator skips, so a process
  that both prints **and** spawns can hold a console cap alongside its child/object handles with no
  index collision — the one fragility U5's review flagged. `File`/`Socket` are scaffolds today
  (resolvable to their kind with rights-checking, but no filesystem/network syscall routes through
  them yet); they prove the table is general, and are where fs/net capabilities will attach.
* **U7 (landed 2026-07-07, the cross-process core)** — capability *transfer between processes*,
  kernel-mediated and single-writer-preserving: `SYS_XFER` deposits an **attenuated** descriptor into
  the recipient's per-ASID transfer **inbox** (the one deliberately cross-ASID surface, CAS-managed);
  `SYS_RECV` pulls it into the recipient's **own** handle row; the recipient is named by a `Child`
  handle in the sender's table (owner-scoped — no global process namespace). A **sender-owned
  transfer record** gives single-level **revoke**: the received cap goes stale at its next resolve.
  A transfer can never amplify (the same monotonic-decrease invariant as grant, now across
  processes), and the sender never writes — or revokes into — anything but its own record.
* **U6 (landed 2026-07-08, aarch64 — the by-NAME ACL)** — the file namespace itself is now ACL'd at
  `SYS_OPEN`, closing the gap that handle-cap gating left open (any process could open/create/unlink
  any name). **Owned-by-default:** an `O_CREAT` of a new name records the creating principal as the
  file's **owner** (private); an `O_PUBLIC` bit opts out to world-access. An open of an owned file is
  admitted only for the owner or a principal it **granted** (`SYS_FGRANT`, owner-scoped via a `Child`
  handle, mirroring `SYS_XFER`; `rights = 0` revokes) — the grant is an ACL edge on the *file*, so the
  grantee simply opens the name and the check admits it. The store is in-kernel and keyed by the file's
  identity, fenced by the `(ASID, ASID_GEN)` incarnation; it is the **enforcement seam** a persisted
  owner/grants store feeds. That store exists TODAY as the K1 `UNAFS.ATR` FAT-bridge sidecar (an on-disk
  owner form since K1/K2/K3 — cross-reboot enforcement metal-confirmed on real Pi 4); at **K4** it becomes
  the NATIVE UnaFS `owner`/`grants:*` typed attributes below, once a kernel UnaFS mount lands (gated on the
  ROADMAP §2 BeFS convergence: no_std port → block adapter → read-only mount → journaled writes). The
  K4-ready projection codec (the 1:1 sidecar→native-attribute string mapping) landed 2026-07-12, ahead of
  that mount. The x86 twin (U6x) is future.
* **Still ahead** — **revocation trees** (a revoked transfer cascading through the recipient's
  re-grants/re-transfers — today a derived copy escapes single-level revoke; per-cap derivation
  records + the reserved `revoke` right are that arc), the **bandy Ring-3 delegation wrapper** (so
  host-native principals delegate over the message bus), and real filesystem/network syscalls
  routing `File`/`Socket` payloads (File transfer needs descriptor migration). This is where the
  UnaFS `owner`/`grants:*` attributes of the model above become the persistent form of these live
  kernel handles.

## 5. The human user (LOGIN, RULINGS R51 — 2026-09-12)

Peter: *"keep working on multi-user so i can login, get a home folder and all."* `docs/SECURITY.md`'s
POSIX hedge, taken: **a user is a named bundle of capabilities on the same `owner`/`grants:*`
attributes the chain above built.** Nothing in §4 is redone; a user is one more principal KIND.

* **A user is a principal.** Kind `user`, canonical string `user:<name>` — beside `prog:<NAME>`
  and `sha256:<digest>` in the same `PrincipalRecord`, the same K4 codec (`user:` forward and
  reverse arms), the same per-slot stamp the loader writes. There is no second identity table.
* **The record** (`fs/users.rs`, `login` knob): `USERS.DAT` at the root of the root volume — the
  NAME, the CREDENTIAL (16-byte salt + SHA-256 of `salt || password`; never a plaintext password on
  disk or on the wire) and the HOME PATH (`/home/<name>`). Fixed-stride rows, a CRC per row and per
  header, fail-closed parse, swap-not-overwrite publish (`USERS.NEW` proven readable before
  `USERS.DAT` is replaced). An attacker holding the card reads names, salts and hashes and can guess
  offline against them; that is the stated posture of a salted hash with no hardware key store.
  Not a Holocron class: the credential file has a different reader (the login screen, before any
  session exists), a different writer and a different blast radius than the Bluetooth bond store.
* **The session** is one record: `login` verifies the credential and opens it, `logout` closes it.
  While it is open, **every program launched is stamped `user:<name>`** at load — aarch64 restamps
  the slot's persistent principal right after the loader's image stamp (`session_restamp`), x86
  carries the users-table id per slot (`SLOT_USER`) because its ACL has no principal string. The
  enforcement is the existing `SYS_OPEN` owner/grants check: a file created in a session is owned
  by `user:<name>`; a later open by that user (any program, any slot, after reboot on aarch64 via
  the by-name branch) is admitted; any other principal is the existing `-EACCES`. **The boundary is
  the SESSION, not the
  launch (SO37, corrected here — this sentence used to read "Programs already running when the session
  closes keep their stamp — the boundary is the launch, not the clock").** A program's stamp is still
  taken once, at load, and never changes; what changed is that the stamp is QUALIFIED by a SESSION EPOCH
  — a `u32` beside the per-slot principal, written with it at the one mint path (`session_restamp` /
  `slot_user_stamp`) and incremented by every `logout`. `slot_ppid_of` — the ONE reader that `SYS_OPEN`'s
  by-name branch, the `O_CREAT` owner persist and the grantee capture all go through — returns NONE for a
  `user:` stamp whose epoch is not the live one, so a program of a CLOSED session is ANONYMOUS: it opens
  nothing the user owns and creates nothing in the user's name, even if the same user logs straight back
  in (the principal string is identical; only the epoch separates the two sessions). x86's twin is
  `SLOT_EPOCH` beside `SLOT_USER`, read by `slot_user_live`, which `owned_user_ok` and `owned_user_stamp`
  consult.
  **NOT AN ABI CHANGE, and SO37 expected one.** Nothing in `PrincipalRecord` changes size or meaning: it
  is 32 bytes, `kind`/`len`/`value[30]`, serialised into `UNAFS.ATR` rows and projected to the native
  `owner` / `grants:<grantee>` strings by the K4 codec. An epoch INSIDE the record would be an on-disk
  format bump AND would make `user:<name>` a per-session identity — a user's own files would stop opening
  after a relogin, which is the property `/home/<name>` exists for. The epoch qualifies the STAMP (this
  slot's claim to be speaking as the user right now), never the IDENTITY (who the user is, durably), so
  it lives in a runtime-only side table that is never serialised and never on the wire as an owner. And
  there was no room anyway: `value` is a hard 30 bytes, fully consumed by `PRIN_IMAGE_SHA256`.
  Cost, stated because `SYS_OPEN` is a hot path: for a caller that is not user-stamped — the whole
  fixture battery, every program on a boot with no session — ONE `cmp` on a byte already in a register,
  inside a lock that was already taken; for a user-stamped caller, two more atomic loads. No allocation,
  no new lock, no fallible call, NO PANIC PATH. Epoch 0 is never live (`SESSION_EPOCH` starts at 1,
  `SLOT_EPOCH` at 0), so an unstamped slot fails the comparison rather than passing it.
* **`/home/<name>` (M2):** created on the EL0 FAT volume at the user's first login (`HOME/<NAME>`, an
  8.3 leaf, so a user name is 1-8 bytes). The directory carries no owner (FAT has none; LEDGER SO35);
  the files a session's programs create inside are owned by `user:<name>` through the SYS_OPEN rows,
  reached on aarch64 by the knob-on path-taking open (`HOME/UNA/NOTES.TXT`, SO20's first step). Proven
  on the real ACL tables (fixture `loginst`): the owner and a later launch under the same user are
  admitted, an anonymous principal is refused (aarch64 by name on the EL0 regime; x86 by users-table
  id on its static name table). The fixtures — which write a KNOWN credential to the medium — ride
  their own knob `UNAOS_LOGINST=1`, never `UNAOS_LOGIN=1` (the `hcronst` rule).
* **The login screen (M3):** the desktop boots to a self-drawn screen (`video/login.rs`, declared beside
  the crystal under its gate; the Mac model: name, password, Enter; the first boot with no users is the
  create-first-user form) that takes every key on every route BEFORE the route's serial echo — so a
  typed password never reaches the wire — and before the shell console; a successful login opens the
  session (and the home) and takes the screen down; Esc dismisses nothing here (R24: menus only). The
  routes reach it through `fs::users::screen_key` / `screen_open_once`, which are inert where no desktop
  is built. **The screen is also closed TO the pointer (LOGINCLOSE):** its `wm` row is minted in the
  shell/desktop owner band, which `wm::controls` gives no control cluster and `wm::hit_test` never
  names, so no close box is drawn and no press can reach the row — measured every witness boot against
  an armed control row (`close_route=refused`), with `login::heal_if_row_gone` putting the three pieces
  of screen state back at the next key offer should anything close the row by another road. **And
  the screen is MODAL TO THE POINTER (SO36 + SO44), which is ONE statement and not two.** While it is
  up, `fs::users::screen_press` answers `true` for EVERY press on the panel — inside its rectangle and
  outside it alike — and it is asked FIRST in `video::strip::press_route`, ahead of the window menu, the
  crystal and the dock, and therefore ahead of every window arm in the aarch64 router; the x86 router
  keeps its OWN furniture arms (the CLICK-BAND re-split) and never calls `press_route`, so the same
  statement is repeated at the head of its press edge rather than assumed. That closes BOTH halves. A
  press INSIDE the rectangle no longer falls THROUGH the screen onto the row beneath and raises it
  (SO44 — Peter: *"the login window appeared over the top of the gui and when i click it it went away"*;
  the screen's `owner_asid = 0` band is never named by `wm::hit_test`, so a press on its own pixels
  resolved to whatever was behind). A press OUTSIDE it no longer reaches a dock tile, the crystal or a
  window menu, so no program can be launched under NO principal before a session exists (SO36). The
  rejected alternative is named so it is not reintroduced: special-casing `owner_asid == 0` in
  `wm::hit_test` would hand the screen a control cluster and a close box back — LOGINCLOSE's measured
  defect — and would gate only the points inside the rectangle. Modality is a property of the ROUTER, not
  of the row.
* **And the press is the SCREEN'S — SO44's second sentence (LOGINFLOW 2026-09-22).** SO44's rule is two
  sentences: *a press outside the rectangle belongs to nobody* **and** *a press inside it belongs to IT*.
  SESSGATE landed the first and left the screen a WALL — `press_swallow` answered `true` for every point
  and did nothing with it, which to the person in front of the glass is not distinguishable from a
  machine that has hung: press the password field, the caret does not move; press where a button should
  be, there is no button. The coordinates the barrier has always taken and thrown away are now USED.
  `login::local_of` maps the panel point through the row's own `wm` geometry (origin AND integer
  upscale), `login::ctl_at` asks which control is there, and the control ACTS — the two fields focus, the
  **button** submits (the same `submit` Enter calls; there is no second path to keep in step), a **user's
  row** picks that name out of the store (`users::name_at`) and moves to the password. The screen
  therefore SHOWS who lives on this machine, the Mac model, and carries a button, because Enter is not
  discoverable and a person who has just chosen a password has no reason to know it is the only way in.
  The RETURN VALUE is unchanged and must stay unchanged — `true` for every press while the screen is up,
  hit or miss — because the modality is the ROUTER's contract and is not conditioned on there being a
  control under the point. Painter and press read ONE accessor (`login::ctl_rect`): `wm::control_disc`'s
  discipline, for LOGINCLOSE's reason one layer out — a control drawn from one rect and hit-tested from
  another is one edit from being drawn where it cannot be pressed. And a **wrong password says one
  thing**: `submit` asks `users::verify`, documented as *one answer for "no such user" and "wrong
  password"*, rather than reading a `UsersError`, so neither the glass (`Login failed`) nor the wire
  (`[login] denied user=<name>`) can grow a reason that tells someone at the keyboard which names exist.
  A credential that verified but whose session did not open is said DIFFERENTLY (`Could not open the
  session`) — that is a storage or slot refusal, not a denial, and a person who typed the right password
  must not be sent to look for a typo. GATE: `login::control_leg` (`loginst`), which drives the **live
  arch router** — `wc_click_route_at` on x86, `strip::press_route` on aarch64, named on its own verdict
  line as `route=` — and never `press_swallow` directly, because a fixture that calls the predicate
  stays green on a tree whose router arm has been deleted (rmbp-ledger B121, one band over).
* **The IGNITION, and the one rule it follows (SO43, LOGINBOOT 2026-09-13):** the screen comes up
  because **the DESKTOP EXISTS**, never because a console route was installed. M3 wrote two of its four
  ignition sites as `if activate() { … }`, and `desktop_firmware::activate`'s return is
  `fbcon::console_is_routed()` — a fact about where the console's glyphs land. On a board whose panel
  reads 0 at the first call that is `false` on a boot whose desktop came up perfectly, so the Orin
  printed `[deskcascade] -> CASCADED windows=2 bar=1 owns_pixels=1 route=ROUTED activate=false` and
  drew no screen, five flights running, while the x86 ladder stayed green because there `activate()`
  is true. Every site now calls one named seam, `fs::users::screen_open_at_ignition(desktop_up,
  console_routed)`, which takes BOTH facts, prints both (`[login] ignition desktop_up=… console_routed=…
  -> OPEN|HELD`) and decides on the first; `desktop_up` is the caller's READBACK of the one
  unconditional step of its own bring-up, the menu bar. Asserted every witness boot by the
  `LOGIN-IGNITION` leg, which drives the Orin's own tuple (`desktop_up=true console_routed=false`) and
  reds the moment the rule consults the second term again.
* **Log Out (M4):** a row in the CRYSTAL menu (`crystal::Verb::LogOut`, R21 — menus belong in the menu
  bar), at the foot of the SHARD tree behind its own separator, knob-on only. The pick drops the session
  principal (`fs::users::logout`) and puts the login screen back up, where a second login opens a NEW
  session under the same or another principal — each with its own `/home/<name>`. What Log Out does NOT
  do is QUIT the programs the session already launched: they keep running and keep the stamp they were
  launched with. That stamp is now DEAD — SO37 is CLOSED: the pick burns the session epoch, so every slot
  stamped in the closed session reads ANONYMOUS from `slot_ppid_of` down and an owned file is NO LONGER
  reachable to a process the logged-out user started (see the session bullet above). What stays reachable
  to it is only what its own live `(asid, gen)` incarnation owns, which is the pre-login world. LEDGER
  SO37 fixed; a real quit-all through the dock's launch registry (R49's `PinnedApp` list) is the Mac
  behaviour and is a separate arc. The row is proven ROW -> VERB -> ACTION through the
  menu's own pure resolver (`crystal::logout_row_fire`, fixture `loginst`), and its placement on the
  real panel by `crystal::selftest` leg 3, which walks every row of the tree.
* **True secure multi-user login (SECLOGIN, R62 — 2026-09-22; the design is `multiuser.md`, the
  ledger row rmbp B169).** B157's eight gaps, each taken in its own milestone. The credential is
  PBKDF2-HMAC-SHA256 at a per-boot calibrated count stored per row (`USERS.DAT` v2, floor 10 000,
  a v1 image adopted and re-hashed at the next successful login; `verify` constant-time for an
  unknown name too). The identity is a `uid` that is never reissued — x86's u32 tables and
  aarch64's `user:<name>#<uid>` string compare the same number, so a recreated user in a recycled
  slot or under a reused name is refused, and `delete_user` exists at last. **Log Out ENDS the
  session's programs** (windows closed, then killed through the close box's own path, then the
  epoch bumps — `[users] logout epoch=<n> ended=<n> windows=<n>`), so the sentence above that a
  program "keeps running" is history. The credential file is kernel-owned on the aarch64 path
  resolver, and the x86 half is one owed line in `fs::vfs::el0_locate` (`multiuser.md` §6). The
  salt draws from `rand.rs` (RDRAND / RNDR / a documented jitter fallback, the source said on the
  wire once), and the epoch is 64 bits. The user rows on the pre-session glass are a stated policy
  predicate (`login::roster_on_glass`), and `ensure_home` names the volume by serial. Encryption
  at rest is NOT built and the design says what it needs.
* **Status (M1, `exec-orin-login`):** record store, session principal on both arches, `login`/
  `logout` shell arms and the fixture landed; measured while landing: the typed verbs wait on one
  `HOST_VERBS` row in `libs/sys/midden_core`, the x86 QEMU build waits on one `UNAOS_LOGIN` map
  line in `builder/src/main.rs`, and x86 EL0 opens a static root name table (SO20) so the home-file
  proof is aarch64-first. `/home/<name>` at first login (M2), the login screen (M3) and Log Out in
  the crystal (M4) all landed in the same arc. LEDGER SO32.

See `docs/SECURITY.md` (the hardening ledger) and `docs/dev/OS/02_KERNEL_CORE/userspace.md`
(the syscall-level detail) for the exact mechanism and evidence.
