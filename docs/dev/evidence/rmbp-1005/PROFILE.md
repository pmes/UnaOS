# PROFILE — a sampling profiler in the kernel (rmbp-ledger B331)

Branch `exec-rmbp-profile`, cut from `4e48ab03` (merge11 tip). Peter, 2026-10-04 (R83): "do we have
some kind of profiling so we can see where things are congesting?" — the answer was no.

## Finding

The kernel has stage timings (`bootpace.rs`, the `:: BOOT:` line), the compositor's `[wc-h]` and
`[comp2]` rollups, TSC cycle counts inside single GPU rungs and a flight recorder (`flight_recorder.rs`).
Every one of them answers a question somebody thought to ask in advance, at one site. None says WHERE
the machine spends its time when it is slow. That is a sampler's job: interrupt the machine at a fixed
rate, write down what it was doing, count.

## The seam

`CHARTER: Kernel — kernel-by-ruling` (R83: built in UnaOS). A profiler of Ring 0 lives in Ring 0; the
only things it borrows are the timer interrupt both arches already take and the syscall dispatcher's
existing return path. Symbolization is NOT in the kernel: the build writes the symbol table beside the
kernel image (`target/kernel.syms`, one arroyo line) and a host script (`tools/flame`) joins it to the
serial dump. The kernel embeds nothing. A ring-3 counterpart (`SYS_PROF`) is owed.

**No new knob.** The sampler is compiled into every image and armed at runtime by `prof start`, because
a profiler that needs a rebuild cannot be pointed at the boot that is slow. Disarmed cost: one relaxed
load and a branch per timer tick and per syscall. The sample rings (4096 samples per CPU, 16 bytes each,
eight CPU slots) are heap-allocated by the FIRST `prof start` and never freed (re-used by every later
run), so an image that is never profiled carries no buffer, and the ISR never allocates. Off by default;
nothing prints at boot (R80).

## Milestones

* **M1 SAMPLER.** `prof::on_tick_x86(rip, ring3)` folded onto the x86 timer ISR's `note_tick()` line;
  `prof::on_tick_arm()` folded onto aarch64 `timer::on_tick`'s tail line (reads `ELR_ELx`/`SPSR_ELx`
  of the interrupted context at the current EL). When armed, every `div`-th tick records (RIP, task id,
  ring, cpu) into this CPU's ring; full ring -> `dropped` counter, never overwrite. Single writer per
  ring (the owning CPU, IF=0); readers see the published `len` (Release/Acquire). Verbs:
  `prof start [hz]` (clamped to the tick rate: 1000 x86, 250 aarch64; default = tick rate),
  `prof stop`, `prof` / `prof status`. HOST_VERBS row + dispatch arm + help row.
* **M2 SYMBOLS.** arroyo: `emit_kernel_syms <elf> <out>` = `llvm-nm -n -C --defined-only <elf> > <out>`
  run on the ELF the link step just wrote (x86 QEMU path and the esp-x86 staged `kernel.elf` ->
  `target/kernel.syms`; aarch64 -> `target/kernel-aarch64.syms`). The kernel is PIE, so a runtime RIP
  is link address + slide: every `prof` dump prints `anchor=` (the runtime address of the
  `#[no_mangle] unaos_prof_anchor` function), and `tools/flame` computes the slide from that symbol's
  line in the syms file. `prof top [n]` prints the top-N 256-byte RIP buckets with sample % (the kernel
  has no symbol table at runtime); `prof dump` prints every sample as
  `[prof] cpu= task= ring= rip=`. `tools/flame <log> [--syms target/kernel.syms] [--top n] [--svg out]`
  prints the top-N symbols and writes a depth-1 (histogram) flame graph SVG; `--self-test` runs it on
  a synthetic log + syms pair.
* **M3 SYSCALL + TASK VIEWS.** per-syscall latency histograms, 64 syscall numbers x 24 log2 buckets of
  nanoseconds, armed with the sampler (`prof start` clears them): one relaxed load on entry, one
  `now_cycles` pair and two adds on exit, folded onto `syscall_dispatch`'s `let rc =` line (x86) and
  the aarch64 SVC dispatcher's return. `prof sys` prints count / mean / p50 / p99 / max bucket per
  syscall. `prof tasks` prints per-task CPU share from the samples (task 0 = idle / no task).
* **M4 COMPOSITOR + FIXTURE.** `prof top` closes with three rows `wc:present`, `wc:compose`,
  `wc:blit` — the `[comp2]` split's per-pass means (`witness` images), read (never drained) from
  the counters `comp2_emit` already keeps, as a delta over the profiling window. `tests prof` arms
  the sampler for 1 s while this CPU runs a synthetic load, then asserts samples > 0 and dropped == 0.

## Witness

`:: PROFILE: hz=<n> samples=<n> dropped=0 top=<bucket> -> PASS ::` (`tests prof`, never at boot).

## Owed

Real stacks (frame-pointer walk; depth 1 today); task NAMES in `prof tasks` (ids only: there is no
safe cross-core tid->name lookup); ring-3 symbolization (ring-3 samples are counted, their RIPs are user
addresses `tools/flame` reports as `[ring3]`); `SYS_PROF` for ring 3; linuxabi syscalls are not timed
(they return before the hook).
