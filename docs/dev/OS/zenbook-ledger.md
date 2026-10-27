# Zenbook ledger — every finding, ticked once

> Arch ledger for the ASUS Zenbook UM5606W track (AMD Ryzen AI 9 HX 370 "Strix Point", x86_64; Peter 2026-10-27:
> "begin support for the um5606w hardware"). Cross-arch and shared-seam items live in `docs/dev/LEDGER.md` (ids `S<n>`,
> `P<n>`, and the seat-prefixed `SO`/`SP`/`SR`); the rMBP's x86 findings live in `docs/dev/OS/rmbp-ledger.md` (A, B, E).
> Ids here are `Z<n>` — orin uses A-D, rmbp A/B/E, pi `PI<n>`, so a distinct prefix keeps the files mergeable without
> renumbering. The queue (the ORDER) is `docs/dev/OS/zenbook-queue.md`; the hardware sheet is
> `docs/dev/OS/01_BOOT_HAL/um5606w.md`.

> Created 2026-10-27 by ZENBOOKSURVEY (wave 36, cloud seat), with Z1. The `owner` column carries `rmbp` until
> `unaos/scripts/ledger-check.sh` learns a `zenbook` owner (its owner set is {orin, pi, rmbp, shared-gate}; the queue's
> ZENBOOK-LEDGERGATE row): the x86 seat carries this track and the code is the x86 kernel. Nothing has booted on the
> Zenbook; the first boot is the bench's.

## Findings

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| Z1 | ZENBOOKSURVEY — the x86 kernel's Mac assumptions measured and the UM5606W track opened: the loader hands the SMBIOS entry point (`BootInfo.smbios_ptr`, tagged `SMBIOS_TAG`), `hw::platform()` reads the Type 1 product and prints `[hw] platform= dmi=` once at `pci.rs init`; the SMC, gmux and HDA probes ask `hw::gate` and print `[hw] skip <probe> platform=` on a positively identified non-Apple machine (the rMBP, QEMU, any Apple machine and a machine with no readable SMBIOS run today's code); the Strix Point PCI ids name their census rows (`hw::pci_id_name`); queue, ledger and hardware sheet created | rmbp | QEMU x86 lanes (`tests platform`); the Zenbook bench's first boot | fixed-unflown | docs/dev/OS/01_BOOT_HAL/um5606w.md; `tests platform` | — |
> Arch ledger for the ASUS Zenbook S 14 OLED UM5606W track (AMD Ryzen AI 9 HX 370 "Strix Point", Radeon 890M,
> MediaTek MT7925, NVMe, USB4; its own panel is dead, so it runs on an external monitor). Cross-arch and
> shared-seam items live in `docs/dev/LEDGER.md` (ids `S<n>`, `P<n>`, and the seat-prefixed `SO`/`SP`/`SR`);
> rows below cross reference it by id. Ids here are `Z<n>` — orin's arch ledger uses A-D, rmbp's A, B, E and
> pi's PI, so a distinct prefix keeps the files mergeable without renumbering.
> Created 2026-10-27 by ZENBOOKNVME (wave 36) because the file was absent at its cut (281b09c3e);
> ZENBOOKSURVEY creates the same file in flight beside it, and the seat unions the two at the fold.
> Peter, 2026-10-27: "begin support for the um5606w hardware". NO METAL until Peter says: the first boot is
> the bench's.
Status: **open** · **fixed-unflown** (in tree, not on metal) · **flown** (scored on the wire) ·
**landed** · **dropped** (ruled out, with the ruling).
## Z. Zenbook lane
| Z2 | **ZENBOOKNVME — the NVMe host controller, rung 1.** `drivers/nvme.rs` (NEW, CHARTER Amber Bytes / driver, `nvme` knob, x86_64): probe by class 01/08/02, 64-bit BAR0, reset (CC.EN=0) and enable bounded by CAP.TO, admin queue depth 8, IDENTIFY controller and namespace 1, ONE I/O queue pair depth 32, READ/WRITE with PRP1/PRP2/one PRP list page through a 32 KiB bounce, completions polled on the phase bit against a 1000 ms bound (`[nvme] cmd timeout opcode=` marks the controller), the masked bounded hold `with_ctrl`; publishes into `block::register_nvme` (block.rs tail, AHCI first rung's free-function registry shape); `[nvme] ctrl mn="<m>" ns=1 lba=<n> blocks=<n> -> ready`. `tests nvme`: sector 0 through `block::read_blocks_nvme`, and on QEMU's controller only a write + read-back + restore of 8 scratch blocks: `:: NVME: ctrl=ok ns=1 rw=ok -> PASS ::`. Knob five-place (arroyo, builder feats + `-device nvme,drive=nvme0,serial=una` on a fresh target/nvmefixture.img, Cargo.toml, k8-reach.registry, banner-cert.sh). OWED (storage.md §5): NVMEBOOT (handle + source + root grant), NVMEWRITE, MSI-X, multiple namespaces, TRIM, the SMART/Health log page, NVMe on the Orin, BOUNDEDWAIT, DISKHOLD. | rmbp (cloud wave 36) | QEMU `UNAOS_NVME=1` then the Zenbook bench | fixed-unflown | code-only, uncompiled at commit; the seat's x86all leg compiles it; the bench runs `tests nvme` | — |
> MediaTek MT7925; its own panel is dead, so it runs on an external monitor). Cross-arch and shared-seam items live in
> `docs/dev/LEDGER.md` (ids `S<n>`, `P<n>`, and the seat-prefixed `SO`/`SP`/`SR`); rows below that cross reference it
> by id. Ids here are `Z<n>` — orin's arch ledger uses A-D, rmbp's A, B, E and pi's PI, so a distinct prefix keeps the
> files mergeable without renumbering.
> Rule (Peter, 2026-09-05): **an arc that fixes, flies, or drops a ledger item ticks it here in the same commit.**
> Nothing boots on the Zenbook until Peter says; the first boot is the bench's (Peter, 2026-10-27: "begin support for
> the um5606w hardware").
**dropped** (ruled out, with the ruling).
| Z3 | **ZENBOOKI8042 — the i8042 PS/2 keyboard controller as a transport into the one boot-keyboard decoder.** `drivers/i8042.rs` (NEW, CHARTER Kernel — driver): init with readbacks and named refusals (status 0xff = SKIP no-controller; AD/A7, flush, config byte, AA = 0x55 else REFUSED, AB = 0x00, AE, keyboard FF = FA AA, F0 02, F2 id read with XLAT off, then XLAT on), called once from `ehci::hid_task_start`; set-1 scancodes through ONE table at keymap's tail (`keymap::set1_usage`, HID usages) into a synthetic boot report handed to EHCI's `decode_boot_keyboard` (Bluetooth's shape) only when it changes, drained by `ehci::service_ehci_hid` under `EHCI_HID`; IRQ1 through the opt-in IOAPIC (`ioapic::isa_gsi` tail fn, runtime `vectors::alloc("i8042")`, `interrupts::i8042_irq_handler` tail ISR, masked then unmasked) else polled with the reason on the line. Platform slot `platform_skip` returns None until the seat composes `crate::hw::platform()` (ZENBOOKSURVEY) at the fold. Owed, unbuilt: ZENBOOK-TOUCHPAD (I2C-HID over AMD DesignWare I2C, address from ACPI _CRS); Fn keys via asus-wmi; set 2 raw; typematic rate 0xF3; lock LEDs 0xED (counted led_owed=); Pause/E1; HIDBOOT-SHARED (the decoder out of the EHCI module, a transport label, the ehcihid gate dropped); the FADT 8042 flag; the aux port. | zenbook | QEMU q35 (`tests i8042`); Zenbook bench | fixed-unflown | code-only, uncompiled; expected wire `[i8042] keyboard id=ab83 set=1xlat irq=polled -> ready` and `:: I8042: init=ready irq=polled table=ok inject=e0,1d,e0,9d mods=10,00 decoded=2 dropped=0 -> PASS ::` | ZENBOOKI8042 |
