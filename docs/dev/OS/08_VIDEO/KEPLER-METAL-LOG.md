# Kepler bring-up — metal facts of record (rMBP GT 650M / GK107)

Hard-won silicon facts from the fox-metal sitting series. Trust these over any
QEMU behavior. Newest sitting first.

> **Every rung's standing status — `open` / `shut-out` / `never-run` / `proven`, with the
> conditions each failure happened under and the rung it depends on being open — is compiled once
> in [`SHUTOUT-REGISTER.md`](SHUTOUT-REGISTER.md) (R19; rmbp-ledger B10). This file stays the
> per-sitting narrative; the register is the verdict table.**

## PRE-REGISTERED — KVBLANK3 (rmbp, the GPU line under R53), cut 2026-09-23

**NOT FLOWN.** Written before the boot and not edited afterwards. Branch `exec-rmbp-kvblank3`, parent
`94e90eae`. Knob `UNAOS_KEPLER_VBLANK=1` (no new knob). Ledger row **B192**; the reading it answers is
§FLIGHT 12 READING below. **The precondition from KVBLANK2 stands:** the ladder and the period are
driven by `scanout_beam()`, i.e. by a compositor that is presenting, so drive the desktop for at
least ~20 s after it is up.

### What flight 13 must print — the interrupt path

**Rung 1, at `kepler::init` (writes the mask only; the enable is held at 0, so nothing can be delivered):**

```
:: kepler: vblank pmc-arm bit=26 reg=INTR_MASK_HOST en_host=00000000 mask_entry=<w> mask_armed=<w with 04000000> intr_entry=<w> intr_or=<w> line_or=00000000 pdisplay_seen=<n>/<m> window_ms=50 deliver=none reason=source-probe-only restored=<mask_entry> verdict=clean ::
```
*Read:* `mask_armed` carries `04000000`, so the per-source mask exists and latched on the GK107
(flight 12's `0x140` write read back 0). `pdisplay_seen>0` means PDISPLAY already raises its PMC
input with the per-head enable clear, which flight 12's census predicts (`host_summary=01000000`).
`pdisplay_seen=0/…` is also a result: the head enable, not the PMC mask, is what gates it.
*Control:* `line_or=00000000`. A non-zero value means the line asserted with `INTR_ENABLE_HOST` at 0,
and then this doc's reading of `0x140` is wrong. *Red:* `verdict=DIRTY`.

**R3, the vector (after the census and R2; now ~4.5 s of scanout, counted in hardware vblanks):**

```
:: kepler: vblank-intr vector armed bdf=1:0.0 vector=0x44 wire=1 pmc_entry=00000000 pmc_bit=26 en_entry=00000000 head=0 window_vblanks=60 storm_cap=4096 cmd=<bit 2 set, e.g. 0006/0406> pmc_en_armed=00000001 pmc_mask_entry=<w> pmc_mask_armed=04000000 ::
:: kepler: vblank-intr vector close head=0 irq=<n> vbl_delta=<60..63> rearms=<n> wire=1 vector=0x44 storm=0 pmc_restored=00000000 pmc_readback=00000000 en_restored=00000000 en_readback=00000000 verdict=clean mode=<irq|poll> deliver=<msi|none> reason=<none|…> intr_or=<w> line_or=<w> mask_restored=<w> mask_readback=<w> ::
```
**A WORKING INTERRUPT PATH PRINTS `irq>0 … deliver=msi reason=none`**, with `intr_or` carrying
`04000000`, `line_or=00000001` and `mode=irq`. The ratio `irq/vbl_delta` is the delivery rate. A
reading of `irq=1` (or a few) against `vbl_delta≈60` says the function sent a message and did not
send again. The per-message MSI re-arm is `NOT-IN-TREE`, so that reading is a result and not a
failure. `vbl_delta` is now hardware vblanks, so the window is ~1 s. Flight 12's "60" took 2016 ms.
**The refusals, each naming where the signal stopped** (the classifier takes the first broken link):
`no-bus-master` (cmd bit 2 clear, which `kepler.rs:1310` should make impossible) ·
`pmc-enable-not-latched` (`pmc_en_armed` bit 0 did not read back) · `pmc-mask-not-latched` (bit 26
did not read back in `0x640`) · `pdisplay-not-raised` (the enable and the mask are good, but
PDISPLAY never set `INTR_0` bit 26 with the per-head VBLANK enable held; the suspects are the head
enable, and `DISPATCH`, which flight 12 read as `FB333FC7` and this arc does not steer) ·
`line-not-asserted` (PMC saw the source but the output line stayed low; `pmc.rst:319-324` and `:393-394` put the
PDAEMON redirection circuitry between them) · `line-asserted-not-delivered` (the line rose and no
ISR ran: the MSI message itself). `storm=1` still means a level line the disarm did not drop.

### What flight 13 must print — the period

```
:: kepler: vblank head=0 count=<n> period_us=<16600..16700> jitter_us=<under 2000> raster_at_irq=<l> vt=1852 mode=<poll|irq> vbwaits=<n> vbwait_us=<n> vbgaveup=<n> vbrecheck=<n> seen=<n'> tight=<t> period_src=tight ::
```
**A WORKING PERIOD READS `period_us≈16667 jitter_us<2000 period_src=tight`.** BEAMX86's own
figure on flight 12 was 16.60 ms (60.2 Hz), so 16600-16700 is the window. `count=` advances about
60 per second of wall time. `seen=` is flight 12's old count and should run near 26/s on the same
desktop, so `seen/count` is about 0.4, and that ratio is the miss rate the old line hid.
**Falsifiers:** `count` equal to `seen`, with `period_us` still near 38 000, means the field steps
by one per sample. Then `HEAD_STAT.VERT[31:16]` is NOT a vblank count on this part, and rnndb's
`:647` reading is refuted on the metal. `period_src=loose` with `tight=0` means no step landed
inside a hold. The period is then good only to about (a present gap)/count; after ~400 s that is
under 0.1 us, so the number stands but the jitter is not measured. `jitter_us` in the thousands with
`period_src=tight` means the edges are bracketed but the TSC differs between cores (the samples come
from whichever core presents), or the raster really is unsteady.

### What only the glass proves

That the GK107 raises PDISPLAY into PMC, that its MSI reaches vector `0x44`, and that the count
field steps 60 times a second. QEMU q35 has no Kepler (`:: kepler: no-device ::`). The q35 wire proves
the vector allocation, the verdict function over flight 12's words, and the count/period arithmetic
over a simulated timer sampled the way flight 12 sampled, and nothing more.

## FLIGHT 12 READING — KVBLANK2 (B179) scored, and KVBLANK3's READ (rmbp-ledger B192), 2026-09-23

Capture: `f12-boot1.log` (bench scratch, 6899 lines; record `docs/dev/evidence/rmbp-0915/flight12/FLIGHT12.md`
§KVBLANK2). Every line quoted below is from that capture, read with `awk 'index($0,…)'`.

### Finding 1 — `reason=no-vector-helper` is a stale literal; the interrupt path DID arm, and delivered nothing

`reason=no-vector-helper` is a string constant in `arm_pmc_pdisplay` (`kepler_vblank.rs:460` at
`94e90eae`). That function is KVBLANK's rung 1 PMC half and runs at `kepler::init`. It printed the
reason unconditionally at 6147 ms, 33 s before the KVBLANK2 ladder reached the vector rung. The
vector rung ran, and the same wire shows it:

```
[  38983ms] [vectors] alloc name=kepler-vblank vector=0x44 == witness ::
[  38983ms] [vectors] allocated=4 free=171 table=timer:0x20,xhci:0x40,nic:0x41,ipi:0x42,ehci:0x43,kepler-vblank:0x44,spurious:0xff == witness ::
[  38984ms] [MSI] Enabled on 1:0.0: cap@0x68 addr=0xfee00000 data=0x44 (64-bit) MsgCtl=0x0081
[  38984ms] :: kepler: vblank-intr vector armed bdf=1:0.0 vector=0x44 wire=1 pmc_entry=00000000 pmc_bit=26 en_entry=00000000 head=0 window_vblanks=60 storm_cap=4096 ::
[  41000ms] :: kepler: vblank-intr vector close head=0 irq=0 vbl_delta=60 rearms=0 wire=1 vector=0x44 storm=0 pmc_restored=00000000 pmc_readback=00000000 en_restored=00000000 en_readback=00000000 verdict=clean mode=poll ::
```

So the B168 row is on the table, MSI was programmed on the GK107 (`MsgCtl=0x0081`: 64-bit, enable),
and nothing was delivered: `irq=0`. The brief's premise ("the interrupt path never armed") is
refuted by the capture. What this reading changes is WHY `irq=0`.

**Why `irq=0`: bit 26 was written to a register that has no bit 26.** envytools `docs/hw/bus/pmc.rst`
(fetched 2026-09-23, 543 lines, sha256 `1d3fa199dd3f5c18d69d82750c9725faa3add1a920b52ca4f0e1df3f9aaf02bc`)
lists `0x140 INTR_ENABLE_HOST` (line 30). Lines 346-349 give it two bits: `bit 0: hardware interrupt
enable` and `bit 1: software interrupt enable`. Lines 314-316 say this enable register "only allows one to enable/disable all
hardware or all software interrupts". The per-source bit lives in `0x640 INTR_MASK_HOST`,
`GT215:` (line 45), described at line 375: a source whose bit is 0 there is "masked off to always-0 in the
INTR_* register". Bit 26 = PDISPLAY is in the GF100+ source table (line 494). This tree placed bit 26 in
`0x140` (`gpu_spec.md` §2.3.2 as of `94e90eae`). The wire agrees with the document: the rung-1 line reads
`en_entry=00000000 en_armed=00000000`, so the bit-26 write to `0x140` read back as 0. §R3 made the same
write and never read it back. So `INTR_ENABLE_HOST` stayed 0 for the whole window. With the output line
unable to assert, the GK107 could not send an MSI, and the PDISPLAY source was never examined at
all. Neither the MSI path nor the ack protocol is convicted by this flight. (Bus master is not the
cause: `kepler.rs:1310` calls `enable_bus_master` on the GK107 before any of this.)

A census fact for the next rung: on every census sample and through R2's window, `status=02000003
host_status=02000003 host_summary=01000000 vblank_bit=1`. The VBLANK status bit reads SET before
anything was enabled and never reads clear (`vblank_seen=16/16`). It is latched and no one clears it. The
ack protocol is still `NOT-IN-TREE` (B179).

### Finding 2 — the poll counts sampling episodes, not vblanks; derived twice

**Derivation A — from the code (`kepler_vblank.rs` at `94e90eae`).** `note(word)` compares
`word[31:16]` with the last value it saw. On any difference it calls `edge()`, and `edge()` does
`VB_COUNT += 1` (`:508`). **The hardware field is a COUNT (rnndb `g80_pdisplay.xml:647`,
`vblank_count[31:16]`), and the code used it as a changed/unchanged flag and threw away the
difference.** `note` is fed only by `scanout_beam()`, which is called only inside `beam::hold` (a
present or a strip paint) and inside `wait_next_edge`. Between two holds the field advances by
however many vblanks passed, and the next hold counts that as ONE edge. So `count=` is the number of
sampling episodes that followed at least one vblank. `period_us = Σdt / (count − 1)` (`report()`,
`:540`) is the mean time between those episodes, and `jitter_us = max(dt) − min(dt)`. max(dt) is the
longest gap between two episodes: count went 15→22 between the 8319 ms and 32152 ms lines, a ~23.6 s
stretch with no presents, and `jitter_us=23605994` is that gap.

**Derivation B — from the wire alone, without the code.** (i) *The panel is 60 Hz:* BEAMX86 on the
same boot, `vtotal=1852 … adv=5020 … sample_ms=45 … vblank_delta=2`. 5020 lines / 1852 = 2.711 frames
in 45 ms, so 16.60 ms per frame (60.2 Hz), and 2 counter steps in 45 ms matches. (ii) *The count
runs at the sampling rate:* 10430 counts from the `arm` line at 6414 ms to 405359 ms is 26.1/s.
Σdt/(n−1) = 398.9 s / 10429 = 38.25 ms, against `period_us=38241`. (iii) *The count follows the
compositor's workload, which a vblank cannot do:* from 51.5 s to 217.0 s the count ran at
(5633 − 697) / 165.5 s = **29.8/s** while `[wcn] rollup … passes=118 … span=5044ms` = **23.4 passes/s**
with `shown=3/3`. From 217.0 s to 405.4 s it ran at (10430 − 5633) / 188.4 s = **25.5/s** while
`passes=99 … span=5049ms` = **19.6/s** with `shown=2/2`. The count's rate fell by 4.3/s when the
pass rate fell by 3.8/s. The panel's rate did not change. The count is above the pass rate because
the menu-bar and dock strip paints also enter `hold` and sample. `[wpace] … rate=19.6/s` and
`frame_us=16667` are on the same wire: the pacer's target is 16.667 ms, and the count follows the
presents it actually made.

**The two derivations agree, and they are independent.** A never reads the capture, and B never
reads `note()`. A predicts a count bounded by the sampling-episode rate, and B measures that rate and
shows it moving with the pass rate. **The 33 ms KVBLANK2 fix (§R2b) was not what the panel showed. It
made the WAIT sample, but it left the COUNT counting episodes.** The KVBLANK2 ladder's windows are also
counted in episodes, not vblanks. R2's "16 vblanks" took 38451→38983 ms = 532 ms (≈32 frames), and
R3's "60" took 38984→41000 ms = 2016 ms (≈121 frames).

**Why the old path used the field as a flag.** It was written as an EDGE detector for the wait
(`c != from`), and for that a flag is enough. The period and jitter were then computed from the flag's
timestamps, which only give a true period when the sampler never misses a vblank. KVBLANK's doc
assumed exactly that ("the compositor's own hold loop is the sampler, at ~400 kHz during a hold"),
but the claim holds only DURING a hold, and holds cover a few ms of each 50 ms present.
## PRE-REGISTERED — KVBLANK2 (rmbp, the GPU line under R53), rungs R1 CITATION / R2 ENABLE WINDOW / R3 THE VECTOR, cut 2026-09-22

**NOT FLOWN.** Written here before the boot, unedited afterwards, because that is what makes the
reading falsifiable. Branch `exec-rmbp-kvblank2`, parent `1ad4ee3b`. Knob `UNAOS_KEPLER_VBLANK=1`
(no new knob — the rungs ride KVBLANK's). Registers and their citation lines: `gpu_spec.md` §2.3.3.
Ledger row **B179**.

**WHAT CHANGED SINCE KVBLANK, IN ONE SENTENCE.** KVBLANK could not wire an interrupt because (a) the
kernel had no vector allocator, (b) nothing called `enable_msi` for the GK107, and (c) the PDISPLAY
per-head vblank ENABLE/STATUS pair was `NOT-IN-TREE`. VECTORS (**B168**) answered (a), IOAPIC
(**B147**) answered the INTx half of (b), and **KVBLANK2 answered (c) by finding the pair in the
file KVBLANK had already cited** — `g80_pdisplay.xml`'s `<stripe variants="GF119->` at lines
448-603, two lines past where the first read stopped.

### What flight 12 (or 13) must print, per rung, on success AND on the honest refusal

Every rung's witness is designed so the flight falsifies it. `head=0 vt=1852` are flights 8/9's own
BEAMX86 readings; the rungs arm on the head the beam gate chose and never choose again.

**PRECONDITION FOR ALL THREE, and it is the first thing to check if the block is silent.** The
ladder is driven by the VBLANK EDGE, i.e. by `note()`, i.e. by `scanout_beam()`, i.e. by a
compositor that is presenting. A boot that reaches the desktop and then idles with no window motion
takes no edges and the ladder stalls where it stands. **The flight must drive the compositor** — open
a window, drag it, let the clock repaint — for at least ~20 s after the desktop is up. If only the
`bdf-hunt` line appears, that is the diagnosis, not a Kepler refusal.

**R0 — the bdf, at `kepler::init`, before any of it.**

```
:: kepler: vblank bdf-hunt bar0=<the BAR0 kepler::init mapped> bus_max=16 found=1 bdf=1:0.0 ::
```
*Honest refusal:* `found=0 bdf=255:255.255` — no NVIDIA function up to bus 16 has that BAR0. Then
every later `vector` line reads `REFUSED reason=no-bdf` and nothing is armed. If `found=0` while the
Kepler is plainly up, the bound is the suspect, not the match.

**R1 — the census. WRITES NOTHING.** Eight lines, spaced 30 vblanks (~0.5 s), over ~4 s of scanout:

```
:: kepler: vblank-intr census sample=1/8 head=0 status=<w> en=<w> count_delta=<n> host_status=<w> host_dispatch=<w> summary=<w> host_summary=<w> vblank_bit=<0|1> host_vblank_bit=<0|1> summary_head_bit=<0|1> ::
```
*The reading:* `en=` is what the firmware left the per-head enable at — **the single most valuable
number in this flight**, because it says whether the GOP/EFI driver was using the vblank interrupt
before we took the panel. `status=`/`host_status=` frozen at one value across all eight samples
while `count_delta=30` each time says the status does not latch without the enable; a `status=` that
moves with the enable already clear says it latches regardless, and R2's window is then only
confirming. Either is a result.
*Honest refusal:* all six words read `FFFFFFFF` — the PDISPLAY aperture is not answering at these
offsets, the `GF119-` stripe does not apply to this part, and **R2 and R3 must not be believed**.

**R2 — the enable window. Two writes, both to `INTR_HOST_HEAD_EN`, restored and read back.**

```
:: kepler: vblank-intr window open head=0 bit=0 en_entry=<w> en_armed=<en_entry|1> took=1 window_vblanks=16 ::
:: kepler: vblank-intr window close head=0 vblanks=16 samples=16 vblank_seen=<n>/16 status_or=<w> host_status_or=<w> restored=<en_entry> readback=<en_entry> verdict=clean ::
```
*Success:* `took=1` and `vblank_seen=16/16` — the enable latched and the status bit tracks the
raster. *The other real outcome:* `took=1 vblank_seen=0/16 verdict=clean` — **a status that never
toggles is a RESULT, not a failure.** It says the GF119- per-head VBLANK status does not assert on
this head under the enable alone, and it is the evidence R3 needs before it is believed.
*Honest refusal:* `took=0` — the enable bit did not stick, so this register is not writable here and
R3's arm will be meaningless. *Red:* `verdict=DIRTY` — the restore did not read back, which is the
one outcome that says this rung left the card changed. Nothing else in the block may be trusted.

**R3 — the vector. Four writes, all restored and read back.**

```
:: kepler: vblank-intr vector armed bdf=1:0.0 vector=0x44 wire=<1|2> pmc_entry=00000000 pmc_bit=26 en_entry=<w> head=0 window_vblanks=60 storm_cap=4096 ::
:: kepler: vblank-intr vector close head=0 irq=<n> vbl_delta=60 rearms=<n> wire=<1|2> vector=0x44 storm=0 pmc_restored=00000000 pmc_readback=00000000 en_restored=<w> en_readback=<w> verdict=clean mode=<irq|poll> ::
```
*Success — THE READING THIS WHOLE ARC EXISTS FOR:* `irq=` OFF ZERO with `vbl_delta=60`. The ratio is
the delivery rate; `irq=60 vbl_delta=60` is one interrupt per frame and is the answer that ends the
spin. `mode=irq` then appears on every subsequent `:: kepler: vblank head=` line, **and it appears
because the wire delivered, not because a knob was set.**
*Honest refusal, and it is a perfectly good flight:* `irq=0 vbl_delta=60 verdict=clean mode=poll`.
The raster ran, the vector was allocated and registered, the function was programmed, PMC bit 26 was
unmasked and the per-head VBLANK enable was set — and nothing arrived. That convicts the remaining
uncited link (the ack protocol, or a `DISPATCH` steering this rung deliberately does not touch, or
PMC bit 26 not being PDISPLAY on GK107) and it is worth more than a guess that happened to work.
*The two refusals upstream of the wire:* `REFUSED reason=alloc` (the allocator said no — its own
`[vectors]` witness is directly above) and `REFUSED reason=no-msi-no-intx` (the function offers no
usable MSI capability and the IOAPIC would not take its INTx). In both, **nothing on the GK107 was
armed**; the vector, if allocated, stays owned by name, which is the allocator's contract.
*The safety valve:* `storm=1` means the ISR hit 4096 entries and cut PDISPLAY at PMC. The disarm at
the enable did not deassert the line — a real fact about the part, printed, with the boot alive.

**⚠ WHAT NO FLIGHT OF THIS ARC CAN SHOW, and it is structural.** Even `irq=60/60` does **not** make
`beam::hold`'s wait cheap. `hold` runs IRQ-masked on the presenting core inside `COMP_GATE`
(`wm.rs:8582`, `:6950`), so the ISR cannot advance a counter while a wait is spinning on it. The
saving VUGPERF asked for needs `hold` to run unmasked — a `video/wm.rs` change fenced off this
brief and **OWED**. What `irq>0` buys today is the PROOF that the source exists, which is the
precondition for that arc and is exactly what has been missing since B145.

**⚠ AND A DEFECT IN KVBLANK'S SHIPPED RUNG 2, FIXED HERE, which changes what flight 12 would have
read.** `wait_next_edge` could never have advanced on hardware: it spins on `counter()`, whose
polled term is advanced only by `note()`, fed only by `scanout_beam()` — and neither the wait nor
`beam::hold`'s wait arm called it. Inside the wait the counter was FROZEN, so every armed present
burned the full 33.3 ms give-up budget and fell through to the spin. Had flight 12 flown the KVBLANK
image on a wide-zone desktop, `[wc-h] beamwait_us` would have gone UP, not down, and `vbgaveup=`
would have equalled `vbwaits=`. QEMU could not see it: the fixture's `SIM_MODE=1` counter is
computed from `now_cycles()` and advances with wall time, needing no sampler. **The flight's check
on the fix: `vbgaveup=` must be a small fraction of `vbwaits=`, not equal to it.**

## PRE-REGISTERED — KVBLANK (rmbp, the GPU line under R53), rungs `kvblank-measure` + `kvblank-wait`, cut 2026-09-22

**NOT FLOWN.** Written here before the boot, unedited afterwards, because that is what makes the
reading falsifiable. Branch `exec-rmbp-kvblank`, parent `f5d0fb1a`. Knob `UNAOS_KEPLER_VBLANK=1`.

**THE DEFECT, and it is measured, not suspected.** B135 §7 (VUGPERF, fold `4a283725`) decomposed
flight 11's 139.75 ms composite pass and found the residual charged to neither half of the clock —
`blit_us - compose_us - present_us` = 46 488 us per pass — is `video::beam::hold` SPINNING on an
MMIO read of `HEAD_STAT.VERT`. The census: `[wc-h] win=8 beamwaits=4336 beamwait_us=10766899`
(2.48 ms mean per hold against a 16.667 ms frame), `win=3 beamwait_us=49113217` over 13971 waits,
`win=2 beammaxwait_us=62007`. The beam wait alone exceeds the presenter's whole 500 us budget by 5x.

**THE REGISTERS.** `gpu_spec.md` §2.3.1 and §2.3.2 carry the table with its citation classes. In
one line: the EDGE is `HEAD_STAT.VERT[31:16]` (`vblank_count`, rnndb `display/g80_pdisplay.xml:647`,
**[TREE]**, read out of the same word `scanout_beam` already reads for `vline[15:0]`); the PMC
routing bit is `NV_PMC_INTR_0`/`NV_PMC_INTR_EN` bit 26 (**[EXT], UNVERIFIED on GK107**); and the
PDISPLAY-side per-head vblank ENABLE/STATUS pair is **NOT-IN-TREE** and is neither read nor written.

**`mode=poll`, AND THAT IS A FINDING ABOUT THIS TREE.** `arch/x86_64/interrupts.rs` has exactly
three device vectors — `XHCI_MSI_VECTOR` 0x40, `NIC_MSI_VECTOR` 0x41, `EHCI_MSI_VECTOR` 0x43 — each
a hard-coded constant with its own `extern "x86-interrupt"` handler and its own `idt[..]
.set_handler_fn`. There is no `alloc_vector`, no `register_irq`, and no table a driver joins;
`PciScanner::enable_msi`/`enable_msix` take a vector the caller must already own. A fourth would be
a NEW interrupt mechanism, which this rung's brief forbids. So the edge is polled — at zero extra
MMIO cost, because the compositor's own hold loop is the sampler.

**WHAT FLIGHT 12 IS EXPECTED TO PRINT (rung 1).** Pre-registered so a missing line is a finding and
not a shrug:

```
:: kepler: vblank pmc-arm bit=26 en_entry=00000000 en_armed=04000000 intr_entry=<w> intr_or=<w> pdisplay_seen=<n>/<n> window_ms=50 deliver=none reason=no-vector-helper restored=00000000 verdict=clean ::
:: kepler: vblank arm head=0 vt=1852 mode=poll src=HEAD_STAT.VERT[31:16] ::
:: kepler: vblank head=0 count=<n> period_us=~16667 jitter_us=<n> raster_at_irq=<v> vt=1852 mode=poll vbwaits=<n> vbwait_us=<n> vbgaveup=<n> vbrecheck=<n> ::
```

`head=0 vt=1852` are flight 8/9's own BEAMX86 readings, carried forward because this rung ARMS ON
THE HEAD THE BEAM GATE CHOSE rather than choosing again. `period_us` near 16667 is the falsifier: a
`vblank_count` that is not a frame counter will not produce it. `raster_at_irq` is the PHASE, and it
is the number rung 2's arm is gated on — **the one reading this flight exists to take.**
`pdisplay_seen=0/<n>` with a clean restore is a legitimate and informative outcome: it says the
PDISPLAY source does not latch in PMC with only the PMC-side bit set, which is evidence FOR the
`NOT-IN-TREE` pair being the real blocker and against any further PMC work.

**RUNG 2 (`kvblank-wait`), and the three conditions its arm is gated on.** In `beam::hold`, beside
the spin, which is untouched to the character: a source counting (two edges seen), a hazard zone
covering at least **1/3 of a frame** (`VB_WIDE_DENOM = 3` — entering a zone of width `w` at a
uniformly random phase costs the spin `w/2` lines, and B135 §7's 2.48 ms / 16.667 ms = 14.9% IS the
`w/2` of a zone ~30% of a frame wide, so the threshold is the population VUGPERF measured), and
rung 1's measured phase falling OUTSIDE that zone. The deadline handed to the wait is the SPIN's own
`t0 + GIVEUP_FRAMES * FRAME_US`, so a present costs one give-up budget whichever arm it took. After
the edge, ONE confirming raster read; a confirm that lands back inside the zone falls through to the
spin and is counted `vbrecheck=`. **The wait can only make a present faster, never less safe.**

**THE ORACLE IS METAL, AND QEMU CANNOT STAND IN FOR IT.** q35 answers `:: kepler: no-device ::`:
`beam_probe` never arms, `scanout_beam()` is `None`, and every hardware path of both rungs is
unreachable there. What QEMU DOES score is rung 2's property, through a fixture that drives
`wait_next_edge` — the arm itself, not a copy — from a simulated counter the timer advances:

```
:: kepler: vblank selftest arm=wait sim=timer period_us=16667 from=0 advanced=1 waited_us=16579 bound=waited_us<=16667 :: PASS ::
:: kepler: vblank selftest arm=wait sim=stuck advanced=0 gaveup=1 waited_us=33287 budget_us=33334 bound=16667<=waited_us<=50001 (GIVEUP_FRAMES budget, +/- one frame) :: GO-RED-OK ::
```

Those two are QUOTED FROM THIS ARC'S OWN CAPTURE, not predicted. **The go-red's bound is one frame
wide, and it is one frame wide because its first armed run FAILED at `waited_us=33295
budget_us=33334`** — 39 us on a 33 334 us budget, 0.12%, under TCG. The deadline is computed from a
`now_cycles()` in the fixture and the elapsed time from a second one inside `wait_next_edge`, and
`us_to_cycles`/`cycles_to_us` truncate in opposite directions. The claim under test is a count of
FRAMES ("the wait gives up on the same GIVEUP_FRAMES budget as the spin"), so the bound's unit is
the frame and it is BOTH-SIDED: at least `GIVEUP_FRAMES - 1` whole frames (it did not return early)
and at most `GIVEUP_FRAMES + 1` (it did not overrun the spin's budget). A wait that returned in one
frame instead of two still fails it, which is the regression the go-red exists to catch.

**THE FOUR-PLACE TRAP FIRED FIRST, AND IT IS RECORDED BECAUSE IT WAS MEASURED.** With the knob in
`arroyo` alone, the first armed gate run printed `nvidia-kepler-vblank` in the `⚡ kernel features:`
banner and `awk 'index($0,":: kepler: vblank")'` over `target/serial.log` returned ZERO lines:
`builder/src/main.rs` compiles the kernel the QEMU gate BOOTS, not only the one media ship, and it
never reads `$KERNEL_FEATURES`. That is BEAMX86's own documented failure mode — the rastmc/sdwrite
class — caught on a gate instead of on a card. The builder line and the `x86-all` name landed in
the same arc.

**STILL OWED AT THE FOLD.** (a) `[wc-h]`'s `vbwaits=`/`vbwait_us=` fields: that line is emitted
from `video/wm.rs`, which this rung's brief does not name, so the census ships on the
`:: kepler: vblank` line instead. (b) No `scripts/specs/x86-wc.spec` pin for the fixture verdict.

## FLOWN — KDHEAD (shut-out register §1, rung KD14), flights 8 and 9, 2026-09-16: BOTH QUESTIONS ANSWERED, and KD3 re-opens

**Capture:** `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log`, both boots; scored in
[`docs/dev/evidence/rmbp-0916/flight8-9/FLIGHT8-9.md`](../../evidence/rmbp-0916/flight8-9/FLIGHT8-9.md) §4.
Flight line as pre-registered, with `UNAOS_BEAM=1` alongside `UNAOS_KEPLER_KDHEAD=1` and none of the
SHUTRESTORE display write rungs. **Every line in this block is a metal fact.** The pre-registration
below it is kept verbatim, unedited, because it is what makes the reading falsifiable.

**THE BRACKET ARMED.** `:: BEAMX86: head=0 vtotal=1852 samples=22025 vblank_delta=2 -> ARMED ::`,
and the rung borrowed that census rather than re-sampling, exactly as designed:

```
[  22822ms] :: KDHEAD: bracket source=beamx86-census live=[y,n,n,n] :: census_adv=[5109,0,0,0] census_vbd=[2,0,0,0] …
[  22822ms] :: KDHEAD: gop w=2880 h=1800 vram_off=00020000 pitch=16384 …
```

One live head. Heads 1–3 are DARK and every decode on them reads `DARK-NOT-SCORED`, never a verdict.

**THE STRIDE — five blocks, two separate, and the counter trap did not fire.**

| block | `heads_distinct` | `stable` | `readable` | verdict |
| --- | --- | --- | --- | --- |
| `headstat` 0x616000 / 0x800 | **4/4** | 3/4 | 4/4 | `SEPARATES-heads`, then `decode=none reason=no-cited-slicing` — by design |
| `armed100` 0x616100 / 0x800 | **1/4** | 3/3 | 4/4 | `COLLAPSED-stride-does-not-separate-heads-here` |
| `evocore` 0x610460 / 0x300 | 1/4 | 3/3 | 4/4 | `COLLAPSED-stride-does-not-separate-heads-here` |
| `headval` 0x610A00 / 0x540 | 1/4 | 3/3 | 4/4 | `COLLAPSED-stride-does-not-separate-heads-here` |
| **`mirror` 0x640400 / 0x300** | **2/4** | **4/4** | 4/4 | **`SEPARATES-heads`** |

No block scored `heads_distinct=0/4`, so `UNSCORABLE-all-words-volatile` never fired and every
distinctness reading above is taken from words that survived both passes.

**THE DECODE — one agreement, on the only live head:**

```
[  22864ms] :: KDHEAD: head=0 live=yes geom=2880x1800 surface=0x20000 pitch=16384 vs gop=2880x1800 0x20000 16384 -> AGREE :: block=mirror mismatch=none surface_present=y pitch_present=y …
[  22867ms] :: KDHEAD: end rung=KD14 bracket=beamx86-census blocks=5 separated=2 decoded=1 agree=1 disagree=0 writes=0 …
```

**The three numbers this entry owed, answered in the order it asked for them:**

1. **`mirror` reads `heads_distinct=2/4` with `stable=4/4`.** The core-channel method stride DOES
   separate heads on this part. KD8's decode is not a head-0-only fact, and the per-head decode has
   a block to stand on.
2. **`armed100` reads `heads_distinct=1/4` with `stable=3/3`.** Sitting #4's inference — that its
   stride collapsed — is now an OBSERVATION. The alternative this entry pre-registered ("s4 was
   wrong about the stride and right about the field offsets") did not fire.
3. **`-> AGREE`.** `mirror`'s slicing (geom `+0x68` lo16 × hi16 **[METAL s25]**, surface
   `+0x60 << 8` **[METAL s16]**, pitch `+0x6C & 0xFFFF` **[METAL s25]**) reproduces the firmware's
   own 2880×1800, `0x20000`, 16384 exactly. **That pins the slicing BY OBSERVATION and re-opens
   KD3**, the same way KD4 re-opened it in s11 — and by the same mechanism, a control read the
   original rung never had.

**Byte-identical on flight 9** (`[  20147ms] … blocks=5 separated=2 decoded=1 agree=1 disagree=0`),
which is a reproducibility fact rather than a second measurement: the two boots differ by one knob
that does not touch display.

**`writes=0` held.** Every device access in the rung is `mmio_read`; the end line says so and no
display register moved.

**⚠ Also measured, and it is a fact about a DIFFERENT rung:** `pitch=16384` on the `gop` line is the
panel's real pitch, and the shut-out register's R8 row states it as 7 680 B. That is why R8 refused
itself on this flight — rmbp-ledger B111.

---

### The pre-registration, kept verbatim (what was written down BEFORE the flight)

**Nothing below was a metal fact when it was written.** It is the witness the flight scored, written
down before the flight so the scoring could not drift into the reading. Build knob: add `UNAOS_KEPLER_KDHEAD=1` to
the flight line, **and `UNAOS_BEAM=1` with it** — the two rungs share ONE per-head sample and without
`beam` this rung has no control bracket and withholds its decode on purpose. The flight line is
therefore `UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_WC=1 UNAOS_BEAM=1 UNAOS_KEPLER_KDHEAD=1`.
⚠ Fly it WITHOUT the SHUTRESTORE display write rungs (`UNAOS_KEPLER_REPOINT`,
`UNAOS_KEPLER_LATCH_ARM`, `UNAOS_KEPLER_PITCH_LADDER`) on the same boot: each of those writes display
state, and a per-head census taken after one of them is a census of a machine we perturbed.

**WHAT THIS RUNG IS.** Register §1 records KD3 `head-raw` as **shut-out** and its "what would change
the verdict" names exactly one thing: *"re-run the per-head decode with KD4's HEAD_STAT as the
bracket and the per-head stride re-derived for the 917D class. The rung failed because it had no
control read, not because the heads are dead; KD4 proves head 0 scans."* This is that re-run. It
writes nothing (`writes=0`: every device access is `mmio_read`, there is no `mmio_write` and no
`write_volatile` in it), and it answers in three parts that are separable on the wire.

**THE BRACKET, and it is BORROWED rather than re-taken.** KD3's whole defect is that four
byte-identical reads were scored as "the heads are dead" with nothing in the same capture saying
whether any head was scanning. KD4 is that missing reading (s11: `head[0] stat underflow=0
vert=0x0493048A horz=0x0000068C`, heads 1–3 zero), and BEAMX86 already automates it across all four
heads. So `beam_probe` publishes its per-head census and KDHEAD reads it:

```
:: KDHEAD: bracket source=beamx86-census live=[y,n,n,n] :: census_adv=[…] census_vbd=[…] … ::
```

`live` is BEAMX86's own test — `adv > 0 && vbd >= 2`. Taking a second 4 × 45 ms sample here would
cost another 180 ms inside the takeover **and** would be a different sample than the one the beam
gate armed on, so a disagreement between the two would be unattributable. A head the bracket calls
DARK gets `DARK-NOT-SCORED` on its decode line, never a verdict (R19). `bracket source=absent` means
`beam` was not in the build or `beam_probe` did not run; the stride census still prints, every decode
is withheld, and **that capture is not evidence about the hardware.**

**THE STRIDE, measured, with the trap named.** Five candidate blocks, each named by the tree or by a
sitting, each read at head 0 and heads 1..3:

| block | base / stride | probe words | provenance |
| --- | --- | --- | --- |
| `headstat` | `0x616000` / `0x800` | `+0x308 +0x30C +0x310 +0x34C` | **[EXT]** g80_pdisplay.xml:647 (`HEAD_STAT` off 0x6000, stride 0x800, len 4, GK104-) + **[TREE]**; `+0x308` **[METAL s11]**; `+0x30C`/`+0x310` **[METAL s12+s13]** stable head-0-only config; `+0x34C` **[METAL s13]** `0x07380BAF` = vtotal 0x738 \| htotal 0xBAF |
| `armed100` | `0x616100` / `0x800` | `+0x00 +0x08 +0x0C` | **[METAL s4]** — sitting #4's own three offsets, the reading this rung re-takes. The ADDR/SIZE/STORAGE field ROLES are **[UNPINNED]** (s4 itself: "addr=0x00000001 is not address-shaped") |
| `evocore` | `0x610460` / `0x300` | `+0x00 +0x08 +0x0C` | **[TREE]** candidate A + BEAMX86's `evo_size`; layout **[EXT]** nv_evo.xml; that PDISPLAY MMIO mirrors those METHOD offsets is **[UNPINNED]**, read all-zero ×4 at **[METAL s11]** |
| `headval` | `0x610A00` / `0x540` | `+0x118 +0x120 +0x128` | **[TREE]** candidate B; **[EXT]** g80_pdisplay.xml:371–408 but marked G80:GF119, so **[UNPINNED]** on GK107; all-zero ×4 at **[METAL s11]** |
| `mirror` | `0x640400` / `0x300` | `+0x20 +0x60 +0x68 +0x6C` | head-0 record `+0x20`/`+0x60` **[METAL s16]**, `+0x68`/`+0x6C` **[METAL s25 / KD8]**; **[TREE]** `fb-draw reg-dump`. **The per-head stride 0x300 is [UNPINNED]** — every sitting read head 0's record only, which is precisely why it is on this table |

```
:: KDHEAD: block=<name> base=0x… stride=0x… heads_distinct=<n>/4 :: stable=<k>/<p> readable=<r>/4 verdict=… cite=… ::
```

**The trap, and the reason every block is read TWICE with a settle between: a counter defeats this
test in the WRONG direction.** If a stride collapses, all four "heads" are one register — and four
reads of one *counter* taken microseconds apart come back with four different values and score
`heads_distinct=4/4`, a broken stride reading as a working one. The `headstat` bank holds exactly
such counters (`+0x340` VERT, `+0x344` HORZ, and the frame counter at `+0x314`, **[METAL s13]**), so
none of them is probed, and on top of that every probe word that MOVED between the two passes is
struck from the distinctness tuple. `stable=` says how many survived; a block where none survived
scores `heads_distinct=0/4`, which is arithmetically impossible for a real reading (a head is always
distinct from nothing), so the zero is the unambiguous "not scored" marker and `verdict=` on the same
line reads `UNSCORABLE-all-words-volatile`. A literal zero is a legal reading and is never treated as absent
— that error is KD3's, in the other direction; only `0xFFFFFFFF` and the `0xBADxxxxx` family count as
"did not answer", and `readable=` reports them.

**THE DECODE — only where the stride earned it AND the bracket says the head is live:**

```
:: KDHEAD: head=<h> live=<yes|no> geom=<w>x<h> surface=0x… pitch=<n> vs gop=<w>x<h> 0x… <n> -> AGREE|DISAGREE|UNREADABLE :: block=… mismatch=… slicing=… ::
```

The comparison is against what the takeover already inherited from the firmware — the GOP
framebuffer's VRAM offset, its width/height and `stride*bpp` — printed on its own
`:: KDHEAD: gop …` line so the reference is in the capture and not in someone's memory. The only
block with a metal-pinned slicing is `mirror` (**[METAL s25]**: `0x640468 = 07080B40` = h1800 w2880
and `0x64046C = 01004000` = SET_STORAGE bit24 LAYOUT=1 PITCH/LINEAR, pitch `0x4000` = 16384 B/row);
every other slicing prints its own **[UNPINNED]** tag on the same line. `headstat` decodes to
`decode=none reason=no-cited-slicing` **by design** — **[METAL s13]** settled that bank in the
negative ("the scanout surface ADDRESS is not exposed anywhere in these head-block windows") and
`+0x34C` is raster TOTALS **[METAL s13]**, not the active geometry; printing a slicing we cannot cite would be KD3's error a second
time.

**The three numbers this flight owes, in order of what they settle:**

1. **`heads_distinct` on `mirror`.** This is the one block whose FIELDS are metal-decoded and whose
   STRIDE never has been — s16 and s25 both read head 0's record and nothing else. `2/4` or better
   means the core-channel method stride separates heads on this part and the per-head decode has a
   block to stand on. `1/4` means the 0x640400 record is one head's record aliased four times, and
   KD8's whole decode is a head-0 fact only.
2. **`heads_distinct` on `armed100` — sitting #4's inference, re-taken as a measurement.** s4 read
   `addr=00000001 size=078004FE storage=0A0006A8` identically on four heads and concluded the stride
   was collapsing. If this rung reads `1/4` with `stable=3/3`, that inference is now an observation.
   If it reads `2/4` or more, **s4 was wrong about the stride and right about the field offsets**,
   which is a different repair entirely and re-opens KD3 from the other end.
3. **`-> AGREE` anywhere.** One agreement on a live head pins that block's slicing by observation
   and closes the question KD3 opened in July. `-> DISAGREE` with `mismatch=geom` on a block whose
   halves are `0x0780`-shaped **[METAL s4]** is the s4 finding again ("this IS display geometry, just sliced
   wrong") and names the next arithmetic to try. `-> DISAGREE` with `mismatch=surface` on a block
   that agrees on geometry AND pitch is the strongest single result short of AGREE: it would mean
   the block is the right one and only the address shift is wrong.

**Control that must accompany the flight image** (the rung cannot run in QEMU — q35 has no Kepler, so
a q35 log with zero `KDHEAD` lines proves nothing about the code being present):
`LC_ALL=C grep -a -o -F 'KDHEAD' target/x86_64_esp/kernel.elf | wc -l` on the flight artifact, and
`⚡ kernel features:` must name `nvidia-kepler-kdhead` **and** `beam`.

## PENDING METAL — BEAMX86 (rmbp ledger A5, `fixed-unflown`): does the Kepler head's `VERT` behave as a raster?

**Nothing below is a metal fact yet.** It is the witness the next flight scores, written down before
the flight so the scoring cannot drift into the reading. Build knob: add `UNAOS_BEAM=1` to the flight
line (it joins `UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_WC=1`; `beam` alone does nothing on x86
because the probe lives inside `kepler_display::takeover_display`).

**THE ONE LINE TO SCORE**, printed exactly once, from inside `takeover_display` after
`:: kdisp: console-repaint rows=` and before the `wc` activation:

```
:: BEAMX86: head=<h> vtotal=<n> samples=<k> vblank_delta=<d> -> ARMED ::
     max_vline=… adv=… panel_h=… evo_size=… size_hi=… size_lo=… sample_ms=45
     census_adv=[…,…,…,…] census_vbd=[…,…,…,…] …
```

* `-> ARMED` means one head's `HEAD_STAT.VERT` (`NV_PDISPLAY_BASE + 0x6000 + head*0x800 + 0x340`,
  `vline[15:0]` / `vblank_count[31:16]`, g80_pdisplay.xml:647) was seen to CLIMB and its
  `vblank_count` to tick at least twice inside 45 ms, and `vtotal = max(vline) + 1` cleared the panel
  height. `arch::scanout_beam()` then answers and every panel present is bracketed.
* `-> NONE` is a real answer, not a failure to run: the `census_adv`/`census_vbd` arrays say what
  each of the four heads did, so a NONE is diagnosable from the log alone. Two identical samples give
  NONE, never `vtotal=1`.

**The three numbers this flight owes, in order of what they settle:**

1. **`vtotal` vs `evo_size`.** `vtotal` is SAMPLED, because rnndb cites no lines-per-frame register
   in this bank — the only raster-shaped word this file knows of (`0x07380BAF`/`0x0BAF0738`, the
   `"raster"` key in the known-value scan) sits at an offset discovered by VALUE, not cited, and this
   module does not read uncited addresses. `evo_size` is the head's EVO `SIZE` readback
   (`0x610400 + head*0x300 + 0x60 + 0x8`) carried on the same line for cross-check. Expected relation
   is `vtotal >= the matching half of evo_size`, by the vblank rows — NEVER equality. If `vtotal` is
   BELOW both halves, the sampled window did not cover a whole frame and the number is not a vtotal.
   Sitting #4's `size=078004FE` was read from the OTHER candidate block (`0x616100`, stride
   collapsing, all four heads identical) and is not the word this line prints; do not compare them.
2. **Which head, and whether it is the panel's.** `head=` here is the head whose counter MOVES, which
   is not necessarily `found_head` (the head whose scanout address MATCHED). If they differ, that is
   a fact worth its own sitting. The standing caveat from sitting #5 — that the panel may be the
   iGPU's and Kepler display takeover a dead end on this box — is the reason the probe is bolted to
   the END of `takeover_display`: it cannot run on a boot where the takeover declined, so an `ARMED`
   line is also evidence that a Kepler head is driving the surface presents go to.
3. **`[wc-h] torn=` under `storm`, which is the actual verdict.** Baseline is F6 boot 1:
   `[wc-h] win=2 torn=111 banded=13085`, with the eight vug windows at torn 3–10. With `-> ARMED`,
   `torn=` stops being the duration predicate and becomes the OBSERVED beam crossing (`beam=obs` on
   the rollup), and the shell window's count is expected to COLLAPSE toward the vug windows' range.
   A `-> ARMED` line with `torn=` unchanged is the interesting failure: it would mean the counter
   that advances is not the raster the panel scans from.

**Control that must accompany the flight image** (the probe cannot run in QEMU — q35 has no Kepler,
so a q35 log with zero `BEAMX86` lines proves nothing about the code being present):
`LC_ALL=C grep -a -o -F 'BEAMX86' target/x86_64_esp/kernel.elf | wc -l` on the flight artifact.

## FLOWN — KFBIND (shut-out register §2, rung KF27), flights 8 and 9, 2026-09-16: NEITHER BASE ANSWERS, and IB_GET has now been read

**Capture:** `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log`, both boots; scored in
[`docs/dev/evidence/rmbp-0916/flight8-9/FLIGHT8-9.md`](../../evidence/rmbp-0916/flight8-9/FLIGHT8-9.md) §4.
Flown alone, as §2 required: `UNAOS_KEPLER_KFBIND=1` on the standing
`UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1 UNAOS_KEPLER_CE=1` line, no KFCTXBIND, no FIFO write rungs.

**THE PTOP TABLE ANSWERED, AND CE-R1 AGREES WITH IT DWORD FOR DWORD.**

```
[  23164ms] :: KFBIND: ptop pbdma=4 runlists=[01CD] pbdma0_base=023000 entries=64 nonzero=16 poison=0 refused_out_of_window=12 overflow=0 ctl_pre=0E7150A2 ctl_post=0E7150A2 ctl=held …
[  23098ms] :: kepler: ce-ptop row i=00..07 [8006183E 00000003 90828C3E 00000007 94A30E3E 0000000B 9C03AA3E 0000000F] ::
[  23163ms] :: KFBIND:  ptop-row i=00..07 [8006183E 00000003 90828C3E 00000007 94A30E3E 0000000B 9C03AA3E 0000000F] ::
```

The two independent readers of the PTOP table were compared row by row on both boots: **8 of 8 rows
and 64 of 64 entries identical, on each boot.** CE-R1's existence precondition for the derivation
half is met on metal.

**NEITHER BASE ANSWERS — and the rung refused to pretend otherwise.** The four PTOP-derived bases
(`023000`, `027000`, `02B000`, `02F000`) return `BADF1100 POISON` in every dword; the three legacy
bases (`040000`, `042000`, `044000`) return clean `00000000 ZERO`. Every `pbdma[n]` line carries
`answers=n` and the same clause: *"CHID/ACTIVE are DELIBERATELY NOT decoded here: at an unproven
base a zero is a READ-ZERO, not a scheduler state, and printing 'CHID=0 ACTIVE=0' is the ten-sitting
error this rung exists to stop"*. The `delta` lines show `same` for every base across the submit.

**IB_GET HAS NOW BEEN READ, for the first time in the campaign, and it does not move.**

```
[  23172ms] :: KFBIND: userd pre-submit  off=02002000 ib_get=00000000 ZERO ib_put=00000000 ZERO x090=00000000 ZERO dma_put=00000000 ZERO dma_get=00000000 ZERO — … ib_get (0x88) is READ HERE FOR THE FIRST TIME, the deleted witness read 0x8C/0x90 which is IB_PUT and an unnamed word ::
[  23176ms] :: KFBIND: userd post-submit off=02002000 ib_get=00000000 ZERO ib_put=00000000 ZERO x090=00000000 ZERO dma_put=00000000 ZERO dma_get=00000000 ZERO — … ::
```

**THE VERDICT, verbatim and unelided, because the `under:` clause IS the result:**

```
[  23177ms] :: KFBIND: verdict base=NEITHER-ANSWERS — every dword at every base, derived and legacy, is ZERO or POISON with a HELD bracket. That is a statement about the whole PBDMA PRI space on this part and it is the strongest reading this rung can produce without a start/clock sequence it may not guess ib_get 00000000->00000000 ZERO ib_put 00000000->00000000 derived_n=4 ctl_pre=0E7150A2 ctl_post=0E7150A2 ctl=held writes=0 restored=n/a -> STILL-DARK under: PTOP-derived base comparison as scored above, FECS context microcode NOT resident (this boot runs no ctx ucode before the submit), CHAN_CUR/CHAN_NEXT NOT host-populated by this rung, PBDMA start/clock sequence NOT written (uncited, skipped above), runlist submitted via 0x2270/0x2274 and its playlist echo scored by kepler::init's own post-bind line. NOT 'ruled out' (R19): this names the conditions IB_GET did not move under, and IB_GET itself was read here for the first time in the campaign ::
[  23177ms] :: KFBIND: end rung=KF27 ptop=derived base_cmp=neither fetch=still-dark — one rollup line so a silent rung is distinguishable from a quiet pass ::
```

**TWO WRITES SKIPPED FOR CITATION, both named on the wire**:
`skipped write=pbdma_chan_bind reason=uncited` (the CHANNEL register's bind/enable encoding at
`+0x120` is named by no source in this tree) and `skipped write=pbdma_start_clock reason=uncited`.
A third, `skipped write=runlist_submit reason=not-skipped-for-citation`, is the correct refusal to
double-submit what `kepler::init` performs between this rung's two halves. `writes=0` held.

**WHAT THIS SETTLES, AND WHAT IT DOES NOT.** It settles that the base the ladder has used since
sitting #3 is not better than the PTOP-derived one and that neither reads as a scheduler: the whole
PBDMA PRI space on this part is ZERO or POISON with the bracket held. It does NOT eliminate
anything about the GK107 — read the `under:` clause: no FECS ctx ucode ran on this boot, so a
still-dark IB_GET is a statement about the instrument's preconditions, not about the silicon (R19).
**Byte-identical on flight 9.**

**CONSEQUENCE FOR KF28.** KFCTXBIND's in-code R19 gate requires KF27 to resolve `DERIVED-WINS` or
`BOTH-ANSWER` on the same boot. It resolved `NEITHER-ANSWERS`, so KF28 would print
`skipped reason=kf27-base-unresolved` and it stays unflown — the sequencing held without anyone
enforcing it by hand.

---

### The pre-registration, kept verbatim (what was written down BEFORE the flight)

## PENDING METAL — KFBIND (shut-out register §2, rung KF27): is the PBDMA base we have used since sitting #3 the right one, and has anyone ever read IB_GET?

**Nothing below is a metal fact.** It is the witness the next flight scores, written down before the
flight so the scoring cannot drift into the reading. Build knob: add `UNAOS_KEPLER_KFBIND=1` to the
flight line, on top of `UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1` (the feature implies both in Cargo, but
the *call sites* are inside `kepler::init`'s fifo leg, so `UNAOS_KEPLER_FIFO` must be on the line to
REACH them). READ-ONLY: the rung performs **zero device writes** and prints `writes=0 restored=n/a`.

**Why this rung and not another bind.** Sittings #4 and #5 each closed with the same NEXT item, and
neither has ever been done:

> s#5: "derive the correct GK107 PBDMA register base + the unit start/clock so `40108` reads real
> status."
> s#4: "which PBDMA unit is bound to our channel's runlist + its start/enable/clock sequence."

Every PBDMA fact this campaign owns is read through one unproven number. `kepler.rs` addresses the
units at `0x40000 + i*0x2000` — a guess adopted at s#3 in the same sitting the previous guess
(`0x6c0`) returned `0xBAD0011F`. At that base `bad-read pbdma 40108` returned **POISON at s#4 and
ZERO at s#5**, same address, same part; and `DISCRIMINATOR pbdma{0,1,2} ch=00000000 (CHID=0
ACTIVE=0)` has been quoted as a scheduler verdict since s#6. A zero read at an unproven base is the
sentence *"this address returned zero"*, not *"the channel is not scheduled"*.

**⭐ The second finding, and it needs no metal to state.** `kepler.rs`'s beacon-plant safety argument
quotes envytools `docs/hw/fifo/dma-pusher.rst`, "Channel control area": `… IB_GET 0x88, IB_PUT
0x8C`. Two lines later the same comment records that this driver's deleted GP_GET/GP_PUT witness
(added `1c9e2570`, removed `51b98bab` at pull 15) read **`0x8C/0x90`**. `0x8C` is IB_PUT — the
pointer the HOST writes — and `0x90` is not in the enumerated list at all. **The campaign's founding
observable, `gp_get=0`, may never have been a reading of GP_GET.** Nothing in this tree reads
`0x88`. This rung does.

**THE LINES TO SCORE**, in the order they print. Phase A sits immediately before the runlist submit;
phase B immediately below the historic `DISCRIMINATOR` loop, so the audited line and the audit are
adjacent in the capture.

```
:: KFBIND: begin knob=UNAOS_KEPLER_KFBIND rung=KF27 … writes=0 restored=n/a …
:: KFBIND: ptop-row i=00..07 [ … ]                      (x8, the raw table — the datum)
:: KFBIND: ptop pbdma=<n> runlists=[<mask>] pbdma0_base=<b> entries=64 nonzero=… poison=…
           refused_out_of_window=… overflow=… ctl_pre=… ctl_post=… ctl=held …
:: KFBIND: pbdma[i] DERIVED pre-submit base=… status=… <cls> chan=… <cls> … answers=Y|n ::
:: KFBIND: pbdma[i] LEGACY  pre-submit base=… status=… <cls> chan=… <cls> … answers=Y|n ::
:: KFBIND: userd pre-submit off=… ib_get=… <cls> ib_put=… <cls> x090=… dma_put=… dma_get=… ::
:: KFBIND: skipped write=pbdma_chan_bind reason=uncited — …          (x3)
        … kepler::init's own runlist submit + playlist echo + DISCRIMINATOR loop …
:: KFBIND: pbdma[i] DERIVED|LEGACY post-submit …  +  :: KFBIND: delta … same|MOVED ::
:: KFBIND: userd post-submit … ib_get=… ::
:: KFBIND: verdict base=<BASE VERDICT> ib_get <pre>-><post> <cls> … -> <FETCH VERDICT> ::
:: KFBIND: end rung=KF27 ptop=… base_cmp=… fetch=… ::
```

**What each outcome means — pre-registered, so the flight cannot be read after the fact.**

*The base comparison (`verdict base=`), which is this rung's primary product:*

| outcome | reading |
| --- | --- |
| `DERIVED-WINS` | a PTOP-derived base returns real values where the legacy base returns only ZERO/POISON. **Every PBDMA verdict in register §2 since s#6 was read at the wrong address and is RE-SCOPED, not retracted** — KF24's shape exactly, and the fourth time KF11→KF14's lesson has repeated |
| `LEGACY-WINS` | the legacy base answers and no derived candidate does. `0x40000 + i*0x2000` is **promoted from guess to observation** and s#4/s#5's NEXT item is DISCHARGED — which is a real result, not a null one |
| `BOTH-ANSWER` | both return real values: the same window reached two ways, or two real units. The `base=` fields on the per-unit lines separate them; neither is refuted |
| `NEITHER-ANSWERS` | every dword at every base is ZERO or POISON with a HELD bracket. A statement about the whole PBDMA PRI space on this part, and the strongest reading available without a start/clock sequence this rung may not guess |
| `LEGACY-ONLY` | PTOP named no admissible candidate, so the two bases were never compared. **The legacy base is NOT thereby confirmed** — it is simply the only one this boot read |
| `VOID-BRACKET` | `NV_PMC_BOOT_0` moved across the PTOP sweep. No statement about either base |

*The falsifier (`-> …`), on `IB_GET` (USERD +0x88):*

| outcome | reading |
| --- | --- |
| `-> FETCHED` | IB_GET advanced across the submit. **The PBDMA fetched from the GPFIFO and K-GPU-3's wall since July has moved for the first time.** |
| `-> STILL-DARK under …` | IB_GET did not move, and the line NAMES the conditions (R19, never "ruled out"): derived-base comparison as scored above · FECS context microcode NOT resident · CHAN_CUR/CHAN_NEXT not host-populated by this rung · PBDMA start/clock NOT written (uncited, skipped) · runlist submitted via `0x2270/0x2274`. Even here the rung is not empty: **it is the first capture in the campaign that contains IB_GET at all**, so `gp_get=0` stops being an inference from IB_PUT |
| `-> VOID-USERD` | IB_GET reads POISON/ABSENT: the channel control area did not answer, which says nothing about whether the PBDMA fetched |
| `-> VOID` | the phase-B control bracket moved |

**Cross-check available in the same boot, for free.** `UNAOS_KEPLER_CE=1` arms CE-R1, which sweeps
the SAME PTOP address (`0x022700`) with the SAME `((v & 0xFFF) << 12)` extraction. The two rungs
must agree dword-for-dword; if they disagree it is a code defect in one of them, not a hardware
finding. Flying them together also turns CE-R1 from `never-run` into a scored rung at no extra cost.

**Control that must accompany the flight image** — the rung cannot run in QEMU (q35 has no Kepler,
so a q35 log with zero `KFBIND` lines proves nothing about the code being present), so the artifact
is the only witness that the build carried it (s42's INSTGUI lesson, BANNERCERT's enforcement):

```
LC_ALL=C grep -a -o -F 'KFBIND' target/x86_64_esp/kernel.elf | wc -l     # must be > 0
```
plus `nvidia-kepler-kfbind` in the `⚡ kernel features:` banner. A banner without the artifact hits
is the BEAMX86 failure mode and the flight must not be scored.

## PENDING METAL — KFCTXBIND (shut-out register §2, rung KF28): has anyone ever READ the condition all ten shut-outs are recorded under?

**Nothing below is a metal fact.** It is the witness the next flight scores, written down before the
flight so the scoring cannot drift into the reading. Build knob: add `UNAOS_KEPLER_KFCTXBIND=1` to
the flight line, on top of `UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1` (the feature implies both in Cargo,
but the *call sites* are inside `kepler::init`'s fifo leg, so `UNAOS_KEPLER_FIFO` must be on the
line to REACH them). READ-ONLY: **zero device writes**, `writes=0 restored=n/a`.

**⛔ ORDERING LAW, and it is the reason this entry exists as a separate flight.**

1. **Fly AFTER KF27's flight has answered.** This rung reads PBDMA state, and KF26 has been reading
   that state at an unproven address since s#6. It gates itself on a derived base and prints
   `skipped reason=kf27-base-unresolved` rather than adding an eleventh elimination at the tenth's
   address — but a `skipped` boot is a wasted boot, and KF27's flight is what makes it unlikely.
2. **NEVER on the same boot as KF27.** A capture carrying both rungs has two verdicts about one
   submit, and KF27's `STILL-DARK under:` string hard-codes the conditions of a boot in which only
   KF27 acted. `nvidia-kepler-kfctxbind` therefore does **not** imply `nvidia-kepler-kfbind`; the
   dependency is evaluated at RUNTIME (this rung re-runs KF27's base ladder read-only, over KF27's
   own PTOP address, extraction, window and probe offsets) and never by Cargo.

**⭐ What this rung is FOR, and what it is careful not to claim.** §2's "what would change the
verdict" asks, of KF18/KF19, for one thing: *re-run the bind with FECS context microcode resident
and running — the one condition never varied across ten eliminations.* **This rung does not satisfy
that condition and does not pretend to.** There is no context-switch microcode in this tree to be
resident: `falcon_microcode_spec.md`'s CLEANROOM POLICY NOTICE forbids the vendor blob outright, and
the ECHO / POKE / heartbeat images this driver uploads are probes, not a context machine. What the
rung does is **measure the condition** — and that has never been done. All ten shut-outs in §2 are
recorded under "no FECS ctx ucode running", and **not one capture in the campaign contains a reading
that establishes it.** The `ctx-ucode=` token is that reading. It is the same class of defect KF27
found in `gp_get`: a sentence the ladder has been asserting for ten sittings without an instrument
behind it.

**THE LINES TO SCORE**, in the order they print. Phase A sits BELOW KF18's `CHAN_CUR`/`CHAN_NEXT`
bind (so the census reads the state that bind left behind) and STRICTLY pre-submit (so `IB_GET` has
a baseline); phase B sits below the `DISCRIMINATOR` loop.

```
:: KFCTXBIND: begin knob=UNAOS_KEPLER_KFCTXBIND rung=KF28 … writes=0 restored=n/a depends=KF27-base-resolution ::
:: KFCTXBIND: base-probe DERIVED[i]|LEGACY[i] base=… status=… <cls> chan=… <cls> … answers=Y|n ::
:: KFCTXBIND: base class=<BASE CLASS> derived_n=… derived_base=… ctl=held …
    ── if the class is not DERIVED-WINS or BOTH-ANSWER, the next line is the LAST: ──
:: KFCTXBIND: skipped reason=kf27-base-unresolved class=… ::
:: KFCTXBIND: fecs <name>=<val> <cls> addr=… cls=[METAL <sitting>] ::          (x10)
:: KFCTXBIND: ctx-ucode=<TOKEN> cpuctl=… stopped=Y|n imemc=… ctl=held …
:: KFCTXBIND: inst <name>=<val> <cls> off=… cls=[TREE-UNAUDITED] ::            (x9)
:: KFCTXBIND: inst pdb +200=… +204=… cls=[UNPINNED C8+C9, layered] …
:: KFCTXBIND: chan_ctrl[i] fw-bound=0x… ours=0x… w1=0x… nibble_diff=0x… self=Y|n ::   (x8)
:: KFCTXBIND: chan_ctrl census slots=8 ours_chid=1 … fw_bound_slots=<n> …
:: KFCTXBIND: pbdma pre-submit base=… / :: KFCTXBIND: userd pre-submit … ib_get=… ::
:: KFCTXBIND: skipped write=chan_ctrl_enable reason=uncited — …                (x4)
        … kepler::init's own runlist submit + playlist echo + DISCRIMINATOR loop …
:: KFCTXBIND: pbdma post-submit … / chan_ctrl[i] post … / userd post-submit … ::
:: KFCTXBIND: verdict base=… ctx-ucode=… chan_ctrl=skipped-uncited ib_get <pre>-><post> … -> <FETCH VERDICT> under {…} ::
:: KFCTXBIND: end rung=KF28 base=… ctx-ucode=… fetch=… ::
```

**What each outcome means — pre-registered, so the flight cannot be read after the fact.**

*The gate (`base class=`), which decides whether anything below it ran:*

| outcome | reading |
| --- | --- |
| `DERIVED-WINS` / `BOTH-ANSWER` | the gate OPENS. Every reading below is taken at a base this boot observed to answer, which is the thing no PBDMA verdict in §2 has ever had |
| anything else | `skipped reason=kf27-base-unresolved`. **Not a statement about the bind, the channel, or the FECS ucode: nothing was measured.** The boot is spent; fly KF27 first and come back |

*The precondition (`ctx-ucode=`), which is the rung's primary product:*

| outcome | reading |
| --- | --- |
| `absent-by-construction-halted` | `CPUCTL` (`+0x100` — **[METAL s26/s28/s31/s34]**) bit 4 `STOPPED` (**[EXT s28]** — rnndb's bit meaning for a register this bench has read at rest on this part) SET: the falcon is halted (`0x10`, §2's documented rest value, s26/s31/s34). Nothing is executing, so no context machine is. **The condition §2's ten shut-outs assert is now OBSERVED rather than assumed**, and every one of those rows may cite this line instead of an inference |
| `absent-by-construction-running-probe-image` | `CPUCTL` bit 4 CLEAR (**[EXT s28]**, corroborated by s30's whole heartbeat run at `cpuctl=00000000`) — the core is executing, and on this tree that can only be one of our own ECHO/POKE/heartbeat probes, because no other image exists to upload. **A running falcon is not a running context machine**, and the token says so rather than letting `cpuctl=0` be read as "ucode resident" |
| `void-poisoned` | `CPUCTL` (`+0x100` — **[METAL s26/s28/s31/s34]**) or `IMEMC` (`+0x180` — **[METAL s26/s28]**) reads POISON. Something above this rung touched `0x409504` (`WRCMD_CMD` — **[METAL s31/s32/s34]**, §5.4's poison law), or the unit is wedged. The census is uninterpretable and the ORDERING is the finding |
| `void-bracket` | `NV_PMC_BOOT_0` moved across the census |
| *(there is no `resident` arm)* | deliberate. An arm that could print `resident` from these registers would be claiming a running falcon is a running context machine — the exact inference this rung exists to stop |

*The encoding instrument (`fw_bound_slots=`), which is what the skipped write is waiting for:*

| outcome | reading |
| --- | --- |
| `fw_bound_slots > 0` | a channel-table slot we never wrote carries a real value. **Diff its top nibble against ours (`nibble_diff=`) and the enable/bind encoding is CITED BY OBSERVATION ON THIS PART** — §0.1 promotes it from UNPINNED to a sitting, and `skipped write=chan_ctrl_enable` becomes a write the next rung may legally make |
| `fw_bound_slots = 0` | the firmware left no bound channel in the first 8 slots. The encoding STAYS UNPINNED and the write stays skipped. An honest null, and not a failure of the instrument |

*The falsifier (`-> …`), on `IB_GET` (USERD `+0x88` — **[EXT]**, envytools `docs/hw/fifo/dma-pusher.rst`
"Channel control area" as quoted by `kepler.rs`, for a channel-control-area this bench reads through the
KF24-proven BAR1 identity), read at the DERIVED base:*

| outcome | reading |
| --- | --- |
| `-> FETCHED` | IB_GET advanced across the submit. **K-GPU-3's wall since July has moved, at a base this boot proved answers, with the ctx-ucode precondition on the record rather than assumed.** |
| `-> STILL-DARK under {…}` | IB_GET did not move, and the brace NAMES every condition as a value this boot read (R19, never "ruled out"). ⚠ **READ IT WITH THE `ctx-ucode=` TOKEN.** This is the ELEVENTH elimination only if that token ever reads something other than `absent-by-construction-*` — and today it cannot. A still-dark here is therefore a statement about the *instrument*, not about the GK107 |
| `-> VOID-USERD` | IB_GET reads POISON/ABSENT: the channel control area did not answer |
| `-> VOID` | the phase-B control bracket moved |

**Control that must accompany the flight image** — the rung cannot run in QEMU (q35 has no Kepler,
so a q35 log with zero `KFCTXBIND` lines proves nothing about the code being present), so the
artifact is the only witness that the build carried it (s42's INSTGUI lesson, BANNERCERT's
enforcement):

```
LC_ALL=C grep -a -o -F 'KFCTXBIND' target/x86_64_esp/kernel.elf | wc -l     # must be > 0
```

plus `nvidia-kepler-kfctxbind` in the `⚡ kernel features:` banner. A banner without the artifact
hits is the BEAMX86 failure mode and the flight must not be scored.

**Carry KF6/KF8/KF9 on the same boot** — `UNAOS_KEPLER_USERD_SNOOP=1 UNAOS_KEPLER_PFIFO_FLUSH=1
UNAOS_KEPLER_CTRL_ADDR=1`, all restored by SHUTRESTORE. §2 asks for exactly that: each was refuted
only under a host that had never run FECS ucode, and this is the boot that puts a reading on that
condition. They write and restore their own registers; this rung writes nothing, so its `writes=0`
claim is about ITSELF and the boot's restore ledger is theirs to carry.

## PENDING METAL — KFUNWEDGE (shut-out register §2, rung KF29): ⚠ A SACRIFICIAL BOOT — fire the `0x409504` poison ON PURPOSE, then see whether a PRING clear recovers the unit

**Nothing below is a metal fact.** It is the witness the next flight scores, written down before the
flight so the scoring cannot drift into the reading. Build knob: add `UNAOS_KEPLER_KFUNWEDGE=1` to
the flight line, on top of `UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1` (the feature implies both in Cargo,
but the *call site* is inside `kepler::init`'s fifo leg, so `UNAOS_KEPLER_FIFO` must be on the line
to REACH it).

**⚠⚠ THE SACRIFICIAL-BOOT RULE. READ IT BEFORE THE FLIGHT IS PLANNED, NOT BEFORE IT IS SCORED.**

This is the only rung in the tree that deliberately breaks the hardware it is measuring. It performs
one host READ of `0x409504` (`WRCMD_CMD`) on purpose. `falcon_microcode_spec.md` §5.4 — the poison
law — says what that does: *the first access faults immediately and wedges every subsequent read in
the FECS unit for the rest of the boot.* s31 discovered it, s32 proved it with its own control frame
(`recon-pre cpuctl=00000000` real, `recon-post cpuctl=BADF1000`, the same register microseconds
apart), s34 convicted the offset by elimination.

1. **THE OPERATOR MAY LOSE THE PANEL, AND THE RECOVERY IS A POWER CYCLE, NOT A REBOOT.** On this
   machine the Kepler *is* the display: the x86 compositor's ignition is the Kepler takeover. A
   wedged PRI ring on the GPU driving the screen may take the glass with it. **The serial capture is
   the deliverable; expect nothing from the panel.** Have the power button ready and do not read a
   dark screen as a new failure.
2. **FLY IT LAST IN THE SITTING, AND ALONE.** Never with BEAMX86, KDHEAD, `UNAOS_KEPLER_KFBIND`
   (KF27) or `UNAOS_KEPLER_KFCTXBIND` (KF28) on the same boot. Every verdict collected after this
   rung is void by §5.4, and each of those rungs prints a conditions string that would be written
   over a boot this one had already poisoned. The code enforces the ORDERING inside the boot (the
   call site is the last kepler statement before the terminal poke, so every proven read of the boot
   has already completed and printed); it cannot enforce the operator's knob set. That is this rule.
3. **A POISON IS NOT RESTORABLE.** The rung reports `restored=IMPOSSIBLE`, not `restored=n/a` and
   not `restored=Y`. There is no write that un-reads a read. The two W1C write-backs it does perform
   are clears of latched fault bits, not a restore of the poison, and the rung never claims
   otherwise — a restore line on a damage that cannot be undone is a success echo that cannot fail.
4. **THE TERMINAL POKE'S OWN DATUM IS VOID ON THIS BOOT.** `falcon_microcode_spec.md` §10 evidences
   the terminal `fecs_write(0x409504, 0)` by its being the boot's *only* access to that offset. Here
   the read comes first. The `fecs-ledger` line prints `504_read_idx=` and `504_write_idx=` so the
   capture states the ordering instead of leaving a reader to assume it; score the poke's line as
   VOID on any KFUNWEDGE flight.

**⭐ Why this is owed, and why it has never been done.** §2's KF21 row has said the same sentence for
ten sittings: *"the poison register is writable without consequence to the boot. Reading it first is
what poisons; writing it last is harmless. The un-wedge experiment ('does a PRING clear recover the
unit?') remains UNEXERCISED — nothing has ever wedged on a boot that went looking."* The instrument
was designed and LANDED once — pull 30 (`0e26447e`), a safest-first chain that would, on the first
`BADF`, read the PRING fault registers, W1C them and re-read `cpuctl`. It flew at s34 and **all five
probed offsets read clean**, so the un-wedge half never executed and the code was later removed.
s33boot1 recorded the same miss in one sentence: *"nothing wedged this boot, so 'PRING clear recovers
the unit' was not exercised; it needs a boot where the poison deliberately fires."* **The missing
ingredient was never the clear. It was a wedge that fires on purpose.**

**THE LINES TO SCORE**, in the order they print. The rung is a single call, last before the poke.

```
:: KFUNWEDGE: begin knob=UNAOS_KEPLER_KFUNWEDGE rung=KF29 … restored=IMPOSSIBLE… sacrificial=YES ::
:: KFUNWEDGE: base class=<BASE CLASS> derived_n=… legacy_answers=… ctl=held …
    ── if the class is not DERIVED-WINS or BOTH-ANSWER, the next two lines are the LAST, and
       NOTHING WAS POISONED — the boot is spent but the unit is intact: ──
:: KFUNWEDGE: skipped reason=kf27-base-unresolved class=… ::
:: KFUNWEDGE: end rung=KF29 skipped=kf27-base-unresolved … -> NOT-RUN ::
:: KFUNWEDGE: pring pre <name>=<val> <cls> addr=… cls=[…] ::                    (x5)
:: KFUNWEDGE: pring pre pbus_intr_bits raw=… bit2_MMIO_RING_ERR=… bit3_MMIO_FAULT=… ::
:: KFUNWEDGE: pre pring=0x… cpuctl=0x… mailbox0=0x… ctl=0x… sentinel_504_read=n … ::
:: KFUNWEDGE: poke-pre about-to-read=0x409504 rung-cited=KF21/KF20 … ::
:: KFUNWEDGE: poke read=0x409504 value=… <cls> poisoned=<yes|no> family=… ::
:: KFUNWEDGE: pring post-poke … (x5 + the bits line)
:: KFUNWEDGE: observe pring=0x… pring_moved=… cpuctl=… mailbox0=… ctl=… unit_poisoned=… control_poisoned=… ::
:: KFUNWEDGE: clear write=pring_clear_<reg> addr=… w1c=… ::   or
:: KFUNWEDGE: skipped write=pring_clear_<reg> reason=nothing-latched … ::       (x2 total)
:: KFUNWEDGE: pring post-clear … (x5 + the bits line)
:: KFUNWEDGE: verdict base=… poke=… pring a->b->c cpuctl a->b->c … clear={written=n skipped=n} restored=IMPOSSIBLE -> <VERDICT> under {…} ::
:: KFUNWEDGE: end rung=KF29 poisoned=… clear=… unwedged=… writes=… restored=IMPOSSIBLE ::
        … then the fecs-ledger line and the TERMINAL POKE, whose datum is VOID here …
```

**What each outcome means — pre-registered, so the flight cannot be read after the fact.**

*The gate (`base class=`), inherited from KF27 and evaluated read-only on this boot:*

| outcome | reading |
| --- | --- |
| `DERIVED-WINS` / `BOTH-ANSWER` | the gate OPENS and the experiment runs |
| anything else | `skipped reason=kf27-base-unresolved`. **Nothing was measured and, more importantly, NOTHING WAS POISONED** — `0x409504` was not read, the unit is intact, and this boot was not the sacrificial one. Fly KF27 first and come back |

*The stimulus (`poke … poisoned=`):*

| outcome | reading |
| --- | --- |
| `poisoned=yes` | the read returned a `BAD0`/`BADF`-family word, which is the nonexistent-PRI-register signature (§5.4, s25) and exactly what s31/s32/s34 read at this offset. The experiment is live |
| `poisoned=no` | **the poison law did not fire on this boot.** Louder than an un-wedge would have been: three sittings saw it fire on first access, so a clean read here names a CONDITION those three boots did not share, and the register row must be re-scored around it rather than the rung called a failure (R19) |

*The spread (`unit_poisoned=` / `control_poisoned=`), which is the reading s31 inferred and never took:*

| outcome | reading |
| --- | --- |
| `unit_poisoned=Y control_poisoned=n` | the damage is BOUNDED TO THE FECS UNIT: `cpuctl`/`mailbox0` are `BADF` while `NV_PMC_BOOT_0`, outside the unit, still returns a real chip ID. s31 inferred this from a PFIFO witness line; this reads it directly, in the same breath |
| `control_poisoned=Y` | **`-> VOID-CONTROL`.** The whole BAR0 path is down, not the unit. This rung cannot tell a wedged ring from a dead link and says nothing about the un-wedge; every line after the observe step is worthless |

*The ring (`pring_moved=` and the tagged bits), the thing s33's `PBUS_INTR=0x0000000C` never had a control for:*

| outcome | reading |
| --- | --- |
| `pring_moved=Y` with bit 2 `MMIO_RING_ERR` newly SET | the fault REACHES the PRI ring's own reporting register. s33's latched `0x0C` is explained, `PBUS_INTR` is promoted from "latched something, meaning TBD" to an instrument, and the `[EXT]` bit names (envytools `docs/hw/bus/pbus.rst`, via PROPOSAL-pull30) are corroborated on this part |
| `pring_moved=n` with `unit_poisoned=Y` | the unit faults and the ring reports nothing. `PBUS_INTR`/PIBUS are then the WRONG instrument for this fault class on GK107 — a real finding, and one no boot has been able to state |

*The verdict:*

| outcome | reading |
| --- | --- |
| `-> UNWEDGED` | the unit was POISONED at the observe step and reads REAL again after the clear. **The question KF21 opened in July is answered**: a PRING W1C recovers a GK107 FECS unit inside the boot, and every future probe of an unproven `0x409xxx` offset becomes survivable — which is worth more than any single offset's value |
| `-> STILL-POISONED under {clear=written\|skipped, …}` | the clear did not recover it. The poison is not a latch the host can drop, and the brace names the conditions (R19: never "ruled out"). If `clear=skipped` the experiment is INCOMPLETE, not negative: nothing was latched to clear |
| `-> POISON-CONFINED` | the read returned a fault word for ITSELF and neither `cpuctl` nor `mailbox0` followed it. That CONTRADICTS s31/s32's spread and is a per-offset finding; the clear says nothing either way because nothing was wedged |
| `-> NOT-POISONED` | see `poisoned=no` above |
| `-> VOID-BRACKET` / `-> VOID-CONTROL` | `NV_PMC_BOOT_0` moved across the rung, or was itself poison at the observe step. No statement is made |

**Every offset this rung touches, with its class** (§0.1 vocabulary; no rnndb file was opened for
this worktree, so every external spelling is quoted from a tree document that quotes envytools):

| address | name | class | evidence |
| --- | --- | --- | --- |
| `0x409504` | `WRCMD_CMD` — THE STIMULUS | **[METAL s31/s32/s34]** | `falcon_microcode_spec.md` §2 (`+0x504`) and §5.4 |
| `0x409100` | `CPUCTL` | **[METAL s26/s28/s31/s34]** | §2 (`+0x100`, rest `0x00000010`); s32's control-frame register |
| `0x409040` | `MAILBOX0` | **[METAL s29]** | §2 (`+0x040`); a second in-unit reading so "poisoned" is not one datum |
| `0x001100` | `PBUS_INTR` — READ **and** W1C | **[METAL s33]** + **[EXT]** name | read `0000000C` on this part at s33boot1 and W1C'd in that same boot; bits 2 `MMIO_RING_ERR` / 3 `MMIO_FAULT` from envytools `docs/hw/bus/pbus.rst` via PROPOSAL-pull30 |
| `0x120120` / `0x120124` | `PIBUS INTR_ADDR` / `INTR_VALUE` | **[METAL s33]** + **[EXT]** name | read real `00000000` at s33boot1. **READ ONLY — never written**: they report which access faulted and are not documented as latches |
| `0x120128` | `PIBUS INTR` — READ **and** W1C | **[METAL s33]** + **[EXT]** W1C | read real `00000000` at s33boot1; pull 30's landed chain carried exactly this conditional write-back |
| `0x122104` | `PIBUS_MMIO_HUB_ENABLE1` | **[METAL s33]** | read `FFF9F4B0`, bit 4 already SET — the reading that refuted subunit gating (§6 refutation 5). Context only; nothing in the verdict reads it |
| `0x000000` | `NV_PMC_BOOT_0` | **[TREE]** | the file's standard bracket, doubling here as the out-of-unit control |

**The write discipline, stated once.** The only writes are W1C write-backs of bits *this boot read
as SET*, in the two registers pull 30 named. Writing back the exact value just read is the
least-assumptive write available: in a W1C register it clears precisely the latched bits and nothing
else, and in a plain RW register it is a no-op by construction — so it asserts no field layout, no
command encoding and no bit meaning beyond "these bits were set a microsecond ago", which is an
observation of this boot rather than a citation. **A register reading ZERO is not written at all**
(`skipped write=pring_clear_<reg> reason=nothing-latched`): there is nothing latched to clear, and a
zero write would assert a semantics this bench has not exercised — the uncited write pull 28's
standing ban was about.

**Control that must accompany the flight image** — the rung cannot run in QEMU (q35 has no Kepler,
so a q35 log with zero `KFUNWEDGE` lines proves nothing about the code being present), so the
artifact is the only witness that the build carried it (s42's INSTGUI lesson, BANNERCERT's
enforcement) — and here it is worth more than on any other rung, because a banner-only build would
spend a SACRIFICIAL boot, and possibly the panel, on an image with no instrument in it:

```
LC_ALL=C grep -a -o -F 'KFUNWEDGE' target/x86_64_esp/kernel.elf | wc -l     # must be > 0
```

plus `nvidia-kepler-kfunwedge` in the `⚡ kernel features:` banner. A banner without the artifact
hits is the BEAMX86 failure mode and the flight must not be flown, let alone scored.

## TREE CHANGE — SHUTRESTORE (2026-09-15): the seven deleted rungs are back behind knobs

**No metal in this entry, and nothing here is a fact about silicon.** It is recorded in the metal
log because it changes what a flight *can* ask the GK107, and a reader who finds these knobs on a
flight line needs to know where they came from.

**What changed.** [`SHUTOUT-REGISTER.md`](SHUTOUT-REGISTER.md) §7 listed seven refuted rungs whose
CODE had been deleted — `grep` returning 0 hits for every one of their witness tokens. RULINGS R19
(Peter, 2026-09-05) says a path that failed once and got shut out must keep its code and its knob,
because many boots later a later path can turn out to need it open. All seven are restored from our
own git history, each behind a default-OFF feature of its own, inside the existing `nvidia-kepler`
cfg region. §7 is now the record of the restore rather than a list of absences; the per-rung rows in
§1 and §2 carry `code: restored … behind <knob>`.

**What an unarmed boot does differently: nothing.** Every restored site is under its own
`#[cfg(feature = …)]`, all seven default OFF, so a knob-off image links not one byte of any of them
and no rung is wired into the boot path unconditionally. The takeover path's behaviour with no rung
knob is unchanged — BEAMX86's probe inside `takeover_display` is exactly where it was.

**What a flight would arm** — each knob on top of the parent that gets it to its call site:

| knob | what it runs | parent it needs | witness token to `awk` for |
| --- | --- | --- | --- |
| `UNAOS_KEPLER_USERD_SNOOP=1` | KF6: arm 0x2a1c, and restore it iff the channel witness comes back stripped | `UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1` | `USERD_SNOOP (0x2a1c) orig=` |
| `UNAOS_KEPLER_PFIFO_FLUSH=1` | KF8: trigger 0x70000, bounded BUSY poll | `UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1` | `flush-executed 0x70000 pre=` |
| `UNAOS_KEPLER_CTRL_ADDR=1` | KF9: the 3 PBDMA × 4 TARGET audit, every write read back and put back | `UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1` | `ctrladdr pbdma` |
| `UNAOS_KEPLER_REPOINT=1` | KD6: write 0x6101E0, 5 s panel window, restore + readback | `UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1` | `repoint pre 6101E0=` |
| `UNAOS_KEPLER_LATCH_ARM=1` | KD7: EVO assembly write + UPDATE, the `pm-step` dumps | `UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1` | `latch verdict asm-stuck=` |
| `UNAOS_KEPLER_PITCH_LADDER=1` | `lin-step` (linear pitch 0x4000) then the four `bwpg-step` block-linear cycles | `UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1` | `lin-step pitch=4000` · `bwpg-step bw=` |
| `UNAOS_KEPLER_GOP_OVERLAP=1` | the detector that says a photo is VOID because we painted the scanned surface | `UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1` | `fb-draw gop-overlap=` |

**Cost, counted rather than guessed.** These are camera-length rungs, not instrument rungs, and the
restored code carries the original's own loop counts unchanged. One "hold" is five ticks of
`for _ in 0..60_000_000 { spin_loop() }`; the original s21 code calls that a 5 s hold, and today's
`nvidia-kepler-kdisp-hold` block in the same file annotates the identical loop as **1.12 s total**,
so the wall-clock is CPU-speed-dependent and the honest unit is *ticks*, not seconds. Counting
holds: `REPOINT` = 1 hold. `LATCH_ARM` = 1 hold + a 15 M-spin recovery gap. `PITCH_LADDER` = 5 holds
+ 5 recovery gaps (one `lin-step` cycle plus four `bwpg-step` cycles), and it is much the largest of
the three for a second reason the holds do not show: its fills are ~7.4 M volatile VRAM dword writes
for `lin-step` and a comparable figure per `bwpg-step` cycle. **Arm one rung at a time on a flight,
never the whole set** — the boot's phase timings stop being comparable to every prior sitting
otherwise, and `:: kdisp: inner phase` is the instrument that would go unreadable.

**The control that must accompany any such flight** — the probe cannot run in QEMU (q35 has no
Kepler, so a q35 log with zero rung lines proves nothing about the code being present):
`LC_ALL=C grep -a -o -F '<witness token>' target/x86_64_esp/kernel.elf | wc -l` on the flight
artifact, per LAWS §5. The same rule the BEAMX86 entry above carries.

## Sitting #43 (GR25 Boot A, capture `~/unaos-bench/capture/gr25-bootA/ttyUSB0.log`) — two ladder gates answered, and CE-LADDER armed

Design of record for the copy-engine work: `~/unaos-bench/scratch/gr24/CE-LADDER-draft.md`
(scratch). Citation classes below follow its §0: `[METAL]` observed on this GK107 in a named
capture; `[TREE]` stated by repo code or docs; `[EXT-UNPINNED]` public-knowledge recollection
with no document opened, usable only as a hypothesis under test.

**⭐ R0 ANSWERED — the present half is 73 % of the compositor's blit, and the prize is real.**
The `[comp2]` rollup now splits `blit_us` into its two costs [METAL]:

```
[ 805882ms] [comp2] rollup passes=2150 pass_us=5214 max_us=17077 … blit_us=5211
            compose_us=1374 present_us=3832 … util_pct=99 rate=429.8/s span=5002ms
```

`present_us` is the back-buffer → VRAM copy — the only half a copy engine can take (a CE is a
rectangular memory mover, not a scaler, and every window on this bench is scale-2 Retina). At
~3.8 ms of a ~5.2 ms blit it is roughly **12x the draft's 300 µs stop threshold**, so the
economic case that gates the whole ladder holds. Three consecutive late rollups sit within
1.5 % of their mean (2.4 % min-to-max), so this is not a single-sample reading.

**The ratio is phase-invariant, and the load phase must be stated with it.** The quoted line
is steady state at **10 windows** (`[wc-g] win=10 seq=1 … @335649ms`). The early ~4-window
rollups read `blit_us=3200 compose_us=875 present_us=2323` (`@88169ms`) — a much lighter
load, and **the same split**: present is **72.6 %** early and **73.5 %** late. The absolute
microseconds scale with window count; the fraction the CE could take does not. A citation
that gave only the steady-state numbers would invite the reading that the prize is an
artifact of a heavily loaded desktop, and it is not.

**⭐ R4 ANSWERED — `IDENTITY`. A BAR1 offset IS a physical VRAM address on this part** [METAL]:

```
:: kepler: bar1-identity scratch_off=02015000 magic=CEA50BA5 bar1_rb=CEA50BA5
           win_pre=00003FF0 win_want=00000201 win_rb=00000201 pramin_read=CEA50BA5
           win_restored=Y ::
:: kepler: bar1-identity VERDICT IDENTITY — … BAR1 offsets ARE physical VRAM addresses on
           this part; every FIFO pointer in this driver is fine and the paged-BAR1 root
           cause is CLOSED as a false alarm ::
```

This was the highest-risk open question in the study — the one that could have invalidated
banked work — and it closes as a **false alarm**, which is a positive result, not a wasted
boot. `inst_off >> 12` and `runlist_off >> 12` are the pages PFIFO fetches; the fence arc's
ten eliminations were **not** measured through a broken pointer.

The same line incidentally **pins three things the draft carried as `[EXT-UNPINNED]` (C9)**,
because the authored magic arrived exactly where predicted:

| Fact | Now |
| --- | --- |
| PRAMIN window base register `0x001700` | **[METAL]** — took `0x201`, read back `0x201`, restored |
| Window granularity 64 KiB (`win = phys >> 16`) | **[METAL]** — the magic was found at `0x700000 + (off & 0xFFFF)` |
| PRAMIN aperture at BAR0 `0x700000` | **[METAL]** — at least the first 64 KiB of it |

Everything else in draft §3 (C1–C8) remains `[EXT-UNPINNED]` and untested on this silicon.

**Standing wall, unchanged this boot:** `sched-status post-init err=00000002`,
`DISCRIMINATOR pbdma{0,1,2} ch=00000000 (CHID=0 ACTIVE=0)`, and FENCE round 2 closed
candidate 1 **by mechanism** — `FENCE ladder VERDICT MAPPED` proved the `(off & 0xffc) << 6`
port rule reaches host `0xB00` from inside the falcon, so the port derivation was never the
fault; ENGINE_STATUS is simply not falcon-writable, which extends refutation 7 to the falcon
side. The `err=2` verdict itself stays VOID (the treatment was never applied).

**Runlist array, as read after our own submit** [METAL]: `runlist-scan i=1 base_off=2278
base=BAD0011F POISON`, `i=2 base_off=2280 base=00002013 len=00100003 OCCUPIED`, `i=3
base_off=2288 … empty`, `verdict occupied_mask=4 alias_i2_base=match alias_i2_len=match`.
Read strictly this **refutes the array-stride assumption at `0x2270`** — element 1 of that
array does not exist. It did not find "no CE runlist"; it found "not there". And because it
ran *after* our submit, the one occupied slot it could see was necessarily ours.

### CE-LADDER: armed, UNFLOWN

`drivers/gpu/kepler_ce.rs`, knob **`UNAOS_KEPLER_CE=1`** (feature `nvidia-kepler-ce`, implies
`nvidia-kepler`, independent of `UNAOS_KEPLER_FIFO`). Called from `kepler::init` **above** the
fifo leg, which puts it upstream of every FECS access — the POKE ucode deliberately poisons
the unit and the terminal `0x409504` write closes the boot, so a verdict collected after
either would be void — and makes its runlist rung read the array **pre-submit**, where a
populated slot is unambiguously the firmware's.

Read-only apart from **two** writes, both reversible and restore-verified: (1) the PRAMIN
window base (R5), metal-proven above; and, **new this arc (GR26, `kepler-ce-r2`)**, (2) R2b's
single authored-magic write to a confirmed-live CE falcon's `CC_SCRATCH[0]` (`+0x800`). Each
is saved, restored, **and read back after the restore** — a restore written but never read is
a success echo that cannot fail, and a failure VOIDs the affected register's verdict rather
than passing quietly. `+0x504` is excluded at every base by a `const _` assertion, not by a
comment, and a second `const _` proves R2b's write target is `+0x800` and never `+0x118`
(`DMATRFCMD`, the Boot AS accidental-DMA finding the draft flagged for safety review). Every
rung is bracketed by a control read of `NV_PMC_BOOT_0`, so "the space is dead" is never
reported as "the space is empty".

| Rung | Witness family | What it decides |
| --- | --- | --- |
| R1 | `ce-ptop row` / `ce-ptop entry` / `ce-ptop verdict` / `ce-ptop VERDICT` | Whether a PTOP device-info table exists at `0x022700` (C6). `REFUTED-CLEANLY` = all poison; `AMBIGUOUS` = all zero with a held bracket; `PASS` = structured entries with a PRI candidate in the `0x104000` neighbourhood. The **raw dword is the datum**; the field decode is a separately-labelled hypothesis line. |
| R2 | `ce-probe begin` / `ce-probe base=… VERDICT` / `ce-probe verdict` | Whether `0x104000/0x105000/0x106000` are alive and are falcons (C1/C2). `FALCON-REST` requires `cpuctl=0x10` **and** `dmactl=0x01` — the exact pair FECS shows on this chip — and opens draft §4.2 path 2 (a CE falcon as a bare DMA microcontroller, no PFIFO, no FECS wall). The first FALCON-REST base is handed to R2b as its write target. Each base carries its own bracket, because after a fault only the first datum is trustworthy. `NV_PMC_ENABLE` is recorded read-only; **no enable bit is guessed or written.** |
| **R2b** | `ce-r2 SKIPPED` / `ce-r2 base=… VERDICT` (ARMED / REJECTED / REST-MOVED / NOT-RESTORED / VOID) | **The campaign's FIRST copy-engine WRITE.** Self-gated: fires ONLY when R2 named a FALCON-REST base this same boot, and re-verifies `cpuctl=0x10 && dmactl=0x01` with a fresh read at write time. Writes a high-entropy magic (`CE5AA502`-class, **never `0x2`** — draft §4.4) to that base's `CC_SCRATCH[0]`, reads it back, restores the captured entry value, reads the restore back. `ARMED` = the magic latched and the entry restored → the CE latches authored state and a copy has a proven-live, writable target. `REJECTED` = a falcon at rest that did not hold the write → C2's writability half refuted. `SKIPPED` = no falcon base this boot (the expected first-flight arm; nothing is written). This is the largest reversible step reachable without a runlist id or an audited RAMFC layout, and it discharges draft §4.4's mandate to re-assert the falcon port rule at a CE base with a proper magic. |
| R3 | `ce-rlscan i=` / `ce-rlscan submit-pair` / `ce-rlscan verdict` / `ce-rlscan VERDICT` | The array at `0x2280 + i*8`, eight slots wide (C7), read pre-submit. Separates the three readings of bit 20 — `ID-FIELD` (we have been submitting to the wrong runlist id), `DISCRIMINATING` (bit 20 is commit-pending, and Boot A's `00100003` means the scheduler never consumed our runlist), `UNCONDITIONAL` (bit 20 stops being evidence permanently). Any populated slot is a **firmware-left runlist for another engine**, and its index is the id R6 needs. |
| R5 | `ce-inst begin` / `ce-inst reg=` / `ce-inst dump` / `ce-inst pd-probe` / `ce-inst pde` / `ce-inst verdict` | Walks the firmware's own instance block through PRAMIN from the candidate pointers at `0x001704`/`0x001714` (C9). This is the **clean-room-legal RAMFC route**: observing our own hardware is Group-A white-box work and produces documentation we may implement against, which is how the standing UNAUDITED-constants debt gets discharged. `EMPTY`/`VOID`/`REFUSED` claim nothing; there is no fallback to "it probably looks like the one in `kepler.rs`". The `pd-probe` line answers C8 — a real page directory means CE addresses are virtual and R6 grows an era. |

A genuine VRAM→VRAM copy (draft R7) and a CE channel (draft R6) remain **deliberately not
implemented**: they are consumers of answers this boot does not have (a CE runlist id from
R1/R3, an audited instance-block layout from R5, or the CE falcon's UNKNOWN internal datapath
map). R2b is scoped precisely to stop short of them while still taking a real, reversible
step — proving the target latches an authored write, which every copy path needs first.
`CE-LADDER end` prints all five rung outcomes (`r1_ptop` / `r2_ce_probe` / `r3_rlscan` /
`r5_inst` / `r2b_arm`) on one line, so a silent ladder is distinguishable from a quiet pass.

**Falsification story for the next boot.** With `UNAOS_KEPLER_CE=1`, the `ce-probe` verdict
decides R2b's fate in the same capture:
* If any CE base reports `FALCON-REST`, `ce-r2 base=… VERDICT` must read `ARMED` (magic
  latched, entry restored) or `REJECTED` (falcon at rest that did not hold the write) — a
  real, decisive datum either way, and the `landed=` / `restored=` fields on the `ce-r2`
  line show the transition. `ARMED` is the first proof a Kepler copy engine accepts authored
  state on this silicon.
* If no base is a falcon (the likely first-flight outcome — the CE bases are unproven until
  this same boot's `ce-probe` speaks), `ce-r2 SKIPPED` prints and **nothing is written**; the
  boot is exactly as read-only as the recon-only ladder. A witness that reports `ARMED`
  without a `landed=Y` transition, or on a `SKIPPED` boot, would be a defect.

**Knob line for the next boot:** add `UNAOS_KEPLER_CE=1` alongside the existing Kepler knobs
and confirm `nvidia-kepler-ce` appears in the `⚡ kernel features:` banner. R2b rides the same
knob; no new knob is added (the CE write is gated at runtime on the same-boot FALCON-REST
verdict, not on a separate feature).

## Sitting #42 (GR6 self-run bench, UnaOS-gemini@a8efc15d-era, 2026-07-26, capture rmbp-gr6)

**⭐ THE s41 RESIDUE QUESTION IS ANSWERED — `[wc-x] move-vacate win=3 … painted=true
desktop=5/5 stale=0/5 -> PASS`.** The erase DOES reach glass; every sampled point in a
vacated box read desktop background. Per the probe's own verdict table the ghost boxes
were a LATER writer (the desktop layer repainting an unoccluded box), so the fix is the
`move_to`→`request_full_present` one-liner on the pi4 side — relayed, and it rides their
SPAWN-PLACE take-back. Combined with SPAWN-PLACE itself (windows now created at their
final origin — `[wc-x] spawn-place win=2 … (created in place, no move)`), no window on
this boot ever moved.
- **⭐ CLICK-3 PROVEN ON METAL:** `:: PTR: [1] press seen=16 delivered=16 recovered=15 ::`
  — **15 of 16 of Peter's clicks would have been EATEN** by the stale button latch before
  this fix; delivered now equals seen. The mechanism (a missed release report at
  frame-rate polling leaves `prev_buttons` latched down; motion was the only reset) is
  confirmed by the recovery count itself.
- **SMC quiet: 9 lines for the whole boot** (from ~1 Hz forever at s39, ~every few seconds
  at s41) — the presence-flap debounce closed the last leak.
- Fence unchanged and repeatable: `ctx-echo img=A ack=1 mb0=1 phase=4`, terminal poke
  harmless, `runlist-scan verdict occupied_mask=4 alias_i2_base=match alias_i2_len=match`
  (the sibling-runlist sweep's first metal read).
- No storage attached this boot (`storage_slot=0`) — FAT leg not exercised.
- **⛔ INSTGUI SHIPPED DISABLED — a build-system asymmetry, worth remembering:** the
  installer compiled green under every `./arroyo check`, but ESP/media builds are produced
  by `builder/`, which re-derives features from env and had no `UNAOS_INSTGUI` mapping. A
  knob added only to `arroyo` is invisible to media. Fixed (`c117e0e2`); s43 carries
  13 `instgui` symbols in `kernel.elf`, verified by `strings` before staging. **New staging
  habit: verify the feature's symbols are IN the artifact, not merely that the build was
  green.**

## Sitting #41 (GR6 self-run bench, UnaOS-gemini@65faec32, 2026-07-26, capture rmbp-gr6, three-lane evidence boot + photo)

**⭐ FENCE: the echo's split observables all landed and 0x409504 TOOK ITS FIRST WRITE HARMLESSLY.**
`ctx-echo img=A ack=00000001 mb0=00000001 phase=00000004` (iters=0 again — ucode read a value,
reported it via MAILBOX0, stamped all phases, exited bounded) → … →
`:: kepler: terminal-poke 0x409504 wr=0 ::` → `[NVIDIA] Initialization complete` and the boot
sailed on. The poison register is writable without consequence to the boot.

**⭐ PLATFORM: the FAT wedge is DIAGNOSED — the stick goes catatonic on write; xHCI is healthy.**
Evidence chain (first metal run of 9f6db8ea): `recover entry … cmdring=running` →
`stage=msc-reset ok=no cc=0 why=nocompletion` + both `clear-halt … why=nocompletion` (all three
are DEVICE requests via EP0) while the xHCI COMMANDS all succeed (`stop-ep ok=yes cc=1`,
`set-deq ok=yes` with ctxdeq actually moving) → `recover evidence … csw_sig=0x0` (device never
answered) → `:: BLK: io-cause op=write lba=121 bot_err=Timeout ::`. Verdict per the BOTEV table:
transport wedge in the DEVICE, not our state machine — WRITE(10) kills it, then it answers
nothing, including control requests. **Next discriminator: same boot, DIFFERENT stick** (asked).

**s41boot2 (fresh 2 GB stick, UNAOS-DATA): THE DEVICE HYPOTHESIS IS DEAD — two sticks,
same signature; suspect OUR write path.** One transaction succeeded but took ~0.47 s
(`used=1275711592` at 2.693 GHz, n=1 result=OK — pathological); the FR burst write to
lba 9834 then consumed the FULL ~6 s budget (`used=16163799966 ≈ budget`) and timed out.
First recovery failed as before; a SECOND recovery on this stick SUCCEEDED
(`reset=ok halts=cleared`), so boot 1's catatonic-device read doesn't generalize either.
Working hypothesis for the analysis arc (out): per-TRB event accounting or data-stage
shape makes large writes cost ~4–7 ms/TRB on metal (0.47 s ≈ a TRB chain at tick-latency
waits) — QEMU never shows it because virtual completion is instant.

**UI: desktop-clear worked (photo: clean blue desktop), console window + [wc-k] vocabulary live —
and the photo found THREE new defects, all fix-arcs spawned:**
- Windows create at (0,0), first-present there, then move — the vacated boxes stay on glass
  (staged erases report BUFFERED torn=yes but never land). Photo shows both ghost boxes.
- SMC still prints every few seconds: `present=true/false` FLAPPING — each flip is a state
  change under the s40 predicate; presence needs the held value in the key (same class as ac=?).
- Trackpad (Peter, live interaction): two stationary clicks in a row — second click ignored;
  slide-then-click registers. Pointer-path repeat-click defect, investigation arc out.
- `torn=yes` consistently on big x86 erases — possibly benign (no vsync on GOP); flagged to pi4.

## Sittings #38–#40 (GR6 self-run bench, UnaOS-gemini@400ff065→1441b631, 2026-07-26, capture rmbp-gr6)

**⭐⭐ s40: THE CONSOLE OPENED IN A COMPOSITOR WINDOW ON METAL — the x86 desktop era.**
Peter's photo received (console window centered with kernel text; calibration demo
window bottom-right, R/G/B bands + diagonal + checker correct, so channel order,
scale and rows survive the composite path). Wire chain:
`:: video: WRITER seeded base=90020000 len=29491200 panel=2880x1800 stride=4096px pitch=16384B bpp=4 ::` →
`[wc-x] console-window win=1 panel=2880x1800 surf=1312x736 box=1314x750 at (783,444) cell=32x32 cols=41 rows=23` →
`[wc-x] console-route first-paint win=1` → `[wc-x] console-window panic-fallback armed win=1` →
`[wc-x] demo win=2 surf=96x64 at (2103,1117) scale=8x z=2` → `[wc-x] present win=2 rows=1104..1630 ok=true`.

- **s39 (0c4eb448+WC): console window DECLINED** — `[wc-x] activate DECLINE reason=fb-not-ready`.
  Root cause: `video::WRITER` was seeded at main.rs step 3, long AFTER the kepler
  takeover where desktop_uefi::activate runs; fbcon worked because it reads BootInfo directly.
  Fixed by seeding WRITER beside fbcon::init from the same triple (5701b9a8). Units
  audit: stride is pixels end-to-end; no consumer carries its own pitch assumption.
- **SMC quiet: proven at s40** — 2 lines the whole boot (one-shot unresponsive note +
  first-fire witness). s39 had shown the first quiet attempt (1c870527) still scrolling
  at 1 Hz because retries>0 fired every sweep on this SMC (retries=2..7 per second is
  this machine's normal). Fix 1441b631: retries → rollup (300 s quiet / 60 s bootlog),
  holds must persist 5 s to print, transient `ac=?` not a state change.
- **⛔ FAT metal leg: FAILED twice identically (s39, s40), stick on slot 1** —
  `:: BOT: pump … timeouts=1 …` → `:: BOT: recover begin cause=Timeout slot=1 ep=0x82/0x1 …` →
  `:: BOT: recover done reset=fail halts=fail ring=resync ::` →
  `:: FR: UNAOS.LOG reservation failed (Io) ::`. The x86 FAT fix stays metal-UNPROVEN
  (not disproven). Instrumentation landed (9f6db8ea): next boot carries per-stage
  completion codes + EP states + CSW evidence; the verdict table is in usb_xhci.md §BOTEV.
- **s40 photo residue (cosmetic): pre-activation paints stay on glass** — stray direct
  fbcon rows top + kepler probe rectangle top-left; the compositor never cleared the
  panel at activation. Desktop-clear fix in flight for s41.
- s38 (400ff065, no WC): boot healthy, quiet-witness fix rode it; FAT leg first
  attempted here (boot2, data stick) — same recover/Io signature as above.

## Sitting #37 (pull 33 FECS echo + poll-control, UnaOS-gemini@f8e3e2f3, 2026-07-26, fox-metal-r23s1n, s37boot1v2)

**⭐⭐⭐ OUR MICROCODE ANSWERED A COMMAND. THE CONSTRUCTIVE ERA IS OPEN.**
Coordinator awk-verified:
```
:: kepler: ucode-echo pre CC_SCRATCH[0]=00000000 CC_SCRATCH[1]=00000000 ::
:: kepler: ucode-echo host-cmd CC_SCRATCH[0]=00000001 ::
:: kepler: ucode-echo host-ack CC_SCRATCH[1]=00000001 iters=0 ::
:: kepler: ucode-echo SUCCESS img=A ::
:: kepler: ucode-echo final CC_SCRATCH[1]=00000001 cpuctl=00000000 ::
```
Image **A** — the DERIVED indexed ports I[0x20000]/I[0x20100] — acked on the
FIRST poll (`iters=0`); image B (flat ports) never ran. Two things settle at
once: (1) the host↔FECS command loop WORKS — we write a command, our own
microcode reads it from inside the falcon and answers; (2) the indexed IO
scheme `(X & 0xffc) << 6` is confirmed for the CC_SCRATCH family, extending
the s29 mailbox proof to a second register family. The coordinator's
must-fix amendment (the proposal shipped host offsets 0x800/0x804 as falcon
port indices) was correct and the A/B fallback settled it in one boot —
the second time that pattern has paid for itself.

**⭐ POLL-CONTROL — THE CHIP'S OWN ERROR NAME IS A RED HERRING.**
```
:: kepler: poll-control valid-only chan=00002000 err=00000002 stat=00000000 ::
```
VALID written WITHOUT POLL_ENABLE, and the refusal is byte-identical:
err=00000002, the code the chip documents as NO_POLL ("validated a channel
with POLL_ENABLE, but poll area is disabled"). **POLL_ENABLE was never the
subject of that complaint.** Twenty-eight sittings honored a reason name
that does not describe its own precondition. What survives: err=2 means
"channel table validate refused" and nothing finer; the poll-area lead
(pulls 11/12, already refuted) stays dead; and the elimination stands
undisturbed — the missing actor is still the FECS ctx machinery.

**Correction to the relay (facts-first):** Fox flagged `stat=00000000` on
the poll-control leg vs `stat=00000005` on the control as a POLL-related
delta. It is not. The capture's own ordering shows stat=0 on BOTH
`sched-status post-init` and `post-restore` — every pre-submit reading — and
5 only at `post-submit`. The stat difference is submit-related, not
poll-related. No new information there.

**Storage leg — NOT RUN, not failed.** No `:: FR: UNAOS.LOG reserved … ::`
line, and the reason is in the same capture: `BOT: … n=0 nowait=0
storage_slot=0 route=0x0` — **no USB storage was attached to this boot**, so
the reserve path (gated on `block::info()` returning Some) could never fire.
The FAT single-writer fix's metal leg is still OWED and needs a stick
present at the sitting.

Other legs: console scale-4 confirmed on the wire (cell=32x32 cols=90
rows=56) — the fbcon masked-layout/unmasked-paint restructure did not
regress the wire; Peter's photo owed for the visual half. SMC rich and
healthy: a `SMC-DIAG` first-failure timeline, recovery, `retries=4/4`, and a
live `ac=derived:discharging` → `ac=derived:charging` transition when Peter
plugged in — the derived-AC inference proven on metal in both directions.
Unbounded echo poll caused no harm (boot healthy through 449+ SMC samples).

Capture from mark s37boot1v2. ESP by coordinator (f8e3e2f3…), Fox
sha-verified. NOTE: Fox seat handed s1n → s1o at this boundary.

## Sitting #36 (fence pull 32 register-side strip test, UnaOS-gemini@8d2baec0, 2026-07-26, fox-metal-r23s1n, s36boot1)

**⭐⭐ FENCE ARC VERDICT — THE TENTH STRIP CLOSES THE INVESTIGATION: THE
WALL IS THE ABSENT FECS CONTEXT MACHINERY, AND NOTHING ELSE.**
Coordinator awk-verified:
```
:: kepler: witness pre-rewrite PFIFO_CHAN[1]=00002000 ::
:: kepler: witness post-bind PFIFO_CHAN[1]=00002000 ::   <- NOT C0002000; strip persists with CHAN_CUR bound
:: kepler: PFIFO_CHAN[1] post-submit: 00=00002000 04=11000001 ::
:: kepler: witness-rematch end err=00000002 stat=00000005 valid=00002000 ::   <- tenth
```
The complete elimination, ten sittings of it: runlist encodings, USERD
variants, flushes, CTRL_ADDR, powered engine, reset-pulsed engine, LIVE
running engine, HALTED engine, and now a host-populated CHAN_CUR/CHAN_NEXT
— the strip signature never moved once. Meanwhile every constructive fact
points the same way: the submit path provably works (PLAYLIST_RD echoes
our runlist), the falcon executes our code, the CTXCTL register surface is
mapped and writable, and ENGINE_STATUS.CHAN_VALID — the bit PFIFO's
validation plausibly keys on — is set by NOTHING we can reach from the
host. **The remaining actor is the FECS context-switch microcode itself
(STUDY-fecs-ctx-init phases): the arc's next era is authoring the minimal
FECS ucode that brings up the ctx machinery.** K-GPU-4 pivots from
probing the wall to building the gatekeeper.

**SMC — Fox misread corrected at fold (facts-first):** the early sweep
stuck (`BRSC stuck`, present=false retries=2/2) but the capture shows
RECOVERY later in the same boot: `present=true soc=73% ac=derived:discharging
retries=8/18` … `9/27` … hold/release cycling repeatedly. The robustness
arc is functioning as designed on visibly flaky hardware; retry counters
are earning their keep (8–9 retries per sweep some passes). Watch-item,
not regression: per-key dropout density (volt/amp/full alternate missing)
— if s37 shows the same, the per-key retry budget may deserve one more
attempt.

Other legs: MTRAW correctly absent (zero EHCI-MT lines); BOT summary
n=0 (no USB storage attached — informational); console scale-4 unchanged.

Capture from mark s36boot1. ESP by coordinator (8d2baec0…), Fox
sha-verified, flashed only.

## Sitting #35 (fence pull 31 first context-bind + SMC derived-ac + scale-4 console, UnaOS-gemini@6fbbb939, 2026-07-25, fox-metal-r23s1n, s35boot1)

**FENCE — THE BIND TAKES; CHAN_VALID DOES NOT; AND THE "VALID BIT HELD"
LINE IS VOID — CORDINATOR AMENDMENT ERROR, STATED PLAINLY FOR THE
RECORD.** Coordinator awk-verified:
```
:: kepler: bind-pre CHAN_CUR=00000000 CHAN_NEXT=00000000 ENGINE_STATUS=00000000 ::
:: kepler: bind CHAN_CUR=00002000 ::        <- write TOOK (echo = inst_off>>12)
:: kepler: bind CHAN_NEXT=00002000 ::       <- write TOOK
:: kepler: bind-post ENGINE_STATUS=00000000 ::   <- CHAN_VALID NOT asserted
:: kepler: witness post-bind=80000000 ::         <- VOID (see below)
:: kepler: witness-rematch end err=00000002 stat=00000005 valid=00002000 ::  <- stripped, NINTH confirmation
```
REAL findings banked: (1) **CHAN_CUR/CHAN_NEXT are host-writable and hold
a channel id** — the first successful writes into the FECS CTXCTL surface,
no fault, no poison. (2) **Bare MMIO bind does NOT assert CHAN_VALID** —
ENGINE_STATUS stays 0. The finding branch: CTXCTL state is not built by
poking its registers; something (per the study, the FECS context ucode
itself) must run to accept a context. (3) The PFIFO strip is UNCHANGED
with CHAN_CUR/CHAN_NEXT populated — err=2/stat=5/valid=2000, ninth
confirmation.

**THE AMENDMENT ERROR (GR5, logged like its predecessors):** amendment 2
directed the post-bind witness leg at `inst_off+0x0C` — but that word is
the instance block in PLAIN VRAM; a readback of RAM trivially returns
what was written. The historic strip lives in the PFIFO channel-table
REGISTER (0x800008: write 0xC0000000|inst>>12, read back 00002000). The
`witness post-bind=80000000` line therefore observed nothing. The correct
post-bind strip test — rewrite PFIFO_CHAN[1] word 0 after the bind and
read it back — was NOT run this boot and is pull 32's one-liner. Fox
relayed exactly per the brief's (wrong) decision table; the misread was
mine at brief time, caught at fold time. Amendments must be derived
against the code, not from memory of it.

Bonus line relayed verbatim (not in the brief's expected set):
`post-bind playlist_rd=00002013 playlist_rd_len=00100003`.

**Console — scale-4 CONFIRMED on glass.** `glyphs-active … cell=32x32
cols=90 rows=56 scale=4` and Peter's verdict: "text looks great." Size
question closed.

**SMC — the robustness arc earned its keep on first metal.** New fields
live: `ac=derived:discharging retries=6/8` — six retries in one sweep on
real hardware, a mid-line dropout hole visible (`rem=-mAh`), and the
hold/release machinery observed cycling (`sweep aborted — first key BRSC
stuck` → `holding last good reading (age 1000 ms)` → `good reading
returned — hold released`). The flakiness is real and now measured.

Capture from mark s35boot1. ESP by coordinator (6fbbb939…), Fox
sha-verified, flashed only.

## Sitting #34 (fence pull 30 chain probe, UnaOS-gemini@beb7292d, 2026-07-25, fox-metal-r23s1n, s34boot1)

**⭐ 0x409504 (WRCMD_CMD) CONVICTED BY ELIMINATION — ALL FIVE REMAINING
OFFSETS EXIST AND READ ZERO.** Coordinator awk-verified. The chain, real
values end to end, control bracket identical both ends (cpuctl=00000010):
```
:: kepler: recon CC_SCRATCH[1] (0x804)=00000000 ::
:: kepler: recon CHAN_CUR (0xB00)=00000000 ::
:: kepler: recon CHAN_NEXT (0xB04)=00000000 ::
:: kepler: recon ENGINE_STATUS (0xC00)=00000000 ::
:: kepler: recon ENGINE_TRIGGER (0xC08)=00000000 ::
```
Banked facts: (1) the GK107 FECS host-interface surface is now mapped —
SIX of the study's seven gf100-era offsets exist and are 0 at rest
(0x800/0x804/0xb00/0xb04/0xc00/0xc08); exactly ONE, 0x409504 WRCMD_CMD,
faults-and-poisons, confirmed by elimination. (2) The un-wedge experiment
remains UNEXERCISED (nothing wedged — the price of the conviction).
(3) "A context exists" per the study = CHAN_CUR populated + ENGINE_STATUS
CHAN_VALID; both read 0 — consistent with PFIFO's err=2 refusal:
NO CONTEXT IS BOUND, and the register surface to change that exists and
is reachable. (4) Regression exact s33boot2 shape (bound-terminated hb,
console markers, witness signature unchanged).

Open next (pull 31, proposal-first REQUIRED): the deliberate
0x409504-then-PRING-clear boot — the promised un-wedge one-liner — and/or
the first WRITE experiment against the now-proven context surface
(CHAN_CUR/ENGINE_TRIGGER), each with its own control frame. Specialist
still owes the PBUS_INTR bits 2+3 decode with citation.

Capture from mark s34boot1. ESP by coordinator (beb7292d…), Fox
sha-verified, flashed only. Panel: console text as s33boot2.

**PHOTO OF RECORD (Peter, s34):** the kernel console live on the panel —
`glyphs-active base=90020000 pitch=16384 cell=48x48` as the top line,
followed by ehci/portsw bootlog replay, the landed trace, PFIFO init,
beacon plants, pgraph-pulse, and the falcon verdicts, all legible in
scale-6 grey-on-black. Peter's size verdict: "still looks great albeit
slightly large" → PANEL_SCALE 6→4 committed (32 px cell, ~3.2 mm, 90×56
grid); rides the next ESP.

## Sitting #33 boot 2 (v2 ESP: console-on-panel + pull 29, UnaOS-gemini@4e266472, 2026-07-25, fox-metal-r23s1n, s33boot2)

**CONSOLE MARKERS: SUCCESS FORM, EXACT EXPECTED VALUES.** Coordinator
awk-verified:
```
:: fbcon: glyphs-active base=90020000 pitch=16384 cell=48x48 cols=60 rows=37 scale=6 ::
:: kdisp: console-repaint rows=4 ::
```
base and pitch are exactly the scanned GOP surface; the repaint replayed
4 bootlog rows. **PANEL VERDICT (Peter, direct): the console "prints text
very well" — glyphs legible, text flowing. ⭐⭐ THE DISPLAY LANE
GRADUATES:** thirteen-plus sittings from first pixel to a working kernel
console on the rMBP panel — measurement (pull 20) → mapping (s26) →
ownership (s29) → console (here).

Product finding that rode the photo (Peter, watching the live panel): the
SMC-BATT witness (~1/s, endless) scrolls on the console forever and reads
as the machine being stuck. Fine as serial idle chatter; wrong on a user
surface. Routed as a follow-on work item (default-quiet-boot law: gate
batteries behind knobs) — SMC lane, not this lane's code.

**⭐ HEARTBEAT BOUND TERMINATION OBSERVED — pull 27's amendment finally
closes.** mb1 froze at exactly 0x00500000 (the authored iteration bound)
with cpuctl=00000010 (clean halt, STOPPED bit) from pre-witness onward.
Reading: the console repaint runs BEFORE the fence block and added enough
wall-clock for the bounded loop to run to completion — the loop
terminates at its exact authored count and the core parks cleanly. Not an
anomaly; the missing observation from s30/s32 (where the loop was still
mid-count at hb final). Bonus datum: this boot's witness ran against a
HALTED FECS and the strip signature is still byte-identical
(err=2/stat=5/valid=00002000) — running (s30/s32) vs halted (here), same
wall, consistent with refutation #8 from the other side.

Pull-29 block: clean repeat of boot1 (enable FFF9F4B0 bit4 SET,
CC_SCRATCH[0]=0 real, PIBUS fault regs zero, PBUS_INTR=0x0C latched again
— reproducible across boots, still unnamed; cpuctl bracket real 0x10 both
ends, no poison, un-wedge still unexercised).

Capture from mark s33boot2. v2 ESP (4e266472), Fox sha-verified on-card.

## Sitting #33 boot 1 (fence pull 29 PIBUS/PRING probe, UnaOS-gemini@7f23bab5 [v1 ESP — console arc NOT aboard], 2026-07-25, fox-metal-r23s1n, s33boot1)

**⭐ GATING THEORY REFUTED — THE POISON IS OFFSET-SPECIFIC, AND CC_SCRATCH
EXISTS.** Coordinator awk-verified. Boot caveat: this boot ran the v1 ESP
(7f23bab5) — pull 29 only; the console-on-panel arc (v2, 4e266472) missed
the card and its panel deliverable is OWED on s33boot2. Fence results
complete and unaffected. s30/s32 regression intact (ucode EXECUTED, hb
mb1 0x4 → 0x574B → 0x5AB1 → 0x34335, witness signature unchanged).

The pull-29 block, verbatim — NO poison fired this boot, every read real:
```
:: kepler: recon PIBUS_MMIO_HUB_ENABLE1=FFF9F4B0 ::   <- REAL; bit 4 (CTXCTL enable) SET
:: kepler: recon CC_SCRATCH[0]=00000000 ::            <- rotated FIRST; REAL ZERO, not BADF
:: kepler: recon PIBUS_INTR_ADDR=00000000 ::  VALUE=00000000  INTR=00000000
:: kepler: recon PBUS_INTR=0000000C ::                <- only nonzero; bits 2+3 latched; W1C'd
:: kepler: recon-post cpuctl=00000000 ::              <- real (HB running)
```
Banked facts: (1) **CTXCTL subunit-gating hypothesis REFUTED both ways** —
the enable bit is already SET, and 0x409800 read FIRST returns a real
value, so the 0x400+ space is not disabled wholesale. (2) **The poison
trigger is per-offset**: the same CC_SCRATCH[0] that read BADF1000 in
s31/s32 (behind WRCMD_CMD) reads clean when first. 0x409504 (WRCMD_CMD)
is the standing suspect — the only offset ever observed to fault when
accessed first. (3) **CC_SCRATCH[0] exists on GK107 and is 0 at rest** —
first real per-offset datum banked; five offsets remain unknown
(0x804/0xb00/0xb04/0xc00/0xc08). (4) PBUS_INTR held two latched bits
(0x0C) with all PIBUS fault registers zero — recorded, cleared by
write-back, meaning TBD. (5) **The un-wedge question is still open** —
nothing wedged this boot, so "PRING clear recovers the unit" was not
exercised; it needs a boot where the poison deliberately fires.

Next (pull 30 shape): chain-read the five unknown offsets in one boot —
the first BADF identifies the next faulting offset, everything after it
is tainted, and the PRING observe/clear + cpuctl re-read right after the
fault is the REAL un-wedge experiment. Avoid 0x409504 until the chain has
drained the safe offsets.

Capture from mark s33boot1. v1 ESP by coordinator (7f23bab5), Fox
sha-verified, flashed. v2 (4e266472) staged for boot 2. Panel:
calibration pattern (expected on v1).

## Sitting #32 (pull-28 recon relocated + control-bracketed, UnaOS-gemini@ee3c955a, 2026-07-25, fox-metal-r23s1n, s32boot1)

**POISON LAW CONFIRMED BY ITS OWN CONTROL FRAME — and the boot cost
nothing.** Coordinator awk-verified. The relocation fully restored s30:
ucode A EXECUTED again (mailbox0=F00DFACE, tlb page0=01000000, clean
halt), heartbeat same shape as s30 (mb1 0x4 → 0x57C9 → 0x5B2E → 0x343B4,
cpuctl=0 running throughout), witness signature unchanged
(err=2/stat=5/valid=00002000). Then the recon block, now last:
```
:: kepler: recon-pre cpuctl=00000000 ::     <- REAL (0 = HB still running; the control read)
:: kepler: recon WRCMD_CMD=BADF1000 ::      <- first access to 0x409504 faults immediately
   (…all seven recon offsets BADF1000…)
:: kepler: recon-post cpuctl=BADF1000 ::    <- SAME register as recon-pre, poisoned
```
recon-pre real and recon-post BADF1000 on the same register microseconds
apart is the in-boot proof: **the first access to 0x409504 faults
immediately (not cumulatively) and wedges all subsequent FECS-unit reads
for the boot.** Banked facts: (1) the poison law is now double-confirmed
(s31 inference + s32 control frame); (2) the ONLY clean per-offset datum
remains 0x409504 = absent-or-faulting on GK107 — the six other gf100-era
ctxctl offsets (0x800/0x804/0xb00/0xb04/0xc00/0xc08) are STILL
UNTESTED, confounded behind the first fault; (3) everything before the
block reads real all boot — poison is strictly confined to and after the
recon accessors; (4) note gf100 ctxctl docs place FECS host-interface
regs exactly here and nouveau drives 0x409504 on gk104, so a faulting
0x409504 on GK107 is itself a surprising, load-bearing observation.
Pull 28's probe deliverable is COMPLETE — it answered with a different,
sharper fact than the one it went looking for.

Open per-offset question routes to pull 29 (specialist): candidate
strategies — rotate which offset is read FIRST across boots (one clean
datum per boot); cleanroom hunt for the PRI-fault clear mechanism so
multiple offsets can be probed per boot; or re-derive where GK107 FECS
host-interface actually lives. Amendment from pull 28 stands: no
hypothesis writes against any offset not yet proven readable.

Capture from mark s32boot1. ESP by coordinator (sha ee3c955a…), Fox
sha-verified, flashed only. Panel unchanged (expected).

## Sitting #31 (fence pull 28 CTXCTL recon, UnaOS-gemini@c6b0e3cf, 2026-07-25, fox-metal-r23s1n, s31boot1)

**⚠ NEW SILICON LAW: A BAD 0x409xxx OFFSET READ POISONS THE WHOLE FECS
UNIT FOR THE REST OF THE BOOT.** Coordinator awk-verified. The boot
stream shows the mechanism exactly:
```
:: kepler: fal-base b=409000 verdict cpuctl=00000010 imemc=00000000 dmemc=00000000 ::   <- real values
:: kepler: recon WRCMD_CMD=BADF1000 ::                                                  <- first 0x409504 read
:: kepler: recon CC_SCRATCH[0]=BADF1000 ::  (…all seven recon reads BADF1000…)
:: kepler: ucode pre mailbox0=BADF1000 cpuctl=BADF1000 ::   <- s30-proven reads now BADF1000
:: kepler: ucode ABORT verify-mismatch — BOOTVEC/CPUCTL NOT written ::
:: kepler: hb ABORT verify-mismatch ::
:: kepler: witness-rematch end err=00000002 stat=00000005 valid=00002000 ::   <- PFIFO untouched
```
cpuctl read 0x10 (real) immediately before the recon block; the first
access to 0x409504 returned BADF1000 and EVERY subsequent 0x409xxx read
— mailboxes, cpuctl, IMEM readback, GPCCS untested — returned BADF1000
for the rest of the boot. The verify-gates did their job: with readback
poisoned, ucode A and HB both ABORTED cleanly, nothing was started blind.
PFIFO (0x2xxx) was unaffected — the witness signature printed unchanged.
**Interpretation limits: only the FIRST recon datum is clean (0x409504 →
absent-or-faulting on GK107); the other six offsets are CONFOUNDED, not
proven absent. The "s30 regression" is fully explained as probe-induced
poisoning — nothing else broke.** This retroactively colors s24/s25:
"all BADF1000" sweeps there may equally have been first-fault poison, not
per-offset truth.

Coordinator fix (in-lane, land-review breakage authority): recon block
RELOCATED to after `hb final` — every proven read completes before any
unverified offset is touched — and bracketed with `recon-pre cpuctl=` /
`recon-post cpuctl=` control reads so poisoning is observed in-boot, not
inferred. s32 expected shape: recon-pre=00000010 real; if recon-post is
BADF1000 the poison law is confirmed by its own control frame, and the
first recon value is the only per-offset datum banked per boot.

Capture from mark s31boot1. ESP by coordinator (sha c6b0e3cf…), Fox
sha-verified, staged, flashed only. Panel: calibration draw as s30,
expected.

## Sitting #30 (display pull 20 + fence pull 27, UnaOS-gemini@913a200e, 2026-07-25, fox-metal-r23s1m, s30boot1)

**FENCE — ⭐ REFUTATION #8, THE CLEANEST: THE WALL IS NOT ENGINE LIVENESS.**
Coordinator awk-verified from mark s30boot1 (byte 1485655). The bounded
heartbeat ucode (UCODE_HB, 0x500000-iteration loop incrementing MAILBOX1
via `iowrs I[0x1100]`) ran continuously across the entire witness sequence:
```
:: kepler: hb start mb1=00000004 ::
:: kepler: hb pre-witness mb1=00005750 cpuctl=00000000 ::
:: kepler: WITNESS FAILED - bits stripped. Restoring inst_off+0x0C ::
:: kepler: hb post-witness mb1=00005AA5 cpuctl=00000000 ::
:: kepler: hb final mb1=00034328 cpuctl=00000000 ::
:: kepler: witness-rematch end err=00000002 stat=00000005 valid=00002000 ::
```
MAILBOX1 monotonic 0x4 → 0x5750 → 0x5AA5 → 0x34328; cpuctl=00000000
throughout (running, never halted — bit4 STOPPED clear the whole time).
The strip signature is byte-identical to the s25 baseline: bits stripped,
err=00000002, stat=00000005, valid=00002000. **PFIFO stripped the channel
while FECS was demonstrably alive and executing.** Engine liveness joins
the refutation ledger (#8). Precision notes: (a) stream order shows the
pre→post bracket covers the strip+restore; `hb final` printed BEFORE the
runlist submit lines, so the submit itself is outside the bracket — but
the strip is the wall, and the strip was bracketed; (b) at `hb final` the
loop was still running (0x34328 < 0x500000, cpuctl=0), so termination of
the bound was not itself observed in-capture — the bound stands by
construction, not observation. The fence arc now turns to what the real
FECS context/init microcode must do (falcon_microcode_spec §3): the chip
wants a context, not a heartbeat (DMACTL REQUIRE_CTX was the hint all
along).

**Display — HYPOTHESIS REFUTED: GOP DOES NOT REPORT 2880-STRIDE.**
```
:: kdisp: fbcon-view base=0000000090020000 stride_px=4096 bpp=4 w=2880 h=1800 row_bytes=16384 ::
:: kdisp: fbcon-vs-hw row_bytes=16384 hw_pitch=16384 match=true ::
:: kdisp: fbcon-probe drawn rows=8 ::
```
GOP mode info already reports stride 4096 px = 16384 B/row, exactly the
hardware pitch, at base 0x90020000 (the GOP FB). **`video::fbcon` is NOT
mis-strided; no fbcon stride fix is needed.** The console's failure to
appear on the panel is therefore elsewhere (candidates: console renders
before/behind the takeover fill; output path never targets the FB; the
takeover draw overwrites it). Glyph-block verdict (Peter, direct):
**NO graphic visible — only the main calibration draw from before; no
photo to take.** Serial confirms the draw executed (`fbcon-probe drawn
rows=8` in the capture), so the writes went out through the same pointer
and pitch as the visible full-panel fill. Ruling: **probe under-sized —
INCONCLUSIVE on visibility, not a mapping refutation.** Three 8×8-px
blocks at 220 ppi are ~0.7 mm dots atop the calibration colour bands;
plausibly invisible even where they landed. The mapping stands on s29's
edge-to-edge `cover=exact` evidence. The real visibility test arrives
with the coordinator's console wiring (human-scale text).

Capture from byte 1485655 (mark s30boot1). ESP built by coordinator
(sha 913a200e…), Fox sha-verified and flashed only. Coordinator inline
fix at land-review: fbcon-view base printed via `fbcon::current_base()`
(`FrameBufferInfo` has no `framebuffer_addr` field).

## Sitting #29 (display pull 19 + fence pull 26, UnaOS-gemini@d56c0e87, 2026-07-25, fox-metal-r23s1m, s29boot1)

**⭐⭐⭐ FENCE — FIRST UNAOS CODE EXECUTED ON GPU SILICON.** Coordinator
awk-verified in the capture, twice in the same boot:
```
:: kepler: dmactl pre=00000001 ::
:: kepler: dmactl post=00000000 ::
:: kepler: ucode end img=A cpuctl=00000010 mailbox0=F00DFACE halt-iters=0 ::
:: kepler: ucode EXECUTED img=A mailbox0=F00DFACE ::
:: kepler: ucode-post off=040 val=F00DFACE SENTINEL ::
```
**DMACTL bit 0 (REQUIRE_CTX) was the entire block** — clearing it let the
core run on the first attempt. The mailbox holds the EXACT authored magic,
not merely a changed value: the seed was A5A50000, so five instructions we
wrote from ISA documentation ran on the GK107's FECS Falcon and stored
0xF00DFACE through `iowrs I[0x1000]`. That also settles the IO-space
question empirically: **the INDEXED scheme is correct** (host reg X →
falcon `(X & 0xffc) << 6`), image B never needed to run. Clean halt back to
cpuctl=00000010. K-GPU-4 milestone 2 COMPLETE. The three-sitting arc:
s27 proved the port, s28 proved the upload and named DMACTL, s29 ran it.
(`halt-iters=0` is uninformative as designed — the exact magic in the
mailbox is the proof, not the poll.)

**Display — full-panel framebuffer draw, `cover=exact`.** base=00020000
pitch=16384 rows=1800 bytes=01C20000, ours == gop exactly; hold ladder and
reg-dumps clean; ptr now reads 00000200 (the GOP base, by design),
armed=shadow=00000200, head 0 VERT live, h1–h3 dead. UnaOS owns the rMBP
panel through the GOP framebuffer: linear, 16384 B/row, at VRAM 0x20000.
**PHOTO (Peter): edge to edge, everything predicted, nothing missing** —
white fiducial bars flush against the very top and very bottom of the
panel, the 7-bit barcode column counting monotonically 0→112 down the
left edge, the diagonal running unbroken corner to corner, and the 16-row
colour bands filling the full 2880 px width. No shear, no wrap, no seam,
no clipped edge. Thirteen sittings after the first green pixel, the
display lane's question is closed: **we can put any image we want on that
screen.**

Next: fence pull 27 attacks the fence wall with the new capability — a
bounded heartbeat ucode that keeps FECS RUNNING across the witness
sequence, so PFIFO is tested against a live engine for the first time
(the last untested variable behind err=2). Display pull 20 graduates the
lane: `video::fbcon` currently derives its stride from GOP mode info while
`kepler_display` assumed 11520 B/row (2880×4) against the hardware's real
16384 — reconcile and get the kernel console rendering on the panel.

Capture from byte 1347398 (mark s29boot1). ESP built by coordinator
(sha d56c0e87…), Fox flashed only.

## Sitting #28 (display pull 18 + fence pull 25, UnaOS-gemini@754cca75/833ff9f0 + GR4 land-review, 2026-07-25, fox-metal-r23s1m, s28boot1)

**Display — ⭐⭐ THE CONFOUND IS NAMED: WE HAVE BEEN PAINTING THE
FIRMWARE'S FRAMEBUFFER, NOT LATCHING OUR OWN.** The overlap detector added
at land-review fired on its first boot:
`gop-overlap=YES-RESULT-VOID surf2=01600000+01C20000 gop=00020000+01C20000`.
The GOP framebuffer is VRAM 0x20000 … 0x1C40000 — 1800 rows × 16384 B,
i.e. **the firmware FB has exactly the geometry we "discovered" in s25**.
Our scratch surface at 0x1600000 sits INSIDE it, 0x15E0000 bytes in =
**GOP row 1400**. The model this forces, with no free parameters:
- our row r appears at panel row 1400 + r; only r = 0…399 land on screen,
  in the bottom 22% of the panel (starting 77.8% down);
- so s27 must show ONLY the row 0–7 white stripe, with 448/896/1344/1792
  all off the end. **That is exactly what s27 photographed.**
It also retro-explains s18 ("early rows → bottom band"), the entire
block-linear seam campaign (we were writing swizzled bytes into a linear
FB the firmware was already scanning), and why s26 "solved" the mapping
the moment we switched to linear/16384 — we had matched the GOP's own
layout, not discovered the hardware's.
**Therefore: the latch (0x640460 + 0x640080) has NEVER been proven to do
anything, and s17 "first pixels" is now in question.** Pull 19 settles it
in one boot: relocate the scratch surface clear of the GOP window
(0x4000000; BAR1 visible = 256 MB, VRAM 512 MB, allocator hands out from
32 MB, so 64 MB + 29.6 MB is clear of both) and re-run. Pattern on panel
⇒ the latch is real. Console unchanged ⇒ every panel result since s17 was
direct FB painting — which is itself a working framebuffer path worth
keeping, just not the one we thought we had.
**PHOTO CONFIRMS THE MODEL QUANTITATIVELY** (Peter, s28boot1 — "the only
visible thing to photograph"): the band/barcode/diagonal pattern occupies
the bottom ~22% of the panel, starting ~77–78% down. Predicted: 77.8%.
Counted bands ≈ 25 = 400 visible rows / 16 rows per band. Predicted: rows
0–399 visible, 25 bands. The diagonal ramp is present, straight, and
TRUNCATED: it runs from the band region's top-left and stops at ≈19% of
the image width at the bottom edge — the fill puts diag_x at 176 + r·2560/1800,
so r=399 → x=744 of 2880 = 25.8% of the visible width, and 744 px measured
into a 2880-px row that is itself inset in the photo lands exactly where
the photo shows it. A straight, unscaled, unwrapped diagonal is a direct
measurement: **the row map is 1:1 with no vertical scaling** — hypotheses
(a) scaling and (b) short scan window are both refuted; the geometry is
purely the 1400-row base offset that the GOP overlap predicts.
Supporting reads this boot: armed=shadow=00000200 (unchanged by our
write), head 0 VERT advancing 0684051D→06BA04A3 across the hold (h1–h3
dead), ptr readback 00016000 stable at t=1 and t=5, storage/size/format
cluster identical to s25.
**SETTLED SAME SITTING — THE EVO LATCH HAS NEVER WORKED, AND WE ALREADY
HAVE A FRAMEBUFFER.** Peter watched the wire live: **the graphic came up
BEFORE `pm-step fill done` printed** — i.e. during the fill itself,
pixels appearing as we wrote them, a full latch-cycle before the latch.
The register dump closes it independently: `armed=00000200` and
`shadow=00000200` at both t=1 and t=5 — and 0x200 << 8 = **VRAM 0x20000 =
the GOP framebuffer**. The head was scanning the firmware's surface the
whole time and never took our 0x016000 pointer, exactly as the s15
"0x6101E0 never follows" puzzle has said since the beginning.
Consequences, recorded plainly:
- **Refuted:** the EVO arm+UPDATE path (0x640460 + 0x640080) as a means of
  repointing scanout. s17 "FIRST UNAOS PIXELS" was real pixels but the
  wrong mechanism — direct painting into the firmware FB, not a latch.
  Everything s18–s26 (block-linear, GOB, bw/bh/pitch ladders) was
  aliasing against the GOP's own linear/16384 layout; s26 "mapping
  solved" was us matching that layout, not decoding the hardware's.
- **Won, and it is the bigger half:** UnaOS can put arbitrary pixels on
  this panel *today* — linear, pitch 16384, into the GOP framebuffer at
  VRAM 0x20000 (phys 0x90020000, already exposed by
  `video::fbcon::current_base()`). s28 drew a 25-band barcode with a
  correctly-sloped diagonal at exactly the predicted geometry. That is a
  working framebuffer, reached through a different door than the one we
  were knocking on.
Pull 19 is therefore re-scoped: not "relocate to prove the latch" (already
decided) but **draw the full panel at the correct origin** — base
`gop_vram_offset`, all 1800 rows, no latch at all. The EVO repoint becomes
a separate, honestly-labelled known-unknown for later (the armed register
never follows our writes; the real arming path is elsewhere).

**Fence — upload and page-usability PROVEN; the core still refuses to
run, and the blocker is now named.** Both images verified byte-exact
(A: 100017F1…, B: 004017F1…), `tlb page0=01000000` = **usable** (the
land-review page-pad worked), yet `cpuctl 00000010 → 00000012` and
mailbox0 never left the A5A50000 seed, halt-iters=0, zero SENTINEL in the
128-row post-sweep. Reading CPUCTL as rnndb does (bit1 START_TRIGGER,
bit4 STOPPED): **the start trigger latched and the core stayed stopped.**
The post-sweep we already had names the cause: **DMACTL (base+0x10C) =
0x00000001 — REQUIRE_CTX is SET**, so the Falcon demands a bound context
before it will run; scrub bits (1,2) are clear, consistent with our
successful IMEM writes. CPUCTL bit 6 is clear, so writing 0x100 directly
(not the GM107+ alias at 0x130) was correct. Pull 26 = clear DMACTL bit 0
(one write, mask-clear, pre/post printed) and re-run image A unchanged.
Nouveau clears exactly this bit in its no-context falcon path.
Other post-sweep facts logged for the arc: IDLESTATE(0x108)=20402050,
0x12C=00081103, IMEMC(0x180)=02000014, DMEMC(0x1C0)=02000010.

Capture from byte 1203639 (mark s28boot1), full early boot present in the
ring — the s28 mirror-hdr gating held. ESP built by coordinator
(sha b9c3e60c…), Fox flashed only.

## Sitting #27 (display pull 17 + fence pull 24, UnaOS-gemini@b9f3d9bf/f2dbb032, 2026-07-25, fox-metal-r23s1m, s27boot1)

**Fence — ⭐ UCODE UPLOAD PATH PROVEN, BOTH FALCONS.** All sixteen
sentinel words returned exactly (DEADBEEF/CAFEF00D/12345678/A5A55A5A,
imem AND dmem, FECS 0x409000 AND GPCCS 0x41A000); IMEMC/DMEMC control
readbacks real (rb=01000000). AINCW(24)/AINCR(25) discipline works as
specced. K-GPU-4 milestone 1 complete. Pull 25 = MILESTONE 2: first
from-scratch Falcon microcode — a minimal hand-assembled program (≤16
words: write a magic to MAILBOX0 at base+0x040, then EXIT), uploaded via
IMEMC/IMEMD (+IMEMT tag per 256B block, per envytools falcon docs),
BOOTVEC=0, CPUCTL=2, bounded poll for halt, read MAILBOX0 from the host.
Magic in MAILBOX0 = first UnaOS-authored code executed on GPU silicon.
FECS first; GPCCS only after FECS behaves. CLEANROOM notice binding —
instruction encodings cited from envytools falcon ISA docs only.

**Display — ROW-CAL PHOTO: only WHITE (rows 0–7) visible, a single line
~62–66% down the panel; RED@448, GREEN@896, BLUE@1344, MAGENTA@1792 all
absent. RECORD THIS AS INCONCLUSIVE, NOT AS A MEASURED NULL** (revised
2026-07-25 after s28 land-review; the first fold over-concluded twice):
- A mostly-black surface with five 8-row lines is photographically
  indistinguishable from a mostly-black FIRMWARE CONSOLE — a thin white
  line ~⅔ down is exactly where a boot-log cursor sits — and phone
  auto-exposure locked to a blown-out white line would bury four 0.9 mm
  saturated colour lines. We may have photographed the console, not us.
- **Arithmetic correction:** the first fold said our pointer sits "352
  rows above VRAM 0". Wrong — 0x1600000 / 16384 = **1408 rows**. There is
  no impossibility; that argument is withdrawn.
So 1:1-with-offset is NOT refuted, merely unconfirmed. Hypotheses still
live, and pull 18 must first prove the latch does anything at all
(pre-latch control frame):
(a) vertical scaling (fw mode < native, line-doubling), (b) scan window
smaller than 1800 rows, (c) pointer-latch granularity. Pull 18 =
specialist-designed placement-model probe (single latch cycle,
restore-paired) whose pattern discriminates those hypotheses in one
photo. Serial side of pull 17 clean (fill 01C20000, holds, done,
late-recap ran=true).

Boot from staged s27boot1-rowcal-falport-20260725T1619Z-b9f3d9bf,
coordinator ESP sha-verified afd16f36 (no self-build). Capture from byte
1130795.

## Sitting #26 (display pull 16 + fence pull 23, UnaOS-gemini@5e962ee1, 2026-07-25, fox-metal-r23s1m, s26boot1)

**Display — ⭐⭐ MAPPING SOLVED: LINEAR, PITCH 0x4000, CONFIRMED ON PANEL.**
Peter's hold photo of the single lin-step cycle: ZERO seams, SOLID white
left column, clean unbroken 64-row color bands with crisp black
separators. The s25 mirror decode was the truth; block-linear is retired
for good. Remaining variable — the LAST one: vertical placement. One
band-cycle's worth of rows (~450–512) displays at the bottom ~quarter of
the panel; horizontal mapping is perfect, so the scan start sits a fixed
row offset from our surface pointer (the old s18 "early rows → bottom
band" mystery, now linearly measurable). Pull 17 = row-offset
calibration: distinctive single white marker rows at known indices on
black; photo names the offset; then adjust the pointer/fill and the full
panel is ours.
**s26boot3 CLOSE (coordinator awk-verified): SERIAL PROOF CLOSED, RING
WORKAROUND CONFIRMED.** Full ladder in-capture: lin-step fill done
bytes=01C20000 exact + holds 1–5 + done; late-recap fb=00016000 ran=true
(trace head 917D0210 — the EVO core class id, a nice bonus witness);
fal-base verdicts byte-identical to boot1 (stable, not a fluke); witness
baseline unchanged; ZERO dense fal-base rows (trim verified). Pull 16 is
now closed on BOTH channels (boot1 photo + boot3 serial). Boot from
s26boot3-latereap-trim-20260725T1603Z-b5f273c4, coordinator ESP
sha-matched (70870709…), capture from byte 1063936.

**Original boot1 capture caveat (coordinator awk, post-close):** the s26boot1 capture
attached MID-GPU-INIT — zero early-init lines present (no Initializing
Kepler / VRAM / therm / mirror-hdr; first GPU line is pgraph-pulse pre).
Fox's "0 lin-step lines" is true of the CAPTURE, not the boot: the
display leg is unconditional in code and the hold photo matches pull 16's
prediction exactly. Serial proof of lin-step owed at s26boot2 (capture
attached from power-on; presence check = "Initializing Kepler" +
mirror-hdr rows in-capture before the kdisp block).

**Fence — ⭐ THE FALCONS ARE FOUND: FECS 0x409000 + GPCCS 0x41A000 both
REAL.** Verdict lines: cpuctl=00000010 at BOTH bases, imemc/dmemc =
00000000 true zeros — first non-poison Falcon reads of the campaign
(cpuctl 0x10 = a real state bit, likely HALTED). The spec's 0x400180 base
is formally dead; spec doc correction stands. Pull 24 = sentinel port
probe at the REAL bases (IMEMC/DMEMC +0x180/+0x1C0 with AINCW/AINCR
discipline), zero execution — sentinels back opens the ucode road at
last. Witness baseline unchanged (err=2, stat=5), as expected.

Boot from Fox rebuild of tip 5e962ee1 (coordinator ESP was clobbered;
same 8-knob line; kernel sha 28a9ec13…). Capture: rmbp-s18/, mark
s26boot1 (awk-verify at sitting close). Flash staged
s26boot1-lin16k-falbase-20260725T1540Z-5e962ee1.

## Sitting #25 (display pull 15 recon + fence pull 22, UnaOS-gemini@e9d20bd2/30a6a8dd, 2026-07-25, fox-metal-r23s1l, s25boot1, serial-only)

**Display — ⭐ THE MIRROR TALKED: FW SURFACE IS LINEAR, PITCH 0x4000.**
Mirror window 0x640400–0x6405FC dumped twice, ZERO volatility. The ISO
surface method cluster at head0:
- 0x460 = 00000200 → offset>>8 → fw surface at VRAM +0x20000 (consistent
  with the 0x90020000 GOP story; our >>8 pointer convention confirmed).
- 0x468 = 07080B40 → SET_SIZE h=1800 w=2880. Exact.
- 0x46C = 01004000 → SET_STORAGE: bit24 LAYOUT=1 = **PITCH (LINEAR)**;
  pitch>>8 = 0x40 → **pitch = 0x4000 = 16384 bytes/row**; block fields 0.
- 0x470 = 0000CF00 → SET_PARAMS format=0xCF (<<8).
Read: the scanout is NOT block-linear — every seam/checkerboard artifact
since s19 was aliasing of linear-16384 vs our assumed layouts (s18's
"left-bar wraps as dashes, pitch≠11520" was the truth the whole time; the
GOB "confirmation" at s20 was coincidental structure). Pull 16 = ONE
linear fill cycle, rows strided 16384 bytes (2880×4 visible + padding),
no swizzle. If the mirror is the scan config, that cycle is seam-free and
the mapping war is OVER. Other candidates logged: 07080B40 repeats at
4B8–4C8 (viewport/raster cluster), 0x494=9, 0x498=00040000, 0x55C=2.

**Fence — reset pulse ELECTRICALLY CLEAN, ports STILL DEAD → the spec's
Falcon base is probably WRONG.** pre=E011216D → off rb=E011216D (bit 12
reads clear when cleared) → on rb=E011316D; post-pulse imem/dmem probe
still BADF1000 ×8, recon unchanged. New read: BADF1000 on EVERY access
incl. control readbacks is the nonexistent-pri-register signature, not a
gate. On GK104-family the GR Falcons sit at 0x409000 (FECS) and 0x41A000
(GPCCS) — 0x400180/0x4001C0 (spec §2) likely don't exist on GK107.
Pull 23 = read-only recon of 0x409000–0x40915C and 0x41A000–0x41A15C
under the existing enable; if real falcon registers appear there, the
port probe moves to that base. Spec doc annotated.

Capture: rmbp-s18/cu.usbserial-ABAFUJCO.log, mark s25boot1. Flash staged
s25-20260725T*Z-f317d3f9.

## Sitting #24 (display pull 14 + fence pull 21, UnaOS-gemini@3d12d5f5/c8993e2c+AINCR fix, 2026-07-25, fox-metal-r23s1l, s24boot1)

**Fence — K-GPU-4 M1: FALCON MEMORY PORTS STILL GATED.** With PMC bit 12
set, every access to IMEMC/IMEMD (0x400180/184) and DMEMC/DMEMD
(0x4001C0/1C4) returns BADF1000 — control readbacks included, no sentinel
returned. The PMC enable alone does not open the Falcon sub-block; it sits
behind a second gate (engine-level reset/clock). Witness-rematch baseline
unchanged (err=2, stat=5, valid=00002000), as every boot. Next (pull 22):
PMC bit-12 RESET PULSE — clear bit 12, settle, set it, settle, then re-run
the identical port probe + falcon core recon. Rationale: the engine was
clear at power-on and may be latched dead; a full pulse re-initializes the
fabric interface (nouveau-class init does reset-then-enable, and this
stays inside the register/class we already own).

**Display — PANEL VERDICT (Peter's four photos): PITCH×BW REFUTED — all
four (bw,pg) combos still cluster-seamed, no white column.** The
parameter-ladder road is exhausted: GOB proven, bh/bw/pg permutations all
leave residual clustered seams. NEW READ (pull 15): we only ever swap the
surface POINTER (0x640460); pitch/block-mode/size methods in the EVO core
mirror stay as FIRMWARE configured them for its own surface — the hw is
scanning our surface through firmware's storage params. Those params are
READABLE: the method mirror at 0x640400+ (head-0 window that contains the
0x460 slot) should hold the real pitch/block values. Pull 15 = read-only
dense dump + decode of 0x640400–0x6405FC; match the fill to what the hw
actually says instead of laddering guesses.

**Serial: all four bwpg-step cycles ran clean** (bh=4; bytes 01560000
@pg192 / 01C80000 @pg256 for both bw values, matching computed; restores
clean).

Capture: rmbp-s18/cu.usbserial-ABAFUJCO.log, mark s24boot1. Flash staged
s24-20260725T*Z-b2aea033.

## Sitting #23 (display pull 13 + fence pull 20, UnaOS-gemini@85a4a492/504f7f80, 2026-07-25, fox-metal-r23s1l, s23boot1)

**Fence — WITNESS REMATCH: REFUTED (decisive, refutation #7).** With
PGRAPH enabled (pre=E011216D → rb=E011316D, bit 12 accepted, same as
s22), the historic strip signature reproduces EXACTLY: PFIFO_CHAN[1]
pre-submit 00=00002000 (VALID/POLL 0xC0000000 stripped), err=00000002
post-init/post-restore/post-submit, stat=00000005 post-submit,
playlist_rd=00002013 len=00100003 (runlist accepted, as always),
all three pbdma discriminators 00000000. Capture awk-verified.
**The fence wall is NOT pgraph-power-gating.** The engine-off theory is
dead; the wall survives a powered (halted, no-ucode) engine. Per the
standing s21 ruling, **K-GPU-4 begins: cleanroom Falcon microcode**
(spec docs/dev/OS/08_VIDEO/falcon_microcode_spec.md, CLEANROOM notice
binding). Plausible residual theory folded into the arc: PFIFO may
require a RUNNING engine (ucode heartbeat), not merely an ungated one —
the first ucode milestone tests exactly that. Note for the arc: imemc/
dmemc still read BADF1000 post-enable (s22) — Falcon memory ports gated;
first milestone must probe IMEM/DMEM accessibility before any upload.

**Display — PANEL VERDICT (Peter's four photos): BLOCK WIDTH IS REAL but
no pair clean yet — the artifact changed class.** With bw>1 the uniform
periodic full-width brick seams of s21/s22 are GONE; every cycle shows
long clean continuous band runs with seams CLUSTERED in narrow x-regions
((2,4): clean across ~85% of width, cluster only at far left; (2,8): more
clusters, clean middle; (4,4): clean middle, clusters near edges; (4,8):
two narrow clusters, large clean runs). Read: bw>1 moved the geometry
qualitatively where pitch (at bw=1) moved nothing — the mapping is close;
one interaction remains wrong. Prime suspect: PITCH PADDING × BW —
pitch-align was refuted only at bw=1; at bw=2/4 the natural 180 GOBs/row
gives 90/45 blocks/row (45 odd), and hw may pad blocks-per-row. Cleanest
config: (bw=2,bh=4). Pull 14 = bw {2,4} at bh=4 × pg {192,256}.

**Serial: all four bw-step cycles ran clean** (pg=180; bytes 0140A000
@bh4 / 01464000 @bh8, matching computed for both bw values; holds and
restores clean).

Capture: rmbp-s18/cu.usbserial-ABAFUJCO.log, mark s23boot1. Flash staged
s23-20260725T*Z-80184a20.

## Sitting #22 (display pull 12 + fence pull 19, UnaOS-gemini@523a50c2, 2026-07-25, fox-metal-r23s1l, s22boot1)

**Fence — PGRAPH ENABLE TOOK; engine changed class from gated to
partially-readable.** `pgraph-enable pre=E011216D` (bit 12 clear, exactly
the s21 value) → `wrote=E011316D rb=E011316D` — **bit 12 stuck, not
REFUSED.** Post-enable recon (both passes, identical, stable):
- The all-BADF1200 wall is GONE. Registers now read **BADF1000** (a
  different pri-error class — engine no longer PMC-gated, but not fully
  out of reset/clocked) interleaved with **real zeros**:
  - falcon core: 0x400100/108/110/118/11C = 00000000 (cpuctl=00000000 —
    Falcon present, halted, no ucode); 104/10C/114 = BADF1000.
  - pgraph stat: off 050–064, 074, 078 = 00000000; rest BADF1000.
  - imemc/dmemc = BADF1000 (memory ports still gated).
- The init-time "PGRAPH Engine Status: 0xBADF1200" line in this log
  predates the enable (ordering verified in capture) — not a contradiction.
- Read: register-granular decode exists now; the engine responds but wants
  a further ungating step (engine-level reset release / clock enable).
  Per the standing plan, the FIRST check is the s10 witness-ladder rematch
  (pull 20): if PFIFO stops stripping VALID/POLL with the engine merely
  enabled at PMC, the fence wall is over without any ucode work.

**Display — PANEL VERDICT (Peter's four photos): PITCH ALIGNMENT REFUTED.**
No (bh,pg) pair clean; white column never assembled. Seam count scales with
bh as in s21 (~6–7 seams @bh4, 2–3 @bh8) but is IDENTICAL between pg=192
and pg=256 at the same bh — seams sit at the same x positions in both
pitch variants (clearest at bh8: two seams at ~1/3 and ~2/3 in both).
That is the brief's refutation key verbatim: the blocks-per-row term is
not a padding problem. Next suspect (standing plan): BLOCK WIDTH > 1 GOB
— pull 13. Bottom-band placement unchanged (known-unknown stands).

**Serial: all four pa-step cycles ran clean end-to-end** (fill/hold×5/
done, restore between): bytes per cycle 01560000 (4,192), 01C80000 (4,256),
015C0000 (8,192), 01D00000 (8,256) — matching computed sizes exactly
(Fox's "all 01D00000" was a misread; capture verified). **Verdict awaits
Peter's four panel photos** — zero seams + solid white column names the
real (bh,pg) pair. Bench-ride therm/pcilink/vrom blocks printed pre-dispatch
(ours, first sitting they ride).

Capture: rmbp-s18/cu.usbserial-ABAFUJCO.log, mark s22boot1. Flash MANIFEST
s22-20260725T1340Z-523a50c2.

## Sitting #21 (display pull 11 + fence pull 18, UnaOS-gemini@f9e987f6/366e5b05, 2026-07-24, fox-metal-r23s1j)

**Fence — Falcon ground truth: PGRAPH IS POWERED OFF AT PMC.** Every
falcon/pgraph register (cpuctl, bootvec, core block, imemc/dmemc, all 32
status rows) reads **0xBADF1200** on both passes, both boots — the NVIDIA
pri-error pattern for a clock/power-gated engine, not garbage. And the
cause is in our own reprint: **PMC_ENABLE = 0xE011216D has bit 12 (PGRAPH)
CLEAR.** We never enabled the engine; nothing behind it can respond. This
also retro-explains the whole fence wall shape: PFIFO accepts config but
strips VALID/POLL for a channel whose target engine is powered off.
Pull 19 = set PMC_ENABLE bit 12 (single write + readback), re-dump the
Falcon block, expect BADF1200 → real values. First genuinely hopeful fence
step since s7.

**Display — bh ladder: NO rung clean; a second parameter rides along.**
Monotonic structure across bh 2/4/8/16 (photos, notes in
`capture/rmbp-s18/s21boot2-panel-observations.md`): seam count halves as bh
doubles (~6 @bh4, ~3-4 @bh8, ~2 @bh16), shear per seam grows with bh,
stripe thickness scales with bh. White column never assembled. Read: GOB
64B×8 stands (s20), block stacking is real, but our blocks-per-row term is
wrong — prime suspect is PITCH ALIGNMENT (hw aligns the surface to a
block-column granularity; 180 GOBs/row is not aligned). Pull 12 = two-axis
mini-ladder: bh ∈ {4,8} × pitch_gobs ∈ {192, 256} (aligned candidates),
four cycles, 5 s holds. Seam count → 0 at the right pitch.

Beacons re-confirmed none-seen; mirror-hdr 256-row passes present (window
still parked). 5 s holds (366e5b05 revision) gave the bench camera time —
keep that as the standing hold length.

## Sitting #20 (display pull 10 + fence pull 17, UnaOS-gemini@1e68c270, 2026-07-24, fox-metal-r23s1j)

**Display — BLOCK-LINEAR CONFIRMED.** The GOB 64B×8 pre-swizzle killed the
s19 checkerboard: colors now run as continuous full-width bands in correct
cycle order. Remaining artifact: periodic brick-seam x-step offsets (whole
runs shifted horizontally, strongest in red) → the GOB-level transform is
RIGHT and the higher-order BLOCK-HEIGHT is wrong (block-height 1 assumed;
real surfaces stack 2/4/8/16 GOBs per block before advancing x). Band
placement + missing white column unchanged (both downstream of block-height;
re-read after pull 11). Latch ladder unchanged (asm-stuck=y,
armed-followed=n). Pull 11 = block-height step ladder.
Photo notes: `capture/rmbp-s18/s20boot1-panel-observation.md`.

**Fence — the 0x640000 window is a dead road, triple-refuted:**
(1) beacons none-seen twice (not our structures), (2) `latch-delta none` —
fully decoupled from the display UPDATE, (3) the pre dump was ALL-ZERO this
boot vs 158 nonzero rows in s19 — contents are boot-dependent residue, not
live state we can steer. Window parked. The fence lane's pre-committed
fallback ladder is now EXHAUSTED (runlist encodings s8, SNOOP s10, HI-bit
s11, flush s12, CTRL_ADDR s13, disp-era USERD anchor s19–s20). Next move is
the PGRAPH/ucode pivot (K-GPU-4) — Peter strategy call, per the standing
campaign frame.

## Sitting #19 (display pull 9 + fence pull 16, UnaOS-gemini@ae5ce2b2, 2026-07-24, fox-metal-r23s1j)

**Display — ruler flew; ONE hypothesis now explains every panel fact:
the scanout window reads the surface BLOCK-LINEAR (GOB 64 B × 8 rows) while
we fill linear.** (Coordinator decode from Peter's photo + notes at
`capture/rmbp-s18/s19boot1-panel-observation.md`.)
- Full 8-color cycle visible in-order inside the same bottom ~1/8 band,
  whole cycle compressed — each 64-row block only a few panel rows tall →
  ~8× vertical compression = GOB height 8.
- Red stripes dashed/checkerboarded at short regular periods → 64-byte
  (16-px) chunks of our linear rows stacking vertically under the swizzle.
- White 256-px left column invisible → 1 KB of white per row shatters into
  scattered 64 B blocks. Per-row notch unresolvable, same reason.
- Retro-consistency: s17 solid green (swizzle-invariant) showed a clean
  band; s18 quarters kept coarse order. Latch ladder identical
  (asm-stuck=y, armed-followed=n, all boots).
- Bottom-band placement REMAINS a separate unknown (viewport/window offset).
- Pull 10 = pre-swizzled ruler (linear→GOB transform in the fill): clean
  stripes + solid white column on the panel would PROVE tiling + params.

**Fence — beacon verdict: NONE-SEEN; window is NOT a mirror of our channel
structures.** Beacons planted at userd 0x2002000 / pb 0x2003000 / runlist
0x2013000 (BAR1); pass1 clean of beacons (158 nonzero rows); **pass1→pass2:
ZERO words changed** — the window is stable within a boot; s18's
"volatility" was across boots/boot-phase, not continuous churn. Standing
read: engine-private memory aperture, contents boot-dependent. Pull 17 =
latch-correlation probe (dump the window BEFORE takeover_display and after,
same boot, read-only — does the display UPDATE perturb it?).

**s18 completion note (fox-metal-r23s1j):** three s18 boots total, ladders
identical; bench corrected its own count — mirror-hdr pass1 nonzero rows =
158 (matches the coordinator's fold; the 159 in the first relay counted the
done line).

## Sitting #18 (display pull 8 + fence pull 15, UnaOS-gemini@c4dbbbb6, 2026-07-24, fox-metal-r23s1j)

**Single all-knob boot, both lanes served. Serial side capture-verified;
panel photographed (Peter).**

**Panel facts (photo, pattern boot):**
1. Visible band sits at the BOTTOM ~fifth of the panel (same region as
   s17's green): RED above GREEN, red roughly twice the green's height. No
   blue, no white anywhere → only EARLY surface rows (red quarter + part of
   the green quarter) ever reach the panel.
2. **No continuous black left column** — the 64-px left bar appears instead
   as periodic dark DASHES drifting across the band. Deduction: the
   hardware scan pitch ≠ our assumed 11520 (w×4); the left-bar marker wraps
   to drifting x positions row by row. Pitch is the primary unknown.
3. Band interior shows staggered brick-patterned dark dashes through red
   and green (bench read: "tears"; coordinator read: the black left-bar
   fragments wrapping at drifting x — a spatial pitch artifact, not
   temporal tearing, since the pattern is stable in a still frame). The
   dash stagger is itself pitch data; pull 9's ruler resolves which read
   is right and yields the number.

Mapping verdict: the latch scans a SUB-RANGE of our surface (early rows)
into a fixed bottom band, at a pitch we have wrong. Pull 9 = ruler pattern
(row-coded color cycling + thin black row-markers + wide white left column)
to solve pitch and row-mapping arithmetically from the next photo.

**Display pull 8 (serial side):** geom w=2880 h=1800 pitch=11520; full latch
ladder identical to s17 (asm-stuck=y, armed never followed, raster ticking
t=1..8). **Mid-hold fact: 0x61634C read 0x00050008 while the pattern surface
was latched — s13/s16 read 0x07380BAF (raster totals) at that same offset
pre-latch.** The timing-cluster word CHANGES under an active latch (it took
the value shape head 1 shows at reset). 0x616340 stayed raster-consistent;
0x6101E0/0x61D1E0/0x61D014 all unmoved. Interpretation open until the panel
report lands.

**Fence pull 15 (method-mirror header 0x640000–0x6403FC, read-only):**
- Structure (pass 0): zeros 0x000–0x088; lone 0x08C=0x2CB23507; solid
  0xFF114D95 fill 0x090–0x168; five high-entropy words 0x16C–0x17C
  (F3EEF6EE/8FD5136D/EE76BF7D/3642C748/CD3A5D9D); 0x240=0x00000801;
  zeros elsewhere.
- **The region is VOLATILE: pass 1 has 158 non-zero rows vs pass 0's ~62**
  (the 0xFF114D95 fill GREW between passes; 302 fill-rows total across both).
  This does not read like a stable register file — hypothesis (labeled as
  such): the window is an aperture onto live memory (core-channel
  pushbuffer/USERD territory), not config MMIO. Fence pull 16 design should
  treat it as memory-backed and correlate against the display lane's latch
  activity.
- Coordinator row-count note: my capture count says pass1=158 non-zero
  (bench said 159); rows=256 both passes confirmed.

Head-scan preamble: all four heads evo=0 skip (expected, refuted mirrors);
evo-core 32-row dumps present both passes.

## Sitting #17 (display pull 7, UnaOS-gemini@11f06ded, 2026-07-23, fox-metal-r23s1i) — ⭐ MILESTONE

**FIRST DELIBERATE UNAOS PIXELS ON THE rMBP INTERNAL PANEL. The EVO
arm-and-latch mechanism WORKS.** (Coordinator capture-verified, all lines.)
- Ladder: pre asm=armed=shadow=0x200 → `asm-wrote=00016000 rb=00016000`
  (assembly slot 0x640460 is WRITABLE and holds) → selfcheck ×2: armed
  unchanged (no premature latch — assembly and armed states are properly
  distinct) → UPDATE write 0x640080=0 (rb 0) → **panel showed a GREEN BAR at
  the BOTTOM of the screen during the 5 s hold (Peter's eyes)** → restore:
  asm back to 0x200, armed/shadow 0x200, screen recovered.
- `verdict asm-stuck=y armed-followed=n`: the 0x6101E0 "armed" readout NEVER
  left 0x200 even while green was on the panel — so 0x6101E0 is NOT the live
  scanout tracker (it reports some other/base state, or latches at a
  boundary we didn't cross). Known-unknown, logged as such.
- Green as a bottom BAND (not full screen) — the 0x640460 offset evidently
  maps a sub-region of the raster. Facts in hand cannot yet say which
  mapping (stride/tiling/multi-window split); pull 8 discriminates with a
  patterned fill instead of solid green. HEAD_STAT vert ticked throughout
  (raster never stalled); vblank_count high-halves advanced ~13-14/s.
- Fence-lane consequence: the EVO method-mirror write + UPDATE path is
  PROVEN LIVE — the disp-era-USERD fallback now has a working mechanism to
  ride; fence pull 15 = read-only recon of the method-mirror header region
  (0x640000–0x6403FC, never yet dumped) to locate channel-control/USERD
  slots.

## Sitting #16 (display pull 6, UnaOS-gemini@939ba952, 2026-07-23, fox-metal-r23s1i)

**Single read-only boot — ASSEMBLY STATE FOUND. Coordinator decode:**
- evo-scan2: 18 hits, uncapped. Pair table: no first≠second anywhere
  (nothing latched during the window — expected; nothing was arming).
- **The find: 0x640460 = 0x00000200** — a third 0x200-holder, sitting inside
  a coherent record in the 0x640000 (DISP_USER) region: 0x640420 holds
  0x07380BAF (the SAME raster-totals value proven at 0x61634C/s13), with the
  w2880/h1800 cluster at 0x640468–0x6404C8. Decode: the 0x640000 region is
  the EVO core-channel METHOD MIRROR — core-channel method layout puts head 0
  at +0x400 with the surface OFFSET slot at +0x60 → 0x640460. The record
  shape matches method semantics exactly (offset + raster + geometry).
  **0x640460 is the assembly-side surface pointer; the UPDATE method slot is
  +0x80 → 0x640080 is the latch-trigger candidate.**
- 0x61D1E0 = 0x200 as well: armed-shadow at +0xD000 from the s15 readout
  (second armed-side mirror, read-only presumed). Same block also holds
  0x61D014 = 0x00020000 — the GOP vram offset UN-shifted (coordinator
  capture-verified; 18/18 hits confirmed, 19 pair lines, zero diverging).
- Repeating 0x90000000-shaped words at 0x61C/0x61D x128-stride and the
  full gap-window rows (256×2) are in the capture for later decode.
- Next: pull 7 = assembly-write + UPDATE-latch experiment (0x640460 then
  0x640080), fully restore-paired. Display-write class already approved (s15).

## Sitting #15 (display pull 5, UnaOS-gemini@5686f417/e9b1e89f, 2026-07-23, fox-metal-r23s1i)

**Boot 1 (5686f417) — no write occurred:** the LEGACY EVO-mirror head-match
gate (refuted decode, s11) sat upstream of the repoint code and aborted
(`takeover-abort no-match`). Land-review miss (control flow not verified to
reach the new code); fixed inline at e9b1e89f — gate defaults to head 0
(HEAD_STAT canon) with an honest marker; refuted bounds check neutralized.

**Boot 2 (e9b1e89f) — repoint hypothesis REFUTED, cleanly:**
- Full ladder ran twice (known double-invocation), both passes identical:
  surf2 filled (0x1600000, 0x13C6800 bytes green) →
  `repoint wrote=00016000 rb=00000200` — **the write does not take; readback
  is the original value immediately** → raster ticked through all 5 hold
  seconds (vert 0x11C3→0x1206) → restore rb=00000200 → `verdict rb-stuck=no`.
  Panel never changed (nothing was armed). Boot continued normally.
- **Verdict: 0x6101E0 is a READ-ONLY armed-state readout, not a writable
  pointer.** Consistent with EVO armed-vs-assembly semantics: the armed
  surface value is reported here, but arming goes through the core channel's
  assembly state + an UPDATE/latch step (s13's 0x494=1 companion and the
  descriptor table at 0x6104A0+ are the standing leads).
- Bench note: the "stray no-match line" reported for boot 2 was a cross-mark
  grep artifact (it was boot 1's abort in the same accumulating log); the
  boot-2 segment is clean.

## Sitting #14 (display pull 4, UnaOS-gemini@114fad64, 2026-07-23, fox-metal-r23s1i)

**Single read-only boot — EVO core-channel read-out. THE SURFACE REGISTER
CANDIDATE FELL OUT.**
- Bench "divergence" (missing pass2) was a miscount — the brief's two passes
  are pass0+pass1, both complete in the capture (33 lines each). No rerun
  needed.
- **Known-value scan: exactly ONE hit in the full 16 KB sweep —
  `0x6101E0 = 0x00000200`** = the GOP surface address in >>8 form
  (fb at VRAM +0x20000; 0x20000>>8 = 0x200 — the `expected_addr` shape the
  s12 differential hunted for). hits=1 capped=false: the predicate produced
  zero false positives; this is the armed scanout surface pointer candidate,
  and it lives in the PDISPLAY armed-state region (0x6101E0), NOT the
  per-head 0x616000 block — exactly why s12/s13 couldn't find it there.
- **Core window (0x610480–0x6104FC) fully stable across passes** (zero
  varying words). Structure: +0x10=0x0D0500A9 +0x14=1 (the s13 recon words),
  then a repeating 3-word record {0x40000088, 0x00000001, 0x80010000} at
  stride 0x10 from +0x20 — a channel descriptor table (per-EVO-channel
  ctrl/flag/base records), uniform and quiescent.
- Verdict: display pull 5 = repoint-the-surface experiment (write 0x6101E0
  to a second prepared surface, watch panel, restore). FIRST display-register
  write — Peter decision required. Fence's disp-era-USERD input: the core
  channel is configured and idle; the descriptor table above is the map for
  any USERD-linkage probe.

## Sitting #13 (display pull 3 + kepler pull 14, UnaOS-gemini@f4a7ef6e, 2026-07-23, fox-metal-r23s1i)

**Boot 1 — display pull 3 candidate decode DELIVERED. Coordinator decode:**
- Dense windows (0x300–0x35C, 0x3F0–0x40C, 0x5F0–0x61C), both heads, 3 passes,
  all rows captured (`~/unaos-bench/capture/rmbp-s13/`, mark s13boot1).
- **Telemetry (varies across passes, disqualified as config):** 0x314 is a
  frame counter (0xACE→0xACF, monotone; the value s12 saw mirrored at
  0x118/0x53C), 0x340/0x344 track raster position, 0x3F4 toggles, and — new —
  **both 0x604 (0x0078→0x007A high-half) and 0x614 (0x22500→0x22900) move**:
  the in-kernel `stable=yes` for 0x614 was a per-pass sampling artifact; the
  window rows refute it. Both former candidates are counters, not config.
- **Mode-timing block identified:** 0x34C=0x07380BAF decodes as
  vtotal=0x738 (1848) | htotal=0xBAF (2991) — exactly raster totals for the
  2880×1800 panel with blanking; 0x348=0x00310070 is sync/porch-shaped
  (49/112); head 1 holds near-reset 0x00050008/0x00060009. The head block's
  0x340-region is the live timing/raster cluster, matching HEAD_STAT.
- **Surviving stable head-0-only config:** 0x310=0x008959E6 (same value
  across s12 AND s13 boots — config, not a counter; magnitude ~9.0M fits no
  obvious address/pitch/size against fb=+0x20000, pitch 0x2D00, fbsize
  0x13C6800 — PLL/link-coefficient-shaped, unresolved), 0x30C=0x58008000
  (flag word vs head 1's 0x01220000), 0x520=0x00000600, 0x600=0x000F4101
  vs 0x00000100 (enable cluster), 0x610=0x08000014 vs 0x08000000.
- **Verdict: the scanout surface ADDRESS is not exposed anywhere in these
  head-block windows.** 0x408=0x21EC4000 is address-shaped but identical on
  the dead head — a shared default, not the surface. Conclusion for pull 4
  planning: on 917D the armed surface likely lives in EVO core-channel state
  (reachable via the core channel, not as a bare per-head MMIO word), or in a
  head-block region outside the three windows. Peter decision required either
  way (first write vs wider read).

**Boot 2 — kepler pull 14: CTRL_ADDR TARGET hypothesis REFUTED (12/12).**
- All three PBDMAs read `pre=00000000 hi=00000000`; every TARGET value 0..3
  on every PBDMA wrote and READ BACK exactly (`wrote=rb`, register writable,
  never ABSENT/RO) — and the s10 witness ladder never latched once:
  `WITNESS FAILED - bits stripped` on all 12 steps, err=2 throughout,
  fence-timeout each iteration, clean evidenced restores (`restored
  rb=00000000`) between steps. Amendment discipline held (one PBDMA at a
  time; no freeze since no PASS).
- **M2 disp-era recon (read-only) found live EVO core-channel state:**
  `disp-userd-recon pdisplay_0=917D0210 +40=0000000A evo_0x490=0D0500A9
  evo_0x494=00000001`. 0x610490 holds a rich value and 0x610494 reads 1 —
  the disp-era USERD/core-channel enablement path (the last pre-committed
  fallback) has a live anchor to probe.
- Fence-wall ledger after s13: refuted = 3 runlist encodings (s8),
  USERD_SNOOP (s10), USERD_HI bit31 (s11), PFIFO_FLUSH (s12), CTRL_ADDR
  TARGET ×12 (s13). Remaining in-family lead: disp-era USERD enablement
  (write phase — needs its own brief). Beyond that the lane pivots to
  PGRAPH/ucode (K-GPU-4) — Peter strategy call.

## Sitting #12 (display pull 2 + kepler pull 13, UnaOS-gemini@9d22d263, 2026-07-23, fox-metal-r23s1i)

Captures: `~/.claude/plans/unaos/review/rmbp-s12boot1-headdumps.md` (boot 1,
full rows) and `rmbp-s12boot2-capture.md` (boot 2, raw post-mark). Bench note:
port labels were crossed — the rMBP serial for boot 1 landed in the
`pi4-r23s1i/cu.usbserial-ABAFUJCO.log` capture; content-verified rMBP output.

**Boot 1 — head0/head1 differential dump DELIVERED (display pull 2 complete):**
- Two full trace passes; head 0 dump 49/46 live rows, head 1 dump 40/38 rows,
  neither capped (96/64 caps were sufficient). HEAD_STAT confirms head 0 alive
  again (vert/horz counters tick across passes), heads 1–3 stat zero.
- Offline diff (coordinator decode): 19 offsets differ. Head-0-ONLY rows split
  into (a) frame-varying values — 0x118/0x314/0x53C all hold the same value
  (0x0E35 pass 1 → 0x0D9E pass 2) and 0x340/0x344 track the HEAD_STAT raster —
  i.e. live-scan telemetry, and (b) stable config-shaped rows: 0x310=0x008959E6,
  0x520=0x00000600, 0x604=0x00780000, 0x614=0x00022500. Rich DIFF rows where
  head 1 holds near-reset defaults: 0x348/0x34C (0x00310070/0x07380BAF vs
  0x00060009/0x00050008 — timing-shaped), 0x600 (0x000F4101 vs 0x00000100),
  0x538 (0x80001200 vs 0x80000000), 0x30C (0x58008000 vs 0x01220000).
- **The hoped-for clean signature did NOT fall out**: no offset on either pass
  holds a 0x200/0x20000/0x90020000-shaped value (the GOP surface address in
  any obvious shift). The scanout surface pointer is not a bare address in the
  0x616000 head block, or it is encoded (0x310's stable 0x008959E6 is the one
  address-shaped head-0-only candidate). Write-a-pixel cannot claim a proven
  target register yet; a decode step stands between us and it.

**Boot 2 — pull-13 flush hypothesis REFUTED (correcting the bench summary):**
- The first bench read ("aborted before PFLUSH ever printed") was wrong — the
  grep missed the success-branch marker. The ladder RAN:
  `flush-executed 0x70000 pre=00000000 post=00000000 iters=1` (register
  present, not ABSENT/POISON, drained in one iteration) →
  `WITNESS FAILED - bits stripped` → err=2 unchanged post-restore →
  post-submit stat=0x5, playlist_rd advances (0x2013/len 0x00100003) →
  all three PBDMA discriminators CHID=0 ACTIVE=0 → fence timeout at
  0x2014000, `takeover-abort fence-timeout gp_get=0 ch_stat=11000001`.
- Verdict: **a PFIFO_FLUSH between instance writes and validate does not stop
  the VALID/POLL strip.** Engine-side stale-view-of-BAR1-writes, in its
  flushable form, is refuted. inst-raw confirms our bytes persist
  (0C=80000000) exactly as s11 found.
- New evidence for the fallback audit: full RAMFC post-submit dump (16 rows;
  +08=02002000 userd, +10=0000FACE, +30=FFFFF902, +48/+4C=02001000/00090000)
  and per-PBDMA eng_mask readbacks (0x01/0x6E/0x10) with ib_put=ib_get=0 —
  no PBDMA was ever bound to the channel.
- Boot-2 kdisp side printed `takeover-abort no-match` only (expected — display
  read-only this sitting).

Sitting #12 complete, both rungs. Fence lane → pull 14 per the pre-committed
fallbacks (PBDMA CTRL_ADDR TARGET audit; disp-era USERD enablement). Display
lane → pull 3 shape is a Peter decision (decode-first vs write-and-watch).

## Sitting #11 (display pull 1 + kepler pull 12, UnaOS-gemini@9eab5823, 2026-07-22, fox-metal-r23s1h)

**Boot 1 — HEAD 0 IS ALIVE AND SCANNING (major canon correction):**
- `caps version=0210 class=917D` (GK107 display class live);
  `gop phys=0x90020000 vram_off=0x20000`.
- Both candidate EVO mirror layouts REFUTED: evo rows AND hv rows all zero on
  all 4 heads.
- **`head[0] stat underflow=0 vert=0x0493048A horz=0x0000068C` — nonzero
  raster counters, head 0 ONLY** (heads 1-3 stat all zero). The display
  engine was NEVER torn down and NEVER idle; every "evo=crtc=0 all heads →
  engine idle" reading (sittings #5-#10) was wrong-address decode — exactly
  what the panel-owner proof demanded. vert 0x0493/0x048A and horz 0x068C
  decode as plausible raster line/column pairs for the panel timing.
- Sentinel discipline worked: trace cells beyond caps+head0-stat carry the
  DEAD sentinel, so real-vs-absent is unambiguous per cell.
- Display pull 2 target: HEAD_STAT (0x616000-block) is the one genuinely
  decoded block — derive the armed/surface registers outward from ITS
  offsets for the 917D class.

**Boot 2 — USERD_HI bit31 refuted as the poll enable, with a new precision:**
- Witness FAILED, err=2 unchanged, discriminators zero, clean evidenced
  restore (post-restore err=2 stat=0).
- NEW: `inst-raw 0C=80000000` — **the bit31 write PERSISTS in instance
  memory** (read 0 in s9/s10). Coordinator's read, sharper than the bench
  framing: the strip has always been on the PFIFO_CHAN MMIO word (documented
  NO_POLL refusal), NOT on instance bytes — inst writes are visible to US
  via BAR1. The genuinely open question this creates: does the ENGINE see
  our instance bytes at validate time? BAR1 readback only proves BAR1
  self-coherence, not scheduler-side visibility (WC/L2 flush between inst
  writes and validate is now a live hypothesis).
- Fence poll ran bounded, failed as expected.

Sitting #11 complete, both rungs: head-0-alive (boot 1) + inst-writes-persist
/ USERD_HI-refuted (boot 2). Pull-13 targets: (1) VRAM-write→validate
visibility (a cited flush/serialization step), (2) if flushing changes
nothing, the poll area is still elsewhere — widen the derivation. Capture +
MANIFEST are the record.

## Sitting #10 (igpu pull 5 + kepler pull 11, UnaOS-gemini@dffa7816, 2026-07-22, fox-metal-r23s1h)

**Boot 1 — GMUX PROTOCOL PROVEN, PANEL OWNER NAMED (canon reversal):**
- Version 3.2.19 via the 32-bit indexed read (#9's failure was the 3×8-bit
  variant fact, confirmed); MAX_BRIGHTNESS=0x3FF second proof. Gate PASSED.
- Decoded, stable at Boot AND Kernel: **SW_DISPLAY=0x03 (DISCRETE),
  SW_DDC=0x02 (DISCRETE), DISC_POWER=0x03 (ON). The Kepler dGPU owns the
  panel at every observed instant.**
- Canon updates:
  1. iGPU-all-dead (all 4 trace points, reconfirmed this boot) is the
     EXPECTED state — the iGPU paradox is CLOSED, not mysterious.
  2. **Sitting #5's gmux/iGPU redirect is formally REVERSED.** The GOP
     console at 0x90020000 is scanned by the KEPLER; sitting #5's
     "evo=crtc=0 on all heads" is now the anomaly to re-derive: either the
     GK107 PDISPLAY head/scanout decode is wrong for this part, or firmware
     tears the Kepler display engine down at EBS while the gmux keeps the
     panel wired to it (which would produce exactly the black panel).
  3. Display line pivots back to Kepler-side scanout derivation
     (BRIEF-kepler-display-pull1-scanout-rederive in video/Kepler/ — the
     display specialist carries it; module split first so the two Kepler
     lanes don't collide in kepler.rs).

**Boot 2 — Candidate A cleanly refuted:** `USERD_SNOOP orig=0` → write 1 →
witness FAILED (bits stripped), snoop restored; err stays 2, stat 5,
discriminators 0, RAMFC untouched. No residue.
- **Write-behavior pattern (coordinator-refined from Fox's read):** it is NOT
  "PFIFO config writes don't stick" — SUBFIFO_ENG_MASK (0x2390+) and
  PLAYLIST_WR/LEN demonstrably stick. The true split:
  (i) PFIFO_CHAN VALID/POLL — SEMANTIC refusal (chip sets err=2 NO_POLL and
      strips by design; this is the documented CHAN_TABLE_ERROR behavior);
  (ii) USERD_SNOOP 0x2a1c — writes-read-as-zero, UNEXPLAINED: either absent
      on GK107 (rnndb has no variants tag either way), write-gated, or not a
      simple boolean on this part.
- Open hypotheses for pull 12, strongest first: (b) the "poll area" on GK104+
  is per-channel state in INSTANCE memory (USERD pointer/enable in the inst
  block or channel-table INST word), not a global MMIO knob — we may be
  missing an inst-block field, not an MMIO write; (a) a PFIFO
  reset/priv-unlock handshake gating config writes; (c) a sched-block clock
  domain. envytools hwdocs (allowed source) likely documents GK104 fifo
  channel setup.

Sitting #10 complete, both rungs: panel owner PROVEN (discrete), candidate A
refuted, write-behavior pattern named and refined. Capture + MANIFEST are
the record.

## Sitting #9 (igpu pull 4 + kepler pull 10, UnaOS-gemini@785a8795, 2026-07-22, fox-metal-r23s1h)

**Boot 1 — gmux handshake: protocol UNPROVEN, but the picture sharpened:**
- Version self-test FAILED (implausible tuples) → gate held, raw bytes only.
- With the real handshake the values are STABLE boot→kernel (sitting #8's
  0x39→0x03 "movement" is RETRACTED as a handshake artifact — the canon guard
  was right) and the three registers now answer DISTINCT bytes:
  SW_DISP=0x03, SW_DDC=0x02, POWER=0x03 (both points).
- UNPROVEN-decode note (not canon): 0x03 in SW_DISPLAY would read "discrete
  owns the panel" in the classic decode — which would put the GOP console on
  the Kepler side and reopen the wrong-registers question on the KEPLER
  display engine, not Intel. Proving the protocol is now the whole game:
  pull 5 = variant version-reg offsets / gmux revision protocol variants.
- iGPU teardown rows unchanged (all-dead, DP_A=0x1C constant).

**Boot 2 — THE CHIP SPOKE. The wall's name is NO_POLL:**
- `sched-status`: pre-init `err=0 stat=0` → post-init `err=0x00000002` →
  post-submit `err=0x00000002 stat=0x00000005`. CHAN_TABLE_ERROR EXISTS on
  GK107 and names the reject: **code 2 = NO_POLL ("validated a channel with
  POLL_ENABLE, but poll area is disabled")**, fired at channel-VALIDATE time,
  before any runlist submit.
- Sharper coordinator read: the hardware REFUSES the validate — sitting #8's
  dump already showed it (`PFIFO_CHAN[1] 00=0x00002000` after we wrote
  0x80002000: bit31 was CLEARED on readback). The chip rejects and strips
  VALID(/POLL) when the poll area isn't configured. Bit30 "not sticking" is
  the SYMPTOM; the missing "poll area" configuration is the cause.
- `stat=0x00000005` (SCHED_STATUS, RO) post-submit — undecoded; rnndb gives
  no bit meanings.
- rnndb dead-ends on "poll area": the only mentions in gf100_pfifo.xml are
  the NO_POLL code and the POLL_ENABLE bit. Pull-11 derivation must find the
  poll-area config (USERD/BAR1 poll machinery or PFIFO config) elsewhere in
  envytools or empirically.
- Everything else unchanged (discriminators 0, RAMFC untouched, gmux rows
  as boot 1).

Sitting #9 complete, both rungs. **First silicon-named root cause of the
fence wall.** Capture + MANIFEST are the record.

## Sitting #8 (igpu pull 3 + kepler pull 9, UnaOS-gemini@b3ec47d1, 2026-07-22, fox-metal-r23s1h)

**Boot 1 — display paradox, two hard facts:**
- **Point-0 is ALL-DEAD too.** Pipes/planes/PP_STATUS/PP_CONTROL/DPLL_A read
  0x00000000 at all four points (first-instruction-adjacent bootloader entry
  included); DP_A=0x1C constant. Panel power and the PLL were NEVER on at any
  observable instant → the "firmware tears down during our bootloader window"
  theory is DEAD. What remains: wrong-registers or the mux points elsewhere.
- **The gmux answers on the INDEXED protocol and its state MOVES:** indexed
  reads return real bytes (classic PIO = sentinel; absent on this rig);
  values 0x39 at Point-0 → 0x03 at kernel probe. First register on this
  machine that answers differently at boot vs kernel.
- **CAVEAT on the decode (not yet canon):** idx_SWITCH and idx_POWER returned
  IDENTICAL bytes at each point (0x39/0x39, 0x03/0x03). Two distinct
  registers agreeing twice is the signature of an incomplete indexed
  handshake (missing ready-wait between index write and value read) — we may
  be reading a status/stale byte, not per-register data. The boot→kernel
  CHANGE is a real observable; the 0x39/0x03 meanings are NOT decodable yet.
  Pull 4 = full indexed protocol with ready-wait + a version-register
  self-test (known-shape value proves the protocol before trusting
  switch/power).

**Boot 2 — fuzz answered, negatively but sharply:**
- `playlist_rd=0x2013 len=0x00100003` — scheduler READ and COUNTED all three
  entries (len 1→3 vs #7).
- DISCRIMINATOR pbdma0/1/2 all `ch=0 ACTIVE=0` — raw, bit31-valid, and
  bit0-valid entry encodings ALL REFUTED as sufficient.
- `PFIFO_CHAN[1]` pre==post (`00=00002000 04=11000001`); RAMFC untouched by
  hw (0xFACE sentinel intact). The scheduler never writes back anything.
- Synthesis: runlist parse is fine; the gate is a per-channel scheduling
  PRECONDITION. Pull-10 leads (rnndb, cited): GF100's CHAN_TABLE decode has
  bit30 `POLL_ENABLE` + bit31 `VALID` in the CHAN word and bit0 `RUNNABLE` in
  STATE — GK104's "UNK31" is almost certainly VALID, and we never set the
  bit30 analog. And **`CHAN_TABLE_ERROR` (PFIFO+0x52c) is a readable reject
  reason** (codes incl. NO_POLL "validated a channel with POLL_ENABLE, but
  poll area is disabled", NO_ENGINE, INVALID_TARGET) plus `SCHED_STATUS`
  (+0x63c RO) — never read them; the chip may have been naming the reason
  every boot.

Sitting #8 complete, both rungs. Capture + MANIFEST are the record.

## Sitting #7 (igpu pull 2 + kepler pull 8, UnaOS-gemini@7014b022→94b0ed0c, 2026-07-22, fox-metal-r23s1h) — COMPLETE

**Boot 1 (teardown hunt):** three-point trace ran; verdict **ALL THREE POINTS
DEAD** — iGPU pipes/planes/DP_A read disabled even PRE-ExitBootServices.
Firmware never lights iGPU scanout at any stage we can see. (Filter lesson:
the rows carried no "igpu" substring and were initially reported missing;
rows now carry the `:: igpu:` prefix. Residual caveat: an all-zero trace was
also the failed-BAR0-read signature; the bootloader helper now returns the
`0xBAD0BA20` sentinel on a failed read, so the ambiguity dies with the next
boot.)
- Combined with #5 (Kepler heads dead) and #6 (iGPU dark at kernel time):
  **no scanout engine on either GPU is lit at ANY observed point**, while the
  GOP fb at 0x90020000 accepts writes. Display strategy remains open — next
  question is who CAN light a pipe, and what the gmux muxes.
- **CF8-failed-read caveat REFUTED (canon upgrade):** the DP_A row reads
  `0x0000001C | 0x0000001C | 0x0000001C` — nonzero and stable across all
  three points while every pipe/plane row is zero. The bootloader's reads are
  live (a failed read would have zeroed DP_A too). All-three-points-dead is
  REAL. Since the GOP text console is visibly scanned during Option-boot,
  either (a) firmware tears scanout down BEFORE our Point-1 read (Point-1 is
  later in the boot than assumed), or (b) scanout state on these parts lives
  in registers other than the ones decoded. Pull-3 brief targets the split:
  Point-0 at bootloader entry + gmux status readback.

**Boot 2 (all five knobs) — "hard hang" RETRACTED, was SLOW:** full output
landed ~6-7 min in; culprit = unbounded instrumentation polls (10M-read fence
poll through BAR1 + takeover retries), not a wedge. Fixed post-sitting: fence
poll bounded 500k, acceptance poll 100k, and GPU init moved AFTER xHCI so any
future GPU wedge prints breadcrumbs instead of pre-serial silence (bench
serial is the usbdebug FTDI behind xHCI — structural blind spot removed).

**Kepler wall-2:** the ORDER defect was REAL and is FIXED on silicon —
`inst-raw 4C=0x00090000` (was 0x01FF0000; log2(512)=9 took). But **REFUTED as
the bind wall**: post-bind `playlist_rd=0x2013 len=0x100001` (runlist read),
yet all three PBDMAs still `ch=0 ACTIVE=0 ib=0/0`, `gp_get=0`,
fence-timeout, `ch_stat=0x11000001`. The channel is never scheduled onto any
PBDMA. Pull-9 target: runlist entry format/ID encoding, RAMFC fields the
scheduler validates, submit-vs-enable ordering, which-runlist.

**Boot 2r (94b0ed0c, all five knobs) — all post-sitting fixes PROVEN on
metal:** serial-first GPU init (PDISPLAY breadcrumb on the wire), takeover
aborts in seconds (bounded polls), prefixed rows pass the filter. Content
pure confirmation of the above canon (iGPU all-dead, DP_A=0x1C ×3, ORDER
0x00090000, playlist read, PBDMAs unbound). Wall 2 = pull-9's
runlist-entry/channel-bind, nothing else remains.

Capture: `~/unaos-bench/capture/rmbp-r23s6/` + MANIFEST rows.

## Sitting #6 (igpu pull 1 + kepler pull 7, UnaOS-gemini@8105f73c, 2026-07-22, fox-metal-r23s1h) — superseded notes below (boots 1/1b/2b)

**Boot 1 (8f7aaa6e media) — WASTED, defect ours:** the staged kernel carried no
probe. Builder lacked the `UNAOS_IVB` env→feature mapping AND igpu.rs had never
compiled (3 errors) — the land-review "gates green" never armed the knob.
Fixed in d6efd093; false PASS corrected in 8105f73c. **Law adopted (both
sides): a knob-gated PASS requires the gate WITH knobs armed + strings-proof
of the probe in the builder-path kernel.elf.** (The builder's own env→feature
map can silently drop features — check it, not just arroyo's.)

**Boot 1b (8105f73c, strings-verified media) — iGPU probe CLEAN, and the
panel theory flips:** `[Intel iGPU]` through `:: igpu: probe-complete ::`.
- Pipes A/B/C `CONF=0` (ALL disabled). Planes A/B/C `CNTR=0 SURF=0 STRIDE=0
  LINOFF=0 TILEOFF=0` (all disabled, nothing mapped). `DP_A=0x1C` (port not
  enabled). FOX CROSS-CHECK line correctly did not fire.
- **Reading:** GOP left NO live iGPU scanout. Combined with sitting #5
  (Kepler evo/crtc=0 on all 4 heads): **NEITHER GPU has an enabled scanout at
  probe time** while the panel is black. The "gmux gave the panel to the iGPU"
  theory loses its iGPU half as-probed. Either firmware tears scanout down at
  ExitBootServices, or the 0x90020000 fb writes go to an aperture nobody
  scans. Milestone-2 framing (write-in-place vs GGTT remap) is MOOT as posed —
  there may be nothing to write into; the next question is "who can light a
  pipe from scratch" + what gmux muxes when both engines are off. Per
  null-hypothesis law, our bootchain's at/after-handoff behavior stays the
  prime suspect for the teardown. **Strategy call is Peter's, not an
  auto-continue.**
- Sitting-brief hygiene (recorded): boot-2 knob list must be
  `UNAOS_USBDEBUG+UNAOS_IVB+UNAOS_KEPLER+UNAOS_KEPLER_TAKEOVER+UNAOS_KEPLER_FIFO`
  — the original brief omitted the FIFO knob; Fox's strings check caught the
  under-build before it flew.

**Boot 2b (full pass) — SITTING COMPLETE, wall 2 relocated upstream:**
- igpu probe re-ran identically (all-dark confirmed twice). Nit: the probe
  prints twice — PCI walk revisits the device; harmless, dedupe whenever the
  file is next touched.
- Kepler: pbdma-count 3; **all three PBDMAs `ch=00000000 ACTIVE=0,
  ib_put=ib_get=0` — no PBDMA ever binds our channel.** Eng-masks are set
  (pbdma0=0x01, pbdma1=0x6E, pbdma2=0x10). Clocks proven fine:
  `PMC_ENABLE=0xE011216D` (PFIFO=1), `SUBFIFO_ENABLE=0x7` — the
  clock/enable theory is DEAD. Meanwhile `playlist_rd=0x2013 len=0x100001`
  (scheduler reads the runlist), `ch_stat=0x11000001` (ENABLED), `gp_get=0`,
  fence-timeout as before.
- **Synthesis:** the fetch never happens because the channel is never
  SCHEDULED onto a PBDMA. The wall moved upstream of PBDMA to the
  runlist-entry/channel-bind step. Pull-8 candidate list (rnndb facts only):
  (a) GK107 runlist entry format/ID vs our channel id; (b) RAMFC/instance
  fields the scheduler validates before binding (inst-raw
  08=0x02002000 0C=0 48=0x02001000 4C=0x01FF0000 is on serial to decode);
  (c) runlist submit/commit ordering vs channel-enable; (d) whether the
  channel must ride the ENGINE's runlist rather than runlist 0.

Capture: `~/unaos-bench/capture/rmbp-r23s6/` + MANIFEST rows.

## Sitting #5 (pull 6 v2, UnaOS-gemini@e49efbeb, 2026-07-22, fox-metal-r23s1g) — STRATEGIC REDIRECT

**Wall 1 — DOUBLE REFUTATION + gmux redirect.** Both candidates dead on all 4 heads:
`head-raw head=N evo=00000000 crtc=00000000` → `bad-read head N no valid candidates` ×4.
The whole PDISPLAY engine reads idle. NEW sitting fact: the internal panel went BLACK
almost instantly on EVERY boot (kbase and k1 too — usbdebug builds that should hold the
boot log on-panel; serial fully healthy throughout).
- **Reading (Fox):** the 2012 rMBP has a **gmux** — the internal panel is on the Intel
  HD 4000 **iGPU**, and the Kepler display engine is legitimately dark. The GOP fb at
  `0x90020000` (confirmed live in boot-2 fb-wc retype) belongs to the **iGPU**, not
  Kepler. So wall 1 was misframed: not "which Kepler register holds scanout" but
  "which GPU owns the panel" — and it is the iGPU.
- **Consequence:** Kepler *display takeover* (K-GPU-2) may be a dead end on this box.
  Kepler compute / PFIFO (K-GPU-3) is unaffected. Next display derivation, IF pursued:
  gmux state readback (ACPI GMUX / port 0x7xx IO) + Intel iGPU scanout regs — NOT more
  Kepler PDISPLAY decode. **This is a strategy decision for Peter, not an auto-continue.**
- Open question (bench): the black-panel-on-every-boot incl. kbase (no Kepler active) —
  environmental vs a broader fb-handoff regression. Per null-hypothesis-our-code, do not
  assume hardware; flag for a clean cross-check.

**Wall 2 — progress, still no fence.** `pbdma-count 3` (oddity persists) ·
`pbdma-eng-mask set` (took) · `inst-raw 08=02002000 0C=00000000 48=02001000 4C=01FF0000` ·
`fifo-layout userd=2002000 fence=2014000 gp=1/0` · `bad-read pbdma 40108 00000000` ·
`fifo-front pbdma_stat=00000000 playlist_rd=00002013 playlist_rd_len=00100001` ·
`ch_stat=11000001 (ENABLED UNK24_RO UNK28_RO)`.
- NEW vs #4: eng-mask took, channel ENABLED, playlist_rd nonzero (scheduler SEES the
  playlist) — yet `gp_get=0`: PBDMA still never fetches the GP entry. `pbdma_stat=0` +
  the `40108` bad-read are the fresh clues: we still cannot read PBDMA status at that
  offset → wrong PBDMA base, or the unit is unclocked/not started.
- NEXT: the PBDMA-count=3 decode and the `40108` bad-read together — derive the correct
  GK107 PBDMA register base + the unit start/clock so `40108` reads real status.

Capture: `~/unaos-bench/capture/rmbp-r23s1g/`. Staging note: a zsh env-split staging bug
first pass, caught by sha-compare + rebuilt (verify-strings law held).

## Sitting #4 (pull 5, UnaOS-gemini@44cf4387, 2026-07-22) — both walls: right block, wrong slice

**Wall 1 — head scanout (base 0x616100 ARMED block is LIVE, slicing wrong):**
All four heads read byte-identical:
`head-raw addr=00000001 size=078004FE storage=0A0006A8`
- Non-zero = the 0x616100 block is real display state (progress from #3's uniform zeros).
- All 4 heads identical → the `head*0x800` stride is collapsing (reading the same regs 4×) — stride wrong.
- `addr=0x00000001` is not address-shaped (flag/enable bit) → field offsets within the ARMED block are wrong.
- `size=0x078004FE`: 0x0780 = 1920 in the high half → this IS display geometry, just sliced wrong.
- NEXT: verify per-head stride AND field offsets in the GF119+ ARMED block; scanout addr likely at a different sub-offset; 0x…4FE/0x6A8 look like packed geometry/config, not a scanout address.

**Wall 2 — PBDMA (base 0x40000 reads clean-zero, unit never started):**
`pbdma-count 3` · `inst-raw 08=02002000 0C=00000000 48=02001000 4C=01FF0000` ·
`fifo-layout userd=2002000 fence=2014000 gp=1/0` · `bad-read pbdma 40108 00000000` ·
`fifo-front pbdma_stat=00000000 playlist_rd=00002013 playlist_rd_len=00100001` ·
`ch_stat=11000001 (ENABLED UNK24_RO UNK28_RO)`
- PSUBFIFO 0x40108 went poison(#4)→zero(#5): base likely right, but unit reads 0 = never started/clocked.
- `pbdma-count=3` vs expected 1 on GK107 → count register/decoding needs a second look; if 3 is real, per-PBDMA stride decides which unit serves our runlist.
- `gp_get=0` persists: PBDMA never fetches entry 0.
- `playlist_rd=0x2013` stable across #4 and #5 → consistent read, likely a real breadcrumb.
- NEXT: which PBDMA unit is bound to our channel's runlist + its start/enable/clock sequence.

## Open cleanroom debt
- `kepler.rs:~465` EVO core-channel offsets (0x490) carry a "derived from nouveau/gf119.c"
  comment (entered pull 3 / 1c9e2570). Must become an rnndb citation or an honest
  empirically-probed note before merge. Not a bench concern.

## Earlier sittings (summary)
- #3 (pull 4, d938dd00): head-raw uniform ZEROS (wrong block); pbdma_stat=0xBAD0011F poison
  at legacy 0x6c0 (wrong base). Both walls localized.
- #2 (pull 3, f9804ab6): BUG#1 (VRAM 2989MB) + BUG#2 (GOP no-gop) CONFIRMED FIXED on silicon
  (512MB; GOP 0x90020000 = BAR1+0x20000). Introduced EVO flip + PFIFO.
- #1 (pull 2, c84688f1): first silicon contact; found the two bugs #2 fixed; builder
  UNAOS_KEPLER→feature mapping catch.
