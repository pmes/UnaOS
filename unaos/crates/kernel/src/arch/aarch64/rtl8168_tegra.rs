// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-NET-4 — Realtek RTL8168/8111 GbE driver + smoltcp bind (`net4` gated). The Orin's FIRST
// network path.
//
// ## Ground truth (the metal record of NET-1/2/3 — NOT re-litigated here)
//
// The Jetson Orin Nano devkit's NIC sits behind Tegra234 PCIe controller 0 (`/bus@0/pcie@140a0000`,
// domain 8). NET-3 widened the tegra TCR PS 36→40-bit, enabled controller-0's LTSSM (link UP gen1
// x1), reached the ECAM (`0x2e_2000_0000`, ~184 GiB) through the widened regime, enumerated
// bus1:dev0:fn0 = **Realtek RTL8168/8111, 0x10ec:0x8168**, and sized its BARs: BAR0 I/O 0x100,
// BAR2 mem 0x1000 (the 4 KiB register window — the driver's MMIO), BAR4 mem 0x4000 (MSI-X). NET-4 is
// the driver that stands on that: claim the device, map BAR2, drive the datasheet C+ command /
// descriptor-ring programming model, read the station MAC, and bind smoltcp over the rings.
//
// ## Code-complete-prior-to-metal (by design)
//
// QEMU models no Tegra234 root complex, so the whole driver is `tegra`-gated at the MMIO/DMA layer:
// a `net4`-standalone (virt) build performs no MMIO and prints a single honest witness line
// (`net4_witness_virt`); only `UNAOS_NET4=1 UNAOS_TEGRA=1` on real Orin silicon exercises the rings.
// Correctness comes from `arroyo check`, the QEMU regression non-regression (the tegra code is
// compiled out on virt), unit-testable descriptor logic, and faithful adherence to the RTL8168
// programming model (Realtek datasheet + Linux `drivers/net/ethernet/realtek/r8169_main.c`).
//
// ## DMA / identity-map invariant (and its honest metal risk)
//
// `mmu_tegra` builds an IDENTITY map (VA==PA) for RAM, so — exactly as the x86 e1000 driver relies on
// UEFI's 1:1 tables — a heap allocation's virtual pointer doubles as the physical address the NIC
// DMAs against. The one metal-pending unknown this cannot settle in QEMU: whether the SMMU
// (`smmu_tegra`) is translating (or bypassing) controller-0's PCIe stream IDs. NET-4 programs the
// rings with the identity-physical addresses and documents the SMMU-bypass assumption; an attended
// sitting confirms it (see arch_arm64.md §ORIN-NET-4).
//
// The SECOND metal-pending unknown (review-lens fold): CACHE COHERENCY. Rings and buffers live in
// Normal cacheable RAM and are handed over with `dsb sy` only — `dsb` orders visibility for
// COHERENT observers, it cleans/invalidates nothing. The x86 e1000 seam gets coherent DMA from the
// architecture; aarch64 does not promise it. Correctness therefore assumes Tegra234 controller-0
// is I/O-coherent (ACE-lite) toward DRAM.
//
// ## NET-4f — that assumption is REFUTED on Orin silicon; the RX buffers are non-coherent.
//
// NET-4e added a `dsb ld` DMA read barrier on the RX pop (the ORDERING theory: CPU sees OWN-clear
// before the payload write). Boot 3 refuted it: barrier active, the FIRST popped frame reads real
// bytes (its buffer's cache line missed), every subsequent frame reads ALL-ZERO payload though its
// DESCRIPTOR carries a real length — the NIC's DMA writes to DRAM are never observed by the CPU.
// Root cause: the buffers are `alloc_zeroed`, which leaves DIRTY zero lines resident in the D-cache;
// controller-0's PCIe write path does not snoop the Cortex-A78 cache (no IO-coherency granted here),
// so the CPU keeps hitting its own cached zeros. This is genuine non-coherent DMA, not an ordering
// bug. The fix (this file, arch/aarch64/cache.rs primitives), matching the Pi 4 VideoCore recipe:
//   * RX (device→CPU, DMA_FROM_DEVICE): INVALIDATE (`dc ivac`) each buffer before handing OWN to the
//     NIC (at alloc + at recycle) so the dirty zero lines are dropped and can never be written back
//     over the NIC's data, and INVALIDATE `[buf, buf+len)` again after OWN-clear before the copy so a
//     speculatively-prefetched line is dropped and the read re-fetches DRAM. `dc ivac` (not `civac`)
//     is mandatory: a clean would flush the stale zeros to DRAM ON TOP of the NIC's payload.
//   * TX (CPU→device, DMA_TO_DEVICE): CLEAN (`dc cvac`) the frame buffer after the copy, before the
//     doorbell, so the NIC reads the CPU's bytes from DRAM rather than stale RAM. TX "worked" pre-fix
//     only by the racy luck of an eviction landing the DISCOVER in DRAM before the NIC fetched it; a
//     lost future TX is the same non-coherent class, so it is fixed symmetrically (honesty > minimal).
// The descriptor rings were NOT given maintenance at NET-4f — metal showed their writebacks observed
// correctly (real per-slot lengths even for zero-payload frames), which read as "ring coherent". That
// asymmetry was later RESOLVED, not exonerated: the writebacks ride the NIC's internal ring
// base+index, while its descriptor FETCHES read ring DRAM — see NET-4x below, which extends the
// clean/invalidate discipline to the rings. The OWN protocol is unweakened.
//
// ## NET-4m — the per-pop invalidate is ALREADY on the live path; the residual zeros are NOT a cache bug.
//
// Boot 15 kept the rx[2..] all-zero payloads even with NET-4l's correct OWN-last re-arm active. The
// natural next suspect — "the `dc ivac` invalidate-before-read fires only on the first pop" — is FALSE:
// `rx_frame_raw`'s `cache::invalidate_range(buf, len)` is UNCONDITIONAL and runs on every pop (only the
// `[net4f]` serial WITNESS is one-shot, `rx_count == 0` — the source of that misread). The copy that
// feeds NET-4d is therefore ALREADY a post-invalidate DRAM read, so the zeros it reports are what the
// buffer DRAM holds after the invalidate — "more invalidate" cannot change them. Two facts pin the
// residual cause down and, decisively, BOTH lie outside this driver's invalidate lane:
//   1. NET-4g proves the descriptor `addr` is all-MATCH (the NIC has the buffer addresses the driver
//      programmed), so this is not descriptor corruption.
//   2. The ring is observed COHERENTLY (real per-slot lengths, OWN-clear seen, frames pop) while the
//      buffers read zero — an ASYMMETRY a pure cache-coherency defect cannot produce, since ring and
//      buffers share one cacheable identity-mapped heap and one DMA master. A cache bug would zero the
//      ring too.
// So the honest "do it right" fork (per the NET-4m brief item 3) resolves to one of two arcs, and the
// `[net4m]` speculation-fenced buffer-DRAM probe below discriminates them on the next boot:
//   * writes-to-nowhere: the NIC's payload DMA never lands in the CPU-visible buffer DRAM — an inbound
//     reachability gap (SMMU / inbound iATU / ORIN-DMA-WINDOW). BELOW the driver's lane.
//   * cache/speculation (less likely, given the asymmetry): the buffer DRAM holds the payload but the
//     cacheable read shadows it — cured only by a NON-CACHEABLE DMA arena (a Normal-NC MAIR slot +
//     splitting mmu_tegra's 1 GiB RAM block to L2/L3 page granularity). An MMU arc, not a driver arc.
// Either fix is a SEPARATE arc outside this file; NET-4m's job is to name which, not to weaken the
// (already-correct) per-pop invalidate or the OWN-last re-arm.
//
// ## NET-4n — the discriminator fired writes-to-nowhere; the truncation is IN this file (64-bit DMA off).
//
// Boot-16 armed the `[net4m]` speculation-fenced probe. Verdict: rx[1..4] read the raw buffer DRAM
// ZERO with a real descriptor length -> WRITES-TO-NOWHERE, not cache/speculation. The RC confirmed it:
// an IOB `FillWrite` RAS, ADDR 0x8000000000000200 (bit-63-stripped 0x200 = the fabric slave-error sink
// for an inbound write that matched no inbound region). So the inbound path (NET-4h iATU armed identity
// [0x8000_0000, 0x2_8000_0000), NET-4i SMMU bypassing) is NOT the gap — the addresses reaching it are.
//
// Root cause, localized to this driver: the C+ RX/TX engine was left in 32-bit-payload-DMA mode
// (CPlusCmd.PCIDAC clear; boot-16 read back 0x2021). The engine reaches the descriptor RING through the
// dedicated 64-bit RDSAR/TNPDS registers (so fetch + writeback of a >4 GiB ring are fine — the ring
// stays coherent, the NET-4m asymmetry), but for the per-buffer PAYLOAD write it uses only
// `Desc.addr[31:0]`. With `ORIN-DMA-WINDOW` seating the heap high (~9.6 GiB; boot-16 ring @ 0x2683ca000,
// buffers @ 0x2683cbXXX, all >4 GiB, net4g [MATCH]) the payload address truncates to ~1.6 GiB, BELOW the
// inbound iATU's 0x8000_0000 base -> no region matches -> slave-error -> the 0x200 FillWrite + a buffer
// that keeps its alloc_zeroed zeros. Only the FIRST buffer filled after RxEnb lands cleanly (its address
// latched from the ring context, not the truncating per-buffer path); rx[5]'s "nonzero" is a torn
// hi/lo write (bytes present, not a valid frame), the same defect. The low-heap boots (heap at
// 0x8000_0000, boots 1..5) never needed the high dword, so they masked this — until the RAS-2 heap-guard
// moved the heap high for good.
//
// ## NET-4n / NET-4o / NET-4p — SUPERSEDED (built on a misread bit).
//
// Boots 16..18 chased CplusCmd.PCIDAC (C+CR bit 4) as the "64-bit >4 GiB buffer-address enable": NET-4n
// set it pre-ring, NET-4n2 re-applied it as the final C+CR write, both read back CLEAR (0x2021), and that
// "won't latch" was read as proof the payload DMA is 32-bit-only. NET-4o then tried to seat the whole NIC
// DMA surface in a sub-4 GiB arena (boot-19: no clean low DRAM exists — the [2,4) GiB window is packed
// with Tegra carveouts), and NET-4p aliased the truncated 32-bit writes back up via inbound iATU regions.
// The L4T facts (r8169_main.c) refute the premise the whole chain rests on: **PCIDAC is a parallel-PCI
// "Dual Address Cycle" relic** the reference driver NEVER sets for a PCIe 8168/8125 and masks OUT of the
// value it writes (CPCMD_MASK). On a PCIe part a 64-bit (DAC) memory TLP is formed NATIVELY from the
// descriptor's `__le64 addr` high dword — there is no C+ enable bit. So "PCIDAC won't latch" is EXPECTED
// and HARMLESS (a no-op on this silicon), NOT evidence of a 32-bit-only path; the arena and the alias were
// solving a non-problem at the wrong layer.
//
// ## NET-4q — emit the full 64-bit descriptor/ring address; region-0 identity carries it; no alias.
//
// The RTL8168 requests a 64-bit DMA mask for every 8168c-and-later (r8169 gates only `mac_version >=
// VER_18`; our PCIe 8168 qualifies), and does 64-bit RX/TX purely by putting the full PA in the
// descriptor's `__le64 addr`. Our driver already publishes the full 64-bit `Desc.addr` (u64, both dwords —
// net4g [MATCH] on metal). The one place we diverged from the documented sequence: the descriptor-RING
// base registers (RDSAR 0xe4/0xe8, TNPDS 0x20/0x24) were written LOW-then-HIGH; r8169 writes the HIGH
// dword BEFORE the Low as a documented erratum ordering. NET-4q (a) reverses that to High-before-Low on
// both ring bases, (b) drops the PCIDAC writes entirely (dead bit; the rings-up readback keeps PCIDAC as a
// witness, EXPECTED clear), and (c) removes the NET-4p inbound-iATU alias. The NIC then emits the full
// `0x2683cbXXX` DAC TLP natively; the NET-4h region-0 identity `[0x8000_0000, 0x2_8000_0000)` already
// covers the high buffer PAs (~9.6 GiB), so the write lands with no alias, no arena, no PCIDAC. Boot-21
// oracle: the full-64 desc witness, `[net4m]` all-nonzero, no `0x200` IOB FillWrite RAS, DHCP LEASE.
// FALLBACK (only if a provably-correct full-64 descriptor STILL yields zero on metal — the first real
// evidence the NIC/RC integration cannot emit >4 GiB TLPs): re-arm the inbound alias per the facts, writing
// UPPER_BASE + UPPER_TARGET with base congruent to target mod 64 KiB and CTRL2=ENABLE read back.
//
// ## NET-4r — the sanctioned fallback fires: boot-24 falsified native-64; re-arm the alias CORRECTLY.
//
// Boot-24 ran NET-4q for real: RINGDUMP proved the descriptors carry the FULL 64-bit buffer addresses
// (`addr=0x2683cb… [MATCH]`, EOR + hi-before-lo ring bases in) — and the result was STILL no lease + the
// `0x200` IOB `FillWrite`/ACI sink RAS. A provably-correct full-64 descriptor yielding the sink is the
// first real evidence this RTL8168/Tegra-RC integration cannot emit/carry a >4 GiB DMA TLP: the payload
// (and, unprovable-otherwise, the ring) truncates to `addr[31:0]` on the wire, landing below the
// `0x8000_0000` DRAM/identity base → the fabric sink. NET-4q's own falsification clause fires: keep the
// full-64 descriptor + ring path UNCHANGED (correct per the reference driver, harmless, and the delivery
// vehicle if the NIC ever emits full-64 natively — the TLP then hits the NET-4h identity region), and
// re-arm the inbound-iATU ALIAS as the delivery mechanism — CORRECTLY this time, per L4T-FACTS §A0-A5:
//   * a FREE inbound index per block (`enumerate_inbound_windows` witnesses the region file; index 0 =
//     the NET-4h identity; aliases take clear indices — §A5, no index collision);
//   * LOWER/UPPER_BASE ← truncated 32-bit bus base, LIMIT ← 64 KiB-rounded end, LOWER/UPPER_TARGET ← the
//     real high PA, ALL written AND READ BACK with refuse-to-proceed on any mismatch — the UPPER_TARGET
//     `0x2` dword boot-20 lost is the whole point (`program_inbound_alias`);
//   * base ≡ target (mod 64 KiB), both 64 KiB-ALIGNED outright (alloc_rx/alloc_tx nudged to 0x10000), so
//     the controller's 64 KiB granularity (§A2) rounds nothing (§A3);
//   * `CTRL2 = ENABLE` only (NO increase-region-size — the match window is sub-4 GiB, §A0/§A2), with the
//     ENABLE bit POLLED on readback (§A5 the driver loops on it);
//   * coverage of BOTH the ring and the buffer block of each direction, witnessed (`arm_dma_aliases`).
// Boot-25 oracle: the enumeration + per-region readbacks all MATCH (UPPER_TARGET=0x2, enabled=1), no
// `0x200` IOB RAS, `[net4m]` all-nonzero, DHCP LEASE.
//
// ## NET-4s — index 2 does not exist: probe the true window count, consolidate to ONE covering region.
//
// Boot-25 ran NET-4r's per-block plan for real. Verdict: `alias region 1 (rx-ring) readback: BASE=0x683d0000
// … UPPER_TARGET=0x00000002 CTRL2=0x80000000 enabled=1 (after 0 polls)` — index 1 armed and readback-PROVED
// on silicon. Then `alias region 2 (rx-buffers) readback: BASE=0 … enabled=0 (after 1000 polls)` → MISMATCH →
// the NET-4r readback ritual correctly fail-closed the bring-up (no DHCP attempted). The Tegra234 DWC RC
// implements FEWER inbound windows than the 8 unroll CSR blocks a CTRL2-read enumeration sees — likely just
// 2 (index 0 = the NET-4h identity, index 1 = ours). NET-4p's "TX region shadowed" is thereby explained:
// those higher indices never existed. Demanding one index PER block (four aliases) could never succeed here.
// The fix keeps NET-4r's full readback ritual + fail-closed UNCHANGED and:
//   1. DISCOVERS the true window count by a WRITABILITY probe (`probe_inbound_windows`, §A5 / the
//      `dw_pcie_iatu_detect` idiom): write a probe value to each index's LOWER_TARGET, read it back, restore —
//      an unimplemented window does not retain the write; windows are contiguous from 0, so the first
//      non-sticking write bounds `num_ib_windows`. Witnessed. Index 0 (the live identity) is counted present
//      without a destructive write.
//   2. CONSOLIDATES every block that needs aliasing into ONE covering region at the proven-armable index 1.
//      The four DMA blocks share one heap neighborhood (boot-25: 0x2683c…-0x2683f…), all carrying the same
//      high dword (0x2). Offset-preservation (§A1: translated = target + (incoming − base)) makes a single
//      64 KiB-aligned window whose fixed up-translate offset is `high_dword << 32` offset-EXACT for every
//      block (`arm_dma_aliases` computes the covering base = min truncated low-32 floored to 64 KiB, limit =
//      max block end rounded up, target = `(high << 32) | base`). The blocks all sharing one high dword is
//      REQUIRED (a single region applies one offset) and enforced fail-closed; the 4 GiB-crossing, PCIe
//      MEM/BAR-overlap, and congruence guards remain. If the heap ever scatters the blocks across a 4 GiB
//      boundary, the covering region refuses rather than mistranslate — the remedy is a re-seated arena.
// Boot-26 oracle: `[net4s] … {N} implemented window(s)` (the discovered count), the covering-window math,
// per-block coverage lines, ONE `program_inbound_alias` readback MATCH (UPPER_TARGET=0x2, enabled=1) at
// index 1, no `0x200` IOB RAS, `[net4m]` all-nonzero, DHCP LEASE.
//
// ## NET-4v — audit the covering-region LIMIT + arm a per-object coverage witness (boot-30 evidence).
//
// Boot-30 (pi4-r23s1 retry): `[net4u] rx[0]` pre-ivac == post-ivac == real LLDP payload — the first
// buffer's DMA write LANDS — while `[net4m] rx[1..5]` read all-zero DRAM at real descriptor lengths.
// Ring writeback (OWN/len) works every slot. The suspect: covering-region LIMIT undersize. The AUDIT
// REFUTES that suspect with the boot's own armed numbers: the region armed as base=0x683d0000
// limit=0x6840ffff target=0x2683d0000 (readback-proven, enabled=1 after 0 polls), and the required
// span — rx-ring [0x683d0000..0x683d0200), rx-buffers [0x683e0000..0x683f0000), tx-ring
// [0x683f0000..0x683f0080), tx-buffers [0x68400000..0x68404000) — tops out at 0x68403fff, i.e.
// 0xc000 BELOW the armed inclusive limit. Every rounding in the math is on the correct side
// (base floors DOWN to 64 KiB, end rounds UP, limit is the inclusive `end − 1`; §A2). rx[1]'s bus
// address 0x683e0800 sits squarely INSIDE the armed window yet its write sinks, 0x800 bytes after an
// address that translates — a cut no 64 KiB-granular iATU limit can make. So the sink is NOT the
// iATU limit; it is below the region (SMMU / another inbound stage), outside this lane.
// What lands is exactly the NET-4h "firmware-residual" signature (ring page + first buffer).
// This arc arms the missing instrument instead of a speculative size change: `witness_dma_coverage`
// re-reads the armed region's BASE/LIMIT/TARGET registers INDEPENDENTLY of `program_inbound_alias`'s
// readback, prints every DMA object's required bus span against them, and issues a one-line verdict
// `covers=ALL` / `covers=MISS(<obj>)` — MISS fails the bring-up CLOSED. Boot-31 oracle: `[net4v]`
// four per-object `covered` lines + `covers=ALL` (exonerating the limit on silicon), rx[1..] still
// zero ⇒ the sink is formally below the iATU.
//
// ## NET-4x — the NIC's DESCRIPTOR FETCHES read stale-zero DRAM: clean the ring to PoC.
//
// Boot-32 exonerated both remaining below-driver suspects (iATU covers=ALL readback-proven on
// boot-31; BOTH SMMU instances CLIENTPD=1 — globally bypassed, no translation, no residual
// mappings) and scaled the pattern: 32 pops in the window, ONLY slot-0 recycles carry payload
// (SSDP len 443, LLDP, …), every other slot DRAM-zero at a real writeback length. The asymmetry
// that exonerated the ring in NET-4f/4m now RESOLVES instead: the NIC addresses its OWN/len
// WRITEBACKS via the internal ring base+index it latched from RDSAR/TNPDS (those land and are
// observed), but its descriptor FETCHES read the per-desc buffer-address field from ring DRAM —
// and our ring lives in CACHEABLE heap written with volatile stores + `dsb sy` only. `dsb`
// orders visibility for coherent observers; it cleans NOTHING to the PoC. So the NIC fetches
// whatever last reached DRAM — mostly the alloc_zeroed fill — and DMA-writes payloads to
// near-zero bus addresses the fabric sinks (bonus: the real source of the intermittent 0x200
// IOB/ACI RAS previously bracketed to xHCI by timing alone). slot-0 lands because its line
// reached DRAM (eviction / init timing), the NET-4h "firmware-residual" misread.
//
// The reference discipline (L4T r8169_main.c, facts only, re-verified this arc against v6.6):
// the rings are allocated with `dma_alloc_coherent` (rtl_open ~:4705-4716 — coherent /
// non-cacheable-to-the-device memory; the in-code comment notes descriptors need 256-byte
// alignment and coherent alloc provides more), so `dma_wmb()` before the OWN publish
// (rtl8169_mark_to_asic :3799-3807; TX start_xmit ~:4244-4249) is ALL the maintenance it ever
// needs, plus `dma_rmb()` after seeing OWN clear (:4430-4438). We kept the barrier discipline
// (NET-4l LAW) but on a CACHEABLE ring — the missing half is the CLEAN. Fix (this arc, option
// (a) of the brief — keeps the cacheable heap):
//   * per re-arm/post: publish the descriptor BODY (OWN clear), `dc cvac` its line to PoC, then
//     the OWN store, then `dc cvac` again — OWN-last now holds AT DRAM, not just in cache order;
//   * at init: clean BOTH whole rings after all descriptors are written, BEFORE RxEnb/TxEnb, and
//     witness desc[1]/desc[17] addr via a post-invalidate DRAM read (`[net4x]` line);
//   * before every CPU read of a descriptor (RX OWN check, TX completion poll): `dc ivac` the
//     line first — a resident copy left by our own clean would otherwise shadow the NIC's
//     writeback (cvac leaves the line resident; without the read-side invalidate the fix would
//     break the PROVEN writeback-observation path).
// Line geometry (hard-won): descriptors are 16 B, 4 per 64 B line; every clean covers a whole
// line, so ALL fields of a descriptor are written before its clean and no clean is issued
// between neighbor writes mid-init (the whole-ring cleans run after the init loops complete).
//
// ## NET-4y — C+ RX-mode audit: early-RX was never disabled; CPlusCmd was never written.
//
// Boot-33 kept the pattern with the ring clean in (init witness MATCH): 13 pops, only slot-0
// carries payload, slots 1+ DRAM-zero at real writeback lengths — and boot-32 showed slot-0's
// CONTENTS changing across pops. The line-by-line audit against r8169 (facts only) found the
// init sequence diverging from `rtl_hw_start`/`rtl_init_rxcfg` in exactly the RX-mode registers:
//   * RxConfig (0x44): we wrote the 8169-only RX_FIFO_THRESH field (7<<13); on every 8168 the
//     reference instead writes RX128_INT_EN(15) | RX_MULTI_EN(14) | RX_DMA_BURST, and for the
//     modern family (VER_40..53) RX_EARLY_OFF(11) — early-RX DISABLED. We never set bit11, so
//     early-RX ran enabled (a mode with its own payload-address latching — the named suspect for
//     every-payload-to-buffer[0]); and we set bit13, which r8169 never sets on an 8168.
//   * CPlusCmd (0xE0): `rtl_hw_start` WRITES `CPlusCmd = reset & CPCMD_MASK`
//     (Normal_mode|RxVlan|RxChkSum); we only read it, leaving reset residue (0x2021's stray
//     bit0) unscrubbed. Now written masked, per the reference.
//   * Order: reference is ChipCmd(RxEnb|TxEnb) -> RxConfig -> TxConfig; we wrote TCR before CR.
//     Aligned.
//   * Verified-matching (no change): RDSAR/TNPDS hi-before-lo, the 16-byte normal descriptor
//     (opts1/opts2/addr — not an 8169-format divergence), RMS (0xDA) <= buffer size, and NO
//     legacy RBSTART(0x30)-era single-buffer path exists anywhere in this driver.
// Plus the decisive witness (knob-gated): on the first ≤3 pops of a slot other than 0, buffer[0]
// is independently invalidated+fenced and its first 16 bytes printed with a
// `buf0-holds-the-frame=yes/no` verdict (nonzero + sane EtherType, and same-as-this-pop's-frame)
// — if yes, the NIC provably resolves every RX payload to buffer[0]'s address.
//
// ## NET-4A — the stuck payload address is a NIC-internal descriptor-address REUSE, not a cache bug.
//
// Boot-36-retry (net4z scan-all, RINGDUMP armed) gave the decisive fact: rx[0]→buffer 0 (correct),
// rx[1]→buffer 1 (correct), rx[2..7]→buffer 1, STUCK — including len=108/128 non-ARP frames whose
// heads cannot false-match a stale ARP. Writebacks (OWN/len) land per-slot correctly the whole time;
// descriptor DRAM held correct DISTINCT addrs pre-enable (net4x [MATCH]). The stuck address is
// desc[1]'s (the LAST descriptor of a 2-deep prefetch burst), NOT the arena base — net4y's
// "buf0-holds-the-frame" was a stale-frame artifact. This arc ruled the brief's two candidate
// mechanisms in/out by CODE ANALYSIS and lands the verdict + a cross-pop witness (no functional fix —
// the survivor is below this driver's lane; NET-4v refutation precedent).
//
//   * Mechanism (b) — per-pop re-arm's cache-line interplay (descs are 16 B, 4 per 64 B line; every
//     clean/invalidate covers a whole line) — is REFUTED, two independent ways:
//       1. CODE: the descriptor `addr` field in ring DRAM is only ever written with the correctly
//          computed `rx_buffers + i*RX_BUF_SIZE` (alloc_rx + every re-arm), and net4x proves DRAM holds
//          the correct distinct addrs. Trace every maintenance point: the read-side `invalidate_range`
//          at the top of `rx_frame_raw` only DROPS clean lines (re-read from DRAM); the re-arm
//          `clean_range` writes back the CPU's cached line, but that line was just refilled from DRAM
//          (the `read_volatile` a few lines above), so its neighbor descriptors carry their CORRECT
//          addrs. The worst a stale whole-line clean can do is resurrect a same-line neighbor's OWN
//          with the neighbor's OWN (correct) address — it can NEVER substitute desc[1]'s address into
//          desc[2..7]'s slot. So no ring cache op can redirect a payload to a different slot's buffer.
//       2. EVIDENCE: the brief's (b) "partially-stale line" sub-case predicts a ZERO addr fetch →
//          payload to bus 0 → the 0x200 IOB/ACI FillWrite sink. net4z shows a VALID buffer-1 landing
//          and NO 0x200 RAS — the opposite of the (b) prediction.
//     Padding the descriptor to a full 64 B line (the other (b) fix) is illegal here: the 8168 C+ engine
//     indexes descriptors by a FIXED 16-byte stride (RDSAR is the base; there is no programmable
//     descriptor-size/stride register), so a 64 B stride would desync the NIC's index math. And making
//     the RING non-cacheable is NOT an in-file fix: `map_mmio_window` (the only Device/NC lever the
//     driver can call) is 1-GiB-block granular, so it would turn the whole RAM GiB the heap lives in into
//     Device memory — breaking `ldxr/stxr` (every spinlock/atomic) in that GiB. A sub-GiB Normal-NC/L3
//     mapping is an mmu_tegra arc, exactly as NET-4m assessed — out of this lane.
//   * Mechanism (a) — descriptor-fetch burst reuse — SURVIVES and describes the signature: the NIC
//     prefetched a 2-descriptor burst at RxEnb (rx[0]→buf0, rx[1]→buf1), then reuses the last-fetched
//     buffer address (desc[1]'s = buf1) for every later completion while its OWN/len writeback rides the
//     correctly-advancing internal ring index. The reference (r8169) exposes NO host RX doorbell / no
//     per-descriptor prefetch kick (RX fetch is continuous on real silicon; RDSAR rewrite only resets
//     the fetch pointer to the ring base, which r8169 never does in steady state), and our ring DRAM is
//     provably correct — so the failure to re-fetch is NIC/RC-internal (descriptor-fetch/prefetch or the
//     inbound descriptor-READ coherency the fabric serves), BELOW the driver's programming lane. There
//     is no honest in-file functional fix; this arc instead lands the decisive cross-pop witness.
//   * The `[net4A]` witness (knob-gated, read-only; folded into the net4z scan): correlates net4z's
//     per-pop landing indices ACROSS pops and emits ONE verdict line the moment ≥4 consecutive pops land
//     in the same buffer index while the completed slot advances 1:1 — the machine-checkable proof of
//     the reuse mechanism on the next boot (and, symmetrically, its refutation: if a future change makes
//     landing-index == completed-slot for ≥4 consecutive slots, the stuck-run never reaches 3 pairs and
//     the verdict never fires).
//
// ## NET-4C — audit PCIe MPS/MRRS: we program NEITHER side's Device Control; the fetch-completion path.
//
// Boot-38's mechanism-(a) survivor (descriptor-fetch burst reuse, NIC/RC-internal) has a concrete PCIe
// candidate the six prior exonerations never examined: the descriptor READ COMPLETION is truncated by a
// Max_Payload_Size mismatch across the controller-0 link. A completion whose data payload exceeds the
// REQUESTER's MPS is a Malformed TLP the requester drops (PCIe base spec: a receiver checks payload <=
// its own MPS; the completer splits at min of the two MPS). CODE AUDIT of the bring-up: the driver
// programs the endpoint COMMAND register (0x04, MEM+bus-master) and the RTL8168 MAC control/ring
// registers, and NET-3 did the appl LTSSM-enable + BAR sizing — but NOTHING on either side ever writes
// the PCIe-capability Device Control register (MPS bits[7:5], MRRS bits[14:12] at cap+0x08). So both the
// Tegra234 DWC root port and the RTL8168 endpoint run whatever UEFI/MB2 left. L4T does NOT: r8169
// `rtl_jumbo_config` sets the endpoint MRRS to 4096 non-jumbo (`pcie_set_readrq`), and the Linux PCI
// core's `pcie_bus_configure_settings` walks the tree and writes MPS = min-MPSS on the path, so L4T runs
// a KNOWN-COHERENT MPS. If firmware left the RC at a larger MPS than the EP, the RC's descriptor-read
// completions overrun the EP's MPS → dropped → only the first chunk consumed → the NIC reuses the
// last-latched buffer address (exactly the observed 32-B-then-stuck signature). Fix (this arc,
// `net4c_mps_mrrs`, BEFORE rings-up): read DevCap/DevCtl/DevSta on BOTH sides via the ECAM (RC =
// bus0:dev0:fn0, EP = bus1:dev0:fn0), print the `[net4C]` readback table UNCONDITIONALLY, reconcile MPS
// to the smallest value both DevCaps advertise (writing each side only if its field differs; DevSta high
// half written 0 so its RW1C error latches survive), and clamp EP MRRS to a conservative 512 B. The
// DevSta error bits are the discriminator and print either way: a latched UnsupReq/Fatal CONVICTS the
// completion path; all-clear + already-matching MPS is the honest REFUTATION (the truncation is not an
// MPS/MRRS mis-program — mechanism-(a) fetch-reuse stands, below this driver's lane). Metal-pending: the
// witness fires only under `UNAOS_NET4=1 UNAOS_TEGRA=1` on Orin silicon (QEMU models no Tegra234 RC).
//
// ## Write discipline
//
// The driver, being a driver, DOES the fabric writes NET-3 refused: it enables the device's
// MEM-decode + bus-master (command register), and programs the RTL8168 control/ring registers. Every
// config-space write is announced on serial before issue. It touches ONLY controller-0's downstream
// device (bus1:dev0:fn0) and that device's own register BAR — no other controller, no MSI/MSI-X, no
// other config function.

#![cfg(feature = "net4")]

/// Stable serial prefix so the operator (and `mbench`) can grep the whole NET-4 bring-up as one block.
/// (Used by both the witness half below and — via `use super::P4` — the tegra `metal` driver.)
const P4: &str = ":: PCIE4:";

// ── The witness half (virt / non-tegra build): one honest line, zero MMIO ──────────────────────────

/// The QEMU-safe witness: on a `net4`-but-not-`tegra` build (the only PCIe surface QEMU offers is the
/// virt generic ecam — no Tegra234 RC), there is no device to claim, so print a single line recording
/// that the driver is compiled-present but its bring-up is metal-only, and return. This keeps the
/// GICv3 virt regression runs unperturbed (no MMIO, no ring alloc). Mirrors the `census2` graceful skip.
#[cfg(not(feature = "tegra"))]
pub fn net4_bringup(dtb_addr: u64, dtb_size: usize, _ram_gib_mask: u64) {
    serial_println!(
        "{} ORIN-NET-4 RTL8168 driver compiled; no Tegra234 RC on this build (QEMU virt) — bring-up is metal-only (UNAOS_NET4=1 UNAOS_TEGRA=1) ::",
        P4
    );
    // ORIN-DMA-WINDOW (virt witness): exercise the `dma-ranges` derivation against the live DTB. QEMU
    // virt exposes a generic (non-Tegra) `pcie@`, so the Tegra-RC-gated parse yields 0 windows and the
    // heap-guard degrades to the RAS-2 highest-clean heuristic — this line witnesses that fallback path
    // in QEMU (the no-dma-ranges case) without touching MMIO. See `select_heap_region` (mmu_tegra.rs).
    let mut win = [(0u64, 0u64); 8];
    let nd = crate::arch::aarch64::fdt_tegra::pcie_dma_windows(dtb_addr, dtb_size, &mut win);
    if nd == 0 {
        serial_println!(
            "{}   [dmawin] no Tegra PCIe dma-ranges in this DTB — inbound-DMA window NOT derivable; heap-guard degrades to the highest-clean heuristic (QEMU-virt fallback path) ::",
            P4
        );
    } else {
        serial_println!(
            "{}   [dmawin] derived {} inbound-DMA window(s) from dma-ranges; window[0] = [{:#x}, {:#x}) ::",
            P4, nd, win[0].0, win[0].0.wrapping_add(win[0].1)
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// The metal driver (`net4` + `tegra`) — device claim, BAR map, MAC read (M1); rings (M2); bind (M3).
// ══════════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "tegra")]
pub use metal::net4_bringup;

#[cfg(feature = "tegra")]
mod metal {
    use super::P4;
    use crate::arch::aarch64::fdt_tegra::Fdt;
    use crate::arch::aarch64::mmu_tegra::{
        install_net4b_nc, map_mmio_window, net4b_nc_window, MmioMap,
    };
    use crate::net_phy::{fmt_mac, RawNic, SmoltcpPhy};
    use core::ptr::{read_volatile, write_volatile};

    /// NET-4B: DMA memory barriers matching Linux r8169's `dma_wmb()` / `dma_rmb()` on arm64
    /// (`dmb oshst` / `dmb oshld`). With the rings + buffers now in Normal-NC memory (no cache to
    /// clean/invalidate), these are the ONLY maintenance the descriptor protocol needs — they order the
    /// CPU's two accesses as an external observer (the NIC) sees them, which cacheability does not change:
    ///   * `dma_wmb` before publishing OWN so the descriptor BODY (addr/len) reaches DRAM first;
    ///   * `dma_rmb` after observing OWN-clear so the payload read is not reordered ahead of the OWN read.
    /// NC stores/loads still reorder on weakly-ordered aarch64, so both remain load-bearing.
    #[inline]
    fn dma_wmb() {
        unsafe { core::arch::asm!("dmb oshst", options(nostack, preserves_flags)) };
    }
    #[inline]
    fn dma_rmb() {
        unsafe { core::arch::asm!("dmb oshld", options(nostack, preserves_flags)) };
    }

    /// The Realtek vendor id and the RTL8168/8111 device id the NET-3 metal enumeration found.
    const REALTEK_VENDOR: u16 = 0x10ec;
    const RTL8168_DEVICE: u16 = 0x8168;

    /// Poison patterns that mean ABSENT DECODE, never "present" (the PI-V3D-1 false-PASS lesson, shared
    /// with the NET-1/2/3 recon): `0xffffffff` = master-abort / unclaimed config; `0xdeadbeef` =
    /// firmware register/DRAM fill; `0xa5a5a5a5` = the Tegra CARVEOUT poison fill the NET-4 M1 metal
    /// FAULT left behind (a raw PCIe BAR value deref'd as a CPU PA into a protected carveout — the exact
    /// class this fix-forward closes; see the outbound-iATU block below). A live register read is none.
    #[inline]
    fn is_poison(v: u32) -> bool {
        v == 0xffff_ffff || v == 0xdead_beef || v == 0xa5a5_a5a5
    }

    // ── RTL8168/8111 register offsets (bytes from the BAR2 MMIO window) ──
    /// IDR0..5: the six station-MAC bytes (offsets 0x00..0x05).
    const REG_IDR0: u64 = 0x00;
    /// TNPDS: Transmit Normal-Priority Descriptor Start Address (64-bit; low @ 0x20, high @ 0x24).
    /// The ring base MUST be 256-byte aligned.
    const REG_TNPDS: u64 = 0x20;
    /// ChipCmd (CR): RST (soft reset, self-clearing), RxEnb (RE), TxEnb (TE).
    const REG_CR: u64 = 0x37;
    const CR_RST: u8 = 1 << 4;
    const CR_RE: u8 = 1 << 3;
    const CR_TE: u8 = 1 << 2;
    /// TPPoll: kick the normal-priority TX queue (NPQ) after posting a descriptor.
    const REG_TPPOLL: u64 = 0x38;
    const TPPOLL_NPQ: u8 = 1 << 6;
    /// PHYstatus (8-bit): LinkSts (bit 1) — 1 = link up.
    const REG_PHYSTATUS: u64 = 0x6c;
    const PHYSTATUS_LINKSTS: u8 = 1 << 1;
    /// IMR / ISR: interrupt Mask / Status (16-bit). Polled bring-up ⇒ IMR = 0, ISR write-1-to-clear.
    const REG_IMR: u64 = 0x3c;
    const REG_ISR: u64 = 0x3e;
    /// TCR / RCR: Transmit / Receive Configuration (32-bit).
    const REG_TCR: u64 = 0x40;
    const REG_RCR: u64 = 0x44;
    /// CFG9346 (93C46 command): 0xC0 unlocks the config/registers for write, 0x00 re-locks.
    const REG_CFG9346: u64 = 0x50;
    const CFG9346_UNLOCK: u8 = 0xc0;
    const CFG9346_LOCK: u8 = 0x00;
    /// RMS: Receive packet Max Size (16-bit) — the largest frame the NIC will DMA into a buffer.
    const REG_RMS: u64 = 0xda;
    /// CPlusCmd: the C+ command register (enables the C+ descriptor-ring receive/transmit engine).
    const REG_CPLUSCMD: u64 = 0xe0;
    /// CPlusCmd.PCIDAC (bit 4) — a parallel-PCI "Dual Address Cycle" relic. NET-4q: this is a DEAD bit on
    /// a PCIe 8168; the reference `r8169` never sets it for the 8168/8125 and masks it out of the value it
    /// writes (CPCMD_MASK). On PCIe, a 64-bit (DAC) memory TLP is formed NATIVELY from the descriptor's
    /// `__le64 addr` high dword — PCIDAC does not gate it. Kept only as a rings-up readback WITNESS: it is
    /// EXPECTED to read CLEAR, and that is the correct 64-bit-capable state (NET-4n's "won't latch ⇒
    /// 32-bit" inference was a misread of this no-op bit). See `init_rings`.
    const CPCMD_PCIDAC: u16 = 1 << 4;
    /// NET-4y — r8169's CPCMD_MASK (facts): the ONLY CPlusCmd bits the reference preserves from the
    /// reset value are Normal_mode(bit13) | RxVlan(bit6) | RxChkSum(bit5); `rtl_hw_start` then WRITES
    /// `CPlusCmd = read & CPCMD_MASK` on every bring-up (our previous init only READ the register and
    /// never wrote it, leaving the undocumented reset residue — boot-16 read 0x2021, stray bit0 set —
    /// in place). See `init_rings`.
    const CPCMD_MASK: u16 = (1 << 13) | (1 << 6) | (1 << 5);
    /// RDSAR: Receive Descriptor Start Address (64-bit; low @ 0xE4, high @ 0xE8). 256-byte aligned.
    const REG_RDSAR: u64 = 0xe4;
    /// MTPS: Max Transmit Packet Size (8-bit, units of 128 bytes).
    const REG_MTPS: u64 = 0xec;

    // ── RCR / TCR field values (datasheet-standard bring-up) ──
    /// RCR: accept-all-packets (promiscuous, for bring-up — mirrors the e1000 driver's promiscuous
    /// bring-up), physical-match, multicast, broadcast; MXDMA unlimited.
    const RCR_AAP: u32 = 1 << 0;
    const RCR_APM: u32 = 1 << 1;
    const RCR_AM: u32 = 1 << 2;
    const RCR_AB: u32 = 1 << 3;
    const RCR_MXDMA_UNLIMITED: u32 = 0x7 << 8;
    // NET-4y — the 8168-family RxConfig mode bits (r8169 facts, `rtl_init_rxcfg`): for every 8168
    // mac_version the reference writes RX128_INT_EN(bit15) | RX_MULTI_EN(bit14) | RX_DMA_BURST, and
    // for the modern 8168 family (VER_40..53 — the RTL8111H class on this devkit) ALSO
    // RX_EARLY_OFF(bit11), i.e. early-RX DISABLED. The old 8169-only RX_FIFO_THRESH field (7<<13)
    // does NOT exist on the 8168: bits 15/14 are the two mode bits above and bit13 is a bit r8169
    // never sets on any 8168. Our previous RCR wrote 7<<13 (bit13 set) and NEVER set RX_EARLY_OFF —
    // early-RX left ENABLED is an RX mode in which the NIC begins DMAing a frame before it is fully
    // received, with its own payload-address latching (the boot-32/33 "every payload lands at
    // buffer[0]'s address" signature is that class). See `init_rings`.
    const RCR_RX128_INT_EN: u32 = 1 << 15;
    const RCR_RX_MULTI_EN: u32 = 1 << 14;
    const RCR_RX_EARLY_OFF: u32 = 1 << 11;
    /// TCR: MXDMA unlimited + the standard IEEE inter-frame gap.
    const TCR_MXDMA_UNLIMITED: u32 = 0x7 << 8;
    const TCR_IFG_STD: u32 = 0x3 << 24;
    /// MTPS ~ 7.5 KiB (0x3B × 128) — well above a 1522-byte frame; matches the r8169 default.
    const MTPS_DEFAULT: u8 = 0x3b;

    // ── C+ descriptor (16 bytes). Written by hardware via DMA, so every access is a whole-struct
    //    volatile read/write on a `packed` copy (a field reference would be unaligned UB). ──
    #[repr(C, packed)]
    #[derive(Clone, Copy)]
    struct Desc {
        /// OWN(31) | EOR(30) | FS(29) | LS(28) | … | frame-length[13:0]. For RX the length field is
        /// the buffer size we advertise; hardware overwrites it with the received length on completion.
        opts1: u32,
        /// VLAN / offload flags — unused in this bring-up (0).
        opts2: u32,
        /// Buffer physical address (identity map ⇒ the allocation's virtual pointer). 64-bit.
        addr: u64,
    }
    /// OWN: 1 = owned by the NIC (RX: ready to receive / TX: ready to send); 0 = owned by the host.
    const DESC_OWN: u32 = 1 << 31;
    /// EOR: End Of Ring — set on the last descriptor so the NIC wraps to descriptor 0.
    const DESC_EOR: u32 = 1 << 30;
    /// FS / LS: First / Last Segment (a single-buffer frame sets both). TX only.
    const DESC_FS: u32 = 1 << 29;
    const DESC_LS: u32 = 1 << 28;
    /// Frame-length / buffer-size field, bits [13:0].
    const DESC_LEN_MASK: u32 = 0x3fff;

    /// RX ring depth (each descriptor 16 bytes; the ring base is 256-byte aligned). 32 mirrors the
    /// e1000 driver's depth.
    const NUM_RX: usize = 32;
    /// TX ring depth.
    const NUM_TX: usize = 8;
    /// Per-descriptor buffer size (one full Ethernet frame fits; fits the 14-bit length field).
    const RX_BUF_SIZE: usize = 2048;
    const TX_BUF_SIZE: usize = 2048;

    /// NET-4g: how many leading RX descriptors the window-close dump prints (one line each). Eight
    /// covers the metal signature (rx[0] real, rx[1..] real-length/zero-payload) without flooding.
    /// `UNAOS_NET4_RINGDUMP` widens this to the full ring (NET-4l instrumentation).
    const NET4G_DUMP_N: usize = 8;
    /// NET-4l: bound on the knob-gated per-real-RX full-ring dumps (so a wrong fix still names the
    /// state machine on the first handful of pops without flooding a whole DHCP window).
    const NET4L_AFTERRX_MAX: u64 = 6;
    /// NET-4m: bound on the knob-gated per-pop speculation-fenced buffer-DRAM probe (the discriminator
    /// that names, on the first handful of pops, whether the zero payload is a cache/speculation artifact
    /// or a writes-to-nowhere inbound-DMA reachability gap). Same six-pop reach as the NET-4l dumps.
    const NET4M_PROBE_N: u64 = 6;
    /// NET-4m: leading buffer bytes the probe dumps per pop (two Ethernet MACs' worth — enough to read
    /// dst/src and tell a real L2 header from an all-zero fill without flooding the window).
    const NET4M_PROBE_BYTES: usize = 16;
    /// NET-4z: bound on the knob-gated SCAN-ALL destination witness (fires on every pop, not just the
    /// non-zero slots net4y inspected). Eight pops localize the payload's TRUE landing index without
    /// flooding the window.
    const NET4Z_PROBE_N: u64 = 8;

    // ── NET-4d: RX-window frame classification (the DHCP no-lease RX-side proof) ──
    // Bounds the per-frame serial noise: a full L2/L3/L4 line for the first NET4D_FULL_LINES popped
    // frames, ALWAYS a full line for any BOOTP/DHCP (UDP port 67/68) frame, and a per-category tally
    // for a single window-close summary. Reads frame bytes only; writes no device register.
    const NET4D_FULL_LINES: u64 = 8;
    /// NET-4k: bound on the per-boot socket-originated DHCP TX witness lines (DISCOVER + a handful of
    /// REQUEST retries is the realistic worst case; the bound only guards a pathological retransmit storm).
    const NET4K_TX_WITNESS_MAX: u64 = 16;
    const RXCAT_N: usize = 6;
    const RXCAT_ARP: usize = 0;
    const RXCAT_DHCP: usize = 1;
    const RXCAT_UDP_OTHER: usize = 2;
    const RXCAT_IPV4_OTHER: usize = 3;
    const RXCAT_IPV6: usize = 4;
    const RXCAT_OTHER: usize = 5;
    /// NET-4t: bound on the raw-bytes witness lines for frames classified `other` — the boot-27
    /// evidence question ("what ARE the 8 other frames?") needs only a handful of exemplars.
    const NET4T_OTHER_DUMPS: u64 = 4;

    /// NET-4t: resolve a frame's EFFECTIVE EtherType by peeling up to two 802.1Q/802.1ad VLAN tags
    /// (0x8100 / 0x88a8). Returns `(ethertype, l3_offset, outermost_vlan_id)` — `l3_offset` is where
    /// the L3 header starts (14 untagged, 18/22 tagged), `None` vlan for an untagged frame. The
    /// boot-27 `other=8` bucket motivated this: a classifier reading the EtherType at fixed offset
    /// 12 files ALL VLAN-tagged traffic (including a DHCP OFFER under a VLAN) as `other`.
    /// Read-only and bounds-checked; a frame too short to carry the claimed tag yields the raw tag
    /// TPID as the ethertype (still classified `other`, and the runt shows in the witness dump).
    fn eth_effective_type(frame: &[u8]) -> (u16, usize, Option<u16>) {
        if frame.len() < 14 {
            return (0, 14, None);
        }
        let mut off = 12usize;
        let mut vlan: Option<u16> = None;
        for _ in 0..2 {
            let et = u16::from_be_bytes([frame[off], frame[off + 1]]);
            if (et == 0x8100 || et == 0x88a8) && frame.len() >= off + 6 {
                let tci = u16::from_be_bytes([frame[off + 2], frame[off + 3]]);
                if vlan.is_none() {
                    vlan = Some(tci & 0x0fff);
                }
                off += 4;
            } else {
                return (et, off + 2, vlan);
            }
        }
        let et = u16::from_be_bytes([frame[off], frame[off + 1]]);
        (et, off + 2, vlan)
    }

    /// A decoded IPv4/UDP/BOOTP view of a frame — the fields the DHCP no-lease investigation needs.
    #[derive(Clone, Copy)]
    struct DhcpInfo {
        sport: u16,
        dport: u16,
        sip: [u8; 4],
        dip: [u8; 4],
        /// BOOTP op (1 = BOOTREQUEST from client, 2 = BOOTREPLY from server).
        op: u8,
        /// DHCP message type (option 53): 1 DISCOVER, 2 OFFER, 3 REQUEST, 5 ACK, 6 NAK, … 0 = none.
        mtype: u8,
        /// BOOTP transaction id — the DISCOVER/OFFER correlation the no-lease proof turns on.
        xid: u32,
        /// "your" IP address the server offers (BOOTP yiaddr).
        yiaddr: [u8; 4],
    }

    /// The human name of a DHCP message type (option 53) for the classification lines.
    fn dhcp_mtype_name(t: u8) -> &'static str {
        match t {
            1 => "DISCOVER",
            2 => "OFFER",
            3 => "REQUEST",
            4 => "DECLINE",
            5 => "ACK",
            6 => "NAK",
            7 => "RELEASE",
            8 => "INFORM",
            _ => "none/?",
        }
    }

    /// Decode `frame` as Ethernet/IPv4/UDP/BOOTP, returning the [`DhcpInfo`] view iff it is a UDP frame
    /// on the BOOTP port pair (67/68). READ-ONLY and fully bounds-checked at every step — a malformed or
    /// truncated frame returns `None`, never a panic or an out-of-bounds read. Shared by the TX-time
    /// DISCOVER-xid capture and the RX-window classifier.
    fn decode_dhcp(frame: &[u8]) -> Option<DhcpInfo> {
        if frame.len() < 14 {
            return None;
        }
        // EtherType must be IPv4 (0x0800) — NET-4t: after peeling any VLAN tag, so a DHCP frame
        // arriving 802.1Q-tagged is still recognized as DHCP (it previously fell into `other`).
        let (et, l3_off, _vlan) = eth_effective_type(frame);
        if et != 0x0800 {
            return None;
        }
        let ip = frame.get(l3_off..)?;
        if ip.len() < 20 || (ip[0] >> 4) != 4 {
            return None;
        }
        let ihl = ((ip[0] & 0x0f) as usize) * 4;
        if ihl < 20 || ip.len() < ihl + 8 {
            return None;
        }
        // Protocol 17 = UDP.
        if ip[9] != 17 {
            return None;
        }
        let sip = [ip[12], ip[13], ip[14], ip[15]];
        let dip = [ip[16], ip[17], ip[18], ip[19]];
        let udp = &ip[ihl..];
        let sport = u16::from_be_bytes([udp[0], udp[1]]);
        let dport = u16::from_be_bytes([udp[2], udp[3]]);
        if !matches!(sport, 67 | 68) && !matches!(dport, 67 | 68) {
            return None;
        }
        // BOOTP fixed header (236 bytes) + the 4-byte DHCP magic cookie.
        let bootp = udp.get(8..)?;
        if bootp.len() < 240 {
            return None;
        }
        let op = bootp[0];
        let xid = u32::from_be_bytes([bootp[4], bootp[5], bootp[6], bootp[7]]);
        let yiaddr = [bootp[16], bootp[17], bootp[18], bootp[19]];
        // DHCP message type (option 53) — only if the magic cookie is present and the options parse.
        let mut mtype = 0u8;
        if bootp[236] == 0x63 && bootp[237] == 0x82 && bootp[238] == 0x53 && bootp[239] == 0x63 {
            let opts = &bootp[240..];
            let mut i = 0usize;
            while i < opts.len() {
                let tag = opts[i];
                if tag == 0xff {
                    break; // End option.
                }
                if tag == 0x00 {
                    i += 1; // Pad option (no length byte).
                    continue;
                }
                if i + 1 >= opts.len() {
                    break;
                }
                let l = opts[i + 1] as usize;
                if i + 2 + l > opts.len() {
                    break;
                }
                if tag == 53 && l >= 1 {
                    mtype = opts[i + 2];
                }
                i += 2 + l;
            }
        }
        Some(DhcpInfo { sport, dport, sip, dip, op, mtype, xid, yiaddr })
    }

    /// NET-4j: the exact smoltcp-dhcpv4 accept verdict for an inbound OFFER — the gates smoltcp applies
    /// ABOVE the three the driver checked (xid + dst-MAC + yiaddr-unicast). Read-only, fully bounds-checked.
    /// Reproduced from smoltcp 0.13.1 `iface/interface/ipv4.rs` + `socket/dhcpv4.rs::process`:
    ///   1. IPv4 header checksum verifies (default `ChecksumCapabilities` verify on RX);
    ///   2. UDP checksum verifies (a zero checksum is legal/accepted per RFC 768);
    ///   3. BOOTP chaddr (client hardware address) equals our station MAC;
    ///   4. DHCP option 54 (server identifier) is present — smoltcp DROPS an OFFER without it.
    /// The transaction-id gate (5) is already reported by `net4d_offer_check`. The NET-4j reproducer
    /// (net_phy.rs, witness-gated) proves a frame passing all of these yields a REQUEST; this probe names
    /// the FIRST gate a real metal OFFER fails so a single boot localizes the drop instead of guessing.
    struct SmoltcpGate {
        ipv4_csum_ok: bool,
        udp_csum_ok: bool,
        udp_csum_zero: bool,
        chaddr_ok: bool,
        server_id: Option<[u8; 4]>,
    }

    /// Fold a running ones-complement 16-bit sum and return the (un-complemented) folded value.
    #[inline]
    fn csum_fold(mut sum: u32) -> u16 {
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        sum as u16
    }

    /// Ones-complement 16-bit sum over `data` (big-endian words; a trailing odd byte is high-padded),
    /// added to `initial`. Returns the folded sum; a valid checksum makes the folded sum `0xffff`.
    fn csum_words(data: &[u8], initial: u32) -> u16 {
        let mut sum = initial;
        let mut i = 0;
        while i + 1 < data.len() {
            sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            i += 2;
        }
        if i < data.len() {
            sum += (data[i] as u32) << 8;
        }
        csum_fold(sum)
    }

    /// Compute smoltcp's OFFER accept gates over a raw RX frame (Eth/IPv4/UDP/BOOTP). `None` if the frame
    /// is not a decodable IPv4/UDP/BOOTP frame. `our_mac` is the station MAC (the chaddr comparison).
    fn smoltcp_offer_gate(frame: &[u8], our_mac: &[u8; 6]) -> Option<SmoltcpGate> {
        if frame.len() < 14 || u16::from_be_bytes([frame[12], frame[13]]) != 0x0800 {
            return None;
        }
        let ip = frame.get(14..)?;
        if ip.len() < 20 || (ip[0] >> 4) != 4 {
            return None;
        }
        let ihl = ((ip[0] & 0x0f) as usize) * 4;
        // IPv4 total length bounds the L3 payload — trailing Ethernet FCS/padding is excluded from both
        // checksums (smoltcp bounds by the header/length fields, so we must too).
        let total_len = u16::from_be_bytes([ip[2], ip[3]]) as usize;
        if ihl < 20 || total_len < ihl || ip.len() < total_len || ip[9] != 17 {
            return None;
        }
        let ipv4_csum_ok = csum_fold(csum_words(&ip[..ihl], 0) as u32) == 0xffff;

        let udp = &ip[ihl..total_len];
        if udp.len() < 8 {
            return None;
        }
        let udp_len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
        let udp_csum = u16::from_be_bytes([udp[6], udp[7]]);
        let udp_csum_zero = udp_csum == 0;
        // UDP pseudo-header: src ip (ip[12..16]) + dst ip (ip[16..20]) + zero + proto(17) + udp length.
        let udp_csum_ok = if udp_csum_zero {
            true // RFC 768: a zero transmitted checksum means "not computed" — accepted.
        } else if udp.len() >= udp_len && udp_len >= 8 {
            let mut init = (ip[12] as u32) << 8 | ip[13] as u32;
            init += (ip[14] as u32) << 8 | ip[15] as u32;
            init += (ip[16] as u32) << 8 | ip[17] as u32;
            init += (ip[18] as u32) << 8 | ip[19] as u32;
            init += 17u32; // protocol
            init += udp_len as u32;
            csum_fold(csum_words(&udp[..udp_len], init) as u32) == 0xffff
        } else {
            false
        };

        let bootp = udp.get(8..)?;
        if bootp.len() < 240 {
            return None;
        }
        // chaddr occupies BOOTP bytes 28..44 (16 bytes); the first 6 are the client Ethernet MAC.
        let chaddr_ok = bootp[28..34] == our_mac[..];
        // Scan options for tag 54 (server identifier), the field smoltcp requires and the driver's
        // original probe never inspected.
        let mut server_id: Option<[u8; 4]> = None;
        if bootp[236] == 0x63 && bootp[237] == 0x82 && bootp[238] == 0x53 && bootp[239] == 0x63 {
            let opts = &bootp[240..];
            let mut i = 0usize;
            while i < opts.len() {
                let tag = opts[i];
                if tag == 0xff {
                    break;
                }
                if tag == 0x00 {
                    i += 1;
                    continue;
                }
                if i + 1 >= opts.len() {
                    break;
                }
                let l = opts[i + 1] as usize;
                if i + 2 + l > opts.len() {
                    break;
                }
                if tag == 54 && l == 4 {
                    server_id = Some([opts[i + 2], opts[i + 3], opts[i + 4], opts[i + 5]]);
                }
                i += 2 + l;
            }
        }
        Some(SmoltcpGate { ipv4_csum_ok, udp_csum_ok, udp_csum_zero, chaddr_ok, server_id })
    }

    // ── PCI config-space offsets (in the ECAM, at bus1:dev0:fn0) ──
    const CFG_VENDOR: u64 = 0x00;
    const CFG_COMMAND: u64 = 0x04;
    const CFG_BAR2: u64 = 0x18;
    const CFG_BAR3: u64 = 0x1c;
    /// Command register bits the driver sets so the device's BARs decode and it can master DMA.
    const CMD_MEM_SPACE: u16 = 1 << 1;
    const CMD_BUS_MASTER: u16 = 1 << 2;

    /// Downstream device config base = ECAM base + bus1:dev0:fn0 offset (`bus<<20 | dev<<15 | fn<<12`).
    const BUS1_DEV0_FN0: u64 = 1 << 20;

    /// The claimed NIC: its register-BAR MMIO base, the station MAC, and the C+ RX/TX descriptor
    /// rings + DMA buffers. The rings are allocated from the kernel heap (identity map ⇒ the pointer
    /// doubles as the physical address the NIC DMAs against, exactly like the x86 e1000 driver).
    pub struct Rtl8168 {
        mmio_base: u64,
        mac: [u8; 6],
        /// NET-4r: controller-0's DWC iATU register block base. The inbound-iATU ALIAS regions (which
        /// catch the NIC's TRUNCATED ring/payload writes and up-translate them to the real high-heap PAs)
        /// are armed from `init_rings` — the only place the buffer PAs are known — so the driver keeps the
        /// ATU base to reach the inbound region slots after allocation.
        atu_base: u64,
        /// NET-4r: the NET-4h identity inbound region `[dma_ident_lo, dma_ident_hi)` (all of RAM). Used to
        /// SKIP arming an alias for any block already sub-4 GiB inside it (the identity region reaches it
        /// untranslated); the enumeration also reports index 0 as the identity slot.
        dma_ident_lo: u64,
        dma_ident_hi: u64,
        /// NET-4r: controller-0's PCIe MEM `ranges` window `[mem_lo, mem_hi)` — where downstream BARs
        /// decode. The alias collision check REFUSES any alias window overlapping it (an inbound write into
        /// that range risks peer-to-peer routing to a BAR instead of upstream to the RC).
        mem_lo: u64,
        mem_hi: u64,
        /// NET-4B: base PA of the Normal-NC DMA window (`mmu_tegra::net4b_nc_window`) the rings + buffers
        /// are laid into. `0` until `init_rings` maps + claims it; the rings/buffers below are sub-slices
        /// of `[nc_base, nc_base + NET4B_NC_SIZE)`, so they are all Non-Cacheable and need no cache
        /// maintenance — only the `dma_wmb`/`dma_rmb` ordering barriers on the OWN protocol.
        nc_base: u64,
        rx_ring: *mut Desc,
        rx_buffers: *mut u8,
        rx_cur: usize,
        rx_count: u64,
        tx_ring: *mut Desc,
        tx_buffers: *mut u8,
        tx_cur: usize,
        tx_count: u64,
        /// NET-4c: TX descriptors the NIC never handed back (OWN stayed set) — the
        /// did-a-frame-ever-LEAVE-the-NIC counter for the DHCP no-lease investigation.
        tx_stalled: u64,
        /// NET-4d: the DHCP DISCOVER's BOOTP transaction id, captured at TX time, so RX-side DHCP
        /// frames can be matched/mismatched against it explicitly. `None` until the DISCOVER is sent.
        d_xid: Option<u32>,
        /// NET-4d: RX frames given a full classification line so far — bounds the per-frame noise to
        /// the first `NET4D_FULL_LINES` popped frames (BOOTP/DHCP frames always print regardless).
        rxcls_full: u64,
        /// NET-4d: per-category RX frame tallies for the single window-close summary.
        rxcat: [u64; RXCAT_N],
        /// NET-4d: classification is live only across the DHCP discover window; closed at window end
        /// so the post-window bounded ICMP poll does not re-classify.
        rxcls_active: bool,
        /// NET-4k: how many socket-originated DHCP frames we have witnessed on TX so far — bounds the
        /// TX-type witness noise to `NET4K_TX_WITNESS_MAX` (DHCP TX is inherently low-volume anyway).
        dhcp_tx_witnessed: u64,
        /// NET-4t: `other`-class frames given a raw-bytes witness dump so far — bounds the boot-27
        /// "what ARE the other frames" evidence lines to `NET4T_OTHER_DUMPS`.
        net4t_other_dumped: u64,
        /// NET-4y: buffer[0] cross-witness lines emitted so far (bounded to the first 3 pops on a
        /// slot other than 0 — the "is the NIC writing EVERY payload to buffer[0]" discriminator).
        net4y_probes: u64,
        /// NET-4z: SCAN-ALL destination-witness lines emitted so far (bounded to NET4Z_PROBE_N pops).
        /// Where net4y inspected ONLY buffer[0], net4z scans the whole ring and names the exact buffer
        /// index each payload actually landed in — the discriminator between "arena base (index 0)" and
        /// "some other wrong index", and the proof it is not the completed slot's own buffer.
        net4z_probes: u64,
        /// NET-4A: cross-pop mechanism witness state. `net4z` reports a per-pop landing index; net4A
        /// correlates them ACROSS pops to prove the boot-36 signature as a single verdict — the payload
        /// landing index STAYS CONSTANT (stuck at the last-fetched descriptor's buffer) while the
        /// completed slot ADVANCES, for ≥4 consecutive pops. `net4A_prev_land`/`net4A_prev_slot` are the
        /// previous pop's landing index and completed slot (−2 = none yet); `net4A_run` counts
        /// consecutive stuck-and-advancing PAIRS; `net4A_fired` one-shots the verdict line.
        net4a_prev_land: i64,
        net4a_prev_slot: i64,
        net4a_run: u64,
        net4a_fired: bool,
    }

    // The driver owns raw DMA pointers; on the single-CPU main-loop/poll discipline it is only ever
    // touched behind the `NET4_DEVICE` mutex, so sharing across contexts is sound.
    unsafe impl Send for Rtl8168 {}

    impl Rtl8168 {
        #[inline]
        fn r8(&self, off: u64) -> u8 {
            unsafe { read_volatile((self.mmio_base + off) as *const u8) }
        }
        #[inline]
        fn w8(&self, off: u64, v: u8) {
            unsafe { write_volatile((self.mmio_base + off) as *mut u8, v) }
        }
        #[inline]
        fn w16(&self, off: u64, v: u16) {
            unsafe { write_volatile((self.mmio_base + off) as *mut u16, v) }
        }
        #[inline]
        fn r32(&self, off: u64) -> u32 {
            unsafe { read_volatile((self.mmio_base + off) as *const u32) }
        }
        #[inline]
        fn w32(&self, off: u64, v: u32) {
            unsafe { write_volatile((self.mmio_base + off) as *mut u32, v) }
        }
        #[inline]
        fn r16(&self, off: u64) -> u16 {
            unsafe { read_volatile((self.mmio_base + off) as *const u16) }
        }

        /// Soft-reset the MAC: set CR.RST and poll (finite backstop) until the controller clears it.
        /// Returns true if the reset completed. Announced before the write (it is a register write).
        fn soft_reset(&self) -> bool {
            serial_println!("{}   >>> REG WRITE (M1): CR[{:#x}] |= RST (soft reset) — issuing ::", P4, REG_CR);
            self.w8(REG_CR, CR_RST);
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
            // RST self-clears when the reset completes; ~1M spins is a generous ceiling (sub-ms on HW).
            const MAX_SPINS: u32 = 1_000_000;
            let mut spins = 0u32;
            while spins < MAX_SPINS {
                if self.r8(REG_CR) & CR_RST == 0 {
                    serial_println!("{}   CR.RST cleared after {} spins — reset complete ::", P4, spins);
                    return true;
                }
                core::hint::spin_loop();
                spins += 1;
            }
            serial_println!("{}   CR.RST STILL set after {} spins — reset did not complete (honest HW result) ::", P4, MAX_SPINS);
            false
        }

        /// Poison-honest liveness probe through the (freshly mapped) register window, done BEFORE any
        /// write — the M3 guard transposed from the V3D-2 lesson: every new MMIO window gets a probe
        /// read before its first write. Reads TCR (0x40), whose chip-version bits ([30:23]) are a
        /// stable RO datum a live RTL8168 always returns (`r8169` reads exactly this to identify the
        /// MAC), and rejects the poison fills — open-bus `0xffffffff`, firmware `0xdeadbeef`, and the
        /// carveout `0xa5a5a5a5` that the M1 metal FAULT left. Returns the value on a live decode, or
        /// `None` on absent decode (so the caller REFUSES rather than issuing the first register write
        /// blind — the fault-at-first-write can never recur). This read is safe: it targets the CPU
        /// aperture the outbound iATU forwards to PCIe, so a mistranslation/link-down returns UR
        /// (all-ones), never a carveout — unlike the raw-BAR deref this fix retired.
        fn probe_alive(&self) -> Option<u32> {
            let tcr = self.r32(REG_TCR);
            if is_poison(tcr) {
                None
            } else {
                Some(tcr)
            }
        }

        /// Read the six station-MAC bytes from IDR0..5 (the RTL8168 loads them from its EEPROM/eFuse at
        /// reset). Reads are byte-wide (the ID registers are a byte array).
        fn read_mac(&self) -> [u8; 6] {
            let mut mac = [0u8; 6];
            for (i, b) in mac.iter_mut().enumerate() {
                *b = self.r8(REG_IDR0 + i as u64);
            }
            mac
        }

        // NET-4B: fixed 64 KiB-aligned offsets of each DMA object inside the 2 MiB Normal-NC window
        // (`nc_base`). 64 KiB spacing keeps every object 64 KiB-aligned OUTRIGHT (NET-4r / L4T-FACTS
        // §A2/§A3: the DWC inbound-iATU hardwires BASE/TARGET low-16 to 0, LIMIT low-16 to 1), so the
        // covering-alias math in `arm_dma_aliases` reads back exactly what it wrote. Objects: rx-ring
        // (512 B), rx-buffers (64 KiB), tx-ring (128 B), tx-buffers (16 KiB) — top 0x34000 < 2 MiB.
        const NC_OFF_RX_RING: u64 = 0x0_0000;
        const NC_OFF_RX_BUFS: u64 = 0x1_0000;
        const NC_OFF_TX_RING: u64 = 0x2_0000;
        const NC_OFF_TX_BUFS: u64 = 0x3_0000;

        /// Lay the RX descriptor ring + contiguous packet buffers into the Normal-NC DMA window and point
        /// each descriptor at its buffer with OWN set (ready for the NIC to fill) and the buffer size in
        /// the length field. The NC-window PA doubles as the physical address (identity map), matching the
        /// x86 e1000 ring allocation. EOR marks the last descriptor. NC memory ⇒ plain volatile stores
        /// only (no cache maintenance, and no `ldxr/stxr` — the Desc is written whole, never atomically).
        fn alloc_rx(&mut self) {
            self.rx_ring = (self.nc_base + Self::NC_OFF_RX_RING) as *mut Desc;
            self.rx_buffers = (self.nc_base + Self::NC_OFF_RX_BUFS) as *mut u8;
            // The window is fresh DRAM the kernel never wrote; zero the RX buffer block so the witnesses
            // read a known baseline (a plain NC memset — no cache line to clean, and no `dc ivac` needed:
            // the NIC's DMA lands straight in this same uncached DRAM the CPU reads).
            unsafe { core::ptr::write_bytes(self.rx_buffers, 0, NUM_RX * RX_BUF_SIZE) };
            for i in 0..NUM_RX {
                let buf_phys = (self.rx_buffers as u64) + (i * RX_BUF_SIZE) as u64;
                let eor = if i == NUM_RX - 1 { DESC_EOR } else { 0 };
                let d = Desc {
                    opts1: DESC_OWN | eor | (RX_BUF_SIZE as u32 & DESC_LEN_MASK),
                    opts2: 0,
                    addr: buf_phys,
                };
                unsafe { write_volatile(self.rx_ring.add(i), d) };
            }
        }

        /// Lay the TX descriptor ring + frame buffers into the Normal-NC DMA window. Descriptors start
        /// host-owned (OWN clear) so `transmit` can post into them; EOR marks the last descriptor.
        fn alloc_tx(&mut self) {
            self.tx_ring = (self.nc_base + Self::NC_OFF_TX_RING) as *mut Desc;
            self.tx_buffers = (self.nc_base + Self::NC_OFF_TX_BUFS) as *mut u8;
            unsafe { core::ptr::write_bytes(self.tx_buffers, 0, NUM_TX * TX_BUF_SIZE) };
            for i in 0..NUM_TX {
                let eor = if i == NUM_TX - 1 { DESC_EOR } else { 0 };
                let d = Desc { opts1: eor, opts2: 0, addr: 0 };
                unsafe { write_volatile(self.tx_ring.add(i), d) };
            }
        }

        /// Bring up the C+ descriptor-ring engine after the M1 soft reset: unlock the config
        /// registers, allocate + program the RX/TX rings, set the packet-size / DMA-burst / RX-filter
        /// configuration, enable RX+TX, re-lock the config, and mask interrupts (polled bring-up).
        /// The register-write ORDER follows the RTL8168 programming guide / Linux `r8169` `rtl_hw_start`.
        /// Returns true if a poison-honest readback confirms the device is still answering. Every
        /// register write is announced before issue (they are fabric-visible controller writes).
        /// NET-4r — arm the inbound-iATU ALIAS regions that catch the NIC's TRUNCATED ring/payload writes
        /// and up-translate them to the real (high-heap) PAs. Boot-24 proved the full-64 descriptor path
        /// correct yet the payload still sank (0x200 RAS) — this RC integration truncates the >4 GiB DMA
        /// TLP, so the alias is NET-4q's sanctioned delivery mechanism. Covers BOTH the ring and buffer
        /// blocks of each direction: boot-24 could not prove the ring rides un-truncated on THIS silicon,
        /// so cover it too (a genuinely-full-64 ring TLP simply hits the NET-4h identity region and the
        /// truncation-keyed alias never matches — harmless). Each block that needs an alias takes a FREE
        /// inbound index (index 0 = the NET-4h identity region). Returns false (fail-closed) on: an
        /// alias/target non-congruence, a 4 GiB-crossing window, a PCIe MEM/BAR overlap, an alias-vs-alias
        /// overlap, no free index, or a readback/enable failure inside `program_inbound_alias`.
        fn arm_dma_aliases(&self) -> bool {
            // NET-4s — boot-25 falsified the one-alias-per-block plan: with an index PER block (rx-ring at
            // index 1, rx-buffers at index 2, …) index 1 armed AND readback-proved on silicon while index 2
            // NEVER latched (all-zero after 1000 polls). Verdict: the Tegra234 DWC RC implements FEWER inbound
            // windows than the 8 CSR blocks a CTRL2-read enumeration sees — NET-4p's "TX region shadowed" was
            // never a shadow, those indices simply do not exist. So (1) DISCOVER the true window count by a
            // WRITABILITY probe (§A5, not a CTRL2 read), and (2) CONSOLIDATE every block that needs aliasing
            // into ONE covering region at the proven-armable index 1. The four DMA blocks share one heap
            // neighborhood (boot-25: 0x2683c…-0x2683f…), so a single 64 KiB-aligned window whose fixed
            // up-translate offset is the shared high dword << 32 covers them all — one region, offset-exact
            // for each block (§A1: translated = target + (incoming − base); with base/target sharing the low
            // 32 bits the offset is exactly high_dword<<32 for every block).
            let (num_ib, enabled) = probe_inbound_windows(self.atu_base);
            // The four DMA blocks the NIC touches, each (pa, len, tag). Rings + buffers are distinct
            // 64 KiB-aligned allocations (alloc_rx/alloc_tx).
            let blocks: [(u64, u64, &str); 4] = [
                (self.rx_ring as u64, (NUM_RX * core::mem::size_of::<Desc>()) as u64, "rx-ring"),
                (self.rx_buffers as u64, (NUM_RX * RX_BUF_SIZE) as u64, "rx-buffers"),
                (self.tx_ring as u64, (NUM_TX * core::mem::size_of::<Desc>()) as u64, "tx-ring"),
                (self.tx_buffers as u64, (NUM_TX * TX_BUF_SIZE) as u64, "tx-buffers"),
            ];
            // Partition the blocks: which NEED an alias (their PA truncates on the wire / sits outside the
            // identity region) and which the NET-4h identity region already reaches untranslated. Accumulate
            // the covering window (in truncated low-32 space) over the blocks that need one, and require they
            // all share ONE high dword (the single covering region up-translates by ONE fixed offset).
            let mut need_hi: Option<u64> = None; // the shared high dword every aliased block must carry
            let mut cover_lo = u64::MAX; // covering window base (low-32 space)
            let mut cover_hi = 0u64; // covering window end (low-32 space, exclusive, pre-round)
            let mut n_need = 0usize;
            for &(pa, len, tag) in blocks.iter() {
                let alias = pa & 0xFFFF_FFFF;
                // Case A — already sub-4 GiB and inside the identity region: the truncation is a no-op and
                // NET-4h's identity region already reaches it. No alias needed.
                if pa == alias && alias >= self.dma_ident_lo && alias + len <= self.dma_ident_hi {
                    serial_println!(
                        "{}   [net4s] {} [{:#x}..{:#x}) inside the identity inbound region [{:#x}..{:#x}) — identity-covered, no alias ::",
                        P4, tag, pa, pa + len, self.dma_ident_lo, self.dma_ident_hi
                    );
                    continue;
                }
                // Needs aliasing. Every aliased block must share one high dword — a single covering region can
                // only apply ONE up-translate offset. (If the heap ever scatters the blocks across a 4 GiB
                // boundary, this refuses rather than silently mistranslate; the remedy is a re-seated arena.)
                let hi = pa >> 32;
                match need_hi {
                    None => need_hi = Some(hi),
                    Some(h) if h != hi => {
                        serial_println!(
                            "{}   !! [net4s] {} high dword {:#x} != {:#x} of the other aliased block(s) — one covering region cannot up-translate both; REFUSING (re-seat the DMA arena into a single 4 GiB-congruent neighborhood) ::",
                            P4, tag, hi, h
                        );
                        return false;
                    }
                    _ => {}
                }
                serial_println!(
                    "{}   [net4s] {} [{:#x}..{:#x}) needs alias: truncated bus [{:#x}..{:#x}), high dword {:#x} ::",
                    P4, tag, pa, pa + len, alias, alias + len, hi
                );
                if alias < cover_lo {
                    cover_lo = alias;
                }
                if alias + len > cover_hi {
                    cover_hi = alias + len;
                }
                n_need += 1;
            }
            if n_need == 0 {
                serial_println!(
                    "{}   [net4s] all four DMA blocks are identity-covered — no inbound alias needed ::",
                    P4
                );
                return true;
            }
            let hi = need_hi.unwrap();
            // ONE covering window over every aliased block. The allocations are 64 KiB-aligned, so cover_lo is
            // already aligned (floor defensively); the end rounds UP to the controller's 64 KiB granularity
            // (§A2). base ≡ target (mod 64 KiB) by construction — both carry the same low 32 bits, both
            // 64 KiB-aligned — so the controller rounds nothing (§A3) and the region reads back exactly.
            let alias_base = cover_lo & !0xFFFFu64;
            let region_hi = (cover_hi + 0xFFFF) & !0xFFFFu64; // 64 KiB-rounded window end (exclusive)
            let size = region_hi - alias_base;
            let target = (hi << 32) | alias_base;
            let cong = (alias_base & 0xFFFF) == (target & 0xFFFF);
            let aligned = (alias_base & 0xFFFF) == 0;
            serial_println!(
                "{}   [net4s] covering window: base {:#x} limit {:#x} target {:#x} (up-translate +{:#x}); spans {} aliased block(s); congruent(mod 64KiB)={} 64KiB-aligned={} ::",
                P4, alias_base, region_hi - 1, target, target - alias_base, n_need, cong as u8, aligned as u8
            );
            if !cong || !aligned {
                serial_println!(
                    "{}   !! [net4s] covering window base/target NOT 64 KiB-congruent — would mistranslate by the low-16 delta; REFUSING ::",
                    P4
                );
                return false;
            }
            // 4 GiB-crossing guard — one region cannot wrap the boundary.
            if alias_base.checked_add(size).map_or(true, |e| e > 0x1_0000_0000) {
                serial_println!(
                    "{}   !! [net4s] covering window [{:#x}..+{:#x}) crosses the 4 GiB boundary — one region cannot cover it; REFUSING (re-seat the DMA arena) ::",
                    P4, alias_base, size
                );
                return false;
            }
            // PCIe MEM/BAR overlap (peer-to-peer mis-route hazard).
            if self.mem_hi > self.mem_lo && alias_base < self.mem_hi && self.mem_lo < region_hi {
                serial_println!(
                    "{}   !! [net4s] covering window [{:#x}..{:#x}) OVERLAPS the PCIe MEM/BAR window [{:#x}..{:#x}) — REFUSING (peer-to-peer mis-route hazard) ::",
                    P4, alias_base, region_hi, self.mem_lo, self.mem_hi
                );
                return false;
            }
            // Take the FIRST free IMPLEMENTED inbound index (skip enabled slots incl. index 0 identity; the
            // index must be within the discovered window count — boot-25 proved index 1 is implemented and
            // index 2 is NOT, so demanding four separate indices could never succeed on this silicon).
            let mut idx = 1u64;
            while idx < num_ib && (enabled & (1u32 << idx)) != 0 {
                idx += 1;
            }
            if idx >= num_ib {
                serial_println!(
                    "{}   !! [net4s] no FREE implemented inbound iATU index (count={}, enabled mask={:#06x}) — REFUSING ::",
                    P4, num_ib, enabled
                );
                return false;
            }
            serial_println!(
                "{}   [net4s] consolidating all {} aliased block(s) into ONE covering region at inbound index {} (of {} implemented) ::",
                P4, n_need, idx, num_ib
            );
            if !program_inbound_alias(self.atu_base, idx, alias_base, size, target, "dma-cover") {
                serial_println!(
                    "{}   !! [net4s] covering alias arm at index {} FAILED its readback/enable proof — REFUSING ::",
                    P4, idx
                );
                return false;
            }
            serial_println!(
                "{}   [net4s] 1 covering inbound alias region armed + readback-proven at index {}; all {} DMA block(s) (RX/TX ring AND buffers) covered by one window ::",
                P4, idx, n_need
            );
            // NET-4v — the coverage witness: prove, from an INDEPENDENT register re-read (not
            // program_inbound_alias's own readback), that the armed window spans every DMA object.
            // Fail-closed on any MISS.
            self.witness_dma_coverage(idx, &blocks)
        }

        /// NET-4v — per-object coverage witness. Re-reads the armed inbound region `idx`'s
        /// BASE/LIMIT/TARGET straight from the CSRs (readback-proving the LIMIT pair the same way
        /// NET-4r proved the rest, but from a fresh read this arc owns), then checks every DMA object's
        /// REQUIRED bus span — computed from the actual allocation (pa truncated to low-32, length from
        /// the live ring/buffer geometry), never hardcoded — against the INCLUSIVE `[base, limit]`
        /// match window (§A0: LIMIT = end − 1, inclusive; §A2: 64 KiB granularity). An object already
        /// inside the NET-4h identity region needs no alias and is reported as such. One line per
        /// object plus the verdict line `covers=ALL` / `covers=MISS(<obj>)`; a MISS returns false so
        /// the bring-up fails CLOSED rather than DMA into a sink. Boot-30 audit note: the armed window
        /// [0x683d0000..0x6840ffff] already covers the required top 0x68403fff — this witness is the
        /// standing on-silicon proof of that (the rx[1..] sink is below the iATU).
        fn witness_dma_coverage(&self, idx: u64, blocks: &[(u64, u64, &str)]) -> bool {
            let region = self.atu_base + ATU_INBOUND_DIR_OFF + idx * ATU_REGION_STRIDE;
            let r = |off: u64| -> u32 { unsafe { read_volatile((region + off) as *const u32) } };
            let rb_base = ((r(ATU_UNR_UPPER_BASE) as u64) << 32) | r(ATU_UNR_LOWER_BASE) as u64;
            let rb_limit = ((r(ATU_UNR_UPPER_LIMIT) as u64) << 32) | r(ATU_UNR_LOWER_LIMIT) as u64;
            let rb_target = ((r(ATU_UNR_UPPER_TARGET) as u64) << 32) | r(ATU_UNR_LOWER_TARGET) as u64;
            let ctrl2 = r(ATU_UNR_REGION_CTRL2);
            serial_println!(
                "{}   [net4v] armed region {} re-read: BASE={:#x} LIMIT={:#x} (inclusive) TARGET={:#x} CTRL2={:#010x} enabled={} span={:#x} ::",
                P4, idx, rb_base, rb_limit, rb_target, ctrl2, (ctrl2 >> 31) & 1,
                rb_limit.wrapping_sub(rb_base).wrapping_add(1)
            );
            let mut miss: Option<&str> = None;
            for &(pa, len, tag) in blocks.iter() {
                let alias = pa & 0xFFFF_FFFF;
                let identity =
                    pa == alias && alias >= self.dma_ident_lo && alias + len <= self.dma_ident_hi;
                // Inclusive end of the object's required bus span.
                let need_end = alias + len - 1;
                let covered = identity || (alias >= rb_base && need_end <= rb_limit);
                serial_println!(
                    "{}   [net4v] {} pa [{:#x}..{:#x}) requires bus [{:#x}..{:#x}] : {} ::",
                    P4, tag, pa, pa + len, alias, need_end,
                    if identity { "identity-covered" } else if covered { "covered" } else { "NOT COVERED" }
                );
                if !covered && miss.is_none() {
                    miss = Some(tag);
                }
            }
            match miss {
                None => {
                    serial_println!("{}   [net4v] verdict: covers=ALL ::", P4);
                    true
                }
                Some(tag) => {
                    serial_println!(
                        "{}   !! [net4v] verdict: covers=MISS({}) — armed window [{:#x}..{:#x}] does not span it; REFUSING (fail-closed) ::",
                        P4, tag, rb_base, rb_limit
                    );
                    false
                }
            }
        }

        fn init_rings(&mut self) -> bool {
            serial_println!("{}   M2 ring bring-up (C+ mode; RTL8168 programming-guide order) ::", P4);
            // Unlock the config/registers for write (93C46 command = 0xC0).
            serial_println!("{}   >>> REG WRITE (M2): CFG9346[{:#x}] = {:#04x} (unlock config) ::", P4, REG_CFG9346, CFG9346_UNLOCK);
            self.w8(REG_CFG9346, CFG9346_UNLOCK);

            // NET-4y — C+ command register: r8169's `rtl_hw_start` WRITES CPlusCmd on every bring-up
            // (`RTL_W16(tp, CPlusCmd, tp->cp_cmd)` where cp_cmd = reset value & CPCMD_MASK — probe-time
            // capture, mask = Normal_mode|RxVlan|RxChkSum). Our previous init only READ the register and
            // "preserved" whatever reset residue it held (boot-16: 0x2021 — a stray bit0 the mask exists
            // to clear; on the 8139C+ heritage encoding bits 1:0 are the CpRx/CpTx mode bits, and an
            // undefined residue there is exactly the kind of state the reference scrubs). Write the
            // masked value, matching the reference line for line.
            let cpc = self.r16(REG_CPLUSCMD);
            let cpc_w = cpc & CPCMD_MASK;
            serial_println!(
                "{}   >>> REG WRITE (M2): CPlusCmd[{:#x}] {:#06x} -> {:#06x} (& CPCMD_MASK — r8169 rtl_hw_start write; NET-4y) ::",
                P4, REG_CPLUSCMD, cpc, cpc_w
            );
            self.w16(REG_CPLUSCMD, cpc_w);

            // NET-4q — the RTL8168 is 64-bit-DMA capable via the descriptor `addr` field ALONE; do NOT
            // gate 64-bit payload DMA on CPlusCmd.PCIDAC. Per the L4T facts (r8169_main.c): PCIDAC (C+CR
            // bit 4) is a parallel-PCI "Dual Address Cycle" relic that the reference driver NEVER sets for
            // a PCIe 8168/8125 and in fact masks OUT of the value it writes (CPCMD_MASK); on a PCIe part a
            // 64-bit (DAC) memory TLP is formed NATIVELY from the descriptor's `__le64 addr` high dword,
            // not from any C+ enable bit. So our boots-16..18 observation "PCIDAC won't latch" is EXPECTED
            // and HARMLESS — a no-op on this silicon — NOT evidence of a 32-bit-only payload path. The
            // NET-4n (PCIDAC-is-the-64-bit-enable) -> NET-4o (sub-4 GiB arena) -> NET-4p (inbound-iATU
            // alias) chain was built on that misread bit and is SUPERSEDED. We preserve the reset-default
            // CplusCmd (it already selects the C+ engine) and only read it back below as a standing witness
            // that PCIDAC stays clear (the correct reading). The full 64-bit buffer PA already rides in
            // every descriptor's `addr` (u64, both dwords published — net4g [MATCH] on metal); the ring
            // bases ride the dedicated 64-bit RDSAR/TNPDS pair (High-before-Low, below). region-0 identity
            // `[0x8000_0000, 0x2_8000_0000)` (NET-4h) already covers the high buffer PAs (~9.6 GiB), so the
            // natively-formed DAC TLP lands with no alias.

            // NET-4B — map + claim the Normal-NC DMA window, then lay the rings + buffers into it. This is
            // the do-it-right move after NET-4A: match Linux r8169's dma_alloc_coherent (non-cacheable)
            // config instead of out-guessing the below-lane NIC/RC descriptor-address-reuse defect on
            // CACHEABLE memory. `install_net4b_nc` flips the reserved window (carved by `select_heap_region`
            // just below the heap, in the same clean + DMA-reachable span) to Normal-NC in both live tables;
            // refuse the bring-up CLOSED if it is unavailable rather than DMA into cacheable/unmapped DRAM.
            if !install_net4b_nc() {
                serial_println!(
                    "{}   !! NET-4B: Normal-NC DMA window could not be mapped — REFUSING ring bring-up (would DMA into cacheable memory, re-arming the maintenance-vs-fetch race NET-4B exists to remove) ::",
                    P4
                );
                return false;
            }
            let (nc_base, nc_size) = net4b_nc_window();
            self.nc_base = nc_base;
            serial_println!(
                "{}   [net4B] rings + buffers in Normal-NC window [{:#x}, {:#x}) (MAIR AttrIdx 2); rx-ring @ +0x0, rx-bufs @ +0x10000, tx-ring @ +0x20000, tx-bufs @ +0x30000 — no cache maintenance, dma_wmb/dma_rmb ordering only ::",
                P4, nc_base, nc_base + nc_size
            );
            // Lay the descriptor rings + buffers into the NC window (64 KiB-aligned sub-slices). The NC PA
            // doubles as the physical address (identity map); region-0 identity (NET-4h) reaches them.
            self.alloc_rx();
            self.alloc_tx();

            // NET-4r — the SANCTIONED FALLBACK. Boot-24 proved the NET-4q full-64 descriptor path correct
            // (RINGDUMP addr=0x2683cb… [MATCH], hi-before-lo ring bases) yet the payload STILL sank at the
            // 0x200 IOB FillWrite RAS with no lease: the first real evidence this RTL8168/Tegra-RC
            // integration truncates the >4 GiB DMA TLP. So re-arm the inbound-iATU ALIAS as the delivery
            // mechanism — correctly this time (L4T-FACTS §A0-A5): a FREE inbound index per block, UPPER_BASE
            // + UPPER_TARGET written AND read back (the UPPER_TARGET=0x2 dword boot-20 lost), base ≡ target
            // (mod 64 KiB) with both 64 KiB-aligned outright, CTRL2=ENABLE polled, covering BOTH the ring
            // and buffer blocks. Armed AFTER the (high-heap) PAs are known and BEFORE RxEnb/TxEnb below, so
            // the FIRST DMA is already covered. Fail the bring-up CLOSED rather than DMA into a sink/mis-route.
            if !self.arm_dma_aliases() {
                serial_println!(
                    "{}   !! NET-4r: inbound-iATU alias could not be armed + readback-proven — REFUSING ring bring-up rather than DMA into an unreachable/mis-routed surface ::",
                    P4
                );
                return false;
            }

            // NET-4B — the rings now live in Normal-NC memory: `alloc_rx`/`alloc_tx`'s plain volatile
            // stores go straight to DRAM (no cache to clean), and the NIC's descriptor-fetch engine reads
            // that same uncached DRAM. Publish them ahead of RDSAR/TNPDS + RxEnb/TxEnb with one write-side
            // ordering barrier (Linux r8169's dma_wmb; NC still reorders on aarch64). No `dc cvac`.
            dma_wmb();
            // NET-4x WITNESS (retained) — prove DRAM holds real buffer addresses pre-enable. On NC memory
            // the read is a direct DRAM read (no invalidate needed): a zero here would mean the store never
            // landed. Kept as the standing on-boot proof of the NC config.
            {
                let d1 = unsafe { self.rx_ring.add(1) };
                let d17 = unsafe { self.rx_ring.add(17) };
                let a1 = unsafe { read_volatile(d1) }.addr;
                let a17 = unsafe { read_volatile(d17) }.addr;
                let e1 = (self.rx_buffers as u64) + RX_BUF_SIZE as u64;
                let e17 = (self.rx_buffers as u64) + (17 * RX_BUF_SIZE) as u64;
                serial_println!(
                    "{}   [net4x] init witness (NC direct DRAM read): rx-desc[1].addr={:#x} expect={:#x} [{}] rx-desc[17].addr={:#x} expect={:#x} [{}] — DRAM holds the programmed buffer addresses pre-enable ::",
                    P4, a1, e1, if a1 == e1 { "MATCH" } else { "MISMATCH" },
                    a17, e17, if a17 == e17 { "MATCH" } else { "MISMATCH" }
                );
            }

            let rx_phys = self.rx_ring as u64;
            let tx_phys = self.tx_ring as u64;
            // NET-4q — program the descriptor-ring bases as full 64-bit hi/lo pairs, HIGH dword FIRST.
            // r8169 writes RxDescAddrHigh / TxDescStartAddrHigh BEFORE the matching Low as a documented
            // erratum ordering (some 8168 variants latch the ring base only when the low dword lands with
            // the high already in place); our prior code wrote Low-then-High, the reverse of the documented
            // sequence. Emit High first so the full 64-bit base is latched from the NIC's view.
            serial_println!("{}   >>> REG WRITE (M2): RDSAR[{:#x}] = {:#x} (RX ring, {} desc; hi-before-lo) ::", P4, REG_RDSAR, rx_phys, NUM_RX);
            self.w32(REG_RDSAR + 4, (rx_phys >> 32) as u32);
            self.w32(REG_RDSAR, rx_phys as u32);
            serial_println!("{}   >>> REG WRITE (M2): TNPDS[{:#x}] = {:#x} (TX ring, {} desc; hi-before-lo) ::", P4, REG_TNPDS, tx_phys, NUM_TX);
            self.w32(REG_TNPDS + 4, (tx_phys >> 32) as u32);
            self.w32(REG_TNPDS, tx_phys as u32);

            // Receive max size + max TX packet size.
            serial_println!("{}   >>> REG WRITE (M2): RMS[{:#x}] = {:#06x}; MTPS[{:#x}] = {:#04x} ::", P4, REG_RMS, RX_BUF_SIZE as u16, REG_MTPS, MTPS_DEFAULT);
            self.w16(REG_RMS, RX_BUF_SIZE as u16);
            self.w8(REG_MTPS, MTPS_DEFAULT);

            // Publish the ring descriptors before the engine starts fetching them.
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };

            // Enable RX + TX in the ChipCmd register.
            // ORIN-X200-1: stamp the NIC's FIRST DMA-arming point (RxEnb|TxEnb — descriptor
            // fetch/writeback can begin from here) so the next metal boot can temporally separate
            // the two 0x200-RAS candidates (twin stamp: xusb_tegra's ":: X200: xhci RS=1 ...").
            serial_println!(
                "{}   :: X200: rtl8168 first-DMA-arm (RxEnb|TxEnb) t={} (cntpct) ::",
                P4,
                crate::arch::aarch64::timer::cntpct()
            );
            serial_println!("{}   >>> REG WRITE (M2): CR[{:#x}] = {:#04x} (RxEnb | TxEnb) ::", P4, REG_CR, CR_RE | CR_TE);
            self.w8(REG_CR, CR_RE | CR_TE);

            // NET-4y — RxConfig, the 8168 value + the reference order. r8169's `rtl_hw_start` writes
            // ChipCmd(RxEnb|TxEnb) FIRST, then RxConfig (`rtl_init_rxcfg`), then TxConfig — our old
            // sequence wrote TCR before CR (benign but divergent; now aligned). The VALUE is the real
            // fix: the 8168 family takes RX128_INT_EN | RX_MULTI_EN | RX_DMA_BURST | RX_EARLY_OFF —
            // early-RX must be DISABLED (bit11 SET). Our old RCR used the 8169-only RX_FIFO_THRESH
            // field (7<<13): that left bit13 set (a bit r8169 never sets on any 8168) and, decisively,
            // RX_EARLY_OFF CLEAR — early-RX enabled, an RX mode with its own payload-address latching
            // (the boot-32/33 every-payload-to-buffer[0] signature's named suspect). Filter bits
            // (promiscuous bring-up) are OR'd in as `rtl_set_rx_mode` does.
            let rcr = RCR_RX128_INT_EN
                | RCR_RX_MULTI_EN
                | RCR_RX_EARLY_OFF
                | RCR_MXDMA_UNLIMITED
                | RCR_AAP
                | RCR_APM
                | RCR_AM
                | RCR_AB;
            serial_println!(
                "{}   >>> REG WRITE (M2): RCR[{:#x}] = {:#010x} (8168 mode: RX128_INT_EN|RX_MULTI_EN|RX_EARLY_OFF|MXDMA + promiscuous filter; NET-4y) ::",
                P4, REG_RCR, rcr
            );
            self.w32(REG_RCR, rcr);
            let rcr_rb = self.r32(REG_RCR);
            serial_println!(
                "{}   [net4y] RCR readback = {:#010x} (RX_EARLY_OFF={} RX_MULTI_EN={} RX128_INT_EN={} bit13={}) [{}] ::",
                P4, rcr_rb, (rcr_rb >> 11) & 1, (rcr_rb >> 14) & 1, (rcr_rb >> 15) & 1, (rcr_rb >> 13) & 1,
                if rcr_rb == rcr { "MATCH" } else { "DIVERGES (NIC-reserved bits)" }
            );

            // Transmit config (MXDMA unlimited + standard IFG) — after RxConfig, the reference order.
            let tcr = TCR_MXDMA_UNLIMITED | TCR_IFG_STD;
            serial_println!("{}   >>> REG WRITE (M2): TCR[{:#x}] = {:#010x} ::", P4, REG_TCR, tcr);
            self.w32(REG_TCR, tcr);

            // NET-4q — no PCIDAC write. Per the facts PCIDAC is a dead parallel-PCI relic on this PCIe
            // 8168 (never set by r8169, masked out of CPCMD_MASK); 64-bit (DAC) payload TLPs form natively
            // from the descriptor high dword. The rings-up readback below keeps PCIDAC as a standing
            // witness: it is EXPECTED to read CLEAR, and that is the correct 64-bit-capable state.

            // Re-lock the config registers.
            self.w8(REG_CFG9346, CFG9346_LOCK);

            // Polled bring-up: mask every interrupt source, clear any latched status (write-1-to-clear).
            self.w16(REG_IMR, 0);
            self.w16(REG_ISR, 0xffff);
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };

            // Poison-honest readback: a live controller returns a plausible TCR (our written value's
            // MXDMA/IFG bits, not an open-bus all-ones). Reject 0xffffffff / 0xdeadbeef as absent decode.
            let tcr_rb = self.r32(REG_TCR);
            if is_poison(tcr_rb) {
                serial_println!("{}   TCR readback = {:#010x} — POISON (open bus / device stopped answering); ring bring-up FAILED ::", P4, tcr_rb);
                return false;
            }
            // NET-4q: read back CplusCmd as a standing witness. PCIDAC is EXPECTED CLEAR — 64-bit DAC
            // payload TLPs form natively from the descriptor high dword, not from this dead legacy bit.
            let cpc_rb = self.r16(REG_CPLUSCMD);
            let pcidac = (cpc_rb & CPCMD_PCIDAC != 0) as u8;
            serial_println!(
                "{}   rings up: RX @ {:#x} ({} desc) TX @ {:#x} ({} desc); TCR readback {:#010x} (live); CPlusCmd {:#06x} PCIDAC={} (expected clear; 64-bit DMA rides the descriptor addr) ::",
                P4, rx_phys, NUM_RX, tx_phys, NUM_TX, tcr_rb, cpc_rb, pcidac
            );
            true
        }

        /// Link state from PHYstatus.LinkSts (bit 1).
        fn link_up(&self) -> bool {
            self.r8(REG_PHYSTATUS) & PHYSTATUS_LINKSTS != 0
        }

        /// Transmit one raw Ethernet frame (smoltcp builds the full L2 frame): copy it into the next
        /// TX buffer, post an OWN|FS|LS descriptor, kick the normal-priority queue (TPPoll.NPQ), and
        /// wait (bounded) for the NIC to clear OWN. A stalled link leaves OWN set — surfaced, not
        /// silently counted. Mirrors the e1000 `transmit` head/tail discipline.
        fn transmit(&mut self, frame: &[u8]) {
            // NET-4k: witness EVERY socket-originated DHCP frame we transmit, BY MESSAGE TYPE — not just
            // the first DISCOVER. The R23s1 boot-13 blind spot the poll-cadence audit exposed: the old
            // probe fired ONLY for the first DISCOVER (the d_xid capture) and net4c fired ONLY on
            // tx_count==1, so a smoltcp-emitted REQUEST left NO serial trace at all. "No REQUEST line"
            // therefore could NOT distinguish "the dhcpv4 socket never emitted a REQUEST" (a frame-accept
            // problem above the driver) from "the REQUEST was sent but un-witnessed" (the drop is the ACK,
            // or the wire). This line resolves it: if the socket accepts the OFFER and dispatches a
            // REQUEST, boot-14 shows `[net4k] TX DHCP 3(REQUEST) ...` on the way to the NIC. Read-only
            // parse; the classifier's DISCOVER-xid capture is preserved.
            if let Some(di) = decode_dhcp(frame) {
                // Preserve the NET-4d classifier's correlation: capture the DISCOVER's xid once.
                if self.d_xid.is_none() && di.op == 1 && di.mtype == 1 {
                    self.d_xid = Some(di.xid);
                }
                if self.dhcp_tx_witnessed < NET4K_TX_WITNESS_MAX {
                    self.dhcp_tx_witnessed += 1;
                    serial_println!(
                        "{}   [net4k] TX DHCP {}({}) xid={:#010x} (udp {}->{}, {} bytes) tx#{} ::",
                        P4, di.mtype, dhcp_mtype_name(di.mtype), di.xid, di.sport, di.dport,
                        frame.len(), self.tx_count + 1
                    );
                }
            }
            let i = self.tx_cur;
            let len = frame.len().min(TX_BUF_SIZE);
            let buf = unsafe { self.tx_buffers.add(i * TX_BUF_SIZE) };
            unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), buf, len) };
            // NET-4B: the TX frame buffer is Normal-NC — the copy above lands straight in the uncached
            // DRAM the NIC fetches; no `dc cvac`. The OWN protocol still needs the r8169 publish order:
            // write the descriptor BODY (OWN clear), `dma_wmb` so it reaches DRAM ahead of the handoff,
            // set OWN LAST (single aligned u32 store), then the barrier before the TPPoll doorbell.
            let eor = if i == NUM_TX - 1 { DESC_EOR } else { 0 };
            let body = DESC_FS | DESC_LS | eor | (len as u32 & DESC_LEN_MASK);
            let desc = unsafe { self.tx_ring.add(i) };
            let d = Desc {
                opts1: body, // OWN CLEAR — published to DRAM before the ownership handoff
                opts2: 0,
                addr: (self.tx_buffers as u64) + (i * TX_BUF_SIZE) as u64,
            };
            unsafe {
                write_volatile(desc, d);
                dma_wmb(); // body reaches DRAM before OWN (r8169 dma_wmb)
                write_volatile(desc as *mut u32, DESC_OWN | body);
                dma_wmb(); // OWN reaches DRAM before the doorbell MMIO write
            }
            self.w8(REG_TPPOLL, TPPOLL_NPQ);
            self.tx_cur = (i + 1) % NUM_TX;

            // Wait (bounded) for the descriptor to be handed back (OWN cleared by the NIC). NC memory ⇒
            // each poll reads OWN straight from DRAM (no invalidate); `dma_rmb` orders the OWN read so the
            // completion is observed correctly.
            let mut done = false;
            for _ in 0..1_000_000 {
                dma_rmb();
                let dd = unsafe { read_volatile(desc) };
                if dd.opts1 & DESC_OWN == 0 {
                    done = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if done {
                self.tx_count += 1;
                // NET-4c: one-shot TX proof on the FIRST consumed frame of the boot (on the
                // armed path that frame is the DHCP DISCOVER). OWN handed back + latched
                // ISR.TOK is NIC-level evidence the frame left the MAC — precisely what the
                // R22 sitting-2 no-lease left unproven.
                if self.tx_count == 1 {
                    let isr = self.r16(REG_ISR);
                    serial_println!(
                        "{}   [net4c] first TX frame ({} bytes) CONSUMED: OWN handed back, ISR={:#06x} (TOK={} TER={}) ::",
                        P4, len, isr, (isr >> 2) & 1, (isr >> 3) & 1);
                }
            } else {
                self.tx_stalled += 1;
                serial_println!("{}   [tx] descriptor {} never completed (OWN still set — link stalled?) ::", P4, i);
            }
        }

        /// NET-4c: bounded, read-only TX/RX evidence snapshot for the DHCP no-lease
        /// investigation — printed after the discover window. TX side: consumed vs stalled
        /// descriptor counts, the last-posted descriptor's OWN bit, and the latched ISR
        /// (TOK/TER/ROK/RER — nothing has cleared ISR since bring-up, so these are
        /// since-bring-up latches). RX side: frames popped plus how many ring slots the NIC
        /// has filled and handed back unread. Reads only; no register is written.
        fn net4c_evidence(&self, label: &str) {
            // NET-4B: the rings are Normal-NC — every read below is a direct DRAM read of the NIC's
            // writebacks; no invalidate needed. `dma_rmb` orders these observations.
            dma_rmb();
            let isr = self.r16(REG_ISR);
            let last_tx = if self.tx_cur == 0 { NUM_TX - 1 } else { self.tx_cur - 1 };
            let d_opts1 = unsafe { read_volatile(self.tx_ring.add(last_tx)) }.opts1;
            let mut rx_filled = 0usize;
            for i in 0..NUM_RX {
                if unsafe { read_volatile(self.rx_ring.add(i)) }.opts1 & DESC_OWN == 0 {
                    rx_filled += 1;
                }
            }
            serial_println!(
                "{}   [net4c {}] TX consumed={} stalled={} last-desc[{}] opts1={:#010x} (OWN={}) | ISR={:#06x} (ROK={} RER={} TOK={} TER={}) | RX popped={} filled-unread={}/{} ::",
                P4, label, self.tx_count, self.tx_stalled, last_tx, d_opts1, (d_opts1 >> 31) & 1,
                isr, isr & 1, (isr >> 1) & 1, (isr >> 2) & 1, (isr >> 3) & 1,
                self.rx_count, rx_filled, NUM_RX);
        }

        /// NET-4g: the decisive RX descriptor-ring dump — the riddle-breaker for "only the FIRST
        /// popped frame ever carries real bytes; every later frame has a real DESCRIPTOR length but an
        /// all-ZERO payload." Everything statically checkable in this driver is correct: `alloc_rx`
        /// programs each slot with a DISTINCT `rx_buffers + i*RX_BUF_SIZE`, `rx_frame_raw` reads the
        /// matching buffer, the arena is one contiguous DRAM region (Orin DRAM base `0x8000_0000`, so
        /// every buffer is equally NIC-reachable — no partial inbound window), and the ring is provably
        /// coherent (the CPU observes the NIC's per-slot length write-backs). The one thing only metal
        /// can answer: what does `desc[i].addr` actually hold after the NIC ran? The C+ RX engine
        /// PRESERVES `addr` across completion (it writes back only opts1/opts2), so for each of the
        /// first `NET4G_DUMP_N` slots this prints the raw post-completion descriptor next to the address
        /// the driver PROGRAMMED. An `ADDR-MISMATCH` proves in-driver corruption / a descriptor-format
        /// or ring-stride mismatch (hypotheses 1 & 2 — an in-file fix); an all-MATCH proves the
        /// descriptors are correct and the payload-to-nowhere is a DMA-write reachability question
        /// (SMMU / inbound iATU) BELOW the driver's lane. Reads only; writes no register.
        fn net4g_desc_dump(&self) {
            serial_println!(
                "{}   [net4g] RX descriptor dump (post-window): ring @ {:#x}, buffers @ {:#x}, stride {} B, {} slots — addr is NIC-preserved across RX completion ::",
                P4, self.rx_ring as u64, self.rx_buffers as u64, RX_BUF_SIZE, NUM_RX
            );
            // NET-4l: default prints the leading NET4G_DUMP_N slots (the metal signature); UNAOS_NET4_RINGDUMP
            // widens it to the full ring so the post-window state of ALL 32 descriptors is captured.
            let n = if option_env!("UNAOS_NET4_RINGDUMP").is_some() {
                NUM_RX
            } else {
                NET4G_DUMP_N.min(NUM_RX)
            };
            // NET-4B: the ring is Normal-NC — these reads are direct DRAM reads; `dma_rmb` orders them.
            dma_rmb();
            for i in 0..n {
                let d = unsafe { read_volatile(self.rx_ring.add(i)) };
                // Copy packed fields BY VALUE before formatting: a format arg takes `&field`, which on a
                // `repr(packed)` struct would be a misaligned reference (a hard error). Mirrors net4c.
                let opts1 = d.opts1;
                let opts2 = d.opts2;
                let addr = d.addr;
                let expect = (self.rx_buffers as u64) + (i * RX_BUF_SIZE) as u64;
                serial_println!(
                    "{}   [net4g] rx-desc[{}] opts1={:#010x} (OWN={} EOR={} len={}) opts2={:#010x} addr={:#x} programmed={:#x} [{}] ::",
                    P4, i, opts1, (opts1 >> 31) & 1, (opts1 >> 30) & 1, opts1 & DESC_LEN_MASK, opts2,
                    addr, expect, if addr == expect { "MATCH" } else { "ADDR-MISMATCH" }
                );
            }
        }

        /// NET-4l: knob-gated (`UNAOS_NET4_RINGDUMP`) full-ring snapshot — OWN/EOR/len/opts2/addr of ALL
        /// 32 RX descriptors at a named point (pre-window, and after each of the first few real RX pops).
        /// The decisive instrumentation for the OWN-last re-arm fix: if the fix is wrong, these lines name
        /// the exact descriptor state the NIC leaves behind (which slots the NIC re-owned, which carry a
        /// real length, which addr diverged) instead of leaving the state machine to guesswork. Read-only.
        fn net4l_ring_dump(&self, tag: &str) {
            serial_println!(
                "{}   [net4l ring-dump {}] ring @ {:#x} buffers @ {:#x} stride {} B rx_cur={} popped={} ::",
                P4, tag, self.rx_ring as u64, self.rx_buffers as u64, RX_BUF_SIZE, self.rx_cur, self.rx_count
            );
            // NET-4B: the ring is Normal-NC — these reads are direct DRAM reads; `dma_rmb` orders them.
            dma_rmb();
            for i in 0..NUM_RX {
                let d = unsafe { read_volatile(self.rx_ring.add(i)) };
                let opts1 = d.opts1;
                let opts2 = d.opts2;
                let addr = d.addr;
                let expect = (self.rx_buffers as u64) + (i * RX_BUF_SIZE) as u64;
                serial_println!(
                    "{}   [net4l] rx-desc[{}] opts1={:#010x} (OWN={} EOR={} len={}) opts2={:#010x} addr={:#x} programmed={:#x} [{}] ::",
                    P4, i, opts1, (opts1 >> 31) & 1, (opts1 >> 30) & 1, opts1 & DESC_LEN_MASK, opts2,
                    addr, expect, if addr == expect { "MATCH" } else { "ADDR-MISMATCH" }
                );
            }
        }

        /// NET-4d: classify one popped RX frame during the DHCP discover window and emit a bounded,
        /// read-only evidence line (the RX-side proof for the no-lease: does the OFFER arrive, and if
        /// so does the driver-visible accept path take it?). A full L2/L3/L4 line for the first
        /// `NET4D_FULL_LINES` frames and ALWAYS for any BOOTP/DHCP (UDP 67/68) frame; every frame is
        /// tallied by category for the window-close summary. For a DHCP frame the BOOTP op / message
        /// type / xid / yiaddr are decoded and the xid is matched against the captured DISCOVER xid;
        /// an OFFER is additionally checked against the driver-visible accept conditions. Read-only.
        fn net4d_classify(&mut self, frame: &[u8]) {
            if !self.rxcls_active {
                return;
            }
            let idx = self.rx_count;
            let len = frame.len();
            if len < 14 {
                self.rxcat[RXCAT_OTHER] += 1;
                // NET-4t: a runt in the `other` bucket is prime DMA-garbage evidence — dump its bytes.
                if self.net4t_other_dumped < NET4T_OTHER_DUMPS {
                    self.net4t_other_dumped += 1;
                    let mut hex = [0u8; 64];
                    for (j, &b) in frame.iter().take(32).enumerate() {
                        const HD: &[u8; 16] = b"0123456789abcdef";
                        hex[j * 2] = HD[(b >> 4) as usize];
                        hex[j * 2 + 1] = HD[(b & 0x0f) as usize];
                    }
                    let n = len.min(32);
                    serial_println!(
                        "{}   [net4t] other[{}] len={} RUNT(<14) first{}B={} ::",
                        P4, self.net4t_other_dumped - 1, len, n,
                        core::str::from_utf8(&hex[..n * 2]).unwrap_or("?")
                    );
                }
                if self.rxcls_full < NET4D_FULL_LINES {
                    self.rxcls_full += 1;
                    serial_println!("{}   [net4d] rx[{}] len={} runt(<14) — class=other ::", P4, idx, len);
                }
                return;
            }
            let d = &frame[0..6];
            let s = &frame[6..12];
            // NET-4t: classify by the EFFECTIVE EtherType (VLAN tags peeled) — the boot-27 `other=8`
            // bucket would have swallowed a DHCP OFFER arriving 802.1Q-tagged.
            let (et, l3_off, vlan) = eth_effective_type(frame);

            // BOOTP/DHCP: full line ALWAYS (unbounded), the frame the investigation is about.
            if let Some(di) = decode_dhcp(frame) {
                self.rxcat[RXCAT_DHCP] += 1;
                self.rxcls_full = self.rxcls_full.saturating_add(1);
                let (xtok, xexp) = match self.d_xid {
                    Some(x) if x == di.xid => ("MATCH", x),
                    Some(x) => ("MISMATCH", x),
                    None => ("no-DISCOVER-xid-seen", 0),
                };
                serial_println!(
                    "{}   [net4d] rx[{}] len={} dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} DHCP {}.{}.{}.{}:{}->{}.{}.{}.{}:{} op={} type={}({}) xid={:#010x} vs-DISCOVER {:#010x} [{}] yiaddr={}.{}.{}.{} ::",
                    P4, idx, len,
                    d[0], d[1], d[2], d[3], d[4], d[5],
                    s[0], s[1], s[2], s[3], s[4], s[5],
                    di.sip[0], di.sip[1], di.sip[2], di.sip[3], di.sport,
                    di.dip[0], di.dip[1], di.dip[2], di.dip[3], di.dport,
                    di.op, di.mtype, dhcp_mtype_name(di.mtype), di.xid, xexp, xtok,
                    di.yiaddr[0], di.yiaddr[1], di.yiaddr[2], di.yiaddr[3]
                );
                // NET-4t: a DHCP frame that arrived VLAN-tagged is the one-line verdict — smoltcp has
                // no 802.1Q support, so the socket can NEVER see it; the no-lease is explained at L2.
                if let Some(vid) = vlan {
                    serial_println!(
                        "{}   [net4t] ^ DHCP frame is 802.1Q-tagged (vlan id={}) — smoltcp does not parse VLAN; the socket never sees it (untag the port or the drop stands) ::",
                        P4, vid
                    );
                    return;
                }
                // Item 3: an OFFER that a lease never followed — name the driver-visible check AND
                // (NET-4j) the exact smoltcp accept gate the frame passes or fails.
                if di.mtype == 2 {
                    self.net4d_offer_check(&di, d, frame);
                }
                return;
            }

            // Non-DHCP: categorize (for the summary) and print a full L2 line only within the bound.
            let cat = match et {
                0x0806 => RXCAT_ARP,
                0x86dd => RXCAT_IPV6,
                0x0800 => {
                    let ip = &frame[l3_off.min(frame.len())..];
                    if ip.len() >= 20 && (ip[0] >> 4) == 4 {
                        let ihl = ((ip[0] & 0x0f) as usize) * 4;
                        if ihl >= 20 && ip.len() >= ihl + 4 && ip[9] == 17 {
                            RXCAT_UDP_OTHER
                        } else {
                            RXCAT_IPV4_OTHER
                        }
                    } else {
                        RXCAT_IPV4_OTHER
                    }
                }
                _ => RXCAT_OTHER,
            };
            self.rxcat[cat] += 1;
            // NET-4t: for the first NET4T_OTHER_DUMPS frames classified `other`, dump len + the first
            // 32 raw bytes + the L2 decode on one line — the boot-27 distinguishing fact (garbage/
            // truncated DMA vs real non-IP traffic) is readable straight off this line. Lives inside
            // the rxcls_active window (the NET4 gated battery), so quiet-boot stays quiet.
            if cat == RXCAT_OTHER && self.net4t_other_dumped < NET4T_OTHER_DUMPS {
                self.net4t_other_dumped += 1;
                let n = len.min(32);
                let mut hex = [0u8; 64];
                for (j, &b) in frame[..n].iter().enumerate() {
                    const HD: &[u8; 16] = b"0123456789abcdef";
                    hex[j * 2] = HD[(b >> 4) as usize];
                    hex[j * 2 + 1] = HD[(b & 0x0f) as usize];
                }
                let hexs = core::str::from_utf8(&hex[..n * 2]).unwrap_or("?");
                serial_println!(
                    "{}   [net4t] other[{}] len={} dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} et={:#06x} vlan={} first{}B={} ::",
                    P4, self.net4t_other_dumped - 1, len,
                    d[0], d[1], d[2], d[3], d[4], d[5],
                    s[0], s[1], s[2], s[3], s[4], s[5],
                    et,
                    vlan.map_or(-1i32, |v| v as i32),
                    n, hexs
                );
            }
            if self.rxcls_full < NET4D_FULL_LINES {
                self.rxcls_full += 1;
                let name = match cat {
                    RXCAT_ARP => "arp",
                    RXCAT_UDP_OTHER => "udp-other",
                    RXCAT_IPV4_OTHER => "ipv4-other",
                    RXCAT_IPV6 => "ipv6",
                    _ => "other",
                };
                serial_println!(
                    "{}   [net4d] rx[{}] len={} dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} et={:#06x} class={} ::",
                    P4, idx, len,
                    d[0], d[1], d[2], d[3], d[4], d[5],
                    s[0], s[1], s[2], s[3], s[4], s[5],
                    et, name
                );
            }
        }

        /// NET-4d: for an inbound DHCP OFFER (message type 2), name the FIRST driver-visible accept
        /// condition it fails — the xid must equal the DISCOVER's, and the destination MAC must be our
        /// station MAC or broadcast. If it passes both, the drop is ABOVE the driver (the smoltcp
        /// dhcpv4 socket), and we say so explicitly rather than blame the wire. Read-only.
        fn net4d_offer_check(&self, di: &DhcpInfo, dst: &[u8], frame: &[u8]) {
            let xid_ok = self.d_xid == Some(di.xid);
            let is_bcast = dst.iter().all(|&b| b == 0xff);
            let is_ours = dst == &self.mac[..];
            if !xid_ok {
                serial_println!(
                    "{}   [net4d] OFFER xid {:#010x} != DISCOVER xid {:#010x} — driver-visible REJECT: wrong transaction ::",
                    P4, di.xid, self.d_xid.unwrap_or(0)
                );
                return;
            }
            if !(is_bcast || is_ours) {
                serial_println!(
                    "{}   [net4d] OFFER xid matches but dst MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} is neither our station MAC nor broadcast — driver-visible REJECT: addressed elsewhere ::",
                    P4, dst[0], dst[1], dst[2], dst[3], dst[4], dst[5]
                );
                return;
            }
            serial_println!(
                "{}   [net4d] OFFER xid matches DISCOVER + addressed to us ({}) yiaddr={}.{}.{}.{} — passes the 3 driver-visible checks ::",
                P4, if is_bcast { "broadcast" } else { "unicast" },
                di.yiaddr[0], di.yiaddr[1], di.yiaddr[2], di.yiaddr[3]
            );
            // NET-4k: the RTL8168 C+ RX engine reports the received length INCLUDING the 4-byte Ethernet
            // FCS (Linux r8169 subtracts 4: `pkt_size = (status & 0x3fff) - 4`); `rx_frame_raw` does NOT,
            // so smoltcp is handed `frame` with the FCS (and any short-frame padding) still appended. This
            // normally parses fine — smoltcp bounds L3/L4 by the IP/UDP length fields — but a length-driven
            // divergence between the driver's tolerant re-decode and smoltcp's own parse is exactly the
            // RTL8168-specific effect QEMU's virtio path (which strips the FCS) cannot reproduce. Witness
            // the delta so boot-14 shows whether excess trailing bytes reach the socket. Read-only.
            if frame.len() >= 18 {
                let ip_total = u16::from_be_bytes([frame[16], frame[17]]) as usize;
                let eth_total = 14 + ip_total;
                let trailing = frame.len().saturating_sub(eth_total);
                serial_println!(
                    "{}   [net4k] OFFER frame len={} ip_total={} eth+ip={} trailing={} B (FCS/pad handed to smoltcp; r8169 strips 4) ::",
                    P4, frame.len(), ip_total, eth_total, trailing
                );
            }
            // NET-4j: the smoltcp dhcpv4 socket applies gates ABOVE the driver's three. Compute each and
            // name the FIRST one this frame fails — the definitive localization the boot-11 "passes ALL
            // driver-visible checks" line could not give (it never inspected these). The NET-4j reproducer
            // proves a frame passing all of these deterministically emits a REQUEST.
            let Some(g) = smoltcp_offer_gate(frame, &self.mac) else {
                serial_println!(
                    "{}   [net4j] OFFER not re-decodable for the smoltcp gate check (unexpected) — cannot localize ::", P4
                );
                return;
            };
            if !g.ipv4_csum_ok {
                serial_println!(
                    "{}   [net4j] smoltcp REJECT at gate 1/4: IPv4 header checksum fails verification — smoltcp drops the packet in Ipv4Repr::parse (default RX checksum caps) ::", P4
                );
            } else if !g.udp_csum_ok {
                serial_println!(
                    "{}   [net4j] smoltcp REJECT at gate 2/4: UDP checksum fails verification (non-zero, mismatched) — smoltcp drops it in UdpRepr::parse before the DHCP socket sees it ::", P4
                );
            } else if !g.chaddr_ok {
                serial_println!(
                    "{}   [net4j] smoltcp REJECT at gate 3/4: BOOTP chaddr != our station MAC — dhcpv4::Socket::process returns early (client_hardware_address mismatch) ::", P4
                );
            } else if g.server_id.is_none() {
                serial_println!(
                    "{}   [net4j] smoltcp REJECT at gate 4/4: DHCP option 54 (server identifier) ABSENT — dhcpv4::Socket::process drops the OFFER (missing server_identifier); no Request is emitted ::", P4
                );
            } else {
                let sid = g.server_id.unwrap();
                serial_println!(
                    "{}   [net4j] smoltcp ACCEPT: IPv4 csum OK, UDP csum {} , chaddr==MAC, server-id={}.{}.{}.{} — the OFFER passes every smoltcp gate; a REQUEST must follow. If none did, the drop is NOT frame content (see reproducer + poll-cadence) ::",
                    P4,
                    if g.udp_csum_zero { "ZERO(accepted)" } else { "OK" },
                    sid[0], sid[1], sid[2], sid[3]
                );
            }
        }

        /// NET-4d: close the classification window — emit the per-category RX summary once and stop
        /// classifying (so the post-window bounded ICMP poll does not re-classify). Read-only.
        fn net4d_window_close(&mut self) {
            self.rxcls_active = false;
            let xid = match self.d_xid {
                Some(x) => x,
                None => 0,
            };
            serial_println!(
                "{}   [net4d window-close] RX by category: arp={} dhcp={} udp-other={} ipv4-other={} ipv6={} other={} (total popped={}); DISCOVER xid={:#010x} ({}) ::",
                P4,
                self.rxcat[RXCAT_ARP], self.rxcat[RXCAT_DHCP], self.rxcat[RXCAT_UDP_OTHER],
                self.rxcat[RXCAT_IPV4_OTHER], self.rxcat[RXCAT_IPV6], self.rxcat[RXCAT_OTHER],
                self.rx_count, xid,
                if self.d_xid.is_some() { "sent" } else { "NEVER SENT" }
            );
        }

        /// Pop one completed RX descriptor's raw Ethernet frame into `out` and recycle the descriptor
        /// (re-arm OWN + buffer size), advancing the ring cursor. Returns the copied length, or `None`
        /// if the current descriptor is still NIC-owned (ring empty). The C+ analog of the e1000
        /// `rx_frame_raw` — no responder dispatch, smoltcp owns the stack.
        fn rx_frame_raw(&mut self, out: &mut [u8]) -> Option<usize> {
            // NET-4B — the descriptor ring is Normal-NC: this read is a direct DRAM read of the NIC's
            // OWN/len writeback (no invalidate, no resident clean copy to shadow it).
            let d = unsafe { read_volatile(self.rx_ring.add(self.rx_cur)) };
            // OWN set ⇒ still owned by the NIC (not yet filled) ⇒ ring empty.
            if d.opts1 & DESC_OWN != 0 {
                return None;
            }
            // NET-4e: DMA READ BARRIER between observing OWN-clear and reading the buffer — the fix
            // for the DHCP no-lease. The NIC commits a received frame by writing the payload FIRST and
            // clearing OWN LAST; on weakly-ordered aarch64 the CPU may observe the OWN-clear without yet
            // observing the payload write, so the copy below reads STALE bytes (the `alloc_zeroed` fill
            // ⇒ an all-zero frame). Each descriptor is popped exactly once and recycled, so a single
            // stale read drops that frame forever — which is precisely how the OFFER got through (it won
            // the race) but the follow-on ACK did not (read as zeros), leaving the lease uncompleted.
            // This is Linux r8169's `dma_rmb()` after the OWN check. NET-4B: the buffers are now
            // Normal-NC, so no cache invalidate is needed for the payload — but this ordering barrier is
            // STILL load-bearing: NC loads reorder on weakly-ordered aarch64, so without it the payload
            // read below could be reordered ahead of the OWN read and observe pre-frame DRAM.
            dma_rmb();
            // Hardware wrote the received length into the length field; clamp so a misbehaving NIC can
            // never make us build an out-of-bounds slice.
            let len = (d.opts1 & DESC_LEN_MASK) as usize;
            let len = len.min(RX_BUF_SIZE).min(out.len());
            let buf = unsafe { self.rx_buffers.add(self.rx_cur * RX_BUF_SIZE) };
            // NET-4u WITNESS (knob-gated, first ≤2 pops): read the leading 16 payload bytes BEFORE the
            // invalidate — paired with the post-ivac read after it, one boot names whether the pop-path
            // invalidate is what turns stale zeros into payload (before=00.. after=real ⇒ the fix is the
            // fix) or whether DRAM itself reads zero (both zero ⇒ below-driver reachability). The
            // pre-read pulls lines into the cache, but the full-span invalidate right below drops them,
            // so the witness cannot perturb the frame the copy sees.
            let net4u_witness = option_env!("UNAOS_NET4_RINGDUMP").is_some() && self.rx_count < 2;
            let mut net4u_pre = [0u8; 16];
            if net4u_witness {
                let n = len.min(16);
                for (i, slot) in net4u_pre.iter_mut().enumerate().take(n) {
                    *slot = unsafe { read_volatile(buf.add(i)) };
                }
            }
            // NET-4B: the buffer is Normal-NC — the copy below reads the NIC's payload straight from
            // DRAM; there are no stale cache lines to invalidate (that was the NET-4f/4u cacheable-RAM
            // maintenance, now removed by construction). The `dma_rmb` above is the only ordering needed.
            // NET-4u WITNESS (post-barrier half): re-read the same 16 bytes — this is what the copy sees.
            if net4u_witness {
                let n = len.min(16);
                let mut post = [0u8; 16];
                for (i, slot) in post.iter_mut().enumerate().take(n) {
                    *slot = unsafe { read_volatile(buf.add(i)) };
                }
                serial_println!(
                    "{}   [net4u] rx[{}] slot={} len={} pre-ivac={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} post-ivac={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} ::",
                    P4, self.rx_count, self.rx_cur, len,
                    net4u_pre[0], net4u_pre[1], net4u_pre[2], net4u_pre[3],
                    net4u_pre[4], net4u_pre[5], net4u_pre[6], net4u_pre[7],
                    net4u_pre[8], net4u_pre[9], net4u_pre[10], net4u_pre[11],
                    net4u_pre[12], net4u_pre[13], net4u_pre[14], net4u_pre[15],
                    post[0], post[1], post[2], post[3], post[4], post[5], post[6], post[7],
                    post[8], post[9], post[10], post[11], post[12], post[13], post[14], post[15]
                );
            }
            // NET-4D (knob UNAOS_NET4_BUF1, INTERIM WORKAROUND — not the fix): boot-39-retry proved
            // every inbound payload lands at buffer[1]'s address (the NIC's last-fetched descriptor
            // addr; mechanism NET-4A, cause unfound after 7 exonerations) while completions advance
            // per-slot. The OFFER was only ever readable because it happened to BE pop 1. Until the
            // NIC-internal fetch defect is cured, read the frame from where the NIC provably writes:
            // for any completed slot other than 1, if buffer[1]'s head is non-zero, copy from
            // buffer[1] instead of the (zero) slot buffer, then zero buffer[1]'s head so a stale
            // frame is never double-consumed. Loses coalesced back-to-back frames (single landing
            // address) — acceptable for the DHCP exchange; NOT a keeper. Off by default.
            let buf1 = unsafe { self.rx_buffers.add(RX_BUF_SIZE) };
            let use_buf1 = option_env!("UNAOS_NET4_BUF1").is_some()
                && self.rx_cur != 1
                && unsafe { read_volatile(buf1) } != 0
                && {
                    let mut nz = false;
                    for i in 0..12 {
                        nz |= unsafe { read_volatile(buf1.add(i)) } != 0;
                    }
                    nz
                };
            let src = if use_buf1 { buf1 } else { buf };
            unsafe {
                core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), len);
            }
            if use_buf1 {
                // Consume: zero the head so the next pop can't re-read this frame.
                for i in 0..16 {
                    unsafe { core::ptr::write_volatile(buf1.add(i), 0u8) };
                }
                serial_println!(
                    "{}   [net4D] rx[{}] slot={} len={} frame HARVESTED from buffer[1] (interim buf1-read workaround; head consumed) ::",
                    P4, self.rx_count, self.rx_cur, len
                );
            }
            // NET-4B: one-shot WITNESS on the first popped frame — names the coherency strategy now on the
            // live RX path (Normal-NC DMA window + `dma_rmb` ordering, replacing every cache-maintenance
            // step). The buffers/ring are uncached, so CPU and NIC share one DRAM view with no clean/inval.
            if self.rx_count == 0 {
                serial_println!(
                    "{}   [net4B] first RX pop len={} — rings + buffers Normal-NC (MAIR AttrIdx 2): dma_rmb after OWN-clear, dma_wmb before OWN publish; NO cache maintenance ::",
                    P4, len
                );
            }
            self.rx_count += 1;
            // NET-4d: classify this frame (bounded, read-only) while the DHCP discover window is live.
            // Borrows `out` (not `self`), so the &mut self counter updates do not alias.
            self.net4d_classify(&out[..len]);
            // NET-4m: the DECISIVE per-pop discriminator (knob-gated, read-only). The zeros survived
            // NET-4l's correct OWN-last re-arm AND the per-pop `dc ivac` above (the copy at line ~1107
            // is ALREADY a post-invalidate DRAM read), so "more invalidate" is a no-op — the open
            // question is WHERE the zero comes from. This probe re-reads the SAME buffer with an
            // independent, speculation-FENCED invalidate (`dc ivac` + `dsb sy` + `isb`, so no line
            // speculatively re-fetched between the invalidate and this read can shadow DRAM), then dumps
            // the leading bytes. It splits the two remaining root causes on the next metal boot:
            //   * bytes NON-ZERO  ⇒ the buffer DRAM holds the NIC's payload and the copy's zero was a
            //     cache/speculation artifact ⇒ the "do it right" fix is a NON-CACHEABLE DMA arena (needs
            //     a Normal-NC MAIR slot + splitting mmu_tegra's 1 GiB RAM block to L2/L3 — an MMU arc,
            //     OUTSIDE this driver-invalidate lane).
            //   * bytes ZERO with a real descriptor len ⇒ the NIC's payload write never landed in the
            //     CPU-visible buffer DRAM ⇒ a WRITES-TO-NOWHERE inbound-DMA reachability gap (SMMU /
            //     inbound iATU / ORIN-DMA-WINDOW), BELOW the driver's lane.
            // The descriptor `addr` is already proven correct by NET-4g (all-MATCH), so those are the
            // only two branches. Bounded to NET4M_PROBE_N pops; gated behind the NET-4 ring knob.
            if option_env!("UNAOS_NET4_RINGDUMP").is_some() && self.rx_count <= NET4M_PROBE_N {
                let n = len.min(NET4M_PROBE_BYTES);
                // NET-4B: NC memory — a direct DRAM read (`dma_rmb` orders it); no invalidate/fence.
                dma_rmb();
                let mut b = [0u8; NET4M_PROBE_BYTES];
                let mut nonzero = false;
                for (i, slot) in b.iter_mut().enumerate().take(n) {
                    let v = unsafe { read_volatile(buf.add(i)) };
                    *slot = v;
                    nonzero |= v != 0;
                }
                serial_println!(
                    "{}   [net4m] rx[{}] slot={} len={} post-ivac(fenced) buf[0..{}]={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} nonzero={} — {} ::",
                    P4, self.rx_count - 1, self.rx_cur, len, n,
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                    b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
                    nonzero,
                    if nonzero {
                        "DRAM holds NIC payload -> copy's zero was cache/speculation (non-cacheable-arena, MMU arc)"
                    } else {
                        "DRAM ZERO w/ real desc len -> writes-to-nowhere (inbound SMMU/iATU, below driver)"
                    }
                );
            }
            // NET-4y — the DECISIVE buffer[0] cross-witness (knob-gated, read-only; first ≤3 pops on a
            // slot other than 0). Boot-32/33 fact pattern: only slot-0 recycles carry payload, slots 1+
            // read DRAM-zero at real writeback lengths, and slot-0's CONTENTS changed across pops —
            // consistent with the NIC resolving EVERY frame's payload write to buffer[0]'s address
            // regardless of which slot's descriptor completed. So on a non-zero-slot pop, read
            // buffer[0] (independently invalidated + fenced so DRAM, not a resident line, answers) and
            // ask: does buffer[0] hold a plausible frame RIGHT NOW (nonzero, sane EtherType), and is it
            // THIS pop's frame? `buf0-holds-the-frame=yes` on a slot-N pop is the smoking gun.
            if option_env!("UNAOS_NET4_RINGDUMP").is_some()
                && self.rx_cur != 0
                && self.net4y_probes < 3
            {
                self.net4y_probes += 1;
                let b0 = self.rx_buffers;
                dma_rmb(); // NET-4B: NC direct DRAM read, no invalidate
                let mut b = [0u8; 16];
                let mut nonzero = false;
                for (i, slot) in b.iter_mut().enumerate() {
                    let v = unsafe { read_volatile(b0.add(i)) };
                    *slot = v;
                    nonzero |= v != 0;
                }
                let et = u16::from_be_bytes([b[12], b[13]]);
                // Plausible Ethernet II EtherType (>= 0x0600) or an 802.1Q TPID — "ethertype sane".
                let et_sane = et >= 0x0600;
                let n = len.min(16);
                let same = n > 0 && b[..n] == out[..n];
                serial_println!(
                    "{}   [net4y] rx[{}] popped slot={} len={} buf0[0..16]={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} etype={:#06x} same-as-this-pop's-frame={} buf0-holds-the-frame={} ::",
                    P4, self.rx_count - 1, self.rx_cur, len,
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                    b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
                    et, same as u8,
                    if nonzero && et_sane { "yes" } else { "no" }
                );
            }
            // NET-4z — the SCAN-ALL destination witness (knob-gated, read-only; first NET4Z_PROBE_N
            // pops, every slot). net4y proved buffer[0] holds the frame but only ever INSPECTED
            // buffer[0], so it cannot tell "every payload to the arena base (index 0)" from "payload to
            // some OTHER wrong index" nor confirm it is not the completed slot's own buffer. This scans
            // EVERY RX buffer — each independently invalidated + speculation-fenced so DRAM answers, not
            // a resident line — and reports the exact index whose head equals THIS pop's frame. `hit==0`
            // on a non-zero-slot pop is the smoking gun that the NIC sources every payload's target from
            // the FIRST descriptor's buffer address (== the arena base in this layout) while advancing
            // its writeback index independently — a NIC-internal descriptor-address latch BELOW the
            // driver's per-descriptor programming lane (whose DRAM is proven correct: net4x [MATCH]).
            if option_env!("UNAOS_NET4_RINGDUMP").is_some() && self.net4z_probes < NET4Z_PROBE_N {
                self.net4z_probes += 1;
                let n = len.min(16);
                let mut hit: i64 = -1;
                if n > 0 {
                    dma_rmb(); // NET-4B: NC direct DRAM reads across the ring, no invalidate
                    for k in 0..NUM_RX {
                        let bk = unsafe { self.rx_buffers.add(k * RX_BUF_SIZE) };
                        let mut same = true;
                        for i in 0..n {
                            if unsafe { read_volatile(bk.add(i)) } != out[i] {
                                same = false;
                                break;
                            }
                        }
                        if same {
                            hit = k as i64;
                            break;
                        }
                    }
                }
                serial_println!(
                    "{}   [net4z] rx[{}] popped slot={} len={} frame-landed-in-buffer-index={} completed-slot={} (arena-base=index 0) — {} ::",
                    P4, self.rx_count - 1, self.rx_cur, len, hit, self.rx_cur,
                    if hit == self.rx_cur as i64 {
                        "payload landed in the COMPLETED slot's own buffer -> per-descriptor addressing WORKS"
                    } else if hit == 0 {
                        "payload landed at the ARENA BASE (index 0), NOT the completed slot -> NIC latches the first descriptor's buffer address; driver DRAM is correct (net4x MATCH) so this is a NIC/RC descriptor-address reuse below the programming lane"
                    } else if hit < 0 {
                        "payload NOT FOUND in any RX buffer (all-zero / sank elsewhere)"
                    } else {
                        "payload landed in an UNEXPECTED buffer index (neither the completed slot nor the arena base)"
                    }
                );
                // NET-4A — the CROSS-POP mechanism verdict. net4z names WHERE each payload landed, one
                // pop at a time; net4A correlates those landings to prove the boot-36 signature as a
                // SINGLE line: the landing index STAYS CONSTANT (stuck at the last-fetched descriptor's
                // buffer) while the completed slot ADVANCES, for ≥4 consecutive pops. That pattern is
                // the fingerprint of NIC-internal descriptor-address REUSE (mechanism (a)): the NIC
                // latched the last descriptor it fetched and does not re-fetch, so every later
                // completion's payload DMA targets the same buffer while its OWN/len writeback still
                // rides the (correctly advancing) internal ring index. It is NOT a driver cache bug
                // (mechanism (b), refuted — see the NET-4A ledger note): our ring DRAM carries the
                // correct distinct addr for every slot (net4x [MATCH]), and no clean/invalidate on the
                // 16 B-strided ring can ever substitute one descriptor's addr into another's slot.
                let slot = self.rx_cur as i64;
                if !self.net4a_fired && hit >= 0 {
                    let advanced = self.net4a_prev_slot >= 0 && slot == self.net4a_prev_slot + 1;
                    let stuck = self.net4a_prev_land >= 0 && hit == self.net4a_prev_land;
                    if advanced && stuck {
                        self.net4a_run += 1;
                    } else {
                        self.net4a_run = 0;
                    }
                    // `net4a_run` counts consecutive stuck-and-advancing PAIRS; 3 pairs = 4 consecutive
                    // pops all landing in the same buffer while the slot advanced each time.
                    if self.net4a_run >= 3 {
                        self.net4a_fired = true;
                        serial_println!(
                            "{}   [net4A] VERDICT mechanism=(a) NIC-internal descriptor-address REUSE: {} consecutive pops (through completed slot {}) all landed in buffer index {} while the completed slot advanced 1:1 — the NIC reuses the LAST-fetched descriptor's buffer addr and does not re-fetch. Ring DRAM is correct (net4x MATCH) ⇒ this is below the driver's programming lane; (b) cache-line interplay is REFUTED (no ring cache op can redirect a payload to another slot's buffer, and (b) predicts a ZERO addr → bus-0 0x200 sink, contradicted by this valid-buffer landing with no RAS) ::",
                            P4, self.net4a_run + 1, slot, hit
                        );
                    }
                }
                self.net4a_prev_land = hit;
                self.net4a_prev_slot = slot;
            }
            // NET-4B: the buffer is Normal-NC — no clean/invalidate before re-arming. The copy above read
            // it uncached (nothing pulled into the D-cache), and the NIC's next fill lands in the same
            // uncached DRAM. The whole NET-4f/4u "evicted-line-written-back-over-DMA" hazard cannot exist
            // on NC memory; it is removed by construction, which is the point of this arc.
            // NET-4l: re-arm with the r8169 OWN-LAST publish discipline — the fix for "only the first
            // popped frame ever carries real bytes; rx[2..] read a real DESCRIPTOR length but an all-zero
            // payload." The INITIAL ring is published by `init_rings`' trailing `dsb sy` BEFORE RX is
            // enabled, so the NIC observes those descriptors fully-formed → the first frame is real. But
            // every RE-ARM here previously wrote the whole 16-byte descriptor as ONE unordered store with
            // NO barrier: on weakly-ordered aarch64 the continuously-polling C+ RX engine could observe
            // OWN=1 (opts1) BEFORE the addr/len/opts2 stores became visible, and DMA the next frame against
            // a STALE (or still-zeroed) descriptor — precisely the "later buffers possibly never written by
            // the NIC at all" signature for slots ≥2, and why the ONE real frame is always the first pop
            // (only the barrier-published initial descriptors are ever seen coherently). Fix: publish the
            // descriptor BODY (addr + len + EOR) with OWN CLEAR first, `dsb sy` to order it ahead of the
            // ownership handoff, then set OWN LAST in a single aligned u32 store (opts1 is at offset 0 of a
            // 16-byte-strided, 256-byte-aligned ring ⇒ always 4-aligned) and `dsb sy` to publish it. This
            // is Linux r8169's addr/opts2 → dma_wmb() → OWN|opts1 order. It is a DMA PUBLISH (write-side)
            // barrier — NOT the refuted read-side `dsb ld`, and NOT cache maintenance (also refuted).
            let eor = if self.rx_cur == NUM_RX - 1 { DESC_EOR } else { 0 };
            let body = eor | (RX_BUF_SIZE as u32 & DESC_LEN_MASK); // OWN CLEAR
            let desc = unsafe { self.rx_ring.add(self.rx_cur) };
            let nd = Desc {
                opts1: body,
                opts2: 0,
                addr: (self.rx_buffers as u64) + (self.rx_cur * RX_BUF_SIZE) as u64,
            };
            unsafe {
                write_volatile(desc, nd);
                // NET-4B — NC ring: the body store reaches DRAM directly; `dma_wmb` orders it ahead of
                // the OWN handoff (r8169's addr/opts2 → dma_wmb() → OWN|opts1 order), no `dc cvac`.
                dma_wmb();
                // Hand ownership to the NIC LAST — a single aligned u32 store to opts1 (offset 0) — then
                // a second `dma_wmb` so OWN reaches DRAM before the next fetch (the NIC polls DRAM).
                write_volatile(desc as *mut u32, DESC_OWN | body);
                dma_wmb();
            }
            // NET-4l instrumentation (knob-gated, read-only): dump the FULL 32-slot ring state after each
            // of the first few real RX pops so a wrong fix names the state machine exactly (brief item 3).
            if option_env!("UNAOS_NET4_RINGDUMP").is_some() && self.rx_count <= NET4L_AFTERRX_MAX {
                self.net4l_ring_dump("after-rx");
            }
            self.rx_cur = (self.rx_cur + 1) % NUM_RX;
            Some(len)
        }
    }

    // ── Controller-0 aperture resolution (a lean, self-contained DTB walk) ──

    /// Resolve controller-0's `ecam` region base from the live DTB: find the first `pcie@` node, then
    /// index its `reg`/`reg-names` for "ecam" (4 cells = addr:2 + size:2 per region, big-endian).
    /// READ-ONLY. Returns `None` on any missing/foreign DTB (QEMU virt has no Tegra234 RC). Confirms
    /// the node is a Tegra DesignWare RC and firmware-enabled before returning the base.
    fn resolve_ecam_base(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<u64> {
        if dtb_addr == 0 || dtb_size == 0 {
            return None;
        }
        // The DTB must be in a mapped GiB (GiB 0 device window, or a RAM GiB) before we deref it.
        let g_lo = dtb_addr >> 30;
        let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
        let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
        if !mapped(g_lo) || !mapped(g_hi) {
            return None;
        }
        let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
        let fdt = Fdt::new(blob)?;

        // First `pcie@` node's path.
        const PATH_CAP: usize = 160;
        let mut path0 = [0u8; PATH_CAP];
        let mut plen0 = 0usize;
        let mut found = false;
        fdt.for_each_prop(|e| {
            if found {
                return;
            }
            let leaf = match e.path.iter().rposition(|&b| b == b'/') {
                Some(i) => &e.path[i + 1..],
                None => e.path,
            };
            if leaf.starts_with(b"pcie@") {
                let l = e.path.len().min(PATH_CAP);
                path0[..l].copy_from_slice(&e.path[..l]);
                plen0 = l;
                found = true;
            }
        });
        if !found {
            return None;
        }
        let path = &path0[..plen0];

        // Capture the props we need in one walk: compatible, status, reg, reg-names.
        let mut compatible: Option<&[u8]> = None;
        let mut status: Option<&[u8]> = None;
        let mut reg: Option<&[u8]> = None;
        let mut reg_names: Option<&[u8]> = None;
        fdt.for_each_prop(|e| {
            if e.path != path {
                return;
            }
            let val = &blob[e.val_off..e.val_off + e.val_len];
            match e.name {
                b"compatible" => compatible = Some(val),
                b"status" => status = Some(val),
                b"reg" => reg = Some(val),
                b"reg-names" => reg_names = Some(val),
                _ => {}
            }
        });

        // Tegra DesignWare RC? (a generic virt ecam is not — graceful skip).
        let is_tegra_rc = compatible
            .map(|c| {
                let has = |n: &[u8]| c.windows(n.len()).any(|w| w == n);
                has(b"tegra234-pcie") || has(b"tegra194-pcie") || has(b"snps,dw-pcie")
            })
            .unwrap_or(false);
        if !is_tegra_rc {
            return None;
        }
        // Firmware-enabled? (absent status ⇒ "okay" per the DT spec; anything but okay/ok ⇒ skip.)
        let okay = match status {
            None => true,
            Some(s) => s.split(|&b| b == 0).any(|item| item == b"okay" || item == b"ok"),
        };
        if !okay {
            return None;
        }

        // Index reg-names for "ecam"; read that region's 64-bit base from reg.
        let (reg, names) = (reg?, reg_names?);
        let mut idx = 0usize;
        for item in names.split(|&b| b == 0) {
            if item.is_empty() {
                continue;
            }
            if item == b"ecam" {
                let off = idx * 16; // 4 cells * 4 bytes
                let b = reg.get(off..off + 8)?;
                let hi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
                let lo = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as u64;
                return Some((hi << 32) | lo);
            }
            idx += 1;
        }
        None
    }

    // ══════════════════════════════════════════════════════════════════════════════════════════════
    // DWC outbound iATU — the fix-forward for the ORIN-NET-4 M1 metal FAULT-AT-M1.
    // ══════════════════════════════════════════════════════════════════════════════════════════════
    //
    // ## The fault of record (adjudicated; NOT re-litigated)
    //
    // The NET-4 driver reached the RTL8168 (config reads/writes via the ECAM fine, twice-confirmed),
    // then the FIRST BAR-register write (CR soft reset) raised a RAS Uncorrectable — SNOC "Illegal
    // address (software fault)" / Carveout, `a5a5a5a5` poison fill; recovery needed a DC cut. The
    // adjudication: BAR2 read back `0x4000_4000` — a PCIe BUS address (firmware assigned the device's
    // BARs inside controller-0's PCIe MEM window, whose PCI base is `0x4000_0000`). With the DWC iATU
    // UNPROGRAMMED (the NET-2 finding), there is NO outbound CPU->PCIe MEM translation, so a PCIe bus
    // address is meaningless as a CPU physical address. The old path mapped `0x4000_4000` as a CPU PA —
    // it falls in the GiB-1 SYSRAM/BPMP carveout that `mmu_tegra::fill_table` maps Device-nGnRE, so
    // `map_mmio_window` even returned `AlreadyMapped` without complaint — and the first register write
    // (`0x4000_4000 + CR`) hit a protected Tegra carveout. The bench observation was RIGHT.
    //
    // ## The fix (DWC / pcie-tegra194 sequence-of-record)
    //
    // Program an OUTBOUND iATU region mapping a CPU aperture window (taken from controller-0's DT
    // `ranges`) to the PCIe MEM window, then access the CPU-SIDE aperture address
    // (`cpu_base + (bar_pci - pci_base)`), NEVER the raw BAR value. Firmware's BAR assignment is KEPT
    // (it already sits inside the ranges-described window NET-3 sized it in) and merely TRANSLATED — no
    // BAR reassignment. That is fewer fabric writes and it is the Linux DWC host model of record
    // (`dw_pcie_prog_outbound_atu` walks `ranges` and leaves enumerated BARs in place).
    //
    // ## DWC unrolled-iATU register model
    //
    // Linux `drivers/pci/controller/dwc/pcie-designware.h`. Outbound region N lives at
    // `atu_base + N*0x200`; `atu_base` = controller-0's `atu_dma` reg region (from the DTB), with the
    // DWC-core `dbi + 0x30_0000` fallback documented for a controller that ships no dedicated ATU
    // region. Every iATU register write is announced on serial before issue (the lane's write
    // discipline). These writes target the controller's OWN internal register block (GiB-0 device
    // window, always decoding on a powered RC — NET-2/3 read dbi/appl/ecam there) — NOT a carveout, so
    // they carry none of the M1 fault's risk.
    const ATU_REGION_STRIDE: u64 = 0x200;
    const ATU_UNR_REGION_CTRL1: u64 = 0x00;
    const ATU_UNR_REGION_CTRL2: u64 = 0x04;
    const ATU_UNR_LOWER_BASE: u64 = 0x08;
    const ATU_UNR_UPPER_BASE: u64 = 0x0c;
    const ATU_UNR_LOWER_LIMIT: u64 = 0x10;
    const ATU_UNR_LOWER_TARGET: u64 = 0x14;
    const ATU_UNR_UPPER_TARGET: u64 = 0x18;
    const ATU_UNR_UPPER_LIMIT: u64 = 0x20;
    /// CTRL1 TYPE field: memory outbound = 0x0. CTRL2: region-enable = bit31; increase-region-size =
    /// bit13 (makes LIMIT the full 64-bit UPPER|LOWER pair — required here, the CPU aperture base sits
    /// ~200 GiB up, well beyond 32 bits).
    const ATU_TYPE_MEM: u32 = 0x0;
    const ATU_ENABLE: u32 = 1 << 31;
    const ATU_INCREASE_REGION_SIZE: u32 = 1 << 13;
    /// The DWC-core fallback ATU offset when a controller exposes no dedicated ATU reg region.
    const ATU_DBI_FALLBACK_OFF: u64 = 0x30_0000;
    /// NET-4h — DWC unrolled-iATU direction bit. Outbound regions live at `atu_base + index*0x200`
    /// (dir=0); INBOUND regions at `atu_base + 0x100 + index*0x200` (dir=1). This is Linux's
    /// `PCIE_ATU_UNROLL_BASE(dir, index) = (index << 9) | (dir << 8)` (`pcie-designware.c`), a SEPARATE
    /// region array from the outbound one — so inbound region 0 does not collide with the outbound
    /// region 0 the M1-fix programmed. An inbound region translates an incoming PCIe (bus-master DMA)
    /// address in [BASE, LIMIT] to TARGET + (addr - BASE); REGION_CTRL2 bit30=0 selects address-match
    /// (not BAR-match). Identity DRAM: BASE = TARGET = DRAM base, LIMIT = DRAM top.
    const ATU_INBOUND_DIR_OFF: u64 = 0x100;

    /// A `ranges` MEM window: its PCIe base, the CPU aperture base it maps to, and its size.
    #[derive(Clone, Copy)]
    struct MemWindow {
        pci_base: u64,
        cpu_base: u64,
        size: u64,
    }

    /// Program one DWC OUTBOUND iATU region `[cpu_base, cpu_base+size)` -> PCIe `[pci_base, …)`, type
    /// MEM, and enable it. `atu_base` must already be reachable (GiB-0 device window; the caller
    /// idempotent-maps it). Base/limit/target are published (`dsb sy`) before the region is armed.
    fn program_outbound_atu(atu_base: u64, index: u64, win: &MemWindow) {
        let region = atu_base + index * ATU_REGION_STRIDE;
        let limit = win.cpu_base + win.size - 1;
        let w = |off: u64, v: u32| unsafe { write_volatile((region + off) as *mut u32, v) };
        serial_println!(
            "{}   M1-fix: outbound iATU region {} @ {:#x} — CPU [{:#x}..{:#x}] -> PCIe {:#x} (type MEM) ::",
            P4, index, region, win.cpu_base, limit, win.pci_base
        );
        serial_println!(
            "{}   >>> ATU WRITE (M1-fix): BASE lo/hi = {:#010x}/{:#010x} ::",
            P4, win.cpu_base as u32, (win.cpu_base >> 32) as u32
        );
        w(ATU_UNR_LOWER_BASE, win.cpu_base as u32);
        w(ATU_UNR_UPPER_BASE, (win.cpu_base >> 32) as u32);
        serial_println!(
            "{}   >>> ATU WRITE (M1-fix): LIMIT lo/hi = {:#010x}/{:#010x} ::",
            P4, limit as u32, (limit >> 32) as u32
        );
        w(ATU_UNR_LOWER_LIMIT, limit as u32);
        w(ATU_UNR_UPPER_LIMIT, (limit >> 32) as u32);
        serial_println!(
            "{}   >>> ATU WRITE (M1-fix): TARGET lo/hi = {:#010x}/{:#010x} ::",
            P4, win.pci_base as u32, (win.pci_base >> 32) as u32
        );
        w(ATU_UNR_LOWER_TARGET, win.pci_base as u32);
        w(ATU_UNR_UPPER_TARGET, (win.pci_base >> 32) as u32);
        serial_println!("{}   >>> ATU WRITE (M1-fix): REGION_CTRL1 = TYPE_MEM ::", P4);
        w(ATU_UNR_REGION_CTRL1, ATU_TYPE_MEM);
        // Publish base/limit/target BEFORE the region goes live.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
        serial_println!(
            "{}   >>> ATU WRITE (M1-fix): REGION_CTRL2 = ENABLE|INCREASE_REGION_SIZE — arming region ::",
            P4
        );
        w(ATU_UNR_REGION_CTRL2, ATU_ENABLE | ATU_INCREASE_REGION_SIZE);
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }

    /// NET-4h — program one DWC INBOUND iATU region for identity DRAM DMA: an incoming bus-master
    /// PCIe write whose address falls in `[dram_base, dram_base+dram_size)` is translated to the SAME
    /// DRAM physical address (BASE = TARGET, identity — matching the driver's identity-map ring/buffer
    /// assumption, where the NIC DMAs against the allocation's PA directly). This is the step the
    /// OUTBOUND-only M1-fix left unprogrammed: with NO inbound region, the NIC's descriptor + payload
    /// writes reach DRAM only through whatever firmware-residual inbound mapping survived — enough for
    /// the ring page and the first buffer, but every later payload write lands nowhere (the NET-4d/f/g
    /// "first frame real, rest real-length/zero-payload" signature, root-caused to DMA-write
    /// reachability BELOW the descriptor level). The Pi 4 seam does the analogous thing with the
    /// brcmstb RC_BAR2 inbound window (`piusb.rs` step (e)); Linux `dw_pcie_setup_rc` programs an
    /// inbound region for the host's memory the same way. Inbound region 0 (dir=1) is a separate slot
    /// from the outbound region 0 the M1-fix armed. Every write is announced (the lane's discipline);
    /// base/limit/target are published (`dsb sy`) before the region is enabled.
    fn program_inbound_atu(atu_base: u64, index: u64, dram_base: u64, dram_size: u64) {
        let region = atu_base + ATU_INBOUND_DIR_OFF + index * ATU_REGION_STRIDE;
        let limit = dram_base + dram_size - 1;
        let w = |off: u64, v: u32| unsafe { write_volatile((region + off) as *mut u32, v) };
        serial_println!(
            "{}   [net4h] inbound iATU region {} @ {:#x} — PCIe DMA [{:#x}..{:#x}] -> DRAM {:#x} (identity, type MEM) ::",
            P4, index, region, dram_base, limit, dram_base
        );
        serial_println!(
            "{}   >>> ATU WRITE (net4h): BASE lo/hi = {:#010x}/{:#010x} ::",
            P4, dram_base as u32, (dram_base >> 32) as u32
        );
        w(ATU_UNR_LOWER_BASE, dram_base as u32);
        w(ATU_UNR_UPPER_BASE, (dram_base >> 32) as u32);
        serial_println!(
            "{}   >>> ATU WRITE (net4h): LIMIT lo/hi = {:#010x}/{:#010x} ::",
            P4, limit as u32, (limit >> 32) as u32
        );
        w(ATU_UNR_LOWER_LIMIT, limit as u32);
        w(ATU_UNR_UPPER_LIMIT, (limit >> 32) as u32);
        serial_println!(
            "{}   >>> ATU WRITE (net4h): TARGET lo/hi = {:#010x}/{:#010x} (identity) ::",
            P4, dram_base as u32, (dram_base >> 32) as u32
        );
        w(ATU_UNR_LOWER_TARGET, dram_base as u32);
        w(ATU_UNR_UPPER_TARGET, (dram_base >> 32) as u32);
        serial_println!("{}   >>> ATU WRITE (net4h): REGION_CTRL1 = TYPE_MEM ::", P4);
        w(ATU_UNR_REGION_CTRL1, ATU_TYPE_MEM);
        // Publish base/limit/target BEFORE the region goes live (address-match, region-size increased
        // because the DRAM window spans >32 bits — base ~2 GiB, limit tens of GiB up).
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
        serial_println!(
            "{}   >>> ATU WRITE (net4h): REGION_CTRL2 = ENABLE|INCREASE_REGION_SIZE (bit30=0 address-match) — arming inbound region ::",
            P4
        );
        w(ATU_UNR_REGION_CTRL2, ATU_ENABLE | ATU_INCREASE_REGION_SIZE);
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }

    /// NET-4s — discover the TRUE inbound-iATU window count AND the enabled-index mask in one pass, BEFORE
    /// arming any alias. Boot-25 proved a CTRL2-read enumeration is not enough: it reports every one of the
    /// 8 unroll CSR blocks as a candidate "free" index, but index 2 NEVER latched a write while index 1 did —
    /// the RC implements fewer inbound windows than the CSR address space exposes. So the count is discovered
    /// by a WRITABILITY probe (L4T-FACTS §A5, `dw_pcie_iatu_detect` idiom): write a probe value to each
    /// index's `LOWER_TARGET` and read it back — an UNIMPLEMENTED window does not retain the write. Windows
    /// are contiguous from 0, so the first index whose write does not stick bounds `num_ib_windows`. A live
    /// ENABLED index (index 0 = the NET-4h identity, in active use) is counted present WITHOUT a destructive
    /// write; a disabled index is probed write-readback-RESTORE (the probe value has low-16 = 0 since
    /// `LOWER_TARGET[15:0]` is hardwired to 0 per §A2, so a present window reads it back exactly, and the
    /// original is restored either way). Returns `(num_ib_windows, enabled_mask)`. Unroll blocks live at
    /// `atu_base + (index<<9) | BIT8` (= `ATU_INBOUND_DIR_OFF + index*ATU_REGION_STRIDE`), all within the
    /// 0x1000 ATU window the caller mapped, so probing 8 indices touches no unmapped register.
    fn probe_inbound_windows(atu_base: u64) -> (u64, u32) {
        const PROBE_N: u64 = 8;
        const PROBE_VAL: u32 = 0x1111_0000; // low-16 zero (§A2 LOWER_TARGET[15:0] hardwired 0) ⇒ exact readback
        let vb = option_env!("UNAOS_NET4").is_some();
        let mut enabled = 0u32;
        let mut count = 0u64;
        let mut counting = true; // stop growing the count at the first gap (windows are contiguous from 0)
        for idx in 0..PROBE_N {
            let region = atu_base + ATU_INBOUND_DIR_OFF + idx * ATU_REGION_STRIDE;
            let ctrl2 = unsafe { read_volatile((region + ATU_UNR_REGION_CTRL2) as *const u32) };
            let en = ctrl2 & ATU_ENABLE != 0;
            if en {
                enabled |= 1u32 << idx;
            }
            // Present iff the window physically exists. A live/enabled index is implemented by definition
            // (don't clobber it to prove so); a disabled index is proved by write-readback-restore.
            let present = if en {
                true
            } else {
                let ltgt = (region + ATU_UNR_LOWER_TARGET) as *mut u32;
                let orig = unsafe { read_volatile(ltgt as *const u32) };
                unsafe {
                    write_volatile(ltgt, PROBE_VAL);
                    core::arch::asm!("dsb sy", options(nostack, preserves_flags));
                }
                let rb = unsafe { read_volatile(ltgt as *const u32) };
                unsafe {
                    write_volatile(ltgt, orig); // restore — the probe leaves the register file untouched
                    core::arch::asm!("dsb sy", options(nostack, preserves_flags));
                }
                rb == PROBE_VAL
            };
            if vb {
                serial_println!(
                    "{}   [net4s] inbound region {} @ {:#x}: CTRL2={:#010x} enabled={} writable={} ::",
                    P4, idx, region, ctrl2, en as u8, present as u8
                );
            }
            if counting {
                if present {
                    count += 1;
                } else {
                    counting = false;
                }
            }
        }
        serial_println!(
            "{}   [net4s] inbound iATU probe: {} implemented window(s) (writability write-readback-restore, §A5); enabled-index mask = {:#06x} (index 0 = NET-4h identity; aliases take a FREE implemented index) ::",
            P4, count, enabled
        );
        (count, enabled)
    }

    /// NET-4r — program ONE DWC INBOUND iATU ALIAS region and PROVE it latched. Unlike NET-4h's identity
    /// region (BASE=TARGET), here BASE != TARGET: an incoming bus-master write whose (truncated 32-bit)
    /// address falls in `[alias_base, limit]` is up-translated to `target + (addr - alias_base)` — back to
    /// the real high-heap PA whose high dword this RC integration drops (boot-24: the full-64 descriptor
    /// was correct yet the payload still sank at 0x200, so the >4 GiB TLP truncates on the wire). Boot-20's
    /// alias failed because `UPPER_TARGET` (the `0x2` dword) never latched, so NET-4r READS BACK every
    /// register and REFUSES (returns false) on ANY mismatch — BASE, LIMIT, TARGET including UPPER_TARGET —
    /// and POLLS `CTRL2` for the `ENABLE` bit before trusting the region (the reference driver loops on
    /// this readback, §A5). `CTRL2 = ENABLE` ONLY — NO `INCREASE_REGION_SIZE`: the MATCH window
    /// `[alias_base, limit]` is wholly sub-4 GiB (§A0/§A2 — the >4 GiB TARGET's upper-32 is independent and
    /// does not require it). The caller 64 KiB-aligns `alias_base`/`target`; the LIMIT is 64 KiB-rounded up
    /// to the controller's granularity so the readback compares exactly.
    fn program_inbound_alias(atu_base: u64, index: u64, alias_base: u64, size: u64, target: u64, tag: &str) -> bool {
        let region = atu_base + ATU_INBOUND_DIR_OFF + index * ATU_REGION_STRIDE;
        // LIMIT rounds UP to the 64 KiB granularity (low-16 hardwired to 1, §A2); alias_base/target are
        // 64 KiB-aligned by the caller, so the whole region reads back exactly what is written.
        let limit = ((alias_base + size + 0xFFFF) & !0xFFFFu64) - 1;
        let w = |off: u64, v: u32| unsafe { write_volatile((region + off) as *mut u32, v) };
        let vb = option_env!("UNAOS_NET4").is_some();
        serial_println!(
            "{}   [net4r] inbound iATU ALIAS region {} @ {:#x} ({}) — truncated PCIe DMA [{:#x}..{:#x}] -> real DRAM {:#x} (up-translate +{:#x}, type MEM) ::",
            P4, index, region, tag, alias_base, limit, target, target - alias_base
        );
        if vb {
            serial_println!("{}   >>> ATU WRITE (net4r): BASE lo/hi = {:#010x}/{:#010x} ::", P4, alias_base as u32, (alias_base >> 32) as u32);
        }
        w(ATU_UNR_LOWER_BASE, alias_base as u32);
        w(ATU_UNR_UPPER_BASE, (alias_base >> 32) as u32);
        if vb {
            serial_println!("{}   >>> ATU WRITE (net4r): LIMIT lo/hi = {:#010x}/{:#010x} ::", P4, limit as u32, (limit >> 32) as u32);
        }
        w(ATU_UNR_LOWER_LIMIT, limit as u32);
        w(ATU_UNR_UPPER_LIMIT, (limit >> 32) as u32);
        if vb {
            serial_println!("{}   >>> ATU WRITE (net4r): TARGET lo/hi = {:#010x}/{:#010x} (the real high buffer PA; UPPER_TARGET is the {:#x} dword boot-20 lost) ::", P4, target as u32, (target >> 32) as u32, (target >> 32) as u32);
        }
        w(ATU_UNR_LOWER_TARGET, target as u32);
        w(ATU_UNR_UPPER_TARGET, (target >> 32) as u32);
        if vb {
            serial_println!("{}   >>> ATU WRITE (net4r): REGION_CTRL1 = TYPE_MEM ::", P4);
        }
        w(ATU_UNR_REGION_CTRL1, ATU_TYPE_MEM);
        // Publish BASE/LIMIT/TARGET/CTRL1 BEFORE the region goes live.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
        if vb {
            serial_println!("{}   >>> ATU WRITE (net4r): REGION_CTRL2 = ENABLE (bit30=0 address-match; NO increase-region-size — match window sub-4 GiB) — arming ::", P4);
        }
        w(ATU_UNR_REGION_CTRL2, ATU_ENABLE);
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
        // ── Readback ritual (the NET-4r spine): refuse on ANY mismatch; poll CTRL2 for ENABLE. ──
        let r = |off: u64| -> u32 { unsafe { read_volatile((region + off) as *const u32) } };
        let mut ctrl2 = r(ATU_UNR_REGION_CTRL2);
        let mut polls = 0u32;
        while ctrl2 & ATU_ENABLE == 0 && polls < 1000 {
            core::hint::spin_loop();
            ctrl2 = r(ATU_UNR_REGION_CTRL2);
            polls += 1;
        }
        let rb_base = ((r(ATU_UNR_UPPER_BASE) as u64) << 32) | r(ATU_UNR_LOWER_BASE) as u64;
        let rb_limit = ((r(ATU_UNR_UPPER_LIMIT) as u64) << 32) | r(ATU_UNR_LOWER_LIMIT) as u64;
        let rb_utarget = r(ATU_UNR_UPPER_TARGET);
        let rb_target = ((rb_utarget as u64) << 32) | r(ATU_UNR_LOWER_TARGET) as u64;
        serial_println!(
            "{}   [net4r] alias region {} ({}) readback: BASE={:#x} LIMIT={:#x} TARGET={:#x} UPPER_TARGET={:#010x} CTRL2={:#010x} enabled={} (after {} polls) ::",
            P4, index, tag, rb_base, rb_limit, rb_target, rb_utarget, ctrl2, (ctrl2 >> 31) & 1, polls
        );
        let want_ut = (target >> 32) as u32;
        if rb_base != alias_base || rb_limit != limit || rb_target != target || rb_utarget != want_ut || (ctrl2 & ATU_ENABLE == 0) {
            serial_println!(
                "{}   !! [net4r] alias region {} ({}) READBACK MISMATCH — expected BASE={:#x} LIMIT={:#x} TARGET={:#x} UPPER_TARGET={:#010x} ENABLE=1; REFUSING (the boot-20 UPPER_TARGET-lost / enable-not-latched failure mode) ::",
                P4, index, tag, alias_base, limit, target, want_ut
            );
            return false;
        }
        true
    }

    /// ORIN-DMA-WINDOW — one-shot READ-ONLY probe (UNAOS_DMAWIN): dump the inbound-DMA window DERIVED
    /// from the RC's `dma-ranges`, read BACK the just-programmed inbound iATU region-0 registers, and
    /// cross-check both against the `ram_gib_mask`-derived identity window NET-4h armed. This closes the
    /// loop the heap-guard opened: the next metal boot confirms the derivation (`pcie_dma_windows`,
    /// which `select_heap_region` now constrains the heap to) matches the hardware the NIC actually
    /// DMAs through. Reads only — no register write; the region was already armed above.
    fn dmawin_probe(dtb_addr: u64, dtb_size: usize, atu_base: u64, dram_base: u64, dram_size: u64) {
        // 1. The firmware-declared inbound window(s) from dma-ranges.
        let mut win = [(0u64, 0u64); 8];
        let nd = crate::arch::aarch64::fdt_tegra::pcie_dma_windows(dtb_addr, dtb_size, &mut win);
        if nd == 0 {
            serial_println!(
                "{}   [dmawin] no PCIe dma-ranges derivable from DTB — inbound window is UNVERIFIED against firmware; iATU armed from ram_gib_mask identity only ::",
                P4
            );
        } else {
            for i in 0..nd {
                serial_println!(
                    "{}   [dmawin] derived inbound window[{}] = [{:#x}, {:#x}) ({} MiB) — the firmware-declared bus->CPU DMA reach ::",
                    P4, i, win[i].0, win[i].0.wrapping_add(win[i].1), win[i].1 >> 20
                );
            }
        }
        // 2. Read back inbound region 0's live registers (BASE/LIMIT/TARGET/CTRL2).
        let region = atu_base + ATU_INBOUND_DIR_OFF;
        let r = |off: u64| -> u32 { unsafe { read_volatile((region + off) as *const u32) } };
        let base = ((r(ATU_UNR_UPPER_BASE) as u64) << 32) | r(ATU_UNR_LOWER_BASE) as u64;
        let limit = ((r(ATU_UNR_UPPER_LIMIT) as u64) << 32) | r(ATU_UNR_LOWER_LIMIT) as u64;
        let target = ((r(ATU_UNR_UPPER_TARGET) as u64) << 32) | r(ATU_UNR_LOWER_TARGET) as u64;
        let ctrl2 = r(ATU_UNR_REGION_CTRL2);
        serial_println!(
            "{}   [dmawin] inbound iATU region0 @ {:#x} readback: BASE={:#x} LIMIT={:#x} TARGET={:#x} CTRL2={:#010x} (enabled={}) ::",
            P4, region, base, limit, target, ctrl2, (ctrl2 >> 31) & 1
        );
        // 3. Cross-check the programmed identity window vs the derivation.
        let prog_lo = dram_base;
        let prog_hi = dram_base.wrapping_add(dram_size);
        let inside_derived = (0..nd).any(|i| prog_lo >= win[i].0 && prog_hi <= win[i].0.wrapping_add(win[i].1));
        serial_println!(
            "{}   [dmawin] programmed identity DRAM window [{:#x}, {:#x}) {} the {} derived dma-ranges window(s); readback BASE/TARGET {} the programmed base ::",
            P4,
            prog_lo, prog_hi,
            if nd == 0 { "UNVERIFIED against" } else if inside_derived { "is INSIDE" } else { "DIVERGES from" },
            nd,
            if base == prog_lo && target == prog_lo { "MATCH" } else { "MISMATCH" }
        );
    }

    /// NET-4h — the identity DRAM window the inbound iATU must cover, derived from `ram_gib_mask` (bit
    /// `g` set ⇒ GiB `g` is RAM). Returns `[lowest RAM GiB .. highest RAM GiB]` as `(base, size)` so a
    /// single inbound region reaches every buffer the kernel heap can hand the NIC (on Orin the arena
    /// sits high — ~9.6 GiB in the boot-6 capture — well above the DRAM base at GiB 2). `None` if the
    /// mask is empty (no RAM known ⇒ refuse rather than program a bogus window).
    fn dram_window(ram_gib_mask: u64) -> Option<(u64, u64)> {
        if ram_gib_mask == 0 {
            return None;
        }
        let lo = ram_gib_mask.trailing_zeros() as u64;
        let hi = 63 - ram_gib_mask.leading_zeros() as u64;
        let base = lo << 30;
        let size = (hi - lo + 1) << 30;
        Some((base, size))
    }

    /// Resolve, from the live DTB, controller-0's `atu_dma` ATU base (with the `dbi + 0x30_0000` DWC
    /// fallback) and the `ranges` MEM window that CONTAINS `bar_pci`. READ-ONLY parse, poison-honest:
    /// a missing/foreign/disabled DTB, an unreachable DTB GiB, or a BAR that no MEM window covers all
    /// return `None`, and the caller REFUSES (clean skip) rather than deref a raw PCIe BAR. Mirrors
    /// `resolve_ecam_base`'s walk (first `pcie@` node, tegra-RC + firmware-`okay` gated).
    fn resolve_atu_and_window(
        dtb_addr: u64,
        dtb_size: usize,
        ram_gib_mask: u64,
        bar_pci: u64,
    ) -> Option<(u64, MemWindow)> {
        if dtb_addr == 0 || dtb_size == 0 {
            return None;
        }
        let g_lo = dtb_addr >> 30;
        let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
        let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
        if !mapped(g_lo) || !mapped(g_hi) {
            return None;
        }
        let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
        let fdt = Fdt::new(blob)?;

        // First `pcie@` node's path.
        const PATH_CAP: usize = 160;
        let mut path0 = [0u8; PATH_CAP];
        let mut plen0 = 0usize;
        let mut found = false;
        fdt.for_each_prop(|e| {
            if found {
                return;
            }
            let leaf = match e.path.iter().rposition(|&b| b == b'/') {
                Some(i) => &e.path[i + 1..],
                None => e.path,
            };
            if leaf.starts_with(b"pcie@") {
                let l = e.path.len().min(PATH_CAP);
                path0[..l].copy_from_slice(&e.path[..l]);
                plen0 = l;
                found = true;
            }
        });
        if !found {
            return None;
        }
        let path = &path0[..plen0];

        let mut compatible: Option<&[u8]> = None;
        let mut status: Option<&[u8]> = None;
        let mut reg: Option<&[u8]> = None;
        let mut reg_names: Option<&[u8]> = None;
        let mut ranges: Option<&[u8]> = None;
        fdt.for_each_prop(|e| {
            if e.path != path {
                return;
            }
            let val = &blob[e.val_off..e.val_off + e.val_len];
            match e.name {
                b"compatible" => compatible = Some(val),
                b"status" => status = Some(val),
                b"reg" => reg = Some(val),
                b"reg-names" => reg_names = Some(val),
                b"ranges" => ranges = Some(val),
                _ => {}
            }
        });

        // Tegra DesignWare RC + firmware-enabled? (same gate as resolve_ecam_base.)
        let is_tegra_rc = compatible
            .map(|c| {
                let has = |n: &[u8]| c.windows(n.len()).any(|w| w == n);
                has(b"tegra234-pcie") || has(b"tegra194-pcie") || has(b"snps,dw-pcie")
            })
            .unwrap_or(false);
        if !is_tegra_rc {
            return None;
        }
        let okay = match status {
            None => true,
            Some(s) => s.split(|&b| b == 0).any(|item| item == b"okay" || item == b"ok"),
        };
        if !okay {
            return None;
        }

        // reg/reg-names region base by name (4 cells = addr:2 + size:2 per region, big-endian).
        let (reg, names) = (reg?, reg_names?);
        let region_base = |want: &[u8]| -> Option<u64> {
            let mut idx = 0usize;
            for item in names.split(|&b| b == 0) {
                if item.is_empty() {
                    continue;
                }
                if item == want {
                    let off = idx * 16;
                    let b = reg.get(off..off + 8)?;
                    let hi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
                    let lo = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as u64;
                    return Some((hi << 32) | lo);
                }
                idx += 1;
            }
            None
        };
        // Prefer the dedicated ATU region; fall back to the DWC-core dbi + 0x30_0000 offset.
        let atu_base = region_base(b"atu_dma")
            .or_else(|| region_base(b"atu"))
            .or_else(|| region_base(b"dbi").map(|d| d + ATU_DBI_FALLBACK_OFF))?;

        // Walk `ranges`: rows of 7 cells (child PCI addr:3, parent CPU addr:2, size:2 = 28 bytes).
        // The child cell-0 high byte's space code ((>>24)&3): 2 = 32-bit MEM, 3 = 64-bit MEM (1 = I/O,
        // skipped). Return the first MEM window whose [pci_base, pci_base+size) contains `bar_pci`.
        let ranges = ranges?;
        let cell = |b: &[u8], i: usize| -> u64 {
            u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]) as u64
        };
        let mut off = 0usize;
        while off + 28 <= ranges.len() {
            let row = &ranges[off..off + 28];
            let space = (cell(row, 0) >> 24) & 0x3;
            let pci_base = (cell(row, 1) << 32) | cell(row, 2);
            let cpu_base = (cell(row, 3) << 32) | cell(row, 4);
            let size = (cell(row, 5) << 32) | cell(row, 6);
            if (space == 2 || space == 3)
                && size != 0
                && bar_pci >= pci_base
                && bar_pci < pci_base + size
            {
                return Some((atu_base, MemWindow { pci_base, cpu_base, size }));
            }
            off += 28;
        }
        None
    }

    // ── NET-4C: PCIe Max_Payload_Size / Max_Read_Request_Size audit + reconcile ──────────────────────
    // The seventh mechanism under test (Boot-38 ledger; the six prior exonerations stand). The NIC ever
    // fetches exactly 2 descriptors (32 B) then reuses buffer 1 forever, while OWN/len writebacks
    // advance 1:1 and ring DRAM is provably correct (net4x/net4A). Theory (NET-4C): the descriptor READ
    // completion is truncated/dropped by an MPS mismatch across the controller-0 link. A completion
    // whose data payload exceeds the REQUESTER's Max_Payload_Size is a Malformed TLP the requester drops
    // (PCIe base spec: a receiver checks payload <= its own MPS; the completer must split at min of the
    // two MPS). If UEFI/MB2 left the DWC root port at a larger MPS than the RTL8168 endpoint, the RC's
    // descriptor-read completions overrun the EP's MPS -> dropped -> only the first (small) chunk is
    // consumed -> the NIC falls back to the last-latched buffer address for payload DMA (the observed
    // signature). L4T runs the endpoint at MRRS=4096 non-jumbo (r8169 `rtl_jumbo_config`: readrq=4096
    // then `pcie_set_readrq`) and MPS = the tree-common value `pcie_bus_configure_settings` computes
    // (min MPSS across the path). UnaOS's driver programs NEITHER side's Device Control — only COMMAND
    // (0x04) — so both run whatever firmware left. This function reads DevCap/DevCtl/DevSta on BOTH sides
    // (RC = bus0:dev0:fn0 = ecam+0; EP = bus1:dev0:fn0), prints the `[net4C]` readback table
    // UNCONDITIONALLY (the boot oracle), then reconciles MPS to the smallest value BOTH DevCaps advertise
    // (mirroring the Linux PCI core) and clamps EP MRRS to a conservative 512 — each side written only
    // if its DevCtl actually changes, announced + read back, BEFORE rings-up. The DevSta error bits
    // (CorrErr/NonFatalErr/FatalErr/UnsupReq latched) are the completion-path discriminator and print
    // either way: a latched UnsupReq/error CONVICTS the completion path; all-clear + already-matching
    // MPS is the honest refutation (the truncation is not an MPS/MRRS mis-program). Read-first; the only
    // writes are Device Control MPS/MRRS fields (DevSta high half written 0 => RW1C untouched).

    /// PCIe capability id (Device Control lives in this capability at cap+0x08).
    const PCIE_CAP_ID: u32 = 0x10;
    /// DevCap (cap+0x04): bits[2:0] = Max_Payload_Size Supported. DevCtl (cap+0x08, low 16): bits[7:5] =
    /// MPS, bits[14:12] = MRRS. DevSta (cap+0x0a, i.e. the dword's high 16): bit0 CorrErr, bit1
    /// NonFatalErr, bit2 FatalErr, bit3 UnsupReq. Size in bytes = 128 << field.
    const NET4C_DEVCAP: u64 = 0x04;
    const NET4C_DEVCTL: u64 = 0x08;

    /// One side's audited PCIe Device-Control state.
    #[derive(Clone, Copy)]
    struct DevCtlState {
        cap_off: u64,
        devcap_mps: u32, // Max_Payload_Size Supported field (ceiling)
        mps_field: u32,  // current DevCtl MPS field
        mrrs_field: u32, // current DevCtl MRRS field
        devctl_lo: u16,  // full 16-bit DevCtl (for read-modify-write)
    }

    /// Walk `cfg_base`'s capability list to the PCIe capability (id 0x10). Mirrors the NET-2 DBI cap
    /// walk: poison-rejecting, bounded, dword-aligned cap pointers. Returns the cap offset or `None`.
    fn net4c_find_pcie_cap(cfg_base: u64) -> Option<u64> {
        let sw = unsafe { read_volatile((cfg_base + 0x04) as *const u32) };
        if is_poison(sw) || (sw >> 16) & (1 << 4) == 0 {
            return None; // absent decode or no capabilities list
        }
        let mut ptr = (unsafe { read_volatile((cfg_base + 0x34) as *const u32) } & 0xff) as u64;
        let mut hops = 0;
        while ptr >= 0x40 && ptr < 0x100 && hops < 48 {
            let h = unsafe { read_volatile((cfg_base + (ptr & !0x3)) as *const u32) };
            if is_poison(h) {
                break;
            }
            let id = h & 0xff;
            let next = (h >> 8) & 0xff;
            if id == PCIE_CAP_ID {
                return Some(ptr);
            }
            if next == 0 || next as u64 == ptr {
                break;
            }
            ptr = next as u64;
            hops += 1;
        }
        None
    }

    /// Read + print one side's DevCap/DevCtl/DevSta. `None` on absent decode / no PCIe cap.
    fn net4c_audit_side(cfg_base: u64, label: &str) -> Option<DevCtlState> {
        let Some(cap) = net4c_find_pcie_cap(cfg_base) else {
            serial_println!(
                "{}   [net4C] {} @ {:#x}: no PCIe capability (absent decode / no cap list) — side UNREAD ::",
                P4, label, cfg_base
            );
            return None;
        };
        let devcap = unsafe { read_volatile((cfg_base + cap + NET4C_DEVCAP) as *const u32) };
        let ctlsta = unsafe { read_volatile((cfg_base + cap + NET4C_DEVCTL) as *const u32) };
        if is_poison(devcap) || is_poison(ctlsta) {
            serial_println!(
                "{}   [net4C] {} PCIe cap @ {:#x}: DevCap/DevCtl poison — side UNREAD ::",
                P4, label, cap
            );
            return None;
        }
        let devcap_mps = devcap & 0x7;
        let devctl_lo = (ctlsta & 0xffff) as u16;
        let devsta = (ctlsta >> 16) as u16;
        let mps_field = ((devctl_lo >> 5) & 0x7) as u32;
        let mrrs_field = ((devctl_lo >> 12) & 0x7) as u32;
        serial_println!(
            "{}   [net4C] {} cap@{:#x}: MPS={}B (cap {}B) MRRS={}B | DevSta={:#06x} CorrErr={} NonFatal={} Fatal={} UnsupReq={} ::",
            P4, label, cap,
            128u32 << mps_field, 128u32 << devcap_mps, 128u32 << mrrs_field,
            devsta, devsta & 1, (devsta >> 1) & 1, (devsta >> 2) & 1, (devsta >> 3) & 1
        );
        let _ = devsta; // printed above; not retained past the witness line
        Some(DevCtlState { cap_off: cap, devcap_mps, mps_field, mrrs_field, devctl_lo })
    }

    /// Audit both sides of the controller-0 link and reconcile MPS/MRRS BEFORE rings-up. `ecam` = the
    /// mapped whole-domain ECAM base (bus0:dev0:fn0 = the DWC root port); `dev` = the RTL8168 endpoint
    /// config (bus1:dev0:fn0). Prints the readback table unconditionally; writes Device Control only
    /// where a field must change (announced + read back). Non-fatal: any unread side leaves the link as
    /// firmware set it and the bring-up proceeds (the witness records what was — or wasn't — seen).
    fn net4c_mps_mrrs(ecam: u64, dev: u64) {
        serial_println!(
            "{}   [net4C] PCIe MPS/MRRS audit — RC root-port (bus0:dev0:fn0) + RTL8168 endpoint (bus1:dev0:fn0) ::",
            P4
        );
        let rc = net4c_audit_side(ecam, "RC root-port");
        let ep = net4c_audit_side(dev, "EP RTL8168 ");
        let (Some(rc), Some(ep)) = (rc, ep) else {
            serial_println!(
                "{}   [net4C] one side UNREAD — MPS/MRRS reconcile SKIPPED; link left as firmware set it (witness stands) ::",
                P4
            );
            return;
        };

        // Smallest MPS both sides SUPPORT (mirrors pcie_bus_configure_settings = min MPSS on the path).
        let common_mps = rc.devcap_mps.min(ep.devcap_mps);
        // Conservative bring-up MRRS: clamp to 512 B (field 0b010). L4T runs 4096 non-jumbo (r8169
        // rtl_jumbo_config); 512 is the safe first-fix value — widen if MPS-reconcile alone leaves the
        // fetch truncated. Never widen MRRS here.
        const MRRS_512_FIELD: u32 = 0b010;
        let ep_target_mrrs = ep.mrrs_field.min(MRRS_512_FIELD);

        let rc_mismatch = rc.mps_field != common_mps;
        let ep_mismatch = ep.mps_field != common_mps || ep.mrrs_field != ep_target_mrrs;
        if !rc_mismatch && !ep_mismatch {
            serial_println!(
                "{}   [net4C] MPS already coherent (both {}B, common-supported {}B) + EP MRRS {}B ≤ 512B — NO reconcile write. If the fetch still truncates, MPS/MRRS mis-program is REFUTED; look to DevSta error bits above (latched UnsupReq/Fatal = completion path). ::",
                P4, 128u32 << common_mps, 128u32 << common_mps, 128u32 << ep.mrrs_field
            );
            return;
        }

        // EP reconcile (the driver-owned device): set MPS = common, MRRS = clamped. DevSta high half
        // written 0 so its RW1C bits are untouched (writing 1 would clear a latched error we want kept).
        if ep_mismatch {
            let new_lo = (ep.devctl_lo & !((0x7 << 5) | (0x7 << 12)))
                | ((common_mps as u16) << 5)
                | ((ep_target_mrrs as u16) << 12);
            serial_println!(
                "{}   >>> CONFIG WRITE (net4C): EP DevCtl[{:#x}] {:#06x} -> {:#06x} (MPS {}B->{}B, MRRS {}B->{}B) — issuing ::",
                P4, ep.cap_off + NET4C_DEVCTL, ep.devctl_lo, new_lo,
                128u32 << ep.mps_field, 128u32 << common_mps,
                128u32 << ep.mrrs_field, 128u32 << ep_target_mrrs
            );
            unsafe {
                write_volatile((dev + ep.cap_off + NET4C_DEVCTL) as *mut u32, new_lo as u32);
                core::arch::asm!("dsb sy", options(nostack, preserves_flags));
            }
            let back = (unsafe { read_volatile((dev + ep.cap_off + NET4C_DEVCTL) as *const u32) }
                & 0xffff) as u16;
            serial_println!(
                "{}   [net4C] EP DevCtl readback = {:#06x} (MPS={}B MRRS={}B) — {} ::",
                P4, back, 128u32 << ((back >> 5) & 0x7), 128u32 << ((back >> 12) & 0x7),
                if back == new_lo { "MATCH" } else { "MISMATCH (field(s) hardwired?)" }
            );
        }

        // RC reconcile (only if the root port runs a larger MPS than the common value — else the EP
        // would still receive over-MPS completions). This is a fabric write to the DWC root port's own
        // config; announced before issue, read back. Left untouched when the RC already matches.
        if rc_mismatch {
            let new_lo = (rc.devctl_lo & !(0x7 << 5)) | ((common_mps as u16) << 5);
            serial_println!(
                "{}   >>> CONFIG WRITE (net4C): RC root-port DevCtl[{:#x}] {:#06x} -> {:#06x} (MPS {}B->{}B; matches EP so completions never overrun) — issuing ::",
                P4, rc.cap_off + NET4C_DEVCTL, rc.devctl_lo, new_lo,
                128u32 << rc.mps_field, 128u32 << common_mps
            );
            unsafe {
                write_volatile((ecam + rc.cap_off + NET4C_DEVCTL) as *mut u32, new_lo as u32);
                core::arch::asm!("dsb sy", options(nostack, preserves_flags));
            }
            let back = (unsafe { read_volatile((ecam + rc.cap_off + NET4C_DEVCTL) as *const u32) }
                & 0xffff) as u16;
            serial_println!(
                "{}   [net4C] RC DevCtl readback = {:#06x} (MPS={}B) — {} ::",
                P4, back, 128u32 << ((back >> 5) & 0x7),
                if back == new_lo { "MATCH" } else { "MISMATCH (field hardwired?)" }
            );
        }
        serial_println!(
            "{}   [net4C] reconcile DONE — both sides MPS={}B; next boot's [net4m]/[net4A] verdict discriminates (payload lands 1:1 = MPS was the cut; still stuck = MPS REFUTED, mechanism-(a) fetch-reuse stands) ::",
            P4, 128u32 << common_mps
        );
    }

    /// ORIN-NET-4 entry point (metal): claim controller-0's downstream RTL8168, map its register BAR,
    /// reset the MAC, and read the station MAC. Rings + init (M2) and the smoltcp bind (M3) land in
    /// later milestones. Graceful on any missing/foreign DTB or absent decode (records and returns).
    pub fn net4_bringup(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
        serial_println!(
            "{} ORIN-NET-4 RTL8168/8111 GbE bring-up (DTB @{:#x} size={:#x}) ::",
            P4, dtb_addr, dtb_size
        );

        // ── Resolve + map controller-0's ECAM (the direct hardware config window NET-3 unlocked) ──
        let Some(ecam) = resolve_ecam_base(dtb_addr, dtb_size, ram_gib_mask) else {
            serial_println!(
                "{}   no enabled Tegra234 RC ecam in the DTB — bring-up SKIPPED (graceful; QEMU virt / no-net) ::",
                P4
            );
            return;
        };
        // The ECAM is ~184 GiB — reachable only through NET-3's PS-widen. map_mmio_window refuses it if
        // the widen is not in effect (a pcie3-off build), which the driver reports rather than deref.
        let ecam_size = 256 * 1024 * 1024; // Tegra234 whole-domain ECAM window
        match map_mmio_window(ecam, ecam_size) {
            MmioMap::Mapped | MmioMap::AlreadyMapped => {
                serial_println!("{}   ecam {:#x} mapped Device-nGnRE (via the PS-widened regime) ::", P4, ecam);
            }
            MmioMap::BeyondPsCeiling => {
                serial_println!(
                    "{}   ecam {:#x} BEYOND the PS ceiling — the NET-3 TCR widen is not in effect; bring-up cannot reach config space ::",
                    P4, ecam
                );
                return;
            }
        }
        let dev = ecam + BUS1_DEV0_FN0;

        // ── Confirm the device identity (poison-rejecting; must be the metal-identified Realtek) ──
        let vd = unsafe { core::ptr::read_volatile((dev + CFG_VENDOR) as *const u32) };
        if is_poison(vd) {
            serial_println!(
                "{}   bus1:dev0:fn0 config[0x00] = {:#010x} — ABSENT DECODE (link down / no device answering); bring-up SKIPPED ::",
                P4, vd
            );
            return;
        }
        let vendor = (vd & 0xffff) as u16;
        let device = (vd >> 16) as u16;
        serial_println!("{}   bus1:dev0:fn0 vendor={:#06x} device={:#06x} ::", P4, vendor, device);
        if vendor != REALTEK_VENDOR || device != RTL8168_DEVICE {
            serial_println!(
                "{}   not the metal-identified Realtek RTL8168/8111 ({:#06x}:{:#06x}) — bring-up SKIPPED (won't drive an unknown device) ::",
                P4, REALTEK_VENDOR, RTL8168_DEVICE
            );
            return;
        }

        // ── Enable MEM-space decode + bus-master so the BARs decode and the NIC can DMA (M2 rings) ──
        // This is the driver doing the config write NET-3 deliberately refused. Announced before issue.
        let cmd = unsafe { core::ptr::read_volatile((dev + CFG_COMMAND) as *const u32) };
        let cmd_lo = (cmd & 0xffff) as u16;
        let newcmd = cmd_lo | CMD_MEM_SPACE | CMD_BUS_MASTER;
        serial_println!(
            "{}   >>> CONFIG WRITE (M1): COMMAND[{:#x}] {:#06x} -> {:#06x} (set MEM-space + bus-master) — issuing ::",
            P4, CFG_COMMAND, cmd_lo, newcmd
        );
        unsafe {
            core::ptr::write_volatile((dev + CFG_COMMAND) as *mut u32, (cmd & 0xffff_0000) | newcmd as u32);
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }

        // ── Resolve the register BAR (BAR2: mem 0x1000 per the NET-3 sizing). Handle 64-bit type. ──
        let bar2 = unsafe { core::ptr::read_volatile((dev + CFG_BAR2) as *const u32) };
        if is_poison(bar2) || bar2 == 0 {
            serial_println!("{}   BAR2 = {:#010x} — unimplemented/absent register BAR; bring-up SKIPPED ::", P4, bar2);
            return;
        }
        if bar2 & 1 == 1 {
            serial_println!("{}   BAR2 = {:#010x} is I/O-space (expected memory) — bring-up SKIPPED ::", P4, bar2);
            return;
        }
        let is_64bit = (bar2 >> 1) & 0x3 == 0x2;
        let base_lo = (bar2 & !0xf) as u64;
        let bar_base = if is_64bit {
            let bar3 = unsafe { core::ptr::read_volatile((dev + CFG_BAR3) as *const u32) };
            ((bar3 as u64) << 32) | base_lo
        } else {
            base_lo
        };
        // BAR2 is firmware's assignment: a PCIe BUS address (the M1 FAULT's root fact). NOT a CPU PA.
        let bar_pci = bar_base;
        serial_println!(
            "{}   register BAR2 = {:#x} ({}-bit {}) — this is a PCIe BUS address (needs outbound iATU translation) ::",
            P4, bar_pci, if is_64bit { "64" } else { "32" },
            if (bar2 >> 3) & 1 == 1 { "prefetchable mem" } else { "mem" }
        );
        if bar_pci == 0 {
            serial_println!("{}   BAR2 base is 0 (firmware left it unassigned) — bring-up SKIPPED ::", P4);
            return;
        }
        let bar_size = 0x1000usize;

        // ── M1-FIX: outbound iATU + PCIe->CPU aperture translation (the fault-forward for FAULT-AT-M1) ──
        // The old path mapped `bar_pci` as a CPU PA and wrote the first register there — into a Tegra
        // carveout (RAS Uncorrectable). Instead: resolve controller-0's `ranges` MEM window + `atu_dma`
        // ATU base from the DTB, program an outbound iATU region for that window, then access the
        // CPU-SIDE aperture address. REFUSE (clean skip) on any unresolved piece — never deref the raw
        // BAR again.
        let Some((atu_base, win)) = resolve_atu_and_window(dtb_addr, dtb_size, ram_gib_mask, bar_pci) else {
            serial_println!(
                "{}   M1-fix: no `ranges` MEM window / `atu_dma` base covers BAR2 {:#x} in the DTB — bring-up SKIPPED (REFUSE: will NOT deref a raw PCIe BAR as a CPU address, the FAULT-AT-M1 class) ::",
                P4, bar_pci
            );
            return;
        };
        serial_println!(
            "{}   M1-fix: BAR2 {:#x} in ranges MEM window PCIe [{:#x}..{:#x}) -> CPU base {:#x}; ATU base {:#x} ::",
            P4, bar_pci, win.pci_base, win.pci_base + win.size, win.cpu_base, atu_base
        );
        // The ATU register block is GiB-0 (already mapped); idempotent-map it Device-nGnRE to be safe.
        match map_mmio_window(atu_base, 0x1000) {
            MmioMap::Mapped | MmioMap::AlreadyMapped => {}
            MmioMap::BeyondPsCeiling => {
                serial_println!("{}   M1-fix: ATU base {:#x} unmappable — bring-up SKIPPED ::", P4, atu_base);
                return;
            }
        }
        // Program outbound region 0 for the whole MEM window (announced writes; enabled last).
        program_outbound_atu(atu_base, 0, &win);

        // ── NET-4h: INBOUND iATU — the missing PCIe->DRAM DMA translation ──────────────────────────
        // The outbound region above only lets the CPU REACH the NIC's registers. It does NOT let the
        // NIC (a bus master) REACH DRAM: an incoming write TLP needs an INBOUND region to be translated
        // to a DRAM physical address. Without it, the NIC's DMA rides only a firmware-residual inbound
        // mapping — enough for the descriptor ring + the first RX buffer, but every later payload write
        // lands nowhere (the NET-4d/f/g "first frame real, rest real-length/zero-payload" no-lease
        // signature). Program inbound region 0 as an IDENTITY DRAM window (PCIe addr == DRAM PA, the
        // driver's identity-map ring/buffer contract), covering all of RAM so any heap buffer the NIC
        // is handed is reachable. If the DRAM window can't be resolved, DO NOT proceed to arm the rings
        // blind — refuse cleanly (the NIC would DMA into a black hole again).
        let Some((dram_base, dram_size)) = dram_window(ram_gib_mask) else {
            serial_println!(
                "{}   [net4h] no RAM in ram_gib_mask ({:#x}) — cannot program the inbound DMA window; bring-up SKIPPED (the NIC could not reach DRAM) ::",
                P4, ram_gib_mask
            );
            return;
        };
        serial_println!(
            "{}   [net4h] inbound DMA window from ram_gib_mask {:#x}: DRAM [{:#x}..{:#x}) ({} GiB) — programming inbound iATU so the NIC can DMA into any heap buffer ::",
            P4, ram_gib_mask, dram_base, dram_base + dram_size, dram_size >> 30
        );
        program_inbound_atu(atu_base, 0, dram_base, dram_size);

        // ── ORIN-DMA-WINDOW (UNAOS_DMAWIN probe): one-shot confirmation that the DERIVED inbound window
        //    (from the RC's dma-ranges) agrees with what NET-4h just programmed. Read-only + knob-gated
        //    (default-quiet law); the next metal boot cross-checks the heap-guard's derivation against
        //    the live iATU registers. Compiled in always (net4-gated already) but silent unless armed. ──
        if option_env!("UNAOS_DMAWIN").is_some() {
            dmawin_probe(dtb_addr, dtb_size, atu_base, dram_base, dram_size);
        }

        // ── NET-4i: the SMMU stream for PCIe controller-0 — the layer BELOW the inbound iATU ─────────
        // The inbound iATU (above) is the DWC controller's internal PCIe↔fabric translation. AFTER it,
        // an inbound write TLP is presented to the Tegra234 ARM MMU-500 (SMMUv2) carrying controller-0's
        // stream id (from the DTB `iommu-map`). NET-4h armed the iATU and the RX payload blackhole
        // survived — the signature (writebacks + first payload land, rest vanish silently) is exactly a
        // stale/partial firmware SMMU context. Recon the live stream state, then arm per-stream BYPASS
        // (identity DMA: PCIe addr == DRAM PA) so the NIC's writes reach the heap buffers untranslated.
        // Fully data-driven off the DTB; fail-closed on poison. Non-fatal — a resolve miss leaves the
        // SMMU as firmware left it and the bring-up proceeds (recon lines say what was — or wasn't —
        // seen), so this never regresses the NET-4h path.
        match crate::arch::aarch64::fdt_tegra::pcie_iommu(dtb_addr, dtb_size, ram_gib_mask) {
            Some(iom) => {
                let sm = &iom.bases[..iom.n_bases];
                crate::arch::aarch64::smmu_tegra::net4i_recon(sm, iom.sid, "pre-fix");
                crate::arch::aarch64::smmu_tegra::net4w_verdict(sm, iom.sid);
                // NET-4w: boot-30 refuted bypass (option b) — the blackhole survived it, matching
                // the MB2 "SMMU external bypass disable" fabric policy (untranslated output is
                // refused). Promote option (a): a stage-1 IDENTITY context we own. Translated
                // output clears the fabric policy while keeping bus addr == DRAM PA. If translate
                // fails closed on every instance, fall back to the NET-4i bypass arm so boot-30
                // behaviour is never regressed.
                if crate::arch::aarch64::smmu_tegra::net4w_translate(sm, iom.sid) == 0 {
                    serial_println!(
                        "{}   [net4w] translate armed 0 instances — falling back to the NET-4i bypass route ::",
                        P4
                    );
                    crate::arch::aarch64::smmu_tegra::net4i_bypass(sm, iom.sid);
                }
                crate::arch::aarch64::smmu_tegra::net4i_recon(sm, iom.sid, "post-fix");
                crate::arch::aarch64::smmu_tegra::net4w_verdict(sm, iom.sid);
            }
            None => {
                serial_println!(
                    "{}   [net4i] PCIe controller-0 SMMU stream unresolved in DTB — SMMU left as firmware set it; NET-4h inbound-iATU path unchanged ::",
                    P4
                );
            }
        }

        // The CPU-side aperture address for BAR2 = cpu_base + (bar_pci - pci_base). This — NOT bar_pci —
        // is what the CPU dereferences; the iATU forwards it to PCIe. It sits ~200 GiB up, inside the
        // PS-widened 40-bit / 512-GiB-table reach.
        let cpu_addr = win.cpu_base + (bar_pci - win.pci_base);
        match map_mmio_window(cpu_addr, bar_size) {
            MmioMap::Mapped | MmioMap::AlreadyMapped => {
                serial_println!(
                    "{}   BAR2 CPU aperture {:#x} (+{:#x}) mapped Device-nGnRE — registers reachable via iATU ::",
                    P4, cpu_addr, bar_size
                );
            }
            MmioMap::BeyondPsCeiling => {
                serial_println!(
                    "{}   BAR2 CPU aperture {:#x} BEYOND the PS ceiling — cannot map register window; bring-up SKIPPED ::",
                    P4, cpu_addr
                );
                return;
            }
        }

        // ── Construct the driver at the CPU aperture (never the raw BAR value) ──
        let mut nic = Rtl8168 {
            mmio_base: cpu_addr,
            mac: [0; 6],
            // NET-4r: the ATU base + the identity inbound window (NET-4h just programmed) + controller-0's
            // PCIe MEM/BAR window — the inputs `arm_dma_aliases` needs to place + collision-check the
            // inbound-iATU alias regions once the (high-heap) ring/buffer PAs are known in `init_rings`.
            atu_base,
            dma_ident_lo: dram_base,
            dma_ident_hi: dram_base + dram_size,
            mem_lo: win.pci_base,
            mem_hi: win.pci_base + win.size,
            nc_base: 0,
            rx_ring: core::ptr::null_mut(),
            rx_buffers: core::ptr::null_mut(),
            rx_cur: 0,
            rx_count: 0,
            tx_ring: core::ptr::null_mut(),
            tx_buffers: core::ptr::null_mut(),
            tx_cur: 0,
            tx_count: 0,
            tx_stalled: 0,
            d_xid: None,
            rxcls_full: 0,
            rxcat: [0; RXCAT_N],
            rxcls_active: true,
            dhcp_tx_witnessed: 0,
            net4t_other_dumped: 0,
            net4y_probes: 0,
            net4z_probes: 0,
            net4a_prev_land: -2,
            net4a_prev_slot: -2,
            net4a_run: 0,
            net4a_fired: false,
        };

        // ── M2/M3 GUARD: poison-honest readback through the NEW window BEFORE any register write ──
        // The lesson of FAULT-AT-M1 (and V3D-2) made law: every new MMIO window earns a probe READ
        // before its first WRITE. A live RTL8168 returns a plausible TCR (chip-version bits); poison
        // (open-bus / carveout `a5a5a5a5` / firmware fill) means the iATU/link is not delivering — so
        // we REFUSE cleanly, and the next sitting can never fault on the first write again.
        let Some(tcr_probe) = nic.probe_alive() else {
            serial_println!(
                "{}   M1-fix readback: TCR through the iATU aperture = POISON (open-bus/carveout/absent) — the register window is NOT live; bring-up REFUSED before any write (no first-write fault) ::",
                P4
            );
            return;
        };
        serial_println!(
            "{}   M1-fix readback: TCR = {:#010x} (live, non-poison) — register window confirmed; first write is now safe ::",
            P4, tcr_probe
        );

        // ── Reset the MAC, read the station MAC (M1) ──
        if !nic.soft_reset() {
            serial_println!("{}   MAC reset did not complete — continuing to read MAC (may be stale) ::", P4);
        }
        nic.mac = nic.read_mac();
        let macs = fmt_mac(&nic.mac);
        serial_println!(
            "{}   station MAC = {} ::",
            P4,
            core::str::from_utf8(&macs).unwrap_or("<mac>")
        );

        // ── NET-4C: audit + reconcile PCIe MPS/MRRS on BOTH sides of the controller-0 link BEFORE the
        //    rings go up (the descriptor-fetch-completion theory; unconditional `[net4C]` witness) ──
        net4c_mps_mrrs(ecam, dev);

        // ── M2: bring up the C+ RX/TX descriptor rings + init sequence ──
        if !nic.init_rings() {
            serial_println!("{} ORIN-NET-4 bring-up STOPPED after ring init failed (device stopped answering) ::", P4);
            return;
        }

        // Register the driver so the smoltcp bind + any poll path can reach it.
        let link = nic.link_up();
        *NET4_DEVICE.lock() = Some(nic);
        serial_println!(
            "{}   RTL8168 @ BAR2 PCIe {:#x} (CPU aperture {:#x}), MAC read, C+ rings up + RX/TX enabled; PHY link {} ::",
            P4, bar_pci, cpu_addr, if link { "UP" } else { "DOWN" }
        );

        // ── M3: bind a smoltcp phy::Device over the rings (the e1000/smolnet seam) ──
        bind_smoltcp();
        serial_println!("{} ORIN-NET-4 DONE — RTL8168 driver up + smoltcp bound (live traffic = attended metal) ::", P4);
    }

    /// The one registered RTL8168 NIC (populated by [`net4_bringup`]). Mirrors the x86 e1000
    /// `NET_DEVICE` registry; the smoltcp Device adapter reaches the rings through it.
    pub static NET4_DEVICE: spin::Mutex<Option<Rtl8168>> = spin::Mutex::new(None);

    // ── Static FALLBACK addressing (used only if DHCP does not lease within the bounded timeout) ──
    // NET-DHCP made the link's real subnet a DHCP input (the do-it-right fix for the NET-4-landing
    // placeholder): `bind_smoltcp` runs a DHCPv4 client first and only falls back to these values if no
    // lease arrives. They remain here as the honest last resort for a metal link with no DHCP server —
    // the interface still comes up. Documented in arch_arm64.md §ORIN-NET-4.
    const OUR_IP: [u8; 4] = [192, 168, 1, 2];
    const GATEWAY_IP: [u8; 4] = [192, 168, 1, 1];
    /// Bounded DHCP-lease timeout (ms). On a devkit link with a DHCP server the lease lands far inside
    /// this; the bound caps how long a DHCP-less link stalls before the static fallback. The clock is
    /// real time (CNTPCT), so this is non-hanging by construction.
    const DHCP_TIMEOUT_MS: i64 = 5_000;

    /// NET-4c: the discover window is knob-tunable — `UNAOS_NET4_DHCP_MS=<millis>` at build time
    /// widens (or narrows) it for an attended sitting; unset, the default 5 s stands unchanged.
    /// Invalid or zero values fall back to the default (never a hang, never a zero window).
    fn dhcp_timeout_ms() -> i64 {
        if let Some(s) = option_env!("UNAOS_NET4_DHCP_MS") {
            let mut v: i64 = 0;
            let mut any = false;
            for b in s.bytes() {
                if b.is_ascii_digit() && v < 3_600_000 {
                    v = v * 10 + (b - b'0') as i64;
                    any = true;
                } else {
                    any = false;
                    break;
                }
            }
            if any && v > 0 {
                return v;
            }
        }
        DHCP_TIMEOUT_MS
    }

    /// Monotonic millisecond clock from the free-running counter (CNTPCT). Readable at EL2, where
    /// `net4_bringup` runs (before the JC3 EL2→EL1 drop); drives both smoltcp time and the DHCP timeout.
    #[inline]
    fn now_ms() -> i64 {
        let (cnt, frq): (u64, u64);
        unsafe {
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
        }
        if frq == 0 { 0 } else { (cnt.wrapping_mul(1_000) / frq) as i64 }
    }

    // ── Raw L2 accessors over the NET4_DEVICE registry (the shared smoltcp Device seam) ──

    /// Pop one raw RX frame for the smoltcp Device. Short-locks NET4_DEVICE per ring op (the poll must
    /// not hold the lock across a transmit) — the e1000 `raw_rx` discipline.
    fn raw_rx(out: &mut [u8]) -> Option<usize> {
        NET4_DEVICE.lock().as_mut().and_then(|n| n.rx_frame_raw(out))
    }
    /// Transmit one raw L2 frame from the smoltcp Device. Short-locks NET4_DEVICE.
    fn raw_tx(frame: &[u8]) {
        if let Some(n) = NET4_DEVICE.lock().as_mut() {
            n.transmit(frame);
        }
    }
    /// Link-up snapshot for the interface witness. `false` if the NIC never came up.
    fn link_up() -> bool {
        NET4_DEVICE.lock().as_ref().map(|n| n.link_up()).unwrap_or(false)
    }

    // ── The RawNic seam: the shared `net_phy::SmoltcpPhy` moves L2 frames through these ──
    struct Rtl8168Nic;
    impl RawNic for Rtl8168Nic {
        fn rx_frame_raw(out: &mut [u8]) -> Option<usize> {
            raw_rx(out)
        }
        fn transmit(frame: &[u8]) {
            raw_tx(frame)
        }
        fn mac() -> Option<[u8; 6]> {
            NET4_DEVICE.lock().as_ref().map(|n| n.mac)
        }
    }

    // ── smoltcp interface plumbing (the phy::Device itself is the shared `net_phy::SmoltcpPhy`) ──

    use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
    use smoltcp::socket::icmp;
    use smoltcp::time::Instant;
    use smoltcp::wire::{EthernetAddress, HardwareAddress};

    /// Bind a smoltcp `Interface` over the RTL8168 Device and poll it a bounded number of times — the
    /// x86 e1000/smolnet seam, transposed to aarch64/tegra. This PROVES the bind end-to-end (Device +
    /// Interface + ICMP socket construct and poll without fault); on real Orin silicon it drives ARP
    /// for the gateway. In QEMU there is no Tegra234 RC (so this metal path is never reached on virt),
    /// and on metal-pre-subnet-config the poll simply finds an empty ring — the honest pre-metal state.
    /// All storage is stack-local (no heap growth), mirroring `smolnet::pump`.
    fn bind_smoltcp() {
        let Some(mac) = Rtl8168Nic::mac() else {
            serial_println!("{}   smoltcp bind SKIPPED — no NIC registered ::", P4);
            return;
        };
        let up = link_up();
        let mut dev = SmoltcpPhy::<Rtl8168Nic>::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = 0x4e45_5434; // ASCII "NET4"
        let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));

        // NET-4l: knob-gated PRE-WINDOW full-ring snapshot — the baseline the after-rx dumps diff against
        // (brief item 3: dump all 32 descriptors BEFORE the window). Read-only; default-quiet.
        if option_env!("UNAOS_NET4_RINGDUMP").is_some() {
            if let Some(n) = NET4_DEVICE.lock().as_ref() {
                n.net4l_ring_dump("pre-window");
            }
        }

        // ── DHCP first: acquire a lease for the link's real subnet, else fall back to the static
        //    placeholder (NET-DHCP — the do-it-right fix for the NET-4-landing static bring-up IP). The
        //    helper configures the interface in place; the bounded witness poll below then exercises the
        //    seam against whichever config it settled on. ──
        let netcfg = crate::net_phy::dhcp_or_static(
            P4, &mut iface, &mut dev, &now_ms, dhcp_timeout_ms(), OUR_IP, 24, GATEWAY_IP,
        );

        // NET-4c: evidence snapshot right after the discover window — did the DISCOVER
        // actually LEAVE the NIC (TX consumed / ISR.TOK), and did anything at all land in the
        // RX ring during the window? Read-only; printed lease-or-not so both outcomes carry
        // the same evidence shape.
        if let Some(n) = NET4_DEVICE.lock().as_ref() {
            n.net4c_evidence("post-discover-window");
        }
        // NET-4d: close the RX-window classifier and emit the per-category summary (the RX-side proof
        // for the no-lease — did the OFFER arrive, and did the driver-visible accept path take it?).
        if let Some(n) = NET4_DEVICE.lock().as_mut() {
            n.net4d_window_close();
        }
        // NET-4g: decisive RX descriptor dump — is desc[i].addr what the driver programmed? This is the
        // riddle-breaker for "first popped frame real, rest real-length/zero-payload"; the code is
        // statically correct, so only the metal descriptors can discriminate corruption (in-file fix)
        // from writes-to-nowhere (SMMU/inbound iATU, below the driver). Read-only.
        if let Some(n) = NET4_DEVICE.lock().as_ref() {
            n.net4g_desc_dump();
        }

        // One ICMP socket, so the poll has a real socket set to service (proves the full seam binds).
        let mut rx_meta = [icmp::PacketMetadata::EMPTY; 4];
        let mut rx_payload = [0u8; 256];
        let mut tx_meta = [icmp::PacketMetadata::EMPTY; 4];
        let mut tx_payload = [0u8; 256];
        let rx_buffer = icmp::PacketBuffer::new(&mut rx_meta[..], &mut rx_payload[..]);
        let tx_buffer = icmp::PacketBuffer::new(&mut tx_meta[..], &mut tx_payload[..]);
        let socket = icmp::Socket::new(rx_buffer, tx_buffer);
        let mut storage: [SocketStorage; 1] = Default::default();
        let mut sockets = SocketSet::new(&mut storage[..]);
        let _handle = sockets.add(socket);

        // Bounded poll — on metal this answers inbound ARP and lets the stack drain; pre-subnet /
        // empty-ring it is a bounded no-op. NET-ARP-1: the old loop polled 4096 iterations of a FAKE
        // clock restarted at 0 — a time regression against the Interface timestamps dhcp_or_static just
        // stamped with real CNTPCT ms, and a window lasting only microseconds of real time (nothing
        // arriving after it could ever be answered). Poll with the real clock for a real-time bound.
        const PUMP_WINDOW_MS: i64 = 1_000;
        let t0 = now_ms();
        loop {
            let t = now_ms();
            if t.saturating_sub(t0) >= PUMP_WINDOW_MS {
                break;
            }
            iface.poll(Instant::from_millis(t), &mut dev, &mut sockets);
        }
        // NET-ARP-1 emission witness (counted at the phy TxToken — wire-side of the seam).
        let (txn, arp_reply, dhcp) = crate::net_phy::tx_emission_counts();
        serial_println!(
            "{}   [netarp1] smoltcp emitted {} frames (arp-reply={} dhcp={}) ::",
            P4, txn, arp_reply, dhcp
        );
        serial_println!(
            "{}   smoltcp 0.13 Interface BOUND over RTL8168: MAC set, {}.{}.{}.{}/{} + default gw {}.{}.{}.{} [{}], medium=ethernet, polled OK; link {} — live ICMP/ARP is attended-metal ::",
            P4,
            netcfg.ip[0], netcfg.ip[1], netcfg.ip[2], netcfg.ip[3], netcfg.prefix_len,
            netcfg.gw[0], netcfg.gw[1], netcfg.gw[2], netcfg.gw[3],
            if netcfg.leased { "dhcp" } else { "static" },
            if up { "UP" } else { "DOWN" }
        );
    }
}
