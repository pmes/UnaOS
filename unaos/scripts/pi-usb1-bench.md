# pi-usb1-bench.md — PI-USB-1 attended Pi 4 sitting (BCM2711 PCIe RC + VL805 xHCI)

The **positive** verification of the BCM2711 PCIe root-complex + VIA VL805 xHCI bring-up. QEMU
`raspi4b` models **no PCIe root complex**, so everything past the DTB `pcie@` census is exercised only
here, on real Pi 4 silicon. This card is for an **attended** sitting (LC drives, Peter physical). It is
**not** part of the QEMU DONE gate — that gate is DTB-census graceful-skip + zero-regression only.

## Build & stage

```
# From the hw-pi4 worktree's unaos/ dir. Unmount UNAOS before kernel8 builds.
UNAOS_PIUSB=1 UNAOS_PI=1 ./arroyo kernel8
```

Then stage the flashable image to `~/unaos-bench/flash/pi4/` (stamp + sha256 + MANIFEST) — never flash
a `target/` path directly (unaos-flash-staging rule). Bench card = the **32 GB** card (the 16 GB UNAOS
card is retired). Rebuild any usbdebug ESP LAST if you also touch x86.

**Timing note (call site).** The USB bring-up is triggered from `boot::build_boot_info` (with the DTB in
hand), which runs in `__rust_boot` — *before* `kernel_main` installs the EL1 exception vectors. This is
QEMU-safe because the **census-before-touch** gate reads only the DTB (RAM) and returns before any RC
MMIO when no `pcie@` node exists (QEMU). On metal the Pi firmware DTB has `pcie@7d500000`, so the RC
MMIO runs — every access is to the live BCM2711 RC / VL805 config / mapped outbound window (no unbacked
address) and all waits are finite backstops, so a fault is not expected. If the sitting sees an
`AARCH64 EXCEPTION` during the `PIUSB` lines, capture ESR/ELR/FAR: it means an RC/VL805 access faulted
before vectors were live.

## What to look for on serial (the metal chain)

Grep the serial log with `awk '/PIUSB/'` (control bytes break `grep`). The expected metal chain:

```
:: PIUSB: PI-USB-1 bring-up starting (BCM2711 PCIe RC @ 0xfd500000 + VL805 xHCI) ::
:: PIUSB: DTB census: `pcie@` controller present — proceeding to RC bring-up ::
:: PIUSB: M1: RC alive (RGR1_SW_INIT_1 = 0x…) — bridge reset sequence ::
:: PIUSB:   >>> WRITE: RGR1_SW_INIT_1 |= INIT_GENERIC|PERST … assert reset ::
:: PIUSB:   >>> WRITE: HARD_DEBUG &= ~SERDES_IDDQ … power up serdes ::
:: PIUSB:   >>> WRITE: RC_BAR2 inbound window = RAM@0 size=4GiB (DMA) ::
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
```

The **honesty line** is the arc's DONE point. Device enumeration (rings + interrupter — needs the heap,
so it runs after the boot heap is live — port-connect pump, `ADDRESS_DEVICE`, HID/storage) is the
follow-on arc. If the link comes up but the VL805 is not found, or the xHCI CAP reads poison, capture
the full `PIUSB` block: the register values discriminate the failing stage (window/BAR mismatch vs
firmware-not-loaded vs link-partner).

## Not part of the QEMU gate

QEMU raspi4b prints only:

```
:: PIUSB: PI-USB-1 bring-up starting (BCM2711 PCIe RC @ 0xfd500000 + VL805 xHCI) ::
:: PIUSB: no `pcie@` node in the firmware DTB (@0x…) — no BCM2711 PCIe RC (expected in QEMU raspi4b; models no RC) — USB bring-up skipped, graceful degradation ::
```

and the boot continues to the normal test suite with 0 FAIL (the zero-regression half of the gate).
