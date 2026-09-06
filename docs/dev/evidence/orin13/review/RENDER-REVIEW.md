# RENDER-REVIEW — adversarial review of ac27b8d2..01739a93 (hw-jetson, orin 13)

Subject: a5a66fc1 STAGECENSUS, 7ffd2122 PAINTPULSE, 01739a93 STACKSEED — all in
`unaos/crates/kernel/src/main.rs` inside the `orinrender`-gated tail
(`tegra_render_arm` 8114-8211, `orin_render_service` 8221-8356 at 01739a93).
Reviewer: read-only; no build, no QEMU. Line numbers are from the tree at 01739a93
unless a branch is named. Flight evidence is the render1 capture,
`~/unaos-bench/capture/line-acm0/orin.log` (image `render1-20260901T0347Z-c61b47e`,
features per `~/unaos-bench/scratch/orin12/build-render1.log`: witness ON).

**Verdict: no blocking finding.** Three things to fix or record before landing, ranked below.

---

## Findings, by severity

### F1 — MEDIUM — a new board-named identifier in a shared file: `ORIN_RENDER_STACK_SIZE`

- `main.rs:8198` mints `const ORIN_RENDER_STACK_SIZE: usize = 32 * 1024;` (fn-local, inside
  `tegra_render_arm`). It is the Orin twin of the Pi's `RENDER_STACK_SIZE`
  (`origin/hw-pi4:unaos/crates/kernel/src/main.rs:7244`, file-level, `baremetal`-gated, same
  value, same job: the render task's sized stack). On this branch the Pi constant does NOT exist
  (`sched.rs:10257` names it only in prose explaining that the SHELLUP `main.rs` half was not taken),
  so this is a name minted fresh, not an addition to a family already in the file.
- Peter's 2026-09-03 ruling: identifiers in shared files are named by subsystem, never by board.
  The commit's own justification ("same size and same shape as the Pi's `U7_LAUNCH_STACK_SIZE`")
  shows the subsystem name was available: `RENDER_STACK_SIZE` — a fn-local const cannot collide
  with the Pi's file-level one even after hw-pi4 lands (an item in fn scope shadows the outer one).
- GATE-FAMILY does not catch it: `origin/hw-rmbp:unaos/scripts/arch-families.sh` `scan()` greps
  `fn` names only, so consts are invisible to the ratchet. The ruling is wider than the gate.
- The probe tag `"orin-render:pass1"` (`main.rs:8337`) derives from the pre-existing task name
  `"orin-render"` (unchanged from ac27b8d2) and the `[orinrender]` witness family is pre-existing —
  those are additions to an existing family and acceptable this arc.

Failure scenario: the landing review applies the ruling and the arc bounces for a rename that
costs one line now.

### F1b — LANDING NOTE (not introduced here, but kept here) — `orin_render_service` will red GATE-FAMILY

- `fn orin_render_service` (`main.rs:8222`, from c61b47e3) is exactly the injected go-red case
  rmbp's ledger records ("injecting `fn orin_render_service` takes the family 2→3, exit 1"):
  `origin/hw-rmbp:unaos/arch-families.ledger` holds `render_service 2 render_service
  x86_render_service`. The ledger + script are on `origin/hw-rmbp` and `main`, not on this branch
  (`ls unaos/arch-families.ledger` fails here). The first `./arroyo check` after this branch meets
  trunk fails at GATE-FAMILY on this name. These commits do not add it, but they build on it; the
  landing seat must either rename (the ruling points the same way as F1) or `--update` the ledger
  with the reason in the commit message, as the script demands.

### F2 — MEDIUM — the one stack gauge is placed one pass BEFORE the task's deepest chain

- Loop order in `orin_render_service`: `pulsewin::service()` at `main.rs:8309`, then
  `ui_status::tick(&mut pal)` at `:8313`, then the present + probe under `if dirty` (`:8321-8338`).
- `pulsewin::service()` opens only when `ui_status::loads` reports `ncpu > 0`
  (`video/pulsewin.rs:598`), and `loads` answers `0` until `tick` has armed
  (`ui_status.rs:946-949`). So on pass 1: service → no open; tick → arms → `changed = true`
  (`ui_status.rs:1133`); render; probe. On pass 2: service → `open()` → `wm::create_at` (which
  composites the new row before returning, `pulsewin.rs:585-590` prose, `:458-468` call) and
  `paint()` → `wm::present(id)` (`:546-570`).
- The composite path is the one whose aarch64 stack cost is on the ledger (occ62 —
  `docs/dev/OS/01_BOOT_HAL/arch_arm64.md:9912-9915`), and on the witness leg the `[wc-g]` readback
  witness rides those presents: in the render1 capture the orin-render TRAVERSED report
  (`orin.log:41895`) follows `[wc-g] … probes=576000`/`[wc-h]` lines (`:41888-41894`) from the
  first present, and jd2-console's TRAVERSED (`:45653`) follows its own `[wc-g]`/`[wc-h]`
  composite (`:45650-45652`) after a key press.
- The commit's claim "the present is this pass's deepest call chain" is true of pass 1 and false
  of the task. `hw=` is a lifetime high-water, so a probe at pass 1 excludes the pass-2 open and
  every later pulse repaint.

Failure scenario: the next witness flight prints `[u7stk] at=orin-render:pass1 … hw=~4000
headroom=~28000` (the Pi's same body measured `hw=3264` at 32768 — hw-pi4 log), the number is
recorded as "32 KiB is generous", and the pass-2 composite that actually went through 17 KiB on
render1 is never measured. Cheapest fix: also probe on `passes == 2`, or fold one probe into the
census branch (`:8344-8350`) — witness leg only, one line per second, the census already spends it.

### F3 — LOW — the redzone witness cannot by itself say WHOSE write hit byte 0; score it with `hw=`

- `sched.rs:5482` prints `TRAVERSED` on `lz & 2`, and `guard_state` (`:1829`) sets bit 2 when the
  FIRST byte of the stack Box is not `GUARD_FILL` — i.e. the lowest address of the allocation. A
  descending SP corrupts byte `n-1` (bit 1, "entered") before byte 0; the report prints TRAVERSED
  whenever bit 2 is set regardless of bit 1, so "TRAVERSED" also matches an upward overrun from
  the heap neighbour below the stack.
- On render1 the SP reading is the stronger one: jd2-console had a genuine `entered` earlier
  (`orin.log:14934`), and each TRAVERSED is adjacent to a witnessed composite (F2). But note
  `task=1:jd2-console` TRAVERSED 8 times on the same flight, same core, same composite path
  (`:45653-45673`), on the blanket 16 KiB — the same defect on the other tegra task, spawned
  outside this gate (`main.rs:2801` region) and outside this arc. The next flight WILL still carry
  `[redzone] … jd2-console`; the scoreable claim is correctly scoped to `orin-render` and must be
  read that way.
- Rate limiter check: reports print only when the reporting task differs from the last
  (`GUARD_LO_LAST.swap`) and stop at 16 total. With orin-render fixed, jd2 prints once and never
  alternates, so the cap is not exhausted and an orin-render traversal would still print. The
  absence claim holds.

### F4 — LOW — stale prose left by the three-way composition (no code effect, knob-off unchanged)

- `main.rs:8104-8105` (`ORINRENDER_ARMED` doc) and the REFUSE text at `:8134` still say a second
  seam pass "would mint a second shell window and orphan the first row's heap store" — after
  PAINTPULSE there is no mint and no store.
- `main.rs:8224` comment lists `clear_screen` among the trait methods the `use` is for; nothing
  in the function calls it now (`pw, ph` survive only for the DECLINE line at `:8302`).
- `arch/aarch64/display_tegra.rs:2498` says `pulsewin::press_route` "returns on a NONE window id …
  unreachable on this build" — on an `orinclick` + `orinrender` image the window now opens
  (`:1328` routes through `wc_click_route`, whose furniture arms include `pulsewin::press_route`
  per `:2494-2495`), so that arm becomes reachable. Correct behaviour; the prose is now wrong.
- The composed code itself is consistent: `shell_id` is an immutable `WIN_NONE` and the
  `shell_id == wm::WIN_NONE` test at `:8276` is constant-true (harmless); `shell_declined` still
  latches both arms; nothing shadows `info`/`pw`/`ph`; `WRITER` is copied out at `:8142`/`:8227`
  and never held across a `wm` call; the present still happens after the stage is reserved
  (reserve at `:8159`, spawn at `:8199`, present inside the task).

### F5 — LOW — what the armed pulse window will look like on the Orin (flight expectation)

- It opens on pass 2 with content painted (`pulsewin.rs:546-570`) — it does not repeat defect 3.
- Placement: bottom-left, one gap in, above the strip (`pulsewin.rs:455-463`), width 2/3 of the
  panel; the routed console window is centred in the work area (`video/fbcon.rs:1967-1968`,
  970x644 on render1). On 1920x1200 the two overlap and the later-created pulse row sits above the
  console's lower-left. Not a defect of these commits (the Pi's placement), but the next capture
  will show it; the operator can drag it. `ncpu` reads 6 (`percpu.rs:156 METER_CPU_COUNT`).
- Allocation: ~1280 x (menu + 6 rows) x 4 B, well under the 48 MiB heap; `try_reserve_exact`
  declines rather than panics (`:412-416`).

### F6 — INFO — knob-solo image: the first present still erases the panel console (pre-existing)

- On a non-`orinconwin` image both arms decline, and the first present blits `DESKTOP_BG` over
  the whole panel minus wm rows and strips. The JD2 console draws through its OWN `Screen` over the
  same front (`main.rs:2874`), and before phase 2 the fbcon boot log is on the panel directly; the
  occluder walk protects neither. Before these commits the same present blitted BLACK — so this is
  unchanged in kind, only in colour. Named because the `no-painter` DECLINE says "the strip pass
  below is what this rung contributes" while that pass also erases the console it declines to
  window. Nothing to do this arc; worth one line in the doc.

### F7 — INFO — census cadence

- `cntpct()`/`cntfrq()` are ungated (`timer.rs:62`, `:339`; the `#[cfg(not(pi))]` at `:55` belongs
  to the static on `:56`). The Orin reads CNTFRQ = 31.25 MHz (`timer.rs:59-60`, "capture-proven"),
  so the 62.5 MHz fallback never fires there; if it ever did the cadence would be ~2 s, consistent
  with `timer::init`'s own substitution (`:79-83`). Seed arithmetic is right in both wrap cases
  (`now.wrapping_sub(seed) >= ticks` on the first pass). Measured on render1: 3576 census lines of
  4309 post-spawn lines = 83%, matching the commit's 82.2%.

---

## Claims verified as sound (with where I read them)

1. **Nothing outside the gate.** Every hunk starts at or after `main.rs:8138`; the two functions
   are gated at `:8114` and `:8221` (`#[cfg(all(target_arch = "aarch64", feature = "orinrender"))]`)
   and close at `:8211` / `:8356`, the file's last line. `git diff --stat` touches main.rs only.
   The knob-off image is unchanged.
2. **`#[cfg(feature = "witness")] if passes == 1 { … }`** is a legal statement attribute: compiled
   both ways with rustc 1.93.1 (`~/unaos-bench/scratch/orin13/review/cfg_if_test.rs`).
3. **`stk_probe` is witness-gated at the definition (`sched.rs:89`) and the call (`main.rs:8335`),
   takes no lock** (reads `SCHED[cpu].current` Acquire and scans the stack; `:90-146`), and
   `current` IS set on the capstone core: `run_capstone_boot_core` (`:10135`) drives
   `dispatch_next` (`:10201`), which publishes `current` at `:5433` before switching in.
   `spawn_inner` paints poison under `witness` (`:3636-3637`) before the guard fill, so the scan
   reads real depth. `spawn_stack` is ungated (`:3721`); `TASK_STACK_SIZE` untouched.
4. **`fill_screen` publishes.** `Screen::fill_screen` = `back.fill_screen` + `mark_full`
   (`screen.rs:1111-1114`); `mark_full` sets the single full-panel rect (`:1085-1087`), which
   `Screen::new` had already set, so no extra present. `wm::DESKTOP_BG` is an ungated `pub const`
   (`wm.rs:2400`), `pub mod wm` is unconditional (`video/mod.rs:52`), and `use …::video::wm` is in
   scope at `:8225`. No interaction with `DESKTOP_SCENE`: its only setter is the Pi render path
   (`main.rs:5576`), so on the Orin `ui_status::tick` stays live and returns dirty on pass 1
   (`ui_status.rs:1133`, `:1285`). a1cf4900's deletion of `clear_screen` was about
   `retire_desktop_chrome`, which is not reintroduced.
5. **Declining `adopt_desktop_bg` is right.** It is a process-global latch consumed by
   `Screen::new` (`screen.rs:853-864`, `:987-995`); `video::witness::run` builds a heap-backed
   `Screen`, flushes, and returns `Err("baseline flush left non-zero front")` on any non-zero byte
   (`witness.rs:72-78`), reachable from `selftest.rs:467`. Arming the latch would fail that leg.
6. **The routed console window survives the seed.** On aarch64 + `desktop_firmware` (which
   `orinrender` implies — `Cargo.toml:2356`) `present_background` subtracts wm rows AND furniture
   strips from every desktop rect (`screen.rs:1560-1585`, arch-neutral `wm::occluders`
   `wm.rs:3150`).
7. **`reserve_stage` is idempotent and grow-only**: `try_lock`, `if stage.len() < target` then
   `try_reserve` + `resize` (`wm.rs:18171-18215`); target = `min(panel bytes, MAX_STAGE_BYTES =
   4 MiB)` (`:17750`, `:18062-18065`), the same target the sibling seams already reserved
   (`main.rs:7344`, `:7941`), so the second call allocates nothing. On a knob-solo image nothing
   can hold `STAGE[0]` at the terminus, and a contended entry falls back to `STAGE_RESERVED`
   (`:18181`), so a spurious 0 is not reachable. Witness leg prints one more `[wedge12]` line.
   Placement (after the panel floor, before arm/spawn) matches deskseam and orinfurn.
8. **`pulsewin::arm()` is a bare latch** (`pulsewin.rs:612-614`), module gated on
   `desktop_firmware` (`video/mod.rs:666-670`). `open()` takes `WRITER` briefly and `wm::TABLE`
   via `create_at`; the render task holds neither (front copied out at `main.rs:8227`). No sleeps
   or barriers in `create_at` (`wm.rs:860-882`) or `present` (`:1075-1077`), so the never-sleeps
   rule for the capstone core is not violated.
9. **No new witness token.** New `[orinrender]` lines only (`REFUSE reason=stage-unreserved`,
   `DECLINE reason=no-painter`); `[u7stk]` and `[wedge12]` are pre-existing subsystem tokens.

## Summary for the caller

- No blocker. Land after: (F1) rename `ORIN_RENDER_STACK_SIZE` → `RENDER_STACK_SIZE`;
  (F2) add a second probe at pass 2 or on the census cadence; (F1b) decide the
  `orin_render_service` GATE-FAMILY question before the trunk merge, since the ledger on
  `origin/hw-rmbp` reds on that exact name.
- Everything else the three commits claim about locks, gates, damage, the latch, the stage and the
  cadence checks out at the lines cited above.
