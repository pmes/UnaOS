# GEN7R11 — RESUME (stopped by the seat 2026-10-14: wrong model; the arc resumes on Opus)

Arc: /root/unaos-bench/scratch/cloud/wave34/gen7r11.md. Ledger row B654 (NEW, rmbp-ledger). Worktree
/home/user/exec/gen7r11, branch exec-rmbp-gen7r11, parent 8da376cb. Executor: Fable 5.1 (the seat's launch error).

## DONE

NO CODE WAS WRITTEN. No file of the tree was edited before the stop. What is done is the read-first and the design.
Last compile command: none run (rc: n/a). No linter, gate, QEMU or suite was run on a changed tree (the three
linters were run once on the UNCHANGED tree as a baseline: charter rc=0, status-check PASS rc=0, appearance-check
rc=0 `literals_outside_theme=0`).

### Sources read (and what is NOT reachable from the cloud)

- Intel's IVB PRM PDFs are NOT reachable from this container: `xorg.freedesktop.org/docs/intel/IVB/*.pdf` and
  `gitlab.freedesktop.org` both answer `CONNECT tunnel failed, response 403` through the proxy; no local copy exists
  (`~/unaos-bench/scratch/rmbp-0915/local3/gen7r10/dl/` is absent on this host). Mesa (genxml gen70.xml,
  elk_inst.h) is 404 on the GitHub mirror and 403 on gitlab. The PRM page cites for every R11 row are therefore
  CARRIED from GEN7R10.md section 6 (bench-read), not re-read here; the bench must re-read them at the fold.
- i915 IS reachable: `raw.githubusercontent.com/torvalds/linux/24a19b95/drivers/gpu/drm/i915/gt/` (the same sha
  R9/R10 pinned). Fetched into the executor scratchpad (read as data only, nothing copied into the tree):
  gen7_renderstate.c sha256 52e5ca5bc8fa2f72d87e3224c9c33169ac092ae441db55d81ac56a5dd0a0c81d (3563 bytes),
  gen7_renderclear.c 87d39b95f42dc024f8bcc672e6fb4c582ae125d1262b09c40f28864dbdc135f2,
  intel_gpu_commands.h d8d14b3edefb858418eb52506f5f7598ee2cd6bd8affa659e43faaf11c85e77c.
  Line numbering matches R10's `rs:` cites exactly (rs:19 = 69040000, rs:112 = 00400810, rs:152 = 0000000f,
  rs:226 = 90031000). rs:18 is the array declaration, so rs:N is batch dword N-19 (state offsets below follow).

### THE FINDING: i915's gen7 null-state batch carries every row GEN7R10.md section 6 left open, bit for bit

The null state (rs:18-258) is a textured-quad pipeline: a 3-element vertex (header, position, TEXCOORD), SBE with
one attribute, a PS with one sampler and two binding-table entries, a SAMPLER_STATE, and a kernel that interpolates
u/v, samples, and writes the render target. R11 re-uses R10's stream and changes only these rows:

| row | R11 value | rs pairing (read here) | PRM page (carried from GEN7R10.md section 6) |
| --- | --- | --- | --- |
| 3DSTATE_VERTEX_ELEMENTS, 3 elements | header `02000000 22220000`; VE1 pos R32G32_FLOAT off 0 `02850000 11230000` (R10's); VE2 texcoord R32G32_FLOAT offset 8 = `(1<<25)\|(0x085<<16)\|8`, components `11230000` -> VUE dwords 8-11 = (u, v, 0, 1) | rs:133-139 (`78090005`, VE2 = `02f60004 11230000`: R16G16_SSCALED at offset 4, same component word) | v2p1:84-88 |
| 3DSTATE_VERTEX_BUFFERS | pitch 16 (`(1<<14)\|16`), 6 vertices x 16 B, end = vb+95 | rs:140-144 | v2p1:77-80 |
| 3DSTATE_URB_VS | UNCHANGED `02010040`: entry size 1 = 2 x 64 B rows holds the 48 B VUE | rs:39 | v2p1:34-35 |
| 3DSTATE_SBE DW1 | `00400810`: 1 attribute (27:22), URB read length 1 (15:11), read offset 1 (9:4) = VUE dword 8 | rs:111-112 EXACT | v2p1:263-265 |
| 3DSTATE_WM | UNCHANGED `20000800` (perspective-pixel barycentric: g2-g5 of the PS payload) | rs:89-90 | v2p1:276-282 |
| 3DSTATE_SAMPLER_STATE_POINTERS_PS | NEW header `gfx3d(0, 0x2F, 2)` = `782F0000`, DW1 = OFF_SAMP (32 B aligned) | rs:109-110 (`782f0000 00000280`) | v2p1:30 |
| SAMPLER_STATE (4 dw at OFF_SAMP) | `0, 0, 0, 0x492`: DW0 mag/min NEAREST, mip NONE, enabled; DW1 LODs 0; DW2 border 0; DW3 TCX/TCY/TCZ = CLAMP (2 at 8:6, 5:3, 2:0) and bit 10 NON-NORMALIZED coordinates (texcoords in texels) | rs:179-182 (state +0x280) EXACT | v4p1:91-100 |
| 3DSTATE_PS | DW2 = `(1<<27)\|(2<<18)` = `08080000`: sampler count 1 (29:27), BT entry count 2; DW4 = `(0x2F<<24)\|(1<<10)\|(1<<1)`: Attribute Enable (10) + SIMD16; DW5 = `00060000` GRF start 6 | rs:125-132 (DW2 `08080000`, DW4 `28000402`, DW5 `00060000`) | v2p1:283-290 |
| BINDING_TABLE | entry 0 = RT SURFACE_STATE, entry 1 = SRC SURFACE_STATE | rs:235-236 (`00000380 000003a0`) | v4p1:61 |
| SURFACE_STATE (source) | R10's SS_DW0 (2D, B8G8R8A8_UNORM, linear), base = src pages, w-1/h-1, pitch-1 | rc:151-168 (R10's row) | v4p1:62-78 |
| SURFACE_STATE (RT) | same, W-1/H-1, pitch P-1, base = RT pages | R10's row | v4p1:68-70 |
| 3DSTATE_DRAWING_RECTANGLE | 0..W-1, 0..H-1 | rs:147-150 | v2p1:244-246 |
| 3DPRIMITIVE | TRILIST (04h, R10's PRIM_TRILIST), 6 vertices (TWO textured triangles, as the arc says) | rs:151-157 (rs draws RECTLIST 0Fh with 3) | v2p1:90-94 |
| BLEND_STATE | UNCHANGED (off): R11 is the opaque composite; blend is section 6 item 5, later | rs:163-164 | v2p1:366-372 |

The kernel is rs:195-226 LITERALLY (same registers as R10: payload g113-g120, barycentric g2-g5, setup g6):

```
pln(16)  g113<1>:F g6.0<0;1,0>:F g2<8;8,1>:F      0080005a 2e2077bd 000000c0 008d0040   rs:195-198  u = c0*x + c1*y + c3
pln(16)  g115<1>:F g6.4<0;1,0>:F g2<8;8,1>:F      0080005a 2e6077bd 000000d0 008d0040   rs:199-202  v
send(16) g12<1>:UW g113<8;8,1>:F  SFID 2 sampler   02800031 21801fa9 008d0e20 08840001   rs:203-206
mov(16)  g113<1>:F g12<8;8,1>:F                    00800001 2e2003bd 008d0180 00000000   rs:207-210  R
mov(16)  g115<1>:F g14<8;8,1>:F                    00800001 2e6003bd 008d01c0 00000000   rs:211-214  G
mov(16)  g117<1>:F g16<8;8,1>:F                    00800001 2ea003bd 008d0200 00000000   rs:215-218  B
mov(16)  g119<1>:F g18<8;8,1>:F                    00800001 2ee003bd 008d0240 00000000   rs:219-222  A
send(16) null g113 RTW EOT                         05800031 20001fa8 008d0e20 90031000   rs:223-226  (R10's)
```

Decoded against R10's helpers (to const-assert in gen7_r11.rs):
- pln opcode 5Ah: `eu_dw0(0x5A, 0) == 0x0080_005A`. pln DW1 = `mov_dw1(reg, FILE_GRF, TYPE_F) | (FILE_GRF << 10)`
  (bit 42 = Src1.RegFile GRF) == `0x2E20_77BD` for g113, `0x2E60_77BD` for g115. DW2 `000000c0` = src0 g6.0
  <0;1,0> (RegNum 6 at 12:5), `000000d0` = g6.4 (SubReg 16 bytes). DW3 `008d0040` = src1 g2 <8;8,1>.
- sampler send DW0 `02800031` = `eu_dw0(EU_SEND, 2)` (SFID 2h = sampler, v1p1:18). DW1 `21801fa9` =
  `send_dw1(FILE_GRF, 12)` with `send_dw1(file, reg) = file | (2<<2) | (1<<5) | (7<<7) | (3<<10) | (1<<12) |
  (reg<<21) | (1<<29)`; the same builder gives R10's `20001FA8` for (ARF, 0). DW3 `08840001` = `(4<<25) | (8<<20) |
  (2<<17) | 1`: message length 4 (u,v x 2 GRF), response length 8 (RGBA x 2), no header, SIMD16 (18:17 = 2),
  message type 0 = sample (16:12), sampler 0 (11:8), binding table index 1 (7:0) (v4p1:123-135).
- mov src DW2 `008d0180/01c0/0200/0240` = g12/g14/g16/g18 <8;8,1> (RegNum = (dw2 >> 5) & 0xFF).
- The kernel is 32 dwords at KSP0 = 0x40, ends at 0xC0; the 128-byte prefetch stays in the page.

PS payload shape this relies on (R10's WM/PS rows): g0-g1 header, g2-g5 SIMD16 perspective-pixel barycentrics
(WM DW1 bit 11), g6 = attribute 0's setup constants (Dispatch GRF Start 6): dwords 0-3 for u (c0, cx, cy, pad),
4-7 for v. With SF view transform off and W = 1.0 the interpolation is affine.

### Geometry and the CPU reference (the comparison the arc asks for)

- Source window: scan `crate::video::wm::info(id)` for id in 1..=256 (ungated, read-only), take the highest `z`
  with w,h > 0 (the shell window that typed `tests gen7`). `wm::front_of_owner` is cfg(feature = "wc") so it is
  NOT used (a gen7r9 build without `wc` would red). WindowInfo: w, h = surface pixels, `scale` = the integer
  nearest-neighbour upscale the compositor applies (wm.rs:198-202).
- Source copy: `wm::thumb(id, out, w, h)` (wm.rs ~30291, ungated, DOCK2) with tw = w, th = h is an EXACT 1:1
  nearest copy (aspect-fit degenerates to dw = w, dh = h, sx = u, sy = v) and masks alpha to 0 (`px &
  0x00FF_FFFF`). Copy it row by row into R11's own page-aligned source pages with pitch `round_up(w*4, 64)`
  (the staging Vec is transient; the pages are held for the boot per GEN7TLB). The window's `surf` field is
  pub(super) to video/, so drivers/gpu cannot map the backing directly: R12 (present) is where that moves.
- RT: W = w*scale, H = h*scale, B8G8R8A8_UNORM linear, pitch P = round_up(W*4, 64), seeded 0x1010_1010 (alpha
  0x10 can never equal a thumb pixel, whose alpha is 0, so `pixels` = dwords != seed is sound).
- Vertices (x, y, u, v as R32G32_FLOAT x2, non-normalized u/v in texels): t0 (0,0,0,0) (W,0,w,0) (0,H,0,h);
  t1 (W,0,w,0) (W,H,w,h) (0,H,0,h). Both triangles see the same affine texcoord plane, so the shared diagonal
  (pixel centres exactly on it exist when W,H share a factor; R10's `edge` is the same line) is owned by exactly
  one triangle under the top-left rule and the colour is identical either way. Two triangles were chosen over
  RECTLIST because the implied 4th vertex's rule (v3 = v0 + v2 - v1) could not be cited from here.
- Nearest sample at pixel centre: u = (x + 0.5) * w / W = (x + 0.5) / scale, texel = floor(u) = x / scale
  (integer division), which is exactly the compositor's upscale (thumb's `sx = u * sw / dw`). CPU reference:
  `want(x, y) = src[(y / scale) * pitch_src/4 + x / scale]`; UNORM8 -> float -> UNORM8 round-trips exactly, blend
  off writes all four channels, so a dword compare is exact. `mismatch` = count of RT dwords != want; the line
  prints the first mismatch's (x, y, got, want).
- Caps and refusals (before any write): RT bytes > 24 MiB or W/H > 8192 -> `rt-too-large`; no window ->
  `no-window`; thumb false -> `no-surface`; the GGTT window: base slot 0x13420 (past R10's 0x13410..0x13413),
  N = 3 + src pages + RT pages slots, census of every slot + both neighbours as R10 does; the four-slot census
  code is reused with N.

### Structure (how it hooks in; the knob is UNCHANGED — R11 rides `gen7r9` exactly as R10 does; no Cargo/arroyo/builder edit)

- NEW `unaos/crates/kernel/src/drivers/gpu/gen7_r11.rs`, first lines `//! GEN7R11 (B654, R121) — rung R11 ...` and
  `//! CHARTER: Kernel — driver`, a CHILD of gen7_r10.rs: at R10's tail `#[path = "gen7_r11.rs"] mod quad;` so
  `use super::*;` in R11 sees R10's private consts/helpers (gfx3d headers, SS_DW0, mov_dw1/eu_dw0, Ring, poke,
  KSP0, OFF_*) and R9's/gen7's through R10's glob, the mechanism R10 already compiles on.
- gen7_r10.rs: rename the body of `pub(super) fn run(r9: &str)` to `fn run_inner(r9) -> &'static str` returning
  "gated" / "declined" / "refused" / the verdict word; `pub(super) fn run(r9) { quad::run(run_inner(r9)) }`.
  R11 gates on `r10 == "r10-triangle-verified"` and prints `-> GATED reason=r10-not-verified` otherwise.
- gen7_r11.rs `run(r10)`: table check (R11_PAGES: every row filled as above; src `None` declines by name), the
  window lookup + copy, `exec` (R10's exec with: N-slot window, state page additions OFF_SAMP = 0x240 and a second
  SURFACE_STATE at OFF_SS2 = 0x260, BT with two entries, the 6-vertex VB at 0x300 (96 B), the R11 stream), the
  read-back compare, then the two lines:
  `[gen7] r11 draw=quad r10=<word> rows_cited=N/N ring=rcs cand=mt prim=trilist verts=6 src=<id>:<w>x<h>@s<scale>
  rt=<W>x<H>:b8g8r8a8 pitch=<P> sampled=<ok|no> pixels=<n>/<W*H> mismatch=<m> first=(x,y) got=0x.. want=0x..
  fence=0x3D11C0DE read=0x.. landed=1 head=.. tail=.. passed=.. idle=1 restored=regs:1,fw:1,ptes:1 gpu_us= us=
  -> r11-quad-verified` and `:: GEN7: r11 draw=quad src=<id> rt=<W>x<H> sampled=ok mismatch=0 landed=1 -> PASS ::`.
  Verdicts: r11-quad-verified (fence hit, idle, pixels == W*H, mismatch 0); r11-drew-nothing (pixels 0);
  r11-quad-mismatch (fence hit, pixels > 0, mismatch > 0: sampled=no); r11-fence-miss; r11-head-stuck
  passed=<cmd>; R6's envelope verdicts as r11-*; REFUSED writes=0; GATED.
- Fence 0x3D11_C0DE, seed 0x5EED_3D11 (R10's pattern). `[gen7] r11` tokens exist only under gen7r9.
- tests.rs: NO change needed (R11 rides `tests gen7` through r9 -> r10 -> r11).
- Docs: a dated R11 paragraph in docs/dev/OS/08_VIDEO/gen7.md after the R10 paragraph (line ~1136, before
  `**PPS**`); docs/dev/evidence/gen7r11/GEN7R11.md with the table above (sources section must state the PRM was
  unreachable and the pages are carried from GEN7R10.md section 6); ledger row B654 appended after B645
  (rmbp-ledger.md line 680, section B table), 7 cells, owner `rmbp`, flies-on `rmbp`, status beginning
  `fixed-unflown — GEN7R11 on exec-rmbp-gen7r11 @the commit carrying this row (parent 8da376cb): compiled (<the
  arc's cargo line> rc=0), proof owed to the fold`, witness `x86 QEMU compile, then the rmbp bench`, no claim word.
- Linters: appearance-check scope is PAINTERS (video/ + listed files); gen7_r10.rs's RT_PIXEL/RT_SEED were not
  flagged on the baseline run, so R11's seed/fence words need no allow row unless the run says otherwise.
- The coordinator's later correction: the pre-commit leg is `cd unaos && ./arroyo gates > gates.log 2>&1` (host
  gates, no build) and its `GATES: 17 run 17 green -> PASS` line goes in the REPORT; QEMU/check/test stay forbidden.

## NOT DONE (the exact next step)

1. Write gen7_r11.rs as designed above (and the 3-line change to gen7_r10.rs's run / tail `mod quad`).
2. Write GEN7R11.md, the gen7.md R11 paragraph, the B654 row.
3. Compile ONCE with the arc's line (`cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2
   CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc
   -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json
   --features "$(cat /root/unaos-bench/scratch/cloud/wave34/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"`), fix once if red.
4. `./arroyo gates` (per the correction) and the three linters; commit with the design in the body; report.

## Learned that the arc file does not say

- The cloud cannot reach the PRM or mesa; only i915 via GitHub. The arc's "cited row by row as R10's table was"
  is satisfiable from here only by pairing rs literals and carrying section 6's page numbers; say so in the row.
- rs's sampler uses NON-NORMALIZED coordinates (DW3 bit 10); keep it, it makes the texcoords texel units and the
  nearest rule identical to the compositor's integer upscale.
- `wm::thumb` drops alpha; the compare is still exact because the GPU writes the sampled alpha (0) back.
- `wm::front_of_owner` is `wc`-gated; `wm::info` and `wm::thumb` are not.
