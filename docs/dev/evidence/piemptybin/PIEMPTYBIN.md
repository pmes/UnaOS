# PIEMPTYBIN — PI24, the `emptybin` rung built (v3d.md §49.25.11f/g)

Desk arc, 2026-10-13. Branch `exec-pi-piemptybin`, parent `bd3d399d`. No boot flew; the rung's first
wire is the bench Pi's next render-class boot (`docs/dev/OS/pi-queue.md`, METAL).

## Sources read (R95 §2 / R110: hardware facts only, cited file:line, no text copied)

Fetched 2026-10-13 from `https://raw.githubusercontent.com/raspberrypi/linux/rpi-6.6.y/` into the
executor's scratch (`~/unaos-bench/scratch/cloud/wave32/piemptybin/src/`, read-only, not in the tree).

| file | sha256 (first 16) | used for |
|---|---|---|
| `include/uapi/drm/v3d_drm.h` | `d502eb3728723fa5` | `V3D_PERFCNT_*` enum: opens `:363` (`enum {`), first member `:364` = 0, contiguous (no `=` in `:363-451`), `V3D_PERFCNT_NUM` `:451` |
| `drivers/gpu/drm/v3d/v3d_regs.h` | `9acec6b3e841ad8b` | PCTR `:355-391`, CLE `:299/315/316/326-335`, `V3D_ERR_STAT` bits `:528-544`, MMU VIO `:205/212` |
| `drivers/gpu/drm/v3d/v3d_perfmon.c` | `1187946b7511b709` | `v3d_perfmon_start` `:26-58` (no `v3d_performance_counters[]` table on this branch) |
| `drivers/gpu/drm/v3d/v3d_sched.c` | `9ef031f294a9a13a` | CT0QTS written from userspace `job->qts`, `:233-236` |

Not reachable: `gitlab.freedesktop.org` (Mesa: `v3d_packet.xml`, the tile-state layout). The in-tree §49
transcriptions are the CL packet source; no new packet was needed.

## Task 0

1. Tile-state words 1..3 = `1/2/1` (boot 15 leg H): **not decoded** — no reachable authority describes
   the array's contents (the kernel only points it). Printed raw.
2. Sources (value = `v3d_drm.h` line − 364):

| slot | id | name | `v3d_drm.h` | boot 15 leg H |
|---|---|---|---|---|
| 0 | 1 | FEP_VALID_PRIMS | :365 | d=0 |
| 1 | 14 | QPU_ACTIVE_CYCLES_VERTEX_COORD_USER | :378 | d=27 |
| 2 | 32 | CYCLE_COUNT (control; also `v3d_regs.h:384`) | :396 | moved |
| 3 | 16 | QPU_CYCLES_VALID_INSTR | :380 | d=52 |
| 4 | 33 | QPU_CYCLES_STALLED_VERTEX_COORD_USER | :397 | d=25 |
| 5 | **58** | **L2T_VCD_READS** (NEW; was 11 PTB_PRIM_CLIP) | :422 | — |
| 6 | **78** | **PSE_MEM_READS** (NEW; was 10 PTB_PRIM_VIEWPOINT_DISCARD) | :442 | — |
| 7 | 35 | PTB_PRIMS_BINNED | :399 | d=1 |

`V3D_PCTR_S0..S3` are 7-bit fields (`v3d_regs.h:372-383`), so ids 58 and 78 fit the select.

## Wire shapes (verbatim templates in `v3d.rs`; read order in v3d.md §49.25.11g)

- QEMU raspi4b: `:: v3d: emptybin SKIP reason=no-v3d-block (hub IDENT0 = 0x00000000: no V3D block — QEMU raspi4b models none) -> SKIP reason=no-v3d-block ::`
- Metal: `[v3d105] EMPTYBIN` · `[v3d105] CT0 HYGIENE` · `[v3d104] SHADREC READBACK` · `[v3d105] SHADREC DIFF` ·
  `[v3d105] PCTRARM` · `[v3d103] ERRSTAT (PRE-ARM)` · per leg `[v3d105] LEG E|H`, `[v3d101] INT_STS DECODE`,
  `[v3d105] EMPTYBIN BANK LEG E|H`, `[v3d103] ERRSTAT (E|H)` · `[v3d105] EMPTYBIN ROW B1..B4` ·
  `[v3d105] EMPTYBIN VERDICT … -> PASS outcome=B<n>` or `-> FAIL reason=inconclusive-<guard>`.
