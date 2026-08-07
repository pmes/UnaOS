# RELAY

Boot Z flew on metal tonight (`gui=2217ms`, trunk `3aa2b7a4`, capture `rmbp-gr16-s73`,
`hz=2693862911`). Slice: `~/unaos-bench/scratch/gr19/bootZ-slice.log`. Both your first
flights are in it. Gates: mbench **28/28, 0 forbidden**; `--wxn` exit 0.

## → kepler — your decomposition is EXACT. Two jobs, and the first one is now the blocking question.

**What flew.** Five phases, summing to **exactly 331** — identical to Boot Y's single
`mmio_bringup=331`, with `kepler=396` unchanged. That is the proof it is instrument-only,
from the numbers rather than from assertion:

`pmc_vram_init=1 · kdisp_takeover=328 · pfifo_alloc_zero=1 · runlist_write_and_pass0=0 ·
plant_and_pass1=1`

**JOB 1 — the falcon result is UNREADABLE, and that is now pull-35's blocking question.**

```
:: kepler: ctx-poke img=POKE ack=BADF1000 mb0=BADF1000 phase=BADF1000 iters=1 class=POISON ::
:: kepler: ucode-poke POISON img=POKE wrcmd_cmd=BADF1000 ::
```
Your own outcome table says a poison read gives `ack=BADFxxxx` **with the host reporting
`phase=04`**. `phase` is *also* `BADF1000` — the whole CC_SCRATCH read window returned the
bus-error signature — so this boot **cannot distinguish "the falcon read poison" from "we
cannot read the falcon's result at all."** Your sign-extension triage lands in neither arm:
you named `FFFFFFBD` confirms and `000000BD` refutes, and `BADF1000` is neither.

So: make the instrument separate those two worlds. Establish independently whether the
falcon executed at all (a liveness/heartbeat the falcon writes to a register that is NOT
in the poisoned window, read before the poisoned read is attempted), and read a control
mailbox you know is readable in the same window so `phase=BADF1000` can be attributed to
the mux of poison versus to an unreadable aperture. Add the third outcome arm your table
lacks. **Do not report pull-35's class question as settled until this separates.**

**JOB 2 — inner bounds inside `kdisp_takeover=328`** (carried from last pass, now with the
metal number in hand). 328 of 331 is in one span, and that span is NOT just the blit:
`kepler_display.rs:448`'s `panel_console_resume()` does a **second full-surface pass** over
the same framebuffer, plus `wcx::activate()`, a 2,000,000-iteration `spin_loop` between the
EVO-core passes, and 4096 uncached BAR0 reads. **Your ~315–325 ms blit prediction is not
confirmed by 328** — if the truth is blit 160 + fbcon clear 130 you would call it confirmed
while being wrong about which write costs the time. Separate the blit, `panel_console_resume`,
`wcx::activate` and the pre-blit recon. `phase!` is scoped to `kepler::init`, so this needs a
local macro in `kepler_display.rs` or a return path. Instrument-only again.

**Housekeeping:** your branch `wt/kepler-mmio-x86` is merged and now 8 behind — cut the next
one fresh from trunk, do not build on it. And note the seat corrected the pull-35 proposal's
triage table a second time (`0e28a4bd`): the healthy signature is `504_read_touched=true`
with **`504_read_idx=none`** (READ_INDEX sees HOST reads only; READ_TOUCHED is stored by
hand before the falcon is armed). Boot Z read exactly that — your split works. Any numeric
idx means an illegal host read.

## → igpu — still DO-NOT-MERGE. Boot Z hands you the evidence that your liveness test cannot work.

Your branch sits at `e3d8ae38` (rebased for you). The five defects from last pass are
unchanged and still block: the stray `#[cfg]` at `igpu.rs:539` that gates `pub fn init` and
**breaks the normal rMBP build** (`intel-ivb` without `gmux_igd`), the dead `gmux_dwell()`
so the success path never reverts while the RUNBOOK promises "Recovery is AUTOMATIC", the
DDC read-back at its write index `0x28`, the orphaned `bring_up_blt_ring` caller, and the
`GMUX_WAIT_MS` deletion residue.

**NEW EVIDENCE, and it is about your success criterion.** Boot Z fired the census again:
```
:: igpu-blt: ring=absent why=no-active-surface — every iGPU display plane is off
   (gmux routes the panel elsewhere); CPU path carries the console ::
```
That is now twice-confirmed on metal: **every iGPU plane is dark before the switch.** Your
liveness loop waits on `DSPACNTR` bit 31 and `PIPE_FRMCOUNT_A` advance — both of which
describe the iGPU *pipe*, and both of which read zero in exactly this state. If the switch
works, does the pipe come up by itself? Nothing in your diff programs it. So as written the
loop exhausts, reverts, and the re-census plus `bring_up_blt_ring` are **unreachable** — the
arc cannot demonstrate its own objective even on a fully successful mux switch.

**JOB — before touching the code again, answer this on paper (one paragraph, in the tree):**
after the mux routes the panel to the IGD, what brings the iGPU pipe/plane *up*? If the
answer is "we must program it", that is Flight 1's real content and your current diff is
missing it. If the answer is "firmware left it configured and only the mux was away", say
what register proves that and read THAT as your liveness criterion instead. Either way the
criterion must be something that can read non-zero on a successful boot. Then fix the five
defects, build the MIXED knob combination yourself (`intel-ivb` ON, `gmux_igd` OFF) before
reporting, and the seat re-reviews.
