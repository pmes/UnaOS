# GEN7R8 — the R8 window from the panel, the blit behind `tests gen7`, R2 re-scored (ledger B320, extends B111)

`CHARTER: Kernel — driver` (the Intel HD 4000 gen7 ladder, `unaos/crates/kernel/src/drivers/gpu/gen7.rs`
`mod r8`; no new file, no handler domain touched).

## Finding — the brief's premise is stale, and the record is what is wrong

B111 (flights 8 and 9, 2026-09-16) is still `open — … REFUSED`, the shut-out register's R8 row still says
"built 2026-09-15, and only a boot can move it", and the queue's GEN7R8 rows still read REFUSED. **None of
that is the state of the silicon.** R8CAP (`17936071`, same day) derived the ceiling from the window
reservation (`R8_MAX_STRIDE_PX = 4096` → 265 slots), and every boot since that carried `UNAOS_IVB3D_R8`
ran the blit and passed. Read with `awk 'index($0,":: gen7: r8 verdict=")'` over the evidence tree:

| flight (evidence file) | r8 verdict lines | `r8-fb-blit-verified` |
| :--- | ---: | ---: |
| 10 (`rmbp-0917/flight10/FLIGHT10.md`) | 1 | 1 |
| 13 (`rmbp-0915/flight13/f13-boot1.log`) | 1 | 1 |
| 14 (`rmbp-0915/flight14/f14-boot1.log`) | 1 | 1 |
| 15 (`rmbp-0915/flight15/f15-boots.log`) | 4 | 4 |
| 16 (`rmbp-0929/flight16/f16-boot.log`) | 1 | 1 |
| 17 (`rmbp-0915/flight17/f17-boot1.log`) | 1 | 1 |
| 19 (`rmbp-0915/flight19/f19-boots.log`) | 4 | 4 |

Thirteen of thirteen. Flight 19's exec column, verbatim:
`r8 cand=mt class=BDW-ONLY col=exec ctl_readback=00000001 head_at_arm=00000000 head_post=00000040
head_moved=1 tail=00000040 sentinel_post=0B8C0DE8 sentinel_hit=1 rect_match=4096/4096 spill=0
dst_crc=E7E603AE src_crc=E7E603AE armed=1 iters=15 cyc=25668` — the BCS copied a 64x64x32bpp rectangle
from system memory to the top-right corner of a 265-page GGTT surface at the panel's 16384-byte pitch,
the ring head advanced to the tail, the MI_FLUSH_DW-ordered store retired, and the checksums agree.
**The iGPU's first blit already happened** (2026-09-16 21:32Z, flight 10). The brief's 11520 B/row is
the panel's pixel bytes; the scanout pitch is 16384 (firmware pads 2880 to a 4096-px stride).

What is real in the brief, and what this arc does:

1. The window is still a compile-time reservation (`R8_MAX_STRIDE_PX`), sized as a stack table; a wider
   panel refuses on a constant, and the refusal line names slots, not `need=`/`have=` bytes.
2. R8 runs **at boot** (the tail of R7's `blit`) — R80 says it is a test, and it goes behind `tests`.
3. Its result is spread over thirty lines; there is no one-line witness, no ring head/tail summary line,
   and no wall time.
4. R2 still prints `r2-unscorable-until-behavioural-witness` on every boot although R8's PASS is that
   witness.

## Seam

Kernel — driver. One file of code (`gen7.rs`, `mod r8` at its tail), one registration line in
`tests.rs` (same-line fold before the comment + a tail function). No knob: `gen7r8` (`UNAOS_IVB3D_R8`)
already gates the rung; the `tests gen7` fixture exists only in that build.

## Milestones

- **M1 — the window from the panel.** Delete `R8_MAX_STRIDE_PX` / `R8_DST_MAX_PAGES` / `R8_WIN_SLOTS_MAX`
  / `R8_DST_MAX_BYTES`. The PTE table becomes a heap `Vec<u32>` of exactly `win_slots`
  (`try_reserve_exact`, refused as `r8-alloc-failed which=ptes` on failure), so the destination window
  is `rows × pitch` rounded up to the 4 KiB GGTT page at runtime. The only capacity refusal left is the
  GGTT itself: `have` = the slots from `R8_BASE_SLOT` to the end of the GGTT as BAR0 maps it, less the
  ring page, the source pages and the trailing neighbour, × 4096. Refusal:
  `r8-refused-surface-too-large need=<bytes> have=<bytes> rows= pitch= …`. `fb_blit`'s frame shrinks
  by ~1 KiB.
- **M2 — the blit behind `tests gen7`.** R7's tail no longer runs R8; it stashes `(bar0, bar0_size,
  bdf, wake, r7 verdict, panel geometry)` and prints nothing (R80). `tests gen7` runs the unchanged rung
  from the stash, adds `[gen7] r8 ring head=<h> tail=<t> advanced=<0|1>` after the drain, times the
  blit (submit → drained) in µs from the TSC, and prints the witness. Once per boot: R8's pages are
  `reclaim=held` (GEN7TLB HOLD), so a second `tests gen7` replays the cached witness (`replay=1`) rather
  than hold another 1 MiB.
- **M3 — R2's behavioural witness.** The witness carries `r2=behavioural-ok` when R8 passed (the engine
  moved bytes, through a held wake, on this boot), else `r2=still-dark`. The boot-time R2 line is left
  verbatim (a boot line `awk`s compare against); the re-score lives in the test line, and gen7.md §2.2
  says which line is authoritative.
- **M4 — the gmux stays untouched.** Documented in gen7.md: how a future rung would hand the panel to
  the IGD for one frame and back (G7 of the register), and why not now.

## Witness

```
[gen7] r8 ring head=00000040 tail=00000040 advanced=1
:: GEN7R8: window=1064960 ring_advanced=1 csum=match us=<n> r2=behavioural-ok verdict=r8-fb-blit-verified -> PASS ::
```

Under `tests gen7` only. REFUSED when no ring was armed (no stash, R7 not verified this boot, a geometry
or GGTT refusal, an unconfirmed wake, an owned window); FAIL when the ring was armed and anything short
of `r8-fb-blit-verified` with matching checksums came back.

## What stays owed

- R1–R7 still run at boot under `UNAOS_IVB3D` (a knob, so R80-admissible, but they are a census). Moving
  the whole ladder behind `tests gen7` is a separate arc: R1's census must read firmware's GGTT before
  `bring_up_blt_ring`, so the order has to be argued, not just moved.
- R8 at test time runs minutes after R7 instead of microseconds; if the GT sleeps in between, the wire
  says `r8-enable-void-under-every-hold` and that is itself a finding (the rung takes its own holds).
- The visible blit (gmux G7), `bring_up_blt_ring` wired to the held wake — unchanged, owed.
