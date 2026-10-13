# FONTS2 — tnum, MVAR, FeatureVariations and a host-stable KAT (rmbp-ledger B650, R124)

Peter, R124 (RULINGS.md:155): "make it the best." FONTSCRAFT (B640, folded at 255d7b32) vendored Inter, JetBrains
Mono and Noto Sans and instanced the variable faces. It left four things owed, and this arc does all four. The
OpenType readers are in `font_core` (`no_std`, the `shared-core` seam that both rings link). The figures rule is in
the kernel's one text seam, `video/text.rs` (CODEX §2: the kernel's own `wm` text path). Nothing here is flown: no
QEMU, no metal, no test suite (R107). The seat runs the fold.

## (a) tnum: tabular figures for readouts

DESIGN-LANGUAGE.md:176-177 says "Figures that change in place (clock, meters, progress, Activity) use `tnum`", and
:426 puts the clock in `type.body` with `tnum`. Callers pass only a string, a face and `bold`. The arc's FILES leave
out Settings and the Activity meters, and the clock's call line (`menubar.rs:2010`) is folded LINE-NEUTRAL. So the
rule lives in the seam:

- `text.rs::tabular` (file tail) is true when the run opens with a figure. Leading spaces are skipped; then it must
  be an ASCII digit, or `+`, `-` or `.` followed by a digit. `HH:MM`, `97%`, `13/16`, `11 min`, `13 px` and
  `1h 2m 3s` are readouts. `Untitled 2` and `Log In 3 times` are not.
- `tt::style(t, face, bold, s)` (text.rs:499-501) ORs `FEAT_TNUM` (font_core ui.rs:73, the bit FONTSCRAFT gave
  `tnum`) into the style when `tabular(s)` is true. `draw_text`, `draw_row`, `draw_with` and `advance` pass their
  string, so `fit` follows. `draw_cell` draws one grid glyph and passes `&[]`. The change is folded onto the
  existing lines, so the `tt` module keeps its line count.
- The clock (`menubar.rs:2010`) right-aligns `HH:MM` by `text::advance`. With tabular figures that advance no
  longer changes from minute to minute. The clock's code is untouched, and its doc line (menubar.rs:194) records
  this.
- Limit: a meter whose label comes first ("CPU 12%") stays proportional. Only a run that opens with a figure is
  classified. A per-call features flag would mean changing settings.rs and activity callers outside FILES, so it is
  owed to those owners.

## (b) MVAR: metrics per instance

OpenType 1.9, "MVAR — Metrics Variations Table". `var.rs::Mvar` (var.rs:373) reads the header: major 1,
valueRecordSize ≥ 8, an Offset16 ItemVariationStore. ValueRecords are tag + outer/inner. `delta(tag, coords)`
(var.rs:438) goes through the existing `ItemVariationStore::delta`. `Font.mvar` is parsed under `fvar`
(font.rs:229). `metric_delta` (font.rs:478) is 0 unless the face is instanced, so the default instance does not
change. `line_metrics` (font.rs:501) applies `hasc`/`hdsc`/`hlgp` to whichever source it chose, typo or hhea. That
is HarfBuzz's `hb_ot_metrics_get_position` rule. The new readers are `win_metrics`, `x_height`, `cap_height` and
`underline` (font.rs:527). `Engine::line_metrics` (ui.rs:355) reads the REGULAR instance at the size: `opsz` comes
from the size and `wght` is as loaded. So MVAR follows the optical size, and a bold run never changes its line height.

Measured on the pinned files with fontTools 4.66.1 (`tables.py`; instancer, see RESUME.md):

| face | MVAR tags | underline (pos, thick) wght 400 / 700 / 900 | xHeight 400 / 700 / 900 |
|---|---|---|---|
| Inter 4.1 | stro strs undo unds xhgt | (-348,140) / (-323,190) / (-302,232) | 1118 / 1118 / 1118 |
| Noto Sans 2.015 | stro xhgt | — | 536 / 546 / 546 |
| JetBrains Mono 2.304 | none | — | — |

None of the faces carries `hasc`/`hdsc`/`hlgp`, so the line box is the same at every weight. That is what the host
test and the kernel fixture check.

## (c) GSUB/GPOS FeatureVariations

OpenType 1.9, "OpenType Layout Common Table Formats" defines the FeatureVariations table, the ConditionSet table,
Condition Table Format 1 and the FeatureTableSubstitution table. Mechanism:

- `LayoutTable.fvars` (layout.rs:131) is set for a header minor ≥ 1 with a non-zero Offset32 at byte 10.
- `selected_variation(coords)` (layout.rs:153) returns the FIRST record whose ConditionSet holds. A null set always
  holds. `condition_set_ok` (layout.rs:397) handles format 1, `min <= coord <= max` in F2Dot14. An unknown format is
  not satisfied, which is how HarfBuzz reads it.
- `feature_table` (layout.rs:179) returns the alternate Feature table that the selected substitution names, or the
  FeatureList's own.
- `feature_lookups_at(.., coords)` (layout.rs:287) gives a feature's lookups at an instance. The shaper calls it with
  `font.coords()` at shape.rs:157/248/291 and complex.rs:390.

**None of the vendored faces carries FeatureVariations.** Inter 4.1's GSUB and GPOS are version 1.0, and so are
JetBrains Mono's and Noto Sans'. So the reader is shown on a CONTROL. `layout::FEATURE_VARIATIONS_PROBE`
(layout.rs:426) is a complete 142-byte GSUB 1.1, built and parsed back independently by `probe.py`. Its bytes equal
the const, as compared this session. In the probe, `ss01` is lookup 0 at the default instance and lookup 1 at axis-0
coordinates of 0.5 and above.

## (d) a KAT that does not depend on the host's fonts

`tests/kat.rs`, `tests/raster.rs` and `tests/name.rs` SKIPped every host font whose sha256 differed from the KAT
source, then failed on "nothing checked". The fix adds rows; every host row stays:

- `tests/data/kat_data.rs` gets the three vendored faces' `Kat` and `LayoutKat` rows, with repo-relative paths. They
  were generated by `gen_vendored.py`, which is `oracle/gen_kat.py`'s logic on fontTools 4.66.1.
- `tests/data/name_kat.tsv` gets their name rows (`gen_name_vendored.py`).
- `tests/common/mod.rs::load` resolves a path that has no leading `/` against the repository root.
- `kat.rs::tables_match_fonttools` asserts `vendored == 3`.

These rows check on every host. Strictly, the arc's FILES name only "the KAT test file FONTSCRAFT added"
(`tests/fontscraft.rs`). The tests that fail are kat.rs, raster.rs and name.rs, through `common::load` and the two
data files. Adding rows there is how this arc reads FILES. `raster.rs` and `name.rs` themselves are not edited.

The kernel fixture checks the loaded slots against `VENDORED_KAT` (text.rs tail). That is numGlyphs, upem and hhea:
Inter 2937 / 2048 / [1984,-494,0], JetBrains Mono 1743 / 1000 / [1020,-300,0], Noto Sans 3877 / 1000 / [1069,-293,0].

## The fixture: `tests fonts`

It is registered in `text.rs::ensure_tests` (text.rs:916) and cfg'd to the desktop builds, folded LINE-NEUTRAL onto
the `"font"` line. The body is at the file tail (`fonts2_fixture`, text.rs:1053), so no line above moves. A knob-off
build does not register it. Expected wire on an x86 `UNAOS_WC=1` boot with the vendored faces staged:

```
[fonts2] tnum clock=1 (<px>px) meters=5/5 control=differs ui_face=inter
[fonts2] mvar tags=stro,strs,undo,unds,xhgt line=(1984, -494, 0) one_line=1 underline/xh@100,400,700,900=...
[fonts2] gsub_fv probe ss01@0=Some([0]) @0.5=Some([1]) @1.0=Some([1]) selected@0.25=None faces_records=0 (...)
[fonts2] kat inter=ok jetbrains-mono=ok noto-sans=ok
:: FONTS2: tnum=ok(clock,meters=5) mvar=ok(instances=4) gsub_fv=ok(rules=1) kat=ok(vendored=3) -> PASS ::
```

The host tests are `tests/fontscraft.rs::mvar_metrics_move_with_the_instance` (the table above) and
`::gsub_feature_variations_probe`, which checks coordinates 0, 0.25, -0.5, 0.5 and 1.0, a missing axis, and
records=0 on the three faces.
