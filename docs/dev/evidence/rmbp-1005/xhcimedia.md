# XHCIMEDIA (rmbp-ledger B438) — one fn-row router, both HID pumps

**Finding (wire + tree).** Flights 24 and 25: the internal keyboard came up on EHCI every boot
(`ehci:kbd-armed`, `EHCI-HID: KEY withheld` x163, no `xHCI: KEY` line) — so on those flights the F-row
worked; the hole is the OTHER path. In the tree at 4ead840a: F1/F2 reach `brightkeys::key` from both
pumps (the theme row via `keymap::resolve_edge` -> `pal::push_event`'s intercept); F7/F8/F9 and F10/F11/F12
reach `status::volkey_usage` from the EHCI pump on every arch, but from the xHCI pump ONLY on aarch64
`desktop_firmware` (inside its `baremetal` typematic block). An x86 xHCI keyboard (the rMBP's internal one
on the firmware path that routes it there, or any USB keyboard on the root ports) gets no media and no
volume. The pumps run the boot protocol: the fn-row arrives as keyboard-page F-key usages (0x3A..0x45), no
consumer page — so the router keys on those.

**Seam: Kernel — kernel-by-ruling.** One module `video/fnrow.rs`: `fn_row_usage(path, usage, mods)` is
called by both pumps on each press edge with the raw usage. It is the only place an F-row action fires:
F1/F2 resolve through the theme's keymap row (the theme stays the binding, R60) and step the backlight;
F7/F8/F9 latch the player's transport (`player::media_key`); F10/F11/F12 step the volume model
(`status::volkey_usage`: store + amp + bezel). `volkey_usage` loses its player call (the router owns it) and
`pal::push_event` drops a brightness Action instead of stepping (the router already did).

**Milestones.** M1: the router + both pump call sites (EHCI's loop moves to it; xHCI gains it on every arch;
the aarch64-only call is removed). M2: `tests xhcimedia` (registered on the desktop pass, R80) feeds the
eight usages through the router on both path tags and reads the model back.

**Witness.** Glass, per press: `[hid] fnrow path=<ehci|xhci> usage=0x<nn> -> <action>`.
`tests xhcimedia`: `:: XHCIMEDIA: router=one paths=ehci,xhci keys=8 bright=ok media=ok volume=ok -> PASS ::`
(the ledger's `keys=9` counts nine; the F-row's routed keys are eight: F1 F2 F7 F8 F9 F10 F11 F12).

**Owed.** Consumer-page usages (a report-protocol keyboard's 0xB5/0xB6/0xCD/0xE2/0xE9/0xEA) — the pumps
parse no report descriptor (boot protocol only); a metal flight with the keyboard on xHCI to read the
`path=xhci` line.
