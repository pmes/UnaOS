# FONTS2 (rmbp-ledger B650, R124) — RESUME after a seat stop (model), 2026-10-14

Branch `exec-rmbp-fonts2`, worktree `/home/user/exec/fonts2`, cut from `8da376cb`. WIP commit `191ac598`
("wip: FONTS2 — stopped by the seat (model), resumes on Opus"). **Nothing has been compiled.** No linters, no gates run.
Last compile command: none.

## What the arc file does not say (measured at 8da376cb, by `tables.py` in this directory, run with `python3 -I`)

- **None of the three vendored faces carries GSUB or GPOS FeatureVariations.** Inter 4.1's GSUB/GPOS headers are
  version 1.0 (`featureVariations=0`); JetBrains Mono and Noto Sans likewise. So (c) is a reader proven on a
  CONTROL: `layout::FEATURE_VARIATIONS_PROBE` (a complete synthetic GSUB 1.1, 142 bytes, built and parsed back
  independently by `probe.py`): `ss01` is lookup 0 at the default instance and lookup 1 at axis-0 coordinate ≥ 0.5.
  The kernel's `gsub_fv=ok(rules=N)` must count the probe (faces contribute 0) and say so in a detail line.
- **MVAR in the tree:** Inter carries `stro, strs, undo, unds, xhgt` (5 records); Noto Sans `stro, xhgt` (2);
  JetBrains Mono has no MVAR. No face carries `hasc/hdsc/hlgp`, so line boxes do not move with wght on these faces.
  fontTools instancer (same directory, see the stop log): Inter underline position/thickness at wght 400/700/900 =
  (-348,140)/(-323,190)/(-302,232), xh 1118 at all three; Noto Sans xh 536 at 400, 546 at 700/900. These are the
  numbers the host test and the kernel's `mvar=ok(instances=N)` detail line should reproduce.
- fontTools 4.66.1 IS installed on this host (`python3 -c "import fontTools"`), so `oracle/gen_kat.py` can run.
- Header facts for the kernel-side KAT: Inter numGlyphs 2937 upem 2048 hhea [1984,-494,0]; JetBrains Mono 1743 /
  1000 / [1020,-300,0]; Noto Sans 3877 / 1000 / [1069,-293,0]. All three have fsSelection 0xC0 (USE_TYPO_METRICS),
  typo == hhea.
- `tests fonts` registration needs NO edit of `tests.rs`: `tests.rs:152` already calls
  `crate::video::text::ensure_tests()`, so `register("fonts", …)` goes beside `register("font", fixture)` in
  `text.rs::ensure_tests`.
- The failing host KATs are `tests/kat.rs` (KATS/LAYOUT_KATS from `tests/data/kat_data.rs`), `tests/raster.rs`
  (iterates KATS) and `tests/name.rs` (`tests/data/name_kat.tsv`): each SKIPs every host font whose sha differs and
  then asserts ≥ 1 checked. The fix is additive rows for the vendored faces with REPO-RELATIVE paths plus a
  `common::load` that resolves a relative path against `CARGO_MANIFEST_DIR/../../../..`; the host rows stay.
- The clock in `menubar.rs:2010` draws `HH:MM` through `text::draw_row(.., false, FACE)` and right-aligns by
  `text::advance`; the API has no features parameter, and Settings/Activity are outside FILES. Hence the design
  below puts the tabular rule in `text.rs`, where every caller passes the string.
- Coordinator corrections received: (1) the pre-commit leg is `cd unaos && ./arroyo gates > gates.log 2>&1;
  echo rc=$?` (paste `GATES: 17 run 17 green -> PASS`), rule 6's ban on `./arroyo gates` withdrawn; QEMU, cargo
  test, `./arroyo check` and suites stay forbidden. The three linters of rule 12 are still cheap scripts.

## DONE (committed in 191ac598, uncompiled)

- `font_core/src/var.rs`: `Mvar` (parse: major 1, valueRecordSize ≥ 8, Offset16 store; `record_count`, `tags`,
  `has`, `delta(tag, coords)` through `ItemVariationStore::delta`), tag consts `MVAR_HASC … MVAR_UNDS`
  (lines ~366–445, before `Gvar`).
- `font_core/src/font.rs`: `pub mvar: Option<Mvar>` parsed under `fvar`; `metric_delta(tag)` (0 unless instanced);
  private `metric(v, tag)` = round(v + delta); `line_metrics()` now applies hasc/hdsc/hlgp to WHICHEVER source
  (typo or hhea) was chosen — HarfBuzz's `hb_ot_metrics_get_position` rule; new `win_metrics()`, `x_height()`,
  `cap_height()`, `underline()`. Default instance unchanged byte for byte (deltas are 0 when `!is_instanced()`).
- `font_core/src/layout.rs`: `LayoutTable.fvars` (header minor ≥ 1, Offset32 at byte 10, non-zero);
  `feature_variation_count`, `selected_variation(coords)` (first record whose ConditionSet holds; null set = holds),
  private `feature_substitution`, `feature_table(fi, subst)` (alternate Feature table from the
  FeatureTableSubstitution, offset from ITS start), `feature_lookup_indices`; `feature_lookups` = `_at(.., &[])`;
  `lookups_for` and `collect_lookups` use the default instance's substitution; `condition_set_ok` (format 1 only,
  unknown formats = not satisfied); `pub const FEATURE_VARIATIONS_PROBE`. Spec citations: OpenType 1.9 "OpenType
  Layout Common Table Formats": FeatureVariations table, ConditionSet table, Condition Table Format 1, 
  FeatureTableSubstitution table.
- `font_core/src/shape.rs` (3 sites) and `complex.rs` (1 site): `feature_lookups_at(.., font.coords())`.
- `font_core/src/ui.rs`: `Engine::line_metrics(role, size)` reads the REGULAR instance at that size (opsz from
  the size), so MVAR follows opsz and a bold run never changes the line height; module doc paragraph.
- Scratch → this directory: `probe.py` (builds + independently parses the probe), `tables.py` (table census),
  `gen_vendored.py` + `vendored_kat.rs` (fontTools KAT rows for the three vendored faces, repo-relative paths),
  `gen_name_vendored.py` + `vendored_names.tsv` (name KAT rows). `vendored_kat.rs` is a whole file: take its
  `Kat {…}` entries (lines 13–285) and `LayoutKat {…}` entries (291–411) only.

## NOT DONE — the exact next steps, in order

1. **(a) tnum — `unaos/crates/kernel/src/video/text.rs`.** Design: a FIGURES RULE at the kernel seam, `pub fn
   tabular(s: &[u8]) -> bool` = the first non-space byte is an ASCII digit, or a `+`/`-`/`.` followed by one (a run
   that OPENS with a figure is a readout: `HH:MM`, `97%`, `13/16`, `11 min`, `13 px`, `1h 2m 3s`; `Untitled 2`
   and `Log In 3 times` stay proportional). Apply in `tt::style(t, face, bold)` → add an `s: &[u8]` parameter and
   OR `FEAT_TNUM` into `st.features` when `tabular(s)`; call sites `tt::draw_text/draw_row/draw_with/advance`
   (not `draw_cell`). DESIGN-LANGUAGE.md:176-177 ("Figures that change in place (clock, meters, progress,
   Activity) use `tnum`") and :426 (the clock in type.body with tnum). `menubar.rs`: ONE `///` doc line at
   `CLOCK_GLYPHS` (line ~194) saying the clock is tabular by `text::tabular` and its width no longer moves per
   minute — comment only, no code (the Face enum and the clock's call line untouched).
2. **Kernel fixture `tests fonts` in text.rs** (register in `ensure_tests` beside `"font"`; cfg'd like `fixture`,
   SKIP line on a build without the desktop):
   `[fonts2] …` detail line(s), then
   `:: FONTS2: tnum=ok(clock,meters=5) mvar=ok(instances=4) gsub_fv=ok(rules=1) kat=ok(vendored=3) -> PASS ::`.
   - tnum: `advance(b"11:11",false,Face::Chrome) == advance(b"00:00",..)` (clock); meters = the five pairs
     ("11%","00%"), ("11/16","00/16"), ("11 min","00 min"), ("11 px","00 px"), ("1h 11m 11s","0h 00m 00s") equal;
     CONTROL: ("x 11111","x 00000") must DIFFER (the rule discriminates; Inter's proportional 1 is narrower).
   - mvar: `tt::with(|t| t.eng.primary_instance(&Style::new(Role::Sans,false,ui_px).with_weight(w)))` for w in
     100/400/700/900: `line_metrics()` equal across all four (instances=4) and, when the face `mvar.is_some()`,
     `underline()` or `x_height()` differs at ≥ 1 non-default weight (Inter: underline moves).
   - gsub_fv: `font_core::layout::Gsub::parse(FEATURE_VARIATIONS_PROBE)`: `feature_lookups_at(&[*b"DFLT"],
     *b"ss01", &[0.0]) == [0]` and at `&[1.0]` == `[1]`, `feature_variation_count()==1`; plus the sum of
     `feature_variation_count()` over the loaded faces' gsub/gpos (0 today) printed in the detail line.
   - kat: a `const VENDORED_KAT: [(&str, u16, u16, [i16;3]); 3]` (slot name, numGlyphs, upem, hhea) from the
     header facts above, matched against `t.eng.slots()` by name; vendored = matches.
   - `[font] face=` witness line: append `mvar=<tags>` and `gsub_fv=<n>` fields only if cheap; optional.
3. **Host tests — `tests/fontscraft.rs`** (the file the arc names): `mvar_metrics_move_with_the_instance`
   (Inter underline at 400/700/900 = the fontTools numbers above; Noto xh 536→546; line_metrics unchanged across
   wght; `Engine::line_metrics` equal for bold/regular) and `gsub_feature_variations_probe` (the probe at
   coords 0 / 0.5 / 1.0 / -0.5, `selected_variation`). **KAT (d):** insert the vendored `Kat`/`LayoutKat`
   entries into `tests/data/kat_data.rs` (before each `];`), append `vendored_names.tsv` rows to
   `tests/data/name_kat.tsv`, make `tests/common/mod.rs::load` resolve a path without a leading `/` against
   `concat!(env!("CARGO_MANIFEST_DIR"), "/../../../..")`, and in `kat.rs::tables_match_fonttools` count
   `vendored` (relative paths) and `assert_eq!(vendored, 3)` while keeping `checked`. Note `gen_kat.py`'s FONTS list
   gains the three relative entries (or point the header comment at `gen_vendored.py`).
4. **Ledger** `docs/dev/OS/rmbp-ledger.md`: NEW row B650 after B649 (line ~672 region, same 7-cell shape as B640
   at line 663): owner `shared (video/text.rs, libs/text/font_core)`, flies-on `rmbp`, status
   `fixed-unflown — FONTS2 on exec-rmbp-fonts2 @the commit carrying this row (parent 8da376cb): compiled (<x86
   line> rc=0; cargo check --tests rc=0), proof owed to the fold` + one sentence; witness `x86 QEMU, then the rmbp
   bench`; no `|` in a cell, no proven/verified/confirmed. Evidence `docs/dev/evidence/fonts2/FONTS2.md` (write it:
   the four mechanisms, the measurements above, the expected wire).
5. **Compile ONCE** (the arc's two commands; shared target dir, never rm), then `cd unaos && ./arroyo gates >
   gates.log 2>&1; echo rc=$?` (paste the GATES line), the three linters, commit with the DESIGN in the body,
   `Co-Authored-By` of the model that did the work. Do not push.
