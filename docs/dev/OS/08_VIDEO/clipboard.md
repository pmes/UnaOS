# The clipboard, and the delivery of an action — APPCLIP

A chord that has been RESOLVED is not a chord any more; it is a **meaning**, and until this arc this
tree had nowhere to put one. [KEYMAP](keymap.md) turned `⌘C`/`⌘V`/`⌘X`/`⌘A` into
`keymap::Action::{Copy,Paste,Cut,SelectAll}` at the two HID decoders and stopped there — no event
carried an action, no buffer held text, and nothing anywhere consumed one. That was written down
rather than half-built (keymap.md §6, now closed). This arc built the three parts it named.

Peter, 2026-09-22, [R61](../../RULINGS.md): *"i prefer command-c and friends (alt-c on pc) then
there's no special case for the command line to resolve that usability question."*

Source: [`video/clipboard.rs`](../../../../unaos/crates/kernel/src/video/clipboard.rs) (the buffer,
the consumer, the fixture), [`video/termsel.rs`](../../../../unaos/crates/kernel/src/video/termsel.rs)
(TERMSEL's selection model and fixture, §7), `console.rs`'s `draw_prompt_line` (the band), [`pal.rs`](../../../../unaos/crates/kernel/src/pal.rs) (the
`Event::Action` variant and its classification), the two `pack_input` functions in
[`arch/x86_64/syscall.rs`](../../../../unaos/crates/kernel/src/arch/x86_64/syscall.rs) and
[`arch/aarch64/syscall.rs`](../../../../unaos/crates/kernel/src/arch/aarch64/syscall.rs),
`una_abi::INPUT_EV_ACTION`, the non-capture arm of each HID decoder
([`drivers/xhci/mod.rs`](../../../../unaos/crates/kernel/src/drivers/xhci/mod.rs),
[`drivers/ehci/mod.rs`](../../../../unaos/crates/kernel/src/drivers/ehci/mod.rs)) and the terminal's
arm in `main.rs`'s x86 render service.

## 1. Delivery — a third kind of event

`pal::Event` gained one variant, `Action(keymap::Action)`, and it is deliberately **neither a key nor
a pointer report**. An action has already been JUDGED — `keymap::resolve` turned a modifier byte and
a usage into a meaning at the decoder — so nothing downstream re-derives it from a keystroke and
nothing treats it as motion.

| Seam | What it does |
|---|---|
| the two HID decoders | push `Event::Action(act)` on the **non-capture** arm of the chord they already resolve, with `[clip] chord=<token> action=<name> via=<xhci\|ehci> -> delivered` on the wire. The capture arm is untouched: `is_capture()` still owns `prtscr::request()`. |
| `pal::push_locked` | classifies it as a third kind, counted in `EVQ_ACT_PUSH`/`EVQ_ACT_DROP` and in NEITHER of the UVUG-10 class counters. |
| `pal::pop_event` | does not count it either — see §5. |
| both arches' `pack_input` | `(una_abi::INPUT_EV_ACTION, clipboard::action_code(act))`, so a ring-3 program receives a `⌘V` through `SYS_INPUT_POLL` exactly the way it receives a key. |

`action_code` lives in `clipboard.rs` and not in `keymap.rs` because it is an **ABI fact about the
input ring**, not a property of the binding table: KEYMAP owns what a chord means, this owns how that
meaning crosses to ring 3. One definition, read by both arches, so the two wires cannot drift.

`Ctrl-C` cannot reach any of this. No shipped table row claims it, so `resolve` returns `None`, the
decoder's chord arm is never entered, and `hid_key_ascii(0x06, HID_MOD_CTRL, false)` still returns
`0x03` for the shell. **Not one line of the ascii fold changed in this arc** — and it is measured
rather than asserted, by KEYMAP's own `ctrl_c_ascii=0x03 ctrl_c_action=none` fields, which are
unchanged on the same capture that carries the APPCLIP verdict.

## 2. The clipboard — one buffer, owned by the session

`CLIP_CAP` is 4096 bytes of static `.bss`, **text only**, and the refusals are on the wire:

```
[clip] set len=10 epoch=0
[clip] get len=10 epoch=0 stale=no
[clip] refuse reason=too-large len=8192 cap=4096
[clip] refuse reason=non-text off=3 byte=0x1b
[clip] clear len=10 reason=<why>
```

A `set` longer than the buffer is REFUSED, never truncated: a silently shortened paste is a corrupted
one and the operator has no way to see it happened. "Text" is printable ASCII plus `\n` and `\t`, and
a control byte is refused rather than filtered — a filtered paste is a paste nobody asked for, and a
stray `\r` in a clipboard is how a paste dispatches a half-typed command.

**Ownership is the session's.** Every `set` stamps the live session epoch; `fresh_len` compares it and
**destroys the buffer** when it differs, reporting `stale=yes`. A later user cannot paste the previous
user's text.

> **The clear is on the READ, not on log-out, and that is a decision with a reason.** The first choice
> was a same-line fold into `fs::users::logout`. That file belongs to SECLOGIN this round and is
> read-only to this arc, so the check lives on the other side of the same fact. The two are equivalent
> for the property that matters — no read of a closed session's text can succeed — and the read-side
> form is strictly harder to bypass, because it does not depend on the session having ended *through
> `logout`*. What it does not do is shrink the window in which the bytes still sit in kernel RAM. If a
> hook is added to `users.rs` later it calls `clipboard::clear`, and this check stays as the belt.

`session_epoch()` reads `arch::syscall::session_epoch()` where a `login` feature and an EL0 regime
exist, and `0` otherwise. That does not weaken the gate: the stamp and the comparison move together,
and the fixture proves the mechanism by AGEING a stamp rather than by opening a session, so it
measures the same code on every build — including the gate lane, which has no `login`.

## 3. The first consumer — the terminal

`clipboard::terminal_action(act, line, sel)` is what the shell's line editor does with an action.
Since TERMSEL it takes the console's line and selection by `&mut` and returns the witness field plus
whether the input line must be repainted (§7).

| Action | What the terminal does |
|---|---|
| `Paste` | pushes each clipboard byte back onto the ring as `Event::Key`. **Not a call into the editor** — the editor's existing `Event::Key` arm types and echoes them, so the line edit, the echo, the `\n` dispatch and the key census are identical to the operator having typed the text, and there is no second entry into the editor to keep in step. |
| `Copy` | copies the **selection** when one is live (`unit=selection`), and the **whole current input line** when none is (`unit=line`, APPCLIP's behaviour unchanged). |
| `Cut` | copies the selection and removes it from the editable line; with none, declined on the wire: `[clip] cut refused reason=no-selection`. |
| `SelectAll` and the five selection actions | move the selection (§7). |
| `Screenshot`, `ScreenshotRegion`, `LogOut` | `ignored` — the captures are acted on at the decoder, and `LogOut` is KEYMAP's slot, bound by nobody. |

**Until TERMSEL there was no selection model in this tree**, so `Copy` took the line and `Cut` and
`SelectAll` were witnessed `unsupported`. TERMSEL (§7) came here and changed the arms and the spec
row that pinned their values, as this paragraph said the selection arc would have to. The refusal
that remains — `⌘X` with nothing selected — is witnessed for the reason the old ones were: an
operator who presses `⌘X` and sees nothing learns that the chord does nothing and stops reporting it.

**No `Ctrl-C` special case exists anywhere on this path** — no guard, no terminal branch, no focus
test. That is R61 discharged: the table never claimed `Ctrl-C`, so nothing had to be carved out for it.

The call site is the x86 render service's event match, the same drain and the same
`wc_route_event` every key and click travel. An action reaches it only when no focused ring-3 window
took the event, which is exactly the condition under which the keyboard belongs to the shell — so no
focus test is added there either.

## 4. The fixture

`clipboard::selftest()`, chained from `drivers::ehci::parser_selftest` **beside**
`keymap::selftest()` and not inside it: KEYMAP's verdict is pinned field-by-field by
`scripts/specs/x86-wc.spec` and must read exactly what it read before this arc.

```
:: APPCLIP: delivered=4 copy=ok paste=ok len=10 line_match=true cut=empty selectall=ok epoch_clear=ok -> PASS ::
```

Every field is a round trip, not a restatement. Four `Event::Action`s go in through
`pal::push_event` and come back out of `pal::next_event`; the shipped `terminal_action` consumes
them; the paste arrives as `Event::Key`s on that same ring, which the fixture rebuilds the line from
using the line editor's own rule (`main::handle_key`'s printable-ASCII arm). So `line_match=` is
`false` if delivery never happened, if the clipboard stored nothing, or if the paste pushed nothing.
`epoch_clear=` ages the stamp by one epoch — the state a log-out leaves — and requires the next read
to destroy the buffer.

`[clip] fixture pre-drain discarded=N` is printed before the run: the fixture drains the ring to a
known state first, and a nonzero reading is the only way anyone would learn it had eaten a real event.

Gate: `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_SMC=1 UNAOS_QEMU_FULL=1 ./arroyo test 240`,
scored by [`scripts/specs/x86-wc.spec`](../../../../unaos/scripts/specs/x86-wc.spec).
Go-red: delete the `paste_into_ring()` call from `terminal_action`'s `Action::Paste` arm —
`len=0 line_match=false -> FAIL`.

## 5. Why an action is outside the UVUG-10 census, on both sides

`[uvug10] evq`'s `push - drop - pop` is read as the **live ring occupancy**, and its push counters
cover two classes: pointer and key. An event counted on exactly one side of the pipeline drifts that
reading permanently — an action counted on the pop side alone would drive it NEGATIVE, one per chord.
So `Event::Action` is excluded from `EVQ_PUSH_*` **and** from `EVQ_POP`, and the two exclusions are
one decision. It gets its own term instead, `pal::action_queue_stats()`, which no existing witness
reads.

Counting it as a key was the other candidate and is wrong for a different reason: `[uvug10] key=` is
read against `[uvug9]`'s keystroke totals to name a second consumer of the KEYBOARD, and an action is
not a keystroke — the two would stop agreeing exactly when the operator started using the clipboard.
This is the same hole `note_uncounted_discard` and `EVQ_COALESCE_PTR` were each written to keep out
of the ledger, and it is closed the same way.

A dropped action gets its own counter (`EVQ_ACT_DROP`) rather than sharing the key drop, because it
is a different event: a dropped motion is re-carried by the next report, and a lost `⌘V` is lost.

## 6. What is next

* **A selection model** — built by TERMSEL, §7 (keyboard, editable line); pointer and scrollback
  selection and a caret by TERMSEL2, §7.10–§7.15.
* **A ring-3 clipboard API** — two syscalls, `SYS_CLIP_SET(ptr, len)` and
  `SYS_CLIP_GET(ptr, cap) -> len`, each owing the same text-only and capacity refusals `set` makes,
  and each owing an ownership question this kernel has not answered: may a background program
  overwrite the clipboard of a foreground one? Ring 3 receives the `⌘C` today (§1) and can act on it
  with its own buffer; it cannot reach the kernel's.
* **Quarry's consumer.** The file manager is on the glass at boot on a `quarry` image and has its own
  keyboard door; `⌘C` on a file is a different meaning of the same action, and the seam for it is
  `terminal_action`'s shape, not a second event.

## 7. The terminal selection — TERMSEL

Until TERMSEL, `⌘C` copied the whole input line because nothing recorded which characters were
selected. This section is the model that records it. It is the smallest selection that is honest about
what the terminal holds, and it names what it leaves for later.

### 7.1 What the terminal's text model is

Measured at the branch's parent (`98fd8e66`), in `unaos/crates/kernel/src/console.rs` and
`main.rs::handle_key`:

| Part | What it is |
|---|---|
| `Console::history` | a `Vec<String>` scrollback, bounded at `HISTORY_MAX` = 256 lines, drop-oldest, painted read-only in grey. |
| `Console::current_input` | ONE editable `String`: the shell line. |
| the editor | `handle_key` is the only code that changes `current_input`, and it has exactly three edits: a printable byte (0x20..0x7E) is APPENDED, BS/DEL POPS the last byte, CR/LF dispatches the line and clears it. |
| the caret | **there is none.** The block cursor is painted one cell past the last character (`draw_prompt_line`), and no key moves it: the arrow keys reach `handle_key` as `0x1C..0x1F` and fall through every arm. |
| a cell | one byte. Every byte in `current_input` is printable ASCII (the only arm that inserts tests `32..=126`, and a paste arrives through that arm as `Event::Key`), so a byte offset and a cell column are the same number. |

### 7.2 What can be selected

**A contiguous run of cells in the editable line, and nothing else.** A selection is two offsets into
`current_input`: an ANCHOR, where it started, and a HEAD, the end that moves. Because the editor
has no caret, a selection that starts from nothing starts with both at the end of the line — the
place the block cursor is painted — so the first `Shift+←` selects the last character, exactly as
it does on a Mac text field whose caret sits at the end.

The scrollback is NOT selectable in this arc. It is already in memory and could be, but reaching a
line above the prompt from the keyboard needs a vertical motion (`Shift+↑`/`Shift+↓` walking into
`history`) and a rule for what a cut does to a read-only line; neither is asked for here, and a
selection that can reach only the editable line has no read-only case to get wrong. Scrollback
selection arrives with the pointer (§7.7).

### 7.3 How a selection is made — every chord is a row in the theme's table

No chord is tested in terminal code. Each is a `Binding` in `video/theme.rs`, resolved by
`keymap::resolve` at the HID decoder, and delivered as `pal::Event::Action` exactly as `⌘C` is
(§1). Five new `keymap::Action` variants carry them:

| Action | Token | What it does to the selection | CRISPY (Mac) | PC |
|---|---|---|---|---|
| `SelectLeft` | `select-left` | HEAD one cell left (stops at 0) | `Shift+←` | `Shift+←` |
| `SelectRight` | `select-right` | HEAD one cell right (stops at the line end) | `Shift+→` | `Shift+→` |
| `SelectLineStart` | `select-line-start` | HEAD to cell 0 | `Cmd+Shift+←`, `Shift+Home` | `Shift+Home` |
| `SelectLineEnd` | `select-line-end` | HEAD to the line end | `Cmd+Shift+→`, `Shift+End` | `Shift+End` |
| `SelectAll` (exists) | `select-all` | ANCHOR 0, HEAD the line end | `Cmd+A` | `Alt+A` |
| `Deselect` | `deselect` | clears it | `Esc` | `Esc` |

`Cmd+Shift+←/→` is on the Mac table because it is the Mac's own chord for "to the start/end of the
line", and because **the rMBP's internal keyboard has no Home or End key**. `Shift+Home/End` is on
both tables so an external PC keyboard works on either. A selection whose ANCHOR and HEAD meet is
no selection (it collapses, and the witness says so).

`Esc` is a row with no roles, like Print Screen. It does not stop being a key: the decoders push the
`0x1B` `Event::Key` exactly as before and push the `Deselect` action beside it, so a menu that
dismisses on `0x1B` still sees it. R24 (as heard by orin 17) is about Esc and app windows — *"esc
should not close any app windows"* — and clearing a selection closes nothing.

### 7.4 What an edit does to a selection

**Any edit drops it.** A typed character, a Backspace, a Return, and every byte of a paste (which
is typed through the same arm) clear the selection before they change the line. A Mac text field
REPLACES a selection with what is typed; that needs a caret to put the replacement where the
selection was, and this editor appends at the end and nowhere else. Replace-on-type is owed with a
caret, and until then an edit that silently kept a selection over text that has moved would paint a
band over the wrong cells.

### 7.5 What the clipboard chords do

| Chord | With a selection | With none |
|---|---|---|
| `Copy` | copies the SELECTED cells (`[clip] copy unit=selection len=N`); the selection stays | copies the whole line (`[clip] copy unit=line len=N`) — **APPCLIP's behaviour, unchanged** |
| `Cut` | copies the selected cells and REMOVES them from the editable line; the selection clears | declined and witnessed (`[clip] cut refused reason=no-selection`) |
| `Paste` | types the clipboard (§3); the first typed byte drops the selection (§7.4) | types the clipboard (§3) |

Cut can only ever remove cells from the editable line, because that is the only line a selection
can reach (§7.2). If scrollback selection lands, cut there must refuse — the scrollback is output,
not input.

### 7.6 How it is shown, and the wire

**Shown:** an inverse-video band painted by the terminal's own painter, `Console::draw_prompt_line`
— the shared routine the full repaint and the per-keystroke fast path both call — so the band can
never disagree between the two. The band is the selected cells filled with the text colour and the
selected characters redrawn in the console background colour; the block cursor stays where it is.

**Wire:** ONE line per STATE CHANGE, from the model and never from the painter (a repaint is not a
change):

```
[termsel] sel=<lo>..<hi> cells=<n> line=<len> by=<action-token|edit|cut>
[termsel] none line=<len> by=<action-token|edit|cut>
```

No selected TEXT is printed — the `[clip]` lines print lengths only, for the same reason.

### 7.7 What this arc does NOT do

* **Pointer selection** — drag to select, double-click a word, and with it scrollback selection. The
  x86 shell has no click model (`main.rs`'s `Event::Button` arm is empty by design); that is the
  next arc, and it writes into the same ANCHOR/HEAD pair. *(Built by TERMSEL2, §7.10–§7.12: the
  arm is still empty — the router notes the press instead — and it did write into the same pair.)*
* **A caret.** Without one, a selection always starts at the line end and an edit cannot replace
  it (§7.4). *(Built by TERMSEL2, §7.13.)*
* **The fixture cannot press a key.** QEMU has no operator's hands (§4); the chords are resolved
  through the table from synthetic report pairs and pushed through the REAL ring, which is the
  proof available off metal. The decoder half and the painted band are flight 12's to see.

### 7.8 The fixture, the gate, and the wire it printed

`video::termsel::selftest()`, chained from `drivers::ehci::parser_selftest` after APPCLIP's fixture,
on its own line. It drives thirteen CHORDS, not actions: each is a synthetic HID report pair resolved
through the live table (`keymap::resolve_edge(keymap::active(), …)`), pushed through `pal::push_event`,
taken back out of `pal::next_event` and handed to the shipped `terminal_action`, against a line
(`unaos select`) and a `LineSel` standing where the console's stand. Measured on the wc lane at
TERMSEL M1 (`UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_SMC=1 UNAOS_QEMU_FULL=1 ./arroyo test 240`,
rc=0 COMPLETE, x86-wc.spec 27/27):

```
[termsel] sel=11..12 cells=1 line=12 by=select-left
[termsel] sel=10..12 cells=2 line=12 by=select-left
[termsel] sel=9..12 cells=3 line=12 by=select-left
[termsel] sel=10..12 cells=2 line=12 by=select-right
[clip] copy unit=selection len=2
[termsel] sel=0..12 cells=12 line=12 by=select-line-start
[termsel] none line=12 by=deselect
[clip] copy unit=line len=12
[termsel] sel=0..12 cells=12 line=12 by=select-all
[termsel] sel=0..11 cells=11 line=12 by=select-left
[clip] cut unit=selection len=11
[termsel] none line=1 by=cut
[clip] cut refused reason=no-selection
[termsel] sel=0..1 cells=1 line=1 by=select-all
[termsel] none line=1 by=edit
:: TERMSEL: resolved=13/13 delivered=13 left=ok copy_sel=ok home=ok esc=ok copy_line=ok cut=ok cut_empty=ok edit=ok pc=ok -> PASS ::
```

(`[clip] set`/`get` lines between them omitted here.) `copy_sel=` reads the clipboard back through
the epoch gate and requires exactly the two selected cells; `copy_line=` requires the whole line
with nothing selected (§3's behaviour); `cut=` checks the clipboard, the line and the selection
after `⌘X`; `pc=` resolves the PC column's five chords through `PC_BINDINGS`. Pinned in
[`scripts/specs/x86-wc.spec`](../../../../unaos/scripts/specs/x86-wc.spec) (REQUIRE + FORBID,
tail-appended). **Go-red, measured:** make `terminal_action`'s `Copy` arm ignore the selection
(`match None::<(usize, usize)>`) — every copy reads `unit=line`, the verdict is
`copy_sel=no … -> FAIL ::`, `./arroyo test` rc=1 and the x86-wc.spec replay 25/27 with the TERMSEL
FORBID hit.

The painted band is NOT proved by the fixture — it runs at `ehci::init`, before any console is
drawn. It is proved by construction (the painter reads `LineSel::range`, the same reading the
fixture asserts) and by flight 12.

### 7.9 What flight 12 can show, stated so it can be wrong

On the shell window, after typing `hello world` at the prompt, **on the internal keyboard (EHCI) or
an external one (xHCI)**, in this order:

* `Shift+←` three times paints an inverse band over `rld`, and the wire carries three
  `[clip] chord=shift-left action=select-left via=<ehci|xhci> -> delivered` lines each followed by
  a `[termsel] sel=… by=select-left` line ending at `sel=8..11 cells=3 line=11`.
* `⌘C` copies `rld` (`[clip] copy unit=selection len=3`) and the band stays.
* `⌘⇧←` then extends the band to the line start (`sel=0..11`); `Esc` removes it
  (`none line=11 by=deselect`); `⌘C` with nothing selected copies the whole line
  (`[clip] copy unit=line len=11`), as before this arc; `⌘V` types `hello world` onto the end.
* `⌘A` then `⌘X` empties the line and the clipboard holds it; `⌘X` again prints
  `[clip] cut refused reason=no-selection` and changes nothing.
* Typing any character while a band is shown removes the band (`by=edit`) and appends the character.
* On an external PC keyboard, `Shift+Home`/`Shift+End` behave as `⌘⇧←`/`⌘⇧→`
  (`chord=shift-home`/`shift-end`). **Unverified:** whether the internal keyboard's `Fn+←` reaches
  the decoder as Home (0x4A); in HID boot protocol the Apple Fn key is not reported, so the
  expectation is that it arrives as a plain `←` and `Fn+Shift+←` selects one cell.

A reading that contradicts any bullet is a TERMSEL defect, except the last clause, which is a
question about the keyboard.

### 7.10 TERMSEL2 — the pointer, the scrollback and the caret

TERMSEL's §7.7 named two things it left undone by design: pointer selection (and with it the
scrollback, which only a pointer can reach) and a caret (without which typing cannot replace a
selection). TERMSEL2 (rmbp-ledger B196) builds both into the SAME model — `LineSel` grew, nothing
was replaced, and a selection that lies wholly on the editable line is still TERMSEL's, with
TERMSEL's `sel=` witness byte for byte.

| Part | What it is now |
|---|---|
| a cell | `(col, row)`. `row` is an ABSOLUTE scrollback line number — `Console::hist_base` (lines dropped off the front of the 256-line scrollback) plus the index into `history` — or `EDIT_ROW` for the editable line, which sorts after every scrollback row. A line keeps its number while newer output pushes it up the screen. A scrollback cell is one `char` (the painter advances one cell per `char` and draws no glyph for a non-ASCII one); an editable-line cell is still one byte. |
| the selection | TERMSEL's ANCHOR/HEAD pair, each offset now with a row: a start CELL and an end CELL. `span()` reads it in reading order; `range()` still answers only for a selection wholly on the editable line, so every TERMSEL path (the chords, `Cut`, the edit rule) reads exactly what it read before. `cols_on(row)` is the one per-row reading — both painters and the copy use it. |
| the caret | a cell BOUNDARY on the editable line, resting at the line end (`CARET_END`, which follows the end as the line grows — TERMSEL's only insertion point). A keyboard selection that starts from nothing starts at the caret, and the caret rides the head of an editable-line selection. |

### 7.11 How a press reaches the shell

The shell window is `KERNEL_OWNER_DESKTOP` furniture, so `wc_click_route_at` CONSUMES a press on its
content in the kernel-owner arm — after the menu bar, the crystal, the dock, Quarry, the controls and
the chrome have all declined it — and the render service's `Event::Button` arm never sees it. That
arm stays empty. Instead:

| Seam | What it does |
|---|---|
| the router's kernel-owner arm | a press whose owner is `KERNEL_OWNER_DESKTOP` is NOTED for the window it hit: `termsel::pointer_press(win, x, y)`, which records the window as HELD. Routing is unchanged — still consumed, still raised, keyboard still to the shell. |
| `wc_route_tail` | while a press is held, each pointer report is a `drag` note at the live cursor (`pointer_held()` + `pointer_motion`); one atomic load otherwise. |
| the router's release arm | the release that ends a held press is its `up` note (`pointer_release`), for the window the press went to — never whatever the pointer has since crossed (the router's own release rule). |
| `termsel`'s press queue | 16 notes in SURFACE pixels (panel pixels less the window's origin, divided by its upscale); a drag directly behind a drag of the same window replaces it; a full queue drops its OLDEST note and says so (`[termsel] press queue full dropped=<n> (oldest)`). `take_press(win)` takes only that window's notes. |
| the render service (`main.rs`, folded) | after each routed event and its drag tail, `take_press(shell_id)` → `Console::pointer` → `Console::repaint`, marking the shell window dirty as a keystroke does. |
| `Console::cell_at` | turns surface pixels into a cell with the painter's own derivation (`top_y`/`history_rows` delegate to `top_y_for`/`history_rows_for`): a point above the first shown row reads as that row, below the prompt as the prompt, left of a row's text as its first cell, past its end as one past its last character. |

The router knows windows and not text; the console knows text and not windows; the queue is the
whole seam between them. Wire, ONE line per press, from the model:

```
[termsel] press cell=(<col>,<row>) kind=down|drag|up|dbl
```

`<row>` is the absolute scrollback line, or `e` for the editable line. A drag prints only when it
enters a NEW cell. `dbl` is a second DOWN on the SAME cell within `DBL_MS` = 500 ms (the macOS
default); a double-click consumes the pair, so a third press is a fresh single click.

### 7.12 What the pointer does to the selection

| Press | Effect |
|---|---|
| DOWN | drops any selection (`[termsel] none … by=click`) and anchors a new one at its cell. Nothing is selected until the pointer leaves the cell. On the editable line it also puts the CARET there (`[termsel] cursor col=<n> by=click`). |
| DRAG | selects every cell from the anchor cell to the drag cell, BOTH INCLUDED, across rows — scrollback rows, the editable line, or from one into the other. |
| DBL | selects the WORD: the run of printable non-space ASCII containing the cell (`by=word`); nothing when the cell is a space or past the end. Drags are ignored until the release. |
| UP | ends the press; a drag that ended on the editable line leaves the caret at its head. |

A selection that reaches the scrollback is witnessed by its cells:
`[termsel] span=(<c>,<r>)..(<c>,<r>) line=<len> by=<drag|word|click|deselect|edit|…>`. It is
painted by `Console::draw_row_band` on each scrollback row it covers (the inverse video of §7.6),
and on the editable line by `draw_prompt_line` through the same `cols_on`.

| Chord / edit | With a selection that reaches the scrollback |
|---|---|
| `⌘C` | copies every selected cell, rows joined by `\n`: `[clip] copy unit=selection len=<n> rows=<n>`. A cell holding anything but printable ASCII is copied as a SPACE — what the glass shows there, and the clipboard's text-only rule (§2) would otherwise refuse the whole copy over one character. Rows that have left the 256-line scrollback are gone from the copy as they are from the glass. (The editable-line copy now carries `rows=1`.) |
| `⌘X` | REFUSED: `[clip] cut refused reason=read-only`; the selection and the line are kept. The scrollback is output, not input — §7.5 said so before there was a way to reach it. |
| `Esc` | clears it (`Deselect`), and the whole terminal is repainted: the band was in the scrollback. |
| any edit | clears it (TERMSEL's rule, §7.4), repaints the whole terminal, then the edit happens at the caret. |

### 7.13 The caret — typing can replace a selection now

Four `keymap::Action`s, ring-3 codes 13..16, and their rows (keymap.md §2/§3):

| Action | Token | CRISPY (Mac) | PC | What it does |
|---|---|---|---|---|
| `CursorLeft` | `cursor-left` | `←` | `←` | caret one cell left; with a selection on the editable line, to its START |
| `CursorRight` | `cursor-right` | `→` | `→` | caret one cell right; with a selection on the editable line, to its END |
| `CursorLineStart` | `cursor-line-start` | `⌘←`, `Home` | `Home` | caret to cell 0 |
| `CursorLineEnd` | `cursor-line-end` | `⌘→`, `End` | `End` | caret to the line end |

Every caret action DROPS any live selection (`[termsel] none … by=<action>`) and, when the caret
moved, prints `[termsel] cursor col=<n> by=<action>`. The caret is not witnessed per keystroke:
the edit is.

**Where the caret actions reuse TERMSEL's and where they diverge.** `SelectLineStart` and
`CursorLineStart` move to the same cell, and `SelectLeft`/`CursorLeft` make the same one-cell
motion; the row tables pair them on the same keys with and without Shift, and `⌘⇧←/→` above `⌘←/→`
is the precedence `no_shadow` enforces. They diverge in what moves: a selection action moves the
HEAD and keeps the anchor; a caret action collapses the selection first — and `←` with a selection
is not "one cell left of the head" but the selection's start, the Mac's rule, which is not a motion
of the head at all.

**The bytes are still typed.** The bare-arrow rows name no roles, so — like `Esc` — the decoder
still pushes the arrow byte (`0x1D` ←, `0x1C` →) ahead of the action; Quarry and `user-vug`, which
read those bytes, see exactly what they saw before. `Home`/`End` type nothing (their ascii is 0) and
`⌘←/→` are suppressed by the `CMD` rule.

**The edits, at the caret** (`LineSel::type_byte`, called by `main::handle_key`; CR/LF still
dispatch through `on_edit` and park the caret at the end of the empty line they leave):

| Byte | With a selection on the editable line | Otherwise |
|---|---|---|
| printable | REPLACES it: `[termsel] none … by=replace`, caret after the typed byte | inserted AT the caret |
| BS / DEL | deletes it: `by=delete`, caret at its start | deletes the byte BEFORE the caret |

A paste types through the same arm (§3), so `⌘V` over a selection replaces it with the clipboard.
`⌘X` leaves the caret where the cut cells were.

**Shown:** a Mac-style insertion BAR at the caret's cell boundary, one font stroke wide (`m.scale`
px) and one cell tall, in the theme's selection/focus accent (`theme::ACCENT`, 0x4A73AA). It
replaces TERMSEL's block, which stood one cell PAST the text — a cell of the row model that held no
character; the bar sits on a boundary and occupies none, so the console's rows are exactly their
characters. It is hidden while the editable line shows a band, as a Mac text field hides its
insertion point over a selection.

**An action pressed while Quarry holds the keyboard is not the shell's.** Quarry takes its keys at
`quarry::key_route` and has no action consumer, so until now the `Event::Action` beside a key fell
through to the shell. With the bare arrows bound that would move the shell's caret out of sight on
every arrow that moves Quarry's selection, so `wc_route_event` now drops an action while
`wm::focus_asid() == quarry::OWNER` with the window open (`wc_action_quarry_held`, tail of
`arch/x86_64/syscall.rs`): `[termsel] action=<name> -> dropped (Quarry holds the keyboard)`. This
also stops `⌘C`/`⌘A` in Quarry from acting on the shell — the pre-existing half of the same leak
(§3's "no focused ring-3 window took it" was true and incomplete: Quarry is kernel furniture).

### 7.14 The fixture, the gate, and the wire it printed

`video::termsel::pointer_selftest()`, chained beside `clickroute_selftest` in
`arch/x86_64/syscall.rs` (both `witness`; `./arroyo test` runs it every boot). It mints a
`KERNEL_OWNER_DESKTOP` probe row (288x72, scale 1: 8-px cells, 12-px lines) and closes it before
`dock::selftest`, which must find no such row; presses and releases go through the LIVE
`wc_click_route_at`, motion through `pointer_motion`, the notes are taken with `take_press` and fed
to a real `Console` holding two scrollback rows `alpha beta`, `gamma delta` and the line
`unaos select`. Press points are cell centres computed from the metrics, not from `cell_at`, so the
two derivations meet. The caret chords are resolved through the live table (`keymap::resolve_edge`)
and handed to the shipped consumer; the ring hop is not repeated there, because this fixture runs
after the input service is up and an action pushed onto the ring then could reach the live render
service — APPCLIP and TERMSEL prove that hop.

Measured on the wc lane at M3 (`UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_SMC=1
UNAOS_QEMU_FULL=1 ./arroyo test 240`, rc=0 COMPLETE, full wall 240.8 s; x86-wc.spec 30/30):

```
[termsel] press cell=(2,0) kind=down
[termsel] press cell=(3,1) kind=drag
[termsel] span=(2,0)..(4,1) line=12 by=drag
[termsel] press cell=(3,1) kind=up
[clip] copy unit=selection len=13 rows=2
[clip] cut refused reason=read-only
[termsel] none line=12 by=deselect
[termsel] press cell=(8,e) kind=down
[termsel] cursor col=8 by=click
[termsel] press cell=(8,e) kind=up
[termsel] press cell=(8,e) kind=dbl
[termsel] sel=6..12 cells=6 line=12 by=word
[termsel] press cell=(8,e) kind=up
[clip] copy unit=selection len=6 rows=1
[termsel] press cell=(6,1) kind=down
[termsel] none line=12 by=click
[termsel] press cell=(4,e) kind=drag
[termsel] span=(6,1)..(5,e) line=12 by=drag
[termsel] press cell=(4,e) kind=up
[clip] copy unit=selection len=11 rows=2
[termsel] none line=12 by=edit
[termsel] press cell=(3,e) kind=down
[termsel] cursor col=3 by=click
[termsel] press cell=(3,e) kind=up
[termsel] cursor col=2 by=cursor-left
[termsel] cursor col=3 by=cursor-right
[termsel] cursor col=0 by=cursor-line-start
[termsel] cursor col=12 by=cursor-line-end
[termsel] cursor col=0 by=cursor-line-start
[termsel] cursor col=12 by=cursor-line-end
[termsel] cursor col=0 by=cursor-line-start
[termsel] cursor col=1 by=cursor-right
[termsel] cursor col=2 by=cursor-right
[termsel] cursor col=3 by=cursor-right
[termsel] cursor col=4 by=cursor-right
[termsel] cursor col=5 by=cursor-right
[termsel] sel=5..12 cells=7 line=12 by=select-line-end
[termsel] none line=6 by=replace
[termsel] sel=0..12 cells=12 line=12 by=select-all
[termsel] none line=12 by=cursor-left
[termsel] cursor col=0 by=cursor-left
[termsel] sel=0..12 cells=12 line=12 by=select-all
[termsel] none line=12 by=cursor-right
:: TERMSEL2: legs=0x7ffff/0x7ffff hit=ok route=ok drag=ok up=ok dbl=ok sel=ok copy=ok cut_ro=ok esc=ok word=ok into_edit=ok edit=ok click_caret=ok arrows=ok insert=ok bs=ok replace=ok collapse=ok pc=ok -> PASS ::
```

(`[clip] set`/`get` and the `[clickroute] press … -> consume` lines between them omitted.) Each
`legs=` bit is one leg, documented at `pointer_selftest`. Pinned in
[`scripts/specs/x86-wc.spec`](../../../../unaos/scripts/specs/x86-wc.spec) (REQUIRE + FORBID,
tail-appended past TERMSEL's). TERMSEL's own verdict is unchanged on the same capture.

**Go-red, measured, two:** (M2) a drag that ignores the row (`self.head_row = a.0`) —
`legs=0xb1f/0xfff … sel=no copy=no cut_ro=no … into_edit=no … -> FAIL ::`, test rc=1, x86-wc.spec
29/30 with the FORBID hit; (M3) an insert that ignores the caret (`let k = len;` in `type_byte`) —
`legs=0x63fff/0x7ffff … insert=no bs=no replace=no … -> FAIL ::` (`replace` fails by cascade: its
start state is the one `insert`/`bs` leave), test rc=1, 29/30. Both reverted to byte-identical files.

**What the fixture proves and what only the glass can.** Proved: the router's disposition of a
press on a `KERNEL_OWNER_DESKTOP` row, the queue, `cell_at` over the shipped layout, every selection
and caret rule, the copy text, the read-only refusal, the live table's resolution of every caret
chord on both tables. NOT proved, because no chord can be pressed and no pointer moved under QEMU:
the HID decoders' pushing of the new actions beside their bytes (the same arm TERMSEL's chords
take), the real trackpad's press/drag/release edges arriving at the router, the render service's
`take_press(shell_id)` drain on a live shell window (the fixture takes its own probe row's notes),
the painted bands and the bar. Those are flight 13's (§7.15).

### 7.15 What flight 13 can show, stated so it can be wrong

On the shell window, after running `help` so the scrollback has lines and typing `hello world` at
the prompt, on the internal trackpad (EHCI) and keyboard:

* **Drag-select in the shell.** Press on a scrollback line, drag down across two lines and onto the
  prompt line, release: an inverse band covers every cell from the press cell to the release cell,
  both included; the wire carries `[termsel] press cell=(c,r) kind=down`, `kind=drag` lines (one per
  new cell), one `[termsel] span=(…)..(…,e) … by=drag` per cell change and `kind=up`.
* **⌘C** then prints `[clip] copy unit=selection len=<n> rows=<3 or more>`; **⌘X** prints
  `[clip] cut refused reason=read-only` and the band and the line stay; **Esc** removes the band
  (`[termsel] none … by=deselect`).
* **Paste.** With the band gone, **⌘V** types the copied text at the caret; its `\n`s DISPATCH lines
  (the paste is typed, §3) — select within one line if that is not wanted.
* **Word double-click.** Double-click `world` on the prompt line: the band covers exactly `world`
  (`kind=dbl`, `sel=6..11 … by=word`); typing `X` replaces it: `hello X`, `none … by=replace`.
  Double-clicking a word in the scrollback bands it; typing then drops the band (`by=edit`) and
  appends.
* **Cursor movement.** A bar (accent blue, one stroke wide) sits after the last character; `←` moves
  it one cell left per press (`[termsel] cursor col=<n> by=cursor-left`, beside
  `[clip] chord=left action=cursor-left via=ehci -> delivered`); typing inserts at the bar and
  Backspace deletes the character before it; `⌘←` and `⌘→` jump to the line ends; a click on the
  prompt line puts the bar under the pointer (`cursor col=<n> by=click`).
* **Quarry keeps its arrows.** With Quarry focused, arrows move Quarry's selection and the wire
  prints `[termsel] action=cursor-left -> dropped (Quarry holds the keyboard)`; the shell's bar does
  not move.

A reading that contradicts any bullet is a TERMSEL2 defect, except two questions about the machine:
whether 500 ms is the right double-click window on this trackpad, and whether the EHCI trackpad's
press edge arrives at the router within one cell of where the arrow shows (`[clickroute] press at
(x,y)` against the band).
