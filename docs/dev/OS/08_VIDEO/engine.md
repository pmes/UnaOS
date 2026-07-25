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
panel and two windows still sit side-by-side), **capped at a legibility ceiling**. Layout depends
only on the live set, not on the order
of creates and closes, so it is deterministic. `move_to` pins a window out of the automatic layout.
`close`/`close_owner` erase the vacated box, relayout and recomposite; `close_owner` is what task
teardown calls, so a dead ASID can never leave a window compositing from a freed address space.

#### The legibility ceiling (WC-SCALE)

The fit rule alone is a function of the *surface*, so the smaller the surface the larger the
magnification — midden's 24×16 status readout landed at `scale=37x` on the 1920×1200 bench panel,
which is ~100 panel pixels per font pixel and reads as an abstract pattern rather than as digits.
`wm::legibility_cap` bounds it: `scale ≤ ui::SCALE_MAX · ui::Metrics::for_height(panel_h).scale`.

Both halves are pre-existing answers, not new constants — THE METRICS RULE holds, no absolute pixel
size appears. `SCALE_MAX` is already the kernel's stated limit for magnifying bitmap content
("beyond 4× legibility gains nothing and glyph blocks get blocky-huge") and `Metrics::for_height` is
how every other UI dimension tracks the panel, so the ceiling is *four font-scale steps*: 4× on a
≤ 1799-row panel, 8× on an 1800p-class one. It is the same **kind** of cap `screen::present_surface`
applies on the compat path (there: ~40 % of the panel's shorter dimension); that path keeps its own
rule, because a compat surface is a full-screen app's whole canvas and wants to be as large as it
comfortably can, whereas a tiled window is one of several and wants to be as *readable* as it can.

The cap shrinks the window's outer box, so tiling stays exactly as deterministic as before — no
letterboxing, no reserved slot. Nothing here can move a checksum: `[wc-c]`/`[wc-d]` `cksum=` is FNV
over the **source** `surf_len`, which no scale change touches. Concretely: on the 640×480 gate panel
128×128 stays 1× and 64×64 stays 3× (both already under the cap, so the gate log is byte-identical);
on the 1920×1200 bench panel 128×128 stays 4×, 64×64 comes down 9× → 4×, and 24×16 comes down
37× → 4× (96×64 panel pixels — a status readout at a readable size).

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

**UVUG is windowed.** `crates/user-vug` no longer maps the 32×32 compat surface. It calls
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
TAB reached the seam — the ring could be left but not re-entered. `pump_usb_into_gui` now calls
`syscall::wc_shell_focus_key` from both of its non-routing paths. That is a second *entry point* onto
the same `wc_focus_key` body, not a second implementation: same predicate, same `el0_input_set_active`
move, same `[wc-c] focus tab-cycle` witness. With focus 0 in no window's slot the cycle takes its
"unknown focus" arm and lands on the ring's head, the first window in window-id order. The `n < 2` guard
is shared, so with one window (or none) TAB remains an ordinary key at the shell too — deliberate
symmetry, since a lone window does not consume TAB and pushing focus into it would re-create the trap.
Nothing was clobbered: the console's `handle_key` ignores byte 9 outright, so TAB had no shell binding
before this.

Which path carries it is the whole substance of the fix. `handle_key` sets `SCREEN_APP_ACTIVE` around
`dispatch_command`, and `run_user_image` parks the shell task for the entire EL0 run — so while apps are
live *and* focus is at the shell, both the `SCREEN_APP_ACTIVE` branch and the shell drain are in play,
and the former returns first. The interception therefore sits inside that branch's existing
peek/requeue scan: a consumed TAB is simply not requeued, and on a real focus cycle the rest of the
buffer is dropped rather than forwarded (a swallowed release edge keeps the buffer — it is requeued and
the scan continues). It must not be sent onward — `render_service` is blocked inside the same
`dispatch_command`, so pushing into the 64-slot `GUI_CHANNEL` would saturate it and block the pump task,
exactly what that branch exists to prevent. Dropping the buffer is not a new policy either: it is what
`el0_input_set_active` would have done to those same events itself, since it drains `pal::EVENT_QUEUE`
on every real focus change; they are outside the queue only because the uncounted peek is holding them.

**Scope, plainly:** as WC-C already conceded, the boot's own programs do not overlap in time, so a ring
of two or more windows arises today only under the `el0-wcb` fixture that creates them deliberately.
This is the mechanism made whole and symmetric — not yet a workflow a metal operator falls into. A
single-window boot sees no behaviour change at all: TAB stays an ordinary key end to end.

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

### WC-D — the scan-out verdict, and why a 640×480 gate could not see the bench

The P56 bench reported the 128×128 windowed crystal rendering **garbled on the panel** while every serial
witness was green. That combination is the whole problem: `[wc-c]`'s per-window checksum hashes the SOURCE
surface, so it proves the app drew something and says nothing whatsoever about the pixels that reached the
panel. Everything between the surface and the scan-out — the upscale's indexing, `put_pixel`'s colour
encoding, the clip, and the cache clean that makes the non-coherent HVS see the result — was unwitnessed.

**Why the gate was blind by construction.** QEMU raspi4b's mailbox panel is 640×480; the bench Pi drives
1920×1200. `wm::place` derives each window's integer upscale FROM the panel, so those are not the same code
path: a 128×128 window lands at **scale 1** on 640×480 and **scale 4** on 1920×1200. The gate was running a
1:1 copy while the bench ran a 4× nearest-neighbour expansion — every scaled-blit defect was invisible to it
for free. `mailbox::FORCE_W` / `FORCE_H` (`UNAOS_FBW` / `UNAOS_FBH`, default off) close that gap: they
override the firmware-queried mode so the gate can be run at the bench's geometry.

**The instrument.** `wm::verify_window` runs once per window, from inside the composite pass that drew it,
and re-derives every destination pixel of the content rect from the source surface, comparing it against the
scan-out buffer **twice**:

| verdict | meaning |
|---|---|
| `bad_cache=0` | the compositor computed the right pixels — stride/pitch arithmetic, upscale indexing, colour encoding, clipping |
| `bad_cache>0` | the **blit** is wrong, or another painter overwrote the rect mid-composite; `first=` names the pixel |
| `bad_ram>0` | the pixels did not reach the memory the HVS scans — a cache-maintenance defect |

The line also carries `cksum=` (the `[wc-c]` FNV over the source slot) and `nonzero=` (destination pixels that
are not black), so a blank surface faithfully blitted onto a blank rect is distinguishable from a verified
crystal instead of reading as an equally green PASS. `first=none` is printed on a clean verdict, so a real
black-on-black mismatch at the origin cannot hide behind an all-zero placeholder. A window refused by a
guard emits `-> SKIP` with its reason rather than silently consuming its one-shot latch.

**`bad_ram` uses a bare `DC IVAC`, and that is load-bearing.** The first cut of this witness used
`DC CIVAC`. It was wrong in exactly the way that mattered: `CIVAC` writes dirty lines back before
invalidating, so had `draw_window`'s trailing `flush_range` been missing or short, the witness would have
cleaned those very lines to RAM and then read the repaired result — printing `bad_ram=0` for a panel that
garbles. The falsifier is concrete: delete the `flush_range` call and the `CIVAC` form still passes. A bare
`IVAC` discards un-cleaned lines instead, so a short flush surfaces as stale RAM.

**The falsifier was run, and it is INCONCLUSIVE off-metal — say so.** The `flush_range` call in
`draw_window` was deleted and the gate re-run at forced bench geometry; the verdict stayed
`bad_cache=0 bad_ram=0 -> PASS`. That is **not** evidence the instrument is broken — it is evidence that
QEMU raspi4b does not model a non-coherent framebuffer at all, so CPU stores reach guest memory whether or
not `DC CVAC` ran, and `bad_ram` has nothing to detect. The consequence is worth stating plainly: **the
`bad_ram` column is unvalidated in BOTH directions off-metal.** Its correctness rests on the primitive being
right by inspection (`IVAC` discards, `CIVAC` would have repaired), not on a passing gate. The next bench
boot is its first real exercise, and a `bad_ram=0` there is the first datum that means anything.

> **Hazard — an instrumented build is not a neutral observer.** `IVAC` discards anything dirty-and-unclean
> in those full scanlines, which in a correct build is nothing (`draw_window` just cleaned a strict superset)
> but in a broken one can drop pixels — and the invalidated extent is full-width panel scanlines, not the
> window's columns, so the concrete exposure is `fbcon`'s deferred dirty band (`mark_rows`/`flush_dirty`):
> unflushed console glyphs on those rows are discarded and the redraw restores only the window, not them.
> `verify_window` therefore redraws and re-flushes the window before returning. Consequence: in the presence
> of a flush defect a `witness` build can differ visibly from a default build, in either direction. Do not
> read a witness build's panel as evidence about a default build's panel.

**What the gate established, and what it did not.** At the bench's exact geometry — `panel=1920x1200`,
`surf=128x128 scale=4x at (9,21)`, matching the P56 log line for line — the verdict is
`checked=262144 bad_cache=0 bad_ram=0 -> PASS`. What that **earns** is the `bad_cache` half: the scaled
blit's indexing, the stride/pitch arithmetic and the pixel format are correct at the geometry that garbled,
so those three suspect classes are excluded. `bad_ram` passing on QEMU earns nothing about metal — QEMU does
not model the Pi's non-coherent scan-out, so that column is only meaningful on the bench, which is precisely
what it is there to report.

**Flush extent is excluded by inspection, not by `bad_ram`.** `draw_window` flushes rows
`[by, by+bh)` of the window's `outer_box`, which spans the title strip and both borders, and `flush_range`
works in whole scanlines at the panel's full stride — a strict superset of every pixel the blit touched.
That argument, not the QEMU verdict, is what rules the extent out.

**Scope of the pitch claim.** Verified against the firmware-reported pitch of 7680 B for a 1920-wide,
4-bytes-per-pixel panel — i.e. pitch == width × bpp with no padding. A bench panel whose firmware returns a
*padded* pitch is not covered by this run; `put_pixel` and `flush_range` both derive from `info.stride`, so
the arithmetic is expected to hold, but it is untested at a padded pitch.

**One thing the rect is not guarded against.** `composite` copies the `FrameBuffer` handle and releases the
window-table lock before drawing, so nothing prevents another core's painter (the console, the render task)
from writing into the rect between the blit and the verification. Such an overwrite is reported as
`bad_cache>0` and is indistinguishable from a genuine blit defect; `first=` is the disambiguator to reach for.

The boot-time "indecipherable" auto-launch surfaces were a **separate, cosmetic** matter and not this defect:
`wm::place`'s scale rule maximised legibility without bound, so midden's 24×16 surface landed at `scale=37x`
on a 1920×1200 panel — a few characters at enormous magnification, exactly as designed. **WC-SCALE fixed
that with the cap described under Layout above** (`wm::legibility_cap`), not with a correctness change.

#### WC-D gate results (2026-07-25, QEMU raspi4b)

`./arroyo check` green both arches · `kernel8` builds · `kernel8-test 120` MBENCH **50/50 required,
0 forbidden** (the arc adds the `[wc-d] … -> PASS` REQUIRE and the `-> FAIL` FORBID) · `test-arm` green ·
and, at forced bench geometry (`UNAOS_FBW=1920 UNAOS_FBH=1200`), `[wc-d] verify win=1 surf=128x128 scale=4x
at (9,21) panel=1920x1200 checked=262144 bad_cache=0 bad_ram=0 -> PASS`.

### WC-E — the garble was never in the pixels: two writers, one scan-out, no ordering

WC-D left the defect cornered and unexplained. The composited pixels were byte-correct in the RAM the HVS
scans (`bad_cache=0 bad_ram=0 nonzero=full` on every window, at the bench's exact geometry), and the panel
still garbled. The remaining suspects were all scan-out geometry — pitch, depth, pixel order, virtual size,
viewport offset, base alignment. WC-E retired every one of them and found the cause somewhere else.

**What the evidence actually said.** Three facts, taken together, leave exactly one explanation.

1. **The geometry is self-consistent and correct.** P57 metal: `pitch=7680B` for a 1920-wide 32-bpp panel
   (`= width × bpp`, no padding), `size=9216000` (`= 1920 × 1200 × 4`, exact). Nothing to shear a row
   against. Suspect 1 is dead on the wire, and 3 and 4 with it.
2. **The console renders correctly while the windows garble.** Console text and window content go through
   the *same* `put_pixel`, the *same* `info.stride` and the *same* framebuffer base. Any scan-out geometry
   error is common to both and would shear the text identically. A defect that afflicts one and not the
   other cannot live in the geometry — it must live in what is *different* about the two paths.
3. **The composited bytes are identical on QEMU and on metal.** The gate at forced bench geometry produces
   the same `[wc-c]` source checksums as the P57 metal boot, window for window
   (`0xa1cf4a91b6138449`, `0xfabe809492cf2325`, `0x591f6cbe80502325`, `0x377ffc1fbd89557d`). The content is
   not corrupted anywhere. Identical bytes, different panel ⇒ the divergence is **temporal**, not spatial:
   the question is not what was written, it is who wrote *last*.

And point 2's difference between the two paths is the answer. **The console is back-buffered; the windows
are not.** `Screen` owns a back buffer in cached RAM, and `flush` copies its damaged rectangles into the
scan-out. The window compositor writes windows **directly** into the scan-out from the presenting task's
syscall context — `screen::present_surface` documents this choice, and reasons it sound for the case it was
written for (a *full-screen* EL0 program parks the render task, so nothing else is flushing). WC-C's
*windowed* apps break that premise: the desktop keeps rendering alongside them. The two writers never knew
about each other, and nothing ordered them.

The bench log measures the collision precisely. `[vugfps] 20.3 fps 3996743 bytes/frame flushed rects=4
union=1920x1200` — the render task presents ~20 times a second with damage unions spanning the **entire
panel**, for the whole life of the boot. So: a window presents; the compositor pokes it into the scan-out;
`verify_window` reads it back microseconds later and correctly reports `bad_cache=0 bad_ram=0`; and then,
within 50 ms, the desktop flush copies desktop content over every pixel of it. Repeat at 20 Hz, in four
parallel bands whose boundaries fall wherever the scheduler puts them, and the window region on the panel
is a shimmer of window content and rotating VUG scene. "Noise-like indecipherability" is exactly what that
looks like. Every witness stays green because every witness reads the framebuffer back *inside the present
that wrote it*, long before the next desktop frame lands. WC-D even named the mechanism as an unguarded
possibility — another core's painter writing into the rect — and only missed it because it looked for the
overwrite *during* the verification window rather than after it.

**The fix is the layering the code always implied.** The desktop is the background layer; the window
compositor is the layer above it. `Screen::flush` now presents the background and then calls `wm::repaint`,
which marks the live windows damaged and re-composites — background, then windows, in one call, so
"windows are above the desktop" is continuously true instead of true only until the next frame.
`composite` already closes the damage set upwards over occlusion, so the restored stack is correct
back-to-front.

**COMPAT windows are excluded, and that exclusion is load-bearing.** A compat row is the full-screen
present path, and while a full-screen EL0 program owns the panel the render task is parked and is not
flushing — there is no second writer to order against, so a repaint there fixes nothing. It also is not
free: the first cut repainted compat rows too, and re-blitting UVUG's 32×32 surface at 15× on every
desktop frame pushed its 300-frame run past the `EXEC-UVUG` deadline —
`45/51 required, 6 forbidden` (the UVUG timeout cascading into all four BANDY verdicts). Scoping the
repaint to *real* (windowed) rows is what the collision actually requires and what restores the gate. On
a windowless desktop frame the whole mechanism costs one table-lock acquisition and a return.

> **Residual, stated plainly.** Window pixels are now overwritten and repainted *within the same present*
> rather than never being overwritten. A scan-out that lands between the two steps can still catch a window
> mid-repaint, so a single-frame tear remains possible. Removing that needs the flush to skip the rows a
> window owns, or a fully double-buffered composite; what this change removes is the *unbounded, every-frame
> erasure*, which is the difference between a window that flickers and a window that is unreadable.

**`[wc-e]` — the scan-out ground truth, on every boot.** `init_framebuffer` programs five setters and then
keeps the values it *requested*; the firmware is free to clamp, round or refuse any of them. `mailbox::
witness_fb_geometry` now asks the firmware what it actually settled on — `GET_PHYS_WH`, `GET_VIRT_WH`,
`GET_VIRT_OFFSET`, `GET_DEPTH`, `GET_PIXEL_ORDER`, `GET_ALPHA_MODE` — and prints one unconditional line
carrying `req=` beside every firmware answer, plus base, size, base alignment, and two derived identities:
`row_ok` (`pitch == virt_w × bpp`, the identity a row-phase garble breaks) and `fit_ok` (the allocation
holds the whole visible image). Query-only tags; a failed call prints a FAIL line and returns, because a
diagnostic must not be able to take down the boot it is diagnosing.

This exists because of a specific blind spot WC-D had. `verify_window` reads the framebuffer back through
the **same `info.stride` it wrote through**, so it agrees with itself no matter what the display pipe is
doing: a witness that asks our numbers can never falsify our numbers. `[wc-e]` asks the firmware's. When it
agrees with `req=`, the entire scan-out geometry suspect surface is retired for that boot and anything
downstream can be blamed honestly; when it diverges, it names the field.

#### WC-E gate results (2026-07-25, QEMU raspi4b)

`./arroyo check` green both arches · `kernel8` builds · `kernel8-test 120` at BOTH default (640×480) and
forced bench geometry (`UNAOS_FBW=1920 UNAOS_FBH=1200`), MBENCH **51/51 required, 0 forbidden** each
(the count includes this arc's own `[wc-e]` REQUIRE; the lens re-measured both) · `test-arm` green. The pre-fix
baseline run at the same geometry is what established fact 3 above (identical `[wc-c]` checksums to P57
metal). **The fix's effect is not observable in QEMU** — the collision is a race between the render task
and the presenting task, and the gate's screenshot samples one instant — so bench verification at the next
Pi 4 boot is what confirms it, with the `[wc-e]` line as the standing geometry check on the same wire.

### WC-F — an independent read of what the HVS actually scans

WC-E fixed a real two-writer ordering bug, and the bench panel garbled the 128×128 crystal anyway on the
P58 boot. Content byte-correct in RAM (WC-D), writers ordered (WC-E), garble persisting: the remaining
surface is scan-out-side, and it is the side both prior witnesses are constitutionally unable to see. Both
address the framebuffer through `info.stride`, the same number the blit writes through.

**What independence is not.** The first cut of this arc failed its own review, and the failure is the most
useful thing in this section. It compared `stride × bpp` against the firmware's pitch — but
`init_framebuffer` *defines* `stride = pitch / 4`, so those are the same number for every possible
firmware, and the check could not fail. It then re-asked `GET_PITCH` and compared that to the allocation's
pitch: the same firmware answering the same question, which confirms the reply and nothing about the
hardware. Both twin paths ended up computing byte-identical offsets, `comp_bad`/`direct_bad` were
structurally zero, and the photograph's "left garbled, right clean" arm was unreachable. **A witness whose
failing branch cannot be reached is decoration.**

Two constructs here are genuinely independent of the pitch reply:

**1. A row step derived from geometry.** `virt_w × 4` is what one row of the reported virtual width
occupies. It equals the pitch when the firmware allocates unpadded and diverges *by the padding* when it
does not — so `rowstep_match` (`stride × bpp` vs `virt_w × 4`) is a real question with a reachable `false`,
and `pad=` states the divergence in bytes. The right-hand twin block is addressed with it, so on a padded
framebuffer the two blocks genuinely land at different addresses and the panel shows it.

| field on `[wc-f] scanout` | the identity |
|---|---|
| `base_match` | the mapping we store into **is** the buffer the firmware allocated |
| `rowstep_match` | `stride × bpp` equals `virt_w × 4` — false exactly when the firmware pads a row the compositor ignores |
| `pad` | that padding, in bytes per row |
| `pitch_match` | the allocation pitch survives an independent re-query (weak alone; a divergence would be decisive) |
| `panel_match` | the geometry `wm::place` lays windows out in is the mode being scanned |
| `fits` | the mapping covers every row the pipe scans |

**2. A marker whose photographed slope IS the hardware's row step.** Every number on the serial wire —
ours and the firmware's alike — is downstream of the firmware's own claim, so no serial line can observe
*"the HVS steps differently than it reported"*. The ramp can. It writes one two-pixel mark per row by pure
byte arithmetic, each `k_row + 4` bytes after the last. If the hardware advances a row by exactly `k_row`,
the marks photograph as a diagonal moving exactly one pixel right per row; if its real step is `k_row + d`,
the marks drift `d/4` px per row and the diagonal visibly bends. 256 rows, ticked white every 16th so rows
can be counted off the photo. **This is measured off the panel, not off the log.**

**The twin probe.** One known 16×16 pattern rendered twice at the bench's 4× upscale, side by side: left
through `put_pixel`/`info.stride` (byte-for-byte `wm::draw_window`'s inner loop), right through raw stores
stepped by `virt_w × 4`. Each block is cross-read through the other path's addressing. The direct path
stores **three** bytes, exactly as `put_pixel` does, so the blocks cannot differ for the non-addressing
reason of one having zeroed the fourth byte under an alpha mode that reads it.

- **left garbled, right clean** ⇒ the compositor's addressing; the defect is in the blit path.
- **both garbled** ⇒ the HVS or the pitch; the blit is faithfully filling a surface nobody scans right.
- **both clean, crystal still garbled** ⇒ neither addressing nor geometry; the defect is specific to the
  window path (surface contents at scan time, or a writer WC-E did not order).

In the **divergent** case the right-hand block deliberately leaves its rectangle: `pad > 0` means
`geom_row < k_row`, so each row lands at a lower offset and the block drifts *upward* out of the bottom
strip. That is the displacement the probe exists to make visible — clamping it would erase the finding —
so it is accepted rather than fixed, at the cost of the overlap guard's coverage on a padded boot.
`direct_lands=(col,row)` states where the block's first row actually comes out under the kernel's row step — a padded boot displaces it both vertically and sideways — so
the operator knows which part of the panel to photograph and which garble is the probe's own.

**A comparison that cannot be made is counted, never dropped.** An out-of-range offset increments
`skipped`; `PASS` requires `skipped == 0`, `lost == 0`, and `checked` equal to the full expected count
(8192). Without that, a probe whose every read fell outside the mapping would report `comp_bad=0` — which
reads exactly like agreement, the one way a witness can lie.

**Every line is one-shot, and a retryable condition never prints SKIP.** The probe fires from a path
that runs hundreds of times per boot, so this is not tidiness. `SKIP` is reserved for **terminal** causes
— no framebuffer, no recorded firmware truth, a layout with no 4-byte RGB pixel, a panel too small — which
nothing later in the boot can change; it prints once and latches. A window sitting over the probe strip is
**retryable**: it emits a one-shot `-> DEFER` (which the spec deliberately does not forbid) and the probe
keeps trying every composite. Conflating the two broke twice over: a run that deferred early and passed
later would carry both lines and trip the unconditional `FORBID … -> SKIP` while being perfectly healthy,
and a window that never moves — a full-screen compat surface — would emit one line per composite pass,
unbounded. The geometry report latches independently of both, so it fires on the first pass regardless.

**Cost and collision discipline.** The probe runs only from composite passes that already drew something,
so its ~12 K stores and row cleans fall where work is happening anyway rather than being charged to every
idle repaint — it instruments the path it runs in, and perturbing that path is a real cost. It refuses to
paint while any live window overlaps its region (it paints last, so it would win, and a window under it
would silently show WC-F's pattern instead of the app's content). Both marks sit in the bottom strip —
twins hard right, ramp hard left — because stacking them put the ramp where `wm::place` lands its first
window at 640×480, and a witness that habitually skips is no witness. The cleaned ranges are invalidated
**separately, never as one convex hull**: when `geom_row > k_row` the hull spans rows this probe never
wrote, and a bare `DC IVAC` over those would discard another writer's still-dirty lines — manufacturing
the exact garble we are hunting. The one-shot latch is set only by a pass that produced a verdict, so a
transient skip cannot silence the witness for the boot.

`witness`-gated **and** aarch64-only: knob-off, `video/wcf.rs` does not compile, the flashable Pi media
are byte-identical (zero `wc-f` strings), every x86 artifact untouched.

Firmware is not re-queried from composite context — the property mailbox has one unlocked static buffer
and is safe only during single-core boot — so the read happens at bring-up and is recorded
(`mailbox::scanout_truth`).

#### WC-F gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

`./arroyo check` green both arches · `./arroyo kernel8` clean, zero `wc-f` strings (armed build: six) ·
`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 120` → MBENCH **55/55 required, 0 forbidden**, on a
spec grown by this arc's two REQUIREs. Both verdicts PASS.

A QEMU PASS carries no news and is not the arc's evidence: QEMU allocates unpadded, so `geom_row == k_row`
there and the divergent branch is unexercised by construction. What the gate establishes is that the probe
runs, addresses both paths, counts what it could not check, and states a verdict — so a bench divergence
appears as a number rather than as silence. **The evidence is the metal boot.**

Carried forward from WC-E and still unactioned: the P58 boot reported `alpha=0` on metal where QEMU reports
`alpha=2` (ignored). `[wc-f] scanout` repeats the field so the divergence stays on the wire, but this arc
does not act on it — alpha mode cannot shift a row's phase, and the compositor composites opaquely.

### CURSOR-1 — the system cursor

The Pi 4 mouse pipeline reached the shell in PIUSB-39 / UVUG-10 (reports decoded, routed, `drop=0`,
click-to-exit working on metal) with **no cursor on the panel**: the operator was pointing blind. The
sprite code existed — `pal::cursor` has owned an arrow, a hot spot and an auto-hide clock since
CURSOR-HIDE — but it draws through a `GneissPal`, i.e. into whatever surface the caller owns, and on the
windowed desktop that surface is the render task's `Screen` **back buffer**. `wm::composite` blits windows
straight into the front framebuffer, so a back-buffer sprite is on top of the console only, until the next
present. `video/cursor.rs` draws where "on top" is structural instead: into the front framebuffer, as the
last painter of every pass.

**The split of duties is the point.** `pal::cursor` remains the pointer's STATE — position (`move_rel` /
`set_abs`, fed from the same `pal::EVENT_QUEUE` drain the `[uvug10]` counters watch, clamped to the panel)
and visibility (`visible()`: some report has ever arrived, and the last one is younger than the ~1.5 s
auto-hide). `video::cursor` is only a painter, and reads both. `click1_dispatch`'s hit test keeps reading
`pal::cursor::pos`, and the sprite's box ORIGIN is that same position — the arrow's tip is the box's top-left
pixel — so the click lands where the tip is drawn. That equality is why the sprite is **clipped** at the panel
edge rather than shifted inward: shifting the box to keep it whole would move the drawn tip up to `side - 1`
px away from the hot spot the click uses, and near the right or bottom edge the operator would be aiming with
an arrow that is not where the click goes. Clipping draws less of the tail and keeps the tip exact.

The arrow SHAPE is shared with `pal::cursor`; the SIZE is not. `pal::cursor` magnifies by `scale + 1` (a
deliberate step above the text scale for a full-screen demo), `video::cursor` by `scale`, which makes the
desktop sprite exactly one glyph cell. The two never coexist — a demo owns the screen while it runs — so this
is a size change across modes, not two cursors at once.

**Three calls, and everyone who paints uses them.** `undraw()` restores the pixels under the sprite and
forgets them; `repaint()` is `undraw()` then save-under-and-draw; `armed()` is the witness latch. The
callers are exactly the front-framebuffer painters:

| painter | bracket |
|---|---|
| `wm::composite` | `undraw()` before the table lock, `repaint()` after the last `draw_window`/`verify_window` — the pass body is a private `composite_inner`, so every early return keeps the ordering |
| `wm::erase` (desktop repaint on close) | `undraw()` first; the `composite()` each caller runs next redraws |
| `render_service` (Pi) | `undraw()` / `repaint()` around `pal.render()` (`Screen::flush` overwrites the front buffer's damaged rects); `repaint()` on each real pointer report; `undraw()` on the auto-hide edge |
| `fbcon` (boot console mirror) | **no bracket, by exclusion** — `fbcon::detach()` at GUI takeover stops it painting the framebuffer, and it is out of the GUI's life cycle thereafter. A panic re-attaches it, which is a state where a stale cursor patch is not the problem to solve. This is the one front-buffer painter that is ordered by *not running* rather than by a bracket, and the reason both of this module's `serial_println!`s are emitted with the sprite lock released |

Composed with **WC-E** the bracket count goes up sharply: `Screen::flush` now calls `wm::repaint` → `composite`
on every desktop present, so the cursor is undrawn and redrawn ~20×/s on the bench even with the pointer at
rest. That is what forced the save-under to read back only the pixels the arrow PAINTS (~50 at scale 1) rather
than the whole 36×36 box (1296 `read_pixel` calls/frame). The composed per-frame cost of the save-under is a
**bench-read item** — it has never been measured on hardware.

**Damage: save-under, sized by the metrics.** The sprite is 8×8 blocks at `Metrics::scale`, i.e. exactly
one glyph cell (`cell_w`×`cell_h` — the derivation `ui.rs` already states for the text cursor), plus one
block of drop shadow: 36 px square at the 4× cap and nothing named in pixels anywhere (THE METRICS RULE).
Only the pixels the arrow PAINTS are saved with `FrameBuffer::read_pixel` before drawing and written back on
`undraw` — the rest of the box was never modified, so restoring it would be a write, and a race, with nothing
to fix — and only its scanlines are cleaned for the non-coherent HVS. The alternative — marking the box damaged and driving a
desktop+window recomposite — would run a composite pass at HID report rate (~125 Hz) to move an arrow three
pixels. Pointer motion therefore does **not** set the render loop's `dirty` flag at all: the sprite bypasses
the `Screen` and its flush entirely, which also removes the pre-CURSOR-1 erase-to-`0x1E1E1E` (neither the
desktop colour nor on top of a window).

A panel whose format has no colour inverse (`read_pixel` returns `None` — the lossy `U8` layout) **disables
the cursor for the boot** with a one-line reason, rather than restoring pixels it could not read. Fail-closed:
no cursor beats a trail of wrong pixels across the desktop.

**The race is the RESTORE, not just the sprite — say it plainly.** The front framebuffer has no single owner,
and WC-E makes the collision routine rather than exceptional: the compositor repaints the window layer on
every desktop flush, from another core's syscall context. The dangerous ordering is not "a window covers the
arrow" but its inverse — the render task draws the sprite at P and saves PRE-window pixels; the compositor
then draws a window over P and verifies it; the compositor's own bracket then undraws, and a naive restore
**stamps those pre-window pixels back INTO the verified window rect**. Nothing would repair it (a composite
repaints damaged windows, not arbitrary rows) and a later `[wc-d] verify` could read it as `bad_cache > 0` —
a line the Pi spec FORBIDs. Three mechanisms, together, close it:

1. **Atomicity.** Every entry point holds the sprite lock across the whole restore → save → draw sequence. The
   earlier split (`repaint` calling a self-locking `undraw`, then re-locking to save) left a window in which
   another core's draw could be captured AS the save-under — which would stamp a permanent white arrow into
   whatever was beneath it.
2. **A colour-guarded restore.** A saved pixel is written back only if the framebuffer still holds the exact
   colour the sprite painted there. A window drawn over the sprite fails that test, so its pixels are left
   alone: the restore cannot touch a rect another painter has taken.
3. **Damage repair.** Whatever rect a restore touched is handed to `wm::damage_intersecting`, which marks every
   overlapping window damaged so the next composite redraws it from its source surface. This closes the colour
   guard's residual hole — a painter whose new content happens to be exactly the sprite's own `FILL` or
   `SHADOW` is indistinguishable from the sprite. It marks only, never composites (composite brackets this
   module; compositing from here would recurse), so the repair lands within one WC-E frame.

Lock order is stated once and must hold: **`SPRITE` → `TABLE`, never the reverse** — nothing in `wm` calls
into the cursor while holding the window table, and the repair runs with the sprite lock released.

**Why no checksum or witness moves — two independent reasons.** *Ordering:* no verified pixel is ever read
with the sprite on the panel (`composite` puts it back only after the last `verify_window` returns), and
`[wc-c]`'s checksum hashes the SOURCE surface, which this module never touches. *Arming:* the sprite is drawn
only while `pal::cursor::visible()`, which requires a real pointer report — QEMU raspi4b delivers no HID
pointer input, so on the gate this code writes zero pixels and prints nothing for the whole boot. The
`kernel8-test` capture and every `[wc-c]` / `[wc-d]` / UVUG checksum are therefore unchanged by construction,
which is what the gate below confirms.

Witness, printed once, at the first draw of the boot (input-driven, so quiet boot is preserved):

```
[cursor] armed x=320 y=240
```

#### CURSOR-1 gate results (2026-07-25, QEMU raspi4b)

`./arroyo check` green both arches · `kernel8` builds · `kernel8-test 120` MBENCH **50/50 required,
0 forbidden**, with every pre-existing checksum and witness byte-identical (the arc adds no spec directive:
`[cursor] armed` cannot fire without a pointer device, and a REQUIRE for it would be a line the gate can
never satisfy) · `test-arm` green.

The **evidence that the cursor is inert on the gate** is the strongest form available off-metal: the
`target/pi-screen.png` capture is sha256
`2686a884320dbc389d6c33b1f37b097fa15eba769b51a751449e2c91a986bc19` — bit-identical to the WC-C baseline —
and `[wc-c] win=… cksum=`, `[wc-d] verify … -> PASS`, `:: UVUG: frames=300 … checksum=0xe68285b85121ac7c ::`
and the compat `mapped=32x32 … checksum=0x8d99530ca96d4b25` are unchanged. Not one `[cursor]` line appears.
What QEMU therefore cannot exercise is the sprite itself: the first draw, the save-under restore and the
`[cursor] armed` line are **unverified until the next bench boot**, exactly as WC-D's `bad_ram` column is.

### FOCUS-VIS — focus you can SEE, and a shell you can read

P59 (bench, 2026-07-25) put two backgrounded windows on the panel — the UVUG crystal and `STAT.ELF` —
and produced two observations that turn out to be one defect:

1. **TAB provably cycled focus and the panel never moved.** `[wc-c] focus tab-cycle 0->1->2->0 (ring of
   2 + shell)` fired on every press, and `STAT` stayed entirely covered by the UVUG window.
2. **The shell was unreadable.** With windows up, the console — prompt, command line, command output —
   was underneath them. The operator could type and could not read.

The common cause: **focus was a pure input-routing fact.** `el0_input_set_active` moved where keystrokes
went and nothing else, and the shell had no position in the z-order at all — it was the surface the
window layer got painted *onto*. Nothing raised a focused window, and nothing could put the console in
front of anything, because "in front of the console" was the only thing a window could be.

#### The shell is a member of the z-order

`wm::SHELL_Z` is the shell's z, allocated out of **the same monotonic `next_z` counter** every window
raise uses. That single fact makes both halves the ordinary comparison:

* a window with `z > SHELL_Z` is above the shell and composites normally;
* a window with `z < SHELL_Z` is below it and is **not drawn** — the console owns those pixels.

`SHELL_Z` starts at `0` ("the shell is at the very bottom"), which is exactly the pre-FOCUS-VIS
behaviour, so a boot that never TABs composites byte-identically to before. That is every QEMU gate run:
raspi4b has no HID.

#### One seam: `wm::focus_changed(asid)`

Called by `wc_focus_key` immediately after `el0_input_set_active`, with `asid == 0` meaning the ring's
SHELL slot.

| `asid` | what happens |
|---|---|
| a window owner | **every** live non-compat window of that ASID takes a fresh z (above all windows *and* above the shell), is marked damaged, and the pass composites. All of the owner's windows, because the ring is keyed by ASID — raising a subset would leave an app half in front. |
| `0` (the shell) | `SHELL_Z` takes the fresh z. Every window is now below it: their outer boxes are erased to `DESKTOP_BG` **immediately** (instant response), and `screen::request_full_present()` asks the desktop layer to repaint the whole panel so the console's *text* comes back over that erase. |

`request_full_present` is a flag rather than a call because the `Screen` is owned by the render task —
there is no global handle, and inventing one would hand a second core a `&mut Screen`. The compositor
raises it from syscall context; `Screen::present_background` consumes it, on its own thread, before it
reads its damage set. Latency floor is the 1 Hz status-strip tick; the erase is what makes the response
look instant regardless.

Compat rows are exempt from the shell test (`above_shell`): a compat window IS the full-screen present
path, it carries owner ASID `0` and is not addressable as a focus target, so it could never be raised
back above a shell that overtook it — hiding it would strand a full-screen app's output permanently.

> **Residual, stated as a limitation.** The exemption is sound in its own terms and it is not free: a
> **backgrounded** compat (full-screen) app still composites over the console after a TAB to the shell,
> so the "read your command's output" case is only fully delivered for *windowed* apps. The foreground
> case is unaffected — a full-screen app run in the foreground parks the shell, so there is no prompt to
> read behind it — and this is the same shape as the residual BGRUN-1 already recorded on `wm::repaint`
> (a bg compat app shimmers while the operator is TABbed into some other app). The cause is shared and
> so is the fix: a compat row has **no owner ASID** to key on (the `SYS_FB_PRESENT` hook signature
> carries none), so it cannot be made a focus target, cannot be raised, and therefore cannot honestly be
> lowered either. Giving compat rows a real owner is a change to the `SYS_FB_PRESENT` seam, not to the
> z-order — flagged here rather than worked around, because every available workaround keys on something
> coarser than ownership and would strand a full-screen app's output in some other state instead.

#### Three smaller defects the same arc closes

* **`create` now composites.** A window's kernel chrome reaches the panel when the row exists, not at the
  owner's first present (which, for an app that maps a surface and then blocks, may be never). It also
  puts window creation *inside the cursor bracket*: before this, the first thing to touch the panel after
  a create was the owner's `draw_window` on another core, with the sprite down and its save-under holding
  pre-window pixels. Not on the compat path — `create_inner` is only the first half of `compat_present`,
  and a composite there would flash the surface once at the row's defaults (1× at the origin).
* **`move_to` now erases the box it vacates.** The compositor draws windows and never the desktop, so a
  moved window used to leave a full copy of itself — content, border and title strip — at its old
  position forever. Same treatment `close` gives a vacated box, because it is the same event.
* **The system cursor survives an app holding focus.** `route_input_to_active_el0` returns after routing,
  so the shell loop's `Mouse`/`MouseAbsolute` arms — the only code that moved `pal::cursor` and repainted
  `video::cursor` — were unreachable while any app had focus. The sprite froze and auto-hid 1.5 s later,
  and no amount of mouse movement brought it back. The router now updates the shared pointer state and
  repaints the sprite alongside delivery (delivery itself is unchanged), gated on
  `pal::cursor::has_reported()` so the boot-time `input_router_selftest`'s **synthetic** `Event::Mouse`
  cannot arm a cursor on a gate that has no pointer.

#### WC-D's one-shot needed a gate

`create` compositing means WC-D's per-window read-back latch would otherwise be claimed by the
create-time pass and verify a **blank** surface — a vacuous `-> PASS` that satisfies the spec's REQUIRE
while the app's real content is never checked. `Window::presented` (set by `present` / `compat_present`)
makes the verdict wait for content the owner actually put there.

#### The witness: `[wc-fv] focus-vis` — a READ-BACK, not a state dump

> **Tag:** `[wc-fv]`, not `[wc-g]`. `WC-G` belongs to the concurrent garble arc (`video/wcg.rs`); the
> regexes would not cross-match, but two subsystems answering to one letter in the ledger is a cost paid
> at every future integration rather than once here.

Every pre-existing focus witness is a statement about kernel state, and `[wc-c] focus tab-cycle` printed
correctly on the bench for a panel that never changed. `wm::focusvis_selftest` never asks the table who
is in front; it asks the **framebuffer what colour is actually there**. Two solid-colour 8×8 windows are
placed at the SAME origin, so exactly one of them can own the probe pixel:

| leg | action | expected pixel |
|---|---|---|
| `stack` | B created after A | B's colour (baseline) |
| `raise` | `focus_changed(A)` | A's colour — **defect 1** |
| `shell` | `focus_changed(0)` | neither colour — the window layer stopped owning those pixels |
| `reraise` | `focus_changed(B)` | B's colour — the shell is a POSITION in the rotation, not a terminus |

Self-cleaning (both windows closed, boxes erased, `SHELL_Z` returned to 0), placed upper-middle so it
cannot collide with WC-F's reserved probe boxes at the bottom edge, and run from the tail of
`wcb_launcher` so it cannot burn the one-shot `[wc-c] side-by-side` / `[wc-d] verify` latches.

```
[wc-fv] focus raise asid=0xf0a windows=1 top_win=1 z=7 shell_z=0
[wc-fv] focus shell z=8 hidden=2
[wc-fv] focus raise asid=0xf0b windows=1 top_win=2 z=9 shell_z=8
[wc-fv] focus-vis at (641,314) a=0xff2020 b=0x20ff20 stack=0x20ff20/true raise=0xff2020/true shell=0x2d2b55/true reraise=0x20ff20/true -> PASS
```

The `shell` leg reads `0x2d2b55` — `DESKTOP_BG` exactly — which is the erase landing, i.e. the window
layer genuinely stopped owning those pixels rather than merely being reordered among itself.

#### FOCUS-VIS gate results (2026-07-25, QEMU raspi4b @ `UNAOS_FBW=1920 UNAOS_FBH=1200`)

`./arroyo check` green both arches · `kernel8` builds clean · `kernel8-test 60`
**57/57 required, 0 forbidden** (56 → 57: the one new `[wc-fv] focus-vis` REQUIRE, with a matching
FORBID).

> `./arroyo test-arm` does **not** build at this tip, and not because of this arc: the aarch64-virt
> profile is `witness` **without** `baremetal`, and `video/wcf.rs` is gated on
> `all(target_arch = "aarch64", feature = "witness")` while importing `arch::aarch64::mailbox`, which is
> gated on `baremetal`. Both files are untouched by FOCUS-VIS (last changed by WC-F, `49e7d5f5`);
> recorded here rather than fixed, since the fix is a WC-F cfg correction outside this arc's lane.

Every pre-existing witness is byte-identical: `:: UVUG: frames=300 threads=2
checksum=0xe68285b85121ac7c ::`, `:: EL0: window verbs — … witness=0x1fff …
checksum=0xfabe809492cf2325 :: PASS ::`, the compat `mapped=32x32 … checksum=0x8d99530ca96d4b25`,
`[wc-c] side-by-side windows=2 drawn=2`, and every `[wc-d] verify … bad_cache=0 bad_ram=0 -> PASS`
including the two the selftest's own 8×8 rows now add. No `[cursor]` line appears — the router
keep-alive is `has_reported()`-gated, so the synthetic router-selftest `Event::Mouse` still arms
nothing on a gate with no pointer, exactly as CURSOR-1 promised.

#### The bench operator test (what must pass on the next boot)

`bg` two apps → TAB to the shell, **type a command and read its output** → TAB to a window, the window is
visibly front → the cursor is visible throughout. The raise, the shell raise and the cursor's survival of
a window create are all gate-checkable; the *legibility* of the console under the full present, and the
sprite itself (QEMU has no pointer), remain bench-only.

### FOCUS-HL — the focused window's chrome says so

FOCUS-VIS made focus *positional*: the focused window raises, the shell raises above everything. That is
the right primitive and it is not, on its own, an answer at the panel. With two windows that do not
overlap — which is the normal `bg` × 2 layout, side by side — raising the focused one moves nothing
visible, so the operator still cannot tell which window has the keyboard without typing into it.

Chrome is the natural carrier, because it is already kernel-drawn and already repainted on every present
(see WC-A: an app draws only inside its surface, the border and title strip are the compositor's). So the
indicator costs no new pixels and no new pass.

* **State.** `wm::FOCUS_ASID` — the ASID that holds focus, `0` for the shell. Written only by
  `focus_changed`, which is already the single seam every focus move passes through, so there is no
  second place for the compositor's idea of focus to diverge from the router's.
* **Read once per pass.** `composite` snapshots `FOCUS_ASID` alongside `SHELL_Z`, for the same reason:
  one pass judges every window against one focus owner, so a pass structurally cannot paint two
  highlights.
* **Two colours, no geometry.** `draw_window(fb, r, focused)` swaps `CHROME_BORDER` →
  `CHROME_BORDER_FOCUS` (`0x8C8CB4`) and `CHROME_TITLE_BG` → `CHROME_TITLE_BG_FOCUS` (`0x3A3A5A`). The
  `outer_box` is unchanged, so focus never moves a pixel — no reflow, no re-damage of neighbours beyond
  what focus already causes, and the highlight is free relative to a present that was going to fill those
  rects anyway. The colours stay in the flat, deliberately un-host-like family WC-A chose: this marks
  focus, it does not imitate a title bar.
* **Shell focus highlights nothing.** `focus == 0` matches no window, which is the honest reading — no
  app has the keyboard. The explicit `!= 0` test also stops a compat row (owner ASID `0`) matching by
  accident, though a compat window draws no chrome to highlight in the first place.
* **The losing window repaints too.** `focus_changed` already damages the windows it *raises*; it now
  also damages the windows of the ASID losing focus. That set is disjoint from the raise (and empty on
  the shell branch, which raises nothing), so without it the previous holder would keep the bright chrome
  until something unrelated damaged it — the same "both ends repaint" property FOCUS-VIS established for
  position, now extended to colour.

Damage is still focus-change-scoped: no per-frame cost, and a boot that never TABs — every QEMU gate run,
since raspi4b has no HID — composites byte-identically to before, which is why the existing surface
checksums are untouched.

**And that is also the limit of what the gate can say about this fold.** raspi4b has no HID, so no QEMU
run ever changes focus, so no QEMU run ever draws the highlight. The gate proves the change is *inert*
where it should be inert (59/59, every checksum unchanged); the highlight itself is **bench-only** and
unverified. The bench test is the FOCUS-VIS recipe with one addition: `bg /fat/VUG.ELF` → `bg
/fat/STAT.ELF` → TAB, and at each stop the front window's border and title strip must be visibly
brighter than the other's, with **neither** highlighted at the shell stop.

#### VUG/STAT gate results (2026-07-25, QEMU raspi4b @ `UNAOS_FBW=1920 UNAOS_FBH=1200`)

`./arroyo check` green both arches · `kernel8` builds clean, staging `VUG.ELF` (12568 B) and `STAT.ELF`
(8472 B) under their new names (the persistence app was `KVUG.ELF` at that arc; STAT-NAME later restored
`STAT.ELF`, byte-identical at 8472 B) · `kernel8-test 60` **59/59 required, 0 forbidden** (unchanged — this arc
adds no spec pattern and renames none) · `./arroyo test-arm` reaches
`>>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`.

> Correction to the FOCUS-VIS note above: **`./arroyo test-arm` does build and pass at this tip.** The
> `wcf.rs` / `mailbox` cfg mismatch recorded there was fixed by a later arc; the note is kept for the
> history but should not be read as a live defect.

The renamed load paths are witnessed in the log rather than inferred:
`:: EXEC-UVUG: run /fat/VUG.ELF — loaded 12568 bytes, entry 0x300000, exit=0 -> PASS ::`, all three
`BGRUN-ST` legs PASS, and `:: UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c ::` is byte-identical
to before the arc.

**VUG-BG was verified by measurement, not by reading the diff.** A temporary probe in `BGRUN-ST`'s
kill leg dwelled 4 s before killing the backgrounded `VUG.ELF` and printed
`:: TEMP-VUGBG: after 4s bg VUG alive=true ::`. Four seconds is several times the ~1 s that same boot
takes to run 300 frames in the foreground, so "still running" cannot be "has not finished the auto path
yet". The probe was reverted and the gate re-run clean on the reverted tree.

### WC-G — the window-path garble, localized

WC-D, WC-E and WC-F between them exonerated everything global. On silicon the scan-out identities are all
true, both twin blocks are clean at 8192/8192, and the ramp photographs straight. And still a live 128×128
window garbles, in horizontal bands, less badly when the app cycles faster.

Every instrument in that chain shares one property: it measures **converged** content. A one-shot
read-back, a static twin, a photographed ramp — each samples after the writing stopped. A window that
repaints forever never converges, and that is exactly the population that garbles. WC-G instruments the
non-converged case: the present path *while it is running*.

#### The instrument

Four checksums of the same surface taken at four moments around one blit, plus a read-back of what that
blit landed, plus the blit's duration:

| leg | when | a divergence means |
|-----|------|--------------------|
| `app` | at `SYS_WIN_PRESENT` entry, owner parked in the syscall | the frame the owner declared finished |
| `blit` | immediately before `draw_window` reads it | `app != blit` — the surface moved between present and copy |
| `civac` | after `DC CIVAC` over the surface | `blit != civac` — **coherency**: the kernel's lines did not match the coherent view |
| `after` | immediately after `draw_window` returns | `blit != after` — **race**: the owner wrote it *mid-copy* |
| `fbbad` | source re-derived and compared against `read_pixel` over the content rect | non-zero — the **blit/upscale** path landed a wrong pixel |
| `us` vs `rectscan_us` | wall clock of the blit vs the beam's time on the window's own rows | `slow=yes` — the scan-out is **guaranteed** to overtake the copy inside the rect |

`blit != civac` and `blit != after` cannot both be explained by one mechanism, which is what makes the
coherency and race suspects separable rather than a shrug.

**Why `CIVAC` here, when WC-D was required to use a bare `IVAC`.** WC-D's rule was about the framebuffer,
which the kernel itself writes: cleaning before reading would have written the blit's own dirty lines out
and healed the short-flush defect the witness existed to catch. The surface is the mirror image — the
kernel only ever *reads* it, so there are no kernel-dirty lines to write back and `CIVAC` cannot repair a
compositor defect; what it can do is force the read from the coherent view, which is the question. A bare
`IVAC` would additionally risk discarding the owner's un-cleaned lines, destroying app data to answer a
question `CIVAC` answers safely. The rulings differ because the buffers differ.

**`own=`** records *why* the window was blitted. `own=yes`: the blit follows this window's own present, so
its owner is inside the syscall and cannot be writing. `own=no`: collateral repaint — the damage set is
closed upward over occlusion, so presenting window A repaints every higher-z window overlapping it, and
B's owner is running free at EL0 with nothing serialising it against the copy of its surface. `own=no`
with `blit != after` is source tearing caught in the act. Both cases occur in the gate.

**`rectscan_us`** is the threshold that matters, and it is derived, not chosen: the beam only has to cross
*this rect* to latch it part-old and part-new, so the criterion is `frame_us × rows_on_panel ÷
panel_height`, not a whole frame period.

Budgeted at four samples per window id (the checksums are 64 KiB reads and the read-back is one probe per
source pixel, from present context at EL0 frame rates — an unbudgeted version would perturb the timing it
reports). Per-window, not per-window-0, so the shared-path claim is provable on the wire. `witness`-gated
and aarch64-only like `wcf`: knob-off, `video/wcg.rs` does not compile and the flashable media are
byte-identical.

#### What the gate found

`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60`:

```
[wc-g] win=1 seq=1 own=yes scale=4x app=0xa1cf…8449 blit=0xa1cf…8449 civac=0xa1cf…8449 after=0xa1cf…8449 fbbad=0/16384 us=12808 rectscan_us=7111 slow=yes -> CLEAN
[wc-g] rollup win=1 scope=window samples=4 coher=0 race=0 blit=0 clean=4 slow=4 maxus=13052 frame_us=16667 -> CLEAN+SLOW
[wc-g] win=2 seq=1 own=no  scale=4x … fbbad=0/4096 us=3060 rectscan_us=3555 slow=no -> CLEAN
```

All three named suspects are **negative**: no `civac` divergence, no source race (in either the `own=yes`
or the `own=no` case), no blit-path or upscale error — `fbbad=0` over every probe. Every byte was correct
at every moment.

`coher=0` should be read narrowly. The compositor's read is a normal cacheable read of Normal
Inner-Shareable memory and the caches are PIPT, so against another core's cacheable writes to the same PA
it is coherent *by construction* — that was never in doubt and needs no witness. What the `civac` leg
tests is the part that is not guaranteed: an **alias-attribute mismatch**, the surface reached through two
mappings whose attributes or shareability disagree (the EL0 `user_data_page` leaf vs the kernel identity
leaf), or a non-cacheable alias in the chain. So `coher=0` means "no alias-attribute mismatch on this
surface", not "coherency is fine in general".

The residual is timing, and the code says why. `draw_window` writes **per-pixel, with `put_pixel`, directly
into the front framebuffer** — the live scan-out — with no vblank synchronisation anywhere in the path.
The desktop does not: it reaches the panel through `Screen`'s back buffer and a contiguous per-row
damage-rect flush, which is why direct desktop writes are clean and only windows garble. That asymmetry is
structural and is the finding; it does not depend on any measured number.

The number puts a size on it, and needs its provenance stated. **The machine was QEMU raspi4b at forced
bench geometry (`UNAOS_FBW=1920 UNAOS_FBH=1200`) — not silicon.** There, a 128×128 window at 4× copies for
13.3 ms against 7.1 ms of beam time on its own rows: a **1.9× overtake**. That figure is QEMU wall clock
and it is not stable across runs — a lens re-run of the same build measured **1.71×**. What both runs
agree on, and all the arc claims, is that **the ratio exceeds 1**, which is the condition for the scan-out
to overtake the copy inside the rect and latch it part-old/part-new at whatever scanline the beam held —
a horizontal band boundary, the shape in the photograph. It also explains "cycling faster looks a little
better": a faster cycle does not remove the tear, it shortens the interval any one torn frame stays on the
panel. The 64×64 window sits right at its own threshold (3.2 ms vs 3.6 ms), consistent with smaller
windows looking better.

**On an A72 the ratio may fall below 1**, and if it does, that falsifies *the number*, not the mechanism:
an unsynchronised per-pixel copy into a live scan-out tears whenever it loses the race, and a ratio under
1 on metal would mean only that it loses less often than QEMU suggested — the bench photograph already
establishes that it loses. `rectscan_us` is itself conservative in the same direction: `frame_us` includes
blanking while `ph` counts only visible lines, so the rect is credited with beam time the beam does not
spend on it, every `slow=yes` understates the problem, and a `slow=no` near the threshold may still tear.

WC-G fixes nothing — it is the localization. The fix is a design question for the next arc (double-buffer
the window layer, or bound the copy under the rect's scan time), not a knob.

#### WC-G gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

QEMU raspi4b, forced bench geometry — **no metal in this arc**.

`./arroyo check` green both arches · `./arroyo kernel8` clean, **zero `wc-g` strings** knob-off (armed
build: two) · `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60` → MBENCH **58/58 required,
0 forbidden**, on a spec grown by this arc's two REQUIREs and three FORBIDs. The REQUIREs assert that
the instrument *ran*, deliberately not what it found: the finding is the arc's output.

**There is no global summary line, and that is the result of three failed attempts rather than an
omission.** (1) Fire when the first window spends its budget: printed the summary *before* window 2 was
sampled at all, including before its `own=no` collateral-repaint sample — a metal race there would have
sat on the wire underneath a green summary contradicting it. (2) Fire when every *sampled* window has
spent its budget: the same bug in new clothes, since the sampled set only holds windows seen so far, so it
is trivially true the instant the first window finishes; it reproduced exactly, printing
`scope=exhausted samples=4 windows=1` before window 2 existed. (3) Fire on quiescence: the gate's two apps
start more than 3 s apart, so an idle gap is not evidence that sampling is over — it fired early too, at
`idle_us=3011902`.

The lesson is structural, not a tuning problem: **nothing observable inside a boot distinguishes "sampling
is finished" from "the next app has not launched yet."** Any global summary is therefore a completeness
claim the instrument cannot support, and one that overstates its scope is worse than none — it is the one
artifact that can make later contrary evidence look already accounted for.

So the rollup is scoped to **one window** and fires when that window spends its budget: deterministic, no
timer, and its scope is exactly what its `win=` says. The job the global line was reaching for — "no
suspect fired anywhere, ever" — moved to the spec as **FORBIDs on `-> COHER`, `-> RACE`, `-> BLIT`**. A
FORBID needs no completeness claim: it catches an anomaly in any window at any point in the boot, including
one appearing long after every rollup has printed. `CLEAN` and `CLEAN+SLOW` stay green, because the timing
finding is this arc's result and not a regression.

Unlike WC-F, the QEMU result here is **not** a formality. The three checksum legs are cache- and
race-sensitive and QEMU models neither caches nor the HVS, so their `CLEAN` verdict on the gate is weak
evidence and must be re-read on metal — a coherency or race defect can only appear there. The timing leg is
the opposite: `us=` is measured against `CNTVCT` on the same per-pixel code path the bench runs, and metal
is expected to be **slower**, not faster (real non-coherent framebuffer stores plus a real `DC CVAC`
sweep). The overtake the gate measures is therefore a lower bound on the bench's.

### WC-H — the window layer gets a back buffer

WC-G ended with a verdict and no fix: `coher=0 race=0 blit=0 clean=4 slow=4 -> CLEAN+SLOW`. Every byte
of the surface was correct at every moment around the copy, the scan-out read-back matched the source
everywhere, and the panel still garbled — because the copy took `us=15524` while the beam crosses the
window's own destination rows in `rectscan_us=7111`. Being overtaken was not a risk, it was arithmetic.

The mechanism is not re-litigated here (see §WC-G). What matters for the fix is *where* the copy landed:
`draw_window` wrote the **live front framebuffer**, one `put_pixel` at a time, upscaled — at 4x, sixteen
separate bounds-checked stores per source pixel — into memory the HVS was scanning at that moment, with
no vblank synchronisation anywhere in the path. The desktop has never had this problem, and not by luck:
it draws into `Screen`'s cached back buffer and reaches the panel through a contiguous per-row damage-rect
flush. Two layers, one scan-out, and only one of them was buffered.

#### The design trade

Three routes were on the table.

**(a) Route window blits through `Screen`.** The "do it right" answer on its face, and structurally
blocked — not merely inconvenient. There is no global `Screen`: each render task constructs its own
(`main.rs` builds four, `video::witness` a fifth) over the shared `WRITER` framebuffer, and none is
reachable from the compositor. The compositor runs from the presenting task's syscall context on an
arbitrary core, while the render task that owns a `Screen` may be parked inside `dispatch_command` and
flushing nothing at all — which is exactly why `present_surface` never routed through it either. Taking
this route means promoting one `Screen` to a global, giving it a lock acquired from syscall context, and
making every window present wait on the desktop's frame cadence. Three structural changes, one of them a
new cross-subsystem lock on a hot path, to buy a property route (b) buys outright.

**(c) Beam-race avoidance.** Bound or schedule the copy against the scan-out position. Rejected as the
brief anticipated: it needs a scan-position source the Pi 4 path does not expose, it degrades rather than
eliminates (a copy that must finish inside a bounded window either fits or tears), and it would make
correctness a function of window size and CPU load.

**(b) A dedicated window back-layer — chosen.** Compose the whole window (chrome, title strip, upscaled
content) into a cached-RAM layer sized to its clipped outer box, then present that box as contiguous
full-width row copies followed by the same single `flush_range`. This is the *same discipline* as the
desktop path — bulk sequential `blit` per row onto the framebuffer, one cache clean over the span — with
no ownership change and no new lock. The layer is one reused buffer (`wm::STAGE`), grown on demand with
`try_reserve`, capped at 4 MiB.

Every decline falls back to the pre-WC-H direct path rather than failing: a compat row, a box over the
cap, another core holding the buffer (`try_lock`, never `lock` — a present must not queue behind another
core's copy), or an allocator that cannot grow it. A window can never be lost to the back-layer.

**Compat rows keep the direct path deliberately.** A compat window is the full-screen `present_surface`
shim; its box is the whole panel, so staging it costs a panel-sized allocation and a full extra panel copy
per present — on the evidence of the `repaint` compat exclusion that preceded it, enough to blow the
`EXEC-UVUG` frame deadline. It also does not need it: while a foreground full-screen EL0 program owns the
panel the render task is parked, so the two-writer contention this arc removes is not present. WC-F's
direct-path probe likewise still bypasses the compositor, unchanged and intentionally.

#### The upscale's vertical runs, written once

A nearest-neighbour upscale emits `scale` **identical** destination lines per source row. On the front
buffer there was no cheap way to exploit that; on the back layer the lines are contiguous byte runs at a
known stride, so the first is composed per-pixel and the rest are produced with one bulk copy each. The
compose phase roughly halved (win 1: ~21000 µs → ~6300 µs). The replication is applied **only** on the
staged path: on the front, copying a composed line would carry that line's existing pad bytes onto the
others (`put_pixel` writes 3 bytes of 4), so the fallback stays a pure per-pixel write and remains
byte-for-byte the pre-WC-H path. On the back layer every pad byte is 0 by construction — the same zero pad
`Screen`'s back buffer has always presented — so the copy reproduces exactly what the per-pixel loop
would have left.

The back layer carries no residue between windows because the chrome fill is dense over the whole clipped
box: every pixel the present copies was written by the pass that composed it.

One caveat on the byte-for-byte claim above, which is about the *fallback* path and not the staged one:
the staged present writes **4 bytes per pixel** (the composed colour plus a zeroed pad byte) where the
direct path wrote 3 and left the pad as it found it. That is not pixel-visible — the pad byte is ignored
by the HVS on an XRGB scan-out, and it is the same zero pad `Screen`'s back buffer has always flushed to
the front — and WC-D is unaffected, since `read_pixel` decodes the three colour bytes. But the staged
output is byte-identical to the direct output only in the colour bytes, not in every byte of the
framebuffer.

#### Declines are samples, not non-events

`stage_window` has four fall-back exits — box over the cap, `try_lock` lost, allocator refusal,
degenerate geometry — and each runs the direct, pre-WC-H path: the tearing regime. The first cut of
`[wc-h]` fired only on staged success, which made its verdict an overclaim. The failing scenario is
concrete, not hypothetical: if 96 of 100 composites lost the lock to a concurrent desktop flush (a ~6 ms
hold window, which is exactly the contention the `try_lock` exists to sidestep), the window would have
torn continuously and the four staged samples would still have printed `TEAR-FREE`, with the FORBID
never firing. The same blind spot would have hidden a window whose box exceeds the 4 MiB cap falling back
*permanently*.

So a decline spends sample budget, prints its own line with its reason, and forces the rollup verdict to
`UNSTAGED` — which the spec FORBIDs alongside `AT-RISK`. `UNSTAGED` is not a softer finding than
`AT-RISK`: it says composites reached the panel unbuffered, so the tear-free claim the staged samples
support does not describe what the window actually did. The cap fallback becomes loud for free.

```
[wc-h] win=1 staged=no reason=fixture -> DIRECT
[wc-h] rollup win=1 scope=window samples=4 torn=0 declines=0 fixture=1 maxpresent_us=1444 ... -> TEAR-FREE
```

#### The fallback fixture — coverage that was nearly traded away

Before WC-H, every `[wc-d] verify` read a directly-drawn window. Afterwards every one of them read a
staged present, and for non-compat windows the fallback path stopped being verified against the scan-out
at all (WC-D skips compat rows, which are the only rows still on the direct path in normal operation).
That is a real regression in coverage, and it would have been silent.

A witness-only global one-shot latch forces the *first* composite WC-D is about to verify onto the direct
path, armed on the same predicate `composite_inner` tests afterwards so the fixture and the verification
cannot land in different passes. The gate runs two windows, so exactly one is verified on each path — the
log above shows `win=1` verified after its `reason=fixture` decline and `win=2` verified on a staged
present. `fixture` is counted apart from `declines` (the kernel asked for it) and printed separately so
the exclusion is visible rather than assumed, and a REQUIRE asserts the fallback was actually exercised.

#### `[wc-h]` — the witness, and why WC-G's `slow=` was not re-scoped

`[wc-h]` splits the operation into the two halves that now mean different things:

```
[wc-h] win=1 box=514x526 bytes=1081456 compose_us=6075 present_us=1084 rectscan_us=7305 torn=no -> BUFFERED
[wc-h] rollup win=1 scope=window samples=4 torn=0 declines=0 fixture=1 maxpresent_us=1444 frame_us=16667 -> TEAR-FREE
```

`compose_us` happens off-screen where no scan-out can observe a partial result. `present_us` is the row
copies — the only phase that can still tear — and `torn=` compares *that* against the beam's time on the
box, computed exactly as WC-G computes `rectscan_us` and with the same deliberate bias toward **not**
reporting a tear (`frame_us` includes blanking the beam does not spend on visible rows, so the box is
credited with more beam time than it gets; a `torn=no` near the threshold is not a proof of safety).

WC-G's `us=`/`slow=` leg and all three of its FORBIDs are **unchanged**. Its bracket still contains the
copy and nothing else — `blit` is still the surface as the copy found it, `after` as the copy left it —
but the copy is now two phases, so `slow=yes` says "the whole operation outran the beam", most of which
the beam cannot see. It no longer implies a torn panel. Deleting or re-scoping the leg would have damaged
the only instrument separating a source race from a coherency fault to fix a sentence of interpretation;
the tear question moved to `[wc-h] torn=` instead, which is narrower and true.

**The witness had to be moved out of its own measurement.** The first cut printed the `[wc-h]` line from
inside `stage_window` — inside `draw_window`, inside WC-G's clock — so every serial character of it was
charged to `[wc-g] us=`, which rose from a baseline max of 15524 to 23468 on the same work while the
compose and present figures the line reported summed to about half that. The sample is now *recorded*
where it is taken and *printed* from `wcg::stage_flush`, which the compositor calls after `wcg::end` has
stopped the clock. Consequence, stated: there is one pending slot per window id, so two cores compositing
the same window concurrently lose one printed line — the per-sample lines are a best-effort trace and can
be fewer than the rollup's `samples=`. The rollup's counters are updated at record time and miss nothing,
which is why the tear assertion is pinned on the rollup verdict (`FORBID -> AT-RISK`).

#### WC-H gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

QEMU raspi4b, forced bench geometry — **no metal in this arc**.

`./arroyo check` green both arches · `./arroyo kernel8` clean, **zero `wc-h` / `wc-g` / `BUFFERED`
strings** knob-off · `./arroyo test-arm` → xHCI MISSION SUCCESS · `UNAOS_FBW=1920 UNAOS_FBH=1200
./arroyo kernel8-test 60` → MBENCH **62/62 required, 0 forbidden** (59 before this arc → **62** with its
three REQUIREs; two new FORBIDs, `-> AT-RISK` and `-> UNSTAGED`).

Cost, on the wire, same geometry, before → after (`[wc-g] us=` is the whole copy; `[wc-h] present_us=`
is the panel-facing part of it):

| window | box | WC-G `maxus` before | WC-G `maxus` after | present max after | `rectscan_us` | verdict |
|---|---|---|---|---|---|---|
| 1 (crystal, 128x128@4x) | 514x526 | 15524 µs | 11294 µs | **1444 µs** | 7305 | TEAR-FREE |
| 2 (`stat.elf`, 64x64@4x) | 258x270 | 4088 µs | 3279 µs | **306 µs** | 3750 | TEAR-FREE |

The panel-facing exposure fell **~11x** (15524 → 1444 µs for the crystal) and now sits at **20%** of the
beam's time on the box, where before it was 218% of it. The total copy did not merely stay within a modest
budget — it got *faster*, because the row-run replication more than paid for the extra copy: window 1's
ceiling dropped from 15524 µs to 11294 µs, and window 2's `[wc-g]` rollup went from `slow=1` to `slow=0`
(`-> CLEAN`, no longer `CLEAN+SLOW`).

`[vugfps]`, honestly: **it does not appear on this gate and cannot**. It is emitted by the `vug` render
loop, which is reachable only through the interactive `vug` shell verb (`shell.rs`), so a headless
battery never runs it — the baseline capture has zero `[vugfps]` lines, before and after. The bandwidth
question it would answer is instead answered directly by `[wc-h] bytes=` (1081456 per staged composite for
window 1) plus the compose/present split above, which is a finer-grained account of the same cost on the
path this arc actually changed.

#### Metal watch-list

* **The crystal is SHARP.** The money reading, and the only one that settles the arc: a 128x128 window at
  4x, repainting continuously, with no horizontal band boundaries. WC-G's photograph is the before.
* `[wc-h] rollup ... -> TEAR-FREE` for every window. Metal is expected to be **slower** than QEMU on both
  phases (real non-coherent framebuffer stores, a real `DC CVAC` sweep), so `present_us` will rise; the
  margin to watch is 1444 µs against 7305 µs — a 5x headroom on the gate that metal must not consume.
  An `AT-RISK` rollup on the bench means the row copies alone are still losing the race and the next step
  is fewer bytes per present (damage sub-rects within the window box), not a bigger buffer.
* `[wc-h] rollup ... declines=0`. A non-zero `declines=` on the bench means composites are reaching
  the panel unbuffered — most likely `reason=lock`, i.e. real contention with the desktop flush that
  QEMU's timing does not reproduce. That is the one number most likely to differ on metal, and it now
  fails the gate rather than hiding behind the staged samples.
* `[wc-g]` verdicts stay `CLEAN` or `CLEAN+SLOW`. A `COHER` or `RACE` on metal would be a *new* finding
  about the surface, unrelated to this arc — the back layer changed the destination, not the source.
* The chrome: border and title strip must land in the same place they did, since they are now composed
  through a different origin. A one-pixel offset would show as a shifted frame.
* Cursor and FOCUS-VIS are unchanged above the buffering: the sprite is still taken off the panel for the
  whole pass, and `erase` still fills the desktop directly. `erase` is *not* staged — it is a one-shot
  solid fill on close/raise rather than a continuous repaint, so it can flash but cannot produce the
  standing tear this arc removes. If the bench sees a torn erase, that is the follow-on.

### WC-I — the periodic full-desktop repaint, and the cursor bracket that rode on it

P60 (Pi 4, 1920x1200, attended) confirmed WC-H: a single vug window is crystal sharp, `[wc-h] rollup
... torn=0 declines=0`, present ~1.2-1.6 ms. It also produced two symptoms WC-H does not cover, and
neither is reproducible in QEMU (one needs the bench's timing, the other needs HID):

1. with **several** vug windows, a fuzz blip in **every** vug window **simultaneously**, slightly faster
   than once a second, while the stat window, the desktop and the console stayed clean;
2. the mouse cursor visible but **spotty and flickering**.

#### The blip: one periodic painter, named

The synchronization across windows and the ~1 Hz period are the fingerprint of a single caller that
repaints the whole window layer, and there is exactly one:

* `main.rs::status_tick` posts an `Event::Timer` to `GUI_CHANNEL` once a second (metal only — QEMU
  raspi4b has no Group-1 IRQ, so the task is not spawned there at all, which is why the gate cannot see
  this);
* the render task's `Event::Timer` arm sets `strip_dirty`, `ui_status::draw` recomposes the PI-UI-2
  status strip, and `pal.render` → `Screen::flush` presents;
* `Screen::flush` then called `wm::repaint()`, which marks **every** live window damaged and composites
  the lot.

That is WC-E's own stated residual — "the window pixels are overwritten and repainted within the same
present rather than never being overwritten at all" — with a period and a trigger attached. Two
consequences, not one:

* every window is erased to desktop content and restored once per tick, so a scan-out landing between
  the two steps catches all of them at once;
* the repaint runs a composite from the **render** core while the vug windows present from **theirs**,
  so `wm`'s single `STAGE` back layer is contended. `try_lock` declines, and a declining window takes
  the pre-WC-H direct path — per-pixel writes into live scan-out, the tearing regime. One window rarely
  collides; N windows collide N times per tick, which is why the symptom needed several vugs.

The stat window reads clean for a mundane reason: it presents rarely, so it is almost never mid-present
when the repaint lands. The desktop and the console are single-writer surfaces and were never at risk.

**The fix removes the overwrite instead of sequencing it.** `wm::occluders` publishes the panel boxes
the window layer owns (live, non-compat, above the shell — the same population `stage_window` and
`wcg::begin` scope to), and `Screen::present_background` subtracts them from its own damage: each
damaged row is copied in the sub-spans no window covers (`next_visible_span`, a linear walk over at
most `MAX_WINDOWS` boxes, at most `2n+2` steps per row, no allocation). Desktop pixels are then never
written where a window is, for any interval however short.

With nothing to undo, the blanket re-blit goes too: `Screen::flush` now calls `wm::service_damage`,
which composites only rows something *else* marked — chiefly `cursor::repair`, whose "marks only, never
composites" contract needs a pass within a frame. `wm::repaint` survives for the one path that can
still intrude, and `present_background` reports which it took.

VUG-PAR's band-parallel flush is taken only when the window layer is **empty**. The band workers copy
whole clipped rects and know nothing about occluders; the case that needs the subtraction (several
windowed apps) is also the case where the desktop's own damage is a strip or a console line, which the
parallel path declines as too little work anyway. The full-screen VUG frame it was built for has no
windows and is byte-for-byte unchanged.

#### The cursor: an unconditional bracket at present rate

`wm::composite` was `cursor::undraw()` → pass → `cursor::repaint()`, unconditionally. `composite` runs
once per window present from the presenting task's own core, so with several high-rate windows the
sprite was in a restore→save→draw cycle on one core or another essentially continuously — and
`cursor::undraw_locked`'s colour guard *declines* to restore a pixel another painter has taken, which
under that contention is most of them. The panel shows a sprite with holes that move from frame to
frame. The cost was paid on every present regardless of where the pointer was.

WC-I makes the bracket **conditional**: `composite_inner` takes it only when `cursor::sprite_box()`
intersects a live window above the shell — plus WC-F's reserved probe boxes, which paint at the tail of
the pass and lie outside the window layer, so `repair` could never mend a sprite pixel they took. The
tail is `cursor::repaint()` when the pass disturbed the sprite and the new `cursor::ensure_drawn()`
when it did not (one lock and a boolean in the common case; the work it does do covers `erase`, which
takes the sprite down and leaves the following composite to put it back).

**Where the decision runs is a correctness constraint, not a preference.** It is taken at the very top
of `composite_inner`, *before* the snapshot that registers the pass as an in-flight blit. `undraw`
takes the SPRITE lock, and F4's drain barrier is a teardown spinning IRQ-masked and unpreemptible
until `BLIT_ACTIVE` reaches zero; acquiring SPRITE inside the `BlitGuard` window would add a second
lock to that wait set, so a core preempted while holding SPRITE would stall the draining core
indefinitely instead of for the length of a bounded blit. Deciding before the guard keeps the drain's
wait set exactly what its termination argument assumes.

The consequence is that the test is deliberately **conservative in two directions**: against every
live window above the shell rather than only the damaged ones (the dirty set is closed upwards over
occlusion *inside* the pass, which this pre-pass cannot see), and against the sprite's **box** rather
than its painted mask (the box is snapshotted without the sprite lock, so it must be the outer
extent). A false positive costs the pre-WC-I behaviour for one pass; there is no false negative,
because every pixel the sprite paints lies inside the box it reports and every window the pass can
paint is a window the test considered.

Nothing WC-D depends on moved: a window this pass paints is a window whose intersection test ran, so
`verify_window` still never reads a rect with the sprite on it.

#### Witnesses

`[wc-i] rollup scope={fixture|desktop} windowed_flushes=N intrusions=N cursor_passes=N
cursor_brackets=N -> {CLEAN|INTRUDED|UNWITNESSED}`

* `intrusions` — desktop presents that wrote background pixels inside a live window's box. The blip is
  this number being one per strip tick; the fix makes it 0.
* `cursor_brackets`/`cursor_passes` — before WC-I these were equal by construction.
* `scope=fixture` fires at the end of the window-verb witness block and proves the counters are wired.
  `scope=desktop` fires once the desktop has presented over a live window layer 64 times, which is the
  first point the verdict means anything. The verdict is `CLEAN` only when the desktop ran over a
  window layer **and** never intruded; a boot where that never happened reports `UNWITNESSED`, so an
  empty run can never be read as a pass.

`[wc-i] reopen closed=W reopened=W survivor=W both=B reopen=B survivor_px=B -> PASS|FAIL` — the
close→reopen scan-out read-back (see below).

#### The close→reopen / undying-vug cluster: what this arc can and cannot say

P60 also reported that a relaunched vug shows an **empty** window, that no further vug then displays,
and that those same relaunched vugs are **unkillable** (`kill armed but unconfirmed`, no
`[skill] killed ... confirmed=1`). A hypothesis put to this arc was that WC-H's back layer carries
per-window state that teardown fails to rebind, so a recycled slot presents into a dead layer.

**That hypothesis is refuted, twice over.**

*By inspection*: `STAGE` carries no window identity at all. It is one global `Vec<u8>`, `try_lock`ed
per composite, grown-only, and `paint_window`'s opening `fill_rect` covers the whole clipped box dense
before the present — so every byte a present copies was written by that present. Nothing in `close`,
`close_owner`, `close_compat` or `win_close_asid` touches it, and there is nothing there to rebind.
The only per-id compositor state is `VERIFIED` (a WC-D one-shot latch, already cleared in
`create_inner` precisely because ids are recycled slot aliases) and `wcg`'s per-id sample budgets;
neither writes a pixel.

*By read-back*: `wm::reopen_selftest` (`[wc-i] reopen`) drives the exact sequence in QEMU — create A
and B, present both, close A, create C into the slot A vacated (the `close win=1` / `create win=1`
aliasing the bench log shows), present C — and reads the **scan-out** back at each content origin. It
reports `PASS` on the gate: the reopened window's pixels are on the panel and the untouched survivor's
still are.

So the compositor is not where those pixels are lost. The bench evidence points the same way once the
fourth symptom is added: the relaunched vug never crosses the kill boundary, and an app that never
reaches its checkpoint also never reaches its first `SYS_WIN_PRESENT`. A window whose owner has not
presented shows kernel chrome over a **zeroed** surface — `boot::build_slot` scrubs the slot's FB
region on recycle — which is precisely "an empty window". `[wc-d] verify ... nonzero=262144 -> PASS`
in the bench log is not a contradiction: it is the *one* vug that did present.

**Out of lane, reported not touched.** The remaining thread is where a relaunched vug on a recycled
slot/ASID spins before its first present, and why an armed kill never confirms against it. That lives
in the EL0 launcher / slot-recycle / task-teardown path (`arch/aarch64/boot.rs::build_slot` +
`teardown_user_slot`, `sched.rs`'s kill boundary, the shell's `skill`), not in the WC-* files this arc
owns. It needs its own arc and its own witness.

#### WC-I gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

QEMU raspi4b, forced bench geometry — **no metal in this arc**; final verification is attended metal.

`./arroyo check` green both arches · `./arroyo test-arm` → xHCI MISSION SUCCESS ·
`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60` → MBENCH **63/63 required, 0 forbidden**, with
every `[wc-g]` / `[wc-h]` / `[wc-d]` / `[wc-fv]` witness unchanged and green.

```
[wc-i] reopen closed=1 reopened=1 survivor=2 both=true reopen=true survivor_px=true -> PASS
[wc-i] rollup scope=fixture windowed_flushes=0 intrusions=0 cursor_passes=335 cursor_brackets=0 -> UNWITNESSED
```

`UNWITNESSED` is the correct and expected gate verdict, not a weak pass: `status_tick` is not spawned
under QEMU and the headless battery delivers no input, so the desktop never presents over a live window
layer there. `cursor_brackets=0` against 335 passes is likewise wiring only — raspi4b delivers no HID
pointer report, so the sprite is never drawn. Both numbers carry their claim only on the bench.

#### Metal watch-list

* **The blip is gone.** Several vugs running: no simultaneous per-second fuzz. `[wc-i] rollup
  scope=desktop ... intrusions=0 -> CLEAN` on the wire is the assertion; a non-zero `intrusions` means
  a desktop path is still writing into a window box and names it as the cause rather than leaving it to
  a photograph.
* **`[wc-h] rollup ... declines=0` should now hold with several windows**, not just one. The
  `reason=lock` declines WC-H anticipated were largely the desktop repaint compositing on a second
  core; with the blanket repaint gone, that contention source is gone with it. Residual declines are
  genuine two-app concurrency.
* **The cursor is solid.** `cursor_brackets` far below `cursor_passes` is the number; the operator
  reading is a sprite that stops flickering while vugs render.
* The status strip and console must still repaint normally *around* windows — the subtraction is
  per-row-span, so a window overlapping the strip should show the strip either side of it and never
  through it.
* `erase` is still unstaged and still fills the desktop directly (unchanged by this arc), so a torn
  erase on close/raise remains the WC-H follow-on it always was.

### CURSOR-3 — the sprite rides the present

**P61 (attended, Pi 4 1920×1200) confirmed WC-I's cursor half and bounded what it left.** The verdict
list: the cursor no longer flickers over the desktop or the status strip — but it is still *spotty
over a vug*, i.e. specifically while the pointer sits on a live window presenting at ~60 fps. That is
not a regression in WC-I; it is the case WC-I documented as remaining, arriving on the bench with a
name.

#### The remaining case is a duty cycle, not a race

WC-I made the bracket conditional: `composite` takes the sprite off the panel only when
`cursor::sprite_box()` intersects a live window above the shell. Over the desktop that is never, and
the flicker went with it. Over a window it is *every present* — and between the `undraw` and the
`repaint` sits the whole of `draw_window`: a full off-screen compose plus `bh` row copies,
milliseconds during which the sprite is simply **not on the panel**. At 60 presents a second the
sprite is absent for a large and irregular fraction of every second, and with two vugs the two cores'
brackets interleave. No amount of care *inside* the bracket shortens that interval, and
`undraw_locked`'s colour guard — correctly — declines to restore pixels the other presenter has since
taken. Holes, moving. The bracket's cost is structural: it is a hole in time, and the fix has to
remove the hole rather than narrow it.

#### The mechanism: compose the cursor into the back layer

WC-H already composes each window into a cached-RAM back layer and presents it as contiguous
full-box rows. CURSOR-3 paints the sprite **into that layer**, after the window is composed and
before the rows are copied:

```
stage_window:   paint_window(layer)            # window chrome + upscaled content
                cursor::compose_into(layer)    # <- CURSOR-3: sprite, last into the layer
                for y in 0..bh { fb.blit(row) }  # unchanged WC-H present
```

The cursor therefore reaches the panel **inside the same row copies the window does**. There is no
undraw phase for those pixels and no interval in which they are window-only: the present is atomic to
exactly the degree the window's own pixels are, which is WC-H's whole claim. Nothing extra is written
to the front buffer, and the trailing `flush_range` already covers the sprite because it is inside
those rows — WC-H's contiguous-row present discipline is untouched.

The save-under must come from the **layer**, and that is the load-bearing detail. The pixels the
sprite hides are the window's, and at overlay time they exist only in the layer — the front still
holds the previous frame. Reading them back from the front afterwards (what `draw_locked` does) would
capture the sprite's own `FILL`, and the next restore would stamp a white arrow permanently into the
window's rect. So `compose_into` saves from the layer into a published plan (`cursor::OVERLAY`), and
the composite's tail *installs* that plan instead of painting.

#### Three tails, and the new one writes no pixels

`composite_inner` now returns `CursorTail`:

| tail | meaning | cost |
| --- | --- | --- |
| `Untouched` | the pass never went near the sprite (WC-I) | `ensure_drawn` — one lock, one bool |
| `Repaint` | the bracket ran to completion (WC-I) | restore → save → draw |
| `Adopt` | the pass carried the sprite through a staged present | install the plan; **zero framebuffer writes** in the common case |

`adopt_overlay` takes `SPRITE`, installs the plan (geometry + the layer-derived save-under) and marks
the sprite drawn — the panel already holds it. Two things are then normalised under the same
acquisition: a sprite another core drew in the meantime is taken down *before* the plan is installed
(its `saved` is front-derived and correct, so restoring it first leaves exactly one sprite on the
panel), and a pointer that has **moved** since the plan was taken falls through to the ordinary
`refresh_locked`. Installing the plan first is precisely what makes that second case safe: the module
now knows those panel pixels are the sprite's and what the window had under them, so the undraw puts
the window's pixels back instead of a save-under capturing the overlay's own `FILL`.

#### Whole-box containment, deliberately

The overlay is taken only when the sprite's box lies **entirely** inside the window's clipped outer
box. A straddling sprite would need per-pixel bookkeeping of which pixels came from a layer and which
from the front, merged across the several windows and the desktop one sprite can span — bookkeeping
whose failure mode is a stamped arrow. A straddling sprite keeps WC-I's bracket unchanged: correct,
and no worse than before. The sprite is one glyph cell plus a shadow block (36 px at the 4× cap), so
it is wholly inside the window it is pointing at for all but its own width at the frame.

#### Every WC-I invariant, preserved and stated

* **`SPRITE` never joins F4's drain wait set.** The plan is snapshotted (and the sprite undrawn) in
  exactly the place WC-I put the bracket decision — *before* the `BlitGuard` registration.
  `compose_into` runs inside the guard but takes only `OVERLAY`, and only with `try_lock`, which is
  the same discipline and the same reason WC-H's `STAGE` uses one: a contended pass declines the
  overlay and falls back to the bracket. `adopt_overlay` takes `SPRITE` in the tail, after the guard
  has been dropped. Lock order `SPRITE → OVERLAY`, never the reverse.
* **The no-intersect path is unchanged.** A pass with no sprite on the panel does one
  `sprite_plan()` — the same acquisition `sprite_box()` was, now also carrying the block scale so the
  geometry is one snapshot rather than two — and takes the `Untouched` tail.
* **`ensure_drawn` semantics are untouched**, including `erase`'s contract that the composite which
  follows puts the sprite back.
* **No verified pixel is ever read with the sprite on it.** `wcg::end`'s `fbbad` count and
  `verify_window`'s scan-out verdict both read this window's destination pixels back and compare them
  against its *source* surface, and a cursor legitimately composited into those pixels would read as
  a blit defect. So the plan is withheld from any window with a live WC-G probe or an unspent WC-D
  bit, and from a sprite overlapping a WC-F reserved box (the probe paints into the front *after* the
  pass, so it would overwrite pixels the plan claims). All three are budgeted one-shots: they cost
  those few passes WC-I's bracket and nothing else.

#### Witnesses

```
[cursor3] present tail=adopt offers=1 taken=1 -> COMPOSED     # first 8 passes that touched the sprite
[cursor3] rollup scope={fixture|desktop} planned= offers= taken= adopt= repaint= ensure= -> VERDICT
```

`planned` counts passes that took the bracket *and* had an eligible plan to hand down; `offers`/`taken`
are per-window overlay attempts and successes (`offers - taken` is a straddling sprite, a contended
plan lock, or an unreadable layer — every one a **missed improvement**, never a defect); `adopt` /
`repaint` / `ensure` are the three tails. Verdict: `COMPOSED` when the mechanism ran, `BRACKETED` when
it was offered and never landed, `UNWITNESSED` when it was never offered, `INCOHERENT` if `taken`
exceeds `offers` (an overlay that landed without being offered — a wiring defect). The rollup is
printed alongside `[wc-i]`'s, from the same harness and at the same two scopes.

#### Gate scope, stated honestly

```
[cursor3] rollup scope=fixture planned=0 offers=0 taken=0 adopt=0 repaint=0 ensure=335 -> UNWITNESSED
```

**QEMU cannot witness this fix, and the verdict says so.** raspi4b delivers no HID pointer report, so
`pal::cursor::visible()` is false for the whole boot, the sprite is never drawn, `sprite_plan()` is
always `None`, and every counter is 0. The gate proves **no-regression only**: the window path still
runs its passes (`ensure=335` — all of them through WC-I's cheap tail), settles every one through a
tail, and never took an overlay it was never offered. `UNWITNESSED` is the correct outcome, and it
exists so a pointerless run can never be read as evidence for the mechanism.

`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60` → MBENCH **64/64 required, 0 forbidden**;
`./arroyo test-arm` → MISSION SUCCESS; `./arroyo check` green both arches. Every `[wc-d]` / `[wc-g]` /
`[wc-h]` / `[wc-i]` witness unchanged.

#### Metal watch-list

* **The verdict is `taken > 0` with `offers == taken` while the pointer rests on a vug**, and the
  operator reading is a cursor that stays solid — no holes, no flicker — while the window presents
  under it.
* `adopt` should dominate `repaint` while the pointer is over a window. A high `repaint` with
  `offers > 0` means the overlay is being offered and declined; `offers - taken` names which of the
  three declines, and a straddling sprite at a window edge is the expected benign one.
* Dragging the pointer *across* a window edge should degrade smoothly to WC-I's behaviour and back —
  a visible step in flicker at the frame is the containment rule doing its job, not a defect.
* A cursor left over a window while WC-D or WC-G spends its one-shot budget will bracket for those
  few passes. Early-boot only, and `[cursor3] present tail=repaint` names it.
