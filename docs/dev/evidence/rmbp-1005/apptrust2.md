# APPTRUST2 (B478) — the four doors APPTRUST (B467) left open

Branch `exec-rmbp-apptrust2`, cut from merge19 @5da57a2d. No knob. No new file (all in `fs/apptrust.rs`,
`fs/appres.rs`, `video/quarry/openers.rs`, `video/quarry/openwith.rs`). Flights 24/25 carry no `[apptrust]` line
(no stick with a program was listed): the finding is B467's own Owed list, not a wire.

## Finding
1. A session grant is the PATH alone: a re-plugged stick (or another stick named the same) carrying a different
   program at the same path is trusted without asking.
2. `appres::app(key)` / `with_pix` take the FIRST memo entry with the key: a stick's `LUMEN.ELF` sighted before
   `/apps/LUMEN.ELF` lends its icon to `lumen` (dock, notifications, About, login) for the boot.
3. Double-clicking the foreign program ITSELF (Quarry `launch`, and the Launcher's file pick, which lands in the
   same dispatch) launches it with no ask.
4. Copy to Apps writes as `kernel` after `users::admin_authority`, beside ROOTACL (B456) instead of through it.

## The seam (no new store)
- Grant = `(volume identity, path, stamp)`: the volume's VOLID fingerprint (`MountTable::volume_id`, the value
  `same_storage` compares: FAT serial + clusters) and APPRES's stamp (`<mtime>:<size>`), bound at ASK time
  (`request` stats; the press/key answer stays queue-only). A grant whose path matches but whose volume or stamp
  does not is dropped and the ask is posted again.
- Icons: a foreign sight is recorded with `App.source = "foreign"`; every by-key lookup goes through one picker that
  prefers a non-foreign entry whatever the order; a foreign sight first sights `/apps/<leaf>` when it exists, so a
  root program's key never draws a foreign icon.
- Direct launch: `openers::open("launch", p)` (the one dispatch Quarry's double-click and the Launcher's file pick
  reach) asks `apptrust::launch_gate` — a foreign path without a matching grant posts the same DIALOG2 ask
  (`Open <prog> from <volume>?`, `Copy to Apps` · `Open` · `Cancel`), file = prog; the answer runs on Quarry's
  service pass as before. The dock's pins are kernel table apps and the Launcher's Programs are `/apps` (root): no
  foreign path reaches them.
- Copy: the principal is the logged-in session's `user:<name>` when its role is Admin (`rootacl::admin_principal`),
  and `create`/`write`/`unlink` run under it — ROOTACL's `write_verdict` decides. No admin session → refused with
  the alert (`dialog::refused(WHAT_SYSTEM_FILES, …)`).

## Milestones
- M1 — bound grant: `[apptrust] grant volume=<name>#<id> path=<p> stamp=<m:s>`; a mismatch:
  `[apptrust] grant stale path=<p> reason=<volume|stamp> -> ask`.
- M2 — icon: `[appres] icon-for name=<key> from=<root|foreign>` (once per decode).
- M3 — launch ask: `[apptrust] launch prog=<p> volume=<v> -> ask` then the ask/answer lines; a grant →
  `[apptrust] launch prog=<p> granted=session`.
- M4 — copy via ROOTACL: `[apptrust] copy <src> -> <dst> bytes=<n> registrant=<yes|no> via=rootacl principal=user:<n>`
  or `refused=no-admin-session` + the alert; `tests apptrust` grows `rebind=asked icon=root launch=asked copy=rootacl`.

## Witness (the next flight reads)
- `tests apptrust` → `:: APPTRUST: foreign=listed registrant=refused asked=once copied=registrant rebind=asked icon=root launch=asked copy=rootacl -> PASS ::`
- Metal (a stick with a program): double-click it in Quarry → `[apptrust] launch prog=/volumes/<v>/X.ELF volume=<v> -> ask`,
  `[apptrust] ask … posted=1`; Open → `[apptrust] grant volume=<v>#<id> path=… stamp=…`; Copy to Apps →
  `[apptrust] copy … via=rootacl principal=user:<admin>`.

## Owed
- The stamp is `<mtime>:<size>` (APPRES's): two programs of equal size and mtime on the same re-formatted-to-the-same-
  serial volume would share a grant; a content hash is the next step if the seat wants it.
- The shell's `bg <path>` / bare name of a foreign program is the operator's typed act and is not asked.
