# PI-GENET — landing report (hw-pi4)

**Arc:** PI-GENET (the Pi 4's on-board Gigabit Ethernet — Broadcom GENET v5 — driver + smoltcp bind;
the Pi's FIRST network path). Branch `hw-pi4`.
**Shape:** code-complete-prior-to-metal, like ORIN-NET-4. QEMU `raspi4b` (11.0.1) models **no** GENET,
so correctness comes from `arroyo check`, the kernel8 build, faithful adherence to the Linux `bcmgenet`
v5 programming model + the shared metal-proven `net_phy` seam, and the attended Pi sitting. The QEMU
half of the gate is DTB-gated graceful skip + zero-regression only. Positive metal verification (link →
MAC → DHCP → ping) is the next attended Pi sitting (`unaos/scripts/pi-genet-bench.md`).

## The load-bearing M1 finding — QEMU 11.0.1 raspi4b does NOT model GENET

Settled empirically on the bench, not assumed. Two facts, both on the serial transcript:

1. QEMU raspi4b hands `-kernel` boots **no usable DTB** (`x0=0x100`, size 0) — there is no
   `ethernet@7d580000` node to resolve.
2. A read of the GENET register window at ARM-physical `0xFD58_0000` raises a **synchronous external
   Data Abort** (`ESR=0x96000010`, EC=0x25, DFSC=0x10, `FAR=0xfd580000`) — the unmodeled address
   decodes to *nothing* and the fabric returns an abort, **not** open-bus `0xffffffff` poison.

Fact (2) is the design pivot: the standard "poison-honest probe read before the first write" (the
PI-V3D-1 / FAULT-AT-M1 law) assumes an absent decode *returns* poison. On the BCM2711 fabric it
**faults** instead. My first implementation probed a documented-fallback base blind and **faulted the
BSP** at `0xFD58_0000` (caught in-arc on the first kernel8-test run; the APs survived, CAPSTONE 6/6, but
the BSP was dead). The fix-forward, landed in the same arc: **DTB-gate the classification before any
MMIO**, exactly as **PI-USB** does (`dtb_has_pcie` — QEMU raspi4b models no PCIe RC either, and touching
`RC_BASE` blind would fault the same way). M1 now resolves the GENET node from the live firmware DTB and
touches the register window **only** if the DTB describes one; the `SYS_REV_CTRL` poison read then guards
a link-down/absent decode on *real* metal. On QEMU (no DTB node) the driver records an honest
compiled-present line and returns before any MMIO. Re-run: no exception, BSP reaches the shell, 0 FAIL.

**Answer to the brief's load-bearing question:** QEMU raspi4b does **not** model GENET; the witness is
**code-complete-prior-to-metal (graceful skip)**, not QEMU-live. Positive datapath is attended metal.

## MAC source

DTB `local-mac-address` of the GENET node (RPi firmware fills it) preferred; fallback = UMAC `MAC0`/`MAC1`
register readback (firmware programs them at boot). The boot log states which was used. On QEMU neither is
reached (skip before MMIO); on metal the DTB property is the expected source.

## Datapath model (Linux `bcmgenet` v5, faithfully mirrored)

Producer/consumer-index ring on the default descriptor ring 16 (`DESC_INDEX`), RX + TX, 32 descriptors
each — **not** the RTL8168 per-descriptor OWN handoff. Bring-up order = `bcmgenet_open`/`init_umac`/
`init_dma`: SYS port mode → UMAC soft reset + RBUF/TBUF flush → MAC + max-frame-len → MIB reset → RBUF
64B/align → RGMII OOB → RX/TX rings → `UMAC_CMD` TX/RX at gigabit + promiscuous bring-up. Interrupts
masked (polled). Sub-block + per-ring register offsets are the bcmgenet.h v5 tables (`GENET_*_OFF`,
`genet_dma_ring_regs` v4/v5, `TOTAL_DESC=256`, `DMA_RING_SIZE=0x40`, ring/global split after the 256
descriptors). The register window lands in the already-mapped `0xC000_0000..0xFFFF_FFFF` Device GiB
(`boot::build_l1` L1[3]), so no new page-table write is needed — cleaner than piusb's outbound window /
NET-4's iATU. Every register write announced on serial before issue.

## DMA / coherency threat note (carried forward)

Rings + buffers are heap-allocated, identity-mapped (pointer == DMA physical address, the e1000 / NET-4 /
VNET invariant), published with `dsb sy`. **No IOMMU** in the Pi 4 path: once RX/TX are enabled the MAC
DMAs against whatever physical addresses the rings hold — safety is by driver discipline (only its own
identity-mapped ring/buffer allocations are targets; length fields are clamped so a misbehaving MAC
cannot force an out-of-bounds slice). The BCM2711 GENET is I/O-coherent toward DRAM; if attended metal
shows stale descriptors on a live link (rings never advance, torn/zero frames), the fix is
clean-before-own / invalidate-before-read on rings + buffers — **not** a weakening of the index protocol.

## Lane / additive-use notes

New module `arch/aarch64/genet.rs` + the `genet` cargo feature + one gated call site
(`main.rs`, post-heap on the BSP beside `piusb::enumerate`) + arroyo knob wiring + the named docs.
Additive wiring to shared files (all inert when the knob is off, verified by byte-identity):
`mod.rs` (module decl + `fdt_tegra` gate extended to include `genet`), `lib.rs` + `net_phy.rs` (the
`#[cfg(any(... ))]` net-feature gate gains `genet` — PI-GENET is the third `net_phy` rider after vnet
and net4; **no** `net_phy` internals touched, additive use only). `piusb`/`v3d`/`emmc2`/`sched` untouched.

## Gate results (verbatim)

- `./arroyo check` (knob-off): x86_64 OK, aarch64 OK.
- `UNAOS_GENET=1 ./arroyo check` (knob-on): x86_64 OK, aarch64 OK (witness half only — `genet` does not
  imply `baremetal`, so check compiles the honest non-MMIO stub; the metal half is `all(baremetal,
  aarch64)`-gated and is compiled by the kernel8 build).
- `./arroyo kernel8-test` (knob-off): 0 FAIL. GENET module + call site + smoltcp dep vanish. CAPSTONE 6/6.
  **Byte-identity to baseline:** text/data/bss sizes identical (683112 / 27952 / 610248), 1441 symbols
  both; symbol tables identical after stripping the build-path-dependent `.llvm.<hash>` suffixes on local
  symbols (the residual raw delta is the differing absolute build path — the PI-USB-2 build-path-metadata
  precedent). Baseline built from a detached `HEAD` worktree.
- `UNAOS_GENET=1 ./arroyo kernel8-test` (knob-on): 0 FAIL. Honest DTB-gated skip
  (`no GENET node in the DTB ... SKIPPED before any MMIO`), **no** `AARCH64 EXCEPTION`, BSP reaches the
  shell, CAPSTONE 6/6.
- `./arroyo test-arm 22`: unregressed (complete, 0 FAIL).
- `UNAOS_GICV3=1 ./arroyo test-arm 40`: unregressed (3/3 secondaries online GICv3, CAPSTONE 6/6, 0 FAIL).
- `./arroyo test 22`: unregressed (complete, 0 FAIL; x86 net selftest gateway ICMP reply OK).

## Flagged

- **QEMU-model finding is the deliverable** (above): raspi4b does not model GENET; unmodeled MMIO
  **faults** (external abort) rather than returning poison — hence the DTB-gate-before-MMIO design. Any
  future Pi MMIO arc should assume fault-on-absent, not poison-on-absent, and gate on the DTB first
  (the piusb/genet discipline).
- Positive link/DHCP/ping is **attended metal only** — never exercised in QEMU. Runbook:
  `unaos/scripts/pi-genet-bench.md`.
- The SoC bus→CPU translation uses the documented BCM2711 `+0x8000_0000` peripheral-ranges offset
  applied to the **DTB-resolved** child base (base is not hardcoded; the translation constant is the
  same one piusb applies to the RC). A full `/soc` `ranges` walk was judged unnecessary given the fixed,
  in-tree-precedented offset; flagged in case a future board revision needs the general walk.
