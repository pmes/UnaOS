# BEZEL (rmbp-ledger B405, MACPARITY row 26) — the brightness / volume bezel

## Design (executor, cut from 7811df99)

**Finding.** F1/F2 (`brightkeys::key` -> `backlight::stage_step`, applied on the desktop pass by
`brightkeys::service` through `set_level_via`, read back) and F10/F11/F12 (`status::volkey_usage`, from the
xHCI/EHCI report edge, writing `hda::vol::apply`) move the panel and the amp; the only readout is the
menubar caption's `brightness=N/16` / `vol=N/16` text (`status::osd_*`). Nothing draws on the glass. No
`bezel` exists in the tree (searched).

**Seam.** `video/bezel.rs` — `//! CHARTER: Kernel — wm`: a compositor draw, the toast's shape
(`wm::overlay_open`, the chromeless compat row owner 0: `hit_test` never names it, nothing focuses it, no
input). No service, no store, no second mixer: the bezel holds only "what to show and until when".
- the key paths ARM it (atomics only — `brightkeys::key` may run in the decoder's context;
  `volkey_usage` in the polled HID service): `bezel::arm(kind)`;
- `brightkeys::service` (already on the desktop pass on both arches, and at the login screen / setter —
  BRIGHTSTEP proves the keys work there) calls `bezel::service()` AFTER the backlight write, so the level it
  reads is the register READBACK (`backlight::cur_raw`), mapped to segments by BRIGHTSLIDER's own
  `pos_for_raw(raw, max, 16)` — the slider's scale;
- volume: the level is the codec's OUTPUT AMP read back (`hda::vol::readback`, GET_AMPLIFIER_GAIN_MUTE on the
  first node `vol::capture` recorded — the same CORB/RIRB rings `vol::apply` drives), inverted over the
  node's step count; mute is the amp's own mute bit. No amp (QEMU, aarch64): the model `status::volume()`,
  said on the wire (`src=model`).

**Milestones.**
- M1 `video/bezel.rs`: arm / service / paint (rounded square, our glyphs: sun, speaker + 0–3 waves, crossed
  speaker; 16-segment bar), centre-bottom, re-arm on repeat (repaint + present the same row), close at
  1500 ms. Wire: `[bezel] show kind=<brightness|volume|mute> level=<n>/16 ms=1500 src=<readback|amp|model> win=<n>`,
  `[bezel] faded kind=<k> after_ms=<n>`.
- M2 the routes: `brightkeys::key` arms + `brightkeys::service` drives; `status::volkey_usage` arms;
  `hda::vol::readback`.
- M3 `tests bezel` (R80: registered from the desktop pass, never run at boot):
  `:: BEZEL: brightness=ok volume=ok mute=ok fade_ms=1500 scale=shared -> PASS ::`.

**Witness the next flight reads.** Press F2, F1, F11, F12, F10 on the glass; the wire says
`[bezel] show kind=brightness level=<n>/16 ms=1500 src=readback …` per press and `[bezel] faded …` after;
then `tests bezel` -> `:: BEZEL: … -> PASS ::`.

**Owed.** Translucency and true rounded-corner transparency: the compat row is xRGB, opaque; the square is
an opaque dark face with a drawn rounded frame (a compositor alpha row is the seat's design question). The
fade is a close at 1500 ms, not an alpha ramp. aarch64: wire-only (no overlay row there — the toast's
shape). No knob: the bezel rides `wc` (x86) like the toast.
