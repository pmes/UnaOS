# LAUNCHERPREFS (rmbp-ledger B451) — the Launcher's recency through the one settings writer

**Finding (ARCHREVIEW F7, MED).** `video/launcher.rs` (LAUNCHER B417) built `<home>/settings/launcher` with a
`PrefTree` and wrote it by raw `mt.unlink`/`create`/`write`: no `.new` swap, no read-back, no `# auto-saved`
line, not through `prefs`. Worse than cosmetic: `prefs::load` already reads every file under `settings/`, so the
launcher's `[launcher] rNN` keys sat in Principia's tree too, and any whole-store `prefs::save()` (a load-time
clamp, a migration, the fixture) rewrote `settings/launcher` from the tree's stale copy — two writers, one file.

**The seam (R79: a preference is Principia's).** `shared-core` — `prefs_core` (declare, files, wire) through the
kernel's `prefs.rs`, the one store writer. No new file, no new knob.

* The Launcher declares its stanza ONCE, as the kernel (PrefDeclare's shape, `prefs_core::declare`):
  `app.launcher.recent` — `str:4096`, default `""`, doc "The Launcher's recent picks, newest first, one token
  per line". `files::domain_of("app", "launcher.recent")` = `launcher`, so the file is still `settings/launcher`.
* Write: `prefs::set("app", "launcher.recent", <tokens joined by newline>)` — the declared clamp, the swap,
  the read-back, the auto-saved line, PrefChanged. An unchanged list is not re-written (persisted_set's rule).
* Read: `prefs::get("app", "launcher.recent")` after `prefs::ensure_loaded()` — the tree, never the file.
* Migration (one-shot): a tree holding the old `launcher.rNN` keys (the B417 file, already loaded by prefs) is
  read in order, the `launcher` namespace retired from the tree (`prefs::retire`), and the list written as the
  new key — so the next file holds only `[app] launcher.recent`. Line: `[launcher] recency migrated rNN -> app.launcher.recent n=<n>`.
* A token holding a newline is not noted (it could not round-trip); the list is cut from the oldest end to fit 4096.

**Milestones.** M1 prefs.rs tail: `declare_kernel(stanza)` (the DECLARED registry, the same insert PrefDeclare
makes) and `retire(ns)`. M2 launcher.rs: lru_load / lru_save through prefs, the raw writer deleted, the
migration, the fixture's `recency=` word.

**Witness.** Metal boot, after a Cmd-Space pick: `[prefs] declared app.launcher keys=1 -> settings/launcher (kernel)`,
`[prefs] set app.launcher.recent=... ok=1`, `[prefs] saved settings/launcher keys=1 by=the program launcher`,
`[launcher] saved recent=<n> via=prefs ok=1`. Typed `tests launcher`:
`:: LAUNCHER: programs=<n> files=<n> settings=<n> math=ok open=<w> recency=via-prefs ms=<n> -> PASS ::`.

**Owed.** PREFSCAP (B454): DECLARE bound to its caller — the kernel's launcher stanza is declared by the kernel
(`declare_kernel`, no bus frame), so it is the caller-bound case's "kernel" row already. GATE-ARCH's `settings`
row (the raw-writer baseline) shrinks by launcher.rs at the seat's fold.
