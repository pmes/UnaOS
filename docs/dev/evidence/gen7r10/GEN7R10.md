# GEN7R10 (rmbp-ledger B638, R121): the first 3D draw on the Ivy Bridge RCS

R121: "una interface waits for 3D so get on it". GEN7R9B (B626) made rung R9 execute and left R10 as a 22-row
table with 2 rows cited, because the cloud could not reach Intel's PRMs. The bench can. This arc fetched the
2012 Ivy Bridge PRM, filled every row against it, wrote the executing draw in
`unaos/crates/kernel/src/drivers/gpu/gen7_r10.rs`, and designed R11 (§6).

Nothing here has flown. The arc compiled one x86 leg (§7). The bench owes the flight.

## 1. Sources

The PRM volumes were fetched on 2026-10-13 from `https://xorg.freedesktop.org/docs/intel/IVB/` (x.org's mirror;
`www.x.org/docs/intel/IVB/` redirects there). They went into a new, empty directory,
`~/unaos-bench/scratch/rmbp-0915/local3/gen7r10/dl/`, and were read as data only. The bench has no poppler, so a
stdlib-only Python reader (run with `-I` from the scratchpad) dumped the text, page by page. **The printed page
number equals the PDF page number in all five volumes.** Every `vXpY:N` cite below is that page.

| key | file | revision | sha256 |
| --- | --- | --- | --- |
| `v1p1` | `IHD_OS_Vol1_Part1.pdf` (GPE: PIPELINE_SELECT, STATE_BASE_ADDRESS, function IDs) | May 2012 | `d7a1b20d2fa4d186fd9090cbbca1224b61342a52148cf2fd668a4d2d77d06cdc` |
| `v1p3` | `IHD_OS_Vol1_Part3.pdf` (MMIO, MI commands) | May 2012 | `d49033fe788bb724a8226b597b8f5868a796f9dceb886c3f203b8bcc46c2837f` |
| `v2p1` | `IHD_OS_Vol2_Part1.pdf` (3D pipeline command reference) | 6/21/2012 | `bacaa98d8301ad1453e1202814f2f6c26e8ddbe0a7c925e46cfa024de9706e8f` |
| `v4p1` | `IHD_OS_Vol4_Part1.pdf` (shared functions: SURFACE_STATE, data port) | 5/31/2012 | `69217f8ebf0d02a4342d860f2e3bc59128ed059cc1c5a8bb098ca52d226e47a5` |
| `v4p3` | `IHD_OS_Vol4_Part3.pdf` (EU ISA) | 5/31/2012 | `125f2256885d69065fab9b1b277d30d3b38b97b72c2dc044bef5818c2fe9f058` |
| `rs` | i915 `gt/gen7_renderstate.c`, linux `24a19b95`, SPDX MIT | — | `52e5ca5bc8fa2f72d87e3224c9c33169ac092ae441db55d81ac56a5dd0a0c81d` (matches GEN7R9B.md §1) |
| `mx` | mesa `src/intel/genxml/gen70.xml`, main `e3f10eab` | — | `fd58ca141b55246826628c3d8d4a0d142b2db51d1ad7033c21c4b282916883ae` |

`rc` (gen7_renderclear.c) and `cs` (gen6_engine_cs.c) are the GEN7R9B files, cited at the same commit.

**What rs:18-158 is.** GEN7R9B.md §3 says it carries values without names. Paired with the PRM, it is a
complete gen7 draw: PIPELINE_SELECT, STATE_BASE_ADDRESS, the whole 3D front end, a sampling pixel shader and a
3DPRIMITIVE of 3 RECTLIST vertices. It renders nothing, by design:

- rs:148 sets the drawing rectangle minimum to 0xFFFF,0xFFFF, so min > max. v2p1:245 says the rectangle is then
  null and every polygon is discarded.
- rs:141 sets bit 13 of the vertex buffer state, Null Vertex Buffer (v2p1:79), so every fetch returns 0.

Each rs literal used here is paired to its command and fields in §2. The const blocks in `gen7_r10.rs` assert
that the constants build those literals. That covers 27 header dwords, the URB, WM, depth and vertex-element
words, the send instruction (all four dwords), and a mov encoding.

`mx` is used for one fact only: the topology encoding. It agrees with the PRM's enumeration order and with
rs:152 (row 37). The genxml file has no licence header. Mesa's `src/intel` is MIT. No text from it is in the
tree.

## 2. The filled table, in command-stream order

The ring is R10's own page, executed as a one-page RCS ring the way R9 runs it. Dword offsets are ring dwords.
`state` is the GGTT address of the state page, and `kern` / `rt` are the kernel and render-target pages (§4).
Unless a row says otherwise, every bit not named is 0, which is the page's named default or MBZ.

Every 3DSTATE header is `3<<29 | 3<<27 | opcode<<24 | sub<<16 | (len-2)` (GFXPIPE, GFXPIPE_3D, the opcode and
sub-opcode from the command's own page, and DWord Length = total-2).

| # | dw | command | dwords and every field set | PRM cite | pairing |
| --- | --- | --- | --- | --- | --- |
| — | 0-9, 12-21, 36-45 | invalidate pairs, flush | R9's rows, unchanged | GEN7R9B.md §2; PIPE_CONTROL v2p1:59-65 | — |
| 1 | 10-11 | PIPELINE_SELECT (3D) + MI_NOOP | `69040000`: GFXPIPE, 28:27=1, 26:24=1, 23:16=04h, 1:0 = 0 (3D). The PRM requires a dummy draw after a select that enables 3D (row 37) | v1p1:19, v1p1:33 | rs:19 |
| 2 | 26-35 | STATE_BASE_ADDRESS | `61010008`. DW1 general base 0, modify. DW2 surface base = `state`, modify. DW3 dynamic base = `state`, modify. DW4 indirect base 0, modify. DW5 instruction base = `kern`, modify. DW6-9: every upper bound 0 with modify, where 0 disables the range check. All MOCS 0 | v1p1:22-27 (bound = 0 ignored v1p1:26) | rs:20-29 |
| 3 | 46-49 | 3DSTATE_MULTISAMPLE | `790D0002` (opcode 1, sub 0Dh, 4 dw). DW1: Pixel Location = CENTER (bit 4 = 0), NUMSAMPLES_1 (3:1 = 0). DW2-3 sample offsets 0 | v2p1:304-305 | rs:30-33 |
| 4 | 50-51 | 3DSTATE_SAMPLE_MASK | `78180000`, DW1 = 1 (sample 0; bits 7:1 must be 0 at 1 sample) | v2p1:293-294 | rs:34-35 |
| 5 | 52-55 | PIPE_CONTROL (VS-state workaround) | `7A000002`. DW1 = Depth Stall (13) \| post-sync Write Immediate (15:14 = 1) \| GGTT (24) \| CS Stall (20) = `01106000`. DW2 = `state+0x440`, DW3 = 0. This is the post-sync and depth-stall control required before any VS state | v2p1:106; bits v2p1:60-63 | — |
| 6 | 56-57 | 3DSTATE_PUSH_CONSTANT_ALLOC_VS | `79120000` (opcode 1, sub 12h). DW1 = 0: offset 0 KB (19:16), size 0 KB (4:0). Size 0 is valid when VS constants are off | v2p1:115-116 | — |
| 7 | 58-59 | 3DSTATE_PUSH_CONSTANT_ALLOC_PS | `79160000`. DW1 = 8: offset 0, size 8 KB | v2p1:292-293 | rs:36-37 |
| 7a | 60-63 | PIPE_CONTROL (CS stall after ALLOC_PS) | `7A000002`, `00100002` (CS Stall \| Stall at Pixel Scoreboard), 0, 0 | v2p1:292; bits v2p1:61,64 | — |
| 8 | 64-65 | 3DSTATE_URB_VS | `78300000`. DW1 `02010040`: start 1 x 8 KB (29:25), so it sits past the 8 KB PS push-constant space. Entry size 1 (24:16, U9-1 of 512-bit rows, giving 2 rows = 128 B, which holds the 32 B VUE). 64 entries (15:0; range [32,704], divisible by 8) | v2p1:34-35 | rs:38-39 |
| 9 | 66-71 | 3DSTATE_URB_HS / _DS / _GS | `78310000 04000000`, `78320000 04000000`, `78330000 02000000`: start 2 / 2 / 1, size 0, 0 entries. The PRM says all four URB commands must be programmed | v2p1:34 note, v2p1:36-40 | rs:40-45 |
| 10 | 72-78 | 3DSTATE_CONSTANT_VS | `78150005` + 6 x 0: all read lengths 0, all pointers 0. It must follow ALLOC_VS before the next draw | v2p1:112-115 (rule v2p1:115) | — |
| 11 | 79-84 | 3DSTATE_VS | `78100004` + 5 x 0. VS Function Enable (DW5 bit 0) = 0, so vertices pass through unmodified. Vertex Cache Disable (bit 1) = 0. Statistics 0 | v2p1:106-111 (enable/pass-through v2p1:111) | rs:46-51 |
| 12 | — | the vertex shader | **none: pass-through.** v2p1:111 states a disabled VS sends VF-built vertices down the pipeline unmodified, so the VUE that VF writes (row 34) is the VS output. i915's own gen7 batch does the same (rs:46-51, all zero). No VS EU kernel is needed or written | v2p1:111 | rs:46-51 |
| 13 | 85-91 | 3DSTATE_HS | `781B0005` + 6 x 0. HS Enable (DW2 bit 31) = 0 | v2p1:128-131 (enable v2p1:129) | rs:52-58 |
| 14 | 92-95 | 3DSTATE_TE | `781C0002` + 3 x 0. TE Enable (DW1 bit 0) = 0 | v2p1:141-143 | rs:59-62 |
| 15 | 96-101 | 3DSTATE_DS | `781D0004` + 5 x 0. DS Function Enable (DW5 bit 0) = 0 | v2p1:149-154 (enable v2p1:154) | rs:63-68 |
| 16 | 102-108 | 3DSTATE_GS | `78110005` + 6 x 0. GS Enable (DW5 bit 0) = 0, which is pass-through; DW6 reserved | v2p1:166-173 (v2p1:173) | rs:69-75 |
| 17 | 109-111 | 3DSTATE_STREAMOUT | `781E0001` + 2 x 0. SO Function Enable (DW1 bit 31) = 0 | v2p1:197-199 | rs:92-94 |
| 18 | 112-115 | 3DSTATE_CLIP | `78120002`. DW1 = `00010000`: Cull Mode = CULLMODE_NONE (17:16 = 1h). DW2 = 0: CLIP Enable (31) = 0, which is pass-through. DW3 = 0 | v2p1:218-222 (enable v2p1:219) | rs:76-79 (rs keeps DW1 = 0; with the clipper off it is unused) |
| 19 | 116-117 | 3DSTATE_VIEWPORT_STATE_POINTERS_SF_CLIP + SF_CLIP_VIEWPORT | `78210000`, `00000140` (64-byte aligned, from Dynamic State Base). The 16-dword SF_CLIP_VIEWPORT at `state+0x140` is all zero. It is unused because the SF view transform (row 21) and the clipper (row 18) are both off | v2p1:32, v2p1:270-271 | rs:80-81 |
| 20 | 118-119 | 3DSTATE_VIEWPORT_STATE_POINTERS_CC + CC_VIEWPORT | `78230000`, `00000100` (32-byte aligned). CC_VIEWPORT at `state+0x100`: min depth 0.0, max depth 1.0 | v2p1:31, v2p1:373 | rs:107-108 (rs: ±1e35 at rs:171-172) |
| 21 | 120-126 | 3DSTATE_SF | `78130005`. DW1 = `00001000`: Depth Buffer Surface Format D32_FLOAT (14:12 = 1h; it must match row 31). View Transform Enable (1) = 0, so positions are screen space. Fill modes SOLID. DW2 = `20000000`: Cull Mode = CULLMODE_NONE (30:29 = 1h), AA off, scissor off, MSRASTMODE 0 (matches WM). DW3-6 = 0 (provoking vertex 0, depth offsets 0) | v2p1:257-263 | rs:82-88 (rs DW1 = 0) |
| 22 | 127-140 | 3DSTATE_SBE | `781F000C`. DW1 = `00000810`: 0 SF output attributes (27:22). Vertex URB Entry Read Length 1 (15:11; 0 is UNDEFINED). Read Offset 1 (9:4). Swizzle off. DW2-13 = 0 | v2p1:263-269 (length v2p1:264-265) | rs:111-124 (rs: 1 attribute) |
| 23 | 141-143 | 3DSTATE_WM | `78140001`. DW1 = `20000800`: Thread Dispatch Enable (29), Barycentric Interpolation Mode bit 0 = perspective pixel (16:11). EDSC_NORMAL, PSCDEPTH_OFF, MSRASTMODE_OFF_PIXEL. DW2 = 0 | v2p1:276-282 | rs:89-91 (identical) |
| 24 | 144-145 | 3DSTATE_BLEND_STATE_POINTERS + BLEND_STATE | `78240000`, `00000040`. BLEND_STATE at `state+0x40`: DW0 = `00000031`, which is blend disabled (31 = 0), src factor ONE (9:5 = 1), dst factor ZERO (4:0 = 11h). DW1 = `00000003`: pre-blend (1) and post-blend (0) clamp on, range UNORM (3:2 = 0). Write-disable bits 27:24 = 0, so every channel is written | v2p1:19, v2p1:366-372 | rs:105-106, rs:163-164 |
| 25 | 146-147 | 3DSTATE_CC_STATE_POINTERS + COLOR_CALC_STATE | `780E0000`, `00000080`. The 6-dword state at `state+0x80` is all zero: no stencil reference, alpha test format 0, blend constant 0 | v2p1:18, v2p1:361-362 | — |
| 26 | 148-149 | 3DSTATE_DEPTH_STENCIL_STATE_POINTERS + DEPTH_STENCIL_STATE | `78250000`, `000000C0`. The 3 dwords at `state+0xC0` are all zero: stencil test off, depth test off, depth write off | v2p1:20, v2p1:362-366 | — |
| 27 | — | SURFACE_STATE (render target) | At `state+0x220`, 8 dwords. DW0 = `23000000`: SURFTYPE_2D (31:29 = 1h), B8G8R8A8_UNORM (26:18 = 0C0h), VALIGN_2, HALIGN_4, linear (14 = 0), write-only RC allocation (8 = 0). DW1 = `rt`. DW2 = `001F001F`: height 32 and width 32 (U14-1). DW3 = `0000007F`: depth 0, pitch 128 (U18 bytes-1). DW4-7 = 0: no rotation, 1 sample, MOCS from the GTT, LOD 0, no MCS, clear colour 0 | v4p1:62-78; format v4p1:80 | rc:151-168 |
| 28 | 150-151 | BINDING_TABLE + 3DSTATE_BINDING_TABLE_POINTERS_PS | `782A0000`, `00000200` (15:5, 32-byte aligned, from Surface State Base). The table at `state+0x200` has entry 0 = `00000220` (Surface State Pointer 31:5) | v2p1:25, v4p1:61 | rs:145-146, rs:235; rc:183 |
| 29 | 152-158 | 3DSTATE_CONSTANT_PS | `78170005` + 6 x 0. It must follow ALLOC_PS before the next draw | v2p1:291 (rule v2p1:292) | — |
| 30 | 159-166 | 3DSTATE_PS | `78200006`. DW1 Kernel Start Pointer[0] = `00000040` (from Instruction Base, 64-byte granular). DW2 = `00040000`: Binding Table Entry Count 1 (25:18), no samplers, IEEE, Dmask, multiple program flows. DW3 = 0 (no scratch). DW4 = `2F000002`: Maximum Number of Threads 2Fh (U8-1, odd as required, inside every listed range), 16 Pixel Dispatch Enable (1) only; Push Constant Enable (11) and Attribute Enable (10) are both 0, as row 22 requires. DW5 = `00060000`: Dispatch GRF Start [0] = 6 (R0-R1 plus the four SIMD16 barycentric GRFs). DW6-7 = 0 | v2p1:283-290 | rs:125-132 (SIMD16-only, kernel in KSP0, GRF start 6) |
| 30a | 167-170 | PIPE_CONTROL (WM-state workaround) | `7A000002`, `00002000` (Depth Stall), 0, 0, after PS and the CC/blend/DS/BT pointers | v2p1:276 | — |
| 31 | 171-198 | depth group: 3 PIPE_CONTROLs, then 3DSTATE_DEPTH_BUFFER, _HIER_DEPTH_BUFFER, _STENCIL_BUFFER, _CLEAR_PARAMS | The three controls are CS stall + post-sync (`01106000` → `state+0x440`), depth cache flush (`00000001`) and depth stall (`00002000`). Then `78050005`, DW1 = `E0040000`: SURFTYPE_NULL (31:29 = 7h), D32_FLOAT (20:18 = 1h), and DW2-6 = 0. Then `78070001` 0 0, `78060001` 0 0, `78040001` 0 0. All four commands are programmed together, as the PRM requires | v2p1:315-325 (rules v2p1:315; NULL v2p1:316; format v2p1:317) | rs:95-104 |
| 32 | 199-202 | 3DSTATE_DRAWING_RECTANGLE | `79000002` (opcode 1, sub 0). DW1 = 0 (Y min 0, X min 0). DW2 = `001F001F` (Y max 31, X max 31, inclusive). DW3 = 0 (origin 0,0) | v2p1:244-246 | rs:147-150 (rs: a null rectangle) |
| 33 | 203-207 | 3DSTATE_VERTEX_BUFFERS | `78080003` (n = 4b-1, b = 1). VERTEX_BUFFER_STATE DW0 = `00004008`: VB 0, VERTEXDATA (20 = 0), MOCS 0, Address Modify Enable (14), pitch 8 (11:0). DW1 = `state+0x300` (a GGTT byte address). DW2 = `state+0x317`, the inclusive end. DW3 = 0 (step rate, ignored for VERTEXDATA) | v2p1:77-80 | rs:140-144 (rs also sets Null VB, bit 13) |
| 34 | 208-212 | 3DSTATE_VERTEX_ELEMENTS | `78090003` (count = (3+1)/2 = 2). VE0 = `02000000 22220000`: VB 0, Valid (25), all four components VFCOMP_STORE_0, giving the zeroed VUE header. VE1 = `02850000 11230000`: VB 0, Valid, R32G32_FLOAT (085h), offset 0, components STORE_SRC, STORE_SRC, STORE_0, STORE_1_FP, giving (x, y, 0.0, 1.0) in VUE dwords 4-7 | v2p1:84-88; format v4p1:79 | rs:133-139 (VE0 identical; VE1 the same components on R16G16_SSCALED) |
| 37a | 213-219 | 3DPRIMITIVE (dummy) | `7B000005`, `00000004`, vertex count 0, start 0, instances 1, 0, 0. This is the dummy draw the PRM requires after a PIPELINE_SELECT that enables 3D | v1p1:33 | — |
| 37 | 220-226 | 3DPRIMITIVE | `7B000005` (opcode 3h, sub 0h, length 5h). DW1 = `00000004`: Primitive Topology 3DPRIM_TRILIST (5:0 = 04h), SEQUENTIAL (8 = 0), no indirect, no predicate. DW2 vertex count 3, DW3 start 0, DW4 instance count 1, DW5 start instance 0, DW6 base vertex 0 | v2p1:90-94; encoding: v2p1:75 lists the topologies in encoding order (POINTLIST first, TRILIST 4th, RECTLIST 15th) | rs:151-157 (RECTLIST = 0Fh, the 15th); mx:25 TRILIST = 4, mx:36 RECTLIST = 15 |
| 38 | 227-230 | PIPE_CONTROL (fence) | `7A000002`, `011050A1` (R9's gen7 RCS breadcrumb: RT flush, depth flush, DC flush, PIPE_CONTROL flush, post-sync write imm, GGTT, CS stall), `state+0x400`, `3D10C0DE` | v2p1:59-65 | cs:355-364 |
| — | 231 | MI_NOOP | QWord pad. TAIL = 232 dwords = `0x3A0` | v1p3:180 | — |

**Rows: 37 of 37 cited** in `R10_PAGES`. Row 12 (VS) is cited as pass-through, not as an EU kernel (§5).
B626's two cited rows are kept: SURFACE_STATE is now cited from the PRM v4p1:62-78 with rc:151-168, and the
binding table from v4p1:61 with rc:183.

## 3. The pixel shader: four movs and one EOT send (`KERNEL` in gen7_r10.rs)

Native 128-bit instructions, laid out per v4p3:96-99, at `kern + 0x40`.

| instr | DW0 | DW1 | DW2 | DW3 |
| --- | --- | --- | --- | --- |
| `mov(16) g113<1>:F 0.0:F` | `00800001` | `2E2073FD` | `00000000` | `00000000` |
| `mov(16) g115<1>:F 1.0:F` | `00800001` | `2E6073FD` | `00000000` | `3F800000` |
| `mov(16) g117<1>:F 0.0:F` | `00800001` | `2EA073FD` | `00000000` | `00000000` |
| `mov(16) g119<1>:F 1.0:F` | `00800001` | `2EE073FD` | `00000000` | `3F800000` |
| `send(16) null g113 RTW EOT` | `05800031` | `20001FA8` | `008D0E20` | `90031000` |

Field by field:

- **DW0.** Opcode in 6:0: mov = 01h (v4p3:127), send = 31h (v4p3:129). ExecSize in 23:21 = 100b, 16 channels
  (v4p3:106). Align1 (bit 8 = 0), no predication, no dependency control. For send, bits 27:24 are SFID[3:0]
  (v4p3:99). SFID 5h is the Render Cache Data Port (v1p1:18).
- **DW1 (mov).** Dst RegFile GRF = 01b (33:32). DstType F = 111b (36:34). Src0 RegFile IMM = 11b (38:37). Src0
  type F = 111b (41:39); in the immediate encoding 111b is also F (v4p3:111). Src1 RegFile ARF = 00b with type F
  (46:44). That is the non-present-operand rule when src0 is an immediate (v4p3:80). Dst.RegNum in 60:53, and
  Dst.HorzStride = 01b in 62:61 (v4p3:96-97). The immediate goes in bits 127:96 (v4p3:96).
- **Pairing.** The same DW1 with a GRF source is rs:208, `2e2003bd`. The const block asserts that
  `mov_dw1(113, GRF, 0)` builds it.
- **send.** DW1 is null dst, Src0 GRF:F and Src1 IMM:D; DW2 is src0 = g113 <8;8,1> (v4p3:96,103). DW3 is the
  message descriptor (v4p3:216), with EOT in bit 127 (v4p3:98,215):
  - Message Length 8 (28:25): R, G, B and A for slots 15:0, two GRFs each (v4p1:285-286).
  - Response Length 0, no header. A header may be absent only for Render Target Write (v4p1:227,281).
  - Message type 1100b, Render Target Write (17:14, v4p1:229).
  - Last Render Target Select (12, v4p1:280), SIMD16 single source (10:8 = 000b, v4p1:280), binding table index 0.
  - All four dwords are exactly rs:223-226.
- **Registers.** The EOT source sits in g113-g120, inside R112-R127 as v4p3:218 requires. v4p1:225 allows EOT on
  a Render Target Write from a windower-dispatched PS.
- **Prefetch.** The kernel ends at `0x90`. The 128-byte instruction prefetch (v4p3:80) stays inside the mapped
  kernel page.

The colour is R 0.0, G 1.0, B 0.0, A 1.0. A B8G8R8A8_UNORM target stores it as the dword `FF00FF00`.

## 4. Memory: R10's own GGTT window

R10 uses four slots at GGTT slot `0x13410`, so ring start `0x13410000` has bits 31:29 = 0. That is past R9's
`0x13400..0x13401` with a gap. The window is checked unowned before any write with R9's census legs: the window
and both neighbours read the scratch fill. Every PTE is restored afterwards, behind a quiesced engine only. Each
page is allocated once and held for the boot (GEN7TLB).

| slot | page | contents |
| --- | --- | --- |
| +0 | ring | the 232-dword stream (§2), MI_NOOP after it |
| +1 | state | Surface State Base and Dynamic State Base. Offsets: `0x040` BLEND_STATE, `0x080` COLOR_CALC_STATE, `0x0C0` DEPTH_STENCIL_STATE, `0x100` CC_VIEWPORT, `0x140` SF_CLIP_VIEWPORT, `0x200` binding table, `0x220` SURFACE_STATE, `0x300` vertex buffer `(0,0) (32,0) (0,32)` as R32G32_FLOAT, `0x400` fence (seeded `5EED3D10`), `0x440` workaround post-sync scratch |
| +2 | kernel | Instruction Base; the kernel at `0x40` |
| +3 | rt | 32 x 32 B8G8R8A8_UNORM, pitch 128, seeded `10101010` |

## 5. Expected wire, and the decision rule

`tests gen7` only (R80). The line follows R9's line, and only on a run where R9 printed `verified`:

```
[gen7] r10 draw=3dprimitive r9=verified rows_cited=37/37 ring=rcs cand=mt prim=trilist verts=3 rt=32x32:b8g8r8a8 color=0xFF00FF00 fence=0x3D10C0DE read=0x3D10C0DE pixels=496 inside=496/496 edge=0 outside=0 other=0 rt0=0xFF00FF00 landed=1 head=0x3a0 tail=0x3a0 passed=noop-pad idle=1 restored=regs:1,fw:1,ptes:1 gpu_us=N us=N -> r10-triangle-verified
```

The counts come from the triangle (0,0)-(32,0)-(0,32), sampled at pixel centres (v2p1:305, PIXLOC_CENTER):

- `inside` is the 496 centres with x+y ≤ 30.
- `edge` is the 32 centres on the hypotenuse, x+y = 31. Under the upper-left rule they are expected to be 0, and
  they decide nothing.
- `outside` (x+y ≥ 32) must be 0.
- `other` counts pixels that are neither the colour nor the seed. It must be 0.

The verdicts, each named before the boot:

- `r10-triangle-verified`: the fence landed, the head reached the tail, inside = 496, outside = 0 and other = 0.
- `r10-triangle-partial`: the fence landed and the shape or the count is off.
- `r10-drew-nothing`: the stream retired and the fence landed with the RT untouched. Suspect the WM/PS dispatch
  or the RT write message, not the command stream.
- `r10-fence-miss`.
- `r10-head-stuck`: `passed=` names the last command retired (`passed=dummy_draw` would say the real draw hung).
- R6's envelope verdicts as `r10-*`.
- `-> REFUSED reason=… writes=0`.
- `-> GATED reason=r9-not-verified`.

**The design risk, stated before the boot.** Like R9, R10 runs on a context-less RCS: there is no MI_SET_CONTEXT
and no golden context. R9 answers whether the 3D front end accepts commands at all. R10 asks whether the WM
dispatches a PS thread and the render cache writes memory without the state i915 loads first. i915 loads that
state with this very rs batch, which draws nothing. A `r10-drew-nothing` with `passed=noop-pad` would point
there. The next step after that verdict is to run rs's state shape exactly, with a live rectangle and a live VB.

## 6. R11, designed: the window composite as a textured quad on the 3D pipe

No code here. R11 is the path Una's refined look waits on: shadows, translucency and scaled windows at no CPU
cost. It is R10 with three changes: a sampled source, an interpolated texture coordinate, and blending.

1. **Primitive.** One RECTLIST per window: 3 vertices, with the 4th implied (v2p1:12). This is what rs:152 does,
   and it is the RECTLIST preconditions R10 already meets: screen space, W = 1.0, viewport transform off, no
   clipping. The vertex is position plus texcoord: VE0 header STORE_0, VE1 R32G32_FLOAT position, VE2
   R32G32_FLOAT (u,v) with STORE_0, STORE_1_FP (v2p1:84-88).
2. **Attributes.** SBE takes 1 SF output attribute, read length 1, offset 1, exactly rs:112's `00400810`
   (v2p1:263-265). 3DSTATE_PS then sets Attribute Enable (bit 10, v2p1:286). WM keeps perspective-pixel
   barycentrics (rs:90).
3. **Source surface.** A second SURFACE_STATE, binding-table entry 1. It is the window's backing store:
   SURFTYPE_2D, B8G8R8A8_UNORM or B8G8R8X8, linear (or X-tiled once the compositor tiles), at its GGTT address
   (v4p1:62-78). A SAMPLER_STATE (v4p1:91-100) uses MAPFILTER_NEAREST for 1:1 composites and LINEAR for scaled
   ones, with TEXCOORDMODE_CLAMP. It is pointed to by 3DSTATE_SAMPLER_STATE_POINTERS_PS (v2p1:30) and counted in
   the PS sampler-count field (v2p1:284).
4. **Kernel.** rs:195-226 is that kernel's shape, and R11 re-encodes it field by field the way §3 does:
   - two `pln` (opcode 5Ah, v4p3:131,203) interpolate u and v from the barycentrics;
   - one `send` to the sampler (SFID 2h, v1p1:18) with a SIMD16 `sample` message returns RGBA in 8 GRFs. The
     message types are at v4p1:123-135, and rs:206 `08840001` is its descriptor to pair;
   - four movs and the RT-write EOT as in §3 (rs:207-226).
5. **Blending.** BLEND_STATE turns on Color Buffer Blend with src SRC_ALPHA and dst INV_SRC_ALPHA, so shadows and
   translucent chrome blend over what is behind them (v2p1:366-368). An opaque window keeps blend off, which is
   cheaper.
6. **Target and damage.** The render target is the scanout surface the compositor already owns. Damage is
   bounded by 3DSTATE_DRAWING_RECTANGLE (v2p1:244-246) or the scissor (SF DW2 bit 11, v2p1:260). There is one
   batch per frame: the stream in a batch buffer started by MI_BATCH_BUFFER_START (v1p3:169) instead of R10's
   one-shot ring, and a fence PIPE_CONTROL the compositor waits on before the flip.
7. **Seam.** `gen7` exports `composite(layers, damage) -> Result<fence, why>`. The compositor calls it only after
   `tests gen7` printed `r11-quad-verified` on this boot, and falls back to its CPU path on any other word. That
   compositor change lives in `video/*`, which is another lane; R11 lands the function and its witness first.
8. **Witness.** Under `tests gen7`, R11 composites a 32x32 source with a known gradient onto a seeded target at
   an odd offset. The CPU computes the same composite. The wire is
   `[gen7] r11 composite=textured-quad src=32x32 dst=64x64@(7,9) blend=off match=N/1024 -> r11-quad-verified`,
   then the same with `blend=src-alpha` against the CPU's blend.

R11 has open rows: SAMPLER_STATE's fields, the sample message descriptor and payload, and `pln`'s operand
layout. Each is a PRM page in the files above, to fill the way §2 does. R11 is gated on R10 verified on metal.

## 7. What this arc measured, and what it did not

- **One compile (R107 / R113 / R78).** The x86 kernel `cargo check` ran under the image line
  (`scripts/image-line.env`'s knobs plus `UNAOS_AETHER/MIDDEN/UNA/SONDE=1`). Its feature set was the one
  `./arroyo state` printed under that line, and it includes `gen7,gen7r8,gen7blit,gen7r9`. The command was
  `_cfg_slot_worker`'s cargo line run in `unaos/crates/kernel`: `cargo +nightly check --release --target
  ../../x86_64-unaos.json $NIGHTLY_FLAGS --features <that set>`. It returned rc=0, with no warning in
  `gen7_r9.rs` or `gen7_r10.rs`. `UNAOS_WIFI_FW_PATH` was not exported: it is media-only and no kernel source
  reads it (image-line.env header; `wifi/ladder.rs:89` names it only in a string).
- **Not run.** No `./arroyo check` matrix, no QEMU (R78), no knoboff. Knob-off is the seat's at the fold.
  `gen7_r10.rs` compiles only under `gen7r9`, as a child of `gen7_r9.rs`. With the knob off, neither file is in
  the build. With it on, R10 is reached only from R9's `run`, under `tests gen7`.
- **tests.rs is unchanged.** `tests gen7` (tests.rs:363-367) already runs the ladder, which calls `r9::run`,
  which calls `r10`. No registration line was needed.
- **Not touched.** `kepler*.rs`, `video/*` and gen7.rs. R9's encodings and helpers are reused through
  `use super::*`.
