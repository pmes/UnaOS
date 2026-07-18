# PI-USB-2 — landing report (hw-pi4)

**Arc:** PI-USB-2 (from the rung-1 honesty line to VL805 device enumeration — xHCI DMA-side init +
polled enumeration). Branch `hw-pi4`.
**Shape:** code-complete-**by-construction** prior to metal — QEMU `raspi4b` models **no** PCIe RC/VL805,
so (like PI-USB-1) correctness comes from the BCM2711/xHCI programming model + the shared, metal-proven
`drivers/xhci` driver + poison-honest reads, and the QEMU half of the gate is DTB/handoff census
graceful-skip + zero-regression only. Positive metal verification is the next attended Pi sitting
(`unaos/scripts/pi-usb1-bench.md`, now the rung-2 runbook).

## M1 — the mandatory gate: `encode_ibar_size` adjudication (RESOLVED to 0x11)

The rung-1 lens flagged the `RC_BAR2` inbound-window size code as **0x11-vs-0x20 unresolved** (a wrong
inbound BAR + bus-master is how DMA lands in the wrong place). Resolved against the Linux programming
model. `drivers/pci/controller/pcie-brcmstb.c`'s `brcm_pcie_encode_ibar_size(u64 size)` maps a **byte
size** to the 5-bit size field (`PCIE_MISC_RC_BAR2_CONFIG_LO_SIZE_MASK = 0x1f`, bits `[4:0]` of
`CONFIG_LO`) by branch on `ilog2(size)`:

```
log2 in [12,15]  (4 KiB .. 32 KiB)   -> (log2 - 12) + 0x1c
log2 in [16,35]  (64 KiB .. 32 GiB)  -> log2 - 15
otherwise                             -> 0 (disabled)
```

A 4 GiB window is `size = 2^32`, so `log2 = 32`, which lands in the **[16,35]** branch:
`code = 32 - 15 = 17 = 0x11`.

**Verdict: `0x11` is CORRECT.** The alternative `0x20` (= 32 decimal, the raw `log2`) is not what the
field encodes and is **WRONG**. The rung-1 constant `RC_BAR2_SIZE_4G = 0x11` is right and is **unchanged**;
the source-derived rule is now stated verbatim in the `piusb.rs` step (e) comment and in
`arch_arm64.md §PI-USB-2`. (Full `CONFIG_LO` = `(RAM base low 32b) | 0x11`; with base 0 that is `0x11`,
`CONFIG_HI = 0`.) This gate passed, so the arc proceeded.

## The whole-RAM inbound window — DMA threat note (carried forward)

`RC_BAR2` maps the PCIe inbound window to **system RAM base 0, 4 GiB** — the entire space a bus-master
device can DMA into, with **no IOMMU** in the Pi 4 path. Once bus-master is on (rung-1 M2) and the
controller runs (rung-2), the VL805 and anything behind its USB ports can read/write any physical RAM the
rings point it at. The safety here is by driver discipline (only its own heap-allocated, identity-mapped
ring/DCBAA/buffer structures are DMA targets; every liveness read poison-rejects) — there is **no hardware
containment**. Hardening item for a later rung: a least-privilege / IOMMU-backed inbound window.

## What landed

**`arch/aarch64/piusb.rs`** (the rung-1 module, extended — still `#[cfg(all(baremetal, piusb))]`):
- **M1 comment** at step (e): the `encode_ibar_size` derivation verbatim (above). Constant unchanged.
- **M2 PORTSC PED-mask robustness nit** in the port-power RMW: mask **PED** (bit 1, RW1CS — write-1
  *disables* the port) off alongside the RW1C change bits, so "power on" (`PP` set) disturbs nothing on a
  warm/already-enabled port. Hardware sets PED itself on a successful reset; we never assert it.
- **Handoff statics** `XHCI_CPU_BASE` + `XHCI_READY`: the honesty line (`bringup`, pre-heap) stashes the
  decoding CPU-side xHCI base + a ready flag; the DMA-side entry reads them across the pre-heap→post-heap
  gap. Single-writer (BSP, pre-SMP) so plain atomics.
- **`pub fn enumerate()`** — the DMA-side bring-up + polled enumeration, called post-heap. Reuses the
  shared driver's JB2b polled-attach machinery **verbatim** (`xhci::init` = halt+HCRST+CNR — OUR fresh
  controller, the plain reset path, **not** the Orin inherited no-HCRST/CRCR takeover — then
  `XhciController::new` + seat `COMMAND_RING`/`EVENT_RING`/`ERST_TABLE` + `init_interrupter` /
  `init_pointers` / `start()` = RS=1). Bounded pump (`poll_events` + `service_hubs` + `service_hid_setproto`
  + `service_slot_disposal` + `service_enum` + `service_storage`), exit at keyboard-ARMED (+ short storage
  settle) or a ~30 s backstop; per-device identity lines (`port_slot_summary` + `usb_summary`). A local
  `keyboard_armed` predicate mirrors the JB2b one. Graceful skip when the honesty line was not reached
  (`XHCI_READY` false — QEMU census-skip / link-down / BAR mismatch).

**`main.rs`:** the piusb-gated `enumerate()` call at the post-heap BSP seam (near `emmc2::probe`).

**Docs:** `arch_arm64.md` new `§PI-USB-2` (encoding adjudication verbatim, DMA threat note, PED nit, the
DMA-side chain, byte-identity reality, expected metal chain); `scripts/pi-usb1-bench.md` refreshed into the
rung-2 runbook (both call sites, the enumeration serial chain, the two QEMU skip lines).

## xhci-core seams flagged

**NONE.** Zero `drivers/xhci` core edits — `enumerate()` uses only existing public entry points
(`init`, `XhciController::new`, `COMMAND_RING`/`EVENT_RING`/`ERST_TABLE` statics, `init_interrupter`,
`init_pointers`, `start`, the `service_*` pumps, `poll_events`, `port_slot_summary`, `usb_summary`, and the
`storage_slot`/`storage_note`/`slots` public fields) — the brief's preferred zero-xhci-core outcome. This
is the exact set `arch/aarch64/xusb_tegra.rs::jb2b_attach` already uses.

## Byte-identity — the one honest caveat

Rung 1 got **strict** byte-identity by choosing call sites (end-of-function / end-of-file) that shift no
panic-location line numbers. Rung 2's `enumerate()` **must** be called post-heap from inside `kernel_main`
(heap + BSP context live only there), and **any** insertion into `kernel_main` shifts the source lines of
every item below it. So the knob-off `kernel8.img` differs from baseline by **exactly one byte**: the
`core::panic::Location.line` u32 of an *unrelated* `assert!` in `input_service` (`1840 -> 1848`, +8 for the
8-line gated insertion). Confirmed by `cmp -l` (1 differing byte, at the little-endian `Location.line`
field `0x0730 -> 0x0738`). **All machine code and data are identical** — the delta is a single embedded
source-line number, not any code or behavior. Knob-off, `piusb::enumerate` compiles out entirely; the full
kernel8 battery is 0 FAIL. This is functional byte-identity; strict single-byte identity is not achievable
for a post-heap call site by construction, and forcing it would require deleting unrelated lines.

## Gates (verbatim results)

- `./arroyo check` — **x86_64 OK, aarch64 OK**, knob-off **and** knob-on (`UNAOS_PIUSB=1`).
- knob-off `./arroyo kernel8-test 35` — **0 FAIL** (191 lines; K3-mount `[w=0x1ff]`, K4-write, BANDY,
  F2/F3, prio-mix all PASS; CAPSTONE 6/6; 0 `AARCH64 EXCEPTION`; **0 `PIUSB` lines** — module gated out).
  kernel8.img sha256 `6a2c2c6214fd71ed163f059aa8825c650402ddcb7344ec98a2cb433c5eab92d2`; baseline (HEAD,
  before this arc) `757c93d9aeb2c37e903df37c538f4e6ce3c3f72a58291bb90dbe43fe4af8a5cf` — differ by 1 byte
  (the panic-`Location` line-number above).
- knob-on `UNAOS_PIUSB=1 ./arroyo kernel8-test 35` — **0 FAIL** (194 lines), **both** graceful census-skip
  lines present (`bringup` DTB-skip + `enumerate` honesty-line-not-reached skip — the DMA-side path
  census-skips in QEMU exactly as rung 1 does), full suite reached (K3-mount `[w=0x1ff]`, CAPSTONE 6/6).
  kernel8.img sha256 `e997279ddf201d4327f1819e393d4fedb0881a66fb4330bed4832b809351eed3`.
- `./arroyo test-arm 22` — **0 FAIL**.
- `UNAOS_GICV3=1 ./arroyo test-arm 40` — **0 FAIL**, `CAPSTONE COMPLETE — all 6 sync primitives`.
- `./arroyo test 22` (x86) — **0 FAIL** (SOCK witnesses OK).

## Flagged

- **`main.rs` touched** (the only file outside `piusb.rs` + named docs): one piusb-gated `enumerate()` call
  at the post-heap BSP seam. This is the sole in-lane way to sequence the heap-dependent DMA-side attach
  (rung 1's pre-heap `bringup` call in `boot.rs` cannot allocate rings). Gated; knob-off it compiles out
  (functional byte-identity, 1-byte panic-`Location` delta as above).
- **1-byte knob-off delta** (panic-`Location` line number) — see the byte-identity section. Functional
  byte-identity holds; strict single-byte identity is not achievable for a post-heap `kernel_main` call.
- **No `drivers/xhci` core edits** — public entry points only (see xhci-core seams above).
- **Metal accrual:** positive verification (rings/RS=1, live device enumeration, `ADDRESS_DEVICE`,
  per-device identity lines, keyboard armed) is attended-metal — plug a USB keyboard (and optionally a
  stick) before boot. Bench card `unaos/scripts/pi-usb1-bench.md`. The pi4-resume / metal-pi4 pending-note
  update is the Maestro's job (per the brief), not done here.
