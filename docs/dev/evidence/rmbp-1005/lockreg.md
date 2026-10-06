# LOCKREG (rmbp-ledger B414) — one kernel lock type, a holder registry, and what it says on the wire

## Finding (the wire, then the code)
- f25-boots.log: `[play] dec spawn path=/system/test-f/TEST.FLAC jid=1 stack=65536 cpu=0` is the last kernel line until
  the seat's next mark (flight 24 card 3: the same); the shell never returns, the GUI lives. STACKGUARD2 (B403) named the
  FTDI capture ring (`ftdi.rs` RING) as the lock a dying holder takes the wire with, and could release only what it
  NAMED (`lockowner.rs`: sink, uart, unafs). CLOCKCORE (B397) names masked sections; a core SPINNING on a lock whose
  holder was preempted is not masked, so nothing names it. PANICSCREEN (B406) cannot see a VFS/UnaFS lock the dying
  task holds (the log write then hangs or skips).
- The code: the kernel's spin locks are `spin::Mutex` (spin 0.12) at 125 files, 263 `static` locks and ~300
  construction sites; NO `spin::RwLock` (the scheduler's `sched::RwLock`/`sched::Mutex` are blocking primitives, not
  in scope). Hot ones: the global heap (`allocator.rs` `Locked<Heap>`), the FTDI ring (`drivers/xhci/ftdi.rs` RING),
  the 16550 (`arch/x86_64/serial.rs` SERIAL1), the UnaFS mount (`fs/unafs.rs` MOUNT), the WM's tables and present
  shadows (`video/wm.rs`), fbcon's console, the xHCI rings. No registry exists (`arch::sync` absent on this tree).

## Seam
Kernel — kernel-by-ruling (no handler's domain: the kernel's own locks). New arch-neutral `src/sync.rs`
(`//! CHARTER: Kernel — kernel-by-ruling`); tracking and the scan are x86_64 only (aarch64: the plain wrapper, no
stores). `lockowner.rs` stays the seed for the three locks whose release needs special handling (the UnaFS mount
is LEAKED, not dropped); the registry releases everything else.

## Shape
- `sync::Mutex<T>` = `spin::Mutex<()>` raw lock + `UnsafeCell<T>` + the construction site (`#[track_caller] const fn
  new`: the lock's NAME and its order class). Same API as spin's. The raw lock is releasable by address without `T`.
- Holder registry: per-CPU partition of 32 entries (an occupancy bitmask claimed by CAS: ISR/NMI-safe, no heap). On
  acquire: one CAS + four stores (tag = tid/cpu/class, lock address, acquire site, t0 TSC); on drop: two stores.
  The ledger's "two stores" is not reachable with a releasable-by-address entry; the count is said, not hidden.
- Spin-wait: the slow path (first `try_lock` failed) publishes a per-CPU waiter record (lock, site, t0, tid).
- Scan: CLOCKCORE's 50 ms per-core tick (one core, `isr_tick`) reads every partition: held > 50 ms →
  `[lock] held-too-long name=<site> by=<tid>@cpu<n> ms=<n> waiters=<n>`; a waiter > 10 ms →
  `[lock] spin-wait name=<site> waiter=<tid>@cpu<n> on=<tid>@cpu<n>|unknown ms=<n>`; a pending inversion →
  `[lock] order-inversion a=<site> b=<site> by=<tid>@cpu<n>`. Once per site each, 16 lines total at most (QUIETBOOT).
  All printing is from the scan via `serial_println!` (try_lock sink only) — never from inside `lock()`.
- Order: classes = construction sites (1024-entry table); on acquire of class B while the same task holds class A on
  this core, edge A→B is recorded (4096-entry open-addressed set); seeing B→A already recorded marks a pending
  inversion. Same-class nesting is skipped.
- Death: `sync::release_task(tid, why)` force-unlocks every registry entry whose holder is `tid` and prints
  `[lock] task=<tid> released=[<every site>] (<why>)`. Callers: STACKGUARD's `overflow_exit` (after the named
  three), the panic log path (PANICSCREEN `write_log`, before its heap wait). DECJOB's abort NAMES, never releases,
  the live decoder's holds and wait (`[lock] task=<tid> holds=[…] waits=<site>|none (dec-abort)`): that task is
  wedged, not dead; force-unlocking under a live holder corrupts.

## Milestones
- M1 `sync::Mutex` + the mechanical sweep (type substitutions only, one commit).
- M2 the registry: holder entries, waiter records, order edges, the 50 ms scan (hook: `interrupts.rs` timer ISR, after
  `clockcore::isr_tick`), armed by `stackguard::boot_line` with `[lock] registry armed classes=<n> cap=<n> (LOCKREG)`;
  `tests lockreg` (typed, R80): a scratch task holds a scratch lock 60 ms and the scan must have seen it; a scratch task
  ends holding a lock and `release_task` must give it back;
  `:: LOCKREG: locks=<n> wrapped=<tracked>/<acquires> held_max_ms=<n> inversions=<n> released_on_death=ok named=ok -> PASS ::`.
- M3 death: STACKGUARD halt (`release_task` after the named three), the panic log path (the named three + every
  other, before the heap wait), DECJOB's abort NAMES (`name_task`).

## Witness (metal)
```
[lock] registry armed classes=<n> cap=256 (LOCKREG)
[lock] held-too-long name=<file>:<line> by=<tid>@cpu<n> ms=<n> waiters=<n>         (live holds over 50 ms)
[lock] spin-wait name=<file>:<line> waiter=<tid>@cpu<n> on=<tid>@cpu<n> ms=<n>      (the preempted-holder case)
[lock] order-inversion a=<file>:<line> b=<file>:<line> by=<tid>@cpu<n>
:: LOCKREG: locks=<n> wrapped=<n>/<n> held_max_ms=<n> inversions=0 released_on_death=ok -> PASS ::
```
The FLAC wedge, if it recurs: `[play] … -> ABORT` followed by `[lock] task=<tid> holds=[…] waits=<site>` names the
lock the decoder sits on (or `waits=none`: not a lock — then it is the decoder's own loop).

## Owed
- aarch64: wrapper only (no registry, no scan).
- Order classes are construction sites: two instances of one class nested are not checked against each other.
- A preempted holder migrated to another core keeps its entry on the old core's partition (the order check of its next
  acquire misses those edges; held/release still see it).
- `lazy_static`'s internal spin lock and `spin::Once` are not swept (not holder locks).
