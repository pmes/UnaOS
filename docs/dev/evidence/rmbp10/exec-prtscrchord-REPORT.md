# PRTSCRCHORD — executor report (rmbp seat, x86_64)

- **Branch / sha:** `exec-rmbp10-prtscrchord` @ `4bae18734998a8103b4caf626cba07f9115c5efa`
  (parent `647f485a`, tree clean after commit)
- **Worktree:** `/home/pmes/unaos-bench/scratch/rmbp10/exec-prtscrchord/wt`
- **Commit:** `x86/input: PRTSCRCHORD — bind ⌘⇧3 and ⌘⇧4 to the Print Screen capture`
- **Not pushed** (seat never pushes). Push Peter will need: `exec-rmbp10-prtscrchord` (or the
  rmbp seat merges `4bae1873` onto `hw-rmbp` first and Peter pushes `hw-rmbp`).

## Files

| File | Lines | What |
|---|---|---|
| `unaos/crates/kernel/src/drivers/xhci/mod.rs` | 329-380 | `HID_USAGE_DIGIT_3/4` consts + `hid_screenshot_chord_edge(cur_keys, prev_keys, modifiers) -> Option<&'static str>` beside `hid_print_screen_edge`; layer rationale, no-keystroke argument, ⌘⇧4 region-select reservation comment (line 377) |
| `unaos/crates/kernel/src/drivers/xhci/mod.rs` | 4981-4996 | xHCI decoder call site: `else if` after the 0x46 test → witness + `prtscr::request()` |
| `unaos/crates/kernel/src/drivers/ehci/mod.rs` | 16402-16418 | EHCI `decode_boot_keyboard` call site (the rMBP internal-keyboard path): same `else if`, same witness, same `request()` |
| `docs/dev/OS/08_VIDEO/screenshot.md` | §1 table + PRTSCRCHORD paragraph; §7 witness lines; §9 QEMU proof + metal note | doc named by the arc |

Untouched: `video/prtscr.rs` (capture path unchanged), `main.rs`, all aarch64 files.

## Layer chosen, and why

The chord is a modifier byte + a usage. The HID boot report (`report[0]` modifiers, `report[2..8]`
usages) is the only layer that sees both in the same report; above the driver `pal::Event::Key` is a
bare `u8` with no modifier field, and `hid_key_ascii` folds every GUI-held usage to 0, so ⌘⇧3 does
not exist above the decoders. Both decoders already had `modifiers`, `cur_keys`, `prev_keys` in scope
at the 0x46 site. The predicate is shared (`xhci::` module, like `hid_print_screen_edge` and
`HID_LOCK_KEYS`) so EHCI and xHCI cannot disagree.

- Left/right both count: `HID_MOD_GUI = 0x88` (bit3|bit7), `HID_MOD_SHIFT = 0x22` (bit1|bit5).
- Edge, not level: digit present now and absent last report (same diff as 0x46 / lock keys).
- Extra modifiers (Ctrl/Alt) do not disqualify — ⌃⌘⇧3 is still a screenshot on macOS.
- `else if` after the 0x46 test: a report carrying both arms exactly once.
- Caller does only what the 0x46 caller does: one `serial_println!` witness + `prtscr::request()`
  (one relaxed store + counter). No `WRITER.lock()`, no I/O, capture path untouched (LOCKFIX).

**Keystroke suppression:** 0x46 is suppressed by its `(0,0)` table entry (fold = 0). The chord reaches
the same result through the existing `hid_key_ascii` rule: any usage with a GUI bit held returns 0
("GUI and Alt suppress the key entirely"). No `Key('3')`/`Key('#')` is ever pushed. The release fold
deliberately ignores GUI and emits a lone `KeyUp('#')`/`KeyUp('$')` — the documented "spurious
release, safe" case; left alone (suppressing it would reintroduce the Boot AJ stuck-key defect).

**⌘⇧4:** bound to the same whole-screen capture; region-select reserved (one-line comment); the
witness says `chord=cmd-shift-4` so the two are distinguishable on the wire.

## Gates (all foreground, all read)

| Gate | Exit | Evidence |
|---|---|---|
| `./arroyo check` | 0 | `✅ x86_64 OK`, `✅ aarch64 OK`, `✅ x86-all`, GATE-FAMILY OK, GATE-KNOB OK (152 declared / 0 phantom / 0 dead); no warnings at the xhci/ehci lines touched (`check.log`) |
| `UNAOS_WC=1 ./arroyo test 150` | 0 | banner `⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`; awk fault scan empty (`serial-test150.log`) |
| `./arroyo test-arm 60` | 0 | `✅ aarch64 test complete` (`serial-testarm.log`) |
| Chord fixture: `UNAOS_WC=1 UNAOS_QEMU_EXTRA="-qmp tcp:127.0.0.1:4491,server,nowait" ./arroyo test-fat sf 200` + `qmp_chord.py` | 0 | below (`serial-testfat-chord.log`, `qmp_chord.log`, `testfat-chord.log`) |

Fixture: the x86 harness has no built-in chord injector; the established precedent (screenshot.md §9,
`scripts/qmp_type.py`) is QMP `send-key` onto the `usb-kbd` the builder attaches to `ehci.0` —
decoded by `decode_boot_keyboard`, the same function that decodes the rMBP's internal keyboard.
`qmp_chord.py` (scratch only, not committed) sends `meta_l`+`shift`+`3` as ONE `send-key` with
`hold-time` 120 ms → boot report modifiers `0x0A` (LGUI|LShift) + usage `0x20`; 45 s later the same
with `4` (usage `0x21`). No `prtscrst`, so the chord was the only possible trigger.

### Serial excerpt (test-fat run, `awk '/PRTSCR|prtscr/'`)

```
:: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on EHCI -> capture armed ::
:: PRTSCR: SCREEN0.PNG 1280x800 3073098 bytes -> OK ::
:: PRTSCR: [prtscr] chord=cmd-shift-4 (GUI+Shift+digit) down on EHCI -> capture armed ::
:: PRTSCR: SCREEN1.PNG 1280x800 3073098 bytes -> OK ::
```

`awk '/KEY:|KEYUP:|hidkeys/'` on the same log — the ONLY key lines in the whole run:

```
EHCI-HID: KEYUP: '#' (scancode 0x20)
EHCI-HID: KEYUP: '$' (scancode 0x21)
```

No `KEY:` press for `3`/`4`/`#`/`$` → the chord typed nothing. Fault scan: empty.

PNGs pulled host-side with `mcopy -i builder/fat-sf.img` (copies in this dir): both
`PNG image data, 1280 x 800, 8-bit/color RGB, non-interlaced`, 3073098 bytes, every chunk CRC valid,
IEND last, IDAT inflates to exactly `800 * (1 + 1280*3) = 3072800` bytes.

## Metal procedure (rMBP, tonight)

Build/stage as for flight 5 (same knobs — the chord path is in the default `ehcihid` build, no new
knob). The x86 image must be built from `4bae1873` or a descendant.

1. Boot the rMBP from the usual SD. Wait for the shell (and, on a `prtscrst` build, the PRTSCR-VOL
   veto line `program source is sdhc and vetoes writes ...`).
2. Plug a FAT-formatted USB stick into the other port. On a `prtscrst` build expect
   `writable volume arrived (source=usb ...)`; otherwise nothing — the stick is adopted silently on
   the next storage-ready pass.
3. On the INTERNAL keyboard press and release **⌘⇧3** (hold ⌘ and ⇧, tap 3). Either ⌘, either ⇧.
4. Expect on the serial wire, in order:
   ```
   :: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on EHCI -> capture armed ::
   :: PRTSCR: SCREEN<n>.PNG 2880x1800 <bytes> bytes -> OK ::
   ```
   `<n>` = first free index on that stick (flight 5 left `SCREEN2.PNG`, so likely `SCREEN3.PNG`
   there, `SCREEN0.PNG` on a fresh stick). Capture takes seconds at 2880x1800 (flight 5 timing).
   Also expect one `EHCI-HID: KEYUP: '#' (scancode 0x20)` — harmless, documented. There must be NO
   `EHCI-HID: KEY:` line for `3`/`#` and no `3`/`#` glyph on the console line.
5. Press **⌘⇧4** the same way → `chord=cmd-shift-4` witness, then `SCREEN<n+1>.PNG ... -> OK`.
6. Unplug the stick, mount it on the host: `SCREEN<n>.PNG` and `SCREEN<n+1>.PNG` at the root;
   `file` → `2880 x 1800, 8-bit/color RGB`; check IHDR+IEND as in flight 5.

Reading a failure: `awk '/chord=cmd-shift/' <serial>` empty → the internal keyboard's report did not
carry GUI|Shift + 0x20 (census `requests` unchanged); fly `UNAOS_USBDEBUG` to see the raw report.
Witness present but no `-> OK` → the refusal line names why (no writable volume / read-only / busy),
same ladder as the 0x46 key.

## Artefacts in this dir

`check.log`, `test150.log`, `serial-test150.log`, `testarm.log`, `serial-testarm.log`,
`testfat-chord.log`, `serial-testfat-chord.log`, `qmp_chord.log`, `qmp_chord.py`, `SCREEN0.PNG`,
`SCREEN1.PNG`.
