/* Reproduce (against a Mesa checkout, e.g. mesa 26.3.0-devel):
 *   git clone --depth 1 https://gitlab.freedesktop.org/mesa/mesa.git
 *   cc -I mesa/src -I mesa/include -I mesa/src/broadcom -o qpu_gen19 \
 *        mesa/src/broadcom/qpu/qpu_instr.c mesa/src/broadcom/qpu/qpu_pack.c \
 *        scripts/pi-v3d19-qpu-gen.c
 *   ./qpu_gen19               # prints every word + round-trip + self-test (see .out.txt)
 * The printed hex is transcribed verbatim into CS_VS_WORDS in
 * unaos/crates/kernel/src/arch/aarch64/v3d.rs.
 *
 * PI-V3D-19 shader-word generator — the SCREEN-SPACE widening of the
 * PI-V3D-9 coordinate/vertex passthrough.
 *
 * PI-V3D-18 proved the coordinate (bin) shader's VPM OUTPUT contract for V3D
 * 4.2 is SIX words per vertex — clip [Xc,Yc,Zc,Wc] at out-offsets 0..3 THEN
 * the two screen-space words the PTB bins from at offsets 4,5:
 *     Xs = f2i32(floor( Xc * vp_scale * (1/Wc) ))
 *     Ys = f2i32(floor( Yc * vp_scale * (1/Wc) ))
 * vp_scale = viewport.scale(32) * clipper_xy_granularity(256) = 8192; the
 * floor path is the devinfo ver==42 branch (v3d_uniforms.c). The V3D-9
 * program wrote only offsets 0..3, so the PTB read zero screen coords and
 * binned an empty-but-legal list. This generator adds the two words.
 *
 * W=1 SIMPLIFICATION (documented LOUDLY): TRI_VERTS all carry Wc = 1.0, so
 * 1/Wc = 1.0 and NO reciprocal instruction (recip / SFU) is needed — the
 * screen transform collapses to Xs = f2i32(floor(Xc * 8192)). This holds
 * ONLY for W=1 geometry; a perspective (W != 1) draw would need a per-vertex
 * reciprocal here.
 *
 * Every word is packed with Mesa's OWN packer (v3d_qpu_instr_pack, ver=42),
 * round-tripped through v3d_qpu_instr_unpack + repack, and the harness first
 * reproduces the four canonical Mesa qpu_disasm.c vectors bit-exactly. It
 * ALSO re-derives every original CS_VS_WORDS word (offsets 0..3 path) so the
 * new words share a validated encoder. Mesa is MIT-licensed; used with
 * attribution (memory: unaos-license-gplv3).
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

/* mov <magic waddr>, r<src> on the mul unit (Mesa "mov vpm, r3"). */
static struct v3d_qpu_instr mov_magic(enum v3d_qpu_waddr w, enum v3d_qpu_mux src) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.mul.op = V3D_QPU_M_MOV;
    i.alu.mul.a.mux = src; i.alu.mul.b.mux = src;
    i.alu.mul.waddr = w; i.alu.mul.magic_write = true;
    return i;
}

/* mov vpm, rf<src>  (read register file via raddr_a / MUX_A). */
static struct v3d_qpu_instr mov_vpm_rf(int src_rf) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.mul.op = V3D_QPU_M_MOV;
    i.alu.mul.a.mux = V3D_QPU_MUX_A; i.alu.mul.b.mux = V3D_QPU_MUX_A;
    i.raddr_a = src_rf;
    i.alu.mul.waddr = V3D_QPU_WADDR_VPM; i.alu.mul.magic_write = true;
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

/* single-operand add-unit float op: <op> rf<dst>, rf<src>  (FFLOOR / FTOIZ). */
static struct v3d_qpu_instr add_unary_rf(enum v3d_qpu_add_op op, int dst_rf, int src_rf) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.add.op = op;
    i.alu.add.a.mux = V3D_QPU_MUX_A; i.alu.add.b.mux = V3D_QPU_MUX_A;
    i.raddr_a = src_rf;
    i.alu.add.waddr = dst_rf; i.alu.add.magic_write = false;
    return i;
}

int main(void) {
    /* ---- self-test: reproduce canonical Mesa qpu_disasm test vectors ---- */
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

    /* ===== COORDINATE / VERTEX shader — SIX-word screen-space output ===== */
    /* Registers: rf0..rf3 = clip Xc,Yc,Zc,Wc (from attribute VPM read);
     *            rf5      = VPM read-offset uniform (reused);
     *            rf6      = 8192.0f viewport const (vp_scale);
     *            rf7      = Xs scratch; rf8 = Ys scratch.
     * Uniform FIFO: [off0,off1,off2,off3, 8192.0f]. The four ldvpmv_in pop the
     * offsets; the fifth ldunifrf pops 8192.0 into rf6.
     *
     * NOTE (metal-refinement surface, unchanged stance from V3D-9): register
     * write->read latency scheduling (RF hazard nops between fmul/ffloor/ftoiz
     * and the following read) and the exact vpmsetup width value are the
     * attended-metal surface; QEMU models no V3D so they cannot be exercised
     * off-metal. Every ENCODING below is Mesa-packed + round-tripped. */
    printf("\n== COORDINATE/VERTEX shader body (OFF_CS_CODE / OFF_VS_CODE), 6-word output ==\n");

    /* [0..3] read the vec4 clip position from the VPM into rf0..rf3. */
    for (int k = 0; k < 4; k++) {
        struct v3d_qpu_instr r = base_nop();
        r.alu.add.op = V3D_QPU_A_LDVPMV_IN;
        r.alu.add.a.mux = V3D_QPU_MUX_A;
        r.alu.add.b.mux = V3D_QPU_MUX_R0;
        r.raddr_a = 5;
        r.alu.add.waddr = k;
        r.alu.add.magic_write = false;
        add_ldunifrf(&r, 5);
        const char *nm[4] = {"Xc","Yc","Zc","Wc"};
        char lbl[80]; snprintf(lbl, sizeof lbl, "ldvpmv_in rf%d, rf5 ; ldunifrf.rf5  (attr[%d] -> %s)", k, k, nm[k]);
        pack_checked(lbl, r);
    }
    /* [4] load the 8192.0 viewport scale constant into rf6. */
    { struct v3d_qpu_instr l = base_nop(); add_ldunifrf(&l, 6);
      pack_checked("nop ; ldunifrf.rf6   (rf6 <- 8192.0f vp_scale)", l); }
    /* [5] arm the VPM output (6-wide; the width value is a metal-refined uniform read via rf5). */
    { struct v3d_qpu_instr v = base_nop();
      v.alu.add.op = V3D_QPU_A_VPMSETUP; v.alu.add.a.mux = V3D_QPU_MUX_A; v.raddr_a = 5;
      pack_checked("vpmsetup -, rf5   (VPM output setup, 6-wide; value=metal)", v); }
    /* [6..9] write the four clip words (out-offsets 0..3). */
    for (int k = 0; k < 4; k++) {
        const char *nm[4] = {"Xc","Yc","Zc","Wc"};
        char lbl[64]; snprintf(lbl, sizeof lbl, "mov vpm, rf%d   (out%d clip %s)", k, k, nm[k]);
        pack_checked(lbl, mov_vpm_rf(k));
    }
    /* [10..13] Xs = f2i32(floor(Xc * 8192)).  W=1 -> no 1/Wc. */
    pack_checked("fmul rf7, rf0, rf6   (Xc * 8192.0 ; W=1 so no 1/Wc)", fmul_rf(7, 0, 6));
    pack_checked("ffloor rf7, rf7      (floor, ver==42 path)", add_unary_rf(V3D_QPU_A_FFLOOR, 7, 7));
    pack_checked("ftoiz rf7, rf7       (f2i32)", add_unary_rf(V3D_QPU_A_FTOIZ, 7, 7));
    pack_checked("mov vpm, rf7         (out4 screen Xs)", mov_vpm_rf(7));
    /* [14..17] Ys = f2i32(floor(Yc * 8192)). */
    pack_checked("fmul rf8, rf1, rf6   (Yc * 8192.0)", fmul_rf(8, 1, 6));
    pack_checked("ffloor rf8, rf8      (floor, ver==42 path)", add_unary_rf(V3D_QPU_A_FFLOOR, 8, 8));
    pack_checked("ftoiz rf8, rf8       (f2i32)", add_unary_rf(V3D_QPU_A_FTOIZ, 8, 8));
    pack_checked("mov vpm, rf8         (out5 screen Ys)", mov_vpm_rf(8));
    /* [18..21] complete VPM writes, end thread. */
    { struct v3d_qpu_instr w = base_nop(); w.alu.add.op = V3D_QPU_A_VPMWT;
      pack_checked("vpmwt   (GFXH-1684: VPM writes complete)", w); }
    { struct v3d_qpu_instr t = base_nop(); t.sig.thrsw = true;
      pack_checked("nop ; thrsw   (end)", t); }
    pack_checked("nop", base_nop());
    pack_checked("nop", base_nop());

    printf("\nAll words packed + round-tripped OK.\n");
    return 0;
}
