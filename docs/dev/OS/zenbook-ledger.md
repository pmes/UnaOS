# Zenbook ledger — every finding, ticked once

> Arch ledger for the ASUS Zenbook S 14 OLED UM5606W track (AMD Ryzen AI 9 HX 370 'Strix Point', Radeon 890M,
> MediaTek MT7925; its own panel is dead, so it runs on an external monitor). Cross-arch and shared-seam items live in
> `docs/dev/LEDGER.md` (ids `S<n>`, `P<n>`, and the seat-prefixed `SO`/`SP`/`SR`); rows below that cross reference it
> by id. Ids here are `Z<n>` — orin's arch ledger uses A-D, rmbp's A, B, E and pi's PI, so a distinct prefix keeps the
> files mergeable without renumbering.

> Rule (Peter, 2026-09-05): **an arc that fixes, flies, or drops a ledger item ticks it here in the same commit.**
> Nothing boots on the Zenbook until Peter says; the first boot is the bench's (Peter, 2026-10-27: "begin support for
> the um5606w hardware").

Status: **open** · **fixed-unflown** (in tree, not on metal) · **flown** (scored on the wire) · **landed** ·
**dropped** (ruled out, with the ruling).

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| Z3 | **ZENBOOKI8042 — the i8042 PS/2 keyboard controller as a transport into the one boot-keyboard decoder.** `drivers/i8042.rs` (NEW, CHARTER Kernel — driver): init with readbacks and named refusals (status 0xff = SKIP no-controller; AD/A7, flush, config byte, AA = 0x55 else REFUSED, AB = 0x00, AE, keyboard FF = FA AA, F0 02, F2 id read with XLAT off, then XLAT on), called once from `ehci::hid_task_start`; set-1 scancodes through ONE table at keymap's tail (`keymap::set1_usage`, HID usages) into a synthetic boot report handed to EHCI's `decode_boot_keyboard` (Bluetooth's shape) only when it changes, drained by `ehci::service_ehci_hid` under `EHCI_HID`; IRQ1 through the opt-in IOAPIC (`ioapic::isa_gsi` tail fn, runtime `vectors::alloc("i8042")`, `interrupts::i8042_irq_handler` tail ISR, masked then unmasked) else polled with the reason on the line. Platform slot `platform_skip` returns None until the seat composes `crate::hw::platform()` (ZENBOOKSURVEY) at the fold. Owed, unbuilt: ZENBOOK-TOUCHPAD (I2C-HID over AMD DesignWare I2C, address from ACPI _CRS); Fn keys via asus-wmi; set 2 raw; typematic rate 0xF3; lock LEDs 0xED (counted led_owed=); Pause/E1; HIDBOOT-SHARED (the decoder out of the EHCI module, a transport label, the ehcihid gate dropped); the FADT 8042 flag; the aux port. | zenbook | QEMU q35 (`tests i8042`); Zenbook bench | fixed-unflown | code-only, uncompiled; expected wire `[i8042] keyboard id=ab83 set=1xlat irq=polled -> ready` and `:: I8042: init=ready irq=polled table=ok inject=e0,1d,e0,9d mods=10,00 decoded=2 dropped=0 -> PASS ::` | ZENBOOKI8042 |
