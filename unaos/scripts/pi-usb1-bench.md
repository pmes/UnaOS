# pi-usb1-bench.md — PI-USB attended Pi 4 sitting (BCM2711 PCIe RC + VL805 xHCI + device enumeration)

The **positive** verification of the BCM2711 PCIe root-complex + VIA VL805 xHCI bring-up **and** (rung 2)
the DMA-side device enumeration. QEMU `raspi4b` models **no PCIe root complex**, so everything past the
DTB `pcie@` census is exercised only here, on real Pi 4 silicon. This card is for an **attended** sitting
(LC drives, Peter physical). It is **not** part of the QEMU DONE gate — that gate is DTB-census
graceful-skip + zero-regression only.

Two arcs share this card:
- **PI-USB-1** — RC link-up → VL805 found → BAR sized → xHCI decoding → ports powered (the *honesty line*).
- **PI-USB-2** — rings/interrupter + RS=1 → port-connect pump → `ADDRESS_DEVICE` → per-device identity
  lines → keyboard ARMED (if a HID keyboard is plugged). This is the `piusb::enumerate()` half, post-heap.

## Build & stage

```
# From the hw-pi4 worktree's unaos/ dir. Unmount UNAOS before kernel8 builds.
UNAOS_PIUSB=1 UNAOS_PI=1 ./arroyo kernel8
```

Then stage the flashable image to `~/unaos-bench/flash/pi4/` (stamp + sha256 + MANIFEST) — never flash
a `target/` path directly (unaos-flash-staging rule). Bench card = the **32 GB** card (the 16 GB UNAOS
card is retired). Rebuild any usbdebug ESP LAST if you also touch x86. **Plug a USB keyboard** (and,
optionally, a USB mass-storage stick) into a Pi 4 USB-A port before boot to exercise rung 2's enumeration.

**Timing note (two call sites).**
- The rung-1 **honesty line** (`piusb::bringup`) is triggered from `boot::build_boot_info` (with the DTB
  in hand), which runs in `__rust_boot` — *before* `kernel_main` installs the EL1 exception vectors. This
  is QEMU-safe because the **census-before-touch** gate reads only the DTB (RAM) and returns before any RC
  MMIO when no `pcie@` node exists (QEMU). On metal the Pi firmware DTB has `pcie@7d500000`, so the RC MMIO
  runs — every access is to the live BCM2711 RC / VL805 config / mapped outbound window (no unbacked
  address) and all waits are finite backstops, so a fault is not expected.
- The rung-2 **enumeration** (`piusb::enumerate`) is triggered post-heap on the BSP in `kernel_main` (near
  the `emmc2::probe` seam), because the xHCI rings/DCBAA/interrupter need the live heap. It reads the
  rung-1 handoff (decoding xHCI base + ready flag); if the honesty line was not reached it prints one skip
  line and returns.

If the sitting sees an `AARCH64 EXCEPTION` during the `PIUSB` lines, capture ESR/ELR/FAR.

## What to look for on serial (the metal chain)

Grep the serial log with `awk '/PIUSB/'` (control bytes break `grep`). The expected metal chain:

```
# ── rung 1: the honesty line (pre-heap) ──
:: PIUSB: PI-USB-1 bring-up starting (BCM2711 PCIe RC @ 0xfd500000 + VL805 xHCI) ::
:: PIUSB: DTB census: `pcie@` controller present — proceeding to RC bring-up ::
:: PIUSB: M1: RC alive (RGR1_SW_INIT_1 = 0x…) — bridge reset sequence ::
:: PIUSB:   >>> WRITE: RGR1_SW_INIT_1 |= INIT_GENERIC|PERST … assert reset ::
:: PIUSB:   >>> WRITE: HARD_DEBUG &= ~SERDES_IDDQ … power up serdes ::
:: PIUSB:   >>> WRITE: RC_BAR2 inbound window = RAM@0 size=4GiB (DMA) ::      (size code 0x11 = encode_ibar_size(4 GiB) = 32-15)
:: PIUSB:   >>> WRITE: outbound MEM WIN0 CPU 0x600000000 -> PCIe [0xc0000000, 0xffffffff] (1 GiB) ::
:: PIUSB:   >>> WRITE: RGR1_SW_INIT_1 &= ~PERST … release link, training ::
:: PIUSB: M1: LINK UP (PCIE_STATUS = 0x…) ::
:: PIUSB:   root port: vendor=0x14e4 device=0x… ::                (Broadcom RC)
:: PIUSB: M2: VL805 config[0x00] = 0x3483_1106 ::
:: PIUSB:   VL805 FOUND: vendor=0x1106 device=0x3483 (VIA VL805 xHCI) ::
:: PIUSB:   class=0x0c subclass=0x03 progif=0x30 … (USB xHCI) ::
:: PIUSB:   BAR0 = MMIO mem, 64bit=…, size=0x… ::
:: PIUSB:   >>> WRITE: BAR0 := 0xc0000000 (PCIe-side; CPU sees it at 0x600000000) ::
:: PIUSB:   NOTIFY_XHCI_RESET (mailbox 0x00030058, dev_addr=0x100000) — firmware VL805 reset/load ::
:: PIUSB: M3: mapped outbound window CPU 0x600000000 … reading VL805 xHCI caps ::
:: PIUSB:   xHCI DECODING: CAPLENGTH=… HCIVERSION=0x0… HCSPARAMS1=0x… MaxPorts=… ::
:: PIUSB: M3: … root port(s) powered (PORTSC.PP set); controller halted-but-decoding — HONESTY LINE reached ::
:: PIUSB:   NEXT (post-heap, this arc): `piusb::enumerate()` programs rings + interrupter, RS=1, … ::

# ── rung 2: DMA-side enumeration (post-heap) ──
:: PIUSB: enumerate: DMA-side bring-up @ 0x600000000 (CAPLENGTH=…) — OUR controller, fresh reset + RS=1 ::
:: PIUSB:   programming interrupter + rings (runtime regs), then RS=1 ::
:: PIUSB: enumerate: keyboard ARMED (slot …, root port …) -> PASS ::        (if a HID keyboard is plugged)
:: PIUSB: enumerate: mass storage ready (slot …) ::                          (if a USB stick is plugged)
:: PIUSB: enumerate: device topology after the polled walk: ::
:: PIUSB:   … (port_slot_summary + usb_summary identity lines) …
:: PIUSB: enumerate: DONE — keyboard armed (slot …, port …); enumeration + HID arming complete ::
```

The rung-2 DONE point is **keyboard ARMED** (or an honest "no keyboard armed within the bounded window"
with the topology dumped). If the link comes up but the VL805 is not found, or the xHCI CAP reads poison,
or enumeration stalls, capture the full `PIUSB` block: the register values + the `port_slot_summary`
topology discriminate the failing stage (window/BAR mismatch vs firmware-not-loaded vs link-partner vs a
device that never completes `ADDRESS_DEVICE`). The enumeration pump is bounded (~30 s worst case); a
device-less boot pays it once and prints the honest "no keyboard" verdict.

## Not part of the QEMU gate

QEMU raspi4b prints only the two graceful-skip lines (no PCIe RC / VL805 modeled):

```
:: PIUSB: PI-USB-1 bring-up starting (BCM2711 PCIe RC @ 0xfd500000 + VL805 xHCI) ::
:: PIUSB: no `pcie@` node in the firmware DTB (@0x…) — no BCM2711 PCIe RC (expected in QEMU raspi4b; models no RC) — USB bring-up skipped, graceful degradation ::
:: PIUSB: enumerate: honesty line not reached this boot (no VL805 xHCI decoding — expected in QEMU raspi4b: models no PCIe RC/VL805) — DMA-side enumeration skipped, graceful degradation ::
```

and the boot continues to the normal test suite with 0 FAIL (the zero-regression half of the gate).
