# The clipboard, and the delivery of an action — APPCLIP

A chord that has been RESOLVED is not a chord any more; it is a **meaning**, and until this arc this
tree had nowhere to put one. [KEYMAP](keymap.md) turned `⌘C`/`⌘V`/`⌘X`/`⌘A` into
`keymap::Action::{Copy,Paste,Cut,SelectAll}` at the two HID decoders and stopped there — no event
carried an action, no buffer held text, and nothing anywhere consumed one. That was written down
rather than half-built (keymap.md §6, now closed). This arc built the three parts it named.

Peter, 2026-09-22, [R61](../../RULINGS.md): *"i prefer command-c and friends (alt-c on pc) then
there's no special case for the command line to resolve that usability question."*

Source: [`video/clipboard.rs`](../../../../unaos/crates/kernel/src/video/clipboard.rs) (the buffer,
the consumer, the fixture), [`pal.rs`](../../../../unaos/crates/kernel/src/pal.rs) (the
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

`clipboard::terminal_action(act, line)` is what the shell's line editor does with an action.

| Action | What the terminal does |
|---|---|
| `Paste` | pushes each clipboard byte back onto the ring as `Event::Key`. **Not a call into the editor** — the editor's existing `Event::Key` arm types and echoes them, so the line edit, the echo, the `\n` dispatch and the key census are identical to the operator having typed the text, and there is no second entry into the editor to keep in step. |
| `Copy` | copies the **whole current input line**. |
| `Cut`, `SelectAll` | accepted and witnessed `[clip] unsupported action=<name> reason=no-selection-model`. |
| `Screenshot`, `ScreenshotRegion`, `LogOut` | `ignored` — the captures are acted on at the decoder, and `LogOut` is KEYMAP's slot, bound by nobody. |

**There is no selection model in this tree.** Nothing on any surface records "these characters are
selected", so a line is the largest honest unit a copy can take and a cut has nothing to remove. The
two unsupported actions are witnessed rather than dropped for one reason: an operator who presses
`⌘X` and sees nothing learns that the chord does nothing and stops reporting it. A selection arc is
the next rung, and it has to come here and change both the arm and the spec row that pins its value.

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
:: APPCLIP: delivered=4 copy=ok paste=ok len=10 line_match=true cut=unsupported selectall=unsupported epoch_clear=ok -> PASS ::
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

* **A selection model.** Without one, `Copy` is line-granular and `Cut`/`SelectAll` are witnessed
  refusals. It is the largest thing this arc left, and it is what makes the clipboard feel like a
  clipboard.
* **A ring-3 clipboard API** — two syscalls, `SYS_CLIP_SET(ptr, len)` and
  `SYS_CLIP_GET(ptr, cap) -> len`, each owing the same text-only and capacity refusals `set` makes,
  and each owing an ownership question this kernel has not answered: may a background program
  overwrite the clipboard of a foreground one? Ring 3 receives the `⌘C` today (§1) and can act on it
  with its own buffer; it cannot reach the kernel's.
* **Quarry's consumer.** The file manager is on the glass at boot on a `quarry` image and has its own
  keyboard door; `⌘C` on a file is a different meaning of the same action, and the seam for it is
  `terminal_action`'s shape, not a second event.

## 7. The terminal selection — TERMSEL (design, M0)

APPCLIP's `⌘C` copies the whole input line because nothing recorded which characters were selected
(§3). This section is the model that records it. It is the smallest selection that is honest about
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
  next arc, and it writes into the same ANCHOR/HEAD pair.
* **A caret.** Without one, a selection always starts at the line end and an edit cannot replace
  it (§7.4).
* **The fixture cannot press a key.** QEMU has no operator's hands (§4); the chords are resolved
  through the table from synthetic report pairs and pushed through the REAL ring, which is the
  proof available off metal. The decoder half and the painted band are flight 12's to see.
