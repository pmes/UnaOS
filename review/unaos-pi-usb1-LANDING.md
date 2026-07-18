# PI-USB-1 — landing report (hw-pi4)

**Arc:** PI-USB-1 (BCM2711 PCIe root complex + VIA VL805 xHCI attach). Branch `hw-pi4`.
**Shape:** code-complete-**by-construction** prior to metal — QEMU `raspi4b` models **no** PCIe RC, so
(like ORIN-NET-4 and PI-V3D-1) correctness comes from the BCM2711 programming model + the Linux
references of record + poison-honest reads, and the QEMU half of the gate is DTB-census graceful-skip +
zero-regression only. Positive metal verification is the next attended Pi sitting
(`unaos/scripts/pi-usb1-bench.md`).

## Why USB = PCIe on the Pi 4

All four Pi 4 USB-A ports hang off ONE endpoint — the VIA **VL805** xHCI (`1106:3483`) — behind the
BCM2711's single PCIe RC (`pcie@7d500000`, ARM-physical `0xFD50_0000`). So this arc is the unlock for
every Pi USB device: it brings the RC + VL805 up to a **halted-but-decoding + ports-powered honesty
line**. Full enumeration (rings/`ADDRESS_DEVICE`/HID/storage — needs the heap + a live device) is the
scoped follow-on.

## What landed

**`arch/aarch64/piusb.rs`** (new module, `#[cfg(all(feature = "baremetal", feature = "piusb"))]`):
- `dtb_has_pcie(dtb)` — a minimal, self-bounded flat-device-tree scan (header magic + `totalsize`
  bound + `FDT_BEGIN_NODE` name walk) for a `pcie@` node. **Census-before-touch**: touches only the DTB
  blob (RAM), and returns false before ANY RC MMIO when absent. This is the anti-abort gate (see below).
- `m1_rc_bringup` — brcmstb sequence (Linux `pcie-brcmstb.c`): absent-RC gate, bridge core reset +
  PERST assert/release, serdes power-up (`HARD_DEBUG.SERDES_IDDQ` clear), inbound DMA BAR (`RC_BAR2` →
  RAM@0, 4 GiB), outbound MEM window (`CPU_2_PCIE_MEM_WIN0_*`: CPU `0x6_0000_0000` ↔ PCIe `0xC000_0000`,
  1 GiB), PERST deassert, finite ~100 ms link-up poll (`PCIE_MISC_PCIE_STATUS` PHYLINKUP|DL_ACTIVE),
  root-port identity read.
- `m2_enumerate_vl805` — child config via `EXT_CFG_INDEX`/`EXT_CFG_DATA`: VL805 identity check
  (`1106:3483`, poison-rejecting), class read, **BAR0 sizing ritual** (all-ones probe + immediate
  restore — the ORIN-NET-3 pattern), BAR0 assign, MEM+bus-master enable, and the `NOTIFY_XHCI_RESET`
  mailbox.
- `m3_attach_xhci` — map the outbound window (`boot::map_device_1gib`), read `CAPLENGTH`/`HCIVERSION`/
  `HCSPARAMS1` (poison-rejecting), attach the shared `drivers/xhci` in polled mode (`xhci::init` — halt
  + HCRST + CNR wait, heap-free), set `PORTSC.PP` per root port, **stop at the honesty line**.
- Local `is_poison` / `live_vendor_device` rejecting BOTH `0xffffffff` and `0xdeadbeef` (the PI-V3D-1
  false-pass rule); every wait is a finite CNTPCT backstop.

**`arch/aarch64/mailbox.rs`:** `notify_xhci_reset(dev_addr)` + tag `NOTIFY_XHCI_RESET = 0x00030058`
(RPi firmware VL805 reset/firmware-load; `dev_addr = 0x0010_0000` = VL805 bus1:dev0:fn0). `piusb`-gated.

**`arch/aarch64/boot.rs`:** `map_device_1gib(pa)` (end-of-file, `piusb`-gated) — installs one L1
Device-nGnRnE block for an MMIO window outside `build_l1`'s fixed 0–4 GiB map (the outbound window at
`0x6_0000_0000`, reachable under IPS=36-bit / VA=39-bit), with the canonical set-descriptor maintenance.
The `piusb::bringup(dtb)` call site is at the **end of `build_boot_info`** (has the DTB; only the gated
helper follows it).

**`arch/aarch64/mod.rs`:** `pub mod piusb` (gated). **`Cargo.toml`:** `piusb = ["baremetal"]`.
**`arroyo`:** `UNAOS_PIUSB=1` → `,piusb`. **Docs:** `docs/.../arch_arm64.md` new §PI-USB;
`unaos/scripts/pi-usb1-bench.md` (attended-metal bench card).

## The anti-abort design decision (the one real hazard found + fixed)

`build_boot_info` runs in `__rust_boot`, **before** `kernel_main` installs the EL1 exception vectors.
The RC aperture (`0xFD50_0000`) is in the `boot.rs` L1[3] Device window, so an **absent** read there is
an external abort — and with no vectors that abort kills the boot. (The V3D probe survives the same
pre-vector timing only because its `0xFEC0_0000` is a QEMU-*modeled* container returning `0`; the RC
region is unmodeled and aborts.) First knob-on kernel8-test proved this: with a bare RC read the boot
died after the first `PIUSB` line and never reached the suite. Fix = **census-before-touch**: read the
DTB for a `pcie@` node first and skip before any RC MMIO. QEMU raspi4b's DTB has no such node → clean
skip; the Pi firmware DTB has `pcie@7d500000` → proceed. Post-fix knob-on reaches the full suite, 0 FAIL.

## Gates (verbatim results)

- `./arroyo check` — **x86_64 OK, aarch64 OK** (both arches).
- knob-off `UNAOS_PI=1 ./arroyo kernel8-test` — **0 FAIL**; `kernel8.img` **byte-identical to baseline**
  (verified against a fresh HEAD build in a throwaway worktree):
  `aac1963302de63dd604028532ef48d7e092c6d7b32a8929707f47adcdce7ee8c` (both).
- knob-on `UNAOS_PIUSB=1 UNAOS_PI=1 ./arroyo kernel8-test` — **0 FAIL**, DTB-census graceful skip,
  full suite reached (K3-mount/K4-write present). knob-on `kernel8.img` sha256
  `6790e39685350e10dc4e28afd1c93157606eb38ae70a343f8df05cc4cb20e9e1`.
- `./arroyo test-arm 22` — **0 FAIL**.
- `UNAOS_GICV3=1 ./arroyo test-arm 40` — **0 FAIL**, `CAPSTONE COMPLETE — all 6 sync primitives`.
- `./arroyo test 22` (x86) — **0 FAIL** (SOCK-2..4 PASS).

## Flagged

- **`arch/aarch64/boot.rs` touched** (beyond `mailbox.rs`/new module): a `piusb`-gated `map_device_1gib`
  helper at end-of-file + the gated `bringup(dtb)` call at the end of `build_boot_info`. boot.rs is a
  Pi-only (`baremetal`-gated) aarch64 arch file — squarely "pi pcie/usb bring-up" — and both additions
  are cfg-gated (knob-off byte-identity proven). Flagged because boot.rs owns the L1 the integrator may
  consider core; there is no other in-lane way to map an MMIO window outside `build_l1`'s fixed map.
- **No `drivers/xhci` core edits** — the attach uses the existing `xhci::init` public entry only (the
  brief's preferred zero-xhci-core outcome).
- **VL805 firmware** is loaded by the RPi bootloader/EEPROM at power-on; `NOTIFY_XHCI_RESET` re-issues
  it. A NOTIFY failure is non-fatal to the honesty line (logged, proceed) — the firmware is normally
  already loaded.
- **Metal accrual:** positive verification (RC link up, VL805 found, BAR sized, xHCI decoding, ports
  powered) is attended-metal — bench card `unaos/scripts/pi-usb1-bench.md`. The pi4-resume / metal-pi4
  pending-note update is the Maestro's job (per the brief), not done here.
