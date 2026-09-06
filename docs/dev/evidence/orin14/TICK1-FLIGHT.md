# TICK1 — the first metal flight of the periodic EL1 tick (`UNAOS_BSPTICK=1`)

Orin 14, executor BSPTICK, 2026-09-05. Answers orin-ledger §"The five gaps" #1's first metal
question and ticks §F "CPU cores / SMP" + A21. **A SEPARATE flight image from render5** — Peter's
SMP D1–D5 ruling is still pending, so the tick is never folded into the render knob line; this
image exists so the gap's first question can be asked on the bench in one boot. Nothing in this
arc changes code: the read pass (§A) found no compose defect, so `timer.rs`/`boot_tegra.rs` are
untouched, no gate leg ran, and the knob-off image is the render5 image by construction.

Every line number below is the tree at `2a04fb4a` (hw-jetson tip). The staged image is built-from `61393272` = `2a04fb4a` + this arc's two docs-only WIP commits (kernel bytes identical, §B).

## A. The read pass — does the tick's IRQ path exist at EL1 on the tegra image?

**Knob → feature → sites.** `unaos/arroyo:794` maps `UNAOS_BSPTICK=1` to `bsptick`
(standalone, does not imply `tegra`; `esp-jetson` forces `tegra`). `unaos/crates/kernel/Cargo.toml:757`
declares it. Every site is `all(tegra, bsptick)`-gated: `timer.rs:634-760` (tail block —
`BSPTICK_CORE`, `BSPTICK_COUNT`, `el1_bsptick_start`, `bsptick_witness`), one statement appended
to `on_tick`'s closing line (`timer.rs:192`), one statement appended to the terminus line
(`main.rs:2717`: `el1_oneshot_proof(); #[cfg(feature = "bsptick")] el1_bsptick_start(); …
run_capstone_boot_core(0)`). Type-checked armed by the `arm-tegra-bsptick` matrix leg
(`arroyo:3176`).

**Where the arm sits in the boot, and what the JM6 drop leaves behind.** `boot_tegra.rs:137`
masks DAIF, `:150-158` sets `CNTHCTL_EL2 |= EL1PCTEN|EL1PCEN` (= 0x3: EL1 may read CNTPCT and
program CNTP_* without trapping) and writes `cntp_ctl_el0 = 0` (the JM6 disarm), `:162-166`
sets `HCR_EL2 = RW` only (IMO=FMO=AMO=0: a physical IRQ taken at EL1 targets EL1, not the
abandoned EL2 vectors), `:186-191` erets with `SPSR_EL2 = 0x3c5` (EL1h, DAIF masked). The
render4 wire confirms exactly this latch at EL1: `[irqel2a] pre-arm cpu=0 CurrentEL=1 DAIF=0x3c0
(I=1) | HCR_EL2=0x80000000 IMO=0 FMO=0 AMO=0 TGE=0 E2H=0 RW=1 | CNTHCTL_EL2=0x3 EL1PCTEN=1
EL1PCEN=1 | ICC_SRE_EL1=0x7 ICC_SRE_EL2=0xf | CNTP_CTL=0x0` (`render4-boot1.log:406`).
`main.rs:2682` is the drop; `:2689` `timer::set_not_live()`; `:2716` `exceptions::install()`
(VBAR_EL1, `exceptions.rs:394-413`, EL chosen from CurrentEL at runtime); `:2717` the terminus
line. So the arm runs AFTER the post-drop vector install, on the boot core, at EL1 — the order
`el1_bsptick_start`'s contract requires (`timer.rs:685-691`). No `CNTKCTL_EL1` write exists
anywhere on the tegra path (`mmu_tegra_el0.rs` only saves/restores DAIF around its TTBR0 swaps,
`:623-635`, `:913-931`); the EL1 kernel touches CNTP_* as the EL2 latch allows.

**Vector table.** `exceptions.rs:124/133/142/151` — all four IRQ entries branch to `__vec_irq`
(`:274-279`); on tegra `irq_bank!`/`irq_unbank!` (`:41`, `:44`) select ELR_EL1/SPSR_EL1 vs the
EL2 pair by a runtime `CurrentEL` test, which is what 0a60e260 added and what the IRQEL-RT
one-shot exists to prove. **Metal-proven on render4**: `:: IRQEL-RT: first IRQ taken at EL1 on
cpu 0 — banked vector path live (ELR_EL1 bank) ::` (`render4-boot1.log:408`). The current-EL
SPx entry (0x280) is the one the tick lands on while the kernel runs; the lower-EL entry (0x480)
is the one it lands on while an EL0 tenant runs (risk R3).

**DAIF.** The one-shot restores its entry DAIF (masked) and disarms CNTP (`timer.rs:528-535`),
leaving the state `el1_bsptick_start` expects. `el1_bsptick_start` (`timer.rs:693-727`) re-seeds
`INTERVAL = CNTFRQ/250` (= 125 000 cycles at the Orin's 31.25 MHz; `on_tick` reloads TVAL from it
every tick), prints the arming banner BEFORE unmasking (the serial lock is free when tick 1's
witness fires), then `arm_this_core()` (`timer.rs:120-129`: `gic::enable_ppi(30)` at this core's
redistributor via `enable_banked`, `gic.rs:616`; TVAL; `CNTP_CTL_EL0 = ENABLE`, IMASK=0; isb)
and finally `msr daifclr, #2` + isb — IRQs stay unmasked from here on. The one-shot proved the
same `enable_ppi` + TVAL + ENABLE sequence delivers at EL1 with this exact latch.

**IRQ dispatch at EL1.** `gic.rs:957-975` `handle_irq_v3`: IAR → INTID 30 → on tegra
`el1_proof_intercept()` (returns false once `EL1_PROOF_CORE` is back to none, `timer.rs:430-455`)
→ `on_tick()` (`timer.rs:158-192`: TVAL reload FIRST + isb — deasserts the level-sensitive PPI
before the EOI, the ordering that matters on metal — then per-CPU `ticks`, then the shared
`TICKS` (the boot core is not in `AP_LOCAL_TICK`, so the global clock resumes advancing after
the JM6 freeze), then `bsptick_witness()`) → EOIR1. The `bsprun` post-EOI `timer_preempt` arm
is NOT compiled (`bsprun` off), and `SCHED_ACTIVE` has no tegra setter without it, so a tick can
never context-switch: the terminus stays cooperative. `bsptick_witness` (`timer.rs:739-757`)
counts only the arming core (`BSPTICK_CORE == cpu_index`, the IRQEL-CORE guard — the five EL2
APs reach this same dispatch ~1250x/s with their own PPI 30 and fall through) and prints at
n == 1 and every 250th tick.

**What runs with IRQs unmasked, and what wakes the pump.** After the terminus line,
`run_capstone_boot_core(0)` (`sched.rs:10135-10201`) spawns CAPSTONE and runs `while
dispatch_next(cpu)`. `dispatch_next` (`sched.rs:5311`) masks IRQ only across the queue pop and
the context switch; `task_trampoline` (`:1647-1648`) unmasks before the body, `sleep_ticks`/
`yield_now` mask across the switch and unmask on resume. The JD2 console pump is a task body,
so it runs UNMASKED: ticks land inside the pump. The pump's cadence is `arch::ms()` (CNTVCT
/ CNTFRQ, `mod.rs:325`) — independent of `TICKS` — so the `[orinrender] census` and
`[serialrx] rx=` rates do not change when `TICKS` starts advancing. **`hlt()` (`mod.rs:270`) is
the JD3 poll-spin on tegra post-drop**: `set_not_live()` cleared `LIVE` at `main.rs:2689`,
`el1_bsptick_start` deliberately does NOT set it, and the boot core is not in `AP_LOCAL_TICK`,
so `is_live() || this_core_has_local_tick()` stays false → `spin_loop()`, never WFI. Nothing
about the pump's progress depends on the tick; the tick is additive. (And `run_capstone_boot_core`
never drains sleepers — `drain_due_sleepers` is called only from `run()`, `sched.rs:6447` — and
the only `sleep_ticks` callers are `baremetal`/x86 tasks, so a now-advancing `percpu.ticks`
wakes nothing new on this image.)

**Compose with `deskcascade` + `orinrender` + `orinrx` + `orinclick` (the render5 line).** The
render pass is a busy-poll on core 0 inside the pump task (`[orinrender] census passes=253350`
per second on render4) — a 250 Hz IRQ on the same core costs ~250 handler entries/s against
~250k passes/s. Masked spans checked:
- serial: `_print` (`serial.rs:178-240`) = `without_interrupts` (save/restore DAIF,
  `mod.rs:406-418`) + `SERIAL_PORT.try_lock()` + staging ring — a tick landing mid-print prints
  its witness via the same path: `try_lock` fails, the line goes to the ring and the holder
  emits it, so `[orinbsptick]` lines can appear a line or two late, never deadlock (R4).
- `SYS_WIN_PRESENT` (`syscall.rs:12520-12569`): `IrqGuard::mask_save()` holds — save/restore,
  nest-safe; a tick raised inside the hold stays pending (level-sensitive PPI, ISTATUS held until
  `on_tick` rewrites TVAL) and lands at the unmask. Delayed, not lost. Same for the 12 `fbcon`
  holds and `video::panel_info_nonblocking` (`video/mod.rs:415`).
- `mmu_tegra_el0.rs:623/913`: DAIF saved + I masked around every TTBR0 swap, so a tick cannot
  land mid-swap.
- `orinclick` (`arroyo:870-909`): `wc_click_route` from the pump's `Event::Button` arm +
  `orin_click_census` on the sweep line (`main.rs:3014`); implies `tegra_el0` only; touches no
  window-table state the cascade refuses (the cascade refuses only ORINCONWIN/ORINDESK/
  ORINTENANT). Composes; this closes A20's "verify it composes" clause on paper — the wire
  answers it.
- The knob-off image of this line IS the render5 image: every `bsptick` site is a tail block or
  a statement appended to an existing line, so no panic `Location` moves (the standing
  Location-shift convention, `timer.rs:634-635`, `main.rs:2717`'s trailing comment).

**The one honest gap the read pass leaves.** Periodic re-delivery at EL2 on this board is
metal-proven (`AARCH64: timer heartbeat live (first tick)` during the EL2 stretch,
`render4-boot1.log`), and a single delivery at EL1 is metal-proven (`:408`). What has never
been observed is the SECOND delivery at EL1 — i.e. that `on_tick`'s TVAL-reload-then-EOI
deasserts and re-pends PPI 30 correctly with `ICC_SRE_EL1=0x7` under the EL2 latch, and that
`HCR_EL2.IMO=0` keeps routing a level-sensitive PPI to EL1 on every assertion, not only the
first. That is the gap's question, and only the wire answers it.

## B. The knob line and the staged image

Render5's line + the one knob. Never add ORINCONWIN/ORINDESK/ORINTENANT (the cascade refuses a
non-empty table); never add `UNAOS_BSPRUN` (that is arc 2 and needs the `sched.rs` grant).

```
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 \
UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_BSPTICK=1 ./arroyo esp-jetson
```

Build log: `~/unaos-bench/scratch/orin14/bsptick/build-tick1.log` (`ESP_JETSON_EXIT=0` appended).
Effective features (quoted once from the log):
`witness,ehcihid,holocron,tegra,bsptick,orinclick,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,deskcascade`.
Witness reachable: `grep -a -c '\[orinbsptick\]' target/aarch64_esp/kernel.elf` = `2`
(NOTE: the brief's `'\[bsptick\]'` pattern is 0 hits by construction — the token is
`[orinbsptick]`, 13 bytes, chosen to clear LLVM's 8-byte immediate-encode floor so `strings` on
the objcopy'd image can see it; `timer.rs:662-663`). ELF max vaddr: `0x2d9f90` (entry `0x7e378`; four LOAD segments, last RW at `0x1f1140` memsz `0xe8e50`). kernel.elf
sha256 `c82f813516ec58d067493a84067f8d9b327224f1fb8d6bdde87601a7a7cf3bd6` (identical across the two builds — run 1 from the tree at 60af59b5 with the ledger edit uncommitted, run 2 from the clean tree at 61393272 — so the two docs-only commits above 2a04fb4a moved no kernel byte).

Staged (not written to the card): `~/unaos-bench/flash/orin/tick1-20260906T0013Z-6139327/` with a per-dir
MANIFEST in the load-card.sh shape (`#` comment lines + `<sha256>  <relpath>` only, awk-validated:
`manifest_bad_lines=0`) and the global `~/unaos-bench/flash/orin/MANIFEST` line appended;
`validate-manifest.py --quiet` exit `0` (`PASS — 17 staged images all recorded`).

## C. Expected wire, and the scorer

**Once, at the terminus (in this order):** the IRQEL-RT one-shot's four lines (`pre-arm`, `armed`,
`first IRQ taken at EL1`, nothing else), then

```
:: [orinbsptick] arming PERIODIC CNTP at EL1 on cpu 0 (250 Hz, PPI30) — IRQs stay UNMASKED across the terminus; dispatch is on_tick ONLY (no timer_preempt arm in the v3 dispatch without ORIN-BSPRUN, SCHED_ACTIVE false): no preemption, the capstone loop stays cooperative ::
:: [orinbsptick] tick 1 taken at EL1 on cpu 0 — periodic CNTP live across the terminus ::
```

then the unchanged terminus train (`tegra_el0` start, `[deskcascade] -> CASCADED`, `[orinrender]
arm … click=1 cascade=1`, `:: AARCH64 SCHED (virt): boot core 0 at EL1 …`).

**Cadence, for the life of the boot:** one `[orinbsptick] tick N taken at EL1 on cpu 0` line
per second with N = 250, 500, 750, … (the witness emits at tick 1 and every 250th tick — NOT
`n=1, n=2, …`; the 250 Hz tick itself is silent), interleaved with the pump's existing ~1 Hz
`[orinrender] census passes=… presents=…` and `[serialrx] rx=… -> RX-LIVE` lines, whose
`passes=` must keep climbing at the render4 rate (~250k/s). The `[orinclick] census` every ~10 s
as before. Keys and clicks still land (A20's own question rides along).

**Scorer** — awk over the capture (the whole serial log is fine; `arm` anchors it). Kept
executable at `~/unaos-bench/scratch/orin14/bsptick/score-tick1.sh` and exercised on six
synthetic captures (one per verdict) plus the real render4 log (→ `ARM-ABSENT`):

```
awk '
/\[orinbsptick\] arming PERIODIC CNTP at EL1 on cpu 0/ { arm++ }
/\[orinbsptick\] tick [0-9]+ taken at EL/ { n=$0; sub(/.*\] tick /,"",n); sub(/ .*/,"",n); n+=0
  tl++; if (n>tmax) tmax=n; lastt=NR; if ($0 ~ /taken at EL2/) el2++ }
/\[orinrender\] census passes=/ { p=$0; sub(/.*passes=/,"",p); sub(/ .*/,"",p); p+=0
  if (arm) { cen++; if (p>cmax) cmax=p; lastc=NR; if (cen==1) cfirst=p } }
/=== AARCH64 EXCEPTION/ { exc++; if (!excline) excline=NR }
END { v="UNSCORED"
  if (arm==0) v="ARM-ABSENT"; else if (exc>0) v="EXCEPTION at line " excline
  else if (el2>0) v="FAIL-EL2"; else if (tl==0) v="ARM-MISS"; else if (tmax==1) v="NO"
  else if (tmax>=2500 && cen>=10 && cmax>cfirst) v="PASS"
  else if (tmax>=2500) v="PUMP-STALL"
  else v=(lastc>lastt) ? "TICK-DIED at " tmax : "SHORT tmax=" tmax
  printf "arm=%d ticklines=%d tmax=%d census=%d passes=%d->%d exceptions=%d -> %s\n", arm,tl,tmax,cen,cfirst,cmax,exc,v }' <capture>
```

| verdict | meaning | what it decides |
|---|---|---|
| **PASS** | `tmax ≥ 2500` (≥ 10 s of ticks, ≥ 10 witness lines), `[orinrender] census` still advancing (≥ 10 lines after the arm, `passes` strictly up), 0 `=== AARCH64 EXCEPTION` | Gap #1's first question is YES: CNTP re-arms at EL1 across the JM6 latch and the pump is unperturbed. Arc 2 (`bsprun`, sched.rs grant) becomes askable; §F row → flown. |
| **NO** | `tick 1` printed, count never advances, pump alive | The latch/re-arm question answered NO: the first delivery works (as the one-shot showed) but the SECOND never comes. Suspects in order: the level-PPI deassert/EOI ordering under `ICC_SRE_EL1` at EL1 (add an `ICC_RPR_EL1`/`GICR_ISPENDR0` snapshot at tick 1), IMASK/ENABLE state after `on_tick`, `CNTHCTL_EL2` trapping the TVAL reload. Code fix lives in `timer.rs` (jetson lane). |
| **ARM-MISS** | banner, no `tick 1` | First delivery never came — differs from the one-shot, so the diff is the arm path: `BSPTICK_CORE` guard vs `cpu_index`, or the `enable_ppi` re-assert. `timer.rs`. |
| **TICK-DIED at N** | ticks reached N then stopped, pump ran on | The re-arm works but something later masks I permanently or disables CNTP (a `daif` restore from a stale save, or the EL0 path — check whether N's timestamp lines up with the first `[pulsewin]`/tenant start: R3). |
| **PUMP-STALL** | ticks advance, census stops | The tick starves or wedges the pump — a masked-span lock (serial ring, fbcon) or the render pass. First real compose defect; find the last pump line. |
| **FAIL-EL2** | any tick line says `EL2` | `HCR_EL2.IMO` regressed — the drop's latch is not what `[irqel2a]` reported. `boot_tegra.rs`. |
| **EXCEPTION** | `=== AARCH64 EXCEPTION` | The vector/DAIF question: read ESR/ELR. EC=0x00 with ELR in the pump → IRQ frame/bank; ELR in EL0 range → R3 (lower-EL entry). |
| **ARM-ABSENT** | no banner | wrong image, or the boot died before the terminus (score A15 first). |

## D. Risks

- **R1 — the second EL1 delivery is unobserved** (§A last paragraph). This is the question, not a
  defect; NO is a legitimate result and the scorer names it.
- **R2 — AP cross-talk**: the five EL2 APs share `on_tick`; scoped by `BSPTICK_CORE`, and their
  own `arm_this_core_ap` keeps them off `TICKS`. An `offcore` count in the scorer's long form
  (`score-tick1.sh`) > 0 would mean the guard failed.
- **R3 — first asynchronous exception from EL0 on this board.** The one-shot fired inside an EL1
  spin. On this image EL0 tenants run (`pulsewin=2`, VUG, STAT), so ticks WILL arrive while
  CurrentEL=0: entry 0x480 → `__vec_irq` with SP_EL0 banking and the tenant's TTBR0 live (the
  kernel is reachable from the tenant tables — SVCs already prove that synchronously). Proven on
  the Pi's preemptive EL0 path, never on the Orin. A TICK-DIED/EXCEPTION whose timestamp matches
  the first tenant dispatch points here.
- **R4 — witness lines late or interleaved**: `[orinbsptick]` prints from IRQ context through the
  `try_lock` + staging ring; a line can trail the pump's line that was mid-flight. The scorer
  keys on content, not adjacency.
- **R5 — `TICKS` resumes advancing.** Nothing on this image consumes it (`input_wait_backstop`
  is `aarch64_el0`-gated and only re-readies parked input waiters; `sleep_ticks` callers are all
  `baremetal`/x86), but any future tegra code that assumed the post-drop freeze now sees a clock.
- **R6 — A15 (1-in-5 CPU_ON death) still precedes everything**; a boot that dies before the
  terminus scores ARM-ABSENT and says nothing about the tick.
- **R7 — not a render5 result.** If Peter wants render5's questions (A20 clicks) answered clean,
  fly render5 first; this image carries the same knobs plus the tick, so a PASS here also
  answers A20, but an A20 miss here is ambiguous (tick or click) until render5 flies.
