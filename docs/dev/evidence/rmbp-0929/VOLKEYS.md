# VOLKEYS

## Design
Finding: no volume control exists. `drivers/hda.rs` walks the codec and finds the output path (pin..DAC) but keeps
nothing after `probe`; the KEYMAP table and both HID decoders carry no volume usage.
Mechanism:
- Keys: F10/F11/F12 = usages 0x43/0x44/0x45 press edges, seen in the EHCI report decoder
  (`drivers/ehci/mod.rs`, folded onto the `typematic_note_report` line) -> `video::status::volkey_usage`.
  Consumer-page usages (0xE2/0xE9/0xEA, which Apple sends in a separate report when fn-mode is media) are NOT
  decoded: the boot-report path never sees them. xHCI feed not wired (rMBP keyboard is EHCI).
- State: boot statics in `video/status.rs` (`VOL_LEVEL` 0..=16, default 12; `VOL_MUTED`). up/down move 1/16 and
  unmute; F10 toggles mute.
- Amp: `hda::vol::capture` (same-line call before `rings.stop` at the end of `probe`) records base, cad, each
  path node with an output amp and its step count; `hda::vol::apply(level, muted)` issues one
  SET_AMPLIFIER_GAIN_MUTE (both channels) per node, gain = level*steps/16, mute bit = flag. Rings re-initialised
  once lazily and kept. Independent of `hda-tone`. Called from the polled HID service, not an interrupt.
- Indicator: generic `(kind, level)` in `status.rs` (`osd_set`, `osd_text`, `osd_overlay`); `menubar` gather
  overlays it on the CAPTION for 1500 ms (`vol=N/16`, `muted`). `OSD_BRIGHT` renders `brightness=N/16` for the
  sibling (exec-rmbp-battery had not landed a brightness shape). Expiry repaints on the next composite only.
Witness: `:: VOLKEYS: key=<up|down|mute> level=<n>/16 muted=<0|1> amp_written=<0|1> indicator=1 -> PASS ::`
(fixture `status::volkeys_selftest`, `witness` feature, called after `battery_selftest`). Spec: x86-witness.spec tail.
No new knob (`hda` already gates the amp).

## Written
M1: all of the above, one commit. Uncompiled.
