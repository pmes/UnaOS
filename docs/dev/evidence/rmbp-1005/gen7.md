# GEN7 (B411, R101) — the Intel HD 4000 as the acceleration route

Branch `exec-rmbp-gen7`, cut from 77db25d2. Method: `docs/dev/DRIVERS-METHOD.md` §6. The ladder of record is
`docs/dev/OS/08_VIDEO/gen7.md` (R1–R8, carried forward, not restarted); this file is the arc's design and its
hand-off.

**Finding (read before building).** Most of item (1) already exists on this tree: GPUTESTS (B334) moved R1–R7
behind `tests gen7` (`gen7::ladder`), the boot calls only `gen7::bank` (reads, prints nothing), and the knob is
wired in arroyo (`UNAOS_IVB3D` → `gen7`, `UNAOS_IVB3D_R8` → `gen7,gen7r8`) and the builder (main.rs:981/991);
the K8 row for `UNAOS_IVB3D` was still `TODO`. Flights 24/25 carry NO gen7 line (the knobs are off the card
image's line since QUIETBOOT dropped them, FLIGHT21.md: gen7 printed 390 boot lines then); the only IVB lines are
the `igpu-dpy` ladder (`gmux_igd`), whose rungs 03/07 print `pp_write=DECLINED why=pp-bits-uncited` although
rung 07b of the SAME ladder already decodes every PPS field with its PRM section and page (IHD-OS-V3 Pt4 §2.4,
pp.38–43). So (4) is a stale token, not a missing citation. The Intel PRM PDFs are NOT reachable from this
session (x.org, cdrdv2-public.intel.com and 01.org answer 403 at the proxy), so no NEW command or register can be
pinned here: (3) is parked with its reopening condition, and every write this arc adds reuses R7/R8's
metal-exercised, already-pinned encodings.

## 1. Capture

- PPS, firmware state as the Kepler-driven boot leaves it (flight 24 card 1–3 and flight 25, identical):
  `pp_ctl=0xABCD0008 pp_sts=0x00000000 on_delays=0x00000000 off_delays=0x00000000 div=0x00186904`
  (`f24-boots.log`, `awk 'index($0,"rung=07b")'`). Decoded at rung 07b: key ABCD, VDD forced, target OFF,
  T3/T9/T10 unprogrammed, T12 300 ms, refclk 125 MHz.
- GGTT, firmware's: `ggtt0=0x8BA00003 ggtt1=0x8BA01003 bdsm=0x8BA00001` (rung 00, same files) — R4b's
  scratch-fill shape, unchanged since flight 4.
- BCS/RCS engine state after firmware: the R1 census and R8's `ring-census engine=BCS entry_*` words (flights
  10–19, gen7.md §2.8). Not re-captured on the card image: the first `tests gen7` boot is that capture.

## 2. Rung ledger (carried from gen7.md §2; new rows marked)

| Rung | Hypothesis | Writes | Discriminator (confirm / refute) | Status |
| --- | --- | --- | --- | --- |
| R1–R4b | GT fabric reachable, GGTT round-trip | ≤3 PTEs, restored | gen7.md §2.1–2.4 | confirmed flight 4 |
| R5/R6 | RCS RING_CTL latches only under a held wake | RCS ring regs + 2 PTEs | `r6-sentinel-hit` / `enable-void` | confirmed flight 4 (`r6-sentinel-hit by=mt attempts=1/3`) |
| R7 | BCS XY_SRC_COPY_BLT copies 1 KiB | BCS ring + 3 PTEs | `r7-blit-verified` 256/256 | confirmed flight 4 |
| R8 | the same blit at the panel pitch, multi-page | BCS ring + 265 PTEs | `r8-fb-blit-verified` | confirmed 13/13 (flights 10–19); **open on the card image** (never flown there) |
| **R0-GEN7** (premise of item 2) | a sysmem→sysmem BCS copy at panel width beats the CPU copy of the same bytes | none new | `[gen7] band … gpu_us= cpu_us=`: gpu < cpu confirms; gpu ≥ cpu refutes → the route is 3D, not blit | **open** — measured by M2 on the first `tests gen7` boot |
| **R8b band** (new) | R8's verified window carries a full-width 32-row copy inside itself (rows 0..31 → 32..63), same encodings | BCS ring dwords 16..31, TAIL 0x40→0x80 | `band_match=<n>/<n> sentinel=1 head=0x80` / partial, `sentinel=0`, head stuck | **open** |
| **R9 rcs-3d** (new) | the RCS takes PIPELINE_SELECT + STATE_BASE_ADDRESS + 3DPRIMITIVE and draws a flat triangle | — | `:: GEN7-3D: tri=ok pixels=<n> us=<n> ::` | **parked**: no PRM page for any 3D command is in reach (proxy 403); the ring half is R6 (confirmed). Reopens when the Vol2/Vol4 PDFs are in the scratchpad or the bench (§4 lists the walls). |
| **PPS-W** (new, item 4) | PP_CONTROL/PP_ON/PP_OFF bits are cited, so the decline is no longer "uncited" | none (declined) | `pp_write=DECLINED why=panel-not-handed-to-igd bits=cited` | **done (wire token)**; the write itself parked on two legs: the gmux keeps the panel with the Kepler (a GMUX arc, Peter's word), and t_source=NONE (firmware delays read 0, EDID/DPCD carry no T values) |

Alternatives for R0-GEN7: (a) BCS faster but the composite still needs the CPU's present into the Kepler surface,
so the win is `cpu_us - gpu_us` minus nothing only if KCOMP composites in sysmem first — the line prints both
legs, the seat decides; (b) the BCS is slower because the GGTT walk over 4 KiB pages dominates — would show as
gpu_us scaling with pages, not rows (one boot cannot split that; noted, not built).

## 3. This boot's tree (`tests gen7`, knobs `UNAOS_IVB3D=1 UNAOS_IVB3D_R8=1`)

Boot: one line `[gen7] arm=banked … ladder=tests-gen7` (R80/R87 arming decision; nothing written).
`tests gen7`, in order, every candidate non-interfering (one ring, sequential, one hold):
1. `:: GEN7LADDER: … r7=<v> -> PASS|FAIL ::` — if FAIL with `r7=enable-void`, the card image's wake differs from
   flights 4–19 (read R3's line); stop reading.
2. `:: GEN7R8: window=… ring_advanced=1 csum=match … -> PASS ::` — if FAIL, the panel geometry differs (read
   `r8 encoding pitch_bytes=`; flights 10–19 read 16384).
3. `[gen7] band rows=32 w=<fb_w> pre_match=0 band_match=<n>/<n> sentinel=1 head=00000080 idle=1 gpu_us= cpu_us=`
   — pre_match must be 0 (else the seed collided: the reading is void); band_match full + sentinel=1 confirms R8b.
4. `:: GEN7BLIT: band=<w>x32 bytes= gpu_us= cpu_us= proj_gpu_us= proj_cpu_us= at=<w>x<h> match=1 -> candidate=bcs|cpu ::`
   — decides R0-GEN7.
5. `:: GEN7: ring=bcs r8=PASS blits=3 us=<n> ::` — the arc's witness (R7 + R8 + band verified).

## 4. Walls known and unapplied (with sources)

- **3D (R9)**, in PRM order — each needs its page before it is written: PIPELINE_SELECT (Vol2 / Vol4 Pt1, 3D
  pipeline select), STATE_BASE_ADDRESS (10 dwords on gen7, Vol4 Pt1), the URB allocation (3DSTATE_URB_VS and the
  push-constant allocs), VS/HS/TE/DS/GS/STREAMOUT disabled, CLIP, SF + SBE, WM + 3DSTATE_PS **with an EU kernel**
  (the pixel shader is EU ISA, Vol4 Pt3 — the largest wall: a flat colour still needs a 4–6 instruction PS that
  writes the render target through a render-target-write message), CC/BLEND state, the binding table + a
  SURFACE_STATE for the sysmem render target, a vertex buffer + VERTEX_ELEMENTS, then 3DPRIMITIVE. Source: the
  IVB PRM Vol2 Pt1 (command streamer) and Vol4 Pt1–3 (3D, shared functions, EU ISA); not in reach this session.
- **KCOMP wiring** of the BCS: only if R0-GEN7 confirms; the hot path would composite into a GGTT-mapped sysmem
  surface and the CPU presents it to the Kepler — `video/blitter.rs` `GpuBlitter` is the Kepler CE's slot today
  (GPUBLIT3, B410, in parallel), so a second candidate needs a `Blitter` impl, not a swap.
- **PPS write**: PP_ON_DELAYS[31:30] must read 01b (DisplayPort A) before the key (§2.4.3 p.41's workaround note,
  rung 07b's `port_sel_conflict`), and real T3/T9/T10 values — from the panel's datasheet or a capture of macOS's
  sequencer state on the iGPU (none exists), never zero.

## 5. Constants table (written by this arc)

| Constant | Value | Sources |
| --- | --- | --- |
| XY_SRC_COPY_BLT DW0, BR13 depth/ROP, MI_FLUSH_DW, MI_STORE_DATA_IMM | R7/R8's, unchanged | IVB PRM Vol1 Pt4 §1.9.14 pp.62–63, §2.2.5 pp.137–139, Vol1 Pt3 §1.2.17 p.186 (in gen7.rs) · metal flights 4 and 10–19 (capture) |
| X2/Y2 exclusive | band X2=fb_w, Y2=64 | PRM (as R7 pins it) · capture: R8 `spill=0` on 13 boots |
| BCS ring TAIL 0x80 | 32 dwords, QWord aligned | Vol1 Pt3 §1.1.11.1 p.75 (tail QWord rule) · R8's 0x40 on metal |
| BR11 source pitch = dst pitch | ≤ 65535 (16-bit) | Vol1 Pt4 §1.9.14 p.63 · R8's own `r8-refused-pitch-too-wide` guard |
| Band sentinel / seeds | arbitrary test patterns | not hardware constants |
| PPS bits (no write) | — | IHD-OS-V3 Pt4 §2.4.2–2.4.5 pp.39–43 · capture `pp_ctl=0xABCD0008` (VDD force + key, as §2.4.2 p.40 describes) |

## 6. Unwind

The band adds no register and no PTE: it runs inside R8's armed window, under R8's hold, before R8's teardown,
which restores BCS START/CTL/HEAD/TAIL to their entry images and releases the hold as before. The band ANDs its
own drain into R8's `ring_idle`, so the GEN7TLB reclaim gate (pages `reclaim=held`) still sees a quiesced engine
only when the band drained too. Nothing destructive. PPS: no write.

## 7. What the next flight reads (knobs `UNAOS_IVB3D=1 UNAOS_IVB3D_R8=1`; type `tests gen7` after login)

`awk 'index($0,"[gen7] arm=")||index($0,"GEN7LADDER")||index($0,"GEN7R8")||index($0,"[gen7] band")||index($0,"GEN7BLIT")||index($0,":: GEN7:")'`,
in that order, read with §3's tree. Expected on a healthy boot:
`:: GEN7: ring=bcs r8=PASS blits=3 us=<n> ::` and `:: GEN7BLIT: … -> candidate=<bcs|cpu> ::` — the latter
confirms or refutes R0-GEN7, and a `candidate=cpu` closes the blit route (the gen7 route is then 3D, R9).
`igpu-dpy rung=07 … pp_write=DECLINED why=panel-not-handed-to-igd bits=cited` on a `gmux_igd` boot.

## Milestones

- M1 — boot arming line, the `:: GEN7: ::` witness, K8 row for `UNAOS_IVB3D` (TODO → NA with evidence).
- M2 — R8b band + R0-GEN7 measurement (`[gen7] band`, `:: GEN7BLIT:`).
- M3 — PPS: rungs 03/07 cite the bits and decline on the gmux (`why=panel-not-handed-to-igd`).
- R9 (3D) parked, walls in §4.
