# GEN7R9B (rmbp-ledger B626): R9 executes, R10 designed

Rung R9 (B504) used to stop with `page-PIPELINE_SELECT-uncited`. This arc fills its source rows from the
Linux i915 driver and writes the executing path. R10, the first draw, is designed here. It stops by name at
its first uncited row.

## 1. Sources and the rule they enter under

Intel's PRM hosts return 403 through the proxy, so the Linux i915 driver is the source. It was fetched with
`curl` on 2026-10-13 from `raw.githubusercontent.com/torvalds/linux/master/drivers/gpu/drm/i915/gt/`. At that
time `git ls-remote` gave master as `24a19b953e2a3d0ea5a8c4951a0ab8f32720c907`. All five files are
**SPDX MIT**. They were read for hardware facts only: opcodes, dword counts and bit fields. This follows
R95 §2 (the nouveau precedent) and R110 (b43). No function was transcribed and no text was copied beyond
names and values.

| key | file | sha256 |
| --- | --- | --- |
| `h` | `gt/intel_gpu_commands.h` | `d8d14b3edefb858418eb52506f5f7598ee2cd6bd8affa659e43faaf11c85e77c` |
| `rc` | `gt/gen7_renderclear.c` | `87d39b95f42dc024f8bcc672e6fb4c582ae125d1262b09c40f28864dbdc135f2` |
| `rs` | `gt/gen7_renderstate.c` | `52e5ca5bc8fa2f72d87e3224c9c33169ac092ae441db55d81ac56a5dd0a0c81d` |
| `cs` | `gt/gen6_engine_cs.c` | `940e5ed76582fc4762c2855ecba230887aaa08460526fcc25d4e7212d2c5c2aa` |
| `gt` | `gt/intel_gt_regs.h` | `ccc7fb4c51e94ce99dc2695f9351284e00dddef7fded90bf0be169c6539d77ac` |

Corroboration of facts already pinned in the tree:

- `gt:864` gives `FORCEWAKE_MT` = 0xA188 and `gt:1495` gives `FORCEWAKE_MT_ACK` = 0x130040. These are R6's
  `mt` row.
- `gt:1475-1476` gives `GFX_FLSH_CNTL_GEN6` = 0x101008 with bit 0 as the enable. This is the GTT flush in
  gen7.md §2.6.
- `intel_engine_regs.h:12,14,19,20` gives RING_TAIL/HEAD/START/CTL at base+0x30/0x34/0x38/0x3C. These are
  R5's RCS block.

## 2. R9: the row table (command, dwords, source line)

The ring sits in R9's own GGTT window: slot 0x13400 holds the ring and slot 0x13401 holds the **pinned
page**. The window is proven unowned at run time with R4b's legs, as `bcs_job` does it. Every
STATE_BASE_ADDRESS base is the pinned page. The post-sync target is the pinned page plus 0x40.

The ordering is renderclear's own, rc:412-419: invalidate, select, NOOP, invalidate, flush, SBA, invalidate.
The stream then ends with i915's gen7 RCS breadcrumb, cs:355-364.

| dw | command | dwords | source |
| --- | --- | --- | --- |
| 0-4 | PIPE_CONTROL (len 5): stall | `7A000003`, `CS_STALL\|STALL_AT_SCOREBOARD` = `00100002`, 0, 0, 0 | `GFX_OP_PIPE_CONTROL(len)` = 3<<29\|3<<27\|2<<24\|(len-2) h:287; CS_STALL bit 20 h:295; STALL_AT_SCOREBOARD bit 1 h:317; shape rc:360-365, cs:280-283 |
| 5-9 | PIPE_CONTROL (len 5): state-cache invalidate | `7A000003`, `00000004`, 0, 0, 0 | STATE_CACHE_INVALIDATE bit 2 h:316; rc:367-371; stall-before-invalidate workaround cs:333-337 |
| 10 | PIPELINE_SELECT (3D) | `69040000` | 3<<29\|1<<27\|1<<24\|4<<16 h:405-406; bit 0 = MEDIA h:407, so 3D = bit 0 clear; the gen7 null state's first dword is exactly `0x69040000` rs:19 |
| 11 | MI_NOOP | `00000000` | rc:414 (MI_NOOP after the select); in-tree pin §1.2.12 p.180 |
| 12-21 | invalidate pair | as dw 0-9 | rc:415 |
| 22-25 | PIPE_CONTROL (len 4): flush | `7A000002`, `00101021`, 0, 0 | RT_CACHE_FLUSH bit 12 h:306, DEPTH_CACHE_FLUSH bit 0 h:318, DC_FLUSH_ENABLE bit 5 h:313, CS_STALL bit 20 h:295; rc:344-349 |
| 26-35 | STATE_BASE_ADDRESS (len 10) | `61010008`; general/surface/dynamic/indirect/instruction base = `pin\|1` (x5); general bound 0, dynamic bound `1`, indirect bound 0, instruction bound `1` | 3<<29\|0<<27\|1<<24\|1<<16 h:402-403; BASE_ADDRESS_MODIFY bit 0 h:404; length and dword order rc:240-256; `0x61010008` is rs:20 |
| 36-45 | invalidate pair | as dw 0-9 | rc:419 |
| 46-49 | PIPE_CONTROL (len 4): post-sync write-immediate | `7A000002`, `011050A1`, `pin+0x40`, `3D09C0DE` | the gen7 RCS breadcrumb cs:355-364: RT flush h:306, depth flush h:318, DC flush h:313, FLUSH_ENABLE bit 7 h:312, post-sync op 01 (`PIPE_CONTROL_QW_WRITE` bit 14 h:301, field 15:14 h:302), GLOBAL_GTT_IVB bit 24 h:292 (the gen7 GGTT selector, in DW1), CS_STALL h:295; DW2 = address, DW3 = data cs:363-364 |
| 50-51 | MI_NOOP x2 | 0, 0 | QWord pad. MI_NOOP replaces the breadcrumb's MI_USER_INTERRUPT (cs:366) because nothing here takes the interrupt |

TAIL = 52 dwords = 208 bytes, which is QWord aligned. `gen7_r9.rs` has a compile-time check that the
constants build `0x69040000`, `0x61010008` and `0x7A000002`. These are the literal dwords at rs:19, rs:20
and h:287 at len 4.

**The "4th PIPE_CONTROL must carry CS_STALL" workaround (cs:300-304).** Excluding the invalidate-only
controls, every control in the stream carries CS_STALL: dw 0, 12, 22, 36 and 46. The post-sync carries
CS_STALL. Per cs:307, "CS_STALL suggests at least a post-sync write".

**Rows filled: 6 of 6** in `PAGES`: stall, invalidate, select, flush, sba, pipe_control. **Rows empty: 0.**

### The R9 wire (what the fold and the bench read)

On a verified run:

```
[gen7] r9 ring=rcs r6=r6-sentinel-hit cand=mt pipeline=3d sba=ok pipe_control=posted immediate=0x3D09C0DE read=0x3D09C0DE landed=1 head=0xd0 tail=0xd0 passed=noop-pad idle=1 restored=regs:1,fw:1,ptes:1 gpu_us=N us=N -> r9-3d-postsync-verified
[gen7] r10 draw=3dprimitive r9=verified rows_cited=2/22 writes=0 -> DECLINED reason=page-3DSTATE_PUSH_CONSTANT_ALLOC_VS-uncited field=pcalloc owed=IVB-PRM-Vol2-Pt1
:: GEN7LADDER: rungs=R1-R7 ggtt=boot-bank wake=acked r7=r7-blit-verified us=N replay=0 r9=verified -> PASS ::
```

The other verdicts, each named before the boot:

- `r9-postsync-landed-head-stuck`
- `r9-postsync-miss`: the head retired the stream and no write landed. Suspect the post-sync address or the
  GGTT bit, not the select.
- `r9-head-stuck`: `passed=` names the last command the CS retired, so a stall at the select or at the SBA is
  named.
- `r9-enable-void`, `r9-claim-write-void`, `r9-ring-would-not-disable`: R6's envelope verdicts.
- `-> REFUSED reason=<window-owned|fill-not-scratch|rcs-ring-live|…> writes=0`: refused before any write.
- `-> GATED reason=r6-did-not-execute-on-the-rcs`: R6 did not hit this run.

The ladder's `r9=` word is one of `verified`, `verified-unrestored`, `failed`, `refused`, `gated` or
`declined`.

### The design risk stated before the boot

R9 runs 3D state commands on a **context-less** RCS. No MI_SET_CONTEXT is issued and no default context is
loaded. R6 retired MI_STORE_DATA_IMM in that state (flight 4 onward). Whether the 3D front end accepts
PIPELINE_SELECT and SBA without a context image is the question this rung answers. `r9-head-stuck
passed=invalidate-0` would say it does not.

## 3. R10: the smallest draw, designed (it declines)

gen7_renderclear.c's own clear is a **media** walker, not a draw:

- rc:413 is `PIPELINE_SELECT_MEDIA`.
- rc:422-427 is MEDIA_VFE_STATE, MEDIA_INTERFACE_DESCRIPTOR_LOAD and MEDIA_OBJECT.

The first real draw needs the 3D front end, URB, VS through PS, a render target, an EU pixel shader and
3DPRIMITIVE. For those, i915 provides:

| row | i915 cites | state |
| --- | --- | --- |
| SURFACE_STATE (render target) | rc:151-168. DW0 SURFACE_2D (1)<<29, format B8G8R8A8_UNORM (0x0C0)<<18, render-cache read-write (1)<<8. DW1 address. DW2 (h/4-1)<<16 \| (w/4-1) in renderclear's units. DW3 pitch. DW7 shader channel select (4,5,6,7)<<25/22/19/16 | **cited** |
| BINDING_TABLE entry 0 | rc:183 (the surface-state offset from the surface base) | **cited** |
| 3DSTATE_PUSH_CONSTANT_ALLOC_VS | not named in i915 | **first uncited: R10 declines here** |
| 3DSTATE_URB_VS, _VS, _HS, _TE, _DS, _GS, _STREAMOUT, _CLIP, _SF, _SBE, _WM, _PS | not named; i915 names only VF_STATISTICS h:408, DX9_CONSTANTF h:419-422, SO_DECL_LIST h:423, BINDING_TABLE_EDIT_* h:426-435 | uncited |
| EU pixel shader | EU ISA, which no i915 header names | uncited |
| 3DSTATE_DEPTH_BUFFER (null), _BINDING_TABLE_POINTERS_PS, _DRAWING_RECTANGLE, _VERTEX_BUFFERS, _VERTEX_ELEMENTS, 3DPRIMITIVE | not named | uncited |

**Note for the next executor.** rs:18-158 is a complete gen7 3D null state. i915 executes it on every gen7
boot (the "golden context", `intel_renderstate.c`). It holds raw header dwords such as rs:151 `0x7b000005`
and ends at rs:158 with `0x05000000` (`/* cmds end */`), with relocations at byte offsets 0x0c, 0x10, 0x18
and 0x1ec (rs:10-16). The file carries **values without names**. Pairing each header with its 3DSTATE
command and fields is a PRM fact this arc does not have. No such pairing is written in the tree, and
R83 / R95 §2 forbids guessing it. When a PRM page or a named source pairs them, fill `R10_PAGES[n].src`
row by row. The executing path, on R9's verified envelope (R9's window plus a render-target page), is
written when the last row fills.

Wire today: `[gen7] r10 draw=3dprimitive r9=<word> rows_cited=2/22 writes=0 -> DECLINED
reason=page-3DSTATE_PUSH_CONSTANT_ALLOC_VS-uncited …`. When R9 did not verify, the line reads `-> GATED
reason=r9-not-verified`.

## 4. What this arc did not do

- The order is **R9 before R8**, and that is unchanged. R9 is called at the existing line-neutral site in
  `ladder::run` (gen7.rs, after R7, before the GEN7LADDER line). R8 runs after it in `gen7_line`. Moving the
  call would change the knob-off layout and leave the `r9=` field `not-reached`.
- GEN7BLIT `candidate=cpu` belongs to another arc and is untouched.
- No QEMU, knoboff or artifact measurement was run (R107). The banner-cert row is `unmeasured-here`.
