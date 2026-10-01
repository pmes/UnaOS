# TERMCOLOR (R75) — the shell/console window prints in colour

## Finding
`console.rs::draw` painted every history row with the one constant `0xAAAAAA` and the prompt `0x00FF00`; the store (`Vec<String>`) had nowhere to keep an attribute, and escape bytes from ring-3 (`sys_write` -> `serial_line::emit_user` -> termring -> `Console::drain_output`) would have been printed as glyphs.

## Mechanism
- `termcolor.rs` (new): `Span{start,fg,bg,bold}`, `parse_line` (SGR, `K`, `2J`, `H`, `C/D/G` caret, `\r \t \b`, other CSI/OSC/charset consumed silently), `SPANS_MAX=16`, `palette256`.
- `console.rs`: `attrs` parallel to `history` (empty Vec for plain lines = no allocation); `push_parsed` is the one ingest (both `place` and `drain_output`); `ESC[2J` moves `clear_abs` (the live view clears, scrollback kept); `draw` -> `draw_span_row` paints bg rect + glyph run per span; `Console::style/println_styled`; `geometry()` atomics set by `draw`.
- `video/theme.rs`: `ANSI16`, `TERM_FG`, `TERM_ACCENT`, `TERM_RED/GREEN/BLUE/DIM`.
- M2: errors/usage red, prompt accent, `ls` dirs blue, `tests` summary green/red, `[... shown]` notices dim.
- M3: Linux `ioctl TIOCGWINSZ 0x5413` in `arch/x86_64/linuxabi/sys.rs` returns rows/cols from `console::geometry()`.

## Witness / pins
`tests termcolor` -> `:: TERMCOLOR: spans_max=16 sgr_ok= erase_ok= palette=16+256+rgb -> PASS ::`; `x86-wc.spec` REQUIRE/FORBID appended. TERMWRAP/TERMSEL/SCROLLBACK rows untouched (plain lines take the old paint path unchanged).

## Written
Boot 17 should show `:: TERMCOLOR: spans_max=16 sgr_ok=ok erase_ok=ok palette=16+256+rgb -> PASS ::` after `tests termcolor`.
