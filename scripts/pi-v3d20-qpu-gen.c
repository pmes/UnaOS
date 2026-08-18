/* Reproduce (against a Mesa checkout, e.g. mesa 26.3.0-devel):
 *   git clone --depth 1 https://gitlab.freedesktop.org/mesa/mesa.git
 *   cc -I mesa/src -I mesa/include -I mesa/src/broadcom -o qpu_gen20 \
 *        mesa/src/broadcom/qpu/qpu_instr.c mesa/src/broadcom/qpu/qpu_pack.c \
 *        scripts/pi-v3d20-qpu-gen.c
 *   ./qpu_gen20               # prints every word + round-trip + self-test (see .out.txt)
 * The printed hex is transcribed verbatim into CS_VS_WORDS in
 * unaos/crates/kernel/src/arch/aarch64/v3d.rs.
 *
 * PI-V3D-20 shader-word generator — CORRECTS the coordinate/vertex shader's
 * VPM OUTPUT MECHANISM for V3D 4.2.
 *
 * ROOT CAUSE (this arc): PI-V3D-9/17/18/19 wrote the coordinate-shader VPM
 * output with the *streamed* VC4 / V3D-3.3 mechanism — a `vpmsetup` to arm a
 * VPM output segment, then `mov vpm, rfN` (writes to the magic waddr VPM=14)
 * that auto-advance an implicit write pointer. On V3D 4.x (devinfo->ver==42,
 * the Pi 4 VideoCore VI) that mechanism DOES NOT EXIST for per-vertex shader
 * output. Mesa's V3D compiler proves it: `vir_VPM_WRITE`
 * (src/broadcom/compiler/nir_to_vir.c) emits EXACTLY ONE instruction per output
 * component — `vir_STVPMV(c, vir_uniform_ui(c, vpm_index), val)` — a store-VPM
 * with an EXPLICIT integer VPM offset operand. There is NO `mov vpm` and NO
 * `vpmsetup` anywhere in the ver-42 VS/CS output path (grep the compiler: the
 * only VPM-output ops it emits are STVPMV/STVPMD + a trailing VPMWT for
 * GFXH-1684). `V3D_QPU_WADDR_VPM` (magic waddr 14) is never a write target on
 * 4.x. So every one of our `mov vpm` writes — the four clip words AND the two
 * PI-V3D-19 screen-space words 5,6 — landed on an unconfigured magic register;
 * the PTB read zero screen coords and binned an empty-but-legal list. That is
 * the exact metal observation (pool[0..8]/tile-STATE[0..8] all zero, CL clean),
 * and it explains why NO word-count change (4→6) ever moved the needle: the
 * *addressing form* was wrong the whole time, not the number of words.
 *
 * THE FIX: emit output with STVPMV + explicit per-component offsets. The six
 * offsets 0..5 arrive as uniforms (Mesa-faithful: `vir_uniform_ui`) and are
 * loaded into rf9..rf14 via ldunifrf; each `stvpmv rf<off>, rf<val>` then
 * stores value at its offset. `vpmsetup` is DROPPED (not used on 4.x VS/CS
 * output). VPMWT stays (GFXH-1684). The input side (ldvpmv_in) and the Xs/Ys
 * math (fmul·8192 → ffloor → ftoiz; W=1 so no 1/Wc) are the validated V3D-19
 * body, carried verbatim.
 *
 * ISA-VERSION NOTE (task-2 audit): `vpmsetup` (V3D_QPU_A_VPMSETUP, opcode 187,
 * first_ver 33) DOES pack on ver 42 — it is a real instruction — but on 4.x it
 * arms VPM *DMA* descriptors (ldvpm/stvpm DMA), not the per-vertex shader
 * output stream, so our committed vpmsetup word was configuring an irrelevant
 * DMA channel while the `mov vpm` writes went nowhere the PTB reads. STVPMV
 * (opcode 248, add A|B, ver-42 table `v3d42_add_ops` in qpu_pack.c) is the
 * ver-42 output op; its packer sets waddr=0 / no_magic_write, so add.magic_write
 * MUST be false and the offset/value are plain register-file reads on mux A/B.
 *
 * W=1 SIMPLIFICATION (documented LOUDLY): TRI_VERTS all carry Wc = 1.0, so
 * 1/Wc = 1.0 and NO reciprocal instruction (recip / SFU) is needed — the
 * screen transform collapses to Xs = f2i32(floor(Xc * 8192)). Perspective
 * (W != 1) geometry would need a per-vertex reciprocal here.
 *
 * Every word is packed with Mesa's OWN packer (v3d_qpu_instr_pack, ver=42),
 * round-tripped through v3d_qpu_instr_unpack + repack, and the harness first
 * reproduces the four canonical Mesa qpu_disasm.c vectors bit-exactly. Mesa is
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

    /* ===== COORDINATE / VERTEX shader — STVPMV six-word screen-space output ===== */
    /* Registers: rf0..rf3 = clip Xc,Yc,Zc,Wc (from attribute VPM read);
     *            rf5      = VPM read-offset uniform (reused by ldvpmv_in);
     *            rf6      = 8192.0f viewport const (vp_scale);
     *            rf7      = Xs scratch; rf8 = Ys scratch;
     *            rf9..rf14 = output VPM offsets 0..5 (ui uniforms).
     * Uniform FIFO: [off0..3, 8192.0f, outoff0..5] = 11 words. The four ldvpmv_in
     * pop off0..3; the fifth ldunifrf pops 8192.0 into rf6; six ldunifrf pop the
     * output offsets 0..5 into rf9..rf14. */
    printf("\n== COORDINATE/VERTEX shader body (OFF_CS_CODE / OFF_VS_CODE), STVPMV 6-word output ==\n");

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
    pack_checked("nop ; ldunifrf.rf6    (rf6 <- 8192.0f vp_scale)", ldunif_into(6));
    /* [5..10] load the six output VPM offsets 0..5 into rf9..rf14. */
    for (int k = 0; k < 6; k++) {
        char lbl[64]; snprintf(lbl, sizeof lbl, "nop ; ldunifrf.rf%d   (out-offset %d)", 9 + k, k);
        pack_checked(lbl, ldunif_into(9 + k));
    }
    /* [11..13] Xs = f2i32(floor(Xc * 8192)).  W=1 -> no 1/Wc. */
    pack_checked("fmul rf7, rf0, rf6   (Xc * 8192.0 ; W=1 so no 1/Wc)", fmul_rf(7, 0, 6));
    pack_checked("ffloor rf7, rf7      (floor, ver==42 path)", add_unary_rf(V3D_QPU_A_FFLOOR, 7, 7));
    pack_checked("ftoiz rf7, rf7       (f2i32)", add_unary_rf(V3D_QPU_A_FTOIZ, 7, 7));
    /* [14..16] Ys = f2i32(floor(Yc * 8192)). */
    pack_checked("fmul rf8, rf1, rf6   (Yc * 8192.0)", fmul_rf(8, 1, 6));
    pack_checked("ffloor rf8, rf8      (floor, ver==42 path)", add_unary_rf(V3D_QPU_A_FFLOOR, 8, 8));
    pack_checked("ftoiz rf8, rf8       (f2i32)", add_unary_rf(V3D_QPU_A_FTOIZ, 8, 8));
    /* [17..20] store the four clip words at out-offsets 0..3 (rf9..rf12). */
    { const char *nm[4] = {"Xc","Yc","Zc","Wc"}; int val[4] = {0,1,2,3};
      for (int k = 0; k < 4; k++) {
          char lbl[72]; snprintf(lbl, sizeof lbl, "stvpmv rf%d, rf%d   (out%d clip %s)", 9 + k, val[k], k, nm[k]);
          pack_checked(lbl, stvpmv_rf(9 + k, val[k]));
      } }
    /* [21] store screen Xs at out-offset 4 (rf13); [22] screen Ys at out-offset 5 (rf14). */
    pack_checked("stvpmv rf13, rf7   (out4 screen Xs)", stvpmv_rf(13, 7));
    pack_checked("stvpmv rf14, rf8   (out5 screen Ys)", stvpmv_rf(14, 8));
    /* [23] GFXH-1684: VPM writes complete before end. */
    { struct v3d_qpu_instr w = base_nop(); w.alu.add.op = V3D_QPU_A_VPMWT;
      pack_checked("vpmwt   (GFXH-1684: VPM writes complete)", w); }
    /* [24] end thread; [25..26] delay-slot nops. */
    { struct v3d_qpu_instr t = base_nop(); t.sig.thrsw = true;
      pack_checked("nop ; thrsw   (end)", t); }
    pack_checked("nop", base_nop());
    pack_checked("nop", base_nop());

    printf("\nAll words packed + round-tripped OK.\n");
    return 0;
}
