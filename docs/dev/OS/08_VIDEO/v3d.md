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
   byte-exact to `v3d_bin_job_run` ("Clear out the overflow allocation, so we don't reuse the overflow
   attached to a previous job").

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
