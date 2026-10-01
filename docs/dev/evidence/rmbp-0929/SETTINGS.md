# SETTINGS (R75) — the settings window

## Design
Finding: every knob already exists but is a typed verb or a key: BRIGHTKEYS `brightkeys::step`/level 0..16, VOLKEYS `status::volkey_usage` + `hda::vol::apply`, DIMIDLE `UNAOS_IDLE_MIN` (build constant), WALLPAPER `wallpaper::cmd`, TPSPEED divisors (`ehci/mod.rs` `TP_MT_DIV_*` consts), the login set-password screen `login::open_set_password`.
Mechanism: `video/settings.rs` (FILEVIEW pattern, gated like fileview, owner KERNEL_OWNER_BASE+7). New runtime seams: `dimidle::set_idle_min`/`idle_min` (runtime atomic over the build value), `brightkeys::set_level`/`level`, `status::set_volume`, `ehci::tp_speed_set` (`tp_scale` now reads `tp_div_low()/tp_div_high()`; normal = the old constants). Entry: `settings` verb (shell.rs) and a Crystal row (`Verb::Settings`, crystal.rs ROWS, after Lock behind a separator, `login` builds). Routes chained in `quarry/live.rs` (key, press, service).
Persistence: `<home>/.settings` (`key=value`: brightness, volume, mute, idle_min, pointer, wallpaper), written through the mount table (KERNEL_PRINCIPAL, unlink+create+write like TEXTEDIT save); loaded once per login by `settings::service` (`[settings] loaded n=`), only present keys are applied.
Milestones: M1 seams+window+persistence+verb+crystal row+fixture+spec pin (single commit). Omitted: Clock 24h/12h (CLOCKBAR glyph path is not a runtime switch); mouse drag (press-to-set only).
Witness: `:: SETTINGS: controls= loaded= saved= -> PASS ::` on open, on every save, and from the `settings` tests fixture. Pin: x86-wc.spec REQUIRE/FORBID. No new UNAOS_* knob.

## Written
Boot 17: `tests settings` (or the `settings` verb / Crystal > Settings) shows `[settings] open win=`, then `:: SETTINGS: controls=9 loaded=<n> saved=<0|6> -> PASS ::`; each control prints `[settings] <name>=<value> applied=<0|1>` followed by `[settings] saved path=/home/<user>/.settings` and the witness.
