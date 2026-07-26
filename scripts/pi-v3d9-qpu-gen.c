/* Reproduce (against a Mesa checkout, e.g. mesa 26.3.0-devel):
 *   git clone --depth 1 https://gitlab.freedesktop.org/mesa/mesa.git
 *   cc -I mesa/src -I mesa/include -o qpu_gen \
 *        mesa/src/broadcom/qpu/qpu_instr.c mesa/src/broadcom/qpu/qpu_pack.c \
 *        scripts/pi-v3d9-qpu-gen.c
 *   ./qpu_gen                 # prints every word + round-trip + self-test (see .out.txt)
 * The printed hex is transcribed verbatim into FS_WORDS / CS_VS_WORDS in
 * unaos/crates/kernel/src/arch/aarch64/v3d.rs.
 *
 * PI-V3D-9 shader-word generator.
 * Builds explicit struct v3d_qpu_instr sequences for the three minimal
 * triangle shaders and emits the 64-bit words using Mesa's OWN packer
 * (v3d_qpu_instr_pack, ver=42). Every word is round-tripped through
 * v3d_qpu_instr_unpack and repacked; a mismatch aborts. The harness also
 * reproduces four canonical vectors from Mesa's qpu_disasm test suite to
 * prove the struct->word path matches Mesa's documented semantics.
 *
 * This file IS the documented provenance for the words transcribed into
 * unaos/.../v3d.rs. Mesa is MIT-licensed; used with attribution.
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

/* mov <magic waddr>, r<src> on the mul unit (matches Mesa "mov vpm, r3"). */
static struct v3d_qpu_instr mov_magic(enum v3d_qpu_waddr w, enum v3d_qpu_mux src) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.mul.op = V3D_QPU_M_MOV;
    i.alu.mul.a.mux = src; i.alu.mul.b.mux = src;
    i.alu.mul.waddr = w; i.alu.mul.magic_write = true;
    return i;
}

/* vfpack <magic waddr>, r<a>, r<b> on the add unit (Mesa "vfpack tlb, r0, r1"). */
static struct v3d_qpu_instr vfpack_magic(enum v3d_qpu_waddr w, enum v3d_qpu_mux a, enum v3d_qpu_mux b) {
    struct v3d_qpu_instr i = base_nop();
    i.alu.add.op = V3D_QPU_A_VFPACK;
    i.alu.add.a.mux = a; i.alu.add.b.mux = b;
    i.alu.add.waddr = w; i.alu.add.magic_write = true;
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
    /* round-trip: unpack and repack, require identical word */
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

int main(void) {
    /* ---- self-test: reproduce canonical Mesa qpu_disasm test vectors ---- */
    printf("== packer self-test vs Mesa qpu_disasm.c vectors (ver 42) ==\n");
    expect("nop", pack_checked("nop (canonical)", base_nop()), 0x3c003186bb800000ull);
    {   /* Mesa vector is dual-issued: or rf0, r3, r3 ; mov vpm, r3 */
        struct v3d_qpu_instr m = mov_magic(V3D_QPU_WADDR_VPM, V3D_QPU_MUX_R3);
        m.alu.add.op = V3D_QPU_A_OR;
        m.alu.add.a.mux = V3D_QPU_MUX_R3; m.alu.add.b.mux = V3D_QPU_MUX_R3;
        m.alu.add.waddr = 0; m.alu.add.magic_write = false;
        expect("or rf0,r3,r3;mov vpm,r3", pack_checked("or rf0, r3, r3 ; mov vpm, r3", m),
               0x3c002380b6edb000ull);
    }
    expect("vfpack tlb, r0, r1",
           pack_checked("vfpack tlb, r0, r1", vfpack_magic(V3D_QPU_WADDR_TLB, V3D_QPU_MUX_R0, V3D_QPU_MUX_R1)),
           0x3c00318735808000ull);
    {   /* fadd r1, r1, r5 ; nop ; thrsw = 0x3c20318105829000 (r1 = accumulator, magic waddr 1) */
        struct v3d_qpu_instr f = base_nop();
        f.sig.thrsw = true;
        f.alu.add.op = V3D_QPU_A_FADD;
        f.alu.add.a.mux = V3D_QPU_MUX_R1; f.alu.add.b.mux = V3D_QPU_MUX_R5;
        f.alu.add.waddr = 1; f.alu.add.magic_write = true;
        expect("fadd r1,r1,r5;nop;thrsw", pack_checked("fadd r1, r1, r5 ; thrsw", f), 0x3c20318105829000ull);
    }

    /* ================= FRAGMENT SHADER (solid colour) ================= */
    /* Following Mesa emit_frag_end + vir_emit_tlb_color_write (F16 RT0,
     * single-thread, passthrough Z). Uniform FIFO order (consumed in
     * sequence): r,g,b,a (via ldunifrf), then Z-config (tlbu-Z pop), then
     * colour-config (vfpack-tlbu pop). */
    printf("\n== FRAGMENT shader (OFF_FS_CODE) ==\n");
    struct v3d_qpu_instr l0 = base_nop(); add_ldunifrf(&l0, 0);
    struct v3d_qpu_instr l1 = base_nop(); add_ldunifrf(&l1, 1);
    struct v3d_qpu_instr l2 = base_nop(); add_ldunifrf(&l2, 2);
    struct v3d_qpu_instr l3 = base_nop(); add_ldunifrf(&l3, 3);
    pack_checked("nop ; ldunifrf.rf0   (rf0 <- colour.r)", l0);
    pack_checked("nop ; ldunifrf.rf1   (rf1 <- colour.g)", l1);
    pack_checked("nop ; ldunifrf.rf2   (rf2 <- colour.b)", l2);
    pack_checked("nop ; ldunifrf.rf3   (rf3 <- colour.a)", l3);
    /* Z passthrough: mov tlbu, <nop-src> pops the Z-config uniform. */
    pack_checked("mov tlbu, r0   (passthrough-Z; pops Z-config)", mov_magic(V3D_QPU_WADDR_TLBU, V3D_QPU_MUX_R0));
    /* colour rg -> tlbu (pops colour-config), ba -> tlb.  rf reads use MUX_A;
     * need raddr_a to select which rf. VFPACK a/b from rf0..rf3 requires the
     * mux to be A and raddr set; emit via MUX_A with raddr below. */
    {
        struct v3d_qpu_instr rg = base_nop();
        rg.alu.add.op = V3D_QPU_A_VFPACK;
        rg.alu.add.a.mux = V3D_QPU_MUX_A;  /* rf via raddr_a */
        rg.alu.add.b.mux = V3D_QPU_MUX_B;  /* rf via raddr_b */
        rg.raddr_a = 0; rg.raddr_b = 1;
        rg.alu.add.waddr = V3D_QPU_WADDR_TLBU; rg.alu.add.magic_write = true;
        pack_checked("vfpack tlbu, rf0, rf1   (colour r,g; pops colour-config)", rg);
        struct v3d_qpu_instr ba = base_nop();
        ba.alu.add.op = V3D_QPU_A_VFPACK;
        ba.alu.add.a.mux = V3D_QPU_MUX_A;
        ba.alu.add.b.mux = V3D_QPU_MUX_B;
        ba.raddr_a = 2; ba.raddr_b = 3;
        ba.alu.add.waddr = V3D_QPU_WADDR_TLB; ba.alu.add.magic_write = true;
        pack_checked("vfpack tlb, rf2, rf3    (colour b,a)", ba);
    }
    { struct v3d_qpu_instr t = base_nop(); t.sig.thrsw = true;
      pack_checked("nop ; thrsw   (last thread switch)", t); }
    pack_checked("nop", base_nop());
    pack_checked("nop", base_nop());

    /* ================= COORDINATE / VERTEX SHADER ================= */
    /* Minimal position passthrough: read 4 attribute components from the VPM
     * (ldvpmv_in with a uniform offset) into rf0..rf3, write 4 position
     * components to the output VPM (mov vpm, r), vpmwt, end.  Structurally
     * faithful to Mesa ntq_emit_vpm_read + emit_store_output_vs + emit_vert_end;
     * viewport transform / exact output layout are the metal-refinement surface. */
    printf("\n== COORDINATE/VERTEX shader body (OFF_CS_CODE / OFF_VS_CODE) ==\n");
    for (int k = 0; k < 4; k++) {
        struct v3d_qpu_instr r = base_nop();
        r.alu.add.op = V3D_QPU_A_LDVPMV_IN;
        r.alu.add.a.mux = V3D_QPU_MUX_A;  /* offset from rf (uniform-loaded) */
        r.alu.add.b.mux = V3D_QPU_MUX_R0;
        r.raddr_a = 5;                    /* rf5 holds the VPM offset */
        r.alu.add.waddr = k;              /* -> rf0..rf3 (ldvpmv_in cannot target accumulators) */
        r.alu.add.magic_write = false;
        add_ldunifrf(&r, 5);              /* also (re)load offset uniform -> rf5 */
        char lbl[64]; snprintf(lbl, sizeof lbl, "ldvpmv_in rf%d, rf5 ; ldunifrf.rf5  (attr[%d])", k, k);
        pack_checked(lbl, r);
    }
    { struct v3d_qpu_instr v = base_nop();
      v.alu.add.op = V3D_QPU_A_VPMSETUP; v.alu.add.a.mux = V3D_QPU_MUX_A; v.raddr_a = 5;
      pack_checked("vpmsetup -, rf5   (VPM output setup; value=metal)", v); }
    for (int k = 0; k < 4; k++) {
        struct v3d_qpu_instr m = base_nop();
        m.alu.mul.op = V3D_QPU_M_MOV;
        m.alu.mul.a.mux = V3D_QPU_MUX_A; m.alu.mul.b.mux = V3D_QPU_MUX_A;
        m.raddr_a = k;                    /* read rf0..rf3 */
        m.alu.mul.waddr = V3D_QPU_WADDR_VPM; m.alu.mul.magic_write = true;
        const char *nm[4] = {"pos.x","pos.y","pos.z","pos.w / 1/Wc"};
        char lbl[48]; snprintf(lbl, sizeof lbl, "mov vpm, rf%d  (%s)", k, nm[k]);
        pack_checked(lbl, m);
    }
    { struct v3d_qpu_instr w = base_nop(); w.alu.add.op = V3D_QPU_A_VPMWT;
      pack_checked("vpmwt   (GFXH-1684: VPM writes complete)", w); }
    { struct v3d_qpu_instr t = base_nop(); t.sig.thrsw = true;
      pack_checked("nop ; thrsw   (end)", t); }
    pack_checked("nop", base_nop());
    pack_checked("nop", base_nop());
    printf("\nAll words packed + round-tripped OK.\n");
    return 0;
}
