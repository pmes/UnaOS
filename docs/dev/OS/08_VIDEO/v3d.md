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
