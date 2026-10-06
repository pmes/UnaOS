# GEN7B (B422, R101) — the BCS as KCOMP's second Blitter, and rung 0 of the IGD display route

Branch `exec-rmbp-gen7b`, cut from df15f19d. Method: `docs/dev/DRIVERS-METHOD.md` §6. Ladder of record:
`docs/dev/OS/08_VIDEO/gen7.md` (R1–R8) and GEN7's `docs/dev/evidence/rmbp-1005/gen7.md` (R0-GEN7, R8b, R9, PPS-W),
carried forward, not restarted. Knob: `UNAOS_IVB3D_BLIT=1` → features `gen7,gen7blit` (x86 only).

**Finding (read before building).** (a) The `Blitter` trait (`video/blitter.rs`, KCOMP B321, filled by GPUBLIT
B371/GPUBLIT3 B410) has two impls, `CpuBlitter` and the Kepler CE's `GpuBlitter`; `selected()` knows only those
two. The hot path (`present_band`) is ALWAYS `Ram -> Scanout`, and on this machine Scanout is the Kepler's BAR1
VRAM (gmux `READ_DISPLAY=0x03` DIS on every flight since Boot AK). No page in-tree cites the IGD DMA-ing into
another device's BAR, so the BCS can serve the hot path only when the panel is the IGD's: a BCS that is faster
than the CPU on sysmem is still `declined` for the present while gmux reads DIS. That is the arming rule, not a
gap. (b) GEN7 (B411, sibling branch, not in this base) measures a band copy inside R8's window (`:: GEN7BLIT:`);
this arc does not depend on it — the BCS job here is its own window, same pinned encodings. (c) Flights 24/25
carry no gen7 line (knob off the card line); the only IGD display lines are `igpu-dpy` rungs 00–08 under
`gmux_igd`, whose rung 07b already decodes the PPS with V3 Pt4 §2.4 citations — the capture reuses those bits.
(d) The Intel PRM hosts are unreachable from this session (seat: 000/403); nothing new is pinned here.

**Seam.** `drivers/gpu/gen7_blit.rs` (CHARTER: Kernel — driver) is a child module of `gen7` (it uses R7/R8's own
helpers: `rd`/`wr`, `fw_acquire`/`fw_release`, `R6_CANDS`, `poll_cycles`, `clflush_range`, the GGTT constants) and
implements KCOMP's trait — the trait stays KCOMP's; `blitter.rs` gains only a second-blitter slot (`arm_bcs`,
`selected()` checks it first, `Timeout` demotes it). The display capture is a read-only fn at the tail of
`igpu.rs` (`dpy_capture`), called from `tests gen7`.

## Milestones
- **M1** — knob `gen7blit` (Cargo, arroyo env arm + aarch64 strip, builder leg, K8 NA row, banner-cert row);
  `gen7_blit.rs`: the one-shot BCS job (`bcs_job`), `BcsBlitter: Blitter`, the `blitter.rs` slot.
- **M2** — `tests gen7` tail: the self-test (512x128 into 640x136 at (8,4), heap scratch), CPU and CE legs on the
  same fixture, the arming decision, `:: GEN7BLIT2: ::` and the `:: KCOMP: ::` decision line.
- **M3** — `[igpu-dpy] capture …` (read-only, cited bits), rung ledger for the hand-off (§2), R9's PRM page list (§4).

## 1. Capture

- PPS (firmware, Kepler boot; flights 24/25 via GEN7's doc): `pp_ctl=0xABCD0008 pp_sts=0x00000000
  on_delays=0x00000000 off_delays=0x00000000 div=0x00186904` — key ABCD, VDD forced, target OFF, T3/T9/T10 zero.
- gmux (Boot AK onward): `DDC=0x02 DISP=0x03 EXT=0x21` — DDC and display on the discrete GPU, external
  Kepler-owned. Pipes: Boot AS "every iGPU pipe and plane reading 0x00000000" (gen7.rs head).
- NEW this arc, one line on `tests gen7`: `[igpu-dpy] capture` — gmux SWITCH_DDC 0x28 / READ_DISPLAY 0x11 /
  READ_EXTERNAL 0x41 (reads; the index write to port 0x7D0 is the read protocol), PIPE_CONF A/B/C, DSPxCNTR A/B/C
  (raw, bits uncited), DP_CTL_A, PCH PP_STATUS/CONTROL/ON/OFF/DIVISOR. Zero writes. This is the reference image the
  hand-off's reproduce→diff starts from. The full working state (macOS on the IGD) is NOT captured: none exists.

## 2. Rung ledger

| Rung | Hypothesis | Writes | Discriminator (confirm / refute) | Status |
| --- | --- | --- | --- | --- |
| R7/R8 | BCS XY_SRC_COPY_BLT copies, panel pitch | (gen7.md) | `r7-blit-verified`, `r8-fb-blit-verified` | confirmed flights 4, 10–19 |
| **B0** (premise) | a sysmem→sysmem BCS job of KCOMP's shape (several rects, one flush, one sentinel) beats the CPU blitter on the same bytes | none beyond B1 | `GEN7BLIT2 … us=` vs `cpu_us=` on the `[gen7blit] job` line: bcs < cpu confirms, ≥ refutes | **open** |
| **B1** job | the BCS copies arbitrary rects between GGTT-mapped heap pages: window 0x11000.., PTEs → our pages, `0x101008` flush after the PTE update, ring of 8n+8 DW | 1+64+85 PTEs (+2 neighbours read), BCS ring ×4, `0x101008` ×2, forcewake req (restored) | `[gen7blit] job … match=<n>/<n> spill=0 sentinel=1 idle=1 restored=1` / partial, spill>0, sentinel=0 | **open** |
| **B2** arm | the BCS wins KCOMP's hot path only if it is fastest AND can address the present's destination | `blitter::arm_bcs` (software) | `:: KCOMP: blitter=bcs …` / `declined why=hot-dst-on-kepler` or `slower-than-<x>` | **open** (declined by construction while gmux reads DIS) |
| **D0** (display premise) | the panel can be handed to the IGD at all | none (capture) | `[igpu-dpy] capture … owner=kepler|igd` + pipe/port/PPS words; refuted if DP_CTL_A detect[2]=0 (no eDP sink on port A) | **open** |
| D1 mux | gmux SWITCH_DISPLAY→IGD (0x02) moves the panel | gmux 0x10 (+0x28 DDC) | READ_DISPLAY 0x11 reads 0x02 after | parked: needs D2/D3 first (a moved mux with no IGD pipe is a black panel) |
| D2 PPS | PP_ON_DELAYS[31:30]=01b, real T3/T9/T10, then target ON | PCH PP ×3 | PP_STATUS[31]=1, seq none | **parked, DESTRUCTIVE**: no T-value source (EDID/DPCD carry none, firmware left 0); a wrong sequence can blank or stress the panel. Flown alone and last, never this arc |
| D3 mode | pipe timing + DP link training on DP_CTL_A[9:8] | pipe A block, DP_A | gmux8/rung 08 dry transcription | parked behind D2 |
| R9 3D | RCS draws a flat triangle | — | `:: GEN7-3D: tri=ok ::` | parked: PRM pages (§4) |

Alternatives for B0: (a) the per-job arm (forcewake + PTE claim + ring arm) dominates — `setup_us` on the job line
separates it from `bcs_us`; (b) the CE beats both — the KCOMP line carries `gpu:<us>` when the CE is armed.
Alternatives for D0: (a) the panel's eDP is wired to the discrete only — DP_CTL_A[2] detect reads 0; (b) wired to
both through the mux (the Apple design apple-gmux assumes) — detect reads 1 only when the mux points at the IGD,
so one capture with DIS cannot refute (b); the D1 boot's capture would.

## 3. This boot's tree (`UNAOS_IVB3D=1 UNAOS_IVB3D_R8=1 UNAOS_IVB3D_BLIT=1`, type `tests gen7`)

1. `:: GEN7LADDER: … r7=r7-blit-verified -> PASS ::` — else the job is gated (R19): `GEN7BLIT2 selftest=gated-r7`.
2. `[igpu-dpy] capture … owner=<kepler|igd|unreadable|no-gmux>` — owner=igd would mean the firmware handed the
   panel over (never seen); the line's pipe/port/PPS words are the hand-off's reference image.
3. `[gen7blit] job rects=2 … match=<m>/<n> spill=<s> sentinel=<0|1> idle=<0|1> restored=<0|1> setup_us= bcs_us=
   cpu_us= gpu_us=` — two rects in one ring (the multi-rect encoding is the new part); match full + spill 0 +
   sentinel confirms B1; partial with row-wrapped geometry reopens the pitch note (gen7.md §2.7).
4. `:: GEN7BLIT2: impl=bcs selftest=<ok|why> us=<n> -> <armed|declined> ::` — B0.
5. `:: KCOMP: blitter=<gpu|bcs|cpu> cand=cpu:<us>,gpu:<us|->,bcs:<us|why> hot_dst=scanout panel=<owner> -> bcs=<armed|declined(why)> ::`.

## 4. Walls known and unapplied (with sources)

- **R9's PRM pages**, for Peter to drop in the bunker (IVB 2012 PRM, `IHD-OS-*` series):
  Vol1 Part3 (render command streamer) — MI_BATCH_BUFFER_START, PIPE_CONTROL (post-sync, CS stall, RT flush);
  Vol2 Part1 (3D pipeline / "3D Media GPGPU") — PIPELINE_SELECT, STATE_BASE_ADDRESS (gen7 10-DW form),
  3DSTATE_URB_VS/HS/DS/GS and 3DSTATE_PUSH_CONSTANT_ALLOC_*, 3DSTATE_VS/HS/TE/DS/GS/STREAMOUT (disable forms),
  3DSTATE_CLIP, 3DSTATE_SF, 3DSTATE_SBE, 3DSTATE_WM, 3DSTATE_PS, 3DSTATE_VIEWPORT_STATE_POINTERS_* (CC/SF_CLIP),
  3DSTATE_CC_STATE_POINTERS, 3DSTATE_BLEND_STATE_POINTERS, 3DSTATE_DEPTH_STENCIL_STATE_POINTERS,
  3DSTATE_BINDING_TABLE_POINTERS_PS, 3DSTATE_VERTEX_BUFFERS, 3DSTATE_VERTEX_ELEMENTS, 3DSTATE_DRAWING_RECTANGLE,
  3DSTATE_MULTISAMPLE / 3DSTATE_SAMPLE_MASK, 3DPRIMITIVE;
  Vol2 Part1 state — SURFACE_STATE (render target), BLEND_STATE, COLOR_CALC_STATE, CC_VIEWPORT, SF_CLIP_VIEWPORT;
  Vol4 Part1 (shared functions) — the data-port render-target-write message descriptor;
  Vol4 Part2/Part3 (EU ISA) — instruction format, `send`, the PS thread payload, register regions (a flat-colour
  pixel shader is 4–6 EU instructions). Volume part numbers as GEN7's doc names them; confirm against the PDFs.
- **The BCS on the hot path**: the present's destination is Kepler BAR1 while gmux reads DIS (capture above). No
  in-tree page cites IGD peer DMA into another PCI BAR; the route is D1–D3 (the panel on the IGD, scanout in
  IGD-addressable memory) or a sysmem composite the CPU then presents (KCOMP's census says the present IS the cost).
- **A held BCS session** (forcewake held, window claimed across frames) would drop `setup_us` from every job; it
  needs the ring left armed across jobs and the TLB invalidate per PTE update — MI_FLUSH_DW DW0[18] "TLB
  Invalidate" is named in Vol1 Part4 §2.2.5 (gen7.rs's MI_FLUSH_DW pin) but has never flown =1: ONE-SOURCE, its
  own rung. Not this arc.
- **Page reclaim**: `0x101008` has a pinned write semantic and no read decode, so per gen7.md §2.6 it is never the
  sole reason a page goes back to the heap: the self-test's pages are `reclaim=held` (≈600 KiB once per boot;
  a replay re-prints the first result).
- **PPS write**: as GEN7's doc §4 (PP_ON_DELAYS[31:30]=01b first, real T3/T9/T10 from a datasheet or a macOS
  capture — none exists).

## 5. Constants table (written by this arc)

| Constant | Value | Sources |
| --- | --- | --- |
| XY_SRC_COPY_BLT DW0, BR13 depth+ROP, MI_FLUSH_DW, MI_STORE_DATA_IMM | R7/R8's (`0x54F00006`, `0x03CC0000`, `0x13000002`, `0x10400002`) | Vol1 Pt4 §1.9.14 pp.62–63, §2.2.5 pp.137–139, Vol1 Pt3 §1.2.17 p.186 · capture: R7/R8 on metal (flights 4, 10–19) |
| DW2/DW3 dst Y1X1 / Y2X2, DW5 src Y1X1 | per rect, Y2/X2 exclusive | §1.9.14 p.63 (layout) · capture: R8 `spill=0` 13 boots (exclusive) |
| DW6 src pitch, BR13[15:0] dst pitch | bytes, ≤ 65535 | Vol1 Pt4 §1.10.7 p.91, §1.10.9 pp.92–93 · capture R8 pitch 16384 |
| GGTT PTE | `phys | 1` (<4 GiB) | gen7.rs R4 `GGTT_PTE_VALID` · capture R4–R8 `ptes_landed=1` |
| GGTT at BAR0+0x200000 | — | [ONE-SOURCE: capture, R4–R8 `ptes_landed=1`]; refuted by `landed=0` on the job line |
| Window base slot | 0x11000 (GGTT 0x11000000, bits 31:29 zero) | Vol1 Pt3 §1.1.11.3 p.77 (RING_START 31:29) · window census reads it unowned this boot or refuses |
| `0x101008` write-any-value | 1 | Vol1 Pt3 §1.2.21 p.191 (pinned in gen7.md §2.6) |
| BCS ring regs 0x22030..0x2203C, CTL bit0 enable | — | gen7.rs `g7regs` IVB-PINNED · capture R7/R8 |
| Display reads (no write) | PIPE_CONF[31:30], DP_CTL_A[31],[30:29],[2], PP_* | V3 Pt3 §5.1.3 pp.100–103, §4.4.1 pp.82–86; V3 Pt4 §2.4.1–2.4.5 pp.38–43 (gpu_spec.md §6) |
| gmux ports/indices | 0x7C2/0x7D0/0x7D4; 0x28, 0x11, 0x41 | apple-gmux.c (igpu.rs port map) · capture Boot AK `DDC=0x02 DISP=0x03 EXT=0x21` |

## 6. Unwind

The job reads the whole window and both neighbours first and refuses unless it is all-zero or the BDSM
scratch-fill (R4b's legs); writes PTEs, flushes `0x101008`, arms the ring under a forcewake hold, then: ring
disabled and read back, ring registers restored to their entry images and re-read, forcewake released to its
entry value, PTEs restored to the fill image and re-read, `0x101008` flushed again. A ring that will not disable
or drain leaves the PTEs claimed (a live engine must not lose its pages) and the job reports `restored=0`. The
pages are held, never freed. `blitter::arm_bcs` is software and is undone by `Timeout` (demote). The capture
writes nothing. Nothing in this arc is destructive; D2 is, and is not built.

## 7. What the next flight reads

`awk 'index($0,"GEN7LADDER")||index($0,"[igpu-dpy] capture")||index($0,"[gen7blit]")||index($0,"GEN7BLIT2")||index($0,":: KCOMP:")'`
in that order, read with §3's tree. Expected on a healthy boot:
`:: GEN7BLIT2: impl=bcs selftest=ok us=<n> -> declined ::` with the KCOMP line's
`-> bcs=declined(hot-dst-on-kepler)` or `declined(slower-than-cpu)` — the first confirms B1 and records B0's
number; `owner=kepler` with `dp_a_detect=` on the capture opens D0's reading.
