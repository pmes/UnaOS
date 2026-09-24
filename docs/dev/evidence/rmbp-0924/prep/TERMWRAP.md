# TERMWRAP — prep

## The finding

Flight 14: "no word wrap in terminal" — the shell window's edit line (and scrollback) does not
wrap at the window width; a ~200-character `mv` line ran off the window. Peter: the cloud has
about $41 of credits left until next Tuesday's refresh; we "will not be able to finish — you are
setting things up and doing what you can to prep so we can hit the ground running."

## Mechanism

The console model is flat: one `String` per logical line, byte offset == cell column, no visual
row exists anywhere.

- `console.rs:237-264` `draw_prompt_line` — one `pal.draw_text(input_x, prompt_y, &current_input,
  …)` call, no width check; a line longer than the window paints past the edge, no second row.
- `console.rs:271-287` `draw` — one `history` entry = one `pal.draw_text` at one `y`, `y +=
  m.line_h` per *entry*, not per visual row.
- `console.rs:218-230` `history_rows`/`top_y` — count scrollback ENTRIES that fit (`usable /
  m.line_h`); no notion an entry could span >1 row.
- `console.rs:337-354` `cell_at` — `vrow = (ly - top) / m.line_h` indexes `history[skip + vrow]`
  directly: one history index per line-height band; `col = (lx - x0) / m.cell_w` only clamped by
  `min(.., cells)`. A click on a wrapped continuation still resolves to the wrong history index.
- `termsel.rs:294-303` `span`, `:539-550` `cols_on` — selection is `(row, col)`; `row` absolute
  scrollback line or `EDIT_ROW`, `col` a flat byte offset. No fold to/from `(visual_row, col)`.
- `termsel.rs:566-600` `selected_text` — copies via `row_text(row)` per absolute row, same flat
  assumption.
- `ui.rs:49-87` `Metrics` (`cell_w`, `text_w`) is where column width lives; there is no `cols()`
  helper anywhere — window column count is only implicit as `(pal.width() - 2*margin)/cell_w`.
- `console.rs:97-102` `place`, `:110-124` `drain_output` — push one flat `String` per line into
  `history`; the spot where wrap-on-push (rejected option, see Plan) would go.

TERMWRAP is new work, not a latent bug in existing wrap code — none exists.

## Plan

Recommend wrap **at draw/read time**, not on push: it keeps `LineSel`'s documented invariant "a
byte offset IS a cell column" (`termsel.rs:43-46`) unchanged and adds only a view-layer fold. Add
one new helper `Console::cols_for(m, w)` (mirrors the existing `top_y`/`top_y_for` split) plus pure
functions `visual_rows(len, cols)` and `wrap_rc(offset, cols)` in `termsel.rs`.

- **M1 — edit line wraps.** Files: `console.rs` `draw_prompt_line`, `draw_input_line`; new
  `visual_rows`/`wrap_rc` in `termsel.rs`. `draw_prompt_line` draws `current_input` in `cols`-wide
  chunks, one `pal.draw_text` per visual row, `y += m.line_h` each; `prompt_y` reserves
  `visual_rows(len, cols)` rows, not 1.
  Witness: `:: TERMWRAP: cols=N len=L rows=R caret=(r,c) -> PASS ::`, once per state change (edit
  or resize), never per repaint (mirrors `termsel.rs:34`'s rule for `[termsel]`).
  Go-red: line of length `cols+1` must render 2 rows; today renders 1 (tail off-surface).

- **M2 — scrollback wraps on draw.** Files: `console.rs` `draw`, `history_rows`,
  `history_rows_for`, `top_y`, `cell_at`. `draw` draws each `history[i]` as
  `visual_rows(len,cols)` rows (reuse M1's chunker); `history_rows*` count visual rows available;
  `skip`/`shown`/`vrow` walk by each entry's visual-row cost, not 1-per-entry.
  Witness: same `:: TERMWRAP: … ::` line; grammar for a caret-less (scrollback) case is an open
  question below.
  Go-red: push a history line of length `2*cols+5`; must cost 3 visual rows against
  `history_rows`; today counts 1, so a 6-row window shows overlapping/garbled lines.

- **M3 — TERMSEL2 span/click use visual rows.** Files: `termsel.rs` (`span`, `cols_on`,
  `selected_text`, `caret_col`, `set_caret`, `pointer`), `console.rs` (`cell_at`, `pointer_at`,
  `draw_row_band`). `cell_at` folds `(lx,ly)` to `(row, visual_row, col_in_row)` via
  `visual_rows`/`cols_for`, then to a flat offset (`visual_row*cols + col`) before calling
  `LineSel` — `LineSel` itself stays flat, the fold lives in `Console`. `draw_row_band`/
  `draw_prompt_line`'s `cols_on` consumer splits `(lo,hi)` across the same wrapped rows it drew.
  Witness: `:: TERMWRAP: … caret=(r,c) -> PASS ::` with `r>0` on a click past row 0.
  Go-red: click at `y = top + line_h` on an edit line of length `cols+10` (its wrapped row 2);
  expect offset `cols + click_x/cell_w`; today `cell_at` has no second-row concept and misresolves.

## Spec pins

`unaos/scripts/specs/x86-wc.spec` (numerics as `\d+`, no look-around, per file's own style):

```
REQUIRE \[TERMWRAP\] cols=\d+ len=\d+ rows=\d+ caret=\(\d+,\d+\) -> PASS
FORBID \[TERMWRAP\].* -> FAIL
FORBID \[TERMWRAP\] rows=1 len=[0-9]{3,}
```

The last FORBID is the go-red guard: a 3+-digit-length line reported `rows=1` is the bug class and
must never pass. Grammar assumes M1's edit-line shape is shared by M2/M3 (see below).

## Open questions

- Does the M2 scrollback witness reuse `caret=(r,c)` verbatim (scrollback has no caret) or drop
  the field? Pick before M2/M3 land — the spec above assumes one shared line shape.
- Wrap-at-draw (recommended) vs. wrap-on-push into `history`: push-time simplifies `draw`/
  `cell_at` but loses the original logical line (harder resize-reflow, harder `selected_text`
  copy of a logical line). Check `RULINGS.md` for prior view-vs-model precedent before deciding.
- Fixed `cols` vs. recomputed each resize from `pal.width()`? Resize-reflow is a bigger behavior
  change than this flight's repro needs — needs a product call, not just an implementation one.

## Next-session start

1. `sed -n '190,270p' unaos/crates/kernel/src/console.rs` — reread `top_y`/`history_rows`/
   `draw_prompt_line` in full before editing.
2. Add `Console::cols_for(m, w)` and `termsel::visual_rows`/`wrap_rc`, then wire M1's chunked
   `draw_prompt_line` + witness first — it is the flight's literal repro.
3. `UNAOS_WC=1 ./arroyo test 240 -> target/serial.log` then
   `LC_ALL=C grep -a -o -F '[TERMWRAP]' target/serial.log` to confirm the witness reaches the log
   before writing real `.spec` REQUIRE lines.

## Draft code (unbuilt)

```rust
// termsel.rs, after the module doc block (~line 38)
pub fn visual_rows(len: usize, cols: usize) -> usize {
    if cols == 0 { return 1; }
    core::cmp::max(1, (len + cols - 1) / cols)
}
pub fn wrap_rc(offset: usize, cols: usize) -> (usize, usize) {
    if cols == 0 { return (0, offset); }
    (offset / cols, offset % cols)
}
```

```rust
// console.rs, after draw_prompt_line (~line 264) — sketch of the chunking shape, not a drop-in
fn draw_wrapped(&self, pal: &mut TargetPal, x0: usize, mut y: usize, text: &str, cols: usize, color: u32) -> usize {
    let m = pal.metrics();
    let rows = crate::video::termsel::visual_rows(text.len(), cols);
    for r in 0..rows {
        let (lo, hi) = (r * cols, core::cmp::min(r * cols + cols, text.len()));
        pal.draw_text(x0, y, text.get(lo..hi).unwrap_or(""), color);
        y += m.line_h;
    }
    serial_println!(":: TERMWRAP: cols={} len={} rows={} caret=({},{}) -> PASS ::", cols, text.len(), rows, 0, 0);
    y
}
```
