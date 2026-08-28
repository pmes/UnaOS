# RASTPORT — is `rast_demo.rs`'s multi-core rung portable to x86?

Arc rmbp-7, executor RASTPORT. Base `948f7859` (hw-rmbp), branch `exec-r7-rastport`.

**Milestone 1 verdict: the claim is VERIFIED WITH AMENDMENTS.** Every load-bearing
call `run_mc` makes has an x86 equivalent with compatible semantics, so the port is
mechanical rather than a design task, and it was carried out. Three amendments:

1. The triage's symbol list was **incomplete** — three bridges are needed, not two.
   `sched::spawn` has a **different signature** on x86 (a 5th `priority` argument).
2. It missed the fact that decides where the demo can run on x86: **arming `rast` on
   x86 compiles the entire SCHED-X86 render/service handoff out of the boot**
   (`main.rs:1468`). That is not an obstacle — it is what makes the port work — but it
   makes the brief's starvation question vacuous rather than answered (§5).
3. **The Screen API does NOT behave identically under the compositor** (§4b). On x86
   with `wc` armed and a real Kepler takeover, the demo's pixels are silently discarded
   before reaching the framebuffer. Timings and serial witnesses stay honest; the glass
   stays unchanged. **QEMU cannot show this** — it has no Kepler.

---

## 1. Prior art — nobody landed this already

| check | result |
|---|---|
| `git log -S 'rast_demo' --all` | 25 commits, all aarch64 (`tegra`/`pi`) or doc. No x86 MC port. |
| `git log -S online_cpu_count --all` | 3 commits: `7ab91c99` (RAST-MC, aarch64), `61eb3427` (BG-SPREAD), `57d6fe7a` (SCHEDPAR). None adds an x86 twin. |
| `57d6fe7a` (SCHEDPAR) in base? | **No** — `git merge-base --is-ancestor` returns non-zero. Its precedent (signature-matched twins) is followed here; its code is not present. |

## 2. Verdict table — every external symbol `run_mc` touches

| symbol (aarch64 use site) | x86 equivalent | semantic match | evidence |
|---|---|---|---|
| `crate::arch::percpu::NUM_CPUS`<br>`rast_demo.rs:241` | `crate::arch::gdt::MAX_CPUS` = 8 | **YES.** Both are *the scheduler's per-CPU array bound*, which is exactly what `MC_MAX`'s doc claims it must be ("a probe spawn can never index a run queue out of range"). aarch64 sizes `SCHED`/`CPU_BUSY`/`ONLINE_MASK` by `NUM_CPUS`; x86 sizes `RUN_QUEUES`/`ONLINE_MASK`/`CORE_CR0` by `MAX_CPUS`. | `arch/aarch64/percpu.rs:88,90`; `arch/aarch64/sched.rs:481,1855`; `arch/x86_64/gdt.rs:75`; `arch/x86_64/sched.rs:1958,3020` |
| `crate::arch::sched::online_cpu_count()`<br>`rast_demo.rs:429` | **did not exist.** x86 has the same `ONLINE_MASK` state and a private `mark_online`/`cpu_dispatching`, but no public count. | **YES, once written.** Both masks mean the identical thing — *"this core has entered `run()` and is therefore actually DISPATCHING"*, explicitly stated in both files. See §3 for the core-0 subtlety. | aarch64 `sched.rs:4070-4071` (the fn), `3120-3122` (`mark_online`); x86 `sched.rs:3010-3020` (the doc + mask), `3097-3106` (`mark_online`/`cpu_dispatching`), `5924` (`run()` sets it) |
| `crate::arch::sched::spawn(name, entry, arg, cpu)`<br>`rast_demo.rs:469` | `spawn(name, entry, arg, target_cpu, priority: u8)` | **SIGNATURE MISMATCH — the triage missed this.** x86 takes a 5th `priority` arg and returns `()`; aarch64 takes 4 and returns `u64`. Semantics of the shared 4 are identical. Bridged with a per-arch call shim, not by changing either public API. | aarch64 `sched.rs:3710`; x86 `sched.rs:2229` |
| — the **pin** contract inside `spawn` | `let steal_ok = target_cpu == CPU_AUTO;` | **YES.** An explicit core index is a no-migrate pin on both arches, which is precisely what makes `MC_ALIVE[cpu]` an honest "this core dispatches" probe rather than a guess (`rast_demo.rs:349-351`). | x86 `sched.rs:2180`, `2168-2169`; and `!steal_ok` filters the steal path at `sched.rs:1792,1870` |
| — spawn onto a **non-dispatching** core | task sits in that core's queue, never dispatched | **YES** — same degradation, and it is the case `MC_ENLIST_MS` exists to survive. x86 documents the hazard in the same words ("spawned, never dispatched, never joined"). | x86 `sched.rs:3014-3016`; `rast_demo.rs:463-467` |
| `crate::arch::ms()` | `apic::ticks()` → `APIC_TICKS` | **YES, with a caveat (§4).** Both are free-running-ish ms-since-boot. Different mechanism: aarch64 reads CNTVCT (hardware counter); x86 reads a counter bumped by **the BSP's timer ISR alone**. | x86 `arch/x86_64/mod.rs:108-110`, `apic.rs:62,271,363-366`; aarch64 `arch/aarch64/mod.rs:325-328` |
| `crate::video::Screen` — `width/height/put_pixel/flush/fill_screen` | same type, same methods | **YES** — arch-neutral. The demo is "call-never-edit" on the shared surface and the single-core `run()` already ships on x86 through the identical path. | `rast_demo.rs:81-186`; `main.rs:1568-1572` (the live x86 call site) |
| `serial_println!`, `core::sync::atomic::*`, `alloc::vec` | identical | **YES** — arch-neutral. | — |

**No load-bearing call is missing an x86 equivalent.** Milestone 1's STOP condition is
not tripped.

## 3. The `online_cpu_count` core-0 subtlety — why it lands correct, and why that is fragile

`run_mc` reads `online_cpu_count()` as *"how many SECONDARY cores can I parallelize
onto"*: it returns early on `online == 0` (`rast_demo.rs:429-436`), and it compares that
number against a roster counted over `1..MC_MAX` only (`rast_demo.rs:474`). On aarch64
that reading is correct because `ONLINE_MASK[0]` is false — `run_capstone_boot_core`
never calls `mark_online` (`arch/aarch64/sched.rs:3169`).

On x86 the mask is set at the top of `run()`, "the one function every core — APs via
`wait_and_run`, the BSP via `run_bsp` — must pass through" (`arch/x86_64/sched.rs:3018`).
So core 0 *would* be counted — **except on a `rast` build, where the BSP never reaches
`run_bsp` at all** (§5). The count therefore means "dispatching secondaries" on exactly
the build this port targets, matching aarch64.

That is a correct-by-coincidence, so the x86 twin carries the reasoning in its doc
comment rather than leaving it to be rediscovered. **Only metal/another build shape can
break it**: a future x86 build that arms `rast` *and* reaches `run_bsp` would count core
0 and inflate the "N secondary core(s) online" witness by one. The consequence is bounded
and non-fatal (the enlist loop's `alive >= online` never trips, so it always waits the
full `MC_ENLIST_MS = 300 ms` before closing the roster — a slower start, not a wrong
result), but the witness line would lie by one. Stated here so the next seat sees it.

## 4. `ms()` — the one semantic difference, and why it does not bite

Every bounded wait in `run_mc` is `ms()`-deadlined (`MC_ENLIST_MS`, `MC_DRAIN_MS`) and
every fps/speedup number is an `ms()` difference. On x86 `ms()` advances **only when the
BSP's APIC timer ISR fires**, so a presenter spinning with interrupts masked would freeze
the clock — which would not hang the demo (`MC_SPIN_CAP` is the finite backstop) but
*would* fabricate a witness: `base_ms`/`mc_ms` both clamp to `.max(1)` and the arithmetic
would print a confident "1.000x speedup" at "90000.000 fps".

It does not bite, and the evidence is already on the record: the shipped x86 RAST-1
witness reads **`90 frames in 4115 ms — 21.871 fps`**
(`docs/dev/OS/08_VIDEO/rasterizer.md:318-320`). 4115 ms is a real measurement over the
same inline BSP path `run_mc` will run on, so the ISR demonstrably fires there and the
clock demonstrably advances. The QEMU run in §6 re-establishes this for the MC path
specifically.

## 4b. The Screen API is signature-identical and contract-different under `wc`

The brief asked specifically whether `Screen` behaves identically when the compositor owns
the panel. **It does not**, and this is the most consequential finding of the audit.

`Screen`'s five verbs carry no `cfg` at all and are arch-neutral in source
(`video/screen.rs:851,967,972,991,1012,1233`). `put_pixel` writes the **back buffer only**
and marks damage; it never touches the panel. The panel write happens in
`present_background`, and *that* is arch-conditional (`screen.rs:1466` vs `:1498`):

- **aarch64/tegra** takes the `wm::occluders`-only arm, and on the Orin demo path the
  window table is empty — so the demo's pixels reach the panel. (`tegra` does not imply
  `desktop_firmware`, `Cargo.toml:696,1706`.)
- **x86 + `wc`** takes the windows-**plus-furniture** arm. And the subtraction happens
  *before* any compositor pass: for each damaged row, `next_visible_span`
  (`screen.rs:783`, called `:1693`) copies back-buffer bytes to the framebuffer **only in
  the gaps between occluder boxes** (`screen.rs:1694-1712`). A pixel under an occluder is
  not composited over — **it is never written at all**, and its damage is consumed and
  cleared anyway (`screen.rs:1395`). `flush()` returns as if it had presented.

On an x86 `wc` boot that reached `desktop_uefi::activate()`, the occluder at panel centre
is the **console window** — minted at `desktop_uefi.rs:523`, sized ~7/8 × 7/8 of the work
area and **centred** (`fbcon.rs:1828-1829,1936-1941`), and always admitted to `occluders`
because `above_shell` passes kernel-owner rows unconditionally (`wm.rs:3040,3150`). Plus
the menu bar (`desktop_uefi.rs:552`) and dock (`strip.rs:647`).

`rast_demo` is exactly the shape that loses: a **centred** `DEMO_W`×`DEMO_H` block of
`put_pixel` then one `flush` (`rast_demo.rs:90-91,151-161`). **Its entire output is
swallowed by the console window.** Nothing appears on glass.

**Why this does not refute the port.** RAST-MC's deliverable is a *speedup ratio* against
a same-boot 1-core baseline. Both arms pay the identical present cost whether or not the
bytes land, so the measurement and every serial witness remain honest. What the port must
not claim — and does not — is "3D pixels on x86 under the compositor".

**The trap, and it is a `QEMU-green ≠ correct` trap of the first kind.**
`desktop_uefi::activate()` has exactly one call site: `kepler_display.rs:486`, inside the
Kepler takeover. QEMU has no Kepler, so on QEMU `activate()` never runs, `is_active()`
stays false, the occluder set is empty, and **the cube renders exactly as intended**. The
occluded path exists only on the bench rMBP with a real Kepler. A QEMU-green run is
therefore not evidence about the panel on metal — see §8.

Making the demo visible on x86 under `wc` means rendering into a compositor **window**
rather than poking panel coordinates. That is a design change to a module whose whole
contract is "call-never-edit" on the shared video path — **out of this arc's lane, and
deliberately not attempted.**

## 5. The structural finding the triage missed — `rast` and the x86 scheduler handoff are mutually exclusive

```
main.rs:1468   #[cfg(all(target_arch = "x86_64", not(feature = "rast")))]
main.rs:1470       let online = unaos_kernel::arch::smp::online_aps();
main.rs:1484       if let Some((render_cpu, svc_cpu)) = split {
main.rs:1520-1546      spawn("usb-pump"/"input"/"render", …, PRIO_NORMAL)
main.rs:1549           unaos_kernel::arch::sched::run_bsp(0);   // -> !  diverges
main.rs:1555       }
main.rs:1568   #[cfg(all(feature = "rast", not(feature = "pi"), not(feature = "tegra")))]
main.rs:1571       unaos_kernel::rast_demo::run(&mut screen);
```

The `not(feature = "rast")` on line 1468 is load-bearing in a way nothing documented:
**on any x86 build with `rast` armed, the whole SCHED-X86 render/input/usb-pump handoff
is compiled out**, the BSP never enters `run_bsp`, and the GUI runs inline on core 0.
Without that gate the rast call site would be dead code on every multi-AP boot, because
`run_bsp` diverges above it.

This answers the brief's scheduling question, but not in the shape it was asked:

- **There is no c1 render pin and no device-service core to starve on an x86 `rast`
  build.** Those three tasks (`main.rs:1520,1528,1544`, all `PRIO_NORMAL`) do not exist
  in this build. The starvation question is therefore vacuous here — and any claim that
  RAST-MC coexists with the x86 compositor's scheduled render lane would be **unfalsifiable
  in this build shape**, so this port does not make it.
- **The APs are nevertheless live and idle, which is exactly what RAST-MC wants.**
  `sched::enable()` is called **unconditionally** on x86 (`main.rs:833`, the PULSE-NCPU
  fix), releasing every AP from `wait_and_run` into `run()` — which calls `mark_online`
  (`arch/x86_64/sched.rs:5924`) and then idles in `sti;hlt`. So a pinned
  `spawn(…, cpu)` from the inline BSP reaches a real dispatching core, and the presenter
  (the BSP) is not competing with a render-lane peer. This is the *best* shape RAST-MC
  can be given; the Orin gets a strictly busier one.
- **Note for the ledger:** the `x86-all` check leg (`arroyo:2548`) carries **both** `rast`
  and `wc`, so `x86-all` is a configuration in which the SCHED-X86 split is compiled out.
  Pre-existing, not introduced here, but worth someone's attention.

**Priority choice.** Workers are spawned at `PRIO_NORMAL` — the same band the SCHED-X86
split gives its own render/input/usb-pump tasks (`main.rs:1524,1532,1544`). Not
`PRIO_HIGH`/`PRIO_RT`: with `sched_demo` armed (as in `x86-all`) the APs also carry
`start_demo` workloads (`main.rs:979`, `arch/x86_64/sched.rs:6598`), and a compute-bound
90-frame render loop in an elevated band would starve them for no panel benefit.

## 6. Interaction with the REDSORT lane (not folded, not duplicated)

REDSORT is fixing a `pi`-leaks-into-x86 defect against `main.rs:1568`'s
`not(feature = "pi")` term. Mechanism confirmed here: `arroyo` accumulates **one shared**
`_feats` string for both arches — `UNAOS_PI=1` appends `pi,` at `arroyo:201` and
`UNAOS_RAST=1` appends `rast,` at `arroyo:257` — so an operator running an x86 target with
both set gets `pi` in the x86 feature list, and `not(feature = "pi")` silently deletes the
rast call site from x86. (`x86-all` itself carries neither `pi` nor `tegra`, so the check
legs are unaffected.)

**This port's dependence on that fix: none for correctness, identical exposure in
practice.** The new x86 MC call site sits on the same source line, inside the same
`#[cfg(all(feature = "rast", not(feature = "pi"), not(feature = "tegra")))]` block, so it
inherits exactly the same `UNAOS_PI=1`-on-x86 disappearance and no more. Deliberately
*not* given a different gate: diverging from the sibling line would leave REDSORT's fix
covering one of the two call sites. When REDSORT lands, its fix covers both with no
rework here.

## 7. What the port actually changes

| change | file | why |
|---|---|---|
| `pub fn online_cpu_count() -> usize` | `arch/x86_64/sched.rs` | signature-matched twin of `arch/aarch64/sched.rs:4070`, placed beside `mark_online`/`cpu_dispatching` where the mask it reads is defined |
| `pub const fn sched_cpu_slots() -> usize` | `arch/x86_64/sched.rs` + `arch/aarch64/sched.rs` | the neutral accessor the brief asked for, instead of renaming `NUM_CPUS` or `MAX_CPUS`; `const fn` so it still works as an array length |
| `mc_spawn(cpu)` per-arch shim | `rast_demo.rs` | confines the `spawn` arity difference to one documented place; neither public `spawn` signature is touched |
| 21 gates re-formed | `rast_demo.rs` | `all(feature="tegra", target_arch="aarch64")` → `any(all(feature="tegra", target_arch="aarch64"), all(feature="rastmc", target_arch="x86_64"))` |
| `rastmc = ["rast"]` feature + `UNAOS_RASTMC` knob | `Cargo.toml`, `arroyo` | the `pirast` precedent (`Cargo.toml:1686`): the module stays `rast`-gated, a separate feature gates only the call site, so the metal-witnessed x86 RAST-1 path is byte-identical with the knob off |
| x86 `run_mc` call, **same source line** as the existing `run` call | `main.rs:1571` | zero added source lines ahead of any `panic::Location`, per the PI-V3D-1 byte-identity convention the tegra call site at `main.rs:6925` follows |

## 8. Gate results

| gate | result |
|---|---|
| `./arroyo check` (baseline, pre-port) | **EXIT=0** — x86_64 OK, aarch64 OK, 36/36 cfg legs |
| `./arroyo check` (post-port) | **EXIT=0** — x86_64 OK, aarch64 OK, 36/36 cfg legs, zero errors. `x86-all` now carries `rastmc`, so the armed polarity of all 21 sites + the x86 call site is type-checked. |
| `UNAOS_WC=1 ./arroyo test` | **EXIT=0**, banner `witness,ehcihid,kbdwit,sdhcblk,smolnet,wc` — `wc` present, `rast*` absent (knob-off path unchanged) |
| `UNAOS_RASTMC=1 UNAOS_WC=1 ./arroyo test 75` | **EXIT=0**, banner carries **both** `rastmc` and `wc`; `strings` on the booted ELF shows `:: RAST-MC:` (reachable, not merely compiled) |

### The armed-boot witnesses, verbatim

```
:: RAST-MC: 5 secondary core(s) online and dispatching — probing for render workers ::
:: RAST-MC: 1-core baseline — 90 frames in 19654 ms — 4.579 fps (same boot, unpaced) ::
:: RAST-MC: pipeline width 5 (render cores [1, 2, 3, 4, 5], present on boot core 0) — 3000 KiB of back/depth buffers off the 48 MiB heap ::
:: RAST-MC: core 1 rendered 18 frame(s) ::
:: RAST-MC: core 2 rendered 18 frame(s) ::
:: RAST-MC: core 3 rendered 18 frame(s) ::
:: RAST-MC: core 4 rendered 18 frame(s) ::
:: RAST-MC: core 5 rendered 18 frame(s) ::
:: RAST-MC: core 0 presented 90 frame(s) (ordered, boot core) ::
:: RAST-MC: 6 core(s), 90 frames, 9.572 fps — speedup 2.090x vs 1-core ::
:: RAST-MC: verdict PASS — 90 frame(s) rendered off the boot core, 90 presented in order (9402 ms vs 19654 ms 1-core) ::
:: RAST: software rasterizer demo — 320x240 spinning cube centered on 1280x800 panel, 90 frames ::
```

Each of the three bridges is independently confirmed by this output:

- **`online_cpu_count` twin** — reports **5**, i.e. the five APs with **core 0 correctly
  excluded**, which is exactly the `run_bsp`-never-reached reasoning of §3 turning up as a
  number rather than an argument.
- **`spawn` pin shim** — `18 = 90/5` frames on **every** one of the five cores, and zero
  on any other. A pin that did not hold would show a skewed distribution or an idle core.
- **`ms()`** — 19654 ms and 9402 ms are real, non-degenerate measurements, so the clock
  advances across the presenter's spins and the §4 failure mode (both arms clamping to
  `.max(1)` and fabricating "1.000x") did not occur.
- **The 2.090x** lands on the module's own a-priori Amdahl prediction — "roughly 2x when
  render and present cost about the same, no matter how many cores are online"
  (`rast_demo.rs`) — with 5 render cores. The rung behaves as designed, not merely as
  compiled.

### A trap this arc walked into, recorded so the next seat does not

The first armed run printed `rastmc` in the feature banner and produced **no RAST witness
at all**. `strings` on the booted ELF had no `RAST-MC` in it, and the serial log showed
`:: SCHED-X86: BSP entered run loop cpu=0 ::` — the handoff that `rast` is supposed to
compile out. Cause: **`arroyo`'s `$KERNEL_FEATURES` is not what the booting x86 kernel is
built from.** The `builder` subprocess rebuilds it from its own env→feature map
(`builder/src/main.rs`, and `arroyo:34-35` says so), so a knob added to `arroyo` alone
banners correctly and is absent from the image. Fixed by adding `UNAOS_RASTMC` to
`builder/src/main.rs:111` as well. This is precisely the "verify reachable (`strings`),
not merely compiled (banner)" rule, and the banner alone would have shipped a green lie.

### What only metal can prove

1. **Whether anything is visible.** QEMU has no Kepler, so `desktop_uefi::activate()` never
   ran on any of these runs and the occluder set was empty — the cube was presented
   unobstructed. On the bench rMBP with `UNAOS_WC=1` and a real takeover, §4b says the
   centred console window swallows the demo region entirely. **These QEMU runs are not
   evidence about the bench panel.** The serial witnesses above will still all appear on
   metal; the glass is the open question.
2. **The speedup number.** Geometry: QEMU's panel is 1280x800, the bench rMBP is
   2880x1800. `run_mc` renders at a fixed 320x240 and centres the blit
   (`rast_demo.rs:34-35,422-423`), so panel geometry changes only `off_x/off_y` and the
   per-frame present cost — never the rendered pixels. A 2880x1800 panel makes the
   **serial** present half more expensive, which by the module's own Amdahl note *lowers*
   the achievable speedup. **2.090x is a QEMU number and should not be quoted as the
   bench's.**
3. **Core count.** QEMU gave 5 APs. The bench rMBP's real topology decides the pipeline
   width there, and `MC_MAX` caps it at `gdt::MAX_CPUS` = 8.
