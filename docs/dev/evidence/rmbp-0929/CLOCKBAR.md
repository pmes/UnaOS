# CLOCKBAR — live clock at the menubar's right end

Finding: the bar already drew UTC `HH:MM` from `clock::try_unix_now` (menubar.rs `clock_hhmm`, `compose_row`
clock branch, repaint once a minute via the model signature) but drew NOTHING before an anchor and never a date.
Nearest missing pieces built: `--:--` placeholder, date on a wide bar, `:: CLOCKBAR:` witness.

Mechanism (menubar.rs, shared by x86 `wc` and aarch64 `desktop_firmware`): the clock branch of `compose_row`
uses `m.clock.unwrap_or(*b"--:--")` and calls `clockbar_paint` (file tail): date `Www DD Mon` (10 glyphs) when the
bar is >= 130 glyph cells wide, left of the status item/clock; theme ink `TITLE_TEXT_INACTIVE`; metrics are `CELL_W`-derived.
The existing sntp6 BARCLOCK witness keeps its meaning (`set` only when really anchored). Hover-date not built (no hover hook in the bar).
Witness: `:: CLOCKBAR: anchored=<0|1> text=HH:MM drawn=1 -> PASS ::`, once per state. Pins: x86-wc.spec tail. No knob.

## Written
M1: all of the above in one commit (menubar.rs tail + one folded painter line, spec pins).
