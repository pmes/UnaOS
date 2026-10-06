# APPTRUST (B467) — a program sighted on a removable volume is not a registrant until the user says so

Branch `exec-rmbp-apptrust`, cut from merge19 @575194d1.

## Finding
OPENERTRUST (B447) made a document's `una:preferred` honour only a REGISTRANT — but a registrant was whatever
APPRES sighted: `appres::sight_in` → `cache` → `publish_doctypes` appended ANY sighted program's path to `una:apps`
on its declared types' objects, and wrote its signature object (`una:app.path`). A program on a USB stick or the
camera card, merely listed in Quarry (`columns.rs` sights every ELF in a listing), became a legitimate opener for
every type it declared — and could take over a root program's SIGNATURE object, so the registry's `una:preferred`
(a signature) would map to the stick's program. SECREVIEW: ring-3 input never widens the kernel's trust by itself.

## The seam (no new store)
`fs/apptrust.rs` (CHARTER: Kernel — fs-core). Trust is WHERE the program is: `trust_of` = the backend claiming the
path is the same storage (`vfs::same_storage`) as a SYSTEM mount (`/`, `/boot`, `/apps`, `/lib`); a removable
volume, another disk under `/volumes`, a stale `/volumes/<gone>` path the root would claim, or an unresolvable path
is FOREIGN. `appres::admit` is the ONE admission a sight makes: root → cache + publish as before; foreign → writes
NOTHING (no attribute on the stick, no signature object, no `una:apps` line) and joins a per-boot RAM list.
Refusal at every reader: `appres::registrants` skips a foreign `una:apps` line (one planted before this arc),
`opener_of_preferred` never maps a signature to a foreign sighting, `assoc::opener_for_in` ignores a registry
preference that names a foreign program.

## Milestones
- M1 — the sight: `[appres] sighted <path> volume=<v> trust=<root|foreign>`; `admit`; the three reader refusals.
- M2 — the ask: Open With lists foreign programs under a divider as `<name>  on <volume>`; a pick posts DIALOG2
  `Open <file> with <program> from <volume>?` (`Copy to Apps` · `Open` · `Cancel`, Cancel the default, Esc cancels;
  `Act::Hook` as DOCK2 does, `dialog::last_button` tells the buttons apart). `Open` grants the program for the
  session (RAM); `Copy to Apps` asks `users::admin_authority("apptrust-copy")`, copies into `/apps` (never over an
  existing file), sights the copy (a root sight → registrant) and opens the file with it. Both run on Quarry's
  service pass, never in the press route.
- M3 — `tests apptrust` + SECURITY.md row.

## Witness (the next flight reads)
- Insert a stick carrying a program, list it in Quarry: `[appres] sighted /volumes/<v>/X.ELF volume=<v> trust=foreign`
  and NO `una:apps` growth; Open With on a file of its type shows it under the divider `on <v>`.
- Pick it: `[apptrust] ask prog=... posted=1`; Open → `[apptrust] answer=open` then `[apptrust] open ... granted=session`;
  pick again → no dialog. Copy to Apps → `[auth] admin=<n> for=apptrust-copy` then `[apptrust] copy ... registrant=yes`.
- `tests apptrust` → `:: APPTRUST: foreign=listed registrant=refused asked=once copied=registrant -> PASS ::`.

## Owed
- `appres::app(key)` (dock / icon lookups by key) still finds a foreign memo entry by its file stem: a stick's
  `LUMEN.ELF` sighted BEFORE the root one could lend its icon to the key `lumen` for the boot (cosmetic, not an opener).
- Double-clicking the foreign PROGRAM itself still launches it (a direct act on the program, not an opener choice).
- Copy to Apps writes as the kernel after `admin_authority`; ROOTACL's tree ACL on `/apps` (B456, running) is the
  same question asked the same way — when it lands the copy should go through its writer.
