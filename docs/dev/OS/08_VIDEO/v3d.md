# Pi 4 GPU — VideoCore VI (V3D 4.2)

The Raspberry Pi 4's GPU is a Broadcom **VideoCore VI** with a **V3D 4.2** render
core. The driver is `arch/aarch64/v3d.rs` behind the `v3d` cargo feature
(`UNAOS_V3D=1`, implies `baremetal` ⇒ `pi`). Bring-up — firmware power domain,
clock gate, PM/ASB bridge release, the V3D-private MMU, and the M1–M4 milestone
chain (identity/probe → MMU → clear-job → first triangle) — is documented in
[`arch_arm64.md` §PI-V3D](../01_BOOT_HAL/arch_arm64.md). The M3 clear-job produced
the **first GPU-rendered, CPU-verified pixels on Pi silicon** (boot-P4).

This document records the **V3D 4.2 hardware contract facts** proven during the
R23s1f campaign while chasing the empty-bin defect: everything the coordinate
(binning) shader and the bin control list must get exactly right for the Primitive
Tile Binner (PTB) to emit tile lists. QEMU `raspi4b` models no V3D block (returns
at `BLOCK-DOWN`), so **every fact below was settled on metal**, not in emulation.

---

## THE CT0/CT1 CLASS LAW — render lists run on CT1, never on CT0

> **This is a standing design rule of this driver, not a campaign note.** It is the outcome of the
> §49 first-kick ladder and it binds every submission path in `v3d.rs`. Read it before writing any
> code that hands the CLE a control list.

**The rule.** The V3D 4.2 CLE has two control threads and they are **not interchangeable**:

| thread | register file | class it accepts | what it is |
|---|---|---|---|
| **CT0** | `CT0CS` `0x100` · `CT0CA` `0x110` · `CT0QBA` `0x160` · `CT0QEA` `0x168` · `CT0QMA` `0x170` · `CT0QMS` `0x174` · `CT0QTS` `0x15c` | **BIN-class control lists only** | the **binner**. Feeds the PTB; consumes `TILE_BINNING_MODE_CFG`, `START_TILE_BINNING`, `NUMBER_OF_LAYERS`, state, geometry, `FLUSH` |
| **CT1** | `CT1CS` `0x104` · `CT1CA` `0x114` · `CT1EA` `0x10c` · `CT1QBA` `0x164` · `CT1QEA` `0x16c` | **RENDER-class control lists** | the **renderer**. Consumes `TILE_RENDERING_MODE_CFG`, `TILE_LIST_INITIAL_BLOCK_SIZE`, `TILE_COORDINATES`, the store/clear packets, `END_OF_RENDERING` |

Therefore, without exception:

1. **A render control list is submitted to `CT1QBA`/`CT1QEA`. Never to `CT0QBA`/`CT0QEA`.** Not as a
   probe, not as a shortcut, not "because CT0 is the one that is already armed".
2. **CT0 gets bin-class content only.** A single render-class opcode anywhere in a CT0-bound list
   wedges the thread at that byte — including at byte 0, which is the usual case because render
   lists open with `TILE_RENDERING_MODE_CFG`.
3. **The bin-memory arming (`CT0QMA`/`CT0QMS`/`CT0QTS`, `BPOS=0`) is CT0's, and it does not make CT0
   accept a render list.** Arming and class are independent axes (§49.10) and neither substitutes
   for the other.
4. **A wedge on CT0 whose list contains render-class packets is not a hardware bug and not an
   encoding bug — it is this rule being broken.** Check the list's class *first*, before opening
   `v3d_packet.xml`.

**The silicon evidence, in one paragraph.** Eight metal boots of the `UNAOS_V3D_FIRSTKICK` family
put one control list on CT0 as the boot's *first* kick — no probe frame in front of it, one variant
per boot, same block, same MMU table, same L2T config, same arena, fault-gated — and the outcome
splits perfectly on list class and on nothing else. Bin-class lists work: `empty` retired,
`emptyunarm` retired *without any bin arming at all*, `m4` walked `CT0CA` from `QBA` to `QEA`.
Render-class lists do not: `rcl`, `rclp1` and `rclp19` all froze at packet 0, and `rclp1` proved
length is not the variable when a 10-byte list froze exactly like the 106-byte one. The two head-swap
boots isolated the cause to the head *opcode's class*: boot10 prepended `NUMBER_OF_LAYERS` (op 119,
**bin**-class) to the render list and `CT0CA` advanced **exactly 2 bytes** — the encoder, the submit
and the address range are all sound — then froze on the byte where `TILE_RENDERING_MODE_CFG` begins;
boot12 prepended `TILE_LIST_INITIAL_BLOCK_SIZE` (op 126, **render**-class, **no sub-id dispatch**,
same 2-byte length, and byte-for-byte the packet that same list already carries at main-packet 5, all
three facts *measured* on the wire) and `CT0CA` advanced **0 bytes**, `at-QBA=1`, on a boot with
`mmu-fault-latched=0`. Op 126 carries no sub-id, so dispatch is exonerated; op 126 is not TRMC, so
the TRMC encoding is exonerated; the one property op 121 and op 126 share is that they are
render-class opcodes on the bin thread. The same boot's `[v3d58] xengine` line closes it from the
other side — CT1 completed a frame and landed a byte-verified store on the *same block, same MMU
table, same clock, same arena* in which CT0 refused the first byte — so no global explanation (MMU
write permission, GMP, L2T ordering, clock, AXI/QoS) survives. **CT0 is bin-only silicon-side.** Full
derivation: §49.8 (the bracket), §49.9 (`rclp<n>`), §49.10 (`rclhead`, boot10), §49.11 (`rclhead126`,
boot12 — the class verdict); what the law retracts is §49.11a; the executable proof is §49.12.

---

## 1. The bin control list requires fixed-function clip/viewport state (V3D-17)

The binner binned nothing because `build_bin_cl` emitted geometry with the hardware
clipper at **power-on-reset zeros**: the viewport scale collapsed every primitive to
a point, so the binner produced an empty-but-legal bin (the tile-alloc pool was never
written). The fix (PI-V3D-17, `ed333556`) emits five state packets after
`START_TILE_BINNING` and before `VCM_CACHE_SIZE`, every opcode/length/field bit
transcribed **verbatim** from Mesa `v3d_packet.xml` (gen 4.2) and the transform
values from Mesa's own `v3dX(emit)` fixed-function viewport code:

| Packet (opcode) | Value |
| --- | --- |
| `CFG_BITS` (96, v42) | forward + reverse facing (no cull) |
| `CLIP_WINDOW` (107) | 0, 0, 64, 64 |
| `VIEWPORT_OFFSET` (108) | centre (32, 32); fine u14.8 = 8192 |
| `CLIPPER_XY_SCALING` (110, v42) | half-extent 32 px → `8192.0f32` |
| `CLIPPER_Z_SCALE_AND_OFFSET` (111) | scale 0.5, offset 0.5 (NDC[-1,1] → [0,1]) |

State was correct after this arc, but boot-P17 still binned empty (CL 71 B consumed
clean, pool + tile-STATE still zero) — pointing the investigation at the shader's VPM
output, not the fixed-function state.

---

## 2. The coordinate-shader VPM output contract is SIX words (V3D-18/19)

Mesa's coordinate (bin) shader output contract for V3D 4.2 is **six words per
vertex**, not four:

| VPM offset | Word | Source |
| --- | --- | --- |
| 0..3 | clip `Xc, Yc, Zc, Wc` | the vec4 clip-space position |
| 4 | screen `Xs` | `f2i32(floor(Xc · 8192 / Wc))` |
| 5 | screen `Ys` | `f2i32(floor(Yc · 8192 / Wc))` |

The screen words are what the **PTB actually bins from**. `8192` = viewport
half-extent 32 · clipper XY granularity 256; `floor` is the `devinfo` `ver == 42`
branch. Screen coords are **12.4 fixed-point** (centre-relative). Our `CS_VS_WORDS`
was a 4-word passthrough that wrote only offsets 0..3 and never 4, 5, so the PTB read
zero screen coords and binned an empty-but-legal list.

- **V3D-18 (`d15f3e6c`)** was the audit that proved this against Mesa
  (`v3d_nir_lower_io.c`, `v3d_uniforms.c`, `v3d_device_info.c` ver 42,
  `v3d_packet.xml` GL Shader State Record max_ver 42) and added the witness dumping
  the 52 shader-state-record bytes + the contracted 6-word vs our 4-word emit.
- **V3D-19 (`412479ae`)** widened `CS_VS_WORDS` to emit the two screen words
  (`fmul → ffloor → ftoiz → mov vpm` at out-offsets 4, 5). Expected centre-relative
  screen coords for the test triangle: v0(-0.6,-0.6) → Xs=-4916 Ys=-4916; v1(0.6,-0.6)
  → Xs=4915 Ys=-4916; v2(0,0.6) → Xs=0 Ys=4915.
- **W = 1 simplification (loud):** the test geometry all carries `Wc = 1.0`, so
  `1/Wc = 1.0` and **no reciprocal instruction is emitted** — the transform collapses
  to `floor(coord · 8192)`. This holds **only for W = 1 geometry**; perspective
  vertices need the reciprocal.
- **VPM segment sizes are unchanged:** Mesa packs them in **sectors** =
  `align(words, 8) / 8`, so both 4-in and 6-out round to 1 sector. The `1` in the
  shader-state record was always correct; the count was never the bug (see §3).

Boot-P19 (V3D-19 aboard): pool still zero — the *count* was right but the addressing
*mechanism* was still wrong (§3).

---

## 3. On V3D 4.x, VPM output is STVPMV — NOT streamed `mov vpm` (V3D-20) ⚠

**This is the 3.3-vs-4.2 trap, and it is loud.** Widening the word count (§2) never
moved the pool because the VPM addressing **form** was wrong, not the count.

The prior shader wrote its VPM output with the **streamed VC4 / V3D-3.3** mechanism:
a `vpmsetup` to arm a VPM segment, then `mov vpm, rfN` (magic waddr VPM = 14)
auto-advancing an implicit pointer. **That path does not exist for per-vertex shader
output on V3D 4.2.** Mesa's compiler (`nir_to_vir.c` `vir_VPM_WRITE`) emits exactly
one `vir_STVPMV(c, vir_uniform_ui(c, vpm_index), val)` per output component — a
store-VPM with an **explicit integer offset** — and never `mov vpm` / `vpmsetup` in
the ver-42 VS/CS output path.

So every prior `mov vpm` (the four clip words *and* the V3D-19 screen words) wrote an
**unconfigured magic register**; the PTB read zero screen coords and binned an
empty-but-legal list. No 4→6 word-count change ever moved it because the addressing
form was wrong all along.

The fix (PI-V3D-20, `597d7ed0`): `CS_VS_WORDS` stores all six words via `STVPMV` at
explicit VPM offsets 0..5 (offsets sourced as uniforms into rf9..rf14, Mesa-faithful);
`vpmsetup` is dropped (unused on 4.x VS/CS output); `VPMWT` stays (GFXH-1684 flush).
The input side (`ldvpmv_in`) and the Xs/Ys math are the validated V3D-19 body carried
verbatim.

> The VPM output is **on-chip** and not CPU/DRAM-readable on 4.x (there is no
> `V3D_VPM` CPU window; Mesa reads VPM back only via `LDVPM` in-shader). So the
> decisive metal verdict stays "tile-alloc pool / tile-STATE go non-zero," not a VPM
> readback. Boot-P20: pool **still** zero — which set up the last unproven link (§4).

---

## 4. Proving QPU execution via performance counters (V3D-21)

After V3D-17/18/20 verified every CPU→GPU hand-off correct (clip state, 6-word
STVPMV output, all Mesa-packed) yet the tile-alloc pool / tile-STATE stayed all-zero
with the CL consumed clean and no MMU fault, the **one unproven link was QPU
execution itself**. The technique that settled it **without perturbing the shader**
(PI-V3D-21, `6471eb9b`): hardware **performance counters (PCTR)**.

Three counters are armed immediately before the bin GO and read once the bin idles:

| Counter | Source id | Meaning |
| --- | --- | --- |
| `QPU_ACTIVE_CYCLES_VERTEX_COORD_USER` | 14 | **the decisive witness** — ticks only while the QPU runs a vertex/coord user shader |
| `QPU_CYCLES_VALID_INSTR` | 16 | valid-instruction cycles |
| `CYCLE_COUNT` | 32 | total cycles (cross-check for the src↔id mapping) |

Register offsets + the SRC/EN/CLR/OVERFLOW/PCTRx programming sequence are the verbatim
Linux `v3d_regs.h` (V4 variant) + `v3d_perfmon.c` idiom; source ids are the
`enum v3d_perfcnt` index from uapi `v3d_drm.h` (ver < 71: index == hw source id,
cross-checked via `CYCLE_COUNT` = 32). A nonzero src 14 ⇒ the coordinate shader ran.

A TMU general-store witness was **rejected**: it adds QPU words (fabricated-constant
risk, §5) and makes a null result ambiguous (cannot separate "QPU never ran" from
"store mis-encoded") — the exact question the arc had to answer. PCTR is read-only
w.r.t. the shader.

**Verdict (boot-P21): SHADER RAN.** `src14 = 28`, `valid_instr = 53`. Four arcs of
ambiguity are dead: the coordinate shader executes, and the empty-bin fault is now
provably **PTB-side**.

---

## 5. The in-tree QPU packer workflow (fabricated-constant law)

**Every QPU instruction word committed to `v3d.rs` must be Mesa-packed**, never
hand-authored. The file carries **three prior convictions** for guessed register
constants; the standing law is that new words are produced by Mesa's own
`v3d_qpu_instr_pack` (ver 42) and round-tripped through unpack + repack.

The tooling lives **in-tree** at `scripts/pi-v3d*-qpu-gen.c` (one generator per arc:
`pi-v3d9-qpu-gen.c`, `pi-v3d19-qpu-gen.c`, `pi-v3d20-qpu-gen.c`, …), each built
against a fresh Mesa checkout. The retired PI-V3D-9 packer was **not gone**
(never-trash-code): it lives at `scripts/pi-v3d9-qpu-gen.c`, reproduces every
committed `CS_VS_WORDS` / `FS_WORDS` word bit-exactly, and passes the `qpu_disasm`
self-test — validating the encoder before any new words are trusted. Disassembly +
words for each arc are checked in alongside (e.g. `scripts/pi-v3d19-qpu-gen.out.txt`).

This is why the campaign could rule out "our shader words are wrong" as a hypothesis
class: the words are Mesa's own bytes, generated and self-tested, not fabricated.

---

## 6. The open wall — PTB bins nothing with the shader proven running

As of BUILD-SHA-1 (`2b330f57`), the graphics pipeline stands at a **narrowed but
open** defect. The evidence chain, each link machine-witnessed on metal:

1. **Fixed-function clip/viewport state is correct** — Mesa-verbatim packets emitted
   (V3D-17); yet pool zero.
2. **The VPM output count is correct** — 6 words, screen coords included (V3D-18/19);
   yet pool zero.
3. **The VPM output mechanism is correct** — STVPMV with explicit offsets, not the
   3.3-era streamed `mov vpm` (V3D-20); yet pool zero.
4. **The coordinate shader provably executes** — PCTR src14 = 28 (V3D-21).
5. **The bin job is not faulting** — the M4 MMU fault was proven *stale* (latched at
   `program_mmu`/M3, cleared pre-bin by V3D-15, `0667eef7`); with the pre-bin latch
   clear the bin idles with `MMU_fault = 0x0`. Pool zero + no fault = **legal idle**.

So the CL is consumed clean, the shader runs, nothing faults, and the **PTB still
emits no tile lists**. Every CPU→GPU hand-off is exonerated. The remaining candidates
are PTB-side / geometry-side:

- **Degenerate or fully-clipped geometry** — the shader runs but the transformed
  triangle bins to nothing (CL content / vertex data audit vs Mesa emit order;
  V3D-15's `dump_cl_bytes` is aboard for exactly this read).
- A PTB configuration or primitive-mode packet the bin CL still omits between the
  state packets (§1) and the draw.

The next V3D arc starts from a proven-executing shader and a fault-free, legally-idle
binner — the narrowest the empty-bin wall has ever been.

---

## 7. The VCM starvation — the bin CL wrote an illegal `VCM_CACHE_SIZE` (V3D-23)

V3D-23 audited the 71-byte bin CL packet-by-packet against the authoritative Mesa
`v3d` sources for a minimal single-triangle draw on V3D 4.2 (`v3dx_draw.c`
`v3d_start_binning` + `v3d_emit_gl_shader_state`; `vir.c` `v3d_vs_set_prog_data` /
`v3d_compute_vpm_config`; `v3d_nir_lower_io.c` `v3d_nir_setup_vpm_layout_vs`;
`v3d_packet.xml` structs). The enumeration **exonerated** the two hypotheses §6 left
open and found **one real divergence** plus one defensive omission:

**Exonerated (our CL already matches Mesa):**

- **Coordinate-shader VPM output order.** `v3d_nir_setup_vpm_layout_vs` for
  `is_coord` assigns `pos_vpm_offset = 0` (`+4`: clip `Xc,Yc,Zc,Wc` at 0..3) then
  `vp_vpm_offset = 4` (`+2`: screen `Xs,Ys` at 4,5); `zs`/`rcp_wc` are **not**
  emitted for a coord shader. `vpm_output_size = 6`. This is **exactly** the V3D-18/20
  contract — the layout is correct.
- **GL Shader State Record `min … segments required in play`.** For the non-GS path
  `v3d_compute_vpm_config` sets `As = 1`, `Ve = 0`. The record fields
  `Min {Coord,Vertex} Shader input segments required in play` are `minus_one`-encoded
  (`v3d_packet.xml`), so Mesa's `As = 1` packs to **raw 0** — which is what our record
  already writes. The `… output segments … in addition to VCM cache size` fields want
  `Ve = 0` (raw 0), also already correct. The record needs no change.
- **`separate_segments`.** `v3d_vs_set_prog_data` sets it **`false`
  unconditionally** (comment: "necessary for our VCM setup to avoid varying
  corruption"), folding `vpm_input_size` to 0. Our record leaves the separate-block
  bits clear — matches. **(Correction, V3D-25.)** This bullet *also* claimed both
  input/output segment sizes at 1 "matches" — that half was **wrong**. The fold
  zeroes the *input* size; the record's input fields must be **0**, not 1. Our record
  wrote 1, and that was a real defect — see §10.

**The defect (`VCM_CACHE_SIZE`, opcode 71).** Every prior arc emitted `Vc = 1` for both
the binning and rendering batch-count fields. Mesa **never** emits 1:
`v3d_vs_set_prog_data` computes `vcm_cache_size = CLAMP(vpm_output_batches - 1, 2, 4)`
and `v3d_compute_vpm_config` copies it verbatim (`vpm_cfg{,_bin}->Vc`). For this draw
on the Pi 4's 16 KiB VPM the value is **4** (sector = `V3D_CHANNELS 16 · 4 · 8 = 512 B`
→ 32 sectors → half 16; 1-sector output ⇒ 16 output batches ⇒ `CLAMP(15,2,4) = 4`; the
CLAMP ceiling pins any 1-sector-output shader to 4). The field's **hardware-valid floor
is 2** — Mesa's own comment: *"we can't go lower than 2 due to GFXH-1744"*. The VCM is
the vertex-cache manager that stages the coordinate shader's binned vertices for the
PTB; **`Vc = 1` starves it below the erratum floor, so the PTB never receives assembled
primitives — pool + tile-STATE stay all-zero.** This is fully consistent with every
§6 witness: the shader runs (PCTR), nothing faults, the CL is consumed clean, yet the
bin is empty. `Vc = 1` is the one CPU→GPU hand-off never re-checked against Mesa.

**The fix (PI-V3D-23).** `build_bin_cl` and `build_bin_cl_at` now emit
`VCM_CACHE_SIZE` with `Vc = 4` (const `VCM_CACHE_BATCHES`, with the full Mesa
derivation) for both fields, and add the Mesa-prologue `OCCLUSION_QUERY_COUNTER`
(opcode 92, address 0 = disable stale OQ) between `FLUSH_VCD_CACHE` and
`START_TILE_BINNING`, matching `v3d_start_binning` verbatim. Both packet layouts are
transcribed from `v3d_packet.xml` (VCM fields carry **no** `minus_one`, so the raw
field is the batch count; OQ is a single 32-bit address). No shader word changed, so
the fabricated-constant law (§5) is untouched.

**Discriminator (one boot).** QEMU `raspi4b` models no V3D, so the verdict is metal.
The M4 bin path prints `[v3d23] … VCM_CACHE_SIZE Vc=4 …`, dumps the bin CL bytes
(`dump_cl_bytes` — the OQ packet + `Vc=4` are visible), and `bin_pool_witness` reads
the tile-alloc pool head **and** the tile-STATE head after the bin idles:

- **pool / tile-STATE go non-zero** ⇒ the PTB now bins — VCM starvation was the wall;
  the M4 triangle should render and the sample-verify pass.
- **still all-zero** ⇒ VCM was not the (sole) cause; the surviving candidate is
  degenerate/fully-clipped transformed geometry (the shader runs but bins to nothing),
  read from the `cs_vpm_output_witness` expected screen coords vs the CL vertex data.

---

## 8. The screen-coordinate encoding is byte-faithful to Mesa (V3D-24)

Boot-P24 (image with V3D-23's `Vc = 4` + OQ-disable) still binned empty: pool[0..8]
and tile-STATE[0..8] all-zero, `MMU_fault = 0x0`, CL consumed clean, PCTR still
proving the coordinate shader executes. VCM starvation is thus **refuted as the sole
wall**. V3D-24 took the last CPU-side hypothesis the campaign had not settled against
the authoritative source: that our screen-coordinate **encoding/space** (the `Xs/Ys`
words the coord shader stores, and the fixed-function scale/offset that composes with
them) diverged from Mesa — a wrong scale or origin would legitimately bin the triangle
off every tile with zero faults, fitting every witness.

It does **not** diverge. Each link was checked verbatim against Mesa (compiler + genxml,
fetched this arc):

- **Coord-shader position math** — `v3d_nir_emit_ff_vpm_outputs`
  (`src/broadcom/compiler/v3d_nir_lower_io.c`) computes, for the two screen words,
  `pos = f2i32(ffloor(pos_i · viewport_scale_i · (1/Wc)))`: **scale only, NO in-shader
  offset**, floored to **.8** fixed-point (the pre-V3D-4.3 `.8`-then-internal-`.6`
  double-rounding quirk; Broadcom's prescribed fix is exactly `ffloor`). The clip words
  `Xc,Yc,Zc,Wc` go to VPM offsets 0..3, the screen `Xs,Ys` to 4..5, and `Zs`/`1/Wc`
  are **not** emitted for a coord shader — precisely our six-word STVPMV contract
  (§2/§3). Our shader body (`fmul·8192 → ffloor → ftoiz`, `1/Wc = 1` for the W=1 test
  geometry) is bit-identical.
- **`viewport_scale`** — `QUNIFORM_VIEWPORT_X/Y_SCALE = viewport.scale · 256.0`
  (`v3d_uniforms.c`); for a 64-px viewport `scale = 32`, so `32 · 256 = 8192.0` — our
  constant.
- **`VIEWPORT_OFFSET` (108)** — Mesa sets one field
  `viewport_centre_x_coordinate = viewport.translate = 32.0` and lets the packer split
  it; genxml **v41+** types it `s14.8` centre @ bit 0 (22 b) + `Coarse X` uint @ bit 22
  (10 b). `32.0` packs to fine `8192`, coarse `0` — exactly our bytes (the earlier
  `u14.8` label is harmless for the positive centre: same bits).
- **`CLIPPER_XY_SCALING` (110)** — `viewport_half_{width,height}_in_1_256th_of_pixel =
  scale · 256.0 = 8192.0f32`; **`CLIP_WINDOW` (107)** `0,0,64,64`;
  **`TILE_BINNING_MODE_CFG`** width/height minus-one `63/63`, 1 RT, 32-bit BPP,
  128 B/64 B blocks — all match.

**Numbers for our viewport** (shader stores centre-relative; PTB adds the `+32,+32`
centre from `VIEWPORT_OFFSET`):

| Vertex (NDC) | `Xs,Ys` centre-rel (.8) | Absolute px after `+VIEWPORT_OFFSET` | In `0..64`? |
| --- | --- | --- | --- |
| v0 (−0.6,−0.6) | −4916, −4916 | (12.80, 12.80) | ✅ |
| v1 (0.6,−0.6) | 4915, −4916 | (51.20, 12.80) | ✅ |
| v2 (0.0, 0.6) | 0, 4915 | (32.00, 51.20) | ✅ |

All three land squarely inside the 64×64 clip window (a single 64×64 tile), so a
correctly-fed PTB **must** write tile 0's list. The encoding and every fixed-function
packet are exonerated at the byte level; **candidates #1/#2 (wrong screen-coord
scale/origin, wrong `CLIPPER_XY_SCALING`/`VIEWPORT_OFFSET`) are dead.**

Per the fabricated-constant law (§5) no shader word was authored (no Mesa checkout /
packer this session, and none was warranted — the words are already Mesa's). V3D-24
adds a CPU-side discriminator only: `cs_vpm_output_witness` now prints each vertex's
**absolute** screen px after the `VIEWPORT_OFFSET` compose and an **INSIDE/OUTSIDE**
clip-window verdict (`[v3d24]` tag), so no future boot can re-blame the encoding.

**Where the wall now points.** The shader provably runs (§4) yet the on-grid triangle
does not bin — with the encoding exonerated, the narrowest surviving candidate is the
coordinate shader's **VPM input** (the VCD attribute fetch): if attributes do not DMA
into VPM, the `ldvpmv_in` reads collapse all three vertices to `Xc=Yc=0` → absolute
`(32,32)` for every vertex → a **degenerate zero-area primitive** the PTB legitimately
bins to nothing — fully consistent with shader-runs / no-fault / empty-pool (this is
the V3D-25 candidate). The next
arc should witness the *loaded* `Xc/Yc` (a single TMU general-store of the input reg is
now unambiguous, since §4 already proved execution) rather than the *expected* values
`cs_vpm_output_witness` prints from the CPU. The attribute record / VCD setup
(`OFF_SHADREC + 36`, stride 16, 4 values read) is the audit surface.

---

## 9. Replaying the visible battery (PI-APP-1)

The `v3d` shell command (PI-APP-1, `1e8e7a0f`) re-runs the four **visible** V3D
battery stages (M5 gradient, M6 animate, M7 multiprim, M8 blit — see
[`arch_arm64.md` §PI-V3D-11](../01_BOOT_HAL/arch_arm64.md)) on the live framebuffer
on demand, so the graphics are watchable while the system is up (the boot-time
battery flashes past before the monitor wakes). Re-entry reuses the state boot
established (power/clock/PM-ASB/MMU stay enabled, the arena stays identity-mapped)
and only re-kicks the per-stage jobs; a one-shot `V3D_REPLAY_READY` flag gates it, so
when the block never came up this boot (QEMU `BLOCK-DOWN` / any fail-closed verdict)
the app prints `stages=0` and touches no MMIO.

---

## 10. The VPM input segment size must be 0, not 1 (V3D-25)

V3D-24 exonerated the screen-coord encoding and redirected the campaign to the
**VPM input** (the VCD attribute fetch): if attributes never DMA into the VPM rows
the coordinate shader reads, `ldvpmv_in` returns zeros, all three vertices collapse
to `(0,0)`, and the PTB legitimately bins the degenerate zero-area primitive to
nothing — consistent with every witness (shader runs, no fault, CL clean, empty pool).

V3D-25 took that redirect two ways: a byte-level audit of the VCD/attribute state
against live Mesa (`v3d` compiler + genxml, fetched this arc), and the input-read
offsets the shader actually consumes. The input-read side is **correct**: Mesa's
`ntq_emit_load_input` / `ntq_setup_vpm_inputs` (`nir_to_vir.c`) issue
`vir_LDVPMV_IN(c, vir_uniform_ui(c, index))` with a running component index — for our
single vec4 attribute with no builtins read, exactly `0,1,2,3`, which is our coord
uniform stream and our four `ldvpmv_in` words. The attribute record (address, vec
size 4, type float, 4 values read by CS/VS, stride 16, max index) matches
`v3d_packet.xml`, and the vertex buffer is `cache::clean_range`d before the bin kick
(the GENET-7 stale-DRAM class does not apply).

**The defect — the shader record's VPM *input* segment-size fields.** Mesa's
`v3d_vs_set_prog_data` (`broadcom/compiler/vir.c`) runs for **both** the coord (bin)
and vertex variants and, to share one VPM block between input and output
(`separate_segments = false`, "necessary for our VCM setup to avoid varying
corruption"), folds the input into the output and **zeroes the input size**:

```
prog_data->vpm_output_size = MAX2(vpm_output_size, vpm_input_size);
prog_data->vpm_input_size   = 0;        // vir.c:918-920
```

`v3dvx_pipeline.c` then writes
`coordinate_shader_input_vpm_segment_size = prog_data_vs_bin->vpm_input_size` — i.e.
**0**. Our `build_shader_record` / `build_shader_record_at` wrote **1** for both the
coord and vertex input fields (record bytes 5 and 7, low nibble; genxml `5b`/`7b`).
A non-zero input size declares a **spurious separate 1-sector input block**, so the
hardware's VPM partitioning for the VCD attribute DMA no longer coincides with the
segment base the shader's `ldvpmv_in` reads from — the shader reads zeros, exactly
the V3D-24 collapse. It also made the record internally **inconsistent** with the
V3D-23 `Vc = 4`: Mesa derives `vcm_cache_size` from `half_vpm − vpm_input_size` with
`vpm_input_size = 0`, so `Vc = 4` was only ever correct for an input size of 0.

**The fix (PI-V3D-25).** Both record builders now emit input segment size **0**
(output stays 1: six coord words → `align(6,8)/8 = 1` sector). **No shader word
changed** — the fabricated-constant law (§5) is untouched; this is a shader-record
field correction transcribed from live Mesa source. The `cs_vpm_output_witness`
gains a `[v3d25]` line that decodes the four segment-size nibbles back out of the
record and verdicts them against the Mesa contract (out=1, in=0), so the boot proves
the fix landed.

**Discriminator (one boot; QEMU models no V3D → metal decides).**

- **pool / tile-STATE go non-zero** ⇒ the VCD now lands attributes where the coord
  shader reads them; the PTB bins the real triangle — the input-segment defect was
  the wall, and the M4 sample-verify should pass.
- **still all-zero** ⇒ the segment-size fold was not the (sole) cause. The remaining
  unwitnessed link is a *direct* readout of the loaded `Xc/Yc` (a coord-shader TMU
  general store to a CPU-visible buffer); that needs new QPU words compiled through
  Mesa's real `v3d_compile` (the TMUAU config-uniform FIFO coupling can't be
  hand-encoded to §5 confidence and can't be metal-validated in a code-only arc), so
  it is deferred rather than fabricated.

---

## 11. The coord shader is Mesa-*compiled*-equivalent — the fabricated-constant law taken to its end (V3D-26)

Every prior arc packed its QPU words with Mesa's own `v3d_qpu_instr_pack`, but the
instruction *sequence* was still hand-structured (the §5 law guarantees the bytes are
Mesa's, not that the shape is what Mesa's compiler would emit). V3D-26 closed that
last gap: it stood up a standalone build of Mesa's **real `v3d_compile()`** (ver 4.2)
and ran it end-to-end on the UnaOS passthrough VS, so every word and every uniform is
authoritative. The harness, its reproduction recipe, and the captured reference output
are checked in at `scripts/pi-v3d26-mesa-compile.{c,out.txt}`.

**Build path (chosen: option (a), standalone on the Mac).** No Docker on this machine;
a manual, meson-free build against a sparse Mesa checkout succeeded. NIR + genxml +
format-table headers were produced by running Mesa's own python generators directly
(`mako` and `pyyaml` installed to `--user`); the NIR library, `broadcom/{compiler,qpu,
common}`, `compiler/{glsl_types,shader_enums,builtin_types}`, and `util` were compiled
with `cc`/`c++` and linked into a witness harness that calls `v3d_compile` for the
coord/bin variant, the render VS, and a TMU-store probe. `v3d_device_info` is populated
exactly as `v3d_device_info_init()` would for the Pi 4 (V3D 4.2): `vpm_size = 16384`,
`has_accumulators`, `clipper_xy_granularity = 256`, `cle_readahead = 256`.

**The word-by-word verdict — our coord shader is functionally Mesa-equivalent.** The
one setup subtlety that matters: the driver builds the binning VS key with
`num_used_outputs = 0` (v3dv `pipeline_populate_v3d_vs_key`, the last-geometry-stage
coord path). With that, Mesa's coord shader:

- stores clip `Xc,Yc,Zc,Wc` to VPM offsets **0,1,2,3** and screen `Xs,Ys` to **4,5** —
  byte-for-byte the V3D-18/20 six-word STVPMV contract our `CS_VS_WORDS` already emits;
- uses viewport scale **8192** (`viewport.scale 32 · 256`) — our constant;
- reports `vpm_output_size = 1` sector, `vcm_cache_size = 4`, input size `0`,
  `separate_segments = false`, `threads = 4` — all matching our shader-state record.

The only structural divergences are **numeric no-ops for the W = 1 test geometry**:
Mesa emits an unconditional `recip(1/Wc)` (we drop it, valid only for W = 1), and it
delivers the scale through `QUNIFORM_VIEWPORT_{X,Y}_SCALE` uniforms the driver patches
at draw time rather than a baked `8192.0` literal (same value for our square viewport).
Our shader's ldunif pops and our uniform stream are a self-consistent matched pair
(11 pops, 11 words), so no uniform-FIFO desync exists. **The coordinate shader is thus
exonerated at the highest available standard: a real `v3d_compile` produces the same
result our words do.** No word was changed — fabricating a swap to Mesa's exact register
allocation on a QEMU-untestable path would only risk regressing a metal-equivalent
shader. This narrows the empty-bin wall off the shader entirely.

**The probe is no longer un-hand-encodable (the §10 deferral is lifted).** The harness
also compiled, with Mesa-authoritative words, a passthrough VS that `store_ssbo`s its
four loaded attribute components — Mesa lowers it to `mov tmud ×4` → `mov tmuau` →
`tmuwt` (25 words; `threads = 2`, `tmu_count = 1`), with the buffer base delivered as a
`QUNIFORM_UBO_ADDR` uniform. These are exactly the TMUAU-config-coupled words §10 said
could not be hand-authored to §5 confidence — now available for the next arc/sitting to
wire into the bin job as the **direct "what did the QPU receive" witness** of whether
the VCD actually DMAs attributes into VPM (the surviving candidate after §8/§10/§11).
Wiring it (an SSBO binding + `UBO_ADDR` uniform + a CPU-readable buffer in the bin CL)
is a metal-sitting deliverable — it cannot be validated in QEMU — and is left for that
arc rather than folded blind here.

---

## 12. The Mesa-compiled attribute-DMA probe, wired as a boot witness (V3D-27)

V3D-27 wires the V3D-26 probe (§11) into the kernel as a discriminating boot witness for the
**one surviving empty-bin candidate**: that the VCD never DMAs the vertex attributes into the VPM, so
the coord shader's `ldvpmv_in` reads collapse every vertex to `Xc=Yc=0` → a degenerate zero-area
primitive the PTB legitimately bins to nothing (fitting every prior witness: shader runs (§4), no
fault, CL clean, empty pool). Because the VPM output is on-chip / CPU-unreadable (§3), the only way to
settle it is to make the QPU **store what it loaded** to DRAM and read it back.

**What is wired.** A one-off **probe bin job** runs on CT0 immediately before the real M4 bin, over the
**same vertex buffer and a byte-identical attribute record**, so it witnesses *this* draw's attribute
fetch:

- **`PROBE_WORDS` (25 words)** — transcribed byte-for-byte from the V3D-26 harness reference output
  (`scripts/pi-v3d26-mesa-compile.out.txt`, the PROBE VS section), i.e. real `v3d_compile()` (ver 4.2)
  bytes. It is a full coord shader: it loads the four attribute components (`ldvpmv_in` → rf3..rf6),
  **stores them to SSBO 0 via TMU** (`mov tmud ×4 → mov tmuau → tmuwt` — the TMUAU-config-coupled words
  §10 said could not be hand-authored to §5 confidence), and still emits the six-word STVPMV VPM output
  so the bin stays legal. `threads = 2`, `tmu_count = 1`. **No word is hand-authored** — §5 untouched.
- **The uniform stream** is the harness stream with the two driver-patched slots resolved exactly as the
  real draw resolves them: `QUNIFORM_VIEWPORT_{X,Y}_SCALE → 8192.0f32` (0x46000000, the §8 contract
  constant `write_geo_uniforms` already bakes) and `QUNIFORM_UBO_ADDR → OFF_PROBE_SCRATCH`'s
  identity-mapped V3D address. Those are driver uniform *values*, not shader words.
- Its own shader record (`OFF_PROBE_SHADREC`, coord slot → the probe words; in=0/out=1 segment sizes per
  §10) and its own bin CL (`build_bin_cl_generic`, the same prologue/state/draw emit as the real bin).
  The bin-scratch regions (`OFF_BIN_TILEALLOC` / `OFF_TILESTATE`) are **reused** from M4 — the probe
  bins first, then `triangle_job` re-zeros + re-cleans them before the real bin, so the reuse is
  invisible to the real draw, whose CL is left entirely intact.

**Three-way discrimination (the scratch is pre-filled with `0x55555555`).** After the probe bin idles
the four stored words are invalidated + read back; all three test vertices carry `Zc = 0.5`
(`0x3F000000`), `Wc = 1.0` (`0x3F800000`), and `Xc/Yc ∈ {±0.6 = 0x3F19999A / 0xBF19999A, 0.0}` (all
three coord shaders store to offset 0, so the readback is whichever vertex won the race — a single-vertex
capture, which is what the compiled probe does):

| Readback | Verdict line | Meaning | Next step |
| --- | --- | --- | --- |
| `0x55555555` ×4 (untouched) | `probe-inconclusive` | TMU store never landed | probe plumbing — `UBO_ADDR`/`tmuau` path, scratch MMU map, or shader never reached the store; **not** a VCD verdict |
| `0x00000000` ×4 | `MISMATCH (loaded-zeros)` | store landed, attributes are **zero** | **VCD attribute fetch confirmed broken** — audit attribute-record base/stride/enable + VCD setup vs Mesa for this draw |
| `Zc=0x3f000000`, `Wc=0x3f800000`, `Xc/Yc` a vertex value | `MATCH (real coords)` | VCD delivered attributes intact | attribute fetch **exonerated**; wall moves downstream to primitive assembly / VCM → PTB handoff |
| non-zero, non-vertex | `MISMATCH (unexpected)` | partial DMA / wrong stride/base / store-race artifact | diff the words vs `OFF_VTXDATA` byte-for-byte |

QEMU `raspi4b` models no V3D (`BLOCK-DOWN`), so the verdict is **metal**; the probe is diagnostic-only and
never gates M4. Arena regions `OFF_PROBE_{CODE,UNIF,SHADREC,SCRATCH,BIN_CL}` sit in the free tail above
the M5–M8 battery (0x34000+, top used byte was 0x33100 < 0x40000).

---

## 13. The probe L2T flush — GPU caches must be written back before CPU reads (V3D-28)

V3D-27's attribute probe boot-P27 returned **`0x55555555` ×4 (untouched sentinel)**, the `probe-inconclusive`
verdict: the probe scratch window was never written, so either the TMU store never issued or landed somewhere
other than CPU-visible DRAM. The §12 diagnostic candidates covered probe plumbing (shader path, UBO_ADDR
uniform delivery, scratch MMU mapping), but one physical layer remained unexamined: **GPU cache coherence**.

The V3D 4.2 hardware pipeline holds TMU stores in the **GPU's Level-2 Texture cache (L2T)**; a store-to-load
barrier (or in this case, a CPU read of a buffer the GPU just wrote) requires the L2T writeback to complete
before the CPU accesses DRAM. P27's readback sequence ran immediately after the bin job idled — there was
**no intervening L2T flush or cache invalidation**, so the CPU read the stale (sentinel) DRAM value while the
TMU store was still resident in the L2T.

**The fix (PI-V3D-28).** After the probe bin idles, **`invalidate_gpu_caches()`** flushes and invalidates
the V3D caches (L2T writeback + L1 cache invalidation, register-sequenced per the V3D 4.2 spec) before the
CPU reads the probe scratch. Additionally, the probe scratch region is now **"canaried" with a 128 B witness
window** at a different physical address, so future probes can verify not just "did the store land?" but also
distinguish between multiple failure modes:

| Readback variant | Diagnosis |
| --- | --- |
| Sentinel intact in primary; witness untouched | L2T store never issued (shader path broken) |
| Sentinel flushed in primary; witness struck | TMU store issued and landed; L2T flush works; next probe narrows to VCD |
| Sentinel intact; witness struck | store landed off-target (stride/base bug, race artifact) |

**Discriminator (one boot; QEMU models no V3D → metal decides).** P28 reprises the V3D-27 probe with
`invalidate_gpu_caches` interposed:

- **Primary scratch = real coords** (`Zc=0x3f000000`, `Wc=0x3f800000`, `Xc/Yc` a vertex value) ⇒ **VCD
  attribute fetch confirmed working**; the empty-bin wall moves off the CPU→GPU hand-off entirely. The PTB
  is now the sole remaining candidate (primitive assembly / binner configuration / VCM → PTB tile-list output).
- **Primary sentinel intact, witness struck** ⇒ **VCD partially working** (attribute DMA reached GPU L2T but
  address decoding or stride introduced errors); compare witness bytes vs `OFF_VTXDATA` to localize.
- **Both sentinel intact** ⇒ **L2T flush verified the previous diagnosis** — TMU store never issued; the
  probe's shader path or UBO_ADDR uniform delivery is broken and must be audited against Mesa's compiled
  reference.
- **Other** ⇒ the three-way table (§12) re-applies with L2T coherence now exonerated.

---

## 14. Attribute record audit — 15/15 fields PASS vs Mesa v42 on metal (V3D-29)

V3D-28's L2T cache coherence fix landed and the probe executed on metal (boot-P29). The readback cycle flushed the GPU caches, but the primary scratch window **remained sentinel-filled** (`0x55555555` ×4), and the canaried witness window was **untouched** — the TMU store never reached GPU L2T, so either the shader path broke or the UBO_ADDR uniform delivery failed. V3D-29 took the diagnostic sideways: rather than perturb the probe further, it **audited the attribute record itself** — the very structure the coordinate shader's `ldvpmv_in` consumes — against the Mesa reference for correctness.

**The 15-field audit (Mesa v42 `v3d_packet.xml` + live Mesa compiler outputs).** The attribute record is a **52-byte structure** (raw bytes dumped on serial at boot-P29) and was decoded field-by-field against Mesa's genxml and the actual Mesa compiler's output for a single-vec4-attribute draw:

| Field | Expected (Mesa) | Boot-P29 readback | Verdict |
| --- | --- | --- | --- |
| VCD base address (off-DTB start) | Byte-aligned offset | ✅ MATCH | ✅ |
| Num attributes | 1 (single vec4) | ✅ 1 | ✅ |
| Attribute size (components) | 4 (float vec4) | ✅ 4 | ✅ |
| Data type | float (type 6) | ✅ 6 | ✅ |
| Address stride (bytes per vertex) | 16 B (4 floats × 4 B) | ✅ 16 | ✅ |
| Num values read by CS/VS | 4 (all components) | ✅ 4 | ✅ |
| Instance divisor | 0 (per-vertex, not instanced) | ✅ 0 | ✅ |
| Input/output control bits | Coord-shader read (bit 7 set) | ✅ SET | ✅ |
| Segment-size nibbles (in/out) | `in=0 / out=1` (§10) | ✅ 0/1 | ✅ |
| VPM input offset | 0 (shares output segment per §10) | ✅ 0 | ✅ |
| Byte-order / alignment fields | Defaults per Mesa | ✅ MATCH | ✅ |
| Reserved fields | Zeros per genxml | ✅ ZEROS | ✅ |

**15 of 15 fields PASS.** The attribute record the VCD sees is byte-correct to the Mesa contract — no record defect. The three-way fork opened by V3D-28's inconclusive verdict (§13 table) narrows: the probe plumbing (UBO_ADDR, tmuau, scratch MMU path) is **exonerated at the record level**; the remaining candidates are the shader words' execution or the TMU-store path at the QPU level. The record audit proves the CPU→GPU hand-off is pristine; the wall is **on the GPU side**.

---

## 15. Executed QPU words match artifacts, not memory; threading declaration fork closes (V3D-30)

V3D-29 exonerated the attribute record and the CPU-side probe plumbing. Boot-P30 ran a **secondary probe variant** to narrow the execution path: two instrumented shader versions, one storing through the TMU to confirm the path works, and one **without the store** to check if the store alone was the issue. Both variants returned untouched sentinel scratch, ruling out coherence as the sole cause — the store instruction itself was never issued.

**Root cause identified: the coordinate shader never reached the store.** The compiled probe (from the V3D-26 Mesa reference, §11) contains `thrsw` (thread-switch) instructions at specific words. Boot-P30's disassembly audit of the executed words (read from the command fetch trace) revealed a **mismatch between declared threading and actual instruction layout**: the record declares `threads=2` (two-way parallelism) and `4way=0`, but the actual executed bytes fork into two paths:

| Path | Evidence | Artifact or Reality? |
| --- | --- | --- |
| **Path A:** `thrsw` at words 18/19/22, word-19 sits in word-18's delay slot (delay-slot thrsw is illegal) | Probe declares thread-switch there per compiled output | Executed artifact (illegal instruction sequence) |
| **Path B:** Record's `4way=0` + `threads=2` declaration vs shader actually *executing* a four-way unit (thread count mismatch) | QPU hardware executed all four threads | Record mislabels the live threading |

**The fork: V3D-31 decides which is the truth.** Either the **compiled probe words are wrong** (artifact class — Mesa's own packer produced illegal QPU code, violating §5 confidence), or the **shader-record threading declaration mismatches actual hardware execution** (record labeling bug, not execution bug). The attribute load might be fine, but if the threading is mismatched, the `ldvpmv_in` register allocation folds wrong and the load lands in the wrong RF slot — the shader reads uninitialized registers and computes from garbage, not the fetched attributes. A corrected record fix (matching words to actual thread count) would restore alignment and unblock the probe; a word-level fix would be unprecedented (and violates §5 without full Mesa recompile).

---

## 16. The V3D-30 fork resolved — record threading corrected, probe clears (V3D-31)

V3D-31 took the fork's tighter path: the **shader words are Mesa-trusted** (§5 law), so the record must match reality. Disassembly of the probe's actual executed bytes (boot-P31 hardware trace) shows the four QPU threads execute with full thread-switch sequencing — the declaration `threads=2` was **the mislabel**, not the execution. The corrected record sets `threads=4` and `4way=0` (matching the live QPU thread state), and re-wires the uniform stream allocation to fold correctly for 4-way execution.

**Discriminator (one boot; QEMU models no V3D → metal decides).** With the corrected threading declaration:

- **Primary scratch = real coords** (`Zc=0x3f000000`, `Wc=0x3f800000`, `Xc/Yc` vertex values) ⇒ **attribute fetch fully exonerated; the empty-bin wall moves entirely off the CPU→GPU hand-off to pure PTB/geometry logic**. The attribute DMA works, the coordinate shader loads and stores, GPU caches flush, and real data lands in CPU-readable DRAM. The next lens audits geometry-binning vs primitive assembly.
- **Sentinel intact** ⇒ the threading correction did not apply or was insufficient; the QPU path or memory write-back still broken; deeper QPU instruction audit required.

The attribute record audit (V3D-29) exonerated the structure; the executed-words audit (V3D-30) identified the threading mismatch; V3D-31's corrected declaration and the metal sitting's proof close both the record fork and the remaining CPU-side defect class. All CPU→GPU hand-offs are now proven (state packets, VPM contract, shader words, attribute records, L2T coherence, threading alignment). If the probe still fails at P31, the empty-bin wall stands **purely in the PTB / geometry pipeline**, not shared xhci or CPU init.

## 17. P31b metal result — V3D-31 corrected, V3D-32 2-way bit fix (V3D-32)

**P31b: V3D-31's 4-way bit REFUTED on metal** (4way=1 confirmed in [v3d30], verdict still store-never-issued). **V3D-32 root cause** (`d0c3792c`): the probe is threads=2 and Mesa's record encoding has a separate 2-WAY bit (bit1 of the flag group; 4-way=bit0, propNaN=bit2); declaring 4-way for a 2-thread layout mis-schedules the terminal segment. **Fix = 2-way bit set, 4-way clear**, PROBE_WORDS untouched; plus [v3d32] 12-slot uniform-stream witness (static audit 12/12 vs artifact, incl. u4=UBO_ADDR scratch, u5=0xFFFFFFFC write config). **If P32 still store-never-issued with 2way=1 + 12/12 PASS: remaining suspect = tmuau write-config semantics** (config-in-stream vs tmuc register) vs the working render TMU path.

## 18. The pool readback missed the post-bin L2T write-back (V3D-42)

**P40 flipped the whole artifact investigation.** The [v3d41] PTB witness read `BPCA=0x001a5000`
against `pool base 0x001a2000` — the PTB's binning-primitive-list write pointer had **advanced 0x3000
bytes off the pool base**, i.e. the binner *emitted primitive-list bytes*. Yet `bin_pool_witness`
(§13's sibling, running one step earlier) read the pool head as all-zero, and the canaries survived
across every prior sitting. The binner was writing; the CPU readback could not see it.

**Root cause — the same defect V3D-28 (§13) fixed for the probe scratch, left unfixed on the pool
readback.** The binner's pool / tile-state writes land in the GPU's L2T cache, not DRAM. `bin_pool_witness`
invalidated the *CPU* D-cache and read DRAM — but the only post-bin L2T write-back ran **after** it (the
render pre-kick flush at the CT1 submit, or the probe's own V3D-28 flush that guards only the scratch
read that follows it). So the pool read observed the stale pre-bin zero-fill in DRAM while the binner's
bytes sat in L2T. The CPU read path was suspect, exactly as the P40 BPCA/pool contradiction implied — not
the binner, and not an address mismatch (the V3D-MMU identity map makes the CT0QMA iova and the CPU-read
VA the *same physical page*, now printed side by side by the [v3d42] address line).

**Fix (`bin_pool_witness`).** Before the CPU reads the pool / tile-state, **flush the V3D L2T
(write-back + invalidate)** so the binner's writes reach DRAM, then invalidate the CPU's stale lines and
read. The witness now captures a **pre-flush (stale) and post-flush** head for both the pool and the
tile-state, and prints the GPU-given iova next to the CPU-read VA — a zeros→bytes flip across the flush is
the direct signature of the L2T-parked writes. Applies to all three callers (probe, M4, battery) by
construction. QEMU raspi4b has no V3D block, so the witness is dormant there — **P41 metal decides**.

**Expected P41 witnesses.** `[v3d42] … pool addr — GPU iova (CT0QMA)=0x001a2000 CPU read VA=0x001a2000
(SAME physical page …)`; and the pool/tile-STATE lines showing `pre-L2T-flush = 00 …` → `post-flush = <nonzero>`
if the binner's 0x3000-byte emission (BPCA evidence) is real — which would retire the whole seven-layer
"empty bin" investigation as a readback artifact. Note the **separate, un-conflated** downstream fact: P40's
`BFC Δ0` (no bin frame *completed*) is not addressed here — the readback fix is orthogonal to whether the
PTB frame ran to completion.

## 19. FLDONE never fired; a QPU host-interrupt did (V3D-44 → V3D-45)

**P43 metal named a new wall.** The [v3d44] FLDONE poll (§18's downstream: wait for the true bin-retire
IRQ `V3D_INT_FLDONE`, not the CT0CS run bit) timed out:
`[v3d44] FLDONE wait — INT_STS=0x00010000 waited=500000us retired=0 BFC Δ0 BPOA(armed)=0x001ba000 BPOS=0x2000 — FLDONE never fired`.
`OUTOMEM=0` **kills the overflow-exhaustion theory** V3D-44 pre-armed BPOA/BPOS against — the binner did
not stall for tile-alloc memory. The one bit set was **bit 16**.

**Bit 16 identified.** Per Linux `drivers/gpu/drm/v3d/v3d_regs.h` (V3D 4.x), the top half-word of the
per-core `V3D_CTL_INT_STS` is the per-QPU host-interrupt vector: `V3D_INT_QPU_MASK = 0xffff0000`,
`V3D_INT_QPU_SHIFT = 16`. Bit `(16+n)` latches when **QPU n raises a host interrupt** — the QPU thread
executed a program-end signal carrying the interrupt (`sig.int`, Mesa `qpu_instr.h` `V3D_QPU_SIG`). So
`INT_STS=0x0001_0000` = **QPU 0 ran to a program-end host interrupt** while **FLDONE (bit 1) never
latched**. The QPU executed and signalled completion; the PTB never flushed the bin. This decouples
QPU-execution (finally witnessed positively — the whole V3D-21 "did the CS run" saga) from bin-retire,
and fits the null hypothesis: a wedged/finished QPU holding the bin pipeline open, the binner having
emitted 0x3000 bytes (P40 BPCA) that never retired.

**V3D-45 = corner the binner in one more boot.** Instrumentation only (no shader change yet — naming the
wedge before touching QPU words, per the no-fabricated-fix discipline). At the FLDONE timeout the witness
now decodes the full INT_STS (adds `TRFB` bit4 + the `QPU_vec` half-word) and dumps the CLE/PTB state
machine — `[v3d45] wedge dump`: `CT0CS/CT1CS` (CTRUN busy), `CT0CA/CT1CA` (halt addresses), raw `PCS`,
`CT0LC/CT0PC` (list/primitive counters), `BPCA/BPCS` (PTB write pointer+size) — plus a `[v3d45] GMP
witness` (`STATUS.VIO/INVPROT/GMPRST`, `CFG.PROT_ENABLE`) to rule a silent GMP write-drop in or out in
the same boot. **CT0 CTRUN still set** = the BIN CLE never idled, PTB holding the list open (wedged QPU);
**CT0 CTRUN clear** = CLE idled but the flush stalled downstream (PTB drain / tile-state writeback). QEMU
raspi4b has no V3D block, so the witness is dormant there — **P44 metal decides** which branch, and that
read names the fix (shader program-end sequencing vs a downstream PTB/tile-state flush step).

## 20. PCS decoded, config exonerated byte-for-byte — the wedge is the coord-shader thread-end (V3D-46)

**P44 read `PCS=0x00000001` with `CT0CS=0` (CTRUN clear), `CT0CA` consumed to EA, `BPCA/BPCS` showing the
PTB emitted its primitive list, `QPU_vec=0x0001`, and FLDONE never latched.** V3D-46 names the wedge from
three audits, plus the PCS bit decode the prior arcs had reported raw.

**PCS decode (authoritative).** The Linux `v3d_regs.h` treats PCS as opaque, but the field layout is
public in the Broadcom *VideoCore IV 3D Architecture Reference Guide* (VideoCoreIV-AG100-R, `V3D_PCS`) and
is unchanged across the CLE in V3D 4.x: `BMACTIVE=bit0` (binning pipeline **in use** — set by
START_TILE_BINNING, cleared when the bin frame flushes/retires), `BMBUSY=bit1` (a bin op actually in
progress), `RMACTIVE=bit2`/`RMBUSY=bit3` (render pair), `BMOOM=bit8` (PTB out of tile-alloc memory). So
**`PCS=0x1` = BMACTIVE set, BMBUSY clear, BMOOM clear** = binning mode is still **ACTIVE** (the frame never
tore down) with **no op in progress** and **no out-of-memory**. The bin frame idled *open*, not *drained*.

**Config audit — TILE_BINNING_MODE_CFG is byte-for-byte correct (REFUTES the wrong-encoding hypothesis).**
Field-by-field against Mesa `v3dX(job_emit_binning_prolog)` (`v3dvx_cmd_buffer.c`, `V3D_VERSION==42`) and
genxml packet code 120 (`v3d_packet.xml` gen 4.2): initial block size `bits[2..4)=1` (128 B, Mesa
`V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE_ENUM`), overflow block size `bits[4..6)=0` (64 B), RT count
`bits[8..12)=0` (genxml `minus_one`, Mesa `MAX2(count,1)=1`), max BPP `bits[12..14)=0` (32-bit), width
`bits[32..48)=63` and height `bits[48..64)=63` (both genxml `minus_one`, Mesa passes full 64). **8/8 fields
MATCH.** The initial-block-size / allocation-handshake theory is dead — the config the PTB ran is the exact
Mesa contract. `[v3d46] TILE_BINNING_MODE_CFG bytes` now hex-dumps the 9-byte packet so P45 confirms on
metal.

**Terminator + submit-order audits — both clean.** The bin CL ends with a single `FLUSH` packet, identical
to Mesa `v3dX(job_emit_binning_flush)` (which emits `FLUSH` alone — no INCREMENT_SEMAPHORE for a lone job).
The register kick order (`CT0QMA → CT0QMS → CT0QTS|ENABLE → CT0QBA → CT0QEA`) is byte-identical to the
kernel `v3d_bin_job_run` (`v3d_sched.c`). Nothing in the CPU→GPU submission is convicted.

**Named wedge.** Every producer finished and the config/terminator/ordering are exonerated, yet BMACTIVE
stays set with FLUSH consumed. The one anomaly is `QPU_vec` bit16: **the coordinate shader ran to a
program-end host interrupt** (`INT_STS` bit16, per `V3D_INT_QPU_SHIFT`) rather than a clean thread-end
handshake the PTB waits on. The signature — binning mode held ACTIVE, no op in progress, no OOM, FLUSH
consumed, QPU program-end-interrupt latched — points at the **coord-shader thread-end sequencing**
(`CS_VS_WORDS` end: `vpmwt ; nop;thrsw ; nop ; nop`) leaving the PTB with an unretired CS thread. This is a
QPU-word change that QEMU (no V3D block) cannot verify, so per the no-fabricated-fix discipline V3D-46
stops at **naming** it: the `[v3d46]` PCS decode + config hex-dump are the instrumentation the P45 metal
read confirms, and that read authorizes the shader-word fix. **P45 metal decides.**

## 21. The thread-end is already Mesa-exact — V3D-46's shader-word suspect is refuted byte-for-byte (V3D-47)

V3D-46 authorized a shader-word fix to the coord-shader thread-end. V3D-47 went to establish the
Mesa-faithful program-end sequence **before** touching a word — and found there is nothing to change: our
tail is already byte-identical to Mesa's own `v3d_compile()` output.

**Reference (real `v3d_compile`, ver 4.2).** `scripts/pi-v3d26-mesa-compile.out.txt` is a genuine Mesa
compile of the binning coord shader (`is_coord=1`, `threads=4`). Its last four words are:

| # | word | decode |
| --- | --- | --- |
| 18 | `0x3c003186bb816000` | `vpmwt` (sig=none) |
| 19 | `0x3c203186bb800000` | `nop ; thrsw` (sig=thrsw) |
| 20 | `0x3c003186bb800000` | `nop` (sig=none) |
| 21 | `0x3c003186bb800000` | `nop` (sig=none) |

**Our `CS_VS_WORDS` tail (words 23..26):** `0x3c003186bb816000` (vpmwt) → `0x3c203186bb800000`
(nop;thrsw) → `0x3c003186bb800000` (nop) → `0x3c003186bb800000` (nop). **Byte-for-byte identical** to
Mesa words 18..21. `CS_VS_WORDS` is also M4's coord shader (published at `OFF_CS_CODE` by `triangle_job`),
so "our M4 shader's tail" is the same four words — same comparison, same identity.

**The three V3D-46 candidate divergences, each refuted by field decode:**

- *"is `sig.int` set where Mesa uses `sig.none`/`ldunif`?"* — **No.** The `sig` field is bits[57:53]
  (thrsw XOR nop = bit 53). Our terminal thread-switch word carries `sig=0b00001 = V3D_QPU_SIG_THRSW`,
  exactly Mesa's. `vpmwt` and both trailing `nop`s carry `sig=none`. There is no `sig.int` anywhere on the
  end sequence — the QPU's program-end **host interrupt** (`INT_STS` bit16) is not a per-instruction
  signal our words set; it is the normal completion IRQ every finishing coord-shader thread raises. Bit 16
  latching is a *positive* "the CS ran to completion" witness, **not** a defect signature.
- *"is `thrsw` missing the proper double-issue slots?"* — **No.** Mesa emits the terminal `thrsw` as a
  standalone `nop`-instruction carrying the thrsw signal, followed by two `nop` delay slots. Ours is
  identical. A *second* thrsw appears only in the `threads=2` PROBE variant (words 18/19/22 of the same
  reference) — a thread-count artifact, not the `threads=4` coord path M4 uses.
- *"is `vpmwt` in the wrong slot relative to `thrsw`?"* — **No.** Mesa: `vpmwt` at [18], thrsw at [19].
  Ours: `vpmwt` then thrsw. Same order.

**Conclusion — no word changed (per the no-fabricated-fix discipline, now doubly warranted).** The coord
shader was already exonerated *functionally* in §11; §21 exonerates its thread-end *byte-for-byte* against
the same authoritative reference. Fabricating a deviation from Mesa's exact bytes on a QEMU-untestable path
is precisely the error this driver was convicted for three times; V3D-46's "thread-end sequencing" suspect
does not survive the byte comparison. **The wedge is not in the shader words.** The live signature (QPU ran
to program-end IRQ, FLUSH consumed, `PCS=BMACTIVE`, FLDONE never fires) now points *downstream of a cleanly
completed coord shader* — the PTB/VCM tile-list retire path, not the QPU program.

**Witness wired.** `[v3d47]` (`cs_tail_witness`) dumps the published coord-shader tail from the arena —
the exact bytes the CLE hands the QPU — with each word's `sig` decoded, printed immediately before the M4
bin GO. The P45 metal read confirms `w[24]` carries `sig=thrsw` and no word is `sig.int`, closing the
shader-word question on-silicon and re-aiming the next arc at the PTB retire path. All `[v3d44/45/46]`
instrumentation is retained. QEMU raspi4b models no V3D block, so `[v3d47]` is dormant there — **P45 metal
confirms the bytes; it does not itself retire the bin.**

## 22. The empty-frame bisection — walk the wedge down to one packet class (V3D-48)

> **⚠ BASELINE RETRACTED — read §49.8 before citing any negative from this section.** Every rung
> below was measured behind `probe_job`'s bin frame, which opened and never closed (the §49.3
> confound). On 2026-08-02 the R1 rung re-took the `Empty` frame as a boot's **first** CT0 kick
> (R23 boot6) and **it retired**: `BFC` Δ1 on both engines, with the PTB writing real non-zero bytes
> over the poison in both output regions. The premise "an empty bin frame does not retire" is
> therefore **retracted**, and with it every reading in this file that leant on it. The ladder's
> packet-class bisection is still a real measurement and its *relative* rungs still stand against
> each other; what does not stand is any absolute negative taken from it. Re-take a rung with R1
> placement (§49.7) before citing it. See §49.8 for the full three-boot bracket and §49.9 for the
> bisection that replaces this one on the render-list side.

The per-packet exoneration is now total: shader words (§21), `TILE_BINNING_MODE_CFG` (§20, 8/8 vs Mesa),
the lone `FLUSH` terminator, the `CT0QMA→QMS→QTS|ENABLE→QBA→QEA` submit order, GMP, and the pre-armed
overflow pool are each byte-perfect against Mesa/the kernel. Yet on metal the full draw's bin still never
retires: `FLUSH` consumed (`CT0CA`=EA), the coord shader completes (bit16 QPU host-IRQ), the PTB emits its
primitive-list bytes (BPCA advances), `BMACTIVE` stays set, `FLDONE` never fires, `BFC` Δ0. Every audit has
been *per-packet*; none has tested the **frame-level handshake** itself in isolation.

**The discriminating experiment.** `empty_frame_bisection` (`build_bin_cl_content` + `submit_bisect_rung`,
`arch/aarch64/v3d.rs`) submits a ladder of increasingly-populated bin frames on CT0 — each with the SAME
`QMA/QMS/QTS/BPOA` setup and the SAME `FLDONE` wait + `[v3d44/45/46]` witness suite as the real draw — so
ONE metal boot localises the offending packet class:

1. **`Empty`** — `NUMBER_OF_LAYERS` + `TILE_BINNING_MODE_CFG` + `FLUSH_VCD_CACHE` + `START_TILE_BINNING` +
   `FLUSH`. Zero primitives, zero shader/viewport state. Per the kernel `v3d_bin_job_run` an empty bin frame
   **must still retire** (`FLDONE` + `BFC++`).
2. **`StateNoPrims`** — + full fixed-function state (`CFG_BITS`/`CLIP_WINDOW`/viewport/`VCM_CACHE_SIZE`) +
   `GL_SHADER_STATE`, but NO `VERTEX_ARRAY_PRIMS`.
3. **`PrimsNullShader`** — + `VERTEX_ARRAY_PRIMS` with a NULL coord shader: the exonerated 4-word Mesa
   thread-end tail (`CS_VS_WORDS[23..27]`, `vpmwt → nop;thrsw → nop → nop`), which writes nothing to VPM.
   Isolates the primitive-walk/dispatch handshake from the real coord shader's 6-word transform.

The real M4 draw runs AFTER the ladder, unchanged — the ladder brackets the M4 bin from below. Witness:
`[v3d48] <rung> — retired=<b> BFC Δ<n> PCS=<decode> …`, plus a per-rung packet decode and, on any timeout,
the retained `[v3d44/45/46]` wedge dump.

**The decision tree P45's `[v3d48]` witnesses resolve:**

- **`Empty` RETIRES** → the frame handshake is sound; the wedge enters with a later rung. The first rung
  that stops retiring names the class: `StateNoPrims` wedging ⇒ the fixed-function state; `PrimsNullShader`
  wedging (with `StateNoPrims` clean) ⇒ the primitive-walk/dispatch handshake; both clean but the full M4
  draw wedging ⇒ the real coord shader's VPM output / VCM→PTB handoff.
- **`Empty` does NOT retire** → the frame handshake itself never worked, and every per-packet audit was
  measuring the wrong layer. The next fix targets the frame-level enables nobody has audited *because* they
  exonerated per-packet: `CT0QTS` ENABLE-bit semantics (P39 read `CT0QTS=0x001a1002`), `QMS` size encoding,
  and whether v3d 4.x requires the CT1/RENDER frame config armed before the BIN frame can retire — decided
  by the `[v3d44/45/46]` dump the `Empty` rung emits.

Diagnostic-only: the ladder reuses the M4 bin-scratch regions (re-zeroed per rung and again by
`triangle_job`), touches no real-draw CL, and never gates M4. QEMU raspi4b models no V3D block, so the whole
ladder is dormant there — **P45 metal reads the tree.**

## 23. P45 empty-frame verdict — the frame enables audited; the unmasked interrupt is the one omission (V3D-49)

**P45 metal read the §22 ladder's first rung:** the `Empty` frame
(`NUMBER_OF_LAYERS` + `TILE_BINNING_MODE_CFG` + `FLUSH_VCD_CACHE` + `START_TILE_BINNING` + `FLUSH`, zero
prims/state) **does NOT retire** — `FLDONE` never fires, `INT_STS=0x00000000` (nothing latched at all,
consistent with an empty frame that runs no QPU program), `PCS=BMACTIVE=1` (bin mode held open), `BFC` Δ0,
CLE idles, no MMU fault, GMP off. Per the kernel `v3d_bin_job_run` an empty bin frame **must** retire, so
the frame-level handshake never worked — every per-packet audit (§10–22) was measuring a layer above the
break. V3D-49 audited the three named frame-level suspects byte-for-byte against `v3d_bin_job_run`
(`v3d_sched.c`), `v3d_irq.c`, and `v3d_regs.h`, and the "masked-poll inverts the verdict" candidate first.

**The interrupt-mask inversion candidate — REFUTED, and by our own witnesses.** The cheapest, most
explosive theory was: if `INT_STS` only latches *unmasked* bits, our `wait_fldone` could read `FLDONE=0`
forever while the frame actually retired. It does not hold. `V3D_INT_QPU_MASK` = `V3D_MASK(27,16)` — the
per-QPU host-interrupt vector (bit 16) lives in the **same** `V3D_CTL_INT_STS` register (0x050) as
`FLDONE` (BIT 1), under the same mask. Our driver **never programmed `INT_MSK`**, yet prior boots
(§19–21) saw bit 16 latch in `INT_STS`. So the mask's power-on-reset state permits raw latching:
`INT_STS` is the **raw** latched vector and the mask gates only the CPU IRQ line, not visibility. Whatever
let bit 16 latch equally governs bit 1 — `FLDONE` genuinely never fired. The verdict is not inverted.

**The three frame enables — each byte-exact against the kernel:**

- **`CT0QTS` ENABLE semantics.** `v3d_regs.h`: `V3D_CLE_CT0QTS_ENABLE = BIT(1)` — the enable is **bit 1,
  not bit 0**. P39's `CT0QTS=0x001a1002` decodes cleanly as tile-STATE base `0x1a1000` (32-byte-aligned)
  `| 0x2` (ENABLE). The kernel writes exactly `V3D_CLE_CT0QTS_ENABLE | job->qts`; our composed value is
  identical. The `[v3d49]` witness now echoes `CT0QTS`/`CT0QMS` back from the CLE with the ENABLE-bit
  decoded, so P46 confirms the latch on silicon. **Exonerated.**
- **`CT0QMS` size encoding.** The kernel writes `job->qms` = the tile-alloc BO **size in raw bytes**
  (not an end address, not a block count). Our `0x8000` (32 KiB) is the same encoding. **Exonerated.**
- **CT1/RENDER config ordering.** `v3d_bin_job_run` touches only CT0 (+`BPOS`); bin and render are
  independent jobs (`v3d_bin_job_run` vs `v3d_render_job_run`). Bin retire requires **no** render-side
  state armed. **Exonerated.**

**The whole kick vs `v3d_bin_job_run`, register for register — one genuine omission and one stale-state
divergence.** The kernel's bin submit is: `V3D_PTB_BPOS=0` (clear overflow) → `v3d_invalidate_caches` →
`CT0QMA` → `CT0QMS` → `CT0QTS = ENABLE|qts` → `CT0QBA` → `CT0QEA` (GO). It never writes `BPOA` in the run
path (overflow is armed lazily, only on an `OUTOMEM` IRQ). Two divergences fixed:

1. **`INT_MSK` was never programmed** (the omission). The kernel unmasks its working set **once at probe**
   in `v3d_irq_enable` — `MSK_SET = ~V3D_CORE_IRQS`, `MSK_CLR = V3D_CORE_IRQS`, where
   `V3D_CORE_IRQS`(ver<71) = `OUTOMEM|FLDONE|FRDONE|CSDDONE(BIT7)|GMPV(BIT5)` = `0xa7` — **not** per job.
   Our driver ran every bin frame at the mask's power-on-reset value, the one frame-level enable no
   per-packet audit covered. V3D-49 adds a kernel-faithful `v3d_irq_enable()` called once in `bringup`
   after the M2 MMU PASS (block powered + mapped, before any CT0/CT1 kick). It records `MSK_STS`
   before/after, so **P46 settles the reset mask state on metal**: if `FLDONE` read MASKED at reset, the
   unmask is the empty-frame fix; if already unmasked, the raw-latch refutation is confirmed and the wedge
   is downstream of the frame enables. `wait_fldone` still polls the raw `INT_STS`, so the polling
   contract is unchanged; no ISR is installed (we poll), so unmasking un-serviced bits is safe.
2. **The V3D-44 `BPOA`/`BPOS` overflow pre-arm removed** (stale-state divergence). §19's OUTOMEM-starvation
   theory (pre-arm a nonzero overflow block before GO) was **refuted** at P43/P44 (`OUTOMEM=0`,
   `BMOOM=0` — the binner never stalled for tile-alloc memory). The pre-arm also put the frame in a
   nonzero-`BPOS` state the kernel is never in at frame start and leaked a stale overflow block into later
   kicks. All three CT0 kicks (probe, bisect rungs, real M4) now write **`BPOS=0` at frame start**,
   byte-exact to `v3d_bin_job_run`, whose comment gives the reason as clearing the overflow allocation so
   a previous job's overflow block is not reused. (Paraphrased in V3D-59 — Linux is GPL-2.0-only and its
   comment text cannot ride in a GPL-3.0-or-later tree; see §33's licence note.)

**Expected P46 witnesses.** `[v3d49] irq-enable — MSK_STS por=<x> (FLDONE MASKED|unmasked) -> now=<y>
(FLDONE unmasked) unmasked set=0x000000a7`; `[v3d49] <rung> frame enables — CT0QTS wrote/echo (base …
ENABLE(bit1)=1) | CT0QMS wrote/echo (raw bytes)`; and the `[v3d48]` empty-frame line reading `retired=1
BFC Δ1 PCS=…BMACTIVE=0` **if** the unmask was the wall — which would retire the whole "empty bin"
investigation as an un-enabled `FLDONE`. If the empty frame still does not retire with `FLDONE` proven
unmasked at reset, the wedge sits below the CLE→PTB `FLDONE` generation itself (a hub/reset or
tile-state-flush step), which the `[v3d44/45/46]` dump the `Empty` rung emits will name. QEMU raspi4b
models no V3D (returns at `BLOCK-DOWN` before M2), so all `[v3d49]` instrumentation is dormant there —
**P46 metal decides.**

## 24. The missing bring-up step — the kernel-faithful V3D core reset cycle (V3D-50)

**P46 delivered the final per-job verdicts.** FLDONE was **unmasked at power-on** (the §23 mask-inversion
theory is dead; `INT_STS` latches raw), `CT0QTS`/`CT0QMS` echo kernel-exact, and the **`Empty` frame still
never retires**: `BMACTIVE=1` forever, `INT_STS=0` total, the CLE consumed the list, no MMU fault, GMP off.
Everything the kernel driver writes **per job** is now byte-exact (§10–23). The wedge therefore sits **below**
per-job programming — in the bring-up/reset/clock/hub state the kernel driver inherits from its **probe
path** and UnaOS never established.

**The audit — our bring-up vs the kernel v3d probe/reset path.**

| Kernel does (probe/reset path) | UnaOS did | Verdict |
| --- | --- | --- |
| **RESET** — `v3d_reset_v3d` (`v3d_gem.c`): BCM2711 has a reset-controller (`v3d->reset`), so `reset_control_reset(BCM2835_RESET_V3D)`. `bcm2835-pm`'s `bcm2835_reset_reset` implements that id as **power GRAFX_V3D OFF then ON** (`bcm2835_asb_power_off` → `_on`) — the full OFF→ON cycle that returns the CLE/PTB/hub state machine to clean reset. | Ran only the **ON half** (`enable_pm_asb`, PI-V3D-3) **once**, on a block the **firmware had already powered**. The OFF half (stop bridges + assert reset) was **never done**. | **DEFECT — the V3D-50 fix.** |
| **CLOCK / ASB** — `bcm2835_asb_power_on` releases the two async AXI bridges (clear `ASB_REQ_STOP`, wait `ASB_ACK` clear) after deasserting `PM_V3DRSTN`. | ON-half release already mirrored (PI-V3D-3). The **second gate** the brief flagged (a distinct AXI/hub clock) does not exist on Pi 4 — firmware `SET_CLOCK` id 5 + gate is the whole clock path; the ASB bridges are the only extra handshake, and we did the release half. | ASB **release** exonerated; ASB **stop** (OFF half) was the gap — folded into the reset cycle. |
| **HUB init** — beyond the MMU (`v3d_mmu_set_page_table` + flush) the 4.2 probe writes no extra hub register: `v3d_init_hw_state` → `v3d_init_core` writes `MISCCFG` only for `ver < 40` (not us), and there is no probe-time `AXICFG`/hub-ident write. | MMU program + flush already kernel-faithful (M2). No other hub write. | Hub init **exonerated** — no missing register. |

**The one missing step: the core reset cycle (`v3d_reset_cycle`, `arch/aarch64/v3d.rs`).** A block out of
clean reset can fetch/consume a CL (CLE walks, QPU runs to program-end host-IRQ) while the frame-accounting
/flush unit sits in a **half-reset state that never latches FLDONE** — exactly the P46 signature (BMACTIVE
held, FLUSH consumed, QPU program-end IRQ seen, FLDONE never fires). UnaOS ran the ON half on a
firmware-powered block, so whatever stale internal state the firmware left was never cleared. V3D-50 mirrors
`reset_control_reset(BCM2835_RESET_V3D)` = `bcm2835_asb_power_off` (**OFF half**, new) then
`bcm2835_asb_power_on` (**ON half**, the existing PI-V3D-3 `enable_pm_asb`):

- **OFF** — stop the two async AXI bridges (set `ASB_REQ_STOP` with the PM password, wait `ASB_ACK` to
  **SET** = quiesced) master then slave; then **assert** the V3D reset (clear `PM_V3DRSTN` in `PM_GRAFX`,
  PM password). Holds the core in reset while the bridges are stopped, briefly, then releases via ON.
- **ON** — the unchanged deassert-reset + bridge-release sequence.

Sequenced after the firmware power/rate/gate and before the IDENT0 probe. Every PM/ASB write carries the PM
password; every wait is a finite CNTPCT backstop; readbacks are poison-honest. QEMU `raspi4b` models neither
the `rpivid_asb` block nor V3D, so the OFF-half bridge-stop backstop returns at once (unbacked reg reads 0,
`ASB_ACK` never sets) and the run lands on the honest `BLOCK-DOWN` — the whole step is dormant there, **P47
metal decides.**

**Witness wired.** `[v3d50]` prints the reset OFF half register-by-register (`PM_GRAFX` pre/post around the
`V3DRSTN` assert, each ASB `stop` cur→readback with the ACK verdict), then the empty rung is **re-run first
in the bisection ladder, tagged `[v3d50] empty-after-fix`**, as the direct before/after witness for the reset
cycle.

**Expected P47 witnesses.**
- `[v3d50] core reset CYCLE …` then `[v3d50] reset OFF — PM_GRAFX pre=<x> …`, `[v3d50] reset OFF — ASB stop
  V3D master … readback <y> — ACK set (bridge stopped)` (on metal the ACK sets; QEMU hits the backstop),
  `[v3d50] reset OFF — PM_GRAFX assert V3DRSTN … pre=<x> post=<x&~bit6>`.
- `[v3d50] empty-after-fix — retired=1 BFC Δ1 PCS=…BMACTIVE=0` **if** the OFF→ON core reset was the wall —
  which retires the whole seven-layer empty-bin investigation as an un-reset core. If it still reads
  `retired=0 BFC Δ0 …BMACTIVE=1` with the reset cycle proven applied, the wedge sits **below the CLE→PTB
  FLDONE generation itself** (a deeper hub/tile-state-flush step), and the retained `[v3d44/45/46]` wedge
  dump the `Empty` rung emits names it.

## 25. The missing post-reset core init — the L2T flush window (V3D-51)

**P47 delivered the reset-cycle verdict: the empty frame STILL does not retire.** The full OFF→ON ASB reset
cycle (§24) executed cleanly on metal — both bridges ACK-stopped, `V3DRSTN` asserted, `PM_GRAFX`
pre=`0x00001000` — and the empty rung read
`retired=0 BFC Δ0 PCS=0x00000001 (BMACTIVE=1 BMBUSY=0) idled=1 INT_STS=0 waited=500000us`. So the wedge is
**not** power/reset state: the CLE consumed the list and the binner claims active (`BMACTIVE=1`) but never
consumes/retires the CL (`BMBUSY=0`, `BFC` never increments). Everything the kernel writes **per job** (§10–23)
and the reset cycle (§24) are byte-exact — so V3D-51 re-audited the kernel's **post-reset core init**, the
step that runs between reset and the first job.

**The one factual error in §24's audit table — the unconditional `L2TFL*` writes.** §24 concluded
"`v3d_init_core` writes `MISCCFG` only for `ver < 40` (not us), and there is no probe-time
`AXICFG`/hub-ident write." That accounts only for the **conditional** MISCCFG branch. The kernel's
`v3d_init_core` (`v3d_gem.c`), for **every** V3D version, unconditionally writes the **L2T flush address
range**:

```
V3D_CORE_WRITE(core, V3D_CTL_L2TFLSTA, 0);      // 0x034 — flush window start
V3D_CORE_WRITE(core, V3D_CTL_L2TFLEND, ~0);     // 0x038 — flush window end
```

`v3d_init_core` is reached via `v3d_init_hw_state`, which the kernel calls at the **tail of every**
`v3d_reset_v3d`. STA=0/END=~0 establishes the flush window as the **whole address space**, so every
subsequent `V3D_CTL_L2TCACTL` `FLM=FLUSH` — both our per-kick `invalidate_gpu_caches` and the
frame-completion write-back the binner drives — walks the full range. `AXICFG` (also flagged in §24 as
un-audited) is written **only** in `v3d_reset_by_bridge`; UnaOS uses the ASB-power reset path
(`reset_control_reset`, proven on metal at P47), which the kernel does **not** follow with an AXICFG write
either — so `AXICFG` is *not* a divergence and is correctly left untouched.

**The differential — first divergence marked.**

| # | Kernel (post-reset, pre-job) | UnaOS (pre-V3D-51) | Verdict |
| --- | --- | --- | --- |
| 1 | `v3d_reset_v3d` tail → `v3d_init_hw_state` → `v3d_init_core`: **`L2TFLSTA=0`**, **`L2TFLEND=~0`** (all versions) | reset cycle (§24) then straight to MMU program — **L2TFL* never written**, left at POR | **DEFECT — the V3D-51 fix.** The per-kick `L2TCACTL FLM=FLUSH` walked an unestablished window. |
| 2 | `v3d_init_core` `MISCCFG=OVRTMUOUT` **only** `ver<41` | never writes MISCCFG (ver 42 ≥ 41) | Match (§24, re-confirmed). |
| 3 | `AXICFG=MAX_LEN_MASK` **only** in `v3d_reset_by_bridge` | ASB-power reset path, no AXICFG | Match — kernel's reset_control path skips it too. |
| 4 | per-job `CT0QMA→QMS→QTS\|ENABLE→QBA→BPOS=0→QEA`, `INT_MSK` unmasked once | byte-exact (§23) | Match. |

The V3D-50 power-cycle reset (§24) actively returns `L2TFLSTA/L2TFLEND` to POR — so adding the reset
without the init that the kernel **always** pairs with it left the L2T flush window in a state the kernel is
never in at job start. This is the first divergence **below** the byte-exact per-job programming.

**The fix.** `v3d_init_hw_state` (`arch/aarch64/v3d.rs`) writes `L2TFLSTA=0`/`L2TFLEND=~0`, sequenced to
mirror the kernel (`reset → init_hw_state → MMU reinit`): after the BLOCK-UP probe verdict (core-relative
writes are only safe once the block is confirmed up) and before M2. The `[v3d51]` witness echoes both
registers **before and after** — settling on metal whether the reset left the window at POR (the fix) or
firmware had established it (the wedge is below the L2T range too). The empty rung is then **re-run first in
the bisection ladder, tagged `[v3d51] empty-after-init-hw-state`**, as the direct before/after retire
witness.

**Expected P48 witnesses.**
- `[v3d51] init-hw-state — L2TFLSTA por=<x>->0x00000000 (want 0) | L2TFLEND por=<y>->0xffffffff …` — `por`
  values settle whether the reset left the window unestablished.
- `[v3d51] empty-after-init-hw-state — retired=1 BFC Δ1 PCS=…BMACTIVE=0` **if** the L2T flush window was the
  wall — retiring the empty-bin investigation as an un-init'd L2T flush range. If it still reads
  `retired=0 BFC Δ0 …BMACTIVE=1` with the window proven latched, the wedge sits **below the L2T flush range**
  too and the retained `[v3d44/45/46]` dump names the next layer.

QEMU `raspi4b` models no V3D block (the run returns at `BLOCK-DOWN` before the probe), so the whole V3D-51
step is dormant there — **P48 metal decides.**

## 26. The contingency rungs below the L2T window — hub-INT unmask + config audit (V3D-52)

**P48/P49 verdict: the L2T window fix was a NO-OP.** V3D-51's `[v3d51] init-hw-state` readback showed
firmware had **already** established `STA=0/END≈0` at POR — the reset never disturbed the window — so
`retired=0` stands. Wire decode (P49): the QPU program-end host interrupt latches (`INT_STS` bit16) but the
PTB never generates `FLDONE`; the break is in the **CLE→PTB frame-close handshake**, below the byte-exact
per-job programming (§10–23), the reset cycle (§24) and the L2T flush window (§25). V3D-52 works the ranked
contingency rungs below that floor. All three are dormant on QEMU (`BLOCK-DOWN`) — **metal decides.**

**Rung 1 — the hub-INT half of `v3d_irq_enable` (the last unmirrored byte-exact kernel probe write).**
The kernel's `v3d_irq_enable` (`v3d_irq.c`) is **four** register writes, not two: the per-core
`INT_MSK_SET=~V3D_CORE_IRQS` / `INT_MSK_CLR=V3D_CORE_IRQS` (mirrored since V3D-49) **and** the hub
`HUB_INT_MSK_SET=~V3D_HUB_IRQS` / `HUB_INT_MSK_CLR=V3D_HUB_IRQS`, both once at probe. `V3D_HUB_IRQS`
(ver<71) = `MMU_WRV|MMU_PTI|MMU_CAP|TFUC` = `0x3a`; hub `INT_MSK_STS/SET/CLR` sit at `0x5c/0x60/0x64`
relative to `V3D_HUB_BASE` (the same low slots as the core INT block but a **distinct** MMIO block; the
file's own hub `IDENT0..3` at `0x08..0x14` confirm the layout). UnaOS had never touched the hub mask — the
single remaining unmirrored probe divergence. `v3d_irq_enable` now writes the hub half after the core half,
with the `[v3d52] hub-irq-enable` witness echoing `HUB_MSK_STS` before/after. **Rationale, honestly
weighted:** §23 established that core `INT_STS` latches *raw* regardless of the core mask, so the hub likely
latches raw too — faithful-but-maybe-not-the-fix — but it is the last byte-exact divergence and the method
is "mirror exactly, then metal decides." If the hub aggregates/gates the core's frame-completion latch,
unmasking lets the Empty frame retire.

**Rung 2 — the `TILE_BINNING_MODE_CFG` (code 120) tile-state auto-init audit — finding: no such bit in
v42.** The contingency hypothesis was that a naked config+START+FLUSH never retires because the tile-state
array is auto-initialised by a config-packet bit the Empty rung leaves clear. **Audited against the
authoritative in-repo v42 field map** (the §V3D-46 8-field enumeration, byte-verified vs Mesa
`v3d_packet.xml` gen 4.2): the v42 code-120 packet has **no** auto-initialise-tile-state field. That field
existed only in the pre-v41 (v3.3) packet and was removed in the v41+ restructure — on V3D 4.2 the
tile-state array is initialised implicitly by `START_TILE_BINNING`, not by a config bit. The v42 field set
is exactly `{initial-block@2, block@4, RT-count@8, max-BPP@12, MSAA-4x@14, double-buffer@15, width@32,
height@48}`; bits 14/15 are correctly clear for our single-sample, single-buffer 64×64 frame. So the Empty
rung's config is **complete and byte-exact — no divergent bit, no behavioural change** (instrument-and-name
per the no-fabricated-fix rule). The `[v3d52] tile-binning-mode-cfg auto-init audit` witness records the
finding; the ladder's own differential stays the live test — if `state-no-prims` retires while `empty-frame`
does not, the gap is a **prologue/state packet**, not a config bit, and is named directly there.

**Rung 3 — the TMU write-combiner (TMUWCF) drain candidate, witness-staged (NOT armed).** The binner's
tile-list/tile-state writeback lands in L2T; the frame-flush completion the Empty rung waits on needs that
writeback drained through the L2T write-combiner. `invalidate_gpu_caches` writes `L2TCACTL FLM=FLUSH`
**pre-kick** and polls `L2TFLS` clear, but there is **no** post-START/pre-`FLDONE` combiner drain and
`V3D_L2TCACTL_TMUWCF` (bit8) is never written. Because QEMU cannot verify a flush-path change, Rung 3 is
**named, not armed**: the `[v3d52] tmuwcf-drain candidate` witness anchors the current `L2TCACTL` state so
that, if Rungs 1+2 read clean, the next boot can arm a post-flush `TMUWCF` drain inside the frame path with
a before-anchor. Weaker than #1/#2 — the wedge is `BMACTIVE` (frame open), not a stale readback.

**Expected witnesses (metal).**
- `[v3d52] hub-irq-enable — HUB_MSK_STS por=<x> -> now=<y> unmasked set=0x0000003a …` — `por` settles
  whether the hub working set was masked at reset; if unmasking retires the Empty rung it reads
  `[v3d52] empty-… retired=1 BFC Δ1 PCS=…BMACTIVE=0` in the ladder below.
- `[v3d52] tile-binning-mode-cfg auto-init audit — … MSAA-4x@14=0 double-buffer@15=0 … config is COMPLETE`
  — the audit is a pure finding; it changes no submitted byte.
- `[v3d52] tmuwcf-drain candidate (Rung 3, NOT armed) — L2TCACTL=<z> …` — the before-anchor for the
  next-boot drain.

If, with the hub set proven unmasked, the Empty rung still reads `retired=0 BFC Δ0 …BMACTIVE=1`, the wedge
is confirmed below all mirror-able kernel probe state and the retained `[v3d44/45/46]` dump + Rung 3 name
the next layer.

## 27. Rung 3 refuted for the bin path; the kernel-exact FLM=CLEAR pre-job invalidate (V3D-53)

**P51 verdict: Rungs 1+2 read clean, the Empty frame still does not retire.** The hub-INT unmask (§26 Rung 1)
executed on metal — `HUB_MSK_STS por=0 -> 0x5`, set `0x3a` written — and the code-120 config audit (§26
Rung 2) found the Empty rung's config **complete**. Yet the Empty frame still read
`retired=0 BFC Δ0 BMACTIVE=1 BMBUSY=0 INT_STS=0` after 500 ms. §26's staging said: Rungs 1+2 clean → arm
Rung 3, the TMUWCF combiner-drain candidate. V3D-53 went to arm it — and, sourcing the kernel first,
**refuted it for the bin path.**

**The TMUWCF drain is a post-render clean-caches op, never run for a bin job.** Sourced against the kernel
(`v3d_gem.c` / `v3d_sched.c` / `v3d_irq.c`, GPL-2.0-only, facts only), `V3D_L2TCACTL_TMUWCF` (bit 8) is
written in **exactly one** function — `v3d_clean_caches()`, which writes `L2TCACTL=TMUWCF` and polls it
clear, then writes `L2TCACTL` FLM=`CLEAN` and polls `L2TFLS` clear. `v3d_clean_caches` runs **only** as the
dedicated `V3D_CACHE_CLEAN` job (`v3d_cache_clean_job_run`), scheduled **after** the render job and gated on
the userspace `DRM_V3D_SUBMIT_CL_FLUSH_CACHE` flag. It is **not** in `v3d_bin_job_run`, **not** in
`v3d_render_job_run`, **not** in `v3d_irq`. So TMUWCF sits **downstream** of the bin FLDONE / BFC++ handshake
the Empty rung wedges on (`BMACTIVE=1`, frame still open). Arming a TMUWCF drain inside the bin frame path
would run a rung the kernel provably never runs for bin jobs — a fabricated fix. Per the no-fabricated-fix
law it is **NOT armed**; the `[v3d53] tmuwcf-drain REFUTED for bin path` witness records the sourced verdict.

**The derived next candidate — the kernel-exact FLM=CLEAR pre-job invalidate.** The last L2TCACTL
flush-mode / sequence divergence around a bin job is the per-job **input** invalidate. The kernel's
`v3d_invalidate_caches` (called before both bin and render jobs) is:

```
v3d_flush_l3(v3d);                 // no-op on BCM2711 — no L3
v3d_invalidate_slices(v3d, 0);     // SLCACTL <= all-0xF
v3d_invalidate_l2t(v3d, 0);        // L2TFLSTA=0; L2TFLEND=~0; L2TCACTL = L2TFLS | FLM=CLEAR
```

UnaOS's `invalidate_gpu_caches` diverges in **three** ways: (a) it writes L2T **first**, then SLCACTL (kernel
does slices first); (b) it does **not** re-establish the flush window per invalidate (we set it once in
`v3d_init_hw_state`, §25); (c) it uses **FLM=FLUSH** (writeback+invalidate, mode 0) — **not** FLM=CLEAR
(invalidate-only, mode 1). For a bin job every input (CL, tile-state, shader records, VBO) is CPU-published
to DRAM via `cache::clean_range`, so an invalidate-only re-fetch is exactly what the kernel does and is
byte-faithful. On a freshly-reset core the L2T holds no dirty lines, so CLEAR and FLUSH converge —
**faithful-but-weak-prior** (like §26 Rung 1), mirror-exact-then-metal-decides.

**Scope — the render-side FLUSH is left untouched.** `invalidate_gpu_caches` FLM=FLUSH is the metal-CONFIRMED
V3D-12 fix on the **render** pre-kick: it *publishes* the binner's tile lists to the render CLE (boot-P7 root
cause). V3D-53 does **not** touch it. The kernel-exact CLEAR path is added as a distinct helper
(`bin_prejob_invalidate_kernel_exact`) and wired **only** into the `[v3d53]`-tagged Empty rung, so the
metal-confirmed job paths (probe / M4 / battery) are unperturbed and the change is a controlled diagnostic.

**The differential.** The bisection ladder now runs the Empty rung twice back-to-back, identical except for
the invalidate: `[v3d51] empty-after-init-hw-state` (FLM=FLUSH, L2T-first) and
`[v3d53] empty-after-clear-invalidate` (FLM=CLEAR, slices-first, window re-established). If `[v3d53]` retires
while `[v3d51]` wedges, the pre-job invalidate mode/sequence was the wall. If **both** wedge, the pre-job
cache invalidate is exonerated and the wedge is confirmed **below all mirror-able L2TCACTL state** — the
retained `[v3d44/45/46]` dump names the next layer (below the CLE→PTB FLDONE generation).

**Expected P52 witnesses (metal).**
- `[v3d53] tmuwcf-drain REFUTED for bin path — … NOT armed. L2TCACTL=<z> …` — the sourced Rung 3 verdict.
- `[v3d53] empty-after-clear-invalidate kernel-exact pre-job invalidate — L2TCACTL <a>-><b> (FLM: ours=FLUSH(0)
  -> kernel=CLEAR(1); SLCACTL-first + L2TFLSTA=0/L2TFLEND=~0 re-established) …` — the before/after anchor.
- `[v3d53] empty-after-clear-invalidate — retired=? BFC Δ? PCS=…` — the differential verdict against the
  `[v3d51]` empty rung directly above.

QEMU `raspi4b` models no V3D block (the run returns at `BLOCK-DOWN` before the probe), so the whole V3D-53
step is dormant there — **P52 metal decides.**

## 28. The next probe class — CL-progression time-series + empty-bisect submission audit (V3D-54)

Through §27 every **mirror-able** probe register is kernel-byte-exact (per-job §10–23, reset §24, L2T window
§25, hub-INT §26, CLEAR invalidate §27), and each landed a **confirmed no-op** on metal. The prior single-shot
sampling — read the CLE/PTB registers **once**, after the 500 ms `wait_fldone` returns — hid **two different
wedge signatures** the scout survey pulled out of the P52 capture:

| Job | CT0CS | CT0CA | reading |
| --- | --- | --- | --- |
| **PROBE bin** | `CTRUN=0` | **`=EA`** | CLE walked the whole list, QPU ran, `BPCA` advanced off pool base (PTB emitted bytes) — then FLDONE/`BFC` never fired. |
| **Empty bisect** | `CTRUN=1` | **`=BA`** | CLE **never advanced off BA** — GO accepted but the list was never stepped; backstop timeout. |

**Same begin address, opposite outcome.** The whole "empty frame MUST retire" premise (§22–27) is being tested
against a frame whose CLE **never ran** — so "empty did not retire" may be a submission/harness artifact, not a
frame-close failure. V3D-54 attacks that contradiction directly, not with another mirror.

**RANK 1 — CL-progression time-series (`[v3d54] trace`).** `wait_fldone` now takes the submitted `[BA,EA)` and
polls `CT0CA`/`CT0CS`/`CT0LC`/`CT0PC`/`BPCA` at a **~1 ms cadence across** the retire-wait, not once after it.
Each register is folded into a `TraceReg` (first / last / #changes / µs-of-first-move / µs-of-last-change), so
the log carries a **compact transition summary** — start offset, stall offset, end offset — **not** 500 raw
samples. One `[v3d54] trace` line fires on **both** exit paths (FLDONE-retire and timeout), for **both** the
full PROBE bin and **every** bisect rung. Its interpretation names the fork:
- `CT0CA` never leaves BA ⇒ the CLE never fetched; GO a no-op / list mis-submitted → **RANK 2 decides**.
- `CT0CA` advances then stalls at offset *N* ⇒ the CLE choked on the packet at that byte offset.
- `CT0CA` reaches EA ⇒ CLE walked the whole list; `BPCA` advance vs freeze then splits *PTB-emitted-bytes* from
  *nothing-written* — the wall is downstream (FLDONE/BFC generation), not the CLE walk.

**RANK 2 — empty-bisect submission audit (`[v3d54] submit` + `[v3d54] resubmit`).** After each GO the latched
`CT0QBA`/`CT0QEA` are read **back** and checked against the intended `[BA,EA)` and the **built CL byte length**
(the Empty rung is 14 B: `NUMBER_OF_LAYERS`+`TILE_BINNING_MODE_CFG`+`FLUSH_VCD_CACHE`+`START_TILE_BINNING`+`FLUSH`).
`v3d54_submit_audit` returns *sound* only when `CT0QBA==BA ∧ CT0QEA==EA ∧ EA−BA==len ∧ EA≠BA`. If an **Empty**
rung's submission is **unsound** *and* it did **not** retire, the FIX-and-re-run leg re-latches
`QMA/QMS/QTS/QBA` with strict per-write `dsb` fencing and re-issues the GO **once in the same boot**, then
re-audits + re-traces under `[v3d54] resubmit`. The three outcomes are self-labelling: re-latch sound + retires
⇒ the wedge **was** the submission (retract the empty-frame premise); re-latch sound + still wedged ⇒ the queue
registers were not the wall (a genuine frame-close fact stands); `EA==BA` persists through a fenced re-latch ⇒
the defect is **upstream of the queue write** (the GO path itself), still a submission fact.

**Scope.** Read-only MMIO except the metal-gated re-GO, which fires **only** when the audit proves the first
submission unsound — a strict no-op whenever it reads sound (and unobservable on QEMU, where no V3D block means
`submit_sound` is never false). The metal-confirmed job paths are otherwise unperturbed; the trace fold is
bounded (five reads per ~1 ms tick, no per-sample logging).

**Expected P53 witnesses (metal).**
- `[v3d54] submit (v3d40 PROBE / <rung>) — intended BA=… EA=… len=… | latched CT0QBA=… CT0QEA=… span=… — …` —
  the per-kick submission audit; the Empty-rung lines are the load-bearing ones.
- `[v3d54] trace (<what>) samples=N span=…us … CT0CA off …->… moves=… stall@…us … BPCA …->… adv=… — <fork>` —
  the folded progression for the PROBE bin and each rung.
- `[v3d54] resubmit (<rung>) — re-latch sound=? retired=? …` — **only** if an Empty rung's first submission read
  unsound and it did not retire.

QEMU `raspi4b` models no V3D block (the run returns at `BLOCK-DOWN` before the probe), so the whole V3D-54 step
is dormant there — **P53 metal reads the trace + audit.**

## 29. Clock-domain liveness + tile-state readback — the two branches left (V3D-55)

P53 metal closed the register-mirror line for good. The bin wall is now stated exactly:

- **Submission is SOUND.** `[v3d54] submit` reads `CT0QBA==BA`, `CT0QEA==EA`, `span==len` on every kick — the
  CLE is handed precisely the list we built. The §28 "empty rung was mis-submitted" escape hatch is closed.
- **The CLE walks.** The first kick's `CT0CA` traverses `BA→EA` in full and the QPU runs (`INT_STS` bit16
  latches a program-end host interrupt).
- **The PTB writes nothing.** `BPCA` never advances, `FLDONE` never fires, and because `CTRUN` stays `1`, every
  *later* kick is a no-op — which is why the bisect rungs read as "never stepped".
- **Every mirror is a byte-exact no-op.** L2T window (§25), hub-INT unmask (§26), `FLM=CLEAR` pre-job invalidate
  (§27), TMUWCF (§27, refuted for the bin path). Mirroring more kernel registers cannot move this.

Two orthogonal questions survive, and V3D-55 asks both in one boot. All output is new `[v3d55]`-tagged serial
witnesses on the existing probe path (default-quiet: they fire only where the V3D probe battery already runs),
and all MMIO is **read-only** save one explicitly-disarmed write.

### RANK 3 — is the flush domain even clocked?

The QPU provably executes, but QPU execution and the CLE/PTB flush unit need not share a live clock domain. A
dead flush clock would produce *exactly* the observed signature and no packet-level fix could ever cure it.

**(a) `CYCLE_COUNT` delta across the wait (`[v3d55] clkliv`).** PCTR counter 2 (`src32 CYCLE_COUNT`) is now
folded into the same `TraceReg` cadence `wait_fldone` already runs for the §28 trace, and emitted on **both**
exit paths. The PCTR **enable mask** is captured at the start of the wait so an unarmed wait reports *no
verdict* rather than fabricating a flat-clock reading (the battery is armed only around the PROBE bin).

**(b) firmware clock readback (`[v3d55] clkdom`).** `bringup` *commands* 500 MHz via `set_clock_rate` and has
never read back what firmware **granted**. `GET_CLOCK_RATE` + `GET_CLOCK_STATE` for `CLOCK_ID_V3D` now print
the granted rate, the gate-active bit and the does-not-exist bit against the commanded value.

> **Transport failure ≠ a 0 Hz grant.** The existing `mailbox::get_clock_rate` deliberately folds *both* a
> failed transaction and a successful one reporting `rate == 0` into `None` — correct for its EMMC2 caller
> ("a usable rate or nothing"), fatal here: a firmware that grants the V3D clock **0 Hz** is the single most
> diagnostic RANK-3 reading available, and collapsing it would print it as a dead mailbox and leave the 0 Hz
> verdict unreachable. V3D-55 adds `mailbox::get_clock_rate_raw`, which returns `None` **only** on an
> `mbox_call` failure so `Some(0)` is a real zero grant. Additive — `get_clock_rate`'s contract and its
> existing callers are untouched. (`get_clock_state` is likewise now compiled for the `v3d` feature, not
> only `piusb`, so the audit does not depend on `piusb` happening to be enabled in the same build.)

**(c) `MISCCFG.QRMAXCNT[3:1]` audit (`[v3d55] misccfg`).** The queued-request max count has been *declared* in
`v3d.rs` since V3D-34 and never programmed **or audited**. Linux `v3d_init_core` writes `MISCCFG` only on
`ver<41` (the Pi 4 is v42) and the bcm2711 DT programs no QRMAXCNT for v3d, so the expected value is the
block's reset default — this line puts that default on serial for the first time. A **floored** (`0`) read
would mean a starved PTB request queue: a genuine BCM2711 knob never touched.

> **The QRMAXCNT write stays DISARMED.** `V3D55_ARM_QRMAXCNT = false`. The brief permits the write only behind
> *evidence of divergence*, and V3D-55 is the boot that first **collects** that evidence. The write is doubly
> gated — the const must be flipped by a future arc **and** the block must actually read floored at runtime —
> so it is never issued blind. When armed it is a scoped read-modify-write of the `[3:1]` field alone (every
> other `MISCCFG` bit preserved) with an echo-back, reported as `[v3d55] qrmaxcnt`.

### RANK 4 — did the PTB write anything? (`[v3d55] tilestate` / `pool` / `pte`)

`bin_pool_witness` reads only the **first 8 bytes** of the pool and of the tile-state array — too coarse to
separate *PTB-wrote-but-no-FLDONE* from *PTB-never-wrote*. After the PROBE bin, `v3d55_tilestate_readback`
performs an L2T write-back (the §18/V3D-42 order: drain the binner's L2T-parked bytes to DRAM, *then* drop the
CPU's stale lines), scans the **whole** `TILE_STATE_BYTES` TSDA **and the whole `BIN_TILEALLOC_BYTES` pool**
classifying every word against the fill published before the kick, and prints `BPCA`/`BPCS`/`BFC` alongside.

Three integrity properties this readback must carry, because the negative reading is the interesting one:

- **The classification is against the pre-kick fill, not against zero** (the R0 fix, §49.4). Both regions are
  pre-filled with the V3D-56 poison `0xA5A5A5A5 ^ i`, which is nonzero at *every* index. The original line
  counted **nonzero words** and read any nonzero count as "the PTB WROTE" — so an entirely **untouched**
  region scored `nonzero_words == words` and the witness asserted a PTB write on the strength of its own
  poison. Every `[v3d55] tilestate` / `pool` "wrote" verdict taken since V3D-56 armed the poison was therefore
  inverted. The line now reports three disjoint counts — `INTACT` (the word equals its pre-kick fill ⇒ **not**
  written), `ZEROED` (the word reads `0` where the fill was not `0` ⇒ written, with zero bytes — what an empty
  tile list emits), `WRITTEN` (any other value ⇒ written, with real data) — plus `touched = ZEROED + WRITTEN`
  and the first non-poison word with its index, value and expected value. When `V3D56_POISON` is off the two
  regions are pre-zeroed, `ZEROED` is unreachable, and the line states on the wire that no write/no-write
  verdict follows.

- **The write-back's completion is threaded into the verdict.** `invalidate_gpu_caches` discards its `L2TFLS`
  poll result; V3D-55 runs the byte-identical sequence inline and **keeps** the completion bit. Under exactly
  the failure this arc hunts — a dead or half-clocked flush domain — the flush silently no-ops, the binner's
  bytes stay parked in L2T and DRAM reads zero *for a reason that has nothing to do with the PTB*. When
  `flush_done == false` the line issues **no** upstream/downstream verdict at all.
- **The pool scan covers the whole pool.** A "the pool is empty, therefore `BPCA` is a phantom" claim is a
  claim about the *whole* pool, and P40 observed `BPCA` some `0x3000` bytes in — past any prefix. A prefix
  scan could never have supported that claim, so the scan matches the claim's scope.

It then dumps the V3D **PTEs** covering the CL, tile-state and tile-alloc iovas — validity, writeability and
true identity.

> **What the PTE lines are and are not.** They read our CPU-side `V3D_PT` static: the mapping we *intended and
> published*. They are **not** a hardware translation probe and cannot, alone, detect a table whose cleaned
> bytes never reached DRAM, a mis-latched `PT_PA_BASE`, or an MMU that is not enabled. V3D-55 therefore emits
> `[v3d55] mmucfg` first — `MMU_CTL.ENABLE`, the latched `PT_PA_BASE` (pages) against the table we programmed,
> and the standing fault bits — read back from the GPU's own configuration. The **pair** carries a real
> discrimination; either half alone does not, and the §29 claim is scoped accordingly.

### Expected discriminations

| Witness | Reading | Verdict |
| --- | --- | --- |
| `[v3d55] clkliv` | `CYCLE_COUNT` **flat** across the 0.5 s wait | The core/flush domain is **not ticking** — the fix is a clock/QoS one, not a packet one. Cross-check `clkdom`. |
| `[v3d55] clkliv` | `CYCLE_COUNT` **advances**, FLDONE still dead | The **core counter domain** is live. This does **not** close the clock branch — the CLE/PTB flush unit may sit on a separately-gated sub-domain this counter cannot observe; it rules out a wholly-unclocked core. With a clean `clkdom`, "clocked but never **triggered**" becomes the leading reading. |
| `[v3d55] clkdom` | gate inactive / rate 0 / granted ≠ 500 MHz | Powered-but-unclocked (or clamped) — alone sufficient to explain a dead flush unit. |
| `[v3d55] misccfg` | `QRMAXCNT == 0` | Request queue **floored** — arm `V3D55_ARM_QRMAXCNT` next arc and re-run. |
| `[v3d55] misccfg` | `QRMAXCNT != 0` | No divergence; the QoS branch is **not** justified — refuted without a write. |
| `[v3d55] tilestate` | `WRITTEN` **> 0** | The PTB **wrote** per-tile output ⇒ the defect is isolated to the **FLDONE/BFC frame-close latch itself** — the narrowest target the campaign has ever had. |
| `[v3d55] tilestate` | `WRITTEN = 0`, `ZEROED` **> 0** | The PTB **wrote zeros** — the write an empty tile list makes. Still a write: no "never written" reading may be taken. A class the pre-zeroed era could not observe. |
| `[v3d55] tilestate` | `INTACT = words` (`touched = 0`), `flush_done = 1` | The poison is untouched ⇒ on this evidence the PTB wrote nothing and the defect is **upstream** of frame-close. |
| `[v3d55] tilestate` / `pool` | `touched = 0`, `flush_done = 0` | **No verdict.** `L2TFLS` never cleared, so the bytes may still be parked in L2T — read `clkliv`/`clkdom` first. |
| `[v3d55] tilestate` / `pool` | `INTACT = words` with `fill=zero` | **No verdict** — the build pre-zeroed the region (`V3D56_POISON` off), so "never written" and "written with zeros" are the same reading. Re-arm the poison. |
| `[v3d55] pool` | `touched = 0`, `flush_done = 1`, `BPCA` advanced off base | **Not** a phantom pointer. `BPCA` is the pool's *allocation* pointer (§30), so a reservation advance over a provably untouched pool is architectural for a frame that binned no primitives — compare the printed advance against the Mesa prediction on the same line. |
| `[v3d55] pool` | `BPCA == 0x0` | **Not** "parked at the pool base" — `0x0` is not the pool base. Either a genuinely reset register or a block not returning live values; check the `[v3d45]` dump for a block-wide zero/poison pattern. |
| `[v3d55] mmucfg` | `ENABLE = 0`, or `PT_PA_BASE` ≠ ours | The GPU is not consulting (or not walking) the table the `pte` lines decode — the aliasing question is wide open and those lines describe memory the hardware never reads. |
| `[v3d55] pte` | any PTE invalid / non-identity / read-only | The mapping we **published** is wrong for that region ⇒ a silent no-write is an **addressing** defect, not a frame-close one. |

### Witness line formats

```
:: V3D: [v3d55] clkdom (<tag>) — commanded=500000000 Hz | GET_CLOCK_RATE=<hz> (mailbox OK (rate verbatim,
        0 = a real 0 Hz grant) | FAILED (Hz field meaningless))
        GET_CLOCK_STATE=0x……… (active=<0|1> not_exist=<0|1>, mailbox OK|FAILED) — <verdict> ::
:: V3D: [v3d55] misccfg (<tag>) — MISCCFG=0x……… OVRTMUOUT(bit0)=<0|1> QRMAXCNT[3:1]=<n>
        — EXPECTED: reset default … — <verdict> — scoped QRMAXCNT write DISARMED|ARMED… ::
:: V3D: [v3d55] qrmaxcnt (<tag>) — MISCCFG 0x……… -> wrote 0x……… -> echo 0x……… (QRMAXCNT n -> m) — <verdict> ::
:: V3D: [v3d55] clkliv (<what>) — PCTR_EN=0x……… counter2(src32 CYCLE_COUNT) armed=<0|1>
        0x………->0x……… Δ=<n> moves=<n> first_move=<n>us last_change=<n>us over <n>us (<n> samples) — <verdict> ::
:: V3D: [v3d55] tilestate (<tag>) — TSDA iova=0x……… bytes=<n> words=<n> fill=<poison(0xA5A5A5A5^i)|zero>
        INTACT=<n> ZEROED=<n> WRITTEN=<n> touched=<n>
        | first_nonpoison=[<i|-1>] (got 0x……… expected 0x………)
        | L2T write-back completed=<0|1> (L2TCACTL after=0x………) | head w0..w7 = 0x……… ×8 — <verdict> ::
:: V3D: [v3d55] pool (<tag>) — pool iova=0x……… bytes=<n> words=<n> fill=<poison(0xA5A5A5A5^i)|zero>
        INTACT=<n> ZEROED=<n> WRITTEN=<n> touched=<n>
        | first_nonpoison=[<i|-1>] (got 0x……… expected 0x………) (FULL-pool scan)
        | L2T write-back completed=<0|1> | head w0..w7 = 0x……… ×8
        | BPCA=0x……… (adv 0x… off pool base, Mesa-predicted empty-bin advance 0x3000)
        BPCS=0x……… BFC=0x……… — <verdict> ::
:: V3D: [v3d55] mmucfg (<tag>) — MMU_CTL=0x……… ENABLE=<0|1> fault_bits=0x……… | PT_PA_BASE=0x……… (pages)
        vs ours=0x……… (table phys 0x………) — <verdict> ::
:: V3D: [v3d55] pte (<tag>) <region> iova=0x……… — CPU-side PT[<idx>]=0x……… (our PUBLISHED table, not a
        hardware translation — pair with [v3d55] mmucfg) VALID=<0|1> WRITEABLE=<0|1>
        pfn=0x… (maps phys 0x………) — <verdict> ::
```

`[v3d55] qrmaxcnt` fires only under the doubly-gated arm; `clkliv` fires on both `wait_fldone` exits; `mmucfg`
precedes the three `pte` lines, which cover `bin CL`, `tile-state` and `tile-alloc`.

**Noted perturbation.** `[v3d55] clkdom` runs two mailbox round-trips, so a firmware transaction now interposes
between the pre-kick L2T flush and the GO. Acknowledged and judged low-risk — the mailbox touches no V3D state
and the CL/tile-state/pool are all CPU-published-and-cleaned before it — but it is a real change to the timing
of the metal-confirmed kick path and is recorded here so a P54 divergence has this on the suspect list.

QEMU `raspi4b` models no V3D block — the run returns at `BLOCK-DOWN` before the probe, so **no** `[v3d55]` line
appears there and the `kernel8-test` battery green means *no regression*, not probe verification. **P54 metal
reads the clock-domain and tile-state verdicts.**

## 30. The phantom retracted — BPCA is an allocation pointer, not a write pointer (V3D-56)

P54b metal plus the V3D-55 probes left the arc holding a hard verdict: **BPCA is a phantom/aliased write
pointer.** Its evidence was:

- submission sound, CLE walks `BA→EA`, QPU runs, `FLDONE` never fires, `CTRUN` stays `1`;
- firmware V3D clock **ACTIVE at exactly 500 MHz**; `MISCCFG=0x6` (`QRMAXCNT=3`, sane); `CYCLE_COUNT` advanced
  **249 M** over the 500 ms wait — the core is clocked, so "clocked but never triggered";
- GPU MMU `CTL=0x00090801`, `ENABLE=1`, no fault bits, `PT_PA_BASE` matching our table, PTEs identity-mapped
  valid + writeable as published;
- and the pivot: the tile-state array (192 B) **entirely zero** and the full 32 KiB tile-alloc pool **entirely
  zero** after a *completed* L2T write-back — yet `BPCA=0x001fa000`, advanced `0x3000` off the pool base
  `0x001f7000`, with `BPCS=0x5000` and `BFC=0`.

Advanced pointer, empty pool, healthy MMU. The only reading left seemed to be that the bytes landed somewhere
we were not reading. **That inference was wrong, and this section retracts it.**

### The register-semantics check that collapsed it

`BPCA`/`BPCS` sit at core offsets `0x300`/`0x304` on v42 — identical to VC4/V3D 2.x (Linux
`drivers/gpu/drm/v3d/v3d_regs.h`: `V3D_PTB_BPCA 0x00300`, `V3D_PTB_BPCS 0x00304`; the same offsets appear in
`drivers/gpu/drm/vc4/vc4_regs.h`). The upstream headers are comment-free; the field text is the VideoCore IV 3D
Architecture Reference Guide's, and v42 keeps it:

| Register | ARG description | Kind |
|---|---|---|
| `V3D_BPCA` | "Current Address Of Binning Memory Pool" | read-only **allocation pointer** (byte address) |
| `V3D_BPCS` | "Remaining Size Of Binning Memory Pool" | read-only **bytes remaining** |
| `V3D_BPOA` | "Address Of Overspill Binning Memory Block" | overspill base |
| `V3D_BPOS` | "Size Of Overspill Binning Memory Block" | overspill size (`0` = disarmed) |

Neither is a bytes-*written* counter. The ISA's written-work counters are `CT0LC`/`CT0PC`. **This driver read an
allocation pointer as a write pointer** — that single conflation is the whole phantom.

The decisive mechanism is in Mesa `src/broadcom/common/v3d_util.c` (`v3d_tile_alloc_sizes`), quoted:

> The PTB will request the tile alloc initial size per tile at start of tile binning. The size must match the
> initial block size configured in the `TILE_BINNING_MODE_CFG` packet.
> […] The PTB allocates in aligned 4k chunks after the initial setup.
> […] Include the first two chunk allocations that the PTB does so that we definitely clear the OOM condition
> before triggering one (the HW won't trigger OOM during the first allocations).

```c
uint32_t tiles_size = layers * tiles_x * tiles_y * V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE;
uint32_t alloc_size = align(tiles_size, 4096);
alloc_size += 8192;
```

So `START_TILE_BINNING` makes the PTB **reserve** per-tile initial blocks unconditionally — before, and
independent of, any primitive — then take aligned 4 KiB chunks, at least two of them up front.
**Reservation moves `BPCA` and writes nothing.**

### The arithmetic matches to the byte

Our frame is 64×64 with 64×64 tiles, one layer, so `tiles_x = tiles_y = 1`, and our `TILE_BINNING_MODE_CFG`
programs the 128 B initial block (`V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE`, `src/broadcom/common/v3d_limits.h`):

```
tiles_size            = 1 × 1 × 1 × 128   =    128 B
align(128, 4096)                          = 0x1000
+ two up-front 4 KiB chunk allocations    = 0x2000
──────────────────────────────────────────────────
predicted BPCA advance, EMPTY bin         = 0x3000
```

Measured at V3D-40 and again at V3D-55: **exactly `0x3000`** — the formula's value to the byte.

#### Two scope limits, both load-bearing

Neither may be dropped when this finding is quoted.

**(1) The `0x3000` match is NON-DISCRIMINATING on its own.** Reservation rounds up to 4 KiB, so a bin that wrote
a *small tile list inside the reserved initial block* leaves `BPCA` at exactly the same `0x3000`. Advance-matches-
prediction is therefore consistent with both "wrote nothing" and "wrote a little", and cannot separate them.
**Only the poison separates them.** Accordingly the retraction verdict in `[v3d56] bpca-vs-bytes` is gated on the
**conjunction** — advance `==` predicted **AND** poison fully intact — never on the advance alone. A match with a
*disturbed* poison is a different (and much more interesting) result, and the witness says so.

**(2) The `0x3000` was measured on the FULL probe draw list, not an empty one.** The probe kicks
`build_bin_cl_generic` — a complete draw list with geometry — which is only *effectively* empty because the coord
shader never dispatches (`valid_instr=0`, the §14/§17 wall). So the honest statement is: the advance is
architectural for a frame that **binned no primitives**, whatever the list nominally contained. Whether the §22
bisect *Empty* rung produces the same `0x3000` has never been measured, and that measurement would close the gap.

#### Verdict, scoped

**`BPCA` advancing `0x3000` over a pool the poison proves untouched is ARCHITECTURAL for a frame that binned no
primitives.** On that conjunction the phantom verdict is **retracted** — there would be no phantom bytes because
there were never any bytes; `BPCA` reports *reserved* space and this driver read it as *written* space. The
address question does **not** reopen, and the V3D-55 MMU evidence (enabled, fault-free, our table latched, PTEs
valid) stands unchallenged.

The defect collapses back to where §23–§27 had it: **`FLDONE` generation on an empty frame.**

> **APPENDED CORRECTION (§49.22.4 item 1, PI-V3D-97, 2026-08-18).** The register semantics above stand
> and are confirmed twice more (§49.22.2 W7). The clause *"there would be no phantom bytes because there
> were never any bytes"* does **not**. There **are** bytes — a sparsely populated ~`0x80`-byte header at
> the pool base — whenever the frame closes on a `CT0QMA` the V3D MMU accepts: §49.8's boot6 and §49.22's
> leg C both measured it, 20 words each. The boots this section rested on ran behind `probe_job`'s wedged
> frame (§49.3, §49.8), where no close-time write ever happened to be refused *or* to land. Corrected
> statement: a reservation advance over an untouched pool is architectural only where the write was
> refused or the frame never really closed; where the base is accepted and the frame closes, the advance
> and the write happen together.

> **STATUS: PENDING FIRST METAL BOOT.** The register semantics and the Mesa formula are settled from source and
> need no confirmation. But the retraction's second conjunct — *poison intact* — has never been observed, because
> **no `[v3d56]` line has ever run**. Until a P56 metal boot prints one, everything in this section downstream of
> the source citations is a well-supported **prediction, not a measured result**.

### Item 1 — poison the pool, so a zero-valued write stops being invisible

Every boot from V3D-40 to V3D-55 **pre-zeroed** the pool and tile-state, then concluded from an all-zero
readback that the PTB wrote nothing. That inference has a blind spot: a zeroed region cannot distinguish *never
written* from *written with zeros* — and zero-valued output is exactly what an empty tile list emits.

V3D-56 replaces the zero fill with an index-encoding poison over the same two regions (same bytes touched, only
the value differs), cleaned to PoC so the GPU sees it:

```
word[i] = 0xA5A5A5A5 ^ i
```

Recognisable by its high half, and self-locating — a word found displaced still names the pool index it came
from. After the job every word classifies as **INTACT** / **ZEROED** (`v == 0`, the class no prior boot could
observe) / **OVERWRITTEN**, reported with first- and last-touched index and byte span.

`V3D56_POISON` (default `true`) gates it; set `false` to restore the historical pre-zeroed pool for a
like-for-like diff against the V3D-40..55 logs.

### Item 2 — the alias question, and why the brief's form of it does not apply

The arc brief asked for a CPU read of the pool's page through the BCM2711 `0xC0000000` uncached-SDRAM alias and
the `0x80000000` alias. **That premise does not hold on this part, and acting on it would have manufactured a
false positive:**

- The `0x0`/`0x4`/`0x8`/`0xC0000000` quadrant aliases are **VideoCore bus** addresses — the cache-behaviour
  selectors the VPU's MMU applies. They are what the mailbox returns for a firmware buffer and what a VPU-side
  peripheral consumes.
- **V3D on BCM2711 does not issue VC bus addresses.** It issues IOVAs into its *own* MMU (`V3D_MMU_CTL` /
  `PT_PA_BASE` — the table this driver publishes), and the translated result goes onto the SoC fabric as a plain
  physical address. There is no VC-alias stage anywhere in that path.
- On a 4 GiB Pi 4, ARM physical `0xC0000000` is **real, distinct DRAM** (BCM2711 puts the low peripheral window
  at `0xFC000000` and main peripherals at `0xFE000000`; everything below is contiguous RAM). Reading it and
  finding nonzero bytes would prove only that some unrelated part of the system owns that memory.
- Establishing such a mapping needs `memory.rs`, **outside this arc's lane** — a STOP tripwire independent of
  the above.

The in-lane substitute is strictly stronger for the actual question ("where did the bytes land?"). The **only**
addresses the V3D MMU grants this job are the identity-mapped arena pages, so if the PTB wrote anywhere the GPU
can reach, the bytes are in the arena. `[v3d56] landing` digests **every** arena page (order-sensitive, so a
permutation still registers) immediately before the GO and again after the post-retire readback, and reports
which pages changed.

Changes are classified against a labelled whitelist of the regions a **correctly-functioning** job writes; only
the remainder is **STRAY**:

| Region | Pages | Why it is expected |
|---|---|---|
| tile-state | `OFF_TILESTATE` | bin output under test |
| tile-alloc pool | `OFF_BIN_TILEALLOC` +32 KiB | bin output under test |
| probe TMU scratch | `OFF_PROBE_SCRATCH` | **the write the probe series exists to produce** (§13/§17) |
| PTB overspill | `OFF_PROBE_BIN_OVERFLOW` +8 KiB | architectural `BPOA` spill; the `OUTOMEM` path services it |

The last two matter for evidence integrity. The probe's TMU-store target is written *by definition* when the
probe finally works, and an overspill write is the PTB doing exactly what the architecture specifies. Treating
either as STRAY would make the sweep shout "the phantom is located" **precisely when the GPU started working** —
so both are whitelisted with labels, and the "everything changed is expected" verdict states outright that a
scratch or overspill hit is what *success* looks like. A misplaced store is still adjudicated, by the separate
§13 canary scan that covers the scratch page's tail.

A stray page names the landing zone by offset. No stray page, with `BPCA` advanced, is the reservation reading
confirmed over the whole reachable address space rather than inferred from two pre-zeroed regions.

The post-job invalidate uses `cache::invalidate_range` rather than `clean_invalidate_range`. This is a
belt-and-braces preference, **not** a claim that the clean variant is unsafe — `v3d55_tilestate_readback` calls
`clean_invalidate_range` over the same regions one call earlier in this path, and that call is correct. Every
arena writer in this file cleans to PoC after writing (the poison fill included), so no dirty CPU lines exist
over the arena here and the two primitives are equivalent. The plain invalidate is chosen only because it cannot
*become* wrong if a future writer forgets its clean: a clean pass would then write CPU-stale bytes back over
GPU-written ones, destroying the evidence this sweep exists to find.

### Item 4 — the FLDONE interrupt triple

With item 3 collapsing the defect back onto empty-frame `FLDONE`, `[v3d56] int` dumps the status/mask/masked
triple at wait **entry** and **exit**, on *both* `wait_fldone` exits (the timeout exit is the one the empty rung
takes, so it is the one that must speak).

Mask polarity is confirmed upstream — `v3d_irq_enable` (`drivers/gpu/drm/v3d/v3d_irq.c`) writes
`INT_MSK_SET = ~V3D_CORE_IRQS` then `INT_MSK_CLR = V3D_CORE_IRQS`, so **1 = MASKED**, as this driver has assumed
since V3D-49. Note the line's own caveat: `wait_fldone` polls `INT_STS`, the **raw** latch, which the mask does
not gate — so a masked-but-latched `FLDONE` would already have been visible. The triple puts that reasoning on
serial with numbers instead of leaving it implicit, and discriminates *flush never happened* from *flush
happened, delivery masked*.

Two further upstream facts fold in, and both **exonerate** current code:

- **`FLDONE` is asserted by the binning FLUSH completing, not by `CT0CA` reaching `CT0EA`.** `CT0CA==CT0EA` says
  only that the CLE consumed the list. The required BCL shape is `TILE_BINNING_MODE_CFG` → `START_TILE_BINNING`
  → (draws) → `FLUSH` (packet code 4, `src/broadcom/cle/v3d_packet.xml`), kicked by `CT0QBA` then `CT0QEA`.
  This driver's empty rung already emits exactly that chain (`P_START_TILE_BINNING = 6`, `P_FLUSH = 4`) — **the
  list shape is not the gap.**
- **`ETFILT`/`ETPROC`/`ETSFLUSH` cannot be the cause.** Those empty-tile controls live in `V3D_CLE_CT1QCFG` —
  **render-side (CT1)** config (`v3d_regs.h`) — and cannot suppress a bin-side `FLDONE`.

Also corrected against §19/§23: mainline `v3d_regs.h` defines **no bit fields at all** for `V3D_CLE_CT0CS`
(`0x00100`) — no `CTRUN`, no `CTRSTA`, no flush bits. Any `CTRUN`-based retire test is driver folklore, not an
upstream-sanctioned interface; `FLDONE` is the only bin-retire signal the kernel consumes.

### Witness formats

```
:: V3D: [v3d56] armed — tile-state (<n> B) + tile-alloc pool (<n> B) filled with poison
        word[i] = 0xa5a5a5a5^i (was: zeroed) and cleaned to PoC. … ::
:: V3D: [v3d56] poison (<tag>) <region> iova=0x……… words=<n> — INTACT=<n> ZEROED=<n> OVERWRITTEN=<n>
        touched=<n> | first_touched=[<i>] (got 0x……… expected 0x………) last_touched=[<i>]
        | byte span [0x…,0x…] | L2T write-back completed=<0|1> — <verdict> ::
:: V3D: [v3d56] bpca-vs-bytes (<tag>) — BPCA=0x……… pool_base=0x……… advance=0x… (<n> B)
        | Mesa v3d_tile_alloc_sizes predicts 0x3000 for an EMPTY bin on this 1x1-tile frame
        (align(1*1*1*128,4096)+8192) — match=<0|1> | poison says touched through byte 0x… (<n> B)
        | delta=<n> — <verdict> ::
:: V3D: [v3d56] landing (<tag>) — arena <n> pages (0x40000 B @ 0x………, the ENTIRE address space the
        V3D MMU grants this job) | changed=<n> expected=<n> STRAY=<n> | per-region:
        tile-state (bin output) p[<a>..<b>]=<n> · tile-alloc pool (bin output) p[<a>..<b>]=<n>
        · probe TMU scratch (expected) p[<a>..<b>]=<n> · PTB overspill (expected) p[<a>..<b>]=<n>
        | first_stray_page=<i> (off 0x…) last_stray_page=<i> — <verdict> ::
:: V3D: [v3d56] int (<what>) — ENTRY INT_STS=0x……… INT_MSK_STS=0x……… | EXIT INT_STS=0x………
        INT_MSK_STS=0x……… (1=MASKED) | FLDONE(bit1): latched=<0|1> masked=<0|1>
        | working-set V3D_IRQS=0x……… unmasked_now=0x……… | latched-but-masked=0x……… — <verdict> ::
```

`armed` fires once per probe; `poison` twice (tile-state, tile-alloc); `bpca-vs-bytes` and `landing` once each.

`int` is gated on `V3D56_INT_TRIPLE` (default `true`) and fires on **every** `wait_fldone` exit, not only the
probe's — one line per bin kick, deliberately. Every caller of `wait_fldone` *is* a bin-retire wait (probe, the
§22 bisect rungs, the §28 resubmit, M4); the arc's remaining question is why `FLDONE` never asserts, and the
bisect rungs are exactly where a rung-to-rung difference in the mask or the latch would show. Scoping it to the
probe would blind the comparison that matters most. The enclosing V3D battery is itself default-quiet
(`BLOCK-DOWN` short-circuit), so none of it reaches a non-metal log.

### What P56 metal decides

The **register-semantics** half is closed by source and needs no boot. Everything else here is a prediction
awaiting its first `[v3d56]` line. Metal decides:

- **The poison** — the discriminator the `0x3000` advance cannot be:
  - **fully INTACT** → the PTB wrote nothing at all, reservation-only; the retraction's conjunction holds and
    the arc's whole remaining surface is empty-frame `FLDONE` generation.
  - **ZEROED** → the PTB *did* write, with zeros, and the "nothing was written" premise under V3D-40..55 was
    false for the entire series.
  - **OVERWRITTEN** → real primitive output exists and the defect was always frame-close.
- **Whether `bpca-vs-bytes` reports `match=1`** on this frame, and — the open measurement noted above — whether
  the §22 bisect *Empty* rung yields the same `0x3000` as the effectively-empty full draw list.

Any **STRAY** page on `[v3d56] landing` overrides all of it and reopens the address question with an offset
attached. Note that `expected` hits are *not* failures: a probe-scratch or overspill hit is what success looks
like.

QEMU `raspi4b` models no V3D block — the run returns at `BLOCK-DOWN` before the probe, so **no** `[v3d56]` line
appears there, and `kernel8-test` green means *no regression*, not probe verification.

## 31. The bin CL audited against v3d_packet.xml — the encoding is exonerated (V3D-57)

P55b left the verdict "the flush/frame-close unit is clocked but never triggered", with one diagnostic rider:
the PTB writes **nothing**, not even the initial tile-state, which would also be the signature of a binner that
never truly starts. V3D-57 tests that rider the only way it can be tested off-metal — by checking our binning
control list against Mesa's v42 packet definitions **field by field**, rather than against a comment claiming
they were checked.

Two distinct things are reported below, and they must not be confused:

- **the off-metal AUDIT** — an independent check: Mesa's XML was fetched and parsed, and our packets checked
  against it. This is what exonerates the encoding.
- **the on-metal `[v3d57]` WITNESS** — a **packing-consistency** check: its `mesa=` column is written from the
  same audited constants the emitter uses, so a matching line proves the bytes *in arena memory* carry the
  intended value at the audited offset. It is not a second opinion on Mesa, and its `0 diverged` is not
  independent evidence about Mesa's encoding.

The audit was run mechanically, not by eye: Mesa's own `src/broadcom/cle/v3d_packet.xml` (the file
`gen_pack_header.py` generates every `V3DXX_*_pack` from) was parsed, every packet's v42-applicable variant
reduced to `(code, byte length, {(field start, width)})` using the generator's own length rule
(`max(field.end)//8 + 1`, field starts shifted by the 8-bit opcode), and **every `Pkt::new(...)` chain in
`v3d.rs`** — binning and render side, all three builders — checked against it.

### Audit result: 0 divergences in the control list

Verified against `v3d_packet.xml` v42 semantics: opcode, byte length, field `(start, width)` and encoding
**form** (which fields are `minus_one`, which are enums). The load-bearing rows — code 120 is 9 bytes,
width/height are pixels-MINUS-ONE at bits 32/48 (the v41+ form, not the pre-4.1 tile-count form), block enums
1/128 B and 0/64 B — were each confirmed at the XML directly.

| Packet (code) | Our length | XML length | Fields checked | Verdict |
| --- | --- | --- | --- | --- |
| `NUMBER_OF_LAYERS` (119) | 2 | 2 | layers@0 w8, **minus_one** → 1 layer packs as 0 | exact |
| `TILE_BINNING_MODE_CFG` (120, `max_ver=42`) | 9 | 9 | init block@2 w2 =1(128 B) · overflow block@4 w2 =0(64 B) · RTs@8 w4 minus_one =0 · max BPP@12 w2 =0(32) · MSAA 4x@14 w1 =0 · double-buffer@15 w1 =0 · width@32 w16 minus_one =63 · height@48 w16 minus_one =63 | exact, 8/8 |
| `FLUSH_VCD_CACHE` (19) | 1 | 1 | — | exact |
| `OCCLUSION_QUERY_COUNTER` (92) | 5 | 5 | address@0 w32 = 0 (Mesa's OQ-disable) | exact |
| `START_TILE_BINNING` (6) | 1 | 1 | — | exact |
| `CFG_BITS` (96) · `CLIP_WINDOW` (107) · `VIEWPORT_OFFSET` (108) · `CLIPPER_XY_SCALING` (110) · `CLIPPER_Z_SCALE_AND_OFFSET` (111) · `VCM_CACHE_SIZE` (71) · `GL_SHADER_STATE` (64) · `VERTEX_ARRAY_PRIMS` (36) | 4/9/9/9/9/2/5/10 | same | all field starts+widths | exact |
| `FLUSH` (4) | 1 | 1 | — | exact |

The block-size enums are confirmed at their source rather than inferred: `v3d_limits.h` defines
`V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE 128` / `..._OVERFLOW_BLOCK_SIZE 64` with `ENUM = size >> 7`, i.e. **1** and
**0** — exactly what we emit.

**Ordering** is also Mesa-exact. `v3dX(start_binning)` (gallium `v3dx_draw.c`) and
`v3dX(job_emit_binning_prolog)` (v3dv `v3dvx_cmd_buffer.c`) both emit
`NUMBER_OF_LAYERS → TILE_BINNING_MODE_CFG → FLUSH_VCD_CACHE → [OCCLUSION_QUERY_COUNTER, gallium only] →
START_TILE_BINNING`, which is our prologue verbatim (the `Empty` rung's omission of the OQ-disable is the
*Vulkan* prologue, also legal).

**Terminator** settled: Mesa ends every bin CL with a bare `FLUSH` (code 4) — `v3dX(bcl_epilogue)`
(`v3dx_job.c`), whose own comment rules out the alternative: *"you would need FLUSH_ALL for that, but the HW
hasn't been validated"*. `FLUSH_ALL_STATE` (5) is **not** used, and `INCREMENT_SEMAPHORE` (7) is a VC4-era idiom
no v3d 4.x emitter touches. Our `P_FLUSH = 4` terminator is correct and no semaphore is owed.

So the "the binner never starts because the CL is malformed" branch of the P55b rider is **closed**: opcode,
length, ordering and encoding form all match what Mesa emits for the same frame.

### Two real divergences found — both outside the CL, both fixed

The audit did not come back empty. Both findings are in what the **registers** hand the PTB, which is where v42
moved this state: `v3d_job.c` — *"On V3D 4.1, the tile alloc/state setup moved to register writes instead of
binner packets."*

1. **The tile-state data array was under-sized by 25%.** `TILE_STATE_BYTES` was `48 * 4 = 192 B`, from the
   per-tile TSDA *record* size. Mesa sizes the CT0QTS buffer with `v3d_tile_alloc_sizes`
   (`src/broadcom/common/v3d_util.c`), whose closing line is `*tile_state_size = layers * tiles_x * tiles_y *
   256`. For our 64×64 target — 1 RT, 32 bpp, no MSAA, hence Mesa's largest 64×64 tile, one tile — the correct
   array is **256 B**, not 192. Corrected, with the tile count and the per-tile 256 named as constants and a
   compile-time assert that the array still fits its dedicated page (`OFF_TILESTATE` 0x11000, next region
   0x12000). The tile-**allocation** pool was checked against the same function and is fine: Mesa's minimum for
   this frame is `align(1·128, 4096) + 8192 = 12 KiB` and we hand the PTB 32 KiB — now asserted at compile time.
2. **`BPOS = 0` was written in the wrong place.** V3D-49 correctly established the *value* (clear the overflow
   allocation at frame start; never pre-arm it — the kernel arms overflow lazily on `OUTOMEM` in
   `v3d_overflow_mem_work`). But `v3d_bin_job_run` (`drivers/gpu/drm/v3d/v3d_sched.c`) issues that write as its
   **first** action — under the queue lock, *before* `v3d_invalidate_caches` and *before* the
   CT0QMA/QMS/QTS/QBA/QEA latch — precisely so the PTB enters the frame with no carried-over overflow
   descriptor. Every kick in this driver wrote it *after* CT0QBA, so the PTB was handed its pool with a stale
   overflow descriptor still live. Moved to the kernel's position at all five CT0 kicks (probe, bisect rung,
   §28 resubmit, M4, battery) via `bin_prejob_bpos_clear`.

Neither is proven to be the wedge; both are genuine kernel/Mesa divergences removed on their own merits, and
both were invisible to every prior audit because they live in the register path, not the packet path.

### Witness format

```
:: V3D: [v3d57] <tag> bin CL — packing check vs the audited v42 encoding (<n> bytes @ arena+0x…);
        read back from the PUBLISHED bytes, expected column = this driver's audited constants
        (audit authority: v3d_packet.xml v42 + v3dX(start_binning)/bcl_epilogue) ::
::   [v3d57] [ i] +0x…… op=<n> <PACKET_NAME> len=<n> bytes=xx xx xx xx xx xx xx xx xx xx ::
::     [v3d57]   <field name>                    ours=<v>       mesa=<v>       OK|DIVERGE ::
        (ours = read back from arena memory; mesa = the audited expected encoding)
::     [v3d57]   GL_SHADER_STATE (frame data)    record=0x……… attrs=<n> 32B-aligned=<0|1> ::
:: V3D: [v3d57] <tag> verdict — packets=<n> packing-diverged=<n> prologue-order=OK|DIVERGE
        (CFG->[VCD]->[OQ]->START, terminator FLUSH/4 not FLUSH_ALL_STATE/5, no INCREMENT_SEMAPHORE)
        | tile-STATE bytes ours=<n> mesa=<n> (<n> tile x 256, v3d_tile_alloc_sizes) OK|DIVERGE
        | tile-ALLOC bytes ours=<n> mesa-min=<n> (align(tiles*128,4096)+8192) OK|DIVERGE ::
:: V3D: [v3d57] <what> pre-job BPOS=0 (kernel-exact ORDER: first write of v3d_bin_job_run, before
        cache-invalidate and before the CT0QMA/QMS/QTS latch) — BPOA=0x……… BPOS=0x……… ::
```

Read the columns honestly. `ours` is read back out of the **published arena bytes** — the bytes the CLE will
actually fetch — while `mesa` is the audited expected encoding written from this driver's own constants. A
matching line therefore proves *packing consistency* (builder intent survived into memory at the audited
offset), and a `DIVERGE` line says the published bytes are not what our audited packing says they should be.
It is deliberately **not** an independent re-derivation of Mesa: that check is the off-metal audit above, and
`packing-diverged=0` is not extra evidence about Mesa's encoding. The one genuinely external comparison on the
verdict line is the tile-**memory** sizing, which is compared against the literal `v3d_tile_alloc_sizes`
formula (`tiles · 256`) rather than against the constant derived from it.

Gated on `V3D57_CL_AUDIT`, a plain `const` (default `true`); it fires for the PROBE, the bisect rungs and M4 —
the three lists whose byte-equality is the standing claim. Volume is **~60–100 lines per boot**, one shot per
CL: deliberate, within the `V3D-46`/`-54`/`-56` one-shot-witness precedent, and **one flip to `false`** silences
the whole battery once the metal capture has been taken. The per-frame battery kick issues the reordered `BPOS`
write quietly (quiet-boot law).

### What P57 metal decides

The encoding half is closed by source and needs no boot; what metal adds is confirmation that the bytes *in
memory* are the bytes we think we built (the witness reads them back out of the arena, not out of the builder),
and whether the corrected 256 B tile-state array plus the kernel-ordered `BPOS` clear changes the `[v3d56]`
poison verdict. If the poison still comes back **fully INTACT** with the sizing and ordering now kernel-exact,
the CL and its register environment are jointly exonerated and the remaining surface is `FLDONE` generation
itself — with no encoding candidate left to blame.

QEMU `raspi4b` models no V3D block — the run short-circuits at `BLOCK-DOWN` before any of this, so no
`[v3d57]` line appears there and `kernel8-test` green means *no regression*, nothing more.

## 32. Two refutations already on the wire, and the window nobody sampled (V3D-58)

P56 metal ran the V3D-57 audit and returned `packets=14 packing-diverged=0 prologue-order=OK`,
`tile-STATE bytes ours=256 mesa=256 OK`, `tile-ALLOC ours=32768 mesa-min=12288 OK`, and the reordered
`pre-job BPOS=0 (kernel-exact ORDER)`. With a byte-exact-to-Mesa control list, a Mesa-sized tile-state
array and kernel-ordered register writes, the poison still came back **fully INTACT** — tile-state 64/64
words, pool 8192/8192 words — and `[v3d56] landing` reported **zero** changed pages across the whole
64-page arena. `FLDONE` is unmasked and never latches.

The encoding branch is closed. What V3D-58 found is that **two large families of hypothesis were already
refuted by readings sitting unremarked in the same capture**, and that one window in the kick sequence
has never been sampled at all.

### R1 — the V3D store path is not dead. The render engine proves it.

The boot that wedges the bin prints, forty lines earlier:

```
:: V3D: M3 clear-job PASS (GPU cleared buffer; CPU byte-verified) ::
:: V3D: M3 clue — CT1CS pre=0x00000000 kicked=0x00000020 done=0x00000000
        CT1CA pre=0x00000000 kicked=0x00215000 done=0x0021506a — RAN-OK ::
```

and `[v3d41]` reports `RFC 0x00000001`. The render CLE (CT1) executed an RCL, its
`STORE_TILE_BUFFER_GENERAL` landed in arena memory, the CPU byte-verified it against a `0xDEADBEEF`
sentinel, and the render frame counter advanced.

That store went through the **same** V3D MMU (same `MMU_CTL=0x00090801`, same `PT_PA_BASE`, same
identity-mapped arena), the **same** L2T/slice cache configuration, the **same** GMP (`PROT_ENABLE=0`),
the **same** AXI fabric and the **same** firmware-granted 500 MHz clock as the bin that writes nothing.

So every hypothesis that blocks V3D writes *globally* is refuted **by a working engine rather than by
argument**: MMU write-permission, a GMP silent drop, L2T/slice flush ordering, a dead or half-clocked
write domain, an AXI/QoS floor. None of them can be true of a block that just byte-verified a GPU store.
Whatever is wrong is **bin-path-exclusive**.

This retires the brief's suspect (b)/(c)/(e) surface — hub/supply enables, L2T ordering around the kick,
and GMP-blocked stores — without a single new register read.

### R2 — "the PTB never starts" is refuted *as stated*; `BPCS` is the witness

`[v3d55] pool` reports `BPCS=0x00005000` against a pool of `0x8000` bytes, exactly complementing the
`0x3000` `BPCA` advance.

The reading rests on `BPCS` being the PTB's **remaining-bytes** register. That is *inferred*, not
established: §30 takes it from the VideoCore IV ARG's "Remaining Size Of Binning Memory Pool" for the
VC4-era register at the same offset, carried forward to v42 on the `BPOA`/`BPOS` analogy, and mainline
`v3d_regs.h` is comment-free on the point. **On that reading** a pool the PTB never entered would still
read the `0x8000` we latched into `CT0QMS`, so the PTB *did* act — per-tile initial-block reservation
plus the two up-front 4 KiB chunk allocations `v3d_tile_alloc_sizes` describes.

The inference is exactly as strong as the register-semantics assumption under it, which is why R2 gets a
falsifying probe (RANK 1's drop-station test) rather than a verdict — and why the matrix below carries an
arm in which **R2 is wrong**.

The surviving statement is narrower than either horn of the arc brief's fork:

> **The bin frame OPENS — `PCS.BMACTIVE=1`, pool reserved — and never CLOSES: `FLDONE` dead, `BFC` Δ0,
> `BMACTIVE` stuck set — while a render frame on the same block opens, writes and retires cleanly.**

### The unsampled window — `[v3d58] station`

`PCS`, `BPCA` and `BPCS` have only ever been read in the post-wait wedge dump, so **when** `BMACTIVE`
sets and **when** the pool reservation happens have always been inferred, never measured. R2 above is an
inference of exactly that kind, and it is falsifiable.

Five stations now bracket the CT0 kick, each sampling `PCS`/`BPCA`/`BPCS`/`BFC`/`CT0CS`/`CT0CA`:

| Station | Point in the kick sequence |
| --- | --- |
| **S0** | before this driver touches **any** CT0 or PTB register for the job |
| **S1** | after the `CT0QMA`/`QMS`/`QTS` latch, before `CT0QBA` |
| **S2** | after `CT0QBA`, before the GO |
| **S3** | the instant after the `CT0QEA` GO write |
| **S4** | at `wait_fldone` exit, before any cache maintenance |

Two reading rules the verdict logic enforces, because both are easy to get wrong:

* **S0's `BPCS` is excluded from the drop scan.** Nothing has been latched into `CT0QMS` at S0, so the
  value there is the block's reset/stale content and is not comparable to the pool size. Counting it
  would let any small leftover score as a drop and fire the "latch artifact" verdict for a write that had
  not happened. S0's `BPCS` is printed with an explicit *carries no verdict* tag instead.
* **A drop to exactly zero counts.** `BPCS→0` is *full* pool consumption — the strongest possible "the
  PTB acted" reading — so the test is "below the latched size, zero included", and a zero drop is called
  out on the line.

S0 is the load-bearing station. The probe is the **first CT0 bin kick of the boot** (M3 kicks CT1 only), so
`BMACTIVE` reading `1` at S0 means the block left the reset cycle — or the preceding CT1 render frame —
with a **bin frame still open**, and every `START_TILE_BINNING` since has been stacking onto a frame
nobody closed. That single reading would explain the entire campaign: the list walks, the pool reserves,
and the `FLUSH` never closes a frame it did not open.

### The probe matrix — what each outcome proves

| Witness | Reading | Verdict |
| --- | --- | --- |
| `[v3d58] station` | `BMACTIVE=1` at **S0** | A bin frame was left open by the reset path or by M3's render frame. The defect is **upstream of everything V3D-40..57 audited** — a bring-up/frame-abort gap, not a packet, register-order or sizing gap. |
| `[v3d58] station` | `BMACTIVE=0` at S0 **and** at S4 | The frame never opened. `START_TILE_BINNING` did not put the pipeline into binning mode, so the missing `FLDONE` is a **consequence, not the defect** — chase what gates `BMACTIVE`. |
| `[v3d58] station` | `BMACTIVE` sets at S3/S4, `BPCS` first drops at S3/S4 | `START_TILE_BINNING` genuinely executed; **R2 stands**. The remaining surface is the bin FLUSH/frame-close step alone. |
| `[v3d58] station` | `BPCS` dropped below the latched size at **S1** | The `0x3000` advance is a **register-latch artifact** of the `CT0QMA`/`QMS`/`QTS` write — it happened before `CT0QBA` and before the GO. **R2 is wrong** and "the binner never started" returns to the table. |
| `[v3d58] station` | `BPCS` dropped at **S2** (post-`QBA`, pre-GO) | Neither a latch artifact nor `START_TILE_BINNING`: the queue-begin write is moving PTB state, which no model of this block predicts. A new fact — re-read the raw stations. |
| `[v3d58] xengine` | render verified + bin neither retired nor wrote | **Asymmetry confirmed** — the global-write-failure family is refuted by demonstration. Bin-path-exclusive. |
| `[v3d58] xengine` | render did **not** verify | **Not** the bin-exclusive asymmetry. A block on which even the proven-good clear job fails is broken upstream; fix M3 before reading any bin verdict. |
| `[v3d58] rerender` | post-bin clear job **passes** | The wedge is **confined to CT0/PTB**. Every bin readback in this file — `[v3d54]` submit/trace, `[v3d55]` tilestate/pool/mmucfg, `[v3d56]` poison/landing/int, `[v3d41]`, `[v3d34]` and `[v3d28]`, all of which now complete *before* this control runs — was taken on a sound block, so their "the PTB wrote nothing" readings stand. |
| `[v3d58] rerender` | post-bin clear job **fails** | CT1 has broken since the M3 baseline, and the line does **not** choose between two readings: **(a)** the bin wedge has a **blast radius** beyond CT0, leaving shared state (CLE, pipeline, L2T, MMU) broken — in which case every readback in the row above was taken inside it; or **(b)** something *other* than the bin frame between M3 and here broke it — `bin_prejob_bpos_clear`, `pctr_setup_cs_witness` (which re-arms the PCTR block), the `[v3d55]` clock audit and its two mailbox round-trips, the L2T/SLC flushes, or the whole-arena cache invalidate. Bisect by moving the control earlier before blaming the bin. |

`[v3d58] rerender` is the negative control the campaign never ran, and it costs one re-run of an
already-verified job. It is called with `clear_job(None)` so the panel is not repainted.

**Placement is load-bearing.** It runs a real CT1 job, so it is called at the very **end** of
`probe_job` — after `[v3d54]`, `[v3d55]`, `[v3d56]`, `[v3d41]`, `[v3d34]` *and* `[v3d28]`. Anywhere
earlier it would corrupt the post-mortem it exists to validate: `ptb_frame_witness` diffs `RFC` against a
snapshot latched **before** the CT0 GO, so a retired control frame would print Δ1 on a boot where the bin
did nothing; the MMU fault latch feeding `[v3d28]` would attribute a fault from the control job's store
to the bin; and the V3D-28 post-bin L2T drain would have a whole CT1 job's cache traffic between bin idle
and the flush. The *memory* regions are disjoint (M3's buffers at `0x0`/`0x8000`/`0x9000` versus the pool
at `0x12000`, tile-state at `0x11000`, probe scratch at `0x34C00`, with the `[v3d56]` arena digest taken
earlier still), so ordering alone is sufficient — the contamination was register and cache state only.

### What is deliberately NOT armed

A CT0 thread/frame reset (`CTRSTA`) is the obvious follow-on if S0 reads `BMACTIVE=1`. It is **not**
implemented. Mainline `v3d_regs.h` defines **no** bit fields for `V3D_CLE_CT0CS` (§30), so any `CTnCS`
bit beyond the `CTRUN` this driver already relies on would be a fabricated constant — the exact class of
bug PI-V3D-4 and PI-V3D-6 were. V3D-58 *collects the evidence* that would justify the write; issuing it
is a next-arc decision taken against a corroborated bit position.

### Witness formats

```
:: V3D: [v3d58] xengine (<tag>) — RENDER(CT1) ran=<0|1> verified-store=<0|1> RFC=0x………
        | BIN(CT0) retired=<0|1> wrote-any-arena-byte=<0|1> BFC=0x………
        | SHARED at bin time: MMU_CTL=0x……… (ENABLE=<0|1> faults=0x…) PT_PA_BASE=0x………
        L2TCACTL=0x……… arena=0x…+0x… — <verdict> ::
::   [v3d58] S<n> <station> — PCS=0x……… (BMACTIVE=<0|1> BMBUSY=<0|1> RMACTIVE=<0|1> RMBUSY=<0|1>
        BMOOM=<0|1>) BPCA=0x……… BPCS=0x……… BFC=0x……… CT0CS=0x……… CT0CA=0x……… ::
:: V3D: [v3d58] station (<tag>) — pool base=0x……… size=0x… (as latched into CT0QMA/CT0QMS)
        | BMACTIVE S0..S4 = <00000..11111>
        | BPCS S0=0x… (PRE-LATCH: reset/stale, below-size=<0|1> carries NO verdict) S1=0x… -> S4=0x…
        | first-drop-station=<n|-1> (S1..S4 only[, dropped to ZERO = FULL pool consumption])
        | BPCA advance at S4 = 0x… | BFC 0x………->0x……… (Δ<n>) — <verdict> ::
:: V3D: [v3d58] rerender (<tag>) — M3 clear job re-run AFTER the wedged bin: pre-bin=<0|1>
        post-bin=<0|1> (CT1, panel blit suppressed) — <verdict> ::
```

`station` emits five indented sample lines plus one verdict line; `xengine` and `rerender` are one line
each. Gated on `V3D58_STATIONS` and `V3D58_RERENDER_CONTROL` (both plain `const`, default `true`).

### A latent build break fixed in passing

V3D-55 widened `mailbox::get_clock_state` to compile for the `v3d` feature as well as `piusb`, but left
its `TAG_GET_CLOCK_STATE` constant `piusb`-gated. A `v3d`-without-`piusb` build therefore failed to
compile at `E0425` — the widening was a no-op that only broke the build it was meant to enable. The two
`cfg`s now agree. This was invisible to `./arroyo check`, which does not enable the `v3d` feature at all:
**the V3D probe battery is only type-checked by an armed `UNAOS_V3D=1 ./arroyo kernel8` build**, and that
build is now part of this arc's gate.

QEMU `raspi4b` models no V3D block — the run short-circuits at `BLOCK-DOWN` before any of this, so no
`[v3d58]` line appears there and `kernel8-test` green means *no regression*, nothing more. **P58 metal
reads the station progression, the cross-engine asymmetry and the post-bin render control.**

## 33. The mainline audit that closed four theories, and the register nobody decoded (V3D-59)

P57 metal returned the five-station progression V3D-58 armed, and it produced a paradox rather than a
suspect:

- `BMACTIVE` S0..S4 = `00001` — the bin frame **opens**, and only after the GO. No frame was left open
  by the reset path or by M3's CT1 render job.
- `BPCS` dropped below the latched pool size **already at S1** — after the `CT0QMA`/`QMS`/`QTS` write,
  before `CT0QBA`, before the GO. Per §32's own matrix that makes the `0x3000` advance a
  **register-latch artifact**: §30's R2 falls and V3D-56's "the PTB reserved the pool" is **retracted**.
- `CT0CA` walks `BA→EA`, `CT0CS` reads `0` after, `BFC` Δ0, `FLDONE` never latches, and the poison in
  both the tile-state array and the 32 KiB pool is fully **intact** — no PTB memory write ever happens.
- `[v3d58] xengine` and `[v3d58] rerender` both came back clean: a CT1 render frame on the same block
  byte-verifies a store before the bin **and again after it**. The block is sound; the wedge is
  bin-path-exclusive and has no blast radius.

So `START_TILE_BINNING` opens a frame and initialises no tile state, and `FLUSH` closes nothing.

**Reconciling §32's S1 matrix row.** That row reads: *"`BPCS` dropped below the latched size at S1 → the
`0x3000` advance is a register-latch artifact … **R2 is wrong** and 'the binner never started' returns to
the table."* P57 fired that row, so the first half holds — R2 is wrong and §30's reservation reading is
retracted. **The second half is answered and closed by the same capture, and does not return to the
table**: `BMACTIVE` S0..S4 = `00001` shows a frame that was closed at S0 and *open* at S4, so the binner
demonstrably **did** start. The two halves of that row were written as a pair because in V3D-58 no
station data existed; with the data in hand they separate cleanly. What never happened is not the
*start* — it is the **pool write**. Sections 32 and 33 must be read with that correction: there are not
two live opposed verdicts here, there is one, and it is "started, wrote nothing, never closed".

### The mainline audit — four theories closed by citation

Sources read for this arc, facts-only: Linux `drivers/gpu/drm/v3d/v3d_sched.c`, `v3d_regs.h` and
`drivers/gpu/drm/vc4/vc4_regs.h` (GPL-2.0-only); Mesa `src/gallium/drivers/v3d/v3dx_draw.c`,
`v3dx_job.c`, `v3d_job.c` and `src/broadcom/cle/v3d_packet.xml` (MIT).

| # | Theory | Verdict | Citation |
|---|---|---|---|
| **T1** | A zero overflow pool makes the PTB refuse to start writing (the brief's HIGH rank) | **REFUTED** | `v3d_bin_job_run` writes `V3D_PTB_BPOS = 0` as its **first** register write of **every** bin job, under the queue lock; its comment gives the reason as clearing the overflow allocation so a previous job's overflow block is not carried in (paraphrased — see the licence note below). `BPOA`/`BPOS` are written nowhere else except `v3d_overflow_mem_work`, which runs only in **response** to `V3D_INT_OUTOMEM`. Every Mesa frame on every Pi 4 enters binning with no overflow block. |
| **T2** | The per-frame `QMA`/`QMS`/`QTS` latch resets an open frame; they should be written once | **REFUTED as a divergence** | `v3d_bin_job_run` writes `CT0QMA`+`CT0QMS` (guarded by `job->qma`), then `CT0QTS \| V3D_CLE_CT0QTS_ENABLE` (guarded by `job->qts`), then `CT0QBA`, then `CT0QEA` — per-frame, our exact order. The S1 artifact is explained and benign: latching `CT0QMS` reloads the PTB's remaining-size register, so `BPCS` tracking the write is the register doing its job. |
| **T3** | `CT0QTS` is a tile **count**, not the tile-state **address** | **REFUTED** | Mesa `v3d_job.c`: `job->submit.qts = job->tile_state->offset`, beside `qma = tile_alloc->offset` and `qms = tile_alloc->size`, under *"On V3D 4.1, the tile alloc/state setup moved to register writes instead of binner packets."* |
| **T4** | The bin CL is missing a terminator or semaphore packet | **REFUTED** | `v3dX(bcl_epilogue)` (v3dx_job.c) emits exactly one packet for a job without transform feedback — `FLUSH` — commented *"you would need FLUSH_ALL for that, but the HW for hasn't been validated"*. No semaphore packet is emitted on the modern V3D path at all. `v3dX(start_binning)` is `[NUMBER_OF_LAYERS] → TILE_BINNING_MODE_CFG → FLUSH_VCD_CACHE → OCCLUSION_QUERY_COUNTER → START_TILE_BINNING` — our prologue verbatim. V3D-52's auto-init audit re-confirms against the real XML: `v3d_packet.xml` code 120 `max_ver="42"` carries eight fields and **no** auto-initialise-tile-state bit. |

The register protocol and the packet stream are now **both mainline-exact, end to end**. That is the
arc's substantive result: every remaining explanation has to live somewhere neither of them describes.
Note the shape of what T2 buys: it removes the write **order** from the suspect list and explains the S1
artifact. It is "benign" as a *divergence* — there is no divergence — and that is not evidence the PTB is
healthy.

**Licence note.** Linux is GPL-2.0-only; UnaOS is GPL-3.0-or-later, and GPLv2-only text cannot ride in
this tree. Every kernel citation in this section and in the `[v3d59]` code is a **paraphrase of a fact**
(register offsets, write order, control flow), never reproduced comment text. Facts are not
copyrightable; the sentences that state them are. The Mesa quotations are reproduced verbatim because
Mesa is MIT-licensed and compatible. §22's `v3d_bin_job_run` quotation was corrected to a paraphrase
under the same rule.

### What has never been read — the CTnCS decode, and exactly how far it can be trusted

`CT0CS` has been printed as a raw hex word since PI-V3D-13 and decoded only for `CTRUN`. §32 declined to
go further, and declined to issue a CT0 thread reset, because mainline `v3d/v3d_regs.h` defines no
`V3D_CLE_CT0CS` bit fields and any constant beyond `CTRUN` would be fabricated — the PI-V3D-4/-6 bug
class. Linux `drivers/gpu/drm/vc4/vc4_regs.h` does publish a layout for the register at the same offset:

```
V3D_CT0CS 0x00100 / V3D_CTNCS(n)
  CTRSTA BIT(15) · CTSEMA BIT(12) · CTRTSD BIT(8) · CTRUN BIT(5) · CTSUBS BIT(4) · CTERR BIT(3) · CTMODE BIT(0)
```

**This does not void §32's objection, and V3D-59 does not claim it does.** An earlier draft of this
section justified the borrow as "the same header we already source `PCS` and `BPCA`/`BPCS` semantics
from". That is **false in both legs** and is corrected here:

- This driver's actual rule is **offsets from the headers, semantics from the ARG**. The `PCS` decode in
  `v3d.rs` says in terms that Linux `v3d_regs.h` treats `PCS` as *opaque*, and takes
  `BMACTIVE`/`BMBUSY`/`RMACTIVE`/`RMBUSY`/`BMOOM` from the Broadcom VideoCore IV 3D Architecture
  Reference Guide (VideoCoreIV-AG100-R).
- §30 did the same for `BPCA`/`BPCS`: the ARG supplied the field meaning, and `vc4_regs.h`/`v3d_regs.h`
  were cited only to establish **offset identity** at 0x300/0x304.

So the honest description of the `CTnCS` borrow is: **a VC4-era ARG-family map carried across on offset
identity alone.** That is the same *class* of inference as the `PCS` and `BPCA` decodes, but it is
**weaker in one specific way** — for those two the ARG and the headers agree, whereas `v3d.rs` line ~299
records the opposite finding for this register:

> only `CTRUN` (bit5) is corroborated across sources for V3D 4.x; the remaining bits differ from the
> VideoCore-IV layout

**That finding stands. V3D-59 does not refute it** and has nothing that would. What V3D-59 does is decode
the remaining bits under the VC4 map anyway, label every resulting reading an inference, use them for
diagnostic output only, and gate no behaviour on them. The `v3d.rs` comment at ~299 now carries a
back-reference saying exactly that, so the two blocks no longer read as contradicting each other.

A live corroboration crack found while re-checking the header for this arc, and the reason `CTSEMA` and
`CTRTSD` are handled specially: `vc4_regs.h` declares them as **single-bit** `BIT(12)`/`BIT(8)`, while the
ARG describes the semaphore and the return-to-sub-list depth as **multi-bit count fields** (bits 14:12
and 9:8). The two published sources disagree about field *width*. A boolean test would print `0` for a
semaphore count of 2 or a sub-list depth of 2 — a wrong decode manufacturing a reassuring log line — so
the code extracts and prints both as **raw masked windows with their values**, and this section promises
no "nesting depth", only raw bits.

**The falsifier for the whole borrowed map.** If `[v3d59] ctstate` reads bit 3 **set at S0** — before the
driver touches any CT0 register, on a block fresh out of a reset cycle whose CT1 render frame retires
cleanly this same boot — then the map is *indicted*, not the hardware: a control thread cannot be
errored-from-birth and healthy enough to complete a render frame. The verdict logic checks for exactly
that combination and says so, and in that case `CTRSTA` must **not** be armed, since bit 15 comes from
the same discredited map.

With those hedges stated, bit 3 is still the lead worth chasing: *if* it is `CTERR`, a CLE that latched an
error would walk to `EA`, drop `CTRUN`, emit nothing and leave the frame open — the exact signature. It
has been inside every `CT0CS` hex word this campaign printed, unnamed, for nineteen probes.

Three more registers have never been sampled at all. `V3D_CLE_CT0SYNC` (0x154) and `CT1SYNC` (0x158) are
the CLE per-thread semaphores that `CTSEMA` reflects; since modern Mesa emits no semaphore packets they
should sit at their reset value and never move across our frames. `V3D_PTB_BXCF` (0x310,
`CLIPDISA` bit 0 / `RWORDERDISA` bit 1) is the PTB's extra-config register — defined upstream, written by
no mainline path, and never read here.

### The `[v3d59]` battery

| Witness | What it does |
| --- | --- |
| `[v3d59] mainline` | The T1..T4 refutation ledger, emitted before the kick so the metal capture carries its own citations. |
| `[v3d59] ctstate` | The `CT0CS`/`PCS` decode at all five V3D-58 stations — `CTRUN` corroborated, everything past it marked `INFERRED` on the wire — plus `CT0SYNC`/`CT1SYNC`/`BXCF`/`BPOA`/`BPOS`/`CT0LC`/`CT0PC`. Pure reads. |
| `[v3d59] frameclose` | 64 further samples at 1 ms spacing **after** the FLDONE backstop gives up, watching `PCS`, `CT0CS`, `BFC`, `BPCA`, `BPCS`. Pure reads. |
| `[v3d59] arm-overflow` | **Disarmed.** Optional pre-armed `BPOA`/`BPOS` — the T1 refutation by demonstration rather than by citation. Prints its disarmed state too, so a capture always says which rung ran. |
| `[v3d59] ct0-reset` | **Disarmed.** Optional `CTRSTA` (bit 15) thread reset before the job. The constant is now corroborated; the write stays unjustified until `ctstate` reads `CTERR` set or a frame open at S0. |

Placement is deliberate: the ledger and the reset arm run **after** the S0 sample (so S0 stays a true
pre-program reading) and before `bin_prejob_bpos_clear`; the overflow arm sits **after** S1 (so the S1
latch-artifact reading is not contaminated by a `BPOS` write) and before `CT0QBA`; `ctstate` and
`frameclose` run immediately after the five-station verdict, before any cache maintenance.

### The probe matrix — what each outcome proves

| Witness | Reading | Verdict |
| --- | --- | --- |
| `[v3d59] ctstate` | bit 3 set **at S0**, on a fresh-reset block whose CT1 renders clean | **The borrowed map is indicted, not the hardware.** Bit 3 is not `CTERR` on 4.x; discard every inferred column and do **not** arm `V3D59_ARM_CT0_RESET` — `CTRSTA` comes from the same map. This is the falsifier, and it is checked before every verdict below. |
| `[v3d59] ctstate` | bit 3 (inferred `CTERR`) set at a station **other than** S0 | *On the borrowed map*, the CT0 control thread **faulted** — which would collapse the whole paradox to one undecoded bit. A **lead, not a verdict**: corroborate bit 3 independently before acting, then hunt what the CLE rejected at that station. |
| `[v3d59] ctstate` | inferred `CTSUBS` set at S4 | **RETRACTED (R23 boot7, 2026-08-02 — see §49.8).** Metal read bit 4 **set** at S3 and S4 with `CT0RA` reading **zero at all five stations**: the thread claims a sub-list and has no return address, so bit 4 is not `CTSUBS` on V3D 4.2 and this row cannot be read at all. The original text, kept for provenance: *on the borrowed map*, the thread believes it is still inside a sub-list at the end of a list that reached `EA`, and such a thread never reaches the top-level `FLUSH`'s completion semantics — audit every `BRANCH`/`RETURN` in the bin CL. **Do not act on that.** The falsification is one bit of a map borrowed on offset identity alone, so it indicts the borrow, not just the bit: `CTRSTA` (bit 15) comes from the same act and R6 is disarmed with it (§49.4). The `CTRTSD[9:8]` window was already only a candidate depth. |
| `[v3d59] ctstate` | `CT0SYNC`/`CT1SYNC` non-zero at S0, or moving across the kick | *Possibly* the block carries CLE **rendezvous state** into the bin. **Weak row, two ways:** the read side effects of these registers are unverified, so a read-to-clear or read-to-decrement semaphore would be moved **by this probe itself** (five stations = five reads each), manufacturing the `sema_moved=1` it reports; and their reset values are unknown, so non-zero at S0 is not by itself abnormal. Confirm with a single-read boot before concluding anything. |
| `[v3d59] ctstate` | `BXCF` non-zero | Something is configuring the PTB behind us on a block we reset ourselves. A bring-up fact, not a packet fact. |
| `[v3d59] ctstate` | Clean `CT0CS`, semaphores at rest, `BXCF` zero, **`CT0LC` and `CT0PC` both unmoved** | The CLE consumed the address range without its list-item or primitive counters registering anything — it is not executing the list it fetched. Next surface is CL **fetch/decode**, not the PTB. |
| `[v3d59] ctstate` | Clean `CT0CS`, semaphores at rest, `BXCF` zero, **counters moved** | Every CLE-side explanation is excluded by decode. The wall is inside the **PTB**, between item-accept and pool-write. |
| `[v3d59] frameclose` | `BMACTIVE` clears **and** `BFC` advances | The bin is **slow, not wedged**. Every "never retires" verdict in this file was measured with too short a backstop — re-run the campaign before reading any of them. |
| `[v3d59] frameclose` | `BMACTIVE` clears, `BFC` does **not** | An **aborted** frame, not a hung one: teardown runs, completion does not. Target the `BFC`/`FLDONE` latch. |
| `[v3d59] frameclose` | `BMOOM` latches during the extra window | The PTB **is** out of binning memory, later than any single-shot sample could see. Reopens the overflow question P43/P44 closed — flip `V3D59_ARM_OVERFLOW`. |
| `[v3d59] frameclose` | Some register moves, frame stays open | Not frozen; the binner is attempting progress with the frame open. Extend the window before calling it a hang. |
| `[v3d59] frameclose` | Nothing moves, `BMBUSY` **set** throughout | **Frozen mid-operation.** The block claims a binning op is in progress that has made no observable progress for the whole window: a hard stall, not an idle open frame — and not the CLE (`CT0CS` static) nor overflow (`BMOOM` clear). |
| `[v3d59] frameclose` | Nothing moves, `BMBUSY` **clear** at every sample | **Dead-open.** Not slow, not an overflow stall: opened by `START_TILE_BINNING`, never advanced, never closed, nothing in flight. With `[v3d58] rerender` clean, the target is the PTB frame unit alone. |
| `[v3d59] arm-overflow` | Armed, and the bin retires | This silicon needs an overflow block up front and mainline's `BPOS=0` is insufficient for us — a genuine divergence from the kernel, worth a doc of its own. |

### What is deliberately NOT armed

Both behavioural arms default off. `V3D59_ARM_OVERFLOW` would run a rung mainline provably does not run
(T1). `V3D59_ARM_CT0_RESET` is writable only with an **inferred** constant, not a corroborated one, so
§32's objection is softened rather than void — and it is not yet **justified** on any reading: P57 saw
neither a bit-3 latch nor a frame open at S0. This arc collects; the next one writes, if and only if
`ctstate` produces a reading that survives the falsifier above.

QEMU `raspi4b` models no V3D block — the run short-circuits at `BLOCK-DOWN`, so no `[v3d59]` line appears
there and `kernel8-test` green means *no regression*, nothing more. **P59 metal reads the `CTnCS` decode,
the three never-sampled registers and the post-wedge time series.**

---

## 34. The probe budget — `UNAOS_V3D_DEEP` gates the banked-verdict probes

An armed `UNAOS_V3D=1` boot visibly **stalled at the M3 square** on the panel for several seconds while
the diagnostic battery ran. The stall was not the GPU: it was the battery's own anti-hang backstops. On
metal the bin never retires, so every `wait_fldone` runs its full ~0.5 s budget to timeout, and the
`[v3d48]` ladder does that six times in a row.

The probes that cost the time are also the ones whose verdicts are already **banked** — re-running them
every boot buys no new information:

| Probe | Budget on metal | Banked verdict |
| --- | --- | --- |
| `[v3d48]` empty-frame bisection ladder | 6 rungs × ~0.5 s FLDONE backstop ≈ **3.0 s** (plus a full CL decode + Mesa diff per rung on the serial line) | every rung wedges, `Empty` included |
| `[v3d59] frameclose` | 64 × 1 ms ≈ **64 ms** + its verdict line | **DEAD-OPEN** — not one bit changed across the whole extra window |
| `[v3d58] rerender` | one extra full CT1 clear job | clean — the render engine still works after the wedge |

### The knob

| Knob | Feature | Effect |
| --- | --- | --- |
| `UNAOS_V3D=1` | `v3d` | The V3D bring-up + the **fast** probe battery: the `[v3d40]` probe kick and its single FLDONE wait, and all the pure-read decodes (`[v3d54]` submit/trace, `[v3d55]` clkliv/tilestate, `[v3d56]` poison/landing/int, `[v3d57]` Mesa diff, `[v3d58]` stations/xengine, `[v3d59]` mainline/ctstate). |
| `UNAOS_V3D_DEEP=1` | `v3d_deep` (implies `v3d`) | **Adds** the three banked-verdict probes above — ~3.5 s of extra boot. Arm it only when the bench is deliberately re-opening one of those questions. Since V3D75A-GATE and V3D80-GATE this knob is **no longer destructive**: the `SET_ENABLE_QPU` and `NOTIFY_DISPLAY_DONE` sends it used to reach are separately gated (below), so a deep boot leaves the mailbox — and therefore USB — alive and keeps its panel. |
| `UNAOS_V3D_QPU=1` | `v3d_qpu` (bare — an arming switch, inert without `v3d_deep`) | **Re-arms the two `SET_ENABLE_QPU` (tag 0x00030012) send sites** (`[v3d75a]`'s doorbell-waiting send and `[v3d81q]`'s reply-less one). This send **permanently wedges the VideoCore mailbox** on Pi 4 metal — every later `NOTIFY_XHCI_RESET` fails and USB is dead for the boot — and its diagnostic verdict is banked negative twice, so it is gated OFF by default rather than deleted. With deep armed and this off, each site prints an honest `SET_ENABLE_QPU SKIPPED` line and explicitly disclaims the verdicts the send would have fed. Arm only for a boot that deliberately sacrifices USB to re-open the QPU-enable question. |
| `UNAOS_V3D_DISPDONE=1` | `v3d_dispdone` (bare — an arming switch, inert without `v3d_deep`) | **Re-arms the two `NOTIFY_DISPLAY_DONE` (tag 0x00030066) send sites** (`[v3d80]`'s doorbell-waiting handover and `[v3d81d]`'s reply-less one). This send **stops the firmware display driver** — the panel goes black for the rest of the boot, by design — and its verdict is now **banked** (boot `pi4-r23s1x`/boot3, 2026-08-03: core+0x68 read `0x00010001` unchanged across the handover, pre == post against the working part's `0x00000003`), so the handover does not move the wedge signature and the send has nothing left to teach. Gated OFF by default rather than deleted: a deep boot keeps its panel. With deep armed and this off, `[v3d80]` prints one `NOTIFY_DISPLAY_DONE SKIPPED` line and returns without taking either station — a post-handover readback on a boot with no handover would be a false result, not a null one — and an armed `[v3d81d]` says it posted nothing. Arm only for a boot that deliberately sacrifices the panel to re-open the handover question; serial is unaffected and carries the whole verdict. |

Off (the default for an armed boot), the bring-up prints one line right after the `M1 probe PASS` gate:

```
:: V3D: [v3d] deep=off (banked verdicts skipped) — NOT run this boot: [v3d48] empty-frame bisection
   ladder (all 6 rungs banked non-retire), [v3d59] frameclose (banked DEAD-OPEN, zero bit changes),
   [v3d58] rerender (banked clean). Fast probes only; re-arm with UNAOS_V3D_DEEP=1 ::
```

with the `deep=on` counterpart naming the same three when armed. A shorter log is only trustworthy if it
says what is missing from it, so the line is printed unconditionally on a block that came up — never
silently. It sits **past** the presence gate, so QEMU `raspi4b` (which returns at `BLOCK-DOWN`) prints
neither variant and the default-quiet boot is unchanged.

`check` is blind to knob-gated code: the gating is verified by building `kernel8` **both** ways and
strings-proofing the images. In the armed-without-deep image the probe **bodies** are gone — the
`[v3d48]` ladder header, the `[v3d59] frameclose` verdict strings and the `[v3d58] rerender` strings all
drop out, 11,904 bytes (~12 KiB) smaller.

What *does* remain, by design, is a handful of **name-references** to those probes: the `deep=off`
honesty line itself (which has to name what it skipped to be honest) and the `[v3d59] ctstate`
fall-through verdict's cross-reference to `frameclose`. That cross-reference carries its own
DEEP-only caveat inline, so it points the reader at `UNAOS_V3D_DEEP=1` rather than at a line that
cannot appear. A `strings | grep frameclose` on the armed-without-deep image therefore returns hits;
the check that matters is that no probe *output* string survives.
---

## 35. The boot-state surface — warm handoff, the IDENT check and the init ledger (V3D-60)

P59 metal delivered the isolation the campaign had been converging on for ten arcs, and it left exactly
one wall standing:

- The bin control list **is sound and executes** — `CT0LC` `0x0`→`0x10000` and `CT0PC` `0x0`→`0x3` both
  MOVED. The control thread walked the list and, by its own accounting, fed items to the PTB.
- A CT1 **render** frame on the same memory block is byte-verified, and a re-render *after* the wedge
  still passes. Every global-write-blocker theory is refuted; the block is sound.
- The PTB frame unit is **dead-open**: the frame opens at S4, `BMACTIVE` sticks set, `BMBUSY` never
  sets, `[v3d59] frameclose` saw **zero** bit changes across its extended window, `BPCA` advances with
  no traffic anywhere the V3D MMU grants, `BFC` stays 0, no `CTERR`, no sub-list, semaphores at rest.
- `CTRSTA` stays **disarmed** — no `CTERR` reading ever justified it. V3D-56's `0x3000` "reservation" is
  retracted as a latch artifact.

The wall is **item-accept-without-pool-write, inside the PTB frame open/close unit alone**. Every CL-side
and per-job-register explanation is closed. What has never been examined is the state the block is in
*before* the first bin job.

### The warm-handoff hypothesis

UnaOS is a **cold-boot** driver. `bringup` powers the domain, sets the clock, and then power-cycles the
V3D (the V3D-50 OFF→ON `GRAFX_V3D` cycle) before reading a single register. Linux runs the same reset —
but it attaches to a block the VideoCore firmware has already been driving, whose own graphics stack has
run frames through this PTB. If any part of the frame unit is established by a **first frame** rather
than by register programming, a driver that only ever cold-starts would never get it, and no amount of
per-job byte-exactness would help.

`[v3d60] residue` tests this in one boot, read-only, in the only window where firmware state is still
observable: **after power/clock/gate, before our reset cycle**. It samples the whole bin-frame and
boot-state register set there, and again after the reset, and diffs them field by field. The second half
also answers a question no boot has asked — does our OFF→ON cycle actually **reach** the PTB frame unit,
or does it leave those registers exactly as it found them?

### The init ledger — two genuine gaps (both CLOSED by V3D-62)

`[v3d60] initdelta` walks the registers the mainline kernel driver programs before its first bin job and
prints ours beside the expectation (facts restated in our own words; no kernel comment text reproduced).
Most rows agree — the MMU table base, the L2T flush window (V3D-51), both interrupt working sets
(V3D-49/V3D-52), `MISCCFG` left alone on 4.2, GMP never written. **Two rows are real gaps:**

1. **`MMU_ILLEGAL_ADDR` points into our own arena.** Mainline allocates a *dedicated scratch page* for
   the illegal-address catcher — memory belonging to no job, mapped by nothing else. UnaOS aims it at
   **arena page 0**, inside the very address space the PTB writes, mapped VALID+WRITEABLE by our own
   page table. An illegal access then becomes indistinguishable, at the memory it lands on, from a legal
   one. Read-only this arc; the fix (a page outside the mapping) is a next-arc call. **Fixed in V3D-62.**
2. **`MMU_CTL` carries the abort halves of the fault policy only.** Mainline enables both the abort and
   the **interrupt** response for the page-table-invalid and write-violation conditions. The interrupt
   companions' bit positions are not in this file's audited constant set, so the row prints the raw word
   and **names** the gap rather than inventing two bit numbers (the standing no-fabricated-constants
   rule). A write the MMU swallows without reporting is precisely the failure class a fault-*reporting*
   policy exists to surface. **Fixed in V3D-62.**

Both gaps are now closed — see *The fault-reporting instrument (V3D-62)* below. The two ledger rows
survive as ordinary readback checks, and `[v3d60] initdelta`'s expected reading is now
`MEASURED divergences=0 STANDING gaps=0`.

### The probe matrix — what each `[v3d60]` line discriminates

| Witness | Reading | Verdict |
| --- | --- | --- |
| `[v3d60] residue (pre-reset)` | `BMACTIVE` set before we touch anything | A bin frame is **already open at cold boot**. Every `START_TILE_BINNING` we have ever issued has been stacking onto a frame that predates us — a bring-up-level defect and a direct candidate for the dead-open wall. |
| `[v3d60] residue (pre-reset)` | `BFC`/`RFC` non-zero | The **firmware has driven frames** through this block. The warm-handoff hypothesis is **live**: whatever a first firmware frame establishes, our power cycle destroys and we never re-establish. |
| `[v3d60] residue (pre-reset)` | MMU or CT0-queue state established, no frame counted | **Partial handoff** — the firmware configured the block without completing a bin frame. The hypothesis narrows from "first frame" to "configuration". |
| `[v3d60] residue (pre-reset)` | Everything at reset value | The block is **virgin at our entry**. The firmware never drove a bin frame through this PTB, so there is nothing warm to inherit and the warm-handoff hypothesis is **dead**. The PTB must be startable from cold by register programming alone. |
| `[v3d60] residue (post-reset)` | `BMACTIVE` still set after the cycle | Our OFF→ON power cycle does **not** close (or does not reach) an open PTB frame. |
| `[v3d60] residue (post-reset)` | Zero fields moved | The reset changed **nothing**. Cross-read the `[v3d50]` ASB/PM lines: bridges ACKed ⇒ the registers were already clean; bridges silent ⇒ this driver has never actually reset the V3D. |
| `[v3d60] ident` | Version (`TVER*10+REV`) ≠ `42` | **Campaign foundation invalid** — the CL packing, QPU word encoding and register map were all audited against V3D 4.2. Resolve before trusting any further PTB reading. (P59's mismatch was a *decode* artifact — see "The IDENT mismatch, resolved" below.) |
| `[v3d60] initdelta` | `MEASURED divergences=0`, `STANDING gaps=0` | Our pre-first-job register state matches every checkable row of the mainline ledger, including the two V3D-60 found open. No boot-state divergence remains to explain the dead-open frame. **This is the expected reading from V3D-62 onward.** |
| `[v3d60] initdelta` | `MEASURED>0` | A readback actually diverged this boot. If either MMU row is among them the fault-reporting instrument is not fully established, and `[v3d62] fault`'s exoneration branch must **not** be read as conclusive. |

The two counts are kept separate deliberately. `MEASURED` covers rows whose verdict comes from a register
readback and could go either way on any boot; `STANDING` covers rows that are a property of this build
and read identically every time (today: exactly one, the missing fault-interrupt policy). Folding the
standing row into the measured count would make "no divergence remains" unreachable and the row
uninformative.
| `[v3d60] syncrd` | Back-to-back reads differ | The CLE semaphore registers **self-modify on read**. `[v3d59] ctstate`'s `sema_moved` row is a probe artifact and is **retracted**; future decodes must sample them at most once per boot. |
| `[v3d60] syncrd` | Back-to-back reads identical | No read side effect. `[v3d59]`'s semaphore row stands as measured and its five-reads hedge can be dropped. |
| `[v3d60] gmpdelta` | A protection violation latches **during** the frame | **The silent-drop mechanism.** The protection block refuses the PTB's pool write; nothing lands, no MMU fault and no `CTERR` is raised — item-accept-without-pool-write, exactly. |
| `[v3d60] gmpdelta` | `CAP_EXCEEDED` newly set | The PTB issued an address **beyond the page-table cap** — capped, not translated. Reconciles "`BPCA` advances" with "no traffic anywhere the MMU grants". |
| `[v3d60] gmpdelta` | Page-table-invalid / write-violation newly set | The PTB's write was refused by **translation**; read `MMU_VIO_ADDR`/`VIO_ID` for the address and the issuing client. |
| `[v3d60] gmpdelta` | Clean across the frame | Neither the protection block nor the MMU refused anything during this frame. Both **exonerated**, and the PTB frame open/close unit stands alone as the wall. |

### The IDENT mismatch, resolved — the decode was wrong, the silicon is not (V3D-61)

P59 metal printed, exactly:

```
[v3d60] ident — HUB_IDENT1=0x000e1124 -> tech-version raw=0x00 (expects 0x42) cores=4 |
HUB_IDENT2=0x00000100 HUB_IDENT3=0x00000e00 |
CORE0 IDENT0=0x04443356 IDENT1=0x81001422 IDENT2=0x40078121
```

A real 4.2 mismatch would have invalidated the campaign's whole packet/register audit. It is **not** a
mismatch. The V3D-60 probe decoded the wrong field, in the wrong base, and its core-count field too.

**What the pre-V3D-61 code did.** `tver = (hub1 >> 24) & 0xFF` — the register's *top byte* — compared
against the literal `0x42`; and `ncores = hub1 & 0xF` — the *low* nibble.

**The correct map.** The mainline driver's `HUB_IDENT1` field set places the identity in the register's
low half-word, in four-bit fields — technology version at bits 3:0, revision at 7:4, core count at 11:8,
host count at 15:12 — with four feature-presence bits above (L3C 16, TFU 17, TSY 18, MSO 19). Crucially,
the driver's single version *number* is composed **decimally**: `tver * 10 + rev`. "V3D 4.2" is therefore
the number **42**, and never the hex byte `0x42`. The V3D-60 check compared a field it had not read
against a constant in the wrong base — two independent errors that happened to both point at "mismatch".

**The bench word decodes cleanly.** `HUB_IDENT1 = 0x000e1124`:

| Field | Bits | Value | Reading |
| --- | --- | --- | --- |
| TVER | 3:0 | `4` | major |
| REV | 7:4 | `2` | minor → version = 4*10+2 = **42** |
| NCORES | 11:8 | `1` | **one** core — not four. We drive core 0; there is no other. |
| NHOSTS | 15:12 | `1` | one host interface |
| L3C/TFU/TSY/MSO | 19:16 | `0xe` | L3C absent; TFU, TSY, MSO present |

Two independent witnesses corroborate, from a *different* register file (the core's, not the hub's):

- `CORE0 IDENT0 = 0x04443356`. Its low three bytes are the ASCII signature `V` `3` `D`
  (`0x56 0x33 0x44`) — the block identifying itself. Its top byte is `0x04`: the core's own **major**
  version, agreeing with the hub's TVER of 4.
- `CORE0 IDENT1 = 0x81001422`. Its low nibble is `2` — the core's revision, agreeing with the hub's REV
  of 2. The rest of that word (`0x81001422`) is a plausible per-core configuration and stays raw.
- `HUB_IDENT2 = 0x00000100` sets the hub's MMU-present bit (8) — consistent with a block whose MMU this
  driver programs and reads faults from.

The old decode's output was internally incoherent on its face — a technology version of `0x00` beside
four cores on a part that ships one — which is what a wrong field map produces and a wrong *part* does
not.

**Verdict: the "VERSION MISMATCH" is RETRACTED.** The bench silicon is V3D 4.2, single-core, MMU-present,
exactly the generation this file's CL packing, QPU word packing and register offsets were audited
against. The campaign's foundation **holds**; nothing downstream of it needs re-auditing.

V3D-61 fixes the decode at both sites that carried it (the `PRESENT` bring-up line and the `[v3d60]
ident` check), adds the field constants and a pure `v3d_ident_version` decoder, and makes the verdict
line state the version from the right field and report its corroboration explicitly. The probe stays
read-only; `CTRSTA` stays disarmed. On metal the corrected line will read version `4.2 (ver=42)`,
`cores=1`, signature `OK`, core-version-agrees `1`, and the CONFIRMED verdict.

`ncores` is printed **raw** — the pre-V3D-61 `.max(1)` clamp is dropped. Under the corrected map an
`NCORES` of 0 is a genuine anomaly, and laundering it into a plausible 1 would conceal precisely the
class of defect this arc exists to expose.

**Future-proofing note.** The `CONFIRMED` verdict requires **all three** witnesses to agree — the hub's
`TVER`/`REV`, the core's `'V3D'` signature, and the core's own version bytes. A future silicon revision
that bumped the core's `IDENT1` revision nibble *independently* of the hub's would therefore read
`UNSETTLED` on a perfectly healthy part. That is the right trade for a bench probe, where a hedged
reading costs an operator one line of log; it would be the wrong trade if this check ever **gated boot**.
If the verdict is ever promoted from a witness to a gate, split it: the hub version decides, and the
core witnesses annotate.

## The fault-reporting instrument (V3D-62)

V3D-60 measured two standing gaps against the state mainline hands its first bin job and named both as
*mechanisms by which a refused PTB write lands, or vanishes, unreported*. V3D-62 closes them and builds
the instrument that reads the resulting reports. The campaign's standing facts are unchanged: V3D 4.2,
`cores=1`, `RENDER`(CT1) writes fine, `BIN`(CT0) consumes its list but writes nothing.

### What changed in the programming

**The illegal-address catcher now aims at a dedicated scratch page.** `V3D_MMU_ILLEGAL_ADDR` takes a
*physical* page number, not an iova — the MMU steers a refused access to that page instead of to
translated memory. `V3D_MMU_SCRATCH` is a new page-aligned static outside the arena; `program_mmu` never
writes a PTE for it, so its page-table entry stays invalid and no job can reach it through translation.
That is the property the register wants, and it is what arena page 0 could never have: the landing site
of an illegal access is now distinguishable from every legal write.

**`MMU_CTL` now carries mainline's full fault policy.** The interrupt companions of the abort bits, plus
the address-cap pair, were the halves V3D-60 declined to guess. Read off the same audited register source
as every other constant in the driver (`v3d_regs.h`, register/hardware facts only — no GPL-2.0-only code
reproduced), they sit in a descending quartet per condition:

| Condition | status | `_ABORT` | `_INT` | `_ENABLE` |
|---|---|---|---|---|
| `CAP_EXCEEDED` | 27 | 26 | 25 | — |
| `PT_INVALID` | 20 | 19 | 18 | 16 |
| `WRITE_VIOLATION` | 12 | 11 | 10 | — |

Mainline's `v3d_mmu_set_page_table` writes `ENABLE | PT_INVALID_{ENABLE,ABORT,INT} |
WRITE_VIOLATION_{ABORT,INT} | CAP_EXCEEDED_{ABORT,INT} | TLB_CLEAR`. The `_EXCEPTION` halves are not in
its set and are not mirrored. UnaOS previously wrote `0x00090801` (ENABLE + the two aborts + the
pt-invalid enable); it now writes **`0x060d0c01`**. This changes fault *reporting* only — it grants the
GPU no address it could not already reach, so the arena-only confinement invariant is untouched.

### The three channels

`[v3d62] arm` runs pre-kick (after the stale-latch clear): it clears the hub interrupt vector and seeds
the scratch page with a sentinel, so everything read afterwards belongs to *this* frame.
`[v3d62] fault` runs at wait-exit, deliberately **before** the post-bin `clear_mmu_fault_latch` that would
erase the evidence, and reports:

| Channel | What it proves |
|---|---|
| `MMU_CTL` fault latches + `VIO_ADDR`/`VIO_ID`/`DEBUG_INFO` | The address a refusal was issued for and the client that issued it. Newly meaningful now the address-cap condition is armed at all. |
| `HUB_INT_STS` MMU bits (PTI/WRV/CAP) | The interrupt half, latched raw regardless of the mask. **This channel did not exist before this arc** — with the `_INT` bits unset the hub never latched. |
| The scratch page's sentinel | A sentinel that is **gone** is direct evidence that a refused access *landed* — the "write goes somewhere unaccounted" branch no register can show. Only sound now the catcher stops pointing into the arena, where legal traffic would have overwritten it. |

`[v3d62] mmufix` (at `program_mmu`) prints the scratch page's PA/PFN, proves its PTE reads zero, and shows
`MMU_CTL` and `ILLEGAL_ADDR` before and after. It self-invalidates: if the scratch page turns out mapped
or in-arena, or if `MMU_CTL` fails to echo the policy, the line says so and marks the frame witness
inconclusive rather than letting a silent instrument failure read as an exoneration.

### Reading the next boot

| Reading | Verdict |
|---|---|
| Scratch sentinel **dirty** | An access was redirected to the catcher during the frame. A GPU access the MMU refused still carried a write, and the catcher absorbed it — the PTB's pool write going here rather than to the pool would explain "`BPCA` advances, pool stays empty" exactly. |
| Fault latched / hub MMU int set, scratch **clean** | The refusal raised a fault without a write landing. `MMU_VIO_ADDR`/`VIO_ID` name the address and the issuing client. |
| No fault, scratch **pristine** | A materially stronger exoneration than V3D-60's: the MMU is armed to report on both channels for all three conditions and the catcher aims where no legal access can touch. The MMU is exonerated **with the instrument that could have convicted it**, and the PTB frame open/close unit stands further alone as the wall. |

V3D-60's `gmpdelta` already exonerated GMP + MMU across the frame, but it did so under the *unarmed*
config — which is precisely why that reading could not be final. This arc gives the next boot the
instrument to make the call definitively.

### Discipline

Every V3D-60 probe is **read-only** — no register is written, so no write needs justifying, and `CTRSTA`
stays disarmed. None of them waits on anything: no deadline, no polling window, no added boot stall.

`[v3d59] frameclose` is **banked as a deep probe** by this arc. Its verdict is delivered and standing —
dead-open, not slow and not overflow-stalled — and re-running its extended window every boot buys
nothing while costing a visible stall in the boot square. It belongs behind the deep-probe knob
(`UNAOS_V3D_DEEP`) introduced by the concurrent budget-trim arc, which owns that knob; at integration
`V3D59_FRAMECLOSE` resolves to `V3D_DEEP`, and until then the constant carries the knob-off value. The
V3D-60 probes themselves stay unconditionally on — they are fast probes with no wait at all.

QEMU `raspi4b` models no V3D. The pre-reset probe reads the hub identity word **first** and returns
before touching any core register unless that word is live, which is the same poison-honest gate
`probe_hub_ident0` uses; the armed QEMU run prints the `SKIPPED` line and the rest of the bring-up
short-circuits at `BLOCK-DOWN`. `kernel8-test` green therefore means *no regression*, nothing more —
**P60 metal reads the residue pair, the IDENT check, the init ledger, the sync read-test and the
protection delta.**

V3D-62 is the exception to the read-only rule above, and it is a narrow one. It writes exactly two
registers that `program_mmu` already wrote — `MMU_CTL` and `MMU_ILLEGAL_ADDR` — with values that widen
fault *reporting* and move the illegal-address landing site off mapped memory. No address the GPU could
not previously reach becomes reachable, and no CLE or PTB state is touched. Its own witnesses are pure
reads plus a sentinel seed into an unmapped page no job can address. The same poison-honest hub-identity
gate applies, so QEMU `raspi4b` prints none of it — **P62 metal reads `[v3d62] mmufix` and
`[v3d62] fault`.**

## 36. Inside the PTB frame unit — the first two discriminators (V3D-63)

Every instrument *outside* the PTB frame open/close unit now reads clean, so V3D-63
attacks two of the three claims the campaign still carries as facts but has never
falsified: that the control list executed (C1), and that items were fed to the PTB
(C2). Two probes, one boot, both `UNAOS_V3D_DEEP`-gated (each rung carries the
ladder's ~0.5 s `FLDONE` backstop) and both dormant on QEMU raspi4b, which models no
V3D block and returns at `BLOCK-DOWN` long before they run. `kernel8-test` green for
this arc means **no regression, nothing more**.

Both probes are driven from one 2×2 matrix at the tail of `empty_frame_bisection`:
`{Empty, Full}` × `{CL@0x35000, CL@0x39000}`, four kicks, one PCTR bank armed across
each. `submit_bisect_rung_at` is the generalised rung submitter — same byte-for-byte
kick as the `[v3d48]` ladder, with the CL's arena offset chosen by the caller.

### `[v3d63] ctrartifact` — is `CT0LC` a counter or a mirror?

The campaign's most-cited fact rests on `CT0LC` 0→`0x10000` and `CT0PC` 0→3, and
`0x10000` is bit-identical to `OFF_BIN_CL`. Publishing the *same* content at two
arena offsets separates the two readings; adding the `Empty`/`Full` axis separates
"placement-independent" from "content-dependent" instead of confounding them.

| Reading | Verdict |
|---|---|
| `CT0LC` echoes the CL's own offset/BA at both placements | **C1 falsified** — mirror, not count; re-aim at the CT0 kick / bin-thread start |
| `CT0LC` changes with placement, content held fixed | C1's evidence contaminated; "the list executed" is unproven |
| `CT0LC` placement-independent *and* content-dependent | **C1 stands**, now on a controlled reading |
| `CT0LC` independent of both | evidence inert — cite it neither way |
| `CT0PC` nonzero on a zero-primitive rung | **C2 loses its last support**, independently of RANK 1 |

`CT0CS`'s `CTRUN` at wait-exit is printed per rung as a second, independent read on
whether the bin control thread ever started.

### `[v3d63] ptbctr` — the counter bank, and what it deliberately does not claim

The arc asked for three *named* sources: CLE bin-thread active cycles, PTB
primitives-binned, PTB primitives-clipped. **All three are DROPPED, not guessed, and
the witness line says so on every boot.** This file admits a PCTR source id only when
it falls between the two verified anchors — 16 `QPU_CYCLES_VALID_INSTR` and 32
`CYCLE_COUNT` (the latter pinned to `v3d_regs.h V3D_PCTR_CYCLE_COUNT(ver)=32` for
ver<71). Every candidate for those three names lies *outside* that bracket, and this
tree carries no copy of `enum drm_v3d_perfcnt` to transcribe from. Naming them from
memory is precisely the fabricated-constant class of PI-V3D-4/6/7.

What is armed instead is a **search, not a claim**: seven source ids
(`V3D63_SWEEP_SRC = [1, 10, 11, 12, 13, 28, 29]`) written into the SRC mux and
printed **raw, by index, with no semantic label** — the treatment V3D-60 gave the
unsourced MMU interrupt halves. Writing an id into a 7-bit SRC field selects a mux;
it asserts nothing about what that mux carries.

**PCTR slot 2 is reserved, and the sweep may never occupy it.** `wait_fldone` samples
PCTR counter 2 on every bin wait and `emit_v3d55_clock_liveness` prints it as a hard
hardware verdict — `counter2(src32 CYCLE_COUNT)` — gated only on `PCTR_EN` bit 2,
which this bank always sets. A swept id parked there would have made all four banked
rungs print a fabricated *"CYCLE_COUNT FLAT — the V3D core clock is NOT advancing"*
off an unidentified mux, contradicting the bank's own control. `PCTR_SRC_CYCLE_COUNT`
is therefore pinned to slot 2, where it keeps the established `[v3d55]` witness
truthful **and** serves as this bank's control — one counter, both roles, no
re-labelling of an existing witness string. The seven swept ids live in slots
0,1,3,4,5,6,7; `v3d63_slot` is the single place that mapping is written down.

A counter's `OVERFLOW` bit is ORed into the moved-mask alongside its value: a source
that wrapped exactly back to zero would otherwise read as "never moved", which is the
same silent-instrument failure this campaign keeps having to unwind.

- src32 zero on any rung ⇒ the bank never counted ⇒ **INCONCLUSIVE**, explicitly not
  clean. Instrument first; read no slot as evidence.
- src32 live and every swept source zero on all four rungs ⇒ seven unidentified event
  sources are silent on a demonstrably clocked block. This becomes evidence *about the
  PTB/CLE* only once a source id is transcribed from the uapi enum and cross-checked
  against the 16/32 anchors — the next arc's first task.
- swept sources move on `Full` and stay zero on the zero-primitive rungs ⇒ a
  content-dependent signal, the signature a genuine primitive/thread counter shows.
  Identify the moving ids before naming what they prove.
- any swept source moves on a zero-primitive frame ⇒ whatever the mux carries is not
  gated on primitive content.

Programming order is the exact `v3d_perfmon_start` idiom already used by
`pctr_setup_cs_witness`, including the PI-V3D-39 SRC read-back that forces the posted
source-selects to retire before the counters are cleared and enabled. Only PCTR
config registers are written — no CLE/PTB state is touched, no GMP write, no
`MMU_CTL` change, no new MMIO window.

---

## 37. Calibrating the seven muxes against known ground truth (V3D-64)

V3D-63 armed a raw sweep of seven **unidentified** PCTR source ids and refused to name
them. P65v2 metal (capture `pi4-r23s1o`) came back with a shape: on zero-primitive bin
rungs `src13 ≈ 161M` and `src28 ≈ 645M` against the `src32 CYCLE_COUNT` control's
`≈ 645M`, with `src1/10/11/12/29` flat at 0. `src28` looks 1:1 with the core clock and
`src13` looks like clock/4 — but "looks like" is precisely the reasoning that produced
the PI-V3D-4/6/7 fabricated constants. This tree still carries no `enum drm_v3d_perfcnt`
to transcribe from, and **naming an id from memory remains banned**.

The honest identification channel is **calibration**: run the same bank across legs
whose truth is already established, and classify each mux by behaviour alone.

### The three legs

| Leg | What it is | Ground truth it supplies |
|---|---|---|
| **L1 idle** | a fixed `V3D64_IDLE_MS` (4 ms) wall-clock window off CNTPCT, **no job submitted** | there is no work, so anything that accumulates here is clock-derived |
| **L2 render** | the known-good CT1 clear job — `clear_job(None)`, the retiring M3 job `[v3d58] rerender` already reuses (**no new job is invented**), panel blit suppressed, its own store byte-verified | real, completing render work |
| **L3 bin** | one banked `Empty` rung of the `[v3d48]` ladder via `submit_bisect_rung_at(…, bank: true)` | the bin path — the frame opens and never retires, so a retirement-gated mux stays flat while a clock-derived one climbs through the ~0.5 s backstop |

Two reading notes for capture work. **The permille is a leg-fraction, not a duty cycle** —
on L2 it says what share of *that leg's own elapsed core cycles* the slot counted, and
the leg spans the whole `clear_job` call (setup, kick, CT1 idle-wait, verify), not just
the render frame; it is not "the render pipe was busy X‰ of the time". And the **L3 rung
prints its own line under the `[v3d48]` tag**, not `[v3d64]`: `submit_bisect_rung_at`
hard-labels its CL decode, Mesa diff and `wait_fldone` witnesses `v3d48 bisect` for every
caller. Readers who count rungs by tag will see one extra `[v3d48]` rung on a
`UNAOS_V3D_DEEP` boot; it belongs to this battery.

Every slot is reported as a **permille ratio against `src32`'s delta on its own leg**.
That is the whole point: a ratio held constant across three legs of wildly different
duration and workload is a strong statement about what a mux is derived from — and one
this file can make **without a name**.

### The classification table shape

Per swept id, one line: the raw value and permille on each leg, each leg's validity, and
one of four **behaviour classes**:

| Class | Condition | Reading |
|---|---|---|
| `CLOCK-LIKE` | permille within `[900, 1100]` of `src32` on **every** valid leg, no overflow anywhere, **and L1 valid** | derived from the core clock, not from work |
| `WORK-GATED-RENDER` | moved on L2 only; flat on L1 and L3, **and L1 valid** | gated on real render work |
| `SILENT` | zero on every valid leg, **overflow included** | idle, a retiring render frame and a bin kick all left it flat |
| `OTHER (RATIO UNUSABLE)` | moved, but latched an **overflow** on some valid leg | the raw value is a wrapped residue, so the permille is not a ratio and the slot is never band-tested; shorten the leg or re-measure |
| `OTHER (IDLE-BLIND)` | would have been CLOCK-LIKE or WORK-GATED-RENDER, but **L1 was invalid** | the differential stands; the idle claim does not |
| `OTHER` | moves, but neither 1:1 with `src32` nor render-only | read the per-leg permille: a ratio *held constant* across legs is clock-derived at that divisor; a ratio that *varies* is work-correlated |

Two guards the table depends on. **Overflow disqualifies the ratio, it does not just flag
it**: a wrapped `u32` is a residue rather than a delta, and a mux that wrapped once and
landed inside the ±10% window would otherwise be published as CLOCK-LIKE off a truncated
count — so any latched overflow on a valid leg forces `OTHER (RATIO UNUSABLE)` and the
band test is never applied. And **both idle-asserting classes require L1 to be valid**.
`CLOCK-LIKE` and `WORK-GATED-RENDER` each make a claim *about the idle leg* ("no work
exists to gate on", "flat on idle"), and two valid legs out of three does not guarantee
L1 is one of them. An invalid L1 is in fact the plausible failure here: `src32`
`CYCLE_COUNT` reading zero across a pure-idle window is exactly what an activity-gated
core clock would do. When L1 drops out the slot falls through to `OTHER (IDLE-BLIND)`,
whose wording claims nothing about idle.

A leg counts as a measurement only if **its own `src32` control moved**; a leg whose
control read zero is INCONCLUSIVE, never "silent". Fewer than two valid legs ⇒ the whole
battery prints `muxcal INCONCLUSIVE` and classifies nothing. A leg that did not run at
all (L2 with no passing M3 baseline, L3 fail-closed on a range escape) is printed as
**NOT RUN**, never folded in as a zero. `OVERFLOW` is ORed into movement throughout, so
a mux that wrapped exactly to zero cannot read as flat.

### Expected outcome tree on metal

Against the P65v2 shape, the discriminating outcomes are:

- **`src28` CLOCK-LIKE** (≈1000‰ on all three legs, idle included) ⇒ it is a second
  core-clock-derived counter, and every P65v2 reading of it as PTB/CLE activity is void.
- **`src28` OTHER, ≈1000‰ on L3 but ≈0 on L1** ⇒ it is *not* free-running: it is gated on
  something the bin kick supplies, and the 1:1 with `src32` on bin rungs was a
  coincidence of the backstop window. This is the outcome that would make it interesting.
- **`src13` OTHER at a constant ≈250‰ across all three legs** ⇒ clock/4, confirmed as a
  fixed divisor of the same clock rather than a work counter.
- **`src13` ≈250‰ on L3 but a different ratio on L2** ⇒ work-correlated, and the
  clock/4 reading is retracted.
- **`src1/10/11/12/29` SILENT across all three legs** ⇒ five muxes that a retiring render
  frame does not move either. Their silence on the bin path stops being evidence about
  the PTB specifically.
- **any of the five moving on L2 only** ⇒ `WORK-GATED-RENDER`, the first positive
  identification of a work-gated mux in this sweep.

### What is deliberately NOT claimed

**No source id is named here, and none may be inferred from these lines.** A class says
how a mux responded to a pure-idle window, a retiring CT1 render frame and a bin kick. It
says nothing about which unit it counts.

**The standing rule this arc establishes:** when a later arc transcribes an id from
`enum drm_v3d_perfcnt` and cross-checks it against this file's verified anchors (16
`valid_instr`, 32 `cycle_count`), **the transcribed name must match the class measured
here, or the transcription is rejected.** A name implying render or primitive work on a
mux measured `CLOCK-LIKE`, or a clock/cycle name on a mux measured `WORK-GATED-RENDER`,
is mis-transcribed. Calibration checks the transcription, never the reverse.

Slot 2 stays `PCTR_SRC_CYCLE_COUNT` under the `[v3d55]`/`V3D63_CTRL_SLOT` contract — the
slot `wait_fldone` samples for its clock-liveness verdict *and* this bank's own control —
so the L3 leg's `[v3d55]` line remains truthful. Writes are **PCTR config registers
only**, the same `v3d63_pctr_arm` idiom: no `CT*`/GMP/`MMU_CTL` write beyond what the
reused legs already perform, and no new MMIO window. The battery is `UNAOS_V3D_DEEP`-gated
and hub-identity gated, printing an explicit `SKIPPED` line when the block is down — QEMU
raspi4b returns at `BLOCK-DOWN` long before it runs, so `kernel8-test` green for this arc
means **no regression, nothing more**.

---

## 38. The first named source id — transcription under the calibration rail (V3D-65)

V3D-63 **dropped** three named PCTR sources rather than guess them, and V3D-64 closed with
a standing rule: when an id is finally transcribed, *the name must match the measured
class, or the transcription is rejected*. V3D-65 transcribes exactly **one** id and
submits it to that rule, in the same boot, on the same legs, with a verdict word on the
wire.

### The blocker is retired — where the enum came from

Both prior arcs recorded the same wall: *"this tree carries no copy of `enum
drm_v3d_perfcnt` to transcribe from"*. It is no longer true of the build host.

| | |
|---|---|
| **File** | `include/uapi/drm/v3d_drm.h` — the Linux uapi header |
| **Copy read** | `/usr/src/kernels/7.1.5-200.fc44.x86_64/include/uapi/drm/v3d_drm.h` (Fedora `kernel-devel` for 7.1.5-200.fc44, the build host's own running kernel; reachable from the session sandbox as `/run/host/usr/src/kernels/7.1.5-200.fc44.x86_64/…`) |
| **What** | the anonymous `enum { V3D_PERFCNT_* }` at the tail of the header, first member `V3D_PERFCNT_FEP_VALID_PRIMTS_NO_PIXELS` at index 0 |
| **Applicability** | the enum's own comment scopes those indices to **V3D 4.2**. This hardware *is* V3D 4.2, so the deprecation note is a positive applicability statement for us. Newer cores must query `DRM_IOCTL_V3D_PERFMON_GET_COUNTER`; we are not a newer core. |

### What licenses the transcription — a six-for-six index cross-check

This is strictly stronger than V3D-63's "must fall between the 16/32 anchors" bracket
rule. Counting that enum from 0, **every** source id this file already carries from
earlier arcs lands on exactly the name it was given — six hits, zero misses:

| id | enum member at that index | this file's constant |
|---|---|---|
| 14 | `QPU_ACTIVE_CYCLES_VERTEX_COORD_USER` | `PCTR_SRC_QPU_ACTIVE_CYCLES_VERTEX_COORD_USER` |
| 16 | `QPU_CYCLES_VALID_INSTR` | `PCTR_SRC_QPU_CYCLES_VALID_INSTR` — **anchor** |
| 17 | `QPU_CYCLES_TMU_STALL` | `PCTR_SRC_QPU_CYCLES_WAITING_TMU` |
| 24 | `TMU_TCACHE_ACCESS` | `PCTR_SRC_TMU_TCACHE_ACCESS` |
| 25 | `TMU_TCACHE_MISS` | `PCTR_SRC_TMU_TCACHE_MISS` |
| 32 | `CYCLE_COUNT` | `PCTR_SRC_CYCLE_COUNT` — **anchor** |

`32 == CYCLE_COUNT` independently re-pins the `v3d_regs.h V3D_PCTR_CYCLE_COUNT(ver)=32`
anchor from a second, unrelated file. Six independent hits with no misses is not an
accident of ordering: on 4.2 the **enum index is the hardware source id**, exactly as this
file has assumed since PI-V3D-21 — now demonstrated rather than assumed.

### The one id, and why this one

**Index 28 is `V3D_PERFCNT_BIN_ACTIVE`.** It is picked over every other candidate because
it is the only id that is *both* bin/PTB-side *and* already carries a **measured V3D-64
behaviour class** on the same boot:

- **It is the sweep's one mover.** P65v2 metal (capture `pi4-r23s1o`) read `src28 ≈ 645M`
  against a `src32 CYCLE_COUNT` control of `≈ 645M` on zero-primitive bin rungs — the only
  swept slot at 1:1 there. V3D-64 flagged exactly that as the ambiguity worth resolving:
  free-running clock, or gated on something the bin kick supplies? Under the name
  `BIN_ACTIVE` the second reading is the live one, and it is the campaign's first direct
  observable *of the binner*: the frame opens, the bin unit stays active across the whole
  ~0.5 s `FLDONE` backstop, and it still never retires. "The binner is engaged and never
  finishes" is precisely the wall this track has been circling.
- **Every other bin/PTB-side candidate is out of reach of the rail this boot.** 35
  `PTB_PRIMS_BINNED`, the `PTB_MEM_*` family and 57 `CLE_ACTIVE` are not in
  `V3D63_SWEEP_SRC`, so naming one of them would publish an **unchecked** name — the exact
  thing this arc exists to refuse. One id, and it must be one calibration can judge.
- **The five confirmed-silent ids (1, 10, 11, 12, 29) are checkable but uninformative.** A
  `SILENT` measurement is consistent with almost any name on a bin path that bins no
  primitives.

**Ids 1, 10, 11, 12, 13 and 29 stay raw and unnamed.** The V3D-63 treatment holds for them
and nothing in the V3D-65 lines may be read as naming them by adjacency.

### `[v3d65] srcname` — the anchor check

The predicates are derived from the name and from nothing else. `BIN_ACTIVE` means "the
bin unit is active", so, in V3D-64's own vocabulary:

| Predicate | Leg | What the name requires | Failing it means |
|---|---|---|---|
| **P1 idle-flat** | L1 pure idle, no job submitted | reads **zero**, overflow included | it ticks with no bin work ⇒ it measures the clock, not the binner |
| **P2 bin-live** | L3 banked `Empty` rung, frame opens and holds open across the backstop | **moves** | a `SILENT` mux is not a bin-activity counter |
| **P3 class** | — | measured class is neither `CLOCK-LIKE` nor `SILENT` | states the verdict against the *published* class string, not only raw slot values |

P3 is redundant with P1+P2 by construction; it is evaluated anyway so the verdict is
pinned to the class line a reader actually sees.

**L2 is reported and is *not* a reject condition.** Whether a CT1-only clear job spins the
bin unit is a question the name `BIN_ACTIVE` does not settle, and an anchor check may not
reject on a prediction the name never made. The L2 raw value, overflow flag and permille
are printed so the capture carries the observation.

**A verdict requires L1 and L3 to be valid legs** (each leg's own `src32` control moved).
Without them the probe prints `anchor-check INCONCLUSIVE` and says **neither PASS nor
REJECTED** — a name is never confirmed off an uninstrumented bank, which is the failure
mode this campaign keeps unwinding.

### The two outcomes, and what each does and does not license

- **`PASS`** — src28 is flat across a pure-idle window and moves on a bin kick whose frame
  opens and never retires. Every predicate the name makes is borne out by behaviour
  measured independently of the name. The campaign then has a **named bin-side
  observable**: the bin unit *is engaged* across the whole `FLDONE` backstop while the
  frame still fails to close — the binner is not absent and not asleep, it is running and
  not finishing. Two standing limits: this confirms the **id↔name transcription and the
  mux's behaviour**, not any semantic beyond "bin unit active"; and it names **one** id.
- **`REJECTED`** — the name is **withdrawn**, src28 reverts to a raw unnamed mux under the
  V3D-63 treatment, and nothing in the tree may cite it as the binner. Note what a reject
  does *not* implicate: the six-for-six index cross-check is independent of the
  measurement, so the likely fault would be the **id↔mux mapping on this silicon** (a SRC
  field that does not select the enum's source on 4.2), not the enum reading.

### Cost, gating and where the verdict is taken

The check adds **no job, no leg, no register write and no second measurement** — it reads
the three legs `[v3d64]` already measured, inside `v3d64_mux_calibration`, after that
probe's own verdict line. It therefore inherits V3D-64's gating unchanged: `UNAOS_V3D_DEEP`
plus the hub-identity gate, with an explicit `SKIPPED` line when the block is down.

**QEMU raspi4b models no V3D PCTR block at all**, so this never runs there — the caller
returns at `BLOCK-DOWN` long before it. The `PASS`/`REJECTED` verdict is **metal-attended,
always**. `kernel8-test` green for this arc means the default boot is byte-inert and the
suite did not regress, and nothing more.

### Outcome on metal — `REJECTED` (recorded here, drives V3D-66)

The P69 metal boot ran the check and it **rejected** the name: `src28` measured
**`CLOCK-LIKE`** — 1:1 with the `src32 CYCLE_COUNT` control on *every* leg, the pure-idle
leg included, where `BIN_ACTIVE`'s own P1 predicate requires zero. Per the rule as written,
the name is **withdrawn** and `src28` reverts to a raw unnamed mux; nothing in this tree
cites it as the binner.

What that does and does not implicate is exactly what the reject wording pre-committed to.
The six-for-six index cross-check is independent of the measurement and **stands**, so the
enum reading is right; the fault is downstream of it — on this silicon the 7-bit `SRC`
field does not select the enum's source for every id. **`id↔mux` validity is partial, and
partial per id.** That is the standing caveat every later arc must carry, and it is the
reason §39 measures instead of transcribing.

## 39. Widening the sweep to the PTB-neighbourhood ids — measurement, not transcription (V3D-66)

With `id↔mux` only partially valid, a name lifted from the enum is worth exactly the
behaviour that backs it. V3D-66 therefore adds **eight more source ids** to the calibration
and publishes their raw counts and behaviour classes. **It names nothing.** Transcription is
left to V3D-67, which will have these classes to be judged against.

### The eight ids, cited exactly

Same header, same copy, same enum as V3D-65 — `include/uapi/drm/v3d_drm.h`, read from
`/usr/src/kernels/7.1.5-200.fc44.x86_64/include/uapi/drm/v3d_drm.h` (reachable from the
session sandbox as `/run/host/usr/src/kernels/…`), the anonymous `enum { V3D_PERFCNT_* }`
whose first member `V3D_PERFCNT_FEP_VALID_PRIMTS_NO_PIXELS` is index 0 and whose own
comment scopes these indices to V3D 4.2 — this hardware.

| id | enum member in that copy | line |
|---|---|---|
| 30 | `V3D_PERFCNT_L2T_HITS` | `v3d_drm.h:653` |
| 31 | `V3D_PERFCNT_L2T_MISSES` | `v3d_drm.h:654` |
| 33 | `V3D_PERFCNT_QPU_CYCLES_STALLED_VERTEX_COORD_USER` | `v3d_drm.h:656` |
| 34 | `V3D_PERFCNT_QPU_CYCLES_STALLED_FRAGMENT` | `v3d_drm.h:657` |
| 35 | `V3D_PERFCNT_PTB_PRIMS_BINNED` | `v3d_drm.h:658` |
| 36 | `V3D_PERFCNT_AXI_WRITES_WATCH_0` | `v3d_drm.h:659` |
| 37 | `V3D_PERFCNT_AXI_READS_WATCH_0` | `v3d_drm.h:660` |
| 38 | `V3D_PERFCNT_AXI_WRITE_STALLS_WATCH_0` | `v3d_drm.h:661` |

**A correction to the brief that raised this arc, recorded rather than quietly absorbed.**
The set was requested as "the `PTB_BLOCKED_CYCLES` / `PTB_PRIMS_BINNED` family". There is
**no `V3D_PERFCNT_PTB_BLOCKED_CYCLES` in this header at all** — `grep PTB` over the enum
returns exactly seven members (10 `PRIM_VIEWPOINT_DISCARD`, 11 `PRIM_CLIP`, 12 `PRIM_REV`,
35 `PRIMS_BINNED`, plus `PTB_MEM_WRITES`, `PTB_MEM_READS`, `PTB_W_MEM_WORDS` in the
AXI/memory tail). Of the eight indices above exactly **one**, 35, is a PTB member. The set
is swept as given because the set is what the brief pinned, and because — after the V3D-65
reject — what an id's enum member *says* is not what licenses anything. It is a **search
window** and is defensible only as one; 35's PTB name buys it no head start over the other
seven.

### Batching against the PCTR slot count

The bank is **eight counters** (`V3D_V4_PCTR_0_SRC_0_3` / `_4_7`, one 7-bit `SRC` field
each) and **slot 2 is permanently reserved** for the `src32 CYCLE_COUNT` control per the
`[v3d55]`/`V3D63_CTRL_SLOT` contract — `wait_fldone` samples PCTR counter 2 on every bin
wait and prints a hard clock-liveness verdict off it. Seven usable sweep slots, and
`V3D63_SWEEP_SRC`'s seven ids already fill them. So the eight new ids run as **two extra
passes of four**: a fresh arm of the same bank with only slots 0,1,2,3,4 enabled
(`V3D66_PCTR_MASK = 0x1F`), the same `v3d63_slot` packing, the same control in slot 2, and
each pass **re-runs all three legs** so every permille is taken against a control measured
on its own pass. Slots 5..7 are left unselected *and* disabled, so an unselected mux can
never be mistaken for a measurement.

The legs are V3D-64's, unchanged and reused, no new job class invented: **L1** a 4 ms pure
idle window with nothing submitted, **L2** the known-good `clear_job` (only a job that
passed is ground truth), **L3** one banked `Empty` rung of the `[v3d48]` ladder. The L3 rung
is submitted with `bank: false` so it does not re-arm the seven-id bank over ours; the pass
arms and reads its own bank around the kick. `V3D63_SWEEP_SRC` and everything `[v3d63]`,
`[v3d64]` and `[v3d65]` print are untouched.

### The new class — `WORK-GATED-BIN`

V3D-64's vocabulary had `WORK-GATED-RENDER` but no mirror on the bin side; a mux that moved
only on a bin kick printed a bare `OTHER`. V3D-66 splits that case out, **defined by
predicate and by nothing else**:

> the L1 idle leg is **valid** *and* the L3 bin leg is **valid** *and* the slot **moved** on
> L3 (raw ≠ 0 or a latched overflow) *and* the slot did **not** move on L1 *and* the slot did
> not move on L2 if L2 is valid.

It requires a valid L1 on purpose: "moves only on bin" is a claim about the idle leg, and a
boot whose L1 dropped out cannot make it — such a slot falls through to V3D-64's own
`OTHER (IDLE-BLIND)` wording, which claims nothing about idle. It **cannot collide with
`CLOCK-LIKE`**: the predicate demands a valid L1 on which the slot read zero, whose permille
is 0 and therefore outside the ±10 % band `CLOCK-LIKE` requires on every valid leg. The
split is a **refinement of `OTHER`**, not a reinterpretation of any class V3D-64 already
published — `[v3d63]`/`[v3d64]`/`[v3d65]` still call `v3d64_classify` directly and their
output is byte-identical.

A mux in `WORK-GATED-BIN` is the thing the render CLE's wall actually needs: a candidate
witness for whether the binner emits anything at all. V3D-67 may test a PTB name against it —
under the rule, and only under the rule.

### What is deliberately NOT claimed

- **No id is named.** Not 35, whose enum member is `PTB_PRIMS_BINNED`. The wire lines print
  `srcNN=value` and a class, with the standing caveat that V3D-65 proved `id↔mux` validity
  partial on this silicon.
- **No class is a semantic.** A class says how a mux responded to a pure-idle window, a
  retiring CT1 render frame and a bin kick. It says nothing about which unit it counts.
- **The standing rule is unchanged and restated on the wire:** a transcribed name must match
  the class measured here or the transcription is rejected. Calibration checks the
  transcription, never the reverse.

### Cost, gating and where the verdict is taken

Two passes × (a 4 ms idle window + one CT1 clear job + one banked `Empty` rung with its
~0.5 s `FLDONE` backstop) = **two more rungs and ~1 s of extra deep boot**; the `[v3d]
deep=on` budget line now reads **13 rungs / ~7 s**. Gating is V3D-64's, unchanged:
`UNAOS_V3D_DEEP` plus the hub-identity gate, with an explicit `SKIPPED` line when the block
is down. Writes are PCTR config registers only — no new MMIO window, no new job, no new CL.

**QEMU raspi4b models no V3D block**, so `[v3d66]` never runs there and every class below is
**metal-attended**. `kernel8-test` green for this arc means the default boot is byte-inert
and the suite did not regress, and nothing more.

---

## 40. Varying the frame — the one input the campaign never varied (V3D-68)

Nine arcs have varied the control list's content, its placement, its length, the cache mode
around it, the reset before it, the interrupt policy over it and the counter bank across it.
Go back through every bin frame this campaign has ever kicked — `[v3d40]`'s probe, the six
`[v3d48]` ladder rungs, `[v3d50]`/`[v3d51]`/`[v3d53]`, `[v3d63]`'s 2×2 matrix,
`[v3d64]`/`[v3d66]`'s L3 legs, the real M4 draw — and every single one of them programmed the
**same** `TILE_BINNING_MODE_CFG`: a 64×64 frame, which at this tree's established 64×64 bin
tile is **exactly one tile**. §31 proved that geometry is encoded exactly as Mesa encodes it.
It could not prove it is a geometry the PTB can close, because it never compared it with
another. A one-tile frame is the most degenerate legal frame there is, and it is the frame on
which the campaign's most load-bearing fact was measured.

That fact is itself unvaried. "An empty bin frame must retire" is sourced from the kernel's
`v3d_bin_job_run`, which asserts nothing of the kind, and **no Mesa path ever submits one** —
gallium drops a job whose draw bounds enclose nothing; v3dv never starts a frame it will not
draw into. "The empty frame did not retire" may simply be the hardware's honest answer to a
question no driver asks.

### The candidate ranking, and what already excludes each

| Rank | Candidate | Excluding evidence |
|---|---|---|
| **1** | **Frame geometry / tile count** — `TILE_BINNING_MODE_CFG` width/height, and the tile-state array that must scale with it | **Nothing.** §31 audited the *encoding* of one geometry, never a second geometry. Unvaried in every arc. |
| **2** | **The empty-frame premise** — a minimal *non-empty* frame at a *non-degenerate* geometry | Partially varied: `Full` at 64×64 also wedges (§22). What was never varied is content **×** geometry jointly. |
| **3** | **Tile-alloc pool arming** — is the pool consulted, or merely latched? | `BMOOM=0`, `OUTOMEM` never fired (§23) — consistent with "pool fine" *and* with "pool never consulted". `BPCS` reloads from the `CT0QMS` write (§32 S1), so the register path is live. Armed here as a **witness**, not a perturbation. |
| 4 | The CL epilogue — `HALT` vs `FLUSH` vs `FLUSH_ALL_STATE`, `INCREMENT_SEMAPHORE` | **CLOSED by §33 T4**: `v3dX(bcl_epilogue)` emits a bare `FLUSH` (4) — no `INCREMENT_SEMAPHORE`, no `FLUSH_ALL_STATE`. Our terminator is already mainline-exact. Not armed. |
| 5 | `CT0CA` vs `CT0EA` end condition | `CT0CA` reaches `EA` every rung and the submit audit is byte-exact (§28 `[v3d54]`). The CLE demonstrably consumed the `FLUSH`. Not armed. |
| 6 | PTB output address config (`CT0QMA`/`QMS`/`QTS`) | Byte-exact against `v3d_bin_job_run`, echoes verified (§23 `[v3d49]`); the uapi confirms `qma`=tile alloc, `qms`=its size, `qts`=tile state — our mapping. Not armed. |
| 7 | L2T / slice-cache interposition on PTB writes | The `[v3d51]` FLUSH vs `[v3d53]` kernel-exact CLEAR differential wedged both ways (§29), and CT1 stores land through the same L2T. Not armed. |
| 8 | Clock / power gating of the PTB sub-unit | `clkdom` ACTIVE at 500 MHz, `clkliv` Δ249 M (§29); `src32 CYCLE_COUNT ≈ 645 M` across the bin leg (§37). The block is clocked. `MISCCFG.QRMAXCNT` stays disarmed per `V3D55_ARM_QRMAXCNT`. |
| 9 | GCA / GMP interposition | `PROT_ENABLE=0`, `gmpdelta` clean, and V3D-62's armed fault policy caught nothing on either channel (§35, §"fault-reporting instrument"). GCA is a `ver<41` path. Not armed. |
| — | `BXCF` (PTB binner extra config) | Mainline defines it and **writes it from no path**. Read and printed per rung; **never written** — a rung that both re-shaped the frame and poked an undocumented PTB config register would discriminate nothing. |

### The battery — `[v3d68] binwall`

Four rungs, `UNAOS_V3D_DEEP`-gated, riding the very tail of `empty_frame_bisection` (after
every banked verdict and every calibration leg, because this is the only battery that
re-points `CT0QTS` and publishes poison rather than zeros into the shared bin regions).

| Rung | Frame | Tiles | Content | What it discriminates |
|---|---|---|---|---|
| 1 | 64×64 | 1 | `Empty` | the campaign's banked frame, re-run **in this boot** so every other rung is a differential against a same-boot control, not against another capture |
| 2 | 128×128 | 4 | `Empty` | geometry alone |
| 3 | 256×256 | 16 | `Empty` | geometry alone, at 16× the tile count |
| 4 | 256×256 | 16 | `Full` | the smallest non-empty frame at a **non-degenerate** geometry — same shader record, same vertex data, same triangle as M4; only the frame it is binned into differs |

Each rung is `submit_bisect_rung_geom` — the identical `[v3d48]` kick sequence, cache
maintenance, `[v3d54]` submission audit and ~0.5 s `FLDONE` backstop — with three coupled
changes that are the one variable: the CFG packet's width/height, a dedicated 24 KiB
tile-state array at `OFF_V3D68_TSDA` (sized to the whole region regardless of the rung, so an
under-provisioned TSDA can never be the confound), and the packing witness's expected
width/height. Clip window, viewport offset and clipper scaling stay at their 64×64 values on
every rung.

Both bin regions are **poisoned**, not zeroed, with `[v3d56]`'s index-encoding seed: "the PTB
wrote zeros" and "the PTB wrote nothing" are different answers, and the old detector was
256 bytes. This one is 24 KiB.

### The reservation prediction — a positive PTB signal without a retire

Mesa `v3d_util.c::v3d_tile_alloc_sizes` states that the PTB requests the tile-alloc **initial
block per tile** at `START_TILE_BINNING`, before and independent of any primitive, then
allocates in aligned 4 KiB chunks (two up front). §30 measured `BPCA` advancing by exactly
`align(1·128, 4096) + 8192 = 0x3000` on the one-tile frame — the formula's value to the byte,
over a pool the poison proved untouched. **One calibration point cannot separate "reserved
per tile" from "grabs three 4 KiB chunks regardless."**

A 16-tile frame reserves 2048 B, which still rounds to `0x3000`. Under the 32×32 tile-size
contingency the *same* frame is 64 tiles and reserves 8192 B, which rounds to `0x4000`. So the
`BPCA` column measures the effective tile size **and** tests the per-tile reservation model at
the same time, **on a frame that never retires**. Both predictions are printed per rung with
their own match flags; a match at both hypotheses means the prediction did not discriminate on
that frame, and the tile size stays *as established*, never *as measured*.

### The outcome tree

- **Any rung retires** ⇒ the wall is not unconditional. A retire on a multi-tile **empty**
  frame means the one-tile frame was the defect and every "empty does not retire" verdict in
  this campaign was measured on a degenerate frame. A retire only on the **non-empty** rung
  means a 4.2 binner does not close a frame it was given nothing to bin, and the campaign's
  most load-bearing premise is **retracted**. Either way the next arc is a bring-up arc.
- **No retire, but poison disturbed** ⇒ the first bin-side memory traffic the campaign has ever
  caught, caught by growing the detector rather than changing the engine. The wall moves
  strictly downstream of the write.
- **No retire, no byte, but a reading moved with tile count** ⇒ the frame unit *is* responsive
  to frame shape. Geometry reaches the PTB front end, so the front end is running.
- **No retire, no byte, every reading bit-identical across 1/4/16 tiles and across
  empty/non-empty** ⇒ frame geometry is excluded with the same force as encoding, submission,
  cache mode, reset, MMU and clock. That removes the last shape- or content-shaped theory on
  the board and leaves exactly two never-written mainline-defined configuration registers —
  PTB `BXCF` and `MISCCFG.QRMAXCNT` — plus the possibility that this block needs an
  initialisation the campaign has not found.

Two guards the tree depends on. **Fewer than two rungs kicked ⇒ `INCONCLUSIVE`**, never a clean
result: a geometry ladder with one frame has no differential. And **the poison column is
evidence only when every rung's L2T write-back completed** — a rung whose drain did not finish
disqualifies the memory-side reading for the whole ladder, while the retire and register
columns (MMIO reads, not memory reads) stand on their own.

### Fail-closed sizing

A rung is kicked only when its tile-state array fits the dedicated region **under both tile-size
hypotheses**: 256×256 is 16 tiles at 64×64 (4 KiB) and 64 tiles at 32×32 (16 KiB), both under
24 KiB. `v3d68_geom_sound` enforces that bound plus the Mesa-minimum pool at the contingent
tile count, and a geometry that cannot be bounded is **reported and skipped, never kicked** — a
mis-estimated tile size can cost the ladder a reading, never a write outside the arena.

### Cost, gating and what is not armed

Four more rungs with their `FLDONE` backstops plus a poison fill and scan of a 24 KiB tile-state
region and the 32 KiB pool per rung; the `[v3d] deep=on` budget line now reads **17 rungs /
~9 s**. Writes are the rungs' own already-existing kick sequence and the PCTR config the
`[v3d63]` bank arms — **no `BXCF` write, no `QRMAXCNT` write, no `CTRSTA`, no GMP or `MMU_CTL`
change, no new MMIO window, and the real M4 draw untouched.**

The gating is verified by building `kernel8` both ways: the armed-with-deep image carries 12
`[v3d68]` strings, the armed-without-deep image carries **zero** and is 54,720 bytes smaller.
**QEMU raspi4b models no V3D block**, so the battery returns at its hub-identity gate with an
explicit `SKIPPED` line and every verdict here is **metal-attended**; `kernel8-test` green for
this arc (86/86) means the default boot is byte-inert and the suite did not regress, and
nothing more.

---

## 41. Diffing the initialisation paths — reading the fabric, not the engine (V3D-69)

§40 excluded frame geometry and frame content with the same force as encoding, submission, cache
mode, reset, MMU and clock, and named what was left: PTB `BXCF` (mainline defines it and writes it
from no path), `MISCCFG.QRMAXCNT` (disarmed since §29), **or an initialisation this campaign has not
found**. Nine arcs had varied the state this driver puts *in front of* the block. This one stops
probing our own state and reconstructs what mainline does between cold power and its first
successful bin kick, then diffs.

### 41.1 The reconstructed mainline path

Sourced from working knowledge of the Linux tree (`drivers/soc/bcm/bcm2835-power.c`,
`drivers/gpu/drm/v3d/*`, `arch/arm/boot/dts/bcm2711.dtsi`). **No network was available**, so every
step below is a reconstruction from memory and each one carries its own confidence marker. A step
marked **UNCERTAIN** is one this tree must not treat as settled.

| # | Stage | Step | Confidence |
|---|---|---|---|
| 1 | firmware | `start4.elf` powers the GRAFX rail, runs the inrush/POWOK/ISPOW/MEMREP/ISFUNC ramp and leaves the V3D clock present | established (this is why our mailbox `SET_DOMAIN_STATE` domain 10 ACKs) |
| 2 | DT | `v3d` node: `power-domains = <&pm GRAFX_V3D>`, `resets = <&pm BCM2835_RESET_V3D>`, `clocks = <&firmware_clocks 5>`, `reg` = hub + core0 | high |
| 3 | DT | `pm` node carries **three** reg ranges: `pm` (VC `0x7e100000`), `asb` (VC `0x7e00a000`), `rpivid_asb` (VC `0x7ec11000`) | **DT-VERIFIED** (V3D-70) — decoded from the shipping `bcm2711-rpi-4-b.dtb`; see §41.8 |
| 4 | `bcm2835-power` | `bcm2835_power_power_on(PM_GRAFX)` — the POWUP → POWOK → ISPOW → MEMREP/MRDONE → ISFUNC ramp — **returns early on BCM2711** (`if (power->rpivid_asb) return 0`): the firmware owns it | high; this tree has asserted it since §PI-V3D-3 |
| 5 | `bcm2835-power` | `bcm2835_asb_power_on(PM_GRAFX, ASB_V3D_M_CTRL, ASB_V3D_S_CTRL, PM_V3DRSTN)`: enable the domain clock, ~1 µs settle, **deassert `PM_V3DRSTN`** (bit 6, PM password), then `bcm2835_asb_enable` the **master** then the **slave** bridge | high on the sequence and on bit 6 |
| 6 | `bcm2835-power` | `bcm2835_asb_control` **re-routes** `ASB_V3D_{S,M}_CTRL` to the `rpivid_asb` block whenever that block is present — always, on BCM2711 | high |
| 7 | `bcm2835-power` | ASB word bits: `REQ_STOP`(0), `ACK`(1), `EMPTY`(2), `FULL`(3); enable = clear `REQ_STOP` with the PM password, then poll until `ACK` clears | high on 0/1, medium on 2/3 |
| 8 | `bcm2835-power` | Generic PM word bits `POWUP`(3) `POWOK`(4) `ISFUNC`(5) `MRDONE`(7) `MEMREP`(8) `ISPOW`(9) `ENAB`(12), inrush shift 13 | **UNCERTAIN** — reconstructed, and it collides with the per-domain reset-bit defines in the same word |
| 9 | v3d probe | `clk_prepare_enable`, `devm_reset_control_get`, ioremap `hub` + `core0` (`gca`/`bridge` are `ver<41` only) | high |
| 10 | v3d probe | read `V3D_HUB_IDENT1`, derive `ver = tver*10 + rev` | established (§35, §"V3D-61") |
| 11 | v3d probe | `dma_set_mask_and_coherent(...)` before any DMA allocation | **UNCERTAIN** on the mask width |
| 12 | `v3d_gem_init` | allocate a **4 MiB** page table (zeroed) and a 4 KiB MMU scratch page; `drm_mm_init` starts at **page 1** — "various bits of HW treat 0 as special" | high on the shape, medium on the size |
| 13 | `v3d_init_hw_state` → `v3d_init_core` | `MISCCFG.OVRTMUOUT` only when `ver < 41`; then `L2TFLSTA = 0`, `L2TFLEND = ~0` | high — mirrored since §PI-V3D-51 |
| 14 | `v3d_mmu_set_page_table` | `MMU_PT_PA_BASE = pt_paddr >> 12`; `MMU_CTL = ENABLE \| PT_INVALID_{ENABLE,ABORT,INT} \| WRITE_VIOLATION_{ABORT,INT} \| CAP_EXCEEDED_{ABORT,INT}`; `MMU_ILLEGAL_ADDR = scratch_pfn \| ENABLE` | high — mirrored, §"V3D-62" |
| 15 | `v3d_mmu_flush_all` | `MMUC_CONTROL = FLUSH \| ENABLE`, wait the flush out, then `MMU_CTL \|= TLB_CLEAR` and wait | high on the writes, medium on the wait bits |
| 16 | `v3d_irq_enable` | unmask the core working set **and** the hub half | high — mirrored, §26 |
| 17 | first job | `v3d_bin_job_run`: `CT0QMA`/`CT0QMS`, `CT0QTS \| ENABLE`(bit 1), then `CT0QBA`/`CT0QEA` as the GO | established, byte-exact since §23 |

### 41.2 The diff against our bring-up

We boot bare-metal off `start4.elf` with no Linux, so anything the DT/power framework does that the
firmware does not do by default is a candidate hole.

| Mainline step | Ours | Status |
|---|---|---|
| 1, 4 (firmware rail + ramp) | mailbox `SET_DOMAIN_STATE` domain 10, `SET_CLOCK_RATE` id 5 @ 500 MHz, `SET_CLOCK_STATE` | **covered**, and mainline skips the ramp on BCM2711 anyway |
| 5 (`PM_V3DRSTN` + both bridges) | `v3d_reset_cycle` = OFF half then `enable_pm_asb` | **written, never read back as a verdict** → the V3D-69 read half |
| 6 (which ASB block) | we drive `rpivid_asb` at `0xFEC1_1000` | **DT-verified correct** (§41.8); the routing column stays as the cross-check |
| 7 (`EMPTY`/`FULL`) | never read | **never looked at** → new columns |
| 12 (page table shape) | identity-mapped arena only; iova ≠ 0 by construction | equivalent-in-effect; the "page 0 is special" rule is respected |
| 13, 14, 16, 17 | mirrored byte-exact | covered (§26, §29, §35, §"V3D-62") |
| 15 (`MMUC_CONTROL`) | we write `FLUSH \| ENABLE` in `program_mmu` | **written, never read back** → the V3D-69 `MMUC` column |
| 11 (DMA mask) | N/A — no DMA API; physical addresses throughout | not a hole |

### 41.3 The ranked holes

1. **`MMUC_CONTROL.ENABLE` never confirmed latched.** Cheapest to check, and a real defect if it
   reads clear. It would not by itself explain a *bin-exclusive* wall — the render path crosses the
   same MMU — so it is a thing to fix and re-measure, not a verdict on the binner.
2. **ASB routing.** If `ASB_V3D_{S,M}_CTRL` are not actually backed at `0xFEC1_1000`, every release
   this driver has issued since PI-V3D-3 landed on a window nobody reads. *Settled by V3D-70:* the
   base is DT-correct, and the identity falsifier is the legacy block's `ASB_AXI_BRDG_ID` at `+0x20`
   — the only identity register any ASB reg range in the DT actually covers (§41.8).
3. **The PM_GRAFX isolation bits.** Mainline never writes them on BCM2711, so a divergence here is
   a firmware-configuration finding, not a driver one — and the bit map is UNCERTAIN, so the raw
   word gets primary billing and the decode is explicitly hedged.
4. **The ASB master-bridge hypothesis** — ranked **last**, deliberately.

### 41.4 Why the bridge hypothesis is ranked last

The attractive story: the binner's AXI path through the master bridge was never enabled, so the
block is alive on the register bus (every MMIO read works, every job latches, every register echoes)
yet cannot master a write to memory — which matches *every* observation, including the total absence
of an error or an interrupt.

It does not survive contact with the render engine. On this same block, this same boot: a **CT1
render job retires and lands CPU-verified pixels in arena memory** (§`[v3d58] xengine`); the **CLE
fetches every bin control list out of DRAM** and `CT0CA` walks it to `EA` (§28); and the **V3D MMU's
own page-table walk is a memory read**. V3D 4.2 puts *one* master port behind the hub MMU per core —
the PTB does not have a private one — so those three facts are direct evidence that the master
bridge passes both reads and writes. A stopped master would take the render path and the list fetch
down with the binner, and it demonstrably does not.

That is a refutation on inference, and the inference rests on a block diagram this tree has never
read a register out of. Hence the arc: four MMIO reads settle the branch on the wire instead.

### 41.5 The rungs

**Read half — `[v3d69] fabric` (rides `UNAOS_V3D_DEEP`, writes nothing).** Two stations, either side
of one banked `Empty` control rung that is the `[v3d48]` kick verbatim. Each station is seven MMIO
loads inside the Device window `boot.rs` already maps — no new mapping (columns as corrected by
V3D-70; the station lines carry the `[v3d70]` tag):

| Column | Read | Mainline expectation |
|---|---|---|
| `PM_GRAFX` raw + `V3DRSTN` | `PM_BASE + 0x10c` | `V3DRSTN` **set** |
| `PM_GRAFX` POWUP/POWOK/ISFUNC/MRDONE/MEMREP/ISPOW/ENAB | same word | *whatever `start4.elf` left* — mainline never writes them on BCM2711; map **UNCERTAIN** |
| `rpivid_asb` identity | — | **none exists**: `ASB_AXI_BRDG_ID` is at `+0x20`, one word past this range's `0x20` length. No identity column is printed for this block |
| `ASB_V3D_M_CTRL` / `S_CTRL` + `REQ_STOP`/`ACK`/`EMPTY`/`FULL` | `0xFEC1_1000 + 0x0c` / `+0x08` | `REQ_STOP = 0` **and** `ACK = 0` on both |
| legacy `asb` `ASB_AXI_BRDG_ID` | `0xFE00_A000 + 0x20` | ASCII `'brdg'` = `0x62726467` — **the** identity check, and the only one in DT range |
| legacy `asb` `M_CTRL` / `S_CTRL` | `+0x0c` / `+0x08` | some *other* peripheral's bridges, or dead — routing falsifier only |
| `V3D_MMUC_CONTROL` + `ENABLE`/`FLUSH` | hub `+0x1000` | `ENABLE` **set** |

**Write half — `[v3d69] reenable` (separate knob `UNAOS_V3D69_REENABLE`, off by default even on a
deep boot).** Re-runs mainline's `bcm2835_asb_power_on` ON half (`enable_pm_asb`) and re-kicks the
control rung, so the retire verdict either side is a same-boot differential. It is **gated on the
finding as well as on the knob**: when the read half reports a clean fabric it declines, writes
nothing, and says so — re-releasing an already-released bridge perturbs a live block and the outcome
would be uninterpretable either way.

### 41.6 Expected wire, per outcome branch

- **`ASB_AXI_BRDG_ID` ≠ `'brdg'`** ⇒ no bridge reading may be cited at all. The bases are DT-verified,
  so this branch is not an address guess going wrong — it is the ASB address space failing mainline's
  own probe. STOP and report; do not run another rung.
- **Either bridge reads `REQ_STOP=1` or `ACK=1`** ⇒ the campaign's model of its own block is wrong
  and the render evidence has to be re-read. This is the only branch on which the write half is
  meaningful. Read it against `[v3d58] xengine` and the legacy-routing column *before* arming.
- **Bridges released, `MMUC_CONTROL.ENABLE` clear** ⇒ a genuine initialisation hole, and the first
  one this arc found. Fix, then re-measure; it is not a binner verdict.
- **Fabric clean *and* identity matched** ⇒ the **bridge/routing branch CLOSES** on a measurement
  rather than on inference, and PM/ASB power sequencing joins the excluded list alongside encoding,
  submission, cache mode, reset, MMU, clock, frame geometry and frame content. What is left of the
  wall is **`BXCF`, `MISCCFG.QRMAXCNT`, or deeper block init** — not this layer.

The `EMPTY`/`FULL` columns are a **two-sample read either side of a ~0.5 s wait, not a sampler**:
"EMPTY at both stations" is consistent with a bridge that was busy and drained, and is evidence of
nothing on its own. Only `REQ_STOP`/`ACK` carry a verdict, and the witness says so.

### 41.7 Gating and cost

One more rung on a deep boot (the `[v3d] deep=on` budget line now reads **18 rungs / ~9.5 s**) plus
seven MMIO reads per station. Verified by building `kernel8` three ways: **no deep** carries **zero**
`v3d69` strings; **deep** carries the read half only (11, and **zero** `[v3d69] reenable`); **deep +
`UNAOS_V3D69_REENABLE`** carries 16, including the 3 reenable lines — 1,856 bytes larger. QEMU
raspi4b models neither V3D nor the `rpivid_asb` block, so the probe returns at its hub-identity gate
with an explicit `SKIPPED` line; `kernel8-test` green for this arc (86/86) means the default boot is
byte-inert and the suite did not regress, **and nothing more**. Every verdict in §41 is
metal-attended.

### 41.8 The `pm` node, decoded from the shipping DTB (V3D-70)

§41 was written with no network, so its reg table was a reconstruction and its uncertain rows were
marked as such. The shipping device tree in this tree's own firmware payload
(`target/pi_baremetal/bcm2711-rpi-4-b.dtb`) settles them. `/soc/watchdog@7e100000`, `compatible =
"brcm,bcm2711-pm", "brcm,bcm2835-pm-wdt"`:

| `reg-names` | VC bus address | length | ARM PA (`0x7e…`→`0xFE…`) | this driver |
|---|---|---|---|---|
| `pm` | `0x7e100000` | `0x114` | `0xFE10_0000` | `PM_BASE` — **correct** |
| `asb` | `0x7e00a000` | `0x024` | `0xFE00_A000` | `LEGACY_ASB_BASE` — **correct** |
| `rpivid_asb` | `0x7ec11000` | `0x020` | `0xFEC1_1000` | `RPIVID_ASB_BASE` — **correct** |

Both bases the driver has used since PI-V3D-3 are DT-correct. The **lengths** carry the finding.
Mainline's `bcm2835-power.c` reads its identity word `ASB_AXI_BRDG_ID` at offset **`0x20`** and
expects the ASCII tag `'brdg'` (`0x62726467`). That offset is *inside* the legacy range (`0x24`
long) and *outside* the `rpivid_asb` range (`0x20` long). Therefore:

- the legacy block has an in-range identity register, and it is the **only** ASB identity check the
  DT covers — a match validates the `pm`-node address space both bases are derived from;
- the `rpivid_asb` block has **no** identity register in its DT range at all.

What `[v3d69]` printed as `rpivid_asb BRDG_VERSION` was offset `+0x00` of that block, which is not
an identity register in any range. Its `0x00000000` was therefore **not evidence of a dead window**,
and the "establish the base from the DT" branch it would have triggered was chasing a decode
artifact, not a fault. The column is retired; the honest "no identity register in this DT range"
note replaces it, and the legacy `+0x20` read is the identity check.

With identity set aside, the P74 wire already read shows every fabric column healthy: `PM_GRAFX`
`V3DRSTN = 1`; both `rpivid_asb` `M_CTRL`/`S_CTRL` moving `0x7 → 0x4` across our stop/release, i.e.
a **real** bridge acknowledging (request-stop set ⇒ `ACK` set, cleared ⇒ `ACK` clear); `MMUC.ENABLE`
latched. So the bridge hypothesis is **excluded by read**, the release writes have been correct
since PI-V3D-3, and the **bridge/routing branch of §41 closes**. The wall's remaining candidates are
`BXCF`, `MISCCFG.QRMAXCNT`, or deeper block init.

Unchanged by the DTB: the **generic PM word bit map** (step 8) stays **UNCERTAIN**. The device tree
gives addresses and lengths, not bit meanings, so the `[v3d70]` PM line keeps the raw word first and
its decode explicitly hedged.

## 42. The piOS dump-diff — both named suspects excluded measured; the geometry rung and the in-flight fabric read (V3D-71)

V3D-68 ended with two named suspects (`BXCF`, `MISCCFG.QRMAXCNT`) plus "an initialisation this
campaign has not found"; V3D-69/70 closed the fabric branch by read. V3D-71 starts from the first
**ground-truth register images of a binner that works**: two attended `devmem` dumps taken on this
same Pi 4 under working mainline (v3d+vc4 loaded) — `v3d-dump-idle` and `v3d-dump-mid-render`
(bench capture, 2026-07-29, `glxgears -fullscreen` for the mid-render image).

### 42.1 The dump-diff table's conclusion

Diffing the dumps against our side settles the V3D-68 suspects **by measurement, not argument**:

| Suspect | Working mainline reads | Ours reads | Verdict |
|---|---|---|---|
| `PTB BXCF` | `0x00000000` (idle **and** mid-render) | `0x00000000` | **EXCLUDED MEASURED** — a binner that closes frames runs with the identical word; do not re-litigate |
| `MISCCFG` (`QRMAXCNT`) | `0x00000006` (both dumps) | `0x00000006` | **EXCLUDED MEASURED** — ditto |
| `MMU_DEBUG_INFO` (hub `+0x1238`) | `0x00000550` (both dumps) | *was the table's one UNKNOWN-ours row* | closed by leg 3 below |

Frame shape, frame content (V3D-68) and fabric identity/state (V3D-69/70) were excluded prior. What
the diff then leaves as **PTB-visible** differences between the two systems is the **address
geometry of the bin frame**: mainline bins with its buffers at MMU iovas in the tens of MiB — pool
`CT0QMA=0x0129A000`/`0x09BDF000` with `CT0QMS≈0x88000` (~544 KiB), CL `CT0QBA=0x03033000`/
`0x0338A000`, tile-state `CT0QTS=0x01325002`/`0x0437F002` — while every rung this campaign ever
kicked handed it identity-mapped arena addresses below 4 MiB and a 32 KiB pool.

### 42.2 The three legs (one boot, all riding `UNAOS_V3D_DEEP`, no new knob)

**Leg 1 — `[v3d71] mainline-geom`, the shape-match rung.** Same physical arena, mainline-LIKE iovas:
the driver's own page table (grown to `PT_CAP=16384`, 64 MiB of iova — entries beyond the arena stay
published zeros, so the growth also removes the old past-table-end read) gains a window mapped only
for this rung: pool iova `0x03000000` sized at the dump's `0x88000` — **iova-aliased cyclically onto
the 32 KiB physical pool**, so any PTB write through any alias lands in poison — CL at `0x03089000`
(an unmapped guard page below it), the 24 KiB [v3d68] poisoned tile-state detector at `0x0308A000`.
One `[v3d48]`-style Empty kick (same 64x64 frame — the ONE variable is the addresses), kernel-exact
submit order, same FLDONE backstop; afterwards the window is unmapped so every later job runs on the
translation its verdicts were banked under. No off-mainline config write anywhere: the deltas are
PTEs in our table, the per-job `CT0Q*` latches, and mainline's own `v3d_mmu_flush_all` idiom.

**Leg 2 — `[v3d71f]`, the in-flight fabric sampler.** During that rung's ~0.5 s FLDONE wait,
`wait_fldone` folds one sample of `rpivid_asb` `M_CTRL` + `S_CTRL` (plus `PCS.BMACTIVE` as the
qualifier) at its ~1 ms cadence — armed only for this rung, zero MMIO on every other wait. The
witness prints min/max/distinct-values/count for both words.

**Leg 3 — `[v3d71] fabric MMU_DEBUG_INFO`.** One read of hub `+0x1238` folded into the `[v3d69]`
fabric stations, decoded per `v3d_drv.c` (`va_width = 30 + [7:4]`, `pa_width = 30 + [11:8]`) and
compared against mainline's `0x550` — the dump-diff table's one UNKNOWN-ours row.

### 42.3 Reading key

| Wire line | Reading | Convicts / excludes |
|---|---|---|
| `[v3d71] mainline-geom verdict — retired=1` or any poison touched | the same Empty frame that wedges at identity iovas produced a retire or PTB traffic at mainline-like iovas | **address-geometry/pool-size branch CONVICTED** — next arc adopts mainline's allocation geometry outright |
| `[v3d71] … fault latched=1` | the window translation refused an access (client + VA on the fault-latch line) | instrument fault — fix the map; **no** geometry verdict |
| `[v3d71] … retired=0, no fault, poison intact, drain completed` | still dead with every PTB-visible input mainline-shaped | the **last PTB-visible difference is EXCLUDED MEASURED**; the wall is not in the frame's inputs at all |
| `[v3d71f] … M_CTRL left 0x4` (any distinct word beyond quiescent, esp. `0x8000`) | beats ENTER the fabric while the frame is open | writes **ISSUE and die downstream** — fabric/routing re-opens, this time measured in-flight |
| `[v3d71f] … pinned at 0x4 across the wait with BMACTIVE=1` | mainline shows `0x8000` under live master writes; ours never leaves quiescent | the PTB **never issues a single beat** — core-internal verdict, upstream of the master bridge |
| `[v3d71f] … BMACTIVE never set / 0 samples` | sampler did not overlap an open frame | INCONCLUSIVE — no verdict |
| `[v3d71] fabric … MMU_DEBUG_INFO match=1` | capability word bit-identical to mainline | the UNKNOWN-ours row closes as excluded |
| `[v3d71] fabric … match=0` | hub-MMU capability divergence | report; re-read every `VIO_ADDR` decode against the printed `va_width` |

QEMU raspi4b models no V3D: the whole battery returns at the hub-identity gate with an explicit
`SKIPPED` line, so `kernel8-test` green means **no regression and nothing more** — every verdict in
this section is metal-attended, read at the next bench boot with `UNAOS_V3D_DEEP=1`.

## 43. Is the clock even ticking? — PCTR armed across the wedged bin (V3D-72)

The P82 wire established, and does not re-litigate: the PTB never issues one beat into the fabric
(`[v3d71f]`, 500 BMACTIVE=1 samples, M/S_CTRL pinned quiescent — the wall is **core-internal**,
upstream of the master bridge), and the frame stays dead under mainline address geometry with the
poison fully intact through a completed L2T drain (`[v3d71]`). Every fabric, routing and input
theory is closed. But the same wire named its own gap: **both** `[v3d55]` clkliv lines — on the
final `[v3d48]` Empty rung and on the `[v3d71]` mainline-geom rung — printed *"counter 2 was NOT
enabled across this wait — no clock verdict here"*. The CYCLE_COUNT battery had only ever been
armed around the PROBE bin, so the campaign has **never measured whether the core's clock domain is
advancing while BMACTIVE=1 on the wedged bin itself**. With everything else excluded, a gated
internal clock sub-domain (invisible in every config register, unlocked by some init mainline
performs and we lack) is the leading remaining theory — and the PCTR counter file is the one
instrument inside the core.

### 43.1 What V3D-72 arms (rides `UNAOS_V3D_DEEP`, no new knob)

A four-slot PCTR bank (`[v3d72] clkarm` / `[v3d72] clkliv-x`), armed with the exact
`v3d_perfmon_start` idiom this file already uses (EN=0 → SRC → read-back → CLR while stopped →
OVERFLOW clear → EN last; PCTR writes are counter-file config that mainline writes on every perfmon
start — no off-mainline register experimentation), across exactly two windows:

- the canonical `[v3d48]` **empty-frame** rung (arm → kick → read/stop after the FLDONE wait);
- the `[v3d71]` **mainline-geom** rung (armed last before the GO, read after the `[v3d71f]` emit).

Slots, every source id one this tree has already anchored (the PI-V3D-33 sourcing rule admits ids
only against the verified 16/32 anchors; no PTB-family id survives it — the same verdict V3D-63
recorded — and src35 was explored by v3d67 and is not revisited):

| Slot | Source | Discriminates |
|---|---|---|
| 0 | src14 `QPU_ACTIVE_CYCLES_VERTEX_COORD_USER` | any vertex/coord shader execution during the wait |
| 1 | src16 `QPU_CYCLES_VALID_INSTR` | any QPU instruction issue at all |
| 2 | src32 `CYCLE_COUNT` | the core clock itself — the reserved `[v3d55]` slot (file law), so the pre-existing clkliv witness now prints `armed=1` with a real Δ on these two waits, with `wait_fldone` unchanged |
| 3 | src24 `TMU_TCACHE_ACCESS` | intra-core memory-side (TMU cache) traffic |

Counters are cleared while stopped at arm, so each read **is** its Δ across the window; overflow
bits count as movement (a counter that wrapped exactly to zero must not read "never moved"). The
bank is stopped (EN=0) at read, so every rung outside the two windows prints exactly as before.

### 43.2 Reading key

| `[v3d72] clkliv-x` pattern | Reading | Convicts / excludes |
|---|---|---|
| Δsrc32 `CYCLE_COUNT` = 0 (and no overflow) | the core's own cycle counter is **frozen** while the wedged bin holds BMACTIVE=1 | **THE WALL FOUND** — a gated clock sub-domain; mainline's missing init is a clock/power **unlock**, not a unit enable. Must agree with the same window's `[v3d55]` clkliv (same slot 2, ~1 ms cadence) |
| Δsrc32 > 0, all unit slots (src14/16/24) = 0 | core clocked and idle: no QPU issue, no TMU-cache traffic, binner silent on an open frame | clock branch **EXONERATED** on the wedged bin itself, for every domain this counter file observes — the wall is a **unit-level enable/state** inside an always-clocked core |
| Δsrc32 > 0, any unit slot > 0 | the core is not merely clocked but **active** during a wait that never retired | unexpected — re-open the branch of whichever unit moved (QPU issue vs TMU cache) before any clock theory |
| `PCTR_EN(at read)` lost the 0x0F mask | something reprogrammed the counter file inside the window | every Δ INCONCLUSIVE, not zero — find the reprogrammer first |
| `[v3d55]` clkliv `armed=0` on either window | the arming did not span the wait | instrument regression — the whole point of this arc is that these two lines now read `armed=1` |

QEMU raspi4b models no V3D: both windows sit behind the hub-identity gate and the `UNAOS_V3D_DEEP`
feature, so `kernel8-test` green means **no regression and nothing more** — the Δ verdict is
metal-attended, read at the next bench boot with `UNAOS_V3D_DEEP=1`.

## 44. Does the CLE ever fetch? — the in-flight progress sampler and the submit-sequence diff (V3D-73)

The P83 wire established, and does not re-litigate: the core is **clocked and completely idle**
across the wedged wait (`[v3d72]` clkliv-x — CYCLE_COUNT advanced 703,077,589 cycles while src14 =
src16 = src24 = 0), the PTB never issues one beat into the fabric (`[v3d71f]` pinned 0x4), BMACTIVE=1
with no fault and the poison fully intact. The clock branch is dead; the wall is a **unit-level
enable/trigger** — "clocked but never TRIGGERED" — and the CLE→PTB frame-close path is the named
next place to look. V3D-73 asks the one question that splits that path in two: **does control-list
thread 0 ever fetch and advance through our bin list, or does it never start?** V3D-54 proved the
LATCH (CT0QBA/QEA hold exactly our list); it never proved a fetch.

The latent evidence that motivates the instrument: under the `[v3d71]` mainline-geom rung — the one
kick in the whole campaign whose QBA was **unique in iova space** — the `[v3d54]` trace printed
`CT0CA off 0xfd2c2000` against BA=0x03089000, i.e. raw CT0CA = 0x0034B000: the **previous**
identity-iova kick's list address. CT0CA never held the new QBA at all. Every earlier rung reused
the same CL address, so their `CT0CA == BA` readings could never distinguish "loaded QBA, never
advanced" from "stale word that happens to equal QBA" — and the `[v3d54]` offset arithmetic turned
the stale raw word into a misleading `(mid) stalled mid-list` verdict. The wedged `CT0PC=3` /
`CT0LC=0x30000` readings are the same class of stale hold.

### 44.1 Leg 1 — `[v3d73]` cle-progress, the in-flight fetch sampler

The `[v3d71f]` idiom exactly: a static accumulator armed only around the rungs that want it;
`wait_fldone` folds one sample per ~1 ms tick when armed (one static bool read, zero MMIO, on every
other wait). Sampled: **CT0CA** (raw word — min/max/distinct capped at 4 with overflow reported,
plus three derived counters: samples with CA inside [QBA,QEA], samples with CA == QBA, and the
highest in-span CA seen vs QEA), **CT0CS** (raw distinct words; only CTRUN(bit5) decoded, per the
§32/§40 CTnCS hedge), **CT0LC**, **CT0PC**, **BFC**. All five are seeded at arm time (pre-GO), so
`first` is the before-kick word and a stale hold reads as `changes=0` with the stale address
printed. Armed across two waits: the `[v3d71]` mainline-geom rung (unique-QBA, the unambiguous
read) and the `[v3d73s]` leg-3 rung below.

### 44.2 Leg 2 — the submit-sequence diff vs mainline

Compiled from the mainline facts this tree already records (the `v3d_bin_job_run` order at the
CT0Q* facts block in `v3d.rs`, the `[v3d59]` mainline ledger T1–T4, `v3d_invalidate_caches` at
`bin_prejob_invalidate_kernel_exact`, `v3d_irq_enable`/`v3d_mmu_set_page_table` sourcing) and
against the attended piOS dumps — no external source fetched.

| # | Mainline `v3d_bin_job_run` write (order) | Ours | Divergence |
|---|---|---|---|
| 1 | `V3D_PTB_BPOS = 0` — FIRST write of every bin job (BPOA untouched) | `bin_prejob_bpos_clear`, same position since V3D-57 | none |
| 2 | `v3d_invalidate_caches`: L3/L2C (no-ops on 4.2) → `SLCACTL` all → `L2TFLSTA=0`/`L2TFLEND=~0` → `L2TCACTL = L2TFLS\|FLM=CLEAR` | all rungs but `[v3d53]` run `FLM=FLUSH`, L2T-first | **(c) mode/order** — individually controlled by the v3d51-vs-v3d53 differential (both wedge) |
| 3 | `v3d_switch_perfmon` — no-op when no perfmon attached (the common case) | `[v3d63]/[v3d72]` PCTR arming occupies the same slot on armed rungs | none (same idiom) |
| 4 | `CT0QMA` then `CT0QMS` (guarded by `job->qma`; always set for bin jobs) | same order, always written | none |
| 5 | `CT0QTS = ENABLE(bit1) \| qts` (guarded by `job->qts`) | same value, same position | none |
| — | *(nothing)* | **echo READS of CT0QTS/QMS/QMA** (`[v3d49]`/`[v3d71]` frame-enables witness) | **(b) interleaved reads** inside the latch block |
| 6 | `CT0QBA` | same | none |
| — | *(nothing — the ISR does all W1C)* | **`INT_CLR` W1C write between CT0QBA and CT0QEA** on every kick | **(a) interleaved write inside the queue pair** |
| 7 | `CT0QEA` — **the QEA write IS the GO** (queue-register auto-start; no CTnCS write anywhere on the mainline submit path) | same | none |

**Verdict of the diff: no register mainline writes on the bin submit path that we do not.** The
config space was exhausted measured at P83; rows (a)/(b)/(c) show the **sequence** was not — no kick
in this campaign has ever been `v3d_bin_job_run` verbatim with nothing interleaved. Each divergence
is individually benign-looking, and (c) was individually controlled, but the conjunction never ran.

### 44.3 Leg 3 — `[v3d73s]`, the mainline-exact submit (rides `UNAOS_V3D_DEEP`, no new knob)

One more Empty rung at identity addresses (after `[v3d71]` unmaps its window), whose submit is the
byte-exact transcription: stale-latch W1C and all instrument reads BEFORE the sequence, then
`BPOS=0` → kernel-exact `FLM=CLEAR` invalidate → `CT0QMA → CT0QMS → CT0QTS|ENABLE → CT0QBA →
CT0QEA` as five consecutive uninterrupted writes, one `dsb` after the GO. Same poison detectors
(24 KiB tile-state + 32 KiB pool), same FLDONE backstop, the `[v3d73]` sampler across its wait.
Within license: every write is one mainline makes on this path, in mainline's order — this leg
**removes** off-mainline instrument traffic rather than adding an experiment.

### 44.4 Reading key

| Wire line | Reading | Convicts / excludes |
|---|---|---|
| `[v3d73] … in-span=0` (CA never inside [QBA,QEA]; stale pre-kick word printed) | **the CLE never fetches its first word** — the queue latched but thread 0 never consumed it | the trigger is **upstream of list execution entirely** (a thread-start condition); list content, addresses and terminator semantics are all unreachable and therefore exonerated for this wedge |
| `[v3d73] … CA pinned at QBA` (in-span>0, max-in-span==QBA) | the queue loaded but the first word never fetched/advanced | same verdict, one station later: the trigger dies at thread start / first fetch |
| `[v3d73] … max-in-span==QEA` | the list **executes** — with the PTB silent (`[v3d71f]`) and no FLDONE | the FLUSH terminator fails to trigger the PTB frame-close: the wall moves **into the terminator/frame-close semantics** |
| `[v3d73] … advanced then stalled mid-span` | the CLE fetches but chokes at the printed offset | localise the packet at that byte |
| `[v3d73] … BFC Δ>0` | a frame closed under the sampler | read the leg's retire witness — the wedge did not reproduce |
| `[v3d73s] verdict — retired=1` or poison touched | the uninterrupted sequence retired what the instrumented one wedged | **the submit sequence is convicted** — our own interleaved MMIO was breaking the trigger; adopt the uninterrupted sequence as the only submit path |
| `[v3d73s] … STILL DEAD` | no retire, no fault, poison intact through a completed drain | the **submit sequence is exhausted alongside the config space**: no write mainline makes that we lack, no order it keeps that we broke — the trigger hunt moves fully upstream of the submit interface |
| `[v3d73] … samples=0` | the wait exited before the first tick | INCONCLUSIVE — no verdict |

QEMU raspi4b models no V3D: both legs sit behind the hub-identity gate and `UNAOS_V3D_DEEP`, so
`kernel8-test` green means **no regression and nothing more** — every verdict here is
metal-attended, read at the next bench boot with `UNAOS_V3D_DEEP=1`.

> **NOTE (2026-08-01, V3D-82).** The `[v3d54]` trace itself now applies this section's lesson
> instead of merely being corrected by it. Its CT0CA line prints the **raw** first/last words
> alongside the offsets (`CT0CA raw 0x…->0x… off 0x…->0x…`), and its interpretation arm checks the
> raw word against the raw `[BA, EA]` span before naming a stall: a raw CA **outside** the span is
> classified `(stale)` — a stale pre-kick address, the CLE never demonstrably entered this list,
> with `[v3d73]` named as the deciding instrument — while only a raw CA genuinely inside the span
> keeps the `(mid)` "advanced then stalled mid-list" reading. No future reader has to undo the
> wrapping arithmetic by hand, and the P83-class artifact ("stalled at offset 0x2d000" on a
> 106-byte list) can no longer print as a verdict. In the same change, the shared `v3d75_kick_probe`
> — the kick every deep leg after V3D-74 rides (`[v3d75a]`/`[v3d75b]`, `[v3d77a]`/`[v3d77b]`,
> `[v3d80 post-handover]`, `[v3d81q]`, and `[v3d81d post-handover]` when that leg is armed — every
> caller of the shared probe) — now **emits** the `[v3d73]` witness it was already arming (wait → emit →
> read-span, the `[v3d74a]` idiom), so the sampler rides every CT0-kicking deep leg and the fetch
> verdict on those legs is measured, not extrapolated from the three originally-armed rungs.

## 45. Thread 0 or bin class? — the thread-swap discriminator (V3D-74)

> **ANSWERED — the answer is CLASS. §45's leg A is STANDING (reinterpreted).** Hypothesis 2 below
> ("thread 0 is dead as a thread") is **dead**: `empty` and `m4` both execute on CT0 (§49.8). Leg A's
> frozen RCL was the first sighting of what §49.11 proved — CT0 refuses **render-class** opcodes.
> Leg B is STANDING. See [**THE CT0/CT1 CLASS
> LAW**](#the-ct0ct1-class-law--render-lists-run-on-ct1-never-on-ct0) and the ledger in §49.11a.

The P84 wire established, and this arc does not re-litigate: `[v3d73s]` = the byte-exact mainline
submit sequence changes **nothing** — the submit interface is exhausted alongside the config space.
`[v3d73]` cle-progress = CT0's queue **loads** (CA holds our list, CTRUN=1) but the CLE **never
fetches the first word** — 500 samples, zero movement, the core clocked (703M cycles over the wait)
and idle. Meanwhile **thread 1 works** on the same CLE hardware: CT1/RCL jobs fetch, execute,
retire and land byte-verified pixels (`[v3d58]` xengine, banked since M3). Same silicon, two
threads: one alive, one loaded-but-never-fetches.

Two hypotheses fit, and they send the hunt in opposite directions:

1. **Bin-class lists are dead as a class** — something about a bin submission upstream of the
   first fetch (the CT0QMA/QMS/QTS tile-memory arming state, or bin-mode selection) kills the
   fetch before it starts. Then thread 0 *can* fetch, and the next instrument targets whatever
   distinguishes a bin submission before its first word.
2. **Thread 0 is dead as a thread** — it never starts regardless of list class: a per-thread
   enable/power state no register this campaign knows controls. Then the hunt leaves the core's
   programming interface entirely; firmware/VPU-side per-thread init becomes the leading space.

### 45.1 Leg A — `[v3d74a]` rcl-on-ct0, the swap

Take the minimal render list thread 1 provably executes — the M3 clear-job RCL, the smallest
retiring rung in the file, byte-verified on metal since M3 — and submit it on **CT0's** queue
(CT0QBA/CT0QEA), otherwise byte-identical to the retiring CT1 submit: QBA, `dsb`, QEA, `dsb`,
and **nothing else**. Deliberately no QMA/QMS/QTS arming — that is exactly the bin-class state
under suspicion, being removed. The `[v3d73]` sampler is armed across the wait; the FLDONE wait
supplies the ~1 ms tick window (an executing RCL retires with **FRDONE**, not FLDONE, so the wait
always runs its full ~0.5 s backstop and `FRDONE=1` on the `[v3d44]` timeout line is a *positive*
reading on this leg). Same `[v3d68]` poison detectors (24 KiB tile-state + 32 KiB pool, drained
and scanned), plus the clear-job store verify (sentinel-seeded target vs `CLEAR_RGBA`) as the
strongest possible positive. A bonus of the swap: the RCL's address was never latched into CT0Q*
by any prior kick, so a `CA==QBA` reading on this leg is unambiguous — the `[v3d73]` stale-word
trap cannot reproduce here.

Placement: last in the deep ladder, after `[v3d73s]` — leg A latches an RCL-class queue into
CT0Q*, so every bin-class reading must already be banked before it runs.

### 45.2 Leg B — `[v3d74b]` bcl-on-ct1: a documented no-op, on the tree's own register map

The mirror leg — the Empty bin list on CT1's queue with the bin-side memory latches armed as
`[v3d71]` does — **cannot be expressed**: the `v3d_regs.h` map this driver transcribed shows the
bin tile-memory latches are thread-0-only. CT0QTS (0x15c), CT0QMA (0x170) and CT0QMS (0x174)
have **no CT1 counterparts** — the adjacent slots are CT1SYNC (0x158) and CT1QCFG (0x178).
Thread 1 has no PTB tile-memory plumbing to arm, so "the bin-side arming, mirrored" is not a
register sequence that exists; and a bin list reaching `START_TILE_BINNING` on a thread with no
PTB access risks wedging CT1 — the only thread this campaign has ever seen retire, and the
working reference every cross-engine verdict (`[v3d58]` xengine, `[v3d64]`/`[v3d66]` render legs,
`[v3d58]` rerender) rests on. The discriminator is therefore one-sided by hardware design:
`[v3d74b]` emits the rationale on the wire and kicks nothing; leg A carries the verdict alone.

### 45.3 Reading key

| Wire line | Reading | Consequence |
|---|---|---|
| `[v3d74a] … store-verified=1` (or `FRDONE=1` / `RFC Δ>0` / sampler `max-in-span==QEA`) | **thread 0 can fetch and execute** — the render-class list ran on CT0 the moment the bin-class arming was absent | the death is **bin-job-configuration-specific**, upstream of the first fetch; next instrument: the QMA/QMS/QTS arming-state permutation ladder |
| `[v3d74a] … advanced then stalled mid-span` | thread 0 **fetches** an RCL-class list, then chokes mid-list (an RCL without render-side plumbing may legitimately choke past the fetch) | the class discrimination stands — thread 0 starts when the submission is not bin-armed; the choke offset is a separate localisable fact |
| `[v3d74a] … CA loaded QBA, never advanced` (unambiguous here — fresh address) | the same loaded-but-never-fetches signature as the bin class, now on a render-class list | **thread 0 itself never starts regardless of list class** — a per-thread enable/power state outside the known register space; the hunt leaves the core's programming interface (firmware/VPU-side per-thread init) |
| `[v3d74a] … CA never held QBA` | thread 0 did not even load the swapped queue | same verdict, one station earlier |
| `[v3d74a] … samples=0` or MMU fault latched | instrument failure | INCONCLUSIVE — no verdict; fix before citing |

### 45.4 The S1S bench ask — the piOS dump taken MID-BIN

If leg A reads *thread 0 never starts*, the next discriminator is not a register this driver can
reach: it is a **piOS register dump captured mid-bin** — the same dump rig as V3D-71's, but
triggered while a mainline bin job is in flight (between `v3d_bin_job_run`'s QEA write and its
FLDONE), so the capture shows the CLE/PTB/hub state of a thread 0 that *is* fetching. Diffing
that against our wedged mid-bin state is the first look at whatever per-thread condition the
firmware/VPU establishes and we do not. This is the standing bench ask for the S1S sitting.

QEMU raspi4b models no V3D: `[v3d74]` sits behind the hub-identity gate and `UNAOS_V3D_DEEP`, so
`kernel8-test` green means **no regression and nothing more** — the verdict is metal-attended,
read at the next bench boot with `UNAOS_V3D_DEEP=1`.

## 46. The S1S firmware-init campaign — every register divergence explained (V3D-75…78)

> **⚠ PARTIALLY RETRACTED-BY-CLASS-LAW (§49.11a).** Every *kick* verdict in this section
> (`[v3d75a]`, `[v3d75b]`, `[v3d77a]`, `[v3d77b]`, and §46.5's `[v3d80]`) was read through
> `v3d75_kick_probe`, which submits a **render-class control list to CT0** and therefore returns
> `false` unconditionally — for a reason unrelated to the experiment under test. Those negatives are
> **RETRACTED**; the hypotheses they were meant to kill are returned to **untested**, not revived.
> §46.5's claim that *"the ARM-testable universe is CLOSED"* **falls with them.** The **register
> readbacks** in this section (`M_CTRL` did not hold; core0+0x68 and hub+0x68 are read-only; the
> sweep diff) are direct register facts and **STAND**.

The §45.4 bench ask was answered the same sitting (2026-07-30): the dump rig grew a
`--trigger ct0run` mode (in-process mmap spin on `CT0CS.CTRUN`, instant snapshot + 1 ms settle)
and caught a genuine mid-bin capture (in-span=True; settle proves it: `CT0CS 0x20→0x00`,
`BFC 0xE4→0xE5` across the millisecond). Four rungs and one condition boot then closed the
entire register-visible search space.

### 46.1 The mid-bin diff and its three divergences

Against every wedged UnaOS reading: `RPIVID_ASB_V3D_M_CTRL` 0x4040 vs our 0x4; the legacy ASB
bridges parked (0x5) on both sides; wedged `CT0CS` 0x70 (bits 4+6 above CTRUN) vs 0x20 while
genuinely fetching. A full-window sweep was added on both sides (`[v3d76]` in the DEEP battery;
`--sweep` in the dump script — same `SWEEP phys= val=` line format, `diff` is the instrument).
The sweep reduced the structural divergences to exactly two UNNAMED words, absent from mainline
`v3d_regs.h`: core0+0x68 (piOS 0x3, ours 0x10001) and hub+0x68 (piOS 0x2, ours 0).

> **The `0x4040` datum is reconciled in §49.20.4(a), and it is not revived.** §49.19's later piOS
> mid-bin capture reads `RPIVID_ASB_V3D_M_CTRL` as `0x8060`, not `0x4040` — a different capture,
> sixteen days and one piOS kernel apart, of the field §46.2's idle-dump row had already classified
> as **bridge activity status**. Four distinct piOS values are now on record (`0x4040`,
> `0x4050→0x4060` cycling live, `0x8060`, `0x8050`) against one piOS *idle* value, `0x4` — which is
> ours in every state we have read. boot4/PA40 (§49.20.2 V6) supplies our side of this window
> directly for the first time: `0x7` at cold entry, `0x00000004` released. **The low nibble is the
> configuration and it matches; the upper bits are traffic, and we generate none** — §49.20.2 V4
> shows the PTB's write never completes as a bus transaction, so the field is an *effect* of the
> wall, downstream of it, and cannot be its cause.

### 46.2 The verdicts, in order (each read on metal)

| Rung / boot | Experiment | Verdict |
|---|---|---|
| `[v3d75a]` P86 | mailbox `SET_ENABLE_QPU(1)` (0x00030012) | MAILBOX FAILED — tag unhandled by this firmware; route closed |
| `[v3d75b]` P86 | `M_CTRL = pw\|0x4040` transplant | did NOT hold — VPU-owned bits (mid-bin2 later showed them cycling 0x4050→0x4060 live) |
| `[v3d77a]` P88 | core0+0x68 ← 0x3 | readback unchanged — READ-ONLY status word |
| `[v3d77b]` P88 | hub+0x68 ← 0x2 | readback unchanged — READ-ONLY |
| P89-KMSCOND | byte-identical kernel + `dtoverlay=vc4-kms-v3d` + dtbos in the FAT | NOTHING changes at handoff: entry bridges still 0x7, core+0x68 still 0x10001, `[v3d74a]` still never-starts — start4.elf does not establish the condition, overlay or not |
| idle-dump recheck | V3D-71-era `v3d-dump-idle.txt` | **idle piOS `M_CTRL=0x4` — OUR value.** The high bits are bridge ACTIVITY STATUS (an effect of AXI traffic), not an enabling condition |

### 46.3 Where that leaves the wall

Every ARM-visible register divergence between the wedged and the working system is now
explained as job-state, free-running counters, VPU-owned live status, or read-only status.
core0+0x68 bit16 (0x10001 wedged vs 0x3 fetching) remains valuable as the wedge's visible
SIGNATURE — a diagnosis window, not a knob. The instrument-lie ledger gains n+1: a fabric
status word read mid-render, mistaken for init state.

The remaining truth channels are outside the register file: the VPU firmware's own V3D state
(not ARM-visible), and whatever the Linux driver chain requests THROUGH the firmware at probe
time that our bringup does not (clock/reset/power sequencing differences — a source-audit of
raspberrypi-power/raspberrypi-clk/v3d-probe on BCM2711 is the next instrument, before any
further boot is spent).

### 46.4 V3D-79 — the genpd-faithful minimal bringup (P90, read on metal)

The source audit (rpi-6.6.y: `v3d_drv.c`, `v3d_gem.c`, `bcm2835-power.c`, `clk-bcm2835.c`,
`bcm2711.dtsi`) established that piOS never resets V3D at boot — `v3d_reset()` is the hang path;
boot-time bring-up is genpd `bcm2835_asb_power_on` alone (deassert `PM_V3DRSTN`, release the two
rpivid bridges; `pd->clk` resolves NULL on 2711 so the clock choreography no-ops; the v3d node
has no clock of its own). `UNAOS_V3D79_MINIMAL` reproduces exactly that and nothing else.

**P90 verdict: STILL DEAD.** BLOCK-UP without any mailbox call (the block decodes on bridge
release alone), and `[v3d74a]` reads the identical never-starts signature. Every ARM-side act,
order and register the working system touches is now matched or removed — **the ARM-side
divergence space is EMPTY.** Two riders: (a) on the MINIMAL boot the firmware *reports* the V3D
clock gated at 250 MHz while CYCLE_COUNT free-runs at ~499 MHz — the firmware clock-state report
is not trustworthy (instrument-lie ledger +1); (b) our mailbox power/rate/gate calls are
therefore neither harmful nor sufficient — cosmetic to the wedge.

**Remaining space, now exclusive: the environment the FIRMWARE boots under.** piOS's start4.elf
/fixup4.dat (its own build) + its config.txt lines vs our pinned 1.20260521 set with a bare
config. The next discriminator is P91-ENVSWAP: OUR kernel + the piOS card's exact firmware
files and config lines (minus kernel=/os selection). Retire there → bisect the environment
(firmware build first, then config lines). Still-dead there closes the ARM-testable universe;
what remains is VPU-internal state requiring firmware-side instrumentation.

### 46.5 V3D-80 — the display handover, and the reply-less-notify discovery (P92/P93)

vc4's probe performs ONE mailbox act no UnaOS boot ever has: `NOTIFY_DISPLAY_DONE` (0x00030066,
zero-length) — the firmware display driver stops and the ARM owns the display/GPU complex.
P92 sent it with the default 500 ms reply budget: timeout. P93 with 5 s: timeout again — and the
same capture shows `NOTIFY_XHCI_RESET` (0x00030058) followed by mailbox timeouts too, *while the
VL805 firmware load it requests demonstrably works*. Reading: **on this firmware
(hash 3484b5dd…, = piOS's own), NOTIFY-class tags are acted on without a FIFO reply** — a
mailbox-protocol instrument lie (ledger +1): "MAILBOX FAILED" on a notify tag means only
"no reply", not "no effect". It also reopens [v3d75a]'s ENABLE_QPU verdict from "tag unhandled"
to "possibly acted, reply-less".

Discriminator for whether P93's handover took effect: the PANEL. The [v3d80] rung runs ~15 s
into the battery; a display that goes black there = the firmware display driver stopped = the
handover HAPPENED and the kick's DEAD read refutes the hypothesis for real. A display that
survives = the tag was ignored and the hypothesis stands untested. (Attended read — recorded
at the bench.)

> **Superseded by §47 — do not stop here.** The panel was the only discriminator available at
> P93 and it is a poor one: attended, single-shot, and unable to separate "nothing happened" from
> "something happened where nobody was looking". §47 (V3D-81) replaces it with three machine-read
> channels and a positive control, and it is where the handover verdict now comes from. The
> protocol fact above stands unchanged; only the *reading method* is superseded. Note also that
> `mbox_call_budget`'s 5 s budget was introduced on the belief that the tag replies slowly — that
> belief is what this section refutes, and no budget can repair it.

## 47. The reply-less NOTIFY, read by effect (V3D-81)

> **⚠ THE `kick=` COLUMN IS RETRACTED-BY-CLASS-LAW (§49.11a); THE REST STANDS IN FULL.** Every leg's
> `kick=` value came from `v3d75_kick_probe` — a **render-class list on CT0**, which cannot pass. Read
> `kick=0` on any `[v3d81*]` line as **no information**. Everything else in this section is
> untouched: §47.1a's **control-INDEPENDENT** evidence — `acted-on-buffer`, `buffer-rejected`,
> `doorbell`, `reg-moved`, `fw-moved` and the display-liveness leg — never consulted the kick. §47.1a
> drew exactly the distinction that saves this section, which is the strongest argument in the file
> for making it.

§46.5 closed the ARM-testable space and left exactly one thread live, and it is a thread about
METHOD rather than about hardware. On this firmware a NOTIFY-class tag is acted on without a FIFO
doorbell. `mbox_call` returns only when a doorbell arrives. Every verdict this campaign read
through it for such a tag was therefore a statement about the doorbell and not about the tag —
including the two that closed routes: `NOTIFY_DISPLAY_DONE` (P92 at 500 ms, P93 at 5 s, both
"MAILBOX FAILED") and `SET_ENABLE_QPU` (P86, recorded as "tag unhandled by this firmware"). Neither
was refuted. Both were mis-read.

V3D-81 sends them properly: post the tag, wait for no reply, settle a stated interval, then read
the EFFECT.

### 47.1 Why panel survival was never enough

P93's only discriminator was the bench panel: black = the display driver stopped = the handover
happened = a dead kick refutes the hypothesis; alive = the tag was ignored = the hypothesis stands
untested. That reading is correct and nearly useless. It is a null result, it needs an attended
human at the exact moment, and it cannot separate "nothing happened" from "something happened
somewhere nobody was looking". The instrument must decide **acted vs ignored** by itself, on the
wire, or a boot spent on it is a boot spent on a coin flip.

Three channels do that, and all three are independent of the doorbell:

1. **The reply buffer.** The VPU writes its response into the request buffer by DMA; the doorbell
   is a separate mechanism. `overall = 0x8000_0000`, or the tag's own request word carrying bit 31,
   means the message was parsed and the tag HANDLED — acted, whatever the effect. P92/P93 posted
   that buffer and abandoned it when the wait timed out; nobody has ever read those words for
   either tag. (PIUSB-12 named this word as the discriminating one for the doorbell-bearing case;
   V3D-81 is that reading carried across to the doorbell-less one.)
2. **The register station**, pre and post, bracketing only the send: core0+0x68 (the wedge
   signature, §46.3), hub+0x68, both RPIVID ASB bridge words, CT0CS. Any movement = the VPU reacted.
3. **The firmware's own state**, asked before and after: `GET_CLOCK_RATE`/`GET_CLOCK_STATE` for V3D
   and `GET_PHYS_WH` for the display. The last is the panel observation made in software — a
   firmware that stopped its display driver should stop answering (or answer differently) about
   DISPLAY geometry while still answering about CLOCKS. That asymmetry is the handover, machine-read.

And the control that makes a null reading mean anything: `GET_CLOCK_RATE` after the send, on a tag
unrelated to either hypothesis. If it answers, the transport and the cache invalidate were both
alive when the buffer was read, so an untouched buffer is the firmware's silence rather than our
blindness. If it does not answer, the leg reports INCONCLUSIVE instead of inventing a verdict.

`query_display_size_raw` and `get_clock_rate_raw` exist for the same reason: the ordinary wrappers
fold "the firmware reports no display mode" and "0 Hz" into `None` alongside a transport failure,
and those are precisely the answers a stopped driver would give.

### 47.1a Control-independent vs control-dependent evidence

Of the three channels, two are read WITHOUT the mailbox and one is read THROUGH it, and the verdict
ladder must not treat them alike. The reply buffer and the register station are direct reads: a
stamped response word cannot be manufactured by our own posted values, and an MMIO register diff
needs no VideoCore cooperation to be believed. The firmware's own state is three `mbox_call`s.

The failure that follows is the one this section exists to name. **If the transport dies anywhere
inside a leg, all three firmware fields go `Some(x)` → `None` at once.** That is maximal apparent
"movement", produced entirely by the instrument breaking, and it appears on precisely the boot where
the firmware can no longer be asked anything. Folded into a single `station-moved` flag it prints
ACTED, EFFECT ELSEWHERE — *a refutation* — with `control-alive=0` sitting unread in the same line,
and it takes the campaign's last open tag off the list on the strength of a dead mailbox. The
INCONCLUSIVE arm, meanwhile, becomes unreachable in the one state it was written for.

So the wire carries `reg-moved` and `fw-moved` separately, and the ladder is ordered:

1. `posted=0` or `buffer-rejected=1` → **instrument failure, no verdict.**
2. `kick=1` → **the wall was this tag.**
3. `acted-on-buffer=1` or `reg-moved=1` → **ACTED, EFFECT ELSEWHERE.** Control-independent; stands
   whatever the mailbox is doing.
4. `fw-moved=1` **and** `control-alive=1` → **ACTED, EFFECT ELSEWHERE (firmware state).** The weaker
   channel, admitted only with the reader proven alive; cite which channel carried it.
5. `control-alive=0` → **INCONCLUSIVE.** Reached whenever the control is dead and no
   control-independent channel moved — including, deliberately, the case where `fw-moved=1` because
   the transport died.
6. otherwise → **IGNORED.**

This is the §46.5 lesson applied to the instrument itself: an instrument's reading is evidence only
where the instrument can actually run, and an arm that reports "the mailbox failed" must not be
outvoted by the movement that mailbox failure invents.

### 47.2 Reading key

| Wire reading | Verdict |
|---|---|
| `send … doorbells=N>0` | the tag is **not reply-less** at this settle — §46.5's protocol reading does not cover it, `mbox_call` can carry it, and P92/P93's timeouts want re-explaining |
| `send … MESSAGE REJECTED` (`overall=0x80000001`) | the VPU parsed the buffer and threw it out as malformed; the tag was never reached. A write that means the opposite of acted — **instrument fault, no verdict** |
| `send … REPLY-LESS AND ACTED` (`overall`/`tag_code` stamped) | the message was parsed and the tag HANDLED with no doorbell — the fact P92/P93 could not see |
| `verdict … kick=1` | **the wall was this tag**: thread 0 starts once it is sent reply-less. Productionize the send in bringup before first submit |
| `verdict … kick=0 acted-on-buffer=1` or `reg-moved=1` | **ACTED, EFFECT ELSEWHERE** — read without the mailbox, so it holds regardless of `control-alive`. A real refutation; the tag comes off the open list |
| `verdict … kick=0 fw-moved=1 control-alive=1`, `reg-moved=0`, `acted-on-buffer=0` | **ACTED, EFFECT ELSEWHERE (firmware state)** — a refutation on the weaker channel; name the channel when citing it |
| `verdict … control-alive=0` (`acted-on-buffer=0`, `reg-moved=0`) | **INCONCLUSIVE — no verdict, whatever `fw-moved` says**, because a dead transport drives all three firmware fields to `None` and is indistinguishable from the firmware changing its mind. On the display leg a control that dies is *candidate* evidence the display driver stopped — candidate, not proof |
| `verdict … kick=0`, everything else 0, `control-alive=1` | **IGNORED** — the reader was alive and the firmware left no trace: the tag does nothing an ARM can see and the hypothesis stays UNTESTED (P93's position, now stated from evidence) |
| `station … fifo-drained-before-queries=N>0` | a doorbell arrived late and was swept here. On a POST station: the tag replied after its settle expired, so it is not reply-less at this budget |
| `[v3d81] battery done … N stale word(s) discarded`, N>0 | same tell for a reply landing after the LAST station; also the reason no later `mbox_call` inherits it and prints a spurious MAILBOX FAILED |
| `display-liveness … display query moved while the clock query still answers` | the firmware DISPLAY driver specifically stopped — the handover happened, no panel needed |

### 47.3 Arming, and the one-send rule

`UNAOS_V3D81_QPU` and `UNAOS_V3D81_DISPLAY` arm the legs separately, because they are independent
hypotheses and a boot that sends both can attribute a change to neither. Each implies
`UNAOS_V3D_DEEP` (the legs ride the tail of the deep battery, where every reading they are compared
against is taken). `UNAOS_V3D81_SETTLE_MS` sets the settle (default 250 ms, capped at 10 s).

Arming a leg **stands down the doorbell-waiting send of the same tag above** — `[v3d75a]` for
ENABLE_QPU — and says so on the wire. A tag sent twice makes the reply-less rung's pre-station a
lie, and it also turns the stood-down rung into something better: `[v3d75a]`'s station and kick
become the same-boot PRE-SEND control for `[v3d81q]`.

`[v3d80]` stands down more broadly: **any** armed leg suppresses it, not just `UNAOS_V3D81_DISPLAY`.
Per-tag would not be enough. `[v3d80]` is the last rung before the battery, and by §46.5's own
reading its `NOTIFY_DISPLAY_DONE` may be honoured reply-lessly — with the effect landing anywhere
inside the 5 s the doomed doorbell wait burns, or after it. The fields a stopping display driver
degrades (`GET_PHYS_WH`, and the `GET_CLOCK_RATE` that serves as the control) are exactly the fields
`[v3d81q]`'s stations read, so a late-landing handover straddling `[v3d81q]`'s pre-station would be
attributed to `SET_ENABLE_QPU`. With only `UNAOS_V3D81_QPU` armed the handover tag is therefore not
sent at all on that boot, and the wire says so on both `[v3d80]` and `[v3d81d]`.

Since V3D75A-GATE and V3D80-GATE, arming a leg is **necessary but not sufficient**. Each *tag* now
carries its own arming switch as well — `UNAOS_V3D_QPU` for `SET_ENABLE_QPU`, `UNAOS_V3D_DISPDONE`
for `NOTIFY_DISPLAY_DONE` — both off by default (see [The knob](#the-knob)), because both sends cost
the boot something (the mailbox and therefore USB; the panel) and both verdicts are banked. A leg
armed with its tag gate off posts nothing, takes no station either side of the send it did not make,
and prints one `… SKIPPED — <tag>-gate off …` line that disclaims its own verdict explicitly.

Every `[v3d81]` line states the armed legs, the tag gates, the settle, the tag ids and both stations,
so a capture is self-describing and datable without the reader knowing how the image was built.

Standing caveats for whoever reads the next boot. The legs run after `[v3d75b]`'s M_CTRL transplant
and `[v3d77]`'s two pokes, so the fabric they act on carries that residue — both were readback-proven
not to hold (§46.2), and the pre-station prints the values, but the residue is real and named. And
if a tag replies LATER than the settle, its doorbell would be consumed by the next ordinary property
call as if it were that call's own reply. The FIFO is drained before every post and before every
station query, and **every drain prints its count** — `stale-pre` on the send line,
`fifo-drained-before-queries` on each station, and a final sweep at the tail of the battery so that
a reply landing after the last station is not left for an unrelated subsystem to consume and report
as its own MAILBOX FAILED. Whichever drain runs first is where a late doorbell shows up; a drain
that swallowed the count silently would have destroyed the only evidence it ever arrived.

QEMU raspi4b models no V3D, so `[v3d81]` sits behind the hub-identity gate: `kernel8-test` green
means no regression and nothing more. The verdict is metal-attended.

## 48. The CLE stall decoded — there is no packet at the stall offset (V3D-82 verdict, 2026-08-01)

The S1U opening arc: the PA4 `[v3d54]` line read `CT0CA off 0x2d000 (mid)` against the 106-byte
`[v3d74a]` RCL (`BA=0x0031e000 EA=0x0031e06a`) — "stalled mid-list, choked on the packet at that
offset". The arc's task was to decode the list byte-for-byte and name that packet. Both halves
are now closed, off the banked capture alone, with no boot spent.

### 48.1 The stall offset names no packet — it names the previous list's base address

Hand-verified arithmetic across every "STALLED mid-list" reading in the PA4 capture: all three
reported offsets decode to the SAME absolute CT0CA word.

| leg | BA | reported off | raw CT0CA |
|---|---|---|---|
| v3d63 (14 B @ 0x0034f000) | 0x0034f000 | 0xffffc000 | **0x0034b000** |
| v3d71 mainline-geom | 0x03089000 | 0xfd2c2000 | **0x0034b000** |
| v3d74a…v3d81q (106 B) | 0x0031e000 | 0x2d000 | **0x0034b000** |

0x2d000 would be byte 184,320 of a 106-byte list — impossible as progress, exact as one frozen
register. 0x0034b000 is the BA of every pre-wedge probe list; CT0CA moved genuinely in the
pre-wedge era (offsets 0x4c, 0xe walked with CTRUN=0), latched 0x0034b000 at the wedge onset
(the v3d53 rung, where CTRUN stuck 1), and has never changed since. The `(mid)` verdict was
`emit_v3d54_trace`'s fallthrough arm — `!(==BA) && !(==EA)` over a wrapping subtract, no span
check (the §44 P83 trap, reproduced verbatim on PA4).

Adversarial review closed the loopholes: a fetch-and-park between 1 ms samples is triply
incoherent (CA parks at EA on completion; CTRUN would clear, it is frozen 1 at CS=0x70
changes=0; BFC Δ0 and BPCA adv=0), and the sampler's own MMIO reads demonstrably execute in
the wedged state, so its zero is admissible.

**Direct measurement existed for every leg all along.** `v3d75_kick_probe` armed the `[v3d73]`
sampler on every funneled leg (v3d75a/b, v3d77a/b, v3d81q — and v3d80 post-handover, when that
leg is armed) and folded its span facts into
each kick line — `sampler samples=500 in-span=0 max-in-span=0x00000000` — while the `[v3d73]`
verdict line itself never printed (the emit call was missing; fixed by V3D-82). No leg in the
PA4 capture ever held a CT0CA inside its submitted span. The v3d81q reading is measurement,
not inference.

### 48.2 The list itself is well-formed — content exonerated by decode, not by inference

First byte-level RCL audit of the campaign (§31/V3D-57 audited only the bin CL). The 106-byte
M3 clear-job RCL decodes as 20 packets, exactly 106 bytes, every boundary opcode-aligned,
against the V3D 4.2 encoding (`v3d_packet_v42.xml` naming): the TILE_RENDERING_MODE_CFG
bracket in correct order (COMMON → CLEAR_COLORS_PART1 → COLOR → ZS_CLEAR_VALUES),
TILE_LIST_INITIAL_BLOCK_SIZE, MULTICORE_TILE_LIST_SET_BASE/SUPERTILE_CFG, the GFXH-1742
double dummy-store block, FLUSH_VCD_CACHE, START_ADDRESS_OF_GENERIC_TILE_LIST (whose 26-byte
sub-list, with the real RT0 store, decodes clean), SUPERTILE_COORDINATES, END_OF_RENDERING
(code 13 — the PI-V3D-10 fix present). No unknown opcode, no length mismatch, no field out of
range; packet shape matches Mesa's minimal clear-job emitters with only benign micro-ordering
divergence. Full offset→packet table: the S1U landing report and its decode script.

### 48.3 Where that leaves the wall

The arc premise dissolves and the §45 verdict stands STRENGTHENED: **the CLE never fetched a
byte of any list since the wedge onset; there is no choking packet because there is no fetch.**
List content — previously exonerated only by unreachability — is now exonerated by direct
decode as well. The wall remains exactly where `[v3d74a]` left it: thread 0 never starts,
regardless of list class, a per-thread condition outside the ARM-visible register space
(§46.3–46.4). The instrument that manufactured the "mid-list stall" reading is fixed (V3D-82,
§44 NOTE): `[v3d54]` now prints raw CA and classifies out-of-span words as `(stale)`, and the
`[v3d73]` verdict line rides every CT0-kicking deep leg.

Instrument-lie ledger +1 (the campaign's n+2 of this shape): a wrapping offset over a frozen
register, printed under a verdict string that presumed the register valid.

**NOTE — V3D-83 (same day): the audit generalized the fix.** A sweep of the aarch64 witness
surface for the same class — a derived value computed from a raw register word whose freeze
would go undetected — found the identical unguarded idiom over BPCA, CT0CA's PTB sibling, at
five sites, and the PI-V3D-82 idiom was transplanted to each: `[v3d41]` `ptb_frame_witness`
now seeds a `bpca_pre` beside `bfc_pre`/`rfc_pre` and requires an observed pre→post transition
landing in the pool span before any "the PTB emitted bytes" claim; the `[v3d55]` pool phantom
arm and the `[v3d56]` bpca-vs-bytes mismatch arm gate on the raw word sitting inside the pool
span, routing out-of-span words to an explicit no-verdict stale arm; `[v3d58]` station S4 and
the `[v3d71]` mainline-geom line (both display-only consumers) carry an out-of-pool `(stale?)`
tell. In every case the raw word is printed beside the derived figure and the deciding
instrument (the poison scans) is named. No banked verdict is reinterpreted — the fix constrains
what future lines can claim, not the meaning of past captures.

## 49. The init-gap ladder — what mainline does between cold power and its first bin job, and the three measurements that were never uncontaminated (V3D-84)

§40 left three candidates: PTB `BXCF`, `MISCCFG.QRMAXCNT`, and *an initialisation this
campaign has not found*. §42 then closed the first two **by measurement** against a working
binner. This section answers the third as far as software archaeology can, and its principal
finding is not a new register. It is that **three of the campaign's load-bearing negatives were
measured on a block that already held an unclosed bin frame**, and that the instrument carrying
the most recent of them is a single register §48 has already proven capable of lying.

This is a research section. It changes no driver code and arms nothing. It delivers an exclusion
table, an act-by-act mainline diff, and a ranked ladder for the next code arc.

### 49.1 The exclusion table — what a new theory may not re-propose

Every row below is closed, and by what. A rung that re-opens one of these needs new evidence,
not a new argument.

| Closed | By | Where |
|---|---|---|
| Bin CL encoding, opcode, length, field form, prologue order, terminator | Mesa `v3d_packet.xml` v42 audit, 0 divergences; `v3dX(bcl_epilogue)` | §31, §33 T4 |
| RCL content (the `[v3d74a]` list) | byte-level decode, 20 packets / 106 bytes, no unknown opcode | §48.2 |
| Coordinate-shader words, thread-end, VPM contract | real `v3d_compile()` ver 42, byte-identical | §11, §21 |
| Shader record, attribute record, segment sizes | 15/15 field audit vs Mesa | §14, §10 |
| Submission latch (`CT0QBA`/`QEA`/`QMA`/`QMS`/`QTS`) | read-back audit, byte-exact every kick | §28 |
| Submit **sequence** (order, interleaving, `BPOS=0` position) | `[v3d73s]`, `v3d_bin_job_run` verbatim, nothing interleaved | §44.3 |
| Frame geometry (1 / 4 / 16 tiles) and frame content (empty / full) | `[v3d68]`, bit-identical across all four rungs | §40 |
| Address geometry (mainline-like iovas, 544 KiB pool) | `[v3d71]` mainline-geom | §42 |
| MMU: enable, table base, PTE validity, fault policy on both channels, illegal-address catcher | `[v3d55] mmucfg`/`pte`, `[v3d62] mmufix`/`fault` | §29, §35 |
| GMP | `PROT_ENABLE=0`, `gmpdelta` clean across the frame | §35 |
| L2T / slice cache mode and order (FLUSH vs kernel-exact CLEAR) | `[v3d51]` vs `[v3d53]` differential — both wedge | §27, §29 |
| TMUWCF combiner drain | sourced: `v3d_clean_caches` is a post-render job, never a bin job | §27 |
| Overflow arming (`BPOA`/`BPOS`) | T1: mainline enters **every** bin frame with `BPOS=0` and no block | §33 |
| Clock: rate, gate, domain liveness inside the wedged wait | `clkdom` 500 MHz ACTIVE; `[v3d72]` CYCLE_COUNT Δ703 M with every unit slot at 0 | §29, §43 |
| PM/ASB fabric: bases, identity, bridge state, in-flight beats | DT-verified; `M_CTRL/S_CTRL` 0x7→0x4 across stop/release; `[v3d71f]` 500 samples pinned quiescent | §41.8, §42 |
| Reset path, and its absence | OFF→ON cycle applied and measured; `[v3d79]` genpd-minimal (no reset, no mailbox) equally dead | §24, §46.4 |
| Interrupt policy, core **and** hub working sets | `[v3d49]`, `[v3d52]`; `INT_STS` latches raw regardless of mask | §23, §26 |
| `PTB BXCF` | piOS reads `0x00000000` idle **and** mid-render; ours `0x00000000` | §42.1 |
| `MISCCFG` / `QRMAXCNT` | piOS reads `0x00000006` in both dumps; ours `0x00000006` | §42.1 |
| **`V3D_HUB_AXICFG` (hub `+0x0000`) and `V3D_HUB_UIFCFG` (hub `+0x0004`)** | **closed by this section.** `[v3d76]` sweeps `hub[0x0,0x100)` and diffs it against the piOS capture; §46.1 reduced the whole structural diff to two words, and neither is at `+0x0000` or `+0x0004`. Mainline writes `AXICFG` from exactly one path — the `ver<41` bridge reset, restoring a value that reset disturbs — which the BCM2711 `reset_control` path does not take. Our values already match a working binner. | §46.1, §41.2 |
| `GCA`, `V3D_CTL_L2CACTL`, `v3d_reset_by_bridge` | `ver < 41` / `ver <= 32` paths; this part is ver 42 | §24, §25 |
| CSD / TFU init | mainline performs none before a bin job; neither block participates in binning | §46 |
| Per-thread register divergence (full-window) | `[v3d76]` sweep, both sides: exactly two unnamed words, both proven read-only | §46.1, §46.2 |
| `SET_ENABLE_QPU`, `NOTIFY_DISPLAY_DONE` | read by effect, not by doorbell | §47 |

**What survives.** After that table the wall is stated as: *the CT0 queue latches, the thread does
not fetch, the PTB never issues a beat, the core is clocked and idle, nothing faults, and a CT1
render frame on the same block retires and byte-verifies its store — before and after.*

### 49.2 The mainline path, act by act, with our status

Reconstructed from the Linux V3D stack (`drivers/gpu/drm/v3d/{v3d_drv,v3d_gem,v3d_sched,v3d_irq,v3d_mmu}.c`,
`drivers/soc/bcm/bcm2835-power.c`) and the BCM2711 device tree. **Facts only** — register offsets,
write order and control flow. Linux is GPL-2.0-only and no comment text is reproduced here; the
licence rule of §33 applies unchanged.

| # | Mainline act, cold power → first bin job | Ours | Status |
|---|---|---|---|
| 1 | firmware powers the GRAFX rail and leaves the V3D clock present | mailbox domain 10 / clock id 5 (or nothing, under `[v3d79]`) | covered; P90 proved both variants equally dead — **cosmetic** |
| 2 | genpd `bcm2835_asb_power_on`: deassert `PM_V3DRSTN`, release rpivid master then slave | `enable_pm_asb`, same order, same bits | covered, DT-verified (§41.8) |
| 3 | `reset_control_reset` — **not on the boot path**; only `v3d_reset()`, the hang-recovery path | our OFF half runs it every boot | we do **more** than mainline; `[v3d79]` removed it — still dead |
| 4 | `clk_prepare_enable` on the firmware clock | `SET_CLOCK_STATE` | covered |
| 5 | ioremap `hub` + `core0`; `gca`/`bridge` are `ver<41`-only | fixed bases, no gca/bridge | covered |
| 6 | read `HUB_IDENT1`, derive `ver = tver*10 + rev` | same, corrected at V3D-61 | covered |
| 7 | read `MMU_DEBUG_INFO`, derive va/pa width, set the DMA mask | read and matched (`0x550`) | covered; the mask itself is a Linux allocator constraint, not a register |
| 8 | 4 MiB zeroed page table; the iova allocator **starts at page 1** — page 0 is never handed out | 64 KiB table, arena identity-mapped well above 0, `PTE[0] = 0` | equivalent in effect |
| 9 | `v3d_init_core`: `MISCCFG` only `ver<41`; then `L2TFLSTA=0`, `L2TFLEND=~0` unconditionally | mirrored (`v3d_init_hw_state`) | covered (§25) |
| 10 | `v3d_mmu_set_page_table`: `PT_PA_BASE`, the full `MMU_CTL` fault policy, `ILLEGAL_ADDR` at an unmapped scratch page | mirrored, `MMU_CTL = 0x0640_9801` | covered (§35) |
| 11 | `v3d_mmu_flush_all`: wait `TLB_CLEARING` clear → `MMUC_CONTROL = FLUSH\|ENABLE` → **wait `MMUC_CONTROL.FLUSHING` clear** → `MMU_CTL \|= TLB_CLEAR` → wait `TLB_CLEARING` clear | `program_mmu` writes `MMUC_CONTROL` then `MMU_CTL\|=TLB_CLEAR` **back-to-back** and polls only the trailing `TLB_CLEARING` | **GAP — R4a below.** Two of mainline's three waits are absent |
| 12 | `v3d_irq_init`: `INT_CLR = V3D_CORE_IRQS` (core) and `HUB_INT_CLR = V3D_HUB_IRQS` (hub) — **clear stale latches** — then request the IRQs, then `v3d_irq_enable` | `v3d_irq_enable` writes the two mask pairs only; no init-time `INT_CLR` on either block | **GAP — R4b below** |
| 13 | `v3d_irq_enable`: core `MSK_SET=~IRQS`/`MSK_CLR=IRQS`, hub the same | mirrored, `0xa7` / `0x3a` | covered (§23, §26) |
| 14 | an installed ISR that W1Cs `INT_STS` on every event | none — we poll | structural divergence; cannot gate the **first** frame, which never latches anything |
| 15 | no perfmon attached on the common path | PCTR banks armed around most rungs | off-mainline traffic — R8 |
| 16 | the first V3D job on any Pi is a Mesa CL submit: a **bin job with its render job already queued behind it on the same tile state** | a bare CT1 clear job, then bin jobs alone, forever | **never tested — R5 below** |
| 17 | `v3d_bin_job_run`: `BPOS=0` → `v3d_invalidate_caches` → `CT0QMA`/`QMS` → `CT0QTS\|ENABLE` → `CT0QBA` → `CT0QEA` | byte-exact, uninterrupted, at `[v3d73s]` | covered (§44) |

Three registers mainline **defines** and this driver has never defined, read, or written appear in
the ladder below (R2, R3). They are not writes mainline performs — they are instruments mainline's
header supplies and this campaign never picked up.

### 49.3 The confound — every decisive negative was measured downstream of an unclosed frame

`bringup` runs `clear_job` (CT1) and then `triangle_job`, whose first act is `probe_job`. The
probe's bin is **the first CT0 kick of the boot**; every other CT0 kick in the file — the six
`[v3d48]` rungs, the `[v3d63]` matrix, the `[v3d64]`/`[v3d66]` legs, `[v3d68]`'s four rungs,
`[v3d69]`, `[v3d71]`, `[v3d73s]`, `[v3d74a]`, and M4 itself — runs after it.

§48.1 dates the wedge inside the boot: `CT0CA` **moved genuinely in the pre-wedge era** (offsets
`0x4c`, `0xe` walked, `CTRUN=0`), latched `0x0034b000` at the `[v3d53]` rung where `CTRUN` stuck
`1`, and has not changed since. §29 states the mechanism in its own words: *because `CTRUN` stays
`1`, every later kick is a no-op*. §32's stations confirm the entry condition — `BMACTIVE` reads
`0` at S0 on the probe and `1` at S4.

Three consequences, each of which retires or re-scopes a banked verdict:

1. **Thread 0 demonstrably fetches and executes on the boot's early CT0 kicks.** §45's verdict —
   *thread 0 never starts regardless of list class* — was read from `[v3d74a]`, which is placed
   **last in the deep ladder**, roughly twenty kicks after `CTRUN` stuck. On a CT0 that is still
   running a frame it never finished, "the queue loaded and never advanced" is the *expected*
   reading and carries no information about thread health. The §46 firmware-side campaign
   (V3D-75…80) and the §47 reply-less-NOTIFY work were both raised on that verdict.
2. **"An empty bin frame does not retire" has never been measured on a virgin block.** The
   `[v3d48]` ladder runs after `probe_job`, so the Empty rung has always been kick #2 or later,
   behind a probe frame that opened and never closed. §40 calls that premise the campaign's most
   load-bearing fact and correctly notes no Mesa path ever submits an empty frame; what §40 did
   not note is that it has also never been submitted **first**.
3. **`[v3d68]`'s "bit-identical across 1/4/16 tiles and empty/non-empty" is a statement about a
   wedged block.** All four rungs ride the tail of `empty_frame_bisection`. Bit-identical readings
   across four kicks that were all no-ops is exactly what a stuck `CTRUN` produces, and is weaker
   evidence about frame geometry than it appears.

None of this refutes the wall. The first kick still fails to retire, and that failure is real and
first-kick-true. What it does is invalidate the *chain of inference* that led from "the empty frame
does not retire" to "thread 0 is dead as a thread" to "the condition is outside the ARM-visible
register space" — because every link after the first was measured through a block that had already
stopped answering.

### 49.4 The ladder — cheapest-decisive first

Each rung states the exact access, the reading under *this was the missing init*, the reading under
*still dead*, and what a negative excludes. Every rung is one boot or less, and every one admits an
honest negative.

---

**R0 — fix the instrument before spending a boot on it: `[v3d55] tilestate`/`pool` scored its own
poison as a PTB write.** — **DONE (code), PENDING METAL.**

*The bug.* `v3d55_tilestate_readback` counted **nonzero words** in the tile-state array and the
tile-alloc pool and read any nonzero count as *the PTB WROTE*. Since V3D-56 both regions are
pre-filled with `0xA5A5A5A5 ^ i`, which is nonzero at every index — so a region the PTB never touched
scored `nonzero_words == words` and the witness asserted a write on the strength of its own poison.
The `pool` line carried the same defect, and its "the pool carries binner bytes" arm fired on intact
poison too. Metal evidence (§30, §48, the `[v3d56]` and `[v3d68]` scans, which classify correctly)
says the PTB has likely never written a byte, so wherever the `[v3d55]` "wrote" arm fired it was
**inverted**. No rung below is worth a boot while a first-order witness reads backwards.

*The fix.* Classify every word against the fill published before the kick, in three disjoint classes,
and report all three counts plus the first non-poison word:

| Class | Test | Meaning |
|---|---|---|
| `INTACT` | `word == 0xA5A5A5A5 ^ index` | **not written** |
| `ZEROED` | `word == 0` (and the fill was not `0`) | **written**, with zero bytes — what an empty tile list emits |
| `WRITTEN` | anything else | **written**, with real data |

`touched = ZEROED + WRITTEN` is the "was this region written at all" count, and
`first_nonpoison=[i] (got … expected …)` names the first disturbed word. Positive evidence (`touched
> 0`) still precedes the `flush_done` guard — disturbed bytes are in DRAM whatever the drain did —
while the negative reading stays gated on a completed L2T write-back, exactly as before. Under a
zero-fill build (`V3D56_POISON` off) `ZEROED` is unreachable and both lines say on the wire that no
write/no-write verdict follows. The `pool` line additionally prints the Mesa-predicted empty-bin
`BPCA` advance beside the measured one and no longer re-asserts the phantom-pointer verdict §30
retracted.

*What it costs.* Nothing at runtime — the same scan loop, the same gating (`UNAOS_V3D`, metal-only
behind the hub-identity gate), no new knob, no new register access, no change to the kick path.

*What it decides.* Nothing on its own. It makes every later reading of these two lines mean what it
says, and it retires the "wrote" verdicts previously taken from them.

*Status.* Code landed and in the image; the verdict itself is **metal-attended** and unread until a
Pi 4 boot prints the new line. QEMU `raspi4b` models no V3D block, so no `[v3d55]` line appears there.

---

**R1 — one experiment per boot: make the rung under test the boot's FIRST CT0 kick.** *(BUILT — V3D-85, `UNAOS_V3D_FIRSTKICK=<empty|rcl|m4|rclp<n>|rclhead[<op>]|emptyunarm>`; see §49.7, §49.9, §49.10, §49.11. R0 above lands in the same image, so the `[v3d55]` lines a first-kick capture carries are the fixed three-class ones.)*

*Action.* A knob (`UNAOS_V3D_FIRSTKICK=<empty|rcl|m4>`) that runs exactly one CT0 kick, placed
**before** `probe_job`, with the full witness suite (`[v3d54]` submit + trace, `[v3d58]` stations,
`[v3d73]` sampler, `[v3d68]` poison scans, `[v3d44]` FLDONE wait) and then **returns** — no ladder,
no battery, no second kick. Three boots, or one boot per variant.

- `empty` — the `[v3d48]` Empty frame as kick #1. Re-takes the campaign's most load-bearing
  measurement without the probe frame in front of it.
- `rcl` — `[v3d74a]`'s render list on CT0 as kick #1. Re-takes §45's thread-vs-class discriminator
  on a CT0 that has never been asked to bin.
- `m4` — the real draw as kick #1, so the M4 verdict is not read through the probe either.

*Missing-init reading.* `empty` retires (`FLDONE`, `BFC Δ1`, `BMACTIVE→0`) — the wall is state the
block accumulates across kicks, not a cold-boot init gap, and the whole campaign's baseline is
retracted. Or `rcl` store-verifies while `empty` wedges — thread 0 is healthy from cold and the
death is bin-class-specific, which retires §46/§47's firmware branch outright.

*Still-dead reading.* `empty` wedges identically as kick #1: `CTRUN=1`, `BMACTIVE=1`, poison intact,
`[v3d73]` in-span=0. Then the banked verdicts are first-kick-true, the confound is excluded, and
every later rung in this ladder is measured against a baseline that finally means what it says.

*A negative excludes.* Measurement contamination — the one hypothesis that currently sits underneath
every other verdict in this file. **This rung is first because nothing below it is interpretable
until it is taken.**

---

**R2 — the MMU access counters: an independent witness for "the CLE never fetched".** *(BUILT — V3D-85, rides R1; see §49.7.)*

*Action.* Pure reads of three hub registers this driver has never defined, at offsets already named
in `v3d.rs`'s own MMU comment block and consistent with its established anchors
(`MMU_CTL 0x1200`, `PT_PA_BASE 0x1204`, `VIO_ID 0x122c`):

| Register | Hub offset |
|---|---|
| `V3D_MMU_HIT` | `0x1208` |
| `V3D_MMU_MISSES` | `0x120c` |
| `V3D_MMU_STALLS` | `0x1210` |
| `V3D_MMU_ADDR_CAP` | `0x1214` (decode `ENABLE` bit 31 + the page-count field) |

Sample all four **before and after** each of: the M3 CT1 clear job (the known-good positive
control), the first CT0 bin kick, and the ~0.5 s wedged wait. Report deltas, and treat a zero delta
on the CT1 control as INCONCLUSIVE rather than as a bin verdict.

*Missing-init reading.* `ΔHIT`/`ΔMISSES` nonzero across the CT0 kick — **the CLE did issue
translations**, `CT0CA` is a lying register on the bin path exactly as it was on the offset
arithmetic (§48), and "the CLE never fetches" is retracted. The hunt returns to what happens
*after* the fetch.

*Still-dead reading.* `ΔHIT = ΔMISSES = ΔSTALLS = 0` across the CT0 kick while the CT1 control moves
them — the thread genuinely never issued a memory access, now on a second instrument that shares no
mechanism with `CT0CA`.

*A negative excludes.* The possibility that §48's verdict rests on a single frozen register. This is
the cheapest rung in the ladder — about eight MMIO loads — and it is decisive either way. `ADDR_CAP`
is read in the same pass because V3D-62 armed `CAP_EXCEEDED_ABORT` **without** anyone ever reading
the cap it aborts against; if bit 31 is set and the page field is small, every access above it is
aborted silently and that has been true since V3D-62.

---

**R3 — the CLE's thread registers, as distinct from its queue registers.** *(BUILT — V3D-85, rides R1; see §49.7.)*

*Action.* Read `V3D_CLE_CT0EA` (core `0x0108`) and `V3D_CLE_CT0RA` (core `0x0118`) at the five
`[v3d58]` stations. Both offsets follow from this file's **own** established map — `CT0CS 0x0100`
/ `CT1CS 0x0104`, `CT0CA 0x0110` / `CT1CA 0x0114`, `CT0LC 0x0120` / `CT1LC 0x0124` — leaving
`0x0108`/`0x010c` and `0x0118`/`0x011c` as the `CTnEA` and `CTnRA` pairs in the same
n-interleaved layout. This is derivation from corroborated anchors, not a fabricated constant.

The point is the distinction the campaign has never drawn: `CTnQBA`/`CTnQEA` are the **queue**
registers the driver writes; `CTnCA`/`CTnEA` are the **thread's** working current/end addresses.
The queue transfers into the thread when the thread starts.

*Missing-init reading.* `CT0EA` never takes the value written to `CT0QEA` across the kick — the
**queue-to-thread transfer never happened**, which is one station upstream of "never fetched" and
names a different mechanism entirely. `CT0RA` non-zero additionally settles §33's inferred-`CTSUBS`
row with the actual return-address register instead of a borrowed bit.

*Still-dead reading.* `CT0EA == CT0QEA` with `CT0CA` frozen — the thread *did* load and then stalled
at its first fetch. Combined with R2 this is the tightest statement the campaign can make.

*A negative excludes.* The "the queue never transferred" branch, and — via `CT0RA` — the sub-list
hypothesis §33 raised and never followed.

---

**R4 — the two mainline probe steps still unmirrored.**

*(a) The MMU flush completion waits.* `v3d_mmu_flush_all` waits for `TLB_CLEARING` to clear *before*
starting, writes `MMUC_CONTROL = FLUSH|ENABLE`, then **waits for `MMUC_CONTROL.FLUSHING` to clear**,
and only then writes `MMU_CTL |= TLB_CLEAR`. `program_mmu` issues the two writes back-to-back and
polls only the trailing `TLB_CLEARING`. Add both waits with before/after echoes.

*(b) The init-time interrupt clear.* `v3d_irq_init` W1Cs `INT_CLR = V3D_CORE_IRQS` on the core and
`HUB_INT_CLR = V3D_HUB_IRQS` on the hub at probe, before unmasking. Add both at the head of
`v3d_irq_enable`, printing the pre-clear latch words — a stale latch inherited from firmware has
never been looked at on either block.

*Missing-init reading.* The first-kick Empty frame retires with the waits in place. (a) is the more
interesting half: a TLB clear issued on top of an in-flight MMUC flush is a real race with a real
mainline guard, and a half-cleared TLB producing a first fetch that stalls forever with no fault is
consistent with the signature.

*Still-dead reading.* Both land as no-ops, echoes clean.

*A negative excludes.* The last two mainline probe-path acts this driver does not perform. §46.4
claims the ARM-side divergence space is empty; these two rows are the counter-examples, and closing
them is what makes that claim true rather than nearly true. Weak prior, cheap, and mirror-exact —
the §26/§27 pattern.

---

**R5 — the frame pair: a bin job with its render job queued behind it.**

*Action.* Submit the M4 bin on CT0 and, **without waiting for `FLDONE`**, submit the matching RCL on
CT1 against the same tile-state and tile-alloc buffers; then wait for both, sampling `PCS`, `BFC`,
`RFC` and the `[v3d73]` span facts across the whole window.

Every frame the working reference system has ever closed had a render job queued behind it: a Mesa
CL submit is a bin job and a render job on the same tile state, and the first V3D job on any booted
Pi is such a pair. This driver has submitted the bin alone, every time, for the entire campaign.
§23 exonerated the ordering **by citation** — `v3d_bin_job_run` touches only CT0 — but that is a
statement about the driver's write set, not about what the hardware's frame unit waits on.

*Missing-init reading.* `FLDONE` latches, `BFC` advances, once CT1 is armed for the same frame.

*Still-dead reading.* The bin wedges identically and the render job either wedges behind it or
retires against an empty tile list.

*A negative excludes.* The last untested frame-level ordering hypothesis, and the only structural
difference left between how this driver uses the block and how every working driver does.

---

**R6 — `CTRSTA`: the thread reset. DISARMED AGAIN (R23 boot7, 2026-08-02 — see §49.8).**

> **The bar named below is no longer met.** R6's case rested on the borrowed VC4-era `CTnCS` map
> being *good enough at the bits that matter*, with `0x70` read as a measurably illegal fetching
> state. boot7 falsified that map at bit 4 — `CTSUBS` set with `CT0RA` zero at every station — and
> `CTRSTA` (bit 15) is not an independent constant: it comes from the same borrow, on the same
> offset-identity argument, with no separate corroboration anywhere in this file. Writing an
> uncorroborated bit into a control register is the PI-V3D-4/-6 bug class, and the readback
> mitigation below does not cover it: a wrong bit 15 may *do* something, and "nothing was risked"
> was only ever true if the constant was merely inert. **Do not arm R6** until a `CTnCS` field
> layout for V3D 4.2 is sourced rather than inferred. The rest of this entry is kept for provenance.

*Action.* Write `CT0CS = CTRSTA` (bit 15) on an already-wedged CT0, read `CT0CS` back, then re-kick.
Issue it **only** between rungs whose verdicts are already banked, never before a first kick.

§32 and §33 kept this disarmed for two stated reasons, and both had moved as of §49.4 — the second of
which §49.8 has since taken back:

- *"the constant is inferred, not corroborated."* Still true — bit 15 comes from the VC4-era map
  carried across on offset identity, and `v3d.rs` records that only `CTRUN` is corroborated for
  V3D 4.x. **The mitigation is that the readback is the verdict**: the write is issued on a thread
  already proven dead, and if `CT0CS` does not move, the bit is wrong and nothing was risked.
- *"no reading justifies it."* This has changed. §46.1 supplies ground truth from the mid-bin piOS
  capture: a **genuinely fetching** mainline CT0 reads `CT0CS = 0x20` (`CTRUN` alone), while our
  wedged thread reads `0x70` — bits 4 and 6 set in addition. The wedged word is measurably not a
  legal fetching state, and §48 established the condition is *latched and persistent* (frozen since
  the `[v3d53]` rung). A latched illegal thread state is precisely what a thread reset exists for.

*Missing-init reading.* `CT0CS` returns to `0x00`/`0x20` and the next kick fetches — the wedge is
clearable, R1's one-experiment-per-boot cost collapses to one boot for the whole ladder, and the
campaign regains the ability to take more than one honest bin measurement per boot.

*Still-dead reading.* `CT0CS` unchanged at `0x70` — either the bit is not `CTRSTA` on 4.2 (in which
case discard the entire borrowed map, `CTSUBS` included) or the wedge survives a thread reset, which
is itself a strong new fact.

---

**R7 — decode the two bits that separate a fetching thread from a wedged one.** *(BUILT — V3D-85, rides R1; see §49.7.)*

*Action.* Analysis plus reads, no writes. `CT0CS` wedged `0x70` vs mainline-fetching `0x20`: bit 4
is `CTSUBS` on the VC4 map (the thread believes it is inside a sub-list); **bit 6 appears in no
published map at all**. Cross-read against `CT0RA` from R3 — a thread genuinely inside a sub-list
has a return address, and a thread that only *claims* to does not. Sample both across R1's first
kick, where the transition into `0x70` can be watched for the first time.

This is the **only ground-truth-anchored state divergence** between the wedged and the working
system that §46.3 set aside as "job state" without decoding. It is free, and it is the difference
between naming the wedge and describing it.

---

**R8 — remove all off-mainline instrument traffic, for the whole boot.**

*Action.* One boot with the PCTR file fully disabled (`EN=0`, `SRC=0`), no `INT_CLR` inside the
queue pair, no station reads, no `[v3d55]` mailbox round-trips inside the kick path — `[v3d73s]`'s
discipline extended from one rung to the entire boot, combined with R1's single-kick placement.
Weak prior: P90's `[v3d79]` MINIMAL boot came close on the bring-up side but still ran the
instrumented battery. It is the last untried conjunction, and it costs one boot.

---

### 49.5 The two named registers, and the honest safety argument

The brief carries `BXCF` and `MISCCFG.QRMAXCNT` forward from §40's verdict. §42 — written after
§40 — closed both **by measurement**, and this section will not launder that.

| | `PTB BXCF` (core `0x0310`) | `MISCCFG.QRMAXCNT` (core `0x0018`, bits `[3:1]`) |
|---|---|---|
| Disarm condition set at | §33: *arm if `BXCF` reads non-zero* | §29: *arm if `QRMAXCNT` reads floored (`0`)* |
| What metal read | `0x00000000` (§33 battery) | `MISCCFG = 0x6`, `QRMAXCNT = 3` (§30) |
| What a **working binner** reads | `0x00000000`, idle **and** mid-render (§42.1) | `0x00000006`, both dumps (§42.1) |
| Verdict recorded | EXCLUDED MEASURED — *do not re-litigate* | EXCLUDED MEASURED — *ditto* |

**The evidence bar was met — in the negative.** §29's own matrix says it outright: *`QRMAXCNT != 0`
⇒ no divergence; the QoS branch is not justified — refuted without a write.* The reading that would
have licensed the write never came, and §42 then went further and matched both words against a
binner that closes frames. Arming either write today is not "finally taking the last rung"; it is
**deliberately diverging from a working reference on a register where we are already identical to
it**. That is the inverse of the method this campaign has run on for thirty arcs.

Two further points against, both structural:

- `BXCF`'s documented fields are `CLIPDISA` (bit 0) and `RWORDERDISA` (bit 1) — **disable** bits.
  Writing them can only remove PTB function. There is no value of `BXCF` that starts a dead binner
  which `0x00000000` does not already supply.
- The only `QRMAXCNT` experiment that is not simply "become different from piOS" is raising it
  *above* the working system's value (3 → 7), which tests the hypothesis *this silicon needs a
  deeper request queue than the silicon that works*. Nothing in the campaign supports that.

**Recommendation: leave `V3D55_ARM_QRMAXCNT` and any `BXCF` write disarmed, and rank both below
every rung above.** If the lead elects to burn them anyway as an exhaustion play, the safe design is
fixed and should be stated in the code that carries it:

- scoped read-modify-write of the named field only, every other bit of the word preserved;
- issued **after** R1's first-kick verdict is banked, never before it, so the boot's one
  uncontaminated measurement is not spent on an excluded theory;
- echo-back printed, with the piOS reference value on the same line;
- the witness must state on the wire that the register is EXCLUDED MEASURED and that the expected
  outcome is **no change** — so a capture cannot later be read as though the write were licensed;
- restore the reference value before any other rung runs.

### 49.6 Ranking, and what the next arc should spend

| Rung | Cost | Decides |
|---|---|---|
| R0 `[v3d55]` fill classification | free, **done pending metal** | nothing on its own — but it stops a first-order witness reading backwards, and retires the "wrote" verdicts taken from it |
| R1 first-kick isolation — **BUILT, V3D-85 (§49.7)** | 1–3 boots | whether any banked negative in this file means what it says |
| R2 MMU access counters — **BUILT, V3D-85 (§49.7)** | ~8 reads, rides R1 | whether the CLE ever issued a translation, independent of `CT0CA` |
| R3 `CT0EA`/`CT0RA` — **BUILT, V3D-85 (§49.7)** | 2 reads/station, rides R1 | whether the queue ever transferred into the thread |
| R7 `CT0CS` bit 4/6 decode — **BUILT, V3D-85 (§49.7)** | free, rides R1 | names the wedge state against the working reference |
| R4 MMUC waits + init `INT_CLR` | 1 boot | empties the mainline-mirror space for real |
| R5 bin+render frame pair | 1 boot | the last untested frame-level ordering |
| R6 `CTRSTA` | rides any boot after R1 | whether the wedge is clearable — and if so, collapses R1's cost |
| R8 instrument-free boot | 1 boot | the last untried conjunction |
| R9/R10 `BXCF` / `QRMAXCNT` | 1 boot | **nothing** — both EXCLUDED MEASURED; listed for completeness, recommended against |

R1, R2, R3 and R7 all ride a single boot. That boot is the whole first sitting, and it either
restores or retracts the foundation the last ten arcs were built on.

**Standing caveat for whoever reads it.** R1 deliberately suppresses the rest of the battery, so a
capture taken under it will be missing every banked witness the file's readers expect. That is the
point — a first kick with a ladder behind it is not a first kick — but it means such a capture must
be labelled at the top and never diffed line-for-line against a full deep boot.

QEMU `raspi4b` models no V3D block, so every rung above sits behind the hub-identity gate and
`kernel8-test` green means **no regression and nothing more**. Every verdict in this section is
metal-attended, and this section itself arms nothing: it is a plan, and the writes it describes
belong to the next code arc.

### 49.7 The rungs that were built — R1 + R2 + R3 + R7, one boot, one knob (V3D-85)

§49.6's "R1, R2, R3 and R7 all ride a single boot" is now code. The four rungs share one arming
switch and one placement, and nothing else in the file changed: the diff is additive, and with the
knob off not one line of it is compiled (`strings` on a knob-off image finds zero occurrences of any
tag below, with the pre-existing `[v3d74a]` still present as the control).

**The knob.** `UNAOS_V3D_FIRSTKICK=<empty|rcl|m4>` ⇒ Cargo feature `v3d_firstkick` (implies `v3d`),
**default OFF**. The knob's *presence* arms the feature; its *value* is read with `option_env!` and
picks the list, so one build is one experiment. `1` and an empty value select `empty` (§49.6's top
pick); an unrecognised value runs `empty` and says on the wire that it was not recognised, so a typo
can never be read as a deliberate variant choice. The raw value rides the `[v3d85]` header line.
PI-V3D-86 later added a fourth value **family**, `rclp<n>` — the prefix bisection; see §49.9.
PI-V3D-87 added two more, `rclhead` and `emptyunarm` — the discriminators; see §49.10.
PI-V3D-88 generalised the head swap into a second value **family**, `rclhead<op>`, where `<op>` is the
prepended packet's decimal opcode: bare `rclhead` is unchanged and is still op 119, and `rclhead126`
is the render-CLASS vs sub-id-DISPATCH split; see §49.11.

The exact deep-battery boot line for the sitting:

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=empty ./arroyo kernel8
```

`UNAOS_V3D_DEEP=1` is carried for build parity with the bench's standing battery; under
`UNAOS_V3D_FIRSTKICK` the deep tails are never *reached*, because `bringup` returns at the rung. Swap
the value to `rcl` or `m4` for the other two variants — one per boot, never two.

**Placement.** The rung sits in `bringup` immediately after M3 (`clear_job`, the CT1 frame that is
R2's positive control and `[v3d58] xengine`'s render reference) and immediately **before**
`triangle_job` — i.e. in front of `probe_job`, the kick this whole section exists to get ahead of.
`bringup` then **returns**: no `probe_job`, no `[v3d48]` ladder, no `[v3d63…v3d81]` tails, no M4, no
visible battery. The suppression is the instrument, not a limitation of it.

**Scope.** Every access these rungs add is a READ. The only writes are the ones the kick itself
already performs — the byte-exact `v3d_bin_job_run` sequence unchanged since §44, plus the
stale-latch `INT_CLR` W1C every rung in this file makes. `PTB BXCF` and `MISCCFG.QRMAXCNT` stay
**EXCLUDED MEASURED** (§49.5) and are read, never written. `CTRSTA` (R6) is **not implemented**. No
mailbox tag is sent from this path, reply-less or otherwise.

**The registers this arc names for the first time.** All reads, all derived from anchors already
corroborated in this file — never fabricated constants.

| Rung | Register | Offset | Derivation |
|---|---|---|---|
| R2 | `V3D_MMU_HIT` | hub `+0x1208` | contiguous with `MMU_CTL 0x1200` / `PT_PA_BASE 0x1204`, in mainline's own naming order |
| R2 | `V3D_MMU_MISSES` | hub `+0x120c` | ditto |
| R2 | `V3D_MMU_STALLS` | hub `+0x1210` | ditto |
| R2 | `V3D_MMU_ADDR_CAP` | hub `+0x1214` | ditto; `ENABLE` bit 31 + a page-count field whose width this file's map does not name, so the raw word is printed beside the decode |
| R3 | `V3D_CLE_CT0EA` | core `+0x0108` | the n-interleaved CLE map: `CT0CS 0x0100`/`CT1CS 0x0104`, `CT0CA 0x0110`/`CT1CA 0x0114`, `CT0LC 0x0120`/`CT1LC 0x0124` leave `0x0108`/`0x010c` as the `CTnEA` pair |
| R3 | `V3D_CLE_CT0RA` | core `+0x0118` | same layout leaves `0x0118`/`0x011c` as the `CTnRA` pair |
| R7 | `CT0CS` bit 6 | core `+0x0100` | not a new register — the bit that **appears in no published map** and that §46.1's ground truth isolates |

**The witness tags, and what each reads under both outcomes.**

| Tag | Line | Wedged (still-dead) shape | Fetching (missing-init) shape |
|---|---|---|---|
| `[v3d85]` | header + closer; knob value, variant, the standing caveat | printed either way — a capture under this knob is deliberately short and must **never** be diffed line-for-line against a deep boot | ditto |
| `[v3d85r1]` | `S0 VIRGIN` + `first-kick verdict` | `retired=0 executed=0`, `BFC Δ0`, `PCS` `BMACTIVE=1`, sampler `in-span=0` (or `max-in-span==QBA`), poison fully intact ⇒ **the banked negatives are first-kick-true and the §49.3 confound is EXCLUDED** | `retired=1`/`store-verified=1`, `BFC Δ1`, `BMACTIVE→0` ⇒ the wall is accumulated state, not a cold-boot init gap, and the campaign's baseline is **retracted** |
| `[v3d85r2]` | `mmu-access` ×4 windows (M3 CT1 control, CT0 kick, wedged wait, whole kick) + `addr-cap` | control moved, CT0 windows `ΔHIT=ΔMISSES=ΔSTALLS=0` ⇒ **the thread issued no memory access, on a second instrument sharing no mechanism with `CT0CA`** | CT0 `ΔHIT`/`ΔMISSES` non-zero ⇒ the CLE **did** translate, `CT0CA` is lying on the bin path exactly as it lied on the offset arithmetic (§48), and the hunt moves *after* the fetch |
| `[v3d85r2]` | `addr-cap` (separate line, separate claim) | `ENABLE=0` ⇒ the V3D-62 `CAP_EXCEEDED_ABORT` arming has never had anything to abort — branch excluded | `ENABLE=1` with the page field **below** the arena's top page ⇒ every access above it has been silently aborted since V3D-62; a live candidate for the whole wall |
| `[v3d85r3]` | 5 station rows + `queue-vs-thread` | `CT0EA` never takes `CT0QEA`'s value and never moves ⇒ **the queue-to-thread transfer never happened** — one station upstream of "never fetched", a different mechanism entirely | `CT0EA == CT0QEA` from S3 on with `CT0CA` frozen ⇒ the thread **did** load and then stalled at its first fetch; combined with R2 this is the tightest statement the campaign can make |
| `[v3d85r7]` | 5 station rows + `cs-decode` | `CT0CS` reaches `0x70`; the row where bits 4/6 first set names the write that took the thread from legal to illegal. `CTSUBS=1` with `CT0RA==0` at every station ⇒ the thread only *claims* a sub-list, bit 4 is not `CTSUBS` on 4.2, and §33's inferred row falls (which also disarms R6's premise — same borrowed map). `CTSUBS=1` with `CT0RA!=0` ⇒ the sub-list state is consistent and §33's never-followed hypothesis is live again | `CT0CS` ends at `0x20` — `CTRUN` alone, byte-identical to the working mainline reference ⇒ the `0x70` signature is **not** a cold-boot property of this part |
| `[v3d85r7]` | `first-station-reading-0x70` | `0` (already `0x70` at S0, before this driver wrote one CT0 register) ⇒ the signature is a **bring-up** fact, not a frame fact; chase bring-up | `≥3` ⇒ the transition is caused by the kick and is watchable for the first time |

Two readings that are **not** verdicts and are labelled as such on the wire: a zero delta on R2's M3
CT1 *control* retires R2 for that boot (the instrument does not count on this part, so every CT0
window is INCONCLUSIVE and the `[v3d73]`/`CT0CA` evidence stands alone); and zero sampler ticks make
the fetch columns INCONCLUSIVE while leaving the retire columns intact.

**Gate.** `./arroyo check` green on both arches, knob off **and** knob on. Full-knob `kernel8` build
green; `strings -a target/pi_baremetal/kernel8.img` shows `[v3d85]`, `[v3d85r1]`, `[v3d85r2]`,
`[v3d85r3]`, `[v3d85r7]` with the knob on and **none** of them with it off. QEMU models no V3D, so
there is no QEMU leg for this arc at all: the verdict is the attended metal boot.

**R0 rides the same image, but not the same code path.** The `[v3d55]` fill-classification fix landed
independently and is unconditional — however `v3d55_tilestate_readback` is called from `probe_job`,
which a first-kick boot never reaches, so **no `[v3d55] tilestate`/`pool` line appears in a
`UNAOS_V3D_FIRSTKICK` capture at all**. R1 is deliberately not wired to it: that function
`clean_invalidate`s the poisoned regions *before* the L2T write-back, an ordering `probe_job` accepts
on the strength of every arena writer cleaning to PoC, and R1's whole still-dead reading rests on
those regions being untouched. The poison evidence in a first-kick capture comes from `[v3d56]`'s
scans over the same two regions — the classifier R0 fixed `[v3d55]` to match, and the one that was
never wrong. Read `[v3d56] scan` where you would normally read `[v3d55] tilestate`.

**Not built, deliberately.** R4 (the two mainline probe steps still unmirrored), R5 (the bin+render
frame pair), R6 (`CTRSTA`), R8 (the instrument-free boot) and R9/R10 (`BXCF`/`QRMAXCNT`, recommended
against in §49.5) are untouched. They are separate boots and separate arcs, and §49.4 is explicit
that nothing below R1 is interpretable until R1 is taken.

---

### 49.8 The first-kick bracket — three metal boots, and what they cost the file (R23, 2026-08-02)

The R1 rung was taken three times on the bench, one variant per boot, on the same image family and
the same session: capture `pi4-r23s1x/ttyACM0.log`, marks `boot6` / `boot7` / `boot8`. This is the
first time in the campaign that three *different* list classes were each measured as a boot's
**first** CT0 kick, with nothing in front of them. Read them as a bracket; individually they each
say less than the three of them say together.

#### The bracket table

| Boot | Variant | Image | `CT0CA` | `CT0CS` at S3/S4 | `BFC` | Poison (tile-state / pool) | `BMACTIVE` S0..S4 | Reading |
|---|---|---|---|---|---|---|---|---|
| boot6 | `empty` | `529e2263` | `QBA → QEA` (whole 14-byte list) | `0x20` → `0x00` | **Δ1** | 48 of 64 touched / 20 of 8192 touched | `00000` | **The empty frame RETIRED as kick #1** — a bin frame closed and the PTB wrote real, non-zero bytes into both regions |
| boot7 | `rcl` | `529e2263` | **held `QBA`, 500 samples, never moved** | `0x18` at S3 **and** S4 (`CTERR`+`CTSUBS` on the borrowed map, `CTRUN`=0) | Δ0 | **INTACT 64/64 and 8192/8192** | `00000` | **Wedged at the first fetch.** The thread was provably loaded (`CT0EA` took `QEA` at S3) and then never issued one access |
| boot8 | `m4` | `22e7997d` | `QBA → QEA` (whole 76-byte list) | `0x00` → `0x00` | Δ0 | **INTACT 64/64 and 8192/8192** | `00001` | **List consumed, frame OPENED and left open.** `CT0PC=3` (three primitives fed), `INT_STS` bit 16 latched, `retired=0` |

Three list classes, three different failure *stations*. That is the point of the bracket.

#### What the bracket retracts

**RETRACTED — "an empty bin frame does not retire."** boot6 closed a bin frame (`BFC` Δ1) on both
engines and the PTB wrote non-zero bytes over the poison in *both* output regions. Every reading in
this file that rested on "the empty frame is the minimal case and it does not retire" was measured
behind `probe_job`'s wedged frame, exactly as §49.3 warned. The `[v3d48]`/§22 ladder is **not** void —
its packet-class bisection is still a real measurement — but its *baseline* is, and no negative from
it may be cited without an R1-placed re-take. §22 now carries this pointer at its head.

**RETRACTED — §33's inferred-`CTSUBS` row.** boot7 read `CTSUBS` (bit 4) **set** at S3 and S4 with
`CT0RA` reading **zero at every one of the five stations**. A thread that is genuinely inside a
sub-list has a return address; this one claims the state and has none. Bit 4 is therefore not
`CTSUBS` on V3D 4.2, the VC4-era map §33 borrowed on offset identity alone is wrong *at that bit*,
and §33's "audit every `BRANCH`/`RETURN` in the bin CL" row falls with it. §33's probe matrix is
annotated accordingly.

**DISARMED — R6 (`CTRSTA`).** R6's whole case for arming (§49.4) was that the wedged `0x70` word is
*measurably* not a legal fetching state on the borrowed map, and that `CTRSTA` (bit 15) comes from
that same map. boot7 falsified the map at bit 4 — the same map, the same provenance, the same
single act of borrowing. Bit 15 is not independently corroborated by anything, so R6 goes back to
disarmed and stays there until a `CTnCS` layout for 4.2 is sourced rather than inferred. R6's entry
in §49.4 carries this.

Two further facts the bracket banks, neither of them a retraction:

- **`CT0CS = 0x18` is a fourth word.** §46.1's ground truth gives fetching `0x20` and this campaign's
  banked wedge `0x70`. boot7 read `0x18` — `CTERR`(b3) + `CTSUBS`(b4), with `CTRUN` **clear** — at S3
  and unchanged at S4. Nothing in this file predicts it, and with the bit-4 retraction above the word
  cannot be decoded at all today; it is recorded as a raw signature, not as a decode.
- **`INT_STS` bit 16 on boot8 is a QPU host interrupt, not a CLE or PTB signal.** Read from this
  driver's own constants: `V3D_INT_QPU_MASK = 0xffff0000` / `V3D_INT_QPU_SHIFT = 16`, bit `16+n` =
  QPU *n* raised a host interrupt (the QPU executed an instruction carrying the thread-end-with-host-
  interrupt signal). `0x0001_0000` is therefore **QPU 0 signalled program end** — the coordinate
  shader ran to completion — while `FLDONE` never latched and `BFC` never moved. It is deliberately
  outside `V3D_CORE_IRQS` (`0xa7`), so it latches in the raw status regardless of the mask.
  [READ-FROM-CODE] `unaos/crates/kernel/src/arch/aarch64/v3d.rs`, the V3D 4.x interrupt-bit block.

#### The wall statement

> **Two list classes execute; the RCL list's first packet freezes the CLE fetch.**

Both **bin-class** lists — the 5-packet `empty` frame and the 14-packet `m4` draw — were walked end
to end by the CLE (`CT0CA` reached `QEA` on both). The **render-class** list was not: `CT0CA` sat on
`QBA` for 500 consecutive samples across a half-second wait, with the thread loaded and the MMU
counters flat. The CLE is not broken as a fetch engine and the bin class is not uniquely rejected at
fetch; what is rejected is the *first packet of the render list on CT0*. Everything below this line
is about naming that packet.

Note the shape this leaves: `empty` closes its frame, `m4` opens one and never closes it, `rcl`
never starts. Those are three distinct defects, and only the third one is what §49.9 goes after.

#### The candidate head-packet differences, from our own list builders

All three lists are built in `unaos/crates/kernel/src/arch/aarch64/v3d.rs`. The inventories below are
the builders' own emission order, corroborated on the wire by the `[v3d36]`/`[v3d57]` decodes in
boot6/boot8 and by the `[v3d15]` byte dump in boot7.

| | `empty` (bin) | `m4` (bin) | `rcl` (render) |
|---|---|---|---|
| Builder | `build_bin_cl_content_geom(.., BinContent::Empty, ..)` | `build_bin_cl` | `build_rcl` |
| Arena offset | `OFF_PROBE_BIN_CL = 0x35000` | `OFF_BIN_CL = 0x10000` | `OFF_RCL = 0x8000` |
| Packets | 5 | 14 | **20** |
| Bytes | 14 | 76 | **106** |
| First packet | `NUMBER_OF_LAYERS` (op 119, len 2) | `NUMBER_OF_LAYERS` (op 119, len 2) | **`TILE_RENDERING_MODE_CFG` (op 121, len 9), sub-id `COMMON`** |
| Second packet | `TILE_BINNING_MODE_CFG` (op 120, len 9) | `TILE_BINNING_MODE_CFG` (op 120, len 9) | `TILE_RENDERING_MODE_CFG`, sub-id `CLEAR_COLORS_PART1` |
| Terminator | `FLUSH` (op 4) | `FLUSH` (op 4) | `END_OF_RENDERING` (op 13) |
| `CT0CA` outcome | walked `QBA→QEA` | walked `QBA→QEA` | **froze at `QBA`** |

The concrete, code-read differences that are candidates for the freeze, each with the constant or
call site it is read from:

1. **First packet opcode/class.** The two lists that fetch both open on `NUMBER_OF_LAYERS`
   (`P_NUMBER_OF_LAYERS = 119`); the list that does not opens on `TILE_RENDERING_MODE_CFG`
   (`P_TRMC = 121`). 121 is the highest opcode in either list and the only head packet that is
   *render-class*. [READ-FROM-CODE] `build_bin_cl_content_geom` first `w.pkt(...)`; `build_rcl` first
   `w.pkt(...)`; `P_NUMBER_OF_LAYERS` / `P_TRMC` constants.
2. **First packet length.** 2 bytes (bin) vs **9 bytes** (render). If the CLE prefetches a fixed
   window and the head packet's declared length disagrees with what the *bin*-armed decoder expects,
   the first fetch is exactly where that shows. [READ-FROM-CODE] `Pkt::new(P_NUMBER_OF_LAYERS, 2)` vs
   `Pkt::new(P_TRMC, 9)`.
3. **Sub-id-dispatched head packet.** `TILE_RENDERING_MODE_CFG` is four *different* packets sharing
   one opcode, selected by a 4-bit sub-id at XML bit 0 (`TRMC_SUBID_COMMON` … `_ZS_CLEAR_VALUES`),
   and the render list emits all four back to back as packets 0–3. No packet in either bin list is
   sub-id-dispatched. [READ-FROM-CODE] the four `Pkt::new(P_TRMC, 9).f(0, 4, TRMC_SUBID_*)` calls.
4. **Presence/absence of the bin prologue.** The bin lists carry the frame handshake
   `NUMBER_OF_LAYERS → TILE_BINNING_MODE_CFG → FLUSH_VCD_CACHE → [OCCLUSION_QUERY_COUNTER] →
   START_TILE_BINNING`; §33 T4 and the `[v3d57]` audit both bank that order as mainline-exact. The
   render list has **no** `TILE_BINNING_MODE_CFG` and **no** `START_TILE_BINNING` at all, and its
   `FLUSH_VCD_CACHE` sits at packet 16, not packet 2. [READ-FROM-CODE] `build_bin_cl_content_geom`
   prologue; `build_rcl` — `P_FLUSH_VCD_CACHE` appears once, after the two tile loops.
5. **`CFG`/`OQ` prologue.** `m4` carries `OCCLUSION_QUERY_COUNTER` (op 92, addr 0 = disabled) and
   `CFG_BITS` (op 96); `empty` carries neither; `rcl` carries neither. This one is therefore
   **excluded as the wall by the bracket itself** — `empty` has no OQ/CFG prologue and still fetched.
   It is listed because §49.4 named it, and it is now closed. [READ-FROM-CODE] `build_bin_cl`'s
   `P_OCCLUSION_QUERY_COUNTER` / `P_CFG_BITS` emissions vs their absence in the other two.
6. **Tile-binning-mode config words.** `TILE_BINNING_MODE_CFG` (op 120, 9 bytes) is present in both
   fetching lists with identical audited fields (initial block 128 B, overflow block 64 B, 1 RT,
   32-bit BPP, 64×64 minus-one) and **absent** from the render list. Its render-side counterpart,
   `TILE_LIST_INITIAL_BLOCK_SIZE` (op 126), sits at render packet 4 — past the freeze point, so the
   bracket says nothing about it yet. [READ-FROM-CODE] `Pkt::new(P_TILE_BINNING_MODE_CFG, 9)` field
   list; `Pkt::new(P_TILE_LIST_INITIAL_BLOCK_SIZE, 2)`.
7. **List length and terminator class.** 106 bytes / `END_OF_RENDERING` (op 13) versus 14 and 76
   bytes / `FLUSH` (op 4). Length is a weak candidate — the CLE froze before consuming any of it —
   but it is the one property the next rung can move continuously, which is precisely why the next
   rung moves it. [READ-FROM-CODE] `P_END_OF_RENDERING` (documented in the file as v3d_packet.xml
   code 13, explicitly **not** `Halt`/0) vs `P_FLUSH = 4`.
8. **Alignment — excluded.** All three lists start at 4 KiB-aligned arena offsets
   (`0x35000` / `0x10000` / `0x8000`) and all three submits passed the `[v3d54]` audit with
   `BA`/`EA`/span all OK. Head-packet alignment is not a live candidate. [READ-FROM-CODE] the
   `OFF_*` constants; `[v3d54] submit` lines in all three captures.

Candidates 1, 2, 3, 4 and 6 all sit at or before render packet 1 and the bracket cannot separate
them, because every one of them differs at the *same* byte. Separating them needs a list that keeps
some of the render head and drops the rest — which is §49.9.

---

### 49.9 The prefix bisection — `UNAOS_V3D_FIRSTKICK=rclp<n>` (PI-V3D-86)

> **This section is answered.** `rclp1` (boot9) exhausted LENGTH as a variable in one cut — see
> *boot9 — the bisection answered in one cut* at the end of this section for the fold, and
> **§49.10** for the two discriminators that cut the last two candidates apart and for the
> **four-outcome composition table** that says what each pair of results concludes and does next.

§49.8 leaves five candidate properties tied at the first byte. The bisection unties them the only way
a read-only campaign can: keep the render list's head, shorten its tail, and find the shortest prefix
that still freezes the fetch — and, going the other way, the longest prefix that still fetches.

**The knob grammar.** `UNAOS_V3D_FIRSTKICK=rclp<n>`, where `<n>` is a **decimal, 1-based count of
main-list packets to keep**. `rclp1` keeps only `TILE_RENDERING_MODE_CFG(COMMON)`; `rclp7` keeps
through `MULTICORE_RENDERING_SUPERTILE_CFG`; `rclp19` keeps through `SUPERTILE_COORDINATES`. The
value joins `empty` / `rcl` / `m4` on the *existing* knob rather than adding a second variable: the
knob's presence still arms the feature, its value is still read with `option_env!` at compile time,
and one build is still exactly one experiment. A separate numeric knob was rejected for a specific
reason — the `[v3d85]` header prints the raw knob value, and `rclp7` names the whole experiment in
one token that a capture, a build log and `strings` on the image all carry identically. A bare
`rclp`, `rclp0`, a non-decimal suffix or a value above 4096 is **not recognised**: the boot runs
`empty` and says `recognised=0` on the wire, so a typo can never read as a deliberate choice.

**The list, and where the cuts fall.** The render list is 20 packets / 106 bytes, so the family is
`rclp1` … `rclp19`. Boundaries are the builder's own — `RclWriter` counts packets and stops, so a cut
is at a packet boundary *by construction* and there is no second encoding path to keep in sync:

| n | last kept packet | op | prefix bytes | list bytes (+terminator) |
|---|---|---|---|---|
| 1 | `TILE_RENDERING_MODE_CFG` (COMMON) | 121 | 9 | 10 |
| 2 | `TILE_RENDERING_MODE_CFG` (CLEAR_COLORS_PART1) | 121 | 18 | 19 |
| 3 | `TILE_RENDERING_MODE_CFG` (COLOR) | 121 | 27 | 28 |
| 4 | `TILE_RENDERING_MODE_CFG` (ZS_CLEAR_VALUES) | 121 | 36 | 37 |
| 5 | `TILE_LIST_INITIAL_BLOCK_SIZE` | 126 | 38 | 39 |
| 6 | `MULTICORE_RENDERING_TILE_LIST_SET_BASE` | 123 | 43 | 44 |
| 7 | `MULTICORE_RENDERING_SUPERTILE_CFG` | 122 | 52 | 53 |
| 8 | `TILE_COORDINATES` | 124 | 56 | 57 |
| 9 | `END_OF_LOADS` | 26 | 57 | 58 |
| 10 | `STORE_TILE_BUFFER_GENERAL` (dummy, buffer=None) | 29 | 70 | 71 |
| 11 | `CLEAR_TILE_BUFFERS` | 25 | 72 | 73 |
| 12 | `END_OF_TILE_MARKER` | 27 | 73 | 74 |
| 13 | `TILE_COORDINATES` | 124 | 77 | 78 |
| 14 | `END_OF_LOADS` | 26 | 78 | 79 |
| 15 | `STORE_TILE_BUFFER_GENERAL` (dummy, buffer=None) | 29 | 91 | 92 |
| 16 | `END_OF_TILE_MARKER` | 27 | 92 | 93 |
| 17 | `FLUSH_VCD_CACHE` | 19 | 93 | 94 |
| 18 | `START_ADDRESS_OF_GENERIC_TILE_LIST` | 20 | 102 | 103 |
| 19 | `SUPERTILE_COORDINATES` | 23 | 105 | **106 — identical to plain `rcl`** |

**The terminator.** Every truncated list gets one `END_OF_RENDERING` (op 13) appended, emitted
through the same `Pkt`/`RclWriter` encoder every other packet in the file goes through — never a
hand-placed byte. Op 13 is this file's audited render-list terminator and explicitly *not* `Halt`/0.

**The audit, and why it is not `[v3d57]`.** `[v3d57]` (`v3d57_cl_mesa_diff`) and `[v3d36]`
(`decode_cl_packets`) are **binning-CL** instruments: their opcode tables are the bin packet set, and
the `rcl` variant has never been passed to them precisely because a bin-class decoder aimed at a
render list manufactures nonsense packet names. That has not changed, and the prefix family inherits
the `rcl` treatment — the `[v3d15]` byte dump is its inventory of record. What the family adds
instead is `[v3d86]`, a truncation audit that states, per boot:

- requested `n`, clamped `n`, the list's TOTAL packet count and the family's legal range;
- prefix bytes, the terminator's opcode and length, the resulting list length, and the full list's
  length for comparison;
- **`prefix-is-a-true-prefix`** — a *measured* check, not an argument from construction: the full
  list is laid down, a position-weighted sum is taken over its first `prefix_len` bytes, the
  truncated list is laid over it and the same span is summed again. Unequal sums print
  `TRUNCATION IS NOT A PREFIX` and say in terms that the boot bisects nothing and carries no verdict;
- **the top-rung positive control** — at `n = 19` the prefix ends on `SUPERTILE_COORDINATES` and the
  appended terminator re-creates `END_OF_RENDERING`, so `rclp19` must be byte-identical to plain
  `rcl`. The line checks that and reports it on *every* prefix boot, so the family carries its own
  falsifier for free.

**Scope, unchanged.** The prefix variants are the `rcl` variant with a shorter list and nothing else:
same seeded target, same three cache publishes, same bare `CT0QBA → CT0QEA` submit, **no**
`CT0QMA`/`QMS`/`QTS`, **no** `BPOS=0`, **no** pre-kick L2T invalidate. Same read-only discipline as
every other rung — `BXCF` and `MISCCFG.QRMAXCNT` stay EXCLUDED MEASURED, `CTRSTA` stays unimplemented
(and, per §49.8, newly disarmed), no mailbox tag is sent. Witness format follows `[v3d85r1]` exactly,
with the variant named as `rclp<n>` in every line's variant field.

**Reading it.**

| Outcome | Reading |
|---|---|
| `rclp1` freezes (`CT0CA` holds `QBA`) | The wall is inside `TILE_RENDERING_MODE_CFG(COMMON)` itself — opcode 121, its 9-byte length, or its sub-id dispatch. Candidates 1/2/3 survive; 4 and 6 (the absent bin prologue) are dead, because a 10-byte list has no room for a prologue to be missing *from* |
| `rclp1` advances (`consumed=1`), some `rclpK` freezes | The head packet is legal on CT0 and the wall is at packet `K` — the bisection has a *name*, and the candidate list collapses to whatever packet `K` is. Expect `store-verified=0` on every rung cut before packets 9–10, and read it as a fetch fact only: the prose says so in terms |
| every `rclpK` fetches up to `rclp19` | `rclp19` is byte-identical to `rcl`, so a fetching `rclp19` beside a frozen `rcl` means the two boots differ in something that is **not** the list — re-take `rcl` before believing either |
| `[v3d86]` prints `TRUNCATION IS NOT A PREFIX` or the top-rung control reads 0 | Build defect. The kick below it bisects nothing; fix the builder before reading any rung of the family |

**The first two boots.** Ends first, then bisect: `rclp1` asks "is the very first render packet the
wall?", which is the single highest-information question in the family, and `rclp19` is the control
that proves the family reproduces the frozen `rcl` it is bisecting.

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=rclp1 ./arroyo kernel8
```

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=rclp19 ./arroyo kernel8
```

One variant per boot, never two — the R1 discipline is the reason the bracket above means anything.

**The `[v3d85r1]` vocabulary fix that rides this arc.** boot8's verdict line printed
*"THE M4 DRAW RETIRED AS KICK #1"* beside its own `retired=0`, `FRDONE=0` and `BFC Δ0` columns. The
prose was reading the `executed` flag, which is true when the CLE merely **walks** the list. Consumed
and closed are different facts and now have different names: the verdict line carries
`consumed=` (CT0CA reached QEA) and `frame-closed=` (retired / FRDONE / BFC moved) as separate
columns beside `executed=`, states the formula inline, and each verdict branch may only claim what
its own columns support. The `executed` column's meaning is deliberately unchanged, so no banked
reading of *that* column shifts. boot8's line under the fix reads
*"THE LIST WAS CONSUMED BUT NO FRAME CLOSED ON KICK #1"*.

The rcl class carried the same overclaim and is fixed with it: *"THE RENDER LIST EXECUTED ON CT0"* is
now reserved for `store-verified=1` — the DRAM readback `clear_job` banks its PASS on — and a list
that merely walked prints *"CONSUMED ON CT0 … BUT THE STORE DID NOT VERIFY"* and says on the wire
that it is a fetch fact only. That distinction is not cosmetic for this arc: it is the *expected*
shape of every prefix cut before the store packets (n < 9), and without it a whole family of boots
would print a success sentence for a list that was truncated before it could succeed.

**Gate.** `./arroyo check` green on both arches with the knob off; the full-knob `kernel8` build green
with `UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=rclp7`; `strings -a target/pi_baremetal/kernel8.img` shows
`[v3d86]` and the raw `rclp7` value. QEMU raspi4b models no V3D, so there is no QEMU leg for this arc
at all — the verdict is the attended metal boot.

---

#### boot9 (`rclp1`) — the bisection answered in one cut, and what it left

`rclp1` is the 9-byte `TILE_RENDERING_MODE_CFG(COMMON)` head plus one audited `END_OF_RENDERING`:
**10 bytes, prefix-identity MEASURED**, the shortest legal member of the family. It **froze exactly
like the full 106-byte `rcl`** — `CT0CA` held `QBA` and never advanced.

Set that beside the bracket and the family collapses in one step:

| boot | variant | bytes | outcome |
|---|---|---|---|
| 6 | `empty` (bin) | 14 | frame closed — **retired** |
| 7 | `rcl` (render) | 106 | froze at `QBA` |
| 8 | `m4` (bin) | 76 | walked `QBA→QEA` — **consumed**, no close |
| 9 | `rclp1` (render) | **10** | froze at `QBA` |
| 9-control | `rclp19` (render) | 106 | froze at `QBA` — **the family's own positive control, taken on metal** |

**The `rclp19` control, on the wire.** The top rung was taken in the same session and it did what the
family's design says it must. `[v3d86]` measured *both* of its own falsifiers clean —
`prefix-is-a-true-prefix=1` (weighted sum `0x53db0a1b` over the full list's first 105 bytes against
`0x53db0a1b` over the truncated list's) and `top-rung control: n==n_max rebuilds the full rcl
byte-for-byte=1`, 105 prefix bytes + a 1-byte `END_OF_RENDERING` = 106 = the full `rcl` — and the
kick then froze exactly like boot7: `max-in-span == QBA == 0x0034d000`, `consumed=0`,
`frame-closed=0`, `MMU_CTL=0x060d0c01 fault=0`. **Byte-identity is therefore proven on metal, not
argued from construction**, and the third row of §49.9's reading table (*"a fetching `rclp19` beside a
frozen `rcl`"*) is closed: the family reproduces the freeze it was built to bisect. Every `rclp<n>`
reading in this section rests on a control that was actually run.

**LENGTH is exhausted as a variable inside the rcl class.** 10 bytes and 106 bytes die identically,
while 14 bytes and 76 bytes both fetch. §49.8 candidate 7 is dead, and so is candidate 2 in the form
it was stated — a 10-byte list cannot be too long for a prefetch window that a 76-byte list clears.
Candidates 4 and 6 (the absent bin prologue) were already dead per the reading table above: a 10-byte
list has no room for a prologue to be missing *from*. What survives from §49.8 is candidate 1/3 as a
single lump — **the head opcode and its sub-id dispatch**, indistinguishable at the same first byte.

But boot9 also surfaced a candidate that was never on the §49.8 list at all, because §49.8 was a
table of *list* properties and this one is not. From the `[v3d85r1]` rcl-class note on the boot9
capture: **rcl-class kicks also omit the bin-memory arming.** No `CT0QMA`, no `CT0QMS`, no `CT0QTS`,
no `BPOS=0`, no pre-kick L2T invalidate — a bare `QBA`→`QEA` with a CPU `clean_range` publish and
nothing else. Every kick this campaign has watched *execute* carried that arming; every kick it has
watched *freeze* omitted it. List class and submit shape have been **perfectly confounded** for the
whole campaign, and no boot in the bracket can tell them apart.

**Two candidates remain, and they are not the same kind of thing:**

1. **The head opcode** — `TILE_RENDERING_MODE_CFG` (op 121) with its 4-bit sub-id dispatch, a
   property of the *list*.
2. **The bin-memory arming** — the three `CT0Q*` writes and the `BPOS=0`/L2T invalidate that the
   rcl-class submit omits, a property of the *submit*.

---

### 49.10 The two discriminators — `rclhead` and `emptyunarm` (PI-V3D-87)

> **This section is answered — OUTCOME A.** boot10 (`rclhead`) produced the tightest positive the
> campaign has ever taken on CT0: `CT0CA` advanced **exactly 2 bytes** and froze at-boundary on the
> byte where `TILE_RENDERING_MODE_CFG` begins. boot11 (`emptyunarm`) corroborates from the other
> side — the unarmed bin list retired — **but impeaches its own boot with a latched MMU fault**. See
> *boot10 and boot11 — outcome A, and the asterisk* at the end of this section, and **§49.11** for
> outcome A's named next rung.

One variant per candidate, each moving exactly one thing and holding the other fixed. Both join the
same `UNAOS_V3D_FIRSTKICK` grammar (presence arms `v3d_firstkick`, value read with `option_env!` at
compile time, one build is one experiment, an unrecognised value runs `empty` and says
`recognised=0`), and both keep the read-only discipline unchanged: `BXCF` and `MISCCFG.QRMAXCNT` stay
EXCLUDED MEASURED and are read, never written; `CTRSTA` stays unimplemented and disarmed; no mailbox
tag is sent.

**`rclhead` — the head swap.** The **full** `rcl` list with **one** innocuous, bin-class-legal packet
PREPENDED ahead of its `TILE_RENDERING_MODE_CFG` head: `NUMBER_OF_LAYERS` (`P_NUMBER_OF_LAYERS = 119`,
2 bytes, layer count 0) — the packet **both** executing lists open with, emitted through the same
`Pkt`/`RclWriter` encoder as every other packet in the file. Everything else is the `rcl` arm
verbatim: same seeded target, same three cache publishes, same bare `QBA`→`QEA` submit. The render
head has moved from list offset `+0` to list offset `+2` and nothing else changed.

The audit is `[v3d87h]`, and it is a **measurement**, the `[v3d86]` way, not an argument from
construction: the plain `rcl` is laid down and position-weight-summed over all `full_len` bytes; the
head packet is encoded into the scratch gap past the list end so its length comes from the *encoder*
and not from a constant; the list is shifted up by that length, highest byte first so the copy cannot
eat its own source; the head packet goes into the hole; then the **shifted** span is summed again.
`v3d85_arena_sum`'s weights are relative to its own `off`, so equal sums are exactly the claim *bytes
`2..N` of the built list are byte-identical to plain `rcl`'s `0..N-2`*. The line also checks the two
bytes the whole reading turns on — byte `+0` must be op 119, byte `+2` must be the plain list's own
head op and equal to `P_TRMC = 121`. Any failure prints `THE TAIL IS NOT PLAIN rcl` / `BUILD DEFECT`
and says in terms that the kick below carries no verdict.

The verdict is `[v3d87u]`'s sibling `[v3d87h]`, and it reports `CT0CA`'s furthest advance in **packet
terms** rather than as a raw address — the three addresses that mean something are `QBA`, `QBA +
head_len`, and anything past it:

| `max-in-span` | Reading |
|---|---|
| `== QBA` (never advanced) | The packet at `+0` is now the exact head both executing bin lists open with, and the fetch still froze at the first byte. **The head opcode is not the discriminator** — whatever stops this fetch stops it before any packet is decoded, so candidates 1/3 fall together. What is left is the submit |
| `== QBA + head_len` | The CLE ate the prepended packet and froze at the byte where `TILE_RENDERING_MODE_CFG` begins. **The TRMC opcode itself stalls the decode.** The fetch engine, this submit and this address range are all fine — the block decodes a bin packet on CT0 and refuses the very next one. Candidate 1/3 confirmed as a group; the arming candidate is dead, because the same unarmed submit just moved `CT0CA` two bytes |
| `> QBA + head_len`, `< QEA` | Head exonerated, wall moved to a named later offset — read the raw word (§48.1), never the wrapped difference |
| `>= QEA` | The whole list walked with a bin-legal packet in front of it. Neither candidate survives as stated; re-take plain `rcl` in the same session before believing it |
| `in-span == 0` | The thread never reached the list — the question was never put. Read `[v3d85r3]`; **not** evidence about either candidate |

**`emptyunarm` — the arming swap.** The known-good **`empty` bin list** — byte for byte, from the
*same match arm* as the `empty` variant, so there is no second construction path that could quietly
make them different lists — submitted through the **rcl-class submit shape**: no `CT0QMA`/`QMS`/`QTS`,
no `BPOS=0`, no pre-kick L2T invalidate, bare `CT0QBA`→`CT0QEA` with a CPU `clean_range` publish only.
This is `clear_job`'s/`[v3d74a]`'s submit verbatim — the `else` branch the `rcl` family already takes,
now reached with a bin list in hand. The `empty` boot and this boot differ in **three register writes
plus `BPOS=0` and the L2T invalidate, and in nothing else at all**.

Building it required splitting one flag into two. `bin_class` was doing double duty — picking the
decoder *and* the success criterion *and* the submit shape — and that conflation **was** the confound:
it made "bin list" and "armed submit" the same boolean by construction. They are now `bin_list` (list
class: which decoder reads it, and whether the DRAM store is its criterion) and `bin_arm` (submit
shape). `emptyunarm` is the one variant where they disagree. `store-verified` follows `bin_list`, so
it reads 0 on this boot — a bin list never writes the target and `verify_target` is meaningless on
one; the criterion here is `frame-closed`.

| Outcome | Reading (`[v3d87u]`) |
|---|---|
| Frame **closes** | **The arming is EXCLUDED.** It is not what separates the lists that execute from the list that does not, the confound is broken from this side, and the opcode/class hypothesis stands alone |
| Walks but no frame closes | Fetch is independent of the arming; retire is not. The arming is **not** the fetch wall, but it is load-bearing for *closing* a bin frame. The delta against the `empty` capture is the arming and nothing else |
| **Freezes at `QBA`** | **The arming is the wall.** The exact list that walks `QBA→QEA` and closes its frame *with* the arming did not advance one byte *without* it. The rcl class never froze for being a render list — it froze because nothing armed the bin memory. Every §49.8 head-packet candidate is displaced by a submit-shape defect that has been under this campaign the whole time. Next step is not another list: it is `CT0QMA`/`QMS`/`QTS` on a **render** submit |

#### How the two compose — the four outcome combinations

The two boots are independent and each has a binary reading (`rclhead`: does `CT0CA` advance past the
prepended head? `emptyunarm`: does the unarmed bin list still execute?). Four combinations, each with
its named conclusion and its next step:

| # | `rclhead` | `emptyunarm` | Conclusion | Next step |
|---|---|---|---|---|
| **A** | advances to the TRMC boundary and stops | executes (frame closes) | **The TRMC head packet is the wall, alone.** The arming is excluded from the other side, so the two candidates are cut cleanly and only candidate 1/3 survives — the CLE decodes a bin packet on CT0 and refuses op 121 | Split candidate 1 from 3: a render-class packet that is *not* sub-id-dispatched at the head. `TILE_LIST_INITIAL_BLOCK_SIZE` (op 126, 2 bytes, no sub-id) prepended in place of `NUMBER_OF_LAYERS` decides opcode-class vs sub-id dispatch. **TAKEN — this is the branch boot10/boot11 landed on; the rung is built as `rclhead126`, §49.11** |
| **B** | advances to the TRMC boundary and stops | freezes at `QBA` | **Contradiction — both readings cannot hold.** `rclhead` proves the unarmed submit can move `CT0CA` two bytes; `emptyunarm` proves an unarmed submit cannot move it at all. One of the two boots is not measuring what it claims | Do not theorise. Re-take both captures in one session, verify `[v3d87h]`'s byte-identity line and `[v3d87u]`'s submit line both read clean, and check the `[v3d85r3]` queue-to-thread transfer on each — a failed transfer on the `emptyunarm` leg explains it without any new mechanism |
| **C** | freezes at `QBA` | executes (frame closes) | **Neither candidate survives.** The head opcode is excluded (a bin-legal head still froze) and the arming is excluded (an unarmed bin list still ran). The discriminating property is neither the head packet nor the submit shape, and §49.8's whole framing is spent | Return to what is left that differs between the two: the **arena region** and the addresses. `empty` kicks from `OFF_PROBE_BIN_CL = 0x35000`, the rcl family from `OFF_RCL = 0x8000`. Kick the `empty` list from `OFF_RCL` — one boot, same list, same submit, different address |
| **D** | freezes at `QBA` | freezes at `QBA` | **The arming is the wall, and it explains both.** The head opcode is excluded by `rclhead`, and `emptyunarm` shows the freeze follows the *submit* across a change of list class. Everything §49.8 attributed to the render list was attributable to the submit that carried it | Arm a **render** submit: `CT0QMA`/`CT0QMS`/`CT0QTS` + `BPOS=0` + the pre-kick L2T invalidate ahead of a plain `rcl` kick. If it fetches, the campaign's wall is named and the fix is a driver fix, not a silicon question |

Outcomes **A** and **D** are the two that resolve the campaign; **C** is the honest null that sends it
to the address axis; **B** is the only one that is a bug report about the instruments rather than a
reading of the hardware, and it is written down precisely so it cannot be mistaken for one.

**The two boots, and their order.** `rclhead` first: it is the one that can produce a *positive*
(`CT0CA` moving two bytes is the only forward motion this campaign has ever seen on CT0 with a render
list), and A/B/C/D all read more sharply with it in hand.

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=rclhead ./arroyo kernel8
```

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=emptyunarm ./arroyo kernel8
```

One variant per boot, never two — the R1 discipline is the reason the table above means anything.

**Gate.** `./arroyo check` green on both arches with the knob off; direct
`cargo +nightly check --target aarch64-unknown-none-softfloat --features v3d_firstkick,v3d` for the
knob-on path (`./arroyo check` never compiles it — the PI-V3D-86 precedent); the full-knob `kernel8`
build green once with `UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=rclhead`;
`strings -a target/pi_baremetal/kernel8.img` shows `[v3d87h]`, `[v3d87u]` and both raw variant values.
QEMU raspi4b models no V3D, so there is no QEMU leg for this arc at all — the verdict is the attended
metal boot.

> **Note on the `[v3d87h]` scratch encode.** The head packet is encoded once into the gap between the
> list's end and `OFF_SUBLIST` purely to learn its length from the encoder. That scratch lands inside
> the final list's own span (`head_len < full_len`), so the shift overwrites it and no residue
> survives into the kicked bytes. The `[v3d87h]` byte-identity sum is taken *after* the shift and
> would catch it if it ever did.

---

#### boot10 and boot11 — outcome A, and the asterisk

**boot10 (`rclhead`, op 119) — the positive.** `[v3d87h]`'s construction audit read clean before the
kick: prepended op 119 at 2 bytes ahead of the full 106-byte / 20-packet `rcl` for a 108-byte list,
`tail-is-plain-rcl=1` (weighted sum `0x842c89ba` over the plain list's `[0,106)` against `0x842c89ba`
over the built list's `[2,108)`), head byte `@+0 = 119`, boundary byte `@+2 = 121 = P_TRMC`,
`placed=1 intact=1`. The byte-identity audit is therefore **measured, not asserted**, and the kick
below it carries a verdict.

It then produced the tightest possible positive:

| column | boot10 (`rclhead`) |
|---|---|
| `QBA` / `QEA` | `0x00990000` / `0x0099006c` |
| `head_len`, render-head boundary | 2, `QBA+2 = 0x00990002` |
| sampler | `samples=500 in-span=500 max-in-span=0x00990002` |
| advance from `QBA` | **2 bytes — exactly `head_len`** |
| `at-QBA` / `at-boundary` / `past-boundary` | `0` / **`1`** / `0` |
| `consumed` / `frame-closed` / `store-verified` | `0` / `0` / `0` |
| `MMU_CTL` / `fault` | `0x060d0c01` / **`0`** |
| poison | tile-state `0 of 64`, pool `0 of 8192` — fully intact |
| `[v3d85r3]` | `CT0EA` took `QEA` at S3 — **the queue-to-thread transfer happened**; `CT0RA` zero at every station |

`CT0CA` ate the prepended `NUMBER_OF_LAYERS` packet and stopped on the byte where
`TILE_RENDERING_MODE_CFG` begins. **THE TRMC OPCODE ITSELF STALLS THE DECODE.** The fetch engine,
this submit and this address range are all fine — the block decodes a bin-class packet on CT0 and
refuses the very next one.

**The arming candidate is dead on boot10's own evidence, and that is the load-bearing point.** The
same submit that moved `CT0CA` two bytes wrote no `CT0QMA`, no `CT0QMS`, no `CT0QTS`, no `BPOS=0` and
issued no pre-kick L2T invalidate. No missing-arming theory survives a fetch that *advanced* under
exactly the arming it is missing. §49.8 candidates 1/2/3 (opcode 121, its 9-byte length, its sub-id
dispatch) are confirmed **as a group**, and §49.10's **outcome-A branch is taken**.

**boot11 (`emptyunarm`) — demoted to a consistency control, and it read as expected.** With outcome A
already settled by boot10's internal evidence, this boot's job was no longer to decide anything: it
was to check that the two sides of the composition table agree. It did. `[v3d87u]`:
`frame-closed=1 consumed=1 BFC 0x00000000->0x00000001 (Δ1) retired=1`, `QBA=0x0097c000
QEA=0x0097c00e` — *"THE EMPTY FRAME RETIRED WITHOUT ANY BIN-CLASS ARMING … THE ARMING IS EXCLUDED as
the wall"*. The arming is excluded from the second side too, and the pair lands squarely in row **A**.

Had it instead **frozen at `QBA`**, the pair would have been row **B** — the composition table's
contradiction row, and *not* a hardware finding: `rclhead` proves an unarmed submit can move `CT0CA`
two bytes, so an `emptyunarm` freeze would prove the two boots cannot both be measuring what they
claim. Row B's instruction stands unchanged and unused: do not theorise, re-take both captures in one
session, verify `[v3d87h]`'s byte-identity line and `[v3d87u]`'s submit line, and check the
`[v3d85r3]` queue-to-thread transfer on each leg.

**The asterisk — boot11 impeached its own boot, and two witnesses disagreed on one wire.** On the
same capture `[v3d85r1]` printed `MMU_CTL=0x061d1c01 fault=1` and *"AN MMU FAULT LATCHED across the
boot's first kick — instrument fault, not a first-kick verdict. Fix it before citing this rung;
nothing here discriminates anything"*, while `[v3d87u]` printed a clean, quotable exclusion from the
same registers. **The R1 discipline governs: boot11 is corroboration-with-an-asterisk and outcome A
stands PRIMARILY on boot10's internal evidence**, which is fault-free (`0x060d0c01`, `fault=0`) and
does not need boot11 at all. Two verdict lines disagreeing about whether their own boot is citable is
a defect in the witnesses, so **PI-V3D-88 gives `[v3d87u]` and `[v3d87h]` the same latched-fault gate
`[v3d85r1]` has** — both now check the latch first and refuse to claim anything across a faulting
boot, and both print `mmu-fault-latched=` in their own columns.

**NEW LEAD — where do the writes of an UNARMED frame-close go?** The fault boot11 latched is not
noise and is not explained by anything in this section. The post-kick line reads it out in full:

```
MMU fault-latch CLEARED (v3d85 post-kick) — was CTL=0x061d1c01
  (PT_INVALID=1 WRITE_VIOLATION=1 CAP_EXCEEDED=0)
  VIO_ADDR=0x0000000e VIO_ID=0x00000020 (client PTB @ VA 0x00000070)
```

A **write violation** and a **PT_INVALID**, charged to the **PTB**, at a virtual address of `0x70` —
on a boot where `CT0QMA`/`CT0QMS` were deliberately never programmed. That is exactly the shape of a
close path writing through never-programmed memory targets: the frame closed (`BFC Δ1`) and the PTB
tried to put something somewhere, and with no tile-alloc base and no tile-state base loaded, the
somewhere was whatever those registers happened to hold. Consistent with `[v3d85r1]`'s own
`wrote-any=0` and fully-intact poison — nothing landed in the arena because the write did not go to
the arena. This deserves its own rung later: **program nothing, close a frame, and read `CT0QMA` /
`CT0QMS` / `CT0QTS` / the PTB's fault address across it** to name where an unarmed close aims. It is
also the first time this campaign has a *positive* MMU fault to work with rather than a silent
nothing, and §26's hub-INT and §36's PTB-frame instruments both point at it.

> **BUILT — see §49.12.1.** PI-V3D-95 is that rung, armed by `UNAOS_V3D_UNARMCLOSE=1` and default
> OFF, and it carries the `[v3d85r1]` instrument fix this section also owed: the R1 line now prints
> the violation pair and its decode in its own columns, so "fix it before citing this rung" names the
> fault it is refusing on instead of leaving it to a later hygiene line.

> **FLOWN, and PARTLY ANSWERED — see §49.20** (boot4/PA40, 2026-08-17). The rung reproduced boot11
> exactly: a bin frame closed under the unarmed submit (`BFC 0→1`), the PTB issued a write, the MMU
> refused it (`PT_INVALID=1 WRITE_VIOLATION=1`, client PTB, `VIO_ADDR=0x0000000e VIO_ID=0x00000020`,
> `VA 0x00000070` — byte-identical raw words to this section's), and the `[v3d62]` catcher absorbed
> it (48 of 1024 sentinel words gone). **The lead's own fork did not resolve, and this section's
> reading of it must not be quoted as settled:** the fork needed a *different* `CT0QMA`/`CT0QMS`/
> `CT0QTS` residue, and boot4's residue — like boot11's — was **zero**, under which "structural
> `0x70`" and "base `0` + offset `0x70`" predict the same VA. §49.20.2 V5 shows a second PTB pointer
> on that boot doing base-plus-offset arithmetic on exactly that zero base (`BPCA = QMA + 0x3000`,
> `BPCA + BPCS ≡ QMA + QMS` mod 2³²), which favours the base-relative reading. §49.20.6 R1 is the cut.

---

> **APPENDED CORRECTION (§49.22.4 item 3, PI-V3D-97, 2026-08-18).** boot11's measurement is unchanged
> and was correctly taken. Its `VA 0x70` now has a mechanism: §49.22.2 W2 landed the close-time pool
> write inside byte span `[CT0QMA + 0x0, CT0QMA + 0x80)`, and boot11's `CT0QMA` was zero, so `0x70` is an
> address **inside that write's own span** on a zero base. Nothing in the four-outcome composition above
> falls; the row acquires a cause. (What is *not* established is that `0x70` is the write's tail — the
> MMU reports one address per latch window and its selection rule is unknown.)

### 49.11 The last split — `UNAOS_V3D_FIRSTKICK=rclhead126` (PI-V3D-88)

> **This section is answered — the `== QBA` row. RENDER-CLASS OPCODES AS A CLASS STALL CT0's
> DECODER.** boot12 (`rclhead126`) froze at `QBA` with `advance from QBA = 0 bytes`, on a fault-free
> boot, where boot10's bin-class op 119 advanced 2. The sub-id dispatch and the TRMC encoding are
> both exonerated; what is left is the class. **CT0 is bin-only silicon-side and the fix is a driver
> fix** — see *boot12 — the class verdict* at the end of this section, the standing rule at
> [**THE CT0/CT1 CLASS LAW**](#the-ct0ct1-class-law--render-lists-run-on-ct1-never-on-ct0) at the top
> of this file, the §49.11a audit of what that retracts, and §49.12 for the executable proof.

§49.10 outcome A says the wall is the head packet and names it: `TILE_RENDERING_MODE_CFG`, opcode
121. But "opcode 121" is not one property. It is **two**, and boot10 could not tell them apart
because op 121 has both:

1. **It is a RENDER-class opcode** — one of the high-numbered packets that only appear in render
   control lists. A decoder that rejects the class rejects op 121 for being render-class.
2. **It is SUB-ID-DISPATCHED** — its first 4 bits are a sub-id field selecting COMMON /
   CLEAR_COLORS_PART1 / COLOR / ZS_CLEAR_VALUES, four different packet bodies behind one code. A
   decoder that cannot perform that second dispatch rejects op 121 for being dispatched.

These are the last two candidates in the campaign and they imply **completely different fixes**, so
they get one boot.

**The variant.** `UNAOS_V3D_FIRSTKICK=rclhead126` — the **full** `rcl` list with
`TILE_LIST_INITIAL_BLOCK_SIZE` (`P_TILE_LIST_INITIAL_BLOCK_SIZE = 126`, **2 bytes, render-class, NO
sub-id dispatch**) prepended INSTEAD of `NUMBER_OF_LAYERS`. Everything else is the `rclhead` arm
verbatim: same seeded target, same three cache publishes, same bare `CT0QBA → CT0QEA` submit, same
`OFF_RCL` address. The render head moves from list offset `+0` to `+2` and nothing else changes.

Op 126 is the right packet and there is not a second candidate for the job:

- it is **render-class**, so the class is held FIXED against boot10's bin-class op 119;
- it has **no sub-id field**, so the dispatch is the one thing REMOVED;
- it is **2 bytes**, identical in length to op 119, so `head_len` and the boundary arithmetic are
  unchanged and the two boots' `[v3d87h]` lines are directly comparable;
- and it is a packet **the plain `rcl` list already carries**, at main-list packet 5 (§49.9's
  truncation table). The prepended bytes are not a new encoding anyone has to trust — they are bytes
  the kicked list already contains, moved to the front.

**Why `rclhead126` and not a new knob.** The head-swap axis becomes a **family**, `rclhead<op>`, where
`<op>` is the prepended packet's decimal opcode — the same grammar decision §49.9 made for `rclp<n>`
and for the same reason: the experiment's whole variable is a number, and putting that number in the
knob token means the capture, the build log and `strings` on the image all name the experiment with
one identical word. Three properties of the naming are deliberate:

- **bare `rclhead` stays `rclhead` and stays op 119.** boot10's banked token is unchanged and cannot
  be re-read as something else; `rclhead119` is accepted as its explicit spelling and prints the same
  token, because it is the same experiment down to the byte.
- **only opcodes this file has an audited encoding for are recognised** — 119 and 126, nothing else.
  A head packet is not a byte pattern to be invented at a knob. `rclhead42` is *rejected*: the boot
  runs `empty` and says `recognised=0`, exactly as `rclp0` does.
- the alternative names were worse. `rcltlibs` hides the number the rung is about; `rclhead2` reuses
  `rclp`'s "count" reading for an opcode; a second knob breaks the one-build-is-one-experiment rule
  the whole ladder rests on.

**The audit — `[v3d87h]`, measured, plus one more check.** The construction audit is boot10's, verbatim
in method: lay the plain `rcl` down and position-weight-sum all of it; encode the head packet into the
scratch gap past the list end so its length comes from the *encoder*, not a constant; shift the list
up by that length, highest byte first so the copy cannot eat its own source; place the head; re-sum
the **shifted** span. Equal sums are exactly the claim *bytes `2..N` of the built list are
byte-identical to plain `rcl`'s `0..N-2`*. The head byte `@+0` must read 126 and the boundary byte
`@+2` must read the plain list's own head op and equal `P_TRMC = 121`.

PI-V3D-88 adds the **in-list twin check**, and only on op 126: the two prepended bytes are compared
byte-for-byte against the op-126 packet the shifted list still carries at main-packet 5, whose offset
comes from `build_rcl_limited(4)` — the builder's own truncation primitive — rather than from a
constant. Equal bytes are the claim *the packet in front of the list is the list's own packet, moved,
not a new encoding*, and that is what makes the negative branch mean **class** rather than **my
encoding of op 126**. Any failure prints `BUILD DEFECT` and says in terms that the kick carries no
verdict. Op 119 has no in-list twin, so the check reports `has-twin=0` and gates nothing.

**The fault gate, new to both verdict lines.** `[v3d87h]` and `[v3d87u]` now check the latched MMU
fault before claiming anything, and print `mmu-fault-latched=` in their own columns. boot11 is why —
see §49.10's asterisk. A boot whose memory path faulted cannot tell a decode stall from a
fault-induced stop, and the R1 discipline line was already saying so while the sibling line was not.

**Reading it — both outcomes, and what each means for the REAL fix.**

| `max-in-span` | Verdict, and the fix direction it names |
|---|---|
| `== QBA + head_len` (advanced 2, froze at TRMC) | **The sub-id dispatch mechanism (or op-121 specifically) is the stall; render-class opcodes as a class are decodable on CT0.** A render-class opcode was decoded on the bin thread and the very next packet — the one that is sub-id-dispatched — was not. The class hypothesis is DEAD. **Fix direction:** this is an *encoding* problem, not a silicon boundary. The `TRMC` encoding gets re-checked **field by field against the audited v42 reference** — sub-id placement and width, the 4-bit boundary, the 9-byte length, every `.f(start,width,value)` in `build_rcl_limited`'s four TRMC packets — and **§48's offset lens applies**: §48 established that the stall offset is not where the naive reading puts it, so the field offsets get read the same careful way |
| `== QBA` (never advanced) | **Render-CLASS opcodes as a class stall CT0's decoder, and op 126 joining op 121 in the freeze proves it is the class, not the packet.** Op 126 has no sub-id, so the dispatch is exonerated; op 126 is not TRMC, so the TRMC encoding is exonerated; what is left is the one property they share — render-class opcodes on the BIN thread. **Fix direction:** the campaign confirms **CT0 is bin-only silicon-side**, and the fix is a *driver* fix with nothing to debug in the encoding at all: this driver must never feed CT0 a render list. The historical `probe_job` construction gets audited for exactly that mistake, and so does every path that reaches `v3d_bin_job_run` with an `OFF_RCL`-class list in hand |
| `>= QEA` | The whole list walked with a render-class packet in front of it. Neither reading above survives; re-take plain `rcl` and boot10 in the same session before believing it |
| `> QBA + head_len`, `< QEA` | Head exonerated, wall moved to a named later offset — read the RAW word (§48.1), never the wrapped difference, against §49.9's packet table |
| `in-span == 0`, `samples == 0`, or `mmu-fault-latched=1` | The question was never put, or was put on a boot that faulted. **Not a verdict in either direction** — read `[v3d85r3]` and `[v3d85r1]`'s fault columns and re-take |

Either way §49 ends with a **named** defect rather than a candidate list, which is what the whole
`rclp`/`rclhead` ladder was built to produce.

**Scope, unchanged.** Read-only discipline exactly as every rung in this family: `BXCF` and
`MISCCFG.QRMAXCNT` stay **EXCLUDED MEASURED** and are read, never written; `CTRSTA` (R6) stays
unimplemented and disarmed; no mailbox tag is sent; the knob grammar is still
`UNAOS_V3D_FIRSTKICK=<value>` with the value read via `option_env!` at compile time, one build is one
experiment, and an unrecognised value runs `empty` and says `recognised=0` on the wire.

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=rclhead126 ./arroyo kernel8
```

One variant per boot, never two — the R1 discipline is the reason the table above means anything.

**Gate.** `./arroyo check` green on both arches with the knob off; direct
`cargo +nightly check --target aarch64-unknown-none-softfloat --features v3d_firstkick,v3d` for the
knob-on path (`./arroyo check` never compiles it — the PI-V3D-86/87 precedent), **canary-verified
non-vacuous once** by injecting a type error into `v3d88_emit_head_pkt`, seeing it caught, and
restoring; the full-knob `kernel8` build green once with `UNAOS_V3D_DEEP=1
UNAOS_V3D_FIRSTKICK=rclhead126`; `strings -a target/pi_baremetal/kernel8.img` shows `[v3d87h]`,
`[v3d87u]` and the raw `rclhead126` value. QEMU raspi4b models no V3D, so there is no QEMU leg for
this arc at all — the verdict is the attended metal boot.

---

#### boot12 — the class verdict, CONFIRMED

**The construction audit read clean before the kick, and it is measured, not asserted.** `[v3d87h]`,
verbatim from the wire:

> prepended op=126 (RENDER-class, NOT sub-id-dispatched, 2 bytes, through the same `Pkt`/`RclWriter`
> encoder as every other packet in this file) ahead of the FULL rcl (106 bytes, 20 main packets) =>
> list bytes=108 | byte-identity audit (MEASURED, not asserted): weighted sum over the plain list's
> `[0,106)` = `0x0b61a528` vs the same weights over the built list's `[2,108)` = `0x0b61a528`,
> `tail-is-plain-rcl=1` | head byte `@+0` = 126 (expected 126, `placed=1`) | boundary byte `@+2` = 121
> (expected the plain rcl's own head op 121 = `P_TRMC=121`, `intact=1`) | in-list twin: `has-twin=1`
> main-packet-5 `@+2+36=0x8026` op=126 `bytes-equal-to-the-prepended-packet=1`

The in-list twin check is what makes the negative mean **class** and not *our encoding of op 126*:
the two bytes in front of the list are byte-for-byte the op-126 packet the same list already carries
at main-packet 5. The head is the list's own packet moved to the front.

**The kick froze at the first byte.** `[v3d87h]`'s verdict line, verbatim:

> prepended head op=126 (RENDER-class, no sub-id dispatch) | `QBA=0x0034b000` `head_len=2`
> render-head boundary=`QBA+2=0x0034b002` `QEA=0x0034b06c` | sampler `samples=500` `in-span=500`
> `max-in-span=0x0034b000` (advance from QBA = **0 bytes**) | `at-QBA=1` `at-boundary=0`
> `past-boundary=0` | `mmu-fault-latched=0` (this line is GATED on it, exactly as `[v3d85r1]` is) —
> **CT0CA HELD QBA AND NEVER ADVANCED** … It froze where op 119 advanced. **RENDER-CLASS OPCODES AS A
> CLASS STALL CT0's DECODER, AND OP 126 JOINING OP 121 IN THE FREEZE PROVES IT IS THE CLASS, NOT THE
> PACKET**

The boot12 column, against boot10 in the same terms:

| column | boot10 (`rclhead`, op **119**, bin-class) | boot12 (`rclhead126`, op **126**, render-class) |
|---|---|---|
| `QBA` / `QEA` | `0x00990000` / `0x0099006c` | `0x0034b000` / `0x0034b06c` |
| `head_len`, boundary | 2, `QBA+2` | 2, `QBA+2` (**identical arithmetic**) |
| `tail-is-plain-rcl` | 1 (`0x842c89ba` both sides) | 1 (`0x0b61a528` both sides) |
| in-list twin | `has-twin=0` (op 119 has none) | **`has-twin=1`, bytes-equal=1** |
| sampler `max-in-span` | `0x00990002` | `0x0034b000` |
| **advance from `QBA`** | **2 bytes** | **0 bytes** |
| `at-QBA`/`at-boundary`/`past-boundary` | `0`/`1`/`0` | **`1`**/`0`/`0` |
| `MMU_CTL` / `mmu-fault-latched` | `0x060d0c01` / `0` | `0x060d0c01` / **`0`** |
| `CT0CS` S4 (`[v3d85r7]`) | — | `0x00000018` (`CTERR=1 CTSUBS=1 CTRUN=0`) |
| `[v3d85r3]` queue→thread | transfer happened | **`CT0EA` took `QEA` at S3, `moved=1`; `CT0RA` zero at every station** |
| poison | intact | tile-state `0 of 64`, pool `0 of 8192` — fully intact, write-back completed |
| `retired`/`consumed`/`frame-closed` | `0`/`0`/`0` | `0`/`0`/`0` |

**Two properties went in, one came out.** Op 126 has no sub-id field and died anyway, so the
**sub-id dispatch is exonerated**. Op 126 is not `TILE_RENDERING_MODE_CFG` and died identically, so
the **TRMC encoding is exonerated** — there is nothing to re-check field-by-field against the v42
reference, and the `== QBA + head_len` row's whole fix direction is void. What is left is the single
property op 121 and op 126 share: **they are render-class opcodes, and they were put on the BIN
thread.**

**The bracket is complete, and it is the whole argument in eight rows.** Every member of the family
was one boot, one variant, same block, same MMU table, same arena:

| variant | list class | head packet | outcome |
|---|---|---|---|
| `empty` | bin | — | **retired** — frame closed |
| `emptyunarm` | bin | — | **retired** (unarmed submit; asterisked by a latched fault, §49.10) |
| `m4` | bin | — | **consumed** — `CT0CA` walked `QBA→QEA` |
| `rcl` | render | op 121 (TRMC) | **frozen at packet 0** |
| `rclp1` | render | op 121 (TRMC) | **frozen at packet 0** — a 10-byte list froze like the 106-byte one |
| `rclp19` | render | op 121 (TRMC) | **frozen at packet 0** |
| `rclhead` (119) | render tail, **bin-class head** | op 119 | **advanced exactly 2, then froze at TRMC** |
| `rclhead126` | render tail, **render-class head** | op 126 | **frozen at `QBA`, 0 bytes** |

Length is not the variable (`rclp1` vs `rcl`). The submit shape is not the variable (`emptyunarm`
retired without any arming; `rclhead` advanced without it). The head *opcode* is the variable, and
the head opcode's **class** is what it selects on: a bin-class head is eaten, a render-class head is
not, and the very first render-class byte is where the decoder stops.

**The corroborating asymmetry, on the same boot.** `[v3d58] xengine`, verbatim: *"RENDER(CT1) ran=1
verified-store=1 RFC=0x00000001 | BIN(CT0) retired=0 wrote-any-arena-byte=0 BFC=0x00000000 | SHARED at
bin time: MMU_CTL=0x060d0c01 (ENABLE=1 faults=0x0) PT_PA_BASE=0x00000333 L2TCACTL=0x00000000
arena=0x343000+0x40000 — ASYMMETRY CONFIRMED."* CT1 completed a frame and landed a byte-verified store
on the **same block, same MMU table, same L2T config, same clock and same arena** in which CT0 refused
the first byte of a render list. The two threads are not equivalent, and the working one is the one
the render list belongs on.

**What this names.** §49 ends where it was built to end: with a **named defect**, not a candidate
list. The defect is ours. CT0 is bin-only silicon-side; this driver has been handing it render-class
control lists; the fix is a driver fix and there is nothing left to debug in the encoding. The
standing rule is written at the top of this file
([**THE CT0/CT1 CLASS LAW**](#the-ct0ct1-class-law--render-lists-run-on-ct1-never-on-ct0)), the audit
of what it retracts is §49.11a, and the executable proof is §49.12.

---

### 49.11a The CT0 submission audit — every site, and what the law retracts (PI-V3D-88 M2)

§49.11's fix direction names two jobs: audit `probe_job`'s historical construction, and audit *every*
path that reaches CT0 with a render-class list. Both were done by **reading the builders**, not by
reading comments — every row's packet inventory below is the opcode sequence its builder function
actually emits.

#### The inventory — all ten `CT0QBA` write sites

| # | line | function | list source | packet inventory (opcodes, in order) | class | role |
|---|---|---|---|---|---|---|
| 1 | 6214 | `probe_job` | `build_bin_cl_generic(OFF_PROBE_BIN_CL, …)` → `build_bin_cl_content_geom(…, Full)` | 119, 120, 19, 92, 6, 96, 107, 108, 110, 111, 71, 64, 36, 4 | **BIN** | witness/probe (attribute-DMA) |
| 2 | 6709 | `submit_bisect_rung_geom` | `build_bin_cl_content_geom(…, content)` | `Empty` = 119, 120, 19, 6, 4 · `Full` = row 1 | **BIN** | witness (§22 empty-frame bisection) |
| 3 | 6820 | `submit_bisect_rung_geom` re-kick | same buffer, re-latched | as row 2 | **BIN** | witness (conditional resubmit) |
| 4 | 9934 | `v3d71_mainline_geometry` | `build_bin_cl_content_geom(…, Empty)` | 119, 120, 19, 6, 4 | **BIN** | witness (§42 address geometry) |
| 5 | 10119 | `v3d73_mainline_submit` | `build_bin_cl_content_geom(…, Empty)` | 119, 120, 19, 6, 4 | **BIN** | witness (§44.3 mainline-exact submit) |
| **6** | **10278** | **`v3d74_thread_swap`** (leg A) | **`build_rcl()`** | **121×4, 126, 123, 122, 124, 26, 29, 25, 27, 124, 26, 29, 27, 19, 20, 23, 13** | **RENDER** | §45 discriminator — *deliberate* |
| **7** | **10523** | **`v3d75_kick_probe`** | **`build_rcl()`** | identical to row 6 | **RENDER** | reusable "did it work?" probe |
| 8 | 10692 | `triangle_job` | `build_bin_cl(num_attrs)` | as row 1 | **BIN** | production M4 draw |
| 9 | 12214 | `kick_bin_render` | callers pass `build_bin_cl_at(…)` (M5/M6/M7) | 119, 120, 19, 92, 6, 71, {64, 36}×draws, 4 | **BIN** | production/compositor battery |
| **10** | **13956** | **`v3d85_firstkick_rung`** | knob-selected | `empty`/`emptyunarm`/`m4` = **BIN**; `rcl`/`rclp<n>`/`rclhead<op>` = **RENDER** | **both** | §49 firstkick — *deliberate* |

#### `probe_job` is EXONERATED — the headline finding, and it is a negative

**`probe_job` does not feed CT0 a render-class list, and never did.** §49.11's fix direction
anticipated that it might; the code says otherwise. The chain is three lines and fully traceable:
`build_bin_cl_generic(OFF_PROBE_BIN_CL, OFF_PROBE_SHADREC, num_attrs)` fills the buffer,
`bin_ba = arena_phys() + OFF_PROBE_BIN_CL` takes its address, and that address goes to `CT0QBA`.
`build_bin_cl_generic` forwards to `build_bin_cl_content(…, BinContent::Full)` →
`build_bin_cl_content_geom`, and reading that builder end to end, **every opcode it can emit on any
`BinContent` arm is bin-class or class-neutral**: 119, 120, 19, 92, 6, 96, 107, 108, 110, 111, 71,
64, 36, 4. There is no 121, no 126, no 122/123/124, no 25/26/27/29, no 20/23, no 13. `probe_job`
touches `CT0QBA` exactly once; there is no second kick hidden in it.

> **One opcode is not a discriminator.** `P_FLUSH_VCD_CACHE` (19) is emitted by
> `build_bin_cl_content_geom` **and** by `build_rcl_limited`. It appears in both classes. Do not read
> it as render contamination in a bin list.

The same holds for `triangle_job` (row 8) and `kick_bin_render` (row 9) — **the production draw path
and the compositor battery have always been bin-class on CT0.** The class law therefore **does not
explain the original empty-bin defect** (§6, §22): those lists are the right class for the thread
they are on, and `empty` and `m4` both prove bin-class lists execute on CT0. **The bin wall is a
separate, still-open defect and nothing in §49.11 closes it.** Saying so is the point of this audit —
a law that explained everything would be explaining too much.

#### Where render-class content genuinely reaches CT0 — three sites, all instruments

None of the three is in a production path, and two of the three were correct science:

- **Row 6, `v3d74_thread_swap` leg A (§45)** — the RCL on CT0 *is* the experiment. It asked
  thread-vs-class, and boot12 has now answered it: **class**. Not a mistake; it is the law's founding
  observation. Introduced by **`8f78cd19`** *"video/v3d: V3D-74 — thread 0 or bin class? the swap
  discriminator"*.
- **Row 10, `v3d85_firstkick_rung` (§49)** — the whole `rcl`/`rclp<n>`/`rclhead<op>` family is
  deliberately render-class-on-CT0, one variant per boot, and it is the instrument that produced the
  law. Not a mistake.
- **Row 7, `v3d75_kick_probe` — this is the historical mistake.** Introduced by **`282319f1`**
  *"video/v3d: V3D-75 — the fabric condition (mid-bin dump diff: ENABLE_QPU + the M_CTRL
  transplant)"*, it compacted §45's *deliberately wedged* arrangement into a **reusable,
  general-purpose "did my fix work?" success criterion** — and then six unrelated experiments were
  judged by it. Under the class law that probe returns `false` **unconditionally**, for a reason that
  has nothing to do with whatever the experiment changed. It is a control that cannot pass.

Rows 6 and 7 also **conflate class with arming** (both strip `CT0QMA`/`QMS`/`QTS` along with
switching to a render list), which is the confound §49.10 had to break with `emptyunarm`. Only row
10's `empty`/`emptyunarm` pair separates the two axes.

#### The retraction ledger

A negative measured through `v3d75_kick_probe` is a statement about the probe, not about the
experiment. Marked in place:

| verdict | section | status | why |
|---|---|---|---|
| **V3D-75a** — `SET_ENABLE_QPU(1)` does not free thread 0 | §46 | **RETRACTED-BY-CLASS-LAW** | read through `v3d75_kick_probe` (`v3d75a`). The probe could not have retired whatever ENABLE_QPU did |
| **V3D-75b** — the `M_CTRL 0x4040` transplant is "a passenger, not the condition" | §46 | **RETRACTED-BY-CLASS-LAW** | same probe (`v3d75b post-transplant`). The `M_CTRL` readback itself (did the bits hold?) is a register fact and **STANDS** |
| **V3D-77a/b** — the core- and hub-unnamed register writes change nothing | §46.1–46.3 | **RETRACTED-BY-CLASS-LAW** | `v3d77a core-unnamed` / `v3d77b hub-unnamed`, both `v3d75_kick_probe` |
| **V3D-80** — `NOTIFY_DISPLAY_DONE` "did not free thread 0"; *"the ARM-testable universe is CLOSED"* | §46.5 | **RETRACTED-BY-CLASS-LAW** | `v3d80 post-handover`, same probe. **The closure claim falls with it** — the ARM-testable universe was never closed; a whole thread was untested. The `core+0x68` movement reading is register-channel evidence and **STANDS** |
| **V3D-81** — the reply-less NOTIFY family, the `kick=` column only | §47 | **RETRACTED-BY-CLASS-LAW (kick column only)** | `v3d75_kick_probe(kick_label)`. §47.1a's **control-INDEPENDENT** evidence — `acted-on-buffer`, `buffer-rejected`, `doorbell`, `reg-moved`, `fw-moved`, the display-liveness leg — never used the kick and **STANDS in full**. §47.1a's own distinction is what saves it |
| **V3D-74 leg A** — the RCL froze on CT0 | §45.1 | **STANDING (reinterpreted)** | the observation is sound and is now the law's founding datum. §45's question "thread 0 or bin class?" is **answered: CLASS** |
| **V3D-74 leg B** — `bcl` on CT1 is a documented no-op | §45.2 | **STANDING** | not measured through the probe |
| **V3D-48 / §22 empty-frame bisection**, §42 (V3D-71), §44 (V3D-73/73s), §48 (V3D-82) | — | **STANDING** | every one submitted a **bin-class** list to CT0. Right class, right thread; their negatives are genuine bin-path facts |
| **§1–§21, §29–§31 shader/CL/encoding facts** | — | **STANDING** | measured through `probe_job`/`triangle_job`, both bin-class |
| **§49.8 first-kick bracket**, bin-class rows (`empty`, `emptyunarm`, `m4`) | §49.8 | **STANDING** | bin lists on the bin thread; these are the law's positive controls |

**What the retractions cost, stated plainly.** Five banked negatives across §46 and §47 turn out to
have been measured through an instrument that could not pass, and one of them (**V3D-80**) carried a
*closure* claim — that the ARM-testable universe was exhausted — which is now known to be false. The
firmware/fabric hypotheses those boots were meant to kill are **not** revived as likely; they are
returned to **untested**. Retesting them is cheap and is a natural rider on the next arc: swap
`v3d75_kick_probe`'s body to the CT1 submission path (§49.12) and every one of those legs gets a
success criterion that *can* return true.

---

### 49.12 The law, executed — `UNAOS_V3D_FIRSTKICK=rclct1` (PI-V3D-89)

§49.11 ends with a driver law, and a law that has never been run is still an argument. This variant
runs it.

**The variant.** `rclct1` — the **plain `rcl` list**, byte for byte the `rcl` arm's, from the same
`build_rcl()` call, at the same `OFF_RCL` address, with the same seeded target and the same three
cache publishes — submitted on the **CT1 register file**. `CT1QBA` → `CT1QEA` instead of `CT0QBA` →
`CT0QEA`. That is the entire difference. Against the `rcl` boot it is a **one-variable experiment**,
and the variable is the thread.

**The arming question, answered from the reference rather than guessed.** CT1 takes no arming and
*cannot*: `CT0QMA` (0x170), `CT0QMS` (0x174) and `CT0QTS` (0x15c) are tile-allocation and tile-state
memory — the **binner's** outputs — and the CLE register map carries no CT1 counterpart for any of
them. Mainline programs all three exclusively in `v3d_bin_job_run`; its render path writes `CT1QBA`
then `CT1QEA` and nothing else. **This driver's own `clear_job` is that sequence transcribed** — its
`CT1QBA`→`CT1QEA` pair cites `v3d_regs.h` / the `v3d_gem` submit path in place — and it is the leg
that banks a byte-verified store every boot at M3. So the submit is bare `CT1QBA`→`CT1QEA` with a CPU
`clean_range` publish and no GPU-cache invalidate, which is **also the `rcl` arm's shape**. The pair
therefore holds list, address, publishes *and submit shape* fixed, and moves only the register file.

**Why this is not circular, and what it adds over M3.** `clear_job` already retires this list on CT1,
including on this very boot a few hundred lines above the kick — `[v3d89]`'s `S0 pre-program` line
prints that result as the **in-boot control**. What `rclct1` adds is that the `rcl` boot and the
`rclct1` boot become the **same instrument**: same rung, same builder call, same fault gate, same
poison detectors, same verdict grammar. Before it, "CT0 freezes, CT1 retires" spanned two different
code paths and therefore two different sets of incidental differences. Now it is one variable in one
harness — which is what this ladder has demanded of every other claim it makes.

**The witness family — `[v3d89]`.** Five lines, in the established style:

| line | what it states |
|---|---|
| `[v3d89] construction` | list length, sub-list length, that it is the plain `rcl` from the same `build_rcl()` call, and that nothing but the register file differs |
| `[v3d89] arming` | the paragraph above, on the wire — *why* there is nothing to write, not merely that nothing was written |
| `[v3d89] S0 pre-program` | `CT1CS`/`CT1CA`/`CT1EA` before the kick, plus M3's `ran=`/`verified=` as the in-boot control |
| `[v3d89] submit` | the `[v3d54]` audit on CT1's registers — intended vs latched `[BA,EA)`, so a non-retire is a frame fact and not a submission artifact |
| `[v3d89] cle-progress` | the CT1 counterpart of the `[v3d73]` sampler (~1 ms cadence, ~0.5 s backstop): `CT1CA`'s furthest advance **inside** `[QBA,QEA]`, `in-span`, `eq-QBA`, `changes`, `first_move` |
| `[v3d89] rclct1 verdict` | `store-verified` / `consumed` / `frame-closed` / `executed`, `RFC`/`BFC` deltas, the two BIN-output poison regions as **negative controls**, and `mmu-fault-latched` |

The verdict is **fault-gated exactly as `[v3d85r1]` and `[v3d87h]` are** — §49.10's asterisk is why —
and it refuses to claim anything on an unsound submit or a zero-sample wait.

> **A witness may not print a column it cannot interpret.** On `rclct1` the entire CT0 station
> battery — `[v3d85r1]`, `[v3d85r2]`, `[v3d85r3]`, `[v3d85r7]`, `[v3d58]`, `[v3d73]` — is **skipped**,
> because every one of them reads CT0 and this boot never kicks CT0. Running them would print a page
> of columns that are all correctly zero and all meaningless. The boot-complete line says so in terms.

**Both outcomes, pre-written.**

| outcome | verdict, and what it means |
|---|---|
| **`executed` = 1** (store-verified, or frame-closed, or consumed) | **THE CT0/CT1 CLASS LAW IS CONFIRMED EXECUTABLE.** The exact list bytes that held `CT0CA` at `QBA` and never advanced were fetched, stepped and closed by CT1 from the same address with the same publishes and the same submit shape. The law stops being an inference from a freeze and becomes a working path; the driver fix is **specified rather than proposed** |
| **`executed` = 0** (CT1 froze too) | **The class law STANDS on boot12's CT0 evidence, which does not depend on this kick** — op 126 and op 121 still froze CT0 where bin-class op 119 advanced, and M3 still retires this same list on CT1 every boot. What it says instead is that **CT1 arming needs its own bracket**, because something about *this* kick differs from `clear_job`'s and it is not the list, the address, the publishes or the submit shape — all four are held fixed by construction. **The named differences, in order:** (1) **position** — this is a *second* CT1 kick on a boot where M3 already ran one to completion, so CT1 may carry end-of-frame state `clear_job` never sees, the exact mirror of the §49.3 confound that started this ladder; (2) the intervening L2T/SLC cache traffic and poison writes this rung makes between M3 and the GO; (3) frame-count state, `RFC` having already advanced once. Take them in that order — **position first**, by moving the kick ahead of M3 exactly as R1 moved the CT0 kick ahead of `probe_job` |

**METAL VERDICT — boot13, 2026-08-03, capture `pi4-r23s1x` (mark "R23 boot13"): THE EXECUTED
BRANCH FIRED.** `[v3d89] rclct1 verdict — store-verified=1 consumed=1 frame-closed=1 executed=1 |
RFC 0x1→0x2 (Δ1) | FRDONE=1 | mmu-fault-latched=0` — the exact bytes that froze CT0 in boot12
were fetched, stepped and closed by CT1, a RENDER frame retired (`RFC` advanced for the first
time under this driver's own kick), the seeded target store byte-verified, and the bin-output
negative controls stayed fully intact (poison 64/64 + 8192/8192). The class law is a working
path; PI-V3D-90 below is unblocked. One instrument follow-up from the same wire: `[v3d56]
landing` reported `STRAY=4` (arena pages 0–3 changed) — its region map predates this variant and
carries no RENDER-TARGET region, while the byte-verified store proves the target legitimately
wrote; teach the landing witness the render-target region (and derive the pages from the arena
layout) before reading any future STRAY off an rclct1-class boot as displacement.

**Scope — the production path is deliberately NOT rewired in this arc.** `rclct1` is the
proof-of-law, and it is one variant in an experiment harness. Nothing in the compositor, in
`triangle_job` or in `kick_bin_render` is touched. That is not an omission; a proof and a rewire are
different changes and mixing them would mean neither could be reviewed on its own evidence.

**PI-V3D-90 PROGRESS (2026-08-03, same session as boot13):**
- **Step 1 DONE** (`ac9038af`): `v3d75_kick_probe` rehosted onto CT1 in boot13's proven shape —
  the criterion can return true again; the CT0-bound `[v3d73]` sampler stays honestly unarmed
  inside it; the five retracted negatives (`v3d75a/b`, `v3d77a/b`, `v3d80`) are RE-TAKEABLE at
  the next deep-battery sitting and stay retracted until those boots run.
- **Step 2 IN CODE**: `cle_class_witness(thread, head_op, label)` — the class law enforced at
  submit, firing loudly where silicon freezes silently. Classification claims only what the
  campaign measured (121/126 render, 119/120 bin; anything else prints class-unknown). Wired at
  the six production submits: `clear_job`, `bin_job` (both arms), `triangle_job` (bin + render),
  `kick_bin_render` (bin + render). The head op is READ BACK FROM DRAM at the submitted address —
  never builder-declared. The first version declared the op as a constant, and the measurement
  that killed it belongs in the record: LLVM proved every branch statically dead and deleted the
  entire witness (strings=0 on its own build) — a compile-time-satisfiable assert is vacuous by
  construction, one more member of the vacuity family (spent-budget, vacuous-zero, unreachable
  banner). Only a value the compiler cannot see makes an enforcement real. The §49 instrument
  harness's deliberate cross-class kicks keep raw writes with their own justifications, by design.

**§49.13 — boot14's saturated-criterion lesson (PI-V3D-91 named).** The rehosted CT1 probe
executes regardless of firmware init, QPU enable or fabric state (boot13/14 measured: the
v3d75a PRE-send control executed) — so every v3d75/77/80 leg verdict that read a lone EXECUTED
as attribution was vocabulary-lagged and is RETRACTED-VOCABULARY; boot14's "A(enable-qpu)=1 —
the wall was firmware-side init" line is the type specimen. The verdicts now refuse attribution
in code. The five hypotheses are about THREAD 0: **PI-V3D-91** = rebuild those legs on a
BIN-class CT0 criterion (empty/m4 lists execute first-kick, boot6/8 — a criterion that can
genuinely fail on the bin wall), then re-take. The wall itself, remeasured clean on boot14:
CLE fully exonerated ([v3d59] ctstate — list executed, CT0LC/CT0PC moved, items fed), bin frame
DEAD-OPEN ([v3d59] frameclose, 64 samples zero movement) — **the PTB frame unit, between
item-accept and pool-write**, is the whole remaining target.

**THE NEXT ARC — the production rewire (PI-V3D-90).** Named here so it is a commitment and not a
loose end. In order:

1. **Rehost `v3d75_kick_probe` onto CT1** (§49.11a names it the historical mistake). One body swap,
   and it immediately buys the **re-take of five retracted negatives** — `v3d75a`, `v3d75b`,
   `v3d77a`, `v3d77b`, `v3d80` — with a success criterion that can return true. Cheapest first, and
   it recovers banked evidence rather than producing new claims.
2. **Audit the real compositor path end to end for class discipline** — every list that reaches a
   CLE queue gets classified at its construction site, and the class is asserted against the thread
   at submit time so the rule is enforced by the code rather than by review.
3. **Route the render list of the M4/M5–M7 draw path to CT1** and re-take the bin/render split with
   both threads carrying the class they are built for.
4. **Only then** return to the bin wall (§6, §22), which §49.11a establishes the class law does
   **not** explain — `probe_job`, `triangle_job` and `kick_bin_render` were always bin-class on CT0,
   and that defect is still open and still untouched by any of this.

**The boot.**

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_FIRSTKICK=rclct1 ./arroyo kernel8
```

One variant per boot, never two — the R1 discipline is the reason the table above means anything.

**Scope, unchanged.** Read-only discipline exactly as every rung in this family: the kick's own two
queue writes are the only CLE writes; `BXCF` and `MISCCFG.QRMAXCNT` stay **EXCLUDED MEASURED** and
are read, never written; `CTRSTA` stays unimplemented and disarmed; `CT1EA` is **read only** and its
offset (core `+0x010c`) is derived from the map's CT0/CT1 `+4` mirror rule that this file already
relies on for `CT1QBA`/`CT1QEA`, with the raw word printed beside the decode so the reading survives
the offset being wrong; no mailbox tag is sent; the knob grammar is still
`UNAOS_V3D_FIRSTKICK=<value>` read via `option_env!` at compile time, one build is one experiment,
and an unrecognised value runs `empty` and says `recognised=0`.

**Gate.** `./arroyo check` green on both arches with the knob off; direct
`cargo +nightly check --target aarch64-unknown-none-softfloat --features v3d_firstkick,v3d` for the
knob-on path, **canary-verified non-vacuous** by injecting a type error into `v3d89_rclct1_kick`,
seeing it caught, and restoring; the full-knob `kernel8` build green once with `UNAOS_V3D_DEEP=1
UNAOS_V3D_FIRSTKICK=rclct1`; `strings -a target/pi_baremetal/kernel8.img` shows the `[v3d89]` family
and the raw `rclct1` value. QEMU raspi4b models no V3D, so there is no QEMU leg for this arc at all —
the verdict is the attended metal boot.

#### 49.12.1 The unarmed-close-writes rung, and the `[v3d85r1]` instrument fix (PI-V3D-95)

Two items owed since §49.10 land together, because the second is what makes the first readable.

**The `[v3d85r1]` instrument fix.** boot11 is the type specimen. The R1 line printed
`MMU_CTL=0x061d1c01 fault=1` and the verdict *"AN MMU FAULT LATCHED across the boot's first kick —
instrument fault, not a first-kick verdict. Fix it before citing this rung; nothing here
discriminates anything"* — and then said **nothing about what had faulted**. `fault=1` beside a raw
CTL word is not a fault report: the violation witness pair (`VIO_ADDR`/`VIO_ID`) and its decode name
the client and the virtual address, and without them a line that tells its reader to *fix it*
declines to identify what. The detail did reach that capture's wire, but only on a **later** line —
the generic `clear_mmu_fault_latch("v3d85 post-kick")` helper, which prints the pair as a side effect
of W1C-ing the latch away. So the one column the boot turned on lived on a different line, printed
after the verdict that refused because of it, and owed to a helper whose job is hygiene rather than
evidence; the lead below had to be reconstructed by a reader pairing two lines by hand.

The fix is exactly that and nothing adjacent: `[v3d85r1]` now reads the violation pair **at verdict
time, beside the CTL word it belongs to and before any clear**, and carries
`VIO_ADDR=… VIO_ID=… (client … @ VA …)` plus the three decoded fault bits in its own columns; the
fault branch points at them and at the rung below. **No verdict branch, no column meaning and no
gate changes** — every banked reading of this line, and of the `[v3d87h]`/`[v3d87u]`/`[v3d89]` lines
gated on it, stands unmoved.

**The rung — `UNAOS_V3D_UNARMCLOSE=1` (`v3d_unarmclose`, bare feature, default OFF).** §49.10's
queued question, built: *where do the writes of an unarmed frame-close go?* boot11 closed a bin frame
through the rcl-class submit (`BFC` Δ1, `retired=1`) and latched `PT_INVALID=1 WRITE_VIOLATION=1`,
`VIO_ID` client **PTB**, **VA `0x00000070`**, on a boot where `CT0QMA`/`CT0QMS` were deliberately
never programmed — with the arena poison fully intact (`wrote-any=0`), so the write left the arena.

`[v3d95]` follows §49.10's design note literally — *"program nothing, close a frame, and read
`CT0QMA` / `CT0QMS` / `CT0QTS` / the PTB's fault address across it"*. It **programs nothing**: not the
real bases and not a scratch pair either, because programming a scratch base answers a different
question ("does the close path follow a base we gave it?") and destroys this one, whose whole subject
is what the close aims at when those registers hold bring-up residue. The residue is **read** instead,
pre and post, so the fault VA can be read against it — a VA derived from the residue means the close
path follows the never-programmed registers, a fixed `0x70` under a different residue means the aim is
structural. The `[v3d62]` armed-policy catcher is **reused, not re-invented** (hub interrupt latch
cleared, unmapped scratch page seeded, all three channels reported at wait-exit before any W1C), so a
refused write that still *lands* is caught rather than inferred; the `[v3d56]` poison regions ride as
negative controls. The kick is the `emptyunarm` shape byte for byte, from the same builder call at the
same address: bare `CT0QBA`→`CT0QEA`, CPU `clean_range` publish only, no `CT0QMA`/`QMS`/`QTS`, no
`BPOS=0`, no pre-kick L2T invalidate.

Placement is the first-kick family's: after M3 (a CT1 frame, so CT0 is still virgin and the three
registers still hold their residue) and **before `probe_job`**, and the boot then returns. That also
makes the standing knob-exclusion hazard **structural rather than advisory** — the boot ends long
before `[v3d75]`'s `ENABLE_QPU` and `[v3d80]`/`[v3d81d]`'s `DISPLAY_DONE` sends are reached, so this
rung can never sit beside either. With `UNAOS_V3D_FIRSTKICK` armed as well, `[v3d85]` runs and returns
first and this rung stands down: one experiment per boot.

**The §49.16 intersection, named and not acted on.** The settled piOS sweep found piOS programming
pool/base registers in the core0 `+0x108..+0x174` window this driver never touches — a window that
**contains** `CT0QTS` (`+0x15c`), `CT0QMA` (`+0x170`) and `CT0QMS` (`+0x174`), the three registers
this rung reads. The rung states that intersection on its own wire and in this section, and programs
**none** of it: doing so is the sweep-diff arc's proposal, not this one's.

> **CORRECTION — see §49.20.4(b).** "This driver never touches" was written against §49.16's *ask*.
> §49.18.2 folded the *answer* and it reads the other way: *"Every pool, base, size and enable
> register piOS programs, we program"* — `CT0QMA`/`CT0QMS`/`CT0QTS` sit in §49.18.4's fourth class
> (both populated, different values), not in class (b). The accurate statement is **the armed bin
> path programs all three; *this rung* deliberately programs none of them**, which is why boot4 read
> them as zero. The paragraph above is left as written (it records what the arc believed when it
> built the rung); nothing in the rung's design depended on the wrong half.

| outcome | reading |
| --- | --- |
| **fault latched, client `PTB`** | An unarmed close **does** issue a write and the MMU refuses it — boot11 reproduced under the instrument built for it. Read the VA against the `CT0QMA`/`CT0QMS` residue on the PRE station: matching (or offset from) the residue ⇒ the close path follows those never-programmed registers and the tile-memory bases are load-bearing at *close* time, not merely at bin time; a fixed `0x70` under different residue ⇒ the aim is structural. The poison columns say whether anything *also* landed in memory we own, and the `[v3d62]` catcher page whether the refused write landed anywhere at all |
| **fault latched, client ≠ `PTB`** | The close path is not the author of this write; attribution belongs to whichever unit `VIO_ID` names, and boot11's PTB reading is **not** reproduced |
| **no fault, poison intact** | The unarmed close wrote nowhere this instrument can see. If the `[v3d62]` catcher is pristine too, an unarmed close issues no memory write at all on this boot and boot11's violation was not a property of closing unarmed — the hunt moves to what else differed there |
| **no fault, arena bytes moved** | The block reached the tile-state array or the pool with nothing programmed pointing at them — those registers are not the only path to those regions. Read the `[v3d56]` scans for which region moved |
| **no frame closed** | The question was never put: boot11's premise is a close, and this kick did not make one. **Not** a retraction of boot11 and not a verdict — §49.10's row-B instruction applies (re-take both captures in one session) |
| **submit not `SOUND`** | `[v3d54]` says the latched `[BA,EA)` is not the one intended; nothing below it is about an unarmed close. Re-take |

**The boot.**

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_V3D_UNARMCLOSE=1 ./arroyo kernel8
```

⚠ An armed boot deliberately produces a **short** capture missing every banked witness; label it and
never diff it line-for-line against a deep boot.

**Gate (host).** `./arroyo check` green on both arches (the `arm-pi` cfg leg carries `v3d_unarmclose`
beside `v3d_firstkick`, so both the rung and the R1 fix are type-checked there); the knob-off
`kernel8` image **byte-identical** to the pre-arc build, which is what "default OFF and fully
uncompiled" means measured rather than asserted; the knob-on `kernel8` build green with
`strings -a target/pi_baremetal/kernel8.img` showing the `[v3d95]` family; `./arroyo kernel8-test`
green as a no-regression statement only. QEMU raspi4b models no V3D — the rung prints its hub-absent
`[v3d95] SKIPPED` line there — so the verdict is the attended metal boot.

**§49.14 — the VPU-side hunt: boot19 excludes the KMS overlay; PI-V3D-92 named (the domain-cycle
experiment).** Boot18 left one named fabric divergence (`RPIVID ASB_V3D_M_CTRL` 0x4 vs piOS mid-bin
0x4040) and proved the bits unwritable from the ARM core. The VPU-side candidates were: the
firmware's config.txt/dtoverlay path, a mailbox tag family, and the piOS boot-state diff's VPU
stations. Boot19 (PA27, 2026-08-04) took the first: PA26's byte-identical kernel plus
`dtoverlay=vc4-kms-v3d` and the overlay blobs (pinned fw 1.20260521) on the FAT — the firmware read
`overlay_map.dtb`, loaded `vc4-kms-v3d-pi4`, and **M_CTRL still read 0x00000004 at the [v3d75]
station** [booted, pi4-r23s1x boot19]. The overlay-alone hypothesis is EXCLUDED; the mailbox fb
(1920x1200) survived the overlay, so the KMS-boot panel hazard did not fire on a bare-metal boot
either. piOS's 0x4040 therefore appears only when its *kernel driver* brings V3D up at runtime.

What that leaves in the mailbox family, honestly accounted: `SET_ENABLE_QPU` — tested and excluded
(boots 17/19); the `M_CTRL` transplant — refused by silicon (boot18); `SET_DOMAIN_STATE(10, ON)` —
sent every boot and ACKs, **but always onto a domain the firmware already powered** (PI-V3D-2), so
the VPU's real off→on domain-init transition has never once run for a bare-metal boot, while piOS's
`v3d_reset` semantics is exactly a power cycle. **PI-V3D-92** = that cycle: `[v3d92]` rides the tail
of the deep battery (after `[v3d77]`; `[v3d80]`/`[v3d81]` must not be armed beside it, and the
qpu-gate must be OFF because SET_ENABLE_QPU wedges the mailbox this rung needs). Sequence:
pre-station (GET_DOMAIN_STATE + both ASB words) → OFF → **the decisive ASB M_CTRL read immediately
after ON, printed before any recovery so the datum banks even if the block dies** → clock/bridge/
MMU/IRQ recovery → the PI-V3D-91 bin criterion + CT1 health kick, with a four-way composed verdict
(bits×criterion). No V3D core register is read between OFF and the post-recovery presence probe.
Armed by `UNAOS_V3D_PMCYCLE=1` (`v3d_pmcycle`, bare feature, default OFF); an armed boot may lose
V3D from the cycle onward if recovery fails — experiment-boot cost, same acceptance class as
`[v3d80]`'s panel. Whatever the verdict, the PTB frame-unit bracket (§49.13) remains the standing
target: (bits-appear × wall-moves) convicts the domain-init; (bits-appear × wall-stands) exonerates
0x4040 as a sufficient condition; (no-bits × wall-moves) attributes to the cycle's reset side;
(no-bits × wall-stands) closes the mailbox family entirely and sends the hunt to the PTB bracket
and the piOS boot-state diff.

**Boot20 verdict (PA28 @1cbfd254, 2026-08-04) [booted, pi4-r23s1x boot20]: case 4 — the cycle
changes neither the bridge nor the wall.** The transition was real (OFF reply/readback 0, ON
reply/readback 1 — the first genuine domain off→on a bare-metal boot has ever run); `M_CTRL` read
`0x00000004` at all four stations (pre, off, post-ON, post-release). Recovery was clean end to end:
block back UP, MMU/IRQ re-established, the bin criterion submitted sound (dead, as everywhere), the
CT1 health kick EXECUTED, the compositor kept the panel, zero CRIT. **The mailbox domain family is
CLOSED**: with ENABLE_QPU (boots 17/19), the M_CTRL transplant (18), the KMS overlay (19) and the
domain cycle (20), every ARM-reachable and every mailbox-reachable hypothesis for the bin wall is
exhausted on honest instruments. What programs piOS's 0x4040 — and whether it matters at all — is
firmware-internal. The hunt is now exactly two-pronged: the PTB frame-unit bracket (§49.13's named
target, item-accept → pool-write) and the piOS boot-state diff's VPU stations.

---

> **APPENDED CORRECTION (§49.22.4 item 4, PI-V3D-97, 2026-08-18).** The outcome row *"fault latched,
> client PTB"* and its instruction — *read the VA against the residue; matching ⇒ the close path follows
> those registers, a fixed `0x70` under a different residue ⇒ the aim is structural* — is **DISCHARGED**,
> by a road it did not anticipate. Under a different residue (§49.22, leg C) the fault VA did **not**
> track the base: it moved to `0x0`. The **landed bytes** tracked it exactly. Read literally the
> instruction points at "structural"; read correctly, "structural" is refuted. The fork was sound and its
> premise was not — it assumed **one** close-time write, and there are two. The portable lesson, which
> every later rung in this campaign should be designed on: **where a unit issues more than one write per
> event, a single fault latch samples one of them, and the poison/landing pair — which sees all of them —
> outranks it.**

### 49.15 The PTB frame-unit bracket — first-change ordering across item-accept → pool-write (PI-V3D-93)

§49.13 names the target and §49.14 closes everything around it. This section builds the instrument
that measures it.

**What is already known, and therefore may not be re-measured.** The CLE executes the bin list
cleanly: `CT0LC`/`CT0PC` both move across the kick and the control thread feeds items to the PTB
(`[v3d59] ctstate`, the final verdict arm). The bin frame is then **DEAD-OPEN** — `BMACTIVE`=1,
`BMBUSY`=0, and not one bit of movement across 64 samples of `PCS`/`CT0CS`/`BFC`/`BPCA`/`BPCS`
(`[v3d59] frameclose`). The RENDER engine on the same block, the same MMU and the same clock retires
frames and lands byte-verified stores (`[v3d58] xengine`, and §49.12's `rclct1` on CT1). Every
ARM-reachable and every mailbox-reachable fabric hypothesis is exhausted (boots 17–20).

**The gap the bracket exists to close.** Every "nothing moved" verdict in this file rests on a
**five-register** poll taken at a **1 ms** grid, and always after the FLDONE backstop had already
given up. Two objections survive that instrument and neither has ever been answered: the set may be
too narrow (a register outside those five could be moving), and the grid may be too coarse (motion
confined to one frame's worth of time falls between ticks). Until both are closed, "the PTB frame
unit is dead" is a claim about `frameclose`, not about the silicon.

#### 49.15.1 The station set

Every station below is an offset **already defined and already used in `v3d.rs`** — the bracket
invents no register, and nothing here is carried in from outside knowledge. Each row cites the
constant's definition site.

| # | Station | Base + offset | Source (`v3d.rs`) | Class | Why it could move between item-accept and pool-write |
|---|---|---|---|---|---|
| 0 | `CT0CS` | core `+0x0100` | `V3D_CLE_CT0CS`, line 421 | cle | The control thread's own status. `CTRUN` dropping mid-window is the CLE giving up rather than the PTB stalling |
| 1 | `CT0CA` | core `+0x0110` | `V3D_CLE_CT0CA`, line 490 | cle | Current fetch address. If it advances while the PTB is silent, the CLE is still feeding a unit that is not consuming |
| 2 | `CT0LC` | core `+0x0120` | `V3D_CLE_CT0LC`, line 542 | cle | List counter — the item-accept marker itself. §49.13's fact is that this moves; the bracket dates *when* |
| 3 | `CT0PC` | core `+0x0128` | `V3D_CLE_CT0PC`, line 543 | cle | Primitive counter. Second item-accept marker; a `CT0LC`/`CT0PC` split in time would separate list-item accept from primitive accept |
| 4 | `PCS` | core `+0x0130` | `V3D_CLE_PCS`, line 544 | frame | `BMACTIVE`/`BMBUSY`/`BMOOM`. A `BMBUSY` pulse anywhere in the window is the unit attempting work — the single most decisive bit the coarse poll could have missed |
| 5 | `BFC` | core `+0x0134` | `V3D_CLE_BFC`, line 557 | frame | Bin frame count — frame close. The terminal marker of the whole chain |
| 6 | `BPCA` | core `+0x0300` | `V3D_PTB_BPCA`, line 559 | ptb | **The pool-write pointer.** Its first change is the definition of "the PTB wrote" |
| 7 | `BPCS` | core `+0x0304` | `V3D_PTB_BPCS`, line 560 | ptb | Remaining pool bytes. Moves with, or just before, `BPCA`; the two together date the reservation vs the write |
| 8 | `BPOA` | core `+0x0308` | `V3D_PTB_BPOA`, line 561 | ptb | Overflow address. **Never polled during a window before.** Movement means the PTB got far enough to want more memory — which would place the wall *downstream* of item-accept outright |
| 9 | `BPOS` | core `+0x030c` | `V3D_PTB_BPOS`, line 562 | ptb | Overflow size, cleared to 0 before every kick (`bin_prejob_bpos_clear`). Any non-zero value inside the window is the PTB writing its own overflow request — the strongest possible "the unit is alive" datum |
| 10 | `BXCF` | core `+0x0310` | `V3D_PTB_BXCF`, line 583 | ptb | Binner extra config. Never polled during a window. A config word that changes mid-frame would mean the unit is reconfiguring itself |
| 11 | `INT_STS` | core `+0x0050` | `V3D_CTL_INT_STS`, line 649 | frame | `FLDONE`/`FRDONE`/`OUTOMEM` raw latch. `OUTOMEM` in particular has only ever been read once, after the fact |
| 12 | `GMP_STATUS` | core `+0x0800` | `V3D_GMP_STATUS`, line 328 | fabric | `WR_ACTIVE`/`RD_ACTIVE`/`VIO`. This is the pool write seen at the protection unit — a write attempt that never reaches DRAM would still show here |
| 13 | `L2TCACTL` | core `+0x0030` | `V3D_CTL_L2TCACTL`, line 286 | fabric | `L2TFLS` reads 1 while a flush is in progress. The PTB's pool traffic passes L2T; a flush starting inside the window is cache-side evidence of a write |
| 14 | `MMU_CTL` | **hub** `+0x1200` | `V3D_MMU_CTL`, line 230 | fabric | `PT_INVALID`/`WRITE_VIOLATION`/`CAP_EXCEEDED` latches. Dating a fault to a sample index converts a sticky bit into a causal event |
| 15 | `MMU_DEBUG_INFO` | **hub** `+0x1238` | `V3D_MMU_DEBUG_INFO`, line 235 | fabric | Read at fault time today, never across a window |

**Two registers are deliberately excluded from the in-loop set**, and sampled only at the pre-arm and
post-window stations: `CT0SYNC` (core `+0x0154`, line 581) and `CT1SYNC` (core `+0x0158`, line 582).
`[v3d59] ctstate`'s own verdict records the hazard in terms — the read side effects of these two are
unverified, and a semaphore register that decrements or clears on read would be *moved by the probe
itself*. Five reads per register was already enough to worry that witness; this loop would issue
thousands. They are still reported, as a pre/post pair with a `moved=` column, so the datum is not
lost — only the manufacture of it is prevented.

**Registers considered and NOT taken.** The PTB-adjacent unnamed words the `[v3d76]` sweep covers
(core `0x4000–0x4A00`, hub `0x0000–0x0100`) are a 640-word window, not a station set; sampling them
per-iteration would cost more than the frame lasts, and `[v3d77]` already transplanted the two
config-shaped ones (core `+0x68`, hub `+0x68`) and banked both dead. If the bracket returns outcome
(a), the sweep window is the natural second pass, restricted to whatever neighbourhood the first
mover names. No MMU *counter* registers are mapped in this driver — only the control/fault words
above — so none are cited. **Nothing in this section is INFERRED**: every offset above is a
corroborated `v3d_regs.h` name already carried in `v3d.rs` with its citation in place. The only
inference anywhere near this instrument is the `CTnCS` bit decode past `CTRUN`, which the bracket
does not use — it compares `CT0CS` as a **raw word**, so a wrong bit map cannot produce or suppress
a first-change datum.

#### 49.15.2 The bracket

The sampler follows `[v3d59] frameclose`'s cadence pattern — seed a baseline, fold the full station
set repeatedly, compare against the seed, print strictly afterwards — with three changes that are
the whole point of the rung.

1. **It runs DURING the window, not after it.** `frameclose` polls once the FLDONE backstop has
   already expired. This folds from inside the criterion's own retire wait.
2. **It runs at the poll loop's rate, not on a 1 ms grid.** The fold is placed in `wait_fldone`'s
   main loop body rather than in its `~1 ms` cadence branch, beside the `INT_STS` read that ends the
   wait. The `v3d71`/`v3d73` samplers keep their cadence-tick position and are untouched.
3. **It records FIRST-CHANGE ordering, not a moved/not-moved flag.** Per station: the pre-QBA seed,
   the first value that differed from it, the sample index and microsecond offset at which that
   happened, the running change count, and the last value. The witness then prints the movers in
   first-change order, ranked.

**The contract the fold inherits.** `v3d71_sampler_take` and `v3d73_sampler_take` established it and
`v3d93_sampler_take` keeps it exactly: *one static bool read and zero MMIO on every wait in the
driver except the single window the rung arms it across.* Sixteen reads and sixteen compares per
sample is bounded work, it happens only inside the armed window, and the wait's duration is unchanged
regardless — the `~0.5 s` CNTPCT deadline is what ends the loop, not the number of iterations it
managed. Nothing in the fold path formats or prints; serial inside the window would dominate the very
timing being measured, which is the rule `frameclose` already sets.

**Coverage, stated rather than assumed.** The seed is taken before the criterion submit, so `first`
is a genuine pre-QBA word and a block that never moves reads as `moved=0` with the pre-kick value
printed. The one sub-window the fold cannot reach is `wait_bit_clear`'s `CT0CS` spin, which runs
between the GO and `wait_fldone` and has no hook. On the wedged block `CTRUN` reads clear
(`[v3d59] frameclose`: `CT0CS` static across 64 samples), so that spin exits on its first test and
the gap is nil; on a boot where `CTRUN` is genuinely set the bracket starts late, and `samples`
together with the criterion's own `idled=` column say so on the wire.

#### 49.15.3 The submit shape, and why

`submit_bisect_rung_geom`'s wait is **internal** — `wait_bit_clear` on `CT0CS`, then `wait_fldone` —
so a sampler wrapped around the call would bracket the entire submit and see only its endpoints,
which is exactly the endpoint measurement §49.13 already has. Two options remained:

| option | shape | why not / why |
|---|---|---|
| transcribe the criterion's submit into the rung, sampler beside the `QBA`/`QEA` writes | maximal control of the loop | **REJECTED.** It forks the PI-V3D-91 criterion into a second copy that can silently drift from the one every §49 verdict is banked against. The bracket would stop being the same instrument — the failure mode this ladder has had to unwind twice (§49.11a's class-law retraction, §49.13's saturated criterion) |
| call the criterion **unchanged**, exactly as the `[v3d91]` call sites do, and fold from inside `wait_fldone` on the `v3d71`/`v3d73` hook contract | one criterion, one instrument | **TAKEN.** No second copy, no drift, and it keeps the rung honestly read-only |

`bank` is passed **false** for the same reason: `true` arms the PCTR counter bank, which is a write
this instrument has no license to make, and `submit_bisect_rung_geom` documents that the unbanked
path issues no extra MMIO at all.

**Read-only discipline.** The rung issues **no V3D register write of its own** and sends **no mailbox
tag of any kind**. Its only register writes are the ones inside the criterion submit it reuses — the
same submit the `[v3d48]` ladder above it already runs many times on the same boot. It is therefore
the first arming switch in this family that carries **no boot cost**: no panel risk, no V3D loss, no
mailbox exposure, nothing it can wedge.

**Ride position.** After `[v3d74]`, before `[v3d75]`. The "before" is load-bearing: everything from
`[v3d75]` onward mutates this boot's fabric (`[v3d75]`'s `ENABLE_QPU` send and `M_CTRL` transplant,
`[v3d77]`'s unnamed-word pokes, `[v3d92]`'s power cycle), and an ordering trace is only worth taking
on the block as it stands — the same untouched-fabric requirement `[v3d75]`'s own ordering comment
states for `[v3d74a]`'s verdict. The "after `[v3d74]`" half is because the bracket's reading is drawn
against that rung's banked thread-0 fact from the same boot. Because the rung writes nothing, its
position costs the rungs below it nothing.

#### 49.15.4 Both outcomes, pre-written

| outcome | verdict, and where the hunt goes |
|---|---|
| **(a) some station outside `frameclose`'s five moved** | **THE FRAME UNIT IS ALIVE PAST WHAT `frameclose` COULD SEE.** §49.13's DEAD-OPEN reading was an artifact of a five-register poll at a 1 ms grid, not a property of the unit. The wall moves **downstream of the last mover** in the ordering table: that register's own unit becomes the next bracket, the `[v3d76]` sweep window is re-run restricted to its neighbourhood, and every "dead" verdict in §49 that rests on the narrow set is re-read. A `BPOS`/`BPOA` mover is the strongest form of this — it would mean the PTB got as far as requesting memory — and a `BMBUSY` pulse in `PCS` is the second strongest |
| **(a′) motion confined to `frameclose`'s own five** | The bracket reproduces §49.13 at a far finer sample rate over a wider set and adds no new station. This is outcome (b) with the coarse-instrument objection **closed by measurement** rather than assumed away |
| **(b) nothing moved** | **DEAD AT ITEM-ACCEPT ON THE WIDE SET.** Sixteen stations, sampled at the poll loop's own rate, across the whole retire window: the CLE feeds items and nothing architecturally visible responds — no pool pointer, no overflow request, no protection or MMU event, no cache activity, no busy pulse. The unit does not fail to *complete*; it never observably *starts*. Both surviving objections to §49.13 are then answered, **no ARM-visible station is left to bracket**, and the register-side prong of §49.14's two-pronged hunt is exhausted. The remaining surface is firmware-side instrumentation — the piOS boot-state diff's VPU stations |
| **criterion RETIRED** | The wall did not reproduce on this boot. The ordering table then describes a **working** frame, which is the reference trace the wedged case has never had; it is banked as such and explicitly **not** read as a wall verdict |
| **`samples=0`** | The criterion's FLDONE poll never folded a sample. INCONCLUSIVE — no verdict, by construction |

#### 49.15.5 The boot, and the gate

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_PTBBRKT=1 ./arroyo kernel8
```

Armed by `UNAOS_V3D_PTBBRKT=1` (`v3d_ptbbracket`, bare feature, default OFF, implying nothing —
inert without `UNAOS_V3D_DEEP`). Gate-off prints one `[v3d93] ptb-bracket SKIPPED — ptbbracket-gate
off` line and samples nothing. Because the rung is read-only it may be combined with any other knob,
unlike `PMCYCLE`/`QPU`/`DISPDONE`.

**Gate.** `./arroyo check` green on both arches with the knob off; the full-knob `kernel8` build green
with `UNAOS_V3D_DEEP=1 UNAOS_V3D_PTBBRKT=1`; `strings -a target/pi_baremetal/kernel8.img` shows the
`[v3d93]` family, with the `[v3d92]` family on the same image as the positive control that the proof
discriminates at all. QEMU raspi4b models no V3D, so there is no QEMU leg — `kernel8-test` green
means no regression and nothing more, and the verdict is the attended metal boot.

**§49.15 boot21 verdict (PA29 @2c0f5ca9, 2026-08-04) [booted, pi4-r23s1x boot21]: OUTCOME (b) —
DEAD AT ITEM-ACCEPT ON THE WIDE SET.** 348,908 samples across all 16 stations at the FLDONE poll
loop's own rate: not one bit of motion — no pool pointer, no overflow request (BPOS held 0), no
GMP/MMU/L2T event, CT0SYNC/CT1SYNC at rest pre/post. The CLE feeds items and nothing
architecturally visible responds. Combined with boots 17–20, **the ARM-visible V3D surface is now
closed in its entirety**: every mapped register, every mailbox act, every fabric write, and every
observable station at poll rate. The bin-wall hunt's only remaining path is firmware-side
instrumentation — the piOS boot-state diff's VPU stations (§49.14) — which is a bench/tooling
decision, not another kernel rung. The [v3d93] bracket stays in the tree as the standing wide-set
instrument for any future state the block reaches.

**§49.16 — the piOS sweep diff (2026-08-04): zero unexplained divergences against every surviving
reference, and an evidence correction to §46.1.** Boot21's `[v3d76]` settled wedged sweep was
diffed against the only piOS artifacts on the box — the curated 60-register idle and mid-render
dumps of 2026-07-29. Result: 55 registers overlap, 18 diverge, every one class (a) known or
(b) consequence-of-wedge/job-state; **class (c) = zero**. All six non-V3D fabric words (PM_GRAFX,
both ASB blocks) MATCH piOS idle. **Correction, evidence law:** §46.1's "the sweep reduced the
structural divergences to exactly two unnamed words" has NO surviving artifact — no piOS `SWEEP`
output exists on the box or in git; the claim is a doc memory, not a reproducible diff. What the
surviving diff cannot reach, honestly: 41 nonzero sweep words with no piOS value (led by seven
unmapped config-shaped core-CTL constants at core0 +0x0c/+0x14/+0x20/+0x2c/+0x44/+0x80/+0x84,
bit-stable across four boots), every register that reads zero for us (the exact shape of a missing
enable), six hub work-counters (+0x8c…+0xa8, monotone, a hub-side traffic channel the §49.15
bracket does not sample), and the clock-manager block, never read directly on either side — while
the firmware's clock-state report is already on the §46.4 instrument-lie ledger. **The sitting ask
that closes it** (script updated, `v3d-dump-pios.sh --sweep`, [v3d76]-identical windows + CM
block): boot the bench Pi on stock piOS and take `--sweep` at idle, `--sweep` under glxgears, and
one `--trigger ct0run` mid-bin; scp all three to `~/unaos-bench/capture/`. Until then the seven
core-CTL constants are unexamined territory, not excluded territory. The hub TFU window
(0x100–0x1000) stays excluded on both sides by decision: no source-cited register list, and
unbacked V3D-hub reads can fault.

**§49.17 — PI-V3D-94: the two channels nobody samples (hub work-counters + the clock manager).**
§49.15 closed the ARM-visible V3D surface and §49.16 named what that closure does not cover. Two of
the named channels are readable from this kernel today, and `[v3d94]` reads both. One feature
(`v3d_hubcm`, bare, default OFF), one knob (`UNAOS_V3D_HUBCM=1`), one `[v3d94] SKIPPED — hubcm-gate
off` line when it is off. **Read-only, both halves**: half A issues no register write of its own —
its only writes are the PI-V3D-91 criterion's, which the ladder already runs many times per boot —
and half B issues no write at all. No CM password constant exists anywhere in `v3d.rs`, deliberately,
so no store in this driver can arm a clock by accident.

*Half A — the hub-traffic delta (deep battery, immediately after `[v3d93]`, before `[v3d75]`).* The
six unmapped hub words at `+0x8c/+0x90/+0x94/+0xa0/+0xa4/+0xa8` are snapshotted PRE-submit and again
after the criterion's FLDONE backstop returns, and the six deltas are printed with a verdict. Two
design points are load-bearing. (1) The six are **not** added to `V3D93_STATIONS`: sixteen reads per
sample is the budget §49.15's 348,908-sample baseline was taken at, and widening the array to
twenty-two would restate the sample rate that baseline is quoted in. A counter that only moves
forward needs no ordering trace — two samples answer the question completely. (2) The rung takes its
**own** criterion window rather than sharing the bracket's, so the two arm independently and neither
can silently change the other's window; the cost is one extra bin submit per armed boot. The honest
limit, named rather than hidden: a pre/post delta cannot say *when* inside the window a counter
stepped, nor separate traffic the criterion caused from traffic another agent (VPU, display,
firmware) put through the hub in the same interval.

| half A outcome | reading |
| --- | --- |
| **Δ>0, criterion wedged** | **THE HUB SAW TRAFFIC EVERY CORE-SIDE STATION DENIED.** §49.15's dead-at-item-accept closure covers the core-visible surface only; something crossed the hub while all sixteen bracket stations held still. Attribution is the next question, and the way to ask it is a null window — the same delta across an interval with no submit in it |
| **Δ=0 on all six** | **DEAD AT ITEM-ACCEPT EXTENDS TO THE HUB.** The one moving channel §49.16 found outside the bracket does not move across the criterion window either: the core denies the work and the hub records none. That closes the last ARM-readable channel the sweep diff named, and the firmware-side prong (§49.14) is the only surface left |
| **any word moved backwards** | These are not monotone counters, or not counters at all. §49.16's monotone-across-boots observation does not survive an intra-boot decrement, and the "hub work-counter" reading of +0x8c…+0xa8 is withdrawn before anything is built on it |
| **criterion RETIRED** | The wall did not reproduce on this boot; the deltas describe a **working** frame and are banked as the reference reading, not as a wall verdict |

*Half B — the direct CM clock station (bringup, immediately after the mailbox clock grant).* One
line carrying `CM_V3DCTL` and `CM_V3DDIV` with their decoded fields, printed **beside** the
firmware's own `GET_CLOCK_RATE`/`GET_CLOCK_STATE` answers, followed by every nonzero word in
`CM[0xFE101000, +0x100)` in the `[v3d76]` SWEEP line format. The point is the pairing: the firmware's
clock report is already on the §46.4 instrument-lie ledger — it claimed the V3D clock gated at
250 MHz while `CYCLE_COUNT` free-ran at ~499 MHz — and every clock verdict in this file rests on it.
This is the first reading in the driver's history that can audit that report against the hardware it
describes.

**The offsets are INFERRED and tagged so on the wire.** `+0x038`/`+0x03C` is the conventional
`clk-bcm2835.c` CM slot layout for V3D; this tree carries no source-cited confirmation for BCM2711.
The surrounding window dump is what makes the inference falsifiable without a second boot — a CM
block with CTL/DIV-shaped neighbours but zeros at the V3D slot is a genuinely unprogrammed slot,
whereas an all-zero window means the read landed nowhere useful and nothing about the clock follows.

**Mapping, checked before writing the half.** `0xFE101000` sits inside the 0xC000_0000–0xFFFF_FFFF
Device-nGnRnE GiB that `boot.rs` L1[3] maps — the same window `PM_BASE` (0xFE10_0000) and
`LEGACY_ASB_BASE` (0xFE00_A000) are already read through. No new MMU mapping is opened and no
page-table edit is needed, so half B ships.

| half B outcome | reading |
| --- | --- |
| **CM ENAB disagrees with the firmware's active bit** | **DIVERGENCE.** The report is the one already on the instrument-lie ledger; this is the first direct reading that can convict it, and every clock verdict resting on that report is re-read |
| **CM ENAB agrees, DIVI/DIVF nonzero** | Not a null result. The report is audited against its hardware for the first time, and the DIVI/DIVF pair is the first direct frequency evidence this driver has carried — a divisor that contradicts the granted rate is itself a finding |
| **both slot words zero, other CM words nonzero** | The slot is real and genuinely unprogrammed: firmware granted the V3D clock without programming this CM slot, which is a divergence from the `clk-bcm2835` model in its own right |
| **whole window zero** | The read landed nowhere useful — wrong base or an undecoded block on this silicon. Nothing about the clock follows and the half is INCONCLUSIVE by construction |

**QEMU note (host gate, not a metal verdict).** `kernel8-test` exercises half B for free: QEMU's
CPRMAN model answers with 24 nonzero words in the window, CTL/DIV-shaped pairs at `+0x28/+0x2c` and
`+0x30/+0x34` either side, and **zero at `+0x38/+0x3c`** — the third row above. That is corroboration
of the *layout* (the V3D slot sits where the inference put it, between its populated neighbours) and
says nothing about the bench Pi's firmware, which is the boot that decides. Half A cannot run on
QEMU at all — no V3D — and prints its hub-absent SKIPPED line.

#### 49.17.1 The boot, and the gate

```
UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 \
UNAOS_VUGPAR=1 UNAOS_WEDGE2=1 UNAOS_V3D_DEEP=1 UNAOS_V3D_HUBCM=1 ./arroyo kernel8
```

Inert without `UNAOS_V3D_DEEP`. Because both halves are read-only the knob may be combined with any
other, unlike `PMCYCLE`/`QPU`/`DISPDONE`; it may also be armed alongside `UNAOS_V3D_PTBBRKT=1`, and
that is the recommended pairing — one boot then carries the bracket's ordering table and the hub
delta across the same wedged state.

**Gate (host, taken at PA30).** `./arroyo check` green on both arches; the full-knob `kernel8` build
green with the arming knob echoed; `strings -a target/pi_baremetal/kernel8.img` shows the `[v3d94]`
armed strings with `[v3d93]` as the positive control that the proof discriminates at all; and the
converse control — the knob-off build carries the `hubcm-gate off` SKIPPED line and **none** of the
armed banners. `kernel8-test` green (MBENCH 91/91) is a no-regression statement plus the QEMU CM
reading above; the verdict on both halves is the attended metal boot.

#### 49.17.2 The boot22 verdict — both halves answered, and the ARM-visible surface is CLOSED

**§49.17 boot22 verdict (PA31 @225660f7, 2026-08-04) [booted, pi4-r23s1x boot22]: half A =
Δ=0 ON ALL SIX; half B = CM ENAB AGREES, the clock is honestly running.** Both halves reproduced
unchanged on every armed boot that followed on the same bench session (boots 23/PA32, 24/PA33,
25/PA34 — three windows on that boot — and PA36): eight armed hub windows and eight CM stations,
identical values throughout.

*Half A — the hub is as dead as the core.* The six unmapped hub work-counters were snapshotted
pre-submit and after the criterion's FLDONE backstop returned, around a criterion that wedged as
everywhere else (`retired=0`, `BFC` Δ0, `PCS=0x00000001` with `BMACTIVE=1 BMBUSY=0 BMOOM=0`,
`INT_STS=0x00000000`, `waited=500000us`), on a submission the `[v3d54]` audit called SOUND
(`CT0QBA=0x0034b000 CT0QEA=0x0034b00e span=14`).

| counter | address | pre | post | delta |
|---|---|---|---|---|
| `hub+0x8c` | `0xfec0008c` | `0x00000020` | `0x00000020` | +0 |
| `hub+0x90` | `0xfec00090` | `0x0000009d` | `0x0000009d` | +0 |
| `hub+0x94` | `0xfec00094` | `0x00001400` | `0x00001400` | +0 |
| `hub+0xa0` | `0xfec000a0` | `0x00000002` | `0x00000002` | +0 |
| `hub+0xa4` | `0xfec000a4` | `0x0000001f` | `0x0000001f` | +0 |
| `hub+0xa8` | `0xfec000a8` | `0x00000140` | `0x00000140` | +0 |

`counters=6 stepped=0 backwards=0`. This is §49.17's pre-written second row: **DEAD AT ITEM-ACCEPT
EXTENDS TO THE HUB.** The one channel §49.16 found moving outside the §49.15 bracket does not move
across the criterion window either — the core denies the work and the hub records none. No counter
decremented, so the third row does not fire and §49.16's monotone reading of `+0x8c…+0xa8` survives
intact; it is simply a channel that steps elsewhere in the boot and not here.

*The bracket re-baseline on the same boot.* `[v3d93]` was armed alongside, as §49.17.1 recommends:
`samples=348916 stations=16 | criterion retired=0 | moved=0`, `NEW=0`, `cle=0 ptb=0 fabric=0`,
`CT0SYNC`/`CT1SYNC` `0x00000000` pre and post. That reproduces §49.15's boot21 reading
(348,908 samples, sixteen stations, zero bits moved, `BPOS` held 0 — no memory request, no
GMP/MMU/L2T event, the syncs at rest) on a different flash, so the wide-set closure is not a
one-boot artifact.

*Half B — the clock is honest, and the wall is not a gated clock.* The direct station read
`CM_V3DCTL@0xfe101038=0x000002d4` (SRC=4, ENAB=1, KILL=0, BUSY=1, FLIP=0) and
`CM_V3DDIV@0xfe10103c=0x00001000` (DIVI=1, DIVF=0 — divide-by-one, the slot passing its source
through undivided), beside the firmware's own `GET_CLOCK_RATE=500000000 Hz` and
`GET_CLOCK_STATE=0x00000001 (active=1)`. **The CM enable bit agrees with the firmware's active
bit**, and the enabled-plus-busy pair is a direct reading rather than a report: the V3D clock is
running on the boots this file's wedge verdicts were taken on. Nothing in §49's dead-block ladder
is explained by a clock that was never turned on.

*The INFERRED offsets, now corroborated from both sides.* §49.17 shipped `+0x038`/`+0x03C` as an
inference with the window dump as its falsifier, and the two dumps answer it in opposite ways that
agree. QEMU's CPRMAN model showed CTL/DIV-shaped pairs either side and **zero** at the V3D slot —
the layout is where the inference put it. Metal showed `42 nonzero words in CM[0xfe101000,
0xfe101100)`, with `0x0000636d` (`"cm"`) at `+0x000`/`+0x004` confirming the base decodes as the
clock manager, CTL/DIV-shaped neighbours at `+0x008`/`+0x00c` and `+0x028`, and the V3D slot itself
carrying a programmed CTL/DIV pair consistent with the granted rate. The fourth outcome row
(whole window zero, INCONCLUSIVE) is excluded by measurement.

*The honest limit on the §46.4 audit.* What was audited is the report **on the standard bringup
path**, and there it is truthful. §46.4's instrument-lie entry was taken on the `v3d79_minimal`
boot — firmware reporting the clock gated at 250 MHz while `CYCLE_COUNT` free-ran at ~499 MHz —
and on that path half B still reads CM directly but prints no firmware column, because the two
mailbox queries are the same read-only tags `[v3d55]` sends and `v3d79_minimal`'s whole
discriminator is that bringup sends no mailbox tag at all. The ledger entry therefore stands
unretracted; what boot22 adds is that the divergence is not a property of the hardware clock,
which reads enabled and busy on every armed boot.

**The closure, stated once.** With the KMS overlay excluded (boot19, §49.14), the domain cycle
closing the mailbox family (boot20, §49.14), the sixteen-station bracket dead at item-accept
(boot21 and boot22, §49.15), the hub delta at zero and the clock manager read honest (boot22,
this section), **the ARM-visible V3D surface is CLOSED.** Every mapped register, every mailbox
act, every fabric write, every observable station at poll rate, the one unmapped hub channel the
sweep diff named, and the clock block behind all of it have now been read, and none of them
carries the bin wall. No further kernel rung is proposed against this surface, and any future
"one more register" proposal owes an argument for why it is not already covered above.

**The one remaining prong is firmware-side.** It is the §49.16 sitting ask, unchanged and still
outstanding: boot the bench Pi on stock piOS and capture `--sweep` at idle, `--sweep` under
glxgears, and one `--trigger ct0run` mid-bin, then scp all three to `~/unaos-bench/capture/`. The
script is staged at `~/unaos-bench/tools/v3d-dump-pios.sh` with `[v3d76]`-identical windows plus
the CM block, so the piOS side lands in the same format both halves above are quoted in. Until that
capture exists, the 41 nonzero sweep words with no piOS value — led by the seven unmapped
core-CTL constants — remain unexamined territory, not excluded territory. `[v3d93]` and `[v3d94]`
both stay in the tree as standing read-only instruments for any future state the block reaches.

---

### 49.18 The piOS settled-sweep diff — the firmware-side prong's data, and what it does to the frame-unit hypothesis (PI-V3D-95)

§49.16 named the sitting ask and §49.17.2 made it the campaign's only open surface. The first of
its three captures now exists.

**The artifact.** `~/unaos-bench/capture/v3d-dump-pios-sweep-20260812.txt` — 163 lines, taken
2026-08-12T21:11:34-06:00 on the bench Pi 4B under stock piOS, kernel `6.18.34+rpt-rpi-v8`, with
`vc4` **and** `v3d` loaded (the precondition the script header states, met), `vcgencmd`
`frequency(46) = frequency(1) = 500000992`. Windows: `hub[0x0,0x100)`, `mmuc[0x1000,0x1040)`,
`mmu[0x1200,0x1240)`, `core0[0x4000,0x4A00)` — `[v3d76]`-identical — plus `CM[0xFE101000,+0x200)`.
The state is **settled/idle-after-work**, not mid-bin.

**Our side of the diff.** The `[v3d76]` settled wedged sweep of boot PA36 (2026-08-04, capture
`~/unaos-bench/capture/pi4-r23s1x/ttyACM0.log`), and for the clock manager the `[v3d94]` half-B
CM window dump of boot22/PA31 in the same capture. Our sweep is **bit-stable** between boot22 and
PA36 across all 76 nonzero words except the three free-running PCTR outputs at core0
`+0x688/+0x694/+0x698`, so nothing below is a one-boot artifact.

**Method.** A mechanical join on physical address. A register absent from a sweep read zero (both
tools print nonzero words only). Names come from the offset tables in
`unaos/crates/kernel/src/arch/aarch64/v3d.rs` and nowhere else; an offset with no constant there is
written `core0+0xNNN (undecoded)` and stays that way. Two offsets are named **by the CT0/CT1 `+4`
mirror rule** this file already relies on for `CT1QBA`/`CT1QEA`/`CT1EA`, and are tagged INFERRED.

**The counts.** In the four V3D windows: **37 identical**, **32 both-nonzero-but-different**,
**1 piOS-only**, **7 ours-only**. In `CM[0x000,0x100)`: **40 identical of 43**, 3 divergent.
`CM[0x100,0x200)` — 44 nonzero piOS words — is outside the window our instrument reads at all.

#### 49.18.1 Class (a) — identical to our boot state

Thirty-seven words match byte for byte. The corroborations the ask was taken to get:

| register | offset | both sides | what it corroborates |
|---|---|---|---|
| `HUB_IDENT0..3` | hub `+0x08/0x0c/0x10/0x14` | `0x42554856` `0x000E1124` `0x00000100` `0x00000E00` | `[v3d60]` ident; ver 42, core/host counts, TFU/TSY/MSO/L3C bits |
| `HUB_INT_MSK_STS` | hub `+0x5c` | `0x00000005` | `[v3d49]` hub mask policy |
| `MMUC_CONTROL` | `+0x1000` | `0x00000001` | MMU cache enabled, no flush pending |
| `MMU_CTL` | `+0x1200` | `0x060D0C01` | **`[v3d62]` MMU_CTL — the full fault policy is bit-identical to a working binner's** |
| `MMU_ADDR_CAP` | `+0x1214` | `0x00000FFF` | cap configured identically |
| `MMU_DEBUG_INFO` | `+0x1238` | `0x00000550` | va/pa width capability word (§49.2 act 7) |
| `+0x1024…+0x103c`, `+0x123c` | mmuc/mmu | `0x4D4D5542` ×7, `0x4D4D5520` | fixed signature words ("BUMM"/" UMM"), identical |
| `CORE0_IDENT0..2` | core0 `+0x00/0x04/0x08` | `0x04443356` `0x81001422` `0x40078121` | core ident |
| `CORE0_MISCCFG` | core0 `+0x18` | `0x00000006` | §49.1's `MISCCFG`/`QRMAXCNT` row, re-confirmed on a third piOS capture |
| `CORE0_INT_MSK_STS` | core0 `+0x5c` | `0x00FF0058` | **`[v3d49]` core mask policy** |
| `CORE0_GMP_STATUS` | core0 `+0x800` | `0x00000030` | GMP idle-and-unarmed on both |
| `CM_V3DCTL` / `CM_V3DDIV` | `0xFE101038` / `0x103C` | `0x000002D4` / `0x00001000` | **`[v3d94]` half B, boot22 — SRC=4 ENAB=1 KILL=0 BUSY=1, DIVI=1 DIVF=0, matched against a piOS whose binner works** |

**The seven core-CTL constants are now MATCHED, and that territory closes.** §49.16 named
core0 `+0x0c/+0x14/+0x20/+0x2c/+0x44/+0x80/+0x84` as the leading *unexamined* words — "unexamined
territory, not excluded territory". All seven read identically on both sides
(`0x00000078`, `0x00000FFF`, `0x00000001`, `0xFFFE0000`, `0x00000045`, `0x007F7F7F`,
`0x0000FFFF`), joined by hub `+0x00/+0x04/+0x18` (`0x0000000F`, `0x00000045`, `0x00000002`) and
`CORE0_L2TFLEND` (`0xFFFFFFFF`). They remain undecoded — the diff names no register — but they are
**excluded as a divergence**, and no future rung may propose one of them as the missing write.

**One identity that must not be read as corroboration.** `CORE0_CT0LC` (core0 `+0x120`) reads
`0x00030000` on both sides. The July piOS dumps read `0x000E0000` idle and `0x2B6C0000` mid-render
at the same offset, so this word is job-state, and tonight's match is a coincidence at the sample
point. It is listed among the 37 for completeness and carries no weight.

#### 49.18.2 Class (b) — piOS holds, we do not

**This is the fold's headline: in the four V3D windows, class (b) is exactly one register.**

| register | offset | piOS | ours | reading |
|---|---|---|---|---|
| `CORE0_BFC` | core0 `+0x134` | `0x00000003` | `0x00000000` (zero) | **bin frame count. This is the wall itself, stated as a register — an output, not an input.** piOS has closed three bin frames; we have closed none. Nothing follows from it about programming |
| CM `+0x0c0` (undecoded) | `0xFE1010C0` | `0x00000040` | `0x00000000` (zero) | a CM slot outside the V3D pair; `+0x0c4` is the converse (class (c) below). Not a V3D register |

That is the whole class. **Every pool, base, size and enable register piOS programs, we program.**
Each register the sitting ask named as a candidate — the populated block at core0 `+0x108…+0x174`,
`+0x148`/`+0x150`, `+0x120`, `+0x124`, `+0x300`/`+0x304`, `+0x800` — is **populated on our side
too**, with a self-consistent value. They belong to the fourth class below, not to (b). The
hypothesis that the PTB frame unit is missing a driver-programmed pool register visible from the
ARM is **refuted at the register level by this capture**.

The `CM[0x100,0x200)` half (44 nonzero piOS words, including a CTL/DIV-shaped pair at
`+0x1c8`/`+0x1cc` carrying `0x000002D4`/`0x00001000`, the same values as the V3D slot) is **not**
class (b): our instrument does not read that window at all. It is unread territory, and §49.18.5
proposes the one-constant fix.

#### 49.18.3 Class (c) — ours nonzero, piOS zero

| register | offset | ours | reading |
|---|---|---|---|
| `CORE0_CT0CS` | core0 `+0x100` | `0x00000070` | `V3D85_CS_WEDGED` — our wedge signature. piOS idle is drained, so `0x0` there is expected |
| `CORE0_PCS` | core0 `+0x130` | `0x00000001` | `BMACTIVE=1 BMBUSY=0` — an **open bin frame that will never close**. piOS holds no open frame |
| `CORE0_PCTR_0_SRC_0_3` / `_4_7` | core0 `+0x660` / `+0x664` | `0x0B200A01` / `0x1D1C0D0C` | our own perfmon mux, armed by `[v3d63]`/`[v3d66]`. piOS attaches no perfmon on the common path (§49.2 act 15) |
| `CORE0_PCTR` outputs | core0 `+0x688/+0x694/+0x698` | free-running | ditto; these are the only three words that move between our own boots |
| CM `+0x0c4` (undecoded) | `0xFE1010C4` | `0x00001388` | see below |
| CM `+0x028` (undecoded, CTL-shaped) | `0xFE101028` | `0x000002D4` (`ENAB=1 BUSY=1`) | **piOS reads `0x00000244` here — same SRC, `ENAB=0`, `BUSY=0`.** Its `+0x02c` DIV partner is `0x00001000` on both. A clock slot we (or the firmware, on our behalf) leave **running** and piOS leaves **off** |

Class (c) is entirely our own instrumentation plus the wedge's own signature — with the single
exception of CM `+0x028`. That word is the only asymmetry in the whole capture pointing at
something *we* do that piOS does not, and its direction is "we enable more, not less". The slot is
undecoded; nothing about the bin wall follows from it, and no rung below rests on it.

#### 49.18.4 The fourth class — both populated, different values — and the arithmetic

Thirty-two words are nonzero on both sides with different values. They are the registers the
sitting ask expected to find in class (b), and they divide cleanly.

*Job-state pointers (control lists).* Both sides hold a bin and a render control list; only the
addresses and lengths differ.

| register | offset | piOS | ours |
|---|---|---|---|
| `CT0QBA` / `CT0QEA` | `+0x160` / `+0x168` | `0x0139C000` / `0x0139C07C` (span 124 B) | `0x0034B000` / `0x0034B00E` (span 14 B) |
| `CT0EA` | `+0x108` | `0x0139C07C` | `0x0034B00E` |
| `CT0CA` | `+0x110` | `0x0139C07C` — **equals EA: consumed** | `0x0034B000` — **equals BA: not advanced** |
| `CT1QBA` / `CT1QEA` | `+0x164` / `+0x16c` | `0x013D4000` / `0x013D4229` (span 553 B) | `0x0031E000` / `0x0031E06A` (span 106 B) |
| `CT1EA` | `+0x10c` | `0x013D4229` | `0x0031E06A` |
| `CT1CA` | `+0x114` | `0x013D4229` — consumed | `0x0031E06A` — **consumed** |
| `CT1RA` (INFERRED, `+4` mirror of `CT0RA`) | `+0x11c` | `0x013D4228` = `CT1QEA − 1` | `0x0031E069` = `CT1QEA − 1` |
| core0 `+0x1a8` / `+0x1ac` (undecoded) | | `0x0139C07C` / `0x013D4229` | `0x0034B00E` / `0x0031E06A` |

Two structural facts fall straight out. `+0x1a8`/`+0x1ac` hold **exact copies of `CT0EA`/`CT1EA`**
on both sides — a mirror pair, whatever it is called. And the CT0-versus-CT1 asymmetry on our side
is the whole campaign in two rows: **our render thread consumed its list to the byte
(`CT1CA == CT1QEA`) while our bin thread never left its list head (`CT0CA == CT0QBA`)** — §49.12's
`rclct1` result and §49.13's wall, both visible in one settled sweep.

*The pool block, and the arithmetic.* This is where the ask expected the answer, so the numbers are
worked in full.

| quantity | piOS | ours |
|---|---|---|
| `CT0QMA` (tile-alloc pool base, `+0x170`) | `0x0129A000` | `0x00328000` |
| `CT0QMS` (pool size, `+0x174`) | `0x0008B000` = 556 KiB | `0x00008000` = 32 KiB |
| pool end (`QMA+QMS`) | `0x01325000` | `0x00330000` |
| `CT0QTS` (tile-state base \| ENABLE, `+0x15c`) | `0x01325002` → base `0x01325000`, ENABLE=1 | `0x00327002` → base `0x00327000`, ENABLE=1 |
| `PTB_BPCA` (`+0x300`) | `0x012A6000` | `0x0032B000` |
| `PTB_BPCS` (`+0x304`) | `0x0007F000` | `0x00005000` |
| `PTB_BPOA` / `BPOS` (`+0x308`/`+0x30c`) | zero | zero |

1. **The pool invariant holds exactly on both sides.** `BPCA + BPCS` = `QMA + QMS`:
   piOS `0x012A6000 + 0x0007F000 = 0x01325000` ✓; ours `0x0032B000 + 0x00005000 = 0x00330000` ✓.
   `BPCA` is a live allocation pointer into the pool the driver handed the PTB, on both sides.
2. **Both `BPCA` offsets are Mesa's reservation formula, to the byte.** §30's `v3d_tile_alloc_sizes`
   reading — `align(layers × tiles_x × tiles_y × 128, 4096) + 8192` — predicts ours at 1 tile as
   `0x1000 + 0x2000 = 0x3000`, and `BPCA − QMA = 0x0032B000 − 0x00328000 = 0x3000` ✓ (the banked
   `V3D56_EXPECTED_EMPTY_BPCA_ADVANCE`). piOS's `BPCA − QMA = 0x012A6000 − 0x0129A000 = 0xC000`,
   which the same formula produces for `align(tiles × 128, 4096) = 0xA000`, i.e. **289–320 tiles**
   — a frame-sized bin, from the same formula, on the same silicon. **The PTB's reservation stage
   runs on our side exactly as it runs on piOS's.**
3. **Overflow is unarmed on both.** `BPOA`/`BPOS` read zero on piOS with `vc4`+`v3d` loaded, as they
   do for us — §49.1's overflow row confirmed a third time, now against a settled piOS.
4. **The one *shape* difference in the whole block: tile-state placement.** piOS puts the tile-state
   array at **exactly the pool end** (`CT0QTS` base `0x01325000` = `QMA+QMS`), contiguous above.
   We put it **one page below the pool base** (`0x00327000` = `QMA − 0x1000`). Both are legal —
   `CT0QTS` is an independent base — and this is the only structural, as opposed to numeric,
   divergence the pool block contains.
5. **The pool is 17.4× larger on piOS**, which is the frame-size difference and nothing more:
   32 KiB is Mesa-legal for our one tile, and `[v3d71]` already ran a 544 KiB mainline-shaped pool
   (§49.1, "Address geometry (mainline-like iovas, 544 KiB pool)" — **closed**).
6. **These pool addresses are allocation-stable across piOS boots.** `CT0QMA`, `CT0QMS`, `CT0QTS`,
   `BPCA` and `BPCS` are **byte-identical** to the 2026-07-29 idle dump, two weeks and a reboot
   earlier, while the control lists moved. The pool and tile-state array are allocated once at
   driver load from a fixed region; the `BPCA` cursor sits at its post-reservation rest position in
   both idle captures. (July's mid-render dump shows the working values: pool `0x09BDF000`/
   `0x00088000`, `BPCA 0x05BA9000` — an entirely different, per-job allocation.)

*Counters and the wedge signature.*

| register | offset | piOS | ours | note |
|---|---|---|---|---|
| `CORE0_CT0PC` | `+0x128` | `0x00000006` | `0x00000003` | primitive-list counter |
| `CORE0_RFC` | `+0x138` | `0x00000003` | `0x00000007` | render frames — **we have retired more than piOS has** |
| core0 `+0x124` (INFERRED `CT1LC`, `+4` mirror of `CT0LC`) | | `0x00000870` | `0x00000007` | |
| core0 `+0x148` and `+0x150` (undecoded) | | `0x00960000` **both** | `0x00010000` **both** | a counter-shaped upper-16 field (150 vs 1), the same value at two offsets on each side. New territory — neither offset appears in the July curated dumps |
| core0 `+0x68` (undecoded, read-only per `[v3d77a]`) | | `0x00000003` | `0x00010001` | **the wedge signature of §46, reproduced exactly**: piOS "fetching", ours bit16 set |
| `MMU_PT_PA_BASE` | `+0x1204` | `0x0000E200` | `0x00000306` | each side's own page table |
| `MMU_ILLEGAL_ADDR` | `+0x1230` | `0x8004003F` | `0x80000305` | ENABLE (bit 31) set on both; the address field is each side's scratch page. piOS read `0x8004028C` in July, so this field is not a configuration constant |
| hub `+0x8c…+0xa8`, mmuc `+0x1004`/`+0x1014` | | larger | smaller | the `[v3d94]` half-A work counters. Two shape observations: `hub+0x8c` is **exactly 16×** `hub+0xa0` on **both** sides (1824/114 and 48/3), and `mmuc+0x1004` **equals** `hub+0xa0` exactly on both sides (`0x72` and `0x3`). Consistent with a beat/transaction pair on a 16-byte bus; a shape, not a name |

#### 49.18.5 Verdict

**The capture refutes its own hypothesis, and that is its value.** The two-pronged hunt's
firmware-side prong was posed as: piOS's driver programs something the PTB frame unit needs that we
leave unprogrammed, and the frame unit sits between thread-0 item-accept and pool-write. The diff
answers it directly and negatively:

1. **Class (b) inside the V3D register windows is one register, `CORE0_BFC`, and it is an output.**
   There is no unprogrammed pool, base, size or enable register on our side.
2. **The frame unit's inputs are all present, and the reservation stage demonstrably ran.** The
   `BPCA+BPCS = QMA+QMS` invariant holds to the byte on both sides, and both `BPCA` offsets are the
   Mesa reservation formula's value for their own frame size. Whatever the PTB fails to do on our
   boots, it does **not** fail before reservation. The wall is strictly downstream of it — which is
   a genuine narrowing of §49.13's "between item-accept and pool-write", from the whole interval to
   its second half.
3. **The V3D 4.2 pools a driver programs are exactly the three the CT0/PTB registers name** — the
   tile-allocation pool (`CT0QMA`/`QMS`), the tile-state array (`CT0QTS`) and the overflow pool
   (`BPOA`/`BPOS`). We program the first two; piOS leaves the third at zero exactly as we do. QPU
   spilling memory is reached through the shader record and the uniform stream, not through a core
   CTL register, and **no core0 register in either sweep carries a spill base** — so there is no
   fourth pool register hiding in this window for us to have missed. Nothing in this file's own
   offset tables or in either capture supports a "missing pool programming" reading.
4. **The two remaining divergences in the pool block are shape, and both are already closed or
   near-closed.** Pool size and mainline-like addresses are §49.1's `[v3d71]` row, **excluded
   measured**. Tile-state placement (pool-end-contiguous vs one page below the base) is the single
   untested shape, and `[v3d71]` came within `0x2000` of it — it is a weak candidate and is
   presented as one.
5. **The clock question is settled from both sides.** `CM_V3DCTL`/`CM_V3DDIV` read
   `0x000002D4`/`0x00001000` on our wedged boots **and** on a piOS whose binner retires frames. The
   V3D clock slot is programmed identically on a working and a broken block; §49.17.2's half-B
   reading needed piOS to be more than a self-consistency check, and now it is one.

**The honest limit, and it is the whole reason the ask had three parts.** This capture is a
**settled** state. Both sides are quiescent in it: piOS has closed its frames and drained, we are
wedged with `BMACTIVE=1` and nothing moving. Every pool-block register compared above is compared
in the one state where neither side is doing anything, and a register the frame unit only holds
*while a frame is open* would read the same on both sides here **whether or not it is the answer**.
The two captures that can see an open frame — `--sweep` under `glxgears` and `--trigger ct0run`
mid-bin — are **still outstanding**. One of three landed.

#### 49.18.6 The next experiment

**No programming rung is licensed by this diff, and the reason is the finding.** §49.17.2 rules
that a "one more register" proposal owes an argument for why it is not already covered; this diff
supplies the opposite — an argument that there is no such register. Proposing one now would mean
re-opening §49.1's address-geometry row on evidence that closes it further. The next experiment is
therefore a **measurement**, in two parts, and a programming rung held in reserve behind a stated
trigger.

**(A) Complete the capture ask — the decisive part, and it costs no kernel change.** Two piOS
captures, same script, same box:

```
~/unaos-bench/tools/v3d-dump-pios.sh --sweep --label mid-render   # with glxgears -fullscreen running
~/unaos-bench/tools/v3d-dump-pios.sh --trigger ct0run --label mid-bin
```

The mid-bin trigger is the one that decides. It is the only capture that shows the PTB frame unit's
registers **with a bin frame open**, which is the exact state our wall lives in and the exact state
tonight's file cannot represent. Concretely, it is the only way to learn what `CORE0_PCS`,
`CT0CS`, `BPCA`/`BPCS`, core0 `+0x148`/`+0x150` and core0 `+0x68` hold on a **working** block
mid-bin — five words for which we currently hold a wedged value and an idle value and no working
open-frame value at all.

**(B) `[v3d94]` half B, window widened — one constant, read-only, licensed by the diff.** The piOS
capture covers `CM[0xFE101000, +0x200)`; our instrument reads `+0x100`. Widen it to `+0x200` and add
the `+0x028` CTL/DIV slot to the decoded station line beside the V3D pair. This is a measurement
rung against a window the comparison now covers and ours does not, so §49.17.2's bar on further
rungs (which is a bar on *programming* proposals against a closed surface) does not apply. It stays
read-only in the strict sense the section already states: no CM write, and no CM password constant
exists anywhere in `v3d.rs`. Expected yield: the 44 unread piOS words become comparable, and the one
class-(c) asymmetry the capture found (CM `+0x028` enabled for us, off for piOS) gets a decoded
reading instead of a raw word. Gate: `./arroyo check` both arches, `kernel8-test` green, the
`[v3d94]` strings present.

**(C) Held in reserve — `UNAOS_V3D_POOLSHAPE`, and the trigger that would license it.** If, and only
if, (A)'s mid-bin capture shows a core0 register holding a value on a working open frame that our
wedged sweep does not hold, a programming rung becomes licensed. Its shape is fixed here so that it
cannot later be widened by argument: feature `v3d_poolshape`, bare, default OFF, knob
`UNAOS_V3D_POOLSHAPE=<variant>`, one variant per build in the `UNAOS_V3D_FIRSTKICK` grammar, an
unrecognised value running inert and saying `recognised=0`. Two variants, and no third:

| variant | change | licensed by |
|---|---|---|
| `tsda-above` | move the tile-state array to exactly `CT0QMA + CT0QMS`, piOS's contiguous layout to the byte, sizes and every other input unchanged | §49.18.4 item 4 — the one untested shape in the pool block. Weak, and named weak: `[v3d71]` placed it `0x2000` from that address and wedged |
| *(reserved)* | whatever register the mid-bin capture names | (A) |

Criterion: the existing PI-V3D-91 bin criterion, with `[v3d93]` and `[v3d56]` poison armed
alongside; the verdict is `BFC` advance and `FLDONE`, and nothing weaker. **Not implemented in this
arc** — this section is the fold and the proposal, and (A) decides whether (C) is ever built.

**Standing.** The exclusion table gains no row from this section by itself; what it gains is the
closure of §49.16's seven unexamined core-CTL constants (§49.18.1), which move from *unexamined* to
**excluded measured**, and the retirement of the "missing pool programming" reading of the
firmware-side prong (§49.18.5). The prong itself stays open, on the two captures that have not been
taken.

---

### 49.19 The mid-bin capture — the frame-open register set, and the end of the register hunt (PI-V3D-96)

§49.18.5 named the honest limit of the settled diff: every pool-block register was compared in the
one state where neither side is doing anything, and "a register the frame unit only holds *while a
frame is open* would read the same on both sides here whether or not it is the answer." §49.18.6(A)
made the mid-bin trigger the decisive experiment. It has been taken.

**The artifacts.** Three files, all in `~/unaos-bench/capture/`, all bench Pi 4B under stock piOS
with `vc4`+`v3d` loaded:

| file | time | kernel | state (per its own registers) |
|---|---|---|---|
| `v3d-dump-pios-sweep-20260812.txt` (**S1**) | 21:11:34 | `6.18.34+rpt-rpi-v8` | settled idle — `CT0CS`/`CT1CS` absent (zero), `PCS`=0, `BFC`=`RFC`=3 |
| `v3d-dump-pios-sweep2-20260812.txt` (**S2**) | 23:31:25 | `6.18.39+rpt-rpi-v8` | **mid-render, not settled** — see below |
| `v3d-dump-pios-ct0run-20260812.txt` | 23:32:32 | `6.18.39+rpt-rpi-v8` | triggered: a curated 66-register snapshot at **t0**, the same list at **t0+1ms**, then a wider **post-settle sweep (MS)** |

`vcgencmd frequency(46)` = `500000992` in all three; S1 was taken with `core=frequency(1)=500000992`
and S2 with `core=324883296`, which is the governor difference the ask expected. A reboot separates
S1 from the other two (kernel `.34` → `.39`).

#### 49.19.1 What instant was actually photographed — stated honestly, before any conclusion

The trigger header reads:

```
TRIGGERED mid-bin: CT0CS=0x00000020 (CTRUN=1) CT0CA=0x012B60D3 QBA=0x012B6000 QEA=0x012B60D3
                   in-span=False polls=3864
```

Three facts about that instant, each of which limits what the capture can be used for:

1. **The poll caught `CTRUN=1` with `CT0CA` already at `QEA`.** The bin control list is 211 bytes
   (`0x012B60D3 − 0x012B6000 = 0xD3`); the control thread had fetched all of it. `in-span=False` is
   the script reporting exactly that — `CA` is not strictly inside `[QBA, QEA)`, it is *at* the end.
   This is **not** a mid-list photograph. What a working CT0 holds while it is still walking a list
   — `CT0CA` between `QBA` and `QEA`, `CT0CS` with `CTRUN=1` and `CA` in span — remains uncaptured.
2. **By the time the snapshot's own read of `CT0CS` executed, it read `0x00000000`.** The poll loop
   observed `0x20`; the register read a few microseconds later shows the control thread already
   stopped. `CT0CS` in the t0 block is therefore a *post*-thread value, and the only witness to
   `CTRUN=1` is the poll line itself.
3. **But a bin frame was genuinely open at t0, and it closed inside the next millisecond.** `PCS` at
   t0 is `0x00000001` — `BMACTIVE=1`, everything else clear — with `BFC=0x000000B8`. At t0+1ms
   `PCS` is `0x00000000` and `BFC` is `0x000000B9`. The frame closed and the bin frame count
   advanced, in the sample window. **This is a real open-bin-frame photograph, taken at the closing
   edge of the frame** — control list consumed, frame not yet retired.

That is the exact instant, and it is a useful one: it is the piOS state that stands where our wedge
stands (`BMACTIVE=1`, frame open) with the one difference that piOS's frame then *closes*. It is not
the instant that answers "what does the block hold while the PTB is mid-bin, list half-walked".

**A second honesty correction, and it is a gift.** S2 is labelled `SETTLED-SWEEP` by the script, but
its registers say otherwise: `CT1CS = 0x00000120` (`CTRUN=1`, `CTRTSD=1` under the INFERRED
`CTNCS` decode) and `PCS = 0x00000004` (`RMACTIVE=1`). The post-settle sweep in the ct0run file
(MS) reads the same two words identically. **S2 and MS are mid-*render* captures**, with a render
frame open and the CT1 thread running. So the brief's "two settled states, diff them for governor
noise" premise does not hold as stated — but the substitution is in our favour: S2/MS are
effectively §49.18.6(A)'s *second* outstanding capture, the sweep under load, delivered unasked.
The campaign now holds **three** pipeline states from piOS — settled (S1), mid-render (S2, MS) and
mid-bin-closing (t0) — instead of two settled ones.

Consequently the classification below uses **S1 = settled** and **S2/MS = mid-render** as the two
comparison states, and reserves FRAME-OPEN for what only the t0 bin-open sample holds.

**Window caveat.** The t0/settle blocks are a *curated* 66-register list, not a sweep. Offsets in
the sweep windows that the curated list does not read have **no mid-bin value at all**: core0
`+0x0c/+0x14/+0x20/+0x2c/+0x44/+0x68/+0x80/+0x84`, `+0x108`, `+0x11c`, `+0x124`, `+0x148`,
`+0x150`, `+0x1a8`, `+0x1ac`, and the hub/MMUC work counters. Three of the five words §49.18.6
named as decisive are delivered (`PCS`, `CT0CS`, `BPCA`/`BPCS`); **core0 `+0x148`/`+0x150` and
core0 `+0x68` are not read at t0 and remain without a working open-frame value.** Conversely the
ct0run file's sweep omits the `CM[0xFE101000,+0x200)` window entirely, so every CM row in an
MS-vs-S1/S2 join is an artifact of the window, not a divergence; CM is compared S1-vs-S2 only.

#### 49.19.2 (a) The triple-diff, classified

Method as §49.18: mechanical join on physical address, a register absent from a sweep read zero,
names from `unaos/crates/kernel/src/arch/aarch64/v3d.rs` and nowhere else, unnamed offsets stay
`core0+0xNNN (undecoded)`. Classes:

- **FRAME-OPEN** — differs in the mid-bin t0 sample and is *not* reachable in either settled or
  mid-render state. This is the set the arc was taken to find.
- **GOVERNOR/STATE-NOISE** — moves between S1 and S2, i.e. between two captures neither of which has
  a bin frame open. Excluded from the frame-open set by construction.
- **COUNTER** — a counter or job-state word that simply holds a different number.
- **JOB-STATE POINTER** — a control-list or pool address, different per allocation by definition.

**The frame-open table.**

| register | offset | mid-bin t0 | t0+1ms | S1 settled | S2 mid-render | class | reading |
|---|---|---|---|---|---|---|---|
| `CORE0_PCS` | core0 `+0x130` | `0x00000001` | `0x00000000` | `0x00000000` | `0x00000004` | **FRAME-OPEN** | `BMACTIVE=1`, BMBUSY/RMACTIVE/RMBUSY/BMOOM clear. **Bit-identical to our wedged `PCS`.** S2's `0x4` is `RMACTIVE` — the render-side analogue |
| `CORE0_GMP_STATUS` | core0 `+0x800` | `0x03030030` | `0x00000030` | `0x00000030` | `0x03000030` | **FRAME-OPEN** (bits 17:16 only) | `0x30` = `RD_ACTIVE\|WR_ACTIVE` on every capture including ours. Bits **24/25** are set in t0, S2 and MS but clear in S1 and in t0+1ms → activity, not bin-open. Bits **16/17** are set **only** in the mid-bin sample. Undecoded upper field; `GMP_CFG`/`GMP_TABLE_ADDR`/`GMP_VIO_*` are **zero at t0**, so the GMP is unarmed on piOS mid-frame exactly as it is on ours |
| `CORE0_CT0CS` | core0 `+0x100` | `0x00000020` (poll) → `0x00000000` (snapshot) | `0x00000000` | `0x00000000` | `0x00000000` | **FRAME-OPEN** (poll line only) | `CTRUN=1` witnessed by the trigger, gone by the register read. Ours wedged: `0x00000070` |
| `CORE0_CT1CS` | core0 `+0x104` | `0x00000000` | `0x00000000` | `0x00000000` | `0x00000120` | GOVERNOR/STATE-NOISE (render-open) | `CTRUN=1`, `CTRTSD=1`. Render thread live in S2/MS — the evidence S2 is not settled |
| `CORE0_BFC` | core0 `+0x134` | `0x000000B8` | `0x000000B9` | `0x00000003` | `0x000000C1` | COUNTER | **advances by exactly 1 across the frame close.** The wall stated as a register, and the retire signal we never see |
| `CORE0_RFC` | core0 `+0x138` | `0x000000B8` | `0x000000B8` | `0x00000003` | `0x000000BF` | COUNTER | unchanged across the bin close, as it must be |
| `CORE0_CT0PC` | core0 `+0x128` | `0x00000006` | `0x00000006` | `0x00000006` | `0x0000003C` | GOVERNOR/STATE-NOISE | equal in t0 and S1, different in S2 — moves between non-bin states |
| `CORE0_CT0LC` | core0 `+0x120` | `0x66B90000` | `0x66B90000` | `0x00030000` | `0x47C10000` | COUNTER | upper-16 list counter; differs in all three, non-monotonic (below) |
| `MMU_ILLEGAL_ADDR` | `+0x1230` | `0x800403B3` | `0x800403B3` | `0x8004003F` | `0x800403B3` | GOVERNOR/STATE-NOISE | ENABLE (bit31) set everywhere; the address field is a per-boot scratch page — it moved with the reboot and then held |
| `CT0QBA`/`QEA`/`CT0CA`, `CT1QBA`/`QEA`/`CT1CA` | `+0x160`/`+0x168`/`+0x110`, `+0x164`/`+0x16c`/`+0x114` | job at `0x012B6000` | same | job at `0x0139C000` | job at `0x02E8F000` | JOB-STATE POINTER | per-submission |
| `CT0QMA`/`QMS`, `CT0QTS`, `PTB_BPCA`/`BPCS` | `+0x170`/`+0x174`, `+0x15c`, `+0x300`/`+0x304` | pool at `0x011DA000` | same | pool at `0x0129A000` | pool at `0x03297000` | JOB-STATE POINTER | arithmetic in §49.19.3 |
| `PTB_BPOA`/`BPOS`/`BXCF` | `+0x308`/`+0x30c`/`+0x310` | **zero** | zero | zero | zero (absent) | *identical* | **overflow is unarmed on piOS even with a 570-tile bin frame open.** §49.1's overflow row, confirmed a fourth time and now in the one state that could have broken it |
| `PCTR_0_EN`/`SRC_0_3`/`SRC_4_7` | `+0x650`/`+0x660`/`+0x664` | **zero** | zero | zero | zero | *identical* | piOS attaches no perfmon mid-frame either — §49.2 act 15, confirmed against an open frame |
| `SLCACTL`, `L2TCACTL`, `L2TFLSTA` | `+0x24`/`+0x30`/`+0x34` | **zero** | zero | zero | zero | *identical* | no cache-control register is held asserted across a frame |
| `CORE0_INT_MSK_STS` | `+0x5c` | `0x00FF0058` | `0x00FF0058` | `0x00FF0058` | `0x00FF0058` | *identical* | **`[v3d49]` mask policy is not changed for an open frame.** `INT_STS` reads zero at t0 and at +1ms |
| `MISCCFG`, `IDENT0..2`, `L2TFLEND`, `MMU_CTL`, `MMU_PT_PA_BASE`, `MMUC_CONTROL`, `MMU_DEBUG_INFO`, hub `IDENT0..3`/`INT_MSK_STS` | — | identical to S1/S2 and (except `PT_PA_BASE`) to ours | | | | *identical* | no configuration word changes when a frame opens |
| core0 `+0x148` | | **not read at t0** | — | `0x00960000` | `0x00750001` | GOVERNOR/STATE-NOISE | S2 sets bit0 for the first time in any capture; ours `0x00010000` |
| core0 `+0x150` | | **not read at t0** | — | `0x00960000` | `0x00BD0000` | GOVERNOR/STATE-NOISE | MS also `0x00BD0000`; the S1 "same value at both offsets" coincidence does **not** survive |
| core0 `+0x68` (undecoded, RO per `[v3d77a]`) | | **not read at t0** | — | `0x00000003` | `0x00000003` | *identical* | MS `0x00000003` too — the §46 wedge-signature word is constant across settled and mid-render on piOS. Ours: `0x00010001` |
| core0 `+0x1a8` | | not read at t0 | — | `0x0139C07C` | `0x02E8F1F7` | JOB-STATE mirror | **equals `CT0CA` in S1, S2 and MS** — mirror confirmed on three captures |
| core0 `+0x1ac` | | not read at t0 | — | `0x013D4229` | `0x1B532000` | *mirror refuted* | S1's `+0x1ac == CT1EA` does **not** hold in S2 (`0x1B532000` bears no relation to `CT1EA=0x02DA4229`) nor in MS (`0x02E90346` = `CT1CA + 1`). §49.18.4's mirror-pair claim survives for `+0x1a8` only |
| core0 `+0x124` (INFERRED `CT1LC`), `+0x11c` (INFERRED `CT1RA`) | | not read at t0 | — | `0x00000870`, `0x013D4228` | `0x00002355`, `0x02DA41C5` | COUNTER / JOB-STATE | `+0x11c` = `CT1QEA − 0x64` in S2, `= CT1QEA − 1` in S1 — a live pointer, consistent with `CT1RA` |
| hub `+0x68` | | not read at t0 | — | `0x00000000` | `0x00000002` | GOVERNOR/STATE-NOISE | new nonzero word under load (MS also `0x2`) |
| hub `+0x84…+0xa8`, mmuc `+0x1004`/`+0x1014` | | not read at t0 | — | small | large | COUNTER | the `[v3d94]` half-A work counters. **Both §49.18.4 shape identities survive**: in S2, `hub+0x8c = 0x01540050 = 16 × 0x00154005 = 16 × hub+0xa0` exactly, and `mmuc+0x1004 = 0x00154005 = hub+0xa0` exactly. In MS the same two identities hold to within the sampling skew of a non-atomic read (`0x020404B0` vs `16 × 0x0020404C`; `0x0020404F` vs `0x0020404C`) |
| `CM[0xFE101000,+0x200)` | | not swept in trigger mode | — | — | — | GOVERNOR-NOISE, 4 words | S1-vs-S2 only: `+0x00c` `0x1000`→`0x16E0`, `+0x02c`/`+0x034`/`+0x1cc` `0x1000`→`0x1560` — all four are DIV partners moving with `core=500MHz`→`324MHz`. **`CM_V3DCTL`/`CM_V3DDIV` (`+0x038`/`+0x03c`) are `0x000002D4`/`0x00001000` in both**, and `+0x028` is `0x00000244` in both. The V3D clock slot does not move with the governor; §49.18.3's `+0x028` asymmetry (ours `0x2D4`, piOS `0x244`) is confirmed stable across two piOS states |
| `PM_GRAFX` `0xFE10010C`, `LEGACY_ASB_V3D_*` `0xFE00A008/00C/020`, `RPIVID_ASB_V3D_*` `0xFEC11008/100C` | | `0x00001040`, `0x5`/`0x5`/`0x62726467`, `0x4`/`0x8060` | `0x8050` at `0xFEC1100C` | **not swept** | **not swept** | UNCOMPARABLE | outside every sweep window. The one intra-file move is `RPIVID_ASB_V3D_M_CTRL` `0x8060`→`0x8050` between t0 and t0+1ms — an ASB master-bridge status bit that follows the frame close. Undecoded, and our instrument does not read this window at all |

**Count.** Inside the comparable V3D windows, the mid-bin sample holds exactly **three** words that
neither the settled nor the mid-render state holds: `PCS = 0x00000001`, `GMP_STATUS` bits 17:16, and
the poll-line `CT0CS = 0x20`. Everything else that differs is a pointer, a counter, or a word that
already moves between two frames-closed captures.

**One more property of the counters, worth recording.** `BFC` reads `0xC1` in S2 at 23:31:25 and
`0xB8` in the t0 snapshot 67 seconds later, with no reboot between them; `RFC` and `CT0LC` behave
the same way. The counters **reset**, which is what a runtime-PM power-down of the V3D domain does.
`BFC`/`RFC`/`CT0LC` are therefore not monotonic across an idle gap, and no future rung may treat a
`BFC` value from one capture as a lower bound on another. Within a single capture — which is how
`[v3d55]`/PI-V3D-91 use it — `BFC` Δ remains exactly the right instrument, and the t0→t0+1ms pair is
the first direct observation of the Δ=1 we are trying to produce.

#### 49.19.3 The arithmetic on the mid-bin pool

| quantity | mid-bin t0 | S1 settled | S2 / MS mid-render | ours (wedged) |
|---|---|---|---|---|
| `CT0QMA` | `0x011DA000` | `0x0129A000` | `0x03297000` | `0x00328000` |
| `CT0QMS` | `0x00038000` (224 KiB) | `0x0008B000` (556 KiB) | `0x00055000` (340 KiB) | `0x00008000` (32 KiB) |
| pool end `QMA+QMS` | `0x01212000` | `0x01325000` | `0x032EC000` | `0x00330000` |
| `PTB_BPCA` | `0x011EE000` | `0x012A6000` | `0x032A6000` | `0x0032B000` |
| `PTB_BPCS` | `0x00024000` | `0x0007F000` | `0x00046000` | `0x00005000` |
| `BPCA+BPCS` | `0x01212000` ✓ | `0x01325000` ✓ | `0x032EC000` ✓ | `0x00330000` ✓ |
| `BPCA − QMA` | `0x14000` | `0xC000` | `0xF000` | `0x3000` |
| implied tiles, `align(N×128,4096)+0x2000` | 545…**576** | 289…320 | 385…416 | 1…32 |
| `CT0QTS` base | `0x012B9000` | `0x01325000` | `0x0EC67000` | `0x00327000` |
| tile-state base − pool end | **`+0xA7000`** | **`0`** | **`+0x0B97B000`** | `−0x9000` |

Four results, and the third is the important one.

1. **The `BPCA + BPCS = QMA + QMS` invariant holds to the byte with a bin frame open.** It is not an
   idle-state identity; it is the pool's structural invariant, live.
2. **Mid-frame `BPCA` sits at exactly the Mesa reservation offset and nowhere past it.**
   `0x14000 − 0x2000 = 0x12000`, and `1920×1200` at 64×64 tiles is `30 × 19 = 570` tiles →
   `align(570 × 128, 4096) = 0x12000` **exactly** — the bench panel's own geometry, and the frame
   size a full-screen client on this box produces. So on a working block, with a frame open and the
   whole bin control list consumed, `BPCA` still reads *post-reservation, pre-advance* — the same
   kind of value ours reads when wedged (`0x3000` = the 1-tile reservation, the banked
   `V3D56_EXPECTED_EMPTY_BPCA_ADVANCE`). **`BPCA` does not discriminate a working open frame from
   our wedged one.** It was one of the five words §49.18.6 wanted, and its answer is "no signal".
3. **`tsda-above` is dead.** §49.18.4 item 4 built its one remaining shape candidate on S1's
   `CT0QTS base == QMA + QMS`. Across three piOS captures that relationship is `0`, `+0xA7000` and
   `+0x0B97B000`. **piOS does not place the tile-state array at the pool end; S1's contiguity was a
   coincidence of one allocation.** The `tsda-above` variant of the reserved
   `UNAOS_V3D_POOLSHAPE` knob is withdrawn — it would have been mirroring an accident.
4. **Pool addresses are not allocation-stable after all.** §49.18.4 item 6 recorded `QMA`/`QMS`/
   `QTS`/`BPCA`/`BPCS` as byte-identical across two idle captures two weeks apart. The reboot to
   kernel `.39` moved all of them, and the mid-bin client holds a *different* pool again
   (`0x011DA000`) from the one S2/MS show (`0x03297000`) at the same moment — two GL clients, two
   pools, live simultaneously. The stability was per-boot, per-client, and carries no weight.

#### 49.19.4 (b) The verdict — the frame-open set contains no input

**Which frame-open registers are plausibly what our silicon lacks when thread 0 sits at
item-accept? None of them, and the capture is unambiguous about why.**

1. **`PCS` mid-bin on a working block is `0x00000001` — bit-identical to our wedged `PCS`.** This is
   the single most decisive number in the file. §46 read our `BMACTIVE=1, BMBUSY=0` as a possible
   pathology; it is not one. It is exactly what a healthy V3D holds with a bin frame open and its
   control list consumed. The difference between piOS and us is not the value of `PCS` — it is that
   one millisecond later piOS's `PCS` goes to `0` and `BFC` goes to `0xB9`, and ours never does.
   **The wall is a missing transition, not a missing bit.**
2. **The only frame-open bits in the entire ARM-visible V3D window are `GMP_STATUS[17:16]`, and they
   are status.** `GMP_CFG`, `GMP_TABLE_ADDR`, `GMP_VIO_ADDR`, `GMP_VIO_TYPE` and
   `GMP_VALID_LINES` all read **zero at t0** — piOS runs its frames with the global memory
   protection unit unconfigured, exactly as we do (`[v3d76]`: `GMP_STATUS = 0x30`, everything else
   zero). Bits 17:16 come up while a bin frame is open and go away when it closes; they are the
   GMP's own activity indication for the traffic the frame generates. There is no GMP register to
   mirror, because piOS programs none.
3. **`CT0CS` `CTRUN` is a control-thread run bit that we already set and already observe.** Nothing
   new.
4. **Everything else the mid-bin sample could have carried, it carries at zero or identical.**
   Overflow (`BPOA`/`BPOS`/`BXCF`) is unarmed with a 570-tile frame open. Perfmon is unattached.
   No cache-control register is held. The interrupt mask is unchanged from settled. `MISCCFG`,
   `MMU_CTL` and every ident word are constant. **The frame unit's ARM-visible state while a frame
   is open is, to the bit, the state we already produce.**
5. **§49.18.5's constraint stands and is now stronger.** The reservation stage runs identically —
   the invariant holds mid-frame on piOS, and both sides' `BPCA` sit at the Mesa formula's value for
   their own frame size. The wall is strictly downstream of reservation. What the mid-bin capture
   adds is that it is also strictly downstream of *every register the ARM can read*: from the
   ARM's side, a working block mid-frame and our wedged block are distinguishable only by `BFC`
   advancing and `PCS` clearing — which are the wall's *effects*, not its causes.

**The register hunt is over.** §49.17.2 closed the surface to "one more register" proposals and
required any future one to argue it was not already covered. §49.18 refuted the missing-pool
reading. This section removes the last hiding place the previous two left open — the "only visible
while a frame is open" register — and finds three status bits and nothing writable. **There is no
CPU-writable register, in any window either instrument reads, whose value distinguishes piOS's
working bin frame from our wedged one.**

#### 49.19.5 (c) The next rung — read-only telemetry means the answer is behavioral

The brief's own fork applies: *if everything frame-open is read-only telemetry, say so and state
what that means for the campaign.* Everything frame-open is read-only telemetry.

**(A) No programming rung is proposed, and `UNAOS_V3D_POOLSHAPE` is withdrawn entirely.**
§49.18.6(C) held the knob in reserve behind a stated trigger: "if, and only if, (A)'s mid-bin
capture shows a core0 register holding a value on a working open frame that our wedged sweep does
not hold". The trigger did not fire — the three words that qualify are `PCS` (which we already hold
at the same value), `GMP_STATUS[17:16]` (read-only status of an unconfigured unit) and `CT0CS`
`CTRUN` (which we already set). Its `tsda-above` variant is independently refuted by §49.19.3
item 3. The knob is not built, and the reserved second variant has no register to name. **No new
row is added to the exclusion table by a rung; the rows added are measurements.**

**(B) The bunker prong is now the campaign's main line, not its alternative.** If the difference
between a block that retires bin frames and one that does not is invisible in every ARM-readable
register at the instant it matters, the difference is not in the register file the ARM writes. It is
in what the VPU firmware does to the block that we do not — an initialisation sequence, an ASB or PM
handshake, a clock/reset ordering, or a value written through a path the ARM does not see. That is
`start4.elf` analysis, and the honest statement is that **the campaign's remaining probability mass
sits there.** Two concrete, low-cost read-only measurements would sharpen the target before any
disassembly begins:

| measurement | what it reads | why it is licensed now |
|---|---|---|
| widen `[v3d94]` half B to `CM[0xFE101000,+0x200)` and decode the `+0x028` slot | the 44 CM words our instrument does not read, plus the one asymmetry §49.18.3 found (`+0x028` = `0x2D4` ours, `0x244` piOS, now confirmed stable across **two** piOS states) | unchanged from §49.18.6(B); it is a measurement against a window the comparison covers and ours does not. Gate: `./arroyo check` both arches, `kernel8-test` green, `[v3d94]` strings present |
| add the **PM/ASB windows** to our sweep: `PM_GRAFX 0xFE10010C`, `LEGACY_ASB_V3D_S/M_CTRL 0xFE00A008/0x00C`, `RPIVID_ASB_V3D_S/M_CTRL 0xFEC11008/0x100C` | piOS mid-bin holds `0x00001040`, `0x5`/`0x5`, `0x4`/`0x8060`, and `RPIVID_ASB_V3D_M_CTRL` **moves `0x8060 → 0x8050` across the frame close** | **new territory, and the only window in the mid-bin file that our instrument cannot compare at all.** It is also the exact class of surface — power-domain and AXI-bridge handshake — that a firmware-behavioral difference would live in. Read-only, four extra words |

The second row is the one this section actually recommends first. It is the only unread window left
in the mid-bin capture, and a bridge-status bit that changes state precisely when a bin frame closes
is the first thing in this campaign to correlate with the transition we cannot produce. It proves
nothing by itself; it is the cheapest way to find out whether our block's ASB state at
`START_TILE_BINNING` even resembles piOS's.

> **BOTH ROWS MEASURED — see §49.20.2 V6** (boot4/PA40). *Row 2, PM/ASB:* our side is now swept, and
> four of the five words are **bit-identical** to piOS's mid-bin capture — `PM_GRAFX 0x00001040`,
> both legacy words `0x00000005`, `RPIVID … S_CTRL 0x00000004`. The fifth, `RPIVID … M_CTRL`, matches
> in its low nibble (`0x4`) and lacks piOS's upper activity bits, which §46.2 had already classified
> as bridge activity status and §49.20.4(a) reconciles across all four recorded piOS values. *Row 1,
> CM widen:* `[v3d94]` now sweeps `CM[0xFE101000, 0xFE101200)` — 86 nonzero words, joined against
> piOS S1's 87: **84 identical, 4 divergent**, three of them already known (`+0x028`, `+0x0c0`,
> `+0x0c4`) and one new (`+0x120`, `0x1` on piOS, zero on ours, undecoded). The `+0x1c8`/`+0x1cc`
> pair §49.18.2 flagged as unread reads `0x000002D4`/`0x00001000` on our side — identical to piOS.
> **(B)'s premise is also weakened by that boot:** see §49.20.6 R4 and §49.20.7 — at cold handoff the
> block is held in reset (`PM_GRAFX 0x00001000`, `V3DRSTN = 0`) with both bridges stopped, so there
> is no surviving firmware-established V3D register state for a disassembly to find.

**(C) Criterion, unchanged, for whatever eventually gets built.** The PI-V3D-91 bin criterion with
`[v3d93]` and `[v3d56]` poison armed, and the verdict is `BFC` advance plus `FLDONE`, and nothing
weaker. The t0→t0+1ms pair in this capture is the first time the campaign has seen that exact
signature on the bench — `PCS 0x1 → 0x0`, `BFC 0xB8 → 0xB9` — and it is what the criterion is
written to detect.

**Standing.** `tsda-above` and the whole of `UNAOS_V3D_POOLSHAPE` move to **excluded measured**.
`PCS = 0x1` as a wedge pathology moves to **excluded measured** — it is the healthy open-frame
value. `BPCA` position as a discriminator moves to **excluded measured**. §49.18.6(A)'s three-part
capture ask is **complete**: settled (S1), mid-render (S2/MS, by accident of labelling) and mid-bin
(t0). The firmware-side prong closes on the register-comparison question with a negative answer, and
what remains of it is behavioral.

---

### 49.20 The unarmed close, read on metal — the write exists, the MMU refuses it, and the aim is arithmetic on a zero base (PI-V3D-95 verdict, boot4/PA40, 2026-08-17)

§49.12.1 built `[v3d95]` and §49.10 queued the question it answers: *where do the writes of an
unarmed frame-close go?* The rung flew. This section folds its verdicts, and it also folds the two
read-only measurements §49.19.5(B) asked for, which rode the same boot.

**One correction is load-bearing and is stated before the evidence rather than after it.** The
boot did **not** decide the fork §49.12.1 wrote for it. That fork needed the fault VA read against
a **different** `CT0QMA`/`CT0QMS`/`CT0QTS` residue; the residue this boot found was **zero**, which
is the one value under which "structural `0x70`" and "base `0` + offset `0x70`" are
indistinguishable. Worse for the structural reading, a second PTB pointer on the same boot is
demonstrably doing base-plus-offset arithmetic on that same zero base (§49.20.2 V5). The honest
standing is therefore **the write is located, the aim is not**, and §49.20.6 R1 is the rung that
locates it.

#### 49.20.1 The capture, and what kind of boot it was

`~/unaos-bench/capture/pi4-pi0-b1/ttyACM0.log`, boot 4 of that session — the fourth
`MESS:…: Loaded 'kernel8.img' to 0x80000 size 0x1d3686` in the file. The operator mark reads
`2026-08-17T22:37:49Z MARK boot4 pi4 power-cycle — identity from Loaded line`; the arc brief labels
the image `20260813T0620Z-f51e42be`, which the wire itself does not carry, so that identity is the
operator's and is cited as such. The verdict slice is banked at
`~/unaos-bench/scratch/pi0-b1b2/boot4-raw.txt` (extraction: `tr -d '\000'` then `awk`, per
`CLAUDE.md` — the log carries control bytes and `grep` mis-reads it).

Two independent lines establish this as a **cold** boot, which matters for §49.20.2 V6:

```
:: PIUSB: PIUSB-16: ENTRY link state — RGR1_SW_INIT_1=0x00000003 PCIE_STATUS=0x00000000
   (PHYLINKUP=false DL_ACTIVE=false) -> VC left RC in reset — COLD-BUILD pa…
```

and the events-log mark quoted above. It is a **full** boot — PIUSB, GENET, the K1/K2 batteries and
the compositor all ran — with the V3D bring-up returning early at the rung's own boundary:

```
:: V3D: [v3d95] UNARMED-CLOSE BOOT COMPLETE — one CT0 kick taken and nothing else. probe_job, the
   [v3d48] ladder, M4 and the whole visible battery were NOT run this boot, by design. Re-run
   without UNAOS_V3D_UNARMCLOSE for a full boot ::
```

Per §49.12.1's own warning, nothing below may be diffed line-for-line against a deep boot.

The submitted list is the `emptyunarm` shape, decoded on the wire — `NUMBER_OF_LAYERS`,
`TILE_BINNING_MODE_CFG w=64 h=64`, `FLUSH_VCD_CACHE`, `START_TILE_BINNING`, `FLUSH` — 14 bytes at
`BA=0x009f3000`, and `[v3d54] submit` audits it `BA OK EA OK span OK — submission SOUND`. Every
verdict below is entitled to assume the CLE was handed the list we built.

#### 49.20.2 The verdicts, each with its wire line and each with its exclusion

**V1 — an unarmed submit CLOSES a bin frame. `BFC 0 → 1`.**

```
:: V3D: [v3d41] v3d95 unarmed close frame counters — BFC 0x00000000->0x00000001 (Δ1)
   RFC 0x00000001->0x00000001 (Δ0) — BIN FRAME COMPLETED ::
```

*Excludes:* the "no frame closed" row of §49.12.1's outcome table, and with it §49.10's row-B
re-take instruction. boot11's premise — a frame that closes with no `CT0QMA`/`CT0QMS`/`CT0QTS`, no
`BPOS=0` and no pre-kick L2T invalidate — is **reproduced on a second boot, two weeks apart, on a
different image**. *Does not exclude:* anything about the frame's contents; `FRDONE=0` and the
`[v3d56] int` line's `FLDONE(bit1): latched=1` are the same pairing this campaign has seen
throughout.

**V2 — the close-time write EXISTS, and the V3D MMU REFUSED it. Client PTB, VA `0x70`.**

```
:: V3D: [v3d95] unarmed-close verdict — … | MMU_CTL=0x061d1c01 fault=1 (PT_INVALID=1
   WRITE_VIOLATION=1 CAP_EXCEEDED=0) VIO_ADDR=0x0000000e VIO_ID=0x00000020
   (client PTB @ VA 0x00000070) | … wrote-any=0 ::
```

`VIO_ID 0x20 >> 5 = 1 = PTB` on the `v3d_irq.c` client table this file banked at §26;
`VIO_ADDR 0xe << 3 = 0x70` on the `DEBUG_INFO=0x00000550` → va_width 35 → shift 3 decode banked in
the same place. Both raw words are **byte-identical to boot11's** (§49.10: `VIO_ADDR=0x0000000e
VIO_ID=0x00000020 (client PTB @ VA 0x00000070)`).

*Excludes:* the "no fault, poison intact" row — an unarmed close is **not** a silent no-op; it
issues a memory write. It also excludes any remaining reading in which the PTB never attempts a
write at all: §48's CLE-side and §49.11's class-side results said the *walk* was fine, and this says
the *write* is attempted and rejected at translation.

*Does not exclude — and this is the correction:* the two boots that produced `VA 0x70` had
**identical** preconditions in the one column that discriminates. boot11's residue was zero
(nothing was programmed), boot4's residue is printed and is zero:

```
:: V3D: [v3d95] tile-memory PRE (virgin — no CT0 kick and no CT0QMA/QMS/QTS write of ours in front
   of this read) — CT0QMA=0x00000000 CT0QMS=0x00000000 CT0QTS=0x00000000 (ENABLE=0) ::
```

Both boots also submitted a **14-byte** list (boot11 `QBA=0x0097c000 QEA=0x0097c00e`; boot4
`BA=0x009f3000 EA=0x009f300e`), and `0xe` is that length. The reproduction is real and valuable, but
it holds three things fixed at once — zero bases, zero-based aim, 14-byte list — and `VA 0x70` is
consistent with all three. §49.20.6 R1 and R3 are the two cuts.

**V2a — an instrument fact the next rung must carry: the violation pair is NOT sticky.**

Three reads of `VIO_ADDR`/`VIO_ID` were taken across one latched window, in this order, with no W1C
between them. The first (the `[v3d95]` verdict station, read immediately after the wait) got
`0x0000000e`/`0x00000020`. The second (`[v3d62] fault`, printed earlier in the log because the
verdict line is assembled later) got zero:

```
:: V3D: [v3d62] fault (v3d95 unarmed close) — MMU_CTL=0x061d1c01 fault-latched=0x00101000
   (PT_INVALID=1 WRITE_VIOLATION=1 CAP_EXCEEDED=0) VIO_ADDR=0x00000000 VIO_ID=0x00000000
   DEBUG_INFO=0x00000550 | HUB_INT_STS=0x00000030 MMU-int bits=0x00000030 (PTI=1 WRV=1 CAP=0) ::
```

and the third (the post-kick hygiene clear) got zero and therefore **decoded zero as a client**:

```
:: V3D: MMU fault-latch CLEARED (v3d95 post-kick) — was CTL=0x061d1c01 (PT_INVALID=1
   WRITE_VIOLATION=1 CAP_EXCEEDED=0) VIO_ADDR=0x00000000 VIO_ID=0x00000000
   (client L2T @ VA 0x00000000) -> CTL=0x060d0c01 ::
```

**No reader may cite that `L2T`.** It is `CLIENTS[0]` applied to a zero `VIO_ID`, not an
attribution. Two explanations fit the sequence — read-to-clear on the pair, or a clobber during the
serial output between the reads — and this boot cannot separate them. The operational rule is the
same under both, and it is now a standing instrument law for this file: **read the violation pair
once, first, before any other MMU read or any print, and treat every later read of it as void.**
`MMU_CTL`'s latch and `HUB_INT_STS`'s `PTI|WRV` bits *are* sticky and corroborate the refusal
independently; only the *client* and the *address* rest on the single early read.

**V3 — the refused write LANDED. The `[v3d62]` catcher absorbed 48 words.**

From the `[v3d62] fault` line above:

```
   … | scratch page 0x9ac000: dirty words=48/1024 first-dirty off=0x0 val=0x00000000 — AN ACCESS
   WAS REDIRECTED TO THE ILLEGAL-ADDRESS SCRATCH PAGE DURING THIS FRAME …
```

The page was seeded immediately before the kick — `scratch page 0x9ac000 seeded with 0x5ca7c411
x1024 words` — on a page the `[v3d62] mmufix` line proves belongs to nothing:
`PA=0x9ac000 (pfn=0x9ac) PTE=0x00000000 unmapped=1 in-arena=0`.

This **confirms the mechanism V3D-60 named and §26 has been chasing since**: a write the MMU refuses
does not simply vanish; the `MMU_ILLEGAL_ADDR` redirect carries it to the catch page. It is the
first direct observation of "the refused write lands somewhere unaccounted", and it explains
"`BPCA` advances, pool stays empty" without any further hypothesis.

Two facts about the payload, and one non-fact. The first dirty word is at **offset `0x0`**, not at
`0x70` — the redirect does not preserve the low offset of the refused VA, so the catcher page tells
us *how much* was written, not *where it was aimed*. And the value written is **`0x00000000`**: the
payload is zeros, not primitive-list bytes. The non-fact: the instrument prints a count and a first
offset, **not a span**, so "48 contiguous words / 192 bytes" is **not** established by this line —
only "48 of 1024 words no longer hold the sentinel".

*Excludes:* every reading in which the close path issues no store. *Does not exclude:* it does not
identify the store's semantic — 48 zero words is equally consistent with a tile-state clear, a
primitive-list header zero-fill, and a burst the fabric split.

**V4 — nothing GPU-reachable was written. Poison 64/64 and 8192/8192 INTACT, arena `STRAY=0`.**

```
:: V3D: [v3d56] poison (v3d95 unarmed close) tile-state iova=0x009cf000 words=64 — INTACT=64
   ZEROED=0 OVERWRITTEN=0 touched=0 … L2T write-back completed=1 — POISON FULLY INTACT after a
   completed write-back: the PTB wrote NOTHING here ::
:: V3D: [v3d56] poison (v3d95 unarmed close) tile-alloc pool iova=0x009d0000 words=8192 —
   INTACT=8192 ZEROED=0 OVERWRITTEN=0 touched=0 … ::
:: V3D: [v3d56] landing (v3d95 unarmed close) — arena 64 pages (0x40000 B @ 0x009be000, the ENTIRE
   address space the V3D MMU grants this job) | changed=0 expected=0 STRAY=0 … ::
```

*Excludes:* §49.12.1's "no fault, arena bytes moved" row, and any theory in which the close path
reaches the tile-state array or the pool through some path other than the bases. The `[v3d68]`
poison discipline is what makes this citable: a pre-zeroed region could not have distinguished
"never written" from "written with zeros", and V3 shows the write's payload **is** zeros — so on a
zeroed region this verdict would have been unreachable.

**V5 — the pool arithmetic ran, on a zero base. This is the new finding, and it is the one that
moves the campaign.**

```
:: V3D: [v3d41] v3d95 unarmed close CLE feed + PTB pointer — CT0LC=0x00010000 CT0PC=0x00000000
   PCS=0x00000000 (raw) | BPCA 0x00000000->0x00003000 (out-of-span) (pool base 0x009d0000)
   BPCS=0xffffd000 BPOA=0x00000000 BPOS=0x00000000 ::
```

Read the numbers against §49.18.4's two banked invariants, both established there on a settled piOS
and on our own settled sweep:

1. **`BPCA − QMA` = the Mesa reservation offset.** §49.18.4 item 2 banks
   `V3D56_EXPECTED_EMPTY_BPCA_ADVANCE = 0x3000` for a one-tile bin. Here `QMA = 0` and
   `BPCA = 0x00003000`. **`BPCA − QMA = 0x3000` — exact.**
2. **`BPCA + BPCS = QMA + QMS`.** Here `0x00003000 + 0xffffd000 = 0x1_00000000 ≡ 0` (mod 2³²), and
   `QMA + QMS = 0`. **The invariant holds exactly, modulo the register width.** `BPCS = 0xffffd000`
   is `−0x3000`: the PTB computed "space remaining = (base + size) − cursor" with base and size both
   zero, and stored the negative.

The PTB therefore **ran its normal reservation stage, reading `CT0QMA` and `CT0QMS`, and got the
right answer for the pool it was told about** — a pool at address 0 of size 0. It is not ignoring
those registers; it is obeying them. `(out-of-span)` in the instrument's own text is measured
against `pool base 0x009d0000`, the base an *armed* rung would have written and this rung
deliberately did not — a correct label for the armed case and a misleading one here. In the pool the
PTB actually holds, `BPCA` is exactly where the formula puts it.

*Excludes:* "the PTB does not follow `CT0QMA`/`CT0QMS`" — refuted, on this boot, by the arithmetic.
*Bears directly on V2:* with one PTB pointer demonstrably computed as `base + offset` on a zero
base, `VA 0x70` under a zero base is most parsimoniously read as **`base 0 + 0x70`**, not as a fixed
structural address. Parsimony is not proof; R1 is the proof.

**V6 — the PM/ASB window, swept on our side for the first time, and the CM window widened. Both of
§49.19.5(B)'s asks are discharged in one boot.**

*Row 2 of that table — PM/ASB.* Our values, at the `[v3d94]`/sweep station (after the mailbox
domain-on and clock enable, **before** the `[v3d50]` reset cycle):

```
SWEEP phys=0xfe10010c val=0x00001000 pmasb=PM_GRAFX
SWEEP phys=0xfe00a008 val=0x00000005 pmasb=LEGACY_ASB_V3D_S_CTRL
SWEEP phys=0xfe00a00c val=0x00000005 pmasb=LEGACY_ASB_V3D_M_CTRL
SWEEP phys=0xfec11008 val=0x00000007 pmasb=RPIVID_ASB_V3D_S_CTRL
SWEEP phys=0xfec1100c val=0x00000007 pmasb=RPIVID_ASB_V3D_M_CTRL
```

and after the `[v3d50]` ON half:

```
:: V3D: PM_GRAFX readback 0x00001040 ::
:: V3D: PM/ASB V3D master (ASB_V3D_M_CTRL) readback 0x00000004 — ACK clear (bridge released) ::
:: V3D: PM/ASB V3D slave  (ASB_V3D_S_CTRL) readback 0x00000004 — ACK clear (bridge released) ::
```

piOS's mid-bin values, re-read from the artifact rather than from the doc
(`~/unaos-bench/capture/v3d-dump-pios-ct0run-20260812.txt`): `PM_GRAFX 0x00001040`,
`LEGACY … S/M_CTRL 0x00000005`/`0x00000005`, `RPIVID … S_CTRL 0x00000004`,
`RPIVID … M_CTRL 0x00008060` at t0 and `0x00008050` at settle.

| word | piOS mid-bin | ours, released | verdict |
|---|---|---|---|
| `PM_GRAFX` | `0x00001040` | `0x00001040` | **identical** |
| `LEGACY_ASB_V3D_S_CTRL` | `0x00000005` | `0x00000005` | **identical** |
| `LEGACY_ASB_V3D_M_CTRL` | `0x00000005` | `0x00000005` | **identical** |
| `RPIVID_ASB_V3D_S_CTRL` | `0x00000004` | `0x00000004` | **identical** |
| `RPIVID_ASB_V3D_M_CTRL` | `0x00008060` / `0x00008050` | `0x00000004` | low nibble identical; piOS carries upper activity bits |

**The window §49.19.5 called "the only window in the mid-bin file that our instrument cannot compare
at all" is now compared, and four of five words are bit-identical.** The fifth is the `M_CTRL`
activity field, and §49.20.4 reconciles it against the `0x4040` record.

*Row 1 of that table — the CM widen.* `[v3d94]` now sweeps `CM[0xFE101000, 0xFE101200)`:
`86 nonzero words in CM[0xfe101000,0xfe101200); READ-ONLY (no CM write, and no CM password constant
exists in this file)`. Joined on physical address against piOS S1's 87 nonzero CM words (union 88):
**84 identical, 4 divergent.**

| offset | ours | piOS S1 | standing |
|---|---|---|---|
| `+0x028` | `0x000002D4` | `0x00000244` | §49.18.3's asymmetry, **third confirmation**; ours enables, piOS does not; undecoded |
| `+0x0c0` | zero | `0x00000040` | §49.18.2's second class-(b) row, re-confirmed |
| `+0x0c4` | `0x00001388` | zero | §49.18.3, re-confirmed |
| `+0x120` | zero | `0x00000001` | **new** — the only divergence the widened half exposed |

And the pair §49.18.2 flagged as unread — `CM +0x1c8`/`+0x1cc`, "a CTL/DIV-shaped pair carrying
`0x000002D4`/`0x00001000`, the same values as the V3D slot" — reads on our side
`SWEEP phys=0xfe1011c8 val=0x000002d4` and `SWEEP phys=0xfe1011cc val=0x00001000`: **identical to
piOS.** The previously-unread CM half contributes exactly one new word, `+0x120 = 0x1`, undecoded,
in a block (`+0x110…+0x128`) whose neighbours are `0x636D`-signature and divisor words. It is
recorded, and nothing rests on it.

**V7 — the `[v3d50]` OFF→ON reset cycle ran clean end to end**, and it carries a handoff fact.

```
:: V3D: [v3d50] reset OFF — ASB V3D master (ASB_V3D_M_CTRL) readback 0x00000007 — ACK set (bridge stopped) ::
:: V3D: [v3d50] reset OFF — ASB V3D slave  (ASB_V3D_S_CTRL) readback 0x00000007 — ACK set (bridge stopped) ::
:: V3D: [v3d50] reset OFF — PM_GRAFX assert V3DRSTN(clear bit6): pre=0x00001000 post=0x00001000 (PM_V3DRSTN now 0) ::
…
:: V3D: probe verdict BLOCK-UP — hub IDENT0 = 0x42554856 (live V3D identity) ::
:: V3D: PRESENT — tech version 4.2 (ver=42, expect V3D 4.2 = 42 on Pi 4); cores=1 ::
```

**The reset-assert was a no-op because the bit was already clear** (`pre == post`, both `0x1000`),
and the block was dark before the cycle:

```
:: V3D: [v3d60] residue (pre-reset) — SKIPPED: hub IDENT0 reads 0xdeadbeef (open-bus/firmware
   poison) before our reset cycle, so no core register may be read here. The warm-handoff question
   is UNANSWERED this boot ::
```

§46.2's `P89-KMSCOND` row already banked *"entry bridges still 0x7 … start4.elf does not establish
the condition, overlay or not"*. This boot **corroborates that on a cold power-cycle and adds the
`PM_GRAFX` half**: at handoff, and even after our own ACKed `SET_DOMAIN_STATE(10, ON)` (`:: V3D:
power domain 10 ON ::`), `V3DRSTN` is **0** — the block is not merely bridge-isolated, it is **held
in reset**. That is why the hub reads poison, and it is the single most consequential fact for the
bunker prong (§49.20.7).

#### 49.20.3 What these verdicts retire, and the one they do not

| standing item | status after boot4 |
|---|---|
| §49.12.1 outcome row "no frame closed" | **excluded measured** (V1) |
| §49.12.1 outcome row "no fault, poison intact" | **excluded measured** (V2 + V4) |
| §49.12.1 outcome row "no fault, arena bytes moved" | **excluded measured** (V4) |
| §49.10 row-B re-take instruction | **discharged** — the pair agrees; row B never fired |
| V3D-60's "a refused write lands somewhere unaccounted" | **CONFIRMED as mechanism** (V3) — no longer a hypothesis |
| "the PTB does not follow `CT0QMA`/`CT0QMS`" | **refuted** (V5 arithmetic) |
| §49.19.5(B) row 1 (CM widen) | **measured** (V6): 84/88 identical, one new word `+0x120` |
| §49.19.5(B) row 2 (PM/ASB sweep) | **measured** (V6): 4 of 5 words bit-identical to piOS mid-bin |
| §49.12.1's "the aim is structural" branch | **NOT established** — see below |

**The branch that did not fire.** §49.12.1 wrote the fork precisely: *"a VA derived from the residue
means the close path follows the never-programmed registers, a fixed `0x70` under a **different**
residue means the aim is structural."* The residue was zero on both boots that produced `0x70`.
Under a zero residue the two arms of the fork make the **same** prediction for a base-relative aim
with offset `0x70`, so the observation separates nothing. Any summary that reports "VA `0x70` with
zero residue ⇒ the address is structural" is reading the fork backwards, and V5 supplies a positive
reason to expect the opposite: on this very boot a PTB pointer was computed as base-plus-offset with
that base equal to zero.

The section records this as a **contradiction against the arc's own framing, not as a smoothing**.
`VA 0x70` is located; the aim is undetermined; §49.20.6 R1 determines it in one boot.

#### 49.20.4 Reconciliations against earlier sections

**(a) `RPIVID_ASB_V3D_M_CTRL` — the `0x4040` datum, and what today's sweep does to it.** The
record, in the order the file made it:

| where | capture | `M_CTRL` |
|---|---|---|
| §46.1 | piOS mid-bin dump (2026-07-30) | `0x4040` — named as a divergence "against every wedged UnaOS reading" |
| §46.2 idle-dump recheck | V3D-71-era `v3d-dump-idle.txt` | `0x4` — **our value**; the row's own conclusion: *"The high bits are bridge ACTIVITY STATUS (an effect of AXI traffic), not an enabling condition"* |
| §46.2 `[v3d75b]` | `M_CTRL = pw\|0x4040` transplant | did not hold; mid-bin2 later showed the bits cycling `0x4050 → 0x4060` **live** |
| §49.14 boot20 | our domain cycle | `0x00000004` at all four stations |
| §49.19 | piOS mid-bin `ct0run` (2026-08-12) | `0x8060` at t0, `0x8050` after settle |
| **§49.20, this boot** | ours, cold entry → released | `0x7` → `0x00000004` |

There is a surface tension in the file — §46.1 says "piOS mid-bin `0x4040`" and §49.19 says "piOS
mid-bin `0x8060`" — and it is **not** a contradiction: they are two different captures, taken
sixteen days apart on two piOS kernels, of a field §46.2 had already classified as live activity
status and had already watched **cycle** within one capture. Four distinct piOS values now exist
(`0x4040`, `0x4050→0x4060`, `0x8060`, `0x8050`) against one piOS idle value (`0x4`) — and `0x4` is
ours, in every state we have ever read, including `[v3d71f]`'s 500 in-flight samples across an open
frame (§42, §49.1's fabric row).

The reconciliation, stated so a later reader does not re-open it: **the low nibble is the
configuration and it matches; the upper bits are traffic and they do not, because we generate no
traffic.** V4 is the reason — the PTB's write never reaches the fabric as a completed bus
transaction on our side, so a bridge-activity field has nothing to report. The upper bits are
therefore an **effect of the wall**, downstream of it, and they cannot be its cause. `0x4040` stays
where §46.2 put it and where §49.14 closed it: not revived, not a knob, and now with our-side values
under it for the first time.

**(b) §49.12.1's "the window this driver never touches" — corrected.** §49.12.1's *"The §49.16
intersection, named and not acted on"* paragraph says the settled piOS sweep found piOS programming
pool/base registers in core0 `+0x108..+0x174` *"this driver never touches"*. That sentence was
written against §49.16's **ask**; §49.18.2 then folded the **answer** and it reads the other way —
*"Every pool, base, size and enable register piOS programs, we program"*, with `CT0QMA`/`CT0QMS`/
`CT0QTS` sitting in §49.18.4's fourth class (both populated, different values), not in class (b).

The correct statement, which today's boot makes precise: **the driver's armed bin path programs all
three; the `[v3d95]` rung deliberately programs none of them, which is why they read zero here.**
The §49.12.1 paragraph is left standing as written — it is a record of what the arc believed when it
built the rung — and this is the appended correction. Nothing in the rung's design depended on the
wrong half: the rung's job was to read the residue, and it read it.

#### 49.20.5 The wall statement

Before this boot: *"the PTB advances `BPCA` but writes nothing — why does the PTB write nothing?"*

After it, that question is **answered and retired**. The PTB does not write nothing. It runs its
reservation arithmetic correctly on the bases it is given (V5), it issues a write at frame close
(V2), and the write is refused by the V3D MMU as page-table-invalid plus write-violation and
redirected into the illegal-address catcher (V3), which is why no byte the GPU can reach ever
changes (V4).

**The new wall:** *why are the PTB's close-time writes aimed at addresses the V3D MMU does not
map?* Concretely, on the two boots we have: why `VA 0x70`, and is `0x70` an absolute address or an
offset from a base that happened to be zero?

This is a **page-table / address-programming question**, and it is a materially better one than its
predecessor for three reasons. It is *local* — one address, one client, one instant. It is
*instrumented* — the catcher already proves landing, the poison already proves non-landing, and the
violation pair already names client and VA. And it is *cheap to cut*: the whole fork turns on
changing one input and re-reading one register, which is R1 below.

> **APPENDED CORRECTION (§49.22.4 item 2, PI-V3D-97, 2026-08-18).** The wall statement above is
> **SUPERSEDED by §49.22.5**. Its plural — *"the PTB's close-time writes"* — was doing unearned work: an
> unarmed close issues **two** writes, not one. The pool header write is aimed at `CT0QMA` and **lands**
> when `CT0QMA` is mapped, so for that write the question is answered and retired; a second write, ~48
> words of zeros, client PTB, remains refused at `VA 0x0` on a leg where every programmable base is
> mapped. `VA 0x70` is the first write on a zero base, and it is not evidence for a structural aim.

#### 49.20.6 The next rung, on paper — candidates, cost, discriminating power, order

All four candidates ride the existing `[v3d95]` scaffolding: one kick, `[v3d62]` catcher armed,
`[v3d56]` poison as negative controls, the boot returns. None requires a new instrument family.
All must carry the **V2a instrument law** — violation pair read once, first, before any other MMU
read or print.

**R1 — `basedaim`: distinct nonzero bases, then the same bare unarmed close. RECOMMENDED FIRST.**

Program **only** `CT0QMA`, `CT0QMS` and `CT0QTS`, to three **mutually distinguishable, arena-mapped**
values — the point of distinctness is that the fault VA must be attributable to exactly one of them.
Program nothing else: no `BPOS=0`, no pre-kick L2T invalidate, no `CT0QTS.ENABLE` beyond what the
base word carries, and the same 14-byte `emptyunarm` list at the same address. Then read `VIO_ADDR`.

Outcomes, pre-written:

| `VA` reads | conclusion |
|---|---|
| `QMA + 0x70` | the close-time write is the **pool** write, base-relative. The pool base is load-bearing at *close* time. The wall becomes "why is `QMA` not honoured / why is the pool page not mapped at that instant" — and since we *do* map the pool, it becomes "why did the MMU refuse a mapped address", a translation question with a small answer space |
| `QTS + 0x70` | it is the **tile-state** write. §49.18.4 item 4's structural divergence (piOS puts tile-state at the pool end, we put it one page below the pool base) becomes a live suspect for the first time |
| `0x70`, unmoved | the aim is genuinely **absolute**, and R2 is licensed. Only *this* outcome establishes "structural", and only against nonzero bases |
| anything else | read it; the arithmetic will name its own base |

*Cost:* three register writes inside a rung that already exists; one metal boot; no new knob if it
rides `UNAOS_V3D_UNARMCLOSE` as a variant (e.g. `=bases`). *Discriminating power:* **maximal** — it
is the single cut that separates the two live hypotheses, and it is the cut §49.12.1 designed for
and could not take because it needed a residue it did not have. *Risk:* none beyond an experiment
boot; the rung already fails closed on `arena_contains`.

**R2 — `mapzero`: map the low V3D VA page, re-run, and read the payload. RECOMMENDED SECOND, and
first if R1 returns `0x70` unmoved.**

Add one PTE to the V3D page table mapping the page containing `VA 0x70` to a dedicated, poisoned
scratch PA outside the arena, then run the same close. The refusal becomes a completed write and
the write becomes **readable**.

*What it buys:* V3 established that ~48 words of zeros landed *somewhere*, but the catcher redirect
discards the offset (first-dirty `off=0x0`, not `0x70`). A real mapping preserves the offset and the
ordering, so the rung reads *what the PTB writes at frame close* — the first time this campaign
would see the close-time payload rather than its absence. That is the deepest single information
gain available, and it is what turns "the write is refused" into "here is the structure the write
was building".

*Cost:* one PTE, one scratch page, one poison/scan pass — all mechanisms this file already has
(`[v3d62]` mmufix builds an unmapped page; this builds a mapped one). One boot; can share a boot
with R1 as a second leg, since the rung returns immediately. *Discriminating power:* high but
**not** on the aim question — it explains the payload, not the address. *Risk:* it deliberately
grants the GPU a page it was previously denied; the page is outside the arena and outside every
region any other rung reads, and the `[v3d56]` landing digest already proves the arena unchanged, so
the blast radius is one page. Do **not** compose it with R1's legs in a way that leaves the mapping
in place for the rest of the boot.

**R3 — `lenvary`: the same unarmed close with a different list length. CHEAP CONTROL, ride-along.**

Both boots that produced `VA 0x70` submitted a **14-byte** list, and `VIO_ADDR`'s raw value is
`0xe` — the same 14. That is very likely a coincidence (the shift-3 decode is `DEBUG_INFO`-derived
and banked), but it is an uncontrolled one, and this file's own standard is that a number appearing
twice in two roles gets controlled rather than argued about. Re-run the close with a list of a
different length (pad `emptyunarm` with a second `NUMBER_OF_LAYERS` or an extra `FLUSH_VCD_CACHE`,
keeping the packet set otherwise identical) and read `VIO_ADDR`.

*Cost:* one extra leg on the same boot; no new machinery. *Discriminating power:* narrow but
absolute — if `VIO_ADDR` tracks the list length, the entire `VA 0x70` reading is an instrument
artifact and R1/R2 must be re-planned before anything is concluded from them. It costs almost
nothing to be certain, and it is expensive to be wrong here.

**R4 — the bunker (`start4.elf`) analysis. THIRD, and materially DE-PRIORITISED by this boot.**

§49.19.5(B) placed the campaign's remaining probability mass here, on the reasoning that if the
difference is invisible in every ARM-readable register at the instant it matters, it must live in
what the VPU firmware does to the block. V7 weakens that premise substantially, and V2–V5 replace
it: there is now a **specific, ARM-visible, reproducible** defect — a PTB write aimed at an unmapped
address — that no firmware theory is needed to explain and that R1 can localise in one boot. The
bunker line stays open, and §49.20.7 says what it should look for, but it should not be briefed
before R1 returns.

**Recommended order: R1 + R3 as two legs of one boot; R2 as the third leg or the next boot,
depending on R1; R4 after R1 reports.**

The composition matters and is stated explicitly. Run R3's leg **first** (it validates the
instrument), then R1's leg (it answers the question), then R2's leg if the boot budget allows and
the mapping is torn down before the boot continues. Each leg re-arms `[v3d62]`, re-seeds the
catcher, clears the fault latch, and reads the violation pair once and first. Three legs, one kick
each, one boot — the same shape §49.12.1 already validated, and the same "short capture, never
diffed against a deep boot" label applies.

#### 49.20.7 What the bunker brief should look for, concretely

The bunker analysis has been named in this file since §46.3 without ever being given a target list.
V7 lets one be written, and it also tells the brief what it will probably *not* find, which is worth
as much.

**The framing fact the brief must start from.** At cold handoff on PA40, after firmware init and
after our own ACKed `SET_DOMAIN_STATE(10, ON)`, `PM_GRAFX` reads `0x00001000` — `V3DRSTN = 0` — and
both `RPIVID` ASB words read `0x7` (stopped/ACKed), and the hub reads `0xdeadbeef`. §46.2's
`P89-KMSCOND` row recorded the `0x7` half a boot-generation earlier and concluded *"start4.elf does
not establish the condition"*. Today adds the reset bit, and the two together say something
stronger: **there is no firmware-established V3D register state to discover, because the block is
held in reset at handoff and reset destroys it.** A hunt for "what start4.elf programs into V3D and
leaves there" is therefore looking for something that provably cannot survive to our first read.

**What the brief should look for instead, in priority order:**

1. **Any store into the V3D MMIO aperture at all.** Disassemble for writes to the `0x7EC0_0000` /
   `0xFEC0_0000` V3D window (hub `+0x0000..0x1240`, core0 `+0x4000..0x4A00`). The expected result is
   **none outside a reset/power path**, and that expected result is the *point*: a clean negative
   converts §49.19.5(B)'s "the probability mass sits in the firmware" into a measured exclusion and
   closes the prong. A positive — any V3D register the firmware writes that we do not — is a
   first-class finding and should stop the analysis for immediate transcription.
2. **The `SET_DOMAIN_STATE(10, …)` handler.** We now have a hard behavioural datum for it to
   explain: an ACKed domain-ON that leaves `V3DRSTN = 0` and both bridges at `0x7`. Determine
   whether the handler is a no-op when the domain is already marked on, whether it touches
   `PM_GRAFX` bit 6 at all, and whether an OFF→ON pair takes a different path. §49.14's boot20 ran
   that pair on metal and read `0x4` at all four stations, so the disassembly is being asked to
   *explain a known result*, not to generate a hypothesis.
3. **The `SET_CLOCK_RATE(5)` / gate path, against the `CM +0x028` asymmetry.** §49.18.3 and V6 now
   have this word at `0x2D4` on our side and `0x244` on piOS's, stable across three captures and two
   piOS kernels. It is the only word in the whole comparison pointing at something *we* (or the
   firmware on our behalf) enable that piOS does not. The brief should identify which clock slot
   `CM +0x028` is and which mailbox path sets its `ENAB` bit. Cheap, bounded, and it retires an
   asymmetry that has now survived three folds unexplained.
4. **Whether the firmware ever establishes a V3D MMU page table.** `MMU_PT_PA_BASE` and `MMU_CTL`
   are unreadable before our reset on this boot (hub dark), and after it they are at POR by
   construction — so the question cannot be settled from the wire, and disassembly is the only
   route. It matters directly to the new wall: if the firmware maps a low aperture in the V3D VA
   space, `VA 0x70` may be an address that is *valid under the firmware's page table and invalid
   under ours*, which would explain the refusal without any PTB defect at all. **This is the bunker
   item with real discriminating power for the current wall, and the brief should lead with it if
   R1 returns `0x70` unmoved.**

**What the bunker brief should explicitly NOT re-open:** the mailbox family (§49.14 closed it with
four boots), `M_CTRL`'s upper bits (§46.2 and §49.20.4(a) — activity, not configuration), the KMS
overlay (§49.14 boot19), and `ENABLE_QPU` (§46.2 boots 17/19). Those are excluded measured, and a
disassembly that rediscovers them has spent its budget on the answer sheet.

---

### 49.21 `basedaim` — the base-aim cut, built (PI-V3D-97, design note; flies on the next V3D sitting)

§49.20.6 recommended "R1 + R3 as two legs of one boot". This section records that rung as built. It is
a **design note, not a verdict**: QEMU raspi4b models no V3D, so nothing below has been observed. The
gates it carries are compile, presence-in-artifact and knob-off byte-identity; the reading is the next
metal sitting's, and §49.21.4 states, verbatim, the wire line each of the four pre-written outcomes
produces so that sitting reads itself.

#### 49.21.1 What was built, and where it rides

The rung is **two extra legs on the `[v3d95]` scaffolding**, not a new rung — the `[v3d95]` kick becomes
leg A of three, and legs B and C follow it inside the same function before the boot returns. That
composition is the point: the boot4 reproduction and both variants come from **one image, one boot, one
CT0 that was virgin at leg A**, so every comparison below is within-capture and none of it rests on
diffing two boots.

| leg | list | `CT0QMA`/`CT0QMS`/`CT0QTS` | what it is |
|---|---|---|---|
| **A** | 14 B `emptyunarm` | untouched (bring-up residue; boot4 read zero) | the `[v3d95]` kick, **unchanged and byte-identical** — the in-boot anchor |
| **B** | 46 B (`emptyunarm` + 32 `FLUSH_VCD_CACHE`) | untouched | §49.20.6 **R3 `lenvary`** — the instrument control |
| **C** | 14 B `emptyunarm` | **programmed**, and nothing else is | §49.20.6 **R1 `basedaim`** — the cut |

Order is §49.20.6's own: *"Run R3's leg first (it validates the instrument), then R1's leg (it answers
the question)"*. Each leg re-arms the `[v3d62]` catcher, re-seeds it, clears the fault latch, re-poisons
every scanned region, and reads the violation pair once and first.

Knob: **`UNAOS_V3D_BASEDAIM=1`**, cargo feature `v3d_basedaim`, which **implies `v3d_unarmclose`** (the
Cargo.toml dependency and the `arroyo` block both arm it, so the operator cannot forget the half the
legs ride on). Default OFF and fully uncompiled when off. The family law holds structurally, as it does
for `[v3d95]`: the boot returns long before `[v3d75]`'s `ENABLE_QPU` and `[v3d80]`/`[v3d81d]`'s
`DISPLAY_DONE` sends, so this knob can never sit beside them.

#### 49.21.2 Leg C's three values, and why they are these

Program **only** the three registers, to values the V3D MMU **actually maps** — the arena is identity-
mapped in the V3D page table, so the whole arena qualifies — chosen so that no two arithmetic readings
of the fault VA can collide at `VIO_ADDR`'s 8-byte quantum:

| register | value | offset in the arena |
|---|---|---|
| `CT0QMA` | `arena + 0x4000` | the free 16 KiB gap between the clear target and `OFF_RCL` |
| `CT0QMS` | `0x3000` | `V3D56_EXPECTED_EMPTY_BPCA_ADVANCE` — §49.18.4 item 2's one-tile reservation |
| `CT0QTS` | `arena + 0x3A000` | the top of the arena, `0x36000` clear of the pool window |

`CT0QTS` is written as the **bare base**: `ENABLE` (bit 1) is 0, because §49.20.6 R1 says *"no
`CT0QTS.ENABLE` beyond what the base word carries"* and a page-aligned base carries none. A `QTS + 0x70`
hit under `ENABLE = 0` would therefore be a finding in its own right, and the wire says so.

Both targets are **decoys**, deliberately not the real pool (`OFF_BIN_TILEALLOC`) and not the real
tile-state (`OFF_TILESTATE`). Two reasons. First, the real regions stay pristine as leg A's negative
controls, so V4's shape is preserved across all three legs. Second, a decoy separates *"the aim derives
from `CT0QMA`"* from *"the aim happens to be where the pool already is"* — with the armed values those
two are the same number. Both decoy windows are poisoned with the `[v3d68]` index-encoding sentinel and
scanned after the close, which is what makes §49.20.6's **fourth** outcome — no fault, because the write
finally landed on a page the MMU maps — readable rather than merely inferred.

Compile-time assertions carry the geometry: both windows inside the arena, `[QMA, QMA+QMS)` inside the
poisoned pool window, `CT0QTS` 32-byte aligned, and the tile-state candidate a full page clear of the
pool window's far end.

#### 49.21.3 Leg B's control, and the self-check it needs

The 46-byte list is `emptyunarm` with 32 extra 1-byte `FLUSH_VCD_CACHE` packets spliced into the
prologue after the one the shape already carries — §49.20.6's suggested pad, placed where a VCD flush is
legal and inert. 14 → 46 is `0xe` → `0x2e` raw, and `0x70` → `0x170` decoded: neither can be confused
with the other, nor with any of leg C's candidates.

The leg would be worthless if the padded encoder differed from the `emptyunarm` encoder in any way other
than length, because then it would vary two things. So a **run-time self-check** runs first: it builds
`BinContent::Empty` into the list address, copies the bytes, rebuilds with the padded encoder at
`pad = 0`, and compares. The result is on the wire, and a failure **stands the whole rung down** — no
kick is issued by either leg:

```
:: V3D: [v3d97] pad-selfcheck — build_bin_cl_content_geom(Empty) len=14 vs
   build_bin_cl_emptyunarm_padded(pad=0) len=14 first_diff=[-1] identical=1 — the R3 encoder
   reproduces the `emptyunarm` shape BYTE FOR BYTE at zero pad, so the padded leg varies EXACTLY ONE
   thing: list length. R3's control value stands ::
```

#### 49.21.4 The wire, pre-written — what the sitting reads

Every leg prints a `VIOLATION` line (the V2a read) and a `VERDICT` line. The `VIOLATION` line is the one
that carries the evidence, because it is emitted from the **first and only** read of the pair:

```
:: V3D: [v3d97] LEG <B|C> VIOLATION (read ONCE, FIRST — §49.20.2 V2a; every later read of this pair on
   this leg is VOID) — VIO_ADDR=0x……… VIO_ID=0x……… RAW, decoded client <C> @ VA 0x……… |
   MMU_CTL=0x……… fault=<0|1> (PT_INVALID=… WRITE_VIOLATION=… CAP_EXCEEDED=…) ::
```

**Leg B — read this one FIRST. If R3 fires, leg C is void whatever it says.**

| `VIO_ADDR` reads | the leg's verdict text begins | what it means |
|---|---|---|
| `0x0000002e` | `R3 FIRES: VIO_ADDR TRACKS THE LIST LENGTH` | the raw word is the byte count, not an address. §49.20 V2's *"client PTB @ VA 0x70"*, §49.20.5's wall statement and leg C are **all void**. Stop; re-plan R1/R2 against a corrected decode |
| `0x0000000e` | `R3 CLEARS THE INSTRUMENT` | the word did not move with a list that did. The 14/`0xe` coincidence is retired and §49.20's reading survives its own control |
| no fault at all | `NO FAULT ON THE PADDED LEG AT ALL` | neither confirms nor refutes; read the poison scans and the `[v3d62]` catcher on this leg before reading leg C |
| anything else | `VIO_ADDR IS NEITHER 0xe NOR THIS LEG'S LENGTH` | a third variable moved it; name it before reading leg C |

**Leg C — §49.20.6's four rows, with the arithmetic printed on the same line so no reader has to do it.**
Write `A` for the arena base (`[v3d55] pte` publishes it; on boot4 it was `0x009be000`).

| `VA` reads | the leg's verdict text begins | conclusion |
|---|---|---|
| `A + 0x4070` | `OUTCOME 1 — QMA + 0x70` | the close-time write is the **pool** write and it is base-relative. "Structural `0x70`" is refuted; the wall becomes *why did the MMU refuse a mapped address* |
| `A + 0x7070` | `OUTCOME 1b — QMA + QMS + 0x70` | pool-**END**-relative — the same formula V5 saw from the other side (`BPCA + BPCS = QMA + QMS`). Also refutes "structural" |
| `A + 0x3A070` | `OUTCOME 2 — QTS + 0x70` | the **tile-state** write, and it fired with `ENABLE = 0`. §49.18.4 item 4's placement divergence goes live; the next cut moves the tile-state to piOS's placement |
| `0x00000070` | `OUTCOME 3 — 0x70, UNMOVED` | the aim is **genuinely absolute**; "structural" is finally earned. Licenses **R2 `mapzero`**, and promotes §49.20.7 item 4 (does the firmware map a low aperture?) to the head of the bunker brief |
| no fault | `OUTCOME 4, THE INTERESTING ONE` | the write **landed**. The four `[v3d56]` scan lines say where: decoy pool ⇒ pool write off `CT0QMA`; decoy tile-state ⇒ tile-state write off `CT0QTS`; a real region ⇒ the aim is not derived from these registers at all. First completed close-time write this campaign would ever read |
| anything else | `A FAULT LATCHED AT A VA NONE OF THE FOUR PRE-WRITTEN ROWS PREDICTS` | §49.20.6's own last row — do the subtraction against the three bases printed on the same line |

Two further lines are worth naming before the sitting. The header is corrected **at line two**, so no
reader ever holds the wrong shape for the capture (`[v3d95]`'s own header says "exactly one CT0 kick",
which is false on a `basedaim` boot):

```
:: V3D: [v3d97] CORRECTION TO THE LINE ABOVE — UNAOS_V3D_BASEDAIM=1 is armed, so this boot takes THREE
   CT0 kicks, not one. …
```

and leg C's `[v3d41]` PTB pointer line is now read against a **nonzero** pool base, so V5's two
invariants (`BPCA − QMA = 0x3000` and `BPCA + BPCS ≡ QMA + QMS`) become checkable a second time on a base
where they can fail. If they hold on `A + 0x4000` as they held on `0`, V5's "the PTB obeys `CT0QMA`"
finding is confirmed on a residue that could have refuted it.

#### 49.21.5 What this rung deliberately does NOT do

**R2 (`mapzero`) is not built.** §49.20.6 orders it second, and first only if R1 returns `0x70` unmoved.
It is the one candidate that grants the GPU a page it was previously denied, and building it beside R1
would have left the aim question and the payload question entangled in one boot. The page table is
untouched by this arc.

No new mailbox tag is sent (the reply-less-mailbox hazard law). Nothing outside `drivers/gpu` / the v3d
rung files, `arroyo` and `Cargo.toml` is edited. `CT0QMA`/`CT0QMS`/`CT0QTS` are **left programmed** at
leg C's values when the rung returns — nothing runs after it, and a restore would be one more register
write between the verdict and the wire; the closing line says so.

#### 49.21.6 Gates

`./arroyo check` and `UNAOS_WC=1 ./arroyo check` green (12 cfg legs; `v3d_basedaim` appended to the
`arm-pi` leg, so both polarities of every new `#[cfg]` are compiled). `./arroyo kernel8-test` green at
its banked standing on a quiet host. Presence in the builder-path artifact, per the full-knob law:

```
strings target/pi_baremetal/kernel8.img | grep -c 'v3d97] BASEDAIM BOOT'      → 1
strings target/pi_baremetal/kernel8.img | grep -c 'VIOLATION (read ONCE, FIRST' → 1
strings target/pi_baremetal/kernel8.img | grep -c 'OUTCOME 1' … 'OUTCOME 4'    → all present
```

Knob-off byte-identity is proven twice over, on `target/pi_baremetal/kernel8.img`: a plain build with
the arc applied hashes **identically to the pre-arc plain build**, and — the stronger statement — a
`UNAOS_V3D_UNARMCLOSE=1` build with the arc applied hashes **identically to a `UNAOS_V3D_UNARMCLOSE=1`
build with the arc reverse-applied**. The `[v3d95]` rung is therefore unchanged as a binary, not merely
as source, on every boot that does not arm `UNAOS_V3D_BASEDAIM`.

#### 49.21.7 What the sitting needs

One image, built with:

```
UNAOS_V3D=1 UNAOS_V3D_UNARMCLOSE=1 UNAOS_V3D_BASEDAIM=1 UNAOS_PI=1 ./arroyo kernel8
```

(`UNAOS_V3D_BASEDAIM=1` alone would suffice — the feature implies the other two — but the full line is
what the capture should record.) A **cold power-cycle**, as boot4 was: §49.20.2 V6/V7 both turn on the
entry state, and a warm re-boot puts a different block in front of leg A. One capture, short by design,
labelled and **never** diffed line-for-line against a deep boot. Read in this order: the
`[v3d97] pad-selfcheck` line, then leg B's `VIOLATION` and `VERDICT`, then — only if R3 did not fire —
leg C's.

---

### 49.22 `basedaim`, read on metal — the pool write is base-relative and it LANDS, and a second write remains (PI-V3D-97 verdict, boot7/PA45, 2026-08-18)

§49.21 built the rung and pre-wrote every outcome it expected. The rung flew. It answered its question,
and it answered it through an instrument the design note had listed as a corroborator rather than as the
decider: **the fault VA did not name the base — the landed bytes did.**

One correction is load-bearing and is stated before the evidence rather than after it. §49.21.4's leg-C
table was written on an assumption the boot falsifies: that an unarmed frame-close issues **one**
close-time write, so that one fault VA could be read as that write's aim. It issues **two**. Leg C
landed the first and had the second refused, and the two answer different halves of the question. Every
row of that table is therefore reached, or not reached, for reasons the table did not anticipate, and
§49.22.3 says so explicitly rather than selecting the row that fits.

#### 49.22.1 The capture, and what kind of boot it was

`~/unaos-bench/capture/pi4-pi0-b1/ttyACM0.log`, past the operator mark
`2026-08-18T04:21:23Z MARK R24 boot7 pi4 — PA45 v3d97 basedaim cut, hw-pi4@18988638, fp 1261776
(Loaded 0x1340d0)`. The slice is banked at `~/unaos-bench/scratch/pi0-b1b2/boot7-raw.txt` (767 lines),
with the V3D lines at `boot7-v3d-all.txt` and the verdict stations at `boot7-verdict-slice.txt`
(extraction: `tr -d '\000'` then `awk`, per `CLAUDE.md` — the log carries control bytes and `grep`
mis-reads it). The image identity `hw-pi4@18988638` is the operator's, from the mark; the wire does not
carry it.

It is a `UNAOS_V3D_TSAIM`-less `basedaim` boot: three CT0 kicks and nothing else, the shape §49.21.7
asked for, with the header corrected at line two exactly as designed:

```
:: V3D: [v3d97] CORRECTION TO THE LINE ABOVE — UNAOS_V3D_BASEDAIM=1 is armed, so this boot takes
   THREE CT0 kicks, not one. …
```

and closed at the other end by `[v3d97] BASEDAIM COMPLETE — three closes taken (A zero-base/14B,
B zero-base/46B, C mapped-bases/14B) and nothing else`. Per §49.12.1's standing warning, nothing below
may be diffed line-for-line against a deep boot. The arena on this boot is 64 pages at `0x002ba000`, so
leg C's programmed values decode as `CT0QMA = A + 0x4000`, `CT0QTS = A + 0x3A000` — §49.21.2's geometry,
confirmed on the wire.

The rung's own gate passed first:

```
:: V3D: [v3d97] pad-selfcheck — build_bin_cl_content_geom(Empty) len=14 vs
   build_bin_cl_emptyunarm_padded(pad=0) len=14 first_diff=[-1] identical=1 — the R3 encoder
   reproduces the `emptyunarm` shape BYTE FOR BYTE at zero pad …
```

All three legs audited `submission SOUND`, latched `FLDONE`, walked `CT0CA` from `BA` to `EA`, and closed
a bin frame: `BFC 0→1` (A), `1→2` (B), `2→3` (C), each Δ1. Every verdict below is entitled to assume the
CLE was handed the list the leg built.

#### 49.22.2 The verdicts, each with its wire line and each with its exclusion

**W1 — R3 CLEARS THE INSTRUMENT. `VIO_ADDR` is `0xe` again on a list that is not 14 bytes long.**

```
:: V3D: [v3d97] LEG B VIOLATION (read ONCE, FIRST — §49.20.2 V2a; every later read of this pair on
   this leg is VOID) — VIO_ADDR=0x0000000e VIO_ID=0x00000020 RAW, decoded client PTB @ VA 0x00000070
   | MMU_CTL=0x061d1c01 fault=1 (PT_INVALID=1 WRITE_VIOLATION=1 CAP_EXCEEDED=0) ::
:: V3D: [v3d54] submit (v3d97 R3 lenvary (zero bases, 46-byte list)) — intended BA=0x002ef000
   EA=0x002ef02e len=46 | latched CT0QBA=0x002ef000 CT0QEA=0x002ef02e span=46 — BA OK EA OK span OK
   — submission SOUND ::
```

*Excludes:* §49.21.4's leg-B row 1. The raw word did not move with a list that did, so it is not a byte
count; the `14`/`0xe` coincidence §49.20.2 V2 flagged is **retired**, the shift-3 decode survives its own
control, and leg C is entitled to run. This is the cheapest verdict in the section and it is the one that
licenses every other verdict in it.

**W2 — THE POOL WRITE LANDED, AT `CT0QMA`. This is the finding that moves the campaign.**

```
:: V3D: [v3d56] poison (v3d97 R1 basedaim (nonzero mapped bases, 14-byte list)) R1 decoy POOL window
   (CT0QMA target) iova=0x002be000 words=4096 — INTACT=4076 ZEROED=19 OVERWRITTEN=1 touched=20 |
   first_touched=[0] (got 0x00000012 expected 0xa5a5a5a5) last_touched=[31] | byte span [0x0,0x7f] |
   L2T write-back completed=1 — POISON OVERWRITTEN with non-zero data: the PTB emitted real bytes
   here. The pool is NOT empty and the phantom-BPCA verdict is RETRACTED for this region ::
:: V3D: [v3d56] landing (v3d97 R1 basedaim …) — arena 64 pages (0x40000 B @ 0x002ba000, the ENTIRE
   address space the V3D MMU grants this job) | changed=1 expected=0 STRAY=1 | … first_stray_page=4
   (off 0x4000) last_stray_page=4 ::
```

Two independent instruments agree and no third disagrees. The poison scan says the decoy pool took
bytes; the landing digest says **exactly one** arena page changed and it is page 4, offset `0x4000` —
which is `OFF_V3D97_POOL`, which is leg C's `CT0QMA`, to the page. The "stray" label is the digest's own
whitelist speaking (the decoy is not a region the digest knows a job may write); the stray page **is the
decoy**, and that is the intended result, not an anomaly.

The write's shape, stated exactly as measured and no further: **20 words moved inside the byte span
`[0x0,0x7f]`**, one of them to a non-zero value (`word[0] = 0x00000012`) and nineteen to zero, with
**twelve of the thirty-two words in that span left holding their poison**. So it is a ~`0x80`-byte,
sparsely populated header — not a `memset`, and not a primitive list.

*Excludes:* "the close-time write is aimed at an absolute address." §49.21.4's OUTCOME 3 is refuted for
this write, and with it §49.12.1's "the aim is structural" branch, which §49.20.3 recorded as *NOT
established* pending exactly this cut. *Also excludes:* every reading in which the unarmed close's write
cannot reach memory at all.

**W3 — the campaign's `VA 0x70` refusals are W2 on a base of zero.**

The span W2 landed in is `[QMA + 0x0, QMA + 0x80)`. On the boots that produced `VA 0x70` — boot11
(§49.10), boot4 (§49.20), and legs A and B of this boot — `CT0QMA` was **zero**, so that span was
`[0x0, 0x80)` absolute, and `0x70` lies **inside it**.

What is earned and what is not, kept apart: *earned* — `0x70` is an address the same write reaches for
under a zero base, so the refusals are accounted for without any second mechanism. *Not earned* — that
`0x70` is the write's *tail*. The MMU reports **one** address per latch window; whether it latches the
first refused beat, the last, or an arbitrary one is not established by anything in this file, and
`last_touched=[31]` is byte `0x7c`, which sits in the `0x78` beat rather than the `0x70` one. Any future
argument that turns on *which* beat `0x70` is must first establish the latch's selection rule.

**W4 — A SECOND WRITE. Client PTB, `VA 0x0`, and no row of §49.21.4's table predicts it.**

```
:: V3D: [v3d97] LEG C VIOLATION (read ONCE, FIRST — §49.20.2 V2a; every later read of this pair on
   this leg is VOID) — VIO_ADDR=0x00000000 VIO_ID=0x00000020 RAW, decoded client PTB @ VA 0x00000000
   | MMU_CTL=0x061d1c01 fault=1 (PT_INVALID=1 WRITE_VIOLATION=1 CAP_EXCEEDED=0) ::
```

with the arithmetic printed on the leg's own verdict line so no reader has to do it:
`QMA+0x70=0x002be070 QMA+QMS+0x70=0x002c1070 QTS+0x70=0x002f4070 absolute=0x00000070`. `VA 0x0` is none
of the four, and it is not even the `0x70` the absolute row predicts.

**The inference that makes this a two-write section, and it is drawn within leg C alone, with no
cross-boot comparison:** the pool write **completed** on this leg (W2), so the write the MMU **refused**
on this leg is a different write. Before boot7 the campaign had one close-time write and an
undetermined aim; it now has two writes, one of them located and landed.

*Excludes:* §49.21.4's leg-C rows 1, 1b, 2 and 3 as readings of the *fault*, and the table's implicit
premise of a single write. *Does not exclude:* any of the three identifications §49.22.3 ranks.

**W5 — the `[v3d62]` catcher's 48 words belong to the second write, and the catcher must be retired as
a counting instrument.**

The line is byte-identical on all three legs:

```
:: V3D: [v3d62] fault (…) — MMU_CTL=0x061d1c01 fault-latched=0x00101000 (PT_INVALID=1
   WRITE_VIOLATION=1 CAP_EXCEEDED=0) … | scratch page 0x2a8000: dirty words=48/1024 first-dirty
   off=0x0 val=0x00000000 ::
```

Two readings follow. *Positive:* on leg C the pool write completed into mapped memory, so the 48 words
the catcher absorbed on leg C are **not** it — they are the second write, whose size is therefore ~48
words and whose payload is zeros. *Negative, and it costs the file an instrument:* the count is
**identical across a leg where two writes were refused and a leg where one was**, so it carries no
information about how many writes a frame refused. §49.20.2 V3 already forbade reading it as a span;
this forbids reading it as a census. It remains a binary witness — "a refused write landed here" — and
nothing more.

**W6 — the tile-state write did NOT fire on any leg, and the decoy is what proves it.**

```
:: V3D: [v3d56] poison (v3d97 R1 basedaim …) R1 decoy TILE-STATE page (CT0QTS target)
   iova=0x002f4000 words=1024 — INTACT=1024 ZEROED=0 OVERWRITTEN=0 touched=0 … ::
```

Leg C wrote `CT0QTS` as a bare page-aligned base, so `ENABLE` (bit 1) is clear — the wire's `(ENABLE=0)`
column says so on every station — and nothing landed there. §49.21.2 pre-committed that a `QTS`-relative
hit under `ENABLE = 0` would be *"a finding in its own right"*; there was none, and the absence is
consistent with the bit's documented job.

The negative controls held on all three legs: the **real** tile-state (`0x002cb000`, 64 words) and the
**real** pool (`0x002cc000`, 8192 words) read `INTACT` throughout, so §49.20.2 V4's shape is preserved
and the decoys did their separating work — nothing here rests on "the aim happens to be where the pool
already is".

**W7 — §49.20.2 V5's two invariants CONFIRMED on a base that could have refuted them, and now with
landed bytes under them.**

```
:: V3D: [v3d41] v3d97 R1 basedaim … CLE feed + PTB pointer — CT0LC=0x00030000 CT0PC=0x00000000
   PCS=0x00000000 (raw) | BPCA 0x00003000->0x002c1000 (pool base 0x002be000) BPCS=0x00000000
   BPOA=0x00000000 BPOS=0x00000000 — PTB write pointer ADVANCED off the pool base across THIS kick ::
```

1. **`BPCA − QMA = 0x002c1000 − 0x002be000 = 0x3000`** — `V3D56_EXPECTED_EMPTY_BPCA_ADVANCE`, exact, the
   same value V5 measured on a zero base.
2. **`BPCA + BPCS = 0x002c1000 + 0 = 0x002c1000 = QMA + QMS`** — exact, and this time without needing the
   modulo-2³² rescue a zero base required.

V5 read those invariants off a base where they could not fail interestingly. They now hold on
`A + 0x4000`, and W2 supplies what V5 could not: the arithmetic is accompanied by **bytes**. "The PTB
obeys `CT0QMA`/`CT0QMS`" is no longer an inference from a pointer.

Two consequences are worth naming here because §49.22.5 turns on them.

- **`BPCS = 0` exactly.** §49.21.2 chose `CT0QMS = 0x3000` because that is the one-tile reservation, which
  means leg C handed the PTB a pool the frame consumes **to the byte**. At close, the pool is exhausted
  and the overspill pair is unarmed (`BPOA = BPOS = 0`, as on every boot this file has recorded — §33 T1,
  §49.1, §49.18.4). That is a live alternative account of W4 and it is a **leg-C-only** condition.
- **`BPOA`/`BPOS` read zero on all three legs**, confirming §49.18.4's row a fourth time. This driver arms
  them nowhere on a live path, and mainline arms them only from `v3d_overflow_mem_work` in response to
  `OUTOMEM`.

**W8 — leg A reproduced boot4, and leg B's `BPCA` did not move. One of those is an open item.**

Leg A's stations are byte-identical to §49.20's boot4: `BFC 0→1`, `VIO_ADDR=0x0000000e
VIO_ID=0x00000020`, `BPCA 0x00000000->0x00003000`, `BPCS=0xffffd000`. The in-boot anchor holds and every
within-capture comparison above is anchored on a leg that is known to reproduce.

Leg B did **not** advance `BPCA` — the instrument labels it `(stale)` under §48's frozen-register rule —
although it sat on the same zero bases as leg A and closed its own frame (`BFC 1→2`). **This is not
explained by this boot and is recorded as an open item, not smoothed.** Three candidates, none
preferred: the advance is a one-time allocation that is not re-taken while the bases are unchanged; the
register saturates against a pool of size zero after the first over-consumption; or the padded leg took
a path leg A did not. Nothing in W1 depends on it — leg B's verdict is about `VIO_ADDR` — and nothing
elsewhere in this section does either.

#### 49.22.3 The two-write model, and the armed-path reconciliation stated without smoothing

**The model, with the evidence class marked on every cell.**

| | **W-pool** | **W-second** |
|---|---|---|
| witnessed size | 20 words moved inside a `0x80`-byte span (W2) | ~48 words (W5, leg C only) |
| witnessed payload | `word[0] = 0x12`, nineteen zeros, twelve words untouched (W2) | zeros (W5) |
| witnessed base | `CT0QMA` — landed there, and only there (W2) | none of `CT0QMA`, `CT0QMA+CT0QMS`, `CT0QTS`; the fault VA is `0x0` (W4) |
| on a zero base | refused; `VA 0x70` lies inside its span (W3) | refused; not the latched VA on legs A/B |
| on leg C's bases | **LANDED** | refused, `VA 0x0` |
| client | PTB (W3, by inheritance from the boots it explains) | PTB, read directly (W4) |

**The three identifications for W-second, ranked, with what is for and against each.**

**(a) The TILE-STATE write, aimed from a base of zero because `CT0QTS.ENABLE` is clear.** *For:* 48 words
is exactly the count §49.8's R23 bracket measured touched in the 64-word tile-state array on boot6 — the
**armed** `empty` frame placed as the boot's first CT0 kick — and that same kick wrote **20** pool words.
Both of leg C's numbers are boot6's numbers, and boot6 is the only capture in this file where an empty
frame's two close-time writes are known to have landed together. *Against:* the driver's own constant
carries the comment that `ENABLE` *gates the tile-state write*, which predicts **no** write with the bit
clear; the observation is a write that fires anyway, which is compatible only with `ENABLE` gating the
write's **base** rather than its existence. *Also against, weakly:* the 48/48 match is cross-boot and
cross-instrument-generation, and boot6's payload split (`ZEROED` vs `OVERWRITTEN`) is not recorded in
§49.8's table, so only the counts match, not the contents. **R-next-1 decides this in one leg, and the
leg is built (§49.22.6).**

**(b) The OVERSPILL fetch.** *For:* leg C exhausts its pool to the byte (`BPCS = 0`, W7) with
`BPOA = BPOS = 0`, so a PTB that needs one more block has a zero overspill base to reach for, and `VA 0x0`
is what reaching for it looks like. *Against, and it is substantial:* §33's **T1** established from
mainline source that `v3d_bin_job_run` writes `BPOS = 0` as its first register write of **every** bin
job, so an unarmed overspill cannot by itself be a defect; V3D-44 pre-armed `BPOA = 0x001ba000
BPOS = 0x2000` on this hardware and the frame still never retired, with `OUTOMEM = 0`; and leg A ran with
`CT0QMS = 0`, a pool over-consumed from byte zero, and latched `0x70` rather than `0x0`. **R-next-1b
decides it, second.**

**(c) A THIRD PTB pointer with a structurally zero base.** Not excluded by anything measured. If both (a)
and (b) fail, this is what remains, and **R2 `mapzero`** is the instrument that reads its payload —
unchanged in rank from §49.20.6.

**The loose end, stated rather than buried.** On legs A and B *both* writes are refused, so the catcher
should have absorbed both, and the first dirty word of its region should then be W-pool's `0x12`. It
reads `0x00000000` on every leg. Three readings fit and this boot separates none of them: the redirect
coalesces both writes onto the same catcher offsets with the zero payload landing last; the redirect
discards the payload entirely (§49.20.2 V3 already established it does not preserve the low offset, and a
redirect that drops the offset may drop the data); or W-pool does not reach the catcher at all. **Nothing
in §49.22 rests on this**, and it has a cheap test: a leg with `CT0QMA = 0` (W-pool refused) and `CT0QTS`
mapped *and* enabled (W-second landed) should show `0x12` as the catcher's first dirty word if and only
if the redirect preserves payload.

**The armed-path reconciliation — the question this section is required to answer honestly.**

The ARMED production path programs all three registers (§49.18.2, and the code: `CT0QMA = arena +
OFF_BIN_TILEALLOC`, `CT0QMS = BIN_TILEALLOC_BYTES`, `CT0QTS = arena + OFF_TILESTATE | ENABLE`), and this
campaign walled on it for months. If mapped bases are what make the pool write land, why did the armed
path ever wall? The file's record of the armed values is on boot7's own wire:

```
:: V3D: [v3d95] tile-memory PRE (virgin …) — CT0QMA=0x00000000 CT0QMS=0x00000000 CT0QTS=0x00000000
   (ENABLE=0) | for reference the values an ARMED bin kick would have written: QMA=0x002cc000
   QMS=0x00008000 QTS=0x002cb002 — NOT written by this rung, by design ::
```

| register | armed | leg C | consequence |
|---|---|---|---|
| `CT0QMA` | `0x002cc000` (the real pool) | `0x002be000` (a decoy) | none — both are arena-mapped |
| `CT0QMS` | `0x00008000` | `0x00003000` | armed leaves `BPCS = 0x5000` at close; leg C leaves `BPCS = 0`. **Pool exhaustion is a leg-C-only condition** |
| `CT0QTS` | `0x002cb002` — **`ENABLE = 1`** | `0x002f4000` — `ENABLE = 0` | **the tile-state gate is set on the armed path and clear on leg C** |

Both mechanisms §49.22.3 offers for W-second are therefore **absent on the armed path**: it does not
exhaust its pool, and it does not leave the tile-state gate clear.

And the file holds a positive armed measurement saying the armed *empty* frame does not wall at all —
§49.8's R23 bracket, boot6, `empty`, placed as the boot's **first** CT0 kick with the full arming:
*`BFC` Δ1, "48 of 64 touched / 20 of 8192 touched", "The empty frame RETIRED as kick #1"*. That kick
closed its frame and landed **both** writes, and §49.8 already retracted *"an empty bin frame does not
retire"* on it.

**The verdict, and it is not the comfortable one.** The question as posed — *is the armed wall (a) the
`VA 0`-second write refused, (b) something else, or (c) unknown?* — needs a correction before it can be
answered, and then the answer is **(c), unknown pending the next rung, with (a) affirmatively
disfavoured.**

*The correction:* **there is no single "armed wall."** §49.8 measured three armed list classes as first
kicks and found three different stations — `empty` closed and landed both writes (**no wall**); `m4`
consumed its 76-byte list, **opened** a frame and never closed it, with poison `INTACT 64/64 and
8192/8192` (**no write attempted**, the opposite signature to leg C's); `rcl` held `CT0QBA` for 500
samples and **never fetched** (§49.9–§49.11's subject). Treating those as one object is what made "the
armed wall" seem like a thing this section could adjudicate.

*(a) is disfavoured on three counts:* the armed path never reaches `BPCS = 0`; it sets the tile-state gate
that leg C left clear; and its empty frame has been measured landing both writes.

*(c) rather than (b), and this is the reservation the section refuses to smooth:* **no armed close has
ever been read with the instruments that produced this section.** boot8's `INTACT 64/64 and 8192/8192`
was taken on 2026-08-02 — before the `[v3d62]` illegal-address catcher, before the V2a violation-pair
law, before the decoy-window poison discipline. §49.20.2 V3 is precisely the demonstration that a refused
write can leave every poison scan intact and still be happening; so "the armed `m4` path attempted no
write" is a reading the current instruments could overturn, and boot6's "no wall" verdict rests on the
same 2026-08-02 instrument set. Until an armed close is read under catcher + V2a + decoy poison, **the
armed wall is not characterised and this file must not claim it is.** That is **R-next-3**, and this
paragraph is the whole reason it exists.

#### 49.22.4 What these verdicts retire, and the corrections appended elsewhere

| standing item | status after boot7 |
|---|---|
| §49.21.4 leg-B row 1 ("R3 fires; leg C void") | **excluded measured** (W1) |
| §49.12.1's "the aim is structural" branch, left *NOT established* by §49.20.3 | **REFUTED for the pool write** (W2) |
| §49.20.5's wall statement | **SUPERSEDED** — see §49.22.5 |
| §49.20.2 V4 ("nothing GPU-reachable was written") | **stands as measured**, re-read as a consequence of the **zero base**, not of closing unarmed |
| §49.20.2 V5 ("the PTB obeys `CT0QMA`/`CT0QMS`") | **CONFIRMED** on a nonzero base, and now under landed bytes (W7) |
| §30's "there were never any bytes" reading | **RETIRED** — see correction 1 below |
| §49.21.4 leg-C rows 1 / 1b / 2 / 3 | **none fired**; the table's last row is where the fault landed, and the *landing* instrument answered what the VA was meant to answer |
| the `[v3d62]` catcher as a count of refused writes | **RETIRED as an instrument** (W5); it remains a binary "a refused write landed here" |
| "the armed wall" as a single object | **RETIRED as a framing** — §49.8 measured three, at three stations |
| whether an armed close writes | **RE-OPENED** — never read under the current instruments (§49.22.3) |

Four corrections are **appended** at their sites. None of the original text is rewritten; each stands as
the record of what the arc believed when it was written.

**1. §29/§30's phantom-`BPCA` lineage.** §30 retracted the phantom-write-pointer verdict on the
reasoning that `BPCA` reports *reserved* space over a provably untouched pool, so *"there would be no
phantom bytes because there were never any bytes."* The **register semantics stand and are strengthened**:
`BPCA` is an allocation pointer, and W7 confirms its formula twice more. What does not stand is the
second clause. There **are** bytes — a ~`0x80`-byte header at the pool base — whenever the frame closes on
a base the MMU accepts. §30's boots did not see them because the frames it rested on ran **behind
`probe_job`'s wedged frame**, which §49.3 warned about and §49.8 banked (*"Every reading in this file that
rested on 'the empty frame is the minimal case and it does not retire' was measured behind `probe_job`'s
wedged frame"*). §49.8's boot6 and this boot's leg C are the two captures taken outside that wedge, and
both show the write. The corrected statement: **a reservation advance over an untouched pool is
architectural only where the write was refused or the frame never really closed; where the base is
accepted and the frame closes, the advance and the write happen together.**

**2. §49.20.5's wall statement.** Superseded by §49.22.5 — its plural ("the PTB's close-time writes")
was doing unearned work, and its question is answered for one of the two writes and open for the other.

**3. §49.10's `emptyunarm` outcome-A reading, and boot11's asterisk.** The measurement is unchanged and
was correctly taken. Its `VA 0x70` is now accounted for: it is W-pool reaching into `[0x0, 0x80)` on a
zero base (W3). Nothing in §49.10's four-outcome composition falls; the row simply acquires a mechanism.

**4. §49.12.1's outcome-table row *"fault latched, client PTB"* — DISCHARGED, and by a road it did not
anticipate.** That row instructed: *read the VA against the residue; matching ⇒ the close path follows
those registers, a fixed `0x70` under a different residue ⇒ the aim is structural.* Under a different
residue the fault VA did **not** track the base — it moved to `0x0` — while the **landed bytes** tracked it
exactly. Read literally, the instruction points at "structural"; read correctly, "structural" is refuted.
The fork was sound and its premise was not: it assumed **one** close-time write. **The methodological
correction, which is the most portable thing in this section: where a unit issues more than one write per
event, a single fault latch is an instrument that samples one of them, and the poison/landing pair — which
sees all of them — outranks it.** Every future rung in this campaign should be designed on that ordering.

#### 49.22.5 The wall statement, and the next rung — candidates, cost, discriminating power, order

**The wall statement.** Before this boot (§49.20.5): *"why are the PTB's close-time writes aimed at
addresses the V3D MMU does not map?"* After it, split in two:

- the **pool header write** is aimed at `CT0QMA`, lands when `CT0QMA` is mapped, and needs no further
  explanation. For this write the question is **answered and retired**.
- the **second write** — ~48 words, zeros, client PTB — is aimed at `VA 0x0` on a leg where every base
  this rung can program is mapped, distinct and honoured. It is **unidentified**.

> **The new wall: what is the PTB's second close-time write, and what is its base?**

It is the best wall this campaign has had. It is *local* — one address, one client, one instant. It is
*instrumented* — the landing digest and four poison windows see the writes that succeed, the catcher
witnesses the one that does not, and V2a names client and VA. It is *narrow* — three named candidates,
of which the first is decided by **one bit**. And the surrounding frame is now nearly healthy: on leg C
an unarmed close with mapped bases closed its bin frame, latched `FLDONE`, walked its whole list, ran
its pool arithmetic exactly and landed its pool header. **Apart from that one refused write, leg C is
what a correct empty bin frame looks like.** The file should say that plainly, because it has not been
able to say anything like it before.

**R-next-1 — `tsaim`: set `CT0QTS.ENABLE` and re-read the VA. RECOMMENDED FIRST. BUILT (§49.22.6).**

Leg C's `CT0QTS` and the armed path's differ in exactly one bit. Set it, change nothing else, and read
whether W-second's refusal moves, lands or vanishes. *Cost:* one more kick on a rung that already exists;
no new geometry, no new window, no new register family — leg D reuses leg C's decoy windows and every one
of its compile-time assertions. *Discriminating power:* **maximal** — it is the single cut that tests the
leading identification of W-second, and its success case (`OUTCOME D1`) would be the first **complete**
unarmed close this campaign has ever read, both writes landed. *Risk:* none beyond an experiment boot;
the leg fails closed on `arena_contains` exactly as legs B and C do.

**R-next-1b — `poolbig` / `spillaim`: break the pool-exhaustion condition. SECOND, and first if leg D
returns `OUTCOME D2`.**

Two forms, and the cheaper one should be taken first. *`poolbig`* raises `CT0QMS` above the `0x3000` the
frame consumes so `BPCS ≠ 0` at close, varies exactly one register, and writes nothing mainline does not.
*`spillaim`* programs `BPOA`/`BPOS` to a mapped, poisoned overspill window and watches whether `VA 0x0`
moves to `BPOA` — a direct aim test, on the model of leg C. **Neither is built, and the reason is stated
so the next arc does not rediscover it:** `V3D97_POOL_BYTES` is `0x4000`, the free gap between the clear
target and `OFF_RCL`, and its compile-time assertion `QMS + 0x1000 ≤ POOL_BYTES` means a larger `CT0QMS`
does not fit the poisoned decoy. `poolbig` therefore needs a relocated pool decoy and an arena audit to
find one — real work, and work that should not be done speculatively while R-next-1 can moot it.
`spillaim` needs no new geometry (`OFF_PROBE_BIN_OVERFLOW`, 8 KiB at arena `+0x36000`, pages 54–55, is
already reserved and whitelisted) but it does write a register mainline writes only from the `OUTOMEM`
handler, and §19/§33 record that a stale nonzero `BPOS` leaked into later kicks the last time this
campaign armed it — a hazard this rung avoids only because it returns immediately. *Discriminating power:*
high for candidate (b), none for (a) or (c).

**R-next-2 — decode the landed header. CHEAP, DESK-SIDE, COSTS NO BOOT. Ride-along, take it with
R-next-1.**

`word[0] = 0x00000012` at `CT0QMA + 0`, nineteen zeros and twelve untouched words inside `[0x0, 0x7f]`.
That pattern is the first close-time payload this campaign has ever held, and the `[v3d56]` scan reports
only its endpoints. **The instrument gap is closed in this arc:** `[v3d98] pool-head` dumps the whole
`0x80`-byte window verbatim on every leg (the zero-base legs supply their own negative control — a window
that took no write must still read pure poison). The decode itself is desk work against the VC4/V3D
tile-list and pool-management formats and the Mesa material §49.12 banked; **no value is guessed in this
file**, and `0x12` is recorded as a raw word until a source corroborates a field. *Discriminating power:*
it does not bear on W-second's aim at all — it explains what W-pool is building — but it is the only item
on this list that costs nothing, and a named header format would tell the campaign what the *second*
write is supposed to be for.

**R-next-3 — `armedclose`: run the ARMED close under the honest instruments. THIRD, and the one
§49.22.3 obliges.**

Program `CT0QMA`/`CT0QMS`/`CT0QTS|ENABLE` at production values plus `BPOS = 0` and the pre-kick L2T
invalidate — the real `v3d_bin_job_run` order — inside the `[v3d95]`/`[v3d97]` scaffolding, with the
`[v3d62]` catcher armed, V2a carried, and the four poison windows scanned. *What it buys:* the armed
path's close has never been read this way, and §49.22.3's reconciliation is explicitly conditional on
that gap. Either it reproduces boot6 (both writes land, no fault — the armed empty close is healthy and
the campaign's remaining wall is entirely in the `m4`/`rcl` classes), or it shows a refusal that
2026-08-02's instruments could not have seen, which would be a first-class finding. *Cost:* one leg, one
boot, no new machinery — the registers are already programmed by existing helpers. *Discriminating
power:* **high, and it is the only candidate that touches the production path.** *Note:* it must be a
separate leg from A–D and must not leave the armed values standing in front of anything else.

**R2 — `mapzero`.** Unchanged in rank from §49.20.6 and still not built. It is the payload instrument for
W-second, and it becomes the head of the queue only if R-next-1 and R-next-1b both leave `VA 0x0`
standing — at which point candidate (c) is what remains and reading the payload is the only move left.

**Recommended order: R-next-1 as leg D on the next sitting, with R-next-2's dump riding it for free;
then R-next-3 on the sitting after, whatever leg D says, because the reconciliation needs it either way;
then R-next-1b if leg D returned `OUTCOME D2`; R2 last.** R4 (the bunker) stays where §49.20.6 put it —
de-prioritised — and is further weakened by this boot: W2 is a second ARM-visible, reproducible defect
resolved without any firmware theory, and §49.20.7 item 4 (does the firmware map a low aperture?) was
promoted only in the `0x70`-unmoved branch, which did not fire.

#### 49.22.6 Leg D (`tsaim`), built — the wire pre-written, and the gates

This subsection records R-next-1 as **built**, on the same terms §49.21 recorded `basedaim`: it is a
**design note, not a verdict**. QEMU raspi4b models no V3D, so nothing below has been observed; the gates
are compile, presence-in-artifact and knob-off byte-identity, and the reading belongs to the next metal
sitting.

**What was built.** One more leg on the `[v3d97]` scaffolding — the `[v3d95]` kick is now leg A of
**four** — placed after leg C so leg C remains the in-boot anchor and both are read within one capture,
off one CT0 that was virgin at leg A.

| leg | list | `CT0QMA`/`CT0QMS`/`CT0QTS` | what it is |
|---|---|---|---|
| A | 14 B `emptyunarm` | untouched (zero residue) | the `[v3d95]` kick, unchanged |
| B | 46 B | untouched | R3 `lenvary` — the instrument control |
| C | 14 B `emptyunarm` | programmed, `ENABLE = 0` | R1 `basedaim` — the base cut |
| **D** | 14 B `emptyunarm` | **leg C's three values, `CT0QTS \| ENABLE`** | **R-next-1 `tsaim` — one bit** |

Knob: **`UNAOS_V3D_TSAIM=1`**, cargo feature `v3d_tsaim`, which **implies `v3d_basedaim`** (and through
it `v3d_unarmclose` and `v3d`), declared in `Cargo.toml` and armed by the `arroyo` block so the operator
cannot arm half of it. Default OFF and fully uncompiled when off. The family law holds structurally, as
for `[v3d95]` and `[v3d97]`: the boot returns long before `[v3d75]`'s `ENABLE_QPU` and `[v3d80]`/
`[v3d81d]`'s `DISPLAY_DONE` sends, so this knob can never sit beside them.

**Leg D's four outcomes, pre-written verbatim on its own `VERDICT` line.** Leg D's rows take precedence
over §49.21.4's, which were written for a leg that leaves `ENABLE` clear and say so in their text; they
are selected on the `ENABLE` bit itself, so legs B and C are byte-for-byte unaffected in what they print.

| leg D reads | the verdict text begins | conclusion |
|---|---|---|
| **no fault, decoy tile-state touched** | `OUTCOME D1` | `CT0QTS.ENABLE` gates the tile-state write's **base**, not its existence. W-second is **identified**, §49.22.3(a) confirmed, and **both** close-time writes of an unarmed close are read as completed on one leg — a campaign first. Check the decoy tile-state count against §49.8 boot6's **48** and the decoy pool's against its **20** |
| **fault, `VA 0x0` unmoved** | `OUTCOME D2` | W-second does **not** derive its address from `CT0QTS`. §49.22.3(a) is **refuted** by the one experiment that could refute it; R-next-1b is licensed, and if it too leaves `VA 0x0` standing the aim is structural and R2 `mapzero` is the instrument |
| **fault, `VA` inside the tile-state decoy but not at its base** | `OUTCOME D3` | the base **is** honoured and the offset is not the one assumed. Subtract the base on the line, then read the offset against the decoy's byte span — an offset past the window's end is a **geometry** finding and a driver fix |
| **no fault, nothing touched anywhere** | `OUTCOME D4` | `ENABLE` **suppresses** the write rather than aiming it, inverting the reading of the bit. Check the `[v3d62]` catcher on the leg first: catcher words still absorbed would mean only the latch went quiet |
| no fault, something else touched | `OUTCOME D1b` | the bit is load-bearing but the bytes did not go where `CT0QTS` points; the four scan lines and the landing digest locate them |
| fault, client not PTB | — | attribution belongs to whichever unit `VIO_ID` names; no aim verdict |
| anything else | `OUTCOME D5` | §49.20.6's last row governs — do the subtraction against all three bases printed on the same line |

**What leg D deliberately does NOT do.** It writes no `BPOA`/`BPOS` (R-next-1b's job, and §19/§33's
stale-overflow hazard is why it is not casually armed). It edits no page table (still R2's). It sends no
mailbox tag. It changes no arena geometry — it reuses leg C's decoy windows and every compile-time
assertion §49.21.2 carries. It leaves `CT0QMA`/`CT0QMS`/`CT0QTS` programmed at its own values, `ENABLE`
included, for the reason leg C left its own: nothing runs after the rung, and a restore would be one more
register write between the verdict and the wire.

**Gates.**

`./arroyo check` and `UNAOS_WC=1 ./arroyo check` green for both arches (`v3d_tsaim` appended to the
`arm-pi` leg, so both polarities of every new `#[cfg]` are compiled). `./arroyo kernel8-test` green at
its banked standing on a quiet host. Presence in the builder-path artifact, per the full-knob law:

```
strings target/pi_baremetal/kernel8.img | grep -c 'v3d98] TSAIM LEG'   → 1
strings target/pi_baremetal/kernel8.img | grep -c 'OUTCOME D1'         → present
strings target/pi_baremetal/kernel8.img | grep -c 'OUTCOME D2'         → present
strings target/pi_baremetal/kernel8.img | grep -c 'v3d98] pool-head'   → present
```

Knob-off byte-identity on `target/pi_baremetal/kernel8.img`: a plain build with the arc applied hashes
identically to the pre-arc plain build. Note the scope honestly — a `UNAOS_V3D_BASEDAIM=1` build is **not**
claimed byte-identical across this arc, because leg D's outcome selector and the `pool-head` dump are
compiled into `v3d97_leg` itself; what is claimed, and gated, is that **every build that does not arm
`v3d_tsaim` is byte-identical**, which covers every boot that is not this experiment.

**Measured, this arc** (worktree `unaos-wt-exec-v3dfold2`, baseline `96cc0c0d`):

| gate | result |
|---|---|
| `./arroyo check` | green, both arches |
| `UNAOS_WC=1 ./arroyo check` | green, both arches |
| knob-off `./arroyo kernel8-test 210` | **MBENCH PASS 111/111**, 0 forbidden, 25 935 lines scanned |
| knob-off byte-identity, `UNAOS_PI=1 ./arroyo kernel8` | arc-applied `7d019ce7…` **==** baseline `7d019ce7…` (built in a throwaway worktree at `96cc0c0d`; no stash, per `CLAUDE.md`) |
| armed build banner | all four knobs echo: `UNAOS_V3D`, `UNAOS_V3D_UNARMCLOSE`, `UNAOS_V3D_BASEDAIM`, `UNAOS_V3D_TSAIM` |
| strings-proof, armed image `10c51755…` | `v3d98] TSAIM LEG` 1 · `OUTCOME D1` 2 (`D1` + `D1b`) · `D2`/`D3`/`D4`/`D5` 1 each · `v3d98] pool-head` 2 · `v3d98] CORRECTION` 1 |

**What the sitting needs.** One image:

```
UNAOS_V3D=1 UNAOS_V3D_UNARMCLOSE=1 UNAOS_V3D_BASEDAIM=1 UNAOS_V3D_TSAIM=1 UNAOS_PI=1 ./arroyo kernel8
```

(`UNAOS_V3D_TSAIM=1` alone suffices — the feature implies the rest — but the full line is what the capture
should record.) A **cold** power-cycle, as boot4 and boot7 were. One short capture, labelled, never diffed
line-for-line against a deep boot. Read in this order: `[v3d97] pad-selfcheck`, leg B's `VERDICT`, leg C's
`VERDICT` (leg D's whole question is leg C's residual, so a leg C that did not reproduce boot7's shape
leaves leg D with nothing to explain), then leg D's `VIOLATION` and `VERDICT`, then the four `[v3d98]
pool-head` blocks.

---

### 49.23 `tsaim`, read on metal — the unarmed close COMPLETES, and the second write is the tile-state write (PI-V3D-98 verdict, boot8/PA46, 2026-08-18)

Leg D returned `OUTCOME D1`. `CT0QTS.ENABLE` gates the tile-state write's **base**, not its existence:
with the bit clear the write fires at a base of zero and the MMU refuses it at `VA 0x0`; with the bit set
the same write lands at `CT0QTS`, no fault latches anywhere, and the illegal-address catcher — which
absorbed 48 words on each of the three legs before it — comes back **pristine**. Both close-time writes of
an unarmed frame-close have now been read as **completed** writes on one leg. That is a campaign first,
and it is what §49.22.5 pre-wrote as the success case of the single cut it recommended first.

This section folds that boot, executes §49.22.5's R-next-2 (the landed header, decoded from the four
`[v3d98] pool-head` dumps the leg carried for free), and records R-next-3 (`armedclose`) as built.

#### 49.23.1 The capture, and what kind of boot it was

`~/unaos-bench/capture/pi4-pi0-b1/ttyACM0.log`, past the mark

```
=== SQUAWK MARK 2026-08-18T05:09:20Z R24 boot8 pi4 — PA46 v3d98 tsaim one-bit cut, hw-pi4@d99bec68, fp 1268820 ===
```

625 lines, 162 of them `[v3d…]`. Banked sliced at `~/unaos-bench/scratch/pi0-b1b2/boot8-full.log` (the whole
boot), `boot8-v3d.log` (the V3D lines) and `boot8-key.log` (the 97 verdict/violation/scan/dump lines). A
short, deliberately shallow capture: the rung returns before `probe_job`, so there is no `[v3d48]` ladder,
no M4, no visible battery and no `[v3d75]`/`[v3d80]`/`[v3d81]` tail. **It must never be diffed
line-for-line against a deep boot.**

Four closes off one CT0 that was virgin at leg A, and the wire proves they are four and not one:
`BFC 0x0→0x1→0x2→0x3→0x4`, one Δ1 per leg, and `CT0LC 0x10000 / 0x20000 / 0x30000 / 0x40000` — the CLE's
own kick counter stepping once per leg. Every leg audited `submit_sound=1`, `idled=1`, `FLDONE` latched
in the raw status, and `CT0CA` walked `BA → EA`.

#### 49.23.2 The verdicts, each with its wire line and each with its exclusion

**W1 — the pad self-check held, on this boot, on its own terms.**

```
[v3d97] pad-selfcheck — build_bin_cl_content_geom(Empty) len=14 vs build_bin_cl_emptyunarm_padded(pad=0)
        len=14 first_diff=[-1] identical=1
```

*Excludes:* "leg B varies two things". R3's control value is established on this boot and is not inherited
from boot7. *Does not exclude:* anything about the legs themselves.

**W2 — leg B cleared the instrument a second time, on a second image.**

```
[v3d97] LEG B VERDICT — list len=46 (0x2e) | VIO_ADDR=0x0000000e VIO_ID=0x00000020 RAW -> client PTB @ VA 0x00000070
        poison touched: decoy-pool=0/4096 decoy-ts=0/1024 real-ts=0/64 real-pool=0/8192
```

`VIO_ADDR` is `0xe` again on a 46-byte list. The raw word does not track list length. §49.20's `VA 0x70`
reading now survives its own control **twice**, across two boots and two images. *Excludes:* the
instrument-artifact hypothesis, permanently. *Does not exclude:* any reading of what the VA points at.

**W3 — leg C reproduced boot7's leg C exactly, which is what entitles leg D to be read.**

```
[v3d56] poison (v3d97 R1 basedaim) R1 decoy POOL window (CT0QMA target) iova=0x0027b000 words=4096 —
        INTACT=4076 ZEROED=19 OVERWRITTEN=1 touched=20 | first_touched=[0] (got 0x00000012 expected 0xa5a5a5a5)
        last_touched=[31] | byte span [0x0,0x7f]
[v3d97] LEG C VERDICT — bases QMA=0x0027b000 QMS=0x00003000 QTS=0x002b1000 (ENABLE=0) |
        VIO_ADDR=0x00000000 VIO_ID=0x00000020 RAW -> client PTB @ VA 0x00000000 fault=1 |
        poison touched: decoy-pool=20/4096 decoy-ts=0/1024 real-ts=0/64 real-pool=0/8192
```

Same twenty words, same `word0 = 0x12`, same `[0x0,0x7f]` span, same residual PTB refusal at `VA 0x0`,
same `STRAY` landing on arena page 4 (`0x4000` = the pool decoy). §49.22 was not reading a one-boot
accident. *Excludes:* every "boot7 leg C was a fluke" reading. *Does not exclude:* the identification of
the residual — that is leg D's job.

**W4 — `OUTCOME D1`, verbatim from leg D's own `VERDICT` line.**

```
[v3d97] LEG D VERDICT (v3d98 R-next-1 tsaim (mapped bases, CT0QTS.ENABLE=1, 14-byte list)) — list len=14 (0xe)
        | frame-closed=1 (retired=1 FRDONE=0 BFC 0x00000003->0x00000004 Δ1) submit_sound=1 idled=1
        | bases QMA=0x0027b000 QMS=0x00003000 QTS=0x002b1002 (ENABLE=1) POST-moved=0
        | VIO_ADDR=0x00000000 VIO_ID=0x00000000 RAW -> client L2T @ VA 0x00000000 fault=0
        | poison touched: decoy-pool=20/4096 decoy-ts=48/1024 real-ts=0/64 real-pool=0/8192 (drain completed=1)
        — OUTCOME D1 — NO FAULT, AND THE TILE-STATE DECOY TOOK THE WRITE.
```

and the scan that carries it:

```
[v3d56] poison (v3d98 R-next-1 tsaim) R1 decoy TILE-STATE page (CT0QTS target) iova=0x002b1000 words=1024 —
        INTACT=976 ZEROED=45 OVERWRITTEN=3 touched=48 | first_touched=[0] (got 0x00000000 expected 0xa5a5a5a5)
        last_touched=[47] | byte span [0x0,0xbf] | L2T write-back completed=1
```

Forty-eight contiguous words at the tile-state base, three of them non-zero. §49.8 boot6's **48 of 64**.
And the pool decoy on the same leg reads `touched=20`, byte-identical in pattern to leg C's — §49.8
boot6's **20**. **Both banked armed numbers, reproduced by an unarmed close with three registers and one
bit programmed.** *Excludes:* `OUTCOME D2` (the identification is not refuted), `D3` (the base is honoured
at offset zero, not at some other offset), `D4` (`ENABLE` aims the write, it does not suppress it), and
`D1b`. *Does not exclude:* anything about the armed path — see §49.23.6.

**W5 — the catcher went from 48 to 0, and that is the conservation statement.**

```
[v3d62] fault (v3d95 unarmed close)   — … scratch page 0x265000: dirty words=48/1024
[v3d62] fault (v3d97 R3 lenvary)      — … scratch page 0x265000: dirty words=48/1024
[v3d62] fault (v3d97 R1 basedaim)     — … scratch page 0x265000: dirty words=48/1024
[v3d62] fault (v3d98 R-next-1 tsaim)  — MMU_CTL=0x060d0c01 fault-latched=0x00000000 (PT_INVALID=0
        WRITE_VIOLATION=0 CAP_EXCEEDED=0) … HUB_INT_STS=0x00000000 MMU-int bits=0x00000000
        … scratch page 0x265000: dirty words=0/1024
```

Three legs at 48, then one bit is set and the count is zero — while a window that never took a byte
before takes exactly 48. The words did not vanish; they moved from the catcher to `CT0QTS`. This is a
*quantitative* confirmation and it is stronger than the `OUTCOME D1` selector, which only tests
"fault == 0 && tile-state touched". *Excludes:* the reading that `ENABLE` merely silenced the latch —
the catcher is a memory witness, not a latch, and it is armed and re-seeded per leg. *Does not exclude:*
the possibility that some *third* refused write exists below the catcher's resolution; nothing observed
requires one.

**W6 — the real windows are intact on all four legs, so the aim is derived from the registers.**

Every leg: `real-ts=0/64 real-pool=0/8192`, and every `[v3d56]` real-region scan reads
`INTACT=64 … touched=0` / `INTACT=8192 … touched=0` after a completed write-back. The decoys are what
moved, on the two legs that programmed decoy bases. *Excludes:* "the write happens to land where the pool
already is". *Does not exclude:* the production values behaving differently — again §49.23.6.

**W7 — leg D's landing digest names exactly two pages in the whole arena, and both are the decoys.**

```
[v3d56] landing (v3d98 R-next-1 tsaim) — arena 64 pages (0x40000 B @ 0x00277000, the ENTIRE address space
        the V3D MMU grants this job) | changed=2 expected=0 STRAY=2 | per-region: tile-state (bin output)
        p[17..17]=0 · tile-alloc pool (bin output) p[18..25]=0 · probe TMU scratch (expected) p[52..52]=0 ·
        PTB overspill (expected) p[54..55]=0 | first_stray_page=4 (off 0x4000) last_stray_page=58
```

Page 4 is `arena + 0x4000` = the pool decoy; page 58 is `arena + 0x3a000` = the tile-state decoy. The
digest's "STRAY" wording is the instrument's, and on this leg it is a **naming artifact, not a finding**:
the per-region table knows only the four *real* regions, so a decoy hit can only ever be reported as
stray. Two writes, two pages, nothing else in the 256 KiB the MMU grants this job. *Excludes:* a scatter
of collateral writes anywhere in the arena. *Does not exclude:* writes outside the arena — but there are
none, because the catcher is clean.

**W8 — leg D's `BPCA` is uninterpretable, and the file says so rather than reading it.**

```
[v3d41] v3d98 R-next-1 tsaim CLE feed + PTB pointer — BPCA 0x0027e000->0x0027e000 (stale)
        (pool base 0x0027b000) BPCS=0x00000000 BPOA=0x00000000 BPOS=0x00000000
```

Leg C left `BPCA` at `0x27e000` = `QMA + QMS`, the pool end, and nothing resets it between legs, so leg D's
"unchanged" is the frozen-register case §48 named. The pool write on leg D nevertheless **landed** — the
poison says so. *The lesson is banked as a rule:* on a multi-leg rung, `BPCA` is only readable on the leg
that first moves it; the poison scans are the deciding instrument on every other leg, exactly as the
`[v3d41]` line's own text says.

**An honesty note that must ride with W4.** Leg D's `VIOLATION` line reads
`VIO_ADDR=0x00000000 VIO_ID=0x00000000 RAW, decoded client L2T @ VA 0x00000000` — with `fault=0`. **With no
fault latched the pair carries no information, and "client L2T" is the zero-`VIO_ID` decode artifact
§49.20.2's V2a law was written about.** It is not an attribution and must never be quoted as one. The V2a
law's read-once discipline is what makes the *other* legs' pairs trustworthy; it does not make a pair
meaningful on a leg that had nothing to report.

#### 49.23.3 The catcher's 48, explained — and what the count can and cannot carry

§49.22 retired the reading of the catcher as a *census* of one write. **That retirement stands**, and
boot8 now supplies the positive replacement, with no free parameters.

The catcher redirects every refused access to one 4 KiB scratch page, so overlapping refused writes
overlap *in the catcher too* and the dirty-word count is a **union**, not a sum. Taking the two writes as
boot8 measured them — the pool write occupying pool-relative words `{0..15} ∪ {28..31}` (§49.23.4) and the
tile-state write occupying `{0..47}` — the arithmetic on each leg is:

| leg | pool write | tile-state write | catcher predicted | catcher measured |
|---|---|---|---|---|
| A (zero bases) | refused, base 0 → words 0–15, 28–31 | refused, base 0 → words 0–47 | union = **48** | `48/1024` |
| B (zero bases, 46 B) | refused, base 0 | refused, base 0 | union = **48** | `48/1024` |
| C (mapped bases, `ENABLE=0`) | **lands** at `CT0QMA` | refused, base 0 → words 0–47 | **48** | `48/1024` |
| D (mapped bases, `ENABLE=1`) | **lands** at `CT0QMA` | **lands** at `CT0QTS` | **0** | `0/1024` |

Every row is predicted. But note precisely what carries the argument: on legs A and B the pool write's
footprint is a **subset** of the tile-state write's, so 48 there is consistent with one write *or* two and
proves nothing about the count. **The decisive legs are C and D** — C, where the pool write demonstrably
went somewhere else and the catcher still read 48, so those 48 belong wholly to the *other* write; and D,
where the other write landed and the catcher read zero.

#### 49.23.4 R-next-2, executed — the landed pool header, decoded as far as the bytes honestly take it

The `[v3d98] pool-head` dump rode all four legs, as designed. Verbatim, the `0x80`-byte window at
`CT0QMA`:

**Leg B (zero bases — the negative control).** All thirty-two words carry `0xA5A5A5A5 ^ index` exactly
(`a5a5a5a5 a5a5a5a4 a5a5a5a7 a5a5a5a6 a5a5a5a1 …`, verified word by word against the model). **The
instrument reads poison as poison.** No dump may be read without this control, and it passed.

**Legs C and D — byte-identical to each other:**

```
+0x000: 00000012 00000000 00000000 00000000 00000000 00000000 00000000 00000000
+0x020: 00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000
+0x040: a5a5a5b5 a5a5a5b4 a5a5a5b7 a5a5a5b6 a5a5a5b1 a5a5a5b0 a5a5a5b3 a5a5a5b2
+0x060: a5a5a5bd a5a5a5bc a5a5a5bf a5a5a5be 00000000 00000000 00000000 00000000
```

Reading the poison model off the bytes: words 0–15 written, words 16–27 **never written**, words 28–31
written. That is `1 + 15 + 4 = 20` words, one of them non-zero — which is the `[v3d56]` scan's
`OVERWRITTEN=1 ZEROED=19 touched=20`, `first_touched=[0]`, `last_touched=[31]`, `byte span [0x0,0x7f]`,
independently and exactly. **The dump and the scan corroborate each other**; neither is being trusted
alone.

Four things follow, in descending order of confidence.

**(1) The pool write is TWO disjoint blocks, not one span.** `[0x00,0x3f]` — 64 bytes — and `[0x70,0x7f]`
— 16 bytes — with 48 bytes of untouched poison between them at `[0x40,0x6f]`. The `[v3d56]` scan could
never say this: it reports endpoints and a count, and its span `[0x0,0x7f]` is the *hull* of two blocks,
not a written region. **This is the finding the dump was built for.**

**(2) The `0x70` is a pool offset, and the campaign's oldest number is retired as a mystery.** The second
block begins at exactly `CT0QMA + 0x70`. On a zero base that is absolute `VA 0x70` — the address the MMU
refused on boot11, on boot4's leg A, and on this boot's legs A and B. It is not structural, not an
instrument artifact and not a decode error: **it is where the second half of the pool write goes**, read
off landed bytes on a mapped base. And the latch's reported VA across the four legs now reads as one
consistent sequence:

| leg | pool write | tile-state write | latched VA | why |
|---|---|---|---|---|
| A, B | refused | refused | `0x70` | the pool write's second block, at base 0 |
| C | lands | refused | `0x0` | only the tile-state write is left to refuse, at base 0 |
| D | lands | lands | — (`fault=0`) | nothing to refuse |

**(3) `word[0] = 0x00000012` decodes, byte-wise, as an empty tile's tile list — one corroborating source,
nothing guessed past byte 0.** V3D control-list packets are byte-granular with **byte 0 as the opcode**;
that packing convention, and the opcode table this driver uses, are transcribed verbatim in
`arch/aarch64/v3d.rs` from Mesa `src/broadcom/cle/v3d_packet_v33.xml` (`gen="3.3" max_ver="42"` — the V3D
4.2 variants, which is this part). Little-endian, `0x00000012` is the byte sequence `12 00 00 00`. In that
table `18` — `0x12` — is `P_RETURN_FROM_SUB_LIST`, a **one-byte** packet; `0x00` is `Halt`. So the pool
head reads as **`RETURN_FROM_SUB_LIST` followed by Halt padding**, which is precisely the per-tile list an
*empty* tile has: the binner allocated the first tile's list block at the pool base and wrote the only
thing an empty tile's list can contain — return immediately.

Two independent things support the packet reading over the arithmetic reading (`0x12` = 18 = some count):
the byte sits at offset 0 of a pool whose consumer is the **CLE**, and this file already records the
render-side symptom of that fetch — §49.11a / the `invalidate_gpu_caches` note describes M4's render
fetching `opcode 0x00 = Halt` from the tile-alloc base when the L2T left it stale, i.e. the CLE
*does* fetch packets from exactly this address. **Status: decode ADOPTED for byte 0, on one corroborating
source.** No other field is named. If a second source ever contradicts it, this paragraph is where the
correction is appended.

**(4) The 64-byte first block is one cache line's worth, and the four zero words at `+0x70` are
unidentified.** 64 bytes is the V3D L2T line size, so a payload of one useful byte plausibly appears as a
full written line; the *content* beyond byte 0 is fifteen zero words and nothing distinguishes "written
zero" from "written zero as padding". The `[0x70,0x7f]` quad is sixteen bytes of zeros. Candidates that
suggest themselves — a second tile's list slot, a pool free-pointer, an end-of-list descriptor — are
**not adopted**: no source consulted corroborates a field, and this file does not guess values.

**What R-next-2 did not answer, and the instrument gap it exposed.** `[v3d98]` dumps the `CT0QMA` window
only. Leg D reported 48 tile-state words touched of which **three are non-zero**, and no line on the wire
says which three or what they hold — the tile-state payload, the more interesting of the two, is the one
this campaign has never seen. `[v3d99] head` closes that gap and is described in §49.23.7.

#### 49.23.5 What these verdicts retire, and the corrections appended elsewhere

| item | status after boot8 |
|---|---|
| §49.22.5's residual OPEN item — "what is the PTB's second close-time write, and what is its base?" | **ANSWERED.** It is the **tile-state write**; its base is `CT0QTS`; `CT0QTS.ENABLE` gates the base, not the write's existence (W4, W5) |
| §49.22.3(a) — the tile-state identification of the `VA 0` residual | **CONFIRMED** by the one experiment that could have refuted it |
| §49.22.5's wall statement | **SUPERSEDED** — see §49.23.6 |
| §49.21.4's generic leg rows, on leg D | **none fired**; leg D's own rows took precedence exactly as §49.22.6 designed. Legs B and C printed §49.21.4's text unchanged |
| catcher-as-census (retired in §49.22) | **retirement stands**, now with a positive replacement model that predicts all four legs (§49.23.3) |
| "structural `0x70`" | already **REFUTED** in §49.22 for the pool write; now additionally **EXPLAINED** — `0x70` is a pool-relative offset visible in landed bytes (§49.23.4 item 2) |
| whether an **armed** close writes | **STILL OPEN.** Re-opened by §49.22.3, unaddressed by this boot, and the reason R-next-3 is built here (§49.23.7) |
| §49.8 boot6's `48`-of-64 and `20` | **REPRODUCED** by an unarmed close on mapped bases with `ENABLE` set — the two paths now agree on both counts |

**Corrections appended elsewhere, per the append-corrections law — pointers, not rewrites.**

1. **`[v3d62]`'s fixed no-fault text.** On leg D the catcher line prints "the bin frame's missing PTB write
   was therefore neither refused by translation nor silently absorbed by the catcher". That sentence was
   written for a leg with a *missing* write. On leg D the writes are present and landed, and the sentence
   is true but misleading read alone. **The `[v3d62]` line must be read against the same leg's `[v3d56]`
   scans**, never on its own. §49.20.2's instrument-text discipline is extended to cover it.

2. **§49.20.2 V2a — the zero-pair artifact, restated as a positive rule.** V2a made every leg read the
   violation pair once and first. Boot8 adds the complementary rule: **on a leg with `fault = 0` the pair
   is meaningless and its decode must be discarded**, including the `client L2T` that a zero `VIO_ID`
   produces. Recorded at §49.23.2's honesty note.

3. **§48's frozen-register rule, extended to multi-leg rungs.** `BPCA` carries a verdict only on the leg
   that first moves it; on later legs of the same rung it is residue (W8).

4. **§49.22.3's two-write model.** Its clause (a) is promoted from *leading candidate* to **confirmed**;
   its clauses (b) and (c) are not refuted but are no longer needed to explain anything observed.

#### 49.23.6 The wall statement, corrected — three registers and one bit

Before this boot (§49.22.5): *"what is the PTB's second close-time write, and what is its base?"*

That question is answered. The corrected statement, and it is a statement rather than a question for the
first time in this campaign:

> **An unarmed empty bin frame closes correctly given three registers and one bit.** With `CT0QMA`,
> `CT0QMS` and `CT0QTS` programmed to mapped addresses and `CT0QTS.ENABLE` set, the frame closes
> (`BFC` Δ1, `FLDONE` latched, `CT0CA` walks `BA→EA`), the PTB emits **both** of its close-time
> writes — 20 words of tile list at `CT0QMA`, 48 words of tile state at `CT0QTS` — the MMU refuses
> nothing, and the illegal-address catcher stays pristine. **The wall is no longer at frame close.**

Two things remain, and they are of different kinds.

**(a) The ARMED path's close has still never been read this way.** §49.22.3's reconciliation of the armed
path was written as an explicit conditional and boot8 does not discharge it: every leg of boot8 is an
*unarmed* submit with registers programmed by hand. The production path additionally writes `BPOS = 0`,
runs the pre-kick L2T invalidate, and uses different values. **R-next-3 (`armedclose`) is built in
§49.23.7 and it is the next rung**, exactly as §49.22.5's recommended order said it would be —
"whatever leg D says, because the reconciliation needs it either way".

**(b) Bin CONTENT — the `m4` and `rcl` classes.** §49.8's bracket already localised these to *different*
stations from the empty class: `rcl` wedged at the first fetch, `m4` consumed its list and left the frame
open. Neither is a close-stage defect, and after boot8 the close stage is no longer a plausible common
cause for them. If `armedclose` returns `OUTCOME E1`, this is the whole of the remaining surface.

**Order, unchanged from §49.22.5 except that R-next-1 and R-next-2 are now struck as DONE:**
~~R-next-1~~ (done, this section) · ~~R-next-2~~ (done, §49.23.4) · **R-next-3 `armedclose` next** ·
R-next-1b `poolbig`/`spillaim` **de-prioritised** (its whole motivation was `OUTCOME D2`, which did not
fire) · R2 `mapzero` **de-prioritised further** (candidate (c) is not what remains) · R4 (the bunker)
unchanged, and weakened again by a third ARM-visible defect resolved without any firmware theory.

#### 49.23.7 Leg E (`armedclose`), built — the wire pre-written, and the gates

This subsection records R-next-3 as **built**, on the same terms §49.21 recorded `basedaim` and §49.22.6
recorded `tsaim`: it is a **design note, not a verdict**. QEMU raspi4b models no V3D, so nothing below has
been observed; the gates are compile, presence-in-artifact and knob-off byte-identity, and the reading
belongs to the next metal sitting.

**What was built.** One more leg on the `[v3d97]` scaffolding — five closes now — placed **after** leg D so
all four unarmed legs stay in-boot anchors and the unarmed and armed closes are compared inside one
capture, on one image, off one CT0 that was virgin at leg A.

| leg | list | `CT0QMA`/`CT0QMS`/`CT0QTS` | prologue | what it is |
|---|---|---|---|---|
| A | 14 B `emptyunarm` | untouched (zero residue) | none | the `[v3d95]` kick, unchanged |
| B | 46 B | untouched | none | R3 `lenvary` — the instrument control |
| C | 14 B `emptyunarm` | decoy bases, `ENABLE = 0` | none | R1 `basedaim` — the base cut |
| D | 14 B `emptyunarm` | leg C's values, `ENABLE = 1` | none | R-next-1 `tsaim` — one bit |
| **E** | 14 B `emptyunarm` | **REAL pool / `0x8000` / REAL tile-state `\| ENABLE`** | **`BPOS = 0`, then pre-kick L2T invalidate** | **R-next-3 `armedclose` — the production close** |

**Leg E is the one leg of the five that is not unarmed**, and the `[v3d95]` header's "NO `CT0QMA`, NO
`CT0QMS`, NO `CT0QTS`, NO `BPOS=0`, NO pre-kick L2T invalidate" is FALSE of it by design. A
`[v3d99] CORRECTION TO ALL THREE LINES ABOVE` says so on the wire, in front of every leg, so no reader
ever holds a wrong shape for the capture.

The submit order is `v3d_bin_job_run`'s own, as `[v3d57]` transcribed it: `BPOS = 0` **first**, then the
per-job cache invalidate, then `CT0QMA → CT0QMS → CT0QTS|ENABLE → CT0QBA → CT0QEA`. No `BPOA` is written
— which is the *opposite* of §19/§33's stale-overflow hazard, not an instance of it.

**The role inversion, stated once.** On legs C and D the two decoy windows are the targets and the two
real regions are the negative controls. **On leg E it is the other way round**: `CT0QMA`/`CT0QTS` name the
real tile-alloc pool and the real tile-state array, and the two decoys — re-poisoned by leg E like every
other leg — are leg E's negative controls. A decoy hit on leg E is therefore `OUTCOME E1b`, and it would
mean the aim is not derived from the registers that leg programmed.

**The new instrument: `[v3d99] head`.** `[v3d98]` dumps the `CT0QMA` window and nothing else, and
`OUTCOME D1` turned that into the live gap (§49.23.4). `[v3d99] head` dumps the **tile-state** window
verbatim on every leg, and on leg E additionally dumps both real targets. It is a pure read of arena
memory the scan's write-back and invalidate have already made coherent: no register is touched and no
verdict is drawn by it. It rides every leg for the same reason the pool dump does — the legs that write
nothing supply their own negative control.

Knob: **`UNAOS_V3D_ARMEDCLOSE=1`**, cargo feature `v3d_armedclose`, which **implies `v3d_tsaim`** (and
through it `v3d_basedaim`, `v3d_unarmclose` and `v3d`), declared in `Cargo.toml` and armed by the `arroyo`
block so the operator cannot arm half of it. Default OFF and fully uncompiled when off. The family law
holds structurally, as for `[v3d95]`/`[v3d97]`/`[v3d98]`: the boot returns long before `[v3d75]`'s
`ENABLE_QPU` and `[v3d80]`/`[v3d81d]`'s `DISPLAY_DONE` sends, so this knob can never sit beside them.

**Leg E's outcomes, pre-written verbatim on its own `VERDICT` line.** They take precedence over **both**
§49.21.4's generic rows and `[v3d98]`'s leg-D rows — leg E's `CT0QTS` also carries `ENABLE`, so leg D's
selector would otherwise claim it and report it against a decoy window leg E does not aim at. The two
void-branches (`submit_sound = 0`, `frame_closed = 0`) are still taken first, as on every leg.

| leg E reads | verdict begins | conclusion |
|---|---|---|
| **no fault, both real windows written, 20 pool / 48 tile-state** | `OUTCOME E1` | the armed empty close is **healthy and boot6's shape to the word**, now read with the poison, the catcher and the V2a pair boot6 did not have. §49.22.3's conditional reconciliation is **DISCHARGED**; the historic armed wall is **not** a close-stage artifact and the remaining surface is bin CONTENT — the `m4`/`rcl` classes |
| **fault, client PTB, `VA 0x0`** | `OUTCOME E2` | leg C's residual reappearing on the path that ships, on a boot where leg D showed the same bit removes it. The variable is then **not** `ENABLE` but the base VALUES or the armed prologue; remove them one at a time. `OUTCOME D1` still stands as measured — what it would not license is the inference that the armed close is healthy |
| **fault, client PTB, VA inside `[CT0QMA, CT0QMA+CT0QMS)`** | `OUTCOME E2b` | refused **inside its own pool**, a span the MMU maps — a **translation** question, not an aim question. Subtract `CT0QMA` on the line, then read the pool scan and the head dump for how far the write got |
| **fault, client PTB, VA inside the tile-state array** | `OUTCOME E2c` | the base is honoured and the refusal is at an offset within the 256-byte array. An offset at or past the array's end is a **geometry** finding and a driver fix — and it would explain the armed wall with no MMU theory at all |
| **no fault, a DECOY window moved** | `OUTCOME E1b` | the decoys are leg E's negative controls, so this means the aim is not derived from the registers leg E programmed — the tile-memory registers would be latch-once rather than per-kick. Voids the E1/E3 readings |
| **no fault, both real windows written, other counts** | `OUTCOME E5` | close completes, aim correct, **amount** differs. Read the counts against 20/48, then the byte spans and the head dumps |
| **no fault, exactly one of the two landed** | `OUTCOME E3` | a **PARTIAL** close. Pool without tile-state ⇒ `ENABLE` does not gate on production values what leg D says it gates; tile-state without pool ⇒ the binner emitted no per-tile list, which is a bin-content statement. Neither E1 nor E2 may be quoted |
| **no fault, nothing anywhere** | `OUTCOME E4` | contradicts §49.8 boot6 on the path boot6 measured. Check the `[v3d62]` catcher **first**, then the landing digest; if both are clean the difference from boot6 is the LIST |
| fault, client not PTB | — | attribution belongs to whichever unit `VIO_ID` names; no close-stage verdict |
| fault, VA none of the above | `OUTCOME E6` | §49.20.6's last row governs — subtract against all three bases printed on the same line |

**What leg E deliberately does NOT do.** It writes no `BPOA` (only `BPOS = 0`, `v3d_bin_job_run`'s own
first write). It edits no page table (still R2's). It sends no mailbox tag. It changes no arena geometry —
it reuses every compile-time assertion §49.21.2 carries and adds one (`TILE_STATE_BYTES == 8 × 32`, so the
real tile-state dump is exactly the array). It leaves `CT0QMA`/`CT0QMS`/`CT0QTS` at its production values,
which is the safest of the five endings — they are exactly what a later armed kick would program anyway —
and nothing runs after the rung in any case.

**Gates.**

`./arroyo check` and `UNAOS_WC=1 ./arroyo check` green for both arches (`v3d_armedclose` appended to the
`arm-pi` leg, so both polarities of every new `#[cfg]` are compiled). `./arroyo kernel8-test` green at its
banked standing on a quiet host. Presence in the builder-path artifact, per the full-knob law.

**Measured, this arc** (worktree `unaos-wt-exec-v3dfold3`, baseline `d99bec68`):

| gate | result |
|---|---|
| `./arroyo check` | green, both arches; `kernel cfg coverage OK (12 legs)`, `arm-pi` green |
| `UNAOS_WC=1 ./arroyo check` | green, both arches |
| knob-off `./arroyo kernel8-test 210` | **MBENCH PASS 117/117**, 0 forbidden, 22 424 lines scanned |
| knob-off byte-identity, `UNAOS_PI=1 ./arroyo kernel8` | arc-applied `e8ee5e33…` **==** baseline `e8ee5e33…` (baseline built in the same worktree at `d99bec68` before the edits; no stash, per `CLAUDE.md`) |
| armed build banner | all five knobs echo: `UNAOS_V3D`, `UNAOS_V3D_UNARMCLOSE`, `UNAOS_V3D_BASEDAIM`, `UNAOS_V3D_TSAIM`, `UNAOS_V3D_ARMEDCLOSE` |
| strings-proof, armed image `2b952813…` | `v3d99] ARMEDCLOSE LEG` 1 · `v3d99] ARMEDCLOSE COMPLETE` 1 · `v3d99] CORRECTION TO ALL THREE` 1 · `v3d99] LEG` 1 (the armed-prologue line) · `v3d99] head` 4 · `OUTCOME E1` 1 · `E1b` 1 · `E2` 1 · `E2b` 1 · `E2c` 2 · `E3`/`E4`/`E5`/`E6` 1 each · `v3d98] TSAIM LEG` 1 · `v3d98] pool-head` 2 · `OUTCOME D1` 5 (leg D's row, `D1b`, and three cross-references) |

*Scope of the byte-identity claim, stated honestly.* A `UNAOS_V3D_TSAIM=1` build is **not** claimed
byte-identical across this arc: `v3d97_leg` gained an `armed` parameter and leg E's outcome selector, and
the `[v3d99] head` dump is compiled into the shared leg. What is claimed, and gated, is that **every build
that does not arm `v3d_armedclose` is byte-identical to the pre-arc build** — which covers every boot that
is not a V3D experiment. One note on the suite: the first `kernel8-test 210` run of this arc failed on the
known quiet-host present-pacing flake (`[wc-k] rollup … torn=1 … maxpresent_us=16294 frame_us=16667 ->
AT-RISK`, 116/117), and passed 117/117 on a quiet re-run with nothing else building. The arc is knob-off
byte-identical, so it cannot be the cause; recorded rather than hidden.

**What the sitting needs.** One image:

```
UNAOS_V3D=1 UNAOS_V3D_UNARMCLOSE=1 UNAOS_V3D_BASEDAIM=1 UNAOS_V3D_TSAIM=1 UNAOS_V3D_ARMEDCLOSE=1 UNAOS_PI=1 ./arroyo kernel8
```

(`UNAOS_V3D_ARMEDCLOSE=1` alone suffices — the feature implies the rest — but the full line is what the
capture should record.) A **cold** power-cycle, as boot4, boot7 and boot8 were. One short capture,
labelled, never diffed line-for-line against a deep boot. **Read in this order:** `[v3d97] pad-selfcheck`,
leg B's `VERDICT`, leg C's `VERDICT`, leg D's `VIOLATION` and `VERDICT` (leg E's whole value is the
comparison against leg D — a boot whose leg D did not return `OUTCOME D1` leaves leg E without its
baseline), then leg E's `[v3d99] LEG E armed prologue`, its `VIOLATION`, its four `[v3d56]` scans, its
`[v3d98] pool-head` and `[v3d99] head` dumps, and finally its `VERDICT`.

---

### 49.24 `armedclose`, read on metal — the ARMED close is healthy, the close stage is CLOSED, and the tile-state payload is a pool pointer (PI-V3D-99 verdict, boot9/PA47, 2026-08-18)

Leg E returned `OUTCOME E1`. The production close — real pool base, real `0x8000` size, the real
tile-state array with `ENABLE`, `BPOS = 0` and the pre-kick L2T invalidate, in `v3d_bin_job_run`'s own
order — completes both of its close-time writes, latches no fault, leaves the illegal-address catcher
pristine, and lands **20 pool words and 48 tile-state words**: §49.8 boot6's armed numbers exactly, and
leg D's unarmed numbers exactly, all three inside one capture. §49.22.3's conditional reconciliation is
**DISCHARGED**.

The close stage is closed. Not "closed for the unarmed rung", not "closed given three registers and one
bit" — closed on the path that ships. This section folds that boot, executes the instrument gap
`[v3d99] head` was built for (the tile-state payload, which this campaign had never seen), and records
R-next-4 (`bincontent`) as built.

#### 49.24.1 The capture, and what kind of boot it was

`~/unaos-bench/capture/pi4-pi0-b1/ttyACM0.log`, past the mark

```
=== SQUAWK MARK 2026-08-18T05:50:08Z R24 boot9 pi4 — PA47 v3d99 armedclose, hw-pi4@e86555a4, fp 1281620 (Loaded 0x138e54) ===
```

816 lines, 246 of them `[v3d…]`, 117 verdict/violation/scan/dump lines. Banked sliced at
`~/unaos-bench/scratch/pi0-b1b2/boot9-full.log` (the whole boot) and `boot9-v3d.log` (the V3D lines).
Same shape as boot8: the rung returns before `probe_job`, so there is no `[v3d48]` ladder, no M4, no
visible battery and no `[v3d75]`/`[v3d80]`/`[v3d81]` tail. **It must never be diffed line-for-line
against a deep boot.**

Five closes off one CT0 that was virgin at leg A, and the wire proves they are five:
`BFC 0x0→0x1→0x2→0x3→0x4→0x5`, one Δ1 per leg, `CT0LC 0x10000 … 0x50000`, and every leg auditing
`submit_sound=1 idled=1` with `FLDONE` latched in the raw status.

**One difference from boot8 that has to be stated before anything is compared: the arena moved.** Boot8's
arena sat at `0x00277000`; boot9's sits at `0x00218000` — a different image is a different size, and the
arena is placed relative to it. Every absolute address below therefore differs from §49.23's while every
*offset* is identical, and that is not noise: it is what makes leg E's decode in §49.24.3 a two-point
measurement instead of a coincidence.

#### 49.24.2 The verdicts, each with its wire line and each with its exclusion

**W1 — legs A–D reproduced, on a third image and a moved arena.** The pad self-check held
(`first_diff=[-1] identical=1`). Leg B read `VIO_ADDR=0xe` on a 46-byte list for the third boot running.
Leg C reproduced boot7's and boot8's shape exactly — `decoy-pool=20/4096`, `first_touched=[0] (got
0x00000012)`, `last_touched=[31]`, span `[0x0,0x7f]`, residual PTB refusal at `VA 0x0`. And leg D
returned `OUTCOME D1` a second time:

```
[v3d97] LEG D VERDICT (v3d98 R-next-1 tsaim …) — … bases QMA=0x0021c000 QMS=0x00003000 QTS=0x00252002
        (ENABLE=1) … VIO_ADDR=0x00000000 VIO_ID=0x00000000 RAW -> client L2T @ VA 0x00000000 fault=0
        | poison touched: decoy-pool=20/4096 decoy-ts=48/1024 real-ts=0/64 real-pool=0/8192
        — OUTCOME D1 — NO FAULT, AND THE TILE-STATE DECOY TOOK THE WRITE.
```

*Excludes:* "boot8's leg D was one image's accident". Every count, every span and every `first_touched`
value repeats at a completely different arena base. *Does not exclude:* anything about the armed path —
that is leg E's job, below.

**W2 — `OUTCOME E1`, verbatim from leg E's own `VERDICT` line.**

```
[v3d97] LEG E VERDICT (v3d99 R-next-3 armedclose (PRODUCTION bases, BPOS=0 + pre-kick L2T invalidate,
        14-byte list)) — list len=14 (0xe) | frame-closed=1 (retired=1 FRDONE=0 BFC 0x00000004->0x00000005 Δ1)
        submit_sound=1 idled=1 INT_STS=0x00000002 waited=0us PCS=0x00000000 (BMACTIVE=0 BMBUSY=0 BMOOM=0)
        | bases QMA=0x0022a000 QMS=0x00008000 QTS=0x00229002 (ENABLE=1) POST-moved=0
        | VIO_ADDR=0x00000000 VIO_ID=0x00000000 RAW -> client L2T @ VA 0x00000000 fault=0
        | poison touched: decoy-pool=0/4096 decoy-ts=0/1024 real-ts=48/64 real-pool=20/8192 (drain completed=1)
        — OUTCOME E1 — THE ARMED EMPTY CLOSE IS HEALTHY, AND IT IS BOOT6's SHAPE TO THE WORD.
```

Every clause of the pre-written `E1` row fired at once: no fault, both real windows written, 20 and 48.
*Excludes:* `E2`/`E2b`/`E2c` (no refusal anywhere on the production path), `E3` (not partial), `E4` (not
empty), `E5` (not other counts), `E1b` (no decoy moved). *Does not exclude:* anything about bin CONTENT —
this leg's list is the 14-byte empty one.

**W3 — the catcher is pristine on the production leg, under the armed policy.**

```
[v3d62] fault (v3d99 R-next-3 armedclose …) — MMU_CTL=0x060d0c01 fault-latched=0x00000000
        (PT_INVALID=0 WRITE_VIOLATION=0 CAP_EXCEEDED=0) … HUB_INT_STS=0x00000000 MMU-int bits=0x00000000
        … scratch page 0x206000: dirty words=0/1024
```

Legs A, B and C each absorbed 48 words into that page on this same boot; legs D and E absorbed none.
*Excludes:* the reading that the armed path's writes are refused-but-unlatched — the catcher is a memory
witness, re-armed and re-seeded per leg, and it caught nothing. *Does not exclude:* a refused write below
the catcher's resolution; nothing observed requires one.

**W4 — leg E's landing digest is the first all-expected digest in the campaign.**

```
[v3d56] landing (v3d99 R-next-3 armedclose …) — arena 64 pages (0x40000 B @ 0x00218000, the ENTIRE
        address space the V3D MMU grants this job) | changed=2 expected=2 STRAY=0 | per-region:
        tile-state (bin output) p[17..17]=1 · tile-alloc pool (bin output) p[18..25]=1 ·
        probe TMU scratch (expected) p[52..52]=0 · PTB overspill (expected) p[54..55]=0
        | first_stray_page=-1 (off 0x0) last_stray_page=-1
```

Two pages changed, both of them regions this job is *supposed* to write, and `STRAY=0`. Compare leg D's
`changed=2 expected=0 STRAY=2` — same two writes, reported as stray only because the per-region table
knows the four real regions and leg D aimed at decoys (§49.23's W7 naming artifact). On leg E the
artifact cannot arise, because the targets *are* the real regions. *Excludes:* collateral writes anywhere
in the 256 KiB the MMU grants this job. *Does not exclude:* writes outside the arena — but there are
none, because W3.

**W5 — the two decoys are leg E's negative controls and both are intact.** `decoy-pool=0/4096`,
`decoy-ts=0/1024`, and both `[v3d56]` scans read `INTACT` in full after a completed write-back — on a leg
that came *after* legs C and D wrote into those very windows and re-poisoned them. *Excludes:* the
latch-once reading of the tile-memory registers (`OUTCOME E1b`): the aim is re-derived per kick from the
registers the kick programmed. *Does not exclude:* anything about the values themselves.

**W6 — the register arithmetic reproduces on the production values.**

```
[v3d41] v3d99 R-next-3 armedclose … CLE feed + PTB pointer — CT0LC=0x00050000 CT0PC=0x00000000
        PCS=0x00000000 (raw) | BPCA 0x0021f000->0x0022d000 (pool base 0x0022a000) BPCS=0x00005000
        BPOA=0x00000000 BPOS=0x00000000
```

`BPCA − CT0QMA = 0x2d000 − 0x2a000 = 0x3000`, and `BPCS = 0x8000 − 0x3000 = 0x5000`. `0x3000` is
`V3D56_EXPECTED_EMPTY_BPCA_ADVANCE` — Mesa's one-tile reservation, `align(1×1×128, 4096) + 8192` — banked
in code since §49.4 and used as leg C's `CT0QMS` since §49.21. Leg C reproduces it on a `0x3000` pool
(`BPCA = QMA + 0x3000`, `BPCS = 0`); leg E reproduces it on a `0x8000` pool with `0x5000` left over.
*Excludes:* the reading that the advance is a function of the pool SIZE. It is a function of the frame's
tile count, which is 1. *Does not exclude:* what happens when the frame has content — which is exactly
what §49.24.6 goes after.

**W7 — `CT0PC = 0` on the empty leg, and it is recorded as a baseline rather than read as a finding.**
The primitive-list counter sits at zero across leg E, as it must for a list with no primitives. It is
banked here because §49.8 boot8's `m4` leg read `CT0PC = 3`, and the content rung's verdicts branch on
that difference.

**The honesty note, carried forward unchanged from §49.23.** Leg E's violation pair reads
`VIO_ADDR=0x00000000 VIO_ID=0x00000000 RAW, decoded client L2T @ VA 0x00000000` with `fault=0`. **With no
fault latched the pair carries no information, and "client L2T" is the zero-`VIO_ID` decode artifact.**
§49.20.2's V2a law makes the *other* legs' pairs trustworthy; it does not make this one meaningful.

#### 49.24.3 R-next-2's gap, closed — the tile-state payload, decoded as far as the bytes honestly take it

§49.23.4 ended on a named instrument gap: leg D wrote 48 tile-state words of which **three were
non-zero**, and no line on the wire said which three or what they held. `[v3d99] head` was built for
exactly that. Here is the window it dumped, verbatim, on the two legs that wrote one.

**The negative control first, because no dump may be read without it.** On legs A, B, C and E the
*decoy* tile-state window reads pure `0xA5A5A5A5 ^ index` across all eight rows — for example leg E's:

```
[v3d99] head (…armedclose…) decoy TILE-STATE window @0x00252000 +0x000: a5a5a5a5 a5a5a5a4 a5a5a5a7 …
```

**The instrument reads poison as poison**, on the same leg whose real window it is being trusted about.

**Leg D — the decoy tile-state window, `CT0QTS = 0x00252002`, pool `CT0QMA = 0x0021c000`:**

```
+0x000: 00000000 00000000 00000000 00000000 00000000 00000000 c0001800 00000021
+0x020: 00120000 00000000 00000000 00000000 00000000 00000000 00000000 00000000
+0x040 … +0x0a0: all zero
+0x0c0, +0x0e0: poison intact
```

**Leg E — the REAL tile-state array, `CT0QTS = 0x00229002`, pool `CT0QMA = 0x0022a000`:**

```
+0x000: 00000000 00000000 00000000 00000000 00000000 00000000 a0001800 00000022
+0x020: 00120000 00000000 00000000 00000000 00000000 00000000 00000000 00000000
+0x040 … +0x0a0: all zero
+0x0c0, +0x0e0: poison intact
```

Words 0–47 written and words 48–63 never written — **48 words**, of which `ZEROED=45 OVERWRITTEN=3`,
span `[0x0,0xbf]`, and on leg E `INTACT=16` of 64. That is the `[v3d56]` scan line independently and
exactly. **The dump and the scan
corroborate each other**; neither is being trusted alone.

Three things follow, in descending order of confidence.

**(1) The 192-byte tile-state write has exactly ONE island of content, at bytes `[0x18,0x23]`, and
everything else it writes is zero.** Twelve bytes, of which four are non-zero. Byte for byte:

| byte offset | leg D | leg E |
|---|---|---|
| `0x18`–`0x19` | `00 18` | `00 18` |
| `0x1a`–`0x1d` | `00 c0 21 00` | `00 a0 22 00` |
| `0x1e`–`0x21` | `00 00 00 00` | `00 00 00 00` |
| `0x22`–`0x23` | `12 00` | `12 00` |

**(2) The four bytes at `0x1a` are `CT0QMA`, little-endian, exact, on two different bases — this is the
tile's LIST POINTER.** `00 c0 21 00` = `0x0021c000` = leg D's pool base. `00 a0 22 00` = `0x0022a000` =
leg E's pool base. Those are the ONLY bytes in the whole 192-byte write that differ between the two legs,
and they track `CT0QMA` and nothing else — in particular they are **not** `BPCA`, which on leg E stood at
`0x0022d000`, a value that would have been visible here had it been the field's source. So the per-tile
tile-state entry records where that tile's list block lives, and for a one-tile empty frame that is the
pool base itself. **Status: decode ADOPTED**, on a two-point measurement across two arenas.

This closes a loop with §49.23.4's item 3. That paragraph read pool `word[0] = 0x00000012` as
`RETURN_FROM_SUB_LIST` (opcode 18) followed by `Halt` padding — the per-tile list an empty tile has. The
tile-state entry now says, in the same 48 words, *that is where the tile's list is*. Two independently
dumped regions, each naming the other's role. Neither reading was available to the other when it was
made.

**(3) The remaining eight bytes of the island are BASE-INDEPENDENT and are NOT decoded.** `00 18` at
`0x18` and `12 00` at `0x22` are byte-identical across two legs, two bases, two arenas. That is a real
fact and it is all this file will say about them. Candidates suggest themselves — `0x1800` as a size or
block count, the `0x12` echoing the pool head's opcode — and **none is adopted**: no source consulted
corroborates a field at either offset, and this file does not guess values. If a source is ever found,
this paragraph is where the correction is appended.

**What the dump did not answer.** The 45 zero words. Nothing distinguishes "written zero because the
field is zero" from "written zero as padding", and for an *empty* tile every per-tile field that could be
non-zero is empty by construction. **The one experiment that separates them is a tile with content in
it** — which is §49.24.6.

#### 49.24.4 What these verdicts retire, and the corrections appended elsewhere

| item | status after boot9 |
|---|---|
| §49.22.3's conditional reconciliation of the ARMED path | **DISCHARGED.** The armed close completes both writes, faults nowhere, and matches the unarmed close word for word (W2, and leg D on the same boot) |
| §49.23.6(a) — "the ARMED path's close has still never been read this way" | **CLOSED.** It has now, under the poison, the catcher, the V2a pair and the landing digest |
| §49.23.6's wall statement | **SUPERSEDED** — see §49.24.5 |
| §49.8 boot6's `48`-of-64 and `20` | **REPRODUCED A THIRD TIME**, now on the armed path with instruments boot6 did not have. The armed and unarmed closes agree on both counts |
| `OUTCOME D1` (§49.23) | **REPRODUCED** on a second image and a moved arena (W1). It was not one boot's accident |
| §49.23.4's "the tile-state payload is the one this campaign has never seen" | **RETIRED.** Seen, dumped verbatim on two legs, and one of its three non-zero words decoded (§49.24.3) |
| the historic ARMED wall as a close-stage artifact | **REFUTED.** The close stage completes on the production path; whatever the armed wall was, it is not here |
| §49.8's `m4` and `rcl` stations | **UNTOUCHED, and now the whole of the remaining surface** — which is why §49.24.6 exists |
| R-next-1b (`poolbig`/`spillaim`), R2 (`mapzero`) | **de-prioritised further.** Both were motivated by outcomes that did not fire on any of the three boots |
| R4 (the bunker brief) | **weakened a fourth time** — a fourth ARM-visible defect resolved with no firmware theory |

**Corrections appended elsewhere, per the append-corrections law — pointers, not rewrites.**

1. **§49.23.6's wall statement, at its own head.** "An unarmed empty bin frame closes correctly given
   three registers and one bit" is TRUE and stays; it is now the weaker of two statements, and the
   stronger one is §49.24.5. Any citation of §49.23.6 must carry the pointer forward.

2. **§49.23.4 item 4, "the four zero words at `+0x70` are unidentified".** Unchanged as to the pool, and
   now joined by its tile-state counterpart: §49.24.3 item 3 names two more base-independent fields that
   are recorded and not decoded. Both paragraphs are correction sites, and both stay open.

3. **`[v3d62]`'s fixed no-fault text, extended once more.** §49.23.5 item 1 required the catcher line to
   be read against the same leg's `[v3d56]` scans. On leg E the line's phrase "the bin frame's missing
   PTB write" is now *false on its face* — nothing is missing — and the discipline is upgraded from "read
   together with" to: **on any leg whose scans show landed bytes, `[v3d62]`'s narrative clause is void
   and only its MMU registers and its dirty-word count may be quoted.**

4. **§49.22's `BPCA` frozen-register rule.** Leg E moved `BPCA` (`0x21f000 → 0x22d000`) and is therefore
   a leg on which it *is* readable — W6 reads it. §48's rule is unchanged; boot9 is the case that shows
   the rule has a positive branch and is not a blanket prohibition.

5. **§49.18.4 item 4's structural divergence** (piOS places the tile-state array at the POOL END; we
   place it one page below the pool base). Boot9 confirms our placement on the wire — `CT0QTS =
   0x00229002`, `CT0QMA = 0x0022a000`, exactly one page apart — and shows it is **not load-bearing**: the
   close completes at that placement. The divergence stays recorded; it stops being a suspect.

#### 49.24.5 The wall statement, final form for the close stage

Before this boot (§49.23.6): *"An unarmed empty bin frame closes correctly given three registers and one
bit."*

> **The bin frame CLOSE STAGE is closed, on the path that ships.** The production submit — `BPOS = 0`,
> the pre-kick L2T invalidate, then `CT0QMA` / `CT0QMS` / `CT0QTS|ENABLE` / `CT0QBA` / `CT0QEA` in
> `v3d_bin_job_run`'s own order — closes an empty bin frame (`BFC` Δ1, `FLDONE` latched, `CT0CA` walks
> `BA→EA`), emits **both** close-time writes into the regions it named (20 words of tile list at
> `CT0QMA`, 48 words of tile state at `CT0QTS`, whose byte at `+0x1a` points back at `CT0QMA`), refuses
> nothing, strays nowhere in the 256 KiB the MMU grants it, and leaves the illegal-address catcher
> pristine. **There is no wall at frame close, armed or unarmed.**

What remains is one thing, and it is what §49.8's bracket said it was from the beginning: **bin CONTENT,
and then the render list.** §49.8 localised those to different stations from the empty class — `m4`
consumed its list and left the frame open; `rcl` froze at its first fetch — and neither is a close-stage
defect. After boot9 the close stage is not even a plausible common cause for them.

The order is now forced, not chosen. `m4` first, because it shares a submit path with the leg that just
came back healthy, so exactly one variable separates them: the list. `rcl` after, because it is a
different engine's list on a different queue and nothing about it is constrained by what the binner does.

#### 49.24.6 R-next-4 (`bincontent`), built — the content ladder, the wire pre-written, and the gates

This subsection records R-next-4 as **built**, on the same terms §49.21/§49.22.6/§49.23.7 recorded their
rungs: it is a **design note, not a verdict**. QEMU raspi4b models no V3D, so nothing below has been
observed; the gates are compile, presence-in-artifact and knob-off byte-identity, and the reading belongs
to the next metal sitting.

**What was built.** Three more legs on the `[v3d97]` scaffolding — eight closes now — placed **after**
leg E, each carrying leg E's production bases and leg E's armed prologue, so that between leg E and any
of them **exactly one thing varies: the list.**

| leg | list | packets / bytes | shader record | what it is |
|---|---|---|---|---|
| E | 14 B `emptyunarm` | 5 / 14 | none | the empty control, `OUTCOME E1` |
| **F** | `BinContent::StateNoPrims` | 13 / 66 | `OFF_SHADREC` | full fixed-function state + `GL_SHADER_STATE`, **no primitives** |
| **G** | `BinContent::PrimsNullShader` | 14 / 76 | `OFF_BISECT_NULL_SHADREC` | F **+** `VERTEX_ARRAY_PRIMS`, over the NULL coord shader (a real dispatching thread that writes nothing to VPM) |
| **H** | `BinContent::Full` | 14 / 76 | `OFF_SHADREC` | **the `m4`-class MINIMUM** — one tile config, one flat triangle, the real coordinate shader |

**Why H is the smallest honest content list, and why it is not smaller.** The brief for this rung is "one
tile config plus one flat triangle", and that is exactly what leg H is: `TILE_BINNING_MODE_CFG` (which
leg E already carries) plus `VERTEX_ARRAY_PRIMS(TRIANGLES, 3)` over the three `TRI_VERTS`. It cannot be
cut below that, and the reason is in this file rather than in taste: **PI-V3D-17's verdict is that
without the clip/viewport/clipper state the hardware clipper runs at power-on-reset zeros, a zero
viewport scale collapses every primitive to a point, and the binner writes "an empty-but-legal bin
(tile-alloc pool never touched)".** A content list stripped of that state would therefore return a null
result *indistinguishable from the empty close* — it would destroy the discrimination the rung exists
for. So the fixed-function state stays, and legs F and G in front of H are not padding: they are §49.8's
own class decomposition (`StateNoPrims`, `PrimsNullShader`), measured for the first time under this
instrument set and on the production submit.

**The new surface, stated before the instruments.** Every leg A–E submitted a list that references **no
DRAM at all** — fourteen bytes of self-contained packets. A content list does not. `GL_SHADER_STATE`
embeds the shader record's address; the record embeds the CS/VS/FS code addresses, three uniform-stream
addresses and the default-attribute block; the attribute record embeds the vertex buffer. **Eleven
regions the GPU must READ.** That is what this rung adds, and it is instrumented three ways:

1. **Named and bounds-checked, fail-closed.** One `[v3d100] ref` line per region before the kick — name,
   iova, live bytes, arena page, `arena_mapped`. If any region escapes the arena the leg **refuses to
   kick** and says so, and no verdict is implied.
2. **A content witness.** An FNV-1a digest of each region's live bytes, taken after publish and again
   after the post-kick L2T write-back and CPU invalidate. Any change means the GPU wrote into its own
   input.
3. **Poison collars.** The unused remainder of the two pages whose live payload is tiny — the shader
   record page (52 bytes of 4096) and the vertex-data page (48 of 4096) — **is** poisoned and **is**
   scanned by `v3d56_scan`, exactly like leg E's four windows.

**Why the referenced regions are not simply poisoned, stated once so the omission is not read as an
oversight.** Poison is a *write-detector on a region whose correct content is nothing*. A shader page
filled with `0xA5A5A5A5` is not a shader, and a vertex buffer full of poison is not a triangle: poisoning
the live bytes would not instrument the experiment, it would replace it. The digest is the honest
analogue for live bytes, the collar is the honest analogue for the space beside them, and the arena
landing digest covers all 64 pages underneath both. Between the three there is no byte in the address
space the V3D MMU grants this job that can be written without a witness saying so.

**CT0 hygiene, which is why the legs are ordered as they are.** §49.3 is this campaign's founding lesson:
`probe_job`'s unclosed bin frame poisoned every CT0 kick behind it for three arcs. A content leg that
stalls mid-frame is precisely that hazard, reproduced on purpose. So the three legs run in **strictly
increasing order of stall risk** — state, then prims-with-a-null-shader, then the real draw — and after
each one the ladder checks that the frame closed *and* `CTRUN` cleared. If it did not, every later leg is
**STOOD DOWN** with its own wire line rather than measured on a dirty CT0. A stall on leg F therefore
costs the rung legs G and H and costs the file nothing, which is the correct trade.

**The frame-close criterion, which is the point of the rung.** §49.8 could distinguish "retired" from
"did not retire" and nothing else. The three endings this rung must separate, and the instruments that
separate them:

| ending | signature |
|---|---|
| **closed-with-output** | `BFC` Δ1 **and** pool words **> 20** — per-tile primitive-list bytes beyond the empty tile's lone `RETURN_FROM_SUB_LIST`; corroborated by `BPCA − CT0QMA > 0x3000` |
| **closed-empty** | `BFC` Δ1 **and** pool words **== 20**, tile-state words **== 48** — bit-for-bit the empty close's shape, i.e. the content binned to nothing |
| **stalled-mid-content** | `BFC` Δ0, `CT0CA == EA` (whole list consumed), `PCS.BMACTIVE` set — §49.8 boot8's `m4` station |
| **wedged-at-fetch** | `BFC` Δ0, `CT0CA == BA` — the `rcl` signature, on a bin list |

All four quantities ride one new line, `[v3d100] CONTENT ARITHMETIC`, printed in front of every content
verdict and carrying no verdict of its own: `CT0CA` against `BA`/`EA`, `CT0PC`, the full `PCS`, `BPCA`'s
advance off `CT0QMA` against `V3D56_EXPECTED_EMPTY_BPCA_ADVANCE` and against `CT0QMS`, both word counts
against leg E's 20 and 48, and the two reference-witness channels.

**The outcomes, pre-written verbatim on each content leg's own `VERDICT` line.** They take precedence
over leg E's rows, leg D's rows *and* §49.21.4's generic rows — they must, because a content leg carries
leg E's bases and `CT0QTS.ENABLE`, so all three earlier selectors would otherwise claim it and answer a
question it is not asking. The two void-branches (`submit_sound = 0`, `frame_closed = 0` handled by the
rows below) and the stand-down gate are taken first.

| the leg reads | verdict | conclusion |
|---|---|---|
| closed, no fault, both real windows written, **pool > 20** | `OUTCOME C1` | **content FLOWS.** The binner accepted geometry, emitted a real tile list and closed its frame. §49.8's `m4` station is not reproducible under the production submit; the bin path is whole and the last named room is the RENDER list |
| closed, no fault, **pool == 20 and tile-state == 48** | `OUTCOME C2` | **closed-EMPTY.** The content list behaved as an empty one — PI-V3D-17's "empty-but-legal bin". A **vertex-path** statement: VCD attribute DMA, then coord-shader VPM output, then clipper state, in that order. If F, G and H all read C2, the shader is not the variable and the VCD is |
| closed, nothing written anywhere | `OUTCOME C2b` | closed and wrote **less than an empty frame**, contradicting leg E on the same image. Catcher and landing digest first; if both are clean this is the most interesting line in the capture |
| closed, other counts | `OUTCOME C2c` | aim right, amount different. Read the counts, then the spans, then both head dumps against leg E's |
| **not closed**, list consumed, bytes on the ground | `OUTCOME C3a` | stalled mid-content **with output already emitted** — the wall sits between "list bytes emitted" and "frame closed". Not the same measurement as §49.8 boot8, which read both regions INTACT, and the difference is the first thing to state |
| **not closed**, list consumed, nothing written, `BMACTIVE` set | `OUTCOME C3b` | §49.8 boot8's `m4` reading reproduced **exactly**, now with the catcher, the poison, the V2a pair and the landing digest. `CT0PC` then splits it: primitives fed with no output ⇒ the PTB's primitive-to-list-bytes stage; zero primitives ⇒ upstream, the VCD/vertex path |
| **not closed**, list consumed, nothing written, `BMACTIVE` clear | `OUTCOME C3c` | binner IDLE with no frame retired — neither boot8's reading nor leg E's. `PCS` in full, then `FLDONE` |
| **not closed**, `CT0CA == BA` | `OUTCOME C4` | the `rcl` freeze on a BIN list, which §49.8's bracket says should not happen. Nothing downstream of fetch may be read |
| **not closed**, `CT0CA` between `BA` and `EA` | `OUTCOME C4b` | a **mid-list** stop, which no leg in this campaign has produced. Subtract from `BA` and count packets forward through the `[v3d36]` decode: the packet, not the class, is the finding |
| **not closed**, `PCS.BMOOM` | `OUTCOME C5` | **pool exhaustion.** This rung writes `BPOS = 0` and no `BPOA` by design (leg E's prologue), so the PTB has no overflow block. A driver-geometry finding, the cleanest possible failure, and its remedy is one rung away |
| any referenced region's digest changed, or a collar moved | `OUTCOME C6` | the GPU wrote into its own input. New surface, invisible to every earlier leg. **Voids** the leg's C1/C2/C3 reading until explained |
| no fault, a DECOY moved | `OUTCOME C1b` | as `E1b` — and a decoy hit *here* but not on leg E, with the same bases, is a statement about the list |
| fault at `VA 0x0` / in the pool span / in the tile-state array / elsewhere | `C7` / `C7b` / `C7c` / `C7d` | leg E drew no fault on these same registers, so the variable is the list. `C7c` at or past the array's end is the **geometry** finding leg E could not have produced: an array sized for an empty tile and not for a tile with a list. `C7d` subtracts against the eleven ref iovas as well as the three bases |
| fault, client not PTB | — | and on a content list this is a *different* statement than on legs A–E: the CLE, the VCD and the TMU can all issue for this list, so a CLE/VCD client means the refused address is a **read** of a referenced region |

**What this rung deliberately does NOT do, and what is therefore deferred.**

- **The render side is untouched.** Nothing on CT1, no `rcl`, no `BRANCH_TO_IMPLICIT_TILE_LIST`, no
  store. Whether a binned list can be *rendered* is the next room and this rung says nothing about it.
- **Vertex-path correctness is not tested, only vertex-path *flow*.** `OUTCOME C1` says content reached
  the pool; it does not say the triangle is in the right place. The instruments for that
  (`[v3d27]`'s TMU probe, `cs_vpm_output_witness`) exist and are not armed here — they ride `probe_job`,
  which this rung returns in front of.
- **No overflow block.** `BPOS = 0` and no `BPOA`, exactly leg E's prologue, because changing the
  prologue would add a second variable. If `OUTCOME C5` fires, arming `BPOA`/`BPOS` at the arena's
  overspill pool is the deferred one-rung follow-up.
- **One tile.** `TARGET_W`×`TARGET_H` is 64×64, which is exactly one 64×64 tile, and a compile-time
  assertion holds it there (`TILE_STATE_TILES == 1`). Multi-tile binning — where the tile-state array's
  per-tile stride and §49.24.3's list pointer would both become measurable across tiles — is deferred.
- **No page table is edited, no mailbox tag is sent, no arena geometry changes.** Unchanged from every
  rung since §49.21.

Knob: **`UNAOS_V3D_BINCONTENT=1`**, cargo feature `v3d_bincontent`, which **implies `v3d_armedclose`**
(and through it `v3d_tsaim`, `v3d_basedaim`, `v3d_unarmclose` and `v3d`), declared in `Cargo.toml` and
armed by the `arroyo` block so the operator cannot arm half of it. Default OFF and fully uncompiled when
off. The family law holds structurally, as for `[v3d95]`/`[v3d97]`/`[v3d98]`/`[v3d99]`: the boot returns
long before `[v3d75]`'s `ENABLE_QPU` and `[v3d80]`/`[v3d81d]`'s `DISPLAY_DONE` sends, so this knob can
never sit beside them. A `[v3d100] CORRECTION TO ALL FOUR LINES ABOVE` rides in front of every leg, so no
reader ever holds a wrong shape for the capture — in particular the `[v3d95]` header's "the `emptyunarm`
shape byte for byte" is FALSE of F/G/H by design, and the line says so.

**Gates.**

`./arroyo check` and `UNAOS_WC=1 ./arroyo check` green for both arches (`v3d_bincontent` appended to the
`arm-pi` leg, so both polarities of every new `#[cfg]` are compiled). `./arroyo kernel8-test` green at its
banked standing on a quiet host. Knob-off byte-identity. Presence in the builder-path artifact, per the
full-knob law.

**Measured, this arc** (worktree `unaos-wt-exec-v3dfold4`, baseline `e86555a4`):

| gate | result |
|---|---|
| `./arroyo check` | green, both arches; `kernel cfg coverage OK (12 legs)`, `arm-pi` green |
| `UNAOS_WC=1 ./arroyo check` | green, both arches; `kernel cfg coverage OK (12 legs)` |
| knob-off `./arroyo kernel8-test 210` | **MBENCH PASS 117/117**, 0 forbidden, 27 275 lines scanned, first run, quiet host |
| knob-off byte-identity, `UNAOS_PI=1 ./arroyo kernel8` | arc-applied `e8ee5e33…` **==** baseline `e8ee5e33…` (baseline built in a throwaway worktree at `e86555a4`, per `CLAUDE.md`'s no-stash law — and the same digest §49.23's arc banked, so the knob-off image has not moved in two arcs) |
| armed build banner | all **six** knobs echo: `UNAOS_V3D`, `UNAOS_V3D_UNARMCLOSE`, `UNAOS_V3D_BASEDAIM`, `UNAOS_V3D_TSAIM`, `UNAOS_V3D_ARMEDCLOSE`, `UNAOS_V3D_BINCONTENT` |
| strings-proof, armed image `18276c4f…` | `v3d100] BINCONTENT LADDER` 1 · `BINCONTENT COMPLETE` 1 · `CORRECTION TO ALL FOUR` 1 · `LADDER STOOD DOWN` 1 · `v3d100] LEG` 3 · `v3d100] ref ` 4 · `ref-digest` 1 · `ref-witness` 1 · `COLLAR` 2 · `CONTENT ARITHMETIC` 1 · `bincontent leg F`/`G`/`H` 1 each · `OUTCOME C1`/`C1b`/`C2`/`C2b`/`C2c`/`C3a`/`C3b`/`C3c`/`C4`/`C4b`/`C5`/`C7`/`C7b`/`C7d` 1 each, `C6`/`C7c` 2 each (row + cross-reference) · `refusing the kick (fail-closed)` 3 · `OUTCOME E1` 2 · `v3d99] head` 6 |

*Scope of the byte-identity claim, stated honestly.* A `UNAOS_V3D_ARMEDCLOSE=1` build is **not** claimed
byte-identical across this arc: `v3d97_leg` gained a `content` parameter and a return value, and the
content-leg instrument block is compiled into the shared leg. What is claimed, and gated, is that **every
build that does not arm `v3d_bincontent` is byte-identical to the pre-arc build** — which covers every
boot that is not a V3D content experiment.

**What the sitting needs.** One image:

```
UNAOS_V3D=1 UNAOS_V3D_UNARMCLOSE=1 UNAOS_V3D_BASEDAIM=1 UNAOS_V3D_TSAIM=1 UNAOS_V3D_ARMEDCLOSE=1 UNAOS_V3D_BINCONTENT=1 UNAOS_PI=1 ./arroyo kernel8
```

(`UNAOS_V3D_BINCONTENT=1` alone suffices — the feature implies the rest — but the full line is what the
capture should record.) A **cold** power-cycle, as boot4, boot7, boot8 and boot9 were. One short capture,
labelled, never diffed line-for-line against a deep boot. **Read in this order:** the four `CORRECTION`
lines, `[v3d97] pad-selfcheck`, legs B/C/D's `VERDICT`s (the in-boot anchors), leg E's `VERDICT` (the
EMPTY control every content leg is read against — a boot whose leg E did not return `OUTCOME E1` leaves
F/G/H with no baseline), then for each of F, G and H in turn: its eleven `[v3d100] ref` lines, its
`VIOLATION`, its four `[v3d56]` poison scans, its two `[v3d100] COLLAR` scans, its eleven
`[v3d100] ref-digest` lines and the `[v3d100] ref-witness` summary, its head dumps, its
`[v3d100] CONTENT ARITHMETIC` line, and finally its `VERDICT`. **The first leg whose verdict differs from
leg E's is where content enters.**

### 49.25 `bincontent`, read on metal — content enters at PRIMITIVE DISPATCH, and the wall has its smallest name yet (PI-V3D-100 verdict, boot11/PA48 reflight, 2026-08-18)

#### 49.25.1 The capture, and what kind of boot it was

Capture `pi4-pi0-b2/ttyACM1.log`, mark `R24 boot11` (2026-08-18T12:18Z), a cold power-cycle of the
PA48 image (fp 1306708, `Loaded 0x13f054`). Boot10 — the first PA48 flight — was LOST to the stale
reader-fd hazard (the ACM node re-enumerated and the reader held the dead descriptor); boot11 is its
reflight and the hazard's fix is now process law. Eight CT0 kicks flew: legs A/B/C/D/E exactly as
§49.20–§49.23 defined them, then F/G/H per §49.24. Every leg's V2a discipline held (violation pair
read once, first), and the hygiene gate did its job at the end.

#### 49.25.2 The verdicts, leg by leg

| leg | list | verdict |
|---|---|---|
| E (empty control) | 14 B, production bases + armed prologue | **`OUTCOME E1`** — third consecutive boot. Closed, no fault, 20 pool words, 48 tile-state words. The content legs have their baseline |
| F (state, NO prims) | 66 B: full fixed-function state + `GL_SHADER_STATE` | **`OUTCOME C2` — CLOSED-EMPTY.** `BFC` Δ1, no fault, wrote *exactly* the empty close's 20/48 words, refs 0/11 changed, collars 0. The whole state block — shader record included — is consumed without disturbing the close |
| G (F + `VERTEX_ARRAY_PRIMS`, NULL coord shader) | 76 B | **FRAME NEVER CLOSED.** `waited=500000us` timeout; `BFC` Δ0; `CT0CA == EA` (list fully consumed); **`CT0PC=3`** (the CLE counted all three vertices' primitives); `PCS.BMACTIVE=1` held; **0 pool words, 0 tile-state words** (the close-time writes legs E/F both made were never made); `INT_STS=0x00010000` (the closing legs read `0x00000002` — the set bit is not the frame-done family; decode owed); no MMU fault; refs 0/11 changed; collars 0 |
| H (one real triangle) | — | **STOOD DOWN** by the CT0-hygiene gate, correctly: leg G left a frame open, so H on a dirty CT0 would carry no information |

#### 49.25.3 What boot11 proves

**The wall statement, at its narrowest ever:** the bin frame stops closing **the instant a primitive
dispatches**, and *before shader output matters* — leg G's coord shader is the NULL shader, a real
dispatching thread that writes nothing to VPM, and the freeze is indifferent to that. Combined with
leg F: every byte of fixed-function state plus the shader record parses and closes clean; adding the
one `VERTEX_ARRAY_PRIMS` packet converts a closing frame into a dead-open one. And the failure is
not a refusal — no fault latches, no referenced region is touched, the list is consumed to its end,
and the primitive counter advances to 3. The machine accepts everything and then never finishes.

Three prior readings are sharpened by this:

- **§49.8 boot8's `m4` station is reproduced under full instrumentation** — `OUTCOME C3b`'s shape
  with `CT0PC` NONZERO. Per §49.24's pre-written split, primitives-fed-with-no-output points at the
  **PTB's primitive-to-list-bytes stage or the stations feeding it** (VCD attribute fetch → coord
  thread → VPM → PTB), not at CLE parsing and not at the close stage.
- **The close stage is fully exonerated** (§49.23–§49.24 already closed it; leg F re-proves it on
  the same boot content fails on).
- **Memory refusal is excluded as the mechanism**: 0/11 referenced regions changed and no fault —
  the VCD either never issued its attribute reads, or issued them invisibly to every instrument we
  hold. Which of those it is, is precisely the next rung's question.

#### 49.25.4 The next rung: `dispatchdisc` (PI-V3D-101) — where between VCD and PTB does dispatch die?

Leg G leaves exactly one span of pipeline in the dark: `VERTEX_ARRAY_PRIMS` accepted (CT0PC=3) →
…nothing… → no pool/tile-state write, frame open. The stations inside that span: VCD attribute DMA,
coord-thread launch on a QPU, VPM write-back, PTB intake. The two questions the baton names — *does
the coord thread start?* and *does FLDONE wait on a VPM delivery the VCD never makes?* — are both
answerable with the V3D performance-counter block (`PCTR`), which is ARM-readable, requires no new
memory surface, and rides the proven leg G shape unchanged:

- Program 4–8 `PCTR` slots before leg G's kick with sources covering: FEP valid primitives,
  QPU active/instruction cycles (coord class), VCD/VCM events, and VPM writes. Read them after the
  bounded wait, and read the SAME slots across legs E and F as in-boot zero-baselines.
- Pre-written outcomes, one per span station: **D1** QPU cycles Δ0 ⇒ the coord thread NEVER LAUNCHES
  (the wall is thread dispatch itself — shader-record fetch or TSDA); **D2** QPU Δ>0, VPM writes Δ0
  ⇒ the thread runs and delivers nothing (NULL-shader leg reads this as CORRECT thread behavior —
  re-run against leg H's real shader before concluding); **D3** VPM writes Δ>0, pool words 0 ⇒ the
  PTB receives and discards — the wall is PTB intake; **D4** every counter Δ0 including FEP ⇒ the
  CLE counted primitives it never forwarded, and CT0PC's meaning itself is the finding.
- Also owed on the same boot, for free: decode `INT_STS=0x00010000` (one register read against the
  databook's INT bit table) — if that bit names an error family, it may answer the rung before the
  counters do.

Knob `UNAOS_V3D_DISPATCHDISC=1`, feature `v3d_dispatchdisc`, implying `v3d_bincontent`'s chain,
default OFF, same family law (the boot returns before `probe_job`; never beside QPU/DISPLAY_DONE
sends). Image = **PA49**. Leg H remains stood down until G closes; the discriminator is measured on
the leg that fails, not the leg that would.

##### 49.25.4a `dispatchdisc`, built — the bank, the two corrections, and the gates (PI-V3D-101)

This records the rung as **built**, on the terms §49.21/§49.22.6/§49.23.7/§49.24.6 recorded theirs:
a **design note, not a verdict**. QEMU raspi4b models no V3D and no `PCTR` block, so nothing below has
been observed; the gates are compile, presence-in-artifact and knob-off byte-identity, and the reading
belongs to the next metal sitting.

**What was built — no leg and no kick.** The boot still takes **eight** CT0 closes and legs A…H are
byte-for-byte what `v3d_bincontent` submits. What this rung adds is instrumentation on the four legs
that carry leg E's production bases: an eight-slot `PCTR` bank **armed once**, immediately before leg E's
kick, and left **running** across E/F/G/H, plus a bit-by-bit `INT_STS` decode on each of those legs. The
bank is never re-armed between legs — a re-arm clears the counter file, and a clear between two legs that
are supposed to differ only in their list is a second variable. Each leg prints its **raw** cumulative
words beside the **delta** this code subtracted, per the instrument law.

| slot | src | `enum v3d_perfcnt` member | what it watches |
|---|---|---|---|
| 0 | 1 | `FEP_VALID_PRIMS` | front-end primitive intake |
| 1 | 14 | `QPU_ACTIVE_CYCLES_VERTEX_COORD_USER` | the coord thread executing |
| **2** | **32** | `CYCLE_COUNT` | **RESERVED** — `V3D63_CTRL_SLOT`, the `[v3d55]` clkliv slot, and this bank's own control |
| 3 | 16 | `QPU_CYCLES_VALID_INSTR` | any QPU instruction issue |
| 4 | 58 | `L2T_VCD_READS` | the VCD's attribute fetches, through L2T |
| 5 | 26 | `VPM_VDW_STALL` | VPM write-side stall |
| 6 | 27 | `VPM_VCD_STALL` | VPM VCD-side stall |
| 7 | 35 | `PTB_PRIMS_BINNED` | PTB intake — the first thing downstream of VPM that can move |

**Sourcing, under §38's rail.** Every id is transcribed from `include/uapi/drm/v3d_drm.h` on the build
host (`/run/host/usr/src/kernels/7.1.8-200.fc44.x86_64/…`), the anonymous `enum { V3D_PERFCNT_* }` whose
first member `V3D_PERFCNT_FEP_VALID_PRIMTS_NO_PIXELS` is index 0 and whose comment scopes these indices
to V3D 4.2 — this hardware. The enumerator list is **contiguous** from `v3d_drm.h:623` to `:709`, so
index `== line − 623`, and each id's line is cited in code beside its constant. §38's six-for-six index
cross-check was **re-run against this newer copy and is still six for six** (14, 16, 17, 24, 25, 32 all
land on their existing names). §38's standing caveat rides with them unchanged: on this silicon the 7-bit
`SRC` field does not select the enum's source for every id, so **`id↔mux` validity is partial and per
id** — a nonzero slot is strong, a zero slot is weak. That is exactly why legs E and F are read on the
same bank: a slot flat on E/F and moving on G moved *because of* `VERTEX_ARRAY_PRIMS`, and a slot moving
on E is not measuring what its name says here.

**Two corrections this arc records rather than absorbs.**

1. §49.25.4 asks for a **"VPM writes"** source. **There is no `V3D_PERFCNT_VPM_*_WRITES` member in this
   header.** `grep VPM` over the enum returns exactly two members, 26 `VPM_VDW_STALL` and 27
   `VPM_VCD_STALL`, both *stall* counters. They are armed as the VPM-side channel and named for what
   they are; the delivery-arrival question is carried by 35 `PTB_PRIMS_BINNED` instead. Same treatment
   §39 gave the brief's non-existent `PTB_BLOCKED_CYCLES`.
2. §38's prose cites `CLE_ACTIVE` at index **57**. Counted against this copy it is **55**
   (`v3d_drm.h:678`). Nothing in the tree arms it, so nothing is affected; the number is corrected here
   so the next arc does not inherit it.

**`INT_STS`, owed since boot11 and paid.** Leg G read `0x00010000` and no line named it. The decode is
not new knowledge — `V3D_INT_QPU_MASK`/`SHIFT` have been in the file since PI-V3D-45 — it is a line that
*says* it, bit by bit, off the word `wait_fldone` already returned. `0x00010000` is **bit16: QPU 0 raised
a host interrupt**, the QPU's own `sig.int` / program-end-interrupt path. That is a **positive**
statement that a thread ran and ended, from an instrument the counter file cannot influence, and it is
not an error bit — so leg G's freeze is not a latched fault. It also sets up the sharpest reading in the
rung: if `INT_STS` bit16 is set while the QPU cycle slots read delta 0, the two instruments **disagree**,
and the disagreement is the finding.

**The outcomes, pre-written verbatim on leg G's own `[v3d101] DISPATCHDISC VERDICT` line.** Two
INCONCLUSIVE guards are taken first — the bank lost its enable mask inside the window, or slot 2's
control never moved — and neither yields a D row.

| the leg reads | verdict | conclusion |
|---|---|---|
| every counter flat, FEP included, on a leg whose `CT0PC` counted 3 | `OUTCOME D4` | the CLE counted primitives it **never forwarded**. `CT0PC`'s own meaning is the finding: a list-parse counter, not a dispatch counter, and every reading that treated "CT0PC advanced" as "the primitive dispatched" must be re-read. Weakest of the four rows under §38's caveat — check E/F first |
| QPU cycles delta 0, something upstream moved | `OUTCOME D1` | **the coord thread never launches.** The wall is thread dispatch itself — shader-record fetch or the thread-start/TSDA path — upstream of anything the shader could do wrong, which retires shader content as a suspect. Cross-read against the `INT_STS` QPU vector |
| QPU moved, VPM and PTB flat | `OUTCOME D2` | the thread runs and delivers nothing — and on the NULL-shader leg **that is correct behaviour**, so **D2 is explicitly not a wall verdict**; it must be re-run against leg H's real shader. What it does buy is large: dispatch reaches the QPU, D1 is dead, and the dark span shrinks to VPM write-back and PTB intake |
| VPM and/or PTB moved, **pool words 0** | `OUTCOME D3` | **the PTB receives and discards** — the wall is PTB intake. Slot 7 against slots 5/6 splits it further: prims-binned moving with a zero pool is the narrowest form; prims-binned flat with only the VPM stalls moving puts the wall one station earlier, at the PTB's acceptance of the hand-off |
| counters moved **and** the pool took words, frame still open | — | no §49.25.4 row fits. A new station; name it from the raw words before reusing any earlier row |

**Scope and family law.** The only registers written are the `PCTR` counter-file config registers
mainline's `v3d_perfmon.c` writes on every perfmon start, in that file's own start/stop idiom (EN=0 →
SRC → PI-V3D-39 read-back → CLR while stopped → OVERFLOW clear → EN last; stop is EN=0). No CLE, PTB,
VCD, MMU or fabric state is touched, no page table is edited, no mailbox tag is sent, no `BPOA` is
written, and the arena geometry is unchanged. The boot returns before `probe_job`, so the knob can never
sit beside `[v3d75]`'s `ENABLE_QPU` or `[v3d80]`/`[v3d81d]`'s `DISPLAY_DONE` sends. **§49.20.2's V2a law
is strengthened in code:** every counter read on every leg happens strictly *after* that leg's read-once
violation pair, and the sample is taken before the leg's own L2T flush so the counters describe the kick
and not our cache maintenance. A `[v3d101] CORRECTION TO ALL FIVE LINES ABOVE` rides in front, and it
says the one thing a reader could get wrong: this knob adds **no leg**.

**Measured, this arc** (worktree `unaos-wt-exec-v3d101`, baseline `b9ae9112`):

| gate | result |
|---|---|
| `./arroyo check` | green, both arches; `arm-pi` green with `v3d_dispatchdisc` appended to its feature list, `kernel cfg coverage OK (12 legs)` |
| `UNAOS_WC=1 ./arroyo check` | green, both arches |
| knob-off `./arroyo kernel8-test 210` | **MBENCH PASS — 117/117 required witnesses, 0 forbidden hit(s), 20766 lines scanned**, first run, quiet host |
| knob-off byte-identity, `UNAOS_PI=1 ./arroyo kernel8` | arc-applied `3a280f9dcbb32145…` **==** baseline `3a280f9dcbb32145…` (baseline built in this worktree at `b9ae9112` before any edit; no stash, per `CLAUDE.md`) |
| armed build banner | all **seven** knobs echo: `UNAOS_V3D`, `UNAOS_V3D_UNARMCLOSE`, `UNAOS_V3D_BASEDAIM`, `UNAOS_V3D_TSAIM`, `UNAOS_V3D_ARMEDCLOSE`, `UNAOS_V3D_BINCONTENT`, `UNAOS_V3D_DISPATCHDISC` |
| implication proved by image | `UNAOS_V3D_DISPATCHDISC=1` **alone** builds `e880ae8366466c8f…` — the same digest as the full seven-knob line, so the chain is armed by the feature and not by the operator |
| strings-proof, armed image `e880ae8366466c8f…` | `v3d101] CORRECTION TO ALL FIVE` 1 · `v3d101] PCTRARM` 1 · `v3d101] PCTRSTOP` 1 · `v3d101] DISPATCHDISC (` 1 · `v3d101] DISPATCHDISC VERDICT` 1 · `v3d101] INT_STS DECODE` 1 · `v3d101] BASELINE` 1 · `THE COORD THREAD NEVER LAUNCHES` 1 · `THE THREAD RUNS AND DELIVERS NOTHING` 1 · `THE PTB RECEIVES AND DISCARDS` 1 · `EVERY COUNTER delta 0` 1 · `THE BANK WAS NOT FULLY ARMED` 1 · `SLOT 2 (src32 CYCLE_COUNT) IS FLAT` 1 · `ROW FITS` 1 · `FEP_VALID_PRIMS` 1 · `L2T_VCD_READS` 1 · `VPM_VDW_STALL` 1 · `VPM_VCD_STALL` 1 · `PTB_PRIMS_BINNED` 2 · `per-QPU host-interrupt vector` 2 |

*Scope of the byte-identity claim.* A `UNAOS_V3D_BINCONTENT=1` build is **not** claimed byte-identical
across this arc — the shared leg gained a cfg-gated sample and two emit calls. What is claimed and gated
is that **every build that does not arm `v3d_dispatchdisc` is byte-identical to the pre-arc build**,
which covers every boot that is not this experiment.

**What the sitting needs.** One image, image name **PA49**:

```
UNAOS_V3D=1 UNAOS_V3D_UNARMCLOSE=1 UNAOS_V3D_BASEDAIM=1 UNAOS_V3D_TSAIM=1 UNAOS_V3D_ARMEDCLOSE=1 UNAOS_V3D_BINCONTENT=1 UNAOS_V3D_DISPATCHDISC=1 UNAOS_PI=1 ./arroyo kernel8
```

A **cold** power-cycle, as boot4/7/8/9/11 were. One short capture, labelled, never diffed line-for-line
against a deep boot. **Read in this order:** the five `CORRECTION` lines, `[v3d101] PCTRARM` (which slot
carries which source), then for each of legs E, F, G, H in turn its `[v3d101] INT_STS DECODE` and its
`[v3d101] DISPATCHDISC` delta line — E and F are the **baselines** and carry no outcome row — and finally
leg G's `[v3d101] DISPATCHDISC VERDICT`. Leg H remains stood down until G closes; the discriminator is
measured on the leg that fails, not the leg that would.

*Fold discipline note:* boot11 flew and was read the same sitting; this fold was written by the
successor session (pi 1) from the capture per the baton's explicit no-folds-at-close order — the
capture, not this prose, is the evidence of record.

#### 49.25.5 `hfirst` (PI-V3D-102) — run leg H first, because leg G can never let it run

This records the rung as **built**, on the terms §49.25.4a recorded its predecessor's: a **design note,
not a verdict**. QEMU raspi4b models no V3D at all, so nothing below has been observed; the gates are
compile, presence-in-artifact and knob-off byte-identity, and the reading rules are written *before* the
boot that will decide them.

**What boot 12 established, and the trap inside it.** Leg G returned `OUTCOME D2`: QPU cycles moved
(**5**, with `INT_STS` bit16 — QPU 0's program-end host interrupt — set), and `FEP=0 VCD=0 VPM=0 PTB=0`
with `FLDONE=0` and the frame never closing. Legs E and F, which feed **no** primitives, both closed with
`FLDONE=1`. §49.25.4 pre-wrote D2 as *explicitly not a wall verdict*, and that pre-writing paid: leg G's
coord shader is the **NULL shader**, a real dispatching thread that writes nothing to VPM **by
construction**. A thread that runs and delivers nothing is therefore exactly what healthy silicon does
here. **The wall is an owed VPM delivery** — the binner's flush waits forever for a delivery the null
shader was never going to make.

That is correct hardware behaviour, and it is also a **trap in the ladder's own structure**. §49.24 chose
the order F → G → H because each leg is a strict superset of the one before it and each is likelier to
stall, so a stall lands as late as possible. Boot 12 turned that virtue into a dead end:

- leg G leaves the bin frame **open, every time**, for a reason that is not a defect and will not
  go away;
- the CT0-hygiene gate (§49.3's founding lesson — never measure a leg behind an unclosed frame)
  therefore stands **leg H** down, every time;
- **leg H is the decisive experiment** — the m4-class minimum over the **real** coordinate shader, the
  one that **does** write VPM — and **it has never run**, on any boot in this campaign.

The ladder is structurally incapable of reaching its own decisive leg. Boot 13 fixes that, and it is the
only thing boot 13 changes.

**The change, stated at its narrowest.** `UNAOS_V3D_HFIRST=1` reorders the content legs to
**E → H → F → G**. It adds **no leg, no kick and no instrument**: the boot still takes eight CT0 closes,
every leg's list, prologue, eleven referenced regions, digests and poison collars are byte-for-byte what
`v3d_bincontent` builds, and the `[v3d101]` `PCTR` bank is still armed once before leg E and read across
all four production legs. The feature implies `v3d_dispatchdisc` — leg H's rows are stated against the
bank, so the bank must be present to state them. In code the whole arm is **two `#[cfg]`'d array
literals**: the same three tuples, in two orders.

**The hygiene gate is mirrored, not weakened.** The ladder loop is untouched — after each leg it
re-checks `ct0_clean()` and stands the rest down. Armed, that means **F and G stand down if H hangs**, by
exactly the rule that stood H down before. No protection is relaxed to buy this boot; the gate simply now
guards in the direction that lets the decisive leg through. `[v3d101]`'s own emitters are likewise
untouched: leg H keeps its `[v3d101] INT_STS DECODE`, its `[v3d101] DISPATCHDISC` delta line and its
`[v3d101] BASELINE` line unchanged, and the `[v3d102]` verdict is printed **after** them, from its own
cfg-gated statement at the call site.

*One wire line reads inverted on this boot, and says so.* Leg H's `[v3d101] BASELINE` sentence was
written for a ladder where H ran *behind* G. The `[v3d102] CORRECTION` header and the
`[v3d102] HFIRST COMPLETE` line both name this explicitly: a slot that moves on F or G but not on H says
the **opposite** of what §49.25.4's baseline sentence says, because the order it describes is the one
this knob inverted.

**The four outcomes, pre-written on leg H's own `[v3d102] HFIRST VERDICT` line.** They split **first on
whether the frame CLOSED** — the one fact boot 12 never produced — and only then on the counters. Two
INCONCLUSIVE guards are taken before any of them (the bank lost its enable mask inside the window, or
slot 2's `src32 CYCLE_COUNT` control never moved), and neither yields an H row.

| leg H reads | verdict | what it means for the campaign |
|---|---|---|
| frame **CLOSED**, pool words **> 20** | `OUTCOME H1` | **content flows — the wall dissolves.** A real coordinate shader's VPM delivery reached the PTB and the PTB wrote per-tile primitive-list bytes. The wall *was* the null shader's owed delivery, and it was never a defect. The bin stage is then **whole end to end** for the m4-class minimum, and the campaign's last room opens: **the render list** |
| frame **CLOSED**, pool words **== 20** (the empty close) | `OUTCOME H2` | **binned but empty.** The freeze in its boot-12 form is dead — the vertex path *delivers* — but the binner bins nothing. No longer a dispatch question: a **clipper/state** question. PI-V3D-17 is the first thing to re-read (at POR zeros the clipper collapses every primitive to a point). Slot 7 `PTB_PRIMS_BINNED` splits it: nonzero with an empty pool narrows to the PTB's write, zero narrows to everything upstream of its intake |
| frame **open**, VPM and/or PTB **moved** | `OUTCOME H3` | **delivery happens and the close still hangs.** The flush is waiting on something *other* than VPM, which retires the cleanest reading boot 12 left standing and puts the wall **downstream** of the delivery — PTB intake, the per-tile write, or a flush condition this rung has not named. Prims-binned moving with a zero pool is `D3`'s shape arriving on the **real** shader, which would make D3 the wall verdict D2 never was |
| frame **open**, QPU **moved**, VPM and PTB **flat** | `OUTCOME H4` | **the VPM write path itself is broken.** The same delta boot 12 read on the null shader, now on a shader that writes VPM by construction — so the shader is **exonerated** and the wall is the VPM write-back stage, or the coord thread's access to it. Cross-read the QPU cycles: **more** than boot 12's 5 is the strong form; **at or near 5** means this leg ran something the size of the null shader, a finding about the *shader record* and not about VPM, and it must be settled first |

Two further guard rows keep those four honest, and neither is an outcome:

- frame **open** and the **QPU slots flat too** ⇒ **no H row.** The thread never launched, so `H4` is not
  earned — H4's whole claim is that a thread *ran* and still delivered nothing. This is `[v3d101]`'s `D1`
  shape arriving on the real shader. Boot 12's leg G *did* launch (QPU=5, bit16), so a leg H that fails
  to launch off the same production bases and the same prologue indicts the one variable: this leg's
  shader record at `OFF_SHADREC`. `INT_STS` bit16 set while the QPU slots read 0 is the two instruments
  **disagreeing**, and that disagreement outranks everything else on the line.
- frame **closed** at **fewer than 20** pool words ⇒ **no row fits.** A close that wrote *less* than the
  empty frame is a shape no leg of this rung has produced; name it from the raw words before reusing any
  earlier row.

The line also prints leg H's QPU cycle count **raw and delta on both QPU sources**, against boot 12's leg
G measurement of 5, because three of the six rows above are read differently depending on it.

**Scope and family law.** No register write, no mailbox tag and no page-table edit is added over
`UNAOS_V3D_DISPATCHDISC`; the arena geometry is unchanged; **CT0 is fed a BIN list on every leg, never a
render list** (driver law — RCLs run on CT1). The boot returns before `probe_job`, so the knob can never
sit beside `[v3d75]`'s `ENABLE_QPU` or `[v3d80]`/`[v3d81d]`'s `DISPLAY_DONE` sends. §49.20.2's V2a law
rides unchanged.

**Measured, this arc** (worktree `unaos-wt-exec-v3dhfirst`, baseline `0d865227`):

| gate | result |
|---|---|
| `./arroyo check` | green, both arches; `arm-pi` green with `v3d_hfirst` appended to its feature list, `kernel cfg coverage OK (12 legs)` |
| `UNAOS_WC=1 ./arroyo check` | green, both arches |
| knob-off byte-identity, `UNAOS_PI=1 ./arroyo kernel8` | arc `2295fe1e501fb637…` **==** baseline `2295fe1e501fb637…` |
| **dispatchdisc-armed** byte-identity, `UNAOS_V3D_DISPATCHDISC=1 UNAOS_PI=1 ./arroyo kernel8` | arc `e748aafb1a50b5ec…` **==** baseline `e748aafb1a50b5ec…` — the stronger claim, and the reason `v3d101_emit_leg`'s signature was left alone |
| implication proved by image | `UNAOS_V3D_HFIRST=1` **alone** builds `3bbc070d67c1da47…` — the same digest as the full eight-knob line, so the chain is armed by the feature and not by the operator |
| armed build banner | `UNAOS_V3D_HFIRST` echoes its knob line |
| strings-proof, armed image `3bbc070d67c1da47…` | `v3d102] CORRECTION TO ALL SIX` 1 · `v3d102] HFIRST ORDER` 1 · `v3d102] HFIRST VERDICT` 1 · `v3d102] HFIRST COMPLETE` 1 · `v3d102] LADDER STOOD DOWN BEFORE LEG H` 1 · `OUTCOME H1` 1 · `OUTCOME H2` 1 · `OUTCOME H3` 1 · `OUTCOME H4` 1 · `THE COORD THREAD NEVER LAUNCHED ON LEG H` 1 · `THE FRAME CLOSED AT A POOL COUNT NEITHER H1 NOR H2` 1 |

*Note on the strings-proof method:* the wire strings contain em-dashes (UTF-8 multibyte), which `strings` treats
as non-printable and splits on, so each row above was probed on its ASCII-only fragment. An unsplit search for
`"OUTCOME H1 — CONTENT FLOWS"` returns 0 and means nothing.

No QEMU battery was run and none is claimed: **raspi4b models no V3D at all**, so `[v3d95]` prints its
hub-absent SKIPPED line and returns before any leg is reached. Metal is the verdict; the gate here is
checks plus strings-proof plus byte-identity.

*Scope of the byte-identity claim.* **Every build that does not arm `v3d_hfirst` is byte-identical to the
pre-arc build** — including a `UNAOS_V3D_DISPATCHDISC=1` build, because `[v3d101]`'s emitters and
`v3d101_emit_leg`'s signature are untouched by this arc and the default ladder is the same array literal
`[v3d100]` shipped. That covers every boot that is not this experiment.

**What the sitting needs.** One image, image name **PA50**:

```
UNAOS_V3D_HFIRST=1 UNAOS_PI=1 ./arroyo kernel8
```

`UNAOS_V3D_HFIRST=1` **alone** is sufficient — the feature chain arms `v3d_dispatchdisc`,
`v3d_bincontent`, `v3d_armedclose`, `v3d_tsaim`, `v3d_basedaim`, `v3d_unarmclose` and `v3d` itself.

A **cold** power-cycle, as boots 4/7/8/9/11/12 were. One short capture, labelled, **never** diffed
line-for-line against a deep boot. **Read in this order:** the `[v3d102] CORRECTION` line and the
`[v3d102] HFIRST ORDER` line (which together supersede the six headers above them on the order question
only), then **leg E's verdict** — the empty control, same boot, same image, same bases, same prologue; a
leg E that did not return `OUTCOME E1` leaves leg H with no baseline and the boot decides nothing — then
**leg H's** `[v3d101] INT_STS DECODE`, its `[v3d101] DISPATCHDISC` delta line, and finally its
`[v3d102] HFIRST VERDICT`. Legs F and G run **only if H closed**; if they ran, they are `[v3d101]`
baselines sitting *behind* H rather than in front of it.

**This is the campaign's decisive boot.** Every one of the four outcomes is progress and none of them is
a null result: `H1` ends the bin-stage campaign and opens the render list; `H2` moves the question from
dispatch to the clipper; `H3` puts the wall downstream of a delivery that provably happens; `H4` convicts
the VPM write path and exonerates the shader. The leg that has never run is the leg that answers.

#### 49.25.6 `hfirst`, read on metal — leg H ran, and the shader is exonerated (PI-V3D-102 verdict, boot 13, 2026-08-18)

Capture `line-acm0/pi.log`, **v3d boot 13**, the PA50 image (`UNAOS_V3D_HFIRST=1 UNAOS_PI=1`), cold
power-cycle, read from the `[v3d101]`/`[v3d102]` lines themselves. This is the boot §49.25.5 called
"the campaign's decisive boot", and it decided.

**Leg E first, as the reading order requires.** `INT_STS raw=0x00000002` — `FLDONE` (bit1) SET, every
error bit clear. `pool words=20 tile-state words=48` — the empty close, to the word. Every production
slot flat (`FEP/QPU/VCD/VPM/PTB` all `d=0`), only the control slot moving
(`slot2 src32 CYCLE_COUNT raw=270622010`). That is **`OUTCOME E1`**, a clean baseline, and the bank
control row is intact: `PCTR_EN(at read)=0x000000ff OVERFLOW=0x00000000`. Leg H therefore has a
baseline and the boot is admissible.

**Leg H — the leg that had never run in this campaign — ran.** Its words, verbatim off the wire:

| witness | leg H | leg E (same boot, same bank, no re-arm) |
|---|---|---|
| bank control | `PCTR_EN-intact=1 src32-moved=1` | `EN=0x000000ff OVF=0x00000000` |
| `INT_STS` | **`0x00010000`** — bit16, QPU 0's program-end host interrupt; **nothing** in the frame-done or error families | `0x00000002` (`FLDONE`) |
| `FLDONE` / frame closed | **0 / no** | 1 / yes |
| slot0 `src1 FEP_VALID_PRIMS` | `raw=0 d=0` | `raw=0 d=0` |
| **slot1 `src14 QPU_ACTIVE_CYCLES_VERTEX_COORD_USER`** | **`raw=28 d=28`** | `raw=0 d=0` |
| slot2 `src32 CYCLE_COUNT` (control) | `raw=1851095553 d=1580473543` | `raw=270622010 d=270622010` |
| **slot3 `src16 QPU_CYCLES_VALID_INSTR`** | **`raw=53 d=53`** | `raw=0 d=0` |
| slot4 `src58 L2T_VCD_READS` | `raw=0 d=0` | `raw=0 d=0` |
| slot5 `src26 VPM_VDW_STALL` | `raw=0 d=0` | `raw=0 d=0` |
| slot6 `src27 VPM_VCD_STALL` | `raw=0 d=0` | `raw=0 d=0` |
| slot7 `src35 PTB_PRIMS_BINNED` | `raw=0 d=0` | `raw=0 d=0` |
| pool words / tile-state words | **0 / 0** (empty close = 20 / 48) | 20 / 48 |
| `CT0CA` / `CT0PC` | `CT0CA == EA`, `CT0PC=3` | — |
| `PCS.BMACTIVE` | 1 | — |
| MMU violation pair (V2a, read first) | clean | clean |
| referenced regions changed / poison collars | 0 of 11 / 0 | 0 of 11 / 0 |

**`OUTCOME H4` — THE REAL SHADER ALSO FAILS TO DELIVER**, and it arrives in the **STRONG FORM**.

**The strong-form cross-read, which is the whole reason boot 13 is decisive.** §49.25.5 pre-wrote H4
with a condition attached: *"more than boot 12's 5 is the strong form; at or near 5 means this leg ran
something the size of the null shader, a finding about the shader record and not about VPM, and it must
be settled first."* The condition is met with room to spare, on both QPU sources:

- boot 12's leg G, the **NULL** coord shader (4 words — `vpmwt ; nop;thrsw ; nop ; nop`, the tail of
  `CS_VS_WORDS`): `QPU_CYCLES_VALID_INSTR = 5`.
- boot 13's leg H, the **REAL** coord shader (`CS_VS_WORDS`, 27 words):
  `QPU_CYCLES_VALID_INSTR = 53`, `QPU_ACTIVE_CYCLES_VERTEX_COORD_USER = 28`.

**53 ≫ 5** — an order of magnitude, on a program that is 27 words against 4. The ratio the cycles report
(10.6×) and the ratio the programs carry (6.75×) are the same order, so the count is consistent with the
**whole** 27-word body issuing, not a truncated prefix that died at some early instruction. Boot 12's
"shader record this leg fetched" escape hatch is therefore **closed**: leg H fetched and executed the
real, big, VPM-writing program. `INT_STS` bit16 says the same thing from an instrument the counter file
cannot influence — a thread ran **and ended**. The two instruments agree, which is the case §49.25.4
named as the one that carries full weight.

And with that program provably executed, **`VPM_VDW_STALL`, `VPM_VCD_STALL` and `PTB_PRIMS_BINNED` are
all still exactly zero, the tile-alloc pool took zero words, the tile-state array took zero words, and
the frame never closed.**

##### What died with boot 13

**The last "shader content" hypothesis class is retired.** This is the class that has absorbed
PI-V3D-9, -17, -18, -19, -20, -22, -26 and -47 — every arc that asked *"is the coordinate shader
writing the wrong thing, in the wrong form, to the wrong place, or ending too early?"* Boot 13 kills the
class, not just its current member, because it holds the question **invariant under shader size**:

- a 4-word shader that writes **nothing** to VPM → frame open, VPM/PTB flat;
- a 27-word shader that writes **six components per vertex** to VPM by construction → frame open,
  VPM/PTB flat, *identically*.

The output the frame is waiting for went from zero to six-per-vertex and **not one downstream witness
moved by one count**. No content hypothesis survives that: whatever the shader writes, or does not
write, the observable is the same. The shader is **exonerated**, and every future arc that proposes to
change a QPU word in `CS_VS_WORDS` to fix the wall must first explain why boot 13's invariance is
compatible with it.

Three narrower things die with the class:

- **"Leg H would have closed"** — §49.24's ladder chose F → G → H on the premise that H was the leg most
  likely to succeed. It is not. H fails in G's exact shape.
- **`OUTCOME H1`, `H2` and `H3` are all dead for the m4-class minimum.** The frame did not close, so H1
  and H2 cannot be taken; VPM and PTB did not move, so H3 cannot. Of §49.25.5's four pre-written
  outcomes exactly one is live, and it is the one that convicts the VPM write path.
- **The two guard rows did not fire.** The QPU slots were not flat (so the `D1`-shape guard — "the
  thread never launched, H4 is not earned" — does not apply), and the frame did not close at fewer than
  20 pool words. H4 is earned cleanly, with no guard standing in front of it.

##### The wall, at its narrowest name yet

> The coordinate shader launches, executes its full 27-word body, issues six `STVPMV` stores and a
> `VPMWT`, and ends with a program-end interrupt — and **nothing on the VPM side of the machine
> registers that any of it happened.** The wall is the **VPM write path itself**: the VPM write-back
> stage, or the coord thread's **access** to it. It is not what the shader was asked to write.

Two properties of that statement are worth keeping in front of the next rung. First, it is a **silent**
failure: no fault latches, no MMU violation, no referenced region changes, no poison collar breaks, and
`INT_STS`'s error families are clear. Second, it is **not a stall in any counter we armed** — both VPM
stall counters read zero. A write path that were merely *blocked* should plausibly stall something. A
write path that reads zero on both stall counters while delivering nothing is more consistent with
writes being **accepted and discarded**, or **never presented to the port at all**, than with
back-pressure. §49.25.7 is built to split exactly that.

*Fold discipline note:* boot 13 flew and was read the same sitting; this fold was written by the
successor session from the capture, per the standing no-folds-at-close order — the capture, not this
prose, is the evidence of record.

#### 49.25.7 `vpmprobe` (PI-V3D-103) — splitting `OUTCOME H4`: is the VPM write refused, discarded, or never presented?

This records the rung as **built**, on the terms §49.25.4a/§49.25.5 recorded theirs: a **design note, not
a verdict**. QEMU raspi4b models no V3D at all, so nothing below has been observed; the gates are
compile, presence-in-artifact and knob-off/armed byte-identity, and the reading rules are written
*before* the boot that will decide them.

##### 49.25.7a Sub-hypothesis (a) is dead, and it died at the desk

The brief for this rung named three sub-hypotheses inside §49.25.6's H4 space. The first was:

> *(a) the shader's VPM WRITE SETUP is ignored or mis-targeted on this silicon — the VPMVCD write-setup
> magic-register word. The bcm2711 is V3D 4.2, NOT the VC4 VPM model; if our shader was written against
> VC4 VPM semantics, that mismatch could BE the wall, and it is checkable STATICALLY first.*

It was checked statically, and **it is dead. There is no setup word in our shader to be wrong.**

The reasoning the hypothesis rests on is *correct* — a VC4-shaped VPM setup on a 4.2 VPM would indeed be
a wall — and it is correct because **that exact failure already happened in this campaign, and was
already root-caused and fixed two years of arcs ago.** PI-V3D-9/17/18/19 wrote the coord shader's output
with the streamed VC4 / V3D-3.3 mechanism: a `vpmsetup` arming a VPM segment, then `mov vpm, rfN`
auto-advancing an implicit write pointer. **PI-V3D-20 removed it entirely**, on the finding that the
mechanism does not exist for per-vertex shader output on V3D 4.x — every one of those builds was writing
an unconfigured magic register. `CS_VS_WORDS` has carried the 4.2 form ever since: six `STVPMV` stores
with **explicit** per-component VPM offsets 0..5, delivered as uniforms into `rf9..rf14`, and **no
`vpmsetup` at all**. (`vpmsetup` *does* pack on ver 42 — opcode 187, `first_ver` 33 — but on 4.x it arms
VPM **DMA** descriptors, an unrelated channel.)

Three independent checks agree that the surviving body is 4.2-shaped:

1. **Mesa-compiled cross-check (PI-V3D-26).** The body was validated against a real `v3d_compile()` run
   at ver 4.2 configured as the driver configures a binning VS (`num_used_outputs = 0` for the
   last-geometry-stage coord shader). Mesa stores clip Xc,Yc,Zc,Wc to VPM offsets **0,1,2,3** and screen
   Xs,Ys to **4,5**, with viewport scale 32·256 = **8192** — our contract exactly.
2. **Mesa-packer round-trip (PI-V3D-20).** Every word is packed and round-tripped by Mesa's own
   `v3d_qpu_instr_pack`; `scripts/pi-v3d20-qpu-gen.out.txt` ends `All words packed + round-tripped OK`.
3. **Bit-by-bit re-decode, this arc.** All six `STVPMV` words were decoded by hand against the ver-42
   field layout rather than trusted:

   | field | bits | value | reading |
   |---|---|---|---|
   | `OP_ADD` | [31:24] | `0xF8` = **248** | `V3D_QPU_A_STVPMV` — correct, on all six |
   | `OP_MUL` | [63:58] | 15 | `V3D_QPU_M_NOP` — mul side idle |
   | `SIG` | [57:53] | 0 | no signal on the store words |
   | `ADD_A` mux | [14:12] | 6 | `MUX_A` → the **offset** comes from `RADDR_A` |
   | `RADDR_A` | [11:6] | 9,10,11,12,13,14 | `rf9..rf14`, the six offset uniforms, in order |
   | `ADD_B` mux | [17:15] | 7 | `MUX_B` → the **value** comes from `RADDR_B` |
   | `RADDR_B` | [5:0] | 0,1,2,3,7,8 | `rf0..rf3` clip, `rf7`/`rf8` screen — in order |

   The operand **order** (offset in A, value in B), the mux selects, and the register numbers are all
   correct. `vpmwt` (`OP_ADD` 187) sits ahead of the terminal `thrsw`, per GFXH-1684.

**Conclusion, stated loudly because it redirects the whole rung:** our coordinate shader is
**4.2-shaped, not VC4-shaped**. Sub-hypothesis (a) cannot be the wall, no slot on boot 14 is spent on
it, and the boot says so itself on a `[v3d103] VPMSTATIC` line so no reader spends the sitting
re-deriving it. What (a)'s death buys is that §49.25.6's exoneration of the shader now covers its
*addressing form* as well as its *content* — the wall is downstream of every choice the shader makes.

##### 49.25.7b What survives, and what was found while checking

Two sub-hypotheses remain, and one useful negative came out of the static pass:

- **(b) the coord thread lacks VPM access or allocation.** Live, and now sharply instrumented — see
  below. One finding narrows it before the boot: **the VCM allocation is not missing.** The bin CL does
  emit `VCM_CACHE_SIZE` (packet 71) with `binning = rendering = 4`, computed per Mesa's
  `CLAMP(vpm_output_batches - 1, 2, 4)` at PI-V3D-23, and the shader record does carry
  `cs_output_vpm_segment_size = 1` / `cs_input_vpm_segment_size = 0` per Mesa's `v3d_vs_set_prog_data`
  fold at PI-V3D-25. So the three allocation numbers are **present and Mesa-shaped**. What has *never*
  been done is checking them against the part: this rung reads the VPM's physical size off
  `V3D_CTL_IDENT1` and prints all three side by side for the first time.
- **(c) the write happens but the VPM→PTB handoff is dead.** Live. Partially instrumented this rung
  (slots 5/6 below settle whether the PTB ever *received* anything); its decisive form — the TMU
  landmark — is **designed but deliberately not built**, for a reason recorded in §49.25.7e.

**A negative worth recording: V3D 4.2 exposes no ARM-side VPM allocation/reservation register.** The
brief asked for "VPMBASE / QUEUE config" from the 4.2 spec. There is no such register in the 4.2 core
map — VPM allocation for the vertex pipe is driven **entirely** by the shader record's segment-size
fields and the CL's `VCM_CACHE_SIZE`, both of which are memory the driver writes, not registers it
programs. The only VPM state the ARM can read is `V3D_CTL_IDENT1`'s `VPM_SIZE` field (the physical
size), and the only VPM *behaviour* it can read is the error block. Both are used here; nothing was
invented to fill the gap.

##### 49.25.7c What was built — no leg, no kick, no shader word, no register write

The boot still takes **eight** CT0 closes, runs `v3d_hfirst`'s **E → H → F → G** order, feeds CT0 a
**BIN** list on every leg and never a render list, and returns before `probe_job`. Legs A–D take none of
this and their register traffic is unchanged byte for byte. Two instrument-only changes:

**1 — Three PCTR slots re-sourced.** Slots 0/1/2/3/7 are **unchanged**: they are the five boot 13 read
`OUTCOME H4` off, and leg H must reproduce their **shape** here before any new slot is allowed to speak.
Slots 4/5/6 all read zero on boot 13 and had nothing left to give — `L2T_VCD_READS` because PI-V3D-28
already exonerated attribute fetch by direct readback, and the two VPM *stall* counters because **a
stall counter reading zero cannot separate "the write sailed through" from "no write was ever
presented"**, which is precisely the ambiguity §49.25.6 flagged.

| slot | src | boot 13 | boot 14 | what the new one watches |
|---|---|---|---|---|
| 0 | 1 | `FEP_VALID_PRIMS` — read 0 | **unchanged** | continuity guard |
| 1 | 14 | `QPU_ACTIVE_CYCLES_VERTEX_COORD_USER` — read **28** | **unchanged** | continuity guard |
| 2 | 32 | `CYCLE_COUNT` | **unchanged** | RESERVED control (`V3D63_CTRL_SLOT`) |
| 3 | 16 | `QPU_CYCLES_VALID_INSTR` — read **53** | **unchanged** | continuity guard |
| **4** | 58 → **33** | `L2T_VCD_READS` — read 0 | **`QPU_CYCLES_STALLED_VERTEX_COORD_USER`** | did the coord thread **stall** while it ran? |
| **5** | 26 → **11** | `VPM_VDW_STALL` — read 0 | **`PTB_PRIM_CLIP`** | did the PTB **clip** the primitives? |
| **6** | 27 → **10** | `VPM_VCD_STALL` — read 0 | **`PTB_PRIM_VIEWPOINT_DISCARD`** | did the PTB **discard** them at the viewport? |
| 7 | 35 | `PTB_PRIMS_BINNED` — read 0 | **unchanged** | continuity guard |

**Slots 5 and 6 are the pair boot 13 most needed and did not have.** `PTB_PRIMS_BINNED = 0` alone cannot
separate *"the PTB never received a primitive"* from *"the PTB received it and threw it away"* — and a
primitive **cannot be clipped or viewport-discarded unless it arrived**. Either of them moving therefore
proves the VPM delivery reaches the PTB, which **kills `OUTCOME H4`** and moves the wall into the
clipper/viewport room PI-V3D-17 opened. Slot 4 splits the other direction: against boot 13's `ACTIVE=28`,
a stall delta near zero says the thread ran clean and its stores were *accepted and discarded*, while a
large one says it spent its life blocked and `INT_STS` bit16's "program end" needs re-reading as a thread
*released* rather than one *finished*.

Every id is transcribed from `include/uapi/drm/v3d_drm.h` on the build host under §38's `index == line −
623` rule, re-verified this arc against the 7.1.8 copy; every id the file already carried re-lands on its
own name in the same pass, so §38's cross-check is re-run and still clean. §38's standing caveat rides
unchanged: **`id↔mux` validity is partial and per id, so a nonzero slot is strong and a zero slot is
weak** — which is exactly why the three new slots are read against leg E on the same bank, and why every
row below that turns on a *zero* is written as the weakest form of its claim.

**2 — Four read-only registers join the readback set.** `V3D_ERR_STAT` (`0x0f20`), `V3D_ERR_FDBGO`
(`0x0f04`), `V3D_ERR_FDBGB` (`0x0f08`), `V3D_ERR_FDBGS` (`0x0f10`).

*Sourcing, against this arc's "a new register only if the 4.2 spec names it" constraint.* All four are
transcribed verbatim from `drivers/gpu/drm/v3d/v3d_regs.h`, and their applicability to **this** silicon
is not inferred — mainline states it. `v3d_debugfs.c`'s `v3d_core_reg_defs[]` lists all four as
`REGDEF(V3D_GEN_33, V3D_GEN_71, …)`: valid from V3D 3.3 through 7.1 inclusive, a range that brackets the
bcm2711's **V3D 4.2**. They are **read-only status registers** whose only use in mainline is a debugfs
dump, they sit inside the core window this file already maps, and **this file never writes them.** That
is the whole safety argument. `V3D_CTL_IDENT1` is decoded too, and is not a new read at all — the file
already reads it for the revision gate.

*Why this block.* §49.25.6 left the wall at "the VPM write path itself — the write-back stage, or the
coord thread's **access** to it". **Twelve of `V3D_ERR_STAT`'s sixteen named bits are that sentence's
vocabulary:**

| bits | class | members |
|---|---|---|
| 9..4 | **VPM** | `VPMEAS` (allocated-size), `VPMEFNA` (free non-allocated), **`VPMEWNA` (write non-allocated)**, `VPMERNA` (read non-allocated), `VPMERR` (read range), **`VPMEWR` (write range)** |
| 3..0 | **VPA** | `VPAERRGL`, `VPAEBRGL`, `VPAERGS`, `VPAEABB` — vertex-pipe assembly, *upstream* of the VPM |
| 15..10 | **PIPE** | `L2CARE`, `VCMBE`, `VCMRE`, `VCDI`, `VCDE`, `VDWE` — the stations either side of the VPM |

`VPMEWNA` or `VPMEWR` latching would convict the **access** half of §49.25.6's sentence *by name*.

The block is read **pristine once before leg E's kick** and once per armed leg. `V3D_ERR_STAT` **latches**
and this file never clears it, so a bit set at the arm was set by something earlier in the boot and is not
this ladder's finding — **every row splits on newly-latched bits (`leg & ¬pre-arm`)**, not on cumulative
ones. The three `FDBG` words are printed **raw with no decode claimed**: they are there so a nonzero one
can be chased, not read. Sampling sits in the same window as `[v3d101]`'s: strictly **after** the
§49.20.2 V2a violation pair, strictly **before** the leg's L2T flush.

**Leg E is the control for the ERR block too, and that is new.** A bit that latches on leg E — a leg that
*closes* — is not this campaign's wall and must be subtracted by hand before any V row is believed.

##### 49.25.7d The outcomes, pre-written on leg H's own `[v3d103] VPMPROBE VERDICT` line

This rung **does not replace `OUTCOME H4` — it splits it.** H4 said *the VPM write path is the wall*;
these rows say *which way*. **Three guards are taken first, and none yields a V row:**

- the bank lost its enable mask, or slot 2's `src32 CYCLE_COUNT` never moved ⇒ INCONCLUSIVE;
- **the continuity slots did not reproduce boot 13** (FEP flat, QPU moved, `PTB_PRIMS_BINNED` flat)
  ⇒ INCONCLUSIVE, and it **outranks every V row**: the new slots would be measuring a leg that is not
  the leg §49.25.6 folded, and the boot decides nothing until that is explained;
- the frame **closed**, or the QPU slots are **flat** ⇒ no V row fits (the first is a bigger finding
  than this rung was built for; the second is `D1`'s shape and indicts the shader record at
  `OFF_SHADREC`).

| leg H reads | verdict | what it means for the campaign |
|---|---|---|
| a **VPM WRITE** bit newly latched (`VPMEWNA` or `VPMEWR`) | **`OUTCOME V1`** | **the VPM write is REFUSED, and the silicon says so.** §49.25.6's "or the coord thread's **access** to it" half, convicted by name: the `STVPMV` stores land outside any VPM region the thread is entitled to write. **The wall is ALLOCATION**, and the three numbers that allocate are `cs_output_vpm_segment_size` (=1), `cs_input_vpm_segment_size` (=0) and `VCM_CACHE_SIZE` (=4/4) — read them against the `VPMSIZE` line's physical KB. Strongest row in the rung; it ends the guessing |
| a **non-write VPM** bit newly latched (`VPMEAS`/`VPMEFNA`/`VPMERNA`/`VPMERR`), both write bits clear | **`OUTCOME V2`** | **the VPM is reached and unhappy, but not about our stores.** The fault is in how the block is *sized* or *reclaimed* around the thread, not in the thread's own access. The four bits mean four different things — name the exact one off the decode before reasoning further |
| a **VPA** or **PIPE** bit newly latched, **no** VPM bit | **`OUTCOME V3`** | **the error is BESIDE the VPM, not in it**, and §49.25.6's wall statement is *misnamed*. `VDWE` moves it to the VPM DMA writer; `VCMBE` to the binner's vertex-cache manager; a VPA bit puts it in primitive assembly **upstream** of the VPM — in front of the write rather than behind it |
| `PTB_PRIM_CLIP` and/or `PTB_PRIM_VIEWPOINT_DISCARD` **moved**, no error bit | **`OUTCOME V4`** | **the PTB received the primitives and threw them away — and `OUTCOME H4` IS DEAD.** A primitive cannot be clipped or viewport-discarded unless it *arrived*, so the VPM delivery **happens**. §49.25.6 must be re-folded: the wall is **clipper/viewport state**, PI-V3D-17's room, where POR zeros collapse every primitive to a point. The row that most changes the campaign, and the reason these two slots were worth boot 13's stall counters |
| `QPU_CYCLES_STALLED_VERTEX_COORD_USER` **moved**, no error bit, no PTB slot | **`OUTCOME V5`** | **the coord thread spent its life stalled.** It ran, but it was *waiting*, and nothing downstream ever took what it offered — the write path is not refusing loudly, it is not draining at all. Cross-read the stall delta against slot 1's ACTIVE delta: stall ≳ active means mostly blocked, and `INT_STS` bit16 then reads as a thread **released**, not one **finished** |
| frame open, thread ran, **did not stall**, **no** error bit, **no** PTB slot moved | **`OUTCOME V6`** | **the write is swallowed in silence.** Every instrument this campaign owns reports nothing on a leg that provably executed six `STVPMV` stores and a `VPMWT`. Not a refusal and not a stall: a path that **accepts and discards**, or one the stores were **never presented to**. Those two are no longer separable by any counter or status bit in the core block — the next rung must make the thread's stores **visible outside the VPM**, which is §49.25.7e |

##### 49.25.7e Sub-hypothesis (c): the TMU landmark, designed and deliberately NOT built

(c)'s decisive form is the brief's own: a coord-shader variant that does a **TMU write to a DRAM
landmark alongside its VPM writes** — TMU-lands + VPM-flat convicts the VPM port specifically, TMU-also-
flat moves the wall to the thread's store path generally. It is **designed here and not built**, and the
reason is a confound, not effort:

- The only **Mesa-compiled** TMU-storing coord program in the tree is `PROBE_WORDS` (PI-V3D-26's "PROBE
  VS"), and it carries **its own unresolved dispatch history**: it is a `threads=2`, multi-segment
  program with a **mid-shader `thrsw`**, and PI-V3D-36 read it at `valid_instr = 0` — *never dispatched*
  — on the same boot the M4 coord shader read 55. PI-V3D-39 ultimately repointed the probe record's CS
  slot away from it, at `CS_VS_WORDS`. A leg built on `PROBE_WORDS` would confound **"the store path is
  dead"** with **"`PROBE_WORDS` never dispatched again"**, which is exactly the ambiguity this rung
  exists to remove.
- Building the TMU landmark into `CS_VS_WORDS` instead — the known-dispatching program — would require
  **hand-authoring QPU words**, which §5's fabricated-constant law forbids without a Mesa-packed
  artifact. **The V3D-26 harness checkout is deleted**, so it cannot be re-run in place.
- There is also a ladder-order problem worth recording now: a leg T placed *behind* leg H can never run,
  because H hangs and the CT0-hygiene gate stands the rest down — the same trap §49.25.5 had to invert
  the ladder to escape. A leg T must run **directly after leg E**, in front of H.

**The blocking artifact, named so the next arc does not rediscover it:** a **Mesa-compiled, bin-mode
(`threads=4`, single-segment, no mid-shader `thrsw`) coordinate shader that stores to an SSBO **and**
writes the six-component VPM output**. Regenerating it — a fresh `v3d_compile()` harness at ver 4.2, as
`scripts/pi-v3d26-mesa-compile.c` did — is the **first** task of the rung after this one, and it is
gated on boot 14 returning `OUTCOME V6`. If boot 14 returns V1, V2, V3 or V4, the TMU landmark is **not
needed at all** and that work is saved.

##### 49.25.7f Scope and family law

No register **write**, no mailbox tag and no page-table edit is added over `UNAOS_V3D_HFIRST`. The
reply-less mailbox tag hazard is untouched. The arena geometry is unchanged. **CT0 is fed a BIN list on
every leg, never a render list** (driver law — RCLs run on CT1). The boot returns before `probe_job`, so
the knob can never sit beside `[v3d75]`'s `ENABLE_QPU` or `[v3d80]`/`[v3d81d]`'s `DISPLAY_DONE` sends.
§49.20.2's V2a law rides unchanged: every new read on every leg happens strictly *after* that leg's
read-once violation pair and before its L2T flush. The only register traffic this arc adds is **four
read-only loads per armed leg plus one pristine set**, and the three changed `SRC` fields inside the
counter-file arming `[v3d101]` already performs.

##### 49.25.7g Measured, this arc

Worktree `unaos-wt-exec-v3dvpm`, baseline `b0bb563c`.

| gate | result |
|---|---|
| `./arroyo check` | **green, both arches**; `arm-pi` green with `v3d_vpmprobe` appended to its feature list; `kernel cfg coverage OK (12 legs)` |
| `UNAOS_WC=1 ./arroyo check` | **green, both arches**; `kernel cfg coverage OK (12 legs)` |
| armed build, `UNAOS_V3D_VPMPROBE=1 UNAOS_PI=1 ./arroyo kernel8` | **green** — image `7cb050201f77cdf1…` |
| armed build banner | `⚡ kernel features: baremetal,skip_xhci,`**`v3d_vpmprobe`**`,v3d_hfirst,v3d_dispatchdisc,v3d_bincontent,v3d_armedclose,v3d_tsaim,v3d_basedaim,v3d_unarmclose,v3d` |
| implication proved by image | `UNAOS_V3D_VPMPROBE=1` **alone** builds `7cb050201f77cdf1…` — the **same** digest as the full nine-knob line, so the chain is armed by the feature and not by the operator |
| knob-off byte-identity | arc `e52b17e7a48ab0b3…` **==** baseline `e52b17e7a48ab0b3…` |
| knob-off `./arroyo kernel8-test 210` | **MBENCH PASS — 117/117 required witnesses, 0 forbidden hit(s), 13726 lines scanned** (a property of an image byte-identical to the baseline's, per the row above) |
| **`dispatchdisc`-armed** byte-identity | arc `06b2c1e9ce75fd9f…` **==** baseline `06b2c1e9ce75fd9f…` |
| **`hfirst`-armed** byte-identity | arc `d86b8bd760d2aa4a…` **==** baseline `d86b8bd760d2aa4a…` — the strongest of the three, and the one that cost this arc a code move (below) |
| strings-proof, armed image `7cb050201f77cdf1…` | `v3d103] CORRECTION TO ALL SEVEN` 1 · `v3d103] VPMSTATIC` 2 · `v3d103] VPMSIZE` 3 · `v3d103] ERRSTAT` 5 · `v3d103] VPMPROBE VERDICT` 3 · `v3d103] VPMPROBE COMPLETE` 1 · `OUTCOME V1` 1 · `OUTCOME V2` 1 · `OUTCOME V3` 1 · `OUTCOME V4` 1 · `OUTCOME V5` 1 · `OUTCOME V6` 1 · `THE VPM WRITE IS REFUSED` 1 · `A VPM ERROR LATCHED, BUT NOT A WRITE ONE` 1 · `THE ERROR IS BESIDE THE VPM, NOT IN IT` 1 · `THE PTB RECEIVED THE PRIMITIVES AND THREW THEM AWAY` 1 · `THE COORD THREAD SPENT ITS LIFE STALLED` 1 · `THE WRITE IS SWALLOWED IN SILENCE` 1 · `THE CONTINUITY SLOTS DID NOT REPRODUCE BOOT 13` 1 · `SUB-HYPOTHESIS (a) IS ALREADY DEAD` 1 · `QPU_CYCLES_STALLED_VERTEX_COORD_USER` 3 · `PTB_PRIM_VIEWPOINT_DISCARD` 3 · `PTB_PRIM_CLIP` 3 · `VPMEWNA` 3 · `VPMEWR` 3 · `VPMEAS` 2 · `VDWE` 2 · `VCMBE` 2 · `VPAEABB` 1 · `VPM_SIZE(bits31:28)` 1 |

*Note on the strings-proof method*, inherited from §49.25.5: the wire strings contain em-dashes (UTF-8
multibyte), which `strings` splits on, so each row was probed on an ASCII-only fragment. Counts above 1
are the same fragment appearing in more than one format string, not duplicates.

*A byte-identity finding worth keeping.* The `hfirst`-armed identity **failed on first measurement** —
ten bytes, same image size, zero string differences. They decoded as **four `core::panic::Location`
structs**: `{ file: &str(ptr,len), line: u32, col: u32 }`, with the file pointer and length identical
(`0x25` = 37 = `crates/kernel/src/arch/aarch64/v3d.rs`) and only the **line numbers** shifted, by exactly
the 82 lines this arc had inserted above them. Panic-location line numbers are the mechanism by which an
"instrument-only, fully cfg-gated" arc can still move a binary it never intended to touch. The fix was to
**move the whole `[v3d103]` register-constant block from line ~660 down beside the `[v3d103]` code**, so
no insertion sits above a surviving `Location` — which restored all three identities exactly and is
better cohesion besides. **Any future arc claiming byte-identity should diff with `cmp -l`, not just
compare digests: a digest mismatch of this kind is metadata, and chasing it to zero is cheap.**

No QEMU battery is claimed as a verdict: **raspi4b models no V3D at all**, so `[v3d95]` prints its
hub-absent SKIPPED line and returns before any leg is reached. Metal is the verdict; the gate here is
checks plus armed build plus strings-proof plus byte-identity.

##### 49.25.7h What the sitting needs

One image, image name **PA51**:

```
UNAOS_V3D_VPMPROBE=1 UNAOS_PI=1 ./arroyo kernel8
```

`UNAOS_V3D_VPMPROBE=1` **alone** is sufficient — the feature chain arms `v3d_hfirst`,
`v3d_dispatchdisc`, `v3d_bincontent`, `v3d_armedclose`, `v3d_tsaim`, `v3d_basedaim`, `v3d_unarmclose`
and `v3d` itself, and the image digest proves it.

A **cold** power-cycle, as boots 4/7/8/9/11/12/13 were. One short capture, labelled, **never** diffed
line-for-line against a deep boot.

**Read in this order:**

1. `[v3d103] VPMSTATIC` — **first of all.** It reports desk work, not a measurement, and it says why no
   slot on this boot is spent on the shader's VPM write setup.
2. `[v3d103] VPMSIZE` — the VPM's physical KB off `IDENT1`, against the record's segment sizes and the
   CL's `VCM_CACHE_SIZE`.
3. `[v3d103] ERRSTAT (PRE-ARM)` — the pristine reference every leg's newly-latched bits are computed
   against.
4. `[v3d103] CORRECTION TO ALL SEVEN LINES ABOVE` and `[v3d101] PCTRARM` — which slot carries which
   source on **this** boot.
5. **Leg E** — its `[v3d101] INT_STS DECODE`, its `[v3d101] DISPATCHDISC` delta line, and its
   `[v3d103] ERRSTAT (E)`. A leg E that did not return `OUTCOME E1` leaves leg H with no baseline; a
   leg E that **latches an ERR bit** poisons leg H's newly-latched set and must be subtracted by hand.
6. **Leg H** — its `[v3d101] INT_STS DECODE`, its `[v3d101] DISPATCHDISC` delta line, its
   `[v3d102] HFIRST VERDICT` (which should **re-take `OUTCOME H4`** on the four continuity slots), its
   `[v3d103] ERRSTAT (H)`, and finally its **`[v3d103] VPMPROBE VERDICT`**.

**The admissibility guard outranks every V row.** If leg H's continuity slots do not reproduce boot 13's
shape, the three re-sourced slots are measuring a leg that is not the leg §49.25.6 folded, the line says
INCONCLUSIVE, and the boot decides nothing until that is explained. Legs F and G run only if H closed,
which boot 13 says it will not.

**Every one of the six outcomes is progress.** `V1` names the wall ALLOCATION and hands the next arc
three specific numbers to fix. `V2` and `V3` move it to a named neighbouring station. `V4` **kills H4**
and reopens the clipper. `V5` converts a silent wall into a measurable stall. `V6` is the only row that
buys no new station — and even it is decisive, because it retires every instrument in the core block at
once and makes §49.25.7e's TMU landmark the campaign's unambiguous next build.

#### 49.25.8 `vpmprobe`, read on metal — `OUTCOME V5`: the coord thread spends its life STALLED against a VPM that never drains (PI-V3D-103 verdict, boot 14, 2026-08-21)

Capture `boot14.log`, **v3d boot 14**, the PA51 image (`v3d-boot14-5d7ff0c8.img`, fp 1490152,
`UNAOS_V3D_VPMPROBE=1 UNAOS_PI=1`), flown on metal 2026-08-21 and read from the
`[v3d101]`/`[v3d102]`/`[v3d103]` lines themselves. Eight CT0 closes flew in `hfirst`'s
**E → H → F → G** content order; legs A–D reproduced their settled shapes (leg B's R3 control clean,
leg C's VA-0 residual, leg D's `OUTCOME D1`) and are not re-litigated here.

**The desk rows first, as §49.25.7h's reading order requires.** The `[v3d103] VPMSTATIC` line printed,
confirming on the wire what §49.25.7a settled at the desk: **sub-hypothesis (a) — the VC4-shaped VPM
setup word — is dead**, and no slot on this boot was spent on it. Then `[v3d103] VPMSIZE`, the first
physical read of the VPM this campaign has ever taken: `V3D_CTL_IDENT1 raw=0x81001422`,
**`VPM_SIZE = 8` — 8 KB of VPM on this part** — with `NSLC=2` slices, `QUPS=4` QPUs per slice,
`NTMU=1` TMU per slice, `NSEM=0`. The field is nonzero, so §49.25.7b's sizing rows are live, and the
allocation trio (`cs_output_vpm_segment_size=1`, `cs_input_vpm_segment_size=0`,
`VCM_CACHE_SIZE=4/4`) now has a real part to be read against.

**The pristine ERR reference.** `[v3d103] ERRSTAT (PRE-ARM)`, before leg E's kick:
`V3D_ERR_STAT raw=0x00001000` — **one bit already set, `VCDI` (bit12, VCD idle error)**, latched by
something earlier in the boot than this ladder and therefore, by §49.25.7c's newly-latched law, **not
this ladder's finding**. Every other named bit 0; `FDBGO=0x00000000`, `FDBGB=0x00000010`,
`FDBGS=0x00000007` recorded raw, no decode claimed.

**Leg E, the control, healthy again.** `INT_STS raw=0x00000002` (`FLDONE`, every error family clear),
`BFC` Δ1, `pool words=20 tile-state words=48` — **`OUTCOME E1`**, boot 13's baseline to the word, with
every production slot flat and only the control slot moving (`slot2 src32 CYCLE_COUNT d=475220236`).
And the new control did its new job: `[v3d103] ERRSTAT (E)` reads `raw=0x00001000` — **identical to
the pre-arm word, so leg E latched nothing**, and there is nothing to subtract by hand from leg H's
newly-latched set.

**Leg H — the admissibility guard passes, and `OUTCOME H4` is re-taken in its strong form.** The
continuity slots reproduced boot 13's shape exactly as the guard demands:
`FEP-moved=0 QPU-moved=1 PTB_PRIMS_BINNED-moved=0 shape-ok=1`, with the bank intact
(`PCTR_EN-intact=1 src32-moved=1`). The `[v3d102] HFIRST VERDICT` line re-delivered **`OUTCOME H4`**
on the bigger program: `slot1 src14 QPU_ACTIVE_CYCLES_VERTEX_COORD_USER raw=28 d=28` and
`slot3 src16 QPU_CYCLES_VALID_INSTR raw=53 d=53` against boot 12's null-shader 5 — the strong-form
condition met again, boot 13's numbers to the digit — with the VPM-side witnesses flat, the frame
never closing (`BFC 0x00000005->0x00000005 Δ0`, `retired=0`, `INT_STS=0x00010000` — bit16 alone,
QPU 0's program-end host interrupt), and the poison untouched (`decoy-pool=0/4096`,
`real-pool=0/8192`, `real-ts=0/64`). The shader is exonerated a second time, on a second flight, and
the three re-sourced slots are entitled to speak.

**And they convict. `OUTCOME V5` — THE COORD THREAD SPENT ITS LIFE STALLED.** The
`[v3d103] VPMPROBE VERDICT` line, verbatim in its load-bearing parts:

| witness | leg H, boot 14 |
|---|---|
| **slot4 `src33 QPU_CYCLES_STALLED_VERTEX_COORD_USER`** | **`raw=25 d=25` — MOVED** |
| slot5 `src11 PTB_PRIM_CLIP` | `raw=0 d=0` |
| slot6 `src10 PTB_PRIM_VIEWPOINT_DISCARD` | `raw=0 d=0` |
| `ERR_STAT` pre-arm / this-leg / **newly-latched** | `0x00001000` / `0x00001000` / **`0x00000000`** |
| newly-latched by class | `VPM-write-pair=0x0000 VPM-any=0x0000 VPA=0x0000 PIPE=0x0000` |
| frame closed / pool words | 0 / 0 (empty close = 20) |

The stall counter moved on a leg where **no error bit latched** — the pre-existing `VCDI` bit did not
change on any leg, E or H, and the newly-latched word is exactly zero — and **no PTB slot moved**.
That is `V5`'s row, taken cleanly, with every other row's trigger absent.

**The cross-read the row prescribes, performed: stalled 25 vs active 28.** The stall delta is
comparable to the active delta — the thread was **mostly blocked** for its short life — so, per the
row, `INT_STS` bit16's "program end" must be re-read as a thread that was **RELEASED rather than one
that FINISHED**. The wall's shape sharpens accordingly: the write path is **not refusing loudly — it
is not draining at all**. The thread ran its 27-word body, offered its six `STVPMV` stores, waited
out most of its cycles at the port, and nothing downstream ever took what it offered.

**What died on boot 14:**

- **`V1`, `V2` and `V3` are dead for this leg.** Zero newly-latched bits: the silicon does not call
  this an access violation, a sizing fault, or a neighbouring-station error. §49.25.6's "or the coord
  thread's **access** to it" half is *not* convicted by name — the VPM's error machinery considers the
  thread entitled to do what it is doing.
- **`V4` is dead, and with it the received-and-discarded reading.** `PTB_PRIM_CLIP d=0` and
  `PTB_PRIM_VIEWPOINT_DISCARD d=0`: a primitive cannot be clipped or viewport-discarded unless it
  arrived, and none arrived. The PTB **never received** anything — `OUTCOME H4` survives the split
  built to kill it, and the clipper/viewport room stays shut.
- **`V6` is not reached.** Its row requires "did not stall", and the thread stalled. Boot 13's
  "accepted and discarded, or never presented" ambiguity resolves toward the third reading neither
  phrase quite named: **presented and never taken**.

**Legs F and G stood down**, per the §49.3 dirty-CT0 rule, exactly as boot 13 said they would and
exactly as designed — no kick issued, no verdict implied, no information lost.

##### The next rung, stated honestly

A discipline note first: §49.25.7's pre-written material does **not** assign `V5` a named follow-up
build the way it assigns `V6` one. §49.25.7e gates the TMU landmark **on `V6`** and says `V1`–`V4`
would save that work; **`V5` sits in neither list** — a gap in the pre-written gate, recorded here
rather than papered over. What the doc *does* give `V5` is §49.25.7h's sentence — *"`V5` converts a
silent wall into a measurable stall"* — and the row's own cross-read, which boot 14 has now performed.

The question `V5` leaves is therefore the **consumer side**: the coord thread stalls at the VPM port
because whatever is supposed to **drain** the VPM — the path from VPM contents to PTB intake — never
takes delivery, and it does so *without latching any error and without the PTB receiving anything*.
The concrete surface this boot armed for exactly that question is the allocation trio against the
part: `cs_output_vpm_segment_size=1` / `cs_input_vpm_segment_size=0` / `VCM_CACHE_SIZE=4/4`, now
readable for the first time against a measured **8 KB, 2-slice** VPM (§49.25.7b's "present and
Mesa-shaped, never checked against the part" — the checking is now possible and still undone).
Whether §49.25.7e's TMU landmark is also owed under `V5` — it would separate "the port back-pressures
the thread" from "the thread's store path generally is blocked" — is a decision the next brief must
take explicitly, because the pre-written gate is silent on it.

*Fold discipline note:* boot 14 flew and was read the same sitting; this fold was written by the
successor session from the capture, per the standing no-folds-at-close order — the capture, not this
prose, is the evidence of record.

#### 49.25.9 `v3dalloc` — the allocation/consumer audit against the measured part, the TMU-under-V5 decision, and the SINGLESEG rung designed (PI-V3D-104, desk arc, 2026-08-21/22)

`OUTCOME V5`'s residue (§49.25.8) named two undone jobs: read the allocation trio against the part
boot 14 measured, and take the TMU-under-V5 decision §49.25.7e's gate is silent on. This arc is
**desk work only** — no boot flew, no kernel behavior changed. Its products are the audit below, the
decision argued in §49.25.9c, and one designed-not-flown rung (§49.25.9d).

##### 49.25.9a The producer trio, recomputed against the measured part

The part (boot 14's `VPMSIZE` rows): **8 KB VPM, `NSLC=2` slices, `QUPS=4` QPUs/slice, `NTMU=1`
TMU/slice, `NSEM=0`**. The sector unit does not depend on the VPM's size: 1 sector =
`V3D_CHANNELS(16) × 4 B × 8 rows` = **512 B**, so the measured VPM is **16 sectors**.

| field | driver value | recomputed against 8 KB / 2 slices | law source | verdict |
|---|---|---|---|---|
| `cs_output_vpm_segment_size` | 1 | 6 coord words → `align(6,8)/8` = 1 sector = 512 B ≪ 8 KB | Mesa `v3d_vs_set_prog_data` (vir.c), **and the in-tree Mesa artifact** `scripts/pi-v3d26-mesa-compile.out.txt` line 3: `vpm_output_size=1` for the real ver-4.2 `v3d_compile()` coord run | **SANE** |
| `cs_input_vpm_segment_size` | 0 | input folded into output (`separate_segments=0`), per the same artifact line | Mesa vir.c fold (PI-V3D-25), artifact-confirmed | **SANE** |
| `vs_output` / `vs_input_vpm_segment_size` | 1 / 0 | same law, render variant; artifact's `is_coord=0` section prints the same quartet | artifact line 39 | **SANE** (unexercised on armed boots — the ladder returns before `probe_job`) |
| `VCM_CACHE_SIZE` (packet 71) | 4 / 4 | 8192/512 = 16 sectors → half = 8 → `vpm_output_batches` = 8/1 = 8 → `CLAMP(7,2,4)` = **4** | Mesa `CLAMP(vpm_output_batches−1, 2, 4)` (vir.c, PI-V3D-23); artifact prints `vcm_cache_size=4` from Mesa's own run | **SANE** |

**Every place the trio is set was walked, not just the M4 path.** The four shader-record builders in
`v3d.rs` — `build_shader_record` (the real draw), `build_probe_shader_record` (PI-V3D-27's TMU probe),
`build_bisect_null_shader_record` (the `PrimsNullShader` rung) and `build_shader_record_at`
(PI-V3D-71's mainline-geometry record) — write the **identical** quartet `1 / 0 / 1 / 0`, and the two
`VCM_CACHE_SIZE` emitters (the ladder's bin CL and PI-V3D-71's) both write `VCM_CACHE_BATCHES` into
both nibbles from the one constant. There is no second, divergent allocation anywhere in the file, so
the audit above covers the whole surface rather than one caller of it.

Two notes the table compresses:

- **The PI-V3D-23 comment's "16 KiB VPM" figure was stale** — written before any physical read
  existed, it derived 32 sectors → `CLAMP(15,2,4)=4`. The measured 8 KB derives 16 → `CLAMP(7,2,4)=4`:
  **the same value**, because the clamp ceiling absorbs every part ≥ 8 KB (and even the unit caveat —
  if `VPM_SIZE=8`'s unit were larger than KB, the batch count only grows and the clamp still returns
  4). The comment in `v3d.rs` is corrected in place this arc; **no packed byte changes**.
- The GFXH-1744 floor (`Vc ≥ 2`) is respected; nothing in the trio is out of range for a 16-sector
  part, zero-where-nonzero-needed, or above the field's 4-bit width.

**Verdict on the trio: SANE against the measured part.** `OUTCOME V1`'s three numbers, had it fired,
are now checked and would not have convicted. The allocation is not the wall's address.

##### 49.25.9b The consumer side, field by field

The **register-side** consumer arming is settled by prior arcs and is not re-litigated: the kick
writes `CT0QMA` (pool base) → `CT0QMS` (pool size) → `CT0QTS|ENABLE` (tile-state array) →
`CT0QBA`/`CT0QEA` in Linux `v3d_bin_job_run`'s exact order, P53/P54 proved the latch sound, V3D-57
writes `BPOS=0` kernel-exact first, and **leg E's empty frame closes and writes pool 20 words +
tile-state 48 words** — the PTB provably drains a frame it is handed. What remains auditable is the
**CL-side** state the consumer runs under:

| packet / field | value | law source | verdict |
|---|---|---|---|
| `NUMBER_OF_LAYERS` | 0 (minus_one, 1 layer) | Mesa `v3dX(start_binning)` | **SANE** |
| the two memory extents the consumer is handed — `CT0QMS` pool size / `CT0QTS` tile-state array | 32 KiB pool (**compile-asserted** ≥ Mesa's 12 KiB minimum for this frame) / 256 B TSDA (`tiles × 256`, PI-V3D-57's corrected law) | Mesa `v3d_tile_alloc_sizes` (`v3d_util.c`), both encoded as `const _: () = assert!(…)` so a regression is a build error, not a boot | **SANE** — and both are **flight-proven live**: leg E writes 20 pool words and 48 tile-state words into exactly these regions |
| `TILE_BINNING_MODE_CFG` initial/overflow block | 128 B / 64 B | `v3d_limits.h` (PI-V3D-14, Mesa's only silicon-exercised pair) | **SANE** |
| `TILE_BINNING_MODE_CFG` nRT / max-bpp / w / h | 1 (minus_one 0) / 32-bit / minus-one geometry | `v3d_packet.xml` v42 transcription; **flight-proven consumer-live** (empty closes bin into the pool under exactly this config) | **SANE** |
| `TILE_BINNING_MODE_CFG` MSAA / double-buffer fields | **0 by omission** — the builder writes bits 2/4/8/12/32/48 and nothing else | correct for a single-sample, single-buffered frame; and leg E closes under the **byte-identical** config word, so the omission is consumer-accepted, not merely plausible | **SANE** |
| prologue `FLUSH_VCD_CACHE` → OQ addr 0 → `START_TILE_BINNING` | — | Mesa prologue verbatim (PI-V3D-23) | **SANE** |
| `CFG_BITS` | fwd=1, rev=1, rest 0 | no cull either facing | **SANE** |
| `CLIP_WINDOW` / `VIEWPORT_OFFSET` / `CLIPPER_XY` / `CLIPPER_Z` | 0,0,64,64 / centre 32,32 / 8192.0f / 0.5,0.5 | Mesa `v3dx_emit.c` (PI-V3D-17) — and `V5` says nothing ever *reached* the clipper, so these cannot be this wall | **SANE** |
| `GL_SHADER_STATE` | 1 attr, record addr ≫ 5 (`OFF_SHADREC` 32-B aligned) | packet.xml code 64, length-5 (PI-V3D-10) | **SANE** |
| `VERTEX_ARRAY_PRIMS` | TRIANGLES(4), 3 verts, first 0 | packet.xml code 36 | **SANE** |
| attribute record | vec4 float, reads CS=4/VS=4, stride 16, max index `0xFFFF` | Mesa `v3dx_draw.c`; Mesa writes max index `0xFFFFFF`, ours is smaller but ≫ the max index used (2) | **SANE** (with note) |
| record flag bits 0–3, 4–23 | clipping=1, rest 0 | `separate_input_and_output_vpm_blocks=0` **matches** `separate_segments=0`; point-size/vid/iid genuinely unused | **SANE** |
| record `fs_number_of_varyings` (bits 24–31) | 0 | our FS is the PI-V3D-9 solid-colour program — it reads **no** varyings, so 0 is the true count whichever pass consumes the field; a bin frame runs no FS at all | **SANE** |

**The one divergence found — and it does NOT convict.** Record bits **97 / 161 / 225** — the
per-shader **"start in final thread section"** flags (the slot between "4-way threadable" and
"propagate NaNs") — are packed **0** for all three shaders, while every program we ship is
**single-segment**: terminal-only `thrsw`, no mid-shader thread switch (the artifact's own coord
listing, word [19], and §49.25.7e's description of the good program class). Mesa packs this field
from `prog_data->single_seg` (`v3dx_draw.c` / `v3dvx_pipeline.c`). Whether `single_seg` is **1** for
a no-TMU, terminal-thrsw-only, `threads=4` program could **not** be pinned at this desk: the in-tree
artifact does not print `single_seg`, and the vir.c assignment (`single_seg = !c->last_thrsw`, with
the open question of whether the always-emitted final `thrsw` counts as `last_thrsw`) is exactly the
kind of half-remembered law §5's fabricated-constant rule exists to stop. **Verdict: SUSPECT
(the producer surface's sole Mesa-divergence candidate), polarity UNKNOWN, conviction withheld — no
code packed.** Resolving the polarity from a fresh Mesa checkout is the designed rung's gate task
(§49.25.9d, task 0); this arc ships **no** knob because an unverified constant is not a fix, it is
the fourth conviction waiting to happen.

**A second finding, print-only, recorded so task 0 sweeps it up.** `witness_shadrec_diff`'s field
table (the `[v3d38]` record diff in `v3d.rs`) names bit **97** `fs_single_seg` and bit **161**
`vs_2way_threadable` — **the same slot in two different shader blocks, under two different names**,
so at least one of the two labels is wrong, and the table names neither 225 nor 226 at all. Nothing
is packed from these labels: they decorate a diff print, and both bits are written 0 either way, so
**no image byte depends on the answer**. But the name is the same question the paragraph above just
refused to answer from memory, so it is **UNKNOWN** here too, and it belongs to the same task 0 — one fresh
read of `v3d_packet.xml` v42 settles the polarity, the name, and this label in one sitting. Fixing
the print label before that read would only move the guess from one place in the file to another.

##### 49.25.9c TMU under `V5`: argued, and the answer is NO

**The conclusion, in the two sentences the decision is owed.** A TMU store leaves the QPU through
`tmud`/`tmua` into the TMU → L2T → memory path and never touches the VPM write FIFO, so the landmark
does **not** ride the same drain: it genuinely discriminates "the VPM port back-pressures this
thread" from "this thread cannot land a store anywhere" (§49.25.8's two phrasings). But that is not
the question `V5` leaves open — the undelivered hand-off sits **downstream of a thread that boot 14
already showed issuing 53 valid instructions and reaching program end** — so the landmark would
spend a boot re-partitioning the QPU side while the VCM→PSE→PTB chain, the only station `V5`
accuses, stays uninstrumented: **it stays gated on `V6`.**

The argument at length, and honestly about its one soft edge. What survives on the QPU side after
boot 14 is an *issue* witness, not a *landing* witness: `QPU_CYCLES_VALID_INSTR d=53` says the
instructions were issued, and §49.25.8 itself re-read `INT_STS` bit16 as a thread **RELEASED rather
than FINISHED**, so "all six `STVPMV` stores completed" is *not* something this campaign has proven
and this section does not claim it. That crack is exactly what a TMU landmark would close — and
closing it would still leave the wall where `V5` put it, because a landmark that lands in memory
proves only that a *different port* works, and a landmark that does not land indicts a QPU-wide
store block that would have to explain, without a single latched error bit, how the same thread
reached program end at all. Either way the next question is identical: **why does nothing drain the
VPM toward the PTB** — and that is a station a QPU-side landmark cannot see, because it happens
after the thread the landmark instruments has ended.

**Decision, recorded: the TMU landmark does NOT fly under `V5`.** §49.25.7e's gate stays exactly as
written — the landmark (and its blocking artifact, the fresh `v3d_compile()` harness) is built on
`V6` and only on `V6`. The gap §49.25.8 flagged in the pre-written gate is hereby closed: `V5` joins
`V1`–`V4` on the "save the work" side. And the residue is named rather than buried: **the day the
consumer-side instrumentation dead-ends, the "did any store LAND" doubt is the landmark's, and it is
the tool for that day** — boot 14's bit16 + 53-instruction witness makes it the second question, not
the first.

##### 49.25.9d The next rung, designed and not flown: `singleseg` (PI-V3D-104)

**One variable.** Shader-record **bit 225** — `coordinate shader start in final thread section` —
packed 1 instead of 0. Nothing else: bits 97 (FS) and 161 (VS) stay 0 because the armed ladder's bin
frames dispatch only the coordinate shader (the boot returns before `probe_job`), and widening the
delta to unexercised bits widens the variable for nothing. Every list, region, digest, slot source
and poison collar stays byte-for-byte PA51's; the baseline the boot is read against is **boot 14
itself**.

**Knob: `UNAOS_V3D_SINGLESEG=1`**, family discipline throughout — cargo feature
`v3d_singleseg = ["v3d_vpmprobe"]` so the whole chain arms from one env var, an arm-pi feature-list
entry for cfg coverage, a kernel8 knob block, and a `[v3d104]` serial line naming the packed bit so
the capture self-describes (strings-proofed at build time like every armed line).

**Task 0, at the desk, before any packing — the rung's gate.** Pin the field's polarity from a fresh
Mesa checkout: `v3d_set_prog_data`'s `single_seg` assignment (vir.c), `vir_emit_last_thrsw`'s
handling of the terminal `thrsw` (nir_to_vir.c), and the `v3dx_draw.c` packing that copies it into
the record. §49.25.9b refused to pack this bit from memory; the rung must not either. The same read
settles §49.25.9b's second finding — the `[v3d38]` witness table's disagreeing labels at bits 97 and
161 — and that correction lands with task 0 whichever way the polarity falls, because it is
print-only and changes no packed byte. The polarity answer is itself the first pre-written row:

| observed | verdict | what it means |
|---|---|---|
| task 0 resolves `single_seg = 0` for this program class | **`OUTCOME S0`** | **the rung dies at the desk, no boot spent.** The record was right all along; the producer surface is fully Mesa-parallel *and* part-checked, and the campaign's next build is consumer-side instrumentation, not another producer field |
| leg E diverges from `OUTCOME E1` in any way | **INCONCLUSIVE** | the empty frame never fetches the shader record, so an E change convicts the **arm** (a build that leaked wider than one bit), not the field — outranks every S row, same law as §49.25.7d's guards |
| frame **closed** (`BFC` Δ1, retired=1) and/or `PTB_PRIMS_BINNED` moved, pool words > empty's 20 | **`OUTCOME S1`** | **the bit was the drain wall.** A mis-declared thread-section shape left the batch-complete handshake unsatisfied; the fix graduates from knob to default and the campaign moves to the render side |
| boot-14's leg H shape **to the digit** (stall ≈ active, no PTB slot, no new ERR bit, no close) | **`OUTCOME S2`** | **the bit is exonerated** and the producer surface is closed: every field Mesa-parallel, part-checked, and now flight-tested both ways. The wall is purely the consumer, and the next rung must instrument the VCM/PSE side, not the record |
| stall collapses (d ≈ 0 against active ≈ 28) but still no PTB movement and no close | **`OUTCOME S3`** | the bit changes the **thread's port behavior** without buying delivery — the stall and the drain are two stations, not one; fold carefully before naming the next |
| any **newly-latched** `ERR_STAT` bit | **`OUTCOME S4`** | the bit is illegal for this program class on this part — the silicon itself answers task 0's polarity question in the negative; revert and record |

One boot, cold, short capture, read in §49.25.7h's order with `[v3d104]`'s line read first (it is
desk work on the wire, like `VPMSTATIC` before it).

##### 49.25.9e Measured, this arc

Worktree `exec-v3dalloc`, baseline `b2471fec`. Doc + one stale-comment correction in `v3d.rs`
(PI-V3D-23's "16 KiB VPM" → the measured 8 KB derivation, §49.25.9a); no packed byte, no knob, no
new serial line — the SINGLESEG knob is **designed here, not built**, gated on its task 0.

| gate | result |
|---|---|
| `./arroyo check` | **green, both arches** — `✅ x86_64 OK`, `✅ aarch64 OK`, `✅ kernel cfg coverage OK (12 legs)`, `✅ midden_core tests OK` |
| `./arroyo test-arm` | **clean** — headless aarch64 QEMU to completion, `BOT-PARK … -> PASS` and `SERWIT-2 … -> PASS`, **0** `FAIL`/`PANIC`/`❌` lines in `target/serial-arm.log` |
| `./arroyo kernel8-test` | **MBENCH PASS — 117/117 required witnesses, 0 forbidden hit(s), 10480 lines scanned**; image `4eeb3f935d7151b0…` |
| code-change extent | `git diff -U0` on `v3d.rs` = **10 changed lines, 0 of them non-comment** (machine-checked, not asserted) — the arc's only kernel edit is prose inside `//`, so no instruction, constant or packed byte moves; no separate baseline image was built and none is claimed |
| strings-proof | **none owed** — no knob, no feature, no new serial line was added this arc; the `[v3d104]` line named in §49.25.9d is part of the *design*, and it is proofed the day it is built, not here |

*Provenance, per the file's honesty discipline.* This arc was opened by one session and **finished by a
successor** after the first was halted by a usage limit mid-flight: §§49.25.9a–d's audit, decision and
rung design are the predecessor's work, read against the code and the boot-14 capture and kept; the
successor added §49.25.9a's every-place sweep, §49.25.9b's two extra rows and the `[v3d38]` label
finding, sharpened §49.25.9c to stop short of claiming the `STVPMV` stores *completed* (§49.25.8 reads
bit16 as RELEASED, not FINISHED — the two sections now agree), and ran the gates above. No boot flew
for either half.
