# Key bindings — KEYMAP

Every chord the desktop understands is a **row in a table that belongs to the theme**, and one
resolver reads it. Before this arc every chord was a literal in a USB driver: `drivers/xhci/mod.rs`
tested `HID_MOD_GUI` and `HID_MOD_SHIFT` and usages `0x20`/`0x21` inline, the EHCI decoder called
that same function, and Print Screen was a second literal (`0x46`) beside it. A Windows-shaped theme
could not have been added without editing the USB drivers.

Peter, 2026-09-22, [R60](../../RULINGS.md): *"we will be implementing a windows-esque them at some
point so key-bindings shouldn't be hard coded."* And [R61](../../RULINGS.md): *"i prefer command-c
and friends (alt-c on pc) then there's no special case for the command line to resolve that
usability question."*

Source: [`video/keymap.rs`](../../../../unaos/crates/kernel/src/video/keymap.rs) (the mechanism —
`Action`, `Binding`, `Table`, the resolver and the fixture),
[`video/theme.rs`](../../../../unaos/crates/kernel/src/video/theme.rs) tail (the rows — `CRISPY_ROWS`
/ `CRISPY_BINDINGS` and `PC_ROWS` / `PC_BINDINGS`), and the two HID decoders that ask it:
[`drivers/xhci/mod.rs`](../../../../unaos/crates/kernel/src/drivers/xhci/mod.rs) (the hand-off
`hid_screenshot_chord_edge` / `hid_print_screen_action_edge`, and the call site in the keyboard
branch of the event dispatch) and
[`drivers/ehci/mod.rs`](../../../../unaos/crates/kernel/src/drivers/ehci/mod.rs) (the twin call site
in `service_ehci_hid`, and the fixture's chain point in `parser_selftest`). The capture half is
[screenshot.md](screenshot.md).

## 1. The shape

| Type | What it is |
|---|---|
| `Action` | what a chord MEANS — `Screenshot`, `ScreenshotRegion`, `Copy`, `Cut`, `Paste`, `SelectAll`, `LogOut`, and TERMSEL's `SelectLeft`, `SelectRight`, `SelectLineStart`, `SelectLineEnd`, `Deselect` ([clipboard.md](clipboard.md) §7), and TERMSEL2's caret actions `CursorLeft`, `CursorRight`, `CursorLineStart`, `CursorLineEnd` (§7.13). Named for the desktop's intent, never for a key. |
| `Binding` | a ROLE mask + a HID usage + the `Action` + a witness `token`. |
| `Table` | a name, a row list in precedence order, and **`cmd_role`** — the physical HID modifier bits that play the abstract *Command* role on that table. |
| `resolve(table, modifiers, usage_edge)` | **the only place a chord is judged.** |

Role bits (`CMD`, `SHIFT`, `CTRL`, `ALT`) are *not* HID bits. `⌘C` is written once, as
`CMD | usage 0x06`, and the table decides whether the operator's hand is on the GUI key or the Alt
key. No row anywhere names a physical modifier — that is the whole seam, and it is one field wide.

## 2. What is in the CRISPY table

`CRISPY_ROWS`, in precedence order:

| Chord | Usage | Action | Token |
|---|---|---|---|
| `Cmd+Shift+3` | 0x20 | `Screenshot` | `cmd-shift-3` |
| `Cmd+Shift+4` | 0x21 | `ScreenshotRegion` | `cmd-shift-4` |
| `Cmd+Shift+Q` | 0x14 | `LogOut` — **a SLOT** | `cmd-shift-q` |
| `Cmd+Shift+←` | 0x50 | `SelectLineStart` (TERMSEL) | `cmd-shift-left` |
| `Cmd+Shift+→` | 0x4F | `SelectLineEnd` (TERMSEL) | `cmd-shift-right` |
| `Cmd+←` | 0x50 | `CursorLineStart` (TERMSEL2; below `Cmd+Shift+←`, which it would shadow) | `cmd-left` |
| `Cmd+→` | 0x4F | `CursorLineEnd` (TERMSEL2) | `cmd-right` |
| `Cmd+C` | 0x06 | `Copy` | `cmd-c` |
| `Cmd+V` | 0x19 | `Paste` | `cmd-v` |
| `Cmd+X` | 0x1B | `Cut` | `cmd-x` |
| `Cmd+A` | 0x04 | `SelectAll` | `cmd-a` |
| `Shift+←` | 0x50 | `SelectLeft` (TERMSEL) | `shift-left` |
| `Shift+→` | 0x4F | `SelectRight` (TERMSEL) | `shift-right` |
| `Shift+Home` | 0x4A | `SelectLineStart` (TERMSEL) | `shift-home` |
| `Shift+End` | 0x4D | `SelectLineEnd` (TERMSEL) | `shift-end` |
| `←` | 0x50 | `CursorLeft` (TERMSEL2; no roles named — below every 0x50 row; the `0x1D` byte is still typed) | `left` |
| `→` | 0x4F | `CursorRight` (TERMSEL2; no roles named; the `0x1C` byte is still typed) | `right` |
| `Home` | 0x4A | `CursorLineStart` (TERMSEL2; no roles named; types nothing) | `home` |
| `End` | 0x4D | `CursorLineEnd` (TERMSEL2; no roles named; types nothing) | `end` |
| `Esc` | 0x29 | `Deselect` (TERMSEL; no roles named — the `0x1B` key is still typed) | `esc` |
| Print Screen | 0x46 | `Screenshot` (no roles named) | `print-screen` |

`cmd_role` is `HID_MOD_GUI` (0x88 — left **and** right, as every `HID_MOD_*` mask is).

**`LogOut` is a slot.** `⌘⇧Q` resolves and nothing in this tree acts on it. LOGINFLOW may bind it;
nothing else may.

## 3. The second table, and why it is here

`PC_BINDINGS` is a PC-shaped table: `cmd_role` is `HID_MOD_ALT`, the edit rows are the same rows in
role space (so `Alt+C` is copy with no second `Copy` row written anywhere), and the capture chords
are `PrtSc` / `Shift+PrtSc` instead of the Apple digits. TERMSEL added its selection rows here too
(`Shift+←/→`, `Shift+Home/End`, `Esc`) and no `Alt+Shift+←/→`: on a PC the line ends are Home and
End, which every PC keyboard has. TERMSEL2 added the caret rows `←`, `→`, `Home`, `End` (no roles,
below the Shift rows on the same usages) and no `Alt+←/→`, for the same reason (15 rows; CRISPY
has 21 — its `⌘←/→` are the Mac's line-end chords, the rMBP having no Home/End key).

**It is selected by nothing.** It is compiled, resolvable, and reached today only by the fixture's
`pc_table_alt_c=` leg. A knob that selects it is NOT this arc; when one is written it changes
`keymap::active()` and nothing else. It exists so that the seam is *proved* rather than asserted: if
the role indirection were cosmetic, `pc_table_alt_c=` would read `none` while every other field
stayed `ok`.

## 4. R61 — why the command line needs no special case

`Ctrl-C` keeps its interrupt meaning in the shell, on every window, because **the table never claims
it**. There is no guard, no terminal branch, and no focus test anywhere: `CTRL` is a role the
resolver understands and no shipped row uses it, so `resolve(CRISPY, HID_MOD_CTRL, 0x06)` is `None`
and `hid_key_ascii(0x06, HID_MOD_CTRL, false)` still returns `0x03` exactly as it did before KEYMAP.
The fixture measures both halves — `ctrl_c_action=none` and `ctrl_c_ascii=0x03` — rather than
asserting them, and not one line of the ascii fold changed in this arc.

**A chord types nothing, and that needed no new rule.** `hid_key_ascii` already returns 0 for any
usage while a GUI **or** an Alt bit is held, so both `cmd_role` spellings suppress the character on
their own. TERMSEL's rows with no `CMD` role are the stated exception: `Shift+←/→` and `Esc` still
type `0x1D`/`0x1C` and `0x1B`, pushed just ahead of the action, and the shell's line editor ignores
those bytes.

## 5. Rules the table keeps from the code it replaced

* **Extra modifiers held do not disqualify.** `⌃⌘⇧3` is still a screenshot (macOS's rule, and the
  old predicate's documented behaviour). A row requires the roles it names to be down and says
  nothing about the ones it does not.
* **Left and right of a modifier count alike.** The test is `!= 0` against a two-bit mask, never
  `== mask`.
* **The edge, not the level.** A boot report carries the set of keys HELD, so `resolve_edge` diffs
  against the previous report. A chord held for half a second arms one capture.
* **Precedence is the table's contract, and it is checked at compile time.** `keymap::no_shadow`
  refuses a table in which a row shadows a later one — same usage, and everything the earlier row
  requires also required by the later, so the later can never be reached. The hazard is real: a bare
  `PrtSc -> Screenshot` row above `Shift+PrtSc -> ScreenshotRegion` disarms the region chord in
  silence, with every gate green, because nothing on a wire says a row was never consulted. Both
  shipped tables assert it at the foot of `theme.rs`.

## 6. Delivery — CLOSED by APPCLIP, 2026-09-22

The screenshot actions were delivered from the first day: the decoders test `Action::is_capture()`
and call `prtscr::request()` exactly as they did before, so the wire keeps its shape and gains one
field:

```
:: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on EHCI -> capture armed action=screenshot ::
:: PRTSCR: PrintScreen (HID 0x46) down on xHCI -> capture armed action=screenshot ::
```

**The edit actions had no consumer and no delivery path, and this section named the seam rather than
half-building it. APPCLIP built it — the section is closed and the table below is the record of what
was owed, with what discharged each row.** The mechanism is [clipboard.md](clipboard.md); nothing in
`keymap.rs` or `theme.rs` changed to get it, which is the seam behaving as designed.

| File | Change that was named here | Discharged by |
|---|---|---|
| `pal.rs` | one variant on `enum Event`, e.g. `Action(crate::video::keymap::Action)`, plus its arm in `push_locked`'s `is_key`/`is_ptr` classification (neither — it is a third kind). | `Event::Action(keymap::Action)`, classified `is_act` — and, symmetrically, excluded from `EVQ_POP`: an event counted on one side of the pipeline only would drift `[uvug10]`'s occupancy reading (clipboard.md §5). |
| `arch/x86_64/syscall.rs` | an arm in the `Event -> (ty, payload)` match, which is **exhaustive**. | `(una_abi::INPUT_EV_ACTION, clipboard::action_code(a))`, folded onto the `Wheel` arm. |
| `arch/aarch64/syscall.rs` | the same arm in the aarch64 twin, also exhaustive. | the same arm, from the same `action_code` — one definition, so the two wires cannot drift. |
| `una_abi` | one `INPUT_EV_ACTION` code, if the action is to reach ring 3 at all. | `INPUT_EV_ACTION = 7`, payload = the action's discriminant. |
| the two decoder call sites | `crate::pal::push_event(Event::Action(act))` on the non-capture arm. | exactly that, plus `[clip] chord=… action=… via=… -> delivered` on the wire. |

Two things this section GUESSED and got wrong are worth keeping, because they are what a reader of a
named seam should expect to have to re-derive:

* *"which the terminal window would then receive like any other event **and ignore**"*. It does not
  ignore it. The terminal is the FIRST CONSUMER: `⌘V` types the clipboard into the current line
  through the same path a typed byte takes, and `⌘C` copies the line. What it cannot do is select,
  so `⌘X` and `⌘A` are witnessed refusals until a selection model exists.
* *"There is no clipboard in this tree (measured: `grep -r -i clipboard unaos/crates/kernel/src/video/`
  finds prose only)"*. True when written; `video/clipboard.rs` is the answer to it.

## 7. The fixture

`keymap::selftest()`, chained from `drivers::ehci::parser_selftest` — the same chain that hosts
`[tp] dispatch self-test`, for the same reason: the DECISION is the only part of an input path QEMU
can exercise, because QEMU has neither an Apple pad nor an operator's hands. It resolves only; it
arms no capture and requests nothing, so the lane behaves exactly as it did.

```
:: KEYMAP: table=crispy resolved=11 screenshot=ok region=ok copy=ok paste=ok cut=ok selectall=ok ctrl_c_ascii=0x03 ctrl_c_action=none pc_table_alt_c=copy prtsc=ok extramods=ok logout=ok rows=8/6 -> PASS ::
```

Every field is a comparison, not a restatement:

* `resolved=` counts AGREEMENTS, not rows — deleting a row lowers it.
* `screenshot=` / `region=` drive a REPORT PAIR, so the edge is under test and not just the mask.
* `ctrl_c_ascii=` is measured through `hid_key_ascii`, the decoder the shell actually reads.
* `pc_table_alt_c=` reads the same role row through the other table's `cmd_role`.
* `extramods=` is `⌃⌘⇧3`, the rule that survives from the predicate this replaced.

Gate: `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_SMC=1 UNAOS_QEMU_FULL=1 ./arroyo test 240`,
scored by [`scripts/specs/x86-wc.spec`](../../../../unaos/scripts/specs/x86-wc.spec).
Go-red: delete the `Cmd+C` row from `CRISPY_ROWS` — `copy=no`, `resolved=10`, `-> FAIL`.

## 8. What a Windows theme will have to add

Nothing in `keymap.rs`, and nothing in either driver. A second theme adds:

1. its own `*_ROWS` and `*_BINDINGS` in its theme module, with `cmd_role` set to whichever physical
   modifier that keyboard puts under the operator's thumb;
2. the rows for chords CRISPY has no concept of (`Ctrl+Alt+Del`, `Win+L`, an `F13` capture key) —
   each one is a row, and a new **meaning** is a new `Action` variant plus its `name()` arm;
3. a way to pick it, which is `keymap::active()` and nothing else.

What it will *not* be able to reuse is the `CTRL` role's emptiness: a Windows-shaped theme that binds
`Ctrl+C` to copy re-opens exactly the command-line question R61 closed, and that is a ruling to ask
for, not a row to write.
