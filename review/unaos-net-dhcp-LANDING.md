# NET-DHCP — landing report

**Branch:** `us-netdhcp` (base `main` @ `20dfc71`). **Knobs:** none new — the helper rides the existing
`vnet` / `net4` (+`tegra`) / `smolnet` feature gates on `net_phy.rs`.
**Arc:** a DHCPv4 client on the aarch64 smoltcp seam — QEMU-proven via slirp, wired for the Orin metal
path. The do-it-right fix for the two hard-coded bring-up addresses (VNET's static slirp IP and, more
pointedly, NET-4's `192.168.1.2/24` placeholder that the NET-4 landing flagged as wrong because "the
link's real subnet is a metal input").

## What landed

| milestone | what |
|---|---|
| M1 | A shared, arch-neutral helper `net_phy::dhcp_or_static(prefix, iface, dev, now_ms, timeout_ms, static_ip, static_prefix, static_gw) -> NetConfig`: runs smoltcp's `dhcpv4::Socket` over an already-built `Interface` until a lease is acquired or a bounded (real-time) timeout elapses; on lease it configures the interface addr + default route in place and emits `NET: DHCP lease ip=<..>/<..> gw=<..> (server <..>) => PASS`; on timeout it emits an honest `NET: no lease within <n> ms — falling back to static <ip>/…` line and applies the static config. Fallback PRESERVED — a link with no DHCP server still comes up. Storage is a single-slot stack-local `SocketSet`, no heap growth. Home: **inside `net_phy.rs`** (arch-neutral — it belongs with the shared smoltcp seam, reachable by all three net stacks). |
| M2 | VNET (`virtio_net.rs`) wired: `bind_and_ping` calls `dhcp_or_static` first (3 s bound; `now_ms` = `now_us()/1000` off `CNTPCT`), then pings the gateway of whichever config it settled on. slirp serves the exact static values, so a healthy run **leases** them; ping asserted 4/4 PASS against the DHCP-acquired config. Static path stays reachable behind the fallback. |
| M3 | NET-4 metal (`rtl8168_tegra.rs`, bind-loop only) wired the same way: `bind_smoltcp` calls `dhcp_or_static` first (5 s bound; module-local `now_ms()` off `CNTPCT` at EL2) before the bounded bind-witness poll; the `192.168.1.2/24` placeholder is now the fallback, not the primary. No ring/MMIO/iATU logic touched. Knob-gating unchanged; knob-off builds byte-identical (net_phy + the whole net4 path vanish). Runbook `scripts/orin-net4-bench.md` expected serial chain updated (DHCP lines before ring/ping expectations). |
| M4 | `arch_arm64.md` §NET-DHCP subsection (helper contract + arch-neutral clock rationale + VNET witness + NET-4 metal note) + the runbook update + this report. |

## Files (lane-clean)

- `crates/kernel/src/net_phy.rs` — **additive**: `NetConfig`, `apply_ipv4`, `dhcp_or_static`, imports. No change to the existing `SmoltcpPhy`/`RawNic`/`RxObserver` seam.
- `crates/kernel/src/arch/aarch64/virtio_net.rs` — `bind_and_ping` DHCP-first; `DHCP_TIMEOUT_MS` const; pings the settled gateway; witness line tags `[dhcp|static]`. Ring/transport code untouched.
- `crates/kernel/src/arch/aarch64/rtl8168_tegra.rs` — **bind-loop only**: module-local `now_ms()`, `DHCP_TIMEOUT_MS`, `bind_smoltcp` DHCP-first, witness line reports the settled config + `[dhcp|static]`. Ring/MMIO/iATU/DTB logic untouched.
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — NET-DHCP subsection.
- `unaos/scripts/orin-net4-bench.md` — DHCP lines in the expected serial chain + a NET-DHCP note.

x86 `smolnet` untouched (flagged below as a trivially reusable future fold).

## Fallback design

The helper is the single point where DHCP-or-static is decided, so both aarch64 seams behave identically:

1. Build the `Interface` (MAC only, no address yet).
2. `dhcp_or_static` adds a `dhcpv4::Socket`, then loops `iface.poll` + `socket.poll()` with a real
   `CNTPCT`-derived millisecond clock feeding *both* smoltcp's `Instant` and the timeout check.
3. **Lease** (`Event::Configured`) ⇒ apply leased addr + router (router-less lease falls back to the
   static gateway for a sane default route), emit `=> PASS`, return `NetConfig { leased: true, .. }`.
4. **Timeout** ⇒ apply the caller's static ip/prefix/gw (the pre-DHCP behaviour), emit the honest
   `no lease … falling back to static …` line, return `NetConfig { leased: false, .. }`.

Non-hanging by construction: the bound is real wall-clock time (CNTPCT), not iteration count. The static
values are never deleted — they are the honest last resort for a DHCP-less metal link.

## Gate results (verbatim)

- `./arroyo check` (default, both arches) — **Finished** (green).
- `UNAOS_NET4=1 UNAOS_TEGRA=1 ./arroyo check` (both arches) — **Finished** (green).
- `UNAOS_VNET=1 ./arroyo check` (both arches) — **Finished** (green).
- `UNAOS_VNET=1 UNAOS_GICV3=1 ./arroyo test-arm 40` — DHCP lease PASS + ping 4/4 `[dhcp]` PASS + CAPSTONE 6/6; **0 FAIL**.
- Knob-off regressions, all **0 FAIL**: `UNAOS_GICV3=1 ./arroyo test-arm 40`, `./arroyo test-arm 22`, `./arroyo test 22` (x86 smolnet SOCK-1..7 unregressed), `./arroyo kernel8-test 35` (Pi 4 raspi4b, exit 0).

## The lease line observed in QEMU (slirp)

```
:: AARCH64 VNET: NET: DHCP discover (timeout 3000 ms) ::
:: AARCH64 VNET: NET: DHCP lease ip=10.0.2.15/24 gw=10.0.2.2 (server 10.0.2.2) => PASS ::
:: AARCH64 VNET: ping 10.0.2.2 RTT 4248 us (4/4 sent, 4/4 replies) [dhcp] => PASS ::
```

## Flagged

- **x86 `smolnet` reuse (future fold).** `dhcp_or_static` is arch-neutral and lives in `net_phy.rs`; the
  x86 default stack could call it in place of its static bind. Out of this arc's lane (x86 smolnet
  untouched per brief) — a clean, small follow-up if desired.
- **Metal is compile-tested only** (QEMU models no Tegra234 RC): the NET-4 DHCP path's runtime correctness
  is carried by the RTL8168/slirp seam equivalence VNET proves + the net4/tegra `check` matrix. The
  attended Orin sitting will show `[dhcp]` (link has a DHCP server) or the bounded `[static]` fallback.
