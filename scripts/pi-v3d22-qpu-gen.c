/* Reproduce (against a Mesa checkout, e.g. mesa 26.3.0-devel):
 *   git clone --depth 1 https://gitlab.freedesktop.org/mesa/mesa.git
 *   cc -I mesa/src -I mesa/include -I mesa/src/broadcom -o qpu_gen22 \
 *        mesa/src/broadcom/qpu/qpu_instr.c mesa/src/broadcom/qpu/qpu_pack.c \
 *        scripts/pi-v3d22-qpu-gen.c
 *   ./qpu_gen22               # prints every word + round-trip + self-test (see .out.txt)
 * The printed hex is transcribed verbatim into GRAD_VS_WORDS in
 * unaos/crates/kernel/src/arch/aarch64/v3d.rs.
 *
 * PI-V3D-22 shader-word generator — applies the PI-V3D-20 finding to the M5
 * gradient VERTEX shader (GRAD_VS_WORDS, the RENDER-path VS at OFF_M5_VS_CODE).
 *
 * ROOT CAUSE (inherited from PI-V3D-20): the M5 VS wrote its per-vertex VPM
 * output with the streamed VC4 / V3D-3.3 mechanism — a `vpmsetup` to arm a VPM
 * segment, then `mov vpm, rfN` (magic waddr VPM=14) auto-advancing an implicit
 * write pointer. That mechanism DOES NOT EXIST for per-vertex shader output on
 * V3D 4.x (devinfo->ver==42, the Pi 4 VideoCore VI). PI-V3D-20 proved it for the
 * coordinate shader and fixed CS_VS_WORDS; this arc applies the identical fix to
 * the M5 render VS: Mesa's `vir_VPM_WRITE` (src/broadcom/compiler/nir_to_vir.c)
 * emits exactly one `vir_STVPMV(c, vir_uniform_ui(c, vpm_index), val)` per output
 * component — a store-VPM with an EXPLICIT integer VPM offset — and never
 * `mov vpm` / `vpmsetup` in the ver-42 VS output path. The old M5 `mov vpm`
 * writes landed on an unconfigured magic register; nothing the rasterizer reads.
 *
 * NON-COORD (RENDER) VPM CONTRACT — verified against Mesa
 * `v3d_nir_setup_vpm_layout_vs` (src/broadcom/compiler/v3d_nir_lower_io.c) for
 * is_coord==false, is_last_geometry_stage==true:
 *     vp_vpm_offset = 0  -> Xs @ 0, Ys @ 1   (screen X/Y, 2 words)
 *     zs_vpm_offset = 2  -> Zs @ 2           (screen depth)
 *     rcp_wc_vpm_offset = 3 -> 1/Wc @ 3
 *     varyings_vpm_offset = 4 -> user varyings @ 4..
 * The render VS emits the FOUR-word position block [Xs, Ys, Zs, 1/Wc], NOT the
 * six-word [Xc,Yc,Zc,Wc,Xs,Ys] coordinate-shader layout (that is the is_coord
 * path only, +4 clip words at 0..3). This is the exact difference PI-V3D-18
 * landed and the brief calls out.
 *
 * SCREEN MATH — verified against `v3d_nir_emit_ff_vpm_outputs` (same file):
 *     rcp_wc = frcp(pos.w)
 *     Xs = f2i32(ffloor(Xc * vp_x_scale * rcp_wc))   (ffloor is the ver==42 path)
 *     Ys = f2i32(ffloor(Yc * vp_y_scale * rcp_wc))
 *     Zs = (Zc * viewport_z_scale) * rcp_wc + viewport_z_offset
 *     1/Wc = rcp_wc
 * vp_scale = viewport.scale(32) * clipper_xy_granularity(256) = 8192
 * (v3d_uniforms.c QUNIFORM_VIEWPORT_X/Y_SCALE; granularity 256.0f for ver 42).
 * viewport_z_scale / viewport_z_offset are the SAME viewport Z params the M5 RCL
 * programs into CLIPPER_Z_SCALE_AND_OFFSET (0.5 / 0.5, mapping NDC z[-1,1] ->
 * depth[0,1]); sourced here as uniforms exactly as Mesa sources them
 * (QUNIFORM_VIEWPORT_Z_SCALE / _OFFSET).
 *
 * W=1 SIMPLIFICATION (documented LOUDLY, same stance as PI-V3D-19/20): the M5
 * TRI_VERTS all carry Wc = 1.0, so rcp_wc = 1.0 and NO reciprocal (recip / SFU)
 * instruction is emitted — the screen transform collapses to
 *   Xs = f2i32(ffloor(Xc * 8192)),  Zs = Zc*z_scale + z_offset,  1/Wc = Wc(=1.0).
 * The `1/Wc` output word is the rf3 (Wc) passthrough, exact because Wc==1.0.
 * Perspective (W != 1) geometry would need a per-vertex reciprocal here and the
 * rcp_wc multiply restored on Xs/Ys/Zs.
 *
 * COLOUR VARYINGS: the M5 vertex data is [pos vec4 | colour vec4] interleaved;
 * the render VS reads the vec4 colour and stores it as four varyings at VPM
 * offsets 4..7 (num_varyings=4 in the shader-state record, unchanged). The FS
 * (GRAD_FS_WORDS) consumes three via ldvary + alpha from a uniform — the raw
 * (un-interpolated) varying semantics remain the M5 metal-refinement seam the
 * brief preserves; this arc changes only the OUTPUT MECHANISM, not the FS.
 *
 * ISA-VERSION NOTE (from PI-V3D-20): STVPMV (opcode 248, add A|B, ver-42 table
 * `v3d42_add_ops` in qpu_pack.c) is the ver-42 output op; its packer sets
 * waddr=0 / no_magic_write, so add.magic_write MUST be false and offset/value
 * are plain register-file reads on mux A/B. `vpmsetup` is DROPPED (on 4.x it
 * arms VPM *DMA* descriptors, not the shader output stream). VPMWT stays
 * (GFXH-1684, ver==42 emit_vert_end).
 *
 * Every word is packed with Mesa's OWN packer (v3d_qpu_instr_pack, ver=42),
 * round-tripped through v3d_qpu_instr_unpack + repack, and the harness first
 * reproduces the canonical Mesa qpu_disasm.c vectors bit-exactly. Mesa is
 * MIT-licensed; used with attribution (memory: unaos-license-gplv3).
 */
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include "broadcom/common/v3d_device_info.h"
#include "broadcom/qpu/qpu_instr.h"

static struct v3d_device_info DEV = { .ver = 42 };

static struct v3d_qpu_instr base_nop(void) {
    struct v3d_qpu_instr i;
    memset(&i, 0, sizeof i);
    i.type = V3D_QPU_INSTR_TYPE_ALU;
    i.alu.add.op = V3D_QPU_A_NOP;
    i.alu.add.waddr = V3D_QPU_WADDR_NOP;
    i.alu.add.magic_write = true;
    i.alu.add.a.mux = V3D_QPU_MUX_R0; i.alu.add.b.mux = V3D_QPU_MUX_R0;
    i.alu.mul.op = V3D_QPU_M_NOP;
    i.alu.mul.waddr = V3D_QPU_WADDR_NOP;
    i.alu.mul.magic_write = true;
    i.alu.mul.a.mux = V3D_QPU_MUX_R0; i.alu.mul.b.mux = V3D_QPU_MUX_R0;
    return i;
}

/* attach ldunifrf -> rf[idx] signal to an instruction (loads next uniform). */
static void add_ldunifrf(struct v3d_qpu_instr *i, int idx) {
    i->sig.ldunifrf = true;
    i->sig_addr = idx;
    i->sig_magic = false;
}

static uint64_t pack_checked(const char *label, struct v3d_qpu_instr in) {
    uint64_t w = 0;
    if (!v3d_qpu_instr_pack(&DEV, &in, &w)) {
        fprintf(stderr, "PACK FAIL: %s\n", label);
        exit(2);
    }
    struct v3d_qpu_instr back;
    if (!v3d_qpu_instr_unpack(&DEV, w, &back)) {
        fprintf(stderr, "UNPACK FAIL: %s (0x%016llx)\n", label, (unsigned long long)w);
        exit(3);
    }
    uint64_t w2 = 0;
    if (!v3d_qpu_instr_pack(&DEV, &back, &w2) || w2 != w) {
        fprintf(stderr, "ROUNDTRIP FAIL: %s 0x%016llx -> 0x%016llx\n",
                label, (unsigned long long)w, (unsigned long long)w2);
        exit(4);
    }
    printf("  0x%016llx  ; %s\n", (unsigned long long)w, label);
    return w;
}

static void expect(const char *what, uint64_t got, uint64_t want) {
    printf("  selftest %-28s 0x%016llx %s\n", what,
           (unsigned long long)got, got == want ? "OK" : "*** MISMATCH ***");
    if (got != want) exit(5);
}

/* mov <magic waddr>, r<src> on the mul unit (Mesa "mov vpm, r3") — used ONLY by
 * the self-test vector; the shader body no longer emits any mov-vpm. */
static struct v3d_qpu_instr mov_magic(enum v3d_qpu_waddr w, enum v3d_qpu_mux src) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.mul.op = V3D_QPU_M_MOV;
    i.alu.mul.a.mux = src; i.alu.mul.b.mux = src;
    i.alu.mul.waddr = w; i.alu.mul.magic_write = true;
    return i;
}

/* fmul rf<dst>, rf<a>, rf<b>  (mul unit; two register-file reads via A/B). */
static struct v3d_qpu_instr fmul_rf(int dst_rf, int a_rf, int b_rf) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.mul.op = V3D_QPU_M_FMUL;
    i.alu.mul.a.mux = V3D_QPU_MUX_A; i.alu.mul.b.mux = V3D_QPU_MUX_B;
    i.raddr_a = a_rf; i.raddr_b = b_rf;
    i.alu.mul.waddr = dst_rf; i.alu.mul.magic_write = false;
    return i;
}

/* fadd rf<dst>, rf<a>, rf<b>  (add unit; two register-file reads via A/B). */
static struct v3d_qpu_instr fadd_rf(int dst_rf, int a_rf, int b_rf) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.add.op = V3D_QPU_A_FADD;
    i.alu.add.a.mux = V3D_QPU_MUX_A; i.alu.add.b.mux = V3D_QPU_MUX_B;
    i.raddr_a = a_rf; i.raddr_b = b_rf;
    i.alu.add.waddr = dst_rf; i.alu.add.magic_write = false;
    return i;
}

/* single-operand add-unit float op: <op> rf<dst>, rf<src>  (FFLOOR / FTOIZ). */
static struct v3d_qpu_instr add_unary_rf(enum v3d_qpu_add_op op, int dst_rf, int src_rf) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.add.op = op;
    i.alu.add.a.mux = V3D_QPU_MUX_A; i.alu.add.b.mux = V3D_QPU_MUX_A;
    i.raddr_a = src_rf;
    i.alu.add.waddr = dst_rf; i.alu.add.magic_write = false;
    return i;
}

/* stvpmv -, rf<off>, rf<val>  — store VPM vector at explicit offset (ver-42
 * output op). NODST: the packer forces waddr=0 / no-magic-write, so
 * magic_write MUST be false. src0 = offset (mux A / raddr_a), src1 = value
 * (mux B / raddr_b), matching vir_STVPMV(c, offset, val) + v3d42_set_src. */
static struct v3d_qpu_instr stvpmv_rf(int off_rf, int val_rf) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.add.op = V3D_QPU_A_STVPMV;
    i.alu.add.a.mux = V3D_QPU_MUX_A; i.alu.add.b.mux = V3D_QPU_MUX_B;
    i.raddr_a = off_rf; i.raddr_b = val_rf;
    i.alu.add.waddr = V3D_QPU_WADDR_NOP; i.alu.add.magic_write = false;
    return i;
}

/* ldvpmv_in rf<dst>, rf5 ; ldunifrf.rf5  — read one VPM input word into rf<dst>
 * and reload the read-offset into rf5 from the uniform FIFO. */
static struct v3d_qpu_instr ldvpmv_in_rf(int dst_rf) {
    struct v3d_qpu_instr r = base_nop();
    r.alu.add.op = V3D_QPU_A_LDVPMV_IN;
    r.alu.add.a.mux = V3D_QPU_MUX_A;
    r.alu.add.b.mux = V3D_QPU_MUX_R0;
    r.raddr_a = 5;
    r.alu.add.waddr = dst_rf;
    r.alu.add.magic_write = false;
    add_ldunifrf(&r, 5);
    return r;
}

/* nop ; ldunifrf.rf<idx>  — pop one uniform into rf<idx>. */
static struct v3d_qpu_instr ldunif_into(int idx) {
    struct v3d_qpu_instr i = base_nop();
    add_ldunifrf(&i, idx);
    return i;
}

int main(void) {
    /* ---- self-test: reproduce canonical Mesa qpu_disasm.c test vectors ---- */
    printf("== packer self-test vs Mesa qpu_disasm.c vectors (ver 42) ==\n");
    expect("nop", pack_checked("nop (canonical)", base_nop()), 0x3c003186bb800000ull);
    {   struct v3d_qpu_instr m = mov_magic(V3D_QPU_WADDR_VPM, V3D_QPU_MUX_R3);
        m.alu.add.op = V3D_QPU_A_OR;
        m.alu.add.a.mux = V3D_QPU_MUX_R3; m.alu.add.b.mux = V3D_QPU_MUX_R3;
        m.alu.add.waddr = 0; m.alu.add.magic_write = false;
        expect("or rf0,r3,r3;mov vpm,r3", pack_checked("or rf0, r3, r3 ; mov vpm, r3", m),
               0x3c002380b6edb000ull);
    }
    {   struct v3d_qpu_instr f = base_nop();
        f.sig.thrsw = true;
        f.alu.add.op = V3D_QPU_A_FADD;
        f.alu.add.a.mux = V3D_QPU_MUX_R1; f.alu.add.b.mux = V3D_QPU_MUX_R5;
        f.alu.add.waddr = 1; f.alu.add.magic_write = true;
        expect("fadd r1,r1,r5;nop;thrsw", pack_checked("fadd r1, r1, r5 ; thrsw", f), 0x3c20318105829000ull);
    }

    /* ===== M5 gradient VERTEX shader — RENDER path, STVPMV output ===== */
    /* Registers: rf0..rf3 = clip Xc,Yc,Zc,Wc (attribute 0 vec4 from VPM input);
     *            rf5      = VPM read-offset uniform (reused by ldvpmv_in);
     *            rf6      = 8192.0f viewport XY scale (vp_scale);
     *            rf7 = Xs scratch; rf8 = Ys scratch;
     *            rf10..rf13 = colour Rc,Gc,Bc,Ac (attribute 1 vec4 from VPM input);
     *            rf14 = viewport_z_scale (0.5f); rf15 = viewport_z_offset (0.5f);
     *            rf16 = Zs scratch;
     *            rf20..rf27 = output VPM offsets 0..7 (ui uniforms).
     * Uniform FIFO (19 words):
     *   [in-off 0..7, 8192.0f, z_scale 0.5f, z_offset 0.5f, out-off 0..7].
     * The eight ldvpmv_in pop in-off 0..7 (via ldunifrf.rf5); then ldunifrf pops
     * 8192.0 -> rf6, z_scale -> rf14, z_offset -> rf15, out-off 0..7 -> rf20..27. */
    printf("\n== M5 gradient VERTEX shader (OFF_M5_VS_CODE), STVPMV render-path output ==\n");

    /* [0..3] read the vec4 clip position into rf0..rf3. */
    { const char *nm[4] = {"Xc","Yc","Zc","Wc"};
      for (int k = 0; k < 4; k++) {
          char lbl[80]; snprintf(lbl, sizeof lbl,
              "ldvpmv_in rf%d, rf5 ; ldunifrf.rf5  (pos.%s)", k, nm[k]);
          pack_checked(lbl, ldvpmv_in_rf(k));
      } }
    /* [4..7] read the vec4 colour into rf10..rf13 (skip rf4/rf5: rf5 is live). */
    { const char *nm[4] = {"r","g","b","a"};
      for (int k = 0; k < 4; k++) {
          char lbl[80]; snprintf(lbl, sizeof lbl,
              "ldvpmv_in rf%d, rf5 ; ldunifrf.rf5  (col.%s)", 10 + k, nm[k]);
          pack_checked(lbl, ldvpmv_in_rf(10 + k));
      } }
    /* [8] 8192.0 XY scale -> rf6; [9] z_scale -> rf14; [10] z_offset -> rf15. */
    pack_checked("nop ; ldunifrf.rf6    (rf6 <- 8192.0f vp_scale)", ldunif_into(6));
    pack_checked("nop ; ldunifrf.rf14   (rf14 <- viewport_z_scale 0.5f)", ldunif_into(14));
    pack_checked("nop ; ldunifrf.rf15   (rf15 <- viewport_z_offset 0.5f)", ldunif_into(15));
    /* [11..18] output VPM offsets 0..7 -> rf20..rf27. */
    for (int k = 0; k < 8; k++) {
        char lbl[64]; snprintf(lbl, sizeof lbl, "nop ; ldunifrf.rf%d   (out-offset %d)", 20 + k, k);
        pack_checked(lbl, ldunif_into(20 + k));
    }
    /* [19..21] Xs = f2i32(floor(Xc * 8192)).  W=1 -> no 1/Wc. */
    pack_checked("fmul rf7, rf0, rf6   (Xc * 8192.0 ; W=1 so no 1/Wc)", fmul_rf(7, 0, 6));
    pack_checked("ffloor rf7, rf7      (floor, ver==42 path)", add_unary_rf(V3D_QPU_A_FFLOOR, 7, 7));
    pack_checked("ftoiz rf7, rf7       (f2i32)", add_unary_rf(V3D_QPU_A_FTOIZ, 7, 7));
    /* [22..24] Ys = f2i32(floor(Yc * 8192)). */
    pack_checked("fmul rf8, rf1, rf6   (Yc * 8192.0)", fmul_rf(8, 1, 6));
    pack_checked("ffloor rf8, rf8      (floor, ver==42 path)", add_unary_rf(V3D_QPU_A_FFLOOR, 8, 8));
    pack_checked("ftoiz rf8, rf8       (f2i32)", add_unary_rf(V3D_QPU_A_FTOIZ, 8, 8));
    /* [25..26] Zs = Zc*z_scale + z_offset.  W=1 -> rcp_wc factor dropped. */
    pack_checked("fmul rf16, rf2, rf14 (Zc * z_scale ; W=1 so no 1/Wc)", fmul_rf(16, 2, 14));
    pack_checked("fadd rf16, rf16, rf15 (+ z_offset -> Zs)", fadd_rf(16, 16, 15));
    /* [27..28] store screen position Xs @0, Ys @1. */
    pack_checked("stvpmv rf20, rf7   (out0 screen Xs)", stvpmv_rf(20, 7));
    pack_checked("stvpmv rf21, rf8   (out1 screen Ys)", stvpmv_rf(21, 8));
    /* [29] store screen depth Zs @2. */
    pack_checked("stvpmv rf22, rf16  (out2 screen Zs)", stvpmv_rf(22, 16));
    /* [30] store 1/Wc @3 (= Wc = rf3 = 1.0 for W=1). */
    pack_checked("stvpmv rf23, rf3   (out3 1/Wc = Wc = 1.0, W=1)", stvpmv_rf(23, 3));
    /* [31..34] store the four colour varyings @4..7. */
    { const char *nm[4] = {"r","g","b","a"};
      for (int k = 0; k < 4; k++) {
          char lbl[72]; snprintf(lbl, sizeof lbl, "stvpmv rf%d, rf%d   (out%d varying col.%s)",
              24 + k, 10 + k, 4 + k, nm[k]);
          pack_checked(lbl, stvpmv_rf(24 + k, 10 + k));
      } }
    /* [35] GFXH-1684: VPM writes complete before end. */
    { struct v3d_qpu_instr w = base_nop(); w.alu.add.op = V3D_QPU_A_VPMWT;
      pack_checked("vpmwt   (GFXH-1684: VPM writes complete)", w); }
    /* [36] end thread; [37..38] delay-slot nops. */
    { struct v3d_qpu_instr t = base_nop(); t.sig.thrsw = true;
      pack_checked("nop ; thrsw   (end)", t); }
    pack_checked("nop", base_nop());
    pack_checked("nop", base_nop());

    printf("\nAll words packed + round-tripped OK.\n");
    return 0;
}
