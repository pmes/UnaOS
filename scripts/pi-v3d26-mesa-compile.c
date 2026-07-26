/* PI-V3D-26 — Mesa-AUTHORITATIVE shader witness (the §5 fabricated-constant law,
 * taken to its end: not just Mesa-PACKED words, but Mesa-COMPILED ones).
 *
 * Prior arcs hand-STRUCTURED the coord/vertex instruction sequence and packed it
 * with Mesa's v3d_qpu_instr_pack (scripts/pi-v3dN-qpu-gen.c). This harness instead
 * builds the UnaOS trivial passthrough VS in NIR and runs Mesa's REAL v3d_compile()
 * (ver 4.2) end-to-end, dumping the exact QPU words + uniform stream a genuine driver
 * would feed the QPU for our draw — for the coord/bin variant, the render VS, and a
 * TMU-store PROBE variant (the direct "what did the QPU receive" witness §10 deferred
 * as un-hand-encodable). Output checked in as pi-v3d26-mesa-compile.out.txt.
 *
 * REPRODUCTION (macOS, no meson/ninja; a manual build against a sparse Mesa checkout):
 *   git clone --filter=blob:none --sparse https://gitlab.freedesktop.org/mesa/mesa.git
 *   cd mesa && git sparse-checkout set include src/broadcom src/util src/compiler \
 *       src/c11 src/gallium/auxiliary/util src/gallium/include
 *   python3 -m pip install --user mako pyyaml        # NIR/genxml/format generators
 *   # generate the meson-time headers (no meson available):
 *   #   src/compiler/nir/*_h.py, *_c.py, nir_intrinsics*.py, nir_opt_algebraic.py
 *   #   src/compiler/builtin_types_{h,c}.py
 *   #   src/util/format/u_format_table.py (--enums/--header/table), src/util/format_srgb.py
 *   #   src/broadcom/cle/gen_pack_header.py v3d_packet.xml 42
 *   #   src/broadcom/compiler/v3d_nir_lower_algebraic.py -p src/compiler/nir
 *   # then cc -DHAVE_PTHREAD -DHAVE_STRUCT_TIMESPEC -DHAVE_TIMESPEC_GET \
 *   #   -DBLAKE3_NO_SSE2 -DBLAKE3_NO_SSE41 -DBLAKE3_NO_AVX2 -DBLAKE3_NO_AVX512 \
 *   #   <nir + broadcom/{compiler,qpu,common} + compiler/{glsl_types,shader_enums,builtin_types}
 *   #    + util + generated> this-file -lpthread -o qpuwitness
 * The full build.sh used is archived alongside this arc's scratchpad. Mesa is MIT —
 * used with attribution. DEV mirrors v3d_device_info_init() for the Pi 4 (V3D 4.2):
 * vpm_size=16384, has_accumulators=1, clipper_xy_granularity=256, cle_readahead=256.
 *
 * FINDING (see docs/dev/OS/08_VIDEO/v3d.md §11): with the key configured as the real
 * driver configures the binning VS (num_used_outputs=0 for the last-geometry-stage
 * coord shader — v3dv pipeline_populate_v3d_vs_key), Mesa's coord shader stores clip
 * Xc,Yc,Zc,Wc to VPM offsets 0,1,2,3 and screen Xs,Ys to 4,5, with viewport scale
 * 32*256=8192 — byte-for-byte our committed CS_VS_WORDS contract. Our hand shader is
 * FUNCTIONALLY MESA-EQUIVALENT for the W=1 test geometry. The coord shader is
 * exonerated at the authoritative level; the empty-bin wall is not in it.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "broadcom/common/v3d_device_info.h"
#include "broadcom/compiler/v3d_compiler.h"
#include "broadcom/qpu/qpu_disasm.h"
#include "compiler/nir/nir.h"
#include "compiler/nir/nir_builder.h"
#include "compiler/shader_enums.h"

/* Pi 4 VideoCore VI, V3D 4.2 — fields as v3d_device_info_init() would fill
 * them from the hardware IDENT registers (see v3d_device_info.c):
 *   vpm_size = (IDENT1[31:28]) * 8192 = 16384 (16 KiB VPM, 32 sectors of 512 B),
 *   has_accumulators = ver < 71 = true, clipper_xy_granularity = 256.0 (ver 42),
 *   cle_readahead = 256, qpu_count = NSLC*QUPS (8 for the Pi 4 single-core). */
static void dbg(const char *msg, void *data);

static struct v3d_device_info DEV = {
    .ver = 42, .rev = 0,
    .vpm_size = 16384,
    .qpu_count = 8,
    .has_accumulators = true,
    .clipper_xy_granularity = 256.0f,
    .cle_readahead = 256u,
    .page_size = 4096,
    .max_render_targets = 4,
};

static const char *quniform_name(enum quniform_contents c) {
    switch (c) {
    case QUNIFORM_CONSTANT: return "CONSTANT";
    case QUNIFORM_UNIFORM: return "UNIFORM";
    case QUNIFORM_VIEWPORT_X_SCALE: return "VIEWPORT_X_SCALE";
    case QUNIFORM_VIEWPORT_Y_SCALE: return "VIEWPORT_Y_SCALE";
    case QUNIFORM_VIEWPORT_Z_OFFSET: return "VIEWPORT_Z_OFFSET";
    case QUNIFORM_VIEWPORT_Z_SCALE: return "VIEWPORT_Z_SCALE";
    default: return "(other)";
    }
}

/* Transcribed verbatim from Mesa v3dv_pipeline_get_nir_options()
 * (src/broadcom/vulkan/v3dv_pipeline.c) — the in-checkout authoritative
 * options for the v3d compiler backend. */
static const nir_shader_compiler_options V3D_NIR_OPTIONS = {
    .lower_uadd_sat = true, .lower_usub_sat = true, .lower_iadd_sat = true,
    .lower_extract_byte = true, .lower_extract_word = true,
    .lower_insert_byte = true, .lower_insert_word = true,
    .lower_bitfield_insert = true, .lower_bitfield_extract = true,
    .lower_bitfield_reverse = true, .lower_bit_count = true,
    .lower_cs_local_id_to_index = true, .lower_ffract = true, .lower_fmod = true,
    .lower_pack_unorm_2x16 = true, .lower_pack_snorm_2x16 = true,
    .lower_unpack_unorm_2x16 = true, .lower_unpack_snorm_2x16 = true,
    .lower_pack_unorm_4x8 = true, .lower_pack_snorm_4x8 = true,
    .lower_unpack_unorm_4x8 = true, .lower_unpack_snorm_4x8 = true,
    .lower_pack_half_2x16 = true, .lower_unpack_half_2x16 = true,
    .lower_pack_32_2x16 = true, .lower_pack_32_2x16_split = true,
    .lower_unpack_32_2x16_split = true, .lower_mul_2x32_64 = true,
    .lower_fdiv = true, .lower_find_lsb = true, .lower_flrp16 = true,
    .lower_flrp32 = true, .lower_fpow = true, .lower_fsqrt = true,
    .lower_ifind_msb = true, .lower_isign = true, .lower_mul_high = true,
    .lower_wpos_pntc = false, .lower_to_scalar = true,
    .lower_device_index_to_zero = true, .lower_fquantize2f16 = true,
    .lower_ufind_msb = true, .has_fsub = true, .has_isub = true,
    .has_imul24 = true, .has_umul24 = true, .has_uclz = true,
    .vertex_id_zero_based = false, .lower_interpolate_at = true,
    .max_unroll_iterations = 16,
    .force_indirect_unrolling = (nir_var_shader_in | nir_var_function_temp),
    .divergence_analysis_options =
        nir_divergence_multiple_workgroup_per_compute_subgroup,
    .discard_is_demote = true, .scalarize_ddx = true,
};

static nir_shader *build_passthrough_vs(const nir_shader_compiler_options *opts) {
    nir_builder b = nir_builder_init_simple_shader(MESA_SHADER_VERTEX, opts,
                                                   "unaos passthrough vs");
    const struct glsl_type *vec4 = glsl_vec4_type();

    nir_variable *in_pos =
        nir_variable_create(b.shader, nir_var_shader_in, vec4, "attr_pos");
    in_pos->data.location = VERT_ATTRIB_GENERIC0;   /* generic attribute 0 */
    in_pos->data.driver_location = 0;

    nir_variable *out_pos =
        nir_variable_create(b.shader, nir_var_shader_out, vec4, "gl_Position");
    out_pos->data.location = VARYING_SLOT_POS;
    out_pos->data.driver_location = 0;

    nir_def *pos = nir_load_var(&b, in_pos);
    nir_store_var(&b, out_pos, pos, 0xf);

    b.shader->info.stage = MESA_SHADER_VERTEX;
    b.shader->num_outputs = 1;
    b.shader->num_inputs = 1;
    return b.shader;
}

/* Probe VS: passthrough (attr0 -> gl_Position) that ALSO stores the four loaded
 * attribute components to SSBO 0 at offset 0 — a direct "what did the QPU
 * receive" witness. v3d_compile lowers store_ssbo to a TMU general store. */
static nir_shader *build_probe_vs(const nir_shader_compiler_options *opts) {
    nir_builder b = nir_builder_init_simple_shader(MESA_SHADER_VERTEX, opts,
                                                   "unaos probe vs");
    const struct glsl_type *vec4 = glsl_vec4_type();
    nir_variable *in_pos =
        nir_variable_create(b.shader, nir_var_shader_in, vec4, "attr_pos");
    in_pos->data.location = VERT_ATTRIB_GENERIC0;
    in_pos->data.driver_location = 0;
    nir_variable *out_pos =
        nir_variable_create(b.shader, nir_var_shader_out, vec4, "gl_Position");
    out_pos->data.location = VARYING_SLOT_POS;
    out_pos->data.driver_location = 0;

    nir_def *pos = nir_load_var(&b, in_pos);
    nir_store_var(&b, out_pos, pos, 0xf);
    /* store the loaded vec4 to SSBO 0 @ offset 0 */
    nir_store_ssbo(&b, pos, nir_imm_int(&b, 0), nir_imm_int(&b, 0),
                   .write_mask = 0xf, .access = 0, .align_mul = 16,
                   .align_offset = 0);

    b.shader->info.stage = MESA_SHADER_VERTEX;
    b.shader->info.num_ssbos = 1;
    b.shader->num_outputs = 1;
    b.shader->num_inputs = 1;
    return b.shader;
}

static void dump_probe(const struct v3d_compiler *comp) {
    nir_shader *s = build_probe_vs(&V3D_NIR_OPTIONS);
    struct v3d_vs_key key; memset(&key, 0, sizeof key);
    key.base.is_last_geometry_stage = true;
    key.is_coord = true;         /* bin/coord variant — the one that runs first */
    key.num_used_outputs = 0;
    struct v3d_prog_data *pd = NULL; uint32_t asz = 0;
    uint64_t *w = v3d_compile(comp, &key.base, &pd, s, dbg, NULL, 0, 0, &asz);
    if (!w) { printf("\nPROBE COMPILE FAILED\n"); return; }
    uint32_t n = asz / 8;
    printf("\n========== PROBE VS (coord + SSBO store of attr inputs) ==========\n");
    printf("threads=%u  tmu_count=%u  uniforms=%u  words=%u\n",
           pd->threads, pd->tmu_count, pd->uniforms.count, n);
    printf("-- uniform stream --\n");
    for (uint32_t i=0;i<pd->uniforms.count;i++)
        printf("  u[%2u] %-18s 0x%08x\n", i, quniform_name(pd->uniforms.contents[i]),
               pd->uniforms.data[i]);
    printf("-- QPU words --\n");
    for (uint32_t i=0;i<n;i++){ const char*d=v3d_qpu_disasm(&DEV,w[i]);
        printf("  [%2u] 0x%016llx  %s\n", i,(unsigned long long)w[i], d?d:"?"); }
    fflush(stdout);
}

static void dbg(const char *msg, void *data) { (void)data; fprintf(stderr, "%s", msg); }

static void dump_variant(const struct v3d_compiler *comp, bool is_coord) {
    /* fresh nir per compile: v3d_compile consumes/mutates and frees it */
    (void)comp;
    nir_shader *s = build_passthrough_vs(&V3D_NIR_OPTIONS);

    struct v3d_vs_key key;
    memset(&key, 0, sizeof key);
    key.base.is_last_geometry_stage = true;   /* no GS/TES: VS is last geom stage */
    key.is_coord = is_coord;
    /* Our draw: single vec4 attribute -> gl_Position, solid-colour FS that
     * consumes NO varyings. The driver sets num_used_outputs = 0 for the
     * binning (coord) VS when it is the last geometry stage
     * (v3dv pipeline_populate_v3d_vs_key, line ~1306/1350), and for the
     * render VS the FS reads nothing, so 0 as well. */
    key.num_used_outputs = 0;

    struct v3d_prog_data *prog_data = NULL;
    uint32_t asm_size = 0;
    uint64_t *words = v3d_compile(comp, &key.base, &prog_data, s,
                                  dbg, NULL, 0, 0, &asm_size);
    if (!words) { printf("COMPILE FAILED (is_coord=%d)\n", is_coord); return; }

    struct v3d_vs_prog_data *vs = (struct v3d_vs_prog_data *)prog_data;
    uint32_t n = asm_size / sizeof(uint64_t);

    printf("\n========== VS variant: is_coord=%d  (%s) ==========\n",
           is_coord, is_coord ? "COORD/BIN shader" : "RENDER VS");
    printf("threads=%u  vpm_input_size=%u  vpm_output_size=%u  vcm_cache_size=%u  separate_segments=%d\n",
           prog_data->threads, vs->vpm_input_size, vs->vpm_output_size,
           vs->vcm_cache_size, vs->separate_segments);
    printf("vattr_sizes[0..3]=%u,%u,%u,%u  uses_vid=%d uses_iid=%d\n",
           vs->vattr_sizes[0], vs->vattr_sizes[1], vs->vattr_sizes[2], vs->vattr_sizes[3],
           vs->uses_vid, vs->uses_iid);

    printf("-- uniform stream (count=%u) --\n", prog_data->uniforms.count);
    for (uint32_t i = 0; i < prog_data->uniforms.count; i++) {
        enum quniform_contents c = prog_data->uniforms.contents[i];
        printf("  u[%2u] %-18s data=0x%08x (%d)\n", i, quniform_name(c),
               prog_data->uniforms.data[i], (int)prog_data->uniforms.data[i]);
    }

    printf("-- QPU words (%u instructions) --\n", n);
    for (uint32_t i = 0; i < n; i++) {
        const char *dis = v3d_qpu_disasm(&DEV, words[i]);
        printf("  [%2u] 0x%016llx  %s\n", i, (unsigned long long)words[i],
               dis ? dis : "?");
    }
    fflush(stdout);
}

int main(void) {
    const struct v3d_compiler *comp = v3d_compiler_init(&DEV, 0);
    if (!comp) { printf("compiler_init failed\n"); return 1; }
    dump_variant(comp, true);   /* coordinate/bin shader */
    dump_variant(comp, false);  /* render VS */
    dump_probe(comp);           /* coord + SSBO-store probe */
    return 0;
}
