# NET5 — the ring RE-FETCH probe: is the payload address stale inside the NIC, or is the payload never delivered?

Executor NET4B, seat orin 16, track `hw-jetson`, base `37c78ad7`. Ledger rows: `docs/dev/OS/orin-ledger.md`
A12 and §F "Ethernet". Gap: orin-ledger §"The five gaps" #2 (Network RX). Predecessor:
[`docs/dev/evidence/orin14/NET4A.md`](../orin14/NET4A.md). Flight log re-derived here:
`~/unaos-bench/scratch/orin16/net4-boot1.log` (2026-09-06, RJ45 cabled, link up gen1 x1, xid=0x541 MATCH).

## 1. What the net4 boot actually measured

The boot answered NET-4A **NEGATIVE** and refuted two hypotheses outright. Those results stand:

| result | line (awk, board-pure excerpt) |
|---|---|
| The rings + all 32 buffers sat below 4 GiB with NO alias in the path, and the latch survived. | `[net4F] rx-ring phys=0x80000000 buffers=32 rx-bufs=[0x80010000..0x80020000) tx-ring phys=0x80020000 below4g=1`; five `[net4s] … identity-covered, no alias` lines; no `[net4r] alias region 1` line; `alias_region1=0 identity_covered=5` in the scorer. **The alias is ACQUITTED.** |
| The engine stops without ever reporting descriptor-unavailable. | `[net4F] RX ring pass verdict: pops=5 NUM_RX=32 wraps=0 RDU-clears=0 RXOVW-clears=0 CR=0x0c RxEnb=1 ISR=0x0084` — **the un-serviced RDU latch is REFUTED** (NET-4F's own third arm). |
| MPS/MRRS is not the truncation. | `[net4C] MPS already coherent (both 128B …) + EP MRRS 512B ≤ 512B — NO reconcile write`, DevSta all-clear on both sides. |
| Ring DRAM held the correct distinct address for every slot, before and after. | `[net4x] init witness … rx-desc[1].addr=0x80010800 [MATCH] rx-desc[17].addr=0x80018800 [MATCH]`; the post-window `[net4g] rx-desc[0..6] … [MATCH]` dump (desc[7] is the NET-4G decoy rewrite, correctly `[ADDR-MISMATCH]`). |

## 2. The third instrument defect — the "single-address latch" does not follow from its own witness

`[net4F]`'s landing tags are re-stamped **only for the slot that just popped** (`rearm_current_rx`:
"Re-stamp the landing tag BEFORE handing the buffer back"). A buffer that never completes — buffer 17
among them — loses its tag ONCE and is then reported as "written" on **every later pop, forever**. The
per-pop scan re-reads all 32 buffers but has no way to restore a tag it did not pop.

So of the five `[net4F] rx[i]` lines, only two carry information:

```
rx[0] slot=0 len=60  own-buffer-written=yes buffers-written(count=1)=[0,…]     ← real: buffer 0 written
rx[1] slot=1 len=60  own-buffer-written=no  buffers-written(count=1)=[17,…]    ← real: buffer 17 written by now
rx[2] slot=2 len=60  own-buffer-written=no  buffers-written(count=1)=[17,…]    ← no new datum
rx[3] slot=3 len=62  own-buffer-written=no  buffers-written(count=1)=[17,…]    ← no new datum
rx[4] slot=4 len=342 own-buffer-written=no  buffers-written(count=1)=[17,…]    ← no new datum
[net4F] VERDICT tag-proven single-address latch: 4 consecutive completions … wrote ONLY buffer 17
```

The VERDICT's premise — *four* completions each *wrote* buffer 17 — is one datum printed four times.
This is the same failure family as the two the campaign has already retired: `[net4z]`'s content match
(NET-4F §"The NET-4A verdict is an artifact of its own witness") and `[net4A]`'s correlation built over
it. Nothing here impeaches the *tag* method; it impeaches the *cadence*.

The NET-4G interim line does **not** rescue the verdict either. `[net4G] interim pop slot=4 (victim=7):
L-written-again=0` was emitted in the *same pop* as `[net4G] DECOY ARMED` (log lines 1250 → 1251:
`net4g_arm` runs inside the [net4F] verdict at pop 4, `net4g_on_pop` a few statements later) — no frame
arrived between the re-stamp and the check, so `L-written-again=0` is vacuous, not a contradiction.

**What the boot DOES prove**, because the scan covers all 32 buffers on every pop: across five
completions with real writeback lengths (60, 60, 62, 342), **exactly two ring buffers were EVER written
— 0 and 17.** Three or four of those payloads reached no ring DRAM at all. That is precisely the
`count=0` reading `[net4F]`'s own vocabulary defines: *"the payload is NOT IN THE RING AT ALL … an
address REUSE cannot produce this."*

One consequence worth recording: `rx[4] slot=4 len=342` is a DHCP-OFFER-sized completion (14 + 20 + 8 +
300). The OFFER reached the MAC. `dhcp-rx: offer=0` is a *delivery* loss inside the host DMA path, not a
wire/server failure — `[net4V]`'s "wire / server / RX filter" wording is now the wrong reading of its
own data.

## 3. Candidate causes, ranked by wire evidence

| # | cause | supporting evidence | contradicting evidence |
|---|---|---|---|
| **C1** | **Inbound PAYLOAD write delivery loss.** 16-byte descriptor writebacks land per-slot for the whole window; 60..342-byte payload bursts do not, after the first. Selective by burst, not by address. | Only 2 buffers ever written across 5 real-length completions (§2). The un-artifacted NET-4m fact — "exactly ONE buffer per RxEnb receives payload" — has held across every placement the campaign has tried, incl. this boot's identity-covered sub-4 GiB one. No RDU, no RXOVW, no DevSta error latch, no 0x200 RAS this boot: the loss is silent. | Does not explain why buffer 17 in particular was written, twice, in two placements. |
| **C2** | **NIC-internal STALE payload address.** The engine emits one address it is holding, and it happens to be desc[17]'s. | Buffer 17 is the same INDEX in two placements two GiB apart — boot7h `0x268018800`, net4 `0x80018800`, both `window+0x18800`. A coincidental write cannot reproduce an index. Ring DRAM held correct distinct addrs both times, so the value is not being *read* wrong. Descriptor writeback addressing (ring base + advancing index) is provably intact while payload addressing is not — two different internal address paths, one broken. | Cannot be told from C1 by any existing instrument: with the sticky tag, "17 again" and "17 once" print identically. |
| C3 | Descriptor-fetch coherency (the descriptors are not visible to the NIC as written). | — | **Refuted, twice over.** Descriptors are in Normal-NC (MAIR AttrIdx 2, `[net4B] rings + buffers in Normal-NC window`), published with `dma_wmb` + a trailing `dsb sy` before RDSAR/RxEnb, and read back through the same NC window pre-enable (`[net4x] … [MATCH]`) and post-window (`[net4g] rx-desc[k] … [MATCH]`). There is no cache in the path to be stale. |
| C4 | RC inbound iATU region ordering (the question NET4A's manifest posed next). | Only one inbound region is enabled (`enabled-index mask = 0x0001`), so there is no ordering to get wrong. | **Refuted by this boot**: `[net4s] inbound region 1..7 … enabled=0`, `2 implemented window(s)`, everything identity-covered by region 0. A single region cannot be mis-ordered against itself. |
| C5 | Ring re-initialised while RxEnb=1 / RDSAR written late. | — | **Refuted by code + log order**: RDSAR/TNPDS are written hi-before-lo *before* `CR = RxEnb\|TxEnb` (`>>> REG WRITE (M2): RDSAR[0xe4] = 0x80000000` precedes `CR[0x37] = 0x0c` in the boot), `alloc_rx` runs long before either, and nothing rewrites the ring base afterwards. |
| C6 | RX mode / filter (early-RX, multicast hash, chip family). | — | **Refuted**: `RCR = 0x0000cf0f` with `RX_EARLY_OFF=1`, `[net4y] RCR readback … [MATCH]`, `[net4F] MAR readback = 0xffffffffffffffff [MATCH]`, `[net4F] MAC chip id … xid=0x541 … [MATCH]`. Frames *are* being accepted — five completions with real lengths prove the filter is open. |

C3–C6 are recorded as **failed under their stated conditions, code kept** (R19); they are not "ruled
out", and a later rung may need them open. The probe below separates **C1 from C2** and nothing else.

## 4. The probe (knob `UNAOS_NET5=1`, feature `net5 = ["net4"]`)

Two coupled changes, in `arch/aarch64/rtl8168_tegra.rs` § NET-5. Every `[net4F]`/`[net4G]`/`[net4z]`
instrument is kept verbatim, so both readings are scorable side by side on the armed boot.

### 4.1 SHADOW RE-POINT — `net5_arm()`, once, at the tail of `init_rings`

Called **after** `CR = RxEnb|TxEnb` and every RX-mode write, so anything the engine uses from that point
it fetched *after* enable. It rewrites `rx-desc[1..31].addr` from `rx_buffers + k*2048` to
`shadow + k*2048`, where `shadow = nc_base + 0x50000` — a 64 KiB block in the SAME Normal-NC window, at
the next free 64 KiB-aligned offset above the NET-4G decoy page. Every shadow buffer carries its own
landing tag at index `0x80 + k` (disjoint from the ring's `0x00..0x1f` and from the decoy/C-site
`0x44`/`0x43`, so no tag can be attributed to the wrong block).

* **Slot 0 is left on its own buffer**, deliberately: it is the one slot that has landed correctly on
  every boot in the record, so it stays as the control. If slot 0 stops landing, the probe itself is
  suspect.
* Each `addr` store is a single aligned 8-byte volatile write to offset 8 of a 16-byte descriptor on a
  256-byte-aligned NC ring, so a concurrent fetch sees the old or the new address **whole** — both are
  named arms (`PREFETCHED` vs the re-fetch arms).
* `rearm_current_rx` **holds** the re-point across recycles for k≥1 (a descriptor reverting to its
  original address would end the experiment mid-pass) and re-stamps the shadow tag.
* **Reachability is proven before arming**, the same way `arm_dma_aliases` proves the four DMA blocks':
  the shadow must lie inside the NET-4h identity inbound region `[dma_ident_lo, dma_ident_hi)` and the
  rings must be below 4 GiB. Otherwise it refuses and says which precondition failed. This is the
  NET-4G lesson: a decoy the fabric cannot reach fakes a null result.
* An NC direct-DRAM readback of `desc[1]` and `desc[17]` proves the rewrite landed.

**The load-bearing consequence: after the re-point, NO descriptor anywhere carries buffer 17's address.**
A payload that still lands at `rx_bufs + 17*2048` is therefore an address the NIC is holding internally.

### 4.2 RESTAMP-ALL — `net5_on_pop()`, every pop, after `net4g_on_pop`, before the re-arm

Scan ring AND shadow for lost tags, emit one verdict arm, then **re-stamp every tag in both blocks**.
Each pop's masks then mean "written **since the previous pop**". The sticky-tag artifact of §2 is
removed by construction, and a repeated landing has to be re-proven at every completion.

Side effect, recorded so it is not read as a defect: on an armed boot the `[net4F]` sets become per-pop
too, and `[net4G]` will typically never arm (its self-gate is the [net4F] run of ≥4, which the honest
cadence should not produce). `[net4V]`'s `zero-payload` counter reads the ORIGINAL ring buffer and is
distorted by the re-point by construction — `[net5V]` is the authority on the armed boot.

The driver also reads the frame from the shadow buffer when the completing slot's shadow tag is gone —
not a NET-4D-style harvest heuristic, but the address the descriptor actually carries. So a
`REFETCH-LIVE` boot can get a real lease.

## 5. Verdict table

Per pop, `[net5T] rx[i] slot=s len=L SINCE-LAST-POP shadow-mask=… ring-mask=… verdict=ARM`:

| arm | condition | what it convicts |
|---|---|---|
| `REFETCH-LIVE` | shadow[s] written, s = completing slot | The NIC re-fetched desc[s] after enable and honored the rewritten address. **Per-descriptor addressing is LIVE post-enable**; the buffer-17 latch was the instrument. RX may simply work on this boot. |
| `REFETCH-WRONGSLOT` | some shadow[j], j≠s | It re-fetched a post-enable address but resolves payloads to another slot's ⇒ **C2**, as an index defect: the address path is live. |
| `STALE-ORIG` | any ORIGINAL ring buffer written | No descriptor holds a ring address any more ⇒ the emitted address predates the re-point ⇒ **C2 CONVICTED**, as a stale internal value. Fix lane = NIC register/errata, not the iATU. |
| `PREFETCHED` | ring buffer s (its own original) written | desc[s] was already prefetched when the re-point ran. Not a site verdict; measures prefetch depth (`max s`, +1). |
| `NOWHERE` | real length, not one tag lost in EITHER block | The payload never reached this DRAM ⇒ **C1 CONVICTED**. Fix lane = the RC inbound write path. |

At window close, `[net5V] ring RE-FETCH verdict: pops-scored=N REFETCH-LIVE=… REFETCH-WRONGSLOT=…
STALE-ORIG=… PREFETCHED=… NOWHERE=… prefetch-depth=… — <ranked answer>`. Decision order:
`REFETCH-LIVE` > `STALE-ORIG` > `REFETCH-WRONGSLOT` > (`NOWHERE` with no `PREFETCHED`) > MIXED;
`pops-scored=0` is UNDECIDED (no traffic).

### Absence is a verdict

| what is missing | reading |
|---|---|
| no `[net5R] ARMED` line, but a `[net5R] NOT ARMED` line is present | The probe refused a precondition it prints (below4g=0, or shadow outside the identity inbound region). **UNDECIDED, never FAIL** — read the `[net4A]` census for why no low block seated. |
| `[net5R] … re-point readback … MISMATCH — probe VOID` | The rewrite never reached DRAM. No landing arm may be read off this boot. |
| no `[net5R]` line at all, while `[net4F] rx-ring phys=` is present | The knob is not in the built image. A **BUILD** fault, never a hardware verdict — check the `effective-features` banner for `net5`. |
| `[net5V]` present with `pops-scored=0` | The probe armed and measured nothing: no RX completion reached the ring (link / cable / traffic). |
| no `[net5T]` line while `[net5R] ARMED` and `[net4F] rx[` lines both exist | Impossible by construction (both hang off the same pop path) — if seen, the instrument is broken, not the NIC. |

## 6. Build and scorers (next flight)

```
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 \
UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCURX=1 \
UNAOS_NET4=1 UNAOS_NET5=1 ./arroyo esp-jetson
```

(render7's knob line MINUS `UNAOS_TCUPROBE`, PLUS the net5 knob. `UNAOS_NET4=1` is written out even
though `net5` implies it — the net4 MANIFEST's precedent, and the render line carries no net4 at all.)

**THE RJ45 MUST BE CABLED** to Peter's bench DHCP segment before power-on; an uncabled boot answers
UNDECIDED, not FAIL. On the board-pure, unwrapped excerpt (`L`), with `awk` and never `grep`:

```
# Q0 — did the probe arm, and did the rewrite reach DRAM?
awk '/\[net5R\]/' $L
#   ARMED + "[MATCH]"        -> the probe is live; read Q2.
#   ARMED + "MISMATCH"       -> VOID.  "NOT ARMED"  -> UNDECIDED (the line names the precondition).
#   no line at all           -> BUILD fault (check the banner for net5), not a hardware verdict.

# Q1 — the placement, unchanged from net4 (the probe is only meaningful below 4 GiB)
awk '/net4F\] rx-ring phys=/' $L        # expect below4g=1 and rx-ring phys < 0x100000000
awk '/net4s\]/' $L                      # expect "identity-covered, no alias"; NO "[net4r] alias region 1"

# Q2 — THE question
awk '/\[net5T\]/' $L
awk '/\[net5V\]/' $L
#   STALE-ORIG >= 1                     -> C2: a NIC-internal STALE payload address.
#   REFETCH-WRONGSLOT >= 1              -> C2 as a post-enable index/address reuse.
#   NOWHERE >= 1 and PREFETCHED == 0    -> C1: inbound payload-write delivery loss.
#   REFETCH-LIVE >= 1                   -> neither: per-descriptor addressing works post-enable.
#   pops-scored=0                       -> UNDECIDED (no traffic reached the ring).

# Q3 — the lease (meaningful only when Q2 is REFETCH-LIVE)
awk '/net4V no-lease verdict|DHCP lease|\[dhcp\]/' $L

# Q4 — the retained net4 readings, now on the honest cadence (side-by-side control)
awk '/net4F\] rx\[|net4F\] distinct buffers-written|RX ring pass verdict/' $L
```

## 7. Gates

Run in this worktree, base `37c78ad7`. Logs in `~/unaos-bench/scratch/orin16/net4b/`.

| gate | command | result |
|---|---|---|
| type-check, default | `./arroyo check` | see `PROGRESS.md` |
| QEMU regression | `./arroyo test-arm 60` | see `PROGRESS.md` |
| armed jetson media | the §6 build line | see `PROGRESS.md` |
| knob-off byte identity | `./arroyo kernel8` before/after | see `PROGRESS.md` (`kernel8-before.sha`, `kernel8-after.sha`) |

## 8. Result

Unflown at commit time (metal-owed; QEMU models no Tegra234 RC, so this boot is the only verdict that
will exist). Row A12 status: `fixed-unflown`, question re-posed.
