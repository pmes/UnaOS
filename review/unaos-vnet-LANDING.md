# AARCH64-VNET — landing report

**Branch:** `us-vnet` (base `main` @ `e3fbf3f`). **Knob:** `UNAOS_VNET=1` (feature `vnet = ["dep:smoltcp"]`).
**Arc:** a virtio-net-mmio driver on the QEMU `virt` machine + smoltcp bind — the **QEMU-testable, end-to-end**
proof of the aarch64 smoltcp seam ORIN-NET-4 built. NET-4's RTL8168 driver is `tegra`-gated (QEMU models no
Tegra234 RC) and only ever compile-tested off metal; AARCH64-VNET exercises the identical seam shape
(ring → `smoltcp::phy::Device` → `Interface` → ICMP echo) against a device QEMU *does* model, driven with
**real packets over slirp**.

## What landed

| milestone | what |
|---|---|
| M1 | virtio-mmio transport discovery: scan the QEMU `virt` fixed window (`0x0a00_0000`, stride `0x200`, 32 slots — in the low-1-GiB Device map, so no DTB needed; the GICv3 handoff leaves no DTB anyway). Match magic `virt` + device-id 1; read + report the `Version` register (LEGACY / v1 is QEMU's `force-legacy=true` default; a modern v2 transport is reported and skipped honestly). |
| M2 | virtio-net device init per the legacy virtio spec: status handshake (reset → ACKNOWLEDGE → DRIVER), minimal feature negotiation (accept **only `VIRTIO_NET_F_MAC`**), `GuestPageSize = 4096`, RX/TX split virtqueues (`QueuePFN = region >> 12`; each ring a page-aligned `alloc_zeroed` block, identity-physical base = the device's DMA target, the NET-4/e1000 invariant), pre-post RX descriptors, read the MAC from config space, DRIVER_OK. Raw frame tx/rx with the 10-byte `virtio_net_hdr` prepended/stripped. |
| M3 | `smoltcp::phy::Device` (`VirtioPhy`) over the rings via `raw_rx`/`raw_tx` on a `VNET_DEVICE` registry — the NET-4 seam shape, mirrored — then an `Interface` (static `10.0.2.15/24`, gw `10.0.2.2`) + ICMP socket, a bounded non-hanging poll pump driving 4 echoes to the slirp gateway, RTT measured off `CNTPCT_EL0`, and the self-checking `:: AARCH64 VNET: … => PASS ::` witness line. |
| M4 | arch_arm64.md §AARCH64-VNET (purpose + the NET-4 relationship table + the transport/negotiation facts + witness) + this report + Cargo/arroyo `UNAOS_VNET` wiring (feature + `VNET_ARG` QEMU netdev). |

## Files (lane-clean)

- **New:** `crates/kernel/src/arch/aarch64/virtio_net.rs` (the whole driver + adapter + witness).
- `crates/kernel/src/arch/aarch64/mod.rs` — `#[cfg(feature="vnet")] pub mod virtio_net;`.
- `crates/kernel/src/main.rs` — one `#[cfg(feature="vnet")]` call site on the GICv3 virt path (next to the NET-4 witness, at EL2 before the JC3 drop).
- `crates/kernel/Cargo.toml` — the `vnet` feature.
- `arroyo` — `UNAOS_VNET` feature wiring + `VNET_ARG` (`-netdev user -device virtio-net-device`) in both `launch_aarch64` and `test_aarch64`.
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — §AARCH64-VNET.

No files outside the named lane were touched. **`rtl8168_tegra.rs` is UNCHANGED.**

## virtio-mmio version + features negotiated (observed)

- **Transport:** LEGACY virtio-mmio, **version 1** (QEMU 11.0.1 `virt`, `force-legacy` default). Found at slot **31** (`0xa003e00`) — QEMU fills virtio-mmio transports top-down; the scan is order-agnostic.
- **Device features offered:** `0x39bf8064`. **Accepted:** `0x00000020` = **`VIRTIO_NET_F_MAC`** only.
- **Station MAC:** `52:54:00:12:34:56` (QEMU default). RX/TX virtqueues 16 descriptors each.

## Witness line

```
:: AARCH64 VNET: ping 10.0.2.2 RTT 4374 us (4/4 sent, 4/4 replies) => PASS ::
```

## Gate results (verbatim)

| gate | result |
|---|---|
| `./arroyo check` (default, both arches) | **OK** (aarch64 OK, x86 OK; unchanged) |
| `UNAOS_VNET=1 ./arroyo check` (both arches) | **OK** (zero net-new warnings) |
| knob-off `./arroyo test-arm 22` (GICv2) | `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<` |
| knob-off `UNAOS_GICV3=1 ./arroyo test-arm 40` | CAPSTONE COMPLETE 6/6 + `priority+aging PASS` + VUG-HONESTY PASS; **no VNET lines** (compiled out) |
| knob-on `UNAOS_VNET=1 UNAOS_GICV3=1 ./arroyo test-arm 40` | the `AARCH64 VNET … RTL … 4/4 replies => PASS` witness fires **and** CAPSTONE still COMPLETE 6/6 |
| `./arroyo kernel8-test` | **0 FAIL** (Pi unaffected — `vnet` is aarch64-virt-only) |
| `./arroyo test 22` (x86) | MISSION SUCCESS + all SOCK-1..7 witnesses OK, **0 FAIL/PANIC** |

## Byte-identity (knob-off)

`vnet` is default-OFF and armed only by `UNAOS_VNET=1`. With it off, the module + call site are compiled
out and the smoltcp dep is not pulled, so the default virt/tegra/Pi media are byte-identical to baseline;
and `VNET_ARG` is empty, so the QEMU invocation is byte-identical too. (Full objcopy per-section
verification is an esp-build step, deferred with metal — this arc built no metal media, and the target is a
QEMU device.)

## Flagged

- **smoltcp-adapter factoring (deliberately NOT done):** the `phy::Device` / `RxToken` / `TxToken` boilerplate
  in `virtio_net.rs` is near-identical to `rtl8168_tegra.rs` (and to the x86 `smolnet.rs`). A shared
  `arch/aarch64/net_phy.rs` could host it — **but** factoring would require editing `rtl8168_tegra.rs` to
  consume the shared module, which is out of this lane and would change NET-4's code. Per the brief, the
  copy stands and the factoring is flagged here for a future integrator-scoped pass across both aarch64 net
  modules once they're on the same base.
- **DMA is unmediated in QEMU** (no SMMU in the default `virt` invocation), so this arc does **not** and
  cannot settle NET-4's SMMU-bypass metal unknown — a different question on different silicon. What it *does*
  settle: the ring-advance / descriptor-ownership / header-strip / smoltcp-poll mechanics are correct with
  real packets.
- Committed on `us-vnet` only; **not merged/pushed** (integrator merges, Peter pushes).
