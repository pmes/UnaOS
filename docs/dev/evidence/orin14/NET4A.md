# NET4A — the RX ring + 32 buffers below 4 GiB: the first metal question of gap #2

Executor NET4A, seat orin 14, track `hw-jetson`, base `2a04fb4a`. Ledger rows: `docs/dev/OS/orin-ledger.md`
A12 and §F "Ethernet". Gap: orin-ledger §"The five gaps" #2 (Network RX, NET-4A).

The question the ledger poses: *with the RX ring and its 32 buffers placed below 4 GiB (no inbound-iATU
alias, which the 32-bit `addr` field forced), do four consecutive RX completions land in four distinct
buffers?* This arc builds the placement, the witness that answers it, and the scorer. It flies on the next
`UNAOS_NET4=1` jetson image with the RJ45 cabled to Peter's bench DHCP server.

## 1. Where the ring lives today, and why the alias is in the path

| fact | evidence |
|---|---|
| Rings + buffers are laid into ONE 2 MiB Normal-NC window at fixed 64 KiB offsets (rx-ring +0x0, rx-bufs +0x10000, tx-ring +0x20000, tx-bufs +0x30000). | `rtl8168_tegra.rs` `alloc_rx`/`alloc_tx` (`NC_OFF_*` constants, "NET-4B: fixed 64 KiB-aligned offsets"); the window comes from `mmu_tegra::net4b_nc_window()`. |
| That window is carved 2 MiB below the kernel heap, in GiB 9, on every boot: `[0x268000000, 0x268200000)`. | boot7h and every boot since: `:: tegra: HEAP-GUARD — kernel heap [0x2683ca000, 0x26b3ca000) (48 MiB), highest clean window (RAS-2 heuristic — NO PCIe dma-ranges in DTB …)` then `:: tegra: [net4B] Normal-NC DMA window reserved [0x268000000, 0x268200000) (2048 KiB), carved just below the heap` (`awk '/net4B\] Normal-NC/' ~/unaos-bench/capture/line-acm0/orin.log`). `mmu_tegra.rs` `select_heap_region`, the `seat` closure ("heap at the TOP of the found window, NC block 2 MiB-aligned just below"). |
| The NIC's payload TLPs truncate to `addr[31:0]` on this RC integration. | NET-4r commit `9614f3f1` (boot-24: a provably full-64 descriptor still sank at the 0x200 IOB FillWrite RAS). The `Desc.addr` field itself is `u64` and DRAM holds the full address (`[net4x] init witness … rx-desc[17].addr=0x268018800 expect=0x268018800 [MATCH]`). |
| So every DMA object rides an inbound-iATU ALIAS: bus `0x68000000..0x6804ffff` → DRAM `0x268000000` at inbound index 1. | boot7h: `[net4s] rx-buffers [0x268010000..0x268020000) needs alias: truncated bus [0x68010000..0x68020000), high dword 0x2`; `[net4s] covering window: base 0x68000000 limit 0x6804ffff target 0x268000000 (up-translate +0x200000000); spans 5 aliased block(s)`; `[net4r] alias region 1 (dma-cover) readback: … UPPER_TARGET=0x00000002 CTRL2=0x80000000 enabled=1`. Code: `arm_dma_aliases` (NET-4s, "consolidate every block that needs an alias into ONE covering region at the proven index 1"). |
| The conviction was measured WITH that alias in the path. | boot7h `[net4F] rx[1..3] … own-buffer-written=no buffers-written(count=1)=[17,-1,-1,-1]` and `[net4F] rx[0] … own-buffer-written=yes … =[0,…]`; ledger §F: "tag-proven single-address latch: 4 consecutive completions wrote ONLY buffer 17 — NIC/RC-internal". |
| The alias arming already has the "no alias needed" arm, unused until now. | `arm_dma_aliases` Case A: `pa == alias && alias >= self.dma_ident_lo && alias + len <= self.dma_ident_hi` → `[net4s] … inside the identity inbound region … identity-covered, no alias`; `dma_ident_lo` = the NET-4h identity region base = DRAM base `0x8000_0000` (`net4_bringup`, `dma_ident_lo: dram_base`). |

The DRAM on this board is `[0x80000000, 0x280000000)` (MB2 `Non-ECC region[0]: Start:0x80000000, End:0x280000000`
in every capture); GiB 2 and 3 (`[0x80000000, 0x100000000)`) are the only 2 GiB a 32-bit address can name that
the NET-4h identity region also covers.

## 2. NET-4o's "no clean sub-4 GiB span" was never a measurement

NET-4o (`24d4ed49`, 2026-07-19) seated a 256 KiB sub-4 GiB arena and boot-19 reported
`!! NET-4o: no sub-4GiB DMA arena seated (mmu_tegra HEAP-GUARD found no clean low span) — REFUSING`; NET-4p
(`4d8f2a54`, 38 minutes later) reverted it with the reading "`[2 GiB, 4 GiB)` is packed with Tegra firmware
carveouts … there is **no** carveout-clean 256 KiB span" (`arch_arm64.md` §ORIN-NET-4, NET-4p). The NET-4o
code refutes that reading:

```
git show 24d4ed49:unaos/crates/kernel/src/arch/aarch64/mmu_tegra.rs | sed -n 1066,1081p
        // Only the derived-window path seats it — the degrade path is QEMU/foreign DTB and never runs on
        // metal, where the arena is the whole point.
        let mut arena: Option<u64> = None;
        for &(wb, ws) in windows {
            let lo = NET4O_ARENA_LO.max(wb);
            ...
            if let Some(ab) = lowest_clean_in(lo, hi, NET4O_ARENA_BYTES, s, s + need) {
```

`windows` is the set of inbound windows DERIVED from the RC's `dma-ranges`, and on this board it is EMPTY
on every boot: `:: tegra: DMA-WINDOW STOP — Tegra RC node is okay but carries NO dma-ranges property = 0x0 /
0x0; inbound-DMA window NOT derivable (heap falls back to the RAS-2 heuristic)` (boot7h and all of
`orin.log`; the heap-guard line that follows says "degraded"). The loop body never ran; `lowest_clean_in`
never scanned a byte of low DRAM; the WARN fired on an empty set. The exclusion set is 157–165 ranges
(UEFI non-`Usable` + 3 DTB `/reserved-memory` nodes + the XCARVE quirks) against 2 GiB of low DRAM — the
"packed" claim has no measurement behind it. NET4A's scan runs on BOTH heap paths and prints a census
either way (§3), so the next flight measures it.

## 3. The mechanism

**Reservation** (`mmu_tegra.rs`, tail-defined `seat_net4a_low_nc`, called from both success paths of
`select_heap_region` right after the heap is seated): the LOWEST 2 MiB-aligned 2 MiB block in
`[0x8000_0000, 0x1_0000_0000)` that is (a) entirely inside one UEFI `Usable` region, (b) clear of every
carveout the heap dodges (same `carveouts` slice: UEFI non-Usable + DTB `/reserved-memory` + XCARVE quirk
slots), (c) clear of the heap span and the NET-4B high window, (d) inside a derived inbound window when
any is derived (none on this board), and (e) in a GiB whose live L1 entry is a TABLE descriptor — i.e. one
`install_carveout_holes` L2-split. GiB 2 and 3 are split on every tegra boot because the XCARVE-8 0xbe
QUIRK `[0xbe000000, 0xc4000000)` straddles them (`mmu_tegra.rs` header at `XCARVE_BE_SIZE`; XCARVE-8
"splits every GiB the span touches"). 2 MiB, not 256 KiB, because the NC flip is one L2 entry. Latched in
`NET4A_LOW_NC_BASE`, published by `net4a_low_nc_window()`.

**Mapping** (`install_nc_window(base, size)`): the NET-4B flip lifted out of `install_net4b_nc` unchanged —
`dc civac` over the window, the L2 entry rewritten with `nc_block()` (MAIR AttrIdx 2 = Normal Inner/Outer
Non-Cacheable 0x44) in the live table AND the EL1 twin when still at EL2, `clean_desc` per entry, then
`tlbi alle2`/`vmalle1` for the active regime. `install_net4b_nc()` now calls it for the high window. Refuses
(false) if the GiB is not split.

**Placement** (`rtl8168_tegra.rs` `init_rings`): `net4a_low_nc_window()` first; if it is non-zero, below
4 GiB and `install_nc_window` succeeds, `nc_base` is the low window and `net4f_below4g = true`; else the
existing `install_net4b_nc()` + `net4b_nc_window()` path runs unchanged, with a `[net4F] sub-4GiB window
NOT reserved … falling back` line. `alloc_rx`/`alloc_tx` and every NC offset (`NC_OFF_*`, the NET-4G decoy
at +0x40000) are untouched; the DMA discipline is the NET-4B one (`dma_wmb`/`dma_rmb` only, no cache
maintenance) because the attribute is the same. `arm_dma_aliases` is untouched: with every block below
4 GiB inside `[dma_ident_lo, dma_ident_hi)` its Case A fires five times and it arms nothing.

**No knob.** The driver module is `#[cfg(feature = "net4")]` and its metal half `#[cfg(feature = "tegra")]`
(`rtl8168_tegra.rs:157`, `:566`); `mmu_tegra` is `any(tegra, pcie3)`. The only board that compiles either
is the Orin, and the placement is fail-open to the exact bring-up boot7h ran. A knob would only let the
flight choose NOT to ask the question. Knob-off byte identity is by construction (the Pi `kernel8.img`
carries neither feature) and measured in §5.

## 4. The witness

At rings-up, after `alloc_rx`/`alloc_tx`:

```
[net4F] rx-ring phys=0x… buffers=32 rx-bufs=[0x…..0x…) tx-ring phys=0x… below4g=1 — every DMA object is < 4 GiB inside the NET-4h identity region: NO inbound-iATU alias in the NIC's write path …
```

Across the first four RX completions, the existing per-pop `[net4F] rx[i] … buffers-written(count=…)=[…]`
scan (every buffer that no longer carries its landing tag) is UNIONED into `net4f_distinct_mask` (bit k =
buffer k). Tags are re-stamped at every re-arm (`rearm_current_rx`), so at pop i the untagged set is
"written since last armed", and the union over pops 0..3 is the set of distinct buffers the NIC wrote for
those completions — coalesced completions (buffers written ahead of the pop that reads them) are absorbed
by the union. The summary prints on the 4th completion (so it is on the wire even if the boot dies in the
window — the 0x200 RAS class) and again at window close (the census cadence, `net4d_window_close`):

```
[net4F] distinct buffers-written(count=N)=[a,b,c,d] across the first M RX completion(s) at=completion-4|window-close below4g=B rx-ring=0x… — <verdict>
```

| reading | verdict text | meaning |
|---|---|---|
| `count=4 below4g=1` | FOUR DISTINCT buffers with NO alias in the path … NET-4A WAS THE ALIAS | per-descriptor addressing works; the buffer-17 latch was RC-side (the covering region). Next: the lease. |
| `count<4 below4g=1` (M=4) | a single-address latch with NO alias in the path ⇒ NIC/RC-internal | the alias is acquitted; next question per the ledger: the RC's inbound iATU region ordering. |
| `below4g=0` | the below-4G question was NOT asked | read the `[net4A]` census line for why no low block seated. |
| `M<4` | UNDECIDED | fewer than four completions reached the ring this window (link / cable / traffic). |

## 5. Gates (this worktree, base `2a04fb4a`)

Logs in `~/unaos-bench/scratch/orin14/net4a/` (build logs only, per the brief).

| gate | command | result |
|---|---|---|
| type-check, tegra-armed | `UNAOS_TEGRA=1 ./arroyo check` | exit 0; `kernel cfg coverage OK (45 legs)`; x86_64 + aarch64 OK; no warning from `mmu_tegra.rs` / `rtl8168_tegra.rs` (`check-tegra.log`) |
| type-check, default | `./arroyo check` | exit 0 (`check-default.log`) |
| QEMU regression | `./arroyo test-arm 60` | exit 0; `target/serial-arm.log`: `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`, 0 `KERNEL PANIC`/SError lines (`test-arm.log`) |
| armed jetson media | `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_NET4=1 ./arroyo esp-jetson` | exit 0; banner `kernel features: … deskcascade,net4,pcie3,pcie2`; `target/aarch64_esp/kernel.elf` (2,874,816 B): `grep -a -c 'buffers-written'` = 2, `'distinct buffers-written'` = 1, `'rx-ring phys='` = 1, `'sub-4GiB Normal-NC DMA window reserved'` = 1, `'NO sub-4GiB Normal-NC DMA window'` = 1, `'NET-4A WAS THE ALIAS'` = 1 |
| knob-off byte identity | `./arroyo kernel8` before and after (`kernel8-before.sha`, `kernel8-after.sha`) | `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` both — identical (the Pi image compiles neither `mmu_tegra` nor `rtl8168_tegra`) |

## 6. Scorer (next flight)

Build: `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_NET4=1 ./arroyo esp-jetson`
— `UNAOS_NET4=1` is load-bearing: the render4 image carried no `net4` (orin-ledger §"not on the wire").

Cable the RJ45 to the bench segment with Peter's DHCP server before power-on. Then, on the board-pure,
unwrapped excerpt (`L` below):

```
# Q0 — did the window seat, and where?
awk '/\[net4A\]/' $L
# expect: ":: tegra: [net4A] sub-4GiB Normal-NC DMA window reserved [0x8……, 0x8……) (2048 KiB) — lowest clean L2-split block … low-DRAM census: …"
#         a "NO sub-4GiB" line instead is a measured negative: its census fields (usable MiB, carveouts, rejected: carveout=/heap=/window=/unsplit=) say why.

# Q1 — did the rings land below 4 GiB with no alias?
awk '/net4F\] rx-ring phys=/' $L          # expect below4g=1 and rx-ring phys < 0x100000000
awk '/net4s\]/' $L                        # expect five "identity-covered, no alias" lines and "all four DMA blocks are identity-covered"; NO "[net4r] alias region 1" line

# Q2 — THE question
awk '/net4F\] distinct buffers-written/' $L
# PASS (alias convicted):  count=4 … across the first 4 … below4g=1
# FAIL (NIC/RC convicted): count<4 … across the first 4 … below4g=1
# UNDECIDED:               "across the first 0..3" (no traffic) or below4g=0

# Q3 — the lease (only meaningful on Q2 PASS)
awk '/net4V no-lease verdict|DHCP lease|\[dhcp\]/' $L
```

Expected wire on PASS, in order: `[net4A] sub-4GiB … reserved`, `[net4B] rings + buffers in Normal-NC window
[0x8……]`, `[net4F] rx-ring phys=0x8…… below4g=1`, `[net4s] … identity-covered, no alias` ×5, `[net4x] init
witness … [MATCH]`, `[net4F] rx[0..3] … own-buffer-written=yes`, `[net4F] distinct buffers-written(count=4)=[0,1,2,3]
across the first 4 RX completion(s) at=completion-4 below4g=1`, then — if the ring also wraps — a `[net4F] ring
WRAP #1` line and `[net4V no-lease verdict] … ACK-SEEN` / a lease instead of `NO-OFFER`. If the OFFER now
arrives but the ring still dies after one pass (`wraps=0`), the one-pass defect (NET-4F) is the next arc, not
this one.

## 7. Result

Unflown at commit time (metal-owed). Row status: `fixed-unflown`.
