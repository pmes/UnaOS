# ZENBOOK — WHAT IS IN FLIGHT AND WHAT IS OWED (the ASUS Zenbook UM5606W track queue, R45)

One file, fixed path, not per-round. Kept current AS WORK HAPPENS, never at close. Only jobs that need the Zenbook's
metal or Zenbook-only code live here (the Strix Point drivers, the Zenbook image line, its specs); shared work is a row
in the trunk queue `docs/dev/QUEUE.md`, rMBP-only work in `docs/dev/OS/rmbp-queue.md`. Ledger:
`docs/dev/OS/zenbook-ledger.md` (Z<n>). Hardware sheet: `docs/dev/OS/01_BOOT_HAL/um5606w.md`. Every row cites its
ledger id — the ledger holds the finding, this file holds the ORDER. Re-derive every sha before acting.
Created 2026-10-27 by ZENBOOKSURVEY (wave 36; Peter 2026-10-27: "begin support for the um5606w hardware" — it joins
the Orin and the Pi in the metal rotation). The machine's own panel is dead: it runs on an EXTERNAL monitor.

**THE NO-METAL RULE (from the arc, Peter's):** nothing boots on the Zenbook until Peter says; the first boot is the
bench's (ZENBOOK-FIRSTBOOT). Branches stay `exec-rmbp-<name>` (the cloud's convention).

## STATE

STATE (2026-10-27, Z1): image 0 — nothing flown; the first boot plan is ZENBOOK-FIRSTBOOT below. The x86 kernel
identifies the machine from SMBIOS (`[hw] platform=zenbook_um5606w`, Z1) and keeps the Apple SMC, the gmux and the
HDA codec drive off it (`[hw] skip smc|gmux|hda`); the four sibling arcs cut with Z1 (ZENBOOKNVME, ZENBOOKI8042,
ZENBOOKGOP, ZENBOOKMT7925) are the next rows. No Zenbook image line exists yet (ZENBOOK-IMAGELINE).

## OWED (in order; each names its ledger id once it has one)

1. **ZENBOOK-FIRSTBOOT** — the card recipe and the bench's first boot. `./arroyo esp-x86` with the ZENBOOK-IMAGELINE
   knobs onto a USB stick, into a USB-A or USB-C port; F2/Esc into the ASUS firmware, Secure Boot OFF, boot the stick.
   The FTDI serial adapter goes on a port of the ONE xHCI the kernel brings up: read `[xhci] controllers=<n> picked=<i>
   wire names the one that bound). Expected first lines and refusals: um5606w.md §6. Bench only, Peter's go.
2. **ZENBOOK-IMAGELINE** — In flight (ZENBOOKIMAGELINE, Z6): `unaos/scripts/image-line-zenbook.env` (25 kernel knobs +
   the `[media]` UNAOS_MT7925_FW_PATH: the rMBP line without its Mac-only knobs and the SATA set, plus NVME) and
   `./arroyo esp-zenbook` (esp-x86 under that file after every inherited `UNAOS_*` is unset, bar four bench values); the
   check leg `x86-zenbook-image`; um5606w.md §5/§6. Owed: ZENBOOK-OLDEST in flight (ZENBOOKOLDEST, Z12: ONE `boot::oldest()`, the cfg
   default overridden by `hw::platform()`: the Zenbook prints zenbook 2024, `@zenbook` bootlist lines), ZENBOOK-QEMULANE (an arroyo `test` lane with the Zenbook knobs), ZENBOOK-LINEGATE (a
   `knob-parity-zenbook` gates row running the x86 `--line` mode on the file) — both in flight (ZENBOOKLANE, Z13: `./arroyo test-zenbook`, x86-zenbook.spec, `UNAOS_PLATFORM`; the typed verdicts: in flight (ZENBOOKLANETYPED, Z16: the first-user form and six `tests <verb>` lines through ONE typist, `unaos/scripts/typist.sh`); the lane's boot list: in flight (ZENBOOKLANELIST, Z19: `unaos/fixtures/lanes/zenbook/TESTS.LIST` through the ONE `bootlist_stage` and the builder's `UNAOS_BOOTLIST_STAGE` carry)); row 1's recipe should name esp-zenbook.
   bdf=<b:d.f>` off the glass, and if the FTDI did not bind rebuild with `UNAOS_XHCI_PICK=<i>` for the next i (Z7). Expected first lines and refusals: um5606w.md §6. Bench only, Peter's go.
3. **ZENBOOK-NVME** — `drivers/nvme.rs` (class 0108, vendor varies): the root volume. Rung 1 ZENBOOKNVME (Z2); in flight (NVMEBOOT, Z9: the handle, the root grant, the granted write); in flight (NVMEBATCH, Z22: one WRITE and one read-back per run, a UnaFS block one run; NVMEROOTONE, the grant refused on a second UnaFS volume). Owed: NVMEINSTALL (Peter's call), NVME4KN, MSI-X, TRIM, SMART log page, NVMEBATCH-COMMIT, NVMEBATCH-READ.
4. **ZENBOOK-I8042** — `drivers/i8042.rs`: the internal keyboard on the EC's 8042 into the input route. In flight
   (ZENBOOKI8042; it asks `hw::platform()`).
5. **ZENBOOK-GOP** — the loader picks the CONNECTED output's native mode (the dead internal panel enumerated or not:
   um5606w.md §4), `desktop_uefi` + dpi for an external monitor. In flight (ZENBOOKGOP). The desktop's ignition without a Kepler takeover: in flight (GOPIGNITE, Z10: `desktop_uefi::gop_ignite`, Origin `gop`, the Zenbook only).
6. **ZENBOOK-XHCIPICK** — the kernel brings up ONE xHCI, the first class 0C/03/30 in walk order (`PciScanner::scan`,
   `unaos/crates/kernel/src/arch/x86_64/pci.rs:895`); Strix Point has four (1022:151a/151b/151e/151f) plus two USB4
   NHIs (151c/151d). Measure which physical ports the first one serves; then bring up every xHCI or pick by port.
7. **ZENBOOK-MT7925** — `drivers/mt7925.rs` (14c3:7925): probe + firmware pin (Z5, fixed-unflown). S2 driver-own and the chip id through
   the L1 remap, Zenbook only (MT7925S2, Z11). S3 (MT7925S3, Z15) the WFSYS reset and the FWDL TX + event RX rings
   with WFDMA0 enabled. In flight (MT7925S4, Z17): S4 the patch and RAM code over the MCU, FW_START, N9_RDY; S5..S10 owed in `docs/dev/OS/06_NETWORK_STACK/mt7925.md`.
6. **ZENBOOK-XHCIPICK** — BUILT by ZENBOOKXHCIPICK (Z7, fixed-unflown, code-only): the driver is single-instance, so
   the kernel brings up ONE xHCI — the first class 0C/03/30 in walk order (today's tuple, unchanged) unless the build knob
   `UNAOS_XHCI_PICK=<index|b:d.f>` (decimal; arroyo validates) names another (`drivers::xhci::pick`, fed the PCI registry
   at `unaos/crates/kernel/src/arch/x86_64/pci.rs:895`); one census line `[xhci] controllers=<n> picked=<i> bdf=<b:d.f>`
   (+ `via=knob` / `refused=<v>`), and `hw` labels the picked controller. Strix Point: four (1022:151a/151b/151e/151f).
   In flight (ZENBOOKXHCIALL, Z21, code-only): every xHCI up in walk order after the pick, one loan slot, rings and ack bases per
   controller, `xhci1` MSI-X for controller 1 (2..3 polled); on 1..3 only the FTDI door binds (wherever it enumerates first), other devices print `[xhci] ctrl=<i> port=<p> device <vid>:<pid>` and are refused by name. Owed: ZENBOOK-XHCIALL-CLASS, ZENBOOK-XHCIALL-MSIX, ZENBOOK-XHCIALL-LADDERS, USB tunnelled behind the NHIs (ZENBOOK-USB4), the Fn-key keyboard (ZENBOOK-EC).
8. **ZENBOOK-EC** — battery, lid and the Fn keys through the ACPI embedded controller (the SMC's job on the rMBP). In flight (ZENBOOKEC, Z8): rung 1 is the ECDT-located transport + a 256-byte RAM census at pci init; the battery decode is ZENBOOK-EC-BATTERY: in flight (ZENBOOKECBATTERY, Z20): `ec::battery` reads a platform's `EC_BATTERY_OFFSETS` row as the EC arm of the status model's ONE board source (`video::status::board_raw` + `ec_arm`, `src=ec`), `tests ecbattery`; the table ships EMPTY, so the Zenbook prints `[power] source=none platform=zenbook_um5606w why=no-offsets` until ZENBOOK-EC-OFFSETS (the first boot's `[ec] ram` dump, plugged and unplugged, or the DSDT's EC Fields). ZENBOOK-EC-LID: in flight (ZENBOOKECLID, Z27: the lid by an EMPTY `EC_LID_OFFSETS` table as the EC source beside MSLD in `lidsleep::read_msld`, census `lid=`, `tests eclid` the bench's diff probe). Owed: ZENBOOK-EC-LIDROW, ZENBOOK-EC-LIDLINE, ZENBOOK-EC-LIDSPEC, ZENBOOK-EC-QEVENTS (`_Qxx`), ZENBOOK-EC-DRAIN (the extras and the sleep drain need unmeasured capacity/cycle offsets), ZENBOOK-SUSPEND.
9. **ZENBOOK-TOUCHPAD** — the touchpad over I2C-HID on the AMD DesignWare I2C controller. In flight (ZENBOOKTOUCHPAD, Z14): rung 1 is `drivers/dwi2c.rs`, the FCH's fixed I2C pages identified read-only, a 1-byte-read bus census and the I2C-HID descriptor at a guess-table address, Zenbook only, each controller restored as found. Owed: ZENBOOK-TOUCHPAD-REPORTS in flight (ZENBOOKTOUCHPADREPORTS, Z18: SET_POWER + RESET, the report descriptor, the Input Mode feature, the Precision Touchpad's first finger polled from the HID pass into `pal::push_pointer_report`; owed DWI2C-GPIO, DWI2C-TPGEST, DWI2C-PALM, DWI2C-TPGAIN, DWI2C-MOUSEMODE), DWI2C-ACPI (the `_CRS` base and the touchpad's address and `_DSM` register: AML or the first boot's dump), DWI2C-FAST, DWI2C-AOAC, the Fn keys.
10. **ZENBOOK-ACP** — audio: the ALC294 codec on the Ryzen HD Audio controller (1022:15e3; Linux quirk 1043:1df3
    `ALC294_FIXUP_BASS_SPEAKER_15`) with its CS35L41 amplifiers, and the ACP 7.x coprocessor (1022:15e2). Until this
    lands `hda::probe` answers `[hda] absent` on the Zenbook (Z1's gate).
11. **ZENBOOK-USB4** — the USB4 host routers (NHI 151c/151d) and the tunnelled DP / PCIe / USB behind them.
12. **ZENBOOK-RADEON** — display beyond GOP: the amdgpu DCN 3.5 scanout on the Radeon 890M (1002:150e, gfx1150) — the
    Kepler-sized job. FBWCLEAF (measured by GOPIGNITE Z10, not built): `set_framebuffer_wc` (`unaos/crates/kernel/src/arch/x86_64/memory.rs:3735`)
    retypes whole firmware identity-map leaves; the APU's GOP fb is the iGPU BAR aperture (MMIO, not RAM), so a 1 GiB leaf
    WC-types the neighbouring MMIO it shares (ECAM, IOAPIC, HPET, LAPIC, the iGPU register BAR), `leaf_is_fb_wc` then keeps
    `map_mmio_window` from re-typing it UC, and the WXN-M2 splitter's named assert fires if that leaf holds kernel text; the
    first boot's `:: x86 fb-wc: retyped N leaf(s) … over lo..hi ::` names the level (a 1 GiB hull = live; the fix splits the
    fb's leaf to 2 MiB / 4 KiB before the retype).
13. **ZENBOOK-SUSPEND** — S0ix / modern standby (the rMBP's `idle` state has no meaning here until the EC row lands).
14. **ZENBOOK-LOADERTRACE** — closed by ZENBOOK-IMAGELINE (no IVB on the line), Z6: the traces (`read_igpu_trace`,
    `read_gmux_trace`; the call at `unaos/crates/bootloader/src/main.rs:448`, the fns at :1389/:1438) are all
    `cfg(feature = "unaos_ivb")`, which the builder passes only under UNAOS_IVB (`unaos/builder/src/main.rs:1235`); the
    Zenbook line carries no IVB and `esp-zenbook` unsets an inherited one. An rMBP-line image booted on the Zenbook
    still runs them (the SMBIOS-gated loader variant stays unbuilt).
15. **ZENBOOK-BPROW** — In flight (ZENBOOKLANETYPED, Z16): `docs/dev/blueprint/boot.md` §1 gains the boards table (loader,
    image line and media verb, oldest, platform token, the QEMU lane of the line, the first-boot doc), one row per board.
16. **ZENBOOK-LEDGERGATE** — `unaos/scripts/ledger-check.sh` knows the owners {orin, pi, rmbp, shared-gate} and the
    queue files {QUEUE, rmbp, orin, pi}; teach it `zenbook` and this file (Z1's row carries `rmbp` meanwhile).
17. **ZENBOOK-MODEL** — Peter's to settle: the arc calls the machine the "Zenbook S 14 OLED"; ASUS's numbering makes
    UM5606WA the Zenbook S **16** (Linux `alc269.c`: "ASUS UM5606WA"; the S 14 is the Intel UX5406). The platform
    match keys on `UM5606` in the SMBIOS product, so the code does not depend on the answer.
