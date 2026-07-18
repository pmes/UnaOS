# SCHED-POLISH landing — two §4 aging/condvar refinements

**Branch:** `hw-rmbp` (off main `0e8cee1`, the KERNEL-CLOCK merge). **Lane:**
`arch/x86_64/sched.rs` + this landing report. aarch64 untouched. Two queued
scheduler-brief §4 candidates, each its own milestone/commit.

## M1 — `effective_level` aging refinement

### Why

Priority aging promotes a long-waiting task one level per `AGE_TICKS`, but the old
design RE-BASED a task to its base priority on every dispatch (`RunQueue::push`
zeroed the aging clock and placed at `priority`). Under bursty mixed load — where a
level between base and the contended level momentarily drains, so the climbing task
gets an *intermediate* dispatch before reaching parity — that dispatch erased the
whole climb, and the task had to re-climb from base. The starvation bound, exact at
`~AGE_TICKS` per level when no intermediate level drains, became finite-but-larger.

### What landed

- **`Task.effective_level: u8`** — a transient level the task currently occupies
  (`priority..NUM_PRIORITIES`). Same discipline as `wait_ticks`: owning-CPU-only,
  lock-protected, NEVER read cross-CPU. The immutable base `priority` stays the
  lock-free read for `poke_for`/`make_ready`/the dispatch publish (invariant kept).
- **Three run-queue placement ops, now distinct:**
  - `RunQueue::push` — FRESH enqueue (spawn / wake): re-bases to `priority`, zeroes
    the clock, `effective_level = base`. Strict priority preserved for new work.
  - `RunQueue::age` — RELOCATE up one level on promotion (raw `VecDeque` move,
    HIGH→LOW, base untouched); now also sets `effective_level` to the destination.
  - `RunQueue::requeue` — NEW. RE-ENQUEUE after a dispatch (yield / preempt):
    DECAYS `effective_level` by one toward base (clamped `>= priority`) instead of
    resetting. `run()`'s post-preempt re-enqueue calls this instead of `push`.
- **Effect:** a task dispatched mid-climb re-climbs at most ONE level, so the bound
  holds at `~AGE_TICKS` per level even when intermediate levels drain. Absent
  contention the level decays back to base within a few dispatches (strict priority
  restored). The invariant `a task in levels[L] has effective_level == L` is
  maintained by every op and guarded by a `debug_assert` in `age`.

### Witness — `AGEREF:`

One PRIO_LOW victim runs the same tiny workload twice under continuous HIGH-priority
load on one AP, differing only in the re-enqueue path exercised:
- **CONTROL** phase: `sleep_ticks(0)` between iters → block + wake → `push` → base
  reset (the OLD behavior).
- **REFINED** phase: `yield_now()` between iters → `requeue` → one-level decay (the
  refinement).

Both phases run back-to-back under identical load, so absolute QEMU timing jitter
cancels in the ratio (self-calibrating). PASS = victim finished AND
`refined <= 75% * ctl`. The 75% gate sits well below 100% (the refinement roughly
halves the re-climb: ~2 levels → ~1) yet clear of the couple-tick block-vs-yield
overhead that makes CONTROL trivially slower even absent any refinement. A verifier
watchdogs it (12 000 ticks): if aging broke entirely, the LOW victim starves forever
and never signals done → FAIL. **Measured: `refined=292 ctl=480` (61%) both regimes.**

## M2 — `Condvar::init_with_capacity(n)` → a >32-reader `RwLock`

### Why

`WAIT_CAPACITY = 32` was a single const feeding every waiter list. An `RwLock`'s
reader queue is unbounded by design, so a lock needing >32 simultaneously-blocked
readers hit the `Condvar::wait` alloc-free-park assert and panicked.

### What landed

- **`Condvar.capacity: AtomicUsize`** (default `WAIT_CAPACITY`, set once at init,
  read Relaxed). `wait()`'s assert now tracks this per-instance value instead of the
  global const. `Condvar::new()` + `init()` reserve exactly 32 as before — behaviour
  byte-identical at every existing call site.
- **`Condvar::init_with_capacity(n)`** — reserves `n` waiter slots and records `n`
  as the ceiling. `init()` now delegates to it with `WAIT_CAPACITY`.
- **`RwLock::init_with_reader_capacity(readers)`** — threads the capacity to the
  READER condvar only; the writer condvar and inner mutex keep the default (they are
  bounded by writer population / transient CPU-pinned contention). `RwLock::new` +
  `init` unchanged.

### Witness — `CVCAP:`

A writer takes the write lock, publishes `CV_WRITER_READY`, and holds it (sleeping,
never hogging a core) until all `CV_READERS = 40` readers have piled up blocked on
the reader condvar — only possible because `RWL2` reserved its reader queue at 48 via
`init_with_reader_capacity`. Releasing wakes them all (`notify_all`). PASS = all 40
finished AND no torn read AND the blocked-reader high-water exceeded 32 (proving the
raised reservation was genuinely exercised). Verifier watchdog 12 000 ticks.
**Measured: `done 40/40, torn=false, max_blocked_readers=40` both regimes.**

## Gate results (verbatim)

- `./arroyo check` — `✅ x86_64 OK` / `✅ aarch64 OK`.
- `./arroyo test 22` (x2APIC) — `xHCI: >>> MISSION SUCCESS (BOT + CSW)...`.
- `UNAOS_CPU=qemu64 ./arroyo test 22` (xAPIC) — `MISSION SUCCESS`.
- `UNAOS_SCHED_DEMO=1 ./arroyo test 30` (x2APIC):
  - `AGEREF: [cpu3] refined=292 ctl=480 ticks (refined<=75% ctl), done=true => PASS`
  - `CVCAP: [cpu3] done 40/40, torn=false, max_blocked_readers=40 (cap 48, need >32) => PASS`
  - `SLEEPMS: ... => PASS` · `JOINTMO: ... => PASS` · `RWLOCK: ... => PASS` (unregressed).
- `UNAOS_CPU=qemu64 UNAOS_SCHED_DEMO=1 ./arroyo test 30` (xAPIC) — same five PASS.
- `./arroyo test-arm 22` — `MISSION SUCCESS` (aarch64 unregressed).

## Notes / flags

- **No §2 invariant touched.** `effective_level` mirrors `wait_ticks`'s
  owning-CPU-only lock-protected discipline; base `priority` remains the immutable
  cross-CPU lock-free read. Capacity is a per-condvar reservation set before any task
  can block; the lock-handoff / alloc-free-park proof is preserved (the assert simply
  tracks the real reservation).
- **AGEREF AP placement:** pinned to a NON-clock AP (the middle AP when ≥3 exist,
  else the busy AP) so its HIGH load never perturbs the SLEEPMS/JOINTMO wall-clock
  witnesses. With ≥3 APs it shares the RwLock AP (blocked-heavy tasks; delaying them
  is watchdog-safe). All three existing witnesses verified unregressed.
- **CVCAP inner-mutex safety:** 40 readers never overflow the inner mutex's default
  32-waiter reservation — CPU-pinning serialises per-core contenders, so at most
  ~one-per-CPU parks on the inner mutex at once; all 40 pile on the reader condvar.
