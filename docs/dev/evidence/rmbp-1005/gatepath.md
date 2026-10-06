# GATEPATH (rmbp-ledger B471) — one verb runs every host gate

**Finding (GATES-2026-10-06 F15).** Executors ran charter-check only; appearance-check, prefs-schema-check,
deps-audit, attrkeys-check and knob-hygiene ran when the seat remembered them; banner-cert ran only inside the
esp media builds, so a knob line whose banner has no registry row was learned an hour into the media build.

**Seam.** Host only: a shell function in `unaos/arroyo` (`gates`, `_gates_one`, `_gates_plant`, `gates_selftest`,
the `GATES_SET` list) and a `--registry` mode on `unaos/scripts/banner-cert.sh`. No kernel file, no knob.

**Milestones.**
- M1 — `./arroyo gates`: charter, arch, attrkeys, status, prefs, appearance, deps (`--check`), knob, verbs, banner
  (`--registry` against this run's `⚡ kernel features:` list), k8reach; one `gate=<name> rc=<n> <last line>` each,
  `GATES: <n> run <n> green -> PASS|FAIL`, exit = the worst rc; a missing script or python3 is rc 127, named.
  `--list` prints the set. `--selftest`: one plant per gate in a scratch copy of the working tree, caught =
  non-zero rc and the log names the plant. banner-cert `--registry [<list>]`: registry self-check, every row names
  a declared kernel feature, every listed feature has a row. Two stale rows removed (`vein`, retired by LUMENAPP
  under R82; `root-prefer`, the cargo features are `root-prefer-ahci`/`-sdhc`): they could never fire.
- M2 — PLAYBOOK §1/§2 and STRUCTURAL_GATES (GATE-GATEPATH) name the verb as the pre-commit leg.

**Witness (host).** `GATES: 11 run 10 green -> FAIL` on this tree (deps: objc2 0.6.4 behind 0.6.5, ARC DEPSLAG);
`GATEPATH selftest: plants=11 caught=11 -> PASS`.

**Measured with the seat's x86 metal feature line:** `banner-cert.sh --registry <line>` names 10 features with no
row: prefs_reset installdemo instgui ahciroot kvblank_trace lidsleep videoplayer ahci-write holocron svg — an
`esp-x86` whose banner carries them stops at NO VERDICT. Owed: their rows, measured on an artifact (R83).

**Stays owed.** GATE-LEDGER, GATE-BRANCH, fc2, lba32, test-roots, spec-roots, fixture-reachable stay in `check`
(they are not sub-2 s or need git history); a knob added to `_feats` without a banner row is caught only when its
knob line is passed to `gates`.
