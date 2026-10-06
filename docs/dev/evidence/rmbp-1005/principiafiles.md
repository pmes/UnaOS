# PRINCIPIAFILES (rmbp-ledger B445) — ARCHREVIEW F1: one settings store

**Finding.** Since SETTINGSFILES (B407, R98) the kernel keeps the store as `<home>/settings/<domain>` and migrates +
DELETES `<home>/.config/unaos/preferences.toml`; `handlers/principia` (lib.rs:85, prefs.rs) still loaded and
rewrote that one file — two stores, and a change made through Principia's own handler landed in a file the kernel
no longer reads (R79: one store; a preference is Principia's).

**Seam.** `unaos/libs/sys/prefs_core` (shared-core, both rings link it). `files` already owns the domain rule
(`domain_of`), the split (`part`/`split`/`merge`) and the file text (`render`, `stamp_of`); this arc adds the two
rules both sides were spelling separately: `files::LEGACY` (the old single file, relative to the home — the
migration both rings run) and `files::writer_of` (the `by` of the auto-saved line). No new kernel file.

**Milestones.**
- M1 — prefs_core `files::LEGACY` + `files::writer_of`; the kernel's `legacy_path`/`by_of` read them; the kernel's
  one-shot import of the two retired private stores (`<home>/.settings`, `<home>/.dock`, B300 — every login since
  B300 has run it and deleted them) is retired, so the three F1 dotslash keys leave GATE-ARCH's baseline.
- M2 — `principia::prefs::PrefStore` is the FOLDER: reads every `<d>` (and an orphaned `<d>.new`) through
  `PrefTree::parse` (the kernel's reader), a refused file holds that domain's writes (the kernel's rule), a set
  rewrites ONLY its domain by the swap (`<d>.new`, fsync, read back, parse = part, rename) with `files::render`;
  the legacy file is migrated once by the writer of record (`Principia`), never by a reader (Vein, Quartzite);
  `toml` leaves principia's dependencies (the lenient second parser is gone).
- M3 — host cross-tests: Principia → kernel-side reader → re-render byte-identical; a kernel-rendered folder →
  Principia → save byte-identical; migration; held domain; Vein's tests on the folder.

**Witness.** Host: `cargo test -p principia -p prefs_core -p vein` exit 0. Metal: none new — the kernel's lines are
unchanged (`[prefs] saved settings/<d> keys=<n> by=<who>`, `[prefs] migrated preferences.toml -> settings/<n>
files n=<n> (R98)`); the retired import's `[prefs] migrated=<n> from=…` line no longer prints.

**Owed.** Principia has no `PrefDeclare` registry, so an `app.<name>` key's declared doc comment is the kernel's
alone (a Principia save of that domain writes it without the comment); reload-on-external-change (the kernel's
delete-resets-domain watch) is not mirrored on the host; GATE-CHARTER's registry rows for `.settings`, `.dock`
and `.config/unaos/preferences.toml` go stale with M1 (seat removes them at the fold).
