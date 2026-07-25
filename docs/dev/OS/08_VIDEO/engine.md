# The graphics engine (Vug) — primitive ledger

`vug` (`unaos/crates/kernel/src/vug.rs`) is UnaOS's software 3D demo **and** the
graphics engine's testbed. Canon (`docs/CODEX.md` §5): Vug is the sculptor, the
future 3D CAD app; a *vug* is a crystal-lined cavity in rock. The demo shows a
real-time, software-rendered rotating quartz crystal, and every new engine
capability lands here with a visible artifact.

This file is the **running ledger of engine primitives** — what the renderer can
do, where each primitive lives, and its invariants. It grows as the engine does.

---

## 0. THE METRICS RULE (standing directive — UI-1)

**No absolute pixel sizes in UI code.** One metrics layer
(`unaos/crates/kernel/src/ui.rs`) derives an integer `scale` from the panel's
real height (`scale = clamp(height / 900, 1, 4)` — 1 at ≤ 900p, 2 at
1800p-class panels such as the rMBP Retina, capped at 4), and **every** UI
dimension derives from the resulting [`ui::Metrics`]:

| metric | derivation | meaning |
|---|---|---|
| `cell_w` / `cell_h` | `8 · scale` | the glyph cell; the text advance; **the cursor is exactly one cell, by construction** |
| `line_h` | `cell + cell/2` | text row pitch (1.5-line rhythm); the input-line clear strip |
| `margin` | `line_h` | page inset of screen-owning views |

With a PAL in hand, `pal.metrics()` (a provided `GneissPal` method) returns the
surface's metrics; they are a pure function of panel height — nothing to
initialise, nothing to go stale. `pal::draw_text` is scale-aware: each glyph
pixel renders as a `scale`×`scale` block (the scale-1 path is the classic
per-pixel loop). `Console::page_rows` — the single source of truth for page
height, which `selftest::Pager` shares — computes from the derived `line_h`.

Evidence line, printed once per surface bring-up (`TargetPal::new`), on every
target, headless-visible:

```
:: UI1: scale=N cell=WxH line=H ::
```

The one deliberate exception: the pre-heap boot console (`video/fbcon.rs`)
keeps its own unscaled 8-px font — it exists before the allocator and is out of
the GUI's life cycle. Anything else that names a pixel count is a bug.

> **Arch-neutral, float-free.** `vug` compiles on x86_64 and aarch64 and is
> reachable from the Orin panel shell (JD2), so it carries no `cfg(tegra)` and
> uses no floating point: all geometry and maths run in **Q16.16 fixed point**
> (`type Fx = i32`; the integer value is `real * 65536`) off a 256-entry *brad*
> sine table (256 brads = one turn).

### 0a. The GUI status strip (PI-UI-2)

`unaos/crates/kernel/src/ui_status.rs` draws an always-on one-line status bar
pinned to the **bottom** line of the panel, surfacing the net/time state a bench
user would otherwise only read on serial:

```
unaos.local   ip 192.168.2.3   2026-07-22 14:31:07 UTC
```

with honest placeholders before the state settles: `no lease` until a bring-up
records an address, `unsynced` until SNTP seeds the wall clock. All geometry
derives from `ui::Metrics` (THE METRICS RULE — the band is one `line_h`, the
glyph cell is centred within it); nothing names a pixel.

**Read-only by construction.** The strip consumes only public snapshot
accessors — `clock::now()` for the wall clock and `net_phy::settled_ipv4()` for
the interface address. The latter is a lock-free atomic snapshot recorded by
`net_phy::dhcp_or_static` (the single chokepoint every arch's bring-up funnels
through), so the render core reads the address without reaching into any
driver's private `NetService` state and takes no net lock in the render path.
The hostname is the fixed mDNS/DNS-SD name `unaos.local` (net11/net17).

**Refresh.** On the Pi bare-metal path the render task (`render_service`,
`main.rs`) draws the strip **after** the console each frame (so it always sits on
top) and presents. A tiny `status_tick` task posts an `Event::Timer` to
`GUI_CHANNEL` once a second (mirroring `rx_backstop`), waking the render loop to
re-draw so the clock advances and a late lease appears without a keystroke. It is
timer-gated at spawn (its `sleep_ticks` nap needs the live timer IRQ); in QEMU
raspi4b, where no Group-1 IRQ is delivered, the tick task is not spawned and the
strip refreshes on input instead. Bring-up witness, printed once by
`render_service`:

```
:: UI2: status strip armed (host+ip+time, 1 Hz) ::
```

---

## 1. Drawing primitives (in `video/` + `pal.rs`)

Added additively — the existing `fbcon`/`Console`/`Screen` contracts (the x86 GUI
and the JD2 panel shell ride them) are unchanged.

| Primitive | `FrameBuffer` | `Screen` (damage-tracked) | `GneissPal` |
|---|---|---|---|
| `draw_line(x0,y0,x1,y1,color)` | Bresenham, per-pixel clipped | + bounding-box damage | default via `draw_pixel`; `TargetPal` → back buffer |
| `fill_triangle(a,b,c,color)` | — | scanline raster + bbox damage | default via `draw_line`; `TargetPal` → back buffer |

Endpoints are signed (`i32`) so off-screen geometry clips cleanly. All drawing
goes to the `Screen` back buffer (cached RAM); `pal.render()` presents the damaged
span **once per frame**. On the Orin the present does one `dc cvac` clean over the
damage span (the DCE scans the carveout from DRAM and does not snoop) — so large
per-frame damage is fine, but there must be exactly one present per frame.

## 2. Fixed-point 3D (in `vug.rs`)

- `Fx` (Q16.16), `fmul`, `fsin`/`fcos` (brad table lookup), `isqrt` (Newton, for
  normal normalisation).
- `Vec3` + `rotate(ay, ax)` — yaw about Y then pitch about X (two axes at
  different rates read as a deliberate tumble).
- Crystal geometry: an elongated hexagonal bipyramid (a quartz point) — 14
  vertices (two apexes + two hex rings), 24 outward-wound triangles.
- **Pipeline:** rotate every vertex → perspective project to pixels (`ppu =
  focal * dist / (z + dist)`) → backface cull by **screen-space signed area**
  (this projection flips Y, so a front-facing outward-CCW triangle has negative
  area) → painter's-order depth sort (a convex solid needs only depth order) →
  per-face integer **Lambert** shade (`N·L / (|N||L|)`, ambient floor + diffuse)
  scaling a deep-amethyst base, lilac seam highlights.

## 3. Interaction

`vug` runs an animation loop **inside** the shell command, so the outer console
pump is blocked. `pal::pump_and_poll()` lets the loop keep input flowing: it polls
the USB HID controller (x86 GUI + the Orin panel deliver keyboard/mouse through
xHCI) and, on aarch64, drains the UART, then returns one queued event. **Any key
exits** and the shell command restores the console (`took_screen` honored). The
loop `yield_now`s between frames and never sleeps (the post-EL2-drop aarch64 rule:
no timer to wake a sleeper). Modes: `vug` / `vug solid` (default), `vug wire`,
`vug bebox` (tribute screen).

> **Bare-metal Pi note.** On the Pi 4 bare-metal path keyboard input is routed
> through a separate scheduled input service / channel (`GUI_CHANNEL` in
> `main.rs`), not the PAL event queue, so `pump_and_poll` does not see it there.
> `vug`'s attended targets are the x86 GUI and the Orin panel, whose input rides
> the PAL queue; wiring the Pi channel into a full-screen demo is a future
> `main.rs`-side hook (out of this arc's lane).

## 4. Instrumentation — the load meters (M3b, redesigned UI-1)

Two small corner meters on the demo (the crystal stays the star), both drawn
through the damage-tracked back buffer (one present per frame still holds):

- **RENDER meter** — the honest software "GPU monitor" (there is no GPU; we render
  in software). Per frame it clocks the render+present span (`arch::now_cycles`)
  against the whole-frame span → a **busy %** bar, and times a ~200 ms window with
  `arch::ms()` → frame time / FPS; it also shows drawn triangles and an estimated
  filled-pixel count (sum of front-face screen areas). **Seam:** a real GPU
  utilization feed would replace `now_cycles`-derived busy % and the pixel
  estimate.
- **CPU pulse row** (UI-1, Peter's sketch) — one horizontal row of per-core
  **numbered segment bars**: `CPU 1 ▮▮▮▮▯▯ 2 ▮▮▯▯▯▯ …`, for however many cores
  `sched::meter_cpu_count()` reports. Each bar is a **fixed 10 segments**
  (`PULSE_SEGS`); filled ∝ load (rounded; any nonzero load lights at least one);
  **empty segments draw dim** — an idle core reads *alive-but-empty*, never
  blank. All geometry derives from the metrics (segments are half-cell wide, one
  cell tall in the corner form).

**The honest two-source rule (M3b, kept verbatim — do not regress).** Per-core
load lives in the shared `CpuPulse` sampler (`vug.rs`), used by both the corner
row and the full-screen `pulse` view. Per ~200 ms window, per core:

- scheduler-accounted core (`Δbusy+Δidle > 0`, i.e. the core is inside
  `sched::run()`) → the scheduler's busy fraction, from the additive relaxed
  per-CPU counters (`CPU_BUSY`/`CPU_IDLE` in both `arch/*/sched.rs`, introspection
  only). This is the Orin scheduled-pump path and the x86 APs.
- unscheduled executing core (counters frozen — the x86 GUI runs the demo in the
  inline BSP loop) → that core IS the demo core; credit it with the render loop's
  own measured busy-vs-yield fraction (the same number the RENDER meter shows),
  and log it once:
  `:: VUG|PULSE: CPU meter — core N is the demo core (unscheduled render loop, load from render busy%) ::`

**Seam:** a real per-core utilization / PMU feed would replace `meter_cpu_ticks`.

## 4b. `pulse` — the full-screen system monitor (UI-1 M3)

`pulse` (shell command; `vug::run_pulse`) is the in-kernel half of the BeOS
Pulse homage — a full-screen monitor view (a host-native `vessels/pulse` vessel is
a separate, future arc). It shows the M2 pulse widget **larger** (double-size
segments, one row per core, with the load percent), plus the honest system
lines available today: core count, uptime (`arch::ms()`), live frame counter,
and frame/present time + FPS measured while the view is open.

The loop follows the vug contract exactly: it runs inside the shell command,
pumps input itself (`pal::pump_and_poll`), presents **once per frame**,
busy-polls + `yield_now`s between frames (never WFI/sleeps — the post-drop
aarch64 rule), and **any key exits** back to the console (`took_screen`
honored; the shell repaints). Per-core loads ride the same `CpuPulse` sampler —
two-source rule and all. The bare-metal Pi input caveat from §3 applies to
`pulse` too.

Serial evidence on entry/exit (headless-gate visible when scripted):

```
:: PULSE: live — N cores ::
:: PULSE: exit clean — N frames ::
```

## 5. Serial evidence

`run_crystal` emits, when invoked:

```
:: VUG: crystal live — 24 faces, solid/wire, exit clean ::
:: VUG: crystal exit clean — N frames ::
```

Headless regression gates never type `vug`, so these are GUI/panel-verified when
the demo is run (attended); the demo does not perturb headless boots.

## 6. VPERF — the fbcon scroll path (x86)

### Root cause (stride-correct)

On the mid-2012 rMBP (MacBookPro10,1) the EFI GOP surface is 2880×1800,
**stride 4096 px**, 4 bpp, Bgr, `framebuffer_size` 29,491,200 B, scanned out by
the GT 650M — every CPU **read** of it is an uncached PCIe round trip.
`FrameBuffer::scroll_up` was a full-surface `core::ptr::copy` whose *source* is
that framebuffer: one 8-px text scroll moved `1800×(4096×4) − 8×16,384` ≈
**28.0 MiB read + 28.0 MiB written**; a full screenful (225 rows) ≈ **6.15 GiB
of uncached reads**. On top of that, glyphs and fills poked VRAM per pixel.
QEMU numbers for calibration (1280×800, stride 1280): one scroll moves
(800−8)×1280×4 = 4,055,040 B; the 400-line scripted scenario = 301 scrolls =
1,220,567,040 B, pre-fix all of it VRAM-read.

### The fix (M3: cached-RAM shadow, x86-only)

`fbcon` late-attaches a heap shadow (`attach_shadow()`, called at the post-heap
usbdebug seam right before the usbdebug `clear()` — fbcon itself initialises
pre-heap by design, so init-time allocation is impossible). Once attached:
glyphs and scrolls go to the shadow (cached RAM); `flush_dirty` presents the
dirty row band to VRAM as **one contiguous sequential write-only blit** (on a
scroll: the whole viewport — every visible row changes on a scroll, so the win
is *read elimination*, not damage shrink). The shadow is **never seeded from
VRAM**; it starts blank with the cursor homed. GUI builds never attach (fbcon
detaches; `Screen`'s ~28 MiB back buffer owns the 48 MiB heap budget — a second
shadow would OOM on metal). `panic_screen()` force-detaches the shadow without
freeing (`mem::forget` — no allocator use from a panic context) so the red
backdrop and panic text always paint direct-to-VRAM. Design choice: a minimal
`Vec<u8>` + second `FrameBuffer` handle (the `Screen` back-store invariant),
*not* `Option<Screen>` — fbcon already has row-band damage tracking, and
`screen.rs` is another lane's file this round. M4 adds `encode4`/`put_raw4`
(pixel encode hoisted out of glyph/fill inner loops) and a word-wide
`fill_rows` band fill; with the shadow attached all stores are full 4-byte
pixels and VRAM receives only whole blitted rows.

### Instrumentation (`videobench` / `videocap` knobs, x86-only, default OFF)

`UNAOS_VIDEOBENCH=1` → feature `videobench` → `video/vperf.rs`: relaxed-atomic
counters (scroll bytes memmoved / VRAM-read bytes / put_pixel calls), emitted
**raw-serial-only** (a line mirrored onto fbcon would recurse through the
scroll path and contaminate screendump compares):

```
:: vperf: scroll=<B> vread=<B> px=<N> ::                        (cadence)
:: vperf: fbmem mtrr=<T>(<how>) pte=<raw>(l<N>) pat=<T> eff=<T> fb=<addr> ::
:: vperf: display <vid>:<did> bar<N> owns fb (base=<addr>) ::
:: vperf: scenario scroll=<B> vread=<B> px=<N> scrolls=<N> lines=400 ::
```

The `fbmem` line is the **effective framebuffer memory type**: MTRR coverage
(RDMSR MTRRCAP/DEF_TYPE + variable ranges, CPUID-guarded), the live
identity-map leaf PTE (PWT/PCD/PAT bits visible raw), the IA32_PAT entry those
bits select, and the SDM 11-7 combination. The kernel programs **zero**
PAT/MTRR state — the WC assumption inherited from firmware
(`arch/x86_64/mod.rs`) is unverified, and **QEMU's picture is synthetic** (TCG:
`mtrr=UC(var-range) pte=0x800000e3(l2) pat=WB eff=UC`); only the attended metal
readout is data. The display probe is read-only (no BAR sizing writes on a
live-scanned-out panel; ownership = highest display mem-BAR base at/below the
fb address within 512 MiB). `UNAOS_VIDEOCAP=1` (implies videobench) halves the
fbcon-private handle's `info.height` — the bench lever that proves scroll cost
scales with viewport height (capping the `rows` field alone is a no-op:
`scroll_up` sizes from `info.height`). Full-surface paints (init fill, clear,
panic backdrop) still cover the whole panel via an uncapped twin handle.

The scripted scenario (usbdebug loop, one-shot ≥15 s so the boot fixtures are
quiet): clear + 400 fixed-width numbered lines + counter deltas. Deterministic
final screen → QMP screendump compares (`UNAOS_QMP_SHOT=<png> ./arroyo test 40`,
port auto-bumps). Measured: pre-M3 `vread=1220567040`, post-M3 **`vread=0`**,
identical workload, screendumps pixel-identical pre/post M3 and M4.

### Ledger: the scroll runs IF-masked under the console lock

The whole fbcon mirror — scroll included — runs interrupts-masked inside
`sys_write`'s serial path under the FBCON `try_lock` (`arch::without_interrupts`
in `serial::_print`/`fbcon::_print`). Post-M3 that window is still
~10 ms-class per scrolled line on metal (a full-viewport write-only blit), and
it **delays irqstorage IRQs** (and everything else) for its duration. Known,
accepted for now; redesigning console locking is explicitly out of the VPERF
arc's scope.

### Metal-confirmed result (round-6 attended rMBP bench, 2026-07-11) ✅

Verbatim readout on the real 2012 rMBP (over FTDI serial):

```
:: fbcon: cached-RAM shadow attached — VRAM is now write-only ::
:: vperf: fbmem mtrr=UC(var-range) pte=0x900000e3(l2) pat=WB eff=UC fb=0x90020000 ::
:: vperf: display 8086:0166 class 030000 @ 0:2.0 ::            (Intel HD 4000)
:: vperf: display 10de:0fd5 class 030000 @ 1:0.0 ::            (NVIDIA GT 650M)
:: vperf: display 10de:0fd5 bar1 owns fb (base=0x90000000) ::  (GT 650M scans out — gmux default)
:: vperf: scenario scroll=… vread=0 px=… scrolls=… lines=400 ::
```

- **The M3 cached-RAM shadow is METAL-CONFIRMED**: `fbcon: cached-RAM shadow attached` fired and the
  scroll scenario measured **`vread=0`** over 400 scrolled lines — zero uncached-VRAM reads on real
  hardware, matching the QEMU 1.22 GB→0. The shadow works on silicon.
- **`eff=UC` → VPERF-WC is a GO** (no longer bench-gated): the framebuffer is UNCACHED (var-range MTRR
  UC, PAT=WB unused in the l2 PTE), NOT write-combining, so the ~10× WC follow-up is real. The metal
  fact it needed is now in hand (UC; fb in the GT 650M's `bar1` @ `0x90000000`), so **VPERF-WC becomes a
  normal codable arc verified at the next bench**, not a bench-first one.
- GPU topology exactly as the interface model predicted: Intel HD 4000 (`8086:0166`) + NVIDIA GT 650M
  (`10de:0fd5`), the GT 650M owning the framebuffer via the gmux default.

### VPERF-WC — write-combining the framebuffer mapping (x86-only, landed post-round-6)

The M3 shadow removed the *reads* (`vread=0`); VPERF-WC accelerates the *writes*. The round-6
metal readout showed the framebuffer still **effective-UC** (var-range MTRR UC, PAT=WB in the l2
PTE), so the shadow's write-only sequential blits were posted **uncombined** — each store its own
uncached PCIe transaction. Marking the framebuffer **Write-Combining** lets the CPU's WC buffers
coalesce those sequential stores into burst transactions (~10× on the write path is the metal
expectation the next attended bench measures).

**Mechanism** (`arch/x86_64/memory.rs`, called from `fbcon::init`):

- `ensure_pat_wc()` programs one **unused IA32_PAT slot** — PA4 — to WC (encoding `0x01`). Power-on
  PAT is `[WB,WT,UC-,UC]` in entries 0–3 and *duplicates* them in 4–7; no firmware/kernel mapping
  ever sets a PTE's PAT bit, so entries 4–7 are dead. Writing only PA4 leaves 0–3 — every live
  mapping — byte-identical. Programmed on the **BSP**; idempotent.
- `set_framebuffer_wc(base, len)` retypes every identity-map **leaf** covering the fb range to
  select PA4 — set the PAT bit (bit 7 in a 4 KiB PTE, **bit 12** in a 2 MiB/1 GiB leaf), clear
  PCD/PWT — under `with_page_tables_writable` (the WP-clear page-table-edit sequence), then
  `invlpg`s the span. Runs **once**. `wbinvd` is **not** issued: the fb was effective-UC, so no
  cache line holds fb data to write back.

**Scope (seat-signed).** This is a **memory-TYPE change only**, on the **fb leaves only**. It writes
no page-permission bit (PRESENT/WRITABLE/USER/NX), no MTRR, and no other mapping — **SMEP/NXE/W^X
are untouched** (they live in CR4/EFER and the U/W/NX PTE bits, none of which this writes). The
firmware maps the fb with 2 MiB huge leaves inside the GPU's own BAR aperture (device MMIO, not
RAM/heap/kernel), so the retyped span *is* the framebuffer's own mapping.

**All x86 builds** carry the retype (GUI blits benefit too), un-gated by `videobench` — it rides
`fbcon::init`, where the fb range is known. aarch64 is **byte-identical** (every symbol is
`cfg(target_arch = "x86_64")`; flat-image equivalence holds under the ratified standard —
default-features virt + `-Ccodegen-units=1` tegra flat images hash-identical base↔tip).

**APs.** An AP that also blits keeps its default PA4=WB, which under the UC MTRR is effective-UC —
so an AP blit is plain UC (no speedup). This is **correct, not merely tolerated**: WC and UC are
both uncacheable (no cache line holds fb data), so the mix is *not* the SDM 11.12.4 WB-aliasing
hazard — it is exactly the write-only-framebuffer pattern, just un-accelerated on the AP. Wiring
`ensure_pat_wc()` into `smp::ap_entry` for uniform WC is a one-line follow-up, left out to keep the
arc in-lane.

**WC drain — SFENCE at every flush seam (F1, seat-review must-fix).** WC stores buffer in the CPU's
write-combining buffers and drain only on a store fence / serializing event / buffer pressure
(SDM Vol. 3A §11.3.1). A streaming console self-evicts under pressure, but the LAST partial WC
buffer before the CPU idles has no eviction trigger — the tail of the panic screen ahead of
`hlt_loop()` is the concrete failure surface, and QEMU/TCG cannot witness it (WC buffering is not
modeled). Fix: the x86 `arch::flush_framebuffer_range` (previously a no-op) now emits **SFENCE**,
so every existing flush seam is a real drain point: fbcon's per-print `flush_dirty` (after the
shadow blit), `panic_screen()`'s `flush_all` plus each subsequent panic-text print, and `Screen`'s
per-frame present. Range-independent (SFENCE drains all WC buffers); negligible next to the blit.
aarch64's flush (`DC CVAC` + `DSB`) is untouched.

**Witness.** The `videobench` `fbmem` readout is the on-target witness (QEMU + metal): the retype
flips it from `pte=…00e3 pat=WB eff=UC` to `pte=…10e3 pat=WC eff=WC`, and a boot line
`:: x86 fb-wc: retyped N leaf(s) WC (PAT PA4) over <base>..<end> ::` fires in every x86 build.
QEMU (synthetic bochs fb) already reflects `eff=WC` after the retype — it proves the **code path**;
only the attended rMBP bench proves the **speed**. QEMU screendumps are **pixel-identical** pre/post
(same SHA-256): WC changes write buffering, not final memory content.

### VPERF-WC — metal-confirmed result (round-9 attended rMBP bench, 2026-07-12) ✅

Verbatim on the real 2012 rMBP (over FTDI serial; knob-ON usbdebug+videobench media, kernel.elf
sha `3083b467`; TWO clean boots):

```
:: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
:: vperf: fbmem mtrr=UC(var-range) pte=0x900010e3(l2) pat=WC eff=WC fb=0x90020000 ::
:: vperf: display 8086:0166 class 030000 @ 0:2.0 ::            (Intel HD 4000)
:: vperf: display 10de:0fd5 class 030000 @ 1:0.0 ::            (NVIDIA GT 650M)
:: vperf: display 10de:0fd5 bar1 owns fb (base=0x90000000) ::
:: vperf: scenario scroll=5167382528 vread=0 px=486060 scrolls=176 lines=400 ::
```

- **The retype LANDED on silicon.** The fbmem readout FLIPPED from round-6's `pte=…00e3 pat=WB eff=UC`
  to **`pte=0x900010e3(l2) pat=WC eff=WC`** — the fb mapping is now effective Write-Combining on the
  GT 650M's `bar1` @ `0x90000000` (15 huge leaves retyped). The `vread=0` shadow win still holds.
- **~10× scroll — subjective PASS (attended eyeball).** The 400-line scripted scroll rendered
  **dramatically faster** than round-6's eff=UC boot on the same panel — the WC write-coalescing win
  is visible, matching the ~10× expectation.
- **GUI blit win — 53.8 fps (attended, Boot 2 GUI).** The vug/quartz scale-2 Retina render clocked
  **53.8 fps**, up from round-6's ~7.6 fps (~7×) — WC accelerates the GUI present path too, exactly as
  "all x86 builds carry the retype" predicts. `:: x86 fb-wc: retyped 15 leaf(s) … ::` fired on the
  GUI build too.
- **WC panic-tail (F1 SFENCE drain) — metal PASS (attended eyeball, Boot 2 GUI).** A deliberate
  `panic` at the GUI console rendered the FULL red panic screen (`=== KERNEL PANIC === / Manual Panic
  Requested by Architect!` + location) with **NO truncated tail** — the last partial WC buffer drained
  via `panic_screen()`'s `flush_all`→SFENCE before `hlt_loop()`, confirming the F1 fix on real WC
  hardware (the one thing QEMU/TCG cannot witness). The FTDI cable shows nothing after the trigger —
  expected: the panic halts the main loop that pumps `service_ftdi`, so the tail is screen-only.

**Spec:** `round6-rmbp.spec` + `x86-fat.spec` carry the `eff=WC` / `fb-wc: retyped` PENDING lines —
seat promotes to REQUIRE at this first metal capture.

**VPERF-OBS — the raw_print console-path lesson (fix `bfa1711`, 2026-07-11).** On the FIRST bench boot
NONE of the `:: vperf: …` readout lines printed on metal, though QEMU had shown them all. Root cause:
`vperf::raw_print` wrote ONLY the SERIAL1 (16550) UART leg, which is `None` on the rMBP (no 16550 — the
FTDI console exists precisely because of that), so every readout line was computed and silently dropped;
the surviving lines used `serial_println!`, which hits the FTDI boot-capture ring leg. The fix has
`raw_print` also feed `ftdi::mirror` (the ring leg), and NEVER the fbcon or selftest mirrors — both
raw_print constraints preserved (try_lock-only, never the FBCON lock; QEMU logs byte-unchanged, since
the UART leg already reaches `serial.log` and the ring is not replayed there). **The lesson: QEMU-visible
≠ metal-visible — a diagnostic must assert the console PATH it will actually reach on the target, not
merely that its code path ran.**

## 7. Headless render-path witnesses (VWIT)

The two drawing primitives (§1) reach the framebuffer by two different paths, and each has its own
`tste` regression check (`unaos/crates/kernel/src/selftest.rs`; arch-neutral, so both fire on the x86
and aarch64 headless boots when `tste` is typed):

| tste row | path exercised | where |
|---|---|---|
| `video.geometry` | the **trait-default** `draw_line`/`fill_triangle` (Bresenham / scanline) via `GneissPal::draw_pixel` | `OffscreenPal` (a raw heap pixel buffer) |
| `video.present` | the **damage-tracked `Screen`** override: `mark` bbox accumulation, the `flush` present, and `FrameBuffer`'s pixel-format encoder | `video/witness.rs` (a `Screen` over a heap-backed offscreen `FrameBuffer`) |

`video.present` (VWIT) exists because the `Screen`/`TargetPal` override — the **steady-state on-screen
renderer** — was previously verified only *visually* by `vug` and the GUI console (attended). It builds
a `Screen` over a `Vec<u8>` (passed as the surface's raw base — `FrameBuffer` holds its base as `usize`
to stay `Send`) and asserts the real present path:

- **Format decode** — a known three-distinct-channel colour lands in `Bgr` byte order (`[B,G,R,0]`) in
  the front buffer; a raw byte-order spot-check catches an Rgb/Bgr swap the round-trip word alone would
  hide. (The `OffscreenPal` path bypasses this encoder.)
- **Damage-limited present** (the load-bearing assertion) — a sentinel poked into the front buffer
  *outside* the next draw's bbox **survives** the `flush`; a disjoint `fill_rect` is presented. Proves
  `mark`/`flush` blit only the damage rectangle, not the whole surface — the invariant `video.geometry`
  cannot observe.
- **No-op flush** — a second `flush` with no new damage leaves the front byte-stable (idempotent).
- **Clip safety** — signed off-screen `draw_line`/`fill_triangle` endpoints clip in-bounds without
  panic/overrun; the clipped line paints the on-screen span, the triangle fills a bounded interior.

Serial evidence (headless-gate visible; drive `tste` with `scripts/qmp_type.py --text tste --enter`):

```
:: VWIT: render present — format=Bgr damage=OK noop=OK clip=OK ::
:: TSTE: video.present -> PASS ::
```

These are QEMU-provable regressions only — a real GPU/present is not involved; on metal the same rows
run under `tste` and mirror to whatever console the target exposes (§4/§6 console-path rule applies).

## 8. WC — the window compositor (`video/wm.rs`, WC-A)

Before WC, `SYS_FB_PRESENT` had exactly one destination: `screen::present_surface` centered the
caller's single 32x32 surface on the panel. There was no window state anywhere, so two EL0 programs
could not be on screen at once. `video::wm` is the seam that fixes that.

### The table

A fixed array of `MAX_WINDOWS = 8` rows, statically allocated behind one `spin::Mutex`. The
compositor runs from syscall context on a non-coherent scan-out path, where a heap allocation would
be both a latency cost and a failure mode. Each row carries: id (`1..=8`; `0` is `WIN_NONE`, the
fail-closed return of every operation), owner ASID (opaque here — `wm` never reads task state),
content origin and source extent, integer scale, z-order, surface pointer + stride, a damage flag,
and a title truncated to `MAX_TITLE = 16` bytes at create time.

### Composite on present, no thread

`present(id)` marks the row damaged and composites inline, from the presenting task's own context.
This is the discipline `present_surface` has always used and it is forced: while a full-screen EL0
program owns the panel the render task is parked in `dispatch_command`, so routing through it would
present nothing. There is no compositor thread in this arc.

Two properties the pass has to get right:

- **Occlusion closure.** Repainting a damaged window would erase anything stacked on top of it, so
  the damage set is closed upwards to a fixed point — every higher-z window whose outer box overlaps
  a damaged one is repainted too, transitively — and the draw order is ascending z (ties by id, i.e.
  creation order).
- **No lock across the scan-out.** The table lock is held only to snapshot the rows and clear their
  damage flags. Every framebuffer write and every `flush_range` happens after it is released. `place`
  reads the panel geometry *before* taking the table lock, so no path ever holds both locks and there
  is no order to invert.

Each composited window gets one `flush_range` over the scanlines of its outer box — one `DC CVAC`
sweep per window rather than one per row, the same present discipline as `Screen::flush`. A no-op on
coherent targets.

### Chrome is kernel-drawn, always

Each window gets a 1-px border and a `TITLE_H = 12` title strip, painted by the compositor from the
kernel's own copy of the title (8x8 `font8x8` glyphs; non-printable bytes render as spaces). Apps
draw only inside their own surface. A hostile EL0 program therefore cannot forge another window's
frame or paint a convincing system dialog, and the presentation-modes law — never fake host chrome —
holds structurally rather than by convention. Colours are flat and deliberately un-host-like.

### Layout

`place` tiles the non-compat windows left-to-right in id order with an 8-px gap, wrapping to a new
row when the next outer box would run off the panel, and picks each window's scale as the largest
integer factor fitting half the panel in each axis (so a 32x32 surface is legible on a 1920-wide
panel and two windows still sit side-by-side). Layout depends only on the live set, not on the order
of creates and closes, so it is deterministic. `move_to` pins a window out of the automatic layout.
`close`/`close_owner` erase the vacated box, relayout and recomposite; `close_owner` is what task
teardown calls, so a dead ASID can never leave a window compositing from a freed address space.

### The compat shim

`screen::present_surface` keeps its geometry math verbatim (legacy centering + the UVUG-7 integer
scale rule) and hands the blit to `wm::compat_present`, which creates one `compat` window on first
call and updates it in place afterwards. A compat window draws **no** chrome and flushes exactly the
rows `[y0, y0+dh)` the legacy path flushed, so pre-window apps are byte-identical: the `kernel8-test`
panel capture matches the pre-WC baseline `pi-screen.png` sha256 exactly, with MBENCH 46/46 and 0
forbidden.

### Serial evidence (`witness` feature)

```
[wc-a] composite windows=1 drawn=1
```

That is the line a `kernel8-test` run emits today: the compat window presenting through the shim. The
other three formats are **not yet exercised** — nothing calls `create`/`close` until WC-B lands the
window syscalls — so they are given here as formats, with the values a 32x32 surface on the QEMU
640x480 panel would produce (scale = `min(640/2/32, 480/2/32)` = 7, origin = the first tile slot):

```
[wc-a] create win=1 asid=0x1 surf=32x32 stride=128 scale=7x at (9,21) z=1
[wc-a] close win=1
[wc-a] close_owner asid=0x1 closed=1
```

`composite` witnesses the **first** pass only — a mini-vug run presents ~300 frames and per-frame
lines would drown the log (the same reason `[uvug2]` fires once).

### Treating window geometry as hostile

Everything the app supplies — `w`, `h`, `stride`, the `move_to` origin — is attacker-controlled once
WC-B exposes the verbs, and the compositor is the one place that turns those numbers into memory
reads. The memory-safety lens on WC-A found six defects here; all are fixed, and the invariants they
established are the ones to preserve:

- **Surface-extent contract.** `create` takes `surf_len`, the real byte length of the mapped slot,
  supplied by the mapping code and never derived from EL0 dimensions. Geometry that would read
  outside it (`w * 4 > stride`, `h * stride > surf_len`, saturating) is rejected at create. Without
  this, a `w=h=10000, stride=40000` window over a 4 KiB slot has the compositor read ~400 MB of EL1
  memory and paint kernel bytes onto the panel — `put_pixel` clips *writes*, never the source read.
- **Panel-clipped loops.** Every composite loop is clipped to the panel intersection before it runs.
  Per-pixel clipping keeps the writes safe but still iterates: the same hostile window would spin
  ~1e8 clipped pokes per present, from syscall context.
- **Saturating geometry.** The kernel builds with overflow checks off, so `w * scale` wrapping would
  silently produce a small damage box for a large paint. All of it saturates, and `move_to` clamps
  the origin on both bounds against the panel.
- **Ids are slot aliases.** There is no generation counter, so "is this still my window?" cannot be
  answered by liveness: a closed row's id is immediately valid again under a new owner. The compat
  shim therefore keys on the row's `compat` flag — a property no recycled row can accidentally have —
  and re-checks it under the lock. (A generation-widened `WinId` was the alternative; the flag is
  exact for this hazard and keeps the id a plain slot number for WC-B's syscall ABI.)
- **Teardown drains in-flight blits, behind a phase barrier.** `composite` checks a drain flag and
  registers itself as in-flight in the *same* table-lock critical section, so both are ordered
  against any later teardown's lock acquisition. `close`/`close_owner` clear their rows, raise the
  barrier, and spin until the in-flight count reaches zero. A composite that takes the lock while the
  barrier is up skips entirely — correct, since the rows it would have drawn are gone and teardown
  recomposites when it is done, and *necessary*, because it is what makes the wait terminate: the
  in-flight count can only fall while the barrier is up. A plain "wait for idle" loop would not
  terminate under continuous presents, and the teardown path (`sched::exit` → `clear_handle_row` →
  `close_owner`) spins IRQ-masked and unpreemptible, so that livelock would be a dead core rather
  than a slow one. Today the underlying race is a stale read; under WC-B's per-ASID surface mappings
  it becomes an EL1 abort mid-blit.
- **The compat window's check-and-create is serialised.** `COMPAT_WIN` alone is check-then-act: two
  `SYS_FB_PRESENT`s on different cores could each create a compat row, and the loser's would be an
  ownerless, unreferenced window nothing can close — F3 again, through a race.
- **The compat window has a lifecycle.** It has no owner ASID, so `close_owner` can never reap it;
  `wm::close_compat()` exists for that and WC-B must call it from the EL0 teardown seam. Otherwise
  the row is immortal and every later composite re-blits a dead app's buffer.

### Seam for the window syscalls (WC-B)

`SYS_WIN_CREATE` / `SYS_WIN_PRESENT` / `SYS_WIN_MOVE` / `SYS_WIN_CLOSE` are thin fail-closed wrappers
over `create` / `present` / `move_to` / `close`; the per-ASID ownership gate reads `owner_of`, and
`clear_handle_row` calls **both** `close_owner` (for the ASID's own windows) and `close_compat` (for
the ownerless shim row). `create` must be passed the mapped slot's real byte length as `surf_len`.
`SYS_FB_MAP` / `SYS_FB_PRESENT` stay as compat wrappers over the compat window.

### WC-INT — the wired seam

The two units are joined. WC-B's `WINDOWS` table stays authoritative for id allocation and ownership;
`wm::create` mints its own `WinId` out of its own table, and WC-B stores that id in the row's `wm_id`
field. The two id spaces are never assumed to line up — every later `wm` call goes through `wm_id`.
`WIN_NONE` is a legal value everywhere, so a window the compositor refused (full table, or geometry
the extent contract rejects) simply has no compositor presence and its verbs still succeed at the
WC-B layer. `surf_len` is WC-B's `pages * 0x1000`, the real mapped-slot byte length, never a
recomputed `h * stride`.

`SYS_FB_MAP`'s compat row deliberately gets **no** `wm` window (`wm_id` stays `WIN_NONE`): the compat
window is minted lazily by `compat_present` and reaped by `close_compat`. A `wm::create` there would
mint a chrome-bearing, tiled window and the pre-WC UVUG panel output would change.

**One forwarding, not two.** Before integration, both the syscall present body and
`screen::present_surface` ended in a present. Now the two cases are exclusive: `wm_id != WIN_NONE`
damage-marks and composites through `wm::present` and never touches the ELF-3 hook; `wm_id ==
WIN_NONE` forwards to the hook, whose target computes the legacy geometry and hands off to
`compat_present`. Both sit *below* the focus-guarded `EL0_FOCUSED_PRESENT_COUNT` bump and the
`FB_PRESENT_COUNT`/checksum witness, so the UVUG-8 suspension cap and the ELF-3 fb-test `present == 1`
verdict are driven by one accounting block whichever path renders.

#### Lock order across the seam

The global order is **`WINDOWS` ⊃ `wm::TABLE` ⊃ `video::WRITER`**, and it is verified, not assumed:

- `WINDOWS` (WC-B's `SpinMutex`, always taken IRQ-masked) is the outermost lock. `sys_win_present`
  holds it across the whole present *including* the composite — deliberately, so the id handed to the
  compositor is provably the id the ownership gate validated (a `close` + `create` pair on other cores
  could otherwise recycle the id in the gap and land the caller's pixels under a new owner's identity).
- Nothing in `video/wm.rs` or `video/screen.rs` references the syscall layer at all, so **no path
  acquires `WINDOWS` from inside `wm`**. The edge is one-way by construction, and the cycle that would
  make the held-across-composite hold dangerous cannot be formed.
- The `TABLE ⊃ WRITER` half is stronger than stated: the two are **never held simultaneously**. Every
  `WRITER` acquisition in the lane is the statement form `let fb = *WRITER.lock();` — `FrameBuffer` is
  `Copy`, so the guard is a temporary dropped at the end of that statement, and what survives is a
  value. `move_to`, `erase`, `composite`, `place` and `present_surface` all read the panel geometry
  that way *before* or *after* their table critical section, never inside it. So a present never holds
  the window lock across a scan-out cache clean, and there is no order to invert.
- `COMPAT_CREATE` sits strictly outside `TABLE` (it is taken only in `compat_present`, around the
  check-and-create) and is never taken while `TABLE` is held.

**The drain barrier is safe under this order.** `close`/`close_owner` spin until in-flight composites
finish. Every caller invokes them with `WINDOWS` *released* — `sys_win_close` and `win_close_asid`
collect the `wm_id`s inside the lock and destroy outside it, and `clear_handle_row` calls
`close_owner`/`close_compat` after `win_close_asid` has returned. Even if a drain *did* run under
`WINDOWS`, it would still terminate: a composite blocked waiting on `WINDOWS` has not yet reached
`wm`, so it has not incremented `BLIT_ACTIVE` and is not a member of the drain set. Registration
happens inside `wm::TABLE`, strictly after `WINDOWS` is acquired. Combined with `DRAIN_PENDING` —
which makes any composite that takes `TABLE` after the barrier goes up skip without registering — the
wait set is fixed at entry, finite, and every member is running a bounded panel-clipped blit. **The
drain set is closed.** That matters because teardown (`sched::exit` → `clear_handle_row` →
`close_owner`) spins IRQ-masked and unpreemptible: a livelock here would be a dead core, not a slow one.

Teardown order is WC-B first, then `wm`: `win_close_asid` unmaps the surfaces and frees the rows, then
`close_owner` sweeps any compositor row whose WC-B row a racing `sys_win_close` already freed, and
`close_compat` reaps the one row that has no owner ASID to match on.

### WC-C — the desktop, the clients, and focus

WC-A built the compositor and WC-B the verbs; WC-C is the arc where real programs use them.

**The desktop is painted, not left blank.** `wm::erase` filled vacated boxes with black, which on a
panel is a *colour*, not an absence: every closed window left a black rectangle over the console's
Moonstone background and the kernel chrome's close-box left a black hole mid-desktop. `erase` now fills
`wm::DESKTOP_BG`, and the WC-INT panel residue is gone — the `kernel8-test` capture is a clean desktop
plus the status strip. `DESKTOP_BG` restates the console's private `Console::BG` rather than importing
it, so the compositor owns its own theme value (the crispy theme will replace it with real desktop
data); a drift between the two is immediately visible as a rectangle where a window used to be.

**UVUG is windowed.** `crates/user-uvug` no longer maps the 32×32 compat surface. It calls
`SYS_WIN_CREATE(128, 128)` — `boot::FB_WIN_MAX_W/H`, exactly one 64 KiB window slot — reads its surface
at its own window base + `0x5000` (window region slot 0, the VA `SYS_FB_MAP` used to return), and
presents with `SYS_WIN_PRESENT`. `FOCAL` scales 6 → 24 so the crystal keeps the same framing at 4× the
linear resolution. The UVUG-8 takeover/cap line is unaffected: `sys_win_present` runs the *same* present
body as `sys_fb_present`, so the focus-guarded `EL0_FOCUSED_PRESENT_COUNT` bump still happens per
window, and the QEMU run still reaches `exit=0` through `run_user_image`'s deadline.

> **Spec change, deliberate.** The 300-frame auto-path checksum is a pure function of the surface, so a
> 128×128 surface necessarily produces a new one: `:: UVUG: frames=300 threads=2
> checksum=0xe68285b85121ac7c ::`, replacing the 32×32 `0x48221e4101db3924`. This is the *second* of the
> two options the brief allowed — the spec is updated with the new value rather than the geometry being
> kept compat — because a compat shim here would mean shipping the 32×32 render forever to protect a
> constant. The invariant that *is* preserved byte-for-byte is the **compat path**: `SYS_FB_MAP` +
> `SYS_FB_PRESENT` still produce the identical centred, chrome-less blit (the ELF-3 fb test's
> `mapped=32x32 … checksum=0x8d99530ca96d4b25` is unchanged).

**midden has a window.** `crates/user-blob/src/midden.rs` creates a 24×16 window and renders its own
bus stats into it: two rows of four hex digits (the live witness bitmask, and the number of legs passed)
from a 3×5 bitmap font packed one `u16` per glyph, blitted 1:1 at EL0. The kernel draws the border and
the title strip and *nothing else* — app content is app-drawn, chrome is kernel-drawn, and neither can
forge the other. The compositor's integer upscale (13× on the QEMU 640×480 panel, ~30× on a 1920-wide
one) is what makes it legible, so midden never learns the panel geometry. A refused `SYS_WIN_CREATE`
makes every repaint a no-op, so midden's bus witnesses never depend on the compositor. Fitting this in
the flat blob's single 4 KiB code page needed the crate's release profile to move from `opt-level = "s"`
to `"z"` (3792 B, ~300 B of headroom); the per-blob page assertion in `arroyo kernel8` is what catches
a regression.

**Focus is a key.** TAB is reserved *by the window system*, intercepted at `el0_input_enqueue` — the one
choke point every event bound for an EL0 ring passes through, so no app can hold focus hostage by not
implementing it. It walks `wm::focus_ring` (the distinct owner ASIDs of the live non-compat windows, in
window-id order — a stable rotation, not a reordering stack) and hands focus to the next entry via
`el0_input_set_active`, which is still the only way focus moves: the incoming ring is reset, the
interactive-takeover latch is cleared, and the UVUG-8 cap therefore keeps holding *per window*. The
matching KeyUp is swallowed on the same predicate (a lone release edge for a press the app never saw is
exactly the shape UVUG-6 removed from the typematic path). With fewer than two windows in the ring the
key falls through as an ordinary TAB.

The ring carries **one slot beyond the windows: the shell** (`EL0_INPUT_ACTIVE == 0`). Without it the
cycle is a closed loop over the live apps — an operator who tabs into a window can never get the keyboard
back, and the wedge watchdog becomes the only exit from a perfectly healthy app. So "no app has focus" is
a position in the rotation, not an absence.

**WC-TAB closed the loop.** WC-C shipped the shell slot as a one-way exit: with focus 0,
`route_input_to_active_el0` is not called at all (`main.rs` gates on `el0_input_active() != 0`), so no
TAB reached the seam — the ring could be left but not re-entered. The shell's own event drain in
`pump_usb_into_gui` now calls `syscall::wc_shell_focus_key` before forwarding anything to
`GUI_CHANNEL`. That is a second *entry point* onto the same `wc_focus_key` body, not a second
implementation: same predicate, same `el0_input_set_active` move, same `[wc-c] focus tab-cycle` witness.
With focus 0 in no window's slot the cycle takes its "unknown focus" arm and lands on the ring's head,
the first window in window-id order. The `n < 2` guard is shared, so with one window (or none) TAB
remains an ordinary key at the shell too — deliberate symmetry, since a lone window does not consume TAB
and pushing focus into it would re-create the trap. Nothing was clobbered: the console's `handle_key`
ignores byte 9 outright, so TAB had no shell binding before this.

#### The side-by-side witness

The arc's claim is two windows composited together. A screenshot shows that to a human and proves
nothing to a gate, and the `[wc-a] create` lines say only that rows *existed*. So `composite` emits, once,
from inside the pass that actually drew them:

```
[wc-c] side-by-side windows=2 drawn=2
[wc-c] win=1 asid=0x1 surf=128x128 scale=1x at (9,21) z=3 cksum=0xfabe809492cf2325
[wc-c] win=2 asid=0x1 surf=64x64 scale=3x at (147,21) z=4 cksum=0x591f6cbe80502325
```

The per-window checksum is FNV-1a over `surf_len` — the mapping-code length, the same bound
`draw_window` reads under — so a window that is present-but-blank, or that composited a stale or
recycled surface, is distinguishable from one that drew real content.

**Honest limit on what drives it.** The boot's real programs cannot overlap: `uvug_witness` and
`bandy_rt_launcher` each run their program to completion, so UVUG's window and midden's window are never
live at the same instant. The vehicle is therefore the `el0-wcb` fixture, widened from 10 witness bits to
13: it creates a second 64×64 window filled with a different byte and presents it **while the first is
still live and still holding its content**, which is the only state in the boot where two real windows
composite in one pass. Window A is re-presented before the closes, so `FB_PRESENT_CHECKSUM` at verdict
time is still A's 128×128 `0xC3` surface and the pre-WC-C verdict line is unchanged. Two live EL0
programs at once is a scheduler question, not a compositor one, and is left to a later arc.

#### WC-C gate results (2026-07-24, QEMU raspi4b)

`./arroyo check` green both arches · `kernel8` builds (midden 3792 B ≤ 4096) · `kernel8-test 120`
MBENCH **49/49 required, 0 forbidden** (the arc adds three `pi4-regression.spec` directives — the UVUG
checksum, the `witness=0x1fff` ledger, and the side-by-side line, which is what gates the per-window
checksums below it) · `:: EL0: window verbs — … witness=0x1fff … :: PASS ::` ·
`:: EXEC-UVUG: … exit=0 -> PASS ::` · all four BANDY verdicts PASS.

`target/pi-screen.png` is **re-baselined by design** — the WC-INT residue this arc removes was the whole
point of the desktop repaint. New sha256 `2686a884320dbc389d6c33b1f37b097fa15eba769b51a751449e2c91a986bc19`.
