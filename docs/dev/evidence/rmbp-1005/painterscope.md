# PAINTERSCOPE (B482) — appearance-check reaches every painter, not just `video/`

**Finding.** APPEARANCE2 (B473) left the gate's scope at `crates/kernel/src/video/`. Run over the rest of the
kernel it reads 1151 literals in 94 files (register / hardware constants), so the files OUTSIDE `video/` that put a
colour on the glass were unchecked: plant A2 (`0x0012_3456` in a non-video painter) was a miss. Seven kernel files
outside `video/` call a colour sink today: `console.rs` (shell console: 0x2D2B55 ground, 0xFFFFFF ink, 0x3A3868
scroll band), `ui_status.rs` (status strip, pulse panel, meter, LED ramp: 10 consts), `splash.rs` (boot splash:
4 consts, a 9-ray spectrum, 3 inline), `pal.rs` (pointer FILL / SHADOW), `selftest.rs` (offscreen pal vectors),
`main.rs` (desktop clears, already tokens) and `arch/aarch64/display_tegra.rs` (Orin format bars / quadrant card,
ladder + rastglass papers). The panic screen and login furniture live under `video/` (already scanned).

**The sinks** (what makes a file a painter). FrameBuffer methods taking a colour: `put_pixel`, `put_raw4`,
`fill_span4`, `fill_rows`, `fill_screen`, `fill_rect`, `draw_line`, `scroll_up` (called as `.name(`); the text
rasteriser (`video/text.rs`, `video/font.rs`): `draw_text`, `draw_row`, `draw_glyph_fb`, `draw_with`, `draw_cell`;
the Pal trait (`pal.rs`): `draw_pixel`, `clear_screen`, `draw_rect`, `fill_triangle` (+ `draw_line`/`draw_text`).

**Seam.** A host gate (`unaos/scripts/appearance-check.py`) over the kernel source; the palette stays
`video/theme.rs`. A PAINTER-FILE list, not an allowlist: `unaos/scripts/appearance.painters` names each file outside
`video/` that paints; the gate scans `video/` plus those files with the same colour shapes; a file outside `video/`
that calls a sink and is not listed is a finding (`unlisted painter`); a listed file that is missing is a finding.
A listed file's colour literals move to `theme.rs` tokens; its format constants are `appearance.allow` rows
(path relative to `video/`, so `../console.rs`). Pixels on this glass are `u32`: a literal suffixed `u64`/`usize`/
`i64` or held by a `const`/`static`/`let` typed `u64`/`usize`/`i64`/`isize` is an address/offset, not a colour
(this is what keeps the Tegra register offsets out without 20 allow rows). No knob, no new kernel file.

**M1** gate: painter list + sink scan + `unlisted` finding + the wide-int rule; `--selftest` gains the A2 plant
(a non-video painter with `0x0012_3456`: caught) and an unlisted-painter plant (caught), plus the u64 plants.
**M2** tree: the listed painters' colours move to theme tokens (`theme::painter` roles, `theme::testcard`, the
existing `theme::fixture` / console tokens); remaining format constants become allow rows; the gate reads 0.

**Witness (host).** `python3 unaos/scripts/appearance-check.py` -> `appearance-check: literals_outside_theme=0
files=0 certified=0 allow_rows=<n> allowed_hits=<n> stale=0 plants=<n> painters=7 unlisted=0` (rc 0). The kernel's
`tests appearance` line keeps printing `literals_outside_theme=0` (the certified constant, unchanged).

**Owed.** Dark-mode variants of the painter roles (they are LIGHT-only consts, not `Tok` rows — the console and
splash have one look); `termcolor.rs` is a colour SOURCE with no sink (xterm 256-cube arithmetic + parse fixtures),
not listed; Ring-3 painters (`vessels/`, `handlers/`) are out of this gate's scope.
