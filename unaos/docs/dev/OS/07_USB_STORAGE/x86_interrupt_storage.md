# STOR-D1 — IF-safe interrupt-driven x86 storage (design)

Status: **S1–S3 LANDED** (2026-07-10, `hw-rmbp`, behind the `irqstorage` knob — QEMU-green,
metal-pending; commits `7b2f05f`/`b73ba08`/`fd5de85`). S1 the storage service task +
`BlockRequest` submit/block/complete; S2 live in-place reads (`sys_read`); S3 live in-place
write-through (`sys_write_file`) closing the close-discards-dirty residual. **S4 (synchronous
grow/create/delete) is DEFERRED** to a tightly-scoped follow-on (decision 4 below), since its
cross-process races are metal-only (risk 3) and it rewrites the most load-bearing lifecycle code.
The original design (below) is unchanged. Track: `hw-rmbp` (x86_64, 2012 rMBP). Twin reference:
the aarch64 polled storage path, which this design brings x86 into semantic parity with
**without** copying its mechanism.

> Placement note: the STOR-D1 brief names `docs/dev/OS/07_STORAGE/`. The repo's storage doc home
> is `docs/dev/OS/07_USB_STORAGE/` (alongside `usb_xhci.md`), so this doc lives there. Flagged for
> the seat in case a rename is wanted.

---

## 1. Why this arc exists — the staged-buffer divergence

Every x86 storage syscall today runs in a context that **cannot touch the disk**. The SYSCALL
handler is entered IRQ-masked: `SFMASK` clears IF on `syscall` entry
(`arch/x86_64/syscall.rs` header, "SFMASK: RFLAGS bits cleared on syscall entry — IF | TF | DF | AC"),
and the only path from the kernel to a USB mass-storage sector is the xHCI **Bulk-Only Transport
(BOT)** pump, which `hlt`s awaiting an asynchronous transfer-completion event
(`XhciController::pump_until_bot_done`, `drivers/xhci/mod.rs:3239` — the loop at `:3245` calls
`crate::hlt()` at `:3258`). A `hlt` under a cleared IF never wakes. So an in-handler disk read or
write would hang the core.

The whole x86 capability chain (U6bx → U9x → U10x → U11x → U6gx) is built around that constraint by
**staging in memory inside the handler and deferring the disk half to an IF=1 context**:

| Mechanism | What it stages / defers | Code site | Ledger residual |
|---|---|---|---|
| **U6bx** staged read | `SYS_OPEN`/`SYS_READ` serve bytes from a **BSP-pre-read set** (HELLO.BIN etc.), not the live volume | `SYS_OPEN=11`/`SYS_READ=12`, `syscall.rs` head-notes ~`:61` | "staged-set constraint retires when an IF-safe storage path lands" (SECURITY.md U6bx) |
| **U9x** staged write | `sys_write_file` overwrites a per-descriptor in-memory **wstage** buffer; disk write-back deferred to a **flush queue that survives teardown** | `sys_write_file` `syscall.rs:2545`; `flush_enqueue` `:4281`, `flush_drain_one` `:4309` | "STAGED … the IF-masked handler cannot drive the `hlt()`-ing xHCI write pump" (SECURITY.md U9x) |
| **U10x** deferred grow/create/delete | every FAT mutation is enqueued as a self-contained op COPY (name-id + kind + bytes) and replayed by a launcher `fat.rs` call at IF=1 | `U10_*` op-queue `syscall.rs:4335`+, `u10_flush_drain_one` `:4409`, `u10_drain_grow` `:4449` | "deferred DELETE is a launcher-side replay (weaker causal fidelity)" (SECURITY.md U10x) |
| **U11x M1** gen-tagged file-ids | `SYS_CLOSE` **discards** any un-flushed dirty write (only whole-task teardown enqueues a flush) | `file_desc_validate`, `files_free`, SECURITY.md U11x | "`files_free` DISCARDS un-flushed dirty bytes" |
| **U11x M2** unlink-defers-free | cross-process sibling open **snapshot-copies** the source wstage (`created_desc_any_row`); global refcount defers the DELETE op | `OPENF_REFS`/`OPENF_PENDING`/`OPENF_HELDSLOT` `syscall.rs:3950`+, `openf_decref` `:3973` | residuals (3) torn-copy / cross-file disclosure, (4) open-vs-unlink TOCTOU, (5) close-discards-dirty (SECURITY.md U11x M2) |
| **U6gx** owner/grants | inherits all of the above; ACL is in-kernel volatile | `OWNED_FILES` `syscall.rs:4052` | "The U11x M2 residuals it inherits … are unchanged — the product fix is the same IF-safe interrupt-driven-storage arc" (SECURITY.md U6gx) |
| **F2** FAT-mutation lock | `with_fat_lock` is a **zero-cost passthrough on x86** — masking IRQs across the `hlt`-wait would hang | `fat.rs:334-340` (x86 arm); rationale `fat.rs:299-308` | x86 excluded from the SMP FAT lock |

The launcher (BSP main loop, IF=1) is the one context that drains these queues:
`main.rs:660` loop calls `xhci.poll_events()` (`:662`), `xhci.service_storage()` (`:663`), and the
`*_probe_once` launchers (`:677`+) that call `flush_drain_one` / `u10_flush_drain_one`. Ring-3
fixtures run as scheduled tasks on APs; the BSP is deliberately **not** scheduled — it stays the
polled service loop (`sched.rs` header, "the BSP is deliberately *not* scheduled").

**The through-line of every ledger residual is the same fact:** an acknowledged write is not on disk
until a *separate* IF=1 drain replays it, and cross-process reads copy a *snapshot* of volatile
staging rather than reading shared backing. Both disappear the moment a syscall can **block on a real
transfer completion and read/write the live volume in place** — which is exactly what the aarch64
side already does (its EMMC2 path is polled PIO, so its SVC handler reads the FAT in-line;
`fat.rs:304-306`). This arc gives x86 the same *semantics* via a different *mechanism*: interrupt
completion + a sleeping syscall, since x86 storage is async DMA, not synchronous PIO.

---

## 2. What already exists that we build on

The pieces for an interrupt-driven block layer are **already in the tree**; the arc wires them
together, it does not invent primitives.

1. **A live MSI-X interrupt path.** The xHCI interrupter 0 fires `XHCI_MSI_VECTOR = 0x40`
   (`interrupts.rs:39`, IDT install `:98`); `xhci_msi_handler` (`interrupts.rs:459`) acks the
   controller lock-free (`interrupt_ack`, `xhci/mod.rs:380` — clears IMAN.IP + USBSTS.EINT via raw
   MMIO, takes **no** locks, does **no** allocation) and EOIs. Its stated job today is only "wake the
   CPU from `hlt` so the pump promptly drains the resulting event" (`interrupts.rs:456`). `XHCI_IRQ_COUNT`
   (`xhci/mod.rs:372`) already confirms the path is live.

2. **A real block-on/wake scheduler.** `sched.rs` has `Semaphore` (`:961`, `wait` `:1010` / `post`
   `:1068`) and `Condvar` (`wait` `:1249` / `notify_one` `:1326` / `notify_all` `:1346`) with a
   documented lost-wakeup-safe protocol and a `STATE_BLOCKED` park mechanism
   (`STATE_BLOCKED` `:152`; `PARK_WAITQ`/`PARK_SLEEP` `:158`; `park_blocked` `:1820`). Blocking is
   **IF-aware**: `Semaphore::wait`/`Condvar::wait` snapshot the caller's IF, switch away with IF=0,
   and restore the caller's IF on resume (`sched.rs:1250-1252`, `:1316-1318`). Tasks are **CPU-pinned**
   (`make_ready` returns a task to `task.cpu`, `:841`), so GS/kernel-stack stay correct across a block.

3. **A wall-clock-deadline pump loop** that already tolerates both the timer-live and timer-off
   regimes (`pump_until_bot_done` uses `now_cycles()`/`hw_wait_budget()`, `mod.rs:3240-3244`;
   `now_cycles` = rdtsc, `arch/x86_64/mod.rs:70`). This becomes the storage-service task's body almost
   verbatim.

4. **The event-ring drain is already re-entrancy-safe and single-owner.** `drain_event_ring_once`
   (`mod.rs:1099`) is the one drain used by both `poll_events` and the BOT pump; BOT completions are
   routed to the in-flight transaction (`mod.rs:1353`+). Today the *controller lock*
   (`XHCI_CONTROLLER`) is the single-owner guard, held by the main loop across the pump.

---

## 3. Target architecture

The design introduces **one scheduled kernel task** (the *storage service task*) that owns the BOT
pump, and turns each storage syscall into a **submit → block → resume** sequence. The IRQ handler's
role is unchanged in spirit (wake), so no lock discipline is weakened in interrupt context.

```
  ring-3 task (AP, IF-masked handler)          storage service task (kernel, IF=1)        MSI-X IRQ
  ----------------------------------           -----------------------------------        ---------
  sys_read/write/open/unlink
    build a BlockRequest {lba,buf,dir,          loop:
                          done: Semaphore}        pop a submitted request  (req queue)
    push onto REQ_QUEUE                           issue CBW/data/CSW TRBs + doorbell
    req.done.wait()   <-- BLOCKS (IF restored)    pump_until_bot_done():
      ... task off every run queue ...              drain_event_ring_once()               ack + EOI
                                                    else hlt()  <----- woken from hlt -----'
                                                  decode CSW -> req.result
  <--- resumed, IF restored ---                   req.done.post()  --> make_ready(task)
    return the sector result                    loop
```

### 3.1 The submit/complete handshake

- **`BlockRequest`**: `{ op: Read|Write, lba, len, buf_phys, result: AtomicI32, done: Semaphore }`,
  living on the **submitting task's kernel stack** (it is blocked in `done.wait()`, so the frame is
  pinned and alive for the whole transfer — the same lifetime trick `Condvar::wait` already relies on,
  `sched.rs:1275-1277`). A small fixed `REQ_QUEUE` ring (a `SpinMutex<VecDeque<*mut BlockRequest>>` or
  an MPSC of raw pointers) carries submissions to the service task.
- **Submit** (in the IF-masked syscall handler): validate the user buffer (unchanged from today's
  `sys_read`/`sys_write_file` bounds checks), push the request pointer, ring nothing else, then call
  `req.done.wait()`. `Semaphore::wait` restores the caller's IF snapshot as it switches away, so the
  handler **sleeps with IF=1 semantics** even though it entered IF-masked — this is the crux that makes
  it IF-safe. The core is free to run other tasks / take the storage IRQ while this syscall sleeps.
- **Service** (the storage task, a normal scheduled kernel task at IF=1): pop one request, translate it
  to a BOT transaction (reusing `bot_transfer` `mod.rs:3107` and `pump_until_bot_done` `mod.rs:3239`),
  write the CSW result into `req.result`, and `req.done.post()`. `post` → `make_ready` returns the
  submitter to its pinned CPU's run queue and pokes it (`sched.rs:846-847`).
- **Resume**: the submitter wakes inside `done.wait()`, reads `req.result`, and returns the sector to
  ring 3 — **in place, no staging**.

### 3.2 What happens to the syscall's IF state, reentrancy, and the single-writer row

- **IF state**: the handler enters IF=0 (SFMASK). It never *executes* disk I/O with IF=0; it only
  builds a request and calls `Semaphore::wait`, which snapshots `was_enabled == false` and, on the
  submitter side, restores that on resume (`sched.rs:1316`). So the syscall body still logically runs
  IF-masked; the *waiting* is done by the scheduler with the switch-away invariant "every switch-away is
  IF=0" (`sched.rs:1297`). The task is off every run queue while blocked (`STATE_BLOCKED`), so nothing
  runs the half-finished syscall on another core.
- **Reentrancy while parked**: because the task is CPU-pinned and off the run queue, no second entry into
  the same syscall frame is possible. Other tasks on the same core run normally (this is strictly better
  than today, where the whole core would hang on an in-handler `hlt`). The service task is the **single
  owner** of the pump and the controller lock, so there is exactly one drainer — the self-deadlock the
  IRQ handler avoids today (`mod.rs:365`, `interrupts.rs:455`) is avoided here by construction: the IRQ
  handler still never touches `XHCI_CONTROLLER`; only the service task does.
- **Single-writer row discipline** (load-bearing across U5x→U8x — "a sender NEVER writes the recipient's
  row", SECURITY.md U7x): **unchanged**. Requests carry the submitter's own `(row, descriptor)` identity;
  the service task performs *I/O* on behalf of a request but never writes another task's HANDLES/FILES
  row. Completion delivery is a `Semaphore::post` to the submitter, which wakes *itself* to write its own
  row. This is the same shape as U7x's inbox model and does not add a cross-row writer.

### 3.3 Lock design — what F2 becomes on x86

With a sleeping submitter and a single pump owner, the F2 FAT-mutation lock can become **real on x86**,
mirroring aarch64 (`fat.rs:309-332`). Two lock tiers:

1. **`REQ_QUEUE` lock** — a short `SpinMutex` around a bounded ring, taken IRQ-masked
   (`without_interrupts`) on both the submit side (already IF=0) and the service pop side. Never held
   across I/O. This is the `DEFERRED_FREE`/`IrqGuard` discipline the pi4 reaper already established
   (SECURITY.md U11 M2b coalesce-fix).
2. **`FAT_MUTATION`** — the aarch64 `with_fat_lock` becomes active on x86 too, spanning **only** a
   single FAT-sector RMW (`set_fat_entry`), exactly as documented for aarch64 (`fat.rs:321-324`). The
   reason it was excluded on x86 (`fat.rs:306-308`: "its FAT path `hlt`s … masking IRQs across it would
   hang") **dissolves**: the FAT RMW now runs *inside the storage service task* at IF=1, and each block
   op blocks on a completion rather than an IRQ-masked `hlt`. The service task is the only FAT writer, so
   the span is a couple of bounded block ops with a completion wait — never an unbounded IRQ-masked spin.
   **Decision point (§6):** whether the x86 `with_fat_lock` masks IRQs like aarch64 or relies on the
   single-service-task-writer invariant instead (a lighter lock, since there is exactly one FAT writer).

### 3.4 What replaces the launcher-side flush/op queues

The `FLUSH_*` queue (`syscall.rs:4246`+) and the `U10_*` op-queue (`:4335`+) exist **solely** to move
disk work out of the IF-masked handler into the IF=1 launcher. Once a syscall can block on a real
completion:

- **Writes** go straight through: `sys_write_file` becomes a `BlockRequest` write of the affected
  sector(s), in place — the wstage buffer, `FILE_DIRTY_*`, `flush_enqueue`, `flush_drain_one`, and
  `flush_all_free` all retire.
- **Grow/create/delete** call the same `fat.rs` primitives (`write_grow`, `create_in_root`,
  `delete_located`) the launcher calls today — but **synchronously from the syscall**, under
  `FAT_MUTATION`, so the `U10_*` op-queue, `u10_flush_drain_one`, `U10_HELD`, and the `find_located`
  re-resolution-by-name retire. The deferred DELETE stops being a "launcher-side replay" and becomes an
  in-syscall unlink — closing the U10x causal-fidelity residual outright.
- **Cross-process opens** read shared on-disk (or shared cached) backing instead of snapshot-copying a
  peer's wstage — retiring `created_desc_any_row`'s torn-copy / cross-file-disclosure residual (U11x M2
  residual 3) and the open-vs-unlink TOCTOU (residual 4), because `OPENF_REFS` now guards a real shared
  file, not private volatile buffers.
- The BSP `*_probe_once` launchers keep running the demos, but they no longer *drain queues* — they just
  spawn fixtures and read results.

**What does NOT change:** the aarch64 polled storage path (`fat.rs` aarch64 `with_fat_lock`, the EMMC2
PIO driver, the pi4 reaper) is untouched — this is an x86-lane arc. The capability layer
(`handle_resolve` CHECK, rights sidecars, U5x→U8x transfer/revoke) is untouched — only the *source of
bytes* and the *timing of persistence* change, exactly as U6bx→U9x changed only the source. `SYS_*`
numbers, the object-table kinds, and the ring-3 ABI are unchanged.

---

## 4. Migration sequence — small always-green arcs

Each step is independently `check`-green + QEMU-green and retires a *named* staged mechanism witnessed
by a *named* fixture. Steps are ordered so the block layer lands and is proven **before** any capability
mechanism is rewired onto it. QEMU (usb-storage, TCG) is the CI witness throughout; metal confirmation
rides attended benches at arc boundaries (§5).

| # | Arc | Retires | Witness fixture / gate |
|---|---|---|---|
| **S1** | **Storage service task + `BlockRequest` submit/complete**, running *beside* the current polled path (no syscall rewired yet). A kernel self-test issues a raw sector read/write through the new path. | nothing yet — adds the primitive | new `bx-blockreq` kernel launcher: read a known sector via `BlockRequest`, compare to the polled read; `test-fat sf` unchanged 21 PASS + 1 |
| **S2** | **Route `sys_read` through the service task** (real in-place reads); keep U6bx staged open for *open*. | U6bx staged **read** (`SYS_READ` serves live bytes) | `u6bx-file` read path now reads the live volume; verdict byte-identical |
| **S3** | **Route `sys_write_file` through the service task** (in-place write-through). | U9x `FLUSH_*` queue + `FILE_DIRTY_*` + close-discards-dirty (residual 5) | `u9x-write`: read-back after write with **no launcher drain**; `SYS_CLOSE` of a dirty descriptor now persists |
| **S4** | **Route grow/create/delete synchronously** (in-syscall `fat.rs` calls under `FAT_MUTATION`). | U10x `U10_*` op-queue + launcher replay (causal-fidelity residual) | `u10x-grow`/`u10cx-create`/`u10dx-delete`: on-disk change visible **within the syscall**, no deferred drain |
| **S5** | **Real shared backing for cross-process opens** (read shared on-disk/cached, not snapshot). | U11x M2 residuals 3 (torn-copy/disclosure) + 4 (open-vs-unlink TOCTOU) | `u11m2-unlink` cross-row read now reads shared backing; add a concurrent-writer stress that would torn-copy under the old snapshot |
| **S6** | **Activate `FAT_MUTATION` on x86** (make `with_fat_lock` real). | F2's x86 zero-cost passthrough exclusion | port the aarch64 `f2_witness_rmw` witness to x86 (`fat.rs:369`); LOCKED pass reaches `2*iters`, UNLOCKED control shows loss under contention |
| **S7** | **Retire U6bx staged-open + the pre-stage buffer** (open resolves any on-disk file). | the BSP-staged set constraint (U6bx) | arbitrary-file open fixture: open a file *not* in the pre-stage set, read it |

Steps S1–S4 are the spine and each is a clean commit-sized milestone; S5–S7 are separable follow-ons
that can be their own sessions. A track never carries more than one unmerged arc, so these land in
sequence with seat review between spine milestones.

---

## 5. Risks

1. **Metal xHCI transfer-IRQ behavior on the 2012 rMBP is UNPROVEN.** The MSI-X path is bench-confirmed
   for *enumeration* (the `3bee9d6` fix; the Panther Point xHCI `0x1e31` addressed HS first-try, chain
   U1→U11x metal-green — resume ledger), but **transfer-event-driven I/O has never been exercised on
   that controller** — every storage arc to date SKIPS or stages on metal. If this controller's
   interrupter does not raise reliably on BOT transfer completion (only on port-status change, say), the
   service task falls back to its wall-clock-deadline poll (`pump_until_bot_done` already spins on
   `hlt`+deadline, `mod.rs:3245-3271`) — correct but latency-bound. **This is the metal-only risk; per
   discipline it is named, not designed around, and it is the first thing an attended bench must
   confirm.** Prior bench scars to watch: the ADDRESS_DEVICE code-4 saga and the `sp=1(FS)` mis-speed
   read (resume ledger) — enumeration-stage, but they show this controller's edge cases are real.

2. **IRQ-context wakeup vs. run-queue lock (the design's sharpest lock question).** If we ever let the
   *IRQ handler itself* wake the service task via `Semaphore::post` → `make_ready`, that takes
   `RUN_QUEUES[target].lock()` (`sched.rs:846`) in interrupt context — a self-deadlock if the interrupted
   core already holds that lock. The scheduler's stated invariant is that handlers "may only READ
   [current] and flip the task's atomic state; they never requeue or free" (`sched.rs:28`). **The design
   avoids this by keeping the IRQ handler wake-only (ack + EOI, exactly as today), and doing all wakeups
   from the service task at IF=1** — but this is a real constraint the code arc must not casually break,
   and §6 records it as a decision point.

3. **QEMU TCG timing masks the concurrency the metal path will expose.** Under RR/TCG, cores rarely
   interleave (the F2 witness already reports "RR-TCG did not interleave, so the race is metal-only",
   `fat.rs:350-353`). So S5's shared-backing race and S6's `FAT_MUTATION` witness will show *no* failure
   in the negative (unlocked) control under QEMU — the same honest-reporting problem F2 documented. The
   witnesses must be written to prove the *lock holds* (positive) and to report a zero-loss control
   honestly, with the true concurrency proof deferred to the bench. **QEMU-green ≠ correct** applies with
   full force to S5/S6.

---

## 6. Decisions the seat / Peter must make

1. **IRQ-context wakeup policy.** Keep the MSI-X handler strictly wake-from-`hlt` (recommended — no
   `make_ready` in interrupt context, matches the existing scheduler invariant `sched.rs:28`), or allow
   a direct `Semaphore::post` from the handler for lower completion latency? The recommended path costs a
   scheduling hop (IRQ wakes the core → service task runs → posts submitter) but keeps interrupt context
   lock-free. **Recommendation: wake-only.**

2. **`FAT_MUTATION` weight on x86** (§3.3). Mask IRQs across the RMW like aarch64 (`without_interrupts`,
   uniform with the pi4 discipline), or rely on the single-service-task-writer invariant for a lighter
   lock? The single-writer invariant is stronger on x86 than aarch64 (there is exactly one pump owner),
   so a non-masking lock may suffice — but uniformity with aarch64 has review value.

3. **Doc/dir placement** — this doc sits in `07_USB_STORAGE/`, not the brief's literal `07_STORAGE/`.
   Confirm or request a rename.

4. **Scope of the code arc(s).** The migration is 7 steps across the one-unmerged-arc-per-track cap;
   S1–S4 (the spine) is a natural first code arc, S5–S7 follow-ons. Confirm the split, or a tighter
   first arc (e.g. S1–S3 only, leaving allocator mutations staged one more arc).

5. **Metal gating.** Because transfer-IRQ I/O is unproven on the rMBP controller (risk 1), should S1 land
   behind a knob (e.g. `UNAOS_IRQSTORAGE=1`) that falls back to the current staged path until an attended
   bench confirms it, so the always-green QEMU chain and the metal boot media never regress?

---

## 7. What explicitly does NOT change

- The **aarch64 polled storage path** — EMMC2 PIO, the aarch64 `with_fat_lock`, the pi4 reaper — is
  untouched (`fat.rs:304-332`, aarch64-only).
- The **capability layer** — `handle_resolve` CHECK, rights/kind sidecars, U5x→U8x transfer/revoke,
  U6gx owner/grants ACL (`OWNED_FILES` `syscall.rs:4052`) — is untouched. Only byte-source and
  persistence-timing change.
- The **ring-3 ABI** — `SYS_*` numbers, object-table kinds, error codes — is unchanged.
- The **single-writer-per-row invariant** (SECURITY.md U7x) is preserved by construction (§3.2).
</content>
</invoke>
