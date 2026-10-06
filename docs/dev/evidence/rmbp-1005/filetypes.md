# FILETYPES (rmbp-ledger B423) — the Be inheritance B3: the FileTypes registry

**Finding.** The type database already exists and already is attributes (FILETYPE B307, `fs/assoc.rs`):
`/system/types/<mime with '.'>` objects carrying `una:opener` / `una:icon` / `una:name`, seeded at BOOT
(`users::boot80_root_and_seed`, a boot step) from `assoc::BUILTIN` — a type→opener TABLE IN CODE (OPENERS
B379 grew it to 26 rows). APPRES (B398) adds `una:apps` on type objects for sighted ring-3 programs, but the
kernel's own apps' doc types (facet, quarry) were never published and the openers that are not windows
(textedit, fileview, markdown, json, play, launch, linux) had no resources at all. `una:type` already is THE
type (ATTRCOLUMNS's `refresh_in` writes the sniff only when the source is not the attribute — a user's edit
sticks). There is no Settings pane and no Open With.

**The seam (R79).** ONE store, not two: the B307 database IS the registry — moved to
`/system/filetypes/<mime, '/' as '-'>` and re-keyed Be's way (`una:description`, `una:extensions`, `una:icon`,
`una:preferred`). The kernel is the fulfiller; the attributes are the store; the type→program table in code
is deleted. Programs register by their RESOURCES: every kernel opener gets a resource block
(`unaos/res/<id>/app.res` + `icon.svg`, packed by `una-res pack`, compiled into APPRES's built-in list) that
declares its doc types; ring-3 programs keep APPRES's `una:apps`. A preferred app is a SIGNATURE
(`org.unaos.facet`); the opener id the dispatch runs is the registrant's key (built-in) or path (ring 3).

Resolution (`assoc::opener_for_in`): the file's `una:preferred` (Be's per-file PREFERRED_APP; a signature, or
the legacy opener id/path) → the type object's `una:preferred` → the FIRST REGISTRANT (built-ins in APPRES
order, then `una:apps`) → `none`. The registry is not needed to open a file (a FAT root, a first boot before
login): the registrants answer from the compiled-in resources.

Descriptions and icons of the types are type FACTS (Be's MIME database shipped them), kept as
`assoc::TYPE_FACTS (mime, icon, description)` with no opener column; extensions are `filetype::EXT_TABLE`'s
(the one place a name decides a type), listed onto the registry.

**Milestones.**
- M1 — registrants: resource blocks for textedit, fileview, markdown, json, play, launch, linux (+ facet
  declares SVG); APPRES built-ins in registrant order; `appres::registrants(mime)`.
- M2 — the registry: `/system/filetypes/`, the new keys, `TYPE_FACTS`, resolution by signature, the BUILTIN
  opener table deleted; built at LOGIN (`login ok` owes it, the device-service pass writes it in one
  transaction; elsewhere inline), the boot-time `assoc-seed` step removed (R80/R93). A missing key is
  filled, a present one never overwritten (the user's amendment sticks).
- M3 — Quarry's Open With: a context-menu row opening a sub-list of every registrant; a pick opens with it.
- M4 — Settings tab 7 "File Types" (index 6; APPEARANCE B408 holds index 5): the type list (icon glyph,
  description, MIME, extensions, preferred app); a press on a row cycles its `una:preferred` through the
  registrants; Prev/Next pages. `system.settings.tab` max 6 (seven tabs, 0-based).
- M5 — `tests filetypes` and the witness; MACPARITY row 29.

**Witness** (`tests filetypes`, R80: only when asked):
`:: FILETYPES: types=<n> preferred=<n> resolved=<ok|why> -> PASS|FAIL :: registry=/system/filetypes source=<db|registrants> sticky=<ok|why> openwith=<n>`
and at login `[filetypes] built at=login dir=/system/filetypes created=<n> filled=<n> types=<n> ms=<n>`.

**Owed.** Extensions as the sniff's source (the registry lists them; `by_extension` still reads
`EXT_TABLE` — a user-added extension does not yet type a file); attribute templates (Be's per-type
attribute list for Tracker); a per-type icon picker (the pane shows the glyph name); the seat adds
§16 B3's "built" note to MACPARITY at the fold (this tree's MACPARITY predates §16).
