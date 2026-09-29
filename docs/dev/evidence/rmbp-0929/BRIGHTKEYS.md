# BRIGHTKEYS — the rMBP brightness keys drive the gmux backlight

**Motivation.** No code handled F1/F2 (grep `brightness` found only iGPU register dumps); the
panel backlight could not be changed from the desktop.

**Mechanism.**
- `video/theme.rs` `CRISPY_ROWS`: two rows, HID 0x3A/0x3B, `roles: 0`, actions `Action::BrightnessDown/Up`
  (`video/keymap.rs`, `is_brightness()`; wire codes 17/18 in `video/clipboard.rs::action_code`).
- Both HID decoders already push non-capture actions via `pal::push_event`; `pal.rs` `push_event` (same-line fold)
  consumes brightness actions there -> `video/brightkeys.rs::key` (atomics only: level 0..16, pending flag,
  `status::bright_show`). Nothing is queued.
- `desktop_uefi.rs` service pass (same line as `status::poll()`) calls `brightkeys::service()`: applies the pending
  step with `igpu::gmux_set_brightness(level*0xFFFF/16)` (gmux index 0x74, 32-bit, values at 0x7C0..0x7C3; cfg `gmux_igd`,
  else `gmux_written=0`) and prints the witness.
- Indicator: `video/status.rs` `bright_show/bright_item/bright_clear` (1.5 s, lock-free); `video/menubar.rs` Model
  field `bright`, folded into `signature()`, drawn as `BRT nn/16` at `bright_slot` = left of the battery's reserved item.

**Milestones.** M1 keys->action->level (+gmux write); M2 bar indicator; M3 witness + spec pins.

**Witness.** `:: BRIGHTKEYS: key=<up|down> level=<n>/16 gmux_written=<0|1> indicator=1 -> PASS ::` (FAIL when the
indicator is not live right after the key). First service pass runs a boot fixture (up, down; no gmux write; indicator
cleared) so QEMU/bench replay print it with `gmux_written=0`.

**Pins.** `unaos/scripts/specs/x86-witness.spec`: REQUIRE the PASS line, FORBID `-> FAIL`.

**Knob.** None new: rides `wc` (module) and the existing `gmux_igd` (`UNAOS_GMUX_IGD`) for the register write.

## Written
All milestones in one commit. Not compiled or run (R76). Caveats: DEFAULT_LEVEL 12 is assumed (register not read back);
`gmux_igd` is required for a real write; F1/F2 are still not typed to the terminal (consumed at push_event).
