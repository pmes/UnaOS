# pi-genet-bench.md — PI-GENET attended Pi 4 sitting (BCM GENET v5 Gigabit Ethernet)

The **positive** verification of the Pi's first network path: the on-board Gigabit Ethernet (Broadcom
GENET v5) driver + smoltcp bind. QEMU `raspi4b` (11.0.1) does **not** model the GENET block — it raises
a synchronous external Data Abort on the register window (`ESR=0x96000010`, `FAR=0xfd580000`) and hands
`-kernel` boots no usable DTB — so everything past the DTB-gated classification is exercised only here,
on real Pi 4 silicon. This card is for an **attended** sitting (LC drives, Peter physical). It is **not**
part of the QEMU DONE gate — that gate is graceful-degradation + zero-regression only.

## Build & stage

```
# From the hw-pi4 worktree's unaos/ dir. Unmount UNAOS before kernel8 builds.
UNAOS_GENET=1 UNAOS_PI=1 ./arroyo kernel8
```

Then stage the flashable image to `~/unaos-bench/flash/pi4/` (stamp + sha256 + MANIFEST) — never flash a
`target/` path directly (unaos-flash-staging rule). Bench card = the **32 GB** card (the 16 GB UNAOS card
is retired). Rebuild any usbdebug ESP LAST if you also touch x86.

## Bench setup

- Plug the Pi's RJ45 into the bench LAN (a switch/router with a **DHCP server** — the driver runs a
  DHCPv4 client first, 5 s bounded timeout, then falls back to the static `192.168.1.2/24 gw .1`).
- Serial console attached (the whole bring-up greps as `:: PI-GENET:`).
- Note the link partner (a plain switch is fine; the witness pings the DHCP-leased gateway).

## Expected chain (real metal)

The firmware hands a full DTB, so M1 resolves the GENET node and proceeds (QEMU never reaches this):

```
:: PI-GENET: BCM GENET v5 GbE bring-up (DTB @<x0> size=<sz>) ::
::   DTB GENET node reg child base 0x7d580000 -> ARM-physical 0xfd580000 (SoC ranges +0x80000000 ...) ::
::   SYS_REV_CTRL = 0x.......6 — LIVE GENET v5 (rev minor .., EPHY 0x....); this build MODELS the block ::
::   station MAC = <mac> (source: dtb local-mac-address) ::      # or "umac-reg readback"
::   M2 bring-up (GENET v5 programming order; polled, interrupts masked) ::
::   ... RBUF/TBUF flush, UMAC reset, MAC0/MAC1, RBUF, RGMII OOB, RDMA/TDMA ring 16 armed ...
::   rings up: RX/TX ring 16 (32 desc each); UMAC_CMD readback 0x........ (live) ::
::   MDIO PHY @ addr 1 (PHYID1=0x....) ::
::   external PHY (MDIO addr 1) link UP ::
::   GENET registered; RX/TX rings live ::
::   NET: DHCP discover (timeout 5000 ms) :: ... :: NET: DHCP lease ip=.../.. gw=... => PASS ::
:: PI-GENET ping <gw> (4/4 sent, N/4 replies) [dhcp] link UP => PASS ::
:: PI-GENET DONE — GENET v5 driver up + smoltcp bound ::
```

The metal chain of record: **link autoneg → MAC → DHCP lease on the bench LAN → ping.**

## What to capture / watch

- The `SYS_REV_CTRL` value + decoded version (`v5` expected). A `POISON` line there means the register
  window is not answering (link-down/absent decode) — record it, do not force writes.
- The **MAC source** line (DTB vs UMAC-register readback) and the MAC value.
- Link state (`UP`/`DOWN`). `DOWN (autoneg pending / no cable)` is the honest pre-cable state.
- The DHCP outcome (`[dhcp]` lease vs `[static]` fallback) and the ping PASS/SKIP line.
- **If an `AARCH64 EXCEPTION` appears during the PI-GENET lines:** capture `ESR`/`ELR`/`FAR`. The
  bring-up runs post-heap on the BSP with vectors live, and every access is to the DTB-resolved live
  GENET window or the identity-mapped rings, so a fault is not expected on metal — but the register
  block is I/O-coherent DRAM DMA, so a fault or stale-descriptor symptom (rings never advance,
  torn/zero frames on a live link) points at the coherency assumption; the fix is clean-before-own /
  invalidate-before-read on rings + buffers (see arch_arm64.md §PI-GENET), never a weakening of the
  producer/consumer-index protocol.

## Fold-back

Fold the sitting result into `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` §PI-GENET (modeling finding is
already recorded; add the metal link/DHCP/ping verdict) and MILESTONES. The QEMU DONE gate
(graceful DTB-gated skip, 0 FAIL, byte-identity) is already green; this sitting is the positive-path
proof only.
