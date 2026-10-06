# APPEARANCE (rmbp-ledger B408, MACPARITY rows 21/22) — Light and Dark, an accent, a highlight

## Design (written before the code)

**Finding.** `video/theme.rs` carries ONE palette (crispy, 25 roles) as `pub const`s read at 645 sites; a `const`
cannot change at run time, so no dark set or accent is possible without a token read. The audit
(`unaos/scripts/appearance-check.py`, comments and strings stripped; a colour = a six-digit hex literal or an
eight-digit one with top byte 00/FF, not an all-0/F mask on `&`/`|`/`^`) counts **before: 103 colour literals in
21 files under `video/` outside theme.rs** (activity 3, ceramic 12, cursor 2, desktop_uefi 1, dock 12, facet 8,
facet_anim 6, fbcon 4, fileview 3, knurl 15, mod 1, paper 17, pulsewin 3, quarry/ops 1, shotsel 1, text 2, wcf 2,
winmenu 1, winsnap 2, witness 2, wm 5). **After: 0** (`appearance-check: literals_outside_theme=0 files=0
certified=0`): 32 live tokens (`theme::Tok`), 16 same-in-both consts (console, pointer, Pulse, Facet's media
ground, desktop/panel backdrop), and the selftest vectors as `theme::fixture` (goldens of ceramic/knurl/paper,
synthetic window surfaces). 379 role reads in 24 files became token calls (`theme::CHROME_FACE` →
`theme::chrome_face()`); the material selftests (ceramic, knurl, paper) keep the kit consts on purpose.
No `system.appearance.*` key exists; no Settings pane names appearance.

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

## Built (exec-rmbp-appearance2, cut from 2574cd13)

- M1 `prefs_core::appearance` + three schema rows + `settings.tab` 0..5; PREFS-SCHEMA.md regenerated; `cargo test -p prefs_core` exit 0.
- M2 `theme::Tok` (32 tokens), `LIGHT` / `DARK`, `ACCENTS` (crispy teal moss amber clay rose violet slate), `accent()`,
  `selection()`, `theme::fixture`; 379 role reads → token calls; 103 → 0 literals.
- M3 `video/appearance.rs` (`CHARTER: Principia — shared-core`): the login's load (`settings::load_for_login`), another
  client's PrefChanged (`settings` bus pass), the Settings choice (latched, stored + applied on the service pass), the
  auto clock (once a minute). One repaint: `theme::set` → `quarry::live::font_repaint_pass` (now also keyed on the
  theme epoch) → `wm::damage_intersecting(panel)`; `strip::seal` carries the epoch so the bar/dock/menus repaint.
- M4 Settings > **Appearance** (sixth tab; the window 520 → 600 logical px): Light/Dark/Auto segments, eight accent
  swatches, nine highlight swatches (the first follows the accent), a selection sample, the auto rule said in the pane.
- M5 `tests appearance`: switches live to the other set with another accent and back (the store is not written).

**Next flight reads.** Settings > Appearance > Dark: `[settings] appearance_mode=dark applied=1` then
`[appearance] mode=dark dark=1 accent=crispy highlight=accent via=settings repaint_ms=<n>`, the chrome, bar, dock and
Settings window go dark at once; a swatch: `[appearance] … accent=<name> via=settings`; after a reboot and login:
`[appearance] mode=dark … via=login`; `tests appearance` →
`:: APPEARANCE: literals_outside_theme=0 tokens=34 sets=2 mode=<m> accent=<name> repaint_ms=<n> -> PASS ::`.

**Owed (said, not hidden).** Kernel windows outside Quarry's repaint list (dialog, toast, window menus, the console)
take the new set on their next own repaint; Ring-3 windows (Lumen, host apps) do not read `system.appearance.*` yet;
the material selftests (ceramic, knurl, paper) and the installer's paper read the kit's Light consts; the console,
the pointer, Pulse and Facet's media ground are the same in both sets; the Appearance tab is mouse-only; the login
screen after a Log Out keeps the last session's set until the next login loads its own; translucency (21c).
