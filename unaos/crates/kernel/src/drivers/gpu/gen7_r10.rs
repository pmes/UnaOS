//! GEN7R10 (B638, R121) — rung R10: the FIRST 3D DRAW on the Ivy Bridge RCS — one triangle, read back by the CPU.
//! CHARTER: Kernel — driver
//!
//! Design and the filled row table: `docs/dev/evidence/gen7r10/GEN7R10.md` (every command -> dwords -> PRM page).
//! A child of `gen7_r9.rs` (declared by `#[path]` there, as `mod draw`), run by R9's `run` right after R9's own
//! witness line, under `tests gen7` only (R80: nothing here runs at boot). Gated on R9 `verified` THIS run.
//!
//! THE RUNG: on R9's envelope (the RCS armed under R6's `mt` hold, one-page ring, its own proven-unowned GGTT
//! window) a ~230-dword stream selects the 3D pipe, points every base at R10's pages, programs the whole gen7 3D
//! front end with VS/HS/TE/DS/GS/SO/CLIP in pass-through, a null depth buffer, one 32x32 B8G8R8A8_UNORM render
//! target, a four-instruction SIMD16 solid-colour pixel shader, and draws ONE TRILIST triangle (0,0)-(32,0)-(0,32)
//! in screen space. A post-sync PIPE_CONTROL writes a fence after the render-target flush; the CPU then reads the
//! render target back and counts the pixels the GPU wrote.
//!
//! SOURCES (R83 / R95 §2): Intel's 2012 open-source Ivy Bridge PRM, fetched by the bench from
//! xorg.freedesktop.org/docs/intel/IVB/ (sha256s in GEN7R10.md §1). Keys: `v1p1` = IHD_OS_Vol1_Part1.pdf,
//! `v2p1` = IHD_OS_Vol2_Part1.pdf, `v4p1` = IHD_OS_Vol4_Part1.pdf, `v4p3` = IHD_OS_Vol4_Part3.pdf, each `:page`
//! (printed page = PDF page in all four). `rs` = i915 gt/gen7_renderstate.c (linux 24a19b95, SPDX MIT) — the raw
//! gen7 null-state batch, each literal paired here to its PRM command and fields; `rc` = gt/gen7_renderclear.c;
//! `cs` = gt/gen6_engine_cs.c; `mx` = mesa src/intel/genxml/gen70.xml (corroboration of the topology enum only).
//! Values only; no text is copied. A row whose `src` is `None` DECLINES the rung by name and nothing is written.

use super::*;

/// R10's rows in command-stream order (`GEN7R10.md` §2 carries the same rows with every dword and bit field).
const R10_PAGES: [Page; 37] = [
    Page { name: "PIPELINE_SELECT", field: "select", owed: "IVB-PRM-Vol1-Pt1", src: Some("v1p1:19,33 rs:19") },
    Page { name: "STATE_BASE_ADDRESS", field: "sba", owed: "IVB-PRM-Vol1-Pt1", src: Some("v1p1:22-27 rs:20-29") },
    Page { name: "3DSTATE_MULTISAMPLE", field: "ms", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:304-305 rs:30-33") },
    Page { name: "3DSTATE_SAMPLE_MASK", field: "smask", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:293-294 rs:34-35") },
    Page { name: "PIPE_CONTROL-depth-stall", field: "wa", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:59-65,106,276,315") },
    Page { name: "3DSTATE_PUSH_CONSTANT_ALLOC_VS", field: "pcalloc", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:115-116") },
    Page { name: "3DSTATE_PUSH_CONSTANT_ALLOC_PS", field: "pcalloc_ps", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:292-293 rs:36-37") },
    Page { name: "3DSTATE_URB_VS", field: "urb", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:34-35 rs:38-39") },
    Page { name: "3DSTATE_URB_HS/DS/GS", field: "urb_rest", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:36-40 rs:40-45") },
    Page { name: "3DSTATE_CONSTANT_VS", field: "const_vs", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:112-115") },
    Page { name: "3DSTATE_VS", field: "vs", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:106-111 rs:46-51") },
    Page { name: "VS-passthrough", field: "vs_kernel", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:111 rs:46-51") },
    Page { name: "3DSTATE_HS", field: "hs", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:128-131 rs:52-58") },
    Page { name: "3DSTATE_TE", field: "te", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:141-143 rs:59-62") },
    Page { name: "3DSTATE_DS", field: "ds", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:149-154 rs:63-68") },
    Page { name: "3DSTATE_GS", field: "gs", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:166-173 rs:69-75") },
    Page { name: "3DSTATE_STREAMOUT", field: "so", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:197-199 rs:92-94") },
    Page { name: "3DSTATE_CLIP", field: "clip", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:218-222 rs:76-79") },
    Page { name: "SF_CLIP_VIEWPORT", field: "sfclip", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:32,270-271 rs:80-81") },
    Page { name: "CC_VIEWPORT", field: "ccvp", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:31,373 rs:107-108") },
    Page { name: "3DSTATE_SF", field: "sf", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:257-263 rs:82-88") },
    Page { name: "3DSTATE_SBE", field: "sbe", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:263-269 rs:111-124") },
    Page { name: "3DSTATE_WM", field: "wm", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:276-282 rs:89-91") },
    Page { name: "BLEND_STATE", field: "blend", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:19,366-372 rs:105-106,163-164") },
    Page { name: "COLOR_CALC_STATE", field: "cc", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:18,361-362") },
    Page { name: "DEPTH_STENCIL_STATE", field: "dss", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:20,362-366") },
    Page { name: "SURFACE_STATE", field: "rt", owed: "IVB-PRM-Vol4-Pt1", src: Some("v4p1:62-81 rc:151-168") },
    Page { name: "BINDING_TABLE", field: "bt", owed: "IVB-PRM-Vol4-Pt1", src: Some("v4p1:61 v2p1:25 rs:145-146 rc:183") },
    Page { name: "3DSTATE_CONSTANT_PS", field: "const_ps", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:291") },
    Page { name: "3DSTATE_PS", field: "ps", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:283-290 rs:125-132") },
    Page { name: "EU-pixel-shader", field: "kernel", owed: "IVB-PRM-Vol4-Pt3", src: Some("v4p3:80,96-99,106,110-111,127,129,215-218 v4p1:225,229,280-286 v1p1:18 rs:207-226") },
    Page { name: "3DSTATE_DEPTH_BUFFER", field: "depth", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:315-325 rs:95-104") },
    Page { name: "3DSTATE_BINDING_TABLE_POINTERS_PS", field: "btp", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:25 rs:145-146") },
    Page { name: "3DSTATE_DRAWING_RECTANGLE", field: "rect", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:244-246 rs:147-150") },
    Page { name: "3DSTATE_VERTEX_BUFFERS", field: "vb", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:77-80 rs:140-144") },
    Page { name: "3DSTATE_VERTEX_ELEMENTS", field: "ve", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:84-88 v4p1:79 rs:133-139") },
    Page { name: "3DPRIMITIVE", field: "prim", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:75,90-94 v1p1:33 rs:151-157 mx:25,36") },
];

// ---- Header dwords. GFXPIPE (3<<29), GFXPIPE_3D (3<<27), opcode, sub-opcode, DWord Length = total - 2 ----------
// (every 3DSTATE header page above; v2p1:106 is the first). The const block below proves each against the literal
// i915 carries (rs line in the comment) where the null-state batch has that command.
const fn gfx3d(opcode: u32, sub: u32, total: u32) -> u32 {
    (0x3 << 29) | (0x3 << 27) | (opcode << 24) | (sub << 16) | (total - 2)
}
const MULTISAMPLE: u32 = gfx3d(1, 0x0D, 4); // v2p1:304-305; rs:30 0x790d0002
const SAMPLE_MASK: u32 = gfx3d(0, 0x18, 2); // v2p1:293-294; rs:34 0x78180000
const PCALLOC_VS: u32 = gfx3d(1, 0x12, 2); // v2p1:115
const PCALLOC_PS: u32 = gfx3d(1, 0x16, 2); // v2p1:292; rs:36 0x79160000
const URB_VS: u32 = gfx3d(0, 0x30, 2); // v2p1:34; rs:38
const URB_HS: u32 = gfx3d(0, 0x31, 2); // v2p1:36; rs:40
const URB_DS: u32 = gfx3d(0, 0x32, 2); // v2p1:37; rs:42
const URB_GS: u32 = gfx3d(0, 0x33, 2); // v2p1:39; rs:44
const CONSTANT_VS: u32 = gfx3d(0, 0x15, 7); // v2p1:112
const VS: u32 = gfx3d(0, 0x10, 6); // v2p1:106; rs:46 0x78100004
const HS: u32 = gfx3d(0, 0x1B, 7); // v2p1:128; rs:52
const TE: u32 = gfx3d(0, 0x1C, 4); // v2p1:141-142; rs:59
const DS: u32 = gfx3d(0, 0x1D, 6); // v2p1:149; rs:63
const GS: u32 = gfx3d(0, 0x11, 7); // v2p1:166; rs:69
const STREAMOUT: u32 = gfx3d(0, 0x1E, 3); // v2p1:197; rs:92
const CLIP: u32 = gfx3d(0, 0x12, 4); // v2p1:218; rs:76
const VP_SF_CLIP: u32 = gfx3d(0, 0x21, 2); // v2p1:32; rs:80
const VP_CC: u32 = gfx3d(0, 0x23, 2); // v2p1:31; rs:107
const SF: u32 = gfx3d(0, 0x13, 7); // v2p1:257; rs:82
const SBE: u32 = gfx3d(0, 0x1F, 14); // v2p1:263; rs:111
const WM: u32 = gfx3d(0, 0x14, 3); // v2p1:276; rs:89
const BLEND_PTR: u32 = gfx3d(0, 0x24, 2); // v2p1:19; rs:105
const CC_PTR: u32 = gfx3d(0, 0x0E, 2); // v2p1:18
const DSS_PTR: u32 = gfx3d(0, 0x25, 2); // v2p1:20
const BT_PS: u32 = gfx3d(0, 0x2A, 2); // v2p1:25; rs:145
const CONSTANT_PS: u32 = gfx3d(0, 0x17, 7); // v2p1:291
const PS: u32 = gfx3d(0, 0x20, 8); // v2p1:283; rs:125
const DEPTH_BUFFER: u32 = gfx3d(0, 0x05, 7); // v2p1:315 (DWords 1..6, v2p1:316-320); rs:95 0x78050005
const STENCIL_BUFFER: u32 = gfx3d(0, 0x06, 3); // v2p1:321
const HIER_DEPTH_BUFFER: u32 = gfx3d(0, 0x07, 3); // v2p1:323
const CLEAR_PARAMS: u32 = gfx3d(0, 0x04, 3); // v2p1:324-325; rs:102
const DRAWING_RECT: u32 = gfx3d(1, 0x00, 4); // v2p1:244-245; rs:147
const VERTEX_BUFFERS_1: u32 = gfx3d(0, 0x08, 5); // v2p1:77 (n = 4b-1, b = 1); rs:140
const VERTEX_ELEMENTS_2: u32 = gfx3d(0, 0x09, 5); // v2p1:84 (count = (len+1)/2 = 2)
const PRIM: u32 = gfx3d(3, 0x00, 7); // v2p1:90-91 (opcode 3h, sub 0h, length 5h); rs:151 0x7b000005
const _: () = assert!(
    MULTISAMPLE == 0x790D_0002 && SAMPLE_MASK == 0x7818_0000 && PCALLOC_PS == 0x7916_0000
        && URB_VS == 0x7830_0000 && URB_HS == 0x7831_0000 && URB_DS == 0x7832_0000 && URB_GS == 0x7833_0000
        && VS == 0x7810_0004 && HS == 0x781B_0005 && TE == 0x781C_0002 && DS == 0x781D_0004 && GS == 0x7811_0005
        && STREAMOUT == 0x781E_0001 && CLIP == 0x7812_0002 && VP_SF_CLIP == 0x7821_0000 && VP_CC == 0x7823_0000
        && SF == 0x7813_0005 && SBE == 0x781F_000C && WM == 0x7814_0001 && BLEND_PTR == 0x7824_0000
        && BT_PS == 0x782A_0000 && PS == 0x7820_0006 && DEPTH_BUFFER == 0x7805_0005 && CLEAR_PARAMS == 0x7804_0001
        && DRAWING_RECT == 0x7900_0002 && VERTEX_BUFFERS_1 == 0x7808_0003 && PRIM == 0x7B00_0005
);

// ---- Field values (GEN7R10.md §2 names every bit; all bits not named here are zero = the page's named default) --
const PC_DEPTH_STALL: u32 = 1 << 13; // v2p1:62-63 "Depth Stall Enable"
/// v2p1:106 (before VS state) and v2p1:315 (before the depth group): post-sync write-immediate + depth stall + CS stall.
const PC_WA_FLAGS: u32 = PC_DEPTH_STALL | PC_POST_SYNC_WRITE_IMM | PC_GLOBAL_GTT_IVB | PC_CS_STALL;
/// v2p1:292: "A PIPE_CONTROL with the CS Stall bit set must be programmed after" ALLOC_PS (CS stall + scoreboard).
const PC_CS_STALL_FLAGS: u32 = PC_CS_STALL | PC_STALL_AT_SCOREBOARD;
/// URB_VS: start 1 (x 8 KB, bits 29:25), entry size 1 (U9-1 of 512-bit rows = 2 rows, bits 24:16), 64 entries
/// (bits 15:0, range [32,704], divisible by 8) — v2p1:34-35; exactly rs:39 `0x02010040`.
const URB_VS_DW1: u32 = (1 << 25) | (1 << 16) | 64;
/// URB_HS/URB_DS: start 2, size 0, 0 entries (v2p1:36-38; rs:41, rs:43 `0x04000000`). URB_GS: start 1, 0 entries
/// (v2p1:39-40; rs:45 `0x02000000`). Zero entries is in range for the disabled stages.
const URB_HS_DS_DW1: u32 = 2 << 25;
const URB_GS_DW1: u32 = 1 << 25;
/// ALLOC_PS: offset 0 KB (bits 19:16), size 8 KB (bits 4:0) — v2p1:293; rs:37 `0x00000008`. VS URB starts at 8 KB,
/// past it, so the URB and the push-constant space do not overlap (v2p1:34).
const PCALLOC_PS_DW1: u32 = 8;
/// CLIP DW1: cull mode CULLMODE_NONE (bits 17:16 = 1h, v2p1:219). DW2 CLIP Enable (bit 31) = 0 -> pass-through (v2p1:219).
const CLIP_DW1: u32 = 1 << 16;
/// SF DW1: depth buffer surface format D32_FLOAT (bits 14:12 = 1h; must match 3DSTATE_DEPTH_BUFFER, v2p1:257),
/// View Transform Enable (bit 1) = 0: positions are screen space (v2p1:259; RECTLIST note v2p1:12).
const SF_DW1: u32 = 1 << 12;
/// SF DW2: cull mode CULLMODE_NONE (bits 30:29 = 1h, v2p1:260); rs:84 `0x20000000`.
const SF_DW2: u32 = 1 << 29;
/// SBE DW1: 0 SF output attributes (bits 27:22, v2p1:264), URB read length 1 (bits 15:11; 0 is UNDEFINED,
/// v2p1:264-265), read offset 1 (bits 9:4) — rs:112 carries the same length/offset with 1 attribute.
const SBE_DW1: u32 = (1 << 11) | (1 << 4);
/// WM DW1: Thread Dispatch Enable (bit 29, v2p1:277), barycentric mode bit 0 = perspective pixel (bits 16:11,
/// v2p1:280) — exactly rs:90 `0x20000800`. DW2 = 0 (MSDISPMODE_PERSAMPLE with 1 sample, v2p1:282).
const WM_DW1: u32 = (1 << 29) | (1 << 11);
/// PS DW2: Binding Table Entry Count 1 (bits 25:18, v2p1:284), no samplers, IEEE, SPF 0, Dmask.
const PS_DW2: u32 = 1 << 18;
/// PS DW4: Maximum Number of Threads 0x2F (U8-1 at bits 31:24; odd, inside every listed range, v2p1:286),
/// 16 Pixel Dispatch Enable only (bit 1, v2p1:288) — rs:129 also dispatches SIMD16-only with the kernel in KSP0.
/// Push Constant Enable (bit 11) and Attribute Enable (bit 10) clear: no constants, SBE has 0 attributes (v2p1:286).
const PS_DW4: u32 = (0x2F << 24) | (1 << 1);
/// PS DW5: Dispatch GRF Start Register for Constant/Setup Data [0] = 6 (bits 22:16, v2p1:289) — rs:130: R0-R1
/// header plus four SIMD16 perspective-pixel barycentric registers (the WM_DW1 bit 11 payload).
const PS_DW5: u32 = 6 << 16;
/// DEPTH_BUFFER DW1: SURFTYPE_NULL (7h, bits 31:29, v2p1:316), format D32_FLOAT (1h, bits 20:18, v2p1:317);
/// exactly rs:96 `0xe0040000`.
const DEPTH_DW1: u32 = (7 << 29) | (1 << 18);
/// VERTEX_BUFFER_STATE DW0: VB index 0, VERTEXDATA, MOCS 0, Address Modify Enable (bit 14), pitch 8 bytes
/// (bits 11:0) — v2p1:78-79. (rs:141 additionally sets bit 13 Null Vertex Buffer: the null state fetches zeros.)
const VB_DW0: u32 = (1 << 14) | 8;
/// VERTEX_ELEMENT_STATE 0: VB 0, Valid (bit 25), format 0, offset 0; all four components VFCOMP_STORE_0 (2h at
/// 30:28, 26:24, 22:20, 18:16) — the zeroed VUE header. v2p1:85-88; exactly rs:134-135.
const VE0: [u32; 2] = [1 << 25, (2 << 28) | (2 << 24) | (2 << 20) | (2 << 16)];
/// VERTEX_ELEMENT_STATE 1: VB 0, Valid, format R32G32_FLOAT (085h, v4p1:79), offset 0; components STORE_SRC,
/// STORE_SRC, STORE_0, STORE_1_FP (1,1,2,3; v2p1:87-88) — position (x, y, 0.0, 1.0). rs:138-139 uses the same
/// component word `0x11230000` with R16G16_SSCALED (0F6h).
const VE1: [u32; 2] = [(1 << 25) | (0x085 << 16), (1 << 28) | (1 << 24) | (2 << 20) | (3 << 16)];
const _: () = assert!(VE0[0] == 0x0200_0000 && VE0[1] == 0x2222_0000 && VE1[1] == 0x1123_0000 && DEPTH_DW1 == 0xE004_0000);
/// 3DPRIMITIVE DW1: Primitive Topology Type 3DPRIM_TRILIST = 04h (bits 5:0), SEQUENTIAL (bit 8 = 0). The PRM
/// enumerates the topologies in encoding order at v2p1:75 (POINTLIST first ... RECTLIST 15th); rs:152 pins
/// RECTLIST = 0Fh at that 15th place; mx:25 names TRILIST = 4 (mx:36 RECTLIST = 15).
const PRIM_TRILIST: u32 = 0x04;
/// SURFACE_STATE DW0: SURFTYPE_2D (1h, bits 31:29, v4p1:62), B8G8R8A8_UNORM (0C0h, bits 26:18, v4p1:63,80),
/// VALIGN_2 / HALIGN_4 / linear (bits 17:13 = 0, v4p1:63-65), write-only render cache allocation (bit 8 = 0, v4p1:66).
const SS_DW0: u32 = (1 << 29) | (0x0C0 << 18);

// ---- EU kernel: SIMD16 solid colour (v4p3 formats; GEN7R10.md §3 decodes every field) ---------------------------
/// DW0: opcode (6:0, v4p3:110) | ExecSize 100b = 16 channels (23:21, v4p3:106); Align1, no predication, NoMask 0.
const fn eu_dw0(opcode: u32, sfid: u32) -> u32 {
    opcode | (4 << 21) | (sfid << 24) // SFID[3:0] at 27:24 for send (v4p3:99)
}
/// DW1 for a `mov(16) gREG<1>:F src0` (v4p3:96-97,111): dst RegFile GRF 01b (33:32), DstType F 111b (36:34),
/// Src0.RegFile (38:37), Src0.SrcType F (41:39), Src1.RegFile ARF 00b (43:42), Src1.SrcType (46:44),
/// Dst.RegNum (60:53), Dst.HorzStride 01b = 1 (62:61).
const fn mov_dw1(reg: u32, src0_file: u32, src1_type: u32) -> u32 {
    1 | (7 << 2) | (src0_file << 5) | (7 << 7) | (src1_type << 12) | (reg << 21) | (1 << 29)
}
const EU_MOV: u32 = 0x01; // v4p3:127
const EU_SEND: u32 = 0x31; // v4p3:129
const FILE_GRF: u32 = 1; // v4p3:111
const FILE_IMM: u32 = 3; // v4p3:111
const TYPE_F: u32 = 7; // v4p3:111 (register and immediate encodings both 111b = F)
const SFID_DP_RC: u32 = 0x5; // v1p1:18 "Render Cache Data Port"
/// The payload registers: R,G,B,A for slots 15:0, two GRFs each (v4p1:285-286), at g113..g120 — inside
/// R112-R127 as an EOT send requires (v4p3:218), the same registers rs:208-225 uses.
const PAY: u32 = 113;
/// The colour: R 0.0, G 1.0, B 0.0, A 1.0 (IEEE floats into a UNORM target, v4p1:285).
const RGBA: [u32; 4] = [0x0000_0000, 0x3F80_0000, 0x0000_0000, 0x3F80_0000];
/// What B8G8R8A8_UNORM stores for it, read as a little-endian dword (B, G, R, A bytes): 0xFF00FF00.
const RT_PIXEL: u32 = 0xFF00_FF00;
/// send DW1: dst null (ARF 00b, RegNum 0), DstType UW, Src0 GRF:F, Src1 IMM:D, Dst.HorzStride 1 — exactly rs:224.
const SEND_DW1: u32 = 0x2000_1FA8;
/// send DW2: Src0 = g113 <8;8,1> (SubReg 68:64 = 0, RegNum 76:69, HorzStride 81:80 = 1, Width 84:82 = 011b,
/// VertStride 88:85 = 0100b; v4p3:96,103) — exactly rs:225 with PAY = 113.
const SEND_DW2: u32 = (PAY << 5) | (1 << 16) | (3 << 18) | (4 << 21);
/// send DW3 = message descriptor (v4p3:216; EOT is bit 127 = DW3 bit 31, v4p3:98,215): EOT, Message Length 8
/// (28:25), Response Length 0 (24:20), Header Present 0 (19; allowed for Render Target Write only, v4p1:227,281),
/// Message Type 1100b Render Target Write (17:14, v4p1:229), Last Render Target Select (12, v4p1:280), SIMD16
/// single source 000b (10:8, v4p1:280), Binding Table Index 0 (7:0, v4p1:228) — exactly rs:226 `0x90031000`.
const SEND_DESC: u32 = (1 << 31) | (8 << 25) | (0xC << 14) | (1 << 12);
const _: () = assert!(
    eu_dw0(EU_MOV, 0) == 0x0080_0001 && mov_dw1(113, FILE_GRF, 0) == 0x2E20_03BD
        && eu_dw0(EU_SEND, SFID_DP_RC) == 0x0580_0031 && SEND_DW2 == 0x008D_0E20 && SEND_DESC == 0x9003_1000
);
/// The kernel: four `mov(16) g113+2i<1>:F imm:F` then the EOT render-target write. 20 dwords.
const fn kernel() -> [u32; 20] {
    let mut k = [0u32; 20];
    let mut i = 0;
    while i < 4 {
        k[i * 4] = eu_dw0(EU_MOV, 0);
        // Immediate src0: RegFile IMM, Src1 = ARF with src0's type (non-present operand rule, v4p3:80).
        k[i * 4 + 1] = mov_dw1(PAY + 2 * i as u32, FILE_IMM, TYPE_F);
        k[i * 4 + 2] = 0;
        k[i * 4 + 3] = RGBA[i]; // Imm32 in bits 127:96 (v4p3:96)
        i += 1;
    }
    k[16] = eu_dw0(EU_SEND, SFID_DP_RC);
    k[17] = SEND_DW1;
    k[18] = SEND_DW2;
    k[19] = SEND_DESC;
    k
}
const KERNEL: [u32; 20] = kernel();
const _: () = assert!(KERNEL[1] == 0x2E20_73FD && KERNEL[5] == 0x2E60_73FD && KERNEL[15] == 0x3F80_0000);

// ---- The window and the pages -----------------------------------------------------------------------------------
/// R10's own four-slot window: ring, state (surface + dynamic state, vertex buffer, fences), kernel, render target.
/// GGTT 0x13410000: RING_START bits[31:29] zero, past R9's two slots (0x13400..0x13401) with a gap; proven unowned
/// at run time before any write, or the rung refuses.
const R10_BASE_SLOT: usize = 0x13410;
const R10_WIN: usize = 4;
const PG_RING: usize = 0;
const PG_STATE: usize = 1;
const PG_KERN: usize = 2;
const PG_RT: usize = 3;
// Offsets inside the state page (= Surface State Base = Dynamic State Base).
const OFF_BLEND: u32 = 0x040; // 64-byte aligned (v2p1:19)
const OFF_CC: u32 = 0x080; // 64-byte aligned (v2p1:18)
const OFF_DSS: u32 = 0x0C0; // 64-byte aligned (v2p1:20)
const OFF_CCVP: u32 = 0x100; // 32-byte aligned (v2p1:31)
const OFF_SFCLIP: u32 = 0x140; // 64-byte aligned (v2p1:32)
const OFF_BT: u32 = 0x200; // 32-byte aligned, bits 15:5 (v2p1:25)
const OFF_SS: u32 = 0x220; // 32-byte aligned, bits 31:5 (v4p1:61)
const OFF_VB: u32 = 0x300; // three R32G32_FLOAT vertices, 24 bytes
const OFF_FENCE: u32 = 0x400; // the final post-sync (QWord aligned)
const OFF_WA: u32 = 0x440; // the workaround PIPE_CONTROLs' post-sync scratch (QWord aligned)
/// The kernel's offset from Instruction Base (KSP0 is 64-byte granular, v2p1:283); the page after it stays mapped
/// in-window for the 128-byte prefetch (v4p3:80).
const KSP0: u32 = 0x40;
/// The render target: 32 x 32 B8G8R8A8_UNORM, linear, pitch 128 bytes = exactly one page.
const RT_W: usize = 32;
const RT_H: usize = 32;
const RT_PITCH: usize = 128;
const RT_SEED: u32 = 0x1010_1010;
/// The fence the final post-sync writes, and the seed the CPU leaves there first.
const R10_FENCE: u32 = 0x3D10_C0DE;
const R10_FENCE_SEED: u32 = 0x5EED_3D10;
/// Pixels strictly inside the triangle (0,0)-(32,0)-(0,32): centres (x+.5, y+.5) with x+y <= 30 -> 31*32/2.
/// The 32 centres ON the hypotenuse (x+y = 31) are reported as `edge=` and decide nothing.
const INSIDE_EXPECT: u32 = 496;

/// The four pages (16 KiB), allocated once and held for the boot (GEN7TLB, gen7.md §2.6).
static PAGES_VA: crate::sync::Mutex<usize> = crate::sync::Mutex::new(0);

/// Command names by end dword, filled as the stream is built; the head offset maps onto the last command retired.
const MARKS: usize = 48;

/// What the RCS run did. Every field a reading.
#[derive(Clone, Copy, Default)]
struct Out {
    cand: &'static str,
    landed: bool,
    armed: bool,
    hit: bool,
    read: u32,
    head: u32,
    tail: u32,
    idle: bool,
    disabled: bool,
    regs_restored: bool,
    fw_restored: bool,
    ptes_restored: bool,
    cyc: u64,
    pixels: u32,
    inside: u32,
    edge: u32,
    outside: u32,
    other: u32,
    rt0: u32,
    passed: &'static str,
}

/// R10 under `tests gen7`, after R9: gated on R9's verified run this boot, then the table, then the draw.
pub(super) fn run(r9: &str) {
    let cited = R10_PAGES.iter().filter(|p| p.src.is_some()).count();
    let rows = R10_PAGES.len();
    if r9 != "verified" {
        serial_println!(
            "[gen7] r10 draw=3dprimitive r9={} rows_cited={}/{} writes=0 -> GATED reason=r9-not-verified",
            r9, cited, rows
        );
        return;
    }
    if let Some(p) = R10_PAGES.iter().find(|p| p.src.is_none()) {
        serial_println!(
            "[gen7] r10 draw=3dprimitive r9={} rows_cited={}/{} writes=0 -> DECLINED reason=page-{}-uncited field={} owed={}",
            r9, cited, rows, p.name, p.field, p.owed
        );
        return;
    }
    let bank = *super::super::ladder::BANK.lock();
    let Some(b) = bank else {
        serial_println!("[gen7] r10 draw=3dprimitive r9={} writes=0 -> REFUSED reason=no-boot-bank", r9);
        return;
    };
    // SAFETY: `bar0` is the IGD BAR0 mapping `igpu::init` published and banked; never unmapped. `exec` writes only
    // its own proven-unowned GGTT window, `0x101008`, one forcewake request (restored) and the four RCS ring
    // registers (restored), and refuses before any write on every doubt it can read.
    let t0 = crate::arch::now_cycles();
    let res = unsafe { exec(b.bar0, b.bar0_size) };
    let us = cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
    let o = match res {
        Err(why) => {
            serial_println!("[gen7] r10 draw=3dprimitive r9={} writes=0 -> REFUSED reason={} us={}", r9, why, us);
            return;
        }
        Ok(o) => o,
    };
    //  r10-triangle-verified   THE WIN: the fence landed, the head retired the stream, every inside pixel is the
    //                          shader's colour and nothing outside the triangle (or any other value) was written.
    //  r10-triangle-partial    the fence landed and the shader wrote, but the count or the shape is off.
    //  r10-drew-nothing        the stream retired and the fence landed with the render target untouched: suspect
    //                          the WM/PS dispatch or the RT write message, not the command stream.
    //  r10-fence-miss          the head retired the stream and the fence did not land.
    //  r10-head-stuck          the CS stopped inside the stream; `passed=` names the last command retired.
    //  r10-enable-void / r10-claim-write-void / r10-ring-would-not-disable   R6's envelope verdicts.
    let verdict = if !o.landed {
        "r10-claim-write-void"
    } else if !o.armed {
        "r10-enable-void"
    } else if !o.disabled {
        "r10-ring-would-not-disable"
    } else if o.hit && o.idle && o.inside == INSIDE_EXPECT && o.outside == 0 && o.other == 0 {
        "r10-triangle-verified"
    } else if o.hit && o.idle && o.pixels == 0 && o.other == 0 {
        "r10-drew-nothing"
    } else if o.hit {
        "r10-triangle-partial"
    } else if o.head == o.tail {
        "r10-fence-miss"
    } else {
        "r10-head-stuck"
    };
    serial_println!(
        "[gen7] r10 draw=3dprimitive r9={} rows_cited={}/{} ring=rcs cand={} prim=trilist verts=3 rt=32x32:b8g8r8a8 color=0x{:08X} fence=0x{:08X} read=0x{:08X} pixels={} inside={}/{} edge={} outside={} other={} rt0=0x{:08X} landed={} head={:#x} tail={:#x} passed={} idle={} restored=regs:{},fw:{},ptes:{} gpu_us={} us={} -> {}",
        r9,
        cited,
        rows,
        o.cand,
        RT_PIXEL,
        R10_FENCE,
        o.read,
        o.pixels,
        o.inside,
        INSIDE_EXPECT,
        o.edge,
        o.outside,
        o.other,
        o.rt0,
        o.hit as u32,
        o.head,
        o.tail,
        o.passed,
        o.idle as u32,
        o.regs_restored as u32,
        o.fw_restored as u32,
        o.ptes_restored as u32,
        cycles_to_us(o.cyc),
        us,
        verdict
    );
}

/// The stream builder: the ring page, the write cursor, and the end-dword marks.
struct Ring {
    p: *mut u32,
    d: usize,
    marks: [(usize, &'static str); MARKS],
    n: usize,
}

impl Ring {
    unsafe fn put(&mut self, v: u32) {
        if self.d < 1024 {
            core::ptr::write_volatile(self.p.add(self.d), v);
        }
        self.d += 1;
    }
    unsafe fn cmd(&mut self, name: &'static str, dws: &[u32]) {
        for &v in dws {
            self.put(v);
        }
        self.mark(name);
    }
    fn mark(&mut self, name: &'static str) {
        if self.n < MARKS {
            self.marks[self.n] = (self.d, name);
            self.n += 1;
        }
    }
    unsafe fn invalidate(&mut self, name: &'static str) {
        let mut d = self.d;
        invalidate(self.p, &mut d); // R9's rc:355-374 shape (10 dwords); the ring page holds 1024
        self.d = d;
        self.mark(name);
    }
    /// The last command whose end is at or below the head's dword.
    fn passed(&self, head: u32) -> &'static str {
        let dw = (head as usize) / 4;
        let mut last = "none";
        for &(end, name) in self.marks[..self.n].iter() {
            if end <= dw {
                last = name;
            }
        }
        last
    }
}

/// Write `v` into a page at byte offset `off`.
unsafe fn poke(page: usize, off: u32, v: &[u32]) {
    for (i, &w) in v.iter().enumerate() {
        core::ptr::write_volatile((page + off as usize + i * 4) as *mut u32, w);
    }
}

/// The RCS run. `Err(why)` before any write; `Ok(out)` once the window was claimed.
///
/// # Safety
/// `bar0`/`bar0_size` are the IGD BAR0 mapping the ladder banked. Writes: the four window PTEs (restored), `0x101008`
/// (write-any-value, gen7.md §2.6), one forcewake request register (restored by `fw_release`), the four RCS ring
/// registers (restored to their entry images, only behind a disabled ring). No display register.
unsafe fn exec(bar0: usize, bar0_size: usize) -> Result<Out, &'static str> {
    let flsh = g7regs::HYP_GFX_FLSH_CNTL;
    let need = (GTT_BASE + (R10_BASE_SLOT + R10_WIN + 1) * 4).max(flsh + 4).max(g7regs::IVB_FORCEWAKE_MT_ACK + 4);
    if R10_BASE_SLOT + R10_WIN + 1 >= GGTT_SLOTS || bar0_size < need {
        return Err("bar0-too-small");
    }
    let Some(&(cand, class_pin, req_off, ack_off, ack_mask, srcp, mask_form)) = R6_CANDS.iter().find(|c| c.0 == "mt") else {
        return Err("no-mt-row");
    };

    // ---- census: the window and both neighbours, before any write (R4b's legs, as R9 reads them) ------------------
    let slot = |k: usize| GTT_BASE + (R10_BASE_SLOT + k) * 4;
    let prev_off = GTT_BASE + (R10_BASE_SLOT - 1) * 4;
    let next_off = slot(R10_WIN);
    let fill = rd(bar0, slot(0));
    let (prev, next) = (rd(bar0, prev_off), rd(bar0, next_off));
    if prev != fill || next != fill || (0..R10_WIN).any(|k| rd(bar0, slot(k)) != fill) {
        return Err("window-owned");
    }
    if fill != 0 {
        let bdsm = crate::arch::pci::read_config_32(0, 0, 0, 0xB0) & 0xFFF0_0000;
        let wellformed = (fill & 1) != 0 && (fill & 0xFFFF_F000) != 0 && (fill & 0xF0) == 0;
        let is_bdsm = (fill >> 12) == (bdsm >> 12);
        let far = R5_FAR_PROBE.iter().all(|&s| GTT_BASE + s * 4 + 4 <= bar0_size && rd(bar0, GTT_BASE + s * 4) == fill);
        if !(wellformed && is_bdsm && far) {
            return Err("fill-not-scratch");
        }
    }
    let base_img = fill;

    // ---- the RCS must be off: a live ring is never armed under ------------------------------------------------------
    let start_pre = rd(bar0, g7regs::RCS_RING_START);
    let ctl_pre = rd(bar0, g7regs::RCS_RING_CTL);
    let head_pre = rd(bar0, g7regs::RCS_RING_HEAD);
    let tail_pre = rd(bar0, g7regs::RCS_RING_TAIL);
    if ctl_pre & 1 != 0 {
        return Err("rcs-ring-live");
    }

    // ---- the pages, held for the boot ---------------------------------------------------------------------------------
    let base_va = {
        let mut pages = PAGES_VA.lock();
        if *pages == 0 {
            let p = alloc_zeroed(Layout::from_size_align(R10_WIN * 4096, 4096).unwrap());
            if p.is_null() {
                return Err("alloc");
            }
            *pages = p as usize;
        }
        *pages
    };
    let va = |k: usize| base_va + k * 4096;
    let mut ptes = [0u32; R10_WIN];
    for k in 0..R10_WIN {
        let Some(pa) = crate::arch::memory::translate(va(k) as u64) else { return Err("va-unmapped") };
        if pa >= 0x1_0000_0000 {
            return Err("phys-above-4g"); // the 32-bit PTE form R4..R9 flew
        }
        ptes[k] = (pa as u32 & 0xFFFF_F000) | GGTT_PTE_VALID;
        if ptes[k] == base_img {
            return Err("pte-indistinct");
        }
    }
    let gtt = |k: usize| ((R10_BASE_SLOT + k) * 4096) as u32;
    let (state, kern, rtg) = (gtt(PG_STATE), gtt(PG_KERN), gtt(PG_RT));

    // ---- the state page (GEN7R10.md §4) ------------------------------------------------------------------------------
    for i in 0..4096 / 4 {
        core::ptr::write_volatile((va(PG_STATE) + i * 4) as *mut u32, 0);
    }
    // BLEND_STATE: blend disabled, src ONE / dst ZERO, pre/post-blend clamp, every channel write-enabled
    // (v2p1:366-372; rs:163-164 `0x00000031, 0x00000003`).
    poke(va(PG_STATE), OFF_BLEND, &[(1 << 5) | 0x11, 0x3]);
    // COLOR_CALC_STATE and DEPTH_STENCIL_STATE: all zero = no alpha test, no stencil, no depth test/write
    // (v2p1:361-366); left zero by the page clear. CC_VIEWPORT: min depth 0.0, max 1.0 (v2p1:373).
    poke(va(PG_STATE), OFF_CCVP, &[0x0000_0000, 0x3F80_0000]);
    // SF_CLIP_VIEWPORT: zero — unused, the SF view transform and the clipper are both off (v2p1:259, v2p1:219).
    // BINDING_TABLE entry 0 -> SURFACE_STATE at OFF_SS (v4p1:61), the shader's binding table index 0 (v4p1:228).
    poke(va(PG_STATE), OFF_BT, &[OFF_SS]);
    // SURFACE_STATE: 2D B8G8R8A8_UNORM linear at the RT page, width/height 32 (U14-1, v4p1:68-69), pitch 128
    // (U18 #bytes-1, v4p1:70); depth 0, 1 sample, MOCS from the GTT, no MCS, LOD 0 (v4p1:71-78).
    poke(va(PG_STATE), OFF_SS, &[SS_DW0, rtg, (((RT_H - 1) as u32) << 16) | (RT_W - 1) as u32, (RT_PITCH - 1) as u32, 0, 0, 0, 0]);
    // The vertex buffer: three R32G32_FLOAT screen-space positions (0,0), (32,0), (0,32).
    poke(va(PG_STATE), OFF_VB, &[0x0000_0000, 0x0000_0000, 0x4200_0000, 0x0000_0000, 0x0000_0000, 0x4200_0000]);
    poke(va(PG_STATE), OFF_FENCE, &[R10_FENCE_SEED, 0]);
    // ---- the kernel page and the render target ----------------------------------------------------------------------
    for i in 0..4096 / 4 {
        core::ptr::write_volatile((va(PG_KERN) + i * 4) as *mut u32, 0);
        core::ptr::write_volatile((va(PG_RT) + i * 4) as *mut u32, RT_SEED);
    }
    poke(va(PG_KERN), KSP0, &KERNEL);
    let vb = state + OFF_VB;

    // ---- the ring (GEN7R10.md §2 lists every dword in this order) ----------------------------------------------------
    let mut r = Ring { p: va(PG_RING) as *mut u32, d: 0, marks: [(0, ""); MARKS], n: 0 };
    for i in 0..1024 {
        core::ptr::write_volatile(r.p.add(i), MI_NOOP);
    }
    r.invalidate("invalidate-0");
    r.cmd("pipeline_select", &[PIPELINE_SELECT_3D, MI_NOOP]);
    r.invalidate("invalidate-1");
    r.cmd("flush", &[PC4, PC_FLUSH_FLAGS, 0, 0]);
    let m = BASE_ADDRESS_MODIFY;
    r.cmd("state_base_address", &[SBA_DW0, m, state | m, state | m, m, kern | m, m, m, m, m]);
    r.invalidate("invalidate-2");
    r.cmd("multisample", &[MULTISAMPLE, 0, 0, 0]);
    r.cmd("sample_mask", &[SAMPLE_MASK, 1]);
    r.cmd("pc-wa-vs", &[PC4, PC_WA_FLAGS, state + OFF_WA, 0]);
    r.cmd("pcalloc_vs", &[PCALLOC_VS, 0]);
    r.cmd("pcalloc_ps", &[PCALLOC_PS, PCALLOC_PS_DW1]);
    r.cmd("pc-cs-stall", &[PC4, PC_CS_STALL_FLAGS, 0, 0]);
    r.cmd("urb", &[URB_VS, URB_VS_DW1, URB_HS, URB_HS_DS_DW1, URB_DS, URB_HS_DS_DW1, URB_GS, URB_GS_DW1]);
    r.cmd("constant_vs", &[CONSTANT_VS, 0, 0, 0, 0, 0, 0]);
    r.cmd("vs", &[VS, 0, 0, 0, 0, 0]);
    r.cmd("hs", &[HS, 0, 0, 0, 0, 0, 0]);
    r.cmd("te", &[TE, 0, 0, 0]);
    r.cmd("ds", &[DS, 0, 0, 0, 0, 0]);
    r.cmd("gs", &[GS, 0, 0, 0, 0, 0, 0]);
    r.cmd("streamout", &[STREAMOUT, 0, 0]);
    r.cmd("clip", &[CLIP, CLIP_DW1, 0, 0]);
    r.cmd("viewport_ptrs", &[VP_SF_CLIP, OFF_SFCLIP, VP_CC, OFF_CCVP]);
    r.cmd("sf", &[SF, SF_DW1, SF_DW2, 0, 0, 0, 0]);
    r.cmd("sbe", &[SBE, SBE_DW1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    r.cmd("wm", &[WM, WM_DW1, 0]);
    r.cmd("cc_ptrs", &[BLEND_PTR, OFF_BLEND, CC_PTR, OFF_CC, DSS_PTR, OFF_DSS]);
    r.cmd("binding_table_ptr_ps", &[BT_PS, OFF_BT]);
    r.cmd("constant_ps", &[CONSTANT_PS, 0, 0, 0, 0, 0, 0]);
    r.cmd("ps", &[PS, KSP0, PS_DW2, 0, PS_DW4, PS_DW5, 0, 0]);
    r.cmd("pc-wa-wm", &[PC4, PC_DEPTH_STALL, 0, 0]);
    // The depth group: CS stall + post-sync, then depth stall / depth flush / depth stall (v2p1:315).
    r.cmd("pc-wa-depth", &[PC4, PC_WA_FLAGS, state + OFF_WA, 0]);
    r.cmd("pc-depth-flush", &[PC4, PC_DEPTH_CACHE_FLUSH, 0, 0]);
    r.cmd("pc-depth-stall", &[PC4, PC_DEPTH_STALL, 0, 0]);
    r.cmd("depth_buffer", &[DEPTH_BUFFER, DEPTH_DW1, 0, 0, 0, 0, 0]);
    r.cmd("hier_stencil_clear", &[HIER_DEPTH_BUFFER, 0, 0, STENCIL_BUFFER, 0, 0, CLEAR_PARAMS, 0, 0]);
    r.cmd("drawing_rectangle", &[DRAWING_RECT, 0, (((RT_H - 1) as u32) << 16) | (RT_W - 1) as u32, 0]);
    r.cmd("vertex_buffers", &[VERTEX_BUFFERS_1, VB_DW0, vb, vb + 3 * 8 - 1, 0]);
    r.cmd("vertex_elements", &[VERTEX_ELEMENTS_2, VE0[0], VE0[1], VE1[0], VE1[1]]);
    // The dummy draw v1p1:33 requires after a PIPELINE_SELECT enabling 3D: vertex count 0, the other enables 0.
    r.cmd("dummy_draw", &[PRIM, PRIM_TRILIST, 0, 0, 1, 0, 0]);
    r.cmd("3dprimitive", &[PRIM, PRIM_TRILIST, 3, 0, 1, 0, 0]);
    // The fence: R9's gen7 RCS breadcrumb (render-target/depth/DC flush + CS stall, post-sync write-immediate).
    r.cmd("fence", &[PC4, PC_POSTSYNC_FLAGS, state + OFF_FENCE, R10_FENCE]);
    if r.d % 2 == 1 {
        r.cmd("noop-pad", &[MI_NOOP]);
    }
    if r.d > 1024 || r.n >= MARKS {
        return Err("ring-length"); // nothing written to the GT yet
    }
    let tail = (r.d * 4) as u32;
    clflush_range(base_va, R10_WIN * 4096);
    let fence = (va(PG_STATE) + OFF_FENCE as usize) as *mut u32;

    let mut out = Out { cand, tail, ..Out::default() };

    // ---- claim: PTEs, landed check, the pinned GTT flush (Vol1 Pt3 §1.2.21 p.191) -----------------------------------
    for k in 0..R10_WIN {
        wr(bar0, slot(k), ptes[k]);
    }
    out.landed = (0..R10_WIN).all(|k| rd(bar0, slot(k)) == ptes[k]) && rd(bar0, prev_off) == prev && rd(bar0, next_off) == next;
    wr(bar0, flsh, 1);

    out.disabled = true;
    if out.landed {
        let hold = fw_acquire(bar0, "r10", cand, class_pin, req_off, ack_off, ack_mask, srcp, mask_form, FW_ACK_BUDGET_CYC);
        wr(bar0, g7regs::RCS_RING_CTL, 0);
        wr(bar0, g7regs::RCS_RING_START, gtt(PG_RING));
        wr(bar0, g7regs::RCS_RING_HEAD, 0);
        wr(bar0, g7regs::RCS_RING_TAIL, 0);
        wr(bar0, g7regs::RCS_RING_CTL, RCS_RING_CTL_1PAGE_EN);
        out.armed = rd(bar0, g7regs::RCS_RING_CTL) & 1 == 1;
        if out.armed {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            let t0 = crate::arch::now_cycles();
            wr(bar0, g7regs::RCS_RING_TAIL, tail);
            let _ = poll_cycles(EXEC_BUDGET_CYC, || {
                clflush_range(fence as usize, 4);
                core::ptr::read_volatile(fence) == R10_FENCE
            });
            let (idle, _, _) = poll_cycles(DRAIN_BUDGET_CYC, || (rd(bar0, g7regs::RCS_RING_HEAD) & RING_HEAD_OFF_MASK) == tail);
            out.cyc = crate::arch::now_cycles().saturating_sub(t0);
            out.idle = idle;
            clflush_range(fence as usize, 4);
            out.read = core::ptr::read_volatile(fence);
            out.hit = out.read == R10_FENCE;
            out.head = rd(bar0, g7regs::RCS_RING_HEAD) & RING_HEAD_OFF_MASK;
        }
        // Teardown under the hold (R6's rule): disable, and restore the entry images only behind a disabled ring.
        wr(bar0, g7regs::RCS_RING_CTL, 0);
        out.disabled = rd(bar0, g7regs::RCS_RING_CTL) & 1 == 0;
        if out.disabled {
            wr(bar0, g7regs::RCS_RING_TAIL, tail_pre);
            wr(bar0, g7regs::RCS_RING_HEAD, head_pre);
            wr(bar0, g7regs::RCS_RING_START, start_pre);
            wr(bar0, g7regs::RCS_RING_CTL, ctl_pre);
            out.regs_restored = rd(bar0, g7regs::RCS_RING_START) == start_pre
                && rd(bar0, g7regs::RCS_RING_CTL) == ctl_pre
                && (rd(bar0, g7regs::RCS_RING_HEAD) & RING_HEAD_OFF_MASK) == (head_pre & RING_HEAD_OFF_MASK)
                && rd(bar0, g7regs::RCS_RING_TAIL) == tail_pre;
        }
        let (fw_ok, _ev) = fw_release(bar0, "r10", &hold);
        out.fw_restored = fw_ok;
    }
    out.passed = r.passed(out.head);

    // ---- the read-back: every render-target pixel, after the fence's render-target flush ----------------------------
    clflush_range(va(PG_RT), 4096);
    for y in 0..RT_H {
        for x in 0..RT_W {
            let v = core::ptr::read_volatile((va(PG_RT) + y * RT_PITCH + x * 4) as *const u32);
            if v == RT_PIXEL {
                out.pixels += 1;
                match x + y {
                    s if s <= 30 => out.inside += 1,
                    31 => out.edge += 1,
                    _ => out.outside += 1,
                }
            } else if v != RT_SEED {
                out.other += 1;
            }
        }
    }
    out.rt0 = core::ptr::read_volatile(va(PG_RT) as *const u32);

    // ---- unwind the window: only behind a quiesced engine (a ring that did not drain may still write) --------------
    if out.disabled && (out.idle || !out.armed) {
        for k in 0..R10_WIN {
            wr(bar0, slot(k), base_img);
        }
        out.ptes_restored = (0..R10_WIN).all(|k| rd(bar0, slot(k)) == base_img) && rd(bar0, prev_off) == prev && rd(bar0, next_off) == next;
        wr(bar0, flsh, 1);
    }
    Ok(out)
}
