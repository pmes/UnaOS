# Zenbook ledger — every finding, ticked once

> Arch ledger for the ASUS Zenbook S 14 OLED UM5606W track (AMD Ryzen AI 9 HX 370 Strix Point, Radeon 890M,
> MT7925, 2880x1800 OLED; runs on an external monitor). Cross-arch and shared-seam items live in
> `docs/dev/LEDGER.md` (ids `S<n>`, `P<n>`, and the seat-prefixed `SO`/`SP`/`SR`); rows below that
> cross reference it by id. Ids here are `Z<n>`, a distinct prefix so the arch ledgers stay mergeable.

Status: **open** · **fixed-unflown** (in tree, not on metal) · **flown** (scored on the wire) ·
**landed** · **dropped** (ruled out, with the ruling).
Owner is the lane that fixes it. `flies-on` is the bench that scores it: x86 QEMU certifies QEMU only;
the Zenbook bench is attended metal, and nothing boots there until Peter says.

## Z. Zenbook lane

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| Z4 | **ZENBOOKGOP — the x86 UEFI loader picks the GOP OUTPUT (every handle enumerated, the first EDID-bearing non-internal one picked, `UNAOS_GOP=<h>:<m>` override) and the kernel says `[display] gop` at the scale latch.** Measured at 281b09c3e: the loader read only `locate_handle_buffer[0]` (`get_handle_for_protocol`, `unaos/crates/bootloader/src/main.rs:517`); the mode rule inside a handle was already EDID-native > current > largest linear (`:552-646`), kept. `mod gopsel` (file tail, x86-gated folds at `:517` and `:625`) prints `[boot] gop handle=<i> modes=<n> current=<w>x<h> edid=<yes/no> size_mm=<w>x<h> out=<int/ext/->` per handle (out = the ACPI _ADR display type) and `[boot] gop pick handle=<i>/<n> why=<knob/only/edid/console>`; one handle (QEMU) picks handle 0 as before. The BootInfo `size_mm` grant was declined: EDID-CARRY already feeds `font_core::ui::edid_ppi` through `video::dpi`, and the carried EDID is now the picked output's. `video/dpi.rs:104` folds `display_note`: `[display] gop <w>x<h> stride= format= ppi= src=<knob/edid/none> scale=` (OLED 2880 px at about 243 ppi -> 2.0). Measured, not changed: `desktop_uefi::activate` has one caller, the Kepler takeover (`unaos/crates/kernel/src/drivers/gpu/kepler_display.rs:511`), so the Zenbook runs the QEMU-shaped desktop path; WC is generic (`set_framebuffer_wc` from `unaos/crates/kernel/src/main.rs:112`). `unaos/arroyo` validates `UNAOS_GOP`. OWED: GOPIGNITE (a GopBoot `desktop_uefi::Origin` so a Kepler-less x86 runs `activate_on`); ZENBOOK-RADEON (DCN 3.5 scanout: mode set beyond GOP, the dead eDP disabled, hotplug); HIDPI-EXTERNAL (the external monitor's real ppi vs Peter's preference, a ruling); OLED10BIT (the 10-bit panel; GOP is 8-bit); FBWCLEAF (`set_framebuffer_wc` assumes BAR-only leaves; an APU carve-out in a RAM-shared 1 GiB leaf needs a split); GOPSERIAL (no UART on the Zenbook: the loader lines show only on the screen). | zenbook (video/loader) | Zenbook bench (first boot is the bench's); x86 QEMU for the one-handle wire | fixed-unflown | design /root/unaos-bench/scratch/cloud/wave36/zenbookgop/design.md; code-only, uncompiled | ZENBOOKGOP |
