# KEPLERGR — the GK107 GR channel behind the ONE fifo bind path, with an executed unwind (ledger B421)

Branch `exec-rmbp-keplergr` (cut from df15f19d; GPUBLIT3 `exec-rmbp-gpublit3` @748e7a30 MERGED in as M0, so the seat's fold of
both branches has one implementation). Files: `drivers/gpu/kepler_fifo.rs` (two new submodules at the tail: `host` = the shared
bind path, `kgr` = the GR leg), `drivers/gpu/kepler_gpublit.rs` (private copies replaced by calls), `drivers/gpu/mod.rs` (module
gate widened), `drivers/gpu/kepler.rs` (one arming call after `kepler_gpublit::arm`), `unaos/scripts/kepler-capture.list`.
No new file under video/ fs/ install/ selfhost/ or the shell files (no CHARTER line owed), no knob, no verb, no dotfile. Structured by
DRIVERS-METHOD §6. Sources (scratchpad, never copied): nouveau v6.10 `nvkm/engine/fifo/{gk104,gf100,runl,chan}.c`,
`nvkm/subdev/top/gk104.c`, `nvkm/subdev/mmu/vmmgf100.c`; open-gpu-doc `cla06f.h`, gv100 `dev_ram.ref`.

**Finding.** The September GR leg (chid 1, `kepler.rs` fifo leg, sittings #8..#43) read `err=00000002` at 0x252c on EVERY boot
and was parked by inference ("the missing actor is the FECS ctx machinery", KEPLER-METAL-LOG s#37/s#43). nouveau names 0x252c
code 2 SNOOP_WITHOUT_BAR1 (gk104.c:600) and writes 0x2254 (the USERD BAR1 table) at fifo init (gk104.c:749); no UnaOS code
wrote 0x2254 until GPUBLIT3. So the GR wall was never tested with its precondition met. **The seam:** `kepler_fifo::host` is the
one fifo-init / RAMFC / bind / commit / decode / unwind path; GPUBLIT's CE leg (chid 2) and the GR leg (chid 1) both call it,
and the ONE USERD table (GPUBLIT3's, `0x2254`) is registered there and read by both (`host::utab()`, slot = table + chid*0x200).

## 1. Capture

None of a WORKING state. Owed from the bench: a Linux/nouveau boot reading the registers in `unaos/scripts/kepler-capture.list`
(0x2254, 0x2630, 0x2a04, 0x2100, 0x252c, the PBDMA 0x04013c words, PFIFO_CHAN of a live channel and its RAMFC words). No value
is invented; the list names only what to read.

## 2. Rung ledger — the GR channel (carried from KEPLER-METAL-LOG sittings #8..#43, never restarted)

| # | hypothesis | writes | discriminator (confirm / refute) | status |
|---|---|---|---|---|
| G0 | PREMISE: a GR channel binds on this GK107 at all | none new (G1 is the treatment) | `:: KGR: bind=ok` confirms; `bind=<reason>` with r2254 set refutes the premise as stated and names the next wall | **open** — every September read was `err=00000002` with 0x2254 = 0 (s#9 "post-init err=0x00000002", s#37 `poll-control valid-only chan=00002000 err=00000002`), i.e. never read under its precondition |
| G1 | err=2 is SNOOP_WITHOUT_BAR1: chid 1's bind needs 0x2254 set and its USERD in that table's slot | 0x2254 (shared), RAMFC +0x08/+0x0C = table + 0x200 | `bind_post=00` confirms; `bind_post=02` with `r2254` enabled refutes (then G1b) | **open** — CE leg's R3 tests the same register on the same boot first |
| G1b | alt: the table must be BAR1-mapped for the host's snoop (KF24 identity may not hold for the table) | — | `bind_post=02` with r2254 set | open (reopens if G1 refutes) |
| G2 | POLL_ENABLE (bit 30) was the September subject (s#9 naming) | none | s#37 `poll-control valid-only ... err=00000002`: VALID-only refused identically | **refuted s#37** (`poll-control valid-only chan=00002000 err=00000002 stat=00000000`) |
| G3 | USERD_SNOOP 0x2a1c is the poll area | — | s#10 `USERD_SNOOP orig=0` write 1 reads 0, err stays 2 | **refuted s#10** (writes read as zero, err unchanged) |
| G4 | RAMFC differs from `gk104_chan_ramfc_write` (+0x0C extra bit 31, +0x94 devm, +0xE4 priv) | RAMFC via `host::ramfc_write` | `ramfc_*` readback = nouveau's; a PBDMA intr0 GPENTRY/PBENTRY after fetch refutes sufficiency | open (applied) |
| G5 | runlist 0 blocked by SCHED_DISABLE bit 0 | 0x2630 &= ~1 | `r2630` pre bit 0 + commit completes | open (applied) [ONE-SOURCE: nouveau] |
| G6 | the PBDMA fetches a GR channel's GPFIFO without a FECS context (host methods NOP + SEMAPHORE are PBDMA-executed) | push NOP + host semaphore release | `nop=ok` (sem = payload) confirms; `ramfc_get>0` with `nop=stuck` and `eng0` `chsw=1`/`load=1` = the GR ctxsw is the wall (FECS, K-GPU-4); `ramfc_get=0` with commit ok = the scheduler never loaded the channel | open — FIRST read on this boot if G1 confirms |
| G7 | the September "missing actor is FECS" conclusion | — | G6's `eng0` decode | **parked: by inference (s#37/s#43), reopened now** — G6 discriminates it |

## 3. This boot's tree (`tests kgr`, after the boot's CE self-test)

Order is DRIVERS-METHOD §4: the CE leg (non-destructive, at the takeover) first; `tests kgr` (destructive: it enables PGRAPH's
PMC bit and schedules a channel onto GR's runlist with no FECS ucode) alone and last, refused unless the CE leg's `bind_post`
read 00 on this boot (`refused(ce-bind=<x>)`) — a CE bind that failed means the shared table is unproven and GR learns nothing.

1. `bind=` (0x252c after OUR bind, after the W1C of PFIFO_INTR bit 0): `ok` -> G0 + G1 confirmed. `SNOOP_WITHOUT_BAR1` -> G1
   refuted -> G1b. `INVALID_RUNLIST` -> PTOP's GR runlist is wrong. `INVALID_CTX_TGT` -> RAMFC/instance target (G4).
2. `commit_us=<n>` vs `stuck` (runlist 0 pending bit 0x2284) -> `r2630` pre (G5).
3. `ramfc_get=` (USERD GP_GET, the BAR1-polled slot) and `nop=`: `ok` -> G6 confirmed: the GR channel runs host methods; the
   GR wall is purely the engine context (K-GPU-4). `stuck` with `ramfc_get=1` -> fetched, semaphore not released: the `[kfifo]
   decode` line's `pb0_intr0` names it (SEMAPHORE/ACQUIRE/METHOD) or `eng0=[... chsw=1 load=1]` = ctxsw to GR waits on FECS.
   `stuck` with `ramfc_get=0` -> the scheduler never loaded chid 1: `eng0` and `sched_dis` decide.
4. Always: `[kfifo] decode chid=1 ...` (MMU fault unit 0x00 GR shown), then `[kfifo] unwind chid=1 ...`.

## 4. Walls known and unapplied

- PFIFO_INTR_EN 0x2140 (gk104.c:752): not written — no PFIFO interrupt handler; status words are read, not taken (as GPUBLIT3).
- GR context (FECS/GPCCS ucode, golden context): not this arc — the falcon fold (`family=ucode ... POISON`) is K-GPU-4's.
- The boot-time September leg (`kepler.rs` fifo leg, `nvidia-kepler-fifo` / `UNAOS_KEPLER_FIFO`) still writes its own RAMFC
  with its own USERD page; it is NOT in the seat's x86 metal shape. If armed it runs AFTER `kepler_gpublit::arm`, i.e. with
  0x2254 set and its USERD outside the table. Seat question: retire it, or route it through `host` (owed, not done here).

## 5. Constants table

| constant | value | citations |
|---|---|---|
| USERD BAR1 table | 0x2254 = 0x10000000 or table>>12 | nouveau gk104.c:749; wire `err=00000002` 0x252c code 2 = gk104.c:600 (two of three) |
| USERD slot | table + chid*0x200 | nouveau gk104.c:116 (size 0x200), chan.c:463; GP_GET/PUT +0x88/+0x8c gf100.c:129-130 |
| RAMFC | +08/+0C userd, +10 face, +30 fffff902, +48/+4C gpfifo, +84 20400000, +94 30000fff, +9C 100, +AC 1f, +E4 20, +E8 chid, +B8 f8000000, +F8 10003080, +FC 10000010 | nouveau gk104.c:88-102, devm/priv :110-111 [ONE-SOURCE: nouveau; layout gv100 dev_ram.ref:448-468] |
| bind / start / stop / unbind | hi mask 0xf0000 = rl<<16, lo 0x80000000 or inst>>12; ENABLE_SET 0x400; ENABLE_CLR 0x800; lo = 0 | nouveau gk104.c:77/68, :52, :44, :60 |
| runlist commit | 0x2270 = target<<28 or addr>>12, 0x2274 = rl<<20 or count; pending 0x2284+rl*8 bit 20; entry (chid, 0) | nouveau gk104.c:446-447, :426, :454-455 |
| preempt | 0x2634 = chid; pending bit 20 | nouveau gf100.c:43, :372 [ONE-SOURCE] |
| SCHED_DISABLE | 0x2630 BIT(rl) | nouveau gk104.c:412/418 [ONE-SOURCE] |
| 0x2a04 / PBDMA 0x04013c | OR 0xbfffffff / clear 0x10000100 | nouveau gk104.c:740, gf100.c:357 [ONE-SOURCE] |
| GR in PTOP | engine type 0; runlist/engine/reset from the ENUM word | nouveau top/gk104.c:45-81 (type 0 at :78) |
| host NOP / SEMAPHOREA..D | 0x0008 / 0x0010..0x001C, release op 2 | open-gpu-doc cla06f.h:75-97 (as GPUBLIT2's M_HOST_SEM) |
| GR MMU fault unit | 0x00 | nouveau gk104.c:474 |
| sysmem PTE dw1 | 5 (shared `host::PTE_HI_SYSMEM`; the GR window is VRAM, dw1 0) | nouveau vmmgf100.c:263,317-318,328 [ONE-SOURCE] |

## 6. Unwind (EXECUTED on every GR verdict that is not ok, after the dump, before anything else runs)

`host::Pre` records a register only by READING it (`capture`), and only if it is on the `RESTORABLE` list (PMC_ENABLE 0x200,
0x2a04, 0x2630, 0x2254, PBDMA i 0x04013c): `restore` writes exactly the recorded entries, newest first, and reads each back —
a register whose pre-image was not captured cannot be written by the unwind. Channel state is not a plain register (0x800004 has
W1 trigger bits; 0x2274 is a submit doorbell), so it is unwound by nouveau's sequence instead: preempt (0x2634, bounded 2 ms),
stop (ENABLE_CLR), unbind (lo = 0), runlist 0 committed EMPTY (count 0). 0x2100 is W1C (nothing to restore). The CE leg's
pre-images are untouched by GR's unwind (GR's capture of 0x2254/0x2a04 is the CE's value, so restoring it is a no-op on them).
`ok` keeps the channel bound (nothing to undo: it is the working state the next rung builds on). Destructive: yes (PGRAPH PMC bit,
a channel on GR's runlist) — hence `tests kgr` only, once per boot, after the CE leg.

## 7. What the next flight reads (in order)

1. Boot: `[kgr] armed chid=1 rl=<n> eng=<n> reset=<n> win=<x> -> tests kgr` (the one arming line, R80/R87) and the CE leg's
   `[gpublit] walls ... bind_post=` + `[kfifo] decode chid=2 ...` on a non-ok self-test.
2. Operator: `tests kgr` -> `[kgr] walls ...` (every pre->post), `[kfifo] decode chid=1 ...`, `[kfifo] unwind chid=1 ...`,
   then the witness `:: KGR: bind=<ok|reason> ramfc_get=<n> nop=<ok|stuck> -> <verdict> ::` (verdict `PASS` only for bind=ok
   AND nop=ok; else the stage that stopped: `BIND-WALL` / `COMMIT-WALL` / `FETCH-WALL` / `EXEC-WALL` / `REFUSED`).
3. The tree of §3 moves G0..G7; the ledger above is updated from the quoted lines only.

## Milestones

- M0 — GPUBLIT3 merged (the source).
- M1 — `kepler_fifo::host`: fifo_init, ramfc_write, inst_pd, bind, runlist_commit, start, pde/pte, the capture/restore type,
  unwind; GPUBLIT's private copies replaced by calls (same writes, same order, same walls line).
- M2 — `pfifo_decode()` lifted; both legs print `[kfifo] decode chid=<n> ...` (the `[gpublit] decode` line is gone).
- M3 — `kepler_fifo::kgr`: the window reservation + arming line at takeover, `tests kgr`, the witness, the executed unwind.
- M4 — `unaos/scripts/kepler-capture.list` (the bench's working-state read list).
