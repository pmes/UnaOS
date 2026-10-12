# EDITCORE (LEDGER SR73) — evidence

Worktree `exec-host-editcore`, cut at c4fa1b56, fast-forwarded to 930cdd79 per the seat's amendment. Host arc: no kernel bytes, no knob, no QEMU.
The runs below were taken BEFORE the R107 amendment (executors compile only; the seat checks at the fold) reached this seat.

## Test counts — `cargo test -p edit_core -p tabula -- --nocapture` (rc=0)

| suite | result |
| --- | --- |
| edit_core::fuzz | 2 passed (10 000-op fuzz vs `Vec<char>` model; line index vs naive split on 50 generated files) |
| edit_core::kat | 26 passed (every operation: piece table, line index, CRLF, grapheme motion over combining marks / ZWJ emoji / flags / CRLF / Hangul jamo, word motion, delete by grapheme/word/line, delete_lines, vertical motion, multi-cursor insert + merge, replace/set_text, newline auto-indent, indent/outdent tabs+spaces, undo grouping, Change replay, search, viewport wrap) |
| edit_core::uax29 | 2 passed — GraphemeBreakTest-17.0.0 766 cases 0 fail; WordBreakTest-17.0.0 1944 cases 0 fail |
| tabula::unit | 33 passed (incl. core-is-record, set_buffer minimal undoable edit, read-only refuses core edits) |
| tabula::console_view | 5 passed |

edit_core 30 tests, tabula 38 tests.

## Fuzz log (default seed)

```
EDITCORE fuzz seed=6840384095321456755
EDITCORE line index: 50 generated files match the naive split
EDITCORE fuzz: 10000 ops OK (replace 1684 type 847 grapheme-del 828 undo 845 redo 829 newline 845 indent 847 word/line-del 804 lines/insert 805 motion 863 sel/idle 803), final len=88 lines=6 pieces=17
```

Ten more seeds (`EDITCORE_SEED=1 2 3 42 77 1234 99999 31337 271828 314159`), each 10 000 ops: all OK.
After every op the fuzz asserts text == model, `Text::check_invariants()` (no empty piece, lengths sum to len,
line index == naive scan, every piece UTF-8), line count == naive LF count + 1, selections sorted,
non-overlapping and on char boundaries, and the model's undo stack depth == `Buffer::undo_depth()`; it ends by
unwinding the whole history back to the initial text.

## Builds

- `cargo build -p edit_core --target aarch64-unknown-none-softfloat` rc=0 (no_std + alloc, zero deps).
- `cargo check -p tabula --features gtk` rc=0 (GTK 4.14; no tabula warnings).

## Tables

`python3 -I tools/gen_tables.py <ucd-dir>` (UCD 17.0.0 from unicode-org/unicodetools; www.unicode.org is refused
by the egress proxy) → `src/tables.rs` (GCB 1386 ranges, WB 1100, InCB 473, ExtPict 156) and
`tests/data/{grapheme,word}_test.txt`. Input sha256 pins are in the script; the four shared with font_core match
`unaos/libs/text/font_core/oracle/vectors.txt`.
