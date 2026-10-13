# GEN7R11 (rmbp-ledger B654, R121): the first composited surface on the Ivy Bridge RCS

R121: "una interface waits for 3D so get on it". GEN7R10 (B638) drew one solid triangle on the RCS and designed
R11 in GEN7R10.md §6, leaving three rows open (SAMPLER_STATE, the sample message, `pln`). This arc fills them,
writes the rung in `unaos/crates/kernel/src/drivers/gpu/gen7_r11.rs`, and compares the GPU's composite of a real
window with the CPU's, pixel for pixel. The GPU does not present: no scanout changes. R12 is the present.

Nothing here has flown. The arc compiled one x86 leg (§7). The bench owes the flight.

## 1. Sources, and what the cloud could not reach

- **The IVB PRM was NOT reachable from the cloud seat that wrote this rung.** `xorg.freedesktop.org/docs/intel/IVB/*.pdf`
  and `gitlab.freedesktop.org` both answered `CONNECT tunnel failed, response 403` through the proxy, and no local
  copy exists on this host. Mesa's genxml and `elk_inst.h` were 404 on the GitHub mirror and 403 on gitlab.
  **Every PRM page cite below (`v1p1`, `v2p1`, `v4p1`, `v4p3`, keys as GEN7R10.md §1) is CARRIED from GEN7R10.md
  §6 and §2, where the bench read them. They were not re-read here. The bench re-reads them at the fold.**
- **i915 was reachable** at the commit R9/R10 pinned: `raw.githubusercontent.com/torvalds/linux/24a19b95/drivers/gpu/drm/i915/gt/`.
  `rs` = `gen7_renderstate.c`, sha256 `52e5ca5bc8fa2f72d87e3224c9c33169ac092ae441db55d81ac56a5dd0a0c81d`
  (3563 bytes; the same file GEN7R9B.md and GEN7R10.md §1 hash). `rc` = `gen7_renderclear.c`, sha256
  `87d39b95f42dc024f8bcc672e6fb4c582ae125d1262b09c40f28864dbdc135f2`. `h` = `intel_gpu_commands.h`, sha256
  `d8d14b3edefb858418eb52506f5f7598ee2cd6bd8affa659e43faaf11c85e77c`. Read as data only; no text is in the tree.
  Line numbers match R10's `rs:` cites exactly (rs:19 = `69040000`, rs:112 = `00400810`, rs:152 = `0000000f`,
  rs:226 = `90031000`). rs:18 is the array declaration, so batch dword `d` is rs line `d + 19`.

**The finding.** i915's gen7 null-state batch (rs:18-258) is a textured-quad pipeline: a 3-element vertex (header,
position, texcoord), SBE with one attribute, a PS with one sampler and two binding-table entries, a SAMPLER_STATE,
and a kernel that interpolates u/v, samples and writes the render target. Every row GEN7R10.md §6 left open pairs
to an rs literal bit for bit. The const blocks in `gen7_r11.rs` assert that R11's field builders reproduce those
literals, including all 32 dwords of the kernel (`same(&KERNEL11, &RS_KERNEL)`).

## 2. The rows R11 changes (R10's other rows run unchanged)

R11 re-runs R10's whole 37-row stream (GEN7R10.md §2) and changes or adds only these rows. `R11_PAGES` in
`gen7_r11.rs` carries them: **17 of 17 cited**. `state`, `kern`, `src`, `rt` are GGTT addresses (§4).

| # | row | R11 value (every bit not named is R10's) | rs pairing (read here) | PRM page (carried from GEN7R10.md §6/§2) |
| --- | --- | --- | --- | --- |
| 1 | 3DSTATE_URB_VS | UNCHANGED `02010040`: entry size 1 = 2 x 64 B rows holds the 48 B VUE (header, position, texcoord) | rs:38-39 | v2p1:34-35 |
| 2 | 3DSTATE_SBE DW1 | `00400810`: 1 SF output attribute (27:22), URB read length 1 (15:11), read offset 1 (9:4) = VUE dword 8, the texcoord | rs:111-112, exact | v2p1:263-265 |
| 3 | 3DSTATE_WM | UNCHANGED `20000800`: perspective-pixel barycentrics, the g2-g5 the `pln`s read | rs:89-90 | v2p1:276-282 |
| 4 | BLEND_STATE | UNCHANGED (off): R11 is the opaque composite. Blend is GEN7R10.md §6 item 5, later | rs:163-164 | v2p1:366-372 |
| 5 | 3DSTATE_SAMPLER_STATE_POINTERS_PS | NEW, after the viewport pointers: header `782F0000` (opcode 0, sub 2Fh, 2 dw); DW1 = `0x240`, the SAMPLER_STATE offset from Dynamic State Base (32-byte aligned) | rs:109-110 (`782f0000 00000280`) | v2p1:30 |
| 6 | SAMPLER_STATE (4 dw at state+0x240) | `0, 0, 0, 00000492`. DW0: sampler enabled, mip filter NONE, mag/min MAPFILTER_NEAREST. DW1: LODs 0. DW2: border colour pointer 0 (unused under CLAMP). DW3: Non-normalized Coordinate Enable (bit 10: u, v in texels), TCX/TCY/TCZ = TEXCOORDMODE_CLAMP (2h at 8:6, 5:3, 2:0) | rs:179-182 (state +0x280), exact | v4p1:91-100 |
| 7 | BINDING_TABLE (state+0x200) | two entries: 0 -> RT SURFACE_STATE (`0x220`, the RT write's index 0), 1 -> source SURFACE_STATE (`0x260`, the sample message's index 1) | rs:235-236 (`00000380 000003a0`) | v4p1:61 |
| 8 | SURFACE_STATE, source (state+0x260) | R10's DW0 `23000000` (SURFTYPE_2D, B8G8R8A8_UNORM, linear). DW1 = `src`. DW2 = (h-1)<<16 / (w-1). DW3 = source pitch - 1, pitch = round_up(w*4, 64). DW4-7 = 0 | rc:151-168 (R10's row) | v4p1:62-78 |
| 9 | SURFACE_STATE, RT (state+0x220) | R10's, at the window-sized RT: W-1, H-1, pitch P-1, base `rt` | R10's row | v4p1:62-81 |
| 10 | 3DSTATE_PS | DW2 = `08080000`: Sampler Count 1 (29:27), Binding Table Entry Count 2 (25:18). DW4 = `2F000402`: R10's max threads and SIMD16, plus Attribute Enable (10). DW5 UNCHANGED `00060000` (GRF start 6) | rs:125-132 (DW2 `08080000`, DW4 `28000402`, DW5 `00060000`) | v2p1:283-290 |
| 11 | EU `pln` x2 | §3 | rs:195-202 | v4p3:131,203 |
| 12 | EU sampler `send` | §3 | rs:203-206 | v4p1:123-135; SFID v1p1:18 |
| 13 | EU 4 `mov` + RT-write EOT | §3 (the EOT send is R10's) | rs:207-226 | v4p3:127,129,215-218; v4p1:225,229,280-286 |
| 14 | 3DSTATE_DRAWING_RECTANGLE | DW2 = (H-1)<<16 / (W-1), the window-sized RT | rs:147-150 | v2p1:244-246 |
| 15 | 3DSTATE_VERTEX_BUFFERS | VERTEX_BUFFER_STATE DW0 = `00004010`: pitch 16 (x, y, u, v as R32 floats). End = vb + 95 (6 vertices x 16 B) | rs:140-144 | v2p1:77-80 |
| 16 | 3DSTATE_VERTEX_ELEMENTS | header `78090005` (3 elements). VE0 and VE1 are R10's. VE2 = `02850008 11230000`: VB 0, Valid, R32G32_FLOAT, offset 8, STORE_SRC, STORE_SRC, STORE_0, STORE_1_FP -> VUE dwords 8-11 = (u, v, 0.0, 1.0) | rs:133-139 (`78090005`; VE2 `02f60004 11230000` is the same component word on R16G16_SSCALED at offset 4) | v2p1:84-88; format v4p1:79 |
| 17 | 3DPRIMITIVE | TRILIST (04h, R10's), vertex count 6: two textured triangles | rs:151-157 (rs draws RECTLIST 0Fh with 3) | v2p1:75,90-94 |

**Why two triangles, not RECTLIST.** GEN7R10.md §6 item 1 proposed one RECTLIST. RECTLIST's implied fourth vertex
rule could not be cited from here (the PRM was unreachable), and R10's TRILIST rows are already in the tree. Two
TRILIST triangles see one affine texcoord plane, so the shared diagonal's colour is identical whichever triangle
owns it under the top-left rule.

## 3. The pixel shader: `KERNEL11` in gen7_r11.rs, rs:195-226 literally

32 dwords at `kern + 0x40`; ends at `0xC0`, so the 128-byte prefetch stays in the kernel page.

| instr | DW0 | DW1 | DW2 | DW3 | rs |
| --- | --- | --- | --- | --- | --- |
| `pln(16) g113<1>:F g6.0<0;1,0>:F g2<8;8,1>:F` | `0080005A` | `2E2077BD` | `000000C0` | `008D0040` | 195-198 (u) |
| `pln(16) g115<1>:F g6.4<0;1,0>:F g2<8;8,1>:F` | `0080005A` | `2E6077BD` | `000000D0` | `008D0040` | 199-202 (v) |
| `send(16) g12<1>:UW g113<8;8,1>:F sampler` | `02800031` | `21801FA9` | `008D0E20` | `08840001` | 203-206 |
| `mov(16) g113<1>:F g12<8;8,1>:F` | `00800001` | `2E2003BD` | `008D0180` | `00000000` | 207-210 (R) |
| `mov(16) g115<1>:F g14<8;8,1>:F` | `00800001` | `2E6003BD` | `008D01C0` | `00000000` | 211-214 (G) |
| `mov(16) g117<1>:F g16<8;8,1>:F` | `00800001` | `2EA003BD` | `008D0200` | `00000000` | 215-218 (B) |
| `mov(16) g119<1>:F g18<8;8,1>:F` | `00800001` | `2EE003BD` | `008D0240` | `00000000` | 219-222 (A) |
| `send(16) null g113 RTW EOT` | `05800031` | `20001FA8` | `008D0E20` | `90031000` | 223-226 (R10's) |

Decoded against R10's builders (each const-asserted in gen7_r11.rs):

- **pln.** Opcode 5Ah: `eu_dw0(0x5A, 0) == 0x0080005A`. DW1 = `mov_dw1(reg, GRF, F) | (GRF << 10)`: R10's mov DW1
  with Src1.RegFile = GRF (bit 42). DW2 = src0 g6.0 / g6.4 with region <0;1,0> (RegNum 6 at 12:5, SubReg byte 0
  or 16). DW3 = src1 g2 <8;8,1>, the same region builder as R10's SEND_DW2 (`rgn8(2)`).
- **sampler send.** DW0 = `eu_dw0(EU_SEND, 2)`: SFID 2h = sampler (v1p1:18). DW1 = `send_dw1(GRF, 12)`, where
  `send_dw1(ARF, 0)` is R10's `20001FA8`. DW2 = `rgn8(113)`, R10's SEND_DW2. DW3 = `(4<<25)|(8<<20)|(2<<17)|1`:
  message length 4 (u, v x 2 GRF), response length 8 (RGBA x 2), no header, SIMD16 (18:17 = 2), message type 0 =
  sample (16:12), sampler 0 (11:8), binding table index 1 (7:0) (v4p1:123-135).
- **mov.** `mov_dw1(113 + 2c, GRF, 0)` (R10's asserted builder) and src0 `rgn8(12 + 2c)`.
- **Payload.** R10's: g0-g1 header, g2-g5 SIMD16 perspective-pixel barycentrics (WM DW1 bit 11), g6 = attribute
  0's setup constants (Dispatch GRF Start 6): dwords 0-3 for u, 4-7 for v. With the SF view transform off and
  W = 1.0, the interpolation is affine.

## 4. Memory: R11's own GGTT window, and the source

Slot `0x13420` (ring start `0x13420000`, bits 31:29 = 0), past R10's `0x13410..0x13413` with a gap. The window is
`3 + src pages + RT pages` slots and must end below R5's first far probe (`0x20000`) or the rung refuses
`window-too-large`. Every slot and both neighbours are checked unowned before any write (R10's census legs over
`n` slots). Every PTE is restored afterwards, behind a quiesced engine only. The pages are allocated once per size
and held for the boot (GEN7TLB); a run needing more pages allocates afresh and the smaller block stays held.

| slot | page | contents |
| --- | --- | --- |
| +0 | ring | the 236-dword stream (R10's 232 + sampler pointers 2 + VE2 2, then the pad), MI_NOOP after it |
| +1 | state | R10's offsets, plus `0x240` SAMPLER_STATE, `0x260` source SURFACE_STATE; BT at `0x200` has 2 entries; `0x300` the 6-vertex VB (96 B); `0x400` fence seeded `5EED3D11` |
| +2 | kernel | Instruction Base; `KERNEL11` at `0x40` |
| +3 .. | source | the window's pixels, w x h, pitch round_up(w*4, 64), B8G8R8A8 |
| .. | rt | W x H = (w*scale) x (h*scale), pitch round_up(W*4, 64), seeded `10101010` |

**The source window.** The front window: the highest `z` (ties by id) among `video::wm::info(id)` for id in
`1..=wm::slots()` (the live row count; ids are slot + 1, no hardcoded bound, R90) with w, h > 0. That is the shell
window that typed `tests gen7`. `wm::front_of_owner` is `wc`-gated, so it is not used: a `gen7r9` build without
`wc` would not compile. `WindowInfo.scale` is the integer nearest-neighbour upscale the compositor applies
(wm.rs:198-202), so W x H is the window's composited size on the panel.

**The copy.** `wm::thumb(id, out, w, h)` (wm.rs, DOCK2, ungated, read-only) at tw = w, th = h is the compositor's
own nearest sampler at 1:1 (the aspect fit degenerates to the full frame) and masks alpha to 0. The window's `surf`
field is `pub(super)` to `video/`, so `drivers/gpu` cannot map the backing store directly; R12 (present) is where
that moves. The arc keeps `video/` read-only.

**Refusals, before any write.** RT bytes > 24 MiB or W or H > 8192 -> `rt-too-large`; no window -> `no-window`;
`thumb` false -> `no-surface`; heap short -> `alloc`; the window owned, the fill not scratch, the RCS ring live,
a page above 4 GiB -> R10's reasons.

## 5. The CPU reference, expected wire, and the decision rule

**The reference is the compositor's upscale.** Vertices (x, y, u, v), u/v non-normalized (texels):
(0,0,0,0) (W,0,w,0) (0,H,0,h) and (W,0,w,0) (W,H,w,h) (0,H,0,h). At pixel centre x + 0.5, u = (x + 0.5) / scale,
and the nearest texel is floor(u) = x / scale (integer division): the fraction is at least 0.5/scale from either
texel edge. That is exactly the compositor's integer nearest upscale (`thumb`'s `sx = u * sw / dw`). So
`want(x, y) = src[y / scale][x / scale]`. UNORM8 -> float -> UNORM8 round-trips exactly, and blend off writes all
four channels, so a dword compare is exact. The sampled alpha is 0 and the seed's is 0x10, so `pixels` (dwords
that differ from the seed) is a sound count of what the GPU wrote.

`tests gen7` only (R80). The line follows R10's line on every run:

```
[gen7] r11 draw=quad src=<id>:<w>x<h>@s<scale> rt=<W>x<H>:b8g8r8a8 sampled=ok mismatch=0 landed=1 r10=r10-triangle-verified rows_cited=17/17 ring=rcs cand=mt prim=trilist verts=6 pitch=<P> slots=<n> pixels=<W*H>/<W*H> first=none at=(0,0) got=0x00000000 want=0x00000000 fence=0x3D11C0DE read=0x3D11C0DE head=0x3b0 tail=0x3b0 passed=noop-pad idle=1 restored=regs:1,fw:1,ptes:1 gpu_us=N us=N -> r11-quad-verified
:: GEN7: r11 draw=quad src=<id> rt=<W>x<H> sampled=ok mismatch=0 landed=1 -> PASS ::
```

The verdicts, each named before the boot:

- `r11-quad-verified` (`-> PASS`): the fence landed, the head reached the tail, every RT pixel was written
  (pixels = W*H) and equals the CPU composite (mismatch = 0).
- `r11-drew-nothing`: the fence landed with the RT untouched. R10 drew this boot, so suspect the attribute,
  sampler or binding-table path, not the command stream.
- `r11-quad-mismatch` (`sampled=no`): the GPU wrote and some pixel differs; `first=mismatch at=(x,y) got= want=`
  names the first one. A half-scale image would say the sampler read normalized coordinates; a constant colour,
  that the `pln` setup register is off.
- `r11-fence-miss`; `r11-head-stuck` with `passed=` naming the last command retired; R6's envelope verdicts as
  `r11-*`.
- `-> REFUSED reason=… writes=0`; `-> GATED reason=r10-not-verified` when R10 did not print
  `r10-triangle-verified` this run. Every path also prints one `:: GEN7: r11 … -> <PASS|verdict|GATED|REFUSED> ::`.

## 6. Structure (the knob is unchanged)

- `gen7_r11.rs` is a child of `gen7_r10.rs` (`#[path = "gen7_r11.rs"] mod quad;` at R10's tail), so
  `use super::*` sees R10's private encodings and builders and, through R10's own glob, R9's and gen7's.
- `gen7_r10.rs`: the body of `run` became `run_inner(r9) -> &'static str` (`gated` / `declined` / `refused` / the
  verdict word), and `run(r9)` is `quad::run(run_inner(r9))`. No R10 line changes shape.
- R11 rides `gen7r9` exactly as R10 does: no Cargo, `arroyo` or builder edit, no new knob. With the knob off,
  none of gen7_r9/r10/r11 is in the build.
- `tests.rs` is unchanged: `tests gen7` runs the ladder -> `r9::run` -> `r10` -> R11.
- Not touched: `video/*` (read only: `wm::info`, `wm::slots`, `wm::thumb`), `kepler*.rs`, gen7.rs, gen7_r9.rs.

## 7. What this arc measured, and what it did not

- **One compile (R107 / R113).** The arc's x86 line, in `unaos/crates/kernel`:
  `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave34/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"`.
  Its rc is in the commit carrying this file and in B654.
- **The host gates.** `./arroyo gates` from `unaos/` before the commit (rule 12); its line is in the report.
- **Not run.** No `./arroyo check` matrix, no QEMU, no knoboff, no suite (R107). Knob-off is the seat's at the fold.
- **Not measured.** The PRM pages (carried, §1); the draw itself (metal-only: Ivy Bridge HD 4000 on the rMBP).
