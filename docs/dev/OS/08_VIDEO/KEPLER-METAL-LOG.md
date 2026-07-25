# Kepler bring-up — metal facts of record (rMBP GT 650M / GK107)

Hard-won silicon facts from the fox-metal sitting series. Trust these over any
QEMU behavior. Newest sitting first.

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
