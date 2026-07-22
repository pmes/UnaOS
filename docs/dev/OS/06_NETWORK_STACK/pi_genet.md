# Pi 4 Ethernet — BCM2711 GENET v5 driver

The Raspberry Pi 4's on-board Gigabit Ethernet is a Broadcom **GENET v5** unimac
controller (`ethernet@7d580000`, `brcm,bcm2711-genet-v5`) driving an external
**BCM54213PE** RGMII PHY. The driver is `arch/aarch64/genet.rs`, armed by the
`genet` cargo feature (`UNAOS_GENET=1`, default OFF). It DTB-resolves the block,
brings up the MAC/PHY/DMA rings per the Linux `bcmgenet` v5 programming model,
and binds a persistent [smoltcp](../../../../unaos/docs/dev/OS/08_NET/networking.md)
interface on top.

This document records the **DMA-coherency and RX/TX datapath findings** proven on
Pi silicon during the R23s1f campaign (the first DHCP lease and first TCP service
on the Pi). The bring-up foundation — DTB resolve, the register/datapath model,
autoneg-honest link (PI-GENET-2), and the ring-wrap fix (PI-GENET-3) — is in
[`arch_arm64.md` §PI-GENET](../01_BOOT_HAL/arch_arm64.md). This doc picks up from
the first frames on the wire.

---

## 1. The load-bearing design fact — GENET buffers are cached, the DMA does not snoop

**The BCM2711 GENET DMA engine does not snoop the A72 data caches.** The RX and TX
ring buffers are allocated from Write-Back **cacheable** DRAM, so every hand-off
between the CPU and the controller must be bracketed by explicit cache
maintenance, or one side reads stale memory. This is the same non-coherent-DMA
class as the [xHCI driver](../07_USB_STORAGE/usb_xhci.md#dma-coherency-xhci-coherence)
on this SoC, and it manifests as a **coherency pair** — one op on each direction
of the ring:

| Direction | Op | When | Arc |
| --- | --- | --- | --- |
| **RX** | `dc ivac` (invalidate) | per buffer line, *before* the CPU reads the length + copies the frame | GENET-5 (`0370ebc1`) |
| **TX** | `dc cvac` (clean/write-back) | over the frame buffer, *before* publishing the descriptor | GENET-7 (`acf11670`) |

Both are applied over the identity-mapped buffer span and are no-ops on a coherent
host. The maintenance lives in `rx_frame_raw` (invalidate before length read + copy)
and `raw_tx` (clean before descriptor publish).

- **RX without the invalidate (GENET-5):** the CPU read stale pre-refill cache lines
  — real lengths off the descriptor but zero/garbage payload, so the ethertype was
  unparseable and **0 frames classified out of 151 popped** (boot-P10). The RBUF
  status-block strip was checked first and exonerated; cache staleness was the
  actual defect. Vindicated at boot-P11/P13: the same pops that read garbage now
  decode real router ARP and sane ethertypes.
- **TX without the clean (GENET-7):** the TDMA producer/consumer indices drained
  cleanly (`prod==cons`, no ring stall) yet **the frames never reached the bridge**
  — the switch's address cache never learned the Pi's MAC — because the MAC
  DMA-read stale DRAM and egressed garbage or nothing. Condemned on the bench by
  elimination (the same share dongle leased the Orin and a second Mac through the
  same wire). GENET-7 is the direct TX mirror of GENET-5 and was **the fix** that
  produced the first lease.

> The RX-clean / TX-stale split is why the Pi's no-lease symptom was NOT the Orin's:
> on the Pi, inbound frames popped and drained (RX ring alive); the gap was TX
> egress. On the Orin the frames never popped. Two different diseases in the same
> non-coherent-DMA family.

---

## 2. The rx[0] first-slot quirk — the descriptor length is authoritative

On **every boot**, the *first* popped RX frame reads an all-zero in-buffer 64-byte
status block even after `dc ivac`, while its descriptor `length_status` word carries
a valid length and the payload at `RX_STATUS_PAD` is the real frame (metal:
`rx[0] dsc=0x01987f80 len=342 et=0x0800`, the live DHCP OFFER). From `rx[1]` onward
`sb_pre == sb_post == dsc` — the status block *is* populated for later slots.

The RDMA completion writes back the descriptor `length_status` and advances the
producer index, but the 64-byte status-block write **lags or is skipped on the first
ring pass**. The design ruling (GENET-8, `565bf8c7`):

- **The descriptor `length_status` is the authoritative length source.** The in-buffer
  status block is kept only as the `[genet5]` coherency witness, never as a length.
- The GENET-6 bounded re-poll (`31f0d269`) — "status block reads 0 ⇒ DMA not yet
  visible, re-read to rescue" — was **REFUTED on metal**: 10k spins never flipped the
  block, the frame was already complete, and the spin burned the DHCP window while
  frames queued. The re-poll was removed.
- This also refutes the early-index hypothesis: a slot popped before completion would
  show `dsc_ls == 0` (init clears it), but `dsc_ls` is a valid length ⇒ the slot WAS
  completed; only the status block lagged.

---

## 3. PHY LED selectors — the LEDs are on the PHY, not the MAC

The RJ45 link/activity LEDs on the Pi 4 are driven by the external **BCM54213PE**
PHY (MDIO addr 1, `PHYID1 = 0x600d`), **not** the GENET MAC. The stuck-solid-amber
LED observed through the whole campaign was therefore *not* a MAC datapath symptom —
the `EXT_RGMII` `OOB_DISABLE` / forced `RGMII_LINK` bits were a red herring.

Left at power-on defaults, a PHY LED selector maps an LED to a solid
link/full-duplex source, so it stays lit the entire time a gigabit link is up. The
fix (GENET-8, `565bf8c7`) adds `mdio_write` plus a **BCM54xx shadow-register writer**
and programs the LED-selector shadow registers (`0x0d` / `0x0e`) to standard
link + activity sources (no tied-on selector), following `bcm-phy-lib` /
`brcmphy.h`. This is PHY-side / MDIO-only — no MAC datapath change, no protection
weakened. Metal-confirmed OUT at boot-P18 power-on.

---

## 4. Net service architecture

Once the interface is DHCP-configured, two long-lived kernel tasks keep the stack
alive and serving. Both live in `genet.rs` and register via
`crate::arch::sched::spawn` from within the driver (no shared kernel-core file is
touched).

### `net9` — the persistent poll task (PI-NET-9, `15c6626e`)

`bind_smoltcp`'s original DHCP+ping window was bounded: it returned, and the
interface stopped being polled, so the gateway's later ARP who-has / ICMP echo
requests (arriving seconds after the lease) hit a dead stack. PI-NET-9 gives the
DHCP-configured smoltcp `Interface` + `Device` a home beyond that window (a static
`NetService` behind a `spin::Mutex`, single-core-owned like `GENET_DEVICE`) and a
forever kernel task (`net9`) that polls it every **~4 ms** (1 per-core tick).
smoltcp's `iface.poll` answers ARP and ICMP echo by itself — no protocol code. Reply
counts come from a TX-seam classifier in `raw_tx` (ARP opcode 2 / ICMP type 0), so
the `[net9] answered arp=N icmp=N` witness is an emission proof, not a poll-loop
guess (rate-limited, change-only — default-quiet).

### `net10` — the first TCP service (PI-NET-10, `4055900c`)

PI-NET-10 grows the persistent `SocketSet` to hold one passive smoltcp TCP socket
(static leaked 2048/4096 ring buffers) listening on **port 80**. Each `net9` poll
takes one bounded HTTP service step (`NetService::http_step`): re-arm `listen(:80)`
from any non-open state (so the service survives repeated requests and rude clients —
smoltcp needs an explicit re-listen after RST / half-open close), drain the request
(path ignored), then on a writable TX half emit a small static `HTTP/1.0 200` page
(OS name, hw-pi4 tip SHA via `option_env!(UNAOS_GIT_SHA)` — see BUILD-SHA-1,
`2b330f57` — else the branch label, uptime, `[net9]` reply counters, served count,
configured IPv4) and close.

> **Scheduler placement note (PI-SCHED-1, `4490e7bf`).** The `net9` task's ~4 ms poll
> was observed parking on the *render* core, and all GUI (`vug`) load landing on the
> orphan-reaper core. PI-SCHED-1 adds a Pi-gated per-spawn placement witness plus a
> `core_load_report()` so every core placement is auditable on the target. This is a
> **probe only** — log-only, it cannot alter scheduling — but it flags that net-poll
> co-tenancy with the render core is a placement item worth an eventual policy arc.

### DHCP shape

The DHCP window is **15 seconds** (GENET-6, `31f0d269`). The original 5 s window
closed just before a real 342-byte OFFER landed at boot-P13; 15 s is the shape that
leased on the Orin, where bootpd answered the *second* DISCOVER. On no lease the
driver falls back to a static address and reports it honestly. The station MAC comes
from the DTB `local-mac-address` property of the GENET node.

---

## 5. Bench verification

| Claim | Arc / commit | Boot | Verdict |
| --- | --- | --- | --- |
| RX invalidate-before-read fixes garbage payloads | GENET-5 `0370ebc1` | P13 | Real payloads (`rx[1..5]` pre16==post16) where P10 read garbage — VINDICATED |
| rx[0] first-slot status block is unwritten; descriptor length authoritative | GENET-8 `565bf8c7` | P13, P17 | `dsc` valid + `sb=0` for slot 0; GENET-6 re-poll refuted (10k spins never flip) |
| DHCP window 15 s (was 5 s) | GENET-6 `31f0d269` | P13→P16 | 5 s closed just before a live frame; 15 s leased |
| TX clean-before-publish is the egress fix | GENET-7 `acf11670` | P16 | **FIRST DHCP LEASE — 192.168.2.3/24**, unicast OFFER/ACK to our MAC, gateway ARP + ping back |
| Lease survives / regresses clean | — | P17 | Lease regression PASS |
| PHY LED selectors fix stuck amber | GENET-8 `565bf8c7` | P18 | LED OUT at power-on — METAL-CONFIRMED |
| First TCP service (HTTP :80 over net9) | NET-9/10 `15c6626e` / `4055900c` | P19 | **"UnaOS answers"** — Mac browser loaded `http://192.168.2.3/` off the Pi; screenshot in hand |

QEMU `raspi4b` (bcm2838) does **not** model GENET; the DTB census makes bring-up a
clean pre-MMIO skip (`kernel8-test` stays green). Every finding above is
attended-metal-only.
