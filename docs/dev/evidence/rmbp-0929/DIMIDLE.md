# DIMIDLE — idle screen blanking (both desktops)

**Finding.** No idle blank existed (grep of `last_input|idle|IDLE_MIN|backlight` found only the cursor's 1.5 s
auto-hide, `pal.rs:329`). gmux in this tree (`drivers/gpu/igpu.rs`, `gmux_igd`) is a MUX switch with no brightness
port, so no backlight step: the blank is a black panel.

**Mechanism.**
- Idle clock + wake + swallow: `video/dimidle.rs` `gate()`, called from `pal::pop_event` (the one drain every
  consumer shares, both arches) — stamps `LAST_ACTIVITY_MS`, wakes, returns true for the waking Key/Button/Wheel
  (and that key's release). Pointer motion wakes but passes.
- Blank: `dimidle::service()` (from `bootpace::service_dump`, every service lane) fills the panel black and sets
  `BLANKED`; `video/mod.rs panel_refuse_term` gains the term "idle-blank", so `wm::composite` and the cursor decline.
- Wake: `service()` fills `DESKTOP_BG`, `wm::damage_intersecting` over the whole panel, `wm::composite()`.
- Knob: `UNAOS_IDLE_MIN` (default 10, 0 = never), `option_env!` + `parse_min` const fn; named in `unaos/arroyo` beside `UNAOS_WC`.
- Gate: `any(all(x86_64, wc), all(aarch64, desktop_firmware))`.

**Milestones.** M1 module + hooks + knob; M2 fixture + spec pins.

**Witness.** `:: DIMIDLE: idle_min=N blanked_at_ms=<t> woke_at_ms=<t> wake_key_swallowed=1 -> PASS ::` (1 s threshold
override, blank, inject `~`, verify refuse term + swallow count + unblanked). Pins: `unaos/scripts/specs/x86-wc.spec`.

## Written
All of the above. Not verified by a compiler (R76). Fixture starts at 12 s uptime.
