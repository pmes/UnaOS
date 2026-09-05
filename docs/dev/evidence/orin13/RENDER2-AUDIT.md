# render2 boot — whole-log audit (CAPTUREAUDIT, 2026-09-05)

Input: the render2 boot re-extracted from `~/unaos-bench/capture/line-acm0/raw.log` (unwrapped with
`unwrap80.sh`, boot starts at unwrapped line 102226 — the `KELF min=0x0 max=0x2d92a8 pg=730` anchor).
Copy audited: `~/unaos-bench/scratch/orin13/audit/render2-full.log`, **2189 lines** (the flight was
scored on the first 924; the boot has run 430 s of uptime since). Every line was read. Line numbers
below are lines of `render2-full.log` (add 102225 for raw.log). Source tip: `077a8fa1`.

The seven pre-registered questions in `FLIGHT-RESULT.md` are not repeated here; their answers stand.

## 0. Verdict in one paragraph

No exception, no wedge, no stall, no `REFUSE`, no `FAIL`, no timeout, no retry in 2189 lines. The
render task ran 244.9 M passes at a steady ~570 k/s with a census every second for 430 s (443
census lines, 10 per 10 s on 42 of 43 intervals — the 43rd is the partial one). Two findings are new
to the ledger, both pre-existing on every Orin boot in this capture but recorded nowhere: **(1) the
pulse window sits on top of the console window's bottom four text rows — the shell's prompt row is
occluded, and `[wcn] win=3 att=0 comp=N` is the fingerprint of that overlap**, and **(2) both USB
hubs fail their status-change endpoint configure (codes 17 and 8), so hot-plug on either hub is
dead for the life of the boot**. One cosmetic witness defect (MOUSE-1 prints `vid:pid=0000:0000`
for hub-attached pointers). Everything else in the pattern sweep is expected on this image or
already on a ledger.

## 1. Findings ranked (new to the ledger first)

### N1 — the pulse window occludes the console window's last ~4 rows (layout overlap)

* **Capture:** line 473 `[wc-x] console-window win=2 … box=1305x780 at (307,158)` and line 528
  `[pulsewin] open win=3 … box=1290x212 at (10,874)`. Console box spans y 158..938; pulse box spans
  y 874..1086 and x 10..1300. Overlap: 64 rows × ~993 px, z=3 over z=2 (`[wcn] win=3 … above=yes
  z=3`). The console SURFACE is at (312,197) 1295x736, cell 7x16, 46 rows: rows 42 (partial) to 45
  lie under the pulse window — the bottom of the scrolling console, where the prompt and the newest
  output are.
* **The compositor confirms it:** four of the eight `[wc-h] win=3` lines after the open (540, 571,
  604, 608) are `span=64 band=yes` — 64-row band repaints, exactly the overlap; every `[wcn] win=3` line from 594 onward
  reads `att=0 comp=N pre=0 drg=N` (N=1..3), and the console's own line carries the dual `dout=2
  dkpx=347` (line 561). Per `wm.rs:11824-11846`, `drg` = "blits of this row that the upward
  occlusion closure ADDED: a lower-z window's damage overlapped this row's outer box". The pulse
  window's owner never presents; it is repainted only because the console under it does.
* **Source:** console placement `video/fbcon.rs:1966-1972` — centred in the work area
  (`oy = wtop + (ph - wtop - chrome_h(ph) - oh) / 2` → 158); pulse placement
  `video/pulsewin.rs:449-453` — bottom-left of the work area (`oy = ph - chrome_h(ph) - gap - oh`
  → 874). Neither site knows about the other. `engine.md` §PULSEWIN (line 14175 ff.) records the
  bottom-left choice and says it "does NOT resolve the underlying conflict" with the console window
  on the rMBP bench; the Orin instance is not in `orin-desktop.md` (no "overlap"/"occlu" hit).
* **Classification:** NEW to the ledger. Pre-existing in mechanism (both placements predate
  render2) but this is the first Orin boot on which the pulse window ever opened, so it is the first
  boot that could show it.
* **Action:** size the console's work-area box against the pulse window's reservation (or vice
  versa) at `fbcon.rs:1966` / `pulsewin.rs:449` — e.g. subtract the pulse box height + gap from the
  console's available height when `pulsewin` is compiled, so `rows` drops from 46 to ~42; or open
  the pulse window bottom-RIGHT where the console's x-extent (307..1612) still collides — so the
  height subtraction is the real fix. Cheap check on the next boot: no `[wc-h] win=3 … span=64
  band=yes` lines, and `[wcn] win=3` silent (a static window with `drg=0` prints no line at all).

### N2 — both USB hubs fail status-change endpoint configure; hot-plug behind the hubs is dead

* **Capture:** line 267 `xHCI: HUB slot 1 status-change Configure-Endpoint code 17` (the Realtek
  0bda:0489 SuperSpeed hub, root port 1) and line 332 `xHCI: HUB slot 3 status-change
  Configure-Endpoint code 8` (the Realtek 0bda:5489 HS hub, root port 6). xHCI completion code 17 =
  Parameter Error, 8 = Bandwidth Error (spec Table 6-90). The success line
  `status-change endpoint configured … hot-plug armed` never prints.
* **Source:** `drivers/xhci/mod.rs:13387` `configure_hub_interrupt_ep`, print at `:13465`, called
  from `bring_up_hub` at `:13157`. On the non-success arm `hub_int_ep` stays 0, so
  `queue_hub_change_read` is never armed and both hot-plug consumers are gated off:
  `:4187` (`s.is_hub && s.hub_int_ep != 0` — re-arm on read error) and `:4556` (`hub_int_dci`
  = None). Consequence: a device plugged into either hub after boot is never enumerated; devices
  present at boot are unaffected (the card reader, mouse and keyboard all enumerated).
* **Pre-existing:** the same two lines appear in every Orin boot in raw.log (unwrapped lines 78151,
  79572, 80954, 88974, 94978, 102492 for slot 1; +65 for slot 3). Not on any Orin ledger:
  `orin-desktop.md` and `arch_arm64.md` have no "status-change"/"hot-plug" entry for it;
  `usb_xhci.md:1555` documents the endpoint as the Pi's VIA-hub M1 milestone. The endpoint context
  built at `:13440-13448` sets Average TRB Length = mps and leaves Max ESIT Payload (dword 4 bits
  31:16) zero, and encodes the SS interval as `bInterval-1`; either is a candidate for the two
  codes, but this audit did not run it down.
* **Classification:** pre-existing on the Orin, unrecorded → treated as NEW to the ledger.
* **Action:** an xHCI item, out of this arc's lane: dump the input context on the non-success arm
  (`:13465`) and compare against spec §6.2.3 for SS (code 17) and HS (code 8); until then note in
  `orin-desktop.md` that hub hot-plug is not armed on this board.

### N3 — `MOUSE-1` witness prints `vid:pid=0000:0000` for hub-attached pointers (cosmetic)

* **Capture:** lines 318 and 329 `:: MOUSE-1: HID pointer detected vid:pid=0000:0000 proto=2
  relative …` / `proto=0 absolute …`, while the enumeration lines 305 and 322 already named the
  devices (`vid=1c4f pid=0034`, `vid=1c4f pid=0002`).
* **Source:** the witness at `drivers/xhci/mod.rs:4302` reads `slots[].vid/pid`; those fields are
  stored only on the ROOT-PORT descriptor path (`:4354-4355`). The downstream path decodes vid/pid
  into locals for its own print (`:14063-14071`) and never stores them. Same on prior boots
  (raw.log 89024, 95029). The `no driver for class 0xe0` line (`:4526`) reads the same fields and
  is correct only because slot 6 is a root-port device. Related cosmetic: `:4328` prints a
  hard-coded `INTERCEPTED DESCRIPTOR EVENT (Slot 1 EP 1)` for slots 3 and 6 (lines 287, 360).
* **Classification:** pre-existing, unrecorded, witness-only. `usb_xhci.md:1580` describes a
  different `vid=0000 pid=0000` (a zeroed descriptor read on the Pi); this one is an unpopulated
  slot field, not a bad read.
* **Action:** store `vid`/`pid` into `self.slots[slot_id].vid/.pid` beside the print at `:14070`.

### No other new defect. Explicitly: no new defect in the compositor, render task, click router,
dock, stage reservation, stack gauge, SMP bring-up, storage, or timing.

## 2. Pattern sweep — every hit, classified

Sweep: case-insensitive `refuse|decline|skip|fail|warn|-E…|error|timeout|retry|wedge|stall|panic|
abort|denied|drop|stuck|miss|lost|degrad|guess|unrecognized|no driver|unknown|fault|torn|clob`
over all 2189 lines. Zero hits for: `REFUSE`, `FAIL` (as a verdict), `-E`, `error` (as an event),
`timeout` (all `timeouts=0`), `retry`, `wedge` (only the `[wedge12] … RESERVED` family), `stall`
(all `stalls=0`). Every periodic counter that could indicate trouble is zero on every line:
`torn=0 stalls=0 longpres=0 declines=0 decl_*=0` (225 `[wc-h]` lines), `aborted=0 stale=0`
(79 `[wcn]` rollups), `stash=0 cash=0 stale=0 -> INERT` (79 `[wc-tail]`), `parks=0` (79
`[fluid3]`), `clob=0 presses=0 raises=0 unhides=0` (79 `[dock]`), `stuck=0 nogeom=0 dropped=0`
(43 `[orinclick]`), `fbbad=0 … -> CLEAN` (22 `[wc-g]`), `bad_cache=0 bad_ram=0 -> PASS`
(3 `[wc-d]`), `short=0 -> RESERVED` (3 `[wedge12]`).

| line | text (abridged) | family / printer | meaning | class |
|---|---|---|---|---|
| 8 | `JB9d[pre-EBS]: MBOX probe SKIPPED ? falcon CSB reads all-ones` | loader `main.rs@1277` | JB9d-GUARD: Falcon CSB all-ones, so the owner-claim write is refused by design | expected |
| 9-12 | `JB6: dummy ACPI … ? XUSB Falcon teardown w:i:l lt esgerlaf…` / `i�v�eM m(IEnLs2t)a…` / `ark-window guard — 89 byte(s) dropped pre-map` | loader `main.rs@1165` + kernel `main.rs:7055` | Two writers byte-interleaved on the wire: the firmware's TCU-buffered tail (`InstallProtocolInterface: 27ABF055-…` = the ExitBootServices event GUID) draining while the kernel starts writing UARTC directly. The kernel's own accounting is the 89 dropped pre-map bytes, identical on prior boots (raw.log 76644, 77931, 79352, 80722) | pre-existing-known (`arch_arm64.md:9651-9683`; the "lossy TCU" caveat at :9708) |
| 15 | `=== butler RESOLVED: unknown -> orin after 2732 unidentified lines over 83.8s` | capture tool, not board output | the line butler identified the board 15 lines into the kernel; the 2732 lines are MB1/MB2/UEFI/loader plus two earlier loader runs (see §4) | expected |
| 17-19 | `XCARVE-6 … source: QUIRK (DTB silent; extent = bounded GUESS)` | `mmu_tegra.rs:125,251` | three carveout windows the DTB does not describe are unmapped at a documented bounded size | expected |
| 125-126 | `DMA-WINDOW STOP … NO dma-ranges … NOT derivable` / `HEAP-GUARD … degraded` | `fdt_tegra.rs:839` | RC node carries no `dma-ranges`; heap placement falls back to the RAS-2 heuristic and says so | expected (by construction) |
| 203, 246, 295 | `[enum port N] … skipping the 100 ms connect debounce` | xhci | attach stable through the 150 ms settle | expected |
| 267 | `HUB slot 1 status-change Configure-Endpoint code 17` | `xhci/mod.rs:13465` | **N2** | pre-existing, unrecorded |
| 332 | `HUB slot 3 status-change Configure-Endpoint code 8` | `xhci/mod.rs:13465` | **N2** | pre-existing, unrecorded |
| 318, 329 | `MOUSE-1: … vid:pid=0000:0000` | `xhci/mod.rs:4302` | **N3** | pre-existing, unrecorded |
| 366 | `no driver for device class 0xe0 (slot 6, 13d3:3549); releasing port` | `xhci/mod.rs:4526` | AzureWave BT combo on root port 7; released rather than parked (the comment at :4515 names this exact device) | expected |
| 370-371 | `BOT: pump … used=36874181 … timeouts=0 result=OK` | xhci | second BOT pump spent 1.18 s (36.9 M cycles @ 31.25 MHz) — the TUR retry, see `SPACE` `tur=1200ms(n=2)` `csw=1180ms`: the card reader answers its first TEST UNIT READY slowly | expected |
| 372 | `BOT: deqprobe … verdict=ctxdeq-stale-under-running` | `xhci/mod.rs:7736` | read-only probe of the EP context dequeue field; verdict = the running read is a birth value | expected (witness) |
| 384 | `SECTOR 0 SIGNATURE:` (blank) | `xhci/mod.rs:12003` | LBA0 bytes 0..21 are zero (boot-code-less MBR: `0x55AA=true type=unrecognized/raw`) | expected |
| 385 | `[IRQ] xHCI interrupts taken so far: 0` | `xhci/mod.rs:12007` | the Orin attach is polled (`JB2b — attaching … (platform, polled, no PCIe)`, line 173); IMAN.IE is set but no GIC SPI is wired, 0 on every prior boot | pre-existing-known |
| 386 | `PIUSB: [piusb25] boot-sector sanity: 0x55AA=true type=unrecognized/raw` | `xhci/mod.rs:12047` | see §5 | expected; naming item |
| 392-395 | `JB3 — inst0/inst1 post-attach faults: sGFSR=0x00000000 … FAR=0x…` | boot_tegra | `sGFSR=0`/`FSR=0`: no fault; the FAR/SYNR values are stale register contents | expected |
| 477 | `[wc-x] console-window panic-fallback armed win=2` | `fbcon.rs` | the fallback is armed; not a panic | expected (FLIGHT noted) |
| 497 | `CAPSTONE: drain witnesses SKIPPED (queue not drained at entry — 4 task(s) pre-staged)` | `sched.rs:10237` | on tegra CAPSTONE always runs beside the pre-staged pump/render tasks | expected (`arch_arm64.md:10731`) |
| 501-506 | `SCHED-BAL: … 0 steals … 0 core(s) ran work`; `SCHED: load c0=100%` | `sched.rs:6050` | everything runs on cpu 0 (render task pinned, console pump cpu 0); c0 pegged by the cooperative busy-poll loop | expected (`main.rs:8318` comment: "the cost lands on cpu 0") |
| 516 | `[orinrender] DECLINE reason=console-already-windowed` | `main.rs:8289` | FLIGHT Q1 | expected |
| 145 | `[wc-h] win=1 staged=no reason=fixture -> DIRECT` | `wcg.rs:728` | the witness build's deliberate one-shot direct-path fixture | expected |
| 152, 469, 565 | `[noatt] … noatt=1 … taker=0 -> UNATTRIBUTED` | `wm.rs:12976` | one seed row per rollup with no attach movement (the fixture / first-frame seeds); `taker=0` because `wcg-paygo` is not compiled, so UNATTRIBUTED is the correct reading. Doc says the first Orin reading is a baseline, not a regression | expected (baseline) |
| 567 | `[comp2] … max_us=188361` | wm.rs | one composite pass of 188 ms inside the window that opened the pulse window and ran CAPSTONE; later maxima 53-56 ms | observation, not a defect |
| 2189 | blank | — | the live tail of the capture | — |

`[pulse5] live … span_max=0ms` (lines 503, 507) and `[pstrip] rollup … redraws=0 skipped=40
srcdelta=0` (44 of 46 lines) are the OPEN item already recorded in `FLIGHT-RESULT.md` and
`orin-desktop.md:3004-3007` (the load sampler reads zero, so the strip never re-dirties).

## 3. The per-window rollups (`[wcn]`)

Printer: `video/wm.rs:13044` `wcn_emit`, line format at `:13160`, rollup at `:13207`; fields
documented at `:11744-11760` and `:11803-11860`.

* `att` — `present` calls that named this row (the owner's attempt), `wcn_note_present` `:12701`.
* `comp` — times the row was actually blitted by a composite pass = `pre + drg` (derived, `:13086`);
  `pre` = the row was already dirty at the table snapshot (own present, cursor rect, erase drain);
  `drg` = the row was ADDED by the upward occlusion closure because a lower-z window's damage
  overlapped its box (`:5836`, `wcn_note_drawn(id, !seed[i])`). `comp > att` is "overlap, not error"
  by the doc's own words.
* `hid` — presents while hidden; `bel` — row declined for sitting below the shell.
* `active`/`parked` — inter-present gaps ≤ 250 ms sum into `active_ms`, longer gaps into
  `parked_ms` (`WCN_PARK_GAP_MS`, `:11801`). `rate` divides by ACTIVE time, falling back to the
  wall span only when there is no active gap.
* `dout`/`dkpx`/`drly` — the dual: this row's own damage promoted a higher-z window (count, kpx,
  relayed).

**win=2 (console):** early `att=28 comp=28 rate=23.4/s active=1195ms parked=4630ms` (line 561),
`att=9 rate=76.2/s active=118ms` (592), `att=7 rate=43.4/s active=161ms` (640): the rate is per
ACTIVE millisecond, so a burst of 7 presents within 161 ms reads 43/s — a boot-log burst, not a
sustained frame rate. Steady state `att=6 comp=6 rate=1.0/s active=0ms parked=5000-6000ms`
(2180 and 40 like it): every gap exceeds 250 ms, the denominator falls back to the span, and the
console presents ~1.2/s for the rest of the boot. What presents at 1/s on an idle console is the
kernel's own witness output mirrored into the console window (`fbcon mirroring the boot log`,
line 30; `[wc-x] console-route first-paint win=2 (glyphs -> window surface)`, 472). Histogram over
the boot: att=5..8 on 75 of 78 lines. Not a defect; it is the cost of the serial chatter being
echoed to glass.

**win=3 (pulse):** line 563 at open: `att=2 comp=5 pre=3 drg=2` (its own two presents plus three
drag-ins); every later line `att=0 comp=1..3 pre=0 drg=1..3`. **Not an accounting defect** —
`pre + drg == comp` holds on every line and a window that is never presented by its owner is the
expected static state (`ui_status::tick` never dirties, the OPEN item). But `drg>0` on a window
nobody touches is the compositor stating that the console's damage reaches the pulse window's box:
it is the fingerprint of **N1**. A correctly laid-out static window would read nothing at all (the
line is suppressed at `att==0 && comp==0 && bel==0`, `:13104`).

## 4. Dock and click router

`[dock] live` is `strip::Ledger::rollup` (`video/strip.rs:541`) with the dock tail
(`video/dock.rs:656`): `passes` = composite passes in which the dock scanned its model,
`paints` = repaints, `rate` = paints per 1000 passes, `clob` = a window blitted over the strip,
`presses/raises/unhides` = click outcomes. Final line 2183: `passes=548 paints=3 rate=5/1k
… clob=0 presses=0 raises=0 unhides=0`. Three paints = three model changes (fixture window →
`paints=0` at line 141 (one pass, model empty), console window → `paints=1` at 456, pulse window +
focus → `paints=3` by 570), then no model change for 430 s, so no paint. The dock is painting when
it has something to paint and idle otherwise; `passes` climbs on every line (37 → 548), so it is
being scanned every pass. Not stuck.

`[orinclick] census` is `display_tegra.rs:1441` `orin_click_census`, printed from INSIDE the input
drain loop every `CLK_CENSUS_PERIOD = 40` sweeps of 250 ms (`t=` climbs by 40 per line, `up=` by
10 s). 43 lines, seq 1..43, `t=70..1750`, `up=10..430s`, all `btn=0 press=0 rel=0 … stuck=0 →
IDLE-NO-CLICKS`. Per the verdict ladder (`:1501-1513`) `btn==0` means the button counter never
moved: nobody clicked during the boot. The router is alive (the line is its own liveness proof,
comment at `:1145`), armed at line 583 (`rows=3 compat=0 focus=0x0 pidesk=1 -> ARMED`), and
nothing is stuck (`stuck=0` would otherwise print `FAIL reason=stuck-focus`). The mouse did
enumerate (`MOUSE-1: 1 reports, last dx=0 dy=0 buttons=0x00`, line 319 — one report at
arming); no button edge ever reached the router because no button was pressed.

## 5. Two `presents` counters

* `[wc-b] rollup presents=N` — `video/wm.rs:18789`, counter `CB_PRESENTS` bumped in `band_note`
  (`:18703`) from `stage_window` — **every staged WINDOW present** of any row (fixture, console,
  pulse), i.e. the compositor's count of window surfaces staged to the back buffer. It climbs with
  the console's ~1/s mirror traffic: 1 → 27 (line 536) → 167 (at the flight's 924 lines) → 632
  (line 2179) — 632 presents / ~430 s ≈ 1.5/s, matching `[wcn] win=2 att≈6 per 5 s` plus the
  pulse window's drag-in band repaints.
* `[orinrender] census … presents=2` — `main.rs:8351`, the local `presents` in
  `orin_render_service` bumped at `:8323` only when the render task's OWN pass was dirty and it
  called `pal.render()`: pass 1 (strip seed) and pass 2 (pulse window open). It counts the status
  strip's Screen presents, not window composites.
* Consistent: `[wc-w] rollup presents=1 … -> HONOURED` (line 519) and `presents=3` (line 580) is
  `video/screen.rs:705`'s Screen-present ledger (`desk_amp_flush`, printed at most once per second
  at a Screen flush): 1 after the render task's pass 1, 3 by the JD4 handover at 581 (pass 2 plus
  the console pump's first Screen flush). It brackets the render task's 2. 632 vs 2 is two
  instruments measuring two different things, both right.

## 6. The `PIUSB` family on the Orin

Lines 380 (`[piusb35] databuf …`), 383-386 (`[piusb25] storage enumerated … 63404032 blocks
(30959 MiB)`, LBA0 read-proof, boot-sector sanity), 387 (`[piusb34] LBA0 re-read post-invalidate`).
Printer: `drivers/xhci/mod.rs:11982, 12028, 12047, 12062` inside `service_storage`, guarded on
`target_arch = "aarch64"` only (comment at `:12013-12020`: "PIUSB-25: Pi mass-storage enumeration
+ LBA0 read-proof witness. aarch64-gated"). It fires on any aarch64 board with USB mass storage.

**The device:** `Disk 'Generic-' 'USB3.0 CRW -SD' block_size=512 num_blocks=63404032 (30959 MiB)`
(line 373) — a Realtek USB 3.0 card reader (0bda:0326, "CRW-SD", SuperSpeed, route 0x1 tier 1)
behind the Realtek 0bda:0489 SuperSpeed hub on root port 1, with a ~32 GB SD card in it. It is
NOT the SoC's SDMMC slot. Whether it is the boot volume cannot be settled from this log: the loader
identifies the boot volume by FAT serial `0xde001a13` (line 4) and the PIUSB probe only reads LBA0
(a boot-code-less MBR, first 16 bytes zero, 0x55AA present — the shape of a GPT protective MBR).
`arch_arm64.md:8400` records "the boot stick sits BEHIND [the 0bda:0489 hub]", so a USB boot volume
behind this hub is the documented bench topology. Expected and healthy (CSW Passed, residue 0, both
reads agree).

**Naming:** a `PIUSB`/`piusbNN` family printing from the shared xHCI driver on the Orin is exactly
the class Peter ruled on 2026-09-03 (shared-file tokens carry the subsystem, never a board). It is
GATE-NEUTRAL for this flight — the FLIGHT already counted it as a census item, not Pi traffic — and
belongs on the rename list (`PIUSB` → a USB-storage subsystem token) for whichever seat owns
`drivers/xhci/mod.rs`. Not this arc's file.

## 7. Timing

Clock sources in the capture (no timestamps on the wire): (a) `[wcn] rollup span=` chains from
`WCN_LAST_MS=0`, so the running sum is `arch::ms()` = CNTVCT/31250 since counter start;
(b) `X200 … t=<cntpct>` at 31.25 MHz; (c) `[orinrender] census` one per second of CNTPCT;
(d) `[wc-h] rollup … age_ms=` per window from its first sighting; (e) `[orinclick] up=` from arming.

| line | event | T (SoC counter) |
|---|---|---|
| 1 | loader anchor `KELF …` | not clocked (loader lines carry no time) |
| 121 | timer heartbeat live | < 22.47 s |
| 148 | first `[wcn]` rollup (fixture window) | 22.473 s |
| 176 / 206 | X200 xHCI pointer-programming / RS=1 | 22.725 s / 23.039 s |
| 378 | storage ready | 22.7 < T < 28.0 |
| 431 | 5/5 secondaries online | 22.7 < T < 28.0 |
| ~452 | console window first seen (win=2 origin) | ≈ 27.5-28.0 s |
| 465 | second `[wcn]` rollup | 28.055 s |
| 488 / 494 | `orinconwin -> ROUTED` / `RENDER-ARMED` | win-2 age 0.56-0.64 s → ≈ 28.2-28.7 s |
| 521 | first `RENDER-LIVE` (census #1, passes=1) | win-2 age ≈ 0.9 s (census #2 at +1 s lands before age 2.93 s at line 553) → **≈ 28.5-29 s** |
| 528 | pulse window open | ≈ 29 s |
| 550 | CAPSTONE COMPLETE | ≈ 29-30 s |
| 564 | `[wcn]` rollup | 33.817 s |
| 581 / 583 | JD4 console owns panel / `[orinclick] arm` | 8 census lines after JD2 (line 508) → JD2 + 8 s, as the JD2 line promises ("~8 s") |
| 2187 | last `[orinclick]` census | up=430 s (≈ 470 s on the SoC counter) |

**Loader anchor → RENDER-LIVE:** the loader phase is not clocked, so the honest statement is:
RENDER-LIVE at ≈ 28.5-29 s on the SoC counter, which counts from SoC reset through MB1/MB2/UEFI;
the kernel's first clocked line is at 22.47 s, and the whole kernel bring-up from heap allocation
(line 128) through xHCI, storage, SMP, EL2→EL1 drop, console routing and the first render pass is
≈ 6-6.5 s. (The butler's "83.8 s" at line 15 is capture wall-clock over 2732 unidentified lines and
includes the two earlier loader runs in §8; it is not a boot duration.)

**Gaps > 5 s:** none. (i) `[orinrender] census` prints once per second of CNTPCT and 443 lines
cover 430+ s: every 10-s `[orinclick]` interval contains exactly 10 census lines (42 of 42 full
intervals); the passes delta per census is 356 k..583 k on every line, so the render task was
never descheduled for more than a fraction of a second. (ii) `[wc-h] win=2 age_ms` advances by
≈ 3000 per emission (516 → 439034 over 157 emissions) with no jump. (iii) `[wcn] span=` maxima are
6000 ms, which is the dirty-paced rollup period (≥ 5 s, emitted at the next present), not a stall.
(iv) The only interval in the boot without a ruler is the loader phase (lines 1-120), which no
clock covers.

## 8. Things a careful reader flags that no question asked

1. **Two dark boots of a foreign image preceded render2 in the same session.** raw.log 100418 and
   101322 are loader runs with the OLD identity wording (`main.rs@743: Kernel ELF … max_vaddr=0x23b968
   pages=572`), boot volume FAT serial `0xabfbdefa`, JB9d in its pre-guard form (`MBOX MSG_ENABLED
   SILENT`), each followed by the ExitBootServices event and then silence until an MB1 `Coldboot`
   (a power-cycle). The third power-on booted render2 from `0xde001a13`. This is the "foreign
   volume" already on the jetson resume (`unaos-jetson-resume.md:52-55`) — pre-existing-known — but
   it means the board can still be steered onto that volume by the firmware's boot order, and two
   of today's three power-ons were. `FLIGHT-RESULT.md` says "One boot. Peter power-cycled."; the
   accurate count is three power-ons, one of which reached the kernel. Worth a line in the flight
   record and, if the stale volume is the card in the USB reader (§6), worth erasing it.
2. **The console mirrors every witness line to glass at ~1.2 presents/s for the life of the boot**
   (§3, §5). Harmless on this image, but it is what keeps `[wc-b] presents` and `[wcn] win=2`
   climbing on an idle desktop and what drags the pulse window (N1). A quiet desktop on this board
   is not silent on the compositor.
3. **`[comp2] max_us=188361`** (line 567): one composite pass of 188 ms during the pulse-window
   open + CAPSTONE window; `[wc-g]` samples cost 7-10 ms each (`cks_blit_us=7334` for the console's
   953 k probes) and `wit_us=109856` per `[wc-g] rollup` — the checksum witness is a large share of
   compose time on a `witness` image. Not a defect; a reason not to read `compose_us` on a witness
   image as the metal-image number.
4. **Hard-coded `(Slot 1 EP 1)` label** in the descriptor-intercept banner (`xhci/mod.rs:4328`)
   printed for slots 3 and 6 — cosmetic, bundled under N3.
5. **The `>>> SYSTEM ALERT: NEW HARDWARE DETECTED <<<` banner** (lines 239-242 etc.) is lore voice
   in a kernel witness. Not a defect; noted because `CLAUDE.md` confines that voice to CODEX/MEMORIA.
6. **`JB1c — CLK 14 enable` is issued twice** (lines 100 and 103; also twice in the JB7 census) —
   the DTB lists clock 14 twice for XUSB and the code walks the list verbatim. Harmless (both
   `err=0`), pre-existing.

## 9. Method

Extraction: `unwrap80.sh raw.log > raw-unwrapped.txt` (104414 lines), `sed -n '102226,$p'` →
`render2-full.log`. Sweep: awk family census (56 families), awk pattern sweep with the periodic
families' known-zero counters excluded and then separately asserted zero, per-window `[wcn]`
extraction, ruler extraction for §7. Every printer cited was read in `077a8fa1`. The repo was not
edited, nothing was built, the bench was not touched.
