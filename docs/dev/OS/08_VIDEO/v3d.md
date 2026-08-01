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
counting nonzero words plus the first-nonzero index, and prints `BPCA`/`BPCS`/`BFC` alongside.

Two integrity properties this readback must carry, because the negative reading is the interesting one:

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
| `[v3d55] tilestate` | nonzero words **> 0** | The PTB **wrote** per-tile output ⇒ the defect is isolated to the **FLDONE/BFC frame-close latch itself** — the narrowest target the campaign has ever had. |
| `[v3d55] tilestate` | **all zero**, `flush_done = 1` | On this evidence the PTB wrote nothing ⇒ the defect is **upstream** of frame-close. |
| `[v3d55] tilestate` / `pool` | **all zero**, `flush_done = 0` | **No verdict.** `L2TFLS` never cleared, so the bytes may still be parked in L2T — read `clkliv`/`clkdom` first. |
| `[v3d55] pool` | full pool all-zero, `flush_done = 1`, `BPCA` advanced off base | `BPCA` is a **phantom/aliased** write pointer — the P40 contradiction is an addressing one; `mmucfg` + the `pte` lines decide. |
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
:: V3D: [v3d55] tilestate (<tag>) — TSDA iova=0x……… bytes=<n> words=<n> nonzero_words=<n> first_nz=[<i|-1>]
        | L2T write-back completed=<0|1> (L2TCACTL after=0x………) | head w0..w7 = 0x……… ×8 — <verdict> ::
:: V3D: [v3d55] pool (<tag>) — pool iova=0x……… bytes=<n> words=<n> nonzero_words=<n> first_nz=[<i|-1>]
        (FULL-pool scan) | L2T write-back completed=<0|1> | head w0..w7 = 0x……… ×8
        | BPCA=0x……… (adv 0x… off pool base) BPCS=0x……… BFC=0x……… — <verdict> ::
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
| `[v3d59] ctstate` | inferred `CTSUBS` set at S4 | *On the borrowed map*, the thread believes it is still inside a **sub-list** at the end of a list that reached `EA`, and such a thread never reaches the top-level `FLUSH`'s completion semantics. Audit every `BRANCH`/`RETURN` in the bin CL. The `CTRTSD[9:8]` window is printed as raw bits — the sources disagree on its width, so it is a candidate depth, not a depth. |
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
| `UNAOS_V3D_DEEP=1` | `v3d_deep` (implies `v3d`) | **Adds** the three banked-verdict probes above — ~3.5 s of extra boot. Arm it only when the bench is deliberately re-opening one of those questions. |

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
> `[v3d80 post-handover]`, `[v3d81q]` — six callers) — now **emits** the `[v3d73]` witness it was already arming (wait → emit →
> read-span, the `[v3d74a]` idiom), so the sampler rides every CT0-kicking deep leg and the fetch
> verdict on those legs is measured, not extrapolated from the three originally-armed rungs.

## 45. Thread 0 or bin class? — the thread-swap discriminator (V3D-74)

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

Every `[v3d81]` line states the armed legs, the settle, the tag ids and both stations, so a capture
is self-describing and datable without the reader knowing how the image was built.

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
