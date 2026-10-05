# BRIGHTSLIDER (rmbp-ledger B377, R89, flight 23) — the slider IS the brightness

Cut from hw-rmbp a60219de. x86 metal shape; no new knob (the gates are `wc` + `gmux_igd` + `intel-ivb`, all in the line).

## Finding (from the wire, f23-boots.log boot 2)

1. **The slider lagged the panel by 1.3 s.** `[backlight] level=11 … via=slider` at the press, then the press
   handler blocked in `persist(0)` (the PrefSet over the bus to UnaFS: `[prefs] set system.display.brightness=11`
   a second later) BEFORE `repaint()`; `[lag] click→shown ms=1478.0 … wm=1471.4 worst=wm`. The panel moved at the
   click, the knob moved 1.5 s later — "first click leaves the slider where it was".
2. **The slider was quantised to 16 steps** (`bright_at` → level 1..16 → `max*l/16`), and painted from the
   window's own copy `CUR.bright`, not from the register.
3. **Login wrote the stored level over the panel's own.** `[backlight] seed readback=160 max=1023 level=2`, then
   `load_for_login` staged the stored 2 and `LOGIN_APPLY` wrote `reg=127 … via=login` — the panel dimmed and the
   slider showed the stored step, not the glass.
4. **`tests brightstep` → `ran=0`**: the GLASSLAG `:: BRIGHTSTEP:` KAT lives inside `brightfloor`; no fixture was
   registered under that name.

## The seam

`video/backlight.rs` stays THE one writer (charter: Kernel — driver). The truth is the REGISTER (its last
readback); the settings slider is painted from it, so whatever moved the panel (click, keys, another client's
PrefSet) moves the knob. Principia's `display.brightness` (1..16) is untouched: the store records the level at
or below the register on every user move (debounced off the press path); it is no longer applied at login.

## Milestones

- **M1** backlight: `set_raw_via` (linear register write, floor-clamped, readback), `raw_for_pos`/`pct_of`
  (the one linear scale), `step_raw` (the keys move along the same scale: the next 1/16 grid point strictly
  above/below the register), `readback_now`.
- **M2** settings: the knob and the % are painted from the register; a press maps the track pixel linearly to
  the register, writes, repaints FIRST, and the store write is debounced 750 ms onto the service pass; the
  Left/Right keys step along the same grid. Witness `[backlight] slider click pos=<pct> -> reg=<n>
  readback=<n> slider=<pct> sync=ok`.
- **M3** login: read the register, seed the slider from it, write NOTHING (exception: a panel below the floor
  gets the floor, said on the line). `[backlight] login keep readback=<n> max=<m> slider=<pct> stored=<l> wrote=0`.
- **M4** `tests brightstep`: five positions through the click path, each read back, the painted knob compared,
  the prior register restored. `:: BRIGHTSTEP: steps=5 readback_ok=5 slider_sync=ok keys=ok driver=gmux -> PASS ::`.

## Witness lines (boot 24)

- at login: `[backlight] login keep readback=160 max=1023 slider=16% stored=2 wrote=0` and NO `via=login` line
- each slider press: `[backlight] slider click pos=<pct> -> reg=<n> readback=<n> slider=<pct> sync=ok`, then ≥ 750 ms
  later one `[prefs] set system.display.brightness=<l>` per burst
- `tests brightstep` → `:: BRIGHTSTEP: steps=5 readback_ok=5 slider_sync=ok keys=ok driver=gmux -> PASS ::`
- Peter: the knob lands where he clicked, at the click; at login the slider shows the glass.

## Owed

- The PrefSet over the bus takes ~1 s on the bench (the UnaFS write on the render thread); moved off the press,
  not cured.
- Principia's key stays 1..16: a finer stored value (percent) is a schema change for the seat to rule on.
- aarch64 desktops: the same code with the simulated register (`driver=sim`); unflown.
