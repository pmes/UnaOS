# APPEARANCE (rmbp-ledger B408, MACPARITY rows 21/22) — Light and Dark, an accent, a highlight

## Design (written before the code)

**Finding.** `video/theme.rs` carries ONE palette (crispy, 25 roles) as `pub const`s read at 645 sites; a `const`
cannot change at run time, so no dark set or accent is possible without a token read. The audit
(`unaos/scripts/appearance-check.py`, comments and strings stripped; a colour = a six-digit hex literal or an
eight-digit one with top byte 00/FF, not an all-0/F mask on `&`/`|`/`^`) counts **before: 107 colour literals in
23 files under `video/` outside theme.rs** (activity 3, ceramic 12, cursor 2, desktop_uefi 1, dock 12, facet 9,
facet_anim 5, fbcon 4, fileview 3, knurl 15, mod 1, paper 17, pulsewin 3, quarry/ops 1, screen 1, shotsel 2,
text 2, vperf 1, wcf 2, winmenu 1, winsnap 2, witness 2, wm 6; three of them are sentinels, not colours, and the
rule is tightened to drop 0xFFFF_xxxx). No `system.appearance.*` key exists; no Settings pane names appearance.

**The seams (R79).**
- THE PREFERENCE is Principia's: `system.appearance.mode` (light / dark / auto), `system.appearance.accent`
  (eight names, ours), `system.appearance.highlight` (`accent` or one of the eight) — three `prefs_core::schema`
  rows; the parse rules and the auto-by-clock rule are `prefs_core::appearance` (shared-core, both rings).
- THE PALETTE is theme.rs (the brief: both sets live there): a `Tok` enum, a LIGHT row (the kit's crispy values,
  unchanged) and a DARK row (our neutral greys), eight accent colours; every role is read through a token fn
  (`theme::chrome_face()` … `theme::accent()`), the kit consts stay as the Light set's provenance.
- THE REPAINT is the wm's full-repaint path, once: a switch bumps `theme::epoch()`, which (a) seals into every
  strip signature (`strip::seal`), so the bar, dock and menus repaint, (b) joins the face epoch in
  `quarry::live::font_repaint_pass`, so every kernel window that caches its pixels repaints, and (c) damages the
  panel (`wm::damage_intersecting`), so the chrome and desktop recomposite.

**Milestones.** M1 prefs_core (appearance module, three schema rows, `settings.tab` 0..5, PREFS-SCHEMA.md, host
tests); M2 tokens: theme.rs `Tok`, LIGHT/DARK, accents, the role fns, every consumer converted, every literal in
`video/` a token (fixtures included: `theme::fixture`), `AUDIT_LITERALS_OUTSIDE = 0` certified by the script;
M3 apply: the login's load, the bus change, the auto clock, the one repaint; M4 the Appearance tab in Settings
(Light/Dark/Auto segments, accent swatches, highlight swatches, the auto rule said in the pane); M5 the witness.

**Witness lines (the next flight reads).**
- `[appearance] mode=<light|dark|auto> dark=<0|1> accent=<name> highlight=<name> via=<login|settings|bus|clock> repaint_ms=<n>`
  (one per switch).
- `:: APPEARANCE: literals_outside_theme=0 tokens=<n> sets=2 mode=<m> accent=<name> repaint_ms=<n> -> PASS ::`
  from `tests appearance` (switch to dark and back, measured; the user's choice restored).

**Owed.** Ring 3 (quartzite / the host apps) does not yet read `system.appearance.*`; the kit json has no dark set
(the Dark row is ours, in theme.rs). Auto uses the RTC hour (19:00–07:00 dark) until NETCLOCK gives a real time.
Translucency (MACPARITY 21 (c)) stays owed. The pointer keeps its two colours in both sets (PA38).
