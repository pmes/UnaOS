# NET-PHY — landing report

**Branch:** `hw-jetson` (base `main` @ `51d7ec1`). **Knobs:** none of its own — the module is gated
`any(feature = "net4", feature = "vnet")`; exercised via `UNAOS_NET4`/`UNAOS_TEGRA` and `UNAOS_VNET`.
**Arc:** factor the shared aarch64 smoltcp `phy::Device` adapter — the integrator-scoped fold the VNET
landing flagged (review/unaos-vnet-LANDING.md §flag 1). A pure code move; **zero behavior change.**

## What landed

| milestone | what |
|---|---|
| M1 | New `crates/kernel/src/arch/aarch64/net_phy.rs`: hosts the `phy::Device` / `RxToken` / `TxToken` boilerplate ONCE as `SmoltcpPhy<N>` (owns the struct-local RX/TX scratch, `FRAME_CAP = 1536`, no heap), generic over a small `RawNic` trait (`rx_frame_raw` / `transmit` / `mac`, all associated fns over each driver's module-static NIC registry). `fmt_mac` moved here too. Ported **both** drivers onto it: `rtl8168_tegra.rs` (`Rtl8168Nic`) and `virtio_net.rs` (`VnetNic`). Each driver's `Interface`/socket bind loop stays per-driver (NET-4 = bounded bind-witness poll; VNET = live ICMP echo). |
| M2 | `arch_arm64.md`: new §NET-PHY + one-liner pointers folded into the §ORIN-NET-4 bind row and the §AARCH64-VNET comparison table. This report. |

## Trait shape chosen

```rust
pub trait RawNic {
    fn rx_frame_raw(out: &mut [u8]) -> Option<usize>; // pop one raw RX frame (recycle the descriptor)
    fn transmit(frame: &[u8]);                          // send one raw L2 frame
    fn mac() -> Option<[u8; 6]>;                         // station MAC, or None if unregistered
}
```

Associated functions (no `self`): the raw-frame accessors reach the one registered NIC through the
`NET4_DEVICE` / `VNET_DEVICE` static registry behind a short-held lock (the e1000 `raw_rx`/`raw_tx`
discipline — never hold the lock across a poll). `SmoltcpPhy<N>` carries a `PhantomData<N>`; the
`PhyTxToken<'a, N>` threads `N` so `consume` calls `N::transmit`. Device GATs use `where Self: 'a`.

## Zero-behavior-change accounting

Pure code move. The adapter's datapath (`receive` destructures disjoint scratch fields; `TxToken::consume`
copies then transmits), the short-lock discipline, the no-alloc struct-local scratch, the smoltcp
capability shape (Ethernet, MTU 1500), witness lines, and both feature gates are the exact code the two
drivers carried. The only mechanical restructure: NET-4's `hw_addr()` (returned `(mac, ip, up)` in one
lock) split into `Rtl8168Nic::mac()` + a local `link_up()` (`ip` is the `OUR_IP` const) — two short locks
where there was one, but single-core and with nothing mutating between them, so observably identical.

## Files (lane-clean)

- **New:** `crates/kernel/src/arch/aarch64/net_phy.rs` (the shared adapter).
- `crates/kernel/src/arch/aarch64/mod.rs` — `#[cfg(any(feature="net4",feature="vnet"))] pub mod net_phy;`.
- `crates/kernel/src/arch/aarch64/rtl8168_tegra.rs` — ported onto `SmoltcpPhy<Rtl8168Nic>` (adapter + `fmt_mac` + `FRAME_CAP` removed; `RawNic` impl + `link_up()` added).
- `crates/kernel/src/arch/aarch64/virtio_net.rs` — ported onto `SmoltcpPhy<VnetNic>` (adapter + `fmt_mac` + `FRAME_CAP` + `hw_addr` removed; `RawNic` impl added).
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — §NET-PHY + the two one-liner pointers.

No files outside the named lane were touched.

## Gate results (verbatim)

| gate | result |
|---|---|
| `./arroyo check` (default, both arches) | `✅ x86_64 OK` · `✅ aarch64 OK` |
| `UNAOS_NET4=1 UNAOS_TEGRA=1 ./arroyo check` (both) | `✅ x86_64 OK` · `✅ aarch64 OK` |
| `UNAOS_VNET=1 ./arroyo check` (both) | `✅ x86_64 OK` · `✅ aarch64 OK` |
| feature matrix: net4-only · net4+vnet · net4+tegra+vnet (both arches) | all `✅ x86_64 OK` · `✅ aarch64 OK` (module compiles under net4-only, vnet-only, both, neither) |
| `UNAOS_VNET=1 UNAOS_GICV3=1 ./arroyo test-arm 40` | `:: AARCH64 VNET: ping 10.0.2.2 RTT 4730 us (4/4 sent, 4/4 replies) => PASS ::` **and** `:: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::` |
| knob-off `UNAOS_GICV3=1 ./arroyo test-arm 40` | CAPSTONE COMPLETE 6/6; **no VNET lines** (compiled out) |
| knob-off `./arroyo test-arm 22` (GICv2) | `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<` |
| `./arroyo kernel8-test` | **0 FAIL** (Pi unaffected) |
| `./arroyo test 22` (x86) | `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<` |

## Flagged

- Nothing outside lane. The x86 `smolnet.rs` carries the same adapter shape but is a different arch/lane —
  not folded here (would exceed the brief). A future cross-arch fold could unify all three, but the token
  lifetimes and registry seams differ enough that it is its own scoped pass, not free.
- Committed on `hw-jetson` only; **not merged/pushed** (integrator merges, Peter pushes).
